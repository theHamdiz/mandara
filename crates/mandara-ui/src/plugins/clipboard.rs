//! Clipboard history utility: a bar button and a panel that sorts what you
//! copy. Every entry is classified when it is copied (text, link, Markdown,
//! code with its language, CSS, colour, image or screenshot, files) and
//! drawn by kind ([`rows`]): colour swatches, image thumbnails, tinted code,
//! rendered Markdown, file icons with their folder, links by host. A search
//! box and filter chips (all, pinned, each kind present, each collection)
//! narrow the list; collections group entries by hand and survive clears.
//!
//! Click copies (or pastes, per `[clipboard] click_action`), right-click
//! does the other, the folder adds an entry to collections, the pin keeps it
//! through clears, the crop button opens an image in your editor and the
//! trash forgets it. [`init`] keeps the history service in step with
//! `[clipboard]`, the focused app (per-app exclusion) and screen locks.
//! Everything the panel does is also a `clipboard …` command.

mod rows;
mod tint;

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, AppContext as _, Entity, FontWeight, InteractiveElement, IntoElement, KeyDownEvent, MouseButton, ParentElement, RenderImage, ScrollDelta, ScrollEvent,
    SharedString, StatefulInteractiveElement, Styled, Task, Window, div, prelude::FluentBuilder as _, px, size,
};
use mandara_config::workflow::ClipboardConfig;
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk, tr};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::clipboard::{ClipCollection, ClipContent, ClipEntry, ClipboardCommand, Kind};
use mandara_theme::MandaraPalette;
use serde_json::json;

use self::rows::{PICKER_H, RowActions, RowCx, RowText, THUMB_H_PX, THUMB_W_PX, Thumb, collection_tint, entry_row, kind_icon, kind_label, row_h};
use super::panel_fx::{HEADER_H, SECTION_H_PX, ViewSet, WASH_SOFT, alpha, empty_state, header, pill_button, readable, wash};
use super::fn_view;
use crate::hub::hub;
use crate::icons::icon;
use crate::kit::{icon_button, reveal, text, tokens};
use crate::state::shell;
use crate::text_field::TextField;

pub use self::rows::{kind_icon as clip_kind_icon, kind_label as clip_kind_label};

pub const ID: &str = "clipboard";
/// History rows per page at most.
const PAGE: usize = 8;
/// Height a page of rows may take (taller rows mean fewer per page), so
/// the panel fits a 1080p screen with pinned rows and the controls above.
const PAGE_BUDGET_H: f32 = PAGE_BUDGET_ROWS * rows::IMAGE_H;
/// Image rows that fit [`PAGE_BUDGET_H`].
const PAGE_BUDGET_ROWS: f32 = 6.0;
/// Wheel notches that turn one page: a wheel click is one notch, touchpads
/// send fractions, so a nudge does not skip a page.
const WHEEL_NOTCHES_PER_PAGE: f32 = 2.0;
/// "Clear all" and "delete collection" stay armed for a second click this long.
const CONFIRM_WINDOW: Duration = Duration::from_secs(3);
/// Longest side of a decoded preview, in physical pixels.
const MAX_THUMB_PX: f32 = 256.0;
/// Upper bound on decoded preview bytes held at once (LRU beyond it).
const THUMB_BUDGET_BYTES: usize = 1024 * 1024;
/// Previews are dropped this long after the panel closes (after its close
/// animation, so it never flashes placeholders).
const DROP_AFTER_CLOSE: Duration = Duration::from_millis(600);
/// Pager arrows.
const PAGER_PAD_X_PX: f32 = 5.0;
const PAGER_GAP_PX: f32 = 2.0;
const PAGER_DISABLED_ALPHA: f32 = 0.35;
const PANEL_W: f32 = 400.0;
const PAD: f32 = 14.0;
const GAP: f32 = 12.0;
const EMPTY_H: f32 = 170.0;
/// Panel height before the first render measures the list.
const INITIAL_H: f32 = 260.0;
/// Gap between the section label and the list.
const LIST_GAP: f32 = tokens::XS;
/// Gap between the controls (search, chips, naming row).
const CONTROL_GAP: f32 = tokens::SM;
/// Bar glyph size.
const BAR_ICON_PX: f32 = 15.0;
/// Bytes per RGBA / BGRA pixel.
const RGBA_CHANNELS: usize = 4;

// Search box and naming row.
const FIELD_H: f32 = 34.0;
const FIELD_RADIUS_PX: f32 = 12.0;
const FIELD_PAD_L_PX: f32 = 12.0;
const FIELD_PAD_R_PX: f32 = 4.0;
const FIELD_ICON_PX: f32 = 14.0;
const FIELD_BUTTON_PX: f32 = 26.0;
const FIELD_GAP_PX: f32 = 6.0;
// Filter chips.
const CHIPS_H: f32 = 28.0;
const CHIP_H_PX: f32 = 26.0;
const CHIP_PAD_X_PX: f32 = 10.0;
const CHIP_GAP_PX: f32 = 6.0;
const CHIP_INNER_GAP_PX: f32 = 5.0;
const CHIP_ICON_PX: f32 = 12.0;
const CHIP_DOT_PX: f32 = 7.0;
const CHIP_IDLE_ALPHA: f32 = 0.06;
const CHIP_ACTIVE_ALPHA: f32 = 0.2;
const CHIP_COUNT_ALPHA: f32 = 0.7;
/// The hairline between kind chips and collection chips.
const CHIP_DIVIDER_W_PX: f32 = 1.0;
const CHIP_DIVIDER_H_PX: f32 = 14.0;
const CHIP_DIVIDER_ALPHA: f32 = 0.14;
/// Section actions (rename / delete a collection).
const SECTION_BUTTON_PX: f32 = 22.0;
const SECTION_ICON_PX: f32 = 12.0;
/// Fill behind the name field's save button (dark, light).
const SAVE_FILL: (f32, f32) = (0.14, 0.1);

fn send(cx: &mut App, command: ClipboardCommand) {
    if let Some(s) = hub(cx).read(cx).clipboard_service.clone() {
        s.send(command);
    }
}

fn request(cx: &mut App, command: ClipboardCommand) -> Result<Output, String> {
    let s = hub(cx).read(cx).clipboard_service.clone().ok_or("clipboard service unavailable")?;
    Ok(Output::Pending(s.request(command)))
}

// ── Filters ──────────────────────────────────────────────────────────────

/// What the panel lists.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Filter {
    /// Pinned entries on top, then the rest of the history.
    #[default]
    All,
    Pinned,
    Kind(Kind),
    Collection(u32),
}

impl Filter {
    fn admits(self, e: &ClipEntry) -> bool {
        match self {
            Filter::All => true,
            Filter::Pinned => e.pinned,
            Filter::Kind(k) => e.class.kind() == k,
            Filter::Collection(c) => e.collections.contains(&c),
        }
    }

    /// Reads `all`, `pinned`, a kind (`code`, `colors`, …) or a collection
    /// (by id or name).
    pub fn parse(s: &str, collections: &[ClipCollection]) -> Option<Filter> {
        let s = s.trim();
        match s.to_lowercase().as_str() {
            "all" | "" => return Some(Filter::All),
            "pinned" | "pins" => return Some(Filter::Pinned),
            _ => {}
        }
        if let Some(k) = Kind::parse(s) {
            return Some(Filter::Kind(k));
        }
        collection(s, collections).map(|c| Filter::Collection(c.id))
    }

    fn describe(self, collections: &[ClipCollection]) -> serde_json::Value {
        match self {
            Filter::All => json!("all"),
            Filter::Pinned => json!("pinned"),
            Filter::Kind(k) => json!(k.id()),
            Filter::Collection(id) => json!({ "collection": id, "name": collections.iter().find(|c| c.id == id).map(|c| c.name.clone()) }),
        }
    }
}

/// A collection by id or (case-insensitive) name.
fn collection<'a>(s: &str, collections: &'a [ClipCollection]) -> Option<&'a ClipCollection> {
    let s = s.trim();
    s.parse::<u32>().ok().and_then(|id| collections.iter().find(|c| c.id == id)).or_else(|| collections.iter().find(|c| c.name.eq_ignore_ascii_case(s)))
}

/// What the panel shows: pinned entries on top (in the All view) and the
/// filtered history, before paging.
struct Listing<'a> {
    pinned: Vec<&'a ClipEntry>,
    main: Vec<&'a ClipEntry>,
}

fn listing<'a>(entries: &'a [ClipEntry], collections: &[ClipCollection], filter: Filter, query: &str, pinned_shown: usize) -> Listing<'a> {
    let hit = |e: &&ClipEntry| filter.admits(e) && e.matches(query, collections);
    match filter {
        Filter::All => Listing { pinned: entries.iter().filter(|e| e.pinned).filter(hit).take(pinned_shown).collect(), main: entries.iter().filter(|e| !e.pinned).filter(hit).collect() },
        _ => Listing { pinned: Vec::new(), main: entries.iter().filter(hit).collect() },
    }
}

