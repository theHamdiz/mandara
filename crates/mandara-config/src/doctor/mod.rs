//! Config doctor: one check for every compositor's config, read the way the
//! compositor itself reads it.
//!
//! Compositors change their config languages between releases: an option is
//! renamed, a value type tightens, a section moves. Instead of guessing,
//! the doctor runs each compositor's **own** checker on the user's real
//! config tree, read-only and sandboxed (`mango -p -c`, `niri validate -c`,
//! `Hyprland --verify-config -c`, `umbriel config validate -c --json`,
//! `sway --validate --verbose --config`; River has no checker, so its
//! version and `init` are checked instead; see [`CompositorCheck`]), and
//! turns its output into structured
//! [`Diagnostic`]s:
//!
//! - **where**: file, line, column (relative names and includes resolved
//!   against the config tree; a message without a location is placed by
//!   searching the tree for the option it names),
//! - **whose**: Mandara's generated drop-in ([`Owner::Mandara`]) or the
//!   user's own files ([`Owner::User`]),
//! - **what**: a [`DiagKind`] with a plain-language [`explain`]ation and the
//!   offending line.
//!
//! Fixes ([`fix`]) are all reversible: a timestamped backup first, an atomic
//! write, a re-check, and an automatic rollback when the result is worse.
//! [`state`] caches reports by the config files' sizes and mtimes and the
//! compositor's version, remembers snoozes and what was already announced,
//! and keeps the small "unsupported on this version" list that stops Mandara
//! from emitting an option the installed compositor rejects.
//!
//! The checker runs with a private, empty `XDG_RUNTIME_DIR` (so it can't
//! reach a running compositor), no Wayland or X display, standard input
//! closed, a [`CHECK_TIMEOUT`] and no other environment than it needs.

pub mod fix;
pub mod state;

mod hyprland;
mod mango;
mod niri;
mod river;
mod kwin;
mod aqueous;
mod sway;
pub(crate) use sway::has_error as sway_has_error;
mod umbriel;

#[cfg(test)]
mod tests;

use std::collections::BTreeSet;
use std::ffi::OsString;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};

use crate::compositor::{Target, included_files, is_generated};

/// A checker that takes longer than this is stopped and reported as hung.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(10);
/// How often a running checker is polled for its exit.
const POLL: Duration = Duration::from_millis(10);
/// Raw checker output kept in a report (for "Details" and bug reports).
const RAW_LIMIT: usize = 16 * 1024;
/// Largest config file the doctor reads lines from.
const MAX_FILE_BYTES: u64 = 4 << 20;
/// Hex digits of a diagnostic id.
const ID_DIGITS: usize = 8;
/// Placeholder for the config path in an [`CheckEnv::argv`] override.
pub const CONFIG_PLACEHOLDER: &str = "{config}";
/// What `sway --validate` needs: sway sets up a backend before it reads the
/// config, so it gets a headless one with the software renderer and no
/// input devices (it never touches a display, seat or GPU).
pub const SWAY_CHECK_ENV: &[(&str, &str)] = &[("WLR_BACKENDS", "headless"), ("WLR_RENDERER", "pixman"), ("WLR_LIBINPUT_NO_DEVICES", "1")];

#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Severity {
    Error,
    Warning,
    Info,
}

/// Whose file a problem is in.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Owner {
    /// Mandara's generated drop-in (`~/.config/mandara/generated/…`).
    Mandara,
    /// The user's own config files.
    User,
}

/// What kind of problem the compositor reported.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum DiagKind {
    /// An option, keyword, section or node the compositor doesn't know.
    UnknownOption,
    /// An option that worked with the previously installed version and is
    /// rejected since the compositor was updated (renamed or removed).
    RemovedOption,
    /// Something Mandara wrote that this compositor version rejects.
    UnsupportedByVersion,
    /// Still accepted, but on its way out.
    Deprecated,
    /// The line can't be parsed at all.
    SyntaxError,
    /// A known option with a value the compositor doesn't accept.
    InvalidValue,
    /// The same key combination bound more than once.
    BindConflict,
    /// An include / source line names a file that isn't there.
    MissingInclude,
    /// Anything else (the raw message is kept).
    Other,
}

