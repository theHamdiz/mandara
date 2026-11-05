//! The install flow against a recording fake host and fixture trees: no
//! command runs, nothing is downloaded and nothing outside a temp folder
//! is touched.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::*;
use crate::host::{Fetch, Output, Row};
use crate::release::tests::tarball;
use mandara_delta::signing::sign;

/// The tests' release key (private, hex).
fn test_key() -> &'static str {
    static KEY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    KEY.get_or_init(|| sign::generate().unwrap().0)
}

/// Publishes `SHA256SUMS` for `files` at `base`, signed with the test key.
fn signed_sums(host: &mut FakeHost, base: &str, files: &[(&str, &[u8])]) -> String {
    let sums: String = files.iter().map(|(name, data)| format!("{}  {name}\n", mandara_delta::sha256_hex(data))).collect();
    host.fetches.insert(format!("{base}/SHA256SUMS"), sums.clone().into_bytes());
    host.fetches.insert(format!("{base}/SHA256SUMS.sig"), sign::sign(test_key(), sums.as_bytes()).unwrap().into_bytes());
    sums
}

#[derive(Default)]
struct FakeHost {
    programs: HashSet<String>,
    fetches: HashMap<String, Vec<u8>>,
    /// Commands (argv joined by spaces) that fail.
    failing: HashSet<String>,
    /// Commands that fail the first time only.
    failing_once: HashSet<String>,
    /// Commands that fail with this exit code.
    codes: HashMap<String, i32>,
    answers: Option<bool>,
    /// What a numbered choice gets (`None`: no terminal).
    choice: Option<usize>,
    /// A terminal is attached.
    tty: bool,
    /// Commands (argv joined by spaces) whose captured stdout is this.
    stdout: HashMap<String, String>,
    /// What `select` returns, in order (the default once empty).
    menus: Vec<usize>,
    /// `select` is cancelled instead.
    cancel: bool,
    /// Cancel this menu index after earlier selections have succeeded.
    cancel_at: Option<usize>,
    /// The prompt and row labels of every menu shown.
    shown: Vec<(String, Vec<String>)>,
    ran: Vec<Cmd>,
    out: Vec<(Level, String)>,
}

impl FakeHost {
    fn with(programs: &[&str]) -> FakeHost {
        FakeHost { programs: programs.iter().map(|p| p.to_string()).collect(), ..Default::default() }
    }

    fn text(&self) -> String {
        self.out.iter().map(|(_, t)| format!("{t}\n")).collect()
    }

    fn ran(&self) -> Vec<String> {
        self.ran.iter().map(Cmd::display).collect()
    }
}

impl Host for FakeHost {
    fn run(&mut self, cmd: &Cmd) -> std::io::Result<Output> {
        self.ran.push(cmd.clone());
        let code = self.codes.get(&cmd.display()).copied();
        let stdout = if cmd.io == crate::host::Io::Capture { self.stdout.get(&cmd.display()).cloned().unwrap_or_default() } else { String::new() };
        let failed_once = self.failing_once.remove(&cmd.display());
        Ok(Output { success: !failed_once && !self.failing.contains(&cmd.display()) && code.is_none(), code, stdout, ..Default::default() })
    }

    fn has_program(&self, name: &str) -> bool {
        self.programs.contains(name)
    }

    fn ask(&mut self, question: &str, default: bool) -> bool {
        self.out.push((Level::Info, format!("asked: {question}")));
        self.answers.unwrap_or(default)
    }

    fn choose(&mut self, question: &str, max: usize, _default: usize) -> Option<usize> {
        self.out.push((Level::Info, format!("asked: {question} (0-{max})")));
        self.choice
    }

    fn interactive(&self) -> bool {
        self.tty
    }

    fn select(&mut self, prompt: &str, rows: &[Row], default: usize) -> Option<usize> {
        self.shown.push((prompt.to_string(), rows.iter().map(|r| r.label.clone()).collect()));
        if self.cancel || self.cancel_at == Some(self.shown.len() - 1) {
            return None;
        }
        if self.menus.is_empty() {
            Some(default.min(rows.len().saturating_sub(1)))
        } else {
            Some(self.menus.remove(0))
        }
    }

    fn fetch(&mut self, url: &str, _limit: u64) -> Result<Vec<u8>, Fetch> {
        self.fetches.get(url).cloned().ok_or(Fetch::NotFound)
    }

    fn say(&mut self, level: Level, text: &str) {
        self.out.push((level, text.to_string()));
    }
}

#[derive(Default)]
struct FakeProbe {
    active: bool,
    outdated: bool,
    keyring: Option<String>,
}

impl LoginProbe for FakeProbe {
    fn active(&self) -> bool {
        self.active
    }

    fn outdated(&self, _exe: &Path) -> bool {
        self.outdated
    }

    fn switch_plan(&self, _exe: &Path, after_packages: bool) -> String {
        format!("Login screen — switch plan{}", if after_packages { " (after packages)" } else { "" })
    }

    fn keyring_plan(&self) -> Option<String> {
        self.keyring.clone()
    }
}

/// A home folder, a system root and a fake release binary, all temporary.
struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        let f = Fixture { dir: tempfile::tempdir().unwrap() };
        std::fs::create_dir_all(f.root().join("etc")).unwrap();
        std::fs::create_dir_all(f.home()).unwrap();
        std::fs::create_dir_all(f.exe().parent().unwrap()).unwrap();
        std::fs::write(f.exe(), b"\x7fELF mandara").unwrap();
        f
    }

    fn home(&self) -> PathBuf {
        self.dir.path().join("home/u")
    }

    fn root(&self) -> PathBuf {
        self.dir.path().join("root")
    }

    fn exe(&self) -> PathBuf {
        self.dir.path().join("dl/mandara-0.1.0-x86_64-linux-gnu/bin/mandara")
    }

    fn facts(&self, family: Family) -> Facts {
        Facts {
            os: OsRelease { id: family.name().into(), id_like: Vec::new(), pretty_name: format!("Test {family}") },
            family,
            arch: "x86_64".into(),
            libc: "gnu",
            x86_64_v3: false,
            exe: self.exe(),
            debug_build: false,
            fingerprinted: true,
            exe_checkout: None,
            cwd: self.dir.path().to_path_buf(),
            home: self.home(),
            user: "u".into(),
            path_var: "/usr/bin".into(),
            is_root: false,
            root: self.root(),
            repo: release::DEFAULT_REPO.into(),
            mirror: None,
            xdg_data: None,
            xdg_config: None,
            release_keys: vec![sign::public_key(test_key()).unwrap()],
        }
    }

    /// Every file below the home folder.
    fn home_files(&self) -> Vec<PathBuf> {
        fn walk(d: &Path, out: &mut Vec<PathBuf>) {
            for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                if e.path().is_dir() {
                    walk(&e.path(), out);
                } else {
                    out.push(e.path());
                }
            }
        }
        let mut out = Vec::new();
        walk(&self.home(), &mut out);
        out
    }
}

fn dry(args: InstallArgs) -> InstallArgs {
    InstallArgs { dry_run: true, ..args }
}

fn run(host: &mut FakeHost, probe: &FakeProbe, facts: Facts, args: InstallArgs) -> Result<(), String> {
    Installer::new(host, probe, facts, args).install()
}

