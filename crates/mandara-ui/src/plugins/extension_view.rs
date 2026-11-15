//! Draws an extension plugin's view (`wit/extensions.wit`, `ui.view`) with
//! Mandara's own components, in every theme, light or dark, left to right
//! or right to left. Shared by the shell (bar, panel, sidebar, desktop) and
//! Studio (plugin tabs), which each pass a [`Frame`]: where events go and
//! how image handles resolve.
//!
//! Everything a plugin sends is data. Sizes are clamped, icons come from
//! the bundled set, images only through host-issued handles, and canvas
//! marks are plain rectangles, ellipses and lines. Views were already
//! checked against the node, text and mark limits
//! ([`mandara_plugins::extension::check_view`]); the walk here still caps
//! depth and the nodes it draws, since children are indices and a few
//! nodes could list each other.
//!
//! State that must survive a re-render (a field's text and caret, a
//! list's scroll, focus) is keyed by the plugin's node ids and lives only
//! while the node is drawn.

use std::cell::Cell;
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;

use gpui::{
    AnyElement, App, ElementId, Entity, FocusHandle, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, ParentElement, PathBuilder, SharedString, StatefulInteractiveElement, Styled, Window, canvas, div, point,
    StyledImage as _, prelude::FluentBuilder as _, px, relative, uniform_list,
};
use mandara_core::ToGpui;
use mandara_plugins::wasm::{Align, ExtContext, ExtEvent, ExtValue, KeyPress, Kind2, Mark, Node2, Shape, Style, Tone, View2};
use mandara_theme::{Color, MandaraPalette};

use crate::icons::icon;
use crate::kit::{card, text, tokens};
use crate::style::ClipRounded as _;
use crate::text_field::TextField;

/// Deepest nesting drawn.
const MAX_DEPTH: u32 = 32;
/// Most nodes drawn per view (a list's rows count when they show).
const MAX_DRAWN: usize = mandara_plugins::extension::MAX_VIEW_NODES;
/// Largest width / height / min size a node may ask for (logical px).
const MAX_DIM_PX: f32 = 4096.0;
/// Largest padding, gap and radius (logical px).
const MAX_SPACE_PX: f32 = 64.0;
/// Font sizes a node may ask for.
const MIN_TEXT_PX: f32 = 6.0;
const MAX_TEXT_PX: f32 = 96.0;
/// Default icon size.
const ICON_PX: f32 = 16.0;
/// Button geometry, and a quiet button's tint.
const BUTTON_H_PX: f32 = 28.0;
const BUTTON_PAD_X_PX: f32 = 12.0;
const BUTTON_GAP_PX: f32 = 6.0;
const BUTTON_TINT_ALPHA: f32 = 0.08;
/// Segmented control geometry: track, segment, inset and radii, the
/// track's tint and the raised segment's shadow.
const SEGMENT_H_PX: f32 = 28.0;
const SEGMENT_PAD_X_PX: f32 = 10.0;
const SEGMENT_INSET_PX: f32 = 2.0;
const SEGMENT_TRACK_RADIUS_PX: f32 = 9.0;
const SEGMENT_RADIUS_PX: f32 = 7.0;
const SEGMENT_TRACK_ALPHA: f32 = 0.07;
const SEGMENT_SHADOW_ALPHA: f32 = 0.18;
const SEGMENT_SHADOW_BLUR_PX: f32 = 3.0;
/// How far a dark theme's raised segment lifts towards the text colour.
const SEGMENT_RAISE_DARK: f32 = 0.2;
/// An inset group: its radius, row padding and the hairline between rows
/// (inset from the leading edge, like a settings list).
const GROUP_RADIUS_PX: f32 = 12.0;
const GROUP_ROW_PAD_X_PX: f32 = 12.0;
const GROUP_ROW_PAD_Y_PX: f32 = 10.0;
const GROUP_TINT_ALPHA: f32 = 0.045;
const HAIRLINE_ALPHA: f32 = 0.08;
/// Meter geometry and track alpha.
const METER_H_PX: f32 = 4.0;
const METER_MIN_W_PX: f32 = 40.0;
const TRACK_ALPHA: f32 = 0.1;
/// A divider's thickness and alpha.
const DIVIDER_PX: f32 = 1.0;
const DIVIDER_ALPHA: f32 = 0.12;
/// Disabled controls are drawn at this opacity.
const DISABLED_ALPHA: f32 = 0.45;
/// A focused interactive node's ring.
const FOCUS_RING_ALPHA: f32 = 0.5;
const FOCUS_RING_PX: f32 = 2.0;
/// Hover tint on clickable nodes.
const HOVER_ALPHA: f32 = 0.07;
/// Default scroll area and list heights, and a list row's default height.
const SCROLL_H_PX: f32 = 240.0;
const LIST_ROW_H_PX: f32 = 36.0;
const LIST_MIN_ROW_PX: f32 = 16.0;
/// Placeholder rows a list draws while the plugin hasn't sent them yet.
const PLACEHOLDER_ALPHA: f32 = 0.06;
/// A canvas's narrowest width and default height, and an image's default size.
const CANVAS_MIN_W_PX: f32 = 48.0;
const CANVAS_H_PX: f32 = 120.0;
const IMAGE_PX: f32 = 64.0;
/// A canvas line's thickness.
const LINE_PX: f32 = 2.0;
/// Grid columns allowed.
const MAX_GRID_COLUMNS: u32 = 12;

