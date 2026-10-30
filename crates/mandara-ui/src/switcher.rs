//! The Alt-Tab window switcher: a keyboard-driven overlay of open windows
//! in most-recently-used order, with live previews copied from each window
//! (`ext-image-copy-capture`) and app icons as a fallback.
//!
//! Driving it: the compositor binds Alt+Tab / Alt+Shift+Tab to
//! `mandara msg switcher next|prev` (Mandara's generated drop-in, see
//! [`bind_alt_tab`]). The first call opens the overlay with exclusive
//! keyboard focus, so it hears the modifier being released through
//! `wl_keyboard.modifiers` and switches then. Robust against the race
//! where Alt is released before the overlay maps: once the surface has the
//! keyboard and no modifier is held, it commits on its own. Tab / arrows /
//! Enter / Escape / clicks work inside the overlay as well, and
//! `switcher commit|cancel` finish it from scripts.
//!
//! Clicks follow [`crate::dismiss`]: a press anywhere on the panel hands the
//! gesture to the pointer (releasing Alt no longer switches, the click
//! decides), presses outside the panel cancel, and keyboard state that
//! arrives with MangoWM's leave/enter pair on a click is not a release.
//! Opened from inside the shell (launcher, hot corner), `next|prev` stay
//! open like `open`: no modifier is held there.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::layer_shell::{Anchor, KeyboardInteractivity, Layer};
use gpui::{
    AnyWindowHandle, App, AppContext as _, ClickEvent, Context, FocusHandle, FontWeight, Global, ImageSource, InteractiveElement, IntoElement, KeyDownEvent, ModifiersChangedEvent, MouseButton, ObjectFit, ParentElement,
    Render, RenderImage, SharedString, StatefulInteractiveElement, Styled, StyledImage as _, Window, WindowHandle, div, img, prelude::FluentBuilder as _, px,
};
use mandara_config::workflow::{SwitcherConfig, SwitcherStyle};
use mandara_core::{FrameClock, LayerSurfaceSpec, SpringConfig, SpringValue, SurfaceRole, ToGpui};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginRegistry};
use mandara_services::toplevels::Toplevel;
use serde_json::json;

use crate::fonts::{fonts, tabular_nums};
use crate::kit::text;
use crate::dismiss::{FocusSettle, Origin};
use crate::plugins::panel_fx::{elevation, readable, rim, wash, WASH_SOFT};
use crate::state::config;

/// Command owner (shared with the window commands).
pub const OWNER: &str = crate::windows::OWNER;
/// Preview aspect ratio (height / width).
const THUMB_ASPECT: f32 = 10.0 / 16.0;
/// Card padding, gap between cards and panel padding, logical px.
const CARD_PAD: f32 = 10.0;
const CARD_GAP: f32 = 12.0;
const PANEL_PAD: f32 = 18.0;
/// Header (count + focused title) height.
const HEADER_H: f32 = 30.0;
/// Caption row under a preview.
const CAPTION_H: f32 = 30.0;
const CAPTION_ICON_PX: f32 = 18.0;
/// App icon drawn when there's no preview.
const FALLBACK_ICON_PX: f32 = 64.0;
/// Compact style: tile and icon sizes.
const COMPACT_TILE: f32 = 76.0;
const COMPACT_ICON: f32 = 48.0;
/// Radii of the panel, cards and previews.
const PANEL_RADIUS: f32 = 28.0;
const CARD_RADIUS: f32 = 20.0;
const THUMB_RADIUS: f32 = 12.0;
/// Width of the selection ring.
const RING_PX: f32 = 2.5;
/// Selection: glow alpha and blur, fill alpha.
const RING_GLOW_ALPHA: f32 = 0.35;
const RING_GLOW_BLUR: f32 = 18.0;
const RING_FILL_ALPHA: f32 = 0.12;
/// Minimized windows are drawn faded.
const MINIMIZED_ALPHA: f32 = 0.7;
/// Bounds of the preview width setting, logical px.
const THUMB_W_RANGE: (f32, f32) = (120.0, 480.0);
/// Selection highlight spring (snappy, slightly under-damped).
const HIGHLIGHT_SPRING: SpringConfig = SpringConfig::new(520.0, 34.0, 1.0);
/// Panel scale-in spring.
const OPEN_SPRING: SpringConfig = SpringConfig::new(420.0, 30.0, 1.0);
/// Panel starts at this scale and grows to 1.
const OPEN_SCALE_FROM: f32 = 0.94;
/// After the overlay has the keyboard this long without ever seeing a
/// modifier held, the release happened before it mapped: commit. The held
/// state can arrive well after focus: through an input method (fcitx5) it
/// was measured at ~225 ms, so a shorter grace switched windows on a held
/// Alt+Tab without showing the switcher. A quick tap shows it this long.
const RELEASE_GRACE: Duration = Duration::from_millis(500);
/// Demo mode closes by itself after this long (never commits).
const DEMO_TIMEOUT: Duration = Duration::from_millis(2600);
/// Demo mode steps the selection this often.
const DEMO_STEP: Duration = Duration::from_millis(700);
/// Scrim behind the panel.
const SCRIM_ALPHA: f32 = 0.28;
/// Screen side margin, logical px.
const SCREEN_MARGIN: f32 = 48.0;
/// Previews are captured at most this many physical pixels wide.
const THUMB_MAX_PHYSICAL_W: f32 = 640.0;