#[test]
fn dry_run_prints_every_package_per_family_and_changes_nothing() {
    for family in Family::ALL {
        let Some(set) = packages::for_family(family) else { continue };
        let pm = PackageManager::for_family(family).unwrap();
        let f = Fixture::new();
        let mut host = FakeHost::with(&[pm.binary(), "sudo", "systemctl", "mango"]);
        let probe = FakeProbe { keyring: Some("Status: changes needed".into()), ..Default::default() };
        run(&mut host, &probe, f.facts(family), dry(InstallArgs::default())).unwrap();
        let text = host.text();
        // Required packages in one command, then each optional one alone.
        let required = format!("would run: sudo {}", pm.install_argv(set.run).join(" "));
        assert!(text.contains(&required), "{family}: missing {required}\n{text}");
        for p in set.opt.iter().chain(set.svc) {
            let line = format!("would run: sudo {}\n", pm.install_argv(&[p]).join(" "));
            assert!(text.contains(&line), "{family}: missing {line}");
        }
        assert!(!text.contains(&pm.install_argv(set.build).join(" ")), "{family}: build tools without --from-source");
        // The compositor's portal (MangoWM → wlr).
        assert!(text.contains("xdg-desktop-portal-wlr"), "{family}: no portal");
        // apt's lists are refreshed every time; the fixture's empty root
        // has no Gentoo tree, so that one is synced first.
        if index::sync_before_install(pm) || pm == PackageManager::Emerge {
            assert!(text.contains(&format!("would run: sudo {}\n", index::sync_argv(pm, false).unwrap().join(" "))), "{family}\n{text}");
        }
        assert!(!text.contains("pacman -Sy"), "{family}: pacman's databases are fine here\n{text}");
        // Login screen: greetd + cage, then the switch and the keyring offer.
        if set.greeter.is_empty() {
            assert!(text.contains("install greetd and cage with your package manager"), "{family}\n{text}");
        } else {
            assert!(text.contains(&format!("would run: sudo {}", pm.install_argv(set.greeter).join(" "))), "{family}\n{text}");
            assert!(text.contains("Login screen — switch plan (after packages)"));
            assert!(text.contains(&format!("would run: sudo {} greeter use mandara --apply", f.home().join(".local/bin/mandara").display())), "{family}\n{text}");
            assert!(text.contains("greeter keyring-unlock --apply"));
        }
        assert!(text.contains("would ask: Use Mandara Login as your login screen? [Y/n] (assuming yes)"));
        assert!(text.contains("then run: mandara setup --autostart"));
        assert!(text.contains("dry run: nothing will be changed"));
        // Nothing ran as root, nothing was written.
        assert!(host.ran().iter().all(|c| c.starts_with("systemctl list-unit-files") || c.starts_with("systemctl is-")), "{family}: {:?}", host.ran());
        assert!(f.home_files().is_empty(), "{family}: {:?}", f.home_files());
    }
}

#[test]
fn from_source_adds_build_tools_and_gentoo_uses_atoms() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["emerge", "sudo", "Hyprland"]);
    let args = dry(InstallArgs { from_source: true, no_greeter: true, ..Default::default() });
    run(&mut host, &FakeProbe::default(), f.facts(Family::Gentoo), args).unwrap();
    let text = host.text();
    assert!(text.contains("would run: sudo emerge --noreplace x11-libs/libxkbcommon dev-libs/wayland media-libs/vulkan-loader media-libs/mesa media-libs/fontconfig media-libs/freetype dev-vcs/git net-misc/curl virtual/pkgconfig dev-build/cmake\n"), "{text}");
    assert!(text.contains("would run: sudo emerge --noreplace gui-libs/xdg-desktop-portal-hyprland\n"));
    assert!(text.contains(&format!("would ensure Rust ≥ {MIN_RUST}")));
    assert!(text.contains("clone of https://github.com/theHamdiz/mandara.git"));
    assert!(text.contains("login screen unchanged (--no-greeter)"));
}

/// Issue #10: the installer puts a root-owned copy in place for Studio's
/// administrator actions, from the binary it just checked.
#[test]
fn installs_the_admin_helper_from_the_checked_binary() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), dry(InstallArgs { no_greeter: true, ..Default::default() })).unwrap();
    let text = host.text();
    assert!(text.lines().any(|l| l.starts_with("would run: sudo install -D -o root -g root -m 0755 ") && l.ends_with(&format!(" {}", super::ADMIN_EXE))), "{text}");
}

#[test]
fn nixos_and_unknown_systems_get_hints() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["sudo"]);
    run(&mut host, &FakeProbe::default(), f.facts(Family::NixOS), dry(InstallArgs { no_greeter: true, ..Default::default() })).unwrap();
    let text = host.text();
    assert!(text.contains("NixOS: install Mandara with its flake") && text.contains("programs.mandara.enable = true;"), "{text}");
    assert!(text.contains("docs/install.md#nixos"), "{text}");
    assert!(!text.contains("would run: sudo"));
    // Mandara Login comes from the NixOS module, not greetd + cage packages.
    let mut host = FakeHost::with(&["sudo"]);
    run(&mut host, &FakeProbe::default(), f.facts(Family::NixOS), dry(InstallArgs::default())).unwrap();
    let text = host.text();
    assert!(text.contains("programs.mandara.greeter.enable = true; and rebuild") && !text.contains("install greetd and cage"), "{text}");
    let mut host = FakeHost::with(&["sudo"]);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Unknown), dry(InstallArgs::default())).unwrap();
    assert!(host.text().contains("Unknown distribution"));
    assert!(host.text().contains("install greetd and cage with your package manager"));
}

#[test]
fn no_deps_skips_packages_and_greeter_packages() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), dry(InstallArgs { no_deps: true, ..Default::default() })).unwrap();
    let text = host.text();
    assert!(!text.contains("pacman"), "{text}");
    assert!(text.contains("missing: greetd cage (--no-deps)"));
}

#[test]
fn interactive_setup_chooses_the_compositor_then_the_rest_is_not_asked() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["dnf", "sudo", "systemctl", "niri"]);
    host.tty = true;
    // Setup → Compositor row, compositor list → Niri, Setup → Install (last row).
    host.menus = vec![0, compositor_rows(&host, None).0.iter().position(|id| *id == "niri").unwrap(), 3];
    run(&mut host, &FakeProbe::default(), f.facts(Family::Fedora), InstallArgs::default()).unwrap();
    let (prompt, rows) = &host.shown[0];
    assert_eq!(prompt, "Setup");
    assert_eq!(rows, &["Compositor", "Start with your compositor", "Login screen", "Install"]);
    assert_eq!(host.shown[1].0, "Compositor");
    let labels: Vec<&str> = compositors::SUPPORTED.iter().filter(|c| c.id != "kwin").map(|c| c.label).collect();
    assert_eq!(host.shown[1].1, labels);
    let bin = f.home().join(".local/bin/mandara");
    let setup = format!("{} setup --compositor niri --autostart", bin.display());
    assert!(host.ran().contains(&setup), "{:?}", host.ran());
    // The login-screen question was answered on the screen, not asked again.
    assert!(!host.text().contains("asked: Use Mandara Login"), "{}", host.text());
    assert!(host.text().contains("Mandara Login is your login screen"), "{}", host.text());
    // A terminal gets the wordmark.
    assert!(host.text().contains("███╗   ███╗"), "{}", host.text());
}

#[test]
fn an_untouched_compositor_row_leaves_detection_to_setup() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["dnf", "sudo", "systemctl", "niri"]);
    host.tty = true;
    // Straight to Install: Compositor, autostart, login screen, Install.
    host.menus = vec![3];
    run(&mut host, &FakeProbe::default(), f.facts(Family::Fedora), InstallArgs::default()).unwrap();
    let bin = f.home().join(".local/bin/mandara");
    let ran = host.ran();
    // Niri is installed, so the row suggested it, but only `setup` itself
    // decides: a machine with several compositors must not be pinned to the
    // wrong one by walking past the row.
    assert!(ran.contains(&format!("{} setup --autostart", bin.display())), "{ran:?}");
    assert!(!ran.iter().any(|c| c.contains("setup --compositor")), "{ran:?}");
}

#[test]
fn choices_given_on_the_command_line_are_not_offered() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["pacman", "sudo", "systemctl"]);
    host.tty = true;
    // Rows left: autostart, login screen, Install.
    host.menus = vec![2];
    let args = InstallArgs { compositor: Some("niri".into()), ..Default::default() };
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), args).unwrap();
    assert_eq!(host.shown[0].1, ["Start with your compositor", "Login screen", "Install"]);
    assert!(host.ran().iter().any(|c| c.ends_with("setup --compositor niri --autostart")), "{:?}", host.ran());
}

#[test]
fn yes_skips_the_setup_screen() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["pacman", "sudo", "systemctl"]);
    host.tty = true;
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { yes: true, ..Default::default() }).unwrap();
    assert!(host.shown.is_empty(), "{:?}", host.shown);
    // No terminal interaction, so the plain one-line title.
    assert!(!host.text().contains("███╗"), "{}", host.text());
}

