//! NetworkManager administration: VPNs (activate / deactivate, WireGuard
//! and OpenVPN profile import), the Wi-Fi hotspot, saved networks (forget,
//! autoconnect, priority, hidden networks), IPv4 / DNS editing and device
//! details.
//!
//! Everything goes through NetworkManager's settings API over D-Bus; the
//! profiles live in NetworkManager, never in Mandara's config. Calls that
//! change settings carry `ALLOW_INTERACTIVE_AUTHORIZATION`, so polkit asks
//! for a password (through Mandara's agent) when the system requires one.
//!
//! The logic is written against [`NmBackend`] so it is unit-tested with
//! [`mock::MockNm`]; [`DbusNm`] is the real system-bus backend. Its methods
//! block: call them from a plain thread (CLI handlers, Studio background
//! tasks), never from a service executor task.

use std::collections::HashMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use anyhow::{Context as _, Result, anyhow, bail};
use serde::Serialize;
use zbus::proxy::MethodFlags;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};

/// `a{sa{sv}}`: setting name → property → value.
pub type Settings = HashMap<String, HashMap<String, OwnedValue>>;

const NM: &str = "org.freedesktop.NetworkManager";
const NM_PATH: &str = "/org/freedesktop/NetworkManager";
const SETTINGS_PATH: &str = "/org/freedesktop/NetworkManager/Settings";
const SETTINGS_IFACE: &str = "org.freedesktop.NetworkManager.Settings";
const CONNECTION_IFACE: &str = "org.freedesktop.NetworkManager.Settings.Connection";
const ACTIVE_IFACE: &str = "org.freedesktop.NetworkManager.Connection.Active";
const DEVICE_IFACE: &str = "org.freedesktop.NetworkManager.Device";
const IP4_IFACE: &str = "org.freedesktop.NetworkManager.IP4Config";
const OPENVPN_SERVICE: &str = "org.freedesktop.NetworkManager.openvpn";
/// Where NetworkManager's VPN plugins register their service names.
const VPN_PLUGIN_DIRS: [&str; 2] = ["/usr/lib/NetworkManager/VPN", "/etc/NetworkManager/VPN"];
/// The connection Mandara's hotspot reuses (one profile, never a pile).
pub const HOTSPOT_ID: &str = "Mandara Hotspot";
/// `NMDeviceType`: Ethernet and Wi-Fi.
const DEVICE_TYPE_ETHERNET: u32 = 1;
const DEVICE_TYPE_WIFI: u32 = 2;
/// `NMActiveConnectionState`: ACTIVATING / ACTIVATED.
const ACTIVE_ACTIVATING: u32 = 1;
const ACTIVE_ACTIVATED: u32 = 2;
/// Linux interface names are at most 15 bytes.
const IFNAME_MAX: usize = 15;
/// Secret settings merged before an update (an update without them would
/// drop stored passwords).
const SECRET_SETTINGS: [&str; 4] = ["802-11-wireless-security", "802-1x", "vpn", "wireguard"];
/// Generated hotspot passwords: length and alphabet (no look-alikes).
const HOTSPOT_PASSWORD_LEN: usize = 12;
const PASSWORD_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyzABCDEFGHJKLMNPQRSTUVWXYZ23456789";

fn ov<'a>(v: impl Into<Value<'a>>) -> OwnedValue {
    OwnedValue::try_from(v.into()).expect("plain values always convert")
}

fn get<'a>(s: &'a Settings, setting: &str, key: &str) -> Option<&'a OwnedValue> {
    s.get(setting)?.get(key)
}

pub fn get_str(s: &Settings, setting: &str, key: &str) -> Option<String> {
    String::try_from(get(s, setting, key)?.try_clone().ok()?).ok()
}

fn get_bool(s: &Settings, setting: &str, key: &str) -> Option<bool> {
    bool::try_from(get(s, setting, key)?).ok()
}

fn get_i32(s: &Settings, setting: &str, key: &str) -> Option<i32> {
    i32::try_from(get(s, setting, key)?).ok()
}

fn get_u64(s: &Settings, setting: &str, key: &str) -> Option<u64> {
    u64::try_from(get(s, setting, key)?).ok()
}

fn get_bytes(s: &Settings, setting: &str, key: &str) -> Option<Vec<u8>> {
    Vec::<u8>::try_from(get(s, setting, key)?.try_clone().ok()?).ok()
}

fn set(s: &mut Settings, setting: &str, key: &str, v: OwnedValue) {
    s.entry(setting.to_string()).or_default().insert(key.to_string(), v);
}

pub fn clone_settings(s: &Settings) -> Settings {
    s.iter().map(|(k, m)| (k.clone(), m.iter().filter_map(|(k, v)| Some((k.clone(), v.try_clone().ok()?))).collect())).collect()
}

/// A saved connection profile.
pub struct Saved {
    pub path: String,
    pub settings: Settings,
}

impl Saved {
    pub fn id(&self) -> String {
        get_str(&self.settings, "connection", "id").unwrap_or_default()
    }
    pub fn uuid(&self) -> String {
        get_str(&self.settings, "connection", "uuid").unwrap_or_default()
    }
    pub fn kind(&self) -> String {
        get_str(&self.settings, "connection", "type").unwrap_or_default()
    }
}

/// An active connection.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Active {
    pub path: String,
    pub uuid: String,
    pub state: u32,
}

/// One network device with its addresses (read-only details).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct DeviceDetails {
    pub interface: String,
    /// `ethernet` / `wifi`.
    pub kind: String,
    pub mac: String,
    pub connection: Option<String>,
    /// `192.168.1.20/24`.
    pub addresses: Vec<String>,
    pub gateway: Option<String>,
    pub dns: Vec<String>,
    /// Link speed, Mb/s.
    pub speed_mbps: Option<u32>,
}

/// What NetworkManager does; [`DbusNm`] for real, [`mock::MockNm`] in tests.
pub trait NmBackend: Send + Sync {
    fn saved(&self) -> Result<Vec<Saved>>;
    fn active(&self) -> Result<Vec<Active>>;
    /// Stored secrets of one setting (empty when none / not allowed).
    fn secrets(&self, path: &str, setting: &str) -> Settings;
    fn add(&self, settings: Settings) -> Result<String>;
    fn update(&self, path: &str, settings: Settings) -> Result<()>;
    fn delete(&self, path: &str) -> Result<()>;
    fn activate(&self, path: &str, device: Option<&str>) -> Result<String>;
    fn add_and_activate(&self, settings: Settings, device: &str) -> Result<String>;
    fn deactivate(&self, active: &str) -> Result<()>;
    fn wifi_device(&self) -> Result<Option<String>>;
    fn devices(&self) -> Result<Vec<DeviceDetails>>;
}

fn find<'a>(saved: &'a [Saved], query: &str) -> Option<&'a Saved> {
    let q = query.trim();
    saved.iter().find(|s| s.uuid() == q).or_else(|| saved.iter().find(|s| s.id() == q)).or_else(|| saved.iter().find(|s| s.id().eq_ignore_ascii_case(q)))
}

fn is_up(active: &[Active], uuid: &str) -> Option<u32> {
    active.iter().find(|a| a.uuid == uuid).map(|a| a.state)
}

/// Updates a profile with its stored secrets merged in, so editing (say)
/// autoconnect never drops a saved password.
fn update_merged(b: &dyn NmBackend, saved: &Saved, edit: impl FnOnce(&mut Settings)) -> Result<()> {
    let mut settings = clone_settings(&saved.settings);
    for name in SECRET_SETTINGS {
        if settings.contains_key(name) {
            for (k, v) in b.secrets(&saved.path, name).remove(name).unwrap_or_default() {
                settings.entry(name.to_string()).or_default().insert(k, v);
            }
        }
    }
    edit(&mut settings);
    b.update(&saved.path, settings)
}

// ── VPN ─────────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct VpnInfo {
    pub name: String,
    pub uuid: String,
    /// `wireguard`, `openvpn`, or the plugin's short name.
    pub kind: String,
    pub active: bool,
    pub connecting: bool,
}

fn vpn_kind(s: &Saved) -> Option<String> {
    match s.kind().as_str() {
        "wireguard" => Some("wireguard".into()),
        "vpn" => {
            let service = get_str(&s.settings, "vpn", "service-type").unwrap_or_default();
            Some(service.rsplit('.').next().unwrap_or("vpn").to_string())
        }
        _ => None,
    }
}

