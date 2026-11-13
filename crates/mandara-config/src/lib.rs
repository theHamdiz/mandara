//! Mandara configuration: the TOML schema, the reactive [`ConfigStore`] with
//! its file watcher, and XDG [`paths`].

pub mod admin;
pub mod alerts;
pub mod app_themes;
pub mod apps;
pub mod bar_items;
pub mod bar_styles;
pub mod components;
pub mod compositor;
pub mod compositor_theme;
pub mod conflicts;
pub mod connectivity;
pub mod desktop;
pub mod desktop_cards;
pub mod desktop_menu;
pub mod desktop_widgets;
pub mod dock;
pub mod doctor;
pub mod floating;
pub mod input;
pub mod kwin;
pub mod login_layout;
pub mod looks;
pub mod mango_keys;
pub mod motion;
pub mod open;
pub mod outputs;
pub mod patch;
pub mod paths;
pub mod power;
pub mod preauth;
pub mod profiles;
pub mod recorder;
pub mod schema;
pub mod side_bars;
pub mod side_edges;
pub mod sidebar;
pub mod stash;
pub mod store;
pub mod surfaces;
pub mod upgrade;
pub mod user_looks;
pub mod wallgen;
pub mod widget_kinds;
pub mod widget_settings;
pub mod widget_style;
pub mod window_motion;
pub mod workflow;

#[cfg(test)]
mod river_check;
#[cfg(test)]
mod validator_tests;

pub use motion::MotionConfig;
pub use schema::*;
pub use store::{ConfigStore, ConfigWatcher, ReloadOutcome, write_atomic};
pub mod agent;
