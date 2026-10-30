//! Process hunter: search running processes by name, command line or PID
//! and end them (SIGTERM, with a force-kill follow-up when a process
//! ignores it). Same-name processes can be grouped (`firefox ×12`, click to
//! expand; "End all" signals every member). A live CPU / memory strip with
//! sparklines heads the panel; sampling runs in the background every 1.5 s
//! only while the panel is open. Opens from the launcher ("Processes") even
//! when it isn't placed in the bar; commands `processes list|kill`.
//!
//! The filtered / grouped / sorted list is derived once per sample (or
//! when the query, sort or grouping changes) and cached, so frames of the
//! open animation and hover redraws never re-scan or re-sort the table.
//! Everything panel-only (process table, groups, sparklines, the sampler's
//! tick baseline) is freed once the panel has closed; the last list height
//! is kept so a reopen shows a same-size placeholder instead of reflowing.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::hash::{Hash as _, Hasher as _};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext as _, ElementId, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px, size,
};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk, tr};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry};
use mandara_services::processes::{self, Process, Sampler, SortBy};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::fn_view;
use super::panel_fx::{
    ANGLE_DOWN_DEG, ANGLE_RIGHT_DEG, EXPIRY_MARGIN, HEADER_H, History, PILL_H_PX, PanelShape, SMALL_ICON_PX, ViewSet, alpha, empty_state, glass_card,
    glow, header, meter, partner, percent_text, readable, redraw_in, sparkline,
};
use super::resources::demo;
use crate::fonts::tabular_nums;
use crate::icons::icon;
use crate::kit::{icon_button, reveal, text, tokens};
use crate::text_field::TextField;
use crate::widgets::system::format_bytes;

pub const ID: &str = "processes";

const PANEL_W: f32 = 560.0;
const PAD: f32 = 14.0;
const GAP: f32 = tokens::MD;
const TILE_H: f32 = 78.0;
const SEARCH_H: f32 = 36.0;
const COLS_H: f32 = 18.0;
const ROW_H: f32 = 52.0;
const CHILD_H: f32 = 44.0;
const ROW_GAP: f32 = 2.0;
/// Rows visible before the list scrolls (a cut row hints at more).
const LIST_ROWS_VISIBLE: f32 = 7.5;
/// Visible list height before it scrolls.
const LIST_MAX_H: f32 = LIST_ROWS_VISIBLE * (ROW_H + ROW_GAP);
const EMPTY_H: f32 = 176.0;
const FOOTER_H: f32 = 26.0;
/// Initial panel height before the first sample sizes it.
const INITIAL_H: f32 = 560.0;
/// Rows (processes or groups) rendered at most.
const ROWS: usize = 40;
/// Sparkline columns (≈ 27 s at the hub's 1.5 s cadence).
const HISTORY: usize = 18;
/// Process-table sample period in ms (`refresh_ms`).
pub const DEFAULT_REFRESH_MS: i64 = 1500;
pub const REFRESH_MS_RANGE: (i64, i64) = (500, 10_000);
/// A sample this close to due counts as due (render timers jitter).
const SAMPLE_SLACK: Duration = Duration::from_millis(100);
/// Gap before a second table when the first has no CPU baseline.
const BASELINE_GAP: Duration = Duration::from_millis(250);
/// Sparkline samples are at least this far apart.
const HISTORY_GAP: Duration = Duration::from_millis(1200);
/// A "End" press arms the button; a second press within this sends SIGTERM.
const ARM: Duration = Duration::from_secs(3);
/// After SIGTERM, offer force kill if the process is still alive by then.
const GRACE: Duration = Duration::from_millis(1500);
/// Redraw cadence while an End button is armed or a SIGTERM is pending.
const TRANSIENT_TICK: Duration = Duration::from_millis(500);
/// App names whose icons were pre-resolved off-thread (bounded).
const WARMED_CAP: usize = 1024;
/// Processes returned by `processes list`.
const CLI_LIST_MAX: usize = 50;
/// A process using at least this much CPU (percent of one core) reads hot
/// (`hot_cpu_percent`).
pub const DEFAULT_HOT_CPU_PERCENT: i64 = 50;
pub const HOT_CPU_RANGE: (i64, i64) = (5, 400);

/// CPU meter scale of one render: the busiest row and the hot threshold.
#[derive(Copy, Clone, Debug)]
struct CpuScale {
    max: f32,
    hot: f32,
}

/// Sample period (`refresh_ms`).
fn sample_every(cx: &App) -> Duration {
    let ms: i64 = crate::state::config(cx).read(cx).snapshot().plugins.setting(ID, "refresh_ms", DEFAULT_REFRESH_MS);
    Duration::from_millis(ms.clamp(REFRESH_MS_RANGE.0, REFRESH_MS_RANGE.1) as u64)
}

/// Hot threshold in percent of one core (`hot_cpu_percent`).
fn hot_cpu(cx: &App) -> f32 {
    let pct: i64 = crate::state::config(cx).read(cx).snapshot().plugins.setting(ID, "hot_cpu_percent", DEFAULT_HOT_CPU_PERCENT);
    pct.clamp(HOT_CPU_RANGE.0, HOT_CPU_RANGE.1) as f32
}
/// CPU meters are scaled to at least one full core.
const CPU_SCALE_FLOOR: f32 = 100.0;
/// PIDs previewed in a group's detail line.
const PID_PREVIEW: usize = 4;
/// How often a closed-panel watch polls the panel's open progress.
const CLOSE_POLL: Duration = Duration::from_secs(1);

/// Calls `on_close` once the panel behind `p` has fully closed (its open
/// progress is back at zero). Arm it from the panel's render; one watch runs
/// per `armed` flag at a time and ends after firing, so a closed panel costs
/// nothing.
pub(super) fn on_panel_close(p: &PanelCx, armed: &Rc<Cell<bool>>, cx: &mut App, on_close: impl FnOnce(&mut App) + 'static) {
    if armed.replace(true) {
        return;
    }
    let (p, armed) = (p.clone(), armed.clone());
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor().timer(CLOSE_POLL).await;
            if p.progress() <= 0.0 {
                break;
            }
        }
        armed.set(false);
        cx.update(on_close);
    })
    .detach();
}

/// `ink_on`: backgrounds lighter than this get dark ink.
const INK_SWITCH_L: f32 = 0.64;
/// Dark ink `(lightness, chroma)` and light ink lightness (OKLCH).
const DARK_INK: (f32, f32) = (0.2, 0.02);
const LIGHT_INK_L: f32 = 0.99;

// Avatars.
const AVATAR_PX: f32 = 34.0;
const AVATAR_RADIUS_FRAC: f32 = 0.32;
const AVATAR_GLYPH_FRAC: f32 = 0.64;
const AVATAR_FILL: (f32, f32) = (0.06, 0.045);
const STATE_DOT_PX: f32 = 10.0;
/// The run-state dot hangs this far past the avatar's corner.
const STATE_DOT_OFFSET_PX: f32 = -2.0;
const CHILD_DOT_PX: f32 = 6.0;
/// Guide line of an expanded group's children.
const GUIDE_W_PX: f32 = 2.0;
const GUIDE_ALPHA: f32 = 0.09;

// Badges.
const BADGE_H_PX: f32 = 16.0;
const BADGE_PAD_X_PX: f32 = 6.0;
const BADGE_TEXT_PX: f32 = 10.0;
const BADGE_FILL: (f32, f32) = (0.14, 0.1);

// Columns.
const CPU_COL_W_PX: f32 = 68.0;
const CPU_METER_W_PX: f32 = 52.0;
const CPU_METER_H_PX: f32 = 4.0;
const CPU_CELL_GAP_PX: f32 = 4.0;
const CPU_FROM_ALPHA: f32 = 0.75;
const MEM_COL_W_PX: f32 = 66.0;
const ACTION_COL_W_PX: f32 = 96.0;
const COLUMN_LABEL_ALPHA: f32 = 0.85;

