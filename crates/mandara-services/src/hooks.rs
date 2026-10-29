//! Event hooks: commands the user attached to shell events (`[[hooks.rules]]`).
//!
//! Safety rules:
//!
//! * **No shell.** A rule's command is split into words with POSIX-style
//!   quoting (`'…'`, `"…"`, `\`) and executed directly, so text that ends up
//!   in the environment (a wallpaper path with `$(…)` in its name, an SSID)
//!   is never interpreted. Users who want pipes write `sh -c '…'` themselves.
//! * **Event data travels only in the environment** (`MANDARA_EVENT`,
//!   `MANDARA_<KEY>`), never spliced into the command line.
//! * **Every hook has a deadline.** It runs in its own process group; on
//!   timeout the whole group gets `SIGTERM`, then `SIGKILL`.
//! * Output goes to `~/.local/state/mandara/hooks.log` (truncated when it
//!   grows past [`LOG_MAX_BYTES`]), never to a pipe that could fill up.
//!
//! Besides programs, a rule can run a Mandara command (`mandara:theme mode
//! dark`, via `mandara msg`) or a launcher skill (`skill:Focus`), whose
//! steps run in order.

use std::collections::VecDeque;
use std::io::Write as _;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime};

use mandara_config::power::{HookEvent, HooksConfig};
use mandara_config::Skill;
use parking_lot::Mutex;
use serde::Serialize;

/// Poll interval while waiting for a hook to exit.
const WAIT_POLL: Duration = Duration::from_millis(25);
/// After `SIGTERM` on timeout, `SIGKILL` follows this much later.
const KILL_GRACE: Duration = Duration::from_millis(1500);
/// Recent runs kept for `hooks status`.
const HISTORY_LEN: usize = 32;
/// The log is truncated once it grows past this.
const LOG_MAX_BYTES: u64 = 512 * 1024;
/// Prefixes selecting the non-program actions.
const MANDARA_PREFIX: &str = "mandara:";
const SKILL_PREFIX: &str = "skill:";
/// Environment variable prefix of event data.
const ENV_PREFIX: &str = "MANDARA_";

/// What a hook command does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HookAction {
    /// A program and its arguments.
    Exec(Vec<String>),
    /// `mandara msg <words>`.
    Mandara(Vec<String>),
    /// A launcher skill by name.
    Skill(String),
}

/// Splits `line` into words like a POSIX shell would, without any
/// expansion: single quotes are literal, double quotes allow `\"`, `\\`,
/// `\$` and `` \` `` escapes, a bare backslash escapes the next character.
pub fn split_words(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut cur = String::new();
    let mut in_word = false;
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        match c {
            '\'' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(c) => cur.push(c),
                        None => return Err(mandara_i18n::t("Unterminated single quote").into()),
                    }
                }
            }
            '"' => {
                in_word = true;
                loop {
                    match chars.next() {
                        Some('"') => break,
                        Some('\\') => match chars.next() {
                            Some(c @ ('"' | '\\' | '$' | '`')) => cur.push(c),
                            Some('\n') => {}
                            Some(c) => {
                                cur.push('\\');
                                cur.push(c);
                            }
                            None => return Err(mandara_i18n::t("Unterminated double quote").into()),
                        },
                        Some(c) => cur.push(c),
                        None => return Err(mandara_i18n::t("Unterminated double quote").into()),
                    }
                }
            }
            '\\' => {
                in_word = true;
                match chars.next() {
                    Some('\n') => {}
                    Some(c) => cur.push(c),
                    None => cur.push('\\'),
                }
            }
            c if c.is_whitespace() => {
                if in_word {
                    words.push(std::mem::take(&mut cur));
                    in_word = false;
                }
            }
            c => {
                in_word = true;
                cur.push(c);
            }
        }
    }
    if in_word {
        words.push(cur);
    }
    Ok(words)
}

