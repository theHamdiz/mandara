//! Opt-in local command inference; existing installations keep ordinary search.
use serde::{Deserialize, Serialize};

/// Debounce bounds keep fast typing from repeatedly loading model weights.
pub const DEBOUNCE_RANGE_MS: (u64, u64) = (100, 1000);
/// Default pause before compiling an edited query.
pub const DEBOUNCE_MS: u64 = 180;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AgentEngine {
    #[default]
    FunctionGemma,
    Qwen,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    pub enabled: bool,
    pub engine: AgentEngine,
    pub debounce_ms: u64,
}
impl Default for AgentConfig {
    fn default() -> Self {
        Self { enabled: false, engine: AgentEngine::default(), debounce_ms: DEBOUNCE_MS }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn old_config_never_enables_or_downloads_an_agent() {
        let old: crate::ShellConfig = toml::from_str("").unwrap();
        assert!(!old.launcher.agent.enabled);
        assert_eq!(old.launcher.agent.debounce_ms, DEBOUNCE_MS);
        let config: AgentConfig = toml::from_str("enabled = true\nengine = 'qwen'").unwrap();
        assert_eq!(config.engine, AgentEngine::Qwen);
    }
}