/// Where events go: the host delivers them to the plugin (and runs what it
/// queued), then re-renders.
pub type Sink = Rc<dyn Fn(ExtEvent, &mut Window, &mut App)>;

/// Resolves an image handle to a file to draw (`None`: not this plugin's,
/// or no longer allowed). Every picture is decoded at the size it is
/// drawn, by the shared bounded thumbnail worker.
pub type Images = Rc<dyn Fn(u64, &App) -> Option<PathBuf>>;

/// What the renderer needs from its host.
#[derive(Clone)]
pub struct Frame {
    /// Unique per plugin surface: element ids and node state hang off it.
    pub key: SharedString,
    pub context: ExtContext,
    pub sink: Sink,
    pub images: Images,
    /// The colour emoji family, for text that is only emoji.
    pub emoji: Option<SharedString>,
}

impl Frame {
    fn event(&self, node: &Node2, value: ExtValue) -> ExtEvent {
        ExtEvent { context: self.context.clone(), node: node.id.clone(), action: node.action.clone(), value }
    }

    fn element_id(&self, node: &Node2, at: usize) -> ElementId {
        if node.id.is_empty() { ElementId::Name(format!("{}#{at}", self.key).into()) } else { ElementId::Name(format!("{}/{}", self.key, node.id).into()) }
    }

    fn state_key(&self, node: &Node2, what: &str) -> SharedString {
        format!("{}/{}/{what}", self.key, node.id).into()
    }
}

/// The theme colour of a tone.
pub fn tone_color(t: Tone, p: &MandaraPalette) -> Color {
    match t {
        Tone::Normal => p.on_surface,
        Tone::Muted => p.on_surface_variant,
        Tone::Accent => p.primary,
        Tone::Error => p.urgent_coral,
        Tone::Success => p.success_emerald,
        Tone::Warning => p.warning_amber,
    }
}

/// A plugin-supplied number made safe for layout: finite and in range.
fn clamp(v: f32, max: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, max) } else { 0.0 }
}

/// Draws `view` (already checked) for `frame`.
pub fn render(view: View2, p: &MandaraPalette, frame: &Frame, window: &mut Window, cx: &mut App) -> AnyElement {
    let view = Rc::new(view);
    request_focus(&view, frame, window, cx);
    let mut drawn = 0;
    node(&view, view.root, 0, p, frame, &mut drawn, window, cx)
}

/// Gives keyboard focus to `view.focus` once each time the plugin names a
/// different node (re-rendering never steals focus back).
fn request_focus(view: &View2, frame: &Frame, window: &mut Window, cx: &mut App) {
    let asked = window.use_keyed_state(SharedString::from(format!("{}/focus-asked", frame.key)), cx, |_, _| String::new());
    if asked.read(cx).as_str() == view.focus {
        return;
    }
    asked.update(cx, |a, _| a.clone_from(&view.focus));
    let Some(target) = view.nodes.iter().find(|n| !n.id.is_empty() && n.id == view.focus) else { return };
    if target.kind == Kind2::Input {
        let field = field_state(target, frame, window, cx);
        field.update(cx, |f, cx| f.focus(window, cx));
    } else {
        focus_handle(target, frame, window, cx).focus(window, cx);
    }
}

