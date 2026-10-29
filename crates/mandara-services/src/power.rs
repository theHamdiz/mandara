//! Power profiles and battery health.
//!
//! * **Profiles** — power-profiles-daemon on the system bus, under either of
//!   its names (`net.hadess.PowerProfiles`, or the newer
//!   `org.freedesktop.UPower.PowerProfiles`): the active profile, the
//!   available ones with their drivers, active holds (which app pinned a
//!   profile and why) and the "performance degraded" reason. Switching sets
//!   `ActiveProfile` with interactive authorization, so polkit can prompt.
//! * **Batteries** — UPower devices: charge, state, energy rate (W), time
//!   estimates, health (`EnergyFull` / `EnergyFullDesign`), cycle count,
//!   temperature, technology, charge-limit support (UPower ≥ 1.90's
//!   `ChargeThreshold*` properties and `EnableChargeThreshold`), plus the
//!   kernel's `charge_control_*_threshold` files shown read-only when UPower
//!   can't drive them. Peripheral batteries (mice, keyboards, headsets…)
//!   come from the same list. `GetHistory` feeds the charge sparkline.
//!
//! The actor lives on its own thread, refreshes on `PropertiesChanged`
//! from either daemon (plus a slow safety poll) and exits once every
//! [`PowerService`] handle is dropped. Mutations are described as pure
//! [`CallPlan`]s and executed through the small [`Caller`] trait so tests
//! can record them without touching the real system.

use std::collections::HashMap;
use std::path::Path;
use std::time::Duration;

use futures::{FutureExt as _, StreamExt as _};
use mandara_i18n::{t, tf};
use serde::Serialize;
use zbus::proxy::MethodFlags;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, MatchRule, MessageStream};

use crate::actor::{Envelope, ServiceHandle, proxy, reply};

pub const UPOWER: &str = "org.freedesktop.UPower";
pub const UPOWER_PATH: &str = "/org/freedesktop/UPower";
pub const DEVICE_IFACE: &str = "org.freedesktop.UPower.Device";
const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";
/// Kernel power-supply class (charge-limit files).
const SYSFS_POWER_SUPPLY: &str = "/sys/class/power_supply";
/// Safety poll when no signal arrives (percentages of peripherals that
/// don't emit `PropertiesChanged`, a daemon that appeared late).
const POLL: Duration = Duration::from_secs(30);
/// Signals are coalesced for this long before a re-read.
const DEBOUNCE: Duration = Duration::from_millis(250);
/// Buffered signals per stream.
const SIGNAL_QUEUE: usize = 64;
/// Buffered state snapshots for the UI.
const UPDATE_QUEUE: usize = 4;
/// Commands buffered from the UI and CLI.
const COMMAND_QUEUE: usize = 32;

/// The two bus names power-profiles-daemon answers on (newest first).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
pub enum ProfilesApi {
    #[serde(rename = "upower")]
    UPower,
    #[serde(rename = "hadess")]
    Hadess,
}

impl ProfilesApi {
    pub const ALL: [ProfilesApi; 2] = [ProfilesApi::UPower, ProfilesApi::Hadess];

    pub fn destination(self) -> &'static str {
        match self {
            ProfilesApi::UPower => "org.freedesktop.UPower.PowerProfiles",
            ProfilesApi::Hadess => "net.hadess.PowerProfiles",
        }
    }

    pub fn path(self) -> &'static str {
        match self {
            ProfilesApi::UPower => "/org/freedesktop/UPower/PowerProfiles",
            ProfilesApi::Hadess => "/net/hadess/PowerProfiles",
        }
    }

    /// The interface name equals the bus name for both generations.
    pub fn interface(self) -> &'static str {
        self.destination()
    }
}

/// One profile power-profiles-daemon offers.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Profile {
    /// `performance`, `balanced` or `power-saver`.
    pub name: String,
    pub driver: String,
    pub cpu_driver: Option<String>,
    pub platform_driver: Option<String>,
}

/// An application holding a profile (`powerprofilesctl launch`, games…).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Hold {
    pub application_id: String,
    pub profile: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Profiles {
    pub api: Option<ProfilesApi>,
    pub active: String,
    pub available: Vec<Profile>,
    pub holds: Vec<Hold>,
    /// Why performance is degraded (`lap-detected`, `high-operating-temperature`), empty if not.
    pub degraded: String,
}

impl Profiles {
    pub fn has(&self, name: &str) -> bool {
        self.available.iter().any(|p| p.name == name)
    }

    /// The profile after `active` in the daemon's order (wrapping).
    pub fn next(&self) -> Option<&str> {
        let i = self.available.iter().position(|p| p.name == self.active)?;
        self.available.get((i + 1) % self.available.len()).map(|p| p.name.as_str())
    }
}

