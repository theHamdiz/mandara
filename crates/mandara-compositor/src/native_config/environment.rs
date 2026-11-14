//! Native lookup uses the Wayland server's environment, never a launch script.

use super::{NativeConfig, from_arguments};
use anyhow::{Result, ensure};
use mandara_config::{compositor::Target, keybinds};
use std::{
    ffi::{OsStr, OsString},
    os::unix::ffi::{OsStrExt as _, OsStringExt as _},
    path::{Path, PathBuf},
};

#[cfg(target_os = "linux")]
pub(super) fn capture(environment: &[u8]) -> mandara_config::compositor::IncludeEnvironment {
    mandara_config::compositor::IncludeEnvironment::captured(
        environment.split(|byte| *byte == 0).filter_map(|entry| {
            let at = entry.iter().position(|byte| *byte == b'=')?;
            Some((
                OsString::from_vec(entry[..at].to_vec()),
                OsString::from_vec(entry[at + 1..].to_vec()),
            ))
        }),
    )
}

pub(super) fn value<'a>(environment: &'a [u8], key: &[u8]) -> Option<&'a OsStr> {
    environment.split(|byte| *byte == 0).find_map(|entry| {
        let at = entry.iter().position(|byte| *byte == b'=')?;
        (entry[..at] == *key && at + 1 < entry.len()).then(|| OsStr::from_bytes(&entry[at + 1..]))
    })
}

