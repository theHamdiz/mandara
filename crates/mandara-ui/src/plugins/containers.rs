//! Containers: Docker and Podman at a glance. The bar chip shows how many
//! containers are running. The panel lists them, optionally grouped by
//! compose project, with live CPU / memory per running container, uptime,
//! published ports and health, plus start / stop / restart / pause and a
//! logs view of the last lines. Talks to the engines' HTTP APIs over their
//! Unix sockets (`mandara_services::containers`).
//!
//! One poller is shared by every bar instance. It lists containers every
//! `refresh_s` (30 s by default) while the chip exists. Stats, start times and image / volume counts
//! are read only while the panel is open. Commands:
//! `containers list|stats|logs|start|stop|restart|pause|unpause`.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext as _, ElementId, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px, size,
};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk, tr};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry};
use mandara_services::containers::{
    self as api, Action, Client, Container, DEFAULT_LOG_LINES, Engine, Group, Health, LogLine, Monitor, Preference, Snapshot, State, Stream,
};
use mandara_theme::{Color, MandaraPalette};
use serde_json::{Value, json};

use super::fn_view;
use super::panel_fx::{
    ANGLE_DIAGONAL_DEG, ANGLE_DOWN_DEG, EXPIRY_MARGIN, HEADER_H, History, PILL_H_PX, PanelShape, SMALL_ICON_PX, ViewSet, alpha, empty_state, glass_card,
    glow, header, meter, partner, readable, redraw_in, sparkline,
};
use crate::fonts::tabular_nums;
use crate::icons::icon;
use crate::kit::{icon_button, reveal, text, tokens};
use crate::text_field::TextField;
use crate::widgets::system::format_bytes;

pub const ID: &str = "containers";

const PANEL_W: f32 = 600.0;
/// A container name never truncates below this width.
const NAME_MIN_W: f32 = 64.0;
/// The project badge shrinks this many times faster than the name.
const PROJECT_BADGE_SHRINK: f32 = 8.0;
const PAD: f32 = 14.0;
const GAP: f32 = 12.0;
const TILE_H: f32 = 74.0;
const SEARCH_H: f32 = 36.0;
const COLS_H: f32 = 18.0;
const ROW_H: f32 = 54.0;
const CHILD_H: f32 = 50.0;
const ROW_GAP: f32 = 2.0;
/// Rows visible before the list scrolls (the cut half row hints at more).
const VISIBLE_ROWS: f32 = 7.5;
/// Visible list height before it scrolls (a cut row hints at more).
const LIST_MAX_H: f32 = VISIBLE_ROWS * (ROW_H + ROW_GAP);
const EMPTY_H: f32 = 190.0;
const ERROR_H: f32 = 230.0;
const LOGS_H: f32 = 392.0;
const LOG_TOOLBAR_H: f32 = 40.0;
const LOG_LINE_H: f32 = 17.0;
const FOOTER_H: f32 = 26.0;
const AVATAR: f32 = 34.0;
const CPU_W: f32 = 68.0;
const MEM_W: f32 = 70.0;
const ACTIONS_W: f32 = 124.0;
/// Rows (containers or projects) rendered at most.
const ROWS: usize = 60;
/// Published ports shown per row before "+n".
const PORT_CHIPS: usize = 2;
/// Sparkline columns of the CPU / memory tiles.
const HISTORY: usize = 18;
/// Minimum spacing of tile sparkline samples.
const HISTORY_EVERY: Duration = Duration::from_millis(1800);
/// How often an open panel re-renders (live readouts, pinging the poller).
const RENDER_EVERY: Duration = Duration::from_secs(1);
/// A Stop press arms the button; a second press within this confirms.
const ARM: Duration = Duration::from_secs(3);
/// CPU above this (percent of one core) reads as hot (`hot_cpu_percent`).
pub const DEFAULT_HOT_CPU_PERCENT: i64 = 80;
pub const HOT_CPU_RANGE: (i64, i64) = (5, 400);

/// CPU meter scale of one render: the busiest container and the hot
/// threshold.
#[derive(Copy, Clone, Debug)]
struct CpuScale {
    max: f32,
    hot: f32,
}

/// How often the container list refreshes while the panel is closed
/// (`refresh_s`, seconds).
fn list_every(cx: &App) -> Duration {
    let secs: f64 = crate::state::config(cx).read(cx).snapshot().plugins.setting(ID, "refresh_s", api::LIST_EVERY.as_secs_f64());
    let (lo, hi) = api::LIST_EVERY_RANGE;
    Duration::from_secs_f64(if secs.is_finite() { secs.clamp(lo.as_secs_f64(), hi.as_secs_f64()) } else { api::LIST_EVERY.as_secs_f64() })
}

/// Hot threshold in percent of one core (`hot_cpu_percent`).
fn hot_cpu(cx: &App) -> f32 {
    let pct: i64 = crate::state::config(cx).read(cx).snapshot().plugins.setting(ID, "hot_cpu_percent", DEFAULT_HOT_CPU_PERCENT);
    pct.clamp(HOT_CPU_RANGE.0, HOT_CPU_RANGE.1) as f32
}
/// Bytes per KiB (`/proc/meminfo` reports KiB).
const KIB: u64 = 1024;
/// Percent per whole (CPU sums are in percent of one core).
const PERCENT: f32 = 100.0;
/// Re-render cadence while a Stop press is armed.
const ARM_TICK: Duration = Duration::from_millis(500);
/// Height changes smaller than this don't resize the panel.
const SIZE_EPSILON_PX: f32 = 0.5;
/// Initial panel height before the first layout.
const INITIAL_H: f32 = 560.0;
/// `sha256:` + 12 hex digits for digest-only images.
const DIGEST_PREFIX_CHARS: usize = 19;
/// FNV-1a 32-bit offset basis and prime (stable per-image hues).
const FNV_OFFSET: u32 = 0x811c_9dc5;
const FNV_PRIME: u32 = 0x0100_0193;
/// Degrees in a hue circle.
const HUE_DEGREES: u32 = 360;

// Visual tokens of this panel (px unless noted; alpha pairs are
// `(dark, light)`).
/// Small tinted label badges and the state / project pills.
const BADGE_H_PX: f32 = 16.0;
const BADGE_PAD_X_PX: f32 = 6.0;
const BADGE_GAP_PX: f32 = 4.0;
const BADGE_TEXT_PX: f32 = 10.0;
const BADGE_TINT: (f32, f32) = (0.14, 0.1);
/// State pill: left / right padding, dot and its glow (alpha, blur).
const STATE_PILL_PAD_L_PX: f32 = 5.0;
const STATE_PILL_PAD_R_PX: f32 = 7.0;
const DOT_PX: f32 = 6.0;
const DOT_GLOW: (f32, f32) = (0.5, 4.0);
/// Port chip radius and fill.
const PORT_RADIUS_PX: f32 = 5.0;
const PORT_FILL: (f32, f32) = (0.07, 0.055);
/// Avatars: radius / glyph as fractions of the side, image-hue tint
/// (lightness, chroma) on dark and light themes, gradient alphas, and the
/// state dot (offset, side).
const AVATAR_RADIUS_FRAC: f32 = 0.32;
const MONOGRAM_FRAC: f32 = 0.42;
const PROJECT_GLYPH_FRAC: f32 = 0.5;
const AVATAR_TINT_DARK: (f32, f32) = (0.78, 0.11);
const AVATAR_TINT_LIGHT: (f32, f32) = (0.48, 0.13);
const AVATAR_FILL: (f32, f32) = (0.2, 0.1);
const STATE_DOT_OFFSET_PX: f32 = -2.0;
const STATE_DOT_PX: f32 = 10.0;
/// Log sheet avatar side.
const LOG_AVATAR_PX: f32 = 28.0;
/// CPU / memory cells: meter width / height and the cool gradient alpha.
const CELL_GAP_PX: f32 = 4.0;
const CPU_METER_W_PX: f32 = 52.0;
const CPU_METER_H_PX: f32 = 4.0;
const COOL_FROM_ALPHA: f32 = 0.75;
/// Rows.
const ROW_PAD_X_PX: f32 = 8.0;
const ROW_INNER_GAP_PX: f32 = 10.0;
const ROW_HOVER: (f32, f32) = (0.05, 0.04);
const ROW_SELECTED: (f32, f32) = (0.1, 0.08);
const ROW_ARMED: (f32, f32) = (0.1, 0.07);
const ROW_LINES_GAP_PX: f32 = 3.0;
const INLINE_GAP_PX: f32 = 6.0;
const SUB_GAP_PX: f32 = 5.0;
const MUTED_TEXT_ALPHA: f32 = 0.82;
/// Child rows hang off a guide line of this alpha.
const GUIDE_ALPHA: f32 = 0.09;
/// Round lifecycle buttons and the Stop / Start-all buttons.
const BUTTON_PX: f32 = PILL_H_PX;
const BUTTON_PAD_X_PX: f32 = 10.0;
const BUSY_PAD_X_PX: f32 = 11.0;
const BUTTON_GAP_PX: f32 = 5.0;
const ACTIONS_GAP_PX: f32 = 4.0;
const MINI_ICON_PX: f32 = 13.0;
const BUTTON_TEXT_PX: f32 = text::LABEL + 0.5;
const MINI_FILL: (f32, f32) = (0.14, 0.1);
const MINI_HOVER: (f32, f32) = (0.26, 0.18);
const STOP_FILL: (f32, f32) = (0.16, 0.11);
const ARMED_GLOW: (f32, f32, f32) = (0.3, 8.0, 1.0);
const BUSY_FILL_ALPHA: f32 = 0.06;
/// Chevron of a project row.
const CHEVRON_BOX_PX: f32 = 16.0;
/// Tiles: padding, value size / line height, caption size, sparkline
/// width / height / start alpha.
const TILE_PAD_X_PX: f32 = 12.0;
const TILE_PAD_Y_PX: f32 = 10.0;
const TILE_ICON_PX: f32 = 13.0;
const TILE_VALUE_PX: f32 = 20.0;
const TILE_VALUE_LINE_PX: f32 = 24.0;
const TILE_CAPTION_PX: f32 = 10.5;
const SPARK_W_PX: f32 = 64.0;
const SPARK_H_PX: f32 = 40.0;
const SPARK_FROM_ALPHA: f32 = 0.75;
/// Segmented controls.
const SEG_INSET_PX: f32 = 3.0;
const SEG_PAD_X_PX: f32 = 10.0;
const SEG_RADIUS_PX: f32 = 9.0;
const SEG_GAP_PX: f32 = 5.0;
const SEGMENTS_RADIUS_PX: f32 = 12.0;
const SEGMENTS_GAP_PX: f32 = 2.0;
const SEGMENTS_FILL: (f32, f32) = (0.05, 0.045);
const SEG_ACTIVE: (f32, f32) = (0.18, 0.13);
const SEG_HOVER_ALPHA: f32 = 0.05;
const TOGGLE_HOVER_ALPHA: f32 = 0.1;
/// Search field.
const SEARCH_PAD_L_PX: f32 = 12.0;
const SEARCH_PAD_R_PX: f32 = 2.0;
const SEARCH_RADIUS_PX: f32 = 12.0;
const SEARCH_ICON_PX: f32 = 14.0;
const CLEAR_BUTTON_PX: f32 = 26.0;
/// Toast: height, padding, shadow (alpha pair, blur, y), icon disc.
const TOAST_H_PX: f32 = 36.0;
const TOAST_PAD_L_PX: f32 = 6.0;
const TOAST_PAD_R_PX: f32 = 14.0;
const TOAST_SHADOW: (f32, f32) = (0.45, 0.14);
const TOAST_SHADOW_BLUR_PX: f32 = 14.0;
const TOAST_SHADOW_DY_PX: f32 = 4.0;
const TOAST_ICON_DISC_PX: f32 = 24.0;
const TOAST_ICON_FILL_ALPHA: f32 = 0.16;
/// The toast stays this many paddings narrower than the panel.
const TOAST_SIDE_PADS: f32 = 4.0;
/// Error card: rings (outer side, ring diameters and alphas), glyph,
/// paddings, hint box.
const ERROR_PAD_X_PX: f32 = 28.0;
const ERROR_RINGS_PX: f32 = 76.0;
const ERROR_RING_D_PX: [f32; 3] = [76.0, 56.0, 38.0];
const ERROR_RING_ALPHA: [f32; 3] = [0.05, 0.08, 0.14];
const ERROR_ICON_PX: f32 = 19.0;
const ERROR_TITLE_GAP_PX: f32 = 8.0;
const HINT_GAP_PX: f32 = 10.0;
const HINT_PAD_X_PX: f32 = 12.0;
const HINT_PAD_Y_PX: f32 = 8.0;
const HINT_RADIUS_PX: f32 = 10.0;
const HINT_FILL: (f32, f32) = (0.05, 0.04);
const HINT_ICON_PX: f32 = 13.0;
/// Logs sheet: toolbar buttons, gaps, well (fill, padding, text), stderr
/// rule alpha and plain-line alpha, nested radius inset.
const LOG_BUTTON_PX: f32 = 30.0;
const LOG_BUTTON_FILL_ALPHA: f32 = 0.06;
const LOG_RELOAD_FILL_ALPHA: f32 = 0.12;
const LOG_GAP_PX: f32 = 10.0;
const LOG_WELL: (f32, f32) = (0.045, 0.035);
const LOG_PAD_PX: f32 = 10.0;
const LOG_TEXT_PX: f32 = 11.5;
const LOG_LINE_GAP_PX: f32 = 8.0;
const STDERR_RULE_ALPHA: f32 = 0.7;
const LOG_TEXT_ALPHA: f32 = 0.86;
const NESTED_RADIUS_INSET_PX: f32 = 4.0;
const MIN_NESTED_RADIUS_PX: f32 = 8.0;
/// Fade over a clipped list's last row; footer glyph; column labels.
const LIST_FADE_H_PX: f32 = 22.0;
const FOOTER_ICON_PX: f32 = 11.0;
const COLUMN_LABEL_ALPHA: f32 = 0.85;
/// Gap between the column labels and the list, and between controls and
/// the list block.
const COLUMNS_GAP_PX: f32 = 4.0;
const CONTROLS_GAP_PX: f32 = 10.0;
/// Rows sit this far inside the card radius, never rounder than the min.
const ROW_RADIUS_INSET_PX: f32 = 2.0;
const MIN_ROW_RADIUS_PX: f32 = 10.0;
/// Bar chip.
const CHIP_GAP_PX: f32 = 5.0;
const CHIP_ICON_PX: f32 = 15.0;
const CHIP_IDLE_ALPHA: f32 = 0.7;
/// Ink on solid fills: fills lighter than this get dark ink.
const INK_FLIP_LIGHTNESS: f32 = 0.64;
const DARK_INK: (f32, f32) = (0.2, 0.02);
const LIGHT_INK_L: f32 = 0.99;

