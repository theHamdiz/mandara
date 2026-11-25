//! Host-wide figures from FreeBSD's sysctl tree: memory and swap
//! (`vm.stats.vm.*`, `vm.swap_info`), CPU time (`kern.cp_time`,
//! `kern.cp_times`), load, uptime (`kern.boottime`), names
//! (`kern.hostname`, `kern.osrelease`, `hw.model`), clock speed
//! (`dev.cpu.0.freq`) and CPU temperatures (`dev.cpu.N.temperature`, from
//! coretemp(4) or amdtemp(4)).
//!
//! The decoders are plain functions over the numbers the kernel returns,
//! so they are tested on every platform; the reads are FreeBSD-only.

/// `kern.cp_time` slots: user, nice, system, interrupt, idle (`CPUSTATES`).
pub const CPUSTATES: usize = 5;
/// `CP_IDLE`: the idle slot of a `kern.cp_time` row.
const CP_IDLE: usize = 4;
/// `struct xswdev` version the decoder understands (`XSWDEV_VERSION`).
const XSWDEV_VERSION: u32 = 2;
/// Byte offsets in `struct xswdev` on 64-bit FreeBSD 12+: `u_int version`,
/// padding, `dev_t dev` (64-bit), `int flags`, `int nblks`, `int used`.
const XSW_NBLKS: usize = 20;
const XSW_USED: usize = 24;
/// Bytes of `struct xswdev` the decoder reads.
const XSW_MIN_BYTES: usize = 28;
/// Tenths of a kelvin per kelvin (`IK` sysctl format).
const DECI: f32 = 10.0;
/// 0 °C in kelvin.
const ZERO_C_KELVIN: f32 = 273.15;
/// Highest CPU index probed for a temperature (`dev.cpu.N`); the probe
/// stops at the first missing CPU.
#[cfg(target_os = "freebsd")]
const MAX_TEMP_CPUS: usize = 1024;

/// Physical memory and swap, in bytes.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Memory {
    pub total: u64,
    /// Free, inactive and (pre-13) cache pages: what the kernel hands out
    /// without paging anything, like Linux's `MemAvailable`.
    pub available: u64,
    pub swap_total: u64,
    pub swap_used: u64,
}

impl Memory {
    pub fn used(&self) -> u64 {
        self.total.saturating_sub(self.available)
    }

    /// Used share of physical memory, `0..=1`.
    pub fn used_fraction(&self) -> Option<f32> {
        (self.total > 0).then(|| (self.used() as f64 / self.total as f64) as f32)
    }
}

/// Page counts (`vm.stats.vm.v_*_count`) as bytes.
pub fn memory_from_pages(page_size: u64, page_count: u64, free: u64, inactive: u64, cache: u64) -> Memory {
    let total = page_count.saturating_mul(page_size);
    let available = free.saturating_add(inactive).saturating_add(cache).saturating_mul(page_size).min(total);
    Memory { total, available, ..Memory::default() }
}

/// `(blocks, used)` in pages from one `vm.swap_info.N` record.
pub fn decode_xswdev(bytes: &[u8]) -> Option<(u64, u64)> {
    if bytes.len() < XSW_MIN_BYTES || u32::from_ne_bytes(bytes[..4].try_into().ok()?) != XSWDEV_VERSION {
        return None;
    }
    let int = |at: usize| -> Option<u64> { u64::try_from(i32::from_ne_bytes(bytes[at..at + 4].try_into().ok()?)).ok() };
    Some((int(XSW_NBLKS)?, int(XSW_USED)?))
}

/// Busy and total ticks of one CPU-state row (user, nice, system,
/// interrupt, idle).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Ticks {
    pub busy: u64,
    pub total: u64,
}

pub fn ticks(row: &[u64]) -> Ticks {
    let total = row.iter().take(CPUSTATES).sum();
    let idle = row.get(CP_IDLE).copied().unwrap_or(0);
    Ticks { busy: total - idle.min(total), total }
}

/// `long` values of a `kern.cp_time(s)` buffer (native-endian, 64-bit).
pub fn longs(bytes: &[u8]) -> impl Iterator<Item = u64> + '_ {
    bytes.as_chunks::<{ std::mem::size_of::<i64>() }>().0.iter().map(|c| i64::from_ne_bytes(*c).max(0) as u64)
}

/// An `IK` sysctl value (tenths of a kelvin) in °C.
pub fn deci_kelvin_to_celsius(v: i32) -> f32 {
    v as f32 / DECI - ZERO_C_KELVIN
}

/// `FreeBSD 14.4` from `kern.ostype` and `kern.osrelease`
/// (`14.4-RELEASE-p2` → `14.4`).
pub fn system_name(ostype: &str, release: &str) -> String {
    let version = release.split('-').next().unwrap_or_default();
    match (ostype.is_empty(), version.is_empty()) {
        (false, false) => format!("{ostype} {version}"),
        (false, true) => ostype.to_string(),
        _ => version.to_string(),
    }
}

