//! Native line formats keep their flags, modes and command tails separate.

use std::ops::Range;

use anyhow::Result;
use mandara_i18n::tk;

use crate::{
    compositor::Target,
    keybinds::{Binding, DYNAMIC, Encoding, ShortcutFormat, field, normalize},
};

pub(super) struct Lines(pub Target);

impl ShortcutFormat for Lines {
    fn parse(&self, source: &str) -> Result<Vec<Binding>> {
        let mut result = Vec::new();
        let mut offset = 0;
        let mut scope = String::from("default");
        let mut sway_block = false;
        let resolved = crate::compositor::chords_by_line(self.0, source);
        for (index, raw) in source.split_inclusive('\n').enumerate() {
            let code = raw.trim_end_matches(['\n', '\r']);
            let start = code.len() - code.trim_start().len();
            let text = &code[start..];
            if text.starts_with('#') || text.is_empty() {
                offset += raw.len();
                continue;
            }
            let origin = offset + start;
            let mut binding = match self.0 {
                Target::Mango | Target::Hyprland => {
                    if let Some((key, value)) = text.split_once('=') {
                        let key = key.trim();
                        if key == "key_mode" || key == "submap" {
                            scope = value.trim().to_owned();
                        }
                        if key.starts_with("bind")
                            && key["bind".len()..].chars().all(|c| c.is_ascii_alphabetic())
                        {
                            csv(self.0, text, origin, key, &scope)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                }
                Target::Sway => {
                    if text.starts_with("mode ") && text.ends_with('{') {
                        scope = text[5..text.len() - 1].trim().trim_matches('"').to_owned();
                    }
                    if text == "}" {
                        if sway_block {
                            sway_block = false;
                        } else {
                            scope = "default".into();
                        }
                    }
                    if text == "bindsym {" || text == "bindcode {" {
                        sway_block = true;
                        None
                    } else {
                        sway(text, origin, &scope, sway_block)
                    }
                }
                Target::River => river(text, origin),
                _ => None,
            };
            if let Some(bind) = &mut binding {
                bind.line = index + 1;
                if let Some((_, chord)) = resolved.iter().find(|(line, _)| *line == index) {
                    bind.chord = Some(chord.clone());
                }
            }
            result.extend(binding);
            offset += raw.len();
        }
        Ok(result)
    }
}

fn comment_start(text: &str) -> usize {
    let mut quote = None;
    let mut escaped = false;
    for (at, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
        } else if matches!(ch, '\'' | '"') {
            quote = Some(ch);
        } else if ch == '#' {
            return at;
        }
    }
    text.len()
}

fn csv(target: Target, text: &str, offset: usize, directive: &str, scope: &str) -> Option<Binding> {
    let equal = text.find('=')?;
    let description =
        target == Target::Hyprland && directive.trim_start_matches("bind").contains('d');
    let head_count = if description { 4 } else { 3 };
    let mut spans = Vec::new();
    let mut from = equal + 1;
    let end = comment_start(text);
    for index in 0..head_count {
        if let Some(comma) = text[from..end].find(',').map(|at| at + from) {
            spans.push(trim(text, from..comma));
            from = comma + 1;
        } else if index == head_count - 1 {
            spans.push(trim(text, from..end));
            from = end;
        } else {
            return None;
        }
    }
    spans.push(trim(text, from..end));
    let mut fields = vec![field(
        tk("Flags"),
        directive,
        offset..offset + equal,
        Encoding::Word,
    )];
    // The directive span excludes padding before '='.
    fields[0].range = (offset + trim(text, 0..equal).start)..(offset + trim(text, 0..equal).end);
    for (at, span) in spans.iter().enumerate() {
        let label = match at {
            0 => tk("Modifiers"),
            1 => tk("Key"),
            2 if description => tk("Description"),
            at if at == head_count - 1 => tk("Action"),
            _ => tk("Arguments"),
        };
        let encoding = if at == head_count {
            Encoding::Plain
        } else {
            Encoding::Csv
        };
        fields.push(field(
            label,
            &text[span.clone()],
            offset + span.start..offset + span.end,
            encoding,
        ));
    }
    let combo = format!(
        "{}+{}",
        text[spans[0].clone()].replace(' ', "+"),
        &text[spans[1].clone()]
    );
    Some(Binding {
        line: 0,
        scope: scope.to_owned(),
        chord: normalize(&combo),
        fields,
        read_only: None,
    })
}

fn sway(text: &str, offset: usize, scope: &str, block: bool) -> Option<Binding> {
    let tokens = words(text);
    let first = tokens.first()?;
    let directive = &text[first.clone()];
    let mut at = if matches!(directive, "bindsym" | "bindcode") {
        1
    } else if block {
        0
    } else {
        return None;
    };
    let mut fields = Vec::new();
    while let Some(span) = tokens.get(at)
        && text[span.clone()].starts_with("--")
    {
        fields.push(field(
            tk("Flags"),
            &text[span.clone()],
            offset + span.start..offset + span.end,
            Encoding::Word,
        ));
        at += 1;
    }
    let key = tokens.get(at)?;
    fields.insert(
        0,
        field(
            tk("Shortcut"),
            &text[key.clone()],
            offset + key.start..offset + key.end,
            Encoding::Word,
        ),
    );
    let action = tokens.get(at + 1)?;
    let tail = trim(text, action.start..tokens.last()?.end);
    fields.push(field(
        tk("Action"),
        &text[tail.clone()],
        offset + tail.start..offset + tail.end,
        Encoding::Plain,
    ));
    Some(Binding {
        line: 0,
        scope: scope.to_owned(),
        chord: if directive == "bindcode" {
            None
        } else {
            normalize(&text[key.clone()])
        },
        fields,
        read_only: None,
    })
}

fn river(text: &str, offset: usize) -> Option<Binding> {
    let tokens = words(text);
    let executable = &text[tokens.first()?.clone()];
    if executable.rsplit('/').next()? != "riverctl" || text[tokens.get(1)?.clone()] != *"map" {
        return None;
    }
    let mut at = 2;
    let mut fields = Vec::new();
    while let Some(span) = tokens.get(at)
        && text[span.clone()].starts_with('-')
    {
        fields.push(field(
            tk("Flags"),
            &text[span.clone()],
            offset + span.start..offset + span.end,
            Encoding::Word,
        ));
        let takes_value = &text[span.clone()] == "-layout";
        at += 1;
        if takes_value {
            let span = tokens.get(at)?;
            fields.push(field(
                tk("Layout"),
                &text[span.clone()],
                offset + span.start..offset + span.end,
                Encoding::Word,
            ));
            at += 1;
        }
    }
    let mode = tokens.get(at)?;
    let mods = tokens.get(at + 1)?;
    let key = tokens.get(at + 2)?;
    let action = tokens.get(at + 3)?;
    fields.push(field(
        tk("Mode"),
        &text[mode.clone()],
        offset + mode.start..offset + mode.end,
        Encoding::Word,
    ));
    fields.push(field(
        tk("Modifiers"),
        &text[mods.clone()],
        offset + mods.start..offset + mods.end,
        Encoding::Word,
    ));
    fields.push(field(
        tk("Key"),
        &text[key.clone()],
        offset + key.start..offset + key.end,
        Encoding::Word,
    ));
    for (index, span) in tokens.iter().enumerate().skip(at + 3) {
        let native = crate::compositor::shell_words(&text[span.clone()]);
        let value = native.first().cloned().unwrap_or_default();
        fields.push(field(
            if index == at + 3 {
                tk("Action")
            } else {
                tk("Argument")
            },
            value,
            offset + span.start..offset + span.end,
            Encoding::ShellWord,
        ));
    }
    let dynamic = text[..action.end].contains(['$', '`', ';', '|', '&']) || text.ends_with('\\');
    let combo = format!("{}+{}", &text[mods.clone()], &text[key.clone()]);
    Some(Binding {
        line: 0,
        scope: text[mode.clone()].to_owned(),
        chord: normalize(&combo),
        fields,
        read_only: dynamic.then_some(DYNAMIC),
    })
}

pub(super) fn trim(text: &str, span: Range<usize>) -> Range<usize> {
    let raw = &text[span.clone()];
    let start = span.start + raw.len() - raw.trim_start().len();
    start..start + raw.trim().len()
}

/// Shell words as source spans, including their quotes; comments stay outside.
pub(super) fn words(text: &str) -> Vec<Range<usize>> {
    let mut result = Vec::new();
    let mut quote = None;
    let mut escaped = false;
    let mut start = None;
    for (at, ch) in text.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        if ch == '\\' && quote != Some('\'') {
            escaped = true;
            start.get_or_insert(at);
            continue;
        }
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '#' && start.is_none() {
            break;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            start.get_or_insert(at);
        } else if ch.is_whitespace() {
            if let Some(from) = start.take() {
                result.push(from..at);
            }
        } else {
            start.get_or_insert(at);
        }
    }
    if let Some(from) = start {
        result.push(from..text.len());
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_formats_preserve_modes_flags_and_other_bindings() {
        for (target, text, scope) in [
            (
                Target::Mango,
                "key_mode=resize\nbindr=SUPER,Q,killclient,\n# keep\n",
                "resize",
            ),
            (
                Target::Hyprland,
                "submap = resize\nbindrd = SUPER, Q, Close window, killactive,\n# keep\n",
                "resize",
            ),
            (
                Target::Sway,
                "mode \"resize\" {\n bindsym --release Mod4+q kill\n}\n# keep\n",
                "resize",
            ),
            (
                Target::River,
                "#!/bin/sh\nriverctl map -release resize Super Q spawn 'foot --title=hello'\n# keep\n",
                "resize",
            ),
        ] {
            let binds = Lines(target).parse(text).unwrap();
            assert_eq!(binds.len(), 1, "{target:?}");
            assert_eq!(binds[0].scope, scope);
            let mut values: Vec<_> = binds[0].fields.iter().map(|f| f.value.clone()).collect();
            let at = binds[0]
                .fields
                .iter()
                .position(|f| matches!(f.label, "Key" | "Shortcut"))
                .unwrap();
            values[at] = if target == Target::Sway {
                "Mod4+w".into()
            } else {
                "W".into()
            };
            let next = crate::keybinds::patch(text, &binds[0], &values).unwrap();
            assert!(next.ends_with("# keep\n"));
            assert_eq!(Lines(target).parse(&next).unwrap()[0].scope, scope);
        }
    }

    #[test]
    fn computed_river_maps_stay_read_only_and_quoted_comment_characters_survive() {
        let source = "riverctl map $mode Super Q spawn 'printf \"#hello\"' # keep\n";
        let binds = Lines(Target::River).parse(source).unwrap();
        assert_eq!(binds[0].read_only, Some(DYNAMIC));
        assert_eq!(binds[0].fields.last().unwrap().value, "printf \"#hello\"");
    }
}