/// UPower `Device.Type`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum DeviceKind {
    LinePower,
    Battery,
    Ups,
    Monitor,
    Mouse,
    Keyboard,
    Pda,
    Phone,
    MediaPlayer,
    Tablet,
    Computer,
    GamingInput,
    Pen,
    Touchpad,
    Modem,
    Network,
    Headset,
    Speakers,
    Headphones,
    Video,
    OtherAudio,
    RemoteControl,
    Printer,
    Scanner,
    Camera,
    Wearable,
    Toy,
    BluetoothGeneric,
    #[default]
    Unknown,
}

impl DeviceKind {
    pub fn from_upower(v: u32) -> Self {
        use DeviceKind::*;
        match v {
            1 => LinePower,
            2 => Battery,
            3 => Ups,
            4 => Monitor,
            5 => Mouse,
            6 => Keyboard,
            7 => Pda,
            8 => Phone,
            9 => MediaPlayer,
            10 => Tablet,
            11 => Computer,
            12 => GamingInput,
            13 => Pen,
            14 => Touchpad,
            15 => Modem,
            16 => Network,
            17 => Headset,
            18 => Speakers,
            19 => Headphones,
            20 => Video,
            21 => OtherAudio,
            22 => RemoteControl,
            23 => Printer,
            24 => Scanner,
            25 => Camera,
            26 => Wearable,
            27 => Toy,
            28 => BluetoothGeneric,
            _ => Unknown,
        }
    }
}

/// UPower `Device.State`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChargeState {
    #[default]
    Unknown,
    Charging,
    Discharging,
    Empty,
    FullyCharged,
    PendingCharge,
    PendingDischarge,
}

impl ChargeState {
    pub fn from_upower(v: u32) -> Self {
        match v {
            1 => ChargeState::Charging,
            2 => ChargeState::Discharging,
            3 => ChargeState::Empty,
            4 => ChargeState::FullyCharged,
            5 => ChargeState::PendingCharge,
            6 => ChargeState::PendingDischarge,
            _ => ChargeState::Unknown,
        }
    }

    pub fn is_charging(self) -> bool {
        matches!(self, ChargeState::Charging | ChargeState::PendingCharge)
    }
}

/// Charge limiting for a battery.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Threshold {
    /// UPower can switch the limit (`ChargeThresholdSupported`).
    pub supported: bool,
    pub enabled: bool,
    /// Start / end percentages (UPower's view, else the sysfs files).
    pub start: Option<u32>,
    pub end: Option<u32>,
}

/// A UPower device with a battery (or the line power supply).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PowerDevice {
    pub path: String,
    pub native_path: String,
    pub kind: DeviceKind,
    /// Powers the computer (a laptop battery, a UPS).
    pub power_supply: bool,
    pub present: bool,
    pub model: String,
    pub vendor: String,
    pub serial: String,
    pub percentage: f64,
    pub state: ChargeState,
    /// Wh.
    pub energy: f64,
    pub energy_full: f64,
    pub energy_full_design: f64,
    /// W (positive; direction from `state`).
    pub energy_rate: f64,
    pub voltage: f64,
    /// Seconds; 0 = unknown.
    pub time_to_empty: i64,
    pub time_to_full: i64,
    /// Health in percent as UPower computes it (0 = unknown).
    pub capacity: f64,
    pub charge_cycles: Option<u32>,
    pub technology: String,
    /// °C.
    pub temperature: Option<f64>,
    pub threshold: Option<Threshold>,
}

impl PowerDevice {
    /// Health as a fraction: full capacity over design capacity.
    pub fn health(&self) -> Option<f64> {
        if self.energy_full_design > 0.0 && self.energy_full > 0.0 {
            Some((self.energy_full / self.energy_full_design).clamp(0.0, 1.5))
        } else if self.capacity > 0.0 {
            Some(self.capacity / 100.0)
        } else {
            None
        }
    }

    /// Seconds until empty (discharging) or full (charging), if known.
    pub fn time_left(&self) -> Option<i64> {
        let secs = if self.state.is_charging() { self.time_to_full } else { self.time_to_empty };
        (secs > 0).then_some(secs)
    }

    pub fn display_name(&self) -> String {
        let name = format!("{} {}", self.vendor.trim(), self.model.trim()).trim().to_string();
        if name.is_empty() { self.native_path.clone() } else { name }
    }
}

/// One `GetHistory` sample.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct HistoryPoint {
    /// Unix seconds.
    pub time: u32,
    pub value: f64,
    pub state: ChargeState,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct PowerState {
    /// The first read finished (either daemon may still be missing).
    pub ready: bool,
    pub profiles: Option<Profiles>,
    pub profiles_error: Option<String>,
    pub upower: bool,
    pub upower_error: Option<String>,
    pub on_battery: bool,
    /// Every battery-carrying device except line power, system ones first.
    pub devices: Vec<PowerDevice>,
}

