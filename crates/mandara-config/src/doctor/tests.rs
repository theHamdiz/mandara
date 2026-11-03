//! Golden tests: real checker output captured from broken fixture configs
//! (`tests/fixtures/doctor/<compositor>/`, `*.stdout` / `*.stderr`, paths
//! written as `@FIXTURES@`) parsed into diagnostics. MangoWM's came from
//! `mango -p` 0.18.4, Niri's from Niri 26.04's own `niri-config` parser,
//! Umbriel's from `umbriel config validate --json` 0.1.0, Sway's from
//! `sway --validate --verbose` 1.12 (headless, in a throwaway container;
//! its start-up banner of environment, kernel and os-release lines and
//! wlroots' backend lines left out). Hyprland's
//! (`*.synthetic.stdout`) are written from Hyprland's source, as no binary
//! was available to capture them.
//!
//! `real_checkers_match_the_golden_files` runs the installed checkers on the
//! same fixtures (skipped for those not installed; `MANDARA_TEST_NIRI_CHECK`
//! and `MANDARA_TEST_UMBRIEL` point at locally built ones). CI's
//! `compositor-checkers` job runs it with sway installed.

use std::ffi::OsString;
use std::path::{Path, PathBuf};

use super::*;

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/doctor")
}

fn read_fixture(rel: &str) -> String {
    std::fs::read_to_string(fixtures().join(rel)).unwrap_or_default().replace("@FIXTURES@", &fixtures().display().to_string())
}

/// Parses the captured output `out` (without extension) for the config
/// `root`, both relative to the fixtures.
fn diagnose(target: Target, root: &str, out: &str) -> Vec<Diagnostic> {
    let ctx = ParseContext::new(target, &fixtures().join(root));
    let output = Output { stdout: read_fixture(&format!("{out}.stdout")), stderr: read_fixture(&format!("{out}.stderr")), success: false };
    let check = checker(target);
    finish(target, check.parse(&output, &ctx), &ctx, check)
}

/// What one diagnostic must look like: file (relative), line, kind, owner,
/// key.
type Expect = (&'static str, usize, DiagKind, Owner, Option<&'static str>);

fn assert_matches(what: &str, got: &[Diagnostic], want: &[Expect]) {
    let summary: Vec<String> = got.iter().map(|d| format!("{:?}:{:?} {:?} {:?} {:?} {:?}", d.file, d.line, d.kind, d.owner, d.key, d.message)).collect();
    assert_eq!(got.len(), want.len(), "{what}: {summary:#?}");
    for (d, (file, line, kind, owner, key)) in got.iter().zip(want) {
        assert_eq!(d.file.as_deref(), Some(fixtures().join(file).as_path()), "{what}: {summary:#?}");
        assert_eq!((d.line, d.kind, d.owner), (Some(*line), *kind, *owner), "{what}: {summary:#?}");
        assert_eq!(d.key.as_deref(), *key, "{what}: {summary:#?}");
        assert!(d.text.is_some(), "{what}: the offending line is shown");
        assert_eq!(d.id.len(), ID_DIGITS);
    }
}

use DiagKind::*;
use Owner::{Mandara, User};

const MANGO: &[(&str, &[Expect])] = &[
    ("unknown", &[("mango/unknown.conf", 2, UnknownOption, User, Some("blur_optimizedx")), ("mango/unknown.conf", 3, UnknownOption, User, Some("borderpx"))]),
    ("badvalue", &[("mango/badvalue.conf", 1, InvalidValue, User, Some("focus_color")), ("mango/badvalue.conf", 2, InvalidValue, User, Some("animation_curve_open"))]),
    ("syntax", &[("mango/syntax.conf", 2, SyntaxError, User, None), ("mango/syntax.conf", 3, SyntaxError, User, Some("bind"))]),
    ("dupbind", &[("mango/dupbind.conf", 3, BindConflict, User, Some("bind"))]),
    ("missing", &[("mango/missing.conf", 1, MissingInclude, User, Some("./nothere.conf"))]),
    ("deprecated", &[("mango/deprecated.conf", 1, Deprecated, User, Some("touchpad"))]),
    ("inc/main", &[("mango/inc/child.conf", 2, UnknownOption, User, Some("bogus_key"))]),
    ("dropin/config", &[("mango/dropin/mandara/generated/mango.conf", 3, UnsupportedByVersion, Mandara, Some("blur_optimizedx"))]),
];

