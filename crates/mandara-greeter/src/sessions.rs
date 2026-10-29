//! Desktop sessions from `wayland-sessions` / `xsessions` `.desktop` files,
//! and the greeter's small persistent state (last user / session).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SessionKind {
    Wayland,
    X11,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Session {
    /// File stem (`niri`, `hyprland`, `mango`), the stable key.
    pub id: String,
    /// Localized name.
    pub name: String,
    pub exec: Vec<String>,
    pub kind: SessionKind,
    pub desktop_names: Vec<String>,
    pub comment: String,
}

impl Session {
    /// The command greetd should run.
    pub fn command(&self) -> Vec<String> {
        match self.kind {
            SessionKind::Wayland => self.exec.clone(),
            // X sessions need an X server; startx runs the session's Exec
            // as its client.
            SessionKind::X11 => ["startx", "/usr/bin/env"].iter().map(|s| s.to_string()).chain(self.exec.iter().cloned()).collect(),
        }
    }

    /// Environment for the session (`KEY=VALUE`).
    pub fn env(&self) -> Vec<String> {
        let desktop = self.desktop_names.first().cloned().unwrap_or_else(|| self.id.clone());
        let mut env = vec![
            format!("XDG_SESSION_TYPE={}", if self.kind == SessionKind::Wayland { "wayland" } else { "x11" }),
            format!("XDG_SESSION_DESKTOP={desktop}"),
        ];
        if !self.desktop_names.is_empty() {
            env.push(format!("XDG_CURRENT_DESKTOP={}", self.desktop_names.join(":")));
        }
        env
    }
}

/// Splits an `Exec=` value into arguments (desktop-entry quoting rules,
/// field codes like `%f` removed).
pub fn split_exec(exec: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut has = false;
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                quoted = !quoted;
                has = true;
            }
            '\\' if quoted => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
            }
            c if c.is_whitespace() && !quoted => {
                if has || !cur.is_empty() {
                    args.push(std::mem::take(&mut cur));
                    has = false;
                }
            }
            c => cur.push(c),
        }
    }
    if has || !cur.is_empty() {
        args.push(cur);
    }
    args.into_iter()
        .filter_map(|a| {
            if a.len() == 2 && a.starts_with('%') {
                return None;
            }
            Some(a.replace("%%", "%"))
        })
        .collect()
}

/// Parses a session `.desktop` file. `lang` picks `Name[ar]`-style
/// translations. Hidden / NoDisplay entries and entries whose `TryExec`
/// is missing return `None`.
pub fn parse_desktop(id: &str, text: &str, kind: SessionKind, lang: &str, exists: &dyn Fn(&str) -> bool) -> Option<Session> {
    let mut in_entry = false;
    let mut fields: BTreeMap<String, String> = BTreeMap::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_entry = line == "[Desktop Entry]";
            continue;
        }
        if !in_entry || line.starts_with('#') {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            fields.entry(k.trim().to_string()).or_insert_with(|| v.trim().to_string());
        }
    }
    let flag = |k: &str| fields.get(k).is_some_and(|v| v.eq_ignore_ascii_case("true"));
    if flag("Hidden") || flag("NoDisplay") {
        return None;
    }
    if let Some(try_exec) = fields.get("TryExec").filter(|t| !t.is_empty())
        && !exists(try_exec)
    {
        return None;
    }
    let exec = split_exec(fields.get("Exec")?);
    if exec.is_empty() {
        return None;
    }
    let primary = lang.split(['_', '.', '@']).next().unwrap_or(lang);
    let localized = |key: &str| -> Option<String> {
        fields
            .get(&format!("{key}[{lang}]"))
            .or_else(|| fields.get(&format!("{key}[{primary}]")))
            .or_else(|| fields.get(key))
            .cloned()
    };
    Some(Session {
        id: id.to_string(),
        name: localized("Name").unwrap_or_else(|| id.to_string()),
        exec,
        kind,
        desktop_names: fields.get("DesktopNames").map(|v| v.split(';').filter(|s| !s.is_empty()).map(str::to_string).collect()).unwrap_or_default(),
        comment: localized("Comment").unwrap_or_default(),
    })
}

