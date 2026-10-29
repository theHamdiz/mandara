//! Studio → Desktop group, end to end through the config file: every
//! setting Studio writes must reach a running shell (the shell's
//! `ConfigStore` reloads the file Studio flushed: the hot-reload path) and
//! survive a restart (a fresh `ConfigStore` on the same file), and the
//! behaviour switches behind hot corners and the launcher / dock pairing
//! must follow those values.

use mandara_config::desktop::{CornerAction, ScreenCorner};
use mandara_config::workflow::{DOCK_PLUGIN_ID, LauncherPosition};
use mandara_config::{ConfigStore, ReloadOutcome, ShellConfig};
use mandara_studio::registry::{self, Control, NavGroup, Setting};
use serde_json::Value;

/// A shell and a Studio sharing one config file.
struct Session {
    _dir: tempfile::TempDir,
    path: std::path::PathBuf,
    shell: ConfigStore,
    studio: ConfigStore,
}

impl Session {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let shell = ConfigStore::open(&path).unwrap();
        let studio = ConfigStore::open(&path).unwrap();
        Self { _dir: dir, path, shell, studio }
    }

    /// Studio writes `setting = value` (as its controls do) and flushes; the
    /// shell reloads the file. Returns the shell's config afterwards.
    fn apply(&mut self, setting: &Setting, value: Value) -> Result<ShellConfig, String> {
        let next = registry::set(self.studio.get(), setting, value)?;
        self.studio.replace(next);
        self.studio.flush().map_err(|e| e.to_string())?;
        self.shell.reload_from_disk().map_err(|e| e.to_string())?;
        Ok((*self.shell.snapshot()).clone())
    }

    /// Like [`Self::apply`] for a whole-config edit (Studio's plugin
    /// switches write `plugins.enabled` directly).
    fn edit(&mut self, f: impl FnOnce(&mut ShellConfig)) -> ShellConfig {
        self.studio.update(f);
        self.studio.flush().unwrap();
        self.shell.reload_from_disk().unwrap();
        (*self.shell.snapshot()).clone()
    }

    fn restarted(&self) -> ShellConfig {
        (*ConfigStore::open(&self.path).unwrap().snapshot()).clone()
    }
}

fn desktop_settings() -> Vec<&'static Setting> {
    let sections = NavGroup::Desktop.sections();
    registry::settings().iter().filter(|s| sections.contains(&s.section)).collect()
}

/// Values for `setting` other than `current`, most likely first.
fn other_values(setting: &Setting, current: &Value) -> Vec<Value> {
    match &setting.control {
        Control::Toggle => vec![Value::from(!current.as_bool().unwrap_or(false))],
        Control::Slider { min, max, step, .. } | Control::OptionalSlider { min, max, step, .. } => {
            let mid = ((min + max) / 2.0 / step).round() * step;
            [*max, *min, mid].into_iter().map(Value::from).collect()
        }
        Control::Choice(choices) => choices.iter().map(|(v, _)| registry::choice_value(v)).collect(),
        Control::Text { .. } | Control::OptionalText { .. } => vec![Value::from("audit-value")],
        Control::List { .. } => vec![serde_json::json!(["audit-a", "audit-b"])],
        Control::Number { min, max } => vec![Value::from(*min), Value::from(*max)],
        Control::Icon => vec![Value::from("search"), Value::from("grid")],
        Control::Time => vec![Value::from("07:30"), Value::from("22:15")],
        Control::Command { .. } => Vec::new(),
    }
    .into_iter()
    .filter(|v| registry::integral(v.clone()) != registry::integral(current.clone()))
    .collect()
}

