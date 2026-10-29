//! Studio → Maintenance: health checks with one-click repairs, downloads
//! (models, fonts), configuration backups and factory reset. Everything
//! here is blocking and runs on background threads; shell-side work
//! (re-syncing the compositor drop-in, re-exporting app themes, reloading
//! config) goes through the shell's IPC.

use std::path::{Path, PathBuf};

use mandara_config::compositor::{Target, include_installed, install_include};
use mandara_config::paths;
use mandara_i18n::{t, tf, tk};

/// Result of one check.
#[derive(Clone, Debug, PartialEq)]
pub enum Health {
    Ok(String),
    /// Works, but something is missing or could be better.
    Warn(String),
    Broken(String),
}

impl Health {
    pub fn is_ok(&self) -> bool {
        matches!(self, Health::Ok(_))
    }

    pub fn detail(&self) -> &str {
        match self {
            Health::Ok(s) | Health::Warn(s) | Health::Broken(s) => s,
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum CheckId {
    Config,
    CompositorInclude,
    CompositorDropIn,
    CompositorConflicts,
    AppThemes,
    Fonts,
    PluginDirs,
}

impl CheckId {
    pub const ALL: [CheckId; 7] = [CheckId::Config, CheckId::CompositorInclude, CheckId::CompositorDropIn, CheckId::CompositorConflicts, CheckId::AppThemes, CheckId::Fonts, CheckId::PluginDirs];

    /// English; display with `tr()`.
    pub fn title(self) -> &'static str {
        match self {
            CheckId::Config => tk("Configuration file"),
            CheckId::CompositorInclude => tk("Compositor include line"),
            CheckId::CompositorDropIn => tk("Compositor drop-in"),
            CheckId::CompositorConflicts => tk("Compositor conflicts"),
            CheckId::AppThemes => tk("App theme files"),
            CheckId::Fonts => tk("Mandara fonts"),
            CheckId::PluginDirs => tk("Plugin folders"),
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            CheckId::Config => "note",
            CheckId::CompositorInclude | CheckId::CompositorDropIn | CheckId::CompositorConflicts => "monitor",
            CheckId::AppThemes => "sun",
            CheckId::Fonts => "keyboard",
            CheckId::PluginDirs => "grid",
        }
    }

    /// English; display with `tr()`.
    pub fn fix_label(self) -> &'static str {
        match self {
            CheckId::Config => tk("Repair"),
            CheckId::CompositorInclude => tk("Add include"),
            CheckId::CompositorDropIn => tk("Regenerate"),
            CheckId::CompositorConflicts => tk("Retire"),
            CheckId::AppThemes => tk("Re-export"),
            CheckId::Fonts => tk("Download"),
            CheckId::PluginDirs => tk("Create"),
        }
    }
}

fn target() -> Option<Target> {
    match mandara_compositor::detect()? {
        mandara_compositor::CompositorKind::MangoWM => Some(Target::Mango),
        mandara_compositor::CompositorKind::Niri => Some(Target::Niri),
        mandara_compositor::CompositorKind::Hyprland => Some(Target::Hyprland),
    }
}

/// A font Mandara's design uses, downloadable from Google Fonts (OFL).
pub struct Font {
    pub family: &'static str,
    pub file: &'static str,
    pub url: &'static str,
    pub size: u64,
}

/// Largest font file accepted from the download (16 MiB).
const FONT_MAX_BYTES: u64 = 16 << 20;
/// Whole-request timeout of a font download.
const DOWNLOAD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

pub const FONTS: &[Font] = &[
    Font { family: "Inter", file: "Inter[opsz,wght].ttf", url: "https://raw.githubusercontent.com/google/fonts/main/ofl/inter/Inter%5Bopsz,wght%5D.ttf", size: 876_576 },
    Font { family: "Cairo", file: "Cairo[slnt,wght].ttf", url: "https://raw.githubusercontent.com/google/fonts/main/ofl/cairo/Cairo%5Bslnt,wght%5D.ttf", size: 599_548 },
    Font { family: "Alexandria", file: "Alexandria[wght].ttf", url: "https://raw.githubusercontent.com/google/fonts/main/ofl/alexandria/Alexandria%5Bwght%5D.ttf", size: 332_488 },
    Font { family: "JetBrains Mono", file: "JetBrainsMono[wght].ttf", url: "https://raw.githubusercontent.com/google/fonts/main/ofl/jetbrainsmono/JetBrainsMono%5Bwght%5D.ttf", size: 187_208 },
];

pub fn fonts_dir() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| PathBuf::from("~/.local/share")).join("fonts").join("mandara")
}

