//! Type compatibility keeps a small compiler from substituting unrelated actions.

use super::catalog::Command;
use anyhow::{Result, anyhow};

/// Select the highest-ranked tool that can represent the query's numeric literals.
/// Boolean-only actions cannot stand in for a requested numeric setting.
pub fn choose<'a>(query: &str, commands: impl IntoIterator<Item = &'a Command>) -> Result<&'a Command> {
    let numbers: Vec<f64> = query
        .split_whitespace()
        .filter_map(|word| word.trim_matches(|c: char| c.is_ascii_punctuation() && !matches!(c, '-' | '.')).parse::<f64>().ok())
        .filter(|n| n.is_finite())
        .collect();
    commands
        .into_iter()
        .find(|command| {
            numbers.iter().all(|number| {
                command.args.iter().any(|arg| match arg.kind.as_str() {
                    "integer" | "number" | "string" => true,
                    "enum" => arg.choices.iter().any(|value| value.parse::<f64>().ok() == Some(*number)),
                    "value-enum" => {
                        arg.values.iter().any(|value| value.as_f64() == Some(*number) || value.as_str().is_some_and(|s| s.parse::<f64>().ok() == Some(*number)))
                    }
                    _ => false,
                })
            })
        })
        .ok_or_else(|| anyhow!("no retrieved command accepts the requested value types"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Numeric requests retain a numeric target even when a related toggle ranks first.
    #[test]
    fn numeric_settings_cannot_be_replaced_with_booleans() {
        let commands: Vec<Command> = serde_json::from_value(json!([
            {"path":"toggle","summary":"toggle","args":[{"name":"value","kind":"bool","required":true,"description":"toggle"}]},
            {"path":"value","summary":"value","args":[{"name":"value","kind":"integer","required":true,"description":"value"}]}
        ]))
        .unwrap();
        assert_eq!(choose("set gaps to 10", &commands).unwrap().path, "value");
        assert_eq!(choose("enable automatic gaps", &commands).unwrap().path, "toggle");
        assert!(choose("set gaps to 10", &commands[..1]).is_err());
    }

    /// String identifiers and explicit numeric enum choices remain valid targets.
    #[test]
    fn numeric_ids_do_not_require_numeric_storage() {
        let commands: Vec<Command> = serde_json::from_value(json!([
            {"path":"enum","summary":"workspace","args":[{"name":"id","kind":"enum","required":true,"description":"workspace","choices":["1","2"]}]},
            {"path":"string","summary":"workspace","args":[{"name":"id","kind":"string","required":true,"description":"workspace"}]}
        ]))
        .unwrap();
        assert_eq!(choose("focus workspace 2", &commands).unwrap().path, "enum");
        assert_eq!(choose("focus workspace 40", &commands).unwrap().path, "string");
        let codec: Vec<Command> = serde_json::from_value(json!([
            {"path":"codec","summary":"codec","args":[{"name":"id","kind":"enum","required":true,"description":"codec","choices":["av1","h264"]}]},
            {"path":"fallback","summary":"fallback","args":[{"name":"id","kind":"string","required":true,"description":"id"}]}
        ]))
        .unwrap();
        assert_eq!(choose("record with h264", &codec).unwrap().path, "codec");
    }
}
