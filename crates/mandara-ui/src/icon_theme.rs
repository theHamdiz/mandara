//! Freedesktop icon theme lookup honouring the user's installed theme
//! (GTK `gtk-icon-theme-name`, or `appearance.icon_theme` in Mandara's
//! config) and its `Inherits` chain, preferring monochrome `-symbolic` SVGs
//! for the bar so app icons tint with the palette.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResolvedIcon {
    pub path: PathBuf,
    /// A monochrome symbolic SVG (render as a tinted mask).
    pub symbolic: bool,
}

fn icon_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(home) = dirs::home_dir() {
        roots.push(home.join(".icons"));
    }
    if let Some(data) = dirs::data_dir() {
        roots.push(data.join("icons"));
    }
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    roots.extend(system.split(':').filter(|s| !s.is_empty()).map(|d| PathBuf::from(d).join("icons")));
    roots
}

static OVERRIDE: Mutex<Option<String>> = Mutex::new(None);

/// Sets the icon theme from Mandara's config (`None` follows GTK).
pub fn set_theme_override(theme: Option<String>) {
    if let Ok(mut o) = OVERRIDE.lock() {
        *o = theme.filter(|t| !t.is_empty());
    }
}

/// How long the GTK icon-theme name read from `settings.ini` is trusted
/// before re-reading (icons resolve during render; this keeps file I/O off
/// the frame path while still following a theme switch within seconds).
const GTK_THEME_TTL: std::time::Duration = std::time::Duration::from_secs(5);

fn active_theme() -> Option<String> {
    static GTK: Mutex<Option<(std::time::Instant, Option<String>)>> = Mutex::new(None);
    if let Some(theme) = OVERRIDE.lock().ok().and_then(|o| o.clone()) {
        return Some(theme);
    }
    let mut cached = GTK.lock().unwrap_or_else(|e| e.into_inner());
    match cached.as_ref() {
        Some((at, theme)) if at.elapsed() < GTK_THEME_TTL => theme.clone(),
        _ => {
            let theme = gtk_icon_theme();
            *cached = Some((std::time::Instant::now(), theme.clone()));
            theme
        }
    }
}

/// The user's GTK icon theme name.
pub fn gtk_icon_theme() -> Option<String> {
    let config = dirs::config_dir()?;
    ["gtk-4.0/settings.ini", "gtk-3.0/settings.ini"].iter().find_map(|f| {
        let text = std::fs::read_to_string(config.join(f)).ok()?;
        text.lines().find_map(|l| l.trim().strip_prefix("gtk-icon-theme-name").and_then(|r| r.trim().strip_prefix('=')).map(|v| v.trim().trim_matches('"').to_string()))
    })
}

/// Every directory of theme `name` across the icon roots. A theme spans
/// all of them (icon theme spec): flatpak's exported `hicolor` holds only
/// flatpak apps, so stopping at the first hit hid every system icon.
fn theme_dirs(name: &str) -> Vec<PathBuf> {
    icon_roots().into_iter().map(|r| r.join(name)).filter(|d| d.is_dir()).collect()
}

fn inherits(dir: &Path) -> Vec<String> {
    std::fs::read_to_string(dir.join("index.theme"))
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("Inherits=").map(|v| v.split(',').map(|s| s.trim().to_string()).collect())))
        .unwrap_or_default()
}

/// [`search_chain`], computed once per theme (it reads every ancestor's
/// `index.theme`; icon misses used to repeat that for each name).
fn cached_chain(theme: &str) -> std::sync::Arc<[PathBuf]> {
    static CHAINS: OnceLock<Mutex<HashMap<String, std::sync::Arc<[PathBuf]>>>> = OnceLock::new();
    let chains = CHAINS.get_or_init(Default::default);
    if let Some(hit) = chains.lock().ok().and_then(|c| c.get(theme).cloned()) {
        return hit;
    }
    let chain: std::sync::Arc<[PathBuf]> = search_chain(theme).into();
    if let Ok(mut c) = chains.lock() {
        c.insert(theme.to_string(), chain.clone());
    }
    chain
}

