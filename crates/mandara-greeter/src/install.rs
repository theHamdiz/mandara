//! `mandara greeter install` / `uninstall`: register Mandara Login as the
//! system's login manager through greetd, and undo it.
//!
//! Nothing here runs implicitly: the plan is always printed first, the
//! filesystem is only touched when the command runs as root, and service
//! changes (`systemctl disable <old>` + `systemctl enable greetd`) only run
//! with `--apply`. Services are never started or stopped, so the running
//! session is not affected; the switch takes effect at the next boot.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub const GREETD_CONFIG: &str = "/etc/greetd/config.toml";
pub const BACKUP_SUFFIX: &str = "mandara-backup";
pub const INSTALLED_EXE: &str = "/usr/local/lib/mandara/mandara-greeter";
pub const STATE_DIR: &str = "/var/lib/mandara-greeter";
pub const MARKER: &str = "# Managed by `mandara greeter install`";
/// Permissions of the installed greeter binary.
const EXE_MODE: u32 = 0o755;
/// Permissions of the system media folders (world-readable).
const MEDIA_DIR_MODE: u32 = 0o755;
/// Permissions of the greeter's state dir (greeter user + group only).
const STATE_DIR_MODE: u32 = 0o750;
const KNOWN_DMS: [&str; 9] = ["sddm", "gdm", "gdm3", "lightdm", "ly", "lxdm", "xdm", "greetd", "entrance"];

/// The minimal compositor greetd runs the greeter in.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Kiosk {
    Cage,
    Niri,
    Hyprland,
}

impl Kiosk {
    pub fn parse(s: &str) -> Option<Kiosk> {
        match s.to_ascii_lowercase().as_str() {
            "cage" => Some(Kiosk::Cage),
            "niri" => Some(Kiosk::Niri),
            "hyprland" | "hypr" => Some(Kiosk::Hyprland),
            _ => None,
        }
    }

    pub fn program(self) -> &'static str {
        match self {
            Kiosk::Cage => "cage",
            Kiosk::Niri => "niri",
            Kiosk::Hyprland => "Hyprland",
        }
    }

    /// The compositor config file this kiosk needs (cage needs none).
    pub fn config_file(self) -> Option<(&'static str, String)> {
        match self {
            Kiosk::Cage => None,
            Kiosk::Niri => Some((
                NIRI_GREETER_KDL,
                format!(
                    "// Niri as the login screen's compositor ({MARKER}).\nhotkey-overlay {{ skip-at-startup; }}\nprefer-no-csd\nanimations {{ off; }}\nspawn-at-startup \"sh\" \"-c\" \"{INSTALLED_EXE} greeter; niri msg action quit --skip-confirmation\"\nwindow-rule {{ open-fullscreen true; }}\n"
                ),
            )),
            Kiosk::Hyprland => Some(hyprland_greeter_config(hyprland_lua())),
        }
    }

    /// greetd's `default_session.command`.
    pub fn command(self) -> String {
        match self {
            Kiosk::Cage => format!("cage -s -- {INSTALLED_EXE} greeter"),
            Kiosk::Niri => "niri -c /etc/mandara/greeter-niri.kdl".to_string(),
            Kiosk::Hyprland => format!("Hyprland --config {}", hyprland_greeter_config(hyprland_lua()).0),
        }
    }
}

/// The Hyprland greeter configs: hyprlang (≤ 0.56) and Lua (0.57+, where
/// hyprlang is gone and an explicit `--config` is read as Lua).
pub const HYPRLAND_GREETER_CONF: &str = "/etc/mandara/greeter-hyprland.conf";
pub const HYPRLAND_GREETER_LUA: &str = "/etc/mandara/greeter-hyprland.lua";
pub const NIRI_GREETER_KDL: &str = "/etc/mandara/greeter-niri.kdl";

/// Whether the installed Hyprland only reads Lua configs.
fn hyprland_lua() -> bool {
    mandara_config::compositor::installed_version(mandara_config::compositor::Target::Hyprland).is_some_and(mandara_config::compositor::hyprland_reads_lua)
}