fn preference(cx: &App) -> Preference {
    Preference::parse(&crate::state::config(cx).read(cx).snapshot().plugins.setting(ID, "engine", "auto".to_string()))
}

fn show_stopped(cx: &App) -> bool {
    crate::state::config(cx).read(cx).snapshot().plugins.setting(ID, "show_stopped", true)
}

fn log_lines(cx: &App) -> usize {
    let default = i64::try_from(DEFAULT_LOG_LINES).unwrap_or(i64::MAX);
    let n: i64 = crate::state::config(cx).read(cx).snapshot().plugins.setting(ID, "log_lines", default);
    usize::try_from(n.max(1)).unwrap_or(usize::MAX).min(api::MAX_LOG_LINES)
}

/// Installed RAM in bytes (read once; primed off the UI thread at
/// registration).
fn host_memory() -> u64 {
    static TOTAL: std::sync::OnceLock<u64> = std::sync::OnceLock::new();
    *TOTAL.get_or_init(|| {
        let text = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
        text.lines().find(|l| l.starts_with("MemTotal:")).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()).unwrap_or(0) * KIB
    })
}

/// Logical CPUs (read once: `available_parallelism` reads cgroup files).
fn cores() -> usize {
    static CORES: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *CORES.get_or_init(|| std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1))
}

/// A cheap, stable element-id key for a string (no per-frame allocation).
fn id_hash(s: &str) -> u64 {
    use std::hash::{Hash as _, Hasher as _};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

// ── Shared poller ────────────────────────────────────────────────────────

struct Shared {
    monitor: Monitor,
    pref: Cell<Preference>,
    /// The `refresh_s` list cadence last sent to the poller.
    list_every: Cell<Duration>,
    /// The poller's snapshot, copied once per change (not once per frame).
    snap: RefCell<Rc<Snapshot>>,
    /// Bumped with every copied snapshot (keys the row cache).
    generation: Cell<u64>,
    /// The chip and panel views; poller changes notify only these.
    views: ViewSet,
    /// Panels of this plugin currently open (any monitor).
    open: Cell<usize>,
}

impl Shared {
    fn snapshot(&self) -> Rc<Snapshot> {
        self.snap.borrow().clone()
    }

    /// Copies the poller's snapshot after it changed. While no panel is
    /// open only what the bar chip needs is kept: live stats and inspect
    /// details stay out of the copy.
    fn absorb(&self) {
        let fresh = {
            let s = self.monitor.snapshot.lock();
            if self.open.get() > 0 {
                s.clone()
            } else {
                Snapshot {
                    endpoints: s.endpoints.clone(),
                    endpoint: s.endpoint.clone(),
                    version: s.version.clone(),
                    containers: s.containers.clone(),
                    inspect: HashMap::new(),
                    live: HashMap::new(),
                    images: s.images,
                    volumes: s.volumes,
                    error: s.error.clone(),
                    listed: s.listed,
                }
            }
        };
        *self.snap.borrow_mut() = Rc::new(fresh);
        self.generation.set(self.generation.get().wrapping_add(1));
    }

    /// Follows the `engine` and `refresh_s` settings.
    fn sync(&self, cx: &App) {
        let pref = preference(cx);
        if self.pref.get() != pref {
            self.pref.set(pref);
            self.monitor.set_preference(pref);
        }
        let every = list_every(cx);
        if self.list_every.get() != every {
            self.list_every.set(every);
            self.monitor.set_list_every(every);
        }
    }
}

thread_local! {
    static SHARED: RefCell<Weak<Shared>> = const { RefCell::new(Weak::new()) };
}

/// The poller every bar instance and panel share; it stops when the last
/// holder drops.
fn shared(cx: &mut App) -> Rc<Shared> {
    if let Some(s) = SHARED.with(|w| w.borrow().upgrade()) {
        return s;
    }
    let pref = preference(cx);
    let monitor = api::spawn_monitor(pref);
    let every = list_every(cx);
    monitor.set_list_every(every);
    let changed = monitor.changed.clone();
    let s = Rc::new(Shared { monitor, pref: Cell::new(pref), list_every: Cell::new(every), snap: RefCell::default(), generation: Cell::new(0), views: ViewSet::default(), open: Cell::new(0) });
    s.absorb();
    let weak = Rc::downgrade(&s);
    // Ends when the poller stops (every holder dropped its monitor).
    cx.spawn(async move |cx| {
        while changed.recv().await.is_ok() {
            let Some(s) = weak.upgrade() else { break };
            s.absorb();
            cx.update(|cx| s.views.notify(cx));
        }
    })
    .detach();
    SHARED.with(|w| *w.borrow_mut() = Rc::downgrade(&s));
    s
}

// ── Panel state ──────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum Key {
    Container(String),
    Project(String),
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Filter {
    Running,
    All,
}

/// A log line ready to draw (shared text, no per-frame copies).
struct LogRow {
    stderr: bool,
    text: SharedString,
}

impl LogRow {
    fn new(l: LogLine) -> Self {
        // An empty line still takes a line of height.
        let text = if l.text.is_empty() { SharedString::new_static(" ") } else { l.text.into() };
        Self { stderr: l.stream == Stream::Stderr, text }
    }
}

struct LogsView {
    id: String,
    name: String,
    image: String,
    lines: Option<Result<Rc<[LogRow]>, String>>,
    loading: bool,
    scroll: ScrollHandle,
}

/// Rows of the list, recomputed only when the snapshot, query, filter or
/// grouping changes.
struct RowsCache {
    key: (u64, String, Filter, bool),
    /// Matching containers (ungrouped view).
    shown: Vec<Container>,
    /// Matching containers by compose project (grouped view).
    groups: Vec<Group>,
}

struct Ui {
    filter: Filter,
    grouped: bool,
    expanded: HashSet<String>,
    armed: HashMap<Key, Instant>,
    /// Container id → action in flight.
    busy: HashMap<String, Action>,
    /// (ok, message, shown at).
    status: Option<(bool, String, Instant)>,
    logs: Option<LogsView>,
    cpu: History,
    mem: History,
}

impl Default for Ui {
    fn default() -> Self {
        Self {
            filter: Filter::All,
            grouped: true,
            expanded: HashSet::new(),
            armed: HashMap::new(),
            busy: HashMap::new(),
            status: None,
            logs: None,
            cpu: History::new(HISTORY),
            mem: History::new(HISTORY),
        }
    }
}

impl Ui {
    fn armed(&self, key: &Key) -> bool {
        self.armed.get(key).is_some_and(|t| t.elapsed() < ARM)
    }
}

type UiRc = Rc<RefCell<Ui>>;

/// Result text for a finished action on `count` containers (`name` when one).
fn done_message(action: Action, name: &str, count: usize) -> String {
    if count == 1 {
        let args: [(&str, &dyn std::fmt::Display); 1] = [("name", &name)];
        match action {
            Action::Start => tf("Started {name}", &args),
            Action::Stop => tf("Stopped {name}", &args),
            Action::Restart => tf("Restarted {name}", &args),
            Action::Pause => tf("Paused {name}", &args),
            Action::Unpause => tf("Resumed {name}", &args),
        }
    } else {
        let args: [(&str, &dyn std::fmt::Display); 1] = [("count", &count)];
        match action {
            Action::Start => tf("Started {count} containers", &args),
            Action::Stop => tf("Stopped {count} containers", &args),
            Action::Restart => tf("Restarted {count} containers", &args),
            Action::Pause => tf("Paused {count} containers", &args),
            Action::Unpause => tf("Resumed {count} containers", &args),
        }
    }
}

fn busy_label(action: Action) -> String {
    match action {
        Action::Start => t("Starting…"),
        Action::Stop => t("Stopping…"),
        Action::Restart => t("Restarting…"),
        Action::Pause => t("Pausing…"),
        Action::Unpause => t("Resuming…"),
    }
    .to_string()
}

/// Runs `action` on `targets` (`(id, name)`), then shows the outcome.
type Targets = Rc<[(String, String)]>;

fn run(ui: &UiRc, sh: &Rc<Shared>, targets: Targets, action: Action, cx: &mut App) {
    if targets.is_empty() {
        return;
    }
    let pending: Vec<_> = {
        let mut u = ui.borrow_mut();
        u.armed.clear();
        targets
            .iter()
            .map(|(id, _)| {
                u.busy.insert(id.clone(), action);
                sh.monitor.act(id, action)
            })
            .collect()
    };
    sh.views.notify(cx);
    let (ui, sh) = (ui.clone(), sh.clone());
    cx.spawn(async move |cx| {
        let mut errors: Vec<String> = Vec::new();
        for rx in pending {
            match rx.recv().await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => errors.push(e),
                Err(_) => errors.push(t("The service did not reply").into()),
            }
        }
        {
            let mut u = ui.borrow_mut();
            for (id, _) in targets.iter() {
                u.busy.remove(id);
            }
            let n = targets.len();
            let msg = match errors.first() {
                None => done_message(action, &targets[0].1, n),
                Some(e) if n == 1 => e.clone(),
                Some(e) => tf("{failed} of {count} failed: {error}", &[("failed", &errors.len()), ("count", &n), ("error", e)]),
            };
            u.status = Some((errors.is_empty(), msg, Instant::now()));
        }
        cx.update(|cx| sh.views.notify(cx));
    })
    .detach();
}

