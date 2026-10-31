//! Host for third-party plugins: Rust code compiled to WebAssembly
//! components (`wasm32-wasip2`) against `wit/plugin.wit`, run in wasmtime.
//!
//! Isolation: WASI is present but empty (no filesystem, network,
//! environment or arguments); every call has a time budget enforced with
//! epoch interruption; and any trap or error disables only that plugin.
//!
//! The epoch ticker thread only ticks while a plugin call is running
//! ([`ActiveCall`]); between calls it is parked and costs nothing.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context as _, Result};
use wasmtime::component::{Component, HasSelf, Linker, ResourceTable};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

wasmtime::component::bindgen!({ world: "plugin", path: "wit" });

pub use mandara::plugin::types::{Kind, Node, Tone};

/// Converts a wasmtime error into an anyhow one with context.
fn w<T>(r: wasmtime::Result<T>, what: &str) -> Result<T> {
    r.map_err(|e| anyhow::anyhow!("{what}: {e:#}"))
}

/// Epoch tick length; a call may run for `CALL_BUDGET_TICKS` ticks.
const TICK_MS: u64 = 10;
const TICK: Duration = Duration::from_millis(TICK_MS);
const CALL_BUDGET_TICKS: u64 = 10;
/// `init` (which may build tables) gets this many call budgets.
const INIT_BUDGET_FACTOR: u64 = 10;
/// Time budget of one export call (`bar`, `panel`, `on-event`, …); a call
/// running past it traps and disables the plugin.
pub const CALL_BUDGET: Duration = Duration::from_millis(TICK_MS * CALL_BUDGET_TICKS);
/// Time budget of `init`.
pub const INIT_BUDGET: Duration = Duration::from_millis(TICK_MS * CALL_BUDGET_TICKS * INIT_BUDGET_FACTOR);
/// Commands a plugin may queue with `host::command` per call.
pub const MAX_QUEUED_COMMANDS: usize = 16;
/// The only permission a manifest can ask for.
pub const PERMISSION_COMMANDS: &str = "commands";

/// The Mandara commands a plugin with [`PERMISSION_COMMANDS`] may queue:
/// interface actions only (open a panel, control media and volume, post a
/// notification, add a note or to-do). Nothing that starts programs, edits
/// settings, startup apps, compositor config or plugins, changes power or
/// the network, or records the screen: those run outside the WebAssembly
/// sandbox, so a plugin could escape it through them. A command's output
/// never goes back to the plugin.
pub const PLUGIN_COMMANDS: &[&str] = &[
    "panel",
    "sidebar",
    "sidebar tab open",
    "launcher",
    "launcher search",
    "switcher",
    "menu",
    "osd",
    "osd show",
    "media next",
    "media previous",
    "media play-pause",
    "media seek",
    "audio volume up",
    "audio volume down",
    "audio volume set",
    "audio mute",
    "audio mic mute",
    "notifications send",
    "notifications dnd",
    "pomodoro",
    "weather refresh",
    "calendar refresh",
    "theme mode",
    "workspace view",
    "workspace toggle",
    "workspace cycle-layout",
    "todo add",
    "todo done",
    "notes add",
    "notes append",
    "clock now",
    "info",
    "ping",
];

/// Whether a plugin may run the command at `path` (the matched command,
/// not the words typed after it).
pub fn plugin_may_run(path: &str) -> bool {
    PLUGIN_COMMANDS.contains(&path)
}

/// Plugin calls currently running (the ticker only runs while > 0).
static ACTIVE_CALLS: AtomicUsize = AtomicUsize::new(0);
/// The epoch ticker thread (unparked when a call starts).
static TICKER: OnceLock<std::thread::Thread> = OnceLock::new();

/// Marks a plugin call in flight for its lifetime, so epoch deadlines
/// advance; the ticker parks again once no call is running.
struct ActiveCall;

