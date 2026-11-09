//! Mandara is pure Wayland. GPUI and its platform crates are built without
//! their `x11` features, and no X11 crate may enter a dependency graph: a
//! default feature turned back on, or a new dependency that pulls in X11,
//! fails here (`cargo test --workspace`, in CI) before it can ship.
//!
//! The lockfiles are read rather than `cargo tree`: a lockfile is the graph
//! resolved for every target and every feature the workspace enables, so a
//! crate that only some platform or build would link is caught too.

use std::path::{Path, PathBuf};

/// Crates that only exist to talk to X11: the X protocol bindings (x11rb,
/// XCB, Xlib), xkbcommon's `x11` feature (which is what pulls in
/// `as-raw-xcb-connection`), X11 input methods and the X11 clipboard.
const X11_CRATES: &[&str] = &[
    "x11rb",
    "x11rb-protocol",
    "xcb",
    "x11",
    "x11-dl",
    "x11-clipboard",
    "xkbcommon-x11",
    "as-raw-xcb-connection",
    "xim",
    "zed-xim",
    "xcb-imdkit",
];

/// The X11 packages a lockfile resolves, sorted.
fn x11_packages(lock: &str) -> Result<Vec<String>, String> {
    let lock: toml::Table = lock.parse().map_err(|e| format!("not a lockfile: {e}"))?;
    let packages = lock.get("package").and_then(toml::Value::as_array).ok_or("a lockfile without packages")?;
    let mut found: Vec<String> = packages
        .iter()
        .filter_map(|p| Some(format!("{} {}", p.get("name")?.as_str()?, p.get("version")?.as_str()?)))
        .filter(|p| p.split_once(' ').is_some_and(|(name, _)| X11_CRATES.contains(&name)))
        .collect();
    found.sort();
    Ok(found)
}

/// Every lockfile Mandara ships from: the workspace, the delta tool (its
/// own root) and each example plugin.
fn lockfiles(root: &Path) -> Vec<PathBuf> {
    let mut locks = vec![root.join("Cargo.lock"), root.join("tools/mandara-delta/Cargo.lock")];
    let mut plugins: Vec<PathBuf> = std::fs::read_dir(root.join("plugins/examples")).into_iter().flatten().flatten().map(|e| e.path().join("Cargo.lock")).filter(|p| p.is_file()).collect();
    plugins.sort();
    locks.extend(plugins);
    locks
}

#[test]
fn no_x11_crate_enters_any_dependency_graph() {
    let root = crate::root();
    for lock in lockfiles(&root) {
        let text = std::fs::read_to_string(&lock).unwrap_or_else(|e| panic!("{}: {e}", lock.display()));
        let found = x11_packages(&text).unwrap_or_else(|e| panic!("{}: {e}", lock.display()));
        assert!(
            found.is_empty(),
            "{} resolves X11 crates {found:?}: Mandara is pure Wayland. Find what pulls them in with `cargo tree -i <crate>` and build that dependency without its X11 feature (`default-features = false`).",
            lock.strip_prefix(&root).unwrap_or(&lock).display(),
        );
    }
}

#[test]
fn the_guard_reads_the_workspace_lockfile() {
    let root = crate::root();
    let text = std::fs::read_to_string(root.join("Cargo.lock")).unwrap();
    let lock: toml::Table = text.parse().unwrap();
    let names: Vec<&str> = lock["package"].as_array().unwrap().iter().filter_map(|p| p.get("name")?.as_str()).collect();
    // The crates the shell's Wayland path really uses: a guard that read
    // nothing would pass on any graph.
    for wayland in ["wayland-client", "xkbcommon", "gpui"] {
        assert!(names.contains(&wayland), "{wayland} missing from Cargo.lock");
    }
    assert!(lockfiles(&root).iter().all(|p| p.is_file()));
}

#[test]
fn the_guard_catches_x11_crates() {
    let lock = r#"
version = 4

[[package]]
name = "xkbcommon"
version = "0.8.0"
dependencies = ["as-raw-xcb-connection", "libc"]

[[package]]
name = "as-raw-xcb-connection"
version = "1.0.1"

[[package]]
name = "x11rb"
version = "0.13.2"

[[package]]
name = "wayland-client"
version = "0.31.11"

[[package]]
name = "xkeysym"
version = "0.2.1"
"#;
    assert_eq!(x11_packages(lock).unwrap(), ["as-raw-xcb-connection 1.0.1", "x11rb 0.13.2"], "xkbcommon's x11 feature shows as as-raw-xcb-connection");
    assert_eq!(x11_packages("version = 4\n[[package]]\nname = \"wayland-client\"\nversion = \"0.31.11\"\n").unwrap(), Vec::<String>::new());
    assert!(x11_packages("not toml [").is_err());
    assert!(x11_packages("version = 4\n").is_err(), "a lockfile with no packages is an error, not a pass");
}
