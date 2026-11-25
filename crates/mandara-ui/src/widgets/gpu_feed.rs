//! The shell's one GPU sampler. The resources panel, the `system
//! resources` command and the desktop system monitor all read their GPUs
//! from here, so a device is read (and `nvidia-smi` run) once per
//! [`FRESH`] however many of them show, and they always agree.
//!
//! Nothing here polls: a reading is taken only when a caller asks for one
//! and the last is older than it allows. [`fresh`] reads on the calling
//! thread (background workers); [`latest`] never blocks, handing back what
//! it has and refreshing on the shared services runtime, for threads that
//! draw (the wallpaper thread). Concurrent callers share one read.
//!
//! [`GpuCaps`] says, for each reading a widget can show, whether this
//! machine has it and, if not, why (no GPU, `nvidia-smi` missing, or a
//! driver that doesn't report it), so settings can lock those options.

use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use mandara_config::desktop_widgets::MonitorRow;
use mandara_i18n::tk;

use super::sensors::{self, Gpu, GpuVendor};
use crate::plugins::resources::demo;

/// How old a reading may be before a caller's request takes a new one.
pub const FRESH: Duration = Duration::from_secs(1);
/// Longest a blocking caller waits for another caller's read.
const WAIT: Duration = Duration::from_secs(5);
/// PCI class prefix of display controllers (`0x03xxxx`).
const DISPLAY_CLASS: &str = "0x03";
/// NVIDIA's PCI vendor id.
const NVIDIA_VENDOR: &str = "0x10de";

/// One reading of every GPU.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    /// Discrete before integrated.
    pub gpus: Arc<[Gpu]>,
    /// `nvidia-smi` is installed (NVIDIA cards can be read).
    pub smi: bool,
    /// Bumped by every reading; `0` until the first one.
    pub generation: u64,
}

impl Snapshot {
    pub fn probed(&self) -> bool {
        self.generation > 0
    }

    /// What the readings can show, once there has been one.
    pub fn caps(&self) -> Option<GpuCaps> {
        self.probed().then(|| GpuCaps::of(&self.gpus, self.smi))
    }
}

struct State {
    snap: Snapshot,
    at: Option<Instant>,
    /// A read is in flight.
    busy: bool,
}

/// A coalescing, on-demand GPU reader (the process has one, [`FEED`]; tests
/// make their own).
pub struct Feed {
    state: Mutex<State>,
    done: Condvar,
}

/// Clears the in-flight mark if a read never finishes (it panicked), so
/// waiters and later requests aren't stuck.
struct Pending<'a>(&'a Feed);

impl Pending<'_> {
    fn finish(self, gpus: Vec<Gpu>, smi: bool) -> Snapshot {
        let snap = {
            let mut st = self.0.lock();
            st.snap = Snapshot { gpus: gpus.into(), smi, generation: st.snap.generation + 1 };
            st.at = Some(Instant::now());
            st.busy = false;
            st.snap.clone()
        };
        self.0.done.notify_all();
        std::mem::forget(self);
        snap
    }
}

impl Drop for Pending<'_> {
    fn drop(&mut self) {
        self.0.lock().busy = false;
        self.0.done.notify_all();
    }
}

impl Default for Feed {
    fn default() -> Self {
        Self::new()
    }
}

impl Feed {
    pub fn new() -> Self {
        Self { state: Mutex::new(State { snap: Snapshot::default(), at: None, busy: false }), done: Condvar::new() }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn stale(st: &State, max_age: Duration) -> bool {
        st.at.is_none_or(|t| t.elapsed() >= max_age)
    }

    /// The last reading, whatever its age.
    pub fn peek(&self) -> Snapshot {
        self.lock().snap.clone()
    }

    /// A reading no older than `max_age`, taken with `read` on this thread
    /// when needed (or waited for when another caller is taking it).
    pub fn fresh(&self, max_age: Duration, read: impl FnOnce() -> (Vec<Gpu>, bool)) -> Snapshot {
        let mut st = self.lock();
        let deadline = Instant::now() + WAIT;
        while st.busy {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return st.snap.clone();
            }
            st = self.done.wait_timeout(st, left).unwrap_or_else(|e| e.into_inner()).0;
        }
        if !Self::stale(&st, max_age) {
            return st.snap.clone();
        }
        st.busy = true;
        drop(st);
        let pending = Pending(self);
        let (gpus, smi) = read();
        pending.finish(gpus, smi)
    }