impl ActiveCall {
    fn start() -> Self {
        if ACTIVE_CALLS.fetch_add(1, Ordering::AcqRel) == 0
            && let Some(t) = TICKER.get()
        {
            t.unpark();
        }
        ActiveCall
    }
}

impl Drop for ActiveCall {
    fn drop(&mut self) {
        ACTIVE_CALLS.fetch_sub(1, Ordering::AcqRel);
    }
}

/// The shared engine; its epoch ticker thread starts with it. Plugins are
/// compiled on the calling thread (no parallel-compilation pool).
pub(crate) fn engine() -> Result<&'static Engine> {
    static ENGINE: OnceLock<Engine> = OnceLock::new();
    static INIT: std::sync::Mutex<()> = std::sync::Mutex::new(());
    if let Some(engine) = ENGINE.get() {
        return Ok(engine);
    }
    // One engine and one ticker even when plugins load concurrently.
    let _init = INIT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(engine) = ENGINE.get() {
        return Ok(engine);
    }
    let mut config = Config::new();
    config.wasm_component_model(true).epoch_interruption(true);
    let engine = w(Engine::new(&config), "starting the wasmtime engine")?;
    let engine = ENGINE.get_or_init(|| engine);
    let ticker = engine.clone();
    if let Ok(handle) = std::thread::Builder::new().name("mandara-wasm-epoch".into()).spawn(move || {
        loop {
            if ACTIVE_CALLS.load(Ordering::Acquire) == 0 {
                // Spurious wake-ups just re-check the counter.
                std::thread::park();
                continue;
            }
            std::thread::sleep(TICK);
            ticker.increment_epoch();
        }
    }) {
        let _ = TICKER.set(handle.thread().clone());
    }
    Ok(engine)
}

/// Most linear memory one plugin may use: plenty for a widget, and a
/// runaway plugin can't take the shell's memory with it.
pub const MAX_PLUGIN_MEMORY: usize = 64 * 1024 * 1024;
/// WASI's poll interface, as wasmtime-wasi links it.
const WASI_POLL: &str = "wasi:io/poll@0.2.12";

pub struct HostState {
    wasi: WasiCtx,
    limits: wasmtime::StoreLimits,
    table: ResourceTable,
    id: String,
    settings: serde_json::Map<String, serde_json::Value>,
    allow_commands: bool,
    /// Commands the plugin asked to run (executed by the shell afterwards).
    pub queued: Vec<String>,
    i18n: PluginCatalogs,
}

/// A plugin's own translations (`<plugin dir>/locales/<code>.toml`),
/// loaded on first use per language.
pub struct PluginCatalogs {
    dir: PathBuf,
    loaded: std::collections::HashMap<mandara_i18n::Lang, Option<mandara_i18n::Catalog>>,
}

impl PluginCatalogs {
    pub fn new(plugin_dir: &Path) -> Self {
        Self { dir: plugin_dir.join("locales"), loaded: Default::default() }
    }

    /// `text` in the current language, or unchanged.
    pub fn translate(&mut self, text: &str) -> String {
        let lang = mandara_i18n::language();
        if lang == mandara_i18n::Lang::En || text.is_empty() {
            return text.to_string();
        }
        let dir = &self.dir;
        let catalog = self.loaded.entry(lang).or_insert_with(|| {
            let path = dir.join(format!("{}.toml", lang.code()));
            let text = std::fs::read_to_string(&path).ok()?;
            mandara_i18n::Catalog::parse(&text).map_err(|e| tracing::warn!("{}: {e}", path.display())).ok()
        });
        catalog.as_ref().and_then(|c| c.get(text)).unwrap_or(text).to_string()
    }

    /// Translates the labels of a view in place.
    fn translate_view(&mut self, view: &mut View) {
        if mandara_i18n::language() == mandara_i18n::Lang::En {
            return;
        }
        for n in &mut view.nodes {
            if matches!(n.kind, Kind::Text | Kind::Button) {
                n.text = self.translate(&n.text);
            }
        }
    }
}

