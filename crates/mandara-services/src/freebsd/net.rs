//! Networking on FreeBSD, which has no NetworkManager: interfaces and
//! addresses from `ifconfig(8)`, Wi-Fi from `wpa_supplicant(8)` through
//! `wpa_cli(8)`, the default route from `route(8)`, byte counters from
//! `getifaddrs(3)` and change events from a routing socket.

use std::fmt::Write as _;

/// One interface as `ifconfig` prints it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Interface {
    pub name: String,
    /// `UP` in its flags (administratively on).
    pub up: bool,
    /// `RUNNING` in its flags.
    pub running: bool,
    pub loopback: bool,
    /// A wlan(4) interface (`groups: wlan` or an IEEE 802.11 medium).
    pub wireless: bool,
    pub ether: Option<String>,
    /// IPv4 addresses with their prefix length.
    pub inet: Vec<(String, u8)>,
    /// IPv6 addresses (link-local ones included, scope stripped).
    pub inet6: Vec<(String, u8)>,
    /// `status:` (`active`, `no carrier`, `associated`…).
    pub status: Option<String>,
    /// The joined Wi-Fi network (`ssid` line).
    pub ssid: Option<String>,
    pub bssid: Option<String>,
    pub channel_mhz: Option<u32>,
}

impl Interface {
    /// Has a carrier: wired `active`, or Wi-Fi `associated`.
    pub fn connected(&self) -> bool {
        self.running && matches!(self.status.as_deref(), Some("active" | "associated"))
    }
}

/// `0xffffff00` → 24.
fn prefix_from_hex(mask: &str) -> Option<u8> {
    u32::from_str_radix(mask.strip_prefix("0x")?, 16).ok().map(|m| m.count_ones() as u8)
}

/// The value after `key` in a whitespace-separated line, honouring the
/// double quotes ifconfig puts around names with spaces.
fn quoted_after<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let rest = line.strip_prefix(key)?.strip_prefix(' ')?;
    if let Some(inner) = rest.strip_prefix('"') {
        inner.find('"').map(|end| &inner[..end])
    } else {
        rest.split_whitespace().next()
    }
}

/// Every interface in `ifconfig` output (all interfaces, or one).
pub fn parse_ifconfig(text: &str) -> Vec<Interface> {
    let mut out: Vec<Interface> = Vec::new();
    for line in text.lines() {
        if !line.starts_with(['\t', ' ']) {
            let Some((name, rest)) = line.split_once(": flags=") else { continue };
            let flags = rest.split_once('<').and_then(|(_, f)| f.split_once('>')).map(|(f, _)| f).unwrap_or_default();
            let has = |f: &str| flags.split(',').any(|x| x == f);
            out.push(Interface { name: name.to_string(), up: has("UP"), running: has("RUNNING"), loopback: has("LOOPBACK"), ..Interface::default() });
            continue;
        }
        let Some(iface) = out.last_mut() else { continue };
        let line = line.trim();
        let mut words = line.split_whitespace();
        match words.next() {
            Some("ether") => iface.ether = words.next().map(str::to_string),
            Some("inet") => {
                let addr = words.next().unwrap_or_default().to_string();
                let mut prefix = 32;
                while let Some(w) = words.next() {
                    if w == "netmask" {
                        prefix = words.next().and_then(prefix_from_hex).unwrap_or(prefix);
                    }
                }
                iface.inet.push((addr, prefix));
            }
            Some("inet6") => {
                let addr = words.next().unwrap_or_default();
                let addr = addr.split('%').next().unwrap_or(addr).to_string();
                let mut prefix = 128;
                while let Some(w) = words.next() {
                    if w == "prefixlen" {
                        prefix = words.next().and_then(|p| p.parse().ok()).unwrap_or(prefix);
                    }
                }
                iface.inet6.push((addr, prefix));
            }
            Some("status:") => iface.status = Some(line["status:".len()..].trim().to_string()),
            Some("groups:") => iface.wireless |= words.any(|g| g == "wlan"),
            Some("media:") => iface.wireless |= line.contains("IEEE 802.11"),
            Some("ssid") => {
                iface.wireless = true;
                iface.ssid = quoted_after(line, "ssid").filter(|s| !s.is_empty()).map(str::to_string);
                if let Some(at) = line.find(" bssid ") {
                    iface.bssid = line[at + " bssid ".len()..].split_whitespace().next().map(str::to_string);
                }
                if let Some(open) = line.find('(') {
                    iface.channel_mhz = line[open + 1..].split_whitespace().next().and_then(|v| v.parse().ok());
                }
            }
            _ => {}
        }
    }
    out
}