/// Opens (or reloads) the logs view for the container `id`, looked up in
/// the current snapshot when clicked (rows don't clone their container
/// into a click handler every frame).
fn open_logs_id(ui: &UiRc, sh: &Rc<Shared>, id: &str, cx: &mut App) {
    let snap = sh.snapshot();
    if let Some(c) = snap.containers.iter().find(|c| c.id == id) {
        open_logs(ui, sh, c, cx);
    }
}

/// Opens (or reloads) the logs view for a container.
fn open_logs(ui: &UiRc, sh: &Rc<Shared>, c: &Container, cx: &mut App) {
    let tail = log_lines(cx);
    {
        let mut u = ui.borrow_mut();
        match &mut u.logs {
            Some(l) if l.id == c.id => l.loading = true,
            _ => u.logs = Some(LogsView { id: c.id.clone(), name: c.name.clone(), image: c.image.clone(), lines: None, loading: true, scroll: ScrollHandle::new() }),
        }
    }
    sh.views.notify(cx);
    let rx = sh.monitor.logs(&c.id, tail);
    let (ui, sh, id) = (ui.clone(), sh.clone(), c.id.clone());
    cx.spawn(async move |cx| {
        let result = rx.recv().await.unwrap_or_else(|_| Err(t("The service did not reply").into()));
        // Converted once into shared rows; renders never copy log text.
        let rows = result.map(|lines| lines.into_iter().map(LogRow::new).collect::<Rc<[LogRow]>>());
        if let Some(l) = ui.borrow_mut().logs.as_mut().filter(|l| l.id == id) {
            l.lines = Some(rows);
            l.loading = false;
            l.scroll.scroll_to_bottom();
        }
        cx.update(|cx| sh.views.notify(cx));
    })
    .detach();
}

// ── Formatting ───────────────────────────────────────────────────────────

/// How long ago / for how long: the shared duration formatter.
fn span(secs: i64) -> String {
    super::panel_fx::duration_short(secs)
}

/// `docker.io/library/postgres:18@sha256:…` → `postgres:18`.
fn short_image(image: &str) -> String {
    let no_digest = image.split('@').next().unwrap_or(image);
    if no_digest.starts_with("sha256:") {
        return no_digest.chars().take(DIGEST_PREFIX_CHARS).collect();
    }
    let mut parts: Vec<&str> = no_digest.split('/').collect();
    // A first segment with a dot, a port or `localhost` is a registry host.
    if parts.len() > 1 && (parts[0].contains('.') || parts[0].contains(':') || parts[0] == "localhost") {
        parts.remove(0);
    }
    if parts.first() == Some(&"library") && parts.len() > 1 {
        parts.remove(0);
    }
    parts.join("/")
}

/// The letter on a container's avatar (image name, else container name).
fn monogram(c: &Container) -> String {
    let img = short_image(&c.image);
    let base = img.rsplit('/').next().unwrap_or(&img);
    base.chars().find(|ch| ch.is_alphanumeric()).or_else(|| c.name.chars().find(|ch| ch.is_alphanumeric())).map(|ch| ch.to_uppercase().to_string()).unwrap_or_default()
}

/// A stable hue per image so the same image always gets the same tile.
fn image_hue(image: &str) -> f32 {
    let img = short_image(image);
    let base = img.split(':').next().unwrap_or(&img);
    // FNV-1a: tiny, stable across runs.
    let mut h: u32 = FNV_OFFSET;
    for b in base.bytes() {
        h ^= u32::from(b);
        h = h.wrapping_mul(FNV_PRIME);
    }
    (h % HUE_DEGREES) as f32
}

fn port_label(p: &api::Port) -> String {
    match p.public {
        Some(public) => format!("{public}→{}", p.private),
        None => format!("{}/{}", p.private, p.proto),
    }
}

/// Uptime / exit / created line for a container.
fn when_line(c: &Container, snap: &Snapshot, now: i64) -> String {
    let insp = snap.inspect.get(&c.id);
    match c.state {
        State::Running | State::Restarting => match insp.and_then(|i| i.started_at) {
            Some(t0) => tf("up {span}", &[("span", &span(now - t0))]),
            None => t("running").to_string(),
        },
        State::Paused => t("paused").to_string(),
        State::Exited | State::Dead => {
            let code = c.exit_code.or(insp.and_then(|i| i.exit_code));
            match (insp.and_then(|i| i.finished_at), code) {
                (Some(t1), Some(code)) => tf("exited {span} ago · code {code}", &[("span", &span(now - t1)), ("code", &code)]),
                (None, Some(code)) => tf("exited · code {code}", &[("code", &code)]),
                (Some(t1), None) => tf("exited {span} ago", &[("span", &span(now - t1))]),
                (None, None) => t("exited").to_string(),
            }
        }
        State::Created => tf("created {span} ago", &[("span", &span(now - c.created))]),
        State::Removing => t("removing").to_string(),
        State::Unknown => c.status.clone(),
    }
}

// ── Pieces ───────────────────────────────────────────────────────────────

/// Legible foreground on a solid `bg`.
fn ink_on(bg: Color) -> Color {
    if bg.lightness() > INK_FLIP_LIGHTNESS { Color::oklch(DARK_INK.0, DARK_INK.1, bg.hue()) } else { Color::oklch(LIGHT_INK_L, 0.0, 0.0) }
}

fn state_color(state: State, exit_code: Option<i64>, palette: &MandaraPalette) -> Color {
    match state {
        State::Running => palette.success_emerald,
        State::Paused | State::Restarting => palette.warning_amber,
        State::Exited if exit_code == Some(0) => palette.on_surface_variant,
        State::Exited | State::Dead => palette.urgent_coral,
        State::Created | State::Removing | State::Unknown => palette.secondary,
    }
}

fn state_label(state: State) -> &'static str {
    match state {
        State::Running => tk("Running"),
        State::Paused => tk("Paused"),
        State::Restarting => tk("Restarting"),
        State::Created => tk("Created"),
        State::Exited => tk("Exited"),
        State::Removing => tk("Removing"),
        State::Dead => tk("Dead"),
        State::Unknown => tk("Unknown"),
    }
}

fn badge(label: impl Into<SharedString>, tint: Color, palette: &MandaraPalette) -> AnyElement {
    div()
        .flex_none()
        .h(px(BADGE_H_PX))
        .px(px(BADGE_PAD_X_PX))
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(BADGE_GAP_PX))
        .bg(tint.with_alpha(alpha(BADGE_TINT, palette)).hsla())
        .font_features(tabular_nums())
        .text_size(px(BADGE_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tint.hsla())
        .child(label.into())
        .into_any_element()
}

/// Running / Paused / Exited pill with a leading dot.
fn state_pill(c: &Container, palette: &MandaraPalette) -> AnyElement {
    let tint = readable(state_color(c.state, c.exit_code, palette), palette);
    div()
        .flex_none()
        .h(px(BADGE_H_PX))
        .pl(px(STATE_PILL_PAD_L_PX))
        .pr(px(STATE_PILL_PAD_R_PX))
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(BADGE_GAP_PX))
        .bg(tint.with_alpha(alpha(BADGE_TINT, palette)).hsla())
        .text_size(px(BADGE_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tint.hsla())
        .child(div().size(px(DOT_PX)).rounded_full().bg(tint.hsla()).when(c.state == State::Running, |el| el.shadow(vec![glow(tint.with_alpha(DOT_GLOW.0), DOT_GLOW.1, 0.0, 0.0)])))
        .child(crate::bidi::paragraph(tr(state_label(c.state))))
        .into_any_element()
}

fn health_badge(h: Health, palette: &MandaraPalette) -> AnyElement {
    let (label, tint) = match h {
        Health::Healthy => (t("healthy"), palette.success_emerald),
        Health::Unhealthy => (t("unhealthy"), palette.urgent_coral),
        Health::Starting => (t("starting"), palette.warning_amber),
    };
    badge(label, readable(tint, palette), palette)
}

fn port_chip(label: String, palette: &MandaraPalette) -> AnyElement {
    div()
        .flex_none()
        .h(px(BADGE_H_PX))
        .px(px(BADGE_PAD_X_PX))
        .rounded(px(crate::style::radius(PORT_RADIUS_PX)))
        .flex()
        .items_center()
        .bg(palette.on_surface.with_alpha(alpha(PORT_FILL, palette)).hsla())
        .font_features(tabular_nums())
        .text_size(px(BADGE_TEXT_PX))
        .text_color(palette.on_surface_variant.hsla())
        .child(label)
        .into_any_element()
}

