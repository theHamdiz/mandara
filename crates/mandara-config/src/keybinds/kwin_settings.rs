//! Compare native KConfig writes without accepting changes to unrelated entries.

use anyhow::{Context as _, Result, ensure};
use std::collections::{BTreeMap, BTreeSet, btree_map::Entry};

fn unescape(value: &str) -> Result<String> {
    let mut bytes = Vec::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            let mut buffer = [0; 4];
            bytes.extend_from_slice(ch.encode_utf8(&mut buffer).as_bytes());
            continue;
        }
        match chars
            .next()
            .ok_or_else(|| anyhow::anyhow!("incomplete KConfig escape"))?
        {
            's' => bytes.push(b' '),
            't' => bytes.push(b'\t'),
            'n' => bytes.push(b'\n'),
            'r' => bytes.push(b'\r'),
            '\\' => bytes.push(b'\\'),
            'x' => {
                let mut value = 0;
                for _ in 0..2 {
                    value = value * 16
                        + chars
                            .next()
                            .and_then(|ch| ch.to_digit(16))
                            .ok_or_else(|| anyhow::anyhow!("invalid KConfig hex escape"))?;
                }
                bytes.push(value as u8);
            }
            _ => anyhow::bail!("unsupported KConfig escape"),
        }
    }
    Ok(String::from_utf8(bytes)?)
}

#[derive(PartialEq, Eq)]
enum Value {
    Literal(String),
    Inverse(String),
    Shortcut {
        live: Vec<[i32; 4]>,
        defaults: Vec<[i32; 4]>,
        label: String,
    },
}

fn entries(source: &str) -> Result<BTreeMap<(String, String), Value>> {
    let mut result = BTreeMap::new();
    let mut group = String::new();
    for raw in source.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with(['#', ';']) {
            continue;
        }
        if line.starts_with('[') {
            group = unescape(line)?;
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| anyhow::anyhow!("invalid KConfig entry"))?;
        let mut key = unescape(key.trim())?;
        let value = unescape(value.trim())?;
        if group.ends_with("[$InverseAction]") {
            let mut inverse = list(&value, 1)
                .filter(|values| values.len() == 1 && !values[0].is_empty())
                .and_then(|mut values| values.pop())
                .context("invalid inverse action list")?;
            // KGlobalAccel writes either direction of an undirected pair.
            // Both forms assign the same inverse action to each endpoint.
            if key > inverse {
                std::mem::swap(&mut key, &mut inverse);
            }
            match result.entry((group.clone(), key)) {
                Entry::Vacant(entry) => {
                    entry.insert(Value::Inverse(inverse));
                }
                Entry::Occupied(entry) => {
                    ensure!(
                        matches!(entry.get(), Value::Inverse(previous) if previous == &inverse),
                        "conflicting inverse actions"
                    )
                }
            }
            continue;
        }
        let value = if super::formats::kwin_shortcut_group(&group) && !key.starts_with("_k_") {
            match list(&value, 3).and_then(|values| <[String; 3]>::try_from(values).ok()) {
                Some([live, defaults, label]) => Value::Shortcut {
                    live: keys(&live)?,
                    defaults: keys(&defaults)?,
                    label,
                },
                None => Value::Literal(value),
            }
        } else {
            Value::Literal(value)
        };
        ensure!(
            result.insert((group.clone(), key), value).is_none(),
            "duplicate KConfig entry"
        );
    }
    let mut endpoints = BTreeSet::new();
    for ((group, key), value) in &result {
        if let Value::Inverse(inverse) = value {
            ensure!(
                endpoints.insert((group, key))
                    && (key == inverse || endpoints.insert((group, inverse))),
                "conflicting inverse actions"
            );
        }
    }
    Ok(result)
}

fn list(value: &str, limit: usize) -> Option<Vec<String>> {
    if value.is_empty() {
        return Some(Vec::new());
    }
    if value == "\\0" {
        return Some(vec![String::new()]);
    }
    let mut result = Vec::with_capacity(limit);
    let mut word = String::new();
    let mut chars = value.chars();
    while let Some(ch) = chars.next() {
        match ch {
            '\\' => match chars.next() {
                Some(ch) => word.push(ch),
                None => break,
            },
            ',' => {
                if result.len() + 1 >= limit {
                    return None;
                }
                result.push(std::mem::take(&mut word));
            }
            ch => word.push(ch),
        }
    }
    result.push(word);
    Some(result)
}

fn keys(value: &str) -> Result<Vec<[i32; 4]>> {
    let mut keys = Vec::new();
    for (index, value) in value.split('\t').enumerate() {
        ensure!(
            index < super::MAX_FIELDS,
            "too many KWin shortcut alternatives"
        );
        let key = super::qt_sequence(value)?;
        if key != [0; 4] {
            keys.push(key);
        }
    }
    keys.sort_unstable();
    keys.dedup();
    Ok(keys)
}

pub fn same_settings(left: &str, right: &str) -> Result<bool> {
    Ok(entries(left)? == entries(right)?)
}

