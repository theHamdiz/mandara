//! Type compatibility keeps a small compiler from substituting unrelated actions.

use super::catalog::Command;
use anyhow::{Result, anyhow};

fn requested_mode(query: &str) -> Option<&'static str> {
    let words: Vec<_> = query
        .split(|c: char| !c.is_alphanumeric())
        .map(str::to_ascii_lowercase)
        .collect();
    if !words.iter().any(|word| word == "mode") {
        return None;
    }
    let light = words.iter().any(|word| word == "light");
    let dark = words.iter().any(|word| word == "dark");
    if light == dark {
        return None;
    }
    let disabled = switch_value(query) == Some(false);
    Some(if light != disabled { "light" } else { "dark" })
}

fn mode_argument(arg: &super::catalog::Argument) -> bool {
    (arg.choices.iter().any(|c| c == "light") && arg.choices.iter().any(|c| c == "dark"))
        || (arg.values.contains(&serde_json::Value::from("light"))
            && arg.values.contains(&serde_json::Value::from("dark")))
}

fn switch_value(query: &str) -> Option<bool> {
    let lower = query.to_ascii_lowercase();
    let has = |word| lower.split_whitespace().any(|part| part.trim_matches(|c: char| c.is_ascii_punctuation()) == word);
    if has("disable") || has("deactivate") || (has("turn") && has("off")) {
        Some(false)
    } else if has("enable") || has("activate") || (has("turn") && has("on")) {
        Some(true)
    } else {
        None
    }
}

/// A named command followed by one text value already supplies its argument.
fn explicit_string<'a>(query: &'a str, command: &Command) -> Option<&'a str> {
    let [arg] = command.args.as_slice() else { return None; };
    if arg.kind != "string" { return None; }
    let mut expected: Vec<_> = command.path.split_whitespace().map(str::to_ascii_lowercase).collect();
    let mut actual = Vec::with_capacity(expected.len());
    let mut rest = query.trim();
    for _ in &expected {
        let end = rest.find(char::is_whitespace)?;
        let (word, tail) = rest.split_at(end);
        actual.push(word.to_ascii_lowercase());
        rest = tail.trim_start();
    }
    expected.sort_unstable();
    actual.sort_unstable();
    if actual != expected || rest.is_empty() { return None; }
    Some(rest.strip_prefix('"').and_then(|s| s.strip_suffix('"'))
        .or_else(|| rest.strip_prefix('\'').and_then(|s| s.strip_suffix('\''))).unwrap_or(rest))
}

/// Read explicit scalar values without interpreting identifiers such as h264 or 2:255.
pub(super) fn numbers(query: &str) -> Vec<f64> {
    query
        .split_whitespace()
        .filter_map(|word| word.trim_matches(|c: char| c.is_ascii_punctuation() && !matches!(c, '-' | '.')).parse::<f64>().ok())
        .filter(|n| n.is_finite())
        .collect()
}

/// Preserve every compatible candidate; ranking alone does not select the action.
pub fn candidates<'a>(
    query: &str,
    commands: impl IntoIterator<Item = &'a Command>,
) -> Result<Vec<&'a Command>> {
    let numbers = numbers(query);
    let commands: Vec<_> = commands.into_iter().collect();
    let words = |text: &str| {
        let cleaned = text.to_lowercase().replace(['\'', '’'], "");
        let mut words: Vec<String> = cleaned
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| {
                !word.is_empty()
                    && !matches!(
                        *word,
                        "the" | "please" | "my" | "shell" | "mandara" | "panel"
                    )
            })
            .map(str::to_owned)
            .collect();
        words.sort_unstable();
        words
    };
    let query_words = words(query);
    if let Some(command) = commands
        .iter()
        .find(|command| command.args.is_empty() && words(&command.path) == query_words)
    {
        return Ok(vec![*command]);
    }
    if requested_mode(query).is_some() {
        let modes: Vec<_> = commands
            .iter()
            .copied()
            .filter(|command| command.args.iter().any(mode_argument))
            .collect();
        if !modes.is_empty() {
            return Ok(modes);
        }
    }
    let switch = switch_value(query).is_some();
    if switch {
        let target: Vec<_> = query_words
            .iter()
            .filter(|word| {
                !matches!(
                    word.as_str(),
                    "enable"
                        | "disable"
                        | "activate"
                        | "deactivate"
                        | "turn"
                        | "on"
                        | "off"
                        | "a"
                        | "an"
                )
            })
            .collect();
        let labelled: Vec<_> = commands
            .iter()
            .copied()
            .filter(|command| {
                let label = words(&command.summary);
                !target.is_empty()
                    && target.iter().all(|word| label.contains(word))
                    && command.args.iter().any(|arg| arg.kind == "bool")
            })
            .collect();
        if !labelled.is_empty() {
            return Ok(labelled);
        }
    }
    let matches: Vec<_> = commands
        .into_iter()
        .filter(|command| {
            if switch
                && numbers.is_empty()
                && command
                    .args
                    .iter()
                    .any(|arg| arg.required && matches!(arg.kind.as_str(), "integer" | "number"))
            {
                return false;
            }
            numbers.iter().all(|number| {
                command.args.iter().any(|arg| match arg.kind.as_str() {
                    "integer" | "number" => {
                        (arg.kind != "integer" || number.fract() == 0.0)
                            && arg.minimum.is_none_or(|lo| *number >= lo)
                            && arg.maximum.is_none_or(|hi| *number <= hi)
                    }
                    "string" => true,
                    "enum" => arg
                        .choices
                        .iter()
                        .any(|value| value.parse::<f64>().ok() == Some(*number)),
                    "value-enum" => arg.values.iter().any(|value| {
                        value.as_f64() == Some(*number)
                            || value
                                .as_str()
                                .is_some_and(|s| s.parse::<f64>().ok() == Some(*number))
                    }),
                    _ => false,
                })
            })
        })
        .collect();
    if matches.is_empty() {
        Err(anyhow!(
            "no retrieved command accepts the requested value types"
        ))
    } else {
        Ok(matches)
    }
}

