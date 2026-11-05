//! Shared surface and ornament treatment for Home cards and action tiles.

use gpui::{InteractiveElement as _, ParentElement as _, StatefulInteractiveElement as _, Styled as _, prelude::FluentBuilder as _};
use mandara_core::ToGpui as _;
use mandara_theme::{Color, MandaraPalette};

use super::{
    GLASS_FILL, Ornament, SHADOW_ALPHA, SHADOW_BLUR_PX, SHADOW_LIGHTNESS, SHADOW_OFFSET_PX, TileCx,
    ornament_layer,
};
use mandara_config::surfaces::MenuTileSurface;

type Tile = gpui::Stateful<gpui::Div>;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Hover {
    Decorate,
    Preserve,
}

/// Surface choices are independent of ornaments and apply even when patterns are off.
fn fills(palette: &MandaraPalette, surface: MenuTileSurface, active: bool) -> (Color, Color) {
    let alpha = if surface == MenuTileSurface::Glass && !active {
        GLASS_FILL
    } else {
        1.0
    };
    (
        palette.surface_container.with_alpha(alpha),
        palette.surface_container_high.with_alpha(alpha),
    )
}

/// Flat cards have no rim; the shadow setting applies to every surface style.
fn shadows(
    palette: &MandaraPalette,
    surface: MenuTileSurface,
    strength: f32,
) -> Vec<gpui::BoxShadow> {
    let mut shadows = if surface == MenuTileSurface::Flat {
        Vec::new()
    } else {
        mandara_core::glass_rim(palette.specular_rim_light, palette.specular_rim_shadow)
    };
    if strength > 0.0 {
        shadows.push(gpui::BoxShadow {
            color: Color::oklcha(SHADOW_LIGHTNESS, 0.0, 0.0, SHADOW_ALPHA * strength).hsla(),
            offset: gpui::point(gpui::px(0.0), gpui::px(SHADOW_OFFSET_PX * strength)),
            blur_radius: gpui::px(SHADOW_BLUR_PX * strength),
            spread_radius: gpui::px(0.0),
            inset: false,
        });
    }
    shadows
}

/// Profile content keeps its photograph, with the same configurable card frame.
pub(super) fn plain(tile: Tile, palette: &MandaraPalette, tc: &TileCx) -> Tile {
    let (rest, _) = fills(palette, tc.surface, false);
    crate::style::ClipRounded::clip_rounded(tile)
        .bg(rest.hsla())
        .shadow(shadows(palette, tc.surface, tc.shadow))
}

/// Share decoration, clipping and hover behavior without replacing card controls.
/// Active toggles keep an opaque semantic fill and their own readable text colors.
pub(super) fn decorate(
    tile: Tile,
    key: &'static str,
    art: Ornament,
    palette: &MandaraPalette,
    tc: &TileCx,
    active: bool,
) -> (Tile, Color) {
    decorated(tile, key, art, palette, tc, active, Hover::Decorate)
}

/// Quick toggles already own their hover fill and activation handlers.
pub(super) fn toggle(
    tile: Tile,
    key: &'static str,
    art: Ornament,
    palette: &MandaraPalette,
    tc: &TileCx,
    active: bool,
) -> Tile {
    decorated(tile, key, art, palette, tc, active, Hover::Preserve).0
}

