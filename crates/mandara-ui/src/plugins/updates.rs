//! Updates: checks every detected package manager (pacman via
//! `checkupdates`, AUR helpers, APT, DNF, Zypper, Flatpak) read-only on a
//! configurable interval while the chip is in the bar, and on demand. The
//! chip carries a count badge; the panel lists packages grouped by source
//! with old → new versions (the changed part lit with the accent), per-source
//! errors and hints, the latest Arch news headline (fetched only while the
//! panel is open), and "Update now" buttons that open the user's terminal
//! with the right command — the shell never runs a privileged update itself.
//! Commands: `updates list|check|sources|run|news`.

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use gpui::{
    Animation, AnimationExt as _, AnyElement, App, AppContext as _, ElementId, Entity, FontWeight, InteractiveElement, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px, size,
};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry};
use mandara_services::updates::{self, CheckOptions, CheckResult, NewsItem, Package, SourceKind, SourceReport};
use mandara_theme::{Color, MandaraPalette};
use serde_json::json;

use super::fn_view;
use super::panel_fx::{
    ANGLE_DIAGONAL_DEG, ANGLE_DOWN_DEG, ANGLE_RIGHT_DEG, EXPIRY_MARGIN, HEADER_H, PILL_H_PX, PanelShape, SMALL_ICON_PX, TINT_FILL, TINT_HOVER, ViewSet, alpha,
    empty_state, glass_card, glow, header, partner, readable, redraw_in,
};
use crate::fonts::tabular_nums;
use crate::icons::icon;
use crate::kit::{icon_button, reveal, text, tokens};
use crate::text_field::TextField;

pub const ID: &str = "updates";

/// Default check interval (`plugins.settings.updates.interval_hours`).
pub const DEFAULT_INTERVAL_HOURS: i64 = 3;
const MIN_INTERVAL_HOURS: i64 = 1;
const MAX_INTERVAL_HOURS: i64 = 24;
const SECS_PER_MIN: i64 = 60;
const SECS_PER_HOUR: i64 = 60 * SECS_PER_MIN;
const SECS_PER_DAY: i64 = 24 * SECS_PER_HOUR;
/// First check after the chip appears (keeps shell start-up light).
const DEFAULT_FIRST_CHECK_DELAY_S: u64 = 20;
/// Longest first-check delay accepted (`first_check_delay_s`).
const MAX_FIRST_CHECK_DELAY_S: u64 = 600;
/// News headlines shown by default (`news_count`) and at most.
const DEFAULT_NEWS_COUNT: usize = 1;
const MAX_NEWS_COUNT: usize = 5;
/// How often the scheduler looks whether a check is due.
const SCHEDULER_TICK: Duration = Duration::from_secs(60);
/// The news feed is refetched at most this often.
const NEWS_TTL: Duration = Duration::from_secs(3600);
/// One turn of the "checking" spinner.
const SPIN: Duration = Duration::from_millis(900);
/// Search appears once there are more packages than this.
const SEARCH_MIN: usize = 12;
/// Source tiles per row.
const TILES_PER_ROW: usize = 3;
/// Package rows rendered at most (per group).
const ROWS_PER_GROUP: usize = 150;
/// Cached last result (survives shell restarts so it isn't re-announced).
const CACHE_FILE: &str = "updates.json";

const PANEL_W: f32 = 560.0;
const PAD: f32 = 14.0;
const GAP: f32 = 12.0;
const TILE_H: f32 = 66.0;
const TILE_GAP: f32 = 8.0;
const NEWS_H: f32 = 58.0;
const CONTROLS_H: f32 = 36.0;
const GROUP_H: f32 = 52.0;
const ROW_H: f32 = 36.0;
const ROW_GAP: f32 = 2.0;
/// Rows visible before the list scrolls (the cut half row hints at more).
const VISIBLE_ROWS: f32 = 9.5;
/// Visible list height before it scrolls (the cut row hints at more).
const LIST_MAX_H: f32 = VISIBLE_ROWS * (ROW_H + ROW_GAP);
const EMPTY_H: f32 = 196.0;
const BUTTON_H: f32 = PILL_H_PX;
const OLD_VERSION_MAX_W: f32 = 150.0;
/// Height changes smaller than this don't resize the panel.
const SIZE_EPSILON_PX: f32 = 0.5;
/// Chip badge counts are capped at this.
const BADGE_COUNT_MAX: usize = 999;

