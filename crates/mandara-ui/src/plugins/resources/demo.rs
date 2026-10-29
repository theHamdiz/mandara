//! Fictional, calm readings for screenshots and demos.
//!
//! Off unless `MANDARA_RESOURCES_DEMO=1` is set in the shell's
//! environment. When on, the Resources panel, its bar chip and the
//! `system resources` command report these values instead of the
//! machine's real CPU, memory, core count, kernel, GPUs, sensors, disks
//! and uptime (the Process hunter's totals too), so docs shots never show
//! real hardware. Process lists and the hostname stay real (in a sandbox
//! they are the sandbox's own).

use std::f32::consts::TAU;
use std::sync::OnceLock;
use std::time::Instant;

use mandara_i18n::tk;

use super::Disk;
use crate::widgets::sensors::{Component, ComponentTemp, Gpu, GpuVendor, Label, Reading};

/// The environment variable that turns the demo source on.
pub const ENV: &str = "MANDARA_RESOURCES_DEMO";
/// The only value that turns it on.
const ENV_ON: &str = "1";

const GB: u64 = 1_000_000_000;
const MB: u64 = 1_000_000;

// ── CPU ──────────────────────────────────────────────────────────────────
/// Mean CPU busy fraction.
const CPU_BASE: f32 = 0.18;
/// Slow swell around the mean.
const CPU_SWAY: f32 = 0.035;
const CPU_SWAY_PERIOD_S: f32 = 47.0;
/// Faster, smaller ripple on top.
const CPU_RIPPLE: f32 = 0.015;
const CPU_RIPPLE_PERIOD_S: f32 = 11.0;
const CPU_RIPPLE_PHASE: f32 = 1.3;
/// Spacing of the pre-filled sparkline samples (the panel's history gap).
const HISTORY_STEP_S: f32 = 1.2;
const CPU_FREQ_GHZ: f32 = 3.6;
const CPU_TEMP_C: f32 = 52.0;
const CPU_HOTTEST_CORE_C: f32 = 58.0;
const LOAD_AVG: [f32; 3] = [1.12, 0.96, 0.88];
/// Logical CPUs.
const THREADS: usize = 16;
/// Kernel release shown next to the (sandbox's own) hostname.
pub const KERNEL: &str = "6.12.4";

// ── Memory ───────────────────────────────────────────────────────────────
const MEM_USED: u64 = 11_400 * MB;
const MEM_TOTAL: u64 = 32 * GB;
const SWAP_USED: u64 = 0;
const SWAP_TOTAL: u64 = 8 * GB;

// ── GPUs ─────────────────────────────────────────────────────────────────
struct DemoGpu {
    slot: &'static str,
    name: &'static str,
    integrated: bool,
    load: f32,
    sway: f32,
    period_s: f32,
    vram_used: u64,
    vram_total: u64,
    temp_c: f32,
    freq_mhz: f32,
}

const DISCRETE_GPU: DemoGpu = DemoGpu {
    slot: "0000:03:00.0",
    name: "Demo GPU",
    integrated: false,
    load: 0.12,
    sway: 0.03,
    period_s: 29.0,
    vram_used: 2_100 * MB,
    vram_total: 16 * GB,
    temp_c: 47.0,
    freq_mhz: 1_350.0,
};

const INTEGRATED_GPU: DemoGpu = DemoGpu {
    slot: "0000:0c:00.0",
    name: "Demo Graphics",
    integrated: true,
    load: 0.04,
    sway: 0.012,
    period_s: 37.0,
    vram_used: 310 * MB,
    vram_total: 2 * GB,
    temp_c: 44.0,
    freq_mhz: 600.0,
};

const GPUS: [&DemoGpu; 2] = [&DISCRETE_GPU, &INTEGRATED_GPU];

// ── Temperatures ─────────────────────────────────────────────────────────
const CPU_CHIP: &str = "demo-cpu";
const GPU_JUNCTION_C: f32 = 55.0;
const GPU_MEMORY_C: f32 = 50.0;
const GPU_CHIP: &str = "demo-gpu";
const NVME_NAME: &str = "Demo NVMe 2TB";
const NVME_TEMP_C: f32 = 41.0;
const NVME_CHIP: &str = "demo-nvme";
const BOARD_TEMP_C: f32 = 38.0;
const BOARD_LOWEST_C: f32 = 33.0;
const BOARD_CHIP: &str = "demo-board";

// ── Storage ──────────────────────────────────────────────────────────────
struct DemoDisk {
    mount: &'static str,
    label: Option<&'static str>,
    fstype: &'static str,
    total: u64,
    /// Used share, in percent.
    used_percent: u64,
}