thread_local! {
    /// Output scale last seen by the overlay (previews are captured at it).
    static LAST_SCALE: std::cell::Cell<f32> = const { std::cell::Cell::new(DEFAULT_SCALE) };
}
/// Scale assumed before the overlay first rendered.
const DEFAULT_SCALE: f32 = 2.0;

/// Previews by window id, kept between openings (refreshed on each).
#[derive(Default)]
struct Thumbs(HashMap<u64, Arc<RenderImage>>);
impl Global for Thumbs {}

#[derive(Default)]
struct Switcher {
    handle: Option<WindowHandle<SwitcherView>>,
}
impl Global for Switcher {}

/// How the overlay was opened.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Mode {
    /// From a held-modifier bind: releasing it switches.
    Hold,
    /// Stays open until Enter, a click or `switcher commit`.
    Sticky,
    /// Screenshot / test mode: no keyboard grab, never switches, closes itself.
    Demo,
}

/// Windows the switcher lists, in order, under `cfg`.
pub fn candidates(list: &[Toplevel], cfg: &SwitcherConfig, hub: &crate::hub::Hub) -> Vec<Toplevel> {
    let focused_output = list.iter().find(|t| t.activated).and_then(|t| t.outputs.first().cloned());
    let mut seen_apps: Vec<String> = Vec::new();
    list.iter()
        .filter(|t| !crate::windows::is_shell(&t.app_id))
        .filter(|t| cfg.include_minimized || !t.minimized)
        .filter(|t| cfg.all_outputs || focused_output.as_ref().is_none_or(|o| t.outputs.is_empty() || t.outputs.contains(o)))
        .filter(|t| !cfg.current_workspace || on_current_workspace(t, hub))
        .filter(|t| {
            if !cfg.group_by_app {
                return true;
            }
            let fresh = !seen_apps.contains(&t.app_id);
            seen_apps.push(t.app_id.clone());
            fresh
        })
        .cloned()
        .collect()
}

/// Whether a window is on its monitor's active workspace / tags, by the
/// compositor's own client list (unknown windows count as visible).
fn on_current_workspace(t: &Toplevel, hub: &crate::hub::Hub) -> bool {
    hub.clients.iter().find(|c| c.app_id == t.app_id && c.title == t.title).is_none_or(|c| client_visible(c, hub))
}

/// Whether a compositor client is on an active workspace / tag of its
/// monitor (clients without tags, or on unknown monitors, count as visible).
pub fn client_visible(client: &mandara_compositor::ClientInfo, hub: &crate::hub::Hub) -> bool {
    let Some(model) = hub.navigation.get(&client.monitor) else { return true };
    let mask = model.active_mask();
    client.tags.is_empty() || client.tags.iter().any(|tag| mask & 1u32.checked_shl(tag.saturating_sub(1)).unwrap_or(0) != 0)
}

/// Initial selection: the previous window going forward, the oldest going back.
pub fn initial_selection(count: usize, forward: bool) -> usize {
    match (count, forward) {
        (0, _) => 0,
        (1, _) => 0,
        (_, true) => 1,
        (n, false) => n - 1,
    }
}

/// The mode `next|prev` open in: a held-modifier gesture only when the
/// command came from outside the shell (a compositor keybind or a script).
pub fn mode_for(origin: Origin) -> Mode {
    match origin {
        Origin::External => Mode::Hold,
        Origin::InProcess => Mode::Sticky,
    }
}

/// What the hold gesture wants after an event.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum HoldVerdict {
    /// Nothing to do yet.
    Wait,
    /// Look again after this long (a release inside a focus-change window).
    Recheck(Duration),
    /// The modifier was let go: switch.
    Commit,
}

/// Hold-mode release detection, free of GPUI so it can be tested.
///
/// A release commits, except: keyboard state arriving within
/// [`crate::dismiss::DEACTIVATE_SETTLE`] of a focus change is re-checked
/// once that settles (MangoWM's leave/enter on a click carries it), and once
/// the pointer has pressed on the panel the click decides instead.
#[derive(Copy, Clone, Debug, Default)]
pub struct HoldGesture {
    /// A switching modifier was seen held since opening.
    saw_held: bool,
    /// When the surface first got the keyboard.
    focused_at: Option<Instant>,
    focus: FocusSettle,
    /// The pointer pressed on the panel: releases no longer switch.
    pointer_owned: bool,
}