// Visual tokens of this panel (px unless noted; alpha pairs are
// `(dark, light)`).
/// Small tinted label badge (repo names, counts).
const BADGE_H_PX: f32 = 16.0;
const BADGE_PAD_X_PX: f32 = 6.0;
const BADGE_TEXT_PX: f32 = 10.0;
const BADGE_TINT: (f32, f32) = (0.14, 0.1);
/// Label size of buttons: half a step above the label size.
const BUTTON_TEXT_PX: f32 = text::LABEL + 0.5;
const BUTTON_PAD_X_PX: f32 = 12.0;
const CHECK_PAD_X_PX: f32 = 11.0;
const BUTTON_GAP_PX: f32 = 6.0;
const CHECK_GAP_PX: f32 = 5.0;
/// Glow under the gradient Update button: alpha, blur, y offset.
const BUTTON_GLOW: (f32, f32, f32) = (0.3, 8.0, 1.0);
const BUTTON_HOVER_OPACITY: f32 = 0.92;
/// The check button dims while a check runs.
const CHECKING_OPACITY: f32 = 0.85;
/// Old version text alpha and the arrow glyph between versions.
const OLD_VERSION_ALPHA: f32 = 0.85;
const ARROW_ICON_PX: f32 = 11.0;
/// Package rows: inset past the guide line, right padding, guide inset
/// from the row height and its alpha.
const ROW_PAD_L_PX: f32 = 16.0;
const ROW_PAD_R_PX: f32 = 10.0;
const ROW_INNER_GAP_PX: f32 = 10.0;
const GUIDE_INSET_PX: f32 = 12.0;
const GUIDE_ALPHA: f32 = 0.09;
const ROW_HOVER: (f32, f32) = (0.05, 0.04);
const GROUP_SELECTED: (f32, f32) = (0.08, 0.06);
const GROUP_PAD_X_PX: f32 = 8.0;
/// Source avatar of a group row: side, radius, glyph, gradient alphas.
const GROUP_AVATAR_PX: f32 = 34.0;
const GROUP_AVATAR_RADIUS_PX: f32 = 11.0;
const GROUP_AVATAR_ICON_PX: f32 = 17.0;
const AVATAR_FILL: (f32, f32) = (0.2, 0.1);
const LINE_GAP_PX: f32 = 1.0;
const INLINE_GAP_PX: f32 = 6.0;
/// Chevron box and glyph.
const CHEVRON_BOX_PX: f32 = 16.0;
/// Source tiles: padding, glyph, big number size / line height, caption.
const TILE_PAD_X_PX: f32 = 12.0;
const TILE_PAD_Y_PX: f32 = 9.0;
const TILE_ICON_PX: f32 = 13.0;
const TILE_VALUE_PX: f32 = 20.0;
const TILE_VALUE_LINE_PX: f32 = 22.0;
const TILE_VALUE_GAP_PX: f32 = 7.0;
const TILE_CAPTION_PAD_PX: f32 = 2.0;
const TILE_CAPTION_PX: f32 = 10.5;
/// News card: padding, hover wash, bell tile (side, radius, alpha, glyph),
/// external-link glyph.
const NEWS_PAD_X_PX: f32 = 12.0;
const NEWS_GAP_PX: f32 = 10.0;
const NEWS_HOVER_ALPHA: f32 = 0.03;
const NEWS_ICON_BOX_PX: f32 = 32.0;
const NEWS_ICON_RADIUS_PX: f32 = 10.0;
const NEWS_ICON_FILL_ALPHA: f32 = 0.14;
const NEWS_ICON_PX: f32 = 15.0;
const NEWS_LINK_ICON_PX: f32 = 13.0;
/// Search field: left / right padding, radius, glyph, clear button.
const SEARCH_PAD_L_PX: f32 = 12.0;
const SEARCH_PAD_R_PX: f32 = 2.0;
const SEARCH_RADIUS_PX: f32 = 12.0;
const SEARCH_GAP_PX: f32 = 2.0;
const SEARCH_ICON_PX: f32 = 14.0;
const CLEAR_BUTTON_PX: f32 = 26.0;
/// Toast: height, padding, shadow (alpha pair, blur, y), icon disc.
const TOAST_H_PX: f32 = 36.0;
const TOAST_PAD_L_PX: f32 = 6.0;
const TOAST_PAD_R_PX: f32 = 14.0;
const TOAST_SHADOW: (f32, f32) = (0.45, 0.14);
const TOAST_SHADOW_BLUR_PX: f32 = 14.0;
const TOAST_SHADOW_DY_PX: f32 = 4.0;
const TOAST_GAP_PX: f32 = 8.0;
const TOAST_ICON_DISC_PX: f32 = 24.0;
const TOAST_ICON_FILL_ALPHA: f32 = 0.16;
/// The toast stays this many paddings narrower than the panel.
const TOAST_SIDE_PADS: f32 = 4.0;
/// Height of the fade over a clipped list's last row.
const LIST_FADE_H_PX: f32 = 22.0;
/// Rows sit this far inside the card radius, never rounder than the min.
const ROW_RADIUS_INSET_PX: f32 = 2.0;
const MIN_ROW_RADIUS_PX: f32 = 10.0;
/// Chip: glyph, badge (min width / height / padding), dim alphas while a
/// check runs.
const CHIP_GAP_PX: f32 = 4.0;
const CHIP_ICON_PX: f32 = 15.0;
const CHIP_BADGE_PX: f32 = 16.0;
const CHIP_BADGE_PAD_X_PX: f32 = 4.0;
const CHIP_CHECKING_ICON_ALPHA: f32 = 0.5;
const CHIP_CHECKING_BADGE_ALPHA: f32 = 0.55;

// ── Shared model ─────────────────────────────────────────────────────────

#[derive(Default)]
struct Model {
    /// The full last result, shared so renders borrow it instead of
    /// cloning every package. Held only while a panel is open; reloaded
    /// from the cache file on the next open.
    result: Option<Rc<CheckResult>>,
    /// Pending updates of the last check (what the bar chip shows).
    count: usize,
    /// When the last check ran (drives the scheduler while `result` is
    /// freed).
    checked_at: Option<i64>,
    /// A cache reload for a reopened panel is in flight.
    reloading: bool,
    /// Panels of this plugin currently open (any monitor).
    open_panels: usize,
    /// The AUR helper on `$PATH`, resolved off the UI thread with each
    /// check (looking it up walks `$PATH`).
    helper: Option<&'static str>,
    checking: bool,
    /// CLI callers waiting for the running check.
    waiters: Vec<async_channel::Sender<Result<String, String>>>,
    /// Total that was last announced (notifications only fire on growth).
    announced: usize,
    news: Option<Result<Vec<NewsItem>, String>>,
    news_at: Option<Instant>,
    news_loading: bool,
    /// Expanded groups; `None` until the user touches one (then the first
    /// non-empty source is open).
    expanded: Option<HashSet<SourceKind>>,
    status: Option<(bool, String, Instant)>,
    /// The chip and panel views; async results notify only these.
    views: ViewSet,
}

impl Model {
    /// Records `r` as the latest result; the package lists are only kept
    /// while a panel shows them.
    fn set_result(&mut self, r: CheckResult) {
        self.count = r.total();
        self.checked_at = Some(r.checked_at);
        self.result = (self.open_panels > 0).then(|| Rc::new(r));
    }

    /// Drops panel-only data once no panel is open (the chip keeps its
    /// count; only the headlines the news cards can show are kept).
    fn free_panel_data(&mut self) {
        self.result = None;
        self.status = None;
        if let Some(Ok(items)) = &mut self.news {
            items.truncate(MAX_NEWS_COUNT);
            items.shrink_to_fit();
        }
    }
}

/// Reloads the cached result for a reopened panel (off the UI thread).
fn reload_result(m: &Shared, cx: &mut App) {
    {
        let mut mb = m.borrow_mut();
        if mb.reloading || mb.result.is_some() {
            return;
        }
        mb.reloading = true;
    }
    let task = cx.background_executor().spawn(async { load_cache() });
    let weak = Rc::downgrade(m);
    cx.spawn(async move |cx| {
        let cached = task.await;
        let Some(m) = weak.upgrade() else { return };
        {
            let mut mb = m.borrow_mut();
            mb.reloading = false;
            // A check that finished meanwhile is newer than the cache.
            if mb.result.is_none()
                && mb.open_panels > 0
                && let Some(r) = cached
            {
                mb.result = Some(Rc::new(r));
            }
        }
        cx.update(|cx| notify(&m, cx));
    })
    .detach();
}

/// Re-renders the views of `m` (never every window).
fn notify(m: &Shared, cx: &mut App) {
    let views = m.borrow().views.clone();
    views.notify(cx);
}

type Shared = Rc<RefCell<Model>>;

thread_local! {
    /// One checker shared by every bar instance; dropped (stopping the
    /// scheduler) when the last chip / panel goes away.
    static SHARED: RefCell<Weak<RefCell<Model>>> = const { RefCell::new(Weak::new()) };
}

