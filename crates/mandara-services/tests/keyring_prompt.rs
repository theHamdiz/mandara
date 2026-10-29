//! Interop with the real gnome-keyring-daemon: Mandara's prompter creates,
//! unlocks (after a wrong password) and refuses to unlock a keyring, with
//! the password crossing gcr's `[sx-aes-1]` secret exchange.
//!
//! Everything is private: a throwaway `dbus-daemon` (no service dirs, so
//! nothing — not even gcr-prompter — can be D-Bus activated), a sandboxed
//! `gnome-keyring-daemon --foreground --components=secrets` whose HOME /
//! XDG dirs / control directory live in a temp dir (a fresh, empty keyring
//! store), and a dead system bus. The user's session bus and keyrings are
//! never touched.
//!
//! Ignored by default (needs gnome-keyring-daemon and dbus-daemon):
//! `cargo test -p mandara-services --test keyring_prompt -- --ignored`

use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use futures::{FutureExt as _, StreamExt as _};
use mandara_services::keyring_prompt::{KeyringEvent, KeyringPrompter, KeyringReply, PromptKind, PromptRequest, PrompterStatus, spawn_keyring_prompt_at};
use mandara_services::runtime;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zeroize::Zeroizing;

const PASSWORD: &str = "Corniche sea-glass 1 ✓ كلمة";
const WRONG: &str = "not the password";
const LABEL: &str = "Mandara Interop";
const STEP: Duration = Duration::from_secs(15);
const SECRETS: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE_IFACE: &str = "org.freedesktop.Secret.Service";
const PROMPT_IFACE: &str = "org.freedesktop.Secret.Prompt";
const COLLECTION_IFACE: &str = "org.freedesktop.Secret.Collection";
/// Unix socket paths must fit in 108 bytes.
const MAX_SOCKET_DIR: usize = 80;

/// Child processes, killed on drop; on a failure the keyring daemon's log
/// is printed.
struct Kill(Vec<Child>, PathBuf);

