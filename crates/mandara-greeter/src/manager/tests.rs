//! Detection, planning, switching and restore against fixture trees (never
//! the real system: every test uses its own temporary root and a recording
//! command runner).

use super::*;

/// A fixture filesystem root.
struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Fixture {
        Fixture { dir: tempfile::tempdir().unwrap() }
    }

    fn root(&self) -> SysRoot {
        SysRoot::at(self.dir.path())
    }

    fn file(&self, abs: &str, text: &str) -> &Fixture {
        let p = self.root().path(abs);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
        self
    }

    fn read(&self, abs: &str) -> Option<String> {
        std::fs::read_to_string(self.root().path(abs)).ok()
    }

    fn bins(&self, names: &[&str]) -> &Fixture {
        for n in names {
            self.file(&format!("/usr/bin/{n}"), "#!/bin/sh\n");
        }
        self
    }

    fn units(&self, units: &[&str]) -> &Fixture {
        for u in units {
            self.file(&format!("/usr/lib/systemd/system/{u}"), "[Unit]\n");
        }
        self
    }

    fn dm(&self, unit: &str) -> &Fixture {
        let link = self.root().path(DISPLAY_MANAGER_LINK);
        std::fs::create_dir_all(link.parent().unwrap()).unwrap();
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(format!("/usr/lib/systemd/system/{unit}"), link).unwrap();
        self
    }

    fn distro(&self, id: &str) -> &Fixture {
        self.file("/etc/os-release", &format!("NAME=\"Test\"\nID={id}\n"))
    }

    fn greetd(&self, text: &str) -> &Fixture {
        self.file(GREETD_CONFIG, text)
    }
}

/// Records commands; fails the one equal to `fail_on`.
#[derive(Default)]
struct Recorder {
    log: Vec<String>,
    fail_on: Option<String>,
}

impl Runner for Recorder {
    fn run(&mut self, argv: &[String]) -> Result<(), String> {
        let line = argv.join(" ");
        self.log.push(line.clone());
        if self.fail_on.as_deref() == Some(line.as_str()) { Err(format!("`{line}` failed")) } else { Ok(()) }
    }
}

const SDDM_CONFIG: &str = "# my greetd config\n[terminal]\n# keep vt 2\nvt = 2\n\n[default_session]\ncommand = \"agreety --cmd /bin/sh\"\nuser = \"_greetd\"\n\n[initial_session]\ncommand = \"sway\"\nuser = \"me\"\n";

fn mandara_command() -> String {
    Kiosk::Cage.command()
}

fn exe(f: &Fixture) -> PathBuf {
    let p = f.dir.path().join("build/mandara");
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, b"\x7fELF test binary").unwrap();
    p
}

// ── Detection ────────────────────────────────────────────────────────────

#[test]
fn detects_each_display_manager() {
    for (unit, id) in [("sddm.service", "sddm"), ("gdm.service", "gdm"), ("gdm3.service", "gdm"), ("lightdm.service", "lightdm"), ("ly.service", "ly"), ("lxdm.service", "lxdm")] {
        let f = Fixture::new();
        f.units(&[unit]).dm(unit);
        let d = detect(&f.root());
        assert_eq!(d.display_manager.as_deref(), Some(unit.trim_end_matches(".service")));
        assert_eq!(d.active, Some(id), "{unit}");
        assert!(d.option(id).unwrap().available(), "{unit} unit is installed");
    }
}

#[test]
fn detects_each_greetd_greeter() {
    let cases = [
        (mandara_command(), Some(MANDARA)),
        ("niri -c /etc/mandara/greeter-niri.kdl".to_string(), Some(MANDARA)),
        ("tuigreet --time --cmd sway".to_string(), Some("tuigreet")),
        ("cage -s -- /usr/bin/regreet".to_string(), Some("regreet")),
        ("cage -s -- gtkgreet -l".to_string(), Some("gtkgreet")),
        ("sway -c /etc/nwg-hello/sway-config; nwg-hello".to_string(), Some("nwg-hello")),
        ("wayfire --config /etc/qtgreet/wayfire.ini -- qtgreet".to_string(), Some("qtgreet")),
        ("agreety --cmd /bin/sh".to_string(), Some("agreety")),
        ("my-own-greeter --fancy".to_string(), None),
    ];
    for (command, want) in cases {
        let f = Fixture::new();
        f.units(&["greetd.service"]).dm("greetd.service").greetd(&format!("[default_session]\ncommand = {}\nuser = \"greeter\"\n", toml_string(&command)));
        let d = detect(&f.root());
        assert_eq!(d.display_manager.as_deref(), Some("greetd"));
        assert_eq!(d.active, want, "{command}");
        assert_eq!(d.greetd.user.as_deref(), Some("greeter"));
    }
    let f = Fixture::new();
    f.units(&["greetd.service"]).dm("greetd.service").greetd("[default_session]\ncommand = \"x\"\n");
    assert_eq!(detect(&f.root()).active_label(), "greetd · custom greeter");
}