#[test]
fn every_desktop_setting_reaches_the_shell_and_survives_a_restart() {
    let settings = desktop_settings();
    assert!(settings.len() > 150, "the Desktop group lost its settings? {}", settings.len());
    let mut checked = 0;
    for setting in settings {
        if matches!(setting.control, Control::Command { .. }) {
            continue;
        }
        let mut session = Session::new();
        let before = registry::get(session.shell.get(), setting);
        let candidates = other_values(setting, &before);
        assert!(!candidates.is_empty(), "{}: no other value to try", setting.path);
        let mut applied = None;
        for value in candidates {
            if let Ok(cfg) = session.apply(setting, value.clone()) {
                applied = Some((value, cfg));
                break;
            }
        }
        let (value, live) = applied.unwrap_or_else(|| panic!("{}: Studio can't write any other value", setting.path));
        let expected = registry::integral(value);
        let live_value = registry::integral(registry::get(&live, setting));
        // Sliders may snap / clamp; everything else must match exactly.
        if matches!(setting.control, Control::Slider { .. } | Control::OptionalSlider { .. }) {
            assert_ne!(live_value, registry::integral(before.clone()), "{}: the running shell never saw the change", setting.path);
        } else {
            assert_eq!(live_value, expected, "{}: the running shell has another value", setting.path);
        }
        assert_eq!(registry::integral(registry::get(&session.restarted(), setting)), live_value, "{}: lost after a restart", setting.path);
        checked += 1;
    }
    assert!(checked > 150);
}

#[test]
fn reloading_an_unchanged_file_is_a_no_op() {
    let mut session = Session::new();
    assert!(matches!(session.shell.reload_from_disk().unwrap(), ReloadOutcome::Unchanged));
}

#[test]
fn hot_corners_from_studio_to_shell_and_compositor() {
    use mandara_config::compositor::{ManagedConfig, emit_mango, emit_niri};
    let mut s = Session::new();
    let set = |s: &mut Session, path: &str, v: Value| s.apply(registry::by_path(path).unwrap_or_else(|| panic!("{path} not in Studio")), v).unwrap();
    let emitted = |cfg: &ShellConfig| {
        let eff = cfg.effective_compositor().into_owned();
        let m = ManagedConfig { config: &eff, borders: None, existing_chords: Default::default(), mandara_bin: "mandara".into(), version: None, user_autostarts: false };
        (emit_mango(&m).text, emit_niri(&m).text)
    };

    // Off by default: nothing fires, the compositor keeps its own corner.
    let cfg = s.shell.snapshot();
    assert!(ScreenCorner::ALL.iter().all(|c| !mandara_ui::corners::corner_allowed(&cfg, *c, false)));
    assert!(!emitted(&cfg).0.contains("enable_hotarea"));

    let cfg = set(&mut s, "/hot_corners/enabled", Value::from(true));
    let cfg2 = set(&mut s, "/hot_corners/bottom_left/action", Value::from("launcher"));
    assert!(mandara_ui::corners::corner_allowed(&cfg2, ScreenCorner::BottomLeft, false));
    assert!(mandara_ui::corners::corner_allowed(&cfg, ScreenCorner::TopLeft, false), "default overview corner");
    let (mango, niri) = emitted(&cfg2);
    assert!(mango.contains("enable_hotarea=0"), "{mango}");
    assert!(niri.contains("hot-corners"), "{niri}");

    // Fullscreen apps: only with the switch.
    assert!(!mandara_ui::corners::corner_allowed(&cfg2, ScreenCorner::BottomLeft, true));
    let cfg3 = set(&mut s, "/hot_corners/over_fullscreen", Value::from(true));
    assert!(mandara_ui::corners::corner_allowed(&cfg3, ScreenCorner::BottomLeft, true));

    // A command corner needs its command.
    let cfg4 = set(&mut s, "/hot_corners/bottom_right/action", Value::from("command"));
    assert!(!mandara_ui::corners::corner_allowed(&cfg4, ScreenCorner::BottomRight, false));
    let cfg5 = set(&mut s, "/hot_corners/bottom_right/command", Value::from("notify-send hi"));
    assert!(mandara_ui::corners::corner_allowed(&cfg5, ScreenCorner::BottomRight, false));
    assert_eq!(cfg5.hot_corners.bottom_right.action, CornerAction::Command);

    // Size is clamped by the shell.
    let cfg6 = set(&mut s, "/hot_corners/size", Value::from(24.0));
    assert_eq!(mandara_ui::corners::hot_size(&cfg6), 24.0);

    // Switching hot corners off gives the compositor its corner back.
    let off = set(&mut s, "/hot_corners/enabled", Value::from(false));
    assert!(!emitted(&off).0.contains("enable_hotarea"));
    assert!(ScreenCorner::ALL.iter().all(|c| !mandara_ui::corners::corner_allowed(&off, *c, false)));
    assert_eq!(s.restarted().hot_corners, off.hot_corners);
}

