//! Everything that gets your attention: notifications (`[notifications]`
//! with `[[notifications.rules]]`), on-screen displays (`[osd]`), audio
//! device and media-player preferences (`[audio]`, `[media]`), shell event
//! sounds (`[sounds]`) and the microphone / camera / screen-share privacy
//! indicators (`[privacy]`). Which outputs show toasts and OSDs is
//! `[displays.surfaces]` (`crate::outputs::assignment`).
//!
//! Every struct is `#[serde(default)]` so older config files keep loading;
//! defaults live in [`defaults`] with their units in the name.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Default values of the alert settings.
pub mod defaults {
    /// Notification popup (toast) width, logical px.
    pub const TOAST_WIDTH_PX: f32 = 380.0;
    /// Toasts stacked at once; older ones wait in the history.
    pub const TOAST_MAX_VISIBLE: usize = 4;
    /// How long toasts stay, per urgency, seconds (0 = until dismissed).
    pub const TIMEOUT_LOW_S: f32 = 4.0;
    pub const TIMEOUT_NORMAL_S: f32 = 6.0;
    pub const TIMEOUT_CRITICAL_S: f32 = 0.0;
    /// Notifications kept in the history.
    pub const HISTORY_MAX: usize = 200;
    /// History entries older than this are dropped, hours (0 = never).
    pub const HISTORY_MAX_AGE_HOURS: u32 = 0;
    /// Quiet hours (do not disturb on a daily schedule), local `HH:MM`.
    pub const QUIET_FROM: &str = "22:00";
    pub const QUIET_TO: &str = "07:00";
    /// How long an on-screen display stays after the last change, ms.
    pub const OSD_HOLD_MS: u64 = 1500;
    /// Volume keys / scroll step, percent.
    pub const VOLUME_STEP_PERCENT: u32 = 5;
    /// Event sound volume, 0–1.
    pub const SOUND_VOLUME: f32 = 0.6;
    /// Camera check interval when the device can't notify, seconds.
    pub const CAMERA_POLL_S: u64 = 3;
}

// ── Notifications ────────────────────────────────────────────────────────

/// Where notification popups stack.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ToastPosition {
    TopLeft,
    TopCenter,
    #[default]
    TopRight,
    BottomLeft,
    BottomCenter,
    BottomRight,
}

impl ToastPosition {
    pub fn is_bottom(self) -> bool {
        matches!(self, ToastPosition::BottomLeft | ToastPosition::BottomCenter | ToastPosition::BottomRight)
    }

    /// `-1` left, `0` centre, `1` right.
    pub fn column(self) -> i8 {
        match self {
            ToastPosition::TopLeft | ToastPosition::BottomLeft => -1,
            ToastPosition::TopCenter | ToastPosition::BottomCenter => 0,
            ToastPosition::TopRight | ToastPosition::BottomRight => 1,
        }
    }
}

/// Notification urgency as written in the config.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum UrgencyLevel {
    Low,
    #[default]
    Normal,
    Critical,
}

/// The built-in notification server and its popups, history and quiet
/// modes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationsConfig {
    /// Serve `org.freedesktop.Notifications`. Off: another daemon (mako,
    /// swaync, dunst) can own the name; takes effect after a restart.
    pub daemon: bool,
    pub position: ToastPosition,
    pub max_visible: usize,
    /// Toast width, logical px.
    pub width: f32,
    /// Toast size multiplier.
    pub scale: f32,
    /// Toast background opacity, 0–1.
    pub opacity: f32,
    /// Seconds each urgency stays on screen (0 = until dismissed).
    pub timeout_low: f32,
    pub timeout_normal: f32,
    pub timeout_critical: f32,
    /// Honour the expiry an app asks for (off: always use the timeouts above).
    pub respect_app_timeout: bool,
    /// Hovering a toast pauses its countdown.
    pub pause_on_hover: bool,
    /// A thin countdown line along the toast's bottom edge.
    pub progress_bar: bool,
    pub show_actions: bool,
    pub show_app_name: bool,
    /// Clicking a toast's body runs the app's default action.
    pub click_action: bool,
    /// Toasts clamp their body to one line.
    pub compact: bool,
    /// Bold / italic / underline / links in bodies (`body-markup`).
    pub markup: bool,
    /// Toasts sit above fullscreen windows (overlay layer).
    pub over_fullscreen: bool,
    /// Toasts hide their body text until opened in the notification center.
    pub privacy: bool,
    /// A repeat of the same app + summary + body replaces the earlier one.
    pub dedupe: bool,
    /// Notifications below this urgency go straight to the history.
    pub min_urgency: UrgencyLevel,
    /// Keep a history at all.
    pub history: bool,
    pub history_max: usize,
    /// Drop entries older than this many hours (0 = keep).
    pub history_max_age_hours: u32,
    /// Low-urgency notifications are kept in the history.
    pub history_low: bool,
    /// Closing a popup only hides it; it stays in the history.
    pub keep_dismissed: bool,
    /// Critical notifications still pop up while do not disturb is on.
    pub dnd_allow_critical: bool,
    /// Do not disturb while the screen is being shared or recorded.
    pub dnd_screen_share: bool,
    /// Do not disturb while the focused window is fullscreen.
    pub dnd_fullscreen: bool,
    /// Do not disturb every day between `quiet_from` and `quiet_to`.
    pub quiet_hours: bool,
    pub quiet_from: String,
    pub quiet_to: String,
    /// Per-app rules, first match wins.
    pub rules: Vec<NotificationRule>,
}