fn focus_handle(n: &Node2, frame: &Frame, window: &mut Window, cx: &mut App) -> FocusHandle {
    window.use_keyed_state(frame.state_key(n, "focus"), cx, |_, cx| cx.focus_handle().tab_stop(true)).read(cx).clone()
}

/// Where a field's typing goes: refreshed on every render, so a plugin that
/// changes the node's action (or a reloaded plugin's new sink) is followed.
type FieldRoute = Rc<std::cell::RefCell<(Frame, Node2)>>;

fn field_state(n: &Node2, frame: &Frame, window: &mut Window, cx: &mut App) -> Entity<TextField> {
    let template = Node2 { children: Vec::new(), options: Vec::new(), marks: Vec::new(), ..n.clone() };
    let route: Entity<FieldRoute> = window.use_keyed_state(frame.state_key(n, "route"), cx, {
        let (frame, template) = (frame.clone(), template.clone());
        move |_, _| Rc::new(std::cell::RefCell::new((frame, template)))
    });
    let route = route.read(cx).clone();
    *route.borrow_mut() = (frame.clone(), template);
    let entity = window.use_keyed_state(frame.state_key(n, "field"), cx, move |_, cx| {
        let on_commit: crate::text_field::OnCommit = Rc::new(move |value, window, cx| {
            let (frame, template) = route.borrow().clone();
            if !template.disabled {
                (frame.sink)(frame.event(&template, ExtValue::Text(value)), window, cx);
            }
        });
        TextField::new(String::new(), String::new(), true, on_commit, cx).max_bytes(mandara_plugins::extension::MAX_PAYLOAD_BYTES)
    });
    entity.update(cx, |f, _| f.sync(&n.text));
    entity
}

/// Applies a node's style to `el`.
fn styled(el: gpui::Stateful<gpui::Div>, s: &Style) -> gpui::Stateful<gpui::Div> {
    let dim = |v: f32| clamp(v, MAX_DIM_PX);
    let space = |v: f32| clamp(v, MAX_SPACE_PX);
    let mut el = el;
    if dim(s.width) > 0.0 {
        el = el.w(px(dim(s.width))).flex_none();
    }
    if dim(s.height) > 0.0 {
        el = el.h(px(dim(s.height)));
    }
    if dim(s.min_width) > 0.0 {
        el = el.min_w(px(dim(s.min_width)));
    }
    if dim(s.min_height) > 0.0 {
        el = el.min_h(px(dim(s.min_height)));
    }
    if space(s.padding) > 0.0 {
        el = el.p(px(space(s.padding)));
    }
    if space(s.radius) > 0.0 {
        el = el.rounded(px(space(s.radius)));
    }
    if s.grow {
        el = el.flex_1().min_w(px(0.0));
    }
    el
}

fn aligned(el: gpui::Stateful<gpui::Div>, a: Align) -> gpui::Stateful<gpui::Div> {
    match a {
        Align::Auto => el,
        Align::Start => el.items_start(),
        Align::Center => el.items_center(),
        Align::End => el.items_end(),
        Align::Stretch => el,
    }
}

fn text_px(n: &Node2, default: f32) -> f32 {
    if n.style.size > 0.0 && n.style.size.is_finite() { n.style.size.clamp(MIN_TEXT_PX, MAX_TEXT_PX) } else { default }
}

fn gap(n: &Node2, default: f32) -> f32 {
    let g = clamp(n.style.gap, MAX_SPACE_PX);
    if g > 0.0 { g } else { default }
}

fn interactive(n: &Node2) -> bool {
    !n.action.is_empty() && !n.disabled
}

