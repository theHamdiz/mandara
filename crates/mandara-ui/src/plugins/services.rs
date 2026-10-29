//! Services: a systemd unit manager for the user and system managers.
//!
//! The bar chip is a server glyph with a coral badge counting failed units
//! on both buses (hidden when nothing failed, or when the alert is off in
//! Studio). The panel lists loaded units with live state pills, search,
//! Failed / Running / All filters and a services-only / all-types switch;
//! selecting a unit opens a sheet with Start / Stop / Restart / Enable /
//! Disable (Stop and Disable arm first, like the process hunter's End) and
//! its recent journal lines. System-unit changes go through polkit, so
//! Mandara's own authentication dialog asks for a password.
//!
//! One model is shared by every bar instance (thread-local `Weak`): a
//! worker task polls the failed counts at a slow cadence while any chip or
//! panel exists and stops once they are all gone; full unit lists are only
//! fetched while the panel is open. Commands: `services list|failed|status|
//! logs|start|stop|restart|enable|disable`.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext as _, ElementId, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px, size,
};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk, tr};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry};
use mandara_services::systemd::{self, Health, JournalLine, Scope, Unit, UnitAction};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::fn_view;
use super::panel_fx::{
    ANGLE_DIAGONAL_DEG, ANGLE_DOWN_DEG, EXPIRY_MARGIN, HEADER_CHIP_PX, HEADER_H, PARTNER_HUE_DEG, PanelShape, SMALL_ICON_PX, ViewSet,
    alpha, empty_state, glass_card, glow, header, readable, redraw_in,
};
use crate::fonts::tabular_nums;
use crate::icons::icon;
use crate::kit::{icon_button, reveal, text, tokens};
use crate::text_field::TextField;

pub const ID: &str = "services";

/// Default for `plugins.settings.services.journal_lines`.
pub const DEFAULT_JOURNAL_LINES: i64 = 40;
/// Range accepted for `journal_lines`.
pub const JOURNAL_LINES_RANGE: (i64, i64) = (10, 200);

const PANEL_W: f32 = 600.0;
const PAD: f32 = 14.0;
const GAP: f32 = 12.0;
const TILE_H: f32 = 68.0;
const SEARCH_H: f32 = 36.0;
const CONTROLS_GAP: f32 = 10.0;
const ROW_H: f32 = 52.0;
const ROW_GAP: f32 = 2.0;
const AVATAR: f32 = 34.0;
/// Rows visible before the list scrolls (the cut half row hints at more).
const VISIBLE_ROWS: f32 = 7.5;
/// Visible list height before it scrolls.
const LIST_MAX_H: f32 = VISIBLE_ROWS * (ROW_H + ROW_GAP);
const EMPTY_H: f32 = 196.0;
const FOOTER_H: f32 = 26.0;
/// Detail sheet under a selected unit.
const SHEET_PAD: f32 = 12.0;
const ACTIONS_H: f32 = 30.0;
const JOURNAL_HEAD_H: f32 = 22.0;
const JOURNAL_H: f32 = 196.0;
const JOURNAL_LINE_H: f32 = 17.0;
/// Gap between the journal head and its well.
const JOURNAL_HEAD_GAP: f32 = 6.0;
const SHEET_H: f32 = SHEET_PAD * 2.0 + ACTIONS_H + GAP + JOURNAL_HEAD_H + JOURNAL_HEAD_GAP + JOURNAL_H;
/// The sheet's bottom margin inside the list.
const SHEET_MARGIN: f32 = 4.0;
/// Units rendered at most (the search narrows the rest).
const ROWS: usize = 60;
/// Failed-unit poll (seconds) while a chip exists (`failed_check_s`).
pub const DEFAULT_FAILED_CHECK_S: i64 = 60;
pub const FAILED_CHECK_S_RANGE: (i64, i64) = (10, 3600);
/// Unit list refresh (seconds) while the panel is open (`list_refresh_s`).
pub const DEFAULT_LIST_REFRESH_S: i64 = 4;
pub const LIST_REFRESH_S_RANGE: (i64, i64) = (1, 60);

/// Failed-unit poll period (`failed_check_s`).
fn failed_poll(cx: &App) -> Duration {
    let s: i64 = crate::state::config(cx).read(cx).snapshot().plugins.setting(ID, "failed_check_s", DEFAULT_FAILED_CHECK_S);
    Duration::from_secs(s.clamp(FAILED_CHECK_S_RANGE.0, FAILED_CHECK_S_RANGE.1) as u64)
}

/// Unit list refresh period (`list_refresh_s`).
fn list_refresh(cx: &App) -> Duration {
    let s: i64 = crate::state::config(cx).read(cx).snapshot().plugins.setting(ID, "list_refresh_s", DEFAULT_LIST_REFRESH_S);
    Duration::from_secs(s.clamp(LIST_REFRESH_S_RANGE.0, LIST_REFRESH_S_RANGE.1) as u64)
}
/// A Stop / Disable press arms the button; a second press within this acts.
const ARM: Duration = Duration::from_secs(3);
/// Badge counts above this read "99+".
const BADGE_MAX: usize = 99;
/// Re-render cadence while a Stop / Disable press is armed.
const ARM_TICK: Duration = Duration::from_millis(500);
/// Height changes smaller than this don't resize the panel.
const SIZE_EPSILON_PX: f32 = 0.5;

