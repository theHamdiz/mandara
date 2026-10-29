//! System updates checker. Detects the package managers on `$PATH` and asks
//! each one, read-only, which packages have newer versions:
//!
//! | source  | check                                             | update command            |
//! |---------|---------------------------------------------------|---------------------------|
//! | pacman  | `checkupdates` (pacman-contrib, private sync db)  | `paru` / `yay` / `sudo pacman -Syu` |
//! | AUR     | `paru -Qua` / `yay -Qua`                          | `paru -Sua` / `yay -Sua`  |
//! | apt     | `apt list --upgradable` (never `apt update`)      | `sudo apt update && sudo apt upgrade` |
//! | dnf     | `dnf check-update` (exit 100 = updates)           | `sudo dnf upgrade`        |
//! | zypper  | `zypper -q list-updates`                          | `sudo zypper update`      |
//! | flatpak | `flatpak remote-ls --updates` + `flatpak list`    | `flatpak update`          |
//!
//! Without `checkupdates` the pacman source reports a hint instead of
//! touching the system sync database. The shell never runs an update
//! itself: [`open_terminal`] launches the user's terminal with the command
//! so they authenticate and watch it there. The parsers are pure functions
//! (fixture-tested); checks run with `LANG=C` and a timeout.

use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use mandara_i18n::{t, tf};
use serde::{Deserialize, Serialize};

/// Longest a single source's check may take (checkupdates downloads the
/// sync databases; AUR helpers query aur.archlinux.org).
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(120);
/// How often a running check polls its child for exit.
const POLL: Duration = Duration::from_millis(50);
/// Timeout for the Arch news feed request.
const NEWS_TIMEOUT: Duration = Duration::from_secs(15);
/// The Arch Linux news RSS feed.
pub const ARCH_NEWS_URL: &str = "https://archlinux.org/feeds/news/";
/// News items kept from the feed.
pub const NEWS_ITEMS: usize = 5;
/// `checkupdates` exit status meaning "no updates".
const CHECKUPDATES_NONE: i32 = 2;
/// `dnf check-update` exit status meaning "updates available".
const DNF_UPDATES: i32 = 100;
/// Terminals tried, in order, when none is configured.
pub const TERMINALS: [&str; 5] = ["ghostty", "kitty", "foot", "alacritty", "wezterm"];

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SourceKind {
    Pacman,
    Aur,
    Apt,
    Dnf,
    Zypper,
    Flatpak,
}

impl SourceKind {
    pub const ALL: [SourceKind; 6] = [SourceKind::Pacman, SourceKind::Aur, SourceKind::Apt, SourceKind::Dnf, SourceKind::Zypper, SourceKind::Flatpak];

    /// Command-line name (`updates run pacman`).
    pub fn id(self) -> &'static str {
        match self {
            SourceKind::Pacman => "pacman",
            SourceKind::Aur => "aur",
            SourceKind::Apt => "apt",
            SourceKind::Dnf => "dnf",
            SourceKind::Zypper => "zypper",
            SourceKind::Flatpak => "flatpak",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|k| k.id().eq_ignore_ascii_case(id))
    }

    /// Display name (proper nouns, not translated).
    pub fn label(self) -> &'static str {
        match self {
            SourceKind::Pacman => "Pacman",
            SourceKind::Aur => "AUR",
            SourceKind::Apt => "APT",
            SourceKind::Dnf => "DNF",
            SourceKind::Zypper => "Zypper",
            SourceKind::Flatpak => "Flatpak",
        }
    }
}

/// One upgradable package.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Package {
    pub name: String,
    /// Installed version (empty when the tool doesn't report it).
    pub old: String,
    /// Available version (empty when the tool doesn't report it).
    pub new: String,
    /// Repository, suite or Flatpak branch, when known.
    pub repo: Option<String>,
}

impl Package {
    fn new(name: &str, old: &str, new: &str, repo: Option<&str>) -> Self {
        Self { name: name.into(), old: old.into(), new: new.into(), repo: repo.filter(|r| !r.is_empty()).map(str::to_string) }
    }
}

