//! The install and uninstall flows.
//!
//! The installer runs as the user and asks for root (sudo, doas, run0 or
//! pkexec) only for package installs and system changes. With `--dry-run`
//! every step prints what it would do and nothing changes: no packages, no
//! files, no services (read-only checks and downloads into a private temp
//! folder still run, so the plan is the real one).

use std::path::{Path, PathBuf};

use mandara_delta::signing::{self, PublicKey, SIG_FILE};
use mandara_delta::{MANIFEST_FILE, SHELL_BINARY};
use mandara_i18n::{Lang, packs};

use crate::args::InstallArgs;
use crate::compositors::{self, Compositor};
use crate::family::{Family, OsRelease};
use crate::files::{self, Paths};
use crate::host::{Cmd, Fetch, Host, Io, Level, Row, TempDir};
use crate::pkg::{PackageManager, Privilege};
use crate::{index, packages};
use crate::platform::{self, MIN_RUST, RELEASED_ARCHES};
use crate::release::{self, ARCHIVE_SUFFIX, MAX_ARCHIVE_BYTES, MAX_JSON_BYTES, SUMS_FILE, SelfCheck};
use crate::tui;

/// greetd + its kiosk compositor: what Mandara Login needs installed.
const LOGIN_PROGRAMS: &[&str] = &["greetd", "cage"];
/// The lock screen's own PAM service (else `login` is used).
const LOCK_PAM: &str = "/etc/pam.d/mandara-lock";
/// Studio's admin helper: the root-owned copy of Mandara its administrator
/// actions run as root (`mandara_config::admin::ADMIN_EXE`).
pub const ADMIN_EXE: &str = "/usr/local/lib/mandara/mandara-admin";
/// Loads i2c-dev at boot (DDC/CI monitor brightness).
const I2C_MODULES_LOAD: &str = "/etc/modules-load.d/i2c-dev.conf";
const I2C_MODULE: &str = "i2c-dev";
/// The group owning `/dev/i2c-*` when none exists yet.
const I2C_DEFAULT_GROUP: &str = "i2c";
/// Where rustup's installer binary is published (`<dist>/<triple>/rustup-init`).
const RUSTUP_DIST: &str = "https://static.rust-lang.org/rustup/dist";
const RUSTUP_INIT: &str = "rustup-init";
/// `.sha256` next to a rustup download.
const SHA256_SUFFIX: &str = ".sha256";
/// Largest rustup-init download.
const MAX_RUSTUP_BYTES: u64 = 64 << 20;
/// Exit text marking a deleted executable in `/proc/<pid>/exe`.
const DELETED_SUFFIX: &str = " (deleted)";
/// Lines of `mandara setup --fix-conflicts` worth showing.
const FIX_CONFLICT_WORDS: &[&str] = &["retired", "rejected", "restored"];
/// Mode of a staged binary.
const MODE_EXECUTABLE: u32 = 0o755;
/// What `mandara_version::source_fingerprint` reports for builds not made
/// by `cargo xtask build`.
const DEV_FINGERPRINT: &str = "dev";
/// `mandara setup --check` exit code for errors the compositor reports in
/// its config (10 means conflicts).
pub const SETUP_CONFIG_ERRORS: i32 = 11;
/// The quiet log level for the setup commands the installer runs.
const SETUP_LOG: (&str, &str) = ("RUST_LOG", "warn");

/// Mandara Login's state, asked in-process (the greeter crate implements it).
pub trait LoginProbe {
    /// Mandara Login is the login screen (greetd runs it).
    fn active(&self) -> bool;
    /// It is active and its installed copy differs from `exe` (an upgrade
    /// refreshes it).
    fn outdated(&self, exe: &Path) -> bool;
    /// What `greeter use mandara` would change (the dry-run plan);
    /// `after_packages`: greetd and cage get installed first, so their
    /// absence isn't an error.
    fn switch_plan(&self, exe: &Path, after_packages: bool) -> String;
    /// What `greeter keyring-unlock --apply` would change, when anything.
    fn keyring_plan(&self) -> Option<String>;
}

/// What was detected about the system.
#[derive(Clone, Debug)]
pub struct Facts {
    pub os: OsRelease,
    pub family: Family,
    pub arch: String,
    pub libc: &'static str,
    /// The CPU runs the x86-64-v3 tuned build.
    pub x86_64_v3: bool,
    /// The running binary.
    pub exe: PathBuf,
    /// The running binary is a debug build: never installed for real.
    pub debug_build: bool,
    /// Built by `cargo xtask build` (as every release is); other builds
    /// can't match a published checksum.
    pub fingerprinted: bool,
    /// The source checkout the running binary was built in.
    pub exe_checkout: Option<PathBuf>,
    pub cwd: PathBuf,
    pub home: PathBuf,
    pub user: String,
    /// `$PATH`, for the "not on your PATH" hint and source builds.
    pub path_var: String,
    pub is_root: bool,
    /// Where system files are read from (`/`; fixture trees in tests).
    pub root: PathBuf,
    pub repo: String,
    /// `$MANDARA_RELEASE_BASE`.
    pub mirror: Option<String>,
    /// `$XDG_DATA_HOME` / `$XDG_CONFIG_HOME`.
    pub xdg_data: Option<String>,
    pub xdg_config: Option<String>,
    /// The release keys signatures are checked against (the shipped ones;
    /// a test key in tests).
    pub release_keys: Vec<PublicKey>,
}

impl Facts {
    /// Detects the real system.
    pub fn detect(host: &dyn Host, repo: Option<&str>) -> Facts {
        let root = PathBuf::from("/");
        let os_text = ["etc/os-release", "usr/lib/os-release"].iter().find_map(|p| std::fs::read_to_string(root.join(p)).ok()).unwrap_or_default();
        let os = OsRelease::parse(&os_text);
        let family = Family::detect(&os, |p| host.has_program(p));
        let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
        let exe = std::env::current_exe().map(|p| PathBuf::from(p.to_string_lossy().trim_end_matches(DELETED_SUFFIX))).unwrap_or_else(|_| PathBuf::from(files::BINARY));
        let exe = std::fs::canonicalize(&exe).unwrap_or(exe);
        let arch = std::env::consts::ARCH.to_string();
        let x86_64_v3 = arch == "x86_64" && platform::cpu_supports_x86_64_v3(&std::fs::read_to_string(root.join("proc/cpuinfo")).unwrap_or_default());
        Facts {
            os,
            family,
            arch,
            libc: platform::compiled_libc(),
            x86_64_v3,
            exe_checkout: files::checkout_of(&exe),
            exe,
            debug_build: cfg!(debug_assertions),
            fingerprinted: mandara_version::source_fingerprint() != DEV_FINGERPRINT,
            cwd: std::env::current_dir().unwrap_or_else(|_| root.clone()),
            home: env("HOME").map(PathBuf::from).unwrap_or_else(|| root.clone()),
            user: env("USER").or_else(|| env("LOGNAME")).unwrap_or_default(),
            path_var: env("PATH").unwrap_or_default(),
            is_root: crate::pkg::is_root(),
            root,
            repo: repo.map(str::to_string).or_else(|| env(release::REPO_ENV)).unwrap_or_else(|| release::DEFAULT_REPO.to_string()),
            mirror: env(release::RELEASE_BASE_ENV),
            xdg_data: env("XDG_DATA_HOME"),
            xdg_config: env("XDG_CONFIG_HOME"),
            release_keys: signing::trusted_keys(),
        }
    }

    /// A system path inside [`Facts::root`].
    fn sys(&self, abs: &str) -> PathBuf {
        self.root.join(abs.trim_start_matches('/'))
    }
}

/// Where the installed binary comes from.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Source {
    /// The running binary (a release tarball, `cargo install`, a build).
    This,
    /// A downloaded release (`None`: the latest).
    Release(Option<String>),
    /// Built from source.
    Build,
}

/// What gets installed.
struct Stage {
    exe: PathBuf,
    desktop: String,
    icon: Vec<u8>,
    /// The binary's version and the language packs that came with it
    /// (`share/mandara/locales/<version>/` of a tarball), copied next to it
    /// so a fresh install never has to download them.
    packs: (String, Vec<(Lang, Vec<u8>)>),
}

impl Stage {
    /// The running binary, with the packs beside it when it's an unpacked
    /// tarball (`<dir>/bin/mandara` and `<dir>/share/mandara/locales/<v>/`).
    fn embedded(exe: PathBuf) -> Stage {
        let version = mandara_version::version().to_string();
        let packs = exe.parent().and_then(Path::parent).map(|top| top.join("share").join(packs::DIR).join(&version)).map(|dir| Lang::ALL.into_iter().filter_map(|l| std::fs::read(dir.join(packs::file_name(l))).ok().map(|b| (l, b))).collect()).unwrap_or_default();
        Stage { exe, desktop: files::DESKTOP_ENTRY.to_string(), icon: files::ICON_SVG.to_vec(), packs: (version, packs) }
    }
}