// Visual tokens of this panel (px unless noted; alpha pairs are
// `(dark, light)`).
/// Small tinted label badge (unit kind, file state).
const BADGE_H_PX: f32 = 16.0;
const BADGE_PAD_X_PX: f32 = 6.0;
const BADGE_TEXT_PX: f32 = 10.0;
const BADGE_TINT: (f32, f32) = (0.14, 0.1);
/// Label size of buttons and segments: half a step above the label size.
const BUTTON_TEXT_PX: f32 = text::LABEL + 0.5;
/// State avatar: radius and glyph as fractions of its side, fill alphas.
const AVATAR_RADIUS_FRAC: f32 = 0.32;
const AVATAR_GLYPH_FRAC: f32 = 0.44;
const AVATAR_FILL: (f32, f32) = (0.14, 0.11);
/// State pill: height, padding, gap, dot, fill and dot glow.
const PILL_H_PX: f32 = 22.0;
const PILL_PAD_X_PX: f32 = 9.0;
const PILL_GAP_PX: f32 = 6.0;
const DOT_PX: f32 = 6.0;
const PILL_FILL: (f32, f32) = (0.12, 0.09);
const DOT_GLOW: (f32, f32) = (0.5, 6.0);
const SUBSTATE_ALPHA: f32 = 0.85;
/// Unit rows.
const ROW_PAD_X_PX: f32 = 8.0;
const ROW_INNER_GAP_PX: f32 = 10.0;
const ROW_HOVER: (f32, f32) = (0.05, 0.04);
const ROW_SELECTED: (f32, f32) = (0.1, 0.08);
const LINE_GAP_PX: f32 = 1.0;
const INLINE_GAP_PX: f32 = 6.0;
const MUTED_TEXT_ALPHA: f32 = 0.82;
/// Action buttons in the sheet: padding, gap, armed glow (alpha, blur, y).
const ACTION_PAD_X_PX: f32 = 12.0;
const ACTION_GAP_PX: f32 = 5.0;
const ARMED_GLOW: (f32, f32, f32) = (0.3, 8.0, 1.0);
const ACTION_FILL: (f32, f32) = (0.16, 0.11);
const ACTION_HOVER: (f32, f32) = (0.26, 0.18);
const BUSY_FILL_ALPHA: f32 = 0.06;
/// Journal: timestamp column width / alpha, well padding and fill.
const JOURNAL_TIME_W_PX: f32 = 92.0;
const JOURNAL_TIME_ALPHA: f32 = 0.75;
const JOURNAL_PAD_X_PX: f32 = 10.0;
const JOURNAL_PAD_Y_PX: f32 = 8.0;
const JOURNAL_LINE_GAP_PX: f32 = 2.0;
const JOURNAL_WELL: (f32, f32) = (0.045, 0.04);
const UPDATING_ALPHA: f32 = 0.7;
/// Quiet one-line states inside the journal well.
const QUIET_PAD_X_PX: f32 = 16.0;
const QUIET_ICON_PX: f32 = 14.0;
/// Sheet: horizontal / bottom inset, lock hint glyph, nested radius inset.
const SHEET_INSET_PX: f32 = 4.0;
const HINT_ICON_PX: f32 = 11.0;
const NESTED_RADIUS_INSET_PX: f32 = 4.0;
const MIN_NESTED_RADIUS_PX: f32 = 8.0;
/// Tiles: padding, gradient alphas, glyph, value size / line height,
/// caption size.
const TILE_PAD_X_PX: f32 = 12.0;
const TILE_PAD_Y_PX: f32 = 10.0;
const TILE_ICON_RADIUS_PX: f32 = 12.0;
const TILE_ICON_FILL: (f32, f32) = (0.2, 0.1);
const TILE_ICON_PX: f32 = 17.0;
const TILE_VALUE_PX: f32 = 20.0;
const TILE_VALUE_LINE_PX: f32 = 24.0;
const TILE_CAPTION_PX: f32 = 10.5;
/// Segmented controls: segment padding / radius, scope toggle height,
/// group padding / radius / fill, hover wash.
const SEG_PAD_X_PX: f32 = 10.0;
const SEG_RADIUS_PX: f32 = 9.0;
const SEG_GAP_PX: f32 = 5.0;
const SEG_INSET_PX: f32 = 3.0;
const SCOPE_SEG_H_PX: f32 = 24.0;
const SCOPE_TOGGLE_H_PX: f32 = 30.0;
const SEGMENTS_RADIUS_PX: f32 = 12.0;
const SEGMENTS_GAP_PX: f32 = 2.0;
const SEGMENTS_FILL: (f32, f32) = (0.05, 0.045);
const SEG_ACTIVE: (f32, f32) = (0.18, 0.13);
const SEG_HOVER_ALPHA: f32 = 0.05;
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
/// Fade over a clipped list's last row; footer glyph.
const LIST_FADE_H_PX: f32 = 22.0;
const FOOTER_ICON_PX: f32 = 11.0;
/// Rows sit this far inside the card radius, never rounder than the min.
const ROW_RADIUS_INSET_PX: f32 = 2.0;
const MIN_ROW_RADIUS_PX: f32 = 10.0;
/// Chip: glyph and failed-count badge.
const CHIP_GAP_PX: f32 = 4.0;
const CHIP_ICON_PX: f32 = 15.0;
const CHIP_BADGE_PX: f32 = 16.0;
const CHIP_BADGE_PAD_X_PX: f32 = 4.0;
/// Ink on solid fills: fills lighter than this get dark ink.
const INK_FLIP_LIGHTNESS: f32 = 0.64;
const DARK_INK: (f32, f32) = (0.2, 0.02);
const LIGHT_INK_L: f32 = 0.99;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Filter {
    Failed,
    Running,
    All,
}

type UnitKey = (Scope, String);

/// A journal line formatted once when it is read (not on every frame).
struct JournalRow {
    time: SharedString,
    message: SharedString,
    priority: u8,
}

impl JournalRow {
    fn new(l: JournalLine) -> Self {
        Self { time: l.timestamp_us.map(format_time).unwrap_or_default().into(), message: l.message.into(), priority: l.priority }
    }
}

#[derive(Default)]
struct Journal {
    key: Option<UnitKey>,
    lines: Option<Result<std::sync::Arc<Vec<JournalRow>>, String>>,
    loading: bool,
}

/// The filtered, ordered rows of one unit list plus its tile counts,
/// recomputed only when the list, query or filters change.
struct VisibleCache {
    key: (Scope, u64, String, Filter, bool),
    /// Indices into the unit list.
    rows: Rc<[usize]>,
    typed: usize,
    running: usize,
    failed: usize,
    enabled: usize,
}

struct Model {
    failed: HashMap<Scope, usize>,
    /// Shared so renders borrow a list instead of cloning every unit.
    lists: HashMap<Scope, Result<Rc<Vec<Unit>>, String>>,
    /// Bumped whenever a list lands (keys the visible-rows cache).
    generation: u64,
    listed_at: HashMap<Scope, Instant>,
    listing: HashSet<Scope>,
    /// `None` follows the Studio default.
    scope: Option<Scope>,
    filter: Filter,
    all_types: bool,
    selected: Option<UnitKey>,
    journal: Journal,
    armed: HashMap<(UnitKey, UnitAction), Instant>,
    busy: HashMap<UnitKey, UnitAction>,
    /// (ok, message, shown at).
    status: Option<(bool, String, Instant)>,
    /// The chip and panel views; async results notify only these.
    views: ViewSet,
    /// Panels of this plugin currently open (any monitor).
    open_panels: usize,
}

impl Model {
    fn new() -> Self {
        Self {
            failed: HashMap::new(),
            lists: HashMap::new(),
            generation: 0,
            listed_at: HashMap::new(),
            listing: HashSet::new(),
            scope: None,
            filter: Filter::All,
            all_types: false,
            selected: None,
            journal: Journal::default(),
            armed: HashMap::new(),
            busy: HashMap::new(),
            status: None,
            views: ViewSet::default(),
            open_panels: 0,
        }
    }

    /// Drops panel-only data once no panel is open: unit lists, the
    /// journal, the selection and transient state. The failed counts stay
    /// for the bar chip.
    fn free_panel_data(&mut self) {
        self.lists = HashMap::new();
        self.listed_at = HashMap::new();
        self.journal = Journal::default();
        self.selected = None;
        self.armed = HashMap::new();
        self.status = None;
    }

    fn failed_total(&self) -> usize {
        self.failed.values().sum()
    }

    fn armed(&self, key: &UnitKey, action: UnitAction) -> bool {
        self.armed.get(&(key.clone(), action)).is_some_and(|t| t.elapsed() < ARM)
    }

    fn absorb_list(&mut self, scope: Scope, result: Result<Vec<Unit>, String>) {
        if let Ok(units) = &result {
            self.failed.insert(scope, units.iter().filter(|u| u.health() == Health::Failed).count());
        }
        self.lists.insert(scope, result.map(Rc::new));
        self.generation = self.generation.wrapping_add(1);
        self.listed_at.insert(scope, Instant::now());
        self.listing.remove(&scope);
    }
}

type Shared = Rc<RefCell<Model>>;

/// Re-renders the views of `m` (never every window).
fn notify(m: &Shared, cx: &mut App) {
    let views = m.borrow().views.clone();
    views.notify(cx);
}

thread_local! {
    static MODEL: RefCell<Weak<RefCell<Model>>> = const { RefCell::new(Weak::new()) };
}

/// The shared model; the first caller starts the failed-count worker.
fn model(cx: &mut App) -> Shared {
    if let Some(m) = MODEL.with(|w| w.borrow().upgrade()) {
        return m;
    }
    let m = Rc::new(RefCell::new(Model::new()));
    MODEL.with(|w| *w.borrow_mut() = Rc::downgrade(&m));
    start_worker(Rc::downgrade(&m), cx);
    m
}

/// Runs blocking work on its own thread (D-Bus calls may wait on a polkit
/// dialog for a long time, so they stay off GPUI's executor pool).
fn off_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> async_channel::Receiver<T> {
    let (tx, rx) = async_channel::bounded(1);
    let _ = std::thread::Builder::new().name("mandara-systemd".into()).spawn(move || {
        let _ = tx.send_blocking(f());
    });
    rx
}