// Action pill.
const ACTION_PAD_X_PX: f32 = 11.0;
const ACTION_GAP_PX: f32 = 5.0;
/// Action label: half a step above the label size.
const ACTION_TEXT_PX: f32 = text::LABEL + 0.5;
const PENDING_FILL_ALPHA: f32 = 0.06;
const ACTION_SOFT: (f32, f32) = (0.16, 0.11);
const ACTION_HOVER: (f32, f32) = (0.26, 0.18);
/// Force-kill fill: darken coral by this much, never below the floor, and
/// rotate the gradient end by this hue.
const FORCE_DARKEN_L: f32 = 0.08;
const FORCE_MIN_L: f32 = 0.3;
const FORCE_HUE_DEG: f32 = -12.0;
/// Glows `(alpha, blur, dy)` under force-kill and armed buttons.
const FORCE_GLOW: (f32, f32, f32) = (0.35, 8.0, 1.0);
const ARMED_GLOW: (f32, f32, f32) = (0.3, 8.0, 1.0);

// Rows.
const ROW_PAD_X_PX: f32 = 8.0;
const ROW_INNER_GAP_PX: f32 = 10.0;
const ROW_HOVER: (f32, f32) = (0.05, 0.04);
const ROW_SELECTED: (f32, f32) = (0.1, 0.08);
const ROW_ARMED: (f32, f32) = (0.1, 0.07);
/// Row radius: concentric inside the card, never below the minimum.
const ROW_RADIUS_INSET_PX: f32 = 2.0;
const ROW_MIN_RADIUS_PX: f32 = 10.0;
const TITLE_GAP_PX: f32 = 6.0;
const SUB_LINE_ALPHA: f32 = 0.82;
const CHEVRON_BOX_PX: f32 = 16.0;
const CHEVRON_GAP_PX: f32 = 4.0;

// Tiles.
const TILE_PAD_X_PX: f32 = 12.0;
const TILE_PAD_Y_PX: f32 = 10.0;
const TILE_GAP_PX: f32 = 10.0;
const TILE_LABEL_GAP_PX: f32 = 6.0;
const TILE_ICON_PX: f32 = 13.0;
const TILE_VALUE_PX: f32 = 20.0;
const TILE_VALUE_LINE_PX: f32 = 24.0;
const TILE_DETAIL_PX: f32 = 10.5;
const TILE_SPARK_W_PX: f32 = 100.0;
const TILE_SPARK_H_PX: f32 = 42.0;
const TILE_SPARK_FROM_ALPHA: f32 = 0.75;

// Search / sort controls.
const SEG_INSET_PX: f32 = 3.0;
const SEG_PAD_X_PX: f32 = 10.0;
const SEG_RADIUS_PX: f32 = 9.0;
const SEG_GAP_PX: f32 = 5.0;
const SEG_TRACK_GAP_PX: f32 = 2.0;
const SEG_HOVER_ALPHA: f32 = 0.05;
const SEG_ACTIVE: (f32, f32) = (0.18, 0.13);
const SEG_TRACK: (f32, f32) = (0.05, 0.045);
const CONTROL_RADIUS_PX: f32 = 12.0;
const SEARCH_PAD_L_PX: f32 = 12.0;
const SEARCH_PAD_R_PX: f32 = 2.0;
const SEARCH_ICON_PX: f32 = 14.0;
const CLEAR_BUTTON_PX: f32 = 26.0;
/// Gap between the controls row and the column labels.
const CONTROLS_GAP_PX: f32 = 10.0;
const COLUMNS_GAP_PX: f32 = 4.0;
const GROUP_TOGGLE_HOVER_ALPHA: f32 = 0.1;
const GROUP_TOGGLE_OFF_ALPHA: f32 = 0.06;

// List chrome.
const LIST_FADE_PX: f32 = 22.0;
/// The list counts as clipped within this of its maximum height.
const CLIP_EPSILON_PX: f32 = 0.5;
/// Size changes smaller than this don't resize the panel.
const RESIZE_EPSILON_PX: f32 = 0.5;
/// The footer line is this much shorter than its slot.
const FOOTER_INSET_PX: f32 = 4.0;
const FOOTER_GAP_PX: f32 = 6.0;
const FOOTER_ICON_PX: f32 = 11.0;
const BAR_ICON_PX: f32 = 15.0;

// Toast.
const TOAST_H: f32 = 36.0;
const TOAST_PAD_L_PX: f32 = 6.0;
const TOAST_PAD_R_PX: f32 = 14.0;
/// Toast drop shadow: alpha `(dark, light)`, blur and dy.
const TOAST_SHADOW: (f32, f32) = (0.45, 0.14);
const TOAST_SHADOW_BLUR_PX: f32 = 14.0;
const TOAST_SHADOW_DY_PX: f32 = 4.0;
const TOAST_BADGE_PX: f32 = 24.0;
const TOAST_BADGE_ALPHA: f32 = 0.16;
/// The toast keeps this much panel padding clear on each side.
const TOAST_SIDE_PADS: f32 = 4.0;

/// What an action button / selection refers to.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key {
    Pid(u32),
    Group(String),
}

/// Same-name processes, summed. `members` index the cached results.
#[derive(Clone, Debug)]
struct Group {
    name: String,
    members: Vec<usize>,
    cpu: f32,
    rss: u64,
}

impl Group {
    fn pids(&self, results: &[Process]) -> Vec<u32> {
        self.members.iter().map(|&i| results[i].pid).collect()
    }
}

/// Groups `results` by name. With a query the relevance order (first
/// appearance) is kept; otherwise groups are ordered by their totals.
fn group(results: &[Process], sort: SortBy, ranked: bool) -> Vec<Group> {
    let mut index: HashMap<&str, usize> = HashMap::new();
    let mut out: Vec<Group> = Vec::new();
    for (i, p) in results.iter().enumerate() {
        match index.get(p.name.as_str()) {
            Some(&g) => {
                let g = &mut out[g];
                g.cpu += p.cpu;
                g.rss += p.rss;
                g.members.push(i);
            }
            None => {
                index.insert(&p.name, out.len());
                out.push(Group { name: p.name.clone(), members: vec![i], cpu: p.cpu, rss: p.rss });
            }
        }
    }
    if !ranked {
        match sort {
            SortBy::Cpu => out.sort_by(|a, b| b.cpu.total_cmp(&a.cpu).then(b.rss.cmp(&a.rss))),
            SortBy::Memory => out.sort_by(|a, b| b.rss.cmp(&a.rss).then(b.cpu.total_cmp(&a.cpu))),
            SortBy::Name => out.sort_by_cached_key(|g| g.name.to_lowercase()),
        }
    }
    out
}

#[derive(Clone, Copy, Debug, Default)]
struct Totals {
    mem_used: u64,
    mem_total: u64,
    cores: usize,
    running: usize,
}

/// Kibibytes → bytes (`/proc/meminfo` reports kB).
const BYTES_PER_KIB: u64 = 1024;

fn meminfo() -> (u64, u64) {
    if demo::active() {
        let (used, total, _, _) = demo::meminfo();
        return (used, total);
    }
    let text = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let field = |name: &str| -> u64 { text.lines().find(|l| l.starts_with(name)).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()).unwrap_or(0) * BYTES_PER_KIB };
    let total = field("MemTotal:");
    (total.saturating_sub(field("MemAvailable:")), total)
}

/// Logical CPUs (read once: `available_parallelism` parses cgroup files).
fn cores() -> usize {
    if demo::active() {
        return demo::threads();
    }
    static CORES: OnceLock<usize> = OnceLock::new();
    *CORES.get_or_init(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1))
}