/// Path and text of Hyprland's greeter config in the dialect it reads.
pub fn hyprland_greeter_config(lua: bool) -> (&'static str, String) {
    if lua {
        let marker = MARKER.trim_start_matches('#').trim();
        (
            HYPRLAND_GREETER_LUA,
            format!(
                "-- Hyprland as the login screen's compositor ({marker}).\nhl.config({{ misc = {{ disable_hyprland_logo = true, disable_splash_rendering = true }}, animations = {{ enabled = false }} }})\nhl.window_rule({{ match = {{ class = \"^(org\\\\.mandara\\\\.greeter)$\" }}, fullscreen = true }})\nhl.on(\"hyprland.start\", function() hl.exec_cmd(\"{INSTALLED_EXE} greeter; hyprctl dispatch 'hl.dsp.exit()'\") end)\n"
            ),
        )
    } else {
        (
            HYPRLAND_GREETER_CONF,
            format!(
                "# Hyprland as the login screen's compositor ({MARKER}).\nmisc {{\n  disable_hyprland_logo = true\n  disable_splash_rendering = true\n}}\nanimations {{\n  enabled = false\n}}\nwindowrule = match:class ^(org\\.mandara\\.greeter)$, fullscreen on\nexec-once = {INSTALLED_EXE} greeter; hyprctl dispatch exit\n"
            ),
        )
    }
}

/// The display manager behind `display-manager.service` (`sddm`, `gdm`…).
pub fn current_display_manager(link: &Path) -> Option<String> {
    let target = std::fs::read_link(link).ok()?;
    let name = target.file_name()?.to_string_lossy().trim_end_matches(".service").to_string();
    (!name.is_empty()).then_some(name)
}

/// Rewrites an existing greetd config: keeps `[terminal]` and any other
/// tables (e.g. `initial_session`), replaces `[default_session]`.
pub fn render_greetd_config(existing: Option<&str>, command: &str, user: &str) -> String {
    let mut table: toml::Table = existing.and_then(|t| toml::from_str(t).ok()).unwrap_or_default();
    let mut session = toml::Table::new();
    session.insert("command".into(), toml::Value::String(command.into()));
    session.insert("user".into(), toml::Value::String(user.into()));
    table.insert("default_session".into(), toml::Value::Table(session));
    if !table.contains_key("terminal") {
        let mut terminal = toml::Table::new();
        terminal.insert("vt".into(), toml::Value::Integer(1));
        table.insert("terminal".into(), toml::Value::Table(terminal));
    }
    format!("{MARKER}; previous file: {GREETD_CONFIG}.{BACKUP_SUFFIX}\n\n{}", toml::to_string(&table).unwrap_or_default())
}

/// The greeter user from an existing config, else `greeter`.
pub fn greeter_user(existing: Option<&str>) -> String {
    existing
        .and_then(|t| toml::from_str::<toml::Table>(t).ok())
        .and_then(|t| t.get("default_session")?.get("user")?.as_str().map(str::to_string))
        .unwrap_or_else(|| "greeter".into())
}

/// `user`'s uid and gid from `<root>/etc/passwd`.
pub(crate) fn passwd_ids_in(root: &Path, user: &str) -> Option<(u32, u32)> {
    let passwd = std::fs::read_to_string(root.join("etc/passwd")).ok()?;
    passwd.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() >= 4 && f[0] == user).then(|| Some((f[2].parse().ok()?, f[3].parse().ok()?)))?
    })
}

fn passwd_ids(user: &str) -> Option<(u32, u32)> {
    passwd_ids_in(Path::new("/"), user)
}

pub(crate) fn is_root() -> bool {
    // SAFETY: geteuid has no preconditions.
    unsafe { libc::geteuid() == 0 }
}

pub struct InstallPlan {
    pub kiosk: Kiosk,
    pub exe: PathBuf,
    pub greeter_user: String,
    pub existing: Option<String>,
    pub new_config: String,
    pub current_dm: Option<String>,
    pub commands: Vec<Vec<String>>,
    pub warnings: Vec<String>,
}

