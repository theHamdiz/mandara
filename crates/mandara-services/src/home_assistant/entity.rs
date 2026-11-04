use mandara_i18n::t;
use serde::Serialize;
use serde_json::Value;

/// Home Assistant identifiers are path segments with a domain and object
/// name. Limit pasted values before they become endpoints or config keys.
const MAX_ENTITY_ID_BYTES: usize = 256;

pub fn entity_id(value: &str) -> bool {
    value.len() <= MAX_ENTITY_ID_BYTES && value.split_once('.').is_some_and(|(domain, name)| {
        [domain, name].iter().all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'))
    })
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Entity {
    pub id: String,
    pub name: String,
    pub state: String,
    pub unit: Option<String>,
}

impl Entity {
    pub fn domain(&self) -> &str { self.id.split_once('.').map_or("", |(domain, _)| domain) }
    pub fn available(&self) -> bool { self.state != "unavailable" }

    pub fn action(&self) -> Option<Action> {
        if !self.available() { return None; }
        match self.domain() {
            "light" | "switch" | "input_boolean" | "fan" if matches!(self.state.as_str(), "on" | "off") => Some(Action::Toggle),
            "scene" | "script" | "button" | "input_button" => Some(Action::Activate),
            _ => None,
        }
    }

    pub(super) fn parse(value: &Value) -> Option<Self> {
        let id = value.get("entity_id")?.as_str()?.to_owned();
        if !entity_id(&id) { return None; }
        let state = value.get("state")?.as_str()?.to_owned();
        let name = value.pointer("/attributes/friendly_name").and_then(Value::as_str)
            .filter(|name| !name.trim().is_empty()).unwrap_or(&id).to_owned();
        let unit = value.pointer("/attributes/unit_of_measurement").and_then(Value::as_str).map(str::to_owned);
        Some(Self { id, name, state, unit })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action { Toggle, Activate }

impl Action {
    pub(super) fn service(self, entity: &Entity) -> Result<&'static str, String> {
        if entity.action() != Some(self) { return Err(t("This entity cannot run that action right now").into()); }
        Ok(match (self, entity.domain()) {
            (Self::Toggle, _) => "toggle",
            (Self::Activate, "button" | "input_button") => "press",
            (Self::Activate, _) => "turn_on",
        })
    }
}