const NIRI: &[(&str, &[Expect])] = &[
    ("unknown", &[("niri/unknown.kdl", 3, UnknownOption, User, Some("blur_optimized"))]),
    ("badvalue", &[("niri/badvalue.kdl", 2, InvalidValue, User, Some("gaps"))]),
    ("syntax", &[("niri/syntax.kdl", 1, SyntaxError, User, Some("layout"))]),
    ("dupbind", &[("niri/dupbind.kdl", 4, BindConflict, User, Some("Mod+Q"))]),
    ("missing", &[("niri/missing.kdl", 1, MissingInclude, User, Some("/nothere.kdl"))]),
    ("inc/main", &[("niri/inc/child.kdl", 3, UnknownOption, User, Some("bogus"))]),
    ("dropin/config", &[("niri/dropin/mandara/generated/niri.kdl", 3, UnsupportedByVersion, Mandara, Some("shadow-optimized"))]),
];

const UMBRIEL: &[(&str, &[Expect])] = &[
    ("unknown", &[("umbriel/unknown.toml", 2, UnknownOption, User, Some("general.blur_optimized")), ("umbriel/unknown.toml", 4, UnknownOption, User, Some("frobnicate"))]),
    ("badvalue", &[("umbriel/badvalue.toml", 2, InvalidValue, User, Some("layout.gap"))]),
    ("syntax", &[("umbriel/syntax.toml", 1, SyntaxError, User, None)]),
    ("dupbind", &[("umbriel/dupbind.toml", 3, BindConflict, User, Some("Mod+q"))]),
    ("missing", &[("umbriel/missing.toml", 2, MissingInclude, User, Some("/nothere.toml"))]),
    ("inc/main", &[("umbriel/inc/child.toml", 2, UnknownOption, User, Some("general.bogus"))]),
    ("dropin/config", &[("umbriel/dropin/mandara/generated/umbriel.toml", 4, UnsupportedByVersion, Mandara, Some("layout.blur_optimized"))]),
];

const SWAY: &[(&str, &[Expect])] = &[
    ("unknown", &[("sway/unknown.conf", 2, UnknownOption, User, Some("frobnicate"))]),
    ("badvalue", &[("sway/badvalue.conf", 1, InvalidValue, User, Some("gaps"))]),
    ("syntax", &[("sway/syntax.conf", 1, SyntaxError, User, Some("for_window"))]),
    // Sway names no line for a bind defined twice: found by its chord.
    ("dupbind", &[("sway/dupbind.conf", 1, BindConflict, User, Some("Mod4+q"))]),
    // Sway skips an include that matches no file without a word.
    ("missing", &[]),
    ("inc/main", &[("sway/inc/child.conf", 2, UnknownOption, User, Some("border_frobnicate"))]),
    ("dropin/config", &[("sway/dropin/mandara/generated/sway.conf", 3, UnsupportedByVersion, Mandara, Some("blur"))]),
];

/// The config file of a fixture case.
fn root_of(target: Target, case: &str) -> String {
    let ext = match target {
        Target::Mango => "conf",
        Target::Niri => "kdl",
        Target::Umbriel => "toml",
        Target::Hyprland | Target::Sway => "conf",
    };
    format!("{}/{case}.{ext}", target.name().to_lowercase().replace("mangowm", "mango"))
}

fn table(target: Target) -> &'static [(&'static str, &'static [Expect])] {
    match target {
        Target::Mango => MANGO,
        Target::Niri => NIRI,
        Target::Umbriel => UMBRIEL,
        Target::Sway => SWAY,
        Target::Hyprland => &[],
    }
}

/// Niri and Umbriel report a missing include with the absolute path they
/// tried: compared relative to the fixtures.
fn fix_niri_missing_key(target: Target, mut got: Vec<Diagnostic>) -> Vec<Diagnostic> {
    let dir = fixtures().join(target.name().to_lowercase()).display().to_string();
    for d in got.iter_mut().filter(|d| d.kind == MissingInclude) {
        d.key = d.key.as_ref().map(|k| k.trim_start_matches(&dir).to_string());
    }
    got
}

