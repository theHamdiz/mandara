//! The network actor on FreeBSD, which has no NetworkManager: links and
//! addresses from `ifconfig`, Wi-Fi through wpa_supplicant's control
//! socket (`wpa_cli`), woken by a routing socket on every link, address
//! or route change. Joining a network hands wpa_supplicant the SSID in hex
//! and a precomputed key, on `wpa_cli`'s standard input (never in a
//! process listing); turning Wi-Fi on or off is `ifconfig wlanN up|down`
//! as root through pkexec.

#[cfg(target_os = "freebsd")]
use std::time::Duration;

#[cfg(target_os = "freebsd")]
use anyhow::{Result, anyhow};

use super::{AccessPoint, Connectivity, NetworkState};
#[cfg(target_os = "freebsd")]
use super::NetworkCommand;
#[cfg(target_os = "freebsd")]
use crate::actor::{Envelope, reply};
use crate::freebsd::net::{self, Interface};

/// Change events coalesced into one re-read.
#[cfg(target_os = "freebsd")]
const DEBOUNCE: Duration = Duration::from_millis(400);
/// Re-read without events this often (Wi-Fi strength and scan results
/// change without a routing message).
#[cfg(target_os = "freebsd")]
const POLL: Duration = Duration::from_secs(30);
/// Bytes read from the routing socket at a time (messages are discarded:
/// any one means "re-read").
#[cfg(target_os = "freebsd")]
const ROUTE_READ: usize = 2048;
/// Links that are VPN tunnels.
const VPN_PREFIXES: [&str; 4] = ["wg", "tun", "ovpn", "tailscale"];

/// The interface that carries Wi-Fi (the first wlan(4)).
fn wlan(links: &[Interface]) -> Option<&Interface> {
    links.iter().find(|i| i.wireless && !i.loopback)
}

/// The state from `ifconfig`, the default route and wpa_supplicant's view.
pub(super) fn state(links: &[Interface], default_iface: Option<&str>, wpa: Option<&net::WpaStatus>, scan: &[net::ScanResult]) -> NetworkState {
    let mut s = NetworkState { available: true, ..NetworkState::default() };
    let wifi = wlan(links);
    s.wifi_hardware = wifi.is_some();
    s.wifi_enabled = wifi.is_some_and(|w| w.up);
    let current_ssid = wpa.filter(|w| w.connected()).and_then(|w| w.ssid.clone()).or_else(|| wifi.filter(|w| w.connected()).and_then(|w| w.ssid.clone()));
    let has_address = |i: &Interface| !i.inet.is_empty() || i.inet6.iter().any(|(a, _)| !a.starts_with("fe80"));
    let primary = default_iface.and_then(|name| links.iter().find(|i| i.name == name)).filter(|i| i.connected() || i.running);
    let wired = links.iter().find(|i| !i.wireless && !i.loopback && i.connected() && has_address(i) && !VPN_PREFIXES.iter().any(|p| i.name.starts_with(p)));
    match (primary, wifi) {
        (Some(p), _) if p.wireless => {
            s.connectivity = Connectivity::Wifi;
            s.primary = current_ssid.clone().unwrap_or_else(|| p.name.clone());
        }
        (Some(p), _) if !VPN_PREFIXES.iter().any(|v| p.name.starts_with(v)) => {
            s.connectivity = Connectivity::Ethernet;
            s.primary = p.name.clone();
        }
        _ if wired.is_some() => {
            s.connectivity = Connectivity::Ethernet;
            s.primary = wired.map(|w| w.name.clone()).unwrap_or_default();
        }
        (_, Some(w)) if w.connected() && current_ssid.is_some() => {
            s.connectivity = Connectivity::Wifi;
            s.primary = current_ssid.clone().unwrap_or_default();
        }
        _ if wpa.is_some_and(|w| w.connecting()) => s.connectivity = Connectivity::Connecting,
        _ => s.connectivity = Connectivity::Disconnected,
    }
    let mut aps: Vec<AccessPoint> = Vec::new();
    for r in scan.iter().filter(|r| !r.ssid.is_empty()) {
        let active = current_ssid.as_deref() == Some(r.ssid.as_str());
        let ap = AccessPoint { ssid: r.ssid.clone(), strength: r.strength, secured: r.secured, active, frequency_mhz: r.frequency_mhz };
        match aps.iter_mut().find(|a| a.ssid == ap.ssid) {
            Some(existing) if ap.strength > existing.strength => *existing = ap,
            Some(_) => {}
            None => aps.push(ap),
        }
    }
    aps.sort_by(|a, b| b.active.cmp(&a.active).then(b.strength.cmp(&a.strength)).then(a.ssid.cmp(&b.ssid)));
    if s.connectivity == Connectivity::Wifi {
        s.strength = aps.iter().find(|a| a.active).map_or(0, |a| a.strength);
    }
    s.access_points = aps;
    s.vpns = links.iter().filter(|i| i.up && i.running && VPN_PREFIXES.iter().any(|p| i.name.starts_with(p)) && has_address(i)).map(|i| i.name.clone()).collect();
    s.vpns.sort();
    s
}