/// The packs inside a release tarball for `version`.
fn archive_packs(archive: &[u8], version: &str) -> Result<Vec<(Lang, Vec<u8>)>, String> {
    let mut out = Vec::new();
    for lang in Lang::ALL.into_iter().filter(|l| *l != Lang::En) {
        if let Some(bytes) = release::extract(archive, &format!("share/{}/{version}/{}", packs::DIR, packs::file_name(lang)))? {
            out.push((lang, bytes));
        }
    }
    Ok(out)
}

/// One run of `mandara install` / `uninstall`.
pub struct Installer<'a> {
    host: &'a mut dyn Host,
    probe: &'a dyn LoginProbe,
    facts: Facts,
    args: InstallArgs,
    paths: Paths,
    privilege: Privilege,
    tmp: Option<TempDir>,
    /// The package databases were brought up to date in this run (or, in a
    /// dry run, would have been).
    synced: bool,
    /// What the interactive setup screen chose (`None` when it didn't run).
    setup: Option<SetupChoices>,
}

/// The answers taken on the interactive setup screen, so the steps below
/// don't ask for them again.
#[derive(Copy, Clone, Debug)]
struct SetupChoices {
    autostart: bool,
    greeter: bool,
}

/// What a row of the setup screen does.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Choice {
    Compositor,
    Autostart,
    Greeter,
    Install,
}

