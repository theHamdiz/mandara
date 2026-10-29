//! Registry entries for Network & Bluetooth, Input (keyboard, mouse,
//! touchpad, cursor), Applications and the compositor extras (Hyprland
//! layouts / groups, gaps that follow the bar). The bespoke parts of those
//! pages (VPN list, hotspot, default apps, layout picker…) live in
//! `app/{network,input,apps}_page.rs`.

use mandara_i18n::tk;

use super::{Control, Section, Setting, TRI, optional, s};

const BANDS: &[(&str, &str)] = &[("auto", tk("Automatic")), ("ghz2", tk("2.4 GHz (range)")), ("ghz5", tk("5 GHz (speed)"))];
const ACCEL: &[(&str, &str)] = &[("null", tk("Your config")), ("adaptive", tk("Adaptive")), ("flat", tk("Flat"))];
const SCROLL: &[(&str, &str)] = &[("null", tk("Your config")), ("two-finger", tk("Two fingers")), ("edge", tk("Edge")), ("on-button-down", tk("Hold a button")), ("no-scroll", tk("Off"))];
const CLICK: &[(&str, &str)] = &[("null", tk("Your config")), ("button-areas", tk("Button areas")), ("clickfinger", tk("Finger count"))];
const HYPR_LAYOUTS: &[(&str, &str)] = &[("null", tk("Your config")), ("dwindle", tk("Dwindle")), ("master", tk("Master"))];
const HYPR_SPLIT: &[(&str, &str)] = &[("null", tk("Your config")), ("0", tk("Follow the pointer")), ("1", tk("Left / top")), ("2", tk("Right / bottom"))];
const HYPR_ORIENTATION: &[(&str, &str)] = &[("null", tk("Your config")), ("left", tk("Left")), ("right", tk("Right")), ("top", tk("Top")), ("bottom", tk("Bottom")), ("center", tk("Center"))];
/// Niri only — the tri-state without "Off" reads oddly, so it keeps all three.
const NIRI_ONLY: &str = tk("Niri only");
/// Help (and locked note) of the options only Hyprland's drop-in carries.
pub const HYPRLAND_ONLY: &str = tk("Hyprland only");
/// Locked note of options MangoWM and Niri carry but Hyprland doesn't.
const MANGO_NIRI_ONLY: &str = tk("MangoWM and Niri only");

/// Group names (also used by the bespoke pages to pick rows).
pub const KEYBOARD: &str = tk("Keyboard");
pub const MOUSE: &str = tk("Mouse");
pub const TOUCHPAD: &str = tk("Touchpad");
pub const CURSOR: &str = tk("Cursor");
pub const FOCUS: &str = tk("Focus");
pub const VPN: &str = tk("VPN");
pub const HOTSPOT: &str = tk("Hotspot");
pub const BLUETOOTH: &str = tk("Bluetooth");
pub const PREFERRED: &str = tk("Preferred apps");
pub const AUTOSTART: &str = tk("Start at login");
pub const HYPR_LAYOUT: &str = tk("Hyprland layout & groups");

/// The cursor theme and size paths (the gallery and size chips write them).
pub const CURSOR_THEME_PATH: &str = "/compositor/input/cursor/theme";
pub const CURSOR_SIZE_PATH: &str = "/compositor/input/cursor/size";
/// The size slider's span, px (common sizes are 24, 32, 48 and 64).
pub const CURSOR_SIZE_MIN: f64 = 16.0;
pub const CURSOR_SIZE_MAX: f64 = 128.0;

/// Why a cursor option does nothing under the running compositor
/// (`running`: MangoWM, Niri, Hyprland), from `INPUT_OPTIONS`.
pub fn cursor_locked(path: &str, running: Option<mandara_config::compositor::Target>) -> Option<&'static str> {
    use mandara_config::compositor::Target;
    let key = path.strip_prefix("/compositor/input/cursor/").map(|k| format!("cursor.{k}"))?;
    let support = mandara_config::input::INPUT_OPTIONS.iter().find(|(k, _)| *k == key)?.1;
    let index = match running? {
        Target::Mango => 0,
        Target::Niri => 1,
        Target::Hyprland => 2,
    };
    match support {
        _ if support[index] => None,
        [false, false, true] => Some(HYPRLAND_ONLY),
        [true, true, false] => Some(MANGO_NIRI_ONLY),
        _ => Some(tk("Not supported by this compositor")),
    }
}

