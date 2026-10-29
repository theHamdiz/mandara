//! Resources plugin: CPU / memory rings in the bar; a system dashboard
//! panel with live CPU and memory sparklines, frequency / temperature /
//! load, per-GPU load / VRAM / temperature cards, component temperatures
//! (in the user's units) with heat bars, per-filesystem storage meters,
//! uptime and the busiest apps (click through to the process hunter); a
//! compact sidebar card.
//!
//! All sampling (process table, `/proc`, hwmon, statvfs) runs on the
//! background executor while the panel is on screen; the views follow
//! [`crate::hub::resources_entity`] for CPU / memory and are notified
//! directly when a panel sample lands. Every label is formatted once per
//! sample, not per frame, and the process-table baseline is dropped once
//! the panel has been closed for a while.
//!
//! `MANDARA_RESOURCES_DEMO=1` swaps every hardware reading for the calm,
//! fictional values in [`demo`] (docs screenshots).

pub(crate) mod demo;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, BackgroundExecutor, FontWeight, InteractiveElement, IntoElement, ObjectFit, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, StyledImage as _, Window, div, img, prelude::FluentBuilder as _, px, size,
};
use mandara_config::Units;
use mandara_core::{SpringConfig, ToGpui};
use mandara_i18n::{Lang, t, tf, tk, tr};
use mandara_plugins::{CommandSpec, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::processes::{Process, Sampler};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::fn_view;
use super::panel_fx::{
    HEADER_H, History, LINK_H_PX, LINK_HOVER_ALPHA, LINK_PAD_X_PX, PERCENT_STEP, PanelShape, Readout, SECTION_H_PX, SMALL_ICON_PX, ViewSet, chip,
    glass_card, header, meter, partner, percent_text, readable, redraw_in, section, sparkline,
};
use crate::fonts::{fonts, tabular_nums};
use crate::icons::{icon, ring};
use crate::kit::{card, reveal, text, tokens};
use crate::widgets::sensors::{self, Component, ComponentTemp, Gpu, Heat, Label};
use crate::widgets::system::{SystemSampler, disk_usage, format_bytes, format_uptime};

pub const ID: &str = "resources";

// ── Layout ───────────────────────────────────────────────────────────────

/// What a right click on the resources chip opens in the terminal.
const RESOURCE_MONITOR: &str = "btop";
const PANEL_W_PX: f32 = 396.0;
/// Panel content inset.
const PAD_PX: f32 = 14.0;
/// Vertical gap between panel blocks.
const GAP_PX: f32 = 14.0;
/// Gap between a section label and its content.
const SECTION_GAP_PX: f32 = 6.0;
/// Gap between the "TOP APPS" label and its rows.
const TOP_SECTION_GAP_PX: f32 = 4.0;
const TILE_H_PX: f32 = 142.0;
const DISK_H_PX: f32 = 40.0;
const PROC_H_PX: f32 = 30.0;
/// Busiest apps listed (`top_apps`; the sampler keeps the range's max).
pub const DEFAULT_TOP_APPS: i64 = 5;
pub const TOP_APPS_RANGE: (i64, i64) = (1, 10);
/// Filesystems listed in the panel, and in the sidebar card.
const MAX_DISKS: usize = 4;
const SIDEBAR_DISKS: usize = 2;
/// Filesystems smaller than this (boot / EFI partitions) are skipped.
const MIN_DISK_BYTES: u64 = 1_000_000_000;
/// Temperature cells: two columns, collapsed to this many until expanded.
const TEMP_CELL_H_PX: f32 = 44.0;
const TEMP_ROW_GAP_PX: f32 = 8.0;
const TEMP_COL_GAP_PX: f32 = 16.0;
const TEMP_COLUMNS: usize = 2;
/// Temperature cells before "show more" (`temps_shown`).
pub const DEFAULT_TEMPS_SHOWN: i64 = 8;
pub const TEMPS_SHOWN_RANGE: (i64, i64) = (2, 24);
/// GPU cards: two per row when there are several.
const GPU_GAP_PX: f32 = 10.0;
const GPU_BASE_H_PX: f32 = 96.0;
const GPU_VRAM_H_PX: f32 = 31.0;
const GPUS_PER_ROW: usize = 2;
/// Panel height changes smaller than this don't resize the panel.
const RESIZE_EPSILON_PX: f32 = 0.5;

// ── Cadence & memory ─────────────────────────────────────────────────────

/// Sparkline columns (≈ 54 s at the hub's 1.5 s cadence).
const HISTORY: usize = 36;
/// Minimum spacing of two sparkline columns.
const HISTORY_MIN_GAP: Duration = Duration::from_millis(1200);
/// Panel sample cadence in ms (process table, temperatures, GPUs;
/// `refresh_ms`).
pub const DEFAULT_REFRESH_MS: i64 = 1500;
pub const REFRESH_MS_RANGE: (i64, i64) = (500, 10_000);
/// Filesystem usage cadence in seconds while the panel is open
/// (`disks_every_s`).
pub const DEFAULT_DISKS_EVERY_S: i64 = 15;
pub const DISKS_EVERY_S_RANGE: (i64, i64) = (5, 300);
/// The very first process table has no CPU baseline: a second one follows
/// after this gap.
const PRIME_GAP: Duration = Duration::from_millis(250);
/// How often a closed panel is checked for, and how long it must have been
/// closed before the process-table baseline is dropped.
const CLOSED_CHECK: Duration = Duration::from_secs(5);
const CLOSED_AFTER: Duration = Duration::from_secs(10);
/// Sample gap of the one-shot `system resources` command.
const COMMAND_SAMPLE_GAP: Duration = Duration::from_millis(120);

// ── Bar chip: two rings with their percentages ───────────────────────────

/// Spring easing the chip rings toward large jumps.
const CHIP_SPRING: SpringConfig = SpringConfig::new(90.0, 19.0, 1.0);
/// Sample-to-sample moves below this fraction snap without animating: on a
/// busy machine CPU wanders 5–10 % per sample, and springing each wander ran
/// the whole bar at 60 fps for ~0.5 s of every 1.5 s sample.
const CHIP_SNAP_BELOW: f32 = 0.15;
const CHIP_RING_D_PX: f32 = 13.0;
const CHIP_RING_STROKE_PX: f32 = 2.0;
const CHIP_RING_GAP_PX: f32 = 6.0;
/// Fixed width of the percentage so `9%` → `10%` never shifts neighbours.
const CHIP_VALUE_W_PX: f32 = 28.0;
const CHIP_GAP_PX: f32 = 10.0;
const CHIP_TRACK_ALPHA: f32 = 0.12;

// ── Panel pieces ─────────────────────────────────────────────────────────

/// Gap between the CPU and memory tiles (and GPU cards in a row).
const TILE_GAP_PX: f32 = 10.0;
/// Icon / label row inside cards.
const ROW_GAP_PX: f32 = 6.0;
const TILE_ICON_PX: f32 = 14.0;
const CARD_ICON_PX: f32 = 13.0;
/// The big tile percentage, its line height, its top margin, and the
/// smaller `%` sign with its baseline nudge.
const TILE_VALUE_PX: f32 = 30.0;
const TILE_VALUE_LINE_PX: f32 = 34.0;
const TILE_VALUE_TOP_PX: f32 = 4.0;
const TILE_UNIT_PX: f32 = 14.0;
const TILE_UNIT_NUDGE_PX: f32 = 5.0;
/// Gap between the value and its unit.
const UNIT_GAP_PX: f32 = 2.0;
const TILE_SPARK_H_PX: f32 = 34.0;
/// Sparkline start alpha (the gradient ends opaque).
const SPARK_FROM_ALPHA: f32 = 0.75;
/// Secondary caption size (mount · fstype, temperature details).
const CAPTION_PX: f32 = 10.5;
const CAPTION_ALPHA: f32 = 0.8;
const DETAIL_ALPHA: f32 = 0.85;
/// Storage rows: icon gap, percentage width, meter height and the usage
/// fractions that turn the meter amber / coral.
const DISK_ROW_GAP_PX: f32 = 7.0;
const DISK_PERCENT_W_PX: f32 = 34.0;
const DISK_METER_H_PX: f32 = 6.0;
/// Filesystem fill (percent) above which it turns amber / coral
/// (`disk_warn_percent` / `disk_full_percent`).
pub const DEFAULT_DISK_WARN_PERCENT: i64 = 80;
pub const DEFAULT_DISK_FULL_PERCENT: i64 = 90;
pub const DISK_PERCENT_RANGE: (i64, i64) = (50, 99);
/// How far nearly-full meters blend toward the next warning colour.
const DISK_WARN_MIX: f32 = 0.45;
const DISK_FULL_MIX: f32 = 0.6;
/// Heat bars: the reading treated as fully cool, and the blends toward
/// amber / coral as a reading warms.
const COOL_FLOOR_C: f32 = 20.0;
const HEAT_HOT_MIX: f32 = 0.5;
const HEAT_WARM_START_MIX: f32 = 0.6;
const HEAT_WARM_END_MIX: f32 = 0.7;
const HEAT_COOL_END_MIX: f32 = 0.45;
/// Temperature cell rows.
const TEMP_LINE_GAP_PX: f32 = 2.0;
const TEMP_TITLE_H_PX: f32 = 16.0;
const TEMP_DETAIL_H_PX: f32 = 14.0;
const TEMP_METER_H_PX: f32 = 4.0;
/// GPU cards: big load text, its line height and unit nudge / size, the
/// load sparkline, the card's line heights and spacing, VRAM meter.
const GPU_LOAD_PX: f32 = 24.0;
const GPU_LOAD_LINE_PX: f32 = 28.0;
const GPU_UNIT_NUDGE_PX: f32 = 4.0;
const GPU_UNIT_GAP_PX: f32 = 1.0;
const GPU_SPARK_H_PX: f32 = 30.0;
const GPU_TITLE_H_PX: f32 = 16.0;
const GPU_SUB_H_PX: f32 = 14.0;
const GPU_SUB_TOP_PX: f32 = 2.0;
const GPU_LOAD_ROW_TOP_PX: f32 = 6.0;
const GPU_LOAD_ROW_H_PX: f32 = 34.0;
const GPU_VRAM_TOP_PX: f32 = 8.0;
const GPU_VRAM_METER_TOP_PX: f32 = 4.0;
const GPU_VRAM_METER_H_PX: f32 = 5.0;
const GPU_VRAM_ALPHA: f32 = 0.85;
/// MHz at and above which a GPU clock reads in GHz.
const MHZ_PER_GHZ: f32 = 1000.0;
/// Top-app rows: avatar, its radius / tint / initial size, gaps, the
/// count badge, the CPU meter and the value columns.
const AVATAR_PX: f32 = 18.0;
const AVATAR_RADIUS_PX: f32 = 6.0;
const AVATAR_TINT_ALPHA: f32 = 0.18;
const BADGE_TEXT_PX: f32 = 10.0;
const PROC_ROW_GAP_PX: f32 = 9.0;
const BADGE_PAD_X_PX: f32 = 5.0;
const BADGE_ALPHA: f32 = 0.07;
const PROC_METER_W_PX: f32 = 56.0;
const PROC_METER_H_PX: f32 = 4.0;
const PROC_METER_ALPHA: f32 = 0.8;
const PROC_CPU_W_PX: f32 = 44.0;
const PROC_RSS_W_PX: f32 = 62.0;
/// Letter-avatar colour: hash seed / multiplier and OKLCH lightness /
/// chroma (hue from the name).
const NAME_HASH_SEED: u32 = 7;
const NAME_HASH_MUL: u32 = 31;
const DEGREES: u32 = 360;
const AVATAR_L: f32 = 0.7;
const AVATAR_C: f32 = 0.12;
/// Gap between a link's label and its chevron.
const LINK_GAP_PX: f32 = 2.0;
/// Sidebar card sparkline height.
const SIDEBAR_SPARK_H_PX: f32 = 26.0;

/// Panel reveal order.
const REVEAL_HEADER: usize = 0;
const REVEAL_TILES: usize = 1;
const REVEAL_GPUS: usize = 2;
const REVEAL_TEMPS: usize = 3;
const REVEAL_STORAGE: usize = 4;
const REVEAL_TOP: usize = 5;

// ── Units ────────────────────────────────────────────────────────────────

/// `scaling_cur_freq` kHz per GHz.
const KHZ_PER_GHZ: f32 = 1e6;
/// hwmon reports milli-degrees Celsius.
const MILLI: f32 = 1000.0;
/// One decimal place for JSON readings.
const ROUND_TENTHS: f64 = 10.0;
/// `/proc/mounts` escapes: `\` plus three octal digits.
const OCTAL_DIGITS: usize = 3;
const OCTAL: u8 = 8;

/// `extra` is the height of the optional GPU + temperature sections.
fn panel_height(disks: usize, extra: f32, top_apps: usize) -> f32 {
    let storage = if disks == 0 { 0.0 } else { GAP_PX + SECTION_H_PX + SECTION_GAP_PX + DISK_H_PX * disks as f32 };
    PAD_PX * 2.0 + HEADER_H + GAP_PX + TILE_H_PX + extra + storage + GAP_PX + SECTION_H_PX + TOP_SECTION_GAP_PX + PROC_H_PX * top_apps as f32
}

/// Height of the temperature section showing `cells` components.
fn temps_height(cells: usize) -> f32 {
    if cells == 0 {
        return 0.0;
    }
    let rows = cells.div_ceil(TEMP_COLUMNS) as f32;
    GAP_PX + SECTION_H_PX + SECTION_GAP_PX + rows * TEMP_CELL_H_PX + (rows - 1.0) * TEMP_ROW_GAP_PX
}

fn gpu_card_h(g: &Gpu) -> f32 {
    GPU_BASE_H_PX + if g.vram_total.is_some() { GPU_VRAM_H_PX } else { 0.0 }
}

/// Card height of each GPU row (one card, or two side by side).
fn gpu_rows(gpus: &[Gpu]) -> Vec<f32> {
    if gpus.len() == 1 {
        return vec![gpu_card_h(&gpus[0])];
    }
    gpus.chunks(GPUS_PER_ROW).map(|pair| pair.iter().map(gpu_card_h).fold(0.0, f32::max)).collect()
}

fn gpus_height(gpus: &[Gpu]) -> f32 {
    if gpus.is_empty() {
        return 0.0;
    }
    let rows = gpu_rows(gpus);
    GAP_PX + SECTION_H_PX + SECTION_GAP_PX + rows.iter().sum::<f32>() + (rows.len() as f32 - 1.0) * GPU_GAP_PX
}

/// Number of temperature cells on screen.
fn temps_shown(total: usize, expanded: bool, limit: usize) -> usize {
    if expanded { total } else { total.min(limit) }
}

/// Percent → fraction.
const PERCENT_F: f32 = 100.0;

/// What the cached [`Labels`] were built from: sample generation, units,
/// language and the temperature-cell limit.
type LabelKey = (u64, Units, Lang, usize);

/// The panel's user settings (`[plugins.settings.resources]`), clamped.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Tuning {
    sample_every: Duration,
    disks_every: Duration,
    top_apps: usize,
    temps_shown: usize,
    disk_warn: f32,
    disk_full: f32,
}

