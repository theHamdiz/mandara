//! Looks: applying, restoring, plugins, and what stays yours.
use super::*;

#[test]
fn every_look_has_a_unique_id() {
    let mut ids: Vec<&str> = LOOKS.iter().map(|l| l.id).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), LOOKS.len());
    for l in &LOOKS {
        assert!(find(l.id).is_some());
        let c = l.seed_color();
        assert!(c.lightness() > 0.3 && c.lightness() < 0.9, "{} seed is usable", l.id);
    }
}

#[test]
fn applying_never_touches_the_wallpaper_image() {
    let mut cfg = ShellConfig::default();
    cfg.wallpaper.path = Some("/tmp/w.png".into());
    for l in &LOOKS {
        let after = l.apply(&cfg, LookOptions::default());
        assert_eq!(after.wallpaper.path, cfg.wallpaper.path);
        assert_eq!(after.looks.active, l.id);
        assert!(matches!(after.theme.source, ThemeSource::Seed { .. }));
        let wp = l.apply(&cfg, LookOptions { wallpaper_colors: true });
        assert!(matches!(wp.theme.source, ThemeSource::Wallpaper { .. }));
        assert!(wp.looks.wallpaper_colors);
    }
    // Without a wallpaper the look's own seed is used.
    let none = LOOKS[0].apply(&ShellConfig::default(), LookOptions { wallpaper_colors: true });
    assert!(matches!(none.theme.source, ThemeSource::Seed { .. }));
}

#[test]
fn apply_then_restore_round_trips() {
    let mut mine = ShellConfig::default();
    mine.bar.height = 33.0;
    mine.typography.ui_font = "Cairo".into();
    mine.plugins.settings.insert("weather".into(), toml::Table::new());
    let before = before_snapshot(&mine, "2026-10-28 12:00").expect("own setup is snapshotted");
    let night = find("night-frame").unwrap().apply(&mine, LookOptions::default());
    assert_eq!(night.bar.position, BarPosition::Left);
    assert!(night.bar.screen_frame);
    let on: Vec<&str> = cards::enabled_cards(&night.desktop.widgets).map(|(_, c)| c.card.as_str()).collect();
    assert_eq!(on, [CALENDAR, MIRRORED_BARS]);
    // A second look keeps the first snapshot.
    assert!(before_snapshot(&night, "later").is_none());
    let wave = find("anfushi-wave").unwrap().apply(&night, LookOptions::default());
    assert_eq!(wave.bar.layout, BarLayout::Micro);
    let on: Vec<&str> = cards::enabled_cards(&wave.desktop.widgets).map(|(_, c)| c.card.as_str()).collect();
    assert_eq!(on, [WAVE]);
    let back = restore(&wave, &before).unwrap();
    assert_eq!(back, mine, "restoring brings the exact setup back");
    assert!(back.looks.active.is_empty());
}

/// A look clears the Expressive bar style and the rail it set up;
/// restoring brings both back, and a side bar setting changed while the
/// look was on (one the style doesn't own) stays as you changed it.
#[test]
fn restoring_from_the_expressive_style_brings_its_rail_back() {
    let mut mine = ShellConfig::default();
    crate::bar_styles::BarStyle::Expressive.apply(&mut mine);
    assert!(mine.side_bars.enabled);
    let before = before_snapshot(&mine, "2026-11-09 12:00").expect("own setup is snapshotted");
    let mut look = find("night-frame").unwrap().apply(&mine, LookOptions::default());
    assert_eq!(look.bar.style, None);
    assert_eq!(look.side_bars.enabled, crate::side_bars::SideBarsConfig::default().enabled);
    look.side_bars.opacity = 0.5;
    let back = restore(&look, &before).unwrap();
    assert_eq!(back.bar.style, Some(crate::bar_styles::BarStyle::Expressive));
    assert!(crate::bar_styles::BarStyle::Expressive.matches(&back));
    assert_eq!(back.side_bars.opacity, 0.5, "a side bar setting the style doesn't own is kept");
    let mut expect = mine.side_bars.clone();
    expect.opacity = 0.5;
    assert_eq!(back.side_bars, expect, "the rail comes back exactly");
}