#[test]
fn cancelling_the_setup_screen_changes_nothing() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    host.tty = true;
    host.cancel = true;
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs::default()).unwrap();
    assert!(host.text().contains("Cancelled — nothing was changed."), "{}", host.text());
    assert!(host.ran().is_empty(), "{:?}", host.ran());
    assert!(f.home_files().is_empty(), "{:?}", f.home_files());
}

#[test]
fn cancelling_the_compositor_picker_changes_nothing() {
    let fixture = Fixture::new();
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    host.tty = true;
    host.menus = vec![0];
    host.cancel_at = Some(1);
    run(&mut host, &FakeProbe::default(), fixture.facts(Family::Arch), InstallArgs::default()).unwrap();
    assert_eq!(host.shown.len(), 2);
    assert!(host.text().contains("Cancelled — nothing was changed."));
    assert!(host.ran().is_empty());
    assert!(fixture.home_files().is_empty());
}

#[test]
fn real_install_puts_files_in_place_and_runs_setup_and_greeter() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["dnf", "sudo", "systemctl"]);
    let probe = FakeProbe { keyring: Some("changes".into()), ..Default::default() };
    let args = InstallArgs { yes: true, compositor: Some("niri".into()), ..Default::default() };
    run(&mut host, &probe, f.facts(Family::Fedora), args).unwrap();
    let bin = f.home().join(".local/bin/mandara");
    assert_eq!(std::fs::read(&bin).unwrap(), b"\x7fELF mandara");
    let desktop = std::fs::read_to_string(f.home().join(".local/share/applications/org.mandara.studio.desktop")).unwrap();
    assert!(desktop.contains(&format!("Exec={} studio", bin.display())));
    assert!(f.home().join(".local/share/icons/hicolor/scalable/apps/org.mandara.studio.svg").is_file());
    assert!(f.home().join(".local/share/mandara/plugins-dev").is_dir());
    let b = bin.display().to_string();
    let ran = host.ran();
    let pos = |needle: &str| ran.iter().position(|c| c == needle).unwrap_or_else(|| panic!("{needle} not run: {ran:#?}"));
    let required = pos("sudo dnf install -y libxkbcommon libwayland-client vulkan-loader mesa-vulkan-drivers fontconfig freetype");
    let setup = pos(&format!("{b} setup --compositor niri --autostart"));
    let check = pos(&format!("{b} setup --check --compositor niri"));
    let greetd = pos("sudo dnf install -y greetd cage");
    let switch = pos(&format!("sudo {b} greeter use mandara --apply"));
    let keyring = pos(&format!("sudo {b} greeter keyring-unlock --apply"));
    assert!(required < setup && setup < check && check < greetd && greetd < switch && switch < keyring);
    assert!(ran.iter().any(|c| c == "sudo systemctl enable --now bluetooth.service") || ran.iter().any(|c| c == "systemctl is-enabled --quiet bluetooth.service"));
    // The setup commands run quietly.
    assert!(host.ran[setup].env.contains(&("RUST_LOG".into(), "warn".into())));
    let text = host.text();
    assert!(text.contains("Mandara is installed.") && text.contains("Studio → About → Update now"));
    assert!(text.contains("is not on your PATH"));
    // `--yes` takes the default: the folder goes on PATH, once.
    let profile = std::fs::read_to_string(f.home().join(".profile")).unwrap();
    assert!(profile.contains(&format!("export PATH='{}':\"$PATH\"", f.home().join(".local/bin").display())));
    assert!(std::fs::read_to_string(f.home().join(".bashrc")).unwrap().contains("Mandara installer"));
    assert!(super::add_to_path(&f.home(), &f.home().join(".local/bin")).unwrap().is_empty());
    // No published checksum reachable: installed with a warning.
    assert!(text.contains("couldn't verify this binary"));
}

#[test]
fn conflicts_are_retired_when_accepted() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    let b = f.home().join(".local/bin/mandara").display().to_string();
    host.codes.insert(format!("{b} setup --check"), crate::installer::SETUP_CONFLICTS);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { yes: true, no_deps: true, no_greeter: true, no_autostart: true, ..Default::default() }).unwrap();
    let ran = host.ran();
    assert!(ran.contains(&format!("{b} setup")), "{ran:#?}");
    assert!(ran.contains(&format!("{b} setup --fix-conflicts")));
}

#[test]
fn active_login_screen_is_refreshed_when_outdated() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["pacman", "sudo", "greetd", "cage"]);
    let probe = FakeProbe { active: true, outdated: true, keyring: None };
    run(&mut host, &probe, f.facts(Family::Arch), InstallArgs { yes: true, no_deps: true, ..Default::default() }).unwrap();
    let b = f.home().join(".local/bin/mandara").display().to_string();
    assert!(host.ran().contains(&format!("sudo {b} greeter use mandara --apply")));
    assert!(host.text().contains("Mandara Login updated"));
    assert!(!host.text().contains("Use Mandara Login as your login screen?"));
}

#[test]
fn declining_the_login_screen_keeps_it() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    host.answers = Some(false);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { no_deps: true, ..Default::default() }).unwrap();
    assert!(host.text().contains("kept your login screen"));
    assert!(!host.ran().iter().any(|c| c.contains("greeter use")));
    // Autostart declined too.
    let b = f.home().join(".local/bin/mandara").display().to_string();
    assert!(host.ran().contains(&format!("{b} setup")));
}

#[test]
fn missing_privilege_helper_fails_required_packages() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["pacman"]);
    let err = run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { yes: true, ..Default::default() }).unwrap_err();
    assert!(err.contains("could not install required packages"), "{err}");
    assert!(host.text().contains("no sudo/doas/run0/pkexec"));
    assert!(f.home_files().is_empty());
}

#[test]
fn refuses_root_and_debug_builds() {
    let f = Fixture::new();
    let mut facts = f.facts(Family::Arch);
    facts.is_root = true;
    let err = run(&mut FakeHost::default(), &FakeProbe::default(), facts, InstallArgs::default()).unwrap_err();
    assert!(err.contains("not root"));
    let mut facts = f.facts(Family::Arch);
    facts.debug_build = true;
    let err = run(&mut FakeHost::default(), &FakeProbe::default(), facts.clone(), InstallArgs { no_deps: true, ..Default::default() }).unwrap_err();
    assert!(err.contains("debug build"), "{err}");
    // A dry run only warns.
    let mut host = FakeHost::default();
    run(&mut host, &FakeProbe::default(), facts, dry(InstallArgs { no_deps: true, no_greeter: true, ..Default::default() })).unwrap();
    assert!(host.text().contains("debug build"));
}

#[test]
fn self_check_uses_update_json() {
    let f = Fixture::new();
    let version = mandara_version::version();
    let target = platform::compiled_target();
    let manifest = |sha: &str| format!(r#"{{"schema":1,"name":"mandara","version":"{version}","targets":{{"{target}":{{"archive":{{"file":"a.tar.gz","sha256":"{sha}","size":1}},"binary":{{"path":"bin/mandara","sha256":"{sha}","size":1}}}}}}}}"#);
    let base = format!("https://github.com/theHamdiz/mandara/releases/download/v{version}");
    let url = format!("{base}/update.json");
    let args = InstallArgs { no_deps: true, no_greeter: true, ..Default::default() };
    let release = |host: &mut FakeHost, sha: &str| {
        let json = manifest(sha).into_bytes();
        signed_sums(host, &base, &[("update.json", &json)]);
        host.fetches.insert(url.clone(), json);
    };
    // Matches.
    let mut host = FakeHost::default();
    release(&mut host, &mandara_delta::sha256_hex(b"\x7fELF mandara"));
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), dry(args.clone())).unwrap();
    assert!(host.text().contains("SHA-256 verified against"), "{}", host.text());
    // Differs: refused, unless --no-verify.
    let mut host = FakeHost::default();
    release(&mut host, &"0".repeat(64));
    let err = run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), dry(args.clone())).unwrap_err();
    assert!(err.contains("doesn't match its release"), "{err}");
    let mut host = FakeHost::default();
    release(&mut host, &"0".repeat(64));
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), dry(InstallArgs { no_verify: true, ..args.clone() })).unwrap();
    assert!(host.text().contains("installing anyway (--no-verify)"));
    // A signed release's update.json counts only when the signed
    // SHA256SUMS lists it: a swapped one is a mismatch.
    if mandara_delta::signing::must_be_signed(version) {
        let mut host = FakeHost::default();
        release(&mut host, &"0".repeat(64));
        host.fetches.insert(url.clone(), manifest(&mandara_delta::sha256_hex(b"\x7fELF mandara")).into_bytes());
        let err = run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), dry(args)).unwrap_err();
        assert!(err.contains("checksum mismatch for update.json"), "{err}");
    }
}

