//! Desktop applications: `.desktop` discovery, icon lookup, launching, and
//! a tiny calculator for the launcher.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct App {
    pub id: String,
    pub name: String,
    pub comment: String,
    pub exec: String,
    pub icon: Option<PathBuf>,
    pub keywords: String,
    pub terminal: bool,
    /// `StartupWMClass`: the window app id when it differs from the id.
    pub wm_class: Option<String>,
    /// `[Desktop Action …]` groups listed in `Actions=`.
    pub actions: Vec<DesktopAction>,
    /// Where the entry was installed from.
    pub origin: AppOrigin,
    /// `Categories=`, e.g. `Development;IDE`.
    pub categories: String,
}

/// A desktop entry action (`New Window`, `New Private Window`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DesktopAction {
    pub id: String,
    pub name: String,
    pub exec: String,
}

/// Packaging of an app, from where its desktop file lives.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum AppOrigin {
    /// Installed by the system package manager.
    #[default]
    System,
    Flatpak,
    Snap,
    /// Desktop entries in `~/.local/share/applications` (AppImages,
    /// web apps, your own launchers).
    User,
    /// An AppImage (Exec points at a `.AppImage`).
    AppImage,
}

impl AppOrigin {
    /// Short badge text (`None` for system packages).
    pub fn badge(self) -> Option<&'static str> {
        match self {
            AppOrigin::System => None,
            AppOrigin::Flatpak => Some("Flatpak"),
            AppOrigin::Snap => Some("Snap"),
            AppOrigin::User => Some(mandara_i18n::tk("Local")),
            AppOrigin::AppImage => Some("AppImage"),
        }
    }

    /// Classifies a desktop file by its path and `Exec` line.
    pub fn of(path: &std::path::Path, exec: &str) -> Self {
        let p = path.to_string_lossy();
        if p.contains("/flatpak/exports/") || exec.split_whitespace().next().is_some_and(|w| w.ends_with("flatpak")) {
            AppOrigin::Flatpak
        } else if p.contains("/snapd/") || p.starts_with("/snap/") {
            AppOrigin::Snap
        } else if exec.to_ascii_lowercase().contains(".appimage") {
            AppOrigin::AppImage
        } else if dirs::data_dir().is_some_and(|d| path.starts_with(d.join("applications"))) {
            AppOrigin::User
        } else {
            AppOrigin::System
        }
    }
}

impl App {
    /// Text the fuzzy matcher scores against.
    pub fn haystack(&self) -> String {
        format!("{} {} {}", self.name, self.keywords, self.id)
    }
}

/// Parses the `[Desktop Action <id>]` groups named in `Actions=`, in that
/// order (actions without a name or command are skipped).
pub fn parse_desktop_actions(text: &str) -> Vec<DesktopAction> {
    let Some(entry) = parse_desktop_entry(text) else { return Vec::new() };
    let ids: Vec<&str> = entry.get("Actions").map(|a| a.split(';').map(str::trim).filter(|s| !s.is_empty()).collect()).unwrap_or_default();
    let mut found: HashMap<String, (Option<String>, Option<String>)> = HashMap::new();
    let mut current: Option<String> = None;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            current = line.strip_prefix("[Desktop Action ").and_then(|r| r.strip_suffix(']')).map(str::to_string);
            continue;
        }
        let (Some(id), Some((k, v))) = (current.as_ref(), line.split_once('=')) else { continue };
        let slot = found.entry(id.clone()).or_default();
        match k.trim() {
            "Name" if slot.0.is_none() => slot.0 = Some(v.trim().to_string()),
            "Exec" if slot.1.is_none() => slot.1 = Some(v.trim().to_string()),
            _ => {}
        }
    }
    ids.into_iter()
        .filter_map(|id| match found.get(id) {
            Some((Some(name), Some(exec))) => Some(DesktopAction { id: id.to_string(), name: name.clone(), exec: exec.clone() }),
            _ => None,
        })
        .collect()
}

fn data_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = dirs::data_dir() {
        dirs.push(home);
    }
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').filter(|s| !s.is_empty()).map(PathBuf::from));
    dirs.push(PathBuf::from("/var/lib/flatpak/exports/share"));
    if let Some(home) = dirs::data_dir() {
        dirs.push(home.join("flatpak/exports/share"));
    }
    dirs
}

