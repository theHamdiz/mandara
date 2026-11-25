//! Readings for the desktop system monitor, sampled on the wallpaper
//! thread only while a monitor widget shows on a visible display (the
//! renderer drops the sampler otherwise). Every reading is a handful of
//! small `/proc` and `/sys` files; sensor paths are found once and cached.
//! The GPUs come from the shell's one GPU sampler
//! ([`crate::widgets::gpu_feed`], shared with the resources panel: AMD and
//! Intel through sysfs, NVIDIA through `nvidia-smi`), which never blocks
//! this thread. `MANDARA_RESOURCES_DEMO=1` swaps them for calm fictional
//! values (docs screenshots), like the resources panel.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mandara_config::desktop_widgets::MonitorRow;
use mandara_i18n::tk;

use crate::widgets::gpu_feed::{self, GpuStat};
use crate::widgets::sensors::Gpu;
use crate::widgets::system::{CpuTimes, cpu_usage, disk_usage, read_cpu_times};

/// Samples kept for the graphs.
pub const HISTORY: usize = 48;
/// hwmon reports milli-degrees.
const MILLI: f32 = 1000.0;
/// Kibibyte (`/proc/meminfo` unit).
const KIB: u64 = 1024;
/// CPU sensor drivers, best first.
const CPU_SENSORS: &[&str] = &["k10temp", "zenpower", "coretemp", "cpu_thermal", "acpitz"];
/// The filesystem the disk row measures.
const DISK_PATH: &str = "/";
/// How old the shared GPU reading may be when a sample asks for it (the
/// resources panel's reading is reused within this).
const GPU_MAX_AGE: Duration = Duration::from_millis(500);
/// Demo readings (fictional).
const DEMO_CPU: f32 = 0.14;
const DEMO_CPU_TEMP: f32 = 47.0;
const DEMO_FAN_RPM: u32 = 820;
const DEMO_BATTERY: f32 = 0.86;
const DEMO_DISK: (u64, u64) = (412_000_000_000, 1_000_000_000_000);
const DEMO_DOWN: f64 = 1_850_000.0;
const DEMO_UP: f64 = 96_000.0;
/// Demo history wobble.
const DEMO_WAVE: f32 = 0.06;

/// A fixed-size history (oldest first), no allocation after creation.
#[derive(Clone, Debug)]
pub struct History {
    values: Vec<f32>,
    next: usize,
    filled: bool,
}

impl History {
    pub fn new(len: usize) -> Self {
        Self { values: vec![0.0; len.max(2)], next: 0, filled: false }
    }

    pub fn push(&mut self, v: f32) {
        self.values[self.next] = v;
        self.next = (self.next + 1) % self.values.len();
        self.filled |= self.next == 0;
    }

    /// Values oldest first, each divided by `scale` (`0..=1` for graphs).
    pub fn normalized(&self, scale: f32) -> Vec<f32> {
        let n = self.values.len();
        let (start, len) = if self.filled { (self.next, n) } else { (0, self.next) };
        (0..len).map(|i| self.values[(start + i) % n] / scale.max(f32::EPSILON)).collect()
    }

    pub fn max(&self) -> f32 {
        self.values.iter().copied().fold(0.0, f32::max)
    }
}

/// One GPU's readings (only the wanted ones are filled in).
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct GpuReading {
    /// Busy fraction.
    pub load: Option<f32>,
    /// `(used, total)` bytes.
    pub vram: Option<(u64, u64)>,
    pub temp: Option<f32>,
    pub power_w: Option<f32>,
}

impl GpuReading {
    fn of(g: &Gpu, wants: Wants) -> Self {
        Self {
            load: g.load.filter(|_| wants.gpu),
            vram: g.vram_used.zip(g.vram_total).filter(|_| wants.vram),
            temp: g.temp_c.filter(|_| wants.gpu_temp),
            power_w: g.power_w.filter(|_| wants.gpu_power),
        }
    }