impl HoldGesture {
    /// The surface's keyboard focus changed.
    pub fn focus_changed(&mut self, active: bool, now: Instant) {
        self.focus.changed(now);
        if active && self.focused_at.is_none() {
            self.focused_at = Some(now);
        }
    }

    /// A button went down on the panel.
    pub fn pointer_pressed(&mut self) {
        self.pointer_owned = true;
    }

    /// Current modifier state (from an event or a frame); `held` is whether
    /// a switching modifier is down.
    pub fn update(&mut self, held: bool, now: Instant) -> HoldVerdict {
        if held {
            self.saw_held = true;
            return HoldVerdict::Wait;
        }
        if self.pointer_owned {
            return HoldVerdict::Wait;
        }
        if let Some(left) = self.focus.remaining(now) {
            return HoldVerdict::Recheck(left);
        }
        if self.saw_held {
            return HoldVerdict::Commit;
        }
        // Released before the overlay had the keyboard: commit once it has
        // had it a moment with nothing held.
        match self.focused_at.map(|t| now.saturating_duration_since(t)) {
            Some(had) if had >= RELEASE_GRACE => HoldVerdict::Commit,
            Some(had) => HoldVerdict::Recheck(RELEASE_GRACE - had),
            None => HoldVerdict::Wait,
        }
    }
}

pub struct SwitcherView {
    focus: FocusHandle,
    mode: Mode,
    list: Vec<Toplevel>,
    selected: usize,
    /// Hold-mode release detection.
    hold: HoldGesture,
    /// A pending re-check of the hold gesture.
    recheck: Option<gpui::Task<()>>,
    /// Pointer position when it first reported, and whether it has moved
    /// since: a cursor resting where a card maps must not pick that card.
    pointer_origin: Option<gpui::Point<gpui::Pixels>>,
    pointer_moved: bool,
    closing: bool,
    committed: bool,
    open: SpringValue<f32>,
    hx: SpringValue<f32>,
    hy: SpringValue<f32>,
    clock: FrameClock,
}

impl SwitcherView {
    fn new(mode: Mode, list: Vec<Toplevel>, selected: usize, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        if mode != Mode::Demo {
            focus.focus(window, cx);
        }
        cx.observe_window_activation(window, |this: &mut Self, window, cx| {
            this.hold.focus_changed(window.is_window_active(), Instant::now());
            let held = holding(&window.modifiers());
            this.judge(held, cx);
        })
        .detach();
        if mode == Mode::Demo {
            cx.spawn(async move |this, cx| {
                let started = Instant::now();
                while started.elapsed() < DEMO_TIMEOUT {
                    cx.background_executor().timer(DEMO_STEP).await;
                    if this.update(cx, |this, cx| this.step(1, cx)).is_err() {
                        return;
                    }
                }
                let _ = this.update(cx, |this, cx| this.cancel(cx));
            })
            .detach();
        }
        Self {
            focus,
            mode,
            list,
            selected,
            hold: HoldGesture::default(),
            recheck: None,
            pointer_origin: None,
            pointer_moved: false,
            closing: false,
            committed: false,
            open: SpringValue::new(0.0, OPEN_SPRING),
            hx: SpringValue::new(0.0, HIGHLIGHT_SPRING),
            hy: SpringValue::new(0.0, HIGHLIGHT_SPRING),
            clock: FrameClock::new(),
        }
    }