/// Parses the `[Desktop Entry]` group of a `.desktop` file.
pub fn parse_desktop_entry(text: &str) -> Option<HashMap<String, String>> {
    let mut in_entry = false;
    let mut map = HashMap::new();
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
            map.entry(k.trim().to_string()).or_insert_with(|| v.trim().to_string());
        }
    }
    (!map.is_empty()).then_some(map)
}

/// Removes `%f %U …` field codes from an `Exec` line.
pub fn strip_field_codes(exec: &str) -> String {
    let mut out = Vec::new();
    for word in exec.split_whitespace() {
        if word.len() == 2 && word.starts_with('%') {
            continue;
        }
        out.push(word.replace("%%", "%"));
    }
    out.join(" ")
}

/// `key[lang]` (e.g. `Name[ar]`) when the entry translates it, else `key`.
fn localized<'a>(map: &'a HashMap<String, String>, key: &str, lang: &str) -> Option<&'a String> {
    map.get(&format!("{key}[{lang}]")).or_else(|| map.get(key))
}

/// Finds a full-color icon file by name in the user's icon theme chain.
pub fn find_icon(name: &str) -> Option<PathBuf> {
    crate::icon_theme::resolve(name, None, false).map(|r| r.path)
}

/// Scans every applications directory. Later directories never override an
/// earlier (user) entry with the same id.
pub fn discover() -> Vec<App> {
    let mut seen = std::collections::HashSet::new();
    let mut apps = Vec::new();
    for dir in data_dirs() {
        let Ok(entries) = std::fs::read_dir(dir.join("applications")) else { continue };
        let mut files: Vec<_> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "desktop")).collect();
        files.sort();
        for path in files {
            let id = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
            if !seen.insert(id.clone()) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let Some(map) = parse_desktop_entry(&text) else { continue };
            let flag = |k: &str| map.get(k).is_some_and(|v| v == "true");
            if map.get("Type").is_some_and(|t| t != "Application") || flag("NoDisplay") || flag("Hidden") {
                continue;
            }
            let lang = mandara_i18n::language().code();
            let (Some(name), Some(exec)) = (localized(&map, "Name", lang), map.get("Exec")) else { continue };
            // English keywords stay searchable next to the translated ones.
            let keywords = [map.get("Keywords"), map.get(&format!("Keywords[{lang}]"))].into_iter().flatten().cloned().collect::<Vec<_>>().join(";");
            apps.push(App {
                id,
                name: name.clone(),
                comment: localized(&map, "Comment", lang).or_else(|| localized(&map, "GenericName", lang)).cloned().unwrap_or_default(),
                exec: exec.clone(),
                icon: map.get("Icon").and_then(|i| find_icon(i)),
                keywords: keywords.replace(';', " "),
                terminal: flag("Terminal"),
                wm_class: map.get("StartupWMClass").filter(|c| !c.is_empty()).cloned(),
                actions: parse_desktop_actions(&text),
                origin: AppOrigin::of(&path, exec),
                categories: map.get("Categories").cloned().unwrap_or_default(),
            });
        }
    }
    apps.sort_by_key(|a| a.name.to_lowercase());
    apps
}

/// Resolves a window's app id (Wayland `app_id` / X11 class) to an icon in
/// the user's icon theme: via desktop entries (exact id, `StartupWMClass`,
/// id tail), then the app id itself. With `prefer_symbolic`, monochrome
/// `-symbolic` SVGs win (they tint with the palette).
/// Studio → Applications' app-id aliases (`steam_app_*` → `steam`).
static ALIASES: std::sync::RwLock<Vec<mandara_config::apps::AppIdAlias>> = std::sync::RwLock::new(Vec::new());

/// Replaces the app-id aliases (kept in step with `apps.id_map`).
pub fn set_app_id_aliases(list: Vec<mandara_config::apps::AppIdAlias>) {
    if let Ok(mut a) = ALIASES.write() {
        *a = list;
    }
}

/// `app_id`, or the desktop id an alias maps it to.
fn aliased(app_id: &str) -> std::borrow::Cow<'_, str> {
    match ALIASES.read().ok().and_then(|a| mandara_config::apps::alias_for(&a, app_id).map(str::to_string)) {
        Some(d) => std::borrow::Cow::Owned(d),
        None => std::borrow::Cow::Borrowed(app_id),
    }
}

