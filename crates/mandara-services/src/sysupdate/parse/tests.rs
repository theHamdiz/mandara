//! The parsers against output recorded in clean containers of each
//! distribution (`../fixtures/`, see `fake.rs`): Arch, Debian 12, Fedora
//! 40 (dnf 4) and 45 (dnf5), openSUSE Tumbleweed and Leap 16, Void and
//! Alpine 3.19, plus Nix 2.35.

use super::*;
use crate::sysupdate::FailureKind;
use crate::sysupdate::fake::{Scenario, recording};
use crate::sysupdate::plan::{self, BackendId, Next, Step, backend};
use crate::sysupdate::protocol::clean_line;

fn feed(b: BackendId, kind: StepKind, text: &str, t: &mut Tracker) {
    t.begin_step(kind);
    for l in text.lines() {
        backend(b).parse(kind, &clean_line(l), t);
    }
}

fn run(b: BackendId, kind: StepKind, scenario: Scenario) -> Tracker {
    let mut t = Tracker::default();
    feed(b, kind, recording(b, kind, scenario).0, &mut t);
    t
}

fn fixture(path: &str) -> String {
    std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/sysupdate/fixtures").join(path)).unwrap()
}

fn tail(text: &str) -> Vec<&str> {
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(40)..].to_vec()
}

/// The step `b` runs of `kind`.
fn step(b: BackendId, kind: StepKind) -> Step {
    backend(b).steps().into_iter().find(|s| s.kind == kind).unwrap()
}

#[test]
fn pacman_download_and_install() {
    let d = run(BackendId::Pacman, StepKind::Download, Scenario::Success);
    assert_eq!((d.downloaded, d.download_total, d.total), (8, 8, 8));
    assert_eq!(d.phase, Phase::Verifying, "ends checking integrity");
    let i = run(BackendId::Pacman, StepKind::Install, Scenario::Success);
    assert_eq!((i.installed, i.total, i.changed), (8, 8, 8));
    assert_eq!((i.phase, i.hooks, i.hooks_total), (Phase::Hooks, 2, 2));
    assert_eq!(i.current, "Rebuilding certificate stores");
    assert_eq!(i.restart_package, None);
    // Mirror errors that pacman recovered from don't fail the step.
    assert_eq!(backend(BackendId::Pacman).after(&step(BackendId::Pacman, StepKind::Download), Some(0), &d), Next::Continue);
}

#[test]
fn pacman_up_to_date_and_locked() {
    let u = run(BackendId::Pacman, StepKind::Download, Scenario::UpToDate);
    assert!(u.nothing_to_do);
    assert_eq!(backend(BackendId::Pacman).after(&step(BackendId::Pacman, StepKind::Download), Some(0), &u), Next::Finish);
    let locked = recording(BackendId::Pacman, StepKind::Download, Scenario::Locked).0;
    assert_eq!(backend(BackendId::Pacman).classify(&tail(locked)), FailureKind::Locked);
}

#[test]
fn pacman_with_a_terminal_counts_too() {
    let mut t = Tracker::default();
    t.begin_step(StepKind::Install);
    for l in [":: Processing package changes...", "( 1/12) upgrading linux                     [###] 100%", "( 2/12) installing linux-headers", ":: Running post-transaction hooks...", "(1/5) Arming ConditionNeedsUpdate..."] {
        pacman(StepKind::Install, l, &mut t);
    }
    assert_eq!((t.installed, t.total, t.changed), (2, 12, 2));
    assert_eq!(t.restart_package.as_deref(), Some("linux"));
    assert_eq!((t.hooks, t.hooks_total), (1, 5));
}