pub fn entries() -> Vec<Setting> {
    use Control::*;
    use Section::*;
    vec![
        // Network & Bluetooth
        s(Network, VPN, "/network/vpn_notifications", tk("VPN notifications"), tk("A notification when a VPN connects or drops"), Toggle),
        s(Network, HOTSPOT, "/network/hotspot_ssid", tk("Hotspot name"), tk("Empty uses this computer's name"), Text { placeholder: tk("e.g. Mandara hotspot") }),
        s(Network, HOTSPOT, "/network/hotspot_band", tk("Band"), tk("2.4 GHz reaches farther and every device; 5 GHz is faster nearby"), Choice(BANDS)),
        s(Network, BLUETOOTH, "/bluetooth/pairing_agent", tk("Pairing dialog"), tk("Mandara answers pairing codes and connection requests (restart the shell to apply)"), Toggle),
        s(Network, BLUETOOTH, "/bluetooth/auto_connect", tk("Reconnect trusted devices"), tk("When Bluetooth turns on, at login and after sleep"), Toggle),
        s(Network, BLUETOOTH, "/bluetooth/connect_notifications", tk("Connection notifications"), tk("A notification when a device connects or disconnects"), Toggle),
        // Input: keyboard
        s(Input, KEYBOARD, "/compositor/input/keyboard/repeat_delay", tk("Repeat delay"), tk("How long a key is held before it repeats"), optional(100.0, 1500.0, 10.0, " ms", 600.0)),
        s(Input, KEYBOARD, "/compositor/input/keyboard/repeat_rate", tk("Repeat rate"), tk("Characters per second while a key is held"), optional(5.0, 80.0, 1.0, " /s", 25.0)),
        s(Input, KEYBOARD, "/compositor/input/keyboard/numlock", tk("Num Lock on at login"), "", Choice(TRI)),
        s(Input, KEYBOARD, "/compositor/input/keyboard/per_window_layout", tk("Remember the layout per window"), NIRI_ONLY, Choice(TRI)),
        s(Input, KEYBOARD, "/compositor/input/keyboard/options", tk("Other xkb options"), tk("Comma-separated, e.g. ctrl:nocaps,compose:ralt"), OptionalText { placeholder: tk("Your config") }),
        s(Input, KEYBOARD, "/compositor/input/keyboard/model", tk("Keyboard model"), tk("Rarely needed, e.g. pc105"), OptionalText { placeholder: tk("Your config") }),
        // Input: mouse
        s(Input, MOUSE, "/compositor/input/mouse/speed", tk("Pointer speed"), tk("-1 is slowest, 1 fastest"), optional(-1.0, 1.0, 0.05, "", 0.0)),
        s(Input, MOUSE, "/compositor/input/mouse/accel_profile", tk("Acceleration"), tk("Flat moves the pointer exactly as far as the mouse"), Choice(ACCEL)),
        s(Input, MOUSE, "/compositor/input/mouse/natural_scroll", tk("Natural scrolling"), tk("Content follows the wheel, like a touchscreen"), Choice(TRI)),
        s(Input, MOUSE, "/compositor/input/mouse/left_handed", tk("Left-handed"), tk("Swap the left and right buttons"), Choice(TRI)),
        s(Input, MOUSE, "/compositor/input/mouse/scroll_factor", tk("Scroll speed"), "", optional(0.1, 5.0, 0.1, "×", 1.0)),
        s(Input, MOUSE, "/compositor/input/mouse/scroll_method", tk("Scroll method"), "", Choice(SCROLL)),
        s(Input, MOUSE, "/compositor/input/mouse/middle_emulation", tk("Middle-click emulation"), tk("Both buttons together click the middle one (MangoWM, Niri)"), Choice(TRI)),
        // Input: touchpad
        s(Input, TOUCHPAD, "/compositor/input/touchpad/tap_to_click", tk("Tap to click"), "", Choice(TRI)),
        s(Input, TOUCHPAD, "/compositor/input/touchpad/natural_scroll", tk("Natural scrolling"), tk("Content follows your fingers"), Choice(TRI)),
        s(Input, TOUCHPAD, "/compositor/input/touchpad/disable_while_typing", tk("Disable while typing"), "", Choice(TRI)),
        s(Input, TOUCHPAD, "/compositor/input/touchpad/tap_and_drag", tk("Tap and drag"), tk("Tap, then slide to drag"), Choice(TRI)),
        s(Input, TOUCHPAD, "/compositor/input/touchpad/drag_lock", tk("Drag lock"), tk("A drag continues briefly after lifting the finger"), Choice(TRI)),
        s(Input, TOUCHPAD, "/compositor/input/touchpad/speed", tk("Pointer speed"), tk("MangoWM, Niri"), optional(-1.0, 1.0, 0.05, "", 0.0)),
        s(Input, TOUCHPAD, "/compositor/input/touchpad/accel_profile", tk("Acceleration"), tk("MangoWM, Niri"), Choice(ACCEL)),
        s(Input, TOUCHPAD, "/compositor/input/touchpad/scroll_method", tk("Scroll method"), tk("MangoWM, Niri"), Choice(SCROLL)),
        s(Input, TOUCHPAD, "/compositor/input/touchpad/click_method", tk("Right click"), tk("Bottom-right corner, or two fingers"), Choice(CLICK)),
        s(Input, TOUCHPAD, "/compositor/input/touchpad/scroll_factor", tk("Scroll speed"), "", optional(0.1, 5.0, 0.1, "×", 1.0)),
        s(Input, TOUCHPAD, "/compositor/input/touchpad/middle_emulation", tk("Middle-click emulation"), "", Choice(TRI)),
        s(Input, TOUCHPAD, "/compositor/input/touchpad/left_handed", tk("Left-handed"), tk("MangoWM, Niri"), Choice(TRI)),
        s(Input, TOUCHPAD, "/compositor/input/touchpad/disable_with_mouse", tk("Off while a mouse is plugged in"), tk("MangoWM, Niri"), Choice(TRI)),
        // Themes → Cursor (the theme gallery is `app/cursor_page.rs`; the
        // theme row is for search and the command line)
        s(Themes, CURSOR, CURSOR_THEME_PATH, tk("Cursor theme"), tk("The XCursor theme name, e.g. Bibata-Modern-Ice; pick one from the gallery above"), OptionalText { placeholder: tk("Your config") }),
        s(Themes, CURSOR, CURSOR_SIZE_PATH, tk("Cursor size"), tk("In logical pixels; apps and the compositor scale it on HiDPI screens"), optional(CURSOR_SIZE_MIN, CURSOR_SIZE_MAX, 2.0, " px", 24.0)),
        s(Themes, CURSOR, "/compositor/input/cursor/hide_when_typing", tk("Hide while typing"), tk("The cursor disappears on a key press and returns when the pointer moves"), Choice(TRI)),
        s(Themes, CURSOR, "/compositor/input/cursor/hide_after_secs", tk("Hide when idle"), tk("Seconds without motion before the cursor hides"), optional(1.0, 60.0, 1.0, " s", 5.0)),
        s(Themes, CURSOR, "/compositor/input/cursor/no_hardware_cursors", tk("Software cursor"), tk("Draws the cursor in software: fixes an invisible or flickering cursor on some GPUs (Hyprland)"), Choice(TRI)),
        s(Themes, CURSOR, "/compositor/input/cursor/hide_on_touch", tk("Hide on touch"), tk("Hides the cursor while you use a touchscreen (Hyprland)"), Choice(TRI)),
        // Input: focus
        s(Input, FOCUS, "/compositor/input/focus_follows_mouse", tk("Focus follows the pointer"), tk("Windows get focus when the pointer enters them"), Choice(TRI)),
        // Applications
        s(Applications, AUTOSTART, "/apps/run_autostart", tk("Mandara starts these at login"), tk("For sessions that don't start them (MangoWM, Niri, Hyprland without uwsm). Skipped when systemd's xdg-autostart does; leave off if your compositor config starts the same apps"), Toggle),
        s(Applications, PREFERRED, "/apps/terminal", tk("Terminal"), tk("Runs updates and the resource monitor; empty detects one"), Text { placeholder: tk("Detect automatically") }),
        s(Applications, PREFERRED, "/apps/file_manager", tk("File manager"), tk("Opens drives and folders from Mandara; empty uses your default"), Text { placeholder: tk("Detect automatically") }),
        s(Applications, PREFERRED, "/apps/browser", tk("Browser"), tk("Opens links from Mandara; empty uses your default"), Text { placeholder: tk("Detect automatically") }),
        // Compositor extras
        s(Compositor, tk("Window look"), "/compositor/look/gaps_follow_bar", tk("Gaps follow the bar"), tk("Window gaps match the bar's side spacing and update with it"), Toggle),
        s(Compositor, HYPR_LAYOUT, "/compositor/hyprland/layout", tk("Tiling layout"), "general:layout", Choice(HYPR_LAYOUTS)),
        s(Compositor, HYPR_LAYOUT, "/compositor/hyprland/dwindle_preserve_split", tk("Keep split direction"), "dwindle:preserve_split", Choice(TRI)),
        s(Compositor, HYPR_LAYOUT, "/compositor/hyprland/dwindle_smart_split", tk("Split toward the pointer"), "dwindle:smart_split", Choice(TRI)),
        s(Compositor, HYPR_LAYOUT, "/compositor/hyprland/dwindle_force_split", tk("New window side"), "dwindle:force_split", Choice(HYPR_SPLIT)),
        s(Compositor, HYPR_LAYOUT, "/compositor/hyprland/master_orientation", tk("Master area"), "master:orientation", Choice(HYPR_ORIENTATION)),
        s(Compositor, HYPR_LAYOUT, "/compositor/hyprland/master_mfact", tk("Master size"), "master:mfact", optional(0.1, 0.9, 0.05, "", 0.55)),
        s(Compositor, HYPR_LAYOUT, "/compositor/hyprland/master_new_on_top", tk("New windows become master"), "master:new_on_top", Choice(TRI)),
        s(Compositor, HYPR_LAYOUT, "/compositor/hyprland/resize_on_border", tk("Resize by dragging borders"), "general:resize_on_border", Choice(TRI)),
        s(Compositor, HYPR_LAYOUT, "/compositor/hyprland/border_grab_area", tk("Border grab area"), "general:extend_border_grab_area", optional(0.0, 40.0, 1.0, " px", 15.0)),
        s(Compositor, HYPR_LAYOUT, "/compositor/hyprland/groupbar", tk("Group tab bar"), "group:groupbar:enabled", Choice(TRI)),
        s(Compositor, HYPR_LAYOUT, "/compositor/hyprland/groupbar_titles", tk("Titles on group tabs"), "group:groupbar:render_titles", Choice(TRI)),
        s(Compositor, HYPR_LAYOUT, "/compositor/hyprland/groupbar_gradients", tk("Gradient group tabs"), "group:groupbar:gradients", Choice(TRI)),
        s(Compositor, HYPR_LAYOUT, "/compositor/hyprland/groupbar_height", tk("Group tab height"), "group:groupbar:height", optional(8.0, 40.0, 1.0, " px", 14.0)),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::{get, set};
    use mandara_config::ShellConfig;
    use serde_json::Value;

    /// Every W6 path resolves, round-trips, and its optional sliders accept
    /// a value inside their range.
    #[test]
    fn system_entries_round_trip() {
        let cfg = ShellConfig::default();
        for s in entries() {
            let v = get(&cfg, &s);
            let back = set(&cfg, &s, v.clone()).unwrap_or_else(|e| panic!("{}: {e}", s.path));
            assert_eq!(get(&back, &s), v, "{}", s.path);
            if let Control::OptionalSlider { default, .. } = s.control {
                let with = set(&cfg, &s, Value::from(default)).unwrap_or_else(|e| panic!("{}: {e}", s.path));
                assert!(!get(&with, &s).is_null(), "{}", s.path);
            }
            if let Control::Choice(choices) = s.control {
                for (raw, _) in choices {
                    set(&cfg, &s, crate::registry::choice_value(raw)).unwrap_or_else(|e| panic!("{} = {raw}: {e}", s.path));
                }
            }
        }
        let speed = entries().into_iter().find(|s| s.path == "/compositor/input/touchpad/speed").unwrap();
        let next = set(&cfg, &speed, Value::from(0.3)).unwrap();
        assert_eq!(next.compositor.input.touchpad.pointer.speed, Some(0.3), "flattened touchpad fields");
    }

    /// The `INPUT_OPTIONS` key a Studio → Input path belongs to.
    fn option_key(path: &str) -> String {
        let rest = path.trim_start_matches("/compositor/input/");
        match rest {
            "keyboard/repeat_delay" | "keyboard/repeat_rate" => "keyboard.repeat".into(),
            "keyboard/options" | "keyboard/model" => "keyboard.layouts".into(),
            other => other.replace('/', "."),
        }
    }

    /// A value that turns the control on: the slider's default, a choice
    /// other than "Your config", or a sample text.
    fn engaged(s: &Setting) -> Value {
        match s.control {
            Control::OptionalSlider { default, .. } => Value::from(default),
            Control::Choice(choices) => crate::registry::choice_value(choices.iter().map(|(raw, _)| *raw).find(|raw| *raw != "null").expect("a real choice")),
            Control::OptionalText { .. } => Value::from(if s.path.ends_with("model") {
                "pc105"
            } else if s.path == CURSOR_THEME_PATH {
                "Bibata-Modern-Ice"
            } else {
                "compose:ralt"
            }),
            _ => panic!("{}: unexpected control", s.path),
        }
    }

    /// Studio → Input, end to end: each control written through the
    /// registry lands in the drop-in of exactly the compositors its help
    /// text names (and that `INPUT_OPTIONS` lists), and "Your config"
    /// writes nothing.
    #[test]
    fn every_input_control_reaches_the_right_drop_ins() {
        use mandara_config::input::{INPUT_OPTIONS, emit_hyprland_conf, emit_mango, emit_niri};
        let base = ShellConfig::default();
        let has_settings = |text: &str| text.lines().map(str::trim).any(|l| !l.is_empty() && !l.starts_with('#') && !l.starts_with("//") && l != "}" && !l.ends_with('{'));
        for s in entries().into_iter().filter(|s| s.path.starts_with("/compositor/input/")) {
            let key = option_key(s.path);
            let support = INPUT_OPTIONS.iter().find(|(k, _)| *k == key).unwrap_or_else(|| panic!("{} → {key} not in INPUT_OPTIONS", s.path)).1;
            let cfg = set(&base, &s, engaged(&s)).unwrap_or_else(|e| panic!("{}: {e}", s.path));
            let input = &cfg.compositor.input;
            let got = [has_settings(&emit_mango(input)), has_settings(&emit_niri(input)), has_settings(&emit_hyprland_conf(input))];
            assert_eq!(got, support, "{} ({key}): [MangoWM, Niri, Hyprland]", s.path);
            // The help text tells the truth about partial support.
            let help = mandara_i18n::tr(s.help);
            match support {
                [true, true, true] => {}
                [true, true, false] => assert!(s.help.contains("MangoWM, Niri"), "{}: help should say MangoWM, Niri: {help}", s.path),
                [false, true, false] => assert_eq!(s.help, NIRI_ONLY, "{}", s.path),
                [false, false, true] => assert!(s.help.contains("(Hyprland)"), "{}: help should say (Hyprland): {help}", s.path),
                other => panic!("{}: no help wording for support {other:?}", s.path),
            }
            // Back to "Your config": nothing is written again.
            let cleared = set(&cfg, &s, Value::Null).unwrap();
            assert!(cleared.compositor.input.is_empty(), "{}: null clears", s.path);
        }
    }

    #[test]
    fn cursor_options_lock_where_they_do_nothing() {
        use mandara_config::compositor::Target;
        let hw = "/compositor/input/cursor/no_hardware_cursors";
        assert_eq!(cursor_locked(hw, Some(Target::Mango)), Some(HYPRLAND_ONLY));
        assert_eq!(cursor_locked(hw, Some(Target::Niri)), Some(HYPRLAND_ONLY));
        assert_eq!(cursor_locked(hw, Some(Target::Hyprland)), None);
        assert_eq!(cursor_locked(hw, None), None, "no compositor: saved for later, not locked");
        for p in [CURSOR_THEME_PATH, CURSOR_SIZE_PATH, "/compositor/input/cursor/hide_when_typing", "/compositor/input/cursor/hide_after_secs"] {
            for t in [Target::Mango, Target::Niri, Target::Hyprland] {
                assert_eq!(cursor_locked(p, Some(t)), None, "{p} works everywhere");
            }
        }
        assert_eq!(cursor_locked("/bar/height", Some(Target::Niri)), None);
        // Every cursor row lives on Themes, none on Input.
        assert!(entries().iter().filter(|s| s.path.starts_with("/compositor/input/cursor/")).all(|s| s.section == Section::Themes && s.group == CURSOR));
    }

    /// Through the real store: a Studio edit is saved to TOML, read back
    /// by the shell, and the drop-in follows (MangoWM syntax checked by
    /// `mango -p` when it's installed).
    #[test]
    fn input_edits_survive_the_config_file_and_reach_mango() {
        use mandara_config::ConfigStore;
        use mandara_config::compositor::{ManagedConfig, emit_mango};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut store = ConfigStore::open(&path).unwrap();
        for (p, v) in [("/compositor/input/keyboard/repeat_rate", Value::from(40.0)), ("/compositor/input/touchpad/tap_to_click", Value::from(true)), ("/compositor/input/touchpad/disable_with_mouse", Value::from(true)), ("/compositor/input/focus_follows_mouse", Value::from(false))] {
            let s = entries().into_iter().find(|s| s.path == p).unwrap();
            let next = set(store.get(), &s, v).unwrap();
            store.replace(next);
        }
        store.flush().unwrap();
        let shell = ConfigStore::open(&path).unwrap();
        let input = &shell.get().compositor.input;
        assert_eq!((input.keyboard.repeat_rate, input.touchpad.tap_to_click, input.focus_follows_mouse), (Some(40), Some(true), Some(false)));
        let compositor = shell.get().compositor.clone();
        let text = emit_mango(&ManagedConfig { config: &compositor, borders: None, existing_chords: Default::default(), mandara_bin: "mandara".into(), version: None, user_autostarts: false }).text;
        for needle in ["repeat_rate=40", "tap_to_click=1", "trackpad_send_events_mode=2", "sloppy_focus=0"] {
            assert!(text.lines().any(|l| l == needle), "missing {needle}\n{text}");
        }
        if std::process::Command::new("mango").arg("-v").output().is_ok() {
            let file = dir.path().join("mango.conf");
            std::fs::write(&file, &text).unwrap();
            let out = std::process::Command::new("mango").arg("-p").arg("-c").arg(&file).output().unwrap();
            let report = String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr);
            assert!(!report.contains("ERROR"), "mango -p:\n{report}");
        }
    }
}