impl DiagKind {
    /// English; display with `tr()`.
    pub fn label(self) -> &'static str {
        match self {
            DiagKind::UnknownOption => mandara_i18n::tk("Unknown option"),
            DiagKind::RemovedOption => mandara_i18n::tk("Removed in this version"),
            DiagKind::UnsupportedByVersion => mandara_i18n::tk("Not supported by this version"),
            DiagKind::Deprecated => mandara_i18n::tk("Deprecated"),
            DiagKind::SyntaxError => mandara_i18n::tk("Syntax error"),
            DiagKind::InvalidValue => mandara_i18n::tk("Invalid value"),
            DiagKind::BindConflict => mandara_i18n::tk("Duplicate keybind"),
            DiagKind::MissingInclude => mandara_i18n::tk("Missing file"),
            DiagKind::Other => mandara_i18n::tk("Problem"),
        }
    }
}

/// A place in a config file (1-based line and column).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Location {
    pub file: PathBuf,
    pub line: usize,
    pub column: Option<usize>,
}

/// One problem the compositor's checker reported.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Diagnostic {
    /// Stable short id (compositor, file, line and message), for `fix <id>`.
    pub id: String,
    pub severity: Severity,
    pub file: Option<PathBuf>,
    /// 1-based.
    pub line: Option<usize>,
    /// 1-based.
    pub column: Option<usize>,
    /// The checker's own message, cleaned of colours and decoration.
    pub message: String,
    pub kind: DiagKind,
    pub owner: Owner,
    /// The option / keyword / node the problem is about, when known.
    pub key: Option<String>,
    /// The offending line as it is in the file (trimmed).
    pub text: Option<String>,
    /// Other places involved (the first definition of a duplicate bind).
    pub related: Vec<Location>,
    /// The checker named no line; it was found by searching for `key`.
    pub guessed: bool,
    /// In a file Mandara never edits (River's `init` program): no comment
    /// or remove fixes, only the explanation.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub read_only: bool,
}

impl Diagnostic {
    /// A diagnostic with only a message (parsers fill in the rest).
    pub fn new(severity: Severity, message: impl Into<String>) -> Self {
        Self { id: String::new(), severity, file: None, line: None, column: None, message: message.into(), kind: DiagKind::Other, owner: Owner::User, key: None, text: None, related: Vec::new(), guessed: false, read_only: false }
    }

    pub fn at(mut self, file: impl Into<PathBuf>, line: usize, column: Option<usize>) -> Self {
        self.file = Some(file.into());
        self.line = Some(line);
        self.column = column;
        self
    }

    pub fn kind(mut self, kind: DiagKind) -> Self {
        self.kind = kind;
        self
    }

    pub fn key(mut self, key: impl Into<String>) -> Self {
        self.key = Some(key.into()).filter(|k: &String| !k.is_empty());
        self
    }

    /// `file:line` for messages, when it has a place.
    pub fn place(&self) -> Option<String> {
        let file = self.file.as_ref()?;
        Some(match self.line {
            Some(l) => format!("{}:{l}", file.display()),
            None => file.display().to_string(),
        })
    }
}

/// How running the checker went.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", content = "detail", rename_all = "kebab-case")]
pub enum CheckerStatus {
    /// The checker ran; its findings are the diagnostics.
    Ran,
    /// The compositor (or its checker) isn't installed.
    NotInstalled,
    /// It didn't finish within [`CHECK_TIMEOUT`] and was stopped.
    TimedOut,
    /// It couldn't be started or crashed.
    Failed(String),
    /// There is no config file to check.
    NoConfig,
}

/// One run of the doctor for one compositor.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub target: Target,
    /// The checker command (`mango -p -c`, …).
    pub checker: String,
    /// The installed compositor's version (`0.18.4`), when it reports one.
    pub version: Option<String>,
    pub status: CheckerStatus,
    /// Whether the output parser was checked against the real checker
    /// (Hyprland's is written from its source code: no binary to test with).
    pub verified_parser: bool,
    pub root: PathBuf,
    /// Every file of the config tree, the drop-in included.
    pub files: Vec<PathBuf>,
    pub diagnostics: Vec<Diagnostic>,
    /// Unix seconds.
    pub checked_at: u64,
    pub duration_ms: u64,
    /// The checker's output (trimmed, bounded).
    pub raw: String,
}

