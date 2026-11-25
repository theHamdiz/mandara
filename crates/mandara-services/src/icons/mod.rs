//! Icon themes: the curated download catalog ([`catalog`]), installing,
//! importing and removing them ([`install`]), making one the desktop's
//! icon theme ([`apply`]), and reading installed themes for the gallery:
//! their `index.theme` ([`IndexTheme`]), which themes are installed
//! ([`installed_themes`]) and a few representative icons for previews
//! ([`preview_paths`]).
//!
//! Nothing here runs anything from a theme archive: catalog archives are
//! verified against a pinned SHA-256, everything is unpacked with
//! pure-Rust readers that refuse unsafe paths, and only folders with a
//! valid `index.theme` are kept (see [`crate::theme_pkg`]).

pub mod apply;
pub mod catalog;
pub mod install;

use std::path::{Path, PathBuf};

use mandara_i18n::{t, tk};

use crate::theme_pkg;

/// Largest `index.theme` read.
const MAX_INDEX_BYTES: u64 = 512 << 10;
/// Most directories an `index.theme` may list.
const MAX_DIRECTORIES: usize = 8192;
/// How far `Inherits=` chains are followed for previews.
const INHERIT_DEPTH: usize = 3;
/// Icon file extensions, preferred first.
const EXTENSIONS: [&str; 3] = ["svg", "png", "xpm"];

/// One `[dir]` section of an `index.theme`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DirInfo {
    pub path: String,
    pub size: u32,
    pub scale: u32,
    pub context: String,
    pub scalable: bool,
}

/// A parsed, validated `index.theme`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IndexTheme {
    pub name: String,
    pub comment: String,
    pub inherits: Vec<String>,
    /// `Directories=` and `ScaledDirectories=`, in order, without repeats.
    pub directories: Vec<String>,
    pub dirs: Vec<DirInfo>,
    pub hidden: bool,
    /// `Example=`: the icon that represents the theme.
    pub example: Option<String>,
}

fn list(value: &str) -> impl Iterator<Item = &str> {
    value.split([',', ';']).map(str::trim).filter(|s| !s.is_empty())
}

fn placeholder(s: &str) -> bool {
    s.contains('%') || (s.contains('@') && s.matches('@').count() >= 2)
}

impl IndexTheme {
    /// Parses and checks `text`: an `[Icon Theme]` section with a `Name`,
    /// at least one icon directory, plain relative directory paths and
    /// `Inherits=` names that are theme names (no paths, no hidden names,
    /// no unfilled build placeholders).
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut out = IndexTheme::default();
        let mut section = String::new();
        let mut seen_main = false;
        let mut current: Option<DirInfo> = None;
        let mut scaled = Vec::new();
        let flush = |current: &mut Option<DirInfo>, out: &mut IndexTheme| {
            if let Some(d) = current.take() {
                out.dirs.push(d);
            }
        };
        for raw in text.lines() {
            let line = raw.trim();
            if line.is_empty() || line.starts_with(['#', ';']) {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|s| s.strip_suffix(']')) {
                flush(&mut current, &mut out);
                section = name.to_string();
                if section == "Icon Theme" {
                    seen_main = true;
                } else if !section.starts_with("X-") {
                    current = Some(DirInfo { path: section.clone(), scale: 1, ..Default::default() });
                }
                continue;
            }
            let Some((key, value)) = line.split_once('=') else { continue };
            let (key, value) = (key.trim(), value.trim());
            if section == "Icon Theme" {
                match key {
                    "Name" => out.name = value.to_string(),
                    "Comment" => out.comment = value.to_string(),
                    "Inherits" => out.inherits = list(value).map(str::to_string).collect(),
                    "Directories" => out.directories.extend(list(value).map(str::to_string)),
                    "ScaledDirectories" => scaled.extend(list(value).map(str::to_string)),
                    "Hidden" => out.hidden = value.eq_ignore_ascii_case("true"),
                    "Example" => out.example = Some(value.to_string()).filter(|v| !v.is_empty()),
                    _ => {}
                }
            } else if let Some(d) = current.as_mut() {
                match key {
                    "Size" => d.size = value.parse().unwrap_or(0),
                    "Scale" => d.scale = value.parse().unwrap_or(1).max(1),
                    "Context" => d.context = value.to_string(),
                    "Type" => d.scalable = value.eq_ignore_ascii_case("Scalable"),
                    _ => {}
                }
            }
        }
        flush(&mut current, &mut out);
        out.directories.extend(scaled);
        let mut unique = Vec::with_capacity(out.directories.len());
        for d in out.directories.drain(..) {
            if !unique.contains(&d) {
                unique.push(d);
            }
        }
        out.directories = unique;
        if !seen_main {
            return Err(t("index.theme has no [Icon Theme] section").to_string());
        }
        if out.name.is_empty() || placeholder(&out.name) {
            return Err(t("index.theme has no usable Name (it may need the theme's build script)").to_string());
        }
        if out.directories.is_empty() {
            return Err(t("index.theme lists no icon folders").to_string());
        }
        if out.directories.len() > MAX_DIRECTORIES {
            return Err(t("index.theme lists too many folders").to_string());
        }
        let safe = |d: &str| !d.contains('\\') && theme_pkg::normalize(Path::new(d)).is_some_and(|n| n.as_os_str() == d && !d.starts_with('/'));
        if !out.directories.iter().all(|d| safe(d)) {
            return Err(t("index.theme lists a folder outside the theme").to_string());
        }
        if !out.inherits.iter().all(|i| theme_pkg::valid_name(i) && !placeholder(i)) {
            return Err(t("index.theme inherits from something that isn't a theme name").to_string());
        }
        Ok(out)
    }

    /// Reads and parses `dir/index.theme` (refusing a symlink or an
    /// oversized file).
    pub fn read(dir: &Path) -> Result<Self, String> {
        let path = dir.join("index.theme");
        let meta = std::fs::symlink_metadata(&path).map_err(|_| t("No index.theme").to_string())?;
        if !meta.is_file() || meta.len() > MAX_INDEX_BYTES {
            return Err(t("index.theme isn't a regular, small file").to_string());
        }
        let text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
        Self::parse(&text)
    }

    /// The first path component of every listed directory (the folders an
    /// installed theme keeps at its top level).
    pub fn top_dirs(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for d in &self.directories {
            if let Some(first) = d.split('/').next()
                && !out.iter().any(|o| o == first)
            {
                out.push(first.to_string());
            }
        }
        out
    }
}