    /// The last reading, and the job of taking a new one when it is older
    /// than `max_age` and no read is in flight (the caller runs it
    /// elsewhere).
    fn begin(&self, max_age: Duration) -> (Snapshot, Option<Pending<'_>>) {
        let mut st = self.lock();
        let start = !st.busy && Self::stale(&st, max_age);
        st.busy |= start;
        let snap = st.snap.clone();
        drop(st);
        // Lazily: a guard made and dropped here would clear another
        // caller's in-flight mark (and lock again under this lock).
        (snap, start.then(|| Pending(self)))
    }
}

/// The process's GPU feed.
static FEED: std::sync::LazyLock<Feed> = std::sync::LazyLock::new(Feed::new);

/// Every GPU, sysfs plus `nvidia-smi` (or the demo's). Blocking.
fn read() -> (Vec<Gpu>, bool) {
    if demo::active() {
        return (demo::gpus(), true);
    }
    (sensors::read_all_gpus(), sensors::has_nvidia_smi())
}

/// A reading no older than `max_age`, read on this thread if needed.
/// Blocking (may run `nvidia-smi`): call off the UI and drawing threads.
pub fn fresh(max_age: Duration) -> Snapshot {
    FEED.fresh(max_age, read)
}

/// The last reading without blocking; when it is older than `max_age` a new
/// one is taken on the services runtime for the next call. The demo's are
/// made up on the spot.
pub fn latest(max_age: Duration) -> Snapshot {
    if demo::active() {
        return fresh(max_age);
    }
    let (snap, job) = FEED.begin(max_age);
    if let Some(job) = job {
        mandara_services::runtime::spawn("gpu-feed", async move {
            let (gpus, smi) = mandara_services::runtime::unblock(read).await;
            job.finish(gpus, smi);
        });
    }
    snap
}

/// The last reading, never reading anything (previews).
pub fn peek() -> Snapshot {
    if demo::active() {
        return fresh(FRESH);
    }
    FEED.peek()
}

// ── What a machine can show ──────────────────────────────────────────────

/// A GPU reading a widget can show.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum GpuStat {
    Load,
    Vram,
    Temp,
    Power,
}

impl GpuStat {
    pub const ALL: [GpuStat; 4] = [GpuStat::Load, GpuStat::Vram, GpuStat::Temp, GpuStat::Power];

    /// The GPU reading a system monitor row shows (`None`: not a GPU row).
    pub fn of_row(r: MonitorRow) -> Option<Self> {
        match r {
            MonitorRow::Gpu => Some(GpuStat::Load),
            MonitorRow::Vram => Some(GpuStat::Vram),
            MonitorRow::GpuTemp => Some(GpuStat::Temp),
            MonitorRow::GpuPower => Some(GpuStat::Power),
            _ => None,
        }
    }

    /// Whether `g` reports this reading.
    pub fn of(self, g: &Gpu) -> bool {
        match self {
            GpuStat::Load => g.load.is_some(),
            GpuStat::Vram => g.vram_used.is_some() && g.vram_total.is_some(),
            GpuStat::Temp => g.temp_c.is_some(),
            GpuStat::Power => g.power_w.is_some(),
        }
    }
}

/// Whether a GPU reading can be shown here.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Support {
    Yes,
    /// No graphics card at all.
    NoGpu,
    /// An NVIDIA card, but `nvidia-smi` isn't installed.
    NeedsNvidiaSmi,
    /// The card's driver doesn't report it.
    NotReported,
}

impl Support {
    pub fn available(self) -> bool {
        self == Support::Yes
    }

    /// Why the reading is locked (a `tk()` key), `None` when it isn't.
    pub fn reason(self) -> Option<&'static str> {
        match self {
            Support::Yes => None,
            Support::NoGpu => Some(tk("No graphics card found")),
            Support::NeedsNvidiaSmi => Some(tk("Install your NVIDIA driver's utilities (nvidia-smi) to read this card")),
            Support::NotReported => Some(tk("Your graphics card doesn't report this")),
        }
    }

    /// A one- or two-word reason for a menu row's hint.
    pub fn short_reason(self) -> Option<&'static str> {
        match self {
            Support::Yes => None,
            Support::NoGpu => Some(tk("No GPU")),
            Support::NeedsNvidiaSmi => Some(tk("Needs nvidia-smi")),
            Support::NotReported => Some(tk("Not reported")),
        }
    }
}

/// What each GPU reading can show on this machine.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct GpuCaps {
    pub load: Support,
    pub vram: Support,
    pub temp: Support,
    pub power: Support,
}

