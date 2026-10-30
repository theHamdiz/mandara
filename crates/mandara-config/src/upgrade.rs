//! Upgrading configs written under older defaults.
//!
//! Mandara's rule is that a user's setup stays as they left it: changing a
//! default only affects new installs. Two tables carry the version of the
//! defaults they were written for, and a config from before that version is
//! brought forward here, on every load, before anything reads it:
//!
//! * **`[plugins] defaults_version`** — 0.1 shipped almost every built-in
//!   switched on. Since version 1 the heavier and niche ones
//!   ([`OPT_IN_SINCE_V1`]) ship switched off, and Wallhaven browsing and
//!   cursor themes became the opt-in `wallhaven` and `cursors` plugins. An
//!   older config gets each of those plugins it still had on written into
//!   `enabled`, so nothing it showed goes away: Wallhaven when
//!   `wallpaper.wallhaven` was on, cursors when any cursor setting was set.
//! * **`[integrations] version`** — 0.1 themed most external apps by
//!   default. Since version 1 every app is off until switched on. A table
//!   still exactly at the 0.1 defaults was never touched by its user, so its
//!   apps are recorded as off: nothing new is written to them, and the files
//!   Mandara wrote back then are left as they are (no surprise rewrite of
//!   the user's files on upgrade; turning an app on and off again takes its
//!   theme back). A table the user changed keeps every value. Compositor
//!   borders live in Mandara's own drop-in, so they keep their value.
//!
//! The upgrade is idempotent and runs in memory; the next write persists it.

use crate::ShellConfig;

/// The `[plugins]` defaults this build ships (see the module docs).
pub const PLUGIN_DEFAULTS_VERSION: u32 = 1;
/// The `[integrations]` defaults this build ships.
pub const INTEGRATIONS_VERSION: u32 = 1;

/// Built-ins that were on by default before version 1 and ship switched off
/// since, each with the reason.
pub const OPT_IN_SINCE_V1: &[(&str, &str)] = &[
    ("app-installer", "a drop target for app packages, mostly used with the dock"),
    ("authenticator", "two-factor codes: wanted by some, kept in the keyring"),
    ("color-picker", "a designer's tool"),
    ("containers", "polls Docker and Podman"),
    ("drives", "watches mounts and removable drives"),
    ("net-speed", "samples network counters continuously"),
    ("notes", "a productivity card"),
    ("pomodoro", "a productivity card"),
    ("privacy", "keeps a monitor of microphone, camera and screen use running"),
    ("processes", "a process hunter for power users"),
    ("services", "systemd unit management for power users"),
    ("soundscapes", "an ambient sound synthesizer"),
    ("todo", "a productivity card"),
    ("updates", "checks package managers in the background"),
    ("world-clock", "a niche info card"),
];

/// The Wallhaven plugin's id (online wallpapers in the picker).
pub const WALLHAVEN_ID: &str = "wallhaven";
/// The cursor themes plugin's id (Studio → Themes → Cursor, `cursor …`).
pub const CURSORS_ID: &str = "cursors";

fn push_unique(list: &mut Vec<String>, id: &str) {
    if !list.iter().any(|x| x == id) {
        list.push(id.to_string());
    }
}

/// Brings `cfg` forward to the current defaults versions. Returns whether
/// anything changed.
pub fn upgrade(cfg: &mut ShellConfig) -> bool {
    let mut changed = upgrade_plugins(cfg);
    changed |= upgrade_integrations(cfg);
    changed
}

/// Keeps on what an older config had on under the old plugin defaults.
fn upgrade_plugins(cfg: &mut ShellConfig) -> bool {
    let legacy_wallhaven = cfg.wallpaper.wallhaven.take();
    if cfg.plugins.defaults_version >= PLUGIN_DEFAULTS_VERSION {
        return legacy_wallhaven.is_some();
    }
    let plugins = &mut cfg.plugins;
    let kept_on = |plugins: &crate::PluginsConfig, id: &str| !plugins.disabled.iter().any(|d| d == id) && !plugins.is_removed(id);
    for (id, _) in OPT_IN_SINCE_V1 {
        if kept_on(plugins, id) {
            push_unique(&mut plugins.enabled, id);
        }
    }
    // Wallhaven browsing was on by default.
    if legacy_wallhaven != Some(false) && kept_on(plugins, WALLHAVEN_ID) {
        push_unique(&mut plugins.enabled, WALLHAVEN_ID);
    }
    // A cursor someone picked keeps being applied.
    if cfg.compositor.input.cursor != crate::input::CursorInput::default() && kept_on(plugins, CURSORS_ID) {
        push_unique(&mut plugins.enabled, CURSORS_ID);
    }
    plugins.defaults_version = PLUGIN_DEFAULTS_VERSION;
    true
}

/// The 0.1 `[integrations]` defaults.
fn integrations_v0() -> crate::IntegrationsConfig {
    crate::IntegrationsConfig { version: 0, kvantum_apply: false, firefox: false, ..crate::IntegrationsConfig::all(true) }
}

