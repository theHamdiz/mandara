//! **Stash**: floating drag-and-drop shelves (opt-in plugin).
//!
//! A shelf is a small chromatic-glass card on its own layer surface
//! (`mandara-stash`, Overlay layer, sized to the card) that you summon
//! anywhere, move by its handle to any output, resize, and drop anything
//! onto: files and folders (kept by reference), pictures, text and HTML,
//! links and any other data (kept in the stash's own folder). Every item
//! drags back out into any app with the right MIME types, copies, opens
//! and reveals. Shelves never close by themselves and come back after a
//! restart (see [`model`] for what is stored where).
//!
//! Summoning (Wayland only shows a client the pointer over its own
//! surfaces, and Mandara never reads input devices):
//! * shaking the pointer, or a drag, over any Mandara surface: the desktop
//!   (`mandara_core::shm_canvas` feeds the shake detector), the bar,
//!   panels, rails and sidebar (the shell surface, [`edges::shake_hook`])
//!   and shelves themselves;
//! * edge catchers: hairline strips along the edges you enable that take a
//!   drag arriving there ([`edges`]); never on an edge another feature uses
//!   (`mandara_config::stash`);
//! * the bar item, the tray, a keybind and `mandara msg stash …`.
//!
//! Cost: nothing exists while the plugin is off. While on and idle there
//! are no timers and no polling: the shake detector runs on pointer events
//! the surfaces already receive. Tile pictures decode off the UI thread at
//! the tile's physical size into a bounded cache ([`thumbs`]) that a
//! minimized or closed shelf releases, followed by the shared glibc trim.

mod edges;
mod drop_zone;
mod ingest;
pub(crate) mod model;
mod quicklook;
mod shelf;
mod store;
mod thumbs;

use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use gpui::{AnyElement, App, AppContext as _, Context, Entity, ExternalData, ExternalPaths, Global, InteractiveElement, IntoElement, ParentElement, Point, Pixels, StatefulInteractiveElement, Styled, WindowHandle, div, prelude::FluentBuilder as _, px};
use mandara_config::stash::{self as cfg_stash, MinimizeTo, ScreenEdge, ShelfView, StashSettings};
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_core::ToGpui as _;
use mandara_theme::MandaraPalette;
use serde_json::json;

pub use edges::{catchers, shake_hook};
use model::{AddArg, Item, ItemKind, Shelf, StashState, Target};

pub const ID: &str = cfg_stash::STASH_PLUGIN_ID;

/// Where a shelf came from (its entrance and whether it may retract).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Origin {
    /// Bar, tray, keybind or command.
    Asked,
    /// A shake of the pointer.
    Shake,
    /// A drag reached an edge catcher: the shelf retracts if the drag
    /// leaves without dropping into it.
    Edge(ScreenEdge),
}

/// What a shelf shows that is not stored: selection, animations, the open
/// menu or dialog, a pending drop.
#[derive(Default)]
pub struct ShelfUi {
    pub selected: Vec<u64>,
    /// Items landing: when, and the window-local point they fly in from.
    pub landing: HashMap<u64, (Instant, Option<Point<Pixels>>)>,
    /// Items leaving with a poof: tile centres, when.
    pub poof: Option<(Instant, Vec<(f32, f32)>)>,
    pub opened: Option<Instant>,
    pub origin: Option<Origin>,
    /// Drops still being read.
    pub busy: u32,
    /// The close dialog is up.
    pub confirm_close: bool,
    /// A tile's menu: item and window-local position.
    pub menu: Option<(u64, Point<Pixels>)>,
    /// The header's menu.
    pub shelf_menu: bool,
    /// A drag is over the shelf.
    pub drag_over: bool,
    /// Last error to show (a drop that could not be kept).
    pub note: Option<String>,
    /// Retract pending for an edge-born shelf whose drag left.
    pub retract: Option<gpui::Task<()>>,
    /// The scale its window last drew at (to release rasterized icons).
    pub scale: f32,
}

/// Stash's runtime: the stored state, each shelf's window and view state,
/// tile pictures and the save in flight.
#[derive(Default)]
pub struct Stash {
    pub(crate) state: StashState,
    loaded: bool,
    running: bool,
    pub(crate) ui: HashMap<u64, ShelfUi>,
    windows: HashMap<u64, WindowHandle<shelf::ShelfWindow>>,
    pub(crate) thumbs: thumbs::Thumbs,
    /// File-type icons resolved off the UI thread, by item.
    pub(crate) icons: HashMap<u64, Option<PathBuf>>,
    saving: bool,
    dirty: bool,
    /// The open shelves alone are over the cache limit (bytes held).
    pub(crate) over_limit: Option<u64>,
    /// An outbound drag of these items is under way (shelf, items).
    pub(crate) drag_out: Option<(u64, Vec<u64>)>,
    quicklook: Option<quicklook::Handle>,
    /// Edges whose catcher is on but inactive (another feature uses the
    /// edge), already reported.
    blocked: [bool; 4],
}

struct GlobalStash(Entity<Stash>);
impl Global for GlobalStash {}

/// The Stash entity (created empty on first use; it holds data only while
/// the plugin runs).
pub fn entity(cx: &mut App) -> Entity<Stash> {
    if let Some(g) = cx.try_global::<GlobalStash>() {
        return g.0.clone();
    }
    let e = cx.new(|_| Stash::default());
    cx.set_global(GlobalStash(e.clone()));
    e
}

fn entity_ref(cx: &App) -> Option<Entity<Stash>> {
    cx.try_global::<GlobalStash>().map(|g| g.0.clone())
}

