//! First-class compositor integration behind one [`CompositorBackend`]
//! trait: MangoWM (bitmask tags, native socket), Niri (scrollable
//! workspaces, JSON socket), Hyprland (workspaces, socket/socket2),
//! Umbriel (per-output workspaces, JSON-lines socket), Sway (per-output
//! workspaces, i3 IPC), River (bitmask tags, its own Wayland protocols),
//! KWin (Plasma window and virtual-desktop Wayland protocols) and Aqueous
//! (per-output workspaces, acknowledged JSON state batches).

pub mod aqueous;
pub mod backend;
pub mod hyprland;
pub mod mango;
pub mod kwin;
pub mod model;
pub mod niri;
pub mod river;
pub mod sway;
pub mod umbriel;

use std::sync::Arc;

pub use aqueous::AqueousDriver;
pub use backend::CompositorBackend;
pub use hyprland::HyprlandDriver;
pub use mango::MangoDriver;
pub use kwin::KWinDriver;
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
/// whether it offers River's control and status protocols. Aqueous exports
/// `AQUEOUS_SOCKET` to its session (and names itself in
/// `XDG_CURRENT_DESKTOP`): with both, it wins like sway does; the socket
/// alone is checked after the other compositors' own variables, so one
/// nested inside Aqueous is still found (a nested Niri, which inherits
/// Aqueous's desktop name too, by its socket naming this display). Aqueous derives from River but
/// offers none of River's protocols.
///
/// A compositor nested inside another inherits the outer one's variables,
/// and some (Sway, Niri outside a full session) don't rename the desktop,
/// so two compositors' variables can be equally plausible. Then the
/// process serving this Wayland display decides (Linux), and the rules
/// above apply when it can't be told.
pub fn detect() -> Option<CompositorKind> {
    let env = |name: &str| std::env::var_os(name);
    detect_with_owner(env, display_owner).or_else(|| (std::env::var_os("WAYLAND_DISPLAY").is_some() && river::running()).then_some(CompositorKind::River))
}

/// The variables a compositor exports for its own session.
const OWN_VARIABLES: [&str; 6] = ["MANGO_INSTANCE_SIGNATURE", "NIRI_SOCKET", "HYPRLAND_INSTANCE_SIGNATURE", "UMBRIEL_SOCKET", "SWAYSOCK", "AQUEOUS_SOCKET"];

/// [`detect_from`], except that when several compositors' own variables
/// are set (one runs nested inside another), `owner` (the compositor
/// serving this display, asked only then) decides when it is known.
fn detect_with_owner(var: impl Fn(&str) -> Option<std::ffi::OsString>, owner: impl FnOnce() -> Option<CompositorKind>) -> Option<CompositorKind> {
    let set = OWN_VARIABLES.iter().filter(|name| var(name).is_some_and(|v| !v.is_empty())).count();
    if set > 1
        && let Some(kind) = owner()
    {
        return Some(kind);
    }
    detect_from(var)
}

/// The compositor serving this process's Wayland display: the peer of its
/// socket, by process name. `None` when that can't be told (no display,
/// an unknown process, or a system without peer process ids).
#[cfg(target_os = "linux")]
fn display_owner() -> Option<CompositorKind> {
    let display = std::path::PathBuf::from(std::env::var_os("WAYLAND_DISPLAY")?);
    let path = if display.is_absolute() { display } else { std::path::PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join(display) };
    let stream = std::os::unix::net::UnixStream::connect(path).ok()?;
    let pid = rustix::net::sockopt::socket_peercred(&stream).ok()?.pid;
    let name = std::fs::read_to_string(format!("/proc/{}/comm", pid.as_raw_nonzero())).ok()?;
    kind_of_process(name.trim())
}

#[cfg(not(target_os = "linux"))]
fn display_owner() -> Option<CompositorKind> {
    None
}

/// A compositor by its process name (`/proc/<pid>/comm`, at most 15 bytes).
#[cfg(any(target_os = "linux", test))]
fn kind_of_process(name: &str) -> Option<CompositorKind> {
    match name {
        "mango" => Some(CompositorKind::MangoWM),
        "niri" => Some(CompositorKind::Niri),
        "Hyprland" | "hyprland" => Some(CompositorKind::Hyprland),
        "umbriel" => Some(CompositorKind::Umbriel),
        "sway" => Some(CompositorKind::Sway),
        "river" => Some(CompositorKind::River),
        "kwin_wayland" => Some(CompositorKind::KWin),
        "aqueous" => Some(CompositorKind::Aqueous),
        _ => None,
    }
}

