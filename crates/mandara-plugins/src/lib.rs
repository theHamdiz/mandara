//! Mandara plugin runtime.
//!
//! * [`plugin`] — manifests, the registry, bar/panel instances and the panel
//!   choreography context.
//! * [`lifecycle`] — switching plugins on / off, removing and restoring
//!   built-ins, and the [`lifecycle::ESSENTIAL`] core that refuses both.
//! * [`vet`] — the compliance check community plugins pass before they
//!   install (broken, malicious or placeholder releases are flagged).
//! * [`command`] — structured, self-documenting commands: every shell action
//!   is reachable as `mandara msg <path> <args>` with JSON results.

pub mod authoring;
pub mod command;
pub mod guard;
pub mod lifecycle;
pub mod plugin;
pub mod vet;
pub mod wasm;

pub use guard::{guard, install_panic_hook};
pub use command::{ArgKind, ArgSpec, Args, CommandGate, CommandRegistry, CommandSpec, Output};
pub use plugin::{
    ClickHandler, ManifestSetting, ManifestText, SettingChoice, SettingKind, PanelCx, PanelSpec, PluginFactory, PluginInstance, PluginManifest, PluginOrigin, PluginRegistry,
    Reveal, Zone, discover_third_party, split_settings, valid_settings,
};
