//! Crash reports: apps that crashed in the last 7 days, from
//! systemd-coredump. The bar chip is a bug glyph with an urgent-coral count
//! of crashes you haven't seen, hidden when nothing is new. The panel sums
//! up the week (how many, which app crashed most, the latest) and lists
//! each crash with the app, its signal (SIGSEGV, SIGABRT…), when, the PID
//! and whether a core file is stored; clicking a row reveals Copy report
//! (`coredumpctl info`) and Debug in a terminal (`coredumpctl debug`).
//! "Mark all as seen" clears the badge. A desktop notification announces
//! each new crash.
//!
//! Data comes from `coredumpctl --json=short list --since=-7d`
//! ([`mandara_services::crashes`]) on a background thread, every 60 s while
//! the chip is on screen and when the panel opens; nothing runs off screen.
//! The "seen up to" mark lives in `~/.local/state/mandara/crashes.json`.
//! Settings (`[plugins.settings.crashes]`): `notify`. Commands:
//! `crashes list`, `crashes info <pid>`, `crashes seen`.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Task, Window, div,
    prelude::FluentBuilder as _, px, size,
};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::crashes::{self as svc, Crash, Listing};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::extras::{Flash, toast};
use super::model_view as fn_view;
use super::panel_fx::{ANGLE_DOWN_DEG, HEADER_H, PanelShape, ViewSet, alpha, empty_state, glow, header, pill_button, readable, rim};
use crate::fonts::{fonts, tabular_nums};
use crate::icons::icon;
use crate::kit::{reveal, text, tokens};

pub const ID: &str = "crashes";

// ── Layout ───────────────────────────────────────────────────────────────

const PANEL_W: f32 = 500.0;
const PAD: f32 = 14.0;
const GAP: f32 = tokens::MD;
/// Summary hero.
const HERO_H: f32 = 84.0;
const HERO_PAD_X_PX: f32 = 18.0;
const HERO_GAP_PX: f32 = 16.0;
const HERO_VALUE_PX: f32 = 34.0;
const HERO_VALUE_LINE_PX: f32 = 38.0;
const HERO_LINE_GAP_PX: f32 = 3.0;
const HERO_ANGLE_DEG: f32 = 125.0;
const HERO_DARK_L: (f32, f32) = (0.27, 0.21);
const HERO_DARK_CHROMA: ((f32, f32), (f32, f32)) = ((0.55, 0.07), (0.5, 0.055));
const HERO_LIGHT_L: (f32, f32) = (0.935, 0.955);
const HERO_LIGHT_CHROMA: ((f32, f32), (f32, f32)) = ((0.35, 0.045), (0.3, 0.035));
/// Rows.
const ROW_H: f32 = 56.0;
const ROW_GAP: f32 = 4.0;
const ACTIONS_H: f32 = 60.0;
const ROW_PAD_X_PX: f32 = 10.0;
const ROW_INNER_GAP_PX: f32 = 12.0;
const ROW_HOVER: (f32, f32) = (0.05, 0.04);
const ROW_SELECTED: (f32, f32) = (0.09, 0.07);
const ROW_RADIUS_INSET_PX: f32 = 2.0;
const ROW_MIN_RADIUS_PX: f32 = 10.0;
const LINE_GAP_PX: f32 = 2.0;
const INLINE_GAP_PX: f32 = 6.0;
const MUTED_ALPHA: f32 = 0.85;
/// App avatar and its "new" dot.
const AVATAR_PX: f32 = 34.0;
const AVATAR_GLYPH_PX: f32 = 22.0;
const AVATAR_ICON_PX: f32 = 16.0;
const AVATAR_RADIUS_FRAC: f32 = 0.32;
const AVATAR_FILL: (f32, f32) = (0.06, 0.045);
const NEW_DOT_PX: f32 = 10.0;
const NEW_DOT_OFFSET_PX: f32 = -2.0;
/// Signal badge.
const BADGE_H_PX: f32 = 16.0;
const BADGE_PAD_X_PX: f32 = 6.0;
const BADGE_TEXT_PX: f32 = 10.0;
const BADGE_FILL: (f32, f32) = (0.14, 0.1);
/// Action strip.
const ACTIONS_PAD_PX: f32 = 8.0;
const ACTIONS_GAP_PX: f32 = 6.0;
const PATH_TEXT_PX: f32 = 10.5;
/// Rows visible before the list scrolls.
const VISIBLE_ROWS: f32 = 6.5;
const LIST_MAX_H: f32 = VISIBLE_ROWS * (ROW_H + ROW_GAP);
const LIST_FADE_PX: f32 = 22.0;
const EMPTY_H: f32 = 180.0;
/// Bar chip.
const CHIP_ICON_PX: f32 = 15.0;
const CHIP_GAP_PX: f32 = 4.0;
const CHIP_BADGE_PX: f32 = 16.0;
const CHIP_BADGE_PAD_X_PX: f32 = 4.0;
const CHIP_BADGE_TEXT_PX: f32 = 10.0;
const CHIP_GLOW: (f32, f32) = (0.45, 6.0);
const BADGE_MAX: usize = 99;
const SIZE_EPSILON_PX: f32 = 0.5;
/// `ink_on`: fills lighter than this get dark ink.
const INK_SWITCH_L: f32 = 0.64;
const DARK_INK: (f32, f32) = (0.2, 0.02);
const LIGHT_INK_L: f32 = 0.99;

