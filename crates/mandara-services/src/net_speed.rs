//! Network throughput: per-interface byte counters from `/proc/net/dev`,
//! rates between samples, local addresses (`getifaddrs`), the public
//! address (one HTTPS request to a configurable endpoint) and a download
//! speed test against Cloudflare's speed endpoint.
//!
//! Nothing here runs on its own: the UI samples counters only while the bar
//! chip or the panel is on screen, fetches the public address when the
//! panel opens, and runs the speed test only from an explicit button.

use std::collections::BTreeMap;
use std::io::Read;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use mandara_i18n::{t, tf};

/// Default endpoint answering with the caller's public IP as plain text.
pub const DEFAULT_PUBLIC_IP_URL: &str = "https://api.ipify.org";
/// Cloudflare's speed test download endpoint (`?bytes=N`).
pub const SPEED_TEST_URL: &str = "https://speed.cloudflare.com/__down";
/// Default speed test download size.
pub const DEFAULT_TEST_MEGABYTES: u64 = 25;
/// Largest download a speed test may request.
pub const MAX_TEST_MEGABYTES: u64 = 200;
const BYTES_PER_MEGABYTE: u64 = 1_000_000;
/// Timeout for the public IP lookup.
pub const PUBLIC_IP_TIMEOUT: Duration = Duration::from_secs(6);
/// Connect timeout of the speed test (the transfer itself is bounded by
/// [`SPEED_TEST_TIMEOUT`]).
const SPEED_TEST_CONNECT_TIMEOUT: Duration = Duration::from_secs(8);
/// A speed test gives up after this long.
pub const SPEED_TEST_TIMEOUT: Duration = Duration::from_secs(60);
/// Read buffer of the speed test.
const READ_CHUNK: usize = 64 * 1024;
/// Longest public-IP reply accepted.
const MAX_IP_REPLY: u64 = 128;

/// Raw counters of one interface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Counters {
    pub name: String,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

/// Parses `/proc/net/dev` (skipping the two header lines and loopback).
pub fn parse_proc_net_dev(text: &str) -> Vec<Counters> {
    text.lines()
        .skip(2)
        .filter_map(|line| {
            let (name, rest) = line.split_once(':')?;
            let name = name.trim();
            if name == "lo" || name.is_empty() {
                return None;
            }
            let fields: Vec<u64> = rest.split_whitespace().filter_map(|f| f.parse().ok()).collect();
            // rx: bytes packets errs drop fifo frame compressed multicast; tx: bytes …
            (fields.len() >= 9).then(|| Counters { name: name.to_string(), rx_bytes: fields[0], tx_bytes: fields[8] })
        })
        .collect()
}

pub fn read_counters() -> Vec<Counters> {
    parse_proc_net_dev(&std::fs::read_to_string("/proc/net/dev").unwrap_or_default())
}

/// What kind of link an interface is (for icons and for which interfaces
/// count toward the total).
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LinkKind {
    Wifi,
    Ethernet,
    /// WireGuard / tun / tap / ppp: traffic also crosses a physical link.
    Tunnel,
    /// Bridges, veth pairs, container networks.
    Virtual,
    Other,
}

impl LinkKind {
    /// Classifies by name, refined by sysfs when available.
    pub fn classify(name: &str) -> Self {
        let sys = std::path::Path::new("/sys/class/net").join(name);
        if sys.join("wireless").exists() || sys.join("phy80211").exists() {
            return LinkKind::Wifi;
        }
        let by_name = Self::from_name(name);
        if by_name == LinkKind::Other && sys.join("device").exists() {
            return LinkKind::Ethernet;
        }
        by_name
    }

    pub fn from_name(name: &str) -> Self {
        const VIRTUAL: &[&str] = &["veth", "docker", "br-", "virbr", "vnet", "lxc", "cni", "flannel", "podman", "vmnet", "dummy", "bond", "ifb"];
        const TUNNEL: &[&str] = &["wg", "tun", "tap", "ppp", "tailscale", "zt", "nordlynx", "proton", "mullvad", "vpn"];
        if name.starts_with("wl") {
            LinkKind::Wifi
        } else if name.starts_with("en") || name.starts_with("eth") {
            LinkKind::Ethernet
        } else if TUNNEL.iter().any(|p| name.starts_with(p)) {
            LinkKind::Tunnel
        } else if VIRTUAL.iter().any(|p| name.starts_with(p)) {
            LinkKind::Virtual
        } else {
            LinkKind::Other
        }
    }

