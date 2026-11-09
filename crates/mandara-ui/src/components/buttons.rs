//! Button designs (`style.button_design`): the shape, edge and elevation
//! of the shell's text buttons ([`crate::plugins::panel_fx::pill_button`]),
//! Studio's buttons and the round icon buttons ([`crate::kit::icon_button`]).
//! A button keeps its role (filled, tonal, outlined or text) and colours in
//! every design; the design only changes how it is shaped and how it moves.
//!
//! - **Classic**: drawn by the button's own code (pills and circles).
//! - **Square**: Material's square shape, corners of 12 px (scaled).
//! - **Expressive**: round at rest; while held the corners spring to 8 px
//!   and back on release (Material 3 Expressive's fast spatial spring).
//! - **Cut**: the corners cut at 45 degrees ([`super::cut`]).
//! - **Elevated**: a soft shadow that lifts on hover and settles while held
//!   (no shadow while `style.shadows` is off).
//!
//! [`dress`] must run before the button's own children are added: the
//! designs that animate or cut their shape draw it as a first child behind
//! the label.

use gpui::{Bounds, Div, ElementId, InteractiveElement as _, ParentElement as _, Pixels, SharedString, Stateful, StatefulInteractiveElement as _, Styled, canvas, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui as _;
use mandara_theme::{Color, MandaraPalette};

use super::ButtonDesign;
use super::motion::{self, Feel};

/// Material's corner sizes, px before the corner scale: the square
/// button's (`CornerMedium`) and a held expressive button's
/// (`CornerSmall`).
pub const SQUARE_RADIUS_PX: f32 = 12.0;
pub const PRESSED_RADIUS_PX: f32 = 8.0;
/// Icon buttons are smaller: their square and pressed corners as a share
/// of their side.
pub const ICON_SQUARE_SHARE: f32 = 0.3;
/// A cut corner as a share of the button's height, and its largest size.
pub const CUT_SHARE: f32 = 0.28;
pub const CUT_MAX_PX: f32 = 10.0;
/// The elevated design's shadow strength at rest, hovered and held
/// (fractions of the panel elevation).
const ELEVATION: (f32, f32, f32) = (0.45, 0.8, 0.2);
/// How much darker (or lighter) a held cut button gets than its hover.
const CUT_PRESS_MIX: f32 = 0.08;
/// The group the designs' shape layer follows for hover.
const GROUP: &str = "mandara-button";
/// Key of the expressive morph's spring under the button.
const MORPH_KEY: &str = "button-morph";

/// A button's colours, from its role.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Ink {
    /// Its container (`None`: a text button, only its hover shows).
    pub bg: Option<Color>,
    /// The container under the pointer.
    pub hover: Color,
    /// An outlined button's edge.
    pub outline: Option<Color>,
}

/// Corner radius of a button `h` tall in `design`, `press` of the way to
/// held (`0` resting, `1` held, a little past while springing).
pub fn radius(design: ButtonDesign, h: f32, press: f32) -> f32 {
    let full = h / 2.0;
    match design {
        ButtonDesign::Classic | ButtonDesign::Elevated => full,
        ButtonDesign::Square => crate::style::radius(SQUARE_RADIUS_PX).min(full),
        ButtonDesign::Expressive => {
            let held = crate::style::radius(PRESSED_RADIUS_PX).min(full);
            (full + (held - full) * press).clamp(0.0, full)
        }
        ButtonDesign::Cut => 0.0,
    }
}

/// Corner radius of a round icon button `side` wide in `design`.
pub fn icon_radius(design: ButtonDesign, side: f32, press: f32) -> f32 {
    let full = side / 2.0;
    let square = (side * ICON_SQUARE_SHARE).min(full);
    match design {
        ButtonDesign::Classic | ButtonDesign::Elevated => full,
        ButtonDesign::Square => square,
        ButtonDesign::Expressive => (full + (square - full) * press).clamp(0.0, full),
        ButtonDesign::Cut => 0.0,
    }
}

/// The cut of a button `h` tall.
pub fn cut(h: f32) -> f32 {
    (h * CUT_SHARE).min(CUT_MAX_PX)
}

/// Shapes a button in the current button design: `classic` styles it as
/// it always looked (its rounding, fill and hover); any other design draws
/// its shape from `ink` instead ([`dress`]). `el` carries `id` already and
/// gets its children after this call (a drawn shape sits behind them).
/// This is how one-off buttons across the shell join the designs.
pub fn designed(el: Stateful<Div>, id: impl Into<ElementId>, h: f32, icon: bool, ink: Ink, palette: Option<&MandaraPalette>, classic: impl FnOnce(Stateful<Div>) -> Stateful<Div>) -> Stateful<Div> {
    match crate::style::button_design() {
        ButtonDesign::Classic => classic(el),
        design => dress(el, id.into(), h, icon, ink, design, palette),
    }
}

