//! Mandara configuration: the TOML schema, the reactive [`ConfigStore`] with
//! its file watcher, and XDG [`paths`].

pub mod admin;
pub mod alerts;
pub mod apps;
pub mod open;
pub mod bar_styles;
pub mod bar_items;
pub mod kwin;
pub mod compositor;
pub mod components;
pub mod connectivity;
pub mod conflicts;
pub mod desktop;
pub mod doctor;
pub mod floating;
pub mod desktop_widgets;
pub mod desktop_menu;
pub mod desktop_cards;
pub mod dock;
pub mod input;
pub mod login_layout;
pub mod looks;
pub mod user_looks;
pub mod mango_keys;
pub mod motion;
pub mod outputs;
pub mod paths;
pub mod power;
pub mod profiles;
pub mod preauth;
pub mod recorder;
pub mod schema;
pub mod side_bars;
pub mod side_edges;
pub mod stash;
pub mod sidebar;
pub mod store;
pub mod surfaces;
pub mod upgrade;
pub mod widget_kinds;
pub mod widget_style;
pub mod wallgen;
pub mod workflow;

#[cfg(test)]
mod river_check;
#[cfg(test)]
mod validator_tests;

pub use schema::*;
pub use motion::MotionConfig;
pub use store::{ConfigStore, ConfigWatcher, ReloadOutcome, write_atomic};
pub mod agent;