/// `wpa_cli status` fields that matter here.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WpaStatus {
    /// `COMPLETED`, `SCANNING`, `ASSOCIATING`, `DISCONNECTED`…
    pub state: String,
    pub ssid: Option<String>,
    pub bssid: Option<String>,
    pub freq: Option<u32>,
    pub ip_address: Option<String>,
}

impl WpaStatus {
    pub fn connected(&self) -> bool {
        self.state == "COMPLETED"
    }

    /// Between scanning and a completed handshake.
    pub fn connecting(&self) -> bool {
        matches!(self.state.as_str(), "AUTHENTICATING" | "ASSOCIATING" | "ASSOCIATED" | "4WAY_HANDSHAKE" | "GROUP_HANDSHAKE")
    }
}

pub fn parse_wpa_status(text: &str) -> WpaStatus {
    let mut out = WpaStatus::default();
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        match k {
            "wpa_state" => out.state = v.to_string(),
            "ssid" => out.ssid = Some(v.to_string()),
            "bssid" => out.bssid = Some(v.to_ascii_lowercase()),
            "freq" => out.freq = v.parse().ok(),
            "ip_address" => out.ip_address = Some(v.to_string()),
            _ => {}
        }
    }
    out
}

/// One `wpa_cli scan_results` row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScanResult {
    pub bssid: String,
    pub frequency_mhz: u32,
    /// Strength `0..=100`.
    pub strength: u8,
    pub secured: bool,
    pub ssid: String,
}

/// A scan level as `0..=100`. Drivers that report dBm give negative
/// levels (-30 excellent, -90 unusable); FreeBSD's net80211 reports the
/// signal above the noise floor in half-dB steps (80 = 40 dB, excellent).
pub fn signal_strength(level: i32) -> u8 {
    const DBM_FLOOR: i32 = -100;
    const DBM_SCALE: i32 = 2;
    const HALF_DB_EXCELLENT: i32 = 80;
    let pct = if level < 0 { (level - DBM_FLOOR) * DBM_SCALE } else { level * 100 / HALF_DB_EXCELLENT };
    pct.clamp(0, 100) as u8
}

/// `wpa_cli scan_results`: a header line, then tab-separated
/// `bssid frequency level flags ssid` rows.
pub fn parse_scan_results(text: &str) -> Vec<ScanResult> {
    text.lines()
        .filter_map(|line| {
            let mut f = line.splitn(5, '\t');
            let bssid = f.next()?;
            if bssid.len() != 17 || bssid.matches(':').count() != 5 {
                return None;
            }
            let frequency_mhz = f.next()?.trim().parse().ok()?;
            let level: i32 = f.next()?.trim().parse().ok()?;
            let flags = f.next().unwrap_or_default();
            let ssid = decode_ssid(f.next().unwrap_or_default());
            Some(ScanResult { bssid: bssid.to_ascii_lowercase(), frequency_mhz, strength: signal_strength(level), secured: ["WPA", "WEP", "RSN", "SAE"].iter().any(|k| flags.contains(k)), ssid })
        })
        .collect()
}

