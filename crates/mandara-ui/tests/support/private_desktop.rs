//! Identity and environment checks for native tests in resource-check desktops.

use std::collections::BTreeMap;
use std::io::{self, BufRead as _, BufReader, Read as _, Write as _};
use std::os::fd::AsRawFd as _;
use std::os::unix::fs::{FileTypeExt as _, MetadataExt as _};
use std::os::unix::net::UnixStream;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

use serde_json::{Value, json};

const TIMEOUT: Duration = Duration::from_secs(5);
const REPLY_LIMIT: u64 = 128 * 1024;
const ENVIRONMENT: [&str; 9] = [
    "HOME",
    "XDG_CONFIG_HOME",
    "XDG_DATA_HOME",
    "XDG_CACHE_HOME",
    "XDG_STATE_HOME",
    "XDG_RUNTIME_DIR",
    "WAYLAND_DISPLAY",
    "DBUS_SESSION_BUS_ADDRESS",
    "DBUS_SYSTEM_BUS_ADDRESS",
];

type Environment = BTreeMap<String, String>;

fn require(valid: bool, message: &str) -> io::Result<()> {
    if valid { Ok(()) } else { Err(io::Error::other(message)) }
}

/// Both roots are canonicalized, so TMPDIR symlinks cannot bypass the boundary.
pub fn marked_runtime(runtime: &Path, root: &Path, marker: &str) -> bool {
    let (Ok(runtime), Ok(temporary), Ok(root)) = (runtime.canonicalize(), std::env::temp_dir().canonicalize(), root.canonicalize()) else {
        return false;
    };
    runtime.parent() == Some(temporary.as_path())
        && runtime.file_name().is_some_and(|name| name.to_string_lossy().starts_with("mrc."))
        && runtime
            .metadata()
            .is_ok_and(|metadata| metadata.is_dir() && metadata.uid() == rustix::process::getuid().as_raw() && metadata.mode() & 0o777 == 0o700)
        && std::fs::symlink_metadata(runtime.join(marker)).is_ok_and(|metadata| metadata.is_file() && metadata.uid() == rustix::process::getuid().as_raw())
        && std::fs::read_to_string(runtime.join(marker)).is_ok_and(|contents| contents == root.to_string_lossy())
}

/// Require the paired throwaway HOME and exact private XDG/bus endpoints.
fn isolated_environment(environment: &Environment, runtime: &Path, display: Option<&str>) -> bool {
    let Some(suffix) = runtime.file_name().and_then(|name| name.to_str()).and_then(|name| name.strip_prefix("mrc.")) else {
        return false;
    };
    let Some(parent) = runtime.parent() else {
        return false;
    };
    let home = parent.join(format!("mandara-rc.{suffix}/home"));
    let path_matches =
        |key: &str, expected: &Path| environment.get(key).is_some_and(|value| Path::new(value).canonicalize().is_ok_and(|path| path == expected));
    path_matches("HOME", &home)
        && home.metadata().is_ok_and(|metadata| metadata.is_dir() && metadata.uid() == rustix::process::getuid().as_raw())
        && ["config", "data", "cache", "state"]
            .into_iter()
            .zip(["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_CACHE_HOME", "XDG_STATE_HOME"])
            .all(|(directory, key)| path_matches(key, &home.join(directory)))
        && path_matches("XDG_RUNTIME_DIR", runtime)
        && match display {
            Some(display) => environment.get("WAYLAND_DISPLAY").is_some_and(|value| value == display),
            None => !environment.contains_key("WAYLAND_DISPLAY") && !environment.contains_key("DISPLAY"),
        }
        && environment.get("DBUS_SYSTEM_BUS_ADDRESS").is_some_and(|value| value == "unix:path=/nonexistent")
        && environment.get("DBUS_SESSION_BUS_ADDRESS").is_some_and(|value| value == &format!("unix:path={}/bus", runtime.display()))
}

/// Process names are insufficient: start ticks distinguish a reused PID.
fn start_ticks(pid: u32) -> io::Result<String> {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))?
        .rsplit_once(')')
        .and_then(|(_, fields)| fields.split_whitespace().nth(19))
        .map(str::to_owned)
        .ok_or_else(|| io::Error::other("missing process start identity"))
}

