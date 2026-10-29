//! Authenticator plugin: TOTP / HOTP two-factor codes. Accounts live in the
//! Secret Service keyring (never in plaintext on disk); the panel lists them
//! with monograms, grouped codes and circular countdown rings, searches,
//! copies a code with a click (flagged as a secret and wiped from the
//! clipboard after a timeout) and adds accounts from a pasted `otpauth://`
//! link or a QR code scanned from a screen region.
//!
//! The keyring is opened only when the panel opens or a command needs it.
//! Settings (`[plugins.settings.authenticator]`): `clear_seconds`.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{AnyElement, App, AppContext as _, Entity, FontWeight, SharedString, InteractiveElement, IntoElement, ParentElement, Pixels, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px, size};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::authenticator::{self as otp, Account, AuthCommand, AuthService, AuthState, AuthStatus, OtpKind};
use mandara_theme::MandaraPalette;
use serde_json::json;

use super::extras::{CONTROL_H, Flash, caption, monogram, name_color, search_box, toast};
use super::fn_view;
use super::panel_fx::{Cached, HEADER_H, PanelShape, ViewSet, WASH_SOFT, chip, empty_state, glass_card, header, pill_button, readable, redraw_in};
use crate::fonts::{fonts, tabular_nums};
use crate::icons::{icon, ring};
use crate::kit::{icon_button, reveal, text};
use crate::text_field::TextField;

pub const ID: &str = "authenticator";

/// Panel height changes smaller than this don't resize the surface.
const RESIZE_EPSILON_PX: f32 = 0.5;

const PANEL_W: f32 = 404.0;
const PAD: f32 = 14.0;
const GAP: f32 = 12.0;
const ROW_H: f32 = 62.0;
const ROW_GAP: f32 = 4.0;
const ADD_H: f32 = 112.0;
const EMPTY_H: f32 = 200.0;
const FOOTER_H: f32 = 18.0;
/// Rows visible before the list scrolls (`visible_rows`).
pub const DEFAULT_VISIBLE_ROWS: i64 = 6;
pub const VISIBLE_ROWS_RANGE: (i64, i64) = (2, 16);

/// Rows visible before the list scrolls (`visible_rows`).
fn visible_rows(cx: &App) -> usize {
    crate::state::config(cx).read(cx).snapshot().plugins.setting(ID, "visible_rows", DEFAULT_VISIBLE_ROWS).clamp(VISIBLE_ROWS_RANGE.0, VISIBLE_ROWS_RANGE.1) as usize
}
/// Show the search box from this many accounts.
const SEARCH_FROM: usize = 4;
const RING_D: f32 = 28.0;
/// Countdown turns coral in the last seconds.
const URGENT_SECONDS: f64 = 6.0;
/// Countdown ring repaint cadence.
const TICK: Duration = Duration::from_millis(250);
/// A delete needs a second click within this window.
const ARM_FOR: Duration = Duration::from_secs(3);
/// A copied row shows its check this long.
const COPIED_FOR: Duration = Duration::from_millis(1600);
const MAX_CLEAR_SECONDS: i64 = 600;
/// Wait for the panel to slide away before freezing the screen for a scan.
const PANEL_CLEAR_DELAY: Duration = Duration::from_millis(280);
/// Ticks land just after the boundary they wait for.
const TICK_MARGIN: Duration = Duration::from_millis(5);
/// Room under a status empty state for its action button.
const STATUS_ACTION_H: f32 = 40.0;
/// Inner padding of the account list card (its height adds twice this).
const LIST_PAD_PX: f32 = 4.0;
/// Codes cached per account and time step (cleared when exceeded).
const CODE_CACHE_CAP: usize = 64;

// Countdown ring.
const RING_STROKE_PX: f32 = 2.5;
const RING_TRACK_ALPHA: f32 = 0.1;
const RING_TEXT_PX: f32 = 10.0;
const COUNTER_TEXT_PX: f32 = 9.5;

