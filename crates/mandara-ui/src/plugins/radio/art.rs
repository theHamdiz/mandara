//! Station art without downloading anything: a two-stop OKLCH gradient
//! whose hue comes from the station's shelf and name (the same station
//! always looks the same), a soft light at the top and the station's
//! initials. Only the initials are cached (by station id, bounded); the
//! colours are a little arithmetic per draw.

use std::cell::RefCell;
use std::collections::HashMap;

use gpui::{AnyElement, Div, FontWeight, IntoElement, ParentElement, SharedString, Styled, div, px};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_services::radio::Category;
use mandara_theme::{Color, MandaraPalette};

use super::ArtStyle;
use crate::plugins::panel_fx::ANGLE_DIAGONAL_DEG;

/// How far a station's hue may sit from its shelf's (degrees, each way).
const HUE_SPREAD_DEG: f32 = 38.0;
/// The two gradient stops `(lightness, chroma)` per style, and how far the
/// second stop's hue turns.
const VIVID: [(f32, f32); 2] = [(0.66, 0.17), (0.42, 0.13)];
const SOFT: [(f32, f32); 2] = [(0.76, 0.07), (0.56, 0.06)];
const STOP_TURN_DEG: f32 = 32.0;
/// The theme style: the accent's lightness shift for the second stop.
const THEME_SHADE: f32 = -0.16;
/// The highlight: its diameter as a share of the tile, offset and alpha.
const LIGHT_SHARE: f32 = 0.9;
const LIGHT_OFFSET: f32 = -0.32;
const LIGHT_ALPHA: f32 = 0.2;
/// Initials size as a share of the tile, and their colour.
const INITIALS_SHARE: f32 = 0.36;
const INITIALS_ALPHA: f32 = 0.94;
/// The placeholder's glyph as a share of the tile.
const GLYPH_SHARE: f32 = 0.5;

fn white(alpha: f32) -> Color {
    Color::oklcha(1.0, 0.0, 0.0, alpha)
}
/// Initials cached at most (then the cache starts over).
const MAX_CACHED: usize = 512;
/// Words skipped for initials: network names in front of the station's.
const SKIP_WORDS: [&str; 4] = ["somafm", "radio", "the", "fm"];

/// A stable hash of a station name (FNV-1a).
fn hash(name: &str) -> u32 {
    name.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

/// The two gradient colours of a station.
pub(crate) fn colors(name: &str, category: Category, style: ArtStyle, palette: &MandaraPalette) -> (Color, Color) {
    let spread = (hash(name) % 1000) as f32 / 1000.0 * 2.0 - 1.0;
    let hue = (category.hue() + spread * HUE_SPREAD_DEG).rem_euclid(360.0);
    match style {
        ArtStyle::Vivid => (Color::oklch(VIVID[0].0, VIVID[0].1, hue), Color::oklch(VIVID[1].0, VIVID[1].1, (hue + STOP_TURN_DEG).rem_euclid(360.0))),
        ArtStyle::Soft => (Color::oklch(SOFT[0].0, SOFT[0].1, hue), Color::oklch(SOFT[1].0, SOFT[1].1, (hue + STOP_TURN_DEG).rem_euclid(360.0))),
        ArtStyle::Theme => (palette.primary, palette.primary.lighten(THEME_SHADE).with_hue((palette.primary.hue() + STOP_TURN_DEG).rem_euclid(360.0))),
    }
}

/// Up to two initials: the first letters of the first two words that
/// aren't a network name (any script; digits count).
pub(crate) fn initials_of(name: &str) -> String {
    let mut words = name.split(|c: char| c.is_whitespace() || c == '-' || c == '(' || c == ')').filter(|w| w.chars().next().is_some_and(char::is_alphanumeric));
    let mut picked: Vec<&str> = words.clone().filter(|w| !SKIP_WORDS.iter().any(|s| w.eq_ignore_ascii_case(s))).take(2).collect();
    if picked.is_empty() {
        picked = words.by_ref().take(2).collect();
    }
    picked.iter().filter_map(|w| w.chars().find(|c| c.is_alphanumeric())).flat_map(char::to_uppercase).collect()
}

thread_local! {
    static INITIALS_CACHE: RefCell<HashMap<String, SharedString>> = RefCell::new(HashMap::new());
}

/// A station's initials, made once per station.
pub(crate) fn initials(id: &str, name: &str) -> SharedString {
    INITIALS_CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(s) = c.get(id) {
            return s.clone();
        }
        if c.len() >= MAX_CACHED {
            *c = HashMap::new();
        }
        let s = SharedString::from(initials_of(name));
        c.insert(id.to_string(), s.clone());
        s
    })
}

