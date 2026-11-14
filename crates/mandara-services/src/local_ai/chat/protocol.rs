//! Three wire formats cover the server presets without duplicating HTTP handling.

use super::{MAX_MODELS, Message, Provider, Settings, TOKEN_RANGE};
use mandara_i18n::t;
use serde_json::{Value, json};

pub(super) trait Protocol: Sync {
    fn catalog_path(&self) -> &'static str;
    fn models(&self, value: Value) -> Result<Vec<String>, String>;
    fn request(
        &self,
        settings: &Settings,
        messages: &[Message],
        session: &str,
    ) -> Result<(String, Value), String>;
    fn event<'a>(&self, value: &'a Value) -> Result<Chunk<'a>, String>;
    fn sse(&self) -> bool {
        true
    }
}

pub(super) struct Chunk<'a> {
    pub text: &'a str,
    pub done: bool,
}
struct Ollama;
struct OpenAi;
struct Workspace;

pub(super) fn for_provider(provider: Provider) -> &'static dyn Protocol {
    match provider {
        Provider::Ollama => &Ollama,
        Provider::AnythingLlm => &Workspace,
        _ => &OpenAi,
    }
}

fn models(value: Value, array: &str, name: &str) -> Result<Vec<String>, String> {
    let items = value
        .get(array)
        .and_then(Value::as_array)
        .ok_or_else(|| t("The AI server returned an invalid model list").to_string())?;
    if items.len() > MAX_MODELS {
        return Err(t("The AI server returned too many models").into());
    }
    let mut names: Vec<String> = items
        .iter()
        .filter_map(|item| item.get(name).and_then(Value::as_str))
        .filter(|name| !name.is_empty() && name.len() <= super::MAX_PROMPT_BYTES)
        .map(str::to_owned)
        .collect();
    names.sort();
    names.dedup();
    Ok(names)
}

fn error(value: &Value) -> Result<(), String> {
    if value
        .get("error")
        .is_some_and(|error| !error.is_null() && error != "" && error != false)
        || value.get("type").is_some_and(|kind| kind == "abort")
    {
        // Error bodies can contain credentials, prompts or arbitrary server details.
        return Err(t("The AI server could not complete this reply").into());
    }
    Ok(())
}

impl Protocol for Ollama {
    fn catalog_path(&self) -> &'static str {
        "api/tags"
    }
    fn models(&self, value: Value) -> Result<Vec<String>, String> {
        models(value, "models", "name")
    }
    fn request(
        &self,
        settings: &Settings,
        messages: &[Message],
        _: &str,
    ) -> Result<(String, Value), String> {
        Ok((
            "api/chat".into(),
            json!({"model":settings.model, "messages":messages, "stream":true,
            "options":{"num_predict":settings.reply_tokens.clamp(*TOKEN_RANGE.start(), *TOKEN_RANGE.end())}}),
        ))
    }
    fn event<'a>(&self, value: &'a Value) -> Result<Chunk<'a>, String> {
        error(value)?;
        Ok(Chunk {
            text: value
                .pointer("/message/content")
                .and_then(Value::as_str)
                .unwrap_or(""),
            done: value.get("done").and_then(Value::as_bool).unwrap_or(false),
        })
    }
    fn sse(&self) -> bool {
        false
    }
}

impl Protocol for OpenAi {
    fn catalog_path(&self) -> &'static str {
        "models"
    }
    fn models(&self, value: Value) -> Result<Vec<String>, String> {
        models(value, "data", "id")
    }
    fn request(
        &self,
        settings: &Settings,
        messages: &[Message],
        _: &str,
    ) -> Result<(String, Value), String> {
        Ok((
            "chat/completions".into(),
            json!({"model":settings.model,"messages":messages,"stream":true,
            "max_tokens":settings.reply_tokens.clamp(*TOKEN_RANGE.start(), *TOKEN_RANGE.end())}),
        ))
    }
    fn event<'a>(&self, value: &'a Value) -> Result<Chunk<'a>, String> {
        error(value)?;
        Ok(Chunk {
            text: value
                .pointer("/choices/0/delta/content")
                .and_then(Value::as_str)
                .unwrap_or(""),
            done: value
                .pointer("/choices/0/finish_reason")
                .is_some_and(|reason| !reason.is_null()),
        })
    }
}

impl Protocol for Workspace {
    fn catalog_path(&self) -> &'static str {
        "v1/workspaces"
    }
    fn models(&self, value: Value) -> Result<Vec<String>, String> {
        models(value, "workspaces", "slug")
    }
    fn request(
        &self,
        settings: &Settings,
        messages: &[Message],
        session: &str,
    ) -> Result<(String, Value), String> {
        if !settings
            .model
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(t("Choose a valid AnythingLLM workspace slug").into());
        }
        let message = messages
            .last()
            .ok_or_else(|| t("Enter a message").to_string())?;
        Ok((
            format!("v1/workspace/{}/stream-chat", settings.model),
            json!({"message":message.content,"mode":"chat","sessionId":session}),
        ))
    }
    fn event<'a>(&self, value: &'a Value) -> Result<Chunk<'a>, String> {
        error(value)?;
        Ok(Chunk {
            text: value
                .get("textResponse")
                .and_then(Value::as_str)
                .unwrap_or(""),
            done: value.get("close").and_then(Value::as_bool).unwrap_or(false),
        })
    }
}
