use super::*;
use mandara_theme::templates::FileStamp;
use std::sync::atomic::AtomicBool;

struct OverrideFixture(Vec<(std::path::PathBuf, Option<Vec<u8>>)>);
impl OverrideFixture {
    fn capture(home: &std::path::Path, paths: &[std::path::PathBuf]) -> Self {
        let home = std::fs::canonicalize(home).unwrap();
        Self(paths.iter().map(|path| {
            let target = mandara_theme::templates::destination::identity(path).unwrap();
            assert!(target.starts_with(&home), "fixture override must stay in the private home");
            let before = match std::fs::read(&target) {
                Ok(bytes) => Some(bytes),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                Err(error) => panic!("could not capture fixture override: {error}"),
            };
            (target, before)
        }).collect())
    }
}
impl Drop for OverrideFixture {
    fn drop(&mut self) {
        for (path, before) in &self.0 {
            let result = match before {
                Some(bytes) => std::fs::write(path, bytes),
                None => match std::fs::remove_file(path) {
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    result => result,
                },
            };
            if let Err(error) = result {
                eprintln!("could not restore private fixture {}: {error}", path.display());
            }
        }
    }
}

#[test]
fn native_fixture_restores_existing_and_absent_overrides_on_success_and_unwind() {
    for unwind in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let existing = root.path().join("existing");
        let absent = root.path().join("absent");
        std::fs::write(&existing, b"original").unwrap();
        let result = std::panic::catch_unwind(|| {
            let _restore = OverrideFixture::capture(root.path(), &[existing.clone(), absent.clone()]);
            std::fs::write(&existing, b"changed").unwrap();
            std::fs::write(&absent, b"new").unwrap();
            assert!(!unwind, "exercise fixture unwind");
        });
        assert_eq!(result.is_err(), unwind);
        assert_eq!(std::fs::read(existing).unwrap(), b"original");
        assert!(!absent.exists());
    }
}

#[test]
fn permission_reads_reject_special_files_and_oversized_metadata() {
    let root = tempfile::tempdir().unwrap();
    let fifo = root.path().join("fifo");
    rustix::fs::mkfifoat(rustix::fs::CWD, &fifo, rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR).unwrap();
    assert!(transaction::read(&fifo).is_err());
    assert!(transaction::read(root.path()).is_err());
    let file = root.path().join("large");
    std::fs::write(&file, vec![b'x'; native::MAX_BYTES as usize + 1]).unwrap();
    assert!(transaction::read(&file).is_err());
}

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
    let rows = model::capabilities(&permissions, &permissions);
    assert!(rows.iter().any(|row| row.value == "pulseaudio" && row.allowed));
    assert!(rows.iter().any(|row| row.value == "all" && row.allowed));
    assert!(!rows.iter().any(|row| matches!(row.value.as_str(), "microphone" | "camera")));
    assert!(rows.iter().any(|row| row.value == "xdg-music:ro" && row.allowed && !row.editable));
}

#[test]
fn broader_and_conditional_grants_cannot_be_mistaken_for_independent_off_switches() {
    let permissions = crate::app_install::flatpak::parse_context("[Context]\nsockets=x11;if:x11:!has-wayland;\ndevices=all;\nfilesystems=host:ro;\n").unwrap();
    let rows = model::capabilities(&permissions, &permissions);
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
    let permissions = crate::app_install::flatpak::parse_context("[Context]\ndevices=dri;\n").unwrap();
    let plain = model::capabilities(&permissions, &permissions);
    assert!(plain.iter().find(|row| row.value == "dri").unwrap().editable);
}

