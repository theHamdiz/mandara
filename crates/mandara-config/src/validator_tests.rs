//! Every emitter's output fed to the compositor's own checker, the way the
//! user's config loads it: a main config that includes / sources the
//! drop-in. Each test is skipped when that compositor isn't installed
//! (`niri validate`, `Hyprland --verify-config`, `sway --validate`;
//! MangoWM's `mango -p` lives next to its emitter tests). CI's
//! `compositor-checkers` job installs sway and runs the Sway ones.

use std::path::Path;
use std::process::Command;

use crate::compositor::*;
use crate::outputs::{OutputConfig, Transform};

/// Absolute binary path written into autostart lines and binds.
const BIN: &str = "/home/u/.local/bin/mandara";

fn installed(program: &str, version_flag: &str) -> bool {
    Command::new(program).arg(version_flag).output().is_ok_and(|o| o.status.success())
}

fn borders() -> BorderColors {
    BorderColors { active: "#c8793c".into(), active_end: "#e0a060".into(), inactive: "#2a2420".into(), urgent: "#ff4f4f".into() }
}

/// Representative configs: defaults, autostart, the Alt+Tab switcher and
/// other default binds, Studio's window rules and look, input, displays.
fn configs() -> Vec<(&'static str, CompositorConfig, bool)> {
    let defaults = CompositorConfig::default();
    let autostart = CompositorConfig { autostart: true, ..Default::default() };
    let mut binds = CompositorConfig { autostart: true, ..Default::default() };
    binds.keybinds.push(Keybind::new(&["super"], "1", BindAction::Mandara("workspace 1".into())));
    binds.keybinds.push(Keybind::new(&["super", "ctrl"], "Return", BindAction::Spawn("foot -e btop".into())));
    binds.keybinds.push(Keybind::new(&["super"], "comma", BindAction::Mandara("studio".into())));
    let mut full = CompositorConfig {
        autostart: true,
        border_width: Some(3),
        disable_builtin_hot_corners: true,
        input: crate::input::tests::full(),
        look: CompositorLook {
            gaps_inner: Some(12),
            gaps_outer: Some(10),
            rounding: Some(14),
            blur: Some(true),
            shadows: Some(false),
            animations: Some(false),
            active_opacity: Some(1.0),
            inactive_opacity: Some(0.9),
            gaps_follow_bar: false,
            windows: crate::window_motion::WindowMotion::default(),
        },
        ..Default::default()
    };
    full.window_rules.push(WindowRule {
        app_id: Some("^pavucontrol$".into()),
        title: Some("Volume".into()),
        floating: Some(true),
        global: Some(true),
        overlay: Some(false),
        focused_opacity: Some(0.95),
        unfocused_opacity: Some(0.8),
        no_blur: Some(true),
        no_shadow: Some(true),
        confine_pointer: Some(false),
        force_tiled_state: Some(true),
    });
    full.window_rules.push(WindowRule { app_id: Some("^org\\.gnome\\.Calculator$".into()), floating: Some(true), ..Default::default() });
    full.niri = NiriOptions {
        default_column_width: Some(0.5),
        preset_column_widths: vec![1.0 / 3.0, 0.5, 2.0 / 3.0],
        center_focused_column: Some("on-overflow".into()),
        always_center_single_column: Some(true),
        outputs: vec![OutputRule { name: "HDMI-A-1".into(), scale: Some(2.0), x: Some(0), y: Some(0) }],
    };
    full.hyprland = HyprlandOptions {
        blur_size: Some(8),
        blur_passes: Some(3),
        shadow_range: Some(20),
        dim_inactive: Some(true),
        dim_strength: Some(0.1),
        layout: Some("master".into()),
        dwindle_preserve_split: Some(true),
        dwindle_smart_split: Some(false),
        dwindle_force_split: Some(2),
        master_orientation: Some("left".into()),
        master_mfact: Some(0.55),
        master_new_on_top: Some(true),
        resize_on_border: Some(true),
        border_grab_area: Some(15),
        groupbar: Some(true),
        groupbar_titles: Some(false),
        groupbar_gradients: Some(false),
        groupbar_height: Some(16),
    };
    full.outputs = vec![
        OutputConfig {
            name: "DP-1".into(),
            make: "Dell Inc.".into(),
            model: "U2720Q".into(),
            serial: "ABC123".into(),
            enabled: true,
            width: Some(3840),
            height: Some(2160),
            refresh_mhz: Some(59_997),
            scale: Some(1.5),
            transform: Some(Transform::Rotate90),
            x: Some(2560),
            y: Some(0),
            ..Default::default()
        },
        OutputConfig { name: "eDP-1".into(), enabled: false, ..Default::default() },
    ];
    vec![("defaults", defaults, false), ("autostart", autostart, false), ("binds", binds, true), ("full", full, true)]
}