/// Splits rows of the given heights into pages of at most [`PAGE`] rows
/// and [`PAGE_BUDGET_H`] (at least one row each).
fn paginate(heights: &[f32]) -> Vec<Range<usize>> {
    let (mut pages, mut start, mut used) = (Vec::new(), 0, 0.0);
    for (i, h) in heights.iter().enumerate() {
        if i > start && (i - start >= PAGE || used + h > PAGE_BUDGET_H) {
            pages.push(start..i);
            (start, used) = (i, 0.0);
        }
        used += h;
    }
    if start < heights.len() || pages.is_empty() {
        pages.push(start..heights.len());
    }
    pages
}

/// Changes whenever anything a search result depends on changes.
fn signature(entries: &[ClipEntry], collections: &[ClipCollection]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for e in entries {
        (e.id, e.pinned, &e.collections).hash(&mut h);
    }
    for c in collections {
        (c.id, &c.name).hash(&mut h);
    }
    h.finish()
}

// ── Panel state ──────────────────────────────────────────────────────────

/// A preview plus its LRU bookkeeping.
struct ThumbSlot {
    thumb: Thumb,
    /// Decoded size in bytes (0 until ready).
    bytes: usize,
    /// [`Ui::tick`] of the last render that showed it.
    used: u64,
}

/// Naming a collection in the row under the chips.
#[derive(Copy, Clone, Debug, PartialEq)]
enum Naming {
    /// A new collection, optionally receiving an entry right away.
    New { add: Option<u64> },
    Rename(u32),
}

/// A search's results, kept while nothing they depend on changes.
struct SearchCache {
    key: (String, Filter, u64),
    pinned: Vec<u64>,
    main: Vec<u64>,
}

/// The panel's state: row texts and thumbnails keyed by entry id, the
/// filter and paging, and the collection editing state.
///
/// Previews are decoded only for the page on screen, at the tile's
/// physical size capped by [`MAX_THUMB_PX`] (never the full screenshot);
/// their total stays under [`THUMB_BUDGET_BYTES`] (least recently shown
/// evicted first), and they are all dropped shortly after the panel closes
/// — rows keep same-size placeholders, so reopening never reflows.
#[derive(Default)]
struct Ui {
    rows: HashMap<u64, RowText>,
    thumbs: HashMap<u64, ThumbSlot>,
    /// Render counter for LRU.
    tick: u64,
    /// Page of the list on screen (0 = newest).
    page: usize,
    filter: Filter,
    /// The entry whose collection picker is open.
    picker: Option<u64>,
    naming: Option<Naming>,
    search: Option<SearchCache>,
    /// Newest entry the bar chip has pre-decoded for.
    newest: Option<u64>,
    /// Pending "drop previews after close" timer (dropping it cancels).
    drop_after_close: Option<Task<()>>,
    /// Views to re-render when a thumbnail lands.
    views: ViewSet,
    /// "Clear all" was clicked once (confirm before this instant).
    confirm_until: Option<Instant>,
    /// "Delete collection" was clicked once.
    confirm_delete: Option<(u32, Instant)>,
    /// The chip strip's scroll, and the filter it last scrolled into view.
    chips: crate::hstrip::HStrip,
    chips_shown: Option<Filter>,
    /// The entry the keyboard is on, if any.
    selected: Option<u64>,
    /// Entry ids in the order the panel draws them: the pinned rows, then
    /// the whole filtered history, pages included, so the arrows can walk
    /// past a page's end and take the list with them.
    order: Vec<u64>,
    /// How many of `order`'s leading ids are pinned (drawn on every page).
    pinned_len: usize,
    /// The page each history row of `order` sits on.
    page_of: Vec<usize>,
    /// How many pages the history has.
    pages: usize,
    /// Wheel distance collected toward the next page turn.
    wheel: f32,
}

type Shared = Rc<RefCell<Ui>>;

impl Ui {
    fn scroll_page(&mut self, notches: f32) {
        if self.pages < 2 || notches * self.wheel < 0.0 { self.wheel = 0.0; }
        if self.pages < 2 { return; }
        self.wheel += notches;
        if self.wheel.abs() < WHEEL_NOTCHES_PER_PAGE { return; }
        let newer = self.wheel > 0.0;
        self.wheel = 0.0;
        let page = if newer { self.page.saturating_sub(1) } else { (self.page + 1).min(self.pages - 1) };
        if page != self.page {
            self.page = page;
            self.picker = None;
            self.selected = None;
        }
    }

    /// Begin on the visible history page, then walk across page boundaries.
    fn next_selection(&self, delta: isize) -> Option<(usize, u64)> {
        if self.selected.is_none() {
            let on_page = |(_, page): &(usize, &usize)| **page == self.page;
            let candidate = if delta > 0 {
                self.page_of.iter().enumerate().find(on_page)
            } else {
                self.page_of.iter().enumerate().rfind(on_page)
            };
            if let Some((index, _)) = candidate {
                let index = index + self.pinned_len;
                if let Some(id) = self.order.get(index) { return Some((index, *id)); }
            }
        }
        step_selection(&self.order, self.selected, delta)
    }

    /// Keep keyboard activation restricted to the rows drawn on this page.
    fn reconcile_selection(&mut self, layout: &Layout<'_>) {
        self.order.clear();
        self.order.extend(layout.listing.pinned.iter().map(|entry| entry.id));
        self.pinned_len = self.order.len();
        self.order.extend(layout.listing.main.iter().map(|entry| entry.id));
        self.page_of.clear();
        for (page, range) in layout.pages.iter().enumerate() {
            self.page_of.extend(range.clone().map(|_| page));
        }
        self.pages = layout.pages.len();
        self.selected = self.selected.filter(|id| layout.listing.pinned.iter().chain(layout.page_rows()).any(|entry| entry.id == *id));
    }

    fn decoded_bytes(&self) -> usize {
        self.thumbs.values().map(|t| t.bytes).sum()
    }

    /// Evicts least-recently-shown previews (never ones in `keep`) until
    /// the decoded total fits the budget.
    fn enforce_budget(&mut self, keep: &[u64]) {
        while self.decoded_bytes() > THUMB_BUDGET_BYTES {
            let victim = self.thumbs.iter().filter(|(id, t)| t.bytes > 0 && !keep.contains(id)).min_by_key(|(_, t)| t.used).map(|(id, _)| *id);
            match victim {
                Some(id) => {
                    self.thumbs.remove(&id);
                }
                None => break,
            }
        }
    }

    /// Frees every decoded preview and resets paging (panel closed).
    fn drop_previews(&mut self) {
        self.thumbs.clear();
        self.thumbs.shrink_to_fit();
        self.rows.clear();
        self.rows.shrink_to_fit();
        self.search = None;
        self.page = 0;
        self.picker = None;
        self.selected = None;
        self.order.clear();
        self.page_of.clear();
        self.pages = 0;
        self.wheel = 0.0;
    }

    /// Switches the list to `filter` (first page, pickers and keyboard
    /// selection closed: the rows under it are different now).
    fn show(&mut self, filter: Filter) {
        self.filter = filter;
        self.page = 0;
        self.picker = None;
        self.selected = None;
        self.wheel = 0.0;
    }
}

fn update(ui: &Shared, cx: &mut App, f: impl FnOnce(&mut Ui)) {
    let views = {
        let mut u = ui.borrow_mut();
        f(&mut u);
        u.views.clone()
    };
    views.notify(cx);
}

/// Where the arrows land in `order`: the index and entry id, clamped to the
/// ends, starting at the top for Down and at the bottom for Up when nothing
/// is selected yet.
fn step_selection(order: &[u64], current: Option<u64>, delta: isize) -> Option<(usize, u64)> {
    let last = order.len().checked_sub(1)?;
    let next = match current.and_then(|id| order.iter().position(|o| *o == id)) {
        Some(at) => (at as isize + delta).clamp(0, last as isize) as usize,
        None if delta > 0 => 0,
        None => last,
    };
    Some((next, order[next]))
}

/// Moves the keyboard selection by `delta` rows, turning the page when it
/// steps past the one on screen (the picker closes: it belongs to a row
/// that may not be visible any more).
fn select_step(ui: &Shared, delta: isize, cx: &mut App) {
    update(ui, cx, |u| {
        let Some((at, id)) = u.next_selection(delta) else {
            u.selected = None;
            return;
        };
        u.selected = Some(id);
        if at >= u.pinned_len {
            u.page = u.page_of.get(at - u.pinned_len).copied().unwrap_or(u.page);
        }
        u.picker = None;
    });
}

/// Turns pages from wheel distance: up goes to newer rows, down to older.
fn wheel_step(ui: &Shared, notches: f32, cx: &mut App) {
    update(ui, cx, |u| u.scroll_page(notches));
}

/// Runs what a left click on the selected row does: copy it (or paste it,
/// where the panel is set to paste).
fn activate(ui: &Shared, cx: &mut App) {
    let Some(id) = ui.borrow().selected else { return };
    let action = {
        let look = Look::of(&crate::state::config(cx).read(cx).get().clipboard);
        let state = hub(cx).read(cx);
        state.clipboard.entries.iter().find(|e| e.id == id).map(|e| row_actions(ui, e, &look).restore.clone())
    };
    if let Some(action) = action {
        action(false, cx);
    }
}

thread_local! {
    /// Every open panel's state (one per bar), for `clipboard view`.
    static PANELS: RefCell<Vec<(Weak<RefCell<Ui>>, Entity<TextField>)>> = const { RefCell::new(Vec::new()) };
}