// ── Timing ───────────────────────────────────────────────────────────────

/// Poll cadence while the chip is on screen.
const POLL_EVERY: Duration = Duration::from_secs(60);
/// An opened panel refreshes data older than this.
const PANEL_FRESH: Duration = Duration::from_secs(10);
/// The poller stops once no view rendered for this long past a cycle.
const IDLE_GRACE: Duration = Duration::from_secs(5);
/// Exe names whose app-icon lookup is remembered (bounded).
const ICON_CACHE_CAP: usize = 256;

fn notify_enabled(cx: &App) -> bool {
    crate::state::config(cx).read(cx).snapshot().plugins.setting(ID, "notify", true)
}

// ── Model ────────────────────────────────────────────────────────────────

/// What the last read found.
#[derive(Clone, Debug, PartialEq)]
enum Source {
    Loading,
    Ready,
    /// coredumpctl isn't installed.
    Missing,
    Failed(String),
}

/// A crash's labels, formatted once per poll.
struct Row {
    name: SharedString,
    signal: SharedString,
    detail: SharedString,
    exe: SharedString,
}

struct Model {
    crashes: Vec<Crash>,
    rows: Vec<Row>,
    source: Source,
    /// systemd-coredump receives the kernel's crashes.
    collecting: bool,
    /// Crashes at or before this time (µs) are seen (`None` until read).
    seen_up_to: Option<u64>,
    /// The newest crash already known (no notification for it again).
    announced: Option<u64>,
    /// Exe name → it has an app icon (looked up off the UI thread).
    icons: HashMap<String, bool>,
    polled_at: Option<Instant>,
    polling: bool,
    /// The row whose actions are revealed: `(time, pid)`.
    selected: Option<(u64, u32)>,
    copying: HashSet<u32>,
    flash: Flash,
    opened: bool,
    seen: Option<Instant>,
    poller: Option<Task<()>>,
    close_watch: Rc<Cell<bool>>,
    views: ViewSet,
}

impl Model {
    fn new() -> Self {
        Self {
            crashes: Vec::new(),
            rows: Vec::new(),
            source: Source::Loading,
            collecting: true,
            seen_up_to: None,
            announced: None,
            icons: HashMap::new(),
            polled_at: None,
            polling: false,
            selected: None,
            copying: HashSet::new(),
            flash: Flash::default(),
            opened: false,
            seen: None,
            poller: None,
            close_watch: Rc::default(),
            views: ViewSet::default(),
        }
    }

    fn unseen(&self) -> usize {
        self.seen_up_to.map_or(0, |s| svc::unseen(&self.crashes, s))
    }

    fn is_new(&self, c: &Crash) -> bool {
        self.seen_up_to.is_some_and(|s| c.time > s)
    }

    /// Folds a read in; returns the crashes that appeared since the last
    /// one (none on the first read, which only sets the baseline).
    fn absorb(&mut self, listing: Listing, collecting: bool, seen: Option<u64>) -> Vec<Crash> {
        if self.seen_up_to.is_none() {
            self.seen_up_to = seen;
        }
        self.collecting = collecting;
        self.polled_at = Some(Instant::now());
        self.polling = false;
        let crashes = match listing {
            Listing::Crashes { crashes } => {
                self.source = Source::Ready;
                crashes
            }
            Listing::Missing => {
                self.source = Source::Missing;
                self.crashes = Vec::new();
                self.rows = Vec::new();
                self.selected = None;
                return Vec::new();
            }
            Listing::Failed { error } => {
                self.source = Source::Failed(error);
                return Vec::new();
            }
        };
        let newest = crashes.iter().map(|c| c.time).max();
        let fresh: Vec<Crash> = match self.announced {
            Some(a) => crashes.iter().filter(|c| c.time > a && self.seen_up_to.is_none_or(|s| c.time > s)).cloned().collect(),
            None => Vec::new(),
        };
        self.announced = self.announced.max(newest).or(Some(0));
        self.rows = crashes.iter().map(row_labels).collect();
        if self.selected.is_some_and(|(t, p)| !crashes.iter().any(|c| c.time == t && c.pid == p)) {
            self.selected = None;
        }
        self.crashes = crashes;
        fresh
    }

