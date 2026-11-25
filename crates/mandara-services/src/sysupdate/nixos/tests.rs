//! NixOS: argument validation, generations from a fixture profile tree,
//! the root side's plans, and output recorded with Nix 2.35 in a
//! `nixos/nix` container (flake update, internal-json builds, closure
//! diffs, garbage collection).

use super::*;
use crate::sysupdate::fake::nixos_tree;

fn tree() -> (tempfile::TempDir, NixPaths) {
    let dir = tempfile::tempdir().unwrap();
    let paths = nixos_tree(dir.path()).unwrap();
    (dir, paths)
}

fn fixture(path: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/sysupdate/fixtures")
            .join(path),
    )
    .unwrap()
}

fn shown(steps: &[Step]) -> Vec<String> {
    steps
        .iter()
        .map(|s| format!("{}: {}", s.kind.id(), s.display()))
        .collect()
}

#[test]
fn actions_are_strictly_validated() {
    use NixAction::*;
    assert_eq!(
        NixAction::parse(&["update", "switch"]),
        Ok(Update(RebuildMode::Switch))
    );
    assert_eq!(
        NixAction::parse(&["rebuild", "boot"]),
        Ok(Rebuild(RebuildMode::Boot))
    );
    assert_eq!(NixAction::parse(&["rollback"]), Ok(Rollback));
    assert_eq!(NixAction::parse(&["switch-to", "41"]), Ok(SwitchTo(41)));
    assert_eq!(
        NixAction::parse(&["clean", "keep", "5"]),
        Ok(Clean(Cleanup::KeepLatest(5)))
    );
    assert_eq!(
        NixAction::parse(&["clean", "older-than", "30"]),
        Ok(Clean(Cleanup::OlderThanDays(30)))
    );
    let bad: &[&[&str]] = &[
        &[],
        &["update"],
        &["update", "now"],
        &["update", "switch", "--flake", "/tmp/evil#x"],
        &["rebuild", "switch", "extra"],
        &["switch-to", "-1"],
        &["switch-to", "+5"],
        &["switch-to", " 5"],
        &["switch-to", "0x10"],
        &["switch-to", "1e3"],
        &["switch-to", "9999999999"],
        &["switch-to", "0"],
        &["switch-to", "../../evil"],
        &["clean", "keep", "0"],
        &["clean", "keep", "101"],
        &["clean", "older-than", "0"],
        &["clean", "older-than", "3651"],
        &["clean", "all"],
        &["rollback", "now"],
        &["sh", "-c", "id"],
    ];
    for words in bad {
        assert!(
            NixAction::parse(words).is_err(),
            "{words:?} must be refused"
        );
    }
}

#[test]
fn helper_arguments_round_trip() {
    let all = [
        NixAction::Update(RebuildMode::Switch),
        NixAction::Update(RebuildMode::Build),
        NixAction::Rebuild(RebuildMode::Test),
        NixAction::Rollback,
        NixAction::SwitchTo(42),
        NixAction::Clean(Cleanup::KeepLatest(3)),
        NixAction::Clean(Cleanup::OlderThanDays(14)),
    ];
    for a in all {
        let args = a.args();
        assert_eq!(args[0], "nixos");
        let words: Vec<&str> = args[1..].iter().map(String::as_str).collect();
        assert_eq!(NixAction::parse(&words), Ok(a));
    }
}

#[test]
fn reads_generations() {
    let (_dir, paths) = tree();
    let gens = generations(&paths);
    assert_eq!(
        gens.iter().map(|g| g.number).collect::<Vec<_>>(),
        [44, 43, 42, 41, 40, 39, 38]
    );
    assert!(gens[0].current && !gens[0].booted);
    assert!(gens[1].booted && !gens[1].current);
    assert_eq!(gens[0].kernel, "6.17.7");
    assert_eq!(gens[0].nixos_version, "25.11.20251106.e5f6a7b8");
    assert!(gens[0].date > gens[1].date);
    let home = home_generations(&[paths
        .profiles
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("home/.local/state/nix/profiles")]);
    assert_eq!(
        home.iter()
            .map(|g| (g.number, g.current))
            .collect::<Vec<_>>(),
        [(12, true), (11, false)]
    );
    assert!(home_generations(&[PathBuf::from("/nonexistent")]).is_empty());
}

#[test]
fn link_names_and_store_names() {
    assert_eq!(link_number("system-42-link", "system"), Some(42));
    assert_eq!(link_number("system-42-link.tmp", "system"), None);
    assert_eq!(link_number("system--1-link", "system"), None);
    assert_eq!(link_number("home-manager-7-link", "home-manager"), Some(7));
    assert_eq!(
        store_name("0c5mr2ljm3b4kx4ij2yr6dqyqv5p6fla-linux-6.6.30"),
        "linux-6.6.30"
    );
    assert_eq!(store_name("not-a-store-name"), "not-a-store-name");
    assert_eq!(
        short_path("/nix/store/4vamsg393v3yj3q4cmj68l0124km0saa-mandara-demo.drv"),
        "mandara-demo"
    );
}

