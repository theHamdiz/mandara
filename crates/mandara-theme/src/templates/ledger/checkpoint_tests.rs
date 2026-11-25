use super::*;

#[test]
fn an_empty_checkpoint_retries_sync_even_after_its_record_was_deleted() {
    let root = tempfile::tempdir().unwrap();
    let record = root.path().join("ledger.toml");
    let path = root.path().join("theme");
    let mut ledger = Ledger::read(&record).unwrap();
    ledger.put_file("app", &path, "palette").unwrap();
    assert!(record.exists());
    std::fs::remove_file(&path).unwrap();
    super::super::file_io::sync_parent(&path).unwrap();
    ledger.files.clear();
    ledger.dirty = true;
    assert!(ledger.is_empty());
    assert!(ledger.dirty);
    for _ in 0..2 {
        assert!(
            ledger
                .checkpoint_with(|target| {
                    assert_eq!(target, record);
                    assert!(!record.exists());
                    Err(std::io::Error::other(
                        "injected record removal sync failure",
                    ))
                })
                .is_err()
        );
        assert!(ledger.dirty);
        assert!(!record.exists());
    }
    let synced = std::cell::Cell::new(false);
    ledger
        .checkpoint_with(|target| {
            super::super::file_io::sync_existing_parent(target)?;
            synced.set(true);
            Ok(())
        })
        .unwrap();
    assert!(synced.get());
    assert!(!ledger.dirty);
    assert!(!record.exists());
}

#[test]
fn empty_checkpoints_report_unreadable_parents_without_clearing_dirty() {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path().join("file");
    std::fs::write(&parent, "personal").unwrap();
    let mut ledger = Ledger {
        record: Some(parent.join("ledger.toml")),
        dirty: true,
        ..Ledger::default()
    };
    assert!(ledger.checkpoint().is_err());
    assert!(ledger.dirty);
    assert_eq!(std::fs::read_to_string(parent).unwrap(), "personal");
}
