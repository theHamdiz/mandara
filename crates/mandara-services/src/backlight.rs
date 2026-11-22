//! Hardware sources for on-screen displays: the display backlight
//! (`/sys/class/backlight`, or `/dev/backlight` on FreeBSD; changed through logind's
//! `Session.SetBrightness` so no root or udev rule is needed) and the
//! keyboard Caps Lock LED.
//!
//! The backlight watcher blocks in `poll(2)` on `actual_brightness`, which
//! the kernel `sysfs_notify`s on every change (hotkeys and sysfs writes), so
//! it costs nothing while the brightness is idle. Machines without a
//! backlight (desktops) simply get no watcher.

#[cfg(not(target_os = "freebsd"))]
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;

#[cfg(not(target_os = "freebsd"))]
const BACKLIGHT_ROOT: &str = "/sys/class/backlight";
/// Lowest brightness `raw_for` allows, as a fraction of the maximum.
const MIN_LEVEL: f32 = 0.01;
/// Re-read interval for backlight drivers that never notify.
const BACKLIGHT_RESYNC: Duration = Duration::from_secs(30);
/// Back-off after a failed `poll(2)`.
#[cfg(not(target_os = "freebsd"))]
const POLL_RETRY: Duration = Duration::from_secs(1);
/// Updates buffered for the UI.
const UPDATE_QUEUE: usize = 4;
/// Bytes read from a sysfs number attribute (`"4294967295\n"` fits).
#[cfg(not(target_os = "freebsd"))]
const SYSFS_NUMBER_BYTES: usize = 16;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Backlight {
    /// sysfs device name, e.g. `intel_backlight`.
    pub name: String,
    pub brightness: u32,
    pub max: u32,
}

impl Backlight {
    /// `0..=1`.
    pub fn level(&self) -> f32 {
        if self.max == 0 { 0.0 } else { (self.brightness as f32 / self.max as f32).clamp(0.0, 1.0) }
    }

    /// Raw value for a `0..=1` level. Never fully off (a black panel looks
    /// broken); the floor is 1 % or one step.
    pub fn raw_for(&self, level: f32) -> u32 {
        let floor = ((self.max as f32) * MIN_LEVEL).ceil().max(1.0) as u32;
        ((level.clamp(0.0, 1.0) * self.max as f32).round() as u32).clamp(floor.min(self.max), self.max)
    }

    #[cfg(not(target_os = "freebsd"))]
    fn read(dir: &Path) -> Option<Self> {
        let name = dir.file_name()?.to_string_lossy().to_string();
        let max = read_u32(&dir.join("max_brightness"))?;
        let brightness = read_u32(&dir.join("actual_brightness")).or_else(|| read_u32(&dir.join("brightness")))?;
        Some(Self { name, brightness: brightness.min(max), max })
    }
}