/// Most icon lookups kept (hits and misses). Callers resolve arbitrary names
/// — every process in the processes panel, every tray item — so the cache
/// is bounded; past this size the oldest half is dropped.
const RESOLVE_CACHE_CAPACITY: usize = 1024;

#[derive(Default)]
struct ResolveCache {
    map: HashMap<(String, String, bool), Option<ResolvedIcon>>,
    /// Insertion order, for eviction.
    order: std::collections::VecDeque<(String, String, bool)>,
}

impl ResolveCache {
    fn get(&self, key: &(String, String, bool)) -> Option<Option<ResolvedIcon>> {
        self.map.get(key).cloned()
    }

    fn insert(&mut self, key: (String, String, bool), value: Option<ResolvedIcon>) {
        if self.map.insert(key.clone(), value).is_none() {
            self.order.push_back(key);
        }
        if self.order.len() > RESOLVE_CACHE_CAPACITY {
            for old in self.order.drain(..RESOLVE_CACHE_CAPACITY / 2) {
                self.map.remove(&old);
            }
            self.map.shrink_to_fit();
        }
    }
}

/// Theme directories to search, in order (theme, its ancestors, hicolor).
pub fn search_chain(theme: &str) -> Vec<PathBuf> {
    let mut chain = Vec::new();
    let mut queue = vec![theme.to_string()];
    let mut seen = std::collections::HashSet::new();
    while let Some(name) = queue.first().cloned() {
        queue.remove(0);
        if !seen.insert(name.clone()) {
            continue;
        }
        let dirs = theme_dirs(&name);
        if let Some(index) = dirs.iter().find(|d| d.join("index.theme").exists()) {
            queue.extend(inherits(index));
        }
        chain.extend(dirs);
    }
    if !seen.contains("hicolor") {
        chain.extend(theme_dirs("hicolor"));
    }
    chain
}

const SIZES: [&str; 8] = ["scalable", "256", "128", "96", "64", "48", "32", "24"];
/// Where icons live in a theme (`mimetypes` last: file-type icons for the
/// dock's stacks).
const CATEGORIES: [&str; 5] = ["apps", "places", "devices", "status", "mimetypes"];

