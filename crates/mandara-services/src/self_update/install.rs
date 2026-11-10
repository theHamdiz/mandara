//! Where the running binary came from, and replacing it safely.
//!
//! Mandara only replaces a binary it installed itself (`mandara install`'s
//! `~/.local/bin/mandara`, or any `--prefix` you can write to). Package
//! manager installs are reported and never touched; cargo build output is
//! refused unless forced.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use mandara_i18n::{t, tf};

/// Folders owned by the system or a package manager (`/usr/local` is the
/// admin's own and is judged by writability instead).
const SYSTEM_PREFIXES: &[&str] = &["/usr/", "/bin/", "/sbin/", "/lib/", "/lib64/", "/opt/", "/snap/", "/var/lib/flatpak/", "/gnu/store/"];
const LOCAL_PREFIX: &str = "/usr/local/";
/// Where Nix installs everything (Mandara's flake, NixOS, Home Manager,
/// `nix profile`); a profile's `bin/mandara` link resolves into it.
const NIX_STORE: &str = "/nix/store/";
/// The manager name of a Nix store install.
pub const NIX: &str = "nix";
/// Cargo profile folders (`target/<profile>/mandara`,
/// `target/<triple>/<profile>/mandara`).
const CARGO_PROFILES: &[&str] = &["debug", "release", "release-native", "profiling"];
/// Longest an ownership query (`pacman -Qo`, …) may take.
const OWNER_QUERY_TIMEOUT: Duration = Duration::from_secs(3);
const OWNER_QUERY_POLL: Duration = Duration::from_millis(20);
/// Suffix of the staged new binary next to the installed one.
const STAGED_SUFFIX: &str = ".new";
/// Suffix of the previous binary kept for `mandara update rollback`.
const BACKUP_SUFFIX: &str = ".old";
/// Mode of the installed binary.
const MODE_EXECUTABLE: u32 = 0o755;
/// First bytes of an ELF executable.
pub const ELF_MAGIC: &[u8] = b"\x7fELF";

/// How the running binary is installed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Install {
    /// Installed by Mandara (`mandara install` or a writable prefix): updatable.
    SelfManaged(PathBuf),
    /// Owned by a package manager (`pacman`, `apt`, `dnf`, …).
    Managed { path: PathBuf, manager: String },
    /// A cargo build (`target/release/mandara`).
    Dev(PathBuf),
    /// Neither managed nor writable by you.
    ReadOnly(PathBuf),
}

impl Install {
    pub fn path(&self) -> &Path {
        match self {
            Install::SelfManaged(p) | Install::Dev(p) | Install::ReadOnly(p) | Install::Managed { path: p, .. } => p,
        }
    }

    /// The binary to replace, or why it can't be (translated).
    pub fn updatable(&self, force_dev: bool) -> Result<&Path, String> {
        match self {
            Install::SelfManaged(p) => Ok(p),
            Install::Dev(p) if force_dev => Ok(p),
            Install::Dev(p) => Err(tf("{path} is a development build; rebuild it from source (or pass --force-dev)", &[("path", &p.display())])),
            Install::Managed { manager, .. } => Err(managed_note(manager)),
            Install::ReadOnly(p) => Err(tf("{path} isn't writable; re-run `mandara install` to update it", &[("path", &p.display())])),
        }
    }
}

/// "Installed with pacman — update through your package manager".
pub fn managed_note(manager: &str) -> String {
    match manager {
        "" => t("Installed by your system — update through your package manager").to_string(),
        NIX => t("Installed by Nix — update it with your flake (nixos-rebuild switch, home-manager switch or nix profile upgrade)").to_string(),
        _ => tf("Installed with {manager} — update through your package manager", &[("manager", &manager)]),
    }
}

/// The running binary's install.
pub fn detect() -> Install {
    classify(&mandara_config::paths::self_exe(), &system_owner)
}

/// Classifies `path`. `owner` names the package manager owning a file under
/// a system prefix (`None`: unknown).
pub fn classify(path: &Path, owner: &dyn Fn(&Path) -> Option<String>) -> Install {
    let path = path.to_path_buf();
    if is_cargo_output(&path) {
        return Install::Dev(path);
    }
    let text = path.to_string_lossy();
    // The Nix store is read-only and Nix's alone (a flake, NixOS, Home
    // Manager or `nix profile`); no other package manager is asked.
    if text.starts_with(NIX_STORE) {
        return Install::Managed { path, manager: NIX.into() };
    }
    if !text.starts_with(LOCAL_PREFIX) && SYSTEM_PREFIXES.iter().any(|p| text.starts_with(p)) {
        let manager = owner(&path).unwrap_or_default();
        return Install::Managed { path, manager };
    }
    if dir_writable(&path) { Install::SelfManaged(path) } else { Install::ReadOnly(path) }
}