#[allow(clippy::too_many_arguments)]
fn node(view: &Rc<View2>, i: u32, depth: u32, p: &MandaraPalette, frame: &Frame, drawn: &mut usize, window: &mut Window, cx: &mut App) -> AnyElement {
    let Some(n) = view.nodes.get(i as usize).filter(|_| depth < MAX_DEPTH && *drawn < MAX_DRAWN) else { return div().into_any_element() };
    *drawn += 1;
    let at = *drawn;
    let color = tone_color(n.style.tone, p);
    let id = frame.element_id(n, at);
    let mut children = |window: &mut Window, cx: &mut App| -> Vec<AnyElement> { n.children.iter().map(|c| node(view, *c, depth + 1, p, frame, drawn, window, cx)).collect() };
    let el: AnyElement = match n.kind {
        Kind2::Row => aligned(styled(div().id(id).flex().flex_row().items_center().gap(px(gap(n, 0.0))), &n.style), n.style.align).children(children(window, cx)).into_any_element(),
        Kind2::Column => aligned(styled(div().id(id).flex().flex_col().gap(px(gap(n, 0.0))), &n.style), n.style.align).children(children(window, cx)).into_any_element(),
        Kind2::Card => {
            let body = styled(card(p).id(id).flex().flex_col().gap(px(gap(n, tokens::SM))), &n.style).children(children(window, cx));
            clickable(body, n, p, frame, window, cx)
        }
        Kind2::Grid => {
            let cols = n.index.clamp(1, MAX_GRID_COLUMNS) as u16;
            styled(div().id(id).grid().grid_cols(cols).gap(px(gap(n, tokens::SM))), &n.style).children(children(window, cx)).into_any_element()
        }
        Kind2::Scroll => {
            let h = if n.style.height > 0.0 { clamp(n.style.height, MAX_DIM_PX) } else { SCROLL_H_PX };
            styled(div().id(id).flex().flex_col().gap(px(gap(n, 0.0))).overflow_y_scroll(), &n.style).h(px(h)).children(children(window, cx)).into_any_element()
        }
        Kind2::List => list(view, n, i, depth, p, frame, window, cx),
        Kind2::Text => {
            let el = styled(div().id(id), &n.style)
                .text_size(px(text_px(n, text::BODY)))
                .text_color(color.hsla())
                .when(n.style.bold, |el| el.font_weight(FontWeight::SEMIBOLD))
                .when_some(frame.emoji.clone().filter(|_| crate::fonts::emoji_only(&n.text)), |el, family| el.font_family(family))
                .child(crate::bidi::paragraph(n.text.clone()));
            clickable(el, n, p, frame, window, cx)
        }
        Kind2::Icon => {
            let name = crate::icons::ICON_NAMES.iter().find(|x| **x == n.text).copied().unwrap_or("circle");
            clickable(styled(div().id(id), &n.style).child(icon(name, text_px(n, ICON_PX), color)), n, p, frame, window, cx)
        }
        Kind2::Button => {
            // One filled primary action, plain text buttons and quiet
            // tinted ones: never a wall of filled pills.
            let (bg, fg) = match n.style.tone {
                Tone::Accent => (Some(p.primary), p.on_primary),
                Tone::Muted => (None, p.primary),
                Tone::Normal => (Some(p.on_surface.with_alpha(BUTTON_TINT_ALPHA)), p.on_surface),
                _ => (Some(color.with_alpha(BUTTON_TINT_ALPHA)), color),
            };
            let el = styled(div().id(id), &n.style)
                .h(px(BUTTON_H_PX))
                .px(px(BUTTON_PAD_X_PX))
                .rounded_full()
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .gap(px(BUTTON_GAP_PX))
                .when_some(bg, |el, bg| el.bg(bg.hsla()))
                .text_color(fg.hsla())
                .text_size(px(text_px(n, text::BODY_S)))
                .font_weight(FontWeight::MEDIUM)
                .children(children(window, cx))
                .when(!n.text.is_empty(), |el| el.child(n.text.clone()));
            clickable(el, n, p, frame, window, cx)
        }
        Kind2::Group => {
            let rows = children(window, cx);
            let count = rows.len();
            let hairline = p.on_surface.with_alpha(HAIRLINE_ALPHA);
            let mut group = styled(div().id(id).flex().flex_col().w_full().rounded(px(GROUP_RADIUS_PX)).bg(p.on_surface.with_alpha(GROUP_TINT_ALPHA).hsla()), &n.style).clip_rounded();
            for (at, row) in rows.into_iter().enumerate() {
                group = group.child(div().flex().flex_col().px(px(GROUP_ROW_PAD_X_PX)).py(px(GROUP_ROW_PAD_Y_PX)).child(row));
                if at + 1 < count {
                    group = group.child(div().h(px(DIVIDER_PX)).ml(px(GROUP_ROW_PAD_X_PX)).bg(hairline.hsla()));
                }
            }
            group.into_any_element()
        }
        Kind2::Meter => styled(div().id(id), &n.style)
            .h(px(METER_H_PX))
            .w_full()
            .min_w(px(METER_MIN_W_PX))
            .rounded_full()
            .bg(p.on_surface.with_alpha(TRACK_ALPHA).hsla())
            .child(div().h_full().rounded_full().w(relative(fraction(n.value))).bg(color.hsla()))
            .into_any_element(),
        Kind2::Spacer => div().flex_1().into_any_element(),
        Kind2::Divider => styled(div().id(id), &n.style).h(px(DIVIDER_PX)).w_full().flex_none().bg(p.on_surface.with_alpha(DIVIDER_ALPHA).hsla()).into_any_element(),
        // A disabled field is drawn, not editable: no caret, typing or paste.
        Kind2::Input if n.disabled => styled(div().id(id), &n.style)
            .when(n.style.width <= 0.0, |el| el.w_full())
            .opacity(DISABLED_ALPHA)
            .px(px(tokens::MD))
            .py(px(tokens::SM))
            .rounded(px(tokens::SM))
            .bg(p.on_surface.with_alpha(PLACEHOLDER_ALPHA).hsla())
            .text_size(px(text::BODY))
            .text_color(if n.text.is_empty() { p.on_surface_variant.hsla() } else { p.on_surface.hsla() })
            .child(if n.text.is_empty() { n.hint.clone() } else { n.text.clone() })
            .into_any_element(),
        Kind2::Input => {
            let field = field_state(n, frame, window, cx);
            field.update(cx, |f, _| f.set_placeholder(&n.hint));
            let (sink, frame2, template) = (frame.sink.clone(), frame.clone(), n.clone());
            let submit = field.clone();
            styled(div().id(id), &n.style)
                .when(n.style.width <= 0.0, |el| el.w_full())
                // Enter submits; the field itself already sent `text`.
                .capture_key_down(move |e: &KeyDownEvent, window, cx| {
                    if e.keystroke.key == "enter" {
                        let value = submit.read(cx).value.clone();
                        sink(frame2.event(&template, ExtValue::Submit(value)), window, cx);
                    }
                })
                .child(field)
                .into_any_element()
        }
        Kind2::Toggle => {
            let on = n.value >= 0.5;
            let el = styled(div().id(id), &n.style)
                .flex()
                .items_center()
                .gap(px(tokens::SM))
                .when(!n.text.is_empty(), |el| el.child(div().flex_1().min_w(px(0.0)).text_size(px(text_px(n, text::BODY))).text_color(p.on_surface.hsla()).child(n.text.clone())))
                .child(crate::components::toggles::switch(on, crate::style::toggle_design(), p));
            control(el, n, ExtValue::Checked(!on), p, frame, window, cx)
        }
        Kind2::Slider => slider(n, id, frame.element_id(n, at + MAX_DRAWN), p, frame),
        Kind2::Choice => choice(n, id, p, frame, window, cx),
        Kind2::Image => image(n, id, p, frame, window, cx),
        Kind2::Canvas => canvas_marks(n, id, p),
    };
    el
}