#[test]
fn self_check_falls_back_to_a_local_tarball_and_sha256sums() {
    let f = Fixture::new();
    let version = mandara_version::version();
    let target = platform::compiled_target();
    // The fixture binary sits in `dl/mandara-…/bin/`; the tarball in `dl/`.
    let exe = f.dir.path().join(format!("dl/mandara-{version}-{target}/bin/mandara"));
    std::fs::create_dir_all(exe.parent().unwrap()).unwrap();
    std::fs::write(&exe, b"\x7fELF release").unwrap();
    let name = format!("mandara-{version}-{target}.tar.gz");
    let archive = tarball(&[(&format!("mandara-{version}-{target}/bin/mandara"), b"\x7fELF release")]);
    std::fs::write(f.dir.path().join("dl").join(&name), &archive).unwrap();
    let sums = format!("{}  {name}\n", mandara_delta::sha256_hex(&archive));
    std::fs::write(f.dir.path().join("dl/SHA256SUMS"), &sums).unwrap();
    std::fs::write(f.dir.path().join("dl/SHA256SUMS.sig"), sign::sign(test_key(), sums.as_bytes()).unwrap()).unwrap();
    let mut facts = f.facts(Family::Arch);
    facts.exe = exe;
    let mut host = FakeHost::default();
    run(&mut host, &FakeProbe::default(), facts, dry(InstallArgs { no_deps: true, no_greeter: true, ..Default::default() })).unwrap();
    assert!(host.text().contains(&format!("SHA-256 verified against {name} and SHA256SUMS")), "{}", host.text());
}

#[test]
fn local_builds_are_not_checked() {
    let f = Fixture::new();
    let mut facts = f.facts(Family::Arch);
    facts.exe_checkout = Some(f.dir.path().to_path_buf());
    let mut host = FakeHost::default();
    run(&mut host, &FakeProbe::default(), facts, dry(InstallArgs { no_deps: true, no_greeter: true, ..Default::default() })).unwrap();
    assert!(host.text().contains("local build from"));
    assert!(host.ran.is_empty());
    // `cargo install` builds carry no fingerprint: nothing to check either.
    let mut facts = f.facts(Family::Arch);
    facts.fingerprinted = false;
    let mut host = FakeHost::default();
    run(&mut host, &FakeProbe::default(), facts, dry(InstallArgs { no_deps: true, no_greeter: true, ..Default::default() })).unwrap();
    assert!(host.text().contains("local build, e.g. `cargo install`"));
}

/// A release on a fake GitHub: latest tag, the tuned tarball, SHA256SUMS
/// and its signature (by the test key).
fn fake_release(host: &mut FakeHost, archive_bin: &[u8], good_sum: bool) -> String {
    fake_release_of(host, "9.9.9", archive_bin, good_sum)
}

fn fake_release_of(host: &mut FakeHost, version: &str, archive_bin: &[u8], good_sum: bool) -> String {
    let base = format!("https://github.com/theHamdiz/mandara/releases/download/v{version}");
    let top = format!("mandara-{version}-x86_64-linux-gnu-x86-64-v3");
    let name = format!("{top}.tar.gz");
    let archive = tarball(&[(&format!("{top}/bin/mandara"), archive_bin), (&format!("{top}/share/applications/org.mandara.studio.desktop"), b"[Desktop Entry]\nExec=mandara studio --from-release\n")]);
    host.fetches.insert(release::latest_url(release::DEFAULT_REPO), format!(r#"{{"tag_name":"v{version}"}}"#).into_bytes());
    signed_sums(host, &base, &[(&name, if good_sum { &archive } else { b"something else" })]);
    host.fetches.insert(format!("{base}/{name}"), archive);
    name
}

#[test]
fn releases_after_0_3_3_must_be_signed_by_a_trusted_key() {
    let base = "https://github.com/theHamdiz/mandara/releases/download/v9.9.9";
    let install = |host: &mut FakeHost| {
        let f = Fixture::new();
        let mut facts = f.facts(Family::Arch);
        facts.x86_64_v3 = true;
        let result = run(host, &FakeProbe::default(), facts, InstallArgs { version: Some("9.9.9".into()), yes: true, no_deps: true, no_greeter: true, ..Default::default() });
        (result, f.home_files())
    };
    // No signature.
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    fake_release(&mut host, b"\x7fELF 9.9.9", true);
    host.fetches.remove(&format!("{base}/SHA256SUMS.sig"));
    let (result, files) = install(&mut host);
    assert!(result.unwrap_err().contains("isn't signed"));
    assert!(files.is_empty());
    // Signed by a key Mandara doesn't trust.
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    fake_release(&mut host, b"\x7fELF 9.9.9", true);
    let sums = host.fetches[&format!("{base}/SHA256SUMS")].clone();
    let (stranger, _) = sign::generate().unwrap();
    host.fetches.insert(format!("{base}/SHA256SUMS.sig"), sign::sign(&stranger, &sums).unwrap().into_bytes());
    let (result, files) = install(&mut host);
    assert!(result.unwrap_err().contains("doesn't match Mandara's release key"));
    assert!(files.is_empty());
    // A listing changed after signing.
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    fake_release(&mut host, b"\x7fELF 9.9.9", true);
    host.fetches.get_mut(&format!("{base}/SHA256SUMS")).unwrap().extend_from_slice(b"0000  extra\n");
    assert!(install(&mut host).0.is_err());
}

#[test]
fn releases_from_before_signing_install_with_a_note() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    fake_release_of(&mut host, "0.3.3", b"\x7fELF 0.3.3", true);
    host.fetches.remove("https://github.com/theHamdiz/mandara/releases/download/v0.3.3/SHA256SUMS.sig");
    let mut facts = f.facts(Family::Arch);
    facts.x86_64_v3 = true;
    run(&mut host, &FakeProbe::default(), facts, InstallArgs { version: Some("0.3.3".into()), yes: true, no_deps: true, no_greeter: true, ..Default::default() }).unwrap();
    assert!(host.text().contains("came out before releases were signed"), "{}", host.text());
}

#[test]
fn latest_release_is_downloaded_verified_and_installed() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    let name = fake_release(&mut host, b"\x7fELF 9.9.9", true);
    let mut facts = f.facts(Family::Arch);
    facts.x86_64_v3 = true;
    run(&mut host, &FakeProbe::default(), facts, InstallArgs { latest: true, yes: true, no_deps: true, no_greeter: true, ..Default::default() }).unwrap();
    let text = host.text();
    assert!(text.contains(&format!("v9.9.9 · {name}")) && text.contains("SHA-256 verified, signed by Mandara's release key"), "{text}");
    let bin = f.home().join(".local/bin/mandara");
    assert_eq!(std::fs::read(&bin).unwrap(), b"\x7fELF 9.9.9");
    // The release's own desktop entry, pointed at the installed binary.
    let desktop = std::fs::read_to_string(f.home().join(".local/share/applications/org.mandara.studio.desktop")).unwrap();
    assert!(desktop.contains(&format!("Exec={} studio --from-release", bin.display())));
}

#[test]
fn checksum_mismatch_stops_a_release_install() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    fake_release(&mut host, b"\x7fELF 9.9.9", false);
    let mut facts = f.facts(Family::Arch);
    facts.x86_64_v3 = true;
    let err = run(&mut host, &FakeProbe::default(), facts, InstallArgs { version: Some("9.9.9".into()), yes: true, no_deps: true, ..Default::default() }).unwrap_err();
    assert!(err.contains("checksum mismatch"), "{err}");
    assert!(f.home_files().is_empty());
}