#[cfg(target_os = "freebsd")]
pub use sys::*;

#[cfg(target_os = "freebsd")]
mod sys {
    use super::*;
    use crate::freebsd::sysctl;

    /// Physical memory and swap.
    pub fn memory() -> Option<Memory> {
        let page = sysctl::number(c"hw.pagesize")?;
        let pages = |name: &std::ffi::CStr| sysctl::number(name).unwrap_or(0);
        let mut m = memory_from_pages(page, sysctl::number(c"vm.stats.vm.v_page_count")?, pages(c"vm.stats.vm.v_free_count"), pages(c"vm.stats.vm.v_inactive_count"), pages(c"vm.stats.vm.v_cache_count"));
        let (total, used) = swap_pages();
        m.swap_total = total.saturating_mul(page);
        m.swap_used = used.saturating_mul(page);
        Some(m)
    }

    /// Total and used swap pages over every `vm.swap_info.N` device.
    fn swap_pages() -> (u64, u64) {
        let mut mib = [0 as libc::c_int; 3];
        let mut n = mib.len() - 1;
        // SAFETY: `mib` has room for `n` ints.
        if unsafe { libc::sysctlnametomib(c"vm.swap_info".as_ptr(), mib.as_mut_ptr(), &mut n) } != 0 {
            return (0, 0);
        }
        let (mut total, mut used) = (0, 0);
        for index in 0.. {
            mib[n] = index;
            let Some((blocks, in_use)) = sysctl::bytes(&mib[..=n]).as_deref().and_then(decode_xswdev) else { break };
            total += blocks;
            used += in_use;
        }
        (total, used)
    }

    /// Ticks of the whole machine (`kern.cp_time`).
    pub fn cpu_ticks() -> Option<Ticks> {
        let row = sysctl::value::<[libc::c_long; CPUSTATES]>(c"kern.cp_time")?;
        Some(ticks(&row.map(|v| v.max(0) as u64)))
    }

    /// Ticks of each CPU (`kern.cp_times`).
    pub fn per_cpu_ticks() -> Vec<Ticks> {
        let Some(bytes) = sysctl::bytes_by_name(c"kern.cp_times") else { return Vec::new() };
        let values: Vec<u64> = longs(&bytes).collect();
        values.as_chunks::<CPUSTATES>().0.iter().map(|row| ticks(row)).collect()
    }

    /// Online CPUs (`kern.smp.cpus`, else `hw.ncpu`).
    pub fn cpu_count() -> Option<usize> {
        sysctl::int(c"kern.smp.cpus").or_else(|| sysctl::int(c"hw.ncpu")).and_then(|n| usize::try_from(n).ok()).filter(|n| *n > 0)
    }

    /// 1, 5 and 15 minute load averages.
    pub fn loadavg() -> [f32; 3] {
        let mut out = [0f64; 3];
        // SAFETY: `out` has room for the three values asked for.
        let n = unsafe { libc::getloadavg(out.as_mut_ptr(), 3) };
        if n == 3 { out.map(|v| v as f32) } else { [0.0; 3] }
    }