/// wpa_supplicant prints non-printable SSID bytes as `\xNN` (and `\\`,
/// `\"`); turns them back into text.
fn decode_ssid(raw: &str) -> String {
    let b = raw.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match (b[i], b.get(i + 1)) {
            (b'\\', Some(b'x')) => match raw.get(i + 2..i + 4).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                Some(v) => {
                    out.push(v);
                    i += 4;
                }
                None => {
                    out.push(b'\\');
                    i += 1;
                }
            },
            (b'\\', Some(c @ (b'\\' | b'"'))) => {
                out.push(*c);
                i += 2;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `route -n get default`: the interface the default route leaves through.
pub fn parse_route_interface(text: &str) -> Option<String> {
    text.lines().find_map(|l| l.trim().strip_prefix("interface:")).map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

/// Lower-case hex of `bytes`.
pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// The WPA pre-shared key (PMK) for `passphrase` on network `ssid`:
/// PBKDF2-HMAC-SHA1, 4096 rounds, 32 bytes (IEEE 802.11i). Handing
/// wpa_supplicant this 64-digit key instead of the passphrase means the
/// passphrase never needs quoting and is never written anywhere.
pub fn wpa_psk(passphrase: &str, ssid: &[u8]) -> [u8; 32] {
    use hmac::{Hmac, Mac as _};
    type HmacSha1 = Hmac<sha1::Sha1>;
    const ROUNDS: usize = 4096;
    let mut out = [0u8; 32];
    for (block, chunk) in out.chunks_mut(20).enumerate() {
        let Ok(mut mac) = HmacSha1::new_from_slice(passphrase.as_bytes()) else { return out };
        mac.update(ssid);
        mac.update(&(block as u32 + 1).to_be_bytes());
        let mut u = mac.finalize().into_bytes();
        let mut t = u;
        for _ in 1..ROUNDS {
            let Ok(mut mac) = HmacSha1::new_from_slice(passphrase.as_bytes()) else { return out };
            mac.update(&u);
            u = mac.finalize().into_bytes();
            t.iter_mut().zip(u.iter()).for_each(|(a, b)| *a ^= b);
        }
        chunk.copy_from_slice(&t[..chunk.len()]);
    }
    out
}

/// The `wpa_cli` commands that set up and select a network, with the SSID
/// as hex and the key as a PMK (`id` is `add_network`'s answer).
pub fn join_commands(id: u32, ssid: &str, passphrase: Option<&str>) -> Vec<String> {
    let mut out = vec![format!("set_network {id} ssid {}", hex(ssid.as_bytes()))];
    match passphrase {
        Some(p) => out.push(format!("set_network {id} psk {}", hex(&wpa_psk(p, ssid.as_bytes())))),
        None => out.push(format!("set_network {id} key_mgmt NONE")),
    }
    out.push(format!("select_network {id}"));
    out
}

#[cfg(target_os = "freebsd")]
pub use sys::*;

#[cfg(target_os = "freebsd")]
mod sys {
    use std::process::{Command, Stdio};

    /// `ifconfig` itself (base system).
    pub const IFCONFIG: &str = "/sbin/ifconfig";
    /// `wpa_cli` (base system).
    pub const WPA_CLI: &str = "/usr/sbin/wpa_cli";
    const ROUTE: &str = "/sbin/route";

    fn output(program: &str, args: &[&str]) -> Option<String> {
        let out = Command::new(program).args(args).stdin(Stdio::null()).stderr(Stdio::null()).output().ok()?;
        out.status.success().then(|| String::from_utf8_lossy(&out.stdout).into_owned())
    }

    pub fn interfaces() -> Vec<super::Interface> {
        output(IFCONFIG, &[]).map(|t| super::parse_ifconfig(&t)).unwrap_or_default()
    }

    /// The default route's interface.
    pub fn default_interface() -> Option<String> {
        output(ROUTE, &["-n", "get", "default"]).as_deref().and_then(super::parse_route_interface)
    }

    /// `wpa_cli -i <iface> <args>`; `None` when wpa_supplicant isn't
    /// running there or its control socket isn't open to this user.
    pub fn wpa(iface: &str, args: &[&str]) -> Option<String> {
        let mut all = vec!["-i", iface];
        all.extend_from_slice(args);
        output(WPA_CLI, &all).filter(|t| !t.starts_with("Failed to connect"))
    }

    /// Runs `commands` in one `wpa_cli` session, fed on standard input so
    /// no key ever appears in a process listing. Returns its output.
    pub fn wpa_session(iface: &str, commands: &[String]) -> std::io::Result<String> {
        use std::io::Write as _;
        let mut child = Command::new(WPA_CLI).args(["-i", iface]).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn()?;
        if let Some(mut stdin) = child.stdin.take() {
            for c in commands {
                writeln!(stdin, "{c}")?;
            }
            writeln!(stdin, "quit")?;
        }
        let out = child.wait_with_output()?;
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }

    /// Bytes received and sent by each interface but loopback.
    pub fn traffic() -> Vec<(String, u64, u64)> {
        let mut out = Vec::new();
        let mut head: *mut libc::ifaddrs = std::ptr::null_mut();
        // SAFETY: getifaddrs allocates the list; it is freed below.
        if unsafe { libc::getifaddrs(&mut head) } != 0 {
            return out;
        }
        let mut cur = head;
        while !cur.is_null() {
            // SAFETY: `cur` is a node of the list getifaddrs returned.
            let ifa = unsafe { &*cur };
            cur = ifa.ifa_next;
            let link = !ifa.ifa_addr.is_null() && i32::from(unsafe { (*ifa.ifa_addr).sa_family }) == libc::AF_LINK;
            if !link || ifa.ifa_data.is_null() || ifa.ifa_flags & libc::IFF_LOOPBACK as libc::c_uint != 0 {
                continue;
            }
            // SAFETY: AF_LINK entries carry a `struct if_data` in ifa_data;
            // ifa_name is a NUL-terminated string.
            let (data, name) = unsafe { (std::ptr::read_unaligned(ifa.ifa_data.cast::<libc::if_data>()), std::ffi::CStr::from_ptr(ifa.ifa_name)) };
            out.push((name.to_string_lossy().into_owned(), data.ifi_ibytes, data.ifi_obytes));
        }
        // SAFETY: `head` came from getifaddrs and is freed once.
        unsafe { libc::freeifaddrs(head) };
        out
    }

    /// A routing socket: readable whenever an interface, address or route
    /// changes (link up and down, DHCP, Wi-Fi association).
    pub fn route_socket() -> std::io::Result<std::os::fd::OwnedFd> {
        // SAFETY: plain socket creation; the fd is owned below.
        let fd = unsafe { libc::socket(libc::PF_ROUTE, libc::SOCK_RAW | libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: `fd` is a fresh descriptor nobody else owns.
        Ok(unsafe { std::os::fd::FromRawFd::from_raw_fd(fd) })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `ifconfig` on the FreeBSD 14.4 test machine.
    const VM_IFCONFIG: &str = "vtnet0: flags=1008843<UP,BROADCAST,RUNNING,SIMPLEX,MULTICAST,LOWER_UP> metric 0 mtu 1500
\toptions=880028<VLAN_MTU,JUMBO_MTU,LINKSTATE,HWSTATS>
\tether 52:54:00:4d:4e:01
\tinet 10.0.2.15 netmask 0xffffff00 broadcast 10.0.2.255
\tinet6 fe80::5054:ff:fe4d:4e01%vtnet0 prefixlen 64 scopeid 0x1
\tinet6 fec0::5054:ff:fe4d:4e01 prefixlen 64 autoconf pltime 14400 vltime 86400
\tmedia: Ethernet autoselect (10Gbase-T <full-duplex>)
\tstatus: active
\tnd6 options=23<PERFORMNUD,ACCEPT_RTADV,AUTO_LINKLOCAL>
lo0: flags=1008049<UP,LOOPBACK,RUNNING,MULTICAST,LOWER_UP> metric 0 mtu 16384
\toptions=680003<RXCSUM,TXCSUM,LINKSTATE,RXCSUM_IPV6,TXCSUM_IPV6>
\tinet 127.0.0.1 netmask 0xff000000
\tinet6 ::1 prefixlen 128
\tinet6 fe80::1%lo0 prefixlen 64 scopeid 0x2
\tgroups: lo
\tnd6 options=21<PERFORMNUD,AUTO_LINKLOCAL>
";

    /// A laptop's `ifconfig wlan0` (iwm(4)), associated to a network whose
    /// name has a space.
    const WLAN_IFCONFIG: &str = "wlan0: flags=8843<UP,BROADCAST,RUNNING,SIMPLEX,MULTICAST> metric 0 mtu 1500
\tether 3c:a9:f4:1b:22:10
\tinet 192.168.1.23 netmask 0xffffff00 broadcast 192.168.1.255
\tgroups: wlan
\tssid \"Corniche Cafe\" channel 6 (2437 MHz 11g ht/20) bssid 00:aa:bb:cc:dd:ee
\tregdomain FCC country US authmode WPA2/802.11i privacy ON
\tdeftxkey UNDEF AES-CCM 2:128-bit txpower 30 bmiss 7 scanvalid 60
\tparent interface: iwm0
\tmedia: IEEE 802.11 Wireless Ethernet MCS mode 11ng
\tstatus: associated
\tnd6 options=29<PERFORMNUD,IFDISABLED,AUTO_LINKLOCAL>
";

    #[test]
    fn reads_wired_and_loopback_interfaces() {
        let all = parse_ifconfig(VM_IFCONFIG);
        assert_eq!(all.len(), 2);
        let vt = &all[0];
        assert_eq!(vt.name, "vtnet0");
        assert!(vt.up && vt.running && !vt.loopback && !vt.wireless);
        assert_eq!(vt.ether.as_deref(), Some("52:54:00:4d:4e:01"));
        assert_eq!(vt.inet, [("10.0.2.15".to_string(), 24)]);
        assert_eq!(vt.inet6[0], ("fe80::5054:ff:fe4d:4e01".to_string(), 64));
        assert!(vt.connected());
        assert!(all[1].loopback);
        assert_eq!(all[1].inet, [("127.0.0.1".to_string(), 8)]);
    }

    #[test]
    fn reads_an_associated_wlan() {
        let w = &parse_ifconfig(WLAN_IFCONFIG)[0];
        assert!(w.wireless && w.connected());
        assert_eq!(w.ssid.as_deref(), Some("Corniche Cafe"));
        assert_eq!(w.bssid.as_deref(), Some("00:aa:bb:cc:dd:ee"));
        assert_eq!(w.channel_mhz, Some(2437));
        let down = parse_ifconfig("wlan0: flags=8802<BROADCAST,SIMPLEX,MULTICAST> metric 0 mtu 1500\n\tgroups: wlan\n\tssid \"\" channel 1 (2412 MHz 11b)\n\tstatus: no carrier\n");
        assert!(down[0].wireless && !down[0].up && !down[0].connected());
        assert_eq!(down[0].ssid, None);
    }

    #[test]
    fn wpa_cli_status_and_scan_results() {
        let s = parse_wpa_status("bssid=00:AA:BB:CC:DD:EE\nfreq=2437\nssid=Corniche Cafe\nid=0\nmode=station\npairwise_cipher=CCMP\ngroup_cipher=CCMP\nkey_mgmt=WPA2-PSK\nwpa_state=COMPLETED\nip_address=192.168.1.23\naddress=3c:a9:f4:1b:22:10\n");
        assert!(s.connected() && !s.connecting());
        assert_eq!((s.ssid.as_deref(), s.bssid.as_deref(), s.freq), (Some("Corniche Cafe"), Some("00:aa:bb:cc:dd:ee"), Some(2437)));
        assert!(parse_wpa_status("wpa_state=4WAY_HANDSHAKE\n").connecting());
        let rows = parse_scan_results("bssid / frequency / signal level / flags / ssid\n00:aa:bb:cc:dd:ee\t2437\t62\t[WPA2-PSK-CCMP][ESS]\tCorniche Cafe\n11:22:33:44:55:66\t5180\t-48\t[ESS]\tMontaza\\x20Guest\n77:88:99:aa:bb:cc\t2412\t-95\t[WPA2-SAE-CCMP][ESS]\t\n");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0], ScanResult { bssid: "00:aa:bb:cc:dd:ee".into(), frequency_mhz: 2437, strength: 77, secured: true, ssid: "Corniche Cafe".into() });
        assert_eq!((rows[1].strength, rows[1].secured, rows[1].ssid.as_str()), (100, false, "Montaza Guest"));
        assert_eq!((rows[2].strength, rows[2].ssid.as_str()), (10, ""));
    }

    #[test]
    fn signal_levels_cover_both_scales() {
        assert_eq!(signal_strength(-30), 100);
        assert_eq!(signal_strength(-70), 60);
        assert_eq!(signal_strength(-120), 0);
        assert_eq!(signal_strength(40), 50);
        assert_eq!(signal_strength(200), 100);
    }

    #[test]
    #[cfg(target_os = "freebsd")]
    fn reads_live_links() {
        let links = interfaces();
        assert!(links.iter().any(|l| l.loopback), "ifconfig lists lo0");
        assert!(traffic().iter().all(|(name, _, _)| !name.starts_with("lo")));
        assert!(route_socket().is_ok());
    }

    #[test]
    fn default_route_interface() {
        let text = "   route to: 0.0.0.0\ndestination: 0.0.0.0\n       mask: 0.0.0.0\n    gateway: 10.0.2.2\n        fib: 0\n  interface: vtnet0\n      flags: <UP,GATEWAY,DONE,STATIC>\n";
        assert_eq!(parse_route_interface(text).as_deref(), Some("vtnet0"));
        assert_eq!(parse_route_interface("route: route has not been found\n"), None);
    }

    /// IEEE 802.11i's test vector (H.4.1): passphrase "password", SSID "IEEE".
    #[test]
    fn psk_matches_the_standard_and_joins_without_the_passphrase() {
        assert_eq!(hex(&wpa_psk("password", b"IEEE")), "f42c6fc52df0ebef9ebb4b90b38a5f902e83fe1b135a70e23aed762e9710a12e");
        let cmds = join_commands(3, "Corniche \"Cafe\"", Some("p@ss \"word\""));
        assert_eq!(cmds[0], format!("set_network 3 ssid {}", hex(b"Corniche \"Cafe\"")));
        assert!(cmds[1].starts_with("set_network 3 psk ") && !cmds[1].contains("word"));
        assert_eq!(cmds[2..], ["select_network 3".to_string()]);
        assert_eq!(join_commands(0, "Open", None)[1], "set_network 0 key_mgmt NONE");
    }
}
