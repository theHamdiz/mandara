//! External-monitor brightness over DDC/CI, in pure Rust: the VESA
//! MCCS "get / set VCP feature" messages over Linux `i2c-dev`
//! (`/dev/i2c-N`, `I2C_SLAVE` ioctl through rustix).
//!
//! Access needs the `i2c-dev` kernel module and read/write permission on
//! the bus nodes — on most distributions membership of the `i2c` group
//! (`sudo usermod -aG i2c $USER`, then log in again). Every call blocks for
//! tens of milliseconds (the monitor's mandated reply delays): run them off
//! the UI thread.

use std::collections::HashMap;
use std::io::{Read as _, Write as _};
use std::os::fd::AsFd as _;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use parking_lot::Mutex;
use serde::Serialize;

/// `I2C_SLAVE` ioctl request (linux/i2c-dev.h).
const I2C_SLAVE: rustix::ioctl::Opcode = 0x0703;
/// DDC/CI device address on the bus (7-bit).
const DDC_ADDRESS: usize = 0x37;
/// Destination address byte (0x37 << 1) mixed into request checksums.
const DEST_WRITE: u8 = 0x6E;
/// Source address of host messages.
const HOST_SOURCE: u8 = 0x51;
/// Virtual host address mixed into reply checksums.
const REPLY_CHECKSUM_SEED: u8 = 0x50;
/// Length byte flag.
const LENGTH_FLAG: u8 = 0x80;
/// Opcodes.
const GET_VCP: u8 = 0x01;
const GET_VCP_REPLY: u8 = 0x02;
const SET_VCP: u8 = 0x03;
/// VCP feature: luminance (brightness).
pub const VCP_BRIGHTNESS: u8 = 0x10;
/// A get-VCP reply is 11 bytes (address, length, 8 payload, checksum).
const GET_REPLY_LEN: usize = 11;
/// Payload length of a get-VCP reply.
const GET_REPLY_PAYLOAD: u8 = 8;
/// MCCS timing: wait after a get request before reading, and after a set.
const REPLY_DELAY: Duration = Duration::from_millis(45);
const SET_DELAY: Duration = Duration::from_millis(55);
/// Attempts per request (monitors drop messages under load).
const RETRIES: usize = 3;
/// Pause between retries.
const RETRY_DELAY: Duration = Duration::from_millis(30);
/// Percent per whole.
const PERCENT: f64 = 100.0;

const DRM_ROOT: &str = "/sys/class/drm";

/// One external monitor answering DDC/CI.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DdcMonitor {
    /// Wayland / DRM connector name, e.g. `HDMI-A-2`.
    pub connector: String,
    /// `/dev/i2c-N`.
    pub bus: u32,
    pub brightness: u16,
    pub max: u16,
}

impl DdcMonitor {
    pub fn percent(&self) -> u32 {
        if self.max == 0 { 0 } else { ((self.brightness as f64 / self.max as f64) * PERCENT).round() as u32 }
    }
}

/// Why DDC/CI isn't usable.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case")]
pub enum Access {
    Ready,
    /// No `/dev/i2c-*` nodes: the `i2c-dev` module isn't loaded.
    NoModule,
    /// Nodes exist but aren't readable and writable by this user.
    NoPermission { group: Option<String> },
}

/// Whether this user can talk to the i2c buses.
pub fn access() -> Access {
    let nodes: Vec<PathBuf> = std::fs::read_dir("/dev").map(|d| d.flatten().map(|e| e.path()).filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("i2c-"))).collect()).unwrap_or_default();
    let Some(first) = nodes.first() else { return Access::NoModule };
    let ok = nodes.iter().any(|p| rustix::fs::access(p, rustix::fs::Access::READ_OK | rustix::fs::Access::WRITE_OK).is_ok());
    if ok { Access::Ready } else { Access::NoPermission { group: group_of(first) } }
}

/// The group owning a device node (from `/etc/group`).
fn group_of(path: &Path) -> Option<String> {
    use std::os::unix::fs::MetadataExt as _;
    let gid = std::fs::metadata(path).ok()?.gid();
    let groups = std::fs::read_to_string("/etc/group").ok()?;
    groups.lines().find_map(|l| {
        let mut f = l.split(':');
        let name = f.next()?;
        let _ = f.next();
        (f.next()?.parse::<u32>().ok()? == gid).then(|| name.to_string())
    })
}

fn checksum(seed: u8, bytes: &[u8]) -> u8 {
    bytes.iter().fold(seed, |acc, b| acc ^ b)
}