impl<'a> Installer<'a> {
    pub fn new(host: &'a mut dyn Host, probe: &'a dyn LoginProbe, facts: Facts, args: InstallArgs) -> Installer<'a> {
        let xdg = |k: &str| match k {
            "XDG_DATA_HOME" => facts.xdg_data.clone(),
            "XDG_CONFIG_HOME" => facts.xdg_config.clone(),
            _ => None,
        };
        let paths = Paths::new(args.prefix.as_deref(), &facts.home, xdg);
        let privilege = Privilege::detect(facts.is_root, |p| host.has_program(p));
        Installer { host, probe, facts, args, paths, privilege, tmp: None, synced: false, setup: None }
    }

    pub fn paths(&self) -> &Paths {
        &self.paths
    }

    // ── Output and helpers ──────────────────────────────────────────────

    fn say(&mut self, level: Level, text: impl AsRef<str>) {
        self.host.say(level, text.as_ref());
    }

    fn dry(&self) -> bool {
        self.args.dry_run
    }

    /// Asks a question: `--yes` takes the default, a dry run shows it and
    /// assumes the default.
    fn ask(&mut self, question: &str, default: bool) -> bool {
        if self.dry() {
            let (hint, answer) = if default { ("[Y/n]", "yes") } else { ("[y/N]", "no") };
            self.say(Level::Detail, format!("would ask: {question} {hint} (assuming {answer})"));
            return default;
        }
        if self.args.yes {
            return default;
        }
        self.host.ask(question, default)
    }

    /// Runs `cmd` as root (shown instead in a dry run). False on failure.
    fn as_root(&mut self, cmd: Cmd) -> bool {
        let shown = self.privilege.display(&cmd.argv);
        if self.dry() {
            self.say(Level::Detail, format!("would run: {shown}"));
            return true;
        }
        let Some(argv) = self.privilege.wrap(cmd.argv.clone()) else {
            self.say(Level::Warn, format!("no sudo/doas/run0/pkexec: run as root: {}", cmd.display()));
            return false;
        };
        let cmd = Cmd { argv, ..cmd };
        match self.host.run(&cmd) {
            Ok(o) => o.success,
            Err(e) => {
                self.say(Level::Warn, format!("{}: {e}", cmd.display()));
                false
            }
        }
    }

    /// The root command line as shown in hints.
    fn root_hint<S: AsRef<str>>(&self, argv: &[S]) -> String {
        let argv: Vec<String> = argv.iter().map(|a| a.as_ref().to_string()).collect();
        match self.privilege {
            Privilege::Root | Privilege::Unavailable => format!("sudo {}", argv.join(" ")),
            p => p.display(&argv),
        }
    }

    /// Runs a user command; `None` when it couldn't start.
    fn run(&mut self, cmd: Cmd) -> Option<crate::host::Output> {
        self.host.run(&cmd).ok()
    }

    fn has(&self, program: &str) -> bool {
        self.host.has_program(program)
    }

    fn tmp(&mut self) -> Result<PathBuf, String> {
        if self.tmp.is_none() {
            self.tmp = Some(TempDir::new().map_err(|e| format!("creating a temporary folder: {e}"))?);
        }
        Ok(self.tmp.as_ref().map(|t| t.path().to_path_buf()).unwrap_or_default())
    }

    fn package_manager(&self) -> Option<PackageManager> {
        PackageManager::detect(self.facts.family, |p| self.host.has_program(p)).or_else(|| PackageManager::for_family(self.facts.family))
    }

    fn source(&mut self) -> Source {
        let source = if self.args.from_source {
            Source::Build
        } else if self.args.latest || self.args.wanted_version().is_some() {
            Source::Release(self.args.wanted_version())
        } else {
            Source::This
        };
        if matches!(source, Source::Release(_)) && !RELEASED_ARCHES.contains(&self.facts.arch.as_str()) {
            let arch = self.facts.arch.clone();
            self.say(Level::Warn, format!("no prebuilt binary for {arch} — building from source"));
            return Source::Build;
        }
        source
    }

    // ── Install ─────────────────────────────────────────────────────────

    /// `mandara install`.
    pub fn install(&mut self) -> Result<(), String> {
        if self.facts.is_root {
            return Err("run this as your user, not root (it asks for sudo when it needs to)".into());
        }
        if self.args.uninstall {
            return self.uninstall();
        }
        let source = self.source();
        let label = match &source {
            Source::This => "this binary",
            Source::Release(_) => "release",
            Source::Build => "from source",
        };
        let banner = format!("Mandara Shell installer · {} ({}) · {} · {label}", self.facts.os.display_name(), self.facts.family, self.facts.arch);
        if self.host.interactive() && !self.args.yes {
            let columns = self.host.columns();
            self.say(Level::Title, tui::banner(columns, &banner));
        } else {
            self.say(Level::Title, banner);
        }
        if self.dry() {
            self.say(Level::Info, "dry run: nothing will be changed");
        }
        if source == Source::This && self.facts.debug_build {
            let msg = format!(
                "{} is a debug build; install a release build instead: `cargo xtask build` then `target/release/mandara install`, or `mandara install --from-source`",
                self.facts.exe.display()
            );
            if !self.dry() {
                return Err(msg);
            }
            self.say(Level::Warn, msg);
        }
        if !self.preflight()? {
            self.say(Level::Info, "cancelled — nothing was changed");
            return Ok(());
        }
        let deps = !self.args.no_deps;
        if deps {
            self.deps(source == Source::Build)?;
        }
        self.offer_compositor();
        let stage = match source {
            Source::This => Some(self.verify_self()?),
            Source::Build => self.build_source()?,
            Source::Release(version) => match self.fetch_release(version)? {
                Some(stage) => Some(stage),
                None if self.dry() => {
                    self.say(Level::Warn, "no installable release — a real run would build from source");
                    None
                }
                None => {
                    self.say(Level::Warn, "falling back to a source build");
                    if deps {
                        self.deps(true)?;
                    }
                    self.build_source()?
                }
            },
        };
        if self.dry() {
            let exe = stage.map(|s| s.exe).unwrap_or_else(|| self.facts.exe.clone());
            return self.dry_run_summary(&exe);
        }
        let stage = stage.ok_or("nothing to install")?;
        self.install_files(&stage)?;
        self.compositor();
        let bin = self.paths.binary();
        self.admin_helper(&bin);
        self.greeter(&bin);
        self.lock_hint();
        self.i2c();
        self.restart();
        self.say(Level::Done, "Mandara is installed.");
        for line in ["Start it:        mandara            (or log in again if autostart is on)", "Settings:        mandara studio", "Health & repair: mandara studio maintenance", "Upgrade:         mandara update"] {
            self.say(Level::Info, line);
        }
        Ok(())
    }

    // ── System packages ─────────────────────────────────────────────────

    /// Runtime libraries (required), build tools (source builds, required),
    /// then fonts, portals and services one by one (optional).
    fn deps(&mut self, from_source: bool) -> Result<(), String> {
        let family = self.facts.family;
        let Some(set) = packages::for_family(family) else {
            if family == Family::NixOS {
                self.say(Level::Warn, "NixOS: install Mandara with its flake and NixOS module instead (Nix then keeps it updated):");
                for line in packages::NIXOS_HINT {
                    self.say(Level::Info, format!("  {line}"));
                }
            } else {
                for line in packages::UNKNOWN_HINT {
                    self.say(Level::Warn, *line);
                }
            }
            return Ok(());
        };
        let Some(pm) = self.package_manager() else { return Ok(()) };
        self.say(Level::Step, "Installing system packages");
        self.prepare_index(pm)?;
        let mut required: Vec<&str> = set.run.to_vec();
        if from_source {
            required.extend(set.build);
        }
        if !self.as_root(Cmd::from_argv(pm.install_argv(&required))) {
            let failed = || format!("could not install required packages: {}", required.join(" "));
            // Databases that were never synced or are out of date make every
            // install fail: bring them up to date once and try again.
            if self.synced || self.privilege == Privilege::Unavailable || index::sync_argv(pm, true).is_none() {
                return Err(failed());
            }
            self.say(Level::Warn, "installing the required packages failed");
            let missing = index::missing(pm, &self.facts.root);
            self.sync_index(pm, &missing, true)?;
            if !self.as_root(Cmd::from_argv(pm.install_argv(&required))) {
                return Err(failed());
            }
        }
        if !self.dry() {
            self.say(Level::Ok, if from_source { "runtime and build dependencies" } else { "runtime libraries" });
        }
        let portals = packages::portal_packages(family, |p| self.host.has_program(p));
        for p in set.opt.iter().chain(set.svc).chain(portals.iter()) {
            let installed = self.as_root(Cmd::from_argv(pm.install_argv(&[p])).io(Io::Quiet));
            if self.dry() {
                continue;
            }
            if installed {
                self.say(Level::Ok, *p);
            } else {
                self.say(Level::Detail, format!("skipped {p} (not available)"));
            }
        }
        self.services_hint();
        Ok(())
    }

    /// Makes sure the package databases can be used before installing: apt's
    /// lists are refreshed every time (cheap, harmless); other managers'
    /// databases only when they are missing ([`index::missing`]).
    fn prepare_index(&mut self, pm: PackageManager) -> Result<(), String> {
        let missing = index::missing(pm, &self.facts.root);
        if index::sync_before_install(pm) {
            let Some(argv) = index::sync_argv(pm, true) else { return Ok(()) };
            let shown = self.root_hint(&argv);
            let io = if missing.is_empty() { Io::Quiet } else { Io::Inherit };
            if !missing.is_empty() {
                self.say(Level::Info, format!("no package lists yet: downloading them ({shown})"));
            }
            if self.as_root(Cmd::from_argv(argv).io(io)) {
                self.synced = true;
            } else if !missing.is_empty() {
                return Err(format!("`{shown}` failed, and without package lists nothing can be installed: check your network and your package sources, then run `mandara install` again (or pass --no-deps to skip system packages)"));
            }
            // Lists that exist but couldn't be refreshed still work.
            return Ok(());
        }
        if missing.is_empty() {
            return Ok(());
        }
        self.sync_index(pm, &missing, false)
    }

    /// Brings the package databases up to date: asked first when that
    /// upgrades the whole system (pacman's `-Syu`), announced otherwise.
    /// `missing`: the databases found missing (empty: they may only be
    /// stale); `after_failure`: an install already failed.
    fn sync_index(&mut self, pm: PackageManager, missing: &[String], after_failure: bool) -> Result<(), String> {
        let unattended = self.args.yes || !self.host.interactive();
        let Some(argv) = index::sync_argv(pm, unattended) else { return Ok(()) };
        let shown = self.root_hint(&argv);
        let what = if missing.is_empty() { "Your package databases may be out of date".to_string() } else { format!("Your package databases are missing or out of date ({})", missing.join(", ")) };
        if index::sync_upgrades_system(pm) {
            let again = if after_failure { " and try again" } else { "" };
            if unattended && !self.dry() {
                // Nobody is asked: say what the default answer does.
                let why = if self.args.yes { "--yes" } else { "no terminal to ask on" };
                self.say(Level::Info, format!("{what}: updating them with `{shown}`, a full system upgrade ({why}: the default answer)"));
            }
            if !self.ask(&format!("{what}. Update them now{again}? This runs `{shown}`, a full system upgrade."), true) {
                return Err(format!("the package databases need updating before anything can be installed: run `{shown}`, then `mandara install` again (or pass --no-deps to skip system packages)"));
            }
        } else {
            self.say(Level::Info, format!("{what}: updating them ({shown})"));
        }
        if !self.as_root(Cmd::from_argv(argv)) {
            return Err(format!("`{shown}` failed: fix what it reports, then run `mandara install` again"));
        }
        self.synced = true;
        if !self.dry() {
            self.say(Level::Ok, "package databases up to date");
        }
        Ok(())
    }

    // ── A compositor ────────────────────────────────────────────────────

    /// Installs a compositor from the distribution's own repositories: the
    /// one `--compositor` names when it is missing (after asking; `--yes`
    /// counts, since it was named), or, when no supported compositor is
    /// installed, the one picked from a numbered list (only when someone
    /// answers: never unattended without `--compositor`). `mandara setup`
    /// then hooks Mandara into it.
    fn offer_compositor(&mut self) {
        let distro = self.facts.os.display_name().to_string();
        if let Some(name) = self.args.compositor.clone() {
            let Some(c) = compositors::by_name(&name) else { return };
            if c.installed(|p| self.has(p)) {
                return;
            }
            self.say(Level::Step, "Installing a compositor");
            let setup = format!("mandara setup --compositor {}", c.id);
            if self.args.no_deps {
                self.say(Level::Warn, format!("{} isn't installed (--no-deps): install it with your package manager, then run `{setup}`", c.label));
                return;
            }
            let Some(package) = self.compositor_package(c) else {
                self.say(Level::Warn, format!("{} isn't installed, and {distro}'s repositories don't carry it: install it yourself, then run `{setup}`", c.label));
                return;
            };
            if self.ask(&format!("{} isn't installed. Install it now ({package}, from your distribution's repositories)?", c.label), true) {
                self.install_compositor(c, package);
            } else {
                self.say(Level::Info, format!("skipped; install {} yourself, then run `{setup}`", c.label));
            }
            return;
        }
        if compositors::any_installed(|p| self.has(p)) {
            return;
        }
        self.say(Level::Step, "Installing a compositor");
        self.say(Level::Info, "No compositor Mandara runs on is installed (MangoWM, Niri, Hyprland, Sway, River Classic or Umbriel).");
        if self.args.no_deps {
            self.say(Level::Info, "install one with your package manager (--no-deps), then run `mandara setup`");
            return;
        }
        let mut offered: Vec<(&'static Compositor, &'static str)> = Vec::new();
        let mut elsewhere: Vec<&'static str> = Vec::new();
        for c in compositors::SUPPORTED {
            match self.compositor_package(c) {
                Some(p) => offered.push((c, p)),
                None => elsewhere.push(c.label),
            }
        }
        if offered.is_empty() {
            self.say(Level::Warn, format!("{distro}'s repositories don't carry any of them: install one yourself, then run `mandara setup`"));
            return;
        }
        let names: Vec<&str> = offered.iter().map(|(c, _)| c.id).collect();
        if self.args.yes && !self.dry() {
            self.say(Level::Info, format!("not installing one unasked (--yes): rerun with --compositor {}, or install one yourself and run `mandara setup`", names.join("|")));
            return;
        }
        for (i, (c, package)) in offered.iter().enumerate() {
            let recommended = if i == 0 { " (recommended)" } else { "" };
            self.say(Level::Info, format!("  {}) {}{recommended} · {package}", i + 1, c.label));
        }
        self.say(Level::Info, "  0) None: I'll install one myself");
        if !elsewhere.is_empty() {
            self.say(Level::Detail, format!("not in {distro}'s repositories: {}", elsewhere.join(", ")));
        }
        if self.dry() && self.package_manager().is_some_and(|pm| !index::missing(pm, &self.facts.root).is_empty()) {
            self.say(Level::Detail, "from Mandara's package table: the package databases aren't downloaded yet, so a real run checks them first");
        }
        const QUESTION: &str = "Install which one?";
        const RECOMMENDED: usize = 1;
        let picked = if self.dry() {
            self.say(Level::Detail, format!("would ask: {QUESTION} [{RECOMMENDED}] (assuming {RECOMMENDED})"));
            Some(RECOMMENDED)
        } else {
            self.host.choose(QUESTION, offered.len(), RECOMMENDED)
        };
        match picked.and_then(|n| n.checked_sub(1)).and_then(|i| offered.get(i).copied()) {
            Some((c, package)) => self.install_compositor(c, package),
            None => self.say(Level::Info, format!("none installed; install one yourself and run `mandara setup`, or rerun with --compositor {}", names.join("|"))),
        }
    }

    /// `c`'s package on this system: in the family's table, and in the
    /// repositories when the package manager can say so quickly.
    fn compositor_package(&mut self, c: &Compositor) -> Option<&'static str> {
        let package = c.package(self.facts.family)?;
        let pm = self.package_manager()?;
        // A dry run that would sync missing databases first can't ask them.
        let databases_ready = !self.dry() || index::missing(pm, &self.facts.root).is_empty();
        if databases_ready
            && let Some((argv, available)) = index::available_query(pm, package)
            && let Some(out) = self.run(Cmd::from_argv(argv).env("LC_ALL", "C").io(Io::Capture))
        {
            return available(&out).then_some(package);
        }
        Some(package)
    }

    /// Installs `c` (and its desktop portal, when it has one), then points
    /// `mandara setup` at it.
    fn install_compositor(&mut self, c: &Compositor, package: &str) {
        let Some(pm) = self.package_manager() else { return };
        if !self.as_root(Cmd::from_argv(pm.install_argv(&[package]))) {
            self.say(Level::Warn, format!("could not install {package}; install {} yourself, then run `mandara setup --compositor {}`", c.label, c.id));
            return;
        }
        if !self.dry() {
            self.say(Level::Ok, format!("{} ({package})", c.label));
        }
        for portal in packages::portal_packages(self.facts.family, |p| c.programs.contains(&p)) {
            if self.as_root(Cmd::from_argv(pm.install_argv(&[portal])).io(Io::Quiet)) && !self.dry() {
                self.say(Level::Ok, portal);
            }
        }
        self.args.compositor = Some(c.id.to_string());
        self.say(Level::Detail, format!("pick {} on your login screen to start it", c.label));
    }

    /// Installing a package doesn't start its service. Bluetooth is turned
    /// on (harmless); NetworkManager only gets a hint, since it would take
    /// over from another network manager (systemd-networkd, iwd, connman).
    fn services_hint(&mut self) {
        if !self.has("systemctl") {
            return;
        }
        let ok = |s: &mut Self, args: &[&str]| s.run(Cmd::new(&[&["systemctl"][..], args].concat()).io(Io::Quiet)).is_some_and(|o| o.success);
        if ok(self, &["list-unit-files", "bluetooth.service"]) && !ok(self, &["is-enabled", "--quiet", "bluetooth.service"]) && self.as_root(Cmd::new(&["systemctl", "enable", "--now", "bluetooth.service"]).io(Io::Quiet)) && !self.dry() {
            self.say(Level::Ok, "bluetooth.service enabled");
        }
        if ok(self, &["list-unit-files", "NetworkManager.service"]) && !ok(self, &["is-active", "--quiet", "NetworkManager.service"]) {
            self.say(Level::Warn, "The Network panel needs NetworkManager running. If nothing else manages your network:");
            let hint = self.root_hint(&["systemctl", "enable", "--now", "NetworkManager"]);
            self.say(Level::Info, format!("  {hint}"));
        }
    }

    // ── Getting the binary ──────────────────────────────────────────────

    /// The running binary, checked against its release's published SHA-256
    /// when one can be reached.
    fn verify_self(&mut self) -> Result<Stage, String> {
        self.say(Level::Step, "Checking this binary");
        let exe = self.facts.exe.clone();
        let version = mandara_version::version();
        if let Some(checkout) = self.facts.exe_checkout.clone() {
            self.say(Level::Ok, format!("{version} · local build from {} (no published checksum to check)", checkout.display()));
            return Ok(Stage::embedded(exe));
        }
        if !self.facts.fingerprinted {
            self.say(Level::Ok, format!("{version} · local build, e.g. `cargo install` (no published checksum to check)"));
            return Ok(Stage::embedded(exe));
        }
        let bytes = std::fs::read(&exe).map_err(|e| format!("{}: {e}", exe.display()))?;
        match self.self_check(&bytes, version, &platform::compiled_target()) {
            SelfCheck::Verified(what) => self.say(Level::Ok, format!("{version} · SHA-256 verified against {what}")),
            SelfCheck::Mismatch(why) if self.args.no_verify => self.say(Level::Warn, format!("{why}; installing anyway (--no-verify)")),
            SelfCheck::Mismatch(why) => return Err(format!("this binary doesn't match its release: {why}. Download the release again, or pass --no-verify to install it anyway")),
            SelfCheck::Unavailable(why) => self.say(Level::Warn, format!("couldn't verify this binary ({why}); installing it unverified")),
        }
        Ok(Stage::embedded(exe))
    }

    /// A release's `SHA256SUMS`, its signature checked (releases after
    /// 0.3.3 must be signed). `Unavailable` when it can't be downloaded,
    /// `Mismatch` when the signature is missing or wrong.
    fn release_sums(&mut self, base: &str, version: &str) -> Result<String, SelfCheck> {
        let sums = self.host.fetch(&format!("{base}/{SUMS_FILE}"), MAX_JSON_BYTES).map_err(|e| SelfCheck::Unavailable(format!("{SUMS_FILE}: {e}")))?;
        let sig = match self.host.fetch(&format!("{base}/{SIG_FILE}"), MAX_JSON_BYTES) {
            Ok(sig) => Some(sig),
            Err(Fetch::NotFound) => None,
            Err(e) => return Err(SelfCheck::Unavailable(format!("{SIG_FILE}: {e}"))),
        };
        signing::checked_sums(&self.facts.release_keys, version, &sums, sig.as_deref()).map_err(SelfCheck::Mismatch)
    }

    /// `update.json`'s binary hash first; else a release tarball next to
    /// the extracted binary, checked against `SHA256SUMS`. For a signed
    /// release, both only count once the signed `SHA256SUMS` vouches for
    /// them.
    fn self_check(&mut self, bytes: &[u8], version: &str, target: &str) -> SelfCheck {
        let base = release::release_base(&self.facts.repo, version, self.facts.mirror.as_deref());
        let actual = mandara_delta::sha256_hex(bytes);
        let signed = signing::must_be_signed(version);
        let mut why = Vec::new();
        let remote_sums = match self.release_sums(&base, version) {
            Ok(sums) => Some(sums),
            Err(SelfCheck::Unavailable(e)) => {
                why.push(e);
                None
            }
            Err(other) => return other,
        };
        match self.host.fetch(&format!("{base}/{MANIFEST_FILE}"), MAX_JSON_BYTES) {
            Ok(json) => {
                let vouched = match (&remote_sums, signed) {
                    (Some(sums), true) => match release::verify_sums(&json, sums, MANIFEST_FILE) {
                        Ok(_) => true,
                        Err(e) => return SelfCheck::Mismatch(e),
                    },
                    (None, true) => {
                        why.push(format!("{MANIFEST_FILE} can't be checked without the signed {SUMS_FILE}"));
                        false
                    }
                    (_, false) => true,
                };
                if vouched {
                    match release::manifest_binary_sha(&json, version, target) {
                        Ok(Some(expected)) if expected == actual => return SelfCheck::Verified(format!("v{version}'s {}", if signed { "signed release" } else { MANIFEST_FILE })),
                        Ok(Some(expected)) => return SelfCheck::Mismatch(format!("v{version}'s {MANIFEST_FILE} publishes {expected} for {target}, this binary is {actual}")),
                        Ok(None) => why.push(format!("{MANIFEST_FILE} lists no {target} build")),
                        Err(e) => why.push(e),
                    }
                }
            }
            Err(e) => why.push(format!("{MANIFEST_FILE}: {e}")),
        }
        for archive in release::local_archives(&self.facts.exe, &self.facts.cwd, version, target) {
            let name = archive.file_name().unwrap_or_default().to_string_lossy().into_owned();
            let local = std::fs::read(archive.with_file_name(SUMS_FILE)).ok().map(|sums| (sums, std::fs::read(archive.with_file_name(SIG_FILE)).ok()));
            let sums = match local {
                Some((sums, Some(sig))) => match signing::checked_sums(&self.facts.release_keys, version, &sums, Some(&sig)) {
                    Ok(sums) => Some(sums),
                    Err(e) => return SelfCheck::Mismatch(e),
                },
                Some((sums, None)) if !signed => Some(String::from_utf8_lossy(&sums).into_owned()),
                _ => remote_sums.clone(),
            };
            let Some(sums) = sums else {
                why.push(format!("no {} {SUMS_FILE} for {name}", if signed { "signed" } else { "published" }));
                continue;
            };
            match std::fs::read(&archive) {
                Ok(data) => return release::check_against_archive(bytes, &data, &name, &sums),
                Err(e) => why.push(format!("{}: {e}", archive.display())),
            }
        }
        SelfCheck::Unavailable(why.join("; "))
    }

    /// Downloads and verifies a release. `Ok(None)`: nothing installable
    /// (already reported; the caller falls back to a source build).
    fn fetch_release(&mut self, version: Option<String>) -> Result<Option<Stage>, String> {
        self.say(Level::Step, "Fetching Mandara");
        if self.facts.mirror.is_some() && version.is_none() {
            return Err(format!("{} needs --version", release::RELEASE_BASE_ENV));
        }
        let repo = self.facts.repo.clone();
        let version = match version {
            Some(v) => v,
            None => match self.host.fetch(&release::latest_url(&repo), MAX_JSON_BYTES) {
                Ok(json) => match release::parse_latest(&json) {
                    Some(v) => v,
                    None => {
                        self.say(Level::Warn, "no release published yet");
                        return Ok(None);
                    }
                },
                Err(_) => {
                    self.say(Level::Warn, format!("couldn't reach GitHub releases for {repo}"));
                    return Ok(None);
                }
            },
        };
        let base = release::release_base(&repo, &version, self.facts.mirror.as_deref());
        let candidates = platform::release_candidates(&version, &self.facts.arch, self.facts.libc, self.facts.x86_64_v3);
        let found = candidates.iter().find_map(|stem| {
            let file = format!("{stem}{ARCHIVE_SUFFIX}");
            self.host.fetch(&format!("{base}/{file}"), MAX_ARCHIVE_BYTES).ok().map(|bytes| (file, bytes))
        });
        let Some((file, archive)) = found else {
            let (arch, libc) = (self.facts.arch.clone(), self.facts.libc);
            self.say(Level::Warn, format!("release v{version} has no build for {arch} ({libc})"));
            return Ok(None);
        };
        self.say(Level::Ok, format!("v{version} · {file}"));
        let sums = self.host.fetch(&format!("{base}/{SUMS_FILE}"), MAX_JSON_BYTES).map_err(|_| format!("couldn't download {SUMS_FILE} for v{version}"))?;
        let sig = match self.host.fetch(&format!("{base}/{SIG_FILE}"), MAX_JSON_BYTES) {
            Ok(sig) => Some(sig),
            Err(Fetch::NotFound) => None,
            Err(e) => return Err(format!("couldn't download {SIG_FILE} for v{version}: {e}")),
        };
        let sums = signing::checked_sums(&self.facts.release_keys, &version, &sums, sig.as_deref()).map_err(|e| format!("{e}; not installing it"))?;
        release::verify_sums(&archive, &sums, &file)?;
        match sig {
            Some(_) => self.say(Level::Ok, "SHA-256 verified, signed by Mandara's release key"),
            None => self.say(Level::Ok, format!("SHA-256 verified (v{version} came out before releases were signed)")),
        }
        let bin = release::extract(&archive, SHELL_BINARY)?.ok_or_else(|| format!("{file} has no {SHELL_BINARY}"))?;
        let exe = self.tmp()?.join(files::BINARY);
        write_executable(&exe, &bin)?;
        let desktop = release::extract(&archive, &format!("share/{}/{}", files::APPLICATIONS_DIR, files::DESKTOP_FILE))?.map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_else(|| files::DESKTOP_ENTRY.to_string());
        let icon = release::extract(&archive, &format!("share/{}/{}", files::ICON_DIR, files::ICON_FILE))?.unwrap_or_else(|| files::ICON_SVG.to_vec());
        let packs = archive_packs(&archive, &version)?;
        Ok(Some(Stage { exe, desktop, icon, packs: (version, packs) }))
    }

    /// A release build from source (`cargo xtask build`). `None` in a dry run.
    fn build_source(&mut self) -> Result<Option<Stage>, String> {
        self.say(Level::Step, "Rust toolchain");
        let cargo_bin = self.facts.home.join(".cargo/bin");
        let path_env = format!("{}:{}", cargo_bin.display(), self.facts.path_var);
        let found = self.tool_version("rustc", &path_env);
        if self.dry() {
            let found = found.unwrap_or_else(|| "none".into());
            self.say(Level::Detail, format!("would ensure Rust ≥ {MIN_RUST} (found: {found}) and build from source"));
            let from = match self.checkout() {
                Some(c) => c.display().to_string(),
                None => format!("{} (clone of {})", self.source_clone_dir().display(), release::clone_url(&self.facts.repo)),
            };
            self.say(Level::Detail, format!("would build: cargo run --locked -p xtask -- build --no-readme in {from}"));
            return Ok(None);
        }
        match found {
            Some(v) if platform::version_ge(&v, MIN_RUST) => self.say(Level::Ok, format!("rustc {v}")),
            _ if self.has("rustup") || cargo_bin.join("rustup").is_file() => {
                for args in [["update", "stable"], ["default", "stable"]] {
                    let ok = self.run(Cmd::new(&[&["rustup"][..], &args[..]].concat()).env("PATH", path_env.clone()).io(Io::Quiet)).is_some_and(|o| o.success);
                    if !ok {
                        return Err(format!("`rustup {}` failed", args.join(" ")));
                    }
                }
                let v = self.tool_version("rustc", &path_env).unwrap_or_default();
                self.say(Level::Ok, format!("updated to {v}"));
            }
            _ => {
                self.say(Level::Info, "installing rustup (https://rustup.rs) for your user");
                self.install_rustup(&path_env)?;
                let v = self.tool_version("rustc", &path_env).unwrap_or_default();
                self.say(Level::Ok, format!("rustc {v}"));
            }
        }
        let src = match self.checkout() {
            Some(c) => {
                self.say(Level::Info, format!("building the checkout {}", c.display()));
                c
            }
            None => self.fetch_source()?,
        };
        self.say(Level::Step, "Building Mandara (release)");
        self.say(Level::Detail, "first build takes a few minutes; later upgrades are incremental");
        // `xtask build`: a release build with the source fingerprint
        // embedded (`mandara --version`); --no-readme leaves README alone.
        let built = self.run(Cmd::new(&["cargo", "run", "--locked", "--quiet", "-p", "xtask", "--", "build", "--no-readme"]).cwd(&src).env("PATH", path_env)).is_some_and(|o| o.success);
        let exe = src.join("target/release").join(files::BINARY);
        if !built || !exe.is_file() {
            return Err(format!("build failed — run: (cd {} && cargo xtask build --no-readme)", src.display()));
        }
        let version = self.run(Cmd::new(&[exe.to_string_lossy().as_ref(), "--version"]).io(Io::Capture)).map(|o| o.stdout).unwrap_or_default();
        let fingerprint = version.split_whitespace().skip_while(|w| *w != "fingerprint").nth(1).map(|f| format!("fingerprint {f}")).unwrap_or_else(|| "built".into());
        self.say(Level::Ok, format!("target/release/mandara ({fingerprint})"));
        let packaging = src.join("packaging");
        let desktop = std::fs::read_to_string(packaging.join(files::DESKTOP_FILE)).unwrap_or_else(|_| files::DESKTOP_ENTRY.to_string());
        let icon = std::fs::read(packaging.join(files::ICON_FILE)).unwrap_or_else(|_| files::ICON_SVG.to_vec());
        // The packs, compiled from the same sources for the built version.
        let built_version = version.split_whitespace().nth(1).unwrap_or_default().to_string();
        let catalogs = src.join("crates/mandara-i18n/locales");
        let packs = Lang::ALL.into_iter().filter(|l| *l != Lang::En).filter_map(|l| std::fs::read_to_string(catalogs.join(format!("{}.toml", l.code()))).ok().and_then(|text| mandara_i18n::compile(l, &built_version, &text).ok()).map(|b| (l, b))).collect();
        Ok(Some(Stage { exe, desktop, icon, packs: (built_version, packs) }))
    }

    /// The second word of `<tool> --version`.
    fn tool_version(&mut self, tool: &str, path_env: &str) -> Option<String> {
        let out = self.run(Cmd::new(&[tool, "--version"]).env("PATH", path_env).io(Io::Capture))?;
        out.success.then(|| out.stdout.split_whitespace().nth(1).map(str::to_string)).flatten()
    }

    /// rustup's own installer binary (no shell script), checksum-verified.
    fn install_rustup(&mut self, path_env: &str) -> Result<(), String> {
        let url = format!("{RUSTUP_DIST}/{}/{RUSTUP_INIT}", platform::rust_triple());
        let bin = self.host.fetch(&url, MAX_RUSTUP_BYTES).map_err(|e| format!("{url}: {e}"))?;
        let sum = self.host.fetch(&format!("{url}{SHA256_SUFFIX}"), MAX_JSON_BYTES).map_err(|e| format!("{url}{SHA256_SUFFIX}: {e}"))?;
        let expected = String::from_utf8_lossy(&sum).split_whitespace().next().unwrap_or_default().to_ascii_lowercase();
        let actual = mandara_delta::sha256_hex(&bin);
        if expected != actual {
            return Err(format!("checksum mismatch for {RUSTUP_INIT} (expected {expected}, got {actual})"));
        }
        let path = self.tmp()?.join(RUSTUP_INIT);
        write_executable(&path, &bin)?;
        let ok = self.run(Cmd::new(&[path.to_string_lossy().as_ref(), "-y", "--profile", "minimal", "--default-toolchain", "stable"]).env("PATH", path_env).io(Io::Quiet)).is_some_and(|o| o.success);
        if ok { Ok(()) } else { Err(format!("{RUSTUP_INIT} failed")) }
    }

    /// The checkout to build: the one this binary came from, else the
    /// current folder when it is one.
    fn checkout(&self) -> Option<PathBuf> {
        self.facts.exe_checkout.clone().or_else(|| files::is_checkout(&self.facts.cwd).then(|| self.facts.cwd.clone()))
    }

    fn source_clone_dir(&self) -> PathBuf {
        self.paths.data_dir.join("mandara/src")
    }

    /// Clones (or updates) the source into `~/.local/share/mandara/src`.
    fn fetch_source(&mut self) -> Result<PathBuf, String> {
        let src = self.source_clone_dir();
        self.say(Level::Step, "Fetching the source");
        let version = self.args.wanted_version();
        let s = src.to_string_lossy().into_owned();
        let steps: Vec<Vec<String>> = if src.join(".git").is_dir() {
            let update = match &version {
                Some(v) => vec!["git".into(), "-C".into(), s.clone(), "checkout".into(), "--quiet".into(), format!("v{v}")],
                None => vec!["git".into(), "-C".into(), s.clone(), "pull".into(), "--ff-only".into(), "--quiet".into()],
            };
            vec![vec!["git".into(), "-C".into(), s.clone(), "fetch".into(), "--quiet".into(), "--tags".into(), "origin".into()], update]
        } else {
            if let Some(parent) = src.parent() {
                std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
            }
            let mut clone: Vec<String> = ["git", "clone", "--quiet", "--depth", "1"].iter().map(|a| a.to_string()).collect();
            if let Some(v) = &version {
                clone.extend(["--branch".into(), format!("v{v}")]);
            }
            clone.extend([release::clone_url(&self.facts.repo), s.clone()]);
            vec![clone]
        };
        for argv in steps {
            let shown = argv.join(" ");
            if !self.run(Cmd::from_argv(argv)).is_some_and(|o| o.success) {
                return Err(format!("`{shown}` failed"));
            }
        }
        self.say(Level::Ok, &s);
        Ok(src)
    }

    // ── Putting it in place ─────────────────────────────────────────────

    fn dry_run_summary(&mut self, exe: &Path) -> Result<(), String> {
        self.say(Level::Step, "Dry run");
        let (bin, desktop, icon) = (self.paths.binary(), self.paths.desktop_entry(), self.paths.icon());
        self.say(Level::Info, format!("would install: {}, {}, {}", bin.display(), desktop.display(), icon.display()));
        let mut setup = String::new();
        if let Some(c) = &self.args.compositor {
            setup.push_str(&format!(" --compositor {c}"));
        }
        if !self.args.no_autostart {
            setup.push_str(" --autostart");
        }
        self.say(Level::Info, format!("then run: mandara setup{setup}"));
        self.say(Level::Info, "then run: mandara setup --check (and offer --fix-conflicts)");
        self.admin_helper(exe);
        self.greeter(exe);
        self.i2c();
        Ok(())
    }

    fn install_files(&mut self, stage: &Stage) -> Result<(), String> {
        self.say(Level::Step, "Installing");
        let err = |p: &Path, e: std::io::Error| format!("{}: {e}", p.display());
        for d in self.paths.dirs() {
            std::fs::create_dir_all(&d).map_err(|e| err(&d, e))?;
        }
        let bin = self.paths.binary();
        files::install_binary(&stage.exe, &bin).map_err(|e| err(&bin, e))?;
        let version = self.run(Cmd::new(&[bin.to_string_lossy().as_ref(), "--version"]).io(Io::Capture)).filter(|o| o.success).and_then(|o| o.stdout.lines().next().map(|l| l.split_once(' ').map_or(l, |(_, v)| v).to_string())).unwrap_or_else(|| "installed".into());
        self.say(Level::Ok, format!("{} ({version})", bin.display()));
        // Language packs: checked (version and language in the header)
        // and written atomically; anything missing downloads on first use.
        let (pack_version, stage_packs) = &stage.packs;
        if !stage_packs.is_empty() {
            let dir = self.paths.data_dir.join(packs::DIR).join(pack_version);
            let copied = stage_packs.iter().filter(|(lang, bytes)| packs::install(*lang, pack_version, bytes, &dir).map_err(|e| self.say(Level::Warn, format!("{} language pack: {e}", lang.code()))).is_ok()).count();
            self.say(Level::Ok, format!("{copied} language packs in {}", dir.display()));
        }
        let desktop = self.paths.desktop_entry();
        std::fs::write(&desktop, files::desktop_entry(&stage.desktop, &bin)).map_err(|e| err(&desktop, e))?;
        // The icon is cosmetic: a failure here never stops the install.
        let _ = std::fs::write(self.paths.icon(), &stage.icon);
        self.say(Level::Ok, "Mandara Studio desktop entry");
        let bin_dir = self.paths.bin_dir.clone();
        if !std::env::split_paths(&self.facts.path_var).any(|p| p == bin_dir) {
            self.say(Level::Warn, format!("{} is not on your PATH", bin_dir.display()));
            if self.ask(&format!("Add {} to your PATH in your shell profiles?", bin_dir.display()), true) && !self.dry() {
                match add_to_path(&self.facts.home, &bin_dir) {
                    Ok(files) if files.is_empty() => {}
                    Ok(files) => {
                        let names: Vec<String> = files.iter().map(|f| f.display().to_string()).collect();
                        self.say(Level::Ok, format!("on your PATH in new terminals ({})", names.join(", ")));
                    }
                    Err(e) => self.say(Level::Warn, format!("couldn't add it to your PATH ({e}) — add export PATH=\"{}:$PATH\" to your shell profile", bin_dir.display())),
                }
            } else {
                self.say(Level::Detail, format!("add it yourself: export PATH=\"{}:$PATH\"", bin_dir.display()));
            }
        }
        Ok(())
    }

    // ── Interactive setup ───────────────────────────────────────────────

    /// The arrow-key setup screen: which compositor to integrate with, and
    /// the autostart and login-screen questions the steps below would
    /// otherwise ask one by one. A choice already given on the command line
    /// is not offered. False when the user cancelled, before anything ran.
    fn preflight(&mut self) -> Result<bool, String> {
        if self.args.yes || self.dry() || !self.host.interactive() {
            return Ok(true);
        }
        // The installed compositor only puts the picker's cursor where the
        // user probably wants it; it is not a choice. Leaving `--compositor`
        // unset keeps `setup` detecting the compositor that is running (a
        // machine can have several installed).
        let suggested = self.args.compositor.clone().or_else(|| installed_compositor(&*self.host));
        let mut compositor: Option<String> = self.args.compositor.clone();
        let mut autostart = !self.args.no_autostart;
        let mut greeter = !self.args.no_greeter;
        loop {
            let (mut rows, mut choices) = (Vec::new(), Vec::new());
            if self.args.compositor.is_none() {
                rows.push(Row::new("Compositor", compositor_label(&*self.host, compositor.as_deref())));
                choices.push(Choice::Compositor);
            }
            if !self.args.no_autostart {
                rows.push(Row::new("Start with your compositor", yes_no(autostart)));
                choices.push(Choice::Autostart);
            }
            if !self.args.no_greeter {
                rows.push(Row::new("Login screen", if greeter { "Mandara Login" } else { "keep yours" }));
                choices.push(Choice::Greeter);
            }
            let install = choices.len();
            rows.push(Row::new("Install", ""));
            choices.push(Choice::Install);
            if install == 0 {
                break;
            }
            let Some(picked) = self.host.select("Setup", &rows, install) else { return Ok(false) };
            match choices[picked] {
                Choice::Compositor => {
                    let (names, list, default) = compositor_rows(&*self.host, compositor.as_deref().or(suggested.as_deref()));
                    // Cancel leaves the whole screen, as it does on the menu
                    // above (and as the screen's hint says).
                    let Some(i) = self.host.select("Compositor", &list, default) else { return Ok(false) };
                    compositor = Some(names[i].to_string());
                }
                Choice::Autostart => autostart = !autostart,
                Choice::Greeter => greeter = !greeter,
                Choice::Install => break,
            }
        }
        // `offer_compositor` installs it when it is missing, and `compositor`
        // integrates with it.
        if self.args.compositor.is_none() {
            self.args.compositor = compositor;
        }
        self.setup = Some(SetupChoices { autostart, greeter });
        Ok(true)
    }

    /// `mandara setup` (one include line, generated drop-in, autostart),
    /// then the conflict check and an offer to retire what it found.
    fn compositor(&mut self) {
        self.say(Level::Step, "Compositor integration");
        let bin = self.paths.binary().to_string_lossy().into_owned();
        let with_compositor = |mut argv: Vec<String>, c: &Option<String>| {
            if let Some(c) = c {
                argv.extend(["--compositor".into(), c.clone()]);
            }
            argv
        };
        let compositor = self.args.compositor.clone();
        let mut setup = with_compositor(vec![bin.clone(), "setup".into()], &compositor);
        let autostart = self.setup.map_or_else(|| !self.args.no_autostart && self.ask("Start Mandara automatically with your compositor?", true), |s| s.autostart);
        if autostart {
            setup.push("--autostart".into());
        }
        match self.run(Cmd::from_argv(setup).env(SETUP_LOG.0, SETUP_LOG.1).io(Io::Capture)) {
            Some(out) => {
                for line in out.stdout.lines().chain(out.stderr.lines()) {
                    self.say(Level::Info, line);
                }
                if !out.success {
                    self.say(Level::Warn, "compositor setup failed; run `mandara setup` later");
                }
            }
            None => self.say(Level::Warn, "compositor setup failed; run `mandara setup` later"),
        }
        // Leftovers that fight Mandara: an old launcher or wallpaper picker
        // on its keys, another bar, notification or wallpaper daemon.
        // Retiring comments those lines out (marked, every file backed up),
        // validates with the compositor and rolls back if it objects.
        let check = with_compositor(vec![bin.clone(), "setup".into(), "--check".into()], &compositor);
        let checked = self.run(Cmd::from_argv(check).env(SETUP_LOG.0, SETUP_LOG.1).io(Io::Quiet));
        if checked.as_ref().is_none_or(|o| o.success) {
            return;
        }
        // No conflicts, but the compositor itself reports errors in the
        // config: the Config doctor explains them and offers fixes.
        if checked.as_ref().and_then(|o| o.code) == Some(SETUP_CONFIG_ERRORS) {
            self.say(Level::Warn, "your compositor reports errors in its config: run `mandara setup --check` to see them, or open Studio → Compositor → Config doctor");
            return;
        }
        if self.ask("Retire the conflicts listed above (backed up, reversible with `mandara setup --restore latest`)?", true) {
            let fix = with_compositor(vec![bin, "setup".into(), "--fix-conflicts".into()], &compositor);
            if let Some(out) = self.run(Cmd::from_argv(fix).env(SETUP_LOG.0, SETUP_LOG.1).io(Io::Capture)) {
                for line in out.stdout.lines().chain(out.stderr.lines()).filter(|l| FIX_CONFLICT_WORDS.iter().any(|w| l.contains(w))) {
                    self.say(Level::Info, line.trim());
                }
            }
        } else {
            self.say(Level::Info, "kept — Mandara skips its binds on those keys; run `mandara setup --fix-conflicts` any time");
        }
    }

    /// Mandara Login becomes the login screen unless --no-greeter: greetd +
    /// cage come from the distribution (not with --no-deps), then
    /// `mandara greeter use mandara --apply` rewrites only greetd's
    /// `[default_session]` command and moves display-manager.service, with a
    /// timestamped backup. Nothing is started or stopped; it shows at the
    /// next boot. `exe` is the binary the probe and the root commands use.
    fn greeter(&mut self, exe: &Path) {
        let wanted = self.setup.map_or(!self.args.no_greeter, |s| s.greeter);
        if !wanted {
            let line = if self.args.no_greeter { "login screen unchanged (--no-greeter); switch any time in Studio → Login screen → Login manager" } else { "login screen kept; switch any time in Studio → Login screen → Login manager" };
            self.say(Level::Detail, line);
            return;
        }
        // greetd's config is generated by NixOS: the module sets it up.
        if self.facts.family == Family::NixOS {
            self.say(Level::Detail, "login screen: on NixOS, set programs.mandara.greeter.enable = true; and rebuild");
            return;
        }
        self.say(Level::Step, "Login screen");
        let exe_s = exe.to_string_lossy().into_owned();
        let use_mandara = |exe: &str| Cmd::new(&[exe, "greeter", "use", "mandara", "--apply"]);
        let hint = self.root_hint(&["mandara", "greeter", "use", "mandara", "--apply"]);
        if self.probe.active() {
            if self.probe.outdated(exe) {
                if self.as_root(use_mandara(&exe_s).io(Io::Quiet)) {
                    if !self.dry() {
                        self.say(Level::Ok, "Mandara Login updated");
                    }
                } else {
                    self.say(Level::Warn, format!("could not update Mandara Login; run: {hint}"));
                }
            } else {
                self.say(Level::Ok, "Mandara Login is your login screen");
            }
            self.keyring_unlock(exe);
            return;
        }
        // The setup screen has already asked; only runs without it ask here.
        if self.setup.is_none() && !self.ask("Use Mandara Login as your login screen?", true) {
            self.say(Level::Info, "kept your login screen; switch any time in Studio → Login screen → Login manager");
            return;
        }
        let missing: Vec<&str> = LOGIN_PROGRAMS.iter().copied().filter(|p| !self.has(p)).collect();
        let installing = !missing.is_empty();
        if installing {
            let pkgs = packages::for_family(self.facts.family).map(|s| s.greeter).unwrap_or_default();
            if self.args.no_deps {
                self.say(Level::Warn, format!("missing: {} (--no-deps): install them, then run: {hint}", missing.join(" ")));
                return;
            }
            let Some(pm) = self.package_manager().filter(|_| !pkgs.is_empty()) else {
                self.say(Level::Warn, format!("install greetd and cage with your package manager, then run: {hint}"));
                return;
            };
            if !self.as_root(Cmd::from_argv(pm.install_argv(pkgs))) {
                self.say(Level::Warn, format!("could not install greetd + cage; login screen unchanged (run `{hint}` later)"));
                return;
            }
            if !self.dry() {
                self.say(Level::Ok, "greetd + cage");
            }
        }
        if self.dry() {
            let plan = self.probe.switch_plan(exe, installing);
            for line in plan.lines() {
                self.say(Level::Info, line);
            }
            let bin = self.paths.binary().to_string_lossy().into_owned();
            self.as_root(use_mandara(&bin));
            self.keyring_unlock(exe);
            return;
        }
        if self.as_root(use_mandara(&exe_s)) {
            self.say(Level::Ok, "Mandara Login is your login screen from the next boot (undo: sudo mandara greeter restore latest)");
            self.keyring_unlock(exe);
        } else {
            self.say(Level::Warn, "the login screen was not changed; see above, or switch later in Studio → Login screen → Login manager");
        }
    }

    /// GNOME Keyring / KWallet open with the login password: their PAM
    /// lines go into greetd's PAM file (marked, backed up). Offered only
    /// when a module is installed and the lines are missing.
    fn keyring_unlock(&mut self, exe: &Path) {
        let Some(plan) = self.probe.keyring_plan() else { return };
        let exe_s = exe.to_string_lossy().into_owned();
        let hint = self.root_hint(&["mandara", "greeter", "keyring-unlock", "--apply"]);
        if self.dry() {
            for line in plan.lines() {
                self.say(Level::Info, line);
            }
            let bin = self.paths.binary().to_string_lossy().into_owned();
            self.as_root(Cmd::new(&[bin.as_str(), "greeter", "keyring-unlock", "--apply"]));
            return;
        }
        if !self.ask("Unlock GNOME Keyring / KWallet with your login password?", true) {
            self.say(Level::Info, format!("skipped; set it up any time: {hint}"));
            return;
        }
        if self.as_root(Cmd::new(&[exe_s.as_str(), "greeter", "keyring-unlock", "--apply"]).io(Io::Quiet)) {
            self.say(Level::Ok, "keyrings unlock when you log in with your password (undo: sudo mandara greeter keyring-unlock --remove)");
        } else {
            self.say(Level::Warn, format!("keyrings were not set up; run: {hint}"));
        }
    }

    /// Studio's administrator actions (login screen, keyring unlock) run as
    /// root only through a root-owned copy of Mandara, never the copy in
    /// your home, which your account (or malware in it) can change. This
    /// installs that copy from the binary just checked against its release.
    fn admin_helper(&mut self, exe: &Path) {
        // NixOS installs nothing as root from here; a `mandara` from the Nix
        // store is root-owned already, and Studio uses that.
        if self.facts.family == Family::NixOS {
            self.say(Level::Detail, "admin helper: on NixOS, install mandara through your configuration; Studio runs that root-owned copy for administrator actions");
            return;
        }
        let exe_s = exe.to_string_lossy().into_owned();
        let argv = ["install", "-D", "-o", "root", "-g", "root", "-m", "0755", exe_s.as_str(), ADMIN_EXE];
        if self.dry() {
            self.as_root(Cmd::new(&argv));
            return;
        }
        self.say(Level::Step, "Admin helper");
        if !self.ask("Install Mandara's admin helper (a copy only root can change, used when Studio needs administrator rights)?", true) {
            self.say(Level::Info, "skipped; Studio installs it the first time it needs administrator rights");
            return;
        }
        if self.as_root(Cmd::new(&argv).io(Io::Quiet)) {
            self.say(Level::Ok, format!("admin helper: {ADMIN_EXE}"));
        } else {
            let hint = self.root_hint(&argv);
            self.say(Level::Warn, format!("admin helper not installed; run: {hint}"));
        }
    }

    /// `mandara lock` uses the `login` PAM service unless its own exists.
    fn lock_hint(&mut self) {
        // On NixOS `programs.mandara.enable` declares the PAM service.
        if self.facts.family != Family::NixOS && !self.facts.sys(LOCK_PAM).exists() {
            let hint = self.root_hint(&["install", "-Dm644", "share/pam.d/mandara-lock", LOCK_PAM]);
            self.say(Level::Detail, format!("Lock screen: `mandara lock` (preview: `mandara lock --demo`). Optional dedicated PAM service, from the release tarball: {hint}"));
        }
    }

    /// Studio → Displays talks DDC/CI over `/dev/i2c-*`: it needs the
    /// i2c-dev module and, on most distributions, the i2c group.
    fn i2c(&mut self) {
        let external = std::fs::read_dir(self.facts.sys("/sys/class/drm")).into_iter().flatten().flatten().any(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.starts_with("card") && (n.contains("-DP-") || n.contains("-HDMI-"))
        });
        if !external {
            return;
        }
        let devices = std::fs::read_dir(self.facts.sys("/dev")).into_iter().flatten().flatten().any(|e| e.file_name().to_string_lossy().starts_with("i2c-"));
        let groups = std::fs::read_to_string(self.facts.sys("/etc/group")).unwrap_or_default();
        let passwd = std::fs::read_to_string(self.facts.sys("/etc/passwd")).unwrap_or_default();
        let group = device_group(&self.facts.sys("/dev/i2c-0"), &groups).unwrap_or_else(|| I2C_DEFAULT_GROUP.to_string());
        let user = self.facts.user.clone();
        if devices && user_in_group(&user, &group, &groups, &passwd) {
            return;
        }
        if !self.ask(&format!("Allow Mandara to set external monitor brightness (load {I2C_MODULE} at boot, add {user} to the {group} group)?"), false) {
            let hint = format!("{} && {} (then log in again)", self.root_hint(&["modprobe", I2C_MODULE]), self.root_hint(&["usermod", "-aG", I2C_DEFAULT_GROUP, "$USER"]));
            self.say(Level::Detail, format!("External monitor brightness later: {hint}"));
            return;
        }
        if !self.as_root(Cmd::new(&["tee", I2C_MODULES_LOAD]).stdin(format!("{I2C_MODULE}\n")).io(Io::Quiet)) {
            self.say(Level::Warn, format!("could not write {I2C_MODULES_LOAD}"));
        }
        self.as_root(Cmd::new(&["modprobe", I2C_MODULE]).io(Io::Quiet));
        if group_gid(&group, &groups).is_none() {
            self.as_root(Cmd::new(&["groupadd", "-r", group.as_str()]).io(Io::Quiet));
        }
        if self.as_root(Cmd::new(&["usermod", "-aG", group.as_str(), user.as_str()])) && !self.dry() {
            self.say(Level::Ok, format!("added {user} to {group} (log in again for it to apply)"));
        }
    }

    /// Restarts a running shell, only if it runs this installation.
    fn restart(&mut self) {
        let bin = self.paths.binary();
        let Ok(procs) = std::fs::read_dir(self.facts.sys("/proc")) else { return };
        for e in procs.flatten() {
            let dir = e.path();
            if !e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()) || std::fs::read_to_string(dir.join("comm")).map(|c| c.trim() != files::BINARY).unwrap_or(true) {
                continue;
            }
            let Ok(target) = std::fs::read_link(dir.join("exe")) else { continue };
            if Path::new(target.to_string_lossy().trim_end_matches(DELETED_SUFFIX)) != bin {
                continue;
            }
            if self.run(Cmd::new(&[bin.to_string_lossy().as_ref(), "msg", "restart"]).io(Io::Quiet)).is_some_and(|o| o.success) {
                self.say(Level::Ok, "restarted the running shell on the new version");
            }
            break;
        }
    }

    // ── Uninstall ───────────────────────────────────────────────────────

    /// `mandara uninstall`: the binary, desktop entry and icon go; the
    /// compositor drop-ins are emptied; settings and data stay.
    pub fn uninstall(&mut self) -> Result<(), String> {
        if self.facts.is_root {
            return Err("run this as your user, not root".into());
        }
        self.say(Level::Step, "Removing Mandara");
        if self.dry() {
            self.say(Level::Info, "dry run: nothing will be changed");
        }
        if self.probe.active() {
            let hint = self.root_hint(&["mandara", "greeter", "restore", "latest"]);
            self.say(Level::Warn, format!("Mandara Login stays your login screen (its own copy keeps working); to switch back first run: {hint}"));
        }
        let bin = self.paths.binary();
        let quit = if bin.is_file() { Some(bin.to_string_lossy().into_owned()) } else { self.has(files::BINARY).then(|| files::BINARY.to_string()) };
        if let Some(exe) = quit {
            if self.dry() {
                self.say(Level::Detail, format!("would run: {exe} msg quit"));
            } else {
                self.run(Cmd::new(&[exe.as_str(), "msg", "quit"]).io(Io::Quiet));
            }
        }
        for f in [bin, self.paths.desktop_entry(), self.paths.icon()] {
            if self.dry() {
                if f.exists() {
                    self.say(Level::Detail, format!("would remove {}", f.display()));
                }
            } else if let Err(e) = std::fs::remove_file(&f)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                return Err(format!("{}: {e}", f.display()));
            }
        }
        if !self.dry() {
            self.say(Level::Ok, "binary and desktop entry removed");
        }
        // The drop-ins would keep starting (and binding keys to) the
        // removed binary.
        let generated = self.paths.generated();
        for name in files::GENERATED_DROPINS {
            let path = generated.join(name);
            if !path.is_file() {
                continue;
            }
            if self.dry() {
                self.say(Level::Detail, format!("would empty {}", path.display()));
            } else {
                std::fs::write(&path, files::emptied_dropin(name)).map_err(|e| format!("{}: {e}", path.display()))?;
            }
        }
        if !self.dry() {
            self.say(Level::Ok, "compositor drop-ins emptied (no autostart, no Mandara keybinds)");
        }
        let (config, data) = (self.paths.config_dir.join("mandara"), self.paths.data_dir.join("mandara"));
        self.say(Level::Info, format!("Kept: your settings in {} and data in {}.", config.display(), data.display()));
        self.say(Level::Info, "Your compositor config still has one Mandara include line (it now loads an empty file); delete it by hand if you like.");
        Ok(())
    }
}