impl WasiView for HostState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView { ctx: &mut self.wasi, table: &mut self.table }
    }
}

impl mandara::plugin::types::Host for HostState {}

impl mandara::plugin::host::Host for HostState {
    fn log(&mut self, message: String) {
        tracing::info!(target: "mandara::plugin", "[{}] {message}", self.id);
    }

    fn now_unix_ms(&mut self) -> u64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
    }

    fn setting(&mut self, key: String) -> Option<String> {
        self.settings.get(&key).map(|v| v.to_string())
    }

    fn command(&mut self, line: String) -> Result<(), String> {
        if !self.allow_commands {
            return Err("this plugin lacks the \"commands\" permission".into());
        }
        if self.queued.len() >= MAX_QUEUED_COMMANDS {
            return Err("too many queued commands".into());
        }
        self.queued.push(line);
        Ok(())
    }

    fn locale(&mut self) -> String {
        mandara_i18n::language().code().to_string()
    }

    fn translate(&mut self, text: String) -> String {
        self.i18n.translate(&text)
    }
}

/// Instances, tables and table entries one plugin may create (a component
/// is a handful of core instances; generous, but bounded).
const MAX_INSTANCES: usize = 64;
const MAX_TABLES: usize = 64;
const MAX_TABLE_ELEMENTS: usize = 100_000;

/// Plugins can't block: WASI's waits return at once. The time limit (epoch
/// interruption) only stops WebAssembly code, so a sleep or a poll inside
/// the host would park the shell for as long as the plugin liked. A plugin
/// has nothing to wait for (no files, sockets or children), so every
/// pollable counts as ready.
fn non_blocking_waits(linker: &mut Linker<HostState>) -> wasmtime::Result<()> {
    use wasmtime::component::Val;
    linker.allow_shadowing(true);
    let mut poll = linker.instance(WASI_POLL)?;
    poll.func_new("[method]pollable.block", |_, _, _, _| Ok(()))?;
    poll.func_new("poll", |_, _, params, results| {
        let ready = match params.first() {
            Some(Val::List(list)) => list.len(),
            _ => 0,
        };
        if let Some(out) = results.first_mut() {
            *out = Val::List((0..ready).map(|i| Val::U32(i as u32)).collect());
        }
        Ok(())
    })?;
    linker.allow_shadowing(false);
    Ok(())
}

/// Compiles (or loads the cached native code for) a component.
pub(crate) fn load_component(path: &Path) -> Result<Component> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let mut h = DefaultHasher::new();
    bytes.hash(&mut h);
    env!("CARGO_PKG_VERSION").hash(&mut h);
    let cache = mandara_config::paths::cache_dir().join("plugins").join(format!("{:016x}.cwasm", h.finish()));
    if cache.exists() {
        // SAFETY: the file was produced by `precompile_component` with this
        // engine configuration and lives in our own cache directory.
        if let Ok(c) = unsafe { Component::deserialize_file(engine()?, &cache) } {
            return Ok(c);
        }
    }
    let compiled = w(engine()?.precompile_component(&bytes), "compiling plugin")?;
    if let Some(dir) = cache.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(&cache, &compiled);
    // SAFETY: bytes just produced by `precompile_component` above.
    w(unsafe { Component::deserialize(engine()?, &compiled) }, "loading compiled plugin")
}

/// A loaded plugin instance.
pub struct WasmPlugin {
    pub id: String,
    store: Store<HostState>,
    bindings: Plugin,
    /// Set (with the reason) once the plugin trapped or errored; a disabled
    /// plugin is never called again.
    pub disabled: Option<String>,
}

impl WasmPlugin {
    pub fn load(id: &str, plugin_dir: &Path, component: &Path, settings: serde_json::Map<String, serde_json::Value>, allow_commands: bool) -> Result<Self> {
        Self::instantiate(id, plugin_dir, &load_component(component)?, settings, allow_commands)
    }

