//! Process table from `/proc`: names, command lines, owners, memory and CPU
//! usage (sampled between snapshots), ranked search by name or PID, and
//! signalling (terminate / force kill).

use std::collections::HashMap;
use std::time::Instant;

use mandara_i18n::{t, tf};
use serde::Serialize;

mod cpu_capacity;

#[derive(Clone, Debug, Serialize)]
pub struct Process {
    pub pid: u32,
    pub ppid: u32,
    /// Short name (`comm`), e.g. `firefox`.
    pub name: String,
    /// Full command line; empty for kernel threads.
    pub cmdline: String,
    pub user: String,
    pub uid: u32,
    /// Resident memory in bytes.
    pub rss: u64,
    /// CPU usage since the previous sample, as a share of the whole machine
    /// (all logical cores): one core fully busy on four reads 25 %.
    pub cpu: f32,
    /// `R`unning, `S`leeping, `Z`ombie, …
    pub state: char,
    #[serde(skip)]
    ticks: u64,
}

/// `kthreadd`'s pid: kernel threads are it and its children.
const KTHREADD_PID: u32 = 2;
/// Samples closer together than this give no meaningful CPU percentage, s.
const MIN_SAMPLE_GAP_S: f32 = 0.05;
/// Percent per unit.
const PERCENT: f32 = 100.0;
/// `/proc/<pid>/stat` fields after `(comm)`, 0-based from `state` (field 3):
/// ppid (4), utime (14), stime (15), rss in pages (24).
const STAT_PPID: usize = 1;
const STAT_UTIME: usize = 11;
const STAT_STIME: usize = 12;
const STAT_RSS: usize = 21;
/// Keep ordinary procfs files between samples without retaining an unusually
/// large command line after its process exits.
const PROC_REUSE_BYTES: usize = 4 * 1024;

impl Process {
    pub fn is_kernel_thread(&self) -> bool {
        self.cmdline.is_empty() && (self.ppid == KTHREADD_PID || self.pid == KTHREADD_PID)
    }
}

/// File buffers reused across processes and samples.
#[derive(Default)]
struct ProcReader {
    path: String,
    text: String,
    bytes: Vec<u8>,
}

impl ProcReader {
    fn path(&mut self, pid: u32, file: &str) -> &str {
        use std::fmt::Write as _;
        self.path.clear();
        let _ = write!(self.path, "/proc/{pid}/{file}");
        &self.path
    }

    fn read_text(&mut self, pid: u32, file: &str) -> Option<&str> {
        use std::io::Read as _;
        self.path(pid, file);
        self.text.clear();
        std::fs::File::open(&self.path).ok()?.read_to_string(&mut self.text).ok()?;
        Some(&self.text)
    }

    fn read_bytes(&mut self, pid: u32, file: &str) -> Option<&[u8]> {
        use std::io::Read as _;
        self.path(pid, file);
        self.bytes.clear();
        std::fs::File::open(&self.path).ok()?.read_to_end(&mut self.bytes).ok()?;
        Some(&self.bytes)
    }

    fn finish_sample(&mut self) {
        self.text.clear();
        self.bytes.clear();
        if self.text.capacity() > PROC_REUSE_BYTES {
            self.text = String::new();
        }
        if self.bytes.capacity() > PROC_REUSE_BYTES {
            self.bytes = Vec::new();
        }
    }
}

/// Keeps the previous snapshot's CPU ticks so usage can be computed.
#[derive(Default)]
pub struct Sampler {
    prev: HashMap<u32, u64>,
    at: Option<Instant>,
    users: HashMap<u32, String>,
    reader: ProcReader,
}

impl Sampler {
    pub fn new() -> Self {
        Self::default()
    }

    /// All processes (kernel threads excluded), CPU relative to the last call.
    pub fn sample(&mut self) -> Vec<Process> {
        if self.users.is_empty() {
            self.users = read_users();
        }
        let hz = clock_ticks() as f32;
        let now = Instant::now();
        let cpus = cpu_capacity::refresh() as f32;
        let elapsed = self.at.map(|t| now.duration_since(t).as_secs_f32()).unwrap_or(0.0);
        let mut out = Vec::with_capacity(self.prev.len());
        let Ok(dir) = std::fs::read_dir("/proc") else { return out };
        let page = page_size();
        for entry in dir.flatten() {
            let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
            let Some(mut p) = read_process(pid, &self.users, &mut self.reader, page) else { continue };
            if elapsed > MIN_SAMPLE_GAP_S
                && let Some(prev) = self.prev.get(&pid)
            {
                p.cpu = share_of_machine(p.ticks.saturating_sub(*prev), elapsed, hz, cpus);
            }
            out.push(p);
        }
        self.prev.clear();
        self.prev.extend(out.iter().map(|p| (p.pid, p.ticks)));
        self.at = Some(now);
        self.reader.finish_sample();
        out
    }
}

fn clock_ticks() -> u64 {
    rustix::param::clock_ticks_per_second()
}