/// Monogram tile tinted per image, with a state dot.
fn avatar(c: &Container, side: f32, palette: &MandaraPalette) -> AnyElement {
    let ring = crate::shell::frame_color(palette);
    let hue = image_hue(&c.image);
    let (l, ch) = if palette.is_dark { AVATAR_TINT_DARK } else { AVATAR_TINT_LIGHT };
    let tint = Color::oklch(l, ch, hue);
    let dot = state_color(c.state, c.exit_code, palette);
    div()
        .relative()
        .size(px(side))
        .flex_none()
        .rounded(px(crate::style::radius(side * AVATAR_RADIUS_FRAC)))
        .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, tint.with_alpha(AVATAR_FILL.0), partner(tint).with_alpha(AVATAR_FILL.1)))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(side * MONOGRAM_FRAC))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tint.hsla())
        .child(monogram(c))
        .child(state_dot(ring, dot))
        .into_any_element()
}

/// The state dot riding an avatar's corner.
fn state_dot(ring: Color, dot: Color) -> gpui::Div {
    div()
        .absolute()
        .right(px(STATE_DOT_OFFSET_PX))
        .bottom(px(STATE_DOT_OFFSET_PX))
        .size(px(STATE_DOT_PX))
        .rounded_full()
        .border_2()
        .border_color(ring.hsla())
        .bg(dot.hsla())
}

/// Stack avatar for a compose project.
fn project_avatar(running: usize, side: f32, palette: &MandaraPalette) -> AnyElement {
    let ring = crate::shell::frame_color(palette);
    let accent = readable(palette.primary, palette);
    let dot = if running > 0 { palette.success_emerald } else { palette.on_surface_variant };
    div()
        .relative()
        .size(px(side))
        .flex_none()
        .rounded(px(crate::style::radius(side * AVATAR_RADIUS_FRAC)))
        .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, accent.with_alpha(AVATAR_FILL.0), partner(accent).with_alpha(AVATAR_FILL.1)))
        .flex()
        .items_center()
        .justify_center()
        .child(icon("box", side * PROJECT_GLYPH_FRAC, accent))
        .child(state_dot(ring, dot))
        .into_any_element()
}

fn cpu_cell(cpu: Option<f32>, scale: CpuScale, palette: &MandaraPalette) -> AnyElement {
    let (max_cpu, hot) = (scale.max, cpu.is_some_and(|c| c >= scale.hot));
    let (from, to) = if hot {
        (palette.warning_amber, palette.urgent_coral)
    } else {
        let a = readable(palette.primary, palette);
        (a.with_alpha(COOL_FROM_ALPHA), readable(partner(palette.primary), palette))
    };
    let frac = cpu.map(|c| (c / max_cpu.max(1.0)).clamp(0.0, 1.0)).unwrap_or(0.0);
    div()
        .w(px(CPU_W))
        .flex_none()
        .flex()
        .flex_col()
        .items_end()
        .gap(px(CELL_GAP_PX))
        .child(
            div()
                .font_features(tabular_nums())
                .text_size(px(text::BODY_S))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if hot { readable(palette.urgent_coral, palette).hsla() } else { palette.on_surface.hsla() })
                .child(cpu.map(|c| format!("{c:.1}%")).unwrap_or_else(|| "–".into())),
        )
        .child(div().w(px(CPU_METER_W_PX)).child(meter(frac, CPU_METER_H_PX, from, to, palette)))
        .into_any_element()
}

fn mem_cell(mem: Option<u64>, palette: &MandaraPalette) -> AnyElement {
    div()
        .w(px(MEM_W))
        .flex_none()
        .flex()
        .justify_end()
        .font_features(tabular_nums())
        .text_size(px(text::BODY_S))
        .text_color(palette.on_surface_variant.hsla())
        .child(mem.map(format_bytes).unwrap_or_else(|| "–".into()))
        .into_any_element()
}

fn row_shell(id: ElementId, height: f32, radius: f32, highlighted: bool, armed: bool, palette: &MandaraPalette) -> gpui::Stateful<gpui::Div> {
    let hover = palette.on_surface.with_alpha(alpha(ROW_HOVER, palette));
    let sel = readable(palette.primary, palette).with_alpha(alpha(ROW_SELECTED, palette));
    let warn = palette.urgent_coral.with_alpha(alpha(ROW_ARMED, palette));
    div()
        .id(id)
        .group("ctr-row")
        .h(px(height))
        .flex_none()
        .px(px(ROW_PAD_X_PX))
        .rounded(px(radius))
        .flex()
        .items_center()
        .gap(px(ROW_INNER_GAP_PX))
        .cursor_pointer()
        .when(armed, |el| el.bg(warn.hsla()))
        .when(highlighted && !armed, |el| el.bg(sel.hsla()))
        .when(!highlighted && !armed, |el| el.hover(move |s| s.bg(hover.hsla())))
}

/// A small round lifecycle button, visible on row hover (or `pinned`).
/// `id` is unique within its row (the row's own id namespaces it).
fn mini_button(id: &'static str, icon_name: &'static str, tint: Color, pinned: bool, palette: &MandaraPalette, on_click: impl Fn(&mut Window, &mut App) + 'static) -> AnyElement {
    let bg = tint.with_alpha(alpha(MINI_FILL, palette));
    let hover = tint.with_alpha(alpha(MINI_HOVER, palette));
    div()
        .id(id)
        .size(px(BUTTON_PX))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .bg(bg.hsla())
        .hover(move |s| s.bg(hover.hsla()))
        .when(!pinned, |el| el.invisible().group_hover("ctr-row", |s| s.visible()))
        .child(icon(icon_name, MINI_ICON_PX, tint))
        .on_click(move |_, window, cx| {
            cx.stop_propagation();
            on_click(window, cx)
        })
        .into_any_element()
}

/// The Stop → Confirm button (arms first, like the process hunter's End).
fn stop_button(key: Key, targets: Targets, ui: &UiRc, sh: &Rc<Shared>, palette: &MandaraPalette, label_all: bool) -> AnyElement {
    let armed = ui.borrow().armed(&key);
    let coral = palette.urgent_coral;
    let soft = readable(coral, palette);
    let ink = ink_on(coral);
    let n = targets.len();
    let label = match (armed, label_all) {
        (true, false) => t("Confirm").to_string(),
        (true, true) => tf("Confirm ×{count}", &[("count", &n)]),
        (false, false) => String::new(),
        (false, true) => t("Stop all").to_string(),
    };
    let compact = label.is_empty();
    let (ui, sh) = (ui.clone(), sh.clone());
    let (stop_hover, stop_fill) = (coral.with_alpha(alpha(MINI_HOVER, palette)), coral.with_alpha(alpha(STOP_FILL, palette)));
    div()
        // Unique within its row (the row's id namespaces it).
        .id("stop")
        .h(px(BUTTON_PX))
        .when(compact, |el| el.w(px(BUTTON_PX)).justify_center())
        .when(!compact, |el| el.px(px(BUTTON_PAD_X_PX)).gap(px(BUTTON_GAP_PX)))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .cursor_pointer()
        .text_size(px(BUTTON_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .when(armed, |el| el.bg(coral.hsla()).text_color(ink.hsla()).shadow(vec![glow(coral.with_alpha(ARMED_GLOW.0), ARMED_GLOW.1, 0.0, ARMED_GLOW.2)]))
        .when(!armed, |el| el.bg(stop_fill.hsla()).text_color(soft.hsla()).hover(move |s| s.bg(stop_hover.hsla())).invisible().group_hover("ctr-row", |s| s.visible()))
        .child(icon(if armed { "check" } else { "stop" }, SMALL_ICON_PX, if armed { ink } else { soft }))
        .when(!compact, |el| el.child(label))
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            let armed = ui.borrow().armed(&key);
            if armed {
                run(&ui, &sh, targets.clone(), Action::Stop, cx);
            } else {
                ui.borrow_mut().armed.insert(key.clone(), Instant::now());
                sh.views.notify(cx);
            }
        })
        .into_any_element()
}

fn busy_pill(action: Action, palette: &MandaraPalette) -> AnyElement {
    div()
        .h(px(BUTTON_PX))
        .px(px(BUSY_PAD_X_PX))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(BUTTON_GAP_PX))
        .bg(palette.on_surface.with_alpha(BUSY_FILL_ALPHA).hsla())
        .text_size(px(BUTTON_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(palette.on_surface_variant.hsla())
        .child(icon("timer", SMALL_ICON_PX, palette.on_surface_variant))
        .child(busy_label(action))
        .into_any_element()
}

/// Lifecycle buttons for one container.
fn actions(c: &Container, ui: &UiRc, sh: &Rc<Shared>, palette: &MandaraPalette) -> AnyElement {
    let (busy, armed) = {
        let u = ui.borrow();
        (u.busy.get(&c.id).copied(), u.armed(&Key::Container(c.id.clone())))
    };
    let mut row = div().w(px(ACTIONS_W)).flex_none().flex().items_center().justify_end().gap(px(ACTIONS_GAP_PX));
    if let Some(a) = busy {
        return row.child(busy_pill(a, palette)).into_any_element();
    }
    let target: Targets = Rc::new([(c.id.clone(), c.name.clone())]);
    let accent = readable(palette.primary, palette);
    let mk = |action: Action, icon_name: &'static str, tint: Color| {
        let (ui, sh, target) = (ui.clone(), sh.clone(), target.clone());
        mini_button(action.verb(), icon_name, tint, armed, palette, move |_, cx| run(&ui, &sh, target.clone(), action, cx))
    };
    if !armed {
        if Action::Start.applies_to(c.state) {
            row = row.child(mk(Action::Start, "play", readable(palette.success_emerald, palette)));
        }
        if Action::Unpause.applies_to(c.state) {
            row = row.child(mk(Action::Unpause, "play", readable(palette.success_emerald, palette)));
        }
        if Action::Pause.applies_to(c.state) {
            row = row.child(mk(Action::Pause, "pause", readable(palette.warning_amber, palette)));
        }
        if Action::Restart.applies_to(c.state) {
            row = row.child(mk(Action::Restart, "restart", accent));
        }
    }
    if Action::Stop.applies_to(c.state) {
        row = row.child(stop_button(Key::Container(c.id.clone()), target, ui, sh, palette, false));
    }
    row.into_any_element()
}

/// Name line: name, state pill, health and (ungrouped) project badges.
fn title_line(c: &Container, show_project: bool, palette: &MandaraPalette) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap(px(INLINE_GAP_PX))
        .min_w(px(0.0))
        // The name keeps a readable minimum; the project badge gives way first.
        .child(div().min_w(px(NAME_MIN_W)).text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).truncate().child(c.name.clone()))
        .child(state_pill(c, palette))
        .when_some(c.health.filter(|_| c.state == State::Running), |el, h| el.child(health_badge(h, palette)))
        .when_some(c.project.clone().filter(|_| show_project), |el, p| {
            let tint = readable(palette.tertiary, palette);
            el.child(
                div()
                    .min_w(px(0.0))
                    .flex_shrink(PROJECT_BADGE_SHRINK)
                    .h(px(BADGE_H_PX))
                    .px(px(BADGE_PAD_X_PX))
                    .rounded_full()
                    .bg(tint.with_alpha(alpha(BADGE_TINT, palette)).hsla())
                    .text_size(px(BADGE_TEXT_PX))
                    .line_height(px(BADGE_H_PX))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(tint.hsla())
                    .truncate()
                    .child(p),
            )
        })
        .into_any_element()
}