/// Writes `bytes` as an executable file.
fn write_executable(path: &Path, bytes: &[u8]) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::write(path, bytes).and_then(|_| std::fs::set_permissions(path, std::fs::Permissions::from_mode(MODE_EXECUTABLE))).map_err(|e| format!("{}: {e}", path.display()))
}

/// `name`'s gid in `/etc/group` text.
fn group_gid(name: &str, groups: &str) -> Option<u32> {
    groups.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() >= 3 && f[0] == name).then(|| f[2].parse().ok()).flatten()
    })
}

/// The group owning `device`, named from `/etc/group` text.
fn device_group(device: &Path, groups: &str) -> Option<String> {
    use std::os::unix::fs::MetadataExt as _;
    let gid = std::fs::metadata(device).ok()?.gid();
    groups.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() >= 3 && f[2].parse() == Ok(gid)).then(|| f[0].to_string())
    })
}

/// Whether `user` is in `group`: listed as a member, or it's their primary
/// group (from `/etc/passwd`).
fn user_in_group(user: &str, group: &str, groups: &str, passwd: &str) -> bool {
    let member = groups.lines().any(|l| {
        let f: Vec<&str> = l.split(':').collect();
        f.len() >= 4 && f[0] == group && f[3].split(',').any(|m| m.trim() == user)
    });
    let primary = passwd.lines().find_map(|l| {
        let f: Vec<&str> = l.split(':').collect();
        (f.len() >= 4 && f[0] == user).then(|| f[3].parse::<u32>().ok()).flatten()
    });
    member || (primary.is_some() && primary == group_gid(group, groups))
}

