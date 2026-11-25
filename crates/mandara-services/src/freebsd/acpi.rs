//! Batteries without UPower: FreeBSD's ACPI battery sysctls
//! (`hw.acpi.battery.life`, `.time`, `.state`, `.units`, `hw.acpi.acline`)
//! and each unit's details from `acpiconf -i N` (capacities, model,
//! cycles, rate).

use crate::power::{ChargeState, DeviceKind, PowerDevice};

/// `ACPI_BATT_STAT_*` bits of `hw.acpi.battery.state`.
const STAT_DISCHARGING: i32 = 0x1;
const STAT_CHARGING: i32 = 0x2;
const STAT_CRITICAL: i32 = 0x4;
/// All three bits: no battery in the slot.
const STAT_NOT_PRESENT: i32 = 0x7;
/// Seconds per minute (`hw.acpi.battery.time` is in minutes).
const MINUTE: i64 = 60;
/// milli-units per unit (mWh → Wh, mV → V).
const MILLI: f64 = 1000.0;

/// `acpiconf -i N` details of one battery unit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnitInfo {
    /// mWh, or mAh when [`UnitInfo::amps`].
    pub design: Option<f64>,
    pub last_full: Option<f64>,
    pub amps: bool,
    pub design_voltage_mv: Option<f64>,
    pub cycles: Option<u32>,
    pub model: String,
    pub serial: String,
    /// `LION`, `LiP`…
    pub kind: String,
    pub oem: String,
    pub state: String,
    pub remaining_pct: Option<f64>,
    /// mW (from mA × mV when the unit reports current).
    pub rate_mw: Option<f64>,
    pub voltage_mv: Option<f64>,
}

/// The number before a unit in an `acpiconf` value (`4400 mWh`).
fn amount(v: &str) -> Option<f64> {
    v.split_whitespace().next()?.trim_end_matches('%').parse().ok()
}

/// Parses `acpiconf -i N` output.
pub fn parse_acpiconf(text: &str) -> UnitInfo {
    let mut out = UnitInfo::default();
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else { continue };
        let value = value.trim();
        match key.trim() {
            "Design capacity" => {
                out.design = amount(value);
                out.amps = value.ends_with("mAh");
            }
            "Last full capacity" => out.last_full = amount(value),
            "Design voltage" => out.design_voltage_mv = amount(value),
            "Cycle Count" => out.cycles = value.parse().ok(),
            "Model number" => out.model = value.to_string(),
            "Serial number" => out.serial = value.to_string(),
            "Type" => out.kind = value.to_string(),
            "OEM info" => out.oem = value.to_string(),
            "State" => out.state = value.to_string(),
            "Remaining capacity" => out.remaining_pct = amount(value),
            // `12345 mW`, or `1200 mA (13320 mW)` for current-reporting units.
            "Present rate" => out.rate_mw = value.split_once('(').map_or_else(|| amount(value).filter(|_| value.ends_with("mW")), |(_, mw)| amount(mw)),
            "Present voltage" => out.voltage_mv = amount(value),
            _ => {}
        }
    }
    out
}

/// The machine's battery from the ACPI sysctls (all units together), with
/// unit 0's details when `acpiconf` could read them. `None` when no battery
/// is present.
pub fn battery(life: i32, time_min: i32, state: i32, acline: Option<i32>, info: Option<&UnitInfo>) -> Option<PowerDevice> {
    if state & STAT_NOT_PRESENT == STAT_NOT_PRESENT || life < 0 {
        return None;
    }
    let on_ac = acline == Some(1);
    let charge = if state & STAT_CHARGING != 0 {
        ChargeState::Charging
    } else if state & STAT_DISCHARGING != 0 {
        ChargeState::Discharging
    } else if state & STAT_CRITICAL != 0 {
        ChargeState::Empty
    } else if on_ac && life >= 100 {
        ChargeState::FullyCharged
    } else if on_ac {
        ChargeState::PendingCharge
    } else {
        ChargeState::Unknown
    };
    let mut d = PowerDevice {
        path: "acpi:battery".into(),
        native_path: "battery0".into(),
        kind: DeviceKind::Battery,
        power_supply: true,
        present: true,
        percentage: f64::from(life.min(100)),
        state: charge,
        time_to_empty: if charge == ChargeState::Discharging && time_min > 0 { i64::from(time_min) * MINUTE } else { 0 },
        ..PowerDevice::default()
    };
    if let Some(i) = info {
        // Capacities in mAh become Wh through the design voltage.
        let to_wh = |v: f64| if i.amps { i.design_voltage_mv.map(|mv| v * mv / MILLI / MILLI) } else { Some(v / MILLI) };
        d.energy_full_design = i.design.and_then(to_wh).unwrap_or(0.0);
        d.energy_full = i.last_full.and_then(to_wh).unwrap_or(0.0);
        d.energy = d.energy_full * d.percentage / 100.0;
        d.energy_rate = i.rate_mw.map(|mw| mw / MILLI).unwrap_or(0.0);
        d.voltage = i.voltage_mv.map(|mv| mv / MILLI).unwrap_or(0.0);
        d.charge_cycles = i.cycles;
        d.model = i.model.clone();
        d.vendor = i.oem.clone();
        d.serial = i.serial.clone();
        d.technology = i.kind.clone();
        if charge == ChargeState::Charging && d.energy_rate > 0.0 && d.energy_full > d.energy {
            d.time_to_full = ((d.energy_full - d.energy) / d.energy_rate * 3600.0) as i64;
        }
    }
    Some(d)
}

