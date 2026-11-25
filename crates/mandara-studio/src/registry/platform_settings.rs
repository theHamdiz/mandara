//! Settings the running system can't carry out, with the reason shown in
//! place of their controls. On Linux nothing here locks; on FreeBSD the
//! rows that need BlueZ, power-profiles-daemon, NetworkManager's hotspot,
//! ddcutil, Linux's keyboard-LED files, a sound server for the visualizer,
//! or a plugin FreeBSD can't run say so.

use mandara_i18n::tk;

/// Why `path` can't apply on `freebsd` (a FreeBSD build passes `true`).
/// `sound_server` says whether PulseAudio or PipeWire answers.
pub(super) fn locked_on(freebsd: bool, sound_server: bool, path: &str) -> Option<&'static str> {
    if !freebsd {
        return None;
    }
    if let Some(id) = path.strip_prefix("/plugins/settings/").and_then(|rest| rest.split('/').next())
        && let Some(why) = mandara_plugins::lifecycle::FREEBSD_UNSUPPORTED.iter().find(|(p, _)| *p == id).map(|(_, why)| *why)
    {
        return Some(why);
    }
    match path {
        p if p.starts_with("/bluetooth/") => Some(tk("FreeBSD has no BlueZ, the Bluetooth service Mandara uses")),
        "/power/profile_ac" | "/power/profile_battery" | "/power/saver_at_low" | "/osd/kinds/power_profile" => Some(tk("FreeBSD has no power-profiles-daemon; powerd(8) sets the processor's speed there")),
        "/network/hotspot_ssid" | "/network/hotspot_band" => Some(tk("Hotspots are made with NetworkManager, which FreeBSD doesn't have")),
        "/displays/brightness/ddc" => Some(tk("Monitor brightness over DDC/CI uses Linux's i2c-dev, which FreeBSD doesn't have")),
        "/osd/kinds/keyboard_backlight" | "/osd/kinds/lock_keys" => Some(tk("Keyboard lights are read from Linux's /sys/class/leds, which FreeBSD doesn't have")),
        p if p.starts_with("/plugins/settings/media/visualizer_") && !sound_server => Some(tk("The visualizer listens through PulseAudio or PipeWire; start one to use it")),
        _ => None,
    }
}

/// Why choice `raw` of `path` can't be picked on `freebsd`.
pub(super) fn choice_locked_on(freebsd: bool, path: &str, raw: &str) -> Option<&'static str> {
    (freebsd && matches!(path, "/idle/suspend_mode" | "/power/critical_action") && matches!(raw, "hibernate" | "suspend-then-hibernate" | "hybrid-sleep" | "hybrid"))
        .then_some(tk("FreeBSD sleeps by suspending to memory (ACPI S3) only"))
}

/// Whether a PulseAudio or PipeWire server answers, checked once.
fn sound_server() -> bool {
    static RUNNING: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *RUNNING.get_or_init(mandara_services::audio::sound_server_running)
}

pub(super) fn locked(path: &str) -> Option<&'static str> {
    if !cfg!(target_os = "freebsd") {
        return None;
    }
    locked_on(true, sound_server(), path)
}

pub(super) fn choice_locked(path: &str, raw: &str) -> Option<&'static str> {
    choice_locked_on(cfg!(target_os = "freebsd"), path, raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn freebsd_locks_what_it_cannot_do() {
        assert!(locked_on(true, true, "/bluetooth/auto_connect").is_some());
        assert!(locked_on(true, true, "/plugins/settings/bluetooth/battery_warn_percent").is_some());
        assert!(locked_on(true, true, "/plugins/settings/services/journal_lines").is_some());
        assert!(locked_on(true, true, "/power/profile_ac").is_some());
        assert!(locked_on(true, true, "/displays/brightness/ddc").is_some());
        assert!(locked_on(true, true, "/plugins/settings/media/visualizer_style").is_none(), "with a sound server");
        assert!(locked_on(true, false, "/plugins/settings/media/visualizer_style").is_some());
        assert!(locked_on(true, false, "/plugins/settings/media/lyrics").is_none());
        assert!(locked_on(true, true, "/power/low_percent").is_none(), "ACPI batteries still alert");
        assert!(choice_locked_on(true, "/idle/suspend_mode", "hibernate").is_some());
        assert!(choice_locked_on(true, "/idle/suspend_mode", "suspend").is_none());
    }

    #[test]
    fn linux_locks_nothing() {
        for path in ["/bluetooth/auto_connect", "/power/profile_ac", "/plugins/settings/services/journal_lines", "/plugins/settings/media/visualizer_style"] {
            assert_eq!(locked_on(false, false, path), None);
        }
        assert_eq!(choice_locked_on(false, "/idle/suspend_mode", "hibernate"), None);
    }
}
