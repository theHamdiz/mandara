//! Font resolution with graceful fallbacks, plus shared font features.

use std::sync::Arc;

use gpui::{App, FontFeatures, Global, SharedString};
use mandara_config::TypographyConfig;

/// Resolved font families (configured family if installed, else a fallback).
#[derive(Clone, Debug)]
pub struct Fonts {
    pub ui: SharedString,
    pub display: SharedString,
    pub arabic: SharedString,
    pub mono: SharedString,
    /// An installed colour emoji family, for text made only of emoji (the
    /// fallback chain would pick a text font's flat symbol first).
    pub emoji: Option<SharedString>,
}

impl Global for Fonts {}

/// Colour emoji families, best first.
pub const EMOJI_FAMILIES: &[&str] = &["Noto Color Emoji", "Twemoji", "Twitter Color Emoji", "JoyPixels", "Apple Color Emoji", "Segoe UI Emoji", "EmojiOne Color"];

/// Whether `text` is only emoji (pictographs with their joiners,
/// presentation selectors, skin tones and spaces): drawn in
/// [`Fonts::emoji`]. Mixed text keeps the normal fallback.
pub fn emoji_only(text: &str) -> bool {
    let mut any = false;
    for c in text.chars() {
        match c as u32 {
            0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B00..=0x2BFF => any = true,
            // ZWJ, variation selectors, skin tones, keycap, tags, spaces.
            0x200D | 0xFE0E | 0xFE0F | 0x20E3 | 0xE0020..=0xE007F | 0x20 => {}
            _ => return false,
        }
    }
    any
}

fn pick(installed: &[String], wanted: &str, fallbacks: &[&str]) -> SharedString {
    std::iter::once(wanted)
        .chain(fallbacks.iter().copied())
        .find(|name| installed.iter().any(|f| f.eq_ignore_ascii_case(name)))
        .unwrap_or(wanted)
        .to_string()
        .into()
}

/// Arabic-capable families, best first, after the configured Arabic font.
/// Each must cover the whole Arabic block with real joining (GSUB
/// `init`/`medi`/`fina`); weights a family lacks are matched to its nearest
/// weight instead of escaping to an unrelated face (e.g. a decorative Kufi
/// that happens to ship a SemiBold).
pub const ARABIC_FALLBACKS: &[&str] = &[
    "Cairo",
    "Alexandria",
    "Noto Sans Arabic",
    "Noto Sans Arabic UI",
    "Noto Naskh Arabic",
    "Noto Kufi Arabic",
];

/// The fallback chain for glyphs a primary font lacks: the configured Arabic
/// font, then [`ARABIC_FALLBACKS`], keeping installed families only.
pub fn fallback_chain(arabic: &str, installed: &[String]) -> Vec<String> {
    let mut chain: Vec<String> = Vec::new();
    for name in std::iter::once(arabic).chain(ARABIC_FALLBACKS.iter().copied()) {
        let Some(family) = installed.iter().find(|f| f.eq_ignore_ascii_case(name)) else { continue };
        if !chain.contains(family) {
            chain.push(family.clone());
        }
    }
    chain
}

/// Monospace families tried after the configured one, best first. GPUI
/// resolves fonts by family *name*, so a generic `monospace` never matches:
/// without one of these the code would fall back to the UI's sans font.
pub const MONO_FALLBACKS: &[&str] = &[
    "JetBrains Mono",
    "JetBrainsMono Nerd Font",
    "Fira Code",
    "Cascadia Code",
    "Source Code Pro",
    "Hack",
    "Noto Sans Mono",
    "DejaVu Sans Mono",
    "Liberation Mono",
    "Ubuntu Mono",
    "Adwaita Mono",
];

