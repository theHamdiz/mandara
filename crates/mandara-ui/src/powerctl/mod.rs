//! Power management inside the shell: idle stages ([`idle`]), the fade
//! before them ([`dim`]), battery alerts and automatic power profiles,
//! sleep / lid / startup locking ([`battery`]), event hooks ([`hooks`]) and
//! the power / session menu ([`session_menu`]).
//!
//! Each part follows the config live and documents its `mandara msg`
//! commands; settings live in Studio → Power & idle.

pub mod battery;
pub mod dim;
pub mod hooks;
pub mod idle;
pub mod session_menu;

use gpui::App;
use mandara_plugins::PluginRegistry;

/// Starts every part (shell process only, after the hub).
pub fn init(cx: &mut App) {
    hooks::init(cx);
    battery::init(cx);
    idle::init(cx);
}

/// Registers the commands of every part.
pub fn register(r: &mut PluginRegistry) {
    idle::register(r);
    battery::register(r);
    hooks::register(r);
    session_menu::register(r);
}

/// Runs `f` on a short-lived thread and hands its JSON result to the IPC
/// layer (keeps blocking D-Bus calls off the UI thread).
pub(crate) fn pending(f: impl FnOnce() -> Result<serde_json::Value, String> + Send + 'static) -> mandara_plugins::Output {
    let (tx, rx) = async_channel::bounded(1);
    let spawned = std::thread::Builder::new().name("mandara-powerctl".into()).spawn(move || {
        let _ = tx.send_blocking(f().map(|v| serde_json::to_string_pretty(&v).unwrap_or_default()));
    });
    if let Err(e) = spawned {
        tracing::warn!("powerctl: {e}");
    }
    mandara_plugins::Output::Pending(rx)
}