    /// Which readings exist, one bit each (keys the label plane).
    pub fn mask(&self) -> u8 {
        u8::from(self.load.is_some()) | u8::from(self.vram.is_some()) << 1 | u8::from(self.temp.is_some()) << 2 | u8::from(self.power_w.is_some()) << 3
    }
}

/// One sample.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    /// Busy fraction.
    pub cpu: Option<f32>,
    /// `(used, total)` bytes.
    pub memory: Option<(u64, u64)>,
    /// `None` without swap.
    pub swap: Option<(u64, u64)>,
    pub disk: Option<(u64, u64)>,
    pub cpu_temp: Option<f32>,
    /// One per GPU that reports anything, discrete first (the storage is
    /// kept between samples).
    pub gpus: Vec<GpuReading>,
    /// The GPU sampler has read at least once, so an empty `gpus` means
    /// this machine has none to show.
    pub gpus_probed: bool,
    pub fan_rpm: Option<u32>,
    pub battery: Option<f32>,
    /// Bytes per second.
    pub down: Option<f64>,
    pub up: Option<f64>,
}

/// Which reads a sample needs (the union of the shown rows).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Wants {
    pub cpu: bool,
    pub gpu: bool,
    pub vram: bool,
    pub memory: bool,
    pub swap: bool,
    pub disk: bool,
    pub gpu_power: bool,
    pub cpu_temp: bool,
    pub gpu_temp: bool,
    pub fan: bool,
    pub battery: bool,
    pub net: bool,
}

impl Wants {
    pub fn add(&mut self, rows: &[MonitorRow]) {
        for r in rows {
            match r {
                MonitorRow::Cpu | MonitorRow::CpuGraph => self.cpu = true,
                MonitorRow::Gpu => self.gpu = true,
                MonitorRow::Vram => self.vram = true,
                MonitorRow::Memory => self.memory = true,
                MonitorRow::Swap => self.swap = true,
                MonitorRow::Disk => self.disk = true,
                MonitorRow::GpuPower => self.gpu_power = true,
                MonitorRow::CpuTemp => self.cpu_temp = true,
                MonitorRow::GpuTemp => self.gpu_temp = true,
                MonitorRow::Fan => self.fan = true,
                MonitorRow::Battery => self.battery = true,
                MonitorRow::Download | MonitorRow::Upload | MonitorRow::NetGraph => self.net = true,
            }
        }
    }

    /// Any GPU reading.
    pub fn any_gpu(&self) -> bool {
        self.gpu || self.vram || self.gpu_temp || self.gpu_power
    }

    fn gpu_stat(&self, stat: GpuStat) -> bool {
        match stat {
            GpuStat::Load => self.gpu,
            GpuStat::Vram => self.vram,
            GpuStat::Temp => self.gpu_temp,
            GpuStat::Power => self.gpu_power,
        }
    }
}

/// Refills `out` (keeping its storage) with the wanted readings of every
/// GPU that reports one of them.
fn fill_gpus(out: &mut Vec<GpuReading>, gpus: &[Gpu], wants: Wants) {
    out.clear();
    out.extend(gpus.iter().filter(|g| GpuStat::ALL.iter().any(|s| wants.gpu_stat(*s) && s.of(g))).map(|g| GpuReading::of(g, wants)));
}

/// Sensor paths, found on first use.
#[derive(Default)]
struct Paths {
    found: bool,
    cpu_temp: Option<PathBuf>,
    fan: Option<PathBuf>,
    battery: Option<PathBuf>,
}

/// How a sampler takes its GPUs from the shared GPU sampler.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum GpuRead {
    /// The last reading; a new one is taken in the background when due
    /// (the drawing thread never waits).
    Latest,
    /// Only what was already read (a thumbnail never starts a read).
    Peek,
    /// Reads and waits (a one-off render on a worker thread).
    Wait,
}

