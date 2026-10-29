//! `greeter keyring-unlock` against fixture roots holding each
//! distribution's default greetd PAM stack (never the real /etc/pam.d).

use super::*;
use crate::manager::{RestorePlan, Runner};

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

    fn module(&self, dir: &str, name: &str) -> &Fixture {
        self.file(&format!("{dir}/{name}"), "ELF")
    }
}

/// No command ever runs for this change.
struct NoCommands;

impl Runner for NoCommands {
    fn run(&mut self, argv: &[String]) -> Result<(), String> {
        panic!("unexpected command {argv:?}")
    }
}

// Arch Linux: greetd's file includes system-local-login → system-login → system-auth.
const ARCH_GREETD: &str = "#%PAM-1.0\n\nauth       required     pam_securetty.so\nauth       requisite    pam_nologin.so\nauth       include      system-local-login\naccount    include      system-local-login\nsession    include      system-local-login\n";
const ARCH_LOCAL: &str = "#%PAM-1.0\n\nauth      include   system-login\naccount   include   system-login\npassword  include   system-login\nsession   include   system-login\n";
const ARCH_LOGIN: &str = "#%PAM-1.0\n\nauth       required   pam_shells.so\nauth       requisite  pam_nologin.so\nauth       include    system-auth\n\naccount    required   pam_access.so\naccount    include    system-auth\n\npassword   include    system-auth\n\nsession    optional   pam_loginuid.so\nsession    optional   pam_keyinit.so       force revoke\nsession    include    system-auth\nsession    optional   pam_motd.so\n-session   optional   pam_systemd.so\nsession    required   pam_env.so\n";
const ARCH_AUTH: &str = "#%PAM-1.0\n\nauth       required                    pam_faillock.so      preauth\n-auth      [success=2 default=ignore]  pam_systemd_home.so\nauth       [success=1 default=bad]     pam_unix.so          try_first_pass nullok\nauth       [default=die]               pam_faillock.so      authfail\nauth       optional                    pam_permit.so\nauth       required                    pam_env.so\nauth       required                    pam_faillock.so      authsucc\n\naccount    required                    pam_unix.so\n\npassword   required                    pam_unix.so          try_first_pass nullok shadow\n\nsession    required                    pam_limits.so\nsession    required                    pam_unix.so\n";

// Debian / Ubuntu: @include of the common-* stacks.
const DEBIAN_GREETD: &str = "#%PAM-1.0\nauth       required     pam_securetty.so\nauth       requisite    pam_nologin.so\n@include common-auth\n@include common-account\nsession    required     pam_loginuid.so\n@include common-session\n@include common-password\n";
const DEBIAN_COMMON_AUTH: &str = "auth\t[success=1 default=ignore]\tpam_unix.so nullok\nauth\trequisite\t\t\tpam_deny.so\nauth\trequired\t\t\tpam_permit.so\n";
const DEBIAN_COMMON_ACCOUNT: &str = "account\t[success=1 new_authtok_reqd=done default=ignore]\tpam_unix.so\naccount\trequisite\t\t\tpam_deny.so\n";
const DEBIAN_COMMON_SESSION: &str = "session\t[default=1]\t\t\tpam_permit.so\nsession\trequisite\t\t\tpam_deny.so\nsession\trequired\t\t\tpam_unix.so\nsession\toptional\t\t\tpam_systemd.so\n";
const DEBIAN_COMMON_PASSWORD: &str = "password\t[success=1 default=ignore]\tpam_unix.so obscure yescrypt\npassword\trequisite\t\t\tpam_deny.so\n";