    /// Physical links make up the machine's total (tunnels and bridges
    /// would count the same bytes twice).
    pub fn counts_toward_total(self) -> bool {
        matches!(self, LinkKind::Wifi | LinkKind::Ethernet | LinkKind::Other)
    }
}

/// Throughput of one interface between two samples.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct IfaceRate {
    pub name: String,
    pub kind: LinkKind,
    /// Bytes per second.
    pub rx: f64,
    pub tx: f64,
    /// Counters at the newer sample (bytes since the link came up).
    pub rx_total: u64,
    pub tx_total: u64,
}

/// A counter snapshot with its timestamp.
#[derive(Clone, Debug)]
pub struct Sample {
    pub at: Instant,
    pub counters: Vec<Counters>,
}

impl Sample {
    pub fn now() -> Self {
        Self { at: Instant::now(), counters: read_counters() }
    }
}

/// Rates per interface between `old` and `new` (counter resets read as 0).
pub fn rates(old: &Sample, new: &Sample, kind: impl Fn(&str) -> LinkKind) -> Vec<IfaceRate> {
    let dt = new.at.saturating_duration_since(old.at).as_secs_f64();
    if dt <= 0.0 {
        return Vec::new();
    }
    new.counters
        .iter()
        .map(|c| {
            let prev = old.counters.iter().find(|o| o.name == c.name);
            let delta = |now: u64, then: Option<u64>| then.map(|t| now.saturating_sub(t) as f64 / dt).unwrap_or(0.0);
            IfaceRate {
                name: c.name.clone(),
                kind: kind(&c.name),
                rx: delta(c.rx_bytes, prev.map(|p| p.rx_bytes)),
                tx: delta(c.tx_bytes, prev.map(|p| p.tx_bytes)),
                rx_total: c.rx_bytes,
                tx_total: c.tx_bytes,
            }
        })
        .collect()
}

/// Sums the interfaces that make up the machine's traffic: physical links,
/// or every interface when there are none (e.g. a VM with only tunnels).
pub fn total(rates: &[IfaceRate]) -> (f64, f64) {
    let physical: Vec<&IfaceRate> = rates.iter().filter(|r| r.kind.counts_toward_total()).collect();
    let pick: Vec<&IfaceRate> = if physical.is_empty() { rates.iter().filter(|r| r.kind != LinkKind::Virtual).collect() } else { physical };
    pick.iter().fold((0.0, 0.0), |(rx, tx), r| (rx + r.rx, tx + r.tx))
}

/// Display unit family.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Unit {
    Bytes,
    Bits,
}

impl Unit {
    pub fn parse(s: &str) -> Self {
        if s.eq_ignore_ascii_case("bits") { Unit::Bits } else { Unit::Bytes }
    }
}

const UNIT_STEP: f64 = 1000.0;