/// A detected package source and the tool that checks it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub kind: SourceKind,
    /// The checking tool (`checkupdates`, `paru`, `apt`, …); `None` when
    /// the manager exists but its read-only checker is missing.
    pub tool: Option<String>,
    /// Shell command that installs this source's updates.
    pub command: String,
}

/// The result of checking one source.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceReport {
    pub kind: SourceKind,
    pub packages: Vec<Package>,
    /// The check failed (message for the user).
    pub error: Option<String>,
    /// The check can't run, with a fix (e.g. install pacman-contrib).
    pub hint: Option<String>,
    pub command: String,
}

/// Which optional sources to check.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CheckOptions {
    pub aur: bool,
    pub flatpak: bool,
}

impl Default for CheckOptions {
    fn default() -> Self {
        Self { aur: true, flatpak: true }
    }
}

/// A whole check.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckResult {
    pub sources: Vec<SourceReport>,
    /// Unix seconds when the check finished.
    pub checked_at: i64,
}

impl CheckResult {
    pub fn total(&self) -> usize {
        self.sources.iter().map(|s| s.packages.len()).sum()
    }
}

// ── Detection ────────────────────────────────────────────────────────────

/// Finds `name` on `$PATH` (an executable regular file).
pub fn find_program(name: &str) -> Option<PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join(name)).find(|p| p.metadata().is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0))
}

fn has(name: &str) -> bool {
    find_program(name).is_some()
}

/// The first AUR helper on `$PATH`.
pub fn aur_helper() -> Option<&'static str> {
    ["paru", "yay"].into_iter().find(|h| has(h))
}

/// Shell command that installs `kind`'s updates.
pub fn update_command(kind: SourceKind, helper: Option<&str>) -> String {
    match kind {
        SourceKind::Pacman => helper.map(str::to_string).unwrap_or_else(|| "sudo pacman -Syu".into()),
        SourceKind::Aur => format!("{} -Sua", helper.unwrap_or("paru")),
        SourceKind::Apt => "sudo apt update && sudo apt upgrade".into(),
        SourceKind::Dnf => "sudo dnf upgrade".into(),
        SourceKind::Zypper => "sudo zypper update".into(),
        SourceKind::Flatpak => "flatpak update".into(),
    }
}

/// One command for several sources: an AUR helper's plain invocation
/// already covers the official repositories and the AUR.
pub fn combined_command(kinds: &[SourceKind], helper: Option<&str>) -> String {
    let mut kinds: Vec<SourceKind> = kinds.to_vec();
    kinds.sort();
    kinds.dedup();
    if helper.is_some() && kinds.contains(&SourceKind::Pacman) {
        kinds.retain(|k| *k != SourceKind::Aur);
    }
    if helper.is_some() && kinds.contains(&SourceKind::Aur) {
        // AUR alone: the helper's full upgrade is the safer path (partial
        // upgrades are unsupported on Arch).
        kinds = kinds.into_iter().map(|k| if k == SourceKind::Aur { SourceKind::Pacman } else { k }).collect();
        kinds.dedup();
    }
    kinds.iter().map(|k| update_command(*k, helper)).collect::<Vec<_>>().join(" && ")
}

/// The package sources available on this system.
pub fn detect(opts: CheckOptions) -> Vec<Source> {
    let helper = aur_helper();
    let mut out = Vec::new();
    if has("pacman") {
        out.push(Source { kind: SourceKind::Pacman, tool: has("checkupdates").then(|| "checkupdates".into()), command: update_command(SourceKind::Pacman, helper) });
        if opts.aur
            && let Some(h) = helper
        {
            out.push(Source { kind: SourceKind::Aur, tool: Some(h.into()), command: update_command(SourceKind::Aur, helper) });
        }
    }
    if has("apt") && has("dpkg") {
        out.push(Source { kind: SourceKind::Apt, tool: Some("apt".into()), command: update_command(SourceKind::Apt, None) });
    }
    if let Some(dnf) = ["dnf5", "dnf"].into_iter().find(|d| has(d)) {
        out.push(Source { kind: SourceKind::Dnf, tool: Some(dnf.into()), command: update_command(SourceKind::Dnf, None) });
    }
    if has("zypper") {
        out.push(Source { kind: SourceKind::Zypper, tool: Some("zypper".into()), command: update_command(SourceKind::Zypper, None) });
    }
    if opts.flatpak && has("flatpak") {
        out.push(Source { kind: SourceKind::Flatpak, tool: Some("flatpak".into()), command: update_command(SourceKind::Flatpak, None) });
    }
    out
}

