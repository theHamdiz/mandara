//! Helper choice, the exact commands, and progress from paru's and yay's
//! recorded output.

use super::*;

macro_rules! fixture {
    ($path:literal) => {
        include_str!(concat!("../fixtures/aur/", $path))
    };
}

fn installed(names: &[AurHelper]) -> impl Fn(AurHelper) -> bool + '_ {
    move |h| names.contains(&h)
}

#[test]
fn paru_is_preferred_then_yay_then_the_rest() {
    use AurHelper::*;
    assert_eq!(pick(None, installed(&[Yay, Paru, Shelly])), Some(Paru));
    assert_eq!(pick(None, installed(&[Shelly, Yay])), Some(Yay));
    assert_eq!(pick(None, installed(&[Shelly, Trizen, Aura, Pikaur])), Some(Pikaur));
    assert_eq!(pick(None, installed(&[Shelly])), Some(Shelly));
    assert_eq!(pick(None, installed(&[])), None);
}

#[test]
fn a_preferred_helper_wins_only_when_installed() {
    use AurHelper::*;
    assert_eq!(pick(Some(Shelly), installed(&[Paru, Shelly])), Some(Shelly));
    assert_eq!(pick(Some(Yay), installed(&[Paru])), Some(Paru), "a preference for a missing helper falls back");
    assert_eq!(pick(Some(Yay), installed(&[])), None);
}

#[test]
fn the_check_uses_a_helper_that_can_list_aur_updates() {
    use AurHelper::*;
    assert_eq!(checker(installed(&[Shelly, Yay])), Some(Yay));
    assert_eq!(checker(installed(&[Shelly, Aura])), None, "Shelly and Aura are checked through the AUR's web API");
    assert_eq!(checker(installed(&[Trizen, Pikaur])), Some(Pikaur));
}

#[test]
fn ids_round_trip() {
    for h in AurHelper::ALL {
        assert_eq!(AurHelper::from_id(h.id()), Some(h));
        assert_eq!(h.step().program, h.id());
        assert_eq!(h.step().kind, StepKind::Aur);
    }
    assert_eq!(AurHelper::from_id("PARU"), Some(AurHelper::Paru));
    assert_eq!(AurHelper::from_id("pamac"), None);
}

/// The exact commands: any change here changes what runs on your system.
#[test]
fn fixed_unattended_commands() {
    let shown = |h: AurHelper| h.step().display();
    assert_eq!(shown(AurHelper::Paru), "paru -Sua --noconfirm --skipreview --noupgrademenu --sudo sudo --sudoloop --color never");
    assert_eq!(shown(AurHelper::Yay), "yay -Sua --noconfirm --answerclean None --answerdiff None --answeredit None --answerupgrade None --sudo sudo --sudoloop");
    assert_eq!(shown(AurHelper::Pikaur), "pikaur -Sua --noconfirm --noedit --nodiff");
    assert_eq!(shown(AurHelper::Aura), "aura -Au --noconfirm");
    assert_eq!(shown(AurHelper::Trizen), "trizen -Sua --noconfirm --noedit --noinfo --nocolors --sudo_command=sudo");
    assert_eq!(shown(AurHelper::Shelly), "shelly upgrade aur --no-confirm");
    assert_eq!(AurHelper::Shelly.step().env, &[("SHELLY_ELEVATOR", "sudo")]);
    for h in AurHelper::ALL {
        let s = h.step();
        assert!(!s.program.contains('/'));
        for a in &s.args {
            let a = a.to_ascii_lowercase();
            assert!(!a.contains("overwrite") && !a.contains("nodeps") && !a.contains("force") && !a.contains("skipinteg") && !a.contains("skippgp"), "{h:?}: {a}");
        }
        // AUR packages only: never a full system upgrade (`-Syu`), which
        // is the system step's.
        assert!(!s.args.iter().any(|a| a.starts_with("-Sy") || a == "-Syu"), "{h:?}");
    }
    assert_eq!(AurHelper::Paru.terminal_command(), "paru -Sua");
    assert_eq!(AurHelper::Shelly.terminal_command(), "shelly upgrade aur");
}

fn replay(text: &str) -> Tracker {
    let mut t = Tracker::default();
    t.begin_step(StepKind::Aur);
    for l in text.lines() {
        parse_line(&crate::sysupdate::protocol::clean_line(l), &mut t);
    }
    t
}

