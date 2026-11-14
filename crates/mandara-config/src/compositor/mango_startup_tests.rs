use super::*;

#[test]
fn mango_uses_its_native_home_even_with_a_separate_xdg_config_directory() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let config = directory.path().join("xdg");
    let expected = home.join(".config/mango/config.conf");
    std::fs::create_dir_all(expected.parent().unwrap()).unwrap();
    std::fs::create_dir_all(config.join("mango")).unwrap();
    std::fs::write(&expected, "border_radius=6\n").unwrap();
    std::fs::write(config.join("mango/config.conf"), "border_radius=12\n").unwrap();

    assert_eq!(Target::Mango.user_config_in(&config, &home), expected);
    assert_eq!(
        crate::keybinds::default_root_in(Target::Mango, &config, &home),
        expected
    );
}

#[test]
fn mango_setup_finds_toml_without_shadowing_it_with_a_new_conf_file() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join(".config");
    let toml = config.join("mango/config.toml");
    let legacy = config.join("mango/config.conf");
    std::fs::create_dir_all(toml.parent().unwrap()).unwrap();
    std::fs::write(&toml, "[global]\nborder_radius = 6\n").unwrap();

    assert_eq!(
        Target::Mango.user_config_in(&config, directory.path()),
        toml
    );
    std::fs::write(&legacy, "border_radius=12\n").unwrap();
    assert_eq!(
        Target::Mango.user_config_in(&config, directory.path()),
        legacy
    );
}

#[test]
fn mango_toml_include_preserves_the_users_global_settings_and_bindings() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("config.toml");
    let original = "# My desktop\n[global]\nborder_radius = 6 # keep\nsource_optional = [\"./colors.conf\"]\n\n[bind.default]\n\"SUPER+Return\" = \"spawn,foot\"\n";
    std::fs::write(&file, original).unwrap();

    assert!(install_include_at(Target::Mango, &file).unwrap());
    let updated = std::fs::read_to_string(&file).unwrap();
    let parsed: toml::Table = updated.parse().expect("MangoWM's TOML must remain valid");
    assert_eq!(parsed["global"]["border_radius"].as_integer(), Some(6));
    assert_eq!(
        parsed["bind"]["default"]["SUPER+Return"].as_str(),
        Some("spawn,foot")
    );
    assert!(updated.contains("border_radius = 6 # keep"));
    let includes = parsed["global"]["source_optional"].as_array().unwrap();
    assert_eq!(includes[0].as_str(), Some("./colors.conf"));
    assert_eq!(includes.len(), 2);
    assert!(include_installed_at(Target::Mango, &file));
    assert!(!install_include_at(Target::Mango, &file).unwrap());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), updated);
}

#[test]
fn mango_config_format_follows_native_extensions_and_sniffing() {
    for (name, source, toml) in [
        ("config.toml", "", true),
        ("config.TOML", "", true),
        ("config.conf", "[global]\n", false),
        ("config.CONF", "[global]\n", false),
        ("config", "# Comment\n\n [global]\n", true),
        ("config", "border_radius=6\n", false),
    ] {
        assert_eq!(mango_toml(Path::new(name), source), toml, "{name}");
    }
}

#[test]
fn include_backups_preserve_existing_legacy_backup_symlinks() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("config.toml");
    let original = "[general]\nmod_key = 'Super'\n";
    std::fs::write(&file, original).unwrap();
    let kept = directory.path().join("keep.txt");
    std::fs::write(&kept, "unrelated data").unwrap();
    let legacy = directory.path().join("config.toml.mandara-backup");
    std::os::unix::fs::symlink(&kept, &legacy).unwrap();
    assert!(install_include_at(Target::Umbriel, &file).unwrap());
    assert_eq!(std::fs::read_to_string(&kept).unwrap(), "unrelated data");
    assert_eq!(std::fs::read_link(&legacy).unwrap(), kept);
    let backup = std::fs::read_dir(directory.path())
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("config.toml.mandara-backup-")
        })
        .unwrap();
    assert_eq!(std::fs::read_to_string(backup).unwrap(), original);
}