fn fraction(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

/// A clickable node: hover tint, focus ring, Enter / Space to activate and
/// other keys sent to the plugin.
fn clickable(el: gpui::Stateful<gpui::Div>, n: &Node2, p: &MandaraPalette, frame: &Frame, window: &mut Window, cx: &mut App) -> AnyElement {
    if !interactive(n) {
        return el.when(n.disabled, |el| el.opacity(DISABLED_ALPHA)).into_any_element();
    }
    control(el, n, ExtValue::Activate, p, frame, window, cx)
}

fn control(el: gpui::Stateful<gpui::Div>, n: &Node2, click: ExtValue, p: &MandaraPalette, frame: &Frame, window: &mut Window, cx: &mut App) -> AnyElement {
    if !interactive(n) {
        return el.when(n.disabled, |el| el.opacity(DISABLED_ALPHA)).into_any_element();
    }
    let focus = (!n.id.is_empty()).then(|| focus_handle(n, frame, window, cx));
    let (sink, f, template, value) = (frame.sink.clone(), frame.clone(), n.clone(), click.clone());
    let (ksink, kf, ktemplate) = (frame.sink.clone(), frame.clone(), n.clone());
    el.cursor_pointer()
        .hover(|s| s.bg(p.on_surface.with_alpha(HOVER_ALPHA).hsla()))
        .when_some(focus, |el, focus| {
            el.track_focus(&focus).focus(|s| s.border(px(FOCUS_RING_PX)).border_color(p.primary.with_alpha(FOCUS_RING_ALPHA).hsla())).on_key_down(move |e: &KeyDownEvent, window, cx| {
                let k = &e.keystroke;
                let value = match k.key.as_str() {
                    "enter" | "space" if !k.modifiers.control && !k.modifiers.alt => click.clone(),
                    _ => ExtValue::Key(KeyPress { key: k.key.clone(), ctrl: k.modifiers.control, alt: k.modifiers.alt, shift: k.modifiers.shift, super_key: k.modifiers.platform }),
                };
                ksink(kf.event(&ktemplate, value), window, cx);
                cx.stop_propagation();
            })
        })
        .on_click(move |_, window, cx| sink(f.event(&template, value.clone()), window, cx))
        .into_any_element()
}

fn slider(n: &Node2, id: ElementId, track: ElementId, p: &MandaraPalette, frame: &Frame) -> AnyElement {
    let (min, max) = if n.min.is_finite() && n.max.is_finite() && n.max > n.min { (n.min, n.max) } else { (0.0, 1.0) };
    let step = if n.step.is_finite() && n.step > 0.0 { n.step } else { 0.0 };
    let at = if n.value.is_finite() { ((n.value - min) / (max - min)).clamp(0.0, 1.0) } else { 0.0 };
    let (sink, f, template) = (frame.sink.clone(), frame.clone(), n.clone());
    let last = Rc::new(Cell::new(n.value));
    let body = crate::kit::slider(track, at, p, move |v, window, cx| {
        if template.disabled {
            return;
        }
        let mut value = min + v * (max - min);
        if step > 0.0 {
            value = (min + ((value - min) / step).round() * step).clamp(min, max);
        }
        // One event per value the person reaches, not per pointer move.
        if last.replace(value) != value {
            sink(f.event(&template, ExtValue::Number(value)), window, cx);
        }
    });
    styled(div().id(id), &n.style)
        .when(n.style.width <= 0.0, |el| el.w_full())
        .when(n.disabled, |el| el.opacity(DISABLED_ALPHA))
        .child(body)
        .into_any_element()
}

fn choice(n: &Node2, id: ElementId, p: &MandaraPalette, frame: &Frame, window: &mut Window, cx: &mut App) -> AnyElement {
    let track = styled(div().id(id), &n.style)
        .when(n.style.width <= 0.0, |el| el.w_full())
        .flex()
        .flex_row()
        .items_center()
        .h(px(SEGMENT_H_PX))
        .p(px(SEGMENT_INSET_PX))
        .gap(px(SEGMENT_INSET_PX))
        .rounded(px(SEGMENT_TRACK_RADIUS_PX))
        .bg(p.on_surface.with_alpha(SEGMENT_TRACK_ALPHA).hsla())
        .when(n.disabled, |el| el.opacity(DISABLED_ALPHA));
    let mut track = track;
    // The chosen segment sits raised on the track: the surface itself in
    // light themes, a lifted tint of it in dark ones.
    let raised = if p.is_dark { p.surface_base.mix(p.on_surface, SEGMENT_RAISE_DARK) } else { p.surface_base };
    for (at, option) in n.options.iter().enumerate() {
        let chosen = at as u32 == n.index;
        let segment = div()
            .id(SharedString::from(format!("{}/{}/opt{at}", frame.key, n.id)))
            .flex_1()
            .min_w(px(0.0))
            .h_full()
            .px(px(SEGMENT_PAD_X_PX))
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(SEGMENT_RADIUS_PX))
            .text_size(px(text_px(n, text::BODY_S)))
            .font_weight(FontWeight::MEDIUM)
            .truncate()
            .when(chosen, |el| {
                el.bg(raised.hsla()).text_color(p.on_surface.hsla()).shadow(vec![gpui::BoxShadow {
                    color: gpui::black().opacity(SEGMENT_SHADOW_ALPHA),
                    offset: point(px(0.0), px(DIVIDER_PX)),
                    blur_radius: px(SEGMENT_SHADOW_BLUR_PX),
                    spread_radius: px(0.0),
                    inset: false,
                }])
            })
            .when(!chosen, |el| el.text_color(p.on_surface_variant.hsla()))
            .child(option.label.clone());
        let segment = if interactive(n) && !chosen {
            let (sink, f, template, value) = (frame.sink.clone(), frame.clone(), n.clone(), option.value.clone());
            segment.cursor_pointer().hover(|s| s.text_color(p.on_surface.hsla())).on_click(move |_, window, cx| sink(f.event(&template, ExtValue::Choice(value.clone())), window, cx))
        } else {
            segment
        };
        track = track.child(segment);
    }
    // Keyboard: focus the control, then ← / → (in reading order) and
    // Home / End pick an option.
    if interactive(n) && !n.id.is_empty() && !n.options.is_empty() {
        let focus = focus_handle(n, frame, window, cx);
        let (sink, f, template) = (frame.sink.clone(), frame.clone(), n.clone());
        track = track.track_focus(&focus).focus(|s| s.border(px(FOCUS_RING_PX)).border_color(p.primary.with_alpha(FOCUS_RING_ALPHA).hsla())).on_key_down(move |e: &KeyDownEvent, window, cx| {
            let (count, at) = (template.options.len(), (template.index as usize).min(template.options.len() - 1));
            let rtl = mandara_i18n::language().is_rtl();
            let next = match e.keystroke.key.as_str() {
                "right" if !rtl => (at + 1).min(count - 1),
                "left" if rtl => (at + 1).min(count - 1),
                "left" | "right" => at.saturating_sub(1),
                "home" => 0,
                "end" => count - 1,
                _ => return,
            };
            cx.stop_propagation();
            if next != at {
                sink(f.event(&template, ExtValue::Choice(template.options[next].value.clone())), window, cx);
            }
        });
    }
    track.into_any_element()
}