pub fn icon_for_app_id(app_id: &str, prefer_symbolic: bool) -> Option<crate::icon_theme::ResolvedIcon> {
    if app_id.is_empty() {
        return None;
    }
    let app_id = &*aliased(app_id);
    let key = app_id.to_lowercase();
    // Reverse-DNS ids (`org.mandara.studio`) only match exactly: their last
    // segment ("studio") would match unrelated apps (`com.obsproject.Studio`).
    let last = (!key.contains('.')).then(|| key.clone());
    let index = &entry_index().icons;
    let names: Vec<String> = [index.get(&key).cloned(), last.as_ref().and_then(|l| index.get(l).cloned()), Some(app_id.to_string()), Some(key.clone())]
        .into_iter()
        .flatten()
        .collect();
    names.iter().find_map(|n| crate::icon_theme::resolve(n, None, prefer_symbolic))
}

/// The real name of a window's app from its desktop entry (matched like
/// [`icon_for_app_id`]), e.g. `chrome-<id>-Default` → `WhatsApp Web`.
pub fn name_for_app_id(app_id: &str) -> Option<String> {
    if app_id.is_empty() {
        return None;
    }
    let app_id = &*aliased(app_id);
    // Mandara's own windows, whether or not a desktop entry is installed.
    match app_id {
        mandara_core::STUDIO_APP_ID => return Some(mandara_i18n::t("Mandara Studio").to_string()),
        mandara_core::SHELL_APP_ID => return Some(mandara_i18n::t("Mandara").to_string()),
        _ => {}
    }
    let key = app_id.to_lowercase();
    let names = &entry_index().names;
    // Exact desktop id / StartupWMClass; reverse-DNS ids never match by their
    // last segment alone (see `icon_for_app_id`).
    names.get(&key).cloned()
}

/// Desktop entries keyed by lowercase desktop id, `StartupWMClass` and id
/// tail: icon names and display names.
struct EntryIndex {
    icons: HashMap<String, String>,
    names: HashMap<String, String>,
}

fn entry_index() -> &'static EntryIndex {
    use std::sync::OnceLock;
    static INDEX: OnceLock<EntryIndex> = OnceLock::new();
    INDEX.get_or_init(build_entry_index)
}

fn build_entry_index() -> EntryIndex {
    let mut icons = HashMap::new();
    let mut names = HashMap::new();
    for dir in data_dirs() {
        let Ok(entries) = std::fs::read_dir(dir.join("applications")) else { continue };
        for path in entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "desktop")) {
            let Ok(text) = std::fs::read_to_string(&path) else { continue };
            let Some(map) = parse_desktop_entry(&text) else { continue };
            let id = path.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_lowercase();
            let tail = id.rsplit('.').next().unwrap_or(&id).to_string();
            let mut keys = vec![id, tail];
            if let Some(class) = map.get("StartupWMClass") {
                keys.insert(0, class.to_lowercase());
            }
            for (field, index) in [("Icon", &mut icons), ("Name", &mut names)] {
                if let Some(value) = map.get(field).filter(|v| !v.is_empty()) {
                    for k in &keys {
                        index.entry(k.clone()).or_insert_with(|| value.clone());
                    }
                }
            }
        }
    }
    EntryIndex { icons, names }
}

/// Splits an `Exec` value into argv per the Desktop Entry spec: double
/// quotes group, and inside them `\"`, `\``, `\$` and `\\` escape; field
/// codes (`%f`, `%U`, …) are dropped and `%%` becomes `%`.
pub fn parse_exec(exec: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    let mut has_token = false;
    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                has_token = true;
            }
            '\\' if in_quotes => {
                if let Some(&n) = chars.peek()
                    && matches!(n, '"' | '`' | '$' | '\\')
                {
                    cur.push(n);
                    chars.next();
                } else {
                    cur.push(c);
                }
            }
            c if c.is_whitespace() && !in_quotes => {
                if has_token {
                    args.push(std::mem::take(&mut cur));
                    has_token = false;
                }
            }
            c => {
                cur.push(c);
                has_token = true;
            }
        }
    }
    if has_token {
        args.push(cur);
    }
    args.into_iter()
        .filter(|a| !(a.len() == 2 && a.starts_with('%') && a != "%%"))
        .map(|a| a.replace("%%", "%"))
        .filter(|a| !a.is_empty())
        .collect()
}