impl PowerState {
    /// Batteries that power this computer.
    pub fn batteries(&self) -> impl Iterator<Item = &PowerDevice> {
        self.devices.iter().filter(|d| d.power_supply && matches!(d.kind, DeviceKind::Battery | DeviceKind::Ups) && d.present)
    }

    /// Mice, keyboards, headsets… that report a battery.
    pub fn peripherals(&self) -> impl Iterator<Item = &PowerDevice> {
        self.devices.iter().filter(|d| !d.power_supply && d.kind != DeviceKind::LinePower)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum PowerCommand {
    Refresh,
    SetProfile(String),
    /// Turns UPower's charge limit on or off for a battery (object path).
    SetChargeLimit { device: String, enabled: bool },
}

pub type PowerService = ServiceHandle<PowerState, PowerCommand>;

// ── Mutations as data ────────────────────────────────────────────────────

/// Argument of a planned call.
#[derive(Clone, Debug, PartialEq)]
pub enum CallArgs {
    /// `Properties.Set(interface, property, <string>)`.
    SetString { interface: &'static str, property: &'static str, value: String },
    Bool(bool),
}

/// One D-Bus method call, fully described.
#[derive(Clone, Debug, PartialEq)]
pub struct CallPlan {
    pub destination: &'static str,
    pub path: String,
    pub interface: &'static str,
    pub method: &'static str,
    pub args: CallArgs,
    /// Sent with `ALLOW_INTERACTIVE_AUTHORIZATION` so polkit may prompt.
    pub interactive: bool,
}

/// Plans switching to `profile`, refusing names the daemon doesn't offer.
pub fn plan_set_profile(profiles: &Profiles, profile: &str) -> Result<CallPlan, String> {
    let api = profiles.api.ok_or_else(|| t("power-profiles-daemon is not running").to_string())?;
    if !profiles.has(profile) {
        let names: Vec<&str> = profiles.available.iter().map(|p| p.name.as_str()).collect();
        return Err(tf("Unknown profile “{profile}” (available: {names})", &[("profile", &profile), ("names", &names.join(", "))]));
    }
    Ok(CallPlan {
        destination: api.destination(),
        path: api.path().into(),
        interface: PROPERTIES_IFACE,
        method: "Set",
        args: CallArgs::SetString { interface: api.interface(), property: "ActiveProfile", value: profile.to_string() },
        interactive: true,
    })
}

/// Plans enabling / disabling UPower's charge limit on `device`.
pub fn plan_charge_limit(device: &PowerDevice, enabled: bool) -> Result<CallPlan, String> {
    if !device.threshold.as_ref().is_some_and(|t| t.supported) {
        return Err(t("This battery's charge limit can't be changed from here").into());
    }
    Ok(CallPlan { destination: UPOWER, path: device.path.clone(), interface: DEVICE_IFACE, method: "EnableChargeThreshold", args: CallArgs::Bool(enabled), interactive: true })
}

/// Executes [`CallPlan`]s (the real bus, or a recorder in tests).
#[allow(async_fn_in_trait)]
pub trait Caller {
    async fn call(&mut self, plan: &CallPlan) -> Result<(), String>;
}

struct BusCaller<'a> {
    conn: &'a Connection,
}

impl Caller for BusCaller<'_> {
    async fn call(&mut self, plan: &CallPlan) -> Result<(), String> {
        let p = proxy(self.conn, plan.destination, &plan.path, plan.interface).await.map_err(|e| e.to_string())?;
        let flags = if plan.interactive { MethodFlags::AllowInteractiveAuth.into() } else { Default::default() };
        let result = match &plan.args {
            CallArgs::SetString { interface, property, value } => p.call_with_flags::<_, _, ()>(plan.method, flags, &(*interface, *property, Value::from(value.as_str()))).await,
            CallArgs::Bool(b) => p.call_with_flags::<_, _, ()>(plan.method, flags, &(*b,)).await,
        };
        result.map(|_| ()).map_err(|e| match e {
            zbus::Error::MethodError(name, msg, _) => friendly_error(name.as_str(), msg.as_deref().unwrap_or_default()),
            other => other.to_string(),
        })
    }
}

/// Polkit / daemon errors as short sentences.
pub fn friendly_error(name: &str, message: &str) -> String {
    if name.ends_with("AccessDenied") || name.ends_with("NotAuthorized") || name.contains("PolicyKit") || message.contains("Not authorized") {
        t("Not authorized — the authentication was cancelled or refused").into()
    } else if name.ends_with("UnknownMethod") {
        t("Your UPower is too old to change the charge limit").into()
    } else if message.is_empty() {
        name.to_string()
    } else {
        message.to_string()
    }
}

// ── Reading ──────────────────────────────────────────────────────────────