impl Default for Tuning {
    fn default() -> Self {
        Self::from_plugins(&mandara_config::PluginsConfig::default())
    }
}

impl Tuning {
    fn from_plugins(p: &mandara_config::PluginsConfig) -> Self {
        let n = |key: &str, default: i64, (lo, hi): (i64, i64)| p.setting(ID, key, default).clamp(lo, hi);
        let refresh = p.setting(ID, "refresh_ms", DEFAULT_REFRESH_MS).clamp(REFRESH_MS_RANGE.0, REFRESH_MS_RANGE.1);
        let disks = p.setting(ID, "disks_every_s", DEFAULT_DISKS_EVERY_S).clamp(DISKS_EVERY_S_RANGE.0, DISKS_EVERY_S_RANGE.1);
        let warn = p.setting(ID, "disk_warn_percent", DEFAULT_DISK_WARN_PERCENT).clamp(DISK_PERCENT_RANGE.0, DISK_PERCENT_RANGE.1);
        let full = p.setting(ID, "disk_full_percent", DEFAULT_DISK_FULL_PERCENT).clamp(DISK_PERCENT_RANGE.0, DISK_PERCENT_RANGE.1);
        Self {
            sample_every: Duration::from_millis(refresh as u64),
            disks_every: Duration::from_secs(disks as u64),
            top_apps: p.setting(ID, "top_apps", DEFAULT_TOP_APPS).clamp(TOP_APPS_RANGE.0, TOP_APPS_RANGE.1) as usize,
            temps_shown: n("temps_shown", DEFAULT_TEMPS_SHOWN, TEMPS_SHOWN_RANGE) as usize,
            disk_warn: warn as f32 / PERCENT_F,
            disk_full: full.max(warn) as f32 / PERCENT_F,
        }
    }

    fn of(cx: &App) -> Self {
        Self::from_plugins(&crate::state::config(cx).read(cx).snapshot().plugins)
    }
}

/// CPU / memory / process sampling period (`refresh_ms`); the hub's bar
/// sampler follows it too.
pub fn sample_every(cx: &App) -> Duration {
    Tuning::of(cx).sample_every
}

// ── Sampling ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct Disk {
    pub mount: String,
    /// Display name; derived from the mount point when `None`.
    pub label: Option<String>,
    pub fstype: String,
    pub used: u64,
    pub total: u64,
}

/// One busy app: processes sharing a name, summed.
#[derive(Clone, Debug)]
struct TopApp {
    name: String,
    count: usize,
    /// Percent of the whole machine (all cores).
    cpu: f32,
    rss: u64,
    icon: Option<PathBuf>,
}

#[derive(Clone, Debug, Default)]
struct Snapshot {
    mem_used: u64,
    mem_total: u64,
    swap_used: u64,
    swap_total: u64,
    freq_ghz: Option<f32>,
    temp_c: Option<f32>,
    load: [f32; 3],
    threads: usize,
    top: Vec<TopApp>,
    temps: Vec<ComponentTemp>,
    gpus: Vec<Gpu>,
    /// The process table had a previous sample (CPU figures are real).
    primed: bool,
    /// Formatted uptime (`4d 2h`).
    uptime: Option<String>,
}

/// One storage row's text.
struct DiskLabels {
    label: SharedString,
    sub: SharedString,
    usage: SharedString,
    percent: SharedString,
    frac: f32,
}

struct TempLabels {
    name: SharedString,
    detail: SharedString,
    value: SharedString,
}

