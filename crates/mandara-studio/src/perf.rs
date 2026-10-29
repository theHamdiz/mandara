//! Resource monitor: samples the shell's process (found through
//! [`ShellLink`]) and Studio's own process from `/proc`, once a second
//! while the Performance page is on screen (and not at all otherwise).
//! Studio runs as its own process, so the shell figures never include it.

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, AsyncApp, Entity, Global};

/// How many samples the sparklines keep (one per second).
pub const HISTORY: usize = 60;
const INTERVAL: Duration = Duration::from_secs(1);
/// The shell's pid is re-resolved over IPC every this many samples.
const PID_REFRESH_SAMPLES: u32 = 10;
/// Percent per whole (CPU time / wall time → percent of one core).
const PERCENT: f32 = 100.0;
/// KiB per MiB.
const KIB_PER_MIB: f32 = 1024.0;
const KIB_PER_GIB: u64 = 1024 * 1024;
const BYTES_PER_KIB: u64 = 1024;
/// Shortest interval CPU usage is computed over (guards a zero divisor).
const MIN_SAMPLE_SECS: f32 = 1e-3;
/// `/proc/<pid>/stat` fields after the command name: state is 0.
const STAT_UTIME: usize = 11;
const STAT_STIME: usize = 12;
const STAT_THREADS: usize = 17;

/// Resolves the running shell's pid (over IPC); installed by the app.
#[derive(Clone)]
pub struct ShellLink(pub Arc<dyn Fn() -> Option<u32> + Send + Sync>);
impl Global for ShellLink {}

/// Runs a shell command over IPC (`["restart"]`, `["quit"]`); installed by
/// the app. Called off the UI thread.
#[derive(Clone)]
pub struct ShellIpc(pub Arc<dyn Fn(Vec<String>) -> Result<(), String> + Send + Sync>);
impl Global for ShellIpc {}

/// Runs a shell command over IPC and returns its JSON result
/// (`["compositor", "clients"]`, `["diagnostics", "frames"]`); installed by
/// the app. Called off the UI thread.
#[derive(Clone)]
pub struct ShellQuery(pub Arc<dyn Fn(Vec<String>) -> Result<serde_json::Value, String> + Send + Sync>);
impl Global for ShellQuery {}

/// Queries the shell off the UI thread; `None` when Studio runs without the
/// app's IPC hooks (tests, embedding).
pub fn query(args: &[&str], cx: &App) -> Option<gpui::Task<Result<serde_json::Value, String>>> {
    let q = cx.try_global::<ShellQuery>()?.clone();
    let args: Vec<String> = args.iter().map(|a| a.to_string()).collect();
    Some(cx.background_executor().spawn(async move { (q.0)(args) }))
}

/// One process reading.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Usage {
    /// Percent of one core (like `top`): 100 = one core fully busy.
    pub cpu: f32,
    /// Resident memory in MiB.
    pub rss_mib: f32,
    pub threads: u32,
    /// Video memory in MiB, from the DRM driver's per-process accounting
    /// (`None` where the driver doesn't report it).
    pub vram_mib: Option<f32>,
}

#[derive(Default)]
pub struct PerfMonitor {
    pub shell_pid: Option<u32>,
    pub shell: VecDeque<Usage>,
    pub studio: Usage,
    pub cores: usize,
    /// A sampling loop is running.
    polling: bool,
}

impl PerfMonitor {
    pub fn latest(&self) -> Option<Usage> {
        self.shell.back().copied()
    }

    /// Average shell CPU over the kept history.
    pub fn average_cpu(&self) -> f32 {
        if self.shell.is_empty() {
            return 0.0;
        }
        self.shell.iter().map(|u| u.cpu).sum::<f32>() / self.shell.len() as f32
    }

    pub fn peak_rss(&self) -> f32 {
        self.shell.iter().map(|u| u.rss_mib).fold(0.0, f32::max)
    }

    pub fn peak_vram(&self) -> f32 {
        self.shell.iter().filter_map(|u| u.vram_mib).fold(0.0, f32::max)
    }
}

/// Raw counters from `/proc/<pid>/stat` + `status`.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Raw {
    ticks: u64,
    rss_kib: u64,
    threads: u32,
    vram_kib: Option<u64>,
}