/// Add a shared frame while leaving an existing control's hover style intact.
fn decorated(
    tile: Tile,
    key: &'static str,
    art: Ornament,
    palette: &MandaraPalette,
    tc: &TileCx,
    active: bool,
    hover: Hover,
) -> (Tile, Color) {
    let mut selected;
    let colors = if active {
        selected = palette.clone();
        selected.surface_container = palette.primary;
        selected.surface_container_high = palette
            .primary
            .mix(palette.on_primary, crate::kit::alpha::HOVER_FILLED);
        selected.on_surface = palette.on_primary;
        selected.on_surface_variant = palette
            .on_primary
            .with_alpha(crate::kit::alpha::SECONDARY_TEXT);
        &selected
    } else {
        palette
    };
    let (rest, high) = fills(colors, tc.surface, active);
    let tile = crate::style::ClipRounded::clip_rounded(tile)
        .relative()
        .shadow(shadows(colors, tc.surface, tc.shadow));
    if !tc.patterns {
        return (
            tile.bg(rest.hsla()).when(hover == Hover::Decorate, |el| el.hover(move |s| s.bg(high.hsla()))),
            colors.primary,
        );
    }
    let ([lip, body], glyph, wash_rest, wash_high) = {
        let mut glow = tc.glow.borrow_mut();
        let ink = glow.ink(key, art, colors, active);
        ornament_layer(art, &ink, colors, glow.value(key), tc.compact)
    };
    let (rest, high) = if tc.surface == MenuTileSurface::Raised && !active {
        (wash_rest, wash_high)
    } else {
        (rest.hsla().into(), high.hsla().into())
    };
    let glow = tc.glow.clone();
    let tile = tile
        .bg(rest)
        .when(hover == Hover::Decorate, |el| el.hover(move |s| s.bg(high)))
        .on_hover(move |on: &bool, window, _| {
            glow.borrow_mut().aim(key, *on);
            window.refresh();
        })
        .when(tc.surface != MenuTileSurface::Flat, |el| el.child(lip))
        .child(body);
    (tile, glyph)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A production toggle already has a hover style; decoration must preserve it.
    #[test]
    fn decorated_quick_toggles_keep_their_existing_hover() {
        use std::{cell::RefCell, rc::Rc};
        for dark in [false, true] {
            let palette = mandara_theme::engine::default_palette(dark);
            for surface in [MenuTileSurface::Flat, MenuTileSurface::Raised, MenuTileSurface::Glass] {
                for patterns in [false, true] {
                    for active in [false, true] {
                        let glow = Rc::new(RefCell::new(super::super::Glow::default()));
                        let tc = TileCx {
                            glow: &glow,
                            patterns,
                            calligraphy: false,
                            compact: true,
                            surface,
                            shadow: 0.0,
                        };
                        let control = crate::kit::toggle_tile("home-toggle", "wifi-4", "Wi-Fi", None, active, &palette, |_, _| {});
                        let mut tile = toggle(control, "home-toggle", Ornament::motif(super::super::Motif::Rings, 0.0), &palette, &tc, active);
                        assert_eq!(glow.borrow().inks.len(), usize::from(patterns));
                        assert_eq!(tile.style().box_shadow.as_ref().unwrap().len(), if surface == MenuTileSurface::Flat { 0 } else { 2 });
                        if active {
                            assert_eq!(tile.style().background, Some(palette.primary.hsla().into()));
                        }
                    }
                }
            }
        }
    }

    /// Turning patterns off must not bypass the selected surface or its shadow.
    #[test]
    fn surface_settings_remain_effective_without_patterns() {
        for dark in [false, true] {
            let palette = mandara_theme::engine::default_palette(dark);
            for surface in [
                MenuTileSurface::Flat,
                MenuTileSurface::Raised,
                MenuTileSurface::Glass,
            ] {
                let (rest, high) = fills(&palette, surface, false);
                let alpha = if surface == MenuTileSurface::Glass {
                    GLASS_FILL
                } else {
                    1.0
                };
                assert_eq!(rest, palette.surface_container.with_alpha(alpha));
                assert_eq!(high, palette.surface_container_high.with_alpha(alpha));
                let rim = shadows(&palette, surface, 0.0).len();
                assert_eq!(
                    rim,
                    if surface == MenuTileSurface::Flat {
                        0
                    } else {
                        2
                    }
                );
                assert_eq!(shadows(&palette, surface, 1.0).len(), rim + 1);
                assert_eq!(fills(&palette, surface, true).0.alpha(), 1.0);
            }
        }
    }

    /// Home controls add ornament data by default and honor the same off switch.
    #[test]
    fn home_cards_follow_the_existing_pattern_switch() {
        use gpui::InteractiveElement as _;
        use std::{cell::RefCell, rc::Rc};
        for dark in [false, true] {
            let palette = mandara_theme::engine::default_palette(dark);
            let panels = mandara_config::surfaces::PanelsConfig::default();
            assert!(panels.menu_patterns);
            for patterns in [panels.menu_patterns, false] {
                let glow = Rc::new(RefCell::new(super::super::Glow::default()));
                let tc = TileCx {
                    glow: &glow,
                    patterns,
                    calligraphy: false,
                    compact: false,
                    surface: panels.menu_tile_surface,
                    shadow: panels.menu_tile_shadow(),
                };
                let art = Ornament::motif(super::super::Motif::Shamsa, 0.0);
                let (mut card, _) = decorate(
                    gpui::div().id("home-test"),
                    "home-test",
                    art,
                    &palette,
                    &tc,
                    false,
                );
                assert_eq!(glow.borrow().inks.len(), usize::from(patterns));
                assert_eq!(card.style().box_shadow.as_ref().unwrap().len(), 0);
                assert!(
                    glow.borrow().springs.is_empty(),
                    "construction never starts an idle animation"
                );
            }
        }
    }
}