struct GpuLabels {
    full_name: SharedString,
    short_name: SharedString,
    sub: SharedString,
    temp: Option<(SharedString, Heat)>,
    load: SharedString,
    vram: Option<SharedString>,
}

struct TopLabels {
    name: SharedString,
    initial: SharedString,
    count: Option<SharedString>,
    cpu: SharedString,
    rss: SharedString,
    /// Avatar hue before the readability nudge.
    tint: Color,
}

/// Every string the panel and sidebar show, formatted once per sample (and
/// units / language) rather than on every frame of the open animation.
struct Labels {
    key: LabelKey,
    uptime: SharedString,
    cpu_corner: Option<SharedString>,
    cpu_detail: SharedString,
    mem_detail: SharedString,
    mem_corner: Option<SharedString>,
    disks: Vec<DiskLabels>,
    temps: Vec<TempLabels>,
    gpus: Vec<GpuLabels>,
    top: Vec<TopLabels>,
    temps_more: SharedString,
}

/// Panel-side state shared by the bar, panel and sidebar views.
struct Live {
    cpu: History,
    mem: History,
    snapshot: Snapshot,
    disks: Vec<Disk>,
    disks_at: Option<Instant>,
    sampled_at: Option<Instant>,
    in_flight: bool,
    /// Process-table baseline: created on the first panel sample, dropped
    /// once the panel has been closed for [`CLOSED_AFTER`].
    sampler: Option<Arc<Mutex<Sampler>>>,
    host: SharedString,
    /// GPU load sparklines keyed by PCI slot.
    gpu_load: Vec<(String, History)>,
    temps_expanded: bool,
    /// The user settings, refreshed on every panel render.
    tuning: Tuning,
    /// Bumped whenever sampled data changes (keys [`Labels`]).
    generation: u64,
    labels: Option<Labels>,
    /// The panel / sidebar views, notified when a sample lands.
    views: ViewSet,
    /// Last render of the open panel, and whether the close watchdog runs.
    panel_seen: Option<Instant>,
    reaper: bool,
}

impl Live {
    fn new() -> Self {
        let (mut cpu, mut mem) = (History::new(HISTORY), History::new(HISTORY));
        if demo::active() {
            let now = Instant::now();
            for (h, values) in [(&mut cpu, demo::cpu_history(HISTORY)), (&mut mem, demo::mem_history(HISTORY))] {
                for v in values {
                    h.push_every(v, Duration::ZERO, now);
                }
            }
        }
        Self {
            cpu,
            mem,
            snapshot: Snapshot::default(),
            disks: Vec::new(),
            disks_at: None,
            sampled_at: None,
            in_flight: false,
            sampler: None,
            host: SharedString::default(),
            gpu_load: Vec::new(),
            temps_expanded: false,
            tuning: Tuning::default(),
            generation: 0,
            labels: None,
            views: ViewSet::default(),
            panel_seen: None,
            reaper: false,
        }
    }

    /// Feeds each GPU's load into its sparkline; forgets GPUs that vanished.
    fn record_gpus(&mut self) {
        let now = Instant::now();
        let gpus = &self.snapshot.gpus;
        self.gpu_load.retain(|(slot, _)| gpus.iter().any(|g| g.slot == *slot));
        for g in gpus {
            let Some(load) = g.load else { continue };
            match self.gpu_load.iter_mut().find(|(slot, _)| *slot == g.slot) {
                Some((_, h)) => {
                    h.push_every(load, HISTORY_MIN_GAP, now);
                }
                None => {
                    let mut h = History::new(HISTORY);
                    if demo::active() {
                        for v in demo::gpu_history(&g.slot, HISTORY - 1).unwrap_or_default() {
                            h.push_every(v, Duration::ZERO, now);
                        }
                    }
                    h.push_every(load, Duration::ZERO, now);
                    self.gpu_load.push((g.slot.clone(), h));
                }
            }
        }
    }

    /// Height of the GPU and temperature sections.
    fn extra_height(&self) -> f32 {
        gpus_height(&self.snapshot.gpus) + temps_height(temps_shown(self.snapshot.temps.len(), self.temps_expanded, self.tuning.temps_shown))
    }

    /// Feeds the hub's smoothed CPU / memory into the sparklines.
    fn record(&mut self, cpu: f32, mem: f32) {
        let now = Instant::now();
        self.cpu.push_every(cpu, HISTORY_MIN_GAP, now);
        self.mem.push_every(mem, HISTORY_MIN_GAP, now);
    }

    /// The labels for the current sample, re-formatted only when the
    /// sample, units or language changed.
    fn labels(&mut self, units: Units) -> &Labels {
        let key = (self.generation, units, mandara_i18n::language(), self.tuning.temps_shown);
        if self.labels.as_ref().is_none_or(|l| l.key != key) {
            self.labels = Some(build_labels(key, &self.snapshot, &self.disks, units));
        }
        // Just filled above.
        self.labels.get_or_insert_with(|| build_labels(key, &self.snapshot, &self.disks, units))
    }
}

fn meminfo() -> (u64, u64, u64, u64) {
    if demo::active() {
        return demo::meminfo();
    }
    const KIB: u64 = 1024;
    let text = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let field = |name: &str| -> u64 {
        text.lines().find(|l| l.starts_with(name)).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()).unwrap_or(0) * KIB
    };
    let total = field("MemTotal:");
    let avail = field("MemAvailable:");
    let (st, sf) = (field("SwapTotal:"), field("SwapFree:"));
    (total.saturating_sub(avail), total, st.saturating_sub(sf), st)
}

fn cpu_freq_ghz() -> Option<f32> {
    if demo::active() {
        return Some(demo::cpu_freq_ghz());
    }
    // cpufreq is cheap and per-policy; fall back to /proc/cpuinfo.
    let mut sum = 0.0;
    let mut n = 0_u32;
    if let Ok(dir) = std::fs::read_dir("/sys/devices/system/cpu/cpufreq") {
        for e in dir.flatten() {
            if let Some(khz) = std::fs::read_to_string(e.path().join("scaling_cur_freq")).ok().and_then(|s| s.trim().parse::<f32>().ok()) {
                sum += khz / KHZ_PER_GHZ;
                n += 1;
            }
        }
    }
    if n == 0 {
        for l in std::fs::read_to_string("/proc/cpuinfo").ok()?.lines().filter(|l| l.starts_with("cpu MHz")) {
            if let Some(mhz) = l.split(':').nth(1).and_then(|v| v.trim().parse::<f32>().ok()) {
                sum += mhz / MHZ_PER_GHZ;
                n += 1;
            }
        }
    }
    (n > 0).then(|| sum / n as f32)
}

fn cpu_temp_c() -> Option<f32> {
    if demo::active() {
        return Some(demo::cpu_temp_c());
    }
    for e in std::fs::read_dir("/sys/class/hwmon").ok()?.flatten() {
        let name = std::fs::read_to_string(e.path().join("name")).unwrap_or_default();
        if matches!(name.trim(), "k10temp" | "coretemp" | "zenpower" | "cpu_thermal" | "acpitz")
            && let Some(m) = std::fs::read_to_string(e.path().join("temp1_input")).ok().and_then(|s| s.trim().parse::<f32>().ok())
        {
            return Some(m / MILLI);
        }
    }
    None
}

fn loadavg() -> [f32; 3] {
    if demo::active() {
        return demo::loadavg();
    }
    let text = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    let mut it = text.split_whitespace().filter_map(|v| v.parse::<f32>().ok());
    [it.next().unwrap_or(0.0), it.next().unwrap_or(0.0), it.next().unwrap_or(0.0)]
}

/// Decodes `/proc/mounts` octal escapes (`\040` → space).
fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let digits = b.get(i + 1..i + 1 + OCTAL_DIGITS);
        match digits {
            Some(d) if b[i] == b'\\' && d.iter().all(|c| (b'0'..=b'7').contains(c)) => {
                // Three octal digits fit a byte for every escape the kernel emits.
                out.push(d.iter().fold(0_u8, |acc, c| acc.wrapping_mul(OCTAL).wrapping_add(c - b'0')));
                i += 1 + OCTAL_DIGITS;
            }
            _ => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Local block-device filesystems from `/proc/mounts` text, one per device
/// (btrfs subvolumes collapse onto their shortest mount point).
pub fn parse_mounts(text: &str) -> Vec<(String, String, String)> {
    const LOCAL: &[&str] = &["ext2", "ext3", "ext4", "btrfs", "xfs", "f2fs", "zfs", "bcachefs", "ntfs", "ntfs3", "exfat", "vfat", "jfs", "reiserfs"];
    const SKIPPED: &[&str] = &["/snap/", "/var/lib/docker", "/run/", "/boot", "/efi"];
    let mut out: Vec<(String, String, String)> = Vec::new();
    for line in text.lines() {
        let mut f = line.split_whitespace();
        let (Some(dev), Some(mount), Some(fs)) = (f.next(), f.next(), f.next()) else { continue };
        if !LOCAL.contains(&fs) || !dev.starts_with('/') {
            continue;
        }
        let mount = unescape(mount);
        if SKIPPED.iter().any(|p| mount.starts_with(p)) {
            continue;
        }
        match out.iter_mut().find(|(d, _, _)| d == dev) {
            Some(existing) if mount.len() < existing.1.len() => existing.1 = mount,
            Some(_) => {}
            None => out.push((dev.to_string(), mount, fs.to_string())),
        }
    }
    out
}

/// Filesystem usage (blocking: one statvfs per mount — background only).
fn read_disks() -> Vec<Disk> {
    if demo::active() {
        return demo::disks();
    }
    let mut disks: Vec<Disk> = Vec::new();
    for (_, mount, fstype) in parse_mounts(&std::fs::read_to_string("/proc/mounts").unwrap_or_default()) {
        let Some((used, total)) = disk_usage(&mount) else { continue };
        // Skip tiny boot / EFI partitions and filesystems already listed.
        if total < MIN_DISK_BYTES || disks.iter().any(|d| (d.used, d.total) == (used, total)) {
            continue;
        }
        disks.push(Disk { mount, label: None, fstype, used, total });
    }
    // System first, then home, then by size.
    disks.sort_by_key(|d| (d.mount != "/", d.mount != "/home", std::cmp::Reverse(d.total)));
    disks.truncate(MAX_DISKS);
    disks
}

/// `hostname · Linux 6.x` (blocking procfs reads — background only).
fn host_line() -> String {
    let host = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default().trim().to_string();
    let kernel = if demo::active() {
        demo::KERNEL.to_string()
    } else {
        std::fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default().trim().split('-').next().unwrap_or_default().to_string()
    };
    match (host.is_empty(), kernel.is_empty()) {
        (false, false) => format!("{host} · Linux {kernel}"),
        (false, true) => host,
        _ => format!("Linux {kernel}"),
    }
}

fn threads() -> usize {
    if demo::active() {
        return demo::threads();
    }
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1)
}

