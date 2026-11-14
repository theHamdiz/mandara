//! Keep descriptor-lifetime assertions separate from unrelated forking tests.

pub(crate) fn isolated(name: &str) {
    let home = tempfile::tempdir().unwrap();
    let runtime = home.path().join("runtime");
    std::fs::create_dir(&runtime).unwrap();
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.env_clear().env("HOME", home.path()).env("TMPDIR", home.path())
        .env("XDG_CONFIG_HOME", home.path().join("config"))
        .env("XDG_CACHE_HOME", home.path().join("cache"))
        .env("XDG_DATA_HOME", home.path().join("data"))
        .env("XDG_STATE_HOME", home.path().join("state"))
        .env("XDG_RUNTIME_DIR", runtime)
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
        .env("DBUS_SYSTEM_BUS_ADDRESS", "unix:path=/nonexistent")
        .args(["--exact", name, "--ignored", "--nocapture", "--test-threads=1"]);
    crate::app_themes::process::native(command, &std::sync::atomic::AtomicBool::new(false), crate::app_themes::process::TIMEOUT)
        .unwrap_or_else(|error| panic!("isolated service test {name}: {error:#}"));
}