/// Shapes `el` (id `id`, `h` tall) in `design` with `ink`. `icon` draws a
/// round icon button's shapes. Classic leaves `el` to the caller. Without
/// a `palette` (icon buttons are drawn from their ink alone) the elevated
/// design casts no shadow.
pub fn dress(el: Stateful<Div>, id: ElementId, h: f32, icon: bool, ink: Ink, design: ButtonDesign, palette: Option<&MandaraPalette>) -> Stateful<Div> {
    let r = move |press: f32| if icon { icon_radius(design, h, press) } else { radius(design, h, press) };
    let outline = |el: Stateful<Div>| match ink.outline {
        Some(c) => el.border_1().border_color(c.hsla()),
        None => el,
    };
    match design {
        ButtonDesign::Classic => el,
        ButtonDesign::Square => {
            let hover = ink.hover;
            outline(el.rounded(px(r(0.0))).when_some(ink.bg, |el, bg| el.bg(bg.hsla())).hover(move |s| s.bg(hover.hsla())))
        }
        ButtonDesign::Elevated => {
            let hover = ink.hover;
            let el = outline(el.rounded(px(r(0.0))).when_some(ink.bg, |el, bg| el.bg(bg.hsla())));
            if let Some(palette) = palette.filter(|_| crate::style::shadows() && ink.bg.is_some()) {
                let elevation = |k: f32| crate::plugins::panel_fx::elevation_scaled(palette, 1.0, k);
                let (rest, lifted, held) = (elevation(ELEVATION.0), elevation(ELEVATION.1), elevation(ELEVATION.2));
                el.shadow(rest).hover(move |s| s.bg(hover.hsla()).shadow(lifted)).active(move |s| s.shadow(held))
            } else {
                el.hover(move |s| s.bg(hover.hsla()))
            }
        }
        ButtonDesign::Expressive => {
            let held = motion::held(&id);
            let (bg, hover, edge) = (ink.bg.map(|c| c.hsla()).unwrap_or_else(gpui::transparent_black), ink.hover.hsla(), ink.outline.map(|c| c.hsla()));
            let layer = div().absolute().top_0().left_0().size_full().group_hover(SharedString::new_static(GROUP), move |s| s.bg(hover));
            let layer = motion::spring(layer, MORPH_KEY, if held { 1.0 } else { 0.0 }, Feel::FastSpatial, move |el, t| {
                el.rounded(px(r(t))).bg(bg).when_some(edge, |el, c| el.border_1().border_color(c))
            });
            motion::track_press(el.relative().group(SharedString::new_static(GROUP)), id).child(layer)
        }
        ButtonDesign::Cut => {
            let held = motion::held(&id);
            // Held: the hover a step deeper (toward the text, or its own
            // ink when drawn without a palette).
            let pressed = match palette {
                Some(p) => ink.hover.mix(p.on_surface, CUT_PRESS_MIX),
                None => ink.hover.with_alpha((ink.hover.alpha() + CUT_PRESS_MIX).min(1.0)),
            };
            let (bg, hover, edge) = (ink.bg, ink.hover, ink.outline);
            let layer = canvas(|_, _, _| {}, move |bounds: Bounds<Pixels>, _, window, _| {
                let fill = if held { Some(pressed) } else if bounds.contains(&window.mouse_position()) { Some(hover) } else { bg };
                let ink = super::cut::Ink::solid(fill, edge.map(|c| (c, 1.0)));
                super::cut::paint(window, bounds, cut(f32::from(bounds.size.height)), ink);
            })
            .absolute()
            .top_0()
            .left_0()
            .size_full();
            // A hover style, so the layer repaints as the pointer enters and leaves.
            motion::track_press(el.relative().hover(|s| s), id).child(layer)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEIGHTS: [f32; 4] = [20.0, 28.0, 34.0, 44.0];

    #[test]
    fn every_shape_stays_inside_its_button() {
        for design in ButtonDesign::ALL {
            for h in HEIGHTS {
                for press in [0.0, 0.5, 1.0, 1.3, -0.2] {
                    let (r, ir) = (radius(design, h, press), icon_radius(design, h, press));
                    assert!((0.0..=h / 2.0).contains(&r), "{design:?} {h} {press}: {r}");
                    assert!((0.0..=h / 2.0).contains(&ir), "{design:?} icon {h} {press}: {ir}");
                }
                assert!(cut(h) > 0.0 && cut(h) <= super::super::cut::clamp_cut(cut(h), h, h) + 1e-4, "{h}: the cut fits");
            }
        }
    }

    #[test]
    fn only_the_expressive_design_morphs_and_it_squares_off_when_held() {
        for design in ButtonDesign::ALL {
            let (rest, held) = (radius(design, 34.0, 0.0), radius(design, 34.0, 1.0));
            if design == ButtonDesign::Expressive {
                assert_eq!(rest, 17.0, "round at rest");
                assert!(held < rest && held > 0.0, "square-ish while held");
                assert!(icon_radius(design, 32.0, 1.0) < icon_radius(design, 32.0, 0.0));
            } else {
                assert_eq!(rest, held, "{design:?} keeps its shape");
            }
        }
        assert_eq!(radius(ButtonDesign::Classic, 34.0, 0.0), 17.0, "classic buttons are pills");
    }
}