/// Whether Stash runs (on, and started).
pub fn running(cx: &App) -> bool {
    entity_ref(cx).is_some_and(|e| e.read(cx).running)
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// The resolved settings of the live config.
pub fn settings(cx: &App) -> StashSettings {
    StashSettings::from_config(crate::state::config(cx).read(cx).get())
}

fn reduce_motion() -> bool {
    mandara_core::spring::motion_profile().reduce
}

// ── Lifecycle ────────────────────────────────────────────────────────────

/// Starts Stash (plugin switched on): loads the saved shelves off the UI
/// thread and opens their windows; sets the shake level.
pub fn start(cx: &mut App) {
    let e = entity(cx);
    if e.read(cx).running {
        apply_settings(cx);
        return;
    }
    e.update(cx, |s, _| s.running = true);
    apply_settings(cx);
    let task = cx.background_executor().spawn(async move { store::load() });
    cx.spawn(async move |cx| {
        let state = task.await;
        cx.update(|cx| {
            let e = entity(cx);
            if !e.read(cx).running {
                return;
            }
            e.update(cx, |s, cx| {
                s.state = state;
                s.loaded = true;
                cx.notify();
            });
            resolve_icons(None, cx);
            reconcile(cx);
            enforce_cache(cx);
            apply_settings(cx);
        });
    })
    .detach();
}

/// Stops Stash (plugin switched off): saves, closes every window and
/// releases everything it held; the shelves stay on disk for next time.
pub fn stop(cx: &mut App) {
    let Some(e) = entity_ref(cx) else { return };
    if !e.read(cx).running {
        return;
    }
    mandara_core::shake::set_level(mandara_core::shake::ShakeSensitivity::Off);
    let (windows, json) = e.update(cx, |s, _| {
        let json = s.loaded.then(|| serde_json::to_string(&s.state).ok()).flatten();
        let windows: Vec<_> = s.windows.drain().map(|(_, w)| w).collect();
        (windows, json)
    });
    cx.defer(move |cx| {
        for w in windows {
            let _ = w.update(cx, |_, window, _| window.remove_window());
        }
    });
    quicklook::close(cx);
    edges::reset();
    let mut images = Vec::new();
    e.update(cx, |s, cx| {
        s.thumbs.release_all(|i| images.push(i));
        *s = Stash::default();
        cx.notify();
    });
    for image in images {
        cx.drop_image(image, None);
    }
    if let Some(json) = json {
        cx.background_executor()
            .spawn(async move {
                if let Err(e) = store::save(&json) {
                    tracing::warn!("stash: saving: {e:#}");
                }
                mandara_services::memory::request_trim();
            })
            .detach();
    }
}

/// Follows the config: the shake level, and a catcher that an edge
/// feature now blocks is reported once (never switched off behind your back).
pub fn apply_settings(cx: &mut App) {
    use mandara_core::shake::ShakeSensitivity;
    let s = settings(cx);
    let on = running(cx);
    let level = if on { ShakeSensitivity::from_u8(s.shake) } else { ShakeSensitivity::Off };
    mandara_core::shake::set_level(level);
    // Reported once the shell is up (the load that follows start-up runs
    // this again).
    if !on || cx.try_global::<crate::hub::GlobalHub>().is_none() {
        return;
    }
    let states = cfg_stash::catcher_states(crate::state::config(cx).read(cx).get(), edge_context(cx));
    let Some(e) = entity_ref(cx) else { return };
    let fresh: Vec<(ScreenEdge, &'static str)> = e.update(cx, |st, _| {
        let mut fresh = Vec::new();
        for edge in ScreenEdge::ALL {
            let blocked = match &states[edge.index()] {
                cfg_stash::CatcherState::Blocked { by } => Some(by.first().map_or("", |u| u.reason)),
                _ => None,
            };
            if let Some(reason) = blocked
                && !st.blocked[edge.index()]
            {
                fresh.push((edge, reason));
            }
            st.blocked[edge.index()] = blocked.is_some();
        }
        fresh
    });
    for (edge, reason) in fresh {
        tracing::warn!("stash: the {} edge catcher is inactive: {reason}", edge.id());
        crate::plugins::notifications::send(
            cx,
            mandara_services::NotificationCommand::Post {
                app: t("Stash").into(),
                icon: "stash".into(),
                summary: tf("{edge}: the edge catcher is paused", &[("edge", &mandara_i18n::tr(edge.label()))]),
                body: tf("{reason}. Choose which one keeps the edge in Studio → Plugins → Stash.", &[("reason", &mandara_i18n::tr(reason))]),
            },
        );
    }
}

// ── Changes ──────────────────────────────────────────────────────────────

/// Runs `f` on the stash, then saves and brings the windows in line.
fn change<R>(cx: &mut App, f: impl FnOnce(&mut Stash, &mut Context<Stash>) -> R) -> Option<R> {
    let e = entity_ref(cx)?;
    if !e.read(cx).running || !e.read(cx).loaded {
        return None;
    }
    let r = e.update(cx, |s, cx| {
        let r = f(s, cx);
        s.dirty = true;
        cx.notify();
        r
    });
    reconcile(cx);
    save_soon(cx);
    Some(r)
}

/// Writes the state off the UI thread; changes during a write are written
/// right after it (one write in flight).
fn save_soon(cx: &mut App) {
    let Some(e) = entity_ref(cx) else { return };
    let json = e.update(cx, |s, _| {
        if s.saving || !s.dirty {
            return None;
        }
        s.saving = true;
        s.dirty = false;
        serde_json::to_string(&s.state).ok()
    });
    let Some(json) = json else { return };
    let task = cx.background_executor().spawn(async move { store::save(&json) });
    cx.spawn(async move |cx| {
        if let Err(err) = task.await {
            tracing::warn!("stash: saving: {err:#}");
        }
        cx.update(|cx| {
            if let Some(e) = entity_ref(cx) {
                e.update(cx, |s, _| s.saving = false);
            }
            save_soon(cx);
        });
    })
    .detach();
}

/// Opens windows for shown shelves and closes the others'.
fn reconcile(cx: &mut App) {
    let Some(e) = entity_ref(cx) else { return };
    let (open, close): (Vec<u64>, Vec<WindowHandle<shelf::ShelfWindow>>) = e.update(cx, |s, _| {
        let alive: Vec<u64> = s.windows.keys().copied().collect();
        let mut close = Vec::new();
        for id in alive {
            if !s.state.shelf(id).is_some_and(|sh| !sh.minimized) {
                close.extend(s.windows.remove(&id));
            }
        }
        let open = s.state.shelves.iter().filter(|sh| !sh.minimized && !s.windows.contains_key(&sh.id)).map(|sh| sh.id).collect();
        (open, close)
    });
    // Deferred: a shelf's own button (minimize, close) closes its window.
    if !close.is_empty() {
        cx.defer(move |cx| {
            for w in close {
                let _ = w.update(cx, |_, window, _| window.remove_window());
            }
        });
    }
    for id in open {
        if let Some(handle) = shelf::open(id, cx) {
            e.update(cx, |s, _| {
                s.windows.insert(id, handle);
            });
        }
    }
}

/// Reopens a shelf's window (moved to another output).
fn reopen(id: u64, cx: &mut App) {
    let Some(e) = entity_ref(cx) else { return };
    let old = e.update(cx, |s, _| s.windows.remove(&id));
    if let Some(w) = old {
        cx.defer(move |cx| {
            let _ = w.update(cx, |_, window, _| window.remove_window());
        });
    }
    reconcile(cx);
}

