//! Run explicitly: starts its own bus, home and Secret Service, then checks
//! the same keyring path used by Studio. It never opens the desktop bus.

use std::io::Write as _;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use mandara_services::credentials::{Access, Key, Keyring, Provider, Secret, Store as _};

const CHILD: &str = "MANDARA_PRIVATE_KEYRING_TEST";
const TEST: &str = "scoped_keys_on_private_secret_service";
const STARTUP_LIMIT: Duration = Duration::from_secs(8);
const STARTUP_POLL: Duration = Duration::from_millis(25);

struct Daemon(Child);

impl Drop for Daemon {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "Needs dbus-run-session and gnome-keyring-daemon; creates a private Secret Service"]
fn scoped_keys_on_private_secret_service() {
    if std::env::var_os(CHILD).is_none() {
        let home = tempfile::tempdir().unwrap();
        let runtime = tempfile::tempdir().unwrap();
        let status = Command::new("dbus-run-session")
            .arg("--").arg(std::env::current_exe().unwrap())
            .args(["--exact", TEST, "--ignored", "--nocapture"])
            .env_clear()
            .env("PATH", "/usr/bin:/bin")
            .env("HOME", home.path())
            .env("XDG_DATA_HOME", home.path().join("data"))
            .env("XDG_DATA_DIRS", home.path().join("no-activation-services"))
            .env("XDG_CONFIG_HOME", home.path().join("config"))
            .env("XDG_CACHE_HOME", home.path().join("cache"))
            .env("XDG_RUNTIME_DIR", runtime.path())
            .env("DBUS_SYSTEM_BUS_ADDRESS", "unix:path=/nonexistent")
            .env(CHILD, "1")
            .status().unwrap();
        assert!(status.success());
        return;
    }
    assert_eq!(std::env::var(CHILD).unwrap(), "1");
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").unwrap();
    let control = std::path::Path::new(&runtime).join("keyring");
    std::fs::create_dir_all(&control).unwrap();
    let child = Command::new("gnome-keyring-daemon")
        .args(["--foreground", "--unlock", "--components=secrets", "--control-directory"])
        .arg(control)
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped())
        .spawn().unwrap();
    let mut daemon = Daemon(child);
    daemon.0.stdin.take().unwrap().write_all(b"mandara-private-fixture").unwrap();
    let first = Key { provider: Provider::HomeAssistant, server: "https://fixture.invalid/proxy".into() };
    let second = Key { server: "https://other.invalid".into(), ..first.clone() };
    let provider = Key { provider: Provider::LibreTranslate, ..first.clone() };
    let started = Instant::now();
    let bus = mandara_services::runtime::block_on(zbus::Connection::session()).unwrap();
    let proxy = mandara_services::runtime::block_on(zbus::Proxy::new(&bus, "org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus")).unwrap();
    while !mandara_services::runtime::block_on(proxy.call::<_, _, bool>("NameHasOwner", &("org.freedesktop.secrets",))).unwrap() {
        assert!(started.elapsed() < STARTUP_LIMIT, "private daemon did not acquire its bus name");
        assert!(daemon.0.try_wait().unwrap().is_none(), "Secret Service exited before acquiring its bus name");
        std::thread::sleep(STARTUP_POLL);
    }
    loop {
        match Keyring.read(&first, Access::Quiet) {
            Ok(None) => break,
            result if started.elapsed() < STARTUP_LIMIT => {
                assert!(daemon.0.try_wait().unwrap().is_none(), "Secret Service exited: {result:?}");
                std::thread::sleep(STARTUP_POLL);
            }
            result => panic!("private Secret Service did not start: {result:?}"),
        }
    }
    Keyring.save(&first, &Secret::new("fixture-one".into()).unwrap()).unwrap();
    Keyring.save(&second, &Secret::new("fixture-two".into()).unwrap()).unwrap();
    Keyring.save(&provider, &Secret::new("fixture-three".into()).unwrap()).unwrap();
    assert_eq!(Keyring.read(&first, Access::Quiet).unwrap().unwrap().expose(), "fixture-one");
    mandara_services::runtime::block_on(async {
        let service = oo7::dbus::Service::new().await.unwrap();
        let collection = service.with_alias(oo7::dbus::Service::DEFAULT_COLLECTION).await.unwrap().unwrap();
        let attributes = [("application", "mandara-shell"), ("purpose", "plugin-api-key"), ("provider", first.provider.id()), ("server", first.server.as_str())];
        collection.create_item("duplicate fixture", &attributes, oo7::Secret::text("fixture-duplicate"), false, None).await.unwrap();
    });
    assert!(Keyring.read(&first, Access::Quiet).is_err());
    Keyring.save(&first, &Secret::new("fixture-replaced".into()).unwrap()).unwrap();
    assert_eq!(Keyring.read(&first, Access::Quiet).unwrap().unwrap().expose(), "fixture-replaced");
    let start = std::sync::Barrier::new(4);
    std::thread::scope(|scope| {
        for index in 0..4 {
            let start = &start;
            let key = &first;
            scope.spawn(move || {
                let secret = Secret::new(format!("fixture-concurrent-{index}")).unwrap();
                start.wait();
                Keyring.save(key, &secret).unwrap();
            });
        }
    });
    let saved = Keyring.read(&first, Access::Quiet).unwrap().unwrap();
    assert!((0..4).any(|index| saved.expose() == format!("fixture-concurrent-{index}")));
    Keyring.remove(&first).unwrap();
    assert!(Keyring.read(&first, Access::Quiet).unwrap().is_none());
    assert_eq!(Keyring.read(&second, Access::Quiet).unwrap().unwrap().expose(), "fixture-two");
    assert_eq!(Keyring.read(&provider, Access::Quiet).unwrap().unwrap().expose(), "fixture-three");
    Keyring.remove(&second).unwrap();
    Keyring.remove(&provider).unwrap();
}
