//! Browser-requested CSS changes stay within the selected profile's chrome folder.

use crate::app_themes::paths::Locations;
use crate::icons::apply::ini_get;
use anyhow::{Result, ensure};
use mandara_theme::templates::Ledger;
use std::path::{Path, PathBuf};

fn profile(locations: &Locations) -> Result<PathBuf> {
    let preferences: serde_json::Value = match super::super::apply::read_text(&locations.config.join("pywalfox/config.json")) {
        Ok(body) => serde_json::from_str(&body)?,
        Err(error) if error.downcast_ref::<std::io::Error>().is_some_and(|error|error.kind() == std::io::ErrorKind::NotFound) => serde_json::Value::Null,
        Err(error) => return Err(error),
    };
    let root = if let Some(root) = preferences["profile_path"]
        .as_str()
        .filter(|path| !path.is_empty())
    {
        PathBuf::from(root)
    } else if locations.config.join("firefox/profiles.ini").is_file() {
        locations.config.join("firefox")
    } else {
        locations.home.join(".mozilla/firefox")
    };
    let body = super::super::apply::read_text(&root.join("profiles.ini"))?;
    let installed = body
        .lines()
        .filter_map(|line| line.trim().strip_prefix('[')?.strip_suffix(']'))
        .filter(|section| section.starts_with("Install"))
        .find_map(|section| ini_get(&body, section, "Default"));
    if let Some(path) = installed {
        let path = root.join(path);
        if path.is_dir() {
            return Ok(path);
        }
    }
    let selected = ini_get(&body, "Profile0", "Path")
        .ok_or_else(|| anyhow::anyhow!("Firefox has no default profile"))?;
    let path = if ini_get(&body, "Profile0", "IsRelative").as_deref() == Some("0") {
        PathBuf::from(selected)
    } else {
        root.join(selected)
    };
    ensure!(path.is_dir(), "Firefox profile directory is missing");
    Ok(path)
}

pub(super) fn apply(
    action: &str,
    target: &str,
    size: Option<i64>,
    styles: &Path,
    locations: &Locations,
) -> Result<()> {
    let target = target.strip_suffix(".css").unwrap_or(target);
    ensure!(
        ["userChrome", "userContent"].contains(&target),
        "unknown browser CSS target"
    );
    let destination = profile(locations)?
        .join("chrome")
        .join(format!("{target}.css"));
    let record = locations.config.join("mandara/generated/firefox-css.toml");
    let mut ledger = Ledger::read(&record)?;
    let result = (|| {
        match action {
            "css:disable" => {
                ledger.take_back(target);
                ensure!(!ledger.has(target),"browser CSS could not be restored");
            }
            "css:enable" => {
                ledger.put_file(
                    target,
                    &destination,
                    &super::super::apply::read_text(&styles.join(format!("{target}.css")))?,
                )?;
            }
            "css:font:size" => {
                let size = size.ok_or_else(|| anyhow::anyhow!("missing browser font size"))?;
                ensure!(
                    (6..=96).contains(&size),
                    "browser font size must be between 6 and 96"
                );
                let source = super::super::apply::read_text(&destination)?;
                ensure!(
                    source.contains("--pywalfox-font-size:"),
                    "browser CSS has no font size setting"
                );
                let mut result = String::with_capacity(source.len());
                for line in source.lines() {
                    if line.contains("--pywalfox-font-size:") {
                        use std::fmt::Write as _;
                        writeln!(&mut result, "  --pywalfox-font-size: {size}px;")?;
                    } else {
                        result.push_str(line);
                        result.push('\n');
                    }
                }
                ledger.put_file(target, &destination, &result)?;
            }
            _ => anyhow::bail!("unknown browser CSS action"),
        }
        Ok(())
    })();
    ledger.store(&record)?;
    result
}

pub(super) fn undo(locations: &Locations) -> Result<Vec<PathBuf>> {
    let record = locations.config.join("mandara/generated/firefox-css.toml");
    let mut ledger = Ledger::read(&record)?;
    let mut files = ledger.take_back("userChrome");
    files.extend(ledger.take_back("userContent"));
    ledger.store(&record)?;
    ensure!(
        ledger.is_empty(),
        "some browser CSS files could not be restored"
    );
    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_css_font_changes_and_disable_restore_the_profile_original() {
        let root = tempfile::tempdir().unwrap();
        let locations = Locations {
            home: root.path().into(),
            config: root.path().join("config"),
            data: root.path().join("data"),
            cache: root.path().join("cache"),
            state: root.path().join("state"),
        };
        let firefox = locations.home.join(".mozilla/firefox");
        let profile = firefox.join("example.default");
        std::fs::create_dir_all(profile.join("chrome")).unwrap();
        std::fs::write(
            firefox.join("profiles.ini"),
            "[InstallTest]\nDefault=example.default\n",
        )
        .unwrap();
        let target = profile.join("chrome/userChrome.css");
        std::fs::write(&target, "original CSS").unwrap();
        let styles = root.path().join("styles");
        std::fs::create_dir_all(&styles).unwrap();
        std::fs::write(
            styles.join("userChrome.css"),
            ":root {\n  --pywalfox-font-size: 12px;\n}\n",
        )
        .unwrap();
        apply("css:enable", "userChrome", None, &styles, &locations).unwrap();
        apply("css:font:size", "userChrome", Some(18), &styles, &locations).unwrap();
        assert!(std::fs::read_to_string(&target).unwrap().contains("18px"));
        undo(&locations).unwrap();
        assert_eq!(std::fs::read_to_string(target).unwrap(), "original CSS");
    }

    #[test]
    fn unknown_css_targets_never_escape_the_profile() {
        let dir = tempfile::tempdir().unwrap();
        let locations = Locations {
            home: dir.path().into(),
            config: dir.path().join("config"),
            data: dir.path().join("data"),
            cache: dir.path().join("cache"),
            state: dir.path().join("state"),
        };
        assert!(
            apply(
                "css:enable",
                "../../elsewhere",
                None,
                dir.path(),
                &locations
            )
            .is_err()
        );
    }
}