/// The sampler with its deltas and histories.
pub struct Sampler {
    prev_cpu: Option<CpuTimes>,
    prev_net: Option<(u64, u64, Instant)>,
    paths: Paths,
    pub reading: Reading,
    pub cpu_history: History,
    pub down_history: History,
    pub up_history: History,
    /// Bumped by every sample (keys the value planes).
    pub generation: u64,
    pub sampled_at: Option<Instant>,
    demo: bool,
    /// How the GPUs are taken from the shared GPU sampler.
    gpu_read: GpuRead,
    /// Samples taken (tests, diagnostics).
    pub samples: u64,
}

impl Default for Sampler {
    fn default() -> Self {
        Self::new()
    }
}

impl Sampler {
    pub fn new() -> Self {
        Self {
            prev_cpu: None,
            prev_net: None,
            paths: Paths::default(),
            reading: Reading::default(),
            cpu_history: History::new(HISTORY),
            down_history: History::new(HISTORY),
            up_history: History::new(HISTORY),
            generation: 0,
            sampled_at: None,
            demo: crate::plugins::resources::demo::active(),
            gpu_read: GpuRead::Latest,
            samples: 0,
        }
    }

    /// A sampler for a one-off preview (Studio's thumbnails): the GPUs are
    /// whatever the shared sampler last read, so a preview never starts a
    /// GPU read of its own.
    pub fn preview() -> Self {
        Self { gpu_read: GpuRead::Peek, ..Self::new() }
    }

    /// A sampler for a one-off render on a worker thread, which waits for
    /// the GPU reading (the shared one when it is fresh).
    pub fn waiting() -> Self {
        Self { gpu_read: GpuRead::Wait, ..Self::new() }
    }

    fn sample_gpus(&mut self, wants: Wants) {
        if !wants.any_gpu() {
            self.reading.gpus.clear();
            self.reading.gpus_probed = false;
            return;
        }
        let snap = match self.gpu_read {
            GpuRead::Latest => gpu_feed::latest(GPU_MAX_AGE),
            GpuRead::Peek => gpu_feed::peek(),
            GpuRead::Wait => gpu_feed::fresh(GPU_MAX_AGE),
        };
        fill_gpus(&mut self.reading.gpus, &snap.gpus, wants);
        self.reading.gpus_probed = snap.probed();
    }

