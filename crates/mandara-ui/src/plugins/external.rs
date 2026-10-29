//! Third-party plugins: every plugin found in the plugin directories
//! (`~/.local/share/mandara/plugins-dev`, `…/plugins`, `$XDG_DATA_DIRS/
//! mandara/plugins`) is registered like a built-in. Its WebAssembly
//! component is loaded on first use; one instance backs its bar item, panel
//! and sidebar card. Views come back as a node tree rendered with Mandara's
//! own styling, so third-party plugins look native.

use std::cell::RefCell;

use gpui::BorrowAppContext as _;
use std::collections::HashMap;
use std::rc::Rc;
use std::time::Duration;

use gpui::{AnyElement, App, FontWeight, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px, size};
use mandara_core::ToGpui;
use mandara_plugins::wasm::{self, Discovered, Kind, Palette, Source, Tone, View, WasmPlugin};
use mandara_plugins::{PanelSpec, PluginInstance, PluginManifest, PluginOrigin, PluginRegistry};
use mandara_theme::{Color, MandaraPalette};

use super::fn_view;
use super::panel_fx::{ViewSet, redraw_in};
use crate::icons::icon;
use crate::kit::{card, text, tokens};
use crate::state::config;

type Shared = Rc<RefCell<Option<WasmPlugin>>>;

/// Shortest refresh interval a plugin may ask for.
const MIN_REFRESH_MS: u64 = 250;
/// Deepest node nesting rendered (guards against cyclic / hostile trees).
const MAX_DEPTH: u32 = 32;
/// Font sizes a plugin may request.
const MIN_TEXT_PX: f32 = 6.0;
const MAX_TEXT_PX: f32 = 96.0;
/// Default icon size.
const ICON_PX: f32 = 16.0;
/// Button geometry.
const BUTTON_H_PX: f32 = 30.0;
const BUTTON_PAD_X_PX: f32 = 14.0;
const BUTTON_GAP_PX: f32 = 6.0;
/// Meter geometry and track alpha.
const METER_H_PX: f32 = 6.0;
const METER_MIN_W_PX: f32 = 40.0;
const METER_TRACK_ALPHA: f32 = 0.1;
/// Default panel size and padding for third-party panels.
const PANEL_W_PX: f32 = 360.0;
const PANEL_H_PX: f32 = 380.0;
const PANEL_PAD_PX: f32 = tokens::LG;

thread_local! {
    /// One loaded instance per plugin id (bar, panel and card share it).
    static LOADED: RefCell<HashMap<String, Shared>> = RefCell::new(HashMap::new());
    /// Why a plugin failed to load (`{err:#}`), for `plugins list` and
    /// Studio → Plugins → Local.
    static LOAD_ERRORS: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
}

/// What `host::setting` answers for `d`: the defaults its manifest declares
/// (`[[settings]]`), overlaid with `[plugins.settings.<id>]`.
fn settings_for(d: &Discovered, cx: &App) -> serde_json::Map<String, serde_json::Value> {
    let mut settings: serde_json::Map<String, serde_json::Value> = d.settings.iter().map(|s| (s.key.clone(), s.default_json())).collect();
    if let Some(table) = config(cx).read(cx).get().plugins.settings.get(&d.id).and_then(|t| serde_json::to_value(t).ok()).and_then(|v| v.as_object().cloned()) {
        settings.extend(table);
    }
    settings
}

/// The shared instance for `d`, loading it on first use.
fn instance(d: &Discovered, cx: &App) -> Shared {
    if let Some(s) = LOADED.with(|l| l.borrow().get(&d.id).cloned()) {
        return s;
    }
    let settings = settings_for(d, cx);
    let allow_commands = d.permissions.iter().any(|p| p == wasm::PERMISSION_COMMANDS);
    let loaded = mandara_plugins::guard(&format!("plugin `{}` load", d.id), || WasmPlugin::load(&d.id, &d.dir, &d.entry, settings, allow_commands))
        .unwrap_or_else(|| Err(anyhow::anyhow!("loader crashed")));
    let shared: Shared = Rc::new(RefCell::new(match loaded {
        Ok(p) => {
            tracing::info!("loaded plugin `{}` from {}", d.id, d.entry.display());
            Some(p)
        }
        Err(err) => {
            tracing::error!("plugin `{}` failed to load: {err:#}", d.id);
            LOAD_ERRORS.with(|e| e.borrow_mut().insert(d.id.clone(), format!("{err:#}")));
            None
        }
    }));
    LOADED.with(|l| l.borrow_mut().insert(d.id.clone(), shared.clone()));
    shared
}

