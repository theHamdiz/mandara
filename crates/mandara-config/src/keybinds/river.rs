//! River identifies a mapping by mode, modifiers, keysym and release state.

use super::Binding;

pub const UNRESOLVED: &str = mandara_i18n::tk(
    "Some compositor shortcuts cannot be resolved safely. Edit them in their source configuration first.",
);

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Identity {
    mode: String,
    chord: String,
    symbol: u32,
    release: bool,
}

/// Layout and repeat options affect dispatch, but do not distinguish mappings.
pub fn identity(row: &Binding) -> Option<Identity> {
    if row.read_only.is_some() && row.scope.contains(['$', '`']) {
        return None;
    }
    use xkbcommon::xkb;
    let value = |label| {
        row.fields
            .iter()
            .find(|field| field.label == label)
            .map(|field| field.value.as_str())
    };
    let key = value("Key")?;
    let mut symbol = xkb::keysym_from_name(key, xkb::KEYSYM_CASE_INSENSITIVE);
    // River retains this historical alias distinction on older xkbcommon.
    if key == "XF86ScreenSaver" || key == "XF86Screensaver" {
        symbol = xkb::keysym_from_name(key, xkb::KEYSYM_NO_FLAGS);
    }
    if symbol.raw() == 0 {
        return None;
    }
    Some(Identity {
        mode: row.scope.clone(),
        symbol: symbol.raw(),
        chord: super::normalize(&format!(
            "{}+{}",
            value("Modifiers")?,
            xkb::keysym_get_name(symbol)
        ))?,
        release: row
            .fields
            .iter()
            .any(|field| field.label == "Flags" && field.value == "-release"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn computed_headers_are_unresolved_but_computed_commands_keep_their_identity() {
        let rows = crate::keybinds::parse(crate::compositor::Target::River, std::path::Path::new("init"), "riverctl map \"$mode\" Super Q close\nriverctl map normal \"$mod\" Q close\nriverctl map normal Super \"$key\" close\nriverctl map normal Super Q spawn \"$terminal\"\nriverctl map normal Super XF86ScreenSaver close\nriverctl map normal Super XF86Screensaver close\n").unwrap();
        assert!(rows[..3].iter().all(|row| identity(row).is_none()));
        assert!(identity(&rows[3]).is_some());
        assert!(identity(&rows[4]) != identity(&rows[5]));
    }
    #[test]
    fn native_mapping_identity_ignores_layout_repeat_and_keysym_case() {
        let rows = crate::keybinds::parse(crate::compositor::Target::River, std::path::Path::new("init"), "riverctl map -layout 2 normal Super Q close\nriverctl map -repeat normal Super q close\nriverctl map -release normal Super Q close\nriverctl map resize Super Q close\n").unwrap();
        assert!(identity(&rows[0]) == identity(&rows[1]));
        assert!(identity(&rows[0]) != identity(&rows[2]));
        assert!(identity(&rows[0]) != identity(&rows[3]));
    }
}
