//! Shell and login translations share a pack while keeping their own wording.

use std::path::Path;

use crate::{Catalog, Lang, pack};

/// Reserved pack key prefix for login-specific translations.
pub const GREETER_PREFIX: &str = "greeter:";

/// Compile both catalogs without replacing shared translations with login wording.
pub fn compile(lang: Lang, version: &str, shared: &str, greeter: &str) -> Result<Vec<u8>, String> {
    let shared = Catalog::parse(shared)?;
    let greeter = Catalog::parse(greeter)?;
    let overrides: Vec<_> = greeter.iter().map(|(key, value)| (format!("{GREETER_PREFIX}{key}"), value)).collect();
    pack::build(lang, version, shared.iter().chain(overrides.iter().map(|(key, value)| (key.as_str(), *value))))
}

/// Read the two source catalogs used by release tooling and source installs.
pub fn compile_files(lang: Lang, version: &str, shared_dir: &Path, greeter_dir: &Path) -> Result<Vec<u8>, String> {
    let read = |dir: &Path| {
        let path = dir.join(format!("{}.toml", lang.code()));
        std::fs::read_to_string(&path).map_err(|error| format!("{}: {error}", path.display()))
    };
    compile(lang, version, &read(shared_dir)?, &read(greeter_dir)?)
}

/// Login overrides are read directly from the already loaded language map.
pub fn greeter(lang: Lang, key: &str) -> Option<&'static str> {
    crate::packs::pack(lang)?.get_prefixed(GREETER_PREFIX, key)
}

/// Reverse shell lookups must never return a login-only prefixed key.
pub(crate) fn shared_key<'a>(pack: &pack::Pack<'a>, value: &str) -> Option<&'a str> {
    pack.iter().find(|(key, translation)| !key.starts_with(GREETER_PREFIX) && *translation == value).map(|(key, _)| key)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_compilation_requires_both_catalogs() {
        let root = tempfile::tempdir().unwrap();
        let shared = root.path().join("shared");
        let greeter = root.path().join("greeter");
        std::fs::create_dir_all(&shared).unwrap();
        std::fs::create_dir_all(&greeter).unwrap();
        std::fs::write(shared.join("pt.toml"), "\"Settings\" = \"Configurações\"").unwrap();
        assert!(compile_files(Lang::Pt, "1", &shared, &greeter).is_err());
        std::fs::write(greeter.join("pt.toml"), "\"Username\" = \"Nome de usuário\"").unwrap();
        let bytes = compile_files(Lang::Pt, "1", &shared, &greeter).unwrap();
        let pack = pack::Pack::parse(&bytes, "1", Lang::Pt).unwrap();
        assert_eq!(pack.get("Settings"), Some("Configurações"));
        assert_eq!(pack.get_prefixed(GREETER_PREFIX, "Username"), Some("Nome de usuário"));
    }

    #[test]
    fn shared_and_login_wording_survive_in_one_pack() {
        let bytes = compile(Lang::Ar, "1", "\"Anchor\" = \"نقطة التثبيت\"", "\"Anchor\" = \"نقطة الارتكاز\"\n\"Username\" = \"اسم المستخدم\"").unwrap();
        let pack = pack::Pack::parse(&bytes, "1", Lang::Ar).unwrap();
        assert_eq!(pack.get("Anchor"), Some("نقطة التثبيت"));
        assert_eq!(pack.get_prefixed(GREETER_PREFIX, "Anchor"), Some("نقطة الارتكاز"));
        assert_eq!(pack.get_prefixed(GREETER_PREFIX, "Username"), Some("اسم المستخدم"));
        assert_eq!(pack.get("Username"), None);
        assert_eq!(shared_key(&pack, "نقطة التثبيت"), Some("Anchor"));
        assert_eq!(shared_key(&pack, "نقطة الارتكاز"), None);
    }
}