fn image(n: &Node2, id: ElementId, p: &MandaraPalette, frame: &Frame, window: &mut Window, cx: &mut App) -> AnyElement {
    let w = if n.style.width > 0.0 { clamp(n.style.width, MAX_DIM_PX) } else { IMAGE_PX };
    let h = if n.style.height > 0.0 { clamp(n.style.height, MAX_DIM_PX) } else { w };
    let radius = clamp(n.style.radius, MAX_SPACE_PX);
    let shell = styled(div().id(id), &n.style).w(px(w)).h(px(h)).flex_none().rounded(px(radius)).overflow_hidden();
    let source = (n.image != 0).then(|| (frame.images)(n.image, cx)).flatten();
    let drawn = source.and_then(|path| {
        // Decoded once at the physical size it is drawn at, by the shared
        // bounded thumbnail worker (cached on disk, never full size here).
        let scale = window.scale_factor();
        let job = (path, crate::wallpaper::fit_size(w * scale), crate::wallpaper::fit_size(h * scale) | crate::wallpaper::FIT_JOB);
        let thumbs = crate::wallpaper::thumbnails(cx);
        let ready = thumbs.read(cx).get_job(&job).cloned();
        if ready.is_none() && !thumbs.read(cx).failed_job(&job) {
            thumbs.update(cx, |t, cx| t.request_jobs([job], cx));
        }
        ready
    });
    // A soft placeholder until the picture is decoded; none behind it after
    // (pictures with transparency keep their own shape).
    let shell = match drawn {
        Some(path) => shell.child(gpui::img(path).size_full().rounded(px(radius)).object_fit(gpui::ObjectFit::Cover)),
        None => shell.bg(p.on_surface.with_alpha(PLACEHOLDER_ALPHA).hsla()),
    };
    clickable(shell, n, p, frame, window, cx)
}

