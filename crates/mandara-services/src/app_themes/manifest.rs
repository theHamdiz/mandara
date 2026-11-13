//! Imported catalogs describe data files and opt-in application setup steps.

use anyhow::{Result, ensure};
use mandara_theme::templates::custom::{CustomColor, Template};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Enough for multi-client themes while bounding one imported package.
pub const MAX_ENTRIES: usize = 128;
/// A manifest is metadata and short inline styles, not an arbitrary data archive.
const MAX_MANIFEST_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Metadata {
    pub name: String,
    pub category: String,
    pub documentation_url: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Options {
    pub custom_colors: BTreeMap<String, CustomColor>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub config: Options,
    pub catalog: BTreeMap<String, Metadata>,
    pub templates: BTreeMap<String, Template>,
}

pub use mandara_config::app_themes::valid_id;

impl Manifest {
    pub fn parse(text: &str) -> Result<Self> {
        ensure!(
            text.len() <= MAX_MANIFEST_BYTES,
            "theme manifest is too large"
        );
        let manifest: Self = toml::from_str(text)?;
        ensure!(
            !manifest.templates.is_empty() && manifest.templates.len() <= MAX_ENTRIES,
            "a theme package must contain 1–{MAX_ENTRIES} entries"
        );
        manifest.validate()?;
        Ok(manifest)
    }

    pub fn parse_user(text: &str) -> Result<Self> {
        ensure!(
            text.len() <= MAX_MANIFEST_BYTES,
            "theme manifest is too large"
        );
        let manifest: Self = toml::from_str(text)?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.templates.len() <= MAX_ENTRIES,
            "too many template entries"
        );
        ensure!(
            self.config.custom_colors.len() <= MAX_ENTRIES,
            "too many custom colors"
        );
        for color in self.config.custom_colors.values() {
            color.validate()?;
        }
        for (id, entry) in &self.templates {
            ensure!(valid_id(id), "invalid theme entry id: {id}");
            ensure!(
                entry
                    .content
                    .as_ref()
                    .is_none_or(|content| content.len()
                        <= mandara_theme::templates::custom::MAX_TEMPLATE_BYTES),
                "{id}: inline template is too large"
            );
            entry
                .validate()
                .map_err(|error| anyhow::anyhow!("{id}: {error:#}"))?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multi_client_entries_and_gtk_extensions_use_the_same_manifest() {
        let manifest = Manifest::parse("[catalog.example]\nname = 'Example'\n[templates.desktop]\ninput_path = 'theme.ini'\noutput_path = ['one.ini', 'two.ini']\n[templates.gtk]\ninput_path = 'app.css'\ngtk_extension = 'gtk3'\n").unwrap();
        assert_eq!(manifest.templates.len(), 2);
        assert!(manifest.templates["desktop"].hook_async);
        for extra in ["post_hook = 'false'", "output_path = 'gtk.css'"] {
            assert!(
                Manifest::parse(&format!(
                    "[templates.gtk]\ninput_path = 'app.css'\ngtk_extension = 'gtk3'\n{extra}\n"
                ))
                .is_err()
            );
        }
        assert!(!valid_id("../outside") && !valid_id("/absolute"));
    }
}