/// Parses a rule's command.
pub fn parse_action(command: &str) -> Result<HookAction, String> {
    let command = command.trim();
    if let Some(rest) = command.strip_prefix(MANDARA_PREFIX) {
        let words = split_words(rest)?;
        return if words.is_empty() { Err(mandara_i18n::t("No Mandara command given").into()) } else { Ok(HookAction::Mandara(words)) };
    }
    if let Some(name) = command.strip_prefix(SKILL_PREFIX) {
        let name = name.trim();
        return if name.is_empty() { Err(mandara_i18n::t("No skill name given").into()) } else { Ok(HookAction::Skill(name.to_string())) };
    }
    let words = split_words(command)?;
    if words.is_empty() {
        return Err(mandara_i18n::t("The command is empty").into());
    }
    Ok(HookAction::Exec(words))
}

/// `wallpaper` → `MANDARA_WALLPAPER`; anything but ASCII alphanumerics
/// becomes `_`.
pub fn env_name(key: &str) -> String {
    let mut out = String::with_capacity(ENV_PREFIX.len() + key.len());
    out.push_str(ENV_PREFIX);
    out.extend(key.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_uppercase() } else { '_' }));
    out
}

/// One finished (or failed-to-start) hook.
#[derive(Clone, Debug, Serialize)]
pub struct HookRun {
    pub event: &'static str,
    pub command: String,
    /// Unix seconds when it started.
    pub started: u64,
    pub duration_ms: u64,
    /// Exit code; `None` when killed, timed out or never started.
    pub code: Option<i32>,
    pub timed_out: bool,
    pub error: Option<String>,
}

struct Registry {
    config: HooksConfig,
    skills: Vec<Skill>,
    history: VecDeque<HookRun>,
}

static REGISTRY: Mutex<Option<Registry>> = Mutex::new(None);

/// Installs (or replaces) the rules and the skills `skill:` refers to.
pub fn configure(config: &HooksConfig, skills: &[Skill]) {
    let mut reg = REGISTRY.lock();
    match reg.as_mut() {
        Some(r) => {
            r.config = config.clone();
            r.skills = skills.to_vec();
        }
        None => *reg = Some(Registry { config: config.clone(), skills: skills.to_vec(), history: VecDeque::new() }),
    }
}

/// The most recent runs, newest first.
pub fn history() -> Vec<HookRun> {
    REGISTRY.lock().as_ref().map(|r| r.history.iter().rev().cloned().collect()).unwrap_or_default()
}

/// Whether any enabled rule listens for `event`.
pub fn has_rules(event: HookEvent) -> bool {
    REGISTRY.lock().as_ref().is_some_and(|r| r.config.for_event(event).next().is_some())
}

fn record(run: HookRun) {
    if let Some(r) = REGISTRY.lock().as_mut() {
        if r.history.len() >= HISTORY_LEN {
            r.history.pop_front();
        }
        r.history.push_back(run);
    }
}

/// A planned hook invocation.
#[derive(Clone, Debug)]
struct Job {
    event: HookEvent,
    command: String,
    timeout: Duration,
    /// Programs to run in order (a skill expands to several).
    argvs: Result<Vec<Vec<String>>, String>,
}

/// `mandara msg <words>` as a program invocation.
fn mandara_argv(words: Vec<String>) -> Vec<String> {
    let mut argv = vec![mandara_config::paths::self_exe().to_string_lossy().into_owned(), "msg".into()];
    argv.extend(words);
    argv
}

/// The programs a command runs: one, or a skill's steps in order.
fn argvs_for(command: &str, skills: &[Skill]) -> Result<Vec<Vec<String>>, String> {
    match parse_action(command)? {
        HookAction::Exec(argv) => Ok(vec![argv]),
        HookAction::Mandara(words) => Ok(vec![mandara_argv(words)]),
        HookAction::Skill(name) => skills
            .iter()
            .find(|s| s.name.eq_ignore_ascii_case(&name))
            .map(|s| s.steps.iter().filter_map(|step| split_words(step).ok()).filter(|w| !w.is_empty()).map(mandara_argv).collect())
            .ok_or_else(|| mandara_i18n::tf("No skill named “{name}”", &[("name", &name)])),
    }
}

fn plan(event: HookEvent) -> Vec<Job> {
    let reg = REGISTRY.lock();
    let Some(reg) = reg.as_ref() else { return Vec::new() };
    reg.config
        .for_event(event)
        .map(|rule| {
            let argvs = argvs_for(&rule.command, &reg.skills);
            Job { event, command: rule.command.clone(), timeout: Duration::from_secs(u64::from(reg.config.timeout_of(rule))), argvs }
        })
        .collect()
}