/// Creates a shelf centred on `at` (output-local) of `output` (the focused
/// one when `None`), or in the middle.
pub fn new_shelf(output: Option<String>, at: Option<(f32, f32)>, origin: Origin, cx: &mut App) -> Option<u64> {
    let s = settings(cx);
    let output = output.or_else(|| crate::outputs::focused_monitor(cx)).or_else(|| outputs(cx).first().map(|o| o.name.clone()));
    let (ow, oh) = output.as_deref().map_or((1920.0, 1080.0), |o| output_size(o, cx));
    let (w, h) = (s.default_width.min(ow - 16.0).max(cfg_stash::SHELF_MIN_W), s.default_height.min(oh - 16.0).max(cfg_stash::SHELF_MIN_H));
    let (x, y) = match (origin, at) {
        (Origin::Edge(edge), Some((ax, ay))) => cfg_stash::shelf_at_edge(edge, if edge.is_horizontal() { ax } else { ay }, ow, oh, w, h, EDGE_INSET),
        (_, Some((ax, ay))) => cfg_stash::snap_shelf(ax - w / 2.0, ay - h / 2.0, w, h, ow, oh, 0.0, EDGE_INSET),
        (_, None) => ((ow - w) / 2.0, (oh - h) / 2.0),
    };
    let view = s.default_view;
    change(cx, move |st, _| {
        let id = st.state.take_id();
        st.state.shelves.push(Shelf::new(id, now_secs(), output, (x, y), (w, h), view));
        st.ui.insert(id, ShelfUi { opened: Some(Instant::now()), origin: Some(origin), ..ShelfUi::default() });
        id
    })
}

/// Space kept between a shelf and the screen edge it springs from or snaps to.
pub const EDGE_INSET: f32 = 8.0;

/// A shake on `output` at `at` (desktop, bar, panels…): a new shelf there.
pub fn shake_at(output: &str, at: (f32, f32), cx: &mut App) {
    // One shake, one shelf: the shelf it opens lands under the pointer, and
    // that surface's own detector must not count the same gesture again.
    const ONE_SHAKE: std::time::Duration = std::time::Duration::from_millis(1200);
    thread_local! {
        static LAST: std::cell::Cell<Option<Instant>> = const { std::cell::Cell::new(None) };
    }
    let now = Instant::now();
    if !running(cx) || LAST.get().is_some_and(|t| now.duration_since(t) < ONE_SHAKE) {
        return;
    }
    LAST.set(Some(now));
    tracing::info!("stash: shake on {output} at {:.0},{:.0}", at.0, at.1);
    new_shelf(Some(output.to_string()), Some(at), Origin::Shake, cx);
}

/// Adds items to `shelf`, flying in from `from` (window-local).
fn add_items(shelf: u64, kinds: Vec<ItemKind>, from: Option<Point<Pixels>>, cx: &mut App) {
    if kinds.is_empty() {
        return;
    }
    let ids = change(cx, move |s, _| {
        let now = Instant::now();
        let added = now_secs();
        let mut ids = Vec::with_capacity(kinds.len());
        let mut fresh = Vec::with_capacity(kinds.len());
        for kind in kinds {
            let id = s.state.take_id();
            fresh.push(Item { id, added, kind });
            ids.push(id);
        }
        let Some(sh) = s.state.shelf_mut(shelf) else { return Vec::new() };
        sh.items.extend(fresh);
        let ui = s.ui.entry(shelf).or_default();
        for id in &ids {
            ui.landing.insert(*id, (now, from));
        }
        ui.retract = None;
        ui.origin = ui.origin.filter(|o| !matches!(o, Origin::Edge(_)));
        ids
    })
    .unwrap_or_default();
    resolve_icons(Some(ids), cx);
    enforce_cache(cx);
}

/// Resolves the file-type icons of `items` (all when `None`) off the UI
/// thread.
fn resolve_icons(items: Option<Vec<u64>>, cx: &mut App) {
    let Some(e) = entity_ref(cx) else { return };
    let wanted: Vec<(u64, Vec<String>)> = {
        let s = e.read(cx);
        s.state
            .shelves
            .iter()
            .flat_map(|sh| sh.items.iter())
            .filter(|i| items.as_ref().is_none_or(|ids| ids.contains(&i.id)) && !s.icons.contains_key(&i.id))
            .filter_map(|i| match &i.kind {
                ItemKind::File { dir, mime, .. } => Some((i.id, icon_names(*dir, mime))),
                ItemKind::Data { mime, .. } => Some((i.id, icon_names(false, mime))),
                _ => None,
            })
            .collect()
    };
    if wanted.is_empty() {
        return;
    }
    let task = cx.background_executor().spawn(async move { wanted.into_iter().map(|(id, names)| (id, names.iter().find_map(|n| crate::icon_theme::resolve(n, None, false)).map(|r| r.path))).collect::<Vec<_>>() });
    cx.spawn(async move |cx| {
        let found = task.await;
        cx.update(|cx| {
            if let Some(e) = entity_ref(cx) {
                e.update(cx, |s, cx| {
                    if s.running {
                        s.icons.extend(found);
                        cx.notify();
                    }
                });
            }
        });
    })
    .detach();
}

/// Icon theme names for a MIME type, most specific first.
fn icon_names(dir: bool, mime: &str) -> Vec<String> {
    if dir {
        return vec!["folder".into(), "inode-directory".into()];
    }
    let major = mime.split('/').next().unwrap_or("application");
    let mut names = vec![mime.replace('/', "-")];
    if let Some(generic) = match major {
        "image" | "video" | "audio" | "text" | "font" | "model" => Some(format!("{major}-x-generic")),
        _ => None,
    } {
        names.push(generic);
    }
    names.push("application-x-generic".into());
    names.push("text-x-generic".into());
    names
}

/// Files dropped on `shelf` at `at` (window-local).
pub fn drop_paths(shelf: u64, paths: &ExternalPaths, at: Option<Point<Pixels>>, cx: &mut App) {
    let paths = paths.paths().to_vec();
    if paths.is_empty() || !running(cx) {
        return;
    }
    let copy = settings(cx).copy_images;
    let dir = store::shelf_dir(shelf);
    busy(shelf, 1, cx);
    let task = cx.background_executor().spawn(async move { ingest::from_paths(&dir, &paths, copy) });
    cx.spawn(async move |cx| {
        let kinds = task.await;
        cx.update(|cx| {
            busy(shelf, -1, cx);
            add_items(shelf, kinds, at, cx);
        });
    })
    .detach();
}

