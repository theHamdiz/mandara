//! KeePassXC: find its databases, tell whether they are locked, and unlock
//! one with a password (Mandara's unlock dialog, `mandara msg keepassxc`).
//!
//! * KeePassXC's main window is on the session bus as [`BUS_NAME`] at
//!   [`OBJECT_PATH`] ([`INTERFACE`]): `openDatabase(file[, password[,
//!   key file]])`, `lockAllDatabases()`, `isHardwareKeySupported()`…
//!   `openDatabase` with a password unlocks the database (or its locked
//!   tab); it never reports failure: KeePassXC shows its own unlock screen
//!   instead. So the outcome is re-checked afterwards ([`UnlockOutcome`]).
//! * Lock state comes from KeePassXC's Secret Service (when enabled in its
//!   settings it owns `org.freedesktop.secrets`): one collection per open
//!   database with a `Locked` property. Without it the state is unknown.
//! * The database list comes from `keepassxc.ini` (`[General]`
//!   `LastOpenedDatabases`, `LastDatabases`, `LastActiveDatabase`,
//!   `LastKeyFiles`, `LastChallengeResponse`), in the local (cache) file of
//!   KeePassXC 2.7+ and the older config locations; maps are Qt
//!   `@Variant(…)` blobs, decoded here.
//!
//! Security: the password travels in clear text on the session bus to
//! KeePassXC (readable only by programs running as you), unlike
//! gnome-keyring's encrypted secret exchange. Mandara's own copy is wiped
//! right after the call.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Serialize;
use zbus::Connection;
use zbus::zvariant::OwnedObjectPath;
use zeroize::Zeroizing;

/// KeePassXC's main-window service name.
pub const BUS_NAME: &str = "org.keepassxc.KeePassXC.MainWindow";
pub const OBJECT_PATH: &str = "/keepassxc";
pub const INTERFACE: &str = "org.keepassxc.KeePassXC.MainWindow";
/// The Secret Service (KeePassXC serves it when its integration is on).
pub const SECRETS_NAME: &str = "org.freedesktop.secrets";
pub const SECRETS_PATH: &str = "/org/freedesktop/secrets";
pub const SERVICE_IFACE: &str = "org.freedesktop.Secret.Service";
pub const COLLECTION_IFACE: &str = "org.freedesktop.Secret.Collection";
/// KeePassXC's Flatpak id (its exported launcher and sandboxed config).
pub const FLATPAK_ID: &str = "org.keepassxc.KeePassXC";
/// KeePassXC's executable.
pub const BINARY_NAME: &str = "keepassxc";
const INI_NAME: &str = "keepassxc.ini";
const INI_DIRS: [&str; 2] = ["keepassxc", "KeePassXC"];
/// Where the exported launchers of system / user Flatpaks live.
const FLATPAK_EXPORTS: [&str; 2] = ["/var/lib/flatpak/exports/bin", ".local/share/flatpak/exports/bin"];
/// Pause between checks after an unlock request (KeePassXC derives the
/// key first: Argon2 takes about a second by default).
pub const VERIFY_STEP: Duration = Duration::from_millis(400);
/// Checks before giving up (about five seconds in all).
pub const VERIFY_ATTEMPTS: u32 = 12;
/// Qt metatype ids in `@Variant(…)` blobs.
const QT_BOOL: u32 = 1;
const QT_INT: u32 = 2;
const QT_UINT: u32 = 3;
const QT_VARIANT_MAP: u32 = 8;
const QT_STRING: u32 = 10;
const QT_STRING_LIST: u32 = 11;
const QT_BYTE_ARRAY: u32 = 12;
const QT_VARIANT_HASH: u32 = 28;
/// A null QString / QByteArray in a QDataStream.
const QT_NULL_LENGTH: u32 = u32::MAX;
/// Error text when KeePassXC isn't on the bus.
pub const NOT_RUNNING: &str = "KeePassXC isn't running";

/// Whether a database is locked.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum LockState {
    Locked,
    Unlocked,
    Unknown,
}