#[cfg(test)]
fn choose<'a>(query: &str, commands: impl IntoIterator<Item = &'a Command>) -> Result<&'a Command> {
    Ok(candidates(query, commands)?[0])
}

/// One explicit scalar must survive compilation unchanged, rather than becoming a default.
pub(super) fn grounded(query: &str, commands: &[Command]) -> Vec<Command> {
    let numbers = numbers(query);
    candidates(query, commands)
        .unwrap_or_default()
        .into_iter()
        .map(|command| {
            let mut command = command.clone();
            if let Some(value) = explicit_string(query, &command) {
                command.args[0].kind = "value-enum".into();
                command.args[0].values = vec![serde_json::Value::from(value)];
            }
            if let [number] = numbers.as_slice()
                && command
                    .args
                    .iter()
                    .filter(|arg| matches!(arg.kind.as_str(), "integer" | "number"))
                    .count()
                    == 1
            {
                for arg in &mut command.args {
                    if matches!(arg.kind.as_str(), "integer" | "number")
                        && arg.minimum.is_none_or(|lo| *number >= lo)
                        && arg.maximum.is_none_or(|hi| *number <= hi)
                        && (arg.kind != "integer" || number.fract() == 0.0)
                    {
                        arg.minimum = Some(*number);
                        arg.maximum = Some(*number);
                    }
                }
            }
            if let Some(mode) = requested_mode(query) {
                for arg in &mut command.args {
                    if arg.choices.iter().any(|c| c == "light")
                        && arg.choices.iter().any(|c| c == "dark")
                    {
                        arg.choices = vec![mode.into()];
                    }
                    if arg.values.contains(&serde_json::Value::from("light"))
                        && arg.values.contains(&serde_json::Value::from("dark"))
                    {
                        arg.values = vec![serde_json::Value::from(mode)];
                    }
                }
            } else if let Some(value) = switch_value(query) {
                for arg in &mut command.args {
                    if arg.kind == "bool" {
                        arg.kind = "value-enum".into();
                        arg.values = vec![serde_json::Value::from(value)];
                    }
                }
            }
            command
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn explicit_text_arguments_keep_references_and_spaces() {
        let commands: Vec<Command> = serde_json::from_value(json!([
            {"path":"quran read","summary":"Read an ayah","args":[{"name":"ref","kind":"string","required":true,"description":"surah or ayah"}]},
            {"path":"bluetooth connect","summary":"Connect a device","args":[{"name":"device","kind":"string","required":true,"description":"device name"}]}
        ])).unwrap();
        for (query, index, value) in [("read quran 2:255", 0, "2:255"), ("quran read al-kahf", 0, "al-kahf"), ("connect Bluetooth \"Living room speaker\"", 1, "Living room speaker")] {
            assert_eq!(explicit_string(query, &commands[index]), Some(value));
            let selected = grounded(query, &commands[index..=index]);
            assert_eq!(selected[0].args[0].values, [serde_json::Value::from(value)]);
        }
        assert_eq!(explicit_string("open quran", &commands[0]), None);
        assert_eq!(explicit_string("read quran", &commands[0]), None);
    }

    #[test]
    fn appearance_modes_and_named_switches_keep_their_own_target() {
        let commands: Vec<Command> = serde_json::from_value(json!([
            {"path":"config set /theme/light_at","summary":"Light from","args":[{"name":"value","kind":"string","required":true,"description":"Scheduled light appearance"}]},
            {"path":"config set /night_light/enabled","summary":"Night light","args":[{"name":"value","kind":"bool","required":true,"description":"Warm the screens"}]},
            {"path":"theme mode","summary":"Switch between light and dark appearance","args":[{"name":"mode","kind":"enum","choices":["light","dark","toggle"],"required":true,"description":"appearance"}]},
            {"path":"config set /osd/kinds/do_not_disturb","summary":"Do not disturb","args":[{"name":"value","kind":"bool","required":true,"description":"On-screen displays / Show for / Do not disturb"}]},
            {"path":"config set /general/osd","summary":"Show on-screen displays","args":[{"name":"value","kind":"bool","required":true,"description":"On-screen displays / Display"}]}
        ])).unwrap();
        for (query, mode) in [
            ("enable light mode", "light"),
            ("disable light mode", "dark"),
            ("turn off dark mode", "light"),
            ("turn light mode off", "dark"),
            ("turn dark mode off.", "light"),
        ] {
            let selected = grounded(query, &commands);
            assert_eq!(selected.len(), 1);
            assert_eq!(selected[0].path, "theme mode");
            assert_eq!(selected[0].args[0].choices, [mode]);
        }
        for (query, value) in [
            ("disable on-screen displays", false),
            ("enable on-screen displays", true),
        ] {
            let selected = grounded(query, &commands);
            assert_eq!(selected.len(), 1);
            assert_eq!(selected[0].path, "config set /general/osd");
            assert_eq!(
                selected[0].args[0].schema(),
                json!({"enum":[value],"description":"On-screen displays / Display"})
            );
        }
    }

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
        assert_eq!(
            candidates("disable the toggle", &commands).unwrap().len(),
            1
        );
        let grounded = grounded("set the value to 42", &commands);
        assert_eq!(grounded.len(), 1);
        assert_eq!(grounded[0].args[0].minimum, Some(42.0));
        assert_eq!(grounded[0].args[0].maximum, Some(42.0));
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