// Account rows.
const ROW_PAD_X_PX: f32 = 10.0;
/// Rows are this much tighter than the panel's concentric radius.
const ROW_RADIUS_INSET_PX: f32 = 4.0;
const ROW_GAP_X_PX: f32 = 11.0;
const ROW_HOVER_ALPHA: f32 = 0.04;
const MONOGRAM_PX: f32 = 36.0;
const TITLE_GAP_PX: f32 = 6.0;
const CHECK_ICON_PX: f32 = 12.0;
const CODE_TOP_PX: f32 = 1.0;
const CODE_TEXT_PX: f32 = 20.0;
const CODE_LINE_PX: f32 = 24.0;
const DELETE_PX: f32 = 24.0;
const ARMED_FILL_ALPHA: f32 = 0.14;

// "Add an account" card.
const ADD_PAD_PX: f32 = 12.0;
const ADD_GAP_PX: f32 = 10.0;
const ADD_TITLE_GAP_PX: f32 = 6.0;
const ADD_ICON_PX: f32 = 13.0;
const ADD_BUTTONS_GAP_PX: f32 = 8.0;

const BAR_ICON_PX: f32 = 15.0;

#[derive(Default)]
struct Model {
    service: Option<AuthService>,
    state: AuthState,
    search: Option<Entity<TextField>>,
    uri: Option<Entity<TextField>>,
    adding: bool,
    copied: Option<(String, Instant)>,
    armed: Option<(String, Instant)>,
    busy: bool,
    /// Grouped code per account id, keyed by its time step / counter: the
    /// HMAC runs once per step instead of on every frame.
    codes: HashMap<String, (u64, SharedString)>,
}

thread_local! {
    static MODEL: Rc<RefCell<Model>> = Rc::new(RefCell::new(Model::default()));
    /// The panel: keyring results notify just this view.
    static VIEWS: ViewSet = ViewSet::default();
}

fn model() -> Rc<RefCell<Model>> {
    MODEL.with(|m| m.clone())
}

fn views() -> ViewSet {
    VIEWS.with(ViewSet::clone)
}

/// The step a code is valid for: the TOTP window or the HOTP counter.
fn code_step(a: &Account, unix_secs: u64) -> u64 {
    match a.kind {
        OtpKind::Totp { period } => unix_secs / period.max(1),
        OtpKind::Hotp { counter } => counter,
    }
}

/// The grouped code of `a` at `unix_secs`, computed once per step.
fn cached_code(codes: &mut HashMap<String, (u64, SharedString)>, a: &Account, unix_secs: u64) -> SharedString {
    let step = code_step(a, unix_secs);
    if let Some((s, code)) = codes.get(&a.id)
        && *s == step
    {
        return code.clone();
    }
    if codes.len() >= CODE_CACHE_CAP {
        codes.clear();
    }
    let code = SharedString::from(otp::group_code(&a.code(unix_secs)));
    codes.insert(a.id.clone(), (step, code.clone()));
    code
}

/// The actor (spawned on first use; it stays idle until asked).
fn service(cx: &mut App) -> AuthService {
    let m = model();
    if let Some(s) = m.borrow().service.clone() {
        return s;
    }
    let s = otp::spawn_authenticator_service();
    m.borrow_mut().service = Some(s.clone());
    let updates = s.updates.clone();
    cx.spawn(async move |cx| {
        while let Ok(state) = updates.recv().await {
            model().borrow_mut().state = state;
            views().notify_async(cx);
        }
    })
    .detach();
    s
}

fn clear_after(cx: &App) -> Option<Duration> {
    let default = i64::try_from(otp::DEFAULT_CLEAR_SECONDS).unwrap_or(MAX_CLEAR_SECONDS);
    let secs = crate::state::config(cx).read(cx).get().plugins.setting(ID, "clear_seconds", default).clamp(0, MAX_CLEAR_SECONDS);
    (secs > 0).then(|| Duration::from_secs(secs.unsigned_abs()))
}

