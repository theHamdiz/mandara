//! Cursor themes: the curated download catalog ([`catalog`]), installing
//! and removing them ([`install`]) and reading their XCursor files for
//! previews ([`xcursor`], [`preview`]).
//!
//! Nothing here runs anything from a theme archive: archives are verified
//! against a pinned SHA-256, unpacked with pure-Rust readers that refuse
//! unsafe paths, and only the theme's `index.theme` + `cursors/` folder is
//! kept.

pub mod catalog;
pub mod install;
pub mod xcursor;

use std::path::{Path, PathBuf};

use mandara_i18n::tk;

/// The cursor shapes a preview shows, each with the names themes use for
/// it (CSS / freedesktop names first, then the X11 legacy ones).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    Default,
    Pointer,
    Text,
    Wait,
    Crosshair,
    ResizeEw,
    ResizeNs,
    ResizeNwse,
    Move,
}

impl Role {
    pub const ALL: [Role; 9] = [Role::Default, Role::Pointer, Role::Text, Role::Wait, Role::Crosshair, Role::ResizeEw, Role::ResizeNs, Role::ResizeNwse, Role::Move];
    /// The five shapes a gallery card's strip shows.
    pub const STRIP: [Role; 5] = [Role::Default, Role::Pointer, Role::Text, Role::Wait, Role::Crosshair];

    /// File names in `cursors/`, most specific first.
    pub fn names(self) -> &'static [&'static str] {
        match self {
            Role::Default => &["default", "left_ptr", "arrow", "top_left_arrow"],
            Role::Pointer => &["pointer", "hand2", "pointing_hand", "hand1", "hand"],
            Role::Text => &["text", "xterm", "ibeam"],
            Role::Wait => &["wait", "watch", "progress", "left_ptr_watch"],
            Role::Crosshair => &["crosshair", "cross", "tcross", "cell"],
            Role::ResizeEw => &["ew-resize", "sb_h_double_arrow", "h_double_arrow", "col-resize", "size_hor"],
            Role::ResizeNs => &["ns-resize", "sb_v_double_arrow", "v_double_arrow", "row-resize", "size_ver"],
            Role::ResizeNwse => &["nwse-resize", "bd_double_arrow", "size_fdiag", "se-resize", "bottom_right_corner"],
            Role::Move => &["move", "fleur", "all-scroll", "size_all", "grabbing"],
        }
    }

    /// `tk` text; display with `tr`.
    pub fn label(self) -> &'static str {
        match self {
            Role::Default => tk("Pointer"),
            Role::Pointer => tk("Link"),
            Role::Text => tk("Text"),
            Role::Wait => tk("Busy"),
            Role::Crosshair => tk("Precision"),
            Role::ResizeEw => tk("Resize horizontally"),
            Role::ResizeNs => tk("Resize vertically"),
            Role::ResizeNwse => tk("Resize diagonally"),
            Role::Move => tk("Move"),
        }
    }
}

/// How far `Inherits=` chains are followed.
const INHERIT_DEPTH: usize = 4;

/// The folder of the installed theme `name` (first match in `dirs`).
pub fn theme_dir(name: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    if name.is_empty() || name.contains('/') || name.starts_with('.') {
        return None;
    }
    dirs.iter().map(|d| d.join(name)).find(|d| d.join("cursors").is_dir() || d.join("index.theme").is_file())
}