pub fn vpns(b: &dyn NmBackend) -> Result<Vec<VpnInfo>> {
    let (saved, active) = (b.saved()?, b.active()?);
    let mut out: Vec<VpnInfo> = saved
        .iter()
        .filter_map(|s| {
            let kind = vpn_kind(s)?;
            let state = is_up(&active, &s.uuid());
            Some(VpnInfo { name: s.id(), uuid: s.uuid(), kind, active: state == Some(ACTIVE_ACTIVATED), connecting: state == Some(ACTIVE_ACTIVATING) })
        })
        .collect();
    out.sort_by_key(|v| v.name.to_lowercase());
    Ok(out)
}

/// Connects (or disconnects) the VPN named / with uuid `query`. Returns its
/// name.
pub fn vpn_set(b: &dyn NmBackend, query: &str, up: bool) -> Result<String> {
    let saved = b.saved()?;
    let s = find(&saved, query).filter(|s| vpn_kind(s).is_some()).ok_or_else(|| anyhow!(mandara_i18n::tf("No VPN named \"{name}\"", &[("name", &query)])))?;
    let active = b.active()?;
    match (up, active.iter().find(|a| a.uuid == s.uuid())) {
        (true, Some(_)) => {}
        (true, None) => {
            b.activate(&s.path, None)?;
        }
        (false, Some(a)) => b.deactivate(&a.path)?,
        (false, None) => {}
    }
    Ok(s.id())
}

/// A parsed WireGuard `wg-quick` profile.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WgProfile {
    pub private_key: String,
    pub addresses: Vec<(IpAddr, u32)>,
    pub dns: Vec<IpAddr>,
    pub listen_port: Option<u32>,
    pub mtu: Option<u32>,
    pub peers: Vec<WgPeer>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct WgPeer {
    pub public_key: String,
    pub preshared_key: Option<String>,
    pub endpoint: Option<String>,
    pub allowed_ips: Vec<String>,
    pub keepalive: Option<u32>,
}

fn parse_cidr(s: &str) -> Result<(IpAddr, u32)> {
    let (ip, prefix) = match s.split_once('/') {
        Some((ip, p)) => (ip.trim(), Some(p.trim())),
        None => (s.trim(), None),
    };
    let ip: IpAddr = ip.parse().with_context(|| format!("bad address {s:?}"))?;
    let max = if ip.is_ipv4() { 32 } else { 128 };
    let prefix = prefix.map(|p| p.parse::<u32>()).transpose().with_context(|| format!("bad prefix in {s:?}"))?.unwrap_or(max);
    if prefix > max {
        bail!("bad prefix in {s:?}");
    }
    Ok((ip, prefix))
}

/// Parses a `wg-quick` `.conf` (`[Interface]` + `[Peer]` sections).
pub fn parse_wireguard(text: &str) -> Result<WgProfile> {
    let mut wg = WgProfile::default();
    let mut section = "";
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or_default().trim();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            section = if line.eq_ignore_ascii_case("[Interface]") {
                "interface"
            } else if line.eq_ignore_ascii_case("[Peer]") {
                wg.peers.push(WgPeer::default());
                "peer"
            } else {
                ""
            };
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim().to_ascii_lowercase(), v.trim());
        let list = || v.split(',').map(str::trim).filter(|s| !s.is_empty());
        match (section, k.as_str()) {
            ("interface", "privatekey") => wg.private_key = v.into(),
            ("interface", "address") => {
                for a in list() {
                    wg.addresses.push(parse_cidr(a)?);
                }
            }
            ("interface", "dns") => wg.dns.extend(list().filter_map(|d| d.parse::<IpAddr>().ok())),
            ("interface", "listenport") => wg.listen_port = v.parse().ok(),
            ("interface", "mtu") => wg.mtu = v.parse().ok(),
            ("peer", key) => {
                let Some(peer) = wg.peers.last_mut() else { continue };
                match key {
                    "publickey" => peer.public_key = v.into(),
                    "presharedkey" => peer.preshared_key = Some(v.into()),
                    "endpoint" => peer.endpoint = Some(v.into()),
                    "allowedips" => peer.allowed_ips.extend(list().map(String::from)),
                    "persistentkeepalive" => peer.keepalive = v.parse().ok(),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    if wg.private_key.is_empty() {
        bail!("no PrivateKey in [Interface]");
    }
    if wg.peers.iter().all(|p| p.public_key.is_empty()) {
        bail!("no [Peer] with a PublicKey");
    }
    Ok(wg)
}

/// A valid interface name from a profile name (`My Office VPN` → `wg-my-office`).
pub fn interface_name(name: &str) -> String {
    let mut s: String = name.chars().filter_map(|c| if c.is_ascii_alphanumeric() { Some(c.to_ascii_lowercase()) } else if c == '-' || c == '_' || c == ' ' { Some('-') } else { None }).collect();
    while s.contains("--") {
        s = s.replace("--", "-");
    }
    let s = s.trim_matches('-');
    let base = if s.starts_with("wg") { s.to_string() } else { format!("wg-{s}") };
    let base = base.trim_end_matches('-');
    base.chars().take(IFNAME_MAX).collect::<String>().trim_end_matches('-').to_string()
}

fn address_data(list: &[(IpAddr, u32)]) -> OwnedValue {
    let items: Vec<HashMap<String, Value<'static>>> =
        list.iter().map(|(ip, prefix)| HashMap::from([("address".to_string(), Value::from(ip.to_string())), ("prefix".to_string(), Value::from(*prefix))])).collect();
    ov(items)
}

/// NetworkManager's legacy IPv4 DNS encoding: `u32`s in network byte order.
fn dns4(list: &[IpAddr]) -> Vec<u32> {
    list.iter().filter_map(|ip| match ip {
        IpAddr::V4(v4) => Some(u32::from_ne_bytes(v4.octets())),
        IpAddr::V6(_) => None,
    })
    .collect()
}

fn dns6(list: &[IpAddr]) -> Vec<Vec<u8>> {
    list.iter().filter_map(|ip| match ip {
        IpAddr::V6(v6) => Some(v6.octets().to_vec()),
        IpAddr::V4(_) => None,
    })
    .collect()
}

fn base_connection(id: &str, kind: &str) -> Settings {
    let mut s = Settings::new();
    set(&mut s, "connection", "id", ov(id.to_string()));
    set(&mut s, "connection", "type", ov(kind.to_string()));
    set(&mut s, "connection", "autoconnect", ov(false));
    s
}

/// NetworkManager settings for a WireGuard profile.
pub fn wireguard_settings(name: &str, wg: &WgProfile) -> Settings {
    let mut s = base_connection(name, "wireguard");
    set(&mut s, "connection", "interface-name", ov(interface_name(name)));
    set(&mut s, "wireguard", "private-key", ov(wg.private_key.clone()));
    // 0 = stored by NetworkManager (no secret agent needed to connect).
    set(&mut s, "wireguard", "private-key-flags", ov(0u32));
    if let Some(p) = wg.listen_port {
        set(&mut s, "wireguard", "listen-port", ov(p));
    }
    if let Some(m) = wg.mtu {
        set(&mut s, "wireguard", "mtu", ov(m));
    }
    let peers: Vec<HashMap<String, Value<'static>>> = wg
        .peers
        .iter()
        .filter(|p| !p.public_key.is_empty())
        .map(|p| {
            let mut m: HashMap<String, Value<'static>> = HashMap::new();
            m.insert("public-key".into(), Value::from(p.public_key.clone()));
            m.insert("allowed-ips".into(), Value::from(p.allowed_ips.clone()));
            if let Some(e) = &p.endpoint {
                m.insert("endpoint".into(), Value::from(e.clone()));
            }
            if let Some(k) = &p.preshared_key {
                m.insert("preshared-key".into(), Value::from(k.clone()));
                m.insert("preshared-key-flags".into(), Value::from(0u32));
            }
            if let Some(k) = p.keepalive {
                m.insert("persistent-keepalive".into(), Value::from(k));
            }
            m
        })
        .collect();
    set(&mut s, "wireguard", "peers", ov(peers));
    let v4: Vec<(IpAddr, u32)> = wg.addresses.iter().filter(|(ip, _)| ip.is_ipv4()).copied().collect();
    let v6: Vec<(IpAddr, u32)> = wg.addresses.iter().filter(|(ip, _)| ip.is_ipv6()).copied().collect();
    if v4.is_empty() {
        set(&mut s, "ipv4", "method", ov("disabled".to_string()));
    } else {
        set(&mut s, "ipv4", "method", ov("manual".to_string()));
        set(&mut s, "ipv4", "address-data", address_data(&v4));
        let d = dns4(&wg.dns);
        if !d.is_empty() {
            set(&mut s, "ipv4", "dns", ov(d));
        }
    }
    if v6.is_empty() {
        set(&mut s, "ipv6", "method", ov("disabled".to_string()));
    } else {
        set(&mut s, "ipv6", "method", ov("manual".to_string()));
        set(&mut s, "ipv6", "address-data", address_data(&v6));
        let d = dns6(&wg.dns);
        if !d.is_empty() {
            set(&mut s, "ipv6", "dns", ov(d));
        }
    }
    s
}

/// A parsed OpenVPN client profile, as NetworkManager-openvpn's `vpn.data`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OvpnProfile {
    pub data: Vec<(String, String)>,
    /// Inline `<ca>`, `<cert>`, `<key>`, `<tls-auth>`, `<tls-crypt>` blocks.
    pub inline: Vec<(String, String)>,
    pub needs_password: bool,
}

/// Parses an `.ovpn` file into NetworkManager-openvpn data keys. Inline
/// certificate blocks are returned separately (written to files on import).
pub fn parse_openvpn(text: &str) -> Result<OvpnProfile> {
    let mut p = OvpnProfile::default();
    let mut remotes: Vec<String> = Vec::new();
    let mut proto_tcp = false;
    let mut port: Option<String> = None;
    let mut has_cert = false;
    let mut block: Option<(String, String)> = None;
    let push = |p: &mut OvpnProfile, k: &str, v: &str| p.data.push((k.to_string(), v.to_string()));
    for raw in text.lines() {
        let line = raw.trim();
        if let Some((tag, body)) = block.as_mut() {
            if line == format!("</{tag}>") {
                p.inline.push((tag.clone(), std::mem::take(body)));
                block = None;
            } else {
                body.push_str(raw);
                body.push('\n');
            }
            continue;
        }
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(tag) = line.strip_prefix('<').and_then(|l| l.strip_suffix('>')) {
            block = Some((tag.to_string(), String::new()));
            continue;
        }
        let mut words = line.split_whitespace();
        let key = words.next().unwrap_or_default();
        let args: Vec<&str> = words.collect();
        match key {
            "remote" => {
                if let Some(host) = args.first() {
                    let mut r = host.to_string();
                    if let Some(port) = args.get(1) {
                        r.push(':');
                        r.push_str(port);
                        if let Some(proto) = args.get(2) {
                            r.push(':');
                            r.push_str(proto);
                        }
                    }
                    remotes.push(r);
                }
            }
            "port" => port = args.first().map(|s| s.to_string()),
            "proto" => proto_tcp = args.first().is_some_and(|p| p.starts_with("tcp")),
            "dev" => push(&mut p, "dev-type", if args.first().is_some_and(|d| d.starts_with("tap")) { "tap" } else { "tun" }),
            "ca" | "cert" | "key" => {
                has_cert |= key != "ca";
                if let Some(f) = args.first() {
                    push(&mut p, key, f);
                }
            }
            "tls-auth" => {
                if let Some(f) = args.first() {
                    push(&mut p, "ta", f);
                }
                if let Some(d) = args.get(1) {
                    push(&mut p, "ta-dir", d);
                }
            }
            "key-direction" => {
                if let Some(d) = args.first() {
                    push(&mut p, "ta-dir", d);
                }
            }
            "tls-crypt" => {
                if let Some(f) = args.first() {
                    push(&mut p, "tls-crypt", f);
                }
            }
            "cipher" | "data-ciphers-fallback" => {
                if let Some(c) = args.first() {
                    push(&mut p, "cipher", c);
                }
            }
            "auth" => {
                if let Some(a) = args.first() {
                    push(&mut p, "auth", a);
                }
            }
            "auth-user-pass" => p.needs_password = true,
            "remote-cert-tls" => {
                if let Some(a) = args.first() {
                    push(&mut p, "remote-cert-tls", a);
                }
            }
            "comp-lzo" => push(&mut p, "comp-lzo", args.first().copied().unwrap_or("adaptive")),
            "compress" => push(&mut p, "compress", args.first().copied().unwrap_or("yes")),
            "tun-mtu" => {
                if let Some(m) = args.first() {
                    push(&mut p, "tunnel-mtu", m);
                }
            }
            "verify-x509-name" => {
                if let Some(n) = args.first() {
                    let kind = args.get(1).copied().unwrap_or("subject");
                    push(&mut p, "verify-x509-name", &format!("{kind}:{n}"));
                }
            }
            _ => {}
        }
    }
    has_cert |= p.inline.iter().any(|(t, _)| t == "cert" || t == "key");
    if remotes.is_empty() {
        bail!("no `remote` server in the profile");
    }
    push(&mut p, "remote", &remotes.join(", "));
    if let Some(port) = port {
        push(&mut p, "port", &port);
    }
    if proto_tcp {
        push(&mut p, "proto-tcp", "yes");
    }
    let kind = match (p.needs_password, has_cert) {
        (true, true) => "password-tls",
        (true, false) => "password",
        (false, _) if p.data.iter().any(|(k, _)| k == "static-key") => "static-key",
        (false, _) => "tls",
    };
    push(&mut p, "connection-type", kind);
    if p.needs_password {
        // Stored by NetworkManager: no secret agent is needed to connect.
        push(&mut p, "password-flags", "0");
    }
    Ok(p)
}

/// Whether NetworkManager's OpenVPN plugin is installed.
pub fn openvpn_plugin_installed() -> bool {
    VPN_PLUGIN_DIRS.iter().any(|d| Path::new(d).join("nm-openvpn-service.name").is_file())
}

/// Credentials asked for at import time (OpenVPN `auth-user-pass`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VpnCredentials {
    pub username: String,
    pub password: String,
}