/// Data (not files) dropped on `shelf`: received now, read off the UI
/// thread; the drag's transfer stays open until the read is done.
pub fn drop_data(shelf: u64, data: &ExternalData, at: Option<Point<Pixels>>, cx: &mut App) {
    if !running(cx) {
        return;
    }
    let Some(plan) = model::plan_drop(data.mime_types()) else {
        note(shelf, t("Nothing in that drop can be kept").to_string(), cx);
        return;
    };
    let streams: Vec<(String, Box<dyn std::io::Read + Send>)> = ingest::plan_types(&plan).into_iter().filter_map(|m| data.receive(&m).map(|r| (m, r))).collect();
    if streams.is_empty() {
        note(shelf, t("The drag ended before its data arrived").to_string(), cx);
        return;
    }
    let keep_open = data.clone();
    let copy = settings(cx).copy_images;
    let dir = store::shelf_dir(shelf);
    busy(shelf, 1, cx);
    tracing::info!("stash: data drop {:?} ({} types offered)", plan, data.mime_types().len());
    let task = cx.background_executor().spawn(async move { ingest::from_streams(&dir, &plan, streams, copy) });
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            // Finishes the transfer (on the UI thread, where it belongs).
            drop(keep_open);
            busy(shelf, -1, cx);
            match result {
                Ok(kinds) => add_items(shelf, kinds, at, cx),
                Err(e) => note(shelf, tf("Couldn't keep that drop: {error}", &[("error", &e)]), cx),
            }
        });
    })
    .detach();
}

fn busy(shelf: u64, delta: i32, cx: &mut App) {
    if let Some(e) = entity_ref(cx) {
        e.update(cx, |s, cx| {
            let ui = s.ui.entry(shelf).or_default();
            ui.busy = ui.busy.saturating_add_signed(delta);
            cx.notify();
        });
    }
}

fn note(shelf: u64, text: String, cx: &mut App) {
    tracing::info!("stash: shelf {shelf}: {text}");
    if let Some(e) = entity_ref(cx) {
        e.update(cx, |s, cx| {
            s.ui.entry(shelf).or_default().note = Some(text);
            cx.notify();
        });
    }
}

/// Removes items (their stash files too).
pub fn remove_items(shelf: u64, ids: &[u64], cx: &mut App) {
    let ids = ids.to_vec();
    let mut images = Vec::new();
    let blobs = change(cx, |s, _| {
        let Some(sh) = s.state.shelf_mut(shelf) else { return Vec::new() };
        let mut blobs = Vec::new();
        sh.items.retain(|i| {
            let gone = ids.contains(&i.id);
            if gone {
                blobs.extend(i.kind.blobs().map(str::to_string));
            }
            !gone
        });
        for id in &ids {
            s.thumbs.release_item(*id, |i| images.push(i));
            s.icons.remove(id);
        }
        if let Some(ui) = s.ui.get_mut(&shelf) {
            ui.selected.retain(|i| !ids.contains(i));
            ui.menu = None;
        }
        blobs
    })
    .unwrap_or_default();
    for image in images {
        cx.drop_image(image, None);
    }
    if !blobs.is_empty() {
        let dir = store::shelf_dir(shelf);
        cx.background_executor().spawn(async move { blobs.iter().for_each(|b| store::remove_blob(&dir, b)) }).detach();
    }
}

/// Empties a shelf with a poof (it stays open).
pub fn clear(shelf: u64, centres: Vec<(f32, f32)>, cx: &mut App) {
    let ids: Vec<u64> = entity_ref(cx).and_then(|e| e.read(cx).state.shelf(shelf).map(|s| s.items.iter().map(|i| i.id).collect())).unwrap_or_default();
    if ids.is_empty() {
        return;
    }
    remove_items(shelf, &ids, cx);
    if !reduce_motion() {
        change(cx, |s, _| {
            s.ui.entry(shelf).or_default().poof = Some((Instant::now(), centres));
        });
    }
}

/// Closes a shelf: restorable with its items (`stash restore`) until the
/// cache limit evicts it, or forgotten at once with `empty`.
pub fn close(shelf: u64, empty: bool, cx: &mut App) {
    release_icons(shelf, cx);
    let mut images = Vec::new();
    let gone = change(cx, |s, _| {
        s.thumbs.release_shelf(shelf, |i| images.push(i));
        s.ui.remove(&shelf);
        s.state.close(shelf, empty, now_secs()).map(|(_, gone)| gone).unwrap_or_default()
    })
    .unwrap_or_default();
    for image in images {
        cx.drop_image(image, None);
    }
    if !gone.is_empty() {
        cx.background_executor().spawn(async move { gone.iter().for_each(|id| store::remove_shelf_dir(*id)) }).detach();
    }
    mandara_services::memory::request_trim();
}

/// The rasterized file-type icons and SVG pictures a shelf drew go (GPUI's
/// shared icon cache keeps them otherwise).
fn release_icons(shelf: u64, cx: &mut App) {
    let Some(e) = entity_ref(cx) else { return };
    let (paths, scale): (Vec<PathBuf>, f32) = {
        let s = e.read(cx);
        let Some(sh) = s.state.shelf(shelf) else { return };
        let scale = s.ui.get(&shelf).map_or(1.0, |u| u.scale).max(1.0);
        let paths = sh
            .items
            .iter()
            .filter_map(|i| match &i.kind {
                ItemKind::File { mime, path, .. } if mime == "image/svg+xml" => Some(path.clone()),
                ItemKind::Image { blob, mime, .. } if mime == "image/svg+xml" => Some(store::blob_path(shelf, blob)),
                _ => s.icons.get(&i.id).cloned().flatten(),
            })
            .collect();
        (paths, scale)
    };
    for path in &paths {
        for size in shelf::ICON_SIZES {
            crate::icon_raster::forget_cached(path, size, scale, cx);
        }
    }
}