// ── Running checks ───────────────────────────────────────────────────────

/// Exit status (None = killed by a signal), stdout, stderr.
type Ran = (Option<i32>, String, String);

/// Runs `program args` with `LANG=C`, no stdin and a timeout.
fn run(program: &str, args: &[&str], timeout: Duration) -> Result<Ran, String> {
    let mut child = Command::new(program)
        .args(args)
        .env("LANG", "C")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| tf("Could not run {program}: {error}", &[("program", &program), ("error", &e)]))?;
    // Drain both pipes on threads so a chatty tool can't block on a full pipe.
    let drain = |pipe: Option<Box<dyn std::io::Read + Send>>| {
        std::thread::spawn(move || {
            let mut buf = Vec::new();
            if let Some(mut p) = pipe {
                let _ = p.read_to_end(&mut buf);
            }
            String::from_utf8_lossy(&buf).into_owned()
        })
    };
    let out = drain(child.stdout.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    let err = drain(child.stderr.take().map(|p| Box::new(p) as Box<dyn std::io::Read + Send>));
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= timeout => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(tf("{program} timed out", &[("program", &program)]));
            }
            Ok(None) => std::thread::sleep(POLL),
            Err(e) => return Err(e.to_string()),
        }
    };
    Ok((status.code(), out.join().unwrap_or_default(), err.join().unwrap_or_default()))
}

/// The last meaningful stderr line, for error messages.
fn last_line(stderr: &str) -> String {
    stderr.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or_default().to_string()
}

fn failed(program: &str, code: Option<i32>, stderr: &str) -> String {
    let line = last_line(stderr);
    match (code, line.is_empty()) {
        (_, false) => line,
        (Some(c), true) => tf("{program} exited with status {code}", &[("program", &program), ("code", &c)]),
        (None, true) => tf("{program} was interrupted", &[("program", &program)]),
    }
}

fn check_source(source: &Source) -> SourceReport {
    let mut report = SourceReport { kind: source.kind, packages: Vec::new(), error: None, hint: None, command: source.command.clone() };
    let Some(tool) = source.tool.as_deref() else {
        report.hint = Some(t("Install pacman-contrib to check for updates without touching the system database").into());
        return report;
    };
    let result: Result<Vec<Package>, String> = match source.kind {
        SourceKind::Pacman => run(tool, &[], CHECK_TIMEOUT).and_then(|(code, out, err)| match code {
            Some(0) => Ok(parse_arrow_list(&out)),
            Some(CHECKUPDATES_NONE) => Ok(Vec::new()),
            _ => Err(failed(tool, code, &err)),
        }),
        SourceKind::Aur => run(tool, &["-Qua"], CHECK_TIMEOUT).and_then(|(code, out, err)| {
            // Helpers exit 1 with empty output when nothing is outdated.
            let pkgs = parse_arrow_list(&out);
            if code == Some(0) || (code == Some(1) && last_line(&err).is_empty()) || !pkgs.is_empty() { Ok(pkgs) } else { Err(failed(tool, code, &err)) }
        }),
        SourceKind::Apt => run(tool, &["list", "--upgradable"], CHECK_TIMEOUT).and_then(|(code, out, err)| match code {
            Some(0) => Ok(parse_apt(&out)),
            _ => Err(failed(tool, code, &err)),
        }),
        SourceKind::Dnf => run(tool, &["-q", "check-update"], CHECK_TIMEOUT).and_then(|(code, out, err)| match code {
            Some(DNF_UPDATES) => Ok(parse_dnf(&out)),
            Some(0) => Ok(Vec::new()),
            _ => Err(failed(tool, code, &err)),
        }),
        SourceKind::Zypper => run(tool, &["-q", "--non-interactive", "list-updates"], CHECK_TIMEOUT).and_then(|(code, out, err)| match code {
            Some(0) => Ok(parse_zypper(&out)),
            _ => Err(failed(tool, code, &err)),
        }),
        SourceKind::Flatpak => {
            let cols = "--columns=application,version,branch";
            run(tool, &["remote-ls", "--updates", cols], CHECK_TIMEOUT).and_then(|(code, updates, err)| {
                if code != Some(0) {
                    return Err(failed(tool, code, &err));
                }
                let installed = run(tool, &["list", cols], CHECK_TIMEOUT).map(|(_, out, _)| out).unwrap_or_default();
                Ok(parse_flatpak(&updates, &installed))
            })
        }
    };
    match result {
        Ok(mut pkgs) => {
            pkgs.sort_by_key(|p| p.name.to_lowercase());
            report.packages = pkgs;
        }
        Err(e) => report.error = Some(e),
    }
    report
}