fn process_environment(pid: u32) -> io::Result<Environment> {
    Ok(std::fs::read(format!("/proc/{pid}/environ"))?
        .split(|byte| *byte == 0)
        .filter_map(|entry| std::str::from_utf8(entry).ok()?.split_once('='))
        .filter(|(key, _)| ENVIRONMENT.contains(key) || ["DISPLAY", "WLR_BACKENDS", "WLR_LIBINPUT_NO_DEVICES", "WLR_HEADLESS_OUTPUTS"].contains(key))
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect())
}

/// Bind the recorded PID/start tick to both the executing and saved candidate inode.
fn process_identity(pid: u32, start: &str, binary: &Path, identity: (u64, u64)) -> io::Result<()> {
    let process = PathBuf::from(format!("/proc/{pid}"));
    require(process.metadata()?.uid() == rustix::process::getuid().as_raw() && start_ticks(pid)? == start, "private shell PID identity changed")?;
    let executable = process.join("exe").metadata()?;
    let known = binary.metadata()?;
    require((executable.dev(), executable.ino()) == identity && (known.dev(), known.ino()) == identity, "private shell executable changed")
}

/// Inspect the connected peer before writing any IPC request bytes.
fn peer(socket: &UnixStream, pid: u32, uid: u32) -> io::Result<()> {
    let mut credentials = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut length = size_of_val(&credentials) as libc::socklen_t;
    // SAFETY: the output pointers reference initialized, correctly sized locals.
    let result =
        unsafe { libc::getsockopt(socket.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, (&mut credentials as *mut libc::ucred).cast(), &mut length) };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    require(
        length as usize == size_of_val(&credentials) && u32::try_from(credentials.pid).ok() == Some(pid) && credentials.uid == uid,
        "private shell socket has a foreign peer",
    )
}

struct RecordedProcess {
    binary: PathBuf,
    pid: u32,
    start: String,
    executable: (u64, u64),
}

impl RecordedProcess {
    /// Load the runner's recorded identity for the shell or its headless compositor.
    fn from_env(prefix: &str) -> io::Result<Self> {
        let get = |key: &str| std::env::var(format!("{prefix}_{key}")).map_err(io::Error::other);
        let binary = PathBuf::from(get("BINARY")?).canonicalize()?;
        let metadata = binary.metadata()?;
        Ok(Self { binary, pid: get("PID")?.parse().map_err(io::Error::other)?, start: get("START")?, executable: (metadata.dev(), metadata.ino()) })
    }

    fn check(&self) -> io::Result<()> {
        process_identity(self.pid, &self.start, &self.binary, self.executable)
    }
}

pub struct PrivateDesktop {
    pub runtime: PathBuf,
    pub display: String,
    root: PathBuf,
    marker: &'static str,
    shell: RecordedProcess,
    compositor: RecordedProcess,
}

impl PrivateDesktop {
    /// The runner supplies the recorded shell identity; the Rust test checks it independently.
    pub fn from_env(prefix: &str, root: &Path, marker: &'static str) -> io::Result<Self> {
        let get = |key: &str| std::env::var(format!("{prefix}_{key}")).map_err(io::Error::other);
        let runtime = PathBuf::from(get("RUNTIME")?);
        require(marked_runtime(&runtime, root, marker), "refusing an unmarked desktop")?;
        let desktop = Self {
            runtime: runtime.canonicalize()?,
            display: get("DISPLAY")?,
            root: root.canonicalize()?,
            marker,
            shell: RecordedProcess::from_env(prefix)?,
            compositor: RecordedProcess::from_env(&format!("{prefix}_COMPOSITOR"))?,
        };
        desktop.check()?;
        Ok(desktop)
    }

