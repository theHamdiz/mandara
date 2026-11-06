//! Lazily loaded catalogs for the strings a component returns.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mandara_i18n::{Catalog, Lang};

use super::{Kind, View};

/// A plugin's own translations (`<plugin dir>/locales/<code>.toml`),
/// loaded on first use per language.
pub struct PluginCatalogs {
    dir: PathBuf,
    loaded: HashMap<Lang, Option<Catalog>>,
}

impl PluginCatalogs {
    pub fn new(plugin_dir: &Path) -> Self {
        Self { dir: plugin_dir.join("locales"), loaded: HashMap::new() }
    }

    fn translation(&mut self, lang: Lang, text: &str) -> Option<&str> {
        if lang == Lang::En || text.is_empty() {
            return None;
        }
        let dir = &self.dir;
        self.loaded.entry(lang).or_insert_with(|| {
            let path = dir.join(format!("{}.toml", lang.code()));
            let text = std::fs::read_to_string(&path).ok()?;
            Catalog::parse(&text).map_err(|error| tracing::warn!("{}: {error}", path.display())).ok()
        }).as_ref()?.get(text)
    }

    /// `text` in the current language, or an owned copy when untranslated.
    pub fn translate(&mut self, text: &str) -> String {
        self.translation(mandara_i18n::language(), text).unwrap_or(text).to_string()
    }

    fn translate_in_place(&mut self, lang: Lang, text: &mut String) {
        if let Some(translated) = self.translation(lang, text) && translated != text.as_str() {
            text.clear();
            text.push_str(translated);
        }
    }

    pub(super) fn translate_owned(&mut self, mut text: String) -> String {
        self.translate_in_place(mandara_i18n::language(), &mut text);
        text
    }

    /// Translates owned labels while keeping their buffers when possible.
    pub(super) fn translate_view(&mut self, view: &mut View) {
        let lang = mandara_i18n::language();
        if lang == Lang::En {
            return;
        }
        for node in &mut view.nodes {
            if matches!(node.kind, Kind::Text | Kind::Button) {
                self.translate_in_place(lang, &mut node.text);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wasm::{Node, Tone};
    use mandara_i18n::with_language;

    fn catalogs() -> PluginCatalogs {
        let mut catalogs = PluginCatalogs::new(Path::new("unused-plugin-directory"));
        catalogs.loaded.insert(Lang::Ar, Some(Catalog::parse("\"Hello\" = \"مرحبا\"\n\"Same\" = \"Same\"\n\"A long label\" = \"قصير\"").unwrap()));
        catalogs.loaded.insert(Lang::Fr, Some(Catalog::parse("\"Hello\" = \"Bonjour\"").unwrap()));
        catalogs.loaded.insert(Lang::De, None);
        catalogs
    }

    #[test]
    fn owned_strings_keep_their_buffers_when_no_replacement_is_needed() {
        let mut catalogs = catalogs();
        for (lang, label) in [(Lang::En, "Hello"), (Lang::Ar, "Missing"), (Lang::Ar, "Same"), (Lang::De, "Hello")] {
            with_language(Some(lang), || {
                let text = label.to_string();
                let pointer = text.as_ptr();
                let translated = catalogs.translate_owned(text);
                assert_eq!(translated, label);
                assert_eq!(translated.as_ptr(), pointer);
            });
        }
    }

    #[test]
    fn translated_labels_reuse_capacity_and_follow_language_changes() {
        let mut catalogs = catalogs();
        let mut text = String::with_capacity(64);
        text.push_str("A long label");
        let pointer = text.as_ptr();
        with_language(Some(Lang::Ar), || {
            let translated = catalogs.translate_owned(text);
            assert_eq!(translated, "قصير");
            assert_eq!(translated.as_ptr(), pointer);
            assert_eq!(catalogs.translate("Hello"), "مرحبا");
        });
        with_language(Some(Lang::Fr), || assert_eq!(catalogs.translate_owned("Hello".into()), "Bonjour"));
        with_language(Some(Lang::En), || assert_eq!(catalogs.translate_owned("Hello".into()), "Hello"));
    }

    fn node(kind: Kind) -> Node {
        Node { kind, text: "Hello".into(), value: 0.0, children: Vec::new(), action: "run".into(), tone: Tone::Normal, size: 0.0, gap: 0.0, bold: false, grow: false }
    }

    #[test]
    fn view_translation_only_changes_text_and_button_labels() {
        let mut catalogs = catalogs();
        let mut view = View { nodes: vec![node(Kind::Text), node(Kind::Button), node(Kind::Icon), node(Kind::Meter)], root: 0 };
        with_language(Some(Lang::Ar), || catalogs.translate_view(&mut view));
        assert_eq!(view.nodes.iter().map(|node| node.text.as_str()).collect::<Vec<_>>(), ["مرحبا", "مرحبا", "Hello", "Hello"]);
        assert!(view.nodes.iter().all(|node| node.action == "run"));
        assert_eq!(view.root, 0);
    }

    #[test]
    fn english_and_empty_labels_do_not_load_a_catalog() {
        let mut catalogs = PluginCatalogs::new(Path::new("unused-plugin-directory"));
        with_language(Some(Lang::En), || assert_eq!(catalogs.translate_owned("Hello".into()), "Hello"));
        with_language(Some(Lang::Ar), || assert!(catalogs.translate_owned(String::new()).is_empty()));
        assert!(catalogs.loaded.is_empty());
    }
}
