//! Everything the installer does to the outside world goes through
//! [`Host`]: running programs, asking questions, downloading and printing.
//! [`SystemHost`] is the real system; tests use a recording fake.

use std::io::{IsTerminal as _, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::Duration;

/// Folders searched for programs besides `$PATH` (sbin is often not on a
/// user's `$PATH`).
const EXTRA_BIN_DIRS: &[&str] = &["/usr/local/sbin", "/usr/local/bin", "/usr/sbin", "/usr/bin", "/sbin", "/bin"];
/// Whole-request timeout for downloads (a release tarball is tens of MB).
const FETCH_TIMEOUT: Duration = Duration::from_secs(600);
/// Download read granularity.
const FETCH_CHUNK: usize = 64 << 10;
/// Any execute bit.
const EXEC_BITS: u32 = 0o111;

/// How a command's output is handled.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Io {
    /// Straight to the terminal (package managers, builds, sudo prompts).
    Inherit,
    /// Captured and returned.
    Capture,
    /// Discarded (stdin stays the terminal so sudo can still ask).
    Quiet,
}

/// A program to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cmd {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: Option<PathBuf>,
    /// Bytes written to its stdin.
    pub stdin: Option<Vec<u8>>,
    pub io: Io,
}

impl Cmd {
    pub fn new<S: AsRef<str>>(argv: &[S]) -> Cmd {
        Cmd { argv: argv.iter().map(|a| a.as_ref().to_string()).collect(), env: Vec::new(), cwd: None, stdin: None, io: Io::Inherit }
    }

    pub fn from_argv(argv: Vec<String>) -> Cmd {
        Cmd { argv, env: Vec::new(), cwd: None, stdin: None, io: Io::Inherit }
    }

    pub fn io(mut self, io: Io) -> Cmd {
        self.io = io;
        self
    }

    pub fn env(mut self, key: &str, value: impl Into<String>) -> Cmd {
        self.env.push((key.to_string(), value.into()));
        self
    }

    pub fn cwd(mut self, dir: impl Into<PathBuf>) -> Cmd {
        self.cwd = Some(dir.into());
        self
    }

    pub fn stdin(mut self, bytes: impl Into<Vec<u8>>) -> Cmd {
        self.stdin = Some(bytes.into());
        self
    }

    /// The command line as shown to the user.
    pub fn display(&self) -> String {
        self.argv.join(" ")
    }
}

/// What a finished command returned.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Output {
    pub success: bool,
    pub code: Option<i32>,
    /// Captured output ([`Io::Capture`] only).
    pub stdout: String,
    pub stderr: String,
}

/// A download that produced no body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Fetch {
    NotFound,
    Failed(String),
}

impl std::fmt::Display for Fetch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Fetch::NotFound => f.write_str("not found (404)"),
            Fetch::Failed(e) => f.write_str(e),
        }
    }
}

/// Kinds of installer output.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Level {
    /// The banner line.
    Title,
    /// `==> Step`.
    Step,
    Info,
    /// Secondary detail (dim).
    Detail,
    Ok,
    Warn,
    /// The closing success line.
    Done,
}

/// The outside world.
pub trait Host {
    /// Runs a program (argv only, never a shell).
    fn run(&mut self, cmd: &Cmd) -> std::io::Result<Output>;
    /// Whether a program is installed.
    fn has_program(&self, name: &str) -> bool;
    /// Asks a yes/no question on the terminal; without one, `default`.
    fn ask(&mut self, question: &str, default: bool) -> bool;
    /// Downloads `url`, at most `limit` bytes.
    fn fetch(&mut self, url: &str, limit: u64) -> Result<Vec<u8>, Fetch>;
    /// Prints one line of installer output.
    fn say(&mut self, level: Level, text: &str);
}

/// The real system.
pub struct SystemHost {
    color: bool,
}

impl SystemHost {
    pub fn new() -> SystemHost {
        SystemHost { color: std::io::stdout().is_terminal() }
    }
}

impl Default for SystemHost {
    fn default() -> Self {
        SystemHost::new()
    }
}

const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const YELLOW: &str = "\x1b[33m";
const CYAN: &str = "\x1b[36m";
const RESET: &str = "\x1b[0m";

/// Whether `name` is an executable file in `$PATH` or a system bin folder.
pub fn find_program(name: &str, path_var: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;
    let exec = |p: &Path| p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & EXEC_BITS != 0);
    if name.contains('/') {
        return exec(Path::new(name)).then(|| PathBuf::from(name));
    }
    path_var.into_iter().flat_map(std::env::split_paths).chain(EXTRA_BIN_DIRS.iter().map(PathBuf::from)).map(|d| d.join(name)).find(|p| exec(p))
}