fn alive() -> Option<Shared> {
    SHARED.with(|s| s.borrow().upgrade())
}

fn cache_path() -> std::path::PathBuf {
    mandara_config::paths::cache_dir().join(CACHE_FILE)
}

fn load_cache() -> Option<CheckResult> {
    serde_json::from_slice(&std::fs::read(cache_path()).ok()?).ok()
}

fn save_cache(r: &CheckResult) {
    let path = cache_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    if let Ok(bytes) = serde_json::to_vec(r) {
        let _ = std::fs::write(path, bytes);
    }
}

struct Settings {
    interval_secs: i64,
    opts: CheckOptions,
    notify: bool,
    news: bool,
    /// News headlines shown (`news_count`, 1–5).
    news_count: usize,
    /// Wait after login before the first check (`first_check_delay_s`).
    first_check_delay: Duration,
    terminal: String,
}

fn settings(cx: &App) -> Settings {
    let cfg = crate::state::config(cx).read(cx).snapshot();
    let s = &cfg.plugins;
    Settings {
        interval_secs: s.setting(ID, "interval_hours", DEFAULT_INTERVAL_HOURS).clamp(MIN_INTERVAL_HOURS, MAX_INTERVAL_HOURS) * SECS_PER_HOUR,
        opts: CheckOptions { aur: s.setting(ID, "aur", true), flatpak: s.setting(ID, "flatpak", true) },
        notify: s.setting(ID, "notify", true),
        news: s.setting(ID, "arch_news", true),
        news_count: (s.setting(ID, "news_count", DEFAULT_NEWS_COUNT as f64).round().max(1.0) as usize).min(MAX_NEWS_COUNT),
        first_check_delay: Duration::from_secs((s.setting(ID, "first_check_delay_s", DEFAULT_FIRST_CHECK_DELAY_S as f64).round().max(0.0) as u64).min(MAX_FIRST_CHECK_DELAY_S)),
        // The plugin's own terminal, else Studio → Applications' choice.
        terminal: Some(s.setting(ID, "terminal", String::new())).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| cfg.apps.terminal_command().unwrap_or_default()),
    }
}

fn now_secs() -> i64 {
    chrono::Local::now().timestamp()
}

/// The shared model, created (with its scheduler) on first use.
fn shared(cx: &mut App) -> Shared {
    if let Some(m) = alive() {
        return m;
    }
    let m: Shared = Rc::new(RefCell::new(Model::default()));
    SHARED.with(|s| *s.borrow_mut() = Rc::downgrade(&m));
    // The cached result (file read + JSON parse) and the AUR helper lookup
    // stay off the UI thread.
    let load = cx.background_executor().spawn(async { (load_cache(), updates::aur_helper()) });
    let weak = Rc::downgrade(&m);
    cx.spawn(async move |cx| {
        let (cached, helper) = load.await;
        if let Some(m) = weak.upgrade() {
            {
                let mut mb = m.borrow_mut();
                mb.helper = helper;
                if mb.checked_at.is_none()
                    && let Some(r) = cached
                {
                    mb.announced = r.total();
                    mb.set_result(r);
                }
            }
            cx.update(|cx| notify(&m, cx));
        }
        let delay = cx.update(|cx| settings(cx).first_check_delay);
        cx.background_executor().timer(delay).await;
        loop {
            let Some(m) = weak.upgrade() else { break };
            cx.update(|cx| {
                let interval = settings(cx).interval_secs;
                let due = {
                    let mb = m.borrow();
                    !mb.checking && mb.checked_at.is_none_or(|at| now_secs() - at >= interval)
                };
                if due {
                    start_check(&m, None, cx);
                }
            });
            drop(m);
            cx.background_executor().timer(SCHEDULER_TICK).await;
        }
    })
    .detach();
    m
}

fn result_json(r: &CheckResult) -> String {
    serde_json::to_string_pretty(r).unwrap_or_default()
}

/// Starts a check unless one is running; `reply` receives the result.
fn start_check(m: &Shared, reply: Option<async_channel::Sender<Result<String, String>>>, cx: &mut App) {
    {
        let mut mb = m.borrow_mut();
        mb.waiters.extend(reply);
        if mb.checking {
            return;
        }
        mb.checking = true;
    }
    let opts = settings(cx).opts;
    // Check, cache write and helper lookup all run off the UI thread.
    let task = cx.background_executor().spawn(async move {
        let result = updates::check(opts);
        save_cache(&result);
        (result, updates::aur_helper())
    });
    let weak = Rc::downgrade(m);
    cx.spawn(async move |cx| {
        let (result, helper) = task.await;
        cx.update(|cx| {
            if let Some(m) = weak.upgrade() {
                m.borrow_mut().helper = helper;
                absorb(&m, result, cx);
                notify(&m, cx);
            }
        });
    })
    .detach();
    notify(m, cx);
}

fn absorb(m: &Shared, result: CheckResult, cx: &mut App) {
    let total = result.total();
    let grew = {
        let mut mb = m.borrow_mut();
        mb.checking = false;
        let grew = total > mb.announced;
        mb.announced = total;
        let json = result_json(&result);
        for w in std::mem::take(&mut mb.waiters) {
            let _ = w.try_send(Ok(json.clone()));
        }
        grew
    };
    let body = (grew && settings(cx).notify)
        .then(|| result.sources.iter().filter(|s| !s.packages.is_empty()).map(|s| format!("{} {}", s.kind.label(), s.packages.len())).collect::<Vec<_>>().join(" · "));
    m.borrow_mut().set_result(result);
    if let Some(body) = body {
        crate::plugins::notifications::send(
            cx,
            mandara_services::NotificationCommand::Post { app: t("Updates").into(), icon: "package".into(), summary: tf("{count} updates available", &[("count", &total)]), body },
        );
    }
}

/// Opens the terminal running `command`; records a toast on `m`.
fn run_in_terminal(m: Option<&Shared>, command: &str, cx: &mut App) -> Result<String, String> {
    let result = updates::open_terminal(&settings(cx).terminal, command);
    if let Some(m) = m {
        let msg = match &result {
            Ok(term) => tf("Opened {terminal}: {command}", &[("terminal", term), ("command", &command)]),
            Err(e) => e.clone(),
        };
        m.borrow_mut().status = Some((result.is_ok(), msg, Instant::now()));
    }
    result
}

fn fetch_news(m: &Shared, cx: &mut App) {
    {
        let mut mb = m.borrow_mut();
        if mb.news_loading || mb.news_at.is_some_and(|t| t.elapsed() < NEWS_TTL) {
            return;
        }
        mb.news_loading = true;
    }
    let task = cx.background_executor().spawn(async move { updates::fetch_news() });
    let weak = Rc::downgrade(m);
    cx.spawn(async move |cx| {
        let news = task.await;
        if let Some(m) = weak.upgrade() {
            {
                let mut mb = m.borrow_mut();
                mb.news = Some(news);
                mb.news_at = Some(Instant::now());
                mb.news_loading = false;
            }
            cx.update(|cx| notify(&m, cx));
        }
    })
    .detach();
}

