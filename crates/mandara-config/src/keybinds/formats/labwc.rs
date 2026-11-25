//! XML attributes and leaf values are edited without rewriting the document.

use anyhow::{Context as _, Result, ensure};
use mandara_i18n::tk;

use crate::keybinds::{Binding, Encoding, ShortcutFormat, field, line, normalize};

pub(super) struct Labwc;

/// Preserve keyboard XML and its namespaces without copying startup/window hooks.
pub fn keyboard(source: &str) -> Result<String> {
    let options = roxmltree::ParsingOptions {
        allow_dtd: false,
        nodes_limit: crate::keybinds::MAX_BINDINGS as u32 * 32,
        ..Default::default()
    };
    let doc = roxmltree::Document::parse_with_options(source, options)?;
    let root = doc.root_element();
    let mut result = String::from("<labwc_config");
    for namespace in root.namespaces() {
        result.push_str(" xmlns");
        if let Some(name) = namespace.name() {
            result.push(':');
            result.push_str(name);
        }
        result.push_str("=\"");
        result.push_str(&Encoding::XmlAttribute.encode(namespace.uri())?);
        result.push('"');
    }
    result.push('>');
    for node in root.children().filter(|node| node.has_tag_name("keyboard")) {
        result.push_str(&source[node.range()]);
    }
    result.push_str("</labwc_config>\n");
    Ok(result)
}

impl ShortcutFormat for Labwc {
    fn parse(&self, source: &str) -> Result<Vec<Binding>> {
        let options = roxmltree::ParsingOptions {
            allow_dtd: false,
            nodes_limit: crate::keybinds::MAX_BINDINGS as u32 * 32,
            ..Default::default()
        };
        let doc = roxmltree::Document::parse_with_options(source, options)?;
        let mut result = Vec::new();
        for node in doc
            .root_element()
            .children()
            .filter(|node| node.has_tag_name("keyboard"))
            .flat_map(|keyboard| keyboard.children())
            .filter(|node| node.has_tag_name("keybind"))
        {
            let mut fields = Vec::new();
            for attribute in node.attributes() {
                let label = if attribute.name() == "key" {
                    tk("Shortcut")
                } else {
                    tk("Option")
                };
                fields.push(
                    field(
                        label,
                        attribute.value(),
                        attribute.range_value(),
                        Encoding::XmlAttribute,
                    )
                    .named(attribute.name()),
                );
            }
            for value in binding_values(node) {
                for attribute in value.attributes() {
                    let label = if value.has_tag_name("action") && attribute.name() == "name" {
                        tk("Action")
                    } else {
                        tk("Argument")
                    };
                    fields.push(
                        field(
                            label,
                            attribute.value(),
                            attribute.range_value(),
                            Encoding::XmlAttribute,
                        )
                        .named(attribute.name()),
                    );
                }
                if value.children().all(|n| n.is_text() || n.is_comment())
                    && value.children().any(|child| child.is_text())
                {
                    let label = match (value.parent(), value.tag_name().name()) {
                        (Some(parent), "key") if parent == node => tk("Shortcut"),
                        (Some(parent), "name") if parent.has_tag_name("action") => tk("Action"),
                        _ => tk("Argument"),
                    };
                    let mut entry = field(
                        label,
                        value
                            .children()
                            .filter(|child| child.is_text())
                            .filter_map(|child| child.text())
                            .collect::<String>(),
                        leaf_span(source, value)?,
                        Encoding::XmlText,
                    )
                    .named(value.tag_name().name());
                    if value.children().any(|child| child.is_comment()) {
                        entry.read_only = Some(crate::keybinds::SOURCE_ONLY);
                    }
                    fields.push(entry);
                }
            }
            ensure!(
                fields.len() <= crate::keybinds::MAX_BINDINGS,
                "XML shortcut has too many values"
            );
            let combo = fields
                .iter()
                .rev()
                .find(|field| field.label == "Shortcut")
                .map(|field| xml_chord(&field.value));
            result.push(Binding {
                line: line(source, node.range().start),
                scope: "keyboard".into(),
                chord: combo.as_deref().and_then(normalize),
                fields,
                read_only: None,
            });
        }
        Ok(result)
    }
}

