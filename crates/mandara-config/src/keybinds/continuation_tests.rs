//! Native syntax regressions for continued lines, block headers and empty tails.

use super::*;
use crate::compositor::Target;
use std::path::Path;

#[test]
fn missing_argument_separators_are_inserted_only_when_a_value_is_added() {
    for (target, source, values, expected) in [
        (
            Target::Mango,
            "bind=SUPER,Q,killclient\n",
            vec!["bind", "SUPER", "Q", "spawn", "foot"],
            "bind=SUPER,Q,spawn,foot\n",
        ),
        (
            Target::Hyprland,
            "bind = SUPER, Q, killactive # keep\n",
            vec!["bind", "SUPER", "Q", "exec", "foot"],
            "bind = SUPER, Q, exec ,foot# keep\n",
        ),
    ] {
        let row = &parse(target, Path::new("config.conf"), source).unwrap()[0];
        let values: Vec<String> = values.into_iter().map(str::to_owned).collect();
        let body = patch(source, row, &values).unwrap();
        assert_eq!(body, expected);
        let parsed = parse(target, Path::new("config.conf"), &body).unwrap();
        assert_eq!(parsed[0].fields.last().unwrap().value, "foot");
        assert!(parsed[0].read_only.is_none());
    }
}

#[test]
fn sway_separate_braces_keep_inherited_flags_and_edit_only_the_selected_body() {
    let source = "set $mod Mod4\n# keep\nbindsym --locked\n{\n    $mod+q kill\n}\nmode \"resize\"\n{\n    bindsym Mod4+w mode default\n}\n";
    let rows = parse(Target::Sway, Path::new("config"), source).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].line, 5);
    assert_eq!(rows[0].chord.as_deref(), Some("super+q"));
    assert_eq!(rows[1].scope, "resize");
    let flags = rows[0]
        .fields
        .iter()
        .find(|field| field.label == "Flags")
        .unwrap();
    assert_eq!(flags.value, "--locked");
    assert!(flags.read_only.is_some());
    let values = rows[0]
        .fields
        .iter()
        .map(|field| {
            if field.label == "Action" {
                "exec foot".to_owned()
            } else {
                field.value.clone()
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        patch(source, &rows[0], &values).unwrap(),
        source.replace("$mod+q kill", "$mod+q exec foot")
    );
    let resolved = crate::compositor::sway_binds(source);
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0].expanded, "bindsym --locked Mod4+q kill");
}

#[test]
fn continued_hyprland_bindings_remain_visible_with_their_physical_locations() {
    let source = "bind = SUPER, \\\n Q, exec, foot\nbind = SUPER, W, killactive,\n";
    let rows = parse(Target::Hyprland, Path::new("hyprland.conf"), source).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].line, 1);
    assert!(rows[0].read_only.is_some());
    assert_eq!(rows[1].line, 3);
    assert!(rows[1].read_only.is_none());
    let values = rows[1]
        .fields
        .iter()
        .map(|field| {
            if field.label == "Key" {
                "E".to_owned()
            } else {
                field.value.clone()
            }
        })
        .collect::<Vec<_>>();
    assert_eq!(
        patch(source, &rows[1], &values).unwrap(),
        source.replace("SUPER, W", "SUPER, E")
    );
}

#[test]
fn configuration_examples_stay_with_the_shortcut_section() {
    let guide = include_str!("../../../../docs/configuration.md");
    let shortcuts = guide
        .split("### Custom keyboard shortcuts")
        .nth(1)
        .unwrap()
        .split("### Studio appearance")
        .next()
        .unwrap();
    assert!(shortcuts.contains("[[compositor.keybinds]]"));
    assert!(shortcuts.contains("Hyprland dispatcher reference"));
    let studio = guide
        .split("### Studio appearance")
        .nth(1)
        .unwrap()
        .split("## What's configurable")
        .next()
        .unwrap();
    assert!(!studio.contains("[[compositor.keybinds]]"));
}

#[test]
fn large_blank_sway_sources_finish_without_repeated_lookahead() {
    let (send, receive) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let source = "\n".repeat(MAX_FILE_BYTES - 1);
        send.send(
            parse(Target::Sway, Path::new("config"), &source)
                .unwrap()
                .is_empty(),
        )
        .unwrap();
    });
    assert!(receive.recv_timeout(crate::doctor::CHECK_TIMEOUT).unwrap());
    worker.join().unwrap();
}

#[test]
fn sway_continuations_keep_rows_read_only_without_hiding_later_bindings() {
    let source = "bindsym \\\nMod4+q kill\nbindsym Mod4+w kill\n";
    let rows = parse(Target::Sway, Path::new("config"), source).unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].chord.as_deref(), Some("super+q"));
    assert!(rows[0].read_only.is_some());
    assert_eq!(rows[1].line, 3);
    assert_eq!(rows[1].chord.as_deref(), Some("super+w"));
}