/// Frees the cache (the plugin was switched off).
pub(crate) fn clear() {
    INITIALS_CACHE.with(|c| *c.borrow_mut() = HashMap::new());
}

/// The station's art tile, `side` px square with `radius` corners.
pub(crate) fn tile(id: &str, name: &str, category: Category, style: ArtStyle, side: f32, radius: f32, palette: &MandaraPalette) -> Div {
    let (a, b) = colors(name, category, style, palette);
    let light = side * LIGHT_SHARE;
    div()
        .size(px(side))
        .flex_none()
        .relative()
        .overflow_hidden()
        .rounded(px(radius))
        .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, a, b))
        .child(div().absolute().left(px(side * LIGHT_OFFSET)).top(px(side * LIGHT_OFFSET)).size(px(light)).rounded_full().bg(white(LIGHT_ALPHA).hsla()))
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(side * INITIALS_SHARE))
                .font_weight(FontWeight::BOLD)
                .text_color(white(INITIALS_ALPHA).hsla())
                .child(initials(id, name)),
        )
}

/// The radio glyph on the theme's accent (no station yet).
pub(crate) fn placeholder(side: f32, radius: f32, palette: &MandaraPalette) -> AnyElement {
    let accent = palette.primary;
    div()
        .size(px(side))
        .flex_none()
        .rounded(px(radius))
        .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, accent.with_alpha(LIGHT_ALPHA), accent.lighten(THEME_SHADE).with_alpha(LIGHT_ALPHA)))
        .flex()
        .items_center()
        .justify_center()
        .child(crate::icons::icon("radio", side * GLYPH_SHARE, accent))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_skip_network_names_and_handle_any_script() {
        assert_eq!(initials_of("SomaFM Groove Salad"), "GS");
        assert_eq!(initials_of("Radio Paradise Mellow Mix"), "PM");
        assert_eq!(initials_of("FIP Jazz"), "FJ");
        assert_eq!(initials_of("1LIVE"), "1");
        assert_eq!(initials_of("RADIO BOB!"), "B");
        assert_eq!(initials_of("Radio"), "R", "only network words: use them");
        assert_eq!(initials_of("إذاعة القرآن الكريم"), "إا");
        assert_eq!(initials_of("  "), "");
        assert_eq!(initials_of("Qur'an Radio Cairo"), "QC");
    }

    #[test]
    fn colours_are_stable_and_follow_the_shelf() {
        let p = mandara_theme::engine::default_palette(true);
        let a = colors("FIP Jazz", Category::Jazz, ArtStyle::Vivid, &p);
        assert_eq!(a, colors("FIP Jazz", Category::Jazz, ArtStyle::Vivid, &p));
        assert_ne!(a, colors("TSF Jazz", Category::Jazz, ArtStyle::Vivid, &p), "names spread within a shelf");
        let hue = a.0.hue();
        let d = (hue - Category::Jazz.hue()).abs().min(360.0 - (hue - Category::Jazz.hue()).abs());
        assert!(d <= HUE_SPREAD_DEG + 1.0, "{hue}");
        let soft = colors("FIP Jazz", Category::Jazz, ArtStyle::Soft, &p);
        assert!(soft.0.chroma() < a.0.chroma());
        assert_eq!(colors("x", Category::News, ArtStyle::Theme, &p).0, p.primary);
        assert_eq!(initials("test-id", "Corniche FM").as_ref(), "C");
        assert_eq!(initials("test-id", "ignored once cached").as_ref(), "C");
        clear();
        assert_eq!(initials("test-id", "Montaza Twilight").as_ref(), "MT");
    }
}
