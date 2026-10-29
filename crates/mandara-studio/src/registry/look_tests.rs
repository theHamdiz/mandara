//! Look & feel, end to end: every Studio control of Themes, Wallpaper,
//! Typography and Motion writes its config key, survives a save
//! and a restart (through `ConfigStore`), reads back the same through the
//! JSON pointers `mandara msg config get` uses, and drives its consumer
//! (palette, app themes, springs, fonts, wallpaper choice) as expected.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use mandara_config::{ConfigStore, ShellConfig};
use mandara_theme::palette::HC_MIN_CONTRAST;
use mandara_theme::{SchemeVariant, ThemeEngine, ThemeSource};
use serde_json::Value;

use super::*;

const LOOK: [Section; 4] = [Section::Themes, Section::Wallpaper, Section::Typography, Section::Motion];
/// A wallpaper the scheme settings can point at (never read by these tests).
const WALLPAPER: &str = "/srv/walls/corniche.jpg";
/// How far gamut mapping may bend a rotated hue, degrees.
const HUE_TOLERANCE_DEG: f32 = 8.0;
/// Number tolerance for values that pass through `f32` config fields.
const F32_EPSILON: f64 = 1e-4;

fn look_settings() -> impl Iterator<Item = &'static Setting> {
    settings().iter().filter(|s| LOOK.contains(&s.section))
}

/// A config with a wallpaper, as after the first pick.
fn with_wallpaper() -> ShellConfig {
    let mut cfg = ShellConfig::default();
    cfg.wallpaper.path = Some(PathBuf::from(WALLPAPER));
    cfg
}

/// A valid value for `setting` that differs from `current` (`None` for
/// buttons, which store nothing).
fn another_value(setting: &Setting, current: &Value) -> Option<Value> {
    Some(match setting.control {
        Control::Toggle => Value::from(!current.as_bool().unwrap_or(false)),
        Control::Slider { min, max, .. } => {
            let v = current.as_f64().unwrap_or(min);
            // The farther end of the range is valid and always different.
            Value::from(if v - min > max - v { min } else { max })
        }
        Control::Choice(choices) => {
            let now = match current {
                Value::String(s) => s.clone(),
                other => other.to_string(),
            };
            choice_value(choices.iter().map(|(raw, _)| *raw).find(|raw| *raw != now)?)
        }
        Control::Time => Value::from(if current.as_str() == Some("06:45") { "07:30" } else { "06:45" }),
        Control::Text { .. } | Control::OptionalText { .. } if setting.path.ends_with("color") => Value::from("#123456"),
        Control::Text { .. } | Control::OptionalText { .. } if setting.path.contains("/variants/") => Value::from("~/Pictures/Wallpapers/test.jpg"),
        Control::Text { .. } | Control::OptionalText { .. } => Value::from("Mandara Test"),
        Control::Command { .. } => return None,
        _ => return None,
    })
}

fn same(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() < F32_EPSILON,
        _ => a == b,
    }
}

#[test]
fn every_look_and_feel_control_writes_its_key_and_survives_a_restart() {
    let mut cfg = with_wallpaper();
    let mut expected: Vec<(&Setting, Value)> = Vec::new();
    let mut controls = 0;
    for setting in look_settings() {
        let Some(value) = another_value(setting, &get(&cfg, setting)) else { continue };
        controls += 1;
        let next = set(&cfg, setting, value.clone()).unwrap_or_else(|e| panic!("{}: {e}", setting.path));
        let written = get(&next, setting);
        assert!(same(&written, &integral(value.clone())), "{}: wrote {value}, reads {written}", setting.path);
        // The shell's `config get <pointer>` answers from the same tree.
        let tree = serde_json::to_value(&next).unwrap();
        assert!(same(tree.pointer(setting.path).unwrap_or(&Value::Null), &written), "{}: IPC view differs", setting.path);
        expected.retain(|(s, _)| s.path != setting.path);
        expected.push((setting, written));
        cfg = next;
    }
    assert!(controls >= 60, "only {controls} Look & feel controls were exercised");

    // Saved, then read back by a fresh store (a shell restart).
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    let mut store = ConfigStore::open(&path).unwrap();
    assert!(store.replace(cfg.clone()));
    store.flush().unwrap();
    let restarted = ConfigStore::open(&path).unwrap();
    for (setting, value) in &expected {
        assert!(same(&get(restarted.get(), setting), value), "{} lost on restart", setting.path);
    }
}

