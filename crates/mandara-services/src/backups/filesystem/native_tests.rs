//! Opt-in checks run only on the two synthetic disks in the storage fixture VM.

use super::*;
use std::path::Path;

fn fixture() -> Vec<Mount> {
    assert_eq!(std::fs::read_to_string("/mandara-storage-fixture").unwrap(), "mandara-disposable-storage-264\n");
    assert!(rustix::process::geteuid().is_root());
    let mounts = mounted().unwrap();
    for (target, filesystem) in [("/fixture/btrfs", "btrfs"), ("/fixture/ext4", "ext4")] {
        let mount = mounts.iter().find(|mount| mount.target == Path::new(target)).unwrap();
        assert_eq!(mount.filesystem, filesystem);
        assert!(mount.target.starts_with("/fixture"));
    }
    mounts
}

fn native(args: &[&str]) {
    let status = std::process::Command::new(args[0]).args(&args[1..]).status().unwrap();
    assert!(status.success(), "{args:?}: {status}");
}

#[test]
#[ignore = "requires the disposable storage VM marker and synthetic Btrfs/ext4 disks"]
fn native_storage_vm_snapshots_and_maintenance() {
    let _lock = crate::backups::native_fixture::lock();
    let mounts = fixture();
    let mount = mounts.iter().find(|mount| mount.target == Path::new("/fixture/btrfs")).unwrap().clone();
    let cancel = AtomicBool::new(false);
    let mut fixture = crate::backups::native_fixture::Fixture::new(&mount.target);
    fixture.default_subvolume(&mount.target);
    let source = fixture.path().join("source");
    let directory = fixture.path().join("snapshots");
    native(&["btrfs", "subvolume", "create", source.to_str().unwrap()]);
    fixture.subvolume(&source);
    std::fs::create_dir(&directory).unwrap();
    std::fs::write(source.join("original"), "snapshot content").unwrap();
    btrfs::apply(&btrfs::SystemRunner, &Request::Create { mount: mount.clone(), source: source.clone(), directory: directory.clone(), name: "reviewed".into() }, &mounted().unwrap(), &cancel).unwrap();
    fixture.subvolume(&directory.join("reviewed"));
    let reply = btrfs::inspect(&btrfs::SystemRunner, &mount, &mounted().unwrap(), &cancel).unwrap();
    assert_eq!(reply.snapshots.len(), 1);
    let snapshot = reply.snapshots[0].clone();
    assert!(!snapshot.protected);
    assert_eq!(std::fs::read_to_string(directory.join("reviewed/original")).unwrap(), "snapshot content");
    std::fs::write(source.join("original"), "new original").unwrap();
    assert_eq!(std::fs::read_to_string(directory.join("reviewed/original")).unwrap(), "snapshot content");

    native(&["btrfs", "subvolume", "set-default", &snapshot.id.to_string(), mount.target.to_str().unwrap()]);
    let protected = btrfs::inspect(&btrfs::SystemRunner, &mount, &mounted().unwrap(), &cancel).unwrap().snapshots.remove(0);
    assert!(protected.protected);
    assert!(btrfs::apply(&btrfs::SystemRunner, &Request::Delete { mount: mount.clone(), snapshot: snapshot.clone() }, &mounted().unwrap(), &cancel).is_err());
    native(&["btrfs", "subvolume", "set-default", "5", mount.target.to_str().unwrap()]);
    let active = fixture.path().join("active");
    std::fs::create_dir(&active).unwrap();
    fixture.mount(&active);
    native(&["mount", "-t", "btrfs", "-o", &format!("subvolid={}", snapshot.id), "/dev/vda", active.to_str().unwrap()]);
    assert!(btrfs::inspect(&btrfs::SystemRunner, &mount, &mounted().unwrap(), &cancel).unwrap().snapshots[0].protected);
    assert!(btrfs::apply(&btrfs::SystemRunner, &Request::Delete { mount: mount.clone(), snapshot: snapshot.clone() }, &mounted().unwrap(), &cancel).is_err());
    native(&["umount", active.to_str().unwrap()]);
    let mut stale = snapshot.clone();
    stale.uuid = "44444444-4444-4444-4444-444444444444".into();
    assert!(btrfs::apply(&btrfs::SystemRunner, &Request::Delete { mount: mount.clone(), snapshot: stale }, &mounted().unwrap(), &cancel).is_err());

    std::fs::create_dir(source.join("foreign")).unwrap();
    fixture.mount(&source.join("foreign"));
    native(&["mount", "--bind", "/fixture/ext4", source.join("foreign").to_str().unwrap()]);
    assert!(btrfs::apply(&btrfs::SystemRunner, &Request::Create { mount: mount.clone(), source: source.clone(), directory: source.join("foreign"), name: "outside".into() }, &mounted().unwrap(), &cancel).is_err());
    native(&["umount", source.join("foreign").to_str().unwrap()]);

    fixture.mount(&mount.target);
    native(&["mount", "--bind", "/fixture/ext4", mount.target.to_str().unwrap()]);
    let stacked = mounted().unwrap();
    for request in [Request::Inspect { mount: mount.clone() }, Request::Delete { mount: mount.clone(), snapshot: snapshot.clone() }, Request::Maintain { mount: mount.clone(), operation: Maintenance::Trim }] {
        assert!(btrfs::apply(&btrfs::SystemRunner, &request, &stacked, &cancel).is_err());
    }
    native(&["umount", mount.target.to_str().unwrap()]);

    btrfs::apply(&btrfs::SystemRunner, &Request::Delete { mount: mount.clone(), snapshot }, &mounted().unwrap(), &cancel).unwrap();
    assert!(btrfs::inspect(&btrfs::SystemRunner, &mount, &mounted().unwrap(), &cancel).unwrap().snapshots.is_empty());
    assert_eq!(std::fs::read_to_string(source.join("original")).unwrap(), "new original");
    for operation in [Maintenance::Scrub, Maintenance::ReclaimEmptyChunks, Maintenance::Trim] {
        btrfs::apply(&btrfs::SystemRunner, &Request::Maintain { mount: mount.clone(), operation }, &mounted().unwrap(), &cancel).unwrap();
    }
    let ext4 = mounts.iter().find(|mount| mount.target == Path::new("/fixture/ext4")).unwrap();
    btrfs::apply(&btrfs::SystemRunner, &Request::Maintain { mount: ext4.clone(), operation: Maintenance::Trim }, &mounted().unwrap(), &cancel).unwrap();
}