/// Restoring your setup keeps the switch for keeping your component
/// designs as it is now, whatever the snapshot held.
#[test]
fn restoring_keeps_the_keep_components_choice() {
    let mine = ShellConfig::default();
    let before = before_snapshot(&mine, "2026-11-09 12:00").expect("own setup is snapshotted");
    let mut look = find("night-frame").unwrap().apply(&mine, LookOptions::default());
    look.looks.keep_components = true;
    let back = restore(&look, &before).unwrap();
    assert!(back.looks.keep_components, "switched on during the look, kept after restoring");
    assert!(back.looks.active.is_empty());
    look.looks.keep_components = false;
    let mut mine_kept = mine.clone();
    mine_kept.looks.keep_components = true;
    let before = before_snapshot(&mine_kept, "later").unwrap();
    assert!(!restore(&look, &before).unwrap().looks.keep_components, "switched off since: the snapshot doesn't switch it back on");
}

/// Picking a look brings its component designs; a design picked
/// afterwards stays; keeping your own designs leaves them alone; and the
/// switch survives looks and a save.
#[test]
fn looks_bring_their_component_designs_unless_you_keep_yours() {
    use crate::components::{ButtonDesign, ComponentDesigns};
    let mine = ShellConfig::default();
    for look in &LOOKS {
        let c = look.apply(&mine, LookOptions::default());
        assert_eq!(ComponentDesigns::of(&c.style), look.components.designs(), "{}", look.id);
        assert_eq!(ComponentPreset::current(&c.style), Some(look.components));
    }
    // A design you pick after the look is yours until the next look.
    let mut night = find("night-frame").unwrap().apply(&mine, LookOptions::default());
    night.style.button_design = ButtonDesign::Elevated;
    let saved: ShellConfig = toml::from_str(&toml::to_string(&night).unwrap()).unwrap();
    assert_eq!(saved.style.button_design, ButtonDesign::Elevated, "nothing resets it behind your back");
    let ink = find("pharos-ink").unwrap().apply(&saved, LookOptions::default());
    assert_eq!(ComponentPreset::current(&ink.style), Some(ComponentPreset::Minimal), "picking a look again applies its designs");
    // Keeping your designs: no look touches them, and the choice stays.
    let mut keep = ShellConfig::default();
    keep.looks.keep_components = true;
    keep.style.toggle_design = crate::components::ToggleDesign::Round;
    let before = ComponentDesigns::of(&keep.style);
    let a = find("anfushi-wave").unwrap().apply(&keep, LookOptions::default());
    let b = find("night-frame").unwrap().apply(&a, LookOptions::default());
    assert_eq!(ComponentDesigns::of(&b.style), before);
    assert!(b.looks.keep_components, "the switch survives switching looks");
    let back: ShellConfig = toml::from_str(&toml::to_string(&b).unwrap()).unwrap();
    assert!(back.looks.keep_components);
    // An older config has the switch off: looks bring their designs.
    let old: ShellConfig = toml::from_str("[looks]\nactive = \"pharos-ink\"").unwrap();
    assert!(!old.looks.keep_components);
    // Every built-in look has a preset, and they aren't all the same.
    assert!(LOOKS.iter().any(|l| l.components != LOOKS[0].components));
}

#[test]
fn looks_switch_on_the_plugins_they_need() {
    let mut cfg = ShellConfig::default();
    cfg.plugins.disabled.push(DOCK_ID.into());
    let forest = find("qaitbay-forest").unwrap().apply(&cfg, LookOptions::default());
    assert!(forest.dock.media_chip);
    assert!(!forest.plugins.disabled.iter().any(|d| d == DOCK_ID));
    let ink = find("pharos-ink").unwrap().apply(&cfg, LookOptions::default());
    assert!(ink.plugins.enabled.iter().any(|e| e == MINI_STATS_ID));
}

