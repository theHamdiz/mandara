//! Niri sends its current XKB layout list before sending index-only switches.

use anyhow::Result;
use serde::Deserialize;
use serde_json::Value;

#[derive(Default, Deserialize)]
pub(super) struct Layouts {
    names: Vec<String>,
    current_idx: u8,
}

impl Layouts {
    pub fn apply(&mut self, event: &mut Value) -> Result<Option<String>> {
        if let Some(layouts) = event.pointer_mut("/KeyboardLayoutsChanged/keyboard_layouts") {
            *self = serde_json::from_value(layouts.take())?;
            return Ok(Some(self.names.get(usize::from(self.current_idx)).cloned().unwrap_or_default()));
        }
        if let Some(index) = event.pointer("/KeyboardLayoutSwitched/idx").and_then(Value::as_u64)
            .and_then(|index| u8::try_from(index).ok())
            && let Some(name) = self.names.get(usize::from(index)) {
                self.current_idx = index;
                return Ok(Some(name.clone()));
            }
        Ok(None)
    }
}
