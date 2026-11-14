//! Shell-wide shared state and the config/theme plumbing every surface uses.

use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Context, Entity, Global, Task};
use mandara_config::{ConfigStore, ReloadOutcome, ShellConfig};
use mandara_theme::{ThemeEngine, ThemeSource};

use crate::floating::dock::{Effect, PanelDock};

/// Live UI state shared by every surface (and driven by IPC commands).
#[derive(Default)]
pub struct ShellState {
    /// Plugin id whose panel is open (`None` = collapsed).
    pub panel: Option<String>,
    /// Horizontal anchor (surface x) for panels opened at the pointer, like
    /// the main menu.
    pub panel_anchor: Option<f32>,
    /// The open panel is pinned: dismissals leave it open (see
    /// [`crate::floating::dock`]).
    pub panel_pinned: bool,
    /// The panel was closed for a capture: bars drop it at once instead of
    /// folding it back, so it is not in the frame. Reset when one opens.
    pub panel_close_at_once: bool,
    /// The output whose bar shows the open panel (`None`: every bar).
    panel_output: Option<String>,
    /// The output the sidebar was revealed on (`None`: every output).
    sidebar_output: Option<String>,
    /// The output of the bar being clicked right now (see [`on_bar`]).
    click_output: Option<String>,
    /// Outputs with a bar, one entry per live bar.
    bars: Vec<String>,
    /// `(plugin, output)`: a floating card just docked into the bar there,
    /// so that output's panel opens already expanded where the card landed.
    pub snap_open: Option<(String, String)>,
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
    /// Runs `f` on the bar's panel slot, then carries out its effect
    /// (floating a displaced pinned panel, raising or closing a card).
    fn dock<R>(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut PanelDock) -> (R, Effect)) -> R {
        let mut d = PanelDock { open: self.panel.take(), pinned: self.panel_pinned };
        let before = d.clone();
        let (out, effect) = f(&mut d);
        if d.open.is_some() && d.open != before.open {
            self.panel_output = self.opening_output(cx);
            self.panel_close_at_once = false;
        }
        if d != before {
            self.snap_open = None;
            cx.notify();
        }
        self.panel = d.open;
        self.panel_pinned = d.pinned;
        if effect != Effect::None {
            cx.defer(move |cx| crate::floating::apply(effect, cx));
        }
        out
    }

    /// The chip / `panel toggle`: closes the open panel (pinned or not) or
    /// the plugin's floating card; opens anything else.
    pub fn toggle_panel(&mut self, id: &str, cx: &mut Context<Self>) {
        let floating = crate::floating::is_floating(id, cx);
        self.dock(cx, |d| ((), d.toggle(id, floating)));
    }

    /// Opens (or, with `None`, closes) the bar's panel on purpose: a
    /// command, a click, the plugin itself. Closing ignores the pin; a
    /// pinned panel displaced by another one floats.
    pub fn set_panel(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        match id {
            None => self.dock(cx, |d| {
                d.close();
                ((), Effect::None)
            }),
            Some(id) => {
                let floating = crate::floating::is_floating(&id, cx);
                self.dock(cx, |d| ((), d.open(&id, floating)))
            }
        }
    }

    /// Closes the bar's panel with no closing animation (a screenshot is
    /// about to be taken). Returns whether one was open.
    pub fn close_panel_now(&mut self, cx: &mut Context<Self>) -> bool {
        let open = self.panel.is_some();
        if open {
            self.panel_close_at_once = true;
            self.set_panel(None, cx);
        }
        open
    }

    /// Closes the open panel unless it is pinned (outside clicks, focus
    /// changes, the pointer leaving, an overlay opening). Returns whether
    /// it closed.
    pub fn dismiss_panel(&mut self, cx: &mut Context<Self>) -> bool {
        self.dock(cx, |d| (d.dismiss(), Effect::None))
    }

    /// Pins `id` in the bar's slot (opening it there if needed).
    pub fn pin_panel(&mut self, id: &str, cx: &mut Context<Self>) {
        self.dock(cx, |d| ((), d.pin(id)));
    }

    /// Unpins `id`; it stays open until the next dismissal. Returns whether
    /// it was pinned.
    pub fn unpin_panel(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        self.dock(cx, |d| (d.unpin(id), Effect::None))
    }

    /// Whether `id` is the pinned panel.
    pub fn is_pinned(&self, id: &str) -> bool {
        self.panel_pinned && self.panel.as_deref() == Some(id)
    }

    /// The output a panel or the sidebar opens on: the bar you clicked,
    /// else the monitor you're on. `None` (every bar) when that output has
    /// no bar.
    fn opening_output(&self, cx: &App) -> Option<String> {
        let name = self.click_output.clone().or_else(|| crate::outputs::focused_monitor(cx))?;
        self.bars.iter().any(|b| b.eq_ignore_ascii_case(&name)).then_some(name)
    }

    /// Whether a surface on `output` shows what was opened on `opened`.
    fn shows_on(opened: Option<&str>, output: &str) -> bool {
        opened.is_none_or(|o| o.eq_ignore_ascii_case(output))
    }

    /// The plugin whose panel `output`'s bar shows: the open panel, on the
    /// output it was opened on only.
    pub fn panel_on(&self, output: &str) -> Option<&str> {
        self.panel.as_deref().filter(|_| Self::shows_on(self.panel_output.as_deref(), output))
    }

    /// Whether the sidebar shows on `output` (a pinned one, which config
    /// decides, shows on every output).
    pub fn sidebar_visible_on(&self, output: &str) -> bool {
        self.sidebar_visible() && Self::shows_on(self.sidebar_output.as_deref(), output)
    }

    /// A bar came up on `output`.
    pub fn bar_added(&mut self, output: &str) {
        self.bars.push(output.to_string());
    }

    /// A bar on `output` closed.
    pub fn bar_gone(&mut self, output: &str) {
        if let Some(i) = self.bars.iter().position(|b| b == output) {
            self.bars.swap_remove(i);
        }
    }

    /// Open by command, hover, or pin (pin lives in config).
    pub fn sidebar_visible(&self) -> bool {
        self.sidebar_open || self.sidebar_hover
    }

    pub fn set_sidebar(&mut self, open: bool, cx: &mut Context<Self>) {
        if open && !self.sidebar_visible() {
            self.sidebar_output = self.opening_output(cx);
        }
        self.sidebar_open = open;
        if !open {
            self.sidebar_hover = false;
        }
        cx.notify();
    }

    /// The pointer reached the sidebar's edge on `output`'s bar.
    pub fn hover_sidebar(&mut self, output: &str, cx: &mut Context<Self>) {
        if !self.sidebar_visible() {
            self.sidebar_output = Some(output.to_string());
        }
        self.set_sidebar_hover(true, cx);
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
                // A pinned panel stays open over the overlay.
                if !self.panel_pinned {
                    self.panel = None;
                }
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
                // A pinned panel stays open over the overlay.
                if !self.panel_pinned {
                    self.panel = None;
                }
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
                // A pinned panel stays open over the overlay.
                if !self.panel_pinned {
                    self.panel = None;
                }
            }
            cx.notify();
        }
    }
}