#[test]
fn apt_refresh_download_install() {
    let r = run(BackendId::Apt, StepKind::Refresh, Scenario::Success);
    assert_eq!(r.phase, Phase::Refreshing);
    assert!(r.percent.is_some_and(|p| p > 99.0));
    let d = run(BackendId::Apt, StepKind::Download, Scenario::Success);
    assert_eq!((d.total, d.download_total), (46, 46));
    assert!(d.downloaded >= 44 && d.phase == Phase::Downloading);
    let i = run(BackendId::Apt, StepKind::Install, Scenario::Success);
    assert_eq!((i.installed, i.changed), (46, 46));
    assert_eq!(i.restart_package.as_deref(), Some("libc6"), "the C library was updated");
    assert_eq!(i.phase, Phase::Hooks, "ends processing triggers");
}

#[test]
fn apt_status_fields() {
    assert_eq!(apt_status("libc6:amd64:12.5:Installing libc6 (amd64)"), Some(("libc6:amd64", 12.5, "Installing libc6 (amd64)")));
    assert_eq!(apt_status("7:41.8475:Retrieving file 7 of 9 (12s remaining)"), Some(("7", 41.8475, "Retrieving file 7 of 9 (12s remaining)")));
    assert_eq!(apt_status("garbage"), None);
}

#[test]
fn apt_up_to_date_and_locked() {
    let u = run(BackendId::Apt, StepKind::Download, Scenario::UpToDate);
    assert!(u.nothing_to_do && !u.planned);
    assert_eq!(backend(BackendId::Apt).after(&step(BackendId::Apt, StepKind::Download), Some(0), &u), Next::Finish);
    let locked = recording(BackendId::Apt, StepKind::Refresh, Scenario::Locked).0;
    assert_eq!(backend(BackendId::Apt).classify(&tail(locked)), FailureKind::Locked);
}

#[test]
fn dnf5_download_and_install() {
    let d = run(BackendId::Dnf, StepKind::Download, Scenario::Success);
    assert_eq!(d.total, 15, "1 installed + 14 upgraded (replaced ones aren't counted twice)");
    assert_eq!((d.downloaded, d.download_total), (15, 15));
    let i = run(BackendId::Dnf, StepKind::Install, Scenario::Success);
    assert_eq!((i.installed, i.total), (31, 31));
    assert_eq!(i.changed, 15);
    assert!(run(BackendId::Dnf, StepKind::Download, Scenario::UpToDate).nothing_to_do);
}

#[test]
fn dnf4_download_and_install() {
    let mut d = Tracker::default();
    feed(BackendId::Dnf, StepKind::Download, &fixture("dnf4/download.txt"), &mut d);
    assert_eq!(d.total, 38);
    assert_eq!((d.downloaded, d.download_total), (38, 38));
    let mut i = Tracker::default();
    feed(BackendId::Dnf, StepKind::Install, &fixture("dnf4/install.txt"), &mut i);
    assert_eq!(i.total, 76);
    assert_eq!(i.changed, 38);
    assert_eq!(i.restart_package.as_deref(), Some("glibc"));
    let mut u = Tracker::default();
    feed(BackendId::Dnf, StepKind::Download, &fixture("dnf4/uptodate.txt"), &mut u);
    assert!(u.nothing_to_do);
}

#[test]
fn rpm_names() {
    assert_eq!(rpm_name("glibc-2.39-38.fc40.x86_64"), "glibc");
    assert_eq!(rpm_name("xz-libs-1:5.8.1-2.fc40.x86_64"), "xz-libs");
    assert_eq!(epoch_name("pcre2-syntax-0:10.49-"), Some("pcre2-syntax"));
    assert_eq!(epoch_name("kernel-core-0:6.17.7-300.fc45.x86_64"), Some("kernel-core"));
    assert_eq!(epoch_name("libdnf5-plugin-expir"), None);
}

#[test]
fn zypper_tumbleweed() {
    let d = run(BackendId::ZypperDup, StepKind::Download, Scenario::Success);
    assert_eq!((d.total, d.download_total, d.downloaded), (13, 12, 12));
    let i = run(BackendId::ZypperDup, StepKind::Install, Scenario::Success);
    assert_eq!((i.installed, i.total, i.changed), (12, 12, 12));
    assert_eq!(i.phase, Phase::Hooks);
    assert!(run(BackendId::ZypperDup, StepKind::Download, Scenario::UpToDate).nothing_to_do);
}

