//! Crash reports for the `crashes` plugin, from systemd-coredump:
//! `coredumpctl --json=short --no-pager list --since=-7d` (a JSON array of
//! `{time, pid, uid, gid, sig, corefile, exe, size}`, `time` in
//! microseconds) and `coredumpctl info <PID>` for a full report.
//!
//! coredumpctl prints "No coredumps found." on stderr and exits 1 when the
//! window is empty; that is an empty list, not an error. Whether
//! systemd-coredump collects crashes at all is read from the kernel's
//! `core_pattern`. The "seen up to" mark lives in
//! `~/.local/state/mandara/crashes.json`. Blocking: call it off the UI
//! thread.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};

/// How far back the list reaches.
pub const SINCE: &str = "-7d";
/// Microseconds per second (`time` is in µs).
pub const MICROS_PER_SEC: u64 = 1_000_000;
/// The kernel's core handler setting.
const CORE_PATTERN: &str = "/proc/sys/kernel/core_pattern";
const STATE_FILE: &str = "crashes.json";

/// One crash.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Crash {
    /// Microseconds since the epoch.
    pub time: u64,
    pub pid: u32,
    #[serde(default)]
    pub uid: u32,
    /// The signal number (11 = SIGSEGV, 6 = SIGABRT, …).
    pub sig: i32,
    /// `present`, `missing`, `none`, `truncated`, `inaccessible`, …
    #[serde(default)]
    pub corefile: String,
    #[serde(default)]
    pub exe: String,
    /// Core size in bytes when one is stored.
    #[serde(default)]
    pub size: Option<u64>,
}

impl Crash {
    /// The executable's file name.
    pub fn name(&self) -> &str {
        Path::new(&self.exe).file_name().and_then(|n| n.to_str()).filter(|n| !n.is_empty()).unwrap_or(&self.exe)
    }

    /// Unix seconds.
    pub fn unix(&self) -> i64 {
        i64::try_from(self.time / MICROS_PER_SEC).unwrap_or(i64::MAX)
    }

    /// A core file is stored (so it can be debugged).
    pub fn has_core(&self) -> bool {
        self.corefile == "present"
    }

    pub fn signal(&self) -> String {
        signal_name(self.sig)
    }
}

/// `SIGSEGV`, `SIGABRT`, … for a signal number (`SIG42` when unknown).
pub fn signal_name(sig: i32) -> String {
    const NAMES: [&str; 31] = [
        "HUP", "INT", "QUIT", "ILL", "TRAP", "ABRT", "BUS", "FPE", "KILL", "USR1", "SEGV", "USR2", "PIPE", "ALRM", "TERM", "STKFLT", "CHLD", "CONT", "STOP", "TSTP", "TTIN",
        "TTOU", "URG", "XCPU", "XFSZ", "VTALRM", "PROF", "WINCH", "IO", "PWR", "SYS",
    ];
    match usize::try_from(sig).ok().and_then(|s| s.checked_sub(1)).and_then(|i| NAMES.get(i)) {
        Some(name) => format!("SIG{name}"),
        None => format!("SIG{sig}"),
    }
}

/// Parses coredumpctl's JSON list, newest first.
pub fn parse_list(json: &str) -> Result<Vec<Crash>, String> {
    let json = json.trim();
    if json.is_empty() {
        return Ok(Vec::new());
    }
    let mut crashes: Vec<Crash> = serde_json::from_str(json).map_err(|e| tf("coredumpctl printed something unexpected: {error}", &[("error", &e)]))?;
    crashes.sort_by_key(|c| std::cmp::Reverse(c.time));
    Ok(crashes)
}

/// What the list call found.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum Listing {
    Crashes { crashes: Vec<Crash> },
    /// coredumpctl isn't installed.
    Missing,
    Failed { error: String },
}

/// Whether the kernel hands crashes to systemd-coredump.
pub fn collecting() -> bool {
    std::fs::read_to_string(CORE_PATTERN).is_ok_and(|p| p.contains("systemd-coredump"))
}

fn coredumpctl(args: &[&str]) -> Result<std::process::Output, std::io::Error> {
    Command::new("coredumpctl").args(args).env("LC_ALL", "C").env("SYSTEMD_PAGER", "").stdin(Stdio::null()).output()
}

/// Crashes of the last [`SINCE`].
pub fn list() -> Listing {
    let since = format!("--since={SINCE}");
    match coredumpctl(&["--json=short", "--no-pager", "list", &since]) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Listing::Missing,
        Err(e) => Listing::Failed { error: e.to_string() },
        Ok(out) => {
            let stderr = String::from_utf8_lossy(&out.stderr);
            if !out.status.success() && stderr.contains("No coredumps found") {
                return Listing::Crashes { crashes: Vec::new() };
            }
            if !out.status.success() {
                let line = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim().to_string();
                return Listing::Failed { error: if line.is_empty() { t("coredumpctl failed").into() } else { line } };
            }
            match parse_list(&String::from_utf8_lossy(&out.stdout)) {
                Ok(crashes) => Listing::Crashes { crashes },
                Err(error) => Listing::Failed { error },
            }
        }
    }
}