#[test]
fn unreachable_release_falls_back_to_source_in_a_dry_run_note() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), dry(InstallArgs { latest: true, no_deps: true, no_greeter: true, ..Default::default() })).unwrap();
    let text = host.text();
    assert!(text.contains("couldn't reach GitHub releases") && text.contains("a real run would build from source"), "{text}");
    // A mirror needs a version.
    let mut facts = f.facts(Family::Arch);
    facts.mirror = Some("https://mirror.example".into());
    let err = run(&mut FakeHost::default(), &FakeProbe::default(), facts, dry(InstallArgs { latest: true, no_deps: true, ..Default::default() })).unwrap_err();
    assert!(err.contains("MANDARA_RELEASE_BASE needs --version"));
    // Other CPUs build from source.
    let mut facts = f.facts(Family::Arch);
    facts.arch = "riscv64".into();
    let mut host = FakeHost::default();
    run(&mut host, &FakeProbe::default(), facts, dry(InstallArgs { latest: true, no_deps: true, no_greeter: true, ..Default::default() })).unwrap();
    assert!(host.text().contains("no prebuilt binary for riscv64"));
}

#[test]
fn external_monitor_brightness_setup() {
    let f = Fixture::new();
    let root = f.root();
    std::fs::create_dir_all(root.join("sys/class/drm/card1-DP-1")).unwrap();
    std::fs::create_dir_all(root.join("dev")).unwrap();
    std::fs::write(root.join("etc/group"), "wheel:x:998:u\n").unwrap();
    std::fs::write(root.join("etc/passwd"), "u:x:1000:1000::/home/u:/bin/sh\n").unwrap();
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    host.answers = Some(true);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { no_deps: true, no_greeter: true, ..Default::default() }).unwrap();
    let ran = host.ran();
    assert!(ran.contains(&"sudo tee /etc/modules-load.d/i2c-dev.conf".to_string()), "{ran:#?}");
    let tee = host.ran.iter().find(|c| c.argv.contains(&"tee".to_string())).unwrap();
    assert_eq!(tee.stdin.as_deref(), Some(&b"i2c-dev\n"[..]));
    assert!(ran.contains(&"sudo modprobe i2c-dev".to_string()));
    assert!(ran.contains(&"sudo groupadd -r i2c".to_string()));
    assert!(ran.contains(&"sudo usermod -aG i2c u".to_string()));
    // Access in this boot does not imply that the module loads next boot.
    std::fs::write(root.join("dev/i2c-0"), "").unwrap();
    std::fs::write(root.join("etc/group"), "i2c:x:20:u\n").unwrap();
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    host.answers = Some(true);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { no_deps: true, no_greeter: true, ..Default::default() }).unwrap();
    assert!(!host.ran().iter().any(|c| c.contains("usermod")));
    assert!(host.ran().contains(&"sudo tee /etc/modules-load.d/i2c-dev.conf".to_string()));
    assert!(!host.ran().iter().any(|c| c.contains("modprobe")));
    let module_config = root.join("etc/modules-load.d/i2c-dev.conf");
    std::fs::create_dir_all(module_config.parent().unwrap()).unwrap();
    std::fs::write(&module_config, "# Kept by the fixture\nother-module").unwrap();
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    host.answers = Some(true);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { no_deps: true, no_greeter: true, ..Default::default() }).unwrap();
    let tee = host.ran.iter().find(|c| c.argv.contains(&"tee".to_string())).unwrap();
    assert_eq!(tee.stdin.as_deref(), Some(&b"# Kept by the fixture\nother-module\ni2c-dev\n"[..]));
    std::fs::write(&module_config, "# Installed\ni2c-dev\n").unwrap();
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { yes: true, no_deps: true, no_greeter: true, ..Default::default() }).unwrap();
    assert!(!host.ran().iter().any(|c| c.contains("modules-load.d") || c.contains("modprobe") || c.contains("usermod")));
}

#[test]
fn group_membership() {
    let groups = "i2c:x:20:alice,bob\nusers:x:100:\n";
    let passwd = "carol:x:1001:100::/home/carol:/bin/sh\n";
    assert!(user_in_group("bob", "i2c", groups, passwd));
    assert!(!user_in_group("carol", "i2c", groups, passwd));
    assert!(user_in_group("carol", "users", groups, passwd));
    assert_eq!(group_gid("i2c", groups), Some(20));
    assert_eq!(group_gid("nope", groups), None);
}

#[test]
fn uninstall_removes_files_and_empties_dropins() {
    let f = Fixture::new();
    let home = f.home();
    let bin = home.join(".local/bin/mandara");
    let desktop = home.join(".local/share/applications/org.mandara.studio.desktop");
    let generated = home.join(".config/mandara/generated");
    for p in [&bin, &desktop] {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, "x").unwrap();
    }
    std::fs::create_dir_all(&generated).unwrap();
    std::fs::write(generated.join("niri.kdl"), "spawn-at-startup \"mandara\"").unwrap();
    std::fs::write(home.join(".config/mandara/config.toml"), "kept").unwrap();
    // A dry run lists, changes nothing.
    let mut host = FakeHost::default();
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { uninstall: true, dry_run: true, ..Default::default() }).unwrap();
    assert!(host.text().contains(&format!("would remove {}", bin.display())) && host.text().contains("would empty"));
    assert!(bin.is_file() && host.ran.is_empty());
    // For real.
    let mut host = FakeHost::default();
    let probe = FakeProbe { active: true, ..Default::default() };
    Installer::new(&mut host, &probe, f.facts(Family::Arch), InstallArgs::default()).uninstall().unwrap();
    assert!(!bin.exists() && !desktop.exists());
    assert!(std::fs::read_to_string(generated.join("niri.kdl")).unwrap().starts_with("// Mandara Shell was uninstalled"));
    assert_eq!(std::fs::read_to_string(home.join(".config/mandara/config.toml")).unwrap(), "kept");
    assert_eq!(host.ran(), [format!("{} msg quit", bin.display())]);
    assert!(host.text().contains("Mandara Login stays your login screen"));
}

#[test]
fn config_errors_are_reported_without_retiring() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    let b = f.home().join(".local/bin/mandara").display().to_string();
    host.codes.insert(format!("{b} setup --check"), crate::installer::SETUP_CONFIG_ERRORS);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { yes: true, no_deps: true, no_greeter: true, no_autostart: true, ..Default::default() }).unwrap();
    let ran = host.ran();
    assert!(!ran.contains(&format!("{b} setup --fix-conflicts")), "{ran:#?}");
    assert!(host.text().contains("Config doctor"));
}

#[test]
fn prerequisite_and_unclassified_setup_failures_never_offer_conflict_retirement() {
    for code in [crate::installer::SETUP_PREREQUISITE, 1] {
        let f = Fixture::new();
        let mut host = FakeHost::with(&["pacman", "sudo"]);
        let b = f.home().join(".local/bin/mandara").display().to_string();
        host.codes.insert(format!("{b} setup --check"), code);
        run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { yes: true, no_deps: true, no_greeter: true, no_autostart: true, ..Default::default() }).unwrap();
        assert!(host.ran().iter().all(|command| !command.contains("--fix-conflicts")));
        let text = host.text();
        assert!(!text.contains("Retire the conflicts"), "{text}");
        assert!(text.contains(if code == crate::installer::SETUP_PREREQUISITE { "Install libxkbcommon" } else { "compositor setup failed" }), "{text}");
    }
}

// ── Package databases (issue #45) ───────────────────────────────────────

/// CachyOS's `pacman.conf`, its databases never synced.
fn unsynced_pacman(f: &Fixture) {
    std::fs::write(f.root().join("etc/pacman.conf"), "[options]\nArchitecture = auto\n\n[cachyos]\nInclude = /etc/pacman.d/cachyos-mirrorlist\n[core]\nInclude = /etc/pacman.d/mirrorlist\n[extra]\nInclude = /etc/pacman.d/mirrorlist\n[multilib]\nInclude = /etc/pacman.d/mirrorlist\n").unwrap();
}

const ARCH_RUN: &str = "pacman -S --needed --noconfirm libxkbcommon wayland vulkan-icd-loader mesa fontconfig freetype2";
const SYNC_QUESTION: &str = "Your package databases are missing or out of date (cachyos, core, extra, multilib). Update them now? This runs `sudo pacman -Syu`, a full system upgrade.";