/// A third-party plugin's runtime state in this process.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "state", content = "error", rename_all = "kebab-case")]
pub enum LoadState {
    /// Loaded and answering calls.
    Running,
    /// Its component couldn't be loaded or `init` refused.
    Failed(String),
    /// Trapped or ran past its time budget; off until reloaded.
    Disabled(String),
}

/// `None` until the plugin is first shown (loading is lazy).
pub fn load_state(id: &str) -> Option<LoadState> {
    LOADED.with(|l| {
        l.borrow().get(id).map(|s| match &*s.borrow() {
            None => LoadState::Failed(LOAD_ERRORS.with(|e| e.borrow().get(id).cloned()).unwrap_or_else(|| "loader crashed".into())),
            Some(p) => p.disabled.clone().map(LoadState::Disabled).unwrap_or(LoadState::Running),
        })
    })
}

/// Why a plugin isn't running, if it isn't.
pub fn status(id: &str) -> Option<String> {
    load_state(id).map(|s| match s {
        LoadState::Running => "running".into(),
        LoadState::Failed(e) => format!("failed to load: {e}"),
        LoadState::Disabled(r) => format!("disabled after an error: {r}"),
    })
}

/// Forgets a plugin's instance and load error (removed or reloaded).
fn forget(id: &str) {
    LOADED.with(|l| l.borrow_mut().remove(id));
    LOAD_ERRORS.with(|e| e.borrow_mut().remove(id));
}

/// The palette record plugins receive, from the live theme. Cached for the
/// last palette seen, so a render doesn't re-encode ten hex strings.
pub fn palette_for(p: &MandaraPalette) -> Palette {
    thread_local! {
        static LAST: RefCell<Option<(MandaraPalette, Palette)>> = const { RefCell::new(None) };
    }
    LAST.with(|last| {
        let mut last = last.borrow_mut();
        if let Some((key, value)) = last.as_ref()
            && key == p
        {
            return value.clone();
        }
        let value = encode_palette(p);
        *last = Some((p.clone(), value.clone()));
        value
    })
}

fn encode_palette(p: &MandaraPalette) -> Palette {
    Palette {
        dark: p.is_dark,
        primary: p.primary.to_hex(),
        on_primary: p.on_primary.to_hex(),
        surface: p.surface_base.to_hex(),
        on_surface: p.on_surface.to_hex(),
        on_surface_variant: p.on_surface_variant.to_hex(),
        container: p.surface_container.to_hex(),
        error: p.urgent_coral.to_hex(),
        success: p.success_emerald.to_hex(),
        warning: p.warning_amber.to_hex(),
    }
}

/// The theme color of a view tone.
pub fn tone_color(t: Tone, p: &MandaraPalette) -> Color {
    match t {
        Tone::Normal => p.on_surface,
        Tone::Muted => p.on_surface_variant,
        Tone::Accent => p.primary,
        Tone::Error => p.urgent_coral,
        Tone::Success => p.success_emerald,
        Tone::Warning => p.warning_amber,
    }
}

