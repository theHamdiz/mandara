//! Power-management settings: what happens when you step away (`[idle]`),
//! battery alerts and power profiles (`[power]`), the power / session menu
//! (`[session_menu]`) and event hooks (`[hooks]`).
//!
//! Every struct is `#[serde(default)]` so older config files keep loading;
//! defaults live in [`defaults`] with their units in the name.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Default values of the power-management settings.
pub mod defaults {
    /// Idle timeouts on mains power, seconds (0 = never).
    pub const AC_LOCK_S: u32 = 600;
    pub const AC_SCREEN_OFF_S: u32 = 900;
    pub const AC_SUSPEND_S: u32 = 0;
    /// Idle timeouts on battery, seconds (0 = never).
    pub const BATTERY_LOCK_S: u32 = 300;
    pub const BATTERY_SCREEN_OFF_S: u32 = 360;
    pub const BATTERY_SUSPEND_S: u32 = 900;
    /// The screen fades this long before locking / turning off; any input
    /// during the fade cancels it.
    pub const FADE_S: u32 = 8;
    /// While locked, screens turn off after this much idle time.
    pub const LOCKED_SCREEN_OFF_S: u32 = 60;
    /// After an automatic lock, input within this window unlocks without a
    /// password (0 = always ask).
    pub const LOCK_GRACE_S: u32 = 0;
    /// Battery alert thresholds, percent.
    pub const LOW_PERCENT: u8 = 20;
    pub const CRITICAL_PERCENT: u8 = 8;
    /// The critical action runs at this charge, percent.
    pub const ACTION_PERCENT: u8 = 4;
    /// Warning shown before the critical action runs, seconds.
    pub const ACTION_DELAY_S: u32 = 60;
    /// Peripheral (mouse, headset…) low-battery threshold, percent.
    pub const PERIPHERAL_LOW_PERCENT: u8 = 15;
    /// Press-and-hold time of hold-to-confirm session actions, ms.
    pub const HOLD_MS: u64 = 900;
    /// Session menu grid columns.
    pub const GRID_COLUMNS: u8 = 3;
    /// Hooks are killed after this long unless the rule sets its own, s.
    pub const HOOK_TIMEOUT_S: u32 = 30;
}

/// Upper bound of every percent setting.
pub const PERCENT_MAX: u8 = 100;

// ───────────────────────────────── idle ─────────────────────────────────

/// Idle timeouts for one power source, seconds; 0 disables a stage.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IdleTimeouts {
    pub lock_s: u32,
    pub screen_off_s: u32,
    pub suspend_s: u32,
}

impl IdleTimeouts {
    pub const fn ac() -> Self {
        Self { lock_s: defaults::AC_LOCK_S, screen_off_s: defaults::AC_SCREEN_OFF_S, suspend_s: defaults::AC_SUSPEND_S }
    }

    pub const fn battery() -> Self {
        Self { lock_s: defaults::BATTERY_LOCK_S, screen_off_s: defaults::BATTERY_SCREEN_OFF_S, suspend_s: defaults::BATTERY_SUSPEND_S }
    }
}

impl Default for IdleTimeouts {
    fn default() -> Self {
        Self::ac()
    }
}

/// `[idle.battery]` with each missing key filled from
/// [`IdleTimeouts::battery`] (the struct's own default is the mains one).
fn battery_timeouts<'de, D: serde::Deserializer<'de>>(d: D) -> Result<IdleTimeouts, D::Error> {
    #[derive(Deserialize)]
    struct Partial {
        lock_s: Option<u32>,
        screen_off_s: Option<u32>,
        suspend_s: Option<u32>,
    }
    let p = Partial::deserialize(d)?;
    let base = IdleTimeouts::battery();
    Ok(IdleTimeouts { lock_s: p.lock_s.unwrap_or(base.lock_s), screen_off_s: p.screen_off_s.unwrap_or(base.screen_off_s), suspend_s: p.suspend_s.unwrap_or(base.suspend_s) })
}