/// NetworkManager settings for an OpenVPN profile whose inline blocks were
/// written to `cert_dir`.
pub fn openvpn_settings(name: &str, p: &OvpnProfile, cert_dir: &Path, creds: Option<&VpnCredentials>) -> Settings {
    let mut s = base_connection(name, "vpn");
    set(&mut s, "vpn", "service-type", ov(OPENVPN_SERVICE.to_string()));
    let mut data: HashMap<String, String> = p.data.iter().cloned().collect();
    for (tag, _) in &p.inline {
        let key = match tag.as_str() {
            "tls-auth" => "ta",
            other => other,
        };
        data.insert(key.to_string(), cert_dir.join(format!("{tag}.pem")).display().to_string());
    }
    let mut secrets: HashMap<String, String> = HashMap::new();
    if let Some(c) = creds {
        if !c.username.is_empty() {
            data.insert("username".into(), c.username.clone());
        }
        if !c.password.is_empty() {
            secrets.insert("password".into(), c.password.clone());
        }
    }
    set(&mut s, "vpn", "data", ov(data));
    if !secrets.is_empty() {
        set(&mut s, "vpn", "secrets", ov(secrets));
    }
    set(&mut s, "ipv4", "method", ov("auto".to_string()));
    s
}

/// Where imported VPN certificates are kept.
pub fn vpn_cert_dir(name: &str) -> PathBuf {
    mandara_config::paths::data_dir().join("vpn").join(mandara_config::profiles::slug(name))
}

/// Imports a WireGuard `.conf` or OpenVPN `.ovpn` file as a new
/// NetworkManager profile. Returns its name.
pub fn import_vpn(b: &dyn NmBackend, file: &Path, name: Option<&str>, creds: Option<&VpnCredentials>) -> Result<String> {
    let text = std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let stem = file.file_stem().and_then(|s| s.to_str()).unwrap_or("VPN").to_string();
    let name = name.map(str::trim).filter(|n| !n.is_empty()).map(String::from).unwrap_or(stem);
    if b.saved()?.iter().any(|s| s.id() == name) {
        bail!("{}", mandara_i18n::tf("A connection named \"{name}\" already exists", &[("name", &name)]));
    }
    let ext = file.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    let settings = if ext == "ovpn" || (ext != "conf" && text.contains("\nremote ")) || text.starts_with("remote ") {
        if !openvpn_plugin_installed() {
            bail!("{}", mandara_i18n::t("NetworkManager's OpenVPN plugin isn't installed (networkmanager-openvpn)"));
        }
        let profile = parse_openvpn(&text)?;
        let dir = vpn_cert_dir(&name);
        if !profile.inline.is_empty() {
            write_private_dir(&dir)?;
            for (tag, body) in &profile.inline {
                write_private_file(&dir.join(format!("{tag}.pem")), body.as_bytes())?;
            }
        }
        openvpn_settings(&name, &profile, &dir, creds)
    } else {
        wireguard_settings(&name, &parse_wireguard(&text)?)
    };
    b.add(settings)?;
    Ok(name)
}