    /// Feeds the modifier state to the hold gesture and acts on its verdict.
    fn judge(&mut self, held: bool, cx: &mut Context<Self>) {
        if self.mode != Mode::Hold || self.closing {
            return;
        }
        let verdict = self.hold.update(held, Instant::now());
        tracing::trace!(target: "mandara::switcher", held, ?verdict, hold = ?self.hold, "hold check");
        match verdict {
            HoldVerdict::Wait => {}
            HoldVerdict::Commit => {
                if release_commits(cx) {
                    self.commit(cx);
                }
            }
            HoldVerdict::Recheck(after) => {
                self.recheck = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(after).await;
                    let _ = this.update(cx, |this, cx| {
                        this.recheck = None;
                        // Re-render: `render` reads the live modifiers.
                        cx.notify();
                    });
                }));
            }
        }
    }

    /// The pointer moved to `at`; hover only selects once it really moved.
    fn pointer_at(&mut self, at: gpui::Point<gpui::Pixels>) {
        match self.pointer_origin {
            None => self.pointer_origin = Some(at),
            Some(origin) if origin != at => self.pointer_moved = true,
            Some(_) => {}
        }
    }

    fn step(&mut self, delta: isize, cx: &mut Context<Self>) {
        let n = self.list.len();
        if n > 0 && !self.closing {
            self.selected = (self.selected as isize + delta).rem_euclid(n as isize) as usize;
            cx.notify();
        }
    }

    /// Focuses the selected window and closes.
    fn commit(&mut self, cx: &mut Context<Self>) {
        if self.closing {
            return;
        }
        if self.mode != Mode::Demo
            && let Some(t) = self.list.get(self.selected)
        {
            crate::windows::activate(t.id, cx);
            self.committed = true;
        }
        self.closing = true;
        cx.notify();
    }

    fn cancel(&mut self, cx: &mut Context<Self>) {
        self.closing = true;
        cx.notify();
    }

    fn on_key(&mut self, e: &KeyDownEvent, cx: &mut Context<Self>) {
        let k = &e.keystroke;
        match k.key.as_str() {
            "tab" if k.modifiers.shift => self.step(-1, cx),
            "tab" | "right" | "down" | "l" | "j" => self.step(1, cx),
            "left" | "up" | "h" | "k" => self.step(-1, cx),
            "home" => {
                self.selected = 0;
                cx.notify();
            }
            "end" => {
                self.selected = self.list.len().saturating_sub(1);
                cx.notify();
            }
            "enter" | "space" => self.commit(cx),
            "escape" => self.cancel(cx),
            // Close the selected window without leaving the switcher.
            "q" | "w" => {
                if let Some(t) = self.list.get(self.selected) {
                    crate::windows::close(t.id, cx);
                    self.list.remove(self.selected);
                    self.selected = self.selected.min(self.list.len().saturating_sub(1));
                    if self.list.is_empty() {
                        self.cancel(cx);
                    }
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    fn on_modifiers(&mut self, e: &ModifiersChangedEvent, cx: &mut Context<Self>) {
        self.judge(holding(&e.modifiers), cx);
    }
}

/// A switching modifier (not Shift, which only reverses) is down.
fn holding(m: &gpui::Modifiers) -> bool {
    m.alt || m.control || m.platform
}

fn release_commits(cx: &App) -> bool {
    config(cx).read(cx).get().switcher.release_commits
}

/// Decodes RGBA into a GPUI image (BGRA).
fn render_image(mut rgba: image::RgbaImage) -> Arc<RenderImage> {
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(rgba)]))
}

/// Copies fresh previews of `list` in the background; each lands in
/// [`Thumbs`] and re-renders the overlay.
fn capture_previews(list: &[Toplevel], scale: f32, cursor: bool, cx: &mut App) {
    let targets: Vec<mandara_services::window_capture::Target> = list.iter().map(|t| mandara_services::window_capture::Target { app_id: t.app_id.clone(), title: t.title.clone() }).collect();
    let ids: Vec<u64> = list.iter().map(|t| t.id).collect();
    let w = (config(cx).read(cx).get().switcher.thumbnail_width * scale).min(THUMB_MAX_PHYSICAL_W).round() as u32;
    let max = (w, (w as f32 * THUMB_ASPECT * 2.0).round() as u32);
    let (tx, rx) = async_channel::unbounded::<(u64, Arc<RenderImage>)>();
    std::thread::Builder::new()
        .name("mandara-switcher-previews".into())
        .spawn(move || {
            let result = mandara_services::window_capture::capture_windows(&targets, max, cursor, |i, img| {
                let _ = tx.send_blocking((ids[i], render_image(img.clone())));
            });
            if let Err(err) = result {
                tracing::debug!("switcher previews: {err:#}");
            }
        })
        .ok();
    cx.spawn(async move |cx| {
        while let Ok((id, image)) = rx.recv().await {
            cx.update(|cx| {
                cx.default_global::<Thumbs>().0.insert(id, image);
                if let Some(h) = cx.try_global::<Switcher>().and_then(|s| s.handle) {
                    let _ = h.update(cx, |_, _, cx| cx.notify());
                }
            });
        }
    })
    .detach();
}

fn is_open(cx: &App) -> bool {
    cx.try_global::<Switcher>().and_then(|s| s.handle).is_some_and(|h| cx.windows().iter().any(|w: &AnyWindowHandle| w.window_id() == h.window_id()))
}

fn surface(mode: Mode, cx: &App) -> gpui::WindowOptions {
    let (display, (w, h)) = crate::outputs::overlay_target(cx);
    let keyboard = if mode == Mode::Demo { KeyboardInteractivity::None } else { KeyboardInteractivity::Exclusive };
    let mut spec = LayerSurfaceSpec::overlay(SurfaceRole::Switcher, gpui::size(px(w), px(h)), display).with_keyboard(keyboard);
    spec.layer = Layer::Overlay;
    spec.anchor = Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT;
    spec.window_options()
}