#[test]
#[ignore = "requires the disposable storage VM marker and synthetic Btrfs/ext4 disks"]
fn native_storage_vm_fixture_cleanup_runs_during_unwinding() {
    let _lock = crate::backups::native_fixture::lock();
    fixture();
    let root = std::cell::RefCell::new(None);
    let original_default = std::cell::RefCell::new(Vec::new());
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let mut fixture = crate::backups::native_fixture::Fixture::new(Path::new("/fixture/btrfs"));
        fixture.default_subvolume(Path::new("/fixture/btrfs"));
        *original_default.borrow_mut() = std::process::Command::new("btrfs").args(["subvolume", "get-default", "/fixture/btrfs"]).output().unwrap().stdout;
        *root.borrow_mut() = Some(fixture.path().to_path_buf());
        let source = fixture.path().join("source");
        native(&["btrfs", "subvolume", "create", source.to_str().unwrap()]);
        fixture.subvolume(&source);
        let output = std::process::Command::new("btrfs").args(["subvolume", "show"]).arg(&source).output().unwrap();
        assert!(output.status.success());
        let details = String::from_utf8(output.stdout).unwrap();
        let id = details.lines().find_map(|line| line.trim().strip_prefix("Subvolume ID:")).unwrap().trim();
        native(&["btrfs", "subvolume", "set-default", id, "/fixture/btrfs"]);
        let mount = fixture.path().join("bound");
        std::fs::create_dir(&mount).unwrap();
        fixture.mount(&mount);
        native(&["mount", "--bind", "/fixture/ext4", mount.to_str().unwrap()]);
        panic!("exercise fixture cleanup after an assertion");
    }));
    assert!(result.is_err());
    let root = root.into_inner().unwrap();
    assert!(!root.exists());
    assert!(!mounted().unwrap().iter().any(|mount| mount.target.starts_with(root.as_path())));
    assert_eq!(std::process::Command::new("btrfs").args(["subvolume", "get-default", "/fixture/btrfs"]).output().unwrap().stdout, original_default.into_inner());
}
