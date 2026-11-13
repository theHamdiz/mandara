//! Studio refinements reuse the shell's control definitions and ranges.

use mandara_config::studio::ranges;
use mandara_i18n::tk;

use super::{Control, Section, Setting, s, span};

pub(crate) const GROUP: &str = tk("Studio design");

/// Only settings used inside Studio are mirrored; desktop-only controls stay out.
const MIRRORS: &[(&str, &str)] = &[
    ("/style/radius_scale", "/studio/style/radius_scale"),
    ("/style/shadows", "/studio/style/shadows"),
    ("/style/ink/outline", "/studio/style/ink/outline"),
    ("/style/ink/shadow", "/studio/style/ink/shadow"),
    ("/style/ink/texture", "/studio/style/ink/texture"),
    ("/style/slider_design", "/studio/style/slider_design"),
    ("/style/slider_handle", "/studio/style/slider_handle"),
    ("/style/button_design", "/studio/style/button_design"),
    ("/style/toggle_design", "/studio/style/toggle_design"),
    ("/style/input_design", "/studio/style/input_design"),
    ("/style/chip_design", "/studio/style/chip_design"),
    ("/style/progress_design", "/studio/style/progress_design"),
    ("/style/surface_corners", "/studio/style/surface_corners"),
    ("/style/card_style", "/studio/style/card_style"),
    ("/theme/mode", "/studio/theme/mode"),
    ("/theme/source/variant", "/studio/theme/variant"),
    (
        "/theme/adjustments/chroma_scale",
        "/studio/theme/adjustments/chroma_scale",
    ),
    (
        "/theme/adjustments/hue_shift",
        "/studio/theme/adjustments/hue_shift",
    ),
    (
        "/theme/adjustments/lightness_shift",
        "/studio/theme/adjustments/lightness_shift",
    ),
    (
        "/theme/adjustments/rim_intensity",
        "/studio/theme/adjustments/rim_intensity",
    ),
    ("/theme/transition_ms", "/studio/theme/transition_ms"),
    (
        "/typography/display_font",
        "/studio/typography/display_font",
    ),
    ("/typography/arabic_font", "/studio/typography/arabic_font"),
    ("/typography/mono_font", "/studio/typography/mono_font"),
    ("/typography/studio_font", "/studio/typography/studio_font"),
    ("/typography/weight", "/studio/typography/weight"),
    ("/motion/stiffness", "/studio/motion/stiffness"),
    ("/motion/damping", "/studio/motion/damping"),
    ("/motion/mass", "/studio/motion/mass"),
    ("/motion/reduce_motion", "/studio/motion/reduce_motion"),
];

pub(super) fn entries(shell: &[Setting]) -> Vec<Setting> {
    use Control::{Choice, Text, Toggle};
    let mut entries: Vec<_> = MIRRORS
        .iter()
        .map(|&(from, to)| {
            let mut setting = shell
                .iter()
                .find(|s| s.path == from)
                .expect("Studio mirror has a shell setting")
                .clone();
            setting.section = Section::Themes;
            setting.group = GROUP;
            setting.path = to;
            setting.help = tk("Changes Studio only. Desktop Looks keep their own settings.");
            if matches!(setting.control, Control::Font { .. }) {
                setting.control = Control::Font { follows: None };
            }
            setting
        })
        .collect();
    let entry = |path, label, control| {
        s(
            Section::Themes,
            GROUP,
            path,
            label,
            tk("Changes Studio only. Desktop Looks keep their own settings."),
            control,
        )
    };
    entries.extend([
        entry(
            "/studio/theme/color",
            tk("Accent color"),
            Text {
                placeholder: "#57aabb",
            },
        ),
        entry(
            "/studio/frame/design",
            tk("Window design"),
            Choice(
                const {
                    &[
                        ("classic", tk("Classic")),
                        ("glass", tk("Glass")),
                        ("header", tk("Header bar")),
                        ("chassis", tk("Chassis")),
                        ("console", tk("Control desk")),
                        ("comic", tk("Comic")),
                        ("editorial", tk("Paper")),
                        ("terminal", tk("Terminal")),
                    ]
                },
            ),
        ),
        entry("/studio/frame/header_accent", tk("Tint the header"), Toggle),
        entry(
            "/studio/frame/opacity",
            tk("Window opacity"),
            span(ranges::OPACITY, ""),
        ),
        entry(
            "/studio/frame/controls_at_start",
            tk("Window buttons at start"),
            Toggle,
        ),
        entry(
            "/studio/frame/inset",
            tk("Window inset"),
            span(ranges::INSET, " px"),
        ),
        entry(
            "/studio/frame/radius",
            tk("Window corners"),
            span(ranges::RADIUS, " px"),
        ),
        entry(
            "/studio/frame/border",
            tk("Window border"),
            span(ranges::BORDER, " px"),
        ),
        entry(
            "/studio/frame/title_height",
            tk("Title bar height"),
            span(ranges::TITLE_HEIGHT, " px"),
        ),
        entry(
            "/studio/frame/sidebar_width",
            tk("Sidebar width"),
            span(ranges::SIDEBAR_WIDTH, " px"),
        ),
        entry(
            "/studio/frame/sidebar_inset",
            tk("Sidebar inset"),
            span(ranges::INSET, " px"),
        ),
        entry(
            "/studio/frame/nav_height",
            tk("Navigation row height"),
            span(ranges::NAV_HEIGHT, " px"),
        ),
        entry(
            "/studio/frame/nav_radius",
            tk("Navigation corners"),
            span(ranges::RADIUS, " px"),
        ),
        entry(
            "/studio/frame/card_radius",
            tk("Card radius"),
            span(mandara_config::surfaces::ranges::CARD_RADIUS, " px"),
        ),
        entry(
            "/studio/frame/card_padding",
            tk("Card padding"),
            span(mandara_config::surfaces::ranges::CARD_PADDING, " px"),
        ),
    ]);
    let defaults = serde_json::to_value(mandara_config::studio::StudioAppearance::default())
        .expect("Studio appearance serializes");
    for setting in &mut entries {
        let pointer = setting
            .path
            .strip_prefix("/studio")
            .expect("Studio setting path");
        setting.default = Some(Box::new(
            defaults
                .pointer(pointer)
                .expect("Studio setting has a default")
                .clone(),
        ));
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::{ShellConfig, studio::StudioLook};

    #[test]
    fn studio_controls_write_only_the_independent_snapshot() {
        let base = ShellConfig {
            studio: Some(Box::new(StudioLook::Mandara.appearance())),
            ..Default::default()
        };
        for setting in super::super::settings().iter().filter(|s| s.group == GROUP) {
            let current = super::super::get(&base, setting);
            assert!(!current.is_null(), "{} must be a real field", setting.path);
            let next = super::super::set(&base, setting, current.clone()).unwrap();
            assert_eq!(super::super::get(&next, setting), current);
            assert_eq!(base.style, next.style);
            assert_eq!(base.theme, next.theme);
            assert_eq!(base.typography, next.typography);
        }
    }
}