/// Opens the switcher (or moves its selection when open).
pub fn show(mode: Mode, forward: bool, cx: &mut App) -> Result<serde_json::Value, String> {
    let cfg = config(cx).read(cx).get().switcher.clone();
    if !cfg.enabled {
        return Err(t("The window switcher is turned off (Studio → Window switcher)").to_string());
    }
    if is_open(cx) {
        let h = cx.global::<Switcher>().handle.ok_or("switcher")?;
        h.update(cx, |v, _, cx| v.step(if forward { 1 } else { -1 }, cx)).map_err(|e| e.to_string())?;
        return status(cx);
    }
    let windows = crate::windows::init(cx);
    let list = candidates(windows.read(cx).list(), &cfg, crate::hub::hub(cx).read(cx));
    if list.is_empty() {
        return Ok(json!({ "open": false, "windows": 0 }));
    }
    let selected = initial_selection(list.len(), forward);
    if cfg.previews && cfg.style == SwitcherStyle::Carousel {
        // Drop previews of windows that are gone.
        let live: Vec<u64> = windows.read(cx).list().iter().map(|t| t.id).collect();
        cx.default_global::<Thumbs>().0.retain(|id, _| live.contains(id));
        capture_previews(&list, LAST_SCALE.with(|s| s.get()), false, cx);
    }
    let n = list.len();
    let handle = cx.open_window(surface(mode, cx), move |window, cx| cx.new(|cx| SwitcherView::new(mode, list, selected, window, cx))).map_err(|e| format!("{e:#}"))?;
    cx.set_global(Switcher { handle: Some(handle) });
    Ok(json!({ "open": true, "windows": n, "selected": selected, "mode": format!("{mode:?}").to_lowercase() }))
}

fn with_view(cx: &mut App, f: impl FnOnce(&mut SwitcherView, &mut Context<SwitcherView>)) -> Result<(), String> {
    if !is_open(cx) {
        return Err(t("The window switcher is not open").to_string());
    }
    let h = cx.global::<Switcher>().handle.ok_or("switcher")?;
    h.update(cx, |v, _, cx| f(v, cx)).map_err(|e| e.to_string())
}

fn status(cx: &App) -> Result<serde_json::Value, String> {
    let open = is_open(cx);
    let (selected, list) = match cx.try_global::<Switcher>().and_then(|s| s.handle).filter(|_| open) {
        Some(h) => h.read(cx).map(|v| (v.selected, v.list.clone())).unwrap_or_default(),
        None => (0, Vec::new()),
    };
    Ok(json!({ "open": open, "selected": selected, "windows": list.iter().map(|t| json!({ "id": t.id, "app_id": t.app_id, "title": t.title })).collect::<Vec<_>>() }))
}

/// Alt+Tab → `switcher next`, Alt+Shift+Tab → `switcher prev`, added to
/// Mandara's compositor binds unless the chord is taken (in your config or
/// by another Mandara bind). Returns the chords added.
pub fn bind_alt_tab(cx: &mut App) -> Result<Vec<String>, String> {
    use mandara_config::compositor::{BindAction, Keybind, existing_chords};
    let target = crate::hub::hub(cx).read(cx).compositor.as_ref().map(|b| b.target()).ok_or_else(|| t("No supported compositor detected").to_string())?;
    let taken = existing_chords(target);
    let wanted = [Keybind::new(&["alt"], "Tab", BindAction::Mandara("switcher next".into())), Keybind::new(&["alt", "shift"], "Tab", BindAction::Mandara("switcher prev".into()))];
    let mut added = Vec::new();
    let mut skipped = Vec::new();
    crate::state::update_config(cx, |c| {
        for bind in wanted {
            let chord = bind.chord();
            let ours = c.compositor.keybinds.iter().any(|b| b.chord() == chord);
            if ours {
                continue;
            }
            if taken.contains(&chord) {
                skipped.push(bind.display());
            } else {
                added.push(bind.display());
                c.compositor.keybinds.push(bind);
            }
        }
    });
    if !skipped.is_empty() && added.is_empty() {
        return Err(tf("Already bound in your compositor config: {chords}", &[("chords", &skipped.join(", "))]));
    }
    Ok(added)
}

/// `switcher next|prev|open|commit|cancel|demo|status|bind`.
pub fn register(r: &mut PluginRegistry) {
    r.command(
        OWNER,
        CommandSpec::new("switcher", tk("Alt-Tab window switcher: next / prev open it or move the selection (releasing Alt switches); commit, cancel; open stays until Enter; demo previews without grabbing the keyboard"), |a, cx| {
            match a.str(0)? {
                "next" => show(mode_for(crate::dismiss::origin(cx)), true, cx),
                "prev" => show(mode_for(crate::dismiss::origin(cx)), false, cx),
                "open" => show(Mode::Sticky, true, cx),
                "demo" => show(Mode::Demo, true, cx),
                "commit" => with_view(cx, |v, cx| v.commit(cx)).map(|_| json!({ "committed": true })),
                "cancel" => with_view(cx, |v, cx| v.cancel(cx)).map(|_| json!({ "open": false })),
                "status" => status(cx),
                "bind" => bind_alt_tab(cx).map(|added| json!({ "added": added })),
                other => Err(format!("unknown action {other:?}")),
            }
        })
        .arg(ArgSpec::required("action", ArgKind::Enum, tk("what to do")).choices(&["next", "prev", "open", "commit", "cancel", "demo", "status", "bind"]))
        .example("switcher next")
        .example("switcher demo"),
    );
}