#[test]
fn golden_outputs_parse_into_diagnostics() {
    for target in [Target::Mango, Target::Niri, Target::Umbriel, Target::Sway] {
        let dir = target.name().to_lowercase().replace("mangowm", "mango");
        for (case, want) in table(target) {
            let got = fix_niri_missing_key(target, diagnose(target, &root_of(target, case), &format!("{dir}/{case}")));
            assert_matches(&format!("{dir}/{case}"), &got, want);
        }
    }
}

#[test]
fn details_survive_parsing() {
    // MangoWM: a place-less detail joins the error it explains.
    let bad = diagnose(Target::Mango, "mango/badvalue.conf", "mango/badvalue");
    assert!(bad[1].message.contains("Failed to parse animation_curve_open") && bad[1].message.contains("Invalid number in array"), "{:?}", bad[1].message);
    // A conflict keeps the first definition as a related place.
    let dup = diagnose(Target::Mango, "mango/dupbind.conf", "mango/dupbind");
    assert_eq!(dup[0].related.iter().map(|l| l.line).collect::<Vec<_>>(), vec![1]);
    assert_eq!(dup[0].severity, Severity::Warning);
    let niri = diagnose(Target::Niri, "niri/dupbind.kdl", "niri/dupbind");
    assert_eq!(niri[0].related.iter().map(|l| l.line).collect::<Vec<_>>(), vec![2]);
    // Niri columns are exact.
    let unknown = diagnose(Target::Niri, "niri/unknown.kdl", "niri/unknown");
    assert_eq!(unknown[0].column, Some(5));
    assert_eq!(unknown[0].text.as_deref(), Some("blur_optimized true"));
    // A message without a line is placed by its key and marked as found.
    let deprecated = diagnose(Target::Mango, "mango/deprecated.conf", "mango/deprecated");
    assert!(deprecated[0].guessed);
    assert_eq!(deprecated[0].severity, Severity::Warning);
    // Umbriel: logs on stderr don't double the JSON report.
    let syntax = diagnose(Target::Umbriel, "umbriel/syntax.toml", "umbriel/syntax");
    assert_eq!((syntax.len(), syntax[0].column, syntax[0].severity), (1, Some(9), Severity::Error));
}

/// Hyprland (unverified: written from its source).
#[test]
fn hyprland_formats_from_source() {
    let lua = diagnose(Target::Hyprland, "hyprland/lua/hyprland.lua", "hyprland/lua/verify.synthetic");
    let want: &[Expect] = &[
        ("hyprland/lua/hyprland.lua", 5, UnknownOption, User, Some("general.blur_optimized")),
        ("hyprland/lua/hyprland.lua", 8, UnknownOption, User, Some("frobnicate")),
        ("hyprland/lua/binds.lua", 2, InvalidValue, User, None),
        ("hyprland/lua/mandara/generated/hyprland.lua", 2, UnsupportedByVersion, Mandara, Some("decoration.shadow_optimized")),
        ("hyprland/lua/hyprland.lua", 8, Other, User, None),
    ];
    assert_matches("hyprland lua", &lua, want);
    assert!(lua[4].message.starts_with("attempt to call a nil value"), "the traceback is dropped: {:?}", lua[4].message);
    let syntax = diagnose(Target::Hyprland, "hyprland/lua/syntax.lua", "hyprland/lua/syntax.synthetic");
    assert_matches("hyprland lua syntax", &syntax, &[("hyprland/lua/syntax.lua", 4, SyntaxError, User, Some("gaps_out"))]);
    assert!(diagnose(Target::Hyprland, "hyprland/lua/hyprland.lua", "hyprland/lua/ok.synthetic").is_empty());
    let conf = diagnose(Target::Hyprland, "hyprland/conf/hyprland.conf", "hyprland/conf/verify.synthetic");
    let (placed, unplaced): (Vec<Diagnostic>, Vec<Diagnostic>) = conf.into_iter().partition(|d| d.file.is_some());
    let want: &[Expect] = &[
        ("hyprland/conf/hyprland.conf", 3, UnknownOption, User, Some("general:blur_optimized")),
        ("hyprland/conf/hyprland.conf", 5, Deprecated, User, Some("windowrulev2")),
        ("hyprland/conf/binds.conf", 2, InvalidValue, User, Some("bind")),
        ("hyprland/conf/hyprland.conf", 7, MissingInclude, User, Some("source")),
    ];
    assert_matches("hyprland conf", &placed, want);
    // An unknown format is kept as text rather than lost.
    assert_eq!(unplaced.len(), 1);
    assert_eq!(unplaced[0].message, "Something Hyprland might print one day that no parser knows");
}