// ── Thumbnails ───────────────────────────────────────────────────────────

/// Physical preview size for `scale`, the longest side capped at
/// [`MAX_THUMB_PX`].
fn thumb_px(scale: f32) -> (u32, u32) {
    let (w, h) = (THUMB_W_PX * scale, THUMB_H_PX * scale);
    let k = (MAX_THUMB_PX / w.max(h)).min(1.0);
    ((w * k).round().max(1.0) as u32, (h * k).round().max(1.0) as u32)
}

/// Decodes `path` cropped to fill `w × h` physical pixels, as BGRA.
fn decode_thumb(path: PathBuf, w: u32, h: u32) -> Option<Arc<RenderImage>> {
    let picture = image::open(&path).ok()?;
    let mut rgba = picture.resize_to_fill(w, h, image::imageops::FilterType::Triangle).into_rgba8();
    drop(picture);
    for px in rgba.as_chunks_mut::<RGBA_CHANNELS>().0 {
        px.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(rgba)])))
}

/// Brings the caches in line with the rows on screen: drops texts of other
/// rows and previews of forgotten entries, derives missing row texts and
/// returns the images that still need a preview (marked as loading).
fn sync_cache(ui: &Shared, entries: &[ClipEntry], shown: &[&ClipEntry], preview_lines: usize) -> Vec<(u64, PathBuf)> {
    let mut u = ui.borrow_mut();
    u.tick += 1;
    let tick = u.tick;
    u.rows.retain(|id, r| r.preview_lines == preview_lines && shown.iter().any(|e| e.id == *id));
    u.thumbs.retain(|id, _| entries.iter().any(|e| e.id == *id));
    let mut pending = Vec::new();
    for e in shown {
        u.rows.entry(e.id).or_insert_with(|| RowText::of(e, preview_lines));
        let ClipContent::Image { path, .. } = &e.content else { continue };
        match u.thumbs.get_mut(&e.id) {
            Some(slot) => slot.used = tick,
            None => {
                u.thumbs.insert(e.id, ThumbSlot { thumb: Thumb::Loading, bytes: 0, used: tick });
                pending.push((e.id, path.clone()));
            }
        }
    }
    pending
}

/// Decodes previews off the UI thread at `scale` and re-renders the panel
/// as each lands.
fn decode_thumbs(ui: &Shared, pending: Vec<(u64, PathBuf)>, scale: f32, cx: &mut App) {
    let (w, h) = thumb_px(scale);
    for (id, path) in pending {
        let task = cx.background_executor().spawn(async move { decode_thumb(path, w, h) });
        let ui = ui.clone();
        cx.spawn(async move |cx| {
            let decoded = task.await;
            let views = {
                let mut u = ui.borrow_mut();
                // Ignore results for entries dropped meanwhile (closed
                // panel, evicted, forgotten).
                if let Some(slot) = u.thumbs.get_mut(&id) {
                    match decoded {
                        Some(image) => {
                            slot.bytes = w as usize * h as usize * RGBA_CHANNELS;
                            slot.thumb = Thumb::Ready(image);
                        }
                        None => slot.thumb = Thumb::Failed,
                    }
                    let keep: Vec<u64> = u.thumbs.iter().filter(|(_, t)| t.used == u.tick).map(|(id, _)| *id).collect();
                    u.enforce_budget(&keep);
                }
                u.views.clone()
            };
            views.notify_async(cx);
        })
        .detach();
    }
}

/// Follows the panel's open state: cancels a pending drop when it opens,
/// schedules dropping every preview once it has closed (after the close
/// animation, so it never flashes placeholders).
fn follow_panel(ui: &Shared, open: bool, cx: &mut App) {
    let mut u = ui.borrow_mut();
    if open {
        u.drop_after_close = None;
        return;
    }
    if u.thumbs.is_empty() && u.rows.is_empty() && u.page == 0 || u.drop_after_close.is_some() {
        return;
    }
    let weak = Rc::downgrade(ui);
    u.drop_after_close = Some(cx.spawn(async move |cx| {
        cx.background_executor().timer(DROP_AFTER_CLOSE).await;
        if let Some(ui) = weak.upgrade() {
            let mut u = ui.borrow_mut();
            u.drop_previews();
            u.drop_after_close = None;
        }
    }));
}

// ── Pieces of the panel ──────────────────────────────────────────────────

/// `‹ 9–16 / 40 ›`: steps through older pages of the list.
fn pager(ui: &Shared, pages: &[Range<usize>], page: usize, n: usize, palette: &MandaraPalette) -> AnyElement {
    let last = pages.len().saturating_sub(1);
    let step = |id: &'static str, glyph: &'static str, enabled: bool, to: usize| {
        let ui = ui.clone();
        div()
            .id(id)
            .px(px(PAGER_PAD_X_PX))
            .rounded_full()
            .text_color(if enabled { palette.on_surface.hsla() } else { palette.on_surface_variant.with_alpha(PAGER_DISABLED_ALPHA).hsla() })
            .when(enabled, |el| {
                el.cursor_pointer().hover(|s| s.bg(wash(WASH_SOFT, palette).hsla())).on_click(move |_, _, cx| {
                    update(&ui, cx, |u| {
                        u.page = to;
                        u.picker = None;
                        // The keyboard selection points at a row of the page
                        // being left, so Enter must not still run it.
                        u.selected = None;
                    })
                })
            })
            .child(glyph)
    };
    let range = pages.get(page).cloned().unwrap_or(0..0);
    div()
        .flex()
        .items_center()
        .gap(px(PAGER_GAP_PX))
        .font_features(crate::fonts::tabular_nums())
        .text_size(px(text::LABEL))
        .text_color(palette.on_surface_variant.hsla())
        .child(step("clip-newer", "‹", page > 0, page.saturating_sub(1)))
        .child(format!("{}–{} / {n}", range.start + 1, range.end))
        .child(step("clip-older", "›", page < last, (page + 1).min(last)))
        .into_any_element()
}

/// A section label with dynamic text (kinds, collection names).
fn section_label(label: SharedString, palette: &MandaraPalette, trailing: Option<AnyElement>) -> AnyElement {
    use crate::bidi::Bidi as _;
    div()
        .row()
        .items_center()
        .justify_between()
        .h(px(SECTION_H_PX))
        .child(div().min_w(px(0.0)).truncate().text_size(px(text::LABEL)).font_weight(FontWeight::SEMIBOLD).text_color(palette.on_surface_variant.hsla()).child(label))
        .when_some(trailing, |el, t| el.child(t))
        .into_any_element()
}

