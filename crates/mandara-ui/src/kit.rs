//! Shared building blocks for plugin bar items, panels and sidebar cards.
//! Soft radii everywhere, no hard outlines: depth comes from tonal surfaces
//! and 8 % state layers (hover) rather than borders.

use crate::bidi::Bidi as _;
use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    AnyElement, App, Bounds, Div, ElementId, FontWeight, InteractiveElement, IntoElement, MouseButton, MouseDownEvent,
    MouseMoveEvent, ParentElement, Pixels, StatefulInteractiveElement, Styled, Window, canvas, div,
    prelude::FluentBuilder as _, px,
};
use mandara_core::ToGpui;
use mandara_plugins::PanelCx;
use mandara_theme::{Color, MandaraPalette};

use crate::icons::icon;

/// Spacing / radius scale (shared by every surface).
pub mod tokens {
    pub const XS: f32 = 4.0;
    pub const SM: f32 = 8.0;
    pub const MD: f32 = 12.0;
    pub const LG: f32 = 16.0;
    pub const XL: f32 = 20.0;
    pub const XXL: f32 = 28.0;
}

/// State-layer and tint alphas (Material-style overlays on a surface).
pub mod alpha {
    /// Hover state layer on a neutral surface.
    pub const HOVER: f32 = 0.07;
    /// Hover state layer mixed into a filled control.
    pub const HOVER_FILLED: f32 = 0.08;
    /// Pressed / emphasized state layer (icon-button hover tint).
    pub const PRESSED: f32 = 0.1;
    /// Minimum alpha of an icon button's hover fill.
    pub const ICON_BUTTON_HOVER_MIN: f32 = 0.12;
    /// Selected row / pinned control tint.
    pub const SELECTED: f32 = 0.14;
    /// Secondary text on a filled tile.
    pub const SECONDARY_TEXT: f32 = 0.72;
    /// Slider knob drop shadow.
    pub const KNOB_SHADOW: f32 = 0.35;
}

/// Control metrics, in px.
pub mod metrics {
    /// Quick-toggle tile height.
    pub const TILE_H: f32 = 56.0;
    /// Quick-toggle tile icon size.
    pub const TILE_ICON: f32 = 18.0;
    /// Icon glyph size as a fraction of its round button.
    pub const ICON_BUTTON_GLYPH: f32 = 0.48;
    /// Slider hit area, track thickness and knob diameter.
    pub const SLIDER_H: f32 = 22.0;
    pub const SLIDER_TRACK_H: f32 = 8.0;
    pub const SLIDER_KNOB: f32 = 16.0;
    /// Slider knob shadow offset / blur.
    pub const KNOB_SHADOW_Y: f32 = 1.0;
    pub const KNOB_SHADOW_BLUR: f32 = 3.0;
    /// Knob lightness lift over the fill colour.
    pub const KNOB_LIGHTEN: f32 = 0.04;
    /// List row height.
    pub const ROW_H: f32 = 46.0;
    /// Extra radius / gap step on top of the spacing scale.
    pub const NUDGE: f32 = 2.0;
}

/// Seconds per minute / hour (duration formatting).
const SECS_PER_MIN: u64 = 60;
const SECS_PER_HOUR: u64 = 3600;

/// Text sizes.
pub mod text {
    pub const LABEL: f32 = 11.0;
    pub const BODY_S: f32 = 12.0;
    pub const BODY: f32 = 13.0;
    pub const TITLE: f32 = 15.0;
    pub const HEADLINE: f32 = 26.0;
}

/// A tonal card (no border).
pub fn card(palette: &MandaraPalette) -> Div {
    // Children are clipped to the rounded corners.
    crate::style::ClipRounded::clip_rounded(div().rounded(px(crate::style::card_radius())).bg(palette.surface_container.hsla()).p(px(crate::style::card_padding())))
}