#[test]
fn small_helpers() {
    assert_eq!(strip_ansi("\u{1b}[1m\u{1b}[31m[ERROR]:\u{1b}[0m x"), "[ERROR]: x");
    assert_eq!(version_in("mango 0.18.4(release)").as_deref(), Some("0.18.4"));
    assert_eq!(version_in("niri 26.04 (8ed0da4)").as_deref(), Some("26.04"));
    assert_eq!(version_in("Hyprland 0.57.0 built from branch main").as_deref(), Some("0.57.0"));
    assert_eq!(version_in("umbriel 0.1.0 (422784526984)").as_deref(), Some("0.1.0"));
    assert_eq!(version_in("error: no"), None);
    assert!(contains_word("source=./nothere.conf", "./nothere.conf"));
    assert!(!contains_word("blur_optimized=1", "blur"));
    // Lua shortens long chunk names; the tree still finds the file.
    let ctx = ParseContext::new(Target::Hyprland, &fixtures().join("hyprland/lua/hyprland.lua"));
    assert_eq!(ctx.resolve("...land/lua/binds.lua"), fixtures().join("hyprland/lua/binds.lua"));
    assert_eq!(ctx.files.len(), 3, "includes followed, the drop-in too: {:?}", ctx.files);
    assert_eq!(normalize(Path::new("/a/b/./c/../d.conf")), PathBuf::from("/a/b/d.conf"));
    // `source=./binds.conf` names the same file the checker reports.
    let mango = ParseContext::new(Target::Mango, &fixtures().join("mango/inc/main.conf"));
    assert_eq!(mango.files[1], fixtures().join("mango/inc/child.conf"));
}

#[test]
fn explanations_name_the_option() {
    let d = Diagnostic::new(Severity::Error, "x").kind(UnknownOption).key("blur_optimized");
    let text = explain(&d, Target::Mango, Some("0.18.4"));
    assert!(text.contains("MangoWM") && text.contains("blur_optimized"), "{text}");
    for kind in [UnknownOption, RemovedOption, UnsupportedByVersion, Deprecated, SyntaxError, InvalidValue, BindConflict, MissingInclude, Other] {
        assert!(!explain(&d.clone().kind(kind), Target::Niri, None).is_empty());
        assert!(!kind.label().is_empty());
    }
}

#[test]
fn node_ranges_cover_whole_nodes() {
    let kdl = ["binds {", "    Mod+T {", "        spawn \"foot\" // }", "    }", "    Mod+Q { close-window; }", "}"];
    assert_eq!(fix::node_range(Target::Niri, Path::new("c.kdl"), &kdl, 1), (1, 3));
    assert_eq!(fix::node_range(Target::Niri, Path::new("c.kdl"), &kdl, 4), (4, 4));
    let toml = ["[general]", "gap = 3", "", "[frobnicate]", "x = 1", "y = [", "  1,", "]", "", "[layout]"];
    assert_eq!(fix::node_range(Target::Umbriel, Path::new("c.toml"), &toml, 3), (3, 7), "a table through its last key");
    assert_eq!(fix::node_range(Target::Umbriel, Path::new("c.toml"), &toml, 5), (5, 7), "a multi-line array");
    let lua = ["hl.bind(\"SUPER + Q\",", "  hl.dsp.exec_cmd(\"kitty\"))", "x = 1"];
    assert_eq!(fix::node_range(Target::Hyprland, Path::new("c.lua"), &lua, 0), (0, 1));
    let conf = ["exec = a \\", "  b", "c = 1"];
    assert_eq!(fix::node_range(Target::Hyprland, Path::new("c.conf"), &conf, 0), (0, 1));
}

