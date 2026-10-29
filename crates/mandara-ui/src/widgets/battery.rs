//! Battery status from `/sys/class/power_supply` (UPower integration arrives
//! with `mandara-services`; sysfs keeps the bar useful without D-Bus).

use std::fs;
use std::path::Path;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ChargeState {
    Charging,
    Discharging,
    Full,
    NotCharging,
    Unknown,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct BatteryInfo {
    pub percent: u8,
    pub state: ChargeState,
}

impl BatteryInfo {
    /// A glyph from the bundled icon-free set (pure Unicode so it renders with
    /// any font).
    pub fn glyph(&self) -> &'static str {
        match (self.state, self.percent) {
            (ChargeState::Charging, _) => "⚡",
            (_, 0..=15) => "▁",
            (_, 16..=40) => "▃",
            (_, 41..=70) => "▅",
            _ => "▇",
        }
    }
}

fn parse_state(s: &str) -> ChargeState {
    match s.trim() {
        "Charging" => ChargeState::Charging,
        "Discharging" => ChargeState::Discharging,
        "Full" => ChargeState::Full,
        "Not charging" => ChargeState::NotCharging,
        _ => ChargeState::Unknown,
    }
}

/// Reads the first battery under `root` (normally `/sys/class/power_supply`).
pub fn read_battery_from(root: &Path) -> Option<BatteryInfo> {
    let mut entries: Vec<_> = fs::read_dir(root).ok()?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    entries.into_iter().find_map(|entry| {
        let dir = entry.path();
        let kind = fs::read_to_string(dir.join("type")).ok()?;
        if kind.trim() != "Battery" {
            return None;
        }
        let percent: u8 = fs::read_to_string(dir.join("capacity")).ok()?.trim().parse().ok()?;
        let state = fs::read_to_string(dir.join("status")).map(|s| parse_state(&s)).unwrap_or(ChargeState::Unknown);
        Some(BatteryInfo { percent: percent.min(100), state })
    })
}

pub fn read_battery() -> Option<BatteryInfo> {
    read_battery_from(Path::new("/sys/class/power_supply"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_first_battery_and_skips_mains() {
        let dir = std::env::temp_dir().join(format!("mandara-bat-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("AC")).unwrap();
        fs::write(dir.join("AC/type"), "Mains\n").unwrap();
        fs::create_dir_all(dir.join("BAT0")).unwrap();
        fs::write(dir.join("BAT0/type"), "Battery\n").unwrap();
        fs::write(dir.join("BAT0/capacity"), "57\n").unwrap();
        fs::write(dir.join("BAT0/status"), "Discharging\n").unwrap();
        let info = read_battery_from(&dir).unwrap();
        assert_eq!(info, BatteryInfo { percent: 57, state: ChargeState::Discharging });
        assert_eq!(info.glyph(), "▅");
        fs::remove_dir_all(&dir).unwrap();
        assert!(read_battery_from(&dir).is_none());
    }
}
