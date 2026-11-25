//! `greeter power-rule` against fixture FreeBSD roots (never the real
//! /usr/local/etc/polkit-1 or /etc/group).

use super::*;
use crate::install::GREETD_CONFIG;
use crate::manager::RestorePlan;

struct Fixture {
    dir: tempfile::TempDir,
}

const GREETD: &str = "[terminal]\nvt = 8\n\n[default_session]\ncommand = \"cage -s -- mandara greeter\"\nuser = \"greeter\"\n";
const PASSWD: &str = "root:*:0:0:Charlie &:/root:/bin/sh\nlayla:*:1001:1001:Layla:/home/layla:/bin/sh\ngreeter:*:1002:1002:greetd greeter:/nonexistent:/usr/sbin/nologin\n";
const GROUP_V025: &str = "wheel:*:0:root,layla\noperator:*:5:root,greeter\nvideo:*:44:layla,greeter\ngreeter:*:1002:\n";
const GROUP_CLEAN: &str = "wheel:*:0:root,layla\noperator:*:5:root\nvideo:*:44:layla,greeter\ngreeter:*:1002:\n";

impl Fixture {
    /// A FreeBSD system with polkit, ConsoleKit2 and greetd's user.
    fn new(group: &str) -> Fixture {
        let f = Fixture { dir: tempfile::tempdir().unwrap() };
        f.file(GREETD_CONFIG, GREETD).file("/etc/passwd", PASSWD).file("/etc/group", group);
        f.file("/usr/local/etc/polkit-1/rules.d/50-default.rules", "polkit.addAdminRule(function(action, subject) { return [\"unix-group:wheel\"]; });\n");
        f.file(CONSOLEKIT_POLICY, "<policyconfig/>\n");
        f
    }

    fn root(&self) -> SysRoot {
        SysRoot::at(self.dir.path()).as_freebsd()
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

    fn plan(&self, action: Action) -> PowerRulePlan {
        PowerRulePlan::build(&self.root(), action)
    }
}

/// Records commands; `pw` edits the fixture's /etc/group like the real one.
struct FakePw<'a> {
    fixture: &'a Fixture,
    log: Vec<String>,
    fail: bool,
}

impl Runner for FakePw<'_> {
    fn run(&mut self, argv: &[String]) -> Result<(), String> {
        self.log.push(argv.join(" "));
        if self.fail {
            return Err("pw: group operator: permission denied".into());
        }
        let [pw, groupmod, group, flag, user] = argv else { panic!("unexpected command {argv:?}") };
        assert_eq!((pw.as_str(), groupmod.as_str(), flag.as_str()), (PW, "groupmod", "-d"));
        let text = self.fixture.read("/etc/group").unwrap();
        let edited: String = text
            .lines()
            .map(|l| {
                let f: Vec<&str> = l.split(':').collect();
                if f[0] == group {
                    let members: Vec<&str> = f[3].split(',').filter(|m| m != user).collect();
                    format!("{}:{}:{}:{}\n", f[0], f[1], f[2], members.join(","))
                } else {
                    format!("{l}\n")
                }
            })
            .collect();
        self.fixture.file("/etc/group", &edited);
        Ok(())
    }
}

fn pw(fixture: &Fixture) -> FakePw<'_> {
    FakePw { fixture, log: Vec::new(), fail: false }
}

/// No command ever runs.
struct NoCommands;

impl Runner for NoCommands {
    fn run(&mut self, argv: &[String]) -> Result<(), String> {
        panic!("unexpected command {argv:?}")
    }
}

