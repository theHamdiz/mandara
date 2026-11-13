use super::super::{FileEntry, hash_hex};
use super::*;

#[test]
fn interrupted_first_writes_recover_before_and_after_moving_files_and_links() {
    for link in [false, true] {
        for phase in 0..3 {
            let root = tempfile::tempdir().unwrap();
            let record = root.path().join("ledger.toml");
            let path = root.path().join("theme");
            let backup = root.path().join("theme.mandara-backup");
            if link {
                std::os::unix::fs::symlink("personal", &path).unwrap();
            } else {
                std::fs::write(&path, "personal settings").unwrap();
            }
            let mut ledger = Ledger::read(&record).unwrap();
            let entry = FileEntry {
                app: "user/example".into(),
                hash: hash_hex(b"new palette"),
                backup: Some(backup.clone()),
                created_dirs: Vec::new(),
            };
            ledger
                .begin_file(path.to_str().unwrap(), &entry, Some(&backup))
                .unwrap();
            if phase >= 1 {
                std::fs::rename(&path, &backup).unwrap();
            }
            if phase >= 2 {
                std::fs::write(&path, "new palette").unwrap();
            }
            drop(ledger);
            let mut recovered = Ledger::read(&record).unwrap();
            assert!(
                recovered
                    .take_back_checked("user/example")
                    .errors
                    .is_empty()
            );
            recovered.store(&record).unwrap();
            if link {
                assert_eq!(std::fs::read_link(&path).unwrap(), Path::new("personal"));
            } else {
                assert_eq!(std::fs::read_to_string(&path).unwrap(), "personal settings");
            }
            assert!(recovered.is_empty());
        }
    }
}

#[test]
fn interrupted_palette_updates_keep_both_old_and_new_ownership_recoverable() {
    for phase in 0..3 {
        let root = tempfile::tempdir().unwrap();
        let record = root.path().join("ledger.toml");
        let path = root.path().join("theme");
        std::fs::write(&path, "personal").unwrap();
        let mut ledger = Ledger::read(&record).unwrap();
        ledger
            .put_file("user/example", &path, "old palette")
            .unwrap();
        let mut next = ledger.files[path.to_str().unwrap()].clone();
        next.hash = hash_hex(b"new palette");
        ledger
            .begin_file(path.to_str().unwrap(), &next, None)
            .unwrap();
        if phase >= 1 {
            super::super::super::file_io::write(&path, "new palette").unwrap();
        }
        if phase == 2 {
            // A compositor rollback replaces the inode while returning old bytes.
            super::super::super::file_io::write(&path, "old palette").unwrap();
        }
        drop(ledger);
        let mut recovered = Ledger::read(&record).unwrap();
        assert!(
            recovered
                .take_back_checked("user/example")
                .errors
                .is_empty()
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), "personal");
    }
}

#[test]
fn interrupted_restores_resume_or_recognize_the_returned_original() {
    for phase in 0..3 {
        let root = tempfile::tempdir().unwrap();
        let record = root.path().join("ledger.toml");
        let path = root.path().join("theme");
        std::fs::write(&path, "personal").unwrap();
        let mut ledger = Ledger::read(&record).unwrap();
        ledger.put_file("user/example", &path, "palette").unwrap();
        let entry = ledger.files[path.to_str().unwrap()].clone();
        ledger.begin_restore(&path, &entry).unwrap();
        if phase >= 1 {
            std::fs::remove_file(&path).unwrap();
        }
        if phase >= 2 {
            super::super::super::restore::no_clobber(entry.backup.as_ref().unwrap(), &path)
                .unwrap();
        }
        drop(ledger);
        let mut recovered = Ledger::read(&record).unwrap();
        assert!(
            recovered
                .take_back_checked("user/example")
                .errors
                .is_empty()
        );
        assert!(recovered.is_empty());
        assert_eq!(std::fs::read_to_string(path).unwrap(), "personal");
    }
}

#[test]
fn missing_backups_after_an_interrupted_write_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let record = root.path().join("ledger.toml");
    let path = root.path().join("theme");
    std::fs::write(&path, "personal").unwrap();
    let mut ledger = Ledger::read(&record).unwrap();
    let backup = root.path().join("missing-backup");
    let next = FileEntry {
        app: "user/example".into(),
        hash: hash_hex(b"palette"),
        backup: Some(backup.clone()),
        created_dirs: Vec::new(),
    };
    ledger
        .begin_file(path.to_str().unwrap(), &next, Some(&backup))
        .unwrap();
    std::fs::write(&path, "newer personal edit").unwrap();
    drop(ledger);
    assert!(Ledger::read(&record).is_err());
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "newer personal edit"
    );
    assert!(std::fs::read_to_string(record).unwrap().contains("pending"));
}