/// Tests point the log elsewhere.
static LOG_OVERRIDE: Mutex<Option<PathBuf>> = Mutex::new(None);

/// `~/.local/state/mandara/hooks.log`.
pub fn log_path() -> PathBuf {
    if let Some(p) = LOG_OVERRIDE.lock().clone() {
        return p;
    }
    dirs::state_dir().unwrap_or_else(|| mandara_config::paths::cache_dir()).join("mandara").join("hooks.log")
}

fn log_file() -> Option<std::fs::File> {
    let path = log_path();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let truncate = std::fs::metadata(&path).is_ok_and(|m| m.len() > LOG_MAX_BYTES);
    std::fs::OpenOptions::new().create(true).append(!truncate).write(true).truncate(truncate).open(path).ok()
}

fn now_unix() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// Sends `signal` to the process group led by `pid`.
fn kill_group(pid: u32, signal: libc::c_int) {
    if let Ok(pid) = libc::pid_t::try_from(pid) {
        // SAFETY: kill(2) on a process group we created (the child called
        // setpgid(0, 0) through `process_group(0)`).
        unsafe { libc::kill(-pid, signal) };
    }
}

/// Runs one program to completion or until `deadline`.
fn run_argv(argv: &[String], vars: &[(String, String)], deadline: Instant) -> (Option<i32>, bool, Option<String>) {
    use std::os::unix::process::CommandExt as _;
    let Some((program, args)) = argv.split_first() else { return (None, false, Some(mandara_i18n::t("The command is empty").into())) };
    let mut cmd = Command::new(mandara_config::paths::expand_tilde(program));
    cmd.args(args).stdin(Stdio::null()).process_group(0);
    for (k, v) in vars {
        cmd.env(k, v);
    }
    match log_file() {
        Some(f) => {
            let _ = writeln!(&f, "[{}] {}", now_unix(), argv.join(" "));
            let err = f.try_clone().map(Stdio::from).unwrap_or_else(|_| Stdio::null());
            cmd.stdout(Stdio::from(f)).stderr(err);
        }
        None => {
            cmd.stdout(Stdio::null()).stderr(Stdio::null());
        }
    }
    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => return (None, false, Some(format!("{program}: {e}"))),
    };
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return (status.code(), false, None),
            Ok(None) if Instant::now() >= deadline => {
                kill_group(child.id(), libc::SIGTERM);
                let grace = Instant::now() + KILL_GRACE;
                while Instant::now() < grace {
                    if let Ok(Some(_)) = child.try_wait() {
                        return (None, true, None);
                    }
                    std::thread::sleep(WAIT_POLL);
                }
                kill_group(child.id(), libc::SIGKILL);
                let _ = child.wait();
                return (None, true, None);
            }
            Ok(None) => std::thread::sleep(WAIT_POLL),
            Err(e) => return (None, false, Some(e.to_string())),
        }
    }
}

fn execute(job: &Job, vars: &[(String, String)]) -> HookRun {
    let run = execute_untracked(job, vars);
    record(run.clone());
    run
}

fn execute_untracked(job: &Job, vars: &[(String, String)]) -> HookRun {
    let started = Instant::now();
    let mut run = HookRun { event: job.event.id(), command: job.command.clone(), started: now_unix(), duration_ms: 0, code: None, timed_out: false, error: None };
    match &job.argvs {
        Err(e) => run.error = Some(e.clone()),
        Ok(argvs) => {
            let deadline = started + job.timeout;
            for argv in argvs {
                let (code, timed_out, error) = run_argv(argv, vars, deadline);
                (run.code, run.timed_out, run.error) = (code, timed_out, error);
                if run.timed_out || run.error.is_some() || run.code != Some(0) {
                    break;
                }
            }
        }
    }
    run.duration_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    if run.error.is_some() || run.timed_out || run.code.is_some_and(|c| c != 0) {
        tracing::warn!("hook {} `{}`: code {:?}, timed out {}, {:?}", run.event, run.command, run.code, run.timed_out, run.error);
    }
    run
}

fn environment(event: HookEvent, vars: &[(&str, String)]) -> Vec<(String, String)> {
    let mut env = vec![(env_name("event"), event.id().to_string())];
    env.extend(vars.iter().map(|(k, v)| (env_name(k), v.clone())));
    env
}