/// [`detect`], answered once per process: the compositor a process runs
/// under can't change, and render paths ask on every frame (each [`detect`]
/// reads several environment variables, and may probe for River).
pub fn detected() -> Option<CompositorKind> {
    static KIND: std::sync::OnceLock<Option<CompositorKind>> = std::sync::OnceLock::new();
    *KIND.get_or_init(detect)
}

/// [`detect`] over any environment.
pub fn detect_from(var: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<CompositorKind> {
    let set = |name: &str| var(name).is_some_and(|v| !v.is_empty());
    let desktop = |name: &str| var("XDG_CURRENT_DESKTOP").is_some_and(|d| d.to_string_lossy().split(':').any(|d| d.eq_ignore_ascii_case(name)));
    // A Niri nested inside Aqueous inherits Aqueous's socket and desktop
    // name (Niri names itself only when it runs a whole session), but its
    // own socket names the Wayland display it serves.
    let niri_here = var("NIRI_SOCKET").zip(var("WAYLAND_DISPLAY")).is_some_and(|(socket, display)| niri_serves(&socket, &display));
    if desktop("umbriel") {
        Some(CompositorKind::Umbriel)
    } else if desktop("sway") && set("SWAYSOCK") {
        Some(CompositorKind::Sway)
    } else if desktop("aqueous") && set("AQUEOUS_SOCKET") && !niri_here {
        Some(CompositorKind::Aqueous)
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
    } else if set("AQUEOUS_SOCKET") {
        Some(CompositorKind::Aqueous)
    } else if desktop("KDE") || desktop("plasma") {
        Some(CompositorKind::KWin)
    } else if desktop("river") {
        Some(CompositorKind::River)
    } else {
        None
    }
}

/// Whether Niri's IPC socket belongs to the Wayland display `display`:
/// Niri names it `niri.<display>.<pid>.sock` (`display` may also be a path).
fn niri_serves(socket: &std::ffi::OsStr, display: &std::ffi::OsStr) -> bool {
    let name = |p: &std::ffi::OsStr| std::path::Path::new(p).file_name().map(|n| n.to_string_lossy().into_owned());
    let (Some(socket), Some(display)) = (name(socket), name(display)) else { return false };
    !display.is_empty() && socket.strip_prefix("niri.").and_then(|rest| rest.strip_prefix(display.as_str())).is_some_and(|rest| rest.starts_with('.'))
}

/// Starts an interactive move of the window under the pointer, for client-
/// side title bars. MangoWM ignores `xdg_toplevel.move`, so it is asked
/// over IPC to run its own `moveresize,curmove` grab (released with the
/// button). Returns `false` where the standard request already works.
pub fn begin_interactive_move() -> bool {
    match detected() {
        Some(CompositorKind::MangoWM) => mango::MangoDriver::from_env().is_some_and(|c| c.dispatch("moveresize", &["curmove"]).is_ok()),
        _ => false,
    }
}

/// Connects to the running compositor's native IPC.
pub fn connect() -> Option<Arc<dyn CompositorBackend>> {
    match detected()? {
        CompositorKind::MangoWM => MangoDriver::from_env().map(|d| Arc::new(d) as Arc<dyn CompositorBackend>),
        CompositorKind::Niri => NiriDriver::from_env().map(|d| Arc::new(d) as Arc<dyn CompositorBackend>),
        CompositorKind::Hyprland => HyprlandDriver::from_env().map(|d| Arc::new(d) as Arc<dyn CompositorBackend>),
        CompositorKind::Umbriel => UmbrielDriver::from_env().map(|d| Arc::new(d) as Arc<dyn CompositorBackend>),
        CompositorKind::Sway => SwayDriver::from_env().map(|d| Arc::new(d) as Arc<dyn CompositorBackend>),
        CompositorKind::River => RiverDriver::from_env().map(|d| Arc::new(d) as Arc<dyn CompositorBackend>),
        CompositorKind::KWin => KWinDriver::from_env().map(|d| Arc::new(d) as Arc<dyn CompositorBackend>),
        CompositorKind::Aqueous => AqueousDriver::from_env().map(|d| Arc::new(d) as Arc<dyn CompositorBackend>),
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
        assert_eq!(detect_with(&[("XDG_CURRENT_DESKTOP", "KDE")]), Some(CompositorKind::KWin));
        assert_eq!(detect_with(&[("XDG_CURRENT_DESKTOP", "Plasma")]), Some(CompositorKind::KWin));
        assert_eq!(detect_with(&[("XDG_CURRENT_DESKTOP", "KDE"), ("NIRI_SOCKET", "/n")]), Some(CompositorKind::Niri));
        // Aqueous: its socket, never its desktop name alone (and never
        // River, which it derives from).
        let socket = ("AQUEOUS_SOCKET", "/run/user/1000/aqueous/0f3c/ipc.sock");
        assert_eq!(detect_with(&[socket]), Some(CompositorKind::Aqueous));
        assert_eq!(detect_with(&[socket, ("XDG_CURRENT_DESKTOP", "Aqueous")]), Some(CompositorKind::Aqueous));
        assert_eq!(detect_with(&[("XDG_CURRENT_DESKTOP", "Aqueous")]), None);
        assert_eq!(detect_with(&[("AQUEOUS_SOCKET", "")]), None);
        // A compositor nested in Aqueous inherits its socket: its own
        // variable wins; Aqueous nested elsewhere names itself.
        assert_eq!(detect_with(&[socket, ("NIRI_SOCKET", "/n")]), Some(CompositorKind::Niri));
        assert_eq!(detect_with(&[socket, ("SWAYSOCK", "/s")]), Some(CompositorKind::Sway));
        assert_eq!(detect_with(&[socket, ("NIRI_SOCKET", "/n"), ("XDG_CURRENT_DESKTOP", "Aqueous")]), Some(CompositorKind::Aqueous));
        // Niri nested inside Aqueous (Aqueous's markers inherited): Niri's
        // socket names the display Mandara is on. Aqueous nested inside
        // Niri: the inherited Niri socket names the outer display.
        let inner_niri = [socket, ("XDG_CURRENT_DESKTOP", "Aqueous"), ("NIRI_SOCKET", "/run/user/1000/niri.wayland-2.4242.sock"), ("WAYLAND_DISPLAY", "wayland-2")];
        assert_eq!(detect_with(&inner_niri), Some(CompositorKind::Niri));
        let inner_aqueous = [socket, ("XDG_CURRENT_DESKTOP", "Aqueous"), ("NIRI_SOCKET", "/run/user/1000/niri.wayland-1.77.sock"), ("WAYLAND_DISPLAY", "wayland-2")];
        assert_eq!(detect_with(&inner_aqueous), Some(CompositorKind::Aqueous));
        // `wayland-1` is not `wayland-10`.
        let near_miss = [socket, ("XDG_CURRENT_DESKTOP", "Aqueous"), ("NIRI_SOCKET", "/run/user/1000/niri.wayland-10.77.sock"), ("WAYLAND_DISPLAY", "wayland-1")];
        assert_eq!(detect_with(&near_miss), Some(CompositorKind::Aqueous));
        let absolute = [socket, ("XDG_CURRENT_DESKTOP", "Aqueous"), ("NIRI_SOCKET", "/run/user/1000/niri.wayland-2.1.sock"), ("WAYLAND_DISPLAY", "/run/user/1000/wayland-2")];
        assert_eq!(detect_with(&absolute), Some(CompositorKind::Niri));
        // Sway nested inside Aqueous and Aqueous nested inside Sway carry
        // the same variables (Sway doesn't rename the desktop): the process
        // serving the display decides, both ways round.
        let both = [socket, ("XDG_CURRENT_DESKTOP", "Aqueous"), ("SWAYSOCK", "/run/user/1000/sway-ipc.1000.42.sock")];
        let lookup = |name: &str| both.iter().find(|(n, _)| *n == name).map(|(_, v)| std::ffi::OsString::from(v));
        assert_eq!(detect_with_owner(lookup, || kind_of_process("sway")), Some(CompositorKind::Sway));
        assert_eq!(detect_with_owner(lookup, || kind_of_process("aqueous")), Some(CompositorKind::Aqueous));
        // An unknown display owner leaves the variable rules in charge.
        assert_eq!(detect_with_owner(lookup, || kind_of_process("weston")), Some(CompositorKind::Aqueous));
        // One compositor's variables alone: the display is never asked.
        let alone = |name: &str| (name == "SWAYSOCK").then(|| std::ffi::OsString::from("/s"));
        assert_eq!(detect_with_owner(alone, || panic!("asked without a reason")), Some(CompositorKind::Sway));
        // Sway nested inside Hyprland (Hyprland's variable comes first).
        let hypr = |name: &str| match name {
            "HYPRLAND_INSTANCE_SIGNATURE" => Some(std::ffi::OsString::from("abc")),
            "SWAYSOCK" => Some("/s".into()),
            _ => None,
        };
        assert_eq!(detect_with_owner(hypr, || kind_of_process("sway")), Some(CompositorKind::Sway));
        assert_eq!(kind_of_process("Hyprland"), Some(CompositorKind::Hyprland));
        assert_eq!(kind_of_process("kwin_wayland"), Some(CompositorKind::KWin));
        assert_eq!(detect_with(&[socket, ("XDG_CURRENT_DESKTOP", "river")]), Some(CompositorKind::Aqueous));
    }
}