/// Renders a plugin view tree (arena) with Mandara's components.
/// `emoji` is the colour emoji family for text that is only emoji.
fn render(view: &View, palette: &MandaraPalette, plugin: &Shared, id: &str, views: &ViewSet, emoji: Option<&gpui::SharedString>) -> AnyElement {
    #[allow(clippy::too_many_arguments)]
    fn node(view: &View, i: u32, depth: u32, p: &MandaraPalette, plugin: &Shared, id: &str, views: &ViewSet, emoji: Option<&gpui::SharedString>, key: &mut usize) -> AnyElement {
        let Some(n) = view.nodes.get(i as usize).filter(|_| depth < MAX_DEPTH) else { return div().into_any_element() };
        *key += 1;
        let children: Vec<AnyElement> = n.children.iter().map(|c| node(view, *c, depth + 1, p, plugin, id, views, emoji, key)).collect();
        let color = tone_color(n.tone, p);
        let size_or = |d: f32| if n.size > 0.0 { n.size.clamp(MIN_TEXT_PX, MAX_TEXT_PX) } else { d };
        let mut el = match n.kind {
            Kind::Row => div().flex().items_center().gap(px(n.gap.max(0.0))).children(children),
            Kind::Column => div().flex().flex_col().gap(px(n.gap.max(0.0))).children(children),
            Kind::Card => card(p).flex().flex_col().gap(px(if n.gap > 0.0 { n.gap } else { tokens::SM })).children(children),
            Kind::Text => div()
                .text_size(px(size_or(text::BODY)))
                .text_color(color.hsla())
                .when(n.bold, |el| el.font_weight(FontWeight::SEMIBOLD))
                .when_some(emoji.filter(|_| crate::fonts::emoji_only(&n.text)), |el, family| el.font_family(family.clone()))
                .child(n.text.clone()),
            Kind::Icon => div().child(icon(crate::icons::ICON_NAMES.iter().find(|x| **x == n.text).copied().unwrap_or("circle"), size_or(ICON_PX), color)),
            Kind::Button => div()
                .h(px(BUTTON_H_PX))
                .px(px(BUTTON_PAD_X_PX))
                .rounded_full()
                .flex()
                .items_center()
                .gap(px(BUTTON_GAP_PX))
                .cursor_pointer()
                .bg(if n.tone == Tone::Accent { p.primary.hsla() } else { p.surface_container_high.hsla() })
                .text_color(if n.tone == Tone::Accent { p.on_primary.hsla() } else { p.on_surface.hsla() })
                .text_size(px(size_or(text::BODY_S)))
                .font_weight(FontWeight::MEDIUM)
                .children(children)
                .when(!n.text.is_empty(), |el| el.child(n.text.clone())),
            Kind::Meter => div()
                .h(px(METER_H_PX))
                .w_full()
                .min_w(px(METER_MIN_W_PX))
                .rounded_full()
                .bg(p.on_surface.with_alpha(METER_TRACK_ALPHA).hsla())
                .child(div().h_full().rounded_full().w(gpui::relative(n.value.clamp(0.0, 1.0))).bg(color.hsla())),
            Kind::Spacer => div().flex_1(),
        };
        if n.grow {
            el = el.flex_1().min_w(px(0.0));
        }
        if n.action.is_empty() {
            return el.into_any_element();
        }
        let (action, plugin, views) = (n.action.clone(), plugin.clone(), views.clone());
        el.id(gpui::SharedString::from(format!("plg-{id}-{key}")))
            .cursor_pointer()
            .on_click(move |_, _, cx| {
                let commands = match plugin.borrow_mut().as_mut() {
                    Some(pl) => {
                        pl.on_event(&action);
                        pl.take_commands()
                    }
                    None => Vec::new(),
                };
                run_commands(commands, cx);
                // The plugin's state changed: re-render its bar, panel and card.
                views.notify(cx);
            })
            .into_any_element()
    }
    let mut key = 0;
    node(view, view.root, 0, palette, plugin, id, views, emoji, &mut key)
}

/// Runs commands a plugin queued (through the documented registry).
fn run_commands(lines: Vec<String>, cx: &mut App) {
    for line in lines {
        let words: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        let registry = cx.global::<PluginRegistry>().commands.clone();
        if let Err(err) = registry.execute(&words, cx) {
            tracing::warn!(target: "mandara::plugin", "plugin command `{line}` failed: {err}");
        }
    }
}

#[derive(Copy, Clone)]
enum Surface {
    Bar,
    Panel,
    Sidebar,
}