#[test]
fn detects_nothing_enabled_and_broken_config() {
    let f = Fixture::new();
    let d = detect(&f.root());
    assert_eq!(d.display_manager, None);
    assert_eq!(d.active, None);
    assert_eq!(d.active_label(), "none");
    f.greetd("[default_session\ncommand = ");
    assert!(detect(&f.root()).greetd.error.is_some());
}

#[test]
fn availability_names_missing_pieces_and_packages() {
    let f = Fixture::new();
    f.distro("cachyos").bins(&["tuigreet"]);
    let d = detect(&f.root());
    let mandara = d.option(MANDARA).unwrap();
    let whats: Vec<&str> = mandara.missing.iter().map(|m| m.what.as_str()).collect();
    assert_eq!(whats, ["greetd", "cage"]);
    assert_eq!(d.option("tuigreet").unwrap().missing.iter().map(|m| m.what.as_str()).collect::<Vec<_>>(), ["greetd"]);
    let regreet = d.option("regreet").unwrap();
    assert!(regreet.missing.iter().any(|m| m.what == "regreet" && m.package == "greetd-regreet"));
    assert!(regreet.missing.iter().any(|m| m.what == "cage"));
    assert_eq!(d.option("sddm").unwrap().missing[0].what, "sddm.service");
    assert!(mandara.missing[0].describe().contains("install it first (package greetd)"));

    // Debian names; greetd found through its unit, cage on PATH dirs.
    let f = Fixture::new();
    f.distro("ubuntu").units(&["greetd.service"]).file("/usr/sbin/cage", "");
    let d = detect(&f.root());
    assert!(d.option(MANDARA).unwrap().available());
    assert_eq!(d.option("gdm").unwrap().missing[0].package, "gdm3");
    assert_eq!(d.option("tuigreet").unwrap().missing[0].package, "tuigreet");
    assert_eq!(d.options.first().map(|o| o.id), Some(MANDARA), "Mandara Login is listed first");
    assert!(describe_list(&d).contains("recommended · default"));
}

#[test]
fn finds_managers_by_alias() {
    assert_eq!(find("Mandara-Login").map(|m| m.id), Some(MANDARA));
    assert_eq!(find("gdm3").map(|m| m.id), Some("gdm"));
    assert!(find("xdm-but-not").is_none());
    let f = Fixture::new();
    assert!(SwitchPlan::build(&f.root(), "nope", Kiosk::Cage, exe(&f)).unwrap_err().contains("unknown login manager"));
}

// ── greetd config rewrite ────────────────────────────────────────────────

#[test]
fn rewrites_only_the_command_line() {
    let out = set_default_session_command(Some(SDDM_CONFIG), "tuigreet --time", "greeter");
    assert_eq!(out, SDDM_CONFIG.replace("command = \"agreety --cmd /bin/sh\"", "command = \"tuigreet --time\""));
    validate_greetd_rewrite(Some(SDDM_CONFIG), &out, "tuigreet --time").unwrap();
    // `initial_session.command` is a different table: untouched.
    assert!(out.contains("command = \"sway\""));
}

#[test]
fn inserts_command_or_table_when_missing() {
    let no_cmd = "[terminal]\nvt = 1\n[default_session]\nuser = \"greeter\"\n";
    let out = set_default_session_command(Some(no_cmd), "regreet", "x");
    assert_eq!(out, "[terminal]\nvt = 1\n[default_session]\ncommand = \"regreet\"\nuser = \"greeter\"\n");
    validate_greetd_rewrite(Some(no_cmd), &out, "regreet").unwrap();

    let no_table = "[terminal]\nvt = 3\n";
    let out = set_default_session_command(Some(no_table), "regreet", "_greetd");
    let t: toml::Table = toml::from_str(&out).unwrap();
    assert_eq!(t["terminal"]["vt"].as_integer(), Some(3));
    assert_eq!(t["default_session"]["user"].as_str(), Some("_greetd"));
    validate_greetd_rewrite(Some(no_table), &out, "regreet").unwrap();

    let fresh = set_default_session_command(None, &mandara_command(), DEFAULT_GREETER_USER);
    let t: toml::Table = toml::from_str(&fresh).unwrap();
    assert_eq!(t["terminal"]["vt"].as_integer(), Some(FRESH_CONFIG_VT));
    assert_eq!(t["default_session"]["command"].as_str(), Some(mandara_command().as_str()));
    validate_greetd_rewrite(None, &fresh, &mandara_command()).unwrap();
}