/// This user's uid (asked once, not per row per frame).
fn my_uid() -> u32 {
    static UID: OnceLock<u32> = OnceLock::new();
    *UID.get_or_init(|| rustix::process::getuid().as_raw())
}

/// A stable element id for a name (no per-frame `format!`).
fn name_id(prefix: &'static str, name: &str) -> ElementId {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    name.hash(&mut h);
    ElementId::NamedInteger(SharedString::new_static(prefix), h.finish())
}

/// What the cached list was derived from.
#[derive(Clone, Debug, PartialEq)]
struct ListKey {
    generation: u64,
    query: String,
    sort: SortBy,
    grouping: bool,
}

/// The filtered (and optionally grouped) list for one [`ListKey`].
#[derive(Default)]
struct ListCache {
    key: Option<ListKey>,
    results: Vec<Process>,
    groups: Vec<Group>,
    max_cpu: f32,
}

struct Hunter {
    sampler: Arc<Mutex<Sampler>>,
    procs: Vec<Process>,
    /// Bumped per sample; keys the list cache.
    generation: u64,
    /// Bumped when panel state is freed; samples from an older epoch are
    /// dropped on arrival.
    epoch: u64,
    /// Height of the list block (rows + footer) at the last sample, kept
    /// across a close so the reopen placeholder has the same size.
    placeholder_h: f32,
    /// A close watch is running.
    close_watch: Rc<Cell<bool>>,
    list: ListCache,
    sampled: Option<Instant>,
    in_flight: bool,
    sort: SortBy,
    grouped: bool,
    expanded: HashSet<String>,
    selected: Option<Key>,
    /// Action → when its End button was armed.
    armed: HashMap<Key, Instant>,
    /// PID → when SIGTERM was sent.
    terminated: HashMap<u32, Instant>,
    /// (ok, message, shown at).
    status: Option<(bool, SharedString, Instant)>,
    totals: Totals,
    cpu: History,
    mem: History,
    /// Names whose icons were already resolved off-thread.
    warmed: HashSet<String>,
    /// The panel view(s); async samples and the search field notify these.
    views: ViewSet,
}

impl Default for Hunter {
    fn default() -> Self {
        Self {
            sampler: Arc::new(Mutex::new(Sampler::new())),
            procs: Vec::new(),
            generation: 0,
            epoch: 0,
            placeholder_h: EMPTY_H,
            close_watch: Rc::default(),
            list: ListCache::default(),
            sampled: None,
            in_flight: false,
            sort: SortBy::Cpu,
            grouped: true,
            expanded: HashSet::new(),
            selected: None,
            armed: HashMap::new(),
            terminated: HashMap::new(),
            status: None,
            totals: Totals::default(),
            cpu: History::new(HISTORY),
            mem: History::new(HISTORY),
            warmed: HashSet::new(),
            views: ViewSet::default(),
        }
    }
}

impl Hunter {
    fn absorb(&mut self, procs: Vec<Process>, (mem_used, mem_total): (u64, u64)) {
        let running = procs.iter().filter(|p| p.state == 'R').count();
        self.totals = Totals { mem_used, mem_total, cores: cores(), running };
        // A terminated process is alive while it is still in the table.
        self.terminated.retain(|pid, _| procs.iter().any(|p| p.pid == *pid));
        self.procs = procs;
        self.generation = self.generation.wrapping_add(1);
        self.sampled = Some(Instant::now());
        self.in_flight = false;
        self.armed.retain(|_, t| t.elapsed() < ARM);
        if let Some(Key::Pid(pid)) = self.selected
            && !self.procs.iter().any(|p| p.pid == pid)
        {
            self.selected = None;
        }
    }

    /// Frees everything only the open panel needs (the panel closed).
    /// Sort, grouping and the placeholder height survive.
    fn free(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.generation = self.generation.wrapping_add(1);
        self.procs = Vec::new();
        self.list = ListCache::default();
        // A fresh sampler drops the previous per-PID tick table.
        self.sampler = Arc::new(Mutex::new(Sampler::new()));
        self.sampled = None;
        self.in_flight = false;
        self.expanded = HashSet::new();
        self.selected = None;
        self.armed = HashMap::new();
        self.terminated = HashMap::new();
        self.status = None;
        self.totals = Totals::default();
        self.cpu = History::new(HISTORY);
        self.mem = History::new(HISTORY);
        self.warmed = HashSet::new();
    }

    /// Re-derives the list only when the sample, query, sort or grouping
    /// changed (never per frame).
    fn ensure_list(&mut self, query: &str, grouping: bool) {
        let key = ListKey { generation: self.generation, query: query.to_string(), sort: self.sort, grouping };
        if self.list.key.as_ref() == Some(&key) {
            return;
        }
        let results = processes::search(&self.procs, query, self.sort);
        let groups = if grouping { group(&results, self.sort, !query.is_empty()) } else { Vec::new() };
        let max_cpu = if grouping { groups.iter().map(|g| g.cpu).fold(CPU_SCALE_FLOOR, f32::max) } else { results.iter().map(|p| p.cpu).fold(CPU_SCALE_FLOOR, f32::max) };
        self.list = ListCache { key: Some(key), results, groups, max_cpu };
    }

    /// Names not yet warmed (marks them warmed).
    fn names_to_warm(&mut self) -> Vec<String> {
        if self.warmed.len() > WARMED_CAP {
            self.warmed.clear();
        }
        let mut out = Vec::new();
        for p in &self.procs {
            if !self.warmed.contains(&p.name) {
                self.warmed.insert(p.name.clone());
                out.push(p.name.clone());
            }
        }
        out
    }

    fn armed(&self, key: &Key) -> bool {
        self.armed.get(key).is_some_and(|t| t.elapsed() < ARM)
    }

    /// Signals `targets` (`(pid, name)`), recording the outcome as a toast.
    fn end(&mut self, key: &Key, targets: &[(u32, String)], force: bool) {
        let mut ended = 0usize;
        let mut error: Option<String> = None;
        for (pid, _) in targets {
            match processes::kill(*pid, force) {
                Ok(()) => {
                    ended += 1;
                    if !force {
                        self.terminated.insert(*pid, Instant::now());
                    }
                }
                Err(e) => error = Some(tr(&e)),
            }
        }
        self.armed.remove(key);
        let msg = match (targets, ended, &error) {
            ([(pid, name)], 1, _) => {
                let args: [(&str, &dyn std::fmt::Display); 2] = [("name", name), ("pid", pid)];
                if force { tf("Killed {name} ({pid})", &args) } else { tf("Asked to quit: {name} ({pid})", &args) }
            }
            (_, 0, Some(e)) => e.clone(),
            (_, n, None) => {
                let name = targets.first().map(|(_, n)| n.as_str()).unwrap_or_default();
                let args: [(&str, &dyn std::fmt::Display); 2] = [("count", &n), ("name", &name)];
                if force { tf("Force-killed {count} × {name}", &args) } else { tf("Asked {count} × {name} to quit", &args) }
            }
            (_, n, Some(e)) => tf("{failed} of {count} could not be ended: {error}", &[("failed", &(targets.len() - n)), ("count", &targets.len()), ("error", e)]),
        };
        self.status = Some((error.is_none(), msg.into(), Instant::now()));
    }
}

type Shared = Rc<RefCell<Hunter>>;

/// One process-table sample (blocking; runs on the background executor).
fn sample(sampler: &Mutex<Sampler>) -> Vec<Process> {
    sampler.lock().map(|mut s| s.sample()).unwrap_or_default()
}