/// `…/target/<profile>/mandara` or `…/target/<triple>/<profile>/mandara`.
pub fn is_cargo_output(path: &Path) -> bool {
    let parts: Vec<String> = path.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
    let n = parts.len();
    let profile_at = |i: usize| parts.get(i).is_some_and(|p| CARGO_PROFILES.contains(&p.as_str()));
    let target_at = |i: usize| parts.get(i).is_some_and(|p| p == "target");
    (n >= 3 && profile_at(n - 2) && target_at(n - 3)) || (n >= 4 && profile_at(n - 2) && target_at(n - 4))
}

fn dir_writable(path: &Path) -> bool {
    let dir = path.parent().unwrap_or(Path::new("/"));
    rustix::fs::access(dir, rustix::fs::Access::WRITE_OK).is_ok() && file_writable(path)
}

/// Whether you may write `path`. FreeBSD answers "text file busy" for a
/// program that is running (the shell's own binary, always), before it
/// looks at permissions; the update replaces the file by renaming a new
/// one over it, never by writing into it, so there the file's owner and
/// mode decide. Linux never refuses that way.
fn file_writable(path: &Path) -> bool {
    match rustix::fs::access(path, rustix::fs::Access::WRITE_OK) {
        Ok(()) => true,
        Err(rustix::io::Errno::TXTBSY) => owner_writable(path),
        Err(_) => false,
    }
}

/// `path` is yours and has its owner's write bit (or you are root).
fn owner_writable(path: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    const OWNER_WRITE: u32 = 0o200;
    let Ok(meta) = std::fs::metadata(path) else { return false };
    let me = rustix::process::geteuid();
    me.is_root() || (meta.uid() == me.as_raw() && meta.mode() & OWNER_WRITE != 0)
}

/// Runs a read-only ownership query; `true` when it exits 0 in time.
fn query(program: &str, args: &[&str], path: &Path) -> bool {
    let Ok(mut child) = Command::new(program).args(args).arg(path).env("LANG", "C").stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else { return false };
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if start.elapsed() < OWNER_QUERY_TIMEOUT => std::thread::sleep(OWNER_QUERY_POLL),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

/// The package manager owning `path`: `pacman -Qo`, `dpkg -S`, `rpm -qf`
/// (whichever is installed; all read-only).
pub fn system_owner(path: &Path) -> Option<String> {
    if query("pacman", &["-Qo"], path) {
        return Some("pacman".into());
    }
    if query("dpkg", &["-S"], path) {
        return Some("apt".into());
    }
    if query("rpm", &["-qf"], path) {
        return Some(if Path::new("/usr/bin/zypper").exists() { "zypper" } else { "dnf" }.into());
    }
    None
}

fn sibling(exe: &Path, suffix: &str) -> PathBuf {
    let mut name = exe.file_name().unwrap_or_default().to_os_string();
    name.push(suffix);
    exe.with_file_name(name)
}

/// The previous binary kept for rollback (`mandara.old`).
pub fn backup_path(exe: &Path) -> PathBuf {
    sibling(exe, BACKUP_SUFFIX)
}

fn fsync_dir(dir: &Path) {
    if let Ok(d) = std::fs::File::open(dir) {
        let _ = d.sync_all();
    }
}

/// Atomically replaces `exe` with `new`: writes `mandara.new` in the same
/// folder, fsyncs it, keeps the current binary as `mandara.old` (a hard
/// link, so a running copy is untouched) and renames the new one into
/// place. A running process keeps its old inode until it restarts.
pub fn replace(exe: &Path, new: &[u8]) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    if !new.starts_with(ELF_MAGIC) {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "not an executable"));
    }
    let dir = exe.parent().unwrap_or(Path::new("."));
    let staged = sibling(exe, STAGED_SUFFIX);
    let result = (|| {
        let mut f = std::fs::File::create(&staged)?;
        f.write_all(new)?;
        f.set_permissions(std::fs::Permissions::from_mode(MODE_EXECUTABLE))?;
        f.sync_all()?;
        drop(f);
        let backup = backup_path(exe);
        if exe.exists() {
            let _ = std::fs::remove_file(&backup);
            if std::fs::hard_link(exe, &backup).is_err() {
                std::fs::copy(exe, &backup)?;
            }
        }
        std::fs::rename(&staged, exe)?;
        fsync_dir(dir);
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&staged);
    }
    result
}

/// Puts `mandara.old` back in place of `exe`.
pub fn rollback(exe: &Path) -> Result<(), String> {
    let backup = backup_path(exe);
    if !backup.is_file() {
        return Err(tf("No previous version to roll back to ({path} is missing)", &[("path", &backup.display())]));
    }
    std::fs::rename(&backup, exe).map_err(|e| e.to_string())?;
    fsync_dir(exe.parent().unwrap_or(Path::new(".")));
    Ok(())
}

