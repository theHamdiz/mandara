//! FunctionGemma's native declarations and calls, constrained to retrieved tools.
use super::catalog::{Action, Argument, Command};
use anyhow::{Result, anyhow, ensure};
use serde_json::{Value, json};
use std::fmt::Write;

const CALL_START: &str = "<start_function_call>";
const CALL_END: &str = "<end_function_call>";
const ESCAPE: &str = "<escape>";

fn name(command: &Command, index: usize) -> String {
    let mut name: String = command
        .path
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    write!(name, "_{index}").expect("writing a String");
    name
}

fn text(value: &str) -> String {
    format!(
        "{ESCAPE}{}{ESCAPE}",
        value.replace('<', "‹").replace('>', "›")
    )
}

fn native(value: &Value) -> String {
    match value {
        Value::String(s) => text(s),
        Value::Array(values) => format!(
            "[{}]",
            values.iter().map(native).collect::<Vec<_>>().join(",")
        ),
        Value::Object(map) => format!(
            "{{{}}}",
            map.iter()
                .map(|(key, value)| format!("{key}:{}", native(value)))
                .collect::<Vec<_>>()
                .join(",")
        ),
        _ => value.to_string(),
    }
}

/// Use the control tokens and declaration format the compiler was trained on.
pub(super) fn prompt(query: &str, commands: &[Command]) -> String {
    let mut out = String::from(
        "<bos><start_of_turn>developer\nYou are a model that can do function calling with the following functions",
    );
    for (index, command) in commands.iter().enumerate() {
        let properties: serde_json::Map<String, Value> = command
            .args
            .iter()
            .map(|arg| {
                let mut schema = arg.schema();
                schema
                    .as_object_mut()
                    .expect("argument schema")
                    .remove("maxLength");
                if schema.get("type").is_none() {
                    let kind = match arg.values.first() {
                        Some(Value::Bool(_)) => "boolean",
                        Some(Value::Number(_)) => "number",
                        _ => "string",
                    };
                    schema["type"] = kind.into();
                }
                if let Some(kind) = schema.get_mut("type") {
                    *kind = kind.as_str().unwrap_or("string").to_uppercase().into();
                }
                (arg.name.clone(), schema)
            })
            .collect();
        let required: Vec<&str> = command
            .args
            .iter()
            .filter(|arg| arg.required)
            .map(|arg| arg.name.as_str())
            .collect();
        let declaration = json!({"description":command.summary,"parameters":{"type":"OBJECT","properties":properties,"required":required}});
        write!(
            out,
            "<start_function_declaration>declaration:{}{}<end_function_declaration>",
            name(command, index),
            native(&declaration)
        )
        .expect("writing a String");
    }
    write!(
        out,
        "<end_of_turn>\n<start_of_turn>user\n{query}<end_of_turn>\n<start_of_turn>model\n"
    )
    .expect("writing a String");
    out
}

fn literal(value: &str) -> String {
    serde_json::to_string(value).expect("string literal")
}

fn choice(value: &Value) -> String {
    match value.as_str() {
        Some(s) => format!("<escape> {} <escape>", literal(s)),
        None => literal(&value.to_string()),
    }
}

fn value_rule(arg: &Argument) -> String {
    if !arg.values.is_empty() {
        return arg
            .values
            .iter()
            .map(choice)
            .collect::<Vec<_>>()
            .join(" | ");
    }
    if !arg.choices.is_empty() {
        return arg
            .choices
            .iter()
            .map(|s| choice(&Value::from(s.clone())))
            .collect::<Vec<_>>()
            .join(" | ");
    }
    match arg.kind.as_str() {
        "integer" | "number" | "bool" => {
            let mut schema = arg.schema();
            schema["x-guidance"] = json!({"whitespace_pattern":r"[\x20\x0A\x0D\x09]{1,2}"});
            format!("%json {schema}")
        }
        _ => "<escape> STRING <escape>".into(),
    }
}

/// Bind names, types, enums and positional optional tails before sampling.
pub(super) fn grammar(query: &str, commands: &[Command]) -> String {
    let mut out = format!(
        "start: <start_function_call> \"call:\" ({}) <end_function_call>\nSTRING: %regex {}\n%ignore /[ \\t\\r\\n]{{1,2}}/\n",
        (0..commands.len())
            .map(|i| format!("tool_{i}"))
            .collect::<Vec<_>>()
            .join(" | "),
        json!({"substring_chars":query})
    );
    for (index, command) in commands.iter().enumerate() {
        let fields = if command.args.is_empty() {
            String::new()
        } else if command.args[0].required {
            format!("args_{index}_0")
        } else {
            format!("[args_{index}_0]")
        };
        writeln!(
            out,
            "tool_{index}: {} \"{{\" {fields} \"}}\"",
            literal(&name(command, index))
        )
        .expect("writing a String");
        for (position, arg) in command.args.iter().enumerate() {
            let tail = match command.args.get(position + 1) {
                None => String::new(),
                Some(next) => {
                    let rule = format!("\",\" args_{index}_{}", position + 1);
                    if next.required {
                        rule
                    } else {
                        format!("[{rule}]")
                    }
                }
            };
            writeln!(
                out,
                "args_{index}_{position}: {} \":\" value_{index}_{position} {tail}\nvalue_{index}_{position}: {}",
                literal(&arg.name),
                value_rule(arg)
            )
            .expect("writing a String");
        }
    }
    out
}