/// One filter chip.
fn chip(id: impl Into<gpui::ElementId>, marker: AnyElement, label: SharedString, count: usize, active: bool, tint: mandara_theme::Color, palette: &MandaraPalette, on_click: impl Fn(&mut App) + 'static) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(CHIP_H_PX))
        .px(px(CHIP_PAD_X_PX))
        .flex_none()
        .rounded_full()
        .flex()
        .items_center()
        .gap(px(CHIP_INNER_GAP_PX))
        .cursor_pointer()
        .text_size(px(text::BODY_S))
        .font_weight(if active { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
        .bg(if active { tint.with_alpha(CHIP_ACTIVE_ALPHA).hsla() } else { palette.on_surface.with_alpha(CHIP_IDLE_ALPHA).hsla() })
        .text_color(if active { palette.on_surface.hsla() } else { palette.on_surface_variant.hsla() })
        .hover(move |s| s.bg(tint.with_alpha(CHIP_ACTIVE_ALPHA).hsla()))
        .child(marker)
        .child(label)
        .when(count > 0, |el| el.child(div().font_features(crate::fonts::tabular_nums()).text_color(palette.on_surface_variant.with_alpha(CHIP_COUNT_ALPHA).hsla()).child(count.to_string())))
        .on_click(move |_, _, cx| on_click(cx))
}

/// Starts naming a collection: clears (or fills) the name field and
/// focuses it.
fn start_naming(ui: &Shared, field: &Entity<TextField>, naming: Naming, name: &str, window: &mut Window, cx: &mut App) {
    field.update(cx, |f, cx| {
        f.set(name);
        f.focus(window, cx);
    });
    update(ui, cx, |u| {
        u.naming = Some(naming);
        u.picker = None;
    });
}

/// The chips: all, pinned, kinds present, collections, and a new-collection
/// button.
fn chips_strip(ui: &Shared, name_field: &Entity<TextField>, entries: &[ClipEntry], collections: &[ClipCollection], kind_chips: bool, palette: &MandaraPalette) -> AnyElement {
    let current = ui.borrow().filter;
    let accent = readable(palette.primary, palette);
    let dragging = ui.borrow().chips.clone();
    let pick = |filter: Filter| {
        let (ui, dragging) = (ui.clone(), dragging.clone());
        // A press that dragged the strip is not a click on the chip.
        move |cx: &mut App| {
            if !dragging.dragged() {
                update(&ui, cx, |u| u.show(filter));
            }
        }
    };
    let glyph = |name: &'static str, tint| icon(name, CHIP_ICON_PX, tint).into_any_element();
    let mut chips: Vec<AnyElement> = vec![chip("clip-chip-all", glyph("clipboard", accent), t("All").into(), entries.len(), current == Filter::All, accent, palette, pick(Filter::All)).into_any_element()];
    let mut active_ix = (current == Filter::All).then_some(0);
    let pinned = entries.iter().filter(|e| e.pinned).count();
    if pinned > 0 || current == Filter::Pinned {
        chips.push(chip("clip-chip-pinned", glyph("pin", accent), t("Pinned").into(), pinned, current == Filter::Pinned, accent, palette, pick(Filter::Pinned)).into_any_element());
        if current == Filter::Pinned {
            active_ix = Some(chips.len() - 1);
        }
    }
    if kind_chips {
        let mut counts = [0usize; Kind::ALL.len()];
        for e in entries {
            if let Some(i) = Kind::ALL.iter().position(|k| *k == e.class.kind()) {
                counts[i] += 1;
            }
        }
        for (i, k) in Kind::ALL.into_iter().enumerate() {
            let active = current == Filter::Kind(k);
            if counts[i] > 0 || active {
                chips.push(chip(("clip-chip-kind", i), glyph(kind_icon(k), accent), tr(kind_label(k)).into(), counts[i], active, accent, palette, pick(Filter::Kind(k))).into_any_element());
                if active {
                    active_ix = Some(chips.len() - 1);
                }
            }
        }
    }
    if !collections.is_empty() {
        chips.push(div().w(px(CHIP_DIVIDER_W_PX)).h(px(CHIP_DIVIDER_H_PX)).flex_none().bg(palette.on_surface.with_alpha(CHIP_DIVIDER_ALPHA).hsla()).into_any_element());
    }
    for c in collections {
        let tint = collection_tint(c.id, palette);
        let count = entries.iter().filter(|e| e.collections.contains(&c.id)).count();
        let dot = div().size(px(CHIP_DOT_PX)).flex_none().rounded_full().bg(tint.hsla()).into_any_element();
        let (ui_r, field, id, name) = (ui.clone(), name_field.clone(), c.id, c.name.clone());
        chips.push(
            chip(("clip-chip-col", u64::from(c.id)), dot, c.name.clone().into(), count, current == Filter::Collection(c.id), tint, palette, pick(Filter::Collection(c.id)))
                .on_mouse_down(MouseButton::Right, move |_, window, cx| {
                    cx.stop_propagation();
                    start_naming(&ui_r, &field, Naming::Rename(id), &name, window, cx);
                })
                .into_any_element(),
        );
        if current == Filter::Collection(c.id) {
            active_ix = Some(chips.len() - 1);
        }
    }
    let (ui_n, field) = (ui.clone(), name_field.clone());
    chips.push(
        div()
            .id("clip-chip-new")
            .child(icon_button(("clip-chip-new", 0u64), "plus", CHIP_H_PX, palette.on_surface_variant, Some(palette.on_surface.with_alpha(CHIP_IDLE_ALPHA)), move |window, cx| {
                start_naming(&ui_n, &field, Naming::New { add: None }, "", window, cx);
            }))
            .into_any_element(),
    );
    // Keep the active chip in view when the filter changes (a command or a
    // click on a chip half off the edge).
    let strip = {
        let mut u = ui.borrow_mut();
        if u.chips_shown != Some(current) {
            u.chips_shown = Some(current);
            if let Some(ix) = active_ix {
                u.chips.scroll.scroll_to_item(ix);
            }
        }
        u.chips.clone()
    };
    let row = strip.row("clip-chips").flex().items_center().gap(px(CHIP_GAP_PX)).children(chips);
    strip.with_chevrons("clip-chips-more", row, CHIPS_H, palette)
}

/// A rounded field row: leading icon, the text field, trailing buttons.
fn field_row(lead_icon: &'static str, field: &Entity<TextField>, trailing: Vec<AnyElement>, palette: &MandaraPalette) -> AnyElement {
    div()
        .h(px(FIELD_H))
        .pl(px(FIELD_PAD_L_PX))
        .pr(px(FIELD_PAD_R_PX))
        .rounded(px(crate::style::radius(FIELD_RADIUS_PX)))
        .bg(palette.surface_container_high.hsla())
        .flex()
        .items_center()
        .gap(px(FIELD_GAP_PX))
        .child(icon(lead_icon, FIELD_ICON_PX, palette.on_surface_variant))
        .child(div().flex_1().min_w(px(0.0)).child(field.clone()))
        .children(trailing)
        .into_any_element()
}

/// Creates or renames a collection from the name field's text.
fn commit_name(ui: &Shared, value: &str, cx: &mut App) {
    let naming = ui.borrow_mut().naming.take();
    let name = value.trim().to_string();
    let views = ui.borrow().views.clone();
    views.notify(cx);
    if name.is_empty() {
        return;
    }
    let Some(service) = hub(cx).read(cx).clipboard_service.clone() else { return };
    match naming {
        Some(Naming::Rename(id)) => service.send(ClipboardCommand::RenameCollection { id, name }),
        Some(Naming::New { add }) => {
            let reply = service.request(ClipboardCommand::CreateCollection(name));
            let ui = ui.clone();
            cx.spawn(async move |cx| {
                let Ok(Ok(json)) = reply.recv().await else { return };
                let Some(id) = serde_json::from_str::<serde_json::Value>(&json).ok().and_then(|v| v["id"].as_u64()).and_then(|id| u32::try_from(id).ok()) else { return };
                cx.update(|cx| match add {
                    Some(entry) => service.send(ClipboardCommand::Collect { id: entry, collection: id, add: true }),
                    None => update(&ui, cx, |u| u.show(Filter::Collection(id))),
                });
            })
            .detach();
        }
        None => {}
    }
}

/// Settings the panel reads per render.
struct Look {
    click_pastes: bool,
    can_edit: bool,
    confirm_clear: bool,
    recording: bool,
    pinned_shown: usize,
    preview_lines: usize,
    kind_chips: bool,
}

impl Look {
    fn of(c: &ClipboardConfig) -> Self {
        Self {
            click_pastes: c.click_action == mandara_config::workflow::ClipClick::Paste,
            can_edit: !c.image_command.trim().is_empty(),
            confirm_clear: c.confirm_clear,
            recording: c.enabled,
            pinned_shown: c.pinned_shown,
            preview_lines: c.preview_lines(),
            kind_chips: c.kind_chips,
        }
    }
}

/// Wires a row's buttons to the service and the panel.
fn row_actions(ui: &Shared, e: &ClipEntry, look: &Look) -> RowActions {
    let (id, pinned, click_pastes) = (e.id, e.pinned, look.click_pastes);
    let ui_pick = ui.clone();
    RowActions {
        restore: Rc::new(move |alt, cx| {
            shell(cx).update(cx, |s, cx| s.set_panel(None, cx));
            send(cx, ClipboardCommand::Restore { id, paste: click_pastes != alt });
        }),
        pin: Rc::new(move |cx| send(cx, ClipboardCommand::Pin { id, pinned: !pinned })),
        delete: Rc::new(move |cx| send(cx, ClipboardCommand::Delete(id))),
        edit: Rc::new(move |cx| {
            shell(cx).update(cx, |s, cx| s.set_panel(None, cx));
            if let Err(err) = edit_image(id, cx) {
                tracing::warn!("clipboard edit: {err}");
            }
        }),
        pick: Rc::new(move |cx| update(&ui_pick, cx, |u| u.picker = if u.picker == Some(id) { None } else { Some(id) })),
    }
}

// ── The panel ────────────────────────────────────────────────────────────

struct View {
    p: PanelCx,
    ui: Shared,
    search: Entity<TextField>,
    name: Entity<TextField>,
    size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>,
    /// The panel's own focus, taken when it opens so the arrows work
    /// without a click first.
    focus: gpui::FocusHandle,
    /// Whether the panel was open on the last frame (take focus once).
    opened: Cell<bool>,
}

/// The list as laid out: what passes the filter and search, its pages and
/// the page on screen.
struct Layout<'a> {
    listing: Listing<'a>,
    pages: Vec<Range<usize>>,
    page: usize,
}

impl<'a> Layout<'a> {
    fn page_rows(&self) -> &[&'a ClipEntry] {
        &self.listing.main[self.pages[self.page].clone()]
    }

    fn shown(&self) -> Vec<&'a ClipEntry> {
        self.listing.pinned.iter().chain(self.page_rows()).copied().collect()
    }
}