/// Polls failed units on both buses until every handle is gone.
fn start_worker(weak: Weak<RefCell<Model>>, cx: &mut App) {
    cx.spawn(async move |cx| {
        loop {
            if weak.strong_count() == 0 {
                break;
            }
            let rx = off_thread(|| Scope::ALL.map(|s| (s, systemd::failed_units(s).map(|u| u.len()))));
            if let Ok(results) = rx.recv().await {
                let Some(m) = weak.upgrade() else { break };
                let changed = {
                    let mut mb = m.borrow_mut();
                    let before = mb.failed.clone();
                    for (scope, r) in results {
                        match r {
                            Ok(n) => {
                                mb.failed.insert(scope, n);
                            }
                            Err(_) => {
                                mb.failed.remove(&scope);
                            }
                        }
                    }
                    mb.failed != before
                };
                // Re-render only when a count actually moved.
                if changed {
                    cx.update(|cx| notify(&m, cx));
                }
            }
            let every = cx.update(|cx| failed_poll(cx));
            cx.background_executor().timer(every).await;
        }
    })
    .detach();
}

fn journal_lines(cx: &App) -> usize {
    let n: i64 = crate::state::config(cx).read(cx).snapshot().plugins.setting(ID, "journal_lines", DEFAULT_JOURNAL_LINES);
    n.clamp(JOURNAL_LINES_RANGE.0, JOURNAL_LINES_RANGE.1) as usize
}

fn default_scope(cx: &App) -> Scope {
    let s: String = crate::state::config(cx).read(cx).snapshot().plugins.setting(ID, "default_scope", "system".to_string());
    Scope::parse(&s).unwrap_or(Scope::System)
}

fn alert_enabled(cx: &App) -> bool {
    crate::state::config(cx).read(cx).snapshot().plugins.setting(ID, "alert_failed", true)
}

/// Starts a unit-list fetch for `scope` when one is due (or `force`d).
fn refresh_list(m: &Shared, scope: Scope, force: bool, cx: &mut App) {
    let every = list_refresh(cx);
    {
        let mut mb = m.borrow_mut();
        let due = force || mb.listed_at.get(&scope).is_none_or(|t| t.elapsed() >= every);
        if mb.listing.contains(&scope) || !due {
            return;
        }
        mb.listing.insert(scope);
    }
    let rx = off_thread(move || systemd::list_units(scope));
    let weak = Rc::downgrade(m);
    cx.spawn(async move |cx| {
        let result = rx.recv().await.unwrap_or_else(|_| Err(t("The service did not reply").into()));
        if let Some(m) = weak.upgrade() {
            {
                let mut mb = m.borrow_mut();
                mb.absorb_list(scope, result);
                // A list landing after every panel closed only updates
                // the chip's failed count.
                if mb.open_panels == 0 {
                    mb.free_panel_data();
                }
            }
            cx.update(|cx| notify(&m, cx));
        }
    })
    .detach();
}

/// Loads the journal of `key` (newest last from journalctl).
fn load_journal(m: &Shared, key: UnitKey, cx: &mut App) {
    let lines = journal_lines(cx);
    {
        let mut mb = m.borrow_mut();
        if mb.journal.key.as_ref() != Some(&key) {
            mb.journal.lines = None;
        }
        mb.journal.key = Some(key.clone());
        mb.journal.loading = true;
    }
    let (scope, unit) = key.clone();
    // Timestamps are formatted on the reader thread, once per line.
    let rx = off_thread(move || systemd::journal(scope, &unit, lines).map(|ls| std::sync::Arc::new(ls.into_iter().map(JournalRow::new).collect::<Vec<_>>())));
    let weak = Rc::downgrade(m);
    cx.spawn(async move |cx| {
        let result = rx.recv().await.unwrap_or_else(|_| Err(t("The service did not reply").into()));
        if let Some(m) = weak.upgrade() {
            {
                let mut mb = m.borrow_mut();
                if mb.journal.key.as_ref() == Some(&key) {
                    mb.journal.lines = Some(result);
                    mb.journal.loading = false;
                }
            }
            cx.update(|cx| notify(&m, cx));
        }
    })
    .detach();
}

fn done_message(action: UnitAction, unit: &str) -> String {
    let args: [(&str, &dyn std::fmt::Display); 1] = [("unit", &unit)];
    match action {
        UnitAction::Start => tf("Started {unit}", &args),
        UnitAction::Stop => tf("Stopped {unit}", &args),
        UnitAction::Restart => tf("Restarted {unit}", &args),
        UnitAction::Enable => tf("Enabled {unit}", &args),
        UnitAction::Disable => tf("Disabled {unit}", &args),
    }
}

/// Runs `action` off-thread (system units show the polkit dialog), then
/// refreshes the list and the journal.
fn run_action(m: &Shared, key: UnitKey, action: UnitAction, cx: &mut App) {
    {
        let mut mb = m.borrow_mut();
        if mb.busy.contains_key(&key) {
            return;
        }
        mb.busy.insert(key.clone(), action);
        mb.armed.retain(|(k, _), _| k != &key);
    }
    let (scope, unit) = key.clone();
    let rx = off_thread(move || systemd::act(action, scope, &unit));
    let weak = Rc::downgrade(m);
    cx.spawn(async move |cx| {
        let result = rx.recv().await.unwrap_or_else(|_| Err(t("The service did not reply").into()));
        let Some(m) = weak.upgrade() else { return };
        {
            let mut mb = m.borrow_mut();
            mb.busy.remove(&key);
            let short = key.1.strip_suffix(".service").unwrap_or(&key.1).to_string();
            mb.status = Some(match result {
                Ok(_) => (true, done_message(action, &short), Instant::now()),
                Err(e) => (false, tf("{unit}: {error}", &[("unit", &short), ("error", &e)]), Instant::now()),
            });
        }
        cx.update(|cx| {
            refresh_list(&m, key.0, true, cx);
            load_journal(&m, key.clone(), cx);
            notify(&m, cx);
        });
    })
    .detach();
}

// ── Pieces ───────────────────────────────────────────────────────────────

/// Legible foreground on a solid `bg` (dark ink on light fills).
fn ink_on(bg: Color) -> Color {
    if bg.lightness() > INK_FLIP_LIGHTNESS { Color::oklch(DARK_INK.0, DARK_INK.1, bg.hue()) } else { Color::oklch(LIGHT_INK_L, 0.0, 0.0) }
}

fn health_color(h: Health, palette: &MandaraPalette) -> Color {
    match h {
        Health::Active => palette.success_emerald,
        Health::Failed => palette.urgent_coral,
        Health::Transitioning => palette.warning_amber,
        Health::Inactive => palette.on_surface_variant,
    }
}

fn health_icon(h: Health) -> &'static str {
    match h {
        Health::Active => "play",
        Health::Failed => "alert",
        Health::Transitioning => "timer",
        Health::Inactive => "stop",
    }
}

fn health_rank(h: Health) -> u8 {
    match h {
        Health::Failed => 0,
        Health::Transitioning => 1,
        Health::Active => 2,
        Health::Inactive => 3,
    }
}

fn state_label(active_state: &str) -> String {
    match active_state {
        "active" => t("active").to_string(),
        "inactive" => t("inactive").to_string(),
        "failed" => t("failed").to_string(),
        "activating" => t("activating").to_string(),
        "deactivating" => t("deactivating").to_string(),
        "reloading" => t("reloading").to_string(),
        other => other.to_string(),
    }
}

/// Units of the current view, filtered and ordered (failed first).
fn visible<'a>(units: &'a [Unit], query: &str, filter: Filter, all_types: bool) -> Vec<&'a Unit> {
    visible_indices(units, query, filter, all_types).into_iter().map(|i| &units[i]).collect()
}

/// [`visible`] as indices into `units`.
fn visible_indices(units: &[Unit], query: &str, filter: Filter, all_types: bool) -> Vec<usize> {
    let q = query.trim().to_lowercase();
    let mut out: Vec<(usize, &Unit)> = units
        .iter()
        .enumerate()
        .filter(|(_, u)| all_types || u.kind() == "service")
        .filter(|(_, u)| match filter {
            Filter::Failed => u.health() == Health::Failed,
            Filter::Running => u.health() == Health::Active,
            Filter::All => true,
        })
        .filter(|(_, u)| q.is_empty() || u.name.to_lowercase().contains(&q) || u.description.to_lowercase().contains(&q))
        .collect();
    // Lowercase each name once, not twice per comparison.
    out.sort_by_cached_key(|(_, u)| (health_rank(u.health()), u.name.to_lowercase()));
    out.into_iter().map(|(i, _)| i).collect()
}