fn is_icon_file(name: &std::ffi::OsStr) -> bool {
    Path::new(name).extension().and_then(|e| e.to_str()).is_some_and(|e| EXTENSIONS.contains(&e) || e == "svgz")
}

/// Whether `dir` is an icon theme: a valid `index.theme` and at least one
/// listed folder holding an icon.
pub fn is_icon_theme(dir: &Path) -> bool {
    let Ok(index) = IndexTheme::read(dir) else { return false };
    index.directories.iter().any(|d| std::fs::read_dir(dir.join(d)).is_ok_and(|mut r| r.any(|e| e.is_ok_and(|e| is_icon_file(&e.file_name())))))
}

/// A theme found in the icon folders.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    /// The folder name (what apps are told).
    pub name: String,
    /// `Name=` from its `index.theme`.
    pub title: String,
    pub dir: PathBuf,
}

/// Every installed icon theme in `roots` (first root wins), sorted by
/// name: hidden themes, `default`, `hicolor` and cursor-only folders are
/// left out.
pub fn installed_themes(roots: &[PathBuf]) -> Vec<Found> {
    let mut out: Vec<Found> = Vec::new();
    for root in roots {
        let Ok(entries) = std::fs::read_dir(root) else { continue };
        for e in entries.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name == "default" || name == "hicolor" || name.starts_with('.') || out.iter().any(|f| f.name == name) {
                continue;
            }
            let dir = e.path();
            let Ok(index) = IndexTheme::read(&dir) else { continue };
            if !index.hidden {
                out.push(Found { name, title: index.name, dir });
            }
        }
    }
    out.sort_by_key(|f| f.name.to_lowercase());
    out
}

/// The folder of installed theme `name` (first root holding its
/// `index.theme`).
pub fn theme_dir(name: &str, roots: &[PathBuf]) -> Option<PathBuf> {
    if !theme_pkg::valid_name(name) {
        return None;
    }
    roots.iter().map(|r| r.join(name)).find(|d| d.join("index.theme").is_file())
}