/// A database KeePassXC knows about.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Database {
    pub path: PathBuf,
    /// File name without `.kdbx`.
    pub name: String,
    /// The key file KeePassXC remembers for it.
    pub key_file: Option<PathBuf>,
    /// Opened with a hardware key (YubiKey challenge-response) last time.
    pub hardware_key: bool,
    /// The database KeePassXC had in front last time.
    pub last_active: bool,
    /// Open when KeePassXC last quit (it reopens these, locked, at start).
    pub reopened: bool,
    pub state: LockState,
}

/// One Secret Service collection.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Collection {
    pub label: String,
    pub locked: bool,
}

/// Everything the dialog and `keepassxc status` show.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Status {
    pub installed: bool,
    pub running: bool,
    /// KeePassXC serves the Secret Service (lock state is known).
    pub secret_service: bool,
    pub hardware_key_support: bool,
    pub databases: Vec<Database>,
    /// KeePassXC's collections (empty without its Secret Service).
    pub collections: Vec<Collection>,
}

impl Status {
    /// Some database is locked (None: KeePassXC can't tell us).
    pub fn any_locked(&self) -> Option<bool> {
        self.secret_service.then(|| self.collections.iter().any(|c| c.locked))
    }

    /// A database by path, file name or name (case-insensitive).
    pub fn find(&self, query: &str) -> Option<&Database> {
        let q = query.trim();
        let ql = q.to_lowercase();
        self.databases.iter().find(|d| d.path == Path::new(q)).or_else(|| {
            self.databases.iter().find(|d| d.name.to_lowercase() == ql || d.path.file_name().is_some_and(|f| f.to_string_lossy().to_lowercase() == ql))
        })
    }

    /// The database to offer first: the last active one, else the first locked one.
    pub fn preferred(&self) -> Option<&Database> {
        let dbs = &self.databases;
        dbs.iter()
            .find(|d| d.last_active && d.state != LockState::Unlocked)
            .or_else(|| dbs.iter().find(|d| d.state == LockState::Locked))
            .or_else(|| dbs.iter().find(|d| d.state != LockState::Unlocked))
            .or(dbs.first())
    }
}

/// What came of an unlock request.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum UnlockOutcome {
    /// The database's collection unlocked (or a new unlocked one appeared).
    Unlocked,
    /// Still locked after waiting: most likely a wrong password / key file.
    StillLocked,
    /// Sent, but KeePassXC can't show the result (no Secret Service).
    Sent,
}

// ── Installation and settings files ─────────────────────────────────────

/// KeePassXC is installed (native binary on `$PATH` or a Flatpak).
pub fn installed() -> bool {
    let on_path = std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(BINARY_NAME).is_file()));
    let home = dirs::home_dir().unwrap_or_default();
    on_path || FLATPAK_EXPORTS.iter().any(|d| if d.starts_with('/') { Path::new(d).join(FLATPAK_ID).exists() } else { home.join(d).join(FLATPAK_ID).exists() })
}

/// `keepassxc.ini` candidates, most specific first: the 2.7+ local file
/// (cache dir, where the database history lives), the config files, and
/// the Flatpak's sandboxed copies.
pub fn config_files() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let roots: Vec<PathBuf> = [dirs::cache_dir(), dirs::config_dir()].into_iter().flatten().collect();
    for root in roots {
        for d in INI_DIRS {
            out.push(root.join(d).join(INI_NAME));
        }
    }
    if let Some(home) = dirs::home_dir() {
        let app = home.join(".var/app").join(FLATPAK_ID);
        for sub in ["cache", "config"] {
            out.push(app.join(sub).join(INI_DIRS[0]).join(INI_NAME));
        }
    }
    out
}

/// The database history of one `keepassxc.ini`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IniSettings {
    pub last_opened: Vec<String>,
    pub last_databases: Vec<String>,
    pub last_active: Option<String>,
    /// Database path → key file path.
    pub key_files: HashMap<String, String>,
    /// Database paths opened with a hardware key.
    pub challenge_response: Vec<String>,
}

