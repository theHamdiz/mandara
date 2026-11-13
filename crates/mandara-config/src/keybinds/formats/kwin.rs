//! KGlobalAccel stores the live alternatives before defaults and the action label.

use anyhow::{Result, bail};
use mandara_i18n::tk;

use crate::keybinds::{Binding, Encoding, ShortcutFormat, field, normalize};

pub(super) struct KWin;

impl ShortcutFormat for KWin {
    fn validate_edit(&self, _before: &Binding, after: &Binding) -> Result<()> {
        for field in &after.fields {
            crate::keybinds::qt_sequence(&field.value)?;
        }
        Ok(())
    }
    fn parse(&self, source: &str) -> Result<Vec<Binding>> {
        let mut result = Vec::new();
        let mut group = "";
        let mut offset = 0;
        let immutable_file = source.lines().any(|line| line.trim() == "[$i]");
        let immutable_groups: std::collections::BTreeSet<_> = source
            .lines()
            .map(str::trim)
            .filter(|line| line.starts_with('['))
            .filter_map(|line| {
                let (group, immutable) = group_options(line);
                (!group.is_empty() && immutable).then_some(group)
            })
            .collect();
        let mut immutable_group = false;
        for (line, raw) in source.split_inclusive('\n').enumerate() {
            let text = raw.trim_end_matches(['\n', '\r']);
            let trimmed = text.trim();
            if trimmed.starts_with('[') {
                group = group_options(trimmed).0;
                immutable_group = immutable_groups.contains(group)
                    || group
                        .match_indices("][")
                        .any(|(at, _)| immutable_groups.contains(&group[..at + 1]));
            } else if shortcut_group(group)
                && !trimmed.starts_with(['#', ';'])
                && let Some((name, value)) = text.split_once('=')
                && !name.trim().starts_with("_k_")
            {
                let (name, flags) = options(name.trim());
                let value_start = text.len() - value.len();
                let fields = alternatives(value, offset + value_start)?;
                let combo = (fields.len() == 1 && !fields[0].value.contains(','))
                    .then(|| fields[0].value.replace("Meta+", "Super+"));
                result.push(Binding {
                    line: line + 1,
                    scope: format!("{group}/{name}"),
                    chord: combo
                        .as_deref()
                        .filter(|key| *key != "none")
                        .and_then(normalize),
                    fields,
                    read_only: if immutable_file || immutable_group || flags.contains('i') {
                        Some(tk(
                            "This shortcut is locked by the compositor configuration.",
                        ))
                    } else if !flags.is_empty() {
                        Some(crate::keybinds::SOURCE_ONLY)
                    } else {
                        None
                    },
                });
            }
            offset += raw.len();
        }
        Ok(result)
    }
}

fn group_options(value: &str) -> (&str, bool) {
    // KConfig recognizes only an exact trailing [$i] on group headers.
    value
        .strip_suffix("[$i]")
        .map_or((value, false), |group| (group, true))
}

pub(in crate::keybinds) fn shortcut_group(group: &str) -> bool {
    group == "[kwin]"
        || group
            .strip_prefix("[kwin][")
            .and_then(|context| context.strip_suffix(']'))
            .is_some_and(|context| !context.starts_with('$') && !context.contains("]["))
}

fn options(value: &str) -> (&str, &str) {
    value
        .find("[$")
        .map_or((value, ""), |at| (&value[..at], &value[at..]))
}