/// Reject ambiguous metadata before asking the shortcut service to write.
pub fn validate_settings(source: &str) -> Result<()> {
    entries(source).map(|_| ())
}

/// Compare native sets of alternatives while retaining action and label identity.
pub fn same_shortcut_settings(left: &str, right: &str, scope: &str) -> Result<bool> {
    let (group, action) = scope
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("invalid KWin shortcut identity"))?;
    let key = (unescape(group)?, unescape(action)?);
    let left_entries = entries(left)?;
    let right_entries = entries(right)?;
    ensure!(
        left_entries.contains_key(&key) && right_entries.contains_key(&key),
        "KWin shortcut disappeared"
    );
    Ok(left_entries == right_entries)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_inverse_pair_direction_preserves_settings_but_changed_endpoints_do_not() {
        let before = "[kwin]\nAction=Meta+Q,none,Keep label\n[kwin][$InverseAction]\nWindow Grow Horizontal=Window Shrink Horizontal\n[app]\nPersonal=keep\n";
        let reversed = before.replace(
            "Window Grow Horizontal=Window Shrink Horizontal",
            "Window Shrink Horizontal=Window Grow Horizontal",
        );
        assert!(same_settings(before, &reversed).unwrap());
        assert!(same_shortcut_settings(before, &reversed, "[kwin]/Action").unwrap());
        assert!(
            !same_settings(
                before,
                &reversed.replace("Window Grow Horizontal", "Window Grow Vertical")
            )
            .unwrap()
        );
        assert!(
            !same_settings(
                before,
                &reversed.replace("Personal=keep", "Personal=changed")
            )
            .unwrap()
        );
        let escaped = "[kwin][$InverseAction]\nGrow, horizontal=Shrink\\\\, horizontal\n";
        let opposite = "[kwin][$InverseAction]\nShrink, horizontal=Grow\\\\, horizontal\n";
        assert!(same_settings(escaped, opposite).unwrap());
        assert!(validate_settings("[kwin][$InverseAction]\na=b\nc=b\n").is_err());
        assert!(validate_settings("[kwin][$InverseAction]\na=b,c\n").is_err());
        assert!(validate_settings("[kwin][$InverseAction]\na=\\\\0\n").is_err());
        assert!(validate_settings("[kwin][$InverseAction]\na=a\n").is_ok());
    }
    #[test]
    fn ordering_trivia_and_native_escaping_do_not_hide_foreign_setting_changes() {
        let before = "# personal\n[kwin]\nAction=Meta+Q,Meta+Q,My label\n[app]\nKey=\\skeep\\s\n";
        let canonical = "[app]\nKey=\\x20keep\\s\n\n[kwin]\nAction=Meta+Q,Meta+Q,My label\n";
        assert!(same_settings(before, canonical).unwrap());
        assert!(!same_settings(before, &canonical.replace("keep", "new")).unwrap());
        assert!(!same_settings(before, &(canonical.to_owned() + "extra=value\n")).unwrap());
        assert!(same_settings(before, "[app]\nKey=a\nKey=b").is_err());
    }

    #[test]
    fn native_set_order_keeps_foreign_chords_defaults_and_labels_intact() {
        let before = "# personal\n[kwin]\nAction=Meta+Q\\tAlt+F4,Alt+F4,Keep label\nOther=Meta+X\\tMeta+Y,none,Other label\n";
        let reordered = before.replace("Meta+Q\\tAlt+F4", "Alt+F4\\tMeta+Q\\tMeta+Q");
        assert!(same_shortcut_settings(before, &reordered, "[kwin]/Action").unwrap());
        assert!(
            same_shortcut_settings(
                before,
                &reordered.replace("Meta+X\\tMeta+Y", "Meta+Y\\tMeta+X"),
                "[kwin]/Action"
            )
            .unwrap()
        );
        assert!(
            !same_shortcut_settings(before, &before.replace("Meta+X", "Meta+Z"), "[kwin]/Action")
                .unwrap()
        );
        let defaults = before.replace(",Alt+F4,Keep", ",Meta+Q\\tAlt+F4,Keep");
        assert!(
            same_shortcut_settings(
                &defaults,
                &defaults.replace(",Meta+Q\\tAlt+F4,Keep", ",Alt+F4\\tMeta+Q,Keep"),
                "[kwin]/Action"
            )
            .unwrap()
        );
        assert!(
            !same_shortcut_settings(
                before,
                &reordered.replace("Keep label", "New label"),
                "[kwin]/Action"
            )
            .unwrap()
        );
        assert!(
            !same_shortcut_settings(
                before,
                &reordered.replace(",Alt+F4,Keep", ",Meta+Q,Keep"),
                "[kwin]/Action"
            )
            .unwrap()
        );
        assert!(
            !same_shortcut_settings(
                before,
                &reordered.replace("Meta+Q\\tMeta+Q", "Meta+W"),
                "[kwin]/Action"
            )
            .unwrap()
        );
    }
}