/// A rate split into a number and its unit: `(12.4, "MB/s")`,
/// `(99, "Mb/s")`. Decimal (SI) prefixes, like network gear.
pub fn format_rate(bytes_per_sec: f64, unit: Unit) -> (String, &'static str) {
    let (mut v, units): (f64, [&'static str; 5]) = match unit {
        Unit::Bytes => (bytes_per_sec.max(0.0), ["B/s", "kB/s", "MB/s", "GB/s", "TB/s"]),
        Unit::Bits => (bytes_per_sec.max(0.0) * 8.0, ["b/s", "kb/s", "Mb/s", "Gb/s", "Tb/s"]),
    };
    let mut i = 0;
    while v >= UNIT_STEP * 0.9995 && i < units.len() - 1 {
        v /= UNIT_STEP;
        i += 1;
    }
    let text = if i == 0 || v >= 100.0 { format!("{v:.0}") } else if v >= 10.0 { format!("{v:.1}") } else { format!("{v:.2}") };
    (text, units[i])
}

/// A byte amount (`1.24 GB`), SI prefixes.
pub fn format_amount(bytes: u64) -> String {
    let units = ["B", "kB", "MB", "GB", "TB", "PB"];
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= UNIT_STEP && i < units.len() - 1 {
        v /= UNIT_STEP;
        i += 1;
    }
    if i == 0 { format!("{bytes} B") } else if v >= 100.0 { format!("{v:.0} {}", units[i]) } else { format!("{v:.1} {}", units[i]) }
}

/// Rolling history: per-second samples plus coarser buckets for a longer
/// window. Gaps (time without sampling) clear what can no longer be shown
/// honestly instead of inventing values.
#[derive(Clone, Debug)]
pub struct RateHistory {
    /// (rx, tx) per second, oldest first.
    pub seconds: std::collections::VecDeque<(f64, f64)>,
    /// (rx, tx) averaged per bucket, oldest first.
    pub buckets: std::collections::VecDeque<(f64, f64)>,
    pending: (f64, f64, u32),
    last: Option<Instant>,
}

/// Seconds kept in the short history.
pub const SHORT_WINDOW: usize = 60;
/// Buckets of the long window and their length (60 × 10 s = 10 min).
pub const LONG_BUCKETS: usize = 60;
pub const BUCKET_SECONDS: u32 = 10;
/// A gap longer than this resets the short history.
pub const SHORT_GAP: Duration = Duration::from_secs(5);
/// A gap longer than this resets the long history.
pub const LONG_GAP: Duration = Duration::from_secs(60);

impl Default for RateHistory {
    fn default() -> Self {
        Self {
            seconds: std::collections::VecDeque::with_capacity(SHORT_WINDOW),
            buckets: std::collections::VecDeque::with_capacity(LONG_BUCKETS),
            pending: (0.0, 0.0, 0),
            last: None,
        }
    }
}

impl RateHistory {
    pub fn push(&mut self, rx: f64, tx: f64, now: Instant) {
        if let Some(last) = self.last {
            let gap = now.saturating_duration_since(last);
            if gap > LONG_GAP {
                self.buckets.clear();
                self.pending = (0.0, 0.0, 0);
            }
            if gap > SHORT_GAP {
                self.seconds.clear();
            }
        }
        self.last = Some(now);
        if self.seconds.len() == SHORT_WINDOW {
            self.seconds.pop_front();
        }
        self.seconds.push_back((rx, tx));
        self.pending = (self.pending.0 + rx, self.pending.1 + tx, self.pending.2 + 1);
        if self.pending.2 >= BUCKET_SECONDS {
            let n = self.pending.2 as f64;
            if self.buckets.len() == LONG_BUCKETS {
                self.buckets.pop_front();
            }
            self.buckets.push_back((self.pending.0 / n, self.pending.1 / n));
            self.pending = (0.0, 0.0, 0);
        }
    }

    pub fn peak_seconds(&self) -> f64 {
        self.seconds.iter().fold(0.0, |m, (r, t)| m.max(*r).max(*t))
    }

    pub fn peak_buckets(&self) -> f64 {
        self.buckets.iter().fold(0.0, |m, (r, t)| m.max(*r).max(*t))
    }
}

/// A local address of an interface.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct LocalAddr {
    pub iface: String,
    pub addr: IpAddr,
}

/// Unicast addresses of every interface except loopback (IPv4 first,
/// link-local IPv6 skipped).
pub fn local_addresses() -> Vec<LocalAddr> {
    let mut out = Vec::new();
    // SAFETY: getifaddrs allocates a linked list we walk read-only and free
    // with freeifaddrs; every pointer is checked for null before use.
    unsafe {
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        if libc::getifaddrs(&mut head) != 0 {
            return out;
        }
        let mut cur = head;
        while !cur.is_null() {
            let ifa = &*cur;
            cur = ifa.ifa_next;
            if ifa.ifa_addr.is_null() || ifa.ifa_name.is_null() {
                continue;
            }
            let name = std::ffi::CStr::from_ptr(ifa.ifa_name).to_string_lossy().into_owned();
            if name == "lo" {
                continue;
            }
            let addr = match (*ifa.ifa_addr).sa_family as i32 {
                libc::AF_INET => {
                    let sin = &*(ifa.ifa_addr as *const libc::sockaddr_in);
                    IpAddr::from(u32::from_be(sin.sin_addr.s_addr).to_be_bytes())
                }
                libc::AF_INET6 => {
                    let sin6 = &*(ifa.ifa_addr as *const libc::sockaddr_in6);
                    IpAddr::from(sin6.sin6_addr.s6_addr)
                }
                _ => continue,
            };
            out.push(LocalAddr { iface: name, addr });
        }
        libc::freeifaddrs(head);
    }
    sort_addresses(out)
}