/// Parses the `[General]` group (and keys before any group).
pub fn parse_ini(text: &str) -> IniSettings {
    let mut out = IniSettings::default();
    let mut group = String::from("General");
    for line in text.lines() {
        let t = line.trim();
        if t.starts_with(';') || t.starts_with('#') || t.is_empty() {
            continue;
        }
        if t.starts_with('[') && t.ends_with(']') {
            group = t[1..t.len() - 1].to_string();
            continue;
        }
        if group != "General" {
            continue;
        }
        let Some((key, value)) = t.split_once('=') else { continue };
        let key = key.trim();
        match key {
            "LastOpenedDatabases" => out.last_opened = qt_value(value.trim()).into_list(),
            "LastDatabases" => out.last_databases = qt_value(value.trim()).into_list(),
            "LastActiveDatabase" => out.last_active = qt_value(value.trim()).into_list().into_iter().next().filter(|s| !s.is_empty()),
            "LastKeyFiles" => out.key_files = qt_value(value.trim()).into_map().into_iter().filter_map(|(k, v)| v.into_string().map(|v| (k, v))).filter(|(_, v)| !v.is_empty()).collect(),
            "LastChallengeResponse" => out.challenge_response = qt_value(value.trim()).into_map().into_iter().filter(|(_, v)| v.truthy()).map(|(k, _)| k).collect(),
            _ => {}
        }
    }
    out
}

/// Every known database, from the given `keepassxc.ini` files (earlier
/// files win), keeping only files that still exist.
pub fn databases_from(files: &[PathBuf]) -> Vec<Database> {
    let settings: Vec<IniSettings> = files.iter().filter_map(|f| std::fs::read_to_string(f).ok()).map(|t| parse_ini(&t)).collect();
    let mut paths: Vec<String> = Vec::new();
    let mut push = |p: &String| {
        if !p.is_empty() && !paths.contains(p) {
            paths.push(p.clone());
        }
    };
    for s in &settings {
        s.last_active.iter().for_each(&mut push);
        s.last_opened.iter().for_each(&mut push);
    }
    for s in &settings {
        s.last_databases.iter().for_each(&mut push);
    }
    let last_active = settings.iter().find_map(|s| s.last_active.clone());
    paths
        .into_iter()
        .filter(|p| Path::new(p).is_file())
        .map(|p| {
            let path = PathBuf::from(&p);
            Database {
                name: path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| p.clone()),
                key_file: settings.iter().find_map(|s| s.key_files.get(&p)).map(PathBuf::from),
                hardware_key: settings.iter().any(|s| s.challenge_response.contains(&p)),
                last_active: last_active.as_deref() == Some(p.as_str()),
                reopened: settings.iter().any(|s| s.last_opened.contains(&p)),
                state: LockState::Unknown,
                path,
            }
        })
        .collect()
}

// ── Qt settings values ──────────────────────────────────────────────────

/// A value from a QSettings ini file.
#[derive(Clone, Debug, PartialEq)]
pub enum QtValue {
    Bool(bool),
    Int(i64),
    String(String),
    List(Vec<String>),
    Map(Vec<(String, QtValue)>),
    Other,
}

impl QtValue {
    fn into_list(self) -> Vec<String> {
        match self {
            QtValue::List(v) => v,
            QtValue::String(s) => vec![s],
            _ => Vec::new(),
        }
    }

    fn into_map(self) -> Vec<(String, QtValue)> {
        match self {
            QtValue::Map(m) => m,
            _ => Vec::new(),
        }
    }

    fn into_string(self) -> Option<String> {
        match self {
            QtValue::String(s) => Some(s),
            _ => None,
        }
    }

    fn truthy(&self) -> bool {
        match self {
            QtValue::Bool(b) => *b,
            QtValue::Int(i) => *i != 0,
            QtValue::String(s) => !s.is_empty() && s != "false" && s != "0",
            QtValue::List(l) => !l.is_empty(),
            QtValue::Map(m) => !m.is_empty(),
            QtValue::Other => true,
        }
    }
}

/// Decodes one QSettings ini value: a string list (`a, "b, c"`), a plain
/// string, or an `@Variant(…)` / `@String(…)` blob.
pub fn qt_value(raw: &str) -> QtValue {
    let items = unescape_list(raw);
    if let [one] = items.as_slice() {
        if let Some(inner) = one.strip_prefix("@Variant(").and_then(|s| s.strip_suffix(')')) {
            // Each char is one byte of the QDataStream.
            let bytes: Option<Vec<u8>> = inner.chars().map(|c| u8::try_from(u32::from(c)).ok()).collect();
            return bytes.and_then(|b| parse_variant(&mut b.as_slice())).unwrap_or(QtValue::Other);
        }
        if let Some(inner) = one.strip_prefix("@String(").and_then(|s| s.strip_suffix(')')) {
            return QtValue::String(inner.to_string());
        }
        if one == "@Invalid()" {
            return QtValue::Other;
        }
        if let Some(rest) = one.strip_prefix("@@") {
            return QtValue::String(format!("@{rest}"));
        }
        return QtValue::String(one.clone());
    }
    if items.is_empty() { QtValue::String(String::new()) } else { QtValue::List(items) }
}