/// Starts a background sample when one is due (or `force`d); the panel
/// re-renders when it lands. A first table without a CPU baseline is
/// re-sampled after an executor timer (no thread sleeps).
fn refresh(h: &Shared, force: bool, cx: &mut App) {
    let every = sample_every(cx);
    let (sampler, epoch) = {
        let mut hb = h.borrow_mut();
        let due = force || hb.sampled.is_none_or(|t| t.elapsed() >= every.saturating_sub(SAMPLE_SLACK));
        if hb.in_flight || !due {
            return;
        }
        hb.in_flight = true;
        (hb.sampler.clone(), hb.epoch)
    };
    let executor = cx.background_executor().clone();
    let h = h.clone();
    cx.spawn(async move |cx| {
        let first = {
            let sampler = sampler.clone();
            executor.spawn(async move { sample(&sampler) }).await
        };
        let procs = if !first.is_empty() && first.iter().all(|p| p.cpu == 0.0) {
            // No CPU baseline yet: take a second table shortly.
            executor.timer(BASELINE_GAP).await;
            executor.spawn(async move { sample(&sampler) }).await
        } else {
            first
        };
        let mem = executor.spawn(async { meminfo() }).await;
        let (warm, views) = {
            let mut hb = h.borrow_mut();
            if hb.epoch != epoch {
                // The panel closed (state freed) while this sample ran.
                return;
            }
            hb.absorb(procs, mem);
            (hb.names_to_warm(), hb.views.clone())
        };
        // Resolve new app icons off the UI thread so rows never block on
        // an icon-theme search (the resolver caches globally).
        if !warm.is_empty() {
            executor
                .spawn(async move {
                    for name in warm {
                        let _ = crate::apps::icon_for_app_id(&name, false);
                    }
                })
                .detach();
        }
        views.notify_async(cx);
    })
    .detach();
}

// ── Pieces ───────────────────────────────────────────────────────────────

/// Legible foreground on a solid `bg` (dark ink on light fills).
fn ink_on(bg: Color) -> Color {
    if bg.lightness() > INK_SWITCH_L { Color::oklch(DARK_INK.0, DARK_INK.1, bg.hue()) } else { Color::oklch(LIGHT_INK_L, 0.0, 0.0) }
}

fn state_dot(state: char, palette: &MandaraPalette) -> Option<Color> {
    match state {
        'R' => Some(palette.success_emerald),
        'D' => Some(palette.warning_amber),
        'Z' | 'X' => Some(palette.urgent_coral),
        'T' | 't' => Some(palette.on_surface_variant),
        _ => None,
    }
}

/// App icon on a soft tile with an optional run-state dot.
fn avatar(name: &str, side: f32, dot: Option<Color>, palette: &MandaraPalette) -> AnyElement {
    let ring = crate::shell::frame_color(palette);
    div()
        .relative()
        .size(px(side))
        .flex_none()
        .rounded(px(crate::style::radius(side * AVATAR_RADIUS_FRAC)))
        .bg(palette.on_surface.with_alpha(alpha(AVATAR_FILL, palette)).hsla())
        .flex()
        .items_center()
        .justify_center()
        .child(super::workspaces::app_icon(name, side * AVATAR_GLYPH_FRAC, palette.on_surface_variant, false, palette))
        .when_some(dot, |el, c| {
            el.child(
                div()
                    .absolute()
                    .right(px(STATE_DOT_OFFSET_PX))
                    .bottom(px(STATE_DOT_OFFSET_PX))
                    .size(px(STATE_DOT_PX))
                    .rounded_full()
                    .border_2()
                    .border_color(ring.hsla())
                    .bg(c.hsla()),
            )
        })
        .into_any_element()
}

fn badge(label: impl Into<SharedString>, tint: Color, palette: &MandaraPalette) -> AnyElement {
    div()
        .flex_none()
        .h(px(BADGE_H_PX))
        .px(px(BADGE_PAD_X_PX))
        .rounded_full()
        .flex()
        .items_center()
        .bg(tint.with_alpha(alpha(BADGE_FILL, palette)).hsla())
        .font_features(tabular_nums())
        .text_size(px(BADGE_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tint.hsla())
        .child(label.into())
        .into_any_element()
}

/// CPU column: tabular percent over a slim gradient meter.
fn cpu_cell(cpu: f32, scale: CpuScale, primed: bool, palette: &MandaraPalette) -> AnyElement {
    let (max_cpu, hot) = (scale.max, cpu >= scale.hot);
    let (from, to) = if hot {
        (palette.warning_amber, palette.urgent_coral)
    } else {
        let a = readable(palette.primary, palette);
        (a.with_alpha(CPU_FROM_ALPHA), readable(partner(palette.primary), palette))
    };
    let frac = if max_cpu > 0.0 { (cpu / max_cpu).clamp(0.0, 1.0) } else { 0.0 };
    div()
        .w(px(CPU_COL_W_PX))
        .flex_none()
        .flex()
        .flex_col()
        .items_end()
        .gap(px(CPU_CELL_GAP_PX))
        .child(
            div()
                .font_features(tabular_nums())
                .text_size(px(text::BODY_S))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if hot { readable(palette.urgent_coral, palette).hsla() } else { palette.on_surface.hsla() })
                .child(if primed { SharedString::from(format!("{cpu:.1}%")) } else { SharedString::new_static("–") }),
        )
        .child(div().w(px(CPU_METER_W_PX)).child(meter(frac, CPU_METER_H_PX, from, to, palette)))
        .into_any_element()
}

fn mem_cell(rss: u64, palette: &MandaraPalette) -> AnyElement {
    div()
        .w(px(MEM_COL_W_PX))
        .flex_none()
        .flex()
        .justify_end()
        .font_features(tabular_nums())
        .text_size(px(text::BODY_S))
        .text_color(palette.on_surface_variant.hsla())
        .child(format_bytes(rss))
        .into_any_element()
}

/// The End → Confirm → (Ending…) → Force kill button for one process or a
/// whole group (`pids`, all named `name`). `pinned` keeps it visible
/// without hover (selected / armed).
fn action(key: Key, name: &str, pids: Vec<u32>, h: &Shared, palette: &MandaraPalette, pinned: bool) -> AnyElement {
    let n = pids.len();
    let (armed, pending, sent) = {
        let hb = h.borrow();
        let mut sent = 0usize;
        let mut pending = false;
        for pid in &pids {
            if let Some(at) = hb.terminated.get(pid) {
                sent += 1;
                pending |= at.elapsed() < GRACE;
            }
        }
        (hb.armed(&key), pending, sent)
    };
    let offer_force = sent > 0 && !pending;
    let coral = palette.urgent_coral;
    let id = match &key {
        Key::Pid(pid) => ElementId::from(("proc-act", *pid as usize)),
        Key::Group(name) => name_id("grp-act", name),
    };
    let base = div()
        .id(id)
        .h(px(PILL_H_PX))
        .px(px(ACTION_PAD_X_PX))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(ACTION_GAP_PX))
        .text_size(px(ACTION_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD);
    if pending {
        return base
            .bg(palette.on_surface.with_alpha(PENDING_FILL_ALPHA).hsla())
            .text_color(palette.on_surface_variant.hsla())
            .child(icon("timer", SMALL_ICON_PX, palette.on_surface_variant))
            .child(t("Ending…"))
            .into_any_element();
    }
    let h = h.clone();
    let name = name.to_string();
    let targets = move |alive_only: bool| -> Vec<(u32, String)> {
        pids.iter().filter(|pid| !alive_only || processes::is_alive(**pid)).map(|pid| (*pid, name.clone())).collect()
    };
    if offer_force {
        let deep = coral.with_lightness((coral.lightness() - FORCE_DARKEN_L).max(FORCE_MIN_L));
        let ink = ink_on(deep);
        let label: SharedString = if n > 1 { tf("Force kill ×{count}", &[("count", &sent)]).into() } else { t("Force kill").into() };
        let (glow_alpha, glow_blur, glow_dy) = FORCE_GLOW;
        return base
            .cursor_pointer()
            .bg(oklab_gradient(ANGLE_RIGHT_DEG, deep, deep.rotate_hue(FORCE_HUE_DEG)))
            .shadow(vec![glow(deep.with_alpha(glow_alpha), glow_blur, 0.0, glow_dy)])
            .text_color(ink.hsla())
            .child(icon("bolt", SMALL_ICON_PX, ink))
            .child(label)
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                // Only still-running members (checked at click time).
                let alive = targets(true);
                h.borrow_mut().end(&key, &alive, true);
                refresh(&h, true, cx);
                let views = h.borrow().views.clone();
                views.notify(cx);
            })
            .into_any_element();
    }
    let ink = ink_on(coral);
    let soft = readable(coral, palette);
    let label: SharedString = match (armed, n > 1) {
        (true, false) => t("Confirm").into(),
        (true, true) => tf("Confirm ×{count}", &[("count", &n)]).into(),
        (false, false) => t("End").into(),
        (false, true) => t("End all").into(),
    };
    let (soft_fill, hover_fill) = (alpha(ACTION_SOFT, palette), alpha(ACTION_HOVER, palette));
    let (glow_alpha, glow_blur, glow_dy) = ARMED_GLOW;
    base.cursor_pointer()
        .when(armed, |el| el.bg(coral.hsla()).text_color(ink.hsla()).shadow(vec![glow(coral.with_alpha(glow_alpha), glow_blur, 0.0, glow_dy)]))
        .when(!armed, |el| {
            el.bg(coral.with_alpha(soft_fill).hsla())
                .text_color(soft.hsla())
                .hover(move |s| s.bg(coral.with_alpha(hover_fill).hsla()))
                .when(!pinned, |el| el.invisible().group_hover("proc-row", |s| s.visible()))
        })
        .child(icon(if armed { "check" } else { "close" }, SMALL_ICON_PX, if armed { ink } else { soft }))
        .child(label)
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            let mut hb = h.borrow_mut();
            if hb.armed(&key) {
                hb.end(&key, &targets(false), false);
                drop(hb);
                refresh(&h, true, cx);
            } else {
                hb.armed.insert(key.clone(), Instant::now());
                hb.selected = Some(key.clone());
                drop(hb);
            }
            let views = h.borrow().views.clone();
            views.notify(cx);
        })
        .into_any_element()
}

