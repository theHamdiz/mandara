//! Keep local template inputs and setup commands relative to their document.

use super::manifest::Manifest;
use anyhow::{Result, ensure};
use std::io::Read as _;
use std::path::Path;

/// A document has a small number of entries; template bodies are bounded separately.
const MAX_DOCUMENT_BYTES: u64 = 2 * 1024 * 1024;

pub fn document(path: &Path) -> Result<Manifest> {
    let path = path.canonicalize()?;
    let mut text = String::new();
    std::fs::File::open(&path)?
        .take(MAX_DOCUMENT_BYTES + 1)
        .read_to_string(&mut text)?;
    ensure!(
        text.len() as u64 <= MAX_DOCUMENT_BYTES,
        "template document is too large"
    );
    let mut manifest = Manifest::parse_user(&text)?;
    for entry in manifest.templates.values_mut() {
        if entry.directory.is_none() {
            entry.directory = path.parent().map(Path::to_path_buf);
        }
    }
    Ok(manifest)
}

/// Import new names without replacing or disabling an existing user's entry.
pub fn merge(
    config: &mut mandara_config::app_themes::AppThemesConfig,
    document: Manifest,
) -> Result<()> {
    ensure!(
        document
            .templates
            .keys()
            .all(|id| !config.user.contains_key(id)),
        "template name already exists; rename it or edit Your templates"
    );
    ensure!(
        document
            .config
            .custom_colors
            .keys()
            .all(|id| !config.custom_colors.contains_key(id)),
        "custom color name already exists"
    );
    let mut next = config.clone();
    next.user
        .extend(document.templates.into_iter().map(|(id, mut entry)| {
            entry.enabled = false;
            (id, entry)
        }));
    next.custom_colors.extend(document.config.custom_colors);
    next.validate()?;
    *config = next;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflicting_imports_leave_existing_templates_enabled_and_unchanged() {
        let mut config = mandara_config::app_themes::AppThemesConfig::default();
        let document = Manifest::parse_user(
            "[templates.example]\nenabled = true\ncontent = 'color'\noutput_path = '~/app.css'\n",
        )
        .unwrap();
        merge(&mut config, document.clone()).unwrap();
        config.user.get_mut("example").unwrap().enabled = true;
        let original = config.clone();
        assert!(merge(&mut config, document).is_err());
        assert_eq!(config, original);
    }

    #[test]
    fn local_imports_keep_relative_inputs_and_do_not_enable_apps() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("themes.toml");
        std::fs::write(
            &path,
            "[templates.example]\ninput_path = 'colors.css'\noutput_path = '~/app/theme.css'\n",
        )
        .unwrap();
        let imported = document(&path).unwrap();
        let entry = &imported.templates["example"];
        assert_eq!(
            entry.directory.as_deref(),
            Some(directory.path().canonicalize().unwrap().as_path())
        );
        assert_eq!(entry.input_path, Path::new("colors.css"));
        assert!(!entry.enabled);
    }
}