/// "Checked just now / 5 min ago / 3 h ago / 27 Oct".
fn checked_label(ts: i64) -> String {
    let secs = (now_secs() - ts).max(0);
    match secs {
        ..SECS_PER_MIN => t("Checked just now").to_string(),
        SECS_PER_MIN..SECS_PER_HOUR => tf("Checked {n} min ago", &[("n", &(secs / SECS_PER_MIN))]),
        SECS_PER_HOUR..SECS_PER_DAY => tf("Checked {n} h ago", &[("n", &(secs / SECS_PER_HOUR))]),
        _ => {
            use chrono::TimeZone as _;
            let date = chrono::Local.timestamp_opt(ts, 0).single().map(|d| d.format("%d %b").to_string()).unwrap_or_default();
            tf("Checked {date}", &[("date", &date)])
        }
    }
}

fn source_icon(kind: SourceKind) -> &'static str {
    match kind {
        SourceKind::Aur => "globe",
        SourceKind::Flatpak => "box",
        _ => "package",
    }
}

/// Packages of `report` matching `q` (name or repo, case-insensitive).
/// An empty query lowercases nothing.
fn matching<'a>(report: &'a SourceReport, q: &str) -> Vec<&'a Package> {
    if q.is_empty() {
        return report.packages.iter().collect();
    }
    let q = q.to_lowercase();
    report.packages.iter().filter(|p| p.name.to_lowercase().contains(&q) || p.repo.as_deref().is_some_and(|r| r.to_lowercase().contains(&q))).collect()
}

// ── Pieces ───────────────────────────────────────────────────────────────

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

/// The accent gradient "Update" button that opens the terminal.
fn update_button(id: ElementId, label: &'static str, command: String, m: &Shared, palette: &MandaraPalette) -> AnyElement {
    let from = palette.primary;
    let to = partner(palette.primary).with_lightness(palette.primary.lightness());
    let ink = palette.on_primary;
    let m = m.clone();
    div()
        .id(id)
        .h(px(BUTTON_H))
        .px(px(BUTTON_PAD_X_PX))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(BUTTON_GAP_PX))
        .cursor_pointer()
        .bg(oklab_gradient(ANGLE_RIGHT_DEG, from, to))
        .shadow(vec![glow(readable(from, palette).with_alpha(BUTTON_GLOW.0), BUTTON_GLOW.1, 0.0, BUTTON_GLOW.2)])
        .hover(|s| s.opacity(BUTTON_HOVER_OPACITY))
        .text_size(px(BUTTON_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(ink.hsla())
        .child(icon("terminal", SMALL_ICON_PX, ink))
        .child(label)
        .on_click(move |_, _, cx| {
            cx.stop_propagation();
            let _ = run_in_terminal(Some(&m), &command, cx);
            notify(&m, cx);
        })
        .into_any_element()
}

/// `old → new`, the changed tail of `new` lit with the accent.
fn version_cell(p: &Package, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let dim = palette.on_surface_variant;
    let cell = div().flex_none().flex().items_center().gap(px(INLINE_GAP_PX)).font_features(tabular_nums()).text_size(px(text::BODY_S));
    if p.old.is_empty() && p.new.is_empty() {
        return cell.text_color(dim.hsla()).child(t("update available")).into_any_element();
    }
    let (same, changed) = updates::version_split(&p.old, &p.new);
    cell.when(!p.old.is_empty(), |el| {
        el.child(div().max_w(px(OLD_VERSION_MAX_W)).truncate().text_color(dim.with_alpha(OLD_VERSION_ALPHA).hsla()).child(p.old.clone())).child(icon("arrow", ARROW_ICON_PX, dim))
    })
    .child(
        div()
            .flex()
            .whitespace_nowrap()
            .child(div().text_color(palette.on_surface.hsla()).child(same.to_string()))
            .child(div().font_weight(FontWeight::SEMIBOLD).text_color(accent.hsla()).child(changed.to_string())),
    )
    .into_any_element()
}

/// One package; `index` is its position in the list (a cheap element id
/// instead of formatting the name into one on every frame).
fn package_row(index: usize, p: &Package, radius: f32, palette: &MandaraPalette) -> AnyElement {
    let hover = palette.on_surface.with_alpha(alpha(ROW_HOVER, palette));
    div()
        .id(ElementId::named_usize("upd-pkg", index))
        .h(px(ROW_H))
        .flex_none()
        .pl(px(ROW_PAD_L_PX))
        .pr(px(ROW_PAD_R_PX))
        .rounded(px(radius))
        .flex()
        .items_center()
        .gap(px(ROW_INNER_GAP_PX))
        .hover(move |s| s.bg(hover.hsla()))
        // A guide line ties the rows to their group.
        .child(div().w(px(2.0)).h(px(ROW_H - GUIDE_INSET_PX)).flex_none().rounded_full().bg(palette.on_surface.with_alpha(GUIDE_ALPHA).hsla()))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .items_center()
                .gap(px(INLINE_GAP_PX))
                .child(div().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).child(p.name.clone()))
                .when_some(p.repo.clone(), |el, r| el.child(badge(r, readable(palette.tertiary, palette), palette))),
        )
        .child(version_cell(p, palette))
        .into_any_element()
}

fn chevron(expanded: bool, palette: &MandaraPalette) -> AnyElement {
    div()
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
        )
        .into_any_element()
}

