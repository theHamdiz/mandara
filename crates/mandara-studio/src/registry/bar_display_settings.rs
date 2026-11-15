//! What the Resources, Notifications, Network and Bluetooth bar items show
//! (`[plugins.settings.<id>] bar_style`): their classic icon, or a live
//! chip ([`mandara_config::bar_display`]), with the chip's own options.
//! Each sits in its plugin's settings; an option a look doesn't use is
//! shown locked with the reason.

use mandara_config::ShellConfig;
use mandara_config::bar_display::{
    self as bd, BarDisplay, BluetoothBar, NetworkBar, NotificationsBar, ResourcesBar,
};
use mandara_i18n::tk;
use serde_json::Value;

use super::{Control, Section, Setting, sd};

/// Plugin groups: the plugins' English names (matched against manifests).
const RESOURCES: &str = tk("Resources");
const NOTIFICATIONS: &str = tk("Notifications");
const NETWORK: &str = tk("Network");
const BLUETOOTH: &str = tk("Bluetooth");

const IN_THE_BAR: &str = tk("In the bar");
const TEXT_WIDTH: &str = tk("Text width");

/// Slider steps (bounds come from the shared ranges).
const MS_STEP: f64 = 250.0;
const PX_STEP: f64 = 10.0;

const RESOURCES_LOOKS: &[(&str, &str)] = &[("rings", tk("Rings and numbers")), ("icon", tk("Icon")), ("numbers", tk("Numbers")), ("sparkline", tk("Sparkline")), ("chart", tk("Inline chart"))];
const RESOURCES_METRICS: &[(&str, &str)] = &[("both", tk("CPU and memory")), ("cpu", tk("CPU")), ("memory", tk("Memory"))];
const NOTIFICATIONS_LOOKS: &[(&str, &str)] = &[("icon", tk("Bell and badge")), ("latest", tk("Latest notification"))];
const NETWORK_LOOKS: &[(&str, &str)] = &[("icon", tk("Icon")), ("name", tk("Network name")), ("signal", tk("Signal strength")), ("both", tk("Name and strength"))];
const BLUETOOTH_LOOKS: &[(&str, &str)] = &[("icon", tk("Icon and count")), ("name", tk("Device name")), ("battery", tk("Name and battery"))];

/// A slider over an integer `(min, max)` range.
fn ints((min, max): (i64, i64), step: f64, unit: &'static str) -> Control {
    Control::Slider { min: min as f64, max: max as f64, step, unit }
}

fn choice(group: &'static str, path: &'static str, help: &'static str, choices: &'static [(&'static str, &'static str)], default: &'static str) -> Setting {
    sd(Section::Plugins, group, path, IN_THE_BAR, help, Control::Choice(choices), Value::from(default))
}

pub fn entries() -> Vec<Setting> {
    use Section::Plugins;
    vec![
        choice(RESOURCES, "/plugins/settings/resources/bar_style", tk("Rings, one icon, labelled numbers or a small live graph beside each number; a click opens the panel either way"), RESOURCES_LOOKS, ResourcesBar::ALL[0].1),
        sd(Plugins, RESOURCES, "/plugins/settings/resources/bar_metrics", tk("Readings"), tk("What the bar item shows"), Control::Choice(RESOURCES_METRICS), Value::from(bd::ResourcesMetrics::ALL[0].1)),
        sd(Plugins, RESOURCES, "/plugins/settings/resources/bar_refresh_ms", tk("Chip refresh every"), tk("How often the Numbers, Sparkline and Chart chips update while the bar shows them"), ints(bd::RESOURCES_BAR_REFRESH_MS_RANGE, MS_STEP, " ms"), Value::from(bd::RESOURCES_BAR_REFRESH_MS)),
        choice(NOTIFICATIONS, "/plugins/settings/notifications/bar_style", tk("The bell and its badge, or the latest notification's app and summary with the count (anyone looking at your screen can read it)"), NOTIFICATIONS_LOOKS, NotificationsBar::ALL[0].1),
        sd(Plugins, NOTIFICATIONS, "/plugins/settings/notifications/bar_width", TEXT_WIDTH, tk("Room for the latest notification before it trails off"), ints(bd::NOTIFICATIONS_BAR_WIDTH_RANGE, PX_STEP, " px"), Value::from(bd::NOTIFICATIONS_BAR_WIDTH)),
        choice(NETWORK, "/plugins/settings/network/bar_style", tk("The connection icon, or a chip with the network's name and / or its Wi-Fi signal"), NETWORK_LOOKS, NetworkBar::ALL[0].1),
        sd(Plugins, NETWORK, "/plugins/settings/network/bar_width", TEXT_WIDTH, tk("Room for the network's name before it trails off"), ints(bd::NETWORK_BAR_WIDTH_RANGE, PX_STEP, " px"), Value::from(bd::NETWORK_BAR_WIDTH)),
        choice(BLUETOOTH, "/plugins/settings/bluetooth/bar_style", tk("The Bluetooth icon with how many devices are connected, or the connected device's name (and its battery)"), BLUETOOTH_LOOKS, BluetoothBar::ALL[0].1),
        sd(Plugins, BLUETOOTH, "/plugins/settings/bluetooth/bar_width", TEXT_WIDTH, tk("Room for the device's name before it trails off"), ints(bd::BLUETOOTH_BAR_WIDTH_RANGE, PX_STEP, " px"), Value::from(bd::BLUETOOTH_BAR_WIDTH)),
    ]
}