/// Minimizes a shelf (to the bar item and / or the tray) and releases its
/// pictures.
pub fn minimize(shelf: u64, cx: &mut App) {
    release_icons(shelf, cx);
    let mut images = Vec::new();
    let freed = change(cx, |s, _| {
        if let Some(sh) = s.state.shelf_mut(shelf) {
            sh.minimized = true;
        }
        if let Some(ui) = s.ui.get_mut(&shelf) {
            ui.menu = None;
            ui.shelf_menu = false;
            ui.landing.clear();
            ui.landing.shrink_to_fit();
        }
        s.thumbs.release_shelf(shelf, |i| images.push(i))
    })
    .unwrap_or(false);
    for image in images {
        cx.drop_image(image, None);
    }
    if freed {
        mandara_services::memory::request_trim();
    }
}

/// Shows a minimized shelf again.
pub fn restore_shelf(shelf: u64, cx: &mut App) {
    change(cx, |s, _| {
        if let Some(sh) = s.state.shelf_mut(shelf) {
            sh.minimized = false;
        }
        s.ui.entry(shelf).or_default().opened = Some(Instant::now());
    });
}

pub fn set_view(shelf: u64, view: ShelfView, cx: &mut App) {
    change(cx, |s, _| {
        if let Some(sh) = s.state.shelf_mut(shelf) {
            sh.view = view;
        }
    });
}

/// Where a shelf now sits (after a move or resize).
pub fn set_geometry(shelf: u64, output: Option<String>, (x, y): (f32, f32), (w, h): (f32, f32), cx: &mut App) {
    let moved_output = change(cx, |s, _| {
        let Some(sh) = s.state.shelf_mut(shelf) else { return false };
        let moved = output.is_some() && output != sh.output;
        if moved {
            sh.output = output;
        }
        (sh.x, sh.y, sh.w, sh.h) = (x, y, w, h);
        moved
    })
    .unwrap_or(false);
    if moved_output {
        reopen(shelf, cx);
    }
}

/// The newest shelf to drop into, made when there is none.
pub fn target_shelf(cx: &mut App) -> Option<u64> {
    let latest = entity_ref(cx).and_then(|e| e.read(cx).state.latest());
    match latest {
        Some(id) => {
            if entity_ref(cx).and_then(|e| e.read(cx).state.shelf(id).map(|s| s.minimized)).unwrap_or(false) {
                restore_shelf(id, cx);
            }
            Some(id)
        }
        None => new_shelf(None, None, Origin::Asked, cx),
    }
}

/// Copies items to the clipboard (off the UI thread).
pub fn copy(shelf: u64, ids: &[u64], cx: &mut App) {
    let Some(e) = entity_ref(cx) else { return };
    let offers = {
        let s = e.read(cx);
        let Some(sh) = s.state.shelf(shelf) else { return };
        let items: Vec<&Item> = sh.items.iter().filter(|i| ids.contains(&i.id)).collect();
        model::drag_offers(&items, |b| store::blob_path(shelf, b))
    };
    let files: Vec<String> = offers.iter().find(|(m, _)| m == model::URI_LIST).and_then(|(_, c)| if let model::OfferContent::Text(t) = c { Some(t.lines().map(str::to_string).collect()) } else { None }).unwrap_or_default();
    cx.background_executor()
        .spawn(async move {
            let mut sources: Vec<(Vec<u8>, Option<String>)> = Vec::new();
            for (mime, content) in offers {
                let bytes = match content {
                    model::OfferContent::Text(t) => t.into_bytes(),
                    model::OfferContent::File(p) => match std::fs::read(&p) {
                        Ok(b) => b,
                        Err(e) => {
                            tracing::warn!("stash: copying {}: {e}", p.display());
                            continue;
                        }
                    },
                };
                sources.push((bytes, Some(mime)));
            }
            if !files.is_empty() && files.iter().all(|f| f.starts_with("file://")) {
                sources.push((format!("copy\n{}", files.join("\n")).into_bytes(), Some("x-special/gnome-copied-files".into())));
            }
            if let Err(e) = mandara_services::clipboard::serve_all(sources) {
                tracing::warn!("stash: copy: {e}");
            }
        })
        .detach();
}

/// Opens an item with its app (a link in the browser).
pub fn open_item(item: &Item, shelf: u64, cx: &mut App) {
    match &item.kind {
        ItemKind::File { path, .. } => crate::opener::open_path(path, cx),
        ItemKind::Image { blob, .. } | ItemKind::Text { blob, .. } | ItemKind::Data { blob, .. } => crate::opener::open_path(&store::blob_path(shelf, blob), cx),
        ItemKind::Url { url, .. } => {
            let url = url.clone();
            cx.background_executor()
                .spawn(async move {
                    if let Err(e) = mandara_services::session::open_uri(&url) {
                        tracing::warn!("stash: opening {url}: {e}");
                    }
                })
                .detach();
        }
    }
}

/// Shows an item's folder in the file manager.
pub fn reveal(item: &Item, shelf: u64, cx: &mut App) {
    let path = match &item.kind {
        ItemKind::File { path, .. } => path.clone(),
        ItemKind::Image { blob, .. } | ItemKind::Text { blob, .. } | ItemKind::Data { blob, .. } => store::blob_path(shelf, blob),
        ItemKind::Url { .. } => return,
    };
    if let Some(dir) = path.parent() {
        crate::opener::open_path(dir, cx);
    }
}

/// Fetches the picture a link points at (HTTPS, size-capped) and turns the
/// link into a picture item.
pub fn fetch_picture(shelf: u64, item: u64, cx: &mut App) {
    let Some(url) = entity_ref(cx).and_then(|e| e.read(cx).state.shelf(shelf).and_then(|s| s.item(item)).and_then(|i| if let ItemKind::Url { url, .. } = &i.kind { Some(url.clone()) } else { None })) else { return };
    let dir = store::shelf_dir(shelf);
    busy(shelf, 1, cx);
    let task = cx.background_executor().spawn(async move { ingest::fetch_picture(&dir, &url) });
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            busy(shelf, -1, cx);
            match result {
                Ok(kind) => {
                    change(cx, |s, _| {
                        if let Some(i) = s.state.shelf_mut(shelf).and_then(|sh| sh.items.iter_mut().find(|i| i.id == item)) {
                            i.kind = kind;
                        }
                    });
                }
                Err(e) => note(shelf, tf("Couldn't fetch the picture: {error}", &[("error", &e)]), cx),
            }
        });
    })
    .detach();
}

