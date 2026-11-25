use super::*;

#[test]
fn an_absent_import_keeps_ownership_until_its_removal_is_synced() {
    let root = tempfile::tempdir().unwrap();
    let record = root.path().join("ledger.toml");
    let path = root.path().join("app.conf");
    std::fs::write(&path, "personal = true\n").unwrap();
    let mut ledger = Ledger::read(&record).unwrap();
    ledger
        .put_block(
            "app",
            &path,
            &["include palette"],
            crate::templates::Placement::End,
        )
        .unwrap();
    let key = path.to_str().unwrap().to_owned();
    crate::templates::remove_block(&path, &["include palette"]).unwrap();
    // A failed post-publication sync leaves this same visible file and record.
    for _ in 0..2 {
        let mut report = SyncReport::default();
        ledger.restore_block_with(key.clone(), &mut report, |target| {
            assert_eq!(target, path);
            assert_eq!(std::fs::read_to_string(target)?, "personal = true\n");
            Err(io::Error::other("injected removal sync failure"))
        });
        assert!(!report.errors.is_empty());
        assert!(ledger.has("app"));
        ledger.store(&record).unwrap();
        ledger = Ledger::read(&record).unwrap();
        assert!(ledger.has("app"));
    }
    let mut report = SyncReport::default();
    ledger.restore_block(key, &mut report);
    assert!(report.errors.is_empty());
    assert!(!ledger.has("app"));
    ledger.store(&record).unwrap();
    assert!(Ledger::read(&record).unwrap().is_empty());
    assert_eq!(std::fs::read_to_string(path).unwrap(), "personal = true\n");
}

#[test]
fn a_deleted_created_import_keeps_ownership_until_its_parent_is_synced() {
    let root = tempfile::tempdir().unwrap();
    let record = root.path().join("ledger.toml");
    let path = root.path().join("created/nested/app.conf");
    let mut ledger = Ledger::read(&record).unwrap();
    ledger
        .put_block(
            "app",
            &path,
            &["include palette"],
            crate::templates::Placement::End,
        )
        .unwrap();
    let key = path.to_str().unwrap().to_owned();
    let mut report = SyncReport::default();
    ledger.restore_block_with(key.clone(), &mut report, |target| {
        assert_eq!(target, path);
        assert!(!path.exists());
        assert!(!root.path().join("created").exists());
        Err(io::Error::other("injected removal sync failure"))
    });
    assert!(!report.errors.is_empty());
    assert!(ledger.has("app"));
    ledger.store(&record).unwrap();
    let mut recovered = Ledger::read(&record).unwrap();
    let mut report = SyncReport::default();
    recovered.restore_block_with(key, &mut report, crate::templates::file_io::sync_recovered);
    assert!(report.errors.is_empty());
    assert!(!recovered.has("app"));
    assert!(!path.exists());
}