fn sub_line(c: &Container, snap: &Snapshot, now: i64, palette: &MandaraPalette) -> AnyElement {
    let ports = c.unique_ports();
    let extra = ports.len().saturating_sub(PORT_CHIPS);
    let muted = palette.on_surface_variant.with_alpha(MUTED_TEXT_ALPHA);
    div()
        .flex()
        .items_center()
        .gap(px(SUB_GAP_PX))
        .min_w(px(0.0))
        .child(div().min_w(px(0.0)).flex_shrink(1.0).text_size(px(text::LABEL)).text_color(muted.hsla()).truncate().child(short_image(&c.image)))
        .child(div().flex_none().text_size(px(text::LABEL)).text_color(muted.hsla()).child("·"))
        .child(div().flex_none().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(muted.hsla()).whitespace_nowrap().child(when_line(c, snap, now)))
        .children(ports.iter().take(PORT_CHIPS).map(|p| port_chip(port_label(p), palette)))
        .when(extra > 0, |el| el.child(port_chip(format!("+{extra}"), palette)))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn container_row(c: &Container, snap: &Snapshot, ui: &UiRc, sh: &Rc<Shared>, palette: &MandaraPalette, scale: CpuScale, radius: f32, child: bool, show_project: bool, now: i64) -> AnyElement {
    let armed = ui.borrow().armed(&Key::Container(c.id.clone()));
    let live = snap.live.get(&c.id);
    let lead = if child {
        div()
            .w(px(AVATAR))
            .h_full()
            .flex_none()
            .flex()
            .justify_center()
            .child(div().w(px(2.0)).h_full().rounded_full().bg(palette.on_surface.with_alpha(GUIDE_ALPHA).hsla()))
            .into_any_element()
    } else {
        avatar(c, AVATAR, palette)
    };
    let (ui2, sh2, id2) = (ui.clone(), sh.clone(), c.id.clone());
    let up = c.state == State::Running;
    row_shell(ElementId::NamedInteger("ctr".into(), id_hash(&c.id)), if child { CHILD_H } else { ROW_H }, radius, false, armed, palette)
        .child(lead)
        .child(div().flex_1().min_w(px(0.0)).flex().flex_col().gap(px(ROW_LINES_GAP_PX)).child(title_line(c, show_project, palette)).child(sub_line(c, snap, now, palette)))
        .child(if up { cpu_cell(live.and_then(|l| l.cpu), scale, palette) } else { cpu_cell(None, scale, palette) })
        .child(mem_cell(live.filter(|_| c.state.is_up()).map(|l| l.mem_used), palette))
        .child(actions(c, ui, sh, palette))
        .on_click(move |_, _, cx| open_logs_id(&ui2, &sh2, &id2, cx))
        .into_any_element()
}

#[allow(clippy::too_many_arguments)]
fn project_rows(g: &Group, snap: &Snapshot, ui: &UiRc, sh: &Rc<Shared>, palette: &MandaraPalette, scale: CpuScale, radius: f32, now: i64) -> Vec<AnyElement> {
    let name = g.project.clone().unwrap_or_default();
    let key = Key::Project(name.clone());
    let (expanded, armed) = {
        let u = ui.borrow();
        (u.expanded.contains(&name), u.armed(&key))
    };
    let running = g.running();
    let (cpu, mem) = g.members.iter().filter_map(|c| snap.live.get(&c.id)).fold((None::<f32>, 0u64), |(cpu, mem), l| (l.cpu.map(|v| cpu.unwrap_or(0.0) + v).or(cpu), mem + l.mem_used));
    let detail = tf("{running} of {count} running", &[("running", &running), ("count", &g.members.len())]);
    let chevron = div().size(px(CHEVRON_BOX_PX)).flex_none().flex().items_center().justify_center().child(
        gpui::svg()
            .path("icons/chevron-right.svg")
            .size(px(SMALL_ICON_PX))
            .flex_none()
            .text_color(palette.on_surface_variant.hsla())
            .with_transformation(gpui::Transformation::rotate(gpui::radians(if expanded { std::f32::consts::FRAC_PI_2 } else { 0.0 }))),
    );
    let busy = g.members.iter().find_map(|c| ui.borrow().busy.get(&c.id).copied());
    let group_action: AnyElement = if let Some(a) = busy {
        busy_pill(a, palette)
    } else if running > 0 {
        let targets: Targets = g.members.iter().filter(|c| Action::Stop.applies_to(c.state)).map(|c| (c.id.clone(), c.name.clone())).collect();
        stop_button(key.clone(), targets, ui, sh, palette, true)
    } else {
        let targets: Targets = g.members.iter().filter(|c| Action::Start.applies_to(c.state)).map(|c| (c.id.clone(), c.name.clone())).collect();
        let (ui, sh) = (ui.clone(), sh.clone());
        let tint = readable(palette.success_emerald, palette);
        let hover = tint.with_alpha(alpha(MINI_HOVER, palette));
        div()
            // Unique within its project row.
            .id("start-all")
            .h(px(BUTTON_PX))
            .px(px(BUTTON_PAD_X_PX))
            .flex_none()
            .rounded_full()
            .flex()
            .items_center()
            .gap(px(BUTTON_GAP_PX))
            .cursor_pointer()
            .bg(tint.with_alpha(alpha(MINI_FILL, palette)).hsla())
            .hover(move |s| s.bg(hover.hsla()))
            .invisible()
            .group_hover("ctr-row", |s| s.visible())
            .text_size(px(BUTTON_TEXT_PX))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(tint.hsla())
            .child(icon("play", SMALL_ICON_PX, tint))
            .child(t("Start all"))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                run(&ui, &sh, targets.clone(), Action::Start, cx);
            })
            .into_any_element()
    };
    let (ui2, sh2, name2) = (ui.clone(), sh.clone(), name.clone());
    let head = row_shell(ElementId::NamedInteger("prj".into(), id_hash(&name)), ROW_H, radius, expanded, armed, palette)
        .child(project_avatar(running, AVATAR, palette))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(ROW_LINES_GAP_PX))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(INLINE_GAP_PX))
                        .min_w(px(0.0))
                        .child(div().min_w(px(0.0)).text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).truncate().child(name.clone()))
                        .child(badge(format!("×{}", g.members.len()), readable(palette.primary, palette), palette))
                        .child(chevron),
                )
                .child(div().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.with_alpha(MUTED_TEXT_ALPHA).hsla()).truncate().child(tf("Compose project · {detail}", &[("detail", &detail)]))),
        )
        .child(cpu_cell(cpu, scale, palette))
        .child(mem_cell((mem > 0).then_some(mem), palette))
        .child(div().w(px(ACTIONS_W)).flex_none().flex().justify_end().child(group_action))
        .on_click(move |_, _, cx| {
            {
                let mut u = ui2.borrow_mut();
                if !u.expanded.remove(&name2) {
                    u.expanded.insert(name2.clone());
                }
            }
            sh2.views.notify(cx);
        })
        .into_any_element();
    let mut out = vec![head];
    if expanded {
        out.extend(g.members.iter().map(|c| container_row(c, snap, ui, sh, palette, scale, radius, true, false, now)));
    }
    out
}

#[allow(clippy::too_many_arguments)]
fn tile(icon_name: &'static str, label: &'static str, value: String, detail: String, history: Option<&History>, (from, to): (Color, Color), radius: f32, palette: &MandaraPalette) -> AnyElement {
    glass_card(palette)
        .rounded(px(radius))
        .flex_1()
        .min_w(px(0.0))
        .h(px(TILE_H))
        .px(px(TILE_PAD_X_PX))
        .py(px(TILE_PAD_Y_PX))
        .flex()
        .gap(px(ROW_INNER_GAP_PX))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .justify_between()
                .child(div().flex().items_center().gap(px(INLINE_GAP_PX)).child(icon(icon_name, TILE_ICON_PX, from)).child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).child(crate::bidi::paragraph(tr(label)))))
                .child(div().font_features(tabular_nums()).text_size(px(TILE_VALUE_PX)).line_height(px(TILE_VALUE_LINE_PX)).font_weight(FontWeight::MEDIUM).whitespace_nowrap().child(value))
                .child(div().font_features(tabular_nums()).text_size(px(TILE_CAPTION_PX)).text_color(palette.on_surface_variant.hsla()).whitespace_nowrap().truncate().child(detail)),
        )
        .when_some(history, |el, h| el.child(div().w(px(SPARK_W_PX)).flex_none().flex().items_end().child(sparkline(h, SPARK_H_PX, from.with_alpha(SPARK_FROM_ALPHA), to, palette))))
        .into_any_element()
}

fn segmented<T: Copy + PartialEq + 'static>(id: &'static str, items: &[(T, &'static str, &'static str)], current: T, palette: &MandaraPalette, on_pick: impl Fn(T, &mut Window, &mut App) + 'static) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let on_pick = Rc::new(on_pick);
    let segs: Vec<AnyElement> = items
        .iter()
        .enumerate()
        .map(|(i, &(value, label, icon_name))| {
            let active = current == value;
            let hover = palette.on_surface.with_alpha(SEG_HOVER_ALPHA);
            let on_pick = on_pick.clone();
            div()
                .id(ElementId::named_usize(id, i))
                .h(px(SEARCH_H - SEG_INSET_PX * 2.0))
                .px(px(SEG_PAD_X_PX))
                .rounded(px(crate::style::radius(SEG_RADIUS_PX)))
                .flex()
                .items_center()
                .gap(px(SEG_GAP_PX))
                .cursor_pointer()
                .text_size(px(BUTTON_TEXT_PX))
                .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                .when(active, |el| el.bg(accent.with_alpha(alpha(SEG_ACTIVE, palette)).hsla()).text_color(accent.hsla()))
                .when(!active, |el| el.text_color(palette.on_surface_variant.hsla()).hover(move |s| s.bg(hover.hsla())))
                .when(!icon_name.is_empty(), |el| el.child(icon(icon_name, SMALL_ICON_PX, if active { accent } else { palette.on_surface_variant })))
                .child(crate::bidi::paragraph(tr(label)))
                .on_click(move |_, window, cx| on_pick(value, window, cx))
                .into_any_element()
        })
        .collect();
    div()
        .h(px(SEARCH_H))
        .flex_none()
        .p(px(SEG_INSET_PX))
        .rounded(px(crate::style::radius(SEGMENTS_RADIUS_PX)))
        .bg(palette.on_surface.with_alpha(alpha(SEGMENTS_FILL, palette)).hsla())
        .flex()
        .items_center()
        .gap(px(SEGMENTS_GAP_PX))
        .children(segs)
        .into_any_element()
}

