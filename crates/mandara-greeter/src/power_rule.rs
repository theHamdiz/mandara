//! The login screen's power buttons on FreeBSD
//! (`mandara greeter power-rule [--apply|--remove] [--dry-run]`).
//!
//! Mandara Login asks ConsoleKit2 to suspend, restart or power off
//! ([`crate::power`]). The login screen's user (greetd's
//! `[default_session] user`, usually `greeter`) has no ConsoleKit2
//! session, so ConsoleKit2's defaults refuse it. One polkit rule, in a
//! drop-in file of Mandara's own ([`RULE_PATH`]), allows exactly that user
//! exactly the three ConsoleKit2 actions in [`ACTIONS`] and nothing else:
//! no other user, no `…-multiple-users` variant (another user's session
//! still needs an administrator), no hibernate, no pkexec.
//!
//! Mandara v0.24 and v0.25 put that user in `operator` instead, which
//! FreeBSD lets run `shutdown` but which can also read raw disks. When the
//! rule is first written for a user, the plan takes that user out of
//! `operator` again: only greetd's login-screen user, only a membership
//! listed in `/etc/group` (not its primary group), and only an account
//! that can't log in (a `nologin` or `false` shell). Anyone else is left
//! alone with a warning. Later runs never touch `operator`, so a membership
//! an administrator adds back by hand stays.
//!
//! Every change is planned first, needs root, is backed up under
//! [`crate::manager::BACKUP_DIR`] with what it did recorded in the backup's
//! manifest (so `greeter restore` removes the rule again), verified after
//! writing and rolled back if writing fails. Re-runs change nothing. A file
//! at [`RULE_PATH`] that isn't Mandara's is never replaced. Everything
//! takes a [`SysRoot`] so tests run against fixture trees.

use std::fmt::Write as _;
use std::os::unix::fs::PermissionsExt as _;

use crate::keyring_pam::Action;
use crate::manager::{BACKUP_DIR, Backup, DEFAULT_GREETER_USER, FileWrite, GreetdConfig, LeftGroup, Manifest, Runner, SysRoot, write_atomic};

/// Mandara's polkit rule. polkitd reads the `rules.d` folders under
/// `/usr/local/etc/polkit-1` and `/usr/local/share/polkit-1` and notices
/// new files by itself; the rule answers only for its user and actions.
pub const RULE_PATH: &str = "/usr/local/etc/polkit-1/rules.d/50-mandara-greeter.rules";
/// The folder the polkit package creates for local rules.
pub const RULES_DIR: &str = "/usr/local/etc/polkit-1/rules.d";
/// ConsoleKit2's polkit actions, installed with the consolekit2 package.
pub const CONSOLEKIT_POLICY: &str = "/usr/local/share/polkit-1/actions/org.freedesktop.consolekit.policy";
/// The first line of Mandara's rule: a file without it isn't Mandara's.
pub const MARKER: &str = "// Mandara's file, written by `mandara greeter power-rule`.";
/// What the rule allows: ConsoleKit2's power off, restart and suspend.
pub const ACTIONS: [&str; 3] = ["org.freedesktop.consolekit.system.stop", "org.freedesktop.consolekit.system.restart", "org.freedesktop.consolekit.system.suspend"];
/// The group v0.24 and v0.25 put the login screen's user in.
pub const LEGACY_GROUP: &str = "operator";
/// FreeBSD's tool for group membership.
pub const PW: &str = "/usr/sbin/pw";
/// The backup manifest's `target` for this change.
pub const BACKUP_TARGET: &str = "power-rule";
/// The rule's mode: polkitd reads it as its own user.
const RULE_MODE: u32 = 0o644;
/// What [`rule`] puts before the user name, so [`rule_user`] finds it.
const USER_TEST: &str = "if (subject.user !== \"";
/// Longest user name FreeBSD's `pw` accepts.
const MAX_USER_LEN: usize = 32;
pub const STATUS_UP_TO_DATE: &str = "Status: up to date";
pub const STATUS_CHANGES: &str = "Status: changes needed";
pub const STATUS_NOT_NEEDED: &str = "Status: not needed on this system";

