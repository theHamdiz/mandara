//! The root side: `mandara boot-admin …`, started by Studio or `mandara
//! msg boot apply|restore` through `pkexec <admin helper>` (the root-owned
//! copy of Mandara, `mandara_config::admin`).
//!
//! This is a security boundary, and it changes what the machine boots
//! with. The helper:
//!
//! * accepts only the fixed actions of [`Action::parse`]: `apply <file>`
//!   (a [`super::plan::Approved`] request) and `restore <id|latest>`;
//! * detects the system itself, with root's view, never from what the
//!   caller says; copies the staged theme into a root-owned folder with
//!   the archive rules (theme file types only, no links), checks it there,
//!   and rebuilds the plan: a digest that differs from the approved one
//!   changes nothing and prints the new plan;
//! * runs only the programs the plan names, from root-owned system
//!   folders, with a cleared environment and no input;
//! * backs up every file before changing it into
//!   `/var/lib/mandara-boot/backups/<id>/` (the record readable by you,
//!   the copies by root only), rebuilds GRUB's menu into a scratch file
//!   and checks it with `grub-script-check` before it replaces the old
//!   one, and puts everything back when a step fails;
//! * [`restore`] undoes an apply: unchanged files come back whole, files
//!   you edited since get only Mandara's lines reverted, and the menu or
//!   initramfs is rebuilt the same checked way.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use super::detect::{self, Detection};
use super::edit;
use super::plan::{self, Approved, GRUB_NEW_SUFFIX, Plan, Request, Step};
use super::theme::{self, Kind};
use super::{BACKUP_DIR, INCOMING_DIR, INSTALLED_FILE, Sys, archive};

pub const EXIT_OK: i32 = 0;
pub const EXIT_FAILED: i32 = 1;
pub const EXIT_USAGE: i32 = 2;
pub const EXIT_NOT_ROOT: i32 = 3;
pub const EXIT_UNSUPPORTED: i32 = 4;
/// The plan with root's view differs from the approved one.
pub const EXIT_PLAN_CHANGED: i32 = 5;
/// Largest request file read.
const REQUEST_MAX: u64 = 1 << 20;
/// Output lines of a failed command kept for the message.
const TAIL_LINES: usize = 12;
/// Largest staged wallpaper accepted.
const WALLPAPER_MAX: u64 = 32 << 20;
const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
/// `PATH` for the programs the helper runs.
const SAFE_PATH: &str = "/usr/sbin:/usr/bin:/sbin:/bin";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Apply(PathBuf),
    Restore(String),
}

impl Action {
    pub const USAGE: &str = "usage: mandara boot-admin apply <request.json> | restore <latest|ID>";

    pub fn parse(args: &[String]) -> Result<Action, String> {
        match args.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
            ["apply", file] if Path::new(file).is_absolute() => Ok(Action::Apply(PathBuf::from(file))),
            ["restore", id] if *id == "latest" || valid_id(id) => Ok(Action::Restore(id.to_string())),
            _ => Err(Self::USAGE.into()),
        }
    }
}

/// A backup id: `YYYYMMDD-HHMMSS-<kind>[-n]`.
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && !id.starts_with(['.', '-']) && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

/// Runs the plan's programs.
pub trait Runner {
    /// Runs argv; Ok(stdout) on success, Err(the output's tail) otherwise.
    fn run(&self, argv: &[String]) -> Result<String, String>;
}

