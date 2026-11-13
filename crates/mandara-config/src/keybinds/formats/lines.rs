//! Native line formats keep their flags, modes and command tails separate.

use std::ops::Range;

use anyhow::{Result, ensure};
use mandara_i18n::tk;

use crate::{
    compositor::Target,
    keybinds::{Binding, DYNAMIC, Encoding, ShortcutFormat, field, normalize},
};

pub(super) struct Lines(pub Target);

struct SwayBlock {
    keycodes: bool,
    flags: Vec<crate::keybinds::Field>,
}

impl ShortcutFormat for Lines {
    fn parse(&self, source: &str) -> Result<Vec<Binding>> {
        let mut result = Vec::new();
        let mut scope = String::from("default");
        let mut sway_block = None;
        let resolved = match self.0 {
            Target::River | Target::Hyprland => Vec::new(),
            Target::Sway => crate::compositor::sway_binds_checked(source)?
                .into_iter()
                .map(|bind| (bind.line, bind.chord))
                .collect(),
            _ => crate::compositor::chords_by_line(self.0, source),
        };
        ensure!(
            resolved.len() <= crate::keybinds::MAX_BINDINGS,
            "{}",
            mandara_i18n::t("The compositor has too many shortcuts to edit at once.")
        );
        let mut resolved = resolved.into_iter().peekable();
        let continuation = match self.0 {
            Target::Hyprland => crate::keybinds::logical_lines::Continuation::Hyprland,
            Target::Sway => crate::keybinds::logical_lines::Continuation::Sway,
            _ => crate::keybinds::logical_lines::Continuation::None,
        };
        for logical in crate::keybinds::logical_lines::LogicalLines::new(source, continuation) {
            let logical = logical?;
            let index = logical.number - 1;
            let offset = logical.offset;
            let raw = logical.text.as_ref();
            let code = raw.trim_end_matches(['\n', '\r']);
            let start = code.len() - code.trim_start().len();
            let text = &code[start..];
            if text.starts_with('#') || text.is_empty() {
                continue;
            }
            let next_brace = self.0 == Target::Sway
                && crate::keybinds::logical_lines::followed_by_brace(source, logical.end);
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
                    if text.starts_with("mode ") && (text.ends_with('{') || next_brace) {
                        scope = text[5..]
                            .trim_end_matches('{')
                            .trim()
                            .trim_matches('"')
                            .to_owned();
                    }
                    if text == "}" {
                        if sway_block.is_some() {
                            sway_block = None;
                        } else {
                            scope = "default".into();
                        }
                    }
                    if let Some(header) = sway_header(text, origin, next_brace) {
                        sway_block = Some(header);
                        None
                    } else {
                        sway(text, origin, &scope, sway_block.as_ref())
                    }
                }
                Target::River => river(text, origin),
                _ => None,
            };
            if let Some(bind) = &mut binding {
                ensure!(
                    result.len() < crate::keybinds::MAX_BINDINGS,
                    "{}",
                    mandara_i18n::t("The compositor has too many shortcuts to edit at once.")
                );
                bind.line = index + 1;
                if logical.continued {
                    bind.read_only = Some(crate::keybinds::SOURCE_ONLY);
                }
                while resolved.next_if(|(line, _)| *line < index).is_some() {}
                if let Some((_, chord)) = resolved.peek().filter(|(line, _)| *line == index) {
                    bind.chord = Some(chord.clone());
                }
            }
            result.extend(binding);
        }
        if self.0 == Target::River && !river_source_complete(source, &result) {
            for row in &mut result {
                row.read_only = Some(crate::keybinds::river::UNRESOLVED);
            }
        }
        Ok(result)
    }
}