/// The id of a saved network for `ssid` in `wpa_cli list_networks`
/// (`network id / ssid / bssid / flags`, tab-separated rows).
pub(super) fn saved_network(list: &str, ssid: &str) -> Option<u32> {
    list.lines().skip(1).find_map(|l| {
        let mut f = l.split('\t');
        let id = f.next()?.trim().parse().ok()?;
        (f.next()? == ssid).then_some(id)
    })
}

/// One blocking read of everything.
#[cfg(target_os = "freebsd")]
fn read() -> NetworkState {
    let links = net::interfaces();
    let default_iface = net::default_interface();
    let (wpa, scan) = match wlan(&links) {
        Some(w) => (net::wpa(&w.name, &["status"]).map(|t| net::parse_wpa_status(&t)), net::wpa(&w.name, &["scan_results"]).map(|t| net::parse_scan_results(&t)).unwrap_or_default()),
        None => (None, Vec::new()),
    };
    state(&links, default_iface.as_deref(), wpa.as_ref(), &scan)
}

#[cfg(target_os = "freebsd")]
fn wlan_name() -> Result<String> {
    wlan(&net::interfaces()).map(|w| w.name.clone()).ok_or_else(|| anyhow!(mandara_i18n::t("No Wi-Fi device")))
}

/// `wpa_cli` answers `OK` / `FAIL`; anything else is its error text.
#[cfg(target_os = "freebsd")]
fn ok(out: Option<String>, what: &str) -> Result<String> {
    match out {
        Some(t) if t.trim() == "OK" => Ok(what.into()),
        Some(t) => Err(anyhow!("{}", t.trim())),
        None => Err(anyhow!(mandara_i18n::t("wpa_supplicant isn't running, or its control socket isn't open to you (ctrl_interface_group in wpa_supplicant.conf)"))),
    }
}

#[cfg(target_os = "freebsd")]
fn execute(command: NetworkCommand) -> Result<String> {
    let iface = wlan_name()?;
    match command {
        NetworkCommand::SetWifi(on) => {
            let status = std::process::Command::new(crate::freebsd::power::PKEXEC).args([net::IFCONFIG, &iface, if on { "up" } else { "down" }]).stdin(std::process::Stdio::null()).status()?;
            if !status.success() {
                return Err(anyhow!("ifconfig {iface}: {status}"));
            }
            Ok(format!("wifi {}", if on { "on" } else { "off" }))
        }
        NetworkCommand::Scan => ok(net::wpa(&iface, &["scan"]), "scan requested"),
        NetworkCommand::Disconnect => ok(net::wpa(&iface, &["disconnect"]), "disconnected"),
        NetworkCommand::Connect { ssid, password } => {
            if password.is_none()
                && let Some(id) = net::wpa(&iface, &["list_networks"]).as_deref().and_then(|l| saved_network(l, &ssid))
            {
                return ok(net::wpa(&iface, &["select_network", &id.to_string()]), &format!("activating saved network {ssid}"));
            }
            let id: u32 = net::wpa(&iface, &["add_network"]).and_then(|t| t.trim().parse().ok()).ok_or_else(|| anyhow!(mandara_i18n::t("wpa_supplicant didn't add the network")))?;
            let out = net::wpa_session(&iface, &net::join_commands(id, &ssid, password.as_deref()))?;
            if out.contains("FAIL") {
                let _ = net::wpa(&iface, &["remove_network", &id.to_string()]);
                return Err(anyhow!(mandara_i18n::tf("wpa_supplicant refused {ssid}", &[("ssid", &ssid)])));
            }
            // Kept for the next boot only with `update_config=1` in
            // wpa_supplicant.conf; the network joins either way.
            let _ = net::wpa(&iface, &["save_config"]);
            Ok(format!("connecting to {ssid}"))
        }
    }
}

/// Waits until the routing socket reports a change (or forever without one).
#[cfg(target_os = "freebsd")]
async fn route_change(sock: &Option<async_io::Async<std::os::fd::OwnedFd>>, buf: &mut [u8]) {
    let Some(sock) = sock else { return futures::future::pending().await };
    if sock.readable().await.is_err() {
        return futures::future::pending().await;
    }
    // Drain everything queued: one re-read answers them all.
    while rustix::io::read(sock.get_ref(), &mut *buf).is_ok_and(|n| n > 0) {}
}