fn read_raw(pid: u32) -> Option<Raw> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let raw = parse_stat(&stat, &std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?)?;
    Some(Raw { vram_kib: read_vram_kib(pid), ..raw })
}

/// Video memory of `pid`: every open DRM file reports its client's usage in
/// `/proc/<pid>/fdinfo` (one GPU client can be open through several files,
/// so each client id counts once).
fn read_vram_kib(pid: u32) -> Option<u64> {
    let mut clients = std::collections::HashMap::new();
    for entry in std::fs::read_dir(format!("/proc/{pid}/fdinfo")).ok()?.flatten() {
        if let Some((client, kib)) = std::fs::read_to_string(entry.path()).ok().as_deref().and_then(parse_fdinfo) {
            clients.insert(client, kib);
        }
    }
    (!clients.is_empty()).then(|| clients.values().sum())
}

/// Keys drivers use for device-local memory in fdinfo (amdgpu, then the
/// generic DRM region name used by i915 / xe and others).
const VRAM_KEYS: &[&str] = &["drm-memory-vram", "drm-total-local0"];

/// `(client id, KiB)` from one fdinfo file, if it is a DRM client that
/// reports device memory.
fn parse_fdinfo(text: &str) -> Option<(String, u64)> {
    let field = |key: &str| text.lines().find_map(|l| l.strip_prefix(key)?.strip_prefix(':').map(str::trim));
    let client = field("drm-client-id")?.to_string();
    let value = VRAM_KEYS.iter().find_map(|k| field(k))?;
    let mut parts = value.split_whitespace();
    let n: u64 = parts.next()?.parse().ok()?;
    let kib = match parts.next() {
        Some("MiB") => n * KIB_PER_MIB as u64,
        Some("GiB") => n * KIB_PER_GIB,
        Some("KiB") => n,
        // Bare numbers are bytes.
        _ => n / BYTES_PER_KIB,
    };
    Some((client, kib))
}

/// Parses `utime + stime`, the thread count and `VmRSS`. The command name
/// in `stat` may contain spaces, so fields are counted after the last `)`.
fn parse_stat(stat: &str, status: &str) -> Option<Raw> {
    let rest = stat.get(stat.rfind(')')? + 1..)?;
    // After the name: state(0) ppid(1) … utime(11) stime(12) … num_threads(17)
    let mut fields = rest.split_whitespace();
    let utime: u64 = fields.nth(STAT_UTIME)?.parse().ok()?;
    let stime: u64 = fields.nth(STAT_STIME - STAT_UTIME - 1)?.parse().ok()?;
    let threads: u32 = fields.nth(STAT_THREADS - STAT_STIME - 1)?.parse().ok()?;
    let rss_kib = status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .and_then(|v| v.split_whitespace().next()?.parse().ok())
        .unwrap_or(0);
    Some(Raw { ticks: utime + stime, rss_kib, threads, vram_kib: None })
}

fn usage(prev: Option<(Raw, Instant)>, now: (Raw, Instant), hz: f32) -> Usage {
    let cpu = match prev {
        Some((p, t)) if now.0.ticks >= p.ticks => {
            let secs = now.1.duration_since(t).as_secs_f32().max(MIN_SAMPLE_SECS);
            (now.0.ticks - p.ticks) as f32 / hz / secs * PERCENT
        }
        _ => 0.0,
    };
    Usage { cpu, rss_mib: now.0.rss_kib as f32 / KIB_PER_MIB, threads: now.0.threads, vram_mib: now.0.vram_kib.map(|k| k as f32 / KIB_PER_MIB) }
}

/// The monitor (idle until [`poll_while`] runs it).
pub fn new(cx: &mut App) -> Entity<PerfMonitor> {
    let cores = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
    cx.new(|_| PerfMonitor { cores, ..Default::default() })
}