/// QSettings' ini unescaping: quotes, `\\`, `\"`, `\n`…, greedy `\x…` hex
/// and `\0…` octal escapes, commas between list items.
fn unescape_list(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut any_quote = false;
    let mut chars = raw.chars().peekable();
    let finish = |cur: &mut String, any_quote: bool, out: &mut Vec<String>| {
        let s = std::mem::take(cur);
        out.push(if any_quote { s } else { s.trim().to_string() });
    };
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                any_quote = true;
            }
            ',' if !quoted => {
                finish(&mut cur, any_quote, &mut out);
                any_quote = false;
            }
            ';' if !quoted => break,
            '\\' => match chars.next() {
                Some('x') => {
                    let mut code: u32 = 0;
                    while let Some(d) = chars.peek().and_then(|c| c.to_digit(16)) {
                        code = code.saturating_mul(16).saturating_add(d);
                        chars.next();
                    }
                    cur.extend(char::from_u32(code));
                }
                Some(d @ '0'..='7') => {
                    let mut code = d.to_digit(8).unwrap_or(0);
                    while let Some(d) = chars.peek().and_then(|c| c.to_digit(8)) {
                        code = code.saturating_mul(8).saturating_add(d);
                        chars.next();
                    }
                    cur.extend(char::from_u32(code));
                }
                Some('a') => cur.push('\x07'),
                Some('b') => cur.push('\x08'),
                Some('f') => cur.push('\x0c'),
                Some('n') => cur.push('\n'),
                Some('r') => cur.push('\r'),
                Some('t') => cur.push('\t'),
                Some('v') => cur.push('\x0b'),
                Some(other) => cur.push(other),
                None => {}
            },
            c => cur.push(c),
        }
    }
    if !cur.is_empty() || any_quote || !out.is_empty() {
        finish(&mut cur, any_quote, &mut out);
    }
    out
}

fn take<'a>(b: &mut &'a [u8], n: usize) -> Option<&'a [u8]> {
    if b.len() < n {
        return None;
    }
    let (head, tail) = b.split_at(n);
    *b = tail;
    Some(head)
}

fn take_u32(b: &mut &[u8]) -> Option<u32> {
    take(b, 4).map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
}

/// A QDataStream QString: byte length + UTF-16BE.
fn take_qstring(b: &mut &[u8]) -> Option<String> {
    let len = take_u32(b)?;
    if len == QT_NULL_LENGTH {
        return Some(String::new());
    }
    let raw = take(b, len as usize)?;
    let units: Vec<u16> = raw.as_chunks::<2>().0.iter().map(|c| u16::from_be_bytes(*c)).collect();
    Some(String::from_utf16_lossy(&units))
}

/// A QDataStream QVariant: type id, null flag, value.
fn parse_variant(b: &mut &[u8]) -> Option<QtValue> {
    let ty = take_u32(b)?;
    let _null = take(b, 1)?;
    Some(match ty {
        QT_BOOL => QtValue::Bool(take(b, 1)?[0] != 0),
        QT_INT => QtValue::Int(i64::from(take_u32(b)? as i32)),
        QT_UINT => QtValue::Int(i64::from(take_u32(b)?)),
        QT_STRING => QtValue::String(take_qstring(b)?),
        QT_BYTE_ARRAY => {
            let len = take_u32(b)?;
            if len == QT_NULL_LENGTH { QtValue::String(String::new()) } else { QtValue::String(String::from_utf8_lossy(take(b, len as usize)?).into_owned()) }
        }
        QT_STRING_LIST => {
            let n = take_u32(b)?;
            QtValue::List((0..n).map(|_| take_qstring(b)).collect::<Option<Vec<_>>>()?)
        }
        QT_VARIANT_MAP | QT_VARIANT_HASH => {
            let n = take_u32(b)?;
            let mut m = Vec::new();
            for _ in 0..n {
                let k = take_qstring(b)?;
                let v = parse_variant(b)?;
                m.push((k, v));
            }
            QtValue::Map(m)
        }
        _ => return None,
    })
}