/// Labwc reads each keyboard binding once; nested keybind subtrees are ignored.
fn binding_values<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
) -> impl Iterator<Item = roxmltree::Node<'a, 'input>> {
    std::iter::successors(node.first_element_child(), move |value| {
        if !value.has_tag_name("keybind")
            && let Some(child) = value.first_element_child()
        {
            return Some(child);
        }
        let mut current = *value;
        loop {
            if let Some(sibling) = current.next_sibling_element() {
                return Some(sibling);
            }
            let parent = current.parent_element()?;
            if parent == node {
                return None;
            }
            current = parent;
        }
    })
    .filter(|value| !value.has_tag_name("keybind"))
}

fn leaf_span(source: &str, node: roxmltree::Node<'_, '_>) -> Result<std::ops::Range<usize>> {
    let range = node.range();
    let raw = &source[range.clone()];
    let mut quote = None;
    let begin = raw
        .char_indices()
        .find_map(|(at, ch)| {
            if let Some(expected) = quote {
                if ch == expected {
                    quote = None;
                }
            } else if ch == '\'' || ch == '"' {
                quote = Some(ch);
            } else if ch == '>' {
                return Some(at + 1);
            }
            None
        })
        .context("XML value has no opening boundary")?;
    let end = raw
        .rfind("</")
        .context("XML value has no closing boundary")?;
    ensure!(begin <= end, "invalid XML value range");
    Ok(range.start + begin..range.start + end)
}

