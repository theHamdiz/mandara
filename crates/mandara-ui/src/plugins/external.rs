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
use mandara_config::desktop_cards as dc;
use mandara_core::ToGpui;
use mandara_plugins::wasm::{self, Discovered, Kind, Palette, Source, Tone, View, WasmPlugin};
use mandara_plugins::{PanelSpec, PluginInstance, PluginManifest, PluginOrigin, PluginRegistry};
use mandara_theme::{Color, MandaraPalette};

use super::extension_view::{self as ext_view, Frame};
use super::fn_view;
use super::panel_fx::{ViewSet, redraw_in};
use crate::icons::icon;
use crate::kit::{card, text, tokens};
use crate::state::config;
use mandara_plugins::extension::Access;
use mandara_plugins::wasm::{ExtContext, ExtEvent, ExtSurface, ExtValue};

mod brokers;

pub(crate) type Shared = Rc<RefCell<Option<WasmPlugin>>>;

/// Shortest refresh interval a plugin may ask for.
const MIN_REFRESH_MS: u64 = 250;
/// Deepest node nesting rendered (guards against cyclic / hostile trees).
const MAX_DEPTH: u32 = 32;
/// Most nodes one plugin view renders. Children are node indices, so a
/// few nodes listing each other repeatedly could expand exponentially.
const MAX_RENDERED_NODES: usize = 2048;
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
/// Padding inside third-party panels (their size is plugin.toml's `[panel]`).
const PANEL_PAD_PX: f32 = tokens::LG;
/// The panel's "whose is this" line is quieter than muted text.
const ATTRIBUTION_ALPHA: f32 = 0.75;

thread_local! {
    /// One loaded instance per plugin id (bar, panel and card share it).
    static LOADED: RefCell<HashMap<String, Shared>> = RefCell::new(HashMap::new());
    /// Why a plugin failed to load (`{err:#}`), for `plugins list` and
    /// Studio → Plugins → Local.
    static LOAD_ERRORS: RefCell<HashMap<String, String>> = RefCell::new(HashMap::new());
    /// Every drawn surface of each plugin (bar, panel, card, desktop
    /// widgets, Studio tabs): a click or a broker reply re-renders them all.
    static VIEWS: RefCell<HashMap<String, ViewSet>> = RefCell::new(HashMap::new());
}

thread_local! {
    /// Each plugin's own catalogs, for the names Mandara shows outside its
    /// views (Studio tab titles, launcher provider names); a language loads
    /// once, on first use.
    static CATALOGS: RefCell<HashMap<String, wasm::PluginCatalogs>> = RefCell::new(HashMap::new());
}

/// `text` translated with plugin `d`'s catalog.
fn translated(d: &Discovered, text: &str) -> String {
    CATALOGS.with(|c| c.borrow_mut().entry(d.id.clone()).or_insert_with(|| wasm::PluginCatalogs::new(&d.dir)).translate(text))
}

/// The views of plugin `id` (made on first use; dropped views fall out).
pub(crate) fn views_of(id: &str) -> ViewSet {
    VIEWS.with(|v| v.borrow_mut().entry(id.to_string()).or_default().clone())
}

/// What plugin `d` may do now: its declared permissions, and of those the
/// ones the person granted (`[plugins.grants.<id>]`).
fn access_for(d: &Discovered, cx: &App) -> Access {
    Access::from_grant(&d.permissions, config(cx).read(cx).get().plugins.grants.get(&d.id))
}

thread_local! {
    /// Follows the config so a grant revoked in Studio applies to every
    /// loaded plugin at once, drawn or not.
    static GRANT_WATCH: RefCell<Option<gpui::Subscription>> = const { RefCell::new(None) };
}

/// Starts following grants (once; it costs nothing until the config changes).
pub(crate) fn watch_grants(cx: &mut App) {
    if GRANT_WATCH.with(|w| w.borrow().is_some()) {
        return;
    }
    let subscription = cx.observe(&config(cx), |_, cx| apply_grants(cx));
    GRANT_WATCH.with(|w| *w.borrow_mut() = Some(subscription));
}

/// Applies the config's grants to every loaded plugin: what lost its grant
/// is closed now (subscriptions, timers, queued requests, image handles).
fn apply_grants(cx: &mut App) {
    let loaded: Vec<(String, Shared)> = LOADED.with(|l| l.borrow().iter().map(|(k, v)| (k.clone(), v.clone())).collect());
    for (id, shared) in loaded {
        let Some(d) = discovered(&id) else { continue };
        let access = access_for(&d, cx);
        let changed = shared.borrow_mut().as_mut().is_some_and(|p| p.set_access(access));
        if changed {
            after_call(&id, &shared, cx);
            views_of(&id).notify(cx);
        }
    }
}