#[test]
fn unsupported_options_are_left_out_of_the_drop_in() {
    let mango = "# head\nblur=1\nblur_optimized=1\nshadows=1\n";
    let out = state::filter_emitted(Target::Mango, Path::new("mango.conf"), mango, &["blur_optimized".into()], "0.18.4");
    assert!(out.contains("# blur_optimized=1") && out.contains("\nblur=1\n") && out.contains("MangoWM 0.18.4 rejects `blur_optimized`"), "{out}");
    let niri = "layout {\n    gaps 8\n    shadow {\n        on\n    }\n}\n";
    let out = state::filter_emitted(Target::Niri, Path::new("niri.kdl"), niri, &["shadow".into()], "26.04");
    assert!(out.contains("//     shadow {") && out.contains("//         on") && out.contains("//     }") && out.contains("\n    gaps 8\n"), "{out}");
    let toml = "[layout]\ngap = 8\nblur_optimized = true\n[colors.border]\nfocused = \"#fff\"\n";
    let out = state::filter_emitted(Target::Umbriel, Path::new("umbriel.toml"), toml, &["layout.blur_optimized".into()], "0.1.0");
    assert!(out.contains("# blur_optimized = true") && out.contains("\ngap = 8\n"), "{out}");
    out.parse::<toml::Table>().expect("still TOML");
    let lua = "hl.config({\n    decoration = {\n        shadow_optimized = true,\n        rounding = 4,\n    },\n})\n";
    let out = state::filter_emitted(Target::Hyprland, Path::new("hyprland.lua"), lua, &["decoration.shadow_optimized".into()], "0.57.0");
    assert!(out.contains("-- shadow_optimized") || out.contains("--         shadow_optimized"), "{out}");
    assert!(out.contains("\n        rounding = 4,\n"));
}

// ── Fixes against real files ─────────────────────────────────────────────

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_tree(&p, &to.join(e.file_name()));
        } else {
            std::fs::copy(&p, to.join(e.file_name())).unwrap();
        }
    }
}

fn on_path(program: &str) -> bool {
    std::env::var_os("PATH").is_some_and(|paths| std::env::split_paths(&paths).any(|d| d.join(program).is_file()))
}

/// A checker that reports one more error once a line is commented out: any
/// fix makes it worse, so it must be rolled back.
fn spiteful_checker() -> Vec<OsString> {
    let script = "printf '[ERROR]: Unknown keyword: a\\n╰─[Index] %s:1\\n' \"$1\"; if grep -q mandara-doctor \"$1\"; then printf '[ERROR]: Unknown keyword: b\\n╰─[Index] %s:2\\n' \"$1\"; fi";
    ["sh", "-c", script, "sh", CONFIG_PLACEHOLDER].iter().map(OsString::from).collect()
}

#[test]
fn a_fix_that_makes_things_worse_is_rolled_back() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config.conf");
    std::fs::write(&root, "a=1\nb=2\n").unwrap();
    let env = CheckEnv { argv: Some(spiteful_checker()), version: Some("0.0.1".into()), ..CheckEnv::default() };
    let before = check_with(Target::Mango, &root, &env);
    assert_eq!(before.errors(), 1, "{before:#?}");
    let id = before.diagnostics[0].id.clone();
    let store = fix::Store { backups: dir.path().join("backups"), unsupported: dir.path().join("unsupported.json") };
    let outcome = fix::apply_in(&before, &id, fix::Fix::Comment, &env, &|| Ok(()), &store).unwrap();
    assert!(outcome.rolled_back && !outcome.resolved);
    assert_eq!(std::fs::read_to_string(&root).unwrap(), "a=1\nb=2\n", "put back byte for byte");
    assert!(outcome.backup.unwrap().join("manifest.txt").exists());
    // A file changed since the check is never edited.
    std::fs::write(&root, "z=1\nb=2\n").unwrap();
    assert!(fix::apply_in(&before, &id, fix::Fix::Remove, &env, &|| Ok(()), &store).is_err());
}

#[test]
fn a_hung_checker_times_out() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config.conf");
    std::fs::write(&root, "a=1\n").unwrap();
    let argv = ["sleep", "5"].iter().map(OsString::from).collect();
    let env = CheckEnv { argv: Some(argv), version: Some("1.0".into()), timeout: Duration::from_millis(200), ..CheckEnv::default() };
    let started = Instant::now();
    assert_eq!(check_with(Target::Mango, &root, &env).status, CheckerStatus::TimedOut);
    assert!(started.elapsed() < Duration::from_secs(2));
    let missing = CheckEnv { argv: Some(vec![OsString::from("/nonexistent/mandara-checker"), OsString::from(CONFIG_PLACEHOLDER)]), ..env };
    assert_eq!(check_with(Target::Mango, &root, &missing).status, CheckerStatus::NotInstalled);
    assert_eq!(check_with(Target::Mango, &dir.path().join("none.conf"), &CheckEnv::default()).status, CheckerStatus::NoConfig);
}