impl Report {
    pub fn errors(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.severity == Severity::Error).count()
    }

    pub fn warnings(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.severity == Severity::Warning).count()
    }

    /// Errors and warnings (deprecations and notes aside).
    pub fn problems(&self) -> usize {
        self.diagnostics.iter().filter(|d| d.severity != Severity::Info).count()
    }

    /// The checker ran and found nothing to fix.
    pub fn is_clean(&self) -> bool {
        self.status == CheckerStatus::Ran && self.problems() == 0
    }

    pub fn find(&self, id: &str) -> Option<&Diagnostic> {
        self.diagnostics.iter().find(|d| d.id == id)
    }

    /// "Errors, then warnings, then the rest" for comparing two runs: a
    /// fix that raises this made things worse.
    pub fn badness(&self) -> (usize, usize, usize) {
        (self.errors(), self.warnings(), self.diagnostics.len())
    }

    /// The report as JSON for `mandara msg` and agents: every diagnostic
    /// with its plain-language explanation and the fixes it offers.
    pub fn to_json(&self) -> serde_json::Value {
        let mut v = serde_json::to_value(self).unwrap_or_default();
        if let Some(list) = v.get_mut("diagnostics").and_then(serde_json::Value::as_array_mut) {
            for (json, d) in list.iter_mut().zip(&self.diagnostics) {
                json["explanation"] = explain(d, self.target, self.version.as_deref()).into();
                json["fixes"] = fixes_for(d).iter().map(|f| f.id()).collect::<Vec<_>>().into();
            }
        }
        v["errors"] = self.errors().into();
        v["warnings"] = self.warnings().into();
        v
    }

    /// Diagnostics by file (files in tree order; unplaced last).
    pub fn by_file(&self) -> Vec<(Option<PathBuf>, Vec<&Diagnostic>)> {
        let mut order: Vec<Option<PathBuf>> = Vec::new();
        for d in &self.diagnostics {
            if !order.contains(&d.file) {
                order.push(d.file.clone());
            }
        }
        let rank = |f: &Option<PathBuf>| f.as_ref().map_or(usize::MAX, |f| self.files.iter().position(|x| x == f).unwrap_or(usize::MAX - 1));
        order.sort_by_key(rank);
        order.into_iter().map(|f| {
            let list = self.diagnostics.iter().filter(|d| d.file == f).collect();
            (f, list)
        }).collect()
    }
}

/// What the checker printed.
#[derive(Clone, Debug, Default)]
pub struct Output {
    pub stdout: String,
    pub stderr: String,
    pub success: bool,
}

impl Output {
    /// stdout then stderr, colours removed.
    pub fn text(&self) -> String {
        strip_ansi(&format!("{}\n{}", self.stdout, self.stderr))
    }
}

/// The config tree a checker's output refers to.
#[derive(Clone, Debug)]
pub struct ParseContext {
    pub target: Target,
    pub root: PathBuf,
    pub files: Vec<PathBuf>,
}

impl ParseContext {
    pub fn new(target: Target, root: &Path) -> Self {
        Self { target, root: root.to_path_buf(), files: config_tree(target, root) }
    }

    /// The file a checker means by `name`: absolute as is; relative to the
    /// root's folder (Niri names included files relative to it); else the
    /// tree file it is a suffix of, else the one with that file name.
    pub fn resolve(&self, name: &str) -> PathBuf {
        let name = name.trim().trim_matches('"');
        let p = Path::new(name);
        if p.is_absolute() {
            return normalize(p);
        }
        // Lua shortens long chunk names to `...tail`.
        let tail = name.trim_start_matches("...");
        let dir = self.root.parent().unwrap_or(Path::new("/"));
        let joined = normalize(&dir.join(tail));
        if tail == name && (joined.exists() || self.files.contains(&joined)) {
            return joined;
        }
        if let Some(f) = self.files.iter().find(|f| f.to_string_lossy().ends_with(tail) && !tail.is_empty()) {
            return f.clone();
        }
        // The only tree file with that name (never a guess between two).
        let file_name = Path::new(tail).file_name();
        let named: Vec<&PathBuf> = self.files.iter().filter(|f| f.file_name() == file_name).collect();
        match named.as_slice() {
            [only] => (*only).clone(),
            _ => joined,
        }
    }
}

/// How to run a checker: who it runs as, and (tests, sandboxes) which
/// program.
#[derive(Clone, Debug)]
pub struct CheckEnv {
    /// `HOME` for the checker; `None` keeps the user's (needed for `~/`
    /// in include lines).
    pub home: Option<PathBuf>,
    /// The whole command line instead of the compositor's own, with
    /// [`CONFIG_PLACEHOLDER`] for the config path (a locally built checker).
    pub argv: Option<Vec<OsString>>,
    /// Version to report instead of asking the program.
    pub version: Option<String>,
    pub timeout: Duration,
}