/// Checks every detected source (in parallel; blocking — call off the UI
/// thread).
pub fn check(opts: CheckOptions) -> CheckResult {
    let sources = detect(opts);
    let handles: Vec<_> = sources.into_iter().map(|s| std::thread::spawn(move || check_source(&s))).collect();
    let sources = handles.into_iter().filter_map(|h| h.join().ok()).collect();
    CheckResult { sources, checked_at: chrono::Local::now().timestamp() }
}

// ── Parsers ──────────────────────────────────────────────────────────────

/// `checkupdates`, `paru -Qua` and `yay -Qua`: `name old -> new` (a
/// trailing `[ignored]` marker is dropped).
pub fn parse_arrow_list(text: &str) -> Vec<Package> {
    text.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let (name, old, arrow, new) = (words.next()?, words.next()?, words.next()?, words.next()?);
            (arrow == "->").then(|| Package::new(name, old, new, None))
        })
        .collect()
}

/// `apt list --upgradable`:
/// `firefox/jammy-updates 120.0 amd64 [upgradable from: 119.0]`.
pub fn parse_apt(text: &str) -> Vec<Package> {
    text.lines()
        .filter_map(|line| {
            let (head, from) = line.split_once("[upgradable from:")?;
            let old = from.trim().trim_end_matches(']').trim();
            let mut words = head.split_whitespace();
            let (name_suite, new) = (words.next()?, words.next()?);
            let (name, suite) = name_suite.split_once('/').unwrap_or((name_suite, ""));
            let suite = suite.split(',').next().unwrap_or_default();
            Some(Package::new(name, old, new, Some(suite)))
        })
        .collect()
}

/// `dnf check-update`: `name.arch  version  repo` rows (a long name may
/// wrap its version onto the next line); stops at "Obsoleting Packages".
pub fn parse_dnf(text: &str) -> Vec<Package> {
    let mut out = Vec::new();
    let mut carry: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with("Obsoleting") || line.starts_with("Security:") {
            break;
        }
        if line.is_empty() || line.starts_with("Last metadata") {
            continue;
        }
        let mut words: Vec<&str> = line.split_whitespace().collect();
        let joined;
        if let Some(prev) = carry.take() {
            joined = format!("{prev} {line}");
            words = joined.split_whitespace().collect();
        }
        match words.as_slice() {
            [name] => carry = Some((*name).to_string()),
            [name_arch, version, repo, ..] if name_arch.contains('.') => {
                let name = name_arch.rsplit_once('.').map(|(n, _)| n).unwrap_or(name_arch);
                out.push(Package::new(name, "", version, Some(repo)));
            }
            _ => {}
        }
    }
    out
}

/// `zypper list-updates` table: `S | Repository | Name | Current Version |
/// Available Version | Arch` (columns located by header when present).
pub fn parse_zypper(text: &str) -> Vec<Package> {
    let mut cols = (1usize, 2usize, 3usize, 4usize); // repo, name, current, available
    let mut out = Vec::new();
    for line in text.lines() {
        if !line.contains('|') || line.trim_start().starts_with("--") {
            continue;
        }
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        if let (Some(r), Some(n), Some(c), Some(a)) = (
            cells.iter().position(|c| *c == "Repository"),
            cells.iter().position(|c| *c == "Name"),
            cells.iter().position(|c| *c == "Current Version"),
            cells.iter().position(|c| *c == "Available Version"),
        ) {
            cols = (r, n, c, a);
            continue;
        }
        let get = |i: usize| cells.get(i).copied().unwrap_or_default();
        let name = get(cols.1);
        if name.is_empty() {
            continue;
        }
        out.push(Package::new(name, get(cols.2), get(cols.3), Some(get(cols.0))));
    }
    out
}