fn row_shell(id: ElementId, height: f32, radius: f32, selected: bool, armed: bool, palette: &MandaraPalette) -> gpui::Stateful<gpui::Div> {
    let hover = palette.on_surface.with_alpha(alpha(ROW_HOVER, palette));
    let sel = readable(palette.primary, palette).with_alpha(alpha(ROW_SELECTED, palette));
    let warn = palette.urgent_coral.with_alpha(alpha(ROW_ARMED, palette));
    div()
        .id(id)
        .group("proc-row")
        .h(px(height))
        .flex_none()
        .px(px(ROW_PAD_X_PX))
        .rounded(px(radius))
        .flex()
        .items_center()
        .gap(px(ROW_INNER_GAP_PX))
        .cursor_pointer()
        .when(armed, |el| el.bg(warn.hsla()))
        .when(selected && !armed, |el| el.bg(sel.hsla()))
        .when(!selected && !armed, |el| el.hover(move |s| s.bg(hover.hsla())))
}

/// Name line: name, optional count / owner / PID badges.
fn title_line(name: &str, count: usize, owner: Option<&str>, pid: Option<u32>, palette: &MandaraPalette) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(TITLE_GAP_PX))
        .min_w(px(0.0))
        .child(div().min_w(px(0.0)).text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).truncate().child(name.to_string()))
        .when(count > 1, |el| el.child(badge(format!("×{count}"), readable(palette.primary, palette), palette)))
        .when_some(owner, |el, u| el.child(badge(u.to_string(), readable(palette.tertiary, palette), palette)))
        .when_some(pid, |el, pid| {
            el.child(div().flex_none().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(pid.to_string()))
        })
        .into_any_element()
}

fn sub_line(s: &str, palette: &MandaraPalette) -> AnyElement {
    div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.with_alpha(SUB_LINE_ALPHA).hsla()).truncate().child(s.to_string()).into_any_element()
}

fn single_row(p: &Process, h: &Shared, palette: &MandaraPalette, scale: CpuScale, primed: bool, radius: f32, child: bool) -> AnyElement {
    let key = Key::Pid(p.pid);
    let (selected, armed) = {
        let hb = h.borrow();
        (hb.selected.as_ref() == Some(&key), hb.armed(&key))
    };
    let owner = (p.uid != my_uid()).then_some(p.user.as_str());
    let cmd = if p.cmdline.is_empty() { &p.name } else { &p.cmdline };
    let h2 = h.clone();
    let k2 = key.clone();
    let lead = if child {
        // Children of an expanded group: a guide line instead of the icon.
        div()
            .w(px(AVATAR_PX))
            .h_full()
            .flex_none()
            .flex()
            .justify_center()
            .child(div().w(px(GUIDE_W_PX)).h_full().rounded_full().bg(palette.on_surface.with_alpha(GUIDE_ALPHA).hsla()))
            .into_any_element()
    } else {
        avatar(&p.name, AVATAR_PX, state_dot(p.state, palette), palette)
    };
    row_shell(ElementId::from(("proc", p.pid as usize)), if child { CHILD_H } else { ROW_H }, radius, selected, armed, palette)
        .child(lead)
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(1.0))
                .child(if child {
                    div()
                        .flex()
                        .items_center()
                        .gap(px(TITLE_GAP_PX))
                        .child(div().font_features(tabular_nums()).text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).child(tf("PID {pid}", &[("pid", &p.pid)])))
                        .when_some(owner, |el, u| el.child(badge(u.to_string(), readable(palette.tertiary, palette), palette)))
                        .when_some(state_dot(p.state, palette), |el, c| el.child(div().size(px(CHILD_DOT_PX)).rounded_full().bg(c.hsla())))
                        .into_any_element()
                } else {
                    title_line(&p.name, 1, owner, Some(p.pid), palette)
                })
                .child(sub_line(cmd, palette)),
        )
        .child(cpu_cell(p.cpu, scale, primed, palette))
        .child(mem_cell(p.rss, palette))
        .child(div().w(px(ACTION_COL_W_PX)).flex_none().flex().justify_end().child(action(key, &p.name, vec![p.pid], h, palette, selected || armed)))
        .on_click(move |_, _, cx| {
            let views = {
                let mut hb = h2.borrow_mut();
                hb.selected = if hb.selected.as_ref() == Some(&k2) { None } else { Some(k2.clone()) };
                hb.views.clone()
            };
            views.notify(cx);
        })
        .into_any_element()
}

