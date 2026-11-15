//! Surface materials shared by the dock and the taskbar
//! (`dock.material`, `[plugins.settings.taskbar] material`): the classic
//! chromatic glass and layered liquid glass, either one with the shared
//! surface grain ([`super::grain`]) on top.
//!
//! Liquid glass is built from what GPUI draws natively, so it needs no blur
//! from the compositor and looks the same everywhere:
//!
//! 1. a translucent body tinted toward the accent, its OKLCH lightness
//!    falling from top to bottom (the lensing of a thick pane);
//! 2. a crisp specular highlight along the inner top edge and a fainter
//!    caustic along the inner bottom edge (inset SDF shadows);
//! 3. light gathered inside the edges (a soft inset glow);
//! 4. a lens highlight across the upper half and a caustic band at the
//!    bottom, both concentric with the surface's corners;
//! 5. tight outer shadows only: MangoWM blurs every pixel with alpha into
//!    a halo, so nothing reaches far past the edge.

use gpui::{AnyWindowHandle, App, BoxShadow, Corners, ParentElement, Styled, div, point, px};
use mandara_config::dock::{DockConfig, DockMaterial};
use mandara_config::grain::Grain;
use mandara_core::{ToGpui as _, oklab_gradient};
use mandara_theme::{Color, MandaraPalette};

use crate::plugins::panel_fx::{ANGLE_DOWN_DEG, elevation, rim, shadow_ink};

/// Gradient angle (degrees) for bottom → top fills.
const ANGLE_UP_DEG: f32 = 0.0;

/// Liquid glass body: alpha at the top and the bottom (times the surface
/// opacity), on dark and on light themes. Translucent enough to read as
/// glass over the wallpaper, dense enough to keep icons legible.
const LIQUID_BODY_DARK: (f32, f32) = (0.64, 0.80);
const LIQUID_BODY_LIGHT: (f32, f32) = (0.74, 0.84);
/// OKLCH lightness shift of the body's top and bottom.
const LIQUID_BODY_L: (f32, f32) = (0.06, -0.02);
/// Specular top edge and bottom caustic: alpha at full gloss `(dark, light)`.
const LIQUID_SPECULAR_TOP: (f32, f32) = (0.62, 1.0);
const LIQUID_SPECULAR_BOTTOM: (f32, f32) = (0.24, 0.6);
/// Their offset (the highlight's width) and blur, logical px.
const LIQUID_SPECULAR_PX: f32 = 1.0;
const LIQUID_SPECULAR_BLUR_PX: f32 = 0.6;
const LIQUID_CAUSTIC_BLUR_PX: f32 = 1.2;
/// Light gathered inside the edges: alpha at full refraction `(dark, light)`
/// and its reach as a share of the surface's thickness.
const LIQUID_EDGE_GLOW: (f32, f32) = (0.16, 0.45);
const LIQUID_EDGE_GLOW_FRAC: f32 = 0.2;
/// The lens highlight over the upper half and the caustic band at the
/// bottom: their alpha at full refraction `(dark, light)`, their share of
/// the inner height and their inset from the edge (share of the thickness,
/// at least the minimum, logical px).
const LIQUID_LENS: (f32, f32) = (0.18, 0.5);
const LIQUID_LENS_FRAC: f32 = 0.5;
const LIQUID_CAUSTIC: (f32, f32) = (0.09, 0.26);
const LIQUID_CAUSTIC_FRAC: f32 = 0.34;
const LIQUID_INSET_FRAC: f32 = 0.05;
const LIQUID_INSET_MIN_PX: f32 = 1.5;
/// Outer shadows: a contact shadow and a short ambient one `(alpha dark,
/// alpha light, dy, blur, spread)`, kept tight (blurred into halos otherwise).
const LIQUID_CONTACT: (f32, f32, f32, f32, f32) = (0.30, 0.10, 1.0, 3.0, 0.0);
const LIQUID_AMBIENT: (f32, f32, f32, f32, f32) = (0.22, 0.08, 4.0, 12.0, -4.0);

/// A surface's material, resolved from its settings.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Material {
    pub kind: DockMaterial,
    /// Body opacity, 0–1.
    pub opacity: f32,
    /// Accent mixed into the body, 0–0.5.
    pub tint: f32,
    /// Liquid glass: specular strength and inner light, 0–1.
    pub gloss: f32,
    pub refraction: f32,
    /// The specular rim (classic) or edges (liquid).
    pub rim: bool,
    pub shadow: bool,
    /// Grain over the surface (off by default).
    pub grain: Grain,
}