/// The rule for `user`.
pub fn rule(user: &str) -> String {
    let mut s = String::new();
    let _ = writeln!(s, "{MARKER}");
    let _ = writeln!(s, "// Undo: sudo mandara greeter power-rule --remove");
    let _ = writeln!(s, "//");
    let _ = writeln!(s, "// Mandara Login's Sleep, Restart and Shut Down buttons: the login screen's");
    let _ = writeln!(s, "// user may suspend, restart and power off through ConsoleKit2, and nothing");
    let _ = writeln!(s, "// else. While another user is logged in, those still need an administrator.");
    let _ = writeln!(s, "polkit.addRule(function(action, subject) {{");
    let _ = writeln!(s, "    {USER_TEST}{user}\") {{");
    let _ = writeln!(s, "        return polkit.Result.NOT_HANDLED;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "    switch (action.id) {{");
    for a in ACTIONS {
        let _ = writeln!(s, "    case \"{a}\":");
    }
    let _ = writeln!(s, "        return polkit.Result.YES;");
    let _ = writeln!(s, "    default:");
    let _ = writeln!(s, "        return polkit.Result.NOT_HANDLED;");
    let _ = writeln!(s, "    }}");
    let _ = writeln!(s, "}});");
    s
}

/// The user a Mandara rule allows.
pub fn rule_user(text: &str) -> Option<&str> {
    if !text.starts_with(MARKER) {
        return None;
    }
    let at = text.find(USER_TEST)? + USER_TEST.len();
    text[at..].split('"').next()
}

/// A name that fits a polkit rule's string as is (no quotes, escapes or
/// line breaks) and that `pw` accepts.
fn plain_user(user: &str) -> bool {
    !user.is_empty() && user.len() <= MAX_USER_LEN && !user.starts_with('-') && user.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))
}

/// `user`'s `/etc/passwd` entry: (uid, gid, shell).
fn passwd_entry(passwd: &str, user: &str) -> Option<(u32, u32, String)> {
    passwd.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() >= 7 && f[0] == user).then(|| Some((f[2].parse().ok()?, f[3].parse().ok()?, f[6].trim().to_string())))?
    })
}

/// `group`'s gid and listed members in `/etc/group`.
fn group_entry<'a>(groups: &'a str, group: &str) -> Option<(u32, Vec<&'a str>)> {
    groups.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() >= 4 && f[0] == group).then(|| Some((f[2].parse().ok()?, f[3].split(',').map(str::trim).filter(|m| !m.is_empty()).collect())))?
    })
}

/// An account nobody logs in to (greetd's users are made this way).
fn no_login(shell: &str) -> bool {
    matches!(shell.rsplit('/').next(), Some("nologin" | "false"))
}