/// [`visible`] as indices plus the tile counts, for [`VisibleCache`].
fn visible_cache(units: &[Unit], key: (Scope, u64, String, Filter, bool)) -> VisibleCache {
    let (_, _, q, filter, all_types) = &key;
    let rows: Rc<[usize]> = visible_indices(units, q, *filter, *all_types).into();
    let typed: Vec<&Unit> = units.iter().filter(|u| *all_types || u.kind() == "service").collect();
    VisibleCache {
        rows,
        typed: typed.len(),
        running: typed.iter().filter(|u| u.health() == Health::Active).count(),
        failed: typed.iter().filter(|u| u.health() == Health::Failed).count(),
        enabled: typed.iter().filter(|u| u.is_enabled()).count(),
        key,
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
        .bg(tint.with_alpha(alpha(BADGE_TINT, palette)).hsla())
        .font_features(tabular_nums())
        .text_size(px(BADGE_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(tint.hsla())
        .child(label.into())
        .into_any_element()
}

fn file_state_badge(state: Option<&str>, palette: &MandaraPalette) -> Option<AnyElement> {
    match state? {
        "enabled" | "enabled-runtime" => Some(badge(t("enabled"), readable(palette.primary, palette), palette)),
        "disabled" => Some(badge(t("disabled"), palette.on_surface_variant, palette)),
        "masked" | "masked-runtime" => Some(badge(t("masked"), readable(palette.urgent_coral, palette), palette)),
        _ => None,
    }
}

/// State glyph on a tinted tile.
fn avatar(h: Health, palette: &MandaraPalette) -> AnyElement {
    let c = health_color(h, palette);
    let fg = readable(c, palette);
    div()
        .size(px(AVATAR))
        .flex_none()
        .rounded(px(crate::style::radius(AVATAR * AVATAR_RADIUS_FRAC)))
        .bg(c.with_alpha(alpha(AVATAR_FILL, palette)).hsla())
        .flex()
        .items_center()
        .justify_center()
        .child(icon(health_icon(h), AVATAR * AVATAR_GLYPH_FRAC, fg))
        .into_any_element()
}

/// Dot + state word + sub-state.
fn state_pill(u: &Unit, palette: &MandaraPalette) -> AnyElement {
    let h = u.health();
    let c = health_color(h, palette);
    let fg = if h == Health::Inactive { palette.on_surface_variant } else { readable(c, palette) };
    div()
        .flex_none()
        .h(px(PILL_H_PX))
        .px(px(PILL_PAD_X_PX))
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(PILL_GAP_PX))
        .bg(c.with_alpha(alpha(PILL_FILL, palette)).hsla())
        .child(div().size(px(DOT_PX)).rounded_full().bg(c.hsla()).when(h == Health::Active || h == Health::Failed, |el| el.shadow(vec![glow(c.with_alpha(DOT_GLOW.0), DOT_GLOW.1, 0.0, 0.0)])))
        .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(fg.hsla()).child(state_label(&u.active_state)))
        .when(!u.sub_state.is_empty() && u.sub_state != u.active_state, |el| {
            el.child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.with_alpha(SUBSTATE_ALPHA).hsla()).child(u.sub_state.clone()))
        })
        .into_any_element()
}

/// One unit; `index` is its position in the unit list (a cheap stable
/// element id instead of formatting the name on every frame).
fn unit_row(u: &Unit, index: usize, scope: Scope, m: &Shared, palette: &MandaraPalette, radius: f32) -> AnyElement {
    let key: UnitKey = (scope, u.name.clone());
    let selected = m.borrow().selected.as_ref() == Some(&key);
    let hover = palette.on_surface.with_alpha(alpha(ROW_HOVER, palette));
    let sel = readable(palette.primary, palette).with_alpha(alpha(ROW_SELECTED, palette));
    let m2 = m.clone();
    div()
        .id(ElementId::named_usize(scope.as_str(), index))
        .h(px(ROW_H))
        .flex_none()
        .px(px(ROW_PAD_X_PX))
        .rounded(px(radius))
        .flex()
        .items_center()
        .gap(px(ROW_INNER_GAP_PX))
        .cursor_pointer()
        .when(selected, |el| el.bg(sel.hsla()))
        .when(!selected, |el| el.hover(move |s| s.bg(hover.hsla())))
        .child(avatar(u.health(), palette))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(LINE_GAP_PX))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(INLINE_GAP_PX))
                        .min_w(px(0.0))
                        .child(div().min_w(px(0.0)).text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).truncate().child(u.short_name().to_string()))
                        .when(u.kind() != "service", |el| el.child(badge(u.kind().to_string(), readable(palette.tertiary, palette), palette)))
                        .when_some(file_state_badge(u.file_state.as_deref(), palette), |el, b| el.child(b)),
                )
                .child(
                    div()
                        .text_size(px(text::LABEL))
                        .text_color(palette.on_surface_variant.with_alpha(MUTED_TEXT_ALPHA).hsla())
                        .truncate()
                        .child(if u.description.is_empty() { u.name.clone() } else { u.description.clone() }),
                ),
        )
        .child(state_pill(u, palette))
        .on_click(move |_, _, cx| {
            let now = {
                let mut mb = m2.borrow_mut();
                if mb.selected.as_ref() == Some(&key) {
                    mb.selected = None;
                    false
                } else {
                    mb.selected = Some(key.clone());
                    true
                }
            };
            if now {
                load_journal(&m2, key.clone(), cx);
            }
            notify(&m2, cx);
        })
        .into_any_element()
}

/// One action button in the detail sheet. Stop / Disable arm first.
fn action_button(m: &Shared, key: &UnitKey, action: UnitAction, palette: &MandaraPalette) -> AnyElement {
    let (label, glyph, tint, needs_arm): (&str, &'static str, Color, bool) = match action {
        UnitAction::Start => (tk("Start"), "play", palette.success_emerald, false),
        UnitAction::Stop => (tk("Stop"), "stop", palette.urgent_coral, true),
        UnitAction::Restart => (tk("Restart"), "restart", palette.primary, false),
        UnitAction::Enable => (tk("Enable"), "check", palette.tertiary, false),
        UnitAction::Disable => (tk("Disable"), "close", palette.urgent_coral, true),
    };
    let armed = m.borrow().armed(key, action);
    let soft = readable(tint, palette);
    let ink = ink_on(tint);
    let (m, key) = (m.clone(), key.clone());
    div()
        .id(ElementId::Name(SharedString::from(format!("svc-act-{}", action.as_str()))))
        .h(px(ACTIONS_H))
        .px(px(ACTION_PAD_X_PX))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(ACTION_GAP_PX))
        .cursor_pointer()
        .text_size(px(BUTTON_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .when(armed, |el| el.bg(tint.hsla()).text_color(ink.hsla()).shadow(vec![glow(tint.with_alpha(ARMED_GLOW.0), ARMED_GLOW.1, 0.0, ARMED_GLOW.2)]))
        .when(!armed, |el| {
            let hover = tint.with_alpha(alpha(ACTION_HOVER, palette));
            el.bg(tint.with_alpha(alpha(ACTION_FILL, palette)).hsla()).text_color(soft.hsla()).hover(move |s| s.bg(hover.hsla()))
        })
        .child(icon(if armed { "check" } else { glyph }, SMALL_ICON_PX, if armed { ink } else { soft }))
        .child(if armed { t("Confirm").to_string() } else { tr(label) })
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            let go = !needs_arm || m.borrow().armed(&key, action);
            if go {
                run_action(&m, key.clone(), action, cx);
            } else {
                m.borrow_mut().armed.insert((key.clone(), action), Instant::now());
            }
            notify(&m, cx);
        })
        .into_any_element()
}

