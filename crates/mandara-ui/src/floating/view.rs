//! One output's `mandara-floating` surface: the floating cards on it.
//!
//! A card is the plugin's panel on an opaque rounded surface (the docked
//! panels' fill, hairline edge and elevation) with a slim grab pill on a
//! strip along its top; hovering the card reveals its dock and close
//! buttons at the strip's end. Pressing anywhere on the card's chrome (the
//! strip or the edge around the plugin's content, never the content
//! itself) and moving a few pixels drags the card inside the surface,
//! clamped clear of the bar's band and the output's edges. The drag
//! listens window-wide, so the card follows the pointer wherever it goes;
//! pointer moves are applied once per frame and the place is saved once,
//! on release.
//! Its rect is one spring: it morphs out of the docked panel or chip,
//! follows the plugin's size, and morphs back into the bar when docked.
//!
//! Every frame the cards' rects (or, while dragging or typing, the whole
//! surface) are pushed to the compositor as the input region. Nothing runs
//! while idle: springs step only while they move, and a settled card's
//! content is a cached view that redraws only when its plugin notifies.

use std::time::Instant;

use crate::bidi::Bidi as _;

use gpui::{
    AnyElement, App, Bounds, Context, CursorStyle, DispatchPhase, FocusHandle, InteractiveElement, IntoElement, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
    Render, SharedString, Size, StatefulInteractiveElement, StyleRefinement, Styled, Window, canvas, div, point, prelude::FluentBuilder as _, px,
};
use mandara_config::{BarPosition, ShellConfig};
use mandara_core::{FrameClock, InputMaskRegistry, SpringConfig, SpringValue, ToGpui};
use mandara_i18n::tf;
use mandara_plugins::plugin::BuildFn;
use mandara_plugins::{PanelCx, PanelSpec};
use serde_json::{Value, json};

use super::geom::{self, Anchor, Chrome};
use super::{bounds_of, rect_of};
use crate::fonts::fonts;
use crate::plugins::panel_fx::{elevation_scaled, surface_edge};
use crate::state::config;
use mandara_i18n::t;

/// Height of the grab strip along a card's top.
const STRIP_PX: f32 = 28.0;
/// The grab pill in it.
const GRAB_W_PX: f32 = 40.0;
const GRAB_H_PX: f32 = 4.0;
/// Grab pill ink: resting, and while hovered or dragged.
const GRAB_ALPHA: (f32, f32) = (0.2, 0.45);
/// Dock / close buttons at the strip's end, their gap and end inset.
const CONTROL_PX: f32 = 20.0;
const CONTROL_GAP_PX: f32 = 2.0;
const CONTROL_INSET_PX: f32 = 6.0;
/// How far the pointer moves (logical px) before a press on the chrome
/// becomes a drag: a plain click never nudges the card.
const DRAG_SLOP_PX: f32 = 3.0;
/// Space kept between a card and the output's edges / the bar's band.
const EDGE_MARGIN_PX: f32 = 8.0;
/// How far a card lifts off the docked panel it pops out of.
const LIFT_PX: f32 = 24.0;
/// A card appearing in place (or closing) grows from (shrinks to) this
/// share smaller.
const APPEAR_SHRINK: f32 = 0.06;
/// The little pop of a card brought to the front.
const RAISE_SHRINK: f32 = 0.03;
/// Base UI text size before the typography scale, as on the shell surface.
const BASE_TEXT_PX: f32 = 12.5;
/// Below this a fading card counts as gone.
const HIDDEN_EPSILON: f32 = 0.002;
/// Frames a docked card stays drawn after the bar took over (the shell
/// surface needs one to draw the panel underneath).
const HANDOFF_FRAMES: u8 = 2;
/// Card fade and controls reveal.
const APPEAR_SPRING: SpringConfig = SpringConfig::new(320.0, 30.0, 1.0);
const HOVER_SPRING: SpringConfig = SpringConfig::SNAPPY;