impl Render for SwitcherView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = mandara_core::frame_palette(window, cx);
        let f = fonts(cx);
        let cfg = config(cx).read(cx).snapshot();
        let sc = &cfg.switcher;
        let vp = window.viewport_size();
        let (sw, sh) = (f32::from(vp.width), f32::from(vp.height));

        // Late release: the overlay got the keyboard after Alt was let go.
        if self.recheck.is_none() {
            self.judge(holding(&window.modifiers()), cx);
        }
        let now = Instant::now();

        self.open.set_target(if self.closing { 0.0 } else { 1.0 });
        let dt = self.clock.tick(now);
        let mut moving = self.open.step(dt);

        let n = self.list.len().max(1);
        let compact = sc.style == SwitcherStyle::Compact;
        let max_cols = (sc.max_columns as usize).max(1);
        let (card_w, card_h) = if compact {
            (COMPACT_TILE, COMPACT_TILE)
        } else {
            let w = sc.thumbnail_width.clamp(THUMB_W_RANGE.0, THUMB_W_RANGE.1);
            (w + CARD_PAD * 2.0, w * THUMB_ASPECT + CARD_PAD * 2.0 + if sc.captions { CAPTION_H } else { 0.0 })
        };
        // Columns that fit the screen.
        let fit = (((sw - SCREEN_MARGIN * 2.0 - PANEL_PAD * 2.0 + CARD_GAP) / (card_w + CARD_GAP)).floor() as usize).max(1);
        let cols = n.min(max_cols).min(fit);
        let rows = n.div_ceil(cols);
        let header_h = if sc.show_count || compact { HEADER_H } else { 0.0 };
        let panel_w = cols as f32 * card_w + (cols - 1) as f32 * CARD_GAP + PANEL_PAD * 2.0;
        let panel_h = rows as f32 * card_h + (rows - 1) as f32 * CARD_GAP + PANEL_PAD * 2.0 + header_h;
        let (px0, py0) = ((sw - panel_w) / 2.0, (sh - panel_h) / 2.0);
        let sel = self.selected.min(n - 1);
        self.hx.set_target((sel % cols) as f32 * (card_w + CARD_GAP));
        self.hy.set_target((sel / cols) as f32 * (card_h + CARD_GAP));
        moving |= self.hx.step(dt);
        moving |= self.hy.step(dt);
        if self.closing && self.open.value() <= 0.02 {
            window.remove_window();
        } else if moving {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        let p = self.open.value().clamp(0.0, 1.0);
        let scale = OPEN_SCALE_FROM + (1.0 - OPEN_SCALE_FROM) * p;
        let accent = readable(palette.primary, &palette);
        let thumbs = cx.try_global::<Thumbs>().map(|t| t.0.clone()).unwrap_or_default();
        // Full-colour app icons read best at switcher sizes.
        let monochrome = false;
        LAST_SCALE.with(|s| s.set(window.scale_factor()));

        let mut cards: Vec<gpui::AnyElement> = Vec::new();
        for (i, t) in self.list.iter().enumerate() {
            let (cx0, cy0) = ((i % cols) as f32 * (card_w + CARD_GAP), (i / cols) as f32 * (card_h + CARD_GAP));
            let selected = i == sel;
            let body: gpui::AnyElement = if compact {
                div().size_full().flex().items_center().justify_center().child(crate::plugins::workspaces::app_icon(&t.app_id, COMPACT_ICON, palette.on_surface, monochrome, &palette)).into_any_element()
            } else {
                let tw = card_w - CARD_PAD * 2.0;
                let th = tw * THUMB_ASPECT;
                let preview = match thumbs.get(&t.id).filter(|_| sc.previews) {
                    Some(image) => img(ImageSource::Render(image.clone())).w(px(tw)).h(px(th)).rounded(px(crate::style::radius(THUMB_RADIUS))).object_fit(ObjectFit::Contain).into_any_element(),
                    None => div()
                        .w(px(tw))
                        .h(px(th))
                        .rounded(px(crate::style::radius(THUMB_RADIUS)))
                        .bg(wash(WASH_SOFT, &palette).hsla())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(crate::plugins::workspaces::app_icon(&t.app_id, FALLBACK_ICON_PX, palette.on_surface_variant, monochrome, &palette))
                        .into_any_element(),
                };
                let app = crate::plugins::window::app_name(&t.app_id);
                let caption: SharedString = if t.title.is_empty() { app.into() } else { t.title.clone().into() };
                div()
                    .size_full()
                    .p(px(CARD_PAD))
                    .flex()
                    .flex_col()
                    .gap(px(CARD_PAD / 2.0))
                    .child(preview)
                    .when(sc.captions, |el| {
                        el.child(
                            div()
                                .h(px(CAPTION_H - CARD_PAD / 2.0))
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .child(crate::plugins::workspaces::app_icon(&t.app_id, CAPTION_ICON_PX, palette.on_surface, monochrome, &palette))
                                .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).when(selected, |el| el.font_weight(FontWeight::MEDIUM)).child(caption))
                                .when(t.minimized, |el| el.child(div().flex_none().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(mandara_i18n::t("minimized")))),
                        )
                    })
                    .into_any_element()
            };
            cards.push(
                div()
                    .id(("switcher-card", i))
                    .absolute()
                    .left(px(cx0))
                    .top(px(cy0))
                    .w(px(card_w))
                    .h(px(card_h))
                    .rounded(px(crate::style::radius(CARD_RADIUS)))
                    .cursor_pointer()
                    .opacity(if selected || !t.minimized { 1.0 } else { MINIMIZED_ALPHA })
                    .child(body)
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered && this.mode != Mode::Demo && this.pointer_moved {
                            this.selected = i;
                            cx.notify();
                        }
                    }))
                    .on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                        this.selected = i;
                        this.commit(cx);
                    }))
                    .into_any_element(),
            );
        }
        let ring = div()
            .absolute()
            .left(px(self.hx.value()))
            .top(px(self.hy.value()))
            .w(px(card_w))
            .h(px(card_h))
            .rounded(px(crate::style::radius(CARD_RADIUS)))
            .bg(accent.with_alpha(RING_FILL_ALPHA).hsla())
            .border(px(RING_PX))
            .border_color(accent.hsla())
            .shadow(vec![crate::plugins::panel_fx::glow(accent.with_alpha(RING_GLOW_ALPHA), RING_GLOW_BLUR, 0.0, 0.0)]);
        let current = self.list.get(sel);
        let header = div()
            .h(px(header_h))
            .flex()
            .items_center()
            .gap(px(10.0))
            .when(sc.show_count, |el| {
                el.child(
                    div()
                        .flex_none()
                        .px(px(10.0))
                        .h(px(22.0))
                        .rounded_full()
                        .flex()
                        .items_center()
                        .bg(accent.with_alpha(0.16).hsla())
                        .text_color(accent.hsla())
                        .text_size(px(text::LABEL))
                        .font_weight(FontWeight::SEMIBOLD)
                        .font_features(tabular_nums())
                        .child(tf("{n} windows", &[("n", &self.list.len())])),
                )
            })
            .child(
                div()
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_size(px(text::BODY))
                    .font_weight(FontWeight::MEDIUM)
                    .child(current.map(|t| if compact && !t.title.is_empty() { t.title.clone() } else { crate::plugins::window::app_name(&t.app_id) }).unwrap_or_default()),
            );
        let mut shadow = elevation(&palette, p);
        shadow.extend(rim(&palette));
        let panel = div()
            .absolute()
            .left(px(px0 + panel_w * (1.0 - scale) / 2.0))
            .top(px(py0 + panel_h * (1.0 - scale) / 2.0))
            .w(px(panel_w * scale))
            .h(px(panel_h * scale))
            .rounded(px(crate::style::radius(PANEL_RADIUS)))
            .bg(crate::shell::frame_color(&palette).hsla())
            .shadow(shadow)
            .opacity(p)
            .overflow_hidden()
            // Presses on the panel never reach the outside catcher behind
            // it, and hand the gesture to the pointer.
            .id("switcher-panel")
            .occlude()
            .on_any_mouse_down(cx.listener(|this, _: &gpui::MouseDownEvent, _, _| this.hold.pointer_pressed()))
            .on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, _, _| this.pointer_at(e.position)))
            .child(
                div()
                    .p(px(PANEL_PAD))
                    .flex()
                    .flex_col()
                    .child(header)
                    .child(div().relative().w(px(panel_w - PANEL_PAD * 2.0)).h(px(panel_h - PANEL_PAD * 2.0 - header_h)).child(ring).children(cards)),
            );

        div()
            .id("switcher-root")
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .font_family(f.ui.clone())
            .font_weight(crate::style::body_weight())
            .text_color(palette.on_surface.hsla())
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| this.on_key(e, cx)))
            .on_modifiers_changed(cx.listener(|this, e: &ModifiersChangedEvent, _, cx| this.on_modifiers(e, cx)))
            .on_mouse_move(cx.listener(|this, e: &gpui::MouseMoveEvent, _, _| this.pointer_at(e.position)))
            // The scrim is the outside catcher: any press on it cancels.
            .child(
                div()
                    .id("switcher-outside")
                    .absolute()
                    .inset_0()
                    .bg(gpui::black().opacity(SCRIM_ALPHA * p))
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| this.cancel(cx)))
                    .on_mouse_down(MouseButton::Right, cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| this.cancel(cx)))
                    .on_mouse_down(MouseButton::Middle, cx.listener(|this, _: &gpui::MouseDownEvent, _, cx| this.cancel(cx))),
            )
            .child(panel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn win(id: u64, app: &str, out: &str) -> Toplevel {
        Toplevel { id, app_id: app.into(), title: format!("{app} {id}"), outputs: vec![out.into()], activated: id == 1, ..Default::default() }
    }

    #[test]
    fn candidates_follow_the_options() {
        let hub = crate::hub::Hub::default();
        let mut list = vec![win(1, "zed", "DP-1"), win(2, "firefox", "DP-1"), win(3, "zed", "HDMI-A-1"), win(4, mandara_core::SHELL_APP_ID, "DP-1")];
        list[1].minimized = true;
        let mut cfg = SwitcherConfig::default();
        let ids = |c: &SwitcherConfig| candidates(&list, c, &hub).iter().map(|t| t.id).collect::<Vec<_>>();
        assert_eq!(ids(&cfg), [1, 2, 3], "MRU order, shell surfaces excluded");
        cfg.include_minimized = false;
        assert_eq!(ids(&cfg), [1, 3]);
        cfg.all_outputs = false;
        assert_eq!(ids(&cfg), [1]);
        cfg = SwitcherConfig { group_by_app: true, ..SwitcherConfig::default() };
        assert_eq!(ids(&cfg), [1, 2]);
    }

    #[test]
    fn only_keybinds_and_scripts_hold() {
        assert_eq!(mode_for(Origin::External), Mode::Hold);
        assert_eq!(mode_for(Origin::InProcess), Mode::Sticky, "launcher / hot corner: no modifier is held");
    }

    #[test]
    fn hold_release_commits() {
        let t0 = Instant::now();
        let mut g = HoldGesture::default();
        g.focus_changed(true, t0);
        let settled = t0 + crate::dismiss::DEACTIVATE_SETTLE;
        assert_eq!(g.update(true, settled), HoldVerdict::Wait);
        assert_eq!(g.update(false, settled), HoldVerdict::Commit, "letting go of Alt switches");
    }

    #[test]
    fn late_release_commits_after_grace() {
        let t0 = Instant::now();
        let mut g = HoldGesture::default();
        assert_eq!(g.update(false, t0), HoldVerdict::Wait, "no keyboard yet: nothing known");
        g.focus_changed(true, t0);
        assert!(matches!(g.update(false, t0), HoldVerdict::Recheck(_)));
        assert_eq!(g.update(false, t0 + RELEASE_GRACE.max(crate::dismiss::DEACTIVATE_SETTLE)), HoldVerdict::Commit);
    }

    #[test]
    fn click_leave_enter_is_not_a_release() {
        // Alt held, then a click: MangoWM sends leave+enter before the
        // press, and the re-entry may carry "nothing held".
        let t0 = Instant::now();
        let mut g = HoldGesture::default();
        g.focus_changed(true, t0);
        let later = t0 + Duration::from_secs(1);
        assert_eq!(g.update(true, later), HoldVerdict::Wait);
        g.focus_changed(false, later);
        g.focus_changed(true, later);
        assert!(matches!(g.update(false, later), HoldVerdict::Recheck(_)), "deferred, not committed");
        g.pointer_pressed();
        let settled = later + crate::dismiss::DEACTIVATE_SETTLE;
        assert_eq!(g.update(false, settled), HoldVerdict::Wait, "the click decides");
    }

    #[test]
    fn release_after_focus_settles_still_commits() {
        let t0 = Instant::now();
        let mut g = HoldGesture::default();
        g.focus_changed(true, t0);
        assert_eq!(g.update(true, t0), HoldVerdict::Wait);
        // The focus blip was an artifact; Alt really is up afterwards.
        g.focus_changed(true, t0 + Duration::from_millis(500));
        let settled = t0 + Duration::from_millis(500) + crate::dismiss::DEACTIVATE_SETTLE;
        assert_eq!(g.update(false, settled), HoldVerdict::Commit);
    }

    #[test]
    fn selection_starts_on_the_previous_window() {
        assert_eq!(initial_selection(0, true), 0);
        assert_eq!(initial_selection(1, true), 0);
        assert_eq!(initial_selection(5, true), 1);
        assert_eq!(initial_selection(5, false), 4);
    }
}