fn format_time(us: u64) -> String {
    use chrono::{Local, TimeZone as _};
    let Some(utc) = i64::try_from(us).ok().and_then(chrono::DateTime::from_timestamp_micros) else { return String::new() };
    let local = Local.from_utc_datetime(&utc.naive_utc());
    if local.date_naive() == Local::now().date_naive() { local.format("%H:%M:%S").to_string() } else { local.format("%m-%d %H:%M").to_string() }
}

/// syslog priorities: emerg..err, warning, notice.
const PRIORITY_ERR: u8 = 3;
const PRIORITY_WARNING: u8 = 4;
const PRIORITY_NOTICE: u8 = 5;

fn priority_color(priority: u8, palette: &MandaraPalette) -> Color {
    match priority {
        0..=PRIORITY_ERR => readable(palette.urgent_coral, palette),
        PRIORITY_WARNING => readable(palette.warning_amber, palette),
        PRIORITY_NOTICE => palette.on_surface,
        _ => palette.on_surface_variant,
    }
}

fn journal_view(m: &Shared, key: &UnitKey, palette: &MandaraPalette, radius: f32, mono: SharedString) -> AnyElement {
    let mb = m.borrow();
    let mine = mb.journal.key.as_ref() == Some(key);
    let loading = mine && mb.journal.loading;
    let body: AnyElement = match (mine, mb.journal.lines.as_ref()) {
        (true, Some(Ok(lines))) if !lines.is_empty() => {
            // Newest first, so the latest entries are visible without scrolling.
            let rows = lines.iter().rev().map(|l| {
                let c = priority_color(l.priority, palette);
                div()
                    .flex()
                    .gap(px(ROW_INNER_GAP_PX))
                    .min_h(px(JOURNAL_LINE_H))
                    .line_height(px(JOURNAL_LINE_H))
                    .child(
                        div()
                            .w(px(JOURNAL_TIME_W_PX))
                            .flex_none()
                            .font_features(tabular_nums())
                            .text_color(palette.on_surface_variant.with_alpha(JOURNAL_TIME_ALPHA).hsla())
                            .child(l.time.clone()),
                    )
                    .child(div().flex_1().min_w(px(0.0)).text_color(c.hsla()).child(l.message.clone()))
            });
            div()
                .id("svc-journal")
                .size_full()
                .overflow_y_scroll()
                .px(px(JOURNAL_PAD_X_PX))
                .py(px(JOURNAL_PAD_Y_PX))
                .font_family(mono)
                .text_size(px(text::LABEL))
                .flex()
                .flex_col()
                .gap(px(JOURNAL_LINE_GAP_PX))
                .children(rows)
                .into_any_element()
        }
        (true, Some(Ok(_))) => quiet_line("note", t("No journal entries for this unit"), palette),
        (true, Some(Err(e))) => quiet_line("alert", e.clone(), palette),
        _ => quiet_line("timer", t("Reading the journal…"), palette),
    };
    drop(mb);
    let (m2, k2) = (m.clone(), key.clone());
    div()
        .flex()
        .flex_col()
        .gap(px(JOURNAL_HEAD_GAP))
        .child(
            div()
                .h(px(JOURNAL_HEAD_H))
                .flex()
                .items_center()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(INLINE_GAP_PX))
                        .child(icon("note", SMALL_ICON_PX, palette.on_surface_variant))
                        .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).child(t("Recent log")))
                        .when(loading, |el| el.child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.with_alpha(UPDATING_ALPHA).hsla()).child(t("updating…")))),
                )
                .child(icon_button("svc-journal-refresh", "refresh", JOURNAL_HEAD_H, palette.on_surface_variant, None, move |_, cx| {
                    load_journal(&m2, k2.clone(), cx);
                    notify(&m2, cx);
                })),
        )
        .child(div().h(px(JOURNAL_H)).rounded(px(radius)).bg(palette.on_surface.with_alpha(alpha(JOURNAL_WELL, palette)).hsla()).child(body))
        .into_any_element()
}

fn quiet_line(glyph: &'static str, msg: impl Into<SharedString>, palette: &MandaraPalette) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .gap(px(tokens::SM))
        .px(px(QUIET_PAD_X_PX))
        .text_size(px(text::BODY_S))
        .text_color(palette.on_surface_variant.hsla())
        .child(icon(glyph, QUIET_ICON_PX, palette.on_surface_variant))
        .child(div().min_w(px(0.0)).child(msg.into()))
        .into_any_element()
}

/// The sheet under a selected unit: actions + journal.
fn detail_sheet(u: &Unit, scope: Scope, m: &Shared, palette: &MandaraPalette, radius: f32, mono: SharedString) -> AnyElement {
    let key: UnitKey = (scope, u.name.clone());
    let busy = m.borrow().busy.get(&key).copied();
    let active = matches!(u.health(), Health::Active | Health::Transitioning);
    let mut actions: Vec<AnyElement> = Vec::new();
    if let Some(action) = busy {
        actions.push(
            div()
                .h(px(ACTIONS_H))
                .px(px(ACTION_PAD_X_PX))
                .rounded_full()
                .flex()
                .items_center()
                .gap(px(INLINE_GAP_PX))
                .bg(palette.on_surface.with_alpha(BUSY_FILL_ALPHA).hsla())
                .text_size(px(BUTTON_TEXT_PX))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(palette.on_surface_variant.hsla())
                .child(icon("timer", SMALL_ICON_PX, palette.on_surface_variant))
                .child(match action {
                    UnitAction::Start => t("Starting…"),
                    UnitAction::Stop => t("Stopping…"),
                    UnitAction::Restart => t("Restarting…"),
                    UnitAction::Enable => t("Enabling…"),
                    UnitAction::Disable => t("Disabling…"),
                })
                .into_any_element(),
        );
    } else {
        if active {
            actions.push(action_button(m, &key, UnitAction::Stop, palette));
            actions.push(action_button(m, &key, UnitAction::Restart, palette));
        } else {
            actions.push(action_button(m, &key, UnitAction::Start, palette));
        }
        match u.file_state.as_deref() {
            Some("enabled") => actions.push(action_button(m, &key, UnitAction::Disable, palette)),
            Some("disabled") => actions.push(action_button(m, &key, UnitAction::Enable, palette)),
            _ => {}
        }
    }
    let hint = match scope {
        Scope::System => Some(div().flex().items_center().gap(px(ACTION_GAP_PX)).child(icon("lock", HINT_ICON_PX, palette.on_surface_variant)).child(t("Asks for your password")).into_any_element()),
        Scope::User => None,
    };
    glass_card(palette)
        .rounded(px(radius))
        .mx(px(SHEET_INSET_PX))
        .mb(px(SHEET_INSET_PX))
        .p(px(SHEET_PAD))
        .flex()
        .flex_col()
        .gap(px(GAP))
        .child(
            div()
                .h(px(ACTIONS_H))
                .flex()
                .items_center()
                .gap(px(tokens::SM))
                .children(actions)
                .child(div().flex_1())
                .when_some(hint, |el, h| el.child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(h))),
        )
        .child(journal_view(m, &key, palette, (radius - NESTED_RADIUS_INSET_PX).max(MIN_NESTED_RADIUS_PX), mono))
        .into_any_element()
}

