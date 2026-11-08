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
//! * **`[compositor] keybinds_version`** — the volume keys shipped first;
//!   the brightness keys and (version 1) the media keys came later, but a
//!   saved key list never grew, so existing installs never got them. An
//!   older list gets every hardware key it lacks (by key and modifiers);
//!   chords the user's own compositor config binds are still skipped when
//!   the drop-in is written.
//!
//! * **`[desktop.widgets] version`** — before version 1 a clock's digits
//!   were Western unless `eastern_numerals` was set, the depth clock's pill
//!   always showed Gregorian · Hijri, and widgets were set in the display
//!   face. Since then each widget has its own numerals (following its
//!   language by default), calendar and font (the widgets font by
//!   default). An older list gets explicit values that show exactly what
//!   it showed ([`crate::desktop_widgets::upgrade`]).
//! * **`[wallpaper] depth_mask`** — before 0.5 the subject mask was
//!   `auto`, `custom` (your masks only) or `off`. The depth effect is now
//!   the `wallpaper-depth` plugin with two exclusive modes (hand-tuned or
//!   model) and your own masks always win. `off` becomes
//!   `depth_effect = false` (the effect stays off); `auto` and `custom`
//!   keep the effect on in hand-tuned mode. The old key is then dropped.
//!
//! * **`[dock] pins_version`** — before version 1 the dock's launcher
//!   button sat fixed at its start (or, with `launcher_at_end`, its end),
//!   on unless `launcher_button = false`. It is now an entry of `pinned`
//!   ([`crate::dock::LAUNCHER_PIN`]) that moves like a pinned app. An older
//!   table gets the button pinned first, or after the pinned apps when it
//!   was at the end, and loses the old keys.
//!
//! The upgrade is idempotent and runs in memory; the next write persists it.

use crate::ShellConfig;

/// The `[plugins]` defaults this build ships (see the module docs).
pub const PLUGIN_DEFAULTS_VERSION: u32 = 1;
/// The `[integrations]` defaults this build ships.
pub const INTEGRATIONS_VERSION: u32 = 1;
/// The key defaults this build ships: 1 added the hardware keys, 2 Alt+V
/// for the clipboard and the Print keys for screenshots.
pub const KEYBIND_DEFAULTS_VERSION: u32 = 2;
/// The lock screen look defaults this build ships.
pub const LOCK_LOOK_VERSION: u32 = 1;
/// The dock pin defaults this build ships: 1 keeps the launcher button in
/// `pinned`.
pub const DOCK_PINS_VERSION: u32 = 1;

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
    changed |= upgrade_keybinds(cfg);
    changed |= upgrade_lock_look(cfg);
    changed |= crate::desktop_widgets::upgrade(&mut cfg.desktop.widgets);
    changed |= upgrade_depth_mask(cfg);
    changed |= cfg.dock.upgrade_launcher_button();
    changed
}

/// The pre-0.5 subject-mask choice: `off` keeps the depth effect off.
fn upgrade_depth_mask(cfg: &mut ShellConfig) -> bool {
    let Some(old) = cfg.wallpaper.depth_mask.take() else { return false };
    if old == crate::DepthMask::Off {
        cfg.wallpaper.depth_effect = false;
    }
    true
}

/// The lock screen blurred and darkened the wallpaper beyond recognition
/// by default; a `[lock]` still at those old defaults gets the lighter
/// ones. Values the user chose stay.
fn upgrade_lock_look(cfg: &mut ShellConfig) -> bool {
    use crate::desktop::defaults::{LOCK_BLUR_PX, LOCK_BLUR_PX_V0, LOCK_DIM, LOCK_DIM_V0};
    let lock = &mut cfg.lock;
    if lock.look_version >= LOCK_LOOK_VERSION {
        return false;
    }
    if (lock.blur - LOCK_BLUR_PX_V0).abs() < 0.01 && (lock.dim - LOCK_DIM_V0).abs() < 0.001 {
        lock.blur = LOCK_BLUR_PX;
        lock.dim = LOCK_DIM;
    }
    lock.look_version = LOCK_LOOK_VERSION;
    true
}