/// `flatpak remote-ls --updates` and `flatpak list`, both with
/// `--columns=application,version,branch` (tab-separated). The installed
/// list supplies the old version; refs are matched by app and branch.
pub fn parse_flatpak(updates: &str, installed: &str) -> Vec<Package> {
    let rows = |text: &str| -> Vec<(String, String, String)> {
        text.lines()
            .filter(|l| !l.trim().is_empty())
            .map(|l| {
                let mut c = l.split('\t').map(str::trim);
                (c.next().unwrap_or_default().to_string(), c.next().unwrap_or_default().to_string(), c.next().unwrap_or_default().to_string())
            })
            // A header row only appears on a terminal, but be safe.
            .filter(|(app, _, _)| !app.is_empty() && app != "Application ID")
            .collect()
    };
    let have = rows(installed);
    rows(updates)
        .into_iter()
        .map(|(app, new, branch)| {
            let old = have.iter().find(|(a, _, b)| *a == app && *b == branch).map(|(_, v, _)| v.as_str()).unwrap_or_default();
            Package::new(&app, old, &new, Some(&branch))
        })
        .collect()
}

/// Splits `new` into the part shared with `old` and the changed rest, cut
/// at the last version separator before the first difference
/// (`1.2.3-1` → `1.2.4-1` gives `("1.2.", "4-1")`).
pub fn version_split<'a>(old: &str, new: &'a str) -> (&'a str, &'a str) {
    let common = old.char_indices().zip(new.chars()).take_while(|((_, a), b)| a == b).map(|((i, a), _)| i + a.len_utf8()).last().unwrap_or(0);
    if common >= new.len() && old.len() == new.len() {
        return (new, "");
    }
    let cut = new[..common].rfind(|c: char| ".-:+_~".contains(c)).map(|i| i + 1).unwrap_or(0);
    new.split_at(cut)
}

// ── Terminal ─────────────────────────────────────────────────────────────

/// Quotes `s` for `sh` (single quotes).
pub fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// The script run inside the terminal: the command, then a prompt so the
/// window stays open to read the result.
pub fn terminal_script(command: &str, prompt: &str) -> String {
    format!("{command}; printf '\\n%s ' {}; read -r _", sh_quote(prompt))
}

/// argv that runs `script` with `sh -c` in `terminal` (`ghostty`, a path,
/// or a command with its own flags such as `kitty --single-instance`).
pub fn terminal_argv(terminal: &str, script: &str) -> Vec<String> {
    let mut argv: Vec<String> = terminal.split_whitespace().map(str::to_string).collect();
    let base = argv.first().map(|p| Path::new(p).file_name().and_then(|n| n.to_str()).unwrap_or(p).to_string()).unwrap_or_default();
    let tail: &[&str] = match base.as_str() {
        "kitty" | "foot" => &[],
        "wezterm" => &["start", "--"],
        "konsole" | "xterm" | "alacritty" | "ghostty" | "gnome-terminal" => &["-e"],
        _ => &["-e"],
    };
    // gnome-terminal wants `--` rather than `-e` for argv-style commands.
    let tail: &[&str] = if base == "gnome-terminal" { &["--"] } else { tail };
    argv.extend(tail.iter().map(|s| s.to_string()));
    argv.extend(["sh".to_string(), "-c".to_string(), script.to_string()]);
    argv
}

/// The configured terminal, or the first known one on `$PATH`.
pub fn resolve_terminal(configured: &str) -> Option<String> {
    let configured = configured.trim();
    if !configured.is_empty() {
        return Some(configured.to_string());
    }
    TERMINALS.into_iter().find(|t| has(t)).map(str::to_string)
}