fn palette_of(cfg: &ShellConfig) -> mandara_theme::MandaraPalette {
    ThemeEngine::new(cfg.theme.source.clone(), cfg.theme.mode.is_dark(), cfg.theme.adjustments).target().clone()
}

fn apply(cfg: &ShellConfig, path: &str, value: Value) -> ShellConfig {
    set(cfg, by_path(path).unwrap_or_else(|| panic!("no setting at {path}")), value).unwrap_or_else(|e| panic!("{path}: {e}"))
}

#[test]
fn appearance_controls_reshape_the_palette() {
    let base = ShellConfig::default();
    let p = palette_of(&base);

    let vivid = palette_of(&apply(&base, "/theme/adjustments/chroma_scale", Value::from(1.6)));
    assert!(vivid.primary.chroma() > p.primary.chroma() + 0.01, "accent vividness raises chroma");
    assert!((vivid.surface_base.chroma() - p.surface_base.chroma()).abs() < 1e-3, "surfaces keep their chroma");

    let hue = palette_of(&apply(&base, "/theme/adjustments/hue_shift", Value::from(60.0)));
    let turned = mandara_theme::color::shortest_hue_delta(p.primary.hue(), hue.primary.hue());
    // Gamut mapping back into sRGB may bend the hue by a few degrees.
    assert!((turned - 60.0).abs() < HUE_TOLERANCE_DEG, "hue turned {turned}°");

    let light = palette_of(&apply(&base, "/theme/adjustments/lightness_shift", Value::from(0.1)));
    assert!(light.surface_base.lightness() > p.surface_base.lightness() + 0.05);

    let no_rim = palette_of(&apply(&base, "/theme/adjustments/rim_intensity", Value::from(0.0)));
    assert_eq!(no_rim.specular_rim_light.alpha(), 0.0);

    let hc = palette_of(&apply(&base, "/theme/adjustments/high_contrast", Value::from(true)));
    for surface in [hc.surface_base, hc.surface_container, hc.surface_container_high] {
        assert!(hc.on_surface.contrast_ratio(surface) >= HC_MIN_CONTRAST, "high contrast reaches AAA");
    }

    let day = apply(&base, "/theme/mode", Value::from("light"));
    assert!(!palette_of(&day).is_dark && palette_of(&day).surface_base.lightness() > p.surface_base.lightness());
}

#[test]
fn theme_crossfade_follows_its_setting_and_reduced_motion() {
    let cfg = apply(&ShellConfig::default(), "/theme/transition_ms", Value::from(900.0));
    assert_eq!(mandara_ui::state::theme_transition(&cfg), Duration::from_millis(900));
    let reduced = apply(&cfg, "/motion/reduce_motion", Value::from(true));
    assert_eq!(mandara_ui::state::theme_transition(&reduced), Duration::ZERO);

    // The engine honours it: a 900 ms crossfade is still running at 400 ms.
    let t0 = Instant::now();
    let mut engine = ThemeEngine::new(cfg.theme.source.clone(), true, cfg.theme.adjustments);
    engine.set_duration(mandara_ui::state::theme_transition(&cfg));
    engine.set_source(ThemeSource::Preset { preset: "montaza-twilight".into() }, true, t0).unwrap();
    assert!(engine.is_animating(t0 + Duration::from_millis(400)));
    engine.set_duration(mandara_ui::state::theme_transition(&reduced));
    engine.set_source(ThemeSource::Preset { preset: "mandara-obsidian".into() }, true, t0).unwrap();
    assert!(!engine.is_animating(t0), "reduced motion swaps palettes at once");
}