fn tile(glyph: &'static str, label: &'static str, value: String, detail: impl Into<SharedString>, tint: Color, radius: f32, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(tint, palette);
    glass_card(palette)
        .rounded(px(radius))
        .flex_1()
        .min_w(px(0.0))
        .h(px(TILE_H))
        .px(px(TILE_PAD_X_PX))
        .py(px(TILE_PAD_Y_PX))
        .flex()
        .items_center()
        .gap(px(tokens::MD))
        .child(
            div()
                .size(px(HEADER_CHIP_PX))
                .flex_none()
                .rounded(px(crate::style::radius(TILE_ICON_RADIUS_PX)))
                .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, accent.with_alpha(TILE_ICON_FILL.0), accent.rotate_hue(PARTNER_HUE_DEG).with_alpha(TILE_ICON_FILL.1)))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(glyph, TILE_ICON_PX, accent)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).child(crate::bidi::paragraph(tr(label))))
                .child(div().font_features(tabular_nums()).text_size(px(TILE_VALUE_PX)).line_height(px(TILE_VALUE_LINE_PX)).font_weight(FontWeight::MEDIUM).child(value))
                .child(div().font_features(tabular_nums()).text_size(px(TILE_CAPTION_PX)).text_color(palette.on_surface_variant.hsla()).truncate().child(detail.into())),
        )
        .into_any_element()
}

/// System / User switch with a coral dot on a scope that has failures.
fn scope_toggle(m: &Shared, current: Scope, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let failed_in = |s: Scope| m.borrow().failed.get(&s).copied().unwrap_or(0) > 0;
    let seg = |label: &'static str, which: Scope| {
        let active = current == which;
        let dot = failed_in(which);
        let m = m.clone();
        let hover = palette.on_surface.with_alpha(SEG_HOVER_ALPHA);
        div()
            .id(label)
            .h(px(SCOPE_SEG_H_PX))
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
            .child(crate::bidi::paragraph(tr(label)))
            .when(dot, |el| el.child(div().size(px(DOT_PX)).rounded_full().bg(palette.urgent_coral.hsla())))
            .on_click(move |_, _, cx| {
                {
                    let mut mb = m.borrow_mut();
                    mb.scope = Some(which);
                    mb.selected = None;
                }
                refresh_list(&m, which, false, cx);
                notify(&m, cx);
            })
    };
    div()
        .h(px(SCOPE_TOGGLE_H_PX))
        .flex_none()
        .p(px(SEG_INSET_PX))
        .rounded(px(crate::style::radius(SEGMENTS_RADIUS_PX)))
        .bg(palette.on_surface.with_alpha(alpha(SEGMENTS_FILL, palette)).hsla())
        .flex()
        .items_center()
        .gap(px(SEGMENTS_GAP_PX))
        .child(seg(tk("System"), Scope::System))
        .child(seg(tk("User"), Scope::User))
        .into_any_element()
}