    /// Takes one sample of what `wants` asks for.
    pub fn sample(&mut self, wants: Wants, now: Instant) {
        self.samples += 1;
        self.generation += 1;
        self.sampled_at = Some(now);
        if self.demo {
            self.demo_sample(wants);
            return;
        }
        if !self.paths.found {
            self.paths = find_paths();
        }
        let r = &mut self.reading;
        if wants.cpu {
            let next = read_cpu_times();
            r.cpu = match (self.prev_cpu, next) {
                (Some(p), Some(n)) => Some(cpu_usage(p, n)),
                _ => None,
            };
            self.prev_cpu = next;
            self.cpu_history.push(r.cpu.unwrap_or(0.0));
        }
        if wants.memory || wants.swap {
            let text = std::fs::read_to_string("/proc/meminfo").ok();
            r.memory = text.as_deref().and_then(meminfo).filter(|_| wants.memory);
            r.swap = text.as_deref().and_then(swapinfo).filter(|_| wants.swap);
        }
        if wants.disk {
            r.disk = disk_usage(DISK_PATH);
        }
        let read_f = |p: &Option<PathBuf>| p.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| s.trim().parse::<f32>().ok());
        if wants.cpu_temp {
            r.cpu_temp = read_f(&self.paths.cpu_temp).map(|v| v / MILLI);
        }
        if wants.fan {
            r.fan_rpm = read_f(&self.paths.fan).map(|v| v.max(0.0) as u32);
        }
        if wants.battery {
            r.battery = read_f(&self.paths.battery).map(|v| v / 100.0);
        }
        // FreeBSD has none of the procfs / sysfs files above.
        #[cfg(target_os = "freebsd")]
        freebsd_readings(r, wants);
        if wants.net {
            let next = read_net_bytes();
            let (down, up) = match (self.prev_net, next) {
                (Some((rx0, tx0, t0)), Some((rx, tx))) => {
                    let dt = now.saturating_duration_since(t0).as_secs_f64().max(f64::EPSILON);
                    (Some(rx.saturating_sub(rx0) as f64 / dt), Some(tx.saturating_sub(tx0) as f64 / dt))
                }
                _ => (None, None),
            };
            r.down = down;
            r.up = up;
            self.prev_net = next.map(|(rx, tx)| (rx, tx, now));
            self.down_history.push(down.unwrap_or(0.0) as f32);
            self.up_history.push(up.unwrap_or(0.0) as f32);
        }
        self.sample_gpus(wants);
    }

    fn demo_sample(&mut self, wants: Wants) {
        let wobble = (self.samples as f32 * 0.7).sin() * DEMO_WAVE;
        let (used, total, swap_used, swap_total) = crate::plugins::resources::demo::meminfo();
        self.sample_gpus(wants);
        let r = &mut self.reading;
        r.cpu = wants.cpu.then_some((DEMO_CPU + wobble).max(0.0));
        r.memory = wants.memory.then_some((used, total));
        r.swap = wants.swap.then_some((swap_used, swap_total));
        r.disk = wants.disk.then_some(DEMO_DISK);
        r.cpu_temp = wants.cpu_temp.then_some(DEMO_CPU_TEMP);
        r.fan_rpm = wants.fan.then_some(DEMO_FAN_RPM);
        r.battery = wants.battery.then_some(DEMO_BATTERY);
        r.down = wants.net.then_some(DEMO_DOWN * (1.0 + wobble as f64 * 4.0));
        r.up = wants.net.then_some(DEMO_UP * (1.0 - wobble as f64 * 3.0));
        if wants.cpu {
            self.cpu_history.push(r.cpu.unwrap_or(0.0));
        }
        if wants.net {
            self.down_history.push(r.down.unwrap_or(0.0) as f32);
            self.up_history.push(r.up.unwrap_or(0.0) as f32);
        }
    }
}

/// The reading's name (a `tk()` key) in settings and menus.
pub fn row_label(r: MonitorRow) -> &'static str {
    match r {
        MonitorRow::Cpu => tk("CPU"),
        MonitorRow::CpuGraph => tk("CPU graph"),
        MonitorRow::Gpu => tk("GPU"),
        MonitorRow::Vram => tk("VRAM"),
        MonitorRow::Memory => tk("Memory"),
        MonitorRow::Swap => tk("Swap"),
        MonitorRow::GpuPower => tk("GPU power"),
        MonitorRow::CpuTemp => tk("CPU temp"),
        MonitorRow::GpuTemp => tk("GPU temp"),
        MonitorRow::Fan => tk("Fan"),
        MonitorRow::Battery => tk("Battery"),
        MonitorRow::Disk => tk("Disk"),
        MonitorRow::Download => tk("Download"),
        MonitorRow::Upload => tk("Upload"),
        MonitorRow::NetGraph => tk("Network graph"),
    }
}

/// `(used, total)` from `/proc/meminfo`.
pub fn meminfo(text: &str) -> Option<(u64, u64)> {
    let total = meminfo_field(text, "MemTotal:")?;
    let avail = meminfo_field(text, "MemAvailable:")?;
    Some((total.saturating_sub(avail), total))
}

/// Swap `(used, total)` from `/proc/meminfo`; `None` without swap.
pub fn swapinfo(text: &str) -> Option<(u64, u64)> {
    let total = meminfo_field(text, "SwapTotal:")?;
    let free = meminfo_field(text, "SwapFree:")?;
    (total > 0).then(|| (total.saturating_sub(free), total))
}

fn meminfo_field(text: &str, name: &str) -> Option<u64> {
    text.lines().find(|l| l.starts_with(name)).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()).map(|v| v * KIB)
}