#[test]
fn wallpaper_scheme_switches_a_designed_theme_to_wallpaper_colors() {
    let scheme = by_path(WALLPAPER_SCHEME_PATH).unwrap();
    assert_eq!(get(&with_wallpaper(), scheme), Value::Null, "a designed theme lights no scheme chip");
    // Without a wallpaper there is nothing to take colors from.
    let err = set(&ShellConfig::default(), scheme, Value::from("vibrant")).unwrap_err();
    assert!(err.contains("wallpaper"), "{err}");

    let cfg = apply(&with_wallpaper(), "/theme/source/variant", Value::from("vibrant"));
    assert_eq!(cfg.theme.source, ThemeSource::Wallpaper { path: WALLPAPER.into(), variant: SchemeVariant::Vibrant, seed: 0 });
    assert!(cfg.wallpaper.match_colors, "the wallpaper sync keeps it (else it switches straight back)");
    assert_eq!(cfg.theme.last_preset.as_deref(), Some(mandara_theme::DEFAULT_THEME), "the designed theme is remembered");
    assert_eq!(get(&cfg, scheme), Value::from("vibrant"));

    // On a wallpaper theme only the scheme changes; the chosen lead color stays.
    let mut led = cfg.clone();
    led.theme.source = ThemeSource::Wallpaper { path: WALLPAPER.into(), variant: SchemeVariant::Vibrant, seed: 2 };
    let next = apply(&led, "/theme/source/variant", Value::from("expressive"));
    assert_eq!(next.theme.source, ThemeSource::Wallpaper { path: WALLPAPER.into(), variant: SchemeVariant::Expressive, seed: 2 });
}

#[test]
fn every_wallpaper_scheme_choice_is_accepted_with_a_wallpaper() {
    let scheme = by_path(WALLPAPER_SCHEME_PATH).unwrap();
    let Control::Choice(choices) = scheme.control else { panic!("the scheme is a choice") };
    for (raw, _) in choices {
        let cfg = set(&with_wallpaper(), scheme, choice_value(raw)).unwrap_or_else(|e| panic!("{raw}: {e}"));
        assert_eq!(get(&cfg, scheme), Value::from(*raw));
    }
}

#[test]
fn motion_controls_retune_every_spring() {
    use mandara_core::SpringConfig;
    let base = ShellConfig::default();
    assert_eq!(mandara_ui::style::motion_profile(&base), mandara_core::spring::MotionProfile::default());
    let stiff = apply(&base, "/motion/stiffness", Value::from(560.0));
    let profile = mandara_ui::style::motion_profile(&stiff);
    assert!(profile.tune(SpringConfig::default()).natural_frequency() > SpringConfig::default().natural_frequency());
    let heavy = mandara_ui::style::motion_profile(&apply(&base, "/motion/mass", Value::from(2.5)));
    assert!(heavy.tune(SpringConfig::GENTLE).natural_frequency() < SpringConfig::GENTLE.natural_frequency());
    let bouncy = mandara_ui::style::motion_profile(&apply(&base, "/motion/damping", Value::from(8.0)));
    assert!(bouncy.tune(SpringConfig::SNAPPY).damping_ratio() < SpringConfig::SNAPPY.damping_ratio());
    let reduced = mandara_ui::style::motion_profile(&apply(&base, "/motion/reduce_motion", Value::from(true)));
    assert!(reduced.reduce);
    let mut s = mandara_core::SpringValue::new(0.0_f32, SpringConfig::default());
    s.set_target(1.0);
    assert!(!s.step_in(1.0 / 144.0, &reduced));
    assert_eq!(s.value(), 1.0, "reduced motion lands at once");
    assert_eq!(apply(&base, "/motion/speed", Value::from(2.0)).motion.speed, 2.0);
}

#[test]
fn app_theme_toggles_select_their_templates() {
    let base = ShellConfig::default();
    let app_paths: Vec<&Setting> = look_settings().filter(|s| s.path.starts_with("/integrations/")).collect();
    assert!(app_paths.len() >= 17, "every app has a switch");
    for setting in app_paths {
        let on = get(&base, setting).as_bool().unwrap();
        let flipped = set(&base, setting, Value::from(!on)).unwrap();
        let (before, before_apps) = mandara_ui::state::app_theme_targets(&base.integrations);
        let (after, after_apps) = mandara_ui::state::app_theme_targets(&flipped.integrations);
        assert!(before != after || before_apps != after_apps, "{} selects no template", setting.path);
    }
    // And the selection writes (or skips) the files.
    let dir = tempfile::tempdir().unwrap();
    let (cache, config, data) = (dir.path().join("cache"), dir.path().join("config"), dir.path().join("data"));
    let only_kitty = {
        let mut c = ShellConfig::default();
        c.integrations = mandara_config::IntegrationsConfig { ghostty: false, gtk4: false, qt6: false, zed: false, fuzzel: false, btop: false, compositor_borders: false, kvantum: false, kvantum_apply: false, kitty: true, foot: false, alacritty: false, wezterm: false, helix: false, neovim: false, firefox: false, vesktop: false, kde: false };
        c
    };
    let (targets, apps) = mandara_ui::state::app_theme_targets(&only_kitty.integrations);
    let p = palette_of(&only_kitty);
    let mut files = mandara_theme::templates::sync(&p, targets, &cache, &config);
    files.extend(mandara_theme::templates::sync_apps(&p, apps, &cache, &config, &data));
    assert_eq!(files, vec![cache.join("kitty.conf"), config.join("kitty/themes/mandara.conf")]);
    assert!(std::fs::read_to_string(&files[0]).unwrap().contains(&p.primary.to_hex()));
}

