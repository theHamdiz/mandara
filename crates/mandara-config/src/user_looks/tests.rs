//! Your Looks: children, files, checks and safety.
use super::*;
use crate::desktop_widgets::{Anchor, ClockFace, ClockWidget, Placement};

fn tmp(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mandara-user-looks-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn corniche() -> &'static LookDef {
    looks::find("corniche-dusk").unwrap()
}

/// Applied fresh, no Look reads as customized.
#[test]
fn a_freshly_applied_look_is_not_customized() {
    for look in &looks::LOOKS {
        let c = look.apply(&ShellConfig::default(), LookOptions::default());
        assert_eq!(customized(&c, None), Customized::No, "{}", look.id);
        // Applying it again over itself changes nothing it owns.
        let again = look.apply(&c, LookOptions::default());
        assert_eq!(projection(&again), projection(&c), "{}: applying is idempotent", look.id);
        assert!(delta_over(look, &c).is_empty(), "{}: {:?}", look.id, delta_over(look, &c));
    }
}

#[test]
fn a_change_makes_a_child_that_reproduces_it() {
    let mut c = corniche().apply(&ShellConfig::default(), LookOptions::default());
    c.typography.ui_font = "Cairo".into();
    c.bar.position = crate::BarPosition::Bottom;
    let Customized::Changed { parent, delta } = customized(&c, None) else { panic!("customized") };
    assert_eq!(parent.id, "corniche-dusk");
    assert_eq!(crate::profiles::get_path(&delta.settings, "typography.ui_font").and_then(Value::as_str), Some("Cairo"));
    let child = UserLook::child(parent, delta, &[], "2026-11-11");
    assert_eq!(child.name, "Corniche Dusk · Custom");
    assert!(valid_id(&child.id) && looks::find(&child.id).is_none());
    // Applying the child elsewhere gives the same Look-owned settings.
    let again = child.apply(&ShellConfig::default(), LookOptions::default()).unwrap();
    assert_eq!(again.typography.ui_font, "Cairo");
    assert_eq!(again.bar.position, crate::BarPosition::Bottom);
    assert_eq!(again.looks.active, child.id);
    assert_eq!(customized(&again, Some(&child)), Customized::No);
    // The built-in Look is never touched.
    assert_eq!(looks::find("corniche-dusk").unwrap().ui_font, "Inter");
    // A second child of the same parent is numbered.
    let taken = vec![child.name.clone()];
    let second = UserLook::child(parent, Delta::default(), &taken, "2026-11-11");
    assert_eq!(second.name, "Corniche Dusk · Custom 2");
}

/// The child keeps only what you changed: a parent improved later passes
/// its other settings on.
#[test]
fn a_child_inherits_what_it_did_not_change() {
    let mut c = corniche().apply(&ShellConfig::default(), LookOptions::default());
    c.typography.ui_font = "Cairo".into();
    let Customized::Changed { parent, delta } = customized(&c, None) else { panic!() };
    let child = UserLook::child(parent, delta, &[], "");
    assert_eq!(child.settings.len(), 1, "{:?}", child.settings);
    // "A newer parent": a different radius scale.
    let mut newer = *corniche();
    newer.radius_scale = 1.4;
    let applied = newer.apply_layered(&ShellConfig::default(), LookOptions::default(), Some(&child.delta()), &child.id);
    assert_eq!(applied.style.radius_scale, 1.4, "inherited from the parent");
    assert_eq!(applied.typography.ui_font, "Cairo", "your change stays");
}

#[test]
fn removing_a_look_widget_is_a_customization_and_yours_are_not() {
    let mut c = corniche().apply(&ShellConfig::default(), LookOptions::default());
    // A widget of yours: not the Look's business.
    c.desktop.widgets.add(DesktopWidget { kind: WidgetKind::Clock(ClockWidget { face: ClockFace::Analog, ..Default::default() }), ..Default::default() });
    assert_eq!(customized(&c, None), Customized::No);
    // Removing the Look's clock is.
    let clock = c.looks.widgets[0].id.clone();
    c.desktop.widgets.items.retain(|w| w.id != clock);
    let Customized::Changed { delta, .. } = customized(&c, None) else { panic!() };
    let kept = delta.widgets.expect("the look's widgets are recorded");
    assert_eq!(kept.len(), corniche().widgets().len() - 1);
}