fn canvas_marks(n: &Node2, id: ElementId, p: &MandaraPalette) -> AnyElement {
    let h = if n.style.height > 0.0 { clamp(n.style.height, MAX_DIM_PX) } else { CANVAS_H_PX };
    // Without a width a canvas fills its parent (charts span their card).
    let el = styled(div().id(id), &n.style).relative().h(px(h)).flex_none();
    let mut el = if n.style.width > 0.0 { el.w(px(clamp(n.style.width, MAX_DIM_PX))) } else { el.w_full().min_w(px(CANVAS_MIN_W_PX)) };
    let mut lines: Vec<(Mark, Color)> = Vec::new();
    for m in &n.marks {
        let color = tone_color(m.tone, p).with_alpha(if m.opacity > 0.0 && m.opacity.is_finite() { m.opacity.min(1.0) } else { 1.0 });
        let (x, y) = (unit(m.x), unit(m.y));
        match m.shape {
            Shape::Rect | Shape::Ellipse => {
                let (mw, mh) = (unit(m.width).min(1.0 - x), unit(m.height).min(1.0 - y));
                let shape = div().absolute().left(relative(x)).top(relative(y)).w(relative(mw)).h(relative(mh)).bg(color.hsla());
                el = el.child(if m.shape == Shape::Ellipse { shape.rounded_full() } else { shape.rounded(px(clamp(m.radius, MAX_SPACE_PX))) });
            }
            Shape::Line => lines.push((*m, color)),
        }
    }
    if !lines.is_empty() {
        el = el.child(
            canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let size = bounds.size;
                    for (m, color) in &lines {
                        let mut path = PathBuilder::stroke(px(LINE_PX));
                        let at = |fx: f32, fy: f32| point(bounds.origin.x + size.width * unit(fx), bounds.origin.y + size.height * unit(fy));
                        path.move_to(at(m.x, m.y));
                        path.line_to(at(m.x + m.width, m.y + m.height));
                        if let Ok(path) = path.build() {
                            window.paint_path(path, color.hsla());
                        }
                    }
                },
            )
            .absolute()
            .size_full(),
        );
    }
    el.into_any_element()
}