fn on(c: &ShellConfig, id: &str) -> bool {
    c.plugins.enabled.iter().any(|e| e == id) && !c.plugins.disabled.iter().any(|d| d == id)
}

#[test]
fn the_next_look_switches_off_the_plugins_the_last_one_switched_on() {
    let mine = ShellConfig::default();
    let forest = find("qaitbay-forest").unwrap();
    let ink = find("pharos-ink").unwrap();
    let crimson = find("montaza-crimson").unwrap();
    assert_eq!(forest.plugins(), [DOCK_ID]);
    assert_eq!(ink.plugins(), [MINI_STATS_ID]);
    assert!(crimson.plugins().is_empty());
    // A → B: the dock A switched on goes off; B's mini stats come on.
    let a = forest.apply(&mine, LookOptions::default());
    assert!(on(&a, DOCK_ID) && a.looks.plugins_enabled == [DOCK_ID]);
    let b = ink.apply(&a, LookOptions::default());
    assert!(!b.plugins.enabled.iter().any(|e| e == DOCK_ID), "{:?}", b.plugins);
    assert!(on(&b, MINI_STATS_ID) && b.looks.plugins_enabled == [MINI_STATS_ID]);
    // B → C (needs none): nothing a look switched on is left.
    let c = crimson.apply(&b, LookOptions::default());
    assert_eq!(c.plugins, mine.plugins);
    assert!(c.looks.plugins_enabled.is_empty() && c.looks.plugins_undisabled.is_empty());
    // A plugin both looks need stays on, still recorded as the looks'.
    let again = forest.apply(&forest.apply(&mine, LookOptions::default()), LookOptions::default());
    assert!(on(&again, DOCK_ID) && again.looks.plugins_enabled == [DOCK_ID]);
    assert_eq!(crimson.apply(&again, LookOptions::default()).plugins, mine.plugins);
    // A plugin you had switched off that a look needed goes back off.
    let mut off = mine.clone();
    off.plugins.disabled.push(DOCK_ID.into());
    let a = forest.apply(&off, LookOptions::default());
    assert!(on(&a, DOCK_ID) && a.looks.plugins_undisabled == [DOCK_ID]);
    assert_eq!(crimson.apply(&a, LookOptions::default()).plugins, off.plugins);
    // The record survives the config file.
    let text = toml::to_string(&a).unwrap();
    let back: ShellConfig = toml::from_str(&text).unwrap();
    assert_eq!(back.looks, a.looks);
}

#[test]
fn plugins_you_switched_on_are_never_switched_off_by_a_look() {
    let mut mine = ShellConfig::default();
    mine.plugins.enabled = vec![DOCK_ID.into(), MINI_STATS_ID.into(), "weather".into()];
    let forest = find("qaitbay-forest").unwrap();
    let ink = find("pharos-ink").unwrap();
    let crimson = find("montaza-crimson").unwrap();
    let mut c = mine.clone();
    for look in [forest, ink, crimson, forest, crimson] {
        c = look.apply(&c, LookOptions::default());
        for id in [DOCK_ID, MINI_STATS_ID, "weather"] {
            assert!(on(&c, id), "{} turned your {id} off", look.id);
        }
        assert!(c.looks.plugins_enabled.is_empty(), "{}: nothing was the look's doing", look.id);
    }
    assert_eq!(c.plugins, mine.plugins);
}

#[test]
fn looks_then_restore_equals_the_original_config() {
    let mut mine = ShellConfig::default();
    mine.plugins.enabled.push("weather".into());
    mine.plugins.disabled.push(DOCK_ID.into());
    mine.bar.height = 31.0;
    let before = before_snapshot(&mine, "2026-10-28 12:00").expect("own setup is snapshotted");
    let a = find("qaitbay-forest").unwrap().apply(&mine, LookOptions::default());
    let b = find("pharos-ink").unwrap().apply(&a, LookOptions::default());
    assert!(on(&b, MINI_STATS_ID));
    let back = restore(&b, &before).unwrap();
    assert_eq!(back, mine, "A → B → restore is exact");
    // A plugin you switched on while a look was applied stays on.
    let mut b2 = b.clone();
    b2.plugins.enabled.push("prayer-times".into());
    let back = restore(&b2, &before).unwrap();
    assert!(on(&back, "prayer-times") && !on(&back, MINI_STATS_ID) && !on(&back, DOCK_ID));
}

