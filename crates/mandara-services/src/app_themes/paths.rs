//! Resolve template paths without changing the shell process's environment.

use anyhow::{Result, ensure};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Locations {
    pub home: PathBuf,
    pub config: PathBuf,
    pub data: PathBuf,
    pub cache: PathBuf,
    pub state: PathBuf,
}

impl Locations {
    pub fn current() -> Result<Self> {
        Ok(Self {
            home: dirs::home_dir()
                .ok_or_else(|| anyhow::anyhow!("home directory is unavailable"))?,
            config: dirs::config_dir()
                .ok_or_else(|| anyhow::anyhow!("config directory is unavailable"))?,
            data: dirs::data_dir()
                .ok_or_else(|| anyhow::anyhow!("data directory is unavailable"))?,
            cache: dirs::cache_dir()
                .ok_or_else(|| anyhow::anyhow!("cache directory is unavailable"))?,
            state: dirs::state_dir()
                .ok_or_else(|| anyhow::anyhow!("state directory is unavailable"))?,
        })
    }

    pub fn catalogs(&self) -> PathBuf {
        self.state.join("mandara/app-templates")
    }
    pub fn ledger(&self) -> PathBuf {
        self.config.join("mandara/generated/app-themes.toml")
    }
    pub fn applied(&self) -> PathBuf {
        self.config.join("mandara/generated/custom-app-themes.json")
    }

    pub fn environment(&self) -> Vec<(String, String)> {
        [
            ("HOME", &self.home),
            ("XDG_CONFIG_HOME", &self.config),
            ("XDG_DATA_HOME", &self.data),
            ("XDG_CACHE_HOME", &self.cache),
            ("XDG_STATE_HOME", &self.state),
        ]
        .into_iter()
        .map(|(key, path)| (key.into(), path.to_string_lossy().into_owned()))
        .collect()
    }

    pub fn resolve(&self, base: &Path, path: &Path) -> Result<PathBuf> {
        let text = path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("template path is not UTF-8"))?;
        ensure!(
            !text.is_empty() && !text.contains(['\0', '\n', '\r']),
            "template path is empty or contains a line break"
        );
        if text == "~" {
            return Ok(self.home.clone());
        }
        if let Some(rest) = text.strip_prefix("~/") {
            return Ok(self.home.join(rest));
        }
        for (key, value) in self.environment() {
            for prefix in [format!("${key}/"), format!("${{{key}}}/")] {
                if let Some(rest) = text.strip_prefix(&prefix) {
                    return Ok(PathBuf::from(&value).join(rest));
                }
            }
            if text == format!("${key}") || text == format!("${{{key}}}") {
                return Ok(PathBuf::from(value));
            }
        }
        ensure!(
            !text.starts_with(['$', '~']),
            "unknown variable in template path: {text}"
        );
        Ok(if path.is_absolute() {
            path.to_path_buf()
        } else {
            base.join(path)
        })
    }

    /// A shared package can describe several clients; only installed client roots are written.
    pub fn client_root(&self, path: &Path) -> Option<PathBuf> {
        for base in [&self.config, &self.data] {
            if let Ok(relative) = path.strip_prefix(base) {
                return relative.components().next().map(|part| base.join(part));
            }
        }
        let flatpak = self.home.join(".var/app");
        if let Ok(relative) = path.strip_prefix(&flatpak) {
            return relative.components().next().map(|part| flatpak.join(part));
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn variables_and_spaces_are_paths_rather_than_commands() {
        let locations = Locations {
            home: "/demo/a b".into(),
            config: "/demo/config".into(),
            data: "/demo/data".into(),
            cache: "/demo/cache".into(),
            state: "/demo/state".into(),
        };
        assert_eq!(
            locations
                .resolve(
                    Path::new("/package"),
                    Path::new("${XDG_CONFIG_HOME}/app/a b")
                )
                .unwrap(),
            Path::new("/demo/config/app/a b")
        );
        assert_eq!(
            locations
                .resolve(Path::new("/package"), Path::new("~/$(false) 'quoted'"))
                .unwrap(),
            Path::new("/demo/a b/$(false) 'quoted'")
        );
        assert_eq!(
            locations
                .resolve(Path::new("/package"), Path::new("relative"))
                .unwrap(),
            Path::new("/package/relative")
        );
        assert!(
            locations
                .resolve(Path::new("/package"), Path::new("$UNKNOWN/path"))
                .is_err()
        );
        assert_eq!(
            locations.client_root(Path::new("/demo/config/editor/themes/colors")),
            Some("/demo/config/editor".into())
        );
    }
}
