//! Each compositor supplies its own parser behind the shared editing contract.

mod cosmic;
mod kwin;
mod labwc;
mod lines;
pub(crate) mod lua;
mod niri;
mod toml;

use std::path::Path;

use crate::{compositor::Target, keybinds::ShortcutFormat};

pub(super) fn format(target: Target, file: &Path) -> &'static dyn ShortcutFormat {
    match target {
        Target::Mango if file.extension().is_some_and(|ext| ext == "toml") => {
            &toml::Toml(Target::Mango)
        }
        Target::Mango => &lines::Lines(Target::Mango),
        Target::Niri => &niri::Niri,
        Target::Hyprland if file.extension().is_some_and(|ext| ext == "lua") => &lua::Lua,
        Target::Hyprland => &lines::Lines(Target::Hyprland),
        Target::Umbriel => &toml::Toml(Target::Umbriel),
        Target::Sway => &lines::Lines(Target::Sway),
        Target::River => &lines::Lines(Target::River),
        Target::KWin => &kwin::KWin,
        Target::Aqueous => &toml::Toml(Target::Aqueous),
        Target::Labwc => &labwc::Labwc,
        Target::Cosmic => &cosmic::Cosmic,
    }
}