/// A config with widgets and bar items of your own: a clock, weather, a
/// plugin card (its plugin switched on), a third-party bar item, a
/// shipped-off item on the bar and one on the right side bar.
fn yours() -> ShellConfig {
    use crate::desktop_widgets::{DesktopWidgetsConfig, WeatherWidget};
    let mut c = ShellConfig::default();
    let mut list = DesktopWidgetsConfig { migrated: true, ..DesktopWidgetsConfig::default() };
    list.add(DesktopWidget { kind: WidgetKind::Clock(ClockWidget { face: ClockFace::Analog, ..Default::default() }), place: Placement { anchor: Anchor::BottomRight, x: -0.1, y: -0.2, scale: 1.3, ..Default::default() }, ..Default::default() });
    list.add(DesktopWidget { kind: WidgetKind::Weather(WeatherWidget::default()), place: Placement { anchor: Anchor::Top, ..Default::default() }, ..Default::default() });
    list.add(cards::new_card("plugin:pomodoro/timer", Anchor::Left));
    c.desktop.widgets = list;
    c.plugins.enabled.push("pomodoro".into());
    c.bar.left.push("my-wasm-widget".into());
    c.bar.right.insert(0, "net-speed".into());
    c.plugins.enabled.push("net-speed".into());
    c.side_bars.right.top = vec!["todo".into()];
    c
}

fn shown_items(c: &ShellConfig) -> Vec<String> {
    crate::bar_styles::ItemZone::ALL.iter().filter(|z| z.shows(c)).flat_map(|z| z.get(c).iter().cloned()).collect()
}

fn assert_kept(c: &ShellConfig, mine: &ShellConfig, what: &str) {
    for w in &mine.desktop.widgets.items {
        assert!(c.desktop.widgets.items.iter().any(|x| x == w), "{what}: your widget {} (id, place, settings) is kept", w.id);
    }
    for id in ["my-wasm-widget", "net-speed"] {
        assert!(shown_items(c).iter().any(|x| x == id), "{what}: your bar item {id} stays: {:?}", shown_items(c));
    }
    assert_eq!(c.side_bars.right.top, ["todo"], "{what}");
    for id in ["pomodoro", "net-speed"] {
        assert!(on(c, id), "{what}: {id} stays on");
    }
    let list = shown_items(c);
    let twice: Vec<&String> = list.iter().enumerate().filter(|(i, id)| list[..*i].contains(id)).map(|(_, id)| id).collect();
    assert!(twice.is_empty(), "{what}: {twice:?} shown twice");
}

#[test]
fn every_look_and_bar_preset_keeps_your_widgets_items_and_plugins() {
    let mine = yours();
    for look in &LOOKS {
        let c = look.apply(&mine, LookOptions::default());
        assert_kept(&c, &mine, look.id);
        // Its own widgets came in too.
        assert_eq!(c.desktop.widgets.items.len(), mine.desktop.widgets.items.len() + look.widgets().len(), "{}", look.id);
        // Its style, when it carries one, is the bar's.
        assert_eq!(c.bar.style, look.bar.style(), "{}", look.id);
        for id in look.plugins() {
            assert!(on(&c, id), "{}: {id} on", look.id);
        }
        // From every look to every other one: still yours, nothing piles up.
        for next in &LOOKS {
            let d = next.apply(&c, LookOptions::default());
            assert_kept(&d, &mine, next.id);
            assert_eq!(d.desktop.widgets.items.len(), mine.desktop.widgets.items.len() + next.widgets().len(), "{} → {}", look.id, next.id);
        }
    }
    for style in crate::bar_styles::BarStyle::ALL {
        let mut c = mine.clone();
        style.apply(&mut c);
        assert_eq!(c.desktop.widgets, mine.desktop.widgets, "{style:?}: the desktop is untouched");
        assert!(on(&c, "pomodoro"), "{style:?}");
        crate::bar_styles::clear(&mut c);
        assert_eq!(c.desktop.widgets, mine.desktop.widgets);
        assert!(on(&c, "pomodoro") && on(&c, "net-speed"), "{style:?}: the plugins your widgets and items need stay on");
    }
}

