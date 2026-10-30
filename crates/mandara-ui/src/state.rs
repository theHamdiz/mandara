//! Shell-wide shared state and the config/theme plumbing every surface uses.

use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use mandara_config::{ConfigStore, ReloadOutcome, ShellConfig};
use mandara_theme::{ThemeEngine, ThemeSource};

/// Live UI state shared by every surface (and driven by IPC commands).
#[derive(Default)]
pub struct ShellState {
    /// Plugin id whose panel is open (`None` = collapsed).
    pub panel: Option<String>,
    /// Horizontal anchor (surface x) for panels opened at the pointer, like
    /// the main menu.
    pub panel_anchor: Option<f32>,
    /// Sidebar explicitly opened (command / click).
    pub sidebar_open: bool,
    /// Sidebar revealed by pointer hover at the left edge.
    pub sidebar_hover: bool,
    pub launcher_open: bool,
    pub session_open: bool,
    pub wallpaper_open: bool,
    flush_task: Option<Task<()>>,
}

impl ShellState {
    pub fn toggle_panel(&mut self, id: &str, cx: &mut Context<Self>) {
        self.panel = if self.panel.as_deref() == Some(id) { None } else { Some(id.to_string()) };
        cx.notify();
    }

    pub fn set_panel(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.panel != id {
            self.panel = id;
            cx.notify();
        }
    }

    /// Open by command, hover, or pin (pin lives in config).
    pub fn sidebar_visible(&self) -> bool {
        self.sidebar_open || self.sidebar_hover
    }

    pub fn set_sidebar(&mut self, open: bool, cx: &mut Context<Self>) {
        self.sidebar_open = open;
        if !open {
            self.sidebar_hover = false;
        }
        cx.notify();
    }

    pub fn set_sidebar_hover(&mut self, hover: bool, cx: &mut Context<Self>) {
        if self.sidebar_hover != hover {
            self.sidebar_hover = hover;
            cx.notify();
        }
    }

    pub fn set_launcher(&mut self, open: bool, cx: &mut Context<Self>) {
        if open && !config(cx).read(cx).get().launcher.enabled {
            return;
        }
        if self.launcher_open != open {
            self.launcher_open = open;
            if open {
                self.session_open = false;
                self.wallpaper_open = false;
                self.panel = None;
            }
            cx.notify();
        }
    }

    pub fn set_session(&mut self, open: bool, cx: &mut Context<Self>) {
        if self.session_open != open {
            self.session_open = open;
            if open {
                self.launcher_open = false;
                self.wallpaper_open = false;
                self.panel = None;
            }
            cx.notify();
        }
    }

    pub fn set_wallpaper_picker(&mut self, open: bool, cx: &mut Context<Self>) {
        if open && !config(cx).read(cx).get().wallpaper.picker {
            return;
        }
        if self.wallpaper_open != open {
            self.wallpaper_open = open;
            if open {
                self.launcher_open = false;
                self.session_open = false;
                self.panel = None;
            }
            cx.notify();
        }
    }
}

/// Opens Mandara Studio (installed by the app, which owns the Studio crate).
#[derive(Clone)]
pub struct StudioHook(pub std::rc::Rc<dyn Fn(Option<String>, &mut App)>);
impl Global for StudioHook {}

pub fn open_studio(cx: &mut App) {
    open_studio_at(None, cx);
}

/// Opens Studio on a named section (e.g. `themes`, `bar`).
pub fn open_studio_at(section: Option<String>, cx: &mut App) {
    if let Some(hook) = cx.try_global::<StudioHook>().cloned() {
        (hook.0)(section, cx);
    } else {
        tracing::warn!("Mandara Studio is not available in this build");
    }
}

/// Forwards every config change to another process (Studio → shell).
/// Returns `true` when delivered, in which case that process persists it.
#[derive(Clone)]
pub struct ConfigMirror(pub std::rc::Rc<dyn Fn(&ShellConfig) -> bool>);
impl Global for ConfigMirror {}

#[derive(Clone)]
pub struct GlobalShell(pub Entity<ShellState>);
impl Global for GlobalShell {}

#[derive(Clone)]
pub struct GlobalConfig(pub Entity<ConfigStore>);
impl Global for GlobalConfig {}

pub fn shell(cx: &App) -> Entity<ShellState> {
    cx.global::<GlobalShell>().0.clone()
}

pub fn config(cx: &App) -> Entity<ConfigStore> {
    cx.global::<GlobalConfig>().0.clone()
}

/// Debounce for persisting in-memory config edits to disk.
const FLUSH_DEBOUNCE: Duration = Duration::from_millis(400);

