//! Localization for Mandara Shell.
//!
//! Strings are keyed by their English text, so English needs no catalog and
//! anything untranslated falls back to English automatically:
//!
//! ```
//! use mandara_i18n::{Lang, set_language, t, tf};
//! set_language(Lang::Ar);
//! // "الإعدادات" with the Arabic pack, "Settings" (English) without it.
//! let settings = t("Settings");
//! assert!(settings == "الإعدادات" || settings == "Settings");
//! assert!(tf("{n} processes", &[("n", &3)]).contains('3'));
//! set_language(Lang::En);
//! assert_eq!(t("Settings"), "Settings");
//! ```
//!
//! Translations are written in `locales/<code>.toml` as
//! `"English" = "Translation"` pairs. They are **not** compiled into the
//! binary: releases ship them as language packs ([`pack`], `.mlang`), one
//! file per language and Mandara version, memory-mapped and read in place
//! ([`packs`] says where they live and how a missing one is downloaded).
//! Debug builds without a pack read the source catalogs instead. Placeholders
//! are `{name}` and must survive translation unchanged (checked by the
//! tests). Plugins bring their own catalogs in the TOML format ([`Catalog`]).

use std::collections::HashMap;
use std::fmt::Display;
use std::sync::atomic::{AtomicU8, Ordering};

pub mod pack;
pub mod packs;
pub mod bundle;

/// The supported languages.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
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
    Id,
    Fa,
    Ko,
    Ur,
    Bn,
    Pt,
    Mr,
    Vi,
    Te,
    Sw,
}

impl Lang {
    pub const ALL: [Lang; 20] = [Lang::En, Lang::Ar, Lang::Fa, Lang::Ur, Lang::Es, Lang::Pt, Lang::Fr, Lang::De, Lang::Tr, Lang::Ru, Lang::Hi, Lang::Mr, Lang::Bn, Lang::Te, Lang::Id, Lang::Sw, Lang::Vi, Lang::Zh, Lang::Ja, Lang::Ko];

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
            Lang::Id => "id",
            Lang::Fa => "fa",
            Lang::Ko => "ko",
            Lang::Ur => "ur",
            Lang::Bn => "bn",
            Lang::Pt => "pt",
            Lang::Mr => "mr",
            Lang::Vi => "vi",
            Lang::Te => "te",
            Lang::Sw => "sw",
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
            Lang::Id => "Bahasa Indonesia",
            Lang::Fa => "فارسی",
            Lang::Ko => "한국어",
            Lang::Ur => "اردو",
            Lang::Bn => "বাংলা",
            Lang::Pt => "Português",
            Lang::Mr => "मराठी",
            Lang::Vi => "Tiếng Việt",
            Lang::Te => "తెలుగు",
            Lang::Sw => "Kiswahili",
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
            Lang::Id => "Indonesian",
            Lang::Fa => "Persian",
            Lang::Ko => "Korean",
            Lang::Ur => "Urdu",
            Lang::Bn => "Bengali",
            Lang::Pt => "Portuguese",
            Lang::Mr => "Marathi",
            Lang::Vi => "Vietnamese",
            Lang::Te => "Telugu",
            Lang::Sw => "Swahili",
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
            Lang::Id => "Halo",
            Lang::Fa => "سلام",
            Lang::Ko => "안녕하세요",
            Lang::Ur => "السلام علیکم",
            Lang::Bn => "হ্যালো",
            Lang::Pt => "Olá",
            Lang::Mr => "नमस्कार",
            Lang::Vi => "Xin chào",
            Lang::Te => "నమస్కారం",
            Lang::Sw => "Habari",
        }
    }

    /// Right-to-left languages: Arabic, Persian and Urdu.
    pub fn is_rtl(self) -> bool {
        matches!(self, Lang::Ar | Lang::Fa | Lang::Ur)
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
            Lang::Id => "id_ID",
            Lang::Fa => "fa_IR",
            Lang::Ko => "ko_KR",
            Lang::Ur => "ur_PK",
            Lang::Bn => "bn_BD",
            // Brazilian Portuguese: the variety this catalog is written in.
            Lang::Pt => "pt_BR",
            Lang::Mr => "mr_IN",
            Lang::Vi => "vi_VN",
            Lang::Te => "te_IN",
            // Standard Swahili; Kenya is the POSIX locale this catalog follows.
            Lang::Sw => "sw_KE",
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
        let values = ["LC_ALL", "LC_MESSAGES", "LANG"].map(|key| std::env::var(key).ok());
        Self::from_locale_values(values.iter().map(|value| value.as_deref()))
    }

    fn from_locale_values<'a>(values: impl IntoIterator<Item = Option<&'a str>>) -> Lang {
        values.into_iter().flatten().find(|value| !value.is_empty())
            .and_then(Lang::from_code)
            .unwrap_or(Lang::En)
    }

    fn from_u8(v: u8) -> Lang {
        Lang::ALL.into_iter().find(|l| *l as u8 == v).unwrap_or(Lang::En)
    }
}