fn unix_now() -> Duration {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default()
}

/// Sends a command; failures surface as a toast.
fn send(command: AuthCommand, flash: Option<Flash>, cx: &mut App) {
    let rx = service(cx).request(command);
    cx.spawn(async move |cx| {
        if let Ok(result) = rx.recv().await {
            if let (Some(flash), Err(e)) = (flash, &result) {
                flash.show(false, e.clone());
            }
            views().notify_async(cx);
        }
    })
    .detach();
}

fn copy_code(account: &Account, flash: &Flash, cx: &mut App) {
    let code = account.code(unix_now().as_secs());
    let clear = clear_after(cx);
    match otp::copy_secret(&code, clear) {
        Ok(()) => {
            model().borrow_mut().copied = Some((account.id.clone(), Instant::now()));
            flash.show(true, match clear {
                Some(d) => tf("Code copied · clears in {n} s", &[("n", &d.as_secs())]),
                None => t("Code copied").to_string(),
            });
            // HOTP codes are single-use: advance the counter.
            if matches!(account.kind, OtpKind::Hotp { .. }) {
                send(AuthCommand::NextCounter(account.id.clone()), Some(flash.clone()), cx);
            }
        }
        Err(e) => flash.show(false, e),
    }
    views().notify(cx);
}

/// Adds every account found in a scanned image.
fn add_from_image(image: image::RgbaImage, flash: Flash, cx: &mut App) {
    let (w, h) = image.dimensions();
    let task = cx.background_executor().spawn(async move { otp::accounts_from_image(w, h, image.as_raw()) });
    cx.spawn(async move |cx| {
        match task.await {
            Ok(accounts) => {
                let n = accounts.len();
                cx.update(|cx| {
                    for a in accounts {
                        send(AuthCommand::Add(a), Some(flash.clone()), cx);
                    }
                    flash.show(true, tf("Added {n} accounts from the QR code", &[("n", &n)]));
                    model().borrow_mut().adding = false;
                    crate::state::shell(cx).update(cx, |s, cx| s.set_panel(Some(ID.into()), cx));
                    views().notify(cx);
                });
            }
            Err(e) => {
                flash.show(false, e);
                cx.update(|cx| {
                    crate::state::shell(cx).update(cx, |s, cx| s.set_panel(Some(ID.into()), cx));
                    views().notify(cx);
                });
            }
        }
    })
    .detach();
}

fn scan(flash: Flash, cx: &mut App) {
    crate::state::shell(cx).update(cx, |s, cx| s.set_panel(None, cx));
    cx.spawn(async move |cx| {
        cx.background_executor().timer(PANEL_CLEAR_DELAY).await;
        cx.update(|cx| super::screen_pick::pick_region(cx, move |image, cx| add_from_image(image, flash, cx)));
    })
    .detach();
}

// ── Views ────────────────────────────────────────────────────────────────

fn status_view(status: &AuthStatus, flash: &Flash, palette: &MandaraPalette) -> Option<AnyElement> {
    let accent = readable(palette.primary, palette);
    let retry = |label: &'static str| {
        let flash = flash.clone();
        pill_button("auth-retry", Some("refresh"), label, accent, palette, move |_, cx| send(AuthCommand::Load, Some(flash.clone()), cx)).into_any_element()
    };
    let (icon_name, title, hint, action): (&'static str, &'static str, SharedString, Option<AnyElement>) = match status {
        AuthStatus::Idle | AuthStatus::Loading => ("lock-open", t("Opening your keyring…"), t("Your keyring may ask you to unlock it").into(), None),
        AuthStatus::Locked => ("lock", t("The keyring is locked"), t("Unlock it to see your codes").into(), Some(retry(t("Unlock")))),
        AuthStatus::Unavailable(e) => ("alert", t("No keyring service"), tf("Install gnome-keyring or KeePassXC (Secret Service) · {error}", &[("error", e)]).into(), Some(retry(t("Retry")))),
        AuthStatus::Error(e) => ("alert", t("Couldn't read the keyring"), e.clone().into(), Some(retry(t("Retry")))),
        AuthStatus::Ready => return None,
    };
    Some(
        div()
            .flex()
            .flex_col()
            .items_center()
            .child(empty_state(icon_name, title, hint, EMPTY_H - STATUS_ACTION_H, palette))
            .when_some(action, |el, a| el.child(div().h(px(STATUS_ACTION_H)).flex().items_center().child(a)))
            .into_any_element(),
    )
}

