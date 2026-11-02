//! Local Git repositories for the `git-repos` plugin: finds repositories
//! under a few folders and reads each one's state with the `git` CLI
//! (`git status --porcelain=v2 --branch` and `git log -1`).
//!
//! Discovery walks each root up to a depth, never descends into a
//! repository it found (so submodules and nested worktrees stay out), and
//! skips `node_modules`, `target` and hidden folders. Roots reached through
//! symlinks are de-duplicated. Status reads run on a few threads at once
//! and never take Git's index lock (`--no-optional-locks`), so they can't
//! disturb a commit in progress. Everything here blocks: call it off the
//! UI thread.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use mandara_i18n::{t, tf};
use serde::Serialize;

/// The default `roots` setting.
pub const DEFAULT_ROOTS: &str = "~/projects, ~/Projects, ~/code, ~/src, ~/dev, ~/git";
/// The default `depth` setting and its range.
pub const DEFAULT_DEPTH: i64 = 3;
pub const DEPTH_RANGE: (i64, i64) = (1, 6);
/// Repositories listed at most (a guard against scanning a whole disk).
pub const MAX_REPOS: usize = 200;
/// Folders never descended into (besides hidden ones).
const SKIPPED_DIRS: [&str; 2] = ["node_modules", "target"];
/// Status reads running at once.
const WORKERS: usize = 6;
/// Field separator of the `git log` format (ASCII unit separator).
const LOG_SEP: char = '\u{1f}';
/// `git log` format: committer time, author name, subject.
const LOG_FORMAT: &str = "--format=%ct%x1f%an%x1f%s";

/// What `git status --porcelain=v2 --branch` reports.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Status {
    /// `None` on a detached HEAD.
    pub branch: Option<String>,
    /// The commit HEAD points at (`None` before the first commit).
    pub head: Option<String>,
    pub upstream: Option<String>,
    pub ahead: u32,
    pub behind: u32,
    /// Changes in the index.
    pub staged: u32,
    /// Changes in the working tree not yet staged.
    pub modified: u32,
    pub untracked: u32,
    /// Unmerged paths (a merge or rebase in progress).
    pub conflicts: u32,
}

impl Status {
    pub fn dirty(&self) -> bool {
        self.staged + self.modified + self.untracked + self.conflicts > 0
    }
}

/// `+3 -1` of `# branch.ab`.
fn ahead_behind(s: &str) -> (u32, u32) {
    let mut it = s.split_whitespace();
    let mut num = |prefix: char| it.next().and_then(|w| w.strip_prefix(prefix)).and_then(|n| n.parse().ok()).unwrap_or(0);
    let ahead = num('+');
    (ahead, num('-'))
}

/// Parses `git status --porcelain=v2 --branch` output.
pub fn parse_status(text: &str) -> Status {
    let mut st = Status::default();
    for line in text.lines() {
        if let Some(header) = line.strip_prefix("# ") {
            let (key, value) = header.split_once(' ').unwrap_or((header, ""));
            match key {
                "branch.oid" => st.head = (value != "(initial)").then(|| value.to_string()),
                "branch.head" => st.branch = (value != "(detached)").then(|| value.to_string()),
                "branch.upstream" => st.upstream = Some(value.to_string()),
                "branch.ab" => (st.ahead, st.behind) = ahead_behind(value),
                _ => {}
            }
            continue;
        }
        let mut fields = line.splitn(3, ' ');
        match (fields.next(), fields.next()) {
            // Ordinary (1) and renamed / copied (2) entries: `XY`, where X
            // is the index side and Y the working tree ('.' = unchanged).
            (Some("1" | "2"), Some(xy)) => {
                let mut c = xy.chars();
                if c.next().is_some_and(|x| x != '.') {
                    st.staged += 1;
                }
                if c.next().is_some_and(|y| y != '.') {
                    st.modified += 1;
                }
            }
            (Some("u"), Some(_)) => st.conflicts += 1,
            (Some("?"), Some(_)) => st.untracked += 1,
            _ => {}
        }
    }
    st
}

/// The last commit.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Commit {
    /// Unix seconds (committer time).
    pub time: i64,
    pub author: String,
    pub subject: String,
}

/// Parses `git log -1 --format=%ct%x1f%an%x1f%s`.
pub fn parse_log(text: &str) -> Option<Commit> {
    let line = text.lines().next()?;
    let mut f = line.splitn(3, LOG_SEP);
    let time = f.next()?.trim().parse().ok()?;
    Some(Commit { time, author: f.next().unwrap_or_default().to_string(), subject: f.next().unwrap_or_default().to_string() })
}