/// How the computer sleeps when idle (or at critical battery).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SuspendMode {
    /// Suspend to RAM.
    #[default]
    Suspend,
    /// Suspend to disk (needs swap / a resume device).
    Hibernate,
    /// Suspend, then hibernate after the system's delay.
    SuspendThenHibernate,
    /// Save to disk and suspend to RAM.
    HybridSleep,
}

impl SuspendMode {
    pub const ALL: [SuspendMode; 4] = [SuspendMode::Suspend, SuspendMode::Hibernate, SuspendMode::SuspendThenHibernate, SuspendMode::HybridSleep];

    /// The `org.freedesktop.login1.Manager` method.
    pub fn logind_method(self) -> &'static str {
        match self {
            SuspendMode::Suspend => "Suspend",
            SuspendMode::Hibernate => "Hibernate",
            SuspendMode::SuspendThenHibernate => "SuspendThenHibernate",
            SuspendMode::HybridSleep => "HybridSleep",
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            SuspendMode::Suspend => "suspend",
            SuspendMode::Hibernate => "hibernate",
            SuspendMode::SuspendThenHibernate => "suspend-then-hibernate",
            SuspendMode::HybridSleep => "hybrid-sleep",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.id() == id)
    }
}

/// Which power source a rule applies on.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PowerSource {
    #[default]
    Any,
    Ac,
    Battery,
}

impl PowerSource {
    pub fn applies(self, on_battery: bool) -> bool {
        match self {
            PowerSource::Any => true,
            PowerSource::Ac => !on_battery,
            PowerSource::Battery => on_battery,
        }
    }
}

/// A custom idle behaviour: run `command` after `timeout_s` idle seconds
/// and `resume_command` when you come back.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct IdleBehavior {
    pub name: String,
    pub enabled: bool,
    pub timeout_s: u32,
    pub command: String,
    pub resume_command: String,
    pub power: PowerSource,
}

impl Default for IdleBehavior {
    fn default() -> Self {
        Self { name: String::new(), enabled: true, timeout_s: defaults::AC_LOCK_S, command: String::new(), resume_command: String::new(), power: PowerSource::Any }
    }
}

/// What happens when you step away, and when the screen locks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IdleConfig {
    /// Mandara watches for idleness (`ext-idle-notify-v1`) and runs the
    /// stages below. Off: leave idle handling to hypridle / swayidle.
    pub enabled: bool,
    pub ac: IdleTimeouts,
    /// Keys missing from a hand-written `[idle.battery]` take the battery
    /// defaults, not the mains ones.
    #[serde(deserialize_with = "battery_timeouts")]
    pub battery: IdleTimeouts,
    /// Fade the screens out for this long before locking or turning them
    /// off; moving the mouse or typing cancels. 0 = no fade.
    pub fade_s: u32,
    /// What "suspend" means for the idle stage and the session menu's Sleep.
    pub suspend_mode: SuspendMode,
    /// While locked, turn the screens off after this much idle time (also
    /// while a video inhibits idle). 0 = never.
    pub locked_screen_off_s: u32,
    /// Playing media (MPRIS) keeps the computer awake.
    pub respect_media: bool,
    /// Lock before the computer sleeps (lid, menu, `systemctl suspend`).
    pub lock_before_sleep: bool,
    /// Lock when the laptop lid closes, even if it does not suspend.
    pub lock_on_lid_close: bool,
    /// Lock once when the session starts (not on shell restarts).
    pub lock_at_startup: bool,
    /// After an automatic (idle) lock, any input within this many seconds
    /// unlocks without a password. 0 = always ask.
    pub lock_grace_s: u32,
    /// Your own commands after N idle seconds.
    pub behaviors: Vec<IdleBehavior>,
}

