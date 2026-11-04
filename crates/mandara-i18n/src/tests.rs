//! Catalog checks: every catalog parses, placeholders survive translation,
//! and every string the code asks for is translated in every language.
//!
//! `MANDARA_I18N_DUMP=1 cargo test -p mandara-i18n` writes the full key
//! list to `locales/keys.txt` (one per line) for translators.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::*;

#[test]
fn terminal_language_obeys_locale_precedence_including_c_and_posix() {
    assert_eq!(Lang::from_locale_values([Some("C"), Some("id_ID.UTF-8"), Some("ar_EG.UTF-8")]), Lang::En);
    assert_eq!(Lang::from_locale_values([Some("POSIX"), None, Some("id")]), Lang::En);
    assert_eq!(Lang::from_locale_values([Some(""), Some("id_ID.UTF-8"), Some("ar")]), Lang::Id);
    assert_eq!(Lang::from_locale_values([None, None, Some("ar_EG.UTF-8")]), Lang::Ar);
    assert_eq!(Lang::from_locale_values([Some("unsupported"), None, Some("id")]), Lang::En);
}

fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Reads a Rust string literal starting right after its opening quote.
fn literal(s: &str) -> Option<String> {
    let mut out = String::new();
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                '\'' => out.push('\''),
                '0' => out.push('\0'),
                'u' => {
                    let rest: String = chars.by_ref().take_while(|c| *c != '}').collect();
                    out.push(char::from_u32(u32::from_str_radix(rest.trim_start_matches('{'), 16).ok()?)?);
                }
                '\n' => {
                    // Line continuation: skip leading whitespace.
                    let rest: String = chars.clone().collect();
                    let trimmed = rest.trim_start();
                    let skip = rest.chars().count() - trimmed.chars().count();
                    for _ in 0..skip {
                        chars.next();
                    }
                }
                other => {
                    out.push('\\');
                    out.push(other);
                }
            },
            c => out.push(c),
        }
    }
    None
}

/// Keys used through `t("…")`, `tf("…"`, `tr("…")` and `tk("…")`.
fn keys_in(source: &str) -> Vec<String> {
    let mut keys = Vec::new();
    for marker in ["t(\"", "tf(\"", "tr(\"", "tk(\""] {
        let mut from = 0;
        while let Some(pos) = source[from..].find(marker) {
            let at = from + pos;
            let prev = source[..at].chars().next_back();
            let ident_before = prev.is_some_and(|c| c.is_alphanumeric() || c == '_');
            if !ident_before && let Some(k) = literal(&source[at + marker.len()..]) {
                keys.push(k);
            }
            from = at + marker.len();
        }
    }
    keys
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        if p.is_dir() {
            if p.file_name().is_some_and(|n| n == "target" || n == "tests.rs") {
                continue;
            }
            rust_files(&p, out);
        } else if p.extension().is_some_and(|x| x == "rs") && p.file_name().is_some_and(|n| n != "tests.rs") {
            out.push(p);
        }
    }
}

/// Every key the shell, Studio, built-in plugins and download command use.
pub(crate) fn source_keys() -> BTreeSet<String> {
    let mut files = Vec::new();
    rust_files(&workspace().join("crates"), &mut files);
    files.push(workspace().join("xtask/src/bootstrap.rs"));
    let mut keys = BTreeSet::new();
    for f in files {
        if f.starts_with(workspace().join("crates/mandara-i18n")) {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&f) {
            keys.extend(keys_in(&text));
        }
    }
    keys
}

#[test]
fn scanner_reads_literals() {
    let src = r#"a(t("Hello")); tf("{n} items", &[]); format(t("Line\nTwo")); not_t("x"); tk("Mark")"#;
    assert_eq!(keys_in(src), vec!["Hello".to_string(), "Line\nTwo".to_string(), "{n} items".to_string(), "Mark".to_string()]);
}

#[test]
fn scoped_languages_nest_and_end() {
    assert_eq!(t_in(Lang::Ar, "Settings"), "الإعدادات");
    assert_eq!(t_in(Lang::En, "Settings"), "Settings");
    {
        let _ar = scoped(Some(Lang::Ar));
        assert_eq!(language(), Lang::Ar);
        assert_eq!(t("Settings"), "الإعدادات");
        // `None` keeps the override around it.
        with_language(None, || assert_eq!(language(), Lang::Ar));
        with_language(Some(Lang::De), || assert_eq!(language(), Lang::De));
        assert_eq!(language(), Lang::Ar);
        // Other threads keep the shell's language.
        assert_eq!(std::thread::spawn(|| language() == shell_language()).join().ok(), Some(true));
    }
    assert_eq!(language(), shell_language());
}

#[test]
fn language_codes() {
    assert_eq!(Lang::from_code("ar_EG.UTF-8"), Some(Lang::Ar));
    assert_eq!(Lang::from_code("zh-CN"), Some(Lang::Zh));
    assert_eq!(Lang::from_code("pt_BR"), None);
    assert!(Lang::Ar.is_rtl() && !Lang::Ja.is_rtl());
    assert_eq!(placeholders("{a} and {b} {c d}"), vec!["a", "b"]);
}