/// Section title inside a panel or card.
pub fn title(text: impl Into<gpui::SharedString>, palette: &MandaraPalette) -> impl IntoElement {
    div().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(palette.on_surface_variant.hsla()).child(text.into())
}

/// Applies a staggered entrance to the `index`-th block of a panel.
pub fn reveal(panel: &PanelCx, index: usize, el: impl IntoElement) -> AnyElement {
    let r = panel.reveal(index);
    div().relative().top(px(r.offset)).opacity(r.opacity).child(el).into_any_element()
}

/// A rounded quick-toggle tile (icon + label + optional detail).
pub fn toggle_tile(
    id: impl Into<ElementId>,
    icon_name: &'static str,
    label: impl Into<gpui::SharedString>,
    detail: Option<String>,
    active: bool,
    palette: &MandaraPalette,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let (bg, fg) = if active {
        (palette.primary, palette.on_primary)
    } else {
        (palette.surface_container_high, palette.on_surface)
    };
    div()
        .id(id)
        .flex_1()
        .min_w(px(0.0))
        .h(px(metrics::TILE_H))
        .px(px(tokens::MD))
        .rounded(px(crate::style::radius(tokens::LG)))
        .bg(bg.hsla())
        .hover(move |s| s.bg(bg.mix(fg, alpha::HOVER_FILLED).hsla()))
        .cursor_pointer()
        .flex()
        .items_center()
        .gap(px(tokens::SM + metrics::NUDGE))
        .child(icon(icon_name, metrics::TILE_ICON, fg))
        .child(
            div()
                .flex()
                .flex_col()
                .min_w(px(0.0))
                .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(fg.hsla()).truncate().child(label.into()))
                .when_some(detail, |el, d| {
                    el.child(div().text_size(px(text::LABEL)).text_color(fg.with_alpha(alpha::SECONDARY_TEXT).hsla()).truncate().child(d))
                }),
        )
        .on_click(move |_, window, cx| on_click(window, cx))
}

/// A soft round icon button.
pub fn icon_button(
    id: impl Into<ElementId>,
    icon_name: &'static str,
    size: f32,
    fg: Color,
    bg: Option<Color>,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let hover_bg = bg
        .unwrap_or(fg.with_alpha(0.0))
        .mix(fg, alpha::PRESSED)
        .with_alpha(bg.map(|b| b.alpha()).unwrap_or(alpha::ICON_BUTTON_HOVER_MIN).max(alpha::ICON_BUTTON_HOVER_MIN));
    div()
        .id(id)
        .size(px(size))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .when_some(bg, |el, bg| el.bg(bg.hsla()))
        .hover(move |s| s.bg(hover_bg.hsla()))
        .child(icon(icon_name, size * metrics::ICON_BUTTON_GLYPH, fg))
        // A right press on a button stays there: a row's or overlay's own
        // right-click action (restore, dismiss) must not fire through it.
        .on_mouse_down(gpui::MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, window, cx| on_click(window, cx))
}