/// One repository's state.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Repo {
    pub path: PathBuf,
    /// The folder name.
    pub name: String,
    pub status: Status,
    pub last_commit: Option<Commit>,
    /// Unix seconds of the latest activity: the last commit or the last
    /// change to the index (staging, checkouts), whichever is newer.
    pub activity: i64,
    /// Why the status couldn't be read.
    pub error: Option<String>,
}

impl Repo {
    /// Uncommitted changes or commits not pushed.
    pub fn needs_attention(&self) -> bool {
        self.status.dirty() || self.status.ahead > 0
    }
}

/// `~/x` → `$HOME/x`.
fn expand(path: &str) -> PathBuf {
    let home = || dirs::home_dir().unwrap_or_default();
    match path.strip_prefix("~/") {
        Some(rest) => home().join(rest),
        None if path == "~" => home(),
        None => PathBuf::from(path),
    }
}

/// The comma-separated `roots` setting → existing folders, without
/// duplicates (symlinked roots resolve to one).
pub fn roots(setting: &str) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    setting
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| std::fs::canonicalize(expand(s)).ok())
        .filter(|p| p.is_dir() && seen.insert(p.clone()))
        .collect()
}

fn is_repo(dir: &Path) -> bool {
    // A folder (normal clone) or a file (worktree, submodule checkout).
    dir.join(".git").exists()
}

fn skipped(name: &str) -> bool {
    name.starts_with('.') || SKIPPED_DIRS.contains(&name)
}

/// Repositories under `roots`, `depth` levels deep at most (a root itself
/// is level 0). Symlinked folders are not followed below the roots.
pub fn discover(roots: &[PathBuf], depth: usize) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = Vec::new();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let mut stack: Vec<(PathBuf, usize)> = roots.iter().map(|r| (r.clone(), 0)).collect();
    while let Some((dir, level)) = stack.pop() {
        if found.len() >= MAX_REPOS {
            break;
        }
        if is_repo(&dir) {
            if seen.insert(dir.clone()) {
                found.push(dir);
            }
            continue;
        }
        if level >= depth {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            if !kind.is_dir() || entry.file_name().to_str().is_none_or(skipped) {
                continue;
            }
            stack.push((entry.path(), level + 1));
        }
    }
    found.sort();
    found
}

/// `git -C <dir> …` with prompts disabled and no stdin.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .output()
        .map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { t("Git isn't installed").to_string() } else { e.to_string() })?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        Err(err.lines().find(|l| !l.trim().is_empty()).unwrap_or_default().trim().to_string())
    }
}

/// Unix seconds of a file's last change.
fn mtime(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    modified.duration_since(std::time::UNIX_EPOCH).ok().and_then(|d| i64::try_from(d.as_secs()).ok())
}

/// Reads one repository.
pub fn inspect(path: &Path) -> Repo {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.display().to_string());
    let (status, error) = match git(path, &["--no-optional-locks", "status", "--porcelain=v2", "--branch"]) {
        Ok(text) => (parse_status(&text), None),
        Err(e) => (Status::default(), Some(e)),
    };
    // An empty repository has no commit: `git log` fails, which is fine.
    let last_commit = if status.head.is_some() { git(path, &["--no-optional-locks", "log", "-1", LOG_FORMAT]).ok().as_deref().and_then(parse_log) } else { None };
    let index = mtime(&path.join(".git/index")).unwrap_or(0);
    let activity = last_commit.as_ref().map_or(0, |c| c.time).max(index);
    Repo { path: path.to_path_buf(), name, status, last_commit, activity, error }
}

/// Finds and reads every repository, most recent activity first.
pub fn scan(roots: &[PathBuf], depth: usize) -> Vec<Repo> {
    let paths = discover(roots, depth);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let results = std::sync::Mutex::new(Vec::with_capacity(paths.len()));
    std::thread::scope(|s| {
        for _ in 0..WORKERS.min(paths.len()) {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(p) = paths.get(i) else { break };
                    let repo = inspect(p);
                    if let Ok(mut r) = results.lock() {
                        r.push(repo);
                    }
                }
            });
        }
    });
    let mut repos = results.into_inner().unwrap_or_default();
    sort(&mut repos);
    repos
}

/// Most recent activity first, then by name.
pub fn sort(repos: &mut [Repo]) {
    repos.sort_by(|a, b| b.activity.cmp(&a.activity).then_with(|| a.name.cmp(&b.name)));
}

/// `git fetch` in `path` (no prompts: a remote needing a password fails).
pub fn fetch(path: &Path) -> Result<(), String> {
    git(path, &["fetch", "--quiet", "--prune"]).map(|_| ()).map_err(|e| if e.is_empty() { tf("Fetching {repo} failed", &[("repo", &path.display())]) } else { e })
}