impl Default for IdleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            ac: IdleTimeouts::ac(),
            battery: IdleTimeouts::battery(),
            fade_s: defaults::FADE_S,
            suspend_mode: SuspendMode::Suspend,
            locked_screen_off_s: defaults::LOCKED_SCREEN_OFF_S,
            respect_media: true,
            lock_before_sleep: true,
            lock_on_lid_close: false,
            lock_at_startup: false,
            lock_grace_s: defaults::LOCK_GRACE_S,
            behaviors: Vec::new(),
        }
    }
}

impl IdleConfig {
    /// The timeouts for the current power source.
    pub fn timeouts(&self, on_battery: bool) -> IdleTimeouts {
        if on_battery { self.battery } else { self.ac }
    }
}

// ───────────────────────────── battery & power ─────────────────────────────

/// How a battery alert is shown.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AlertStyle {
    /// A notification and an on-screen display.
    #[default]
    Both,
    Notification,
    Osd,
    Off,
}

impl AlertStyle {
    pub fn notifies(self) -> bool {
        matches!(self, AlertStyle::Both | AlertStyle::Notification)
    }

    pub fn shows_osd(self) -> bool {
        matches!(self, AlertStyle::Both | AlertStyle::Osd)
    }
}

/// What to do when the battery is nearly empty.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum CriticalAction {
    Nothing,
    /// Sleep the way `idle.suspend_mode` says.
    #[default]
    Suspend,
    Hibernate,
    Shutdown,
}

/// Battery alerts and automatic power profiles.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PowerConfig {
    pub low_percent: u8,
    pub low_alert: AlertStyle,
    pub critical_percent: u8,
    pub critical_alert: AlertStyle,
    /// Runs at `action_percent` while discharging, after a warning.
    pub critical_action: CriticalAction,
    pub action_percent: u8,
    pub action_delay_s: u32,
    /// Warn about mice, keyboards, headsets… running low.
    pub peripheral_alerts: bool,
    pub peripheral_low_percent: u8,
    /// Per-device thresholds keyed by model (or serial), overriding
    /// `peripheral_low_percent`.
    pub device_thresholds: BTreeMap<String, u8>,
    /// Switch to the power-saver profile at the low threshold (and back
    /// when charging).
    pub saver_at_low: bool,
    /// Profile to switch to on mains power / on battery (`None` = leave it).
    pub profile_ac: Option<String>,
    pub profile_battery: Option<String>,
    /// Notify when the battery is full or reaches its charge limit.
    pub notify_full: bool,
    /// The battery chip pulses while the charge is critical.
    pub critical_pulse: bool,
}

impl Default for PowerConfig {
    fn default() -> Self {
        Self {
            low_percent: defaults::LOW_PERCENT,
            low_alert: AlertStyle::Both,
            critical_percent: defaults::CRITICAL_PERCENT,
            critical_alert: AlertStyle::Both,
            critical_action: CriticalAction::Suspend,
            action_percent: defaults::ACTION_PERCENT,
            action_delay_s: defaults::ACTION_DELAY_S,
            peripheral_alerts: true,
            peripheral_low_percent: defaults::PERIPHERAL_LOW_PERCENT,
            device_thresholds: BTreeMap::new(),
            saver_at_low: false,
            profile_ac: None,
            profile_battery: None,
            notify_full: false,
            critical_pulse: true,
        }
    }
}

impl PowerConfig {
    /// The low threshold of a peripheral (model or serial override first).
    pub fn peripheral_threshold(&self, model: &str, serial: &str) -> u8 {
        [model, serial].iter().filter(|k| !k.is_empty()).find_map(|k| self.device_thresholds.get(*k).copied()).unwrap_or(self.peripheral_low_percent)
    }
}

// ───────────────────────────── session menu ─────────────────────────────

/// What a session-menu entry does.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionEntryKind {
    #[default]
    Lock,
    Logout,
    Suspend,
    Hibernate,
    Reboot,
    Shutdown,
    /// Restart userspace only (`systemctl soft-reboot`).
    SoftReboot,
    /// Restart into the firmware (UEFI) setup.
    Firmware,
    /// Restart into a boot-loader entry (`boot_entry`).
    BootEntry,
    /// Restart Mandara Shell.
    RestartShell,
    /// Run `command`.
    Custom,
}