#[test]
fn x11_fallback_and_its_wayland_condition_are_not_independent_switches() {
    for sockets in ["x11;wayland;fallback-x11;", "fallback-x11;"] {
        let permissions = crate::app_install::flatpak::parse_context(&format!("[Context]\nsockets={sockets}\n")).unwrap();
        let rows = model::capabilities(&permissions, &permissions);
        for value in ["x11", "fallback-x11", "wayland"] {
            let row = rows.iter().find(|row| row.value == value).unwrap();
            assert!(!row.editable, "{sockets}: {value}");
            assert_eq!(row.constraint, Some(Constraint::Conditional));
        }
    }
    let permissions = crate::app_install::flatpak::parse_context("[Context]\nsockets=x11;wayland;\n").unwrap();
    assert!(model::capabilities(&permissions, &permissions).iter().filter(|row| matches!(row.value.as_str(), "x11" | "wayland")).all(|row| row.editable));
    for conditional in ["if:x11:!has-wayland", "if:fallback-x11:!has-wayland", "if:wayland:has-wayland"] {
        let permissions = crate::app_install::flatpak::parse_context(&format!("[Context]\nsockets=x11;wayland;{conditional};\n")).unwrap();
        for row in model::capabilities(&permissions, &permissions).iter().filter(|row| matches!(row.value.as_str(), "x11" | "wayland")) {
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
    let session = Session { app: App { id: "org.example.MandaraTest".into(), name: "Test".into(), installation: "user".into() }, capabilities: model::capabilities(&permissions, &permissions), permissions, metadata: before.into(), path: path.clone(), target: target.clone(), before: before.into(), stamp: FileStamp::at(&target).unwrap() };
    let network = session.capabilities.iter().find(|row| row.value == "network").unwrap();
    let undo = transaction::apply(&session, network, false, &AtomicBool::new(false), || Ok(())).unwrap();
    assert!(std::fs::read_to_string(&target).unwrap().contains("!network"));
    assert_eq!(std::fs::read_link(&path).unwrap(), target);
    assert_eq!(std::fs::read_to_string(undo.backup.as_ref().unwrap()).unwrap(), before);
    undo.restore().unwrap();
    assert_eq!(std::fs::read_to_string(&target).unwrap(), before);
    assert!(undo.restore().is_err(), "Undo only owns its original publication");
    assert!(transaction::apply(&session, network, false, &AtomicBool::new(false), || Ok(())).is_err(), "same-byte replacement still makes an old session stale");

    let absent = root.path().join("absent-override");
    let fresh = Session { path: absent.clone(), target: absent.clone(), before: String::new(), stamp: None, ..session.clone() };
    let undo = transaction::apply(&fresh, network, false, &AtomicBool::new(false), || Ok(())).unwrap();
    assert!(std::fs::read_to_string(&absent).unwrap().contains("!network"));
    assert!(undo.backup.is_none());
    undo.restore().unwrap();
    assert!(!absent.exists());
    assert!(!std::fs::read_dir(root.path()).unwrap().filter_map(Result::ok)
        .any(|entry| entry.file_name().to_string_lossy().starts_with(".mandara-undo-")));
}

#[test]
fn revoked_non_common_grants_remain_available_for_a_later_edit() {
    let metadata = "[Context]\ndevices=!kvm;!input;\nsockets=!cups;\nfilesystems=!host-os;!xdg-documents;\n";
    let effective = crate::app_install::flatpak::parse_context(metadata).unwrap();
    let domain = crate::app_install::flatpak::context_domain(metadata).unwrap();
    assert!(effective.is_empty());
    let rows = model::capabilities(&effective, &domain);
    for value in ["kvm", "input", "cups", "host-os", "xdg-documents"] {
        let row = rows.iter().find(|row| row.value == value).unwrap();
        assert!(!row.allowed && row.editable, "{value}");
    }
    assert!(!rows.iter().any(|row| row.value == "usb"), "do not invent unsupported capabilities");
}

#[test]
fn host_access_cannot_be_revoked_through_its_narrower_controls() {
    for host in ["host", "host:ro", "host:rw", "host:create"] {
        let metadata = format!("[Context]\nfilesystems={host};host-os;host-etc;\n");
        let effective = crate::app_install::flatpak::parse_context(&metadata).unwrap();
        for row in model::capabilities(&effective, &effective).iter().filter(|row| matches!(row.value.as_str(), "host-os" | "host-etc")) {
            assert!(row.allowed && !row.editable, "{host}: {}", row.value);
            assert_eq!(row.constraint, Some(Constraint::Broader));
        }
    }
    let metadata = "[Context]\nfilesystems=!host;host-os;host-etc;\n";
    let effective = crate::app_install::flatpak::parse_context(metadata).unwrap();
    let domain = crate::app_install::flatpak::context_domain(metadata).unwrap();
    assert!(model::capabilities(&effective, &domain).iter().filter(|row| matches!(row.value.as_str(), "host-os" | "host-etc")).all(|row| row.editable));
}

#[test]
fn full_root_grants_lock_narrower_filesystem_controls_without_locking_a_revoked_root() {
    use crate::app_install::flatpak::{parse_context, context_domain};
    let narrow = ["host", "home", "host-os", "host-etc", "xdg-desktop", "xdg-documents", "xdg-download", "xdg-music", "xdg-pictures", "xdg-videos"];
    for grant in ["host-root", "host-root:ro", "host-root:rw", "host-root:create"] {
        let metadata = format!("[Context]\nfilesystems={grant};{};\n", narrow.join(";"));
        let effective = parse_context(&metadata).unwrap();
        let rows = model::capabilities(&effective, &effective);
        for value in narrow {
            let row = rows.iter().find(|row| row.value == value).unwrap();
            assert!(!row.editable, "{grant}: {value}");
            assert_eq!(row.constraint, Some(Constraint::Broader));
        }
        assert_eq!(rows.iter().find(|row| row.value == grant).unwrap().constraint, Some(Constraint::Scoped));
    }
    let metadata = "[Context]\nfilesystems=!host-root;!host-os;!host-etc;\n";
    let effective = parse_context(metadata).unwrap();
    let domain = context_domain(metadata).unwrap();
    for row in model::capabilities(&effective, &domain).iter().filter(|row| narrow.contains(&row.value.as_str())) {
        assert!(row.editable);
        assert!(!row.allowed);
    }
}

#[test]
fn effective_permission_changes_and_override_changes_invalidate_cached_controls() {
    let root = tempfile::tempdir().unwrap();
    let make = |metadata: &str| {
        let permissions = crate::app_install::flatpak::parse_context(metadata).unwrap();
        Session { app: App { id: "org.example.Test".into(), name: "Test".into(), installation: "user".into() }, capabilities: model::capabilities(&permissions, &permissions), permissions, metadata: metadata.into(), path: root.path().join("override"), target: root.path().join("override"), before: "unchanged".into(), stamp: None }
    };
    let session = make("[Context]\ndevices=dri;\n");
    let dri = session.capabilities.iter().find(|row| row.value == "dri").unwrap();
    assert!(native::validate_current(&session, dri, &session).is_ok());
    assert!(native::validate_current(&session, dri, &make("[Context]\ndevices=all;dri;\n")).is_err());
    let mut changed = make("[Context]\ndevices=dri;\n");
    changed.before = "edited elsewhere".into();
    assert!(native::validate_current(&session, dri, &changed).is_err());
    changed.before = session.before.clone();
    changed.target = root.path().join("other");
    assert!(native::validate_current(&session, dri, &changed).is_err());
}

#[test]
#[ignore = "opt-in native Flatpak inspection; requires a marked private home and an installed fixture app"]
fn native_effective_grants_are_rechecked_without_changing_the_personal_override() {
    let home = std::path::PathBuf::from(std::env::var_os("MANDARA_BACKUP_TEST_HOME").expect("private native home required"));
    assert_eq!(std::env::var_os("HOME").unwrap(), home.as_os_str());
    assert_eq!(std::fs::read_to_string(home.join(".mandara-backup-test")).unwrap(), "mandara restic fixture\n");
    let backend = Flatpak::default();
    assert!(backend.data.starts_with(&home));
    let app = App { id: std::env::var("MANDARA_FLATPAK_TEST_APP").expect("installed fixture app required"), name: "Fixture".into(), installation: "system".into() };
    let overrides = backend.data.join("overrides");
    std::fs::create_dir_all(&overrides).unwrap();
    let path = overrides.join(&app.id);
    let global = overrides.join("global");
    let _restore = OverrideFixture::capture(&home, &[path.clone(), global.clone()]);
    std::fs::write(&global, "").unwrap();
    let before = "[Context]\ndevices=dri;\n";
    std::fs::write(&path, before).unwrap();
    let cancel = AtomicBool::new(false);
    let session = backend.inspect(&app, &cancel).unwrap();
    let dri = session.capabilities.iter().find(|row| row.value == "dri").unwrap();
    assert!(dri.allowed && dri.editable);
    std::fs::write(&global, "[Context]\ndevices=all;\n").unwrap();
    let current = backend.inspect(&app, &cancel).unwrap();
    assert!(current.capabilities.iter().any(|row| row.value == "dri" && row.constraint == Some(Constraint::Broader)));
    assert!(backend.set(&session, dri, false, &cancel).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    std::fs::write(&global, "").unwrap();
    std::fs::write(&path, "[Context]\nfilesystems=home;\n").unwrap();
    let session = backend.inspect(&app, &cancel).unwrap();
    let capability = session.capabilities.iter().find(|row| row.value == "home").unwrap();
    assert!(capability.editable);
    std::fs::write(&global, format!("[Context]\nfilesystems={};\n", home.display())).unwrap();
    let current = backend.inspect(&app, &cancel).unwrap();
    assert!(current.capabilities.iter().any(|row| row.value == "home" && row.constraint == Some(Constraint::Broader)));
    assert!(backend.set(&session, capability, false, &cancel).is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "[Context]\nfilesystems=home;\n");

    std::fs::write(&global, "").unwrap();
    std::fs::write(&path, "[Context]\nshared=network;\n").unwrap();
    let mut command = std::process::Command::new("flatpak");
    command.args(["override", "--user"]);
    for name in ["A;B", "C:D", "E\\F"] {
        let directory = home.join(name);
        std::fs::create_dir_all(&directory).unwrap();
        let escaped = directory.to_str().unwrap().replace('\\', "\\\\").replace(':', "\\:");
        command.arg(format!("--filesystem={escaped}:ro"));
    }
    command.arg(&app.id);
    native::run(command, &cancel).unwrap();
    let current = backend.inspect(&app, &cancel).unwrap();
    for suffix in ["/A;B:ro", "/C\\:D:ro", "/E\\\\F:ro"] {
        assert!(current.permissions.filesystems.iter().any(|value| value.ends_with(suffix)), "{}: {:?}", suffix, current.permissions.filesystems);
    }
    assert!(current.capabilities.iter().any(|row| row.value == "network" && row.editable));
}
