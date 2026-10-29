//! The power / session menu (`[session_menu]`): your entries in your order
//! (lock, log out, sleep, hibernate, restart, shut down, soft restart,
//! firmware setup, a boot-loader entry, restart Mandara, or any command),
//! shown as the edge dock or a centered grid, with optional hold-to-confirm
//! for everything that ends the session and number-key shortcuts.
//!
//! Choosing an entry runs its before-hooks (`before-logout`,
//! `before-reboot`, `before-shutdown`) to completion first.

use std::time::{Duration, Instant};

use gpui::{
    App, ClickEvent, Context, Entity, FocusHandle, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent, MouseButton, ParentElement, Render,
    StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px,
};
use mandara_config::ShellConfig;
use mandara_config::power::{HookEvent, SessionEntry, SessionEntryKind, SessionMenuConfig, SessionMenuStyle, SuspendMode};
use mandara_core::{Edge, FrameClock, SpringConfig, SpringValue, ToGpui, edge_drawer_snapped};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginRegistry};
use mandara_services::{SessionAction, hooks, sleep};
use mandara_theme::Color;
use serde_json::json;

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
/// Viewport sizes within this many px count as settled.
const SETTLE_TOLERANCE_PX: f32 = 2.0;
/// Opacity ramps of the buttons and the label during the slide.
const BUTTONS_FADE: (f32, f32) = (0.2, 0.6);
const LABEL_FADE: (f32, f32) = (0.5, 0.5);
/// Closed below this slide progress.
const CLOSED_EPSILON: f32 = 0.01;
/// Slide overshoot cap (springy entrance without flying off).
const SLIDE_OVERSHOOT: f32 = 1.2;
/// Output size assumed when nothing reports one.
const FALLBACK_OUTPUT: (f32, f32) = (1920.0, 1080.0);
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
                SessionEntryKind::Lock => SessionAction::Lock.run(),
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
            }
        })
        .map(|_| ())
        .map_err(|e| e.to_string())
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
    expected: (f32, f32),
    /// Last press on the menu (focus changes right after it are its echo).
    last_press: Option<Instant>,
    deactivate_task: Option<gpui::Task<()>>,
    error: Option<String>,
}

fn output_size(cx: &App) -> (f32, f32) {
    cx.try_global::<crate::overlays::Outputs>().and_then(|o| o.0.first().map(|g| (g.width as f32, g.height as f32))).unwrap_or(FALLBACK_OUTPUT)
}

impl SessionView {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
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
            expected: output_size(cx),
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
        let vp = window.viewport_size();
        let (sw, sh) = (f32::from(vp.width), f32::from(vp.height));
        let settled = (sw - self.expected.0).abs() < SETTLE_TOLERANCE_PX && (sh - self.expected.1).abs() < SETTLE_TOLERANCE_PX;
        if !settled && !self.closing {
            window.request_animation_frame();
            return div().id("session-root").size_full().track_focus(&self.focus).into_any_element();
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
                    .bg(gpui::black().opacity(SCRIM_ALPHA * p.min(1.0)))
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

        // Dock: the edge opposite the sidebar (left in RTL).
        let dock_w = BTN_PX + DOCK_PAD_PX * 2.0;
        let dock_h = n as f32 * (BTN_PX + DOCK_GAP_PX) - DOCK_GAP_PX + DOCK_PAD_PX * 2.0;
        let on_left = cfg.sidebar_on_right();
        // The side-panel look (`[sidebar.look]`): flush with flares, or a
        // floating rounded slab `inset` clear of the edge.
        let look = &cfg.sidebar.look;
        let inset = look.inset_with(&cfg.bar);
        let travel = dock_w + inset;
        let x = if on_left { travel * p.min(SLIDE_OVERSHOOT) - dock_w - 2.0 } else { sw - travel * p.min(SLIDE_OVERSHOOT) };
        let (radius, flare) = (crate::style::side_panel_radius(look), if look.floats_with(&cfg.bar) { 0.0 } else { crate::style::flare_radius(&cfg.bar) });
        let fill = fill.opacity(mandara_config::surfaces::ranges::OPACITY.clamp(look.opacity, 1.0));
        let y = (sh - dock_h) / 2.0;
        root.on_mouse_move(cx.listener(move |this, e: &gpui::MouseMoveEvent, window, cx| {
            let vp = window.viewport_size();
            let y0 = (f32::from(vp.height) - dock_h) / 2.0 - DOCK_REACH_Y_PX;
            let px_x = f32::from(e.position.x);
            let near = if on_left { px_x <= dock_w + DOCK_REACH_PX } else { px_x >= f32::from(vp.width) - dock_w - DOCK_REACH_PX };
            let inside = near && f32::from(e.position.y) >= y0 && f32::from(e.position.y) <= y0 + dock_h + 2.0 * DOCK_REACH_Y_PX;
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
        .child(if look.floats_with(&cfg.bar) {
            div().absolute().left(px(x)).top(px(y)).w(px(dock_w + 2.0)).h(px(dock_h)).rounded(px(radius)).bg(fill).into_any_element()
        } else {
            edge_drawer_snapped(if on_left { Edge::Left } else { Edge::Right }, x, y, dock_w + 2.0, dock_h, radius, flare, fill, window.scale_factor())
        })
        // Occludes the scrim over the drawer and its edge flares.
        .child(div().id("dock-body").absolute().left(px(x)).top(px(y - flare)).w(px(dock_w + 2.0)).h(px(dock_h + 2.0 * flare)).occlude())
        .child(
            div()
                .absolute()
                .left(px(x + DOCK_PAD_PX))
                .top(px(y + DOCK_PAD_PX))
                .flex()
                .flex_col()
                .gap(px(DOCK_GAP_PX))
                .opacity(ramp(BUTTONS_FADE))
                .children(buttons),
        )
        .child(
            div()
                .absolute()
                .top(px(y + DOCK_PAD_PX + self.selected as f32 * (BTN_PX + DOCK_GAP_PX) + BTN_PX / 2.0 - LABEL_H_PX / 2.0))
                .left(px(if on_left { x + dock_w + LABEL_GAP_PX } else { x - LABEL_W_PX - LABEL_GAP_PX }))
                .w(px(LABEL_W_PX))
                .h(px(LABEL_H_PX))
                .flex()
                .when(on_left, |el| el.justify_start())
                .when(!on_left, |el| el.justify_end())
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
            Ok(json!({ "style": cfg.session_menu.style, "hold_to_confirm": cfg.session_menu.hold_to_confirm, "hold_ms": cfg.session_menu.hold_ms, "entries": entries }))
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
    fn visible_entries_and_default_selection() {
        let mut menu = SessionMenuConfig::default();
        menu.default_action = "reboot".into();
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
