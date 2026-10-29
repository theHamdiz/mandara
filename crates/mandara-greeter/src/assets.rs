//! Icons: the shell's set plus a few the login screen needs.

use std::borrow::Cow;

use gpui::{AssetSource, SharedString};
use mandara_ui::MandaraAssets;

const HEAD: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="black" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">"#;

fn extra(name: &str) -> Option<&'static str> {
    Some(match name {
        "arrow-right" => r#"<path d="M5 12h14"/><path d="M13 6l6 6-6 6"/>"#,
        "arrow-left" => r#"<path d="M19 12H5"/><path d="M11 6l-6 6 6 6"/>"#,
        "chevron-down" => r#"<path d="M6 9.5l6 6 6-6"/>"#,
        "chevron-up" => r#"<path d="M6 14.5l6-6 6 6"/>"#,
        "user" => r#"<circle cx="12" cy="8.5" r="4"/><path d="M4.5 20.5a7.5 7.5 0 0 1 15 0"/>"#,
        "caps" => r#"<path d="M12 4l7.5 8H15v4.5H9V12H4.5z"/><path d="M9 20.5h6"/>"#,
        "eye" => r#"<path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z"/><circle cx="12" cy="12" r="3"/>"#,
        "eye-off" => r#"<path d="M2.5 12S6 5.5 12 5.5 21.5 12 21.5 12 18 18.5 12 18.5 2.5 12 2.5 12z" stroke-opacity="0.5"/><path d="M4 4l16 16"/>"#,
        _ => return None,
    })
}

pub const EXTRA_ICONS: [&str; 8] = ["arrow-right", "arrow-left", "chevron-down", "chevron-up", "user", "caps", "eye", "eye-off"];

/// The greeter's extra icons as files (for `svg().external_path`), so the
/// view also renders where only the shell's asset source is installed
/// (Mandara Studio's preview). Written once per process to a private
/// temporary folder.
pub fn extra_icon_file(name: &str) -> Option<String> {
    use std::sync::OnceLock;
    static DIR: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    let body = extra(name)?;
    let dir = DIR.get_or_init(|| {
        // The per-user runtime dir (private), else a uid-named temp folder.
        // SAFETY: getuid has no preconditions.
        let uid = unsafe { libc::getuid() };
        let dir = match std::env::var_os("XDG_RUNTIME_DIR") {
            Some(run) => std::path::PathBuf::from(run).join("mandara-greeter-icons"),
            None => std::env::temp_dir().join(format!("mandara-greeter-icons-{uid}")),
        };
        std::fs::create_dir_all(&dir).ok()?;
        for n in EXTRA_ICONS {
            if let Some(b) = extra(n) {
                std::fs::write(dir.join(format!("{n}.svg")), format!("{HEAD}{b}</svg>")).ok()?;
            }
        }
        let _ = body;
        Some(dir)
    });
    dir.as_ref().map(|d| d.join(format!("{name}.svg")).to_string_lossy().to_string())
}

pub struct GreeterAssets;

impl AssetSource for GreeterAssets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        if let Some(body) = path.strip_prefix("icons/").and_then(|p| p.strip_suffix(".svg")).and_then(extra) {
            return Ok(Some(Cow::Owned(format!("{HEAD}{body}</svg>").into_bytes())));
        }
        MandaraAssets.load(path)
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        let mut all = MandaraAssets.list(path)?;
        if path.trim_end_matches('/') == "icons" {
            all.extend(EXTRA_ICONS.iter().map(|n| SharedString::from(format!("icons/{n}.svg"))));
        }
        Ok(all)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serves_extras_and_shell_icons() {
        for n in EXTRA_ICONS.iter().chain(["power", "restart", "moon", "monitor", "check", "close"].iter()) {
            let svg = GreeterAssets.load(&format!("icons/{n}.svg")).unwrap().unwrap_or_else(|| panic!("missing icon {n}"));
            assert!(std::str::from_utf8(&svg).unwrap().starts_with("<svg"));
        }
        assert!(GreeterAssets.load("icons/nope.svg").unwrap().is_none());
    }
}
