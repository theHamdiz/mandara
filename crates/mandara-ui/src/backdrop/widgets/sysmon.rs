//! Readings for the desktop system monitor, sampled on the wallpaper
//! thread only while a monitor widget shows on a visible display (the
//! renderer drops the sampler otherwise). Every reading is a handful of
//! small `/proc` and `/sys` files; sensor paths are found once and cached.
//! `MANDARA_RESOURCES_DEMO=1` swaps them for calm fictional values (docs
//! screenshots), like the resources panel.

use std::path::{Path, PathBuf};
use std::time::Instant;

use mandara_config::desktop_widgets::MonitorRow;

use crate::widgets::system::{CpuTimes, cpu_usage, disk_usage, read_cpu_times};

/// Samples kept for the graphs.
pub const HISTORY: usize = 48;
/// hwmon reports milli-degrees, micro-watts.
const MILLI: f32 = 1000.0;
const MICRO: f32 = 1_000_000.0;
/// Kibibyte (`/proc/meminfo` unit).
const KIB: u64 = 1024;
/// CPU sensor drivers, best first.
const CPU_SENSORS: &[&str] = &["k10temp", "zenpower", "coretemp", "cpu_thermal", "acpitz"];
/// The filesystem the disk row measures.
const DISK_PATH: &str = "/";
/// Demo readings (fictional).
const DEMO_CPU: f32 = 0.14;
const DEMO_GPU: f32 = 0.08;
const DEMO_GPU_POWER_W: f32 = 23.0;
const DEMO_CPU_TEMP: f32 = 47.0;
const DEMO_GPU_TEMP: f32 = 41.0;
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

/// One sample.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Reading {
    /// Busy fractions.
    pub cpu: Option<f32>,
    pub gpu: Option<f32>,
    /// `(used, total)` bytes.
    pub memory: Option<(u64, u64)>,
    pub disk: Option<(u64, u64)>,
    pub gpu_power_w: Option<f32>,
    pub cpu_temp: Option<f32>,
    pub gpu_temp: Option<f32>,
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
    pub memory: bool,
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
                MonitorRow::Memory => self.memory = true,
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
}

/// Sensor paths, found on first use.
#[derive(Default)]
struct Paths {
    found: bool,
    gpu_busy: Option<PathBuf>,
    gpu_temp: Option<PathBuf>,
    gpu_power: Option<PathBuf>,
    cpu_temp: Option<PathBuf>,
    fan: Option<PathBuf>,
    battery: Option<PathBuf>,
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
            samples: 0,
        }
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
        if wants.memory {
            r.memory = std::fs::read_to_string("/proc/meminfo").ok().and_then(|t| meminfo(&t));
        }
        if wants.disk {
            r.disk = disk_usage(DISK_PATH);
        }
        let read_f = |p: &Option<PathBuf>| p.as_ref().and_then(|p| std::fs::read_to_string(p).ok()).and_then(|s| s.trim().parse::<f32>().ok());
        if wants.gpu {
            r.gpu = read_f(&self.paths.gpu_busy).map(|v| v / 100.0);
        }
        if wants.gpu_temp {
            r.gpu_temp = read_f(&self.paths.gpu_temp).map(|v| v / MILLI);
        }
        if wants.gpu_power {
            r.gpu_power_w = read_f(&self.paths.gpu_power).map(|v| v / MICRO);
        }
        if wants.cpu_temp {
            r.cpu_temp = read_f(&self.paths.cpu_temp).map(|v| v / MILLI);
        }
        if wants.fan {
            r.fan_rpm = read_f(&self.paths.fan).map(|v| v.max(0.0) as u32);
        }
        if wants.battery {
            r.battery = read_f(&self.paths.battery).map(|v| v / 100.0);
        }
        if wants.net {
            let next = std::fs::read_to_string("/proc/net/dev").ok().and_then(|t| net_bytes(&t));
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
    }

    fn demo_sample(&mut self, wants: Wants) {
        let wobble = (self.samples as f32 * 0.7).sin() * DEMO_WAVE;
        let (used, total, _, _) = crate::plugins::resources::demo::meminfo();
        let r = &mut self.reading;
        r.cpu = wants.cpu.then_some((DEMO_CPU + wobble).max(0.0));
        r.gpu = wants.gpu.then_some((DEMO_GPU + wobble / 2.0).max(0.0));
        r.memory = wants.memory.then_some((used, total));
        r.disk = wants.disk.then_some(DEMO_DISK);
        r.gpu_power_w = wants.gpu_power.then_some(DEMO_GPU_POWER_W);
        r.cpu_temp = wants.cpu_temp.then_some(DEMO_CPU_TEMP);
        r.gpu_temp = wants.gpu_temp.then_some(DEMO_GPU_TEMP);
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

/// `(used, total)` from `/proc/meminfo`.
pub fn meminfo(text: &str) -> Option<(u64, u64)> {
    let field = |name: &str| text.lines().find(|l| l.starts_with(name)).and_then(|l| l.split_whitespace().nth(1)?.parse::<u64>().ok()).map(|v| v * KIB);
    let total = field("MemTotal:")?;
    let avail = field("MemAvailable:")?;
    Some((total.saturating_sub(avail), total))
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
    // The discrete GPU (it has VRAM), else the first one reporting busy.
    let mut cards: Vec<PathBuf> = hwmons(Path::new("/sys/class/drm")).into_iter().filter(|c| c.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.starts_with("card") && !n.contains('-'))).collect();
    cards.sort_by_key(|c| !c.join("device/mem_info_vram_total").exists());
    if let Some(dev) = cards.iter().map(|c| c.join("device")).find(|d| d.join("gpu_busy_percent").exists()) {
        p.gpu_busy = Some(dev.join("gpu_busy_percent"));
        if let Some(h) = hwmons(&dev.join("hwmon")).into_iter().next() {
            p.gpu_temp = Some(h.join("temp1_input")).filter(|f| f.exists());
            p.gpu_power = [h.join("power1_average"), h.join("power1_input")].into_iter().find(|f| f.exists());
            if p.fan.is_none() {
                p.fan = Some(h.join("fan1_input")).filter(|f| f.exists());
            }
        }
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
    fn samples_only_what_is_shown() {
        let mut s = Sampler::new();
        let mut wants = Wants::default();
        wants.add(&[MonitorRow::Memory]);
        s.sample(wants, Instant::now());
        assert!(s.reading.cpu.is_none() && s.reading.down.is_none());
        assert_eq!(s.samples, 1);
        assert_eq!(s.cpu_history.normalized(1.0).len(), 0, "no CPU history without the row");
    }
}