impl Default for CheckEnv {
    fn default() -> Self {
        Self { home: None, argv: None, version: None, timeout: CHECK_TIMEOUT }
    }
}

/// One compositor's own config checker: how to run it and read its output.
pub trait CompositorCheck: Sync {
    fn target(&self) -> Target;
    /// The program on `PATH`.
    fn program(&self) -> &'static str;
    /// Arguments that check `config` without starting anything.
    fn args(&self, config: &Path) -> Vec<OsString>;
    /// Availability probes can run before the compositor creates a config.
    fn needs_config(&self) -> bool {
        true
    }
    /// The flag that prints the version.
    fn version_flag(&self) -> &'static str {
        "--version"
    }
    /// Extra environment the checker needs (on top of the private runtime
    /// dir; see [`SWAY_CHECK_ENV`]).
    fn env(&self) -> &'static [(&'static str, &'static str)] {
        &[]
    }
    /// The part of the checker's output worth keeping in a report (Sway's
    /// verbose log also prints the environment and kernel).
    fn clean_raw(&self, text: &str) -> String {
        text.to_string()
    }
    /// Diagnostics in the checker's output (locations as printed; the
    /// doctor resolves and classifies them afterwards).
    fn parse(&self, out: &Output, ctx: &ParseContext) -> Vec<Diagnostic>;
    /// Whether the checker considers the config broken, beyond what was
    /// parsed (an exit status; MangoWM's isn't reliable).
    fn rejected(&self, out: &Output) -> bool {
        !out.success
    }
    /// The option a config line sets (`blur=1` → `blur`).
    fn line_key(&self, line: &str) -> Option<String>;
    /// The parser was checked against the real program's output.
    fn verified(&self) -> bool {
        true
    }
}

/// The checker for `target`.
pub fn checker(target: Target) -> &'static dyn CompositorCheck {
    match target {
        Target::Mango => &mango::Mango,
        Target::Niri => &niri::Niri,
        Target::Hyprland => &hyprland::Hyprland,
        Target::Umbriel => &umbriel::Umbriel,
        Target::Sway => &sway::Sway,
        Target::River => &river::River,
        Target::KWin => &kwin::KWin,
        Target::Aqueous => &aqueous::Aqueous,
    }
}

/// Every file of the config tree under `root`, includes followed
/// recursively (Mandara's drop-in too), in discovery order.
pub fn config_tree(target: Target, root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let mut stack = vec![normalize(root)];
    while let Some(file) = stack.pop() {
        if !seen.insert(file.clone()) || !file.is_file() {
            continue;
        }
        let text = read_small(&file).unwrap_or_default();
        let mut children: Vec<PathBuf> = included_files(target, &file, &text).into_iter().map(|p| normalize(&p)).collect();
        children.reverse();
        out.push(file);
        stack.extend(children);
    }
    out
}

/// `path` with `.` and `..` resolved lexically (`a/./b.conf` → `a/b.conf`),
/// so the same file always has the same name.
pub fn normalize(path: &Path) -> PathBuf {
    use std::path::Component;
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

/// Reads a config file unless it is suspiciously large.
pub(crate) fn read_small(path: &Path) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    (meta.len() <= MAX_FILE_BYTES).then(|| std::fs::read_to_string(path).ok()).flatten()
}

/// Checks the user's real config for `target` (blocking: call it off the
/// UI thread).
pub fn check(target: Target) -> Report {
    check_with(target, &target.user_config(), &CheckEnv::default())
}

