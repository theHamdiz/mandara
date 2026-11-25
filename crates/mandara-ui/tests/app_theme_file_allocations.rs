//! An edited theme can be much larger than a generated palette file.

#[path = "support/allocations.rs"]
mod allocations;
use allocations::allocations;
use mandara_theme::templates::Ledger;

/// Large enough that a whole-file ownership read fails the allocation bound.
const EDIT_BYTES: u64 = 32 * 1024 * 1024;
/// Path, ownership and tempfile metadata fit comfortably inside this allowance.
const MAX_SYNC_ALLOCATION_BYTES: usize = 512 * 1024;

#[test]
fn a_large_user_edit_is_backed_up_without_reading_it_into_one_allocation() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("theme");
    let mut ledger = Ledger::default();
    ledger
        .put_file("community:example/main", &path, "first palette")
        .unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(EDIT_BYTES)
        .unwrap();
    let control = allocations(|| drop(std::fs::read(&path).unwrap()));
    let current = allocations(|| {
        ledger
            .put_file("community:example/main", &path, "next palette")
            .unwrap()
    });
    assert!(
        control.1 >= EDIT_BYTES as usize,
        "positive control: {control:?}"
    );
    assert!(
        current.1 < MAX_SYNC_ALLOCATION_BYTES,
        "bounded ownership check: {current:?}"
    );
    eprintln!("32 MiB edited theme: {current:?} versus {control:?} (allocation calls, bytes)");
    assert!(
        ledger
            .take_back_checked("community:example/main")
            .errors
            .is_empty()
    );
    assert_eq!(std::fs::metadata(path).unwrap().len(), EDIT_BYTES);
}
