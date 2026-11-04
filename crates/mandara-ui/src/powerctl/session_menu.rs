//! The power / session menu (`[session_menu]`): your entries in your order
//! (lock, log out, sleep, hibernate, restart, shut down, soft restart,
//! firmware setup, a boot-loader entry, restart Mandara, or any command),
//! shown as the edge dock or a centered grid, with optional hold-to-confirm
//! for everything that ends the session and number-key shortcuts. The dock
//! opens on a screen side, or hangs from the top or bottom centre
//! (`[session_menu] position`): from the bar on that edge, one piece with
//! it in the edge-frame styles, the way the launcher and the drop-down
//! panels join it ([`crate::launcher_place`]).
//!
//! Choosing an entry runs its before-hooks (`before-logout`,
//! `before-reboot`, `before-shutdown`) to completion first.

use std::time::{Duration, Instant};

use gpui::{
    App, ClickEvent, Context, Entity, FocusHandle, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent, MouseButton, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px,
};
use mandara_config::ShellConfig;
use mandara_config::power::{HookEvent, SessionEntry, SessionEntryKind, SessionMenuConfig, SessionMenuPosition, SessionMenuStyle, SuspendMode};
use mandara_config::workflow::{LauncherEdge, LauncherPosition};
use mandara_core::{Edge, FrameClock, SpringConfig, SpringValue, ToGpui, edge_drawer_snapped};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginRegistry};
use mandara_services::{SessionAction, hooks, sleep};
use mandara_theme::Color;
use serde_json::json;

use crate::bar::layout::Metrics;
use crate::first_frame::{self, FirstFrame, Settle};
use crate::launcher_place::{self, Join, Placement, Rect};
use crate::fonts::{fonts, tabular_nums};
use crate::icons::any_icon;
use crate::kit::text;
use crate::shell::frame_color;
use crate::state::{ShellState, config, shell};

/// Owner id of the session commands.
const OWNER: &str = "session";

// Dock geometry (logical px).
const BTN_PX: f32 = 72.0;
const DOCK_PAD_PX: f32 = 14.0;
const DOCK_GAP_PX: f32 = 12.0;
const DOCK_ICON_PX: f32 = 26.0;
/// The floating label beside the dock.
const LABEL_W_PX: f32 = 168.0;
const LABEL_H_PX: f32 = 32.0;
const LABEL_GAP_PX: f32 = 16.0;
const LABEL_PAD_X_PX: f32 = 14.0;
/// How far from the dock the pointer may wander before it closes.
const DOCK_REACH_PX: f32 = 170.0;
const DOCK_REACH_Y_PX: f32 = 24.0;
// Grid geometry (logical px).
const TILE_PX: f32 = 112.0;
const TILE_ICON_PX: f32 = 30.0;
const GRID_GAP_PX: f32 = 14.0;
const GRID_PAD_PX: f32 = 22.0;
const GRID_TITLE_GAP_PX: f32 = 14.0;
const GRID_RADIUS_PX: f32 = 28.0;
const TILE_LABEL_GAP_PX: f32 = 8.0;
// Button corner radii: pressed, resting, focused.
const RADIUS_PRESSED_PX: f32 = 12.0;
const RADIUS_REST_PX: f32 = 20.0;
const RADIUS_FOCUSED_PX: f32 = 30.0;
/// Shortcut badge.
const BADGE_PX: f32 = 18.0;
const BADGE_INSET_PX: f32 = 5.0;
/// Scrim darkness when fully open.
const SCRIM_ALPHA: f32 = 0.45;
/// Grid columns bounds.
const GRID_COLUMNS_MAX: usize = 6;
// Shared focus-out settle (see `crate::dismiss`).
use crate::dismiss::DEACTIVATE_SETTLE;
/// The hold ring fills a little past the end so the action fires visibly full.
const HOLD_DONE: f32 = 1.0;
/// Hold progress fill alpha.
const HOLD_FILL_ALPHA: f32 = 0.42;
/// Opacity ramps of the buttons and the label during the slide.
const BUTTONS_FADE: (f32, f32) = (0.2, 0.6);
const LABEL_FADE: (f32, f32) = (0.5, 0.5);
/// Closed below this slide progress.
const CLOSED_EPSILON: f32 = 0.01;
/// Slide overshoot cap (springy entrance without flying off).
const SLIDE_OVERSHOOT: f32 = 1.2;
const OPEN_SPRING: SpringConfig = SpringConfig::new(280.0, 22.0, 1.0);
const CLOSE_STIFFNESS: f32 = 420.0;
const RADIUS_SPRING: SpringConfig = SpringConfig::new(420.0, 26.0, 1.0);

/// The built-in name of an entry kind (English key; display with `t`).
pub fn kind_label(kind: SessionEntryKind) -> &'static str {
    match kind {
        SessionEntryKind::Lock => tk("Lock"),
        SessionEntryKind::Logout => tk("Log out"),
        SessionEntryKind::Suspend => tk("Sleep"),
        SessionEntryKind::Hibernate => tk("Hibernate"),
        SessionEntryKind::Reboot => tk("Restart"),
        SessionEntryKind::Shutdown => tk("Shut down"),
        SessionEntryKind::SoftReboot => tk("Soft restart"),
        SessionEntryKind::Firmware => tk("Firmware setup"),
        SessionEntryKind::BootEntry => tk("Restart into…"),
        SessionEntryKind::RestartShell => tk("Restart Mandara"),
        SessionEntryKind::Custom => tk("Custom command"),
    }
}

/// The built-in icon of an entry kind.
pub fn kind_icon(kind: SessionEntryKind) -> &'static str {
    match kind {
        SessionEntryKind::Lock => "lock",
        SessionEntryKind::Logout => "logout",
        SessionEntryKind::Suspend => "moon",
        SessionEntryKind::Hibernate => "hibernate",
        SessionEntryKind::Reboot => "restart",
        SessionEntryKind::Shutdown => "power",
        SessionEntryKind::SoftReboot => "refresh",
        SessionEntryKind::Firmware => "cpu",
        SessionEntryKind::BootEntry => "server",
        SessionEntryKind::RestartShell => "window",
        SessionEntryKind::Custom => "terminal",
    }
}

/// The label shown for an entry.
pub fn entry_label(e: &SessionEntry) -> String {
    if !e.label.trim().is_empty() {
        return mandara_i18n::tr(e.label.trim());
    }
    match e.kind {
        SessionEntryKind::BootEntry if !e.boot_entry.is_empty() => tf("Restart into {entry}", &[("entry", &e.boot_entry)]),
        SessionEntryKind::Custom if !e.command.is_empty() => e.command.clone(),
        kind => t(kind_label(kind)).to_string(),
    }
}