impl InstallPlan {
    pub fn build(kiosk: Kiosk) -> InstallPlan {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("mandara"));
        let existing = std::fs::read_to_string(GREETD_CONFIG).ok();
        let user = greeter_user(existing.as_deref());
        let new_config = render_greetd_config(existing.as_deref(), &kiosk.command(), &user);
        let current_dm = current_display_manager(Path::new("/etc/systemd/system/display-manager.service"));
        let mut commands = Vec::new();
        if let Some(dm) = current_dm.as_deref().filter(|d| *d != "greetd") {
            commands.push(vec!["systemctl".into(), "disable".into(), format!("{dm}.service")]);
        }
        if current_dm.as_deref() != Some("greetd") {
            commands.push(vec!["systemctl".into(), "enable".into(), "greetd.service".into()]);
        }
        let mut warnings = Vec::new();
        if !crate::sessions::program_exists("greetd") && !Path::new("/usr/bin/greetd").exists() {
            warnings.push("greetd is not installed (package `greetd`); install it before rebooting.".into());
        }
        if !crate::sessions::program_exists(kiosk.program()) {
            warnings.push(format!("`{}` is not installed; install it (package `{}`) or pick another with --compositor.", kiosk.program(), kiosk.program().to_lowercase()));
        }
        if passwd_ids(&user).is_none() {
            warnings.push(format!("user `{user}` does not exist yet (the greetd package normally creates it)."));
        }
        if let Some(dm) = current_dm.as_deref()
            && !KNOWN_DMS.contains(&dm)
        {
            warnings.push(format!("display-manager.service points to `{dm}`, which isn't a display manager this installer knows; check before applying."));
        }
        InstallPlan { kiosk, exe, greeter_user: user, existing, new_config, current_dm, commands, warnings }
    }

    pub fn describe(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "Mandara Login — install plan\n");
        let _ = writeln!(s, "  1. Copy {} → {INSTALLED_EXE}", self.exe.display());
        let _ = writeln!(s, "  2. Create {} and {} (drop .webm/.mkv/.ivf clips and images there)", crate::config::SYSTEM_VIDEOS, crate::config::SYSTEM_IMAGES);
        let _ = writeln!(s, "  3. Create {STATE_DIR} owned by `{}` (remembers the last user / session)", self.greeter_user);
        let _ = writeln!(s, "  4. Write {} if missing (theme, language, video settings)", crate::config::SYSTEM_CONFIG);
        let mut n = 5;
        if let Some((path, _)) = self.kiosk.config_file() {
            let _ = writeln!(s, "  {n}. Write {path} ({} as the greeter's compositor)", self.kiosk.program());
            n += 1;
        }
        match &self.existing {
            Some(_) => {
                let _ = writeln!(s, "  {n}. Back up {GREETD_CONFIG} → {GREETD_CONFIG}.{BACKUP_SUFFIX} and write:");
            }
            None => {
                let _ = writeln!(s, "  {n}. Write {GREETD_CONFIG}:");
            }
        }
        for line in self.new_config.lines() {
            let _ = writeln!(s, "        {line}");
        }
        let _ = writeln!(s);
        match self.current_dm.as_deref() {
            Some("greetd") => {
                let _ = writeln!(s, "  greetd is already the display manager; nothing to switch.");
            }
            Some(dm) => {
                let _ = writeln!(s, "  Current display manager: {dm}");
            }
            None => {
                let _ = writeln!(s, "  No display manager is enabled (display-manager.service is not set).");
            }
        }
        if !self.commands.is_empty() {
            let _ = writeln!(s, "  To switch at the next boot (run with --apply to do it now; nothing is stopped or started):");
            for c in &self.commands {
                let _ = writeln!(s, "      sudo {}", c.join(" "));
            }
        }
        for w in &self.warnings {
            let _ = writeln!(s, "\n  warning: {w}");
        }
        s
    }

    /// Performs the plan (root only). Service commands run only with `apply`.
    pub fn execute(&self, apply: bool) -> Result<(), String> {
        if !is_root() {
            return Err("not running as root: re-run with sudo to install (nothing was changed)".into());
        }
        for line in install_runtime(Path::new("/"), &self.exe, &self.greeter_user)? {
            println!("✓ {line}");
        }
        if let Some(dm) = self.current_dm.as_deref().filter(|d| *d != "greetd") {
            let _ = std::fs::write(Path::new(STATE_DIR).join("previous-dm"), format!("{dm}\n"));
        }
        let err = |what: &str, e: std::io::Error| format!("{what}: {e}");
        if let Some((path, text)) = self.kiosk.config_file() {
            std::fs::write(path, text).map_err(|e| err(path, e))?;
            println!("✓ wrote {path}");
        }
        // 5. greetd config (backup first; an existing backup is kept).
        std::fs::create_dir_all("/etc/greetd").map_err(|e| err("/etc/greetd", e))?;
        let backup = format!("{GREETD_CONFIG}.{BACKUP_SUFFIX}");
        if let Some(existing) = &self.existing
            && !existing.starts_with(MARKER)
            && !Path::new(&backup).exists()
        {
            std::fs::write(&backup, existing).map_err(|e| err("backing up greetd config", e))?;
            println!("✓ backed up the previous greetd config to {backup}");
        }
        std::fs::write(GREETD_CONFIG, &self.new_config).map_err(|e| err(GREETD_CONFIG, e))?;
        println!("✓ wrote {GREETD_CONFIG}");
        // 6. Services.
        if self.commands.is_empty() {
            println!("\ngreetd is already enabled; Mandara Login appears at the next greetd start (reboot).");
        } else if apply {
            run_all(&self.commands)?;
            println!("\nDone. Mandara Login takes over at the next boot. Your current session was not touched.");
        } else {
            println!("\nFiles are in place. To switch display managers at the next boot run:");
            for c in &self.commands {
                println!("    sudo {}", c.join(" "));
            }
            println!("(or re-run with --apply)");
        }
        Ok(())
    }
}

