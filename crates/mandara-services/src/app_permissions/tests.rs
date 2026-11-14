use super::*;
use mandara_theme::templates::FileStamp;
use std::sync::atomic::AtomicBool;

#[test]
fn native_records_keep_installations_distinct_and_reject_invalid_ids() {
    let rows = native::apps("org.example.Player\tPlayer\tsystem\norg.example.Player\tPlayer\tuser\n").unwrap();
    assert_eq!(rows.len(), 2);
    assert_ne!(rows[0].installation, rows[1].installation);
    assert!(native::apps("--delete\tNo\tuser").is_err());
}

#[test]
fn coarse_audio_and_device_grants_are_not_presented_as_independent_microphone_or_camera_controls() {
    let permissions = crate::app_install::flatpak::parse_context("[Context]\nsockets=pulseaudio;\ndevices=all;\nfilesystems=xdg-music:ro;\n").unwrap();
    let rows = model::capabilities(&permissions);
    assert!(rows.iter().any(|row| row.value == "pulseaudio" && row.allowed));
    assert!(rows.iter().any(|row| row.value == "all" && row.allowed));
    assert!(!rows.iter().any(|row| matches!(row.value.as_str(), "microphone" | "camera")));
    assert!(rows.iter().any(|row| row.value == "xdg-music:ro" && row.allowed && !row.editable));
}

#[test]
fn broader_and_conditional_grants_cannot_be_mistaken_for_independent_off_switches() {
    let permissions = crate::app_install::flatpak::parse_context("[Context]\nsockets=x11;if:x11:!has-wayland;\ndevices=all;\nfilesystems=host:ro;\n").unwrap();
    let rows = model::capabilities(&permissions);
    for value in ["dri", "home"] {
        let row = rows.iter().find(|row| row.value == value).unwrap();
        assert!(!row.editable);
        assert_eq!(row.constraint, Some(Constraint::Broader));
    }
    for value in ["x11", "if:x11:!has-wayland"] {
        let row = rows.iter().find(|row| row.value == value).unwrap();
        assert!(!row.editable);
        assert_eq!(row.constraint, Some(Constraint::Conditional));
    }
    assert!(rows.iter().find(|row| row.value == "all").unwrap().editable);
    let plain = model::capabilities(&crate::app_install::flatpak::parse_context("[Context]\ndevices=dri;\n").unwrap());
    assert!(plain.iter().find(|row| row.value == "dri").unwrap().editable);
}

#[test]
fn x11_fallback_and_its_wayland_condition_are_not_independent_switches() {
    for sockets in ["x11;wayland;fallback-x11;", "fallback-x11;"] {
        let permissions = crate::app_install::flatpak::parse_context(&format!("[Context]\nsockets={sockets}\n")).unwrap();
        let rows = model::capabilities(&permissions);
        for value in ["x11", "fallback-x11", "wayland"] {
            let row = rows.iter().find(|row| row.value == value).unwrap();
            assert!(!row.editable, "{sockets}: {value}");
            assert_eq!(row.constraint, Some(Constraint::Conditional));
        }
    }
    let permissions = crate::app_install::flatpak::parse_context("[Context]\nsockets=x11;wayland;\n").unwrap();
    assert!(model::capabilities(&permissions).iter().filter(|row| matches!(row.value.as_str(), "x11" | "wayland")).all(|row| row.editable));
    for conditional in ["if:x11:!has-wayland", "if:fallback-x11:!has-wayland", "if:wayland:has-wayland"] {
        let permissions = crate::app_install::flatpak::parse_context(&format!("[Context]\nsockets=x11;wayland;{conditional};\n")).unwrap();
        for row in model::capabilities(&permissions).iter().filter(|row| matches!(row.value.as_str(), "x11" | "wayland")) {
            assert!(!row.editable);
            assert_eq!(row.constraint, Some(Constraint::Conditional));
        }
    }
}

#[test]
fn a_selected_key_keeps_comments_other_groups_and_unknown_grants() {
    let before = "# personal\r\n[Context]\r\nsockets=wayland;\r\nshared=network;\r\n# keep me\r\n[Environment]\r\nMY_VALUE=untouched\r\n";
    let generated = "[Context]\nsockets=wayland;\nshared=!network;\n[Environment]\nMY_VALUE=untouched\n";
    let next = transaction::merge(before, generated, "shared").unwrap();
    assert_eq!(next, before.replace("shared=network;\r\n", "shared=!network;\n"));
    assert!(transaction::merge("[Context]\nshared=ipc;\nshared=network;\n", generated, "shared").is_err());
    assert!(transaction::merge("[Context]\n[Other]\n[Context]\n", generated, "shared").is_err());
    assert_eq!(transaction::merge("# personal\n[Other]\nkey=value\n", generated, "shared").unwrap(), "# personal\n[Other]\nkey=value\n[Context]\nshared=!network;\n");
}

#[test]
#[ignore = "opt-in native Flatpak override validation; writes only to a private data tree"]
fn native_override_preserves_symlinks_and_stale_undo_never_erases_personal_edits() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("org.example.MandaraTest");
    let target = root.path().join("personal-override");
    let before = "# personal comment\n[Context]\nshared=network;ipc;\nsockets=wayland;\n[Environment]\nKEEP=yes\n";
    std::fs::write(&target, before).unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let permissions = crate::app_install::flatpak::parse_context(before).unwrap();
    let session = Session { app: App { id: "org.example.MandaraTest".into(), name: "Test".into(), installation: "user".into() }, capabilities: model::capabilities(&permissions), permissions, metadata: before.into(), path: path.clone(), target: target.clone(), before: before.into(), stamp: FileStamp::at(&target).unwrap() };
    let network = session.capabilities.iter().find(|row| row.value == "network").unwrap();
    let undo = Flatpak { data: root.path().into() }.set(&session, network, false, &AtomicBool::new(false)).unwrap();
    assert!(std::fs::read_to_string(&target).unwrap().contains("!network"));
    assert_eq!(std::fs::read_link(&path).unwrap(), target);
    assert_eq!(std::fs::read_to_string(undo.backup.as_ref().unwrap()).unwrap(), before);
    undo.restore().unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), before);
    assert!(undo.restore().is_err(), "Undo only owns its original publication");
    assert!(Flatpak { data: root.path().into() }.set(&session, network, false, &AtomicBool::new(false)).is_err(), "same-byte replacement still makes an old session stale");
}