    /// [`Self::load`] for an already compiled component: links the host,
    /// instantiates it and runs `init`.
    pub(crate) fn instantiate(id: &str, plugin_dir: &Path, component: &Component, settings: serde_json::Map<String, serde_json::Value>, allow_commands: bool) -> Result<Self> {
        let mut linker: Linker<HostState> = Linker::new(engine()?);
        w(wasmtime_wasi::p2::add_to_linker_sync(&mut linker), "linking WASI")?;
        w(non_blocking_waits(&mut linker), "linking WASI")?;
        w(Plugin::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |s| s), "linking host")?;
        let state = HostState {
            // No preopens, env, args or network: an empty sandbox.
            wasi: WasiCtxBuilder::new().build(),
            limits: wasmtime::StoreLimitsBuilder::new().memory_size(MAX_PLUGIN_MEMORY).instances(MAX_INSTANCES).tables(MAX_TABLES).table_elements(MAX_TABLE_ELEMENTS).trap_on_grow_failure(true).build(),
            table: ResourceTable::new(),
            id: id.to_string(),
            settings,
            allow_commands,
            queued: Vec::new(),
            i18n: PluginCatalogs::new(plugin_dir),
        };
        let mut store = Store::new(engine()?, state);
        // Memory and table growth past the limits traps (the plugin is
        // disabled) instead of taking the shell's memory.
        store.limiter(|s| &mut s.limits);
        store.set_epoch_deadline(CALL_BUDGET_TICKS * INIT_BUDGET_FACTOR);
        let bindings = {
            let _active = ActiveCall::start();
            w(Plugin::instantiate(&mut store, component, &linker), "instantiating plugin")?
        };
        let mut plugin = Self { id: id.to_string(), store, bindings, disabled: None };
        plugin.call_within("init", CALL_BUDGET_TICKS * INIT_BUDGET_FACTOR, |b, s| b.call_init(s)).map(|r| r.map_err(|e| anyhow::anyhow!("init failed: {e}"))).unwrap_or(Ok(()))?;
        Ok(plugin)
    }

    /// Runs one export under the time budget; a trap disables the plugin.
    fn call<T>(&mut self, what: &str, f: impl FnOnce(&Plugin, &mut Store<HostState>) -> wasmtime::Result<T>) -> Option<T> {
        self.call_within(what, CALL_BUDGET_TICKS, f)
    }

    /// [`Self::call`] with an explicit budget in epoch ticks.
    fn call_within<T>(&mut self, what: &str, ticks: u64, f: impl FnOnce(&Plugin, &mut Store<HostState>) -> wasmtime::Result<T>) -> Option<T> {
        if self.disabled.is_some() {
            return None;
        }
        self.store.set_epoch_deadline(ticks);
        let result = {
            let _active = ActiveCall::start();
            f(&self.bindings, &mut self.store)
        };
        match result {
            Ok(v) => Some(v),
            Err(err) => {
                let reason = format!("{what}: {err:#}");
                tracing::error!(target: "mandara::plugin", "plugin `{}` disabled — {reason}", self.id);
                self.disabled = Some(reason);
                None
            }
        }
    }

    fn localized(&mut self, view: Option<View>) -> Option<View> {
        let mut view = view?;
        self.store.data_mut().i18n.translate_view(&mut view);
        Some(view)
    }

    pub fn bar(&mut self, theme: &Palette) -> Option<View> {
        let v = self.call("bar", |b, s| b.call_bar(s, theme)).flatten();
        self.localized(v)
    }

    pub fn panel(&mut self, theme: &Palette) -> Option<View> {
        let v = self.call("panel", |b, s| b.call_panel(s, theme)).flatten();
        self.localized(v)
    }

    pub fn sidebar(&mut self, theme: &Palette) -> Option<View> {
        let v = self.call("sidebar", |b, s| b.call_sidebar(s, theme)).flatten();
        self.localized(v)
    }

    pub fn on_event(&mut self, action: &str) {
        self.call("on-event", |b, s| b.call_on_event(s, action));
    }

    /// The settings `host::setting` answers from.
    pub fn settings(&self) -> &serde_json::Map<String, serde_json::Value> {
        &self.store.data().settings
    }

    /// Replaces them (a live edit in Studio reaches the plugin at once).
    pub fn set_settings(&mut self, settings: serde_json::Map<String, serde_json::Value>) {
        self.store.data_mut().settings = settings;
    }

    pub fn refresh_ms(&mut self) -> u32 {
        self.call("refresh-ms", |b, s| b.call_refresh_ms(s)).unwrap_or(0)
    }

    /// Commands queued by the plugin since the last call.
    pub fn take_commands(&mut self) -> Vec<String> {
        std::mem::take(&mut self.store.data_mut().queued)
    }
}