    /// Revalidate filesystem, process, caller environment and peer before every side effect.
    pub fn check(&self) -> io::Result<()> {
        require(marked_runtime(&self.runtime, &self.root, self.marker), "private desktop marker changed")?;
        let path = Path::new(&self.display);
        require(
            matches!((path.components().next(), path.components().count()), (Some(Component::Normal(_)), 1)),
            "display must be a socket in the private runtime",
        )?;
        require(std::fs::symlink_metadata(self.runtime.join(&self.display))?.file_type().is_socket(), "private display socket missing")?;
        require(std::fs::symlink_metadata(self.runtime.join("bus"))?.file_type().is_socket(), "private session bus missing")?;
        let uid = rustix::process::getuid().as_raw();
        self.shell.check()?;
        self.compositor.check()?;
        let current = ENVIRONMENT.into_iter().filter_map(|key| std::env::var(key).ok().map(|value| (key.into(), value))).collect();
        require(isolated_environment(&current, &self.runtime, Some(&self.display)), "test environment is not the private desktop")?;
        require(
            isolated_environment(&process_environment(self.shell.pid)?, &self.runtime, Some(&self.display)),
            "shell environment is not the private desktop",
        )?;
        let compositor = process_environment(self.compositor.pid)?;
        require(
            isolated_environment(&compositor, &self.runtime, None)
                && compositor.get("WLR_BACKENDS").is_some_and(|value| value == "headless")
                && compositor.get("WLR_LIBINPUT_NO_DEVICES").is_some_and(|value| value == "1")
                && compositor.get("WLR_HEADLESS_OUTPUTS").is_some_and(|value| value == "1"),
            "compositor is not the private headless desktop",
        )?;
        peer(&UnixStream::connect(self.runtime.join(&self.display))?, self.compositor.pid, uid)?;
        peer(&UnixStream::connect(self.runtime.join("mandara.sock"))?, self.shell.pid, uid)
    }

    /// A fresh peer check is applied to the exact socket receiving each request.
    pub fn request(&self, args: &[&str]) -> io::Result<Value> {
        self.check()?;
        let mut socket = UnixStream::connect(self.runtime.join("mandara.sock"))?;
        peer(&socket, self.shell.pid, rustix::process::getuid().as_raw())?;
        socket.set_read_timeout(Some(TIMEOUT))?;
        socket.set_write_timeout(Some(TIMEOUT))?;
        writeln!(socket, "{}", json!({"args": args}))?;
        let mut line = String::new();
        BufReader::new(socket).take(REPLY_LIMIT).read_line(&mut line)?;
        require(line.ends_with('\n'), "truncated private shell reply")?;
        let reply: Value = serde_json::from_str(&line)?;
        require(reply["ok"] == true, "private shell request failed")?;
        Ok(reply["data"].clone())
    }

    /// Tools inherit no owner HOME, bus, display or desktop variables.
    pub fn tool(&self, name: &str) -> io::Result<Command> {
        self.check()?;
        let environment = process_environment(self.shell.pid)?;
        let mut command = Command::new(name);
        command.env_clear().env("PATH", "/usr/local/bin:/usr/bin:/bin").stdin(Stdio::null());
        for key in ENVIRONMENT {
            command.env(key, &environment[key]);
        }
        Ok(command)
    }

