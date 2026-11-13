//! XML attributes and leaf values are edited without rewriting the document.

use anyhow::{Result, ensure};
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
        for node in doc.descendants().filter(|n| {
            n.has_tag_name("keybind") && n.ancestors().any(|a| a.has_tag_name("keyboard"))
        }) {
            // Chained keybinds retain their parent as an explicit scope.
            let mut scope = String::from("keyboard");
            for parent in node
                .ancestors()
                .skip(1)
                .filter(|p| p.has_tag_name("keybind"))
            {
                if let Some(key) = parent.attribute("key") {
                    scope.push('/');
                    scope.push_str(key);
                }
            }
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
            for value in node.descendants().skip(1).filter(|n| {
                n.is_element()
                    && n.ancestors().skip(1).find(|p| p.has_tag_name("keybind")) == Some(node)
            }) {
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
                if value.children().all(|n| n.is_text() || n.is_comment()) {
                    let texts: Vec<_> = value.children().filter(|n| n.is_text()).collect();
                    if let [text] = texts.as_slice() {
                        fields.push(
                            field(
                                tk("Argument"),
                                text.text().unwrap_or_default(),
                                text.range(),
                                Encoding::XmlText,
                            )
                            .named(value.tag_name().name()),
                        );
                    }
                }
            }
            ensure!(
                fields.len() <= crate::keybinds::MAX_BINDINGS,
                "XML shortcut has too many values"
            );
            let combo = node.attribute("key").map(xml_chord);
            result.push(Binding {
                line: line(source, node.range().start),
                scope,
                chord: combo.as_deref().and_then(normalize),
                fields,
                read_only: None,
            });
        }
        Ok(result)
    }
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
    fn labwc_keeps_comments_namespaces_chains_and_unrelated_settings() {
        let text = "<labwc_config xmlns='urn:test'><theme><name>Mine</name></theme><!-- keep --><keyboard><keybind key='W-Return' onRelease='yes'><action name='Execute'><command>foot &amp; safe</command></action></keybind><keybind key='W-x'><keybind key='q'><action name='Close'/></keybind></keybind></keyboard></labwc_config>";
        let binds = Labwc.parse(text).unwrap();
        assert_eq!(binds.len(), 3);
        assert_eq!(binds[0].chord.as_deref(), Some("super+return"));
        assert_eq!(binds[2].scope, "keyboard/W-x");
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
        assert_eq!(Labwc.parse(&next).unwrap().len(), 3);
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
