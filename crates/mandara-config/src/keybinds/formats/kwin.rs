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
        for (line, raw) in source.split_inclusive('\n').enumerate() {
            let text = raw.trim_end_matches(['\n', '\r']);
            let trimmed = text.trim();
            if trimmed.starts_with('[') {
                group = trimmed;
            } else if (group == "[kwin]" || group.starts_with("[kwin]["))
                && !group.contains("[$")
                && !trimmed.starts_with(['#', ';'])
            {
                if let Some((name, value)) = text.split_once('=')
                    && !name.trim().starts_with("_k_")
                {
                    let value_start = text.len() - value.len();
                    let fields = alternatives(value, offset + value_start)?;
                    let combo = (fields.len() == 1 && !fields[0].value.contains(','))
                        .then(|| fields[0].value.replace("Meta+", "Super+"));
                    result.push(Binding {
                        line: line + 1,
                        scope: format!("{group}/{}", name.trim()),
                        chord: combo
                            .as_deref()
                            .filter(|key| *key != "none")
                            .and_then(normalize),
                        fields,
                        read_only: None,
                    });
                }
            }
            offset += raw.len();
        }
        Ok(result)
    }
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