fn surface_view(d: &Discovered, which: Surface, views: ViewSet, window: &mut gpui::Window, cx: &mut App) -> gpui::AnyView {
    let plugin = instance(d, cx);
    let refresh = plugin.borrow_mut().as_mut().map(|p| p.refresh_ms()).unwrap_or(0);
    // The plugin's refresh interval runs only while this surface renders
    // (redraw_in from render), never as a free-running timer.
    let tick = (refresh > 0).then(|| Duration::from_millis(u64::from(refresh).max(MIN_REFRESH_MS)));
    let id = d.id.clone();
    let declared = d.clone();
    // Settings resolved under a config generation (the snapshot's address):
    // re-derived only when the config changes, not every frame.
    let settings_gen = RefCell::new(0_usize);
    fn_view(
        None,
        move |palette, window, cx| {
            views.track(window);
            // Looked up per render (a map hit): after a hot reload an open
            // bar item, panel or card draws the fresh instance at once
            // instead of the one it was created with.
            let plugin = instance(&declared, cx);
            if let Some(every) = tick {
                redraw_in(every, window, cx);
            }
            let theme = palette_for(palette);
            // Live settings: a Studio edit reaches the plugin on its next render.
            let generation = std::sync::Arc::as_ptr(&config(cx).read(cx).snapshot()) as usize;
            if *settings_gen.borrow() != generation {
                *settings_gen.borrow_mut() = generation;
                let current = settings_for(&declared, cx);
                if let Some(p) = plugin.borrow_mut().as_mut().filter(|p| p.settings() != &current) {
                    p.set_settings(current);
                }
            }
            let view = {
                let mut guard = plugin.borrow_mut();
                match guard.as_mut() {
                    Some(p) => match which {
                        Surface::Bar => p.bar(&theme),
                        Surface::Panel => p.panel(&theme),
                        Surface::Sidebar => p.sidebar(&theme),
                    },
                    None => None,
                }
            };
            let commands = plugin.borrow_mut().as_mut().map(|p| p.take_commands()).unwrap_or_default();
            if !commands.is_empty() {
                // Defer: rendering must not mutate the app.
                cx.defer(move |cx| run_commands(commands, cx));
            }
            match view {
                Some(v) => {
                    let emoji = cx.try_global::<crate::fonts::Fonts>().and_then(|f| f.emoji.clone());
                    let el = render(&v, palette, &plugin, &id, &views, emoji.as_ref());
                    match which {
                        Surface::Panel => div().size_full().p(px(PANEL_PAD_PX)).child(el).into_any_element(),
                        _ => el,
                    }
                }
                None => match &*plugin.borrow() {
                    Some(p) if p.disabled.is_some() => div().text_size(px(text::LABEL)).text_color(palette.urgent_coral.hsla()).child(mandara_i18n::t("plugin stopped")).into_any_element(),
                    None => div().text_size(px(text::LABEL)).text_color(palette.urgent_coral.hsla()).child(mandara_i18n::t("plugin failed")).into_any_element(),
                    _ => div().into_any_element(),
                },
            }
        },
        window,
        cx,
    )
}

thread_local! {
    /// Fingerprint (files' sizes + mtimes) of each registered plugin, to
    /// spot updates on disk.
    static PRINTS: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
}

/// Hash of the manifest, component and catalogs' sizes and mtimes.
fn fingerprint(d: &Discovered) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    let mut add = |p: &std::path::Path| {
        if let Ok(m) = std::fs::metadata(p) {
            p.hash(&mut h);
            m.len().hash(&mut h);
            m.modified().ok().hash(&mut h);
        }
    };
    add(&d.dir.join("plugin.toml"));
    add(&d.entry);
    if let Ok(entries) = std::fs::read_dir(d.dir.join("locales")) {
        let mut files: Vec<_> = entries.flatten().map(|e| e.path()).collect();
        files.sort();
        for f in files {
            add(&f);
        }
    }
    h.finish()
}

fn register_one(r: &mut PluginRegistry, d: Discovered) {
    let manifest = PluginManifest {
        id: d.id.clone(),
        name: d.name.clone(),
        description: d.description.clone(),
        author: d.author.clone(),
        version: d.version.clone(),
        origin: PluginOrigin::ThirdParty { path: d.dir.clone() },
        enabled_by_default: d.source == Source::Development,
        zone: d.zone,
        translations: d.translations.clone(),
        permissions: d.permissions.clone(),
        homepage: d.homepage.clone(),
        settings: d.settings.clone(),
    };
    if PRINTS.with(|p| !p.borrow().contains_key(&d.id)) {
        let print = fingerprint(&d);
        PRINTS.with(|p| p.borrow_mut().insert(d.id.clone(), print));
    }
    r.register(manifest, move |window, cx, _panel_cx| {
        // One plugin instance backs all three surfaces: a click in any of
        // them re-renders the others.
        let views = ViewSet::default();
        PluginInstance {
            bar: Some(surface_view(&d, Surface::Bar, views.clone(), window, cx)),
            panel: Some(PanelSpec {
                view: surface_view(&d, Surface::Panel, views.clone(), window, cx),
                size: Rc::new(std::cell::Cell::new(size(px(PANEL_W_PX), px(PANEL_H_PX)))),
            }),
            sidebar: Some(surface_view(&d, Surface::Sidebar, views, window, cx)),
            ..PluginInstance::empty()
        }
    });
}