fn group_row(g: &Group, results: &[Process], h: &Shared, palette: &MandaraPalette, scale: CpuScale, primed: bool, radius: f32, rows: &mut Vec<AnyElement>) {
    let key = Key::Group(g.name.clone());
    let (selected, armed, expanded) = {
        let hb = h.borrow();
        (hb.selected.as_ref() == Some(&key), hb.armed(&key), hb.expanded.contains(&g.name))
    };
    let me = my_uid();
    let members = || g.members.iter().map(|&i| &results[i]);
    let owner = members().find(|p| p.uid != me).map(|p| p.user.as_str());
    let pids: Vec<String> = members().take(PID_PREVIEW).map(|p| p.pid.to_string()).collect();
    let more = if g.members.len() > PID_PREVIEW { "…" } else { "" };
    let detail = tf("{count} processes · PIDs {pids}{more}", &[("count", &g.members.len()), ("pids", &pids.join(", ")), ("more", &more)]);
    let busiest = members().map(|p| p.state).find(|s| *s == 'R').unwrap_or('S');
    let (h2, name) = (h.clone(), g.name.clone());
    let chevron = div()
        .size(px(CHEVRON_BOX_PX))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .child(
            gpui::svg()
                .path("icons/chevron-right.svg")
                .size(px(SMALL_ICON_PX))
                .flex_none()
                .text_color(palette.on_surface_variant.hsla())
                .with_transformation(gpui::Transformation::rotate(gpui::radians(if expanded { std::f32::consts::FRAC_PI_2 } else { 0.0 }))),
        );
    let head = row_shell(name_id("grp", &g.name), ROW_H, radius, selected || expanded, armed, palette)
        .child(avatar(&g.name, AVATAR_PX, state_dot(busiest, palette), palette))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(1.0))
                .child(div().flex().items_center().gap(px(CHEVRON_GAP_PX)).min_w(px(0.0)).child(title_line(&g.name, g.members.len(), owner, None, palette)).child(chevron))
                .child(sub_line(&detail, palette)),
        )
        .child(cpu_cell(g.cpu, scale, primed, palette))
        .child(mem_cell(g.rss, palette))
        .child(div().w(px(ACTION_COL_W_PX)).flex_none().flex().justify_end().child(action(key, &g.name, g.pids(results), h, palette, selected || armed)))
        .on_click(move |_, _, cx| {
            let views = {
                let mut hb = h2.borrow_mut();
                if !hb.expanded.remove(&name) {
                    hb.expanded.insert(name.clone());
                }
                hb.selected = Some(Key::Group(name.clone()));
                hb.views.clone()
            };
            views.notify(cx);
        })
        .into_any_element();
    rows.push(head);
    if expanded {
        rows.extend(members().map(|p| single_row(p, h, palette, scale, primed, radius, true)));
    }
}

#[allow(clippy::too_many_arguments)]
fn tile(icon_name: &'static str, label: &'static str, value: SharedString, detail: String, history: &History, (from, to): (Color, Color), radius: f32, palette: &MandaraPalette) -> AnyElement {
    glass_card(palette)
        .rounded(px(radius))
        .flex_1()
        .min_w(px(0.0))
        .h(px(TILE_H))
        .px(px(TILE_PAD_X_PX))
        .py(px(TILE_PAD_Y_PX))
        .flex()
        .gap(px(TILE_GAP_PX))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(TILE_LABEL_GAP_PX))
                        .child(icon(icon_name, TILE_ICON_PX, from))
                        .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).child(label)),
                )
                .child(div().font_features(tabular_nums()).text_size(px(TILE_VALUE_PX)).line_height(px(TILE_VALUE_LINE_PX)).font_weight(FontWeight::MEDIUM).child(value))
                .child(div().font_features(tabular_nums()).text_size(px(TILE_DETAIL_PX)).text_color(palette.on_surface_variant.hsla()).whitespace_nowrap().truncate().child(detail)),
        )
        .child(div().w(px(TILE_SPARK_W_PX)).flex_none().flex().items_end().child(sparkline(history, TILE_SPARK_H_PX, from.with_alpha(TILE_SPARK_FROM_ALPHA), to, palette)))
        .into_any_element()
}

fn segmented(h: &Shared, palette: &MandaraPalette) -> AnyElement {
    let current = h.borrow().sort;
    let accent = readable(palette.primary, palette);
    let active_fill = alpha(SEG_ACTIVE, palette);
    let seg = |label: &'static str, icon_name: &'static str, which: SortBy| {
        let active = current == which;
        let h = h.clone();
        let hover = palette.on_surface.with_alpha(SEG_HOVER_ALPHA);
        div()
            .id(label)
            .h(px(SEARCH_H - SEG_INSET_PX * 2.0))
            .px(px(SEG_PAD_X_PX))
            .rounded(px(crate::style::radius(SEG_RADIUS_PX)))
            .flex()
            .items_center()
            .gap(px(SEG_GAP_PX))
            .cursor_pointer()
            .text_size(px(ACTION_TEXT_PX))
            .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
            .when(active, |el| el.bg(accent.with_alpha(active_fill).hsla()).text_color(accent.hsla()))
            .when(!active, |el| el.text_color(palette.on_surface_variant.hsla()).hover(move |s| s.bg(hover.hsla())))
            .child(icon(icon_name, SMALL_ICON_PX, if active { accent } else { palette.on_surface_variant }))
            .child(crate::bidi::paragraph(tr(label)))
            .on_click(move |_, _, cx| {
                let views = {
                    let mut hb = h.borrow_mut();
                    hb.sort = which;
                    hb.views.clone()
                };
                views.notify(cx);
            })
    };
    div()
        .h(px(SEARCH_H))
        .flex_none()
        .p(px(SEG_INSET_PX))
        .rounded(px(crate::style::radius(CONTROL_RADIUS_PX)))
        .bg(palette.on_surface.with_alpha(alpha(SEG_TRACK, palette)).hsla())
        .flex()
        .items_center()
        .gap(px(SEG_TRACK_GAP_PX))
        .child(seg(tk("CPU"), "cpu", SortBy::Cpu))
        .child(seg(tk("Memory"), "memory", SortBy::Memory))
        .child(seg(tk("Name"), "list-check", SortBy::Name))
        .into_any_element()
}

fn search_box(field: &Entity<TextField>, query: &str, views: &ViewSet, palette: &MandaraPalette) -> AnyElement {
    let (f, views) = (field.clone(), views.clone());
    div()
        .flex_1()
        .min_w(px(0.0))
        .h(px(SEARCH_H))
        .pl(px(SEARCH_PAD_L_PX))
        .pr(px(SEARCH_PAD_R_PX))
        .rounded(px(crate::style::radius(CONTROL_RADIUS_PX)))
        .bg(palette.surface_container_high.hsla())
        .flex()
        .items_center()
        .gap(px(SEG_TRACK_GAP_PX))
        .child(icon("search", SEARCH_ICON_PX, palette.on_surface_variant))
        .child(div().flex_1().min_w(px(0.0)).child(field.clone()))
        .when(!query.is_empty(), |el| {
            el.child(icon_button("proc-clear", "close", CLEAR_BUTTON_PX, palette.on_surface_variant, None, move |_, cx| {
                f.update(cx, |f, _| f.set(""));
                views.notify(cx);
            }))
        })
        .into_any_element()
}

fn toast(ok: bool, msg: SharedString, palette: &MandaraPalette) -> AnyElement {
    let tint = readable(if ok { palette.success_emerald } else { palette.urgent_coral }, palette);
    div()
        .absolute()
        .left(px(PAD))
        .right(px(PAD))
        .bottom(px(PAD))
        .flex()
        .justify_center()
        .child(
            div()
                .max_w(px(PANEL_W - PAD * TOAST_SIDE_PADS))
                .h(px(TOAST_H))
                .pl(px(TOAST_PAD_L_PX))
                .pr(px(TOAST_PAD_R_PX))
                .rounded_full()
                .bg(palette.surface_container_high.hsla())
                .shadow(vec![glow(Color::oklch(0.0, 0.0, 0.0).with_alpha(alpha(TOAST_SHADOW, palette)), TOAST_SHADOW_BLUR_PX, 0.0, TOAST_SHADOW_DY_PX)])
                .flex()
                .items_center()
                .gap(px(tokens::SM))
                .child(
                    div()
                        .size(px(TOAST_BADGE_PX))
                        .flex_none()
                        .rounded_full()
                        .bg(tint.with_alpha(TOAST_BADGE_ALPHA).hsla())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(if ok { "check" } else { "close" }, SMALL_ICON_PX, tint)),
                )
                .child(div().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).child(msg)),
        )
        .into_any_element()
}

// ── Panel ────────────────────────────────────────────────────────────────

struct View {
    p: PanelCx,
    h: Shared,
    field: Entity<TextField>,
    size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>,
}

