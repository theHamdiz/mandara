//! The files `mandara install` puts in place and `mandara uninstall`
//! removes, all under the user's own prefix and XDG folders.

use std::path::{Path, PathBuf};

/// Studio's desktop entry and icon, embedded so the binary installs on its
/// own (from a release tarball, `cargo install` or a bare download).
pub const DESKTOP_ENTRY: &str = include_str!("../../../packaging/org.mandara.studio.desktop");
pub const ICON_SVG: &[u8] = include_bytes!("../../../packaging/org.mandara.studio.svg");
pub const DESKTOP_FILE: &str = "org.mandara.studio.desktop";
pub const ICON_FILE: &str = "org.mandara.studio.svg";
/// Where they sit below a data folder (and inside a release tarball's `share/`).
pub const APPLICATIONS_DIR: &str = "applications";
pub const ICON_DIR: &str = "icons/hicolor/scalable/apps";
/// The binary's name, in the prefix's `bin/`.
pub const BINARY: &str = "mandara";
/// Mandara's plugin folders below the data folder.
pub const PLUGIN_DIRS: &[&str] = &["mandara/plugins", "mandara/plugins-dev"];
/// The compositor drop-ins Mandara generates (`~/.config/mandara/generated/`).
pub const GENERATED_DIR: &str = "mandara/generated";
pub const GENERATED_DROPINS: &[&str] = &["mango.conf", "hyprland.conf", "niri.kdl", "hyprland.lua"];
/// Mode of the installed binary.
const MODE_EXECUTABLE: u32 = 0o755;
/// The `Exec=` value the packaged desktop entry starts with.
const EXEC_PREFIX: &str = "Exec=mandara";

/// Where an installation lives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Paths {
    /// `<prefix>/bin`.
    pub bin_dir: PathBuf,
    /// `$XDG_DATA_HOME` (`~/.local/share`).
    pub data_dir: PathBuf,
    /// `$XDG_CONFIG_HOME` (`~/.config`).
    pub config_dir: PathBuf,
}

impl Paths {
    /// From `--prefix` (default `~/.local`) and the XDG variables.
    pub fn new(prefix: Option<&Path>, home: &Path, env: impl Fn(&str) -> Option<String>) -> Paths {
        let xdg = |key: &str, fallback: &str| env(key).filter(|v| v.starts_with('/')).map(PathBuf::from).unwrap_or_else(|| home.join(fallback));
        let prefix = prefix.map(Path::to_path_buf).unwrap_or_else(|| home.join(".local"));
        Paths { bin_dir: prefix.join("bin"), data_dir: xdg("XDG_DATA_HOME", ".local/share"), config_dir: xdg("XDG_CONFIG_HOME", ".config") }
    }

    pub fn binary(&self) -> PathBuf {
        self.bin_dir.join(BINARY)
    }

    pub fn desktop_entry(&self) -> PathBuf {
        self.data_dir.join(APPLICATIONS_DIR).join(DESKTOP_FILE)
    }

    pub fn icon(&self) -> PathBuf {
        self.data_dir.join(ICON_DIR).join(ICON_FILE)
    }

    /// Every folder an install creates.
    pub fn dirs(&self) -> Vec<PathBuf> {
        let mut out = vec![self.bin_dir.clone(), self.data_dir.join(APPLICATIONS_DIR), self.data_dir.join(ICON_DIR)];
        out.extend(PLUGIN_DIRS.iter().map(|d| self.data_dir.join(d)));
        out
    }

    pub fn generated(&self) -> PathBuf {
        self.config_dir.join(GENERATED_DIR)
    }
}

/// The desktop entry with `Exec=mandara…` pointing at the installed binary
/// (the prefix need not be on `$PATH`).
pub fn desktop_entry(template: &str, binary: &Path) -> String {
    let exec = format!("Exec={}", binary.display());
    let mut out: String = template.lines().map(|l| if let Some(rest) = l.strip_prefix(EXEC_PREFIX) { format!("{exec}{rest}\n") } else { format!("{l}\n") }).collect();
    if !template.ends_with('\n') {
        out.pop();
    }
    out
}

/// Copies `src` to `dst` atomically (`.mandara.new` next to it, then a
/// rename), so a running shell keeps its old executable. Nothing happens
/// when both are the same file.
pub fn install_binary(src: &Path, dst: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    if same_file(src, dst) {
        return Ok(());
    }
    let tmp = dst.with_file_name(format!(".{}.new", dst.file_name().unwrap_or_default().to_string_lossy()));
    std::fs::copy(src, &tmp)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(MODE_EXECUTABLE))?;
    std::fs::rename(&tmp, dst).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Whether two paths are the same file.
pub fn same_file(a: &Path, b: &Path) -> bool {
    matches!((std::fs::canonicalize(a), std::fs::canonicalize(b)), (Ok(x), Ok(y)) if x == y)
}