    /// Seconds since boot (`kern.boottime` against the wall clock).
    pub fn uptime_secs() -> Option<u64> {
        let boot = sysctl::value::<libc::timeval>(c"kern.boottime")?;
        let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).ok()?.as_secs();
        now.checked_sub(u64::try_from(boot.tv_sec).ok()?)
    }

    pub fn hostname() -> String {
        sysctl::string(c"kern.hostname").unwrap_or_default()
    }

    /// `kern.osrelease`, e.g. `14.4-RELEASE`.
    pub fn os_release() -> String {
        sysctl::string(c"kern.osrelease").unwrap_or_default()
    }

    /// `FreeBSD 14.4`.
    pub fn system() -> String {
        system_name(&sysctl::string(c"kern.ostype").unwrap_or_else(|| "FreeBSD".into()), &os_release())
    }

    /// The processor's marketing name (`hw.model`).
    pub fn cpu_model() -> String {
        sysctl::string(c"hw.model").unwrap_or_default()
    }

    /// The current clock of CPU 0 in GHz (`dev.cpu.0.freq`, in MHz; only
    /// with cpufreq(4)).
    pub fn cpu_freq_ghz() -> Option<f32> {
        const MHZ_PER_GHZ: f32 = 1000.0;
        sysctl::int(c"dev.cpu.0.freq").filter(|mhz| *mhz > 0).map(|mhz| mhz as f32 / MHZ_PER_GHZ)
    }

    /// Each CPU's temperature in °C, in CPU order; empty unless coretemp(4)
    /// or amdtemp(4) is loaded.
    pub fn cpu_temperatures() -> Vec<f32> {
        let mut out = Vec::new();
        let mut name = String::new();
        for cpu in 0..MAX_TEMP_CPUS {
            use std::fmt::Write as _;
            name.clear();
            let _ = write!(name, "dev.cpu.{cpu}.temperature\0");
            let Ok(c) = std::ffi::CStr::from_bytes_with_nul(name.as_bytes()) else { break };
            match sysctl::int(c) {
                Some(v) => out.push(deci_kelvin_to_celsius(v)),
                None => break,
            }
        }
        out
    }

    /// The hottest CPU reading.
    pub fn cpu_temperature() -> Option<f32> {
        cpu_temperatures().into_iter().reduce(f32::max)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recorded on the FreeBSD 14.4 test machine (16 GiB, 4 KiB pages).
    #[test]
    fn memory_counts_free_inactive_and_cache_as_available() {
        let m = memory_from_pages(4096, 4_065_141, 88_120, 1_205_243, 0);
        assert_eq!(m.total, 4_065_141 * 4096);
        assert_eq!(m.available, (88_120 + 1_205_243) * 4096);
        let used = m.used_fraction().unwrap();
        assert!((used - 0.6818).abs() < 0.001, "{used}");
        assert_eq!(memory_from_pages(4096, 0, 0, 0, 0).used_fraction(), None);
        // Counters sampled a moment apart can't report more than all.
        assert_eq!(memory_from_pages(4096, 10, 8, 8, 0).used(), 0);
    }

    #[test]
    fn swap_records_decode_by_version() {
        let mut rec = [0u8; 32];
        rec[..4].copy_from_slice(&2u32.to_ne_bytes());
        rec[XSW_NBLKS..XSW_NBLKS + 4].copy_from_slice(&262_144i32.to_ne_bytes());
        rec[XSW_USED..XSW_USED + 4].copy_from_slice(&14_994i32.to_ne_bytes());
        assert_eq!(decode_xswdev(&rec), Some((262_144, 14_994)));
        rec[..4].copy_from_slice(&1u32.to_ne_bytes());
        assert_eq!(decode_xswdev(&rec), None, "an unknown layout is never guessed at");
        assert_eq!(decode_xswdev(&rec[..8]), None);
    }

    /// `sysctl kern.cp_time kern.cp_times` on the test machine.
    #[test]
    fn cpu_time_rows() {
        let total = ticks(&[2_173_107, 0, 193_615, 14_818, 4_511_908]);
        assert_eq!(total, Ticks { busy: 2_173_107 + 193_615 + 14_818, total: 2_173_107 + 193_615 + 14_818 + 4_511_908 });
        let raw: Vec<u8> = [191_206i64, 0, 15_219, 468, 367_561, 167_583, 0, 23_225, 423, 383_223].iter().flat_map(|v| v.to_ne_bytes()).collect();
        let values: Vec<u64> = longs(&raw).collect();
        let per_cpu: Vec<Ticks> = values.as_chunks::<CPUSTATES>().0.iter().map(|row| ticks(row)).collect();
        assert_eq!(per_cpu.len(), 2);
        assert_eq!(per_cpu[1], Ticks { busy: 167_583 + 23_225 + 423, total: 167_583 + 23_225 + 423 + 383_223 });
        assert_eq!(ticks(&[]), Ticks::default());
    }

    /// The live sysctl tree (FreeBSD only).
    #[test]
    #[cfg(target_os = "freebsd")]
    fn reads_the_live_system() {
        let m = memory().expect("vm.stats.vm");
        assert!(m.total > 0 && m.available <= m.total);
        let a = cpu_ticks().expect("kern.cp_time");
        std::thread::sleep(std::time::Duration::from_millis(300));
        let b = cpu_ticks().unwrap();
        assert!(b.total > a.total, "kern.cp_time advances: {a:?} {b:?}");
        assert_eq!(per_cpu_ticks().len(), cpu_count().unwrap());
        assert!(uptime_secs().unwrap() > 0);
        assert!(!hostname().is_empty() && system().starts_with("FreeBSD "));
        assert!(!cpu_model().is_empty());
        assert!(loadavg().iter().all(|l| *l >= 0.0));
    }

    #[test]
    fn temperatures_and_names() {
        // coretemp reports 3231 for 50.0 °C (`IK` = tenths of a kelvin).
        assert!((deci_kelvin_to_celsius(3231) - 49.95).abs() < 0.01);
        assert_eq!(system_name("FreeBSD", "14.4-RELEASE"), "FreeBSD 14.4");
        assert_eq!(system_name("FreeBSD", "15.0-CURRENT"), "FreeBSD 15.0");
        assert_eq!(system_name("FreeBSD", ""), "FreeBSD");
    }
}