#[test]
fn missing_pacman_databases_are_synced_with_a_full_upgrade_first() {
    // Dry run: the question and the plan, nothing run as root.
    let f = Fixture::new();
    unsynced_pacman(&f);
    let mut host = FakeHost::with(&["pacman", "sudo", "mango"]);
    host.tty = true;
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), dry(InstallArgs { no_greeter: true, ..Default::default() })).unwrap();
    let text = host.text();
    assert!(text.contains(&format!("would ask: {SYNC_QUESTION} [Y/n] (assuming yes)")), "{text}");
    let sync = text.find("would run: sudo pacman -Syu\n").expect("sync planned");
    let install = text.find(&format!("would run: sudo {ARCH_RUN}\n")).expect("install planned");
    assert!(sync < install);
    assert!(!text.contains("pacman -Sy ") && !text.contains("pacman -Sy\n"), "never a bare -Sy");
    assert!(host.ran().iter().all(|c| !c.starts_with("sudo")), "{:?}", host.ran());

    // Asked at a terminal and accepted: pacman shows the upgrade itself.
    let mut host = FakeHost::with(&["pacman", "sudo", "mango"]);
    host.tty = true;
    host.answers = Some(true);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { no_greeter: true, ..Default::default() }).unwrap();
    let ran = host.ran();
    let sync = ran.iter().position(|c| c == "sudo pacman -Syu").unwrap_or_else(|| panic!("{ran:#?}"));
    let install = ran.iter().position(|c| c == &format!("sudo {ARCH_RUN}")).unwrap();
    assert!(sync < install);
    assert!(host.text().contains(&format!("asked: {SYNC_QUESTION}")));
    assert!(host.text().contains("package databases up to date"));

    // --yes: the default answer, unattended.
    let mut host = FakeHost::with(&["pacman", "sudo", "mango"]);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { yes: true, no_greeter: true, ..Default::default() }).unwrap();
    assert!(host.ran().contains(&"sudo pacman -Syu --noconfirm".to_string()), "{:?}", host.ran());
    assert!(host.text().contains("updating them with `sudo pacman -Syu --noconfirm`, a full system upgrade (--yes: the default answer)"), "{}", host.text());
}

#[test]
fn declining_the_upgrade_stops_with_what_to_do() {
    let f = Fixture::new();
    unsynced_pacman(&f);
    let mut host = FakeHost::with(&["pacman", "sudo", "mango"]);
    host.tty = true;
    host.answers = Some(false);
    let err = run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { no_greeter: true, ..Default::default() }).unwrap_err();
    assert!(err.contains("run `sudo pacman -Syu`, then `mandara install` again") && err.contains("--no-deps"), "{err}");
    assert!(host.ran().iter().all(|c| !c.starts_with("sudo")), "{:?}", host.ran());
    assert!(f.home_files().is_empty());
    // A failed upgrade stops too, with its command.
    let mut host = FakeHost::with(&["pacman", "sudo", "mango"]);
    host.failing.insert("sudo pacman -Syu --noconfirm".into());
    let err = run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { yes: true, no_greeter: true, ..Default::default() }).unwrap_err();
    assert!(err.contains("`sudo pacman -Syu --noconfirm` failed"), "{err}");
    assert!(!host.ran().contains(&format!("sudo {ARCH_RUN}")));
}

#[test]
fn a_failed_install_syncs_once_and_retries() {
    // pacman: databases present but stale (a 404 from the mirror, say).
    let f = Fixture::new();
    let mut host = FakeHost::with(&["pacman", "sudo", "mango"]);
    host.tty = true;
    host.failing_once.insert(format!("sudo {ARCH_RUN}"));
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { no_greeter: true, ..Default::default() }).unwrap();
    let ran = host.ran();
    let installs: Vec<usize> = ran.iter().enumerate().filter(|(_, c)| **c == format!("sudo {ARCH_RUN}")).map(|(i, _)| i).collect();
    let sync = ran.iter().position(|c| c == "sudo pacman -Syu").unwrap_or_else(|| panic!("{ran:#?}"));
    assert!(installs.len() == 2 && installs[0] < sync && sync < installs[1], "{ran:#?}");
    assert!(host.text().contains("asked: Your package databases may be out of date. Update them now and try again? This runs `sudo pacman -Syu`, a full system upgrade."), "{}", host.text());
    // dnf refreshes its metadata without asking (no upgrade).
    let mut host = FakeHost::with(&["dnf", "sudo", "niri"]);
    let required = "sudo dnf install -y libxkbcommon libwayland-client vulkan-loader mesa-vulkan-drivers fontconfig freetype";
    host.failing_once.insert(required.into());
    run(&mut host, &FakeProbe::default(), f.facts(Family::Fedora), InstallArgs { yes: true, no_greeter: true, ..Default::default() }).unwrap();
    let ran = host.ran();
    let refresh = ran.iter().position(|c| c == "sudo dnf makecache --refresh").unwrap_or_else(|| panic!("{ran:#?}"));
    assert_eq!(ran.iter().filter(|c| *c == required).count(), 2);
    assert!(ran.iter().rposition(|c| c == required).unwrap() > refresh);
    // Failing again after the sync: the usual error, no loop.
    let mut host = FakeHost::with(&["dnf", "sudo", "niri"]);
    host.failing.insert(required.into());
    let err = run(&mut host, &FakeProbe::default(), f.facts(Family::Fedora), InstallArgs { yes: true, no_greeter: true, ..Default::default() }).unwrap_err();
    assert!(err.contains("could not install required packages"));
    assert_eq!(host.ran().iter().filter(|c| *c == "sudo dnf makecache --refresh").count(), 1);
}

#[test]
fn apt_downloads_missing_package_lists_first() {
    let f = Fixture::new();
    std::fs::create_dir_all(f.root().join("var/lib/apt/lists/partial")).unwrap();
    let required = "sudo env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends libxkbcommon0 libwayland-client0 libvulkan1 mesa-vulkan-drivers libfontconfig1 libfreetype6 ca-certificates";
    let mut host = FakeHost::with(&["apt-get", "sudo", "niri"]);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Debian), InstallArgs { yes: true, no_greeter: true, ..Default::default() }).unwrap();
    let ran = host.ran();
    assert!(ran.iter().position(|c| c == "sudo apt-get update").unwrap() < ran.iter().position(|c| c == required).unwrap(), "{ran:#?}");
    assert!(host.text().contains("no package lists yet: downloading them (sudo apt-get update)"));
    // It can't download them: a clear stop, not apt's "Unable to locate package".
    let mut host = FakeHost::with(&["apt-get", "sudo", "niri"]);
    host.failing.insert("sudo apt-get update".into());
    let err = run(&mut host, &FakeProbe::default(), f.facts(Family::Debian), InstallArgs { yes: true, no_greeter: true, ..Default::default() }).unwrap_err();
    assert!(err.contains("without package lists nothing can be installed"), "{err}");
    assert!(!host.ran().contains(&required.to_string()));
    // Lists present: a failed refresh is only a stale index; installing goes on.
    std::fs::write(f.root().join("var/lib/apt/lists/deb.debian.org_debian_dists_sid_main_binary-amd64_Packages"), "x").unwrap();
    let mut host = FakeHost::with(&["apt-get", "sudo", "niri"]);
    host.failing.insert("sudo apt-get update".into());
    run(&mut host, &FakeProbe::default(), f.facts(Family::Debian), InstallArgs { yes: true, no_greeter: true, ..Default::default() }).unwrap();
    assert!(host.ran().contains(&required.to_string()));
}

// ── Installing a compositor ─────────────────────────────────────────────

