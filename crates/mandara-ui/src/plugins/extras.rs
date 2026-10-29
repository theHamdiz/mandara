//! Small shared pieces for the utility plugins (world clock, net speed,
//! authenticator, color picker, soundscapes): segmented controls, search
//! boxes, transient toasts, monogram avatars and a time-limited flash
//! state, all in the panel_fx visual vocabulary.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Instant;

use gpui::{
    AnyElement, App, Entity, FontWeight, SharedString, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, Window, div,
    prelude::FluentBuilder as _, px,
};
use mandara_core::ToGpui;
use mandara_theme::{Color, MandaraPalette};

use super::panel_fx::{ANGLE_DIAGONAL_DEG, EXPIRY_MARGIN, SMALL_ICON_PX, WASH_SOFT, glow, readable, redraw_in, wash};
use crate::icons::icon;
use crate::kit::{icon_button, text};
use crate::text_field::TextField;

/// Height of [`segmented`] and [`search_box`].
pub const CONTROL_H: f32 = 34.0;
/// The quick toast stays this share of `panels.toast_ms` (2.6 s at the
/// default 4 s).
const QUICK_TOAST_SHARE: f64 = 0.65;
const TOAST_H: f32 = 36.0;
/// Corner radius of segmented controls and search boxes.
const CONTROL_RADIUS_PX: f32 = 12.0;

// Segmented control.
const SEG_PAD_PX: f32 = 3.0;
const SEG_GAP_PX: f32 = 2.0;
const SEG_PAD_X_PX: f32 = 10.0;
const SEG_RADIUS_PX: f32 = 9.0;
const SEG_ICON_GAP_PX: f32 = 5.0;
/// Segment labels: half a step above the label size.
const SEG_TEXT_PX: f32 = text::LABEL + 0.5;
/// Track wash and active-segment fill: `(dark, light)` alphas.
const SEG_TRACK: (f32, f32) = (0.05, 0.045);
const SEG_ACTIVE: (f32, f32) = (0.18, 0.13);

// Search box.
const SEARCH_PAD_L_PX: f32 = 12.0;
const SEARCH_PAD_R_PX: f32 = 2.0;
const SEARCH_GAP_PX: f32 = 2.0;
const SEARCH_ICON_PX: f32 = 14.0;
const SEARCH_CLEAR_PX: f32 = 26.0;

// Toast.
const TOAST_PAD_L_PX: f32 = 6.0;
const TOAST_PAD_R_PX: f32 = 14.0;
const TOAST_GAP_PX: f32 = 8.0;
const TOAST_BADGE_PX: f32 = 24.0;
const TOAST_BADGE_ALPHA: f32 = 0.16;
/// Drop shadow: `(dark, light)` alpha, blur and y offset.
const TOAST_SHADOW: (f32, f32) = (0.45, 0.14);
const TOAST_SHADOW_BLUR_PX: f32 = 14.0;
const TOAST_SHADOW_DY_PX: f32 = 4.0;

// Monograms and name colours.
/// Hash seed / multiplier of the per-name hue.
const NAME_HASH_SEED: u32 = 7;
const NAME_HASH_MUL: u32 = 31;
const DEGREES: u32 = 360;
/// OKLCH lightness / chroma of name colours.
const NAME_L: f32 = 0.7;
const NAME_C: f32 = 0.13;
/// Monogram radius and text size as fractions of its side; fill alphas.
const MONOGRAM_RADIUS_FRAC: f32 = 0.32;
const MONOGRAM_TEXT_FRAC: f32 = 0.38;
const MONOGRAM_FILL: (f32, f32) = (0.26, 0.14);

// Caption.
const CAPTION_PX: f32 = 10.5;
const CAPTION_ALPHA: f32 = 0.85;

/// One segment: label, icon, whether it's active, and what a click does.
pub struct Segment {
    pub label: SharedString,
    pub icon: Option<&'static str>,
    pub active: bool,
    pub on_click: Rc<dyn Fn(&mut Window, &mut App)>,
}