/// [`check`] of any config tree.
pub fn check_with(target: Target, root: &Path, env: &CheckEnv) -> Report {
    let started = Instant::now();
    let check = checker(target);
    let ctx = ParseContext::new(target, root);
    let mut report = Report {
        target,
        checker: command_line(check, env),
        version: None,
        status: CheckerStatus::Ran,
        verified_parser: check.verified(),
        root: root.to_path_buf(),
        files: ctx.files.clone(),
        diagnostics: Vec::new(),
        checked_at: now_secs(),
        duration_ms: 0,
        raw: String::new(),
    };
    if check.needs_config() && !root.is_file() {
        report.status = CheckerStatus::NoConfig;
        return report;
    }
    report.version = env.version.clone().or_else(|| probe_version(check, env));
    let argv = match &env.argv {
        Some(argv) => argv.iter().map(|a| if a == CONFIG_PLACEHOLDER { root.as_os_str().to_owned() } else { a.clone() }).collect(),
        None => std::iter::once(OsString::from(check.program())).chain(check.args(root)).collect::<Vec<_>>(),
    };
    let cwd = root.parent().filter(|p| p.is_dir()).unwrap_or(Path::new("/"));
    match run(&argv, cwd, env, check.env()) {
        Err(RunError::NotFound) => report.status = CheckerStatus::NotInstalled,
        Err(RunError::TimedOut) => report.status = CheckerStatus::TimedOut,
        Err(RunError::Other(e)) => report.status = CheckerStatus::Failed(e),
        Ok(out) => {
            let mut raw = check.clean_raw(&out.text()).trim().to_string();
            if raw.len() > RAW_LIMIT {
                let mut cut = RAW_LIMIT;
                while !raw.is_char_boundary(cut) {
                    cut -= 1;
                }
                raw.truncate(cut);
            }
            report.raw = raw;
            let mut diags = check.parse(&out, &ctx);
            if diags.is_empty() && check.rejected(&out) {
                // An output format this parser doesn't know: keep the text
                // (the end of the whole output when the kept part is empty,
                // as Sway's is without an `[ERROR]` line).
                let text = if report.raw.trim().is_empty() {
                    let full = out.text();
                    let lines: Vec<&str> = full.lines().filter(|l| !l.trim().is_empty()).collect();
                    lines[lines.len().saturating_sub(MAX_RAW_LINES)..].join("\n")
                } else {
                    report.raw.lines().filter(|l| !l.trim().is_empty()).take(MAX_RAW_LINES).collect::<Vec<_>>().join("\n")
                };
                diags.push(Diagnostic::new(Severity::Error, if text.is_empty() { t("The checker rejected the config without saying why.").to_string() } else { text }));
            }
            report.diagnostics = finish(target, diags, &ctx, check);
        }
    }
    report.duration_ms = started.elapsed().as_millis().try_into().unwrap_or(u64::MAX);
    report
}

/// Lines of unparsed output kept as one diagnostic's message.
const MAX_RAW_LINES: usize = 12;

fn command_line(check: &dyn CompositorCheck, env: &CheckEnv) -> String {
    match &env.argv {
        Some(argv) => argv.iter().map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" "),
        None => std::iter::once(check.program().to_string()).chain(check.args(Path::new(CONFIG_PLACEHOLDER)).into_iter().map(|a| a.to_string_lossy().into_owned())).collect::<Vec<_>>().join(" "),
    }
}

pub(crate) fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or_default()
}

/// The installed version as the program prints it (`mango 0.18.4(release)`
/// → `0.18.4`).
fn probe_version(check: &dyn CompositorCheck, env: &CheckEnv) -> Option<String> {
    let program = env.argv.as_ref().and_then(|a| a.first().cloned()).unwrap_or_else(|| check.program().into());
    let out = run(&[program, check.version_flag().into()], Path::new("/"), env, &[]).ok()?;
    version_in(&out.text())
}

/// The first `1.2.3`-like word of a version banner.
pub fn version_in(text: &str) -> Option<String> {
    text.split_whitespace().find_map(|w| {
        let w = w.trim_start_matches('v');
        let v: String = w.chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        let v = v.trim_end_matches('.');
        (v.contains('.') && v.starts_with(|c: char| c.is_ascii_digit())).then(|| v.to_string())
    })
}

/// The installed compositor's version (`None` when it isn't installed),
/// asked once per process.
pub fn installed_version(target: Target) -> Option<String> {
    static CACHE: [std::sync::OnceLock<Option<String>>; Target::ALL.len()] = [const { std::sync::OnceLock::new() }; Target::ALL.len()];
    CACHE[target.index()].get_or_init(|| probe_version(checker(target), &CheckEnv::default())).clone()
}

enum RunError {
    NotFound,
    TimedOut,
    Other(String),
}