/// The real runner: root-owned programs from system folders only.
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, argv: &[String]) -> Result<String, String> {
        let prog = Path::new(argv.first().ok_or("empty command")?);
        let allowed = prog.parent().is_some_and(|d| detect::TOOL_DIRS.iter().any(|t| Path::new(t) == d)) && mandara_config::admin::trusted_root_file(prog);
        if !allowed {
            return Err(format!("{} isn't a root-owned system program", prog.display()));
        }
        let out = Command::new(prog).args(&argv[1..]).env_clear().env("PATH", SAFE_PATH).env("LANG", "C").env("LC_ALL", "C").env("HOME", "/root").stdin(Stdio::null()).output().map_err(|e| format!("{}: {e}", prog.display()))?;
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            let tail: Vec<&str> = text.lines().rev().take(TAIL_LINES).collect();
            Err(format!("{} failed ({}):\n{}", argv.join(" "), out.status, tail.into_iter().rev().collect::<Vec<_>>().join("\n")))
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum State {
    Applied,
    RolledBack,
    Restored,
}

/// How a changed file is undone when it was edited after the apply.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "edit", rename_all = "kebab-case")]
pub enum Edit {
    /// Put the whole copy back.
    Whole,
    GrubKeys { previous: edit::Previous },
    LimineBlock,
    RefindBlock,
    /// Mandara's own file: removed.
    Owned,
    /// Generated (GRUB's menu): rebuilt, not restored.
    Generated,
    /// `plymouthd.conf`: the previous theme is selected again.
    PlymouthConf { previous: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileBackup {
    pub path: PathBuf,
    pub existed: bool,
    /// The copy's name in the backup folder.
    pub copy: Option<String>,
    /// SHA-256 after the apply (`None`: removed or absent).
    pub after: Option<String>,
    pub edit: Edit,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirChange {
    pub path: PathBuf,
    /// Mandara's previous version of the folder, kept in the backup.
    pub previous: Option<String>,
}

/// One apply, as kept in its backup folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Record {
    pub id: String,
    pub created: u64,
    pub kind: String,
    /// What was applied ("mandara-vimix-1080p", "Limine look", …).
    pub what: String,
    pub state: State,
    pub files: Vec<FileBackup>,
    pub dirs: Vec<DirChange>,
    /// The steps to run again after a restore (menu, initramfs, enrol).
    pub regenerate: Vec<Step>,
}

/// The theme folders Mandara installed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Installed {
    pub dirs: Vec<PathBuf>,
}

impl Installed {
    pub fn load(sys: &Sys) -> Self {
        detect::read(sys, INSTALLED_FILE).and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default()
    }

    fn save(&self, sys: &Sys) -> Result<(), String> {
        write_file(sys, Path::new(INSTALLED_FILE), serde_json::to_string_pretty(self).unwrap_or_default().as_bytes(), 0o644)
    }
}

/// Every backup record, newest first (readable without root).
pub fn records(sys: &Sys) -> Vec<Record> {
    let mut out: Vec<Record> = std::fs::read_dir(sys.path(BACKUP_DIR)).into_iter().flatten().flatten().filter_map(|e| std::fs::read_to_string(e.path().join("record.json")).ok()).filter_map(|t| serde_json::from_str(&t).ok()).collect();
    out.sort_by(|a: &Record, b: &Record| b.created.cmp(&a.created).then(b.id.cmp(&a.id)));
    out
}

/// The apply "Restore previous" undoes: the newest one still applied.
pub fn latest(sys: &Sys) -> Option<Record> {
    records(sys).into_iter().find(|r| r.state == State::Applied)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// `YYYYMMDD-HHMMSS` (UTC) of Unix seconds `t`.
fn stamp(t: u64) -> String {
    let days = (t / 86_400) as i64;
    let secs = t % 86_400;
    // Civil date from days since 1970-01-01 (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
}

/// Writes `bytes` to system path `path` through a scratch file in the
/// same folder (a crash leaves the old file or the new one, never half).
pub fn write_file(sys: &Sys, path: &Path, bytes: &[u8], mode: u32) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    let real = sys.path(path);
    let dir = real.parent().ok_or("no parent folder")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mode = std::fs::metadata(&real).map(|m| m.permissions().mode() & 0o7777).unwrap_or(mode);
    let tmp = dir.join(format!(".{}.{}", real.file_name().unwrap_or_default().to_string_lossy(), super::MARK));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp).map_err(|e| format!("{}: {e}", tmp.display()))?;
        f.write_all(bytes).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        // FAT has no modes: best effort.
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(mode));
        std::fs::rename(&tmp, &real).map_err(|e| format!("{}: {e}", real.display()))
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn read_text(sys: &Sys, path: &Path) -> Result<String, String> {
    std::fs::read_to_string(sys.path(path)).map_err(|e| format!("{}: {e}", path.display()))
}

/// Copies a tree (theme folders between file systems), theme files only.
fn install_dir(from: &Path, to: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    archive::copy_tree(from, to, archive::INSTALL_LIMITS)?;
    for f in archive::files(to) {
        let _ = std::fs::set_permissions(to.join(f), std::fs::Permissions::from_mode(0o644));
    }
    Ok(())
}

/// An apply in progress: what to undo if a step fails.
struct Journal<'a> {
    sys: &'a Sys,
    dir: PathBuf,
    record: Record,
    ran: Vec<Step>,
}