/// A pill-shaped segmented control (equal-width segments).
pub fn segmented(id: &'static str, segments: Vec<Segment>, palette: &MandaraPalette) -> AnyElement {
    let accent = readable(palette.primary, palette);
    let hover = wash(WASH_SOFT, palette);
    div()
        .h(px(CONTROL_H))
        .flex_none()
        .p(px(SEG_PAD_PX))
        .rounded(px(crate::style::radius(CONTROL_RADIUS_PX)))
        .bg(wash(SEG_TRACK, palette).hsla())
        .flex()
        .items_center()
        .gap(px(SEG_GAP_PX))
        .children(segments.into_iter().enumerate().map(move |(i, s)| {
            let on_click = s.on_click.clone();
            div()
                .id((id, i))
                .flex_1()
                .h_full()
                .px(px(SEG_PAD_X_PX))
                .rounded(px(crate::style::radius(SEG_RADIUS_PX)))
                .flex()
                .items_center()
                .justify_center()
                .gap(px(SEG_ICON_GAP_PX))
                .cursor_pointer()
                .text_size(px(SEG_TEXT_PX))
                .font_weight(if s.active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                .when(s.active, |el| el.bg(accent.with_alpha(super::panel_fx::alpha(SEG_ACTIVE, palette)).hsla()).text_color(accent.hsla()))
                .when(!s.active, |el| el.text_color(palette.on_surface_variant.hsla()).hover(move |st| st.bg(hover.hsla())))
                .when_some(s.icon, |el, name| el.child(icon(name, SMALL_ICON_PX, if s.active { accent } else { palette.on_surface_variant })))
                .child(s.label)
                .on_click(move |_, window, cx| on_click(window, cx))
        }))
        .into_any_element()
}

/// A search field with a leading glass icon and a clear button.
pub fn search_box(id: &'static str, field: &Entity<TextField>, query: &str, palette: &MandaraPalette) -> AnyElement {
    let f = field.clone();
    div()
        .flex_1()
        .min_w(px(0.0))
        .h(px(CONTROL_H))
        .pl(px(SEARCH_PAD_L_PX))
        .pr(px(SEARCH_PAD_R_PX))
        .rounded(px(crate::style::radius(CONTROL_RADIUS_PX)))
        .bg(palette.surface_container_high.hsla())
        .flex()
        .items_center()
        .gap(px(SEARCH_GAP_PX))
        .child(icon("search", SEARCH_ICON_PX, palette.on_surface_variant))
        .child(div().flex_1().min_w(px(0.0)).child(field.clone()))
        .when(!query.is_empty(), |el| {
            el.child(icon_button(id, "close", SEARCH_CLEAR_PX, palette.on_surface_variant, None, move |window, cx| {
                f.update(cx, |f, _| f.set(""));
                window.refresh();
            }))
        })
        .into_any_element()
}

/// A transient message (`ok` tints it green, otherwise coral).
#[derive(Clone, Default)]
pub struct Flash(Rc<RefCell<Option<(bool, SharedString, Instant)>>>);

impl Flash {
    pub fn show(&self, ok: bool, msg: impl Into<SharedString>) {
        *self.0.borrow_mut() = Some((ok, msg.into(), Instant::now()));
    }

    /// The live message (a cheap shared clone), scheduling the redraw that
    /// hides it.
    pub fn current(&self, window: &mut Window, cx: &mut App) -> Option<(bool, SharedString)> {
        let toast = crate::state::config(cx).read(cx).snapshot().panels.toast().mul_f64(QUICK_TOAST_SHARE);
        let mut slot = self.0.borrow_mut();
        if slot.as_ref().is_some_and(|(_, _, at)| at.elapsed() >= toast) {
            *slot = None;
        }
        let (ok, msg, at) = slot.clone()?;
        redraw_in(toast.saturating_sub(at.elapsed()) + EXPIRY_MARGIN, window, cx);
        Some((ok, msg))
    }
}

/// A floating pill toast anchored to the bottom of a panel.
pub fn toast(ok: bool, msg: impl Into<SharedString>, pad: f32, palette: &MandaraPalette) -> AnyElement {
    let tint = readable(if ok { palette.success_emerald } else { palette.urgent_coral }, palette);
    div()
        .absolute()
        .left(px(pad))
        .right(px(pad))
        .bottom(px(pad))
        .flex()
        .justify_center()
        .child(
            div()
                .max_w_full()
                .h(px(TOAST_H))
                .pl(px(TOAST_PAD_L_PX))
                .pr(px(TOAST_PAD_R_PX))
                .rounded_full()
                .bg(palette.surface_container_high.hsla())
                .shadow(vec![glow(
                    Color::oklch(0.0, 0.0, 0.0).with_alpha(super::panel_fx::alpha(TOAST_SHADOW, palette)),
                    TOAST_SHADOW_BLUR_PX,
                    0.0,
                    TOAST_SHADOW_DY_PX,
                )])
                .flex()
                .items_center()
                .gap(px(TOAST_GAP_PX))
                .child(
                    div()
                        .size(px(TOAST_BADGE_PX))
                        .flex_none()
                        .rounded_full()
                        .bg(tint.with_alpha(TOAST_BADGE_ALPHA).hsla())
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(icon(if ok { "check" } else { "close" }, SMALL_ICON_PX, tint)),
                )
                .child(div().min_w(px(0.0)).truncate().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).child(msg.into())),
        )
        .into_any_element()
}

