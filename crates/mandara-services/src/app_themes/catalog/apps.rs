//! Searchable product names for templates shared by several applications.

use anyhow::{Result, ensure};
use mandara_config::app_themes::{COMMUNITY_FILES_URL, COMMUNITY_INDEX_URL, CatalogKind, CatalogSource};

use super::Entry;

/// Bound cached presentation metadata independently of the catalog payload.
const MAX_ALIASES: usize = 8;
const MAX_ALIAS_BYTES: usize = 128;
const CHROMIUM_APPS: &[&str] = &["Helium", "Chromium", "Chrome", "Brave"];

pub(super) fn annotate(source: &CatalogSource, entries: &mut [Entry]) {
    let canonical = source.kind == CatalogKind::Index
        && source.index_url == COMMUNITY_INDEX_URL
        && source.files_url == COMMUNITY_FILES_URL;
    for entry in entries {
        entry.aliases.clear();
        if canonical && entry.id == "chromium-based" {
            entry.aliases.extend(CHROMIUM_APPS.iter().map(|name| (*name).to_owned()));
        }
    }
}

pub(super) fn validate(aliases: &[String]) -> Result<()> {
    ensure!(aliases.len() <= MAX_ALIASES && aliases.iter().all(|name| name.len() <= MAX_ALIAS_BYTES), "app aliases are too large");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{Transport, list_with};
    use mandara_config::app_themes::AppThemesConfig;

    struct Catalog;
    impl Transport for Catalog {
        fn get(&self, _: &str, _: u64) -> Result<Vec<u8>> {
            Ok(br#"[{"name":"chromium-based","files":[]}]"#.to_vec())
        }
    }

    #[test]
    fn shared_browser_names_survive_cache_and_keep_template_identity() {
        let root = tempfile::tempdir().unwrap();
        let source = AppThemesConfig::default().sources.remove(0);
        for refresh in [true, false] {
            let entries = list_with(&source, root.path(), refresh, &Catalog).unwrap();
            assert_eq!(entries[0].aliases, CHROMIUM_APPS);
            assert_eq!(entries[0].qualified_id(), "community:chromium-based");
            assert_eq!(entries[0].folder, "chromium-based");
        }
        let mut custom = source;
        custom.files_url = "https://example.org/{app}/{file}".into();
        assert!(list_with(&custom, root.path(), true, &Catalog).unwrap()[0].aliases.is_empty());
    }

    #[test]
    fn old_cached_catalogs_gain_names_and_untrusted_names_are_removed() {
        let root = tempfile::tempdir().unwrap();
        let source = AppThemesConfig::default().sources.remove(0);
        let mut entries = list_with(&source, root.path(), true, &Catalog).unwrap();
        entries[0].aliases.clear();
        let file = root.path().join("community/catalog.json");
        std::fs::write(&file, serde_json::to_vec(&serde_json::json!({"source":source,"entries":entries})).unwrap()).unwrap();
        assert_eq!(list_with(&source, root.path(), false, &Catalog).unwrap()[0].aliases, CHROMIUM_APPS);
        let mut custom = source;
        custom.index_url = "https://example.org/index".into();
        annotate(&custom, &mut entries);
        assert!(entries[0].aliases.is_empty());
        assert!(validate(&vec![String::new(); MAX_ALIASES + 1]).is_err());
        assert!(validate(&["x".repeat(MAX_ALIAS_BYTES + 1)]).is_err());
    }
}