fn group_row(report: &SourceReport, shown: usize, expanded: bool, m: &Shared, radius: f32, palette: &MandaraPalette) -> AnyElement {
    let kind = report.kind;
    let n = report.packages.len();
    let accent = readable(palette.primary, palette);
    let (tint, detail): (Color, String) = if let Some(e) = &report.error {
        (readable(palette.urgent_coral, palette), e.clone())
    } else if let Some(h) = &report.hint {
        (readable(palette.warning_amber, palette), h.clone())
    } else if n == 0 {
        (readable(palette.success_emerald, palette), t("Up to date").to_string())
    } else {
        (palette.on_surface_variant, report.command.clone())
    };
    let hover = palette.on_surface.with_alpha(alpha(ROW_HOVER, palette));
    let sel = accent.with_alpha(alpha(GROUP_SELECTED, palette));
    let m2 = m.clone();
    let lead_tint = if report.error.is_some() { palette.urgent_coral } else { palette.primary };
    div()
        .id(ElementId::Name(SharedString::new_static(kind.id())))
        .h(px(GROUP_H))
        .flex_none()
        .px(px(GROUP_PAD_X_PX))
        .rounded(px(radius))
        .flex()
        .items_center()
        .gap(px(ROW_INNER_GAP_PX))
        .when(n > 0, |el| el.cursor_pointer())
        .when(expanded && n > 0, |el| el.bg(sel.hsla()))
        .when(!expanded && n > 0, |el| el.hover(move |s| s.bg(hover.hsla())))
        .child(
            div()
                .size(px(GROUP_AVATAR_PX))
                .flex_none()
                .rounded(px(crate::style::radius(GROUP_AVATAR_RADIUS_PX)))
                .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, readable(lead_tint, palette).with_alpha(AVATAR_FILL.0), readable(partner(lead_tint), palette).with_alpha(AVATAR_FILL.1)))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(source_icon(kind), GROUP_AVATAR_ICON_PX, readable(lead_tint, palette))),
        )
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
                        .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(kind.label()))
                        .when(n > 0, |el| {
                            let label = if shown < n { format!("{shown}/{n}") } else { n.to_string() };
                            el.child(badge(label, accent, palette)).child(chevron(expanded, palette))
                        }),
                )
                .child(div().text_size(px(text::LABEL)).text_color(tint.hsla()).truncate().child(detail)),
        )
        .when(n > 0, |el| el.child(update_button("upd-run".into(), t("Update"), report.command.clone(), m, palette)))
        .on_click(move |_, window, _| {
            if n == 0 {
                return;
            }
            let mut mb = m2.borrow_mut();
            let first = first_open(mb.result.as_deref());
            let set = mb.expanded.get_or_insert_with(|| first.into_iter().collect());
            if !set.remove(&kind) {
                set.insert(kind);
            }
            window.refresh();
        })
        .into_any_element()
}

/// The group open by default: the first source with packages.
fn first_open(result: Option<&CheckResult>) -> Option<SourceKind> {
    result?.sources.iter().find(|s| !s.packages.is_empty()).map(|s| s.kind)
}

fn tile(report: &SourceReport, radius: f32, palette: &MandaraPalette) -> AnyElement {
    let n = report.packages.len();
    let (tint, detail) = if report.error.is_some() {
        (readable(palette.urgent_coral, palette), t("Check failed").to_string())
    } else if report.hint.is_some() {
        (readable(palette.warning_amber, palette), t("Needs pacman-contrib").to_string())
    } else if n == 0 {
        (readable(palette.success_emerald, palette), t("Up to date").to_string())
    } else {
        // The big number already says how many; the caption says what.
        (readable(palette.primary, palette), t("pending").to_string())
    };
    glass_card(palette)
        .rounded(px(radius))
        .flex_1()
        .min_w(px(0.0))
        .h(px(TILE_H))
        .px(px(TILE_PAD_X_PX))
        .py(px(TILE_PAD_Y_PX))
        .flex()
        .flex_col()
        .justify_between()
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(INLINE_GAP_PX))
                .child(icon(source_icon(report.kind), TILE_ICON_PX, tint))
                .child(div().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).child(report.kind.label())),
        )
        .child(
            div()
                .flex()
                .items_end()
                .gap(px(TILE_VALUE_GAP_PX))
                .child(div().font_features(tabular_nums()).text_size(px(TILE_VALUE_PX)).line_height(px(TILE_VALUE_LINE_PX)).font_weight(FontWeight::MEDIUM).child(if report.error.is_some() || report.hint.is_some() { "–".to_string() } else { n.to_string() }))
                .child(div().pb(px(TILE_CAPTION_PAD_PX)).min_w(px(0.0)).truncate().text_size(px(TILE_CAPTION_PX)).text_color(tint.hsla()).child(detail)),
        )
        .into_any_element()
}

/// News cards shown for up to `count` headlines (one card while loading,
/// on errors or with no news).
fn news_shown(m: &Shared, count: usize) -> usize {
    match &m.borrow().news {
        Some(Ok(items)) => items.len().clamp(1, count.max(1)),
        _ => 1,
    }
}

/// The `i`-th news card.
fn news_card(m: &Shared, i: usize, radius: f32, palette: &MandaraPalette) -> AnyElement {
    let (item, loading, error) = {
        let mb = m.borrow();
        match &mb.news {
            Some(Ok(items)) => (items.get(i).cloned(), mb.news_loading, None),
            Some(Err(e)) => (None, mb.news_loading, Some(e.clone())),
            None => (None, true, None),
        }
    };
    let accent = readable(palette.tertiary, palette);
    let (title, sub) = match (&item, &error) {
        (Some(n), _) => {
            let when = n.published.map(|ts| {
                use chrono::TimeZone as _;
                chrono::Local.timestamp_opt(ts, 0).single().map(|d| d.format("%d %b %Y").to_string()).unwrap_or_default()
            });
            (n.title.clone(), when.map(|w| tf("Arch Linux news · {date}", &[("date", &w)])).unwrap_or_else(|| t("Arch Linux news").to_string()))
        }
        (None, Some(e)) => (t("News unavailable").to_string(), e.clone()),
        (None, None) if loading => (t("Loading Arch news…").to_string(), "archlinux.org".to_string()),
        (None, None) => (t("No news").to_string(), "archlinux.org".to_string()),
    };
    let link = item.map(|n| n.link).filter(|l| !l.is_empty());
    let hover = palette.on_surface.with_alpha(NEWS_HOVER_ALPHA);
    glass_card(palette)
        .id(("upd-news", i))
        .rounded(px(radius))
        .h(px(NEWS_H))
        .px(px(NEWS_PAD_X_PX))
        .flex()
        .items_center()
        .gap(px(NEWS_GAP_PX))
        .when(link.is_some(), |el| el.cursor_pointer().hover(move |s| s.bg(hover.hsla())))
        .child(
            div()
                .size(px(NEWS_ICON_BOX_PX))
                .flex_none()
                .rounded(px(crate::style::radius(NEWS_ICON_RADIUS_PX)))
                .bg(accent.with_alpha(NEWS_ICON_FILL_ALPHA).hsla())
                .flex()
                .items_center()
                .justify_center()
                .child(icon("bell", NEWS_ICON_PX, accent)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(LINE_GAP_PX))
                .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::SEMIBOLD).truncate().child(title))
                .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(sub)),
        )
        .when(link.is_some(), |el| el.child(icon("external", NEWS_LINK_ICON_PX, palette.on_surface_variant)))
        .on_click(move |_, _, _| {
            if let Some(link) = link.clone() {
                std::thread::spawn(move || {
                    if let Err(e) = mandara_services::session::open_uri(&link) {
                        tracing::warn!("updates: {e}");
                    }
                });
            }
        })
        .into_any_element()
}