/// A mark coordinate clamped to the canvas.
fn unit(v: f32) -> f32 {
    if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.0 }
}

/// A virtual list: `index` rows of a uniform height; the plugin sends the
/// rows `value .. value + children` and gets a `range` event when the rows
/// on screen move outside them. Rows not sent yet draw as placeholders.
#[allow(clippy::too_many_arguments)]
fn list(view: &Rc<View2>, n: &Node2, i: u32, depth: u32, p: &MandaraPalette, frame: &Frame, window: &mut Window, cx: &mut App) -> AnyElement {
    let count = n.index.min(mandara_plugins::extension::MAX_LIST_ROWS) as usize;
    let start = if n.value.is_finite() && n.value > 0.0 { n.value as usize } else { 0 };
    // At least a readable row: tiny rows would put thousands on screen.
    let row_h = if n.style.min_height > 0.0 { clamp(n.style.min_height, MAX_DIM_PX).max(LIST_MIN_ROW_PX) } else { LIST_ROW_H_PX };
    let h = if n.style.height > 0.0 { clamp(n.style.height, MAX_DIM_PX) } else { SCROLL_H_PX };
    let asked = window.use_keyed_state(frame.state_key(n, "range"), cx, |_, _| Rc::new(Cell::new((u32::MAX, 0u32))));
    let asked = asked.read(cx).clone();
    let (view, frame2, palette, template) = (view.clone(), frame.clone(), p.clone(), n.clone());
    let sent = n.children.len();
    uniform_list(frame.element_id(n, i as usize), count, move |range: Range<usize>, window, cx| {
        let covered = range.start >= start && range.end <= start + sent;
        let want = (range.start as u32, range.len() as u32);
        if !covered && asked.get() != want {
            asked.set(want);
            let (sink, event) = (frame2.sink.clone(), frame2.event(&template, ExtValue::Range(want)));
            window.defer(cx, move |window, cx| sink(event, window, cx));
        }
        let mut drawn = 0;
        range
            .map(|row| {
                let shown = row.checked_sub(start).and_then(|k| template.children.get(k)).copied();
                let body = match shown {
                    Some(child) => node(&view, child, depth + 1, &palette, &frame2, &mut drawn, window, cx),
                    None => div().w_full().h(px(row_h * PLACEHOLDER_FILL)).rounded(px(tokens::SM)).bg(palette.on_surface.with_alpha(PLACEHOLDER_ALPHA).hsla()).into_any_element(),
                };
                // A row the plugin marks `grow` fills the list's width.
                div().id(row).h(px(row_h)).w_full().flex().items_center().child(body).into_any_element()
            })
            .collect::<Vec<_>>()
    })
    .h(px(h))
    .w_full()
    .into_any_element()
}

/// How much of a placeholder row's height its bar fills.
const PLACEHOLDER_FILL: f32 = 0.6;