impl GpuCaps {
    pub fn get(&self, stat: GpuStat) -> Support {
        match stat {
            GpuStat::Load => self.load,
            GpuStat::Vram => self.vram,
            GpuStat::Temp => self.temp,
            GpuStat::Power => self.power,
        }
    }

    /// Whether a system monitor row can show here (rows that aren't GPU
    /// readings always can; with no probe yet, nothing is locked).
    pub fn row(caps: Option<Self>, r: MonitorRow) -> Support {
        GpuStat::of_row(r).zip(caps).map_or(Support::Yes, |(s, c)| c.get(s))
    }

    /// Any GPU reading at all.
    pub fn any(&self) -> bool {
        GpuStat::ALL.iter().any(|s| self.get(*s).available())
    }

    /// From readings; `smi` says whether `nvidia-smi` is installed.
    pub fn of(gpus: &[Gpu], smi: bool) -> Self {
        let one = |stat: GpuStat| {
            if gpus.iter().any(|g| stat.of(g)) {
                Support::Yes
            } else if gpus.is_empty() {
                Support::NoGpu
            } else if !smi && gpus.iter().any(|g| g.vendor == GpuVendor::Nvidia) {
                Support::NeedsNvidiaSmi
            } else {
                Support::NotReported
            }
        };
        Self { load: one(GpuStat::Load), vram: one(GpuStat::Vram), temp: one(GpuStat::Temp), power: one(GpuStat::Power) }
    }

    /// From sysfs under `root` without running `nvidia-smi` (a settings
    /// page): an NVIDIA card counts as fully readable when `smi` says the
    /// tool is installed. Blocking: call off the UI thread.
    pub fn probe_at(root: &Path, smi: bool) -> Self {
        let mut gpus = sensors::read_gpus(root);
        if !gpus.iter().any(|g| g.vendor == GpuVendor::Nvidia) && nvidia_on_pci(root) {
            gpus.push(Gpu { slot: String::new(), vendor: GpuVendor::Nvidia, name: None, integrated: false, load: None, vram_used: None, vram_total: None, temp_c: None, freq_mhz: None, power_w: None });
        }
        if smi {
            for g in gpus.iter_mut().filter(|g| g.vendor == GpuVendor::Nvidia) {
                (g.load, g.vram_used, g.vram_total, g.temp_c, g.power_w) = (Some(0.0), Some(0), Some(1), Some(0.0), Some(0.0));
            }
        }
        Self::of(&gpus, smi)
    }

    /// This machine's (or the demo's). Blocking: call off the UI thread.
    pub fn probe() -> Self {
        if demo::active() {
            return Self::of(&demo::gpus(), true);
        }
        Self::probe_at(Path::new("/"), sensors::has_nvidia_smi())
    }
}