/// The look picked for `plugin` in `cfg`.
fn look<T: BarDisplay>(cfg: &ShellConfig, plugin: &str) -> T {
    T::parse(cfg.plugins.setting_str(plugin, bd::KEY))
}

/// Why a chip option doesn't apply to the look picked now.
pub fn locked(path: &str, cfg: &ShellConfig) -> Option<&'static str> {
    match path {
        "/plugins/settings/resources/bar_metrics" if look::<ResourcesBar>(cfg, "resources") == ResourcesBar::Icon => Some(tk("The icon shows no readings")),
        "/plugins/settings/resources/bar_refresh_ms" if !look::<ResourcesBar>(cfg, "resources").ticks() => Some(tk("For the Numbers, Sparkline and Chart chips; the rings follow Refresh every")),
        "/plugins/settings/notifications/bar_width" if look::<NotificationsBar>(cfg, "notifications") == NotificationsBar::Icon => Some(tk("For the Latest notification chip")),
        "/plugins/settings/network/bar_width" if !look::<NetworkBar>(cfg, "network").name() => Some(tk("For the chips that show the network's name")),
        "/plugins/settings/bluetooth/bar_width" if look::<BluetoothBar>(cfg, "bluetooth") == BluetoothBar::Icon => Some(tk("For the chips that show the device's name")),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Studio offers exactly the looks the shell knows, the classic first.
    #[test]
    fn choices_match_the_shared_looks() {
        fn ids(c: &'static [(&'static str, &'static str)]) -> Vec<&'static str> {
            c.iter().map(|(id, _)| *id).collect()
        }
        assert_eq!(ids(RESOURCES_LOOKS), ResourcesBar::ids().collect::<Vec<_>>());
        assert_eq!(ids(RESOURCES_METRICS), bd::ResourcesMetrics::ids().collect::<Vec<_>>());
        assert_eq!(ids(NOTIFICATIONS_LOOKS), NotificationsBar::ids().collect::<Vec<_>>());
        assert_eq!(ids(NETWORK_LOOKS), NetworkBar::ids().collect::<Vec<_>>());
        assert_eq!(ids(BLUETOOTH_LOOKS), BluetoothBar::ids().collect::<Vec<_>>());
    }

    #[test]
    fn chip_options_lock_while_the_classic_look_is_on() {
        let mut cfg = ShellConfig::default();
        // The rings show readings, so they pick metrics but follow the
        // panel's refresh; every other chip option waits for a chip.
        for e in entries() {
            let open = e.path.ends_with("/bar_style") || e.path.ends_with("/bar_metrics");
            assert_eq!(locked(e.path, &cfg).is_none(), open, "{}", e.path);
        }
        let set = |cfg: &mut ShellConfig, id: &str, look: &str| {
            cfg.plugins.settings.entry(id.into()).or_default().insert(bd::KEY.into(), toml::Value::String(look.into()));
        };
        set(&mut cfg, "resources", "sparkline");
        set(&mut cfg, "network", "signal");
        assert!(locked("/plugins/settings/resources/bar_refresh_ms", &cfg).is_none());
        assert!(locked("/plugins/settings/network/bar_width", &cfg).is_some(), "the signal look shows no name");
        set(&mut cfg, "resources", "rings");
        set(&mut cfg, "network", "both");
        assert!(locked("/plugins/settings/resources/bar_metrics", &cfg).is_none());
        assert!(locked("/plugins/settings/resources/bar_refresh_ms", &cfg).is_some());
        assert!(locked("/plugins/settings/network/bar_width", &cfg).is_none());
    }
}
