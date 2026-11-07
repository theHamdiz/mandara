//! Owned command descriptions cross the worker boundary; handlers never do.

use anyhow::{Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Bound both the grammar and the amount of work one Enter press can perform.
pub const MAX_ACTIONS: usize = 4;
/// Only this many retrieved schemas are exposed to the compiler.
pub const TOP_K: usize = 10;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Command {
    pub path: String,
    pub summary: String,
    #[serde(default)]
    pub args: Vec<Argument>,
    #[serde(default)]
    pub examples: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Argument {
    pub name: String,
    pub kind: String,
    pub required: bool,
    pub description: String,
    #[serde(default)]
    pub choices: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<Value>,
    pub minimum: Option<f64>,
    pub maximum: Option<f64>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Action {
    pub command: String,
    pub args: serde_json::Map<String, Value>,
}

impl Argument {
    fn schema(&self) -> Value {
        let mut schema = match self.kind.as_str() {
            "integer" => json!({"type":"integer"}),
            "number" => json!({"type":"number"}),
            "bool" => json!({"type":"boolean"}),
            "enum" => json!({"type":"string", "enum": self.choices}),
            "value-enum" => json!({"enum":self.values}),
            _ => json!({"type":"string", "maxLength": super::MAX_QUERY_BYTES}),
        };
        schema["description"] = self.description.clone().into();
        if let Some(n) = self.minimum {
            schema["minimum"] = n.into();
        }
        if let Some(n) = self.maximum {
            schema["maximum"] = n.into();
        }
        schema
    }

    fn accepts(&self, value: &Value) -> bool {
        let typed = match self.kind.as_str() {
            "integer" => value.as_i64().is_some(),
            "number" => value.as_f64().is_some_and(f64::is_finite),
            "bool" => value.is_boolean(),
            "enum" => value.as_str().is_some_and(|s| self.choices.iter().any(|c| c == s)),
            "value-enum" => self.values.contains(value),
            _ => value.as_str().is_some_and(|s| s.len() <= super::MAX_QUERY_BYTES),
        };
        typed && value.as_f64().is_none_or(|n| self.minimum.is_none_or(|lo| n >= lo) && self.maximum.is_none_or(|hi| n <= hi))
    }
}

/// Every alternative binds its own command name to its own argument object.
pub fn schema(commands: &[Command], compound: bool) -> Value {
    let choices: Vec<Value> = commands
        .iter()
        .map(|c| {
            let properties: serde_json::Map<String, Value> = c.args.iter().map(|a| (a.name.clone(), a.schema())).collect();
            let required: Vec<&str> = c.args.iter().filter(|a| a.required).map(|a| a.name.as_str()).collect();
            json!({"type":"object", "additionalProperties":false,
            "required":["command","args"], "properties":{
                "command":{"const":c.path},
                "args":{"type":"object","additionalProperties":false,"properties":properties,"required":required}
            }})
        })
        .collect();
    json!({"type":"array","minItems":1,"maxItems":if compound {MAX_ACTIONS} else {1},"items":{"anyOf":choices}})
}

/// Revalidate compiler output before it reaches the live dispatcher. Optional
/// positional arguments cannot be skipped in the middle of a command line.
pub fn validate(actions: &[Action], commands: &[Command], compound: bool) -> Result<Vec<Vec<String>>> {
    ensure!(!actions.is_empty() && actions.len() <= if compound { MAX_ACTIONS } else { 1 }, "invalid action count");
    actions
        .iter()
        .map(|action| {
            let c = commands.iter().find(|c| c.path == action.command).ok_or_else(|| anyhow::anyhow!("command is outside the retrieved catalog"))?;
            ensure!(action.args.keys().all(|name| c.args.iter().any(|a| a.name == *name)), "unknown argument");
            let mut words: Vec<String> = c.path.split_whitespace().map(str::to_owned).collect();
            let mut omitted = false;
            for a in &c.args {
                match action.args.get(&a.name) {
                    Some(value) => {
                        ensure!(!omitted && a.accepts(value), "invalid argument {}", a.name);
                        words.push(if c.path.starts_with("config set /") {
                            value.to_string()
                        } else {
                            value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string())
                        });
                    }
                    None if a.required => bail!("missing argument {}", a.name),
                    None => omitted = true,
                }
            }
            Ok(words)
        })
        .collect()
}

/// Display arguments without ever passing the result through a shell.
pub fn cli(words: &[String]) -> String {
    let mut line = String::from("mandara msg");
    for word in words {
        line.push(' ');
        if word.chars().all(|c| c.is_alphanumeric() || "-_/.:".contains(c)) && !word.is_empty() {
            line.push_str(word);
        } else {
            line.push('\'');
            line.push_str(&word.replace('\'', "'\\''"));
            line.push('\'');
        }
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    fn commands() -> Vec<Command> {
        serde_json::from_value(json!([{"path":"audio volume set","summary":"volume","args":[{"name":"percent","kind":"integer","required":true,"description":"volume","minimum":0,"maximum":150}]},{"path":"theme mode","summary":"mode","args":[{"name":"mode","kind":"enum","required":true,"description":"mode","choices":["light","dark"]}]}])).unwrap()
    }
    fn actions(value: Value) -> Vec<Action> {
        serde_json::from_value(value).unwrap()
    }
    #[test]
    fn binds_types_ranges_and_choices_to_their_command() {
        let c = commands();
        for invalid in [
            json!([{"command":"exec","args":{}}]),
            json!([{"command":"audio volume set","args":{"percent":151}}]),
            json!([{"command":"audio volume set","args":{"percent":"10"}}]),
            json!([{"command":"theme mode","args":{"mode":"blue"}}]),
            json!([{"command":"theme mode","args":{"mode":"dark","extra":0}}]),
            json!([{"command":"theme mode","args":{}}]),
        ] {
            assert!(validate(&actions(invalid), &c, true).is_err());
        }
        let a = actions(json!([{"command":"audio volume set","args":{"percent":10}},{"command":"theme mode","args":{"mode":"dark"}}]));
        assert_eq!(validate(&a, &c, true).unwrap()[0], ["audio", "volume", "set", "10"]);
        assert!(validate(&a, &c, false).is_err());
        assert_eq!(schema(&c, true)["items"]["anyOf"].as_array().unwrap().len(), c.len());
    }
    #[test]
    fn command_preview_quotes_metacharacters_literally() {
        assert_eq!(cli(&["notes".into(), "$(touch /tmp/nope) 'hi'".into()]), "mandara msg notes '$(touch /tmp/nope) '\\''hi'\\'''");
    }
}
