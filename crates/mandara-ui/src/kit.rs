//! Shared building blocks for plugin bar items, panels and sidebar cards.
//! Soft radii everywhere, no hard outlines: depth comes from tonal surfaces
//! and 8 % state layers (hover) rather than borders.

use crate::bidi::Bidi as _;

use gpui::{
    AnyElement, App, Div, ElementId, FontWeight, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement,
    Styled, Window, div, prelude::FluentBuilder as _, px,
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
    /// Drop shadow under a floating knob (the snip toolbar's handles).
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
    /// A quick-toggle tile's detail line (an SSID, a device): a touch
    /// smaller than a label, so more of it fits before it ellipsizes.
    pub const CAPTION: f32 = 10.5;
    pub const LABEL: f32 = 11.0;
    pub const BODY_S: f32 = 12.0;
    pub const BODY: f32 = 13.0;
    pub const TITLE: f32 = 15.0;
    pub const HEADLINE: f32 = 26.0;
}

/// A tonal card (no border).
pub fn card(palette: &MandaraPalette) -> Div {
    // Children are clipped to the rounded corners.
    // Shaped and lifted in the card designs of Themes → Components.
    use crate::components::surfaces::{CardSurface as _, Fill};
    div().clipped_card_surface(crate::style::card_radius(), Fill(palette.surface_container, None), Vec::new(), palette).p(px(crate::style::card_padding()))
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
) -> gpui::Stateful<Div> {
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
        // Less room past the text than before the icon: the detail line
        // (a network or device name) keeps as much width as it can.
        .pad_start(px(tokens::MD))
        .pad_end(px(tokens::SM))
        .rounded(px(crate::style::radius(tokens::LG)))
        .bg(bg.hsla())
        .hover(move |s| s.bg(bg.mix(fg, alpha::HOVER_FILLED).hsla()))
        .cursor_pointer()
        // Icon then label in reading order (mirrored right to left).
        .row()
        .items_center()
        .gap(px(tokens::SM + metrics::NUDGE))
        .child(icon(icon_name, metrics::TILE_ICON, fg))
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .min_w(px(0.0))
                .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).text_color(fg.hsla()).truncate().child(label.into()))
                .when_some(detail, |el, d| {
                    el.child(div().text_size(px(text::CAPTION)).text_color(fg.with_alpha(alpha::SECONDARY_TEXT).hsla()).truncate().child(d))
                }),
        )
        .on_click(move |_, window, cx| on_click(window, cx))
}

/// A soft round icon button, shaped in the button design picked under
/// Themes → Components (`style.button_design`).
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
    let id: ElementId = id.into();
    let design = crate::style::button_design();
    let el = div().id(id.clone()).size(px(size)).flex_none();
    let el = if design == crate::components::ButtonDesign::Classic {
        el.rounded_full().when_some(bg, |el, bg| el.bg(bg.hsla())).hover(move |s| s.bg(hover_bg.hsla()))
    } else {
        crate::components::buttons::dress(el, id, size, true, crate::components::buttons::Ink { bg, hover: hover_bg, outline: None }, design, None)
    };
    el.flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .child(icon(icon_name, size * metrics::ICON_BUTTON_GLYPH, fg))
        // A right press on a button stays there: a row's or overlay's own
        // right-click action (restore, dismiss) must not fire through it.
        .on_mouse_down(gpui::MouseButton::Right, |_, _, cx| cx.stop_propagation())
        .on_click(move |_, window, cx| on_click(window, cx))
}

/// A horizontal slider ([`crate::slider`], the standard size). A press
/// jumps to the value under the pointer and drags until release, even
/// outside the track; while held it draws your value, not the owner's
/// echoes ([`crate::slider_model`]). `on_change` writes a value (`0..=1`)
/// and returns the write's completion (`None` for a local value set at
/// once, or nothing). In right-to-left languages it fills from the right.
pub fn slider<R: crate::slider_model::IntoWrite>(
    id: impl Into<ElementId>,
    value: f32,
    palette: &MandaraPalette,
    on_change: impl Fn(f32, &mut Window, &mut App) -> R + 'static,
) -> impl IntoElement {
    crate::slider::slider(id.into(), value, crate::slider::Size::Standard, crate::slider::Look::default(), palette, on_change)
}

/// [`slider`] with a value bubble above the handle while it is held,
/// showing `label` of the value (`0..=1`).
pub fn slider_with_label<R: crate::slider_model::IntoWrite>(
    id: impl Into<ElementId>,
    value: f32,
    palette: &MandaraPalette,
    label: impl Fn(f32) -> gpui::SharedString + 'static,
    on_change: impl Fn(f32, &mut Window, &mut App) -> R + 'static,
) -> impl IntoElement {
    let look = crate::slider::Look { label: Some(std::rc::Rc::new(label)), ..Default::default() };
    crate::slider::slider(id.into(), value, crate::slider::Size::Standard, look, palette, on_change)
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