/// Mutates the shared config in memory (instantly visible to every surface)
/// and schedules a debounced atomic flush to disk. Theme-affecting changes
/// are forwarded to the theme engine immediately.
pub fn update_config(cx: &mut App, f: impl FnOnce(&mut ShellConfig)) {
    let config = config(cx);
    let changed = config.update(cx, |store, cx| {
        let changed = store.update(f);
        if changed {
            cx.notify();
        }
        changed
    });
    if !changed {
        return;
    }
    sync_theme_from_config(cx);
    if let Some(mirror) = cx.try_global::<ConfigMirror>().cloned()
        && (mirror.0)(&config.read(cx).snapshot())
    {
        return;
    }
    let shell = shell(cx);
    shell.update(cx, |state, cx| {
        state.flush_task = Some(cx.spawn(async move |_, cx| {
            cx.background_executor().timer(FLUSH_DEBOUNCE).await;
            config.update(cx, |store, _| {
                if let Err(err) = store.flush() {
                    tracing::error!("failed to persist config: {err:#}");
                }
            });
        }));
    });
}

/// Re-reads the config file after an external edit and applies it.
pub fn reload_config_from_disk(cx: &mut App) -> Result<bool, String> {
    let config = config(cx);
    let outcome = config.update(cx, |store, cx| {
        let outcome = store.reload_from_disk();
        if matches!(outcome, Ok(ReloadOutcome::Changed)) {
            cx.notify();
        }
        outcome
    });
    match outcome {
        Ok(ReloadOutcome::Changed) => {
            tracing::info!("config reloaded from disk");
            sync_theme_from_config(cx);
            Ok(true)
        }
        Ok(ReloadOutcome::Unchanged) => Ok(false),
        Err(err) => {
            tracing::warn!("ignoring invalid config edit: {err:#}");
            Err(format!("{err:#}"))
        }
    }
}

/// How long a palette change crossfades: `theme.transition_ms`, or
/// instantly with reduced motion.
pub fn theme_transition(cfg: &ShellConfig) -> Duration {
    if cfg.motion.reduce_motion { Duration::ZERO } else { Duration::from_millis(cfg.theme.transition_ms) }
}

/// Pushes the config's theme section into the theme engine, crossfading in
/// OKLCH. Wallpaper sources are decoded on the background executor.
pub fn sync_theme_from_config(cx: &mut App) {
    let cfg = config(cx).read(cx).snapshot();
    let theme: Entity<ThemeEngine> = mandara_core::theme_entity(cx);
    let now = Instant::now();
    let dark = cfg.theme.mode.is_dark();

    theme.update(cx, |engine, cx| {
        engine.set_duration(theme_transition(&cfg));
        if *engine.adjustments() != cfg.theme.adjustments {
            // Live slider edits track the pointer without tween lag.
            engine.set_adjustments(cfg.theme.adjustments, false, now);
            cx.notify();
        }
    });

    let source = cfg.theme.source.clone();
    if *theme.read(cx).source() == source && theme.read(cx).is_dark() == dark {
        return;
    }
    match &source {
        ThemeSource::Wallpaper { .. } => {
            let task = cx.background_executor().spawn({
                let source = source.clone();
                async move { source.resolve(dark) }
            });
            cx.spawn(async move |cx| match task.await {
                Ok(resolved) => {
                    theme.update(cx, |engine, cx| {
                        engine.apply_resolved(source, dark, resolved, Instant::now());
                        cx.notify();
                    });
                }
                Err(err) => tracing::error!("wallpaper theme failed: {err:#}"),
            })
            .detach();
        }
        _ => theme.update(cx, |engine, cx| {
            if let Err(err) = engine.set_source(source, dark, now) {
                tracing::error!("theme source failed: {err:#}");
            }
            cx.notify();
        }),
    }
}

/// Installs the shared entities as globals.
pub fn init_state(config_store: ConfigStore, cx: &mut App) -> (Entity<ConfigStore>, Entity<ShellState>) {
    let cfg = config_store.snapshot();
    let config = cx.new(|_| config_store);
    cx.set_global(GlobalConfig(config.clone()));
    let shell = cx.new(|_| ShellState::default());
    cx.set_global(GlobalShell(shell.clone()));
    let theme = mandara_core::init_theme(cfg.theme.source.clone(), cfg.theme.mode.is_dark(), cfg.theme.adjustments, cx);
    theme.update(cx, |engine, _| engine.set_duration(theme_transition(&cfg)));
    (config, shell)
}