impl Journal<'_> {
    fn backup(&mut self, path: &Path) -> Result<(), String> {
        if self.record.files.iter().any(|f| f.path == path) {
            return Ok(());
        }
        let real = self.sys.path(path);
        let existed = std::fs::symlink_metadata(&real).is_ok_and(|m| m.is_file());
        let copy = if existed {
            use std::os::unix::fs::PermissionsExt as _;
            let name = format!("file-{}", self.record.files.len());
            let dest = self.dir.join(&name);
            std::fs::copy(&real, &dest).map_err(|e| format!("backing up {}: {e}", path.display()))?;
            let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(0o600));
            Some(name)
        } else {
            None
        };
        self.record.files.push(FileBackup { path: path.to_path_buf(), existed, copy, after: None, edit: Edit::Whole });
        Ok(())
    }

    fn set_edit(&mut self, path: &Path, edit: Edit) {
        if let Some(f) = self.record.files.iter_mut().find(|f| f.path == path) {
            f.edit = edit;
        }
    }

    /// Puts every backed-up file back and removes installed folders.
    fn rollback(&mut self, runner: &dyn Runner) -> Vec<String> {
        let mut problems = Vec::new();
        for f in self.record.files.iter().rev() {
            if let Err(e) = put_back(self.sys, &self.dir, f) {
                problems.push(e);
            }
        }
        for d in self.record.dirs.iter().rev() {
            let real = self.sys.path(&d.path);
            let _ = std::fs::remove_dir_all(&real);
            if let Some(prev) = &d.previous
                && let Err(e) = install_dir(&self.dir.join(prev), &real)
            {
                problems.push(e);
            }
        }
        // Configs are back: bring what was built from them back too.
        let regen: Vec<Step> = self.ran.iter().filter(|s| matches!(s, Step::Initramfs { .. } | Step::LimineEnroll { .. })).cloned().collect();
        for step in regen {
            if let Err(e) = run_step(self.sys, runner, &step) {
                problems.push(e);
            }
        }
        self.record.state = State::RolledBack;
        problems
    }
}

/// Restores one file from its backup copy (or removes it if it was new).
fn put_back(sys: &Sys, dir: &Path, f: &FileBackup) -> Result<(), String> {
    let real = sys.path(&f.path);
    match &f.copy {
        Some(copy) => {
            let bytes = std::fs::read(dir.join(copy)).map_err(|e| format!("{}: {e}", f.path.display()))?;
            write_file(sys, &f.path, &bytes, 0o644)
        }
        None => match std::fs::remove_file(&real) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", f.path.display())),
        },
    }
}