#[test]
fn handles_multiline_command_and_rejects_bad_rewrites() {
    let multi = "[default_session]\ncommand = \"\"\"\nagreety\n--cmd sh\"\"\"\nuser = \"greeter\"\n";
    let out = set_default_session_command(Some(multi), "tuigreet", "greeter");
    assert_eq!(out, "[default_session]\ncommand = \"tuigreet\"\nuser = \"greeter\"\n");
    validate_greetd_rewrite(Some(multi), &out, "tuigreet").unwrap();
    // A rewrite that changed another value is refused.
    assert!(validate_greetd_rewrite(Some(SDDM_CONFIG), &SDDM_CONFIG.replace("vt = 2", "vt = 4").replace("agreety --cmd /bin/sh", "x"), "x").is_err());
}

// ── Plans ────────────────────────────────────────────────────────────────

fn arch_with_sddm() -> Fixture {
    let f = Fixture::new();
    f.distro("arch").units(&["sddm.service", "greetd.service"]).bins(&["greetd", "cage", "tuigreet"]).dm("sddm.service").greetd(SDDM_CONFIG);
    f
}

#[test]
fn plans_mandara_from_another_display_manager() {
    let f = arch_with_sddm();
    let plan = SwitchPlan::build(&f.root(), MANDARA, Kiosk::Cage, exe(&f)).unwrap();
    assert!(plan.errors.is_empty(), "{:?}", plan.errors);
    assert!(plan.install_mandara);
    assert_eq!(plan.greeter_user, "_greetd", "the configured greeter user is kept");
    assert_eq!(plan.writes.len(), 1);
    assert_eq!(plan.writes[0].contents, SDDM_CONFIG.replace("agreety --cmd /bin/sh", &mandara_command()));
    assert_eq!(plan.commands, vec![systemctl("disable", "sddm.service"), systemctl("enable", GREETD_UNIT)]);
    let text = plan.describe();
    for want in ["Now:    SDDM", "Target: Mandara Login", "- command = \"agreety --cmd /bin/sh\"", "systemctl disable sddm.service", "restore latest", "next boot"] {
        assert!(text.contains(want), "plan lacks {want:?}:\n{text}");
    }
}

#[test]
fn plans_greeter_change_within_greetd() {
    let f = arch_with_sddm();
    f.dm("greetd.service");
    let plan = SwitchPlan::build(&f.root(), "tuigreet", Kiosk::Cage, exe(&f)).unwrap();
    assert!(plan.commands.is_empty(), "greetd stays enabled");
    assert!(!plan.install_mandara);
    assert!(plan.writes[0].contents.contains("command = \"tuigreet --time --remember --remember-session\""));
    // Mandara with a niri kiosk also writes the niri config.
    f.bins(&["niri"]);
    let plan = SwitchPlan::build(&f.root(), MANDARA, Kiosk::Niri, exe(&f)).unwrap();
    assert_eq!(plan.writes.iter().map(|w| w.path.as_str()).collect::<Vec<_>>(), [GREETD_CONFIG, "/etc/mandara/greeter-niri.kdl"]);
}