fn search_box(field: &Entity<TextField>, query: &str, palette: &MandaraPalette) -> AnyElement {
    let f = field.clone();
    div()
        .flex_1()
        .min_w(px(0.0))
        .h(px(SEARCH_H))
        .pl(px(SEARCH_PAD_L_PX))
        .pr(px(SEARCH_PAD_R_PX))
        .rounded(px(crate::style::radius(SEARCH_RADIUS_PX)))
        .bg(palette.surface_container_high.hsla())
        .flex()
        .items_center()
        .gap(px(SEGMENTS_GAP_PX))
        .child(icon("search", SEARCH_ICON_PX, palette.on_surface_variant))
        .child(div().flex_1().min_w(px(0.0)).child(field.clone()))
        .when(!query.is_empty(), |el| {
            el.child(icon_button("ctr-clear", "close", CLEAR_BUTTON_PX, palette.on_surface_variant, None, move |window, cx| {
                f.update(cx, |f, _| f.set(""));
                window.refresh();
            }))
        })
        .into_any_element()
}

fn toast(ok: bool, msg: String, palette: &MandaraPalette) -> AnyElement {
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
                .h(px(TOAST_H_PX))
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
                        .size(px(TOAST_ICON_DISC_PX))
                        .flex_none()
                        .rounded_full()
                        .bg(tint.with_alpha(TOAST_ICON_FILL_ALPHA).hsla())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(if ok { "check" } else { "close" }, SMALL_ICON_PX, tint)),
                )
                .child(div().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).child(msg)),
        )
        .into_any_element()
}

/// The engine unreachable: what happened and what to do.
fn error_card(err: &api::ApiError, radius: f32, palette: &MandaraPalette) -> AnyElement {
    let coral = readable(palette.urgent_coral, palette);
    let hint = err.hint();
    let (title, icon_name) = match err {
        api::ApiError::NoEngine => (t("No container engine").to_string(), "box"),
        api::ApiError::PermissionDenied { .. } => (t("No access to the engine").to_string(), "lock"),
        _ => (t("Engine unreachable").to_string(), "alert"),
    };
    let rings = div().relative().size(px(ERROR_RINGS_PX)).flex().items_center().justify_center().children(ERROR_RING_D_PX.into_iter().zip(ERROR_RING_ALPHA).map(move |(d, a)| {
        let inset = (ERROR_RINGS_PX - d) / 2.0;
        div().absolute().left(px(inset)).top(px(inset)).size(px(d)).rounded_full().bg(coral.with_alpha(a).hsla())
    }));
    glass_card(palette)
        .rounded(px(radius))
        .h(px(ERROR_H))
        .px(px(ERROR_PAD_X_PX))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(tokens::XS))
        .child(rings.child(icon(icon_name, ERROR_ICON_PX, coral)))
        .child(div().mt(px(ERROR_TITLE_GAP_PX)).text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(title))
        .child(div().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).text_center().child(err.message()))
        .when(!hint.is_empty(), |el| {
            el.child(
                div()
                    .mt(px(HINT_GAP_PX))
                    .px(px(HINT_PAD_X_PX))
                    .py(px(HINT_PAD_Y_PX))
                    .rounded(px(crate::style::radius(HINT_RADIUS_PX)))
                    .bg(palette.on_surface.with_alpha(alpha(HINT_FILL, palette)).hsla())
                    .flex()
                    .items_center()
                    .gap(px(tokens::SM))
                    .child(icon("terminal", HINT_ICON_PX, palette.on_surface_variant))
                    .child(div().text_size(px(BUTTON_TEXT_PX)).text_color(palette.on_surface_variant.hsla()).child(hint)),
            )
        })
        .into_any_element()
}

/// The logs sheet: toolbar plus the last lines in the monospace font.
fn logs_sheet(ui: &UiRc, sh: &Rc<Shared>, snap: &Snapshot, radius: f32, palette: &MandaraPalette, cx: &mut App) -> AnyElement {
    let u = ui.borrow();
    let Some(l) = u.logs.as_ref() else { return div().into_any_element() };
    let mono = crate::fonts::fonts(cx).mono;
    let container = snap.containers.iter().find(|c| c.id == l.id);
    let accent = readable(palette.primary, palette);
    let back = {
        let (ui, sh) = (ui.clone(), sh.clone());
        icon_button("ctr-logs-back", "arrow", LOG_BUTTON_PX, palette.on_surface_variant, Some(palette.on_surface.with_alpha(LOG_BUTTON_FILL_ALPHA)), move |_, cx| {
            // Closing the sheet frees its lines.
            ui.borrow_mut().logs = None;
            sh.views.notify(cx);
        })
    };
    let reload = container.map(|c| {
        let (ui, sh, id) = (ui.clone(), sh.clone(), c.id.clone());
        icon_button("ctr-logs-reload", "refresh", LOG_BUTTON_PX, accent, Some(accent.with_alpha(LOG_RELOAD_FILL_ALPHA)), move |_, cx| open_logs_id(&ui, &sh, &id, cx))
    });
    let (count, errors) = match &l.lines {
        Some(Ok(lines)) => (lines.len(), lines.iter().filter(|x| x.stderr).count()),
        _ => (0, 0),
    };
    let detail = match &l.lines {
        Some(Ok(_)) if errors > 0 => tf("{count} lines · {errors} on stderr", &[("count", &count), ("errors", &errors)]),
        Some(Ok(_)) => tf("Last {count} lines", &[("count", &count)]),
        _ if l.loading => t("Loading logs…").to_string(),
        _ => short_image(&l.image),
    };
    let toolbar = div()
        .h(px(LOG_TOOLBAR_H))
        .flex()
        .items_center()
        .gap(px(LOG_GAP_PX))
        .child(back)
        .when_some(container, |el, c| el.child(avatar(c, LOG_AVATAR_PX, palette)))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().flex().items_center().gap(px(INLINE_GAP_PX)).child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).truncate().child(l.name.clone())).when_some(container, |el, c| el.child(state_pill(c, palette))))
                .child(div().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(detail)),
        )
        .when_some(reload, |el, r| el.child(r));
    let body_h = LOGS_H - LOG_TOOLBAR_H - LOG_GAP_PX;
    let well = palette.on_surface.with_alpha(alpha(LOG_WELL, palette));
    let body: AnyElement = match &l.lines {
        None => empty_state("note", t("Loading logs…"), l.name.clone(), body_h, palette).into_any_element(),
        Some(Err(e)) => empty_state("alert", t("Could not read the logs"), e.clone(), body_h, palette).into_any_element(),
        Some(Ok(lines)) if lines.is_empty() => empty_state("note", t("No log output yet"), t("This container hasn't written anything to stdout or stderr"), body_h, palette).into_any_element(),
        Some(Ok(lines)) => {
            let coral = readable(palette.urgent_coral, palette);
            let plain = palette.on_surface.with_alpha(LOG_TEXT_ALPHA);
            // Rows are plain divs: they need no ids (no per-row state).
            let rows = lines.iter().map(|line| {
                let err = line.stderr;
                div()
                    .flex()
                    .gap(px(LOG_LINE_GAP_PX))
                    .min_h(px(LOG_LINE_H))
                    .child(div().w(px(2.0)).flex_none().rounded_full().when(err, |el| el.bg(coral.with_alpha(STDERR_RULE_ALPHA).hsla())))
                    .child(div().flex_1().min_w(px(0.0)).text_color(if err { coral.hsla() } else { plain.hsla() }).child(line.text.clone()))
            });
            div()
                .id("ctr-logs")
                .track_scroll(&l.scroll)
                .h(px(body_h))
                .rounded(px(crate::style::concentric(radius, NESTED_RADIUS_INSET_PX).max(crate::style::radius(MIN_NESTED_RADIUS_PX))))
                .bg(well.hsla())
                .p(px(LOG_PAD_PX))
                .overflow_y_scroll()
                .font_family(mono)
                .text_size(px(LOG_TEXT_PX))
                .line_height(px(LOG_LINE_H))
                .flex()
                .flex_col()
                .children(rows)
                .into_any_element()
        }
    };
    div().h(px(LOGS_H)).flex().flex_col().gap(px(LOG_GAP_PX)).child(toolbar).child(body).into_any_element()
}

// ── Panel ────────────────────────────────────────────────────────────────

/// How often an open panel checks (without re-rendering) whether the
/// host has closed it.
const CLOSE_POLL: Duration = Duration::from_secs(1);

/// Runs a panel's clean-up once the host has fully closed it: armed by
/// the first visible render, it polls the host-set progress (no renders)
/// and stops itself after `on_close`.
#[derive(Clone, Default)]
struct CloseWatch(Rc<Cell<bool>>);

impl CloseWatch {
    /// Starts watching `p` unless already watching; reports whether it did.
    fn arm(&self, p: &PanelCx, on_close: impl FnOnce(&mut App) + 'static, cx: &mut App) -> bool {
        if self.0.replace(true) {
            return false;
        }
        let (flag, p) = (self.0.clone(), p.clone());
        cx.spawn(async move |cx| {
            while p.progress() > 0.0 {
                cx.background_executor().timer(CLOSE_POLL).await;
            }
            flag.set(false);
            cx.update(on_close);
        })
        .detach();
        true
    }
}

struct View {
    p: PanelCx,
    sh: Rc<Shared>,
    ui: UiRc,
    field: Entity<TextField>,
    size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>,
    /// Filtered / grouped rows (see [`RowsCache`]).
    rows: Rc<RefCell<Option<RowsCache>>>,
    watch: CloseWatch,
}

impl View {
    /// Frees panel-only state when the panel closes: logs, sparkline
    /// histories, the row cache and (once every panel is closed) the live
    /// stats in the shared snapshot. The chip keeps its counts; the panel
    /// keeps its last height so reopening doesn't reflow.
    fn watch_close(&self, cx: &mut App) {
        let (ui, rows, sh) = (self.ui.clone(), self.rows.clone(), self.sh.clone());
        let armed = self.watch.arm(
            &self.p,
            move |_| {
                {
                    let mut u = ui.borrow_mut();
                    u.logs = None;
                    u.cpu = History::new(HISTORY);
                    u.mem = History::new(HISTORY);
                    u.armed.clear();
                    u.status = None;
                }
                *rows.borrow_mut() = None;
                sh.open.set(sh.open.get().saturating_sub(1));
                if sh.open.get() == 0 {
                    sh.absorb();
                }
            },
            cx,
        );
        if armed {
            self.sh.open.set(self.sh.open.get() + 1);
            // Pick up the full snapshot (live stats) for the open panel.
            self.sh.absorb();
        }
    }

    fn set_height(&self, height: f32) {
        if (f32::from(self.size_cell.get().height) - height).abs() > SIZE_EPSILON_PX {
            self.size_cell.set(size(px(PANEL_W), px(height)));
        }
    }

    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let (ui, sh) = (&self.ui, &self.sh);
        sh.views.track(window);
        sh.sync(cx);
        if self.p.progress() > 0.0 {
            self.watch_close(cx);
            sh.monitor.ping_detail();
            redraw_in(RENDER_EVERY, window, cx);
        }
        let snap = sh.snapshot();
        let shape = PanelShape::of(cx);
        let card_r = shape.inner(PAD);
        let row_r = (card_r - ROW_RADIUS_INSET_PX).max(MIN_ROW_RADIUS_PX);
        let now = chrono::Utc::now().timestamp();
        let accent = readable(palette.primary, palette);
        let stopped_too = show_stopped(cx);