fn lookup_in(dir: &Path, name: &str, symbolic: bool) -> Option<PathBuf> {
    if symbolic {
        let file = format!("{name}-symbolic.svg");
        let mut candidates = vec![dir.join("symbolic/apps").join(&file)];
        for size in ["scalable", "16", "22", "24", "32"] {
            for cat in CATEGORIES {
                candidates.push(dir.join(size).join(cat).join(&file));
                candidates.push(dir.join(cat).join(size).join(&file));
            }
        }
        return candidates.into_iter().find(|p| p.exists());
    }
    for size in SIZES {
        for cat in CATEGORIES {
            for ext in ["svg", "png"] {
                for p in [dir.join(size).join(cat).join(format!("{name}.{ext}")), dir.join(cat).join(size).join(format!("{name}.{ext}"))] {
                    if p.exists() {
                        return Some(p);
                    }
                }
            }
        }
        // hicolor-style "256x256" directories.
        if size != "scalable" {
            let sized = format!("{size}x{size}");
            for ext in ["png", "svg"] {
                let p = dir.join(&sized).join("apps").join(format!("{name}.{ext}"));
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }
    None
}

/// Resolves an icon name in `theme` (or the user's GTK theme). With
/// `prefer_symbolic`, a monochrome `-symbolic` variant wins when available.
pub fn resolve(name: &str, theme: Option<&str>, prefer_symbolic: bool) -> Option<ResolvedIcon> {
    static CACHE: OnceLock<Mutex<ResolveCache>> = OnceLock::new();
    if name.is_empty() {
        return None;
    }
    let p = Path::new(name);
    let absolute = p.is_absolute();
    // Absolute paths don't depend on the theme (and must not stat the file
    // system on every render either).
    let theme = if absolute { String::new() } else { theme.map(str::to_string).or_else(active_theme).unwrap_or_else(|| "hicolor".into()) };
    let key = (name.to_string(), theme.clone(), prefer_symbolic);
    let cache = CACHE.get_or_init(Default::default);
    if let Some(hit) = cache.lock().ok().and_then(|c| c.get(&key)) {
        return hit;
    }
    if absolute {
        let found = p.exists().then(|| ResolvedIcon { path: p.to_path_buf(), symbolic: false });
        if let Ok(mut c) = cache.lock() {
            c.insert(key, found.clone());
        }
        return found;
    }
    let chain = cached_chain(&theme);
    let found = (prefer_symbolic)
        .then(|| chain.iter().find_map(|d| lookup_in(d, name, true)).map(|path| ResolvedIcon { path, symbolic: true }))
        .flatten()
        .or_else(|| chain.iter().find_map(|d| lookup_in(d, name, false)).map(|path| ResolvedIcon { path, symbolic: false }))
        // Unthemed icons: directly in each icon base directory, then pixmaps
        // (freedesktop icon theme spec, "Icon Lookup").
        .or_else(|| {
            icon_roots().into_iter().chain(std::iter::once(PathBuf::from("/usr/share/pixmaps"))).find_map(|dir| {
                ["svg", "png"].iter().find_map(|ext| {
                    let p = dir.join(format!("{name}.{ext}"));
                    p.exists().then_some(ResolvedIcon { path: p, symbolic: false })
                })
            })
        });
    if let Ok(mut c) = cache.lock() {
        c.insert(key, found.clone());
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hicolor_is_always_in_the_chain() {
        let chain = search_chain("definitely-not-a-theme");
        assert!(chain.iter().all(|d| d.is_dir()));
    }

    #[test]
    fn a_theme_spans_every_icon_root() {
        let dirs = theme_dirs("hicolor");
        assert_eq!(dirs.len(), icon_roots().iter().filter(|r| r.join("hicolor").is_dir()).count());
        let chain = search_chain("hicolor");
        assert!(dirs.iter().all(|d| chain.contains(d)), "every hicolor directory is searched, not just the first");
    }

    #[test]
    fn absolute_paths_and_empty_names() {
        assert!(resolve("", None, true).is_none());
        assert!(resolve("/nonexistent/icon.svg", None, false).is_none());
    }
}

/// The distribution's logo icon name (`LOGO=` in os-release).
/// Read once: os-release does not change while the shell runs, and bar
/// icons ask for it on every render.
pub fn distro_logo() -> Option<String> {
    static LOGO: OnceLock<Option<String>> = OnceLock::new();
    LOGO.get_or_init(|| {
        ["/etc/os-release", "/usr/lib/os-release"].iter().find_map(|f| {
            let text = std::fs::read_to_string(f).ok()?;
            text.lines().find_map(|l| l.strip_prefix("LOGO=")).map(|v| v.trim().trim_matches('"').to_string()).filter(|v| !v.is_empty())
        })
    })
    .clone()
}

/// Every icon name available in the active theme chain (sorted, unique).
/// Scanned once per theme, then cached.
pub fn all_names() -> Vec<String> {
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<String>>>> = OnceLock::new();
    let theme = active_theme().unwrap_or_else(|| "hicolor".into());
    let cache = CACHE.get_or_init(Default::default);
    if let Some(names) = cache.lock().ok().and_then(|c| c.get(&theme).cloned()) {
        return names;
    }
    fn walk(dir: &Path, depth: u8, out: &mut std::collections::BTreeSet<String>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                if depth > 0 {
                    walk(&p, depth - 1, out);
                }
            } else if matches!(p.extension().and_then(|x| x.to_str()), Some("svg" | "png"))
                && let Some(stem) = p.file_stem()
            {
                out.insert(stem.to_string_lossy().into_owned());
            }
        }
    }
    let mut set = std::collections::BTreeSet::new();
    for dir in search_chain(&theme) {
        walk(&dir, 3, &mut set);
    }
    let names: Vec<String> = set.into_iter().collect();
    if let Ok(mut c) = cache.lock() {
        c.insert(theme, names.clone());
    }
    names
}