/// Runs `argv` read-only: stdin closed, a private empty runtime dir, no
/// display, a timeout; both pipes drained on their own threads so a chatty
/// checker never blocks. `extra` is the checker's own environment.
fn run(argv: &[OsString], cwd: &Path, env: &CheckEnv, extra: &[(&str, &str)]) -> Result<Output, RunError> {
    use std::process::{Command, Stdio};
    let (program, args) = argv.split_first().ok_or(RunError::NotFound)?;
    let runtime = PrivateDir::new().map_err(|e| RunError::Other(e.to_string()))?;
    let mut cmd = Command::new(program);
    cmd.args(args).current_dir(cwd).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).env_clear();
    for var in ["PATH", "HOME", "XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_DATA_DIRS", "XDG_CONFIG_DIRS", "LANG", "LC_ALL", "USER", "LOGNAME"] {
        if let Some(v) = std::env::var_os(var) {
            cmd.env(var, v);
        }
    }
    if let Some(home) = &env.home {
        cmd.env("HOME", home);
    }
    // Plain output: no colours, and Niri's narrated (line-exact) reports.
    cmd.env("XDG_RUNTIME_DIR", runtime.path()).env("NO_COLOR", "1").env("NO_GRAPHICS", "1").env("TERM", "dumb").envs(extra.iter().copied());
    let mut child = cmd.spawn().map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { RunError::NotFound } else { RunError::Other(e.to_string()) })?;
    let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            String::from_utf8_lossy(&buf).into_owned()
        })
    };
    let out_thread = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    let err_thread = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    let deadline = Instant::now() + env.timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(RunError::TimedOut);
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(e) => return Err(RunError::Other(e.to_string())),
        }
    };
    let stdout = out_thread.join().unwrap_or_default();
    let stderr = err_thread.join().unwrap_or_default();
    Ok(Output { stdout, stderr, success: status.success() })
}

/// Runs `argv` the way checkers run (no display, a private runtime dir,
/// [`CHECK_TIMEOUT`], `extra` environment): its output, or `None` when it
/// couldn't be started, set up or finished in time.
pub(crate) fn run_isolated(argv: &[OsString], extra: &[(&str, &str)]) -> Option<Output> {
    run(argv, Path::new("/"), &CheckEnv::default(), extra).ok()
}

/// A private `0700` folder, removed again when dropped.
pub(crate) struct PrivateDir(PathBuf);

impl PrivateDir {
    pub(crate) fn new() -> std::io::Result<Self> {
        use std::os::unix::fs::DirBuilderExt as _;
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or_default();
        let dir = std::env::temp_dir().join(format!("mandara-doctor-{}-{nanos}", std::process::id()));
        std::fs::DirBuilder::new().mode(0o700).create(&dir)?;
        Ok(Self(dir))
    }

    pub(crate) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for PrivateDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Removes ANSI colour / cursor sequences.
pub fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                // Parameters, then one final byte in `@`..=`~`.
                for c in chars.by_ref() {
                    if ('@'..='~').contains(&c) {
                        break;
                    }
                }
            }
            continue;
        }
        out.push(c);
    }
    out
}

/// Whether a config line is a comment in `target`'s dialect.
pub(crate) fn is_comment_line(target: Target, line: &str) -> bool {
    let t = line.trim_start();
    match target {
        Target::Niri => t.starts_with("//") || t.starts_with("/-"),
        Target::Hyprland => t.starts_with('#') || t.starts_with("--"),
        _ => t.starts_with('#'),
    }
}

/// Resolves, places, attributes and classifies parsed diagnostics.
fn finish(target: Target, diags: Vec<Diagnostic>, ctx: &ParseContext, check: &dyn CompositorCheck) -> Vec<Diagnostic> {
    let mut cache: Vec<(PathBuf, Vec<String>)> = Vec::new();
    let mut lines_of = |file: &Path| -> Vec<String> {
        if let Some((_, lines)) = cache.iter().find(|(f, _)| f == file) {
            return lines.clone();
        }
        let lines: Vec<String> = read_small(file).unwrap_or_default().lines().map(str::to_string).collect();
        cache.push((file.to_path_buf(), lines.clone()));
        lines
    };
    let mut out: Vec<Diagnostic> = Vec::new();
    for mut d in diags {
        // A message without a place: find the line naming its key.
        if d.file.is_none()
            && let Some(key) = d.key.clone()
        {
            for file in &ctx.files {
                let lines = lines_of(file);
                if let Some(i) = lines.iter().position(|l| !is_comment_line(target, l) && contains_word(l, &key)) {
                    d.file = Some(file.clone());
                    d.line = Some(i + 1);
                    d.guessed = true;
                    break;
                }
            }
        }
        if let (Some(file), Some(line)) = (d.file.clone(), d.line) {
            let lines = lines_of(&file);
            if d.text.is_none() {
                d.text = lines.get(line.saturating_sub(1)).map(|l| l.trim().to_string()).filter(|l| !l.is_empty());
            }
            if d.key.is_none() {
                d.key = d.text.as_deref().and_then(|l| check.line_key(l));
            }
        }
        d.owner = match &d.file {
            Some(f) if is_generated(f) => Owner::Mandara,
            Some(_) => Owner::User,
            None if d.message.contains("/mandara/generated/") => Owner::Mandara,
            None => Owner::User,
        };
        if d.owner == Owner::Mandara && matches!(d.kind, DiagKind::UnknownOption | DiagKind::InvalidValue) {
            d.kind = DiagKind::UnsupportedByVersion;
        }
        d.id = diagnostic_id(target, &d);
        if !out.iter().any(|o| o.id == d.id) {
            out.push(d);
        }
    }
    out
}

