use std::cell::RefCell;

use super::*;

#[derive(Default)]
struct Fixture {
    calls: RefCell<Vec<(Method, String, Value, Option<String>)>>,
    response: Value,
}

impl Transport for Fixture {
    fn json(&self, method: Method, url: &str, body: Option<&Value>, auth: Option<Authorization<'_>>) -> Result<Value, String> {
        let key = auth.map(|auth| match auth { Authorization::DeepL(key) | Authorization::Bearer(key) => key.expose().to_owned() });
        self.calls.borrow_mut().push((method, url.into(), body.cloned().unwrap_or(Value::Null), key));
        Ok(self.response.clone())
    }
}

#[test]
fn deepl_uses_its_official_endpoint_header_and_auto_detection() {
    let backend = Http { transport: Fixture { response: json!({"translations":[{"text":"مرحبا","detected_source_language":"EN"}]}), ..Default::default() } };
    let settings = Settings { provider: Provider::DeepLFree, server: "http://unrelated.example".into() };
    let key = Secret::new("fixture:fx".into()).unwrap();
    let result = backend.translate(&settings, &Request { text: "Hello", source: None, target: "AR" }, Some(&key)).unwrap();
    assert_eq!(result, Translation { text: "مرحبا".into(), source: Some("EN".into()), target: "AR".into() });
    let calls = backend.transport.calls.borrow();
    assert_eq!(calls.as_slice(), [(Method::POST, "https://api-free.deepl.com/v2/translate".into(), json!({"text":["Hello"],"target_lang":"AR"}), Some("fixture:fx".into()))]);
    assert_eq!(settings.credential_key().unwrap().server, FREE_SERVER);
    assert_ne!(settings.credential_key().unwrap(), Settings { provider: Provider::DeepLPro, ..settings }.credential_key().unwrap());
}

#[test]
fn a_self_hosted_translation_keeps_its_proxy_path_and_needs_no_key() {
    let backend = Http { transport: Fixture { response: json!({"translatedText":"Hola","detectedLanguage":{"language":"en","confidence":99.0}}), ..Default::default() } };
    let settings = Settings { provider: Provider::LibreTranslate, server: "http://localhost:5000/translator/".into() };
    assert_eq!(backend.translate(&settings, &Request { text: "Hello", source: None, target: "es" }, None).unwrap().text, "Hola");
    let calls = backend.transport.calls.borrow();
    assert_eq!(calls[0], (Method::POST, "http://localhost:5000/translator/translate".into(), json!({"q":"Hello","source":"auto","target":"es","format":"text"}), None));
}

#[test]
fn invalid_input_and_missing_credentials_never_send_text() {
    let backend = Http { transport: Fixture::default() };
    let settings = Settings::default();
    for (text, target) in [("", "de"), ("Hello", "auto"), ("Hello", "../de"), ("Hello", "")] {
        assert!(backend.translate(&settings, &Request { text, source: None, target }, None).is_err());
    }
    assert!(backend.translate(&settings, &Request { text: &"界".repeat(MAX_TEXT_CHARS + 1), source: None, target: "de" }, None).is_err());
    assert!(backend.translate(&settings, &Request { text: "Hello", source: None, target: "de" }, None).is_err());
    assert!(backend.transport.calls.borrow().is_empty());
}

#[test]
fn provider_errors_and_empty_translations_do_not_look_like_success() {
    for value in [Value::Null, json!({"error":"bad target"}), json!({"translations":[]}), json!({"translations":[{"text":""}]}), json!({"translations":[{"text":"a"},{"text":"b"}]})] {
        assert!(parse(Provider::DeepLPro, "de", value).is_err());
    }
    assert!(parse(Provider::LibreTranslate, "de", json!({"translatedText":["unexpected batch"]})).is_err());
}

#[test]
fn current_language_catalog_keeps_source_and_target_variants_separate() {
    let backend = Http { transport: Fixture { response: json!([
        {"lang":"en","name":"English","usable_as_source":true,"usable_as_target":false,"status":"stable"},
        {"lang":"en-US","name":"English (American)","usable_as_source":false,"usable_as_target":true,"status":"stable"},
        {"lang":"de","name":"German","usable_as_source":true,"usable_as_target":true,"status":"stable"},
        {"lang":"DE","name":"Duplicate","usable_as_source":true,"usable_as_target":true},
        {"lang":"../x","name":"Invalid","usable_as_source":true}
    ]), ..Default::default() } };
    let key = Secret::new("fixture-key".into()).unwrap();
    let languages = backend.languages(&Settings::default(), Some(&key)).unwrap();
    assert_eq!(languages.len(), 3);
    assert!(languages.iter().any(|language| language.code == "en-US" && language.target && !language.source));
    assert!(languages.iter().any(|language| language.code == "en" && language.source && !language.target));
    assert!(backend.transport.calls.borrow()[0].1.ends_with("/v3/languages?resource=translate_text"));
    let libre = languages::parse(false, json!([{"code":"en","name":"English","targets":["es","de"]}])).unwrap();
    assert_eq!(libre[0].targets, ["es", "de"]);
    assert!(languages::parse(false, json!([{"code":"en","name":"界".repeat(129)}])).is_err());
}