/// Opens `command` in a terminal, detached from the shell's process group.
/// Returns the terminal used.
pub fn open_terminal(configured: &str, command: &str) -> Result<String, String> {
    use std::os::unix::process::CommandExt as _;
    let terminal = resolve_terminal(configured).ok_or_else(|| t("No terminal found — set one in Studio → Plugins → Updates").to_string())?;
    let argv = terminal_argv(&terminal, &terminal_script(command, &t("Press Enter to close")));
    let child = Command::new(&argv[0])
        .args(&argv[1..])
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| tf("Could not start {terminal}: {error}", &[("terminal", &terminal), ("error", &e)]))?;
    // Reap it when it exits so it doesn't linger as a zombie.
    crate::reaper::adopt(child);
    Ok(terminal)
}

// ── Arch news ────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewsItem {
    pub title: String,
    pub link: String,
    /// Unix seconds, when the feed's date parses.
    pub published: Option<i64>,
}

fn decode_entities(s: &str) -> String {
    let s = s.trim();
    let s = s.strip_prefix("<![CDATA[").and_then(|x| x.strip_suffix("]]>")).unwrap_or(s);
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let entity = &rest[1..end];
        let ch = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            e if e.starts_with("#x") || e.starts_with("#X") => u32::from_str_radix(&e[2..], 16).ok().and_then(char::from_u32),
            e if e.starts_with('#') => e[1..].parse().ok().and_then(char::from_u32),
            _ => None,
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn tag<'a>(block: &'a str, name: &str) -> Option<&'a str> {
    let open = format!("<{name}>");
    let close = format!("</{name}>");
    let start = block.find(&open)? + open.len();
    let end = block[start..].find(&close)? + start;
    Some(&block[start..end])
}

/// Items of an RSS 2.0 feed (title, link, pubDate), newest first as served.
pub fn parse_rss(xml: &str) -> Vec<NewsItem> {
    xml.split("<item>")
        .skip(1)
        .filter_map(|chunk| {
            let block = chunk.split("</item>").next()?;
            let title = decode_entities(tag(block, "title")?);
            let link = tag(block, "link").map(decode_entities).unwrap_or_default();
            let published = tag(block, "pubDate").and_then(|d| chrono::DateTime::parse_from_rfc2822(d.trim()).ok()).map(|d| d.timestamp());
            (!title.is_empty()).then_some(NewsItem { title, link, published })
        })
        .take(NEWS_ITEMS)
        .collect()
}

/// Fetches the Arch Linux news feed (blocking).
pub fn fetch_news() -> Result<Vec<NewsItem>, String> {
    crate::offline::check()?;
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(NEWS_TIMEOUT)).build().into();
    let err = |e: &dyn std::fmt::Display| tf("Arch news request failed: {error}", &[("error", e)]);
    let mut response = agent.get(ARCH_NEWS_URL).header("User-Agent", concat!("mandara-shell/", env!("CARGO_PKG_VERSION"))).call().map_err(|e| err(&e))?;
    let body = response.body_mut().read_to_string().map_err(|e| err(&e))?;
    Ok(parse_rss(&body))
}

#[cfg(test)]
mod tests {
    use super::*;