/// Terminals tried, in order, for `Terminal=true` apps when neither the
/// launcher setting nor `$TERMINAL` names one: `(program, flag that runs
/// a command)`.
const TERMINALS: &[(&str, &str)] = &[("ghostty", "-e"), ("kitty", "-e"), ("foot", "-e"), ("alacritty", "-e"), ("wezterm", "start --"), ("konsole", "-e"), ("gnome-terminal", "--"), ("xterm", "-e")];

/// Whether `program` is an executable on `$PATH` (or an existing path).
pub fn on_path(program: &str) -> bool {
    if program.contains('/') {
        return std::path::Path::new(program).exists();
    }
    std::env::var_os("PATH").is_some_and(|path| std::env::split_paths(&path).any(|d| d.join(program).is_file()))
}

/// The argv prefix that runs a command in a terminal: the configured
/// command (`ghostty -e`), else `$TERMINAL -e`, else the first installed
/// well-known terminal.
pub fn terminal_prefix(configured: &str) -> Vec<String> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let words = parse_exec(configured);
        // A bare program name gets its own "run this" flag (`wezterm start
        // --`, `gnome-terminal --`), else the usual `-e`.
        return if words.len() == 1 { with_run_flag(&words[0]) } else { words };
    }
    if let Ok(term) = std::env::var("TERMINAL")
        && !term.trim().is_empty()
    {
        return with_run_flag(term.trim());
    }
    let (program, flag) = TERMINALS.iter().find(|(p, _)| on_path(p)).copied().unwrap_or(TERMINALS[0]);
    std::iter::once(program.to_string()).chain(flag.split_whitespace().map(str::to_string)).collect()
}

/// `program` plus the flag that makes it run a command, from
/// [`TERMINALS`] by its file name, else `-e`.
fn with_run_flag(program: &str) -> Vec<String> {
    let base = std::path::Path::new(program).file_name().and_then(|n| n.to_str()).unwrap_or(program);
    let flag = TERMINALS.iter().find(|(p, _)| *p == base).map_or("-e", |(_, f)| f);
    std::iter::once(program.to_string()).chain(flag.split_whitespace().map(str::to_string)).collect()
}

/// The full argv for running `exec` under the launch options: terminal
/// for `Terminal=true` apps, then the wrapper (`uwsm app --`).
pub fn launch_argv(exec: &str, terminal: bool, options: &mandara_config::workflow::LaunchOptions) -> Vec<String> {
    let mut argv = parse_exec(exec);
    if terminal {
        argv.splice(0..0, terminal_prefix(&options.terminal));
    }
    let wrapper = parse_exec(options.wrapper.trim());
    argv.splice(0..0, wrapper);
    argv
}

/// Launches an app detached from the shell: argv is executed directly (no
/// shell) in its own process group.
pub fn launch(app: &App) -> Result<(), String> {
    launch_with(app, None, &mandara_config::workflow::LaunchOptions::default())
}

/// Launches an app (or one of its desktop actions) with the launch options:
/// wrapper, terminal, and optionally its own systemd scope.
pub fn launch_with(app: &App, action: Option<&DesktopAction>, options: &mandara_config::workflow::LaunchOptions) -> Result<(), String> {
    use std::os::unix::process::CommandExt as _;
    let exec = action.map_or(app.exec.as_str(), |a| a.exec.as_str());
    let argv = launch_argv(exec, app.terminal, options);
    let (program, args) = argv.split_first().ok_or_else(|| format!("{} has an empty Exec line", app.name))?;
    let child = Command::new(program)
        .args(args)
        .process_group(0)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("launching {}: {e}", app.name))?;
    let pid = child.id();
    // Waited for in the background, so it never lingers as a zombie.
    mandara_services::reaper::adopt(child);
    if options.systemd_scope {
        let (id, name) = (app.id.clone(), app.name.clone());
        // A D-Bus round trip: off the UI thread.
        std::thread::spawn(move || {
            if let Err(err) = mandara_services::app_scope::move_into_scope(&id, pid, &name) {
                tracing::warn!("systemd scope for {name}: {err}");
            }
        });
    }
    Ok(())
}