impl SessionEntryKind {
    pub const ALL: [SessionEntryKind; 11] = [
        SessionEntryKind::Lock,
        SessionEntryKind::Logout,
        SessionEntryKind::Suspend,
        SessionEntryKind::Hibernate,
        SessionEntryKind::Reboot,
        SessionEntryKind::Shutdown,
        SessionEntryKind::SoftReboot,
        SessionEntryKind::Firmware,
        SessionEntryKind::BootEntry,
        SessionEntryKind::RestartShell,
        SessionEntryKind::Custom,
    ];

    pub fn id(self) -> &'static str {
        match self {
            SessionEntryKind::Lock => "lock",
            SessionEntryKind::Logout => "logout",
            SessionEntryKind::Suspend => "suspend",
            SessionEntryKind::Hibernate => "hibernate",
            SessionEntryKind::Reboot => "reboot",
            SessionEntryKind::Shutdown => "shutdown",
            SessionEntryKind::SoftReboot => "soft-reboot",
            SessionEntryKind::Firmware => "firmware",
            SessionEntryKind::BootEntry => "boot-entry",
            SessionEntryKind::RestartShell => "restart-shell",
            SessionEntryKind::Custom => "custom",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.id() == id)
    }

    /// Ends the session or restarts / powers off the machine.
    pub fn is_destructive(self) -> bool {
        matches!(
            self,
            SessionEntryKind::Logout
                | SessionEntryKind::Reboot
                | SessionEntryKind::Shutdown
                | SessionEntryKind::SoftReboot
                | SessionEntryKind::Firmware
                | SessionEntryKind::BootEntry
        )
    }
}

/// One entry of the power / session menu.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionEntry {
    pub kind: SessionEntryKind,
    pub enabled: bool,
    /// Empty: the built-in name.
    pub label: String,
    /// Bundled icon name; empty: the built-in one.
    pub icon: String,
    /// `custom`: the command to run (no shell unless you write `sh -c`).
    pub command: String,
    /// `boot-entry`: the boot-loader entry id (`mandara msg session boot-entries`).
    pub boot_entry: String,
}

impl SessionEntry {
    pub fn new(kind: SessionEntryKind, enabled: bool) -> Self {
        Self { kind, enabled, ..Default::default() }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SessionMenuStyle {
    /// A column docked to the screen edge (the default).
    #[default]
    Dock,
    /// A centered grid of large tiles.
    Grid,
}

/// The power / session menu.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SessionMenuConfig {
    /// Entries in order; disabled ones are hidden.
    /// Built-in entries missing from the list (an older or hand-written
    /// config) come back disabled on load, so Studio's editor can turn
    /// them on again.
    #[serde(deserialize_with = "session_entries")]
    pub entries: Vec<SessionEntry>,
    pub style: SessionMenuStyle,
    pub grid_columns: u8,
    /// Destructive entries need a press-and-hold (`hold_ms`).
    pub hold_to_confirm: bool,
    pub hold_ms: u64,
    /// The entry selected when the menu opens (its kind id).
    pub default_action: String,
    /// Number-key hints (1–9 run an entry).
    pub show_shortcuts: bool,
}

impl Default for SessionMenuConfig {
    fn default() -> Self {
        use SessionEntryKind::*;
        let on = [Lock, Logout, Suspend, Reboot, Shutdown];
        let off = [Hibernate, SoftReboot, Firmware, RestartShell];
        Self {
            entries: on.iter().map(|k| SessionEntry::new(*k, true)).chain(off.iter().map(|k| SessionEntry::new(*k, false))).collect(),
            style: SessionMenuStyle::Dock,
            grid_columns: defaults::GRID_COLUMNS,
            hold_to_confirm: false,
            hold_ms: defaults::HOLD_MS,
            default_action: Lock.id().into(),
            show_shortcuts: true,
        }
    }
}

fn session_entries<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<SessionEntry>, D::Error> {
    let mut entries = Vec::<SessionEntry>::deserialize(d)?;
    SessionMenuConfig::restore_builtins(&mut entries);
    Ok(entries)
}

impl SessionMenuConfig {
    /// Appends, disabled, every built-in kind the list lacks (not the
    /// repeatable boot-entry / custom kinds).
    pub fn restore_builtins(entries: &mut Vec<SessionEntry>) {
        for kind in SessionEntryKind::ALL {
            if !matches!(kind, SessionEntryKind::BootEntry | SessionEntryKind::Custom) && !entries.iter().any(|e| e.kind == kind) {
                entries.push(SessionEntry::new(kind, false));
            }
        }
    }