#[test]
fn switching_back_and_forth_is_stable() {
    let mine = yours();
    let a = find("redmond").unwrap();
    let b = find("phosphor").unwrap();
    let first = b.apply(&a.apply(&mine, LookOptions::default()), LookOptions::default());
    let mut c = first.clone();
    for _ in 0..4 {
        c = b.apply(&a.apply(&c, LookOptions::default()), LookOptions::default());
    }
    assert_eq!(c.bar, first.bar);
    assert_eq!(c.side_bars, first.side_bars);
    assert_eq!(c.desktop.widgets, first.desktop.widgets);
    assert_eq!(c.plugins, first.plugins);
}

#[test]
fn a_look_widget_you_moved_becomes_yours() {
    let c = find("corniche-dusk").unwrap().apply(&ShellConfig::default(), LookOptions::default());
    let mut moved = c.clone();
    moved.desktop.widgets.items[0].place.x = 0.3;
    let id = moved.desktop.widgets.items[0].id.clone();
    let next = find("pharos-ink").unwrap().apply(&moved, LookOptions::default());
    assert!(next.desktop.widgets.items.iter().any(|w| w.id == id && w.place.x == 0.3), "kept where you put it");
    let untouched = find("pharos-ink").unwrap().apply(&c, LookOptions::default());
    assert!(!untouched.desktop.widgets.items.iter().any(|w| w.place == c.desktop.widgets.items[0].place && w.kind == c.desktop.widgets.items[0].kind), "an untouched look widget goes with its look");
}

/// A config from before looks kept your widgets: the active look's widgets
/// are recognised by kind and place, yours stay.
#[test]
fn an_older_look_config_is_read_safely() {
    let night = find("night-frame").unwrap();
    let mut old = ShellConfig::default();
    old.looks.active = night.id.into();
    old.desktop.widgets.migrated = true;
    let mut items = night.widgets();
    items.push(DesktopWidget { id: "clock-9".into(), kind: WidgetKind::Clock(ClockWidget { face: ClockFace::Binary, ..Default::default() }), ..Default::default() });
    old.desktop.widgets.items = items;
    let wave = find("anfushi-wave").unwrap();
    let next = wave.apply(&old, LookOptions::default());
    assert!(next.desktop.widgets.items.iter().any(|w| w.id == "clock-9"), "yours stays");
    assert_eq!(next.desktop.widgets.items.len(), 1 + wave.widgets().len(), "the old look's own went");
}

#[test]
fn restoring_keeps_what_you_added_during_a_look() {
    let mine = yours();
    let before = before_snapshot(&mine, "2026-11-11 12:00").unwrap();
    let mut on_look = find("cupertino").unwrap().apply(&mine, LookOptions::default());
    assert!(on_look.dock.magnification && on(&on_look, DOCK_ID));
    on_look.desktop.widgets.add(DesktopWidget { kind: WidgetKind::Clock(ClockWidget::default()), ..Default::default() });
    on_look.bar.left.push("color-picker".into());
    let back = restore(&on_look, &before).unwrap();
    assert_kept(&back, &mine, "restore");
    assert_eq!(back.desktop.widgets.items.len(), mine.desktop.widgets.items.len() + 1, "the widget you added is kept");
    assert!(shown_items(&back).contains(&"color-picker".to_string()));
    assert_eq!(back.dock.magnification, mine.dock.magnification, "the look's dock settings go back");
    assert_eq!(back.launcher.look, mine.launcher.look);
}