/// The configured monospace family when installed, else the first
/// installed [`MONO_FALLBACKS`] family, else any installed family whose
/// faces are fixed-pitch (`fixed_pitch`, asked only then).
fn pick_mono(installed: &[String], wanted: &str, fixed_pitch: impl FnOnce() -> Option<String>) -> SharedString {
    let has = |name: &str| installed.iter().find(|f| f.eq_ignore_ascii_case(name));
    if let Some(found) = std::iter::once(wanted).chain(MONO_FALLBACKS.iter().copied()).find_map(has) {
        return found.clone().into();
    }
    fixed_pitch().unwrap_or_else(|| wanted.to_string()).into()
}

/// The first fixed-pitch family among the system's fonts (scans the font
/// database: only used when no known monospace family is installed).
fn system_fixed_pitch_family() -> Option<String> {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let mut families: Vec<String> = db.faces().filter(|f| f.monospaced).filter_map(|f| f.families.first().map(|(name, _)| name.clone())).collect();
    families.sort();
    families.into_iter().next()
}

impl Fonts {
    /// Resolves the configured families and installs the process-wide
    /// fallback chain, so Arabic in any font (Inter, JetBrains Mono, …) and
    /// at any weight renders in an Arabic face.
    pub fn resolve(cfg: &TypographyConfig, cx: &App) -> Self {
        let installed = cx.text_system().all_font_names();
        gpui_wgpu::set_default_font_fallbacks(fallback_chain(&cfg.arabic_font, &installed));
        Self {
            ui: pick(&installed, &cfg.ui_font, &["Inter", "Inter Display", "Cantarell", "Noto Sans", "DejaVu Sans"]),
            display: pick(
                &installed,
                &cfg.display_font,
                &["Alexandria", "Inter Display", "Inter", "Cairo", "Noto Sans"],
            ),
            arabic: pick(&installed, &cfg.arabic_font, &["Cairo", "Alexandria", "Noto Sans Arabic", "Noto Naskh Arabic"]),
            mono: pick_mono(&installed, &cfg.mono_font, system_fixed_pitch_family),
            emoji: EMOJI_FAMILIES.iter().find(|name| installed.iter().any(|f| f.eq_ignore_ascii_case(name))).map(|f| SharedString::from(*f)),
        }
    }
}

pub fn fonts(cx: &App) -> Fonts {
    cx.global::<Fonts>().clone()
}

