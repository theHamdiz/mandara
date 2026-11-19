//! Point qt6ct / qt5ct at Mandara's palette file without rewriting the rest
//! of their config.

use super::{Action, Context};
use crate::app_themes::preferences;
use anyhow::{Result, ensure};
use std::collections::BTreeMap;
use std::path::Path;

pub(super) struct Qt;

impl Action for Qt {
    fn apply(&self, context: &Context<'_>) -> Result<()> {
        apply_scheme(context.owner, context.output, context.locations)
    }
}

/// `…/qt6ct/colors/mandara.conf` → set `…/qt6ct/qt6ct.conf` Appearance keys.
fn apply_scheme(owner: &str, scheme: &Path, locations: &crate::app_themes::paths::Locations) -> Result<()> {
    let colors_dir = scheme.parent().ok_or_else(|| anyhow::anyhow!("qt colour scheme has no parent"))?;
    ensure!(colors_dir.file_name().is_some_and(|n| n == "colors"), "qt colour scheme must live in a colors/ folder");
    let ct_dir = colors_dir.parent().ok_or_else(|| anyhow::anyhow!("qt colours folder has no parent"))?;
    let name = ct_dir.file_name().and_then(|n| n.to_str()).unwrap_or_default();
    ensure!(name == "qt6ct" || name == "qt5ct", "unknown Qt config toolkit {name}");
    ensure!(
        ct_dir == locations.config.join(name).as_path(),
        "qt colour scheme must live under the reviewed configuration root"
    );
    let conf = ct_dir.join(format!("{name}.conf"));
    let scheme_path = scheme.to_string_lossy().into_owned();
    let mut keys: BTreeMap<String, BTreeMap<String, String>> = BTreeMap::new();
    keys.entry("Appearance".into()).or_default().extend([
        ("color_scheme_path".into(), scheme_path),
        ("custom_palette".into(), "true".into()),
    ]);
    // Only touch an existing qt*ct.conf (or create one under our config root
    // when the colours file was just written there).
    if !conf.exists() {
        std::fs::create_dir_all(ct_dir)?;
    }
    preferences::set(owner, &conf, &keys, locations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_themes::paths::Locations;

    #[test]
    fn selects_mandara_palette_in_existing_qt6ct_conf() {
        let root = tempfile::tempdir().unwrap();
        let locations = Locations {
            home: root.path().into(),
            config: root.path().join("config"),
            data: root.path().join("data"),
            cache: root.path().join("cache"),
            state: root.path().join("state"),
        };
        let qt6 = locations.config.join("qt6ct");
        std::fs::create_dir_all(qt6.join("colors")).unwrap();
        let scheme = qt6.join("colors/mandara.conf");
        std::fs::write(&scheme, "[ColorScheme]\nactive_colors=#ffffffff\n").unwrap();
        std::fs::write(qt6.join("qt6ct.conf"), "[Appearance]\nstyle=Fusion\nicon_theme=Papirus\ncolor_scheme_path=/old.colors\ncustom_palette=false\n").unwrap();
        apply_scheme("qt", &scheme, &locations).unwrap();
        let body = std::fs::read_to_string(qt6.join("qt6ct.conf")).unwrap();
        assert!(body.contains("style=Fusion") && body.contains("icon_theme=Papirus"));
        assert!(body.contains(&format!("color_scheme_path={}", scheme.display())));
        assert!(body.contains("custom_palette=true"));
    }
}
