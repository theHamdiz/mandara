//! Qt PortableText sequences used by KGlobalAccel's native D-Bus API.

use anyhow::{Result, ensure};
use mandara_i18n::t;

/// QKeySequence has four strokes; each stroke combines a key with Qt modifiers.
const STROKES: usize = 4;
const F1: i32 = 0x01000030;
const MODIFIERS: [(&str, i32); 5] = [
    ("Shift", 0x02000000),
    ("Ctrl", 0x04000000),
    ("Alt", 0x08000000),
    ("Meta", 0x10000000),
    ("Num", 0x20000000),
];

/// Compare native Qt sequences with shell bindings using Qt's XKB mapping.
pub fn keybind(binding: &crate::compositor::Keybind) -> Option<i32> {
    use xkbcommon::xkb;
    let name = binding.keysym();
    let symbol = xkb::keysym_from_name(&name, xkb::KEYSYM_CASE_INSENSITIVE);
    let canonical = xkb::keysym_get_name(symbol);
    let named = super::qt_xkb::KEYS
        .iter()
        .find(|(name, _)| canonical == *name)
        .map(|(_, key)| *key);
    let function = canonical
        .strip_prefix('F')
        .and_then(|number| number.parse::<i32>().ok())
        .filter(|number| (1..=35).contains(number))
        .map(|number| F1 + number - 1);
    let character = || {
        let mut typed = binding.key.trim().chars();
        let character = match (typed.next(), typed.next()) {
            (Some(character), None) => character,
            _ => char::from_u32(xkb::keysym_to_utf32(symbol))?,
        };
        if character.is_control() {
            return None;
        }
        let mut upper = character.to_uppercase();
        let key = upper.next()?;
        Some(if upper.next().is_none() {
            key
        } else {
            character
        } as i32)
    };
    let mut key = named.or(function).or_else(character)?;
    for modifier in &binding.mods {
        key |= match crate::compositor::normalize_mod(modifier)? {
            "shift" => MODIFIERS[0].1,
            "ctrl" => MODIFIERS[1].1,
            "alt" => MODIFIERS[2].1,
            "super" => MODIFIERS[3].1,
            _ => return None,
        };
    }
    Some(key)
}

pub fn sequence(value: &str) -> Result<[i32; STROKES]> {
    let mut result = [0; STROKES];
    let mut rest = value.trim();
    if rest.is_empty() || rest.eq_ignore_ascii_case("none") {
        return Ok(result);
    }
    let mut index = 0;
    while !rest.is_empty() {
        ensure!(
            index < STROKES,
            "{}",
            t("A shortcut sequence can contain at most four keys.")
        );
        // A final comma is the comma key. In a doubled comma, the first belongs
        // to the key and the second separates the next stroke (Qt PortableText).
        let split = rest.find(',').filter(|at| *at + 1 < rest.len()).map(|at| {
            if rest.as_bytes().get(at + 1) == Some(&b',') {
                at + 1
            } else {
                at
            }
        });
        let (part, next) =
            split.map_or((rest, ""), |at| (&rest[..at], rest[at + 1..].trim_start()));
        result[index] = stroke(part.trim())?;
        index += 1;
        rest = next;
        ensure!(
            split.is_none() || !rest.is_empty(),
            "{}",
            t("Enter a complete shortcut sequence.")
        );
    }
    Ok(result)
}

fn stroke(value: &str) -> Result<i32> {
    let mut rest = value;
    let mut modifiers = 0;
    while let Some(at) = rest.find('+') {
        if rest == "+" {
            break;
        }
        let name = rest[..at].trim();
        let flag = MODIFIERS
            .iter()
            .find(|(modifier, _)| name.eq_ignore_ascii_case(modifier))
            .map(|(_, flag)| *flag)
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "{}",
                    t("Use Qt shortcut names such as Meta, Ctrl, Alt, Shift and Num.")
                )
            })?;
        modifiers |= flag;
        rest = rest[at + 1..].trim_start();
    }
    let mut chars = rest.chars();
    let first = chars
        .next()
        .ok_or_else(|| anyhow::anyhow!("{}", t("Enter a complete shortcut sequence.")))?;
    let key = if chars.next().is_none() && first.len_utf16() == 1 {
        let mut upper = first.to_uppercase();
        let key = upper.next().unwrap_or(first);
        if upper.next().is_none() {
            key as i32
        } else {
            first as i32
        }
    } else if let Some(number) = rest
        .strip_prefix(['f', 'F'])
        .and_then(|value| value.parse::<i32>().ok())
        .filter(|number| (1..=35).contains(number))
    {
        F1 + number - 1
    } else {
        super::qt_keys::NAMES
            .iter()
            .find(|(name, _)| rest.eq_ignore_ascii_case(name))
            .map(|(_, key)| *key)
            .ok_or_else(|| anyhow::anyhow!("{}: {rest}", t("Unknown Qt shortcut key")))?
    };
    Ok(modifiers | key)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shell_keys_match_qt_punctuation_aliases_media_and_sequence_prefixes() {
        use crate::compositor::{BindAction, Keybind};
        for (native, shell, mods) in [
            ("Ctrl+,", ",", &["ctrl"][..]),
            ("Ctrl++", "+", &["ctrl"][..]),
            ("Meta+Page Up", "Page_Up", &["super"][..]),
            ("Ctrl+K, Ctrl+C", "k", &["ctrl"][..]),
            ("Volume Up", "XF86AudioRaiseVolume", &[][..]),
            ("Meta+F35", "F35", &["super"][..]),
            ("Meta+أ", "أ", &["super"][..]),
        ] {
            let binding = Keybind::new(mods, shell, BindAction::Spawn("true".into()));
            assert_eq!(
                keybind(&binding),
                Some(sequence(native).unwrap()[0]),
                "{native}"
            );
        }
    }
    #[test]
    fn portable_sequences_cover_punctuation_alternatives_media_and_native_limits() {
        assert_eq!(sequence("Meta+Q").unwrap(), [0x10000051, 0, 0, 0]);
        assert_eq!(
            sequence("Ctrl+K, Ctrl+C").unwrap(),
            [0x0400004b, 0x04000043, 0, 0]
        );
        assert_eq!(
            sequence("Ctrl+,, Shift++").unwrap(),
            [0x0400002c, 0x0200002b, 0, 0]
        );
        assert_eq!(sequence("Alt+Num+F35").unwrap()[0], 0x29000052);
        assert_eq!(sequence("none").unwrap(), [0; 4]);
        assert_eq!(sequence("ß").unwrap(), [0xdf, 0, 0, 0]);
        for (name, key) in super::super::qt_keys::NAMES {
            assert_eq!(sequence(name).unwrap()[0], *key, "{name}");
        }
        for invalid in ["Meta+", "Super+Q", "Ctrl+invented", "A,B,C,D,E"] {
            assert!(sequence(invalid).is_err(), "{invalid}");
        }
    }
}