/// Runs `f` as a click on `output`'s bar: a panel or the sidebar it opens
/// shows on that output, not on every monitor's bar.
pub fn on_bar<R>(output: &str, cx: &mut App, f: impl FnOnce(&mut App) -> R) -> R {
    let state = shell(cx);
    state.update(cx, |s, _| s.click_output = Some(output.to_string()));
    let out = f(cx);
    state.update(cx, |s, _| s.click_output = None);
    out
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
/// Acknowledges actual delivery; a failed or closed reply falls back to a
/// local save. `None` means the other process is already known to be offline.
#[derive(Clone)]
pub struct ConfigMirror(pub std::rc::Rc<dyn Fn(
            std::sync::Arc<ShellConfig>,
            std::sync::Arc<ShellConfig>,
        ) -> Option<async_channel::Receiver<bool>>,
    >);
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

struct ConfigWriter(mandara_services::config_save::Writer);
impl Global for ConfigWriter {}

fn queue_save(
    cx: &mut App,
) -> Option<async_channel::Receiver<Result<mandara_config::store::Saved, String>>> {
    let save = config(cx).read(cx).save_request()?;
    if cx.try_global::<ConfigWriter>().is_none() {
        cx.set_global(ConfigWriter(Default::default()));
    }
    Some(cx.global::<ConfigWriter>().0.submit(save))
}

fn finish_save(saved: mandara_config::store::Saved, cx: &mut App) -> Result<(), String> {
    let changed = config(cx).update(cx, |store, cx| {
        let changed = store
            .finish_save(saved)
            .map_err(|error| format!("merging saved configuration: {error:#}"))?;
        if changed {
            cx.notify();
        }
        Ok::<_, String>(changed)
    })?;
    if changed {
        sync_theme_from_config(cx);
    }
    Ok(())
}

/// Saves pending preferences before handing over to another shell process.
pub fn flush_config(cx: &mut App) -> Result<(), String> {
    let Some(reply) = queue_save(cx) else {
        return Ok(());
    };
    let saved = reply.recv_blocking().map_err(|error| error.to_string())??;
    finish_save(saved, cx)
}

/// Mutates the shared config in memory (instantly visible to every surface)
/// and schedules a debounced atomic flush to disk. Theme-affecting changes
/// are forwarded to the theme engine immediately.
pub fn update_config(cx: &mut App, f: impl FnOnce(&mut ShellConfig)) {
    let config = config(cx);
    let before = config.read(cx).snapshot();
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
    let delivery = cx.try_global::<ConfigMirror>().cloned().and_then(|mirror| (mirror.0)(before, config.read(cx).snapshot()));
    if let Some(delivery) = delivery {
        // Delivery is independent of the save debounce. A later successful
        // edit must not discard an earlier failed delivery's local save.
        // The mirror closes superseded pending replies, keeping at most
        // one in-flight and one pending delivery waiter.
        cx.spawn(async move |cx| {
            if delivery.recv().await == Ok(false) {
                cx.update(schedule_flush);
            }
        })
        .detach();
    } else {
        schedule_flush(cx);
    }
}

fn schedule_flush(cx: &mut App) {
    let shell = shell(cx);
    shell.update(cx, |state, cx| {
        state.flush_task = Some(cx.spawn(async move |_, cx| {
            cx.background_executor().timer(FLUSH_DEBOUNCE).await;
            if let Some(reply) = cx.update(queue_save) {
                match reply.recv().await {
                    Ok(Ok(saved)) => {
                if let Err(error) = cx.update(|cx| finish_save(saved, cx)) {
                    tracing::error!("failed to persist config: {error}");
                }
            }
                    Ok(Err(error)) => tracing::error!("failed to persist config: {error}"),
                    // A newer pending save replaced this one.
                    Err(_) => {}
                }
            }
        }));
    });
}

/// Re-reads the config file after an external edit and applies it.
pub fn reload_config_from_disk(cx: &mut App) -> gpui::Task<Result<bool, String>> {
    let request = config(cx).read(cx).reload_request();
    cx.spawn(async move |cx| {
        let result = mandara_services::runtime::unblock(move || request.read()).await;
    let outcome = result.and_then(|loaded| {
            cx.update(|cx| {
                config(cx).update(cx, |store, cx| {
        let result = store.finish_reload(loaded);
        if matches!(result, Ok(ReloadOutcome::Changed)) {
            cx.notify();
        }
                    result
                })
            })
        });
    match outcome {
        Ok(ReloadOutcome::Changed) => {
            tracing::info!("config reloaded from disk");
                cx.update(sync_theme_from_config);
            Ok(true)
        }
        Ok(ReloadOutcome::Unchanged) => Ok(false),
        Err(error) => {
            tracing::warn!("ignoring invalid config edit: {error:#}");
            Err(format!("{error:#}"))
        }
    }
    })
}

/// How long a palette change crossfades: `theme.transition_ms`, or
/// instantly with reduced motion.
pub fn theme_transition(cfg: &ShellConfig) -> Duration {
    if cfg.motion.reduced(mandara_core::spring::system_reduced_motion()) { Duration::ZERO } else { Duration::from_millis(cfg.theme.transition_ms) }
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

mod app_themes;
pub use app_themes::{
    app_theme_status, app_theme_targets, export_app_themes, export_app_themes_report,
    init_app_theme_sync,
    };

#[cfg(test)]
fn app_theme_key(palette: &mandara_theme::MandaraPalette, integrations: &mandara_config::IntegrationsConfig) -> (
    mandara_theme::MandaraPalette, mandara_config::IntegrationsConfig) {
    (palette.clone(), integrations.clone())
}

/// Keeps the interface language in step with `general.locale` (shell and
/// Studio). Views re-render on config changes, so text follows at once.
///
/// Translations come from language packs: one used without its pack (the
/// shell's language, a plugin's or a widget's own) downloads in the
/// background and the windows re-render once it's in; a language just
/// chosen is also checked against the published pack
/// (`mandara_services::self_update::langpacks`).
pub fn init_language(cx: &mut App) {
    init_language_with_font(cx, |config| &config.typography.arabic_font);
}

/// The interface language stays shared; Studio's font fallback uses its own recipe.
pub fn init_studio_language(cx: &mut App) {
    init_language_with_font(cx, |config| &config.studio_typography().arabic_font);
}

fn init_language_with_font(cx: &mut App, arabic: fn(&ShellConfig) -> &str) {
    use mandara_services::self_update::langpacks;
    langpacks::install_hooks();
    let store = config(cx);
    let mut last = store.read(cx).get().general.locale;
    let mut in_use = langpacks::plugin_languages(store.read(cx).get());
    mandara_i18n::set_language(last.lang());
    // The font fallback chain follows the language (Urdu's Nastaliq, the
    // CJK / Bengali / Devanagari / Telugu faces).
    crate::fonts::sync_fallbacks(arabic(store.read(cx).get()), cx);
    let packs = langpacks::subscribe();
    cx.spawn(async move |cx| {
        while packs.recv().await.is_ok() {
            cx.update(|cx| cx.refresh_windows());
        }
    })
    .detach();
    cx.observe(&store, move |store, cx| {
        let locale = store.read(cx).get().general.locale;
        // Cheap (a few plugin tables): config changes arrive per slider step.
        let now = langpacks::plugin_languages(store.read(cx).get());
        for lang in now.iter().filter(|l| !in_use.contains(l)) {
            langpacks::selected(*lang);
        }
        in_use = now;
        if locale != last {
            last = locale;
            mandara_i18n::set_language(locale.lang());
            langpacks::selected(locale.lang());
            crate::fonts::sync_fallbacks(arabic(store.read(cx).get()), cx);
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
            cava: false, tmux: false, zellij: false, spicetify: false,
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
            app_themes: Default::default(),
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

    /// #22: a panel or the sidebar opened on one monitor shows on that
    /// monitor's bar only; with no output known, every bar shows it.
    #[test]
    fn panels_and_the_sidebar_show_on_their_own_output() {
        let mut s = ShellState { panel: Some("weather".into()), panel_output: Some("DP-1".into()), ..Default::default() };
        assert_eq!(s.panel_on("DP-1"), Some("weather"));
        assert_eq!(s.panel_on("dp-1"), Some("weather"));
        assert_eq!(s.panel_on("HDMI-A-1"), None);
        s.panel_output = None;
        assert_eq!(s.panel_on("HDMI-A-1"), Some("weather"));

        s.sidebar_open = true;
        s.sidebar_output = Some("HDMI-A-1".into());
        assert!(s.sidebar_visible_on("HDMI-A-1"));
        assert!(!s.sidebar_visible_on("DP-1"));

        s.bar_added("DP-1");
        s.bar_added("HDMI-A-1");
        s.bar_gone("DP-1");
        assert_eq!(s.bars, vec!["HDMI-A-1".to_string()]);
    }
}
