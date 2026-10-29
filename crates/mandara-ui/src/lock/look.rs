//! The lock screen's configurable look (`[lock]`, `datetime.lock_*`).
//!
//! The locker is its own process, so what only the shell knows (track
//! playing, unread notifications, current weather) is handed over as a
//! snapshot in environment variables when the shell starts it; the battery
//! is read live from sysfs.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use mandara_config::ShellConfig;
use mandara_config::desktop::{LockBackground, LockClockStyle, LockConfig};

/// Environment variables carrying the shell's snapshot to the locker.
pub const MEDIA_ENV: &str = "MANDARA_LOCK_MEDIA";
pub const NOTIFICATIONS_ENV: &str = "MANDARA_LOCK_NOTIFICATIONS";
pub const WEATHER_ENV: &str = "MANDARA_LOCK_WEATHER";
/// sysfs power supplies (batteries report `type` = `Battery`).
const POWER_SUPPLY: &str = "/sys/class/power_supply";
/// Blur radius → Gaussian sigma on the 480 px working copy (the default
/// 28 px radius gives the original 7.0 sigma).
const BLUR_SIGMA_PER_PX: f32 = 0.25;

/// What the shell last saw, for the next locker it starts.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Snapshot {
    pub media: Option<String>,
    pub notifications: usize,
    pub weather: Option<String>,
}

static SNAPSHOT: Mutex<Option<Snapshot>> = Mutex::new(None);

/// Records the shell's current state (shell process).
pub fn set_snapshot(s: Snapshot) {
    if let Ok(mut slot) = SNAPSHOT.lock() {
        *slot = Some(s);
    }
}

/// Passes the snapshot to a locker command.
pub fn export(cmd: &mut std::process::Command) {
    let Some(s) = SNAPSHOT.lock().ok().and_then(|s| s.clone()) else { return };
    if let Some(m) = &s.media {
        cmd.env(MEDIA_ENV, m);
    }
    cmd.env(NOTIFICATIONS_ENV, s.notifications.to_string());
    if let Some(w) = &s.weather {
        cmd.env(WEATHER_ENV, w);
    }
}

/// Keeps the snapshot current from the hub (shell process).
pub fn init(cx: &mut gpui::App) {
    let hub = crate::hub::hub(cx);
    let update = |hub: &crate::hub::Hub, cfg: &ShellConfig| {
        let media = hub.media.as_ref().filter(|m| m.status == mandara_services::PlaybackStatus::Playing && !m.title.is_empty()).map(|m| {
            if m.artist.is_empty() { m.title.clone() } else { format!("{} — {}", m.title, m.artist) }
        });
        let weather = hub.weather.weather.as_ref().map(|w| cfg.units().format_temperature(w.temp));
        set_snapshot(Snapshot { media, notifications: hub.notifications.unread(), weather });
    };
    update(hub.read(cx), &crate::state::config(cx).read(cx).snapshot());
    cx.observe(&hub, move |hub, cx| update(hub.read(cx), &crate::state::config(cx).read(cx).snapshot())).detach();
}

/// Battery `(percent, charging)` of the first system battery.
pub fn battery() -> Option<(u8, bool)> {
    battery_in(Path::new(POWER_SUPPLY))
}

fn battery_in(root: &Path) -> Option<(u8, bool)> {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(root).ok()?.flatten().map(|e| e.path()).collect();
    entries.sort();
    entries.into_iter().find_map(|dir| {
        let kind = std::fs::read_to_string(dir.join("type")).ok()?;
        // Peripheral batteries (mice, headsets) report `scope` = Device.
        let device = std::fs::read_to_string(dir.join("scope")).is_ok_and(|s| s.trim() == "Device");
        if kind.trim() != "Battery" || device {
            return None;
        }
        let capacity: u8 = std::fs::read_to_string(dir.join("capacity")).ok()?.trim().parse().ok()?;
        let status = std::fs::read_to_string(dir.join("status")).unwrap_or_default();
        Some((capacity.min(100), matches!(status.trim(), "Charging" | "Full")))
    })
}

/// Everything the lock screen draws beyond its fixed parts.
#[derive(Clone, Debug)]
pub struct LockLook {
    pub cfg: LockConfig,
    /// Clock / date strftime patterns (validated when drawn).
    pub time_pattern: String,
    pub date_pattern: Option<String>,
    pub clock_24h: bool,
    /// A custom time pattern is set (automatic 12-hour clocks draw AM/PM
    /// small beside the digits instead).
    pub custom_time: bool,
    pub snapshot: Snapshot,
}

