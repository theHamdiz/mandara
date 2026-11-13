//! Compare native KConfig writes without accepting changes to unrelated entries.

use anyhow::{Result, ensure};
use std::collections::BTreeMap;

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

fn entries(source: &str) -> Result<BTreeMap<(String, String), String>> {
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
        ensure!(
            result
                .insert(
                    (group.clone(), unescape(key.trim())?),
                    unescape(value.trim())?
                )
                .is_none(),
            "duplicate KConfig entry"
        );
    }
    Ok(result)
}

pub fn same_settings(left: &str, right: &str) -> Result<bool> {
    Ok(entries(left)? == entries(right)?)
}

/// KGlobalAccel may reorder only the selected action's live alternatives.
pub fn same_shortcut_settings(left: &str, right: &str, scope: &str) -> Result<bool> {
    let (group, action) = scope
        .split_once('/')
        .ok_or_else(|| anyhow::anyhow!("invalid KWin shortcut identity"))?;
    let key = (unescape(group)?, unescape(action)?);
    let mut left_entries = entries(left)?;
    let mut right_entries = entries(right)?;
    ensure!(
        left_entries.remove(&key).is_some() && right_entries.remove(&key).is_some(),
        "KWin shortcut disappeared"
    );
    if left_entries != right_entries {
        return Ok(false);
    }
    fn selected(source: &str, scope: &str) -> Result<(Vec<[i32; 4]>, String)> {
        let binding = super::parse(
            crate::compositor::Target::KWin,
            std::path::Path::new("kglobalshortcutsrc"),
            source,
        )?
        .into_iter()
        .find(|row| row.scope == scope)
        .ok_or_else(|| anyhow::anyhow!("KWin shortcut disappeared"))?;
        let mut alternatives = binding
            .fields
            .iter()
            .map(|field| super::qt_sequence(&field.value))
            .collect::<Result<Vec<_>>>()?;
        alternatives.retain(|keys| *keys != [0; 4]);
        alternatives.sort_unstable();
        alternatives.dedup();
        let end = binding
            .fields
            .last()
            .ok_or_else(|| anyhow::anyhow!("KWin shortcut has no fields"))?
            .range
            .end;
        let suffix = source[end..].lines().next().unwrap_or_default().trim();
        Ok((alternatives, unescape(suffix)?))
    }
    Ok(selected(left, scope)? == selected(right, scope)?)
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn only_selected_live_alternatives_can_be_reordered_or_deduplicated() {
        let before = "# personal\n[kwin]\nAction=Meta+Q\\tAlt+F4,Alt+F4,Keep label\nOther=Meta+X\\tMeta+Y,none,Other label\n";
        let reordered = before.replace("Meta+Q\\tAlt+F4", "Alt+F4\\tMeta+Q\\tMeta+Q");
        assert!(same_shortcut_settings(before, &reordered, "[kwin]/Action").unwrap());
        assert!(
            !same_shortcut_settings(
                before,
                &reordered.replace("Meta+X\\tMeta+Y", "Meta+Y\\tMeta+X"),
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