impl Default for NotificationsConfig {
    fn default() -> Self {
        Self {
            daemon: true,
            position: ToastPosition::default(),
            max_visible: defaults::TOAST_MAX_VISIBLE,
            width: defaults::TOAST_WIDTH_PX,
            scale: 1.0,
            opacity: 1.0,
            timeout_low: defaults::TIMEOUT_LOW_S,
            timeout_normal: defaults::TIMEOUT_NORMAL_S,
            timeout_critical: defaults::TIMEOUT_CRITICAL_S,
            respect_app_timeout: true,
            pause_on_hover: true,
            progress_bar: true,
            show_actions: true,
            show_app_name: true,
            click_action: true,
            compact: false,
            markup: true,
            over_fullscreen: false,
            privacy: false,
            dedupe: true,
            min_urgency: UrgencyLevel::Low,
            history: true,
            history_max: defaults::HISTORY_MAX,
            history_max_age_hours: defaults::HISTORY_MAX_AGE_HOURS,
            history_low: true,
            keep_dismissed: false,
            dnd_allow_critical: true,
            dnd_screen_share: true,
            dnd_fullscreen: false,
            quiet_hours: false,
            quiet_from: defaults::QUIET_FROM.into(),
            quiet_to: defaults::QUIET_TO.into(),
            rules: Vec::new(),
        }
    }
}

impl NotificationsConfig {
    /// The configured popup time for `urgency`, seconds (0 = sticky).
    pub fn timeout_for(&self, urgency: UrgencyLevel) -> f32 {
        match urgency {
            UrgencyLevel::Low => self.timeout_low,
            UrgencyLevel::Normal => self.timeout_normal,
            UrgencyLevel::Critical => self.timeout_critical,
        }
    }
}

/// A per-app notification rule. Every non-empty pattern must match
/// (case-insensitive regular expressions; plain words match as text).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NotificationRule {
    pub enabled: bool,
    /// App name or desktop entry.
    pub app: String,
    pub summary: String,
    pub body: String,
    /// The `category` hint (`email.arrived`, `im.received`…).
    pub category: String,
    /// Only notifications sent with this urgency.
    pub urgency: Option<UrgencyLevel>,
    /// Drop matching notifications entirely.
    pub block: bool,
    /// Show a popup (off: straight to the history).
    pub popup: bool,
    /// Keep in the history.
    pub history: bool,
    /// Play the notification sound.
    pub sound: bool,
    /// Pop up even during do not disturb.
    pub bypass_dnd: bool,
    /// Treat as this urgency instead.
    pub set_urgency: Option<UrgencyLevel>,
    /// Popup time in seconds instead of the urgency's (0 = until dismissed).
    pub timeout: Option<f32>,
}

impl Default for NotificationRule {
    fn default() -> Self {
        Self {
            enabled: true,
            app: String::new(),
            summary: String::new(),
            body: String::new(),
            category: String::new(),
            urgency: None,
            block: false,
            popup: true,
            history: true,
            sound: true,
            bypass_dnd: false,
            set_urgency: None,
            timeout: None,
        }
    }
}

impl NotificationRule {
    /// A rule matching one app, with default effects.
    pub fn for_app(app: &str) -> Self {
        Self { app: app.to_string(), ..Self::default() }
    }
}

