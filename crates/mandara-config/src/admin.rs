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
/// Read size when comparing two copies of Mandara.
const COMPARE_CHUNK: usize = 64 * 1024;

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
/// copy. `None` until one exists ([`setup_and_run`] installs the helper).
pub fn admin_exe() -> Option<PathBuf> {
    let running = std::env::current_exe().ok();
    pick(running.as_deref(), trusted_root_file)
}

fn pick(running: Option<&Path>, trusted: impl Fn(&Path) -> bool) -> Option<PathBuf> {
    running.map(Path::to_path_buf).into_iter().chain([PathBuf::from(ADMIN_EXE), PathBuf::from(GREETER_EXE)]).find(|p| trusted(p))
}

/// [`admin_exe`] only when it is this very build. An older copy (the
/// helper from an earlier version, or the login screen's copy) may not
/// read what this one writes, such as a login screen config with a newer
/// element, so it gives `None` and [`setup_and_run`] replaces the helper.
/// Reads both binaries: call off the UI thread.
pub fn current_admin_exe() -> Option<PathBuf> {
    let running = std::env::current_exe().ok()?;
    admin_exe().filter(|exe| *exe == running || same_contents(exe, &running))
}

/// Byte-for-byte equal files (false when either can't be read).
pub fn same_contents(a: &Path, b: &Path) -> bool {
    use std::io::Read as _;
    let (Ok(fa), Ok(fb)) = (std::fs::File::open(a), std::fs::File::open(b)) else { return false };
    if fa.metadata().map(|m| m.len()).ok() != fb.metadata().map(|m| m.len()).ok() {
        return false;
    }
    let (mut ra, mut rb) = (std::io::BufReader::new(fa), std::io::BufReader::new(fb));
    let (mut ba, mut bb) = ([0u8; COMPARE_CHUNK], [0u8; COMPARE_CHUNK]);
    loop {
        let (Ok(na), Ok(nb)) = (ra.read(&mut ba), rb.read(&mut bb)) else { return false };
        if na != nb || ba[..na] != bb[..nb] {
            return false;
        }
        if na == 0 {
            return true;
        }
    }
}

/// Copies `$1` (the running binary) into `$0` ([`ADMIN_EXE`]), root-owned.
#[cfg(not(target_os = "freebsd"))]
macro_rules! install_script {
    () => {
        "install -D -o root -g root -m 0755 \"$1\" \"$0\""
    };
}
/// FreeBSD's `install` can't create the folder on the way (no GNU `-D`)
/// and root's group is `wheel`.
#[cfg(target_os = "freebsd")]
macro_rules! install_script {
    () => {
        "install -d -o root -g wheel -m 0755 \"${0%/*}\" && install -o root -g wheel -m 0755 \"$1\" \"$0\""
    };
}

/// The script behind [`setup_and_run`]: a constant, with every path and
/// argument passed as `$0`, `$1`, `$@` (never pasted into it). It installs
/// the helper, then becomes it.
const SETUP_AND_RUN_SCRIPT: &str = concat!(install_script!(), " && shift && exec \"$0\" \"$@\"");

/// One approval that puts `from` (the running binary) in place as the
/// admin helper and runs `args` with it: `pkexec /bin/sh -c <script>
/// ADMIN_EXE <from> <args…>`. Used when no helper exists yet or the one
/// there is an older version, so refreshing it never costs a second
/// password prompt.
pub fn setup_and_run(from: &Path, args: impl IntoIterator<Item = OsString>) -> (OsString, Vec<OsString>) {
    let argv = ["/bin/sh", "-c", SETUP_AND_RUN_SCRIPT, ADMIN_EXE].into_iter().map(OsString::from).chain([from.as_os_str().to_owned()]).chain(args).collect();
    ("pkexec".into(), argv)
}

