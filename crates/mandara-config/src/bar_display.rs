//! What a plugin's bar item shows (`[plugins.settings.<id>] bar_style`):
//! its classic look, or a compact live chip that still opens the plugin's
//! panel on click. The choices, defaults and ranges live here so the shell
//! (which draws them) and Studio (which offers them) never disagree.
//!
//! Every plugin's default is the look it had before chips existed, so an
//! existing setup keeps its bar exactly as it was until you pick a chip.

/// A plugin's bar look: a fixed set of choices, each with the id written
/// to the config. The first choice is the default.
pub trait BarDisplay: Copy + Eq + 'static {
    /// Every choice with its config id, the default first.
    const ALL: &'static [(Self, &'static str)];

    /// The look for a raw config value; unknown or missing values fall
    /// back to the default (the classic look).
    fn parse(raw: Option<&str>) -> Self {
        raw.and_then(|r| Self::ALL.iter().find(|(_, id)| *id == r)).map_or(Self::ALL[0].0, |(v, _)| *v)
    }

    /// The config id of this look.
    fn id(self) -> &'static str {
        Self::ALL.iter().find(|(v, _)| *v == self).map_or(Self::ALL[0].1, |(_, id)| id)
    }

    /// The config ids, the default first (commands, docs, Studio).
    fn ids() -> impl Iterator<Item = &'static str> {
        Self::ALL.iter().map(|(_, id)| *id)
    }
}

/// The config key every chip-capable plugin reads its look from.
pub const KEY: &str = "bar_style";

// ── Resources ────────────────────────────────────────────────────────────

/// The Resources bar item.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ResourcesBar {
    /// Two rings with their percentages (the classic look).
    Rings,
    /// One glyph that opens the panel.
    Icon,
    /// Short labels with live percentages.
    Numbers,
    /// A small live graph beside each percentage.
    Sparkline,
}

impl BarDisplay for ResourcesBar {
    const ALL: &'static [(Self, &'static str)] = &[(Self::Rings, "rings"), (Self::Icon, "icon"), (Self::Numbers, "numbers"), (Self::Sparkline, "sparkline")];
}

impl ResourcesBar {
    /// Shows live values (re-renders on every sample).
    pub fn live(self) -> bool {
        self != Self::Icon
    }

    /// Ticks at the chip's own cadence (`bar_refresh_ms`) instead of the
    /// panel's `refresh_ms`, which the classic rings keep following.
    pub fn ticks(self) -> bool {
        matches!(self, Self::Numbers | Self::Sparkline)
    }
}

/// Which readings a Resources chip shows (`bar_metrics`).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ResourcesMetrics {
    Both,
    Cpu,
    Memory,
}

impl BarDisplay for ResourcesMetrics {
    const ALL: &'static [(Self, &'static str)] = &[(Self::Both, "both"), (Self::Cpu, "cpu"), (Self::Memory, "memory")];
}

impl ResourcesMetrics {
    pub fn cpu(self) -> bool {
        self != Self::Memory
    }

    pub fn memory(self) -> bool {
        self != Self::Cpu
    }
}

/// How often a Resources chip updates while it is on screen, in ms
/// (`bar_refresh_ms`).
pub const RESOURCES_BAR_REFRESH_MS: i64 = 1000;
pub const RESOURCES_BAR_REFRESH_MS_RANGE: (i64, i64) = (500, 10_000);

// ── Notifications ────────────────────────────────────────────────────────

/// The Notifications bar item.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NotificationsBar {
    /// The bell with its unread badge (the classic look).
    Icon,
    /// The bell, the latest notification and the unread count.
    Latest,
}

impl BarDisplay for NotificationsBar {
    const ALL: &'static [(Self, &'static str)] = &[(Self::Icon, "icon"), (Self::Latest, "latest")];
}

/// Room for the latest notification's text, in px (`bar_width`).
pub const NOTIFICATIONS_BAR_WIDTH: i64 = 180;
pub const NOTIFICATIONS_BAR_WIDTH_RANGE: (i64, i64) = (80, 360);