/// The themes `index.theme` inherits from.
pub fn inherits(dir: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(dir.join("index.theme")) else { return Vec::new() };
    text.lines()
        .filter_map(|l| l.trim().strip_prefix("Inherits"))
        .filter_map(|rest| rest.trim_start().strip_prefix('='))
        .flat_map(|v| v.split([',', ';']))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// The cursor file for `role` in theme `name`, following `Inherits=` for
/// shapes the theme leaves out.
pub fn find_cursor(name: &str, role: Role, dirs: &[PathBuf]) -> Option<PathBuf> {
    let mut queue = vec![(name.to_string(), 0usize)];
    let mut seen: Vec<String> = Vec::new();
    while let Some((theme, depth)) = queue.pop() {
        if seen.contains(&theme) || depth > INHERIT_DEPTH {
            continue;
        }
        seen.push(theme.clone());
        let Some(dir) = theme_dir(&theme, dirs) else { continue };
        let cursors = dir.join("cursors");
        if let Some(file) = role.names().iter().map(|n| cursors.join(n)).find(|p| p.is_file()) {
            return Some(file);
        }
        // Breadth by inheritance order: push in reverse so the first
        // parent is searched first.
        queue.extend(inherits(&dir).into_iter().rev().map(|t| (t, depth + 1)));
    }
    None
}

/// The first frame of `role` in theme `name`, decoded for `px` device
/// pixels.
pub fn preview(name: &str, role: Role, px: u32, dirs: &[PathBuf]) -> Option<xcursor::Image> {
    xcursor::load(&find_cursor(name, role, dirs)?, px).ok()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A 1×1 opaque cursor file in `color` (ARGB) at 24 and 48.
    pub(crate) fn cursor_file(color: u32) -> Vec<u8> {
        xcursor::encode(&[(24, 1, 1, 0, 0, 0, vec![color]), (48, 2, 2, 0, 0, 0, vec![color; 4])])
    }

    /// A theme folder `dir/name` with `cursors` (name → ARGB) and an
    /// optional `Inherits=`.
    pub(crate) fn make_theme(dir: &Path, name: &str, cursors: &[(&str, u32)], inherit: Option<&str>) {
        let root = dir.join(name);
        std::fs::create_dir_all(root.join("cursors")).unwrap();
        let mut index = format!("[Icon Theme]\nName={name}\n");
        if let Some(i) = inherit {
            index.push_str(&format!("Inherits={i}\n"));
        }
        std::fs::write(root.join("index.theme"), index).unwrap();
        for (file, color) in cursors {
            std::fs::write(root.join("cursors").join(file), cursor_file(*color)).unwrap();
        }
    }

    #[test]
    fn finds_shapes_by_any_of_their_names() {
        let dir = tempfile::tempdir().unwrap();
        make_theme(dir.path(), "Legacy", &[("left_ptr", 0xff00_0000), ("hand2", 0xff11_1111), ("xterm", 0xff22_2222)], None);
        let dirs = vec![dir.path().to_path_buf()];
        assert!(find_cursor("Legacy", Role::Default, &dirs).unwrap().ends_with("left_ptr"));
        assert!(find_cursor("Legacy", Role::Pointer, &dirs).unwrap().ends_with("hand2"));
        assert!(find_cursor("Legacy", Role::Text, &dirs).unwrap().ends_with("xterm"));
        assert!(find_cursor("Legacy", Role::Wait, &dirs).is_none());
        let img = preview("Legacy", Role::Pointer, 40, &dirs).unwrap();
        assert_eq!((img.nominal, img.width), (48, 2));
        assert_eq!(&img.rgba[..4], &[0x11, 0x11, 0x11, 0xff]);
    }

    #[test]
    fn follows_inherits_without_looping() {
        let dir = tempfile::tempdir().unwrap();
        make_theme(dir.path(), "Child", &[("default", 0xff00_0000)], Some("Parent, Missing"));
        make_theme(dir.path(), "Parent", &[("watch", 0xff33_3333)], Some("Child"));
        let dirs = vec![dir.path().to_path_buf()];
        assert_eq!(inherits(&dir.path().join("Child")), vec!["Parent", "Missing"]);
        assert!(find_cursor("Child", Role::Wait, &dirs).unwrap().starts_with(dir.path().join("Parent")));
        assert!(find_cursor("Child", Role::Crosshair, &dirs).is_none(), "the Child ↔ Parent cycle ends");
        assert!(theme_dir("../etc", &dirs).is_none() && theme_dir(".hidden", &dirs).is_none());
    }
}
