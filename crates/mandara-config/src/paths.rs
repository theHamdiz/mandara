//! XDG locations used by Mandara Shell.

use std::path::{Path, PathBuf};

const APP_DIR: &str = "mandara";

pub(crate) fn home() -> PathBuf {
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

/// Where each person's wallpaper is shared with Mandara Login: a sticky
/// folder anyone can add to (like `/tmp`) and nobody can change another's
/// files in. Root creates it when Mandara Login is installed or its config
/// is applied ([`ensure_login_wallpaper_dir`]); the shell copies your
/// wallpaper in as `<user>.jpg`.
pub const LOGIN_WALLPAPER_DIR: &str = "/var/lib/mandara-login/wallpapers";
/// Sticky and writable by everyone.
pub const LOGIN_WALLPAPER_DIR_MODE: u32 = 0o1777;

/// Creates [`LOGIN_WALLPAPER_DIR`] under `root` (root only).
pub fn ensure_login_wallpaper_dir(root: &Path) -> std::io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt as _;
    let dir = root.join(LOGIN_WALLPAPER_DIR.trim_start_matches('/'));
    std::fs::create_dir_all(&dir)?;
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(LOGIN_WALLPAPER_DIR_MODE))?;
    Ok(dir)
}

/// The file the shell shares `user`'s wallpaper as, in `dir`: `<user>.jpg`,
/// or `<user>.<uid>.jpg` when someone else already took that name.
pub fn login_wallpaper_target(dir: &Path, user: &str, uid: u32) -> PathBuf {
    let plain = dir.join(format!("{user}.jpg"));
    match std::fs::symlink_metadata(&plain) {
        Ok(m) if std::os::unix::fs::MetadataExt::uid(&m) != uid => dir.join(format!("{user}.{uid}.jpg")),
        _ => plain,
    }
}

/// `user`'s shared wallpaper in `dir`, as Mandara Login reads it: a regular
/// file (never a link) named `<user>.jpg` or `<user>.<…>.jpg` and owned by
/// `uid`, so nobody can put a picture under someone else's name.
pub fn shared_login_wallpaper(dir: &Path, user: &str, uid: u32) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt as _;
    let named = |n: &str| n == format!("{user}.jpg") || (n.starts_with(&format!("{user}.")) && n.ends_with(".jpg") && !n.starts_with('.'));
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| named(&e.file_name().to_string_lossy()))
        .filter_map(|e| {
            let m = std::fs::symlink_metadata(e.path()).ok()?;
            (m.file_type().is_file() && m.uid() == uid && m.len() > 0).then(|| (m.mtime(), e.path()))
        })
        .max()
        .map(|(_, p)| p)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Only the person's own regular file counts as their login wallpaper.
    #[test]
    fn shared_login_wallpapers_belong_to_their_owner() {
        use std::os::unix::fs::MetadataExt as _;
        let dir = tempfile::tempdir().unwrap();
        let me = std::fs::metadata(dir.path()).unwrap().uid();
        assert_eq!(login_wallpaper_target(dir.path(), "amal", me), dir.path().join("amal.jpg"));
        std::fs::write(dir.path().join("amal.jpg"), b"jpeg").unwrap();
        assert_eq!(shared_login_wallpaper(dir.path(), "amal", me), Some(dir.path().join("amal.jpg")));
        assert_eq!(shared_login_wallpaper(dir.path(), "amal", me + 1), None, "a file owned by someone else");
        assert_eq!(shared_login_wallpaper(dir.path(), "am", me), None, "another user's name prefix");
        std::os::unix::fs::symlink(dir.path().join("amal.jpg"), dir.path().join("omar.jpg")).unwrap();
        assert_eq!(shared_login_wallpaper(dir.path(), "omar", me), None, "links never count");
        std::fs::write(dir.path().join("sara.jpg"), b"").unwrap();
        assert_eq!(shared_login_wallpaper(dir.path(), "sara", me), None, "empty files don't count");
        assert_eq!(login_wallpaper_target(dir.path(), "amal", me + 1), dir.path().join(format!("amal.{}.jpg", me + 1)), "a squatted name");
        assert_eq!(shared_login_wallpaper(dir.path(), "nobody", me), None);
        assert_eq!(shared_login_wallpaper(&dir.path().join("missing"), "amal", me), None);
    }

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