/// What a card floats out of, and where it goes.
#[derive(Copy, Clone, Debug, Default)]
pub struct Placement {
    /// The docked panel or chip it lifts off (for its first place).
    pub anchor: Option<Anchor>,
    /// The rect its spring starts from, and whether content shows there
    /// already (an open docked panel) or fades in (a chip).
    pub from: Option<(Bounds<Pixels>, bool)>,
    /// Its place: asked for, or remembered for this output.
    pub at: Option<(f32, f32)>,
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum Leaving {
    /// Fading out where it is.
    Close,
    /// Springing into the bar; `snap`: onto the docked panel's rect.
    Dock { pinned: bool, snap: bool },
    /// The bar's panel took over; drawn a few more frames.
    Handed(u8),
}

struct Card {
    id: String,
    capture: Option<crate::panel_capture::Owner>,
    /// The plugin factory generation it was built from (a hot reload or
    /// switching the plugin off closes it).
    generation: u64,
    spec: PanelSpec,
    panel_cx: PanelCx,
    /// Focused by presses on the card itself (not on a text field), so the
    /// keyboard bridge knows nothing wants keys.
    focus: FocusHandle,
    /// The card's rect, animated.
    bounds: SpringValue<Bounds<Pixels>>,
    /// Its resting top-left.
    origin: (f32, f32),
    /// Its content size (the plugin's, fitted).
    content: (f32, f32),
    appear: SpringValue<f32>,
    hover: SpringValue<f32>,
    hovered: bool,
    leaving: Option<Leaving>,
}

struct Drag {
    id: String,
    /// Pointer minus the card's origin at the press.
    grab: (f32, f32),
    /// Where the press was.
    press: (f32, f32),
    /// The pointer's latest position, applied at the next frame.
    pointer: Option<(f32, f32)>,
    /// Past [`DRAG_SLOP_PX`]: the card follows, and its place is saved on
    /// release.
    moved: bool,
}

pub struct FloatingView {
    output: String,
    surface: (f32, f32),
    mask: InputMaskRegistry,
    clock: FrameClock,
    root_focus: FocusHandle,
    /// Bottom to top.
    cards: Vec<Card>,
    drag: Option<Drag>,
    /// The keyboard bridge is typing into this surface.
    keyboard: bool,
}

fn chrome(cfg: &ShellConfig) -> Chrome {
    Chrome { pad: cfg.panels.padding_for(false), strip: STRIP_PX, margin: EDGE_MARGIN_PX }
}

/// `b` shrunk by `k` (a share of its size) about its centre.
fn shrink(b: Bounds<Pixels>, k: f32) -> Bounds<Pixels> {
    let (w, h) = (f32::from(b.size.width), f32::from(b.size.height));
    let (dx, dy) = (w * k / 2.0, h * k / 2.0);
    Bounds { origin: point(b.origin.x + px(dx), b.origin.y + px(dy)), size: gpui::size(px(w - 2.0 * dx), px(h - 2.0 * dy)) }
}

fn bounds_at((x, y): (f32, f32), (w, h): (f32, f32)) -> Bounds<Pixels> {
    bounds_of(crate::dismiss::Rect::new(x, y, w, h))
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

impl FloatingView {
    pub fn new(output: String, surface: (f32, f32), cx: &mut Context<Self>) -> Self {
        cx.observe(&config(cx), |_, _, cx| cx.notify()).detach();
        cx.observe(&mandara_core::theme_entity(cx), |_, _, cx| cx.notify()).detach();
        Self {
            output,
            surface,
            mask: InputMaskRegistry::new(),
            clock: FrameClock::new(),
            root_focus: cx.focus_handle(),
            cards: Vec::new(),
            drag: None,
            keyboard: false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.cards.is_empty()
    }

    pub(crate) fn capture_panel(&mut self, id: &str, cx: &mut Context<Self>) -> Result<async_channel::Receiver<Result<String, String>>, String> {
        let card = self.cards.iter().find(|card| card.id == id && card.leaving.is_none()).ok_or_else(|| t("The panel is not visible").to_string())?;
        let state = card.capture.as_ref().ok_or_else(|| t("The panel is not visible").to_string())?.state.clone();
        crate::panel_capture::start(&state, card.bounds.value(), self.surface, cx)
    }

    /// Where cards may go: the output less the bar's band.
    fn area(&self, cfg: &ShellConfig) -> crate::dismiss::Rect {
        let zone = crate::shell::ExclusionView::zone_for(cfg.bar.position, cfg, &self.output);
        geom::usable_area(self.surface, cfg.bar.position, zone)
    }

    fn card(&self, id: &str) -> Option<&Card> {
        self.cards.iter().find(|c| c.id == id && !matches!(c.leaving, Some(Leaving::Handed(_))))
    }

    fn card_mut(&mut self, id: &str) -> Option<&mut Card> {
        self.cards.iter_mut().find(|c| c.id == id && c.leaving.is_none())
    }

    /// Builds `id`'s instance on this surface and floats its panel.
    /// Returns its resting top-left.
    pub fn add_card(&mut self, id: &str, generation: u64, build: &BuildFn, place: Placement, window: &mut Window, cx: &mut Context<Self>) -> Result<(f32, f32), String> {
        // A card still leaving under this id is replaced at once.
        self.cards.retain(|c| c.id != id);
        let panel_cx = PanelCx::for_monitor(&self.output);
        let pcx = panel_cx.clone();
        let instance = mandara_plugins::guard(&format!("plugin `{id}` floating panel"), || build(window, cx, pcx)).ok_or_else(|| tf("{plugin} failed to start", &[("plugin", &id)]))?;
        let spec = instance.panel.ok_or_else(|| tf("{plugin} has no panel", &[("plugin", &id)]))?;
        let cfg = config(cx).read(cx).snapshot();
        let (area, chrome) = (self.area(&cfg), chrome(&cfg));
        let want = spec.size.get();
        let content = geom::content_size((f32::from(want.width), f32::from(want.height)), cfg.panels.width_scale(), area, chrome);
        let size = geom::card_size(content, chrome);
        let origin = match place.at {
            Some(at) => geom::clamp_origin(at, size, area, EDGE_MARGIN_PX),
            None => geom::float_origin(place.anchor, size, cfg.bar.position, area, EDGE_MARGIN_PX, LIFT_PX),
        };
        let target = bounds_at(origin, size);
        let (start, shown) = place.from.unwrap_or((shrink(target, APPEAR_SHRINK), false));
        let (spring, _) = crate::shell::panel_spring(&cfg);
        let mut bounds = SpringValue::new(start, spring);
        bounds.set_target(target);
        let mut appear = SpringValue::new(if shown { 1.0 } else { 0.0 }, APPEAR_SPRING);
        appear.set_target(1.0);
        panel_cx.set_progress(appear.value());
        let capture = crate::panel_capture::Owner::new(self.output.clone(), cx);
        self.cards.push(Card {
            id: id.to_string(),
            capture: Some(capture),
            generation,
            spec,
            panel_cx,
            focus: cx.focus_handle(),
            bounds,
            origin,
            content,
            appear,
            hover: SpringValue::new(0.0, HOVER_SPRING),
            hovered: false,
            leaving: None,
        });
        cx.notify();
        Ok(origin)
    }

    /// Moves `id`'s card to `at` (clamped); returns where it went.
    pub fn move_card(&mut self, id: &str, at: (f32, f32), cx: &mut Context<Self>) -> Option<(f32, f32)> {
        let cfg = config(cx).read(cx).snapshot();
        let area = self.area(&cfg);
        let card = self.card_mut(id)?;
        let size = (f32::from(card.bounds.target().size.width), f32::from(card.bounds.target().size.height));
        card.origin = geom::clamp_origin(at, size, area, EDGE_MARGIN_PX);
        card.bounds.set_target(bounds_at(card.origin, size));
        let origin = card.origin;
        cx.notify();
        Some(origin)
    }

    /// Brings `id`'s card to the front with a little pop.
    pub fn raise(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(i) = self.cards.iter().position(|c| c.id == id && c.leaving.is_none()) else { return };
        let mut card = self.cards.remove(i);
        let target = card.bounds.target();
        card.bounds.snap_to(shrink(target, RAISE_SHRINK));
        card.bounds.set_target(target);
        self.cards.push(card);
        cx.notify();
    }

    /// The size `id`'s plugin asks for (for its place in the bar).
    pub fn wanted_size(&self, id: &str) -> Option<Size<Pixels>> {
        self.card(id).map(|c| c.spec.size.get())
    }

    /// `panel floating`'s entry for `id`.
    pub fn card_json(&self, id: &str) -> Option<Value> {
        let c = self.card(id).filter(|c| c.leaving.is_none())?;
        let t = c.bounds.target();
        Some(json!({
            "plugin": c.id,
            "output": self.output,
            "x": c.origin.0,
            "y": c.origin.1,
            "width": f32::from(t.size.width).round(),
            "height": f32::from(t.size.height).round(),
        }))
    }

    /// Springs `id`'s card to `target` in the bar (or fades it out without
    /// one), then hands over to the bar's panel.
    pub fn begin_dock(&mut self, id: &str, target: Option<Bounds<Pixels>>, pinned: bool, window: &mut Window, cx: &mut Context<Self>) {
        let typing_here = self.typing_in(id, window, cx);
        let Some(card) = self.card_mut(id) else { return };
        card.leaving = Some(Leaving::Dock { pinned, snap: target.is_some() });
        card.capture = None;
        match target {
            Some(t) => {
                card.bounds.set_target(t);
            }
            None => {
                let shrunk = shrink(card.bounds.target(), APPEAR_SHRINK);
                card.bounds.set_target(shrunk);
                card.appear.set_target(0.0);
            }
        }
        self.end_drag_of(id);
        if typing_here {
            self.release_keyboard(window, cx);
        }
        cx.notify();
    }

    /// Fades `id`'s card out where it is.
    pub fn begin_close(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let typing_here = self.typing_in(id, window, cx);
        let Some(card) = self.card_mut(id) else { return };
        card.leaving = Some(Leaving::Close);
        card.capture = None;
        let shrunk = shrink(card.bounds.target(), APPEAR_SHRINK);
        card.bounds.set_target(shrunk);
        card.appear.set_target(0.0);
        self.end_drag_of(id);
        if typing_here {
            self.release_keyboard(window, cx);
        }
        cx.notify();
    }

    /// Whether the keyboard bridge is typing into `id`'s card.
    fn typing_in(&self, id: &str, window: &Window, cx: &App) -> bool {
        self.keyboard && self.cards.iter().any(|c| c.id == id && c.focus.contains_focused(window, cx))
    }

    fn end_drag_of(&mut self, id: &str) {
        if self.drag.as_ref().is_some_and(|d| d.id == id) {
            self.drag = None;
        }
    }

    // ── Keyboard ─────────────────────────────────────────────────────────

    /// After a press or a forwarded key: the bridge exists exactly while
    /// something inside a card other than the card itself (a text field, a
    /// keyboard-driven list) is focused.
    pub fn sync_keyboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wants = window.focused(cx).is_some_and(|f| f != self.root_focus && !self.cards.iter().any(|c| c.focus == f));
        if wants != self.keyboard {
            self.keyboard = wants;
            let output = self.output.clone();
            cx.defer(move |cx| super::set_keyboard(&output, wants, cx));
            cx.notify();
        }
    }

    /// Stops typing: focus goes back to the surface, the bridge closes.
    pub fn release_keyboard(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.root_focus.focus(window, cx);
        if self.keyboard {
            self.keyboard = false;
            let output = self.output.clone();
            cx.defer(move |cx| super::set_keyboard(&output, false, cx));
            cx.notify();
        }
    }

    // ── Dragging ─────────────────────────────────────────────────────────

    /// A press on `id`'s card: starts a drag when it landed on the chrome.
    fn start_drag(&mut self, id: &str, e: &MouseDownEvent, cx: &mut Context<Self>) {
        let cfg = config(cx).read(cx).snapshot();
        let Some(card) = self.card(id).filter(|c| c.leaving.is_none()) else { return };
        let b = card.bounds.value();
        let press = (f32::from(e.position.x), f32::from(e.position.y));
        if !geom::on_chrome(rect_of(b), card.content, chrome(&cfg), press) {
            return;
        }
        let grab = (press.0 - f32::from(b.origin.x), press.1 - f32::from(b.origin.y));
        self.drag = Some(Drag { id: id.to_string(), grab, press, pointer: None, moved: false });
        cx.notify();
    }

    /// Any pointer move while dragging (window-wide): remembered for the
    /// next frame, which moves the card.
    fn pointer_moved(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.as_mut() else { return };
        if e.pressed_button != Some(MouseButton::Left) {
            self.end_drag(cx);
            return;
        }
        let p = (f32::from(e.position.x), f32::from(e.position.y));
        if !drag.moved {
            if (p.0 - drag.press.0).hypot(p.1 - drag.press.1) < DRAG_SLOP_PX {
                return;
            }
            drag.moved = true;
        }
        drag.pointer = Some(p);
        cx.notify();
    }

    /// Moves the dragged card under the pointer's latest position (once
    /// per frame, however many moves arrived).
    fn apply_drag(&mut self, cfg: &ShellConfig) {
        let area = self.area(cfg);
        let Some(drag) = self.drag.as_mut() else { return };
        let Some(p) = drag.pointer.take() else { return };
        let grab = drag.grab;
        let Some(card) = self.cards.iter_mut().find(|c| c.id == drag.id && c.leaving.is_none()) else {
            self.drag = None;
            return;
        };
        let t = card.bounds.target();
        let size = (f32::from(t.size.width), f32::from(t.size.height));
        card.origin = geom::clamp_origin((p.0 - grab.0, p.1 - grab.1), size, area, EDGE_MARGIN_PX);
        // The card follows the pointer exactly; only its size springs.
        let cur = card.bounds.value();
        card.bounds.snap_to(Bounds { origin: point(px(card.origin.0), px(card.origin.1)), size: cur.size });
        card.bounds.set_target(bounds_at(card.origin, size));
    }

    /// The button went up: the card lands where the pointer let go and
    /// its place is saved (once per drag).
    fn end_drag(&mut self, cx: &mut Context<Self>) {
        if self.drag.is_none() {
            return;
        }
        let cfg = config(cx).read(cx).snapshot();
        self.apply_drag(&cfg);
        let Some(drag) = self.drag.take() else { return };
        cx.notify();
        if !drag.moved {
            return;
        }
        if let Some(origin) = self.card(&drag.id).map(|c| c.origin) {
            super::persist(&drag.id, &self.output, origin, true, cx);
        }
    }

    // ── Frame ────────────────────────────────────────────────────────────

    /// Follows plugin sizes and settings, steps the springs, retires the
    /// cards that left. Returns whether another frame is needed.
    fn step(&mut self, cfg: &ShellConfig, dt: f32, cx: &mut Context<Self>) -> bool {
        let (area, chrome) = (self.area(cfg), chrome(cfg));
        let (spring, custom) = crate::shell::panel_spring(cfg);
        let dragged = self.drag.as_ref().map(|d| d.id.clone());
        // A plugin switched off, removed or hot-reloaded takes its card.
        let stale: Vec<String> = {
            let registry = cx.global::<mandara_plugins::PluginRegistry>();
            self.cards
                .iter()
                .filter(|c| c.leaving.is_none() && (registry.get(&c.id).map(|f| f.generation) != Some(c.generation) || !registry.is_enabled(&c.id, &cfg.plugins)))
                .map(|c| c.id.clone())
                .collect()
        };
        for id in stale {
            cx.defer(move |cx| {
                super::close(&id, cx);
            });
        }
        let mut animating = false;
        let mut handoffs = Vec::new();
        for card in &mut self.cards {
            card.bounds.set_config(spring);
            if card.leaving.is_none() {
                let want = card.spec.size.get();
                card.content = geom::content_size((f32::from(want.width), f32::from(want.height)), cfg.panels.width_scale(), area, chrome);
                let size = geom::card_size(card.content, chrome);
                if dragged.as_deref() != Some(card.id.as_str()) {
                    card.origin = geom::clamp_origin(card.origin, size, area, EDGE_MARGIN_PX);
                }
                let target = bounds_at(card.origin, size);
                if card.bounds.target() != target {
                    card.bounds.set_target(target);
                }
            }
            card.hover.set_target(if card.hovered || dragged.as_deref() == Some(card.id.as_str()) { 1.0 } else { 0.0 });
            animating |= crate::shell::step_spring(&mut card.bounds, custom, dt);
            animating |= card.appear.step(dt);
            animating |= card.hover.step(dt);
            card.panel_cx.set_progress(card.appear.value());
            match card.leaving {
                Some(Leaving::Dock { pinned, snap: true }) if !card.bounds.is_animating() => {
                    handoffs.push((card.id.clone(), pinned, true));
                    card.leaving = Some(Leaving::Handed(HANDOFF_FRAMES));
                    animating = true;
                }
                Some(Leaving::Dock { pinned, snap: false }) if card.appear.value() <= HIDDEN_EPSILON && !card.appear.is_animating() => {
                    handoffs.push((card.id.clone(), pinned, false));
                    card.leaving = Some(Leaving::Handed(0));
                }
                Some(Leaving::Handed(n)) if n > 0 => {
                    card.leaving = Some(Leaving::Handed(n - 1));
                    animating = true;
                }
                _ => {}
            }
        }
        self.cards.retain(|c| match c.leaving {
            Some(Leaving::Close) => c.appear.value() > HIDDEN_EPSILON || c.appear.is_animating(),
            Some(Leaving::Handed(n)) => n > 0,
            _ => true,
        });
        for (id, pinned, snap) in handoffs {
            let output = self.output.clone();
            cx.defer(move |cx| super::docked(&id, &output, pinned, snap, cx));
        }
        if self.cards.is_empty() {
            let output = self.output.clone();
            cx.defer(move |cx| super::window_emptied(&output, cx));
        }
        animating
    }

    /// Build the card chrome and controls around its cached plugin content.
    fn card_element(&self, card: &Card, cfg: &ShellConfig, palette: &mandara_theme::MandaraPalette, panels_font: SharedString, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let b = card.bounds.value();
        let a = card.appear.value().clamp(0.0, 1.0);
        let reveal = card.hover.value().clamp(0.0, 1.0);
        let chrome = chrome(cfg);
        let radius = crate::style::panel_radius(&cfg.bar);
        let fill = crate::shell::frame_color(palette);
        let fill = fill.with_alpha(fill.alpha() * mandara_config::surfaces::ranges::OPACITY.clamp(cfg.panels.opacity, 1.0));
        let dragging = self.drag.as_ref().is_some_and(|d| d.id == card.id);
        let settled = a >= 1.0 && !card.bounds.is_animating() && card.leaving.is_none();
        let (cw, ch) = card.content;
        let id = card.id.clone();
        let grab_ink = palette.on_surface.with_alpha(GRAB_ALPHA.0 + (GRAB_ALPHA.1 - GRAB_ALPHA.0) * reveal);
        // Toward the bar: where docking sends it.
        let dock_icon = match cfg.bar.position {
            BarPosition::Top => "arrow-up",
            BarPosition::Bottom => "arrow-down",
            BarPosition::Left => "chevron-left",
            BarPosition::Right => "chevron-right",
        };
        let fg = palette.on_surface_variant;
        let (dock_id, close_id) = (id.clone(), id.clone());
        let controls = div()
            .absolute()
            .top(px((STRIP_PX - CONTROL_PX) / 2.0))
            .when(!crate::bidi::rtl(), |el| el.right(px(CONTROL_INSET_PX)))
            .when(crate::bidi::rtl(), |el| el.left(px(CONTROL_INSET_PX)))
            .flex()
            .gap(px(CONTROL_GAP_PX))
            .opacity(reveal)
            // The buttons never start a drag.
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .when_some(card.capture.as_ref(), |row, capture| row.child(crate::panel_capture::button(&capture.state, b, self.surface, CONTROL_PX, fg, palette, window, cx)))
            .child(crate::kit::icon_button(SharedString::from(format!("float-dock-{id}")), dock_icon, CONTROL_PX, fg, None, move |_, cx| {
                let id = dock_id.clone();
                cx.defer(move |cx| {
                    if let Err(err) = super::dock(&id, false, cx) {
                        tracing::warn!("docking `{id}`: {err}");
                    }
                });
            }))
            .child(crate::kit::icon_button(SharedString::from(format!("float-close-{id}")), "close", CONTROL_PX, fg, None, move |_, cx| {
                let id = close_id.clone();
                cx.defer(move |cx| {
                    super::close(&id, cx);
                });
            }));
        let drag_id = id.clone();
        // The chrome's hand cursor: everywhere on the card under the
        // content (which sets its own).
        let grip = div().absolute().top_0().left_0().size_full().cursor(if dragging { CursorStyle::ClosedHand } else { CursorStyle::OpenHand });
        let strip = div()
            .absolute()
            .top_0()
            .left_0()
            .w_full()
            .h(px(STRIP_PX))
            .child(
                div()
                    .absolute()
                    .top(px((STRIP_PX - GRAB_H_PX) / 2.0))
                    .left(px((f32::from(b.size.width) - GRAB_W_PX) / 2.0))
                    .w(px(GRAB_W_PX))
                    .h(px(GRAB_H_PX))
                    .rounded_full()
                    .bg(grab_ink.hsla()),
            )
            .child(controls);
        let content = div()
            .absolute()
            .left(px(chrome.pad))
            .top(px(STRIP_PX))
            .w(px(cw))
            .h(px(ch))
            .cursor(CursorStyle::Arrow)
            .opacity(smoothstep(0.0, 1.0, a))
            // Typography → Panels & plugins font.
            .font_family(panels_font)
            .child(if settled { card.spec.view.clone().cached(StyleRefinement::default().size_full()).into_any_element() } else { card.spec.view.clone().into_any_element() });
        // The plugin sizes itself while it renders: lay the card out again
        // on the next frame when that changed (as docked panels do).
        let size_cell = card.spec.size.clone();
        let read = size_cell.get();
        let resize = canvas(
            move |_, window, _| {
                if size_cell.get() != read {
                    window.request_animation_frame();
                }
            },
            |_, _, _, _| {},
        )
        .absolute()
        .size_0();
        let hover_id = id.clone();
        let raise_id = id;
        div()
            .id(SharedString::from(format!("floating-card-{}", card.id)))
            .absolute()
            .left(b.origin.x)
            .top(b.origin.y)
            .w(b.size.width)
            .h(b.size.height)
            .opacity(a)
            .rounded(px(radius))
            .bg(fill.hsla())
            .border_1()
            .border_color(surface_edge(palette, cfg.theme.adjustments.rim_intensity).hsla())
            .shadow(elevation_scaled(palette, a, cfg.panels.shadow_strength))
            .overflow_hidden()
            // Presses on a card never reach the typing catcher.
            .occlude()
            .track_focus(&card.focus)
            .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                // GPUI re-tests hover on repaint against the last pointer position, stale once the pointer left the surface.
                if *hovered && !window.is_window_hovered() {
                    return;
                }
                if let Some(c) = this.cards.iter_mut().find(|c| c.id == hover_id) {
                    c.hovered = *hovered;
                }
                cx.notify();
            }))
            .capture_any_mouse_down(cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                if this.cards.last().is_some_and(|c| c.id != raise_id)
                    && let Some(i) = this.cards.iter().position(|c| c.id == raise_id)
                {
                    let card = this.cards.remove(i);
                    this.cards.push(card);
                    cx.notify();
                }
            }))
            // Presses on the chrome drag (the dock / close buttons stop
            // theirs; presses on the content are the plugin's).
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| this.start_drag(&drag_id, e, cx)))
            .child(grip)
            .child(content)
            .child(strip)
            .child(resize)
            .into_any_element()
    }
}