// ── On-screen displays ───────────────────────────────────────────────────

/// Which events show an on-screen display.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OsdKinds {
    pub volume: bool,
    pub microphone: bool,
    pub brightness: bool,
    pub keyboard_layout: bool,
    /// Caps / Num / Scroll Lock.
    pub lock_keys: bool,
    /// Track changes of the followed media player.
    pub media: bool,
    pub power_profile: bool,
    pub do_not_disturb: bool,
    pub night_light: bool,
    pub keyboard_backlight: bool,
    /// Microphone, camera or screen sharing started / stopped.
    pub privacy: bool,
    /// The default audio output switched.
    pub output_device: bool,
    /// `mandara msg osd show …` from scripts and plugins.
    pub custom: bool,
}

impl Default for OsdKinds {
    fn default() -> Self {
        Self {
            volume: true,
            microphone: true,
            brightness: true,
            keyboard_layout: true,
            lock_keys: true,
            media: false,
            power_profile: true,
            do_not_disturb: true,
            night_light: true,
            keyboard_backlight: true,
            privacy: true,
            output_device: true,
            custom: true,
        }
    }
}

/// On-screen display look and behaviour. The master switch and position
/// stay in `[general]` (`osd`, `osd_position`) for older configs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct OsdConfig {
    pub kinds: OsdKinds,
    /// How long the pill stays after the last change, ms.
    pub hold_ms: u64,
    /// Size multiplier.
    pub scale: f32,
    /// Background opacity, 0–1.
    pub opacity: f32,
    /// Show the numeric value next to level bars.
    pub show_value: bool,
    /// Extra offset from the anchored edge, logical px.
    pub offset_x: f32,
    pub offset_y: f32,
}

impl Default for OsdConfig {
    fn default() -> Self {
        Self { kinds: OsdKinds::default(), hold_ms: defaults::OSD_HOLD_MS, scale: 1.0, opacity: 1.0, show_value: true, offset_x: 0.0, offset_y: 0.0 }
    }
}

// ── Audio & media ────────────────────────────────────────────────────────

/// Sends new streams of matching apps to a chosen output.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioRoute {
    /// App name, binary, app id or media role (`music`, `phone`, `game`…),
    /// case-insensitive.
    pub app: String,
    /// Output device (server name).
    pub output: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Volume keys and scroll step, percent.
    pub volume_step: u32,
    /// Allow volume above 100 % (up to 150 %).
    pub overamplify: bool,
    /// Device server names left out of pickers and the bar panel.
    pub hidden_devices: Vec<String>,
    /// Your names for devices, keyed by server name.
    pub device_labels: BTreeMap<String, String>,
    /// Per-app (or per-role) default outputs.
    pub routes: Vec<AudioRoute>,
}

impl Default for AudioConfig {
    fn default() -> Self {
        Self { volume_step: defaults::VOLUME_STEP_PERCENT, overamplify: true, hidden_devices: Vec::new(), device_labels: BTreeMap::new(), routes: Vec::new() }
    }
}

impl AudioConfig {
    /// The highest volume allowed (linear, `1.0` = 100 %).
    pub fn volume_limit(&self) -> f32 {
        /// The ceiling with over-amplification on.
        const OVERAMPLIFIED: f32 = 1.5;
        if self.overamplify { OVERAMPLIFIED } else { 1.0 }
    }

    /// The name shown for a device: your label, else its description.
    pub fn device_label<'a>(&'a self, name: &str, description: &'a str) -> &'a str {
        self.device_labels.get(name).map(String::as_str).filter(|l| !l.trim().is_empty()).unwrap_or(description)
    }