const DISKS: [DemoDisk; 2] = [
    DemoDisk { mount: "/", label: None, fstype: "btrfs", total: 1_000 * GB, used_percent: 38 },
    DemoDisk { mount: "/home/demo/projects", label: Some("Projects"), fstype: "ext4", total: 2_000 * GB, used_percent: 51 },
];
const PERCENT: u64 = 100;

// ── Uptime ───────────────────────────────────────────────────────────────
const UPTIME_HOURS: u64 = 3;
const UPTIME_MINUTES: u64 = 12;
const SECS_PER_MINUTE: u64 = 60;
const SECS_PER_HOUR: u64 = 3_600;

/// Whether an env value turns the demo on (only exactly `1`).
pub fn enabled_by(value: Option<&std::ffi::OsStr>) -> bool {
    value.is_some_and(|v| v == ENV_ON)
}

/// Whether the demo source is on for this process (read once).
pub fn active() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| enabled_by(std::env::var_os(ENV).as_deref()))
}

/// Seconds since the demo clock started (first use).
fn now_s() -> f32 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f32()
}

fn wave(base: f32, sway: f32, period_s: f32, t: f32) -> f32 {
    base + sway * (TAU * t / period_s).sin()
}

/// CPU busy fraction at `t` seconds on the demo clock.
pub fn cpu_at(t: f32) -> f32 {
    wave(CPU_BASE, CPU_SWAY, CPU_SWAY_PERIOD_S, t) + CPU_RIPPLE * (TAU * t / CPU_RIPPLE_PERIOD_S + CPU_RIPPLE_PHASE).sin()
}

/// CPU busy fraction now.
pub fn cpu_now() -> f32 {
    cpu_at(now_s())
}

/// Memory used fraction.
pub fn mem_fraction() -> f32 {
    (MEM_USED as f64 / MEM_TOTAL as f64) as f32
}

/// `n` samples ending at `t = 0`, spaced like the panel's sparklines.
fn history(n: usize, f: impl Fn(f32) -> f32) -> Vec<f32> {
    (0..n).map(|i| f(-((n - 1 - i) as f32) * HISTORY_STEP_S)).collect()
}

/// A pre-filled CPU sparkline so the panel opens with a gentle history.
pub fn cpu_history(n: usize) -> Vec<f32> {
    history(n, cpu_at)
}

/// A flat memory sparkline.
pub fn mem_history(n: usize) -> Vec<f32> {
    vec![mem_fraction(); n]
}

/// A pre-filled load sparkline for a demo GPU slot.
pub fn gpu_history(slot: &str, n: usize) -> Option<Vec<f32>> {
    let g = GPUS.iter().find(|g| g.slot == slot)?;
    Some(history(n, |t| wave(g.load, g.sway, g.period_s, t)))
}

/// `(mem_used, mem_total, swap_used, swap_total)` in bytes.
pub fn meminfo() -> (u64, u64, u64, u64) {
    (MEM_USED, MEM_TOTAL, SWAP_USED, SWAP_TOTAL)
}

pub fn cpu_freq_ghz() -> f32 {
    CPU_FREQ_GHZ
}

pub fn cpu_temp_c() -> f32 {
    CPU_TEMP_C
}

pub fn loadavg() -> [f32; 3] {
    LOAD_AVG
}

pub fn threads() -> usize {
    THREADS
}

/// The GPUs, with load following the demo clock.
pub fn gpus() -> Vec<Gpu> {
    let t = now_s();
    GPUS.iter()
        .map(|g| Gpu {
            slot: g.slot.into(),
            vendor: GpuVendor::Other,
            name: Some(g.name.into()),
            integrated: g.integrated,
            load: Some(wave(g.load, g.sway, g.period_s, t)),
            vram_used: Some(g.vram_used),
            vram_total: Some(g.vram_total),
            temp_c: Some(g.temp_c),
            freq_mhz: Some(g.freq_mhz),
        })
        .collect()
}

fn temp(kind: Component, name: Label, device: Option<&str>, celsius: f32, readings: Vec<Reading>, chip: &str) -> ComponentTemp {
    let (warm, hot) = kind.thresholds();
    ComponentTemp { kind, name, device: device.map(Into::into), celsius, readings, warm, hot, chip: chip.into() }
}

