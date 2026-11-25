//! systemd unit manager over D-Bus (`org.freedesktop.systemd1`), on both
//! the user bus (your session's units) and the system bus.
//!
//! Reads (unit lists, failed units, unit-file states) are plain method
//! calls. Mutations (start / stop / restart / enable / disable) are built as
//! pure [`UnitCall`] plans and run by [`execute`] through a [`Caller`], so the
//! request construction is unit-tested with a recording mock and the real
//! [`BusCaller`] stays a thin shim. System-bus mutations carry the
//! `ALLOW_INTERACTIVE_AUTHORIZATION` flag: polkit then asks the session's
//! authentication agent (Mandara's own dialog) for a password instead of
//! refusing outright.
//!
//! Journal lines come from `journalctl -o json` run as a subprocess. Reading
//! the journal natively would need `sd-journal`, i.e. libsystemd through
//! FFI; journalctl is the standard, always-present system tool for it and
//! its JSON export format is stable. Unit names are validated (and never
//! start with `-`) before they reach journalctl or D-Bus.
//!
//! Every function here blocks; callers run them off the UI thread.

use std::collections::HashMap;

use mandara_i18n::{t, tf};
use serde::Serialize;
use zbus::zvariant::OwnedObjectPath;

const DEST: &str = "org.freedesktop.systemd1";
const PATH: &str = "/org/freedesktop/systemd1";
const MANAGER: &str = "org.freedesktop.systemd1.Manager";
/// Job mode for start / stop / restart: replace conflicting queued jobs.
const JOB_MODE: &str = "replace";
/// Longest unit name systemd accepts.
const UNIT_NAME_MAX: usize = 256;
/// Upper bound for journal lines requested at once.
pub const JOURNAL_MAX: usize = 1000;
/// Unit type suffixes; a bare name gets [`DEFAULT_SUFFIX`].
const UNIT_SUFFIXES: [&str; 11] =
    [".service", ".socket", ".target", ".timer", ".mount", ".automount", ".path", ".slice", ".scope", ".device", ".swap"];
const DEFAULT_SUFFIX: &str = ".service";

/// Which systemd instance.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    User,
    System,
}

impl Scope {
    pub const ALL: [Scope; 2] = [Scope::System, Scope::User];

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "user" | "--user" => Some(Scope::User),
            "system" | "--system" => Some(Scope::System),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Scope::User => "user",
            Scope::System => "system",
        }
    }

    async fn connect(self) -> zbus::Result<zbus::Connection> {
        match self {
            Scope::User => crate::runtime::session().await,
            Scope::System => crate::runtime::system().await,
        }
    }
}

/// One loaded unit (a `ListUnits` row).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Unit {
    pub name: String,
    pub description: String,
    /// `loaded`, `not-found`, `masked`, `error`…
    pub load_state: String,
    /// `active`, `inactive`, `failed`, `activating`, `deactivating`, `reloading`.
    pub active_state: String,
    /// Type-specific detail: `running`, `exited`, `dead`, `waiting`…
    pub sub_state: String,
    /// Unit-file state when known: `enabled`, `disabled`, `static`, `masked`…
    pub file_state: Option<String>,
}

/// Coarse state for colouring and filtering.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Health {
    Active,
    Failed,
    Transitioning,
    Inactive,
}

impl Unit {
    pub fn health(&self) -> Health {
        match self.active_state.as_str() {
            "active" | "reloading" | "refreshing" => Health::Active,
            "failed" => Health::Failed,
            "activating" | "deactivating" | "maintenance" => Health::Transitioning,
            _ => Health::Inactive,
        }
    }

    /// `foo.service` → `service`.
    pub fn kind(&self) -> &str {
        self.name.rsplit_once('.').map(|(_, k)| k).unwrap_or("")
    }

    /// The name without its `.service` suffix (other types keep theirs).
    pub fn short_name(&self) -> &str {
        self.name.strip_suffix(DEFAULT_SUFFIX).unwrap_or(&self.name)
    }

    pub fn is_enabled(&self) -> bool {
        matches!(self.file_state.as_deref(), Some("enabled" | "enabled-runtime" | "alias"))
    }
}

/// `ListUnits` row: name, description, load, active, sub, following, path,
/// job id, job type, job path.
type UnitRow = (String, String, String, String, String, String, OwnedObjectPath, u32, String, OwnedObjectPath);

