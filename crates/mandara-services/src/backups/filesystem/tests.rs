use super::*;
#[cfg(target_os = "linux")]
use std::cell::RefCell;
#[cfg(target_os = "linux")]
use std::ffi::OsString;
#[cfg(target_os = "linux")]
use std::sync::atomic::AtomicBool;
#[cfg(target_os = "linux")]
use std::time::Duration;

fn mount(target: &std::path::Path) -> Mount {
    #[cfg(target_os = "linux")]
    let id = rustix::fs::statx(std::fs::File::open(target).unwrap(), "", rustix::fs::AtFlags::EMPTY_PATH, rustix::fs::StatxFlags::MNT_ID).unwrap().stx_mnt_id;
    #[cfg(not(target_os = "linux"))]
    let id = 8;
    Mount { id, device: "0:33".into(), root: "/".into(), target: target.into(), filesystem: "btrfs".into(), read_only: false, subvolume_id: Some(5) }
}

const SNAPSHOTS: &str = "ID 257 gen 123 top level 5 parent_uuid 11111111-1111-1111-1111-111111111111 uuid 22222222-2222-2222-2222-222222222222 path snapshots/one\nID 258 gen 124 top level 5 parent_uuid 11111111-1111-1111-1111-111111111111 uuid 33333333-3333-3333-3333-333333333333 path snapshots/two with spaces\n";

#[cfg(target_os = "linux")]
#[derive(Default)]
struct Recorder { calls: RefCell<Vec<(String, Vec<OsString>)>>, paths: RefCell<Vec<std::path::PathBuf>> }
#[cfg(target_os = "linux")]
impl btrfs::Runner for Recorder {
    fn run(&self, program: &str, args: &[OsString], _: &AtomicBool, _: Duration) -> anyhow::Result<String> {
        self.calls.borrow_mut().push((program.into(), args.to_vec()));
        self.paths.borrow_mut().push(std::path::Path::new(args.last().unwrap()).canonicalize().unwrap());
        if args.get(1).is_some_and(|arg| arg == "get-default") { return Ok("ID 5 (FS_TREE)\n".into()); }
        if args.get(1).is_some_and(|arg| arg == "list") { return Ok(SNAPSHOTS.into()); }
        Ok(String::new())
    }
}

#[test]
fn mount_tables_decode_paths_and_keep_read_only_and_subvolume_identity() {
    let rows = mounts::parse("30 1 0:33 /@ /mnt/my\\040disk rw,relatime - btrfs /dev/test rw,subvolid=256,subvol=/@\n31 1 8:1 / /media/old ro - ext4 /dev/test2 ro\n").unwrap();
    assert_eq!(rows[0].target, std::path::Path::new("/mnt/my disk"));
    assert_eq!(rows[0].subvolume_id, Some(256));
    assert_eq!(rows[0].supports_snapshots(), cfg!(target_os = "linux"));
    assert!(!rows[1].supports_trim());
    assert!(mounts::parse("incomplete").is_err());
}

#[test]
fn default_and_mounted_snapshots_are_protected_and_listing_paths_are_not_executed() {
    let root = tempfile::tempdir().unwrap();
    let base = mount(root.path());
    let mut active = base.clone(); active.id = 9; active.subvolume_id = Some(258);
    let rows = btrfs::snapshots(SNAPSHOTS, &base, &[base.clone(), active], 257).unwrap();
    assert!(rows.iter().all(|row| row.protected));
    assert_eq!(rows[1].path, std::path::Path::new("snapshots/two with spaces"));
    assert!(btrfs::snapshots(&SNAPSHOTS.replace("snapshots/one", "../root"), &base, &[], 5).is_err());
    assert!(btrfs::snapshots(&SNAPSHOTS.replace("22222222-2222-2222-2222-222222222222", "-"), &base, &[], 5).is_err());
}