    /// Marks everything up to now as seen; returns the mark to save.
    fn mark_seen(&mut self) -> u64 {
        let up_to = self.crashes.iter().map(|c| c.time).max().unwrap_or(0).max(now_micros());
        self.seen_up_to = Some(up_to);
        up_to
    }
}

fn now_micros() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_micros() as u64).unwrap_or(0)
}

/// A plain-words name for the common crash signals.
fn signal_meaning(sig: i32) -> Option<&'static str> {
    Some(match sig {
        11 => t("Segmentation fault"),
        6 => t("Aborted"),
        7 => t("Bus error"),
        8 => t("Arithmetic error"),
        4 => t("Illegal instruction"),
        5 => t("Trace trap"),
        31 => t("Bad system call"),
        3 => t("Quit"),
        _ => return None,
    })
}

fn row_labels(c: &Crash) -> Row {
    let core = if c.has_core() { t("core stored") } else { t("no core") };
    let mut parts: Vec<String> = Vec::new();
    if let Some(m) = signal_meaning(c.sig) {
        parts.push(m.into());
    }
    parts.push(tf("PID {pid}", &[("pid", &c.pid)]));
    parts.push(super::notifications::ago(c.unix()));
    parts.push(core.into());
    Row { name: c.name().to_string().into(), signal: c.signal().into(), detail: parts.join(" · ").into(), exe: c.exe.clone().into() }
}

type Shared = Rc<RefCell<Model>>;

thread_local! {
    static MODEL: RefCell<Weak<RefCell<Model>>> = const { RefCell::new(Weak::new()) };
}

fn model() -> Shared {
    if let Some(m) = MODEL.with(|w| w.borrow().upgrade()) {
        return m;
    }
    let m = Rc::new(RefCell::new(Model::new()));
    MODEL.with(|w| *w.borrow_mut() = Rc::downgrade(&m));
    m
}

/// Runs blocking work on its own thread (coredumpctl reads the journal,
/// which can take a moment).
fn off_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> async_channel::Receiver<T> {
    let (tx, rx) = async_channel::bounded(1);
    let _ = std::thread::Builder::new().name("mandara-crashes".into()).spawn(move || {
        let _ = tx.send_blocking(f());
    });
    rx
}

/// Announces new crashes (one notification, or a summary for several).
fn announce(fresh: &[Crash], cx: &mut App) {
    let (summary, body) = match fresh {
        [] => return,
        [c] => (tf("{app} crashed", &[("app", &c.name())]), format!("{} · {}", c.signal(), tf("PID {pid}", &[("pid", &c.pid)]))),
        many => {
            let mut names: Vec<&str> = many.iter().map(Crash::name).collect();
            names.dedup();
            (tf("{count} apps crashed", &[("count", &many.len())]), names.join(", "))
        }
    };
    super::notifications::send(cx, mandara_services::NotificationCommand::Post { app: t("Crash reports").into(), icon: "bug".into(), summary, body });
}

/// Reads the crash list off the UI thread when due (or `force`d).
fn poll(m: &Shared, force: bool, cx: &mut App) {
    let (need_seen, known) = {
        let mut mb = m.borrow_mut();
        let due = mb.polled_at.is_none_or(|t| t.elapsed() + IDLE_GRACE >= POLL_EVERY);
        if mb.polling || !(force || due) {
            return;
        }
        mb.polling = true;
        if mb.icons.len() > ICON_CACHE_CAP {
            mb.icons.clear();
        }
        (mb.seen_up_to.is_none(), mb.icons.keys().cloned().collect::<HashSet<String>>())
    };
    let notify = notify_enabled(cx);
    let m = m.clone();
    cx.spawn(async move |cx| {
        let rx = off_thread(move || {
            let listing = svc::list();
            let seen = need_seen.then(svc::load_seen);
            let icons: Vec<(String, bool)> = match &listing {
                Listing::Crashes { crashes } => {
                    let names: HashSet<&str> = crashes.iter().map(Crash::name).filter(|n| !known.contains(*n)).collect();
                    names.into_iter().map(|n| (n.to_string(), crate::apps::icon_for_app_id(n, false).is_some())).collect()
                }
                _ => Vec::new(),
            };
            (listing, svc::collecting(), seen, icons)
        });
        let Ok((listing, collecting, seen, icons)) = rx.recv().await else {
            m.borrow_mut().polling = false;
            return;
        };
        cx.update(|cx| {
            let (fresh, views) = {
                let mut mb = m.borrow_mut();
                mb.icons.extend(icons);
                (mb.absorb(listing, collecting, seen), mb.views.clone())
            };
            if notify {
                announce(&fresh, cx);
            }
            views.notify(cx);
        });
    })
    .detach();
}

