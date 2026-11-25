//! Restricted procfs access falls back without guessing a relative config root.

use super::{MAX_LAUNCH_BYTES, NativeConfig, aqueous, environment, from_arguments};
use anyhow::{Context as _, Result, ensure};
use std::{
    ffi::OsString,
    io,
    os::unix::ffi::OsStrExt as _,
    path::{Path, PathBuf},
};

pub(super) fn current_environment() -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    for (key, value) in std::env::vars_os() {
        ensure!(
            bytes
                .len()
                .checked_add(key.len())
                .and_then(|size| size.checked_add(value.len()))
                .and_then(|size| size.checked_add(2))
                .is_some_and(|size| size <= MAX_LAUNCH_BYTES),
            "configuration environment is too large"
        );
        bytes.extend_from_slice(key.as_bytes());
        bytes.push(b'=');
        bytes.extend_from_slice(value.as_bytes());
        bytes.push(0);
    }
    Ok(bytes)
}

pub(super) fn inspect(
    pid: u32,
    target: mandara_config::compositor::Target,
    arguments: &[OsString],
    api_file: Option<PathBuf>,
    cwd: io::Result<PathBuf>,
    environment: io::Result<Vec<u8>>,
    current: impl FnOnce() -> Result<Vec<u8>>,
) -> Result<NativeConfig> {
    let cwd = match cwd {
        Ok(cwd) => Some(cwd),
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => None,
        Err(error) => return Err(error).with_context(|| format!("/proc/{pid}/cwd")),
    };
    let environment = match environment {
        Ok(environment) => environment,
        Err(error) if error.kind() == io::ErrorKind::PermissionDenied => current()?,
        Err(error) => return Err(error).with_context(|| format!("/proc/{pid}/environ")),
    };
    ensure!(
        environment.len() <= MAX_LAUNCH_BYTES,
        "{}",
        mandara_i18n::t("The compositor's environment is too large to inspect.")
    );
    if cwd.is_none() {
        if api_file.is_none()
            && let Some(file) = from_arguments(target, arguments, Path::new(""))?
        {
            ensure!(
                file.is_absolute(),
                "cannot resolve a relative compositor config without /proc/{pid}/cwd"
            );
        }
        for key in [
            b"HOME".as_slice(),
            b"NIRI_CONFIG",
            b"AQUEOUS_CONFIG",
            b"XDG_CONFIG_HOME",
        ] {
            let relevant = key == b"HOME"
                || target == mandara_config::compositor::Target::Niri && key == b"NIRI_CONFIG"
                || target == mandara_config::compositor::Target::Aqueous
                    && matches!(key, b"AQUEOUS_CONFIG" | b"XDG_CONFIG_HOME");
            if relevant && let Some(path) = environment::value(&environment, key) {
                let home_path = key == b"AQUEOUS_CONFIG"
                    && Path::new(path).starts_with("~")
                    && environment::value(&environment, b"HOME")
                        .is_some_and(|home| Path::new(home).is_absolute());
                ensure!(
                    Path::new(path).is_absolute() || home_path,
                    "cannot resolve a relative compositor config without /proc/{pid}/cwd"
                );
            }
        }
    }
    let cwd = cwd.as_deref().unwrap_or(Path::new(""));
    let mut config = if target == mandara_config::compositor::Target::Aqueous {
        aqueous::inspect(&environment, cwd, Path::exists)
    } else {
        environment::inspect(target, arguments, &environment, cwd)?
    };
    if let Some(file) = api_file {
        config.file = Some(file);
    }
    config.environment = Some(environment::capture(&environment));
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::compositor::Target;
    fn denied<T>() -> io::Result<T> {
        Err(io::ErrorKind::PermissionDenied.into())
    }
    fn current() -> Result<Vec<u8>> {
        Ok(b"HOME=/local/home\0XDG_CONFIG_HOME=/local/config\0".to_vec())
    }

    #[test]
    fn denied_procfs_retains_absolute_overrides_and_uses_the_users_config_environment() {
        let config = inspect(
            1,
            Target::KWin,
            &["kwin_wayland".into()],
            None,
            denied(),
            denied(),
            current,
        )
        .unwrap();
        assert_eq!(config.file, Some("/local/config/kglobalshortcutsrc".into()));
        assert!(config.environment.is_some());
        let args = ["mango".into(), "--config=/native/keys.conf".into()];
        assert_eq!(
            inspect(1, Target::Mango, &args, None, denied(), denied(), current)
                .unwrap()
                .file,
            Some("/native/keys.conf".into())
        );
        let args = ["sway".into(), "--config=relative".into()];
        assert_eq!(
            inspect(
                1,
                Target::Sway,
                &args,
                Some("/api/config".into()),
                denied(),
                denied(),
                current
            )
            .unwrap()
            .file,
            Some("/api/config".into())
        );
    }

    #[test]
    fn unknown_working_directories_reject_relative_overrides_and_other_io_errors_propagate() {
        let args = ["mango".into(), "--config=relative".into()];
        assert!(inspect(1, Target::Mango, &args, None, denied(), denied(), current).is_err());
        let config = inspect(
            1,
            Target::Mango,
            &args,
            None,
            Ok("/server".into()),
            denied(),
            current,
        )
        .unwrap();
        assert_eq!(config.file, Some("/server/relative".into()));
        assert!(
            inspect(
                1,
                Target::Niri,
                &["niri".into()],
                None,
                denied(),
                Ok(b"HOME=/server\0NIRI_CONFIG=relative\0".to_vec()),
                current
            )
            .is_err()
        );
        for error in [io::ErrorKind::NotFound, io::ErrorKind::InvalidData] {
            let failure = inspect(
                1,
                Target::KWin,
                &[],
                None,
                Err(error.into()),
                denied(),
                current,
            )
            .err()
            .unwrap();
            assert!(format!("{failure:#}").contains("/proc/1/cwd"));
            let failure = inspect(
                1,
                Target::KWin,
                &[],
                None,
                Ok("/server".into()),
                Err(error.into()),
                current,
            )
            .err()
            .unwrap();
            assert!(format!("{failure:#}").contains("/proc/1/environ"));
        }
        let config = inspect(
            1,
            Target::KWin,
            &[],
            None,
            Ok("/server".into()),
            Ok(b"HOME=/server/home\0".to_vec()),
            || panic!("successful reads must not fall back"),
        )
        .unwrap();
        assert_eq!(
            config.file,
            Some("/server/home/.config/kglobalshortcutsrc".into())
        );
    }
}
