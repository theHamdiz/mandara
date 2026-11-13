use super::*;
#[test]
fn interrupted_import_updates_restore_only_the_lines_that_were_written() {
    for phase in 0..3 {
        let root = tempfile::tempdir().unwrap();
        let record = root.path().join("ledger.toml");
        let path = root.path().join("app.conf");
        std::fs::write(&path, "personal = true\n").unwrap();
        let mut ledger = Ledger::read(&record).unwrap();
        ledger
            .put_block(
                "user/example",
                &path,
                &["include old"],
                super::super::super::Placement::AfterImports,
            )
            .unwrap();
        let key = path.to_str().unwrap();
        let mut next = ledger.blocks[key].clone();
        next.lines = vec!["include new".into()];
        ledger.begin_block(key, &next).unwrap();
        if phase >= 1 {
            super::super::super::remove_block(&path, &["include old"]).unwrap();
        }
        if phase >= 2 {
            super::super::super::ensure_block(
                &path,
                &["include new"],
                super::super::super::Placement::AfterImports,
            )
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
        assert_eq!(std::fs::read_to_string(path).unwrap(), "personal = true\n");
    }
}

#[test]
fn an_existing_personal_import_is_never_claimed_by_an_integration() {
    let root = tempfile::tempdir().unwrap();
    let record = root.path().join("ledger.toml");
    let path = root.path().join("app.conf");
    std::fs::write(&path, "include palette\npersonal = true\n").unwrap();
    let mut ledger = Ledger::read(&record).unwrap();
    ledger
        .put_block(
            "user/example",
            &path,
            &["include palette"],
            super::super::super::Placement::AfterImports,
        )
        .unwrap();
    assert!(ledger.take_back_checked("user/example").errors.is_empty());
    assert_eq!(
        std::fs::read_to_string(path).unwrap(),
        "include palette\npersonal = true\n"
    );
}

#[test]
fn unchanged_owned_imports_do_not_rewrite_the_config_or_the_ownership_record() {
    let root = tempfile::tempdir().unwrap();
    let record = root.path().join("ledger.toml");
    let path = root.path().join("app.conf");
    std::fs::write(&path, "personal = true\n").unwrap();
    let mut ledger = Ledger::read(&record).unwrap();
    let block = ["include palette"];
    ledger
        .put_block(
            "user/example",
            &path,
            &block,
            crate::templates::Placement::AfterImports,
        )
        .unwrap();
    let config_before = crate::templates::FileStamp::at(&path).unwrap();
    let record_before = crate::templates::FileStamp::at(&record).unwrap();
    assert!(
        !ledger
            .put_block(
                "user/example",
                &path,
                &block,
                crate::templates::Placement::AfterImports
            )
            .unwrap()
    );
    assert_eq!(
        config_before,
        crate::templates::FileStamp::at(&path).unwrap()
    );
    assert_eq!(
        record_before,
        crate::templates::FileStamp::at(&record).unwrap()
    );
}

#[test]
fn changed_imports_never_claim_a_matching_personal_line() {
    for wrapped in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let record = root.path().join("ledger.toml");
        let path = root.path().join("app.conf");
        let personal = "[main]\n  include new\npersonal = true\n";
        std::fs::write(&path, personal).unwrap();
        let mut ledger = Ledger::read(&record).unwrap();
        let old = if wrapped {
            vec!["# owned import", "[main]", "include old"]
        } else {
            vec!["include old"]
        };
        let next = if wrapped {
            vec!["# owned import", "[main]", "include new"]
        } else {
            vec!["include new"]
        };
        ledger
            .put_block(
                "user/example",
                &path,
                &old,
                crate::templates::Placement::End,
            )
            .unwrap();
        let before = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            ledger
                .put_block(
                    "user/example",
                    &path,
                    &next,
                    crate::templates::Placement::End
                )
                .unwrap_err()
                .kind(),
            std::io::ErrorKind::AlreadyExists
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
        assert!(ledger.pending.is_none());
        drop(ledger);
        let mut recovered = Ledger::read(&record).unwrap();
        assert!(
            recovered
                .take_back_checked("user/example")
                .errors
                .is_empty()
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), personal);
    }
}

#[test]
fn initial_imports_do_not_claim_part_of_a_personal_block() {
    let root = tempfile::tempdir().unwrap();
    let record = root.path().join("ledger.toml");
    let path = root.path().join("app.conf");
    let personal = "[main]\ninclude palette\npersonal = true\n";
    std::fs::write(&path, personal).unwrap();
    let mut ledger = Ledger::read(&record).unwrap();
    assert!(
        ledger
            .put_block(
                "user/example",
                &path,
                &["# owned import", "[main]", "include palette"],
                crate::templates::Placement::End
            )
            .is_err()
    );
    assert!(ledger.is_empty());
    assert!(!record.exists());
    assert_eq!(std::fs::read_to_string(path).unwrap(), personal);
}