/// Runs the hooks of `event` in the background. `vars` become
/// `MANDARA_<KEY>` environment variables. Returns how many were started.
pub fn emit(event: HookEvent, vars: &[(&str, String)]) -> usize {
    let jobs = plan(event);
    if jobs.is_empty() {
        return 0;
    }
    let env = environment(event, vars);
    let count = jobs.len();
    for job in jobs {
        let env = env.clone();
        let spawned = std::thread::Builder::new().name("mandara-hook".into()).spawn(move || {
            execute(&job, &env);
        });
        if let Err(e) = spawned {
            tracing::warn!("hook {}: {e}", event.id());
        }
    }
    count
}

/// Runs one command outside the rule list (idle behaviours, template
/// post-hooks) with the same safety rules, in the background. `label`
/// names it in `hooks status`.
pub fn run_command(label: &'static str, command: &str, vars: &[(&str, String)], timeout: Duration) {
    let argvs = {
        let reg = REGISTRY.lock();
        argvs_for(command, reg.as_ref().map(|r| r.skills.as_slice()).unwrap_or_default())
    };
    let mut env = vec![(env_name("event"), label.to_string())];
    env.extend(vars.iter().map(|(k, v)| (env_name(k), v.clone())));
    let job = Job { event: HookEvent::Started, command: command.to_string(), timeout, argvs };
    let spawned = std::thread::Builder::new().name("mandara-hook".into()).spawn(move || {
        let mut run = execute_untracked(&job, &env);
        run.event = label;
        record(run);
    });
    if let Err(e) = spawned {
        tracing::warn!("{label}: {e}");
    }
}

/// Runs one rule (its position in `[[hooks.rules]]`) now and waits for it,
/// even while it or hooks as a whole are switched off: Studio's per-rule
/// Test button. The rule must belong to `event`.
pub fn test_rule(event: HookEvent, index: usize, vars: &[(&str, String)]) -> Result<HookRun, String> {
    let job = {
        let reg = REGISTRY.lock();
        let reg = reg.as_ref().ok_or_else(|| mandara_i18n::t("Hooks aren't set up yet").to_string())?;
        let rule = reg.config.rules.get(index).filter(|r| r.event == event).ok_or_else(|| mandara_i18n::t("No such hook").to_string())?;
        Job { event, command: rule.command.clone(), timeout: Duration::from_secs(u64::from(reg.config.timeout_of(rule))), argvs: argvs_for(&rule.command, &reg.skills) }
    };
    let env = environment(event, vars);
    Ok(execute(&job, &env))
}

/// Runs the hooks of `event` and waits for them (each within its own
/// timeout, all in parallel) — for events that must finish first
/// (before sleep / logout / reboot / shutdown).
pub fn emit_and_wait(event: HookEvent, vars: &[(&str, String)]) -> Vec<HookRun> {
    run_and_wait(event, vars, plan(event))
}

/// [`emit_and_wait`] with every hook's time limit capped at `budget`, for
/// deadlines set by someone else (logind's sleep delay: a slow hook must
/// not hold the machine past it).
pub fn emit_and_wait_within(event: HookEvent, vars: &[(&str, String)], budget: Duration) -> Vec<HookRun> {
    let jobs = plan(event).into_iter().map(|mut job| {
        job.timeout = job.timeout.min(budget);
        job
    });
    run_and_wait(event, vars, jobs.collect())
}