/// The icon shown for an entry (a bundled name).
pub fn entry_icon(e: &SessionEntry) -> String {
    if e.icon.trim().is_empty() { kind_icon(e.kind).to_string() } else { e.icon.trim().to_string() }
}

/// Before-hook of an entry, if it ends the session.
fn before_hook(kind: SessionEntryKind) -> Option<HookEvent> {
    match kind {
        SessionEntryKind::Logout => Some(HookEvent::BeforeLogout),
        SessionEntryKind::Reboot | SessionEntryKind::SoftReboot | SessionEntryKind::Firmware | SessionEntryKind::BootEntry => Some(HookEvent::BeforeReboot),
        SessionEntryKind::Shutdown => Some(HookEvent::BeforeShutdown),
        _ => None,
    }
}

/// Carries out an entry (off the UI thread for anything blocking). Dry runs
/// (`MANDARA_POWER_DRY_RUN=1`) only log.
pub fn run(entry: &SessionEntry, cfg: &ShellConfig, cx: &mut App) -> Result<(), String> {
    let entry = entry.clone();
    if entry.kind == SessionEntryKind::RestartShell {
        let registry = cx.global::<PluginRegistry>().commands.clone();
        return registry.execute(&["restart".to_string()], cx).map(|_| ());
    }
    if entry.kind == SessionEntryKind::Custom {
        let timeout = Duration::from_secs(u64::from(cfg.hooks.timeout_s.max(1)));
        hooks::run_command("session-menu", &entry.command, &[], timeout);
        return Ok(());
    }
    if entry.kind == SessionEntryKind::BootEntry && entry.boot_entry.trim().is_empty() {
        return Err(t("Choose a boot entry for this item in Studio → Power & idle").into());
    }
    let suspend_mode = cfg.idle.suspend_mode;
    let own_lock_screen = cfg.general.lock_screen;
    let (label, icon) = (entry_label(&entry), kind_icon(entry.kind));
    let (done, outcome) = async_channel::bounded::<String>(1);
    std::thread::Builder::new()
        .name("mandara-session".into())
        .spawn(move || {
            if let Some(event) = before_hook(entry.kind) {
                hooks::emit_and_wait(event, &[("action", entry.kind.id().to_string())]);
            }
            if sleep::dry_run() && entry.kind != SessionEntryKind::Lock {
                tracing::info!("dry run: session {}", entry.kind.id());
                return;
            }
            let result = match entry.kind {
                // logind tells every locker, Mandara's included; when it
                // can't be reached, Mandara's own lock screen locks anyway.
                SessionEntryKind::Lock => SessionAction::Lock.run().or_else(|e| if own_lock_screen { crate::lock::lock_now().map(|_| ()) } else { Err(e) }),
                SessionEntryKind::Logout => SessionAction::Logout.run(),
                SessionEntryKind::Suspend => sleep::sleep(suspend_mode),
                SessionEntryKind::Hibernate => sleep::sleep(SuspendMode::Hibernate),
                SessionEntryKind::Reboot => SessionAction::Reboot.run(),
                SessionEntryKind::Shutdown => SessionAction::Shutdown.run(),
                SessionEntryKind::SoftReboot => sleep::soft_reboot(),
                SessionEntryKind::Firmware => sleep::reboot_to_firmware(),
                SessionEntryKind::BootEntry => sleep::reboot_to_entry(&entry.boot_entry),
                SessionEntryKind::RestartShell | SessionEntryKind::Custom => Ok(()),
            };
            if let Err(e) = result {
                tracing::error!("session {}: {e}", entry.kind.id());
                let _ = done.send_blocking(e);
            }
        })
        .map_err(|e| e.to_string())?;
    // The menu has closed by now: a failure (polkit said no, logind or the
    // compositor refused) shows as a notification instead of nothing.
    cx.spawn(async move |cx| {
        if let Ok(error) = outcome.recv().await {
            cx.update(|cx| failed(&label, icon, error, cx));
        }
    })
    .detach();
    Ok(())
}

/// Tells the user that a session action didn't happen, and why.
fn failed(label: &str, icon: &str, error: String, cx: &mut App) {
    let summary = tf("{action} didn't work", &[("action", &label)]);
    crate::plugins::notifications::send(cx, mandara_services::NotificationCommand::Post { app: t("Session").into(), icon: icon.into(), summary, body: error });
}

/// Visible entries and the index selected on opening.
fn visible(menu: &SessionMenuConfig) -> (Vec<SessionEntry>, usize) {
    let entries: Vec<SessionEntry> = menu.visible().cloned().collect();
    let entries = if entries.is_empty() { vec![SessionEntry::new(SessionEntryKind::Lock, true)] } else { entries };
    let selected = entries.iter().position(|e| e.kind.id() == menu.default_action).unwrap_or(0);
    (entries, selected)
}

/// Grid columns for `count` entries (never more than entries).
fn grid_columns(menu: &SessionMenuConfig, count: usize) -> usize {
    usize::from(menu.grid_columns).clamp(1, GRID_COLUMNS_MAX).min(count.max(1))
}

/// Where the dock style opens.
#[derive(Copy, Clone, Debug, PartialEq)]
enum DockPlace {
    /// Docked to a screen side, as it always was.
    Side { on_left: bool },
    /// Top or bottom centre: the launcher's attached placement for a
    /// column `size` big, so it hangs from the bar like the launcher and
    /// the drop-down panels (flares in the edge-frame styles, a card a gap
    /// under a floating bar, the bare screen edge where there is no bar).
    Hang(Placement),
}

/// Where the dock `size` big opens on a `screen`-sized output for
/// `position`, with the bar `bar` (shown on this output when `bar_here`),
/// the sidebar on the right when `sidebar_right` and a bottom dock keeping
/// `dock_clearance` free.
fn dock_place(position: SessionMenuPosition, sidebar_right: bool, bar: Metrics, bar_here: bool, dock_clearance: Option<f32>, screen: (f32, f32), size: (f32, f32)) -> DockPlace {
    let at = match position {
        // The edge opposite the sidebar.
        SessionMenuPosition::Side => return DockPlace::Side { on_left: sidebar_right },
        SessionMenuPosition::Left => return DockPlace::Side { on_left: true },
        SessionMenuPosition::Right => return DockPlace::Side { on_left: false },
        SessionMenuPosition::Top => LauncherPosition::TopCenter,
        SessionMenuPosition::Bottom => LauncherPosition::BottomCenter,
    };
    DockPlace::Hang(launcher_place::place(&launcher_place::Inputs {
        position: at,
        screen,
        size,
        max_h: size.1,
        bar,
        bar_here,
        dock_clearance,
        button: None,
        animation: mandara_config::surfaces::PanelAnimation::Slide,
    }))
}