fn write_private_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
    Ok(())
}

fn write_private_file(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(path)?;
    f.write_all(bytes)?;
    Ok(())
}

/// Deletes a VPN profile (and certificates Mandara imported for it).
pub fn vpn_remove(b: &dyn NmBackend, query: &str) -> Result<String> {
    let saved = b.saved()?;
    let s = find(&saved, query).filter(|s| vpn_kind(s).is_some()).ok_or_else(|| anyhow!(mandara_i18n::tf("No VPN named \"{name}\"", &[("name", &query)])))?;
    b.delete(&s.path)?;
    let dir = vpn_cert_dir(&s.id());
    if dir.starts_with(mandara_config::paths::data_dir()) && dir.is_dir() {
        let _ = std::fs::remove_dir_all(dir);
    }
    Ok(s.id())
}

// ── Saved Wi-Fi networks ────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct SavedWifi {
    pub name: String,
    pub uuid: String,
    pub ssid: String,
    pub autoconnect: bool,
    pub priority: i32,
    pub hidden: bool,
    pub secured: bool,
    /// Unix time of the last connection (0 = never).
    pub last_used: u64,
    pub active: bool,
    /// An access point profile (Mandara's hotspot or another).
    pub hotspot: bool,
}

pub fn saved_wifi(b: &dyn NmBackend) -> Result<Vec<SavedWifi>> {
    let (saved, active) = (b.saved()?, b.active()?);
    let mut out: Vec<SavedWifi> = saved
        .iter()
        .filter(|s| s.kind() == "802-11-wireless")
        .map(|s| SavedWifi {
            name: s.id(),
            uuid: s.uuid(),
            ssid: get_bytes(&s.settings, "802-11-wireless", "ssid").map(|b| String::from_utf8_lossy(&b).trim_end_matches('\0').to_string()).unwrap_or_default(),
            autoconnect: get_bool(&s.settings, "connection", "autoconnect").unwrap_or(true),
            priority: get_i32(&s.settings, "connection", "autoconnect-priority").unwrap_or(0),
            hidden: get_bool(&s.settings, "802-11-wireless", "hidden").unwrap_or(false),
            secured: s.settings.contains_key("802-11-wireless-security"),
            last_used: get_u64(&s.settings, "connection", "timestamp").unwrap_or(0),
            active: is_up(&active, &s.uuid()) == Some(ACTIVE_ACTIVATED),
            hotspot: get_str(&s.settings, "802-11-wireless", "mode").as_deref() == Some("ap"),
        })
        .collect();
    out.sort_by(|a, b| b.active.cmp(&a.active).then(b.last_used.cmp(&a.last_used)).then(a.name.cmp(&b.name)));
    Ok(out)
}

fn saved_by(b: &dyn NmBackend, query: &str) -> Result<Saved> {
    let mut saved = b.saved()?;
    let i = {
        let s = find(&saved, query).ok_or_else(|| anyhow!(mandara_i18n::tf("No saved connection \"{name}\"", &[("name", &query)])))?;
        saved.iter().position(|x| std::ptr::eq(x, s)).unwrap_or(0)
    };
    Ok(saved.swap_remove(i))
}

/// Forgets a saved network. Returns its name.
pub fn forget(b: &dyn NmBackend, query: &str) -> Result<String> {
    let s = saved_by(b, query)?;
    b.delete(&s.path)?;
    Ok(s.id())
}

pub fn set_autoconnect(b: &dyn NmBackend, query: &str, on: bool) -> Result<String> {
    let s = saved_by(b, query)?;
    update_merged(b, &s, |st| set(st, "connection", "autoconnect", ov(on)))?;
    Ok(s.id())
}

/// Autoconnect priority bounds NetworkManager accepts.
pub const PRIORITY_RANGE: (i32, i32) = (-999, 999);

pub fn set_priority(b: &dyn NmBackend, query: &str, priority: i32) -> Result<String> {
    let s = saved_by(b, query)?;
    let p = priority.clamp(PRIORITY_RANGE.0, PRIORITY_RANGE.1);
    update_merged(b, &s, |st| set(st, "connection", "autoconnect-priority", ov(p)))?;
    Ok(s.id())
}

/// Connects to a hidden network (a new profile with `hidden=true`).
pub fn connect_hidden(b: &dyn NmBackend, ssid: &str, password: Option<&str>) -> Result<String> {
    let ssid = ssid.trim();
    if ssid.is_empty() {
        bail!("{}", mandara_i18n::t("The network name is empty"));
    }
    let dev = b.wifi_device()?.ok_or_else(|| anyhow!(mandara_i18n::t("No Wi-Fi device")))?;
    let mut s = base_connection(ssid, "802-11-wireless");
    set(&mut s, "connection", "autoconnect", ov(true));
    set(&mut s, "802-11-wireless", "ssid", ov(ssid.as_bytes().to_vec()));
    set(&mut s, "802-11-wireless", "hidden", ov(true));
    if let Some(p) = password.filter(|p| !p.is_empty()) {
        set(&mut s, "802-11-wireless-security", "key-mgmt", ov("wpa-psk".to_string()));
        set(&mut s, "802-11-wireless-security", "psk", ov(p.to_string()));
    }
    b.add_and_activate(s, &dev)?;
    Ok(ssid.to_string())
}

// ── Hotspot ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HotspotInfo {
    pub ssid: String,
    /// Only filled when asked for (sharing the QR code).
    pub password: Option<String>,
    pub band: Option<String>,
    pub active: bool,
}

fn hotspot_profile(saved: &[Saved]) -> Option<&Saved> {
    saved.iter().find(|s| s.id() == HOTSPOT_ID)
}

/// The hotspot profile and whether it runs; `with_password` reads its
/// stored passphrase (for the QR code).
pub fn hotspot_status(b: &dyn NmBackend, with_password: bool) -> Result<Option<HotspotInfo>> {
    let saved = b.saved()?;
    let Some(s) = hotspot_profile(&saved) else { return Ok(None) };
    let active = b.active()?;
    let password = with_password.then(|| get_str(&b.secrets(&s.path, "802-11-wireless-security"), "802-11-wireless-security", "psk")).flatten();
    Ok(Some(HotspotInfo {
        ssid: get_bytes(&s.settings, "802-11-wireless", "ssid").map(|b| String::from_utf8_lossy(&b).to_string()).unwrap_or_default(),
        password,
        band: get_str(&s.settings, "802-11-wireless", "band"),
        active: is_up(&active, &s.uuid()).is_some(),
    }))
}

/// A random passphrase without look-alike characters.
pub fn generate_password() -> String {
    let mut bytes = [0u8; HOTSPOT_PASSWORD_LEN];
    if std::fs::File::open("/dev/urandom").and_then(|mut f| std::io::Read::read_exact(&mut f, &mut bytes)).is_err() {
        let seed = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        for (i, b) in bytes.iter_mut().enumerate() {
            *b = (seed >> ((i % 16) * 8)) as u8 ^ (i as u8).wrapping_mul(37);
        }
    }
    bytes.iter().map(|b| PASSWORD_ALPHABET[*b as usize % PASSWORD_ALPHABET.len()] as char).collect()
}

/// Hotspot connection settings (WPA2-PSK, shared IPv4 = NAT + DHCP).
pub fn hotspot_settings(ssid: &str, password: &str, band: Option<&str>) -> Settings {
    let mut s = base_connection(HOTSPOT_ID, "802-11-wireless");
    set(&mut s, "802-11-wireless", "ssid", ov(ssid.as_bytes().to_vec()));
    set(&mut s, "802-11-wireless", "mode", ov("ap".to_string()));
    if let Some(band) = band {
        set(&mut s, "802-11-wireless", "band", ov(band.to_string()));
    }
    set(&mut s, "802-11-wireless-security", "key-mgmt", ov("wpa-psk".to_string()));
    set(&mut s, "802-11-wireless-security", "psk", ov(password.to_string()));
    set(&mut s, "802-11-wireless-security", "proto", ov(vec!["rsn".to_string()]));
    set(&mut s, "802-11-wireless-security", "pairwise", ov(vec!["ccmp".to_string()]));
    set(&mut s, "802-11-wireless-security", "group", ov(vec!["ccmp".to_string()]));
    set(&mut s, "ipv4", "method", ov("shared".to_string()));
    set(&mut s, "ipv6", "method", ov("ignore".to_string()));
    s
}