/// What an uninstalled drop-in is replaced with: a comment in its own
/// syntax. Emptied rather than deleted, because Niri before 26.04 refuses a
/// config whose include file is missing.
pub fn emptied_dropin(file: &str) -> String {
    let comment = if file.ends_with(".kdl") {
        "//"
    } else if file.ends_with(".lua") {
        "--"
    } else {
        "#"
    };
    format!("{comment} Mandara Shell was uninstalled; reinstall it and run: mandara setup\n")
}

/// Whether `dir` is a Mandara source checkout.
pub fn is_checkout(dir: &Path) -> bool {
    std::fs::read_to_string(dir.join("Cargo.toml")).is_ok_and(|t| t.contains("crates/mandara-app"))
}

/// The checkout a cargo-built binary came from (`<root>/target/<profile>/
/// mandara` or `<root>/target/<triple>/<profile>/mandara`).
pub fn checkout_of(exe: &Path) -> Option<PathBuf> {
    exe.ancestors().skip(1).take(4).find(|d| d.file_name().is_some_and(|n| n == "target")).and_then(Path::parent).filter(|d| is_checkout(d)).map(Path::to_path_buf)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_follow_prefix_and_xdg() {
        let home = Path::new("/home/u");
        let p = Paths::new(None, home, |_| None);
        assert_eq!(p.binary(), Path::new("/home/u/.local/bin/mandara"));
        assert_eq!(p.desktop_entry(), Path::new("/home/u/.local/share/applications/org.mandara.studio.desktop"));
        assert_eq!(p.icon(), Path::new("/home/u/.local/share/icons/hicolor/scalable/apps/org.mandara.studio.svg"));
        assert_eq!(p.generated(), Path::new("/home/u/.config/mandara/generated"));
        let p = Paths::new(Some(Path::new("/opt/m")), home, |k| match k {
            "XDG_DATA_HOME" => Some("/d".into()),
            "XDG_CONFIG_HOME" => Some("relative/ignored".into()),
            _ => None,
        });
        assert_eq!(p.bin_dir, Path::new("/opt/m/bin"));
        assert_eq!(p.data_dir, Path::new("/d"));
        assert_eq!(p.config_dir, Path::new("/home/u/.config"));
        assert!(p.dirs().contains(&PathBuf::from("/d/mandara/plugins-dev")));
    }

    #[test]
    fn desktop_entry_points_at_the_installed_binary() {
        let out = desktop_entry(DESKTOP_ENTRY, Path::new("/home/u/.local/bin/mandara"));
        assert!(out.contains("\nExec=/home/u/.local/bin/mandara studio\n"), "{out}");
        assert!(!out.contains("Exec=mandara"));
        assert_eq!(out.lines().count(), DESKTOP_ENTRY.lines().count());
        assert!(out.contains("Name[ar]=ستوديو المندرة"));
        assert!(out.starts_with("[Desktop Entry]\n"));
        assert_eq!(desktop_entry("Exec=mandara x", Path::new("/b/mandara")), "Exec=/b/mandara x");
        assert!(ICON_SVG.starts_with(b"<svg") || ICON_SVG.starts_with(b"<?xml"));
    }

    #[test]
    fn binary_is_replaced_atomically() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let (src, dst) = (dir.path().join("src"), dir.path().join("mandara"));
        std::fs::write(&src, b"new").unwrap();
        std::fs::write(&dst, b"old").unwrap();
        install_binary(&src, &dst).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"new");
        assert_eq!(std::fs::metadata(&dst).unwrap().permissions().mode() & 0o777, 0o755);
        assert!(!dir.path().join(".mandara.new").exists());
        // Installing the installed binary over itself is a no-op.
        install_binary(&dst, &dst).unwrap();
        assert_eq!(std::fs::read(&dst).unwrap(), b"new");
    }

    #[test]
    fn dropins_are_emptied_in_their_own_syntax() {
        assert!(emptied_dropin("niri.kdl").starts_with("// "));
        assert!(emptied_dropin("hyprland.lua").starts_with("-- "));
        assert!(emptied_dropin("mango.conf").starts_with("# "));
        assert!(emptied_dropin("hyprland.conf").contains("mandara setup"));
    }

    #[test]
    fn finds_the_checkout_of_a_cargo_build() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "members = [\"crates/mandara-app\"]").unwrap();
        assert!(is_checkout(dir.path()));
        assert_eq!(checkout_of(&dir.path().join("target/release/mandara")).as_deref(), Some(dir.path()));
        assert_eq!(checkout_of(&dir.path().join("target/x86_64-unknown-linux-musl/release/mandara")).as_deref(), Some(dir.path()));
        assert_eq!(checkout_of(Path::new("/home/u/.local/bin/mandara")), None);
    }
}