#[test]
fn wallpaper_controls_choose_what_the_desktop_shows() {
    let cfg = apply(&with_wallpaper(), "/wallpaper/variants/light", Value::from("~/Pictures/day.jpg"));
    // Studio's text field turns an emptied optional field into null.
    let cfg = apply(&cfg, "/wallpaper/variants/dark", Value::Null);
    assert_eq!(cfg.wallpaper.variants.dark, None, "an emptied field clears the variant");
    let home = mandara_config::paths::expand_tilde("~");
    let pick = |c: &ShellConfig, dark: bool| mandara_config::desktop::effective_wallpaper(c.wallpaper.path.as_ref(), &c.wallpaper.variants, Some("DP-1"), dark);
    assert_eq!(pick(&cfg, false), Some(home.join("Pictures/day.jpg")));
    assert_eq!(pick(&cfg, true), Some(PathBuf::from(WALLPAPER)));

    // A fill color must be a color.
    let fill = by_path("/wallpaper/fill_color").unwrap();
    assert!(set(&cfg, fill, Value::from("red")).is_err());
    assert_eq!(set(&cfg, fill, Value::from("#1e1e2e")).unwrap().wallpaper.fill_color, "#1e1e2e");
    assert_eq!(mandara_ui::wallpaper_fill::fill_color("#1e1e2e").to_hex(), "#1e1e2e");

    // The depth clock is a desktop widget now: moving it reaches the
    // clock's layout through the `[wallpaper]` keys kept in step.
    let mut moved = cfg.clone();
    mandara_config::desktop_widgets::migrate(&mut moved);
    let w = moved.desktop.widgets.get_mut(mandara_config::desktop_widgets::DEPTH_CLOCK_ID).expect("migrated depth clock");
    w.place.x = -0.25;
    w.place.scale = 1.5;
    mandara_config::desktop_widgets::sync_depth_clock(&mut moved);
    let (a, b) = (mandara_ui::desktop::layout(1920.0, 1080.0, &cfg.wallpaper), mandara_ui::desktop::layout(1920.0, 1080.0, &moved.wallpaper));
    assert!((b.center.0 - 0.25 * 1920.0).abs() < 1.0 && b.size > a.size);
    assert!(by_path("/wallpaper/clock_x").is_none(), "edited in the Desktop widgets gallery");
}

#[test]
fn typography_controls_write_the_families_and_weight() {
    let cfg = apply(&ShellConfig::default(), "/typography/ui_font", Value::from("Cairo"));
    let cfg = apply(&cfg, "/typography/weight", choice_value("600"));
    let cfg = apply(&cfg, "/typography/scale", Value::from(1.25));
    assert_eq!((cfg.typography.ui_font.as_str(), cfg.typography.weight, cfg.typography.scale), ("Cairo", 600, 1.25));
    let Control::Choice(weights) = by_path("/typography/weight").unwrap().control else { panic!() };
    for (raw, _) in weights {
        let w = apply(&cfg, "/typography/weight", choice_value(raw));
        assert_eq!(get(&w, by_path("/typography/weight").unwrap()).to_string(), *raw, "the active chip matches");
    }
}

#[test]
fn look_pages_show_each_group_once() {
    for section in LOOK {
        let groups = crate::app::group_rows(in_section(section));
        let mut titles: Vec<&str> = groups.iter().map(|(t, _)| *t).collect();
        let n = titles.len();
        titles.sort_unstable();
        titles.dedup();
        assert_eq!(titles.len(), n, "{:?} repeats a group", section.title());
        assert_eq!(groups.iter().map(|(_, l)| l.len()).sum::<usize>(), in_section(section).count());
    }
    let themes = crate::app::group_rows(in_section(Section::Themes));
    let apps = themes.iter().find(|(t, _)| *t == "App themes").map(|(_, l)| l.len());
    assert!(apps >= Some(17), "every app theme sits in one card");
}