    /// The entries shown, in order.
    pub fn visible(&self) -> impl Iterator<Item = &SessionEntry> {
        self.entries.iter().filter(|e| e.enabled && (e.kind != SessionEntryKind::Custom || !e.command.trim().is_empty()))
    }
}

// ───────────────────────────────── hooks ─────────────────────────────────

/// Shell events a hook can run on.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum HookEvent {
    /// The shell started (also after a restart).
    #[default]
    Started,
    WallpaperChanged,
    /// The palette changed (theme, wallpaper colors, adjustments).
    ColorsChanged,
    ThemeModeChanged,
    /// Theme files for other apps were written.
    TemplatesRendered,
    Locked,
    Unlocked,
    Idle,
    Resumed,
    ScreensOff,
    ScreensOn,
    BeforeSleep,
    AfterSleep,
    BeforeLogout,
    BeforeReboot,
    BeforeShutdown,
    PluggedIn,
    Unplugged,
    BatteryLevel,
    BatteryLow,
    BatteryCritical,
    BatteryFull,
    PowerProfileChanged,
    WifiOn,
    WifiOff,
    BluetoothOn,
    BluetoothOff,
}

impl HookEvent {
    pub const ALL: [HookEvent; 27] = [
        HookEvent::Started,
        HookEvent::WallpaperChanged,
        HookEvent::ColorsChanged,
        HookEvent::ThemeModeChanged,
        HookEvent::TemplatesRendered,
        HookEvent::Locked,
        HookEvent::Unlocked,
        HookEvent::Idle,
        HookEvent::Resumed,
        HookEvent::ScreensOff,
        HookEvent::ScreensOn,
        HookEvent::BeforeSleep,
        HookEvent::AfterSleep,
        HookEvent::BeforeLogout,
        HookEvent::BeforeReboot,
        HookEvent::BeforeShutdown,
        HookEvent::PluggedIn,
        HookEvent::Unplugged,
        HookEvent::BatteryLevel,
        HookEvent::BatteryLow,
        HookEvent::BatteryCritical,
        HookEvent::BatteryFull,
        HookEvent::PowerProfileChanged,
        HookEvent::WifiOn,
        HookEvent::WifiOff,
        HookEvent::BluetoothOn,
        HookEvent::BluetoothOff,
    ];