/// The compositor the setup screen starts on: one already installed (the
/// first the installer recommends), else nothing — then the choice is left
/// to `offer_compositor`, which knows what the distribution carries.
fn installed_compositor(host: &dyn Host) -> Option<String> {
    compositors::SUPPORTED.iter().find(|c| c.installed(|p| host.has_program(p))).map(|c| c.id.to_string())
}

/// The current compositor as the setup screen shows it.
fn compositor_label(host: &dyn Host, chosen: Option<&str>) -> String {
    let Some(c) = chosen.and_then(compositors::by_name) else { return "detect automatically".into() };
    if c.installed(|p| host.has_program(p)) {
        format!("{} (installed)", c.label)
    } else {
        c.label.to_string()
    }
}

/// The compositor picker's rows, the names behind them and where the cursor
/// starts.
fn compositor_rows(host: &dyn Host, chosen: Option<&str>) -> (Vec<&'static str>, Vec<Row>, usize) {
    let rows = compositors::SUPPORTED.iter().map(|c| Row::new(c.label, if c.installed(|p| host.has_program(p)) { "installed" } else { "" })).collect();
    let names: Vec<&'static str> = compositors::SUPPORTED.iter().map(|c| c.id).collect();
    let default = chosen.and_then(|c| names.iter().position(|id| *id == c)).unwrap_or(0);
    (names, rows, default)
}