fn string(map: &HashMap<String, OwnedValue>, key: &str) -> String {
    map.get(key).and_then(|v| String::try_from(v.try_clone().ok()?).ok()).unwrap_or_default()
}

fn float(map: &HashMap<String, OwnedValue>, key: &str) -> f64 {
    map.get(key).and_then(|v| f64::try_from(v).ok()).unwrap_or(0.0)
}

fn uint(map: &HashMap<String, OwnedValue>, key: &str) -> Option<u32> {
    map.get(key).and_then(|v| u32::try_from(v).ok())
}

fn int(map: &HashMap<String, OwnedValue>, key: &str) -> Option<i64> {
    let v = map.get(key)?;
    i64::try_from(v).ok().or_else(|| i32::try_from(v).ok().map(i64::from))
}

fn boolean(map: &HashMap<String, OwnedValue>, key: &str) -> bool {
    map.get(key).and_then(|v| bool::try_from(v).ok()).unwrap_or(false)
}

/// A `Profiles` entry (`a{sv}`).
pub fn parse_profile(map: &HashMap<String, OwnedValue>) -> Profile {
    let opt = |k: &str| Some(string(map, k)).filter(|s| !s.is_empty() && s != "placeholder");
    Profile { name: string(map, "Profile"), driver: string(map, "Driver"), cpu_driver: opt("CpuDriver"), platform_driver: opt("PlatformDriver") }
}

/// An `ActiveProfileHolds` entry (`a{sv}`).
pub fn parse_hold(map: &HashMap<String, OwnedValue>) -> Hold {
    Hold { application_id: string(map, "ApplicationId"), profile: string(map, "Profile"), reason: string(map, "Reason") }
}

/// A device's `GetAll` result; `sysfs` reads charge-limit files for
/// `NativePath` (injected so tests stay off the real filesystem).
pub fn parse_device(path: &str, map: &HashMap<String, OwnedValue>, sysfs: impl Fn(&str, &str) -> Option<u32>) -> PowerDevice {
    let native_path = string(map, "NativePath");
    let temperature = Some(float(map, "Temperature")).filter(|t| *t > 0.0);
    // UPower reports -1 (or 0 on some drivers) when the cycle count is unknown.
    let charge_cycles = int(map, "ChargeCycles").filter(|c| *c > 0).and_then(|c| u32::try_from(c).ok());
    let kind = DeviceKind::from_upower(uint(map, "Type").unwrap_or(0));
    let supported = boolean(map, "ChargeThresholdSupported");
    let threshold = if supported {
        Some(Threshold {
            supported,
            enabled: boolean(map, "ChargeThresholdEnabled"),
            start: uint(map, "ChargeStartThreshold").filter(|v| *v > 0 && *v < 100),
            end: uint(map, "ChargeEndThreshold").filter(|v| *v > 0 && *v <= 100),
        })
    } else if kind == DeviceKind::Battery && !native_path.is_empty() {
        let end = sysfs(&native_path, "charge_control_end_threshold");
        end.map(|end| Threshold { supported: false, enabled: end < 100, start: sysfs(&native_path, "charge_control_start_threshold"), end: Some(end) })
    } else {
        None
    };
    PowerDevice {
        path: path.to_string(),
        native_path,
        kind,
        power_supply: boolean(map, "PowerSupply"),
        present: map.get("IsPresent").and_then(|v| bool::try_from(v).ok()).unwrap_or(true),
        model: string(map, "Model"),
        vendor: string(map, "Vendor"),
        serial: string(map, "Serial"),
        percentage: float(map, "Percentage"),
        state: ChargeState::from_upower(uint(map, "State").unwrap_or(0)),
        energy: float(map, "Energy"),
        energy_full: float(map, "EnergyFull"),
        energy_full_design: float(map, "EnergyFullDesign"),
        energy_rate: float(map, "EnergyRate").abs(),
        voltage: float(map, "Voltage"),
        time_to_empty: int(map, "TimeToEmpty").unwrap_or(0),
        time_to_full: int(map, "TimeToFull").unwrap_or(0),
        capacity: float(map, "Capacity"),
        charge_cycles,
        technology: technology(uint(map, "Technology").unwrap_or(0)).to_string(),
        temperature,
        threshold,
    }
}

/// UPower `Device.Technology` as a short chemistry name.
pub fn technology(v: u32) -> &'static str {
    match v {
        1 => "Li-ion",
        2 => "Li-polymer",
        3 => "LiFePO₄",
        4 => "Lead-acid",
        5 => "NiCd",
        6 => "NiMH",
        _ => "",
    }
}