#[test]
fn persian_korean_urdu_and_bengali() {
    // The desktop's locale picks them; Persian and Urdu read right to left.
    for (env, lang, rtl, posix) in [("fa_IR.UTF-8", Lang::Fa, true, "fa_IR"), ("ko_KR.UTF-8", Lang::Ko, false, "ko_KR"), ("ur_PK.UTF-8", Lang::Ur, true, "ur_PK"), ("bn_BD.UTF-8", Lang::Bn, false, "bn_BD"), ("bn_IN", Lang::Bn, false, "bn_BD"), ("ur-IN", Lang::Ur, true, "ur_PK")] {
        assert_eq!(Lang::from_code(env), Some(lang), "{env}");
        assert_eq!(lang.is_rtl(), rtl, "{env}");
        assert_eq!(lang.posix(), posix);
    }
    assert_eq!([Lang::Fa, Lang::Ko, Lang::Ur, Lang::Bn].map(Lang::native_name), ["فارسی", "한국어", "اردو", "বাংলা"]);
    // Every language has its own catalog slot, code and sample.
    let mut codes: Vec<&str> = Lang::ALL.iter().map(|l| l.code()).collect();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), Lang::ALL.len());
    for lang in Lang::ALL {
        assert!((lang as usize) < Lang::ALL.len() && !lang.sample().is_empty(), "{lang:?}");
    }
}

/// `lang`'s source catalog (`locales/<code>.toml`).
fn source_catalog(lang: Lang) -> Catalog {
    let path = packs::source_catalog(lang);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    Catalog::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", lang.code()))
}

#[test]
fn catalogs_parse_and_keep_placeholders() {
    for lang in Lang::ALL.into_iter().filter(|l| *l != Lang::En) {
        let c = source_catalog(lang);
        for k in c.keys() {
            let v = c.get(k).unwrap();
            assert_eq!(placeholders(k), placeholders(v), "{}: placeholders differ in {k:?} → {v:?}", lang.code());
        }
    }
}

#[test]
fn every_key_is_translated() {
    let keys = source_keys();
    if std::env::var_os("MANDARA_I18N_DUMP").is_some() {
        let list: Vec<&str> = keys.iter().map(String::as_str).collect();
        std::fs::write(Path::new(env!("CARGO_MANIFEST_DIR")).join("locales/keys.txt"), list.join("\n") + "\n").unwrap();
    }
    let mut report = Vec::new();
    for lang in Lang::ALL.into_iter().filter(|l| *l != Lang::En) {
        let c = source_catalog(lang);
        let missing: Vec<&String> = keys.iter().filter(|k| c.get(k).is_none()).collect();
        if !missing.is_empty() {
            report.push(format!("{}: {} of {} missing, e.g. {:?}", lang.code(), missing.len(), keys.len(), &missing[..missing.len().min(5)]));
        }
    }
    assert!(report.is_empty(), "untranslated strings:\n{}", report.join("\n"));
}

#[test]
fn fill_replaces_known_placeholders_in_one_pass() {
    use super::fill;
    assert_eq!(fill("{a} and {b}, {a}!", &[("a", &1), ("b", &"two")]), "1 and two, 1!");
    // Unknown names, stray and unclosed braces are copied verbatim.
    assert_eq!(fill("{x} {a} { {", &[("a", &"ok")]), "{x} ok { {");
    // A value containing braces is not re-expanded.
    assert_eq!(fill("{a}{b}", &[("a", &"{b}"), ("b", &"B")]), "{b}B");
}

#[test]
fn every_catalog_compiles_into_a_pack_that_answers_like_it() {
    for lang in Lang::ALL.into_iter().filter(|l| *l != Lang::En) {
        let c = source_catalog(lang);
        let bytes = pack::build(lang, "9.9.9", c.iter()).unwrap_or_else(|e| panic!("{}: {e}", lang.code()));
        let p = pack::Pack::parse(&bytes, "9.9.9", lang).unwrap();
        assert_eq!(p.len(), c.len(), "{}", lang.code());
        for (k, v) in c.iter() {
            assert_eq!(p.get(k), Some(v), "{}: {k:?}", lang.code());
        }
    }
}

#[test]
fn lookups_and_untranslate_go_through_the_pack() {
    // Debug builds compile the source catalog when no pack is installed.
    assert!(packs::is_available(Lang::Ar));
    assert_eq!(lookup(Lang::Ar, "Settings"), Some("الإعدادات"));
    assert_eq!(lookup(Lang::En, "Settings"), None);
    assert_eq!(tr_in(Lang::Ar, "Not a key at all"), "Not a key at all");
    // Several keys may share a translation; any of them translates back.
    with_language(Some(Lang::Ar), || assert_eq!(tr_in(Lang::Ar, &untranslate("الإعدادات")), "الإعدادات"));
    with_language(Some(Lang::Ar), || assert_eq!(untranslate("unknown"), "unknown"));
}
