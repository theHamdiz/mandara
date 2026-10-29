//! The KeePassXC client against a mock KeePassXC on a private bus.
//!
//! A throwaway `dbus-daemon` (no service dirs, nothing activatable) hosts a
//! tiny zbus server that owns `org.keepassxc.KeePassXC.MainWindow` and
//! `org.freedesktop.secrets`, answers `openDatabase` (all three overloads),
//! `lockAllDatabases`, `isHardwareKeySupported`, and serves one Secret
//! Service collection per open database with a `Locked` property. Real
//! KeePassXC, the user's session bus and their databases are never used.
//!
//! Skipped (passes) when `dbus-daemon` isn't installed.

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use futures::StreamExt as _;
use mandara_services::keepassxc::{self, BUS_NAME, COLLECTION_IFACE, LockState, NOT_RUNNING, OBJECT_PATH, SECRETS_NAME, SECRETS_PATH, SERVICE_IFACE, UnlockOutcome};
use mandara_services::runtime;
use zbus::zvariant::{ObjectPath, Value};
use zeroize::Zeroizing;

const PASSWORD: &str = "Corniche sea-glass ✓ كلمة";
const KEY_FILE: &str = "/keys/work.keyx";
/// Unix socket paths must fit in 108 bytes.
const MAX_SOCKET_DIR: usize = 80;
const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";

struct Kill(Vec<Child>);