    pub fn is_hidden(&self, name: &str) -> bool {
        self.hidden_devices.iter().any(|h| h == name)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaConfig {
    /// Players never followed (identity or bus name, case-insensitive
    /// substring: `firefox`, `kdeconnect`…).
    pub ignore_players: Vec<String>,
    /// Switch to whichever player starts playing.
    pub follow_playing: bool,
}

impl Default for MediaConfig {
    fn default() -> Self {
        Self { ignore_players: Vec::new(), follow_playing: true }
    }
}

impl MediaConfig {
    /// Whether a player (identity / bus name) is on the ignore list.
    pub fn ignores(&self, identity: &str, bus_name: &str) -> bool {
        let (identity, bus) = (identity.to_lowercase(), bus_name.to_lowercase());
        self.ignore_players.iter().map(|p| p.trim().to_lowercase()).filter(|p| !p.is_empty()).any(|p| identity.contains(&p) || bus.contains(&p))
    }
}

// ── Sounds ───────────────────────────────────────────────────────────────

/// Shell event sounds from the freedesktop sound theme.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SoundsConfig {
    pub enabled: bool,
    /// Sound theme name; empty follows GTK's `gtk-sound-theme-name`.
    pub theme: String,
    /// 0–1.
    pub volume: f32,
    /// Stay quiet while music or video plays.
    pub mute_while_media: bool,
    pub volume_change: bool,
    pub notification_low: bool,
    pub notification: bool,
    pub notification_critical: bool,
    pub charger: bool,
    pub screenshot: bool,
    pub login: bool,
    /// Your own files per event (`volume-change`, `notification`, …).
    pub files: BTreeMap<String, String>,
}

impl Default for SoundsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            theme: String::new(),
            volume: defaults::SOUND_VOLUME,
            mute_while_media: true,
            volume_change: true,
            notification_low: false,
            notification: true,
            notification_critical: true,
            charger: true,
            screenshot: true,
            login: true,
            files: BTreeMap::new(),
        }
    }
}

// ── Privacy indicators ───────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PrivacyConfig {
    /// A bar chip while the microphone, camera or screen is in use.
    pub indicators: bool,
    pub microphone: bool,
    pub camera: bool,
    pub screen_share: bool,
    /// Apps never reported (case-insensitive regular expressions).
    pub ignore_microphone: Vec<String>,
    pub ignore_camera: Vec<String>,
    pub ignore_screen_share: Vec<String>,
    /// Camera re-check interval when the device can't notify, seconds.
    pub camera_poll_s: u64,
}

impl Default for PrivacyConfig {
    fn default() -> Self {
        Self {
            indicators: true,
            microphone: true,
            camera: true,
            screen_share: true,
            ignore_microphone: Vec::new(),
            ignore_camera: Vec::new(),
            ignore_screen_share: Vec::new(),
            camera_poll_s: defaults::CAMERA_POLL_S,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ShellConfig;

    #[test]
    fn sections_default_and_round_trip() {
        let cfg = ShellConfig::default();
        let text = toml::to_string(&cfg).unwrap();
        let back: ShellConfig = toml::from_str(&text).unwrap();
        assert_eq!(back.notifications, cfg.notifications);
        assert_eq!(back.osd, cfg.osd);
        assert_eq!(back.sounds, cfg.sounds);
        // Older configs without the sections still load.
        let old: ShellConfig = toml::from_str("[general]\nosd = false\n").unwrap();
        assert_eq!(old.notifications.max_visible, defaults::TOAST_MAX_VISIBLE);
        assert!(!old.general.osd && old.osd.kinds.volume);
    }

    #[test]
    fn rules_parse_with_partial_fields() {
        let cfg: ShellConfig = toml::from_str("[[notifications.rules]]\napp = \"Spotify\"\npopup = false\n[[notifications.rules]]\napp = \"backup\"\nset_urgency = \"low\"\ntimeout = 2.5\n").unwrap();
        let r = &cfg.notifications.rules;
        assert_eq!(r.len(), 2);
        assert!(r[0].enabled && !r[0].popup && r[0].history);
        assert_eq!(r[1].set_urgency, Some(UrgencyLevel::Low));
        assert_eq!(r[1].timeout, Some(2.5));
    }

    #[test]
    fn media_ignore_and_device_labels() {
        let m = MediaConfig { ignore_players: vec!["Firefox".into(), " ".into()], ..Default::default() };
        assert!(m.ignores("Mozilla Firefox", "org.mpris.MediaPlayer2.firefox.instance_1"));
        assert!(!m.ignores("Spotify", "org.mpris.MediaPlayer2.spotify"));
        let mut a = AudioConfig::default();
        a.device_labels.insert("alsa_output.usb".into(), "Desk speakers".into());
        assert_eq!(a.device_label("alsa_output.usb", "USB Audio"), "Desk speakers");
        assert_eq!(a.device_label("other", "HDMI"), "HDMI");
        assert_eq!(a.volume_limit(), 1.5);
        a.overamplify = false;
        assert_eq!(a.volume_limit(), 1.0);
        assert!(ToastPosition::BottomCenter.is_bottom() && ToastPosition::TopLeft.column() == -1);
    }
}