/// Lays the list out for the panel's filter and `query` (search results
/// are cached until the query, filter or history change).
fn layout<'a>(ui: &Shared, entries: &'a [ClipEntry], collections: &[ClipCollection], look: &Look, query: &str) -> Layout<'a> {
    // A filter on a collection that was deleted falls back to all.
    let filter = {
        let mut u = ui.borrow_mut();
        if let Filter::Collection(id) = u.filter
            && !collections.iter().any(|c| c.id == id)
        {
            u.show(Filter::All);
        }
        u.filter
    };
    let listing = if query.trim().is_empty() {
        listing(entries, collections, filter, "", look.pinned_shown)
    } else {
        let key = (query.to_string(), filter, signature(entries, collections));
        let cached = ui.borrow().search.as_ref().filter(|s| s.key == key).map(|s| (s.pinned.clone(), s.main.clone()));
        let (pinned, main) = cached.unwrap_or_else(|| {
            let l = listing(entries, collections, filter, query, look.pinned_shown);
            let ids = (l.pinned.iter().map(|e| e.id).collect::<Vec<_>>(), l.main.iter().map(|e| e.id).collect::<Vec<_>>());
            ui.borrow_mut().search = Some(SearchCache { key, pinned: ids.0.clone(), main: ids.1.clone() });
            ids
        });
        let by_id: HashMap<u64, &ClipEntry> = entries.iter().map(|e| (e.id, e)).collect();
        Listing { pinned: pinned.iter().filter_map(|id| by_id.get(id).copied()).collect(), main: main.iter().filter_map(|id| by_id.get(id).copied()).collect() }
    };
    let heights: Vec<f32> = listing.main.iter().map(|e| row_h(e, look.preview_lines)).collect();
    let pages = paginate(&heights);
    let page = {
        let mut u = ui.borrow_mut();
        u.page = u.page.min(pages.len() - 1);
        u.page
    };
    Layout { listing, pages, page }
}

/// Derives the rows on screen and starts decoding their thumbnails.
fn refresh(ui: &Shared, look: &Look, query: &str, window: &Window, cx: &mut App) {
    let pending = {
        let state = &hub(cx).read(cx).clipboard;
        let l = layout(ui, &state.entries, &state.collections, look, query);
        sync_cache(ui, &state.entries, &l.shown(), look.preview_lines)
    };
    if !pending.is_empty() {
        decode_thumbs(ui, pending, window.scale_factor(), cx);
    }
}

impl View {
    fn render(&self, palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
        let ui = &self.ui;
        ui.borrow().views.track(window);
        // Take the keyboard as the panel opens, so Up / Down and Enter work
        // without a click; clicking a field takes focus from here.
        if self.p.progress() > 0.0 {
            if !self.opened.replace(true) {
                self.focus.focus(window, cx);
            }
        } else {
            self.opened.set(false);
        }
        let look = Look::of(&crate::state::config(cx).read(cx).get().clipboard);
        let query = self.search.read(cx).value.clone();
        if self.p.progress() > 0.0 {
            refresh(ui, &look, &query, window, cx);
        }
        let cx: &App = cx;
        let mono = crate::fonts::fonts(cx).mono;
        let state = &hub(cx).read(cx).clipboard;
        let (entries, collections) = (&state.entries, &state.collections);
        let lay = layout(ui, entries, collections, &look, &query);
        let (listing, pages, page) = (&lay.listing, &lay.pages, lay.page);
        // Snapshot the drawing order for the arrows: pinned rows first, then
        // the whole history, with the page each row sits on.
        ui.borrow_mut().reconcile_selection(&lay);
        let filter = ui.borrow().filter;
        let page_rows: Vec<&ClipEntry> = lay.page_rows().to_vec();
        let shown = lay.shown();
        let picker = ui.borrow().picker.filter(|id| shown.iter().any(|e| e.id == *id));
        let naming = ui.borrow().naming;
        let accent = readable(palette.primary, palette);
        let n = entries.len();
        let loose = entries.iter().filter(|e| !e.kept()).count();

        // Header.
        let armed = ui.borrow().confirm_until.is_some_and(|t| Instant::now() < t);
        let clear = (filter == Filter::All && loose > 0).then(|| {
            let (ui, confirm) = (ui.clone(), look.confirm_clear);
            pill_button("clip-clear", Some("trash"), if armed { t("Confirm clear") } else { t("Clear all") }, palette.urgent_coral, palette, move |_, cx| {
                let fire = {
                    let mut u = ui.borrow_mut();
                    let armed = u.confirm_until.is_some_and(|t| Instant::now() < t);
                    u.confirm_until = (confirm && !armed).then(|| Instant::now() + CONFIRM_WINDOW);
                    u.confirm_until.is_none()
                };
                if fire {
                    send(cx, ClipboardCommand::Clear { pinned: false });
                }
                update(&ui, cx, |_| {});
            })
            .into_any_element()
        });
        let subtitle: SharedString = if !look.recording {
            t("History paused").into()
        } else if n == 0 {
            t("History is empty").into()
        } else if look.click_pastes {
            tf("{n} items · right-click copies", &[("n", &n)]).into()
        } else {
            tf("{n} items · right-click pastes", &[("n", &n)]).into()
        };

        // Search, chips, naming.
        let search = {
            let field = self.search.clone();
            let views = ui.borrow().views.clone();
            let clear = (!query.is_empty()).then(|| {
                icon_button("clip-search-clear", "close", FIELD_BUTTON_PX, palette.on_surface_variant, None, move |_, cx| {
                    field.update(cx, |f, _| f.set(""));
                    views.notify(cx);
                })
                .into_any_element()
            });
            field_row("search", &self.search, clear.into_iter().collect(), palette)
        };
        let chips = chips_strip(ui, &self.name, entries, collections, look.kind_chips, palette);
        let naming_row = naming.map(|nm| {
            let (ui_save, field) = (ui.clone(), self.name.clone());
            let mut trailing = vec![
                icon_button("clip-name-save", "check", FIELD_BUTTON_PX, accent, Some(accent.with_alpha(alpha(SAVE_FILL, palette))), move |_, cx| {
                    let value = field.read(cx).value.clone();
                    field.update(cx, |f, _| f.set(""));
                    commit_name(&ui_save, &value, cx);
                })
                .into_any_element(),
            ];
            let ui_cancel = ui.clone();
            trailing.push(icon_button("clip-name-cancel", "close", FIELD_BUTTON_PX, palette.on_surface_variant, None, move |_, cx| update(&ui_cancel, cx, |u| u.naming = None)).into_any_element());
            let lead = if matches!(nm, Naming::Rename(_)) { "pencil" } else { "folder" };
            field_row(lead, &self.name, trailing, palette)
        });

        // The list.
        let row_with_picker = |e: &ClipEntry, index: usize, u: &Ui| -> AnyElement {
            let rcx = RowCx {
                index,
                text: u.rows.get(&e.id),
                thumb: u.thumbs.get(&e.id).map(|s| &s.thumb),
                mono: mono.clone(),
                preview_lines: look.preview_lines,
                can_edit: look.can_edit,
                picking: picker == Some(e.id),
                selected: u.selected == Some(e.id),
            };
            let row = entry_row(e, &rcx, palette, row_actions(ui, e, &look));
            if picker != Some(e.id) {
                return row;
            }
            let id = e.id;
            let toggle: Rc<dyn Fn(u32, bool, &mut App)> = Rc::new(move |collection, add, cx| send(cx, ClipboardCommand::Collect { id, collection, add }));
            let (ui_new, field) = (ui.clone(), self.name.clone());
            let new: Rc<dyn Fn(&mut Window, &mut App)> = Rc::new(move |window, cx| start_naming(&ui_new, &field, Naming::New { add: Some(id) }, "", window, cx));
            div().flex().flex_col().child(row).child(rows::picker(e, collections, palette, toggle, new)).into_any_element()
        };
        let picker_h = if picker.is_some() { PICKER_H } else { 0.0 };
        let block_h = |rows: &[&ClipEntry]| if rows.is_empty() { 0.0 } else { SECTION_H_PX + LIST_GAP + rows.iter().map(|e| row_h(e, look.preview_lines)).sum::<f32>() };
        let empty = listing.pinned.is_empty() && page_rows.is_empty();
        let list_h = if empty {
            EMPTY_H
        } else {
            block_h(&listing.pinned) + block_h(&page_rows) + picker_h + if listing.pinned.is_empty() || page_rows.is_empty() { 0.0 } else { GAP - LIST_GAP }
        };
        let controls_h = FIELD_H + CONTROL_GAP + CHIPS_H + if naming.is_some() { CONTROL_GAP + FIELD_H } else { 0.0 };
        self.size_cell.set(size(px(PANEL_W), px(PAD * 2.0 + HEADER_H + GAP + controls_h + GAP + list_h)));

        let body = if empty {
            let (title, hint) = match (n, filter, query.trim().is_empty()) {
                (0, _, _) => (t("Nothing copied yet"), t("Copied text and images appear here")),
                (_, Filter::Collection(_), true) => (t("This collection is empty"), t("Use the folder button on an entry to add it here")),
                _ => (t("No matches"), t("Try another word or filter")),
            };
            empty_state("clipboard", title, hint, EMPTY_H, palette).into_any_element()
        } else {
            let u = ui.borrow();
            let mut col = div().flex().flex_col().gap(px(LIST_GAP));
            if !listing.pinned.is_empty() {
                col = col.child(section_label(t("PINNED").into(), palette, None)).child(div().flex().flex_col().children(listing.pinned.iter().enumerate().map(|(i, e)| row_with_picker(e, i, &u))));
            }
            if !page_rows.is_empty() {
                let at = listing.pinned.len();
                let paged = (pages.len() > 1).then(|| pager(ui, pages, page, listing.main.len(), palette));
                let label: SharedString = match filter {
                    Filter::All => t("RECENT").into(),
                    Filter::Pinned => t("PINNED").into(),
                    Filter::Kind(k) => tr(kind_label(k)).to_uppercase().into(),
                    Filter::Collection(id) => collections.iter().find(|c| c.id == id).map(|c| c.name.to_uppercase()).unwrap_or_default().into(),
                };
                let trailing = match filter {
                    Filter::Collection(id) => Some(self.collection_actions(id, collections, paged, palette)),
                    _ => paged,
                };
                col = col
                    .when(!listing.pinned.is_empty(), |el| el.child(div().h(px(GAP - LIST_GAP))))
                    .child(section_label(label, palette, trailing))
                    .child(div().flex().flex_col().children(page_rows.iter().enumerate().map(|(i, e)| row_with_picker(e, at + i, &u))));
            }
            col.into_any_element()
        };
        let mut controls = div().flex().flex_col().gap(px(CONTROL_GAP)).child(search).child(chips);
        if let Some(row) = naming_row {
            controls = controls.child(row);
        }
        div()
            .id("clipboard-panel")
            .size_full()
            .p(px(PAD))
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .on_key_down({
                let ui = ui.clone();
                let focus = self.focus.clone();
                move |e: &KeyDownEvent, window, cx| {
                    if !focus.is_focused(window) { return; }
                    match e.keystroke.key.as_str() {
                    "up" => {
                        cx.stop_propagation();
                        select_step(&ui, -1, cx);
                    }
                    "down" => {
                        cx.stop_propagation();
                        select_step(&ui, 1, cx);
                    }
                    "enter" => {
                        cx.stop_propagation();
                        activate(&ui, cx);
                    }
                    _ => {}
                    }
                }
            })
            // The wheel turns the pages (the chips strip takes its own).
            .on_scroll({
                let ui = ui.clone();
                move |e: &ScrollEvent, window, cx| {
                    let line = f32::from(window.line_height());
                    let notches = match e.delta {
                        ScrollDelta::Lines(l) => l.y,
                        ScrollDelta::Pixels(p) => f32::from(p.y) / line,
                    };
                    wheel_step(&ui, notches, cx);
                }
            })
            // Nothing shrinks when the panel is taller than the screen.
            .child(div().flex_none().child(reveal(&self.p, 0, header("clipboard", accent, t("Clipboard"), subtitle, palette, clear))))
            .child(div().flex_none().h(px(GAP)))
            .child(div().flex_none().child(reveal(&self.p, 1, controls)))
            .child(div().flex_none().h(px(GAP)))
            .child(div().flex_none().child(reveal(&self.p, 2, body)))
            .into_any_element()
    }