/// Families not installed, given the system's font family names.
pub fn missing_fonts(installed: &[String]) -> Vec<&'static Font> {
    FONTS.iter().filter(|f| !installed.iter().any(|n| n.eq_ignore_ascii_case(f.family)) && !fonts_dir().join(f.file).exists()).collect()
}

/// The folders Maintenance works on: the user's XDG folders, or a
/// sandbox in tests.
#[derive(Clone, Debug)]
pub struct Roots {
    /// `~/.config/mandara`
    pub config: PathBuf,
    /// `~/.local/share/mandara`
    pub data: PathBuf,
    /// `~/.cache/mandara`
    pub cache: PathBuf,
}

impl Roots {
    pub fn user() -> Self {
        Self { config: paths::config_dir(), data: paths::data_dir(), cache: paths::cache_dir() }
    }

    fn config_file(&self) -> PathBuf {
        self.config.join("config.toml")
    }

    fn backups(&self) -> PathBuf {
        self.data.join("backups")
    }

    fn themes(&self) -> PathBuf {
        self.cache.join("themes")
    }

    fn plugin_dirs(&self) -> [PathBuf; 2] {
        [self.data.join("plugins"), self.data.join("plugins-dev")]
    }
}

/// Runs one check. `installed_fonts` comes from the text system.
pub fn check(id: CheckId, installed_fonts: &[String]) -> Health {
    check_in(&Roots::user(), id, installed_fonts)
}

/// [`check`] against `roots`.
pub fn check_in(roots: &Roots, id: CheckId, installed_fonts: &[String]) -> Health {
    match id {
        CheckId::Config => {
            let path = roots.config_file();
            match std::fs::read_to_string(&path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => Health::Warn(t("No config file yet — defaults are in use").into()),
                Err(e) => Health::Broken(format!("{}: {e}", path.display())),
                Ok(text) => match toml::from_str::<mandara_config::ShellConfig>(&text) {
                    Ok(_) => Health::Ok(path.display().to_string()),
                    Err(e) => Health::Broken(tf("Doesn't parse: {error}", &[("error", &e.message())])),
                },
            }
        }
        CheckId::CompositorInclude => match target() {
            None => Health::Warn(t("No supported compositor detected (MangoWM, Niri, Hyprland)").into()),
            Some(c) if include_installed(c) => Health::Ok(tf("{file} sources Mandara's drop-in", &[("file", &c.user_config().display())])),
            Some(c) => Health::Broken(tf("{file} doesn't include {drop_in}", &[("file", &c.user_config().display()), ("drop_in", &c.generated_file().display())])),
        },
        CheckId::CompositorDropIn => match target() {
            None => Health::Warn(t("No supported compositor detected").into()),
            Some(c) if c.generated_file().exists() => Health::Ok(c.generated_file().display().to_string()),
            Some(c) => Health::Broken(tf("{file} is missing", &[("file", &c.generated_file().display())])),
        },
        CheckId::CompositorConflicts => match target() {
            None => Health::Warn(t("No supported compositor detected").into()),
            Some(c) => {
                let cfg = mandara_config::ConfigStore::open_default().map(|s| s.snapshot()).unwrap_or_default();
                let found: Vec<_> = mandara_config::conflicts::scan(c, &cfg.compositor).into_iter().filter(|x| x.is_conflict()).collect();
                match found.as_slice() {
                    [] => Health::Ok(t("Nothing in your compositor config fights Mandara").into()),
                    [one] => Health::Warn(one.summary()),
                    many => Health::Warn(tf("{n} conflicts, e.g. {first}", &[("n", &many.len()), ("first", &many[0].summary())])),
                }
            }
        },
        CheckId::AppThemes => {
            let dir = roots.themes();
            let n = std::fs::read_dir(&dir).map(|d| d.count()).unwrap_or(0);
            if n > 0 { Health::Ok(tf("{n} theme files in {folder}", &[("n", &n), ("folder", &dir.display())])) } else { Health::Warn(t("Not exported yet — Ghostty, GTK, Qt, Zed and btop follow Mandara once exported").into()) }
        }
        CheckId::Fonts => {
            let missing = missing_fonts(installed_fonts);
            if missing.is_empty() {
                Health::Ok(t("Inter, Cairo, Alexandria and JetBrains Mono are installed").into())
            } else {
                Health::Warn(tf("Missing {fonts} — fallbacks in use", &[("fonts", &missing.iter().map(|f| f.family).collect::<Vec<_>>().join(", "))]))
            }
        }
        CheckId::PluginDirs => {
            let dirs = roots.plugin_dirs();
            let missing: Vec<_> = dirs.iter().filter(|d| !d.exists()).collect();
            if missing.is_empty() {
                Health::Ok(tf("{first} and {second}", &[("first", &dirs[0].display()), ("second", &dirs[1].display())]))
            } else {
                Health::Warn(tf("{folders} missing", &[("folders", &missing.iter().map(|d| d.display().to_string()).collect::<Vec<_>>().join(", "))]))
            }
        }
    }
}