// Fedora: substack system-auth, then postlogin.
const FEDORA_GREETD: &str = "#%PAM-1.0\nauth       substack     system-auth\nauth       include      postlogin\naccount    required     pam_nologin.so\naccount    include      system-auth\npassword   include      system-auth\nsession    required     pam_selinux.so close\nsession    required     pam_loginuid.so\nsession    required     pam_selinux.so open\nsession    optional     pam_keyinit.so force revoke\nsession    include      system-auth\nsession    include      postlogin\n";
const FEDORA_SYSTEM_AUTH: &str = "auth        required      pam_env.so\nauth        sufficient    pam_unix.so try_first_pass nullok\nauth        required      pam_deny.so\naccount     required      pam_unix.so\nsession     optional      pam_keyinit.so revoke\nsession     required      pam_limits.so\nsession     required      pam_unix.so\n";
const FEDORA_POSTLOGIN: &str = "session     optional      pam_umask.so silent\nsession     [success=1 default=ignore] pam_succeed_if.so service !~ gdm* quiet\nsession     [default=1]   pam_lastlog.so nowtmp showfailed\n";

// openSUSE: vendor file in /usr/lib/pam.d; pam-config already wires
// gnome-keyring into common-auth / common-session.
const SUSE_GREETD: &str = "#%PAM-1.0\nauth     include   common-auth\naccount  include   common-account\npassword include   common-password\nsession  required  pam_loginuid.so\nsession  include   common-session\n";
const SUSE_COMMON_AUTH: &str = "auth\trequired\tpam_env.so\nauth\toptional\tpam_gnome_keyring.so\nauth\trequired\tpam_unix.so\ttry_first_pass\n";
const SUSE_COMMON_SESSION: &str = "session\trequired\tpam_limits.so\nsession\trequired\tpam_unix.so\ttry_first_pass\nsession\toptional\tpam_systemd.so\nsession\toptional\tpam_gnome_keyring.so\tauto_start only_if=gdm,gdm-password,lxdm,lightdm,greetd\n";

fn arch() -> Fixture {
    let f = Fixture::new();
    f.file("/etc/os-release", "ID=arch\n")
        .file("/etc/pam.d/greetd", ARCH_GREETD)
        .file("/etc/pam.d/system-local-login", ARCH_LOCAL)
        .file("/etc/pam.d/system-login", ARCH_LOGIN)
        .file("/etc/pam.d/system-auth", ARCH_AUTH)
        .module("/usr/lib/security", "pam_gnome_keyring.so")
        .module("/usr/lib/security", "pam_kwallet5.so");
    f
}

fn debian() -> Fixture {
    let f = Fixture::new();
    f.file("/etc/os-release", "ID=debian\n")
        .file("/etc/pam.d/greetd", DEBIAN_GREETD)
        .file("/etc/pam.d/common-auth", DEBIAN_COMMON_AUTH)
        .file("/etc/pam.d/common-account", DEBIAN_COMMON_ACCOUNT)
        .file("/etc/pam.d/common-session", DEBIAN_COMMON_SESSION)
        .file("/etc/pam.d/common-password", DEBIAN_COMMON_PASSWORD)
        .module("/lib/x86_64-linux-gnu/security", "pam_gnome_keyring.so");
    f
}

fn fedora() -> Fixture {
    let f = Fixture::new();
    f.file("/etc/os-release", "ID=fedora\n")
        .file("/etc/pam.d/greetd", FEDORA_GREETD)
        .file("/etc/pam.d/system-auth", FEDORA_SYSTEM_AUTH)
        .file("/etc/pam.d/postlogin", FEDORA_POSTLOGIN)
        .module("/usr/lib64/security", "pam_gnome_keyring.so")
        .module("/usr/lib64/security", "pam_kwallet5.so");
    f
}

fn suse() -> Fixture {
    let f = Fixture::new();
    f.file("/etc/os-release", "ID=\"opensuse-tumbleweed\"\nID_LIKE=\"opensuse suse\"\n")
        .file("/usr/lib/pam.d/greetd", SUSE_GREETD)
        .file("/etc/pam.d/common-auth", SUSE_COMMON_AUTH)
        .file("/etc/pam.d/common-session", SUSE_COMMON_SESSION)
        .file("/etc/pam.d/common-account", "account\trequired\tpam_unix.so\n")
        .file("/etc/pam.d/common-password", "password\trequired\tpam_unix.so\n")
        .module("/usr/lib64/security", "pam_gnome_keyring.so")
        .module("/usr/lib64/security", "pam_kwallet5.so");
    f
}