#[test]
fn the_dock_makes_the_launcher_click_only_and_lifts_it() {
    let mut s = Session::new();
    let edge = registry::by_path("/edges/launcher_bottom").expect("launcher edge row");
    let dock_off = s.edit(|c| {
        c.plugins.enabled.retain(|p| p != DOCK_PLUGIN_ID);
        c.edges.launcher_bottom = true;
        c.launcher.look.position = LauncherPosition::BottomCenter;
    });
    assert!(dock_off.launcher_hover_enabled());
    assert_eq!(registry::locked(edge, &dock_off), None);
    let screen = (1920.0, 1080.0);
    let off = mandara_ui::launcher_place::for_config(&dock_off, screen, 400.0, true, None);
    assert!(off.rect.bottom() == screen.1 && off.join != mandara_ui::launcher_place::Join::Card, "a drawer out of the bottom edge");

    // Studio's Dock switch (plugin_switch writes plugins.enabled).
    let dock_on = s.edit(|c| c.plugins.enabled.push(DOCK_PLUGIN_ID.into()));
    assert!(!dock_on.launcher_hover_enabled(), "hover-to-open is off while the dock is on");
    assert_eq!(registry::locked(edge, &dock_on), Some("Click only while the dock is on"));
    assert!(dock_on.edges.launcher_bottom, "the user's own choice is kept for later");
    let on = mandara_ui::launcher_place::for_config(&dock_on, screen, 400.0, true, None);
    assert!(screen.1 - on.rect.bottom() >= dock_on.dock.band() && on.join == mandara_ui::launcher_place::Join::Card, "a floating card above the dock");
    assert!(mandara_ui::plugins::dock::stays_out_for_launcher(&dock_on.dock, true));

    // Survives a restart, and switching the dock off restores hover.
    assert!(!s.restarted().launcher_hover_enabled());
    let back = s.edit(|c| c.plugins.enabled.retain(|p| p != DOCK_PLUGIN_ID));
    assert!(back.launcher_hover_enabled());
}

#[test]
fn launcher_placement_reaches_the_shell_and_survives_a_restart() {
    // Studio's choice list is exactly the config's placements.
    let row = registry::by_path("/launcher/look/position").expect("placement row");
    let Control::Choice(choices) = row.control else { panic!("placement is a choice") };
    assert_eq!(choices.iter().map(|(id, _)| *id).collect::<Vec<_>>(), LauncherPosition::IDS);
    let mut s = Session::new();
    for pos in LauncherPosition::ALL {
        let cfg = s.edit(|c| {
            c.launcher.look.position = pos;
            c.launcher.look.align_to_button = pos != LauncherPosition::TopLeft;
            c.launcher.look.center_width = 900.0;
        });
        assert_eq!(cfg.launcher.look.position, pos);
        assert_eq!(s.restarted().launcher.look.position, pos);
        assert_eq!(s.restarted().launcher.look.align_to_button, pos != LauncherPosition::TopLeft);
    }
    // The center card has its own size.
    let cfg = s.edit(|c| c.launcher.look.position = LauncherPosition::Center);
    assert_eq!(cfg.launcher.look.size().0, 900.0);
    for path in ["/launcher/look/align_to_button", "/launcher/look/height", "/launcher/look/center_width", "/launcher/look/center_height"] {
        assert!(registry::by_path(path).is_some(), "{path} has a Studio row");
    }
}