    const CHECKUPDATES: &str = "brave-bin 1:1.97.59-1 -> 1:1.98.60-1\ngcc 16.2.1+r23+gd564253eb6c8-1 -> 16.2.1+r23+gd564253eb6c8-3\nlinux 6.17.1.arch1-1 -> 6.17.2.arch1-1\n";
    const PARU: &str = "cmus-git v2.12.0.r64.ge82cc1f-1 -> latest-commit\npiper-tts-bin 2023.11.14-1 -> 2023.11.14-2\nheld-pkg 1.0-1 -> 1.1-1 [ignored]\n:: warning noise\n";
    const APT: &str = "Listing... Done\nfirefox/jammy-updates,jammy-security 120.0+build2-0ubuntu0.22.04.1 amd64 [upgradable from: 119.0+build2-0ubuntu0.22.04.1]\nlibc6/jammy-updates 2.35-0ubuntu3.5 amd64 [upgradable from: 2.35-0ubuntu3.4]\n";
    const DNF: &str = "\nfirefox.x86_64                     120.0-1.fc39                 updates\nkernel.x86_64   6.6.2-201.fc39   updates\nsome-really-long-package-name-that-wraps.noarch\n                     2.0-1.fc39    fedora\n\nObsoleting Packages\ngrub2-tools.x86_64  1:2.06-100.fc39  updates\n";
    const ZYPPER: &str = "S | Repository        | Name     | Current Version | Available Version | Arch\n--+-------------------+----------+-----------------+-------------------+-------\nv | repo-oss          | bash     | 5.2.15-1.1      | 5.2.21-1.1        | x86_64\nv | repo-update       | vim      | 9.0.1-1         | 9.0.2-1           | x86_64\n";
    const FLATPAK_UPDATES: &str = "org.freedesktop.Platform.GL.default\t26.2.5\t26.08\norg.gnome.Platform\t\t50\ncom.spotify.Client\t1.2.50\tstable\n";
    const FLATPAK_LIST: &str = "com.spotify.Client\t1.2.42\tstable\norg.freedesktop.Platform.GL.default\t26.2.4\t26.08\norg.freedesktop.Platform.GL.default\t25.1\t25.08\n";
    const RSS: &str = r#"<?xml version="1.0" encoding="utf-8"?><rss version="2.0"><channel><title>Arch Linux: Recent news updates</title><link>https://archlinux.org/news/</link>
<item><title>Manual intervention for pacman &amp; keyring &#8212; 7.0</title><link>https://archlinux.org/news/manual-intervention/</link><description>&lt;p&gt;text&lt;/p&gt;</description><pubDate>Tue, 20 Oct 2026 12:00:00 +0000</pubDate></item>
<item><title><![CDATA[Valkey replaces Redis]]></title><link>https://archlinux.org/news/valkey/</link><pubDate>bad date</pubDate></item>
</channel></rss>"#;

