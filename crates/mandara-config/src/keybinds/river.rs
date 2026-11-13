//! River identifies a mapping by mode, modifiers, keysym and release state.

use super::Binding;

#[derive(PartialEq, Eq)]
pub struct Identity<'a> {
    mode: &'a str,
    chord: String,
    release: bool,
}

/// Layout and repeat options affect dispatch, but do not distinguish mappings.
pub fn identity(row: &Binding) -> Option<Identity<'_>> {
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
        mode: &row.scope,
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
    fn native_mapping_identity_ignores_layout_repeat_and_keysym_case() {
        let rows = crate::keybinds::parse(crate::compositor::Target::River, std::path::Path::new("init"), "riverctl map -layout 2 normal Super Q close\nriverctl map -repeat normal Super q close\nriverctl map -release normal Super Q close\nriverctl map resize Super Q close\n").unwrap();
        assert!(identity(&rows[0]) == identity(&rows[1]));
        assert!(identity(&rows[0]) != identity(&rows[2]));
        assert!(identity(&rows[0]) != identity(&rows[3]));
    }
}
