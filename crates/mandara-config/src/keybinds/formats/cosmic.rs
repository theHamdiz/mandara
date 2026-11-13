//! COSMIC's RON map preserves native actions and every untouched source byte.

use std::{fmt, ops::Range};

use anyhow::{Result, ensure};
use mandara_i18n::tk;
use ron::value::RawValue;
use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};

use crate::keybinds::{Binding, Encoding, ShortcutFormat, field, line, normalize};

pub(super) struct Cosmic;

#[derive(Deserialize)]
enum Modifier {
    Super,
    Ctrl,
    Alt,
    Shift,
}
impl Modifier {
    fn name(&self) -> &'static str {
        match self {
            Self::Super => "super",
            Self::Ctrl => "ctrl",
            Self::Alt => "alt",
            Self::Shift => "shift",
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBinding<'a> {
    #[serde(borrow)]
    modifiers: &'a RawValue,
    #[serde(default, borrow, deserialize_with = "optional_raw")]
    key: Option<&'a RawValue>,
    #[serde(default, borrow, deserialize_with = "optional_raw")]
    keycode: Option<&'a RawValue>,
    #[serde(default, borrow, deserialize_with = "optional_raw")]
    description: Option<&'a RawValue>,
}

fn optional_raw<'de, D: Deserializer<'de>>(de: D) -> Result<Option<&'de RawValue>, D::Error> {
    <&RawValue>::deserialize(de).map(Some)
}

struct Pairs<'a>(Vec<(&'a RawValue, &'a RawValue)>);
impl<'de> Deserialize<'de> for Pairs<'de> {
    fn deserialize<D: Deserializer<'de>>(de: D) -> Result<Self, D::Error> {
        struct PairsVisitor;
        impl<'de> Visitor<'de> for PairsVisitor {
            type Value = Pairs<'de>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a COSMIC shortcut map")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut access: A) -> Result<Self::Value, A::Error> {
                let mut pairs = Vec::new();
                while let Some(pair) = access.next_entry::<&RawValue, &RawValue>()? {
                    if pairs.len() >= crate::keybinds::MAX_BINDINGS {
                        return Err(serde::de::Error::custom("too many shortcuts"));
                    }
                    pairs.push(pair);
                }
                Ok(Pairs(pairs))
            }
        }
        de.deserialize_map(PairsVisitor)
    }
}

fn span(source: &str, raw: &RawValue) -> Result<Range<usize>> {
    let value = raw.get_ron();
    let start = (value.as_ptr() as usize)
        .checked_sub(source.as_ptr() as usize)
        .ok_or_else(|| anyhow::anyhow!("RON value is not borrowed from its source"))?;
    let range = start
        ..start
            .checked_add(value.len())
            .ok_or_else(|| anyhow::anyhow!("RON span overflow"))?;
    ensure!(
        source.get(range.clone()) == Some(value),
        "invalid RON source span"
    );
    Ok(range)
}

impl ShortcutFormat for Cosmic {
    fn parse(&self, source: &str) -> Result<Vec<Binding>> {
        let Pairs(pairs) = ron::from_str::<Pairs<'_>>(source)?;
        let mut result = Vec::with_capacity(pairs.len());
        for (binding, action) in pairs {
            let raw = binding.into_rust::<RawBinding<'_>>()?;
            let modifiers = raw.modifiers.into_rust::<Vec<Modifier>>()?;
            let key = raw.key.map(|key| key.into_rust::<String>()).transpose()?;
            if let Some(key) = key.as_deref() {
                ensure!(
                    xkbcommon::xkb::keysym_from_name(key, xkbcommon::xkb::KEYSYM_NO_FLAGS).raw()
                        != 0,
                    "invalid COSMIC key: {key}"
                );
            }
            if let Some(code) = raw.keycode {
                let _: Option<u32> = code.into_rust()?;
            }
            if let Some(description) = raw.description {
                let _: Option<String> = description.into_rust()?;
            }
            let mut fields = vec![field(
                tk("Modifiers"),
                raw.modifiers.get_ron(),
                span(source, raw.modifiers)?,
                Encoding::RonValue,
            )];
            for (label, value) in [
                (tk("Key"), raw.key),
                (tk("Keycode"), raw.keycode),
                (tk("Description"), raw.description),
            ] {
                if let Some(value) = value {
                    fields.push(field(
                        label,
                        value.get_ron(),
                        span(source, value)?,
                        Encoding::RonValue,
                    ));
                }
            }
            fields.push(field(
                tk("Action"),
                action.get_ron(),
                span(source, action)?,
                Encoding::RonValue,
            ));
            let combo = key.map(|key| {
                format!(
                    "{}+{key}",
                    modifiers
                        .iter()
                        .map(Modifier::name)
                        .collect::<Vec<_>>()
                        .join("+")
                )
            });
            result.push(Binding {
                line: line(source, span(source, binding)?.start),
                scope: "custom".into(),
                chord: combo.as_deref().and_then(normalize),
                fields,
                read_only: None,
            });
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cosmic_preserves_native_action_variants_descriptions_and_comment_positions() {
        let text = "{\n // keep\n (modifiers: [Super], key: \"q\", description: Some(\"Close\")): Close,\n (modifiers: [Ctrl, Alt], key: \"Return\"): Spawn(\"foot\"),\n}\n";
        let binds = Cosmic.parse(text).unwrap();
        assert_eq!(binds.len(), 2);
        assert_eq!(binds[0].chord.as_deref(), Some("super+q"));
        let mut values: Vec<_> = binds[0].fields.iter().map(|f| f.value.clone()).collect();
        values[1] = "\"w\"".into();
        let next = crate::keybinds::patch(text, &binds[0], &values).unwrap();
        assert_eq!(next, text.replacen("\"q\"", "\"w\"", 1));
        assert_eq!(Cosmic.parse(&next).unwrap().len(), 2);
    }
}