fn check_button(m: &Shared, checking: bool, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let hover = accent.with_alpha(alpha(TINT_HOVER, palette));
    let spinner = gpui::svg()
        .path("icons/refresh.svg")
        .size(px(SMALL_ICON_PX))
        .flex_none()
        .text_color(accent.hsla())
        .with_animation("upd-spin", Animation::new(SPIN).repeat(), |svg, delta| svg.with_transformation(gpui::Transformation::rotate(gpui::radians(delta * std::f32::consts::TAU))));
    let m = m.clone();
    div()
        .id("upd-check")
        .h(px(BUTTON_H))
        .px(px(CHECK_PAD_X_PX))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(CHECK_GAP_PX))
        .bg(accent.with_alpha(alpha(TINT_FILL, palette)).hsla())
        .text_size(px(BUTTON_TEXT_PX))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(accent.hsla())
        .when(checking, |el| el.opacity(CHECKING_OPACITY).child(spinner).child(t("Checking…")))
        .when(!checking, |el| {
            // start_check notifies the plugin's views itself.
            el.cursor_pointer().hover(move |s| s.bg(hover.hsla())).child(icon("refresh", SMALL_ICON_PX, accent)).child(t("Check now")).on_click(move |_, _, cx| start_check(&m, None, cx))
        })
        .into_any_element()
}