/// Received / sent bytes of every interface but loopback (`/proc/net/dev`).
pub fn net_bytes(text: &str) -> Option<(u64, u64)> {
    let mut any = false;
    let (mut rx, mut tx) = (0u64, 0u64);
    for l in text.lines().skip(2) {
        let Some((name, rest)) = l.split_once(':') else { continue };
        if name.trim() == "lo" {
            continue;
        }
        let f: Vec<u64> = rest.split_whitespace().filter_map(|v| v.parse().ok()).collect();
        if f.len() >= 9 {
            rx += f[0];
            tx += f[8];
            any = true;
        }
    }
    any.then_some((rx, tx))
}

/// Received / sent bytes of every link but loopback: `/proc/net/dev`, or
/// FreeBSD's `getifaddrs` link data.
#[cfg(not(target_os = "freebsd"))]
fn read_net_bytes() -> Option<(u64, u64)> {
    std::fs::read_to_string("/proc/net/dev").ok().and_then(|t| net_bytes(&t))
}

#[cfg(target_os = "freebsd")]
fn read_net_bytes() -> Option<(u64, u64)> {
    let links = mandara_services::freebsd::net::traffic();
    (!links.is_empty()).then(|| links.iter().fold((0, 0), |(rx, tx), (_, r, t)| (rx + r, tx + t)))
}

/// FreeBSD: memory from `vm.stats.vm`, the CPU temperature from
/// `dev.cpu.N.temperature` and the battery from the ACPI sysctls.
#[cfg(target_os = "freebsd")]
fn freebsd_readings(r: &mut Reading, wants: Wants) {
    use mandara_services::freebsd::host;
    if wants.memory {
        r.memory = host::memory().map(|m| (m.used(), m.total));
    }
    if wants.cpu_temp {
        r.cpu_temp = host::cpu_temperature();
    }
    if wants.battery {
        r.battery = crate::widgets::battery::read_battery().map(|b| f32::from(b.percent) / 100.0);
    }
}

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

fn hwmons(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir).map(|d| d.flatten().map(|e| e.path()).collect()).unwrap_or_default();
    v.sort();
    v
}