/// A horizontal slider (click or drag). `on_change` receives `0..=1`. In
/// right-to-left languages it fills from the right.
pub fn slider(
    id: impl Into<ElementId>,
    value: f32,
    palette: &MandaraPalette,
    on_change: impl Fn(f32, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let value = value.clamp(0.0, 1.0);
    let rtl = crate::bidi::rtl();
    let bounds: Rc<Cell<Bounds<Pixels>>> = Rc::default();
    let on_change = Rc::new(on_change);
    let pick = {
        let bounds = bounds.clone();
        move |x: Pixels| -> f32 {
            let b = bounds.get();
            let w = f32::from(b.size.width).max(1.0);
            let frac = ((f32::from(x) - f32::from(b.origin.x)) / w).clamp(0.0, 1.0);
            if rtl { 1.0 - frac } else { frac }
        }
    };
    let (pick_down, pick_move) = (pick.clone(), pick);
    let (change_down, change_move) = (on_change.clone(), on_change);
    let fill = palette.primary;
    let track = palette.surface_container_high;
    div()
        .id(id)
        .relative()
        .h(px(metrics::SLIDER_H))
        .flex()
        .items_center()
        .cursor_pointer()
        .child(
            div()
                .w_full()
                .h(px(metrics::SLIDER_TRACK_H))
                .rounded_full()
                .bg(track.hsla())
                .row()
                .child(div().h_full().rounded_full().w(gpui::relative(value)).bg(fill.hsla())),
        )
        .child(
            // Knob
            div()
                .absolute()
                .inset_start(gpui::relative(value))
                .when(rtl, |el| el.mr(px(-metrics::SLIDER_KNOB / 2.0)))
                .when(!rtl, |el| el.ml(px(-metrics::SLIDER_KNOB / 2.0)))
                .size(px(metrics::SLIDER_KNOB))
                .rounded_full()
                .bg(palette.primary.lighten(metrics::KNOB_LIGHTEN).hsla())
                .shadow(vec![gpui::BoxShadow {
                    color: palette.specular_rim_shadow.with_alpha(alpha::KNOB_SHADOW).hsla(),
                    offset: gpui::point(px(0.0), px(metrics::KNOB_SHADOW_Y)),
                    blur_radius: px(metrics::KNOB_SHADOW_BLUR),
                    spread_radius: px(0.0),
                    inset: false,
                }]),
        )
        .child(
            canvas(move |b, _, _| bounds.set(b), |_, _, _, _| {})
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
        )
        .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, window, cx| {
            change_down(pick_down(e.position.x), window, cx);
        })
        .on_mouse_move(move |e: &MouseMoveEvent, window, cx| {
            if e.pressed_button == Some(MouseButton::Left) {
                change_move(pick_move(e.position.x), window, cx);
            }
        })
}

/// A list row with leading icon, title/subtitle and trailing content.
pub fn list_row(
    id: impl Into<ElementId>,
    leading: impl IntoElement,
    title: impl Into<gpui::SharedString>,
    subtitle: Option<String>,
    trailing: Option<AnyElement>,
    highlighted: bool,
    palette: &MandaraPalette,
    on_click: impl Fn(&mut Window, &mut App) + 'static,
) -> impl IntoElement {
    let bg = if highlighted { palette.primary.with_alpha(alpha::SELECTED) } else { palette.on_surface.with_alpha(0.0) };
    let hover = palette.on_surface.with_alpha(alpha::HOVER);
    div()
        .id(id)
        .h(px(metrics::ROW_H))
        .px(px(tokens::MD))
        .rounded(px(crate::style::radius(tokens::MD + metrics::NUDGE)))
        .bg(bg.hsla())
        .hover(move |s| if highlighted { s } else { s.bg(hover.hsla()) })
        .cursor_pointer()
        .row()
        .items_center()
        .gap(px(tokens::MD))
        .child(leading)
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(text::BODY)).truncate().child(title.into()))
                .when_some(subtitle, |el, s| {
                    el.child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(s))
                }),
        )
        .when_some(trailing, |el, t| el.child(t))
        .on_click(move |_, window, cx| on_click(window, cx))
}

/// Formats seconds as `m:ss` / `h:mm:ss`.
pub fn format_duration(secs: u64) -> String {
    let (h, m, s) = (secs / SECS_PER_HOUR, (secs / SECS_PER_MIN) % SECS_PER_MIN, secs % SECS_PER_MIN);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// Truncates to `max` chars with an ellipsis.
pub fn ellipsize(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        format!("{}…", s.chars().take(max.saturating_sub(1)).collect::<String>().trim_end())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_and_ellipsis() {
        assert_eq!(format_duration(65), "1:05");
        assert_eq!(format_duration(3725), "1:02:05");
        assert_eq!(ellipsize("Harbor-Main-5GHz", 8), "Harbor-…");
        assert_eq!(ellipsize("short", 8), "short");
    }
}