/// What a preview strip shows: a label and the names themes use for it,
/// most specific first.
pub const PREVIEW: [(&str, &[&str]); 8] = [
    (tk("Folder"), &["folder", "inode-directory"]),
    (tk("Home"), &["user-home", "folder-home", "go-home"]),
    (tk("Files"), &["system-file-manager", "org.gnome.Nautilus", "file-manager", "nautilus"]),
    (tk("Terminal"), &["utilities-terminal", "org.gnome.Terminal", "terminal", "Terminal"]),
    (tk("Browser"), &["firefox", "web-browser", "internet-web-browser", "browser"]),
    (tk("Settings"), &["preferences-system", "org.gnome.Settings", "systemsettings", "preferences-desktop"]),
    (tk("Text editor"), &["accessories-text-editor", "org.gnome.TextEditor", "text-editor", "gedit"]),
    (tk("Music"), &["multimedia-audio-player", "audio-player", "rhythmbox", "org.gnome.Music"]),
];

/// Contexts searched for previews (app, place and device icons).
const PREVIEW_CONTEXTS: [&str; 6] = ["Applications", "Apps", "Places", "Devices", "MimeTypes", "System"];

/// The theme `name`'s chain (itself, then what it inherits, breadth
/// first, without `hicolor`): `(theme folder, its index)`.
fn chain(name: &str, roots: &[PathBuf]) -> Vec<(PathBuf, IndexTheme)> {
    let mut out = Vec::new();
    let mut queue = vec![(name.to_string(), 0usize)];
    let mut seen: Vec<String> = Vec::new();
    while !queue.is_empty() {
        let (theme, depth) = queue.remove(0);
        if seen.contains(&theme) || depth > INHERIT_DEPTH || theme == "hicolor" {
            continue;
        }
        seen.push(theme.clone());
        let Some(dir) = theme_dir(&theme, roots) else { continue };
        let Ok(index) = IndexTheme::read(&dir) else { continue };
        queue.extend(index.inherits.iter().map(|i| (i.clone(), depth + 1)));
        out.push((dir, index));
    }
    out
}

/// The directories of `index` worth searching for a preview at `px`:
/// scalable ones first, then fixed sizes nearest to `px` (bigger before
/// smaller), only app / place / device contexts when the theme says.
fn preview_dirs(index: &IndexTheme, px: u32) -> Vec<&str> {
    let mut dirs: Vec<(&str, (bool, u32, bool))> = index
        .dirs
        .iter()
        .filter(|d| index.directories.contains(&d.path))
        .filter(|d| d.context.is_empty() || PREVIEW_CONTEXTS.contains(&d.context.as_str()))
        .filter(|d| !d.path.contains("symbolic"))
        .map(|d| {
            let size = d.size.saturating_mul(d.scale);
            (d.path.as_str(), (!d.scalable, size.abs_diff(px), size < px))
        })
        .collect();
    dirs.sort_by_key(|(_, k)| *k);
    let mut out: Vec<&str> = dirs.into_iter().map(|(p, _)| p).collect();
    // Listed folders without a section still count, last.
    for d in &index.directories {
        if !d.contains("symbolic") && !out.contains(&d.as_str()) && !index.dirs.iter().any(|i| &i.path == d) {
            out.push(d);
        }
    }
    out
}