/// Shell commands to run after a fix (through IPC).
pub type Followups = Vec<Vec<String>>;

fn cmd(words: &[&str]) -> Vec<String> {
    words.iter().map(|s| s.to_string()).collect()
}

/// Applies the fix for `id`. Returns a message and shell commands to run.
pub fn fix(id: CheckId, installed_fonts: &[String], progress: &dyn Fn(u64, u64)) -> Result<(String, Followups), String> {
    fix_in(&Roots::user(), id, installed_fonts, progress)
}

/// [`fix`] against `roots`.
pub fn fix_in(roots: &Roots, id: CheckId, installed_fonts: &[String], progress: &dyn Fn(u64, u64)) -> Result<(String, Followups), String> {
    match id {
        CheckId::Config => {
            let backup = backup_in(roots, "before-repair")?;
            // Keep whatever still parses, value by value; defaults for the rest.
            let text = std::fs::read_to_string(roots.config_file()).unwrap_or_default();
            let (cfg, dropped) = mandara_config::profiles::salvage(&text);
            write_config_in(roots, &cfg)?;
            let message = if dropped.is_empty() {
                tf("Config rewritten (old copy in {folder})", &[("folder", &backup.display())])
            } else {
                tf("Config rewritten; reset to defaults: {settings} (old copy in {folder})", &[("settings", &dropped.join(", ")), ("folder", &backup.display())])
            };
            Ok((message, vec![cmd(&["config", "reload"])]))
        }
        CheckId::CompositorInclude => {
            let c = target().ok_or(t("no supported compositor detected"))?;
            install_include(c).map_err(|e| format!("{e:#}"))?;
            Ok((tf("Include added to {file} (backup kept)", &[("file", &c.user_config().display())]), vec![cmd(&["compositor", "sync"])]))
        }
        CheckId::CompositorDropIn => Ok((t("Regenerating the drop-in").into(), vec![cmd(&["compositor", "sync"])])),
        CheckId::CompositorConflicts => {
            use mandara_config::conflicts::{self, Validation};
            let t_ = target().ok_or_else(|| t("No supported compositor detected").to_string())?;
            let mut store = mandara_config::ConfigStore::open_default().map_err(|e| e.to_string())?;
            let found: Vec<_> = conflicts::scan(t_, &store.get().compositor).into_iter().filter(|x| x.is_conflict()).collect();
            if found.is_empty() {
                return Ok((t("Nothing to retire").into(), Vec::new()));
            }
            let backup = conflicts::retire(t_, &found).map_err(|e| format!("{e:#}"))?;
            if let Validation::Invalid { tool, report } = conflicts::validate(t_) {
                conflicts::restore(&backup).map_err(|e| format!("{e:#}"))?;
                return Err(tf("{tool} rejected the result, so nothing was changed: {report}", &[("tool", &tool), ("report", &report)]));
            }
            let takeovers = conflicts::takeover_keybinds(&found);
            if !takeovers.is_empty() {
                store.update(|c| {
                    for k in takeovers {
                        if !c.compositor.keybinds.iter().any(|x| x.chord() == k.chord()) {
                            c.compositor.keybinds.push(k);
                        }
                    }
                });
                store.flush().map_err(|e| format!("{e:#}"))?;
            }
            Ok((
                tf("Retired {n} line(s) — backup in {dir} (undo: mandara setup --restore latest)", &[("n", &found.len()), ("dir", &backup.display())]),
                vec![cmd(&["config", "reload"]), cmd(&["compositor", "sync"])],
            ))
        }
        CheckId::AppThemes => Ok((t("Exporting app themes").into(), vec![cmd(&["theme", "export"])])),
        CheckId::Fonts => {
            let missing = missing_fonts(installed_fonts);
            let total: u64 = missing.iter().map(|f| f.size).sum();
            let dir = fonts_dir();
            std::fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
            let mut done = 0;
            for f in &missing {
                let bytes = download(f.url, FONT_MAX_BYTES)?;
                let path = dir.join(f.file);
                mandara_config::store::write_atomic(&path, &bytes).map_err(|e| format!("{}: {e:#}", path.display()))?;
                done += f.size;
                progress(done, total);
            }
            Ok((tf("Installed {fonts} into {folder} — restart the shell to use them", &[("fonts", &missing.iter().map(|f| f.family).collect::<Vec<_>>().join(", ")), ("folder", &dir.display())]), Vec::new()))
        }
        CheckId::PluginDirs => {
            for d in roots.plugin_dirs() {
                std::fs::create_dir_all(&d).map_err(|e| format!("{}: {e}", d.display()))?;
            }
            Ok((t("Plugin folders created").into(), Vec::new()))
        }
    }
}