fn xml_chord(value: &str) -> String {
    value
        .split('-')
        .map(|part| match part {
            "W" => "Super",
            "C" => "Ctrl",
            "A" => "Alt",
            "S" => "Shift",
            _ => part,
        })
        .collect::<Vec<_>>()
        .join("+")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn element_roles_and_merged_cdata_use_complete_editable_leaf_spans() {
        let text = "<labwc_config><keyboard><!-- keep --><keybind><key>W-q</key><action><name>Execute</name><command note='>'>\n<![CDATA[foot]]>\n</command></action></keybind></keyboard></labwc_config>";
        let row = Labwc.parse(text).unwrap().remove(0);
        assert_eq!(row.chord.as_deref(), Some("super+q"));
        assert!(
            row.fields
                .iter()
                .any(|field| field.label == "Action" && field.value == "Execute")
        );
        let mut values: Vec<_> = row.fields.iter().map(|field| field.value.clone()).collect();
        let index = row
            .fields
            .iter()
            .position(|field| field.name.as_deref() == Some("command"))
            .unwrap();
        values[index] = "kitty".into();
        let next = crate::keybinds::patch(text, &row, &values).unwrap();
        assert_eq!(next, text.replace("\n<![CDATA[foot]]>\n", "kitty"));
        assert_eq!(Labwc.parse(&next).unwrap()[0].fields[index].value, "kitty");
    }

    #[test]
    fn comments_inside_a_leaf_are_preserved_when_another_field_changes() {
        let text = "<labwc_config><keyboard><keybind key='W-q'><action name='Execute'><command>foot<!--keep--> --title=X</command></action></keybind></keyboard></labwc_config>";
        let row = Labwc.parse(text).unwrap().remove(0);
        assert!(row.fields.last().unwrap().read_only.is_some());
        let mut values: Vec<_> = row.fields.iter().map(|field| field.value.clone()).collect();
        values[0] = "W-w".into();
        assert!(
            crate::keybinds::patch(text, &row, &values)
                .unwrap()
                .contains("foot<!--keep--> --title=X")
        );
        *values.last_mut().unwrap() = "kitty".into();
        assert!(crate::keybinds::patch(text, &row, &values).is_err());
    }

    #[test]
    fn the_private_check_keeps_keyboard_namespaces_and_omits_other_hooks() {
        let source = "<labwc_config xmlns='urn:test' xmlns:x='urn:keys'><windowRules><windowRule><action name='Execute' command='never-run'/></windowRule></windowRules><keyboard><!-- keep --><keybind key='W-q'><action name='Close'/></keybind></keyboard></labwc_config>";
        let sample = keyboard(source).unwrap();
        assert!(!sample.contains("never-run") && sample.contains("<!-- keep -->"));
        let original = Labwc.parse(source).unwrap();
        let parsed = Labwc.parse(&sample).unwrap();
        assert_eq!(original[0].fields.len(), parsed[0].fields.len());
        for (before, after) in original[0].fields.iter().zip(&parsed[0].fields) {
            assert_eq!(
                (&before.label, &before.name, &before.value, before.encoding),
                (&after.label, &after.name, &after.value, after.encoding)
            );
        }
        assert!(roxmltree::Document::parse(&sample).is_ok());
    }

    #[test]
    fn labwc_keeps_comments_namespaces_and_unrecognized_chains_unchanged() {
        let text = "<labwc_config xmlns='urn:test'><theme><name>Mine</name></theme><!-- keep --><keyboard><keybind key='W-Return' onRelease='yes'><action name='Execute'><command>foot &amp; safe</command></action></keybind><keybind key='W-x'><keybind key='q'><action name='Close'/></keybind></keybind></keyboard></labwc_config>";
        let binds = Labwc.parse(text).unwrap();
        assert_eq!(binds.len(), 2);
        assert_eq!(binds[0].chord.as_deref(), Some("super+return"));
        assert_eq!(binds[1].scope, "keyboard");
        assert_eq!(binds[1].fields.len(), 1);
        assert_eq!(binds[1].fields[0].label, "Shortcut");
        assert_eq!(binds[1].fields[0].value, "W-x");
        let renamed = crate::keybinds::patch(text, &binds[1], &["W-z".into()]).unwrap();
        assert_eq!(renamed, text.replace("key='W-x'", "key='W-z'"));
        assert!(renamed.contains("<keybind key='q'><action name='Close'/></keybind>"));
        let mut values: Vec<_> = binds[0].fields.iter().map(|f| f.value.clone()).collect();
        let at = binds[0]
            .fields
            .iter()
            .position(|f| f.name.as_deref() == Some("command"))
            .unwrap();
        values[at] = "kitty --title='Studio'".into();
        let next = crate::keybinds::patch(text, &binds[0], &values).unwrap();
        assert_eq!(
            next,
            text.replace("foot &amp; safe", "kitty --title=&apos;Studio&apos;")
        );
        assert_eq!(Labwc.parse(&next).unwrap().len(), 2);
    }

    #[test]
    fn nested_keybinds_never_supply_rows_chords_or_parent_action_fields() {
        let text = "<labwc_config><keyboard><keybind key='W-q'><action name='Execute'><command>foot</command></action><keybind key='W-w'><action name='Close'/></keybind><action name='Raise'/></keybind></keyboard></labwc_config>";
        let bindings = Labwc.parse(text).unwrap();
        assert_eq!(bindings.len(), 1);
        let binding = &bindings[0];
        assert_eq!(binding.chord.as_deref(), Some("super+q"));
        assert_eq!(
            binding
                .fields
                .iter()
                .map(|field| field.value.as_str())
                .collect::<Vec<_>>(),
            ["W-q", "Execute", "foot", "Raise"]
        );
        let next = crate::keybinds::patch(
            text,
            binding,
            &[
                "W-e".into(),
                "Execute".into(),
                "kitty".into(),
                "Raise".into(),
            ],
        )
        .unwrap();
        assert_eq!(next, text.replace("W-q", "W-e").replace("foot", "kitty"));
    }

    #[test]
    fn deeply_nested_keybinds_retain_only_the_native_outer_binding() {
        let mut source = String::from("<labwc_config><keyboard>");
        let opening = format!("<keybind key='{}'>", "a".repeat(64));
        for _ in 0..4096 {
            source.push_str(&opening);
        }
        for _ in 0..4096 {
            source.push_str("</keybind>");
        }
        source.push_str("</keyboard></labwc_config>");
        assert!(source.len() < crate::keybinds::MAX_FILE_BYTES);
        let bindings = Labwc.parse(&source).unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].scope, "keyboard");
        assert_eq!(bindings[0].fields.len(), 1);
        assert_eq!(bindings[0].fields[0].value.len(), 64);
    }

    #[test]
    fn external_entities_are_not_loaded() {
        assert!(
            Labwc
                .parse("<!DOCTYPE x [<!ENTITY secret SYSTEM 'file:///etc/passwd'>]><x>&secret;</x>")
                .is_err()
        );
    }
}