fn top_apps(procs: &[Process], threads: usize) -> Vec<TopApp> {
    let mut groups: Vec<TopApp> = Vec::new();
    for p in procs {
        match groups.iter_mut().find(|g| g.name == p.name) {
            Some(g) => {
                g.count += 1;
                g.cpu += p.cpu;
                g.rss += p.rss;
            }
            None => groups.push(TopApp { name: p.name.clone(), count: 1, cpu: p.cpu, rss: p.rss, icon: None }),
        }
    }
    for g in &mut groups {
        g.cpu /= threads.max(1) as f32;
    }
    groups.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.rss.cmp(&a.rss)));
    groups.truncate(TOP_APPS_RANGE.1 as usize);
    for g in &mut groups {
        g.icon = crate::apps::icon_for_app_id(&g.name, false).map(|r| r.path);
    }
    groups
}

/// One panel sample on the background executor. The first process table
/// has no CPU baseline, so a second one follows after [`PRIME_GAP`] (an
/// executor timer — no worker thread is put to sleep).
async fn collect(sampler: Arc<Mutex<Sampler>>, executor: BackgroundExecutor) -> Snapshot {
    let threads = threads();
    let sample = || sampler.lock().map(|mut s| s.sample()).ok();
    let (procs, primed) = match sample() {
        Some(first) if first.iter().all(|p| p.cpu == 0.0) => {
            executor.timer(PRIME_GAP).await;
            (sample().unwrap_or_default(), true)
        }
        Some(first) => (first, true),
        None => (Vec::new(), false),
    };
    let (mem_used, mem_total, swap_used, swap_total) = meminfo();
    let (temps, gpus) = hardware();
    let temp_c = sensors::cpu_temperature(&temps).or_else(cpu_temp_c);
    Snapshot {
        mem_used,
        mem_total,
        swap_used,
        swap_total,
        freq_ghz: cpu_freq_ghz(),
        temp_c,
        load: loadavg(),
        threads,
        top: top_apps(&procs, threads),
        temps,
        gpus,
        primed,
        uptime: uptime(),
    }
}

/// Component temperatures and GPUs (blocking; may run `nvidia-smi`).
fn hardware() -> (Vec<ComponentTemp>, Vec<Gpu>) {
    if demo::active() {
        return (demo::temperatures(), demo::gpus());
    }
    (sensors::read_temperatures(std::path::Path::new("/")), sensors::read_all_gpus())
}

/// Formatted uptime (`4d 2h`), or the demo's.
fn uptime() -> Option<String> {
    if demo::active() { Some(format_uptime(demo::uptime_secs())) } else { crate::widgets::system::uptime() }
}

/// Starts a background sample when one is due; notifies the plugin's
/// views when it lands.
fn refresh(live: &Rc<RefCell<Live>>, cx: &mut App) {
    let (sampler, want_disks) = {
        let mut l = live.borrow_mut();
        let tuning = l.tuning;
        if l.in_flight || l.sampled_at.is_some_and(|t| t.elapsed() < tuning.sample_every) {
            return;
        }
        l.in_flight = true;
        let sampler = l.sampler.get_or_insert_with(|| Arc::new(Mutex::new(Sampler::new()))).clone();
        (sampler, l.disks_at.is_none_or(|t| t.elapsed() >= tuning.disks_every))
    };
    let executor = cx.background_executor().clone();
    let task = cx.background_executor().spawn(async move {
        let snapshot = collect(sampler, executor).await;
        (snapshot, want_disks.then(read_disks))
    });
    let live = live.clone();
    cx.spawn(async move |cx| {
        let (snapshot, disks) = task.await;
        let views = {
            let mut l = live.borrow_mut();
            l.snapshot = snapshot;
            if let Some(disks) = disks {
                l.disks = disks;
                l.disks_at = Some(Instant::now());
            }
            l.sampled_at = Some(Instant::now());
            l.in_flight = false;
            l.generation += 1;
            l.record_gpus();
            l.views.clone()
        };
        views.notify_async(cx);
    })
    .detach();
}

/// Marks the panel as on screen and, once, starts a watchdog that drops
/// the process-table baseline after the panel has been closed a while.
fn panel_seen(live: &Rc<RefCell<Live>>, cx: &mut App) {
    {
        let mut l = live.borrow_mut();
        l.panel_seen = Some(Instant::now());
        if l.reaper {
            return;
        }
        l.reaper = true;
    }
    let weak = Rc::downgrade(live);
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(CLOSED_CHECK).await;
            let Some(live) = weak.upgrade() else { break };
            let mut l = live.borrow_mut();
            if l.panel_seen.is_none_or(|t| t.elapsed() >= CLOSED_AFTER) {
                l.sampler = None;
                l.reaper = false;
                break;
            }
        }
    })
    .detach();
}

// ── Labels ───────────────────────────────────────────────────────────────

/// Percent numbers `0..=100` without the sign (the tiles draw a smaller
/// `%`), formatted once per thread.
fn percent_number(frac: f32) -> SharedString {
    const STEPS: usize = 101;
    thread_local! {
        static NUMBERS: Vec<SharedString> = (0..STEPS).map(|p| SharedString::from(p.to_string())).collect();
    }
    let i = (frac.clamp(0.0, 1.0) / PERCENT_STEP).round() as usize;
    NUMBERS.with(|n| n[i.min(STEPS - 1)].clone())
}

fn disk_label(mount: &str) -> String {
    match mount {
        "/" => t("System").into(),
        "/home" => t("Home").into(),
        m => m.rsplit('/').find(|s| !s.is_empty()).unwrap_or(m).to_string(),
    }
}

/// A stable, pleasant per-name colour for letter avatars.
fn name_tint(name: &str) -> Color {
    let hue = name.bytes().fold(NAME_HASH_SEED, |h, b| h.wrapping_mul(NAME_HASH_MUL).wrapping_add(u32::from(b))) % DEGREES;
    Color::oklch(AVATAR_L, AVATAR_C, hue as f32)
}

