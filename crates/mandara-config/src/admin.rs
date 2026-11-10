//! The binary Mandara runs as root for administrator actions (the login
//! screen, keyring unlock at login): never your own copy in
//! `~/.local/bin`, which your user account can change, so malware there
//! would get root the next time an administrator approves a prompt. Only a
//! copy that root owns and nobody else can change runs as root: a package's
//! `mandara`, the admin helper (`mandara install` puts it in place, or
//! Studio once, with your approval), or the login screen's own copy.

use std::ffi::OsString;
use std::os::unix::fs::MetadataExt as _;
use std::path::{Path, PathBuf};

/// The admin helper: a root-owned copy of Mandara.
pub const ADMIN_EXE: &str = "/usr/local/lib/mandara/mandara-admin";
/// The login screen's root-owned copy (`mandara greeter use mandara`).
pub const GREETER_EXE: &str = "/usr/local/lib/mandara/mandara-greeter";
/// Group / other write bits.
const SHARED_WRITE: u32 = 0o022;
/// The sticky bit: in such a folder only an entry's owner (or root) can
/// rename or delete it.
const STICKY: u32 = 0o1000;

/// Whether `path` and every folder above it are owned by root and writable
/// by root alone, so no other account can replace what runs as root. A
/// root-owned sticky folder that others may add to also counts (the Nix
/// store, `drwxrwxr-t root nixbld`): nobody else can replace the
/// root-owned entries in it.
pub fn trusted_root_file(path: &Path) -> bool {
    trusted_with(path, |p| std::fs::metadata(p).ok().map(|m| (m.uid(), m.mode())))
}

fn trusted_with(path: &Path, meta: impl Fn(&Path) -> Option<(u32, u32)>) -> bool {
    if !path.is_absolute() {
        return false;
    }
    path.ancestors().enumerate().all(|(depth, p)| meta(p).is_some_and(|(uid, mode)| uid == 0 && (mode & SHARED_WRITE == 0 || (depth > 0 && mode & STICKY != 0))))
}

/// The binary to run as root: the running one when it's already root-owned
/// (a package install), else the admin helper, else the login screen's
/// copy. `None` until one exists ([`setup_command`] installs the helper).
pub fn admin_exe() -> Option<PathBuf> {
    let running = std::env::current_exe().ok();
    pick(running.as_deref(), trusted_root_file)
}

fn pick(running: Option<&Path>, trusted: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    running.map(Path::to_path_buf).into_iter().chain([PathBuf::from(ADMIN_EXE), PathBuf::from(GREETER_EXE)]).find(|p| trusted(p))
}

/// `pkexec install -D -o root -g root -m 0755 <from> ADMIN_EXE`: copies
/// `from` (the running binary) into the root-owned helper. An administrator
/// approves this once; after that, changing the copy in your home can't
/// reach root.
#[cfg(not(target_os = "freebsd"))]
pub fn setup_command(from: &Path) -> (OsString, Vec<OsString>) {
    let args = ["install", "-D", "-o", "root", "-g", "root", "-m", "0755"].into_iter().map(OsString::from).chain([from.as_os_str().to_owned(), OsString::from(ADMIN_EXE)]).collect();
    ("pkexec".into(), args)
}

/// FreeBSD: [`freebsd_setup_command`].
#[cfg(target_os = "freebsd")]
pub fn setup_command(from: &Path) -> (OsString, Vec<OsString>) {
    freebsd_setup_command(from)
}

/// The FreeBSD script behind [`freebsd_setup_command`]: a constant, with
/// the source path passed as `$1` (never pasted into it).
const FREEBSD_SETUP_SCRIPT: &str = "install -d -o root -g wheel -m 0755 \"${0%/*}\" && install -o root -g wheel -m 0755 \"$1\" \"$0\"";

/// FreeBSD's `install` can't create the folder on the way (no GNU `-D`)
/// and root's group is `wheel`, so one approval runs both steps through
/// `/bin/sh` with a fixed script: `$0` is [`ADMIN_EXE`], `$1` the running
/// binary.
pub fn freebsd_setup_command(from: &Path) -> (OsString, Vec<OsString>) {
    let args = ["/bin/sh", "-c", FREEBSD_SETUP_SCRIPT, ADMIN_EXE].into_iter().map(OsString::from).chain([from.as_os_str().to_owned()]).collect();
    ("pkexec".into(), args)
}