/// Fetches a link's site icon (HTTPS, on your click).
pub fn fetch_icon(shelf: u64, item: u64, cx: &mut App) {
    let Some(url) = entity_ref(cx).and_then(|e| e.read(cx).state.shelf(shelf).and_then(|s| s.item(item)).and_then(|i| if let ItemKind::Url { url, .. } = &i.kind { Some(url.clone()) } else { None })) else { return };
    let dir = store::shelf_dir(shelf);
    busy(shelf, 1, cx);
    let task = cx.background_executor().spawn(async move { ingest::fetch_icon(&dir, &url) });
    cx.spawn(async move |cx| {
        let result = task.await;
        cx.update(|cx| {
            busy(shelf, -1, cx);
            match result {
                Ok(blob) => {
                    change(cx, |s, _| {
                        if let Some(ItemKind::Url { icon, .. }) = s.state.shelf_mut(shelf).and_then(|sh| sh.items.iter_mut().find(|i| i.id == item)).map(|i| &mut i.kind) {
                            *icon = Some(blob);
                        }
                    });
                }
                Err(e) => note(shelf, tf("Couldn't load the site's icon: {error}", &[("error", &e)]), cx),
            }
        });
    })
    .detach();
}

/// Decodes queued tile pictures, one at a time off the UI thread; each lands
/// as it is ready. Results for pictures released meanwhile are dropped.
pub(crate) fn kick_thumbs(cx: &mut App) {
    const RESULTS_QUEUED: usize = 1;
    let Some(e) = entity_ref(cx) else { return };
    let Some(first) = e.update(cx, |s, _| s.thumbs.queue.start()) else { return };
    let weak = e.downgrade();
    cx.spawn(async move |cx| {
        let mut batch = Some(first);
        while let Some(pending) = batch {
            let generation = pending.generation();
            let (tx, rx) = async_channel::bounded(RESULTS_QUEUED);
            let task = cx.background_executor().spawn(async move {
                pending.run(|job: thumbs::Job| (job.key, thumbs::decode(&job.path, job.key)), tx).await;
            });
            while let Ok((key, image)) = rx.recv().await {
                let Some(e) = weak.upgrade() else { break };
                cx.update(|cx| {
                    let mut dropped = Vec::new();
                    e.update(cx, |s, cx| {
                        if s.thumbs.queue.current(generation) {
                            s.thumbs.land(key, image, |i| dropped.push(i));
                            cx.notify();
                        } else if let Some(i) = image {
                            dropped.push(i);
                        }
                    });
                    for i in dropped {
                        cx.drop_image(i, None);
                    }
                });
            }
            drop(rx);
            task.await;
            let Some(e) = weak.upgrade() else { break };
            batch = cx.update(|cx| e.update(cx, |s, _| s.thumbs.queue.finish()));
        }
    })
    .detach();
}

/// Applies the cache limit off the UI thread: evicts the oldest closed
/// shelves; when open shelves alone pass it, says so (never deletes them).
fn enforce_cache(cx: &mut App) {
    let Some(e) = entity_ref(cx) else { return };
    let limit = settings(cx).max_cache_bytes();
    let (open, closed): (Vec<u64>, Vec<(u64, u64)>) = {
        let s = e.read(cx);
        (s.state.shelves.iter().map(|sh| sh.id).collect(), s.state.closed.iter().map(|c| (c.shelf.id, c.closed)).collect())
    };
    let task = cx.background_executor().spawn(async move { store::enforce_limit(&open, &closed, limit) });
    cx.spawn(async move |cx| {
        let (plan, open_bytes) = task.await;
        cx.update(|cx| {
            if !plan.evict.is_empty() {
                tracing::info!("stash: cache limit: evicted closed shelves {:?}", plan.evict);
                change(cx, |s, _| s.state.closed.retain(|c| !plan.evict.contains(&c.shelf.id)));
                mandara_services::memory::request_trim();
            }
            if let Some(e) = entity_ref(cx) {
                e.update(cx, |s, cx| {
                    let over = plan.over_with_open.then_some(open_bytes);
                    if s.over_limit != over {
                        s.over_limit = over;
                        cx.notify();
                    }
                });
            }
        });
    })
    .detach();
}

// ── Outputs ──────────────────────────────────────────────────────────────

fn outputs(cx: &App) -> Vec<mandara_core::OutputGeometry> {
    cx.try_global::<crate::overlays::Outputs>().map(|o| o.0.clone()).unwrap_or_default()
}

fn output_size(name: &str, cx: &App) -> (f32, f32) {
    outputs(cx).iter().find(|o| o.name == name).filter(|o| o.has_size()).map(|o| (o.width as f32, o.height as f32)).unwrap_or_else(|| crate::overlays::primary_output_size(cx))
}

// ── Bar item, tray, commands ─────────────────────────────────────────────

/// The item count on a minimized shelf's tray icon: size and inset.
const TRAY_COUNT_PX: f32 = 8.0;
const TRAY_COUNT_INSET: f32 = 1.0;
/// Bar chip size.
const BAR_ICON_PX: f32 = 15.0;
const BAR_GAP_PX: f32 = 5.0;

fn counts(cx: &App) -> (usize, usize, usize) {
    entity_ref(cx).map_or((0, 0, 0), |e| {
        let s = &e.read(cx).state;
        (s.shelves.len(), s.shelves.iter().filter(|x| x.minimized).count(), s.item_count())
    })
}

/// The bar item's click: brings minimized shelves back, else minimizes the
/// shown ones, or makes the first shelf.
fn bar_click(cx: &mut App) {
    let Some(e) = entity_ref(cx) else { return };
    let (minimized, shown): (Vec<u64>, Vec<u64>) = {
        let s = &e.read(cx).state;
        (s.shelves.iter().filter(|x| x.minimized).map(|x| x.id).collect(), s.shelves.iter().filter(|x| !x.minimized).map(|x| x.id).collect())
    };
    if !minimized.is_empty() {
        minimized.into_iter().for_each(|id| restore_shelf(id, cx));
    } else if !shown.is_empty() {
        shown.into_iter().for_each(|id| minimize(id, cx));
    } else {
        new_shelf(None, None, Origin::Asked, cx);
    }
}