/// Marks a view as on screen and keeps the 60 s poll running while any is.
fn keep_alive(m: &Shared, window: &Window, cx: &mut App) {
    let mut mb = m.borrow_mut();
    mb.views.track(window);
    mb.seen = Some(Instant::now());
    if mb.poller.is_some() {
        return;
    }
    let weak = Rc::downgrade(m);
    mb.poller = Some(cx.spawn(async move |cx| {
        let mut slept = Duration::ZERO;
        loop {
            let Some(m) = weak.upgrade() else { return };
            // On screen = rendered since this poller went to sleep (each
            // wake notifies the views, so a visible chip always has).
            if m.borrow().seen.is_none_or(|t| t.elapsed() > slept + IDLE_GRACE) {
                break;
            }
            cx.update(|cx| {
                poll(&m, false, cx);
                m.borrow().views.notify(cx);
            });
            drop(m);
            cx.background_executor().timer(POLL_EVERY).await;
            slept = POLL_EVERY;
        }
        if let Some(m) = weak.upgrade() {
            m.borrow_mut().poller = None;
        }
    }));
}

/// Marks every crash as seen and saves the mark off the UI thread.
fn mark_all_seen(m: &Shared, cx: &mut App) -> u64 {
    let (up_to, views) = {
        let mut mb = m.borrow_mut();
        (mb.mark_seen(), mb.views.clone())
    };
    let _ = off_thread(move || {
        if let Err(e) = svc::save_seen(up_to) {
            tracing::warn!("crashes: saving the seen mark: {e}");
        }
    });
    views.notify(cx);
    up_to
}

// ── Actions ──────────────────────────────────────────────────────────────

fn copy_report(m: &Shared, pid: u32, cx: &mut App) {
    if !m.borrow_mut().copying.insert(pid) {
        return;
    }
    m.borrow().views.notify(cx);
    let m = m.clone();
    cx.spawn(async move |cx| {
        let rx = off_thread(move || svc::info(pid).and_then(|report| mandara_services::clipboard::serve(report.into_bytes(), None)));
        let result = rx.recv().await.unwrap_or_else(|_| Err(t("The service did not reply").into()));
        let views = {
            let mut mb = m.borrow_mut();
            mb.copying.remove(&pid);
            match result {
                Ok(()) => mb.flash.show(true, tf("Copied the report of PID {pid}", &[("pid", &pid)])),
                Err(e) => mb.flash.show(false, e),
            }
            mb.views.clone()
        };
        views.notify_async(cx);
    })
    .detach();
}

fn debug_in_terminal(m: &Shared, pid: u32, cx: &mut App) {
    let term = crate::state::config(cx).read(cx).get().apps.terminal_command().unwrap_or_default();
    let result = match mandara_services::updates::resolve_terminal(&term) {
        None => Err(t("No terminal found — choose one in Studio → Applications").to_string()),
        Some(_) => mandara_services::updates::open_terminal(&term, &svc::debug_command(pid)),
    };
    if let Err(e) = result {
        m.borrow().flash.show(false, e);
        m.borrow().views.notify(cx);
    }
}

// ── Pieces ───────────────────────────────────────────────────────────────

fn ink_on(bg: Color) -> Color {
    if bg.lightness() > INK_SWITCH_L { Color::oklch(DARK_INK.0, DARK_INK.1, bg.hue()) } else { Color::oklch(LIGHT_INK_L, 0.0, 0.0) }
}

fn signal_color(sig: i32, palette: &MandaraPalette) -> Color {
    readable(if matches!(sig, 11 | 6 | 7 | 4 | 8) { palette.urgent_coral } else { palette.warning_amber }, palette)
}

fn crash_id(prefix: &'static str, c: &Crash) -> ElementId {
    ElementId::NamedInteger(SharedString::new_static(prefix), c.time ^ (u64::from(c.pid) << 1))
}

fn actions(c: &Crash, row: &Row, m: &Shared, palette: &MandaraPalette, cx: &App) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let muted = palette.on_surface_variant;
    let copying = m.borrow().copying.contains(&c.pid);
    let pid = c.pid;
    let copy: AnyElement = if copying {
        pill_button(crash_id("crash-copying", c), Some("timer"), t("Copying…"), muted, palette, |_, _| {}).into_any_element()
    } else {
        let m = m.clone();
        pill_button(crash_id("crash-copy", c), Some("copy"), t("Copy report"), accent, palette, move |_, cx| copy_report(&m, pid, cx)).into_any_element()
    };
    let debug: AnyElement = if c.has_core() {
        let m = m.clone();
        pill_button(crash_id("crash-debug", c), Some("terminal"), t("Debug in terminal"), accent, palette, move |_, cx| debug_in_terminal(&m, pid, cx)).into_any_element()
    } else {
        div().text_size(px(text::LABEL)).text_color(muted.hsla()).child(t("No core file to debug")).into_any_element()
    };
    div()
        .h(px(ACTIONS_H))
        .px(px(ACTIONS_PAD_PX))
        .flex()
        .flex_col()
        .justify_center()
        .gap(px(ACTIONS_GAP_PX))
        .child(div().flex().items_center().gap(px(ACTIONS_GAP_PX)).child(copy).child(debug))
        .child(div().truncate().font_family(fonts(cx).mono).text_size(px(PATH_TEXT_PX)).text_color(muted.with_alpha(MUTED_ALPHA).hsla()).child(row.exe.clone()))
        .into_any_element()
}