/// Samples every [`INTERVAL`] (notifying observers) for as long as `active`
/// says the numbers are on screen; a no-op while a loop already runs.
pub fn poll_while(monitor: &Entity<PerfMonitor>, active: impl Fn(&mut AsyncApp) -> bool + 'static, cx: &mut App) {
    if monitor.update(cx, |m, _| std::mem::replace(&mut m.polling, true)) {
        return;
    }
    let link = cx.try_global::<ShellLink>().cloned();
    let weak = monitor.downgrade();
    cx.spawn(async move |cx| {
        let hz = rustix::param::clock_ticks_per_second() as f32;
        let me = std::process::id();
        let mut shell_prev: Option<(Raw, Instant)> = None;
        let mut studio_prev: Option<(Raw, Instant)> = None;
        let mut pid: Option<u32> = weak.read_with(cx, |m, _| m.shell_pid).ok().flatten();
        let mut lookups = 0u32;
        loop {
            if !active(cx) {
                let _ = weak.update(cx, |m, _| m.polling = false);
                return;
            }
            let link = link.clone();
            let known = pid;
            // Re-resolve the pid every few samples (or until found), off the UI thread.
            let refresh = known.is_none() || lookups.is_multiple_of(PID_REFRESH_SAMPLES);
            lookups = lookups.wrapping_add(1);
            let (next_pid, shell_raw, studio_raw) = cx
                .background_executor()
                .spawn(async move {
                    let pid = if refresh { link.and_then(|l| (l.0)()).or(known) } else { known };
                    let shell = pid.and_then(read_raw).map(|r| (r, Instant::now()));
                    (shell.and(pid), shell, read_raw(me).map(|r| (r, Instant::now())))
                })
                .await;
            if next_pid != pid {
                shell_prev = None;
            }
            pid = next_pid;
            let alive = weak
                .update(cx, |m, cx| {
                    m.shell_pid = pid;
                    match shell_raw {
                        Some(now) => {
                            m.shell.push_back(usage(shell_prev, now, hz));
                            while m.shell.len() > HISTORY {
                                m.shell.pop_front();
                            }
                        }
                        None => m.shell.clear(),
                    }
                    if let Some(now) = studio_raw {
                        m.studio = usage(studio_prev, now, hz);
                    }
                    cx.notify();
                })
                .is_ok();
            if !alive {
                return;
            }
            shell_prev = shell_raw;
            studio_prev = studio_raw;
            cx.background_executor().timer(INTERVAL).await;
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stat_with_spaces_in_name() {
        let stat = "4242 (mandara (bar)) S 1 4242 4242 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 17 0 123 456 789";
        let status = "Name:\tmandara\nVmRSS:\t  81920 kB\nThreads:\t17\n";
        let raw = parse_stat(stat, status).unwrap();
        assert_eq!(raw, Raw { ticks: 300, rss_kib: 81920, threads: 17, vram_kib: None });
    }

    #[test]
    fn cpu_percent_from_tick_delta() {
        let t0 = Instant::now();
        let a = Raw { ticks: 100, rss_kib: 2048, threads: 4, vram_kib: None };
        let b = Raw { ticks: 150, rss_kib: 2048, threads: 4, vram_kib: None };
        let u = usage(Some((a, t0)), (b, t0 + Duration::from_secs(1)), 100.0);
        assert!((u.cpu - 50.0).abs() < 0.5);
        assert!((u.rss_mib - 2.0).abs() < 1e-3);
        assert_eq!(usage(None, (b, t0), 100.0).cpu, 0.0);
    }

    #[test]
    fn reads_own_process() {
        let raw = read_raw(std::process::id()).unwrap();
        assert!(raw.rss_kib > 0 && raw.threads >= 1);
    }

    #[test]
    fn video_memory_counts_each_gpu_client_once() {
        let amd = "pos:\t0\nflags:\t02100002\ndrm-driver:\tamdgpu\ndrm-client-id:\t6661\ndrm-memory-vram:\t187036 KiB\ndrm-memory-gtt: \t4104 KiB\n";
        assert_eq!(parse_fdinfo(amd), Some(("6661".into(), 187036)));
        assert_eq!(parse_fdinfo("drm-client-id:\t7\ndrm-total-local0:\t12 MiB\n"), Some(("7".into(), 12 * 1024)));
        assert_eq!(parse_fdinfo("drm-client-id:\t8\ndrm-total-local0:\t2048\n"), Some(("8".into(), 2)));
        // Not a DRM file, or a driver without device memory.
        assert_eq!(parse_fdinfo("pos:\t0\nflags:\t02\n"), None);
        assert_eq!(parse_fdinfo("drm-client-id:\t9\ndrm-memory-gtt:\t4104 KiB\n"), None);
    }
}