/// Joins an absolute system path onto a filesystem root (`/` in real use,
/// a fixture tree in tests).
pub(crate) fn under(root: &Path, abs: &str) -> PathBuf {
    root.join(abs.trim_start_matches('/'))
}

/// Puts Mandara Login's runtime in place under `root`: the binary at
/// [`INSTALLED_EXE`], the media folders, the state dir (owned by the
/// greeter user) and a default system config if there is none. Returns one
/// line per step done. Never touches greetd's config or any service.
pub fn install_runtime(root: &Path, exe: &Path, greeter_user: &str) -> Result<Vec<String>, String> {
    use std::os::unix::fs::PermissionsExt as _;
    let err = |what: &Path, e: std::io::Error| format!("{}: {e}", what.display());
    let mut done = Vec::new();
    // 1. Binary.
    let dst = under(root, INSTALLED_EXE);
    let parent = dst.parent().unwrap_or(root).to_path_buf();
    std::fs::create_dir_all(&parent).map_err(|e| err(&parent, e))?;
    if std::fs::canonicalize(exe).ok() != std::fs::canonicalize(&dst).ok() {
        let tmp = dst.with_extension("new");
        std::fs::copy(exe, &tmp).map_err(|e| err(&tmp, e))?;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(EXE_MODE)).map_err(|e| err(&tmp, e))?;
        std::fs::rename(&tmp, &dst).map_err(|e| err(&dst, e))?;
    }
    done.push(format!("installed {INSTALLED_EXE}"));
    // 2. Media folders.
    for d in [crate::config::SYSTEM_VIDEOS, crate::config::SYSTEM_IMAGES] {
        let d = under(root, d);
        std::fs::create_dir_all(&d).map_err(|e| err(&d, e))?;
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(MEDIA_DIR_MODE)).map_err(|e| err(&d, e))?;
    }
    done.push("media folders under /usr/share/mandara/login".into());
    // 3. State dir.
    let state = under(root, STATE_DIR);
    std::fs::create_dir_all(&state).map_err(|e| err(&state, e))?;
    if let Some((uid, gid)) = passwd_ids_in(root, greeter_user) {
        std::os::unix::fs::chown(&state, Some(uid), Some(gid)).map_err(|e| err(&state, e))?;
    }
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(STATE_DIR_MODE)).map_err(|e| err(&state, e))?;
    // 4. Greeter config (only when missing).
    let sys = under(root, crate::config::SYSTEM_CONFIG);
    if !sys.exists() {
        let parent = sys.parent().unwrap_or(root).to_path_buf();
        std::fs::create_dir_all(&parent).map_err(|e| err(&parent, e))?;
        let text = format!("{MARKER}\n{}", toml::to_string(&crate::config::GreeterConfig::default()).unwrap_or_default());
        std::fs::write(&sys, text).map_err(|e| err(&sys, e))?;
        done.push(format!("wrote {}", crate::config::SYSTEM_CONFIG));
    }
    Ok(done)
}