// ── Discovery ────────────────────────────────────────────────────────────

/// Where a discovered plugin came from.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Source {
    /// `$XDG_DATA_HOME/mandara/plugins` — installed (marketplace / manual).
    Installed,
    /// `$XDG_DATA_HOME/mandara/plugins-dev` — your own, in development.
    Development,
    /// `$XDG_DATA_DIRS/mandara/plugins` — distribution packages.
    System,
}

/// A plugin folder with a valid `plugin.toml`.
#[derive(Clone, Debug, serde::Serialize)]
pub struct Discovered {
    pub dir: PathBuf,
    pub source: Source,
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: String,
    pub version: String,
    pub zone: Option<crate::Zone>,
    /// Path of the `.wasm` component (`entry` in the manifest).
    pub entry: PathBuf,
    pub permissions: Vec<String>,
    pub homepage: String,
    /// `[i18n.<code>]` name / description overrides.
    pub translations: std::collections::BTreeMap<String, crate::ManifestText>,
    /// `[[settings]]` declarations (validated).
    pub settings: Vec<crate::ManifestSetting>,
}

/// The contents of a `plugin.toml` (the format documented in PLUGINS.md
/// and Studio → Plugins → Local; [`crate::authoring::EXAMPLE_MANIFEST`]
/// shows every field).
#[derive(Clone, Debug, serde::Deserialize)]
pub struct ManifestFile {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub author: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub zone: Option<crate::Zone>,
    /// The component, relative to the plugin folder.
    #[serde(default = "default_entry")]
    pub entry: String,
    #[serde(default)]
    pub permissions: Vec<String>,
    #[serde(default)]
    pub homepage: String,
    #[serde(default)]
    pub i18n: std::collections::BTreeMap<String, crate::ManifestText>,
    /// As declared (unvalidated).
    #[serde(default)]
    pub settings: Vec<crate::ManifestSetting>,
}

fn default_version() -> String {
    "0.0.0".into()
}

fn default_entry() -> String {
    "plugin.wasm".into()
}

/// Longest accepted plugin id.
pub const MAX_ID_LEN: usize = 64;

/// Plugin ids: 1–64 lowercase ASCII letters, digits, `-` and `_` (the
/// marketplace refuses anything else).
pub fn valid_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= MAX_ID_LEN && id.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

impl ManifestFile {
    /// Parses plugin.toml text.
    pub fn parse(text: &str) -> Result<Self, String> {
        toml::from_str(text).map_err(|e| e.to_string().trim().to_string())
    }

