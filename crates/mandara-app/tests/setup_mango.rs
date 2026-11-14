//! Setup checks must describe broken startup without touching the live desktop.

use std::{
    path::PathBuf,
    process::{Command, Output, Stdio},
};

struct Fixture {
    root: tempfile::TempDir,
    home: PathBuf,
    config: PathBuf,
    programs: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let config = root.path().join("xdg");
        let programs = root.path().join("bin");
        for path in [&home, &config, &programs] {
            std::fs::create_dir(path).unwrap();
        }
        std::os::unix::fs::symlink(env!("CARGO_BIN_EXE_mandara"), programs.join("mandara"))
            .unwrap();
        Self {
            root,
            home,
            config,
            programs,
        }
    }

    fn run(&self, args: &[&str]) -> Output {
        let paths = std::env::join_paths(std::iter::once(self.programs.clone()).chain(
            std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
        ))
        .unwrap();
        Command::new(env!("CARGO_BIN_EXE_mandara"))
            .args(args)
            .env_clear()
            .env("PATH", paths)
            .env("LC_ALL", "C")
            .env("HOME", &self.home)
            .env("XDG_CONFIG_HOME", &self.config)
            .env("XDG_DATA_HOME", self.root.path().join("data"))
            .env("XDG_CACHE_HOME", self.root.path().join("cache"))
            .env("XDG_RUNTIME_DIR", self.root.path())
            .env("DBUS_SYSTEM_BUS_ADDRESS", "unix:path=/nonexistent")
            .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
            .env("MANDARA_NO_IPC", "1")
            .stdin(Stdio::null())
            .output()
            .unwrap()
    }

    fn config_file(&self, extension: &str) -> PathBuf {
        self.home
            .join(".config/mango")
            .join(format!("config.{extension}"))
    }
}

#[test]
fn missing_mango_configuration_is_reported_as_an_error_without_creating_it() {
    let fixture = Fixture::new();
    let output = fixture.run(&["setup", "--compositor", "mango", "--check"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        output.status.code(),
        Some(11),
        "{stdout}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        stdout.contains("No compositor configuration was found"),
        "{stdout}"
    );
    assert!(
        stdout.contains(fixture.config_file("conf").to_str().unwrap()),
        "{stdout}"
    );
    assert!(!fixture.config_file("conf").exists());
    assert!(!fixture.config_file("toml").exists());
    assert!(!fixture.config.join("mandara/generated").exists());
}

#[test]
fn mango_setup_connects_native_conf_and_toml_to_a_separate_xdg_directory() {
    if Command::new("mango").arg("-v").output().is_err() {
        return;
    }
    for (extension, original) in [
        (
            "conf",
            "# My desktop\nborder_radius=6\nbind=SUPER,Return,spawn,foot\n",
        ),
        (
            "toml",
            "# My desktop\n[global]\nborder_radius = 6\n[bind.default]\n'SUPER+Return' = 'spawn,foot'\n",
        ),
    ] {
        let fixture = Fixture::new();
        let main = fixture.config_file(extension);
        std::fs::create_dir_all(main.parent().unwrap()).unwrap();
        std::fs::write(&main, original).unwrap();
        let kept = fixture.home.join("keep.txt");
        std::fs::write(&kept, "unrelated data").unwrap();
        let legacy_backup = main.with_extension(format!("{extension}.mandara-backup"));
        std::os::unix::fs::symlink(&kept, &legacy_backup).unwrap();
        let output = fixture.run(&["setup", "--compositor", "mango", "--autostart"]);
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        let generated = fixture.config.join("mandara/generated/mango.conf");
        let dropin = std::fs::read_to_string(&generated).unwrap();
        assert!(
            dropin.contains(&format!(
                "exec_once={}",
                fixture.programs.join("mandara").display()
            )),
            "{dropin}"
        );
        assert!(!fixture.config.join("mango").exists());
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "unrelated data");
        assert_eq!(std::fs::read_link(&legacy_backup).unwrap(), kept);
        let backups = fixture.root.path().join("data/mandara/doctor-backups");
        let backup = std::fs::read_dir(backups)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert_eq!(
            std::fs::read_to_string(backup.join(format!("0-config.{extension}"))).unwrap(),
            original
        );
        let updated = std::fs::read_to_string(&main).unwrap();
        assert!(updated.contains("# My desktop"));
        assert!(updated.contains(generated.to_str().unwrap()), "{updated}");
        if extension == "toml" {
            assert!(updated.contains("border_radius = 6"));
            assert!(updated.contains("'SUPER+Return' = 'spawn,foot'"));
            assert!(!fixture.config_file("conf").exists());
        } else {
            assert!(updated.starts_with(original));
        }
        let output = fixture.run(&["setup", "--compositor", "mango", "--check"]);
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(!output.stdout.is_empty());
        assert_eq!(std::fs::read_to_string(&main).unwrap(), updated);
    }
}

#[test]
fn missing_generated_autostart_is_detected_and_setup_repairs_it() {
    if Command::new("mango").arg("-v").output().is_err() {
        return;
    }
    let fixture = Fixture::new();
    let main = fixture.config_file("conf");
    std::fs::create_dir_all(main.parent().unwrap()).unwrap();
    std::fs::write(&main, "border_radius=6\n").unwrap();
    let setup = ["setup", "--compositor", "mango", "--autostart"];
    let first = fixture.run(&setup);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let original = std::fs::read_to_string(&main).unwrap();
    let generated = fixture.config.join("mandara/generated/mango.conf");
    std::fs::remove_file(&generated).unwrap();
    let check = fixture.run(&["setup", "--compositor", "mango", "--check"]);
    let stdout = String::from_utf8_lossy(&check.stdout);
    assert_eq!(check.status.code(), Some(11), "{stdout}");
    assert!(
        stdout.contains("Autostart is enabled but its generated configuration is not connected"),
        "{stdout}"
    );
    assert!(
        !generated.exists(),
        "a read-only check must not repair files"
    );
    let repaired = fixture.run(&setup);
    assert!(
        repaired.status.success(),
        "{}",
        String::from_utf8_lossy(&repaired.stderr)
    );
    assert!(generated.exists());
    assert_eq!(std::fs::read_to_string(&main).unwrap(), original);
    assert!(
        fixture
            .run(&["setup", "--compositor", "mango", "--check"])
            .status
            .success()
    );
}
