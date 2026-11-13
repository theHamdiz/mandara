//! KGlobalAccel stores the live alternatives before defaults and the action label.

use anyhow::Result;
use mandara_i18n::tk;

use crate::keybinds::{Binding, Encoding, ShortcutFormat, field, normalize};

pub(super) struct KWin;

impl ShortcutFormat for KWin {
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
                    let mut escaped = false;
                    let mut comma = value.len();
                    for (at, ch) in value.char_indices() {
                        if escaped {
                            escaped = false;
                            continue;
                        }
                        if ch == '\\' {
                            escaped = true;
                        } else if ch == ',' {
                            comma = at;
                            break;
                        }
                    }
                    let live = &value[..comma];
                    let mut fields = Vec::new();
                    let mut from = 0;
                    for key in live.split("\\t") {
                        let start = offset + value_start + from;
                        fields.push(field(
                            tk("Shortcut"),
                            key,
                            start..start + key.len(),
                            Encoding::KConfig,
                        ));
                        from += key.len() + 2;
                    }
                    let combo = (fields.len() == 1).then(|| live.replace("Meta+", "Super+"));
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
}