    /// The kebab-case name (`wallpaper-changed`), as in `config.toml`.
    pub fn id(self) -> &'static str {
        match self {
            HookEvent::Started => "started",
            HookEvent::WallpaperChanged => "wallpaper-changed",
            HookEvent::ColorsChanged => "colors-changed",
            HookEvent::ThemeModeChanged => "theme-mode-changed",
            HookEvent::TemplatesRendered => "templates-rendered",
            HookEvent::Locked => "locked",
            HookEvent::Unlocked => "unlocked",
            HookEvent::Idle => "idle",
            HookEvent::Resumed => "resumed",
            HookEvent::ScreensOff => "screens-off",
            HookEvent::ScreensOn => "screens-on",
            HookEvent::BeforeSleep => "before-sleep",
            HookEvent::AfterSleep => "after-sleep",
            HookEvent::BeforeLogout => "before-logout",
            HookEvent::BeforeReboot => "before-reboot",
            HookEvent::BeforeShutdown => "before-shutdown",
            HookEvent::PluggedIn => "plugged-in",
            HookEvent::Unplugged => "unplugged",
            HookEvent::BatteryLevel => "battery-level",
            HookEvent::BatteryLow => "battery-low",
            HookEvent::BatteryCritical => "battery-critical",
            HookEvent::BatteryFull => "battery-full",
            HookEvent::PowerProfileChanged => "power-profile-changed",
            HookEvent::WifiOn => "wifi-on",
            HookEvent::WifiOff => "wifi-off",
            HookEvent::BluetoothOn => "bluetooth-on",
            HookEvent::BluetoothOff => "bluetooth-off",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|e| e.id() == id)
    }

    /// Whether the shell waits for the hook before carrying on (before
    /// sleep / logout / reboot / shutdown), up to its timeout.
    pub fn blocks(self) -> bool {
        matches!(self, HookEvent::BeforeSleep | HookEvent::BeforeLogout | HookEvent::BeforeReboot | HookEvent::BeforeShutdown)
    }
}

/// Run `command` when `event` happens.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct HookRule {
    pub event: HookEvent,
    /// A program with arguments (run directly, no shell), `mandara:<command>`
    /// (a `mandara msg` command) or `skill:<name>` (a launcher skill). Write
    /// `sh -c '…'` yourself for pipes and redirections.
    pub command: String,
    pub enabled: bool,
    /// Kill the hook after this long; 0 = `hooks.timeout_s`.
    pub timeout_s: u32,
}

impl Default for HookRule {
    fn default() -> Self {
        Self { event: HookEvent::Started, command: String::new(), enabled: true, timeout_s: 0 }
    }
}

/// Event hooks.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct HooksConfig {
    pub enabled: bool,
    /// Default timeout, seconds.
    pub timeout_s: u32,
    pub rules: Vec<HookRule>,
    /// Per-app-template command run after its theme file is written
    /// (keys: `ghostty`, `gtk4`, `qt6`, `zed`, `fuzzel`, `btop`, `kvantum`).
    pub template_post: BTreeMap<String, String>,
}

impl Default for HooksConfig {
    fn default() -> Self {
        Self { enabled: true, timeout_s: defaults::HOOK_TIMEOUT_S, rules: Vec::new(), template_post: BTreeMap::new() }
    }
}

impl HooksConfig {
    /// Enabled rules for `event`.
    pub fn for_event(&self, event: HookEvent) -> impl Iterator<Item = &HookRule> {
        self.rules.iter().filter(move |r| self.enabled && r.enabled && r.event == event && !r.command.trim().is_empty())
    }