fn run_all(commands: &[Vec<String>]) -> Result<(), String> {
    for c in commands {
        println!("$ {}", c.join(" "));
        let status = std::process::Command::new(&c[0]).args(&c[1..]).status().map_err(|e| format!("{}: {e}", c[0]))?;
        if !status.success() {
            return Err(format!("`{}` failed ({status})", c.join(" ")));
        }
    }
    Ok(())
}

pub struct UninstallPlan {
    pub backup: Option<PathBuf>,
    pub ours: bool,
    pub previous_dm: Option<String>,
    pub commands: Vec<Vec<String>>,
}

impl UninstallPlan {
    pub fn build() -> UninstallPlan {
        let backup = PathBuf::from(format!("{GREETD_CONFIG}.{BACKUP_SUFFIX}"));
        let backup = backup.exists().then_some(backup);
        let ours = std::fs::read_to_string(GREETD_CONFIG).is_ok_and(|t| t.starts_with(MARKER));
        let previous_dm = std::fs::read_to_string(Path::new(STATE_DIR).join("previous-dm")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        let mut commands = Vec::new();
        if let Some(dm) = &previous_dm {
            commands.push(vec!["systemctl".into(), "disable".into(), "greetd.service".into()]);
            commands.push(vec!["systemctl".into(), "enable".into(), format!("{dm}.service")]);
        }
        UninstallPlan { backup, ours, previous_dm, commands }
    }

    pub fn describe(&self) -> String {
        let mut s = String::from("Mandara Login — uninstall plan\n\n");
        match (&self.backup, self.ours) {
            (Some(b), _) => {
                let _ = writeln!(s, "  1. Restore {} → {GREETD_CONFIG}", b.display());
            }
            (None, true) => {
                let _ = writeln!(s, "  1. Remove {GREETD_CONFIG} (written by Mandara; there was no previous file)");
            }
            (None, false) => {
                let _ = writeln!(s, "  1. Leave {GREETD_CONFIG} alone (not written by Mandara)");
            }
        }
        let _ = writeln!(s, "  2. Remove {INSTALLED_EXE}, {NIRI_GREETER_KDL}, {HYPRLAND_GREETER_CONF}, {HYPRLAND_GREETER_LUA}");
        let _ = writeln!(s, "     (kept: {}, /usr/share/mandara/login, {STATE_DIR})", crate::config::SYSTEM_CONFIG);
        match &self.previous_dm {
            Some(dm) => {
                let _ = writeln!(s, "\n  To go back to {dm} at the next boot (run with --apply to do it now):");
                for c in &self.commands {
                    let _ = writeln!(s, "      sudo {}", c.join(" "));
                }
            }
            None => {
                let _ = writeln!(s, "\n  No previous display manager was recorded. Re-enable yours with e.g.:\n      sudo systemctl disable greetd.service && sudo systemctl enable sddm.service");
            }
        }
        s
    }

    pub fn execute(&self, apply: bool) -> Result<(), String> {
        if !is_root() {
            return Err("not running as root: re-run with sudo to uninstall (nothing was changed)".into());
        }
        if let Some(b) = &self.backup {
            std::fs::rename(b, GREETD_CONFIG).map_err(|e| format!("restoring {}: {e}", b.display()))?;
            println!("✓ restored {GREETD_CONFIG}");
        } else if self.ours {
            std::fs::remove_file(GREETD_CONFIG).map_err(|e| format!("{GREETD_CONFIG}: {e}"))?;
            println!("✓ removed {GREETD_CONFIG}");
        }
        for f in [INSTALLED_EXE, NIRI_GREETER_KDL, HYPRLAND_GREETER_CONF, HYPRLAND_GREETER_LUA] {
            if std::fs::remove_file(f).is_ok() {
                println!("✓ removed {f}");
            }
        }
        if apply && !self.commands.is_empty() {
            run_all(&self.commands)?;
            println!("\nDone; {} returns at the next boot.", self.previous_dm.as_deref().unwrap_or("your display manager"));
        } else if !self.commands.is_empty() {
            println!("\nTo switch back at the next boot run:");
            for c in &self.commands {
                println!("    sudo {}", c.join(" "));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_greetd_config_keeping_other_tables() {
        let existing = "[terminal]\nvt = 2\n\n[default_session]\ncommand = \"agreety --cmd /bin/sh\"\nuser = \"greeter\"\n\n[initial_session]\ncommand = \"sway\"\nuser = \"me\"\n";
        let out = render_greetd_config(Some(existing), &Kiosk::Cage.command(), &greeter_user(Some(existing)));
        assert!(out.starts_with(MARKER));
        let t: toml::Table = toml::from_str(&out).unwrap();
        assert_eq!(t["terminal"]["vt"].as_integer(), Some(2));
        assert_eq!(t["default_session"]["command"].as_str(), Some(format!("cage -s -- {INSTALLED_EXE} greeter").as_str()));
        assert_eq!(t["default_session"]["user"].as_str(), Some("greeter"));
        assert_eq!(t["initial_session"]["command"].as_str(), Some("sway"));
    }

    #[test]
    fn fresh_config_gets_vt1_and_default_user() {
        let out = render_greetd_config(None, "x", &greeter_user(None));
        let t: toml::Table = toml::from_str(&out).unwrap();
        assert_eq!(t["terminal"]["vt"].as_integer(), Some(1));
        assert_eq!(t["default_session"]["user"].as_str(), Some("greeter"));
        assert_eq!(greeter_user(Some("[default_session]\nuser = \"_greetd\"\n")), "_greetd");
    }

    #[test]
    fn detects_display_manager_from_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("display-manager.service");
        std::os::unix::fs::symlink("/usr/lib/systemd/system/sddm.service", &link).unwrap();
        assert_eq!(current_display_manager(&link), Some("sddm".into()));
        assert_eq!(current_display_manager(&dir.path().join("missing")), None);
    }

    #[test]
    fn kiosk_commands() {
        assert_eq!(Kiosk::parse("Cage"), Some(Kiosk::Cage));
        assert_eq!(Kiosk::parse("hyprland"), Some(Kiosk::Hyprland));
        assert_eq!(Kiosk::parse("weston"), None);
        assert!(Kiosk::Cage.config_file().is_none());
        assert!(Kiosk::Niri.config_file().unwrap().1.contains("spawn-at-startup"));
        assert!(Kiosk::Hyprland.command().contains("--config /etc/mandara/greeter-hyprland."));
        // hyprlang (≤ 0.56): current rule syntax (`windowrulev2` is an error).
        let (path, conf) = hyprland_greeter_config(false);
        assert!(path.ends_with(".conf") && conf.contains("windowrule = match:class ^(org\\.mandara\\.greeter)$, fullscreen on") && !conf.contains("windowrulev2"));
        // Lua (0.57+): a Lua dispatch to quit.
        let (path, lua) = hyprland_greeter_config(true);
        assert!(path.ends_with(".lua") && lua.starts_with("-- ") && lua.contains("fullscreen = true") && lua.contains("hl.dsp.exit()"), "{lua}");
    }

    /// Niri's own parser accepts the greeter config, when niri is installed.
    #[test]
    fn niri_validates_the_greeter_config() {
        if !std::process::Command::new("niri").arg("--version").output().is_ok_and(|o| o.status.success()) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("greeter.kdl");
        std::fs::write(&file, Kiosk::Niri.config_file().unwrap().1).unwrap();
        let out = std::process::Command::new("niri").arg("validate").arg("-c").arg(&file).output().unwrap();
        assert!(out.status.success(), "{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    }

    #[test]
    fn refuses_without_root() {
        if is_root() {
            return;
        }
        let plan = InstallPlan::build(Kiosk::Cage);
        assert!(plan.execute(false).is_err());
        assert!(UninstallPlan::build().execute(false).is_err());
        assert!(plan.describe().contains(GREETD_CONFIG));
    }
}