impl Render for FloatingView {
    /// Step moving cards at the panel speed and publish their current input bounds.
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Frame timing for Studio → Performance (`diagnostics frames`).
        let _frame = crate::plugins::diagnostics::frame(cx.entity_id().as_u64());
        let now = Instant::now();
        let cfg = config(cx).read(cx).snapshot();
        let _motion = mandara_core::spring::MotionScope::new(mandara_config::motion::surface_speed(cfg.motion.surfaces.panels));
        let palette = mandara_core::frame_palette(window, cx);
        let f = fonts(cx);
        let viewport = window.viewport_size();
        if f32::from(viewport.width) > 1.0 {
            self.surface = (f32::from(viewport.width), f32::from(viewport.height));
        }
        let dt = self.clock.tick_vsync(now, window.refresh_interval(cx));
        self.apply_drag(&cfg);
        if self.step(&cfg, dt, cx) {
            window.request_animation_frame();
        } else {
            self.clock.idle();
        }
        let mut root = div()
            .id("mandara-floating")
            .size_full()
            .relative()
            .track_focus(&self.root_focus)
            .font_family(f.ui.clone())
            .text_start()
            .text_size(px(BASE_TEXT_PX * cfg.typography.scale))
            .font_weight(crate::style::body_weight())
            .text_color(palette.on_surface.hsla())
            // After any press: did it focus something that wants keys?
            .capture_any_mouse_down(cx.listener(|_, _: &MouseDownEvent, window, cx| {
                cx.defer_in(window, |this, window, cx| this.sync_keyboard(window, cx));
            }));
        if self.drag.is_some() {
            // Window-wide, not on an element: the cards occlude everything
            // beneath them, so an element's move / release listeners would
            // miss every event over the dragged card itself.
            let view = cx.entity().downgrade();
            root = root.child(
                canvas(
                    |_, _, _| {},
                    move |_, _, window, _| {
                        let moves = view.clone();
                        window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                let _ = moves.update(cx, |this, cx| this.pointer_moved(e, cx));
                            }
                        });
                        let ups = view.clone();
                        window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
                                let _ = ups.update(cx, |this, cx| this.end_drag(cx));
                            }
                        });
                    },
                )
                .absolute()
                .size_0(),
            );
        }
        // While dragging or typing every press counts: the whole surface
        // joins the input region, beneath the cards. A press outside the
        // cards stops typing (and is used up, like a menu's outside click).
        let everything = self.drag.is_some() || self.keyboard;
        if everything {
            let release = |_: MouseButton| cx.listener(|this: &mut Self, _: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>| this.release_keyboard(window, cx));
            root = root.child(
                div()
                    .id("floating-catcher")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .on_mouse_down(MouseButton::Left, release(MouseButton::Left))
                    .on_mouse_down(MouseButton::Right, release(MouseButton::Right))
                    .on_mouse_down(MouseButton::Middle, release(MouseButton::Middle)),
            );
        }
        let cards: Vec<AnyElement> = self.cards.iter().map(|c| self.card_element(c, &cfg, &palette, f.panels.clone(), window, cx)).collect();
        // The input region: the cards as drawn this frame, or everything.
        let rects: Vec<crate::dismiss::Rect> = self.cards.iter().map(|c| rect_of(c.bounds.value())).collect();
        let all = everything.then(|| crate::dismiss::Rect::new(0.0, 0.0, self.surface.0, self.surface.1));
        for r in geom::input_region(&rects, all) {
            self.mask.register(bounds_of(r));
        }
        root.children(cards).child(self.mask.commit())
    }
}