/// `coredumpctl info <pid>`: the full text report (with a stack trace when
/// the core is still stored).
pub fn info(pid: u32) -> Result<String, String> {
    let pid = pid.to_string();
    let out = coredumpctl(&["info", "--no-pager", &pid]).map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { t("coredumpctl isn't installed").to_string() } else { e.to_string() })?;
    let text = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
    if out.status.success() && !text.is_empty() {
        Ok(text)
    } else {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        Err(if err.is_empty() { tf("No crash report for PID {pid}", &[("pid", &pid)]) } else { err })
    }
}

/// The command that opens a debugger on a crash's core.
pub fn debug_command(pid: u32) -> String {
    format!("coredumpctl debug {pid}")
}

#[derive(Default, Serialize, Deserialize)]
struct SeenFile {
    /// Crashes at or before this time (µs) count as seen.
    #[serde(default)]
    seen_up_to: u64,
}

fn state_path() -> PathBuf {
    mandara_config::paths::state_dir().join(STATE_FILE)
}

fn load_seen_from(path: &Path) -> u64 {
    std::fs::read_to_string(path).ok().and_then(|s| serde_json::from_str::<SeenFile>(&s).ok()).map_or(0, |f| f.seen_up_to)
}

fn save_seen_to(path: &Path, up_to: u64) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    let body = serde_json::to_string(&SeenFile { seen_up_to: up_to }).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, body).and_then(|()| std::fs::rename(&tmp, path)).map_err(|e| e.to_string())
}

/// The "seen up to" mark (µs; 0 when nothing was marked).
pub fn load_seen() -> u64 {
    load_seen_from(&state_path())
}

/// Saves the "seen up to" mark.
pub fn save_seen(up_to: u64) -> Result<(), String> {
    save_seen_to(&state_path(), up_to)
}

/// Crashes newer than the mark.
pub fn unseen(crashes: &[Crash], seen_up_to: u64) -> usize {
    crashes.iter().filter(|c| c.time > seen_up_to).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real `coredumpctl --json=short list` output (systemd 262).
    const SAMPLE: &str = r#"[{"time":1793259987887410,"pid":2966,"uid":1000,"gid":1000,"sig":6,"corefile":"missing","exe":"/usr/bin/easyeffects","size":null},{"time":1793339351397520,"pid":3914091,"uid":1000,"gid":1000,"sig":6,"corefile":"present","exe":"/usr/lib/at-spi-bus-launcher","size":101088},{"time":1793340708397327,"pid":4171954,"uid":1000,"gid":1000,"sig":11,"corefile":"none","exe":"/home/u/.cache/app/target/debug/app","size":null}]
"#;

    #[test]
    fn parses_coredumpctl_json_newest_first() {
        let c = parse_list(SAMPLE).unwrap();
        assert_eq!(c.iter().map(|c| c.pid).collect::<Vec<_>>(), [4171954, 3914091, 2966]);
        assert_eq!((c[0].name(), c[0].signal(), c[0].has_core()), ("app", "SIGSEGV".to_string(), false));
        assert_eq!((c[1].name(), c[1].size, c[1].has_core()), ("at-spi-bus-launcher", Some(101088), true));
        assert_eq!(c[2].unix(), 1_793_259_987);
        assert!(parse_list("").unwrap().is_empty());
        assert!(parse_list("No coredumps found.").is_err());
    }

    #[test]
    fn names_signals() {
        assert_eq!(signal_name(11), "SIGSEGV");
        assert_eq!(signal_name(6), "SIGABRT");
        assert_eq!(signal_name(7), "SIGBUS");
        assert_eq!(signal_name(8), "SIGFPE");
        assert_eq!(signal_name(4), "SIGILL");
        assert_eq!(signal_name(5), "SIGTRAP");
        assert_eq!(signal_name(31), "SIGSYS");
        assert_eq!(signal_name(0), "SIG0");
        assert_eq!(signal_name(64), "SIG64");
        assert_eq!(signal_name(-3), "SIG-3");
    }

    #[test]
    fn seen_mark_round_trips_and_counts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/crashes.json");
        assert_eq!(load_seen_from(&path), 0, "no file: nothing seen");
        save_seen_to(&path, 1_793_339_351_397_520).unwrap();
        assert_eq!(load_seen_from(&path), 1_793_339_351_397_520);
        let c = parse_list(SAMPLE).unwrap();
        assert_eq!(unseen(&c, load_seen_from(&path)), 1);
        assert_eq!(unseen(&c, 0), 3);
        assert_eq!(debug_command(42), "coredumpctl debug 42");
    }
}
