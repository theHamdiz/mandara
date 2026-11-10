//! What opening a file, a folder or the Trash runs.
//!
//! * A **file** opens with the app set for its MIME type: the type comes
//!   from shared-mime-info's glob list (`mime/globs2`), and the app from
//!   Studio → Applications → Defaults (`$XDG_CONFIG_HOME/mimeapps.list`)
//!   first, then the rest of the XDG `mimeapps.list` chain, then the apps
//!   that declare the type (`mimeinfo.cache`). A type without an app of its
//!   own falls back to its parents (`text/x-log` → `text/plain`).
//! * A **folder** opens in the file manager: Studio → Applications →
//!   Preferred apps, else the `inode/directory` default, else the first
//!   installed well-known one.
//! * The **Trash** opens in that same file manager, with the address it
//!   understands (`trash:///`, `trash:/` for Dolphin) or, for managers that
//!   can't open the trash address, the trash folder itself
//!   (`$XDG_DATA_HOME/Trash/files`).
//!
//! Everything here only decides; the shell launches the app's own `Exec`
//! line with the target (never through `xdg-open` or the desktop portal,
//! whose app chooser ignores the default app for files). The lookups read a
//! handful of small files per click and keep nothing.

use std::io::{BufRead as _, Read as _};
use std::path::{Path, PathBuf};

use crate::apps::{DesktopEntry, FILE_MANAGER_CANDIDATES, mimeapps_file, parse_desktop};

/// The folders the lookups read.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OpenEnv {
    /// `mimeapps.list` files, most important first (Studio's own first).
    pub mimeapps: Vec<PathBuf>,
    /// `applications/` folders, user first.
    pub app_dirs: Vec<PathBuf>,
    /// shared-mime-info folders (`…/mime`), user first.
    pub mime_dirs: Vec<PathBuf>,
}

impl OpenEnv {
    /// The session's folders, with `app_dirs` as the shell discovers apps.
    pub fn system(app_dirs: Vec<PathBuf>) -> Self {
        let data_dirs = data_dirs();
        Self { mimeapps: mimeapps_chain(&crate::apps::current_desktops(), &data_dirs), app_dirs, mime_dirs: data_dirs.iter().map(|d| d.join("mime")).collect() }
    }
}

/// `$XDG_DATA_HOME`, then `$XDG_DATA_DIRS`.
fn data_dirs() -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = dirs::data_dir().into_iter().collect();
    let system = std::env::var("XDG_DATA_DIRS").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/usr/local/share:/usr/share".into());
    out.extend(system.split(':').filter(|d| !d.is_empty()).map(PathBuf::from));
    out
}

/// The `mimeapps.list` files of the XDG MIME Applications spec, most
/// important first, except that Studio's own file (`$XDG_CONFIG_HOME/
/// mimeapps.list`, where Studio → Applications → Defaults writes) leads.
/// `desktops` are `XDG_CURRENT_DESKTOP`'s entries; the legacy
/// `defaults.list` of each data folder is read last for that folder.
pub fn mimeapps_chain(desktops: &[String], data_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let config_home = dirs::config_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"));
    let config_dirs = std::env::var("XDG_CONFIG_DIRS").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| "/etc/xdg".into());
    let mut out = vec![mimeapps_file()];
    let mut push_dir = |dir: &Path, legacy: bool| {
        for d in desktops {
            out.push(dir.join(format!("{}-mimeapps.list", d.to_lowercase())));
        }
        out.push(dir.join("mimeapps.list"));
        if legacy {
            out.push(dir.join("defaults.list"));
        }
    };
    push_dir(&config_home, false);
    for dir in config_dirs.split(':').filter(|d| !d.is_empty()) {
        push_dir(Path::new(dir), false);
    }
    for dir in data_dirs {
        push_dir(&dir.join("applications"), true);
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.clone()));
    out
}

// ── MIME types ──────────────────────────────────────────────────────────────

/// The type of a folder.
pub const DIRECTORY: &str = "inode/directory";
/// Files without a known type that hold text.
const TEXT: &str = "text/plain";
/// Files without a known type.
const UNKNOWN: &str = "application/octet-stream";
/// Bytes read to tell text from binary when no glob matches.
const SNIFF_BYTES: u64 = 1024;
/// Parents followed at most (`text/x-c++src` → `text/x-csrc` → `text/plain`).
const MAX_TYPES: usize = 12;

