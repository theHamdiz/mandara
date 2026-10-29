//! Localization for Mandara Shell.
//!
//! Strings are keyed by their English text, so English needs no catalog and
//! anything untranslated falls back to English automatically:
//!
//! ```
//! use mandara_i18n::{Lang, set_language, t, tf};
//! set_language(Lang::Ar);
//! assert_eq!(t("Settings"), "الإعدادات");
//! assert!(tf("{n} processes", &[("n", &3)]).contains('3'));
//! set_language(Lang::En);
//! assert_eq!(t("Settings"), "Settings");
//! ```
//!
//! Catalogs live in `locales/<code>.toml` as `"English" = "Translation"`
//! pairs and are compiled in. Placeholders are `{name}` and must survive
//! translation unchanged (checked by the tests). Plugins bring their own
//! catalogs in the same format ([`Catalog`]).

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};

/// The supported languages.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Lang {
    #[default]
    En,
    Ar,
    Es,
    Fr,
    Zh,
    Tr,
    Ja,
    De,
    Ru,
    Hi,
}

impl Lang {
    pub const ALL: [Lang; 10] = [Lang::En, Lang::Ar, Lang::Es, Lang::Fr, Lang::De, Lang::Tr, Lang::Ru, Lang::Hi, Lang::Zh, Lang::Ja];

    /// ISO 639-1 code.
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::Ar => "ar",
            Lang::Es => "es",
            Lang::Fr => "fr",
            Lang::Zh => "zh",
            Lang::Tr => "tr",
            Lang::Ja => "ja",
            Lang::De => "de",
            Lang::Ru => "ru",
            Lang::Hi => "hi",
        }
    }

    /// The language's name in itself ("العربية", "日本語").
    pub fn native_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Ar => "العربية",
            Lang::Es => "Español",
            Lang::Fr => "Français",
            Lang::Zh => "简体中文",
            Lang::Tr => "Türkçe",
            Lang::Ja => "日本語",
            Lang::De => "Deutsch",
            Lang::Ru => "Русский",
            Lang::Hi => "हिन्दी",
        }
    }

    pub fn english_name(self) -> &'static str {
        match self {
            Lang::En => "English",
            Lang::Ar => "Arabic",
            Lang::Es => "Spanish",
            Lang::Fr => "French",
            Lang::Zh => "Chinese (Simplified)",
            Lang::Tr => "Turkish",
            Lang::Ja => "Japanese",
            Lang::De => "German",
            Lang::Ru => "Russian",
            Lang::Hi => "Hindi",
        }
    }

    /// A short sample in the language, for pickers.
    pub fn sample(self) -> &'static str {
        match self {
            Lang::En => "Hello",
            Lang::Ar => "مرحبًا",
            Lang::Es => "Hola",
            Lang::Fr => "Bonjour",
            Lang::Zh => "你好",
            Lang::Tr => "Merhaba",
            Lang::Ja => "こんにちは",
            Lang::De => "Hallo",
            Lang::Ru => "Привет",
            Lang::Hi => "नमस्ते",
        }
    }

    pub fn is_rtl(self) -> bool {
        self == Lang::Ar
    }

    /// The POSIX locale used for dates (`ar_EG`, `ja_JP`, …).
    pub fn posix(self) -> &'static str {
        match self {
            Lang::En => "en_US",
            Lang::Ar => "ar_EG",
            Lang::Es => "es_ES",
            Lang::Fr => "fr_FR",
            Lang::Zh => "zh_CN",
            Lang::Tr => "tr_TR",
            Lang::Ja => "ja_JP",
            Lang::De => "de_DE",
            Lang::Ru => "ru_RU",
            Lang::Hi => "hi_IN",
        }
    }

    /// Parses `ar`, `ar_EG.UTF-8`, `zh-CN`, … Unknown languages → `None`.
    pub fn from_code(code: &str) -> Option<Lang> {
        let primary = code.trim().split(['_', '-', '.', '@']).next()?.to_ascii_lowercase();
        Lang::ALL.into_iter().find(|l| l.code() == primary)
    }

    /// The desktop's language (`LC_ALL`, `LC_MESSAGES`, `LANG`), English
    /// when unset or unsupported.
    pub fn from_env() -> Lang {
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.is_empty() && v != "C" && v != "POSIX"))
            .and_then(|v| Lang::from_code(&v))
            .unwrap_or(Lang::En)
    }

    fn from_u8(v: u8) -> Lang {
        Lang::ALL.into_iter().find(|l| *l as u8 == v).unwrap_or(Lang::En)
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(Lang::En as u8);

pub fn set_language(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

pub fn language() -> Lang {
    Lang::from_u8(CURRENT.load(Ordering::Relaxed))
}

/// An English-keyed string table.
/// One language's translations. Keys and values are exact-size boxed
/// strings (no spare capacity), moved out of the parsed TOML, not cloned.
#[derive(Clone, Debug, Default)]
pub struct Catalog {
    map: HashMap<Box<str>, Box<str>>,
}

impl Catalog {
    /// Parses `"English" = "Translation"` TOML (tables are flattened, so
    /// catalogs may be grouped under `[section]` headers for readability).
    pub fn parse(text: &str) -> Result<Catalog, String> {
        let value: toml::Table = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut map = HashMap::with_capacity(value.len());
        fn walk(t: toml::Table, map: &mut HashMap<Box<str>, Box<str>>) {
            for (k, v) in t {
                match v {
                    toml::Value::String(s) if !s.is_empty() => {
                        map.insert(k.into_boxed_str(), s.into_boxed_str());
                    }
                    toml::Value::Table(inner) => walk(inner, map),
                    _ => {}
                }
            }
        }
        walk(value, &mut map);
        map.shrink_to_fit();
        Ok(Catalog { map })
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        self.map.get(key).map(|v| &**v)
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn keys(&self) -> impl Iterator<Item = &str> {
        self.map.keys().map(|k| &**k)
    }
}

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

/// The built-in catalog for `lang` (parsed once, leaked for `'static`).
pub fn catalog(lang: Lang) -> &'static Catalog {
    static CATALOGS: [OnceLock<Catalog>; 10] = [const { OnceLock::new() }; 10];
    CATALOGS[lang as usize].get_or_init(|| match Catalog::parse(source(lang)) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("mandara-i18n: {} catalog is invalid: {e}", lang.code());
            Catalog::default()
        }
    })
}