/// Reads `/sys/class/power_supply/<supply>/<file>` as an integer.
pub fn read_sysfs(supply: &str, file: &str) -> Option<u32> {
    // NativePath is a bare supply name for batteries; reject anything else.
    if supply.is_empty() || supply.contains('/') || supply.contains("..") {
        return None;
    }
    std::fs::read_to_string(Path::new(SYSFS_POWER_SUPPLY).join(supply).join(file)).ok()?.trim().parse().ok()
}

async fn read_profiles(conn: &Connection) -> Result<Profiles, String> {
    let mut last_err = String::new();
    for api in ProfilesApi::ALL {
        let p = match proxy(conn, api.destination(), api.path(), api.interface()).await {
            Ok(p) => p,
            Err(e) => {
                last_err = e.to_string();
                continue;
            }
        };
        let active: String = match p.get_property("ActiveProfile").await {
            Ok(a) => a,
            Err(e) => {
                last_err = e.to_string();
                continue;
            }
        };
        let available: Vec<HashMap<String, OwnedValue>> = p.get_property("Profiles").await.unwrap_or_default();
        let holds: Vec<HashMap<String, OwnedValue>> = p.get_property("ActiveProfileHolds").await.unwrap_or_default();
        let degraded: String = p.get_property("PerformanceDegraded").await.unwrap_or_default();
        return Ok(Profiles { api: Some(api), active, available: available.iter().map(parse_profile).collect(), holds: holds.iter().map(parse_hold).collect(), degraded });
    }
    Err(if last_err.is_empty() { t("power-profiles-daemon is not running").into() } else { last_err })
}

async fn read_devices(conn: &Connection) -> Result<(bool, Vec<PowerDevice>), String> {
    let root = proxy(conn, UPOWER, UPOWER_PATH, UPOWER).await.map_err(|e| e.to_string())?;
    let paths: Vec<OwnedObjectPath> = root.call("EnumerateDevices", &()).await.map_err(|e| e.to_string())?;
    let on_battery: bool = root.get_property("OnBattery").await.unwrap_or(false);
    let mut devices = Vec::new();
    for path in paths {
        let props = proxy(conn, UPOWER, path.as_str(), PROPERTIES_IFACE).await.map_err(|e| e.to_string())?;
        let Ok(map) = props.call::<_, _, HashMap<String, OwnedValue>>("GetAll", &(DEVICE_IFACE,)).await else { continue };
        let d = parse_device(path.as_str(), &map, read_sysfs);
        // Line power has no battery; keep the list to battery carriers.
        if d.kind != DeviceKind::LinePower {
            devices.push(d);
        }
    }
    sort_devices(&mut devices);
    Ok((on_battery, devices))
}

/// System batteries first, then peripherals by name.
pub fn sort_devices(devices: &mut [PowerDevice]) {
    devices.sort_by(|a, b| b.power_supply.cmp(&a.power_supply).then_with(|| a.display_name().to_lowercase().cmp(&b.display_name().to_lowercase())));
}

async fn read_state(conn: &Connection) -> PowerState {
    let (profiles, profiles_error) = match read_profiles(conn).await {
        Ok(p) => (Some(p), None),
        Err(e) => (None, Some(e)),
    };
    let (upower, upower_error, on_battery, devices) = match read_devices(conn).await {
        Ok((on_battery, devices)) => (true, None, on_battery, devices),
        Err(e) => (false, Some(e), false, Vec::new()),
    };
    PowerState { ready: true, profiles, profiles_error, upower, upower_error, on_battery, devices }
}

/// `GetHistory` of a device over the last `span` (blocking; run it on a
/// background thread). `kind` is `charge` or `rate`; oldest first.
pub fn history(device: &str, kind: &str, span: Duration, resolution: u32) -> Result<Vec<HistoryPoint>, String> {
    let (device, kind) = (device.to_string(), kind.to_string());
    crate::runtime::block_on(async move {
        let conn = crate::runtime::system().await.map_err(|e| e.to_string())?;
        let p = proxy(&conn, UPOWER, &device, DEVICE_IFACE).await.map_err(|e| e.to_string())?;
        let span_s = u32::try_from(span.as_secs()).unwrap_or(u32::MAX);
        let raw: Vec<(u32, f64, u32)> = p.call("GetHistory", &(kind.as_str(), span_s, resolution)).await.map_err(|e| e.to_string())?;
        Ok(normalize_history(raw))
    })
}

/// Sorts samples oldest first and drops the zero-time placeholders UPower
/// returns for empty histories.
pub fn normalize_history(raw: Vec<(u32, f64, u32)>) -> Vec<HistoryPoint> {
    let mut out: Vec<HistoryPoint> = raw.into_iter().filter(|(t, _, _)| *t > 0).map(|(time, value, s)| HistoryPoint { time, value, state: ChargeState::from_upower(s) }).collect();
    out.sort_by_key(|p| p.time);
    out
}