/// `path`'s MIME type: `inode/directory` for folders, else by name from
/// the glob lists in `mime_dirs`, else text or binary by its first bytes.
pub fn mime_type_in(mime_dirs: &[PathBuf], path: &Path) -> String {
    if path.is_dir() {
        return DIRECTORY.into();
    }
    let name = path.file_name().map(|n| n.to_string_lossy()).unwrap_or_default();
    if let Some(t) = glob_type(mime_dirs, &name) {
        return t;
    }
    let mut head = Vec::new();
    let read = std::fs::File::open(path).and_then(|f| f.take(SNIFF_BYTES).read_to_end(&mut head));
    // Text: no NUL bytes, and valid UTF-8 up to a character the read cut.
    let text = read.is_ok() && !head.contains(&0) && std::str::from_utf8(&head).map_or_else(|e| e.error_len().is_none(), |_| true);
    if text { TEXT.into() } else { UNKNOWN.into() }
}

/// The best glob match for file `name` (literal names beat patterns, then
/// the higher weight, then the longer pattern; on a tie the earlier folder).
fn glob_type(mime_dirs: &[PathBuf], name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    let mut best: Option<((bool, u32, usize), String)> = None;
    let mut line = String::new();
    for dir in mime_dirs {
        let Ok(file) = std::fs::File::open(dir.join("globs2")) else { continue };
        let mut reader = std::io::BufReader::new(file);
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let l = line.trim_end();
            if l.starts_with('#') {
                continue;
            }
            let mut parts = l.splitn(4, ':');
            let (Some(weight), Some(mime), Some(pattern)) = (parts.next(), parts.next(), parts.next()) else { continue };
            let case_sensitive = parts.next().is_some_and(|flags| flags.split(',').any(|f| f == "cs"));
            if mime == "__NOGLOBS__" {
                continue;
            }
            let matched = if case_sensitive { glob(pattern, name) } else { glob(&pattern.to_lowercase(), &lower) };
            if !matched {
                continue;
            }
            let literal = !pattern.contains(['*', '?', '[']);
            let rank = (literal, weight.parse().unwrap_or(50), pattern.len());
            if best.as_ref().is_none_or(|(r, _)| rank > *r) {
                best = Some((rank, mime.to_string()));
            }
        }
    }
    best.map(|(_, m)| m)
}

