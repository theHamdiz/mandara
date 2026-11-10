//! The AUR step's `sudo` wrapper, started the way AUR helpers start it: by
//! bare name, found through `PATH` in the step's private folder. The real
//! `mandara` binary must run as the wrapper (handing over to the system's
//! `sudo -A`), not as Mandara's own command line.
#![cfg(target_os = "linux")]

use std::os::unix::fs::{PermissionsExt as _, symlink};
use std::process::Command;

/// Asks for `--version`, which needs no password: the system's sudo prints
/// its own version, and Mandara's command line would print "mandara …".
fn run_sudo_by_name(socket_beside_link: bool) -> String {
    let dir = tempfile::tempdir().unwrap();
    std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    symlink(env!("CARGO_BIN_EXE_mandara"), dir.path().join("sudo")).unwrap();
    let elsewhere = tempfile::tempdir().unwrap();
    let socket = if socket_beside_link { dir.path() } else { elsewhere.path() }.join("askpass.sock");
    let path = std::env::join_paths([dir.path().to_path_buf()].into_iter().chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()))).unwrap();
    let out = Command::new("sudo")
        .arg("--version")
        .env("PATH", path)
        .env("MANDARA_ASKPASS_SOCKET", &socket)
        .env_remove("MANDARA_ASKPASS_TOKEN")
        .output()
        .unwrap();
    format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr))
}

#[test]
fn the_wrapper_started_by_name_hands_over_to_the_system_sudo() {
    let said = run_sudo_by_name(true);
    // The system's sudo answers ("Sudo version …"), or, on a machine
    // without one, the wrapper says it can't find it ("sudo: …").
    assert!(said.starts_with("Sudo version") || said.starts_with("sudo: "), "dispatched to the wrapper: {said}");
    assert!(!said.starts_with("mandara"), "{said}");
}

#[test]
fn a_sudo_link_outside_the_step_folder_is_not_the_wrapper() {
    // Same link found by name, but the socket lives elsewhere: this is
    // not the step's own folder, so Mandara's command line runs.
    let said = run_sudo_by_name(false);
    assert!(!said.starts_with("Sudo version") && !said.starts_with("sudo: "), "not the wrapper: {said}");
}