#[test]
fn zypper_carries_on_past_broken_repositories() {
    let z = backend(BackendId::ZypperDup);
    let mut t = Tracker::default();
    feed(BackendId::ZypperDup, StepKind::Refresh, &fixture("zypper/broken-repo.txt"), &mut t);
    assert_eq!(t.warnings, 1);
    assert_eq!(z.after(&step(BackendId::ZypperDup, StepKind::Refresh), Some(4), &t), Next::Continue);
    // Leap 16 with its main repository unreachable: refresh exits 4,
    // the update skips it (106) and has nothing to do.
    let mut r = Tracker::default();
    feed(BackendId::ZypperUpdate, StepKind::Refresh, &fixture("zypper/leap-refresh-skipped.txt"), &mut r);
    assert!(r.warnings >= 1);
    let mut u = Tracker::default();
    feed(BackendId::ZypperUpdate, StepKind::Install, &fixture("zypper/leap-update-skipped.txt"), &mut u);
    assert!(u.nothing_to_do);
    assert_eq!(backend(BackendId::ZypperUpdate).after(&step(BackendId::ZypperUpdate, StepKind::Install), Some(plan::ZYPPER_REPOS_SKIPPED), &u), Next::Continue);
    // A refresh that failed without skipping anything is a failure.
    assert!(matches!(z.after(&step(BackendId::ZypperDup, StepKind::Refresh), Some(4), &Tracker::default()), Next::Fail(_)));
}

#[test]
fn xbps_download_and_install() {
    let d = run(BackendId::Xbps, StepKind::Download, Scenario::Success);
    assert_eq!((d.downloaded, d.download_total), (49, 49));
    let i = run(BackendId::Xbps, StepKind::Install, Scenario::Success);
    assert_eq!(i.total, 59, "8 installed + 51 updated");
    assert_eq!((i.installed, i.changed), (59, 59));
    assert_eq!((i.hooks, i.hooks_total), (59, 59));
    let mut one = Tracker::default();
    feed(BackendId::Xbps, StepKind::Install, &fixture("xbps/install-one.txt"), &mut one);
    assert_eq!((one.total, one.installed, one.changed), (1, 1, 1), "the old version's \"updating to\" line isn't counted twice");
}

#[test]
fn xbps_up_to_date_failures_and_self_update() {
    let x = backend(BackendId::Xbps);
    let u = run(BackendId::Xbps, StepKind::Download, Scenario::UpToDate);
    assert!(!u.planned);
    assert_eq!(x.after(&step(BackendId::Xbps, StepKind::Download), Some(0), &u), Next::Finish);
    let failed = recording(BackendId::Xbps, StepKind::Download, Scenario::Fails).0;
    assert_eq!(x.classify(&tail(failed)), FailureKind::Network);
    let mut t = Tracker::default();
    t.begin_step(StepKind::Download);
    // Recorded from xbps 0.59 on an old Void image.
    for l in fixture("xbps/self-update-needed.txt").lines() {
        xbps(StepKind::Download, l, &mut t);
    }
    assert!(t.self_update_needed);
    let Next::Insert(s) = x.after(&step(BackendId::Xbps, StepKind::Download), Some(16), &t) else { panic!("xbps updates itself first") };
    assert_eq!(s.display(), "xbps-install -uy xbps");
    t.restarts = 1;
    assert_eq!(x.after(&s, Some(0), &t), Next::Insert(step(BackendId::Xbps, StepKind::Download)));
}

#[test]
fn apk_upgrade() {
    let i = run(BackendId::Apk, StepKind::Install, Scenario::Success);
    assert_eq!((i.installed, i.total), (9, 9));
    assert_eq!(i.changed, 10, "apk-tools first, then nine packages");
    assert_eq!(i.restart_package.as_deref(), Some("musl"));
    assert!(run(BackendId::Apk, StepKind::Install, Scenario::UpToDate).nothing_to_do);
}

