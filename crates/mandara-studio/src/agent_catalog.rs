use crate::registry;

/// Build typed `config set <pointer>` tools only in the inference child.
/// Studio remains the single source of labels, choices and numeric bounds.
pub fn settings_catalog() -> Vec<mandara_services::command_agent::catalog::Command> {
    use mandara_services::command_agent::catalog::{Argument, Command};
    use registry::Control;
    registry::settings()
        .iter()
        .filter_map(|setting| {
            let mut arg = Argument {
                name: "value".into(),
                kind: "string".into(),
                required: true,
                description: setting.help.into(),
                choices: Vec::new(),
                values: Vec::new(),
                minimum: None,
                maximum: None,
            };
            match &setting.control {
                Control::Toggle => arg.kind = "bool".into(),
                Control::Slider { min, max, step, .. } | Control::OptionalSlider { min, max, step, .. } => {
                    arg.kind = if step.fract() == 0.0 { "integer" } else { "number" }.into();
                    arg.minimum = Some(*min);
                    arg.maximum = Some(*max);
                }
                Control::Number { min, max } => {
                    arg.kind = "number".into();
                    arg.minimum = Some(*min);
                    arg.maximum = Some(*max);
                }
                Control::Choice(choices) => {
                    arg.kind = "value-enum".into();
                    arg.values = choices.iter().map(|(value, _)| serde_json::from_str(value).unwrap_or_else(|_| serde_json::Value::from(*value))).collect();
                }
                Control::Text { .. }
                | Control::OptionalText { .. }
                | Control::Icon
                | Control::Time
                | Control::Font { .. }
                | Control::Device { .. }
                | Control::Display => {}
                Control::Command { .. } | Control::DepthModel | Control::List { .. } | Control::RecorderBackends => return None,
            }
            Some(Command { path: format!("config set {}", setting.path), summary: setting.label.into(), args: vec![arg], examples: Vec::new() })
        })
        .collect()
}

#[cfg(test)]
mod agent_catalog_tests {
    use super::*;
    use mandara_services::command_agent::catalog::{Action, validate};
    #[test]
    /// Check config pointers, numeric bounds and JSON quoting against Studio’s actual registry.
    fn settings_tools_use_real_paths_ranges_and_json_choice_types() {
        let commands = settings_catalog();
        let gaps = commands.iter().find(|c| c.path.ends_with("/compositor/look/gaps_inner")).unwrap();
        // Keep the display summary as a translation key, not an English sentence
        // assembled from several keys. The argument retains the retrieval help.
        assert_eq!(gaps.summary, "Gaps between windows");
        assert_eq!(gaps.args[0].kind, "integer");
        assert!(gaps.args[0].maximum.is_some());
        let action = |command: &str, value: serde_json::Value| Action {
            command: command.into(),
            args: serde_json::from_value(serde_json::json!({"value":value})).unwrap(),
        };
        let words = validate(&[action(&gaps.path, serde_json::json!(8))], &commands, false).unwrap();
        assert_eq!(words[0], ["config", "set", "/compositor/look/gaps_inner", "8"]);
        let words = validate(&[action("config set /theme/mode", serde_json::json!("dark"))], &commands, false).unwrap();
        assert_eq!(words[0].last().unwrap(), "\"dark\"");
        assert!(validate(&[action(&gaps.path, serde_json::json!(u32::MAX))], &commands, false).is_err());
    }
}
