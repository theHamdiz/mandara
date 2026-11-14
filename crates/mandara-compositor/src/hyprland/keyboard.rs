//! Initial keyboard state comes from devices; socket2 carries subsequent layout names.

use serde_json::Value;

pub(super) fn initial(devices: &Value) -> Option<&str> {
    let keyboards = devices.get("keyboards")?.as_array()?;
    fn layout(keyboard: &Value) -> Option<&str> {
        keyboard.get("active_keymap").and_then(Value::as_str).filter(|name| !name.is_empty())
    }
    // Prefer the active keyboard, even when other connected devices report a different layout.
    keyboards.iter().find(|keyboard| keyboard.get("main").and_then(Value::as_bool) == Some(true))
        .and_then(layout).or_else(|| keyboards.iter().find_map(layout)).or(Some(""))
}

pub(super) fn switched(data: &str) -> Option<&str> {
    let (keyboard, layout) = data.split_once(',')?;
    (!keyboard.is_empty() && !layout.is_empty()).then_some(layout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn initial_layout_prefers_the_main_keyboard_and_handles_disconnection() {
        let devices = json!({"keyboards":[{"main":false,"active_keymap":"English (US)"},{"main":true,"active_keymap":"Arabic"}]});
        assert_eq!(initial(&devices), Some("Arabic"));
        assert_eq!(initial(&json!({"keyboards":[{"active_keymap":"French"}]})), Some("French"));
        assert_eq!(initial(&json!({"keyboards":[]})), Some(""));
        assert_eq!(initial(&json!({"keyboards":null})), None);
    }

    #[test]
    fn layout_names_keep_commas_and_non_latin_letters() {
        assert_eq!(switched("keyboard,Arabic"), Some("Arabic"));
        assert_eq!(switched("keyboard,English (US, intl.)"), Some("English (US, intl.)"));
        assert_eq!(switched("keyboard,العربية"), Some("العربية"));
        assert_eq!(switched("keyboard,"), None);
        assert_eq!(switched(",Arabic"), None);
        assert_eq!(switched("malformed"), None);
    }
}