#[test]
fn clean_up_never_deletes_the_current_or_booted_generation() {
    let (_dir, paths) = tree();
    let mut gens = generations(&paths);
    let now = chrono::Utc::now().timestamp();
    assert_eq!(
        to_delete(&gens, Cleanup::KeepLatest(2), now),
        [38, 39, 40, 41, 42]
    );
    assert_eq!(
        to_delete(&gens, Cleanup::KeepLatest(1), now),
        [38, 39, 40, 41, 42],
        "43 is booted"
    );
    assert_eq!(
        to_delete(&gens, Cleanup::KeepLatest(100), now),
        Vec::<u32>::new()
    );
    assert_eq!(
        to_delete(&gens, Cleanup::OlderThanDays(10), now),
        [38, 39, 40, 41, 42]
    );
    assert_eq!(
        to_delete(&gens, Cleanup::OlderThanDays(3650), now),
        Vec::<u32>::new()
    );
    // An old booted generation stays too.
    for g in &mut gens {
        g.booted = g.number == 39;
    }
    assert_eq!(
        to_delete(&gens, Cleanup::KeepLatest(1), now),
        [38, 40, 41, 42, 43]
    );
}

#[test]
fn update_plans_a_user_flake_build_and_switch() {
    let (_dir, paths) = tree();
    let Setup::Flake {
        dir,
        root_owned,
        host,
    } = setup(&paths)
    else {
        panic!("a flake system")
    };
    assert!(!root_owned, "the fixture is yours, not root's");
    assert_eq!(host, "mandara");
    let d = dir.to_string_lossy();
    let steps = plan(&NixAction::Update(RebuildMode::Switch), &paths).unwrap();
    assert_eq!(
        shown(&steps),
        [
            format!(
                "build: nix --extra-experimental-features nix-command flakes build --log-format internal-json -v --no-link --print-out-paths {d}#nixosConfigurations.\"mandara\".config.system.build.toplevel"
            ),
            format!("activate: nixos-rebuild switch --flake {d}#mandara"),
        ],
        "root never updates a flake you own: you do, before the helper"
    );
    assert_eq!(
        user_flake_update(&dir).display(),
        format!("nix --extra-experimental-features nix-command flakes flake update --flake {d}")
    );
    let build_only = plan(&NixAction::Rebuild(RebuildMode::Build), &paths).unwrap();
    assert_eq!(build_only.len(), 1);
    assert_eq!(build_only[0].kind, StepKind::Build);
    let boot = plan(&NixAction::Rebuild(RebuildMode::Boot), &paths).unwrap();
    assert_eq!(boot[1].args[0], "boot");
}

#[test]
fn root_trusts_only_the_flakes_own_git_work_tree() {
    let (dir, paths) = tree();
    let steps = plan(&NixAction::Rebuild(RebuildMode::Switch), &paths).unwrap();
    assert!(
        steps.iter().all(|s| s.git_safe_dir.is_none()),
        "not a Git repository: nothing to trust"
    );
    // The flake lives in a subfolder of a repository you own (`/etc/nixos`
    // linking into it, or the repository itself): Nix opens the
    // repository's top, so that is what root's Git must accept.
    std::fs::create_dir(dir.path().join("etc/.git")).unwrap();
    let work_tree = std::fs::canonicalize(dir.path().join("etc")).unwrap();
    let steps = plan(&NixAction::Update(RebuildMode::Switch), &paths).unwrap();
    assert_eq!(
        steps
            .iter()
            .map(|s| (s.kind, s.git_safe_dir.clone()))
            .collect::<Vec<_>>(),
        [
            (StepKind::Build, Some(work_tree.clone())),
            (StepKind::Activate, Some(work_tree.clone()))
        ]
    );
    // A `.git` file (a worktree or submodule) counts as Nix counts it.
    std::fs::remove_dir(dir.path().join("etc/.git")).unwrap();
    std::fs::write(paths.etc_nixos.join(".git"), "gitdir: /elsewhere\n").unwrap();
    let steps = plan(&NixAction::Rebuild(RebuildMode::Build), &paths).unwrap();
    assert_eq!(
        steps[0].git_safe_dir,
        Some(std::fs::canonicalize(&paths.etc_nixos).unwrap())
    );
    // Nothing else root runs gets it.
    for action in [
        NixAction::Rollback,
        NixAction::Clean(Cleanup::KeepLatest(2)),
        NixAction::SwitchTo(42),
    ] {
        assert!(
            plan(&action, &paths)
                .unwrap()
                .iter()
                .all(|s| s.git_safe_dir.is_none()),
            "{action:?}"
        );
    }
}