fn comment_start(text: &str, doubled_hash: bool) -> usize {
    let mut quote = None;
    let mut escaped = false;
    let mut chars = text.char_indices().peekable();
    while let Some((at, ch)) = chars.next() {
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
            if doubled_hash && chars.peek().is_some_and(|(_, next)| *next == '#') {
                chars.next();
                continue;
            }
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
    let mut missing_separator = false;
    let end = comment_start(text, target == Target::Hyprland);
    for index in 0..head_count {
        if let Some(comma) = text[from..end].find(',').map(|at| at + from) {
            spans.push(trim(text, from..comma));
            from = comma + 1;
        } else if index == head_count - 1 {
            missing_separator = true;
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
        let encoding = if at == head_count && missing_separator {
            Encoding::CsvTail
        } else if at == head_count {
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

fn sway_header(text: &str, offset: usize, next_brace: bool) -> Option<SwayBlock> {
    if !text.ends_with('{') && !next_brace {
        return None;
    }
    let tokens = words(text);
    let directive = &text[tokens.first()?.clone()];
    if !matches!(directive, "bindsym" | "bindcode") {
        return None;
    }
    let end = tokens.len() - usize::from(text[tokens.last()?.clone()] == *"{");
    let mut flags = Vec::new();
    for span in &tokens[1..end] {
        if !text[span.clone()].starts_with("--") {
            return None;
        }
        let mut flag = field(
            tk("Flags"),
            &text[span.clone()],
            offset + span.start..offset + span.end,
            Encoding::Word,
        );
        flag.read_only = Some(crate::keybinds::SOURCE_ONLY);
        flags.push(flag);
    }
    Some(SwayBlock {
        keycodes: directive == "bindcode",
        flags,
    })
}

fn sway(text: &str, offset: usize, scope: &str, block: Option<&SwayBlock>) -> Option<Binding> {
    let tokens = words(text);
    let first = tokens.first()?;
    let directive = &text[first.clone()];
    let mut at = if matches!(directive, "bindsym" | "bindcode") {
        1
    } else if block.is_some() {
        0
    } else {
        return None;
    };
    let mut fields = block.map_or_else(Vec::new, |block| block.flags.clone());
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
        chord: if directive == "bindcode" || block.is_some_and(|block| block.keycodes) {
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
    let mut native = crate::compositor::shell_words(&text[..tokens.last()?.end]);
    if native.len() != tokens.len()
        || native.first()?.rsplit('/').next()? != "riverctl"
        || native.get(1)? != "map"
    {
        return None;
    }
    let mut at = 2;
    let mut fields = Vec::new();
    while native.get(at).is_some_and(|word| word.starts_with('-')) {
        let takes_value = native[at] == "-layout";
        let span = tokens.get(at)?;
        fields.push(field(
            tk("Flags"),
            std::mem::take(&mut native[at]),
            offset + span.start..offset + span.end,
            Encoding::ShellWord,
        ));
        at += 1;
        if takes_value {
            let span = tokens.get(at)?;
            fields.push(field(
                tk("Layout"),
                std::mem::take(&mut native[at]),
                offset + span.start..offset + span.end,
                Encoding::ShellWord,
            ));
            at += 1;
        }
    }
    let scope = native.get(at)?.clone();
    let combo = format!("{}+{}", native.get(at + 1)?, native.get(at + 2)?);
    native.get(at + 3)?;
    for (index, span) in tokens.iter().enumerate().skip(at) {
        let label = match index - at {
            0 => tk("Mode"),
            1 => tk("Modifiers"),
            2 => tk("Key"),
            3 => tk("Action"),
            _ => tk("Argument"),
        };
        fields.push(field(
            label,
            std::mem::take(&mut native[index]),
            offset + span.start..offset + span.end,
            Encoding::ShellWord,
        ));
    }
    let dynamic = tokens
        .iter()
        .any(|span| computed_shell_word(&text[span.clone()]));
    Some(Binding {
        line: 0,
        scope,
        chord: normalize(&combo),
        fields,
        read_only: dynamic.then_some(DYNAMIC),
    })
}

fn computed_shell_word(word: &str) -> bool {
    dynamic_shell_word(word, true)
}

fn dynamic_shell_word(word: &str, parameters: bool) -> bool {
    let mut quote = None;
    let mut chars = word.char_indices().peekable();
    while let Some((at, ch)) = chars.next() {
        if quote == Some('\'') {
            if ch == '\'' {
                quote = None;
            }
            continue;
        }
        if ch == '\\' {
            if (quote.is_none()
                || chars
                    .peek()
                    .is_some_and(|(_, next)| matches!(*next, '$' | '`' | '"' | '\\' | '\n')))
                && chars.next().is_none()
            {
                return true;
            }
            continue;
        }
        if ch == '"' {
            quote = if quote.is_some() { None } else { Some(ch) };
        } else if ch == '\'' && quote.is_none() {
            quote = Some(ch);
        } else if ch == '`'
            || ch == '$' && (parameters || chars.peek().is_some_and(|(_, next)| *next == '('))
            || (quote.is_none()
                && (matches!(
                    ch,
                    ';' | '|' | '&' | '<' | '>' | '(' | ')' | '*' | '?' | '[' | '{'
                ) || ch == '~' && at == 0))
        {
            return true;
        }
    }
    quote.is_some()
}

/// A native edit cannot account for mappings hidden in shell control flow or scripts.
fn river_source_complete(source: &str, bindings: &[Binding]) -> bool {
    let mut bindings = bindings.iter().peekable();
    for (index, line) in source.lines().enumerate() {
        let text = line.trim();
        if text.is_empty() || text.starts_with('#') {
            continue;
        }
        let spans = words(text);
        let Some(end) = spans.last().map(|span| span.end) else {
            continue;
        };
        let native = crate::compositor::shell_words(&text[..end]);
        if native.len() != spans.len()
            || spans
                .iter()
                .any(|span| dynamic_shell_word(&text[span.clone()], false))
        {
            return false;
        }
        if bindings.next_if(|row| row.line == index + 1).is_some() {
            continue;
        }
        if spans
            .iter()
            .any(|span| computed_shell_word(&text[span.clone()]))
        {
            return false;
        }
        let Some(command) = native.first() else {
            return false;
        };
        if command.rsplit('/').next() == Some("riverctl") {
            // Spawned programs can install bindings of their own. Unmap changes
            // the effective source identity just as an unparsed map would.
            if native
                .get(1)
                .is_some_and(|verb| !matches!(verb.as_str(), "map" | "unmap" | "spawn"))
            {
                continue;
            }
            return false;
        }
        let assignments = if command == "export" {
            &native[1..]
        } else {
            &native[..]
        };
        if !assignments.is_empty()
            && assignments.iter().all(|word| {
                word.split_once('=').is_some_and(|(name, _)| {
                    !name.is_empty()
                        && name
                            .chars()
                            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
                })
            })
        {
            continue;
        }
        return false;
    }
    true
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
    fn oversized_line_binding_lists_stop_at_the_parser_budget() {
        for (target, line) in [
            (Target::River, "riverctl map normal Super Q close\n"),
            (Target::Mango, "bind=SUPER,Q,killclient\n"),
            (Target::Hyprland, "bind=SUPER,Q,exec,foot\n"),
            (Target::Sway, "bindsym Mod4+q kill\n"),
        ] {
            let source = line.repeat(30_000);
            assert!(Lines(target).parse(&source).is_err(), "{target:?}");
        }
    }

    #[test]
    fn river_arguments_keep_literal_quotes_but_expanded_values_are_read_only() {
        for (argument, computed) in [
            ("\"$terminal\"", true),
            ("$(printf foot)", true),
            ("'$terminal'", false),
            (r#""\$terminal""#, false),
            ("'foot; keep|literal'", false),
            ("*.sh", true),
        ] {
            let source = format!("riverctl map 'normal' Super 'Q' spawn {argument}\n");
            let row = Lines(Target::River).parse(&source).unwrap().remove(0);
            assert_eq!(row.scope, "normal");
            assert_eq!(row.read_only.is_some(), computed, "{source}");
        }
        assert_eq!(
            crate::compositor::shell_words(r#""foot\path" "\$literal""#),
            [r"foot\path", "$literal"]
        );
    }

    #[test]
    fn sway_blocks_keep_shared_flags_locked_and_continuations_stay_in_the_source() {
        let source = "bindsym --locked {\n Mod4+q kill\n}\nbindsym Mod4+w exec \\\n foot\nbindsym Mod4+e kill\n";
        let rows = Lines(Target::Sway).parse(source).unwrap();
        assert_eq!(rows.len(), 3);
        assert!(
            rows[0]
                .fields
                .iter()
                .any(|field| field.value == "--locked" && field.read_only.is_some())
        );
        assert_eq!(rows[1].read_only, Some(crate::keybinds::SOURCE_ONLY));
        assert!(rows[2].read_only.is_none());
        let mut values: Vec<_> = rows[0]
            .fields
            .iter()
            .map(|field| field.value.clone())
            .collect();
        values[0] = "Mod4+a".into();
        let next = crate::keybinds::patch(source, &rows[0], &values).unwrap();
        assert!(next.starts_with("bindsym --locked {\n Mod4+a kill"));
    }

    #[test]
    fn hyprland_doubled_hashes_remain_inside_the_replaced_argument_span() {
        let source = "bind = SUPER, T, exec, kitty --title=##work # keep\n";
        let row = Lines(Target::Hyprland).parse(source).unwrap().remove(0);
        assert_eq!(row.fields.last().unwrap().value, "kitty --title=##work");
        let mut values: Vec<_> = row.fields.iter().map(|field| field.value.clone()).collect();
        *values.last_mut().unwrap() = "foot".into();
        assert_eq!(
            crate::keybinds::patch(source, &row, &values).unwrap(),
            "bind = SUPER, T, exec, foot # keep\n"
        );
    }

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