/// OpenType `tnum`: tabular figures so clocks and counters never jitter.
pub fn tabular_nums() -> FontFeatures {
    FontFeatures(Arc::new(vec![("tnum".into(), 1)]))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{FontRun, FontWeight, PlatformTextSystem, px};
    use gpui_wgpu::CosmicTextSystem;

    /// Section headings and nav rows that rendered in Reem Kufi at 600.
    const ARABIC_SAMPLE: &str = "الساعة المظهر والإحساس سطح المكتب";
    const SAMPLE_PX: f32 = 16.0;
    /// Every weight the UI uses for text.
    const WEIGHTS: [FontWeight; 5] =
        [FontWeight::LIGHT, FontWeight::NORMAL, FontWeight::MEDIUM, FontWeight::SEMIBOLD, FontWeight::BOLD];
    /// The word space the renderer guarantees in Arabic fallback text
    /// (`FALLBACK_MIN_WORD_SPACE_EM` in the vendored text system), less a
    /// rounding margin.
    const MIN_ARABIC_SPACE_EM: f32 = 0.33;
    /// Arabic joining forms: without them letters render isolated.
    const JOINING: [&str; 3] = ["init", "medi", "fina"];

    fn system_with_chain() -> Option<(CosmicTextSystem, Vec<String>)> {
        let system = CosmicTextSystem::new("IBM Plex Sans");
        let installed = system.all_font_names();
        let chain = fallback_chain("Cairo", &installed);
        if chain.is_empty() {
            eprintln!("no Arabic-capable family installed; skipping");
            return None;
        }
        gpui_wgpu::set_default_font_fallbacks(chain.clone());
        Some((system, chain))
    }

    /// Lays out the sample in `family` at `weight`; returns the faces
    /// (family, weight, GSUB features) its Arabic glyphs draw with.
    fn arabic_faces(system: &CosmicTextSystem, family: &str, weight: FontWeight) -> Vec<(String, u16, Vec<String>)> {
        let font = gpui::Font { weight, ..gpui::font(family.to_string()) };
        let id = system.font_id(&font).expect("font resolves");
        let layout = system.layout_line(ARABIC_SAMPLE, px(SAMPLE_PX), &[FontRun { len: ARABIC_SAMPLE.len(), font_id: id }]);
        let mut faces = Vec::new();
        for run in &layout.runs {
            assert!(run.glyphs.iter().all(|g| g.id.0 != 0), "{family} {weight:?}: .notdef glyph");
            let arabic = run.glyphs.iter().any(|g| !ARABIC_SAMPLE[g.index..].starts_with(' '));
            if arabic {
                faces.push(system.face_summary(run.font_id).expect("face"));
            }
        }
        faces
    }

    #[test]
    fn mono_resolves_to_an_installed_monospace_family() {
        let installed: Vec<String> = ["Inter", "DejaVu Sans Mono", "Noto Sans Mono"].iter().map(|s| s.to_string()).collect();
        // The configured family wins when installed (any case).
        assert_eq!(pick_mono(&installed, "dejavu sans mono", || None).as_ref(), "DejaVu Sans Mono");
        // A missing JetBrains Mono falls to an installed monospace family,
        // never to a name GPUI can't load (which draws the sans UI font).
        assert_eq!(pick_mono(&installed, "JetBrains Mono", || panic!("not needed")).as_ref(), "Noto Sans Mono");
        let bare: Vec<String> = vec!["Inter".into()];
        assert_eq!(pick_mono(&bare, "JetBrains Mono", || Some("Terminus".into())).as_ref(), "Terminus");
        assert_eq!(pick_mono(&bare, "JetBrains Mono", || None).as_ref(), "JetBrains Mono");
    }

    #[test]
    fn arabic_in_a_latin_font_uses_the_arabic_chain_at_every_weight() {
        let Some((system, chain)) = system_with_chain() else { return };
        for family in ["Inter", "JetBrains Mono"] {
            if !system.all_font_names().iter().any(|f| f == family) {
                continue;
            }
            for weight in WEIGHTS {
                let faces = arabic_faces(&system, family, weight);
                assert!(!faces.is_empty(), "{family} {weight:?}: no Arabic runs");
                for (face, _, substitutions) in faces {
                    assert_eq!(face, chain[0], "{family} {weight:?}: Arabic escaped to {face}");
                    for feature in JOINING {
                        assert!(substitutions.iter().any(|s| s == feature), "{face} lacks GSUB {feature}");
                    }
                }
            }
        }
    }

    #[test]
    fn variable_faces_draw_at_the_requested_weight() {
        let Some((system, _)) = system_with_chain() else { return };
        let Some(variable) = ["Alexandria", "Cairo"].into_iter().find(|f| system.all_font_names().iter().any(|n| n == f))
        else {
            return;
        };
        for weight in WEIGHTS {
            for (face, drawn, _) in arabic_faces(&system, variable, weight) {
                // Static families snap to their nearest weight; variable ones are exact.
                if face == "Alexandria" {
                    assert_eq!(drawn, weight.0 as u16, "{face} drawn at {drawn} for {weight:?}");
                }
            }
        }
    }

    #[test]
    fn punctuation_inside_arabic_keeps_its_place() {
        let Some((system, _)) = system_with_chain() else { return };
        // RTL: the colon sits left of "مجدول" (its logical predecessor) and
        // right of "التبديل", never past either end of the line.
        let text = "مجدول: التبديل";
        let id = system.font_id(&gpui::font("Inter")).expect("font");
        let layout = system.layout_line(text, px(SAMPLE_PX), &[FontRun { len: text.len(), font_id: id }]);
        let x_of = |index: usize| {
            layout.runs.iter().flat_map(|r| &r.glyphs).find(|g| g.index == index).map(|g| f32::from(g.position.x)).expect("glyph")
        };
        let colon = x_of(text.find(':').expect("colon"));
        let first = x_of(0);
        let last = x_of(text.char_indices().last().expect("char").0);
        assert!(last < colon && colon < first, "colon at {colon} outside {last}..{first}");
    }

    #[test]
    fn numbers_before_arabic_are_laid_out() {
        let Some((system, _)) = system_with_chain() else { return };
        // "5 min" in Arabic: the digit run differs from the line's direction.
        let text = "5 د";
        let id = system.font_id(&gpui::font("Inter")).expect("font");
        let layout = system.layout_line(text, px(SAMPLE_PX), &[FontRun { len: text.len(), font_id: id }]);
        let indices: Vec<usize> = layout.runs.iter().flat_map(|r| &r.glyphs).map(|g| g.index).collect();
        assert!(indices.contains(&0), "the digit was dropped: {indices:?}");
        let digit = layout.runs.iter().flat_map(|r| &r.glyphs).find(|g| g.index == 0).map(|g| f32::from(g.position.x)).expect("digit");
        let letter = layout.runs.iter().flat_map(|r| &r.glyphs).find(|g| g.index == 2).map(|g| f32::from(g.position.x)).expect("letter");
        assert!(digit > letter, "RTL: the number reads first, at the right");
    }

    /// Glyph x positions by source byte index.
    fn positions(system: &CosmicTextSystem, text: &str) -> Vec<(usize, f32, gpui::FontId, u32)> {
        let id = system.font_id(&gpui::font("Inter")).expect("font");
        let layout = system.layout_line(text, px(SAMPLE_PX), &[FontRun { len: text.len(), font_id: id }]);
        layout.runs.iter().flat_map(|r| r.glyphs.iter().map(|g| (g.index, f32::from(g.position.x), r.font_id, g.id.0))).collect()
    }

    #[test]
    fn arabic_glued_to_latin_is_shaped_and_ordered() {
        let Some((system, chain)) = system_with_chain() else { return };
        // (text, the word before و, the word after it, right-to-left paragraph)
        let cases = [
            ("الشبكة وBluetooth", "الشبكة", "Bluetooth", true),
            ("MangoWM وHyprland", "MangoWM", "Hyprland", false),
            ("Alt+Tab وAlt+Shift+Tab", "Alt+Tab", "Alt+Shift+Tab", false),
        ];
        for (text, before, after, rtl) in cases {
            let glyphs = positions(&system, text);
            let waw_at = text.find('و').expect("waw");
            let &(_, waw_x, waw_font, waw_glyph) = glyphs.iter().find(|g| g.0 == waw_at).expect("waw glyph");
            // و is its own Arabic run, in the Arabic face, in its isolated form.
            let (face, _, _) = system.face_summary(waw_font).expect("face");
            assert_eq!(face, chain[0], "{text}: و drawn with {face}");
            assert_eq!(system.glyph_for_char(waw_font, 'و').map(|g| g.0), Some(waw_glyph), "{text}: و not in its isolated form");
            // Bidi order: the word before و is on its reading-start side.
            let span_x = |word: &str, from: usize| {
                let start = text[from..].find(word).expect("word") + from;
                let xs: Vec<f32> = glyphs.iter().filter(|g| (start..start + word.len()).contains(&g.0)).map(|g| g.1).collect();
                (xs.iter().copied().fold(f32::MAX, f32::min), xs.iter().copied().fold(f32::MIN, f32::max))
            };
            let (before_min, before_max) = span_x(before, 0);
            let (after_min, after_max) = span_x(after, waw_at);
            if rtl {
                assert!(after_max < waw_x && waw_x < before_min, "{text}: expected {after} | و | {before} left to right");
            } else {
                assert!(before_max < waw_x && waw_x < after_min, "{text}: expected {before} | و | {after} left to right");
            }
        }
    }

    #[test]
    fn arabic_word_spaces_stay_open() {
        let Some((system, _)) = system_with_chain() else { return };
        // «البحث في الإعدادات»: Cairo's 0.22 em space made the first two words touch.
        let text = "البحث في الإعدادات";
        let glyphs = positions(&system, text);
        let x_of = |index: usize| glyphs.iter().find(|g| g.0 == index).map(|g| g.1).expect("glyph");
        let space = text.find(' ').expect("space");
        // Right to left: the space's advance runs from its x to the last
        // letter of the word before it (ث), drawn just right of it.
        let tha = text[..space].char_indices().last().map(|(i, _)| i).expect("letter");
        let advance = x_of(tha) - x_of(space);
        assert!(advance >= SAMPLE_PX * MIN_ARABIC_SPACE_EM, "word space only {advance}px at {SAMPLE_PX}px");
    }

    #[test]
    fn css_weight_matching() {
        let cairo_static = [200, 300, 400, 600, 700, 900];
        assert_eq!(gpui_wgpu::css_nearest_weight(500, &cairo_static), Some(400));
        assert_eq!(gpui_wgpu::css_nearest_weight(600, &cairo_static), Some(600));
        assert_eq!(gpui_wgpu::css_nearest_weight(800, &cairo_static), Some(900));
        assert_eq!(gpui_wgpu::css_nearest_weight(450, &[400, 500, 700]), Some(500));
        assert_eq!(gpui_wgpu::css_nearest_weight(100, &[400, 700]), Some(400));
        assert_eq!(gpui_wgpu::css_nearest_weight(600, &[400]), Some(400));
        assert_eq!(gpui_wgpu::css_nearest_weight(600, &[]), None);
    }

    #[test]
    fn chain_puts_the_configured_font_first_and_skips_missing() {
        let installed: Vec<String> = ["Inter", "Noto Sans Arabic", "Cairo", "Alexandria"].map(String::from).to_vec();
        assert_eq!(fallback_chain("alexandria", &installed), ["Alexandria", "Cairo", "Noto Sans Arabic"]);
        assert_eq!(fallback_chain("Missing", &installed), ["Cairo", "Alexandria", "Noto Sans Arabic"]);
    }

    #[test]
    fn only_emoji_text_takes_the_emoji_font() {
        assert!(emoji_only("🌕\u{FE0F}"));
        assert!(emoji_only("👩\u{200D}💻 🌑"));
        assert!(!emoji_only("Full moon 🌕"), "mixed text keeps the normal fallback");
        assert!(!emoji_only("42"), "digits are text, not keycaps");
        assert!(!emoji_only(""));
    }

    /// Emoji-only text asked for in the colour emoji family is shaped with
    /// it and drawn as colour glyphs (vendored patch 8), not a text font's
    /// flat symbol.
    #[test]
    fn emoji_text_draws_with_the_colour_emoji_font() {
        let system = CosmicTextSystem::new("IBM Plex Sans");
        let installed = system.all_font_names();
        let Some(family) = EMOJI_FAMILIES.iter().find(|name| installed.iter().any(|f| f.eq_ignore_ascii_case(name))) else {
            eprintln!("no colour emoji font installed; skipping");
            return;
        };
        let text = "🌕\u{FE0F}";
        let id = system.font_id(&gpui::font(family.to_string())).expect("the emoji family loads as a primary font");
        let layout = system.layout_line(text, px(SAMPLE_PX), &[FontRun { len: text.len(), font_id: id }]);
        let glyphs: Vec<_> = layout.runs.iter().flat_map(|r| r.glyphs.iter()).collect();
        assert!(!glyphs.is_empty() && glyphs.iter().all(|g| g.is_emoji && g.id.0 != 0), "drawn as colour emoji: {glyphs:?}");
    }
}