/// `word` appears in `line` as a whole word (option names, not substrings
/// of other options).
fn contains_word(line: &str, word: &str) -> bool {
    let is_word = |c: char| c.is_alphanumeric() || c == '_' || c == '-' || c == '.';
    line.match_indices(word).any(|(at, _)| {
        let before = line[..at].chars().next_back();
        let after = line[at + word.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
}

/// FNV-1a over the compositor, file, line and message.
fn diagnostic_id(target: Target, d: &Diagnostic) -> String {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let key = format!("{}|{}|{}|{}", target.name(), d.file.as_ref().map(|f| f.display().to_string()).unwrap_or_default(), d.line.unwrap_or_default(), d.message);
    let hash = key.bytes().fold(OFFSET, |h, b| (h ^ u64::from(b)).wrapping_mul(PRIME));
    format!("{hash:016x}")[..ID_DIGITS].to_string()
}

/// What a diagnostic means, in plain words, for the person reading it.
pub fn explain(d: &Diagnostic, target: Target, version: Option<&str>) -> String {
    let compositor = target.name();
    let key = d.key.as_deref().unwrap_or("?");
    let version = version.unwrap_or("?");
    match d.kind {
        DiagKind::UnknownOption => tf("{compositor} doesn't know `{key}`. It may be misspelled, or it was renamed or removed in a newer version.", &[("compositor", &compositor), ("key", &key)]),
        DiagKind::RemovedOption => tf("`{key}` worked with your previous version, but {compositor} {version} no longer accepts it: it was probably renamed or removed in this update.", &[("compositor", &compositor), ("key", &key), ("version", &version)]),
        DiagKind::UnsupportedByVersion => tf("Mandara wrote `{key}`, which {compositor} {version} doesn't support. Regenerating stops Mandara from writing it for this version.", &[("compositor", &compositor), ("key", &key), ("version", &version)]),
        DiagKind::Deprecated => tf("`{key}` still works, but {compositor} has deprecated it and a future version may drop it.", &[("compositor", &compositor), ("key", &key)]),
        DiagKind::SyntaxError => tf("{compositor} can't read this line: something in its syntax is broken (a missing bracket, quote or `=`), so it may skip everything after it.", &[("compositor", &compositor)]),
        DiagKind::InvalidValue => tf("{compositor} knows `{key}`, but not the value it is given here.", &[("compositor", &compositor), ("key", &key)]),
        DiagKind::BindConflict => tf("This key combination is bound more than once; {compositor} uses only one of the binds.", &[("compositor", &compositor)]),
        DiagKind::MissingInclude => tf("This line includes a file that isn't there, so {compositor} can't load it.", &[("compositor", &compositor)]),
        // Mandara's own findings (River has no checker) explain themselves.
        DiagKind::Other if matches!(target, Target::River | Target::KWin | Target::Aqueous) => d.message.clone(),
        DiagKind::Other => tf("{compositor} reported a problem here.", &[("compositor", &compositor)]),
    }
}

/// The fixes that make sense for a diagnostic, best first.
pub fn fixes_for(d: &Diagnostic) -> Vec<fix::Fix> {
    use fix::Fix;
    let mut out = Vec::new();
    if d.read_only {
        return out;
    }
    match d.owner {
        Owner::Mandara => out.push(Fix::Regenerate),
        Owner::User if d.file.is_some() && d.line.is_some() && d.text.is_some() => {
            out.push(Fix::Comment);
            out.push(Fix::Remove);
        }
        Owner::User => {}
    }
    out
}