/// Starts the hotspot (creating or updating Mandara's profile). A missing
/// password keeps the stored one, or generates one. Returns the info with
/// its password.
pub fn hotspot_start(b: &dyn NmBackend, ssid: &str, password: Option<&str>, band: Option<&str>) -> Result<HotspotInfo> {
    let dev = b.wifi_device()?.ok_or_else(|| anyhow!(mandara_i18n::t("No Wi-Fi device")))?;
    let saved = b.saved()?;
    let existing = hotspot_profile(&saved);
    let stored = existing.and_then(|s| get_str(&b.secrets(&s.path, "802-11-wireless-security"), "802-11-wireless-security", "psk"));
    let password = match password.map(str::trim).filter(|p| !p.is_empty()) {
        Some(p) if !mandara_config::connectivity::valid_hotspot_password(p) => bail!("{}", mandara_i18n::t("The password needs 8–63 characters")),
        Some(p) => p.to_string(),
        None => stored.unwrap_or_else(generate_password),
    };
    let settings = hotspot_settings(ssid, &password, band);
    match existing {
        Some(s) => {
            let mut merged = settings;
            if let Some(uuid) = get_str(&s.settings, "connection", "uuid") {
                set(&mut merged, "connection", "uuid", ov(uuid));
            }
            b.update(&s.path, merged)?;
            b.activate(&s.path, Some(&dev))?;
        }
        None => {
            b.add_and_activate(settings, &dev)?;
        }
    }
    Ok(HotspotInfo { ssid: ssid.to_string(), password: Some(password), band: band.map(String::from), active: true })
}

pub fn hotspot_stop(b: &dyn NmBackend) -> Result<bool> {
    let saved = b.saved()?;
    let Some(s) = hotspot_profile(&saved) else { return Ok(false) };
    let active = b.active()?;
    match active.iter().find(|a| a.uuid == s.uuid()) {
        Some(a) => {
            b.deactivate(&a.path)?;
            Ok(true)
        }
        None => Ok(false),
    }
}

// ── IPv4 / DNS ──────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Ipv4Settings {
    /// `auto` (DHCP) or `manual`.
    pub method: String,
    /// `192.168.1.20/24`.
    pub addresses: Vec<String>,
    pub gateway: Option<String>,
    pub dns: Vec<String>,
    /// With DHCP: ignore the DNS servers it hands out (use `dns` only).
    pub ignore_auto_dns: bool,
}

pub fn ipv4(b: &dyn NmBackend, query: &str) -> Result<Ipv4Settings> {
    let s = saved_by(b, query)?;
    let st = &s.settings;
    let addresses = get(st, "ipv4", "address-data")
        .and_then(|v| Vec::<HashMap<String, OwnedValue>>::try_from(v.try_clone().ok()?).ok())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|m| {
            let addr = String::try_from(m.get("address")?.try_clone().ok()?).ok()?;
            let prefix = u32::try_from(m.get("prefix")?).ok()?;
            Some(format!("{addr}/{prefix}"))
        })
        .collect();
    let dns = get(st, "ipv4", "dns")
        .and_then(|v| Vec::<u32>::try_from(v.try_clone().ok()?).ok())
        .unwrap_or_default()
        .into_iter()
        .map(|n| std::net::Ipv4Addr::from(n.to_ne_bytes()).to_string())
        .collect();
    Ok(Ipv4Settings {
        method: get_str(st, "ipv4", "method").unwrap_or_else(|| "auto".into()),
        addresses,
        gateway: get_str(st, "ipv4", "gateway").filter(|g| !g.is_empty()),
        dns,
        ignore_auto_dns: get_bool(st, "ipv4", "ignore-auto-dns").unwrap_or(false),
    })
}

/// Validates and writes IPv4 settings. They take effect on the next
/// connect (NetworkManager keeps the running link as it is).
/// The `ipv4` keys the static IP / DNS editor writes (and so replaces).
/// The legacy `addresses` / `dns` forms go too, so NetworkManager never
/// sees two disagreeing copies.
const IPV4_EDITOR_KEYS: &[&str] = &["method", "address-data", "addresses", "gateway", "dns", "dns-data", "ignore-auto-dns"];

pub fn set_ipv4(b: &dyn NmBackend, query: &str, v4: &Ipv4Settings) -> Result<String> {
    let method = match v4.method.as_str() {
        "auto" | "manual" => v4.method.clone(),
        other => bail!("unknown IPv4 method {other:?} (auto or manual)"),
    };
    let mut addresses = Vec::new();
    for a in &v4.addresses {
        let (ip, prefix) = parse_cidr(a)?;
        if !ip.is_ipv4() {
            bail!("{a} is not an IPv4 address");
        }
        addresses.push((ip, if a.contains('/') { prefix } else { 24 }));
    }
    if method == "manual" && addresses.is_empty() {
        bail!("{}", mandara_i18n::t("A manual configuration needs an address"));
    }
    let gateway = v4.gateway.as_deref().map(str::trim).filter(|g| !g.is_empty()).map(|g| g.parse::<std::net::Ipv4Addr>().map(|ip| ip.to_string()).with_context(|| format!("bad gateway {g:?}"))).transpose()?;
    let dns: Vec<IpAddr> = v4.dns.iter().map(|d| d.trim().parse::<std::net::Ipv4Addr>().map(IpAddr::V4).with_context(|| format!("bad DNS server {d:?}"))).collect::<Result<_>>()?;
    let s = saved_by(b, query)?;
    update_merged(b, &s, |st| {
        let ipv4 = st.entry("ipv4".to_string()).or_default();
        // Only the keys this editor owns: routes, route metrics, DNS search
        // domains, never-default and the rest of the profile survive.
        for key in IPV4_EDITOR_KEYS {
            ipv4.remove(*key);
        }
        ipv4.insert("method".into(), ov(method.clone()));
        if method == "manual" {
            ipv4.insert("address-data".into(), address_data(&addresses));
            if let Some(g) = &gateway {
                ipv4.insert("gateway".into(), ov(g.clone()));
            }
        }
        if !dns.is_empty() {
            ipv4.insert("dns".into(), ov(dns4(&dns)));
        }
        ipv4.insert("ignore-auto-dns".into(), ov(v4.ignore_auto_dns && !dns.is_empty()));
    })?;
    Ok(s.id())
}

// ── D-Bus backend ───────────────────────────────────────────────────────────

/// The system-bus NetworkManager.
pub struct DbusNm {
    conn: zbus::Connection,
}

impl DbusNm {
    pub fn connect() -> Result<Self> {
        let conn = crate::runtime::block_on(crate::runtime::system()).context("no system bus")?;
        Ok(Self { conn })
    }

    fn block<T>(&self, f: impl std::future::Future<Output = zbus::Result<T>>) -> Result<T> {
        crate::runtime::block_on(f).map_err(|e| anyhow!(nm_error(e)))
    }

    async fn proxy<'a>(&self, path: &'a str, iface: &'a str) -> zbus::Result<zbus::Proxy<'a>> {
        crate::actor::proxy(&self.conn, NM, path, iface).await
    }
}

/// A readable message for a NetworkManager error.
fn nm_error(e: zbus::Error) -> String {
    match e {
        zbus::Error::MethodError(name, msg, _) => {
            let name = name.to_string();
            if name.ends_with("PermissionDenied") || name.ends_with("NotAuthorized") {
                mandara_i18n::t("Not authorized (the administrator password was refused or no authentication agent answered)").to_string()
            } else {
                msg.unwrap_or(name)
            }
        }
        other => other.to_string(),
    }
}

impl NmBackend for DbusNm {
    fn saved(&self) -> Result<Vec<Saved>> {
        self.block(async {
            let settings = self.proxy(SETTINGS_PATH, SETTINGS_IFACE).await?;
            let paths: Vec<OwnedObjectPath> = settings.call("ListConnections", &()).await?;
            let mut out = Vec::new();
            for p in paths {
                let c = self.proxy(p.as_str(), CONNECTION_IFACE).await?;
                if let Ok(s) = c.call::<_, _, Settings>("GetSettings", &()).await {
                    out.push(Saved { path: p.to_string(), settings: s });
                }
            }
            Ok(out)
        })
    }