fn is_builtin(r: &PluginRegistry, id: &str) -> bool {
    r.get(id).is_some_and(|f| f.manifest.origin == PluginOrigin::Builtin)
}

/// Registers every discovered plugin. Development plugins start enabled;
/// installed and system ones are opt-in (Studio → Plugins).
pub fn register(r: &mut PluginRegistry) {
    for d in wasm::discover() {
        if is_builtin(r, &d.id) {
            tracing::warn!("plugin `{}` at {} clashes with a built-in id; skipped", d.id, d.dir.display());
            continue;
        }
        register_one(r, d);
    }
}

/// Hot reload: re-scans the plugin folders and adds new plugins, reloads
/// changed ones (fresh WebAssembly instance, new manifest and catalogs) and
/// removes deleted ones — live, without restarting the shell. Returns what
/// changed.
pub fn sync(cx: &mut App) -> Vec<String> {
    apply_scan(scan(), cx)
}

/// Discovers plugins and fingerprints their files (blocking file I/O: run
/// it off the UI thread where possible).
fn scan() -> Vec<(Discovered, u64)> {
    wasm::discover().into_iter().map(|d| {
        let print = fingerprint(&d);
        (d, print)
    }).collect()
}

/// Applies a [`scan`]: registers, reloads and removes plugins.
fn apply_scan(found: Vec<(Discovered, u64)>, cx: &mut App) -> Vec<String> {
    let mut changes = Vec::new();
    cx.update_global::<PluginRegistry, _>(|r, _| {
        let registered: Vec<String> = r.manifests().filter(|m| matches!(m.origin, PluginOrigin::ThirdParty { .. })).map(|m| m.id.clone()).collect();
        for id in registered.iter().filter(|id| !found.iter().any(|(d, _)| &d.id == *id)) {
            r.unregister(id);
            forget(id);
            PRINTS.with(|p| p.borrow_mut().remove(id));
            changes.push(format!("removed {id}"));
        }
        for (d, print) in found {
            if is_builtin(r, &d.id) {
                continue;
            }
            let known = PRINTS.with(|p| p.borrow().get(&d.id).copied());
            if known == Some(print) {
                continue;
            }
            forget(&d.id);
            PRINTS.with(|p| p.borrow_mut().insert(d.id.clone(), print));
            changes.push(format!("{} {}", if known.is_some() { "reloaded" } else { "added" }, d.id));
            register_one(r, d);
        }
    });
    if !changes.is_empty() {
        tracing::info!("plugins hot-reloaded: {}", changes.join(", "));
        cx.refresh_windows();
    }
    changes
}

/// Quiet time after the last change in a plugin folder before reloading
/// (lets copies and builds finish writing).
pub const RELOAD_DEBOUNCE: Duration = Duration::from_millis(500);

/// Watches the plugin folders and hot-reloads on changes (debounced).
pub fn watch(cx: &mut App) {
    use notify::Watcher as _;
    // A change signal: one pending wake-up is enough (bursts coalesce).
    let (tx, rx) = async_channel::bounded::<()>(1);
    let mut watcher = match notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if res.is_ok_and(|e| !matches!(e.kind, notify::EventKind::Access(_))) {
            let _ = tx.try_send(());
        }
    }) {
        Ok(w) => w,
        Err(err) => {
            tracing::warn!("plugin hot reload unavailable: {err}");
            return;
        }
    };
    for (dir, source) in wasm::plugin_dirs() {
        if source != Source::System {
            let _ = std::fs::create_dir_all(&dir);
        }
        if dir.is_dir() {
            if let Err(err) = watcher.watch(&dir, notify::RecursiveMode::Recursive) {
                tracing::warn!("watching {}: {err}", dir.display());
            }
        }
    }
    cx.spawn(async move |cx| {
        // Keep the watcher alive for the life of the shell.
        let _watcher = watcher;
        while rx.recv().await.is_ok() {
            // Let copies / builds finish writing, then coalesce the burst.
            cx.background_executor().timer(RELOAD_DEBOUNCE).await;
            while rx.try_recv().is_ok() {}
            let found = cx.background_executor().spawn(async { scan() }).await;
            cx.update(|cx| {
                apply_scan(found, cx);
            });
        }
    })
    .detach();
}