/// argv running `args` with the admin binary through pkexec.
pub fn pkexec(admin: &Path, args: impl IntoIterator<Item = OsString>) -> (OsString, Vec<OsString>) {
    ("pkexec".into(), std::iter::once(admin.as_os_str().to_owned()).chain(args).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Issue #10: only a root-owned binary, in root-owned folders, runs as
    /// root.
    #[test]
    fn only_root_owned_binaries_in_root_owned_folders_are_trusted() {
        let fs: HashMap<&str, (u32, u32)> = HashMap::from([
            ("/", (0, 0o755)),
            ("/usr", (0, 0o755)),
            ("/usr/local", (0, 0o755)),
            ("/usr/local/lib", (0, 0o755)),
            ("/usr/local/lib/mandara", (0, 0o755)),
            ("/usr/local/lib/mandara/mandara-admin", (0, 0o755)),
            ("/home", (0, 0o755)),
            ("/home/u", (1000, 0o700)),
            ("/home/u/.local/bin/mandara", (1000, 0o755)),
            ("/opt/shared", (0, 0o777)),
            ("/opt", (0, 0o755)),
            ("/opt/shared/mandara", (0, 0o755)),
            ("/usr/local/lib/mandara/group-writable", (0, 0o775)),
        ]);
        let meta = |p: &Path| fs.get(p.to_str().unwrap()).copied();
        assert!(trusted_with(Path::new("/usr/local/lib/mandara/mandara-admin"), meta));
        assert!(!trusted_with(Path::new("/home/u/.local/bin/mandara"), meta), "your own copy");
        assert!(!trusted_with(Path::new("/opt/shared/mandara"), meta), "a root file in a folder anyone can write");
        assert!(!trusted_with(Path::new("/usr/local/lib/mandara/group-writable"), meta), "group-writable");
        assert!(!trusted_with(Path::new("/usr/local/lib/mandara/missing"), meta));
        assert!(!trusted_with(Path::new("relative/mandara"), meta));
    }

    /// Issue #39: NixOS's `mandara` lives in the Nix store, a root-owned
    /// sticky folder the build users may add to; a single-user Nix store
    /// (owned by you) is not trusted.
    #[test]
    fn the_nix_store_is_trusted_when_root_owns_it() {
        let store = "/nix/store/0c5mr2ljm3b4kx4ij2yr6dqyqv5p6fla-mandara-0.10.1";
        let exe = format!("{store}/bin/mandara");
        let bin = format!("{store}/bin");
        let mut fs: HashMap<&str, (u32, u32)> = HashMap::from([("/", (0, 0o755)), ("/nix", (0, 0o755)), ("/nix/store", (0, 0o1775)), (store, (0, 0o555)), (bin.as_str(), (0, 0o555)), (exe.as_str(), (0, 0o555))]);
        assert!(trusted_with(Path::new(&exe), |p: &Path| fs.get(p.to_str().unwrap()).copied()));
        fs.insert("/nix/store", (0, 0o775));
        assert!(!trusted_with(Path::new(&exe), |p: &Path| fs.get(p.to_str().unwrap()).copied()), "group-writable without the sticky bit");
        fs.insert("/nix/store", (1000, 0o1775));
        assert!(!trusted_with(Path::new(&exe), |p: &Path| fs.get(p.to_str().unwrap()).copied()), "a store you own");
        fs.insert("/nix/store", (0, 0o1775));
        fs.insert(exe.as_str(), (0, 0o1777));
        assert!(!trusted_with(Path::new(&exe), |p: &Path| fs.get(p.to_str().unwrap()).copied()), "the sticky bit never excuses the file itself");
    }

    /// On the real filesystem: a system folder is trusted, and the test
    /// binary itself (built by your user) isn't.
    #[test]
    fn real_files() {
        assert!(trusted_root_file(Path::new("/usr")));
        let me = std::env::current_exe().unwrap();
        if process_uid() != 0 {
            assert!(!trusted_root_file(&me), "{} is yours, not root's", me.display());
        }
    }

    /// The test process's user id (from /proc, no extra crate).
    fn process_uid() -> u32 {
        std::fs::metadata("/proc/self").map(|m| m.uid()).unwrap_or(0)
    }

    /// FreeBSD: one approval makes the folder and copies the binary, with
    /// both paths as arguments of a constant script.
    #[test]
    fn freebsd_setup_is_one_fixed_script() {
        let from = Path::new("/home/u/my $(evil)/mandara");
        let (prog, args) = freebsd_setup_command(from);
        assert_eq!(prog, "pkexec");
        assert_eq!(args[..3], ["/bin/sh", "-c", FREEBSD_SETUP_SCRIPT].map(OsString::from));
        assert_eq!(args[3], ADMIN_EXE, "$0: where the helper goes");
        assert_eq!(args[4], from.as_os_str(), "$1: the binary, passed as is");
        assert_eq!(args.len(), 5);
        // The script itself never names a caller-chosen path.
        assert!(!FREEBSD_SETUP_SCRIPT.contains("home"));
        // Its folder is the helper's own.
        if let Ok(o) = std::process::Command::new("/bin/sh").args(["-c", "printf %s \"${0%/*}\"", ADMIN_EXE]).output() {
            assert_eq!(String::from_utf8_lossy(&o.stdout), "/usr/local/lib/mandara");
        }
    }

    #[test]
    fn the_running_binary_is_used_only_when_root_owns_it() {
        let trusted = |p: &Path| p == Path::new(ADMIN_EXE) || p == Path::new("/usr/bin/mandara");
        assert_eq!(pick(Some(Path::new("/usr/bin/mandara")), trusted), Some(PathBuf::from("/usr/bin/mandara")));
        assert_eq!(pick(Some(Path::new("/home/u/.local/bin/mandara")), trusted), Some(PathBuf::from(ADMIN_EXE)));
        assert_eq!(pick(Some(Path::new("/home/u/.local/bin/mandara")), |_| false), None);
        let (prog, args) = setup_command(Path::new("/home/u/.local/bin/mandara"));
        assert_eq!(prog, "pkexec");
        if !cfg!(target_os = "freebsd") {
            assert_eq!(args.join(std::ffi::OsStr::new(" ")), "install -D -o root -g root -m 0755 /home/u/.local/bin/mandara /usr/local/lib/mandara/mandara-admin");
        }
        let (_, run) = pkexec(Path::new(ADMIN_EXE), ["greeter".into(), "list".into()]);
        assert_eq!(run, [ADMIN_EXE, "greeter", "list"].map(OsString::from));
    }
}
