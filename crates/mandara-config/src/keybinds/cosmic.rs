//! Default COSMIC shortcuts are overridden in the user's custom map.

use super::{Binding, Encoding};
use anyhow::{Result, ensure};
use mandara_i18n::t;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Identity(u8, u32);

pub(crate) fn keysym(name: &str) -> u32 {
    use xkbcommon::xkb;
    let exact = xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS).raw();
    if exact == 0 {
        xkb::keysym_from_name(name, xkb::KEYSYM_CASE_INSENSITIVE).raw()
    } else {
        exact
    }
}

/// COSMIC compares modifiers and keysym; descriptions and keycodes do not
/// participate in Binding's native equality implementation.
pub fn identity(row: &Binding) -> Result<Identity> {
    let modifiers = row
        .fields
        .iter()
        .find(|field| field.label == "Modifiers")
        .ok_or_else(|| anyhow::anyhow!("missing COSMIC modifiers"))?;
    let names: Vec<super::formats::cosmic::Modifier> = ron::from_str(&modifiers.value)?;
    let mut mask = 0;
    for name in names {
        mask |= name.mask();
    }
    let key = row
        .fields
        .iter()
        .find(|field| field.label == "Key")
        .map(|field| ron::from_str::<String>(&field.value))
        .transpose()?;
    let symbol = key.as_deref().map_or(0, keysym);
    Ok(Identity(mask, symbol))
}

fn entry(row: &Binding, disabled: bool) -> Result<String> {
    let mut result = String::from("(");
    for (index, field) in row
        .fields
        .iter()
        .filter(|field| field.label != "Action")
        .enumerate()
    {
        if index != 0 {
            result.push_str(", ");
        }
        result.push_str(match field.label {
            "Modifiers" => "modifiers",
            "Key" => "key",
            "Keycode" => "keycode",
            "Description" => "description",
            _ => anyhow::bail!("unknown COSMIC field"),
        });
        result.push_str(": ");
        result.push_str(&Encoding::RonValue.encode(&field.value)?);
    }
    result.push_str("): ");
    if disabled {
        result.push_str("Disable");
    } else {
        let action = row
            .fields
            .iter()
            .find(|field| field.label == "Action")
            .ok_or_else(|| anyhow::anyhow!("missing COSMIC action"))?;
        result.push_str(&Encoding::RonValue.encode(&action.value)?);
    }
    Ok(result)
}

/// Append only overrides. A moved default gets Disable at its original chord;
/// installed defaults keep receiving package updates.
pub fn override_default(custom: &str, before: &Binding, after: &Binding) -> Result<String> {
    let rows = super::parse(
        crate::compositor::Target::Cosmic,
        std::path::Path::new("custom"),
        custom,
    )?;
    let old = identity(before)?;
    let new = identity(after)?;
    for row in &rows {
        let key = identity(row)?;
        ensure!(
            key != old && key != new,
            "{}",
            t("A custom shortcut already uses this key. Edit that shortcut instead.")
        );
    }
    // Find the map's closing brace through RON's borrowed top-level span, so
    // trailing comments and whitespace remain byte-for-byte intact.
    let close = map_end(custom)?;
    let mut added = String::new();
    if let Some(last) = rows.last().and_then(|row| row.fields.last()) {
        let tail = ron::Deserializer::from_str(&custom[last.range.end..close])?;
        if !tail.remainder().starts_with(',') {
            added.push(',');
        }
    }
    added.push('\n');
    if old != new {
        added.push_str("    ");
        added.push_str(&entry(before, true)?);
        added.push_str(",\n");
    }
    added.push_str("    ");
    added.push_str(&entry(after, false)?);
    added.push_str(",\n");
    let mut result = String::with_capacity(custom.len() + added.len());
    result.push_str(&custom[..close]);
    result.push_str(&added);
    result.push_str(&custom[close..]);
    super::parse(
        crate::compositor::Target::Cosmic,
        std::path::Path::new("custom"),
        &result,
    )?;
    Ok(result)
}

fn map_end(source: &str) -> Result<usize> {
    use serde::Deserialize as _;
    let mut parser = ron::Deserializer::from_str(source)?;
    serde::de::IgnoredAny::deserialize(&mut parser)?;
    let close = source
        .len()
        .checked_sub(parser.remainder().len() + 1)
        .ok_or_else(|| anyhow::anyhow!("invalid COSMIC map span"))?;
    ensure!(
        source.as_bytes().get(close) == Some(&b'}'),
        "invalid COSMIC map span"
    );
    Ok(close)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compositor::Target;

    #[test]
    fn native_key_resolution_preserves_exact_case_and_retries_unknown_spellings() {
        assert_eq!(keysym("return"), keysym("Return"));
        assert_ne!(keysym("a"), keysym("A"));
        assert_eq!(keysym("not-a-keysym"), 0);
        let parse = |key| {
            super::super::parse(
                Target::Cosmic,
                std::path::Path::new("custom"),
                &format!("{{(modifiers:[Super],key:\"{key}\"): Close}}"),
            )
        };
        assert_eq!(
            identity(&parse("return").unwrap()[0]).unwrap(),
            identity(&parse("Return").unwrap()[0]).unwrap()
        );
        assert!(parse("not-a-keysym").is_err());
    }

    #[test]
    fn default_overrides_preserve_trailing_comments_and_separators() {
        let row = super::super::parse(
            Target::Cosmic,
            std::path::Path::new("defaults"),
            "{(modifiers:[Super],key:\"q\"): Close}",
        )
        .unwrap()
        .remove(0);
        for tail in [
            ", // keep\n",
            ", /* keep */",
            " /* before */ , // after\n",
            " // keep\n",
            " /* keep */",
        ] {
            let custom = format!("{{(modifiers:[Alt],key:\"F12\"): Close{tail}}}");
            let next = override_default(&custom, &row, &row).unwrap();
            assert!(next.starts_with(&custom[..custom.len() - 1]));
            assert_eq!(
                super::super::parse(Target::Cosmic, std::path::Path::new("custom"), &next)
                    .unwrap()
                    .len(),
                2
            );
        }
    }

    #[test]
    fn moved_defaults_are_disabled_and_overridden_without_copying_the_catalog() {
        let defaults = "{(modifiers: [Super], key: \"q\"): Close}";
        let before =
            super::super::parse(Target::Cosmic, std::path::Path::new("defaults"), defaults)
                .unwrap()
                .remove(0);
        let mut after = before.clone();
        after.fields[1].value = "\"w\"".into();
        let custom = "// keep\n{(modifiers:[Alt],key:\"F12\"): Spawn(\"foot\")} // trailing }\n";
        let next = override_default(custom, &before, &after).unwrap();
        assert!(next.starts_with("// keep\n{(modifiers:[Alt],key:\"F12\"): Spawn(\"foot\"),"));
        assert!(next.ends_with("} // trailing }\n"));
        let rows =
            super::super::parse(Target::Cosmic, std::path::Path::new("custom"), &next).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[1].fields.last().unwrap().value, "Disable");
        assert_eq!(rows[2].chord.as_deref(), Some("super+w"));
        assert!(override_default(&next, &before, &after).is_err());
        let same = override_default("{}", &before, &before).unwrap();
        assert_eq!(
            super::super::parse(Target::Cosmic, std::path::Path::new("custom"), &same)
                .unwrap()
                .len(),
            1
        );
    }
}