#[test]
fn nix_build_progress() {
    let b = run(BackendId::Nix, StepKind::Build, Scenario::Success);
    assert_eq!((b.downloaded, b.download_total), (5, 5), "five paths copied from the cache");
    assert_eq!(b.phase, Phase::Building);
    assert_eq!(b.counts(), Some((5, 5)));
    let mut built = Tracker::default();
    feed(BackendId::Nix, StepKind::Build, &fixture("nix/build.txt"), &mut built);
    assert!(built.built == 2 && built.build_total >= 2, "{} of {}", built.built, built.build_total);
    assert!(built.current.starts_with("mandara-"), "{}", built.current);
    let gc = run(BackendId::Nix, StepKind::CollectGarbage, Scenario::Success);
    assert_eq!(gc.freed, "15.4 KiB");
}

#[test]
fn counts_and_fractions() {
    let mut t = Tracker { phase: Phase::Installing, installed: 3, total: 12, ..Tracker::default() };
    assert_eq!(t.counts(), Some((3, 12)));
    assert_eq!(t.fraction(), Some(0.25));
    t.percent = Some(150.0);
    assert_eq!(t.fraction(), Some(1.0));
    t.percent = None;
    t.phase = Phase::Refreshing;
    assert_eq!(t.counts(), None);
    let long = "x".repeat(500);
    t.set_current(&long);
    assert_eq!(t.current.len(), 120);
}

/// The step of FreeBSD's plan that runs `program` for `kind`.
fn freebsd_step(kind: StepKind, program: &str) -> Step {
    backend(BackendId::Pkg).steps().into_iter().find(|s| s.kind == kind && s.program == program).unwrap()
}

fn replay_step(step: &Step, scenario: Scenario) -> (Tracker, i32) {
    let (text, code) = crate::sysupdate::fake::recording_of(BackendId::Pkg, step, scenario);
    let mut t = Tracker::default();
    feed(BackendId::Pkg, step.kind, text, &mut t);
    (t, code)
}

#[test]
fn pkg_refresh_download_and_install() {
    let r = run(BackendId::Pkg, StepKind::Refresh, Scenario::Success);
    assert_eq!(r.phase, Phase::Refreshing);
    assert_eq!(r.current, "FreeBSD-kmods", "the last catalogue refreshed");
    let d = run(BackendId::Pkg, StepKind::Download, Scenario::Success);
    assert_eq!((d.downloaded, d.download_total, d.total), (6, 6, 6));
    assert_eq!(d.phase, Phase::Downloading);
    assert_eq!(d.current, "sqlite3-3.54.1,1");
    assert!(!d.nothing_to_do && d.planned);
    let i = run(BackendId::Pkg, StepKind::Install, Scenario::Success);
    assert_eq!((i.installed, i.total, i.changed), (6, 6, 6));
    assert_eq!(i.phase, Phase::Installing);
    assert_eq!(i.restart_package.as_deref(), Some("drm-61-kmod"), "a kernel module asks for a restart");
    assert_eq!(i.counts(), Some((6, 6)));
}

#[test]
fn pkg_up_to_date_still_checks_the_base_system() {
    let u = run(BackendId::Pkg, StepKind::Download, Scenario::UpToDate);
    assert!(u.nothing_to_do);
    // The packages are current; freebsd-update still runs next.
    assert_eq!(backend(BackendId::Pkg).after(&freebsd_step(StepKind::Download, "pkg"), Some(0), &u), Next::Continue);
    let locked = recording(BackendId::Pkg, StepKind::Refresh, Scenario::Locked).0;
    assert_eq!(backend(BackendId::Pkg).classify(&tail(locked)), FailureKind::Locked);
}

