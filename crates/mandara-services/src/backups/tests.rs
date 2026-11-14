use super::*;
use std::os::unix::fs::PermissionsExt as _;
use std::sync::atomic::AtomicBool;

fn native_environment() {
    let directory = std::path::PathBuf::from(std::env::var_os("MANDARA_BACKUP_TEST_HOME").expect("run native backup tests with a private fixture home"));
    assert_eq!(std::fs::read_to_string(directory.join(".mandara-backup-test")).unwrap(), "mandara restic fixture\n");
    assert_eq!(std::env::var_os("HOME").unwrap(), directory.as_os_str());
    assert!(std::path::Path::new(&std::env::var_os("XDG_CACHE_HOME").unwrap()).starts_with(&directory));
    assert!(std::env::var_os("WAYLAND_DISPLAY").is_none());
    for variable in ["DBUS_SESSION_BUS_ADDRESS", "DBUS_SYSTEM_BUS_ADDRESS"] { assert_eq!(std::env::var(variable).unwrap(), "unix:path=/nonexistent"); }
}

#[test]
fn split_progress_records_and_restore_counters_are_lossless_and_bounded() {
    let mut events = restic::Events::new();
    let text = b"{\"message_type\":\"status\",\"percent_done\":0.25,\"files_restored\":3,\"bytes_restored\":19}\n{\"message_type\":\"summary\",\"snapshot_id\":\"abc\",\"total_files_processed\":4,\"total_bytes_processed\":200}";
    let mut updates = Vec::new();
    for chunk in text.chunks(7) { events.push(chunk, &mut |progress| updates.push(progress)).unwrap(); }
    events.finish(&mut |progress| updates.push(progress)).unwrap();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].percent_done, 0.25);
    assert_eq!(updates[0].files_done, 3);
    assert_eq!(updates[0].bytes_done, 19);
    assert_eq!(events.summary.files, 4);
    assert_eq!(events.summary.bytes, 200);
    assert!(events.push(&vec![b'x'; 256 * 1024 + 1], &mut |_| {}).is_err());
}

#[test]
fn histories_validate_identity_and_restore_creates_fresh_destinations() {
    assert!(restic::snapshots(br#"[{"id":"--delete","time":"now"}]"#).is_err());
    let root = tempfile::tempdir().unwrap();
    let id = "a".repeat(64);
    let first = restic::restore_directory(root.path(), &id).unwrap();
    let second = restic::restore_directory(root.path(), &id).unwrap();
    assert_ne!(first.path(), second.path());
    std::fs::write(first.path().join("existing"), "keep").unwrap();
    assert!(!second.path().join("existing").exists());
    assert!(restic::restore_directory(root.path(), "../../etc").is_err());
    assert!(Settings::default().validate().is_err());
}

#[test]
fn older_snapshots_do_not_invent_zero_counts_and_password_modes_match_the_settings_contract() {
    let rows = restic::snapshots(format!(r#"[{{"id":"{}","time":"2026-11-13"}}]"#, "a".repeat(64)).as_bytes()).unwrap();
    assert!(rows[0].summary.is_none());
    let root = tempfile::tempdir().unwrap();
    let password = root.path().join("password");
    std::fs::write(&password, "synthetic unit-test secret").unwrap();
    let settings = Settings { repository: root.path().join("repository"), password_file: password.clone(), sources: Vec::new() };
    for mode in [0o644, 0o640, 0o400, 0o700] {
        std::fs::set_permissions(&password, std::fs::Permissions::from_mode(mode)).unwrap();
        assert!(restic::command(&settings).is_err(), "{mode:o}");
    }
    std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600)).unwrap();
    assert!(restic::command(&settings).is_ok());
}

#[test]
#[ignore = "opt-in native restic integration; private repository and synthetic files only"]
fn native_encrypted_backup_check_and_restore_copy() {
    native_environment();
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    std::fs::create_dir(&source).unwrap();
    std::fs::write(source.join("original.txt"), "first edition").unwrap();
    let password = root.path().join("password");
    std::fs::write(&password, "synthetic integration-test secret").unwrap();
    std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600)).unwrap();
    let settings = Settings { repository: root.path().join("repository"), password_file: password.clone(), sources: vec![source.clone()] };
    let cancel = AtomicBool::new(false);
    Restic.execute(&settings, Operation::Initialize, &cancel, &mut |_| {}).unwrap();
    let summary = Restic.execute(&settings, Operation::Backup, &cancel, &mut |_| {}).unwrap();
    assert!(summary.snapshot_id.is_some());
    Restic.execute(&settings, Operation::Check, &cancel, &mut |_| {}).unwrap();
    let snapshots = Restic.snapshots(&settings, &cancel).unwrap();
    assert_eq!(snapshots.len(), 1);
    std::fs::write(source.join("original.txt"), "later edition").unwrap();
    let restored = Restic.execute(&settings, Operation::Restore { snapshot: snapshots[0].id.clone(), parent: root.path().into() }, &cancel, &mut |_| {}).unwrap().restored_to.unwrap();
    assert_eq!(std::fs::read_to_string(source.join("original.txt")).unwrap(), "later edition");
    let relative = source.strip_prefix("/").unwrap().join("original.txt");
    assert_eq!(std::fs::read_to_string(restored.join(relative)).unwrap(), "first edition");
    std::fs::set_permissions(password, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Restic.snapshots(&settings, &cancel).is_err());
}

#[test]
#[ignore = "opt-in native restic cancellation; private repository and synthetic files only"]
fn native_cancelled_backup_releases_its_repository_lock() {
    native_environment();
    let root = tempfile::tempdir().unwrap();
    let source = root.path().join("source");
    std::fs::create_dir(&source).unwrap();
    let mut content = vec![0; 1024 * 1024];
    getrandom::fill(&mut content).unwrap();
    std::fs::write(source.join("synthetic"), content).unwrap();
    let password = root.path().join("password");
    std::fs::write(&password, "synthetic cancellation-test secret").unwrap();
    std::fs::set_permissions(&password, std::fs::Permissions::from_mode(0o600)).unwrap();
    let settings = Settings { repository: root.path().join("repository"), password_file: password, sources: vec![source.clone()] };
    let cancel = std::sync::Arc::new(AtomicBool::new(false));
    Restic.execute(&settings, Operation::Initialize, &cancel, &mut |_| {}).unwrap();
    let mut command = restic::command(&settings).unwrap();
    // A rate limit keeps this small fixture active until cancellation.
    command.args(["--limit-upload", "64", "backup", "--"]).arg(source);
    let token = cancel.clone();
    /// Allow the native process to acquire its lock before interrupting the upload.
    const CANCEL_AFTER_SECONDS: u64 = 1;
    let interrupt = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(CANCEL_AFTER_SECONDS));
        token.store(true, std::sync::atomic::Ordering::Release);
    });
    let result = crate::app_themes::process::stream_native(command, &cancel, crate::app_themes::process::TIMEOUT, &mut |_| Ok(()));
    interrupt.join().unwrap();
    assert!(result.unwrap_err().to_string().contains("cancelled"));
    let locks = settings.repository.join("locks");
    assert_eq!(std::fs::read_dir(locks).unwrap().count(), 0, "SIGINT lets restic release its own lock");
    assert!(Restic.snapshots(&settings, &AtomicBool::new(false)).is_ok());
}