impl Material {
    /// The dock's material (`cfg` already sanitized).
    pub fn dock(cfg: &DockConfig) -> Self {
        Self {
            kind: cfg.material,
            opacity: cfg.opacity,
            tint: cfg.tint,
            gloss: cfg.gloss,
            refraction: cfg.refraction,
            rim: cfg.rim,
            shadow: cfg.shadow,
            grain: cfg.grain,
        }
    }

    /// The body colour before its alpha: the frame colour tinted toward the
    /// accent.
    pub fn base(&self, palette: &MandaraPalette) -> Color {
        crate::shell::frame_color(palette).mix(palette.primary, self.tint)
    }
}

/// `(dark, light)` picked for `palette`.
fn pick(pair: (f32, f32), palette: &MandaraPalette) -> f32 {
    if palette.is_dark { pair.0 } else { pair.1 }
}

/// White at `alpha` (highlights are light, whatever the theme).
fn light(alpha: f32) -> Color {
    Color::oklch(1.0, 0.0, 0.0).with_alpha(alpha.clamp(0.0, 1.0))
}

/// The inset of liquid glass's inner layers on a surface `h` px thick.
fn liquid_inset(h: f32) -> f32 {
    (h * LIQUID_INSET_FRAC).max(LIQUID_INSET_MIN_PX)
}

/// The outer and inset shadows of `m` on a surface `h` px thick.
pub fn shadows(m: &Material, h: f32, palette: &MandaraPalette) -> Vec<BoxShadow> {
    // Inked (`[style.ink]`): the ink's hard shadow replaces the soft ones.
    if crate::style::ink_shadow() > 0.0 {
        return elevation(palette, 1.0);
    }
    let mut out = Vec::new();
    match m.kind {
        DockMaterial::Classic => {
            if m.shadow {
                out.extend(elevation(palette, 1.0));
            }
            if m.rim {
                out.extend(rim(palette));
            }
        }
        DockMaterial::Liquid => {
            if m.shadow && crate::style::shadows() {
                let ink = shadow_ink(palette);
                for (dark, light_a, dy, blur, spread) in [LIQUID_CONTACT, LIQUID_AMBIENT] {
                    out.push(BoxShadow { color: ink.with_alpha(pick((dark, light_a), palette)).hsla(), offset: point(px(0.0), px(dy)), blur_radius: px(blur), spread_radius: px(spread), inset: false });
                }
            }
            if m.rim && crate::style::ink_outline() <= 0.0 {
                out.push(BoxShadow { color: light(m.gloss * pick(LIQUID_SPECULAR_TOP, palette)).hsla(), offset: point(px(0.0), px(LIQUID_SPECULAR_PX)), blur_radius: px(LIQUID_SPECULAR_BLUR_PX), spread_radius: px(0.0), inset: true });
                out.push(BoxShadow { color: light(m.gloss * pick(LIQUID_SPECULAR_BOTTOM, palette)).hsla(), offset: point(px(0.0), px(-LIQUID_SPECULAR_PX)), blur_radius: px(LIQUID_CAUSTIC_BLUR_PX), spread_radius: px(0.0), inset: true });
            }
            if m.refraction > 0.0 {
                out.push(BoxShadow { color: light(m.refraction * pick(LIQUID_EDGE_GLOW, palette)).hsla(), offset: point(px(0.0), px(0.0)), blur_radius: px(h * LIQUID_EDGE_GLOW_FRAC), spread_radius: px(0.0), inset: true });
            }
        }
    }
    out
}

/// Dresses `el`, a surface `h` px thick with corner radius `r` (already
/// scaled), in material `m`: its fill, shadows and inner layers (the liquid
/// lens and caustic, the grain, each inside its rounded shape). Call it
/// before adding the surface's own children, which then paint over the
/// layers. `scale_factor` and `redraw` are the window's (the grain tile is
/// drawn for it, and the window redraws when it is ready).
pub fn dress<E: Styled + ParentElement>(el: E, m: &Material, h: f32, r: f32, palette: &MandaraPalette, scale_factor: f32, redraw: Option<AnyWindowHandle>, cx: &mut App) -> E {
    let base = m.base(palette);
    let el = el.rounded(px(r)).shadow(shadows(m, h, palette));
    // Inked (`[style.ink]`): an ink outline.
    let ink = crate::style::ink_outline();
    let el = if ink > 0.0 { el.border(px(ink)).border_color(crate::plugins::panel_fx::ink(palette).hsla()) } else { el };
    let el = match m.kind {
        DockMaterial::Classic => el.bg(base.with_alpha(m.opacity).hsla()),
        DockMaterial::Liquid => {
            let (top, bottom) = if palette.is_dark { LIQUID_BODY_DARK } else { LIQUID_BODY_LIGHT };
            let el = el.bg(oklab_gradient(ANGLE_DOWN_DEG, base.lighten(LIQUID_BODY_L.0).with_alpha(m.opacity * top), base.lighten(LIQUID_BODY_L.1).with_alpha(m.opacity * bottom)));
            liquid_layers(el, m, h, r, palette)
        }
    };
    match super::grain::overlay_with(&m.grain, Corners::all(px(r)), palette, scale_factor, redraw, cx) {
        Some(layer) => el.child(layer),
        None => el,
    }
}