    fn active(&self) -> Result<Vec<Active>> {
        self.block(async {
            let nm = self.proxy(NM_PATH, NM).await?;
            let paths: Vec<OwnedObjectPath> = nm.get_property("ActiveConnections").await?;
            let mut out = Vec::new();
            for p in paths {
                let a = self.proxy(p.as_str(), ACTIVE_IFACE).await?;
                out.push(Active { path: p.to_string(), uuid: a.get_property("Uuid").await.unwrap_or_default(), state: a.get_property("State").await.unwrap_or(0) });
            }
            Ok(out)
        })
    }

    fn secrets(&self, path: &str, setting: &str) -> Settings {
        self.block(async {
            let c = self.proxy(path, CONNECTION_IFACE).await?;
            c.call::<_, _, Settings>("GetSecrets", &(setting,)).await
        })
        .unwrap_or_default()
    }

    fn add(&self, settings: Settings) -> Result<String> {
        self.block(async {
            let s = self.proxy(SETTINGS_PATH, SETTINGS_IFACE).await?;
            let p: Option<OwnedObjectPath> = s.call_with_flags("AddConnection", MethodFlags::AllowInteractiveAuth.into(), &(settings,)).await?;
            Ok(p.map(|p| p.to_string()).unwrap_or_default())
        })
    }

    fn update(&self, path: &str, settings: Settings) -> Result<()> {
        self.block(async {
            let c = self.proxy(path, CONNECTION_IFACE).await?;
            c.call_with_flags::<_, _, ()>("Update", MethodFlags::AllowInteractiveAuth.into(), &(settings,)).await.map(|_| ())
        })
    }

    fn delete(&self, path: &str) -> Result<()> {
        self.block(async {
            let c = self.proxy(path, CONNECTION_IFACE).await?;
            c.call_with_flags::<_, _, ()>("Delete", MethodFlags::AllowInteractiveAuth.into(), &()).await.map(|_| ())
        })
    }

    fn activate(&self, path: &str, device: Option<&str>) -> Result<String> {
        self.block(async {
            let nm = self.proxy(NM_PATH, NM).await?;
            let conn = OwnedObjectPath::try_from(path)?;
            let dev = OwnedObjectPath::try_from(device.unwrap_or("/"))?;
            let root = OwnedObjectPath::try_from("/")?;
            let p: Option<OwnedObjectPath> = nm.call_with_flags("ActivateConnection", MethodFlags::AllowInteractiveAuth.into(), &(conn, dev, root)).await?;
            Ok(p.map(|p| p.to_string()).unwrap_or_default())
        })
    }

    fn add_and_activate(&self, settings: Settings, device: &str) -> Result<String> {
        self.block(async {
            let nm = self.proxy(NM_PATH, NM).await?;
            let dev = OwnedObjectPath::try_from(device)?;
            let root = OwnedObjectPath::try_from("/")?;
            let r: Option<(OwnedObjectPath, OwnedObjectPath)> = nm.call_with_flags("AddAndActivateConnection", MethodFlags::AllowInteractiveAuth.into(), &(settings, dev, root)).await?;
            Ok(r.map(|(_, a)| a.to_string()).unwrap_or_default())
        })
    }

    fn deactivate(&self, active: &str) -> Result<()> {
        self.block(async {
            let nm = self.proxy(NM_PATH, NM).await?;
            let a = OwnedObjectPath::try_from(active)?;
            nm.call_with_flags::<_, _, ()>("DeactivateConnection", MethodFlags::AllowInteractiveAuth.into(), &(a,)).await.map(|_| ())
        })
    }

    fn wifi_device(&self) -> Result<Option<String>> {
        self.block(async {
            let nm = self.proxy(NM_PATH, NM).await?;
            let paths: Vec<OwnedObjectPath> = nm.call("GetDevices", &()).await?;
            for p in paths {
                let d = self.proxy(p.as_str(), DEVICE_IFACE).await?;
                if d.get_property::<u32>("DeviceType").await.unwrap_or(0) == DEVICE_TYPE_WIFI {
                    return Ok(Some(p.to_string()));
                }
            }
            Ok(None)
        })
    }

    fn devices(&self) -> Result<Vec<DeviceDetails>> {
        self.block(async {
            let nm = self.proxy(NM_PATH, NM).await?;
            let paths: Vec<OwnedObjectPath> = nm.call("GetDevices", &()).await?;
            let mut out = Vec::new();
            for p in paths {
                let d = self.proxy(p.as_str(), DEVICE_IFACE).await?;
                let kind = match d.get_property::<u32>("DeviceType").await.unwrap_or(0) {
                    DEVICE_TYPE_ETHERNET => "ethernet",
                    DEVICE_TYPE_WIFI => "wifi",
                    _ => continue,
                };
                let mut det = DeviceDetails { interface: d.get_property("Interface").await.unwrap_or_default(), kind: kind.into(), mac: d.get_property("HwAddress").await.unwrap_or_default(), ..Default::default() };
                let active: OwnedObjectPath = d.get_property("ActiveConnection").await.unwrap_or_else(|_| OwnedObjectPath::default());
                if active.as_str() != "/" {
                    let a = self.proxy(active.as_str(), ACTIVE_IFACE).await?;
                    det.connection = a.get_property::<String>("Id").await.ok();
                }
                det.speed_mbps = if kind == "ethernet" {
                    self.proxy(p.as_str(), "org.freedesktop.NetworkManager.Device.Wired").await?.get_property::<u32>("Speed").await.ok().filter(|s| *s > 0)
                } else {
                    // Bitrate is in kb/s.
                    self.proxy(p.as_str(), "org.freedesktop.NetworkManager.Device.Wireless").await?.get_property::<u32>("Bitrate").await.ok().map(|k| k / 1000).filter(|s| *s > 0)
                };
                let ip4: OwnedObjectPath = d.get_property("Ip4Config").await.unwrap_or_else(|_| OwnedObjectPath::default());
                if ip4.as_str() != "/" {
                    let c = self.proxy(ip4.as_str(), IP4_IFACE).await?;
                    let addrs: Vec<HashMap<String, OwnedValue>> = c.get_property("AddressData").await.unwrap_or_default();
                    det.addresses = addrs
                        .iter()
                        .filter_map(|m| Some(format!("{}/{}", String::try_from(m.get("address")?.try_clone().ok()?).ok()?, u32::try_from(m.get("prefix")?).ok()?)))
                        .collect();
                    det.gateway = c.get_property::<String>("Gateway").await.ok().filter(|g| !g.is_empty());
                    let ns: Vec<HashMap<String, OwnedValue>> = c.get_property("NameserverData").await.unwrap_or_default();
                    det.dns = ns.iter().filter_map(|m| String::try_from(m.get("address")?.try_clone().ok()?).ok()).collect();
                }
                out.push(det);
            }
            Ok(out)
        })
    }
}

/// An in-memory NetworkManager for tests (and `MANDARA_NM_MOCK=1` demos).
pub mod mock {
    use super::*;
    use parking_lot::Mutex;

    #[derive(Default)]
    pub struct MockNm {
        pub saved: Mutex<Vec<(String, Settings, Settings)>>,
        pub active: Mutex<Vec<Active>>,
        /// Every mutating call, in order (`add <id>`, `activate <path>`…).
        pub log: Mutex<Vec<String>>,
        pub wifi: Option<String>,
    }

    impl MockNm {
        pub fn with_wifi() -> Self {
            Self { wifi: Some("/dev/wifi0".into()), ..Default::default() }
        }

        /// Adds a profile with `secrets` kept separately (like NM does).
        pub fn insert(&self, settings: Settings, secrets: Settings) -> String {
            let mut saved = self.saved.lock();
            let path = format!("/settings/{}", saved.len() + 1);
            let mut settings = settings;
            if get_str(&settings, "connection", "uuid").is_none() {
                set(&mut settings, "connection", "uuid", ov(format!("uuid-{}", saved.len() + 1)));
            }
            saved.push((path.clone(), settings, secrets));
            path
        }

        pub fn settings_of(&self, id: &str) -> Option<Settings> {
            self.saved.lock().iter().find(|(_, s, _)| get_str(s, "connection", "id").as_deref() == Some(id)).map(|(_, s, _)| clone_settings(s))
        }

        fn uuid_of(&self, path: &str) -> String {
            self.saved.lock().iter().find(|(p, ..)| p == path).and_then(|(_, s, _)| get_str(s, "connection", "uuid")).unwrap_or_default()
        }
    }

    /// Splits secret keys (`psk`, `password`, `private-key`) out of settings.
    fn split_secrets(mut s: Settings) -> (Settings, Settings) {
        let mut secrets = Settings::new();
        for (name, props) in s.iter_mut() {
            for key in ["psk", "private-key", "secrets", "password"] {
                if let Some(v) = props.remove(key) {
                    secrets.entry(name.clone()).or_default().insert(key.to_string(), v);
                }
            }
        }
        (s, secrets)
    }

