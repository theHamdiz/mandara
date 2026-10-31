//! Mandara configuration: the TOML schema, the reactive [`ConfigStore`] with
//! its file watcher, and XDG [`paths`].

pub mod admin;
pub mod alerts;
pub mod apps;
pub mod compositor;
pub mod connectivity;
pub mod conflicts;
pub mod desktop;
pub mod desktop_widgets;
pub mod desktop_cards;
pub mod dock;
pub mod input;
pub mod looks;
pub mod outputs;
pub mod paths;
pub mod power;
pub mod profiles;
pub mod schema;
pub mod sidebar;
pub mod store;
pub mod surfaces;
pub mod upgrade;
pub mod wallgen;
pub mod workflow;

#[cfg(test)]
mod validator_tests;

pub use schema::*;
pub use store::{ConfigStore, ConfigWatcher, ReloadOutcome, write_atomic};