/// What `greeter power-rule` would change, built from what is on disk.
#[derive(Clone, Debug)]
pub struct PowerRulePlan {
    pub action: Action,
    /// Only FreeBSD needs the rule; elsewhere logind allows the buttons.
    pub freebsd: bool,
    /// greetd's login-screen user.
    pub user: String,
    /// [`RULE_PATH`]'s text now (None: it doesn't exist).
    pub previous: Option<String>,
    /// … and afterwards (None: it doesn't exist afterwards).
    pub contents: Option<String>,
    /// Takes [`Self::user`] out of [`LEGACY_GROUP`].
    pub leave_group: bool,
    /// polkit keeps its rules folders to itself (`polkitd`, mode 0700), so
    /// a plan made as another user can't see the rule: it shows what root
    /// makes sure of, and root's run changes only what's missing.
    pub unseen: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl PowerRulePlan {
    pub fn build(root: &SysRoot, action: Action) -> PowerRulePlan {
        let user = GreetdConfig::read(root).user.unwrap_or_else(|| DEFAULT_GREETER_USER.into());
        let (previous, unseen) = match std::fs::read_to_string(root.path(RULE_PATH)) {
            Ok(text) => (Some(text), false),
            Err(e) => (None, e.kind() == std::io::ErrorKind::PermissionDenied),
        };
        let mut plan = PowerRulePlan { action, freebsd: root.freebsd(), user, contents: previous.clone(), previous, leave_group: false, unseen, errors: Vec::new(), warnings: Vec::new() };
        if !plan.freebsd {
            return plan;
        }
        let ours = plan.previous.as_deref().is_none_or(|p| p.starts_with(MARKER));
        if !ours {
            plan.errors.push(format!("{RULE_PATH} exists and isn't Mandara's: it was left alone (move it away to let Mandara write its rule)."));
            return plan;
        }
        if action == Action::Remove {
            plan.contents = None;
            return plan;
        }
        if !plain_user(&plan.user) {
            plan.errors.push(format!("greetd's login-screen user {:?} isn't a plain user name, so no rule is written for it.", plan.user));
            return plan;
        }
        let passwd = root.read("/etc/passwd").unwrap_or_default();
        let Some((uid, gid, shell)) = passwd_entry(&passwd, &plan.user) else {
            plan.errors.push(format!("the login screen's user {} doesn't exist yet (greetd's setup creates it).", plan.user));
            return plan;
        };
        if !root.path(RULES_DIR).is_dir() {
            plan.errors.push(format!("polkit isn't installed ({RULES_DIR} is missing): pkg install polkit"));
        }
        if !root.path(CONSOLEKIT_POLICY).is_file() {
            plan.errors.push("ConsoleKit2 isn't installed: pkg install consolekit2".into());
        }
        if !plan.errors.is_empty() {
            return plan;
        }
        plan.contents = Some(rule(&plan.user));
        let groups = root.read("/etc/group").unwrap_or_default();
        let Some((group_gid, members)) = group_entry(&groups, LEGACY_GROUP) else { return plan };
        let first_rule = plan.previous.as_deref().and_then(rule_user) != Some(plan.user.as_str());
        let listed = members.contains(&plan.user.as_str());
        let can_read_disks = format!("{} is in {LEGACY_GROUP}, which can read raw disks; the login screen doesn't need it.", plan.user);
        if gid == group_gid {
            plan.warnings.push(format!("{can_read_disks} It is the user's primary group, so Mandara leaves it: change it with pw usermod {} -g <group>.", plan.user));
        } else if listed && first_rule && uid != 0 && no_login(&shell) {
            plan.leave_group = true;
        } else if listed {
            plan.warnings.push(format!("{can_read_disks} Left as it is; take it out with: pw groupmod {LEGACY_GROUP} -d {}", plan.user));
        }
        plan
    }

    /// The command that takes the user out of [`LEGACY_GROUP`].
    pub fn leave_command(&self) -> Vec<String> {
        vec![PW.into(), "groupmod".into(), LEGACY_GROUP.into(), "-d".into(), self.user.clone()]
    }

    fn rule_changes(&self) -> bool {
        self.contents != self.previous || self.unseen
    }

    /// The result differs from what's there.
    pub fn changes(&self) -> bool {
        self.freebsd && self.errors.is_empty() && (self.rule_changes() || self.leave_group)
    }

    pub fn describe(&self) -> String {
        let mut s = String::new();
        let verb = match self.action {
            Action::Add => "power buttons",
            Action::Remove => "remove the power-button rule",
        };
        let _ = writeln!(s, "Login screen — {verb} plan (FreeBSD)\n");
        if !self.freebsd {
            let _ = writeln!(s, "  Only FreeBSD needs this: logind lets the login screen suspend, restart and power off.");
            let _ = writeln!(s, "\n{STATUS_NOT_NEEDED}");
            return s;
        }
        let _ = writeln!(s, "  User:   {} (greetd's [default_session] user)", self.user);
        let _ = writeln!(s, "  Rule:   {RULE_PATH}");
        if self.unseen {
            let _ = writeln!(s, "\n  polkit's rules folder is readable only by root, so this plan shows what");
            let _ = writeln!(s, "  root makes sure of; run as root, it changes only what's missing.");
        }
        if self.changes() {
            let _ = writeln!(s, "\n  Changes (backed up first to {BACKUP_DIR}/<timestamp>/):");
            match (&self.contents, self.rule_changes()) {
                (_, false) => {}
                (None, true) => {
                    let _ = writeln!(s, "    delete {RULE_PATH} (Mandara's rule)");
                }
                (Some(text), true) => {
                    let _ = writeln!(s, "    write {RULE_PATH}:");
                    for line in text.lines() {
                        let _ = writeln!(s, "    + {line}");
                    }
                }
            }
            if self.leave_group {
                let _ = writeln!(s, "    $ {}", self.leave_command().join(" "));
                let _ = writeln!(s, "      ({} leaves {LEGACY_GROUP}, which Mandara v0.24 and v0.25 added it to; that group can read raw disks)", self.user);
            }
        } else if self.errors.is_empty() {
            let _ = writeln!(s, "\n  Nothing to change.");
        }
        if self.action == Action::Add && self.errors.is_empty() {
            let _ = writeln!(s, "\n  Sleep, Restart and Shut Down on the login screen go through ConsoleKit2; {} gets no other rights.", self.user);
            let _ = writeln!(s, "  Undo: sudo mandara greeter power-rule --remove  (or: sudo mandara greeter restore latest)");
        }
        if self.action == Action::Remove && self.errors.is_empty() {
            let _ = writeln!(s, "\n  Without the rule the login screen's power buttons can't work; power off from a session instead.");
        }
        for e in &self.errors {
            let _ = writeln!(s, "\n  error: {e}");
        }
        for w in &self.warnings {
            let _ = writeln!(s, "\n  warning: {w}");
        }
        let status = if !self.errors.is_empty() {
            "Status: error"
        } else if self.changes() {
            STATUS_CHANGES
        } else {
            STATUS_UP_TO_DATE
        };
        let _ = writeln!(s, "\n{status}");
        s
    }

    /// Performs the plan as root (backup, write, verify; rollback if the
    /// rule can't be written).
    pub fn execute(&self, root: &SysRoot, runner: &mut dyn Runner) -> Result<String, String> {
        if !self.errors.is_empty() {
            return Err(format!("{}\n(nothing was changed)", self.errors.join("\n")));
        }
        if !self.changes() {
            return Ok("Nothing to change.".into());
        }
        if !crate::install::is_root() {
            return Err("not running as root: re-run with sudo to change the login screen's power rule (nothing was changed)".into());
        }
        self.perform(root, runner)
    }

    /// [`Self::execute`] without the root check (tests use fixture roots).
    pub(crate) fn perform(&self, root: &SysRoot, runner: &mut dyn Runner) -> Result<String, String> {
        if !self.errors.is_empty() {
            return Err(format!("{}\n(nothing was changed)", self.errors.join("\n")));
        }
        if !self.changes() {
            return Ok("Nothing to change.".into());
        }
        // The file must still be what the plan was built from.
        if root.read(RULE_PATH) != self.previous {
            return Err(format!("{RULE_PATH} changed since the plan was made; run it again (nothing was changed)"));
        }
        let manifest = Manifest { created: chrono::Local::now().to_rfc3339(), target: BACKUP_TARGET.into(), previous_unit: None, new_unit: None, units_switched: false, undo: Vec::new(), files: Vec::new(), left_groups: Vec::new() };
        let writes: Vec<FileWrite> = if self.rule_changes() { vec![FileWrite { path: RULE_PATH.into(), contents: self.contents.clone().unwrap_or_default(), previous: self.previous.clone() }] } else { Vec::new() };
        let mut backup = Backup::create(root, manifest, &writes)?;
        let mut log = vec![format!("backed up to {}", backup.logical_dir())];
        if self.rule_changes()
            && let Err(e) = self.write_rule(root, &mut log)
        {
            let tail = match backup.restore_files(root) {
                Ok(_) => "everything was rolled back".to_string(),
                Err(err) => format!("rollback problem: {err}"),
            };
            return Err(format!("{e}\n({tail})"));
        }
        let mut s = String::new();
        if self.leave_group {
            match runner.run(&self.leave_command()) {
                Ok(()) => {
                    backup.manifest.left_groups.push(LeftGroup { user: self.user.clone(), group: LEGACY_GROUP.into() });
                    backup.save()?;
                    log.push(format!("took {} out of {LEGACY_GROUP} (Mandara v0.24 and v0.25 added it; that group can read raw disks)", self.user));
                }
                Err(e) => {
                    let _ = writeln!(s, "warning: {} is still in {LEGACY_GROUP}: {e}\nTake it out with: pw groupmod {LEGACY_GROUP} -d {}\n", self.user, self.user);
                }
            }
        }
        let done: String = log.iter().map(|l| format!("✓ {l}\n")).collect();
        s.insert_str(0, &done);
        match self.action {
            Action::Add => {
                let _ = write!(s, "\nThe login screen's Sleep, Restart and Shut Down buttons work through ConsoleKit2 (no restart needed).\nUndo: sudo mandara greeter power-rule --remove");
            }
            Action::Remove => {
                let _ = write!(s, "\nThe login screen's power buttons no longer have Mandara's rule.");
            }
        }
        Ok(s)
    }

    fn write_rule(&self, root: &SysRoot, log: &mut Vec<String>) -> Result<(), String> {
        let p = root.path(RULE_PATH);
        match &self.contents {
            Some(text) => {
                write_atomic(&p, text)?;
                std::fs::set_permissions(&p, std::fs::Permissions::from_mode(RULE_MODE)).map_err(|e| format!("{RULE_PATH}: {e}"))?;
                if root.read(RULE_PATH).as_deref() != Some(text.as_str()) {
                    return Err(format!("{RULE_PATH} doesn't read back as written"));
                }
                log.push(format!("wrote {RULE_PATH} (lets {} suspend, restart and power off through ConsoleKit2)", self.user));
            }
            None => {
                std::fs::remove_file(&p).map_err(|e| format!("{RULE_PATH}: {e}"))?;
                log.push(format!("removed {RULE_PATH} (Mandara's rule)"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "power_rule_tests.rs"]
mod tests;