fn download(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    use std::io::Read;
    mandara_services::offline::check()?;
    let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(DOWNLOAD_TIMEOUT)).build().into();
    let mut response = agent.get(url).header("User-Agent", concat!("mandara-studio/", env!("CARGO_PKG_VERSION"))).call().map_err(|e| tf("download failed: {error}", &[("error", &e)]))?;
    let mut out = Vec::new();
    response.body_mut().with_config().limit(limit).reader().read_to_end(&mut out).map_err(|e| tf("download failed: {error}", &[("error", &e)]))?;
    Ok(out)
}

fn write_config_in(roots: &Roots, cfg: &mandara_config::ShellConfig) -> Result<(), String> {
    let text = toml::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    let path = roots.config_file();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    mandara_config::store::write_atomic(&path, text.as_bytes()).map_err(|e| format!("{e:#}"))
}

// ── Backups & factory reset ───────────────────────────────────────────────

pub fn backups_dir() -> PathBuf {
    Roots::user().backups()
}

fn copy_dir(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)? {
        let entry = entry?;
        let dest = to.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &dest)?;
        } else {
            std::fs::copy(entry.path(), dest)?;
        }
    }
    Ok(())
}

/// Copies `~/.config/mandara` into a timestamped backup folder.
pub fn backup_now(reason: &str) -> Result<PathBuf, String> {
    backup_in(&Roots::user(), reason)
}

/// [`backup_now`] against `roots`. Two backups in the same second get
/// distinct folders.
pub fn backup_in(roots: &Roots, reason: &str) -> Result<PathBuf, String> {
    let stem = format!("{}-{reason}", chrono::Local::now().format("%Y%m%d-%H%M%S"));
    let mut dest = roots.backups().join(&stem);
    let mut n = 1;
    while dest.exists() {
        n += 1;
        dest = roots.backups().join(format!("{stem}-{n}"));
    }
    let src = &roots.config;
    if src.exists() {
        copy_dir(&src, &dest).map_err(|e| tf("backup to {folder}: {error}", &[("folder", &dest.display()), ("error", &e)]))?;
    } else {
        std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    }
    Ok(dest)
}

/// Backups, newest first.
pub fn backups() -> Vec<PathBuf> {
    backups_in(&Roots::user())
}

