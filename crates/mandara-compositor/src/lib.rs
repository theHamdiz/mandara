//! First-class compositor integration behind one [`CompositorBackend`]
//! trait: MangoWM (bitmask tags, native socket), Niri (scrollable
//! workspaces, JSON socket) and Hyprland (workspaces, socket/socket2).

pub mod backend;
pub mod hyprland;
pub mod mango;
pub mod model;
pub mod niri;

use std::sync::Arc;

pub use backend::CompositorBackend;
pub use hyprland::HyprlandDriver;
pub use mango::MangoDriver;
pub use model::*;
pub use niri::NiriDriver;

/// Detects the running compositor from the environment.
pub fn detect() -> Option<CompositorKind> {
    if std::env::var_os("MANGO_INSTANCE_SIGNATURE").is_some()
        || std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.eq_ignore_ascii_case("mango"))
    {
        Some(CompositorKind::MangoWM)
    } else if std::env::var_os("NIRI_SOCKET").is_some() {
        Some(CompositorKind::Niri)
    } else if std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE").is_some() {
        Some(CompositorKind::Hyprland)
    } else {
        None
    }
}

/// Starts an interactive move of the window under the pointer, for client-
/// side title bars. MangoWM ignores `xdg_toplevel.move`, so it is asked
/// over IPC to run its own `moveresize,curmove` grab (released with the
/// button). Returns `false` where the standard request already works.
pub fn begin_interactive_move() -> bool {
    match detect() {
        Some(CompositorKind::MangoWM) => mango::MangoDriver::from_env().is_some_and(|c| c.dispatch("moveresize", &["curmove"]).is_ok()),
        _ => false,
    }
}

/// Connects to the running compositor's native IPC.
pub fn connect() -> Option<Arc<dyn CompositorBackend>> {
    match detect()? {
        CompositorKind::MangoWM => MangoDriver::from_env().map(|d| Arc::new(d) as Arc<dyn CompositorBackend>),
        CompositorKind::Niri => NiriDriver::from_env().map(|d| Arc::new(d) as Arc<dyn CompositorBackend>),
        CompositorKind::Hyprland => HyprlandDriver::from_env().map(|d| Arc::new(d) as Arc<dyn CompositorBackend>),
    }
}

/// Events buffered between a driver's watcher thread and the UI before the
/// watcher waits (back-pressure instead of unbounded growth).
pub(crate) const EVENT_QUEUE: usize = 64;
/// Read timeout of one-shot IPC requests.
pub(crate) const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
/// Pause before reconnecting a dropped event stream.
pub(crate) const RECONNECT_DELAY: std::time::Duration = std::time::Duration::from_secs(1);