#[test]
fn mango_toml_includes_keep_scalar_sources_inline_tables_and_existing_comments() {
    for original in [
        "[global]\nsource_optional = './colors.conf' # keep this source\n",
        "global = { source_optional = ['./colors.conf'], border_radius = 7 }\n",
        "# No global section\n[bind.default]\n'SUPER+Return' = 'spawn,foot'\n",
    ] {
        let path = "/tmp/mandara/generated/mango.conf";
        let updated = mango_config::add_include(original, path).unwrap().unwrap();
        let parsed: toml::Table = updated.parse().unwrap();
        assert!(
            parsed["global"]["source_optional"]
                .as_array()
                .unwrap()
                .iter()
                .any(|value| value.as_str() == Some(path))
        );
        assert!(mango_config::add_include(&updated, path).unwrap().is_none());
        if original.contains("keep this source") {
            assert!(updated.contains("# keep this source"));
        }
    }
    for invalid in [
        "global = 3\n",
        "[global]\nsource_optional = 3\n",
        "[global\n",
    ] {
        assert!(mango_config::add_include(invalid, "/tmp/mandara/generated/mango.conf").is_err());
    }
}

#[test]
fn mango_seed_copies_both_system_formats_once_and_can_be_undone() {
    if std::process::Command::new("mango")
        .arg("-v")
        .output()
        .is_err()
    {
        return;
    }
    for (extension, original) in [
        ("conf", "border_radius=6\n"),
        ("toml", "[global]\nborder_radius = 6\n"),
    ] {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let system = directory.path().join("system");
        std::fs::create_dir(&system).unwrap();
        let source = system.join(format!("config.{extension}"));
        std::fs::write(&source, original).unwrap();
        let store = crate::doctor::fix::Store {
            backups: directory.path().join("backups"),
            unsupported: directory.path().join("unsupported.json"),
        };
        assert!(
            mango_seed::seed_in(&home, &system, &crate::doctor::CheckEnv::default(), &store)
                .unwrap()
        );
        let destination = mango_config::user_file(&home);
        assert_eq!(destination.extension().unwrap(), extension);
        assert_eq!(std::fs::read_to_string(&destination).unwrap(), original);
        assert_eq!(std::fs::read_to_string(&source).unwrap(), original);
        assert!(
            !mango_seed::seed_in(&home, &system, &crate::doctor::CheckEnv::default(), &store)
                .unwrap()
        );
        let backup = std::fs::read_dir(&store.backups)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        crate::doctor::fix::restore(&backup).unwrap();
        assert!(!destination.exists());
        assert_eq!(std::fs::read_to_string(&source).unwrap(), original);
    }
}

#[test]
fn mango_seed_keeps_existing_and_broken_symlink_configs() {
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let destination = home.join(".config/mango/config.toml");
    std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink("missing.toml", &destination).unwrap();
    let store = crate::doctor::fix::Store {
        backups: directory.path().join("backups"),
        unsupported: directory.path().join("unsupported.json"),
    };
    assert!(
        !mango_seed::seed_in(
            &home,
            &directory.path().join("system"),
            &crate::doctor::CheckEnv::default(),
            &store
        )
        .unwrap()
    );
    assert_eq!(
        std::fs::read_link(&destination).unwrap(),
        Path::new("missing.toml")
    );
    assert!(!destination.with_extension("conf").exists());
    assert!(!store.backups.exists());
}

#[test]
fn mango_seed_rolls_back_when_relative_includes_break_in_the_new_location() {
    if std::process::Command::new("mango")
        .arg("-v")
        .output()
        .is_err()
    {
        return;
    }
    let directory = tempfile::tempdir().unwrap();
    let home = directory.path().join("home");
    let system = directory.path().join("system");
    std::fs::create_dir(&system).unwrap();
    let original = "source=./colors.conf\n";
    std::fs::write(system.join("config.conf"), original).unwrap();
    std::fs::write(system.join("colors.conf"), "border_radius=6\n").unwrap();
    let store = crate::doctor::fix::Store {
        backups: directory.path().join("backups"),
        unsupported: directory.path().join("unsupported.json"),
    };
    assert!(
        mango_seed::seed_in(&home, &system, &crate::doctor::CheckEnv::default(), &store).is_err()
    );
    assert!(!mango_config::user_file(&home).exists());
    assert_eq!(
        std::fs::read_to_string(system.join("config.conf")).unwrap(),
        original
    );
}