/// A yes/no setting as the setup screen shows it.
fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}

#[cfg(test)]
mod tests;

/// Marks the lines [`add_to_path`] writes, so a second install adds nothing.
const PATH_MARK: &str = "# Added by the Mandara installer";

/// Puts `bin_dir` on `PATH` in `~/.profile`, `~/.bashrc`, an existing
/// `~/.zshrc` and, when fish is set up, a fish `conf.d` snippet. Returns the
/// files it changed; a file that already has the line is left alone.
fn add_to_path(home: &Path, bin_dir: &Path) -> std::io::Result<Vec<PathBuf>> {
    use std::io::Write as _;
    let dir = bin_dir.display();
    let posix = format!("\n{PATH_MARK}\ncase \":$PATH:\" in *\":{dir}:\"*) ;; *) export PATH=\"{dir}:$PATH\" ;; esac\n");
    let mut targets = vec![(home.join(".profile"), posix.clone()), (home.join(".bashrc"), posix.clone())];
    if home.join(".zshrc").is_file() {
        targets.push((home.join(".zshrc"), posix));
    }
    let fish = home.join(".config/fish");
    if fish.is_dir() {
        targets.push((fish.join("conf.d/mandara-path.fish"), format!("{PATH_MARK}\nfish_add_path --path {dir}\n")));
    }
    let mut changed = Vec::new();
    for (file, line) in targets {
        if std::fs::read_to_string(&file).is_ok_and(|s| s.contains(PATH_MARK)) {
            continue;
        }
        if let Some(parent) = file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::OpenOptions::new().create(true).append(true).open(&file)?.write_all(line.as_bytes())?;
        changed.push(file);
    }
    Ok(changed)
}