/// INI escapes wrap QStringList escapes; split alternatives only after both.
fn alternatives(value: &str, base: usize) -> Result<Vec<crate::keybinds::Field>> {
    let mut chars = value.char_indices();
    let mut fields = Vec::new();
    let mut text = String::new();
    let mut start = 0;
    let mut end = 0;
    let mut quoted = false;
    while let Some((at, mut ch)) = chars.next() {
        end = at + ch.len_utf8();
        if ch == '\\' {
            let Some((next, escape)) = chars.next() else {
                bail!(
                    "{}",
                    mandara_i18n::t("The shortcut has an incomplete escape.")
                );
            };
            end = next + escape.len_utf8();
            ch = match escape {
                't' => '\t',
                's' => ' ',
                'n' => '\n',
                'r' => '\r',
                '\\' => '\\',
                _ => bail!(
                    "{}",
                    mandara_i18n::t("This shortcut uses an unsupported configuration escape.")
                ),
            };
        }
        if quoted {
            text.push(ch);
            quoted = false;
        } else if ch == '\\' {
            quoted = true;
        } else if ch == ',' {
            end = at;
            break;
        } else if ch == '\t' {
            fields.push(field(
                tk("Shortcut"),
                std::mem::take(&mut text),
                base + start..base + at,
                Encoding::KConfig,
            ));
            start = end;
        } else {
            text.push(ch);
        }
    }
    fields.push(field(
        tk("Shortcut"),
        text,
        base + start..base + end,
        Encoding::KConfig,
    ));
    Ok(fields)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_metadata_groups_neither_lock_actions_nor_appear_as_shortcuts() {
        let text = "[kwin]\nWindow Close=Alt+F4\\tMeta+Ctrl+Alt+F11,Alt+F4,Close Window\n[kwin][$InverseAction]\nDecrease Opacity=Increase Opacity\n[kwin][$Trigger]\nAction=metadata\n[kwin][resize]\nGrow=Meta+Right,none,Grow\n[kwin][resize][$InverseAction]\nGrow=Shrink\n[kwin][locked][$i]\nAction=Meta+X,none,Locked\n";
        let rows = KWin.parse(text).unwrap();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].scope, "[kwin]/Window Close");
        assert_eq!(rows[0].fields.len(), 2);
        assert!(rows[0].read_only.is_none());
        assert_eq!(rows[1].scope, "[kwin][resize]/Grow");
        assert!(rows[1].read_only.is_none());
        assert_eq!(rows[2].scope, "[kwin][locked]/Action");
        assert!(rows[2].read_only.is_some());
    }

    #[test]
    fn immutable_file_group_and_entry_flags_keep_real_action_ids_readable() {
        for text in [
            "[$i]\n[kwin]\nWindow Close=Meta+Q,Alt+F4,Close\n",
            "[kwin][$i]\nWindow Close=Meta+Q,Alt+F4,Close\n",
            "[kwin]\nWindow Close[$ei]=Meta+Q,Alt+F4,Close\n",
        ] {
            let rows = KWin.parse(text).unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0].scope, "[kwin]/Window Close");
            assert!(rows[0].read_only.is_some());
        }
        let rows = KWin
            .parse("[kwin][$i]\n[kwin][context]\nAction=Meta+Q,none,Action\n")
            .unwrap();
        assert_eq!(rows[0].scope, "[kwin][context]/Action");
        assert!(rows[0].read_only.is_some());
    }

    #[test]
    fn kwin_edits_one_live_alternative_and_keeps_defaults_action_ids_and_other_apps() {
        let text = "[kwin]\n_k_friendly_name=KWin\nWindow Close=Meta+Q\\tAlt+F4,Alt+F4,Close Window\n[plasmashell]\nMenu=Alt+F1,Alt+F1,Menu\n";
        let binds = KWin.parse(text).unwrap();
        assert_eq!(binds.len(), 1);
        assert_eq!(binds[0].fields.len(), 2);
        let next =
            crate::keybinds::patch(text, &binds[0], &["Meta+W".into(), "Alt+F4".into()]).unwrap();
        assert_eq!(next, text.replace("Meta+Q", "Meta+W"));
    }

    #[test]
    fn two_escape_layers_preserve_commas_backslashes_tabs_and_multistroke_sequences() {
        let source = "[kwin]\nAction=Meta+Q,Meta+Q,Keep this label\n";
        let binding = KWin.parse(source).unwrap().remove(0);
        for value in ["Meta+,", "Ctrl+K, Ctrl+C", "Meta+\\", "Meta+\\t"] {
            let next = crate::keybinds::patch(source, &binding, &[value.into()]).unwrap();
            let parsed = KWin.parse(&next).unwrap();
            assert_eq!(parsed[0].fields.len(), 1, "{next}");
            assert_eq!(parsed[0].fields[0].value, value, "{next}");
            assert!(next.ends_with(",Meta+Q,Keep this label\n"));
        }
    }
}