impl View {
    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let h = &self.h;
        h.borrow().views.track(window);
        if self.p.progress() > 0.0 {
            refresh(h, false, cx);
            redraw_in(sample_every(cx), window, cx);
            let watch = h.borrow().close_watch.clone();
            if !watch.get() {
                let freed = h.clone();
                on_panel_close(&self.p, &watch, cx, move |_| freed.borrow_mut().free());
            }
        }
        let shape = PanelShape::of(cx);
        let card_r = shape.inner(PAD);
        let row_r = (card_r - ROW_RADIUS_INSET_PX).max(ROW_MIN_RADIUS_PX);
        let query = self.field.read(cx).value.clone();
        let q = query.trim();
        // A PID query always shows the individual process.
        let pid_query = !q.is_empty() && q.chars().all(|c| c.is_ascii_digit());

        // Transient states (arming, grace, toast) and the live strip's
        // sparklines (the hub's smoothed system CPU / memory).
        let sys = crate::hub::resources_entity(cx).read(cx).clone();
        let status = {
            let mut hb = h.borrow_mut();
            let grouping = hb.grouped && !pid_query;
            hb.ensure_list(q, grouping);
            let toast_for = crate::state::config(cx).read(cx).snapshot().panels.toast();
            if hb.status.as_ref().is_some_and(|(_, _, at)| at.elapsed() >= toast_for) {
                hb.status = None;
            }
            if !hb.armed.is_empty() || !hb.terminated.is_empty() {
                redraw_in(TRANSIENT_TICK, window, cx);
            }
            let now = Instant::now();
            hb.cpu.push_every(sys.cpu, HISTORY_GAP, now);
            hb.mem.push_every(sys.mem, HISTORY_GAP, now);
            hb.status.clone()
        };
        if let Some((_, _, at)) = &status {
            redraw_in(crate::state::config(cx).read(cx).snapshot().panels.toast().saturating_sub(at.elapsed()) + EXPIRY_MARGIN, window, cx);
        }

        let hb = h.borrow();
        let (total, totals, primed, grouped) = (hb.procs.len(), hb.totals, hb.sampled.is_some(), hb.grouped);
        let grouping = grouped && !pid_query;
        let list_data = &hb.list;
        let (results, groups) = (&list_data.results, &list_data.groups);
        let scale = CpuScale { max: list_data.max_cpu, hot: hot_cpu(cx) };
        let entries = if grouping { groups.len() } else { results.len() };
        let shown = entries.min(ROWS);

        // Rows (and their heights, to size the panel to its content).
        let mut list_h = 0.0;
        let mut rows: Vec<AnyElement> = Vec::with_capacity(shown);
        if grouping {
            for g in groups.iter().take(ROWS) {
                if let [only] = g.members[..] {
                    rows.push(single_row(&results[only], h, palette, scale, primed, row_r, false));
                    list_h += ROW_H + ROW_GAP;
                } else {
                    let before = rows.len();
                    group_row(g, results, h, palette, scale, primed, row_r, &mut rows);
                    list_h += ROW_H + ROW_GAP + (rows.len() - before - 1) as f32 * (CHILD_H + ROW_GAP);
                }
            }
        } else {
            for p in results.iter().take(ROWS) {
                rows.push(single_row(p, h, palette, scale, primed, row_r, false));
                list_h += ROW_H + ROW_GAP;
            }
        }
        let empty = rows.is_empty();
        let more = entries.saturating_sub(shown);
        let footer_h = if more > 0 { FOOTER_H } else { 0.0 };
        // Until the first sample, hold the last list height (placeholder).
        let placeholder_h = hb.placeholder_h;
        let list_h = if !primed { placeholder_h } else if empty { EMPTY_H } else { list_h.min(LIST_MAX_H) };
        let block_h = if primed { list_h + footer_h } else { placeholder_h };
        let height = PAD * 2.0 + HEADER_H + GAP + TILE_H + GAP + SEARCH_H + CONTROLS_GAP_PX + COLS_H + COLUMNS_GAP_PX + block_h;
        if (f32::from(self.size_cell.get().height) - height).abs() > RESIZE_EPSILON_PX {
            self.size_cell.set(size(px(PANEL_W), px(height)));
        }

        // ── Header ───────────────────────────────────────────────────────
        let accent = readable(palette.primary, palette);
        let subtitle = if q.is_empty() {
            if grouping {
                tf("{n} processes · {apps} apps", &[("n", &total), ("apps", &groups.len())])
            } else {
                tf("{n} processes", &[("n", &total)])
            }
        } else {
            tf("{matches} of {total} match", &[("matches", &results.len()), ("total", &total)])
        };
        let group_toggle = {
            let h = h.clone();
            let on = grouped;
            let hover = palette.on_surface.with_alpha(GROUP_TOGGLE_HOVER_ALPHA);
            div()
                .id("proc-group")
                .h(px(PILL_H_PX))
                .px(px(ACTION_PAD_X_PX))
                .flex_none()
                .rounded_full()
                .flex()
                .items_center()
                .gap(px(ACTION_GAP_PX))
                .cursor_pointer()
                .text_size(px(ACTION_TEXT_PX))
                .font_weight(FontWeight::SEMIBOLD)
                .when(on, |el| el.bg(accent.with_alpha(alpha(SEG_ACTIVE, palette)).hsla()).text_color(accent.hsla()))
                .when(!on, |el| el.bg(palette.on_surface.with_alpha(GROUP_TOGGLE_OFF_ALPHA).hsla()).text_color(palette.on_surface_variant.hsla()).hover(move |s| s.bg(hover.hsla())))
                .child(icon("grid", SMALL_ICON_PX, if on { accent } else { palette.on_surface_variant }))
                .child(t("Group"))
                .on_click(move |_, _, cx| {
                    let views = {
                        let mut hb = h.borrow_mut();
                        hb.grouped = !hb.grouped;
                        hb.selected = None;
                        hb.views.clone()
                    };
                    views.notify(cx);
                })
        };
        let head = header("crosshair", accent, t("Process hunter"), subtitle, palette, Some(group_toggle.into_any_element()));

        // ── Live strip ───────────────────────────────────────────────────
        let pair = |c: Color| (readable(c, palette), readable(partner(c), palette));
        let cpu_detail = if primed && totals.cores > 0 {
            tf("{cores} cores · {running} running", &[("cores", &totals.cores), ("running", &totals.running)])
        } else {
            t("sampling…").to_string()
        };
        let mem_detail = if totals.mem_total > 0 {
            tf("{used} / {total}", &[("used", &format_bytes(totals.mem_used)), ("total", &format_bytes(totals.mem_total))])
        } else {
            t("sampling…").to_string()
        };
        let strip = div()
            .flex()
            .gap(px(TILE_GAP_PX))
            .child(tile("cpu", t("CPU"), percent_text(sys.cpu), cpu_detail, &hb.cpu, pair(palette.primary), card_r, palette))
            .child(tile("memory", t("Memory"), percent_text(sys.mem), mem_detail, &hb.mem, pair(palette.tertiary), card_r, palette));
        let views = hb.views.clone();
        drop(hb);
        if primed {
            h.borrow_mut().placeholder_h = block_h;
        }

        // ── Search + sort ────────────────────────────────────────────────
        let controls = div().flex().items_center().gap(px(tokens::SM)).child(search_box(&self.field, &query, &views, palette)).child(segmented(h, palette));

        let label = |s: &'static str| div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.with_alpha(COLUMN_LABEL_ALPHA).hsla()).child(s);
        let columns = div()
            .h(px(COLS_H))
            .flex()
            .items_center()
            .gap(px(ROW_INNER_GAP_PX))
            .px(px(ROW_PAD_X_PX))
            .child(div().w(px(AVATAR_PX)).flex_none())
            .child(div().flex_1().child(label(if grouping { t("APP") } else { t("PROCESS") })))
            .child(div().w(px(CPU_COL_W_PX)).flex().justify_end().child(label(t("CPU"))))
            .child(div().w(px(MEM_COL_W_PX)).flex().justify_end().child(label(t("MEMORY"))))
            .child(div().w(px(ACTION_COL_W_PX)).flex_none());

