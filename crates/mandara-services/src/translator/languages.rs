use mandara_i18n::t;
use serde::Serialize;
use serde_json::Value;
use ureq::http::Method;

use super::{Provider, Settings, language_code, required_key};
use crate::credentials::Secret;
use crate::web_api::{Authorization, Transport, endpoint};

/// Both language catalogs are small; bound a misconfigured server before
/// building selectors in the panel.
const MAX_LANGUAGES: usize = 512;
/// Display names must remain small enough for a native picker row.
const MAX_NAME_CHARS: usize = 128;

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Language {
    pub code: String,
    pub name: String,
    pub source: bool,
    pub target: bool,
    /// LibreTranslate can advertise only some target pairs per source.
    pub targets: Vec<String>,
}

pub(super) fn fetch(transport: &impl Transport, settings: &Settings, key: Option<&Secret>) -> Result<Vec<Language>, String> {
    let server = settings.server()?;
    let deepl = settings.provider != Provider::LibreTranslate;
    let (path, auth) = if deepl { ("v3/languages?resource=translate_text", Some(Authorization::DeepL(required_key(key)?))) } else { ("languages", None) };
    let value = transport.json(Method::GET, &endpoint(&server, path)?, None, auth)?;
    parse(deepl, value)
}

pub(super) fn parse(deepl: bool, value: Value) -> Result<Vec<Language>, String> {
    let values = value.as_array().filter(|values| !values.is_empty() && values.len() <= MAX_LANGUAGES)
        .ok_or_else(|| t("The provider returned no language list").to_string())?;
    let mut seen = std::collections::HashSet::new();
    let mut languages: Vec<_> = values.iter().filter_map(|value| {
        let code = value.get(if deepl { "lang" } else { "code" })?.as_str()?;
        let name = value.get("name")?.as_str()?.trim();
        if !language_code(code) || name.is_empty() || name.chars().count() > MAX_NAME_CHARS || !seen.insert(code.to_ascii_lowercase()) { return None; }
        Some(Language {
            code: code.into(), name: name.into(),
            source: !deepl || value.get("usable_as_source").and_then(Value::as_bool).unwrap_or(false),
            target: !deepl || value.get("usable_as_target").and_then(Value::as_bool).unwrap_or(false),
            targets: value.get("targets").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str)
                .filter(|code| language_code(code)).take(MAX_LANGUAGES).map(str::to_owned).collect(),
        })
    }).filter(|language| language.source || language.target).collect();
    languages.sort_by(|a, b| a.name.cmp(&b.name));
    if languages.is_empty() { return Err(t("The provider returned no language list").into()); }
    Ok(languages)
}