fn from_row(r: UnitRow) -> Unit {
    Unit { name: r.0, description: r.1, load_state: r.2, active_state: r.3, sub_state: r.4, file_state: None }
}

/// Validates a unit name and appends `.service` when it has no type suffix
/// (`sshd` → `sshd.service`). Rejects anything that could be read as an
/// option or that systemd would never accept.
pub fn normalize_unit(name: &str) -> Result<String, String> {
    let name = name.trim();
    let bad = || tf("Not a valid unit name: {name}", &[("name", &name)]);
    if name.is_empty() || name.len() > UNIT_NAME_MAX || name.starts_with('-') || name.starts_with('.') {
        return Err(bad());
    }
    if !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, ':' | '-' | '_' | '.' | '\\' | '@')) {
        return Err(bad());
    }
    if UNIT_SUFFIXES.iter().any(|s| name.ends_with(s) && name.len() > s.len()) {
        Ok(name.to_string())
    } else {
        Ok(format!("{name}{DEFAULT_SUFFIX}"))
    }
}

/// A human sentence for a D-Bus error from systemd / polkit.
pub fn describe_error(error_name: &str, message: &str) -> String {
    match error_name {
        "org.freedesktop.DBus.Error.AccessDenied" | "org.freedesktop.systemd1.AccessDenied" => t("Authorization cancelled or denied").into(),
        "org.freedesktop.DBus.Error.InteractiveAuthorizationRequired" | "org.freedesktop.systemd1.InteractiveAuthorizationRequired" => {
            t("Administrator rights needed, but no authentication agent answered").into()
        }
        "org.freedesktop.systemd1.NoSuchUnit" | "org.freedesktop.systemd1.LoadFailed" => t("No such unit").into(),
        "org.freedesktop.systemd1.UnitMasked" => t("The unit is masked").into(),
        "org.freedesktop.DBus.Error.ServiceUnknown" | "org.freedesktop.DBus.Error.NoServer" => t("systemd is not reachable on this bus").into(),
        _ if message.is_empty() => error_name.to_string(),
        _ => message.to_string(),
    }
}

fn zbus_error(e: zbus::Error) -> String {
    match &e {
        zbus::Error::MethodError(name, detail, _) => describe_error(name.as_str(), detail.as_deref().unwrap_or("")),
        zbus::Error::FDO(fdo) => match fdo.as_ref() {
            zbus::fdo::Error::AccessDenied(m) => describe_error("org.freedesktop.DBus.Error.AccessDenied", m),
            zbus::fdo::Error::InteractiveAuthorizationRequired(m) => describe_error("org.freedesktop.DBus.Error.InteractiveAuthorizationRequired", m),
            zbus::fdo::Error::ServiceUnknown(m) => describe_error("org.freedesktop.DBus.Error.ServiceUnknown", m),
            other => other.to_string(),
        },
        _ => e.to_string(),
    }
}

async fn manager(conn: &zbus::Connection) -> zbus::Result<zbus::Proxy<'static>> {
    crate::actor::proxy(conn, DEST, PATH, MANAGER).await
}

// ── Reads ────────────────────────────────────────────────────────────────

async fn list_async(scope: Scope, with_files: bool) -> zbus::Result<Vec<Unit>> {
    let conn = scope.connect().await?;
    let m = manager(&conn).await?;
    let rows: Vec<UnitRow> = m.call("ListUnits", &()).await?;
    let mut units: Vec<Unit> = rows.into_iter().map(from_row).collect();
    if with_files {
        // One call for every unit file's state (enabled / disabled / static…).
        let files: Vec<(String, String)> = m.call("ListUnitFiles", &()).await.unwrap_or_default();
        let states: HashMap<&str, &str> =
            files.iter().filter_map(|(path, state)| Some((path.rsplit('/').next()?, state.as_str()))).collect();
        for u in &mut units {
            u.file_state = states.get(u.name.as_str()).map(|s| s.to_string());
        }
    }
    Ok(units)
}

/// Every loaded unit of `scope`, with unit-file states.
pub fn list_units(scope: Scope) -> Result<Vec<Unit>, String> {
    crate::runtime::block_on(list_async(scope, true)).map_err(zbus_error)
}