fn countdown(account: &Account, now_ms: u64, palette: &MandaraPalette) -> AnyElement {
    match account.kind {
        OtpKind::Totp { period } => {
            let left = otp::seconds_left(now_ms, period);
            let frac = (left / period as f64) as f32;
            let urgent = left <= URGENT_SECONDS;
            let tint = readable(if urgent { palette.urgent_coral } else { palette.primary }, palette);
            div()
                .relative()
                .size(px(RING_D))
                .flex_none()
                .child(ring(frac, RING_D, RING_STROKE_PX, tint, palette.on_surface.with_alpha(RING_TRACK_ALPHA)))
                .child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .font_features(tabular_nums())
                        .text_size(px(RING_TEXT_PX))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(tint.hsla())
                        // `left` is in (0, period]: the cast cannot truncate.
                        .child(seconds_text(left.ceil() as u64)),
                )
                .into_any_element()
        }
        OtpKind::Hotp { counter } => div()
            .size(px(RING_D))
            .flex_none()
            .rounded_full()
            .bg(palette.on_surface.with_alpha(WASH_SOFT.0).hsla())
            .flex()
            .items_center()
            .justify_center()
            .font_features(tabular_nums())
            .text_size(px(COUNTER_TEXT_PX))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(palette.on_surface_variant.hsla())
            .child(format!("#{counter}"))
            .into_any_element(),
    }
}

/// Countdown seconds `0..=MAX` as shared labels, formatted once per thread.
fn seconds_text(secs: u64) -> SharedString {
    /// Labels kept; longer TOTP periods format on demand.
    const LABELS: u64 = 121;
    thread_local! {
        static TEXT: Vec<SharedString> = (0..LABELS).map(|s| SharedString::from(s.to_string())).collect();
    }
    if secs < LABELS {
        // `secs < LABELS`: the cast cannot truncate.
        TEXT.with(|t| t[secs as usize].clone())
    } else {
        secs.to_string().into()
    }
}