fn crash_row(i: usize, m: &Shared, palette: &MandaraPalette, radius: f32, cx: &App) -> AnyElement {
    let mb = m.borrow();
    let (c, row) = (&mb.crashes[i], &mb.rows[i]);
    let selected = mb.selected == Some((c.time, c.pid));
    let new = mb.is_new(c);
    let has_icon = mb.icons.get(c.name()).copied().unwrap_or(false);
    let muted = palette.on_surface_variant;
    let coral = readable(palette.urgent_coral, palette);
    let hover = palette.on_surface.with_alpha(alpha(ROW_HOVER, palette));
    let sel = readable(palette.primary, palette).with_alpha(alpha(ROW_SELECTED, palette));
    let tint = signal_color(c.sig, palette);
    let glyph: AnyElement = if has_icon { super::workspaces::app_icon(c.name(), AVATAR_GLYPH_PX, muted, false, palette) } else { icon("bug", AVATAR_ICON_PX, muted).into_any_element() };
    let ring = crate::shell::frame_color(palette);
    let avatar = div()
        .relative()
        .size(px(AVATAR_PX))
        .flex_none()
        .rounded(px(crate::style::radius(AVATAR_PX * AVATAR_RADIUS_FRAC)))
        .bg(palette.on_surface.with_alpha(alpha(AVATAR_FILL, palette)).hsla())
        .flex()
        .items_center()
        .justify_center()
        .child(glyph)
        .when(new, |el| {
            el.child(div().absolute().right(px(NEW_DOT_OFFSET_PX)).top(px(NEW_DOT_OFFSET_PX)).size(px(NEW_DOT_PX)).rounded_full().border_2().border_color(ring.hsla()).bg(coral.hsla()))
        });
    let key = (c.time, c.pid);
    let m2 = m.clone();
    let head = div()
        .id(crash_id("crash-row", c))
        .h(px(ROW_H))
        .flex_none()
        .px(px(ROW_PAD_X_PX))
        .flex()
        .items_center()
        .gap(px(ROW_INNER_GAP_PX))
        .cursor_pointer()
        .when(!selected, |el| el.rounded(px(radius)).hover(move |s| s.bg(hover.hsla())))
        .child(avatar)
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
                        .child(div().min_w(px(0.0)).truncate().text_size(px(text::BODY)).font_weight(if new { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM }).child(row.name.clone()))
                        .child(
                            div()
                                .flex_none()
                                .h(px(BADGE_H_PX))
                                .px(px(BADGE_PAD_X_PX))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .bg(tint.with_alpha(alpha(BADGE_FILL, palette)).hsla())
                                .text_size(px(BADGE_TEXT_PX))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(tint.hsla())
                                .child(row.signal.clone()),
                        ),
                )
                .child(div().min_w(px(0.0)).truncate().font_features(tabular_nums()).text_size(px(text::LABEL)).text_color(muted.hsla()).child(row.detail.clone())),
        )
        .on_click(move |_, _, cx| {
            let views = {
                let mut mb = m2.borrow_mut();
                mb.selected = if mb.selected == Some(key) { None } else { Some(key) };
                mb.views.clone()
            };
            views.notify(cx);
        });
    if !selected {
        return head.into_any_element();
    }
    div().flex_none().rounded(px(radius)).bg(sel.hsla()).flex().flex_col().child(head).child(actions(c, row, m, palette, cx)).into_any_element()
}