        // Transient timers (arming, toast).
        let status = {
            let mut u = ui.borrow_mut();
            let toast_for = crate::state::config(cx).read(cx).snapshot().panels.toast();
            if u.status.as_ref().is_some_and(|(_, _, at)| at.elapsed() >= toast_for) {
                u.status = None;
            }
            u.armed.retain(|_, at| at.elapsed() < ARM);
            if !u.armed.is_empty() {
                redraw_in(ARM_TICK, window, cx);
            }
            if !stopped_too {
                u.filter = Filter::Running;
            }
            u.status.clone()
        };
        if let Some((_, _, at)) = &status {
            redraw_in(crate::state::config(cx).read(cx).snapshot().panels.toast().saturating_sub(at.elapsed()) + EXPIRY_MARGIN, window, cx);
        }

        // ── Header ───────────────────────────────────────────────────────
        let running = snap.running();
        let engine = snap.endpoint.as_ref().map(|e| e.engine);
        let subtitle = match (&snap.error, engine, snap.listed) {
            (Some(e), _, _) => e.message(),
            (None, Some(eng), Some(_)) => {
                let name = match &snap.version {
                    Some(v) if !v.is_empty() => format!("{} {v}", eng.label()),
                    _ => eng.label().to_string(),
                };
                tf("{engine} · {running} running of {total}", &[("engine", &name), ("running", &running), ("total", &snap.containers.len())])
            }
            _ => t("Connecting…").to_string(),
        };
        let mut trailing = div().flex().items_center().gap(px(INLINE_GAP_PX));
        if snap.endpoints.len() > 1
            && let Some(current) = engine
        {
            let items: Vec<(Engine, &'static str, &'static str)> = snap.endpoints.iter().map(|e| (e.engine, e.engine.label(), "")).collect();
            let sh2 = sh.clone();
            trailing = trailing.child(segmented("ctr-engine", &items, current, palette, move |e, _, cx| {
                sh2.monitor.use_engine(e);
                sh2.views.notify(cx);
            }));
        }
        if snap.error.is_none() && ui.borrow().logs.is_none() {
            let on = ui.borrow().grouped;
            let (ui2, sh2) = (ui.clone(), sh.clone());
            let hover = palette.on_surface.with_alpha(TOGGLE_HOVER_ALPHA);
            trailing = trailing.child(
                div()
                    .id("ctr-group")
                    .h(px(BUTTON_PX))
                    .px(px(BUSY_PAD_X_PX))
                    .flex_none()
                    .rounded_full()
                    .flex()
                    .items_center()
                    .gap(px(BUTTON_GAP_PX))
                    .cursor_pointer()
                    .text_size(px(BUTTON_TEXT_PX))
                    .font_weight(FontWeight::SEMIBOLD)
                    .when(on, |el| el.bg(accent.with_alpha(alpha(SEG_ACTIVE, palette)).hsla()).text_color(accent.hsla()))
                    .when(!on, |el| el.bg(palette.on_surface.with_alpha(BUSY_FILL_ALPHA).hsla()).text_color(palette.on_surface_variant.hsla()).hover(move |s| s.bg(hover.hsla())))
                    .child(icon("grid", SMALL_ICON_PX, if on { accent } else { palette.on_surface_variant }))
                    .child(t("Projects"))
                    .on_click(move |_, _, cx| {
                        {
                            let mut u = ui2.borrow_mut();
                            u.grouped = !u.grouped;
                        }
                        sh2.views.notify(cx);
                    }),
            );
        }
        let head = header("box", accent, t("Containers"), subtitle, palette, Some(trailing.into_any_element()));

        // ── Error state ──────────────────────────────────────────────────
        if let Some(err) = &snap.error {
            self.set_height(PAD * 2.0 + HEADER_H + GAP + ERROR_H);
            let p = &self.p;
            return div()
                .relative()
                .size_full()
                .p(px(PAD))
                .flex()
                .flex_col()
                .child(reveal(p, 0, head))
                .child(div().h(px(GAP)))
                .child(reveal(p, 1, error_card(err, card_r, palette)))
                .into_any_element();
        }

        // ── Tiles ────────────────────────────────────────────────────────
        let cores = cores() as f32;
        let cpu_sum: f32 = snap.live.values().filter_map(|l| l.cpu).sum();
        let mem_sum: u64 = snap.live.values().map(|l| l.mem_used).sum();
        let mem_total = host_memory();
        let have_cpu = snap.live.values().any(|l| l.cpu.is_some());
        {
            let mut u = ui.borrow_mut();
            let at = Instant::now();
            if have_cpu {
                u.cpu.push_every(cpu_sum / (cores * PERCENT), HISTORY_EVERY, at);
            }
            if mem_total > 0 && !snap.live.is_empty() {
                u.mem.push_every(mem_sum as f32 / mem_total as f32, HISTORY_EVERY, at);
            }
        }
        let pair = |c: Color| (readable(c, palette), readable(partner(c), palette));
        let stopped = snap.containers.len() - snap.containers.iter().filter(|c| c.state.is_up()).count();
        let counts = match (snap.images, snap.volumes) {
            (Some(i), Some(v)) => tf("{images} images · {volumes} volumes", &[("images", &i), ("volumes", &v)]),
            _ => tf("{stopped} stopped", &[("stopped", &stopped)]),
        };
        let u = ui.borrow();
        let strip = div()
            .flex()
            .gap(px(ROW_INNER_GAP_PX))
            .child(tile("play", tk("Running"), running.to_string(), counts, None, pair(palette.success_emerald), card_r, palette))
            .child(tile(
                "cpu",
                tk("CPU"),
                if have_cpu { format!("{cpu_sum:.0}%") } else { "–".into() },
                if have_cpu { tf("of {cores} cores", &[("cores", &cores)]) } else { t("sampling…").to_string() },
                Some(&u.cpu),
                pair(palette.primary),
                card_r,
                palette,
            ))
            .child(tile(
                "memory",
                tk("Memory"),
                if snap.live.is_empty() { "–".into() } else { format_bytes(mem_sum) },
                if mem_total > 0 { tf("of {total}", &[("total", &format_bytes(mem_total))]) } else { t("sampling…").to_string() },
                Some(&u.mem),
                pair(palette.tertiary),
                card_r,
                palette,
            ));
        let logs_open = u.logs.is_some();
        let (filter, grouped) = (u.filter, u.grouped);
        drop(u);

        let base_h = PAD * 2.0 + HEADER_H + GAP + TILE_H + GAP;
        let p = &self.p;
        if logs_open {
            self.set_height(base_h + LOGS_H);
            return div()
                .relative()
                .size_full()
                .p(px(PAD))
                .flex()
                .flex_col()
                .child(reveal(p, 0, head))
                .child(div().h(px(GAP)))
                .child(reveal(p, 1, strip))
                .child(div().h(px(GAP)))
                .child(reveal(p, 2, logs_sheet(ui, sh, &snap, card_r, palette, cx)))
                .when_some(status, |el, (ok, msg, _)| el.child(toast(ok, msg, palette)))
                .into_any_element();
        }

        // ── Rows ─────────────────────────────────────────────────────────
        let query = self.field.read(cx).value.clone();
        let q = query.trim().to_string();
        // Matching / grouping clones containers only when the snapshot,
        // query, filter or grouping changed — not on every frame.
        let key = (sh.generation.get(), q.clone(), filter, grouped);
        let mut cache = self.rows.borrow_mut();
        if cache.as_ref().is_none_or(|c| c.key != key) {
            let shown: Vec<Container> = snap.containers.iter().filter(|c| (filter == Filter::All || c.state.is_up()) && c.matches(&q)).cloned().collect();
            let groups = if grouped { api::group_by_project(&shown) } else { Vec::new() };
            *cache = Some(RowsCache { key, shown: if grouped { Vec::new() } else { shown }, groups });
        }
        let Some(rc) = cache.as_ref() else { return div().into_any_element() };
        let scale = CpuScale { max: snap.live.values().filter_map(|l| l.cpu).fold(PERCENT, f32::max), hot: hot_cpu(cx) };
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut list_h = 0.0;
        let entries;
        if grouped {
            let groups = &rc.groups;
            entries = groups.len();
            for g in groups.iter().take(ROWS) {
                if g.project.is_none() || g.members.len() == 1 {
                    for c in &g.members {
                        rows.push(container_row(c, &snap, ui, sh, palette, scale, row_r, false, g.project.is_some(), now));
                        list_h += ROW_H + ROW_GAP;
                    }
                } else {
                    let els = project_rows(g, &snap, ui, sh, palette, scale, row_r, now);
                    list_h += ROW_H + ROW_GAP + (els.len() - 1) as f32 * (CHILD_H + ROW_GAP);
                    rows.extend(els);
                }
            }
        } else {
            entries = rc.shown.len();
            for c in rc.shown.iter().take(ROWS) {
                rows.push(container_row(c, &snap, ui, sh, palette, scale, row_r, false, true, now));
                list_h += ROW_H + ROW_GAP;
            }
        }
        drop(cache);
        let more = entries.saturating_sub(ROWS);
        let empty = rows.is_empty();
        let list_h = if empty { EMPTY_H } else { list_h.min(LIST_MAX_H) };
        self.set_height(base_h + SEARCH_H + CONTROLS_GAP_PX + COLS_H + COLUMNS_GAP_PX + list_h + if more > 0 { FOOTER_H } else { 0.0 });

        let mut controls = div().flex().items_center().gap(px(tokens::SM)).child(search_box(&self.field, &query, palette));
        if stopped_too {
            let (ui2, sh2) = (ui.clone(), sh.clone());
            controls = controls.child(segmented("ctr-filter", &[(Filter::Running, tk("Running"), "play"), (Filter::All, tk("All"), "list-check")], filter, palette, move |f, _, cx| {
                ui2.borrow_mut().filter = f;
                sh2.views.notify(cx);
            }));
        }

        let label = |s: &'static str| div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.with_alpha(COLUMN_LABEL_ALPHA).hsla()).child(s);
        let columns = div()
            .h(px(COLS_H))
            .flex()
            .items_center()
            .gap(px(ROW_INNER_GAP_PX))
            .px(px(ROW_PAD_X_PX))
            .child(div().w(px(AVATAR)).flex_none())
            .child(div().flex_1().child(label(t("CONTAINER"))))
            .child(div().w(px(CPU_W)).flex().justify_end().child(label(t("CPU"))))
            .child(div().w(px(MEM_W)).flex().justify_end().child(label(t("MEMORY"))))
            .child(div().w(px(ACTIONS_W)).flex_none());