/// Adds the hardware keys (media, brightness, volume) an older key list
/// lacks.
fn upgrade_keybinds(cfg: &mut ShellConfig) -> bool {
    let c = &mut cfg.compositor;
    if c.keybinds_version >= KEYBIND_DEFAULTS_VERSION {
        return false;
    }
    let mut new = if c.keybinds_version < 1 { crate::compositor::hardware_keybinds() } else { Vec::new() };
    new.extend(crate::compositor::added_keybinds().into_iter().filter(|(since, _)| *since > c.keybinds_version).map(|(_, bind)| bind));
    for bind in new {
        if !c.keybinds.iter().any(|b| b.chord() == bind.chord()) {
            c.keybinds.push(bind);
        }
    }
    c.keybinds_version = KEYBIND_DEFAULTS_VERSION;
    true
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
    crate::IntegrationsConfig { version: 0, kvantum_apply: false, firefox: false, cava: false, tmux: false, zellij: false, spicetify: false, ..crate::IntegrationsConfig::all(true) }
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
    fn an_old_dock_launcher_button_is_pinned_where_it_was() {
        use crate::dock::LAUNCHER_PIN;
        let mut c: ShellConfig = toml::from_str("[dock]\npinned = [\"firefox\"]\nlauncher_button = true\nlauncher_at_end = true\n").unwrap();
        assert!(upgrade(&mut c));
        assert_eq!(c.dock.pinned, ["firefox", LAUNCHER_PIN]);
        assert!(!toml::to_string(&c).unwrap().contains("launcher_button"));
        // A dock table without a pinned list keeps the default one, which
        // already has the button first.
        let mut d: ShellConfig = toml::from_str("[dock]\nlauncher_button = true\nlauncher_at_end = false\n").unwrap();
        upgrade(&mut d);
        assert_eq!(d.dock.pinned, ShellConfig::default().dock.pinned);
        assert_eq!(d.dock.pinned.iter().filter(|p| *p == LAUNCHER_PIN).count(), 1);
    }

    #[test]
    fn old_depth_mask_choice_carries_over() {
        let mut off: ShellConfig = toml::from_str("[wallpaper]\ndepth_mask = \"off\"\n").unwrap();
        assert!(off.wallpaper.depth_effect);
        assert!(upgrade(&mut off));
        assert!(!off.wallpaper.depth_effect, "an effect switched off stays off");
        assert_eq!(off.wallpaper.depth_mask, None, "the old key is consumed");
        assert!(!toml::to_string(&off).unwrap().contains("depth_mask"));
        for old in ["auto", "custom"] {
            let mut c: ShellConfig = toml::from_str(&format!("[wallpaper]\ndepth_mask = \"{old}\"\n")).unwrap();
            upgrade(&mut c);
            assert!(c.wallpaper.depth_effect);
            assert_eq!(c.wallpaper.depth.mode, crate::DepthMode::HandTuned);
        }
        let mut fresh = ShellConfig::default();
        assert!(!upgrade_depth_mask(&mut fresh), "nothing to do without the old key");
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

    /// Widgets written before their numerals, calendar and font were
    /// options show what they showed: Western digits even in Arabic, the
    /// Eastern ones where asked for, the depth pill's two calendars.
    #[test]
    fn old_widget_lists_keep_their_digits_calendars_and_faces() {
        use crate::desktop_widgets::{Calendar, FontRole, Numerals, WIDGETS_VERSION, WidgetKind};
        let old = "[desktop.widgets]\nmigrated = true\n\n[[desktop.widgets.items]]\nid = \"depth-clock\"\nkind = \"clock\"\nface = \"stacked\"\n[desktop.widgets.items.look]\nfont = \"display\"\n\n[[desktop.widgets.items]]\nid = \"clock-1\"\nkind = \"clock\"\nface = \"minimal\"\neastern_numerals = true\n\n[[desktop.widgets.items]]\nid = \"monitor-1\"\nkind = \"monitor\"\n[desktop.widgets.items.look]\nfont = \"mono\"\n";
        let cfg = load(old);
        let w = &cfg.desktop.widgets;
        assert_eq!(w.version, WIDGETS_VERSION);
        let WidgetKind::Clock(depth) = &w.items[0].kind else { panic!() };
        assert_eq!((depth.locale.numerals, depth.locale.calendar, depth.look.font), (Numerals::Western, Calendar::Both, FontRole::Widgets));
        let WidgetKind::Clock(c) = &w.items[1].kind else { panic!() };
        assert_eq!((c.locale.numerals, c.locale.calendar), (Numerals::EasternArabic, Calendar::Gregorian));
        let WidgetKind::Monitor(m) = &w.items[2].kind else { panic!() };
        assert_eq!((m.locale.numerals, m.look.font), (Numerals::Western, FontRole::Mono), "a chosen face stays");
        // Only the widgets: the calendar's own `eastern_numerals` is a
        // different, current setting.
        let text = toml::to_string(&cfg.desktop.widgets).unwrap();
        assert!(!text.contains("eastern_numerals"), "the old key isn't written back");
        let mut again = cfg.clone();
        assert!(!upgrade(&mut again), "runs once");
        // New widgets follow the language and the widgets font.
        let fresh = crate::desktop_widgets::ClockWidget::default();
        assert_eq!((fresh.locale.numerals, fresh.look.font), (Numerals::Auto, FontRole::Widgets));
    }

    #[test]
    fn every_external_app_is_off_by_default() {
        let i = ShellConfig::default().integrations;
        assert!(!i.any_app() && !i.compositor_borders, "{i:?}");
        assert_eq!(i, crate::IntegrationsConfig::all(false));
    }

    #[test]
    fn the_old_lock_defaults_become_the_lighter_ones() {
        use crate::desktop::defaults::{LOCK_BLUR_PX, LOCK_DIM};
        let old: ShellConfig = toml::from_str("[lock]\nblur = 28.0\ndim = 0.35\n").unwrap();
        let mut cfg = old.clone();
        assert!(upgrade(&mut cfg));
        assert_eq!((cfg.lock.blur, cfg.lock.dim), (LOCK_BLUR_PX, LOCK_DIM));
        let mut chosen: ShellConfig = toml::from_str("[lock]\nblur = 40.0\ndim = 0.35\n").unwrap();
        upgrade(&mut chosen);
        assert_eq!((chosen.lock.blur, chosen.lock.dim), (40.0, 0.35), "your own values stay");
        assert!(!upgrade(&mut cfg), "runs once");
    }

    /// Issue #13: an install from before the media and brightness keys gets
    /// them added once; keys it already binds keep their action.
    #[test]
    fn older_key_lists_get_the_hardware_keys() {
        use crate::compositor::{BindAction, Keybind};
        let mut cfg = ShellConfig::default();
        let own_play = Keybind::new(&[], "XF86AudioPlay", BindAction::Mandara("media next".into()));
        cfg.compositor.keybinds = vec![Keybind::new(&["super"], "space", BindAction::Mandara("launcher toggle".into())), own_play.clone()];
        cfg.compositor.keybinds_version = 0;
        assert!(upgrade(&mut cfg));
        let keys: Vec<&str> = cfg.compositor.keybinds.iter().map(|b| b.key.as_str()).collect();
        for key in ["XF86AudioNext", "XF86AudioPrev", "XF86MonBrightnessUp", "XF86MonBrightnessDown", "XF86AudioMute", "XF86AudioRaiseVolume"] {
            assert!(keys.contains(&key), "{key} missing from {keys:?}");
        }
        assert_eq!(cfg.compositor.keybinds.iter().filter(|b| b.key == "XF86AudioPlay").collect::<Vec<_>>(), [&own_play], "your own binding stays");
        assert_eq!(cfg.compositor.keybinds_version, KEYBIND_DEFAULTS_VERSION);
        let after = cfg.compositor.keybinds.len();
        assert!(!upgrade(&mut cfg) && cfg.compositor.keybinds.len() == after, "runs once");
        // A file without the field reads as version 0.
        let old: ShellConfig = toml::from_str("[compositor]\nmanage = true\n").unwrap();
        assert_eq!(old.compositor.keybinds_version, 0);
    }

    /// Alt+V opens the clipboard on configs from before it was a default,
    /// unless Alt+V already does something else there.
    #[test]
    fn older_key_lists_get_alt_v_for_the_clipboard() {
        use crate::compositor::{BindAction, Keybind};
        let clipboard = |b: &Keybind| b.chord() == "alt+v" && b.action == BindAction::Mandara("panel toggle clipboard".into());
        let mut cfg = ShellConfig::default();
        cfg.compositor.keybinds.retain(|b| !clipboard(b));
        cfg.compositor.keybinds_version = 1;
        assert!(upgrade(&mut cfg));
        assert_eq!(cfg.compositor.keybinds.iter().filter(|b| clipboard(b)).count(), 1);
        assert!(!cfg.compositor.keybinds.iter().any(|b| b.key == "XF86AudioPlay" && b.chord() != "xf86audioplay"), "version 1 isn't given the hardware keys twice");
        let mut taken = ShellConfig::default();
        let own = Keybind::new(&["alt"], "v", BindAction::Spawn("my-clipboard".into()));
        taken.compositor.keybinds = vec![own.clone()];
        taken.compositor.keybinds_version = 1;
        upgrade(&mut taken);
        let alt_v: Vec<&Keybind> = taken.compositor.keybinds.iter().filter(|b| b.chord() == "alt+v").collect();
        assert_eq!(alt_v, [&own], "your own Alt+V stays, and no second one is added");
        assert!(taken.compositor.keybinds.iter().any(|b| b.key == "Print" && b.mods.is_empty()), "the Print keys still arrive");
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
