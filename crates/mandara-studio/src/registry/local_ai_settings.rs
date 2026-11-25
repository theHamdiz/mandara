//! Connection presets keep provider defaults separate from a custom server address.
use super::{Control, Section, Setting, sd};
use mandara_i18n::tk;
use mandara_services::local_ai::chat as ai;
use serde_json::Value;

pub(super) fn entries() -> Vec<Setting> {
    vec![
        sd(
            Section::Plugins,
            tk("Local AI"),
            "/plugins/settings/local-ai/provider",
            tk("Provider"),
            tk("Connect to a running local AI server"),
            Control::Choice(
                const {
                    &[
                        ("ollama", tk("Ollama")),
                        ("llama-cpp", "llama.cpp"),
                        ("jan", "Jan"),
                        ("lm-studio", "LM Studio"),
                        ("anything-llm", "AnythingLLM"),
                        ("open-ai", tk("OpenAI-compatible")),
                    ]
                },
            ),
            Value::from("ollama"),
        ),
        sd(
            Section::Plugins,
            tk("Local AI"),
            "/plugins/settings/local-ai/server",
            tk("Server address"),
            tk("Empty uses the provider's local address; an existing Ollama address is preserved"),
            Control::Text {
                placeholder: "http://127.0.0.1:8080/v1",
            },
            Value::from(""),
        ),
        sd(
            Section::Plugins,
            tk("Local AI"),
            "/plugins/settings/local-ai/model",
            tk("Model or workspace"),
            tk("Exact model ID, or the AnythingLLM workspace slug"),
            Control::Text {
                placeholder: "local-model",
            },
            Value::from(""),
        ),
        sd(
            Section::Plugins,
            tk("Local AI"),
            "/plugins/settings/local-ai/use_api_key",
            tk("Use an API key"),
            tk("Save the key in the keyring below. AnythingLLM always requires one."),
            Control::Toggle,
            Value::from(false),
        ),
        sd(
            Section::Plugins,
            tk("Local AI"),
            "/plugins/settings/local-ai/reply_tokens",
            tk("Reply tokens"),
            tk("Maximum generated tokens; the server owns its model and hardware limits"),
            Control::Slider {
                min: f64::from(*ai::TOKEN_RANGE.start()),
                max: f64::from(*ai::TOKEN_RANGE.end()),
                step: 128.0,
                unit: "",
            },
            Value::from(ai::DEFAULT_REPLY_TOKENS),
        ),
        sd(
            Section::Plugins,
            tk("Local AI"),
            "/plugins/settings/local-ai/context_bytes",
            tk("Context kept"),
            tk(
                "Oldest complete turns leave the in-memory transcript at this limit; AnythingLLM manages context on its server",
            ),
            Control::Slider {
                min: *ai::CONTEXT_RANGE.start() as f64,
                max: *ai::CONTEXT_RANGE.end() as f64,
                step: 1024.0,
                unit: " B",
            },
            Value::from(ai::DEFAULT_CONTEXT_BYTES),
        ),
    ]
}