    /// The effective timeout of `rule`, seconds.
    pub fn timeout_of(&self, rule: &HookRule) -> u32 {
        if rule.timeout_s > 0 { rule.timeout_s } else { self.timeout_s.max(1) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_round_trip() {
        for e in HookEvent::ALL {
            assert_eq!(HookEvent::from_id(e.id()), Some(e));
            let toml = toml::to_string(&HookRule { event: e, command: "true".into(), ..Default::default() }).unwrap();
            assert!(toml.contains(e.id()), "{toml}");
        }
        for k in SessionEntryKind::ALL {
            assert_eq!(SessionEntryKind::from_id(k.id()), Some(k));
        }
        for m in SuspendMode::ALL {
            assert_eq!(SuspendMode::from_id(m.id()), Some(m));
        }
    }

    #[test]
    fn defaults_are_conservative_and_load_partially() {
        let idle = IdleConfig::default();
        assert!(!idle.enabled, "idle management is opt-in");
        assert!(idle.lock_before_sleep);
        assert_eq!(idle.timeouts(true).suspend_s, defaults::BATTERY_SUSPEND_S);
        assert_eq!(idle.timeouts(false).suspend_s, 0);
        let parsed: IdleConfig = toml::from_str("enabled = true\n[battery]\nlock_s = 60\n").unwrap();
        assert!(parsed.enabled);
        assert_eq!(parsed.battery.lock_s, 60);
        assert_eq!(parsed.battery.screen_off_s, defaults::BATTERY_SCREEN_OFF_S, "missing battery keys take the battery defaults");
        assert_eq!(parsed.battery.suspend_s, defaults::BATTERY_SUSPEND_S, "not the mains 0 (never)");
        let ac: IdleConfig = toml::from_str("[ac]\nlock_s = 60\n").unwrap();
        assert_eq!(ac.ac.screen_off_s, defaults::AC_SCREEN_OFF_S);
        assert_eq!(ac.battery, IdleTimeouts::battery(), "an absent table is the battery default");
        let round: IdleConfig = toml::from_str(&toml::to_string(&parsed).unwrap()).unwrap();
        assert_eq!(round, parsed);
        let menu = SessionMenuConfig::default();
        let shown: Vec<_> = menu.visible().map(|e| e.kind).collect();
        assert_eq!(shown, [SessionEntryKind::Lock, SessionEntryKind::Logout, SessionEntryKind::Suspend, SessionEntryKind::Reboot, SessionEntryKind::Shutdown]);
    }

    #[test]
    fn session_menu_restores_missing_builtins_disabled() {
        let menu: SessionMenuConfig = toml::from_str("[[entries]]\nkind = \"shutdown\"\nenabled = true\n[[entries]]\nkind = \"custom\"\nenabled = true\ncommand = \"true\"\n").unwrap();
        assert_eq!(menu.entries[0].kind, SessionEntryKind::Shutdown, "the user's order is kept");
        assert_eq!(menu.entries[1].kind, SessionEntryKind::Custom);
        for kind in SessionEntryKind::ALL.into_iter().filter(|k| !matches!(k, SessionEntryKind::BootEntry | SessionEntryKind::Custom)) {
            let found: Vec<_> = menu.entries.iter().filter(|e| e.kind == kind).collect();
            assert_eq!(found.len(), 1, "{kind:?}");
            assert!(kind == SessionEntryKind::Shutdown || !found[0].enabled, "{kind:?} comes back disabled");
        }
        let shown: Vec<_> = menu.visible().map(|e| e.kind).collect();
        assert_eq!(shown, [SessionEntryKind::Shutdown, SessionEntryKind::Custom], "restored entries stay hidden");
        let mut defaults = SessionMenuConfig::default().entries;
        let before = defaults.clone();
        SessionMenuConfig::restore_builtins(&mut defaults);
        assert_eq!(defaults, before, "the default list is already complete");
    }

    #[test]
    fn peripheral_overrides_and_hook_filters() {
        let mut power = PowerConfig::default();
        power.device_thresholds.insert("MX Master 3".into(), 30);
        assert_eq!(power.peripheral_threshold("MX Master 3", ""), 30);
        assert_eq!(power.peripheral_threshold("Other", "x"), defaults::PERIPHERAL_LOW_PERCENT);
        let mut hooks = HooksConfig::default();
        hooks.rules.push(HookRule { event: HookEvent::Locked, command: "true".into(), ..Default::default() });
        hooks.rules.push(HookRule { event: HookEvent::Locked, command: "  ".into(), ..Default::default() });
        hooks.rules.push(HookRule { event: HookEvent::Locked, command: "x".into(), enabled: false, ..Default::default() });
        assert_eq!(hooks.for_event(HookEvent::Locked).count(), 1);
        assert_eq!(hooks.timeout_of(&hooks.rules[0]), defaults::HOOK_TIMEOUT_S);
        hooks.enabled = false;
        assert_eq!(hooks.for_event(HookEvent::Locked).count(), 0);
        assert!(HookEvent::BeforeShutdown.blocks() && !HookEvent::Locked.blocks());
    }
}