/// An NVIDIA display controller on the PCI bus (the proprietary driver
/// may not register a DRM card without modesetting).
fn nvidia_on_pci(root: &Path) -> bool {
    let Ok(dir) = std::fs::read_dir(root.join("sys/bus/pci/devices")) else { return false };
    dir.flatten().any(|e| {
        let read = |f: &str| std::fs::read_to_string(e.path().join(f)).map(|s| s.trim().to_ascii_lowercase()).unwrap_or_default();
        read("vendor") == NVIDIA_VENDOR && read("class").starts_with(DISPLAY_CLASS)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn gpu(vendor: GpuVendor, load: Option<f32>) -> Gpu {
        Gpu { slot: "0000:01:00.0".into(), vendor, name: None, integrated: false, load, vram_used: None, vram_total: None, temp_c: None, freq_mhz: None, power_w: None }
    }

    #[test]
    fn callers_share_one_read_while_it_is_fresh() {
        let feed = Feed::new();
        let reads = AtomicUsize::new(0);
        let read = || {
            reads.fetch_add(1, Ordering::SeqCst);
            (vec![gpu(GpuVendor::Amd, Some(0.5))], false)
        };
        assert!(!feed.peek().probed(), "nothing read before anyone asks");
        let a = feed.fresh(Duration::from_secs(60), read);
        let b = feed.fresh(Duration::from_secs(60), read);
        assert_eq!(reads.load(Ordering::SeqCst), 1, "the second caller reuses the reading");
        assert_eq!((a.generation, b.generation), (1, 1));
        assert!(Arc::ptr_eq(&a.gpus, &b.gpus), "one shared snapshot, not a copy");
        let c = feed.fresh(Duration::ZERO, read);
        assert_eq!((reads.load(Ordering::SeqCst), c.generation), (2, 2), "a stale reading is taken again");
    }

    #[test]
    fn a_background_refresh_starts_once_and_lands() {
        let feed = Feed::new();
        let (snap, job) = feed.begin(FRESH);
        assert!(!snap.probed());
        let job = job.expect("stale: a refresh starts");
        assert!(feed.begin(FRESH).1.is_none(), "one read in flight at a time");
        assert!(feed.begin(FRESH).1.is_none(), "and a refused request leaves it in flight");
        job.finish(vec![gpu(GpuVendor::Nvidia, Some(0.1))], true);
        let (snap, job) = feed.begin(Duration::from_secs(60));
        assert!(job.is_none(), "fresh: nothing to do");
        assert_eq!((snap.generation, snap.gpus.len(), snap.smi), (1, 1, true));
    }

    #[test]
    fn a_failed_read_does_not_wedge_the_feed() {
        let feed = Feed::new();
        let (_, job) = feed.begin(FRESH);
        drop(job); // the reader panicked
        assert!(feed.begin(FRESH).1.is_some(), "the next request retries");
    }

    #[test]
    fn caps_explain_what_is_missing() {
        assert!(GpuCaps::of(&[], false).load == Support::NoGpu && GpuCaps::of(&[], true).temp == Support::NoGpu);
        let nvidia = [gpu(GpuVendor::Nvidia, None)];
        assert_eq!(GpuCaps::of(&nvidia, false).load, Support::NeedsNvidiaSmi);
        assert_eq!(GpuCaps::of(&nvidia, true).load, Support::NotReported, "installed but nothing came back");
        let intel = [gpu(GpuVendor::Intel, None)];
        let caps = GpuCaps::of(&intel, false);
        assert_eq!(caps.load, Support::NotReported);
        assert!(!caps.any());
        let amd = [Gpu { temp_c: Some(50.0), power_w: Some(17.0), ..gpu(GpuVendor::Amd, Some(0.2)) }, gpu(GpuVendor::Intel, None)];
        let caps = GpuCaps::of(&amd, false);
        assert!(caps.load.available() && caps.temp.available() && caps.power.available());
        assert_eq!(caps.vram, Support::NotReported);
        assert_eq!(GpuCaps::row(Some(caps), MonitorRow::Vram), Support::NotReported);
        assert_eq!(GpuCaps::row(Some(GpuCaps::of(&[], false)), MonitorRow::Cpu), Support::Yes, "not a GPU reading");
        assert_eq!(GpuCaps::row(None, MonitorRow::Gpu), Support::Yes, "unknown yet: not locked");
        assert_eq!(MonitorRow::ALL.iter().filter(|r| GpuStat::of_row(**r).is_some()).count(), 4);
        assert!(MonitorRow::ALL.iter().all(|r| r.is_gpu() == GpuStat::of_row(*r).is_some()));
        for s in [Support::NoGpu, Support::NeedsNvidiaSmi, Support::NotReported] {
            assert!(s.reason().is_some() && s.short_reason().is_some());
        }
        assert!(Support::Yes.reason().is_none());
    }

    #[test]
    fn probe_finds_nvidia_without_running_it() {
        let dir = std::env::temp_dir().join(format!("mandara-gpu-probe-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dev = dir.join("sys/bus/pci/devices/0000:01:00.0");
        std::fs::create_dir_all(&dev).unwrap();
        std::fs::write(dev.join("vendor"), "0x10de\n").unwrap();
        std::fs::write(dev.join("class"), "0x030000\n").unwrap();
        assert_eq!(GpuCaps::probe_at(&dir, false).load, Support::NeedsNvidiaSmi);
        assert!(GpuCaps::probe_at(&dir, true).vram.available());
        std::fs::write(dev.join("class"), "0x040300\n").unwrap(); // its audio function
        assert_eq!(GpuCaps::probe_at(&dir, true).load, Support::NoGpu);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Reads this machine's GPUs through the shared feed (read-only:
    /// sysfs and, when installed, `nvidia-smi`).
    #[test]
    fn live_feed_reads_without_panicking() {
        let snap = fresh(Duration::ZERO);
        assert!(snap.probed());
        let caps = snap.caps().expect("probed");
        assert_eq!(caps.any(), snap.gpus.iter().any(|g| GpuStat::ALL.iter().any(|s| s.of(g))));
        let _ = GpuCaps::probe();
    }
}