fn run_and_wait(event: HookEvent, vars: &[(&str, String)], jobs: Vec<Job>) -> Vec<HookRun> {
    let env = environment(event, vars);
    let handles: Vec<_> = jobs
        .into_iter()
        .map(|job| {
            let env = env.clone();
            std::thread::spawn(move || execute(&job, &env))
        })
        .collect();
    handles.into_iter().filter_map(|h| h.join().ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::power::HookRule;

    #[test]
    fn words_split_like_a_shell_without_expansion() {
        assert_eq!(split_words("notify-send 'Hi there' \"it's $HOME\"").unwrap(), ["notify-send", "Hi there", "it's $HOME"]);
        assert_eq!(split_words(r#"a\ b "q\"x" '' c"#).unwrap(), ["a b", "q\"x", "", "c"]);
        assert_eq!(split_words("  x   y ").unwrap(), ["x", "y"]);
        assert_eq!(split_words("echo $(rm -rf ~)").unwrap(), ["echo", "$(rm", "-rf", "~)"]);
        assert!(split_words("echo 'oops").is_err());
        assert!(split_words("echo \"oops").is_err());
    }

    #[test]
    fn actions_parse() {
        assert_eq!(parse_action("mandara:theme mode dark").unwrap(), HookAction::Mandara(vec!["theme".into(), "mode".into(), "dark".into()]));
        assert_eq!(parse_action("skill: Focus").unwrap(), HookAction::Skill("Focus".into()));
        assert_eq!(parse_action("sh -c 'a | b'").unwrap(), HookAction::Exec(vec!["sh".into(), "-c".into(), "a | b".into()]));
        assert!(parse_action("   ").is_err());
        assert!(parse_action("mandara:").is_err());
        assert_eq!(env_name("battery-percent"), "MANDARA_BATTERY_PERCENT");
    }

    /// One test drives the global registry so parallel tests never race.
    #[test]
    fn hooks_run_with_env_and_time_out() {
        let dir = tempfile::tempdir().unwrap();
        *LOG_OVERRIDE.lock() = Some(dir.path().join("hooks.log"));
        let out = dir.path().join("out.txt");
        let script = format!("printf '%s|%s' \"$MANDARA_EVENT\" \"$MANDARA_WALLPAPER\" > '{}'", out.display());
        let mut cfg = HooksConfig::default();
        cfg.rules.push(HookRule { event: HookEvent::WallpaperChanged, command: format!("sh -c {}", shell_quote(&script)), ..Default::default() });
        cfg.rules.push(HookRule { event: HookEvent::BeforeSleep, command: "sleep 30".into(), timeout_s: 1, ..Default::default() });
        cfg.rules.push(HookRule { event: HookEvent::Locked, command: "skill:nope".into(), ..Default::default() });
        cfg.rules.push(HookRule { event: HookEvent::BeforeLogout, command: "sleep 30".into(), timeout_s: 60, ..Default::default() });
        configure(&cfg, &[]);
        // A budget shorter than the rule's own limit wins.
        let started = Instant::now();
        let runs = emit_and_wait_within(HookEvent::BeforeLogout, &[], Duration::from_millis(300));
        assert!(runs.len() == 1 && runs[0].timed_out, "{runs:?}");
        assert!(started.elapsed() < Duration::from_secs(10), "capped at the budget, not 60 s");
        // A wallpaper name that would be dangerous in a shell stays data.
        let runs = emit_and_wait(HookEvent::WallpaperChanged, &[("wallpaper", "/tmp/$(touch pwned).png".into())]);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].code, Some(0), "{:?}", runs[0]);
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "wallpaper-changed|/tmp/$(touch pwned).png");
        let started = Instant::now();
        let runs = emit_and_wait(HookEvent::BeforeSleep, &[]);
        assert!(runs[0].timed_out, "{:?}", runs[0]);
        assert!(started.elapsed() < Duration::from_secs(10));
        let runs = emit_and_wait(HookEvent::Locked, &[]);
        assert!(runs[0].error.as_deref().is_some_and(|e| e.contains("nope")));
        // One rule by position, even with hooks switched off; never a rule
        // of another event.
        let mut off = cfg.clone();
        off.enabled = false;
        configure(&off, &[]);
        assert!(emit_and_wait(HookEvent::WallpaperChanged, &[]).is_empty(), "off: nothing runs on events");
        std::fs::remove_file(&out).ok();
        let run = test_rule(HookEvent::WallpaperChanged, 0, &[("wallpaper", "w.png".into())]).unwrap();
        assert_eq!(run.code, Some(0), "{run:?}");
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "wallpaper-changed|w.png");
        assert!(test_rule(HookEvent::Locked, 0, &[]).is_err(), "rule 0 is a wallpaper hook");
        assert!(test_rule(HookEvent::Locked, 99, &[]).is_err());
        configure(&cfg, &[]);
        assert_eq!(emit(HookEvent::Unlocked, &[]), 0);
        assert!(history().len() >= 3);
    }

    fn shell_quote(s: &str) -> String {
        format!("'{}'", s.replace('\'', "'\\''"))
    }
}