/// The week in numbers: crash count, the app that crashed most, the latest.
fn hero(mb: &Model, palette: &MandaraPalette, radius: f32) -> AnyElement {
    let tone = |c: Color, l: f32, (share, cap): (f32, f32)| c.with_lightness(l).with_chroma((c.chroma() * share).min(cap));
    let (a, b) = (palette.urgent_coral, palette.tertiary);
    let (bg_a, bg_b) = if palette.is_dark {
        (tone(a, HERO_DARK_L.0, HERO_DARK_CHROMA.0), tone(b, HERO_DARK_L.1, HERO_DARK_CHROMA.1))
    } else {
        (tone(a, HERO_LIGHT_L.0, HERO_LIGHT_CHROMA.0), tone(b, HERO_LIGHT_L.1, HERO_LIGHT_CHROMA.1))
    };
    let mut counts: HashMap<&str, usize> = HashMap::new();
    for c in &mb.crashes {
        *counts.entry(c.name()).or_default() += 1;
    }
    let top = counts.into_iter().max_by(|x, y| x.1.cmp(&y.1).then(y.0.cmp(x.0)));
    let most = top.map(|(name, n)| tf("Most often: {app} ×{count}", &[("app", &name), ("count", &n)])).unwrap_or_default();
    let last = mb.crashes.first().map(|c| tf("Latest: {app}, {when}", &[("app", &c.name()), ("when", &super::notifications::ago(c.unix()))])).unwrap_or_default();
    let unseen = mb.unseen();
    div()
        .h(px(HERO_H))
        .rounded(px(radius))
        .bg(oklab_gradient(HERO_ANGLE_DEG, bg_a, bg_b))
        .shadow(rim(palette))
        .px(px(HERO_PAD_X_PX))
        .flex()
        .items_center()
        .gap(px(HERO_GAP_PX))
        .child(
            div()
                .flex_none()
                .font_features(tabular_nums())
                .text_size(px(HERO_VALUE_PX))
                .line_height(px(HERO_VALUE_LINE_PX))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if unseen > 0 { readable(palette.urgent_coral, palette).hsla() } else { palette.on_surface.hsla() })
                .child(mb.crashes.len().to_string()),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(HERO_LINE_GAP_PX))
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(match (mb.crashes.len(), unseen) {
                    (1, 0) => t("crash in the last 7 days").to_string(),
                    (1, _) => t("crash in the last 7 days · new").to_string(),
                    (_, 0) => t("crashes in the last 7 days").to_string(),
                    (_, n) => tf("crashes in the last 7 days · {count} new", &[("count", &n)]),
                }))
                .child(div().truncate().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(most))
                .child(div().truncate().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(last)),
        )
        .into_any_element()
}

// ── Panel ────────────────────────────────────────────────────────────────

struct View {
    p: PanelCx,
    m: Shared,
    size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>,
}

impl View {
    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let m = &self.m;
        keep_alive(m, window, cx);
        if self.p.progress() > 0.0 {
            let stale = m.borrow().polled_at.is_none_or(|t| t.elapsed() > PANEL_FRESH);
            if !std::mem::replace(&mut m.borrow_mut().opened, true) && stale {
                poll(m, true, cx);
            }
            let watch = m.borrow().close_watch.clone();
            if !watch.get() {
                let closed = m.clone();
                super::processes::on_panel_close(&self.p, &watch, cx, move |_| {
                    let mut mb = closed.borrow_mut();
                    mb.opened = false;
                    mb.selected = None;
                });
            }
        }
        let flash = m.borrow().flash.current(window, cx);
        let shape = PanelShape::of(cx);
        let card_r = shape.inner(PAD);
        let row_r = (card_r - ROW_RADIUS_INSET_PX).max(ROW_MIN_RADIUS_PX);
        let (count, unseen, source, collecting, selected) = {
            let mb = m.borrow();
            (mb.crashes.len(), mb.unseen(), mb.source.clone(), mb.collecting, mb.selected.is_some())
        };
        let accent = readable(if unseen > 0 { palette.urgent_coral } else { palette.primary }, palette);
        let subtitle = match (&source, count) {
            (Source::Loading, _) => t("Reading the crash log…").to_string(),
            (Source::Ready, 0) => t("No crashes in the last 7 days").to_string(),
            (Source::Ready, n) if unseen > 0 => tf("{count} in the last 7 days · {new} new", &[("count", &n), ("new", &unseen)]),
            (Source::Ready, n) => tf("{count} in the last 7 days", &[("count", &n)]),
            (Source::Missing, _) => t("systemd-coredump isn't available").to_string(),
            (Source::Failed(_), _) => t("Couldn't read the crash log").to_string(),
        };
        let mark: Option<AnyElement> = (unseen > 0).then(|| {
            let m = m.clone();
            pill_button("crash-seen", Some("check"), t("Mark all as seen"), readable(palette.primary, palette), palette, move |_, cx| {
                mark_all_seen(&m, cx);
            })
            .into_any_element()
        });
        let head = header("bug", accent, t("Crash reports"), subtitle, palette, mark);