/// Averages `points` into `buckets` equal time slots ending at `now`
/// (`None` for slots without samples), values scaled to `0..=1` by `max`.
pub fn bucket_history(points: &[HistoryPoint], now: u32, span: Duration, buckets: usize, max: f64) -> Vec<Option<f32>> {
    let span_s = span.as_secs().max(1) as f64;
    let start = f64::from(now) - span_s;
    let mut sums = vec![(0.0_f64, 0_u32); buckets];
    for p in points {
        let x = (f64::from(p.time) - start) / span_s;
        if !(0.0..=1.0).contains(&x) || buckets == 0 {
            continue;
        }
        let i = ((x * buckets as f64) as usize).min(buckets - 1);
        sums[i].0 += p.value;
        sums[i].1 += 1;
    }
    sums.into_iter().map(|(s, n)| (n > 0).then(|| ((s / f64::from(n)) / max.max(f64::EPSILON)).clamp(0.0, 1.0) as f32)).collect()
}

/// A one-shot read of everything (blocking), for CLI commands.
pub fn snapshot() -> PowerState {
    crate::runtime::block_on(async {
        match crate::runtime::system().await {
            Ok(conn) => read_state(&conn).await,
            Err(e) => PowerState { ready: true, profiles_error: Some(e.to_string()), upower_error: Some(e.to_string()), ..Default::default() },
        }
    })
}

/// Runs one planned call on the system bus (blocking).
pub fn execute(plan: CallPlan) -> Result<(), String> {
    crate::runtime::block_on(async move {
        let conn = crate::runtime::system().await.map_err(|e| e.to_string())?;
        BusCaller { conn: &conn }.call(&plan).await
    })
}

// ── Actor ────────────────────────────────────────────────────────────────

/// Applies one command against `state` through `caller`.
pub async fn apply(caller: &mut impl Caller, state: &PowerState, command: &PowerCommand) -> Result<String, String> {
    match command {
        PowerCommand::Refresh => Ok(String::new()),
        PowerCommand::SetProfile(name) => {
            let profiles = state.profiles.as_ref().ok_or_else(|| t("power-profiles-daemon is not running").to_string())?;
            let plan = plan_set_profile(profiles, name)?;
            caller.call(&plan).await?;
            Ok(name.clone())
        }
        PowerCommand::SetChargeLimit { device, enabled } => {
            let d = state.devices.iter().find(|d| &d.path == device).ok_or_else(|| t("No such battery").to_string())?;
            caller.call(&plan_charge_limit(d, *enabled)?).await?;
            Ok(if *enabled { t("Charge limit on").into() } else { t("Charge limit off").into() })
        }
    }
}

async fn signal_stream(conn: &Connection, sender: &'static str) -> Option<futures::stream::Fuse<MessageStream>> {
    let rule = MatchRule::builder().msg_type(zbus::message::Type::Signal).sender(sender).ok()?.interface(PROPERTIES_IFACE).ok()?.member("PropertiesChanged").ok()?.build();
    MessageStream::for_match_rule(rule, conn, Some(SIGNAL_QUEUE)).await.ok().map(|s| s.fuse())
}

async fn run(tx: &async_channel::Sender<PowerState>, rx: &async_channel::Receiver<Envelope<PowerCommand>>) -> zbus::Result<()> {
    // Private connection: its signal streams must never stall other actors.
    let conn = crate::runtime::connect(zbus::connection::Builder::system()?).await?;
    // Device-added/removed come from UPower's own signals.
    let added = MatchRule::builder().msg_type(zbus::message::Type::Signal).sender(UPOWER)?.interface(UPOWER)?.build();
    let mut upower_devices = MessageStream::for_match_rule(added, &conn, Some(SIGNAL_QUEUE)).await?.fuse();
    let mut upower = signal_stream(&conn, UPOWER).await;
    let mut ppd = signal_stream(&conn, ProfilesApi::Hadess.destination()).await;
    let mut state = read_state(&conn).await;
    let _ = tx.try_send(state.clone());
    loop {
        let mut dirty = false;
        let mut command = None;
        {
            let upower_next = async {
                match upower.as_mut() {
                    Some(s) => s.next().await,
                    None => futures::future::pending().await,
                }
            };
            let ppd_next = async {
                match ppd.as_mut() {
                    Some(s) => s.next().await,
                    None => futures::future::pending().await,
                }
            };
            futures::select! {
                cmd = rx.recv().fuse() => match cmd {
                    Ok(c) => command = Some(c),
                    Err(_) => return Ok(()), // every handle dropped
                },
                _ = upower_next.fuse() => dirty = true,
                _ = ppd_next.fuse() => dirty = true,
                _ = upower_devices.next() => dirty = true,
                _ = futures::FutureExt::fuse(async_io::Timer::after(POLL)) => dirty = true,
            }
        }
        if let Some(Envelope { command, reply: r }) = command {
            let result = apply(&mut BusCaller { conn: &conn }, &state, &command).await;
            reply(&r, result);
            dirty = true;
        } else if dirty {
            // Coalesce bursts (a charging battery updates several properties).
            async_io::Timer::after(DEBOUNCE).await;
        }
        if dirty {
            let next = read_state(&conn).await;
            if next != state {
                state = next;
                let _ = tx.try_send(state.clone());
            }
        }
    }
}