/// Host-wide logical CPUs, matching the aggregate `/proc/stat` gauge even
/// when this process has an affinity mask or a cgroup CPU quota.
pub fn cpu_capacity() -> usize {
    cpu_capacity::get()
}

/// `ticks` of CPU time used over `elapsed_s` seconds, as a share of the whole
/// machine: percent of all `cpus` logical cores together, so a busy
/// multi-threaded process stays at or below 100 (like the Resources panel),
/// instead of the per-core figure `top` prints.
fn share_of_machine(ticks: u64, elapsed_s: f32, hz: f32, cpus: f32) -> f32 {
    ((ticks as f32 / hz) / elapsed_s * PERCENT / cpus.max(1.0)).min(PERCENT)
}

fn page_size() -> u64 {
    rustix::param::page_size() as u64
}

fn read_users() -> HashMap<u32, String> {
    std::fs::read_to_string("/etc/passwd")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut f = l.split(':');
            let name = f.next()?;
            let uid = f.nth(1)?.parse().ok()?;
            Some((uid, name.to_string()))
        })
        .collect()
}

/// One process, or `None` for kernel threads and vanished pids. Kernel
/// threads are recognized from `stat` alone, before any other file is read.
fn read_process(pid: u32, users: &HashMap<u32, String>, reader: &mut ProcReader, page: u64) -> Option<Process> {
    use std::os::unix::fs::MetadataExt as _;
    let stat = reader.read_text(pid, "stat")?;
    // `pid (comm) state ppid …` — comm may contain spaces and parentheses.
    let open = stat.find('(')?;
    let close = stat.rfind(')')?;
    let name = stat[open + 1..close].to_string();
    let mut rest = stat.get(close + 2..)?.split_whitespace();
    let state = rest.next()?.chars().next()?;
    // `nth` counts from the iterator's current position (after `state`).
    let ppid: u32 = rest.nth(STAT_PPID - 1)?.parse().ok()?;
    let utime: u64 = rest.nth(STAT_UTIME - STAT_PPID - 1)?.parse().ok()?;
    let stime: u64 = rest.nth(STAT_STIME - STAT_UTIME - 1)?.parse().ok()?;
    let rss = rest.nth(STAT_RSS - STAT_STIME - 1)?.parse::<u64>().ok()? * page;
    if ppid == KTHREADD_PID || pid == KTHREADD_PID {
        return None;
    }
    let cmdline = reader
        .read_bytes(pid, "cmdline")
        .map(command_line)
        .unwrap_or_default();
    // The owner of `/proc/<pid>` is the process's uid (one stat, no parsing).
    let uid = std::fs::metadata(reader.path(pid, "")).map(|m| m.uid()).unwrap_or(0);
    Some(Process { pid, ppid, name, cmdline, user: users.get(&uid).cloned().unwrap_or_else(|| uid.to_string()), uid, rss, cpu: 0.0, state, ticks: utime + stime })
}

fn command_line(bytes: &[u8]) -> String {
    let decoded = String::from_utf8_lossy(bytes);
    let mut args = decoded.split('\0').filter(|arg| !arg.is_empty());
    let Some(first) = args.next() else { return String::new() };
    let mut joined = String::with_capacity(decoded.len());
    joined.push_str(first);
    for arg in args {
        joined.push(' ');
        joined.push_str(arg);
    }
    joined
}

/// How the list is ordered when the query doesn't decide.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum SortBy {
    #[default]
    Cpu,
    Memory,
    Name,
}

/// Filters and ranks by `query`: digits match PIDs (exact first, then
/// prefix); text matches names (exact, prefix, substring) then command
/// lines. Ties are broken by `sort`.
pub fn search(procs: &[Process], query: &str, sort: SortBy) -> Vec<Process> {
    let q = query.trim().to_lowercase();
    let mut ranked: Vec<(u8, &Process)> = procs
        .iter()
        .filter_map(|p| {
            if q.is_empty() {
                return Some((0, p));
            }
            if q.chars().all(|c| c.is_ascii_digit()) {
                let pid = p.pid.to_string();
                if pid == q {
                    return Some((0, p));
                }
                if pid.starts_with(&q) {
                    return Some((1, p));
                }
            }
            let name = p.name.to_lowercase();
            if name == q {
                Some((0, p))
            } else if name.starts_with(&q) {
                Some((1, p))
            } else if name.contains(&q) {
                Some((2, p))
            } else if p.cmdline.to_lowercase().contains(&q) {
                Some((3, p))
            } else {
                None
            }
        })
        .collect();
    ranked.sort_by(|(ra, a), (rb, b)| {
        ra.cmp(rb).then_with(|| match sort {
            SortBy::Cpu => b.cpu.total_cmp(&a.cpu).then(b.rss.cmp(&a.rss)),
            SortBy::Memory => b.rss.cmp(&a.rss),
            SortBy::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.pid.cmp(&b.pid)),
        })
    });
    ranked.into_iter().map(|(_, p)| p.clone()).collect()
}