#[allow(clippy::too_many_arguments)]
fn account_row(index: usize, a: &Account, code: SharedString, now: Duration, copied: bool, armed: bool, flash: &Flash, mono: SharedString, palette: &MandaraPalette, radius: f32) -> AnyElement {
    let tint = name_color(if a.issuer.is_empty() { &a.label } else { &a.issuer }, palette);
    let (title, sub) = if a.issuer.is_empty() { (a.label.clone(), String::new()) } else { (a.issuer.clone(), a.label.clone()) };
    let emerald = readable(palette.success_emerald, palette);
    let coral = readable(palette.urgent_coral, palette);
    let account = a.clone();
    let id = a.id.clone();
    let flash_copy = flash.clone();
    let flash_del = flash.clone();
    div()
        .id(("auth-row", index))
        .group("auth-row")
        .h(px(ROW_H))
        .px(px(ROW_PAD_X_PX))
        .rounded(px(crate::style::concentric(radius, ROW_RADIUS_INSET_PX)))
        .flex()
        .items_center()
        .gap(px(ROW_GAP_X_PX))
        .cursor_pointer()
        .hover(|s| s.bg(palette.on_surface.with_alpha(ROW_HOVER_ALPHA).hsla()))
        .child(monogram(&a.initials(), tint, MONOGRAM_PX))
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(TITLE_GAP_PX))
                        .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::SEMIBOLD).truncate().child(title))
                        .when(copied, |el| el.child(icon("check", CHECK_ICON_PX, emerald))),
                )
                .when(!sub.is_empty(), |el| el.child(caption(sub, palette).truncate()))
                .child(div().mt(px(CODE_TOP_PX)).font_family(mono).font_features(tabular_nums()).text_size(px(CODE_TEXT_PX)).line_height(px(CODE_LINE_PX)).font_weight(FontWeight::MEDIUM).text_color(if copied { emerald.hsla() } else { palette.on_surface.hsla() }).child(code)),
        )
        .child(
            div().w(px(DELETE_PX)).flex_none().when(!armed, |el| el.invisible().group_hover("auth-row", |s| s.visible())).child(icon_button(
                // Element ids are scoped by the row's id.
                "auth-del",
                "trash",
                DELETE_PX,
                if armed { coral } else { palette.on_surface_variant },
                armed.then(|| coral.with_alpha(ARMED_FILL_ALPHA)),
                move |_, cx| {
                    cx.stop_propagation();
                    let m = model();
                    let confirm = m.borrow().armed.as_ref().is_some_and(|(i, at)| *i == id && at.elapsed() < ARM_FOR);
                    if confirm {
                        m.borrow_mut().armed = None;
                        send(AuthCommand::Remove(id.clone()), Some(flash_del.clone()), cx);
                        flash_del.show(true, t("Account removed from the keyring"));
                    } else {
                        m.borrow_mut().armed = Some((id.clone(), Instant::now()));
                        flash_del.show(false, t("Click the bin again to remove this account"));
                    }
                    views().notify(cx);
                },
            )),
        )
        .child(countdown(a, u64::try_from(now.as_millis()).unwrap_or(u64::MAX), palette))
        .on_click(move |_, _, cx| copy_code(&account, &flash_copy, cx))
        .into_any_element()
}

fn add_card(uri: &Entity<TextField>, flash: &Flash, palette: &MandaraPalette, radius: f32) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let field = uri.clone();
    let flash_add = flash.clone();
    let flash_scan = flash.clone();
    glass_card(palette)
        .rounded(px(radius))
        .h(px(ADD_H))
        .p(px(ADD_PAD_PX))
        .flex()
        .flex_col()
        .gap(px(ADD_GAP_PX))
        .child(div().flex().items_center().gap(px(ADD_TITLE_GAP_PX)).child(icon("plus", ADD_ICON_PX, accent)).child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::SEMIBOLD).child(t("Add an account"))).child(div().flex_1()).child(caption(t("Ctrl+V pastes"), palette)))
        .child(div().h(px(CONTROL_H)).child(uri.clone()))
        .child(
            div()
                .flex()
                .gap(px(ADD_BUTTONS_GAP_PX))
                .child(pill_button("auth-add", Some("check"), t("Add link"), accent, palette, move |_, cx| {
                    let text = field.read(cx).value.clone();
                    match otp::parse_uri(&text) {
                        Ok(account) => {
                            let title = account.title();
                            send(AuthCommand::Add(account), Some(flash_add.clone()), cx);
                            field.update(cx, |f, _| f.set(""));
                            model().borrow_mut().adding = false;
                            flash_add.show(true, tf("Added {account}", &[("account", &title)]));
                        }
                        Err(e) => flash_add.show(false, e),
                    }
                    views().notify(cx);
                }))
                .child(pill_button("auth-scan", Some("crop"), t("Scan QR from screen"), readable(palette.tertiary, palette), palette, move |_, cx| scan(flash_scan.clone(), cx))),
        )
        .into_any_element()
}

fn panel_height(rows: usize, visible: usize, adding: bool, search: bool, status: bool) -> f32 {
    let head = PAD * 2.0 + HEADER_H;
    let add = if adding { GAP + ADD_H } else { 0.0 };
    let search_h = if search { GAP + CONTROL_H } else { 0.0 };
    let body = if status || rows == 0 { EMPTY_H } else { rows.min(visible) as f32 * (ROW_H + ROW_GAP) + LIST_PAD_PX * 2.0 };
    head + add + search_h + GAP + body + GAP + FOOTER_H
}