/// The files the preview strip draws for theme `name` at `px` device
/// pixels: one per [`PREVIEW`] entry, `None` where the theme (and what it
/// inherits) has none. Blocking: call off the UI thread.
pub fn preview_paths(name: &str, px: u32, roots: &[PathBuf]) -> Vec<Option<PathBuf>> {
    let chain = chain(name, roots);
    let searched: Vec<(&PathBuf, Vec<&str>)> = chain.iter().map(|(dir, index)| (dir, preview_dirs(index, px))).collect();
    PREVIEW
        .iter()
        .map(|(_, names)| {
            searched.iter().find_map(|(dir, subdirs)| {
                names.iter().find_map(|n| subdirs.iter().find_map(|sub| EXTENSIONS[..2].iter().map(|ext| dir.join(sub).join(format!("{n}.{ext}"))).find(|p| p.is_file())))
            })
        })
        .collect()
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) const INDEX: &str = "[Icon Theme]\nName=Fancy\nComment=Test\nInherits=hicolor\nDirectories=48/apps,scalable/apps,48/places\nScaledDirectories=48@2x/apps\n\n[48/apps]\nSize=48\nContext=Applications\nType=Fixed\n\n[scalable/apps]\nSize=64\nContext=Applications\nType=Scalable\n\n[48/places]\nSize=48\nContext=Places\n\n[48@2x/apps]\nSize=48\nScale=2\nContext=Applications\n";

    /// A theme folder `root/name` with `index` and `icons` (relative paths).
    pub(crate) fn make_theme(root: &Path, name: &str, index: &str, icons: &[&str]) {
        let dir = root.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("index.theme"), index).unwrap();
        for icon in icons {
            let p = dir.join(icon);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, "<svg xmlns=\"http://www.w3.org/2000/svg\"/>").unwrap();
        }
    }

    #[test]
    fn parses_a_real_index() {
        let i = IndexTheme::parse(INDEX).unwrap();
        assert_eq!(i.name, "Fancy");
        assert_eq!(i.inherits, vec!["hicolor"]);
        assert_eq!(i.directories, vec!["48/apps", "scalable/apps", "48/places", "48@2x/apps"]);
        assert_eq!(i.top_dirs(), vec!["48", "scalable", "48@2x"]);
        assert!(i.dirs.iter().any(|d| d.path == "scalable/apps" && d.scalable));
        assert!(i.dirs.iter().any(|d| d.path == "48@2x/apps" && d.scale == 2));
    }

    #[test]
    fn bad_indexes_are_refused() {
        for bad in [
            "",
            "[Other]\nName=x\nDirectories=a\n",
            "[Icon Theme]\nDirectories=48/apps\n",
            "[Icon Theme]\nName=%NAME%\nDirectories=48/apps\n",
            "[Icon Theme]\nName=@ThemeName@\nDirectories=48/apps\n",
            "[Icon Theme]\nName=x\n",
            "[Icon Theme]\nName=x\nDirectories=../../etc\n",
            "[Icon Theme]\nName=x\nDirectories=/usr/share/icons\n",
            "[Icon Theme]\nName=x\nDirectories=a/./b\n",
            "[Icon Theme]\nName=x\nDirectories=a\nInherits=../evil\n",
            "[Icon Theme]\nName=x\nDirectories=a\nInherits=.hidden\n",
            "[Icon Theme]\nName=x\nDirectories=a\nInherits=%INHERITS%\n",
        ] {
            assert!(IndexTheme::parse(bad).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn cursor_only_and_empty_themes_are_not_icon_themes() {
        let dir = tempfile::tempdir().unwrap();
        make_theme(dir.path(), "Fancy", INDEX, &["48/apps/firefox.svg"]);
        make_theme(dir.path(), "Empty", INDEX, &[]);
        make_theme(dir.path(), "Cursors", "[Icon Theme]\nName=Cursors\n", &["cursors/left_ptr.svg"]);
        assert!(is_icon_theme(&dir.path().join("Fancy")));
        assert!(!is_icon_theme(&dir.path().join("Empty")));
        assert!(!is_icon_theme(&dir.path().join("Cursors")));
        // Listed: valid indexes only, `hicolor` / `default` / hidden skipped.
        make_theme(dir.path(), "hicolor", INDEX, &["48/apps/x.svg"]);
        make_theme(dir.path(), "Hidden", &INDEX.replace("Comment=Test", "Hidden=true"), &["48/apps/x.svg"]);
        let names: Vec<String> = installed_themes(&[dir.path().to_path_buf()]).into_iter().map(|f| f.name).collect();
        assert_eq!(names, vec!["Empty", "Fancy"]);
    }

    #[test]
    fn previews_prefer_the_theme_then_what_it_inherits() {
        let dir = tempfile::tempdir().unwrap();
        let parent = INDEX.replace("Name=Fancy", "Name=Parent");
        let child = INDEX.replace("Name=Fancy", "Name=Child").replace("Inherits=hicolor", "Inherits=Parent,Child");
        make_theme(dir.path(), "Child", &child, &["48/apps/firefox.png", "scalable/apps/firefox.svg", "48/places/folder.svg"]);
        make_theme(dir.path(), "Parent", &parent, &["48/apps/utilities-terminal.svg", "48/places/user-home.svg", "48/apps/firefox.svg"]);
        let paths = preview_paths("Child", 48, &[dir.path().to_path_buf()]);
        assert_eq!(paths.len(), PREVIEW.len());
        assert!(paths[0].as_ref().unwrap().ends_with("Child/48/places/folder.svg"));
        assert!(paths[1].as_ref().unwrap().ends_with("Parent/48/places/user-home.svg"), "inherited");
        assert!(paths[3].as_ref().unwrap().starts_with(dir.path().join("Parent")));
        assert!(paths[4].as_ref().unwrap().ends_with("Child/scalable/apps/firefox.svg"), "scalable wins in the theme itself");
        assert!(paths[2].is_none() && paths[7].is_none());
        assert!(preview_paths("../etc", 48, &[dir.path().to_path_buf()]).iter().all(Option::is_none));
    }
}