#[cfg(not(target_os = "freebsd"))]
fn read_u32(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Parses a sysfs number re-read from an open file (no allocation).
#[cfg(not(target_os = "freebsd"))]
fn pread_u32(file: &std::fs::File) -> Option<u32> {
    let mut buf = [0u8; SYSFS_NUMBER_BYTES];
    let n = rustix::io::pread(file, &mut buf, 0).ok()?;
    std::str::from_utf8(&buf[..n]).ok()?.trim().parse().ok()
}

/// Every backlight device.
#[cfg(not(target_os = "freebsd"))]
pub fn devices() -> Vec<Backlight> {
    let Ok(dir) = std::fs::read_dir(BACKLIGHT_ROOT) else { return Vec::new() };
    dir.flatten().filter_map(|e| Backlight::read(&e.path())).filter(|b| b.max > 0).collect()
}

/// The backlight to drive: firmware interfaces beat platform ones beat raw
/// registers (the kernel's own preference order), then the finest range.
#[cfg(not(target_os = "freebsd"))]
pub fn primary() -> Option<Backlight> {
    let rank = |b: &Backlight| match std::fs::read_to_string(Path::new(BACKLIGHT_ROOT).join(&b.name).join("type")).unwrap_or_default().trim() {
        "firmware" => 0,
        "platform" => 1,
        _ => 2,
    };
    devices().into_iter().min_by_key(|b| (rank(b), u32::MAX - b.max))
}

/// Re-reads one device.
#[cfg(not(target_os = "freebsd"))]
pub fn read(name: &str) -> Option<Backlight> {
    Backlight::read(&Path::new(BACKLIGHT_ROOT).join(name))
}

/// Sets the raw brightness through logind (`org.freedesktop.login1`
/// `Session.SetBrightness("backlight", name, value)` on the caller's
/// session) on the service runtime; the receiver gets the result.
#[cfg(not(target_os = "freebsd"))]
pub fn set_brightness(name: &str, value: u32) -> async_channel::Receiver<Result<String, String>> {
    let (tx, rx) = async_channel::bounded(1);
    let name = name.to_string();
    crate::runtime::spawn("set-brightness", async move {
        let result = async {
            let conn = crate::runtime::system().await.map_err(|e| e.to_string())?;
            let session = crate::actor::proxy(&conn, "org.freedesktop.login1", "/org/freedesktop/login1/session/auto", "org.freedesktop.login1.Session").await.map_err(|e| e.to_string())?;
            let r: zbus::Result<()> = session.call("SetBrightness", &("backlight", name.as_str(), value)).await;
            r.map_err(|e| e.to_string())?;
            Ok(format!("{name} = {value}"))
        }
        .await;
        let _ = tx.send(result).await;
    });
    rx
}

/// Follows the primary backlight. `None` when the machine has none.
#[cfg(not(target_os = "freebsd"))]
pub fn watch_backlight() -> Option<async_channel::Receiver<Backlight>> {
    let device = primary()?;
    let dir = PathBuf::from(BACKLIGHT_ROOT).join(&device.name);
    let (tx, rx) = async_channel::bounded(UPDATE_QUEUE);
    // A dedicated thread: sysfs change notification is POLLPRI, which the
    // async reactor doesn't wait for. Laptops only.
    let _ = std::thread::Builder::new().name("mandara-backlight".into()).spawn(move || {
        use rustix::event::{PollFd, PollFlags, Timespec, poll};
        let path = if dir.join("actual_brightness").exists() { dir.join("actual_brightness") } else { dir.join("brightness") };
        let Ok(file) = std::fs::File::open(&path) else { return };
        let mut last = device.brightness;
        loop {
            // sysfs attributes must be read before poll() reports changes.
            let Some(v) = pread_u32(&file) else { return };
            if v != last {
                last = v;
                if tx.send_blocking(Backlight { name: device.name.clone(), brightness: v.min(device.max), max: device.max }).is_err() {
                    return;
                }
            }
            if tx.is_closed() {
                return;
            }
            let mut fds = [PollFd::new(&file, PollFlags::PRI | PollFlags::ERR)];
            // A long timeout only resyncs drivers that never notify.
            let timeout = Timespec { tv_sec: BACKLIGHT_RESYNC.as_secs() as i64, tv_nsec: 0 };
            if poll(&mut fds, Some(&timeout)).is_err() {
                std::thread::sleep(POLL_RETRY);
            }
        }
    });
    Some(rx)
}

// ── FreeBSD: backlight(9) devices, percent levels ──────────────────────────

#[cfg(target_os = "freebsd")]
fn from_device(d: crate::freebsd::backlight::Device) -> Backlight {
    Backlight { name: d.name, brightness: d.percent, max: 100 }
}

/// Every panel backlight in `/dev/backlight` (keyboard ones excluded).
#[cfg(target_os = "freebsd")]
pub fn devices() -> Vec<Backlight> {
    use crate::freebsd::backlight as bl;
    bl::devices().into_iter().filter(|d| d.kind == bl::TYPE_PANEL).map(from_device).collect()
}

/// The first panel backlight.
#[cfg(target_os = "freebsd")]
pub fn primary() -> Option<Backlight> {
    devices().into_iter().next()
}

#[cfg(target_os = "freebsd")]
pub fn read(name: &str) -> Option<Backlight> {
    crate::freebsd::backlight::read(name).map(from_device)
}

/// Sets `name` to `value` percent with its ioctl, on the blocking pool.
#[cfg(target_os = "freebsd")]
pub fn set_brightness(name: &str, value: u32) -> async_channel::Receiver<Result<String, String>> {
    let (tx, rx) = async_channel::bounded(1);
    let name = name.to_string();
    crate::runtime::spawn("set-brightness", async move {
        let result = crate::runtime::unblock(move || {
            crate::freebsd::backlight::set(&name, value).map(|_| format!("{name} = {value}")).map_err(|e| {
                if e.kind() == std::io::ErrorKind::PermissionDenied {
                    mandara_i18n::tf("Changing {name} needs membership in the video group", &[("name", &name)])
                } else {
                    e.to_string()
                }
            })
        })
        .await;
        let _ = tx.send(result).await;
    });
    rx
}

/// Follows the primary backlight. backlight(9) has no change events, so
/// this re-reads it every [`BACKLIGHT_RESYNC`] (Mandara's own changes are
/// known at once).
#[cfg(target_os = "freebsd")]
pub fn watch_backlight() -> Option<async_channel::Receiver<Backlight>> {
    let device = primary()?;
    let (tx, rx) = async_channel::bounded(UPDATE_QUEUE);
    crate::runtime::spawn("backlight", async move {
        let mut last = device.brightness;
        while !tx.is_closed() {
            crate::runtime::sleep(BACKLIGHT_RESYNC).await;
            let name = device.name.clone();
            let Some(now) = crate::runtime::unblock(move || read(&name)).await else { return };
            if now.brightness != last {
                last = now.brightness;
                if tx.send(now).await.is_err() {
                    return;
                }
            }
        }
    });
    Some(rx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels_and_floor() {
        let b = Backlight { name: "x".into(), brightness: 480, max: 960 };
        assert!((b.level() - 0.5).abs() < 1e-6);
        assert_eq!(b.raw_for(1.0), 960);
        assert_eq!(b.raw_for(0.0), 10);
        assert_eq!(b.raw_for(0.25), 240);
        let tiny = Backlight { name: "y".into(), brightness: 3, max: 7 };
        assert_eq!(tiny.raw_for(0.0), 1);
    }

    #[test]
    fn absent_hardware_is_graceful() {
        // Never panics, whatever the machine has.
        let _ = devices();
        let _ = primary();
    }
}
