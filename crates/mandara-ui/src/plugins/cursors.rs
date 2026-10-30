//! Cursor themes: a gallery of open-source cursor themes to install,
//! preview and apply live (Studio → Themes → Cursor and the
//! `cursor list|install|apply|remove` commands, registered with
//! `system_admin`). Opt-in: switched off, Mandara leaves the cursor
//! configuration alone entirely — no cursor lines in the compositor
//! drop-in (`ShellConfig::effective_compositor`), no `~/.icons/default`,
//! no live `setcursor`.

use mandara_i18n::tk;
use mandara_plugins::{PluginInstance, PluginManifest, PluginRegistry};

pub const ID: &str = mandara_config::upgrade::CURSORS_ID;

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Cursor themes"), tk("Install, preview and apply cursor themes, live, with size and hiding options"), None).opt_in(), |_, _, _| PluginInstance::empty());
}
