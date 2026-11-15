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
    /// Catalogs under `<plugin_dir>/locales`; nothing is read until a
    /// language is first needed.
    pub fn new(plugin_dir: &Path) -> Self {
        Self { dir: plugin_dir.join("locales"), loaded: HashMap::new() }
    }

    /// `text`'s translation in `lang`, loading that language's catalog once
    /// (a missing or invalid file is remembered as absent). English and empty
    /// text are never looked up.
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

    /// Replaces `text` with its translation, reusing its buffer.
    fn translate_in_place(&mut self, lang: Lang, text: &mut String) {
        if let Some(translated) = self.translation(lang, text) && translated != text.as_str() {
            text.clear();
            text.push_str(translated);
        }
    }

    /// An owned string a component returned, translated into the current
    /// language without a second allocation when it fits.
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

    /// Replaces `text` with its translation only while the translated total
    /// stays within `budget` bytes (a catalog can map a short label to a
    /// long string); the length is checked before anything is copied.
    /// Returns whether it still fits.
    fn translate_bounded(&mut self, lang: Lang, text: &mut String, budget: &mut usize) -> bool {
        let len = match self.translation(lang, text) {
            Some(t) if t != text.as_str() => t.len(),
            _ => text.len(),
        };
        if len > *budget {
            return false;
        }
        *budget -= len;
        self.translate_in_place(lang, text);
        true
    }

    /// [`Self::translate_view`] for extension views: labels, button text,
    /// hints and option labels, within the view's text budget
    /// ([`crate::extension::MAX_VIEW_BYTES`]): past it the rest stays as
    /// sent. A field's value (`input`) is the person's own text and is
    /// never translated.
    pub(super) fn translate_view2(&mut self, view: &mut super::View2) {
        use super::Kind2;
        let lang = mandara_i18n::language();
        if lang == Lang::En {
            return;
        }
        // The untranslated text already counts against the budget.
        let sent: usize = view.nodes.iter().map(|n| n.text.len() + n.hint.len() + n.options.iter().map(|o| o.label.len()).sum::<usize>()).sum();
        let mut budget = crate::extension::MAX_VIEW_BYTES.saturating_sub(sent);
        for node in &mut view.nodes {
            // Each replacement may use what its original text used, plus
            // what is left.
            let grow = |this: &mut Self, text: &mut String, budget: &mut usize| {
                let mut room = *budget + text.len();
                if this.translate_bounded(lang, text, &mut room) {
                    *budget = room;
                }
            };
            if matches!(node.kind, Kind2::Text | Kind2::Button | Kind2::Toggle) {
                grow(self, &mut node.text, &mut budget);
            }
            grow(self, &mut node.hint, &mut budget);
            for option in &mut node.options {
                grow(self, &mut option.label, &mut budget);
            }
        }
    }

    /// Translates a launcher result's title and detail, each only when its
    /// translation fits `max` bytes.
    pub(super) fn translate_hit(&mut self, hit: &mut super::Hit, max: usize) {
        let lang = mandara_i18n::language();
        if lang == Lang::En {
            return;
        }
        for text in [&mut hit.title, &mut hit.detail] {
            let mut room = max;
            self.translate_bounded(lang, text, &mut room);
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

    /// A catalog mapping a short label to a huge string can't grow a view
    /// past its text budget (the copies stop before it, untranslated).
    #[test]
    fn translation_stays_within_the_view_budget() {
        use crate::wasm::{Align, Kind2, Node2, Style, View2};
        let huge = "y".repeat(crate::extension::MAX_VIEW_BYTES / 4);
        let mut catalogs = PluginCatalogs::new(Path::new("unused-plugin-directory"));
        catalogs.loaded.insert(Lang::Ar, Some(Catalog::parse(&format!("\"x\" = \"{huge}\"")).unwrap()));
        let style = Style { width: 0.0, height: 0.0, min_width: 0.0, min_height: 0.0, padding: 0.0, gap: 0.0, radius: 0.0, size: 0.0, grow: false, bold: false, align: Align::Auto, tone: Tone::Normal };
        let text = |t: &str| Node2 { id: String::new(), kind: Kind2::Text, text: t.into(), hint: String::new(), action: String::new(), value: 0.0, min: 0.0, max: 0.0, step: 0.0, index: 0, children: Vec::new(), style: style.clone(), options: Vec::new(), image: 0, marks: Vec::new(), disabled: false };
        let mut view = View2 { nodes: (0..100).map(|_| text("x")).collect(), root: 0, focus: String::new() };
        with_language(Some(Lang::Ar), || catalogs.translate_view2(&mut view));
        let total: usize = view.nodes.iter().map(|n| n.text.len()).sum();
        assert!(total <= crate::extension::MAX_VIEW_BYTES, "{total} bytes");
        assert!(view.nodes.iter().any(|n| n.text == "x"), "the rest stays as sent");
    }
}
