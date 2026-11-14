//! Explicit chat requests to an existing server. Mandara never starts an inference engine.

mod protocol;
mod stream;

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Duration;

use mandara_i18n::t;
use serde::{Deserialize, Serialize};

use crate::credentials::{Key, Provider as CredentialProvider, Secret};
use crate::web_api::{Authorization, Client, Transport as _, endpoint};

/// A bounded transcript stays small even when a server ignores its token limit.
pub const MAX_REPLY_BYTES: usize = 8 * 1024;
/// A pasted prompt leaves room for previous turns inside the request budget.
pub const MAX_PROMPT_BYTES: usize = 8 * 1024;
/// Retain whole turns, never an ever-growing conversation.
pub const MAX_MESSAGES: usize = 16;
pub const DEFAULT_CONTEXT_BYTES: usize = 16 * 1024;
pub const CONTEXT_RANGE: std::ops::RangeInclusive<usize> = 16 * 1024..=32 * 1024;
pub const DEFAULT_REPLY_TOKENS: u32 = 1024;
pub const TOKEN_RANGE: std::ops::RangeInclusive<u32> = 128..=4096;
/// Local models may need to load, but no request runs indefinitely.
const CHAT_TIMEOUT: Duration = Duration::from_secs(300);
/// DNS and TCP establishment remain bounded if cancellation arrives before a socket exists.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
/// Model catalogs are labels, not files or complete model metadata.
pub const MAX_MODELS: usize = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Provider {
    #[default]
    Ollama,
    LlamaCpp,
    Jan,
    LmStudio,
    AnythingLlm,
    OpenAi,
}

impl Provider {
    pub fn id(self) -> &'static str {
        match self {
            Self::Ollama => "ollama",
            Self::LlamaCpp => "llama-cpp",
            Self::Jan => "jan",
            Self::LmStudio => "lm-studio",
            Self::AnythingLlm => "anything-llm",
            Self::OpenAi => "open-ai",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Ollama => "Ollama",
            Self::LlamaCpp => "llama.cpp",
            Self::Jan => "Jan",
            Self::LmStudio => "LM Studio",
            Self::AnythingLlm => "AnythingLLM",
            Self::OpenAi => "OpenAI-compatible",
        }
    }
    pub fn server(self) -> &'static str {
        match self {
            Self::Ollama => super::DEFAULT_URL,
            Self::LlamaCpp | Self::OpenAi => "http://127.0.0.1:8080/v1",
            Self::Jan => "http://127.0.0.1:1337/v1",
            Self::LmStudio => "http://127.0.0.1:1234/v1",
            Self::AnythingLlm => "http://127.0.0.1:3001/api",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub provider: Provider,
    pub server: String,
    pub model: String,
    pub use_api_key: bool,
    pub context_bytes: usize,
    pub reply_tokens: u32,
}

impl Settings {
    pub fn server(&self) -> Result<String, String> {
        endpoint(
            if self.server.trim().is_empty() {
                self.provider.server()
            } else {
                &self.server
            },
            "",
        )
        .map(|server| server.trim_end_matches('/').into())
    }
    pub fn credential_key(&self) -> Result<Option<Key>, String> {
        if !self.use_api_key && self.provider != Provider::AnythingLlm {
            return Ok(None);
        }
        Ok(Some(Key {
            provider: CredentialProvider::LocalAi,
            server: self.server()?,
        }))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Message {
    pub role: Role,
    pub content: Arc<str>,
}

/// Remove complete oldest turns; the newest prompt is always kept intact.
pub fn trim(messages: &mut Vec<Message>, byte_limit: usize) {
    let byte_limit = byte_limit.clamp(*CONTEXT_RANGE.start(), *CONTEXT_RANGE.end());
    let mut bytes = messages
        .iter()
        .map(|message| message.content.len())
        .sum::<usize>();
    while messages.len() > MAX_MESSAGES || (bytes > byte_limit && messages.len() > 2) {
        let remove = messages.len().min(2);
        for message in messages.drain(..remove) {
            bytes -= message.content.len();
        }
    }
    messages.shrink_to(MAX_MESSAGES);
}

/// One request owns its connection pool. Cancelling closes an active read, including TLS.
#[derive(Clone)]
pub struct Session {
    agent: ureq::Agent,
    cancelled: Arc<AtomicBool>,
}

impl Default for Session {
    fn default() -> Self {
        let agent = ureq::Agent::config_builder()
            .timeout_global(Some(CHAT_TIMEOUT))
            .timeout_connect(Some(CONNECT_TIMEOUT))
            .timeout_resolve(Some(CONNECT_TIMEOUT))
            .max_redirects(0)
            .max_idle_connections(0)
            .http_status_as_error(false)
            .build()
            .into();
        Self {
            agent,
            cancelled: Arc::default(),
        }
    }
}

impl Session {
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.agent.clone().shutdown();
    }
    pub fn cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    /// Blocking streaming work belongs on the shared blocking executor. The sink
    /// receives borrowed chunks and can stop without allocating a queue of replies.
    pub fn chat(
        &self,
        settings: &Settings,
        messages: &[Message],
        session_id: &str,
        key: Option<&Secret>,
        mut sink: impl FnMut(&str) -> bool,
    ) -> Result<(), String> {
        crate::offline::check()?;
        if self.cancelled() {
            return Ok(());
        }
        if settings.model.is_empty() || settings.model.len() > MAX_PROMPT_BYTES {
            return Err(t("Choose a model or workspace in Studio").into());
        }
        let prompt = messages
            .last()
            .filter(|message| message.role == Role::User)
            .ok_or_else(|| t("Enter a message").to_string())?;
        if prompt.content.trim().is_empty() || prompt.content.len() > MAX_PROMPT_BYTES {
            return Err(t("Keep a message under 8 KiB").into());
        }
        let protocol = protocol::for_provider(settings.provider);
        let (path, body) = protocol.request(settings, messages, session_id)?;
        let request = Client::request(
            ureq::http::Method::POST,
            &endpoint(&settings.server()?, &path)?,
            Some(&body),
            key.map(Authorization::Bearer),
        )?;
        let response = self.agent.run(request);
        if self.cancelled() {
            return Ok(());
        }
        let mut response = response.map_err(|_| t("Could not reach the AI server").to_string())?;
        Client::check_status(response.status())?;
        stream::read(
            response.body_mut().as_reader(),
            protocol,
            || self.cancelled(),
            &mut sink,
        )
    }
}

pub fn catalog(settings: &Settings, key: Option<&Secret>) -> Result<Vec<String>, String> {
    let protocol = protocol::for_provider(settings.provider);
    let value = Client::default()
        .json(
            ureq::http::Method::GET,
            &endpoint(&settings.server()?, protocol.catalog_path())?,
            None,
            key.map(Authorization::Bearer),
        )
        .map_err(String::from)?;
    protocol.models(value)
}

/// AnythingLLM partitions its server-side history by this fresh, unguessable identity.
pub fn session_id() -> Result<String, String> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| error.to_string())?;
    use std::fmt::Write as _;
    let mut id = String::from("mandara-");
    for byte in bytes {
        write!(id, "{byte:02x}").expect("writing to a String");
    }
    Ok(id)
}

#[cfg(test)]
mod tests;