/// A hanging dock part way open (`p`, past 1 while the spring overshoots):
/// the drawer's visible rect (it grows out of its edge, stretching through
/// the overshoot rather than leave the edge) and the top of its content,
/// which slides out from behind the edge.
fn hang_frame(rect: Rect, edge: LauncherEdge, p: f32) -> (Rect, f32) {
    let visible = rect.h * p.max(0.0);
    let content = rect.y + rect.h * (p - 1.0) * if edge == LauncherEdge::Top { 1.0 } else { -1.0 };
    match edge {
        LauncherEdge::Top => (Rect::new(rect.x, rect.y, rect.w, visible), content),
        LauncherEdge::Bottom => (Rect::new(rect.x, rect.bottom() - visible, rect.w, visible), content),
    }
}

/// Uses the connector selected when the overlay opened before a size fallback.
fn bar_shows_here(output: Option<&str>, vp: (f32, f32), cx: &App) -> bool {
    let outputs = cx.try_global::<crate::overlays::Outputs>().map(|o| o.0.as_slice()).unwrap_or_default();
    let cfg = config(cx).read(cx).snapshot();
    bar_on_target(&cfg, outputs, output, vp)
}

fn bar_on_target(cfg: &mandara_config::ShellConfig, outputs: &[mandara_core::OutputGeometry], output: Option<&str>, vp: (f32, f32)) -> bool {
    let target = output.and_then(|name| outputs.iter().find(|o| o.name == name))
        .or_else(|| outputs.iter().find(|o| crate::overlays::is_output(vp, o)));
    target.is_none_or(|o| cfg.bar_on_output(&o.name))
}

/// A flush drawer's `(x, width)` that keeps it on its screen edge: past the
/// slide's end (spring overshoot) it stretches, rather than leave a gap.
/// At rest it reaches 2 px past the edge (`x = -2` on the left).
fn edge_attached(on_left: bool, x: f32, w: f32, screen_w: f32) -> (f32, f32) {
    if on_left {
        let edge = -2.0;
        if x > edge { (edge, w + x - edge) } else { (x, w) }
    } else {
        let edge = screen_w + 2.0;
        (x, w.max(edge - x))
    }
}

pub struct SessionView {
    shell: Entity<ShellState>,
    focus: FocusHandle,
    entries: Vec<SessionEntry>,
    selected: usize,
    pressed: Option<usize>,
    /// Hold-to-confirm in progress: entry and when it started.
    holding: Option<(usize, Instant)>,
    slide: SpringValue<f32>,
    radii: Vec<SpringValue<f32>>,
    clock: FrameClock,
    closing: bool,
    entered: bool,
    leave_task: Option<gpui::Task<()>>,
    /// Waits for the size of the output the menu opened on (issue #55: it
    /// waited for the first output's, so on any other display it never
    /// drew and held the keyboard and pointer).
    first: FirstFrame,
    /// The bar shows on the output the menu opened on (known from its
    /// first real frame): a top / bottom dock hangs from it there.
    bar_here: bool,
    output: Option<String>,
    /// Last press on the menu (focus changes right after it are its echo).
    last_press: Option<Instant>,
    deactivate_task: Option<gpui::Task<()>>,
    error: Option<String>,
}

impl SessionView {
    /// A menu on a surface opened for an output of logical size `expected`
    /// ([`crate::outputs::overlay_target`]).
    pub fn new(expected: (f32, f32), output: Option<String>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let shell = shell(cx);
        cx.observe(&shell, |this: &mut Self, shell, cx| {
            this.closing = !shell.read(cx).session_open;
            if !this.closing {
                this.entered = false;
                this.leave_task = None;
            }
            cx.notify();
        })
        .detach();
        cx.observe_window_activation(window, |this: &mut Self, window, cx| {
            if window.is_window_active() {
                this.deactivate_task = None;
                return;
            }
            this.deactivate_task = Some(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(DEACTIVATE_SETTLE).await;
                let _ = this.update_in(cx, |this, window, cx| {
                    if !window.is_window_active() && crate::dismiss::focus_loss_dismisses(this.last_press, Instant::now()) {
                        this.close(cx);
                    }
                });
            }));
        })
        .detach();
        let focus = cx.focus_handle();
        focus.focus(window, cx);
        first_frame::guard("session menu", window, cx, |this| this.first.drawn(), |this, cx| this.close(cx));
        let (entries, selected) = visible(&config(cx).read(cx).get().session_menu);
        Self {
            last_press: None,
            deactivate_task: None,
            shell,
            focus,
            radii: entries.iter().map(|_| SpringValue::new(RADIUS_REST_PX, RADIUS_SPRING)).collect(),
            entries,
            selected,
            pressed: None,
            holding: None,
            slide: SpringValue::new(0.0, OPEN_SPRING),
            clock: FrameClock::new(),
            closing: false,
            entered: false,
            leave_task: None,
            first: FirstFrame::new(expected),
            bar_here: true,
            output,
            error: None,
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.closing = true;
        self.holding = None;
        self.shell.update(cx, |s, cx| s.set_session(false, cx));
    }

    fn needs_hold(&self, i: usize, cx: &App) -> bool {
        config(cx).read(cx).get().session_menu.hold_to_confirm && self.entries.get(i).is_some_and(|e| e.kind.is_destructive())
    }

    /// Activates entry `i` (click, Enter, number key): runs it, or starts
    /// the hold when it needs confirming.
    fn activate(&mut self, i: usize, cx: &mut Context<Self>) {
        if i >= self.entries.len() {
            return;
        }
        self.selected = i;
        if self.needs_hold(i, cx) {
            if self.holding.is_none_or(|(h, _)| h != i) {
                self.holding = Some((i, Instant::now()));
            }
            cx.notify();
            return;
        }
        self.fire(i, cx);
    }

    fn fire(&mut self, i: usize, cx: &mut Context<Self>) {
        self.holding = None;
        let Some(entry) = self.entries.get(i).cloned() else { return };
        let cfg = config(cx).read(cx).snapshot();
        match run(&entry, &cfg, cx) {
            Ok(()) => self.close(cx),
            Err(e) => {
                self.error = Some(e);
                cx.notify();
            }
        }
    }

    fn release(&mut self, cx: &mut Context<Self>) {
        if self.holding.take().is_some() {
            cx.notify();
        }
    }

    /// Hold progress 0–1 of entry `i`.
    fn hold_progress(&self, i: usize, hold: Duration) -> f32 {
        match self.holding {
            Some((h, since)) if h == i => (since.elapsed().as_secs_f32() / hold.as_secs_f32().max(f32::EPSILON)).min(HOLD_DONE),
            _ => 0.0,
        }
    }
}