static CURRENT: AtomicU8 = AtomicU8::new(Lang::En as u8);

thread_local! {
    /// A language this thread speaks for now instead of the shell's (a
    /// plugin with a language of its own, while its views are built).
    static OVERRIDE: std::cell::Cell<Option<Lang>> = const { std::cell::Cell::new(None) };
}

pub fn set_language(lang: Lang) {
    CURRENT.store(lang as u8, Ordering::Relaxed);
}

/// The language strings are translated into: a [`scoped`] override on this
/// thread, else the shell's; English while that language's pack isn't
/// there yet (so layout direction, dates and digits match the English text
/// shown meanwhile). [`requested_language`] is the one asked for.
pub fn language() -> Lang {
    let lang = requested_language();
    if packs::is_available(lang) { lang } else { Lang::En }
}

/// The language asked for (a [`scoped`] override, else the shell's), whether
/// or not its pack is loaded.
pub fn requested_language() -> Lang {
    OVERRIDE.with(std::cell::Cell::get).unwrap_or_else(shell_language)
}

/// The shell's language, whatever a [`scoped`] override says.
pub fn shell_language() -> Lang {
    Lang::from_u8(CURRENT.load(Ordering::Relaxed))
}

/// While it lives, [`language`] (and so [`t`], [`tf`], [`tr`] and every
/// reading-direction check built on it) answers `lang` on this thread;
/// `None` changes nothing. Dropping it restores what was there before, so
/// scopes nest. For a plugin whose own language differs from the shell's.
#[must_use = "the override ends when the guard is dropped"]
pub struct LangScope {
    previous: Option<Lang>,
    active: bool,
}

/// Overrides the language on this thread until the guard drops.
pub fn scoped(lang: Option<Lang>) -> LangScope {
    match lang {
        Some(l) => LangScope { previous: OVERRIDE.with(|o| o.replace(Some(l))), active: true },
        None => LangScope { previous: None, active: false },
    }
}

impl Drop for LangScope {
    fn drop(&mut self) {
        if self.active {
            OVERRIDE.with(|o| o.set(self.previous));
        }
    }
}

/// Runs `f` with the language overridden by `lang` (see [`scoped`]).
pub fn with_language<R>(lang: Option<Lang>, f: impl FnOnce() -> R) -> R {
    let _scope = scoped(lang);
    f()
}

/// The translation of `key` in `lang` (`None`: English, untranslated, or
/// no pack for `lang`).
#[inline]
pub fn lookup(lang: Lang, key: &str) -> Option<&'static str> {
    packs::pack(lang)?.get(key)
}

/// [`t`] in an explicit language.
#[inline]
pub fn t_in(lang: Lang, key: &'static str) -> &'static str {
    lookup(lang, key).unwrap_or(key)
}

/// [`tr`] in an explicit language.
pub fn tr_in(lang: Lang, key: &str) -> String {
    lookup(lang, key).unwrap_or(key).to_string()
}

/// Compiles a `locales/<code>.toml` catalog into `lang`'s language pack for
/// Mandara `version` (release tooling, the sandbox and tests).
pub fn compile(lang: Lang, version: &str, toml_text: &str) -> Result<Vec<u8>, String> {
    pack::build(lang, version, Catalog::parse(toml_text)?.iter())
}

/// An English-keyed string table parsed from TOML: plugins' own catalogs,
/// and the input of [`compile`]. Keys and values are exact-size boxed
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

    /// Every `(english, translation)` pair.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.map.iter().map(|(k, v)| (&**k, &**v))
    }
}

/// Translates `key` (English) into the current language.
pub fn t(key: &'static str) -> &'static str {
    t_in(requested_language(), key)
}

/// [`t`] for keys that aren't `'static` (e.g. names from data).
pub fn tr(key: &str) -> String {
    tr_in(requested_language(), key)
}

/// Translates `key` and fills `{name}` placeholders.
pub fn tf(key: &'static str, args: &[(&str, &dyn Display)]) -> String {
    fill(t(key), args)
}

/// The English key of a string translated into the current language (or
/// the string itself when it isn't a known translation).
pub fn untranslate(s: &str) -> String {
    packs::pack(requested_language()).and_then(|p| bundle::shared_key(p, s)).unwrap_or(s).to_string()
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