fn managed(config: &CompositorConfig, colors: bool, version: Option<(u32, u32)>) -> ManagedConfig<'_> {
    ManagedConfig { config, borders: colors.then(borders), existing_chords: Default::default(), mandara_bin: BIN.into(), version, user_autostarts: false }
}

fn run(cmd: &mut Command) -> (bool, String) {
    let out = cmd.output().expect("validator runs");
    (out.status.success(), String::from_utf8_lossy(&out.stdout).to_string() + &String::from_utf8_lossy(&out.stderr))
}

/// A user's niri config with its own binds and an output, ending in
/// Mandara's include line (as `mandara setup` appends it).
fn niri_main(dir: &Path, include: &str) -> std::path::PathBuf {
    let main = dir.join("config.kdl");
    let text = format!(
        "input {{\n    keyboard {{\n        xkb {{\n            layout \"us\"\n        }}\n    }}\n}}\n\noutput \"eDP-1\" {{\n    scale 1.25\n}}\n\nbinds {{\n    Mod+T {{ spawn \"alacritty\"; }}\n    Mod+Q {{ close-window; }}\n}}\n\n{include}\n",
    );
    std::fs::write(&main, text).unwrap();
    main
}

#[test]
fn niri_validates_every_dropin() {
    if !installed("niri", "--version") {
        return; // Niri isn't installed here.
    }
    let dir = tempfile::tempdir().unwrap();
    for version in [Some((25, 11)), Some((26, 4)), None] {
        for (name, cfg, colors) in configs() {
            let dropin = dir.path().join("generated/niri.kdl");
            std::fs::create_dir_all(dropin.parent().unwrap()).unwrap();
            let text = emit_niri(&managed(&cfg, colors, version)).text;
            std::fs::write(&dropin, &text).unwrap();
            let include = format!("include {:?}", dropin.display().to_string());
            let main = niri_main(dir.path(), &include);
            let (ok, log) = run(Command::new("niri").arg("validate").arg("-c").arg(&main));
            assert!(ok, "niri rejected the {name} drop-in (version {version:?}):\n{log}\n--- drop-in ---\n{text}");
            assert!(text.contains("background-color \"transparent\""), "the backdrop wallpaper must show through workspaces");
        }
    }
}

/// The exact line `mandara setup` appends, including `~/` and `optional`
/// forms on niri versions that support them.
#[test]
fn niri_accepts_the_include_line() {
    if !installed("niri", "--version") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let dropin = dir.path().join("niri.kdl");
    std::fs::write(&dropin, emit_niri(&managed(&CompositorConfig { autostart: true, ..Default::default() }, false, None)).text).unwrap();
    let line = niri_include_line(&dropin.display().to_string(), crate::compositor::installed_version(Target::Niri));
    let main = niri_main(dir.path(), &line);
    let (ok, log) = run(Command::new("niri").arg("validate").arg("-c").arg(&main));
    assert!(ok, "{line}\n{log}");
}

#[test]
fn hyprland_validates_every_dropin() {
    if !installed("Hyprland", "--version") {
        return; // Hyprland isn't installed here.
    }
    let version = installed_version(Target::Hyprland);
    let lua = version.is_none_or(hyprland_reads_lua);
    let dir = tempfile::tempdir().unwrap();
    for (name, cfg, colors) in configs() {
        let m = managed(&cfg, colors, version);
        let (dropin, main, text, main_text) = if lua {
            let text = emit_hyprland_lua(&m).text;
            let dropin = dir.path().join("mandara.lua");
            (dropin.clone(), dir.path().join("hyprland.lua"), text, format!("hl.config({{ general = {{ gaps_in = 4 }} }})\n{}\n", hyprland_lua_include(&dropin.display().to_string())))
        } else {
            let text = emit_hyprland_conf(&m).text;
            let dropin = dir.path().join("mandara.conf");
            (dropin.clone(), dir.path().join("hyprland.conf"), text, format!("general {{\n    gaps_in = 4\n}}\nsource = {}\n", dropin.display()))
        };
        std::fs::write(&dropin, &text).unwrap();
        std::fs::write(&main, main_text).unwrap();
        let (ok, log) = run(Command::new("Hyprland").arg("--verify-config").arg("-c").arg(&main));
        let clean = ok && !log.to_lowercase().contains("error");
        assert!(clean, "Hyprland rejected the {name} drop-in:\n{log}\n--- drop-in ---\n{text}");
    }
}