#[test]
#[cfg(target_os = "linux")]
fn deletion_rechecks_identity_and_never_emits_recursive_or_default_changes() {
    let root = tempfile::tempdir().unwrap();
    let base = mount(root.path());
    let snapshot = btrfs::snapshots(SNAPSHOTS, &base, std::slice::from_ref(&base), 5).unwrap().remove(0);
    let runner = Recorder::default();
    btrfs::apply(&runner, &Request::Delete { mount: base.clone(), snapshot: snapshot.clone() }, std::slice::from_ref(&base), &AtomicBool::new(false)).unwrap();
    let calls = runner.calls.borrow();
    assert_eq!(&calls.last().unwrap().1[..5], [OsString::from("subvolume"), "delete".into(), "--commit-after".into(), "--subvolid".into(), "257".into()]);
    assert!(runner.paths.borrow().iter().all(|path| path == root.path()));
    assert!(calls.iter().all(|(_, args)| !args.iter().any(|arg| arg == "--recursive" || arg == "set-default")));
    drop(calls);
    let mut stale = snapshot; stale.uuid = "44444444-4444-4444-4444-444444444444".into();
    assert!(btrfs::apply(&runner, &Request::Delete { mount: base.clone(), snapshot: stale }, std::slice::from_ref(&base), &AtomicBool::new(false)).is_err());
    let mut changed = base.clone(); changed.id += 1;
    assert!(btrfs::apply(&runner, &Request::Inspect { mount: changed }, &[base], &AtomicBool::new(false)).is_err());
}

#[test]
#[cfg(target_os = "linux")]
fn maintenance_is_scoped_and_never_runs_an_unfiltered_balance_or_repair() {
    let root = tempfile::tempdir().unwrap();
    let base = mount(root.path());
    let runner = Recorder::default();
    for operation in [Maintenance::Scrub, Maintenance::ReclaimEmptyChunks, Maintenance::Trim] {
        btrfs::apply(&runner, &Request::Maintain { mount: base.clone(), operation }, std::slice::from_ref(&base), &AtomicBool::new(false)).unwrap();
    }
    let calls = runner.calls.borrow();
    assert_eq!(calls[0].1[0..3], ["scrub", "start", "-B"].map(OsString::from));
    assert_eq!(calls[1].1[0..4], ["balance", "start", "-dusage=0", "-musage=0"].map(OsString::from));
    assert_eq!(calls[2].0, "fstrim");
    assert!(runner.paths.borrow().iter().all(|path| path == root.path()));
    assert!(calls.iter().all(|(_, args)| !args.iter().any(|arg| arg == "--repair" || arg == "--all")));
    let mut ext4 = base; ext4.filesystem = "ext4".into();
    assert!(btrfs::apply(&runner, &Request::Maintain { mount: ext4.clone(), operation: Maintenance::Scrub }, &[ext4], &AtomicBool::new(false)).is_err());
}

#[test]
#[cfg(target_os = "linux")]
fn an_overmounted_record_is_rejected_even_when_its_path_and_table_entry_survive() {
    let root = tempfile::tempdir().unwrap();
    let visible = mount(root.path());
    let mut hidden = visible.clone();
    hidden.id += 1;
    let mounts = [visible.clone(), hidden.clone()];
    assert!(btrfs::checked_mount(&visible, &mounts).is_ok());
    let runner = Recorder::default();
    for request in [Request::Inspect { mount: hidden.clone() }, Request::Maintain { mount: hidden, operation: Maintenance::Trim }] {
        assert!(btrfs::apply(&runner, &request, &mounts, &AtomicBool::new(false)).is_err());
    }
    assert!(runner.calls.borrow().is_empty());
}

#[test]
fn administrator_requests_cannot_supply_arbitrary_actions_or_options() {
    assert!(serde_json::from_str::<Request>(r#"{"action":"format","device":"/dev/sda"}"#).is_err());
    assert!(serde_json::from_str::<Request>(r#"{"action":"maintain","mount":{},"operation":"repair"}"#).is_err());
    assert_eq!(admin::main(&[]), 1);
}