fn build_labels(key: LabelKey, snap: &Snapshot, disks: &[Disk], units: Units) -> Labels {
    let cpu_corner = match (snap.freq_ghz, snap.temp_c) {
        (Some(f), Some(t)) => Some(format!("{f:.1} GHz · {}", units.format_temperature(t)).into()),
        (Some(f), None) => Some(format!("{f:.1} GHz").into()),
        (None, Some(t)) => Some(units.format_temperature(t).into()),
        _ => None,
    };
    let [l1, l5, l15] = snap.load;
    let cpu_detail = if snap.threads > 0 { tf("load {load}", &[("load", &format!("{l1:.2} · {l5:.2} · {l15:.2}"))]).into() } else { t("sampling…").into() };
    let mem_detail = if snap.mem_total > 0 {
        tf("{used} of {total}", &[("used", &format_bytes(snap.mem_used)), ("total", &format_bytes(snap.mem_total))]).into()
    } else {
        t("sampling…").into()
    };
    let mem_corner = (snap.swap_total > 0).then(|| tf("swap {size}", &[("size", &format_bytes(snap.swap_used))]).into());
    let disks = disks
        .iter()
        .map(|d| DiskLabels {
            label: d.label.clone().unwrap_or_else(|| disk_label(&d.mount)).into(),
            sub: format!("{} · {}", d.mount, d.fstype).into(),
            usage: format!("{} / {}", format_bytes(d.used), format_bytes(d.total)).into(),
            frac: d.used as f32 / d.total.max(1) as f32,
            percent: percent_text(d.used as f32 / d.total.max(1) as f32),
        })
        .collect();
    let temps = snap
        .temps
        .iter()
        .map(|c| {
            let name = c.name.text();
            let detail = if !c.readings.is_empty() {
                c.readings.iter().map(|r| format!("{} {}", r.label.text(), units.format_degrees(r.celsius))).collect::<Vec<_>>().join(" · ")
            } else {
                c.device.clone().filter(|d| *d != name).unwrap_or_else(|| tr(c.kind.kind_label()))
            };
            TempLabels { name: name.into(), detail: detail.into(), value: units.format_temperature(c.celsius).into() }
        })
        .collect();
    let gpus = snap
        .gpus
        .iter()
        .map(|g| {
            let full = g.display_name();
            let kind = if g.integrated { t("Integrated") } else { t("Discrete") };
            let sub = match g.freq_mhz {
                Some(f) if f >= MHZ_PER_GHZ => format!("{kind} · {:.2} GHz", f / MHZ_PER_GHZ),
                Some(f) => format!("{kind} · {f:.0} MHz"),
                None => kind.to_string(),
            };
            let temp = g.temp_c.map(|c| {
                let (warm, hot) = Component::Gpu.thresholds();
                let heat = if c >= hot { Heat::Hot } else if c >= warm { Heat::Warm } else { Heat::Cool };
                (SharedString::from(units.format_temperature(c)), heat)
            });
            GpuLabels {
                short_name: sensors::short_gpu_name(&full).into(),
                full_name: full.into(),
                sub: sub.into(),
                temp,
                load: g.load.map(percent_number).unwrap_or_else(|| "–".into()),
                vram: g.vram_total.zip(g.vram_used).map(|(total, used)| format!("{} / {}", format_bytes(used), format_bytes(total)).into()),
            }
        })
        .collect();
    let top = snap
        .top
        .iter()
        .map(|a| TopLabels {
            name: a.name.clone().into(),
            initial: a.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default().into(),
            count: (a.count > 1).then(|| format!("×{}", a.count).into()),
            cpu: if snap.primed { format!("{:.1}%", a.cpu).into() } else { "–".into() },
            rss: format_bytes(a.rss).into(),
            tint: name_tint(&a.name),
        })
        .collect();
    let hidden = snap.temps.len().saturating_sub(key.3);
    Labels {
        key,
        uptime: tf("up {time}", &[("time", &snap.uptime.as_deref().unwrap_or_default())]).into(),
        cpu_corner,
        cpu_detail,
        mem_detail,
        mem_corner,
        disks,
        temps,
        gpus,
        top,
        temps_more: tf("+{n} more", &[("n", &hidden)]).into(),
    }
}

// ── Views ────────────────────────────────────────────────────────────────

fn colors(palette: &MandaraPalette) -> [(Color, Color); 3] {
    let pair = |c: Color| (readable(c, palette), readable(partner(c), palette));
    [pair(palette.primary), pair(palette.tertiary), pair(palette.secondary)]
}

#[allow(clippy::too_many_arguments)]
fn tile(
    icon_name: &'static str,
    label: &'static str,
    corner: Option<SharedString>,
    value: f32,
    detail: SharedString,
    history: &History,
    (from, to): (Color, Color),
    palette: &MandaraPalette,
    display: SharedString,
    radius: f32,
) -> impl IntoElement {
    glass_card(palette)
        .rounded(px(radius))
        .flex_1()
        .min_w(px(0.0))
        .h(px(TILE_H_PX))
        .p(px(tokens::MD))
        .flex()
        .flex_col()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(ROW_GAP_PX))
                .child(icon(icon_name, TILE_ICON_PX, from))
                .child(div().flex_1().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(palette.on_surface_variant.hsla()).child(label))
                .when_some(corner, |el, c| {
                    el.child(div().text_size(px(text::LABEL)).font_features(tabular_nums()).text_color(palette.on_surface_variant.hsla()).child(c))
                }),
        )
        .child(
            div()
                .mt(px(TILE_VALUE_TOP_PX))
                .flex()
                .items_end()
                .gap(px(UNIT_GAP_PX))
                .font_family(display)
                .font_features(tabular_nums())
                .child(div().text_size(px(TILE_VALUE_PX)).line_height(px(TILE_VALUE_LINE_PX)).font_weight(FontWeight::MEDIUM).child(percent_number(value)))
                .child(div().pb(px(TILE_UNIT_NUDGE_PX)).text_size(px(TILE_UNIT_PX)).font_weight(FontWeight::MEDIUM).text_color(palette.on_surface_variant.hsla()).child("%")),
        )
        .child(div().text_size(px(text::LABEL)).font_features(tabular_nums()).text_color(palette.on_surface_variant.hsla()).truncate().child(detail))
        .child(div().flex_1())
        .child(sparkline(history, TILE_SPARK_H_PX, from.with_alpha(SPARK_FROM_ALPHA), to, palette))
}

#[allow(clippy::too_many_arguments)]
fn tiles(live: &Live, labels: &Labels, cpu: f32, mem: f32, palette: &MandaraPalette, display: SharedString, radius: f32) -> impl IntoElement {
    let [cpu_c, mem_c, _] = colors(palette);
    div()
        .flex()
        .gap(px(TILE_GAP_PX))
        .child(tile("cpu", t("Processor"), labels.cpu_corner.clone(), cpu, labels.cpu_detail.clone(), &live.cpu, cpu_c, palette, display.clone(), radius))
        .child(tile("memory", t("Memory"), labels.mem_corner.clone(), mem, labels.mem_detail.clone(), &live.mem, mem_c, palette, display, radius))
}

fn storage(disks: &[DiskLabels], tuning: Tuning, palette: &MandaraPalette) -> impl IntoElement {
    let [_, _, (from, to)] = colors(palette);
    div().flex().flex_col().children(disks.iter().map(move |d| {
        // Nearly-full filesystems shift toward the warning colours (raw
        // tokens for the fill; the icon gets the legible variant).
        let (a, b) = if d.frac > tuning.disk_full {
            (palette.urgent_coral, palette.urgent_coral.mix(palette.warning_amber, DISK_FULL_MIX))
        } else if d.frac > tuning.disk_warn {
            (palette.warning_amber, palette.warning_amber.mix(palette.urgent_coral, DISK_WARN_MIX))
        } else {
            (from, to)
        };
        div()
            .h(px(DISK_H_PX))
            .flex()
            .flex_col()
            .justify_center()
            .gap(px(ROW_GAP_PX))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(DISK_ROW_GAP_PX))
                    .child(icon("disk", TILE_ICON_PX, readable(a, palette)))
                    .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).child(d.label.clone()))
                    .child(div().text_size(px(CAPTION_PX)).text_color(palette.on_surface_variant.with_alpha(CAPTION_ALPHA).hsla()).child(d.sub.clone()))
                    .child(div().flex_1())
                    .child(div().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(d.usage.clone()))
                    .child(
                        div()
                            .w(px(DISK_PERCENT_W_PX))
                            .flex()
                            .justify_end()
                            .font_features(tabular_nums())
                            .text_size(px(text::BODY_S))
                            .font_weight(FontWeight::SEMIBOLD)
                            .child(d.percent.clone()),
                    ),
            )
            .child(meter(d.frac, DISK_METER_H_PX, a, b, palette))
    }))
}

/// Heat-bar gradient: emerald while cool, shading through amber toward
/// coral as a reading approaches its component's thresholds (raw tokens for
/// the fill; text gets the legible variant).
fn heat_gradient(celsius: f32, warm: f32, hot: f32, palette: &MandaraPalette) -> (Color, Color) {
    let (ok, warn, bad) = (palette.success_emerald, palette.warning_amber, palette.urgent_coral);
    if celsius >= hot {
        (warn.mix(bad, HEAT_HOT_MIX), bad)
    } else if celsius >= warm {
        let k = ((celsius - warm) / (hot - warm).max(1.0)).clamp(0.0, 1.0);
        (ok.mix(warn, HEAT_WARM_START_MIX), warn.mix(bad, k * HEAT_WARM_END_MIX))
    } else {
        let k = ((celsius - COOL_FLOOR_C) / (warm - COOL_FLOOR_C).max(1.0)).clamp(0.0, 1.0);
        (ok, ok.mix(warn, k * HEAT_COOL_END_MIX))
    }
}

/// Colour for a temperature readout: plain text while cool.
fn heat_text(heat: Heat, palette: &MandaraPalette) -> gpui::Hsla {
    match heat {
        Heat::Cool => palette.on_surface.hsla(),
        Heat::Warm => readable(palette.warning_amber, palette).hsla(),
        Heat::Hot => readable(palette.urgent_coral, palette).hsla(),
    }
}

fn component_icon(c: &ComponentTemp) -> &'static str {
    match (&c.kind, &c.name) {
        (Component::Network, Label::Key(k)) if *k == "Wi-Fi" => "wifi-4",
        (kind, _) => kind.icon(),
    }
}

