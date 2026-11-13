//! TOML shortcut tables use immutable spans; only edited keys/values change.

use anyhow::{Context as _, Result};
use mandara_i18n::tk;
use toml_edit::{Document, Item, Table, Value};

use crate::{
    compositor::Target,
    keybinds::{Binding, Encoding, ShortcutFormat, field, line, normalize},
};

pub(super) struct Toml(pub Target);

impl ShortcutFormat for Toml {
    fn parse(&self, source: &str) -> Result<Vec<Binding>> {
        let document = Document::parse(source)?;
        let mut bindings = Vec::new();
        let name = if self.0 == Target::Mango {
            "bind"
        } else {
            "keybinds"
        };
        if let Some(table) = document.get(name).and_then(Item::as_table) {
            read_table(self.0, source, table, name, &mut bindings)?;
        }
        Ok(bindings)
    }
}

fn read_table(
    target: Target,
    source: &str,
    table: &Table,
    scope: &str,
    out: &mut Vec<Binding>,
) -> Result<()> {
    for (key, item) in table.iter() {
        if let Some(child) = item.as_table() {
            read_table(target, source, child, &format!("{scope}.{key}"), out)?;
            continue;
        }
        let Some(value) = item.as_value() else {
            continue;
        };
        let key_span = table
            .key(key)
            .and_then(|k| k.span())
            .context("shortcut key has no source span")?;
        let custom_aqueous = target == Target::Aqueous && scope == "keybinds.custom";
        let key_is_chord = target != Target::Aqueous || custom_aqueous;
        let mut fields = vec![field(
            if key_is_chord {
                tk("Shortcut")
            } else {
                tk("Action")
            },
            key,
            key_span.clone(),
            Encoding::TomlString,
        )];
        if target == Target::Aqueous && !custom_aqueous {
            fields[0].read_only = Some(tk(
                "The compositor defines this action. Change its shortcut here.",
            ));
        }
        let mut chords = Vec::new();
        let values: Vec<&Value> = value
            .as_array()
            .map_or_else(|| vec![value], |array| array.iter().collect());
        for value in values {
            if let Some(text) = value.as_str() {
                let span = value.span().context("shortcut value has no source span")?;
                fields.push(field(
                    if key_is_chord {
                        tk("Action")
                    } else {
                        tk("Shortcut")
                    },
                    text,
                    span,
                    Encoding::TomlString,
                ));
                if !key_is_chord {
                    chords.push(text);
                }
            }
        }
        if fields.len() == 1 {
            continue;
        }
        let chord = if key_is_chord {
            normalize(key)
        } else if chords.len() == 1 {
            normalize(chords[0])
        } else {
            None
        };
        out.push(Binding {
            line: line(source, key_span.start),
            scope: scope.to_owned(),
            chord,
            fields,
            read_only: None,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn umbriel_and_mango_keep_modes_arrays_comments_and_unrelated_tables() {
        for (target, group) in [
            (Target::Umbriel, "keybinds"),
            (Target::Mango, "bind.resize.release"),
        ] {
            let text = format!(
                "# mine\n[{group}]\n'Super+T' = [ 'spawn foot', 'second action' ] # keep\n[other]\nvalue = 4\n"
            );
            let binds = Toml(target).parse(&text).unwrap();
            assert_eq!(binds.len(), 1);
            assert_eq!(binds[0].scope, group);
            let mut values: Vec<_> = binds[0].fields.iter().map(|f| f.value.clone()).collect();
            values[1] = "spawn kitty".into();
            let next = crate::keybinds::patch(&text, &binds[0], &values).unwrap();
            assert_eq!(next, text.replacen("'spawn foot'", "\"spawn kitty\"", 1));
        }
    }

    #[test]
    fn aqueous_edits_individual_alternate_shortcuts_and_custom_actions() {
        let text = "[keybinds]\ncycle_focus = ['Super+Tab', 'Alt+Tab']\n[keybinds.custom]\n'Super+T' = 'spawn:foot'\n";
        let binds = Toml(Target::Aqueous).parse(text).unwrap();
        assert_eq!(binds.len(), 2);
        assert_eq!(binds[0].fields[1].value, "Super+Tab");
        assert_eq!(binds[1].chord.as_deref(), Some("super+t"));
    }
}