fn is_link_local_v6(addr: &IpAddr) -> bool {
    matches!(addr, IpAddr::V6(v6) if (v6.segments()[0] & 0xffc0) == 0xfe80)
}

/// Drops loopback / link-local, orders IPv4 before IPv6 and keeps the
/// interface order stable.
pub fn sort_addresses(mut list: Vec<LocalAddr>) -> Vec<LocalAddr> {
    list.retain(|a| !a.addr.is_loopback() && !is_link_local_v6(&a.addr));
    list.sort_by(|a, b| a.addr.is_ipv6().cmp(&b.addr.is_ipv6()).then_with(|| a.iface.cmp(&b.iface)));
    list.dedup();
    list
}

/// Validates a public-IP reply (plain text, one address).
pub fn parse_ip_reply(body: &str) -> Result<IpAddr, String> {
    body.trim().parse::<IpAddr>().map_err(|_| tf("The endpoint did not answer with an IP address: {reply}", &[("reply", &body.trim().chars().take(40).collect::<String>())]))
}

/// Looks up the public address (blocking; call off the UI thread).
pub fn public_ip(endpoint: &str) -> Result<IpAddr, String> {
    crate::offline::check()?;
    if !endpoint.starts_with("https://") && !endpoint.starts_with("http://") {
        return Err(t("The public IP endpoint must be an http(s) URL").into());
    }
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(PUBLIC_IP_TIMEOUT)).build().into();
    let mut reply = agent.get(endpoint).header("Accept", "text/plain").call().map_err(|e| e.to_string())?;
    let mut body = String::new();
    reply.body_mut().as_reader().take(MAX_IP_REPLY).read_to_string(&mut body).map_err(|e| e.to_string())?;
    parse_ip_reply(&body)
}

/// Live progress of a speed test, shared with the UI.
#[derive(Default, Debug)]
pub struct SpeedProgress {
    pub bytes: AtomicU64,
    /// Total expected bytes.
    pub expected: AtomicU64,
    /// Microseconds since the first byte arrived.
    pub micros: AtomicU64,
    pub cancel: AtomicBool,
}

impl SpeedProgress {
    /// Current throughput in bytes per second.
    pub fn rate(&self) -> f64 {
        let us = self.micros.load(Ordering::Relaxed);
        if us == 0 { 0.0 } else { self.bytes.load(Ordering::Relaxed) as f64 / (us as f64 / 1e6) }
    }

    pub fn fraction(&self) -> f32 {
        let e = self.expected.load(Ordering::Relaxed);
        if e == 0 { 0.0 } else { (self.bytes.load(Ordering::Relaxed) as f64 / e as f64).clamp(0.0, 1.0) as f32 }
    }
}

/// A finished speed test.
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct SpeedResult {
    pub bytes: u64,
    pub seconds: f64,
    /// Bytes per second.
    pub rate: f64,
    /// Time to the first byte (ms), a rough latency figure.
    pub first_byte_ms: f64,
}

/// The speed test URL for a download of `megabytes`.
pub fn speed_test_url(megabytes: u64) -> String {
    format!("{SPEED_TEST_URL}?bytes={}", megabytes.clamp(1, MAX_TEST_MEGABYTES) * BYTES_PER_MEGABYTE)
}

