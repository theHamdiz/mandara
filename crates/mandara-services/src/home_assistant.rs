//! Home Assistant state summaries and explicit entity actions. Devices
//! are controlled through services; writing a state is never an action.

mod entity;

use mandara_i18n::t;
use serde_json::json;
use ureq::http::Method;

use crate::credentials::Secret;
use crate::web_api::{Authorization, Client, Transport, endpoint};

pub use entity::{Action, Entity, entity_id};

/// The panel is a set of favourites, not a second full home dashboard.
pub const MAX_FAVOURITES: usize = 24;
/// Discovery is bounded independently of how many entities the server has.
const MAX_ENTITIES: usize = 512;

pub trait Backend {
    fn states(&self, server: &str, key: &Secret, favourites: &[String]) -> Result<Vec<Entity>, String>;
    fn act(&self, server: &str, key: &Secret, id: &str, action: Action) -> Result<(), String>;
}

#[derive(Default)]
pub struct Http<T = Client> { transport: T }

impl Http<Client> {
    pub fn new() -> Self { Self::default() }
}

impl<T: Transport> Backend for Http<T> {
    fn states(&self, server: &str, key: &Secret, favourites: &[String]) -> Result<Vec<Entity>, String> {
        if favourites.len() > MAX_FAVOURITES || favourites.iter().any(|id| !entity_id(id)) {
            return Err(t("Choose up to 24 Home Assistant entities").into());
        }
        let value = self.transport.json(Method::GET, &endpoint(server, "api/states")?, None, Some(Authorization::Bearer(key)))?;
        let values = value.as_array().ok_or_else(|| t("Home Assistant returned no entity list").to_string())?;
        let mut seen = std::collections::HashSet::new();
        let mut entities: Vec<_> = values.iter().filter_map(Entity::parse)
            .filter(|entity| (favourites.is_empty() || favourites.contains(&entity.id)) && seen.insert(entity.id.clone()))
            .take(MAX_ENTITIES).collect();
        if favourites.is_empty() { entities.sort_by(|a, b| a.name.cmp(&b.name)); }
        else { entities.sort_by_key(|entity| favourites.iter().position(|id| id == &entity.id)); }
        Ok(entities)
    }

    fn act(&self, server: &str, key: &Secret, id: &str, action: Action) -> Result<(), String> {
        if !entity_id(id) { return Err(t("Choose a valid Home Assistant entity").into()); }
        let current = self.transport.json(Method::GET, &endpoint(server, &format!("api/states/{id}"))?, None, Some(Authorization::Bearer(key)))?;
        let entity = Entity::parse(&current).filter(|entity| entity.id == id)
            .ok_or_else(|| t("Home Assistant could not find this entity").to_string())?;
        let service = action.service(&entity)?;
        let domain = entity.domain();
        self.transport.json(Method::POST, &endpoint(server, &format!("api/services/{domain}/{service}"))?, Some(&json!({"entity_id": id})), Some(Authorization::Bearer(key)))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