/// [`backups`] in `roots`.
pub fn backups_in(roots: &Roots) -> Vec<PathBuf> {
    let mut list: Vec<PathBuf> = std::fs::read_dir(roots.backups()).map(|d| d.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default();
    list.sort();
    list.reverse();
    list
}

/// Backs up, then replaces the config with defaults. Compositor dotfiles
/// keep their single include line; the drop-in is regenerated.
pub fn factory_reset() -> Result<(PathBuf, Followups), String> {
    factory_reset_in(&Roots::user())
}

/// [`factory_reset`] against `roots`.
pub fn factory_reset_in(roots: &Roots) -> Result<(PathBuf, Followups), String> {
    let backup = backup_in(roots, "factory-reset")?;
    write_config_in(roots, &mandara_config::ShellConfig::default())?;
    Ok((backup, vec![cmd(&["config", "reload"]), cmd(&["compositor", "sync"]), cmd(&["theme", "export"])]))
}

/// Restores `config.toml` from a backup (after backing up the current one).
pub fn restore(backup: &Path) -> Result<Followups, String> {
    restore_in(&Roots::user(), backup)
}

/// [`restore`] against `roots`.
pub fn restore_in(roots: &Roots, backup: &Path) -> Result<Followups, String> {
    let file = backup.join("config.toml");
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let cfg: mandara_config::ShellConfig = toml::from_str(&text).map_err(|e| tf("that backup doesn't parse: {error}", &[("error", &e.message())]))?;
    backup_in(roots, "before-restore")?;
    write_config_in(roots, &cfg)?;
    Ok(vec![cmd(&["config", "reload"]), cmd(&["compositor", "sync"])])
}

/// Total size of Mandara's caches (thumbnails, Wallhaven previews, …).
pub fn cache_size() -> u64 {
    cache_size_in(&Roots::user())
}

/// [`cache_size`] in `roots`.
pub fn cache_size_in(roots: &Roots) -> u64 {
    fn walk(p: &Path) -> u64 {
        std::fs::read_dir(p)
            .map(|d| d.flatten().map(|e| match e.metadata() {
                Ok(m) if m.is_dir() => walk(&e.path()),
                Ok(m) => m.len(),
                Err(_) => 0,
            }).sum())
            .unwrap_or(0)
    }
    walk(&roots.cache)
}

/// Clears caches that regenerate on demand (keeps exported app themes).
pub fn clear_caches() -> Result<u64, String> {
    clear_caches_in(&Roots::user())
}

/// [`clear_caches`] in `roots`.
pub fn clear_caches_in(roots: &Roots) -> Result<u64, String> {
    let before = cache_size_in(roots);
    let root = roots.cache.clone();
    let themes = roots.themes();
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        // No cache yet: nothing to clear.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(format!("{}: {e}", root.display())),
    };
    for entry in entries.flatten() {
        let p = entry.path();
        if p == themes {
            continue;
        }
        let r = if p.is_dir() { std::fs::remove_dir_all(&p) } else { std::fs::remove_file(&p) };
        r.map_err(|e| format!("{}: {e}", p.display()))?;
    }
    Ok(before.saturating_sub(cache_size_in(roots)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fonts_have_https_urls_and_sizes() {
        assert!(FONTS.iter().all(|f| f.url.starts_with("https://") && f.size > 0));
        assert!(missing_fonts(&FONTS.iter().map(|f| f.family.to_string()).collect::<Vec<_>>()).is_empty());
    }

    /// A sandbox: nothing here touches the user's own folders.
    fn sandbox() -> (tempfile::TempDir, Roots) {
        let dir = tempfile::tempdir().unwrap();
        let roots = Roots { config: dir.path().join("config/mandara"), data: dir.path().join("share/mandara"), cache: dir.path().join("cache/mandara") };
        (dir, roots)
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn no_progress(_: u64, _: u64) {}

    #[test]
    fn config_check_and_salvaging_repair() {
        let (_dir, roots) = sandbox();
        assert!(matches!(check_in(&roots, CheckId::Config, &[]), Health::Warn(_)), "no file yet");
        write(&roots.config_file(), "[bar]\nheight = 31.0\nlayout = \"sideways\"\n[general]\nclock_24h = false\n");
        assert!(matches!(check_in(&roots, CheckId::Config, &[]), Health::Broken(_)));
        let (message, followups) = fix_in(&roots, CheckId::Config, &[], &no_progress).unwrap();
        assert!(message.contains("bar.layout"), "{message}");
        assert_eq!(followups, vec![cmd(&["config", "reload"])]);
        assert!(check_in(&roots, CheckId::Config, &[]).is_ok());
        let fixed: mandara_config::ShellConfig = toml::from_str(&std::fs::read_to_string(roots.config_file()).unwrap()).unwrap();
        assert_eq!(fixed.bar.height, 31.0, "good values survive the repair");
        assert!(!fixed.general.clock_24h);
        // The broken original is kept in a backup.
        let backup = backups_in(&roots).into_iter().next().unwrap();
        assert!(std::fs::read_to_string(backup.join("config.toml")).unwrap().contains("sideways"));
    }

    #[test]
    fn plugin_dirs_and_app_themes_checks() {
        let (_dir, roots) = sandbox();
        assert!(matches!(check_in(&roots, CheckId::PluginDirs, &[]), Health::Warn(_)));
        fix_in(&roots, CheckId::PluginDirs, &[], &no_progress).unwrap();
        assert!(check_in(&roots, CheckId::PluginDirs, &[]).is_ok());
        assert!(roots.data.join("plugins").is_dir() && roots.data.join("plugins-dev").is_dir());
        assert!(matches!(check_in(&roots, CheckId::AppThemes, &[]), Health::Warn(_)));
        write(&roots.themes().join("ghostty"), "x");
        assert!(check_in(&roots, CheckId::AppThemes, &[]).is_ok());
        let (_, followups) = fix_in(&roots, CheckId::AppThemes, &[], &no_progress).unwrap();
        assert_eq!(followups, vec![cmd(&["theme", "export"])], "the shell re-exports");
        let (_, followups) = fix_in(&roots, CheckId::CompositorDropIn, &[], &no_progress).unwrap();
        assert_eq!(followups, vec![cmd(&["compositor", "sync"])], "the shell regenerates the drop-in");
    }

    #[test]
    fn fonts_check_follows_the_installed_families() {
        let (_dir, roots) = sandbox();
        let all: Vec<String> = FONTS.iter().map(|f| f.family.to_string()).collect();
        assert!(check_in(&roots, CheckId::Fonts, &all).is_ok());
        let some: Vec<String> = all.iter().skip(1).cloned().collect();
        match check_in(&roots, CheckId::Fonts, &some) {
            Health::Warn(detail) => assert!(detail.contains(FONTS[0].family) || !fonts_dir().join(FONTS[0].file).exists(), "{detail}"),
            Health::Ok(_) => assert!(fonts_dir().join(FONTS[0].file).exists(), "only an installed file makes a missing family OK"),
            Health::Broken(d) => panic!("{d}"),
        }
    }

    #[test]
    fn backups_restore_and_factory_reset() {
        let (_dir, roots) = sandbox();
        let mut custom = mandara_config::ShellConfig::default();
        custom.bar.height = 29.0;
        write(&roots.config_file(), &toml::to_string(&custom).unwrap());
        write(&roots.config.join("greeter.toml"), "language = \"ar\"\n");
        let (backup, followups) = factory_reset_in(&roots).unwrap();
        assert!(followups.contains(&cmd(&["config", "reload"])) && followups.contains(&cmd(&["compositor", "sync"])));
        let now: mandara_config::ShellConfig = toml::from_str(&std::fs::read_to_string(roots.config_file()).unwrap()).unwrap();
        assert_eq!(now, mandara_config::ShellConfig::default(), "factory reset writes defaults");
        assert!(backup.join("greeter.toml").is_file(), "the whole folder is backed up");
        // Restore brings the old config back and backs up the reset one.
        restore_in(&roots, &backup).unwrap();
        let back: mandara_config::ShellConfig = toml::from_str(&std::fs::read_to_string(roots.config_file()).unwrap()).unwrap();
        assert_eq!(back.bar.height, 29.0);
        let list = backups_in(&roots);
        assert_eq!(list.len(), 2, "{list:?}");
        assert!(list.iter().any(|b| b.to_string_lossy().ends_with("before-restore")));
        // A backup that doesn't parse is refused and changes nothing.
        write(&roots.backups().join("bad").join("config.toml"), "[bar]\nheight = \"tall\"\n");
        assert!(restore_in(&roots, &roots.backups().join("bad")).is_err());
        let still: mandara_config::ShellConfig = toml::from_str(&std::fs::read_to_string(roots.config_file()).unwrap()).unwrap();
        assert_eq!(still.bar.height, 29.0);
    }

    #[test]
    fn clearing_caches_keeps_app_themes() {
        let (_dir, roots) = sandbox();
        assert_eq!(clear_caches_in(&roots).unwrap(), 0, "a missing cache is nothing to clear");
        write(&roots.cache.join("wallhaven/a.jpg"), &"x".repeat(1000));
        write(&roots.cache.join("thumbs.bin"), &"y".repeat(500));
        write(&roots.themes().join("gtk.css"), "z");
        assert_eq!(cache_size_in(&roots), 1501);
        assert_eq!(clear_caches_in(&roots).unwrap(), 1500);
        assert!(roots.themes().join("gtk.css").is_file());
        assert!(!roots.cache.join("wallhaven").exists());
    }

    #[test]
    fn every_check_has_labels() {
        for id in CheckId::ALL {
            assert!(!id.title().is_empty() && !id.fix_label().is_empty());
        }
    }
}
