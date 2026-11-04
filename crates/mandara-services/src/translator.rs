//! Explicit text translation through the selected provider. Nothing is
//! read from the clipboard or sent while the user is still typing.

mod languages;

use mandara_i18n::t;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use ureq::http::Method;

use crate::credentials::{self, Secret};
use crate::web_api::{Authorization, Client, Transport, endpoint};

pub use languages::Language;

/// A quick translation fits in a panel and leaves headroom for JSON
/// escaping within the shared request limit.
pub const MAX_TEXT_CHARS: usize = 5000;
const FREE_SERVER: &str = "https://api-free.deepl.com";
const PRO_SERVER: &str = "https://api.deepl.com";

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provider { #[default] DeepLFree, DeepLPro, LibreTranslate }

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub provider: Provider,
    pub server: String,
}

impl Settings {
    pub fn server(&self) -> Result<String, String> {
        match self.provider {
            Provider::DeepLFree => Ok(FREE_SERVER.into()),
            Provider::DeepLPro => Ok(PRO_SERVER.into()),
            Provider::LibreTranslate => endpoint(&self.server, "").map(|url| url.trim_end_matches('/').into()),
        }
    }

    pub fn credential_key(&self) -> Result<credentials::Key, String> {
        Ok(credentials::Key {
            provider: if self.provider == Provider::LibreTranslate { credentials::Provider::LibreTranslate } else { credentials::Provider::DeepL },
            server: self.server()?,
        })
    }
}

pub struct Request<'a> {
    pub text: &'a str,
    /// `None` asks the provider to detect the source language.
    pub source: Option<&'a str>,
    pub target: &'a str,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Translation {
    pub text: String,
    pub source: Option<String>,
    pub target: String,
}

pub trait Backend {
    fn languages(&self, settings: &Settings, key: Option<&Secret>) -> Result<Vec<Language>, String>;
    fn translate(&self, settings: &Settings, request: &Request<'_>, key: Option<&Secret>) -> Result<Translation, String>;
}

#[derive(Default)]
pub struct Http<T = Client> { transport: T }

impl Http<Client> {
    pub fn new() -> Self { Self::default() }
}

pub(super) fn language_code(code: &str) -> bool {
    /// ISO language codes and their regional/script subtags fit here.
    const MAX_CODE_BYTES: usize = 16;
    !code.is_empty() && code.len() <= MAX_CODE_BYTES
        && code.split('-').all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric()))
        && code.bytes().next().is_some_and(|byte| byte.is_ascii_alphabetic())
}

fn required_key(key: Option<&Secret>) -> Result<&Secret, String> {
    key.ok_or_else(|| t("Add your translation API key in Studio").to_string())
}

impl<T: Transport> Backend for Http<T> {
    fn languages(&self, settings: &Settings, key: Option<&Secret>) -> Result<Vec<Language>, String> {
        languages::fetch(&self.transport, settings, key)
    }

    fn translate(&self, settings: &Settings, request: &Request<'_>, key: Option<&Secret>) -> Result<Translation, String> {
        if request.text.trim().is_empty() { return Err(t("Enter some text to translate").into()); }
        if request.text.chars().count() > MAX_TEXT_CHARS { return Err(t("Keep a translation under 5,000 characters").into()); }
        if !language_code(request.target) || request.target.eq_ignore_ascii_case("auto") || request.source.is_some_and(|source| !language_code(source)) {
            return Err(t("Choose a source and target language").into());
        }
        let server = settings.server()?;
        let value = match settings.provider {
            Provider::DeepLFree | Provider::DeepLPro => {
                let mut body = json!({"text": [request.text], "target_lang": request.target});
                if let Some(source) = request.source { body["source_lang"] = source.into(); }
                self.transport.json(Method::POST, &endpoint(&server, "v2/translate")?, Some(&body), Some(Authorization::DeepL(required_key(key)?)))?
            }
            Provider::LibreTranslate => {
                let mut body = json!({"q": request.text, "source": request.source.unwrap_or("auto"), "target": request.target, "format": "text"});
                if let Some(key) = key { body["api_key"] = key.expose().into(); }
                self.transport.json(Method::POST, &endpoint(&server, "translate")?, Some(&body), None)?
            }
        };
        parse(settings.provider, request.target, value)
    }
}

fn parse(provider: Provider, target: &str, value: Value) -> Result<Translation, String> {
    let (text, source) = match provider {
        Provider::DeepLFree | Provider::DeepLPro => {
            let item = value.get("translations").and_then(Value::as_array).filter(|items| items.len() == 1).and_then(|items| items.first());
            (item.and_then(|item| item.get("text")).and_then(Value::as_str), item.and_then(|item| item.get("detected_source_language")).and_then(Value::as_str))
        }
        Provider::LibreTranslate => (value.get("translatedText").and_then(Value::as_str), value.pointer("/detectedLanguage/language").and_then(Value::as_str)),
    };
    let text = text.filter(|text| !text.trim().is_empty()).ok_or_else(|| t("The provider returned no translation").to_string())?;
    Ok(Translation { text: text.into(), source: source.filter(|source| language_code(source)).map(str::to_owned), target: target.into() })
}

#[cfg(test)]
mod tests;