/// Records apps that were only themed because 0.1 did so by default as off.
fn upgrade_integrations(cfg: &mut ShellConfig) -> bool {
    let i = &mut cfg.integrations;
    if i.version >= INTEGRATIONS_VERSION {
        return false;
    }
    let untouched = crate::IntegrationsConfig { version: 0, ..i.clone() } == integrations_v0();
    if untouched {
        let borders = i.compositor_borders;
        *i = crate::IntegrationsConfig { compositor_borders: borders, ..crate::IntegrationsConfig::all(false) };
    }
    i.version = INTEGRATIONS_VERSION;
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `[plugins]` / `[integrations]` / `[wallpaper]` excerpt as 0.1 wrote it.
    const V0_CONFIG: &str = r#"
[wallpaper]
wallhaven = true

[integrations]
ghostty = true
gtk4 = true
qt6 = true
zed = true
fuzzel = true
btop = true
compositor_borders = true
kvantum = true
kvantum_apply = false
kitty = true
foot = true
alacritty = true
wezterm = true
helix = true
neovim = true
firefox = false
vesktop = true
kde = true

[plugins]
enabled = ["dock"]
disabled = ["notes"]
"#;

    fn load(text: &str) -> ShellConfig {
        let mut cfg: ShellConfig = toml::from_str(text).unwrap();
        upgrade(&mut cfg);
        cfg
    }

    #[test]
    fn fresh_configs_are_current_and_upgrade_to_themselves() {
        let mut cfg = ShellConfig::default();
        assert_eq!(cfg.plugins.defaults_version, PLUGIN_DEFAULTS_VERSION);
        assert_eq!(cfg.integrations.version, INTEGRATIONS_VERSION);
        assert!(cfg.plugins.enabled.is_empty(), "nothing opt-in is on in a fresh config");
        assert!(!upgrade(&mut cfg), "a fresh config needs no upgrade");
        // And a written-then-read fresh config stays current.
        let back = load(&toml::to_string(&ShellConfig::default()).unwrap());
        assert_eq!(back, ShellConfig::default());
    }

    #[test]
    fn every_external_app_is_off_by_default() {
        let i = ShellConfig::default().integrations;
        assert!(!i.any_app() && !i.compositor_borders, "{i:?}");
        assert_eq!(i, crate::IntegrationsConfig::all(false));
    }

    #[test]
    fn old_configs_keep_the_plugins_they_had_on() {
        let cfg = load(V0_CONFIG);
        assert_eq!(cfg.plugins.defaults_version, PLUGIN_DEFAULTS_VERSION);
        for (id, _) in OPT_IN_SINCE_V1 {
            let on = cfg.plugins.opted_in(id);
            assert_eq!(on, *id != "notes", "{id}: kept on unless the user had switched it off");
        }
        assert!(cfg.plugins.opted_in("dock"), "the user's own choices stay");
        assert!(cfg.plugins.opted_in(WALLHAVEN_ID), "Wallhaven was on: the plugin is on");
        assert!(!cfg.plugins.opted_in(CURSORS_ID), "no cursor was set: cursors stay off");
        assert_eq!(cfg.wallpaper.wallhaven, None, "the old key is consumed");
        // Idempotent.
        let mut again = cfg.clone();
        assert!(!upgrade(&mut again));
        assert_eq!(again, cfg);
    }

    #[test]
    fn wallhaven_off_stays_off_and_a_chosen_cursor_turns_cursors_on() {
        let text = V0_CONFIG.replace("wallhaven = true", "wallhaven = false") + "\n[compositor.input.cursor]\ntheme = \"Bibata-Modern-Ice\"\n";
        let cfg = load(&text);
        assert!(!cfg.plugins.opted_in(WALLHAVEN_ID));
        assert!(cfg.plugins.opted_in(CURSORS_ID));
        // A removed plugin is never brought back.
        let removed = load(&V0_CONFIG.replace("disabled = [\"notes\"]", "disabled = [\"todo\"]\nremoved = [\"todo\"]"));
        assert!(!removed.plugins.opted_in("todo") && !removed.plugins.enabled.iter().any(|e| e == "todo"));
    }

    #[test]
    fn untouched_old_integrations_are_recorded_off_and_changed_ones_kept() {
        let cfg = load(V0_CONFIG);
        assert_eq!(cfg.integrations.version, INTEGRATIONS_VERSION);
        assert!(!cfg.integrations.any_app(), "on only by the old default: recorded off");
        assert!(cfg.integrations.compositor_borders, "borders live in Mandara's own drop-in and keep their value");
        // A user who changed anything chose their set: every value stays.
        let chosen = load(&V0_CONFIG.replace("kitty = true", "kitty = false"));
        assert!(chosen.integrations.gtk4 && chosen.integrations.ghostty && !chosen.integrations.kitty);
        let mut again = chosen.clone();
        assert!(!upgrade(&mut again), "idempotent");
    }
}
