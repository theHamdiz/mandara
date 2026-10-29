//! Greeter strings: an English-keyed catalog per language (in
//! `locales/<code>.toml`, the same format as `mandara-i18n`), falling back
//! to the shell's shared catalogs and then to English. The active language
//! is `mandara_i18n::language()`, so `set_language` drives both.

use std::fmt::Display;
use std::sync::OnceLock;

use mandara_i18n::{Catalog, Lang, fill, language, tr};

fn source(lang: Lang) -> &'static str {
    match lang {
        Lang::En => "",
        Lang::Ar => include_str!("../locales/ar.toml"),
        Lang::Es => include_str!("../locales/es.toml"),
        Lang::Fr => include_str!("../locales/fr.toml"),
        Lang::Zh => include_str!("../locales/zh.toml"),
        Lang::Tr => include_str!("../locales/tr.toml"),
        Lang::Ja => include_str!("../locales/ja.toml"),
        Lang::De => include_str!("../locales/de.toml"),
        Lang::Ru => include_str!("../locales/ru.toml"),
        Lang::Hi => include_str!("../locales/hi.toml"),
    }
}

/// The greeter's own catalog for `lang`.
pub fn catalog(lang: Lang) -> &'static Catalog {
    static CATALOGS: [OnceLock<Catalog>; 10] = [const { OnceLock::new() }; 10];
    CATALOGS[lang as usize].get_or_init(|| match Catalog::parse(source(lang)) {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("mandara-greeter: {} catalog is invalid: {e}", lang.code());
            Catalog::default()
        }
    })
}

/// Translates an English key into the current language.
pub fn tl(key: &str) -> String {
    match language() {
        Lang::En => key.to_string(),
        lang => catalog(lang).get(key).map(str::to_string).unwrap_or_else(|| tr(key)),
    }
}

/// [`tl`] with `{name}` placeholders.
pub fn tlf(key: &str, args: &[(&str, &dyn Display)]) -> String {
    fill(&tl(key), args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Keys used through `tl` / `tlf` string literals in this crate.
    fn source_keys() -> BTreeSet<String> {
        let mut keys = BTreeSet::new();
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut stack = vec![root.join("src")];
        // Studio's Login screen page uses the greeter catalog too.
        let mut files = vec![root.join("../mandara-studio/src/app/greeter_page.rs")];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    files.push(p);
                }
            }
        }
        {
            for p in files {
                if !p.exists() {
                    continue;
                }
                if p.extension().is_none_or(|x| x != "rs") {
                    continue;
                }
                let text = std::fs::read_to_string(&p).unwrap();
                for marker in ["tl(\"", "tlf(\""] {
                    let mut from = 0;
                    while let Some(pos) = text[from..].find(marker) {
                        let at = from + pos + marker.len();
                        let prev = text[..from + pos].chars().next_back();
                        if !prev.is_some_and(|c| c.is_alphanumeric() || c == '_') {
                            let end = text[at..].find('"').unwrap();
                            keys.insert(text[at..at + end].replace("\\\"", "\""));
                        }
                        from = at;
                    }
                }
            }
        }
        keys
    }

    #[test]
    fn catalogs_parse_and_keep_placeholders() {
        for lang in Lang::ALL.into_iter().filter(|l| *l != Lang::En) {
            let c = Catalog::parse(source(lang)).unwrap_or_else(|e| panic!("{}: {e}", lang.code()));
            for k in c.keys() {
                let v = c.get(k).unwrap();
                assert_eq!(mandara_i18n::placeholders(k), mandara_i18n::placeholders(v), "{}: {k:?} → {v:?}", lang.code());
            }
        }
    }

    #[test]
    fn every_greeter_string_is_translated() {
        let keys = source_keys();
        assert!(keys.len() >= 15, "scanner found only {} keys", keys.len());
        let mut missing = Vec::new();
        for lang in Lang::ALL.into_iter().filter(|l| *l != Lang::En) {
            for k in &keys {
                if catalog(lang).get(k).is_none() && mandara_i18n::catalog(lang).get(k).is_none() {
                    missing.push(format!("{}: {k:?}", lang.code()));
                }
            }
        }
        assert!(missing.is_empty(), "untranslated greeter strings:\n{}", missing.join("\n"));
    }
}
