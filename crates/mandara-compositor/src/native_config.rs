//! Locate a config override without running a compositor's startup command.

use anyhow::Result;
#[cfg(any(target_os = "linux", test))]
use anyhow::{Context as _, ensure};
use mandara_config::compositor::Target;
#[cfg(any(target_os = "linux", test))]
use mandara_i18n::t;
use std::path::PathBuf;
#[cfg(any(target_os = "linux", test))]
mod aqueous;
#[cfg(any(target_os = "linux", test))]
mod environment;

#[derive(Default)]
pub struct NativeConfig {
    pub file: Option<PathBuf>,
    pub additional_files: Vec<PathBuf>,
    pub primary_mod_is_alt: bool,
    pub environment: Option<mandara_config::compositor::IncludeEnvironment>,
}
#[cfg(any(target_os = "linux", test))]
use std::{
    ffi::{OsStr, OsString},
    os::unix::ffi::{OsStrExt as _, OsStringExt as _},
    path::Path,
};

/// Linux caps combined launch arguments below this bound. Read procfs to EOF
/// first, then reject malformed input instead of interpreting a truncated argv.
#[cfg(target_os = "linux")]
const MAX_LAUNCH_BYTES: usize = 8 * 1024 * 1024;

#[cfg(target_os = "linux")]
pub fn running(target: Target) -> Result<NativeConfig> {
    let sway_file = if target == Target::Sway {
        crate::sway::SwayDriver::from_env()
            .map(|driver| driver.config_file())
            .transpose()?
    } else {
        None
    };
    let Some(pid) = super::display_server_pid() else {
        return Ok(NativeConfig {
            file: sway_file,
            ..Default::default()
        });
    };
    let bytes = std::fs::read(format!("/proc/{pid}/cmdline"))?;
    ensure!(
        bytes.len() <= MAX_LAUNCH_BYTES,
        "{}",
        t("The compositor's launch arguments are too large to inspect.")
    );
    let arguments: Vec<_> = bytes
        .split(|byte| *byte == 0)
        .filter(|arg| !arg.is_empty())
        .map(|arg| OsString::from_vec(arg.to_vec()))
        .collect();
    let cwd = std::fs::read_link(format!("/proc/{pid}/cwd"))?;
    let environment = std::fs::read(format!("/proc/{pid}/environ"))?;
    ensure!(
        environment.len() <= MAX_LAUNCH_BYTES,
        "{}",
        t("The compositor's environment is too large to inspect.")
    );
    let mut config = if target == Target::Aqueous {
        aqueous::inspect(&environment, &cwd, Path::exists)
    } else {
        environment::inspect(target, &arguments, &environment, &cwd)?
    };
    if let Some(file) = sway_file {
        config.file = Some(file);
    }
    config.environment = Some(environment::capture(&environment));
    Ok(config)
}

#[cfg(not(target_os = "linux"))]
pub fn running(_: Target) -> Result<NativeConfig> {
    Ok(NativeConfig::default())
}

#[cfg(any(target_os = "linux", test))]
fn from_arguments(target: Target, arguments: &[OsString], cwd: &Path) -> Result<Option<PathBuf>> {
    if matches!(target, Target::KWin | Target::Cosmic | Target::Aqueous) {
        return Ok(None);
    }
    let mut file = None;
    let mut directory = None;
    let mut arguments = arguments.iter().skip(1);
    while let Some(argument) = arguments.next() {
        if argument == "--" {
            break;
        }
        let bytes = argument.as_bytes();
        let flag = if argument == "-c" || argument == "--config" {
            Some((false, None))
        } else if target == Target::Labwc && (argument == "-C" || argument == "--config-dir") {
            Some((true, None))
        } else if let Some(value) = bytes.strip_prefix(b"--config=") {
            Some((false, Some(OsStr::from_bytes(value))))
        } else if target == Target::Labwc
            && let Some(value) = bytes.strip_prefix(b"--config-dir=")
        {
            Some((true, Some(OsStr::from_bytes(value))))
        } else if target == Target::Labwc && bytes.starts_with(b"-C") && bytes.len() > 2 {
            Some((true, Some(OsStr::from_bytes(&bytes[2..]))))
        } else if bytes.starts_with(b"-c") && bytes.len() > 2 {
            Some((false, Some(OsStr::from_bytes(&bytes[2..]))))
        } else {
            None
        };
        let Some((is_directory, inline)) = flag else {
            continue;
        };
        ensure!(
            target != Target::River,
            "{}",
            t(
                "River was started with a configuration command. Its computed shortcuts must be edited in that command's source."
            )
        );
        let value = inline
            .or_else(|| arguments.next().map(OsString::as_os_str))
            .context(t("The compositor's configuration path is missing."))?;
        ensure!(
            !value.is_empty(),
            "{}",
            t("The compositor's configuration path is missing.")
        );
        let path = PathBuf::from(value);
        let path = if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        };
        if is_directory {
            directory = Some(path);
        } else {
            file = Some(path);
        }
    }
    Ok(file.or_else(|| directory.map(|path| path.join("rc.xml"))))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn arguments(words: &[&str]) -> Vec<OsString> {
        words.iter().map(OsString::from).collect()
    }

    #[test]
    fn explicit_files_directories_and_relative_paths_follow_native_launch_flags() {
        for target in [
            Target::Mango,
            Target::Niri,
            Target::Hyprland,
            Target::Umbriel,
            Target::Sway,
        ] {
            for flag in [
                vec!["compositor", "-c", "keys/my.conf"],
                vec!["compositor", "--config=keys/my.conf"],
                vec!["compositor", "-ckeys/my.conf"],
            ] {
                assert_eq!(
                    from_arguments(target, &arguments(&flag), Path::new("/work")).unwrap(),
                    Some("/work/keys/my.conf".into())
                );
            }
        }
        assert_eq!(
            from_arguments(
                Target::Labwc,
                &arguments(&["labwc", "-C", "/custom"]),
                Path::new("/")
            )
            .unwrap(),
            Some("/custom/rc.xml".into())
        );
        assert_eq!(
            from_arguments(
                Target::Labwc,
                &arguments(&["labwc", "-C/custom"]),
                Path::new("/")
            )
            .unwrap(),
            Some("/custom/rc.xml".into())
        );
        assert_eq!(
            from_arguments(
                Target::Labwc,
                &arguments(&["labwc", "-c", "/specific.xml", "-C", "/custom"]),
                Path::new("/")
            )
            .unwrap(),
            Some("/specific.xml".into())
        );
        assert!(
            from_arguments(
                Target::River,
                &arguments(&["river", "-c", "never run this"]),
                Path::new("/")
            )
            .is_err()
        );
        assert!(
            from_arguments(
                Target::Mango,
                &arguments(&["mango", "--config"]),
                Path::new("/")
            )
            .is_err()
        );
    }

    #[test]
    fn config_paths_preserve_non_utf8_names_and_stop_at_the_option_boundary() {
        let path = OsString::from_vec(b"--config=/custom/\xff.conf".to_vec());
        let found = from_arguments(Target::Mango, &["mango".into(), path], Path::new("/"))
            .unwrap()
            .unwrap();
        assert_eq!(found.as_os_str().as_bytes(), b"/custom/\xff.conf");
        assert!(
            from_arguments(
                Target::Mango,
                &arguments(&["mango", "--", "-c", "/unrelated"]),
                Path::new("/")
            )
            .unwrap()
            .is_none()
        );
    }
}