    /// Problems that don't stop the plugin from loading but that its
    /// author should fix (each is also logged when the shell loads it).
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if !valid_id(&self.id) {
            out.push(format!("id `{}` must be 1–{MAX_ID_LEN} lowercase letters, digits, - or _ (the marketplace rejects it)", self.id));
        }
        if self.name.trim().is_empty() {
            out.push("name is empty".into());
        }
        out.extend(self.permissions.iter().filter(|p| p.as_str() != PERMISSION_COMMANDS).map(|p| format!("unknown permission `{p}` (only `{PERMISSION_COMMANDS}` exists)")));
        out.extend(crate::split_settings(self.settings.clone()).1);
        out
    }

    fn into_discovered(self, dir: PathBuf, source: Source) -> Discovered {
        Discovered {
            settings: crate::split_settings(self.settings).0,
            entry: dir.join(&self.entry),
            dir,
            source,
            id: self.id,
            name: self.name,
            description: self.description,
            author: self.author,
            version: self.version,
            zone: self.zone,
            permissions: self.permissions,
            homepage: self.homepage,
            translations: self.i18n,
        }
    }
}

/// The plugin directories, highest precedence first (a development copy
/// shadows an installed one, which shadows a system one).
pub fn plugin_dirs() -> Vec<(PathBuf, Source)> {
    let data = mandara_config::paths::data_dir();
    let mut dirs = vec![(data.join("plugins-dev"), Source::Development), (data.join("plugins"), Source::Installed)];
    let system = std::env::var("XDG_DATA_DIRS").unwrap_or_else(|_| "/usr/local/share:/usr/share".into());
    dirs.extend(system.split(':').filter(|d| !d.is_empty()).map(|d| (PathBuf::from(d).join("mandara/plugins"), Source::System)));
    dirs
}

/// What discovery made of one sub-folder of a plugin directory.
#[derive(Clone, Debug)]
pub enum Inspection {
    /// A plugin (the first with its id). `entry_missing`: its component
    /// isn't built / copied yet, so loading it will fail.
    Found { plugin: Discovered, entry_missing: bool, problems: Vec<String> },
    /// Same id as a plugin in a folder with higher precedence.
    Shadowed { id: String, by: PathBuf },
    /// plugin.toml couldn't be read or parsed.
    Invalid { error: String },
    /// No plugin.toml (not a plugin folder).
    NoManifest,
}

/// One plugin directory and what's in it.
#[derive(Clone, Debug)]
pub struct FolderReport {
    pub dir: PathBuf,
    pub source: Source,
    pub exists: bool,
    /// Sub-folders, sorted by path.
    pub entries: Vec<(PathBuf, Inspection)>,
}

impl FolderReport {
    /// Plugins this folder contributes (shadowed ones excluded).
    pub fn plugin_count(&self) -> usize {
        self.entries.iter().filter(|(_, i)| matches!(i, Inspection::Found { .. })).count()
    }
}

/// Inspects `dirs` in precedence order (first occurrence of an id wins).
pub fn inspect(dirs: &[(PathBuf, Source)]) -> Vec<FolderReport> {
    let mut seen: Vec<(String, PathBuf)> = Vec::new();
    let mut reports = Vec::new();
    for (dir, source) in dirs {
        let mut report = FolderReport { dir: dir.clone(), source: *source, exists: dir.is_dir(), entries: Vec::new() };
        let mut subdirs: Vec<PathBuf> = std::fs::read_dir(dir).map(|e| e.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect()).unwrap_or_default();
        subdirs.sort();
        for plugin_dir in subdirs {
            let manifest = plugin_dir.join("plugin.toml");
            let inspection = if !manifest.is_file() {
                Inspection::NoManifest
            } else {
                match std::fs::read_to_string(&manifest).map_err(|e| e.to_string()).and_then(|t| ManifestFile::parse(&t)) {
                    Err(error) => Inspection::Invalid { error },
                    Ok(file) => match seen.iter().find(|(id, _)| *id == file.id) {
                        Some((id, by)) => Inspection::Shadowed { id: id.clone(), by: by.clone() },
                        None => {
                            seen.push((file.id.clone(), plugin_dir.clone()));
                            let problems = file.problems();
                            let plugin = file.into_discovered(plugin_dir.clone(), *source);
                            Inspection::Found { entry_missing: !plugin.entry.is_file(), plugin, problems }
                        }
                    },
                }
            };
            report.entries.push((plugin_dir, inspection));
        }
        reports.push(report);
    }
    reports
}