#[test]
fn serialized_slot_identities_keep_the_full_native_identifier_range() {
    let original = SlotStamp {
        device: u64::MAX as i64,
        inode: (1_u64 << 63) as i64,
        length: i64::MAX as u64,
        seconds: i64::MIN,
        nanos: 999_999_999,
    };
    let encoded = toml::to_string(&original).unwrap();
    let decoded: SlotStamp = toml::from_str(&encoded).unwrap();
    assert_eq!(decoded, original);
    assert_eq!(decoded.device as u64, u64::MAX);
    assert_eq!(decoded.inode as u64, 1_u64 << 63);
}

#[test]
fn an_unreadable_owned_file_is_not_moved_during_a_palette_update() {
    use std::os::unix::fs::PermissionsExt as _;
    let root = tempfile::tempdir().unwrap();
    let record = root.path().join("ledger.toml");
    let path = root.path().join("theme");
    std::fs::write(&path, "personal").unwrap();
    let mut ledger = Ledger::read(&record).unwrap();
    ledger
        .put_file("user/example", &path, "old palette")
        .unwrap();
    let permissions = std::fs::metadata(&path).unwrap().permissions();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&path).is_ok() {
        // Privileged test users can bypass file permissions.
        std::fs::set_permissions(&path, permissions).unwrap();
        return;
    }
    let result = ledger.put_file("user/example", &path, "new palette");
    std::fs::set_permissions(&path, permissions).unwrap();
    assert!(result.is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "old palette");
    assert!(ledger.pending.is_none());
    assert!(ledger.take_back_checked("user/example").errors.is_empty());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "personal");
}

#[test]
fn recovery_keeps_the_previous_fingerprint_when_a_read_fails() {
    let root = tempfile::tempdir().unwrap();
    let record = root.path().join("ledger.toml");
    let path = root.path().join("theme");
    std::fs::write(&path, "personal").unwrap();
    let mut ledger = Ledger::read(&record).unwrap();
    ledger
        .put_file("user/example", &path, "old palette")
        .unwrap();
    let mut next = ledger.files[path.to_str().unwrap()].clone();
    next.hash = hash_hex(b"new palette");
    ledger
        .begin_file(path.to_str().unwrap(), &next, None)
        .unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    drop(ledger);
    assert!(Ledger::read(&record).is_err());
    assert!(
        std::fs::read_to_string(&record)
            .unwrap()
            .contains("pending")
    );
    std::fs::remove_dir(&path).unwrap();
    std::fs::write(&path, "old palette").unwrap();
    let mut recovered = Ledger::read(&record).unwrap();
    assert!(
        recovered
            .take_back_checked("user/example")
            .errors
            .is_empty()
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "personal");
    assert!(recovered.is_empty());
}

#[test]
fn recovered_backup_sync_must_succeed_before_ownership_is_committed() {
    use std::os::unix::fs::PermissionsExt as _;
    let root = tempfile::tempdir().unwrap();
    let record = root.path().join("ledger.toml");
    let path = root.path().join("theme");
    let backup = root.path().join("theme.backup");
    std::fs::write(&path, "personal").unwrap();
    let mut ledger = Ledger::read(&record).unwrap();
    let next = FileEntry {
        app: "user/example".into(),
        hash: hash_hex(b"palette"),
        backup: Some(backup.clone()),
        created_dirs: Vec::new(),
    };
    ledger
        .begin_file(path.to_str().unwrap(), &next, Some(&backup))
        .unwrap();
    std::fs::rename(&path, &backup).unwrap();
    std::fs::write(&path, "palette").unwrap();
    let permissions = std::fs::metadata(&backup).unwrap().permissions();
    std::fs::set_permissions(&backup, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::File::open(&backup).is_ok() {
        std::fs::set_permissions(&backup, permissions).unwrap();
        return;
    }
    drop(ledger);
    assert!(Ledger::read(&record).is_err());
    assert!(
        std::fs::read_to_string(&record)
            .unwrap()
            .contains("pending")
    );
    std::fs::set_permissions(&backup, permissions).unwrap();
    let mut recovered = Ledger::read(&record).unwrap();
    assert!(
        recovered
            .take_back_checked("user/example")
            .errors
            .is_empty()
    );
    assert_eq!(std::fs::read_to_string(path).unwrap(), "personal");
}

#[test]
fn restored_originals_can_be_edited_in_place_before_recovery_finishes() {
    let root = tempfile::tempdir().unwrap();
    let record = root.path().join("ledger.toml");
    let path = root.path().join("theme");
    std::fs::write(&path, "personal").unwrap();
    let mut ledger = Ledger::read(&record).unwrap();
    ledger.put_file("user/example", &path, "palette").unwrap();
    let entry = ledger.files[path.to_str().unwrap()].clone();
    ledger.begin_restore(&path, &entry).unwrap();
    std::fs::remove_file(&path).unwrap();
    crate::templates::restore::no_clobber(entry.backup.as_ref().unwrap(), &path).unwrap();
    std::fs::write(&path, "newer personal edit with a different length").unwrap();
    drop(ledger);
    let recovered = Ledger::read(&record).unwrap();
    assert!(recovered.is_empty());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "newer personal edit with a different length");
}
