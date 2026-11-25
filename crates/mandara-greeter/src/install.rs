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

use crate::manager::{BACKUP_DIR, BACKUP_DIR_MODE, Runner, SysRoot, SystemRunner, owned_by_us, plain_unit_stem, undo_command, write_atomic};

#[cfg(not(target_os = "freebsd"))]
pub const GREETD_CONFIG: &str = "/etc/greetd/config.toml";
/// FreeBSD: greetd reads `/usr/local/etc/greetd/config.toml` first (its
/// FreeBSD port's prefix).
#[cfg(target_os = "freebsd")]
pub const GREETD_CONFIG: &str = "/usr/local/etc/greetd/config.toml";
/// The VT a new greetd config runs on (FreeBSD: `ttyv8`, past the eight
/// gettys of `/etc/ttys`).
const FRESH_VT: i64 = if cfg!(target_os = "freebsd") { 8 } else { 1 };
pub const BACKUP_SUFFIX: &str = "mandara-backup";
pub const INSTALLED_EXE: &str = "/usr/local/lib/mandara/mandara-greeter";
pub const STATE_DIR: &str = "/var/lib/mandara-greeter";
/// Name of the file recording the display manager `greeter install`
/// replaced. It lives in [`BACKUP_DIR`] (only root writes there): the state
/// folder belongs to the greeter user, who parses untrusted avatars and
/// media, and `uninstall --apply` turns this name into a root `systemctl`
/// argument.
pub const PREVIOUS_DM_FILE: &str = "previous-dm";
/// Longest `previous-dm` file read: a unit name is short, and the legacy
/// copy sits in a folder an unprivileged user once owned.
const PREVIOUS_DM_MAX_BYTES: u64 = 256;
pub const MARKER: &str = "# Managed by `mandara greeter install`";
/// Permissions of the installed greeter binary.
const EXE_MODE: u32 = 0o755;
/// Permissions of the system media folders (world-readable).
const MEDIA_DIR_MODE: u32 = 0o755;
/// Permissions of the greeter's state dir (greeter user + group only).
const STATE_DIR_MODE: u32 = 0o750;
const KNOWN_DMS: [&str; 10] = ["sddm", "plasmalogin", "gdm", "gdm3", "lightdm", "ly", "lxdm", "xdm", "greetd", "entrance"];

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

    /// Whether greetd runs an older command of this kiosk that an update
    /// replaces: cage without `-m last` stretched the login screen across
    /// every monitor.
    pub fn outdated_command(self, command: &str) -> bool {
        self == Kiosk::Cage && command.trim() == format!("cage -s -- {INSTALLED_EXE} greeter")
    }

    /// greetd's `default_session.command`.
    pub fn command(self) -> String {
        match self {
            // One output (`-m last`): cage stretches its window across every
            // monitor otherwise.
            Kiosk::Cage => format!("cage -s -m last -- {INSTALLED_EXE} greeter"),
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
                "-- Hyprland as the login screen's compositor ({marker}).\nhl.config({{ misc = {{ disable_hyprland_logo = true, disable_splash_rendering = true, disable_watchdog_warning = true }}, animations = {{ enabled = false }} }})\nhl.window_rule({{ match = {{ class = \"^(org\\\\.mandara\\\\.greeter)$\" }}, fullscreen = true }})\nhl.on(\"hyprland.start\", function() hl.exec_cmd(\"{INSTALLED_EXE} greeter; hyprctl dispatch 'hl.dsp.exit()'\") end)\n"
            ),
        )
    } else {
        (
            HYPRLAND_GREETER_CONF,
            format!(
                "# Hyprland as the login screen's compositor ({MARKER}).\nmisc {{\n  disable_hyprland_logo = true\n  disable_splash_rendering = true\n  disable_watchdog_warning = true\n}}\nanimations {{\n  enabled = false\n}}\nwindowrule = match:class ^(org\\.mandara\\.greeter)$, fullscreen on\nexec-once = {INSTALLED_EXE} greeter; hyprctl dispatch exit\n"
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
        terminal.insert("vt".into(), toml::Value::Integer(FRESH_VT));
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

/// Password databases, in the order they are searched: `/etc/passwd`,
/// then `/usr/lib/passwd`, where image-based systems (Fedora Atomic's
/// nss-altfiles) keep the system users their packages create, such as
/// greetd's.
const PASSWD_FILES: [&str; 2] = ["etc/passwd", "usr/lib/passwd"];

/// `user`'s uid and gid from `<root>/etc/passwd` (or `/usr/lib/passwd`).
pub(crate) fn passwd_ids_in(root: &Path, user: &str) -> Option<(u32, u32)> {
    PASSWD_FILES.iter().find_map(|file| {
        let passwd = std::fs::read_to_string(root.join(file)).ok()?;
        passwd.lines().find_map(|l| {
            let f: Vec<&str> = l.split(':').collect();
            (f.len() >= 4 && f[0] == user).then(|| Some((f[2].parse().ok()?, f[3].parse().ok()?)))?
        })
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
    /// `undo[i]` reverses `commands[i]` (worked out before they run).
    pub undo: Vec<Vec<String>>,
    pub warnings: Vec<String>,
}

impl InstallPlan {
    pub fn build(kiosk: Kiosk) -> InstallPlan {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("mandara"));
        let existing = std::fs::read_to_string(GREETD_CONFIG).ok();
        let user = greeter_user(existing.as_deref());
        let new_config = render_greetd_config(existing.as_deref(), &kiosk.command(), &user);
        let root = crate::manager::SysRoot::system();
        let current_dm = if root.freebsd() { crate::manager::display_manager(&root) } else { current_display_manager(Path::new("/etc/systemd/system/display-manager.service")) };
        let mut commands = Vec::new();
        let mut undo = Vec::new();
        let mut service = |verb: &str, unit: &str| {
            commands.push(crate::manager::service_command(&root, verb, unit));
            undo.push(undo_command(&root, verb, unit));
        };
        if let Some(dm) = current_dm.as_deref().filter(|d| *d != "greetd") {
            service("disable", &format!("{dm}.service"));
        }
        if current_dm.as_deref() != Some("greetd") {
            service("enable", "greetd.service");
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
        InstallPlan { kiosk, exe, greeter_user: user, existing, new_config, current_dm, commands, undo, warnings }
    }

    pub fn describe(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(s, "Mandara Login — install plan\n");
        let _ = writeln!(s, "  1. Copy {} → {INSTALLED_EXE}", self.exe.display());
        let _ = writeln!(s, "  2. Create {} and {} (drop .webm/.mkv/.ivf clips and images there)", crate::config::SYSTEM_VIDEOS, crate::config::SYSTEM_IMAGES);
        let _ = writeln!(s, "  3. Create {STATE_DIR} owned by `{}` (remembers the last user / session)", self.greeter_user);
        let _ = writeln!(s, "  4. Create {} (sticky, like /tmp): sessions share each person's wallpaper there", mandara_config::paths::LOGIN_WALLPAPER_DIR);
        let _ = writeln!(s, "  5. Write {} if missing (theme, language, video settings)", crate::config::SYSTEM_CONFIG);
        let mut n = 6;
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
        self.perform(Path::new("/"), apply, &mut SystemRunner)
    }

    /// [`Self::execute`] without the root check, on `root` (tests use
    /// fixture trees). A failing step puts back every file it wrote and
    /// undoes the service commands that already ran, so a failed switch
    /// never leaves the old display manager disabled and greetd not enabled.
    pub(crate) fn perform(&self, root: &Path, apply: bool, runner: &mut dyn Runner) -> Result<(), String> {
        for line in install_runtime(root, &self.exe, &self.greeter_user)? {
            println!("✓ {line}");
        }
        if let Some(dm) = self.current_dm.as_deref().filter(|d| *d != "greetd") {
            record_previous_dm(root, dm)?;
        }
        // Files written so far, newest last, and how many service commands ran.
        let mut written = Vec::new();
        let mut done = 0;
        let result = self.switch(root, apply, runner, &mut written, &mut done);
        let Err(e) = result else { return Ok(()) };
        let mut notes = Vec::new();
        for undo in self.undo[..done].iter().rev() {
            if let Err(err) = runner.run(undo) {
                notes.push(err);
            }
        }
        for step in written.into_iter().rev() {
            if let Err(err) = step.undo() {
                notes.push(err);
            }
        }
        let tail = if notes.is_empty() { "everything was rolled back".to_string() } else { format!("rollback problems: {}", notes.join("; ")) };
        Err(format!("{e}\n({tail})"))
    }

    /// The writes and service commands of [`Self::perform`], recording what
    /// each one changed so a failure can undo it.
    fn switch(&self, root: &Path, apply: bool, runner: &mut dyn Runner, written: &mut Vec<Written>, done: &mut usize) -> Result<(), String> {
        if let Some((path, text)) = self.kiosk.config_file() {
            write_tracked(&under(root, path), &text, written)?;
            println!("✓ wrote {path}");
        }
        // greetd config (backup first; an existing backup is kept).
        let config = under(root, GREETD_CONFIG);
        let greetd_dir = config.parent().unwrap_or(root);
        std::fs::create_dir_all(greetd_dir).map_err(|e| format!("{}: {e}", greetd_dir.display()))?;
        let backup = under(root, &format!("{GREETD_CONFIG}.{BACKUP_SUFFIX}"));
        if let Some(existing) = &self.existing
            && !existing.starts_with(MARKER)
            && !backup.exists()
        {
            write_tracked(&backup, existing, written).map_err(|e| format!("backing up greetd config: {e}"))?;
            println!("✓ backed up the previous greetd config to {GREETD_CONFIG}.{BACKUP_SUFFIX}");
        }
        write_tracked(&config, &self.new_config, written)?;
        println!("✓ wrote {GREETD_CONFIG}");
        // Services.
        if self.commands.is_empty() {
            println!("\ngreetd is already enabled; Mandara Login appears at the next greetd start (reboot).");
        } else if apply {
            for c in &self.commands {
                runner.run(c)?;
                *done += 1;
            }
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

/// A file [`InstallPlan::perform`] wrote, and how to put it back.
enum Written {
    /// The file existed with this text.
    Replaced(PathBuf, String),
    /// The file didn't exist.
    Created(PathBuf),
}

impl Written {
    fn undo(self) -> Result<(), String> {
        match self {
            Written::Replaced(path, text) => write_atomic(&path, &text),
            Written::Created(path) => std::fs::remove_file(&path).map_err(|e| format!("{}: {e}", path.display())),
        }
    }
}

/// Writes `text` to `path` atomically and records what it replaced.
fn write_tracked(path: &Path, text: &str, written: &mut Vec<Written>) -> Result<(), String> {
    let previous = match std::fs::read_to_string(path) {
        Ok(old) => Some(old),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("{}: {e}", path.display())),
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    write_atomic(path, text)?;
    written.push(match previous {
        Some(old) => Written::Replaced(path.to_path_buf(), old),
        None => Written::Created(path.to_path_buf()),
    });
    Ok(())
}

/// Where `root`'s record of the display manager `greeter install` replaced
/// lives: in the folder only root writes.
fn previous_dm_path(root: &Path) -> PathBuf {
    under(root, BACKUP_DIR).join(PREVIOUS_DM_FILE)
}

/// Records `dm` as the display manager to go back to on `uninstall`.
fn record_previous_dm(root: &Path, dm: &str) -> Result<(), String> {
    use std::os::unix::fs::DirBuilderExt as _;
    let path = previous_dm_path(root);
    let dir = path.parent().unwrap_or(root);
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
    }
    match std::fs::DirBuilder::new().mode(BACKUP_DIR_MODE).create(dir) {
        Err(e) if e.kind() != std::io::ErrorKind::AlreadyExists => return Err(format!("{}: {e}", dir.display())),
        _ => {}
    }
    write_atomic(&path, &format!("{dm}\n"))
}

/// The display manager `greeter install` recorded, if the record can be
/// trusted: a service name in systemd's alphabet (no path, no leading `-`)
/// in a regular file that is not a link. A record in the greeter-owned
/// state folder (where older installs put it) counts only while root owns
/// it and nobody else may write it, since the greeter user could otherwise
/// plant a name that `uninstall --apply` hands to `systemctl` as root.
fn read_previous_dm(root: &Path) -> Option<String> {
    let current = previous_dm_path(root);
    let legacy = under(root, STATE_DIR).join(PREVIOUS_DM_FILE);
    let text = read_small_file(&current).or_else(|| owned_by_us(&legacy).then(|| read_small_file(&legacy)).flatten())?;
    let dm = text.trim();
    plain_unit_stem(dm).then(|| dm.to_string())
}

/// `path`'s text, at most [`PREVIOUS_DM_MAX_BYTES`], refusing a symlink.
fn read_small_file(path: &Path) -> Option<String> {
    use std::io::Read as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let file = std::fs::OpenOptions::new().read(true).custom_flags(libc::O_NOFOLLOW).open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    // One byte more than allowed: a longer file is refused, not cut short.
    file.take(PREVIOUS_DM_MAX_BYTES + 1).read_to_string(&mut text).ok()?;
    (text.len() as u64 <= PREVIOUS_DM_MAX_BYTES).then_some(text)
}

/// Joins an absolute system path onto a filesystem root (`/` in real use,
/// a fixture tree in tests).
pub(crate) fn under(root: &Path, abs: &str) -> PathBuf {
    root.join(abs.trim_start_matches('/'))
}

/// Copies `exe` over `dst` atomically, executable by everyone.
fn put_exe(exe: &Path, dst: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    let tmp = dst.with_extension("new");
    std::fs::copy(exe, &tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(EXE_MODE)).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, dst).map_err(|e| format!("{}: {e}", dst.display()))
}

/// Replaces the login screen's installed copy under `root` with `exe` when
/// it is another build. An older copy can't read a config this build
/// wrote (a newer element fails as an "unknown variant"). Leaves a system
/// without Mandara Login alone. `Ok(true)` when it was replaced.
pub fn refresh_installed_exe(root: &Path, exe: &Path) -> Result<bool, String> {
    let dst = under(root, INSTALLED_EXE);
    if !dst.is_file() || std::fs::canonicalize(exe).ok() == std::fs::canonicalize(&dst).ok() || mandara_config::admin::same_contents(exe, &dst) {
        return Ok(false);
    }
    put_exe(exe, &dst).map(|()| true)
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
        put_exe(exe, &dst)?;
    }
    done.push(format!("installed {INSTALLED_EXE}"));
    // 2. Media folders: in /usr/local/share where /usr is a read-only
    // image (Fedora Atomic, openSUSE Aeon); the login screen looks there too.
    let mut media_parent = "/usr/share/mandara/login";
    for (system, local) in [(crate::config::SYSTEM_VIDEOS, crate::config::LOCAL_VIDEOS), (crate::config::SYSTEM_IMAGES, crate::config::LOCAL_IMAGES)] {
        let (d, fell_back) = media_dir(root, system, local, &|d| std::fs::create_dir_all(d))?;
        if fell_back {
            media_parent = "/usr/local/share/mandara/login";
        }
        std::fs::set_permissions(&d, std::fs::Permissions::from_mode(MEDIA_DIR_MODE)).map_err(|e| err(&d, e))?;
    }
    done.push(format!("media folders under {media_parent}"));
    // 3. State dir.
    let state = under(root, STATE_DIR);
    std::fs::create_dir_all(&state).map_err(|e| err(&state, e))?;
    if let Some((uid, gid)) = passwd_ids_in(root, greeter_user) {
        std::os::unix::fs::chown(&state, Some(uid), Some(gid)).map_err(|e| err(&state, e))?;
    }
    std::fs::set_permissions(&state, std::fs::Permissions::from_mode(STATE_DIR_MODE)).map_err(|e| err(&state, e))?;
    // 4. The shared folder sessions copy each person's wallpaper into.
    let shared = mandara_config::paths::ensure_login_wallpaper_dir(root).map_err(|e| err(&under(root, mandara_config::paths::LOGIN_WALLPAPER_DIR), e))?;
    done.push(format!("shared wallpaper folder {}", shared.display()));
    // 5. Greeter config (only when missing).
    let sys = under(root, crate::config::SYSTEM_CONFIG);
    if !sys.exists() {
        let parent = sys.parent().unwrap_or(root).to_path_buf();
        std::fs::create_dir_all(&parent).map_err(|e| err(&parent, e))?;
        let text = format!("{MARKER}\n{}", toml::to_string(&crate::config::GreeterConfig::default()).unwrap_or_default());
        std::fs::write(&sys, text).map_err(|e| err(&sys, e))?;
        done.push(format!("wrote {}", crate::config::SYSTEM_CONFIG));
    }
    // 6. The login screen's language pack, next to the copy.
    let lang = crate::config::GreeterConfig::load_from(&sys).unwrap_or_default().lang();
    match install_language_pack(root, lang) {
        Ok(Some(line)) => done.push(line),
        Ok(None) => {}
        // Never fatal: the login screen shows English without it.
        Err(e) => done.push(format!("the {} language pack wasn't installed ({e}); the login screen shows English until it is", lang.code())),
    }
    Ok(done)
}

/// Where the login screen's language packs go: a system data dir it
/// searches (`/usr/local/share/mandara/locales/<version>/`), since it runs
/// as its own user and can't read anyone's home.
pub const SYSTEM_LOCALES: &str = "/usr/local/share";

/// Puts `lang`'s pack for this version where the installed copy finds it:
/// copied from a pack this binary can see (its own tarball or package, the
/// user who ran the command), else downloaded from this version's signed
/// release. `None` for English (no pack) or when it's already there.
pub fn install_language_pack(root: &Path, lang: mandara_i18n::Lang) -> Result<Option<String>, String> {
    use mandara_i18n::packs;
    if lang == mandara_i18n::Lang::En {
        return Ok(None);
    }
    let version = packs::running_version();
    let dir = under(root, SYSTEM_LOCALES).join(packs::DIR).join(version);
    let dest = dir.join(packs::file_name(lang));
    if packs::open(&dest, version, lang).is_ok() {
        return Ok(None);
    }
    // The person who ran `sudo` / `pkexec` keeps the packs in their home.
    let invoker = std::env::var("SUDO_UID").or_else(|_| std::env::var("PKEXEC_UID")).ok().and_then(|uid| uid.parse::<u32>().ok()).and_then(home_of);
    let candidates = invoker.map(|home| home.join(".local/share").join(packs::DIR).join(version).join(packs::file_name(lang))).into_iter().chain(packs::find(lang, version).map(|(p, _)| p));
    for path in candidates {
        if let Ok(bytes) = std::fs::read(&path)
            && packs::install(lang, version, &bytes, &dir).is_ok()
        {
            return Ok(Some(format!("{} language pack for the login screen (from {})", lang.code(), path.display())));
        }
    }
    if root != Path::new("/") {
        // A fixture tree (tests): never the network.
        return Err(format!("no {} pack for {version} to copy", lang.code()));
    }
    let langpacks = mandara_services::self_update::langpacks::published(&mandara_services::self_update::api_base(), version)?;
    mandara_services::self_update::langpacks::download(&langpacks, lang, &dir, &|_, _| {})?;
    Ok(Some(format!("{} language pack for the login screen (downloaded for {version})", lang.code())))
}

/// Creates the media folder `system` under `root` with `mkdir`, or `local`
/// when `system` is on a read-only file system (an image-based system's
/// `/usr`). The folder made, and whether it is `local`.
fn media_dir(root: &Path, system: &str, local: &str, mkdir: &dyn Fn(&Path) -> std::io::Result<()>) -> Result<(PathBuf, bool), String> {
    let d = under(root, system);
    match mkdir(&d) {
        Ok(()) => Ok((d, false)),
        Err(e) if e.kind() == std::io::ErrorKind::ReadOnlyFilesystem => {
            let d = under(root, local);
            mkdir(&d).map_err(|e| format!("{}: {e}", d.display()))?;
            Ok((d, true))
        }
        Err(e) => Err(format!("{}: {e}", d.display())),
    }
}

/// `uid`'s home folder, from the password database.
fn home_of(uid: u32) -> Option<PathBuf> {
    // SAFETY: getpwuid returns a pointer into static storage or null; the
    // fields are read before any other passwd call on this thread.
    unsafe {
        let pw = libc::getpwuid(uid);
        if pw.is_null() || (*pw).pw_dir.is_null() {
            return None;
        }
        Some(PathBuf::from(std::ffi::CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned()))
    }
}

/// Runs `commands`; when one fails, runs the `undo` of those that already
/// ran (newest first) so the system keeps the display manager it had.
fn run_switch(commands: &[Vec<String>], undo: &[Vec<String>], runner: &mut dyn Runner) -> Result<(), String> {
    for (done, c) in commands.iter().enumerate() {
        if let Err(e) = runner.run(c) {
            let notes: Vec<String> = undo[..done].iter().rev().filter_map(|u| runner.run(u).err()).collect();
            let tail = if notes.is_empty() { "the services were put back as they were".to_string() } else { format!("rollback problems: {}", notes.join("; ")) };
            return Err(format!("{e}\n({tail})"));
        }
    }
    Ok(())
}

pub struct UninstallPlan {
    pub backup: Option<PathBuf>,
    pub ours: bool,
    pub previous_dm: Option<String>,
    pub commands: Vec<Vec<String>>,
    /// `undo[i]` reverses `commands[i]`.
    pub undo: Vec<Vec<String>>,
}

impl UninstallPlan {
    pub fn build() -> UninstallPlan {
        let backup = PathBuf::from(format!("{GREETD_CONFIG}.{BACKUP_SUFFIX}"));
        let backup = backup.exists().then_some(backup);
        let ours = std::fs::read_to_string(GREETD_CONFIG).is_ok_and(|t| t.starts_with(MARKER));
        let previous_dm = read_previous_dm(Path::new("/"));
        let mut commands = Vec::new();
        let mut undo = Vec::new();
        if let Some(dm) = &previous_dm {
            let root = SysRoot::system();
            let dm_unit = format!("{dm}.service");
            for (verb, unit) in [("disable", "greetd.service"), ("enable", dm_unit.as_str())] {
                commands.push(crate::manager::service_command(&root, verb, unit));
                undo.push(undo_command(&root, verb, unit));
            }
        }
        UninstallPlan { backup, ours, previous_dm, commands, undo }
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
        let _ = writeln!(s, "  2. Remove {INSTALLED_EXE}, {NIRI_GREETER_KDL}, {HYPRLAND_GREETER_CONF}, {HYPRLAND_GREETER_LUA} and the language packs in {SYSTEM_LOCALES}/mandara/locales");
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
        let locales = Path::new(SYSTEM_LOCALES).join(mandara_i18n::packs::DIR);
        if std::fs::remove_dir_all(&locales).is_ok() {
            println!("✓ removed {}", locales.display());
        }
        if apply && !self.commands.is_empty() {
            run_switch(&self.commands, &self.undo, &mut SystemRunner)?;
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
    /// Image-based systems: greetd's user lives in /usr/lib/passwd, and a
    /// read-only /usr/share sends the media folders to /usr/local/share.
    #[test]
    fn image_based_systems_keep_users_in_usr_lib_and_media_in_usr_local() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("etc")).unwrap();
        std::fs::create_dir_all(root.join("usr/lib")).unwrap();
        std::fs::write(root.join("etc/passwd"), "root:x:0:0:root:/root:/bin/bash\nu:x:1000:1000::/var/home/u:/bin/bash\n").unwrap();
        std::fs::write(root.join("usr/lib/passwd"), "greeter:x:973:973:greetd greeter user:/var/lib/greetd:/sbin/nologin\n").unwrap();
        assert_eq!(passwd_ids_in(root, "u"), Some((1000, 1000)));
        assert_eq!(passwd_ids_in(root, "greeter"), Some((973, 973)), "from /usr/lib/passwd");
        assert_eq!(passwd_ids_in(root, "nobody-here"), None);
        let read_only_usr_share = |d: &Path| if d.starts_with(root.join("usr/share")) { Err(std::io::Error::from(std::io::ErrorKind::ReadOnlyFilesystem)) } else { std::fs::create_dir_all(d) };
        let (d, local) = media_dir(root, crate::config::SYSTEM_VIDEOS, crate::config::LOCAL_VIDEOS, &read_only_usr_share).unwrap();
        assert!(local && d == root.join("usr/local/share/mandara/login/videos") && d.is_dir());
        let (d, local) = media_dir(root, crate::config::SYSTEM_IMAGES, crate::config::LOCAL_IMAGES, &|d| std::fs::create_dir_all(d)).unwrap();
        assert!(!local && d == root.join("usr/share/mandara/login/backgrounds"));
        let denied = |_: &Path| Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
        assert!(media_dir(root, crate::config::SYSTEM_IMAGES, crate::config::LOCAL_IMAGES, &denied).is_err(), "only a read-only file system falls back");
    }

    use super::*;

    #[test]
    fn rewrites_greetd_config_keeping_other_tables() {
        let existing = "[terminal]\nvt = 2\n\n[default_session]\ncommand = \"agreety --cmd /bin/sh\"\nuser = \"greeter\"\n\n[initial_session]\ncommand = \"sway\"\nuser = \"me\"\n";
        let out = render_greetd_config(Some(existing), &Kiosk::Cage.command(), &greeter_user(Some(existing)));
        assert!(out.starts_with(MARKER));
        let t: toml::Table = toml::from_str(&out).unwrap();
        assert_eq!(t["terminal"]["vt"].as_integer(), Some(2));
        assert_eq!(t["default_session"]["command"].as_str(), Some(format!("cage -s -m last -- {INSTALLED_EXE} greeter").as_str()));
        assert_eq!(t["default_session"]["user"].as_str(), Some("greeter"));
        assert_eq!(t["initial_session"]["command"].as_str(), Some("sway"));
    }

    #[test]
    fn fresh_config_gets_vt1_and_default_user() {
        let out = render_greetd_config(None, "x", &greeter_user(None));
        let t: toml::Table = toml::from_str(&out).unwrap();
        assert_eq!(t["terminal"]["vt"].as_integer(), Some(FRESH_VT), "1 on Linux, 8 on FreeBSD");
        assert_eq!(t["default_session"]["user"].as_str(), Some("greeter"));
        assert_eq!(greeter_user(Some("[default_session]\nuser = \"_greetd\"\n")), "_greetd");
    }

    #[test]
    fn detects_display_manager_from_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let link = dir.path().join("display-manager.service");
        for manager in ["sddm", "plasmalogin"] {
            std::os::unix::fs::symlink(format!("/usr/lib/systemd/system/{manager}.service"), &link).unwrap();
            assert_eq!(current_display_manager(&link), Some(manager.into()));
            assert!(KNOWN_DMS.contains(&manager), "a recognised manager must not get the unknown-manager warning");
            std::fs::remove_file(&link).unwrap();
        }
        assert_eq!(current_display_manager(&dir.path().join("missing")), None);
    }

    #[test]
    fn kiosk_commands() {
        assert_eq!(Kiosk::parse("Cage"), Some(Kiosk::Cage));
        assert_eq!(Kiosk::parse("hyprland"), Some(Kiosk::Hyprland));
        assert_eq!(Kiosk::parse("weston"), None);
        assert!(Kiosk::Cage.config_file().is_none());
        // The login screen shows on one monitor, and an update moves older
        // installs (stretched across every monitor) onto that command.
        assert!(Kiosk::Cage.command().contains(" -m last "));
        assert!(Kiosk::Cage.outdated_command(&format!("cage -s -- {INSTALLED_EXE} greeter")));
        assert!(!Kiosk::Cage.outdated_command(&Kiosk::Cage.command()));
        assert!(!Kiosk::Niri.outdated_command(&format!("cage -s -- {INSTALLED_EXE} greeter")));
        assert!(Kiosk::Niri.config_file().unwrap().1.contains("spawn-at-startup"));
        assert!(Kiosk::Hyprland.command().contains("--config /etc/mandara/greeter-hyprland."));
        // hyprlang (≤ 0.56): current rule syntax (`windowrulev2` is an error).
        // `disable_watchdog_warning` keeps the greeter from showing Hyprland's
        // "started without start-hyprland" toast (greetd starts Hyprland directly).
        let (path, conf) = hyprland_greeter_config(false);
        assert!(
            path.ends_with(".conf")
                && conf.contains("windowrule = match:class ^(org\\.mandara\\.greeter)$, fullscreen on")
                && conf.contains("disable_watchdog_warning = true")
                && !conf.contains("windowrulev2")
        );
        // Lua (0.57+): a Lua dispatch to quit.
        let (path, lua) = hyprland_greeter_config(true);
        assert!(
            path.ends_with(".lua")
                && lua.starts_with("-- ")
                && lua.contains("fullscreen = true")
                && lua.contains("hl.dsp.exit()")
                && lua.contains("disable_watchdog_warning = true"),
            "{lua}"
        );
    }

    /// Hyprland's own checker accepts the greeter configs, when Hyprland is installed.
    #[test]
    fn hyprland_validates_the_greeter_config() {
        if !std::process::Command::new("Hyprland").arg("--version").output().is_ok_and(|o| o.status.success()) {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        for lua in [false, true] {
            let (name, text) = hyprland_greeter_config(lua);
            let file = dir.path().join(Path::new(name).file_name().unwrap());
            std::fs::write(&file, &text).unwrap();
            let out = std::process::Command::new("Hyprland").arg("--config").arg(&file).arg("--verify-config").output().unwrap();
            assert!(
                out.status.success(),
                "{name}: {}{}",
                String::from_utf8_lossy(&out.stdout),
                String::from_utf8_lossy(&out.stderr)
            );
        }
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

    /// Records the commands it is asked to run and fails on the one named.
    struct ScriptedRunner {
        ran: Vec<String>,
        fail_on: Option<&'static str>,
    }

    impl Runner for ScriptedRunner {
        fn run(&mut self, argv: &[String]) -> Result<(), String> {
            let line = argv.join(" ");
            self.ran.push(line.clone());
            if self.fail_on == Some(line.as_str()) { Err(format!("`{line}` failed")) } else { Ok(()) }
        }
    }

    fn systemd_plan(root: &Path, existing: Option<&str>) -> InstallPlan {
        let exe = root.join("mandara");
        std::fs::write(&exe, "binary").unwrap();
        let kiosk = Kiosk::Niri;
        let user = greeter_user(existing);
        let new_config = render_greetd_config(existing, &kiosk.command(), &user);
        let cmd = |verb: &str, unit: &str| vec!["systemctl".to_string(), verb.to_string(), unit.to_string()];
        InstallPlan {
            kiosk,
            exe,
            greeter_user: user,
            existing: existing.map(str::to_string),
            new_config,
            current_dm: Some("sddm".into()),
            commands: vec![cmd("disable", "sddm.service"), cmd("enable", "greetd.service")],
            undo: vec![cmd("enable", "sddm.service"), cmd("disable", "greetd.service")],
            warnings: Vec::new(),
        }
    }

    /// Enabling greetd fails after the old display manager was disabled:
    /// the old one is enabled again and the files are put back, so the next
    /// boot still has a login screen.
    #[test]
    fn a_failed_service_switch_rolls_back_services_and_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let original = "[default_session]\ncommand = \"agreety --cmd /bin/sh\"\nuser = \"greeter\"\n";
        let config = under(root, GREETD_CONFIG);
        std::fs::create_dir_all(config.parent().unwrap()).unwrap();
        std::fs::write(&config, original).unwrap();
        let plan = systemd_plan(root, Some(original));
        let mut runner = ScriptedRunner { ran: Vec::new(), fail_on: Some("systemctl enable greetd.service") };
        let err = plan.perform(root, true, &mut runner).unwrap_err();
        assert!(err.contains("`systemctl enable greetd.service` failed") && err.contains("everything was rolled back"), "{err}");
        assert_eq!(runner.ran, ["systemctl disable sddm.service", "systemctl enable greetd.service", "systemctl enable sddm.service"], "only the command that succeeded is undone");
        assert_eq!(std::fs::read_to_string(&config).unwrap(), original, "the greetd config is back");
        assert!(!under(root, &format!("{GREETD_CONFIG}.{BACKUP_SUFFIX}")).exists(), "the backup this run made is gone");
        assert!(!under(root, NIRI_GREETER_KDL).exists(), "the kiosk file this run made is gone");
    }

    #[test]
    fn a_successful_switch_writes_the_files_and_records_the_old_manager() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let plan = systemd_plan(root, None);
        let mut runner = ScriptedRunner { ran: Vec::new(), fail_on: None };
        plan.perform(root, true, &mut runner).unwrap();
        assert_eq!(runner.ran, ["systemctl disable sddm.service", "systemctl enable greetd.service"]);
        assert_eq!(std::fs::read_to_string(under(root, GREETD_CONFIG)).unwrap(), plan.new_config);
        assert!(under(root, NIRI_GREETER_KDL).is_file());
        assert_eq!(read_previous_dm(root).as_deref(), Some("sddm"));
        assert!(previous_dm_path(root).starts_with(under(root, BACKUP_DIR)), "recorded where only root writes");
        assert!(!under(root, STATE_DIR).join(PREVIOUS_DM_FILE).exists(), "not in the greeter-owned folder");
    }

    #[test]
    fn uninstall_rolls_back_when_the_old_manager_cannot_be_enabled() {
        let commands = vec![vec!["systemctl".to_string(), "disable".into(), "greetd.service".into()], vec!["systemctl".to_string(), "enable".into(), "sddm.service".into()]];
        let undo = vec![vec!["systemctl".to_string(), "enable".into(), "greetd.service".into()], vec!["systemctl".to_string(), "disable".into(), "sddm.service".into()]];
        let mut runner = ScriptedRunner { ran: Vec::new(), fail_on: Some("systemctl enable sddm.service") };
        let err = run_switch(&commands, &undo, &mut runner).unwrap_err();
        assert!(err.contains("put back as they were"), "{err}");
        assert_eq!(runner.ran, ["systemctl disable greetd.service", "systemctl enable sddm.service", "systemctl enable greetd.service"]);
    }

    /// `uninstall --apply` turns the recorded name into a root `systemctl`
    /// argument: a path, an option, a link or an oversized file is ignored.
    #[test]
    fn previous_dm_record_is_validated() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(read_previous_dm(root), None, "nothing recorded");
        record_previous_dm(root, "sddm").unwrap();
        assert_eq!(read_previous_dm(root).as_deref(), Some("sddm"));
        let path = previous_dm_path(root);
        for bad in ["/var/lib/mandara-greeter/evil", "../evil", "--root=/tmp/x", "-x", "a b", "sddm\nenable evil", ""] {
            std::fs::write(&path, format!("{bad}\n")).unwrap();
            assert_eq!(read_previous_dm(root), None, "{bad:?}");
        }
        std::fs::write(&path, format!("{}\n", "a".repeat(1000))).unwrap();
        assert_eq!(read_previous_dm(root), None, "oversized");
        // A link is never followed, even to a valid name.
        let elsewhere = root.join("elsewhere");
        std::fs::write(&elsewhere, "sddm\n").unwrap();
        std::fs::remove_file(&path).unwrap();
        std::os::unix::fs::symlink(&elsewhere, &path).unwrap();
        assert_eq!(read_previous_dm(root), None, "symlink");
        // Older installs kept it in the state folder: honoured only while
        // root (here: the test's user) owns it and nobody else may write.
        std::fs::remove_file(&path).unwrap();
        let legacy = under(root, STATE_DIR).join(PREVIOUS_DM_FILE);
        std::fs::create_dir_all(legacy.parent().unwrap()).unwrap();
        std::fs::write(&legacy, "gdm\n").unwrap();
        std::fs::set_permissions(&legacy, std::os::unix::fs::PermissionsExt::from_mode(0o644)).unwrap();
        assert_eq!(read_previous_dm(root).as_deref(), Some("gdm"));
        std::fs::set_permissions(&legacy, std::os::unix::fs::PermissionsExt::from_mode(0o666)).unwrap();
        assert_eq!(read_previous_dm(root), None, "writable by others");
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