fn bar_view(palette: &MandaraPalette, cx: &mut App) -> AnyElement {
    let (shelves, minimized, items) = counts(cx);
    let to_bar = cfg_stash::minimize_to(crate::state::config(cx).read(cx).get()).bar();
    let label = if shelves == 0 { None } else { Some(items) };
    div()
        .id("stash-bar")
        .flex()
        .items_center()
        .gap(px(BAR_GAP_PX))
        .child(crate::icons::icon("stash", crate::style::bar_icon(BAR_ICON_PX), if minimized > 0 && to_bar { palette.primary } else { palette.on_surface }))
        .when_some(label, |el, n| el.child(div().font_features(crate::fonts::tabular_nums()).text_color(palette.on_surface_variant.hsla()).child(n.to_string())))
        .drag_over::<ExternalPaths>(move |s, _, _, _| s.opacity(0.7))
        .drag_over::<ExternalData>(move |s, _, _, _| s.opacity(0.7))
        .on_drop(|paths: &ExternalPaths, _, cx| {
            if let Some(id) = target_shelf(cx) {
                drop_paths(id, paths, None, cx);
            }
        })
        .on_drop(|data: &ExternalData, _, cx| {
            if let Some(id) = target_shelf(cx) {
                drop_data(id, data, None, cx);
            }
        })
        .into_any_element()
}

/// Minimized shelves for the tray (when they go there).
pub fn tray_cells(palette: &MandaraPalette, cell: f32, icon_px: f32, cx: &mut App) -> Vec<AnyElement> {
    if !running(cx) || !cfg_stash::minimize_to(crate::state::config(cx).read(cx).get()).tray() {
        return Vec::new();
    }
    let Some(e) = entity_ref(cx) else { return Vec::new() };
    let shelves: Vec<(u64, usize)> = e.read(cx).state.shelves.iter().filter(|s| s.minimized).map(|s| (s.id, s.items.len())).collect();
    let hover = palette.on_surface.with_alpha(crate::kit::alpha::HOVER);
    shelves
        .into_iter()
        .map(|(id, n)| {
            div()
                .id(("stash-tray", id as usize))
                .size(px(cell))
                .flex_none()
                .rounded(px(crate::style::radius(6.0)))
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .hover(move |s| s.bg(hover.hsla()))
                .child(crate::icons::icon("stash", icon_px, palette.primary))
                .child(div().absolute().top(px(TRAY_COUNT_INSET)).right(px(TRAY_COUNT_INSET)).text_size(px(TRAY_COUNT_PX)).text_color(palette.on_surface_variant.hsla()).child(n.to_string()))
                .on_click(move |_, _, cx| restore_shelf(id, cx))
                .into_any_element()
        })
        .collect()
}