#[test]
fn no_compositor_offers_a_numbered_choice() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    host.tty = true;
    host.choice = Some(2);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { no_greeter: true, no_autostart: true, ..Default::default() }).unwrap();
    let text = host.text();
    assert!(text.contains("  1) MangoWM (recommended) · mangowm\n  2) Niri · niri\n  3) Hyprland · hyprland\n  4) Sway · sway\n  5) River Classic · river-classic\n  0) None: I'll install one myself\n"), "{text}");
    assert!(text.contains("not in Test arch's repositories: Umbriel"));
    assert!(text.contains("asked: Install which one? (0-5)"));
    let ran = host.ran();
    // Availability asked read-only, in the C locale.
    assert!(ran.contains(&"pacman -Si mangowm".to_string()));
    let query = host.ran.iter().find(|c| c.display() == "pacman -Si niri").unwrap();
    assert!(query.env.contains(&("LC_ALL".into(), "C".into())) && query.io == Io::Capture);
    let b = f.home().join(".local/bin/mandara").display().to_string();
    let niri = ran.iter().position(|c| c == "sudo pacman -S --needed --noconfirm niri").unwrap_or_else(|| panic!("{ran:#?}"));
    assert!(ran.contains(&"sudo pacman -S --needed --noconfirm xdg-desktop-portal-gnome".to_string()));
    let setup = ran.iter().position(|c| *c == format!("{b} setup --compositor niri")).unwrap_or_else(|| panic!("{ran:#?}"));
    assert!(niri < setup);
    assert!(!ran.iter().any(|c| c.ends_with(" mangowm") && c.starts_with("sudo")));

    // 0, or no terminal: nothing installed, setup finds what it finds.
    for choice in [Some(0), None] {
        let mut host = FakeHost::with(&["pacman", "sudo"]);
        host.choice = choice;
        run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { no_greeter: true, no_autostart: true, ..Default::default() }).unwrap();
        assert!(!host.ran().iter().any(|c| ["mangowm", "niri", "hyprland"].iter().any(|p| *c == format!("sudo pacman -S --needed --noconfirm {p}"))), "{:?}", host.ran());
        assert!(host.text().contains("none installed; install one yourself"));
        assert!(host.ran().contains(&format!("{b} setup")));
    }
}

#[test]
fn unattended_installs_only_the_named_compositor() {
    let f = Fixture::new();
    // --yes alone never picks one.
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    host.choice = Some(1);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { yes: true, no_greeter: true, ..Default::default() }).unwrap();
    assert!(host.text().contains("not installing one unasked (--yes): rerun with --compositor mango|niri|hyprland"), "{}", host.text());
    assert!(!host.ran().iter().any(|c| c == "sudo pacman -S --needed --noconfirm mangowm"));
    // --compositor hypr --yes: installed, then set up.
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { yes: true, no_greeter: true, compositor: Some("hypr".into()), ..Default::default() }).unwrap();
    let ran = host.ran();
    assert!(ran.contains(&"sudo pacman -S --needed --noconfirm hyprland".to_string()), "{ran:#?}");
    assert!(ran.contains(&"sudo pacman -S --needed --noconfirm xdg-desktop-portal-hyprland".to_string()));
    let b = f.home().join(".local/bin/mandara").display().to_string();
    assert!(ran.contains(&format!("{b} setup --compositor hyprland --autostart")), "{ran:#?}");
    // Named with --yes: installed without a question.
    assert!(host.text().contains("Hyprland (hyprland)") && !host.text().contains("asked:"), "{}", host.text());
    // Already installed: nothing to do.
    let mut host = FakeHost::with(&["pacman", "sudo", "Hyprland"]);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { yes: true, no_greeter: true, compositor: Some("hyprland".into()), ..Default::default() }).unwrap();
    assert!(!host.text().contains("Installing a compositor"));
    // Declined: kept as it is.
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    host.answers = Some(false);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { no_greeter: true, compositor: Some("niri".into()), ..Default::default() }).unwrap();
    assert!(!host.ran().contains(&"sudo pacman -S --needed --noconfirm niri".to_string()));
    assert!(host.text().contains("asked: Niri isn't installed. Install it now (niri, from your distribution's repositories)?"));
    assert!(host.text().contains("skipped; install Niri yourself, then run `mandara setup --compositor niri`"));
}

#[test]
fn compositors_follow_each_familys_repositories() {
    let f = Fixture::new();
    // A Debian without Hyprland or Sway: apt says so, nothing is offered.
    let mut host = FakeHost::with(&["apt-get", "sudo"]);
    host.failing.insert("apt-cache show hyprland".into());
    host.failing.insert("apt-cache show sway".into());
    host.choice = Some(1);
    std::fs::create_dir_all(f.root().join("var/lib/apt/lists")).unwrap();
    std::fs::write(f.root().join("var/lib/apt/lists/deb.debian.org_debian_dists_trixie_main_binary-amd64_Packages"), "x").unwrap();
    run(&mut host, &FakeProbe::default(), f.facts(Family::Debian), InstallArgs { no_greeter: true, ..Default::default() }).unwrap();
    assert!(host.text().contains("Test debian's repositories don't carry any of them"), "{}", host.text());
    assert!(!host.ran().iter().any(|c| c.contains("install") && c.ends_with(" hyprland")));
    // Fedora: Niri only.
    let mut host = FakeHost::with(&["dnf", "sudo"]);
    host.choice = Some(1);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Fedora), InstallArgs { no_greeter: true, ..Default::default() }).unwrap();
    assert!(host.text().contains("  1) Niri (recommended) · niri\n  2) Sway · sway\n  0) None"), "{}", host.text());
    assert!(host.ran().contains(&"sudo dnf install -y niri".to_string()));
    // apk lists nothing for a package it lacks.
    let mut host = FakeHost::with(&["apk", "sudo"]);
    host.stdout.insert("apk search -x niri".into(), "niri-25.11-r2\n".into());
    host.choice = Some(1);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Alpine), InstallArgs { no_greeter: true, ..Default::default() }).unwrap();
    assert!(host.text().contains("  1) Niri (recommended) · niri\n  0) None"), "{}", host.text());
    // Named but not carried: a hint, no install.
    let mut host = FakeHost::with(&["emerge", "sudo"]);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Gentoo), dry(InstallArgs { no_greeter: true, compositor: Some("niri".into()), ..Default::default() })).unwrap();
    assert!(host.text().contains("Niri isn't installed, and Test gentoo's repositories don't carry it"), "{}", host.text());
    // --no-deps: hints only.
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { yes: true, no_deps: true, no_greeter: true, compositor: Some("niri".into()), ..Default::default() }).unwrap();
    assert!(host.text().contains("Niri isn't installed (--no-deps)"));
    assert!(!host.ran().iter().any(|c| c.starts_with("sudo pacman") || c.starts_with("pacman")));
}

#[test]
fn dry_run_shows_the_compositor_plan() {
    let f = Fixture::new();
    unsynced_pacman(&f);
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    host.tty = true;
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), dry(InstallArgs { no_greeter: true, ..Default::default() })).unwrap();
    let text = host.text();
    assert!(text.contains("would ask: Install which one? [1] (assuming 1)"), "{text}");
    let sync = text.find("would run: sudo pacman -Syu\n").unwrap();
    let mango = text.find("would run: sudo pacman -S --needed --noconfirm mangowm\n").unwrap();
    assert!(sync < mango);
    assert!(text.contains("would run: sudo pacman -S --needed --noconfirm xdg-desktop-portal-wlr\n"));
    assert!(text.contains("then run: mandara setup --compositor mango --autostart"));
    // The databases aren't synced yet in a dry run: the table is trusted
    // (and says so) and nothing is queried.
    assert!(host.ran().iter().all(|c| !c.contains("pacman")), "{:?}", host.ran());
    assert!(text.contains("from Mandara's package table: the package databases aren't downloaded yet"));
}