// ── Network ──────────────────────────────────────────────────────────────

/// The Network bar item.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum NetworkBar {
    /// The connection glyph alone (the classic look).
    Icon,
    /// The glyph and the network's name.
    Name,
    /// Signal bars and the Wi-Fi strength.
    Signal,
    /// The glyph, the network's name and the Wi-Fi strength.
    Both,
}

impl BarDisplay for NetworkBar {
    const ALL: &'static [(Self, &'static str)] = &[(Self::Icon, "icon"), (Self::Name, "name"), (Self::Signal, "signal"), (Self::Both, "both")];
}

impl NetworkBar {
    pub fn name(self) -> bool {
        matches!(self, Self::Name | Self::Both)
    }

    pub fn strength(self) -> bool {
        matches!(self, Self::Signal | Self::Both)
    }
}

/// Room for the network's name, in px (`bar_width`).
pub const NETWORK_BAR_WIDTH: i64 = 120;
pub const NETWORK_BAR_WIDTH_RANGE: (i64, i64) = (60, 240);

// ── Bluetooth ────────────────────────────────────────────────────────────

/// The Bluetooth bar item.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum BluetoothBar {
    /// The glyph with the connected count (the classic look).
    Icon,
    /// The connected device's name (or how many are connected).
    Name,
    /// The device's name and its battery, when it reports one.
    Battery,
}

impl BarDisplay for BluetoothBar {
    const ALL: &'static [(Self, &'static str)] = &[(Self::Icon, "icon"), (Self::Name, "name"), (Self::Battery, "battery")];
}

/// Room for the device's name, in px (`bar_width`).
pub const BLUETOOTH_BAR_WIDTH: i64 = 120;
pub const BLUETOOTH_BAR_WIDTH_RANGE: (i64, i64) = (60, 240);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_and_unknown_values_keep_the_classic_look() {
        assert_eq!(ResourcesBar::parse(None), ResourcesBar::Rings);
        assert_eq!(ResourcesBar::parse(Some("nope")), ResourcesBar::Rings);
        assert_eq!(NotificationsBar::parse(None), NotificationsBar::Icon);
        assert_eq!(NetworkBar::parse(Some("")), NetworkBar::Icon);
        assert_eq!(BluetoothBar::parse(None), BluetoothBar::Icon);
        assert_eq!(ResourcesMetrics::parse(None), ResourcesMetrics::Both);
    }

    #[test]
    fn ids_round_trip() {
        fn check<T: BarDisplay + std::fmt::Debug>() {
            for (v, id) in T::ALL {
                assert_eq!(T::parse(Some(id)), *v);
                assert_eq!(v.id(), *id);
            }
            let ids: Vec<_> = T::ids().collect();
            let mut unique = ids.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(unique.len(), ids.len(), "duplicate ids");
        }
        check::<ResourcesBar>();
        check::<ResourcesMetrics>();
        check::<NotificationsBar>();
        check::<NetworkBar>();
        check::<BluetoothBar>();
    }

    #[test]
    fn only_chips_tick_and_defaults_fit_their_ranges() {
        assert!(!ResourcesBar::Rings.ticks() && !ResourcesBar::Icon.ticks());
        assert!(ResourcesBar::Numbers.ticks() && ResourcesBar::Sparkline.ticks());
        assert!(!ResourcesBar::Icon.live() && ResourcesBar::Rings.live());
        assert!(NetworkBar::Both.name() && NetworkBar::Both.strength() && !NetworkBar::Icon.name());
        for (v, (lo, hi)) in [
            (RESOURCES_BAR_REFRESH_MS, RESOURCES_BAR_REFRESH_MS_RANGE),
            (NOTIFICATIONS_BAR_WIDTH, NOTIFICATIONS_BAR_WIDTH_RANGE),
            (NETWORK_BAR_WIDTH, NETWORK_BAR_WIDTH_RANGE),
            (BLUETOOTH_BAR_WIDTH, BLUETOOTH_BAR_WIDTH_RANGE),
        ] {
            assert!((lo..=hi).contains(&v));
        }
    }
}