fn search_box(field: &Entity<TextField>, query: &str, palette: &MandaraPalette) -> AnyElement {
    let f = field.clone();
    div()
        .flex_1()
        .min_w(px(0.0))
        .h(px(CONTROLS_H))
        .pl(px(SEARCH_PAD_L_PX))
        .pr(px(SEARCH_PAD_R_PX))
        .rounded(px(crate::style::radius(SEARCH_RADIUS_PX)))
        .bg(palette.surface_container_high.hsla())
        .flex()
        .items_center()
        .gap(px(SEARCH_GAP_PX))
        .child(icon("search", SEARCH_ICON_PX, palette.on_surface_variant))
        .child(div().flex_1().min_w(px(0.0)).child(field.clone()))
        .when(!query.is_empty(), |el| {
            el.child(icon_button("upd-clear", "close", CLEAR_BUTTON_PX, palette.on_surface_variant, None, move |window, cx| {
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
                .gap(px(TOAST_GAP_PX))
                .child(
                    div()
                        .size(px(TOAST_ICON_DISC_PX))
                        .flex_none()
                        .rounded_full()
                        .bg(tint.with_alpha(TOAST_ICON_FILL_ALPHA).hsla())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(if ok { "terminal" } else { "close" }, SMALL_ICON_PX, tint)),
                )
                .child(div().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).child(msg)),
        )
        .into_any_element()
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
    m: Shared,
    field: Entity<TextField>,
    size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>,
    watch: CloseWatch,
}

impl View {
    /// Frees the package lists when the last panel closes.
    fn watch_close(&self, cx: &mut App) {
        let m = self.m.clone();
        let armed = self.watch.arm(
            &self.p,
            move |_| {
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

    /// The reopened panel while its result reloads: the header and a
    /// placeholder at the last height (no reflow when it lands).
    fn reloading(&self, checked_at: i64, checking: bool, palette: &MandaraPalette) -> AnyElement {
        let accent = readable(palette.primary, palette);
        let body_h = (f32::from(self.size_cell.get().height) - PAD * 2.0 - HEADER_H - GAP).max(EMPTY_H);
        let head = header("package", accent, t("Updates"), checked_label(checked_at), palette, Some(check_button(&self.m, checking, palette)));
        div()
            .relative()
            .size_full()
            .p(px(PAD))
            .flex()
            .flex_col()
            .child(reveal(&self.p, 0, head))
            .child(div().h(px(GAP)))
            .child(reveal(&self.p, 1, empty_state("package", checked_label(checked_at), SharedString::default(), body_h, palette)))
            .into_any_element()
    }

    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let m = &self.m;
        m.borrow().views.track(window);
        let s = settings(cx);
        let visible = self.p.progress() > 0.0;
        if visible {
            self.watch_close(cx);
        }
        let (freed_at, checking) = {
            let mb = m.borrow();
            (mb.checked_at.filter(|_| mb.result.is_none()), mb.checking)
        };
        if let Some(at) = freed_at {
            if visible {
                reload_result(m, cx);
            }
            return self.reloading(at, checking, palette);
        }
        let shape = PanelShape::of(cx);
        let card_r = shape.inner(PAD);
        let row_r = (card_r - ROW_RADIUS_INSET_PX).max(MIN_ROW_RADIUS_PX);
        let query = self.field.read(cx).value.clone();
        let q = query.trim().to_string();

        // Toast lifetime.
        let status = {
            let mut mb = m.borrow_mut();
            let toast_for = crate::state::config(cx).read(cx).snapshot().panels.toast();
            if mb.status.as_ref().is_some_and(|(_, _, at)| at.elapsed() >= toast_for) {
                mb.status = None;
            }
            mb.status.clone()
        };
        if let Some((_, _, at)) = &status {
            redraw_in(crate::state::config(cx).read(cx).snapshot().panels.toast().saturating_sub(at.elapsed()) + EXPIRY_MARGIN, window, cx);
        }

        let (result, checking, expanded, helper) = {
            let mb = m.borrow();
            (mb.result.clone(), mb.checking, mb.expanded.clone(), mb.helper)
        };
        let reports: &[SourceReport] = result.as_ref().map_or(&[], |r| &r.sources);
        let total: usize = reports.iter().map(|r| r.packages.len()).sum();
        let has_pacman = reports.iter().any(|r| r.kind == SourceKind::Pacman);
        let show_news = s.news && has_pacman;
        if visible && show_news {
            fetch_news(m, cx);
        }

        // ── Header ───────────────────────────────────────────────────────
        let accent = readable(palette.primary, palette);
        let subtitle = match (&result, checking) {
            (_, true) => t("Checking for updates…").to_string(),
            (Some(r), false) if total > 0 => format!("{} · {}", if total == 1 { t("1 update").to_string() } else { tf("{count} updates", &[("count", &total)]) }, checked_label(r.checked_at)),
            (Some(r), false) => checked_label(r.checked_at),
            (None, false) => t("Not checked yet").to_string(),
        };
        let head = header("package", accent, t("Updates").to_string(), subtitle, palette, Some(check_button(m, checking, palette)));

        // ── Source tiles ─────────────────────────────────────────────────
        let tile_rows: Vec<AnyElement> = reports
            .chunks(TILES_PER_ROW)
            .map(|chunk| div().flex().gap(px(TILE_GAP)).children(chunk.iter().map(|r| tile(r, card_r, palette))).into_any_element())
            .collect();
        let tiles_h = if tile_rows.is_empty() { 0.0 } else { tile_rows.len() as f32 * TILE_H + (tile_rows.len() - 1) as f32 * TILE_GAP };

        // ── Groups ───────────────────────────────────────────────────────
        let searching = !q.is_empty();
        let open = expanded.unwrap_or_else(|| first_open(result.as_deref()).into_iter().collect());
        let mut rows: Vec<AnyElement> = Vec::new();
        let mut list_h = 0.0;
        let mut matches = 0usize;
        let mut package_index = 0usize;
        for r in reports {
            let hits = matching(r, &q);
            matches += hits.len();
            if searching && hits.is_empty() {
                continue;
            }
            let is_open = !r.packages.is_empty() && (searching || open.contains(&r.kind));
            rows.push(group_row(r, hits.len(), is_open, m, row_r, palette));
            list_h += GROUP_H + ROW_GAP;
            if is_open {
                for p in hits.iter().take(ROWS_PER_GROUP) {
                    rows.push(package_row(package_index, p, row_r, palette));
                    package_index += 1;
                    list_h += ROW_H + ROW_GAP;
                }
            }
        }
        let errors = reports.iter().any(|r| r.error.is_some() || r.hint.is_some());
        let empty_kind: Option<(&'static str, String, String)> = if result.is_none() {
            Some(if checking {
                ("refresh", t("Checking for updates…").to_string(), t("Asking each package manager").to_string())
            } else {
                ("package", t("Not checked yet").to_string(), t("Press Check now to look for updates").to_string())
            })
        } else if reports.is_empty() {
            Some(("package", t("No package manager found").to_string(), t("Supports pacman, AUR helpers, APT, DNF, Zypper and Flatpak").to_string()))
        } else if searching && matches == 0 {
            Some(("search", tf("No package matches \u{201c}{query}\u{201d}", &[("query", &q)]), t("Try another name or repository").to_string()))
        } else if total == 0 && !errors {
            Some(("check", t("Everything is up to date").to_string(), result.as_ref().map(|r| checked_label(r.checked_at)).unwrap_or_default()))
        } else {
            None
        };
        // Up-to-date sources stay listed under the empty state only when
        // something needs attention (errors / hints).
        let list_h = match &empty_kind {
            Some(_) => EMPTY_H,
            None => list_h.min(LIST_MAX_H),
        };

        // ── Controls: search + update all ────────────────────────────────
        let show_controls = total > 0;
        let kinds: Vec<SourceKind> = reports.iter().filter(|r| !r.packages.is_empty()).map(|r| r.kind).collect();
        let all_cmd = updates::combined_command(&kinds, helper);
        let controls = div()
            .h(px(CONTROLS_H))
            .flex()
            .items_center()
            .gap(px(tokens::SM))
            .when(total > SEARCH_MIN, |el| el.child(search_box(&self.field, &query, palette)))
            .when(total <= SEARCH_MIN, |el| {
                el.child(
                    div()
                        .flex_1()
                        .min_w(px(0.0))
                        .truncate()
                        .text_size(px(text::BODY_S))
                        .text_color(palette.on_surface_variant.hsla())
                        .child(tf("{count} packages from {sources} sources", &[("count", &total), ("sources", &kinds.len())])),
                )
            })
            .child(update_button("upd-run-all".into(), t("Update all"), all_cmd, m, palette));

        // ── Size ─────────────────────────────────────────────────────────
        let mut height = PAD * 2.0 + HEADER_H;
        if tiles_h > 0.0 {
            height += GAP + tiles_h;
        }
        let news_n = if show_news { news_shown(m, s.news_count) } else { 0 };
        if news_n > 0 {
            height += GAP + news_n as f32 * NEWS_H + (news_n - 1) as f32 * TILE_GAP;
        }
        if show_controls {
            height += GAP + CONTROLS_H;
        }
        height += GAP + list_h;
        if (f32::from(self.size_cell.get().height) - height).abs() > SIZE_EPSILON_PX {
            self.size_cell.set(size(px(PANEL_W), px(height)));
        }

        let list: AnyElement = match empty_kind {
            Some((icon_name, title, hint)) => empty_state(icon_name, title, hint, EMPTY_H, palette).into_any_element(),
            None => {
                let fill = crate::shell::frame_color(palette);
                let clipped = list_h >= LIST_MAX_H - SIZE_EPSILON_PX;
                div()
                    .relative()
                    .h(px(list_h))
                    .child(div().id("upd-list").size_full().overflow_y_scroll().flex().flex_col().gap(px(ROW_GAP)).children(rows))
                    .when(clipped, |el| el.child(div().absolute().left_0().right_0().bottom_0().h(px(LIST_FADE_H_PX)).bg(oklab_gradient(ANGLE_DOWN_DEG, fill.with_alpha(0.0), fill))))
                    .into_any_element()
            }
        };

        let p = &self.p;
        let mut index = 0;
        let mut next = || {
            index += 1;
            index
        };
        div()
            .relative()
            .size_full()
            .p(px(PAD))
            .flex()
            .flex_col()
            .child(reveal(p, 0, head))
            .when(tiles_h > 0.0, |el| el.child(div().h(px(GAP))).child(reveal(p, next(), div().flex().flex_col().gap(px(TILE_GAP)).children(tile_rows))))
            .when(news_n > 0, |el| el.child(div().h(px(GAP))).child(reveal(p, next(), div().flex().flex_col().gap(px(TILE_GAP)).children((0..news_n).map(|i| news_card(m, i, card_r, palette))))))
            .when(show_controls, |el| el.child(div().h(px(GAP))).child(reveal(p, next(), controls)))
            .child(div().h(px(GAP)))
            .child(reveal(p, next(), list))
            .when_some(status, |el, (ok, msg, _)| el.child(toast(ok, msg, palette)))
            .into_any_element()
    }
}

fn chip(m: &Shared, palette: &MandaraPalette, window: &Window) -> AnyElement {
    let (count, checking) = {
        let mb = m.borrow();
        mb.views.track(window);
        (mb.count, mb.checking)
    };
    let fg = if checking { palette.on_surface.with_alpha(CHIP_CHECKING_ICON_ALPHA) } else if count > 0 { palette.primary } else { palette.on_surface };
    div()
        .flex()
        .items_center()
        .gap(px(CHIP_GAP_PX))
        .child(icon("package", crate::style::bar_icon(CHIP_ICON_PX), fg))
        .when(count > 0, |el| {
            el.child(
                div()
                    .min_w(px(CHIP_BADGE_PX))
                    .h(px(CHIP_BADGE_PX))
                    .px(px(CHIP_BADGE_PAD_X_PX))
                    .rounded_full()
                    .bg(palette.primary.with_alpha(if checking { CHIP_CHECKING_BADGE_ALPHA } else { 1.0 }).hsla())
                    .text_color(palette.on_primary.hsla())
                    .font_features(tabular_nums())
                    .text_size(px(BADGE_TEXT_PX))
                    .font_weight(FontWeight::BOLD)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(count.min(BADGE_COUNT_MAX).to_string()),
            )
        })
        .into_any_element()
}

// ── Commands ─────────────────────────────────────────────────────────────

/// A blocking job on a thread, answered as a pending JSON text result.
fn pending(job: impl FnOnce() -> Result<String, String> + Send + 'static) -> Output {
    let (tx, rx) = async_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.try_send(job());
    });
    Output::Pending(rx)
}

fn check_now(cx: &mut App) -> Output {
    match alive() {
        Some(m) => {
            let (tx, rx) = async_channel::bounded(1);
            start_check(&m, Some(tx), cx);
            Output::Pending(rx)
        }
        None => {
            let opts = settings(cx).opts;
            pending(move || {
                let r = updates::check(opts);
                save_cache(&r);
                Ok(result_json(&r))
            })
        }
    }
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Updates"), tk("Pending package updates from pacman, the AUR, APT, DNF, Zypper and Flatpak"), None), |window, cx, panel_cx| {
        let m = shared(cx);
        // Typing re-renders this plugin's views only.
        let views = m.borrow().views.clone();
        let field = cx.new(|cx| TextField::new(String::new(), t("Filter packages…"), true, Rc::new(move |_, _, cx| views.notify(cx)), cx));
        let bar_m = m.clone();
        let bar = fn_view(None, move |palette, window, _| chip(&bar_m, palette, window), window, cx);
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(EMPTY_H + HEADER_H + PAD * 2.0 + GAP))));
        let secondary_m = m.clone();
        let view = View { p: panel_cx.clone(), m, field, size_cell: size_cell.clone(), watch: CloseWatch::default() };
        let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        PluginInstance {
            bar: Some(bar),
            panel: Some(PanelSpec { view: panel, size: size_cell }),
            // Right-click the chip: check now.
            on_secondary_click: Some(Rc::new(move |_, cx| start_check(&secondary_m, None, cx))),
            ..PluginInstance::empty()
        }
    });
    r.command(
        ID,
        CommandSpec::new_output("updates list", tk("Pending updates by source (the last check; runs one if there is none)"), |_, cx| {
            if let Some(r) = alive().and_then(|m| m.borrow().result.clone()) {
                return Ok(Output::Value(super::json(&*r)));
            }
            if let Some(r) = load_cache() {
                return Ok(Output::Value(super::json(r)));
            }
            Ok(check_now(cx))
        }),
    );
    r.command(ID, CommandSpec::new_output("updates check", tk("Check every package source now (read-only)"), |_, cx| Ok(check_now(cx))));
    r.command(
        ID,
        CommandSpec::new("updates sources", tk("Detected package sources, their checkers and update commands"), |_, cx| {
            let s = settings(cx);
            Ok(json!({ "sources": updates::detect(s.opts), "terminal": updates::resolve_terminal(&s.terminal) }))
        }),
    );
    r.command(
        ID,
        CommandSpec::new("updates run", tk("Open a terminal running the update command (all sources with updates, or one)"), |a, cx| {
            let s = settings(cx);
            let detected = updates::detect(s.opts);
            let helper = updates::aur_helper();
            let command = match a.get(0) {
                Some(id) => {
                    let kind = SourceKind::from_id(id).ok_or_else(|| tf("Unknown source `{source}`", &[("source", &id)]))?;
                    detected.iter().find(|d| d.kind == kind).map(|d| d.command.clone()).ok_or_else(|| tf("{source} is not available here", &[("source", &kind.label())]))?
                }
                None => {
                    let pending: Vec<SourceKind> = alive()
                        .and_then(|m| m.borrow().result.clone())
                        // The lists are freed while no panel is open.
                        .or_else(|| load_cache().map(Rc::new))
                        .map(|r| r.sources.iter().filter(|s| !s.packages.is_empty()).map(|s| s.kind).collect())
                        .filter(|k: &Vec<SourceKind>| !k.is_empty())
                        .unwrap_or_else(|| detected.iter().map(|d| d.kind).collect());
                    if pending.is_empty() {
                        return Err(t("No package manager found").into());
                    }
                    updates::combined_command(&pending, helper)
                }
            };
            let m = alive();
            let terminal = run_in_terminal(m.as_ref(), &command, cx)?;
            if let Some(m) = &m {
                notify(m, cx);
            }
            Ok(json!({ "terminal": terminal, "command": command }))
        })
        .arg(ArgSpec::optional("source", ArgKind::String, tk("one source only")).choices(&["pacman", "aur", "apt", "dnf", "zypper", "flatpak"]))
        .example("updates run flatpak"),
    );
    r.command(
        ID,
        CommandSpec::new_output("updates news", tk("Latest Arch Linux news headlines"), |_, _| {
            Ok(pending(|| updates::fetch_news().map(|items| serde_json::to_string_pretty(&items).unwrap_or_default())))
        }),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report(kind: SourceKind, names: &[&str]) -> SourceReport {
        SourceReport {
            kind,
            packages: names.iter().map(|n| Package { name: (*n).into(), old: "1.0".into(), new: "1.1".into(), repo: Some("extra".into()) }).collect(),
            error: None,
            hint: None,
            command: String::new(),
        }
    }

    #[test]
    fn filters_by_name_and_repo() {
        let r = report(SourceKind::Pacman, &["linux", "Linux-firmware", "mesa"]);
        assert_eq!(matching(&r, "LINUX").len(), 2);
        assert_eq!(matching(&r, "").len(), 3);
        assert_eq!(matching(&r, "extra").len(), 3);
        assert!(matching(&r, "zzz").is_empty());
    }

    #[test]
    fn first_group_with_packages_opens() {
        let result = CheckResult { sources: vec![report(SourceKind::Pacman, &[]), report(SourceKind::Flatpak, &["org.gnome.Platform"])], checked_at: 0 };
        assert_eq!(first_open(Some(&result)), Some(SourceKind::Flatpak));
        assert_eq!(first_open(None), None);
    }

    #[test]
    fn checked_label_buckets() {
        let now = now_secs();
        assert_eq!(checked_label(now), t("Checked just now"));
        assert!(checked_label(now - 5 * 60).contains('5'));
        assert!(checked_label(now - 2 * SECS_PER_HOUR).contains('2'));
    }

    #[test]
    fn cache_round_trips() {
        let r = CheckResult { sources: vec![report(SourceKind::Aur, &["paru"])], checked_at: 42 };
        let bytes = serde_json::to_vec(&r).unwrap();
        let back: CheckResult = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back, r);
        assert_eq!(back.total(), 1);
    }
}