/// Evaluates arithmetic: `+ - * / ^` (also `x`, `×`, `÷`, `**`), `%` as
/// modulo between numbers and as percent after one (`50%` = 0.5,
/// `200 * 15%` = 30), `!` factorial, parentheses, unary minus below `^`
/// (`-2^2` = -4), scientific notation (`1.5e3`), the constants `pi`, `e`,
/// `tau` and the functions `sqrt cbrt abs ln log log2 log10 exp sin cos tan
/// asin acos atan floor ceil round` (radians).
pub fn calculate(expr: &str) -> Option<f64> {
    struct P<'a> {
        s: &'a [u8],
        i: usize,
    }
    impl P<'_> {
        fn ws(&mut self) {
            while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
                self.i += 1;
            }
        }
        fn peek(&mut self) -> Option<u8> {
            self.ws();
            self.s.get(self.i).copied()
        }
        /// The first non-space byte after the current one.
        fn peek_after(&self) -> Option<u8> {
            self.s[self.i + 1..].iter().copied().find(|b| !b.is_ascii_whitespace())
        }
        fn expr(&mut self) -> Option<f64> {
            let mut v = self.term()?;
            while let Some(c @ (b'+' | b'-')) = self.peek() {
                self.i += 1;
                let r = self.term()?;
                v = if c == b'+' { v + r } else { v - r };
            }
            Some(v)
        }
        fn term(&mut self) -> Option<f64> {
            let mut v = self.unary()?;
            while let Some(c @ (b'*' | b'/' | b'%' | b'x')) = self.peek() {
                self.i += 1;
                let r = self.unary()?;
                v = match c {
                    b'/' => v / r,
                    b'%' => v % r,
                    _ => v * r,
                };
            }
            Some(v)
        }
        fn unary(&mut self) -> Option<f64> {
            match self.peek() {
                Some(b'-') => {
                    self.i += 1;
                    Some(-self.unary()?)
                }
                Some(b'+') => {
                    self.i += 1;
                    self.unary()
                }
                _ => self.power(),
            }
        }
        /// Right-associative; the exponent may be signed (`2^-1`).
        fn power(&mut self) -> Option<f64> {
            let base = self.postfix()?;
            if self.peek() == Some(b'^') {
                self.i += 1;
                return Some(base.powf(self.unary()?));
            }
            Some(base)
        }
        fn postfix(&mut self) -> Option<f64> {
            let mut v = self.atom()?;
            loop {
                match self.peek() {
                    // `%` followed by an operand is modulo (handled by `term`).
                    Some(b'%') if !self.peek_after().is_some_and(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'(') => {
                        self.i += 1;
                        v /= 100.0;
                    }
                    Some(b'!') => {
                        self.i += 1;
                        if v < 0.0 || v.fract() != 0.0 || v > 170.0 {
                            return None;
                        }
                        v = (1..=v as u32).map(f64::from).product();
                    }
                    _ => return Some(v),
                }
            }
        }
        fn atom(&mut self) -> Option<f64> {
            match self.peek()? {
                b'(' => {
                    self.i += 1;
                    let v = self.expr()?;
                    if self.peek() != Some(b')') {
                        return None;
                    }
                    self.i += 1;
                    Some(v)
                }
                b'0'..=b'9' | b'.' => {
                    let start = self.i;
                    while self.i < self.s.len() && (self.s[self.i].is_ascii_digit() || self.s[self.i] == b'.') {
                        self.i += 1;
                    }
                    // Exponent: `1e3`, `2.5E-4` (a bare `e` is the constant).
                    if matches!(self.s.get(self.i), Some(b'e' | b'E')) {
                        let mut j = self.i + 1;
                        if matches!(self.s.get(j), Some(b'+' | b'-')) {
                            j += 1;
                        }
                        if self.s.get(j).is_some_and(u8::is_ascii_digit) {
                            self.i = j;
                            while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
                                self.i += 1;
                            }
                        }
                    }
                    std::str::from_utf8(&self.s[start..self.i]).ok()?.parse().ok()
                }
                b'a'..=b'z' | b'A'..=b'Z' => {
                    let start = self.i;
                    while self.i < self.s.len() && self.s[self.i].is_ascii_alphanumeric() {
                        self.i += 1;
                    }
                    let name = std::str::from_utf8(&self.s[start..self.i]).ok()?.to_ascii_lowercase();
                    match name.as_str() {
                        "pi" => return Some(std::f64::consts::PI),
                        "e" => return Some(std::f64::consts::E),
                        "tau" => return Some(std::f64::consts::TAU),
                        _ => {}
                    }
                    let f: fn(f64) -> f64 = match name.as_str() {
                        "sqrt" => f64::sqrt,
                        "cbrt" => f64::cbrt,
                        "abs" => f64::abs,
                        "ln" => f64::ln,
                        "log" | "log10" => f64::log10,
                        "log2" => f64::log2,
                        "exp" => f64::exp,
                        "sin" => f64::sin,
                        "cos" => f64::cos,
                        "tan" => f64::tan,
                        "asin" => f64::asin,
                        "acos" => f64::acos,
                        "atan" => f64::atan,
                        "floor" => f64::floor,
                        "ceil" => f64::ceil,
                        "round" => f64::round,
                        _ => return None,
                    };
                    // `sqrt(16)^2` applies to the parenthesis; `sqrt 16` to the operand.
                    let arg = if self.peek() == Some(b'(') { self.atom()? } else { self.unary()? };
                    Some(f(arg))
                }
                _ => None,
            }
        }
    }
    let normalized = expr.replace('×', "*").replace('÷', "/").replace('−', "-").replace("**", "^").replace('π', "pi");
    let mut p = P { s: normalized.as_bytes(), i: 0 };
    let v = p.expr()?;
    (p.peek().is_none() && v.is_finite()).then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Studio → Applications' terminal runs `Terminal=true` apps with the
    /// right flag for that terminal.
    #[test]
    fn configured_terminals_get_their_run_flag() {
        assert_eq!(terminal_prefix("wezterm"), ["wezterm", "start", "--"]);
        assert_eq!(terminal_prefix("/usr/bin/gnome-terminal"), ["/usr/bin/gnome-terminal", "--"]);
        assert_eq!(terminal_prefix("ghostty"), ["ghostty", "-e"]);
        assert_eq!(terminal_prefix("myterm"), ["myterm", "-e"], "unknown: -e");
        assert_eq!(terminal_prefix("kitty --single-instance -e"), ["kitty", "--single-instance", "-e"], "a full command is used as written");
        let opts = mandara_config::workflow::LaunchOptions { terminal: "wezterm".into(), ..Default::default() };
        assert_eq!(launch_argv("htop", true, &opts), ["wezterm", "start", "--", "htop"]);
    }

    #[test]
    fn localized_names_win_for_their_language() {
        let map = parse_desktop_entry("[Desktop Entry]\nName=Mandara Studio\nName[ar]=ستوديو المندرة\nComment=Settings\n").unwrap();
        assert_eq!(localized(&map, "Name", "ar").map(String::as_str), Some("ستوديو المندرة"));
        assert_eq!(localized(&map, "Name", "de").map(String::as_str), Some("Mandara Studio"));
        assert_eq!(localized(&map, "Comment", "ar").map(String::as_str), Some("Settings"));
    }

    #[test]
    fn parses_desktop_entries() {
        let text = "[Desktop Entry]\nName=Zed\nExec=zed %U\nIcon=zed\nKeywords=editor;code;\n[Desktop Action new]\nName=New Window\n";
        let map = parse_desktop_entry(text).unwrap();
        assert_eq!(map["Name"], "Zed");
        assert_eq!(strip_field_codes(&map["Exec"]), "zed");
        assert_eq!(strip_field_codes("echo 100%% %f"), "echo 100%");
    }

    #[test]
    fn desktop_actions_follow_the_actions_key() {
        let text = "[Desktop Entry]\nName=Fox\nExec=fox %u\nActions=new-window;private;broken;\n[Desktop Action private]\nName=New Private Window\nExec=fox --private\n[Desktop Action new-window]\nName=New Window\nExec=fox --new\n[Desktop Action broken]\nName=Nope\n";
        let actions = parse_desktop_actions(text);
        assert_eq!(actions.iter().map(|a| a.name.as_str()).collect::<Vec<_>>(), ["New Window", "New Private Window"]);
        assert_eq!(actions[1].exec, "fox --private");
        assert!(parse_desktop_actions("[Desktop Entry]\nName=x\n").is_empty());
    }

    #[test]
    fn launch_argv_applies_terminal_and_wrapper() {
        let mut o = mandara_config::workflow::LaunchOptions { terminal: "foot".into(), ..Default::default() };
        assert_eq!(launch_argv("htop", true, &o), ["foot", "-e", "htop"]);
        o.wrapper = "uwsm app --".into();
        assert_eq!(launch_argv("zed %U", false, &o), ["uwsm", "app", "--", "zed"]);
        o.terminal = "wezterm start --".into();
        assert_eq!(launch_argv("btop", true, &o), ["uwsm", "app", "--", "wezterm", "start", "--", "btop"]);
    }

    #[test]
    fn origins_from_paths() {
        use std::path::Path;
        assert_eq!(AppOrigin::of(Path::new("/var/lib/flatpak/exports/share/applications/x.desktop"), "x"), AppOrigin::Flatpak);
        assert_eq!(AppOrigin::of(Path::new("/var/lib/snapd/desktop/applications/x.desktop"), "x"), AppOrigin::Snap);
        assert_eq!(AppOrigin::of(Path::new("/usr/share/applications/x.desktop"), "/opt/X.AppImage"), AppOrigin::AppImage);
        assert_eq!(AppOrigin::of(Path::new("/usr/share/applications/x.desktop"), "x"), AppOrigin::System);
        assert_eq!(AppOrigin::System.badge(), None);
    }

    #[test]
    fn exec_parsing_follows_the_spec() {
        assert_eq!(parse_exec("zed %U"), vec!["zed"]);
        assert_eq!(parse_exec(r#"sh -c "echo \"hi\" \$HOME""#), vec!["sh", "-c", r#"echo "hi" $HOME"#]);
        assert_eq!(parse_exec("app --rate 100%% %f"), vec!["app", "--rate", "100%"]);
        assert_eq!(parse_exec(r#""/opt/My App/bin" --x"#), vec!["/opt/My App/bin", "--x"]);
        assert!(parse_exec("   ").is_empty());
    }

    #[test]
    fn calculator() {
        assert_eq!(calculate("2 + 3 * 4"), Some(14.0));
        assert_eq!(calculate("(2 + 3) * 4"), Some(20.0));
        assert_eq!(calculate("2 ^ 3 ^ 2"), Some(512.0));
        assert_eq!(calculate("-3 + 5"), Some(2.0));
        assert_eq!(calculate("10 / 4"), Some(2.5));
        assert_eq!(calculate("1 / 0"), None);
        assert_eq!(calculate("2 +"), None);
        assert_eq!(calculate("hello"), None);
        assert_eq!(calculate("-2^2"), Some(-4.0));
        assert_eq!(calculate("2^-1"), Some(0.5));
        assert_eq!(calculate("10 % 3"), Some(1.0));
        assert_eq!(calculate("50%"), Some(0.5));
        assert_eq!(calculate("200 * 15%"), Some(30.0));
        assert_eq!(calculate("(1+2)%"), Some(0.03));
        assert_eq!(calculate("5!"), Some(120.0));
        assert_eq!(calculate("sqrt(16)^2"), Some(16.0));
        assert_eq!(calculate("sqrt 16 + 1"), Some(5.0));
        assert_eq!(calculate("2 * pi").map(|v| (v * 1e6).round()), Some((std::f64::consts::TAU * 1e6).round()));
        assert_eq!(calculate("1.5e3 + 1"), Some(1501.0));
        assert_eq!(calculate("e").map(|v| (v * 1e3).round()), Some(2718.0));
        assert_eq!(calculate("3 × 4 ÷ 2"), Some(6.0));
        assert_eq!(calculate("2 ** 3"), Some(8.0));
        assert_eq!(calculate("2x3"), Some(6.0));
        assert_eq!(calculate("log(1000)"), Some(3.0));
        assert_eq!(calculate("((2)"), None);
        assert_eq!(calculate("sqrt(-1)"), None);
        assert_eq!(calculate("foo(2)"), None);
    }

    #[test]
    fn discovers_installed_apps() {
        let apps = discover();
        for a in &apps {
            let argv = parse_exec(&a.exec);
            assert!(!a.name.is_empty());
            assert!(argv.iter().all(|w| !(w.len() == 2 && w.starts_with('%'))), "{}: {:?}", a.name, argv);
        }
    }
}