const GNOME_AUTH: &str = "-auth      optional     pam_gnome_keyring.so";
const GNOME_SESSION: &str = "-session   optional     pam_gnome_keyring.so auto_start";
const KWALLET_AUTH: &str = "-auth      optional     pam_kwallet5.so force_run";
const KWALLET_SESSION: &str = "-session   optional     pam_kwallet5.so auto_start force_run";

/// Index of the first line equal to `line` (trimmed).
fn line_of(text: &str, line: &str) -> usize {
    text.lines().position(|l| l.trim() == line).unwrap_or_else(|| panic!("no line {line:?} in:\n{text}"))
}

/// Adds, checks idempotence, removes and checks the file is back byte for byte.
fn round_trip(f: &Fixture, original: &str) -> String {
    let root = f.root();
    let plan = KeyringPlan::build(&root, Action::Add);
    assert!(plan.errors.is_empty(), "{:?}", plan.errors);
    assert!(plan.changes(), "{}", plan.describe());
    assert!(plan.describe().contains(STATUS_CHANGES));
    plan.perform(&root).unwrap();
    let added = f.read("/etc/pam.d/greetd").unwrap();
    let again = KeyringPlan::build(&root, Action::Add);
    assert!(!again.changes(), "second run must change nothing:\n{}", again.describe());
    assert!(again.describe().contains(STATUS_UP_TO_DATE));
    assert!(again.modules.iter().filter(|m| m.installed()).all(ModuleStatus::unlocks_at_login));
    let remove = KeyringPlan::build(&root, Action::Remove);
    assert!(remove.changes());
    remove.perform(&root).unwrap();
    assert_eq!(f.read("/etc/pam.d/greetd").as_deref(), Some(original), "remove restores the file exactly");
    assert!(!KeyringPlan::build(&root, Action::Remove).changes());
    added
}

#[test]
fn arch_adds_both_keyrings_after_the_login_stack() {
    let f = arch();
    let added = round_trip(&f, ARCH_GREETD);
    let include = line_of(&added, "auth       include      system-local-login");
    assert!(added.lines().nth(include + 1).unwrap().starts_with(BEGIN));
    assert_eq!(line_of(&added, GNOME_AUTH), include + 2);
    assert_eq!(line_of(&added, KWALLET_AUTH), include + 3);
    assert!(line_of(&added, GNOME_SESSION) > line_of(&added, "session    include      system-local-login"));
    assert!(line_of(&added, KWALLET_SESSION) > line_of(&added, GNOME_SESSION));
    assert!(added.trim_end().ends_with(END));
}

#[test]
fn debian_places_lines_after_common_auth_and_common_session() {
    let f = debian();
    let added = round_trip(&f, DEBIAN_GREETD);
    assert!(line_of(&added, GNOME_AUTH) > line_of(&added, "@include common-auth"));
    assert!(line_of(&added, GNOME_AUTH) < line_of(&added, "@include common-account"));
    let session = line_of(&added, GNOME_SESSION);
    assert!(session > line_of(&added, "@include common-session"));
    assert!(session < line_of(&added, "@include common-password"));
    // KWallet isn't installed: no line, a package hint in the status.
    assert!(!added.contains("pam_kwallet"));
    let kw = KeyringPlan::build(&f.root(), Action::Add).modules.into_iter().find(|m| m.keyring == Keyring::KWallet).unwrap();
    assert_eq!((kw.installed(), kw.package), (false, "libpam-kwallet5"));
}

