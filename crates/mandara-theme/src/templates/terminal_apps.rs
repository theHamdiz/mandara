//! Native theme formats for terminal tools and Spicetify.

use crate::MandaraPalette;

/// Export a Cava gradient using palette roles accepted by its INI parser.
pub fn cava(p: &MandaraPalette) -> String {
    format!(
        "[color]\nbackground = '{}'\nforeground = '{}'\ngradient = 1\ngradient_color_1 = '{}'\ngradient_color_2 = '{}'\ngradient_color_3 = '{}'\n",
        p.surface_base.to_hex(),
        p.on_surface.to_hex(),
        p.primary.to_hex(),
        p.secondary.to_hex(),
        p.tertiary.to_hex()
    )
}

/// Export sourceable tmux styles without changing the user's session or config.
pub fn tmux(p: &MandaraPalette) -> String {
    let fg = p.on_surface.to_hex();
    let bg = p.surface_container.to_hex();
    let accent = p.primary.to_hex();
    let selected = p.on_primary.to_hex();
    format!(
        "# Mandara palette\nset -g status-style 'fg={fg},bg={bg}'\nset -g message-style 'fg={selected},bg={accent}'\nset -g pane-border-style 'fg={border}'\nset -g pane-active-border-style 'fg={accent}'\nset -g mode-style 'fg={selected},bg={accent}'\nset -g window-status-current-style 'fg={selected},bg={accent}'\n",
        border = p.outline.to_hex()
    )
}

/// Export Zellij's KDL roles, preserving distinct success and error colours.
pub fn zellij(p: &MandaraPalette) -> String {
    use std::fmt::Write;
    let rgb = |color: crate::Color| {
        let [r, g, b, _] = color.to_rgba8();
        format!("{r} {g} {b}")
    };
    let mut out = String::from("themes {\n    mandara {\n");
    for (name, selected) in [
        ("text_unselected", false),
        ("text_selected", true),
        ("ribbon_unselected", false),
        ("ribbon_selected", true),
        ("table_title", true),
        ("table_cell_unselected", false),
        ("table_cell_selected", true),
        ("list_unselected", false),
        ("list_selected", true),
        ("frame_unselected", false),
        ("frame_selected", true),
        ("frame_highlight", true),
        ("exit_code_success", false),
        ("exit_code_error", false),
    ] {
        let fg = match name {
            "exit_code_success" => p.success_emerald,
            "exit_code_error" => p.urgent_coral,
            _ if selected => p.on_primary,
            _ => p.on_surface,
        };
        let bg = if selected { p.primary } else { p.surface_base };
        writeln!(out, "        {name} {{\n            base {}\n            background {}\n            emphasis_0 {}\n            emphasis_1 {}\n            emphasis_2 {}\n            emphasis_3 {}\n        }}", rgb(fg), rgb(bg), rgb(p.secondary), rgb(p.tertiary), rgb(p.success_emerald), rgb(p.warning_amber)).expect("writing a String");
    }
    out.push_str("    }\n}\n");
    out
}

/// Export the supported colour keys for Spicetify's Mandara scheme.
pub fn spicetify(p: &MandaraPalette) -> String {
    use std::fmt::Write;
    let mut out = String::from("[Mandara]\n");
    for (name, color) in [
        ("text", p.on_surface),
        ("subtext", p.on_surface_variant),
        ("main", p.surface_base),
        ("main-elevated", p.surface_container_high),
        ("highlight", p.surface_container_high),
        ("highlight-elevated", p.surface_container_high),
        ("sidebar", p.surface_container_low),
        ("player", p.surface_container),
        ("card", p.surface_container_high),
        ("shadow", p.surface_dim),
        ("selected-row", p.primary_container),
        ("button", p.primary),
        ("button-active", p.secondary),
        ("button-disabled", p.outline),
        ("tab-active", p.primary_container),
        ("notification", p.primary),
        ("notification-error", p.urgent_coral),
        ("misc", p.on_surface_variant),
    ] {
        writeln!(out, "{name} = {}", color.to_hex_rgb_bare()).expect("writing a String");
    }
    out
}

/// Spicetify expects a CSS file even when the theme only replaces colours.
pub fn spicetify_css(_: &MandaraPalette) -> String {
    "/* Colours are provided by color.ini. */\n".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_formats_follow_light_and_dark_palettes() {
        for dark in [false, true] {
            let p = crate::engine::default_palette(dark);
            assert!(cava(&p).contains(&format!("background = '{}'", p.surface_base.to_hex())));
            assert!(tmux(&p).contains(&format!("pane-active-border-style 'fg={}'", p.primary.to_hex())));
            let z = zellij(&p);
            assert_eq!(z.matches("emphasis_3 ").count(), 14);
            assert_eq!(z.matches('{').count(), z.matches('}').count());
            for (name, color) in [("exit_code_success", p.success_emerald), ("exit_code_error", p.urgent_coral)] {
                let [r, g, b, _] = color.to_rgba8();
                assert!(z.contains(&format!("{name} {{\n            base {r} {g} {b}\n")));
            }
            let s = spicetify(&p);
            assert!(s.starts_with("[Mandara]\n"));
            assert_eq!(s.lines().filter(|line| line.contains(" = ")).count(), 18);
            for value in s.lines().filter_map(|line| line.split_once(" = ").map(|(_, v)| v)) {
                assert_eq!(value.len(), 6);
                assert!(value.bytes().all(|b| b.is_ascii_hexdigit()));
            }
        }
    }
}