/// Starts the power actor on the service runtime.
pub fn spawn_power_service() -> PowerService {
    let (tx, updates) = async_channel::bounded(UPDATE_QUEUE);
    let (commands, rx) = async_channel::bounded(COMMAND_QUEUE);
    crate::runtime::spawn("power", async move {
        if let Err(e) = run(&tx, &rx).await {
            tracing::warn!("power: {e}");
            let _ = tx.try_send(PowerState { ready: true, profiles_error: Some(e.to_string()), upower_error: Some(e.to_string()), ..Default::default() });
            while let Ok(Envelope { reply: r, .. }) = rx.recv().await {
                reply(&r, Err(t("The system bus is not available").into()));
            }
        }
    });
    ServiceHandle::new(updates, commands)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ov(v: Value<'_>) -> OwnedValue {
        v.try_to_owned().unwrap()
    }

    fn profiles() -> Profiles {
        let p = |n: &str| Profile { name: n.into(), driver: "amd_pstate".into(), ..Default::default() };
        Profiles { api: Some(ProfilesApi::Hadess), active: "balanced".into(), available: vec![p("power-saver"), p("balanced"), p("performance")], holds: vec![], degraded: String::new() }
    }

    fn battery_map() -> HashMap<String, OwnedValue> {
        let mut m = HashMap::new();
        m.insert("NativePath".into(), ov(Value::from("BAT0")));
        m.insert("Type".into(), ov(Value::from(2u32)));
        m.insert("PowerSupply".into(), ov(Value::from(true)));
        m.insert("Model".into(), ov(Value::from("5B10W51867")));
        m.insert("Vendor".into(), ov(Value::from("LGC")));
        m.insert("Percentage".into(), ov(Value::from(81.0f64)));
        m.insert("State".into(), ov(Value::from(2u32)));
        m.insert("EnergyFull".into(), ov(Value::from(45.0f64)));
        m.insert("EnergyFullDesign".into(), ov(Value::from(50.0f64)));
        m.insert("EnergyRate".into(), ov(Value::from(-7.5f64)));
        m.insert("TimeToEmpty".into(), ov(Value::from(5400i64)));
        m.insert("ChargeCycles".into(), ov(Value::from(212i32)));
        m.insert("Technology".into(), ov(Value::from(2u32)));
        m
    }

    #[derive(Default)]
    struct Recorder(Vec<CallPlan>);

    impl Caller for Recorder {
        async fn call(&mut self, plan: &CallPlan) -> Result<(), String> {
            self.0.push(plan.clone());
            Ok(())
        }
    }

    #[test]
    fn set_profile_plans_an_interactive_property_write() {
        let plan = plan_set_profile(&profiles(), "power-saver").unwrap();
        assert_eq!(plan.destination, "net.hadess.PowerProfiles");
        assert_eq!(plan.path, "/net/hadess/PowerProfiles");
        assert_eq!((plan.interface, plan.method), (PROPERTIES_IFACE, "Set"));
        assert_eq!(plan.args, CallArgs::SetString { interface: "net.hadess.PowerProfiles", property: "ActiveProfile", value: "power-saver".into() });
        assert!(plan.interactive);
        assert!(plan_set_profile(&profiles(), "turbo").is_err());
        assert!(plan_set_profile(&Profiles::default(), "balanced").is_err());
        let mut newer = profiles();
        newer.api = Some(ProfilesApi::UPower);
        assert_eq!(plan_set_profile(&newer, "balanced").unwrap().destination, "org.freedesktop.UPower.PowerProfiles");
    }

    #[test]
    fn next_profile_wraps() {
        let mut p = profiles();
        assert_eq!(p.next(), Some("performance"));
        p.active = "performance".into();
        assert_eq!(p.next(), Some("power-saver"));
        p.active = "gone".into();
        assert_eq!(p.next(), None);
    }

    #[test]
    fn parses_battery_health_and_estimates() {
        let d = parse_device("/org/freedesktop/UPower/devices/battery_BAT0", &battery_map(), |_, _| None);
        assert_eq!(d.kind, DeviceKind::Battery);
        assert!(d.power_supply && d.present);
        assert_eq!(d.state, ChargeState::Discharging);
        assert!((d.health().unwrap() - 0.9).abs() < 1e-9);
        assert_eq!(d.energy_rate, 7.5);
        assert_eq!(d.time_left(), Some(5400));
        assert_eq!(d.charge_cycles, Some(212));
        assert_eq!(d.technology, "Li-polymer");
        assert_eq!(d.display_name(), "LGC 5B10W51867");
        assert!(d.threshold.is_none());
    }

    #[test]
    fn unknown_cycles_and_sysfs_threshold() {
        let mut m = battery_map();
        m.insert("ChargeCycles".into(), ov(Value::from(-1i32)));
        let d = parse_device("/b", &m, |supply, file| (supply == "BAT0" && file == "charge_control_end_threshold").then_some(80));
        assert_eq!(d.charge_cycles, None);
        let t = d.threshold.unwrap();
        assert!(!t.supported && t.enabled);
        assert_eq!((t.start, t.end), (None, Some(80)));
        // UPower-managed limits win over sysfs.
        m.insert("ChargeThresholdSupported".into(), ov(Value::from(true)));
        m.insert("ChargeThresholdEnabled".into(), ov(Value::from(false)));
        m.insert("ChargeStartThreshold".into(), ov(Value::from(75u32)));
        m.insert("ChargeEndThreshold".into(), ov(Value::from(80u32)));
        let t = parse_device("/b", &m, |_, _| Some(100)).threshold.unwrap();
        assert!(t.supported && !t.enabled);
        assert_eq!((t.start, t.end), (Some(75), Some(80)));
    }

    #[test]
    fn profiles_parse_and_skip_placeholders() {
        let mut m = HashMap::new();
        m.insert("Profile".to_string(), ov(Value::from("balanced")));
        m.insert("Driver".to_string(), ov(Value::from("multiple")));
        m.insert("CpuDriver".to_string(), ov(Value::from("amd_pstate")));
        m.insert("PlatformDriver".to_string(), ov(Value::from("placeholder")));
        let p = parse_profile(&m);
        assert_eq!(p.name, "balanced");
        assert_eq!(p.cpu_driver.as_deref(), Some("amd_pstate"));
        assert_eq!(p.platform_driver, None);
        let mut h = HashMap::new();
        h.insert("ApplicationId".to_string(), ov(Value::from("org.gnome.Software")));
        h.insert("Profile".to_string(), ov(Value::from("power-saver")));
        h.insert("Reason".to_string(), ov(Value::from("Battery saver")));
        assert_eq!(parse_hold(&h), Hold { application_id: "org.gnome.Software".into(), profile: "power-saver".into(), reason: "Battery saver".into() });
    }

    #[test]
    fn apply_records_calls_without_touching_the_bus() {
        let mut state = PowerState { profiles: Some(profiles()), ..Default::default() };
        let mut d = parse_device("/bat", &battery_map(), |_, _| None);
        let mut rec = Recorder::default();
        let r = zbus::block_on(apply(&mut rec, &state, &PowerCommand::SetProfile("performance".into())));
        assert_eq!(r.as_deref(), Ok("performance"));
        assert_eq!(rec.0.len(), 1);
        // No threshold support → refused before any call.
        state.devices = vec![d.clone()];
        let r = zbus::block_on(apply(&mut rec, &state, &PowerCommand::SetChargeLimit { device: "/bat".into(), enabled: true }));
        assert!(r.is_err());
        assert_eq!(rec.0.len(), 1);
        d.threshold = Some(Threshold { supported: true, ..Default::default() });
        state.devices = vec![d];
        zbus::block_on(apply(&mut rec, &state, &PowerCommand::SetChargeLimit { device: "/bat".into(), enabled: true })).unwrap();
        let plan = rec.0.last().unwrap();
        assert_eq!((plan.destination, plan.path.as_str(), plan.method, plan.interactive), (UPOWER, "/bat", "EnableChargeThreshold", true));
        assert_eq!(plan.args, CallArgs::Bool(true));
    }

    #[test]
    fn history_is_sorted_and_bucketed() {
        let pts = normalize_history(vec![(1_000_300, 50.0, 2), (0, 0.0, 0), (1_000_000, 100.0, 4)]);
        assert_eq!(pts.len(), 2);
        assert_eq!(pts[0].time, 1_000_000);
        let b = bucket_history(&pts, 1_000_400, Duration::from_secs(400), 4, 100.0);
        assert_eq!(b, vec![Some(1.0), None, None, Some(0.5)]);
    }

    #[test]
    fn sysfs_rejects_paths() {
        assert_eq!(read_sysfs("../etc", "passwd"), None);
        assert_eq!(read_sysfs("a/b", "x"), None);
    }

    /// Read-only: prints the real power state (`--ignored`).
    #[test]
    #[ignore]
    fn live_snapshot() {
        let s = snapshot();
        println!("{}", serde_json::to_string_pretty(&s).unwrap());
        assert!(s.ready);
    }
}
