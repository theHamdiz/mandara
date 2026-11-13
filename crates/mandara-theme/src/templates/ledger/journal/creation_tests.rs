use super::super::hash_hex;
use super::*;
#[test]
fn interrupted_creation_cleans_the_owned_file_and_its_empty_directories() {
    for publish in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let record = root.path().join("ledger.toml");
        let directory = root.path().join("app");
        let path = directory.join("theme");
        let mut ledger = Ledger::read(&record).unwrap();
        std::fs::create_dir(&directory).unwrap();
        let next = FileEntry {
            app: "user/example".into(),
            hash: hash_hex(b"palette"),
            backup: None,
            created_dirs: vec![directory.clone()],
        };
        ledger
            .begin_file(path.to_str().unwrap(), &next, None)
            .unwrap();
        if publish {
            std::fs::write(&path, "palette").unwrap();
        }
        drop(ledger);
        let mut recovered = Ledger::read(&record).unwrap();
        assert!(
            recovered
                .take_back_checked("user/example")
                .errors
                .is_empty()
        );
        assert!(!directory.exists());
        assert!(recovered.is_empty());
    }
}

#[test]
fn an_import_interrupted_before_creation_does_not_leave_a_stuck_record() {
    let root = tempfile::tempdir().unwrap();
    let record = root.path().join("ledger.toml");
    let directory = root.path().join("app");
    let path = directory.join("config");
    let mut ledger = Ledger::read(&record).unwrap();
    std::fs::create_dir(&directory).unwrap();
    let next = BlockEntry {
        app: "user/example".into(),
        path: Some(path.clone()),
        lines: vec!["include palette".into()],
        created: true,
        created_dirs: vec![directory.clone()],
    };
    ledger.begin_block(path.to_str().unwrap(), &next).unwrap();
    drop(ledger);
    let mut recovered = Ledger::read(&record).unwrap();
    assert!(
        recovered
            .take_back_checked("user/example")
            .errors
            .is_empty()
    );
    assert!(!directory.exists());
    assert!(recovered.is_empty());
}

#[test]
fn kvantum_ownership_is_durable_before_the_selection_can_change() {
    let root = tempfile::tempdir().unwrap();
    let record = root.path().join("ledger.toml");
    let selection = root.path().join("kvantum.kvconfig");
    std::fs::write(&selection, "[General]\ntheme=Personal\n").unwrap();
    let mut ledger = Ledger::read(&record).unwrap();
    ledger
        .record_kvantum(
            &root.path().join("Mandara"),
            &root.path().join("theme"),
            &selection,
        )
        .unwrap();
    drop(ledger);
    let recovered = Ledger::read(&record).unwrap();
    let backup = recovered
        .kvantum
        .as_ref()
        .unwrap()
        .kvconfig_backup
        .as_ref()
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(backup).unwrap(),
        "[General]\ntheme=Personal\n"
    );
}

#[test]
fn pending_ownership_and_the_ledger_destination_are_protected() {
    let root = tempfile::tempdir().unwrap();
    let record = root.path().join("ledger.toml");
    let mut ledger = Ledger::read(&record).unwrap();
    assert!(
        ledger
            .put_file("user/example", &record, "replacement")
            .is_err()
    );
    assert!(
        ledger
            .put_block(
                "user/example",
                &record,
                &["include replacement"],
                crate::templates::Placement::End
            )
            .is_err()
    );
    assert!(!record.exists());
    let path = root.path().join("theme");
    let next = FileEntry {
        app: "user/example".into(),
        hash: hash_hex(b"palette"),
        backup: None,
        created_dirs: Vec::new(),
    };
    ledger
        .begin_file(path.to_str().unwrap(), &next, None)
        .unwrap();
    assert!(ledger.has("user/example"));
    assert!(!ledger.has("another"));
}