/// Units of `scope` in the `failed` state.
pub fn failed_units(scope: Scope) -> Result<Vec<Unit>, String> {
    crate::runtime::block_on(async move {
        let conn = scope.connect().await?;
        let m = manager(&conn).await?;
        let rows: Vec<UnitRow> = m.call("ListUnitsFiltered", &(vec!["failed"],)).await?;
        Ok::<_, zbus::Error>(rows.into_iter().map(from_row).collect())
    })
    .map_err(zbus_error)
}

/// Current state of one unit (loaded or not), with its unit-file state.
pub fn unit_status(scope: Scope, name: &str) -> Result<Unit, String> {
    let name = normalize_unit(name)?;
    crate::runtime::block_on(async move {
        let conn = scope.connect().await?;
        let m = manager(&conn).await?;
        let rows: Vec<UnitRow> = m.call("ListUnitsByNames", &(vec![name.as_str()],)).await?;
        let mut unit = rows.into_iter().next().map(from_row).unwrap_or(Unit {
            name: name.clone(),
            description: String::new(),
            load_state: "not-found".into(),
            active_state: "inactive".into(),
            sub_state: "dead".into(),
            file_state: None,
        });
        unit.file_state = m.call::<_, _, String>("GetUnitFileState", &(name.as_str(),)).await.ok();
        Ok::<_, zbus::Error>(unit)
    })
    .map_err(zbus_error)
}

/// Unit-file state (`enabled`, `disabled`, `static`, `masked`…).
pub fn unit_file_state(scope: Scope, name: &str) -> Result<String, String> {
    let name = normalize_unit(name)?;
    crate::runtime::block_on(async move {
        let conn = scope.connect().await?;
        manager(&conn).await?.call::<_, _, String>("GetUnitFileState", &(name.as_str(),)).await
    })
    .map_err(zbus_error)
}

// ── Mutations ────────────────────────────────────────────────────────────

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum UnitAction {
    Start,
    Stop,
    Restart,
    Enable,
    Disable,
}

impl UnitAction {
    pub const ALL: [UnitAction; 5] = [UnitAction::Start, UnitAction::Stop, UnitAction::Restart, UnitAction::Enable, UnitAction::Disable];

    pub fn parse(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|a| a.as_str() == s)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            UnitAction::Start => "start",
            UnitAction::Stop => "stop",
            UnitAction::Restart => "restart",
            UnitAction::Enable => "enable",
            UnitAction::Disable => "disable",
        }
    }
}

/// Arguments of one Manager method call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CallArgs {
    /// `(unit, mode)` for StartUnit / StopUnit / RestartUnit.
    UnitMode { unit: String, mode: &'static str },
    /// `(files, runtime, force)` for EnableUnitFiles, `(files, runtime)`
    /// for DisableUnitFiles (`force: None`).
    Files { files: Vec<String>, runtime: bool, force: Option<bool> },
    /// No arguments (Reload).
    Empty,
}

/// One planned Manager method call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UnitCall {
    pub scope: Scope,
    pub method: &'static str,
    pub args: CallArgs,
    /// Sets `ALLOW_INTERACTIVE_AUTHORIZATION` (system bus).
    pub interactive: bool,
}

/// The Manager calls that carry out `action` on `unit` (already normalized).
/// Enabling / disabling persists the symlinks, then reloads the manager so
/// the change is picked up, exactly like `systemctl enable`.
pub fn plan(action: UnitAction, scope: Scope, unit: &str) -> Vec<UnitCall> {
    let interactive = scope == Scope::System;
    let call = |method, args| UnitCall { scope, method, args, interactive };
    let unit_mode = || CallArgs::UnitMode { unit: unit.to_string(), mode: JOB_MODE };
    match action {
        UnitAction::Start => vec![call("StartUnit", unit_mode())],
        UnitAction::Stop => vec![call("StopUnit", unit_mode())],
        UnitAction::Restart => vec![call("RestartUnit", unit_mode())],
        UnitAction::Enable => vec![
            call("EnableUnitFiles", CallArgs::Files { files: vec![unit.to_string()], runtime: false, force: Some(false) }),
            call("Reload", CallArgs::Empty),
        ],
        UnitAction::Disable => vec![
            call("DisableUnitFiles", CallArgs::Files { files: vec![unit.to_string()], runtime: false, force: None }),
            call("Reload", CallArgs::Empty),
        ],
    }
}