/// Translates `key` (English) into the current language.
pub fn t(key: &'static str) -> &'static str {
    match language() {
        Lang::En => key,
        lang => catalog(lang).get(key).unwrap_or(key),
    }
}

/// [`t`] for keys that aren't `'static` (e.g. names from data).
pub fn tr(key: &str) -> String {
    match language() {
        Lang::En => key.to_string(),
        lang => catalog(lang).get(key).unwrap_or(key).to_string(),
    }
}

/// Translates `key` and fills `{name}` placeholders.
pub fn tf(key: &'static str, args: &[(&str, &dyn Display)]) -> String {
    fill(t(key), args)
}

/// The English key of a string translated into the current language (or
/// the string itself when it isn't a known translation).
pub fn untranslate(s: &str) -> String {
    match language() {
        Lang::En => s.to_string(),
        lang => catalog(lang).map.iter().find(|(_, v)| &***v == s).map(|(k, _)| k.to_string()).unwrap_or_else(|| s.to_string()),
    }
}

/// Fills `{name}` placeholders in an already translated string.
///
/// One pass, one output buffer: each `{name}` with a matching argument is
/// written in place; anything else (unknown names, stray braces) is copied.
pub fn fill(template: &str, args: &[(&str, &dyn Display)]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(template.len());
    let mut rest = template;
    while let Some(open) = rest.find('{') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let arg = after.find('}').and_then(|close| args.iter().find(|(name, _)| *name == &after[..close]).map(|(_, v)| (close, v)));
        match arg {
            Some((close, value)) => {
                let _ = write!(out, "{value}");
                rest = &after[close + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `{name}` placeholders in a string, sorted.
pub fn placeholders(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(open) = rest.find('{') {
        let after = &rest[open + 1..];
        match after.find('}') {
            Some(close) if after[..close].chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && close > 0 => {
                out.push(after[..close].to_string());
                rest = &after[close + 1..];
            }
            _ => rest = after,
        }
    }
    out.sort();
    out
}

#[cfg(test)]
mod tests;

/// Marks a string for translation without translating it yet (e.g. in a
/// `const` table; translate at display time with [`tr`]).
pub const fn tk(key: &'static str) -> &'static str {
    key
}
