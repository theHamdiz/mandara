//! Niri bindings and action arguments retain their original KDL source spans.

use anyhow::Result;
use mandara_i18n::tk;

use crate::keybinds::{Binding, Encoding, ShortcutFormat, field, line, normalize};

pub(super) struct Niri;

impl ShortcutFormat for Niri {
    fn parse(&self, source: &str) -> Result<Vec<Binding>> {
        let doc = kdl::KdlDocument::parse(source)?;
        let mut result = Vec::new();
        for group in doc
            .nodes()
            .iter()
            .filter(|node| node.name().value() == "binds")
        {
            let Some(binds) = group.children() else {
                continue;
            };
            for bind in binds.nodes() {
                let span = bind.name().span();
                let mut fields = vec![field(
                    tk("Shortcut"),
                    bind.name().value(),
                    span.offset()..span.offset() + span.len(),
                    Encoding::KdlName,
                )];
                entries(source, bind, &mut fields);
                if let Some(actions) = bind.children() {
                    for action in actions.nodes() {
                        let span = action.name().span();
                        fields.push(field(
                            tk("Action"),
                            action.name().value(),
                            span.offset()..span.offset() + span.len(),
                            Encoding::KdlName,
                        ));
                        entries(source, action, &mut fields);
                    }
                }
                result.push(Binding {
                    line: line(source, span.offset()),
                    scope: "binds".into(),
                    chord: normalize(bind.name().value()),
                    fields,
                    read_only: None,
                });
            }
        }
        Ok(result)
    }
}

fn entries(source: &str, node: &kdl::KdlNode, fields: &mut Vec<crate::keybinds::Field>) {
    for entry in node.entries() {
        let span = entry.span();
        let mut range = span.offset()..span.offset() + span.len();
        if let Some(name) = entry.name() {
            let name_span = name.span();
            let from = name_span.offset() + name_span.len();
            if let Some(at) = source[from..range.end].find('=') {
                range.start = from + at + 1;
                range.start +=
                    source[range.clone()].len() - source[range.clone()].trim_start().len();
            }
        }
        let label = if entry.name().is_some() {
            tk("Option")
        } else {
            tk("Argument")
        };
        let (value, encoding) = if entry.ty().is_none()
            && let Some(value) = entry.value().as_string()
        {
            (value.to_owned(), Encoding::KdlString)
        } else {
            (source[range.clone()].to_owned(), Encoding::KdlValue)
        };
        let mut field = field(label, value, range, encoding);
        if let Some(name) = entry.name() {
            field = field.named(name.value());
        }
        fields.push(field);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn niri_edits_one_argument_and_preserves_properties_comments_and_sibling_binds() {
        let text = "input { mod-key \"Super\"; }\nbinds {\n // keep\n Mod+T cooldown-ms=150 { spawn \"foot\" \"--title=hello\"; }\n Super+Q { close-window; }\n}\n";
        let binds = Niri.parse(text).unwrap();
        assert_eq!(binds.len(), 2);
        assert!(binds[0].chord.is_none());
        let mut values: Vec<_> = binds[0].fields.iter().map(|f| f.value.clone()).collect();
        values[3] = "kitty".into();
        let next = crate::keybinds::patch(text, &binds[0], &values).unwrap();
        assert_eq!(next, text.replacen("\"foot\"", "\"kitty\"", 1));
        assert_eq!(Niri.parse(&next).unwrap().len(), 2);
    }
}
