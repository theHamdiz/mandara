//! XDG locations used by Mandara Shell.

use std::path::PathBuf;

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

#[cfg(test)]
mod tests {
    use super::*;

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