/// A stable, pleasant per-name colour (monograms, avatars).
pub fn name_color(name: &str, palette: &MandaraPalette) -> Color {
    // Hashes the UTF-8 of the lowercased name without allocating it.
    let mut buf = [0u8; 4];
    let mut h = NAME_HASH_SEED;
    for c in name.chars().flat_map(char::to_lowercase) {
        for b in c.encode_utf8(&mut buf).bytes() {
            h = h.wrapping_mul(NAME_HASH_MUL).wrapping_add(u32::from(b));
        }
    }
    let hue = h % DEGREES;
    readable(Color::oklch(NAME_L, NAME_C, hue as f32), palette)
}

/// A rounded-square monogram (initials on a tinted gradient).
pub fn monogram(initials: &str, tint: Color, side: f32) -> AnyElement {
    div()
        .size(px(side))
        .flex_none()
        .rounded(px(crate::style::radius(side * MONOGRAM_RADIUS_FRAC)))
        .bg(mandara_core::oklab_gradient(ANGLE_DIAGONAL_DEG, tint.with_alpha(MONOGRAM_FILL.0), super::panel_fx::partner(tint).with_alpha(MONOGRAM_FILL.1)))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(side * MONOGRAM_TEXT_FRAC))
        .font_weight(FontWeight::BOLD)
        .text_color(tint.hsla())
        .child(initials.to_string())
        .into_any_element()
}

/// A tiny caption under a value (`10.5 px`, muted).
pub fn caption(s: impl Into<gpui::SharedString>, palette: &MandaraPalette) -> gpui::Div {
    div().text_size(px(CAPTION_PX)).text_color(palette.on_surface_variant.with_alpha(CAPTION_ALPHA).hsla()).child(s.into())
}

/// Linear step of a value toward a target at `rate` per second — a cheap
/// critically damped follower for numbers that animate between samples.
pub fn approach(current: f32, target: f32, dt: f32, rate: f32) -> f32 {
    current + (target - current) * (1.0 - (-rate * dt).exp())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn approach_converges_without_overshoot() {
        let mut v = 0.0;
        for _ in 0..240 {
            v = approach(v, 1.0, 1.0 / 144.0, 8.0);
            assert!(v <= 1.0);
        }
        assert!((v - 1.0).abs() < 1e-3);
    }

    #[test]
    fn name_colors_are_stable() {
        let p = mandara_theme::engine::default_palette(true);
        assert_eq!(name_color("GitHub", &p), name_color("github", &p));
        assert_ne!(name_color("GitHub", &p), name_color("Proton", &p));
    }
}