/// Whether another process (the shell or its supervisor) runs `exe`.
pub fn running_from(exe: &Path) -> bool {
    super::processes::other_runs(exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_managed_paths_are_never_self_updated() {
        let pacman = |_: &Path| Some("pacman".to_string());
        let nobody = |_: &Path| None;
        assert_eq!(classify(Path::new("/usr/bin/mandara"), &pacman), Install::Managed { path: "/usr/bin/mandara".into(), manager: "pacman".into() });
        assert_eq!(classify(Path::new("/usr/bin/mandara"), &nobody), Install::Managed { path: "/usr/bin/mandara".into(), manager: String::new() });
        assert!(matches!(classify(Path::new("/opt/mandara/bin/mandara"), &nobody), Install::Managed { .. }));
        // Managed installs explain themselves and refuse.
        let err = classify(Path::new("/usr/bin/mandara"), &pacman).updatable(true).unwrap_err();
        assert!(err.contains("pacman"), "{err}");
    }

    /// Issue #39: a Nix build (flake, NixOS module, Home Manager, `nix
    /// profile`) is never replaced in place, whatever else claims it, and
    /// says how Nix updates it.
    #[test]
    fn nix_store_installs_update_through_nix() {
        let pacman = |_: &Path| Some("pacman".to_string());
        for p in ["/nix/store/0c5mr2ljm3b4kx4ij2yr6dqyqv5p6fla-mandara-0.10.1/bin/mandara", "/nix/store/abc-mandara/bin/.mandara-wrapped"] {
            let install = classify(Path::new(p), &pacman);
            assert_eq!(install, Install::Managed { path: p.into(), manager: NIX.into() }, "{p}");
            for force_dev in [false, true] {
                let err = install.updatable(force_dev).unwrap_err();
                assert!(err.contains("Nix") && err.contains("nixos-rebuild"), "{err}");
            }
        }
        // Only the store itself: a home folder named nix stays yours.
        assert!(!matches!(classify(Path::new("/home/u/nix/store/bin/mandara"), &pacman), Install::Managed { .. }));
    }

    #[test]
    fn cargo_builds_are_dev_installs() {
        let nobody = |_: &Path| None;
        for p in ["/home/u/src/mandara/target/release/mandara", "/home/u/src/mandara/target/x86_64-unknown-linux-gnu/release/mandara", "/srv/m/target/debug/mandara"] {
            let install = classify(Path::new(p), &nobody);
            assert_eq!(install, Install::Dev(p.into()), "{p}");
            assert!(install.updatable(false).is_err());
            assert!(install.updatable(true).is_ok());
        }
        assert!(!is_cargo_output(Path::new("/home/u/.local/bin/mandara")));
        assert!(!is_cargo_output(Path::new("/home/u/target/bin/mandara")));
    }

    #[test]
    fn writable_prefixes_are_self_managed() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("bin/mandara");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::write(&exe, b"\x7fELF old").unwrap();
        assert_eq!(classify(&exe, &|_| None), Install::SelfManaged(exe.clone()));
        assert!(matches!(classify(Path::new("/usr/local/bin/definitely-not-here"), &|_| None), Install::ReadOnly(_) | Install::SelfManaged(_)));
    }

    /// The running binary stays updatable: FreeBSD refuses `access(W_OK)`
    /// on a program while it runs (ETXTBSY), which made Studio call every
    /// FreeBSD install read-only.
    #[test]
    fn a_running_binary_is_still_self_managed() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("bin/mandara");
        std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
        std::fs::copy("/bin/sleep", &exe).unwrap();
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(MODE_EXECUTABLE)).unwrap();
        let mut running = Command::new(&exe).arg("30").spawn().unwrap();
        let install = classify(&exe, &|_| None);
        let _ = running.kill();
        let _ = running.wait();
        assert_eq!(install, Install::SelfManaged(exe.clone()));
        assert!(owner_writable(&exe));
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o555)).unwrap();
        assert!(!owner_writable(&exe) || rustix::process::geteuid().is_root(), "no write bit, not root");
    }

    #[test]
    fn replace_keeps_a_rollback_copy() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("mandara");
        std::fs::write(&exe, b"\x7fELF version one").unwrap();
        assert!(replace(&exe, b"not an elf").is_err());
        assert_eq!(std::fs::read(&exe).unwrap(), b"\x7fELF version one");
        replace(&exe, b"\x7fELF version two").unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"\x7fELF version two");
        assert_eq!(std::fs::read(backup_path(&exe)).unwrap(), b"\x7fELF version one");
        assert!(!sibling(&exe, STAGED_SUFFIX).exists());
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(std::fs::metadata(&exe).unwrap().permissions().mode() & 0o777, MODE_EXECUTABLE);
        rollback(&exe).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"\x7fELF version one");
        assert!(rollback(&exe).is_err(), "only one rollback step is kept");
    }
}
