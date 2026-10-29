//! Switching plugins on and off, removing built-in plugins and restoring
//! them.
//!
//! Built-in plugins are compiled into the shell, so "removing" one means:
//! off, taken out of every bar zone and sidebar list (its positions are
//! remembered in `plugins.removed_places`), and listed under "Removed
//! Mandara plugins" in Studio until restored. A few core plugins
//! ([`ESSENTIAL`]) hold the shell together and refuse both.

use mandara_config::{LayoutLane, LayoutSlot, ShellConfig};
use mandara_i18n::{tf, tk, tr};

use crate::plugin::Zone;

/// Core built-in plugins that can't be switched off or removed, each with
/// the reason shown when someone tries. (The compositor driver, the
/// notification server, OSDs, the lock screen and the Polkit agent are
/// shell services, not plugins, so they never appear as switches at all.)
pub const ESSENTIAL: &[(&str, &str)] = &[
    ("launcher", tk("It opens apps and runs Mandara's commands, including the one that turns plugins back on")),
    ("main-menu", tk("It holds Mandara Studio, the settings and the session actions")),
    ("session", tk("It is the on-screen way to lock, log out, restart and shut down")),
];

/// Why `id` can't be switched off, when it is [`ESSENTIAL`] (English key;
/// translate with `tr`).
pub fn essential_reason(id: &str) -> Option<&'static str> {
    ESSENTIAL.iter().find(|(e, _)| *e == id).map(|(_, why)| *why)
}

/// The refusal shown for an essential plugin named `name`.
pub fn essential_refusal(id: &str, name: &str) -> Option<String> {
    essential_reason(id).map(|why| tf("{plugin} is part of Mandara's core and can't be turned off or removed: {reason}", &[("plugin", &name), ("reason", &tr(why))]))
}

/// Bar lane of a manifest's default zone.
fn zone_lane(zone: Zone) -> LayoutLane {
    match zone {
        Zone::Left => LayoutLane::BarLeft,
        Zone::Center => LayoutLane::BarCenter,
        Zone::Right => LayoutLane::BarRight,
    }
}

fn push_unique(list: &mut Vec<String>, id: &str) {
    if !list.iter().any(|x| x == id) {
        list.push(id.to_string());
    }
}

/// Switches `id` on or off. Turning on a removed built-in restores it (with
/// its layout positions); essential plugins refuse to turn off.
pub fn set_enabled(cfg: &mut ShellConfig, id: &str, name: &str, on: bool, zone: Option<Zone>) -> Result<(), String> {
    if on {
        if cfg.plugins.is_removed(id) {
            restore(cfg, id, zone);
            return Ok(());
        }
        cfg.plugins.disabled.retain(|x| x != id);
        push_unique(&mut cfg.plugins.enabled, id);
        return Ok(());
    }
    if let Some(refusal) = essential_refusal(id, name) {
        return Err(refusal);
    }
    cfg.plugins.enabled.retain(|x| x != id);
    push_unique(&mut cfg.plugins.disabled, id);
    Ok(())
}

/// Removes built-in `id`: off, out of every layout (positions remembered),
/// and listed as removed. Essential plugins refuse.
pub fn remove(cfg: &mut ShellConfig, id: &str, name: &str) -> Result<(), String> {
    if let Some(refusal) = essential_refusal(id, name) {
        return Err(refusal);
    }
    let places = cfg.take_plugin_places(id);
    // Removing twice keeps the positions of the first removal.
    if !places.is_empty() {
        cfg.plugins.removed_places.insert(id.to_string(), places);
    }
    cfg.plugins.enabled.retain(|x| x != id);
    push_unique(&mut cfg.plugins.disabled, id);
    push_unique(&mut cfg.plugins.removed, id);
    Ok(())
}