#[cfg(test)]
mod tests {
    use super::*;

    const DIRTY: &str = "# branch.oid 4f1c2a9e0d4b7f1e2c3a5b6d7e8f9a0b1c2d3e4f
# branch.head main
# branch.upstream origin/main
# branch.ab +2 -1
1 .M N... 100644 100644 100644 3f1e2d 3f1e2d src/main.rs
1 M. N... 100644 100644 100644 aa11bb aa22cc Cargo.toml
1 MM N... 100644 100644 100644 aa11bb aa22cc README.md
2 R. N... 100644 100644 100644 aa11bb aa11bb R100 src/new name.rs\tsrc/old.rs
u UU N... 100644 100644 100644 100644 a1 b2 c3 conflict.rs
? notes/todo.md
? scratch.txt
! target/
";

    #[test]
    fn parses_porcelain_v2_branch_and_counts() {
        let st = parse_status(DIRTY);
        assert_eq!(st.branch.as_deref(), Some("main"));
        assert_eq!(st.upstream.as_deref(), Some("origin/main"));
        assert_eq!(st.head.as_deref(), Some("4f1c2a9e0d4b7f1e2c3a5b6d7e8f9a0b1c2d3e4f"));
        assert_eq!((st.ahead, st.behind), (2, 1));
        assert_eq!((st.staged, st.modified, st.untracked, st.conflicts), (3, 2, 2, 1));
        assert!(st.dirty());
    }

    #[test]
    fn parses_clean_detached_and_initial_heads() {
        let clean = parse_status("# branch.oid 4f1c2a\n# branch.head feature/x\n");
        assert_eq!((clean.branch.as_deref(), clean.upstream.as_deref(), clean.ahead, clean.dirty()), (Some("feature/x"), None, 0, false));
        let detached = parse_status("# branch.oid 4f1c2a\n# branch.head (detached)\n");
        assert_eq!(detached.branch, None);
        assert!(detached.head.is_some());
        let initial = parse_status("# branch.oid (initial)\n# branch.head main\n? a.txt\n");
        assert_eq!((initial.head, initial.untracked), (None, 1));
    }

    #[test]
    fn parses_the_last_commit() {
        let c = parse_log("1793597448\u{1f}Ahmad Hamdi\u{1f}feat: round the corners (#19)\n").unwrap();
        assert_eq!((c.time, c.author.as_str(), c.subject.as_str()), (1_793_597_448, "Ahmad Hamdi", "feat: round the corners (#19)"));
        // A separator in the subject stays in the subject.
        assert_eq!(parse_log("1\u{1f}a\u{1f}x\u{1f}y").unwrap().subject, "x\u{1f}y");
        assert!(parse_log("").is_none());
    }

    #[test]
    fn discovery_stops_at_repos_and_skips_noise() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        for p in ["a/.git", "a/sub/.git", "group/b/.git", "node_modules/c/.git", ".hidden/d/.git", "target/e/.git", "deep/1/2/3/f/.git"] {
            std::fs::create_dir_all(root.join(p)).unwrap();
        }
        let found: Vec<String> = discover(&[root.to_path_buf()], 3).iter().map(|p| p.strip_prefix(root).unwrap().display().to_string()).collect();
        assert_eq!(found, ["a", "group/b"]);
        let deeper: Vec<String> = discover(&[root.to_path_buf()], 5).iter().map(|p| p.strip_prefix(root).unwrap().display().to_string()).collect();
        assert!(deeper.contains(&"deep/1/2/3/f".to_string()));
    }

    #[test]
    fn roots_expand_and_dedupe() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        std::fs::create_dir_all(&real).unwrap();
        std::os::unix::fs::symlink(&real, dir.path().join("link")).unwrap();
        let setting = format!("{0}/real, {0}/link, {0}/missing,  ", dir.path().display());
        assert_eq!(roots(&setting).len(), 1);
        assert_eq!(expand("~/x"), dirs::home_dir().unwrap().join("x"));
    }

    #[test]
    fn sorts_by_activity_and_flags_attention() {
        let repo = |name: &str, activity: i64, ahead: u32| Repo {
            path: PathBuf::from(name),
            name: name.into(),
            status: Status { ahead, ..Status::default() },
            last_commit: None,
            activity,
            error: None,
        };
        let mut v = vec![repo("old", 10, 0), repo("new", 30, 1), repo("mid", 20, 0)];
        sort(&mut v);
        assert_eq!(v.iter().map(|r| r.name.as_str()).collect::<Vec<_>>(), ["new", "mid", "old"]);
        assert!(v[0].needs_attention() && !v[1].needs_attention());
    }
}