/// Niri keeps the wallpaper in the Overview backdrop; without see-through
/// workspaces it would only show in the Overview (found in a nested niri
/// 26.04 session: a plain grey desktop).
#[test]
fn niri_wallpaper_shows_outside_the_overview() {
    for (_, cfg, colors) in configs() {
        let text = emit_niri(&managed(&cfg, colors, None)).text;
        assert!(text.contains("place-within-backdrop true") && text.contains("background-color \"transparent\""), "{text}");
        assert_eq!(text.matches("\nlayout {").count(), 1, "niri rejects a second `layout` node in one file:\n{text}");
    }
}

/// Hyprland 0.57 option names and types the Lua drop-in must use (checked
/// against its sources: dashes are underscores, `drag_lock` is an int).
#[test]
fn hyprland_lua_uses_057_option_names() {
    let (_, cfg, colors) = configs().pop().unwrap();
    let lua = emit_hyprland_lua(&managed(&cfg, colors, Some((0, 57)))).text;
    assert!(!lua.contains("[\"") && lua.contains("tap_to_click = true") && lua.contains("drag_lock = 1"), "{lua}");
    assert!(lua.contains("pcall(hl.on, \"hyprland.start\", function() hl.exec_cmd(\"/home/u/.local/bin/mandara\") end)"));
}

/// Sway's own check, through the config doctor (`sway --validate
/// --verbose`, headless backend, private runtime dir): `None` when sway
/// isn't installed, else every problem it reports.
fn sway_problems(main: &Path, home: &Path) -> Option<Vec<String>> {
    let env = crate::doctor::CheckEnv { home: Some(home.to_path_buf()), ..Default::default() };
    let report = crate::doctor::check_with(Target::Sway, main, &env);
    if report.status == crate::doctor::CheckerStatus::NotInstalled {
        return None;
    }
    assert_eq!(report.status, crate::doctor::CheckerStatus::Ran, "{report:#?}");
    Some(report.diagnostics.iter().map(|d| format!("{}: {}", d.place().unwrap_or_default(), d.message)).collect())
}

/// A user's sway config with its own binds and an output, ending in
/// Mandara's include line (as `mandara setup` appends it).
fn sway_main(dir: &Path, dropin: &Path) -> std::path::PathBuf {
    let main = dir.join("config");
    let text = format!("set $mod Mod4\noutput eDP-1 scale 1.25\nbindsym $mod+t exec alacritty\nbindsym $mod+q kill\ninput type:keyboard xkb_layout us\n\n{}\n", sway_include_line(&dropin.display().to_string()));
    std::fs::write(&main, text).unwrap();
    main
}

/// Every Sway drop-in (defaults, autostart, binds, the full Studio set with
/// input and displays) passes `sway --validate` through the include line,
/// without an error or an overwritten bind; the include works from a
/// folder with a space in its name too.
#[test]
fn sway_validates_every_dropin() {
    let dir = tempfile::tempdir().unwrap();
    let generated = dir.path().join("mandara config/generated");
    std::fs::create_dir_all(&generated).unwrap();
    for (name, cfg, colors) in configs() {
        let text = emit_sway(&managed(&cfg, colors, None)).text;
        assert!(!text.contains("\n\n\n"), "{name}: tidy output\n{text}");
        let dropin = generated.join("sway.conf");
        std::fs::write(&dropin, &text).unwrap();
        let main = sway_main(dir.path(), &dropin);
        let Some(problems) = sway_problems(&main, dir.path()) else {
            eprintln!("sway isn't installed; skipped");
            return;
        };
        assert!(problems.is_empty(), "sway rejected the {name} drop-in:\n{}\n--- drop-in ---\n{text}", problems.join("\n"));
    }
}

/// River has no checker: every River command for every representative
/// config (with and without colours) goes to a headless River through
/// riverctl and must be accepted (see `river_check`).
#[test]
fn river_accepts_every_command_set() {
    // One River session, as the shell syncs: each set takes the previous
    // one's binds and rules back first.
    let mut all = Vec::new();
    let mut previous: Vec<Vec<String>> = Vec::new();
    for (_, cfg, colors) in configs() {
        let (commands, _) = river_commands(&managed(&cfg, colors, None));
        all.extend(river_undo(&previous));
        all.extend(commands.clone());
        previous = commands;
    }
    if let Some(failures) = crate::river_check::run_commands(&all) {
        assert!(failures.is_empty(), "River rejected:\n{}", failures.join("\n"));
    }
}