        let show_hero = source == Source::Ready && count > 0;
        let rows: Vec<AnyElement> = (0..count).map(|i| crash_row(i, m, palette, row_r, cx)).collect();
        let content_h = count as f32 * (ROW_H + ROW_GAP) + if selected { ACTIONS_H } else { 0.0 };
        let list_h = if count == 0 { EMPTY_H } else { content_h.min(LIST_MAX_H) };
        let list: AnyElement = match (&source, count) {
            (Source::Loading, _) => empty_state("bug", t("Reading the crash log…"), t("Asking coredumpctl"), EMPTY_H, palette).into_any_element(),
            (Source::Missing, _) => empty_state("bug", t("Crashes aren't being collected"), t("coredumpctl isn't installed, so systemd-coredump isn't collecting crashes"), EMPTY_H, palette).into_any_element(),
            (Source::Failed(e), _) => empty_state("alert", t("Couldn't read the crash log"), e.clone(), EMPTY_H, palette).into_any_element(),
            (Source::Ready, 0) if !collecting => empty_state("bug", t("Crashes aren't being collected"), t("systemd-coredump isn't set as the kernel's core handler"), EMPTY_H, palette).into_any_element(),
            (Source::Ready, 0) => empty_state("check", t("No crashes this week"), t("Apps that crash show up here"), EMPTY_H, palette).into_any_element(),
            (Source::Ready, _) => {
                let fill = crate::shell::frame_color(palette);
                let clipped = content_h > LIST_MAX_H + SIZE_EPSILON_PX;
                div()
                    .relative()
                    .h(px(list_h))
                    .child(div().id("crash-list").size_full().overflow_y_scroll().flex().flex_col().gap(px(ROW_GAP)).children(rows))
                    .when(clipped, |el| el.child(div().absolute().left_0().right_0().bottom_0().h(px(LIST_FADE_PX)).bg(oklab_gradient(ANGLE_DOWN_DEG, fill.with_alpha(0.0), fill))))
                    .into_any_element()
            }
        };
        let hero_h = if show_hero { HERO_H + GAP } else { 0.0 };
        let height = PAD * 2.0 + HEADER_H + GAP + hero_h + list_h;
        if (f32::from(self.size_cell.get().height) - height).abs() > SIZE_EPSILON_PX {
            self.size_cell.set(size(px(PANEL_W), px(height)));
        }
        let p = &self.p;
        let hero_el = show_hero.then(|| hero(&m.borrow(), palette, card_r));
        div()
            .relative()
            .size_full()
            .p(px(PAD))
            .flex()
            .flex_col()
            .gap(px(GAP))
            .child(reveal(p, 0, head))
            .when_some(hero_el, |el, h| el.child(reveal(p, 1, h)))
            .child(reveal(p, 2, list))
            .when_some(flash, |el, (ok, msg)| el.child(toast(ok, msg, PAD, palette)))
            .into_any_element()
    }
}

fn chip(m: &Shared, palette: &MandaraPalette, window: &Window, cx: &mut App) -> AnyElement {
    keep_alive(m, window, cx);
    poll(m, false, cx);
    let n = m.borrow().unseen();
    if n == 0 {
        return div().into_any_element();
    }
    let coral = palette.urgent_coral;
    div()
        .flex()
        .items_center()
        .gap(px(CHIP_GAP_PX))
        .child(icon("bug", crate::style::bar_icon(CHIP_ICON_PX), readable(coral, palette)))
        .child(
            div()
                .min_w(px(CHIP_BADGE_PX))
                .h(px(CHIP_BADGE_PX))
                .px(px(CHIP_BADGE_PAD_X_PX))
                .rounded_full()
                .bg(coral.hsla())
                .shadow(vec![glow(coral.with_alpha(CHIP_GLOW.0), CHIP_GLOW.1, 0.0, 0.0)])
                .text_color(ink_on(coral).hsla())
                .font_features(tabular_nums())
                .text_size(px(CHIP_BADGE_TEXT_PX))
                .font_weight(FontWeight::BOLD)
                .flex()
                .items_center()
                .justify_center()
                .child(if n > BADGE_MAX { format!("{BADGE_MAX}+") } else { n.to_string() }),
        )
        .into_any_element()
}

// ── Commands ─────────────────────────────────────────────────────────────

/// Runs `f` on its own thread; the CLI receives its JSON.
fn pending(f: impl FnOnce() -> Result<serde_json::Value, String> + Send + 'static) -> Output {
    let (tx, rx) = async_channel::bounded(1);
    let _ = std::thread::Builder::new().name("mandara-crashes-cmd".into()).spawn(move || {
        let _ = tx.send_blocking(f().map(|v| serde_json::to_string_pretty(&v).unwrap_or_default()));
    });
    Output::Pending(rx)
}