/// The loaded instance of plugin `id`, if it is loaded.
pub(crate) fn loaded(id: &str) -> Option<Shared> {
    LOADED.with(|l| l.borrow().get(id).cloned())
}

/// The registered plugin `id`.
pub(crate) fn discovered(id: &str) -> Option<Discovered> {
    DISCOVERED.with(|m| m.borrow().get(id).cloned())
}

/// After a call into the plugin outside rendering: runs the commands it
/// queued, hands its broker requests to the brokers and stops the
/// subscriptions it cancelled.
pub(crate) fn after_call(id: &str, plugin: &Shared, cx: &mut App) {
    let (commands, requests, cancelled, storage) = match plugin.borrow_mut().as_mut() {
        Some(p) => (p.take_commands(), p.take_requests(), p.take_cancelled(), p.take_storage_flush()),
        None => return,
    };
    if let Some((path, json)) = storage {
        write_storage_later(path, json, cx);
    }
    watch_grants(cx);
    run_commands(id, commands, cx);
    brokers::stop(id, &cancelled, cx);
    if !requests.is_empty() {
        brokers::dispatch(id, requests, cx);
    }
}

/// Writes a plugin's storage file on the background executor. Writes of
/// the same file never land out of order: each carries a generation and
/// only the newest one is written.
fn write_storage_later(path: std::path::PathBuf, json: String, cx: &mut App) {
    use std::sync::Mutex;
    static NEWEST: Mutex<Option<HashMap<std::path::PathBuf, u64>>> = Mutex::new(None);
    let generation = {
        let mut map = NEWEST.lock().unwrap_or_else(|e| e.into_inner());
        let g = map.get_or_insert_with(HashMap::new).entry(path.clone()).or_insert(0);
        *g += 1;
        *g
    };
    cx.background_executor()
        .spawn(async move {
            let map = NEWEST.lock().unwrap_or_else(|e| e.into_inner());
            if map.as_ref().and_then(|m| m.get(&path)) == Some(&generation)
                && let Err(err) = wasm::write_storage(&path, &json)
            {
                tracing::warn!(target: "mandara::plugin", "writing {}: {err}", path.display());
            }
        })
        .detach();
}

/// [`after_call`] from a render: deferred, since rendering must not
/// change the app.
fn after_render(id: &str, plugin: &Shared, cx: &mut App) {
    let pending = plugin.borrow().as_ref().is_some_and(|p| p.has_pending());
    if pending {
        let (id, plugin) = (id.to_string(), plugin.clone());
        cx.defer(move |cx| after_call(&id, &plugin, cx));
    }
}

/// Sends `event` to plugin `id`, runs what it queued and re-renders it.
pub(crate) fn send_event(id: &str, plugin: &Shared, event: &ExtEvent, cx: &mut App) {
    if let Some(p) = plugin.borrow_mut().as_mut() {
        p.extension_event(event);
    }
    after_call(id, plugin, cx);
    views_of(id).notify(cx);
}

/// The event sink for an extension view of plugin `d`.
pub(crate) fn sink_for(d: &Discovered) -> ext_view::Sink {
    let declared = d.clone();
    Rc::new(move |event, _window, cx| {
        let plugin = instance(&declared, cx);
        send_event(&declared.id, &plugin, &event, cx);
    })
}