#[test]
fn plans_display_manager_switch_and_noops() {
    let f = arch_with_sddm();
    f.dm("greetd.service");
    let plan = SwitchPlan::build(&f.root(), "sddm", Kiosk::Cage, exe(&f)).unwrap();
    assert!(plan.writes.is_empty());
    assert_eq!(plan.commands, vec![systemctl("disable", GREETD_UNIT), systemctl("enable", "sddm.service")]);
    // Already active: nothing to do.
    f.dm("sddm.service");
    let plan = SwitchPlan::build(&f.root(), "sddm", Kiosk::Cage, exe(&f)).unwrap();
    assert!(plan.already_active && plan.commands.is_empty());
    f.dm("greetd.service").greetd(&format!("[default_session]\ncommand = {}\n", toml_string(&mandara_command())));
    // Active, but the installed copy is missing / outdated: refresh only.
    let plan = SwitchPlan::build(&f.root(), MANDARA, Kiosk::Cage, exe(&f)).unwrap();
    assert!(plan.already_active && plan.writes.is_empty() && plan.commands.is_empty() && plan.install_mandara);
    assert!(plan.describe().contains(OUTDATED));
    let mut rec = Recorder::default();
    assert!(plan.perform(&f.root(), true, &mut rec).unwrap().contains("refreshed"));
    assert!(rec.log.is_empty() && backups(&f.root()).is_empty());
    let plan = SwitchPlan::build(&f.root(), MANDARA, Kiosk::Cage, exe(&f)).unwrap();
    assert!(plan.already_active && !plan.install_mandara, "up to date now");
    assert!(plan.perform(&f.root(), true, &mut Recorder::default()).unwrap().contains("nothing changed"));
    // No display manager at all: just enable.
    let f = arch_with_sddm();
    std::fs::remove_file(f.root().path(DISPLAY_MANAGER_LINK)).unwrap();
    let plan = SwitchPlan::build(&f.root(), "sddm", Kiosk::Cage, exe(&f)).unwrap();
    assert_eq!(plan.commands, vec![systemctl("enable", "sddm.service")]);
}

#[test]
fn validation_refuses_missing_pieces_and_broken_config() {
    let f = arch_with_sddm();
    let plan = SwitchPlan::build(&f.root(), "gdm", Kiosk::Cage, exe(&f)).unwrap();
    assert!(plan.errors.iter().any(|e| e.contains("gdm.service")));
    let err = plan.perform(&f.root(), true, &mut Recorder::default()).unwrap_err();
    assert!(err.contains("nothing was changed"));
    f.greetd("[default_session\nbroken");
    let plan = SwitchPlan::build(&f.root(), "tuigreet", Kiosk::Cage, exe(&f)).unwrap();
    assert!(plan.errors.iter().any(|e| e.contains("doesn't parse")));
    let mut rec = Recorder::default();
    assert!(plan.perform(&f.root(), true, &mut rec).is_err());
    assert!(rec.log.is_empty());
    assert_eq!(f.read(GREETD_CONFIG).as_deref(), Some("[default_session\nbroken"), "a broken config is never touched");
}

#[test]
fn refuses_without_root() {
    if crate::install::is_root() {
        return;
    }
    let f = arch_with_sddm();
    let plan = SwitchPlan::build(&f.root(), "tuigreet", Kiosk::Cage, exe(&f)).unwrap();
    let mut rec = Recorder::default();
    assert!(plan.execute(&f.root(), true, &mut rec).unwrap_err().contains("not running as root"));
    assert!(rec.log.is_empty());
    assert_eq!(f.read(GREETD_CONFIG).as_deref(), Some(SDDM_CONFIG));
    assert!(backups(&f.root()).is_empty());
}

// ── Apply, rollback, restore (fixture root, recording runner) ────────────

#[test]
fn switch_then_restore_latest() {
    let f = arch_with_sddm();
    let plan = SwitchPlan::build(&f.root(), MANDARA, Kiosk::Cage, exe(&f)).unwrap();
    let mut rec = Recorder::default();
    let out = plan.perform(&f.root(), true, &mut rec).unwrap();
    assert!(out.contains("restore 2"), "{out}");
    assert_eq!(rec.log, ["systemctl disable sddm.service", "systemctl enable greetd.service"]);
    assert!(f.read(GREETD_CONFIG).unwrap().contains(&mandara_command()));
    assert!(f.root().path(INSTALLED_EXE).is_file());
    assert!(f.root().path(crate::config::SYSTEM_CONFIG).is_file());
    let ids = backups(&f.root());
    assert_eq!(ids.len(), 1);

    let restore = RestorePlan::build(&f.root(), "latest").unwrap();
    assert!(restore.manifest.units_switched);
    assert_eq!(restore.commands, vec![systemctl("disable", GREETD_UNIT), systemctl("enable", "sddm.service")]);
    assert!(restore.describe().contains("restore  /etc/greetd/config.toml"));
    let mut rec = Recorder::default();
    restore.perform(&f.root(), true, &mut rec).unwrap();
    assert_eq!(f.read(GREETD_CONFIG).as_deref(), Some(SDDM_CONFIG));
    assert_eq!(rec.log, ["systemctl disable greetd.service", "systemctl enable sddm.service"]);
    assert!(RestorePlan::build(&f.root(), "1999").unwrap_err().contains(&ids[0]));
}