/// Something that can perform a [`UnitCall`] (the bus, or a test mock).
pub trait Caller {
    fn call(&mut self, call: &UnitCall) -> Result<(), String>;
}

/// Validates `unit`, plans `action` and runs the calls in order, stopping at
/// the first failure. Returns the normalized unit name.
pub fn execute<C: Caller>(caller: &mut C, action: UnitAction, scope: Scope, unit: &str) -> Result<String, String> {
    let unit = normalize_unit(unit)?;
    for call in plan(action, scope, &unit) {
        caller.call(&call)?;
    }
    Ok(unit)
}

/// The real [`Caller`]: one connection per bus, opened lazily.
#[derive(Default)]
pub struct BusCaller {
    conns: HashMap<Scope, zbus::Connection>,
}

async fn invoke<B, R>(proxy: &zbus::Proxy<'_>, method: &'static str, interactive: bool, body: &B) -> zbus::Result<()>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
    R: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
{
    if interactive {
        proxy.call_with_flags::<_, B, R>(method, zbus::proxy::MethodFlags::AllowInteractiveAuth.into(), body).await.map(|_| ())
    } else {
        proxy.call::<_, B, R>(method, body).await.map(|_| ())
    }
}

impl Caller for BusCaller {
    fn call(&mut self, call: &UnitCall) -> Result<(), String> {
        crate::runtime::block_on(async {
            let conn = match self.conns.get(&call.scope) {
                Some(c) => c.clone(),
                None => {
                    let c = call.scope.connect().await?;
                    self.conns.insert(call.scope, c.clone());
                    c
                }
            };
            let m = manager(&conn).await?;
            match &call.args {
                CallArgs::UnitMode { unit, mode } => invoke::<_, OwnedObjectPath>(&m, call.method, call.interactive, &(unit.as_str(), *mode)).await,
                CallArgs::Files { files, runtime, force: Some(force) } => {
                    invoke::<_, (bool, Vec<(String, String, String)>)>(&m, call.method, call.interactive, &(files, *runtime, *force)).await
                }
                CallArgs::Files { files, runtime, force: None } => {
                    invoke::<_, Vec<(String, String, String)>>(&m, call.method, call.interactive, &(files, *runtime)).await
                }
                CallArgs::Empty => invoke::<_, ()>(&m, call.method, call.interactive, &()).await,
            }
        })
        .map_err(zbus_error)
    }
}

/// Runs `action` on the bus (may show the polkit dialog for system units).
pub fn act(action: UnitAction, scope: Scope, unit: &str) -> Result<String, String> {
    execute(&mut BusCaller::default(), action, scope, unit)
}

// ── Journal ──────────────────────────────────────────────────────────────

/// One journal entry.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct JournalLine {
    /// Microseconds since the Unix epoch.
    pub timestamp_us: Option<u64>,
    /// Syslog priority, 0 (emerg) … 7 (debug); 6 (info) when absent.
    pub priority: u8,
    pub message: String,
    pub identifier: Option<String>,
    pub pid: Option<u32>,
}

/// Default priority (info) for entries without `PRIORITY`.
const PRIORITY_INFO: u8 = 6;

fn json_text(v: &serde_json::Value) -> Option<String> {
    match v {
        serde_json::Value::String(s) => Some(s.clone()),
        // Non-UTF-8 / binary fields are exported as byte arrays.
        serde_json::Value::Array(bytes) => {
            let raw: Vec<u8> = bytes.iter().filter_map(|b| b.as_u64().and_then(|n| u8::try_from(n).ok())).collect();
            Some(String::from_utf8_lossy(&raw).into_owned())
        }
        _ => None,
    }
}

/// Parses `journalctl -o json` output (one JSON object per line); lines that
/// are not JSON objects are skipped.
pub fn parse_journal(text: &str) -> Vec<JournalLine> {
    text.lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(line.trim()).ok())
        .map(|o| {
            let field = |k: &str| o.get(k).and_then(json_text);
            JournalLine {
                timestamp_us: field("__REALTIME_TIMESTAMP").and_then(|s| s.parse().ok()),
                priority: field("PRIORITY").and_then(|s| s.parse().ok()).unwrap_or(PRIORITY_INFO),
                message: field("MESSAGE").unwrap_or_default().trim_end().to_string(),
                identifier: field("SYSLOG_IDENTIFIER"),
                pid: field("_PID").and_then(|s| s.parse().ok()),
            }
        })
        .collect()
}