/// Sends SIGTERM (or SIGKILL when `force`). Refuses PID 1 and the shell
/// itself.
pub fn kill(pid: u32, force: bool) -> Result<(), String> {
    if pid <= 1 {
        return Err(t("Refusing to signal init (PID 1)").into());
    }
    if pid == std::process::id() {
        return Err(t("That's Mandara itself — use Studio → Performance → Quit").into());
    }
    let p = rustix::process::Pid::from_raw(pid as i32).ok_or(t("Invalid PID"))?;
    let sig = if force { rustix::process::Signal::KILL } else { rustix::process::Signal::TERM };
    rustix::process::kill_process(p, sig).map_err(|e| match e {
        rustix::io::Errno::PERM => tf("Permission denied: PID {pid} belongs to another user", &[("pid", &pid)]),
        rustix::io::Errno::SRCH => tf("PID {pid} is no longer running", &[("pid", &pid)]),
        e => tf("Could not signal PID {pid}: {error}", &[("pid", &pid), ("error", &e)]),
    })
}

pub fn is_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, name: &str, cmd: &str, cpu: f32) -> Process {
        Process { pid, ppid: 1, name: name.into(), cmdline: cmd.into(), user: "u".into(), uid: 1000, rss: pid as u64, cpu, state: 'S', ticks: 0 }
    }

    #[test]
    fn ranks_names_then_cmdlines_and_pids() {
        let list = vec![p(10, "firefox-bin", "/usr/lib/firefox", 1.0), p(20, "fire", "fire", 0.5), p(30, "bash", "bash -c firefox", 9.0), p(1234, "zsh", "zsh", 0.0)];
        let r: Vec<u32> = search(&list, "fire", SortBy::Cpu).iter().map(|p| p.pid).collect();
        assert_eq!(r, vec![20, 10, 30]);
        assert_eq!(search(&list, "1234", SortBy::Cpu)[0].pid, 1234);
        assert_eq!(search(&list, "12", SortBy::Cpu)[0].pid, 1234);
        assert_eq!(search(&list, "", SortBy::Cpu)[0].pid, 30);
    }

    #[test]
    fn cpu_is_a_share_of_the_whole_machine() {
        // One core fully busy for a second reads 100 % on a single-core
        // machine and a quarter of it on four.
        assert!((share_of_machine(100, 1.0, 100.0, 1.0) - 100.0).abs() < 0.01);
        assert!((share_of_machine(100, 1.0, 100.0, 4.0) - 25.0).abs() < 0.01);
        // All four cores busy at once still reads 100 %, never 400.
        assert!((share_of_machine(400, 1.0, 100.0, 4.0) - 100.0).abs() < 0.01);
        // A missing core count can't divide by zero.
        assert!((share_of_machine(100, 1.0, 100.0, 0.0) - 100.0).abs() < 0.01);
        // Sampling timestamps and procfs reads can straddle a scheduling tick.
        assert_eq!(share_of_machine(401, 1.0, 100.0, 4.0), PERCENT);
    }

    #[test]
    fn cached_host_capacity_is_available_to_rendered_rows() {
        assert!(cpu_capacity() > 0);
    }

    #[test]
    fn command_lines_keep_lossy_utf8_and_empty_argument_behavior() {
        for bytes in [b"".as_slice(), b"\0\0", b"one\0two\0", b"\0one\0\0two\0", b"one two\0three", b"bad\xff\0utf8\0"] {
            let expected = String::from_utf8_lossy(bytes).split('\0').filter(|arg| !arg.is_empty()).collect::<Vec<_>>().join(" ");
            assert_eq!(command_line(bytes), expected);
        }
        let large = "argument".repeat(PROC_REUSE_BYTES);
        assert_eq!(command_line(large.as_bytes()), large);
    }

    #[test]
    fn proc_reader_keeps_small_buffers_and_releases_unusual_command_lines() {
        let mut reader = ProcReader { path: String::with_capacity(32), text: String::from("a process stat"), bytes: Vec::from(b"command\0line".as_slice()) };
        let pointers = (reader.path.as_ptr(), reader.text.as_ptr(), reader.bytes.as_ptr());
        reader.finish_sample();
        assert!(reader.text.is_empty() && reader.bytes.is_empty());
        assert_eq!((reader.path.as_ptr(), reader.text.as_ptr(), reader.bytes.as_ptr()), pointers);
        reader.text.reserve(PROC_REUSE_BYTES + 1);
        reader.bytes.resize(PROC_REUSE_BYTES + 1, b'x');
        reader.finish_sample();
        assert_eq!((reader.text.capacity(), reader.bytes.capacity()), (0, 0));
        assert_eq!(reader.path.as_ptr(), pointers.0);
    }

    #[test]
    fn samples_self() {
        let mut s = Sampler::new();
        let me = std::process::id();
        let procs = s.sample();
        let mine = procs.iter().find(|p| p.pid == me).expect("own process listed");
        assert!(mine.rss > 0);
        assert!(!mine.cmdline.is_empty());
    }

    #[test]
    fn refuses_init_and_self() {
        assert!(kill(1, false).is_err());
        assert!(kill(std::process::id(), false).is_err());
    }
}