/// argv running `args` with the admin binary through pkexec.
pub fn pkexec(admin: &Path, args: impl IntoIterator<Item = OsString>) -> (OsString, Vec<OsString>) {
    ("pkexec".into(), std::iter::once(admin.as_os_str().to_owned()).chain(args).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// An older copy of Mandara is told apart from this build.
    #[test]
    fn same_contents_compares_every_byte() {
        let dir = tempfile::tempdir().unwrap();
        let file = |name: &str, body: &[u8]| {
            let p = dir.path().join(name);
            std::fs::write(&p, body).unwrap();
            p
        };
        let long = vec![7u8; COMPARE_CHUNK * 2 + 3];
        let mut late = long.clone();
        if let Some(b) = late.last_mut() {
            *b = 8;
        }
        let (a, b, c, d) = (file("a", &long), file("b", &long), file("c", &late), file("d", b"short"));
        assert!(same_contents(&a, &b));
        assert!(!same_contents(&a, &c), "a difference past the first chunk counts");
        assert!(!same_contents(&a, &d), "another size");
        assert!(!same_contents(&a, &dir.path().join("missing")));
    }

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

    /// One approval installs the helper and runs the command with it, with
    /// every path and argument passed to a constant script.
    #[test]
    fn setup_and_run_is_one_fixed_script() {
        let from = Path::new("/home/u/my $(evil)/mandara");
        let (prog, args) = setup_and_run(from, ["greeter".into(), "apply-config".into(), "--from".into(), "/home/u/a b.toml".into()]);
        assert_eq!(prog, "pkexec");
        assert_eq!(args[..3], ["/bin/sh", "-c", SETUP_AND_RUN_SCRIPT].map(OsString::from));
        assert_eq!(args[3], ADMIN_EXE, "$0: where the helper goes");
        assert_eq!(args[4], from.as_os_str(), "$1: the binary, passed as is");
        assert_eq!(args[5..], ["greeter", "apply-config", "--from", "/home/u/a b.toml"].map(OsString::from));
        // The script itself never names a caller-chosen path.
        assert!(!SETUP_AND_RUN_SCRIPT.contains("home"));
        // Its folder is the helper's own.
        if let Ok(o) = std::process::Command::new("/bin/sh").args(["-c", "printf %s \"${0%/*}\"", ADMIN_EXE]).output() {
            assert_eq!(String::from_utf8_lossy(&o.stdout), "/usr/local/lib/mandara");
        }
    }

    /// The script, run unprivileged with `install` swapped for a stub:
    /// it copies `$1` to `$0`, then runs `$0` with exactly the remaining
    /// arguments (spaces and `$(…)` untouched), and stops if the copy fails.
    #[test]
    fn setup_and_run_installs_then_becomes_the_helper() {
        let dir = tempfile::tempdir().unwrap();
        let stub = dir.path().join("bin");
        std::fs::create_dir(&stub).unwrap();
        let fake_install = stub.join("install");
        // The last two arguments are the source and the destination.
        let script = "#!/bin/sh\n[ -n \"$FAIL\" ] && exit 1\nfor a; do src=$dst; dst=$a; done\nif [ \"$1\" = -d ]; then mkdir -p \"$dst\"; exit; fi\nmkdir -p \"${dst%/*}\" && cp \"$src\" \"$dst\" && chmod 755 \"$dst\"\n";
        std::fs::write(&fake_install, script).unwrap();
        let from = dir.path().join("mandara");
        std::fs::write(&from, "#!/bin/sh\nprintf '%s|' \"$@\"\n").unwrap();
        for p in [&fake_install, &from] {
            std::fs::set_permissions(p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        }
        let helper = dir.path().join("lib/mandara-admin");
        let path = format!("{}:{}", stub.display(), std::env::var("PATH").unwrap_or_default());
        let run = |fail: bool| {
            let mut cmd = std::process::Command::new("/bin/sh");
            cmd.args(["-c", SETUP_AND_RUN_SCRIPT]).arg(&helper).arg(&from).args(["greeter", "a b", "$(id)"]).env("PATH", &path);
            if fail {
                cmd.env("FAIL", "1");
            }
            cmd.output().unwrap()
        };
        let failed = run(true);
        assert!(!failed.status.success() && !helper.exists(), "nothing runs when the copy fails");
        let out = run(false);
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        assert_eq!(String::from_utf8_lossy(&out.stdout), "greeter|a b|$(id)|");
        assert!(same_contents(&from, &helper));
    }

    #[test]
    fn the_running_binary_is_used_only_when_root_owns_it() {
        let trusted = |p: &Path| p == Path::new(ADMIN_EXE) || p == Path::new("/usr/bin/mandara");
        assert_eq!(pick(Some(Path::new("/usr/bin/mandara")), trusted), Some(PathBuf::from("/usr/bin/mandara")));
        assert_eq!(pick(Some(Path::new("/home/u/.local/bin/mandara")), trusted), Some(PathBuf::from(ADMIN_EXE)));
        assert_eq!(pick(Some(Path::new("/home/u/.local/bin/mandara")), |_| false), None);
        let (_, run) = pkexec(Path::new(ADMIN_EXE), ["greeter".into(), "list".into()]);
        assert_eq!(run, [ADMIN_EXE, "greeter", "list"].map(OsString::from));
    }
}