impl Drop for Kill {
    fn drop(&mut self) {
        for child in &mut self.0 {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn which(bin: &str) -> Option<PathBuf> {
    std::env::var_os("PATH").and_then(|paths| std::env::split_paths(&paths).map(|p| p.join(bin)).find(|p| p.is_file()))
}

fn scratch() -> tempfile::TempDir {
    let base = std::env::temp_dir();
    let base = if base.as_os_str().len() > MAX_SOCKET_DIR { PathBuf::from("/tmp") } else { base };
    tempfile::Builder::new().prefix("mkp").tempdir_in(base).unwrap()
}

fn private_bus(dir: &Path, kids: &mut Kill) -> String {
    let config = dir.join("bus.conf");
    std::fs::write(
        &config,
        format!(
            r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:path={}/bus</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#,
            dir.display()
        ),
    )
    .unwrap();
    let mut child = Command::new("dbus-daemon")
        .arg(format!("--config-file={}", config.display()))
        .args(["--nofork", "--print-address=1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
    kids.0.push(child);
    line.trim().to_string()
}

/// One mock database.
struct MockDb {
    file: String,
    label: &'static str,
    key_file: Option<&'static str>,
    /// Open in a tab (a collection exists).
    open: bool,
    locked: bool,
}

#[derive(Default)]
struct Mock {
    dbs: Vec<MockDb>,
    /// Every call: member + signature + file argument (never the password).
    calls: Vec<String>,
    /// Passwords received (to check what crossed the bus).
    passwords: Vec<String>,
}

impl Mock {
    fn collection_paths(&self) -> Vec<String> {
        self.dbs.iter().enumerate().filter(|(_, d)| d.open).map(|(i, _)| format!("{SECRETS_PATH}/collection/c{i}")).collect()
    }
}

/// Serves the mock on `conn` until the connection closes.
async fn serve(conn: zbus::Connection, mock: Arc<Mutex<Mock>>) {
    let mut stream = zbus::MessageStream::from(&conn);
    while let Some(Ok(msg)) = stream.next().await {
        if msg.message_type() != zbus::message::Type::MethodCall {
            continue;
        }
        let header = msg.header();
        let (Some(path), Some(member)) = (header.path().map(|p| p.to_string()), header.member().map(|m| m.to_string())) else { continue };
        let iface = header.interface().map(|i| i.to_string()).unwrap_or_default();
        let body = msg.body();
        let signature = body.signature().to_string().trim_start_matches('(').trim_end_matches(')').to_string();
        let reply = match (path.as_str(), iface.as_str(), member.as_str()) {
            (OBJECT_PATH, _, "openDatabase") => {
                let (file, password, key) = match signature.as_str() {
                    "sss" => body.deserialize::<(String, String, String)>().map(|(f, p, k)| (f, Some(p), Some(k))).unwrap(),
                    "ss" => body.deserialize::<(String, String)>().map(|(f, p)| (f, Some(p), None)).unwrap(),
                    _ => body.deserialize::<(String,)>().map(|(f,)| (f, None, None)).unwrap(),
                };
                {
                    let mut m = mock.lock().unwrap();
                    m.calls.push(format!("openDatabase({signature}) {file}"));
                    if let Some(pw) = &password {
                        m.passwords.push(pw.clone());
                    }
                    if let Some(db) = m.dbs.iter_mut().find(|d| d.file == file) {
                        db.open = true;
                        let key_ok = db.key_file.is_none_or(|k| key.as_deref() == Some(k));
                        if password.as_deref() == Some(PASSWORD) && key_ok {
                            db.locked = false;
                        }
                    }
                }
                conn.reply(&header, &()).await
            }
            (OBJECT_PATH, _, "lockAllDatabases") => {
                {
                    let mut m = mock.lock().unwrap();
                    m.calls.push("lockAllDatabases".into());
                    m.dbs.iter_mut().for_each(|d| d.locked = true);
                }
                conn.reply(&header, &()).await
            }
            (OBJECT_PATH, _, "isHardwareKeySupported") => conn.reply(&header, &true).await,
            (_, PROPERTIES_IFACE, "Get") => {
                let (prop_iface, prop): (String, String) = body.deserialize().unwrap();
                let value = {
                    let m = mock.lock().unwrap();
                    match (path.as_str(), prop_iface.as_str(), prop.as_str()) {
                        (SECRETS_PATH, SERVICE_IFACE, "Collections") => {
                            Some(Value::from(m.collection_paths().into_iter().map(|p| ObjectPath::try_from(p).unwrap()).collect::<Vec<_>>()).try_to_owned().unwrap())
                        }
                        (p, COLLECTION_IFACE, what) => {
                            let i: Option<usize> = p.strip_prefix(&format!("{SECRETS_PATH}/collection/c")).and_then(|n| n.parse().ok());
                            i.and_then(|i| m.dbs.get(i)).map(|d| match what {
                                "Label" => Value::from(d.label).try_to_owned().unwrap(),
                                _ => Value::from(d.locked).try_to_owned().unwrap(),
                            })
                        }
                        _ => None,
                    }
                };
                match value {
                    Some(v) => conn.reply(&header, &Value::from(v)).await,
                    None => conn.reply_error(&header, "org.freedesktop.DBus.Error.UnknownProperty", &prop).await,
                }
            }
            _ => conn.reply_error(&header, "org.freedesktop.DBus.Error.UnknownMethod", &member).await,
        };
        let _ = reply;
    }
}

struct Setup {
    _kids: Kill,
    _dir: tempfile::TempDir,
    bus: String,
    mock: Arc<Mutex<Mock>>,
    ini: PathBuf,
    passwords_db: PathBuf,
    work_db: PathBuf,
}

/// The bus, the mock (owning both names unless `secret_service` is off) and
/// a keepassxc.ini listing two databases.
fn setup(secret_service: bool) -> Option<Setup> {
    if which("dbus-daemon").is_none() {
        eprintln!("dbus-daemon isn't installed; skipping");
        return None;
    }
    let dir = scratch();
    let mut kids = Kill(Vec::new());
    let bus = private_bus(dir.path(), &mut kids);
    let passwords_db = dir.path().join("Passwords.kdbx");
    let work_db = dir.path().join("Work.kdbx");
    std::fs::write(&passwords_db, b"kdbx").unwrap();
    std::fs::write(&work_db, b"kdbx").unwrap();
    let ini = dir.path().join("keepassxc.ini");
    std::fs::write(&ini, format!("[General]\nLastActiveDatabase={}\nLastOpenedDatabases={}, {}\n", passwords_db.display(), passwords_db.display(), work_db.display())).unwrap();
    let mock = Arc::new(Mutex::new(Mock {
        dbs: vec![
            MockDb { file: passwords_db.display().to_string(), label: "Passwords", key_file: None, open: true, locked: true },
            MockDb { file: work_db.display().to_string(), label: "Work", key_file: Some(KEY_FILE), open: true, locked: true },
        ],
        ..Mock::default()
    }));
    let server = runtime::block_on(async {
        let mut b = zbus::connection::Builder::address(bus.as_str()).unwrap().name(BUS_NAME).unwrap();
        if secret_service {
            b = b.name(SECRETS_NAME).unwrap();
        }
        b.build().await.unwrap()
    });
    let m = mock.clone();
    std::thread::spawn(move || runtime::block_on(serve(server, m)));
    Some(Setup { _kids: kids, _dir: dir, bus, mock, ini, passwords_db, work_db })
}

fn client(bus: &str) -> zbus::Connection {
    runtime::block_on(async { zbus::connection::Builder::address(bus).unwrap().build().await.unwrap() })
}

#[test]
fn status_lists_databases_and_lock_states() {
    let Some(s) = setup(true) else { return };
    let conn = client(&s.bus);
    let status = runtime::block_on(keepassxc::status_on(&conn, std::slice::from_ref(&s.ini)));
    assert!(status.running && status.secret_service && status.hardware_key_support);
    assert_eq!(status.any_locked(), Some(true));
    assert_eq!(status.databases.iter().map(|d| (d.name.as_str(), d.state)).collect::<Vec<_>>(), [("Passwords", LockState::Locked), ("Work", LockState::Locked)]);
    assert_eq!(status.preferred().unwrap().name, "Passwords", "the last active one");
}

#[test]
fn unlock_verifies_through_the_secret_service() {
    let Some(s) = setup(true) else { return };
    let conn = client(&s.bus);
    // Wrong password: KeePassXC reports nothing, the collection stays locked.
    let out = runtime::block_on(keepassxc::unlock_on(&conn, &s.passwords_db, Zeroizing::new("wrong".into()), None)).unwrap();
    assert_eq!(out, UnlockOutcome::StillLocked);
    // Right password: two-argument overload, collection unlocks.
    let out = runtime::block_on(keepassxc::unlock_on(&conn, &s.passwords_db, Zeroizing::new(PASSWORD.into()), None)).unwrap();
    assert_eq!(out, UnlockOutcome::Unlocked);
    // A key file: the three-argument overload.
    let out = runtime::block_on(keepassxc::unlock_on(&conn, &s.work_db, Zeroizing::new(PASSWORD.into()), Some(Path::new(KEY_FILE)))).unwrap();
    assert_eq!(out, UnlockOutcome::Unlocked);
    let status = runtime::block_on(keepassxc::status_on(&conn, std::slice::from_ref(&s.ini)));
    assert_eq!(status.any_locked(), Some(false));
    // Lock everything again; open KeePassXC's own window (one argument).
    runtime::block_on(keepassxc::lock_all_on(&conn)).unwrap();
    runtime::block_on(keepassxc::open_window_on(&conn, &s.work_db)).unwrap();
    let status = runtime::block_on(keepassxc::status_on(&conn, std::slice::from_ref(&s.ini)));
    assert!(status.databases.iter().all(|d| d.state == LockState::Locked));
    let m = s.mock.lock().unwrap();
    let p = s.passwords_db.display();
    let w = s.work_db.display();
    assert_eq!(m.calls, [format!("openDatabase(ss) {p}"), format!("openDatabase(ss) {p}"), format!("openDatabase(sss) {w}"), "lockAllDatabases".into(), format!("openDatabase(s) {w}")]);
    assert_eq!(m.passwords, ["wrong", PASSWORD, PASSWORD]);
}

#[test]
fn without_secret_service_the_result_is_unknown() {
    let Some(s) = setup(false) else { return };
    let conn = client(&s.bus);
    let status = runtime::block_on(keepassxc::status_on(&conn, std::slice::from_ref(&s.ini)));
    assert!(status.running && !status.secret_service);
    assert_eq!(status.any_locked(), None);
    assert!(status.databases.iter().all(|d| d.state == LockState::Unknown));
    let out = runtime::block_on(keepassxc::unlock_on(&conn, &s.passwords_db, Zeroizing::new(PASSWORD.into()), None)).unwrap();
    assert_eq!(out, UnlockOutcome::Sent);
}

#[test]
fn reports_keepassxc_not_running() {
    if which("dbus-daemon").is_none() {
        return;
    }
    let dir = scratch();
    let mut kids = Kill(Vec::new());
    let bus = private_bus(dir.path(), &mut kids);
    let conn = client(&bus);
    let err = runtime::block_on(keepassxc::unlock_on(&conn, Path::new("/x.kdbx"), Zeroizing::new(PASSWORD.into()), None)).unwrap_err();
    assert_eq!(err, NOT_RUNNING);
    assert!(!runtime::block_on(keepassxc::status_on(&conn, &[])).running);
    assert!(!runtime::block_on(keepassxc::wait_running(&conn, std::time::Duration::ZERO, std::time::Duration::ZERO)));
}