        let list: AnyElement = if empty {
            if !primed {
                empty_state("crosshair", t("Scanning processes…"), t("Reading the process table"), list_h, palette).into_any_element()
            } else {
                empty_state("search", tf("No process matches \u{201c}{query}\u{201d}", &[("query", &q)]), t("Try a name, part of a command line or a PID"), EMPTY_H, palette).into_any_element()
            }
        } else {
            let fill = crate::shell::frame_color(palette);
            let clipped = list_h >= LIST_MAX_H - CLIP_EPSILON_PX;
            div()
                .relative()
                .h(px(list_h))
                .child(div().id("proc-list").size_full().overflow_y_scroll().flex().flex_col().gap(px(ROW_GAP)).children(rows))
                // Soft fade where the list scrolls under the panel edge.
                .when(clipped, |el| {
                    el.child(div().absolute().left_0().right_0().bottom_0().h(px(LIST_FADE_PX)).bg(oklab_gradient(ANGLE_DOWN_DEG, fill.with_alpha(0.0), fill)))
                })
                .into_any_element()
        };

        let p = &self.p;
        div()
            .relative()
            .size_full()
            .p(px(PAD))
            .flex()
            .flex_col()
            .child(reveal(p, 0, head))
            .child(div().h(px(GAP)))
            .child(reveal(p, 1, strip))
            .child(div().h(px(GAP)))
            .child(reveal(p, 2, controls))
            .child(div().h(px(CONTROLS_GAP_PX)))
            .child(reveal(
                p,
                3,
                div()
                    .flex()
                    .flex_col()
                    .gap(px(COLUMNS_GAP_PX))
                    .child(columns)
                    .child(list)
                    .when(more > 0, |el| {
                        el.child(
                            div()
                                .h(px(FOOTER_H - FOOTER_INSET_PX))
                                .flex()
                                .items_center()
                                .justify_center()
                                .gap(px(FOOTER_GAP_PX))
                                .text_size(px(text::LABEL))
                                .text_color(palette.on_surface_variant.hsla())
                                .child(icon("search", FOOTER_ICON_PX, palette.on_surface_variant))
                                .child(tf("+{count} more — refine the search", &[("count", &more)])),
                        )
                    }),
            ))
            .when_some(status, |el, (ok, msg, _)| el.child(toast(ok, msg, palette)))
            .into_any_element()
    }
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Process hunter"), tk("Find running processes by name or PID and end them"), None).opt_in(), |window, cx, panel_cx| {
        let hunter: Shared = Rc::new(RefCell::new(Hunter::default()));
        // Typing re-renders only this panel, not every window.
        let views = hunter.borrow().views.clone();
        let field = cx.new(|cx| TextField::new(String::new(), t("Name, command or PID…"), true, Rc::new(move |_, _, cx| views.notify(cx)), cx));
        let bar = fn_view(None, |palette, _, _| icon("crosshair", crate::style::bar_icon(BAR_ICON_PX), palette.on_surface).into_any_element(), window, cx);
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(INITIAL_H))));
        let view = View { p: panel_cx.clone(), h: hunter, field, size_cell: size_cell.clone() };
        let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), ..PluginInstance::empty() }
    });
    r.command(
        ID,
        CommandSpec::new("processes list", tk("Running processes matching a name, command or PID (CPU order)"), |a, _| {
            let mut s = Sampler::new();
            s.sample();
            // CPU needs two tables; the command is synchronous by contract.
            std::thread::sleep(BASELINE_GAP);
            let procs = s.sample();
            Ok(json!(processes::search(&procs, &a.rest(0), SortBy::Cpu).into_iter().take(CLI_LIST_MAX).collect::<Vec<_>>()))
        })
        .arg(ArgSpec::optional("query", ArgKind::String, tk("name, command line or PID")))
        .example("processes list firefox"),
    );
    r.command(
        ID,
        CommandSpec::new("processes kill", tk("End a process by PID (SIGTERM; --force sends SIGKILL)"), |a, _| {
            let pid: u32 = a.get(0).and_then(|s| s.parse().ok()).ok_or("expected a PID")?;
            let force = a.get(1) == Some("--force");
            processes::kill(pid, force)?;
            Ok(json!({ "pid": pid, "signal": if force { "KILL" } else { "TERM" } }))
        })
        .arg(ArgSpec::required("pid", ArgKind::Integer, tk("process ID")))
        .arg(ArgSpec::optional("force", ArgKind::String, tk("send SIGKILL instead of SIGTERM")).choices(&["--force"]))
        .example("processes kill 4242"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn proc(pid: u32, name: &str, cpu: f32, rss: u64) -> Process {
        let mut s = Sampler::new();
        // Build from a real process so private fields are initialised, then
        // override the public ones.
        let mut p = s.sample().into_iter().next().expect("at least one process");
        p.pid = pid;
        p.name = name.into();
        p.cpu = cpu;
        p.rss = rss;
        p
    }

    #[test]
    fn groups_sum_and_order() {
        let ps = vec![proc(1, "b", 5.0, 10), proc(2, "a", 1.0, 300), proc(3, "b", 7.0, 20), proc(4, "c", 30.0, 5)];
        let g = group(&ps, SortBy::Cpu, false);
        assert_eq!(g.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["c", "b", "a"]);
        assert_eq!(g[1].members.len(), 2);
        assert!((g[1].cpu - 12.0).abs() < 1e-6);
        assert_eq!(g[1].rss, 30);
        assert_eq!(g[1].pids(&ps), vec![1, 3]);
        let by_mem = group(&ps, SortBy::Memory, false);
        assert_eq!(by_mem[0].name, "a");
        // A query keeps relevance order (first appearance).
        let ranked = group(&ps, SortBy::Cpu, true);
        assert_eq!(ranked.iter().map(|g| g.name.as_str()).collect::<Vec<_>>(), ["b", "a", "c"]);
    }

    #[test]
    fn list_is_derived_once_per_sample_and_query() {
        let mut h = Hunter::default();
        h.absorb(vec![proc(1, "qqbeta", 5.0, 10), proc(2, "qqalpha", 1.0, 300)], (1, 2));
        h.ensure_list("", true);
        let first = h.list.key.clone();
        h.ensure_list("", true);
        assert_eq!(h.list.key, first, "same inputs keep the cache");
        assert_eq!(h.list.groups.len(), 2);
        h.ensure_list("qqalpha", true);
        assert_eq!(h.list.results.len(), 1);
        h.absorb(vec![proc(1, "qqbeta", 5.0, 10)], (1, 2));
        h.ensure_list("qqalpha", true);
        assert!(h.list.results.is_empty(), "a new sample re-derives the list");
        assert!(h.list.max_cpu >= CPU_SCALE_FLOOR);
    }

    #[test]
    fn closing_frees_panel_state_but_keeps_layout() {
        let mut h = Hunter::default();
        h.absorb(vec![proc(1, "qqbeta", 5.0, 10)], (1, 2));
        h.ensure_list("", true);
        h.placeholder_h = 321.0;
        h.sort = SortBy::Memory;
        let epoch = h.epoch;
        h.free();
        assert!(h.procs.is_empty() && h.list.results.is_empty() && h.list.key.is_none());
        assert!(h.sampled.is_none() && h.cpu.is_empty() && h.warmed.is_empty());
        assert_ne!(h.epoch, epoch, "in-flight samples are discarded");
        assert_eq!((h.placeholder_h, h.sort), (321.0, SortBy::Memory));
    }

    #[test]
    fn ink_contrasts_with_fill() {
        assert!(ink_on(Color::oklch(0.8, 0.12, 30.0)).lightness() < 0.4);
        assert!(ink_on(Color::oklch(0.5, 0.15, 30.0)).lightness() > 0.9);
    }
}
