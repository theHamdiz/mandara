//! Pieces shared by the throw-away headless MangoWM sessions
//! (`resource-check`, `sandbox`): the private session bus configuration and
//! the sockets the compositor creates in the session's runtime dir.

use std::path::{Path, PathBuf};
use std::time::Duration;

/// The private session bus socket, inside the runtime dir.
pub const BUS_SOCKET: &str = "bus";
/// How long the compositor gets to create its Wayland socket.
pub const COMPOSITOR_TIMEOUT: Duration = Duration::from_secs(10);
/// A system bus address nothing listens on: nothing in a headless session may
/// reach the real NetworkManager, logind or BlueZ.
pub const DEAD_SYSTEM_BUS: &str = "unix:path=/nonexistent";
/// Session variables of the real desktop that must never leak into a
/// headless session.
pub const SESSION_VARS: [&str; 5] = ["WAYLAND_DISPLAY", "MANGO_INSTANCE_SIGNATURE", "NIRI_SOCKET", "HYPRLAND_INSTANCE_SIGNATURE", "DISPLAY"];

/// A `dbus-daemon` configuration for a session bus listening only at
/// `<runtime>/bus`, open to everything on it.
pub fn bus_config(runtime: &Path) -> String {
    format!(
        r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN" "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig><type>session</type><listen>unix:path={}</listen><auth>EXTERNAL</auth>
<policy context="default"><allow send_destination="*" eavesdrop="true"/><allow eavesdrop="true"/><allow own="*"/></policy></busconfig>
"#,
        runtime.join(BUS_SOCKET).display()
    )
}

/// The session bus address for a runtime dir.
pub fn bus_address(runtime: &Path) -> String {
    format!("unix:path={}", runtime.join(BUS_SOCKET).display())
}

/// Files in `dir` whose names satisfy `want`, sorted.
fn matching(dir: &Path, want: impl Fn(&str) -> bool) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|d| d.flatten().filter(|e| e.file_name().to_str().is_some_and(&want)).map(|e| e.path()).collect())
        .unwrap_or_default();
    found.sort();
    found
}

/// The compositor's Wayland socket (`wayland-N`) in `runtime`.
pub fn wayland_socket(runtime: &Path) -> Option<PathBuf> {
    matching(runtime, |n| n.starts_with("wayland-") && !n.ends_with(".lock")).into_iter().next()
}

/// MangoWM's IPC socket (`mango-*.sock`) in `runtime`.
pub fn mango_socket(runtime: &Path) -> Option<PathBuf> {
    matching(runtime, |n| n.starts_with("mango-") && n.ends_with(".sock")).into_iter().next()
}

/// A MangoWM monitor rule for the headless output.
pub fn monitor_rule(width: u32, height: u32, refresh: u32, scale: f64) -> String {
    format!("monitor_rule=name:^HEADLESS-1$,width:{width},height:{height},refresh:{refresh},x:0,y:0,scale:{scale},primary:1")
}

/// A second headless output (`HEADLESS-2`) at scale 1, `x` px right of the
/// first: multi-monitor checks.
pub fn second_monitor_rule(width: u32, height: u32, refresh: u32, x: u32) -> String {
    format!("monitor_rule=name:^HEADLESS-2$,width:{width},height:{height},refresh:{refresh},x:{x},y:0,scale:1")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bus_listens_only_in_the_runtime_dir() {
        let conf = bus_config(Path::new("/tmp/x"));
        assert!(conf.contains("<listen>unix:path=/tmp/x/bus</listen>"));
        assert_eq!(bus_address(Path::new("/tmp/x")), "unix:path=/tmp/x/bus");
    }

    #[test]
    fn monitor_rule_matches_the_script() {
        assert_eq!(monitor_rule(3840, 2160, 144, 1.5), "monitor_rule=name:^HEADLESS-1$,width:3840,height:2160,refresh:144,x:0,y:0,scale:1.5,primary:1");
    }
}
