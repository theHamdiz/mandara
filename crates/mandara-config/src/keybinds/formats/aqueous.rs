//! Validate values against Aqueous's literal action and chord grammar.

mod actions;
mod keys;

use crate::keybinds::Binding;
use anyhow::{Result, ensure};
use mandara_i18n::t;

/// Aqueous keeps the complete action verb in its fixed native Text buffer.
const ACTION_BYTES: usize = 256;

pub(super) fn builtin(value: &str) -> bool {
    let action = value.split(':').next().unwrap_or_default();
    actions::ACTIONS.contains(&action)
        || action.starts_with("set_layout_")
        || action.starts_with("scroll_viewport_left")
        || action.starts_with("scroll_viewport_right")
        || ["focus_workspace_", "move_to_workspace_"]
            .iter()
            .any(|prefix| {
                action
                    .strip_prefix(prefix)
                    .and_then(|number| number.parse::<u32>().ok())
                    .is_some_and(|number| number > 0)
            })
        || [
            "wheel_scroll_left",
            "wheel_scroll_right",
            "wheel_scroll_up",
            "wheel_scroll_down",
        ]
        .contains(&action)
}

/// Physical XKB modifier bits used by Aqueous's key-binding lookup.
const SHIFT: u8 = 1;
const CTRL: u8 = 4;
const ALT: u8 = 8;
const SUPER: u8 = 64;

fn parsed(value: &str, primary_mod_is_alt: bool) -> Option<(u8, Option<u32>)> {
    let (mut modifiers, mut found, mut symbol) = (0, false, None);
    for token in value.split('+').map(str::trim) {
        let modifier = if ["super", "mod4", "logo", "win"]
            .iter()
            .any(|name| token.eq_ignore_ascii_case(name))
        {
            Some(if primary_mod_is_alt { ALT } else { SUPER })
        } else if token.eq_ignore_ascii_case("meta") {
            Some(SUPER)
        } else if ["ctrl", "control"]
            .iter()
            .any(|name| token.eq_ignore_ascii_case(name))
        {
            Some(CTRL)
        } else if ["alt", "mod1"]
            .iter()
            .any(|name| token.eq_ignore_ascii_case(name))
        {
            Some(ALT)
        } else if token.eq_ignore_ascii_case("shift") {
            Some(SHIFT)
        } else {
            None
        };
        if let Some(modifier) = modifier {
            modifiers |= modifier;
            continue;
        }
        if found {
            return None;
        }
        found = true;
        if ["WheelUp", "WheelDown", "WheelLeft", "WheelRight"]
            .iter()
            .any(|name| token.eq_ignore_ascii_case(name))
        {
            continue;
        }
        let function = (2..=3)
            .contains(&token.len())
            .then(|| token.strip_prefix(['f', 'F']))
            .flatten()
            .and_then(|number| number.parse::<u8>().ok())
            .filter(|number| (1..=24).contains(number));
        let mut bytes = token.bytes();
        let ascii = bytes
            .next()
            .filter(|ch| (b' '..=b'~').contains(ch))
            .filter(|_| bytes.next().is_none());
        symbol = Some(if let Some(number) = function {
            0xffbe + u32::from(number) - 1
        } else if let Some(ch) = ascii {
            u32::from(ch.to_ascii_lowercase())
        } else {
            keys::KEYS
                .iter()
                .find(|(name, _)| token.eq_ignore_ascii_case(name))?
                .1
        });
    }
    found.then_some((modifiers, symbol))
}

fn chord(value: &str) -> bool {
    parsed(value, false).is_some()
}

pub fn normalized(value: &str, primary_mod_is_alt: bool) -> Option<String> {
    let (modifiers, symbol) = parsed(value, primary_mod_is_alt)?;
    let key = xkbcommon::xkb::keysym_get_name(xkbcommon::xkb::Keysym::new(symbol?));
    Some(crate::compositor::chord(
        [
            (SHIFT, "shift"),
            (CTRL, "ctrl"),
            (ALT, "alt"),
            (SUPER, "super"),
        ]
        .into_iter()
        .filter_map(|(mask, name)| (modifiers & mask != 0).then_some(name)),
        &key.to_ascii_lowercase(),
    ))
}

pub(super) fn validate(before: &Binding, after: &Binding) -> Result<()> {
    for (old, field) in before.fields.iter().zip(&after.fields) {
        if field.label == "Shortcut" {
            ensure!(
                field.value.is_empty() || chord(&field.value),
                "{}",
                t("Enter a key combination supported by Aqueous.")
            );
            ensure!(
                field.name.is_none() || !field.value.contains(','),
                "{}",
                t("Use the named key Comma in an Aqueous shortcut list.")
            );
        } else if after.scope == "keybinds.custom" && old.value != field.value {
            let (verb, argument) = field.value.split_once(':').unwrap_or_default();
            ensure!(
                field.value.len() <= ACTION_BYTES
                    && !argument.trim().is_empty()
                    && match verb {
                        "spawn" | "launch" => true,
                        "set_layout" => [
                            "tile",
                            "monocle",
                            "grid",
                            "rows",
                            "dwindle",
                            "reverse-dwindle",
                            "reverse_dwindle",
                            "scrolling",
                            "float",
                            "floating",
                            "stack",
                            "stacking",
                            "game-mode",
                            "game_mode",
                            "composable"
                        ]
                        .contains(&argument.trim()),
                        "builtin" => builtin(argument),
                        _ => false,
                    },
                "{}",
                t(
                    "Use a supported Aqueous action: spawn, launch, set_layout or builtin, followed by a colon and its value."
                )
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_chords_reject_ignored_keys_and_accept_aliases_media_and_wheel_bindings() {
        for name in [
            "Super+Return",
            "Ctrl+F24",
            "PrintScreen",
            "Caps",
            "Alt+Meta+WheelUp",
            "XF86AudioMicMute",
        ] {
            assert!(chord(name), "{name}");
        }
        for name in [
            "Super",
            "Super+Invented",
            "Ctrl+F25",
            "Ctrl+F001",
            "Super+Q+W",
            "Super+界",
        ] {
            assert!(!chord(name), "{name}");
        }
        assert!(builtin("move_to_workspace_12") && builtin("reload_rules"));
        assert!(!builtin("invented"));
        assert_eq!(normalized("Super+Q", true).as_deref(), Some("alt+q"));
        assert_eq!(normalized("Meta+Q", true).as_deref(), Some("super+q"));
        assert_eq!(
            normalized("Super+Shift+PrintScreen", false).as_deref(),
            Some("shift+super+print")
        );
    }
}