fn list_json(listing: Listing, seen_up_to: u64) -> serde_json::Value {
    match listing {
        Listing::Crashes { crashes } => {
            let items: Vec<serde_json::Value> = crashes
                .iter()
                .map(|c| json!({ "time": c.time, "unix": c.unix(), "pid": c.pid, "uid": c.uid, "exe": c.exe, "name": c.name(), "signal": c.signal(), "sig": c.sig, "corefile": c.corefile, "size": c.size, "unseen": c.time > seen_up_to }))
                .collect();
            json!({ "state": "ok", "collecting": svc::collecting(), "since": svc::SINCE, "seen_up_to": seen_up_to, "unseen": svc::unseen(&crashes, seen_up_to), "crashes": items })
        }
        Listing::Missing => json!({ "state": "missing", "collecting": false, "crashes": [] }),
        Listing::Failed { error } => json!({ "state": "failed", "error": error, "crashes": [] }),
    }
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Crash reports"), tk("Apps that crashed this week, from systemd-coredump, with reports and a debugger a click away"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let m = model();
        let bar_model = m.clone();
        let bar = fn_view(None, move |palette, window, cx| chip(&bar_model, palette, window, cx), window, cx);
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(PAD * 2.0 + HEADER_H + GAP + EMPTY_H))));
        let view = View { p: panel_cx.clone(), m, size_cell: size_cell.clone() };
        let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), ..PluginInstance::empty() }
    });
    r.command(
        ID,
        CommandSpec::new_output("crashes list", tk("Crashes of the last 7 days from systemd-coredump, newest first, with which are unseen"), |_, _| {
            Ok(pending(|| Ok(list_json(svc::list(), svc::load_seen()))))
        })
        .example("crashes list"),
    );
    r.command(
        ID,
        CommandSpec::new_output("crashes info", tk("The full coredumpctl report of a crash (with a stack trace when the core is stored)"), |a, _| {
            let pid = u32::try_from(a.int(0)?).map_err(|_| t("expected a PID").to_string())?;
            Ok(pending(move || svc::info(pid).map(|report| json!({ "pid": pid, "report": report }))))
        })
        .arg(ArgSpec::required("pid", ArgKind::Integer, tk("PID of the crashed process")))
        .example("crashes info 4242"),
    );
    r.command(
        ID,
        CommandSpec::new_output("crashes seen", tk("Mark every crash as seen (clears the bar badge)"), |_, cx| {
            let up_to = match MODEL.with(|w| w.borrow().upgrade()) {
                Some(m) => mark_all_seen(&m, cx),
                None => {
                    let up_to = now_micros();
                    return Ok(pending(move || svc::save_seen(up_to).map(|()| json!({ "seen_up_to": up_to }))));
                }
            };
            Ok(Output::Value(json!({ "seen_up_to": up_to })))
        })
        .example("crashes seen"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crash(time: u64, pid: u32, sig: i32, exe: &str) -> Crash {
        Crash { time, pid, uid: 1000, sig, corefile: "present".into(), exe: exe.into(), size: Some(1) }
    }

    #[test]
    fn first_read_sets_a_baseline_and_later_crashes_are_announced() {
        let mut m = Model::new();
        let first = m.absorb(Listing::Crashes { crashes: vec![crash(200, 2, 11, "/usr/bin/a"), crash(100, 1, 6, "/usr/bin/b")] }, true, Some(150));
        assert!(first.is_empty(), "crashes from before the shell started aren't announced");
        assert_eq!(m.unseen(), 1);
        let second = m.absorb(Listing::Crashes { crashes: vec![crash(300, 3, 11, "/usr/bin/c"), crash(200, 2, 11, "/usr/bin/a"), crash(100, 1, 6, "/usr/bin/b")] }, true, None);
        assert_eq!(second.iter().map(|c| c.pid).collect::<Vec<_>>(), [3]);
        assert_eq!(m.unseen(), 2);
        let third = m.absorb(Listing::Crashes { crashes: m.crashes.clone() }, true, None);
        assert!(third.is_empty(), "announced once");
        assert!(m.mark_seen() >= 300);
        assert_eq!(m.unseen(), 0);
    }

    #[test]
    fn missing_coredumpctl_and_failures_are_states() {
        let mut m = Model::new();
        m.absorb(Listing::Missing, false, Some(0));
        assert_eq!((m.source.clone(), m.crashes.len()), (Source::Missing, 0));
        m.absorb(Listing::Failed { error: "journal unavailable".into() }, true, None);
        assert_eq!(m.source, Source::Failed("journal unavailable".into()));
        let j = list_json(Listing::Missing, 0);
        assert_eq!(j["state"], "missing");
    }

    #[test]
    fn labels_name_the_signal() {
        let r = row_labels(&crash(1_793_259_987_887_410, 2966, 6, "/usr/bin/easyeffects"));
        assert_eq!((r.name.as_ref(), r.signal.as_ref()), ("easyeffects", "SIGABRT"));
        assert!(r.detail.starts_with("Aborted · PID 2966 · "), "{}", r.detail);
        assert!(r.detail.ends_with("core stored"));
        assert_eq!(signal_meaning(11), Some("Segmentation fault"));
        assert_eq!(signal_meaning(42), None);
    }
}