fn filter_segments(m: &Shared, palette: &MandaraPalette) -> AnyElement {
    let (current, all_types) = {
        let mb = m.borrow();
        (mb.filter, mb.all_types)
    };
    let accent = readable(palette.primary, palette);
    let hover = palette.on_surface.with_alpha(SEG_HOVER_ALPHA);
    let seg_h = SEARCH_H - SEG_INSET_PX * 2.0;
    let seg = |label: &'static str, glyph: &'static str, which: Filter| {
        let active = current == which;
        let m = m.clone();
        div()
            .id(label)
            .h(px(seg_h))
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
            .child(icon(glyph, SMALL_ICON_PX, if active { accent } else { palette.on_surface_variant }))
            .child(crate::bidi::paragraph(tr(label)))
            .on_click(move |_, _, cx| {
                m.borrow_mut().filter = which;
                notify(&m, cx);
            })
    };
    let types = {
        let m = m.clone();
        div()
            .id("svc-types")
            .h(px(seg_h))
            .px(px(SEG_PAD_X_PX))
            .rounded(px(crate::style::radius(SEG_RADIUS_PX)))
            .flex()
            .items_center()
            .gap(px(SEG_GAP_PX))
            .cursor_pointer()
            .text_size(px(BUTTON_TEXT_PX))
            .font_weight(if all_types { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
            .when(all_types, |el| el.bg(accent.with_alpha(alpha(SEG_ACTIVE, palette)).hsla()).text_color(accent.hsla()))
            .when(!all_types, |el| el.text_color(palette.on_surface_variant.hsla()).hover(move |s| s.bg(hover.hsla())))
            .child(icon("grid", SMALL_ICON_PX, if all_types { accent } else { palette.on_surface_variant }))
            .child(t("All types"))
            .on_click(move |_, _, cx| {
                {
                    let mut mb = m.borrow_mut();
                    mb.all_types = !mb.all_types;
                    mb.selected = None;
                }
                notify(&m, cx);
            })
    };
    let group = |children: Vec<AnyElement>| {
        div()
            .h(px(SEARCH_H))
            .flex_none()
            .p(px(SEG_INSET_PX))
            .rounded(px(crate::style::radius(SEGMENTS_RADIUS_PX)))
            .bg(palette.on_surface.with_alpha(alpha(SEGMENTS_FILL, palette)).hsla())
            .flex()
            .items_center()
            .gap(px(SEGMENTS_GAP_PX))
            .children(children)
    };
    div()
        .flex()
        .items_center()
        .gap(px(INLINE_GAP_PX))
        .child(group(vec![
            seg(tk("Failed"), "alert", Filter::Failed).into_any_element(),
            seg(tk("Running"), "play", Filter::Running).into_any_element(),
            seg(tk("All"), "list-check", Filter::All).into_any_element(),
        ]))
        .child(group(vec![types.into_any_element()]))
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
            el.child(icon_button("svc-clear", "close", CLEAR_BUTTON_PX, palette.on_surface_variant, None, move |window, cx| {
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

// ── Panel ────────────────────────────────────────────────────────────────

/// How often an open panel checks (without re-rendering) whether the
/// host has closed it.
const CLOSE_POLL: Duration = Duration::from_secs(1);
/// Panel height without the unit list (or its footer).
const FIXED_H: f32 = PAD * 2.0 + HEADER_H + GAP + TILE_H + GAP + SEARCH_H + CONTROLS_GAP;

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
    m: Shared,
    field: Entity<TextField>,
    size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>,
    /// Filtered / sorted rows of the shown list (see [`VisibleCache`]).
    cache: Rc<RefCell<Option<VisibleCache>>>,
    watch: CloseWatch,
    /// Height of the last loaded panel: a reopened panel shows its loading
    /// placeholder at this height, so nothing reflows when units arrive.
    last_h: Cell<f32>,
}

impl View {
    /// Frees panel-only data (lists, journal, row cache) when the last
    /// panel closes; the chip keeps its failed counts.
    fn watch_close(&self, cx: &mut App) {
        let (m, cache) = (self.m.clone(), self.cache.clone());
        let armed = self.watch.arm(
            &self.p,
            move |_| {
                *cache.borrow_mut() = None;
                let mut mb = m.borrow_mut();
                mb.open_panels = mb.open_panels.saturating_sub(1);
                if mb.open_panels == 0 {
                    mb.free_panel_data();
                }
            },
            cx,
        );
        if armed {
            self.m.borrow_mut().open_panels += 1;
        }
    }

    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let m = &self.m;
        m.borrow().views.track(window);
        let scope = m.borrow().scope.unwrap_or_else(|| default_scope(cx));
        if self.p.progress() > 0.0 {
            self.watch_close(cx);
            refresh_list(m, scope, false, cx);
            redraw_in(list_refresh(cx), window, cx);
        }
        let shape = PanelShape::of(cx);
        let card_r = shape.inner(PAD);
        let row_r = (card_r - ROW_RADIUS_INSET_PX).max(MIN_ROW_RADIUS_PX);
        let mono = crate::fonts::fonts(cx).mono;
        let query = self.field.read(cx).value.clone();
        let q = query.trim().to_string();

        // Transient timers (arming, toast).
        let status = {
            let mut mb = m.borrow_mut();
            let toast_for = crate::state::config(cx).read(cx).snapshot().panels.toast();
            if mb.status.as_ref().is_some_and(|(_, _, at)| at.elapsed() >= toast_for) {
                mb.status = None;
            }
            mb.armed.retain(|_, at| at.elapsed() < ARM);
            if !mb.armed.is_empty() {
                redraw_in(ARM_TICK, window, cx);
            }
            mb.status.clone()
        };
        if let Some((_, _, at)) = &status {
            redraw_in(crate::state::config(cx).read(cx).snapshot().panels.toast().saturating_sub(at.elapsed()) + EXPIRY_MARGIN, window, cx);
        }

        // The list is shared (`Rc`): borrowed, never cloned per frame.
        let (listing, generation, filter, all_types, selected) = {
            let mb = m.borrow();
            (mb.lists.get(&scope).cloned(), mb.generation, mb.filter, mb.all_types, mb.selected.clone())
        };

        let units: &[Unit] = match &listing {
            Some(Ok(u)) => u,
            _ => &[],
        };
        // Filtering, lowercasing and sorting hundreds of units runs only
        // when the list, query or filters change — not on every frame.
        let key = (scope, generation, q.clone(), filter, all_types);
        let mut cache = self.cache.borrow_mut();
        if cache.as_ref().is_none_or(|c| c.key != key) {
            *cache = Some(visible_cache(units, key));
        }
        let Some(vis) = cache.as_ref() else { return div().into_any_element() };

        // ── Tiles ────────────────────────────────────────────────────────
        let (running, failed, enabled) = (vis.running, vis.failed, vis.enabled);
        let loaded = matches!(listing, Some(Ok(_)));
        let num = |n: usize| if loaded { n.to_string() } else { "–".into() };
        let tiles = div()
            .flex()
            .gap(px(ROW_INNER_GAP_PX))
            .child(tile("play", tk("Running"), num(running), if loaded { tf("of {total} loaded", &[("total", &vis.typed)]) } else { t("loading…").to_string() }, palette.success_emerald, card_r, palette))
            .child(tile(
                "alert",
                tk("Failed"),
                num(failed),
                if !loaded {
                    t("loading…")
                } else if failed == 0 {
                    t("All clear")
                } else {
                    t("Needs attention")
                },
                if failed > 0 { palette.urgent_coral } else { palette.on_surface_variant },
                card_r,
                palette,
            ))
            .child(tile("check", tk("Enabled"), num(enabled), if loaded { t("start at boot or login") } else { t("loading…") }, palette.primary, card_r, palette));

        // ── Rows ─────────────────────────────────────────────────────────
        let total = vis.rows.len();
        let mut rows: Vec<AnyElement> = Vec::with_capacity(total.min(ROWS) + 1);
        let mut list_h = 0.0;
        let mut has_sheet = false;
        for &i in vis.rows.iter().take(ROWS) {
            let Some(u) = units.get(i) else { continue };
            rows.push(unit_row(u, i, scope, m, palette, row_r));
            list_h += ROW_H + ROW_GAP;
            if selected.as_ref().is_some_and(|(s, n)| *s == scope && *n == u.name) {
                rows.push(detail_sheet(u, scope, m, palette, card_r, mono.clone()));
                list_h += SHEET_H + ROW_GAP + SHEET_MARGIN;
                has_sheet = true;
            }
        }
        drop(cache);
        let empty = rows.is_empty();
        let max_h = if has_sheet { LIST_MAX_H + SHEET_H } else { LIST_MAX_H };
        // While the first list loads, reserve the full list height so the
        // panel doesn't grow (and reflow) when the units arrive.
        // While a list loads, the placeholder keeps the last loaded height.
        let list_h = match (&listing, empty) {
            (None, _) => (self.last_h.get() - FIXED_H).max(EMPTY_H),
            (_, true) => EMPTY_H,
            (_, false) => list_h.min(max_h),
        };
        let more = total.saturating_sub(ROWS);
        let height = FIXED_H + list_h + if more > 0 { FOOTER_H } else { 0.0 };
        if listing.is_some() {
            self.last_h.set(height);
        }
        if (f32::from(self.size_cell.get().height) - height).abs() > SIZE_EPSILON_PX {
            self.size_cell.set(size(px(PANEL_W), px(height)));
        }

        // ── Header ───────────────────────────────────────────────────────
        let accent = readable(palette.primary, palette);
        let subtitle = match scope {
            Scope::System => t("System units · changes ask for your password").to_string(),
            Scope::User => t("Your session's units").to_string(),
        };
        let head = header("server", accent, t("Services"), subtitle, palette, Some(scope_toggle(m, scope, palette)));

        let controls = div().flex().items_center().gap(px(tokens::SM)).child(search_box(&self.field, &query, palette)).child(filter_segments(m, palette));

        let list: AnyElement = if empty {
            let (glyph, title, hint) = match &listing {
                None => ("server", t("Loading units…").to_string(), t("Asking systemd").to_string()),
                Some(Err(e)) => ("alert", t("Can't reach systemd").to_string(), e.clone()),
                Some(Ok(_)) if !q.is_empty() => ("search", tf("No unit matches \u{201c}{query}\u{201d}", &[("query", &q)]), t("Try part of a unit name or its description").to_string()),
                Some(Ok(_)) if filter == Filter::Failed => ("check", t("No failed units").to_string(), t("Everything started cleanly").to_string()),
                Some(Ok(_)) => ("server", t("No units here").to_string(), t("Try All types or another filter").to_string()),
            };
            empty_state(glyph, title, hint, list_h, palette).into_any_element()
        } else {
            let fill = crate::shell::frame_color(palette);
            let clipped = list_h >= max_h - SIZE_EPSILON_PX;
            div()
                .relative()
                .h(px(list_h))
                .child(div().id("svc-list").size_full().overflow_y_scroll().flex().flex_col().gap(px(ROW_GAP)).children(rows))
                .when(clipped, |el| el.child(div().absolute().left_0().right_0().bottom_0().h(px(LIST_FADE_H_PX)).bg(oklab_gradient(ANGLE_DOWN_DEG, fill.with_alpha(0.0), fill))))
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
            .child(reveal(p, 1, tiles))
            .child(div().h(px(GAP)))
            .child(reveal(p, 2, controls))
            .child(div().h(px(CONTROLS_GAP)))
            .child(reveal(
                p,
                3,
                div().flex().flex_col().child(list).when(more > 0, |el| {
                    el.child(
                        div()
                            .h(px(FOOTER_H))
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

fn chip(m: &Shared, palette: &MandaraPalette, window: &Window, cx: &App) -> AnyElement {
    m.borrow().views.track(window);
    let failed = if alert_enabled(cx) { m.borrow().failed_total() } else { 0 };
    let coral = palette.urgent_coral;
    div()
        .flex()
        .items_center()
        .gap(px(CHIP_GAP_PX))
        .child(icon("server", crate::style::bar_icon(CHIP_ICON_PX), if failed > 0 { readable(coral, palette) } else { palette.on_surface }))
        .when(failed > 0, |el| {
            el.child(
                div()
                    .min_w(px(CHIP_BADGE_PX))
                    .h(px(CHIP_BADGE_PX))
                    .px(px(CHIP_BADGE_PAD_X_PX))
                    .rounded_full()
                    .bg(coral.hsla())
                    .text_color(ink_on(coral).hsla())
                    .font_features(tabular_nums())
                    .text_size(px(BADGE_TEXT_PX))
                    .font_weight(FontWeight::BOLD)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(if failed > BADGE_MAX { format!("{BADGE_MAX}+") } else { failed.to_string() }),
            )
        })
        .into_any_element()
}

// ── Commands ─────────────────────────────────────────────────────────────

/// Runs `f` on its own thread; the CLI receives its JSON (pretty-printed).
fn pending(f: impl FnOnce() -> Result<serde_json::Value, String> + Send + 'static) -> Output {
    let (tx, rx) = async_channel::bounded(1);
    let _ = std::thread::Builder::new().name("mandara-systemd-cmd".into()).spawn(move || {
        let _ = tx.send_blocking(f().map(|v| serde_json::to_string_pretty(&v).unwrap_or_default()));
    });
    Output::Pending(rx)
}

/// `--user` anywhere selects the user manager; returns the other words.
fn split_scope(words: &[String]) -> (Scope, Vec<String>) {
    let user = words.iter().any(|w| w == "--user");
    (if user { Scope::User } else { Scope::System }, words.iter().filter(|w| *w != "--user" && *w != "--system").cloned().collect())
}

fn action_command(action: UnitAction) -> (&'static str, &'static str, &'static str) {
    match action {
        UnitAction::Start => ("services start", tk("Start a unit (system units ask for authorization)"), "services start syncthing --user"),
        UnitAction::Stop => ("services stop", tk("Stop a unit"), "services stop bluetooth"),
        UnitAction::Restart => ("services restart", tk("Restart a unit"), "services restart NetworkManager"),
        UnitAction::Enable => ("services enable", tk("Enable a unit at boot / login and reload systemd"), "services enable sshd"),
        UnitAction::Disable => ("services disable", tk("Disable a unit and reload systemd"), "services disable sshd"),
    }
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Services"), tk("systemd units: failed alerts, start, stop, enable and journal"), None), |window, cx, panel_cx| {
        let m = model(cx);
        // Typing re-renders this plugin's views only.
        let views = m.borrow().views.clone();
        let field = cx.new(|cx| TextField::new(String::new(), t("Search units…"), true, Rc::new(move |_, _, cx| views.notify(cx)), cx));
        let bar_model = m.clone();
        let bar = fn_view(None, move |palette, window, cx| chip(&bar_model, palette, window, cx), window, cx);
        // The first open reserves the loading height (see View::render).
        let initial_h = FIXED_H + LIST_MAX_H;
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(initial_h))));
        let view = View { p: panel_cx.clone(), m, field, size_cell: size_cell.clone(), cache: Rc::default(), watch: CloseWatch::default(), last_h: Cell::new(initial_h) };
        let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), ..PluginInstance::empty() }
    });
    r.command(
        ID,
        CommandSpec::new_output("services list", tk("Loaded units of the system or user manager, optionally filtered by name"), |a, _| {
            let mut words = a.0.clone();
            let scope = words.first().and_then(|w| Scope::parse(w)).inspect(|_| {
                words.remove(0);
            });
            let scope = scope.unwrap_or(Scope::System);
            let query = words.join(" ");
            Ok(pending(move || {
                let units = systemd::list_units(scope)?;
                let shown: Vec<&Unit> = visible(&units, &query, Filter::All, true);
                Ok(json!({ "scope": scope, "count": shown.len(), "units": shown }))
            }))
        })
        .arg(ArgSpec::optional("scope", ArgKind::String, tk("system (default) or user")).choices(&["system", "user"]))
        .arg(ArgSpec::optional("query", ArgKind::String, tk("part of a unit name or description")))
        .example("services list user")
        .example("services list system ssh"),
    );
    r.command(
        ID,
        CommandSpec::new_output("services failed", tk("Failed units on the system and user managers"), |_, _| {
            Ok(pending(|| {
                let side = |s: Scope| match systemd::failed_units(s) {
                    Ok(u) => json!(u),
                    Err(e) => json!({ "error": e }),
                };
                Ok(json!({ "system": side(Scope::System), "user": side(Scope::User) }))
            }))
        })
        .example("services failed"),
    );
    r.command(
        ID,
        CommandSpec::new_output("services status", tk("State and unit-file state of one unit"), |a, _| {
            let (scope, words) = split_scope(&a.0);
            let unit = words.first().cloned().ok_or_else(|| t("expected a unit name").to_string())?;
            Ok(pending(move || systemd::unit_status(scope, &unit).map(|u| json!({ "scope": scope, "unit": u }))))
        })
        .arg(ArgSpec::required("unit", ArgKind::String, tk("unit name (.service may be omitted)")))
        .arg(ArgSpec::optional("user", ArgKind::String, tk("use the user manager")).choices(&["--user"]))
        .example("services status sshd"),
    );
    r.command(
        ID,
        CommandSpec::new_output("services logs", tk("Recent journal lines of a unit"), |a, cx| {
            let (scope, words) = split_scope(&a.0);
            let unit = words.first().cloned().ok_or_else(|| t("expected a unit name").to_string())?;
            let lines = match words.get(1) {
                Some(n) => n.parse::<usize>().map_err(|_| t("expected a number of lines").to_string())?,
                None => journal_lines(cx),
            };
            Ok(pending(move || systemd::journal(scope, &unit, lines).map(|l| json!(l))))
        })
        .arg(ArgSpec::required("unit", ArgKind::String, tk("unit name")))
        .arg(ArgSpec::optional("lines", ArgKind::Integer, tk("how many lines (default from Studio)")))
        .arg(ArgSpec::optional("user", ArgKind::String, tk("use the user manager")).choices(&["--user"]))
        .example("services logs NetworkManager 20"),
    );
    for action in UnitAction::ALL {
        let (path, summary, example) = action_command(action);
        r.command(
            ID,
            CommandSpec::new_output(path, summary, move |a, _| {
                let (scope, words) = split_scope(&a.0);
                let unit = words.first().cloned().ok_or_else(|| t("expected a unit name").to_string())?;
                Ok(pending(move || systemd::act(action, scope, &unit).map(|unit| json!({ "unit": unit, "scope": scope, "action": action }))))
            })
            .arg(ArgSpec::required("unit", ArgKind::String, tk("unit name (.service may be omitted)")))
            .arg(ArgSpec::optional("user", ArgKind::String, tk("use the user manager")).choices(&["--user"]))
            .example(example),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit(name: &str, desc: &str, active: &str) -> Unit {
        Unit { name: name.into(), description: desc.into(), load_state: "loaded".into(), active_state: active.into(), sub_state: String::new(), file_state: None }
    }

    #[test]
    fn filters_and_orders_failed_first() {
        let units = vec![
            unit("b.service", "Beta", "active"),
            unit("a.service", "Alpha", "inactive"),
            unit("z.service", "Zed", "failed"),
            unit("t.timer", "Daily", "active"),
            unit("c.service", "Gamma", "activating"),
        ];
        let names = |v: Vec<&Unit>| v.into_iter().map(|u| u.name.clone()).collect::<Vec<_>>();
        assert_eq!(names(visible(&units, "", Filter::All, false)), ["z.service", "c.service", "b.service", "a.service"]);
        assert_eq!(names(visible(&units, "", Filter::Running, true)), ["b.service", "t.timer"]);
        assert_eq!(names(visible(&units, "", Filter::Failed, false)), ["z.service"]);
        assert_eq!(names(visible(&units, "alph", Filter::All, false)), ["a.service"]);
        assert_eq!(names(visible(&units, "DAILY", Filter::All, true)), ["t.timer"]);
    }

    #[test]
    fn scope_flags_are_split_out() {
        let (s, rest) = split_scope(&["sshd".to_string(), "--user".to_string()]);
        assert_eq!((s, rest), (Scope::User, vec!["sshd".to_string()]));
        assert_eq!(split_scope(&["x".to_string()]).0, Scope::System);
    }

    #[test]
    fn every_action_has_a_command() {
        let paths: HashSet<&str> = UnitAction::ALL.into_iter().map(|a| action_command(a).0).collect();
        assert_eq!(paths.len(), UnitAction::ALL.len());
        assert!(paths.iter().all(|p| p.starts_with("services ")));
    }

    #[test]
    fn model_counts_failures_from_lists() {
        let mut m = Model::new();
        m.absorb_list(Scope::System, Ok(vec![unit("a.service", "", "failed"), unit("b.service", "", "active")]));
        m.absorb_list(Scope::User, Ok(vec![unit("c.service", "", "failed")]));
        assert_eq!(m.failed_total(), 2);
        // An error keeps the last known count.
        m.absorb_list(Scope::User, Err("down".into()));
        assert_eq!(m.failed_total(), 2);
    }

    #[test]
    fn ink_contrasts_with_fill() {
        assert!(ink_on(Color::oklch(0.8, 0.12, 30.0)).lightness() < 0.4);
        assert!(ink_on(Color::oklch(0.5, 0.15, 30.0)).lightness() > 0.9);
    }
}