#[test]
fn the_rule_allows_only_that_user_only_the_three_power_actions() {
    let text = rule("greeter");
    assert!(text.starts_with(MARKER), "{text}");
    assert!(text.contains("if (subject.user !== \"greeter\") {\n        return polkit.Result.NOT_HANDLED;"), "{text}");
    for a in ACTIONS {
        assert!(text.contains(&format!("    case \"{a}\":\n")), "{a}: {text}");
    }
    assert_eq!(text.matches("polkit.Result.YES").count(), 1, "one answer that allows: {text}");
    assert_eq!(text.matches("case \"").count(), 3, "{text}");
    for never in ["multiple-users", "hibernate", "hybridsleep", "policykit.exec", "unix-group", "addAdminRule"] {
        assert!(!text.contains(never), "{never}: {text}");
    }
    assert_eq!(rule_user(&text), Some("greeter"));
    assert_eq!(rule_user("polkit.addRule(function(action, subject) { if (subject.user !== \"greeter\") {} });"), None, "not Mandara's");
    // Every line is a comment or the rule's JavaScript; it ends with a newline.
    assert!(text.ends_with("});\n"));
}

#[test]
fn only_plain_user_names_go_into_the_rule() {
    for ok in ["greeter", "_greetd", "greetd-greeter", "g.reeter", "Greeter2"] {
        assert!(plain_user(ok), "{ok}");
    }
    for bad in ["", "-greeter", "gree\"ter", "gree\\ter", "greeter\n", "a b", "مستخدم", &"g".repeat(33)] {
        assert!(!plain_user(bad), "{bad:?}");
    }
    let f = Fixture::new(GROUP_CLEAN);
    f.file(GREETD_CONFIG, "[default_session]\ncommand = \"x\"\nuser = \"x\\\"); polkit.addRule(\"\n");
    let plan = f.plan(Action::Add);
    assert!(!plan.changes() && plan.errors.iter().any(|e| e.contains("isn't a plain user name")), "{:?}", plan.errors);
}

/// A clean system: the rule is written (backed up, 0644), nothing else
/// runs, and a second run changes nothing.
#[test]
fn install_writes_the_rule_once() {
    let f = Fixture::new(GROUP_CLEAN);
    let plan = f.plan(Action::Add);
    assert!(plan.errors.is_empty(), "{:?}", plan.errors);
    assert!(plan.changes() && !plan.leave_group);
    assert!(plan.describe().contains(STATUS_CHANGES), "{}", plan.describe());
    let out = plan.perform(&f.root(), &mut NoCommands).unwrap();
    assert!(out.contains(&format!("✓ wrote {RULE_PATH}")), "{out}");
    assert_eq!(f.read(RULE_PATH).as_deref(), Some(rule("greeter").as_str()));
    let mode = std::fs::metadata(f.root().path(RULE_PATH)).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o644);
    let again = f.plan(Action::Add);
    assert!(!again.changes() && again.warnings.is_empty(), "{}", again.describe());
    assert!(again.describe().contains(STATUS_UP_TO_DATE));
    assert_eq!(again.perform(&f.root(), &mut NoCommands).unwrap(), "Nothing to change.");
    assert_eq!(crate::manager::backups(&f.root()).len(), 1, "a re-run makes no second backup");
}

/// A v0.24/v0.25 system: the login screen's user leaves `operator` in the
/// same run that first writes the rule, and the backup records it. A
/// membership added back by hand later is left alone, with a warning.
#[test]
fn the_first_rule_takes_the_login_screen_user_out_of_operator() {
    let f = Fixture::new(GROUP_V025);
    let plan = f.plan(Action::Add);
    assert!(plan.changes() && plan.leave_group, "{}", plan.describe());
    let described = plan.describe();
    assert!(described.contains("$ /usr/sbin/pw groupmod operator -d greeter"), "{described}");
    assert!(described.contains("can read raw disks"), "{described}");
    let mut runner = pw(&f);
    let out = plan.perform(&f.root(), &mut runner).unwrap();
    assert_eq!(runner.log, ["/usr/sbin/pw groupmod operator -d greeter"]);
    assert!(out.contains("✓ took greeter out of operator"), "{out}");
    assert!(f.read("/etc/group").unwrap().contains("\noperator:*:5:root\n"));
    let restore = RestorePlan::build(&f.root(), "latest").unwrap();
    assert_eq!(restore.manifest.target, BACKUP_TARGET);
    assert_eq!(restore.manifest.left_groups, [crate::manager::LeftGroup { user: "greeter".into(), group: LEGACY_GROUP.into() }]);
    // Up to date now.
    assert!(!f.plan(Action::Add).changes());
    // An administrator puts it back by hand: Mandara leaves it there.
    f.file("/etc/group", GROUP_V025);
    let later = f.plan(Action::Add);
    assert!(!later.changes() && !later.leave_group);
    assert!(later.warnings.iter().any(|w| w.contains("pw groupmod operator -d greeter")), "{:?}", later.warnings);
    assert_eq!(later.perform(&f.root(), &mut NoCommands).unwrap(), "Nothing to change.");
}

