//! Hook interpolation passes values as data, including quoted file paths.

use anyhow::{Result, ensure};
use minijinja::{Environment, Error, ErrorKind, Value};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use super::{Limited, MAX_RENDER_BYTES, MAX_TEMPLATE_BYTES, syntax};

/// A hook has no reason to interpolate more values than a full app theme.
const MAX_VALUES: usize = 1024;
const MARKER: char = '\u{1f}';

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Command {
    pub shell: String,
    pub environment: Vec<(String, String)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Quote {
    None,
    Single,
    Double,
}

enum Scope {
    Command { outer: Quote, depth: usize },
    Backtick { outer: Quote },
}
/// Shell nesting is bounded independently of the render fuel.
const MAX_SCOPES: usize = 64;

pub(super) fn render(
    environment: &Environment<'static>,
    context: &BTreeMap<String, Value>,
    source: &str,
) -> Result<Command> {
    ensure!(
        source.len() <= MAX_TEMPLATE_BYTES && !source.contains(MARKER),
        "invalid hook source"
    );
    let mut environment = environment.clone();
    let values = Arc::new(Mutex::new(Vec::new()));
    let captured = values.clone();
    environment.set_formatter(move |out, _, value| {
        if value.is_undefined() {
            return Err(Error::from(ErrorKind::UndefinedError));
        }
        let mut values = captured
            .lock()
            .map_err(|_| Error::new(ErrorKind::InvalidOperation, "hook values are unavailable"))?;
        if values.len() >= MAX_VALUES {
            return Err(Error::new(
                ErrorKind::InvalidOperation,
                "too many hook values",
            ));
        }
        let marker = values.len();
        values.push(value.to_string());
        write!(out, "{MARKER}{marker}{MARKER}")?;
        Ok(())
    });
    let normalized = syntax::normalize(source)?;
    let template = environment.template_from_str(&normalized)?;
    let mut output = Limited {
        text: String::with_capacity(source.len()),
        limit: MAX_RENDER_BYTES,
    };
    template.render_captured_to(Value::from_object(context.clone()), &mut output)?;
    let mut shell = String::with_capacity(output.text.len());
    let mut quote = Quote::None;
    let mut scopes = Vec::new();
    let mut escape = false;
    let mut rest = output.text.as_str();
    while !rest.is_empty() {
        let ch = rest.chars().next().unwrap();
        rest = &rest[ch.len_utf8()..];
        if ch == MARKER {
            ensure!(!escape, "an escaped interpolation needs its own quotes");
            let end = rest
                .find(MARKER)
                .ok_or_else(|| anyhow::anyhow!("incomplete hook value"))?;
            let index = rest[..end].parse::<usize>()?;
            let key = format!("MANDARA_TEMPLATE_{index}");
            match quote {
                Quote::None => write!(&mut shell, "\"${{{key}}}\"")?,
                Quote::Single => write!(&mut shell, "'\"${{{key}}}\"'")?,
                Quote::Double => write!(&mut shell, "${{{key}}}")?,
            }
            rest = &rest[end + MARKER.len_utf8()..];
            continue;
        }
        shell.push(ch);
        if escape {
            escape = false;
            continue;
        }
        if quote != Quote::Single && ch == '$' && rest.starts_with('(') {
            ensure!(
                scopes.len() < MAX_SCOPES,
                "hook command nesting is too deep"
            );
            scopes.push(Scope::Command {
                outer: quote,
                depth: 1,
            });
            quote = Quote::None;
            shell.push('(');
            rest = &rest[1..];
            continue;
        }
        if quote != Quote::Single && ch == '`' {
            if matches!(scopes.last(), Some(Scope::Backtick { .. })) {
                if let Some(Scope::Backtick { outer }) = scopes.pop() {
                    quote = outer;
                }
            } else {
                ensure!(
                    scopes.len() < MAX_SCOPES,
                    "hook command nesting is too deep"
                );
                scopes.push(Scope::Backtick { outer: quote });
                quote = Quote::None;
            }
            continue;
        }
        if quote == Quote::None
            && let Some(Scope::Command { depth, .. }) = scopes.last_mut()
        {
            if ch == '(' {
                *depth += 1;
            }
            if ch == ')' {
                *depth -= 1;
                if *depth == 0
                    && let Some(Scope::Command { outer, .. }) = scopes.pop()
                {
                    quote = outer;
                }
                continue;
            }
        }
        match (quote, ch) {
            (Quote::None | Quote::Double, '\\') => escape = true,
            (Quote::None, '\'') => quote = Quote::Single,
            (Quote::Single, '\'') => quote = Quote::None,
            (Quote::None, '"') => quote = Quote::Double,
            (Quote::Double, '"') => quote = Quote::None,
            _ => {}
        }
    }
    let environment = values
        .lock()
        .map_err(|_| anyhow::anyhow!("hook values are unavailable"))?
        .iter()
        .enumerate()
        .map(|(i, value)| (format!("MANDARA_TEMPLATE_{i}"), value.clone()))
        .collect();
    Ok(Command { shell, environment })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::templates::custom::Renderer;

    #[test]
    fn data_cannot_change_shell_syntax_in_any_quote_style_or_in_a_loop() {
        let mut renderer = Renderer::new(
            &crate::engine::default_palette(true),
            &crate::engine::default_palette(false),
            &BTreeMap::new(),
        )
        .unwrap();
        let path = "/tmp/a 'quote' \"double\" $(false); unicode العَرَبِيَّة";
        renderer.set("config_dir", path);
        for command in [
            "printf '%s' {{ config_dir }}",
            "printf '%s' '{{ config_dir }}'",
            "printf '%s' \"{{ config_dir }}\"",
        ] {
            let command = renderer.render_command(command).unwrap();
            let output = std::process::Command::new("sh")
                .args(["-c", &command.shell])
                .envs(command.environment)
                .output()
                .unwrap();
            assert!(output.status.success());
            assert_eq!(String::from_utf8(output.stdout).unwrap(), path);
        }
        for source in [
            "printf '%s' \"$(printf '%s' '{{ config_dir }}')\"",
            "printf '%s' \"$(printf '%s' \"{{ config_dir }}\")\"",
            "printf '%s' \"`printf '%s' '{{ config_dir }}'`\"",
        ] {
            let command = renderer.render_command(source).unwrap();
            let output = std::process::Command::new("sh")
                .args(["-c", &command.shell])
                .envs(command.environment)
                .output()
                .unwrap();
            assert!(
                output.status.success(),
                "{source}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            assert_eq!(String::from_utf8(output.stdout).unwrap(), path, "{source}");
        }
        let command = renderer
            .render_command("<* for i in 0..3 *>printf '%s' '{{ i }}';<* endfor *>")
            .unwrap();
        let output = std::process::Command::new("sh")
            .args(["-c", &command.shell])
            .envs(command.environment)
            .output()
            .unwrap();
        assert_eq!(output.stdout, b"012");
    }
}