/// The arguments for journalctl (unit already normalized).
pub fn journal_args(scope: Scope, unit: &str, lines: usize) -> Vec<String> {
    let mut args = Vec::new();
    if scope == Scope::User {
        args.push("--user".to_string());
    }
    args.extend(["-u".to_string(), unit.to_string(), "-n".to_string(), lines.clamp(1, JOURNAL_MAX).to_string()]);
    args.extend(["-o", "json", "--no-pager", "-q"].map(String::from));
    args
}

/// The last `lines` journal entries of `unit`, oldest first.
pub fn journal(scope: Scope, unit: &str, lines: usize) -> Result<Vec<JournalLine>, String> {
    let unit = normalize_unit(unit)?;
    let out = std::process::Command::new("journalctl")
        .args(journal_args(scope, &unit, lines))
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| tf("Could not run journalctl: {error}", &[("error", &e)]))?;
    let text = String::from_utf8_lossy(&out.stdout);
    let entries = parse_journal(&text);
    if entries.is_empty() && !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
        return Err(if err.is_empty() { t("journalctl failed").into() } else { err });
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Recorder {
        calls: Vec<UnitCall>,
        fail_on: Option<&'static str>,
    }

    impl Caller for Recorder {
        fn call(&mut self, call: &UnitCall) -> Result<(), String> {
            self.calls.push(call.clone());
            match self.fail_on {
                Some(m) if m == call.method => Err("Access denied".into()),
                _ => Ok(()),
            }
        }
    }

    #[test]
    fn unit_names_are_validated_and_normalized() {
        assert_eq!(normalize_unit("sshd").unwrap(), "sshd.service");
        assert_eq!(normalize_unit(" docker.socket ").unwrap(), "docker.socket");
        assert_eq!(normalize_unit("getty@tty1.service").unwrap(), "getty@tty1.service");
        assert_eq!(normalize_unit("dev-disk-by\\x2duuid-1.device").unwrap(), "dev-disk-by\\x2duuid-1.device");
        assert_eq!(normalize_unit("foo.bar").unwrap(), "foo.bar.service");
        for bad in ["", "-u", "--user", "a b", "x;rm", "a\nb", ".service", "ü.service", &"a".repeat(300)] {
            assert!(normalize_unit(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn start_stop_restart_plans() {
        for (action, method) in [(UnitAction::Start, "StartUnit"), (UnitAction::Stop, "StopUnit"), (UnitAction::Restart, "RestartUnit")] {
            let p = plan(action, Scope::System, "nginx.service");
            assert_eq!(p, vec![UnitCall { scope: Scope::System, method, args: CallArgs::UnitMode { unit: "nginx.service".into(), mode: "replace" }, interactive: true }]);
        }
        // User units need no authorization.
        assert!(!plan(UnitAction::Start, Scope::User, "x.service")[0].interactive);
    }

    #[test]
    fn enable_disable_persist_then_reload() {
        let mut r = Recorder::default();
        assert_eq!(execute(&mut r, UnitAction::Enable, Scope::User, "syncthing").unwrap(), "syncthing.service");
        assert_eq!(r.calls.iter().map(|c| c.method).collect::<Vec<_>>(), ["EnableUnitFiles", "Reload"]);
        assert_eq!(r.calls[0].args, CallArgs::Files { files: vec!["syncthing.service".into()], runtime: false, force: Some(false) });
        assert_eq!(r.calls[1].args, CallArgs::Empty);
        assert!(r.calls.iter().all(|c| c.scope == Scope::User && !c.interactive));

        let mut r = Recorder::default();
        execute(&mut r, UnitAction::Disable, Scope::System, "bluetooth.service").unwrap();
        assert_eq!(r.calls[0].method, "DisableUnitFiles");
        assert_eq!(r.calls[0].args, CallArgs::Files { files: vec!["bluetooth.service".into()], runtime: false, force: None });
        assert!(r.calls.iter().all(|c| c.interactive));
    }

    #[test]
    fn execution_stops_at_first_failure_and_rejects_bad_names() {
        let mut r = Recorder { fail_on: Some("EnableUnitFiles"), ..Default::default() };
        assert!(execute(&mut r, UnitAction::Enable, Scope::System, "x").is_err());
        assert_eq!(r.calls.len(), 1, "Reload must not run after a failed enable");
        let mut r = Recorder::default();
        assert!(execute(&mut r, UnitAction::Stop, Scope::System, "--force").is_err());
        assert!(r.calls.is_empty());
    }

    #[test]
    fn errors_read_like_sentences() {
        assert!(describe_error("org.freedesktop.DBus.Error.AccessDenied", "x").contains("cancelled"));
        assert_eq!(describe_error("org.freedesktop.systemd1.NoSuchUnit", "Unit x not found."), "No such unit");
        assert_eq!(describe_error("com.example.Other", "boom"), "boom");
        assert_eq!(describe_error("com.example.Other", ""), "com.example.Other");
    }

    #[test]
    fn journal_json_is_parsed() {
        let fixture = concat!(
            r#"{"__REALTIME_TIMESTAMP":"1761552000123456","PRIORITY":"6","MESSAGE":"Started Daily man-db regeneration.","SYSLOG_IDENTIFIER":"systemd","_PID":"1"}"#,
            "\n",
            r#"{"__REALTIME_TIMESTAMP":"1761552001000000","PRIORITY":"3","MESSAGE":[104,105,255],"SYSLOG_IDENTIFIER":"app"}"#,
            "\n",
            "not json\n",
            r#"{"__REALTIME_TIMESTAMP":"1761552002000000","MESSAGE":null}"#,
            "\n",
            r#"{"MESSAGE":"trailing newline\n"}"#,
        );
        let lines = parse_journal(fixture);
        assert_eq!(lines.len(), 4);
        assert_eq!(lines[0].timestamp_us, Some(1_761_552_000_123_456));
        assert_eq!(lines[0].priority, 6);
        assert_eq!(lines[0].identifier.as_deref(), Some("systemd"));
        assert_eq!(lines[0].pid, Some(1));
        assert_eq!(lines[1].priority, 3);
        assert_eq!(lines[1].message, "hi\u{fffd}");
        assert_eq!(lines[2].message, "");
        assert_eq!(lines[2].priority, PRIORITY_INFO);
        assert_eq!(lines[3].message, "trailing newline");
        assert_eq!(lines[3].timestamp_us, None);
    }

    #[test]
    fn journal_arguments() {
        assert_eq!(journal_args(Scope::System, "sshd.service", 40), ["-u", "sshd.service", "-n", "40", "-o", "json", "--no-pager", "-q"]);
        let user = journal_args(Scope::User, "a.service", 0);
        assert_eq!(user[0], "--user");
        assert_eq!(user[4], "1");
        assert_eq!(journal_args(Scope::System, "a.service", usize::MAX)[3], JOURNAL_MAX.to_string());
    }

    #[test]
    fn unit_helpers() {
        let u = Unit { name: "sshd.service".into(), description: String::new(), load_state: "loaded".into(), active_state: "failed".into(), sub_state: "failed".into(), file_state: Some("enabled".into()) };
        assert_eq!(u.health(), Health::Failed);
        assert_eq!(u.kind(), "service");
        assert_eq!(u.short_name(), "sshd");
        assert!(u.is_enabled());
        assert_eq!(Scope::parse("--user"), Some(Scope::User));
        assert_eq!(UnitAction::parse("restart"), Some(UnitAction::Restart));
    }

    // Read-only live checks against this machine's systemd.
    #[test]
    #[ignore]
    fn live_read_only() {
        for scope in Scope::ALL {
            let units = list_units(scope).unwrap();
            let failed = failed_units(scope).unwrap();
            println!("{scope:?}: {} units, {} failed, {} enabled", units.len(), failed.len(), units.iter().filter(|u| u.is_enabled()).count());
            assert!(!units.is_empty());
        }
        let st = unit_status(Scope::System, "systemd-journald").unwrap();
        println!("journald: {} / {} file={:?}", st.active_state, st.sub_state, st.file_state);
        assert_eq!(st.load_state, "loaded");
        println!("file state: {:?}", unit_file_state(Scope::System, "systemd-journald.service"));
        let j = journal(Scope::System, "systemd-journald", 5).unwrap();
        println!("journal: {j:#?}");
    }
}