/// The app-theme targets `[integrations]` enables.
pub fn app_theme_targets(i: &mandara_config::IntegrationsConfig) -> (mandara_theme::templates::Targets, mandara_theme::templates::AppTargets) {
    let targets = mandara_theme::templates::Targets { ghostty: i.ghostty, gtk: i.gtk4, qt6: i.qt6, zed: i.zed, fuzzel: i.fuzzel, btop: i.btop, kvantum: i.kvantum, kvantum_apply: i.kvantum_apply };
    let apps = mandara_theme::templates::AppTargets {
        kitty: i.kitty,
        foot: i.foot,
        alacritty: i.alacritty,
        wezterm: i.wezterm,
        helix: i.helix,
        neovim: i.neovim,
        firefox: i.firefox,
        vesktop: i.vesktop,
        kde: i.kde,
    };
    (targets, apps)
}

/// Writes the cross-app theme files (Ghostty, GTK, Qt, Zed, Fuzzel, btop…)
/// for `palette`, honouring `[integrations]`; runs off the UI thread.
pub fn export_app_themes(palette: mandara_theme::MandaraPalette, cx: &mut App) -> gpui::Task<Vec<std::path::PathBuf>> {
    let (targets, apps) = app_theme_targets(&config(cx).read(cx).get().integrations);
    cx.background_executor().spawn(async move {
        let config_dir = dirs::config_dir().unwrap_or_default();
        let data_dir = dirs::data_dir().unwrap_or_default();
        let cache = mandara_config::paths::themes_cache_dir();
        let ledger_path = mandara_config::paths::app_themes_ledger();
        let mut ledger = mandara_theme::templates::Ledger::load(&ledger_path);
        let mut files = mandara_theme::templates::sync(&palette, targets, &cache, &config_dir, &mut ledger);
        files.extend(mandara_theme::templates::sync_apps(&palette, apps, &cache, &config_dir, &data_dir, &mut ledger));
        if let Err(err) = ledger.store(&ledger_path) {
            tracing::warn!("app themes: {}: {err}", ledger_path.display());
        }
        files
    })
}

/// Waits this long after the last palette or `[integrations]` change before
/// writing app themes (a theme crossfade or slider drag writes once).
const APP_THEME_SETTLE: Duration = Duration::from_millis(600);

/// What decides the app-theme files: the palette's identity and the
/// enabled integrations. A new key means the files must be rewritten.
fn app_theme_key(palette: &mandara_theme::MandaraPalette, integrations: &mandara_config::IntegrationsConfig) -> (String, mandara_config::IntegrationsConfig) {
    (format!("{}{}{}{}", palette.name, palette.primary.to_hex(), palette.surface_base.to_hex(), palette.is_dark), integrations.clone())
}

/// Re-exports app themes shortly after the palette settles or an app is
/// switched on / off in `[integrations]` (shell process).
pub fn init_app_theme_sync(cx: &mut App) {
    let theme = mandara_core::theme_entity(cx);
    let mut last: Option<(String, mandara_config::IntegrationsConfig)> = None;
    let pending: std::rc::Rc<std::cell::RefCell<Option<Task<()>>>> = Default::default();
    let on_change = std::rc::Rc::new(std::cell::RefCell::new(move |cx: &mut App| {
        let palette = mandara_core::theme_entity(cx).read(cx).target().clone();
        let key = app_theme_key(&palette, &config(cx).read(cx).get().integrations);
        if last.as_ref() == Some(&key) {
            return;
        }
        last = Some(key);
        let slot = pending.clone();
        *pending.borrow_mut() = Some(cx.spawn(async move |cx| {
            cx.background_executor().timer(APP_THEME_SETTLE).await;
            let task = cx.update(|cx| export_app_themes(palette, cx));
            let files = task.await;
            tracing::info!("app themes updated ({} files)", files.len());
            cx.update(|cx| crate::powerctl::hooks::templates_rendered(&files, cx));
            slot.borrow_mut().take();
        }));
    }));
    (on_change.borrow_mut())(cx);
    let on_theme = on_change.clone();
    cx.observe(&theme, move |_, cx| (on_theme.borrow_mut())(cx)).detach();
    let store = config(cx);
    cx.observe(&store, move |_, cx| (on_change.borrow_mut())(cx)).detach();
}