/// One temperature: icon, name and value; secondary readings (or the
/// device) underneath; a heat bar.
fn temp_cell(c: &ComponentTemp, l: &TempLabels, palette: &MandaraPalette) -> impl IntoElement {
    let (from, to) = heat_gradient(c.celsius, c.warm, c.hot, palette);
    div()
        .flex_1()
        .min_w(px(0.0))
        .h(px(TEMP_CELL_H_PX))
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(TEMP_LINE_GAP_PX))
        .child(
            div()
                .h(px(TEMP_TITLE_H_PX))
                .flex()
                .items_center()
                .gap(px(ROW_GAP_PX))
                .child(icon(component_icon(c), CARD_ICON_PX, readable(from, palette)))
                .child(div().flex_1().min_w(px(0.0)).text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).truncate().child(l.name.clone()))
                .child(
                    div()
                        .flex_none()
                        .font_features(tabular_nums())
                        .text_size(px(text::BODY_S))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(heat_text(c.heat(), palette))
                        .child(l.value.clone()),
                ),
        )
        .child(
            div()
                .h(px(TEMP_DETAIL_H_PX))
                .line_height(px(TEMP_DETAIL_H_PX))
                .text_size(px(CAPTION_PX))
                .font_features(tabular_nums())
                .text_color(palette.on_surface_variant.with_alpha(DETAIL_ALPHA).hsla())
                .truncate()
                .child(l.detail.clone()),
        )
        .child(meter(c.fraction(), TEMP_METER_H_PX, from, to, palette))
}

fn temperatures(temps: &[ComponentTemp], labels: &[TempLabels], shown: usize, palette: &MandaraPalette) -> impl IntoElement {
    let n = shown.min(temps.len()).min(labels.len());
    let cells = temps[..n].iter().zip(&labels[..n]).collect::<Vec<_>>();
    div().flex().flex_col().gap(px(TEMP_ROW_GAP_PX)).children(cells.chunks(TEMP_COLUMNS).map(|pair| {
        div()
            .flex()
            .gap(px(TEMP_COL_GAP_PX))
            .children(pair.iter().map(|(c, l)| temp_cell(c, l, palette)))
            .when(pair.len() == 1, |el| el.child(div().flex_1()))
    }))
}

/// A small text link pill (`+3 more`, `All processes`).
fn link(id: &'static str, palette: &MandaraPalette) -> gpui::Stateful<gpui::Div> {
    let fg = readable(palette.primary, palette);
    let hover = fg.with_alpha(LINK_HOVER_ALPHA);
    div()
        .id(id)
        .h(px(LINK_H_PX))
        .px(px(LINK_PAD_X_PX))
        .rounded_full()
        .flex()
        .items_center()
        .cursor_pointer()
        .text_size(px(text::LABEL))
        .font_weight(FontWeight::MEDIUM)
        .text_color(fg.hsla())
        .hover(move |s| s.bg(hover.hsla()))
}

/// The "+3 more" / "Show less" link beside the temperatures label.
fn temps_toggle(live: &Rc<RefCell<Live>>, more: SharedString, expanded: bool, palette: &MandaraPalette) -> AnyElement {
    let live = live.clone();
    link("resources-temps-more", palette)
        .font_features(tabular_nums())
        .child(if expanded { SharedString::from(t("Show less")) } else { more })
        .on_click(move |_, _, cx| {
            let views = {
                let mut l = live.borrow_mut();
                l.temps_expanded = !l.temps_expanded;
                l.views.clone()
            };
            views.notify(cx);
        })
        .into_any_element()
}

/// One GPU: name, kind · clock and temperature, load with a sparkline and
/// a VRAM meter when the driver reports one.
#[allow(clippy::too_many_arguments)]
fn gpu_card(g: &Gpu, l: &GpuLabels, history: Option<&History>, compact: bool, height: f32, palette: &MandaraPalette, display: SharedString, radius: f32) -> impl IntoElement {
    let accent = readable(if g.integrated { palette.tertiary } else { palette.primary }, palette);
    let temp = l.temp.as_ref().map(|(text, heat)| div().flex_none().font_features(tabular_nums()).font_weight(FontWeight::SEMIBOLD).text_color(heat_text(*heat, palette)).child(text.clone()));
    let load = div()
        .flex_none()
        .flex()
        .items_end()
        .gap(px(GPU_UNIT_GAP_PX))
        .font_family(display)
        .font_features(tabular_nums())
        .child(div().text_size(px(GPU_LOAD_PX)).line_height(px(GPU_LOAD_LINE_PX)).font_weight(FontWeight::MEDIUM).child(l.load.clone()))
        .when(g.load.is_some(), |el| {
            el.child(div().pb(px(GPU_UNIT_NUDGE_PX)).text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(palette.on_surface_variant.hsla()).child("%"))
        });
    let graph = match (history, g.load) {
        (Some(h), Some(_)) => sparkline(h, GPU_SPARK_H_PX, accent.with_alpha(SPARK_FROM_ALPHA), partner(accent), palette).into_any_element(),
        _ => div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(t("No load data")).into_any_element(),
    };
    glass_card(palette)
        .rounded(px(radius))
        .flex_1()
        .min_w(px(0.0))
        .h(px(height))
        .p(px(tokens::MD))
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(GPU_TITLE_H_PX))
                .flex()
                .items_center()
                .gap(px(ROW_GAP_PX))
                .child(icon("monitor", CARD_ICON_PX, accent))
                .child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .text_size(px(text::BODY_S))
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(if compact { l.short_name.clone() } else { l.full_name.clone() }),
                ),
        )
        .child(
            div()
                .mt(px(GPU_SUB_TOP_PX))
                .h(px(GPU_SUB_H_PX))
                .flex()
                .items_center()
                .gap(px(ROW_GAP_PX))
                .text_size(px(text::LABEL))
                .child(div().flex_1().min_w(px(0.0)).font_features(tabular_nums()).text_color(palette.on_surface_variant.hsla()).truncate().child(l.sub.clone()))
                .children(temp),
        )
        .child(
            div()
                .mt(px(GPU_LOAD_ROW_TOP_PX))
                .h(px(GPU_LOAD_ROW_H_PX))
                .flex()
                .items_center()
                .gap(px(TILE_GAP_PX))
                .child(load)
                .child(div().flex_1().min_w(px(0.0)).flex().justify_end().child(graph)),
        )
        .when_some(g.vram_total.zip(g.vram_used).zip(l.vram.clone()), |el, ((total, used), usage)| {
            let frac = used as f32 / total.max(1) as f32;
            el.child(
                div()
                    .mt(px(GPU_VRAM_TOP_PX))
                    .h(px(GPU_SUB_H_PX))
                    .flex()
                    .items_center()
                    .gap(px(ROW_GAP_PX))
                    .text_size(px(text::LABEL))
                    .child(div().flex_none().font_weight(FontWeight::MEDIUM).text_color(palette.on_surface_variant.hsla()).child(t("VRAM")))
                    .child(div().flex_1())
                    .child(div().min_w(px(0.0)).font_features(tabular_nums()).text_color(palette.on_surface_variant.hsla()).truncate().child(usage)),
            )
            .child(div().mt(px(GPU_VRAM_METER_TOP_PX)).child(meter(frac, GPU_VRAM_METER_H_PX, accent.with_alpha(GPU_VRAM_ALPHA), partner(accent), palette)))
        })
}

fn gpus_section(live: &Live, labels: &[GpuLabels], palette: &MandaraPalette, display: SharedString, radius: f32) -> impl IntoElement {
    let gpus = &live.snapshot.gpus;
    let compact = gpus.len() > 1;
    let per_row = if compact { GPUS_PER_ROW } else { 1 };
    let rows = gpu_rows(gpus);
    let history = |g: &Gpu| live.gpu_load.iter().find(|(slot, _)| *slot == g.slot).map(|(_, h)| h);
    div().flex().flex_col().gap(px(GPU_GAP_PX)).children(gpus.chunks(per_row).zip(labels.chunks(per_row)).zip(rows).map(|((pair, names), h)| {
        div()
            .flex()
            .gap(px(TILE_GAP_PX))
            .children(pair.iter().zip(names).map(|(g, l)| gpu_card(g, l, history(g), compact, h, palette, display.clone(), radius)))
            .when(compact && pair.len() == 1, |el| el.child(div().flex_1()))
    }))
}

fn processes(snap: &Snapshot, labels: &[TopLabels], top_apps: usize, palette: &MandaraPalette) -> impl IntoElement {
    let [(from, to), _, _] = colors(palette);
    let max = snap.top.iter().map(|a| a.cpu).fold(1.0_f32, f32::max);
    let rows: Vec<AnyElement> = if snap.top.is_empty() {
        vec![
            div()
                .h(px(PROC_H_PX * top_apps as f32))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(text::BODY_S))
                .text_color(palette.on_surface_variant.hsla())
                .child(t("Measuring…"))
                .into_any_element(),
        ]
    } else {
        snap.top
            .iter()
            .take(top_apps)
            .zip(labels)
            .map(|(a, l)| {
                let avatar = match &a.icon {
                    Some(path) => div().size(px(AVATAR_PX)).flex_none().child(img(path.clone()).size_full().object_fit(ObjectFit::Contain)),
                    None => {
                        let tint = readable(l.tint, palette);
                        div()
                            .size(px(AVATAR_PX))
                            .flex_none()
                            .rounded(px(crate::style::radius(AVATAR_RADIUS_PX)))
                            .bg(tint.with_alpha(AVATAR_TINT_ALPHA).hsla())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(BADGE_TEXT_PX))
                            .font_weight(FontWeight::BOLD)
                            .text_color(tint.hsla())
                            .child(l.initial.clone())
                    }
                };
                div()
                    .h(px(PROC_H_PX))
                    .flex()
                    .items_center()
                    .gap(px(PROC_ROW_GAP_PX))
                    .child(avatar)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.0))
                            .flex()
                            .items_center()
                            .gap(px(ROW_GAP_PX))
                            .child(div().min_w(px(0.0)).text_size(px(text::BODY_S)).truncate().child(l.name.clone()))
                            .when_some(l.count.clone(), |el, count| {
                                el.child(
                                    div()
                                        .flex_none()
                                        .px(px(BADGE_PAD_X_PX))
                                        .rounded_full()
                                        .bg(palette.on_surface.with_alpha(BADGE_ALPHA).hsla())
                                        .text_size(px(BADGE_TEXT_PX))
                                        .font_features(tabular_nums())
                                        .text_color(palette.on_surface_variant.hsla())
                                        .child(count),
                                )
                            }),
                    )
                    .child(div().w(px(PROC_METER_W_PX)).flex_none().child(meter((a.cpu / max).min(1.0), PROC_METER_H_PX, from.with_alpha(PROC_METER_ALPHA), to, palette)))
                    .child(
                        div()
                            .w(px(PROC_CPU_W_PX))
                            .flex()
                            .justify_end()
                            .font_features(tabular_nums())
                            .text_size(px(text::BODY_S))
                            .font_weight(FontWeight::MEDIUM)
                            .child(l.cpu.clone()),
                    )
                    .child(
                        div()
                            .w(px(PROC_RSS_W_PX))
                            .flex()
                            .justify_end()
                            .font_features(tabular_nums())
                            .text_size(px(text::LABEL))
                            .text_color(palette.on_surface_variant.hsla())
                            .child(l.rss.clone()),
                    )
                    .into_any_element()
            })
            .collect()
    };
    div().flex().flex_col().children(rows)
}