/// Liquid glass's lens highlight and bottom caustic band, concentric with
/// the surface's corners (so no clip is needed).
fn liquid_layers<E: ParentElement>(el: E, m: &Material, h: f32, r: f32, palette: &MandaraPalette) -> E {
    if m.refraction <= 0.0 {
        return el;
    }
    let inset = liquid_inset(h);
    let inner = (h - inset * 2.0).max(0.0);
    let radius = px(crate::style::concentric(r, inset));
    let lens = div()
        .absolute()
        .top(px(inset))
        .left(px(inset))
        .right(px(inset))
        .h(px(inner * LIQUID_LENS_FRAC))
        .rounded(radius)
        .bg(oklab_gradient(ANGLE_DOWN_DEG, light(m.refraction * pick(LIQUID_LENS, palette)), light(0.0)));
    let caustic = div()
        .absolute()
        .bottom(px(inset))
        .left(px(inset))
        .right(px(inset))
        .h(px(inner * LIQUID_CAUSTIC_FRAC))
        .rounded(radius)
        .bg(oklab_gradient(ANGLE_UP_DEG, light(m.refraction * pick(LIQUID_CAUSTIC, palette)), light(0.0)));
    el.child(lens).child(caustic)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn liquid() -> Material {
        Material { kind: DockMaterial::Liquid, ..Material::dock(&DockConfig::default()) }
    }

    /// The classic material is the dock as it always was: its frame
    /// colour, rim and elevation, untouched by the liquid settings.
    #[test]
    fn classic_is_the_old_glass() {
        let palette = mandara_theme::engine::default_palette(true);
        let classic = Material::dock(&DockConfig::default());
        assert_eq!(classic.kind, DockMaterial::Classic);
        let mut expected = elevation(&palette, 1.0);
        expected.extend(rim(&palette));
        assert_eq!(shadows(&classic, 64.0, &palette), expected);
        let glossy = Material { gloss: 1.0, refraction: 1.0, ..classic };
        assert_eq!(shadows(&glossy, 64.0, &palette), expected, "gloss and refraction are liquid-only");
    }

    /// Liquid glass draws its edges inside the surface and keeps its outer
    /// shadows tight (MangoWM would blur anything wider into a halo).
    #[test]
    fn liquid_glass_stays_tight_and_follows_its_settings() {
        for dark in [true, false] {
            let palette = mandara_theme::engine::default_palette(dark);
            let all = shadows(&liquid(), 64.0, &palette);
            let outer: Vec<_> = all.iter().filter(|s| !s.inset).collect();
            assert!(!outer.is_empty());
            for s in &outer {
                assert!(f32::from(s.blur_radius) + f32::from(s.offset.y) <= 16.0, "{s:?}");
            }
            assert!(all.iter().filter(|s| s.inset).count() >= 3, "specular edges and inner light");
            let matte = Material { rim: false, refraction: 0.0, shadow: false, ..liquid() };
            assert!(shadows(&matte, 64.0, &palette).is_empty());
            // Gloss brightens the top edge.
            let top = |g: f32| shadows(&Material { gloss: g, ..liquid() }, 64.0, &palette).into_iter().find(|s| s.inset && f32::from(s.offset.y) > 0.0).map(|s| s.color.a).unwrap_or(0.0);
            assert!(top(1.0) > top(0.3) && top(0.3) > 0.0);
        }
    }

    #[test]
    fn inner_layers_fit_inside_the_corners() {
        for h in [20.0, 32.0, 64.0, 152.0] {
            let inset = liquid_inset(h);
            assert!(inset >= LIQUID_INSET_MIN_PX && inset < h / 4.0, "{h}: {inset}");
            const { assert!(LIQUID_LENS_FRAC + LIQUID_CAUSTIC_FRAC < 1.0, "the lens and the caustic never meet") };
        }
    }
}