/// Builds a host → monitor message.
fn message(payload: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(payload.len() + 3);
    m.push(HOST_SOURCE);
    m.push(LENGTH_FLAG | payload.len() as u8);
    m.extend_from_slice(payload);
    m.push(checksum(DEST_WRITE, &m));
    m
}

/// Parses a get-VCP reply: `(current, max)`.
fn parse_get_reply(r: &[u8], vcp: u8) -> Result<(u16, u16), String> {
    if r.len() < GET_REPLY_LEN || r[1] & !LENGTH_FLAG != GET_REPLY_PAYLOAD || r[2] != GET_VCP_REPLY || r[4] != vcp {
        return Err(format!("unexpected DDC/CI reply {r:02x?}"));
    }
    if checksum(REPLY_CHECKSUM_SEED, &r[..GET_REPLY_LEN - 1]) != r[GET_REPLY_LEN - 1] {
        return Err("DDC/CI reply checksum mismatch".into());
    }
    if r[3] != 0 {
        return Err(format!("monitor doesn't support VCP {vcp:#04x}"));
    }
    Ok((u16::from_be_bytes([r[8], r[9]]), u16::from_be_bytes([r[6], r[7]])))
}

fn open(bus: u32) -> std::io::Result<std::fs::File> {
    let file = std::fs::OpenOptions::new().read(true).write(true).open(format!("/dev/i2c-{bus}"))?;
    // SAFETY: I2C_SLAVE takes the 7-bit address by value.
    unsafe { rustix::ioctl::ioctl(file.as_fd(), rustix::ioctl::IntegerSetter::<I2C_SLAVE>::new_usize(DDC_ADDRESS))? };
    Ok(file)
}

fn retry<T>(mut f: impl FnMut() -> Result<T, String>) -> Result<T, String> {
    let mut last = String::new();
    for attempt in 0..RETRIES {
        match f() {
            Ok(v) => return Ok(v),
            Err(e) => last = e,
        }
        if attempt + 1 < RETRIES {
            std::thread::sleep(RETRY_DELAY);
        }
    }
    Err(last)
}

/// Reads a VCP feature: `(current, max)`.
pub fn get_vcp(bus: u32, vcp: u8) -> Result<(u16, u16), String> {
    retry(|| {
        let mut f = open(bus).map_err(|e| e.to_string())?;
        f.write_all(&message(&[GET_VCP, vcp])).map_err(|e| e.to_string())?;
        std::thread::sleep(REPLY_DELAY);
        let mut r = [0u8; GET_REPLY_LEN];
        f.read_exact(&mut r).map_err(|e| e.to_string())?;
        parse_get_reply(&r, vcp)
    })
}

/// Writes a VCP feature.
pub fn set_vcp(bus: u32, vcp: u8, value: u16) -> Result<(), String> {
    let [hi, lo] = value.to_be_bytes();
    retry(|| {
        let mut f = open(bus).map_err(|e| e.to_string())?;
        f.write_all(&message(&[SET_VCP, vcp, hi, lo])).map_err(|e| e.to_string())?;
        std::thread::sleep(SET_DELAY);
        Ok(())
    })
}

/// i2c buses a connector's DDC may live on: its `ddc` link first, then the
/// DisplayPort AUX buses listed under it.
fn candidate_buses(connector_dir: &Path) -> Vec<u32> {
    let bus_of = |p: &Path| p.file_name()?.to_str()?.strip_prefix("i2c-")?.parse::<u32>().ok();
    let mut out = Vec::new();
    if let Ok(target) = std::fs::read_link(connector_dir.join("ddc"))
        && let Some(b) = bus_of(&target)
    {
        out.push(b);
    }
    for e in std::fs::read_dir(connector_dir).into_iter().flatten().flatten() {
        if let Some(b) = bus_of(&e.path())
            && !out.contains(&b)
        {
            out.push(b);
        }
    }
    out
}

/// Connected external connectors (`card1-HDMI-A-2` → `HDMI-A-2`); laptop
/// panels (eDP / LVDS / DSI) use the backlight instead.
fn external_connectors() -> Vec<(String, PathBuf)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(DRM_ROOT).into_iter().flatten().flatten() {
        let file = e.file_name().to_string_lossy().to_string();
        let Some((card, connector)) = file.split_once('-') else { continue };
        if !card.starts_with("card") || ["eDP", "LVDS", "DSI", "Writeback"].iter().any(|p| connector.starts_with(p)) {
            continue;
        }
        if std::fs::read_to_string(e.path().join("status")).is_ok_and(|s| s.trim() == "connected") {
            out.push((connector.to_string(), e.path()));
        }
    }
    out.sort();
    out
}