#[test]
fn a_bad_host_name_is_refused() {
    let (_dir, paths) = tree();
    std::fs::write(&paths.hostname, "evil\"; rm -rf /\n").unwrap();
    assert!(plan(&NixAction::Update(RebuildMode::Switch), &paths).is_err());
}

#[test]
fn channels_plan_with_a_fixed_nix_path() {
    let (_dir, paths) = tree();
    std::fs::remove_file(paths.etc_nixos.join("flake.nix")).unwrap();
    assert!(matches!(setup(&paths), Setup::Channels { .. }));
    let steps = plan(&NixAction::Update(RebuildMode::Switch), &paths).unwrap();
    assert_eq!(
        shown(&steps),
        [
            "channel-update: nix-channel --update",
            "build: nix-build --log-format internal-json -v --no-out-link <nixpkgs/nixos> -A system",
            "activate: nixos-rebuild switch"
        ]
    );
    assert!(steps.iter().all(|s| s.env == NIX_PATH));
    assert_eq!(
        shown(&plan(&NixAction::Rollback, &paths).unwrap()),
        shown(&plan(&NixAction::SwitchTo(43), &paths).unwrap())
    );
}

#[test]
fn switching_needs_an_existing_other_generation() {
    let (_dir, paths) = tree();
    let steps = plan(&NixAction::SwitchTo(41), &paths).unwrap();
    assert_eq!(
        steps[0].display(),
        format!(
            "nix-env -p {} --switch-generation 41",
            paths.system_profile().display()
        )
    );
    assert_eq!(steps[1].program, "systemd-run");
    assert!(steps[1].args.last().is_some_and(|a| a == "switch"));
    assert!(
        steps[1]
            .args
            .iter()
            .any(|a| a.ends_with("bin/switch-to-configuration"))
    );
    assert!(
        plan(&NixAction::SwitchTo(44), &paths).is_err(),
        "already current"
    );
    assert!(
        plan(&NixAction::SwitchTo(99), &paths).is_err(),
        "doesn't exist"
    );
}

#[test]
fn rollback_uses_the_previous_existing_generation_without_a_configuration_source() {
    let (_dir, paths) = tree();
    std::fs::remove_file(paths.etc_nixos.join("flake.nix")).unwrap();
    let rollback = plan(&NixAction::Rollback, &paths).unwrap();
    assert_eq!(
        shown(&rollback),
        shown(&plan(&NixAction::SwitchTo(43), &paths).unwrap())
    );
    assert_eq!(rollback[0].program, "nix-env");
    assert_eq!(rollback[1].program, "systemd-run");
    for generation in generations(&paths)
        .into_iter()
        .filter(|generation| !generation.current)
    {
        std::fs::remove_file(paths.generation_link(generation.number)).unwrap();
    }
    assert!(plan(&NixAction::Rollback, &paths).is_err());
}

#[test]
fn as_root_only_root_owned_store_programs_run() {
    // The real system's checks on a tree your user owns: refused.
    let (_dir, paths) = tree();
    let strict = NixPaths {
        trusted_store: true,
        ..paths
    };
    let err = plan(&NixAction::SwitchTo(41), &strict).unwrap_err();
    assert!(err.contains("root-owned program in the Nix store"), "{err}");
    assert!(NixPaths::system().trusted_store);
}

#[test]
fn clean_plan_deletes_listed_generations_then_collects_garbage() {
    let (_dir, paths) = tree();
    let steps = plan(&NixAction::Clean(Cleanup::KeepLatest(3)), &paths).unwrap();
    assert_eq!(
        steps[0].display(),
        format!(
            "nix-env -p {} --delete-generations 38 39 40 41",
            paths.system_profile().display()
        )
    );
    assert_eq!(steps[1].display(), "nix-collect-garbage");
    assert!(
        steps[1].args.is_empty(),
        "never -d: that would delete every old generation of every profile"
    );
    assert!(
        steps[2].args.last().is_some_and(|a| a == "boot"),
        "the boot menu drops the deleted ones"
    );
    let nothing = plan(&NixAction::Clean(Cleanup::KeepLatest(100)), &paths).unwrap();
    assert_eq!(shown(&nothing), ["collect-garbage: nix-collect-garbage"]);
}