/// fnmatch-style matching: `*`, `?` and `[…]` classes (with `!`/`^` and
/// ranges).
fn glob(pattern: &str, text: &str) -> bool {
    fn class(p: &[char], c: char) -> Option<(bool, usize)> {
        // p starts after '['; returns (matched, chars consumed incl. ']').
        let mut i = 0;
        let negate = matches!(p.first(), Some('!' | '^'));
        if negate {
            i += 1;
        }
        let mut hit = false;
        let mut first = true;
        while i < p.len() {
            if p[i] == ']' && !first {
                return Some((hit != negate, i + 1));
            }
            first = false;
            if i + 2 < p.len() && p[i + 1] == '-' && p[i + 2] != ']' {
                hit |= p[i] <= c && c <= p[i + 2];
                i += 3;
            } else {
                hit |= p[i] == c;
                i += 1;
            }
        }
        None
    }
    let p: Vec<char> = pattern.chars().collect();
    let t: Vec<char> = text.chars().collect();
    let (mut pi, mut ti) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while ti < t.len() {
        let step = match p.get(pi) {
            Some('*') => {
                star = Some((pi, ti));
                pi += 1;
                continue;
            }
            Some('?') => Some(1),
            Some('[') => class(&p[pi + 1..], t[ti]).and_then(|(ok, used)| ok.then_some(used + 1)),
            Some(&c) => (c == t[ti]).then_some(1),
            None => None,
        };
        match step {
            Some(n) => {
                pi += n;
                ti += 1;
            }
            None => match star {
                Some((sp, st)) => {
                    pi = sp + 1;
                    ti = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// `mime`, its aliases and its parents, closest first: the subclass lists
/// in `mime_dirs`, and `text/plain` under every `text/*` type.
pub fn type_and_parents_in(mime_dirs: &[PathBuf], mime: &str) -> Vec<String> {
    let read = |file: &str| mime_dirs.iter().filter_map(|d| std::fs::read_to_string(d.join(file)).ok()).collect::<Vec<_>>();
    let (subclasses, aliases) = (read("subclasses"), read("aliases"));
    let pairs = |texts: &'_ [String]| texts.iter().flat_map(|t| t.lines()).filter_map(|l| l.split_once(' ')).map(|(a, b)| (a.trim().to_string(), b.trim().to_string())).collect::<Vec<_>>();
    let (subclasses, aliases) = (pairs(&subclasses), pairs(&aliases));
    // Unalias first (`application/x-pdf` → `application/pdf`).
    let canonical = aliases.iter().find(|(a, _)| a == mime).map_or(mime, |(_, c)| c.as_str());
    let mut out = vec![canonical.to_string()];
    let mut i = 0;
    while i < out.len() && out.len() < MAX_TYPES {
        let current = out[i].clone();
        let mut parents: Vec<String> = subclasses.iter().filter(|(sub, _)| *sub == current).map(|(_, parent)| parent.clone()).collect();
        if current.starts_with("text/") && current != TEXT {
            parents.push(TEXT.into());
        }
        for p in parents {
            if !out.contains(&p) && out.len() < MAX_TYPES {
                out.push(p);
            }
        }
        i += 1;
    }
    // Lists may name a type by an alias.
    let named: Vec<String> = aliases.iter().filter(|(a, c)| out.contains(c) && !out.contains(a)).map(|(a, _)| a.clone()).collect();
    out.extend(named);
    out
}

// ── Apps for a type ─────────────────────────────────────────────────────────

/// An app that opens things: a desktop entry, or a bare command from
/// Studio → Applications.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Handler {
    /// The desktop id (`org.gnome.Nautilus.desktop`); empty for a command.
    pub id: String,
    pub name: String,
    /// Its `Exec` line (field codes kept) or the command.
    pub exec: String,
    pub terminal: bool,
}

impl Handler {
    fn from_entry(e: DesktopEntry) -> Self {
        let name = if e.name.is_empty() { e.id.trim_end_matches(".desktop").to_string() } else { e.name };
        Self { id: e.id, name, exec: e.exec, terminal: e.terminal }
    }

    fn command(command: &str) -> Self {
        Self { id: String::new(), name: program_name(command).to_string(), exec: command.to_string(), terminal: false }
    }

    /// The desktop id without `.desktop`.
    pub fn stem(&self) -> &str {
        self.id.strip_suffix(".desktop").unwrap_or(&self.id)
    }

    /// The file name of the program it runs (`nautilus`).
    pub fn program(&self) -> &str {
        program_name(&self.exec)
    }
}

/// The file name of a command's program, past `env VAR=x` prefixes.
fn program_name(exec: &str) -> &str {
    let mut words = exec.split_whitespace().map(|w| w.trim_matches('"'));
    let mut word = words.next().unwrap_or_default();
    if Path::new(word).file_name().is_some_and(|n| n == "env") {
        word = words.find(|w| !w.contains('=') && !w.starts_with('-')).unwrap_or_default();
    }
    Path::new(word).file_name().and_then(|n| n.to_str()).unwrap_or(word)
}

/// Desktop entry `id` from the first of `app_dirs` holding it, when it can
/// be launched (not `Hidden`, with an `Exec` line).
fn entry_in(app_dirs: &[PathBuf], id: &str) -> Option<DesktopEntry> {
    if id.is_empty() || id.contains('/') {
        return None;
    }
    app_dirs.iter().map(|d| d.join(id)).find(|p| p.is_file()).and_then(|p| {
        let e = parse_desktop(id, &std::fs::read_to_string(&p).ok()?);
        (!e.hidden && !e.exec.trim().is_empty()).then_some(e)
    })
}

/// The desktop ids listed for `mime` under `group` in a mimeapps text.
fn listed<'a>(text: &'a str, group: &str, mime: &str) -> impl Iterator<Item = &'a str> {
    let mut in_group = false;
    text.lines()
        .filter_map(move |line| {
            let t = line.trim();
            if t.starts_with('[') {
                in_group = t == group;
                return None;
            }
            let (k, v) = t.split_once('=')?;
            (in_group && k.trim() == mime).then_some(v)
        })
        .flat_map(|v| v.split(';').map(str::trim).filter(|s| !s.is_empty()))
}

/// The app that opens `types` (a type, then its parents): the first
/// installed default of any type across the `mimeapps.list` chain, then an
/// added association, then an app that declares the type; associations a
/// list removes are skipped.
pub fn handler_for_types(env: &OpenEnv, types: &[String]) -> Option<Handler> {
    let lists: Vec<String> = env.mimeapps.iter().filter_map(|p| std::fs::read_to_string(p).ok()).collect();
    let pick = |id: &str| entry_in(&env.app_dirs, id).map(Handler::from_entry);
    for mime in types {
        for text in &lists {
            if let Some(h) = listed(text, "[Default Applications]", mime).find_map(pick) {
                return Some(h);
            }
        }
    }
    let removed = |mime: &str, id: &str| lists.iter().any(|text| listed(text, "[Removed Associations]", mime).any(|r| r == id));
    for mime in types {
        for text in &lists {
            if let Some(h) = listed(text, "[Added Associations]", mime).filter(|id| !removed(mime, id)).find_map(pick) {
                return Some(h);
            }
        }
        for dir in &env.app_dirs {
            if let Some(h) = declared_in(dir, mime).into_iter().filter(|id| !removed(mime, id)).find_map(|id| pick(&id)) {
                return Some(h);
            }
        }
    }
    None
}

/// Desktop ids in `dir` that declare `mime`: from its `mimeinfo.cache`, or
/// by reading its entries when it has none.
fn declared_in(dir: &Path, mime: &str) -> Vec<String> {
    if let Ok(cache) = std::fs::read_to_string(dir.join("mimeinfo.cache")) {
        return listed(&cache, "[MIME Cache]", mime).map(String::from).collect();
    }
    let Ok(files) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut ids: Vec<String> = files
        .flatten()
        .filter_map(|f| {
            let id = f.file_name().to_string_lossy().into_owned();
            let text = id.ends_with(".desktop").then(|| std::fs::read_to_string(f.path()).ok()).flatten()?;
            parse_desktop(&id, &text).mime_types.iter().any(|m| m == mime).then_some(id)
        })
        .collect();
    ids.sort();
    ids
}

/// The file manager: Studio → Applications → Preferred apps (a desktop id
/// or a command), else the `inode/directory` app, else the first
/// installed well-known one (`on_path`).
pub fn file_manager(env: &OpenEnv, configured: &str, on_path: impl Fn(&str) -> bool) -> Option<Handler> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let id = format!("{}.desktop", configured.strip_suffix(".desktop").unwrap_or(configured));
        return Some(entry_in(&env.app_dirs, &id).map_or_else(|| Handler::command(configured), Handler::from_entry));
    }
    handler_for_types(env, &[DIRECTORY.to_string()]).or_else(|| FILE_MANAGER_CANDIDATES.iter().find(|c| on_path(c)).map(|c| Handler::command(c)))
}