/// Preserve commas, braces and Unicode inside native string delimiters.
pub(super) fn parse(output: &str, commands: &[Command]) -> Result<Action> {
    let body = output
        .trim()
        .strip_prefix(CALL_START)
        .and_then(|s| s.strip_suffix(CALL_END))
        .and_then(|s| s.trim().strip_prefix("call:"))
        .ok_or_else(|| anyhow!("incomplete function call"))?
        .trim();
    let (function, mut input) = body
        .split_once('{')
        .ok_or_else(|| anyhow!("missing function arguments"))?;
    let command = commands
        .iter()
        .enumerate()
        .find(|(i, c)| name(c, *i) == function.trim())
        .map(|(_, c)| c)
        .ok_or_else(|| anyhow!("unknown function"))?;
    let mut args = serde_json::Map::new();
    input = input.trim_start();
    while !input.starts_with('}') {
        let (key, rest) = input
            .split_once(':')
            .ok_or_else(|| anyhow!("missing argument value"))?;
        ensure!(
            command.args.iter().any(|arg| arg.name == key.trim()),
            "unknown function argument"
        );
        let rest = rest.trim_start();
        let (value, remaining) = if let Some(rest) = rest.strip_prefix(ESCAPE) {
            let (string, rest) = rest
                .split_once(ESCAPE)
                .ok_or_else(|| anyhow!("unterminated function string"))?;
            (Value::from(string), rest)
        } else {
            let end = rest
                .find([',', '}'])
                .ok_or_else(|| anyhow!("unterminated function value"))?;
            (serde_json::from_str(rest[..end].trim())?, &rest[end..])
        };
        ensure!(
            args.insert(key.trim().to_owned(), value).is_none(),
            "duplicate function argument"
        );
        input = remaining.trim_start();
        if let Some(rest) = input.strip_prefix(',') {
            input = rest.trim_start();
            ensure!(!input.starts_with('}'), "trailing argument separator");
        } else {
            ensure!(input.starts_with('}'), "invalid function separator");
        }
    }
    ensure!(input == "}", "text after function call");
    let action = Action {
        command: command.path.clone(),
        args,
    };
    super::catalog::validate(std::slice::from_ref(&action), commands, false)?;
    Ok(action)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn commands() -> Vec<Command> {
        serde_json::from_value(json!([{"path":"theme mode","summary":"Appearance","args":[{"name":"mode","kind":"enum","choices":["light","dark"],"required":true,"description":"mode"}]},{"path":"fixture set","summary":"Set a value","args":[{"name":"text","kind":"string","required":true,"description":"text"},{"name":"enabled","kind":"bool","required":false,"description":"enabled"}]}])).unwrap()
    }
    #[test]
    fn native_strings_keep_punctuation_unicode_and_false() {
        let action = parse(
            "<start_function_call>call:fixture_set_1{text:<escape>مرحبا, {world}: \"quoted\"<escape>,enabled:false}<end_function_call>",
            &commands(),
        )
        .unwrap();
        assert_eq!(action.args["text"], "مرحبا, {world}: \"quoted\"");
        assert_eq!(action.args["enabled"], false);
        for invalid in [
            "<start_function_call>call:theme_mode_0{mode:<escape>blue<escape>}<end_function_call>",
            "<start_function_call>call:fixture_set_1{text:<escape>a<escape>,text:<escape>b<escape>}<end_function_call>",
        ] {
            assert!(parse(invalid, &commands()).is_err());
        }
    }
    #[test]
    fn declarations_and_grammar_use_native_control_tokens() {
        let c = commands();
        let p = prompt("switch to light mode", &c);
        assert!(p.contains("<start_function_declaration>declaration:theme_mode_0"));
        assert!(p.contains("type:<escape>STRING<escape>"));
        let g = grammar("switch to light mode", &c);
        assert!(g.contains("<start_function_call>"));
        assert!(g.contains("<escape> \"light\" <escape>"));
        assert!(g.contains("[\",\" args_1_1]"));
    }
}