#[cfg(target_os = "freebsd")]
pub(super) async fn run(tx: async_channel::Sender<NetworkState>, rx: async_channel::Receiver<Envelope<NetworkCommand>>) {
    use futures::FutureExt as _;
    let sock = net::route_socket().ok().and_then(|fd| async_io::Async::new(fd).ok());
    let mut buf = vec![0u8; ROUTE_READ];
    let mut last: Option<NetworkState> = None;
    loop {
        let state = crate::runtime::unblock(read).await;
        if last.as_ref() != Some(&state) {
            if tx.send(state.clone()).await.is_err() {
                return;
            }
            last = Some(state);
        }
        futures::select! {
            cmd = rx.recv().fuse() => match cmd {
                Ok(Envelope { command, reply: r }) => {
                    let result = crate::runtime::unblock(move || execute(command)).await.map_err(|e| format!("{e:#}"));
                    reply(&r, result);
                }
                Err(_) => return,
            },
            _ = route_change(&sock, &mut buf).fuse() => crate::runtime::sleep(DEBOUNCE).await,
            _ = crate::runtime::sleep(POLL).fuse() => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn link(name: &str, wireless: bool, status: &str, inet: bool) -> Interface {
        Interface { name: name.into(), up: true, running: true, wireless, status: Some(status.into()), inet: if inet { vec![("10.0.0.2".into(), 24)] } else { Vec::new() }, ..Interface::default() }
    }

    #[test]
    fn wired_vm_reads_as_ethernet() {
        let links = net::parse_ifconfig("vtnet0: flags=1008843<UP,BROADCAST,RUNNING,SIMPLEX,MULTICAST,LOWER_UP> metric 0 mtu 1500\n\tinet 10.0.2.15 netmask 0xffffff00 broadcast 10.0.2.255\n\tstatus: active\nlo0: flags=1008049<UP,LOOPBACK,RUNNING,MULTICAST,LOWER_UP> metric 0 mtu 16384\n\tinet 127.0.0.1 netmask 0xff000000\n");
        let s = state(&links, Some("vtnet0"), None, &[]);
        assert_eq!((s.available, s.connectivity, s.primary.as_str(), s.wifi_hardware), (true, Connectivity::Ethernet, "vtnet0", false));
    }

    #[test]
    fn wifi_reads_its_network_and_neighbours() {
        let wlan0 = Interface { ssid: Some("Corniche Cafe".into()), ..link("wlan0", true, "associated", true) };
        let wpa = net::parse_wpa_status("wpa_state=COMPLETED\nssid=Corniche Cafe\n");
        let scan = net::parse_scan_results("bssid / frequency / signal level / flags / ssid\n00:aa:bb:cc:dd:ee\t2437\t62\t[WPA2-PSK-CCMP][ESS]\tCorniche Cafe\n00:aa:bb:cc:dd:ef\t5180\t70\t[WPA2-PSK-CCMP][ESS]\tCorniche Cafe\n11:22:33:44:55:66\t2412\t30\t[ESS]\tMontaza\n");
        let s = state(&[wlan0], Some("wlan0"), Some(&wpa), &scan);
        assert_eq!((s.connectivity, s.primary.as_str(), s.wifi_enabled), (Connectivity::Wifi, "Corniche Cafe", true));
        assert_eq!(s.access_points.len(), 2, "one row per network, its strongest radio");
        assert!(s.access_points[0].active && s.access_points[0].frequency_mhz == 5180);
        assert_eq!(s.strength, s.access_points[0].strength);
        assert!(!s.access_points[1].secured);
    }

    #[test]
    fn connecting_disconnected_and_vpns() {
        let down = Interface { up: false, running: false, ..link("wlan0", true, "no carrier", false) };
        let wpa = net::parse_wpa_status("wpa_state=ASSOCIATING\n");
        assert_eq!(state(std::slice::from_ref(&down), None, Some(&wpa), &[]).connectivity, Connectivity::Connecting);
        let s = state(&[down, link("wg0", false, "active", true)], None, None, &[]);
        assert_eq!((s.connectivity, s.wifi_enabled, s.vpns.as_slice()), (Connectivity::Disconnected, false, &["wg0".to_string()][..]));
    }

    #[test]
    fn finds_saved_networks() {
        let list = "network id / ssid / bssid / flags\n0\tHome\tany\t\n1\tCorniche Cafe\tany\t[CURRENT]\n";
        assert_eq!(saved_network(list, "Corniche Cafe"), Some(1));
        assert_eq!(saved_network(list, "Montaza"), None);
    }
}
