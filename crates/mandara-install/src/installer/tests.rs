//! The install flow against a recording fake host and fixture trees: no
//! command runs, nothing is downloaded and nothing outside a temp folder
//! is touched.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use super::*;
use crate::host::{Fetch, Output};
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
    answers: Option<bool>,
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
        Ok(Output { success: !self.failing.contains(&cmd.display()), ..Default::default() })
    }

    fn has_program(&self, name: &str) -> bool {
        self.programs.contains(name)
    }

    fn ask(&mut self, _question: &str, default: bool) -> bool {
        self.answers.unwrap_or(default)
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
        if let Some(refresh) = pm.refresh_argv() {
            assert!(text.contains(&format!("would run: sudo {}", refresh.join(" "))));
        }
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
    assert!(text.contains("NixOS: add to your configuration") && text.contains("services.gnome.gnome-keyring.enable = true;"));
    assert!(!text.contains("would run: sudo"));
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
    assert!(text.contains("Mandara is installed.") && text.contains("Upgrade:         mandara update"));
    assert!(text.contains("is not on your PATH"));
    // No published checksum reachable: installed with a warning.
    assert!(text.contains("couldn't verify this binary"));
}

#[test]
fn conflicts_are_retired_when_accepted() {
    let f = Fixture::new();
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    let b = f.home().join(".local/bin/mandara").display().to_string();
    host.failing.insert(format!("{b} setup --check"));
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
    // Already in the group with the devices present: nothing asked.
    std::fs::write(root.join("dev/i2c-0"), "").unwrap();
    std::fs::write(root.join("etc/group"), "i2c:x:20:u\n").unwrap();
    let mut host = FakeHost::with(&["pacman", "sudo"]);
    run(&mut host, &FakeProbe::default(), f.facts(Family::Arch), InstallArgs { yes: true, no_deps: true, no_greeter: true, ..Default::default() }).unwrap();
    assert!(!host.ran().iter().any(|c| c.contains("usermod")));
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