    /// Evidence writes stay inside the same currently verified runtime.
    pub fn artifact(&self, name: &str) -> io::Result<PathBuf> {
        self.check()?;
        require(
            matches!((Path::new(name).components().next(), Path::new(name).components().count()), (Some(Component::Normal(_)), 1)),
            "artifact must be one private filename",
        )?;
        Ok(self.runtime.join(name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    /// Missing/foreign markers, permissive runtimes and path escapes cannot authorize input.
    #[test]
    fn marker_and_runtime_permissions_are_required() {
        let root = tempfile::tempdir().unwrap();
        let runtime = tempfile::Builder::new().prefix("mrc.").tempdir().unwrap();
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let marker = ".mandara-test";
        assert!(!marked_runtime(runtime.path(), root.path(), marker));
        std::fs::write(runtime.path().join(marker), "foreign checkout").unwrap();
        assert!(!marked_runtime(runtime.path(), root.path(), marker));
        std::fs::write(runtime.path().join(marker), root.path().canonicalize().unwrap().to_string_lossy().as_bytes()).unwrap();
        assert!(marked_runtime(runtime.path(), root.path(), marker));
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!marked_runtime(runtime.path(), root.path(), marker));
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let foreign = tempfile::tempdir().unwrap();
        let escaped = foreign.path().join("mrc.escape");
        std::fs::create_dir(&escaped).unwrap();
        std::fs::set_permissions(&escaped, std::fs::Permissions::from_mode(0o700)).unwrap();
        std::fs::write(escaped.join(marker), root.path().canonicalize().unwrap().to_string_lossy().as_bytes()).unwrap();
        let link = runtime.path().join("mrc.escape");
        std::os::unix::fs::symlink(&escaped, &link).unwrap();
        assert!(!marked_runtime(&link, root.path(), marker));
    }

    /// Test and shell environments must use the paired HOME, XDG roots and dead/private buses.
    #[test]
    fn environment_rejects_foreign_home_and_live_buses() {
        let runtime = tempfile::Builder::new().prefix("mrc.").tempdir().unwrap();
        let suffix = runtime.path().file_name().unwrap().to_str().unwrap().strip_prefix("mrc.").unwrap();
        let work = tempfile::Builder::new().prefix(&format!("mandara-rc.{suffix}")).rand_bytes(0).tempdir().unwrap();
        let home = work.path().canonicalize().unwrap().join("home");
        let runtime_path = runtime.path().canonicalize().unwrap();
        let mut environment = Environment::new();
        for (key, directory) in
            [("HOME", ""), ("XDG_CONFIG_HOME", "config"), ("XDG_DATA_HOME", "data"), ("XDG_CACHE_HOME", "cache"), ("XDG_STATE_HOME", "state")]
        {
            let path = home.join(directory);
            std::fs::create_dir_all(&path).unwrap();
            environment.insert(key.into(), path.to_string_lossy().into_owned());
        }
        environment.insert("XDG_RUNTIME_DIR".into(), runtime_path.to_string_lossy().into_owned());
        environment.insert("WAYLAND_DISPLAY".into(), "wayland-0".into());
        environment.insert("DBUS_SESSION_BUS_ADDRESS".into(), format!("unix:path={}/bus", runtime_path.display()));
        environment.insert("DBUS_SYSTEM_BUS_ADDRESS".into(), "unix:path=/nonexistent".into());
        assert!(isolated_environment(&environment, &runtime_path, Some("wayland-0")));
        assert!(!isolated_environment(&environment, &runtime_path, None));
        let mut server = environment.clone();
        server.remove("WAYLAND_DISPLAY");
        assert!(isolated_environment(&server, &runtime_path, None));
        server.insert("DISPLAY".into(), ":0".into());
        assert!(!isolated_environment(&server, &runtime_path, None));
        for (key, value) in [
            ("HOME", "/".into()),
            ("XDG_CONFIG_HOME", "/tmp".into()),
            ("DBUS_SYSTEM_BUS_ADDRESS", "unix:path=/run/dbus/system_bus_socket".into()),
            ("DBUS_SESSION_BUS_ADDRESS", "unix:path=/run/user/1000/bus".into()),
            ("WAYLAND_DISPLAY", "wayland-owner".into()),
        ] {
            let mut foreign = environment.clone();
            foreign.insert(key.into(), value);
            assert!(!isolated_environment(&foreign, &runtime_path, Some("wayland-0")), "{key}");
        }
    }

    /// A valid display peer cannot authorize a different recorded compositor or start tick.
    #[test]
    fn display_peer_must_match_the_recorded_compositor() {
        let (display, _) = UnixStream::pair().unwrap();
        let binary = std::env::current_exe().unwrap();
        let metadata = binary.metadata().unwrap();
        let mut compositor =
            RecordedProcess { binary, pid: std::process::id(), start: start_ticks(std::process::id()).unwrap(), executable: (metadata.dev(), metadata.ino()) };
        assert!(compositor.check().is_ok());
        assert!(peer(&display, compositor.pid, rustix::process::getuid().as_raw()).is_ok());
        compositor.start.push('0');
        assert!(compositor.check().is_err());
        compositor.pid = u32::MAX;
        assert!(compositor.check().is_err());
        assert!(peer(&display, compositor.pid, rustix::process::getuid().as_raw()).is_err());
    }

    /// Socket credentials are checked before request bytes, independently of a path marker.
    #[test]
    fn connected_peer_rejects_a_foreign_pid_or_uid() {
        let (socket, _) = UnixStream::pair().unwrap();
        let uid = rustix::process::getuid().as_raw();
        assert!(peer(&socket, std::process::id(), uid).is_ok());
        assert!(peer(&socket, std::process::id().wrapping_add(1), uid).is_err());
        assert!(peer(&socket, std::process::id(), uid.wrapping_add(1)).is_err());
        let pid = std::process::id();
        let start = start_ticks(pid).unwrap();
        let binary = std::env::current_exe().unwrap();
        let metadata = binary.metadata().unwrap();
        let identity = (metadata.dev(), metadata.ino());
        assert!(process_identity(pid, &start, &binary, identity).is_ok());
        assert!(process_identity(pid, "foreign start identity", &binary, identity).is_err());
        assert!(process_identity(pid, &start, &binary, (0, 0)).is_err());
        let foreign = tempfile::NamedTempFile::new().unwrap();
        assert!(process_identity(pid, &start, foreign.path(), identity).is_err());
    }
}
