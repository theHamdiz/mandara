//! XDG locations used by Mandara Shell.

use std::path::{Path, PathBuf};

const APP_DIR: &str = "mandara";

fn home() -> PathBuf {
    dirs::home_dir().unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// `$XDG_CONFIG_HOME/mandara` (usually `~/.config/mandara`).
pub fn config_dir() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| home().join(".config")).join(APP_DIR)
}

/// The primary user config, `~/.config/mandara/config.toml`.
pub fn config_file() -> PathBuf {
    config_dir().join("config.toml")
}

/// Generated compositor drop-ins, `~/.config/mandara/generated/`.
pub fn generated_dir() -> PathBuf {
    config_dir().join("generated")
}

/// What app theming changed outside Mandara's folders, so switching an app
/// off takes exactly that back (`mandara_theme::templates::Ledger`):
/// `~/.config/mandara/generated/app-themes.toml`. Machine-written like the
/// compositor drop-ins beside it, and kept out of the cache so clearing
/// caches never forgets what to restore.
pub fn app_themes_ledger() -> PathBuf {
    generated_dir().join("app-themes.toml")
}

/// `$XDG_CACHE_HOME/mandara` (usually `~/.cache/mandara`).
pub fn cache_dir() -> PathBuf {
    dirs::cache_dir().unwrap_or_else(|| home().join(".cache")).join(APP_DIR)
}

/// User data that is not configuration (notes, to-dos), `~/.local/share/mandara/`.
pub fn data_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| home().join(".local/share")).join(APP_DIR)
}

/// Per-user state that is neither config nor data (last sidebar tab…),
/// `$XDG_STATE_HOME/mandara` (usually `~/.local/state/mandara`).
pub fn state_dir() -> PathBuf {
    dirs::state_dir().unwrap_or_else(|| home().join(".local/state")).join(APP_DIR)
}

/// Cross-app theme outputs, `~/.cache/mandara/themes/`.
pub fn themes_cache_dir() -> PathBuf {
    cache_dir().join("themes")
}

/// Foreground subject alpha masks, `~/.cache/mandara/depth_masks/`.
pub fn depth_masks_dir() -> PathBuf {
    cache_dir().join("depth_masks")
}

/// IPC socket path, `$XDG_RUNTIME_DIR/mandara.sock` (falls back to the cache
/// dir when no runtime dir is available).
pub fn ipc_socket() -> PathBuf {
    match dirs::runtime_dir() {
        Some(dir) => dir.join("mandara.sock"),
        None => cache_dir().join("mandara.sock"),
    }
}

/// Expands a leading `~/` to the home directory.
pub fn expand_tilde(path: &str) -> PathBuf {
    match path.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None if path == "~" => home(),
        None => PathBuf::from(path),
    }
}

/// The wallpaper folder Mandara used to default to, written into configs
/// before it followed your Pictures folder.
pub const LEGACY_WALLPAPER_DIR: &str = "~/Pictures/Wallpapers";

/// `Wallpapers` in your Pictures folder: `XDG_PICTURES_DIR` from
/// `user-dirs.dirs` (`~/Imagens` in Portuguese), else `~/Pictures`.
pub fn default_wallpaper_dir() -> PathBuf {
    wallpaper_dir_in(dirs::picture_dir(), &home())
}

fn wallpaper_dir_in(pictures: Option<PathBuf>, home: &Path) -> PathBuf {
    pictures.unwrap_or_else(|| home.join("Pictures")).join("Wallpapers")
}

/// [`default_wallpaper_dir`] written with `~` (the config's default).
pub fn default_wallpaper_dir_display() -> String {
    tilde(&default_wallpaper_dir(), &home())
}

/// `path` with the home folder written as `~`.
fn tilde(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if !rest.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

/// The folder `wallpaper.directory` names. Empty, or the old English
/// default when that folder doesn't exist, means your Pictures folder's
/// `Wallpapers`; anything else is used as written.
pub fn wallpaper_dir(configured: &str) -> PathBuf {
    resolve_wallpaper_dir(configured, default_wallpaper_dir(), |p| p.is_dir())
}

fn resolve_wallpaper_dir(configured: &str, default: PathBuf, exists: impl Fn(&Path) -> bool) -> PathBuf {
    let configured = configured.trim();
    if configured.is_empty() {
        return default;
    }
    let path = expand_tilde(configured);
    if configured == LEGACY_WALLPAPER_DIR && !exists(&path) { default } else { path }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Issue #9: the default follows XDG_PICTURES_DIR (e.g. pt_BR ~/Imagens).
    #[test]
    fn the_wallpaper_folder_follows_your_pictures_folder() {
        let home = Path::new("/home/u");
        assert_eq!(wallpaper_dir_in(Some("/home/u/Imagens".into()), home), Path::new("/home/u/Imagens/Wallpapers"));
        assert_eq!(wallpaper_dir_in(None, home), Path::new("/home/u/Pictures/Wallpapers"));
        assert_eq!(tilde(Path::new("/home/u/Imagens/Wallpapers"), home), "~/Imagens/Wallpapers");
        assert_eq!(tilde(Path::new("/srv/walls"), home), "/srv/walls");
        let default = PathBuf::from("/home/u/Imagens/Wallpapers");
        // A config from before keeps the old English default: it follows
        // Pictures unless that folder really exists (then it's yours).
        assert_eq!(resolve_wallpaper_dir(LEGACY_WALLPAPER_DIR, default.clone(), |_| false), default);
        assert!(resolve_wallpaper_dir(LEGACY_WALLPAPER_DIR, default.clone(), |_| true).ends_with("Pictures/Wallpapers"));
        assert_eq!(resolve_wallpaper_dir("", default.clone(), |_| true), default);
        assert_eq!(resolve_wallpaper_dir("/srv/walls", default, |_| false), Path::new("/srv/walls"));
    }

    #[test]
    fn tilde_expansion() {
        assert_eq!(expand_tilde("~/x/y"), home().join("x/y"));
        assert_eq!(expand_tilde("/abs"), PathBuf::from("/abs"));
        assert_eq!(expand_tilde("~"), home());
    }

    #[test]
    fn layout_is_nested_under_mandara() {
        assert!(config_file().ends_with("mandara/config.toml"));
        assert!(generated_dir().ends_with("mandara/generated"));
        assert!(depth_masks_dir().ends_with("mandara/depth_masks"));
    }
}

/// The running `mandara` executable. Once a rebuild or upgrade replaces the
/// file, Linux reports `/path/mandara (deleted)`; spawning that fails, so the
/// suffix is stripped (the new binary sits at the same path). Falls back to
/// `mandara` on `PATH`.
pub fn self_exe() -> PathBuf {
    let exe = std::env::current_exe().ok().and_then(|p| p.to_str().map(str::to_string)).unwrap_or_default();
    let exe = PathBuf::from(exe.strip_suffix(" (deleted)").unwrap_or(&exe));
    if exe.is_file() { exe } else { PathBuf::from("mandara") }
}