#[test]
fn cached_checks_rerun_only_when_something_changed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config.conf");
    std::fs::write(&root, "a=1\n").unwrap();
    let state = dir.path().join("doctor.json");
    let env = CheckEnv { argv: Some(spiteful_checker()), version: Some("1.0".into()), ..CheckEnv::default() };
    let (first, ran) = state::check_cached_in(Target::Mango, &root, &env, &state);
    assert!(ran && first.errors() == 1);
    let (again, ran) = state::check_cached_in(Target::Mango, &root, &env, &state);
    assert!(!ran, "nothing changed: the stored report");
    assert_eq!(again.diagnostics, first.diagnostics);
    let newer = CheckEnv { version: Some("1.1".into()), ..env.clone() };
    assert!(state::check_cached_in(Target::Mango, &root, &newer, &state).1, "a new compositor version re-checks");
    std::fs::write(&root, "a=1\nb=2\n").unwrap();
    assert!(state::check_cached_in(Target::Mango, &root, &newer, &state).1, "an edited file re-checks");
}

/// With MangoWM installed: comment, then remove, real problems in a copy of
/// the fixtures; each one goes and nothing else breaks.
#[test]
fn mango_fixes_resolve_real_problems() {
    if !on_path("mango") {
        eprintln!("mango isn't installed; skipped");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    copy_tree(&fixtures().join("mango"), &dir.path().join("mango"));
    let home = dir.path().join("home");
    std::fs::create_dir_all(&home).unwrap();
    let env = CheckEnv { home: Some(home), ..CheckEnv::default() };
    let root = dir.path().join("mango/unknown.conf");
    let before = check_with(Target::Mango, &root, &env);
    assert_eq!(before.errors(), 2, "{before:#?}");
    let backups = dir.path().join("backups");
    let store = fix::Store { backups: backups.clone(), unsupported: dir.path().join("unsupported.json") };
    let outcome = fix::apply_in(&before, &before.diagnostics[0].id, fix::Fix::Comment, &env, &|| Ok(()), &store).unwrap();
    assert!(outcome.resolved && !outcome.rolled_back, "{outcome:#?}");
    assert!(std::fs::read_to_string(&root).unwrap().contains("# mandara-doctor: blur_optimizedx=1"));
    let after = outcome.after;
    let outcome = fix::apply_in(&after, &after.diagnostics[0].id, fix::Fix::Remove, &env, &|| Ok(()), &store).unwrap();
    assert!(outcome.resolved && outcome.after.is_clean(), "{outcome:#?}");
    assert!(!std::fs::read_to_string(&root).unwrap().contains("borderpx"));
    // Undo both: the original file comes back.
    for b in std::fs::read_dir(&backups).unwrap().flatten().map(|e| e.path()).collect::<std::collections::BTreeSet<_>>().into_iter().rev() {
        fix::restore(&b).unwrap();
    }
    assert_eq!(std::fs::read_to_string(&root).unwrap(), std::fs::read_to_string(fixtures().join("mango/unknown.conf")).unwrap());
}

/// The installed checkers (or locally built ones) still print what the
/// golden files hold.
#[test]
fn real_checkers_match_the_golden_files() {
    let home = tempfile::tempdir().unwrap();
    let mut ran = Vec::new();
    for target in [Target::Mango, Target::Niri, Target::Umbriel, Target::Sway] {
        let argv: Option<Vec<OsString>> = match target {
            Target::Niri if !on_path("niri") => std::env::var_os("MANDARA_TEST_NIRI_CHECK").map(|p| vec![p, OsString::from(CONFIG_PLACEHOLDER)]),
            Target::Umbriel if !on_path("umbriel") => std::env::var_os("MANDARA_TEST_UMBRIEL").map(|p| ["config", "validate", "-c", CONFIG_PLACEHOLDER, "--json"].iter().map(OsString::from).fold(vec![p], |mut v, a| {
                v.push(a);
                v
            })),
            _ if on_path(checker(target).program()) => None,
            _ => {
                eprintln!("{} isn't installed; skipped", target.name());
                continue;
            }
        };
        if argv.is_none() && !on_path(checker(target).program()) {
            eprintln!("{} isn't installed; skipped", target.name());
            continue;
        }
        let env = CheckEnv { home: Some(home.path().to_path_buf()), argv, version: Some("test".into()), ..CheckEnv::default() };
        for (case, want) in table(target) {
            let report = check_with(target, &fixtures().join(root_of(target, case)), &env);
            assert_eq!(report.status, CheckerStatus::Ran, "{case}: {report:#?}");
            let got = fix_niri_missing_key(target, report.diagnostics);
            assert_matches(&format!("{} {case} (live)", target.name()), &got, want);
        }
        ran.push(target.name());
    }
    eprintln!("checked live: {ran:?}");
}

/// With MangoWM installed: a drop-in option this version rejects is learned
/// and left out (by the write guard and by the Regenerate fix), and a
/// drop-in that breaks the config in another way is rolled back.
#[test]
fn mango_drop_in_writes_heal_or_roll_back() {
    if !on_path("mango") {
        eprintln!("mango isn't installed; skipped");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("config.conf");
    let generated = dir.path().join("mandara/generated/mango.conf");
    std::fs::create_dir_all(generated.parent().unwrap()).unwrap();
    std::fs::write(&root, "border_radius=6\nsource_optional=./mandara/generated/mango.conf\n").unwrap();
    std::fs::write(&generated, "blur=1\n").unwrap();
    let store = fix::Store { backups: dir.path().join("backups"), unsupported: dir.path().join("unsupported.json") };
    let version = "0.18.4";
    let env = CheckEnv { home: Some(dir.path().to_path_buf()), version: Some(version.into()), ..CheckEnv::default() };
    // What the emitter would write, minus what the list says to leave out.
    let emit = |text: &'static str| {
        let (generated, store) = (generated.clone(), store.clone());
        move || -> anyhow::Result<()> {
            let skip = state::unsupported(&store.unsupported, Target::Mango, version);
            std::fs::write(&generated, state::filter_emitted(Target::Mango, &generated, text, &skip, version))?;
            Ok(())
        }
    };
    let before = check_with(Target::Mango, &root, &env);
    assert!(before.is_clean(), "{before:#?}");
    let guarded = fix::guarded_write_in(&before, &env, &emit("blur=1\nblur_optimizedx=1\n"), &generated, &store).unwrap();
    assert_eq!(guarded.left_out, vec!["blur_optimizedx".to_string()]);
    assert!(!guarded.rolled_back && guarded.after.is_clean(), "{guarded:#?}");
    assert!(std::fs::read_to_string(&generated).unwrap().contains("# blur_optimizedx=1"));
    // A broken drop-in that isn't an option to leave out: the old one comes back.
    let broken = fix::guarded_write_in(&guarded.after, &env, &emit("blur=1\nthis is broken\n"), &generated, &store).unwrap();
    assert!(broken.rolled_back && broken.after.is_clean(), "{broken:#?}");
    assert!(!std::fs::read_to_string(&generated).unwrap().contains("broken"));
    // Regenerate on a reported drop-in problem learns the option too.
    std::fs::write(&generated, "blur=1\nshadows_optimizedx=1\n").unwrap();
    let report = check_with(Target::Mango, &root, &env);
    let d = report.diagnostics.iter().find(|d| d.owner == Owner::Mandara).expect("drop-in problem").clone();
    assert_eq!((d.kind, d.key.as_deref()), (UnsupportedByVersion, Some("shadows_optimizedx")));
    assert_eq!(fixes_for(&d), vec![fix::Fix::Regenerate]);
    let outcome = fix::apply_in(&report, &d.id, fix::Fix::Regenerate, &env, &emit("blur=1\nshadows_optimizedx=1\n"), &store).unwrap();
    assert!(outcome.resolved && !outcome.rolled_back, "{outcome:#?}");
    assert_eq!(state::unsupported(&store.unsupported, Target::Mango, version), vec!["blur_optimizedx".to_string(), "shadows_optimizedx".to_string()]);
    assert!(state::unsupported(&store.unsupported, Target::Mango, "0.19.0").is_empty(), "a new version tries the options again");
}