// ── D-Bus ────────────────────────────────────────────────────────────────

async fn owner(conn: &Connection, name: &'static str) -> Option<String> {
    let dbus = zbus::fdo::DBusProxy::new(conn).await.ok()?;
    let name = zbus::names::BusName::try_from(name).ok()?;
    dbus.get_name_owner(name).await.ok().map(|o| o.to_string())
}

/// KeePassXC's main window is on the bus.
pub async fn running(conn: &Connection) -> bool {
    owner(conn, BUS_NAME).await.is_some()
}

async fn proxy<'a>(conn: &Connection, dest: &'a str, path: &'a str, iface: &'a str) -> zbus::Result<zbus::Proxy<'a>> {
    zbus::proxy::Builder::<zbus::Proxy>::new(conn).destination(dest)?.path(path)?.interface(iface)?.cache_properties(zbus::proxy::CacheProperties::No).build().await
}

/// KeePassXC's Secret Service collections, when KeePassXC (the owner of
/// its main-window name) also owns `org.freedesktop.secrets`.
pub async fn collections(conn: &Connection) -> Option<Vec<Collection>> {
    let kp = owner(conn, BUS_NAME).await?;
    if owner(conn, SECRETS_NAME).await.as_deref() != Some(kp.as_str()) {
        return None;
    }
    let service = proxy(conn, SECRETS_NAME, SECRETS_PATH, SERVICE_IFACE).await.ok()?;
    let paths: Vec<OwnedObjectPath> = service.get_property("Collections").await.ok()?;
    let mut out = Vec::new();
    for p in paths {
        let Ok(c) = proxy(conn, SECRETS_NAME, p.as_str(), COLLECTION_IFACE).await else { continue };
        let label: String = c.get_property("Label").await.unwrap_or_default();
        let locked: bool = c.get_property("Locked").await.unwrap_or(true);
        out.push(Collection { label, locked });
    }
    Some(out)
}

/// The collection that belongs to `db` (KeePassXC labels it with the
/// database's name, usually the file name).
fn collection_for<'a>(collections: &'a [Collection], db: &Path) -> Option<&'a Collection> {
    let stem = db.file_stem()?.to_string_lossy().to_lowercase();
    collections.iter().find(|c| c.label.to_lowercase() == stem)
}

/// Status from the bus and the given settings files.
pub async fn status_on(conn: &Connection, files: &[PathBuf]) -> Status {
    let mut databases = databases_from(files);
    let running = running(conn).await;
    let collections = if running { collections(conn).await } else { None };
    let hardware_key_support = running && hardware_key_supported(conn).await;
    for db in &mut databases {
        db.state = match (&collections, running) {
            (_, false) => LockState::Locked,
            (Some(cs), true) => match collection_for(cs, &db.path) {
                Some(c) if c.locked => LockState::Locked,
                Some(_) => LockState::Unlocked,
                None => LockState::Unknown,
            },
            (None, true) => LockState::Unknown,
        };
    }
    Status { installed: installed(), running, secret_service: collections.is_some(), hardware_key_support, databases, collections: collections.unwrap_or_default() }
}

/// `isHardwareKeySupported()` (false when it can't be asked).
pub async fn hardware_key_supported(conn: &Connection) -> bool {
    let Ok(p) = proxy(conn, BUS_NAME, OBJECT_PATH, INTERFACE).await else { return false };
    p.call::<_, _, bool>("isHardwareKeySupported", &()).await.unwrap_or(false)
}

/// Did the unlock of `db` work, comparing collections before and after?
fn judge(before: &[Collection], now: &[Collection], db: &Path) -> Option<UnlockOutcome> {
    if let Some(c) = collection_for(now, db) {
        return Some(if c.locked { UnlockOutcome::StillLocked } else { UnlockOutcome::Unlocked });
    }
    let locked = |cs: &[Collection]| cs.iter().filter(|c| c.locked).count();
    let unlocked = |cs: &[Collection]| cs.iter().filter(|c| !c.locked).count();
    if locked(now) < locked(before) || unlocked(now) > unlocked(before) {
        return Some(UnlockOutcome::Unlocked);
    }
    None
}

