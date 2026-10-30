//! Wallhaven: online wallpapers in the wallpaper picker (search, browse
//! the toplist, download at full size). Opt-in: switched off, the picker
//! shows no Wallhaven source and Mandara makes no request to wallhaven.cc.
//! The picker side lives in `crate::wallpaper`, the client in
//! `mandara_services::wallhaven`, and its settings (default search, sort
//! order) on Studio → Wallpaper while the plugin is on.

use mandara_i18n::tk;
use mandara_plugins::{PluginInstance, PluginManifest, PluginRegistry};

pub const ID: &str = mandara_config::upgrade::WALLHAVEN_ID;

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Wallhaven"), tk("Search and download wallpapers from wallhaven.cc right in the wallpaper picker"), None).opt_in(), |_, _, _| PluginInstance::empty());
}