impl Drop for Kill {
    fn drop(&mut self) {
        if std::thread::panicking() {
            eprintln!("--- gnome-keyring log ---\n{}", std::fs::read_to_string(&self.1).unwrap_or_default());
        }
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
    tempfile::Builder::new().prefix("mkr").tempdir_in(base).unwrap()
}

/// A private session bus; returns its address.
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

/// A sandboxed gnome-keyring with an empty store under `dir/home`.
fn keyring_daemon(dir: &Path, bus: &str, kids: &mut Kill) {
    let home = dir.join("home");
    let runtime_dir = dir.join("run");
    let control = dir.join("control");
    for d in [&home, &runtime_dir, &control] {
        std::fs::create_dir_all(d).unwrap();
        std::fs::set_permissions(d, std::os::unix::fs::PermissionsExt::from_mode(0o700)).unwrap();
    }
    let log = std::fs::File::create(&kids.1).unwrap();
    let child = Command::new("gnome-keyring-daemon")
        .args(["--foreground", "--components=secrets"])
        .arg(format!("--control-directory={}", control.display()))
        .env_clear()
        .env("PATH", std::env::var_os("PATH").unwrap_or_default())
        .env("HOME", &home)
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("DBUS_SESSION_BUS_ADDRESS", bus)
        .env("DBUS_SYSTEM_BUS_ADDRESS", "unix:path=/nonexistent")
        .env("LANG", "C.UTF-8")
        .stdin(Stdio::null())
        .stdout(log.try_clone().unwrap())
        .stderr(log)
        .spawn()
        .unwrap();
    kids.0.push(child);
}

async fn within<T>(what: &str, fut: impl std::future::Future<Output = T>) -> T {
    futures::select! {
        v = fut.fuse() => v,
        _ = runtime::sleep(STEP).fuse() => panic!("timed out waiting for {what}"),
    }
}

/// The next prompt the UI would show (statuses and closes are skipped).
async fn next_prompt(prompter: &KeyringPrompter, closes: &mut Vec<u64>) -> PromptRequest {
    loop {
        match within("a prompt", prompter.events.recv()).await.unwrap() {
            KeyringEvent::Show(req) => return req,
            KeyringEvent::Close { id } => closes.push(id),
            KeyringEvent::Status(_) => {}
        }
    }
}

async fn proxy<'a>(conn: &zbus::Connection, path: &'a str, iface: &'a str) -> zbus::Proxy<'a> {
    zbus::Proxy::new(conn, SECRETS, path, iface).await.unwrap()
}

/// Runs a Secret Service prompt object; returns (dismissed, result) once
/// `answer` has played the dialog.
async fn run_prompt<F>(conn: &zbus::Connection, path: &OwnedObjectPath, answer: F) -> (bool, OwnedValue)
where
    F: std::future::Future<Output = ()>,
{
    assert_ne!(path.as_str(), "/", "a prompt was expected");
    let prompt = proxy(conn, path.as_str(), PROMPT_IFACE).await;
    let mut completed = prompt.receive_signal("Completed").await.unwrap();
    prompt.call::<_, _, ()>("Prompt", &("",)).await.unwrap();
    answer.await;
    let msg = within("Prompt.Completed", completed.next()).await.unwrap();
    msg.body().deserialize::<(bool, OwnedValue)>().unwrap()
}

async fn locked(conn: &zbus::Connection, collection: &str) -> bool {
    proxy(conn, collection, COLLECTION_IFACE).await.get_property::<bool>("Locked").await.unwrap()
}

#[test]
#[ignore = "spawns a private dbus-daemon and gnome-keyring-daemon"]
fn gnome_keyring_accepts_mandara_prompts() {
    for bin in ["dbus-daemon", "gnome-keyring-daemon"] {
        if which(bin).is_none() {
            eprintln!("skipping: {bin} not installed");
            return;
        }
    }
    let dir = scratch();
    let mut kids = Kill(Vec::new(), dir.path().join("gnome-keyring.log"));
    let bus = private_bus(dir.path(), &mut kids);
    assert!(bus.starts_with("unix:path="), "private bus address: {bus}");

    let prompter = spawn_keyring_prompt_at(&bus);
    runtime::block_on(async {
        // Mandara owns the prompter name on the private bus.
        loop {
            match within("prompter status", prompter.events.recv()).await.unwrap() {
                KeyringEvent::Status(PrompterStatus::Serving) => break,
                KeyringEvent::Status(other) => panic!("prompter status: {other:?}"),
                _ => {}
            }
        }
    });
    keyring_daemon(dir.path(), &bus, &mut kids);

    runtime::block_on(async {
        let conn = runtime::connect(zbus::connection::Builder::address(bus.as_str()).unwrap()).await.unwrap();
        let dbus = zbus::fdo::DBusProxy::new(&conn).await.unwrap();
        within("gnome-keyring on the bus", async {
            while !dbus.name_has_owner(SECRETS.try_into().unwrap()).await.unwrap_or(false) {
                runtime::sleep(Duration::from_millis(100)).await;
            }
        })
        .await;
        let service = proxy(&conn, SERVICE_PATH, SERVICE_IFACE).await;
        let mut closes = Vec::new();

        // 1. A new keyring: password-new prompt, answered once.
        let mut props = std::collections::HashMap::new();
        props.insert("org.freedesktop.Secret.Collection.Label", Value::from(LABEL));
        let (_, prompt): (OwnedObjectPath, OwnedObjectPath) = service.call("CreateCollection", &(props, "")).await.unwrap();
        let mut created_req = None;
        let (dismissed, result) = run_prompt(&conn, &prompt, async {
            let req = next_prompt(&prompter, &mut closes).await;
            assert_eq!(req.kind, PromptKind::Password);
            assert!(req.props.password_new, "creating a keyring asks for a new password: {req:?}");
            prompter.reply(KeyringReply::Password { id: req.id, password: Zeroizing::new(PASSWORD.into()), choice: false });
            created_req = Some(req);
        })
        .await;
        assert!(!dismissed, "create prompt dismissed");
        let collection = OwnedObjectPath::try_from(result).expect("collection path").to_string();
        eprintln!("created {collection}; prompt: {:?}", created_req.map(|r| r.props));
        let keyrings = dir.path().join("home/.local/share/keyrings");
        let files: Vec<_> = std::fs::read_dir(&keyrings).unwrap().flatten().map(|e| e.file_name()).collect();
        assert!(files.iter().any(|f| f.to_string_lossy().ends_with(".keyring")), "keyring file written: {files:?}");
        assert!(!locked(&conn, &collection).await);

        // 2. Lock it, then unlock: wrong password → retry with a warning
        // in the same dialog → right password.
        let path = OwnedObjectPath::try_from(collection.as_str()).unwrap();
        let (_, lock_prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = service.call("Lock", &(vec![path.clone()],)).await.unwrap();
        assert_eq!(lock_prompt.as_str(), "/");
        assert!(locked(&conn, &collection).await);
        let (_, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = service.call("Unlock", &(vec![path.clone()],)).await.unwrap();
        let (dismissed, _) = run_prompt(&conn, &prompt, async {
            let first = next_prompt(&prompter, &mut closes).await;
            assert!(!first.props.password_new);
            assert!(first.props.warning.is_empty());
            eprintln!("unlock prompt: {:?}", first.props);
            prompter.reply(KeyringReply::Password { id: first.id, password: Zeroizing::new(WRONG.into()), choice: false });
            let retry = next_prompt(&prompter, &mut closes).await;
            assert_eq!(retry.id, first.id, "the retry reuses the open dialog");
            assert!(!retry.props.warning.is_empty(), "a wrong password brings a warning");
            eprintln!("retry warning: {:?}", retry.props.warning);
            prompter.reply(KeyringReply::Password { id: retry.id, password: Zeroizing::new(PASSWORD.into()), choice: false });
        })
        .await;
        assert!(!dismissed, "unlock dismissed");
        assert!(!locked(&conn, &collection).await, "gnome-keyring accepted the exchanged password");

        // 3. Cancel: the keyring stays locked.
        let _: (Vec<OwnedObjectPath>, OwnedObjectPath) = service.call("Lock", &(vec![path.clone()],)).await.unwrap();
        let (_, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) = service.call("Unlock", &(vec![path.clone()],)).await.unwrap();
        let (dismissed, _) = run_prompt(&conn, &prompt, async {
            let req = next_prompt(&prompter, &mut closes).await;
            prompter.reply(KeyringReply::Cancel { id: req.id });
        })
        .await;
        assert!(dismissed, "cancel dismisses the prompt");
        assert!(locked(&conn, &collection).await);

        // Every dialog was closed again (StopPrompting → Close).
        within("the last close", async {
            while closes.len() < 3 {
                if let Ok(KeyringEvent::Close { id }) = prompter.events.recv().await {
                    closes.push(id);
                }
            }
        })
        .await;
        prompter.stop();
    });
    drop(kids);
}