    /// Rename / delete buttons for the collection on screen, then the pager.
    fn collection_actions(&self, id: u32, collections: &[ClipCollection], pager: Option<AnyElement>, palette: &MandaraPalette) -> AnyElement {
        let name = collections.iter().find(|c| c.id == id).map(|c| c.name.clone()).unwrap_or_default();
        let armed = self.ui.borrow().confirm_delete.is_some_and(|(c, until)| c == id && Instant::now() < until);
        let (ui_r, field) = (self.ui.clone(), self.name.clone());
        let ui_d = self.ui.clone();
        div()
            .flex()
            .items_center()
            .gap(px(PAGER_GAP_PX))
            .child(icon_button("clip-col-rename", "pencil", SECTION_BUTTON_PX, palette.on_surface_variant, None, move |window, cx| {
                start_naming(&ui_r, &field, Naming::Rename(id), &name, window, cx);
            }))
            .child(
                div()
                    .id("clip-col-delete")
                    .h(px(SECTION_BUTTON_PX))
                    .px(px(PAGER_PAD_X_PX))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .gap(px(PAGER_GAP_PX))
                    .cursor_pointer()
                    .text_size(px(text::LABEL))
                    .text_color(if armed { palette.urgent_coral.hsla() } else { palette.on_surface_variant.hsla() })
                    .hover(|s| s.bg(wash(WASH_SOFT, palette).hsla()))
                    .child(icon("trash", SECTION_ICON_PX, if armed { palette.urgent_coral } else { palette.on_surface_variant }))
                    .when(armed, |el| el.child(t("Delete collection")))
                    .on_click(move |_, _, cx| {
                        let armed = ui_d.borrow().confirm_delete.is_some_and(|(c, until)| c == id && Instant::now() < until);
                        if armed {
                            send(cx, ClipboardCommand::DeleteCollection(id));
                            update(&ui_d, cx, |u| {
                                u.confirm_delete = None;
                                u.show(Filter::All);
                            });
                        } else {
                            update(&ui_d, cx, |u| u.confirm_delete = Some((id, Instant::now() + CONFIRM_WINDOW)));
                        }
                    }),
            )
            .children(pager)
            .into_any_element()
    }
}

// ── Plugin and commands ──────────────────────────────────────────────────

/// Entries as JSON (what `clipboard list` and its siblings reply).
fn entries_json<'a>(entries: impl Iterator<Item = &'a ClipEntry>) -> serde_json::Value {
    super::json(entries.collect::<Vec<_>>())
}

/// The collection named by argument `i` (id or name).
fn collection_arg(a: &mandara_plugins::Args, i: usize, cx: &App) -> Result<ClipCollection, String> {
    let s = a.str(i)?;
    let state = &hub(cx).read(cx).clipboard;
    collection(s, &state.collections).cloned().ok_or_else(|| tf("No clipboard collection {name}", &[("name", &s)]))
}