impl Host for SystemHost {
    fn run(&mut self, cmd: &Cmd) -> std::io::Result<Output> {
        let (prog, args) = cmd.argv.split_first().ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "empty command"))?;
        let mut c = Command::new(prog);
        c.args(args);
        for (k, v) in &cmd.env {
            c.env(k, v);
        }
        if let Some(dir) = &cmd.cwd {
            c.current_dir(dir);
        }
        c.stdin(if cmd.stdin.is_some() { Stdio::piped() } else { Stdio::inherit() });
        match cmd.io {
            Io::Inherit => c.stdout(Stdio::inherit()).stderr(Stdio::inherit()),
            Io::Capture => c.stdout(Stdio::piped()).stderr(Stdio::piped()),
            Io::Quiet => c.stdout(Stdio::null()).stderr(Stdio::null()),
        };
        let mut child = c.spawn()?;
        if let (Some(bytes), Some(mut stdin)) = (&cmd.stdin, child.stdin.take()) {
            stdin.write_all(bytes)?;
        }
        let out = child.wait_with_output()?;
        Ok(Output { success: out.status.success(), code: out.status.code(), stdout: String::from_utf8_lossy(&out.stdout).into_owned(), stderr: String::from_utf8_lossy(&out.stderr).into_owned() })
    }

    fn has_program(&self, name: &str) -> bool {
        find_program(name, std::env::var_os("PATH").as_deref()).is_some()
    }

    fn ask(&mut self, question: &str, default: bool) -> bool {
        // The terminal itself, so it works with stdin redirected too.
        let Ok(mut tty) = std::fs::OpenOptions::new().read(true).write(true).open("/dev/tty") else { return default };
        let hint = if default { "[Y/n]" } else { "[y/N]" };
        if write!(tty, "    {question} {hint} ").and_then(|_| tty.flush()).is_err() {
            return default;
        }
        let mut answer = Vec::new();
        let mut byte = [0u8; 1];
        while tty.read(&mut byte).is_ok_and(|n| n == 1) && byte[0] != b'\n' {
            answer.push(byte[0]);
        }
        match String::from_utf8_lossy(&answer).trim() {
            "" => default,
            a => matches!(a, "y" | "Y" | "yes" | "YES" | "Yes"),
        }
    }

    fn fetch(&mut self, url: &str, limit: u64) -> Result<Vec<u8>, Fetch> {
        if let Some(path) = url.strip_prefix("file://") {
            return std::fs::read(path).map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { Fetch::NotFound } else { Fetch::Failed(format!("{path}: {e}")) });
        }
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(FETCH_TIMEOUT)).http_status_as_error(false).build().into();
        let mut r = agent.get(url).header("User-Agent", concat!("mandara-install/", env!("CARGO_PKG_VERSION"))).call().map_err(|e| Fetch::Failed(format!("{url}: {e}")))?;
        let status = r.status();
        if status == ureq::http::StatusCode::NOT_FOUND {
            return Err(Fetch::NotFound);
        }
        if !status.is_success() {
            return Err(Fetch::Failed(format!("{url}: HTTP {status}")));
        }
        let mut reader = r.body_mut().with_config().limit(limit.saturating_add(1)).reader();
        let mut out = Vec::new();
        let mut buf = vec![0u8; FETCH_CHUNK];
        loop {
            let n = reader.read(&mut buf).map_err(|e| Fetch::Failed(format!("{url}: {e}")))?;
            if n == 0 {
                return Ok(out);
            }
            out.extend_from_slice(&buf[..n]);
            if out.len() as u64 > limit {
                return Err(Fetch::Failed(format!("{url}: larger than expected")));
            }
        }
    }

    fn say(&mut self, level: Level, text: &str) {
        let c = |code: &'static str| if self.color { code } else { "" };
        let (b, d, g, y, cy, n) = (c(BOLD), c(DIM), c(GREEN), c(YELLOW), c(CYAN), c(RESET));
        match level {
            Level::Title => println!("{text}"),
            Level::Step => println!("\n{cy}==>{n} {b}{text}{n}"),
            Level::Info => println!("    {text}"),
            Level::Detail => println!("    {d}{text}{n}"),
            Level::Ok => println!("    {g}✓{n} {text}"),
            Level::Warn => eprintln!("    {y}!{n} {text}"),
            Level::Done => println!("\n{g}{b}{text}{n}"),
        }
    }
}

/// A private temporary folder, removed on drop.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new() -> std::io::Result<TempDir> {
        use std::os::unix::fs::DirBuilderExt as _;
        /// Owner-only: downloads are verified there before use.
        const MODE: u32 = 0o700;
        let base = std::env::temp_dir();
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or_default();
        let dir = base.join(format!("mandara-install.{}.{nanos}", std::process::id()));
        std::fs::DirBuilder::new().mode(MODE).create(&dir)?;
        Ok(TempDir(dir))
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_programs_on_path_only_when_executable() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("mandara-test-tool");
        std::fs::write(&exe, "").unwrap();
        let path = std::ffi::OsString::from(dir.path());
        assert_eq!(find_program("mandara-test-tool", Some(&path)), None);
        std::fs::set_permissions(&exe, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(find_program("mandara-test-tool", Some(&path)), Some(exe.clone()));
        assert_eq!(find_program(exe.to_str().unwrap(), None), Some(exe));
    }

    #[test]
    fn runs_argv_with_stdin_env_and_capture() {
        let mut host = SystemHost { color: false };
        let out = host.run(&Cmd::new(&["cat"]).stdin("i2c-dev\n").io(Io::Capture)).unwrap();
        assert!(out.success);
        assert_eq!(out.stdout, "i2c-dev\n");
        let out = host.run(&Cmd::new(&["env"]).env("MANDARA_INSTALL_TEST", "1").io(Io::Capture)).unwrap();
        assert!(out.stdout.lines().any(|l| l == "MANDARA_INSTALL_TEST=1"));
        assert!(!host.run(&Cmd::new(&["false"]).io(Io::Quiet)).unwrap().success);
        assert!(host.run(&Cmd::new::<&str>(&[])).is_err());
    }

    #[test]
    fn file_urls_read_local_files() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("SHA256SUMS"), "x").unwrap();
        let mut host = SystemHost { color: false };
        assert_eq!(host.fetch(&format!("file://{}/SHA256SUMS", dir.path().display()), 1).unwrap(), b"x");
        assert_eq!(host.fetch(&format!("file://{}/missing", dir.path().display()), 1), Err(Fetch::NotFound));
    }

    #[test]
    fn temp_dirs_are_private_and_removed() {
        use std::os::unix::fs::PermissionsExt as _;
        let t = TempDir::new().unwrap();
        let p = t.path().to_path_buf();
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o700);
        drop(t);
        assert!(!p.exists());
    }
}