#[test]
fn the_new_looks_set_what_they_promise() {
    let mine = ShellConfig::default();
    let r = find("redmond").unwrap().apply(&mine, LookOptions::default());
    assert_eq!(r.bar.style, Some(crate::bar_styles::BarStyle::Taskbar));
    assert_eq!((r.bar.position, r.theme.mode), (BarPosition::Bottom, ThemeMode::Light));
    assert!(r.bar.center.starts_with(&["launcher".to_string(), crate::bar_styles::TASKBAR.to_string()]), "{:?}", r.bar.center);
    assert!(on(&r, crate::bar_styles::TASKBAR));
    assert_eq!(r.notifications.position, crate::alerts::ToastPosition::BottomRight);
    let m = find("cupertino").unwrap().apply(&r, LookOptions::default());
    assert_eq!((m.bar.position, m.bar.height, m.bar.style), (BarPosition::Top, 26.0, None));
    assert!(on(&m, DOCK_ID) && m.dock.magnification);
    assert_eq!(m.launcher.look.style, crate::workflow::LauncherStyle::Beacon);
    assert!(!on(&m, crate::bar_styles::TASKBAR), "Redmond's taskbar went with it");
    // Redmond switches a dock you have on off, and gives it back after.
    let mut docked = ShellConfig::default();
    docked.plugins.enabled.push(DOCK_ID.into());
    let r = find("redmond").unwrap().apply(&docked, LookOptions::default());
    assert!(!on(&r, DOCK_ID));
    let back = find("night-frame").unwrap().apply(&r, LookOptions::default());
    assert!(on(&back, DOCK_ID));
    // Zen: the bar hides, hot corners on; the next look puts them back.
    let z = find("zen").unwrap().apply(&mine, LookOptions::default());
    assert!(z.hot_corners.enabled && z.bar.auto_hide.mode == AutoHideMode::Always);
    let after = find("pharos-ink").unwrap().apply(&z, LookOptions::default());
    assert_eq!(after.hot_corners, mine.hot_corners);
    assert_eq!(after.bar.auto_hide, mine.bar.auto_hide);
    // Every bar style preset has a look.
    for style in crate::bar_styles::BarStyle::ALL {
        assert!(LOOKS.iter().any(|l| l.bar.style() == Some(style)), "{style:?} has a look");
    }
}

/// Ink Rush draws everything inked and gives the ink back on leaving; its
/// widgets are comic panels whose text stays on their plate's colour.
#[test]
fn ink_rush_inks_the_desktop_and_the_next_look_takes_it_back() {
    let mine = yours();
    let ink = find("ink-rush").unwrap();
    let c = ink.apply(&mine, LookOptions::default());
    assert!(c.style.ink.is_on() && c.style.ink.outline() >= 2.0 && c.style.ink.shadow() > 0.0);
    assert_eq!(c.style.ink.texture, crate::desktop::SurfaceTexture::Halftone);
    assert!(on(&c, DOCK_ID));
    for w in ink.widgets() {
        if let WidgetKind::Clock(clock) = &w.kind {
            assert_eq!(clock.look.shadow, crate::desktop_widgets::ShadowKind::Hard);
            assert_eq!(clock.look.plate, crate::desktop_widgets::Plate::Solid, "a solid plate keeps the text's contrast");
        }
    }
    assert_kept(&c, &mine, "ink-rush");
    let after = find("pharos-ink").unwrap().apply(&c, LookOptions::default());
    assert_eq!(after.style.ink, mine.style.ink, "the ink goes with its look");
    // Ink you set yourself afterwards stays yours.
    let mut tuned = c.clone();
    tuned.style.ink.outline = 2.0;
    let after = find("pharos-ink").unwrap().apply(&tuned, LookOptions::default());
    assert_eq!(after.style.ink.outline, 2.0);
    // The ink style round-trips and an old config has none.
    let back: ShellConfig = toml::from_str(&toml::to_string(&c).unwrap()).unwrap();
    assert_eq!(back.style.ink, c.style.ink);
    let old: ShellConfig = toml::from_str("[style]\nshadows = true\n").unwrap();
    assert!(!old.style.ink.is_on());
}