/// Runs one regeneration step (also used by restore).
fn run_step(sys: &Sys, runner: &dyn Runner, step: &Step) -> Result<(), String> {
    let s = |p: &Path| p.display().to_string();
    match step {
        Step::GrubPreflight { mkconfig, check, cfg } | Step::GrubConfig { mkconfig, check, cfg } => {
            let tmp = PathBuf::from(format!("{}.{GRUB_NEW_SUFFIX}", cfg.display()));
            let result = (|| {
                runner.run(&[s(mkconfig), "-o".into(), s(&tmp)])?;
                if let Some(check) = check {
                    runner.run(&[s(check), s(&tmp)])?;
                }
                let text = std::fs::read_to_string(sys.path(&tmp)).map_err(|e| format!("{}: {e}", tmp.display()))?;
                if !text.contains("menuentry") && !text.contains("blscfg") {
                    return Err(format!("{} has no boot entries, so it wasn't used", tmp.display()));
                }
                if matches!(step, Step::GrubConfig { .. }) {
                    std::fs::rename(sys.path(&tmp), sys.path(cfg)).map_err(|e| format!("{}: {e}", cfg.display()))?;
                }
                Ok(())
            })();
            let _ = std::fs::remove_file(sys.path(&tmp));
            result
        }
        Step::LimineEnroll { tool } => runner.run(&[s(tool)]).map(|_| ()),
        Step::Initramfs { argv } => runner.run(argv).map(|_| ()),
        Step::PlymouthDefault { tool, name } => {
            runner.run(&[s(tool), name.clone()])?;
            let now = runner.run(&[s(tool)])?;
            if now.trim() != name {
                return Err(format!("plymouth-set-default-theme reports {:?}, not {name}", now.trim()));
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// Copies the staged theme (or wallpaper) into root's own folder and checks
/// it there. Returns its path.
fn take_in(req: &Request, scratch: &Path) -> Result<Option<PathBuf>, String> {
    if let Some((staged, kind)) = req.staged() {
        let to = scratch.join(&staged.name);
        archive::copy_tree(&staged.dir, &to, archive::INSTALL_LIMITS)?;
        match kind {
            Kind::Grub => theme::check_grub(&to).map(|_| ())?,
            Kind::Refind => theme::check_refind(&to, &staged.name, staged.include.as_deref().unwrap_or_default())?,
            Kind::Plymouth => theme::check_plymouth(&to, &staged.name, None).map(|_| ())?,
        }
        return Ok(Some(to));
    }
    if let Request::LimineStyle { style } = req
        && let Some(w) = &style.wallpaper
    {
        let meta = std::fs::symlink_metadata(w).map_err(|e| format!("{}: {e}", w.display()))?;
        if !meta.is_file() || meta.len() > WALLPAPER_MAX {
            return Err(format!("{} isn't a wallpaper Mandara copies", w.display()));
        }
        let bytes = std::fs::read(w).map_err(|e| e.to_string())?;
        if !bytes.starts_with(PNG_MAGIC) {
            return Err("the wallpaper must be a PNG".into());
        }
        let to = scratch.join("wallpaper.png");
        std::fs::write(&to, bytes).map_err(|e| e.to_string())?;
        return Ok(Some(to));
    }
    Ok(None)
}

/// The result printed for the caller.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Outcome {
    pub ok: bool,
    pub backup: Option<String>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan: Option<Plan>,
}

/// Applies `approved` on `sys` (root's view of the system).
pub fn apply(sys: &Sys, det: &Detection, approved: &Approved, runner: &dyn Runner) -> Result<Outcome, (i32, Outcome)> {
    let fail = |code: i32, message: String| (code, Outcome { ok: false, backup: None, message, plan: None });
    let req = &approved.request;
    req.validate().map_err(|e| fail(EXIT_USAGE, e))?;
    let incoming = sys.path(INCOMING_DIR);
    let _ = std::fs::create_dir_all(&incoming);
    let scratch = incoming.join(format!("{}-{}", std::process::id(), now()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(&scratch).map_err(|e| fail(EXIT_FAILED, format!("{}: {e}", scratch.display())))?;
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(scratch.clone());
    let taken = take_in(req, &scratch).map_err(|e| fail(EXIT_FAILED, e))?;
    let plan = plan::build(det, req, taken.as_deref()).map_err(|e| fail(EXIT_UNSUPPORTED, e))?;
    if plan.digest() != approved.digest {
        return Err((EXIT_PLAN_CHANGED, Outcome { ok: false, backup: None, message: "With admin rights the system looks different from what you approved, so nothing was changed. Review the new plan.".into(), plan: Some(plan) }));
    }
    // Theme folders Mandara may replace: its own, or new ones.
    let mut installed = Installed::load(sys);
    for step in &plan.steps {
        if let Step::InstallTheme { to, .. } = step
            && sys.path(to).exists()
            && !installed.dirs.contains(to)
        {
            return Err(fail(EXIT_FAILED, format!("{} exists and wasn't installed by Mandara, so it was left as it is", to.display())));
        }
    }
    let created = now();
    let mut id = format!("{}-{}", stamp(created), req.slug());
    let backups = sys.path(BACKUP_DIR);
    let mut n = 1;
    while backups.join(&id).exists() {
        n += 1;
        id = format!("{}-{}-{n}", stamp(created), req.slug());
    }
    let dir = backups.join(&id);
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::create_dir_all(&dir).map_err(|e| fail(EXIT_FAILED, format!("{}: {e}", dir.display())))?;
        let _ = std::fs::set_permissions(sys.path(super::STATE_DIR), std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::set_permissions(&backups, std::fs::Permissions::from_mode(0o755));
        let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755));
    }
    let what = match req {
        Request::MenuTheme { theme, .. } => theme.name.clone(),
        Request::LimineStyle { .. } => "Limine look".into(),
        Request::Splash { theme: plan::SplashChoice::Installed { name } } => name.clone(),
        Request::Splash { theme: plan::SplashChoice::Staged(s) } => s.name.clone(),
    };
    let mut j = Journal { sys, dir: dir.clone(), record: Record { id: id.clone(), created, kind: req.slug().into(), what, state: State::Applied, files: Vec::new(), dirs: Vec::new(), regenerate: Vec::new() }, ran: Vec::new() };
    let total = plan.steps.len();
    for (i, step) in plan.steps.iter().enumerate() {
        eprintln!("step {}/{total}: {}", i + 1, step.detail());
        if let Err(e) = exec(&mut j, step, taken.as_deref(), runner) {
            // A rebuild that failed half way is rebuilt again from the
            // restored configs.
            if matches!(step, Step::Initramfs { .. } | Step::LimineEnroll { .. }) {
                j.ran.push(step.clone());
            }
            let problems = j.rollback(runner);
            let _ = write_file(sys, &sys.system_path(&dir.join("record.json")), serde_json::to_string_pretty(&j.record).unwrap_or_default().as_bytes(), 0o644);
            let mut message = format!("{e}\nEverything was put back as it was.");
            if !problems.is_empty() {
                message = format!("{e}\nPutting things back also had problems:\n{}", problems.join("\n"));
            }
            return Err(fail(EXIT_FAILED, message));
        }
        j.ran.push(step.clone());
    }
    for f in &mut j.record.files {
        f.after = super::sha256_file(&sys.path(&f.path));
    }
    j.record.regenerate = plan.steps.iter().filter(|s| matches!(s, Step::GrubConfig { .. } | Step::Initramfs { .. } | Step::LimineEnroll { .. })).cloned().collect();
    for d in &j.record.dirs {
        if !installed.dirs.contains(&d.path) {
            installed.dirs.push(d.path.clone());
        }
    }
    let saved = installed.save(sys).and_then(|()| write_file(sys, &sys.system_path(&dir.join("record.json")), serde_json::to_string_pretty(&j.record).unwrap_or_default().as_bytes(), 0o644));
    if let Err(e) = saved {
        return Err(fail(EXIT_FAILED, format!("Applied, but the backup record couldn't be saved: {e}")));
    }
    Ok(Outcome { ok: true, backup: Some(id), message: "Applied. The previous setup is backed up; Restore puts it back.".into(), plan: Some(plan) })
}

/// Runs one step of an apply.
fn exec(j: &mut Journal<'_>, step: &Step, taken: Option<&Path>, runner: &dyn Runner) -> Result<(), String> {
    let sys = j.sys;
    match step {
        Step::GrubPreflight { .. } => run_step(sys, runner, step),
        Step::Backup { path } => j.backup(path),
        Step::InstallTheme { to, .. } => {
            let from = taken.ok_or("the staged theme went missing")?;
            let real = sys.path(to);
            let mut previous = None;
            if real.exists() {
                let name = format!("dir-{}", j.record.dirs.len());
                install_dir(&real, &j.dir.join(&name))?;
                std::fs::remove_dir_all(&real).map_err(|e| format!("{}: {e}", to.display()))?;
                previous = Some(name);
            }
            j.record.dirs.push(DirChange { path: to.clone(), previous });
            install_dir(from, &real)
        }
        Step::MakeFont { tool, size, font, out } => runner.run(&[tool.display().to_string(), "-s".into(), size.to_string(), "-o".into(), out.display().to_string(), font.display().to_string()]).map(|_| ()),
        Step::GrubKeys { file, keys } => {
            let (text, previous) = edit::grub_set(&read_text(sys, file)?, keys)?;
            j.set_edit(file, Edit::GrubKeys { previous });
            write_file(sys, file, text.as_bytes(), 0o644)
        }
        Step::GrubDropIn { file, keys } => {
            j.set_edit(file, Edit::Owned);
            write_file(sys, file, edit::grub_dropin(keys)?.as_bytes(), 0o644)
        }
        Step::GrubConfig { .. } => {
            if let Step::GrubConfig { cfg, .. } = step {
                j.set_edit(cfg, Edit::Generated);
            }
            run_step(sys, runner, step)
        }
        Step::LimineOptions { file, options } => {
            let before = read_text(sys, file)?;
            let entries = |t: &str| t.lines().filter(|l| l.trim_start().starts_with('/')).count();
            let text = edit::limine_set(&before, options)?;
            // Self-check: the entries are exactly as they were.
            if entries(&text) != entries(&before) || edit::limine_restore(&text) != edit::limine_restore(&before) {
                return Err("the Limine edit would change more than the look options".into());
            }
            j.set_edit(file, Edit::LimineBlock);
            write_file(sys, file, text.as_bytes(), 0o644)
        }
        Step::CopyFile { to, .. } => {
            let from = taken.ok_or("the staged wallpaper went missing")?;
            j.set_edit(to, Edit::Owned);
            let bytes = std::fs::read(from).map_err(|e| e.to_string())?;
            write_file(sys, to, &bytes, 0o644)
        }
        Step::LimineEnroll { .. } | Step::Initramfs { .. } => run_step(sys, runner, step),
        Step::RefindInclude { file, include } => {
            let text = edit::refind_set(&read_text(sys, file)?, include)?;
            j.set_edit(file, Edit::RefindBlock);
            write_file(sys, file, text.as_bytes(), 0o644)
        }
        Step::PlymouthDefault { tool, .. } => {
            let previous = runner.run(&[tool.display().to_string()]).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
            j.set_edit(Path::new(plan::PLYMOUTHD_CONF), Edit::PlymouthConf { previous });
            run_step(sys, runner, step)
        }
    }
}

/// Undoes the apply `id` (`latest`: the newest one still applied).
pub fn restore(sys: &Sys, id: &str, runner: &dyn Runner) -> Result<Outcome, (i32, Outcome)> {
    let fail = |code: i32, message: String| (code, Outcome { ok: false, backup: None, message, plan: None });
    let mut record = if id == "latest" { latest(sys).ok_or_else(|| fail(EXIT_FAILED, "There is nothing to restore".into()))? } else { records(sys).into_iter().find(|r| r.id == id).ok_or_else(|| fail(EXIT_FAILED, format!("No backup {id}")))? };
    if record.state != State::Applied {
        return Err(fail(EXIT_FAILED, format!("{} was already {}", record.id, if record.state == State::Restored { "restored" } else { "rolled back" })));
    }
    let dir = sys.path(BACKUP_DIR).join(&record.id);
    let mut notes = Vec::new();
    let mut plymouth_previous = None;
    for f in record.files.iter().rev() {
        let current = super::sha256_file(&sys.path(&f.path));
        let unchanged = current == f.after;
        let result = match (&f.edit, unchanged) {
            (Edit::Generated, _) => Ok(()),
            (Edit::PlymouthConf { previous }, _) => {
                plymouth_previous = previous.clone();
                if unchanged { put_back(sys, &dir, f) } else { Ok(()) }
            }
            (_, true) | (Edit::Whole, false) => put_back(sys, &dir, f),
            (Edit::Owned, false) => put_back(sys, &dir, f),
            (Edit::GrubKeys { previous }, false) => read_text(sys, &f.path).and_then(|t| write_file(sys, &f.path, edit::grub_restore(&t, previous).as_bytes(), 0o644)),
            (Edit::LimineBlock, false) => read_text(sys, &f.path).and_then(|t| write_file(sys, &f.path, edit::limine_restore(&t).as_bytes(), 0o644)),
            (Edit::RefindBlock, false) => read_text(sys, &f.path).and_then(|t| write_file(sys, &f.path, edit::refind_restore(&t).as_bytes(), 0o644)),
        };
        if !unchanged && !matches!(f.edit, Edit::Generated | Edit::Whole | Edit::Owned) {
            notes.push(format!("{} changed since; only Mandara's lines were reverted", f.path.display()));
        }
        result.map_err(|e| fail(EXIT_FAILED, e))?;
    }
    // The splash theme from before (when plymouthd.conf was edited since).
    for step in &record.regenerate {
        if let (Step::Initramfs { .. }, Some(prev)) = (step, &plymouth_previous)
            && let Some(p) = detect::detect(sys).plymouth
        {
            let now = runner.run(&[p.set_default.display().to_string()]).unwrap_or_default();
            if now.trim() != prev {
                runner.run(&[p.set_default.display().to_string(), prev.clone()]).map_err(|e| fail(EXIT_FAILED, e))?;
            }
        }
    }
    let mut installed = Installed::load(sys);
    for d in record.dirs.iter().rev() {
        let real = sys.path(&d.path);
        let _ = std::fs::remove_dir_all(&real);
        installed.dirs.retain(|p| *p != d.path);
        if let Some(prev) = &d.previous {
            install_dir(&dir.join(prev), &real).map_err(|e| fail(EXIT_FAILED, e))?;
            installed.dirs.push(d.path.clone());
        }
    }
    let _ = installed.save(sys);
    for step in &record.regenerate {
        run_step(sys, runner, step).map_err(|e| fail(EXIT_FAILED, format!("The files are back, but rebuilding failed: {e}")))?;
    }
    record.state = State::Restored;
    let _ = write_file(sys, &sys.system_path(&dir.join("record.json")), serde_json::to_string_pretty(&record).unwrap_or_default().as_bytes(), 0o644);
    let mut message = format!("Restored the setup from before {}.", record.what);
    for n in notes {
        message.push('\n');
        message.push_str(&n);
    }
    Ok(Outcome { ok: true, backup: Some(record.id), message, plan: None })
}

fn print(outcome: &Outcome) {
    println!("{}", serde_json::to_string_pretty(outcome).unwrap_or_default());
}

/// `mandara boot-admin …`.
pub fn main(args: &[String]) -> i32 {
    let action = match Action::parse(args) {
        Ok(a) => a,
        Err(usage) => {
            eprintln!("{usage}");
            return EXIT_USAGE;
        }
    };
    if cfg!(target_os = "freebsd") {
        print(&Outcome { ok: false, backup: None, message: "Boot look isn't supported on FreeBSD".into(), plan: None });
        return EXIT_UNSUPPORTED;
    }
    // SAFETY: geteuid has no preconditions.
    if unsafe { libc::geteuid() } != 0 {
        print(&Outcome { ok: false, backup: None, message: "not running as root (Studio and `mandara msg boot apply` ask for approval); nothing was changed".into(), plan: None });
        return EXIT_NOT_ROOT;
    }
    let sys = Sys::real();
    let result = match action {
        Action::Apply(file) => {
            let approved = std::fs::metadata(&file).map_err(|e| e.to_string()).and_then(|m| if m.len() > REQUEST_MAX { Err("the request is too large".into()) } else { Ok(()) }).and_then(|()| std::fs::read_to_string(&file).map_err(|e| e.to_string())).and_then(|t| serde_json::from_str::<Approved>(&t).map_err(|e| e.to_string()));
            match approved {
                Ok(a) => apply(&sys, &detect::detect(&sys), &a, &SystemRunner),
                Err(e) => Err((EXIT_USAGE, Outcome { ok: false, backup: None, message: format!("{}: {e}", file.display()), plan: None })),
            }
        }
        Action::Restore(id) => restore(&sys, &id, &SystemRunner),
    };
    match result {
        Ok(o) => {
            print(&o);
            EXIT_OK
        }
        Err((code, o)) => {
            print(&o);
            code
        }
    }
}

#[cfg(test)]
mod tests;
