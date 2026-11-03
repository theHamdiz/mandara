//! First-class compositor integration behind one [`CompositorBackend`]
//! trait: MangoWM (bitmask tags, native socket), Niri (scrollable
//! workspaces, JSON socket), Hyprland (workspaces, socket/socket2),
//! Umbriel (per-output workspaces, JSON-lines socket), Sway (per-output
//! workspaces, i3 IPC) and River (bitmask tags, its own Wayland
//! protocols).

pub mod backend;
pub mod hyprland;
pub mod mango;
pub mod model;
pub mod niri;
pub mod river;
pub mod sway;
pub mod umbriel;

use std::sync::Arc;

pub use backend::CompositorBackend;
pub use hyprland::HyprlandDriver;
pub use mango::MangoDriver;
pub use model::*;
pub use niri::NiriDriver;
pub use river::RiverDriver;
pub use sway::SwayDriver;
pub use umbriel::UmbrielDriver;

/// Detects the running compositor from the environment. Umbriel exports
/// `UMBRIEL_SOCKET` and always sets `XDG_CURRENT_DESKTOP=umbriel` for its
/// clients, so that desktop name wins even over variables inherited from an
/// outer compositor when Umbriel runs nested. Sway exports `SWAYSOCK` (and
/// `I3SOCK`) but leaves `XDG_CURRENT_DESKTOP` to the session: when that
/// says `sway`, it wins the same way; otherwise `SWAYSOCK` is checked after
/// the others, so a compositor nested inside sway (inheriting `SWAYSOCK`)
/// is still found by its own variable. River sets no variable of its own:
/// a session that names it (`XDG_CURRENT_DESKTOP=river`) is River once no
/// compositor's own variable matched (a compositor nested in River keeps
/// its own), and when nothing matched at all, the display is asked once
/// whether it offers River's control and status protocols.
pub fn detect() -> Option<CompositorKind> {
    detect_from(|name| std::env::var_os(name)).or_else(|| (std::env::var_os("WAYLAND_DISPLAY").is_some() && river::running()).then_some(CompositorKind::River))
}

/// [`detect`] over any environment.
pub fn detect_from(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<CompositorKind> {
    let set = |name: &str| var(name).is_some_and(|v| !v.is_empty());
    let desktop = |name: &str| var("XDG_CURRENT_DESKTOP").is_some_and(|d| d.to_string_lossy().split(':').any(|d| d.eq_ignore_ascii_case(name)));
    if desktop("umbriel") {
        Some(CompositorKind::Umbriel)
    } else if desktop("sway") && set("SWAYSOCK") {
        Some(CompositorKind::Sway)
    } else if set("MANGO_INSTANCE_SIGNATURE") || desktop("mango") {
        Some(CompositorKind::MangoWM)
    } else if set("NIRI_SOCKET") {
        Some(CompositorKind::Niri)
    } else if set("HYPRLAND_INSTANCE_SIGNATURE") {
        Some(CompositorKind::Hyprland)
    } else if set("UMBRIEL_SOCKET") {
        Some(CompositorKind::Umbriel)
    } else if set("SWAYSOCK") {
        Some(CompositorKind::Sway)
    } else if desktop("river") {
        Some(CompositorKind::River)
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
        CompositorKind::Umbriel => UmbrielDriver::from_env().map(|d| Arc::new(d) as Arc<dyn CompositorBackend>),
        CompositorKind::Sway => SwayDriver::from_env().map(|d| Arc::new(d) as Arc<dyn CompositorBackend>),
        CompositorKind::River => RiverDriver::from_env().map(|d| Arc::new(d) as Arc<dyn CompositorBackend>),
    }
}

/// Events buffered between a driver's watcher thread and the UI before the
/// watcher waits (back-pressure instead of unbounded growth).
pub(crate) const EVENT_QUEUE: usize = 64;
/// Read timeout of one-shot IPC requests.
pub(crate) const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(2);
/// Pause before reconnecting a dropped event stream.
pub(crate) const RECONNECT_DELAY: std::time::Duration = std::time::Duration::from_secs(1);

#[cfg(test)]
mod tests {
    use super::*;

    fn detect_with(vars: &[(&str, &str)]) -> Option<CompositorKind> {
        detect_from(|name| vars.iter().find(|(n, _)| *n == name).map(|(_, v)| v.into()))
    }

    #[test]
    fn detection_never_mistakes_one_compositor_for_another() {
        assert_eq!(detect_with(&[]), None);
        assert_eq!(detect_with(&[("SWAYSOCK", "/run/user/1000/sway-ipc.1000.42.sock"), ("I3SOCK", "/run/user/1000/sway-ipc.1000.42.sock")]), Some(CompositorKind::Sway));
        // Niri or Hyprland nested in sway inherit SWAYSOCK: their own
        // variable wins.
        assert_eq!(detect_with(&[("SWAYSOCK", "/s"), ("NIRI_SOCKET", "/n")]), Some(CompositorKind::Niri));
        assert_eq!(detect_with(&[("SWAYSOCK", "/s"), ("HYPRLAND_INSTANCE_SIGNATURE", "abc")]), Some(CompositorKind::Hyprland));
        assert_eq!(detect_with(&[("SWAYSOCK", "/s"), ("MANGO_INSTANCE_SIGNATURE", "/m")]), Some(CompositorKind::MangoWM));
        // Sway nested in Niri, with a session that names it.
        assert_eq!(detect_with(&[("SWAYSOCK", "/s"), ("NIRI_SOCKET", "/n"), ("XDG_CURRENT_DESKTOP", "sway")]), Some(CompositorKind::Sway));
        // The desktop name alone isn't a running sway.
        assert_eq!(detect_with(&[("XDG_CURRENT_DESKTOP", "sway")]), None);
        assert_eq!(detect_with(&[("XDG_CURRENT_DESKTOP", "sway:wlroots"), ("SWAYSOCK", "/s")]), Some(CompositorKind::Sway));
        assert_eq!(detect_with(&[("XDG_CURRENT_DESKTOP", "umbriel"), ("SWAYSOCK", "/s")]), Some(CompositorKind::Umbriel));
        assert_eq!(detect_with(&[("SWAYSOCK", "")]), None, "an empty variable is unset");
        // River sets nothing itself; a session naming it is River, and a
        // compositor nested in it (inheriting that name) is still found by
        // its own variable.
        assert_eq!(detect_with(&[("XDG_CURRENT_DESKTOP", "river")]), Some(CompositorKind::River));
        assert_eq!(detect_with(&[("XDG_CURRENT_DESKTOP", "river"), ("NIRI_SOCKET", "/n")]), Some(CompositorKind::Niri));
        assert_eq!(detect_with(&[("XDG_CURRENT_DESKTOP", "river"), ("SWAYSOCK", "/s")]), Some(CompositorKind::Sway));
        assert_eq!(detect_with(&[("NIRI_SOCKET", "/n")]), Some(CompositorKind::Niri));
    }
}