// ── The Trash ───────────────────────────────────────────────────────────────

/// How a file manager is pointed at the Trash.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TrashTarget {
    /// Its trash address.
    Uri(&'static str),
    /// The trash folder itself (`$XDG_DATA_HOME/Trash/files`).
    Folder,
}

/// File managers that open the trash address, by desktop id (lowercase,
/// without `.desktop`) or program. GIO-based managers take `trash:///`;
/// Dolphin's KIO names it `trash:/`. COSMIC Files takes only paths and
/// `file://` addresses (other addresses are ignored), so it gets the folder,
/// like every manager not listed here.
const TRASH_ADDRESSES: [(&str, Option<&str>); 14] = [
    ("org.gnome.nautilus", Some("trash:///")),
    ("nautilus", Some("trash:///")),
    ("nemo", Some("trash:///")),
    ("caja", Some("trash:///")),
    ("caja-browser", Some("trash:///")),
    ("caja-folder-handler", Some("trash:///")),
    ("thunar", Some("trash:///")),
    ("thunar-folder-handler", Some("trash:///")),
    ("org.kde.dolphin", Some("trash:/")),
    ("dolphin", Some("trash:/")),
    ("pcmanfm", Some("trash:///")),
    ("pcmanfm-qt", Some("trash:///")),
    ("com.system76.cosmicfiles", None),
    ("cosmic-files", None),
];

/// How `fm` opens the Trash: known managers by desktop id, else by the
/// program they run; anything else gets the folder.
pub fn trash_target(fm: &Handler) -> TrashTarget {
    let known = |key: &str| TRASH_ADDRESSES.iter().find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, uri)| *uri);
    let by_id = (!fm.id.is_empty()).then(|| known(fm.stem())).flatten();
    match by_id.or_else(|| known(fm.program())) {
        Some(Some(uri)) => TrashTarget::Uri(uri),
        _ => TrashTarget::Folder,
    }
}