/// Downloads `megabytes` from Cloudflare, measuring throughput from the
/// first byte (blocking; call off the UI thread). `progress` updates live.
pub fn speed_test(megabytes: u64, progress: Arc<SpeedProgress>) -> Result<SpeedResult, String> {
    crate::offline::check()?;
    let expected = megabytes.clamp(1, MAX_TEST_MEGABYTES) * BYTES_PER_MEGABYTE;
    progress.expected.store(expected, Ordering::Relaxed);
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_connect(Some(SPEED_TEST_CONNECT_TIMEOUT)).timeout_global(Some(SPEED_TEST_TIMEOUT)).build().into();
    let started = Instant::now();
    let mut reply = agent.get(&speed_test_url(megabytes)).call().map_err(|e| e.to_string())?;
    let mut reader = reply.body_mut().with_config().limit(expected + BYTES_PER_MEGABYTE).reader();
    let mut buf = vec![0u8; READ_CHUNK];
    let mut first: Option<Instant> = None;
    let mut total = 0u64;
    loop {
        if progress.cancel.load(Ordering::Relaxed) {
            return Err(t("Speed test cancelled").into());
        }
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        let first_at = *first.get_or_insert_with(Instant::now);
        total += n as u64;
        progress.bytes.store(total, Ordering::Relaxed);
        progress.micros.store(first_at.elapsed().as_micros() as u64, Ordering::Relaxed);
    }
    let first_at = first.ok_or_else(|| t("The speed test server sent no data").to_string())?;
    let seconds = first_at.elapsed().as_secs_f64().max(1e-3);
    Ok(SpeedResult { bytes: total, seconds, rate: total as f64 / seconds, first_byte_ms: first_at.duration_since(started).as_secs_f64() * 1000.0 })
}