struct View {
    p: PanelCx,
    size_cell: Rc<Cell<gpui::Size<Pixels>>>,
    flash: Flash,
    /// Footer hint keyed by the clipboard clear timeout.
    footer: RefCell<Cached<Option<u64>>>,
}

/// Creates the panel's text fields on first render; returns `(search, uri)`.
fn fields(m: &Rc<RefCell<Model>>, cx: &mut App) -> (Entity<TextField>, Entity<TextField>) {
    let mut mm = m.borrow_mut();
    let search = mm
        .search
        .get_or_insert_with(|| cx.new(|cx| TextField::new(String::new(), t("Search accounts…"), true, Rc::new(|_, _, cx| views().notify(cx)), cx)))
        .clone();
    let uri = mm.uri.get_or_insert_with(|| cx.new(|cx| TextField::new(String::new(), t("otpauth://totp/…"), true, Rc::new(|_, _, cx| views().notify(cx)), cx))).clone();
    (search, uri)
}

impl View {
    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        views().track(window);
        let visible = self.p.progress() > 0.0;
        let m = model();
        if visible {
            service(cx);
            if m.borrow().state.status == AuthStatus::Idle && !m.borrow().busy {
                m.borrow_mut().busy = true;
                let rx = service(cx).request(AuthCommand::Load);
                cx.spawn(async move |cx| {
                    let _ = rx.recv().await;
                    model().borrow_mut().busy = false;
                    views().notify_async(cx);
                })
                .detach();
            }
            // Repaint the countdown rings on tick boundaries, only while open.
            let tick_ms = TICK.as_millis();
            let into_tick = Duration::from_millis(u64::try_from(unix_now().as_millis() % tick_ms).unwrap_or(0));
            redraw_in(TICK.saturating_sub(into_tick) + TICK_MARGIN, window, cx);
        }
        let (search, uri) = fields(&m, cx);
        {
            let mut mm = m.borrow_mut();
            if mm.copied.as_ref().is_some_and(|(_, at)| at.elapsed() >= COPIED_FOR) {
                mm.copied = None;
            }
            if mm.armed.as_ref().is_some_and(|(_, at)| at.elapsed() >= ARM_FOR) {
                mm.armed = None;
            }
        }
        let mut mm = m.borrow_mut();
        let query = search.read(cx).value.clone();
        let Model { state, codes, adding, copied, armed, .. } = &mut *mm;
        let adding = *adding;
        let ready = state.status == AuthStatus::Ready;
        // `(index in the full list, account)`: indices give rows stable ids.
        let accounts: Vec<(usize, &Account)> = state.accounts.iter().enumerate().filter(|(_, a)| a.matches(&query)).collect();
        let show_search = ready && state.accounts.len() >= SEARCH_FROM;
        let visible = visible_rows(cx);
        let h = panel_height(accounts.len(), visible, adding, show_search, !ready);
        if (f32::from(self.size_cell.get().height) - h).abs() > RESIZE_EPSILON_PX {
            self.size_cell.set(size(px(PANEL_W), px(h)));
        }
        let radius = PanelShape::of(cx).inner(PAD);
        let f = fonts(cx);
        let accent = readable(palette.primary, palette);
        let subtitle: SharedString = if state.demo {
            t("Demo data — not your accounts").into()
        } else if ready {
            tf("{n} accounts · {backend}", &[("n", &state.accounts.len()), ("backend", &state.backend)]).into()
        } else {
            t("Secret Service keyring").into()
        };
        let trailing: AnyElement = if state.demo {
            chip("alert", t("Demo"), readable(palette.warning_amber, palette), palette).into_any_element()
        } else {
            pill_button("auth-toggle-add", Some(if adding { "close" } else { "plus" }), if adding { t("Close") } else { t("Add") }, accent, palette, move |_, cx| {
                let m = model();
                let open = !m.borrow().adding;
                m.borrow_mut().adding = open;
                views().notify(cx);
            })
            .into_any_element()
        };
        let head = header("shield", accent, t("Authenticator"), subtitle, palette, Some(trailing));
        let p = &self.p;
        let mut col = div().relative().size_full().p(px(PAD)).flex().flex_col().child(reveal(p, 0, head));
        if adding && !state.demo {
            col = col.child(div().h(px(GAP))).child(reveal(p, 1, add_card(&uri, &self.flash, palette, radius)));
        }
        if show_search {
            col = col.child(div().h(px(GAP))).child(reveal(p, 2, div().flex().child(search_box("auth-search-clear", &search, &query, palette))));
        }
        let body: AnyElement = if let Some(s) = status_view(&state.status, &self.flash, palette) {
            s
        } else if state.accounts.is_empty() {
            empty_state("shield", t("No accounts yet"), t("Add one from an otpauth:// link or a QR code"), EMPTY_H, palette).into_any_element()
        } else if accounts.is_empty() {
            empty_state("search", tf("Nothing matches \u{201c}{query}\u{201d}", &[("query", &query.trim())]), t("Search by service or account name"), EMPTY_H, palette).into_any_element()
        } else {
            let now = unix_now();
            let copied = copied.as_ref().map(|(id, _)| id.as_str());
            let armed = armed.as_ref().map(|(id, _)| id.as_str());
            let rows: Vec<AnyElement> = accounts
                .iter()
                .map(|(i, a)| {
                    let code = cached_code(codes, a, now.as_secs());
                    account_row(*i, a, code, now, copied == Some(a.id.as_str()), armed == Some(a.id.as_str()), &self.flash, f.mono.clone(), palette, radius)
                })
                .collect();
            let list_h = accounts.len().min(visible) as f32 * (ROW_H + ROW_GAP);
            glass_card(palette).rounded(px(radius)).p(px(LIST_PAD_PX)).child(div().id("auth-list").h(px(list_h)).overflow_y_scroll().flex().flex_col().gap(px(ROW_GAP)).children(rows)).into_any_element()
        };
        let clear = clear_after(cx);
        let footer = caption(
            self.footer.borrow_mut().get(clear.map(|d| d.as_secs()), |secs| match secs {
                Some(n) => tf("Click a code to copy it · the clipboard clears after {n} s", &[("n", n)]),
                None => t("Click a code to copy it").to_string(),
            }),
            palette,
        );
        drop(mm);
        col.child(div().h(px(GAP)))
            .child(reveal(p, 3, body))
            .child(div().h(px(GAP)))
            .child(reveal(p, 4, div().h(px(FOOTER_H)).flex().items_center().justify_center().child(footer)))
            .when_some(self.flash.current(window, cx), |el, (ok, msg)| el.child(toast(ok, msg, PAD, palette)))
            .into_any_element()
    }
}