        let list: AnyElement = if empty {
            if snap.listed.is_none() {
                empty_state("box", t("Connecting to the engine…"), t("Reading the container list"), EMPTY_H, palette).into_any_element()
            } else if !q.is_empty() {
                empty_state("search", tf("No container matches \u{201c}{query}\u{201d}", &[("query", &q)]), t("Try a name, image, compose project or ID"), EMPTY_H, palette).into_any_element()
            } else if snap.containers.is_empty() {
                empty_state("box", t("No containers yet"), t("Containers you create with docker or podman appear here"), EMPTY_H, palette).into_any_element()
            } else {
                empty_state("pause", t("Nothing is running"), tf("{count} stopped containers — show All to start one", &[("count", &snap.containers.len())]), EMPTY_H, palette).into_any_element()
            }
        } else {
            let fill = crate::shell::frame_color(palette);
            let clipped = list_h >= LIST_MAX_H - SIZE_EPSILON_PX;
            div()
                .relative()
                .h(px(list_h))
                .child(div().id("ctr-list").size_full().overflow_y_scroll().flex().flex_col().gap(px(ROW_GAP)).children(rows))
                .when(clipped, |el| el.child(div().absolute().left_0().right_0().bottom_0().h(px(LIST_FADE_H_PX)).bg(oklab_gradient(ANGLE_DOWN_DEG, fill.with_alpha(0.0), fill))))
                .into_any_element()
        };

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
                div().flex().flex_col().gap(px(COLUMNS_GAP_PX)).child(columns).child(list).when(more > 0, |el| {
                    el.child(
                        div()
                            .h(px(FOOTER_H - COLUMNS_GAP_PX))
                            .flex()
                            .items_center()
                            .justify_center()
                            .gap(px(INLINE_GAP_PX))
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

// ── Commands ─────────────────────────────────────────────────────────────

/// Runs `work` on a thread; its JSON comes back as the command result.
fn pending(work: impl FnOnce() -> Result<Value, String> + Send + 'static) -> Output {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let result = work().map(|v| serde_json::to_string_pretty(&v).unwrap_or_default());
        let _ = tx.try_send(result);
    });
    Output::Pending(rx)
}

/// `containers stats` rounds CPU percentages to one decimal.
const CPU_DECIMALS: f32 = 10.0;

/// Pause between the two stats samples of `containers stats`.
const STATS_GAP: Duration = Duration::from_millis(1000);

fn container_json(c: &Container) -> Value {
    json!({
        "id": c.short_id(),
        "name": c.name,
        "image": c.image,
        "state": c.state,
        "status": c.status,
        "health": c.health,
        "project": c.project,
        "service": c.service,
        "ports": c.unique_ports().iter().map(port_label).collect::<Vec<_>>(),
    })
}

fn register_action(r: &mut PluginRegistry, path: &'static str, summary: &'static str, action: Action, example: &'static str) {
    r.command(
        ID,
        CommandSpec::new_output(path, summary, move |a, cx| {
            let target = a.str(0)?.to_string();
            let pref = preference(cx);
            Ok(pending(move || {
                let (ep, mut client) = Client::connect(pref).map_err(|e| e.message())?;
                let c = client.resolve(&target).map_err(|e| e.message())?;
                client.act(&c.id, action).map_err(|e| e.message())?;
                Ok(json!({ "engine": ep.engine, "name": c.name, "id": c.short_id(), "action": action }))
            }))
        })
        .arg(ArgSpec::required("container", ArgKind::String, tk("container name or ID")))
        .example(example),
    );
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Containers"), tk("Docker and Podman containers: status, live usage, logs and controls"), None), |window, cx, panel_cx| {
        let sh = shared(cx);
        // Prime the installed-RAM figure off the UI thread.
        cx.background_executor().spawn(async { host_memory() }).detach();
        let bar_sh = sh.clone();
        let bar = fn_view(
            None,
            move |palette, window, cx| {
                bar_sh.views.track(window);
                bar_sh.sync(cx);
                let snap = bar_sh.snapshot();
                let running = snap.running();
                let ok = snap.error.is_none() && snap.listed.is_some();
                let tint = if ok && running > 0 { palette.on_surface } else { palette.on_surface_variant.with_alpha(CHIP_IDLE_ALPHA) };
                div()
                    .flex()
                    .items_center()
                    .gap(px(CHIP_GAP_PX))
                    .child(icon("box", crate::style::bar_icon(CHIP_ICON_PX), tint))
                    .when(ok && running > 0, |el| el.child(div().font_features(tabular_nums()).text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).child(running.to_string())))
                    .into_any_element()
            },
            window,
            cx,
        );
        // Typing re-renders this plugin's views only.
        let views = sh.views.clone();
        let field = cx.new(|cx| TextField::new(String::new(), t("Name, image or project…"), true, Rc::new(move |_, _, cx| views.notify(cx)), cx));
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(INITIAL_H))));
        let view = View { p: panel_cx.clone(), sh, ui: Rc::new(RefCell::new(Ui::default())), field, size_cell: size_cell.clone(), rows: Rc::default(), watch: CloseWatch::default() };
        let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), ..PluginInstance::empty() }
    });
    r.command(
        ID,
        CommandSpec::new_output("containers list", tk("Containers of the configured engine (JSON), optionally filtered"), |a, cx| {
            let pref = preference(cx);
            let query = a.rest(0);
            Ok(pending(move || {
                let (ep, mut client) = Client::connect(pref).map_err(|e| e.message())?;
                let list = client.list().map_err(|e| e.message())?;
                let items: Vec<Value> = list.iter().filter(|c| c.matches(&query)).map(container_json).collect();
                Ok(json!({ "engine": ep.engine, "socket": ep.socket, "containers": items }))
            }))
        })
        .arg(ArgSpec::optional("query", ArgKind::String, tk("name, image, compose project or ID")))
        .example("containers list postgres"),
    );
    r.command(
        ID,
        CommandSpec::new_output("containers stats", tk("Live CPU and memory of running containers (JSON)"), |_, cx| {
            let pref = preference(cx);
            Ok(pending(move || {
                let (ep, mut client) = Client::connect(pref).map_err(|e| e.message())?;
                let running: Vec<Container> = client.list().map_err(|e| e.message())?.into_iter().filter(|c| c.state == State::Running).collect();
                let first: Vec<_> = running.iter().map(|c| client.stats(&c.id).ok()).collect();
                std::thread::sleep(STATS_GAP);
                let items: Vec<Value> = running
                    .iter()
                    .zip(first)
                    .filter_map(|(c, a)| {
                        let b = client.stats(&c.id).ok()?;
                        let cpu = a.and_then(|a| api::cpu_percent(&a, &b));
                        Some(json!({ "name": c.name, "id": c.short_id(), "cpu_percent": cpu.map(|v| (v * CPU_DECIMALS).round() / CPU_DECIMALS), "mem_used": b.mem_used, "mem_limit": b.mem_limit }))
                    })
                    .collect();
                Ok(json!({ "engine": ep.engine, "running": items }))
            }))
        })
        .example("containers stats"),
    );
    r.command(
        ID,
        CommandSpec::new_output("containers logs", tk("The last log lines of a container"), |a, cx| {
            let target = a.str(0)?.to_string();
            let lines = match a.get(1) {
                Some(_) => a.int(1)?.max(1) as usize,
                None => log_lines(cx),
            };
            let pref = preference(cx);
            let (tx, rx) = async_channel::bounded(1);
            std::thread::spawn(move || {
                let result = (|| {
                    let (_, mut client) = Client::connect(pref).map_err(|e| e.message())?;
                    let c = client.resolve(&target).map_err(|e| e.message())?;
                    let logs = client.logs(&c.id, lines).map_err(|e| e.message())?;
                    Ok(logs.into_iter().map(|l| l.text).collect::<Vec<_>>().join("\n"))
                })();
                let _ = tx.try_send(result);
            });
            Ok(Output::Pending(rx))
        })
        .arg(ArgSpec::required("container", ArgKind::String, tk("container name or ID")))
        .arg(ArgSpec::optional("lines", ArgKind::Integer, tk("how many recent lines")))
        .example("containers logs kaizen-api-1 50"),
    );
    register_action(r, "containers start", tk("Start a stopped container"), Action::Start, "containers start kaizen-db-1");
    register_action(r, "containers stop", tk("Stop a running container"), Action::Stop, "containers stop kaizen-db-1");
    register_action(r, "containers restart", tk("Restart a container"), Action::Restart, "containers restart kaizen-api-1");
    register_action(r, "containers pause", tk("Freeze a running container's processes"), Action::Pause, "containers pause kaizen-api-1");
    register_action(r, "containers unpause", tk("Resume a paused container"), Action::Unpause, "containers unpause kaizen-api-1");
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Seconds per minute / hour / day.
    const MINUTE: i64 = 60;
    const HOUR: i64 = 60 * MINUTE;
    const DAY: i64 = 24 * HOUR;

    #[test]
    fn spans_are_compact() {
        assert_eq!(span(45), "45 sec");
        assert_eq!(span(8 * MINUTE + 3), "8 min");
        assert_eq!(span(4 * HOUR + 12 * MINUTE), "4 h 12 min");
        assert_eq!(span(3 * DAY + 2 * HOUR + 59), "3 d 2 h");
        assert_eq!(span(-5), "0 sec");
    }

    #[test]
    fn images_are_shortened() {
        assert_eq!(short_image("postgres:18.6-trixie@sha256:4ef4"), "postgres:18.6-trixie");
        assert_eq!(short_image("docker.io/library/nginx:latest"), "nginx:latest");
        assert_eq!(short_image("ghcr.io/owner/app:1"), "owner/app:1");
        assert_eq!(short_image("localhost:5000/team/svc"), "team/svc");
        assert_eq!(short_image("portainer/portainer-ce"), "portainer/portainer-ce");
        assert_eq!(short_image("sha256:4ef4dbc939d61acea57712655ddb4b4ab274"), "sha256:4ef4dbc939d6");
        // Same image → same hue regardless of tag or registry.
        assert_eq!(image_hue("docker.io/library/nginx:1"), image_hue("nginx:latest"));
    }

    #[test]
    fn ports_render_published_first() {
        assert_eq!(port_label(&api::Port { private: 80, public: Some(8080), ip: String::new(), proto: "tcp".into() }), "8080→80");
        assert_eq!(port_label(&api::Port { private: 5432, public: None, ip: String::new(), proto: "tcp".into() }), "5432/tcp");
    }

    #[test]
    fn messages_name_the_container() {
        assert_eq!(done_message(Action::Stop, "web", 1), "Stopped web");
        assert_eq!(done_message(Action::Unpause, "web", 1), "Resumed web");
        assert_eq!(done_message(Action::Start, "", 3), "Started 3 containers");
    }

    #[test]
    fn ink_contrasts_with_fill() {
        assert!(ink_on(Color::oklch(0.8, 0.12, 30.0)).lightness() < 0.4);
        assert!(ink_on(Color::oklch(0.5, 0.15, 30.0)).lightness() > 0.9);
    }
}
