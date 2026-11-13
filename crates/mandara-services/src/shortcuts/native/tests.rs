//! Opt-in checks against installed compositors, isolated from the desktop.

use super::*;
use doctor::CheckerStatus;

#[test]
#[ignore = "requires the selected compositor's native checker"]
fn native_checkers_validate_shortcuts_without_running_configuration_hooks() {
    let target =
        Target::parse(&std::env::var("MANDARA_CHECK_COMPOSITOR").expect("select a native checker"))
            .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let sentinel = directory.path().join("startup-ran");
    let touch = format!("touch {}", sentinel.display());
    let (name, valid, invalid) = match target {
        Target::Mango => ("config.conf", format!("exec_once={touch}\nbind=SUPER,Q,killclient\n"), format!("exec_once={touch}\nbind=SUPER,ThisKeyDoesNotExist,killclient\n")),
        Target::Niri => ("config.kdl", format!("spawn-at-startup \"touch\" \"{}\"\nbinds {{ Super+Q {{ close-window; }}; }}\n", sentinel.display()), "binds { Super+ThisKeyDoesNotExist { close-window; }; }\n".into()),
        Target::Hyprland => ("hyprland.lua", format!("os.execute('{touch}')\nhl.bind('SUPER + Q', hl.dsp.window.close())\n"), "hl.bind('SUPER + ThisKeyDoesNotExist', hl.dsp.window.close())\n".into()),
        Target::Umbriel => ("config.toml", format!("[general]\nautostart=[\"{touch}\"]\n[keybinds]\n\"Super+Q\"=\"window-close\"\n"), "[keybinds]\n\"Super+Q\"=\"this-action-does-not-exist\"\n".into()),
        Target::Sway => ("config", format!("exec {touch}\nbindsym Mod4+q kill\n"), "bindsym Mod4+ThisKeyDoesNotExist kill\n".into()),
        Target::River => ("init", format!("{touch}\nriverctl map normal Super Q close\n"), "riverctl map normal Super ThisKeyDoesNotExist close\n".into()),
        Target::Labwc => ("rc.xml", format!("<labwc_config><keyboard><keybind key=\"W-q\"><action name=\"Execute\"><command>{touch}</command></action></keybind></keyboard></labwc_config>"), "<labwc_config><keyboard><keybind key=\"W-ThisKeyDoesNotExist\"><action name=\"Close\"/></keybind></keyboard></labwc_config>".into()),
        _ => panic!("this compositor uses its typed settings parser or live shortcut service"),
    };
    let file = directory.path().join(name);
    let check = |source: &str| -> Result<()> {
        std::fs::write(&file, source)?;
        let rows = keybinds::parse(target, &file, source)?;
        match target {
            Target::Hyprland => {
                let session = Session {
                    target,
                    root: file.clone(),
                    scan: keybinds::scan_from(target, &file),
                    primary_mod_is_alt: false,
                };
                private::hyprland(&session, &rows[0])
            }
            Target::River => private::river(&rows[0]),
            Target::Labwc => private::labwc(source),
            _ => {
                let report = doctor::check_with(target, &file, &CheckEnv::default());
                ensure!(
                    report.status == CheckerStatus::Ran,
                    "native checker did not run: {}",
                    report.raw
                );
                ensure!(report.problems() == 0, "{}", report.raw);
                Ok(())
            }
        }
    };
    check(&valid)
        .unwrap_or_else(|error| panic!("valid {target:?} shortcut was rejected: {error:#}"));
    assert!(
        !sentinel.exists(),
        "configuration startup code ran during validation"
    );
    assert!(
        check(&invalid).is_err(),
        "invalid {target:?} shortcut was accepted"
    );
    assert!(!sentinel.exists());
    assert_eq!(std::fs::read_to_string(file).unwrap(), invalid);
}