    impl NmBackend for MockNm {
        fn saved(&self) -> Result<Vec<Saved>> {
            Ok(self.saved.lock().iter().map(|(p, s, _)| Saved { path: p.clone(), settings: clone_settings(s) }).collect())
        }
        fn active(&self) -> Result<Vec<Active>> {
            Ok(self.active.lock().clone())
        }
        fn secrets(&self, path: &str, setting: &str) -> Settings {
            self.saved.lock().iter().find(|(p, ..)| p == path).map(|(_, _, sec)| sec.iter().filter(|(k, _)| k.as_str() == setting).map(|(k, m)| (k.clone(), m.iter().filter_map(|(k, v)| Some((k.clone(), v.try_clone().ok()?))).collect())).collect()).unwrap_or_default()
        }
        fn add(&self, settings: Settings) -> Result<String> {
            self.log.lock().push(format!("add {}", get_str(&settings, "connection", "id").unwrap_or_default()));
            let (s, sec) = split_secrets(settings);
            Ok(self.insert(s, sec))
        }
        fn update(&self, path: &str, settings: Settings) -> Result<()> {
            self.log.lock().push(format!("update {path}"));
            let (s, sec) = split_secrets(settings);
            let mut saved = self.saved.lock();
            let entry = saved.iter_mut().find(|(p, ..)| p == path).ok_or_else(|| anyhow!("no {path}"))?;
            // Like NetworkManager: an update without a secret drops it.
            entry.1 = s;
            entry.2 = sec;
            Ok(())
        }
        fn delete(&self, path: &str) -> Result<()> {
            self.log.lock().push(format!("delete {path}"));
            self.saved.lock().retain(|(p, ..)| p != path);
            Ok(())
        }
        fn activate(&self, path: &str, _device: Option<&str>) -> Result<String> {
            self.log.lock().push(format!("activate {path}"));
            let uuid = self.uuid_of(path);
            let a = format!("/active/{uuid}");
            self.active.lock().push(Active { path: a.clone(), uuid, state: ACTIVE_ACTIVATED });
            Ok(a)
        }
        fn add_and_activate(&self, settings: Settings, device: &str) -> Result<String> {
            let path = self.add(settings)?;
            self.activate(&path, Some(device))
        }
        fn deactivate(&self, active: &str) -> Result<()> {
            self.log.lock().push(format!("deactivate {active}"));
            self.active.lock().retain(|a| a.path != active);
            Ok(())
        }
        fn wifi_device(&self) -> Result<Option<String>> {
            Ok(self.wifi.clone())
        }
        fn devices(&self) -> Result<Vec<DeviceDetails>> {
            Ok(vec![DeviceDetails { interface: "wlan0".into(), kind: "wifi".into(), mac: "00:11:22:33:44:55".into(), addresses: vec!["192.168.1.20/24".into()], ..Default::default() }])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::mock::MockNm;
    use super::*;

    const WG: &str = "[Interface]\nPrivateKey = cHJpdmF0ZQ==\nAddress = 10.8.0.2/24, fd00::2/64\nDNS = 1.1.1.1, 2606:4700:4700::1111\nListenPort = 51820\n\n[Peer]\nPublicKey = cHVibGlj\nPresharedKey = cHNr\nAllowedIPs = 0.0.0.0/0, ::/0\nEndpoint = vpn.example.com:51820\nPersistentKeepalive = 25 # NAT\n";

    fn wifi(id: &str, ssid: &str, psk: &str) -> (Settings, Settings) {
        let mut s = base_connection(id, "802-11-wireless");
        set(&mut s, "connection", "autoconnect", ov(true));
        set(&mut s, "802-11-wireless", "ssid", ov(ssid.as_bytes().to_vec()));
        set(&mut s, "802-11-wireless-security", "key-mgmt", ov("wpa-psk".to_string()));
        let mut sec = Settings::new();
        set(&mut sec, "802-11-wireless-security", "psk", ov(psk.to_string()));
        (s, sec)
    }

    #[test]
    fn parses_wireguard_profiles() {
        let wg = parse_wireguard(WG).unwrap();
        assert_eq!(wg.addresses.len(), 2);
        assert_eq!(wg.dns.len(), 2);
        assert_eq!(wg.peers[0].allowed_ips, vec!["0.0.0.0/0", "::/0"]);
        assert_eq!(wg.peers[0].keepalive, Some(25));
        assert!(parse_wireguard("[Interface]\nAddress = 10.0.0.2/32\n").is_err());
        let s = wireguard_settings("Office VPN", &wg);
        assert_eq!(get_str(&s, "connection", "type").as_deref(), Some("wireguard"));
        assert_eq!(get_str(&s, "connection", "interface-name").as_deref(), Some("wg-office-vpn"));
        assert_eq!(get_str(&s, "ipv4", "method").as_deref(), Some("manual"));
        let dns: Vec<u32> = Vec::<u32>::try_from(get(&s, "ipv4", "dns").unwrap().try_clone().unwrap()).unwrap();
        assert_eq!(dns, vec![u32::from_ne_bytes([1, 1, 1, 1])]);
        assert!(get(&s, "wireguard", "peers").is_some());
    }

    #[test]
    fn interface_names_fit_the_kernel_limit() {
        assert_eq!(interface_name("Home"), "wg-home");
        assert_eq!(interface_name("wg0"), "wg0");
        assert!(interface_name("A very long VPN profile name").len() <= IFNAME_MAX);
        assert_eq!(interface_name("ünï cödé"), "wg-n-cd");
    }

    #[test]
    fn parses_openvpn_with_inline_certs() {
        let ovpn = "client\ndev tun\nproto tcp\nremote vpn.example.com 1194\nremote backup.example.com 443 tcp\nauth-user-pass\ncipher AES-256-GCM\nremote-cert-tls server\n<ca>\n-----BEGIN CERTIFICATE-----\nAAA\n-----END CERTIFICATE-----\n</ca>\nkey-direction 1\n<tls-auth>\nKEY\n</tls-auth>\n";
        let p = parse_openvpn(ovpn).unwrap();
        let data: HashMap<_, _> = p.data.iter().cloned().collect();
        assert_eq!(data["remote"], "vpn.example.com:1194, backup.example.com:443:tcp");
        assert_eq!(data["proto-tcp"], "yes");
        assert_eq!(data["connection-type"], "password");
        assert_eq!(data["ta-dir"], "1");
        assert_eq!(p.inline.len(), 2);
        assert!(p.inline[0].1.contains("BEGIN CERTIFICATE"));
        let s = openvpn_settings("Work", &p, Path::new("/tmp/certs"), Some(&VpnCredentials { username: "me".into(), password: "pw".into() }));
        let vpn_data = HashMap::<String, String>::try_from(get(&s, "vpn", "data").unwrap().try_clone().unwrap()).unwrap();
        assert_eq!(vpn_data["ca"], "/tmp/certs/ca.pem");
        assert_eq!(vpn_data["ta"], "/tmp/certs/tls-auth.pem");
        assert_eq!(vpn_data["username"], "me");
        assert!(get(&s, "vpn", "secrets").is_some());
        assert!(parse_openvpn("client\n").is_err());
    }

    #[test]
    fn vpn_list_up_down_import_remove() {
        let nm = MockNm::default();
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("home.conf");
        std::fs::write(&file, WG).unwrap();
        assert_eq!(import_vpn(&nm, &file, None, None).unwrap(), "home");
        assert!(import_vpn(&nm, &file, None, None).is_err(), "duplicate names are refused");
        let (w, sec) = wifi("Cafe", "Cafe", "secret123");
        nm.insert(w, sec);
        let list = vpns(&nm).unwrap();
        assert_eq!(list.len(), 1, "Wi-Fi isn't a VPN");
        assert_eq!((list[0].kind.as_str(), list[0].active), ("wireguard", false));
        vpn_set(&nm, "HOME", true).unwrap();
        assert!(vpns(&nm).unwrap()[0].active);
        vpn_set(&nm, "home", true).unwrap();
        assert_eq!(nm.log.lock().iter().filter(|l| l.starts_with("activate")).count(), 1, "already up: no second activation");
        vpn_set(&nm, "home", false).unwrap();
        assert!(!vpns(&nm).unwrap()[0].active);
        assert!(vpn_set(&nm, "Cafe", true).is_err(), "a Wi-Fi profile is not a VPN");
        vpn_remove(&nm, "home").unwrap();
        assert!(vpns(&nm).unwrap().is_empty());
    }

    #[test]
    fn saved_network_edits_keep_secrets() {
        let nm = MockNm::with_wifi();
        let (w, sec) = wifi("Home", "Home-5G", "hunter22");
        let path = nm.insert(w, sec);
        let list = saved_wifi(&nm).unwrap();
        assert_eq!((list[0].ssid.as_str(), list[0].autoconnect, list[0].secured), ("Home-5G", true, true));
        set_autoconnect(&nm, "Home", false).unwrap();
        set_priority(&nm, "Home", 5000).unwrap();
        let s = nm.settings_of("Home").unwrap();
        assert_eq!(get_bool(&s, "connection", "autoconnect"), Some(false));
        assert_eq!(get_i32(&s, "connection", "autoconnect-priority"), Some(PRIORITY_RANGE.1));
        // The password survived both updates.
        assert_eq!(get_str(&nm.secrets(&path, "802-11-wireless-security"), "802-11-wireless-security", "psk").as_deref(), Some("hunter22"));
        forget(&nm, "Home").unwrap();
        assert!(saved_wifi(&nm).unwrap().is_empty());
        connect_hidden(&nm, "Secret Lab", Some("p4ssword!")).unwrap();
        let hidden = saved_wifi(&nm).unwrap();
        assert!(hidden[0].hidden && hidden[0].active);
    }

    #[test]
    fn hotspot_reuses_one_profile_and_its_password() {
        let nm = MockNm::with_wifi();
        assert!(hotspot_status(&nm, false).unwrap().is_none());
        assert!(hotspot_start(&nm, "Lab", Some("short"), None).is_err());
        let first = hotspot_start(&nm, "Lab", None, Some("bg")).unwrap();
        let pw = first.password.clone().unwrap();
        assert_eq!(pw.len(), HOTSPOT_PASSWORD_LEN);
        assert!(hotspot_status(&nm, false).unwrap().unwrap().active);
        assert!(hotspot_stop(&nm).unwrap());
        assert!(!hotspot_stop(&nm).unwrap());
        // Restarting keeps the stored password and the single profile.
        let again = hotspot_start(&nm, "Lab 2", None, None).unwrap();
        assert_eq!(again.password.as_deref(), Some(pw.as_str()));
        assert_eq!(nm.saved.lock().len(), 1);
        let st = hotspot_status(&nm, true).unwrap().unwrap();
        assert_eq!((st.ssid.as_str(), st.password.as_deref()), ("Lab 2", Some(pw.as_str())));
        let s = nm.settings_of(HOTSPOT_ID).unwrap();
        assert_eq!(get_str(&s, "802-11-wireless", "mode").as_deref(), Some("ap"));
        assert_eq!(get_str(&s, "ipv4", "method").as_deref(), Some("shared"));
        assert!(MockNm::default().wifi_device().unwrap().is_none());
        assert!(hotspot_start(&MockNm::default(), "x", None, None).is_err(), "no Wi-Fi device");
    }

    /// Studio → Network → Hotspot: the name and band settings reach the
    /// profile NetworkManager runs, and "Automatic" drops the band again.
    #[test]
    fn hotspot_name_and_band_follow_the_settings() {
        use mandara_config::connectivity::{HotspotBand, NetworkConfig};
        let nm = MockNm::with_wifi();
        let mut cfg = NetworkConfig { hotspot_ssid: "  Mandara Lab ".into(), ..Default::default() };
        for (band, want) in [(HotspotBand::Ghz5, Some("a")), (HotspotBand::Ghz2, Some("bg")), (HotspotBand::Auto, None)] {
            cfg.hotspot_band = band;
            hotspot_start(&nm, &cfg.hotspot_name(), None, cfg.hotspot_band.nm()).unwrap();
            let s = nm.settings_of(HOTSPOT_ID).unwrap();
            assert_eq!(get_str(&s, "802-11-wireless", "band").as_deref(), want, "{band:?}");
            assert_eq!(get_bytes(&s, "802-11-wireless", "ssid").map(|b| String::from_utf8_lossy(&b).to_string()).as_deref(), Some("Mandara Lab"));
        }
        assert_eq!(nm.saved.lock().len(), 1, "one profile, updated in place");
    }

    /// The static IP editor replaces only its own keys.
    #[test]
    fn ipv4_editor_keeps_routes_and_other_keys() {
        let nm = MockNm::with_wifi();
        let (mut w, sec) = wifi("Lab", "Lab", "password1");
        set(&mut w, "ipv4", "route-metric", ov(50i64));
        set(&mut w, "ipv4", "dns-search", ov(vec!["corp.example".to_string()]));
        set(&mut w, "ipv4", "never-default", ov(true));
        set(&mut w, "ipv4", "dns", ov(vec![0x0101_0101u32]));
        nm.insert(w, sec);
        let manual = Ipv4Settings { method: "manual".into(), addresses: vec!["10.0.0.5/16".into()], gateway: Some("10.0.0.1".into()), dns: vec![], ignore_auto_dns: false };
        set_ipv4(&nm, "Lab", &manual).unwrap();
        let s = nm.settings_of("Lab").unwrap();
        assert_eq!(get(&s, "ipv4", "route-metric").and_then(|v| i64::try_from(v).ok()), Some(50));
        assert_eq!(get_bool(&s, "ipv4", "never-default"), Some(true));
        assert!(get(&s, "ipv4", "dns-search").is_some(), "search domains survive");
        assert!(get(&s, "ipv4", "dns").is_none(), "the editor's own DNS key is replaced (cleared here)");
        assert_eq!(ipv4(&nm, "Lab").unwrap(), manual);
    }

    #[test]
    fn ipv4_editor_round_trip_and_validation() {
        let nm = MockNm::with_wifi();
        let (w, sec) = wifi("Office", "Office", "password1");
        nm.insert(w, sec);
        assert_eq!(ipv4(&nm, "Office").unwrap().method, "auto");
        let manual = Ipv4Settings { method: "manual".into(), addresses: vec!["192.168.10.5/24".into()], gateway: Some("192.168.10.1".into()), dns: vec!["9.9.9.9".into()], ignore_auto_dns: true };
        set_ipv4(&nm, "Office", &manual).unwrap();
        assert_eq!(ipv4(&nm, "Office").unwrap(), manual);
        let dhcp_dns = Ipv4Settings { method: "auto".into(), dns: vec!["1.1.1.1".into()], ignore_auto_dns: true, ..Default::default() };
        set_ipv4(&nm, "Office", &dhcp_dns).unwrap();
        assert_eq!(ipv4(&nm, "Office").unwrap(), dhcp_dns);
        for bad in [
            Ipv4Settings { method: "manual".into(), ..Default::default() },
            Ipv4Settings { method: "manual".into(), addresses: vec!["300.1.1.1/24".into()], ..Default::default() },
            Ipv4Settings { method: "auto".into(), dns: vec!["not-an-ip".into()], ..Default::default() },
            Ipv4Settings { method: "link-local".into(), ..Default::default() },
        ] {
            assert!(set_ipv4(&nm, "Office", &bad).is_err(), "{bad:?}");
        }
        let path = nm.saved.lock()[0].0.clone();
        assert_eq!(get_str(&nm.secrets(&path, "802-11-wireless-security"), "802-11-wireless-security", "psk").as_deref(), Some("password1"));
    }

    /// Read-only against the real NetworkManager (`--ignored`): lists, never
    /// changes anything.
    #[test]
    #[ignore]
    fn live_read_only() {
        let nm = DbusNm::connect().unwrap();
        let v = vpns(&nm).unwrap();
        let w = saved_wifi(&nm).unwrap();
        let d = nm.devices().unwrap();
        let h = hotspot_status(&nm, false).unwrap();
        println!("vpns={} saved_wifi={} active_wifi={} devices={:?} hotspot={}", v.len(), w.len(), w.iter().filter(|x| x.active).count(), d.iter().map(|d| (&d.kind, d.addresses.len(), d.dns.len(), d.speed_mbps)).collect::<Vec<_>>(), h.is_some());
        if let Some(active) = w.iter().find(|x| x.active) {
            let v4 = ipv4(&nm, &active.uuid).unwrap();
            println!("active ipv4 method={} addresses={}", v4.method, v4.addresses.len());
        }
    }

    #[test]
    fn generated_passwords_are_valid() {
        for _ in 0..20 {
            assert!(mandara_config::connectivity::valid_hotspot_password(&generate_password()));
        }
    }
}