impl Render for SessionView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (sw, sh) = first_frame::viewport(window);
        match self.first.settle((sw, sh)) {
            Settle::Waiting if !self.closing => {
                window.request_animation_frame();
                return div().id("session-root").size_full().track_focus(&self.focus).into_any_element();
            }
            Settle::First { .. } => self.bar_here = bar_shows_here(self.output.as_deref(), (sw, sh), cx),
            _ => {}
        }
        let palette = mandara_core::frame_palette(window, cx);
        let f = fonts(cx);
        let cfg = config(cx).read(cx).snapshot();
        let menu = cfg.session_menu.clone();
        let hold = Duration::from_millis(menu.hold_ms.max(1));
        let n = self.entries.len();

        // Springs.
        self.slide.set_config(if self.closing { SpringConfig::critical(CLOSE_STIFFNESS) } else { OPEN_SPRING });
        self.slide.set_target(if self.closing { 0.0 } else { 1.0 });
        for (i, r) in self.radii.iter_mut().enumerate() {
            r.set_target(if self.pressed == Some(i) || self.holding.is_some_and(|(h, _)| h == i) {
                RADIUS_PRESSED_PX
            } else if self.selected == i {
                RADIUS_FOCUSED_PX
            } else {
                RADIUS_REST_PX
            });
        }
        let dt = self.clock.tick(Instant::now());
        let mut moving = self.slide.step(dt);
        for r in &mut self.radii {
            moving |= r.step(dt);
        }
        if let Some((i, _)) = self.holding {
            if self.hold_progress(i, hold) >= HOLD_DONE {
                self.fire(i, cx);
            } else {
                moving = true;
            }
        }
        if self.closing && self.slide.value() <= CLOSED_EPSILON {
            window.remove_window();
        } else if moving {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        let p = self.slide.value().max(0.0);
        let fill = frame_color(&palette).hsla();
        let ramp = |(from, span): (f32, f32)| ((p - from) / span).clamp(0.0, 1.0);

        // One button / tile.
        let grid = menu.style == SessionMenuStyle::Grid;
        let button = |i: usize, entry: &SessionEntry, this: &Self, cx: &mut Context<Self>| {
            let focused = this.selected == i;
            let destructive = entry.kind.is_destructive();
            let (bg, fg) = if focused {
                if destructive { (palette.urgent_coral, palette.urgent_coral.best_on(Color::hex("#ffffff"), Color::hex("#1a0d0b"))) } else { (palette.primary, palette.on_primary) }
            } else {
                (palette.surface_container_high, palette.on_surface)
            };
            let progress = this.hold_progress(i, hold);
            let side = if grid { TILE_PX } else { BTN_PX };
            let radius = this.radii.get(i).map(|r| r.value()).unwrap_or(RADIUS_REST_PX);
            div()
                .id(("session-entry", i))
                .relative()
                .size(px(side))
                .flex_none()
                .rounded(px(crate::style::radius(radius)))
                .overflow_hidden()
                .bg(bg.hsla())
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(TILE_LABEL_GAP_PX))
                .cursor_pointer()
                // Hold-to-confirm: a tide rising from the bottom.
                .when(progress > 0.0, |el| {
                    el.child(div().absolute().left_0().bottom_0().w_full().h(px(side * progress)).bg(fg.with_alpha(HOLD_FILL_ALPHA).hsla()))
                })
                .child(any_icon(&entry_icon(entry), if grid { TILE_ICON_PX } else { DOCK_ICON_PX }, fg))
                .when(grid, |el| {
                    el.child(div().px(px(BADGE_INSET_PX)).text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(fg.hsla()).truncate().max_w(px(side - 2.0 * BADGE_INSET_PX)).child(entry_label(entry)))
                })
                .when(menu.show_shortcuts && i < 9, |el| {
                    el.child(
                        div()
                            .absolute()
                            .top(px(BADGE_INSET_PX))
                            .right(px(BADGE_INSET_PX))
                            .size(px(BADGE_PX))
                            .rounded_full()
                            .bg(fg.with_alpha(0.14).hsla())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(text::LABEL))
                            .font_features(tabular_nums())
                            .text_color(fg.with_alpha(0.8).hsla())
                            .child((i + 1).to_string()),
                    )
                })
                .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                    if *hovered {
                        this.selected = i;
                        cx.notify();
                    } else if this.holding.is_some_and(|(h, _)| h == i) {
                        this.release(cx);
                    }
                }))
                .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    this.pressed = Some(i);
                    if this.needs_hold(i, cx) {
                        this.activate(i, cx);
                    }
                    cx.notify();
                }))
                .on_mouse_up(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                    this.pressed = None;
                    this.release(cx);
                }))
                .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.pressed = None;
                    if !this.needs_hold(i, cx) {
                        this.activate(i, cx);
                    }
                }))
        };
        let buttons: Vec<_> = self.entries.clone().iter().enumerate().map(|(i, e)| button(i, e, self, cx).into_any_element()).collect();
        let label = self.entries.get(self.selected).map(entry_label).unwrap_or_default();
        let hint = match (&self.error, self.holding) {
            (Some(e), _) => e.clone(),
            (None, Some(_)) => t("Keep holding to confirm").to_string(),
            (None, None) if self.needs_hold(self.selected, cx) => tf("{action} — press and hold", &[("action", &label)]),
            _ => label.clone(),
        };

        // The dock's place (the grid always opens in the middle).
        let dock_w = BTN_PX + DOCK_PAD_PX * 2.0;
        let dock_h = n as f32 * (BTN_PX + DOCK_GAP_PX) - DOCK_GAP_PX + DOCK_PAD_PX * 2.0;
        let place = dock_place(menu.position, cfg.sidebar_on_right(), Metrics::of(&cfg, crate::style::radius_scale()), self.bar_here, cfg.dock_bottom_clearance(), (sw, sh), (dock_w, dock_h));
        // Hanging from the bar, the scrim leaves the bar's band bright (it
        // still closes the menu when clicked).
        let keep_bright = match place {
            DockPlace::Hang(pl) if !grid => pl.keep_bright,
            _ => None,
        };
        let dim = div().absolute().left_0().right_0().bg(gpui::black().opacity(SCRIM_ALPHA * p.min(1.0)));
        let dim = match keep_bright {
            Some((by, bh)) if by <= 0.0 => dim.top(px(bh)).bottom_0(),
            Some((_, bh)) => dim.top_0().bottom(px(bh)),
            None => dim.top_0().bottom_0(),
        };

        let root = div()
            .id("session-root")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            // Capture phase sees every press, even over the occluding menu.
            // Outside presses land on the scrim below (the menu, its label
            // pill and flares occlude it; see `crate::dismiss`).
            .capture_any_mouse_down(cx.listener(|this, _: &gpui::MouseDownEvent, _, _| this.last_press = Some(Instant::now())))
            .font_family(f.ui.clone())
            .font_weight(crate::style::body_weight())
            .text_color(palette.on_surface.hsla())
            .on_key_down(cx.listener(move |this, e: &KeyDownEvent, _, cx| {
                let n = this.entries.len().max(1);
                let cols = if grid { grid_columns(&config(cx).read(cx).get().session_menu, n) } else { 1 };
                let key = e.keystroke.key.as_str();
                match key {
                    "escape" => this.close(cx),
                    "down" | "j" => this.selected = (this.selected + cols).min(n - 1),
                    "up" | "k" => this.selected = this.selected.saturating_sub(cols),
                    "right" | "l" | "tab" => this.selected = (this.selected + 1) % n,
                    "left" | "h" => this.selected = (this.selected + n - 1) % n,
                    "enter" | "space" if !e.is_held => this.activate(this.selected, cx),
                    digit if digit.len() == 1 && !e.is_held => {
                        if let Some(d) = digit.chars().next().and_then(|c| c.to_digit(10)).filter(|d| *d >= 1) {
                            this.activate(d as usize - 1, cx);
                        }
                    }
                    _ => {}
                }
                cx.notify();
            }))
            .on_key_up(cx.listener(|this, _: &KeyUpEvent, _, cx| this.release(cx)))
            .child(
                div()
                    .id("scrim")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .child(dim)
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| this.close(cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| this.close(cx))),
            );

        if grid {
            let cols = grid_columns(&menu, n);
            let rows = n.div_ceil(cols);
            let inner_w = cols as f32 * TILE_PX + (cols.saturating_sub(1)) as f32 * GRID_GAP_PX;
            let w = inner_w + GRID_PAD_PX * 2.0;
            let h = rows as f32 * TILE_PX + rows.saturating_sub(1) as f32 * GRID_GAP_PX + GRID_PAD_PX * 2.0 + LABEL_H_PX + GRID_TITLE_GAP_PX;
            let (x, y) = ((sw - w) / 2.0, (sh - h) / 2.0);
            let scale = 0.92 + 0.08 * p.min(1.0);
            return root
                .child(
                    div()
                        .id("session-grid")
                        .absolute()
                        .left(px(x + (1.0 - scale) * w / 2.0))
                        .top(px(y + (1.0 - scale) * h / 2.0))
                        .w(px(w * scale))
                        .h(px(h * scale))
                        .rounded(px(crate::style::radius(GRID_RADIUS_PX)))
                        .bg(fill)
                        .border_1()
                        .border_color(palette.specular_rim_light.with_alpha(0.12).hsla())
                        .opacity(p.min(1.0))
                        .occlude()
                        .p(px(GRID_PAD_PX * scale))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(GRID_TITLE_GAP_PX))
                        .child(
                            div()
                                .w(px(inner_w))
                                .flex()
                                .flex_wrap()
                                .gap(px(GRID_GAP_PX))
                                .children(buttons),
                        )
                        .child(div().h(px(LABEL_H_PX)).flex().items_center().font_weight(FontWeight::MEDIUM).text_color(if self.error.is_some() { palette.urgent_coral.hsla() } else { palette.on_surface.hsla() }).child(hint)),
                )
                .into_any_element();
        }

        // Dock: on a screen side (by default the one opposite the sidebar),
        // or hanging from the top / bottom centre.
        let scale = window.scale_factor();
        // (drawer, the rect occluding the scrim, the buttons' column
        // (clipped to the drawer while it pours out), label `(x, y)` and
        // whether it sits right of the dock, the area the pointer may roam)
        let (drawer, occluder, column, label_at, label_right, zone) = match place {
            DockPlace::Side { on_left } => {
                // The side-panel look (`[sidebar.look]`): flush with flares, or a
                // floating rounded slab `inset` clear of the edge.
                let look = &cfg.sidebar.look;
                let inset = look.inset_with(&cfg.bar);
                let travel = dock_w + inset;
                let x = if on_left { travel * p.min(SLIDE_OVERSHOOT) - dock_w - 2.0 } else { sw - travel * p.min(SLIDE_OVERSHOOT) };
                let floats = look.floats_with(&cfg.bar);
                let (radius, flare) = (crate::style::side_panel_radius(look), if floats { 0.0 } else { crate::style::flare_radius(&cfg.bar) });
                let fill = fill.opacity(mandara_config::surfaces::ranges::OPACITY.clamp(look.opacity, 1.0));
                let y = (sh - dock_h) / 2.0;
                // A flush drawer stretches through the overshoot instead of leaving the edge.
                let (drawer_x, drawer_w) = if floats { (x, dock_w + 2.0) } else { edge_attached(on_left, x, dock_w + 2.0, sw) };
                let drawer = if floats {
                    div().absolute().left(px(x)).top(px(y)).w(px(dock_w + 2.0)).h(px(dock_h)).rounded(px(radius)).bg(fill).into_any_element()
                } else {
                    edge_drawer_snapped(if on_left { Edge::Left } else { Edge::Right }, drawer_x, y, drawer_w, dock_h, radius, flare, fill, scale)
                };
                let column = div().absolute().left(px(x + DOCK_PAD_PX)).top(px(y + DOCK_PAD_PX)).flex().flex_col().gap(px(DOCK_GAP_PX)).opacity(ramp(BUTTONS_FADE)).children(buttons).into_any_element();
                let label_x = if on_left { x + dock_w + LABEL_GAP_PX } else { x - LABEL_W_PX - LABEL_GAP_PX };
                // From the screen side out to the reach (past the side too).
                let reach = dock_w + DOCK_REACH_PX;
                let zone = Rect::new(if on_left { -sw } else { sw - reach }, y - DOCK_REACH_Y_PX, sw + reach, dock_h + 2.0 * DOCK_REACH_Y_PX);
                (drawer, Rect::new(drawer_x, y - flare, drawer_w, dock_h + 2.0 * flare), column, (label_x, y), on_left, zone)
            }
            DockPlace::Hang(pl) => {
                let rect = pl.rect;
                let edge = pl.edge.unwrap_or(LauncherEdge::Top);
                let top = edge == LauncherEdge::Top;
                let joined = pl.keep_bright.is_some() && pl.join == Join::Flares;
                let (fill, seam) = crate::overlays::panel_fill(&cfg, &palette, joined);
                let radius = crate::style::panel_radius(&cfg.bar);
                let buttons_col = |x: f32, y: f32| div().absolute().left(px(x)).top(px(y)).flex().flex_col().gap(px(DOCK_GAP_PX)).opacity(ramp(BUTTONS_FADE)).children(buttons);
                let (drawer, occluder, column, content_y) = if pl.join == Join::Card {
                    // Under a floating bar: a card the panels' gap away
                    // that slides in and fades, like a drop-down panel.
                    let open = p.min(1.0);
                    let y = rect.y + (1.0 - open) * crate::bar::layout::PANEL_SLIDE_PX * if top { -1.0 } else { 1.0 };
                    let card = div()
                        .absolute()
                        .left(px(rect.x))
                        .top(px(y))
                        .w(px(rect.w))
                        .h(px(rect.h))
                        .rounded(px(radius))
                        .bg(fill)
                        .border_1()
                        .border_color(crate::plugins::panel_fx::surface_edge(&palette, cfg.theme.adjustments.rim_intensity).hsla())
                        .shadow(crate::plugins::panel_fx::elevation_scaled(&palette, open, cfg.panels.shadow_strength))
                        .opacity(open)
                        .into_any_element();
                    (card, Rect::new(rect.x, y, rect.w, rect.h), buttons_col(rect.x + DOCK_PAD_PX, y + DOCK_PAD_PX).into_any_element(), y)
                } else {
                    // Out of the bar (or the bare edge) with its flares:
                    // the drawer grows from the edge, the buttons slide out
                    // from behind it.
                    let (shown, content_y) = hang_frame(rect, edge, p);
                    let flare = pl.flare;
                    let drawer = if top {
                        edge_drawer_snapped(Edge::Top, shown.x, shown.y - seam, shown.w, shown.h + seam, radius, flare, fill, scale)
                    } else {
                        edge_drawer_snapped(Edge::Bottom, shown.x, shown.y, shown.w, shown.h + seam, radius, flare, fill, scale)
                    };
                    let column = div()
                        .absolute()
                        .left(px(shown.x))
                        .top(px(shown.y))
                        .w(px(shown.w))
                        .h(px(shown.h))
                        .overflow_hidden()
                        .child(buttons_col(DOCK_PAD_PX, content_y - shown.y + DOCK_PAD_PX))
                        .into_any_element();
                    (drawer, Rect::new(shown.x - flare, shown.y, shown.w + 2.0 * flare, shown.h), column, content_y)
                };
                // The label beside the column, on the reading side.
                let right = !crate::bidi::rtl();
                let label_x = if right { rect.right() + LABEL_GAP_PX } else { rect.x - LABEL_W_PX - LABEL_GAP_PX };
                // Roaming room: around the column and its label, and the
                // bar it hangs from.
                let (y0, y1) = if top { (0.0, rect.bottom() + DOCK_REACH_Y_PX) } else { (rect.y - DOCK_REACH_Y_PX, sh) };
                let zone = Rect::new(rect.x - DOCK_REACH_PX - LABEL_W_PX, y0, rect.w + 2.0 * (DOCK_REACH_PX + LABEL_W_PX), y1 - y0);
                (drawer, occluder, column, (label_x, content_y), right, zone)
            }
        };
        let (label_x, dock_y) = label_at;
        root.on_mouse_move(cx.listener(move |this, e: &gpui::MouseMoveEvent, _, cx| {
            let (mx, my) = (f32::from(e.position.x), f32::from(e.position.y));
            let inside = mx >= zone.x && mx <= zone.x + zone.w && my >= zone.y && my <= zone.y + zone.h;
            if inside {
                this.entered = true;
                this.leave_task = None;
            } else if this.entered && this.leave_task.is_none() && this.holding.is_none() {
                // Grace before closing (`panels.leave_delay_ms`).
                let delay = crate::state::config(cx).read(cx).snapshot().panels.leave_delay();
                this.leave_task = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(delay).await;
                    let _ = this.update(cx, |this, cx| this.close(cx));
                }));
            }
        }))
        .child(drawer)
        // Occludes the scrim over the drawer and its edge flares.
        .child(div().id("dock-body").absolute().left(px(occluder.x)).top(px(occluder.y)).w(px(occluder.w)).h(px(occluder.h)).occlude())
        .child(column)
        .child(
            div()
                .absolute()
                .top(px(dock_y + DOCK_PAD_PX + self.selected as f32 * (BTN_PX + DOCK_GAP_PX) + BTN_PX / 2.0 - LABEL_H_PX / 2.0))
                .left(px(label_x))
                .w(px(LABEL_W_PX))
                .h(px(LABEL_H_PX))
                .flex()
                .when(label_right, |el| el.justify_start())
                .when(!label_right, |el| el.justify_end())
                .opacity(ramp(LABEL_FADE))
                .child(
                    div()
                        .id("session-label")
                        .occlude()
                        .px(px(LABEL_PAD_X_PX))
                        .rounded_full()
                        .bg(fill)
                        .flex()
                        .items_center()
                        .font_weight(FontWeight::MEDIUM)
                        .when(self.error.is_some(), |el| el.text_color(palette.urgent_coral.hsla()))
                        .child(hint),
                ),
        )
        .into_any_element()
    }
}