fn open_hunter_link(palette: &MandaraPalette) -> AnyElement {
    let fg = readable(palette.primary, palette);
    link("resources-open-processes", palette)
        .gap(px(LINK_GAP_PX))
        .child(t("All processes"))
        .child(icon("chevron-right", SMALL_ICON_PX, fg))
        .on_click(|_, _, cx| {
            crate::state::shell(cx).update(cx, |s, cx| s.set_panel(Some(super::processes::ID.into()), cx));
        })
        .into_any_element()
}

/// A labelled panel section.
fn titled(label: &'static str, gap: f32, palette: &MandaraPalette, trailing: Option<AnyElement>, body: impl IntoElement) -> gpui::Div {
    div().flex().flex_col().gap(px(gap)).child(section(label, palette, trailing)).child(body)
}

fn panel_view(p: PanelCx, live: Rc<RefCell<Live>>, size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>) -> impl Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement {
    move |palette, window, cx| {
        let (cpu, mem) = {
            let s = crate::hub::resources_entity(cx).read(cx);
            (s.cpu, s.mem)
        };
        live.borrow().views.track(window);
        if p.progress() > 0.0 {
            live.borrow_mut().tuning = Tuning::of(cx);
            panel_seen(&live, cx);
            refresh(&live, cx);
            redraw_in(live.borrow().tuning.sample_every, window, cx);
        }
        let units = crate::state::config(cx).read(cx).get().units();
        let display = fonts(cx).display;
        let radius = PanelShape::of(cx).inner(PAD_PX);
        let mut guard = live.borrow_mut();
        guard.record(cpu, mem);
        let h = panel_height(guard.disks.len(), guard.extra_height(), guard.tuning.top_apps);
        if (f32::from(size_cell.get().height) - h).abs() > RESIZE_EPSILON_PX {
            size_cell.set(size(px(PANEL_W_PX), px(h)));
        }
        guard.labels(units);
        let l = &*guard;
        let Some(labels) = l.labels.as_ref() else { return div().into_any_element() };
        let accent = readable(palette.primary, palette);
        let mut col = div()
            .size_full()
            .p(px(PAD_PX))
            .flex()
            .flex_col()
            .child(reveal(
                &p,
                REVEAL_HEADER,
                header("cpu", accent, t("System"), l.host.clone(), palette, Some(chip("timer", labels.uptime.clone(), palette.on_surface_variant, palette).into_any_element())),
            ))
            .child(div().h(px(GAP_PX)))
            .child(reveal(&p, REVEAL_TILES, tiles(l, labels, cpu, mem, palette, display.clone(), radius)));
        if !l.snapshot.gpus.is_empty() {
            col = col
                .child(div().h(px(GAP_PX)))
                .child(reveal(&p, REVEAL_GPUS, titled(t("GRAPHICS"), SECTION_GAP_PX, palette, None, gpus_section(l, &labels.gpus, palette, display, radius))));
        }
        let temps = &l.snapshot.temps;
        if !temps.is_empty() {
            let shown = temps_shown(temps.len(), l.temps_expanded, l.tuning.temps_shown);
            let toggle = (temps.len() > l.tuning.temps_shown).then(|| temps_toggle(&live, labels.temps_more.clone(), l.temps_expanded, palette));
            col = col
                .child(div().h(px(GAP_PX)))
                .child(reveal(&p, REVEAL_TEMPS, titled(t("TEMPERATURES"), SECTION_GAP_PX, palette, toggle, temperatures(temps, &labels.temps, shown, palette))));
        }
        if !labels.disks.is_empty() {
            col = col.child(div().h(px(GAP_PX))).child(reveal(&p, REVEAL_STORAGE, titled(t("STORAGE"), SECTION_GAP_PX, palette, None, storage(&labels.disks, l.tuning, palette))));
        }
        col.child(div().h(px(GAP_PX)))
            .child(reveal(
                &p,
                REVEAL_TOP,
                titled(t("TOP APPS"), TOP_SECTION_GAP_PX, palette, Some(open_hunter_link(palette)), processes(&l.snapshot, &labels.top, l.tuning.top_apps, palette)),
            ))
            .into_any_element()
    }
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Resources"), tk("CPU, memory and disk usage"), Some(Zone::Right)), |window, cx, panel_cx| {
        let live = Rc::new(RefCell::new(Live::new()));
        // Displayed values spring toward each smoothed sample.
        let shown = Rc::new(RefCell::new((
            Readout::new(CHIP_SPRING, CHIP_SNAP_BELOW, PERCENT_STEP),
            Readout::new(CHIP_SPRING, CHIP_SNAP_BELOW, PERCENT_STEP),
        )));
        let bar_live = live.clone();
        let bar = fn_view(
            None,
            move |palette, window, cx| {
                let (cpu, mem) = {
                    let s = crate::hub::resources_entity(cx).read(cx);
                    (s.cpu, s.mem)
                };
                // The bar renders on every hub sample, so history keeps
                // accumulating while the panel is closed.
                bar_live.borrow_mut().record(cpu, mem);
                let (cpu, mem) = {
                    let mut st = shown.borrow_mut();
                    (st.0.show(cpu, window), st.1.show(mem, window))
                };
                let track = palette.on_surface.with_alpha(CHIP_TRACK_ALPHA);
                let meter = |value: f32, color| {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(CHIP_RING_GAP_PX))
                        .child(ring(value, CHIP_RING_D_PX, CHIP_RING_STROKE_PX, color, track))
                        .child(div().w(px(CHIP_VALUE_W_PX)).text_color(palette.on_surface_variant.hsla()).child(percent_text(value)))
                };
                div()
                    .flex()
                    .items_center()
                    .gap(px(CHIP_GAP_PX))
                    .font_features(tabular_nums())
                    .child(meter(cpu, palette.primary))
                    .child(meter(mem, palette.tertiary))
                    .into_any_element()
            },
            window,
            cx,
        );
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W_PX), px(panel_height(0, 0.0, DEFAULT_TOP_APPS as usize)))));
        // Prime host, filesystems, temperatures and GPUs off the UI thread
        // so the panel opens at its final size with its header filled in.
        {
            let task = cx.background_executor().spawn(async { (host_line(), read_disks(), hardware(), uptime()) });
            let (live, size_cell) = (live.clone(), size_cell.clone());
            cx.spawn(async move |cx| {
                let (host, disks, (temps, gpus), up) = task.await;
                let views = {
                    let mut l = live.borrow_mut();
                    l.host = host.into();
                    if l.disks_at.is_none() {
                        l.disks = disks;
                        l.disks_at = Some(Instant::now());
                    }
                    if l.sampled_at.is_none() {
                        l.snapshot.uptime = up;
                        l.snapshot.temp_c = sensors::cpu_temperature(&temps).or(l.snapshot.temp_c);
                        l.snapshot.temps = temps;
                        l.snapshot.gpus = gpus;
                        l.record_gpus();
                    }
                    l.generation += 1;
                    size_cell.set(size(px(PANEL_W_PX), px(panel_height(l.disks.len(), l.extra_height(), l.tuning.top_apps))));
                    l.views.clone()
                };
                views.notify_async(cx);
            })
            .detach();
        }
        let panel = fn_view(None, panel_view(panel_cx.clone(), live.clone(), size_cell.clone()), window, cx);
        let side_live = live.clone();
        let sidebar = fn_view(
            None,
            move |palette, window, cx| {
                let (cpu, mem) = {
                    let s = crate::hub::resources_entity(cx).read(cx);
                    (s.cpu, s.mem)
                };
                let units = crate::state::config(cx).read(cx).get().units();
                let mut guard = side_live.borrow_mut();
                guard.tuning = Tuning::of(cx);
                guard.views.track(window);
                guard.record(cpu, mem);
                guard.labels(units);
                let l = &*guard;
                let [cpu_c, mem_c, _] = colors(palette);
                let mini = |icon_name: &'static str, label: &'static str, v: f32, h: &History, (a, b): (Color, Color)| {
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .flex()
                        .flex_col()
                        .gap(px(ROW_GAP_PX))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(ROW_GAP_PX))
                                .child(icon(icon_name, CARD_ICON_PX, a))
                                .child(div().flex_1().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(label))
                                .child(div().font_features(tabular_nums()).text_size(px(text::BODY_S)).font_weight(FontWeight::SEMIBOLD).child(percent_text(v))),
                        )
                        .child(sparkline(h, SIDEBAR_SPARK_H_PX, a.with_alpha(SPARK_FROM_ALPHA), b, palette))
                };
                let disks = l.labels.as_ref().map_or(&[][..], |labels| &labels.disks[..labels.disks.len().min(SIDEBAR_DISKS)]);
                card(palette)
                    .flex()
                    .flex_col()
                    .gap(px(tokens::MD))
                    .child(div().flex().gap(px(tokens::MD)).child(mini("cpu", t("CPU"), cpu, &l.cpu, cpu_c)).child(mini("memory", t("Memory"), mem, &l.mem, mem_c)))
                    .when(!disks.is_empty(), |el| el.child(storage(disks, l.tuning, palette)))
                    .into_any_element()
            },
            window,
            cx,
        );
        let samples = crate::hub::resources_entity(cx);
        for view in [&bar, &panel, &sidebar] {
            super::follow(view, &samples, cx);
        }
        PluginInstance {
            bar: Some(bar),
            panel: Some(PanelSpec { view: panel, size: size_cell }),
            sidebar: Some(sidebar),
            on_click: None,
            on_secondary_click: Some(Rc::new(|_, cx| {
                // Studio → Applications' terminal, else $TERMINAL, else ghostty.
                let term = crate::state::config(cx).read(cx).get().apps.terminal_command().or_else(|| std::env::var("TERMINAL").ok()).unwrap_or_else(|| "ghostty".into());
                // A multi-word command (`kitty --single-instance`) and each
                // terminal's own "run this" flag (`wezterm start --`).
                let argv = mandara_services::updates::terminal_argv(&term, RESOURCE_MONITOR);
                use std::os::unix::process::CommandExt as _;
                let _ = mandara_services::reaper::spawn(std::process::Command::new(&argv[0]).args(&argv[1..]).process_group(0));
            })),
        }
    });
    r.command(ID, CommandSpec::new("system resources", tk("CPU, memory, GPUs, temperatures, disks and uptime"), |_, cx| {
        let units = crate::state::config(cx).read(cx).get().units();
        let mut s = SystemSampler::default();
        s.sample();
        std::thread::sleep(COMMAND_SAMPLE_GAP);
        s.sample();
        let disks: Vec<_> = read_disks()
            .iter()
            .map(|d| json!({ "mount": d.mount, "fstype": d.fstype, "used_bytes": d.used, "total_bytes": d.total }))
            .collect();
        let (mem_used, mem_total, swap_used, swap_total) = meminfo();
        let (temps, gpus) = hardware();
        let round1 = |c: f32| (f64::from(c) * ROUND_TENTHS).round() / ROUND_TENTHS;
        let temperatures: Vec<_> = temps
            .iter()
            .map(|c| {
                json!({
                    "component": c.kind.key(),
                    "name": c.name.english(),
                    "device": c.device,
                    "chip": c.chip,
                    "celsius": round1(c.celsius),
                    "warm_celsius": c.warm,
                    "hot_celsius": c.hot,
                    "readings": c.readings.iter().map(|r| json!({ "label": r.label.english(), "celsius": round1(r.celsius) })).collect::<Vec<_>>(),
                })
            })
            .collect();
        let gpus: Vec<_> = gpus
            .iter()
            .map(|g| {
                json!({
                    "name": g.name,
                    "vendor": g.vendor.key(),
                    "pci_slot": g.slot,
                    "integrated": g.integrated,
                    "load": g.load,
                    "vram_used_bytes": g.vram_used,
                    "vram_total_bytes": g.vram_total,
                    "temp_c": g.temp_c.map(round1),
                    "freq_mhz": g.freq_mhz.map(f32::round),
                })
            })
            .collect();
        Ok(json!({
            "cpu": s.cpu,
            "memory": s.mem,
            "memory_used_bytes": mem_used,
            "memory_total_bytes": mem_total,
            "swap_used_bytes": swap_used,
            "swap_total_bytes": swap_total,
            "load": loadavg(),
            "cpu_ghz": cpu_freq_ghz(),
            "cpu_temp_c": sensors::cpu_temperature(&temps).or_else(cpu_temp_c).map(round1),
            // Temperatures below are Celsius; `units` is the display preference.
            "units": units,
            "temperatures": temperatures,
            "gpus": gpus,
            "disks": disks,
            "uptime": uptime(),
        }))
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mounts_dedupe_devices_and_skip_virtual() {
        let text = "proc /proc proc rw 0 0\n\
                    /dev/nvme0n1p2 / btrfs rw,subvol=/@ 0 0\n\
                    /dev/nvme0n1p2 /home btrfs rw,subvol=/@home 0 0\n\
                    /dev/nvme0n1p1 /boot vfat rw 0 0\n\
                    /dev/sda1 /mnt/My\\040Data ext4 rw 0 0\n\
                    tmpfs /tmp tmpfs rw 0 0\n\
                    /dev/loop0 /snap/core squashfs ro 0 0\n";
        let m = parse_mounts(text);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0], ("/dev/nvme0n1p2".into(), "/".into(), "btrfs".into()));
        assert_eq!(m[1].1, "/mnt/My Data");
        assert_eq!(disk_label("/mnt/My Data"), "My Data");
        assert_eq!(disk_label("/"), "System");
    }

    #[test]
    fn unescape_handles_trailing_and_partial_escapes() {
        assert_eq!(unescape("a\\040b"), "a b");
        assert_eq!(unescape("end\\04"), "end\\04");
        assert_eq!(unescape("x\\"), "x\\");
    }

    #[test]
    fn panel_height_tracks_disk_count() {
        assert!(panel_height(2, 0.0, 5) > panel_height(1, 0.0, 5));
        assert_eq!(panel_height(2, 0.0, 5) - panel_height(1, 0.0, 5), DISK_H_PX);
        assert_eq!(panel_height(1, 50.0, 5) - panel_height(1, 0.0, 5), 50.0);
        assert!(!read_disks().is_empty());
        assert!(loadavg()[0] >= 0.0);
    }

    fn gpu(slot: &str, vram: bool) -> Gpu {
        Gpu {
            slot: slot.into(),
            vendor: sensors::GpuVendor::Amd,
            name: None,
            integrated: !vram,
            load: Some(0.2),
            vram_used: vram.then_some(1),
            vram_total: vram.then_some(2),
            temp_c: Some(50.0),
            freq_mhz: None,
        }
    }

    #[test]
    fn hardware_sections_size_to_content() {
        assert_eq!(temps_height(0), 0.0);
        assert_eq!(temps_height(2), GAP_PX + SECTION_H_PX + SECTION_GAP_PX + TEMP_CELL_H_PX);
        assert_eq!(temps_height(3) - temps_height(2), TEMP_CELL_H_PX + TEMP_ROW_GAP_PX);
        assert_eq!(temps_shown(12, false, 8), 8);
        assert_eq!(temps_shown(12, true, 8), 12);
        assert_eq!(gpus_height(&[]), 0.0);
        // Two GPUs share a row at the taller card's height.
        let pair = [gpu("a", true), gpu("b", false)];
        assert_eq!(gpu_rows(&pair), vec![GPU_BASE_H_PX + GPU_VRAM_H_PX]);
        let three = [gpu("a", true), gpu("b", false), gpu("c", false)];
        assert_eq!(gpus_height(&three), GAP_PX + SECTION_H_PX + SECTION_GAP_PX + GPU_BASE_H_PX + GPU_VRAM_H_PX + GPU_BASE_H_PX + GPU_GAP_PX);
    }

    #[test]
    fn heat_gradient_shifts_with_thresholds() {
        let p = mandara_theme::engine::default_palette(true);
        assert_eq!(heat_gradient(30.0, 70.0, 85.0, &p).0, p.success_emerald);
        assert_eq!(heat_gradient(90.0, 70.0, 85.0, &p).1, p.urgent_coral);
    }

    #[test]
    fn labels_are_rebuilt_only_when_the_sample_changes() {
        let mut live = Live::new();
        live.snapshot.top = vec![TopApp { name: "zed".into(), count: 2, cpu: 3.25, rss: 1, icon: None }];
        live.snapshot.primed = true;
        let first = live.labels(Units::Metric).top[0].cpu.clone();
        assert_eq!(first.as_ref(), "3.2%");
        live.snapshot.top[0].cpu = 9.0;
        assert_eq!(live.labels(Units::Metric).top[0].cpu, first, "same generation reuses the labels");
        live.generation += 1;
        assert_eq!(live.labels(Units::Metric).top[0].cpu.as_ref(), "9.0%");
        assert_eq!(percent_number(0.424).as_ref(), "42");
    }

    #[test]
    fn gpu_histories_forget_removed_gpus() {
        let mut live = Live::new();
        live.snapshot.gpus = vec![gpu("a", true), gpu("b", false)];
        live.record_gpus();
        live.snapshot.gpus.truncate(1);
        live.record_gpus();
        assert_eq!(live.gpu_load.len(), 1);
    }
}