/// Keeps the interface language in step with `general.locale` (shell and
/// Studio). Views re-render on config changes, so text follows at once.
pub fn init_language(cx: &mut App) {
    let store = config(cx);
    let mut last = store.read(cx).get().general.locale;
    mandara_i18n::set_language(last.lang());
    cx.observe(&store, move |store, cx| {
        let locale = store.read(cx).get().general.locale;
        if locale != last {
            last = locale;
            mandara_i18n::set_language(locale.lang());
            cx.refresh_windows();
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_themes_are_rewritten_when_an_app_is_switched_on_or_off() {
        let palette = mandara_theme::engine::default_palette(true);
        let mut integrations = mandara_config::IntegrationsConfig::default();
        let before = app_theme_key(&palette, &integrations);
        assert_eq!(before, app_theme_key(&palette, &integrations), "an unrelated config change writes nothing");
        integrations.kitty = !integrations.kitty;
        assert_ne!(before, app_theme_key(&palette, &integrations), "a toggled app is written (or taken back) at once");
        let light = mandara_theme::engine::default_palette(false);
        assert_ne!(app_theme_key(&palette, &integrations), app_theme_key(&light, &integrations), "a new palette rewrites them");
    }

    #[test]
    fn every_integration_maps_to_its_target() {
        let all_off = mandara_config::IntegrationsConfig {
            version: mandara_config::upgrade::INTEGRATIONS_VERSION,
            ghostty: false,
            gtk4: false,
            qt6: false,
            zed: false,
            fuzzel: false,
            btop: false,
            compositor_borders: false,
            kvantum: false,
            kvantum_apply: false,
            kitty: false,
            foot: false,
            alacritty: false,
            wezterm: false,
            helix: false,
            neovim: false,
            firefox: false,
            vesktop: false,
            kde: false,
        };
        let (t, a) = app_theme_targets(&all_off);
        assert_eq!(a, mandara_theme::templates::AppTargets::default());
        assert!(!(t.ghostty || t.gtk || t.qt6 || t.zed || t.fuzzel || t.btop || t.kvantum || t.kvantum_apply));
        let gtk = app_theme_targets(&mandara_config::IntegrationsConfig { gtk4: true, ..all_off.clone() }).0;
        assert!(gtk.gtk && !gtk.qt6);
        let kde = app_theme_targets(&mandara_config::IntegrationsConfig { kde: true, ..all_off }).1;
        assert!(kde.kde && !kde.kitty);
    }

    #[test]
    fn default_integrations_write_nothing_outside_mandaras_folders() {
        let dir = std::env::temp_dir().join(format!("mandara-defaults-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (cache, config, data) = (dir.join("cache/mandara/themes"), dir.join("config"), dir.join("data"));
        // A user's own configs, as they might have them.
        let user: &[(&str, &str)] = &[
            ("gtk-4.0/gtk.css", "@import 'colors.css';\nbutton { color: red; }\n"),
            ("gtk-3.0/gtk.css", "window { padding: 0; }\n"),
            ("fuzzel/fuzzel.ini", "[main]\nfont=Inter\n"),
            ("ghostty/config", "theme = catppuccin\n"),
            ("kitty/kitty.conf", "font_size 11\n"),
            ("Kvantum/kvantum.kvconfig", "[General]\ntheme=KvArc\n"),
        ];
        for (rel, body) in user {
            let f = config.join(rel);
            std::fs::create_dir_all(f.parent().unwrap()).unwrap();
            std::fs::write(&f, body).unwrap();
        }
        let listing = |root: &std::path::Path| -> Vec<std::path::PathBuf> {
            fn walk(d: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
                for e in std::fs::read_dir(d).into_iter().flatten().flatten() {
                    let p = e.path();
                    out.push(p.clone());
                    if p.is_dir() {
                        walk(&p, out);
                    }
                }
            }
            let mut out = Vec::new();
            walk(root, &mut out);
            out.sort();
            out
        };
        let before = listing(&dir);
        let (targets, apps) = app_theme_targets(&mandara_config::IntegrationsConfig::default());
        let ledger_path = config.join("mandara/generated/app-themes.toml");
        let mut ledger = mandara_theme::templates::Ledger::load(&ledger_path);
        for p in [mandara_theme::engine::default_palette(true), mandara_theme::engine::default_palette(false)] {
            let mut files = mandara_theme::templates::sync(&p, targets, &cache, &config, &mut ledger);
            files.extend(mandara_theme::templates::sync_apps(&p, apps, &cache, &config, &data, &mut ledger));
            assert!(files.is_empty(), "defaults theme no app: {files:?}");
        }
        ledger.store(&ledger_path).unwrap();
        assert_eq!(listing(&dir), before, "no file or folder appeared anywhere");
        for (rel, body) in user {
            assert_eq!(std::fs::read_to_string(config.join(rel)).unwrap(), *body, "{rel} changed");
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reduced_motion_makes_theme_changes_instant() {
        let mut cfg = ShellConfig::default();
        cfg.theme.transition_ms = 700;
        assert_eq!(theme_transition(&cfg), Duration::from_millis(700));
        cfg.motion.reduce_motion = true;
        assert_eq!(theme_transition(&cfg), Duration::ZERO);
    }
}
