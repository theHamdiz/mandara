//! The installer must start before its GUI runtime packages exist.

#[test]
fn the_binary_does_not_require_keyboard_libraries_at_process_start() {
    let report = std::process::Command::new("readelf")
        .args(["--dynamic", env!("CARGO_BIN_EXE_mandara")]).env("LC_ALL", "C").output().unwrap();
    assert!(report.status.success(), "{}", String::from_utf8_lossy(&report.stderr));
    let report = String::from_utf8(report.stdout).unwrap();
    let needed: Vec<_> = report.lines().filter(|line| line.contains("(NEEDED)")).collect();
    assert!(!needed.is_empty(), "expected the Linux C runtime dependencies");
    assert!(!needed.iter().any(|line| line.contains("libxkbcommon")), "{report}");
}

#[test]
fn installer_help_uses_the_terminal_language_pack_before_parsing() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let version = mandara_version::version();
    let packs = data.join("mandara/locales").join(version);
    std::fs::create_dir_all(&packs).unwrap();
    let short = "Pilihan compositor dari paket uji";
    let long = "Pilih compositor. KWin membutuhkan Plasma dari paket uji.";
    let bytes = mandara_i18n::pack::build(mandara_i18n::Lang::Id, version, [
        ("Compositor (default: detected)", short),
        ("Choose mango, niri, hyprland, umbriel, sway, river or kwin (default: detected). Missing compositors can be installed after asking. KWin requires an existing Plasma installation.", long),
    ]).unwrap();
    std::fs::write(packs.join("id.mlang"), bytes).unwrap();
    for (flag, expected) in [("-h", short), ("--help", long)] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_mandara"))
            .args(["install", flag]).env("LC_ALL", "id_ID.UTF-8").env("XDG_DATA_HOME", &data)
            .env("XDG_CONFIG_HOME", dir.path().join("config")).env("HOME", dir.path())
            .stdin(std::process::Stdio::null()).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
        assert!(String::from_utf8_lossy(&output.stdout).contains(expected), "{}", String::from_utf8_lossy(&output.stdout));
    }
    assert!(!dir.path().join("config").exists(), "help must not create a configuration");
}

#[test]
fn setup_reports_a_missing_keyboard_library_without_panicking_or_writing_config() {
    let dir = tempfile::tempdir().unwrap();
    let library = dir.path().join("library");
    let config = dir.path().join("config");
    std::fs::create_dir(&library).unwrap();
    std::fs::create_dir_all(config.join("mango")).unwrap();
    std::fs::write(library.join("libxkbcommon.so.0"), "invalid ELF fixture").unwrap();
    let mango = config.join("mango/config.conf");
    let original = "bind=SUPER,space,spawn,fuzzel\n";
    std::fs::write(&mango, original).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_mandara"))
        .args(["setup", "--compositor", "mango", "--check"])
        .env("LD_LIBRARY_PATH", &library).env("LC_ALL", "C").env("LANG", "id_ID.UTF-8")
        .env("XDG_CONFIG_HOME", &config).env("HOME", dir.path()).env("MANDARA_NO_IPC", "1")
        .stdin(std::process::Stdio::null()).output().unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(12), "{stderr}");
    assert!(stderr.contains("Install libxkbcommon"), "{stderr}");
    assert!(!stderr.contains("panicked"), "{stderr}");
    assert_eq!(std::fs::read_to_string(mango).unwrap(), original);
    assert!(!config.join("mandara").exists(), "the prerequisite check must precede configuration writes");
}

#[test]
fn kwin_setup_check_reports_availability_without_touching_compositor_files() {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config");
    let programs = dir.path().join("bin");
    std::fs::create_dir_all(&config).unwrap();
    std::fs::create_dir_all(&programs).unwrap();
    let kwinrc = config.join("kwinrc");
    std::fs::write(&kwinrc, "[Desktops]\nNumber=3\n").unwrap();
    let success = mandara_services::recording::backend::which("true").expect("true on PATH");
    let failure = mandara_services::recording::backend::which("false").expect("false on PATH");
    for (program, expected_status) in [(None, 11), (Some(success.as_path()), 0), (Some(failure.as_path()), 11)] {
        let probe = programs.join("kwin_wayland");
        if probe.exists() { std::fs::remove_file(&probe).unwrap(); }
        if let Some(program) = program { std::os::unix::fs::symlink(program, &probe).unwrap(); }
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_mandara"))
            .args(["setup", "--check", "--compositor", "kwin"])
            .env("PATH", &programs).env("LC_ALL", "C")
            .env("HOME", dir.path()).env("XDG_CONFIG_HOME", &config)
            .env("XDG_DATA_HOME", dir.path().join("data"))
            .env("XDG_CACHE_HOME", dir.path().join("cache"))
            .env("XDG_RUNTIME_DIR", dir.path()).env("MANDARA_NO_IPC", "1")
            .stdin(std::process::Stdio::null()).output().unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(output.status.code(), Some(expected_status), "{stdout}");
        if program == Some(success.as_path()) { assert!(stdout.contains("KWin has no standalone configuration checker"), "{stdout}"); }
        if program.is_none() { assert!(stdout.contains("isn't installed"), "{stdout}"); }
        assert_eq!(std::fs::read_to_string(&kwinrc).unwrap(), "[Desktops]\nNumber=3\n");
        assert!(!config.join("autostart").exists());
        assert!(!config.join("mandara/generated").exists());
    }
}
