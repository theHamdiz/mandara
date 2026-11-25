//! `[network]` and `[bluetooth]`: preferences for the NetworkManager and
//! BlueZ integrations (the connections themselves live in NetworkManager
//! and BlueZ, never in Mandara's config).

use serde::{Deserialize, Serialize};

/// Hotspot band.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HotspotBand {
    /// Let NetworkManager pick.
    #[default]
    Auto,
    /// 2.4 GHz (`bg`): longest range, every device.
    Ghz2,
    /// 5 GHz (`a`): faster, shorter range.
    Ghz5,
}

impl HotspotBand {
    /// NetworkManager's `802-11-wireless.band` value.
    pub fn nm(self) -> Option<&'static str> {
        match self {
            HotspotBand::Auto => None,
            HotspotBand::Ghz2 => Some("bg"),
            HotspotBand::Ghz5 => Some("a"),
        }
    }
}

/// Shortest WPA2 passphrase.
pub const HOTSPOT_MIN_PASSWORD: usize = 8;
/// Longest WPA2 passphrase.
pub const HOTSPOT_MAX_PASSWORD: usize = 63;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NetworkConfig {
    /// Hotspot network name; empty = `<hostname> hotspot`.
    pub hotspot_ssid: String,
    pub hotspot_band: HotspotBand,
    /// The VPN connected last (reconnected by `network vpn up` without a
    /// name and offered first in the panel).
    pub last_vpn: Option<String>,
    /// Notify when a VPN connects or drops.
    pub vpn_notifications: bool,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self { hotspot_ssid: String::new(), hotspot_band: HotspotBand::Auto, last_vpn: None, vpn_notifications: true }
    }
}

impl NetworkConfig {
    /// The hotspot SSID to use.
    pub fn hotspot_name(&self) -> String {
        if !self.hotspot_ssid.trim().is_empty() {
            return self.hotspot_ssid.trim().to_string();
        }
        let host = std::fs::read_to_string("/etc/hostname").map(|h| h.trim().to_string()).unwrap_or_default();
        if host.is_empty() { "Mandara hotspot".into() } else { format!("{host} hotspot") }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct BluetoothConfig {
    /// Mandara answers pairing requests (PIN / passkey confirmation) with
    /// its own dialog (`org.bluez.Agent1`).
    pub pairing_agent: bool,
    /// Reconnect trusted devices when Bluetooth turns on and at startup.
    pub auto_connect: bool,
    /// Notify when a device connects or disconnects.
    pub connect_notifications: bool,
    /// Adapter to use (`hci1`); `None` = the first one.
    pub adapter: Option<String>,
}

impl Default for BluetoothConfig {
    fn default() -> Self {
        Self { pairing_agent: true, auto_connect: true, connect_notifications: true, adapter: None }
    }
}

/// `WIFI:` QR payload (the de-facto Wi-Fi sharing format phones scan):
/// `WIFI:T:WPA;S:<ssid>;P:<password>;;` with `\;,:"` escaped.
pub fn wifi_qr_payload(ssid: &str, password: Option<&str>, hidden: bool) -> String {
    fn esc(s: &str) -> String {
        let mut out = String::new();
        for c in s.chars() {
            if matches!(c, '\\' | ';' | ',' | ':' | '"') {
                out.push('\\');
            }
            out.push(c);
        }
        out
    }
    let mut s = String::from("WIFI:");
    match password.filter(|p| !p.is_empty()) {
        Some(p) => s.push_str(&format!("T:WPA;S:{};P:{};", esc(ssid), esc(p))),
        None => s.push_str(&format!("T:nopass;S:{};", esc(ssid))),
    }
    if hidden {
        s.push_str("H:true;");
    }
    s.push(';');
    s
}

/// Checks a hotspot password (WPA2-PSK: 8–63 printable ASCII).
pub fn valid_hotspot_password(p: &str) -> bool {
    (HOTSPOT_MIN_PASSWORD..=HOTSPOT_MAX_PASSWORD).contains(&p.len()) && p.chars().all(|c| c.is_ascii_graphic() || c == ' ')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_payload_escapes() {
        assert_eq!(wifi_qr_payload("Home", Some("p@ss;word"), false), "WIFI:T:WPA;S:Home;P:p@ss\\;word;;");
        assert_eq!(wifi_qr_payload("Cafe:1", None, true), "WIFI:T:nopass;S:Cafe\\:1;H:true;;");
    }

    #[test]
    fn hotspot_passwords() {
        assert!(valid_hotspot_password("12345678"));
        assert!(!valid_hotspot_password("short"));
        assert!(!valid_hotspot_password("ünicode-pass"));
        assert_eq!(HotspotBand::Ghz5.nm(), Some("a"));
    }

    #[test]
    fn defaults_load() {
        let n: NetworkConfig = toml::from_str("").unwrap();
        assert!(n.vpn_notifications);
        let b: BluetoothConfig = toml::from_str("auto_connect = false").unwrap();
        assert!(!b.auto_connect && b.pairing_agent);
    }
}