#[cfg(target_os = "freebsd")]
pub use sys::*;

#[cfg(target_os = "freebsd")]
mod sys {
    use super::*;
    use crate::freebsd::sysctl;

    const ACPICONF: &str = "/usr/sbin/acpiconf";

    /// Whether ACPI reports a power source at all (`hw.acpi.acline`).
    pub fn acline() -> Option<i32> {
        sysctl::int(c"hw.acpi.acline")
    }

    /// Charge percent and state from the sysctls alone (no process): what
    /// the bar, lock screen and desktop widgets draw.
    pub fn level() -> Option<(u8, ChargeState)> {
        let life = sysctl::int(c"hw.acpi.battery.life")?;
        let state = sysctl::int(c"hw.acpi.battery.state").unwrap_or(STAT_NOT_PRESENT);
        battery(life, -1, state, acline(), None).map(|b| (b.percentage as u8, b.state))
    }

    /// The battery, or `None` on machines without one (or without ACPI).
    pub fn read_battery() -> Option<PowerDevice> {
        let life = sysctl::int(c"hw.acpi.battery.life")?;
        let state = sysctl::int(c"hw.acpi.battery.state").unwrap_or(STAT_NOT_PRESENT);
        let time = sysctl::int(c"hw.acpi.battery.time").unwrap_or(-1);
        let info = std::process::Command::new(ACPICONF)
            .args(["-i", "0"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| parse_acpiconf(&String::from_utf8_lossy(&o.stdout)));
        battery(life, time, state, acline(), info.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `acpiconf -i 0` on a ThinkPad (power units).
    const THINKPAD: &str = "Design capacity:\t57000 mWh\nLast full capacity:\t48120 mWh\nTechnology:\t\tsecondary (rechargeable)\nDesign voltage:\t\t11400 mV\nCapacity (warn):\t2406 mWh\nCapacity (low):\t\t200 mWh\nCycle Count:\t\t312\nMeasurement Accuracy:\t95%\nMax Average Interval:\t60000 ms\nMin Average Interval:\t1000 ms\nLow/warn granularity:\t1 mWh\nWarn/full granularity:\t1 mWh\nModel number:\t\t01AV431\nSerial number:\t\t 1234\nType:\t\t\tLiP\nOEM info:\t\tSMP\nState:\t\t\tdischarging\nRemaining capacity:\t87%\nRemaining time:\t\t3:13\nPresent rate:\t\t12345 mW\nPresent voltage:\t12010 mV\n";
    /// A unit reporting current (mAh / mA).
    const AMPS: &str = "Design capacity:\t4000 mAh\nLast full capacity:\t3600 mAh\nDesign voltage:\t\t10000 mV\nState:\t\t\tcharging\nRemaining capacity:\t50%\nRemaining time:\t\tunknown\nPresent rate:\t\t1500 mA (15000 mW)\nPresent voltage:\t12000 mV\n";

    #[test]
    fn parses_acpiconf_in_both_units() {
        let i = parse_acpiconf(THINKPAD);
        assert_eq!((i.design, i.last_full, i.amps), (Some(57_000.0), Some(48_120.0), false));
        assert_eq!((i.cycles, i.model.as_str(), i.kind.as_str(), i.oem.as_str()), (Some(312), "01AV431", "LiP", "SMP"));
        assert_eq!((i.rate_mw, i.voltage_mv, i.remaining_pct), (Some(12_345.0), Some(12_010.0), Some(87.0)));
        let a = parse_acpiconf(AMPS);
        assert!(a.amps);
        assert_eq!(a.rate_mw, Some(15_000.0));
        assert_eq!(parse_acpiconf("Present rate:\t\tunknown\n").rate_mw, None);
    }

    #[test]
    fn battery_from_sysctls() {
        let i = parse_acpiconf(THINKPAD);
        let b = battery(87, 193, STAT_DISCHARGING, Some(0), Some(&i)).unwrap();
        assert_eq!((b.percentage, b.state, b.time_to_empty), (87.0, ChargeState::Discharging, 193 * 60));
        assert!((b.energy_full_design - 57.0).abs() < 1e-9 && (b.energy_full - 48.12).abs() < 1e-9);
        assert!((b.health().unwrap() - 0.8442).abs() < 0.001);
        assert_eq!((b.charge_cycles, b.display_name()), (Some(312), "SMP 01AV431".to_string()));
        // Charging, from mAh capacities through the design voltage.
        let c = battery(50, -1, STAT_CHARGING, Some(1), Some(&parse_acpiconf(AMPS))).unwrap();
        assert!((c.energy_full - 36.0).abs() < 1e-9);
        assert_eq!(c.time_to_full, ((36.0 - 18.0) / 15.0 * 3600.0) as i64);
        assert_eq!(battery(100, -1, 0, Some(1), None).unwrap().state, ChargeState::FullyCharged);
        assert_eq!(battery(5, 10, STAT_CRITICAL | STAT_DISCHARGING, Some(0), None).unwrap().state, ChargeState::Discharging);
        assert!(battery(-1, -1, STAT_NOT_PRESENT, Some(1), None).is_none(), "an empty slot");
    }
}