/// Connector → bus that answered, so later calls skip probing.
fn bus_cache() -> &'static Mutex<HashMap<String, u32>> {
    static CACHE: OnceLock<Mutex<HashMap<String, u32>>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}

/// The DDC/CI bus of a connector (probing its candidates once).
fn bus_for(connector: &str, dir: &Path) -> Option<(u32, u16, u16)> {
    if let Some(bus) = bus_cache().lock().get(connector).copied()
        && let Ok((cur, max)) = get_vcp(bus, VCP_BRIGHTNESS)
    {
        return Some((bus, cur, max));
    }
    for bus in candidate_buses(dir) {
        if let Ok((cur, max)) = get_vcp(bus, VCP_BRIGHTNESS) {
            bus_cache().lock().insert(connector.to_string(), bus);
            return Some((bus, cur, max));
        }
    }
    None
}

/// Every connected external monitor that answers DDC/CI, with its
/// brightness. Blocks (≈ 50–150 ms per monitor).
pub fn monitors() -> Vec<DdcMonitor> {
    external_connectors().into_iter().filter_map(|(connector, dir)| bus_for(&connector, &dir).map(|(bus, brightness, max)| DdcMonitor { connector, bus, brightness, max })).collect()
}

/// `percent` within `min_percent..=100`. Never panics: a floor above 100
/// (a hand-edited config) is capped, and a non-finite request sits on the
/// floor instead of becoming 0 % (a black screen).
pub fn floored_percent(percent: f64, min_percent: u32) -> f64 {
    let floor = (min_percent as f64).min(PERCENT);
    if percent.is_finite() { percent.clamp(floor, PERCENT) } else { floor }
}

/// Sets a monitor's brightness in percent (floored at `min_percent`).
/// Returns the percentage set.
pub fn set_percent(connector: &str, percent: f64, min_percent: u32) -> Result<u32, String> {
    let dir = external_connectors().into_iter().find(|(c, _)| c == connector).map(|(_, d)| d).ok_or_else(|| mandara_i18n::tf("No connected external monitor named {name}", &[("name", &connector)]))?;
    let (bus, _, max) = bus_for(connector, &dir).ok_or_else(|| mandara_i18n::tf("{name} doesn't answer DDC/CI (enable it in the monitor's menu)", &[("name", &connector)]))?;
    let p = floored_percent(percent, min_percent);
    let raw = ((p / PERCENT) * max as f64).round() as u16;
    set_vcp(bus, VCP_BRIGHTNESS, raw)?;
    Ok(p.round() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightness_floor_never_panics() {
        assert_eq!(floored_percent(40.0, 10), 40.0);
        assert_eq!(floored_percent(2.0, 10), 10.0);
        assert_eq!(floored_percent(120.0, 10), PERCENT);
        assert_eq!(floored_percent(50.0, 150), PERCENT, "a floor above 100 % is capped, not a panic");
        assert_eq!(floored_percent(f64::NAN, 5), 5.0);
    }

    #[test]
    fn request_checksums_match_the_spec() {
        // The classic "get brightness" request: 51 82 01 10 ac.
        assert_eq!(message(&[GET_VCP, VCP_BRIGHTNESS]), vec![0x51, 0x82, 0x01, 0x10, 0xac]);
        let set = message(&[SET_VCP, VCP_BRIGHTNESS, 0x00, 0x32]);
        assert_eq!(&set[..6], &[0x51, 0x84, 0x03, 0x10, 0x00, 0x32]);
        assert_eq!(set[6], checksum(DEST_WRITE, &set[..6]));
    }

    #[test]
    fn parses_replies() {
        // 6e 88 02 00 10 00 00 64 00 32 <chk>: brightness 50 of 100.
        let mut r = vec![0x6e, 0x88, 0x02, 0x00, 0x10, 0x00, 0x00, 0x64, 0x00, 0x32];
        r.push(checksum(REPLY_CHECKSUM_SEED, &r));
        assert_eq!(parse_get_reply(&r, VCP_BRIGHTNESS), Ok((50, 100)));
        let mut bad = r.clone();
        bad[10] ^= 1;
        assert!(parse_get_reply(&bad, VCP_BRIGHTNESS).is_err());
        let mut unsupported = r[..10].to_vec();
        unsupported[3] = 1;
        unsupported.push(checksum(REPLY_CHECKSUM_SEED, &unsupported));
        assert!(parse_get_reply(&unsupported, VCP_BRIGHTNESS).unwrap_err().contains("doesn't support"));
        assert_eq!(DdcMonitor { connector: "DP-1".into(), bus: 3, brightness: 50, max: 100 }.percent(), 50);
    }
}