#[test]
fn fedora_auth_goes_right_after_the_password_substack() {
    let f = fedora();
    let added = round_trip(&f, FEDORA_GREETD);
    let substack = line_of(&added, "auth       substack     system-auth");
    assert_eq!(line_of(&added, GNOME_AUTH), substack + 2, "before `include postlogin`, like GDM's file");
    assert!(line_of(&added, KWALLET_AUTH) < line_of(&added, "auth       include      postlogin"));
    assert!(line_of(&added, GNOME_SESSION) > line_of(&added, "session    include      postlogin"));
}

#[test]
fn opensuse_copies_the_vendor_file_and_skips_what_pam_config_wired() {
    let f = suse();
    let root = f.root();
    let plan = KeyringPlan::build(&root, Action::Add);
    assert_eq!(plan.vendor.as_deref(), Some("/usr/lib/pam.d/greetd"));
    let gnome = plan.modules.iter().find(|m| m.keyring == Keyring::Gnome).unwrap();
    assert_eq!(gnome.auth, Hook::Elsewhere("/etc/pam.d/common-auth".into()));
    assert!(gnome.unlocks_at_login() && !gnome.by_mandara());
    assert_eq!(plan.adding, [KWALLET_AUTH, KWALLET_SESSION]);
    plan.perform(&root).unwrap();
    let copy = f.read("/etc/pam.d/greetd").unwrap();
    assert!(copy.starts_with("#%PAM-1.0\n# mandara-keyring begin"), "{copy}");
    assert!(!copy.contains("pam_gnome_keyring"));
    assert_eq!(f.read("/usr/lib/pam.d/greetd").as_deref(), Some(SUSE_GREETD), "the vendor file is never touched");
    assert!(!KeyringPlan::build(&root, Action::Add).changes());
    let remove = KeyringPlan::build(&root, Action::Remove);
    assert_eq!(remove.contents, None);
    assert!(remove.describe().contains("delete /etc/pam.d/greetd"));
    remove.perform(&root).unwrap();
    assert!(f.read("/etc/pam.d/greetd").is_none(), "Mandara's copy is deleted again");
}

#[test]
fn a_module_installed_later_extends_the_block() {
    let f = debian();
    let root = f.root();
    KeyringPlan::build(&root, Action::Add).perform(&root).unwrap();
    f.module("/usr/lib/x86_64-linux-gnu/security", "pam_kwallet5.so");
    let plan = KeyringPlan::build(&root, Action::Add);
    assert_eq!(plan.adding, [KWALLET_AUTH, KWALLET_SESSION]);
    assert!(plan.removing.is_empty());
    plan.perform(&root).unwrap();
    let text = f.read("/etc/pam.d/greetd").unwrap();
    assert_eq!(text.matches(BEGIN).count(), 2, "still one auth and one session block:\n{text}");
    KeyringPlan::build(&root, Action::Remove).perform(&root).unwrap();
    assert_eq!(f.read("/etc/pam.d/greetd").as_deref(), Some(DEBIAN_GREETD));
}

#[test]
fn nothing_installed_changes_nothing() {
    let f = Fixture::new();
    f.file("/etc/pam.d/greetd", ARCH_GREETD).file("/etc/pam.d/system-local-login", ARCH_LOCAL);
    let plan = KeyringPlan::build(&f.root(), Action::Add);
    assert!(plan.errors.is_empty() && !plan.changes() && plan.nothing_installed());
    assert!(plan.describe().contains(STATUS_NOTHING));
    assert_eq!(plan.execute(&f.root()).unwrap(), "Nothing to change.");
}

#[test]
fn hand_added_lines_count_as_set_up() {
    let f = arch();
    let by_hand = format!("{ARCH_GREETD}auth optional pam_gnome_keyring.so\nsession optional pam_gnome_keyring.so auto_start\n");
    f.file("/etc/pam.d/greetd", &by_hand);
    let plan = KeyringPlan::build(&f.root(), Action::Add);
    assert_eq!(plan.adding, [KWALLET_AUTH, KWALLET_SESSION], "only KWallet is missing");
    let gnome = plan.modules.iter().find(|m| m.keyring == Keyring::Gnome).unwrap();
    assert_eq!(gnome.session, Hook::Elsewhere("/etc/pam.d/greetd".into()));
}

