//! CPU and memory load from procfs.

use std::fs;

/// Aggregate CPU jiffies from the first line of `/proc/stat`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub busy: u64,
    pub total: u64,
}

pub fn parse_cpu_times(stat: &str) -> Option<CpuTimes> {
    let line = stat.lines().find(|l| l.starts_with("cpu "))?;
    let fields: Vec<u64> = line.split_whitespace().skip(1).filter_map(|f| f.parse().ok()).collect();
    if fields.len() < 4 {
        return None;
    }
    // user nice system idle iowait irq softirq steal ...
    let idle = fields[3] + fields.get(4).copied().unwrap_or(0);
    let total: u64 = fields.iter().take(8).sum();
    Some(CpuTimes { busy: total.saturating_sub(idle), total })
}

/// Busy fraction between two samples, `0..=1`.
pub fn cpu_usage(prev: CpuTimes, next: CpuTimes) -> f32 {
    let total = next.total.saturating_sub(prev.total);
    if total == 0 {
        return 0.0;
    }
    (next.busy.saturating_sub(prev.busy) as f32 / total as f32).clamp(0.0, 1.0)
}

/// Used memory fraction from `/proc/meminfo` text.
pub fn parse_mem_usage(meminfo: &str) -> Option<f32> {
    let field = |name: &str| -> Option<u64> {
        meminfo
            .lines()
            .find(|l| l.starts_with(name))?
            .split_whitespace()
            .nth(1)?
            .parse()
            .ok()
    };
    let total = field("MemTotal:")?;
    let available = field("MemAvailable:")?;
    if total == 0 {
        return None;
    }
    Some(1.0 - available as f32 / total as f32)
}

pub fn read_cpu_times() -> Option<CpuTimes> {
    parse_cpu_times(&fs::read_to_string("/proc/stat").ok()?)
}

pub fn read_mem_usage() -> Option<f32> {
    parse_mem_usage(&fs::read_to_string("/proc/meminfo").ok()?)
}

/// `(used, total)` bytes of the filesystem containing `path`.
pub fn disk_usage(path: &str) -> Option<(u64, u64)> {
    let st = rustix::fs::statvfs(path).ok()?;
    let block = st.f_frsize.max(1);
    let total = st.f_blocks * block;
    // `f_bavail` is what unprivileged users can still write.
    let used = total.saturating_sub(st.f_bavail * block);
    (total > 0).then_some((used, total))
}

/// Human-readable byte size with one decimal (`1.4 TB`).
pub fn format_bytes(bytes: u64) -> String {
    use mandara_i18n::tk;
    const UNITS: [&str; 5] = [tk("B"), tk("KB"), tk("MB"), tk("GB"), tk("TB")];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1000.0 && unit < UNITS.len() - 1 {
        value /= 1000.0;
        unit += 1;
    }
    let unit_name = mandara_i18n::tr(UNITS[unit]);
    if unit == 0 { format!("{bytes} {unit_name}") } else { format!("{value:.1} {unit_name}") }
}

/// System uptime as `3d 4h` / `5h 12m` / `7m`.
pub fn uptime() -> Option<String> {
    let secs: f64 = fs::read_to_string("/proc/uptime").ok()?.split_whitespace().next()?.parse().ok()?;
    Some(format_uptime(secs as u64))
}

/// `secs` as `3d 4h` / `5h 12m` / `7m`.
pub fn format_uptime(secs: u64) -> String {
    let (d, h, m) = (secs / 86_400, (secs / 3600) % 24, (secs / 60) % 60);
    use mandara_i18n::tf;
    if d > 0 {
        tf("{days}d {hours}h", &[("days", &d), ("hours", &h)])
    } else if h > 0 {
        tf("{hours}h {minutes}m", &[("hours", &h), ("minutes", &m)])
    } else {
        tf("{minutes}m", &[("minutes", &m)])
    }
}

/// Rolling system sampler with exponential smoothing, so readouts move
/// calmly instead of jittering with every scheduler blip.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SystemSampler {
    last_cpu: Option<CpuTimes>,
    pub cpu: f32,
    pub mem: f32,
    primed: bool,
}

/// Smoothing factor per sample (higher = more responsive).
const EMA_ALPHA: f32 = 0.4;

impl SystemSampler {
    pub fn sample(&mut self) {
        if crate::plugins::resources::demo::active() {
            self.cpu = crate::plugins::resources::demo::cpu_now();
            self.mem = crate::plugins::resources::demo::mem_fraction();
            self.primed = true;
            return;
        }
        if let Some(now) = read_cpu_times() {
            if let Some(prev) = self.last_cpu {
                let raw = cpu_usage(prev, now);
                self.cpu = if self.primed { self.cpu + (raw - self.cpu) * EMA_ALPHA } else { raw };
                self.primed = true;
            }
            self.last_cpu = Some(now);
        }
        if let Some(mem) = read_mem_usage() {
            self.mem = mem;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_usage_from_two_samples() {
        let a = parse_cpu_times("cpu  100 0 100 800 0 0 0 0 0 0\ncpu0 1 2 3 4").unwrap();
        let b = parse_cpu_times("cpu  150 0 150 900 0 0 0 0 0 0\n").unwrap();
        assert_eq!(a, CpuTimes { busy: 200, total: 1000 });
        assert!((cpu_usage(a, b) - 0.5).abs() < 1e-6);
        assert_eq!(cpu_usage(b, b), 0.0);
    }

    #[test]
    fn mem_usage() {
        let text = "MemTotal:       16000000 kB\nMemFree:  100 kB\nMemAvailable:    4000000 kB\n";
        assert!((parse_mem_usage(text).unwrap() - 0.75).abs() < 1e-6);
        assert!(parse_mem_usage("garbage").is_none());
    }

    #[test]
    fn bytes_and_disks() {
        assert_eq!(format_bytes(999), "999 B");
        assert_eq!(format_bytes(1_400_000_000_000), "1.4 TB");
        let (used, total) = disk_usage("/").unwrap();
        assert!(used <= total && total > 0);
        assert!(uptime().is_some());
        assert_eq!(format_uptime(3 * 3600 + 12 * 60), "3h 12m");
        assert_eq!(format_uptime(7 * 60), "7m");
    }

    #[test]
    fn smoothing_damps_spikes() {
        let mut s = SystemSampler { primed: true, cpu: 0.10, ..Default::default() };
        // A single 100% spike only moves the readout part of the way.
        let raw = 1.0;
        s.cpu += (raw - s.cpu) * EMA_ALPHA;
        assert!((s.cpu - 0.46).abs() < 1e-4);
    }

    #[test]
    fn live_procfs_is_readable() {
        assert!(read_cpu_times().is_some());
        let mem = read_mem_usage().unwrap();
        assert!((0.0..=1.0).contains(&mem));
    }
}