/// Asks KeePassXC to unlock `db` with `password` (and `key_file`), then
/// watches its Secret Service collections to tell whether it worked.
/// The password buffer is wiped as soon as the call is sent.
pub async fn unlock_on(conn: &Connection, db: &Path, password: Zeroizing<String>, key_file: Option<&Path>) -> Result<UnlockOutcome, String> {
    if !running(conn).await {
        return Err(NOT_RUNNING.into());
    }
    let before = collections(conn).await;
    let p = proxy(conn, BUS_NAME, OBJECT_PATH, INTERFACE).await.map_err(|e| e.to_string())?;
    let file = db.to_string_lossy();
    let sent = match key_file {
        Some(k) => p.call_method("openDatabase", &(file.as_ref(), password.as_str(), k.to_string_lossy().as_ref())).await,
        None => p.call_method("openDatabase", &(file.as_ref(), password.as_str())).await,
    };
    drop(password);
    sent.map_err(|e| format!("KeePassXC: {e}"))?;
    let Some(before) = before else { return Ok(UnlockOutcome::Sent) };
    let mut last = before.clone();
    for _ in 0..VERIFY_ATTEMPTS {
        crate::runtime::sleep(VERIFY_STEP).await;
        let Some(now) = collections(conn).await else { return Ok(UnlockOutcome::Sent) };
        if let Some(UnlockOutcome::Unlocked) = judge(&before, &now, db) {
            return Ok(UnlockOutcome::Unlocked);
        }
        last = now;
    }
    Ok(match judge(&before, &last, db) {
        Some(o) => o,
        // Nothing changed: a locked database stayed locked, or one KeePassXC
        // doesn't expose over the Secret Service did something unseen.
        None if before.iter().any(|c| c.locked) => UnlockOutcome::StillLocked,
        None => UnlockOutcome::Sent,
    })
}

/// Brings up KeePassXC's own window for `db` (hardware keys, failures).
pub async fn open_window_on(conn: &Connection, db: &Path) -> Result<(), String> {
    let p = proxy(conn, BUS_NAME, OBJECT_PATH, INTERFACE).await.map_err(|e| e.to_string())?;
    p.call_method("openDatabase", &(db.to_string_lossy().as_ref(),)).await.map(|_| ()).map_err(|e| format!("KeePassXC: {e}"))
}

/// Locks every open database.
pub async fn lock_all_on(conn: &Connection) -> Result<(), String> {
    if !running(conn).await {
        return Err(NOT_RUNNING.into());
    }
    let p = proxy(conn, BUS_NAME, OBJECT_PATH, INTERFACE).await.map_err(|e| e.to_string())?;
    p.call_method("lockAllDatabases", &()).await.map(|_| ()).map_err(|e| format!("KeePassXC: {e}"))
}

// ── Session-bus helpers (run on the service runtime) ─────────────────────

