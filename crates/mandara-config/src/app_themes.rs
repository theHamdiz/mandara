//! Opt-in imported and user-authored application themes.

use std::collections::BTreeMap;

use mandara_theme::templates::custom::{CustomColor, Template};
use serde::{Deserialize, Serialize};

/// Enough for the current catalogs, with a bounded number of jobs and inline styles.
pub const MAX_TEMPLATES: usize = 128;
/// Identifiers are also package directory names.
const MAX_ID_BYTES: usize = 128;
const MAX_SOURCES: usize = 16;
const MAX_INLINE_BYTES: usize = 2 * mandara_theme::templates::custom::MAX_TEMPLATE_BYTES;

pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_BYTES
        && id.bytes().all(|character| {
            character.is_ascii_alphanumeric() || character == b'-' || character == b'_'
        })
}

pub fn valid_approval(fingerprint: &str) -> bool {
    fingerprint.len() == 64 && fingerprint.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// A catalog is fetched when opened; palette changes use installed files.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CatalogSource {
    pub id: String,
    pub name: String,
    pub kind: CatalogKind,
    pub index_url: String,
    pub files_url: String,
    #[serde(default)]
    pub directory_url: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CatalogKind {
    Index,
    Manifest,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppThemesConfig {
    pub sources: Vec<CatalogSource>,
    /// Catalog ids are qualified by source (`community:bat`).
    pub enabled: Vec<String>,
    /// Consent is bound to the installed revision and its reviewed destinations.
    pub approved: BTreeMap<String, String>,
    pub user: BTreeMap<String, Template>,
    pub custom_colors: BTreeMap<String, CustomColor>,
    /// Template setup commands run only for an explicitly enabled entry.
    pub run_hooks: bool,
}

impl Default for AppThemesConfig {
    fn default() -> Self {
        Self {
            sources: vec![
                CatalogSource {
                    id: "community".into(),
                    name: "Community templates".into(),
                    kind: CatalogKind::Index,
                    index_url: "https://api.noctalia.dev/templates".into(),
                    files_url: "https://api.noctalia.dev/templates/{app}/{file}".into(),
                    directory_url: String::new(),
                },
                CatalogSource {
                    id: "official".into(),
                    name: "Official templates".into(),
                    kind: CatalogKind::Manifest,
                    index_url: "https://raw.githubusercontent.com/noctalia-dev/noctalia/main/assets/templates/builtin.toml".into(),
                    files_url: "https://raw.githubusercontent.com/noctalia-dev/noctalia/main/assets/templates/{app}/{file}".into(),
                    directory_url: "https://api.github.com/repos/noctalia-dev/noctalia/contents/assets/templates/{app}".into(),
                },
            ],
            enabled: Vec::new(),
            approved: BTreeMap::new(),
            user: BTreeMap::new(),
            custom_colors: BTreeMap::new(),
            run_hooks: true,
        }
    }
}

impl AppThemesConfig {
    /// Record an explicit activation before any background work can finish.
    pub fn approve(&mut self, id: &str, fingerprint: &str) -> anyhow::Result<()> {
        anyhow::ensure!(id.split_once(':').is_some_and(|(source, app)| valid_id(source) && source != "user" && valid_id(app)) && valid_approval(fingerprint), "invalid app-template approval");
        anyhow::ensure!((self.approved.contains_key(id) || self.approved.len() < MAX_TEMPLATES)
            && (self.enabled.iter().any(|enabled| enabled == id) || self.enabled.len() < MAX_TEMPLATES), "too many app templates");
        self.approved.insert(id.into(), fingerprint.into());
        if !self.enabled.iter().any(|enabled| enabled == id) { self.enabled.push(id.into()); }
        Ok(())
    }

    pub fn disable_catalog(&mut self, id: &str) {
        self.enabled.retain(|enabled| enabled != id);
        self.approved.remove(id);
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.sources.len() <= MAX_SOURCES,
            "too many app-template sources"
        );
        let sources: std::collections::BTreeSet<_> = self
            .sources
            .iter()
            .map(|source| source.id.as_str())
            .collect();
        anyhow::ensure!(
            sources.len() == self.sources.len()
                && sources.iter().all(|id| valid_id(id) && *id != "user"),
            "app-template source ids must be valid, unique and different from user"
        );
        anyhow::ensure!(
            self.enabled.len() <= MAX_TEMPLATES
                && self.user.len() <= MAX_TEMPLATES
                && self.custom_colors.len() <= MAX_TEMPLATES,
            "too many app templates or custom colors"
        );
        anyhow::ensure!(
            self.user.keys().all(|id| valid_id(id)),
            "invalid user template id"
        );
        let unique: std::collections::BTreeSet<_> =
            self.enabled.iter().map(String::as_str).collect();
        anyhow::ensure!(
            unique.len() == self.enabled.len()
                && self.enabled.iter().all(|id| id
                    .split_once(':')
                    .is_some_and(|(source, app)| valid_id(source) && valid_id(app))),
            "enabled app-template ids must be valid and unique"
        );
        anyhow::ensure!(self.approved.len() <= MAX_TEMPLATES && self.approved.iter().all(|(id, fingerprint)| {
            id.split_once(':').is_some_and(|(source, app)| valid_id(source) && valid_id(app))
                && valid_approval(fingerprint)
        }), "invalid app-template approval");
        for source in &self.sources {
            anyhow::ensure!(
                source.name.len() <= 256
                    && [&source.index_url, &source.files_url, &source.directory_url]
                        .iter()
                        .all(|url| url.len() <= 4096),
                "app-template source metadata is too large"
            );
            anyhow::ensure!(
                [&source.index_url, &source.files_url]
                    .iter()
                    .all(|url| url.starts_with("https://"))
                    && (source.directory_url.is_empty()
                        || source.directory_url.starts_with("https://")),
                "app-template catalogs must use HTTPS"
            );
        }
        for color in self.custom_colors.values() {
            color.validate()?;
        }
        let mut bytes = 0_usize;
        for template in self.user.values() {
            template.validate()?;
            bytes = bytes.saturating_add(template.text_bytes());
            anyhow::ensure!(
                bytes <= MAX_INLINE_BYTES,
                "app templates exceed their combined size limit"
            );
        }
        Ok(())
    }
    pub fn any_enabled(&self) -> bool {
        !self.enabled.is_empty() || self.user.values().any(|t| t.enabled)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_approvals_activate_once_and_disabling_revokes_the_token() {
        let mut config = AppThemesConfig::default();
        let token = "a".repeat(64);
        for invalid in ["", "../outside:app", "user:app", "source:"] { assert!(config.approve(invalid, &token).is_err()); }
        assert!(config.approve("community:app", "bad token").is_err());
        assert!(!config.any_enabled() && config.approved.is_empty());
        config.approve("community:app", &token).unwrap();
        config.approve("community:app", &token).unwrap();
        assert_eq!(config.enabled, ["community:app"]);
        assert_eq!(config.approved["community:app"], token);
        config.validate().unwrap();
        config.disable_catalog("community:app");
        assert!(!config.any_enabled() && config.approved.is_empty());
        config.validate().unwrap();
    }

    #[test]
    fn catalogs_are_available_without_enabling_any_app_or_hook() {
        let config = AppThemesConfig::default();
        assert!(!config.any_enabled());
        assert!(config.run_hooks, "an explicitly enabled template can run its setup unless commands are switched off");
        config.validate().unwrap();
        let encoded = toml::to_string(&config).unwrap();
        assert_eq!(toml::from_str::<AppThemesConfig>(&encoded).unwrap(), config);
        let template: Template = toml::from_str(
            "enabled = true\ninput_path = 'app.css'\noutput_path = ['one.css', 'two.css']\n",
        )
        .unwrap();
        assert_eq!(template.output_path.iter().count(), 2);
    }

    #[test]
    fn sources_and_inline_templates_have_explicit_limits() {
        let mut config = AppThemesConfig::default();
        config.sources.push(config.sources[0].clone());
        assert!(config.validate().is_err());
        config = AppThemesConfig::default();
        config.user.insert(
            "large".into(),
            Template {
                content: Some(
                    "x".repeat(mandara_theme::templates::custom::MAX_TEMPLATE_BYTES + 1)
                        .into(),
                ),
                ..Default::default()
            },
        );
        assert!(config.validate().is_err());
    }
}