/// Groups addresses by interface (for the panel and the CLI).
pub fn addresses_by_iface(list: &[LocalAddr]) -> BTreeMap<String, Vec<IpAddr>> {
    let mut map: BTreeMap<String, Vec<IpAddr>> = BTreeMap::new();
    for a in list {
        map.entry(a.iface.clone()).or_default().push(a.addr);
    }
    map
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets errs drop fifo colls carrier compressed
    lo: 1000      10    0    0    0     0          0         0     1000      10    0    0    0     0       0          0
wlan0: 5000000   4000    0    0    0     0          0         0   800000    3000    0    0    0     0       0          0
  wg0:  200000    100    0    0    0     0          0         0    50000     100    0    0    0     0       0          0
docker0: 99999     10    0    0    0     0          0         0    99999      10    0    0    0     0       0          0
";

    #[test]
    fn parses_proc_net_dev_without_loopback() {
        let c = parse_proc_net_dev(SAMPLE);
        assert_eq!(c.len(), 3);
        assert_eq!(c[0], Counters { name: "wlan0".into(), rx_bytes: 5_000_000, tx_bytes: 800_000 });
        assert_eq!(c[1].name, "wg0");
        assert!(parse_proc_net_dev("garbage").is_empty());
        assert!(!read_counters().iter().any(|c| c.name == "lo"));
    }

    #[test]
    fn rates_handle_resets_and_new_interfaces() {
        let t0 = Instant::now();
        let old = Sample { at: t0, counters: vec![Counters { name: "eth0".into(), rx_bytes: 1000, tx_bytes: 500 }] };
        let new = Sample {
            at: t0 + Duration::from_secs(2),
            counters: vec![Counters { name: "eth0".into(), rx_bytes: 5000, tx_bytes: 100 }, Counters { name: "wg0".into(), rx_bytes: 9, tx_bytes: 9 }],
        };
        let r = rates(&old, &new, LinkKind::from_name);
        assert_eq!(r[0].rx, 2000.0);
        assert_eq!(r[0].tx, 0.0, "a counter that went backwards (reset) reads as zero");
        assert_eq!(r[1].rx, 0.0, "a new interface has no baseline yet");
        assert!(rates(&new, &new, LinkKind::from_name).is_empty());
    }

    #[test]
    fn totals_skip_tunnels_and_bridges() {
        let r = |name: &str, rx: f64| IfaceRate { name: name.into(), kind: LinkKind::from_name(name), rx, tx: 0.0, rx_total: 0, tx_total: 0 };
        assert_eq!(total(&[r("wlan0", 100.0), r("wg0", 90.0), r("docker0", 5.0)]).0, 100.0);
        // Only a tunnel: it is all we have.
        assert_eq!(total(&[r("tun0", 40.0), r("veth1", 7.0)]).0, 40.0);
        assert_eq!(LinkKind::from_name("enp5s0"), LinkKind::Ethernet);
        assert_eq!(LinkKind::from_name("br-1234"), LinkKind::Virtual);
    }

    #[test]
    fn formats_rates_in_bytes_and_bits() {
        assert_eq!(format_rate(0.0, Unit::Bytes), ("0".into(), "B/s"));
        assert_eq!(format_rate(999.0, Unit::Bytes), ("999".into(), "B/s"));
        assert_eq!(format_rate(1_240_000.0, Unit::Bytes), ("1.24".into(), "MB/s"));
        assert_eq!(format_rate(12_500_000.0, Unit::Bits), ("100".into(), "Mb/s"));
        assert_eq!(format_rate(45_600.0, Unit::Bytes), ("45.6".into(), "kB/s"));
        assert_eq!(format_amount(1_240_000_000), "1.2 GB");
        assert_eq!(format_amount(512), "512 B");
        assert_eq!(Unit::parse("BITS"), Unit::Bits);
    }

    #[test]
    fn history_buckets_and_gaps() {
        let mut h = RateHistory::default();
        let t0 = Instant::now();
        for i in 0..25u64 {
            h.push(i as f64, 1.0, t0 + Duration::from_secs(i));
        }
        assert_eq!(h.seconds.len(), 25);
        assert_eq!(h.buckets.len(), 2);
        assert_eq!(h.buckets[0].0, 4.5, "first bucket averages seconds 0..9");
        assert_eq!(h.peak_seconds(), 24.0);
        // A 10 s gap: the per-second graph restarts, the buckets survive.
        h.push(1.0, 1.0, t0 + Duration::from_secs(35));
        assert_eq!(h.seconds.len(), 1);
        assert_eq!(h.buckets.len(), 2);
        // Over a minute: both restart.
        h.push(1.0, 1.0, t0 + Duration::from_secs(200));
        assert!(h.buckets.is_empty());
        let mut full = RateHistory::default();
        for i in 0..(SHORT_WINDOW as u64 + 5) {
            full.push(1.0, 1.0, t0 + Duration::from_secs(i));
        }
        assert_eq!(full.seconds.len(), SHORT_WINDOW);
    }

    #[test]
    fn addresses_sort_and_filter() {
        let a = |iface: &str, ip: &str| LocalAddr { iface: iface.into(), addr: ip.parse().unwrap() };
        let sorted = sort_addresses(vec![a("wlan0", "fe80::1"), a("wlan0", "2001:db8::5"), a("wlan0", "192.168.1.20"), a("lo", "127.0.0.1"), a("eth0", "10.0.0.2")]);
        assert_eq!(sorted.iter().map(|x| x.addr.to_string()).collect::<Vec<_>>(), ["10.0.0.2", "192.168.1.20", "2001:db8::5"]);
        assert_eq!(addresses_by_iface(&sorted)["wlan0"].len(), 2);
        // The live call works and never lists loopback.
        assert!(local_addresses().iter().all(|x| !x.addr.is_loopback()));
    }

    #[test]
    fn ip_replies_and_urls() {
        assert_eq!(parse_ip_reply(" 203.0.113.9\n").unwrap().to_string(), "203.0.113.9");
        assert!(parse_ip_reply("<html>").is_err());
        assert_eq!(speed_test_url(25), "https://speed.cloudflare.com/__down?bytes=25000000");
        assert!(speed_test_url(10_000).ends_with(&(MAX_TEST_MEGABYTES * BYTES_PER_MEGABYTE).to_string()));
        assert!(public_ip("ftp://nope").is_err());
        let p = SpeedProgress::default();
        assert_eq!(p.rate(), 0.0);
        p.expected.store(100, Ordering::Relaxed);
        p.bytes.store(50, Ordering::Relaxed);
        assert_eq!(p.fraction(), 0.5);
    }
}
