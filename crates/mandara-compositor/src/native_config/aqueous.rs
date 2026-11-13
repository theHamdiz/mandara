//! Read only Aqueous's path and modifier overrides from its own environment.

use super::NativeConfig;
use std::{
    ffi::OsStr,
    os::unix::ffi::OsStrExt as _,
    path::{Path, PathBuf},
};

fn value<'a>(environment: &'a [u8], key: &[u8]) -> Option<&'a OsStr> {
    environment.split(|byte| *byte == 0).find_map(|entry| {
        let at = entry.iter().position(|byte| *byte == b'=')?;
        (entry[..at] == *key && at + 1 < entry.len()).then(|| OsStr::from_bytes(&entry[at + 1..]))
    })
}

pub(super) fn inspect(
    environment: &[u8],
    cwd: &Path,
    exists: impl Fn(&Path) -> bool,
) -> NativeConfig {
    let home = value(environment, b"HOME").map(PathBuf::from);
    let absolute = |path: PathBuf| {
        if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        }
    };
    let explicit = value(environment, b"AQUEOUS_CONFIG").map(|path| {
        if let Some(rest) = path.as_bytes().strip_prefix(b"~/")
            && let Some(home) = &home
        {
            return home.join(OsStr::from_bytes(rest));
        }
        absolute(path.into())
    });
    let file = explicit.or_else(|| {
        let xdg = value(environment, b"XDG_CONFIG_HOME")
            .map(|path| absolute(PathBuf::from(path).join("aqueous/wm.toml")));
        let home = home.map(|path| absolute(path.join(".config/aqueous/wm.toml")));
        [xdg, home, Some("/etc/xdg/aqueous/wm.toml".into())]
            .into_iter()
            .flatten()
            .find(|path| exists(path))
    });
    NativeConfig {
        file,
        primary_mod_is_alt: value(environment, b"AQUEOUS_MOD")
            .is_some_and(|value| value.as_bytes().eq_ignore_ascii_case(b"alt")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn explicit_paths_and_native_fallbacks_use_the_servers_environment() {
        let env = b"HOME=/home/fixture\0XDG_CONFIG_HOME=/xdg\0AQUEOUS_CONFIG=~/keys.toml\0AQUEOUS_MOD=Alt\0SECRET=not-reported\0";
        let config = inspect(env, Path::new("/work"), |_| false);
        assert_eq!(config.file, Some("/home/fixture/keys.toml".into()));
        assert!(config.primary_mod_is_alt);
        let config = inspect(
            b"HOME=/home/fixture\0XDG_CONFIG_HOME=/xdg\0",
            Path::new("/work"),
            |path| path == Path::new("/home/fixture/.config/aqueous/wm.toml"),
        );
        assert_eq!(
            config.file,
            Some("/home/fixture/.config/aqueous/wm.toml".into())
        );
        assert!(!config.primary_mod_is_alt);
        assert_eq!(
            inspect(b"AQUEOUS_CONFIG=keys.toml\0", Path::new("/work"), |_| false).file,
            Some("/work/keys.toml".into())
        );
    }
}