pub(super) fn inspect(
    target: Target,
    arguments: &[OsString],
    environment: &[u8],
    cwd: &Path,
) -> Result<NativeConfig> {
    if let Some(file) = from_arguments(target, arguments, cwd)? {
        return Ok(NativeConfig {
            file: Some(file),
            explicit_file: true,
            ..Default::default()
        });
    }
    let absolute = |path: &OsStr| {
        let path = PathBuf::from(path);
        if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        }
    };
    if target == Target::Niri
        && let Some(path) = value(environment, b"NIRI_CONFIG")
    {
        return Ok(NativeConfig {
            file: Some(absolute(path)),
            ..Default::default()
        });
    }
    let home = value(environment, b"HOME").map(absolute);
    let config = value(environment, b"XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .or_else(|| home.as_ref().map(|home| home.join(".config")));
    let Some(config) = config else {
        return Ok(NativeConfig::default());
    };
    let home = home.as_deref().unwrap_or(cwd);
    let root = keybinds::default_root_in(target, &config, home);
    if target == Target::Labwc {
        let directories = value(environment, b"XDG_CONFIG_DIRS").unwrap_or(OsStr::new("/etc/xdg"));
        let mut files = vec![root.clone()];
        for directory in std::env::split_paths(directories).filter(|path| path.is_absolute()) {
            ensure!(
                files.len() < keybinds::MAX_FILES,
                "too many native configuration directories"
            );
            files.push(directory.join("labwc/rc.xml"));
        }
        files.retain(|file| file.is_file());
        if arguments
            .iter()
            .skip(1)
            .take_while(|arg| *arg != "--")
            .any(|arg| arg == "--merge-config" || arg == "-m")
        {
            files.reverse();
        } else {
            files.truncate(1);
        }
        let mut files = files.into_iter();
        return Ok(NativeConfig {
            file: Some(files.next().unwrap_or(root)),
            additional_files: files.collect(),
            ..Default::default()
        });
    }
    let file = if root.is_file() {
        root
    } else {
        let system = match target {
            Target::Mango => ["/etc/mango/config.conf", "/etc/mango/config.toml"].as_slice(),
            Target::Niri => ["/etc/niri/config.kdl"].as_slice(),
            Target::Sway => [mandara_config::compositor::SWAY_SYSTEM_CONFIG].as_slice(),
            _ => &[],
        };
        system
            .iter()
            .map(PathBuf::from)
            .find(|path| path.is_file())
            .unwrap_or(root)
    };
    Ok(NativeConfig {
        file: Some(file),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mango_follows_its_home_and_distinguishes_explicit_config_arguments() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path().join("home");
        let config = directory.path().join("xdg");
        let file = home.join(".config/mango/config.toml");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::create_dir_all(config.join("mango")).unwrap();
        std::fs::write(&file, "[global]\nborder_radius = 6\n").unwrap();
        std::fs::write(config.join("mango/config.conf"), "border_radius=12\n").unwrap();
        let environment = format!("HOME={}\0XDG_CONFIG_HOME={}\0", home.display(), config.display());
        let default = inspect(Target::Mango, &["mango".into()], environment.as_bytes(), directory.path()).unwrap();
        assert_eq!(default.file.as_ref(), Some(&file));
        assert!(!default.explicit_file);
        let explicit = inspect(Target::Mango, &["mango".into(), "-c".into(), file.clone().into_os_string()], environment.as_bytes(), directory.path()).unwrap();
        assert_eq!(explicit.file.as_ref(), Some(&file));
        assert!(explicit.explicit_file);
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn captured_includes_preserve_server_paths_and_keep_values_private() {
        let directory = tempfile::tempdir().unwrap();
        let server = directory
            .path()
            .join(OsString::from_vec(b"keys=\xff".to_vec()));
        std::fs::create_dir(&server).unwrap();
        let root = directory.path().join("config.conf");
        let child = server.join("child.conf");
        std::fs::write(&root, "source=$KEYS/child.conf\n").unwrap();
        std::fs::write(&child, "bind=SUPER,Q,killclient\n").unwrap();
        let mut bytes = b"malformed\0KEYS=".to_vec();
        bytes.extend_from_slice(server.as_os_str().as_bytes());
        bytes.extend_from_slice(b"\0TOKEN=private-value\0");
        let environment = capture(&bytes);
        let scan = keybinds::scan_roots_in(Target::Mango, [root], &environment);
        assert!(scan.errors.is_empty(), "{:?}", scan.errors);
        assert!(scan.sources.iter().any(|source| source.file == child));
        assert!(!format!("{environment:?}").contains("private-value"));
    }

    #[test]
    fn server_home_xdg_and_niri_overrides_keep_native_precedence_and_non_utf8_paths() {
        let directory = tempfile::tempdir().unwrap();
        let home = directory.path();
        let environment = format!(
            "HOME={}\0XDG_CONFIG_HOME=relative\0NIRI_CONFIG=keys.kdl\0",
            home.display()
        );
        let args = [OsString::from("niri")];
        assert_eq!(
            inspect(Target::Niri, &args, environment.as_bytes(), home)
                .unwrap()
                .file,
            Some(home.join("keys.kdl"))
        );
        let args = ["niri".into(), "--config=explicit.kdl".into()];
        assert_eq!(
            inspect(Target::Niri, &args, environment.as_bytes(), home)
                .unwrap()
                .file,
            Some(home.join("explicit.kdl"))
        );
        let mut environment = format!("HOME={}\0NIRI_CONFIG=", home.display()).into_bytes();
        environment.extend_from_slice(b"/custom/\xff.kdl\0");
        let path = inspect(Target::Niri, &["niri".into()], &environment, home)
            .unwrap()
            .file
            .unwrap();
        assert_eq!(
            path.into_os_string(),
            OsString::from_vec(b"/custom/\xff.kdl".to_vec())
        );
        let environment = format!("HOME={}\0", home.display());
        assert_eq!(
            inspect(
                Target::KWin,
                &["kwin_wayland".into()],
                environment.as_bytes(),
                home
            )
            .unwrap()
            .file,
            Some(home.join(".config/kglobalshortcutsrc"))
        );
    }

    #[test]
    fn labwc_merge_reads_all_roots_in_native_order_and_explicit_config_stays_single() {
        let directory = tempfile::tempdir().unwrap();
        let user = directory.path().join("user");
        let first = directory.path().join("first");
        let second = directory.path().join("second");
        for path in [&user, &first, &second] {
            std::fs::create_dir_all(path.join("labwc")).unwrap();
            std::fs::write(path.join("labwc/rc.xml"), "<labwc_config/>").unwrap();
        }
        let environment = format!(
            "XDG_CONFIG_HOME={}\0XDG_CONFIG_DIRS={}:{}\0",
            user.display(),
            first.display(),
            second.display()
        );
        let inspect_args = |arguments: &[&str]| {
            inspect(
                Target::Labwc,
                &arguments.iter().map(OsString::from).collect::<Vec<_>>(),
                environment.as_bytes(),
                directory.path(),
            )
            .unwrap()
        };
        assert_eq!(
            inspect_args(&["labwc"]).file,
            Some(user.join("labwc/rc.xml"))
        );
        let merged = inspect_args(&["labwc", "--merge-config"]);
        let short = inspect_args(&["labwc", "-m"]);
        assert_eq!(short.file, merged.file);
        assert_eq!(short.additional_files, merged.additional_files);
        assert_eq!(merged.file, Some(second.join("labwc/rc.xml")));
        assert_eq!(
            merged.additional_files,
            [first.join("labwc/rc.xml"), user.join("labwc/rc.xml")]
        );
        let explicit = inspect_args(&["labwc", "--merge-config", "-c", "selected.xml"]);
        assert_eq!(explicit.file, Some(directory.path().join("selected.xml")));
        assert!(explicit.additional_files.is_empty());
    }
}