fn entry_arg(a: &mandara_plugins::Args, i: usize) -> Result<u64, String> {
    a.str(i)?.parse().map_err(|_| t("id must be a number (see `clipboard list`)").to_string())
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Clipboard"), tk("Clipboard history sorted by kind, with collections"), Some(Zone::Right)), |window, cx, panel_cx| {
        let ui = Shared::default();
        let bar_ui = ui.clone();
        let bar = fn_view(
            None,
            move |palette, window, cx| {
                // Warm the first page's previews when something new is
                // copied, so the panel usually opens with them decoded.
                let newest = hub(cx).read(cx).clipboard.entries.first().map(|e| e.id);
                let warm = {
                    let mut u = bar_ui.borrow_mut();
                    let changed = u.newest != newest;
                    u.newest = newest;
                    changed && u.page == 0 && u.filter == Filter::All
                };
                if warm {
                    let look = Look::of(&crate::state::config(cx).read(cx).get().clipboard);
                    refresh(&bar_ui, &look, "", window, cx);
                }
                div().flex().items_center().child(icon("clipboard", crate::style::bar_icon(BAR_ICON_PX), palette.on_surface)).into_any_element()
            },
            window,
            cx,
        );
        // Drop decoded previews once the panel has closed.
        let follow = ui.clone();
        cx.observe(&shell(cx), move |shell, cx| {
            let open = shell.read(cx).panel.as_deref() == Some(ID);
            follow_panel(&follow, open, cx);
        })
        .detach();
        // Typing re-renders only this panel.
        let views = ui.borrow().views.clone();
        let search = cx.new(|cx| TextField::new(String::new(), t("Search text, kinds, collections…"), true, Rc::new(move |_, _, cx| views.notify(cx)), cx));
        let ui_name = ui.clone();
        let name = cx.new(|cx| TextField::new(String::new(), t("Collection name"), false, Rc::new(move |value, _, cx| commit_name(&ui_name, &value, cx)), cx).clear_on_commit());
        PANELS.with(|p| {
            let mut p = p.borrow_mut();
            p.retain(|(w, _)| w.strong_count() > 0);
            p.push((Rc::downgrade(&ui), search.clone()));
        });
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(INITIAL_H))));
        let view = View { p: panel_cx.clone(), ui, search, name, size_cell: size_cell.clone(), focus: cx.focus_handle(), opened: Cell::new(false) };
        let panel = fn_view(None, move |palette, window, cx| view.render(palette, window, cx), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), ..PluginInstance::empty() }
    });
    r.command(ID, CommandSpec::new("clipboard list", tk("Clipboard history, newest first, with each entry's kind and collections"), |_, cx| Ok(super::json(&hub(cx).read(cx).clipboard.entries))));
    r.command(
        ID,
        CommandSpec::new("clipboard search", tk("Entries matching every word: text, kind, language, host, app or collection"), |a, cx| {
            let state = &hub(cx).read(cx).clipboard;
            Ok(entries_json(state.entries.iter().filter(|e| e.matches(&a.rest(0), &state.collections))))
        })
        .arg(ArgSpec::required("query", ArgKind::String, tk("words to look for")))
        .example("clipboard search rust"),
    );
    r.command(
        ID,
        CommandSpec::new("clipboard kinds", tk("How many entries of each kind the history holds"), |_, cx| {
            let entries = &hub(cx).read(cx).clipboard.entries;
            let counts: serde_json::Map<String, serde_json::Value> = Kind::ALL.iter().map(|k| (k.id().to_string(), json!(entries.iter().filter(|e| e.class.kind() == *k).count()))).collect();
            Ok(serde_json::Value::Object(counts))
        }),
    );
    r.command(
        ID,
        CommandSpec::new("clipboard kind", tk("Entries of one kind, optionally matching a search"), |a, cx| {
            let kind = Kind::parse(a.str(0)?).ok_or_else(|| tf("Unknown kind {kind}", &[("kind", &a.str(0).unwrap_or_default())]))?;
            let state = &hub(cx).read(cx).clipboard;
            Ok(entries_json(state.entries.iter().filter(|e| e.class.kind() == kind && e.matches(&a.rest(1), &state.collections))))
        })
        .arg(ArgSpec::required("kind", ArgKind::Enum, tk("what to list")).choices(&["text", "link", "markdown", "code", "css", "color", "image", "files"]))
        .arg(ArgSpec::optional("query", ArgKind::String, tk("words to look for")))
        .example("clipboard kind color"),
    );
    r.command(
        ID,
        CommandSpec::new("clipboard collections", tk("Your clipboard collections and how many entries each holds"), |_, cx| {
            let state = &hub(cx).read(cx).clipboard;
            Ok(json!(state.collections.iter().map(|c| json!({ "id": c.id, "name": c.name, "created": c.created, "count": state.entries.iter().filter(|e| e.collections.contains(&c.id)).count() })).collect::<Vec<_>>()))
        }),
    );
    r.command(
        ID,
        CommandSpec::new_output("clipboard collection create", tk("Create a collection"), |a, cx| request(cx, ClipboardCommand::CreateCollection(a.rest(0))))
            .arg(ArgSpec::required("name", ArgKind::String, tk("collection name")))
            .example("clipboard collection create Palette"),
    );
    r.command(
        ID,
        CommandSpec::new_output("clipboard collection rename", tk("Rename a collection"), |a, cx| {
            let c = collection_arg(a, 0, cx)?;
            request(cx, ClipboardCommand::RenameCollection { id: c.id, name: a.rest(1) })
        })
        .arg(ArgSpec::required("collection", ArgKind::String, tk("collection id or name")))
        .arg(ArgSpec::required("name", ArgKind::String, tk("new name"))),
    );
    r.command(
        ID,
        CommandSpec::new_output("clipboard collection delete", tk("Delete a collection (its entries stay in the history)"), |a, cx| {
            let c = collection_arg(a, 0, cx)?;
            request(cx, ClipboardCommand::DeleteCollection(c.id))
        })
        .arg(ArgSpec::required("collection", ArgKind::String, tk("collection id or name"))),
    );
    for (path, summary, add) in [("clipboard collection add", tk("Add an entry to a collection"), true), ("clipboard collection remove", tk("Take an entry out of a collection"), false)] {
        r.command(
            ID,
            CommandSpec::new_output(path, summary, move |a, cx| {
                let c = collection_arg(a, 0, cx)?;
                let id = entry_arg(a, 1)?;
                request(cx, ClipboardCommand::Collect { id, collection: c.id, add })
            })
            .arg(ArgSpec::required("collection", ArgKind::String, tk("collection id or name")))
            .arg(ArgSpec::required("id", ArgKind::String, tk("entry id from `clipboard list`"))),
        );
    }
    r.command(
        ID,
        CommandSpec::new("clipboard collection show", tk("The entries of a collection, optionally matching a search"), |a, cx| {
            let c = collection_arg(a, 0, cx)?;
            let state = &hub(cx).read(cx).clipboard;
            Ok(entries_json(state.entries.iter().filter(|e| e.collections.contains(&c.id) && e.matches(&a.rest(1), &state.collections))))
        })
        .arg(ArgSpec::required("collection", ArgKind::String, tk("collection id or name")))
        .arg(ArgSpec::optional("query", ArgKind::String, tk("words to look for"))),
    );
    r.command(
        ID,
        CommandSpec::new("clipboard view", tk("Show a filter in the clipboard panel (all, pinned, a kind or a collection), with an optional search"), |a, cx| {
            let collections = hub(cx).read(cx).clipboard.collections.clone();
            let filter = Filter::parse(a.get(0).unwrap_or("all"), &collections).ok_or_else(|| tf("Unknown filter {filter}", &[("filter", &a.str(0).unwrap_or_default())]))?;
            let query = a.rest(1);
            let panels = PANELS.with(|p| p.borrow().iter().filter_map(|(w, f)| w.upgrade().map(|u| (u, f.clone()))).collect::<Vec<_>>());
            for (ui, field) in panels {
                field.update(cx, |f, _| f.set(&query));
                update(&ui, cx, |u| u.show(filter));
            }
            Ok(json!({ "filter": filter.describe(&collections), "query": query }))
        })
        .arg(ArgSpec::optional("filter", ArgKind::String, tk("all, pinned, a kind (code, color, …) or a collection id or name")))
        .arg(ArgSpec::optional("query", ArgKind::String, tk("words to look for")))
        .example("clipboard view color"),
    );
    r.command(
        ID,
        CommandSpec::new_output("clipboard copy", tk("Copy a history entry back (by id), optionally pasting it"), |a, cx| {
            let id = entry_arg(a, 0)?;
            request(cx, ClipboardCommand::Restore { id, paste: a.get(1) == Some("paste") })
        })
        .arg(ArgSpec::required("id", ArgKind::String, tk("entry id from `clipboard list`")))
        .arg(ArgSpec::optional("paste", ArgKind::Enum, tk("also paste with Ctrl+V")).choices(&["paste"])),
    );
    r.command(
        ID,
        CommandSpec::new_output("clipboard delete", tk("Remove one entry"), |a, cx| {
            let id: u64 = a.str(0)?.parse().map_err(|_| "id must be a number")?;
            request(cx, ClipboardCommand::Delete(id))
        })
        .arg(ArgSpec::required("id", ArgKind::String, tk("entry id"))),
    );
    r.command(
        ID,
        CommandSpec::new_output("clipboard clear", tk("Forget the history (pinned and collected entries stay unless `all`)"), |a, cx| request(cx, ClipboardCommand::Clear { pinned: a.get(0) == Some("all") }))
            .arg(ArgSpec::optional("scope", ArgKind::Enum, tk("`all` also forgets pinned and collected entries")).choices(&["all"])),
    );
    for (path, summary, pinned) in [("clipboard pin", tk("Pin an entry (survives clears)"), true), ("clipboard unpin", tk("Unpin an entry"), false)] {
        r.command(
            ID,
            CommandSpec::new_output(path, summary, move |a, cx| {
                let id = entry_arg(a, 0)?;
                request(cx, ClipboardCommand::Pin { id, pinned })
            })
            .arg(ArgSpec::required("id", ArgKind::String, tk("entry id"))),
        );
    }
    r.command(
        ID,
        CommandSpec::new("clipboard record", tk("Pause or resume recording what you copy"), |a, cx| {
            let on = match a.str(0)? {
                "on" => true,
                "off" => false,
                _ => !crate::state::config(cx).read(cx).get().clipboard.enabled,
            };
            crate::state::update_config(cx, |c| c.clipboard.enabled = on);
            Ok(json!({ "recording": on }))
        })
        .arg(ArgSpec::required("state", ArgKind::Enum, tk("on, off or toggle")).choices(&["on", "off", "toggle"])),
    );
    r.command(
        ID,
        CommandSpec::new("clipboard edit", tk("Open a copied image in your image editor (`[clipboard] image_command`)"), |a, cx| {
            let id = entry_arg(a, 0)?;
            edit_image(id, cx).map(|cmd| serde_json::json!({ "ran": cmd }))
        })
        .arg(ArgSpec::required("id", ArgKind::String, tk("entry id of an image"))),
    );
}

/// Runs the configured image command on entry `id` (`{path}` replaced by
/// the image file). Returns the command line.
fn edit_image(id: u64, cx: &mut App) -> Result<String, String> {
    use std::os::unix::process::CommandExt as _;
    let template = crate::state::config(cx).read(cx).get().clipboard.image_command.trim().to_string();
    if template.is_empty() {
        return Err(t("No image command set (Studio → Plugins → Clipboard)").to_string());
    }
    let entry = hub(cx).read(cx).clipboard.entries.iter().find(|e| e.id == id).cloned().ok_or_else(|| tf("No clipboard entry {id}", &[("id", &id)]))?;
    let ClipContent::Image { path, .. } = entry.content else { return Err(t("That entry is not an image").to_string()) };
    let quoted = format!("'{}'", path.display().to_string().replace('\'', "'\\''"));
    let line = if template.contains("{path}") { template.replace("'{path}'", "{path}").replace("\"{path}\"", "{path}").replace("{path}", &quoted) } else { format!("{template} {quoted}") };
    mandara_services::reaper::spawn(std::process::Command::new("sh").arg("-c").arg(&line).stdin(std::process::Stdio::null()).process_group(0)).map_err(|e| e.to_string())?;
    Ok(line)
}