fn spawn_with<T, F, Fut>(name: &'static str, f: F) -> async_channel::Receiver<Result<T, String>>
where
    T: Send + 'static,
    F: FnOnce(Connection) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<T, String>> + Send,
{
    let (tx, rx) = async_channel::bounded(1);
    crate::runtime::spawn(name, async move {
        let result = match crate::runtime::session().await {
            Ok(conn) => f(conn).await,
            Err(e) => Err(e.to_string()),
        };
        let _ = tx.send(result).await;
    });
    rx
}

/// [`status_on`] on the session bus with the user's settings files.
pub fn spawn_status() -> async_channel::Receiver<Result<Status, String>> {
    spawn_with("keepassxc-status", |conn| async move { Ok(status_on(&conn, &config_files()).await) })
}

/// [`unlock_on`] on the session bus.
pub fn spawn_unlock(db: PathBuf, password: Zeroizing<String>, key_file: Option<PathBuf>) -> async_channel::Receiver<Result<UnlockOutcome, String>> {
    spawn_with("keepassxc-unlock", move |conn| async move { unlock_on(&conn, &db, password, key_file.as_deref()).await })
}

/// [`open_window_on`] on the session bus.
pub fn spawn_open_window(db: PathBuf) -> async_channel::Receiver<Result<(), String>> {
    spawn_with("keepassxc-open", move |conn| async move { open_window_on(&conn, &db).await })
}

/// [`lock_all_on`] on the session bus.
pub fn spawn_lock_all() -> async_channel::Receiver<Result<(), String>> {
    spawn_with("keepassxc-lock", |conn| async move { lock_all_on(&conn).await })
}

/// Waits up to `timeout` for KeePassXC, then reads the status (session
/// bus, the user's settings files).
pub fn spawn_wait_status(timeout: Duration, step: Duration) -> async_channel::Receiver<Result<Status, String>> {
    spawn_with("keepassxc-wait", move |conn| async move {
        wait_running(&conn, timeout, step).await;
        Ok(status_on(&conn, &config_files()).await)
    })
}

/// Waits (up to `timeout`) for KeePassXC to appear on the bus, e.g. when
/// it autostarts with the session.
pub async fn wait_running(conn: &Connection, timeout: Duration, step: Duration) -> bool {
    let start = std::time::Instant::now();
    loop {
        if running(conn).await {
            return true;
        }
        if start.elapsed() >= timeout {
            return false;
        }
        crate::runtime::sleep(step).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Encodes like QSettings: chars outside printable ASCII as `\x..`,
    /// a hex digit right after an escape escaped too, and the whole value
    /// quoted when it holds `;`, `,` or `=`.
    fn qsettings_escape(bytes: &[u8]) -> String {
        let quote = bytes.iter().any(|b| matches!(b, b';' | b',' | b'='));
        let mut s = String::new();
        let mut after_escape = false;
        for &b in bytes {
            let c = b as char;
            if b == 0 {
                s.push_str("\\0");
                after_escape = true;
            } else if !(0x20..0x7f).contains(&b) || (after_escape && c.is_ascii_hexdigit()) {
                s.push_str(&format!("\\x{b:x}"));
                after_escape = true;
            } else if c == '\\' || c == '"' {
                s.push('\\');
                s.push(c);
                after_escape = false;
            } else {
                s.push(c);
                after_escape = false;
            }
        }
        if quote { format!("\"{s}\"") } else { s }
    }

    fn qstring(s: &str) -> Vec<u8> {
        let units: Vec<u8> = s.encode_utf16().flat_map(u16::to_be_bytes).collect();
        let mut v = (units.len() as u32).to_be_bytes().to_vec();
        v.extend(units);
        v
    }

    /// `@Variant(…)` of a QVariantHash of string → string.
    fn variant_hash(pairs: &[(&str, &str)]) -> String {
        let mut b = QT_VARIANT_HASH.to_be_bytes().to_vec();
        b.push(0);
        b.extend((pairs.len() as u32).to_be_bytes());
        for (k, v) in pairs {
            b.extend(qstring(k));
            b.extend(QT_STRING.to_be_bytes());
            b.push(0);
            b.extend(qstring(v));
        }
        qsettings_escape(&[b"@Variant(".as_slice(), &b, b")"].concat())
    }

    #[test]
    fn decodes_qsettings_lists_and_strings() {
        assert_eq!(qt_value("/home/u/a.kdbx, /home/u/b.kdbx"), QtValue::List(vec!["/home/u/a.kdbx".into(), "/home/u/b.kdbx".into()]));
        assert_eq!(qt_value("\"/home/u/with, comma.kdbx\", /x.kdbx"), QtValue::List(vec!["/home/u/with, comma.kdbx".into(), "/x.kdbx".into()]));
        assert_eq!(qt_value("/home/u/only.kdbx"), QtValue::String("/home/u/only.kdbx".into()));
        assert_eq!(qt_value("/home/u/\\x645\\x628.kdbx"), QtValue::String("/home/u/مب.kdbx".into()));
        assert_eq!(qt_value("@Invalid()"), QtValue::Other);
    }

    #[test]
    fn decodes_variant_hashes_like_keepassxc_writes_them() {
        let raw = variant_hash(&[("/home/u/Passwords.kdbx", "/home/u/keys/main.key"), ("/home/u/كلمات.kdbx", "/home/u/k2.keyx")]);
        assert!(raw.starts_with("\"@Variant(\\0\\0\\0\\x1c\\0\\0\\0\\0\\x2\\0"), "{raw}");
        let QtValue::Map(m) = qt_value(&raw) else { panic!("not a map: {raw}") };
        assert_eq!(m[0], ("/home/u/Passwords.kdbx".to_string(), QtValue::String("/home/u/keys/main.key".into())));
        assert_eq!(m[1].0, "/home/u/كلمات.kdbx");
    }

    #[test]
    fn reads_database_history_and_key_files() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("Passwords.kdbx");
        let b = dir.path().join("Work.kdbx");
        let gone = dir.path().join("Deleted.kdbx");
        std::fs::write(&a, b"kdbx").unwrap();
        std::fs::write(&b, b"kdbx").unwrap();
        let (a_s, b_s, gone_s) = (a.display().to_string(), b.display().to_string(), gone.display().to_string());
        let local = dir.path().join("cache.ini");
        std::fs::write(
            &local,
            format!(
                "[General]\nLastActiveDatabase={b_s}\nLastOpenedDatabases={b_s}\nLastDatabases={a_s}, {b_s}, {gone_s}\nLastKeyFiles={}\nLastChallengeResponse={}\n\n[Browser]\nLastDatabases=/nope.kdbx\n",
                variant_hash(&[(&a_s, "/k/main.key")]),
                {
                    let mut bytes = QT_VARIANT_HASH.to_be_bytes().to_vec();
                    bytes.push(0);
                    bytes.extend(1u32.to_be_bytes());
                    bytes.extend(qstring(&b_s));
                    bytes.extend(QT_BOOL.to_be_bytes());
                    bytes.extend([0, 1]);
                    qsettings_escape(&[b"@Variant(".as_slice(), &bytes, b")"].concat())
                }
            ),
        )
        .unwrap();
        let old = dir.path().join("config.ini");
        std::fs::write(&old, format!("[General]\nLastOpenedDatabases={a_s}\n")).unwrap();
        let dbs = databases_from(&[local, old, dir.path().join("missing.ini")]);
        assert_eq!(dbs.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), ["Work", "Passwords"], "active first, deleted files dropped");
        assert!(dbs[0].last_active && dbs[0].reopened && dbs[0].hardware_key && dbs[0].key_file.is_none());
        assert_eq!(dbs[1].key_file.as_deref(), Some(Path::new("/k/main.key")));
        assert!(dbs[1].reopened && !dbs[1].hardware_key);
    }

    #[test]
    fn judges_unlocks_by_collection() {
        let c = |label: &str, locked| Collection { label: label.into(), locked };
        let db = Path::new("/h/Passwords.kdbx");
        assert_eq!(judge(&[c("Passwords", true)], &[c("Passwords", false)], db), Some(UnlockOutcome::Unlocked));
        assert_eq!(judge(&[c("Passwords", true)], &[c("Passwords", true)], db), Some(UnlockOutcome::StillLocked));
        assert_eq!(judge(&[c("Vault", true)], &[c("Vault", false)], db), Some(UnlockOutcome::Unlocked), "an unnamed collection unlocked");
        assert_eq!(judge(&[], &[c("Vault", false)], db), Some(UnlockOutcome::Unlocked), "a closed database opened");
        assert_eq!(judge(&[c("Vault", true)], &[c("Vault", true)], db), None);
    }

    #[test]
    fn finds_databases_by_name_or_path() {
        let db = |name: &str, state| Database { path: PathBuf::from(format!("/h/{name}.kdbx")), name: name.into(), key_file: None, hardware_key: false, last_active: false, reopened: false, state };
        let s = Status { databases: vec![db("Work", LockState::Unlocked), db("Passwords", LockState::Locked)], ..Status::default() };
        assert_eq!(s.find("passwords").unwrap().name, "Passwords");
        assert_eq!(s.find("Work.kdbx").unwrap().name, "Work");
        assert_eq!(s.find("/h/Work.kdbx").unwrap().name, "Work");
        assert!(s.find("nope").is_none());
        assert_eq!(s.preferred().unwrap().name, "Passwords", "the locked one first");
    }
}