#[test]
fn freebsd_update_fetch_and_install() {
    let fetch = freebsd_step(StepKind::Download, plan::FREEBSD_UPDATE);
    let (f, code) = replay_step(&fetch, Scenario::Success);
    assert_eq!(code, 0);
    assert_eq!((f.downloaded, f.download_total), (1, 1), "the last download line: one whole file");
    assert_eq!(f.total, 15, "files the update changes");
    assert!(f.reboot, "/boot/kernel/kernel changes: restart");
    assert!(!f.nothing_to_do);
    assert_eq!(backend(BackendId::Pkg).after(&fetch, Some(0), &f), Next::Continue);
    let install = freebsd_step(StepKind::Install, plan::FREEBSD_UPDATE);
    let (i, code) = replay_step(&install, Scenario::Success);
    assert_eq!(code, 0);
    assert_eq!(i.phase, Phase::Installing);
    assert_eq!(i.changed, 1, "the base system counts as one change");
    assert_eq!(backend(BackendId::Pkg).after(&install, Some(0), &i), Next::Continue);
}

#[test]
fn freebsd_update_nothing_to_do_or_unmanaged_finishes() {
    let fetch = freebsd_step(StepKind::Download, plan::FREEBSD_UPDATE);
    let (u, _) = replay_step(&fetch, Scenario::UpToDate);
    assert!(u.nothing_to_do && u.warnings == 0);
    assert_eq!(backend(BackendId::Pkg).after(&fetch, Some(0), &u), Next::Finish);
    // A -STABLE system: freebsd-update refuses (exit 1); the packages were
    // all there was to update, with a warning.
    let (s, code) = replay_step(&fetch, Scenario::Fails);
    assert_eq!(code, 1);
    assert!(s.nothing_to_do);
    assert_eq!(s.warnings, 1);
    assert_eq!(backend(BackendId::Pkg).after(&fetch, Some(code), &s), Next::Finish);
    // Past its end of life: what was fetched is still installed.
    let mut eol = Tracker::default();
    feed(BackendId::Pkg, StepKind::Download, &format!("{}\nWARNING: FreeBSD 14.2-RELEASE-p5 HAS PASSED ITS END-OF-LIFE DATE.\nAny security issues discovered after Tue Sep 30 20:00:00 EEST 2025\nwill not have been corrected.\n", fixture("freebsd/base-fetch.txt")), &mut eol);
    assert!(eol.end_of_life && eol.warnings == 1);
    assert_eq!(backend(BackendId::Pkg).after(&fetch, Some(1), &eol), Next::Continue);
    // Nothing fetched, run separately: exit 2.
    let install = freebsd_step(StepKind::Install, plan::FREEBSD_UPDATE);
    let (n, code) = replay_step(&install, Scenario::UpToDate);
    assert_eq!(code, plan::FREEBSD_UPDATE_NOTHING);
    assert_eq!(backend(BackendId::Pkg).after(&install, Some(code), &n), Next::Continue);
    // Any other failure stops the update.
    assert_eq!(backend(BackendId::Pkg).after(&install, Some(1), &Tracker::default()), Next::Fail(FailureKind::Other));
}

#[test]
fn freebsd_failures_are_classified() {
    let p = backend(BackendId::Pkg);
    assert_eq!(p.classify(&["pkg: https://pkg.FreeBSD.org/FreeBSD:14:amd64/quarterly/meta.txz: No address record", "Unable to update repository FreeBSD", "Error updating repositories!"]), FailureKind::Network);
    assert_eq!(p.classify(&["pkg: Not enough space in /var/cache/pkg, needed 21 MiB available 3 MiB"]), FailureKind::DiskFull);
    assert_eq!(p.classify(&["pkg: No trusted public keys found"]), FailureKind::Keyring);
    assert_eq!(p.classify(&["Looking up update.FreeBSD.org mirrors... none found.", "Fetching metadata signature for 14.4-RELEASE from update.FreeBSD.org... failed."]), FailureKind::Network);
}