/// Keeps the history service in step with `[clipboard]`, the focused app
/// and screen locks (clear on lock). The service itself runs only while
/// this plugin is on ([`crate::hub`] starts and stops it), so every hook
/// looks it up when it fires instead of holding it.
pub fn init(cx: &mut App) {
    fn send(command: ClipboardCommand, cx: &App) {
        if let Some(s) = hub(cx).read(cx).clipboard_service.clone() {
            s.send(command);
        }
    }
    let store = crate::state::config(cx);
    // A (re)started service gets the current settings and focused app.
    let mut last: Option<(ClipboardConfig, String)> = None;
    let mut sync = move |cx: &mut App| {
        let running = hub(cx).read(cx).clipboard_service.as_ref().map(|s| s.updates.clone());
        let Some(updates) = running else {
            last = None;
            LAST_SERVICE.with(|l| l.borrow_mut().take());
            return;
        };
        let config = crate::state::config(cx).read(cx).get().clipboard.clone();
        let focused = match crate::windows::windows(cx) {
            Some(w) => w.read(cx).state.active().map(|t| t.app_id.clone()).unwrap_or_default(),
            None => hub(cx).read(cx).focused.as_ref().map(|c| c.app_id.clone()).unwrap_or_default(),
        };
        // A different channel means a new service: resend everything.
        let fresh = LAST_SERVICE.with(|l| {
            let same = l.borrow().as_ref().is_some_and(|prev| prev.same_channel(&updates));
            if !same {
                *l.borrow_mut() = Some(updates);
            }
            !same
        });
        if fresh {
            last = None;
        }
        let (prev_config, prev_focused) = last.clone().unzip();
        if prev_config.as_ref() != Some(&config) {
            send(ClipboardCommand::Configure(config.clone()), cx);
        }
        if prev_focused.as_ref() != Some(&focused) {
            send(ClipboardCommand::FocusedApp(focused.clone()), cx);
        }
        last = Some((config, focused));
    };
    sync(cx);
    let sync = std::rc::Rc::new(std::cell::RefCell::new(sync));
    {
        let sync = sync.clone();
        cx.observe(&store, move |_, cx| sync.borrow_mut()(cx)).detach();
    }
    // Per-app exclusion needs the focused app at copy time: from the
    // window list, else the compositor's focus events (the hub also
    // notifies when the service starts or stops).
    if let Some(w) = crate::windows::windows(cx) {
        let sync = sync.clone();
        cx.observe(&w, move |_, cx| sync.borrow_mut()(cx)).detach();
    }
    cx.observe(&hub(cx), move |_, cx| sync.borrow_mut()(cx)).detach();
    // Clear on lock (logind lock requests: idle daemons, `loginctl
    // lock-session`, lid / suspend lockers).
    let (tx, rx) = async_channel::unbounded::<()>();
    mandara_services::logind::watch_lock_requests(move || {
        let _ = tx.try_send(());
    });
    cx.spawn(async move |cx| {
        while rx.recv().await.is_ok() {
            cx.update(|cx| {
                if crate::state::config(cx).read(cx).get().clipboard.clear_on_lock {
                    send(ClipboardCommand::Clear { pinned: false }, cx);
                }
            });
        }
    })
    .detach();
}

thread_local! {
    /// Update channel of the service [`init`] last configured.
    static LAST_SERVICE: std::cell::RefCell<Option<async_channel::Receiver<mandara_services::ClipboardState>>> = const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_services::clipboard::kind::classify_text;

    fn entry(id: u64, text: &str) -> ClipEntry {
        ClipEntry { id, content: ClipContent::Text { text: text.into() }, copied_at: 0, pinned: false, app_id: String::new(), class: classify_text(text), collections: Vec::new() }
    }

    #[test]
    fn previews_are_capped() {
        // 104×68 logical at 1.5× → 156×102 physical, under the cap.
        assert_eq!(thumb_px(1.5), (156, 102));
        // At 4× the longest side is capped.
        let (w, h) = thumb_px(4.0);
        assert_eq!(w as f32, MAX_THUMB_PX);
        assert!(h < w);
    }

    #[test]
    fn pages_fill_by_count_and_height() {
        assert_eq!((paginate(&[]).len(), paginate(&[]).first().cloned()), (1, Some(0..0)));
        assert_eq!(paginate(&[rows::TEXT_H; PAGE + 1]), [0..PAGE, PAGE..PAGE + 1]);
        // Tall rows make shorter pages; a row taller than the budget still
        // gets a page of its own.
        let tall = PAGE_BUDGET_H / 2.0 + 1.0;
        assert_eq!(paginate(&[tall, tall, tall]), [0..1, 1..2, 2..3]);
        assert_eq!((paginate(&[PAGE_BUDGET_H * 2.0]).len(), paginate(&[PAGE_BUDGET_H * 2.0]).first().cloned()), (1, Some(0..1)));
    }

    #[test]
    fn arrows_walk_the_rows_and_hold_at_the_ends() {
        let order = [10u64, 20, 30, 40];
        // Down from nothing starts at the top, Up at the bottom.
        assert_eq!(step_selection(&order, None, 1), Some((0, 10)));
        assert_eq!(step_selection(&order, None, -1), Some((3, 40)));
        assert_eq!(step_selection(&order, Some(30), 1), Some((3, 40)));
        assert_eq!(step_selection(&order, Some(30), -1), Some((1, 20)));
        // The ends hold instead of wrapping.
        assert_eq!(step_selection(&order, Some(10), -1), Some((0, 10)));
        assert_eq!(step_selection(&order, Some(40), 1), Some((3, 40)));
        // Nothing to walk.
        assert_eq!(step_selection(&[], None, 1), None);
    }

    #[test]
    fn the_first_arrow_starts_on_the_visible_page_even_with_pins() {
        let ui = Ui { order: vec![1, 10, 20, 30, 40], pinned_len: 1, page_of: vec![0, 1, 1, 2], page: 1, ..Default::default() };
        assert_eq!(ui.next_selection(1), Some((2, 20)));
        assert_eq!(ui.next_selection(-1), Some((3, 30)));
        let selected = Ui { selected: Some(30), ..ui };
        assert_eq!(selected.next_selection(1), Some((4, 40)), "selected arrows still cross pages");
    }

    #[test]
    fn scrolling_discards_single_page_distance_and_restarts_after_a_reversal() {
        let mut ui = Ui { pages: 1, ..Default::default() };
        ui.scroll_page(-10.0);
        assert_eq!(ui.wheel, 0.0);
        ui.pages = 3;
        ui.scroll_page(-WHEEL_NOTCHES_PER_PAGE / 2.0);
        assert_eq!(ui.page, 0);
        ui.scroll_page(WHEEL_NOTCHES_PER_PAGE / 2.0);
        assert_eq!(ui.wheel, WHEEL_NOTCHES_PER_PAGE / 2.0);
        ui.scroll_page(-WHEEL_NOTCHES_PER_PAGE / 2.0);
        assert_eq!(ui.page, 0);
        ui.scroll_page(-WHEEL_NOTCHES_PER_PAGE / 2.0);
        assert_eq!(ui.page, 1);
        ui.show(Filter::All);
        assert_eq!(ui.wheel, 0.0);
    }

    #[test]
    fn a_history_change_cannot_leave_an_invisible_entry_selected() {
        let entries = [entry(1, "pinned"), entry(10, "first"), entry(20, "second"), entry(30, "third")];
        let layout = Layout { listing: Listing { pinned: vec![&entries[0]], main: entries[1..].iter().collect() }, pages: vec![0..2, 2..3], page: 0 };
        let mut ui = Ui { selected: Some(30), ..Default::default() };
        ui.reconcile_selection(&layout);
        assert_eq!(ui.selected, None);
        for id in [1, 10, 20] {
            ui.selected = Some(id);
            ui.reconcile_selection(&layout);
            assert_eq!(ui.selected, Some(id), "visible rows keep their selection");
        }
    }

    #[test]
    fn filters_and_listing() {
        let cols = vec![ClipCollection { id: 4, name: "Palette".into(), created: 0 }];
        let mut list = vec![entry(1, "#cba6f7"), entry(2, "fn main() {\n    println!(\"hi\");\n}"), entry(3, "hello"), entry(5, "https://mandara.dev")];
        list[0].collections.push(4);
        list[2].pinned = true;
        assert_eq!(Filter::parse("colors", &cols), Some(Filter::Kind(Kind::Color)));
        assert_eq!(Filter::parse("palette", &cols), Some(Filter::Collection(4)));
        assert_eq!(Filter::parse("4", &cols), Some(Filter::Collection(4)));
        assert_eq!(Filter::parse("Pinned", &cols), Some(Filter::Pinned));
        assert_eq!(Filter::parse("nope", &cols), None);
        let ids = |l: Listing| (l.pinned.iter().map(|e| e.id).collect::<Vec<_>>(), l.main.iter().map(|e| e.id).collect::<Vec<_>>());
        assert_eq!(ids(listing(&list, &cols, Filter::All, "", 4)), (vec![3], vec![1, 2, 5]));
        assert_eq!(ids(listing(&list, &cols, Filter::All, "", 0)), (vec![], vec![1, 2, 5]));
        assert_eq!(ids(listing(&list, &cols, Filter::Kind(Kind::Code), "", 4)), (vec![], vec![2]));
        assert_eq!(ids(listing(&list, &cols, Filter::Collection(4), "", 4)), (vec![], vec![1]));
        assert_eq!(ids(listing(&list, &cols, Filter::Pinned, "", 4)), (vec![], vec![3]));
        // Search works across all of it: kinds, hosts, collection names.
        assert_eq!(ids(listing(&list, &cols, Filter::All, "palette", 4)), (vec![], vec![1]));
        assert_eq!(ids(listing(&list, &cols, Filter::All, "mandara.dev", 4)), (vec![], vec![5]));
        assert_eq!(ids(listing(&list, &cols, Filter::All, "rust", 4)), (vec![], vec![2]));
        assert_eq!(ids(listing(&list, &cols, Filter::All, "hel", 4)), (vec![3], vec![]));
        assert_ne!(signature(&list, &cols), signature(&list[1..], &cols));
    }
}