#[test]
fn files_round_trip_and_are_safe() {
    let dir = tmp("roundtrip");
    let mut c = corniche().apply(&ShellConfig::default(), LookOptions::default());
    c.typography.display_font = "Cairo".into();
    let Customized::Changed { parent, delta } = customized(&c, None) else { panic!() };
    let child = UserLook::child(parent, delta, &[], "2026-11-11");
    save_in(&dir, &child).unwrap();
    assert_eq!(find_in(&dir, &child.id), Some(child.clone()));
    let text = export_text_in(&dir, &child.id, "2026-11-11").unwrap();
    assert!(text.contains("format = 1") && text.contains("parent = \"corniche-dusk\""));
    // Importing it beside the original: own id, numbered name.
    let imported = import_in(&dir, &text).unwrap();
    assert_ne!(imported.id, child.id);
    assert_eq!(imported.name, "Corniche Dusk · Custom (2)");
    assert_eq!(imported.settings, child.settings);
    // Rename, duplicate, delete.
    let renamed = rename_in(&dir, &child.id, "Evening").unwrap();
    assert_eq!((renamed.id.as_str(), renamed.name.as_str()), (child.id.as_str(), "Evening"));
    let copy = duplicate_in(&dir, &child.id).unwrap();
    assert_eq!(copy.name, "Evening copy");
    assert_eq!(list_in(&dir).len(), 3);
    delete_in(&dir, &copy.id).unwrap();
    assert_eq!(list_in(&dir).len(), 2);
    // A built-in Look exports too, and imports as one of yours.
    let builtin = export_text_in(&dir, "phosphor", "").unwrap();
    let mine = import_in(&dir, &builtin).unwrap();
    assert_eq!(mine.parent, "phosphor");
    assert_ne!(mine.id, "phosphor");
    assert!(save_in(&dir, &UserLook { id: "phosphor".into(), ..mine }).is_err(), "never written over a built-in");
    let _ = std::fs::remove_dir_all(&dir);
}

fn file(extra: &str) -> String {
    format!("format = 1\nid = \"x\"\nname = \"X\"\nparent = \"corniche-dusk\"\n{extra}")
}

#[test]
fn import_refuses_what_a_look_cannot_hold() {
    assert!(parse(&file("")).is_ok());
    let bad = [
        ("malformed", "format = = 1".to_string()),
        ("unknown top-level key", file("evil = true\n")),
        ("newer format", file("").replace("format = 1", "format = 9")),
        ("unknown parent", file("").replace("corniche-dusk", "nope")),
        ("path traversal in the id", file("").replace("id = \"x\"", "id = \"../../x\"")),
        ("control characters", file("").replace("name = \"X\"", "name = \"X\\u0007\"")),
        ("a setting a look doesn't own", file("[settings.launcher]\nenabled = false\n")),
        ("plugin settings", file("[settings.plugins.settings.weather]\napi_key = \"k\"\n")),
        ("a command", file("[settings.hot_corners.top_left]\naction = \"command\"\n")),
        ("a command key", file("[settings.hot_corners.top_left]\ncommand = \"rm -rf ~\"\n")),
        ("a path value", file("[settings.typography]\nui_font = \"../../etc/passwd\"\n")),
        ("a wallpaper image", file("[settings.theme.source]\nkind = \"wallpaper\"\npath = \"/home/me/w.png\"\nvariant = \"vibrant\"\n")),
        ("a theme that isn't built in", file("[settings.theme.source]\nkind = \"preset\"\npreset = \"my-secret-theme\"\n")),
        ("an unknown key in a section", file("[settings.bar]\nnot_a_setting = 3\n")),
        ("a value that doesn't fit", file("[settings.bar]\nposition = \"diagonal\"\n")),
        ("unset outside", file("unset = [\"plugins.enabled\"]\n")),
        ("a photo widget", file("[[widgets]]\nid = \"photo-1\"\nkind = \"photo\"\n")),
        ("a widget on one display", file("[[widgets]]\nid = \"clock-1\"\nkind = \"clock\"\n[widgets.place]\ndisplays = [\"DP-1\"]\n")),
        ("oversized", file(&format!("author = \"{}\"\n", "a".repeat(MAX_FILE_BYTES)))),
    ];
    for (what, text) in bad {
        assert!(parse(&text).is_err(), "{what} must be refused");
    }
}

#[test]
fn export_leaves_out_anything_personal() {
    let mut look = UserLook::child(corniche(), Delta::default(), &[], "");
    let mut theme = Table::new();
    let mut src = Table::new();
    src.insert("kind".into(), Value::String("wallpaper".into()));
    src.insert("path".into(), Value::String("/home/me/Pictures/me.png".into()));
    theme.insert("source".into(), Value::Table(src));
    look.settings.insert("theme".into(), Value::Table(theme));
    let mut bar = Table::new();
    bar.insert("monitors".into(), Value::String("DP-1".into()));
    look.settings.insert("bar".into(), Value::Table(bar));
    let mut plugins = Table::new();
    plugins.insert("enabled".into(), Value::Array(vec![]));
    look.settings.insert("plugins".into(), Value::Table(plugins));
    look.widgets = Some(vec![
        DesktopWidget { kind: WidgetKind::Clock(ClockWidget::default()), place: Placement { displays: vec!["HDMI-A-1".into()], anchor: Anchor::Top, ..Default::default() }, ..Default::default() },
        DesktopWidget { kind: WidgetKind::Photo(Default::default()), ..Default::default() },
    ]);
    let text = look.to_text().unwrap();
    for leak in ["/home/me", "DP-1", "HDMI-A-1", "photo", "plugins"] {
        assert!(!text.contains(leak), "{leak} leaked:\n{text}");
    }
    let back = parse(&text).expect("an export always imports");
    assert!(back.wallpaper_colors, "the wallpaper palette is kept as the flag");
    assert_eq!(back.widgets.map(|w| w.len()), Some(1));
}