#[test]
fn files_only_switch_restores_without_unit_commands() {
    let f = Fixture::new();
    f.distro("arch").units(&["greetd.service"]).bins(&["greetd", "cage", "regreet"]).dm("sddm.service");
    let plan = SwitchPlan::build(&f.root(), "regreet", Kiosk::Cage, exe(&f)).unwrap();
    let out = plan.perform(&f.root(), false, &mut Recorder::default()).unwrap();
    assert!(out.contains("sudo systemctl enable greetd.service"), "{out}");
    assert!(f.read(GREETD_CONFIG).unwrap().starts_with(FRESH_CONFIG_HEADER));
    let restore = RestorePlan::build(&f.root(), "latest").unwrap();
    assert!(restore.commands.is_empty(), "units never switched");
    restore.perform(&f.root(), true, &mut Recorder::default()).unwrap();
    assert!(f.read(GREETD_CONFIG).is_none(), "a config that didn't exist is removed again");
    assert!(RestorePlan::build(&SysRoot::at(f.dir.path().join("empty")), "latest").unwrap_err().contains("no switch backups"));
}

#[test]
fn failed_step_rolls_back() {
    let f = arch_with_sddm();
    let plan = SwitchPlan::build(&f.root(), "tuigreet", Kiosk::Cage, exe(&f)).unwrap();
    let mut rec = Recorder { fail_on: Some("systemctl enable greetd.service".into()), ..Recorder::default() };
    let err = plan.perform(&f.root(), true, &mut rec).unwrap_err();
    assert!(err.contains("rolled back"), "{err}");
    assert_eq!(rec.log, ["systemctl disable sddm.service", "systemctl enable greetd.service", "systemctl enable sddm.service"]);
    assert_eq!(f.read(GREETD_CONFIG).as_deref(), Some(SDDM_CONFIG));
}

#[test]
fn backup_ids_are_unique() {
    let f = arch_with_sddm();
    f.dm("greetd.service");
    for _ in 0..2 {
        let plan = SwitchPlan::build(&f.root(), "regreet", Kiosk::Cage, exe(&f)).unwrap();
        // regreet isn't installed in this fixture: install it.
        f.bins(&["regreet"]);
        let plan = if plan.errors.is_empty() { plan } else { SwitchPlan::build(&f.root(), "regreet", Kiosk::Cage, exe(&f)).unwrap() };
        let _ = plan.perform(&f.root(), false, &mut Recorder::default());
        f.greetd(SDDM_CONFIG);
    }
    let ids = backups(&f.root());
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], ids[1]);
}

#[test]
fn install_lists_each_missing_package_once() {
    // ReGreet on a bare Arch system: greetd, the greeter and cage.
    let f = Fixture::new();
    let regreet = find("regreet").unwrap();
    let missing = missing_for(&f.root(), regreet, Kiosk::Cage, Family::Arch);
    assert_eq!(packages_for(&missing), ["greetd", "greetd-regreet", "cage"]);
    // LightDM brings a greeter of its own, which a bare LightDM lacks.
    let lightdm = find("lightdm").unwrap();
    let missing = missing_for(&f.root(), lightdm, Kiosk::Cage, Family::Debian);
    assert_eq!(packages_for(&missing), ["lightdm", "lightdm-gtk-greeter"]);
    // Mandara Login is this binary: greetd and its kiosk compositor only.
    let mandara = find(MANDARA).unwrap();
    let missing = missing_for(&f.root(), mandara, Kiosk::Cage, Family::Fedora);
    assert_eq!(packages_for(&missing), ["greetd", "cage"]);
}

#[test]
fn install_uses_the_distributions_package_manager() {
    let f = Fixture::new();
    let pkgs = vec!["greetd".to_string(), "cage".to_string()];
    // Nothing to install, or no package manager: no command.
    assert_eq!(install_command(&f.root(), Family::Arch, &[]), None);
    assert_eq!(install_command(&f.root(), Family::Arch, &pkgs), None);
    // The family's own manager wins over another one that is also present.
    f.bins(&["pacman", "dnf"]);
    assert_eq!(install_command(&f.root(), Family::Arch, &pkgs).unwrap(), ["pacman", "-S", "--needed", "--noconfirm", "greetd", "cage"]);
    assert_eq!(install_command(&f.root(), Family::Fedora, &pkgs).unwrap(), ["dnf", "install", "-y", "greetd", "cage"]);
    // Other families use whichever known manager is installed.
    let g = Fixture::new();
    g.bins(&["zypper"]);
    assert_eq!(install_command(&g.root(), Family::Unknown, &pkgs).unwrap(), ["zypper", "--non-interactive", "install", "--no-recommends", "greetd", "cage"]);
}