fn find_paths() -> Paths {
    let mut p = Paths { found: true, ..Paths::default() };
    // The GPU readings come from the shared GPU sampler; only the
    // discrete card's fan is a fallback here (its own hwmon).
    let mut cards: Vec<PathBuf> = hwmons(Path::new("/sys/class/drm")).into_iter().filter(|c| c.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("card") && !n.contains('-'))).collect();
    cards.sort_by_key(|c| !c.join("device/mem_info_vram_total").exists());
    if let Some(dev) = cards.iter().map(|c| c.join("device")).find(|d| d.join("gpu_busy_percent").exists())
        && let Some(h) = hwmons(&dev.join("hwmon")).into_iter().next()
    {
        p.fan = Some(h.join("fan1_input")).filter(|f| f.exists());
    }
    let mons = hwmons(Path::new("/sys/class/hwmon"));
    for name in CPU_SENSORS {
        if let Some(h) = mons.iter().find(|h| read_trim(&h.join("name")).as_deref() == Some(name)) {
            p.cpu_temp = Some(h.join("temp1_input")).filter(|f| f.exists());
            break;
        }
    }
    // A fan that is spinning beats the GPU's (often idle at 0 rpm).
    if let Some(f) = mons.iter().flat_map(|h| (1..=4).map(move |i| h.join(format!("fan{i}_input")))).find(|f| read_trim(f).and_then(|v| v.parse::<u32>().ok()).is_some_and(|v| v > 0)) {
        p.fan = Some(f);
    }
    p.battery = hwmons(Path::new("/sys/class/power_supply")).into_iter().find(|d| read_trim(&d.join("type")).as_deref() == Some("Battery")).map(|d| d.join("capacity"));
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn history_is_a_ring() {
        let mut h = History::new(3);
        h.push(1.0);
        h.push(2.0);
        assert_eq!(h.normalized(2.0), [0.5, 1.0]);
        h.push(3.0);
        h.push(4.0);
        assert_eq!(h.normalized(1.0), [2.0, 3.0, 4.0]);
        assert_eq!(h.max(), 4.0);
    }

    #[test]
    fn parses_proc_files() {
        assert_eq!(meminfo("MemTotal: 1000 kB\nMemFree: 1 kB\nMemAvailable: 250 kB\n"), Some((750 * KIB, 1000 * KIB)));
        let dev = "Inter-|   Receive\n face |bytes\n    lo: 999 1 0 0 0 0 0 0 999 1 0 0 0 0 0 0\n  eth0: 100 1 0 0 0 0 0 0 40 1 0 0 0 0 0 0\n wlan0: 5 1 0 0 0 0 0 0 7 1 0 0 0 0 0 0\n";
        assert_eq!(net_bytes(dev), Some((105, 47)));
        assert_eq!(net_bytes("a\nb\n"), None);
    }

    #[test]
    fn swap_is_read_and_hidden_without_swap() {
        let text = "MemTotal: 1000 kB\nMemAvailable: 250 kB\nSwapTotal: 800 kB\nSwapFree: 600 kB\n";
        assert_eq!(swapinfo(text), Some((200 * KIB, 800 * KIB)));
        assert_eq!(swapinfo("SwapTotal: 0 kB\nSwapFree: 0 kB\n"), None, "no swap: no row");
        assert_eq!(swapinfo("MemTotal: 1 kB\n"), None);
    }

    fn gpu(slot: &str, load: Option<f32>, vram: Option<(u64, u64)>, temp: Option<f32>, power: Option<f32>) -> Gpu {
        Gpu {
            slot: slot.into(),
            vendor: crate::widgets::sensors::GpuVendor::Nvidia,
            name: None,
            integrated: false,
            load,
            vram_used: vram.map(|v| v.0),
            vram_total: vram.map(|v| v.1),
            temp_c: temp,
            freq_mhz: Some(1500.0),
            power_w: power,
        }
    }

    #[test]
    fn gpus_come_from_the_shared_reading_with_only_the_wanted_values() {
        let gpus = [gpu("a", Some(0.4), Some((1, 4)), Some(60.0), Some(120.0)), gpu("b", None, None, None, None), gpu("c", None, None, Some(45.0), None)];
        let mut wants = Wants::default();
        wants.add(&[MonitorRow::Gpu, MonitorRow::GpuTemp]);
        let mut out = Vec::with_capacity(4);
        let storage = out.as_ptr();
        fill_gpus(&mut out, &gpus, wants);
        // "b" reports nothing the widget wants; it gets no line.
        assert_eq!(out, [GpuReading { load: Some(0.4), vram: None, temp: Some(60.0), power_w: None }, GpuReading { temp: Some(45.0), ..Default::default() }]);
        fill_gpus(&mut out, &gpus[..1], wants);
        assert_eq!((out.len(), out.as_ptr()), (1, storage), "the storage is reused between samples");
        wants.add(&[MonitorRow::Vram, MonitorRow::GpuPower]);
        fill_gpus(&mut out, &gpus, wants);
        assert_eq!(out[0].vram, Some((1, 4)));
        assert_eq!(out[0].power_w, Some(120.0));
        assert_eq!(out[0].mask(), 0b1111);
        let mut only_vram = Wants::default();
        only_vram.add(&[MonitorRow::Vram]);
        fill_gpus(&mut out, &gpus, only_vram);
        assert_eq!(out.len(), 1, "only the card with VRAM figures");
        assert!(!Wants::default().any_gpu() && only_vram.any_gpu());
    }

    #[test]
    fn samples_only_what_is_shown() {
        let mut s = Sampler::new();
        let mut wants = Wants::default();
        wants.add(&[MonitorRow::Memory]);
        s.sample(wants, Instant::now());
        assert!(s.reading.cpu.is_none() && s.reading.down.is_none());
        assert_eq!(s.samples, 1);
        assert_eq!(s.cpu_history.normalized(1.0).len(), 0, "no CPU history without the row");
        assert!(s.reading.gpus.is_empty() && !s.reading.gpus_probed, "no GPU row: the GPU sampler isn't asked");
    }
}