/// Whether a program exists (absolute path, or found in `$PATH`).
pub fn program_exists(prog: &str) -> bool {
    if prog.contains('/') {
        return Path::new(prog).exists();
    }
    let path = std::env::var_os("PATH").unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
    std::env::split_paths(&path).any(|d| d.join(prog).is_file())
}

/// Session directories (XDG data dirs + the usual system locations).
pub fn session_dirs() -> Vec<(PathBuf, SessionKind)> {
    let data = std::env::var("XDG_DATA_DIRS").unwrap_or_default();
    let mut roots: Vec<PathBuf> = data.split(':').filter(|s| !s.is_empty()).map(PathBuf::from).collect();
    for d in ["/usr/local/share", "/usr/share"] {
        if !roots.iter().any(|r| r == Path::new(d)) {
            roots.push(PathBuf::from(d));
        }
    }
    let mut out = Vec::new();
    for kind in [SessionKind::Wayland, SessionKind::X11] {
        for r in &roots {
            out.push((r.join(if kind == SessionKind::Wayland { "wayland-sessions" } else { "xsessions" }), kind));
        }
    }
    out
}

/// Every available session: Wayland first, then X11, each sorted by name;
/// the first file with a given id wins.
pub fn list_sessions(lang: &str) -> Vec<Session> {
    let mut out: Vec<Session> = Vec::new();
    for (dir, kind) in session_dirs() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        let mut found: Vec<Session> = entries
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension().is_none_or(|x| x != "desktop") {
                    return None;
                }
                let id = p.file_stem()?.to_string_lossy().to_string();
                let text = std::fs::read_to_string(&p).ok()?;
                parse_desktop(&id, &text, kind, lang, &program_exists)
            })
            .filter(|s| !out.iter().any(|o| o.id == s.id && o.kind == s.kind))
            .collect();
        found.sort_by_key(|s| s.name.to_lowercase());
        out.extend(found);
    }
    // An X11 entry with the same id as a Wayland one is a distinct session
    // (e.g. plasma vs plasmax11); mark it in the name.
    out
}

/// Remembered choices.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GreeterState {
    pub last_user: Option<String>,
    /// Last session id per user.
    pub sessions: BTreeMap<String, String>,
}