/// Resolves plugin `id`'s image handles: only ones its live instance
/// issued and whose origin is still granted.
pub(crate) fn images_for(id: &str) -> ext_view::Images {
    let id = id.to_string();
    Rc::new(move |handle, _cx| {
        let plugin = loaded(&id)?;
        let guard = plugin.borrow();
        Some(guard.as_ref()?.image(handle)?.path.clone())
    })
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
    let access = access_for(d, cx);
    let loaded = mandara_plugins::guard(&format!("plugin `{}` load", d.id), || WasmPlugin::load(&d.id, &d.dir, &d.entry, settings, access))
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

/// Whether third-party plugin `id` shows anywhere but the sidebar: a bar
/// item, its panel (open or floating) or a desktop widget.
fn shown_outside_sidebar(id: &str, cx: &App) -> bool {
    let cfg = config(cx).read(cx).get();
    let in_bar = cfg.plugin_places(id).iter().any(|slot| slot.lane != mandara_config::LayoutLane::Sidebar);
    let on_desktop = cfg.desktop.widgets.items.iter().filter(|w| w.enabled).filter_map(dc::as_card).any(|c| dc::parse_plugin_card(&c.card).is_some_and(|(plugin, _)| plugin == id));
    let panel = crate::state::shell(cx).read(cx).panel.as_deref() == Some(id) || crate::floating::is_floating(id, cx);
    in_bar || on_desktop || panel
}

/// The sidebar closed: a plugin only its card showed is unloaded (its
/// WebAssembly memory goes back), and loads again when the card next shows.
fn release_card(id: &str, cx: &mut App) {
    if shown_outside_sidebar(id, cx) || !LOADED.with(|l| l.borrow().contains_key(id)) {
        return;
    }
    // A click handler may still hold the instance for a moment; the last
    // handle frees it. Its subscriptions end with it.
    LOADED.with(|l| l.borrow_mut().remove(id));
    brokers::forget(id, cx);
    mandara_services::memory::request_trim();
}

/// Forgets a plugin's instance and load error (removed or reloaded).
fn forget(id: &str, cx: &mut App) {
    LOADED.with(|l| l.borrow_mut().remove(id));
    CATALOGS.with(|c| c.borrow_mut().remove(id));
    LOAD_ERRORS.with(|e| e.borrow_mut().remove(id));
    brokers::forget(id, cx);
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
    ext_view::tone_color(t, p)
}

/// Renders a plugin view tree (arena) with Mandara's components.
/// `emoji` is the colour emoji family for text that is only emoji.
fn render(view: &View, palette: &MandaraPalette, plugin: &Shared, id: &str, views: &ViewSet, emoji: Option<&gpui::SharedString>) -> AnyElement {
    #[allow(clippy::too_many_arguments)]
    fn node(view: &View, i: u32, depth: u32, p: &MandaraPalette, plugin: &Shared, id: &str, views: &ViewSet, emoji: Option<&gpui::SharedString>, key: &mut usize) -> AnyElement {
        let Some(n) = view.nodes.get(i as usize).filter(|_| depth < MAX_DEPTH && *key < MAX_RENDERED_NODES) else { return div().into_any_element() };
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
                if let Some(pl) = plugin.borrow_mut().as_mut() {
                    pl.on_event(&action);
                }
                let id = plugin.borrow().as_ref().map(|p| p.id.clone()).unwrap_or_default();
                after_call(&id, &plugin, cx);
                // The plugin's state changed: re-render its bar, panel and card.
                views.notify(cx);
            })
            .into_any_element()
    }
    let mut key = 0;
    node(view, view.root, 0, palette, plugin, id, views, emoji, &mut key)
}