    #[test]
    fn parses_checkupdates_and_aur_helpers() {
        let p = parse_arrow_list(CHECKUPDATES);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0], Package::new("brave-bin", "1:1.97.59-1", "1:1.98.60-1", None));
        let a = parse_arrow_list(PARU);
        assert_eq!(a.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["cmus-git", "piper-tts-bin", "held-pkg"]);
        assert_eq!(a[0].new, "latest-commit");
        assert_eq!(a[2].new, "1.1-1");
    }

    #[test]
    fn parses_apt() {
        let p = parse_apt(APT);
        assert_eq!(p.len(), 2);
        assert_eq!(p[0], Package::new("firefox", "119.0+build2-0ubuntu0.22.04.1", "120.0+build2-0ubuntu0.22.04.1", Some("jammy-updates")));
        assert_eq!(p[1].old, "2.35-0ubuntu3.4");
    }

    #[test]
    fn parses_dnf_with_wrapping_and_obsoletes() {
        let p = parse_dnf(DNF);
        assert_eq!(p.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["firefox", "kernel", "some-really-long-package-name-that-wraps"]);
        assert_eq!(p[2].new, "2.0-1.fc39");
        assert_eq!(p[2].repo.as_deref(), Some("fedora"));
        assert!(p[0].old.is_empty());
    }

    #[test]
    fn parses_zypper_table() {
        let p = parse_zypper(ZYPPER);
        assert_eq!(p, vec![Package::new("bash", "5.2.15-1.1", "5.2.21-1.1", Some("repo-oss")), Package::new("vim", "9.0.1-1", "9.0.2-1", Some("repo-update"))]);
    }

    #[test]
    fn parses_flatpak_matching_branches() {
        let p = parse_flatpak(FLATPAK_UPDATES, FLATPAK_LIST);
        assert_eq!(p.len(), 3);
        assert_eq!(p[0], Package::new("org.freedesktop.Platform.GL.default", "26.2.4", "26.2.5", Some("26.08")));
        assert_eq!(p[1], Package::new("org.gnome.Platform", "", "", Some("50")));
        assert_eq!(p[2].old, "1.2.42");
    }

    #[test]
    fn splits_versions_at_separators() {
        assert_eq!(version_split("1.2.3-1", "1.2.4-1"), ("1.2.", "4-1"));
        assert_eq!(version_split("1:1.97.59-1", "1:1.98.60-1"), ("1:1.", "98.60-1"));
        assert_eq!(version_split("16.2.1+r23-1", "16.2.1+r23-3"), ("16.2.1+r23-", "3"));
        assert_eq!(version_split("", "2.0"), ("", "2.0"));
        assert_eq!(version_split("v2.12", "latest-commit"), ("", "latest-commit"));
        assert_eq!(version_split("1.0", "1.0"), ("1.0", ""));
        assert_eq!(version_split("1.9", "1.10"), ("1.", "10"));
    }

    #[test]
    fn commands_and_combination() {
        assert_eq!(update_command(SourceKind::Pacman, Some("paru")), "paru");
        assert_eq!(update_command(SourceKind::Pacman, None), "sudo pacman -Syu");
        assert_eq!(update_command(SourceKind::Aur, Some("yay")), "yay -Sua");
        assert_eq!(combined_command(&[SourceKind::Aur, SourceKind::Pacman, SourceKind::Flatpak], Some("paru")), "paru && flatpak update");
        assert_eq!(combined_command(&[SourceKind::Aur], Some("paru")), "paru");
        assert_eq!(combined_command(&[SourceKind::Pacman, SourceKind::Flatpak], None), "sudo pacman -Syu && flatpak update");
        assert_eq!(SourceKind::from_id("FLATPAK"), Some(SourceKind::Flatpak));
    }

    #[test]
    fn terminal_argv_per_terminal() {
        let s = "paru";
        assert_eq!(terminal_argv("ghostty", s), ["ghostty", "-e", "sh", "-c", "paru"]);
        assert_eq!(terminal_argv("kitty", s), ["kitty", "sh", "-c", "paru"]);
        assert_eq!(terminal_argv("foot", s), ["foot", "sh", "-c", "paru"]);
        assert_eq!(terminal_argv("alacritty", s), ["alacritty", "-e", "sh", "-c", "paru"]);
        assert_eq!(terminal_argv("wezterm", s), ["wezterm", "start", "--", "sh", "-c", "paru"]);
        assert_eq!(terminal_argv("/usr/bin/kitty --single-instance", s), ["/usr/bin/kitty", "--single-instance", "sh", "-c", "paru"]);
        assert_eq!(terminal_argv("gnome-terminal", s), ["gnome-terminal", "--", "sh", "-c", "paru"]);
        assert_eq!(resolve_terminal("  foot "), Some("foot".into()));
    }

    #[test]
    fn terminal_script_quotes_prompt() {
        assert_eq!(sh_quote("it's"), r"'it'\''s'");
        let script = terminal_script("sudo pacman -Syu", "Press Enter to close");
        assert_eq!(script, "sudo pacman -Syu; printf '\\n%s ' 'Press Enter to close'; read -r _");
    }

    #[test]
    fn parses_rss_items() {
        let items = parse_rss(RSS);
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].title, "Manual intervention for pacman & keyring — 7.0");
        assert_eq!(items[0].link, "https://archlinux.org/news/manual-intervention/");
        assert_eq!(items[0].published, Some(1_792_497_600));
        assert_eq!(items[1].title, "Valkey replaces Redis");
        assert_eq!(items[1].published, None);
        assert_eq!(decode_entities("a & b &unknown; &#x41;"), "a & b &unknown; A");
    }

    #[test]
    fn finds_programs_on_path() {
        assert!(find_program("sh").is_some());
        assert!(find_program("definitely-not-a-real-program-mandara").is_none());
    }

    /// Live, read-only: runs the real checks on this machine.
    #[test]
    #[ignore]
    fn live_check() {
        let sources = detect(CheckOptions::default());
        println!("sources: {sources:#?}");
        let r = check(CheckOptions::default());
        for s in &r.sources {
            println!("{:?}: {} packages, error {:?}, hint {:?}", s.kind, s.packages.len(), s.error, s.hint);
            for p in s.packages.iter().take(3) {
                println!("  {} {} -> {}", p.name, p.old, p.new);
            }
        }
    }

    #[test]
    #[ignore]
    fn live_news() {
        let items = fetch_news().expect("news");
        println!("{items:#?}");
        assert!(!items.is_empty());
    }
}