#[test]
fn paru_upgrade_progress() {
    let t = replay(fixture!("paru-upgrade.txt"));
    assert_eq!(t.build_total, 3, "the plan's Aur (3)");
    assert_eq!(t.built, 3);
    assert_eq!(t.changed, 3, "cbonsai, nerdfetch and pfetch upgraded");
    assert_eq!(t.failed_package, None);
    assert!(!t.nothing_to_do);
}

#[test]
fn yay_upgrade_progress() {
    let t = replay(fixture!("yay-upgrade.txt"));
    assert_eq!(t.build_total, 3, "yay's `3 packages to upgrade`");
    assert_eq!(t.built, 3, "one was built already (`already made -- skipping build`)");
    assert_eq!(t.changed, 4, "the three, and cbonsai's debug package");
    assert_eq!(t.failed_package, None);
}

#[test]
fn progress_names_what_is_building_downloading_and_installing() {
    let mut t = Tracker::default();
    t.begin_step(StepKind::Aur);
    parse_line("==> Making package: nerdfetch 8.5.5-1 (Tue Nov 10 00:30:33 2026)", &mut t);
    assert_eq!((t.phase, t.current.as_str()), (Phase::AurBuild, "nerdfetch"));
    parse_line("==> Retrieving sources...", &mut t);
    assert_eq!((t.phase, t.current.as_str()), (Phase::Downloading, "nerdfetch"));
    parse_line("==> Starting package()...", &mut t);
    assert_eq!(t.phase, Phase::AurBuild);
    parse_line(":: Processing package changes...", &mut t);
    parse_line("upgrading nerdfetch...", &mut t);
    assert_eq!((t.phase, t.current.as_str()), (Phase::Installing, "nerdfetch"));
    // A repository package installed for the build doesn't count as built.
    assert_eq!(t.built, 0);
}

#[test]
fn up_to_date() {
    for text in [fixture!("paru-uptodate.txt"), fixture!("yay-uptodate.txt")] {
        let t = replay(text);
        assert!(t.nothing_to_do);
        assert_eq!((t.built, t.changed), (0, 0));
    }
}

#[test]
fn a_failed_build_names_its_package() {
    for text in [fixture!("paru-buildfail.txt"), fixture!("yay-buildfail.txt")] {
        let t = replay(text);
        assert_eq!(t.failed_package.as_deref(), Some("cbonsai"));
        assert_eq!(t.changed, 0, "nothing of the AUR was installed");
        let lines: Vec<&str> = text.lines().rev().take(8).collect();
        assert_eq!(classify(&lines), FailureKind::BuildFailed);
    }
    assert_eq!(failed_package("error: failed to build 'python-foo-bar-1:2.0.r3-1': "), Some("python-foo-bar"));
    assert_eq!(failed_package("-> error making: yay-git-exit status 4"), Some("yay-git"));
    assert_eq!(strip_version("cbonsai-1.4.2-1"), "cbonsai");
    assert_eq!(strip_version("weird"), "weird");
}

#[test]
fn failures_that_need_a_person_are_told_apart() {
    assert_eq!(classify(&["sudo: a terminal is required to read the password; either use the -S option to read from standard input or configure an askpass helper"]), FailureKind::NoPrompt);
    assert_eq!(classify(&["sudo: 3 incorrect password attempts"]), FailureKind::NotAuthorized);
    assert_eq!(classify(&["sudo: no password was provided", "sudo: a password is required"]), FailureKind::NotAuthorized);
    assert_eq!(classify(&[":: foo-git and foo are in conflict. Remove foo? [y/N]", "error: unresolvable package conflicts detected"]), FailureKind::Conflict);
    assert_eq!(classify(&["==> ERROR: One or more PGP signatures could not be verified!"]), FailureKind::Keyring);
    assert_eq!(classify(&["error: failed to download sources for 'foo-1-1': "]), FailureKind::Network);
    assert_eq!(classify(&["something unexpected"]), FailureKind::Other);
    assert!(needs_terminal(FailureKind::Conflict) && needs_terminal(FailureKind::BuildFailed));
    assert!(!needs_terminal(FailureKind::NotAuthorized) && !needs_terminal(FailureKind::Network));
}