/// `$XDG_DATA_HOME/Trash/files` (default `~/.local/share/Trash/files`).
pub fn trash_files_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share")).join("Trash/files")
}

/// Creates the trash folder (and `Trash/`) private to the user when
/// missing, as the Trash spec's implementations do.
pub fn ensure_trash_files(dir: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::DirBuilderExt as _;
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)
}

// ── The decision ────────────────────────────────────────────────────────────

/// What an app is handed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenTarget {
    /// A file or folder.
    Path(PathBuf),
    /// An address (`trash:///`).
    Uri(String),
}

/// An app and what it opens.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenPlan {
    pub handler: Handler,
    pub target: OpenTarget,
}

/// Why nothing can be opened.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OpenError {
    /// No app opens this type.
    NoApp { mime: String },
    /// No file manager is set or installed.
    NoFileManager,
}

/// What opening `path` runs: a folder in the file manager, a file in the
/// app for its type.
pub fn plan_path(env: &OpenEnv, path: &Path, file_manager_setting: &str, on_path: impl Fn(&str) -> bool) -> Result<OpenPlan, OpenError> {
    let mime = mime_type_in(&env.mime_dirs, path);
    let handler = if mime == DIRECTORY {
        file_manager(env, file_manager_setting, on_path).ok_or(OpenError::NoFileManager)?
    } else {
        handler_for_types(env, &type_and_parents_in(&env.mime_dirs, &mime)).ok_or(OpenError::NoApp { mime })?
    };
    Ok(OpenPlan { handler, target: OpenTarget::Path(path.to_path_buf()) })
}

/// What opening the Trash runs: the file manager with its trash address,
/// or with `files` (the trash folder) when it can't open one.
pub fn plan_trash(env: &OpenEnv, file_manager_setting: &str, on_path: impl Fn(&str) -> bool, files: &Path) -> Result<OpenPlan, OpenError> {
    let handler = file_manager(env, file_manager_setting, on_path).ok_or(OpenError::NoFileManager)?;
    let target = match trash_target(&handler) {
        TrashTarget::Uri(uri) => OpenTarget::Uri(uri.to_string()),
        TrashTarget::Folder => OpenTarget::Path(files.to_path_buf()),
    };
    Ok(OpenPlan { handler, target })
}

#[cfg(test)]
mod tests {
    use super::*;

    const GLOBS: &str = "# generated\n80:text/html:*.html\n50:text/plain:*.txt\n50:image/png:*.png\n50:application/pdf:*.pdf\n50:text/markdown:*.md\n50:text/x-log:*.log\n50:application/gzip:*.gz\n50:application/x-compressed-tar:*.tar.gz\n50:text/x-readme:README\n50:text/x-c++src:*.C:cs\n50:text/x-csrc:*.c\n10:text/x-log:*.log.[0-9]\n";
    const SUBCLASSES: &str = "text/markdown text/plain\ntext/x-log text/plain\napplication/x-compressed-tar application/gzip\n";
    const ALIASES: &str = "application/x-pdf application/pdf\ntext/x-markdown text/markdown\n";

    struct Fixture {
        dir: tempfile::TempDir,
        env: OpenEnv,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path();
            for d in ["mime", "apps", "sys-apps", "config", "files"] {
                std::fs::create_dir_all(root.join(d)).unwrap();
            }
            std::fs::write(root.join("mime/globs2"), GLOBS).unwrap();
            std::fs::write(root.join("mime/subclasses"), SUBCLASSES).unwrap();
            std::fs::write(root.join("mime/aliases"), ALIASES).unwrap();
            let env = OpenEnv { mimeapps: vec![root.join("config/mimeapps.list"), root.join("sys-apps/mimeapps.list")], app_dirs: vec![root.join("apps"), root.join("sys-apps")], mime_dirs: vec![root.join("mime")] };
            Self { dir, env }
        }

        fn root(&self) -> &Path {
            self.dir.path()
        }

        fn app(&self, dir: &str, id: &str, exec: &str, mime: &str) {
            let text = format!("[Desktop Entry]\nType=Application\nName={id}\nExec={exec}\nMimeType={mime}\n");
            std::fs::write(self.root().join(dir).join(id), text).unwrap();
        }

        fn studio(&self, text: &str) {
            std::fs::write(&self.env.mimeapps[0], text).unwrap();
        }

        fn system(&self, text: &str) {
            std::fs::write(&self.env.mimeapps[1], text).unwrap();
        }

        fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let p = self.root().join("files").join(name);
            std::fs::write(&p, bytes).unwrap();
            p
        }

        fn opens(&self, path: &Path) -> Result<String, OpenError> {
            plan_path(&self.env, path, "", |_| false).map(|p| p.handler.id)
        }
    }

    #[test]
    fn globs_pick_literal_then_weight_then_longest() {
        let f = Fixture::new();
        let mime = |name: &str| mime_type_in(&f.env.mime_dirs, &f.root().join("files").join(name));
        assert_eq!(mime("notes.txt"), "text/plain");
        assert_eq!(mime("Shot.PNG"), "image/png", "case-insensitive unless marked cs");
        assert_eq!(mime("a.tar.gz"), "application/x-compressed-tar", "the longer pattern wins a tie");
        assert_eq!(mime("b.gz"), "application/gzip");
        assert_eq!(mime("README"), "text/x-readme");
        assert_eq!(mime("x.C"), "text/x-c++src");
        assert_eq!(mime("x.c"), "text/x-csrc", "a case-sensitive glob only matches its own case");
        assert_eq!(mime("app.log.3"), "text/x-log", "character classes");
        assert_eq!(mime("page.html"), "text/html");
        assert_eq!(mime_type_in(&f.env.mime_dirs, f.root()), DIRECTORY);
        // No glob: text or binary by content.
        assert_eq!(mime_type_in(&f.env.mime_dirs, &f.file("LICENSE", b"MIT License\n")), "text/plain");
        assert_eq!(mime_type_in(&f.env.mime_dirs, &f.file("blob", &[0x7f, b'E', 0, 1])), "application/octet-stream");
        assert_eq!(mime_type_in(&[], &f.file("empty", b"")), "text/plain");
    }

    #[test]
    fn glob_matching() {
        assert!(glob("*.txt", "a.txt"));
        assert!(!glob("*.txt", "a.txt.bak"));
        assert!(glob("*.[ch]", "x.h"));
        assert!(!glob("*.[!ch]", "x.h"));
        assert!(glob("*.[0-9]", "log.7"));
        assert!(glob("?akefile", "Makefile"));
        assert!(glob("*", ""));
        assert!(!glob("a", ""));
        assert!(glob("*rc*", "bashrc.d"));
        assert!(!glob("[", "["), "an unclosed class never matches");
    }

    #[test]
    fn parents_and_aliases() {
        let f = Fixture::new();
        assert_eq!(type_and_parents_in(&f.env.mime_dirs, "text/markdown"), ["text/markdown", "text/plain", "text/x-markdown"]);
        assert_eq!(type_and_parents_in(&f.env.mime_dirs, "application/x-pdf"), ["application/pdf", "application/x-pdf"]);
        assert_eq!(type_and_parents_in(&f.env.mime_dirs, "text/x-weird"), ["text/x-weird", "text/plain"]);
        assert_eq!(type_and_parents_in(&f.env.mime_dirs, "application/x-compressed-tar"), ["application/x-compressed-tar", "application/gzip"]);
        assert_eq!(type_and_parents_in(&[], "image/png"), ["image/png"]);
    }

    /// The dock's Downloads fan handed files to the desktop portal, whose
    /// app chooser doesn't use the default app for files: the files open
    /// with Studio's Defaults first.
    #[test]
    fn files_open_with_studios_default_then_the_system_ones() {
        let f = Fixture::new();
        f.app("apps", "editor.desktop", "editor %F", "text/plain;");
        f.app("sys-apps", "gedit.desktop", "gedit %U", "text/plain;");
        f.app("sys-apps", "viewer.desktop", "viewer %u", "image/png;");
        f.app("sys-apps", "reader.desktop", "reader %f", "application/pdf;");
        f.studio("[Default Applications]\ntext/plain=editor.desktop;\n");
        f.system("[Default Applications]\ntext/plain=gedit.desktop;\nimage/png=viewer.desktop;\napplication/pdf=reader.desktop\n");
        let txt = f.file("notes.txt", b"hi");
        assert_eq!(f.opens(&txt), Ok("editor.desktop".into()), "Studio's choice wins over the system's");
        assert_eq!(f.opens(&f.file("shot.png", b"\x89PNG")), Ok("viewer.desktop".into()));
        assert_eq!(f.opens(&f.file("doc.pdf", b"%PDF")), Ok("reader.desktop".into()));
        // A default that isn't installed is skipped.
        f.studio("[Default Applications]\ntext/plain=gone.desktop;editor.desktop;\n");
        assert_eq!(f.opens(&txt), Ok("editor.desktop".into()));
        // A subtype without a default of its own uses its parent's.
        assert_eq!(f.opens(&f.file("README.md", b"# hi")), Ok("editor.desktop".into()));
        assert_eq!(f.opens(&f.file("app.log", b"ok")), Ok("editor.desktop".into()));
        let plan = plan_path(&f.env, &txt, "", |_| false).unwrap();
        assert_eq!(plan.target, OpenTarget::Path(txt.clone()));
        assert_eq!(plan.handler.exec, "editor %F");
    }

    #[test]
    fn without_a_default_associations_then_declared_apps() {
        let f = Fixture::new();
        f.app("sys-apps", "first.desktop", "first %f", "image/png;");
        f.app("sys-apps", "second.desktop", "second %f", "image/png;");
        let png = f.file("a.png", b"x");
        assert_eq!(f.opens(&png), Ok("first.desktop".into()), "apps that declare the type, by id");
        std::fs::write(f.root().join("sys-apps/mimeinfo.cache"), "[MIME Cache]\nimage/png=second.desktop;first.desktop;\n").unwrap();
        assert_eq!(f.opens(&png), Ok("second.desktop".into()), "mimeinfo.cache's order");
        f.studio("[Removed Associations]\nimage/png=second.desktop;\n");
        assert_eq!(f.opens(&png), Ok("first.desktop".into()), "removed associations are skipped");
        f.studio("[Added Associations]\nimage/png=second.desktop;\n");
        assert_eq!(f.opens(&png), Ok("second.desktop".into()));
        // Hidden entries and entries without Exec never open anything.
        std::fs::write(f.root().join("apps/second.desktop"), "[Desktop Entry]\nName=x\nExec=x\nHidden=true\n").unwrap();
        assert_eq!(f.opens(&png), Ok("first.desktop".into()), "a user entry hiding a system one hides it");
    }

    #[test]
    fn no_app_is_reported_with_the_type() {
        let f = Fixture::new();
        assert_eq!(f.opens(&f.file("doc.pdf", b"%PDF")), Err(OpenError::NoApp { mime: "application/pdf".into() }));
        assert_eq!(f.opens(&f.file("blob", &[0, 1, 2])), Err(OpenError::NoApp { mime: "application/octet-stream".into() }));
    }

    #[test]
    fn folders_open_in_the_file_manager() {
        let f = Fixture::new();
        let folder = f.root().join("files");
        f.app("sys-apps", "org.gnome.Nautilus.desktop", "nautilus --new-window %U", "inode/directory;");
        f.app("sys-apps", "thunar.desktop", "thunar %F", "inode/directory;");
        f.system("[Default Applications]\ninode/directory=org.gnome.Nautilus.desktop;\n");
        assert_eq!(f.opens(&folder), Ok("org.gnome.Nautilus.desktop".into()));
        f.studio("[Default Applications]\ninode/directory=thunar.desktop;\n");
        assert_eq!(f.opens(&folder), Ok("thunar.desktop".into()), "Studio's Defaults → File manager");
        // Studio's Preferred apps setting: a desktop id (with or without
        // `.desktop`) or a command.
        let fm = |setting: &str| plan_path(&f.env, &folder, setting, |_| false).unwrap().handler;
        assert_eq!(fm("org.gnome.Nautilus").id, "org.gnome.Nautilus.desktop");
        assert_eq!(fm("org.gnome.Nautilus.desktop").exec, "nautilus --new-window %U");
        assert_eq!(fm("yazi --cwd"), Handler { id: String::new(), name: "yazi".into(), exec: "yazi --cwd".into(), terminal: false });
        // Nothing set: the first installed well-known one.
        let bare = OpenEnv { mimeapps: Vec::new(), app_dirs: Vec::new(), ..f.env.clone() };
        assert_eq!(plan_path(&bare, &folder, "", |p| p == "dolphin").unwrap().handler.exec, "dolphin");
        assert_eq!(plan_path(&bare, &folder, "", |_| false), Err(OpenError::NoFileManager));
    }

    #[test]
    fn trash_addresses_by_file_manager() {
        let entry = |id: &str, exec: &str| Handler { id: id.into(), name: id.into(), exec: exec.into(), terminal: false };
        for (id, exec) in [
            ("org.gnome.Nautilus.desktop", "nautilus --new-window %U"),
            ("nemo.desktop", "nemo %U"),
            ("caja.desktop", "caja --no-desktop %U"),
            ("caja-browser.desktop", "caja --no-desktop --browser %U"),
            ("thunar.desktop", "thunar %F"),
            ("Thunar-folder-handler.desktop", "thunar %F"),
            ("pcmanfm.desktop", "pcmanfm %U"),
            ("pcmanfm-qt.desktop", "pcmanfm-qt %U"),
        ] {
            assert_eq!(trash_target(&entry(id, exec)), TrashTarget::Uri("trash:///"), "{id}");
        }
        assert_eq!(trash_target(&entry("org.kde.dolphin.desktop", "dolphin %u")), TrashTarget::Uri("trash:/"));
        assert_eq!(trash_target(&entry("com.system76.CosmicFiles.desktop", "cosmic-files %U")), TrashTarget::Folder);
        // Unknown ids: by the program they run, else the folder.
        assert_eq!(trash_target(&entry("my-nautilus.desktop", "/usr/bin/nautilus -w %U")), TrashTarget::Uri("trash:///"));
        assert_eq!(trash_target(&entry("my-dolphin.desktop", "env QT_QPA_PLATFORM=wayland dolphin %u")), TrashTarget::Uri("trash:/"));
        assert_eq!(trash_target(&entry("yazi.desktop", "yazi %u")), TrashTarget::Folder);
        assert_eq!(trash_target(&entry("org.kde.krusader.desktop", "krusader -qwindowtitle %c %u")), TrashTarget::Folder);
        // A known desktop id wins over its program (a flatpak wrapper).
        assert_eq!(trash_target(&entry("org.gnome.Nautilus.desktop", "/usr/bin/flatpak run org.gnome.Nautilus %U")), TrashTarget::Uri("trash:///"));
        // A command from Studio's Preferred apps.
        assert_eq!(trash_target(&Handler::command("thunar")), TrashTarget::Uri("trash:///"));
        assert_eq!(trash_target(&Handler::command("cosmic-files")), TrashTarget::Folder);
    }

    /// Opening the Trash handed `trash:///` to the system, which opened it
    /// with whatever handles the address: it goes to the file manager.
    #[test]
    fn the_trash_opens_in_the_file_manager() {
        let f = Fixture::new();
        let files = f.root().join("data/Trash/files");
        f.app("sys-apps", "org.kde.dolphin.desktop", "dolphin %u", "inode/directory;");
        f.app("sys-apps", "com.system76.CosmicFiles.desktop", "cosmic-files %U", "inode/directory;");
        f.system("[Default Applications]\ninode/directory=org.kde.dolphin.desktop;\n");
        let plan = plan_trash(&f.env, "", |_| false, &files).unwrap();
        assert_eq!((plan.handler.id.as_str(), plan.target), ("org.kde.dolphin.desktop", OpenTarget::Uri("trash:/".into())));
        let plan = plan_trash(&f.env, "com.system76.CosmicFiles", |_| false, &files).unwrap();
        assert_eq!(plan.target, OpenTarget::Path(files.clone()), "managers that can't open the address get the folder");
        let bare = OpenEnv { mimeapps: Vec::new(), app_dirs: Vec::new(), ..f.env.clone() };
        assert_eq!(plan_trash(&bare, "", |_| false, &files), Err(OpenError::NoFileManager));
        assert!(!files.exists());
        ensure_trash_files(&files).unwrap();
        use std::os::unix::fs::PermissionsExt as _;
        for dir in [files.as_path(), files.parent().unwrap()] {
            assert_eq!(std::fs::metadata(dir).unwrap().permissions().mode() & 0o777, 0o700, "{}", dir.display());
        }
        ensure_trash_files(&files).unwrap();
    }

    #[test]
    fn the_mimeapps_chain_puts_studios_file_first() {
        let chain = mimeapps_chain(&["Mango".into()], &[PathBuf::from("/usr/share")]);
        assert_eq!(chain[0], mimeapps_file());
        assert!(chain.iter().any(|p| p.ends_with("mango-mimeapps.list")));
        assert!(chain.ends_with(&[PathBuf::from("/usr/share/applications/mango-mimeapps.list"), PathBuf::from("/usr/share/applications/mimeapps.list"), PathBuf::from("/usr/share/applications/defaults.list")]));
        assert_eq!(chain.iter().filter(|p| **p == mimeapps_file()).count(), 1);
    }
}
