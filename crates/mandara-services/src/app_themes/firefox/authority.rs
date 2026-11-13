//! Validate browser write authority after waiting for the theme worker.

use super::{Settings, css};
use crate::app_themes::paths::Locations;
use anyhow::{Result, ensure};

pub(super) fn apply(
    action: &str,
    target: &str,
    size: Option<i64>,
    expected: Option<&Settings>,
    locations: &Locations,
) -> Result<()> {
    let expected = expected.ok_or_else(|| anyhow::anyhow!("no browser app theme is enabled"))?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(locations.config.join("mandara/generated/app-themes.lock"))?;
    lock.lock()?;
    let current = super::settings(locations)?;
    ensure!(
        current.owner == expected.owner
            && current.styles == expected.styles
            && current.colors == expected.colors,
        "the browser app theme changed while this request was waiting"
    );
    css::apply(action, target, size, &current.styles, locations)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_waiting_behind_disable_cannot_recreate_the_profile_css() {
        let root = tempfile::tempdir().unwrap();
        let locations = Locations {
            home: root.path().into(),
            config: root.path().join("config"),
            data: root.path().join("data"),
            cache: root.path().join("cache"),
            state: root.path().join("state"),
        };
        let generated = locations.config.join("mandara/generated");
        std::fs::create_dir_all(&generated).unwrap();
        let styles = root.path().join("styles");
        std::fs::create_dir(&styles).unwrap();
        std::fs::write(styles.join("userChrome.css"), "generated CSS").unwrap();
        let profiles = locations.home.join(".mozilla/firefox");
        let chrome = profiles.join("default/chrome");
        std::fs::create_dir_all(&chrome).unwrap();
        std::fs::write(
            profiles.join("profiles.ini"),
            "[Profile0]\nPath=default\nIsRelative=1\n",
        )
        .unwrap();
        let settings = Settings {
            colors: root.path().join("colors"),
            mode: "dark".into(),
            styles,
            owner: "browser".into(),
            _lease: None,
        };
        let path = super::super::settings_path(&locations);
        std::fs::write(&path, serde_json::to_vec(&settings).unwrap()).unwrap();
        let lock = std::fs::File::create(generated.join("app-themes.lock")).unwrap();
        lock.lock().unwrap();
        let (started, waiting) = std::sync::mpsc::channel();
        let thread = std::thread::spawn(move || {
            started.send(()).unwrap();
            apply(
                "css:enable",
                "userChrome",
                None,
                Some(&settings),
                &locations,
            )
        });
        waiting.recv().unwrap();
        std::fs::write(chrome.join("userChrome.css"), "restored personal CSS").unwrap();
        std::fs::remove_file(path).unwrap();
        drop(lock);
        assert!(thread.join().unwrap().is_err());
        assert_eq!(
            std::fs::read_to_string(chrome.join("userChrome.css")).unwrap(),
            "restored personal CSS"
        );
        assert!(!generated.join("firefox-css.toml").exists());
    }
}