/// Shelves as `stash list` reports them.
fn list_json(s: &StashState) -> serde_json::Value {
    json!({
        "shelves": s.shelves.iter().map(|sh| json!({
            "id": sh.id,
            "output": sh.output,
            "x": sh.x, "y": sh.y, "width": sh.w, "height": sh.h,
            "view": sh.view.id(),
            "minimized": sh.minimized,
            "items": sh.items.iter().map(|i| json!({"id": i.id, "kind": i.kind.id(), "title": i.title()})).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "closed": s.closed.iter().map(|c| json!({"id": c.shelf.id, "items": c.shelf.items.len(), "closed": c.closed})).collect::<Vec<_>>(),
    })
}

fn status_json(cx: &mut App) -> serde_json::Value {
    let cfg = crate::state::config(cx).read(cx).get();
    let ctx = edge_context(cx);
    let states = cfg_stash::catcher_states(cfg, ctx);
    let s = settings(cx);
    let (shelves, minimized, items) = counts(cx);
    let (thumb_bytes, thumbs) = entity_ref(cx).map_or((0, 0), |e| (e.read(cx).thumbs.bytes(), e.read(cx).thumbs.len()));
    json!({
        "running": running(cx),
        "shelves": shelves,
        "minimized": minimized,
        "items": items,
        "shake": cfg_stash::SHAKE_LEVELS[usize::from(s.shake).min(3)],
        "catchers": ScreenEdge::ALL.iter().map(|e| json!({"edge": e.id(), "catcher": states[e.index()]})).collect::<Vec<_>>(),
        "corners": cfg_stash::corner_users(cfg, ctx).iter().map(|c| c.corner.id()).collect::<Vec<_>>(),
        "thumbnails": thumbs,
        "thumbnail_bytes": thumb_bytes,
        "over_limit_bytes": entity_ref(cx).and_then(|e| e.read(cx).over_limit),
        "cache_limit_mib": s.max_cache_mib,
    })
}

/// The compositor, for the edge rules.
pub fn edge_context(cx: &App) -> cfg_stash::EdgeContext {
    // Before the hub exists (start-up), the environment says which one.
    let kind = match cx.try_global::<crate::hub::GlobalHub>() {
        Some(hub) => hub.0.read(cx).compositor.as_ref().map(|c| c.kind()),
        None => mandara_compositor::detected(),
    };
    cfg_stash::EdgeContext { niri: kind == Some(mandara_compositor::CompositorKind::Niri), kwin: kind == Some(mandara_compositor::CompositorKind::KWin) }
}

fn require_running(cx: &App) -> Result<(), String> {
    if running(cx) && entity_ref(cx).is_some_and(|e| e.read(cx).loaded) { Ok(()) } else { Err(t("Stash is still loading its shelves; try again in a moment").to_string()) }
}

fn targets(arg: Option<&str>, cx: &App) -> Result<Vec<u64>, String> {
    let target = model::parse_target(arg)?;
    let ids = entity_ref(cx).map(|e| model::resolve_target(&e.read(cx).state, target)).unwrap_or_default();
    if ids.is_empty() {
        return Err(match target {
            Target::One(id) => tf("No shelf {id}; see `mandara msg stash list`", &[("id", &id)]),
            _ => t("No shelves").to_string(),
        });
    }
    Ok(ids)
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Stash"), tk("Floating shelves you drag anything onto and back out of"), Some(Zone::Right)).opt_in(), |window, cx, _panel_cx| {
        let e = entity(cx);
        let bar = super::fn_view(None, |palette, _, cx| bar_view(palette, cx), window, cx);
        super::follow(&bar, &e, cx);
        PluginInstance {
            bar: Some(bar),
            panel: None,
            sidebar: None,
            on_click: Some(Rc::new(|_, cx| bar_click(cx))),
            on_secondary_click: Some(Rc::new(|_, cx| {
                new_shelf(None, None, Origin::Asked, cx);
            })),
        }
    });
    let cmd = |path: &'static str, about: &'static str, f: fn(&mandara_plugins::Args, &mut App) -> Result<serde_json::Value, String>| CommandSpec::new(path, about, f);
    r.command(ID, cmd("stash", tk("Stash's state: shelves, items, shake and edge catchers"), |_, cx| Ok(status_json(cx))).example("stash"));
    r.command(
        ID,
        cmd("stash new", tk("Open a new shelf (in the middle of the focused output, or at x y)"), |a, cx| {
            require_running(cx)?;
            let at = match (a.get(0), a.get(1)) {
                (Some(x), Some(y)) => Some((x.parse::<f32>().map_err(|e| e.to_string())?, y.parse::<f32>().map_err(|e| e.to_string())?)),
                _ => None,
            };
            let output = a.get(2).map(str::to_string);
            let id = new_shelf(output, at, Origin::Asked, cx).ok_or_else(|| t("Stash is off").to_string())?;
            Ok(json!({"shelf": id}))
        })
        .arg(ArgSpec::optional("x", ArgKind::Number, tk("centre, logical px from the output's left")))
        .arg(ArgSpec::optional("y", ArgKind::Number, tk("centre, logical px from the output's top")))
        .arg(ArgSpec::optional("output", ArgKind::String, tk("output name (default: the focused one)")))
        .example("stash new")
        .example("stash new 400 300 DP-1"),
    );
    r.command(ID, cmd("stash list", tk("Shelves and their items (and closed shelves you can restore)"), |_, cx| Ok(entity_ref(cx).map(|e| list_json(&e.read(cx).state)).unwrap_or_default())).example("stash list"));
    for (path, about, act) in [
        ("stash show", tk("Show minimized shelves (id, last or all)"), 0u8),
        ("stash hide", tk("Minimize shelves (id, last or all)"), 1),
    ] {
        let spec = CommandSpec::new(path, about, move |a, cx| {
            require_running(cx)?;
            let ids = targets(a.get(0), cx)?;
            for id in &ids {
                if act == 0 { restore_shelf(*id, cx) } else { minimize(*id, cx) }
            }
            Ok(json!({"shelves": ids}))
        })
        .arg(ArgSpec::optional("shelf", ArgKind::String, tk("a shelf id, last (default) or all")));
        r.command(ID, spec.example(if act == 0 { "stash show all" } else { "stash hide 3" }));
    }
    r.command(
        ID,
        cmd("stash close", tk("Close shelves; their items can be restored until the cache limit evicts them (--empty: delete them now)"), |a, cx| {
            require_running(cx)?;
            let empty = a.0.iter().any(|w| w == "--empty");
            let ids = targets(a.0.iter().find(|w| *w != "--empty").map(String::as_str), cx)?;
            for id in &ids {
                close(*id, empty, cx);
            }
            Ok(json!({"closed": ids, "emptied": empty}))
        })
        .arg(ArgSpec::optional("shelf", ArgKind::String, tk("a shelf id, last (default) or all")))
        .arg(ArgSpec::optional("mode", ArgKind::String, tk("--empty to delete the items instead of keeping them restorable")))
        .example("stash close 2")
        .example("stash close all --empty"),
    );
    r.command(
        ID,
        cmd("stash clear", tk("Remove every item from shelves (they stay open)"), |a, cx| {
            require_running(cx)?;
            let ids = targets(a.get(0), cx)?;
            for id in &ids {
                clear(*id, Vec::new(), cx);
            }
            Ok(json!({"cleared": ids}))
        })
        .arg(ArgSpec::optional("shelf", ArgKind::String, tk("a shelf id, last (default) or all")))
        .example("stash clear 1"),
    );
    r.command(
        ID,
        cmd("stash restore", tk("Reopen a closed shelf with its items (the last one closed by default)"), |a, cx| {
            require_running(cx)?;
            let id = match a.get(0) {
                Some(s) => Some(s.trim_start_matches('#').parse::<u64>().map_err(|e| e.to_string())?),
                None => None,
            };
            let back = change(cx, |s, _| {
                let back = s.state.restore(id);
                if let Some(b) = back {
                    s.ui.insert(b, ShelfUi { opened: Some(Instant::now()), ..ShelfUi::default() });
                }
                back
            })
            .flatten()
            .ok_or_else(|| t("No closed shelf to restore").to_string())?;
            resolve_icons(None, cx);
            Ok(json!({"shelf": back}))
        })
        .arg(ArgSpec::optional("shelf", ArgKind::String, tk("a closed shelf's id (see `stash list`)")))
        .example("stash restore"),
    );
    r.command(
        ID,
        cmd("stash add", tk("Add files, a link or text (text:…) to a shelf"), |a, cx| {
            require_running(cx)?;
            let (target, rest): (Option<&str>, &[String]) = match a.0.first().map(String::as_str) {
                Some(first) if model::parse_target(Some(first)).is_ok() && a.0.len() > 1 => (Some(first), &a.0[1..]),
                _ => (None, &a.0[..]),
            };
            if rest.is_empty() {
                return Err(t("Nothing to add").to_string());
            }
            let shelf = match target {
                Some(t) => targets(Some(t), cx)?[0],
                None => target_shelf(cx).ok_or_else(|| t("Stash is off").to_string())?,
            };
            let dir = store::shelf_dir(shelf);
            let copy = settings(cx).copy_images;
            let args: Vec<AddArg> = rest.iter().map(|w| model::parse_add(w)).collect();
            let n = args.len();
            let task = cx.background_executor().spawn(async move {
                let mut kinds = Vec::new();
                let mut paths = Vec::new();
                for arg in args {
                    match arg {
                        AddArg::Path(p) => paths.push(p),
                        AddArg::Link(url) => kinds.push(ItemKind::Url { url, title: None, icon: None }),
                        AddArg::Text(text) => match ingest::text_item(&dir, &text, None) {
                            Ok(k) => kinds.push(k),
                            Err(e) => tracing::warn!("stash add: {e}"),
                        },
                    }
                }
                kinds.extend(ingest::from_paths(&dir, &paths, copy));
                kinds
            });
            cx.spawn(async move |cx| {
                let kinds = task.await;
                cx.update(|cx| add_items(shelf, kinds, None, cx));
            })
            .detach();
            Ok(json!({"shelf": shelf, "adding": n}))
        })
        .arg(ArgSpec::optional("shelf", ArgKind::String, tk("a shelf id (default: the newest shelf)")))
        .arg(ArgSpec::optional("items", ArgKind::String, tk("paths, file:// or https:// links, or text:…")))
        .example("stash add ~/Downloads/report.pdf")
        .example("stash add 2 https://example.org text:remember this"),
    );
}

/// What the bar shows for a minimized shelf count (for tests).
pub fn minimize_targets(to: MinimizeTo) -> (bool, bool) {
    (to.bar(), to.tray())
}

#[cfg(test)]
mod tests;