#[test]
fn flake_input_changes_from_lock_files() {
    let before = locked_inputs(&fixture("nix/flake.lock.old"));
    let after = locked_inputs(&fixture("nix/flake.lock.new"));
    assert_eq!(
        before.iter().map(|i| i.name.as_str()).collect::<Vec<_>>(),
        ["flake-utils", "systems"]
    );
    let changes = input_changes(&before, &after);
    assert_eq!(changes.len(), 2);
    assert_eq!(
        changes[0].old.as_ref().unwrap().rev,
        "b1d9ab70662946ef0850d488da1c9019f3a9752a"
    );
    assert_eq!(
        changes[0].new.as_ref().unwrap().rev,
        "b7a79d3fea1e214f6d668d521cecec0c01e26544"
    );
    assert!(
        changes[0].new.as_ref().unwrap().last_modified
            > changes[0].old.as_ref().unwrap().last_modified
    );
    assert!(input_changes(&after, &after).is_empty());
    assert!(locked_inputs("not json").is_empty());
    // An input that was dropped.
    let gone = input_changes(&before, &after[..1]);
    assert!(gone.iter().any(|c| c.name == "systems" && c.new.is_none()));
}

#[test]
fn closure_diffs_and_sizes() {
    let changes = parse_diff_closures(&fixture("nix/diff-closures.txt"));
    assert!(changes.len() > 50);
    assert_eq!(
        changes[0],
        PackageChange {
            name: "acl".into(),
            old: String::new(),
            new: "2.4.0".into(),
            delta: 137_626
        }
    );
    let bash = changes
        .iter()
        .find(|c| c.name == "bash-interactive")
        .unwrap();
    assert_eq!((bash.old.as_str(), bash.new.as_str()), ("5.3p9", ""));
    assert!(bash.delta < -7_000_000);
    assert_eq!(
        parse_diff_closures("foo: 1.0, 1.1 → 1.2, -1.0 KiB\nbar: ε → 2\n"),
        [
            PackageChange {
                name: "foo".into(),
                old: "1.0, 1.1".into(),
                new: "1.2".into(),
                delta: -1024
            },
            PackageChange {
                name: "bar".into(),
                old: String::new(),
                new: "2".into(),
                delta: 0
            },
        ]
    );
    let sizes = parse_path_info(&fixture("nix/path-info.txt"));
    assert_eq!(sizes.len(), 2);
    assert_eq!(sizes[0].1, 113_858_984);
    assert_eq!(
        diff_argv(Path::new("/a"), Path::new("/b"))[2..],
        ["store", "diff-closures", "/a", "/b"]
    );
}

#[test]
fn nix_log_lines() {
    let mut t = Tracker::default();
    assert_eq!(
        nix_line(
            r#"@nix {"action":"msg","level":0,"msg":"error: boom"}"#,
            &mut t
        ),
        NixLine::Show("error: boom".into())
    );
    assert_eq!(
        nix_line(
            r#"@nix {"action":"msg","level":5,"msg":"evaluating"}"#,
            &mut t
        ),
        NixLine::Hide
    );
    assert_eq!(
        nix_line(
            r#"@nix {"action":"result","fields":["step 1"],"id":7,"type":101}"#,
            &mut t
        ),
        NixLine::Show("step 1".into())
    );
    assert_eq!(
        nix_line(
            r#"@nix {"action":"start","id":9,"level":0,"parent":0,"text":"","type":104}"#,
            &mut t
        ),
        NixLine::Hide
    );
    assert_eq!(
        nix_line(
            r#"@nix {"action":"result","fields":[3,7,1,0],"id":9,"type":105}"#,
            &mut t
        ),
        NixLine::Hide
    );
    assert_eq!((t.built, t.build_total), (3, 7));
    assert_eq!(nix_line("@nix {broken", &mut t), NixLine::Hide);
    assert_eq!(
        nix_line("plain text", &mut t),
        NixLine::Show("plain text".into())
    );
    assert_eq!(
        parse_gc_summary("10 store paths deleted, 15.4 KiB freed"),
        Some("15.4 KiB")
    );
    assert_eq!(parse_gc_summary("deleting garbage..."), None);
}

#[test]
fn list_generations_json() {
    let json = r#"[{"generation":44,"date":"2026-11-06 10:00:00","nixosVersion":"25.11","kernelVersion":"6.17.7","configurationRevision":"abc123","specialisations":[],"current":true},{"generation":43,"date":"2026-10-20 09:00:00","nixosVersion":"25.11","kernelVersion":"6.17.4","configurationRevision":"","specialisations":[],"current":false}]"#;
    let g = parse_list_generations_json(json);
    assert_eq!(g.len(), 2);
    assert_eq!(
        (g[0].number, g[0].current, g[0].revision.as_deref()),
        (44, true, Some("abc123"))
    );
    assert_eq!(g[1].revision, None);
    assert!(g[0].date > g[1].date);
    assert!(parse_list_generations_json("nope").is_empty());
}