impl Default for LockLook {
    fn default() -> Self {
        Self { cfg: LockConfig::default(), time_pattern: "%H:%M".into(), date_pattern: None, clock_24h: true, custom_time: false, snapshot: Snapshot::default() }
    }
}

impl LockLook {
    /// From the config plus the snapshot in this process's environment.
    pub fn from_config(cfg: &ShellConfig) -> Self {
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.trim().is_empty());
        let custom_date = cfg.datetime.lock_date_format.trim();
        Self {
            cfg: cfg.lock.clone(),
            time_pattern: cfg.datetime.lock_time_pattern(cfg.general.clock_24h),
            date_pattern: (!custom_date.is_empty()).then(|| custom_date.to_string()),
            clock_24h: cfg.general.clock_24h,
            custom_time: !cfg.datetime.lock_time_format.trim().is_empty(),
            snapshot: Snapshot {
                media: env(MEDIA_ENV).filter(|_| cfg.lock.show_media),
                notifications: env(NOTIFICATIONS_ENV).and_then(|n| n.parse().ok()).filter(|_| cfg.lock.show_notifications).unwrap_or(0),
                weather: env(WEATHER_ENV).filter(|_| cfg.lock.show_weather),
            },
        }
    }

    /// The image behind the lock screen, if any.
    pub fn background_image(&self, wallpaper: Option<PathBuf>) -> Option<PathBuf> {
        match self.cfg.background {
            LockBackground::Wallpaper => wallpaper,
            LockBackground::Image => self.cfg.image.as_ref().map(|p| mandara_config::paths::expand_tilde(&p.to_string_lossy())).or(wallpaper),
            LockBackground::Color => None,
        }
    }

    /// Gaussian sigma for the background blur (0 = sharp).
    pub fn blur_sigma(&self) -> f32 {
        (self.cfg.blur.max(0.0) * BLUR_SIGMA_PER_PX).max(0.0)
    }

    pub fn stacked(&self) -> bool {
        self.cfg.clock_style == LockClockStyle::Stacked
    }

    pub fn compact(&self) -> bool {
        self.cfg.clock_style == LockClockStyle::Compact
    }

    /// The info line under the date: now playing · weather · notifications.
    pub fn info_line(&self) -> Option<String> {
        let mut parts: Vec<String> = Vec::new();
        if let Some(w) = &self.snapshot.weather {
            parts.push(w.clone());
        }
        if let Some(m) = &self.snapshot.media {
            parts.push(format!("♪ {m}"));
        }
        if self.snapshot.notifications > 0 {
            parts.push(mandara_i18n::tf("{count} notifications", &[("count", &self.snapshot.notifications)]));
        }
        (!parts.is_empty()).then(|| parts.join("   ·   "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn background_choice_and_blur() {
        let mut look = LockLook::default();
        let wp = Some(PathBuf::from("/w.jpg"));
        assert_eq!(look.background_image(wp.clone()), wp);
        look.cfg.background = LockBackground::Color;
        assert_eq!(look.background_image(wp.clone()), None);
        look.cfg.background = LockBackground::Image;
        assert_eq!(look.background_image(wp.clone()), wp, "no image chosen → wallpaper");
        look.cfg.image = Some("/lock.png".into());
        assert_eq!(look.background_image(wp), Some("/lock.png".into()));
        assert!((LockLook::default().blur_sigma() - 7.0).abs() < 1e-4, "default matches the original blur");
    }

    #[test]
    fn info_line_joins_what_is_shown() {
        let mut look = LockLook::default();
        assert_eq!(look.info_line(), None);
        look.snapshot = Snapshot { media: Some("Song — Artist".into()), notifications: 3, weather: Some("23°C".into()) };
        let line = look.info_line().unwrap();
        assert!(line.starts_with("23°C") && line.contains("♪ Song — Artist") && line.contains('3'));
    }

    #[test]
    fn reads_the_system_battery_only() {
        let dir = std::env::temp_dir().join(format!("mandara-ps-{}", std::process::id()));
        let mouse = dir.join("hidpp_battery_0");
        let bat = dir.join("BAT0");
        for (d, kind) in [(&mouse, "Battery"), (&bat, "Battery")] {
            std::fs::create_dir_all(d).unwrap();
            std::fs::write(d.join("type"), format!("{kind}\n")).unwrap();
        }
        std::fs::write(mouse.join("scope"), "Device\n").unwrap();
        std::fs::write(mouse.join("capacity"), "12\n").unwrap();
        std::fs::write(bat.join("capacity"), "81\n").unwrap();
        std::fs::write(bat.join("status"), "Charging\n").unwrap();
        assert_eq!(battery_in(&dir), Some((81, true)));
        std::fs::remove_dir_all(dir).unwrap();
    }
}