impl GreeterState {
    pub fn load(path: &Path) -> Self {
        std::fs::read_to_string(path).ok().and_then(|t| toml::from_str(&t).ok()).unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let text = toml::to_string(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(tmp, path)
    }

    /// The session to preselect for `user`: their last one if it still
    /// exists, else the first session.
    pub fn session_for(&self, user: &str, sessions: &[Session]) -> usize {
        self.sessions.get(user).and_then(|id| sessions.iter().position(|s| &s.id == id)).unwrap_or(0)
    }
}

/// Where the greeter keeps its state. The real greeter uses
/// `/var/lib/mandara-greeter` (created by `mandara greeter install`, owned
/// by the greeter user) and falls back to its cache dir; the demo uses the
/// invoking user's cache so it never touches system state.
pub fn state_path(demo: bool) -> PathBuf {
    if let Some(p) = std::env::var_os("MANDARA_GREETER_STATE") {
        return PathBuf::from(p);
    }
    let cache = dirs::cache_dir().unwrap_or_else(std::env::temp_dir);
    if demo {
        return cache.join("mandara").join("greeter-demo.toml");
    }
    let system = Path::new("/var/lib/mandara-greeter");
    if system.is_dir() && writable(system) {
        return system.join("state.toml");
    }
    cache.join("mandara-greeter").join("state.toml")
}

fn writable(dir: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt as _;
    let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) else { return false };
    // SAFETY: valid NUL-terminated path.
    unsafe { libc::access(c.as_ptr(), libc::W_OK) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NIRI: &str = "\
[Desktop Entry]
Name=Niri
Name[ar]=نيري
Comment=A scrollable-tiling Wayland compositor
Exec=niri-session
Type=Application
DesktopNames=niri
";

    #[test]
    fn parses_wayland_session() {
        let s = parse_desktop("niri", NIRI, SessionKind::Wayland, "en_US.UTF-8", &|_| true).unwrap();
        assert_eq!(s.name, "Niri");
        assert_eq!(s.exec, vec!["niri-session"]);
        assert_eq!(s.command(), vec!["niri-session"]);
        assert!(s.env().contains(&"XDG_SESSION_TYPE=wayland".to_string()));
        assert!(s.env().contains(&"XDG_CURRENT_DESKTOP=niri".to_string()));
        let ar = parse_desktop("niri", NIRI, SessionKind::Wayland, "ar_EG.UTF-8", &|_| true).unwrap();
        assert_eq!(ar.name, "نيري");
    }

    #[test]
    fn x11_sessions_go_through_startx() {
        let text = "[Desktop Entry]\nName=Xfce Session\nExec=startxfce4\nDesktopNames=XFCE\n";
        let s = parse_desktop("xfce", text, SessionKind::X11, "en", &|_| true).unwrap();
        assert_eq!(s.command(), vec!["startx", "/usr/bin/env", "startxfce4"]);
        assert!(s.env().contains(&"XDG_SESSION_TYPE=x11".to_string()));
    }

    #[test]
    fn skips_hidden_and_missing_tryexec() {
        let hidden = "[Desktop Entry]\nName=A\nExec=a\nNoDisplay=true\n";
        assert!(parse_desktop("a", hidden, SessionKind::Wayland, "en", &|_| true).is_none());
        let tryexec = "[Desktop Entry]\nName=B\nExec=b --flag\nTryExec=/nonexistent/b\n";
        assert!(parse_desktop("b", tryexec, SessionKind::Wayland, "en", &|p| p != "/nonexistent/b").is_none());
        assert!(parse_desktop("b", tryexec, SessionKind::Wayland, "en", &|_| true).is_some());
        let other_group = "[Desktop Action x]\nExec=nope\n[Desktop Entry]\nName=C\nExec=c\n";
        assert_eq!(parse_desktop("c", other_group, SessionKind::Wayland, "en", &|_| true).unwrap().exec, vec!["c"]);
        assert!(parse_desktop("d", "[Desktop Entry]\nName=D\n", SessionKind::Wayland, "en", &|_| true).is_none());
    }

    #[test]
    fn exec_splitting() {
        assert_eq!(split_exec("Hyprland"), vec!["Hyprland"]);
        assert_eq!(split_exec("env FOO=1 mango -s %U"), vec!["env", "FOO=1", "mango", "-s"]);
        assert_eq!(split_exec(r#""/opt/My WM/start" --name "a \"b\"" 100%%"#), vec!["/opt/My WM/start", "--name", "a \"b\"", "100%"]);
        assert_eq!(split_exec("  "), Vec::<String>::new());
    }

    #[test]
    fn state_round_trip_and_session_choice() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub/state.toml");
        let mut st = GreeterState::default();
        st.last_user = Some("layla".into());
        st.sessions.insert("layla".into(), "niri".into());
        st.save(&path).unwrap();
        let back = GreeterState::load(&path);
        assert_eq!(back, st);
        let sessions = vec![
            parse_desktop("mango", "[Desktop Entry]\nName=Mango\nExec=mango\n", SessionKind::Wayland, "en", &|_| true).unwrap(),
            parse_desktop("niri", NIRI, SessionKind::Wayland, "en", &|_| true).unwrap(),
        ];
        assert_eq!(back.session_for("layla", &sessions), 1);
        assert_eq!(back.session_for("someone", &sessions), 0);
        assert_eq!(GreeterState::load(&dir.path().join("missing.toml")), GreeterState::default());
    }
}