/// Only greetd's own account leaves `operator`: one that can log in, or
/// whose primary group it is, is left alone with the command to run.
#[test]
fn a_login_account_keeps_its_operator_membership() {
    let f = Fixture::new("operator:*:5:root,layla\n");
    f.file(GREETD_CONFIG, "[default_session]\ncommand = \"x\"\nuser = \"layla\"\n");
    let plan = f.plan(Action::Add);
    assert!(plan.changes() && !plan.leave_group);
    assert!(plan.warnings.iter().any(|w| w.contains("pw groupmod operator -d layla")), "{:?}", plan.warnings);
    plan.perform(&f.root(), &mut NoCommands).unwrap();
    assert!(f.read("/etc/group").unwrap().contains("root,layla"));

    let f = Fixture::new("operator:*:5:root\n");
    f.file("/etc/passwd", "greeter:*:1002:5:greetd greeter:/nonexistent:/usr/sbin/nologin\n");
    let plan = f.plan(Action::Add);
    assert!(!plan.leave_group && plan.warnings.iter().any(|w| w.contains("primary group")), "{:?}", plan.warnings);

    let f = Fixture::new("operator:*:5:root,greeter\n");
    f.file("/etc/passwd", "greeter:*:0:1002:not really:/nonexistent:/usr/sbin/nologin\n");
    assert!(!f.plan(Action::Add).leave_group, "never uid 0");
}

/// `pw` fails: the rule stays (it's what the buttons need), the membership
/// isn't recorded as gone, and the command to run is shown.
#[test]
fn a_failed_pw_keeps_the_rule_and_says_so() {
    let f = Fixture::new(GROUP_V025);
    let mut runner = FakePw { fixture: &f, log: Vec::new(), fail: true };
    let out = f.plan(Action::Add).perform(&f.root(), &mut runner).unwrap();
    assert!(out.contains("warning: greeter is still in operator") && out.contains("pw groupmod operator -d greeter"), "{out}");
    assert!(f.read(RULE_PATH).is_some());
    assert!(RestorePlan::build(&f.root(), "latest").unwrap().manifest.left_groups.is_empty());
}

#[test]
fn someone_elses_file_is_never_replaced() {
    let f = Fixture::new(GROUP_V025);
    let theirs = "polkit.addRule(function(action, subject) { return polkit.Result.NOT_HANDLED; });\n";
    f.file(RULE_PATH, theirs);
    for action in [Action::Add, Action::Remove] {
        let plan = f.plan(action);
        assert!(!plan.changes() && !plan.leave_group && plan.errors.iter().any(|e| e.contains("isn't Mandara's")), "{:?}", plan.errors);
        assert!(plan.perform(&f.root(), &mut NoCommands).is_err());
    }
    assert_eq!(f.read(RULE_PATH).as_deref(), Some(theirs));
}

#[test]
fn missing_pieces_stop_the_plan_before_anything_changes() {
    let f = Fixture::new(GROUP_V025);
    std::fs::remove_file(f.root().path(CONSOLEKIT_POLICY)).unwrap();
    let plan = f.plan(Action::Add);
    assert!(!plan.changes() && !plan.leave_group && plan.errors.iter().any(|e| e.contains("pkg install consolekit2")), "{:?}", plan.errors);

    let f = Fixture::new(GROUP_V025);
    std::fs::remove_dir_all(f.root().path(RULES_DIR)).unwrap();
    let plan = f.plan(Action::Add);
    assert!(plan.errors.iter().any(|e| e.contains("pkg install polkit")), "{:?}", plan.errors);

    let f = Fixture::new(GROUP_V025);
    f.file("/etc/passwd", "root:*:0:0:Charlie &:/root:/bin/sh\n");
    let plan = f.plan(Action::Add);
    assert!(plan.errors.iter().any(|e| e.contains("greeter doesn't exist yet")), "{:?}", plan.errors);
    assert!(f.read(RULE_PATH).is_none());
}