#[test]
fn refuses_broken_markers_missing_files_and_stale_plans() {
    let f = arch();
    f.file("/etc/pam.d/greetd", &format!("{ARCH_GREETD}{BEGIN}\n-auth optional pam_gnome_keyring.so\n"));
    let plan = KeyringPlan::build(&f.root(), Action::Add);
    assert!(plan.errors.iter().any(|e| e.contains("unbalanced")), "{:?}", plan.errors);
    assert!(plan.execute(&f.root()).is_err());

    let empty = Fixture::new();
    let plan = KeyringPlan::build(&empty.root(), Action::Add);
    assert!(plan.errors.iter().any(|e| e.contains("wasn't found")));

    let f = arch();
    let plan = KeyringPlan::build(&f.root(), Action::Add);
    f.file("/etc/pam.d/greetd", &format!("{ARCH_GREETD}# edited meanwhile\n"));
    assert!(plan.perform(&f.root()).unwrap_err().contains("changed since the plan"));
    assert_eq!(f.read("/etc/pam.d/greetd").unwrap(), format!("{ARCH_GREETD}# edited meanwhile\n"));
}

#[test]
fn greetd_service_name_and_autologin_come_from_its_config() {
    let f = arch();
    f.file(GREETD_CONFIG, "[general]\nservice = \"greetd-custom\"\n\n[default_session]\ncommand = \"x\"\n\n[initial_session]\ncommand = \"sway\"\nuser = \"me\"\n").file("/etc/pam.d/greetd-custom", ARCH_GREETD);
    let plan = KeyringPlan::build(&f.root(), Action::Add);
    assert_eq!(plan.path, "/etc/pam.d/greetd-custom");
    assert!(plan.autologin && plan.warnings.iter().any(|w| w.contains("autologin")));
}

#[test]
fn include_that_can_stop_early_is_flagged() {
    let f = fedora();
    f.file("/etc/pam.d/greetd", &FEDORA_GREETD.replace("auth       substack     system-auth", "auth       include      system-auth"));
    let plan = KeyringPlan::build(&f.root(), Action::Add);
    assert!(plan.warnings.iter().any(|w| w.contains("substack")), "{:?}", plan.warnings);
}

#[test]
fn backups_restore_through_greeter_restore() {
    let f = arch();
    let root = f.root();
    let out = KeyringPlan::build(&root, Action::Add).perform(&root).unwrap();
    assert!(out.contains("GNOME Keyring and KWallet unlock"), "{out}");
    let restore = RestorePlan::build(&root, "latest").unwrap();
    assert_eq!(restore.manifest.target, BACKUP_TARGET);
    assert!(restore.describe().contains("keyring-unlock"));
    restore.perform(&root, true, &mut NoCommands).unwrap();
    assert_eq!(f.read("/etc/pam.d/greetd").as_deref(), Some(ARCH_GREETD));
}

#[test]
fn parses_bracket_controls_and_dash_types() {
    let r = Rule::parse("-auth      [success=2 default=ignore]  pam_systemd_home.so").unwrap();
    assert_eq!((r.ty.as_deref(), r.control.as_str(), r.module()), (Some("auth"), "[success=2 default=ignore]", Some("pam_systemd_home.so")));
    let r = Rule::parse("@include common-auth").unwrap();
    assert!(r.is_include() && r.may_be("session"));
    assert!(Rule::parse("  # comment").is_none());
    assert_eq!(Rule::parse("session optional /usr/lib/security/pam_kwallet5.so auto_start").unwrap().module(), Some("pam_kwallet5.so"));
}