/// Runs commands a plugin queued (through the documented registry), only
/// those on the plugin allowlist ([`mandara_plugins::wasm::PLUGIN_COMMANDS`]):
/// anything else would act outside the WebAssembly sandbox.
fn run_commands(plugin: &str, lines: Vec<String>, cx: &mut App) {
    if lines.is_empty() {
        return;
    }
    // Extension plugins also need the capability a command stands for: the
    // v0.1 `commands` permission must not get around a revoked grant.
    let extension = loaded(plugin).is_some_and(|p| p.borrow().as_ref().is_some_and(WasmPlugin::is_extension));
    let access = discovered(plugin).map(|d| access_for(&d, cx));
    for line in lines {
        let words: Vec<String> = line.split_whitespace().map(str::to_string).collect();
        let registry = cx.global::<PluginRegistry>().commands.clone();
        match registry.resolve(&words) {
            Ok(r) if extension && mandara_plugins::extension::command_capability(r.spec.path).is_some_and(|c| !access.as_ref().is_some_and(|a| a.allows(&c))) => {
                tracing::warn!(target: "mandara::plugin", "plugin command `{}` refused: it needs the matching capability granted", r.spec.path);
                continue;
            }
            Ok(r) if mandara_plugins::wasm::plugin_may_run(r.spec.path) => {}
            Ok(r) => {
                tracing::warn!(target: "mandara::plugin", "plugin command `{}` refused: plugins may only run interface commands", r.spec.path);
                continue;
            }
            Err(err) => {
                tracing::warn!(target: "mandara::plugin", "plugin command `{line}` failed: {err}");
                continue;
            }
        }
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

impl Surface {
    fn ext(self) -> ExtSurface {
        match self {
            Self::Bar => ExtSurface::Bar,
            Self::Panel => ExtSurface::Panel,
            Self::Sidebar => ExtSurface::Sidebar,
        }
    }
}

thread_local! {
    /// The config snapshot (its address) each plugin's settings and grants
    /// were last read from.
    static FOLLOWED: RefCell<HashMap<String, usize>> = RefCell::new(HashMap::new());
}

/// Keeps a loaded plugin's settings and grants in step with the config:
/// re-read only when the config snapshot changes (its address), so a
/// render costs a pointer compare. A Studio edit or a revoked grant
/// reaches the plugin on its next render.
fn follow_config(declared: &Discovered, plugin: &Shared, cx: &mut App) {
    let generation = std::sync::Arc::as_ptr(&config(cx).read(cx).snapshot()) as usize;
    let instance = Rc::as_ptr(plugin) as usize;
    // Keyed by instance too: a reloaded plugin reads the config afresh.
    let key = generation ^ instance.rotate_left(32);
    if FOLLOWED.with(|f| f.borrow_mut().insert(declared.id.clone(), key)) == Some(key) {
        return;
    }
    let current = settings_for(declared, cx);
    let access = access_for(declared, cx);
    if let Some(p) = plugin.borrow_mut().as_mut() {
        if p.settings() != &current {
            p.set_settings(current);
        }
        p.set_access(access);
    }
}

/// A notice drawn where a plugin's view was refused (too large, …).
fn refused(reason: &str, palette: &MandaraPalette) -> AnyElement {
    tracing::warn!(target: "mandara::plugin", "plugin view refused: {reason}");
    div().text_size(px(text::LABEL)).text_color(palette.urgent_coral.hsla()).child(mandara_i18n::t("plugin view too large")).into_any_element()
}

/// The frame an extension view of `d` draws in.
fn frame_for(d: &Discovered, context: ExtContext, key: String, cx: &App) -> Frame {
    Frame { key: key.into(), context, sink: sink_for(d), images: images_for(&d.id), emoji: cx.try_global::<crate::fonts::Fonts>().and_then(|f| f.emoji.clone()) }
}

/// The extension view of plugin `d` for `context`, drawn; `None` when it
/// has none there (draw the older export instead).
pub(crate) fn draw_extension(d: &Discovered, plugin: &Shared, context: ExtContext, key: String, palette: &MandaraPalette, window: &mut gpui::Window, cx: &mut App) -> Option<AnyElement> {
    let theme = palette_for(palette);
    let result = {
        let mut guard = plugin.borrow_mut();
        let p = guard.as_mut().filter(|p| p.is_extension())?;
        p.extension_view(&theme, &context)
    };
    after_render(&d.id, plugin, cx);
    match result {
        Ok(Some(view)) => Some(ext_view::render(view, palette, &frame_for(d, context, key, cx), window, cx)),
        Ok(None) => None,
        Err(reason) => Some(refused(&reason, palette)),
    }
}

fn surface_view(d: &Discovered, which: Surface, views: ViewSet, window: &mut gpui::Window, cx: &mut App) -> gpui::AnyView {
    let plugin = instance(d, cx);
    let refresh = plugin.borrow_mut().as_mut().map(|p| p.refresh_ms()).unwrap_or(0);
    // The plugin's refresh interval runs only while this surface renders
    // (redraw_in from render), never as a free-running timer.
    let tick = (refresh > 0).then(|| Duration::from_millis(u64::from(refresh).max(MIN_REFRESH_MS)));
    let id = d.id.clone();
    let declared = d.clone();
    let plugin_views = views_of(&d.id);
    fn_view(
        None,
        move |palette, window, cx| {
            views.track(window);
            plugin_views.track(window);
            // Looked up per render (a map hit): after a hot reload an open
            // bar item, panel or card draws the fresh instance at once
            // instead of the one it was created with.
            let plugin = instance(&declared, cx);
            if let Some(every) = tick {
                redraw_in(every, window, cx);
            }
            let theme = palette_for(palette);
            follow_config(&declared, &plugin, cx);
            let bounds = window.viewport_size();
            let context = ExtContext { surface: which.ext(), outlet: String::new(), instance: String::new(), width: f32::from(bounds.width), height: f32::from(bounds.height) };
            if let Some(el) = draw_extension(&declared, &plugin, context, format!("plg-{id}-{}", which.ext() as u8), palette, window, cx) {
                return match which {
                    // A quiet line says whose panel it is, so a plugin's view
                    // can't pass for one of Mandara's own (a sign-in prompt).
                    Surface::Panel => {
                        let name = cx.global::<PluginRegistry>().get(&id).map(|f| f.manifest.display_name()).unwrap_or_else(|| declared.name.clone());
                        let by = div()
                            .flex()
                            .items_center()
                            .gap(px(tokens::XS))
                            .text_size(px(text::CAPTION))
                            .text_color(palette.on_surface_variant.with_alpha(ATTRIBUTION_ALPHA).hsla())
                            .child(icon("plug", text::CAPTION, palette.on_surface_variant.with_alpha(ATTRIBUTION_ALPHA)))
                            .child(name);
                        div().size_full().p(px(PANEL_PAD_PX)).flex().flex_col().gap(px(tokens::SM)).child(div().flex_1().min_h(px(0.0)).child(el)).child(by).into_any_element()
                    }
                    _ => el,
                };
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
            // Defer what it queued: rendering must not mutate the app.
            after_render(&id, &plugin, cx);
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
        repository: d.repository.clone(),
        support: d.support.clone(),
        settings: d.settings.clone(),
    };
    if PRINTS.with(|p| !p.borrow().contains_key(&d.id)) {
        let print = fingerprint(&d);
        PRINTS.with(|p| p.borrow_mut().insert(d.id.clone(), print));
    }
    DISCOVERED.with(|m| m.borrow_mut().insert(d.id.clone(), d.clone()));
    let widgets = d.widgets.clone();
    let mut catalog = wasm::PluginCatalogs::new(&d.dir);
    let plugin_id = d.id.clone();
    r.register(manifest, move |window, cx, panel_cx| {
        // One plugin instance backs all three surfaces: a click in any of
        // them re-renders the others.
        let views = ViewSet::default();
        if panel_cx.is_card() {
            // A sidebar card: one surface (each costs a call into the plugin).
            return PluginInstance { sidebar: Some(surface_view(&d, Surface::Sidebar, views, window, cx)), ..PluginInstance::empty() };
        }
        PluginInstance {
            bar: Some(surface_view(&d, Surface::Bar, views.clone(), window, cx)),
            panel: Some(PanelSpec {
                view: surface_view(&d, Surface::Panel, views.clone(), window, cx),
                size: Rc::new(std::cell::Cell::new(size(px(d.panel.width), px(d.panel.height)))),
            }),
            sidebar: Some(surface_view(&d, Surface::Sidebar, views, window, cx)),
            ..PluginInstance::empty()
        }
    });
    // The card renders a fresh tree each frame and keeps nothing; the
    // plugin's loaded instance goes too when nothing else shows it.
    let release_id = plugin_id.clone();
    r.sidebar_card(&plugin_id, mandara_plugins::CardRelease::shared(move |cx| release_card(&release_id, cx)));
    // Its desktop widgets: cards the card host draws with the plugin's
    // `desktop-widget` view.
    for w in widgets {
        let card = dc::plugin_card(&plugin_id, &w.id);
        let opts = dc::PluginCardOptions { width: w.width, height: w.height, ..dc::PluginCardOptions::default() };
        let name = catalog.translate(&w.name);
        let description = catalog.translate(&w.description);
        let make = move || {
            let mut widget = dc::new_card(&card, mandara_config::desktop_widgets::Anchor::Center);
            if let mandara_config::desktop_widgets::WidgetKind::Card(c) = &mut widget.kind {
                dc::set_options(c, &opts);
            }
            vec![widget]
        };
        r.desktop_widget(mandara_plugins::DesktopWidgetType::new(&plugin_id, &w.id, &name, &description, &w.icon, make).in_menu());
    }
}

thread_local! {
    /// Every registered third-party plugin, by id (the card host finds a
    /// plugin's desktop widget here).
    static DISCOVERED: RefCell<HashMap<String, Discovered>> = RefCell::new(HashMap::new());
}

/// Inner padding of a plugin's desktop widget card.
const DESKTOP_CARD_PAD_PX: f32 = tokens::LG;

/// The view of plugin card `card` (`plugin:<id>/<widget>`) placed as
/// widget `instance`: the plugin's `desktop-widget` view in a glass card of
/// `size` (logical px), re-asked after clicks, setting changes and every
/// `refresh-ms`.
pub fn desktop_card_view(card: &str, placed: String, size: (f32, f32), window: &mut gpui::Window, cx: &mut App) -> Option<gpui::AnyView> {
    let (plugin_id, widget) = dc::parse_plugin_card(card)?;
    let d = DISCOVERED.with(|m| m.borrow().get(plugin_id).cloned())?;
    let widget = widget.to_string();
    let views = ViewSet::default();
    let refresh = instance(&d, cx).borrow_mut().as_mut().map(|p| p.refresh_ms()).unwrap_or(0);
    let tick = (refresh > 0).then(|| Duration::from_millis(u64::from(refresh).max(MIN_REFRESH_MS)));
    let plugin_views = views_of(&d.id);
    Some(fn_view(
        None,
        move |palette, window, cx| {
            views.track(window);
            plugin_views.track(window);
            if let Some(every) = tick {
                redraw_in(every, window, cx);
            }
            let plugin = instance(&d, cx);
            follow_config(&d, &plugin, cx);
            let ext = ExtContext { surface: ExtSurface::Desktop, outlet: widget.clone(), instance: placed.clone(), width: size.0, height: size.1 };
            if let Some(el) = draw_extension(&d, &plugin, ext, format!("plg-{}-desk-{placed}", d.id), palette, window, cx) {
                return div()
                    .size_full()
                    .p(px(crate::cards::SHADOW_PAD_PX))
                    .text_color(palette.on_surface.hsla())
                    .child(crate::cards::now_playing::glass(palette, crate::style::card_radius(), 1.0).size_full().p(px(DESKTOP_CARD_PAD_PX)).flex().flex_col().child(el))
                    .into_any_element();
            }
            let opts: dc::PluginCardOptions = config(cx).read(cx).get().desktop.widgets.get(&placed).and_then(dc::as_card).map(dc::options).unwrap_or_default();
            let context = wasm::WidgetContext {
                widget: widget.clone(),
                instance: placed.clone(),
                width: size.0,
                height: size.1,
                numerals: serde_plain(&opts.numerals),
                calendar: serde_plain(&opts.calendar),
                language: opts.language.clone(),
            };
            let theme = palette_for(palette);
            let view = plugin.borrow_mut().as_mut().and_then(|p| p.desktop_widget(&theme, &context));
            after_render(&d.id, &plugin, cx);
            let emoji = cx.try_global::<crate::fonts::Fonts>().and_then(|f| f.emoji.clone());
            let body = view.map(|v| render(&v, palette, &plugin, &d.id, &views, emoji.as_ref()));
            let _ = window;
            div()
                .size_full()
                .p(px(crate::cards::SHADOW_PAD_PX))
                .text_color(palette.on_surface.hsla())
                .child(crate::cards::now_playing::glass(palette, crate::style::card_radius(), 1.0).size_full().p(px(DESKTOP_CARD_PAD_PX)).flex().flex_col().children(body))
                .into_any_element()
        },
        window,
        cx,
    ))
}

/// A choice's name as in the config (`eastern-arabic`).
fn serde_plain(v: &impl serde::Serialize) -> String {
    serde_json::to_value(v).ok().and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default()
}


// ── Studio tabs ──────────────────────────────────────────────────────────

/// The Studio tabs switched-on third-party plugins add to `page` (one of
/// [`mandara_plugins::extension::STUDIO_TAB_PAGES`]): `(plugin id, plugin
/// name, tab)`, ordered by plugin then declaration.
pub fn studio_tabs(page: &str, cx: &App) -> Vec<(String, String, mandara_plugins::extension::ManifestStudioTab)> {
    let cfg = config(cx).read(cx).get();
    let registry = cx.global::<PluginRegistry>();
    let mut out: Vec<_> = DISCOVERED.with(|m| {
        m.borrow()
            .values()
            .filter(|d| registry.is_enabled(&d.id, &cfg.plugins))
            .flat_map(|d| {
                d.studio_tabs.iter().filter(|t| t.page == page).map(|t| {
                    let mut tab = t.clone();
                    tab.title = translated(d, &t.title);
                    (d.id.clone(), registry.get(&d.id).map(|f| f.manifest.display_name()).unwrap_or_else(|| d.name.clone()), tab)
                })
            })
            .collect()
    });
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// Draws plugin `plugin`'s Studio tab `tab` (`width` logical px wide):
/// its extension view for that outlet, or a short note when it has none.
pub fn studio_tab_view(plugin: &str, tab: &str, width: f32, palette: &MandaraPalette, window: &mut gpui::Window, cx: &mut App) -> AnyElement {
    let Some(d) = discovered(plugin) else { return note(mandara_i18n::t("This plugin isn't installed any more"), palette) };
    views_of(plugin).track(window);
    watch_grants(cx);
    let shared = instance(&d, cx);
    follow_config(&d, &shared, cx);
    let context = ExtContext { surface: ExtSurface::Studio, outlet: tab.to_string(), instance: String::new(), width, height: 0.0 };
    if let Some(el) = draw_extension(&d, &shared, context, format!("plg-{plugin}-studio-{tab}"), palette, window, cx) {
        return el;
    }
    match &*shared.borrow() {
        Some(p) if p.disabled.is_some() => note(mandara_i18n::t("plugin stopped"), palette),
        None => note(mandara_i18n::t("plugin failed"), palette),
        _ => note(mandara_i18n::t("This plugin has nothing to show here"), palette),
    }
}

fn note(text: impl Into<gpui::SharedString>, palette: &MandaraPalette) -> AnyElement {
    div().text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla()).child(text.into()).into_any_element()
}

// ── Launcher providers ───────────────────────────────────────────────────

/// Most plugin providers a plain launcher search asks.
const MAX_GLOBAL_PROVIDERS: usize = 2;
/// Longest word a `plugins run` command passes on.
const MAX_COMMAND_WORD: usize = 256;

/// `plugins run <plugin> <command> [args…]`: runs a command plugin.toml's
/// `[[commands]]` declares (a user gesture for the plugin).
pub fn run_command(plugin: &str, words: &[String], cx: &mut App) -> Result<(), String> {
    let d = discovered(plugin).ok_or_else(|| format!("no plugin `{plugin}`"))?;
    let name = words.first().ok_or("a command name is needed")?;
    if !d.commands.iter().any(|c| c.name == *name) {
        return Err(format!("`{plugin}` has no command `{name}`"));
    }
    if words.len() > 1 + mandara_plugins::extension::MAX_COMMAND_ARGS || words.iter().any(|w| w.len() > MAX_COMMAND_WORD) {
        return Err("too many or too long words".into());
    }
    if !cx.global::<PluginRegistry>().is_enabled(plugin, &config(cx).read(cx).get().plugins) {
        return Err(format!("`{plugin}` is switched off"));
    }
    let shared = instance(&d, cx);
    if shared.borrow().as_ref().is_none_or(|p| !p.is_extension()) {
        return Err(format!("`{plugin}` isn't running"));
    }
    let event = ExtEvent { context: ExtContext { surface: ExtSurface::Launcher, outlet: String::new(), instance: String::new(), width: 0.0, height: 0.0 }, node: String::new(), action: name.clone(), value: ExtValue::Command(words.to_vec()) };
    send_event(plugin, &shared, &event, cx);
    Ok(())
}

/// The commands every switched-on plugin offers: (plugin, command).
pub fn plugin_commands(cx: &App) -> Vec<(String, mandara_plugins::extension::ManifestCommand)> {
    let cfg = config(cx).read(cx).get();
    let registry = cx.global::<PluginRegistry>();
    DISCOVERED.with(|m| m.borrow().values().filter(|d| registry.is_enabled(&d.id, &cfg.plugins)).flat_map(|d| d.commands.iter().map(|c| (d.id.clone(), c.clone()))).collect())
}

/// A launcher provider a third-party plugin declares (`[[launcher]]`).
#[derive(Clone, Debug, PartialEq)]
pub struct PluginProvider {
    pub plugin: String,
    pub id: String,
    pub name: String,
    pub prefix: String,
    pub global: bool,
    pub icon: &'static str,
}

/// The launcher providers of switched-on extension plugins.
pub fn launcher_providers(cx: &App) -> Vec<PluginProvider> {
    let cfg = config(cx).read(cx).get();
    let registry = cx.global::<PluginRegistry>();
    DISCOVERED.with(|m| {
        m.borrow()
            .values()
            .filter(|d| !d.launcher.is_empty() && registry.is_enabled(&d.id, &cfg.plugins))
            .flat_map(|d| {
                d.launcher.iter().map(|l| PluginProvider {
                    plugin: d.id.clone(),
                    id: l.id.clone(),
                    name: translated(d, &l.name),
                    prefix: l.prefix.clone(),
                    global: l.global,
                    icon: crate::icons::ICON_NAMES.iter().find(|x| **x == l.icon).copied().unwrap_or("plug"),
                })
            })
            .collect()
    })
}

/// Plugin launcher results for `query`: `(routed, hits)`. When the query
/// starts with a provider's prefix (the longest wins) only that provider
/// answers and `routed` is true; otherwise providers marked `global` add a
/// few hits to the plain search. Each query runs in the plugin's sandbox
/// under the launcher's time budget.
pub fn launcher_hits(query: &str, cx: &App) -> (bool, Vec<crate::launcher_ext::Hit>) {
    let q = query.trim_start();
    let providers = launcher_providers(cx);
    if providers.is_empty() || q.is_empty() {
        return (false, Vec::new());
    }
    let routed = providers.iter().filter(|p| !p.prefix.is_empty() && q.starts_with(p.prefix.as_str())).max_by_key(|p| p.prefix.len());
    if let Some(p) = routed {
        return (true, query_provider(p, &q[p.prefix.len()..], crate::launcher_ext::PROVIDER_LIMIT, cx));
    }
    // Plain searches go only to providers the person allowed to see them
    // (`launcher.global`), and to a few at most: each keystroke waits.
    let may_see = |p: &&PluginProvider| discovered(&p.plugin).is_some_and(|d| access_for(&d, cx).allows(&mandara_plugins::extension::Capability::LauncherGlobal));
    let hits = providers.iter().filter(|p| p.global).filter(may_see).take(MAX_GLOBAL_PROVIDERS).flat_map(|p| query_provider(p, q, crate::launcher_ext::global_limit(), cx)).collect();
    (false, hits)
}

fn query_provider(p: &PluginProvider, query: &str, limit: usize, cx: &App) -> Vec<crate::launcher_ext::Hit> {
    use crate::launcher_ext::{Hit, HitAction, Lead};
    let Some(d) = discovered(&p.plugin) else { return Vec::new() };
    let plugin = instance(&d, cx);
    let hits = plugin.borrow_mut().as_mut().map(|pl| pl.launcher_query(&p.id, query.trim(), limit)).unwrap_or_default();
    let images = images_for(&p.plugin);
    // Every result says which plugin it comes from, so none can pass for an
    // app or a Mandara action.
    let from = cx.global::<PluginRegistry>().get(&p.plugin).map(|f| f.manifest.display_name()).unwrap_or_else(|| d.name.clone());
    hits.into_iter()
        .map(|h| {
            let lead = match (h.image != 0).then(|| images(h.image, cx)).flatten() {
                Some(path) => Lead::Picture(path),
                None => Lead::Icon(crate::icons::ICON_NAMES.iter().find(|x| **x == h.icon).copied().unwrap_or(p.icon)),
            };
            Hit {
                provider: "plugin",
                key: format!("{}/{}/{}", p.plugin, p.id, h.id),
                label: h.title,
                detail: if h.detail.is_empty() { from.clone() } else { format!("{from} · {}", h.detail) },
                lead,
                badge: mandara_i18n::tk("Plugin"),
                keywords: h.keywords,
                action: HitAction::Command(vec!["plugins".into(), "launch".into(), p.plugin.clone(), p.id.clone(), h.id]),
            }
        })
        .collect()
}

/// `plugins launch <plugin> <provider> <hit>`: a launcher result was chosen.
pub fn launch(plugin: &str, provider: &str, hit: &str, cx: &mut App) -> Result<(), String> {
    let d = discovered(plugin).filter(|d| d.launcher.iter().any(|l| l.id == provider)).ok_or_else(|| format!("no launcher provider {plugin}/{provider}"))?;
    if !cx.global::<PluginRegistry>().is_enabled(plugin, &config(cx).read(cx).get().plugins) {
        return Err(format!("`{plugin}` is switched off"));
    }
    let shared = instance(&d, cx);
    let event = ExtEvent { context: ExtContext { surface: ExtSurface::Launcher, outlet: provider.to_string(), instance: String::new(), width: 0.0, height: 0.0 }, node: String::new(), action: String::new(), value: ExtValue::Launch(hit.to_string()) };
    send_event(plugin, &shared, &event, cx);
    Ok(())
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
    cx.update_global::<PluginRegistry, _>(|r, cx| {
        let registered: Vec<String> = r.manifests().filter(|m| matches!(m.origin, PluginOrigin::ThirdParty { .. })).map(|m| m.id.clone()).collect();
        for id in registered.iter().filter(|id| !found.iter().any(|(d, _)| &d.id == *id)) {
            r.unregister(id);
            forget(id, cx);
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
            forget(&d.id, cx);
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
        if dir.is_dir()
            && let Err(err) = watcher.watch(&dir, notify::RecursiveMode::Recursive) {
                tracing::warn!("watching {}: {err}", dir.display());
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

#[cfg(test)]
mod tests {
    use mandara_plugins::wasm::{PLUGIN_COMMANDS, plugin_may_run};

    /// Issue #10: plugins may only queue interface commands. Every allowed
    /// path is a real built-in command (a typo would silently block it),
    /// and nothing that starts programs or changes settings, startup apps,
    /// compositor config, plugins or the system is among them.
    #[test]
    fn plugins_may_only_run_interface_commands() {
        let mut registry = mandara_plugins::PluginRegistry::default();
        super::super::register_builtin(&mut registry);
        let paths: Vec<&str> = registry.commands.all().iter().map(|c| c.path).collect();
        for allowed in PLUGIN_COMMANDS {
            assert!(paths.contains(allowed), "`{allowed}` isn't a command");
        }
        for risky in ["launcher run", "apps launch", "config set", "hooks add", "plugins install", "updates apply", "session logout", "power off", "screenshot", "record", "compositor dispatch", "keep-awake", "profile apply"] {
            if paths.contains(&risky) {
                assert!(!plugin_may_run(risky), "plugins must not run `{risky}`");
            }
        }
        assert!(plugin_may_run("media play-pause") && !plugin_may_run("config set"));
    }
}