/// A command whose reply comes from the actor.
fn request(command: AuthCommand, cx: &mut App) -> Output {
    Output::Pending(service(cx).request(command))
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Authenticator"), tk("Two-factor codes (TOTP / HOTP) stored in your keyring"), Some(Zone::Right)), |window, cx, panel_cx| {
        let bar = fn_view(None, |palette, _, _| icon("shield", crate::style::bar_icon(BAR_ICON_PX), palette.on_surface).into_any_element(), window, cx);
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(panel_height(0, DEFAULT_VISIBLE_ROWS as usize, false, false, true)))));
        let view = View { p: panel_cx.clone(), size_cell: size_cell.clone(), flash: Flash::default(), footer: RefCell::default() };
        let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), ..PluginInstance::empty() }
    });
    r.command(
        ID,
        CommandSpec::new_output("authenticator list", tk("Accounts in the keyring (issuer, label, type — never secrets); may prompt to unlock"), |_, cx| Ok(request(AuthCommand::Load, cx))),
    );
    r.command(
        ID,
        CommandSpec::new_output("authenticator code", tk("Current codes of accounts matching a query"), |a, cx| Ok(request(AuthCommand::Codes(a.rest(0)), cx)))
            .arg(ArgSpec::required("query", ArgKind::String, tk("issuer or account name")))
            .example("authenticator code github"),
    );
    r.command(
        ID,
        CommandSpec::new_output("authenticator copy", tk("Copy the first matching account's code (cleared from the clipboard after the timeout)"), |a, cx| {
            let rx = service(cx).request(AuthCommand::Codes(a.rest(0)));
            let clear = clear_after(cx);
            let (tx, out) = async_channel::bounded(1);
            cx.background_executor()
                .spawn(async move {
                    let result = match rx.recv().await {
                        Ok(Ok(body)) => serde_json::from_str::<serde_json::Value>(&body).map_err(|e| e.to_string()).and_then(|v| {
                            let first = v.get(0).cloned().unwrap_or_default();
                            let code = first["code"].as_str().unwrap_or_default().to_string();
                            otp::copy_secret(&code, clear).map(|_| json!({ "issuer": first["issuer"], "label": first["label"], "copied": true, "clears_after_s": clear.map(|d| d.as_secs()) }).to_string())
                        }),
                        Ok(Err(e)) => Err(e),
                        Err(_) => Err(t("The service did not reply").into()),
                    };
                    let _ = tx.send(result).await;
                })
                .detach();
            Ok(Output::Pending(out))
        })
        .arg(ArgSpec::required("query", ArgKind::String, tk("issuer or account name")))
        .example("authenticator copy github"),
    );
    r.command(
        ID,
        CommandSpec::new_output("authenticator add", tk("Add an account from an otpauth:// link (stored in the keyring)"), |a, cx| {
            let account = otp::parse_uri(&a.rest(0))?;
            Ok(request(AuthCommand::Add(account), cx))
        })
        .arg(ArgSpec::required("uri", ArgKind::String, tk("otpauth://totp/Issuer:name?secret=…")))
        .example("authenticator add otpauth://totp/Example:me?secret=JBSWY3DPEHPK3PXP&issuer=Example"),
    );
    r.command(
        ID,
        CommandSpec::new_output("authenticator remove", tk("Remove an account (by id from `authenticator list`) from the keyring"), |a, cx| Ok(request(AuthCommand::Remove(a.str(0)?.to_string()), cx)))
            .arg(ArgSpec::required("id", ArgKind::String, tk("account id")))
            .example("authenticator remove 3f2a9c0d1b2e4f56"),
    );
    r.command(
        ID,
        CommandSpec::new("authenticator scan", tk("Select a screen region with a QR code and add the account it holds"), |_, cx| {
            let flash = Flash::default();
            scan(flash, cx);
            Ok(json!("opened"))
        }),
    );
    r.command(
        ID,
        CommandSpec::new_output("authenticator demo", tk("Show clearly-labelled demo accounts instead of the keyring (on/off)"), |a, cx| {
            let on = match a.get(0) {
                Some(_) => a.bool(0)?,
                None => !model().borrow().state.demo,
            };
            Ok(request(AuthCommand::Demo(on), cx))
        })
        .arg(ArgSpec::optional("on", ArgKind::Bool, tk("on or off (default: toggle)")))
        .example("authenticator demo on"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn panel_height_scrolls_after_visible_rows() {
        let a = panel_height(6, 6, false, true, false);
        assert_eq!(panel_height(6 + 5, 6, false, true, false), a);
        assert!(panel_height(2, 6, true, false, false) > panel_height(2, 6, false, false, false));
        assert_eq!(panel_height(0, 6, false, false, true), panel_height(0, 6, false, false, false));
    }

    #[test]
    fn countdown_labels_are_shared() {
        assert_eq!(seconds_text(7).as_ref(), "7");
        assert_eq!(seconds_text(500).as_ref(), "500");
    }
}