/// CPU, GPU, NVMe and motherboard temperatures, all comfortably cool.
pub fn temperatures() -> Vec<ComponentTemp> {
    vec![
        temp(Component::Cpu, Label::Key(tk("CPU")), None, CPU_TEMP_C, vec![Reading { label: Label::Key(tk("Hottest core")), celsius: CPU_HOTTEST_CORE_C }], CPU_CHIP),
        temp(
            Component::Gpu,
            Label::Raw(DISCRETE_GPU.name.into()),
            None,
            DISCRETE_GPU.temp_c,
            vec![Reading { label: Label::Key(tk("Junction")), celsius: GPU_JUNCTION_C }, Reading { label: Label::Key(tk("Memory")), celsius: GPU_MEMORY_C }],
            GPU_CHIP,
        ),
        temp(Component::Storage, Label::Raw(NVME_NAME.into()), None, NVME_TEMP_C, Vec::new(), NVME_CHIP),
        temp(Component::Board, Label::Key(tk("Motherboard")), None, BOARD_TEMP_C, vec![Reading { label: Label::Key(tk("Lowest")), celsius: BOARD_LOWEST_C }], BOARD_CHIP),
    ]
}

pub fn disks() -> Vec<Disk> {
    DISKS
        .iter()
        .map(|d| Disk { mount: d.mount.into(), label: d.label.map(Into::into), fstype: d.fstype.into(), used: d.total / PERCENT * d.used_percent, total: d.total })
        .collect()
}

/// Uptime in seconds.
pub fn uptime_secs() -> u64 {
    UPTIME_HOURS * SECS_PER_HOUR + UPTIME_MINUTES * SECS_PER_MINUTE
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn off_by_default() {
        assert!(!enabled_by(None));
        assert!(!enabled_by(Some(OsStr::new(""))));
        assert!(!enabled_by(Some(OsStr::new("0"))));
        assert!(!enabled_by(Some(OsStr::new("yes"))));
        assert!(enabled_by(Some(OsStr::new("1"))));
        if std::env::var_os(ENV).is_none() {
            assert!(!active());
        }
    }

    #[test]
    fn readings_are_stable_and_in_range() {
        // Same instant, same reading.
        assert_eq!(cpu_at(12.5), cpu_at(12.5));
        assert_eq!(cpu_history(36), cpu_history(36));
        let hist = cpu_history(36);
        assert_eq!(hist.len(), 36);
        assert_eq!(hist.last().copied(), Some(cpu_at(0.0)));
        for t in (0..600).map(|s| s as f32 * 0.7) {
            let c = cpu_at(t);
            assert!((0.12..=0.24).contains(&c), "cpu {c} at {t}");
        }
        let (used, total, swap_used, swap_total) = meminfo();
        assert!(used < total && swap_used <= swap_total);
        assert!((mem_fraction() - 0.356).abs() < 0.001);
        assert!(mem_history(4).iter().all(|&m| m == mem_fraction()));

        let gpus = gpus();
        assert_eq!(gpus.len(), 2);
        assert_eq!(gpus.iter().filter(|g| g.integrated).count(), 1);
        for g in &gpus {
            let load = g.load.unwrap();
            assert!((0.0..=0.2).contains(&load), "gpu load {load}");
            assert!(g.vram_used.unwrap() < g.vram_total.unwrap() / 4);
            let (warm, _) = Component::Gpu.thresholds();
            assert!(g.temp_c.unwrap() < warm);
            assert_eq!(gpu_history(&g.slot, 8).map(|h| h.len()), Some(8));
        }
        assert!(gpu_history("0000:ff:00.0", 8).is_none());

        let temps = temperatures();
        assert_eq!(temps.len(), 4);
        for c in &temps {
            assert!(c.celsius > 25.0 && c.celsius < c.warm, "{:?} {}", c.kind, c.celsius);
            assert!(c.readings.iter().all(|r| r.celsius < c.warm));
        }
        assert_eq!(crate::widgets::sensors::cpu_temperature(&temps), Some(cpu_temp_c()));

        let disks = disks();
        assert_eq!(disks.len(), 2);
        assert_eq!(disks[0].mount, "/");
        let percent: Vec<u64> = disks.iter().map(|d| d.used * 100 / d.total).collect();
        assert_eq!(percent, vec![38, 51]);
        assert_eq!(disks[1].label.as_deref(), Some("Projects"));

        assert_eq!(uptime_secs(), 3 * 3600 + 12 * 60);
        assert!(threads() > 0 && !KERNEL.is_empty());
        assert!(cpu_freq_ghz() > 0.0 && loadavg().iter().all(|&l| l > 0.0 && l < 2.0));
    }
}
