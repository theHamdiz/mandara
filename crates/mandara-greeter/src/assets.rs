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
        "palette" => r#"<path d="M12 21a9 9 0 1 1 9-9c0 2.5-2 3.5-4 3.5h-1.8a1.7 1.7 0 0 0-1.2 2.9 1.6 1.6 0 0 1-1.1 2.6z"/><circle cx="7.5" cy="11" r="1"/><circle cx="10" cy="7" r="1"/><circle cx="14.5" cy="7" r="1"/><circle cx="17" cy="11" r="1"/>"#,
        _ => return None,
    })
}

/// Mandara's single-colour emblem (the login screen's default logo).
const EMBLEM: &str = include_str!("../../../assets/brand/mandara-symbolic.svg");

/// Chrome Halo's ring arc: a 120° stroke fading out at both ends (an alpha mask
/// tinted per arc).
pub const HALO_ARC: &str = r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 200"><defs><linearGradient id="fade" x1="0" y1="0" x2="1" y2="0"><stop offset="0" stop-color="#000" stop-opacity="0"/><stop offset="0.5" stop-color="#000" stop-opacity="1"/><stop offset="1" stop-color="#000" stop-opacity="0"/></linearGradient></defs><path d="M 14.7 50.75 A 98.5 98.5 0 0 1 185.3 50.75" fill="none" stroke="url(#fade)" stroke-width="1.1" stroke-linecap="round"/></svg>"##;
/// A complete SVG for an extra icon.
fn svg_text(name: &str) -> Option<String> {
    match name {
        "emblem" => return Some(EMBLEM.to_string()),
        "halo-arc" => return Some(HALO_ARC.to_string()),
        _ => {}
    }
    extra(name).map(|body| format!("{HEAD}{body}</svg>"))
}

pub const EXTRA_ICONS: [&str; 11] = ["arrow-right", "arrow-left", "chevron-down", "chevron-up", "user", "caps", "eye", "eye-off", "palette", "emblem", "halo-arc"];

/// The greeter's extra icons as files (for `svg().external_path`), so the
/// view also renders where only the shell's asset source is installed
/// (Mandara Studio's preview). Written once per process to a private
/// temporary folder.
pub fn extra_icon_file(name: &str) -> Option<String> {
    use std::sync::OnceLock;
    static DIR: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();
    let body = svg_text(name)?;
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
            if let Some(svg) = svg_text(n) {
                std::fs::write(dir.join(format!("{n}.svg")), svg).ok()?;
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
        if let Some(svg) = path.strip_prefix("icons/").and_then(|p| p.strip_suffix(".svg")).and_then(svg_text) {
            return Ok(Some(Cow::Owned(svg.into_bytes())));
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

    /// Every look shows Mandara's emblem when no logo file is set: it is
    /// a real, non-empty drawing, served to the greeter and to Studio's
    /// preview (as a file), and the Material You look's scalloped avatar
    /// shapes are served too.
    #[test]
    fn the_emblem_and_the_avatar_shapes_are_served() {
        assert!(EMBLEM.len() > 1000 && EMBLEM.contains("<path") && EMBLEM.contains("viewBox"), "the phoenix emblem");
        let served = GreeterAssets.load("icons/emblem.svg").unwrap().expect("served");
        assert_eq!(&served[..], EMBLEM.as_bytes());
        let file = extra_icon_file("emblem").expect("a file for Studio's preview");
        assert_eq!(std::fs::read_to_string(file).unwrap(), EMBLEM);
        for i in 0..mandara_core::avatar_shape::STEPS {
            let step = mandara_core::avatar_shape::Step::new(i);
            let svg = GreeterAssets.load(step.asset_path()).unwrap().unwrap_or_else(|| panic!("{}", step.asset_path()));
            assert!(svg.starts_with(b"<svg"));
        }
    }
}