/// What `sh` (or `fish`) ends up with on `PATH` after reading `profile`,
/// or `None` when that shell isn't installed.
fn path_after(shell: &str, profile: &Path) -> Option<String> {
    crate::host::find_program(shell, std::env::var_os("PATH").as_deref())?;
    let script = match shell {
        "fish" => format!("set -gx PATH /usr/bin; source {}; string join : $PATH", super::fish_word(&profile.to_string_lossy())),
        _ => format!("PATH=/usr/bin; . {}; printf %s \"$PATH\"", super::sh_word(&profile.to_string_lossy())),
    };
    let flags: &[&str] = if shell == "fish" { &["--no-config", "-c"] } else { &["-c"] };
    let out = std::process::Command::new(shell).args(flags).arg(&script).env("HOME", profile.parent()?).output().ok()?;
    assert!(out.status.success(), "{shell}: {}", String::from_utf8_lossy(&out.stderr));
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[test]
fn the_path_line_keeps_an_awkward_folder_as_one_word() {
    let f = Fixture::new();
    std::fs::create_dir_all(f.home().join(".config/fish")).unwrap();
    let bin = f.home().join("my bin/it's $HOME `x` \"q\" \\b");
    std::fs::create_dir_all(&bin).unwrap();
    let changed = super::add_to_path(&f.home(), &bin).unwrap();
    assert_eq!(changed.len(), 3, "{changed:?}");
    let want = bin.to_string_lossy();
    if let Some(path) = path_after("sh", &f.home().join(".profile")) {
        assert_eq!(path, format!("{want}:/usr/bin"));
    }
    if let Some(path) = path_after("fish", &f.home().join(".config/fish/conf.d/mandara-path.fish")) {
        assert!(path.split(':').any(|p| p == want), "{path}");
    }
    // Already on PATH there: reading the profile twice adds it once.
    let twice = f.home().join("twice");
    let profile = std::fs::read_to_string(f.home().join(".profile")).unwrap();
    std::fs::write(&twice, format!("{profile}{profile}")).unwrap();
    if let Some(path) = path_after("sh", &twice) {
        assert_eq!(path, format!("{want}:/usr/bin"));
    }
    // A second run with the same folder changes nothing.
    assert!(super::add_to_path(&f.home(), &bin).unwrap().is_empty());
}

#[test]
fn a_new_prefix_replaces_the_old_path_line() {
    let f = Fixture::new();
    std::fs::write(f.home().join(".profile"), "export EDITOR=vi\n").unwrap();
    let old = f.home().join(".local/bin");
    let new = f.home().join("opt/bin");
    assert_eq!(super::add_to_path(&f.home(), &old).unwrap().len(), 2);
    // The mark alone isn't enough: the folder on that line is what counts.
    assert_eq!(super::add_to_path(&f.home(), &new).unwrap().len(), 2);
    let profile = std::fs::read_to_string(f.home().join(".profile")).unwrap();
    assert!(profile.starts_with("export EDITOR=vi\n"), "{profile}");
    assert_eq!(profile.matches(super::PATH_MARK).count(), 1, "{profile}");
    assert!(profile.contains(&super::posix_path_line(&new.to_string_lossy())) && !profile.contains(&*old.to_string_lossy()), "{profile}");
}

#[test]
fn installing_into_an_existing_path_refreshes_only_old_marked_entries() {
    let f = Fixture::new();
    super::add_to_path(&f.home(), &f.home().join("old/bin")).unwrap();
    std::fs::write(f.home().join(".bashrc"), "export EDITOR=vi\n").unwrap();
    let mut facts = f.facts(Family::Fedora);
    facts.path_var = format!("{}:/usr/bin", f.home().join(".local/bin").display());
    let mut host = FakeHost::with(&["dnf", "sudo"]);
    run(&mut host, &FakeProbe::default(), facts, InstallArgs { yes: true, no_deps: true, ..Default::default() }).unwrap();
    let profile = std::fs::read_to_string(f.home().join(".profile")).unwrap();
    assert!(profile.contains(&*f.home().join(".local/bin").to_string_lossy()));
    assert!(!profile.contains("old/bin"));
    assert_eq!(std::fs::read_to_string(f.home().join(".bashrc")).unwrap(), "export EDITOR=vi\n");
}

#[test]
fn an_unreadable_profile_does_not_abort_an_existing_path_install() {
    let f = Fixture::new();
    let original = [0xff, 0xfe];
    std::fs::write(f.home().join(".profile"), original).unwrap();
    let mut facts = f.facts(Family::Fedora);
    facts.path_var = format!("{}:/usr/bin", f.home().join(".local/bin").display());
    let mut host = FakeHost::with(&["dnf", "sudo"]);
    run(&mut host, &FakeProbe::default(), facts, InstallArgs { yes: true, no_deps: true, ..Default::default() }).unwrap();
    assert_eq!(std::fs::read(f.home().join(".profile")).unwrap(), original);
    assert!(host.text().contains("Could not update PATH"));
    assert!(f.home().join(".local/bin/mandara").is_file());
}

#[test]
fn uninstall_removes_only_mandaras_own_plasma_startup_entry() {
    for (owned, dry_run) in [(true, false), (false, false), (true, true)] {
        let f = Fixture::new();
        let startup = f.home().join(".config/autostart").join(files::KWIN_AUTOSTART_FILE);
        std::fs::create_dir_all(startup.parent().unwrap()).unwrap();
        let text = if owned { format!("[Desktop Entry]\nExec=mandara\n{}\n", files::KWIN_AUTOSTART_MARKER) } else { "[Desktop Entry]\nExec=my-command\n".into() };
        std::fs::write(&startup, &text).unwrap();
        let mut host = FakeHost::default();
        run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { uninstall: true, dry_run, ..Default::default() }).unwrap();
        if owned && !dry_run { assert!(!startup.exists()); }
        else { assert_eq!(std::fs::read_to_string(startup).unwrap(), text); }
    }
}

#[test]
fn an_existing_plasma_session_needs_no_extra_compositor_installation() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["kwin_wayland", "dnf", "sudo"]);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Fedora), InstallArgs { compositor: Some("kwin".into()), yes: true, no_deps: true, ..Default::default() }).unwrap();
    assert!(!host.text().contains("Installing a compositor"));
    assert!(host.ran().iter().any(|cmd| cmd.contains("setup --compositor kwin")));
}

#[test]
fn completion_explains_when_the_calling_terminal_needs_a_new_path() {
    let fixture = Fixture::new();
    for on_path in [false, true] {
        let mut host = FakeHost::default();
        let probe = FakeProbe::default();
        let mut facts = fixture.facts(Family::Arch);
        let bin = fixture.home().join(".local/bin");
        facts.path_var = if on_path { format!("{}:/usr/bin", bin.display()) } else { "/usr/bin".into() };
        Installer::new(&mut host, &probe, facts, InstallArgs::default()).completion();
        let output: Vec<_> = host.out.iter().map(|(_, text)| text.as_str()).collect();
        let path_note = output.iter().position(|text| text.contains("open a new terminal"));
        assert_eq!(path_note.is_some(), !on_path);
        let start = output.iter().position(|text| text.starts_with("Start it:")).unwrap();
        if let Some(note) = path_note { assert!(note < start); }
        assert!(output.iter().any(|text| text.contains("application menu")));
        assert!(output.iter().any(|text| text.contains("Studio → About → Update now")));
        assert!(host.ran.is_empty(), "completion only prints instructions");
    }
}

#[test]
fn kwin_is_offered_only_when_installed_and_defaults_use_the_visible_rows() {
    for installed in [false, true] {
        let host = FakeHost::with(if installed { &["kwin_wayland", "niri"] } else { &["niri"] });
        let (names, rows, default) = compositor_rows(&host, Some("niri"));
        assert_eq!(names.contains(&"kwin"), installed);
        assert_eq!(names[default], "niri");
        assert_eq!(rows[default].label, "Niri");
        let (names, _, default) = compositor_rows(&host, Some("kwin"));
        assert_eq!(default, 0);
        assert_eq!(names[default], if installed { "kwin" } else { "mango" });
    }
}

#[test]
fn failed_kwin_availability_is_not_reported_as_a_config_or_keyboard_error() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["pacman", "sudo", "kwin_wayland"]);
    let bin = f.home().join(".local/bin/mandara").display().to_string();
    let check = format!("{bin} setup --check --compositor kwin");
    host.codes.insert(check.clone(), crate::installer::SETUP_AVAILABILITY);
    host.stdout.insert(check, "KWin availability: unable to start probe".into());
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { yes: true, no_deps: true, no_greeter: true, no_autostart: true, compositor: Some("kwin".into()), ..Default::default() }).unwrap();
    let text = host.text();
    assert!(text.contains("KWin availability: unable to start probe"), "{text}");
    assert!(!text.contains("your compositor reports errors in its config"));
    assert!(!text.contains("Install libxkbcommon"));
    assert!(host.ran().iter().all(|command| !command.contains("--fix-conflicts")));
}