/// Every plugin found in [`plugin_dirs`] (first occurrence of an id wins).
pub fn discover() -> Vec<Discovered> {
    let mut out = Vec::new();
    for report in inspect(&plugin_dirs()) {
        for (dir, inspection) in report.entries {
            match inspection {
                Inspection::Found { plugin, problems, .. } => {
                    for p in problems {
                        tracing::warn!("plugin `{}`: {p}", plugin.id);
                    }
                    out.push(plugin);
                }
                Inspection::Invalid { error } => tracing::warn!("skipping plugin {}: {error}", dir.join("plugin.toml").display()),
                Inspection::Shadowed { .. } | Inspection::NoManifest => {}
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_and_shadows_by_precedence() {
        let root = std::env::temp_dir().join(format!("mandara-plugins-wasm-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let write = |dir: &Path, id: &str, name: &str| {
            std::fs::create_dir_all(dir.join(id)).unwrap();
            std::fs::write(dir.join(id).join("plugin.toml"), format!("id = \"{id}\"\nname = \"{name}\"\npermissions = [\"commands\"]\n")).unwrap();
        };
        let (dev, inst, missing) = (root.join("dev"), root.join("inst"), root.join("nope"));
        write(&dev, "moon", "Moon (dev)");
        std::fs::write(dev.join("moon/plugin.wasm"), b"\0asm").unwrap();
        write(&inst, "moon", "Moon");
        write(&inst, "clock2", "Clock 2");
        std::fs::create_dir_all(inst.join("broken")).unwrap();
        std::fs::write(inst.join("broken/plugin.toml"), "not toml [").unwrap();
        std::fs::create_dir_all(inst.join("assets")).unwrap();
        let reports = inspect(&[(dev.clone(), Source::Development), (inst.clone(), Source::Installed), (missing, Source::System)]);
        assert_eq!(reports.len(), 3);
        assert!(reports[0].exists && reports[1].exists && !reports[2].exists);
        assert_eq!((reports[0].plugin_count(), reports[1].plugin_count()), (1, 1));
        let found: Vec<&Discovered> = reports.iter().flat_map(|r| &r.entries).filter_map(|(_, i)| match i {
            Inspection::Found { plugin, .. } => Some(plugin),
            _ => None,
        }).collect();
        let moon = found.iter().find(|d| d.id == "moon").unwrap();
        assert_eq!(moon.source, Source::Development, "dev copy shadows installed");
        assert_eq!(moon.permissions, ["commands"]);
        assert!(moon.entry.ends_with("plugin.wasm"));
        assert!(found.iter().any(|d| d.id == "clock2"));
        let kinds: Vec<&Inspection> = reports[1].entries.iter().map(|(_, i)| i).collect();
        assert!(matches!(kinds[0], Inspection::NoManifest), "assets/ has no plugin.toml");
        assert!(matches!(kinds[1], Inspection::Invalid { .. }));
        assert!(matches!(kinds[2], Inspection::Found { entry_missing: true, .. }), "clock2 has no plugin.wasm");
        assert!(matches!(kinds[3], Inspection::Shadowed { by, .. } if *by == dev.join("moon")));
        assert!(matches!(&reports[0].entries[0].1, Inspection::Found { entry_missing: false, .. }));
        std::fs::remove_dir_all(root).unwrap();
    }

    /// What Studio's "Test load" does: load the example component on a
    /// worker thread and render each surface. Skipped until the example is
    /// built (`cargo build --release --target wasm32-wasip2` in
    /// plugins/examples/moon-phase).
    #[test]
    fn test_loads_the_example_off_thread() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../plugins/examples/moon-phase");
        let entry = dir.join("target/wasm32-wasip2/release/moon_phase.wasm");
        if !entry.is_file() {
            eprintln!("skipped: {} not built", entry.display());
            return;
        }
        let file = ManifestFile::parse(&std::fs::read_to_string(dir.join("plugin.toml")).unwrap()).unwrap();
        let settings = crate::split_settings(file.settings.clone()).0.iter().map(|s| (s.key.clone(), s.default_json())).collect();
        let palette = Palette {
            dark: true,
            primary: "#80cbc4".into(),
            on_primary: "#00201d".into(),
            surface: "#101418".into(),
            on_surface: "#e0e3e8".into(),
            on_surface_variant: "#c0c7cd".into(),
            container: "#1c2024".into(),
            error: "#ffb4ab".into(),
            success: "#7dd3a0".into(),
            warning: "#f5c26b".into(),
        };
        let id = file.id.clone();
        let result = std::thread::spawn(move || {
            let mut p = WasmPlugin::load(&id, &dir, &entry, settings, false).map_err(|e| format!("{e:#}"))?;
            let bar = p.bar(&palette).is_some();
            Ok::<_, String>((bar, p.disabled.clone()))
        })
        .join()
        .unwrap();
        assert_eq!(result, Ok((true, None)));
    }

    /// Issue #10: a hostile plugin can't stall the shell with a sleep (the
    /// time limit only stops WebAssembly code, so host waits return at
    /// once) or take its memory (growth past MAX_PLUGIN_MEMORY traps and
    /// disables only that plugin). Run with MANDARA_HOSTILE_PLUGINS set to a
    /// folder holding `sleep/` and `mem/` builds of the example whose `bar`
    /// sleeps 10 s / allocates 200 MiB.
    #[test]
    fn a_hostile_plugin_cannot_stall_or_exhaust_the_shell() {
        let Some(root) = std::env::var_os("MANDARA_HOSTILE_PLUGINS").map(std::path::PathBuf::from) else {
            eprintln!("skipped: MANDARA_HOSTILE_PLUGINS not set");
            return;
        };
        let palette = Palette { dark: true, primary: "#80cbc4".into(), on_primary: "#00201d".into(), surface: "#101418".into(), on_surface: "#e0e3e8".into(), on_surface_variant: "#c0c7cd".into(), container: "#1c2024".into(), error: "#ffb4ab".into(), success: "#7dd3a0".into(), warning: "#f5c26b".into() };
        let run = |variant: &str| {
            let dir = root.join(variant);
            let entry = dir.join("target/wasm32-wasip2/release/moon_phase.wasm");
            let palette = palette.clone();
            std::thread::spawn(move || {
                let mut p = WasmPlugin::load("moon-phase", &dir, &entry, serde_json::Map::new(), false).map_err(|e| format!("{e:#}"))?;
                let started = std::time::Instant::now();
                let bar = p.bar(&palette).is_some();
                Ok::<_, String>((bar, p.disabled.clone(), started.elapsed()))
            })
            .join()
            .unwrap()
        };
        let (_, _, took) = run("sleep").unwrap();
        assert!(took < Duration::from_secs(2), "a 10 s sleep held the shell for {took:?}");
        let (bar, disabled, _) = run("mem").unwrap();
        assert!(!bar && disabled.is_some(), "a 200 MiB allocation must disable the plugin: {disabled:?}");
    }

    #[test]
    fn manifest_problems() {
        let m = ManifestFile::parse("id = \"Bad Id\"\nname = \"\"\npermissions = [\"commands\", \"network\"]\n[[settings]]\nkey = \"x\"\ntype = \"choice\"\n").unwrap();
        let p = m.problems();
        assert_eq!(p.len(), 4, "{p:?}");
        assert!(p[0].contains("Bad Id") && p[2].contains("network") && p[3].contains("no choices"));
        assert!(ManifestFile::parse("name = \"no id\"").unwrap_err().contains("id"));
        assert!(valid_id("moon-phase_2") && !valid_id("") && !valid_id("a/b") && !valid_id(&"a".repeat(MAX_ID_LEN + 1)));
    }
}
