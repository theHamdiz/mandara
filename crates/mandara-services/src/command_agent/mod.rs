//! Offline command retrieval and compilation, owned by a short-lived worker.
//!
//! Normal launcher searches never call this module. Model allocations live in
//! a child invocation of the same Rust binary, so cancellation and completion
//! release the entire inference runtime, including dependency thread pools.

pub mod catalog;
mod compiler;
mod embeddings;
mod gemma;
mod mapped;
pub mod models;
mod native_call;
mod qwen;
mod selection;
mod worker;
pub use worker::{Job, active_requests, purge, run, worker_main};

use catalog::{Action, Command};
use serde::{Deserialize, Serialize};

/// Limit untrusted input and the compiler context before tokenization.
pub const MAX_QUERY_BYTES: usize = 2048;
/// The lifetime ceiling after the most recent query, including cold loading.
pub const IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// Explicit setup can build the whole catalog before interactive inference.
pub const INDEX_SETUP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// What a launcher query starts with to go to the agent (not configurable;
/// the launcher's prefix help shows it while the agent is on).
pub const PREFIX: char = '!';

/// The strict trigger accepts either `!command` or `! command`.
pub fn query(input: &str) -> Option<&str> {
    input.trim_start().strip_prefix(PREFIX).map(str::trim)
}

#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Engine {
    #[default]
    FunctionGemma,
    Qwen,
}

#[derive(Serialize, Deserialize)]
pub struct Request {
    pub query: String,
    pub catalog: Vec<Command>,
    pub engine: Engine,
    #[serde(default)]
    pub prepare_index: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preferred_command: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Match {
    pub command: Command,
    pub score: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "kebab-case")]
pub enum Event {
    Matches { matches: Vec<Match>, retrieval_ms: f64 },
    Ready { actions: Vec<Action>, words: Vec<Vec<String>>, inference_ms: f64 },
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_the_leading_bang_routes_to_the_agent() {
        for text in ["!set gaps 8", "! set gaps 8", "  ! set gaps 8  "] {
            assert_eq!(query(text), Some("set gaps 8"));
        }
        assert_eq!(query("!"), Some(""));
        for text in ["firefox", "find !important", "/theme dark", ""] {
            assert_eq!(query(text), None);
        }
    }
}