/// Entry kinds `session run` accepts.
const RUN_CHOICES: &[&str] = &["lock", "logout", "suspend", "hibernate", "reboot", "shutdown", "soft-reboot", "firmware", "restart-shell"];

pub fn register(r: &mut PluginRegistry) {
    r.command(
        OWNER,
        CommandSpec::new("session entries", tk("The session menu's entries in order, with labels and icons (JSON)"), |_, cx| {
            let cfg = config(cx).read(cx).snapshot();
            let entries: Vec<_> = cfg
                .session_menu
                .entries
                .iter()
                .map(|e| json!({ "kind": e.kind.id(), "enabled": e.enabled, "label": entry_label(e), "icon": entry_icon(e), "destructive": e.kind.is_destructive(), "command": e.command, "boot_entry": e.boot_entry }))
                .collect();
            Ok(json!({ "style": cfg.session_menu.style, "position": cfg.session_menu.position, "hold_to_confirm": cfg.session_menu.hold_to_confirm, "hold_ms": cfg.session_menu.hold_ms, "entries": entries }))
        })
        .example("session entries"),
    );
    r.command(
        OWNER,
        CommandSpec::new_output("session boot-entries", tk("Restart targets: firmware setup support and boot-loader entries (JSON)"), |_, _| Ok(super::pending(|| Ok(json!(sleep::reboot_targets())))))
            .example("session boot-entries"),
    );
    r.command(
        OWNER,
        CommandSpec::new("session reboot-to", tk("Restart once into a boot-loader entry, or `firmware` for the firmware setup (needs confirm)"), |a, cx| {
            let target = a.str(0)?.to_string();
            if a.get(1) != Some("confirm") {
                return Err(tf("This restarts the computer; repeat as `session reboot-to {target} confirm`", &[("target", &target)]));
            }
            let entry = if target == "firmware" { SessionEntry::new(SessionEntryKind::Firmware, true) } else { SessionEntry { boot_entry: target.clone(), ..SessionEntry::new(SessionEntryKind::BootEntry, true) } };
            let cfg = config(cx).read(cx).snapshot();
            run(&entry, &cfg, cx)?;
            Ok(json!({ "restarting_into": target, "dry_run": sleep::dry_run() }))
        })
        .arg(ArgSpec::required("entry", ArgKind::String, tk("boot entry id (see `session boot-entries`) or firmware")))
        .arg(ArgSpec::optional("confirm", ArgKind::Enum, tk("required: this restarts the computer")).choices(&["confirm"]))
        .example("session reboot-to firmware confirm"),
    );
    r.command(
        OWNER,
        CommandSpec::new("session do", tk("Run a session-menu action the way the menu would, with its before-hooks (destructive ones need confirm)"), |a, cx| {
            let kind = SessionEntryKind::from_id(a.str(0)?).ok_or_else(|| t("Unknown session action").to_string())?;
            if kind.is_destructive() && a.get(1) != Some("confirm") {
                return Err(tf("`{action}` ends the session; repeat as `session do {action} confirm`", &[("action", &kind.id())]));
            }
            let cfg = config(cx).read(cx).snapshot();
            let entry = cfg.session_menu.entries.iter().find(|e| e.kind == kind).cloned().unwrap_or_else(|| SessionEntry::new(kind, true));
            run(&entry, &cfg, cx)?;
            Ok(json!({ "action": kind.id(), "dry_run": sleep::dry_run() }))
        })
        .arg(ArgSpec::required("action", ArgKind::Enum, tk("session action")).choices(RUN_CHOICES))
        .arg(ArgSpec::optional("confirm", ArgKind::Enum, tk("required for actions that end the session")).choices(&["confirm"]))
        .example("session do lock")
        .example("session do soft-reboot confirm"),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_size_outputs_keep_the_selected_connectors_bar_assignment() {
        use mandara_core::OutputGeometry;
        let outputs: Vec<_> = ["DP-1", "DP-2"].into_iter().map(|name| OutputGeometry { name: name.into(), x: 0, y: 0, width: 1920, height: 1080 }).collect();
        let mut cfg = mandara_config::ShellConfig::default();
        cfg.bar.monitors = "DP-2".into();
        assert!(bar_on_target(&cfg, &outputs, Some("DP-2"), (1920.0, 1080.0)));
        assert!(!bar_on_target(&cfg, &outputs, Some("DP-1"), (1920.0, 1080.0)));
        assert!(!bar_on_target(&cfg, &outputs, None, (1920.0, 1080.0)));
    }

    #[test]
    fn the_menu_on_a_second_scaled_display_draws_at_that_display_size() {
        use crate::first_frame::FALLBACK_FRAMES;
        use crate::outputs::{logical_size, pick_output};
        use mandara_config::outputs::{DisplaysConfig, SurfaceKind};
        use mandara_core::OutputGeometry;
        // Issue #55: the main display on the left, a smaller one on the
        // right (1920x1080 at 1.25: 1536x864 logical, offset by the first).
        let main = OutputGeometry { name: "DP-1".into(), x: 0, y: 0, width: 2560, height: 1440 };
        let side = OutputGeometry { name: "HDMI-A-1".into(), x: 2560, y: 0, width: 1536, height: 864 };
        let displays = DisplaysConfig::default();
        // Either listing order: the menu waits for the display it opens on.
        for outputs in [[main.clone(), side.clone()], [side.clone(), main.clone()]] {
            for (focused, viewport) in [("HDMI-A-1", (1536.0, 864.0)), ("DP-1", (2560.0, 1440.0))] {
                let target = pick_output(&outputs, Some(focused), &displays, SurfaceKind::Overlays).expect("an output");
                assert_eq!(target.name, focused);
                assert_eq!(logical_size(target), viewport, "logical, not the layout offset or physical pixels");
                let mut first = FirstFrame::new(logical_size(target));
                assert!(first.ready(viewport), "{focused}: draws on its first frame");
            }
        }
        // The old expectation, the first-listed output's size, never matched
        // on the other display: an invisible menu holding the keyboard (and
        // on Hyprland the pointer) for good. Now it draws after a few frames.
        let mut stale = FirstFrame::new(logical_size(&main));
        for _ in 0..FALLBACK_FRAMES {
            assert!(!stale.ready((1536.0, 864.0)));
        }
        assert!(stale.ready((1536.0, 864.0)));
        // The dock sits on that display's own right edge (surface-local
        // coordinates), however far right the display is in the layout.
        let dock_w = BTN_PX + DOCK_PAD_PX * 2.0;
        let (x, w) = edge_attached(false, 1536.0 - dock_w, dock_w, 1536.0);
        assert!(x >= 0.0 && x + w <= 1536.0 + 2.0 + f32::EPSILON, "{x} + {w} stays on the 1536 px display");
    }

    /// Issue #57: the dock at the top centre, under the bar and joined
    /// with it; elsewhere it falls back to what that edge offers.
    #[test]
    fn the_dock_hangs_from_the_bar_at_the_top_centre() {
        use mandara_config::{BarLayout, BarPosition, ShellConfig};
        let (screen, size) = ((1536.0, 864.0), (BTN_PX + DOCK_PAD_PX * 2.0, 5.0 * (BTN_PX + DOCK_GAP_PX) - DOCK_GAP_PX + DOCK_PAD_PX * 2.0));
        let bar = |layout: BarLayout, position: BarPosition| {
            let mut cfg = ShellConfig::default();
            cfg.bar.layout = layout;
            cfg.bar.position = position;
            Metrics::of(&cfg, 1.0)
        };
        let hang = |position, m: Metrics, bar_here| match dock_place(position, false, m, bar_here, None, screen, size) {
            DockPlace::Hang(pl) => pl,
            side => panic!("{side:?}"),
        };
        // Edge frame at the top: one piece with the bar, centred, sized to
        // this display, the bar's band left bright.
        let m = bar(BarLayout::MinimalEdge, BarPosition::Top);
        let pl = hang(SessionMenuPosition::Top, m, true);
        assert_eq!((pl.join, pl.edge), (Join::Flares, Some(LauncherEdge::Top)));
        assert_eq!(pl.rect.y, m.bar_far_edge(), "it starts where the bar ends");
        assert_eq!(pl.flare, m.flare);
        assert!((pl.rect.x + pl.rect.w / 2.0 - screen.0 / 2.0).abs() < 0.5, "centred: {:?}", pl.rect);
        assert_eq!((pl.rect.w, pl.rect.h), size);
        assert!(pl.rect.bottom() <= screen.1, "on the display it opened on");
        assert_eq!(pl.keep_bright, Some((0.0, crate::bar::layout::exclusive_zone(&m))));
        // Floating glass pills: a card a gap under the bar.
        let m = bar(BarLayout::FloatingPills, BarPosition::Top);
        let pl = hang(SessionMenuPosition::Top, m, true);
        assert_eq!(pl.join, Join::Card);
        assert_eq!(pl.rect.y, m.bar_far_edge() + m.panel_gap);
        // The bar elsewhere (or not on this display): from the bare top edge.
        for (m, here) in [(bar(BarLayout::MinimalEdge, BarPosition::Bottom), true), (bar(BarLayout::MinimalEdge, BarPosition::Top), false)] {
            let pl = hang(SessionMenuPosition::Top, m, here);
            assert_eq!((pl.join, pl.rect.y, pl.keep_bright), (Join::Flares, 0.0, None));
        }
        // Bottom centre rises from a bottom bar.
        let m = bar(BarLayout::MinimalEdge, BarPosition::Bottom);
        let pl = hang(SessionMenuPosition::Bottom, m, true);
        assert_eq!(pl.edge, Some(LauncherEdge::Bottom));
        assert!((pl.rect.bottom() - (screen.1 - m.bar_far_edge())).abs() < 0.5, "{:?}", pl.rect);
        // The sides, as before: by default the one opposite the sidebar.
        let m = bar(BarLayout::MinimalEdge, BarPosition::Top);
        assert_eq!(dock_place(SessionMenuPosition::Side, false, m, true, None, screen, size), DockPlace::Side { on_left: false });
        assert_eq!(dock_place(SessionMenuPosition::Side, true, m, true, None, screen, size), DockPlace::Side { on_left: true });
        assert_eq!(dock_place(SessionMenuPosition::Left, false, m, true, None, screen, size), DockPlace::Side { on_left: true });
        assert_eq!(dock_place(SessionMenuPosition::Right, true, m, true, None, screen, size), DockPlace::Side { on_left: false });
    }

    #[test]
    fn a_hanging_dock_pours_out_of_its_edge() {
        let rest = Rect::new(700.0, 30.0, 100.0, 400.0);
        // Closed: nothing shows, the buttons wait behind the bar.
        let (shown, content) = hang_frame(rest, LauncherEdge::Top, 0.0);
        assert_eq!((shown.y, shown.h, content), (30.0, 0.0, 30.0 - 400.0));
        // Open: the whole column at rest.
        assert_eq!(hang_frame(rest, LauncherEdge::Top, 1.0), (rest, 30.0));
        // Overshooting, the drawer stretches and stays on the bar.
        let (shown, content) = hang_frame(rest, LauncherEdge::Top, 1.1);
        assert!((shown.y - 30.0).abs() < 1e-3 && shown.h > 400.0 && content > 30.0);
        // Mirrored at the bottom.
        let low = Rect::new(700.0, 400.0, 100.0, 400.0);
        let (shown, content) = hang_frame(low, LauncherEdge::Bottom, 0.5);
        assert_eq!((shown.bottom(), shown.h, content), (800.0, 200.0, 600.0));
    }

    #[test]
    fn visible_entries_and_default_selection() {
        let mut menu = SessionMenuConfig { default_action: "reboot".into(), ..Default::default() };
        let (entries, selected) = visible(&menu);
        assert_eq!(entries[selected].kind, SessionEntryKind::Reboot);
        // Nothing enabled still leaves Lock.
        for e in &mut menu.entries {
            e.enabled = false;
        }
        assert_eq!(visible(&menu).0.len(), 1);
        assert_eq!(grid_columns(&SessionMenuConfig { grid_columns: 9, ..Default::default() }, 4), 4);
    }

    #[test]
    fn a_flush_drawer_stays_on_its_edge_through_the_overshoot() {
        let (w, sw) = (82.0, 1920.0);
        // At rest and while sliding in, the drawer keeps its own width.
        assert_eq!(edge_attached(true, -2.0, w, sw), (-2.0, w));
        assert_eq!(edge_attached(true, -40.0, w, sw), (-40.0, w));
        assert_eq!(edge_attached(false, sw - 80.0, w, sw), (sw - 80.0, w));
        // Overshooting, it stretches back to the edge instead of leaving a gap.
        assert_eq!(edge_attached(true, 14.0, w, sw), (-2.0, w + 16.0));
        let (x, width) = edge_attached(false, sw - 96.0, w, sw);
        assert_eq!((x, x + width), (sw - 96.0, sw + 2.0));
    }

    #[test]
    fn labels_icons_and_hooks() {
        let custom = SessionEntry { kind: SessionEntryKind::Custom, command: "systemctl --user restart pipewire".into(), ..Default::default() };
        assert_eq!(entry_label(&custom), "systemctl --user restart pipewire");
        assert_eq!(entry_icon(&custom), "terminal");
        let named = SessionEntry { label: "Windows".into(), icon: "monitor".into(), boot_entry: "windows.conf".into(), ..SessionEntry::new(SessionEntryKind::BootEntry, true) };
        assert_eq!((entry_label(&named).as_str(), entry_icon(&named).as_str()), ("Windows", "monitor"));
        assert_eq!(before_hook(SessionEntryKind::Firmware), Some(HookEvent::BeforeReboot));
        assert_eq!(before_hook(SessionEntryKind::Lock), None);
        for kind in SessionEntryKind::ALL {
            assert!(crate::icons::ICON_NAMES.contains(&kind_icon(kind)), "{kind:?} icon");
        }
    }
}