/// Brings back a removed built-in, switched on, at its previous positions
/// (else where the default layout shows it, else the end of its default
/// bar `zone`). A plugin that wasn't removed is just switched on.
pub fn restore(cfg: &mut ShellConfig, id: &str, zone: Option<Zone>) {
    let was_removed = cfg.plugins.is_removed(id);
    cfg.plugins.removed.retain(|x| x != id);
    cfg.plugins.disabled.retain(|x| x != id);
    push_unique(&mut cfg.plugins.enabled, id);
    let remembered = cfg.plugins.removed_places.remove(id).unwrap_or_default();
    if !was_removed {
        return;
    }
    let mut places = if remembered.is_empty() { ShellConfig::default_plugin_places(id) } else { remembered };
    if places.is_empty()
        && let Some(zone) = zone
    {
        places.push(LayoutSlot::new(zone_lane(zone), usize::MAX));
    }
    cfg.put_plugin_places(id, &places);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn essential_plugins_refuse_off_and_remove() {
        let mut c = ShellConfig::default();
        for (id, _) in ESSENTIAL {
            assert!(set_enabled(&mut c, id, id, false, None).is_err(), "{id}");
            assert!(remove(&mut c, id, id).is_err(), "{id}");
        }
        assert!(c.plugins.disabled.is_empty() && c.plugins.removed.is_empty(), "refusals change nothing");
        let msg = essential_refusal("session", "Session").unwrap();
        assert!(msg.contains("Session"), "{msg}");
    }

    #[test]
    fn remove_takes_it_out_of_every_layout_and_restore_puts_it_back() {
        let mut c = ShellConfig::default();
        let before = c.clone();
        remove(&mut c, "weather", "Weather").unwrap();
        assert!(c.plugins.is_removed("weather"));
        assert!(c.plugins.disabled.iter().any(|x| x == "weather"));
        assert!(c.plugin_places("weather").is_empty(), "no holes left in bar or sidebar");
        assert!(!c.plugins.removed_places["weather"].is_empty());
        // Removing again keeps the remembered positions.
        let remembered = c.plugins.removed_places["weather"].clone();
        remove(&mut c, "weather", "Weather").unwrap();
        assert_eq!(c.plugins.removed_places["weather"], remembered);
        assert_eq!(c.plugins.removed.iter().filter(|x| *x == "weather").count(), 1);

        restore(&mut c, "weather", Some(Zone::Right));
        assert!(!c.plugins.is_removed("weather"));
        assert!(!c.plugins.disabled.iter().any(|x| x == "weather"));
        assert!(c.plugins.enabled.iter().any(|x| x == "weather"), "restore re-enables");
        assert!(c.plugins.removed_places.is_empty());
        assert_eq!(c.bar.right, before.bar.right);
        assert_eq!(c.sidebar.tabs, before.sidebar.tabs);
    }

    #[test]
    fn restore_without_positions_uses_the_default_zone() {
        let mut c = ShellConfig::default();
        c.plugins.removed.push("prayer-times".into());
        restore(&mut c, "prayer-times", Some(Zone::Right));
        assert_eq!(c.bar.right.last().map(String::as_str), Some("prayer-times"));
        // A plugin the default layout shows goes back where defaults put it.
        let mut c = ShellConfig::default();
        c.take_plugin_places("clock");
        c.plugins.removed.push("clock".into());
        restore(&mut c, "clock", Some(Zone::Right));
        assert_eq!(c.bar.center, ShellConfig::default().bar.center);
    }

    #[test]
    fn enabling_a_removed_plugin_restores_it_and_plain_toggles_stay_put() {
        let mut c = ShellConfig::default();
        remove(&mut c, "tray", "Tray").unwrap();
        set_enabled(&mut c, "tray", "Tray", true, Some(Zone::Right)).unwrap();
        assert!(!c.plugins.is_removed("tray"));
        assert_eq!(c.bar.right, ShellConfig::default().bar.right);
        // Off / on never moves a plugin in the layout.
        set_enabled(&mut c, "tray", "Tray", false, Some(Zone::Right)).unwrap();
        assert_eq!(c.bar.right, ShellConfig::default().bar.right);
        set_enabled(&mut c, "tray", "Tray", true, Some(Zone::Right)).unwrap();
        assert_eq!(c.plugins.enabled.iter().filter(|x| *x == "tray").count(), 1);
        assert!(!c.plugins.disabled.iter().any(|x| x == "tray"));
    }
}