/// A user's Umbriel config with its own binds and an output, and Mandara's
/// include added the way `mandara setup` adds it.
fn umbriel_main(dir: &Path, dropin: &Path) -> std::path::PathBuf {
    let main = dir.join("config.toml");
    let text = "[general]\nmod_key = \"Super\"\n\n[output.eDP-1]\nscale = 1.25\n\n[keybinds]\n\"Mod+T\" = \"spawn alacritty\"\n\"Mod+Q\" = \"window-close\"\n";
    let with_include = umbriel_add_include(text, &dropin.display().to_string()).unwrap().expect("include added");
    std::fs::write(&main, with_include).unwrap();
    main
}

/// Every Umbriel drop-in is TOML whose rule lists and binds have the shape
/// Umbriel documents; Umbriel's own checker accepts the drop-in through the
/// include when it is installed.
#[test]
fn umbriel_validates_every_dropin() {
    let checker = installed("umbriel", "--version");
    let dir = tempfile::tempdir().unwrap();
    for (name, cfg, colors) in configs() {
        let text = emit_umbriel(&managed(&cfg, colors, None)).text;
        let doc: toml::Table = text.parse().unwrap_or_else(|e| panic!("{name}: invalid TOML: {e}\n{text}"));
        let rules = doc.get("layer_rule").and_then(toml::Value::as_array).expect("layer rules");
        assert!(rules.iter().all(|r| r.get("match").and_then(|m| m.get("namespace")).is_some()), "{name}: every layer rule matches a namespace\n{text}");
        if let Some(binds) = doc.get("keybinds").and_then(toml::Value::as_table) {
            assert!(binds.values().all(|v| v.as_str().is_some_and(|a| a.starts_with("spawn "))), "{name}: binds run `spawn …`\n{text}");
        }
        if !checker {
            continue; // Umbriel isn't installed here.
        }
        let dropin = dir.path().join("umbriel.toml");
        std::fs::write(&dropin, &text).unwrap();
        let main = umbriel_main(dir.path(), &dropin);
        let (ok, log) = run(Command::new("umbriel").arg("config").arg("validate").arg("-c").arg(&main));
        assert!(ok, "umbriel rejected the {name} drop-in:\n{log}\n--- drop-in ---\n{text}");
    }
}

/// Every window animation style in the Hyprland format this Hyprland
/// reads, beside a full config: Hyprland's own `--verify-config` accepts
/// each (both formats when `MANDARA_HYPR_BOTH` is set and it reads both).
#[test]
fn hyprland_validates_every_window_animation() {
    use crate::window_motion::{WindowMotion, WindowStyle};
    if !installed("Hyprland", "--version") {
        return; // Hyprland isn't installed here.
    }
    let version = installed_version(Target::Hyprland);
    let lua = version.is_none_or(hyprland_reads_lua);
    let dir = tempfile::tempdir().unwrap();
    for style in WindowStyle::ALL {
        for speed in [0.25, 1.0, 3.0] {
            let (_, mut cfg, colors) = configs().pop().unwrap();
            cfg.look.animations = Some(true);
            cfg.look.windows = WindowMotion { style, speed };
            let m = managed(&cfg, colors, version);
            let (dropin, main, text, main_text) = if lua {
                let text = emit_hyprland_lua(&m).text;
                let dropin = dir.path().join("mandara.lua");
                (dropin.clone(), dir.path().join("hyprland.lua"), text, format!("hl.config({{ general = {{ gaps_in = 4 }} }})\n{}\n", hyprland_lua_include(&dropin.display().to_string())))
            } else {
                let text = emit_hyprland_conf(&m).text;
                let dropin = dir.path().join("mandara.conf");
                (dropin.clone(), dir.path().join("hyprland.conf"), text, format!("general {{\n    gaps_in = 4\n}}\nsource = {}\n", dropin.display()))
            };
            // The animation calls without their `pcall`, so a mistake in
            // them is an error Hyprland reports, not one `pcall` swallows.
            let text = text.replace("pcall(hl.curve, ", "hl.curve(").replace("pcall(hl.animation, ", "hl.animation(");
            assert_eq!(text.contains("windowsIn"), lua && style != WindowStyle::Compositor, "{text}");
            std::fs::write(&dropin, &text).unwrap();
            std::fs::write(&main, main_text).unwrap();
            let (ok, log) = run(Command::new("Hyprland").arg("--verify-config").arg("-c").arg(&main));
            let clean = ok && !log.to_lowercase().contains("error");
            assert!(clean, "Hyprland rejected window style {style:?}:\n{log}\n--- drop-in ---\n{text}");
        }
    }
}
