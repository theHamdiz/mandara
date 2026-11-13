//! Toggle designs (`style.toggle_design`): switches
//! ([`crate::plugins::panel_fx::switch`], every switch in the shell and
//! Studio), check boxes and radio buttons (tray menus, Studio's lists).
//!
//! - **Classic**: drawn by each control's own code (the gradient switch).
//! - **Material**: Material 3's switch at the shell's switch size (an
//!   outlined track whose small thumb grows and carries a check when on),
//!   check boxes with small corners and a thick edge, ringed radios.
//! - **Expressive**: the thumb keeps its size and stretches as it crosses
//!   the track, check boxes round into circles as they fill, the radio dot
//!   springs in.
//! - **Round**: a near-full round thumb, circular check boxes, filled
//!   radio discs.
//!
//! Every switch keeps the classic box (42 × 24 px), so rows never shift
//! when the design changes. Flipping a switch or a check box springs
//! between its states (Material 3 Expressive's springs at the shell's
//! animation speed, instant with reduced motion); drawn for the first time
//! they show their state without animating. Right to left, a switch that
//! is on has its thumb on the left.

use gpui::{AnyElement, Div, IntoElement, ParentElement as _, Styled, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui as _;
use mandara_theme::{Color, MandaraPalette};

use super::ToggleDesign;
use super::motion::{self, Feel};
use crate::bidi::Bidi as _;

/// The switch's box (the classic switch's), px.
pub const SWITCH_W_PX: f32 = 42.0;
pub const SWITCH_H_PX: f32 = 24.0;
/// Material 3's thumb sizes scaled to the shell's switch (16, 24 and 28 dp
/// on a 32 dp track), px: off, on, and held.
const MATERIAL_THUMB_PX: (f32, f32) = (12.0, 18.0);
/// The expressive thumb's size, and how far it stretches mid-travel, px.
const EXPRESSIVE_THUMB_PX: f32 = 18.0;
const EXPRESSIVE_STRETCH_PX: f32 = 10.0;
/// The round design's thumb.
const ROUND_THUMB_PX: f32 = 20.0;
/// Room kept between any thumb and the track's end.
const THUMB_PAD_PX: f32 = 2.0;
/// The track's outline while off (Material 3's 2 dp at this size).
const TRACK_OUTLINE_PX: f32 = 1.5;
/// The thumb's check, as a share of the thumb.
const THUMB_ICON_SHARE: f32 = 0.7;
/// The round thumb's soft shadow: alpha and blur.
const ROUND_SHADOW: (f32, f32) = (0.22, 4.0);
/// Check boxes: Material's corner (2 of 18 dp) and edge (2 of 18 dp) as
/// shares of the side; the expressive box's resting corner share.
const CHECK_CORNER_SHARE: f32 = 2.0 / 18.0;
const CHECK_EDGE_SHARE: f32 = 2.0 / 18.0;
const EXPRESSIVE_CORNER_SHARE: f32 = 0.28;
/// The check glyph as a share of the box.
const CHECK_ICON_SHARE: f32 = 0.78;
/// Radios: the dot as a share of the ring (Material's 10 of 20 dp), the
/// round design's inner dot.
const RADIO_DOT_SHARE: f32 = 0.5;
const ROUND_RADIO_DOT_SHARE: f32 = 0.38;
/// The classic radio's dot (7 of the tray's 16 px) and its quiet ring.
const CLASSIC_DOT_SHARE: f32 = 7.0 / 16.0;
const CLASSIC_RING_ALPHA: f32 = 0.55;
/// Thinnest edge a small box or ring is drawn with.
const MIN_EDGE_PX: f32 = 1.5;
/// Keys of the toggles' springs under their parent.
const SWITCH_KEY: &str = "switch-thumb";
const CHECK_KEY: &str = "check-fill";
const RADIO_KEY: &str = "radio-dot";

/// A switch thumb in the switch's box, px from the reading start and top.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Thumb {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// The thumb of a switch in `design` at `t` of the way from off to on (a
/// little past either end while it springs).
pub fn thumb(design: ToggleDesign, t: f32) -> Thumb {
    let s = t.clamp(0.0, 1.0);
    let (w, h) = match design {
        ToggleDesign::Material => {
            let side = MATERIAL_THUMB_PX.0 + (MATERIAL_THUMB_PX.1 - MATERIAL_THUMB_PX.0) * s;
            (side, side)
        }
        ToggleDesign::Expressive => {
            // Longest halfway across, back to round at either end.
            let stretch = EXPRESSIVE_STRETCH_PX * (std::f32::consts::PI * s).sin();
            (EXPRESSIVE_THUMB_PX + stretch, EXPRESSIVE_THUMB_PX - stretch * 0.2)
        }
        ToggleDesign::Classic | ToggleDesign::Round => (ROUND_THUMB_PX, ROUND_THUMB_PX),
    };
    let (lo, hi) = (THUMB_PAD_PX + w / 2.0, SWITCH_W_PX - THUMB_PAD_PX - w / 2.0);
    let travel = (SWITCH_H_PX / 2.0, SWITCH_W_PX - SWITCH_H_PX / 2.0);
    let centre = (travel.0 + (travel.1 - travel.0) * t).clamp(lo, hi.max(lo));
    Thumb { x: centre - w / 2.0, y: (SWITCH_H_PX - h) / 2.0, w, h }
}

/// A switch in a non-classic `design`.
pub fn switch(on: bool, design: ToggleDesign, palette: &MandaraPalette) -> Div {
    let p = palette.clone();
    let track = div().w(px(SWITCH_W_PX)).h(px(SWITCH_H_PX)).relative().rounded_full();
    let feel = if design == ToggleDesign::Expressive { Feel::FastSpatial } else { Feel::DefaultSpatial };
    let drawn = motion::spring(track, SWITCH_KEY, if on { 1.0 } else { 0.0 }, feel,
        move |el, t| switch_at(el, t, design, &p));
    div().flex_none().child(drawn)
}

/// A still preview keeps no spring state and uses the same control geometry.
pub fn switch_still(on: bool, design: ToggleDesign, palette: &MandaraPalette) -> Div {
    if design == ToggleDesign::Classic {
        return crate::plugins::panel_fx::switch_as(design, on, palette);
    }
    let track = div().w(px(SWITCH_W_PX)).h(px(SWITCH_H_PX)).relative().rounded_full().flex_none();
    switch_at(track, if on { 1.0 } else { 0.0 }, design, palette)
}

fn switch_at(el: Div, t: f32, design: ToggleDesign, p: &MandaraPalette) -> Div {
    let s = t.clamp(0.0, 1.0);
    let (off_track, on_track) = match design {
        ToggleDesign::Round => (crate::plugins::panel_fx::wash(crate::plugins::panel_fx::WASH_STRONG, p), p.primary),
        _ => (p.surface_container_high, p.primary),
    };
    let (off_thumb, on_thumb) = match design {
        ToggleDesign::Round => (p.on_surface_variant, p.on_primary),
        _ => (p.outline, p.on_primary),
    };
    let th = thumb(design, t);
    let ring = (design != ToggleDesign::Round && s < 1.0).then(|| div().absolute().top_0().left_0().size_full().rounded_full().border(px(TRACK_OUTLINE_PX)).border_color(p.outline.with_alpha(1.0 - s).hsla()));
    let icon = (design != ToggleDesign::Round && s > 0.5).then(|| crate::icons::icon("check", th.h * THUMB_ICON_SHARE, p.primary.with_alpha((s - 0.5) * 2.0)));
    let knob = div()
        .absolute()
        .top(px(th.y))
        .inset_start(px(th.x))
        .w(px(th.w))
        .h(px(th.h))
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .bg(off_thumb.mix(on_thumb, s).hsla())
        .when(design == ToggleDesign::Round, |el| el.shadow(vec![crate::plugins::panel_fx::glow(p.surface_dim.with_alpha(ROUND_SHADOW.0), ROUND_SHADOW.1, 0.0, 1.0)]))
        .children(icon);
    el.bg(off_track.mix(on_track, s).hsla()).children(ring).child(knob)
}

/// Corner radius of a check box `side` wide in `design`, `t` of the way
/// from clear to checked.
pub fn check_radius(design: ToggleDesign, side: f32, t: f32) -> f32 {
    let full = side / 2.0;
    match design {
        ToggleDesign::Classic | ToggleDesign::Material => crate::style::radius(side * CHECK_CORNER_SHARE).min(full),
        ToggleDesign::Expressive => {
            let rest = side * EXPRESSIVE_CORNER_SHARE;
            (rest + (full - rest) * t.clamp(0.0, 1.0)).min(full)
        }
        ToggleDesign::Round => full,
    }
}

/// The edge of a box or ring `side` wide.
pub fn edge(side: f32) -> f32 {
    (side * CHECK_EDGE_SHARE).max(MIN_EDGE_PX)
}

/// A check box `side` wide in a non-classic `design`: `Some(true)` checked,
/// `Some(false)` clear, `None` mixed.
pub fn checkbox(state: Option<bool>, side: f32, design: ToggleDesign, palette: &MandaraPalette) -> AnyElement {
    let p = palette.clone();
    let target = if state == Some(false) { 0.0 } else { 1.0 };
    let glyph = if state.is_none() { "minus" } else { "check" };
    let base = div().size(px(side)).flex_none().flex().items_center().justify_center();
    motion::spring(base, CHECK_KEY, target, Feel::FastSpatial, move |el, t| {
        let s = t.clamp(0.0, 1.0);
        // The glyph pops: it follows the spring past full size, but never
        // out of the box.
        let glyph_px = (side * CHECK_ICON_SHARE * t.max(0.0)).min(side);
        el.rounded(px(check_radius(design, side, t)))
            .bg(p.primary.with_alpha(s).hsla())
            .when(s < 1.0, |el| el.border(px(edge(side))).border_color(p.on_surface_variant.with_alpha(1.0 - s).hsla()))
            .when(s > 0.0, |el| el.child(crate::icons::icon(glyph, glyph_px, p.on_primary.with_alpha(s))))
    })
    .into_any_element()
}

/// The classic radio button (the tray menus'): an accent ring around a dot
/// when on, a quiet hairline ring when off.
pub fn classic_radio(on: bool, side: f32, palette: &MandaraPalette) -> AnyElement {
    let accent = crate::plugins::panel_fx::readable(palette.primary, palette);
    let base = div().size(px(side)).flex_none().flex().items_center().justify_center().rounded_full();
    if on {
        base.border_2().border_color(accent.hsla()).child(div().size(px(side * CLASSIC_DOT_SHARE)).rounded_full().bg(accent.hsla())).into_any_element()
    } else {
        base.border_1().border_color(palette.on_surface_variant.with_alpha(CLASSIC_RING_ALPHA).hsla()).into_any_element()
    }
}

/// The dot of a radio button `side` wide in `design`, `t` of the way to
/// selected (it may spring past).
pub fn radio_dot(design: ToggleDesign, side: f32, t: f32) -> f32 {
    let share = if design == ToggleDesign::Round { ROUND_RADIO_DOT_SHARE } else { RADIO_DOT_SHARE };
    let room = (side - 2.0 * edge(side)).max(0.0);
    (side * share * t.max(0.0)).min(room)
}

/// A radio button `side` wide in a non-classic `design`.
pub fn radio(on: bool, side: f32, design: ToggleDesign, palette: &MandaraPalette) -> AnyElement {
    let p = palette.clone();
    let base = div().size(px(side)).flex_none().rounded_full().flex().items_center().justify_center();
    let feel = if design == ToggleDesign::Expressive { Feel::FastSpatial } else { Feel::FastEffects };
    motion::spring(base, RADIO_KEY, if on { 1.0 } else { 0.0 }, feel, move |el, t| {
        let s = t.clamp(0.0, 1.0);
        let dot = radio_dot(design, side, t);
        // The ring turns from the quiet ink to the accent; a round radio
        // fills with the accent and shows its dot in the ink on it.
        let ring: Color = p.on_surface_variant.mix(p.primary, s);
        let ink = if design == ToggleDesign::Round { p.on_primary } else { p.primary };
        el.when(design == ToggleDesign::Round, |el| el.bg(p.primary.with_alpha(s).hsla()))
            .border(px(edge(side)))
            .border_color(ring.hsla())
            .when(dot > 0.0, |el| el.child(div().size(px(dot)).rounded_full().bg(ink.hsla())))
    })
    .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRAVEL: [f32; 9] = [-0.2, 0.0, 0.1, 0.3, 0.5, 0.7, 0.9, 1.0, 1.25];
    const DESIGNS: [ToggleDesign; 3] = [ToggleDesign::Material, ToggleDesign::Expressive, ToggleDesign::Round];

    #[test]
    fn the_thumb_stays_inside_the_switch_and_ends_where_its_state_says() {
        for design in DESIGNS {
            for t in TRAVEL {
                let th = thumb(design, t);
                assert!(th.x >= THUMB_PAD_PX - 1e-4 && th.x + th.w <= SWITCH_W_PX - THUMB_PAD_PX + 1e-4, "{design:?} at {t}: {th:?}");
                assert!(th.y >= 0.0 && th.y + th.h <= SWITCH_H_PX, "{design:?} at {t}: {th:?}");
            }
            let (off, on) = (thumb(design, 0.0), thumb(design, 1.0));
            assert!(off.x + off.w / 2.0 < SWITCH_W_PX / 2.0 && on.x + on.w / 2.0 > SWITCH_W_PX / 2.0, "{design:?}: off at the start, on at the end");
        }
        // Material's thumb grows as it turns on; the expressive one is round
        // at rest and stretched halfway.
        assert!(thumb(ToggleDesign::Material, 1.0).w > thumb(ToggleDesign::Material, 0.0).w);
        let (rest, mid) = (thumb(ToggleDesign::Expressive, 0.0), thumb(ToggleDesign::Expressive, 0.5));
        assert_eq!(rest.w, rest.h);
        assert!(mid.w > rest.w && mid.h < rest.h);
    }

    #[test]
    fn preview_switches_build_without_a_window_or_animation_state() {
        let palette = mandara_theme::ThemeSource::default().resolve(false).unwrap().palette;
        for design in [ToggleDesign::Classic, ToggleDesign::Material, ToggleDesign::Expressive, ToggleDesign::Round] {
            for on in [false, true] {
                let _: Div = switch_still(on, design, &palette);
            }
        }
    }

    #[test]
    fn boxes_and_dots_fit_and_the_expressive_box_rounds_as_it_fills() {
        for design in DESIGNS {
            for side in [14.0, 16.0, 18.0, 22.0] {
                for t in TRAVEL {
                    assert!((0.0..=side / 2.0).contains(&check_radius(design, side, t)), "{design:?} {side} {t}");
                    let dot = radio_dot(design, side, t);
                    assert!(dot >= 0.0 && dot <= side - 2.0 * edge(side), "{design:?} {side} {t}: {dot}");
                }
                assert_eq!(radio_dot(design, side, 0.0), 0.0, "no dot when clear");
            }
        }
        assert!(check_radius(ToggleDesign::Expressive, 18.0, 1.0) > check_radius(ToggleDesign::Expressive, 18.0, 0.0));
        assert_eq!(check_radius(ToggleDesign::Round, 18.0, 0.0), 9.0, "round boxes are circles");
    }
}