/// `--remove` deletes Mandara's rule and never adds anyone to `operator`;
/// restoring the backup of the first write removes it too.
#[test]
fn remove_and_restore_take_the_rule_out() {
    let f = Fixture::new(GROUP_V025);
    f.plan(Action::Add).perform(&f.root(), &mut pw(&f)).unwrap();
    let restore = RestorePlan::build(&f.root(), "latest").unwrap();
    let described = restore.describe();
    assert!(described.contains("Undoes a `greeter power-rule` change."), "{described}");
    assert!(described.contains(&format!("remove   {RULE_PATH}")), "{described}");
    assert!(described.contains("keeps greeter out of operator") && described.contains("pw groupmod operator -m greeter"), "{described}");
    assert!(described.contains(crate::manager::POLKIT_TAKES_EFFECT) && !described.contains("next boot"), "{described}");

    let plan = f.plan(Action::Remove);
    assert!(plan.changes() && !plan.leave_group, "{}", plan.describe());
    let out = plan.perform(&f.root(), &mut NoCommands).unwrap();
    assert!(out.contains(&format!("✓ removed {RULE_PATH}")), "{out}");
    assert!(f.read(RULE_PATH).is_none());
    assert!(f.read("/etc/group").unwrap().contains("\noperator:*:5:root\n"), "nobody joins operator");
    assert!(!f.plan(Action::Remove).changes(), "nothing left to remove");
}

/// As a user, polkit's rules folder can't be read: the plan says so and
/// shows what root makes sure of, without guessing that nothing is needed.
#[test]
fn an_unreadable_rules_folder_leaves_the_decision_to_root() {
    if crate::install::is_root() {
        return; // root reads any folder
    }
    let f = Fixture::new(GROUP_CLEAN);
    f.plan(Action::Add).perform(&f.root(), &mut NoCommands).unwrap();
    let dir = f.root().path(RULES_DIR);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o000)).unwrap();
    let plan = f.plan(Action::Add);
    let remove = f.plan(Action::Remove);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(plan.unseen && plan.changes() && plan.errors.is_empty(), "{}", plan.describe());
    assert!(plan.describe().contains("readable only by root"), "{}", plan.describe());
    assert!(remove.unseen && remove.changes(), "{}", remove.describe());
    // Root (here: a readable folder) sees the rule and changes nothing.
    assert!(!f.plan(Action::Add).changes());
}

/// Elsewhere logind allows the login screen's buttons: nothing to do.
#[test]
fn other_systems_need_no_rule() {
    let f = Fixture::new(GROUP_V025);
    let plan = PowerRulePlan::build(&SysRoot::at(f.dir.path()), Action::Add);
    assert!(!plan.changes() && !plan.leave_group && plan.errors.is_empty());
    assert!(plan.describe().contains(STATUS_NOT_NEEDED));
}

/// A file at the rule's path that can't be read as text (not UTF-8) is
/// someone else's: never taken for an absent one, never replaced.
#[test]
fn an_unreadable_file_at_the_rule_path_is_left_alone() {
    let f = Fixture::new(GROUP_CLEAN);
    let p = f.root().path(RULE_PATH);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    let theirs = b"// caf\xe9: a Latin-1 comment\npolkit.addRule(function() {});\n";
    std::fs::write(&p, theirs).unwrap();
    for action in [Action::Add, Action::Remove] {
        let plan = f.plan(action);
        assert!(!plan.changes(), "{action:?}");
        assert!(plan.errors.iter().any(|e| e.contains("can't be read")), "{:?}", plan.errors);
    }
    assert_eq!(std::fs::read(&p).unwrap(), theirs, "untouched");
}
