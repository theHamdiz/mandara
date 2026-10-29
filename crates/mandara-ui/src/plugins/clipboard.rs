//! Clipboard history utility: bar button; a panel listing pinned entries
//! above recent text and image entries — images with large rounded
//! previews — where click copies (or pastes, per `[clipboard] click_action`),
//! right-click does the other, the pin keeps an entry through clears, the
//! pencil opens an image in your editor and the trash forgets an entry.
//! [`init`] keeps the history service in step with `[clipboard]`, the
//! focused app (per-app exclusion) and screen locks.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{Local, TimeZone};
use gpui::{
    AnyElement, App, FontWeight, ImageSource, Task, prelude::FluentBuilder as _, InteractiveElement, IntoElement, MouseButton, ObjectFit, ParentElement, RenderImage, SharedString,
    StatefulInteractiveElement, Styled, StyledImage as _, Window, div, img, px, size,
};
use mandara_config::workflow::ClipboardConfig;
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tk};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, Output, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::clipboard::{ClipContent, ClipEntry, ClipboardCommand};
use mandara_theme::MandaraPalette;

use super::panel_fx::{HEADER_H, SECTION_H_PX, ViewSet, WASH_SOFT, alpha, empty_state, header, pill_button, readable, section, wash};
use super::{fn_view, json};
use crate::hub::hub;
use crate::icons::icon;
use crate::kit::{icon_button, reveal, text, tokens};
use crate::state::shell;

pub const ID: &str = "clipboard";
/// History rows per page.
const PAGE: usize = 8;
/// "Clear all" stays armed for a second click this long.
const CONFIRM_WINDOW: Duration = Duration::from_secs(3);
/// Pin / edit buttons.
const ROW_BUTTON_PX: f32 = 28.0;
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
const TEXT_H: f32 = 52.0;
const IMAGE_H: f32 = 84.0;
const EMPTY_H: f32 = 170.0;
/// Panel height before the first render measures the list.
const INITIAL_H: f32 = 260.0;
/// Gap between the section label and the list.
const LIST_GAP: f32 = tokens::XS;
/// Bar glyph size.
const BAR_ICON_PX: f32 = 15.0;

// Rows.
const ROW_PAD_X_PX: f32 = 8.0;
const ROW_RADIUS_PX: f32 = 12.0;
const ROW_GAP_PX: f32 = 12.0;
const ROW_TEXT_GAP_PX: f32 = 2.0;
/// Row title: half a step above the small body size.
const ROW_TITLE_PX: f32 = text::BODY_S + 0.5;
/// Image preview tile.
const THUMB_W_PX: f32 = 104.0;
const THUMB_H_PX: f32 = IMAGE_H - 16.0;
const THUMB_RADIUS_PX: f32 = 10.0;
/// Text-kind badge.
const BADGE_PX: f32 = 34.0;
const BADGE_RADIUS_PX: f32 = 10.0;
const BADGE_ICON_PX: f32 = 15.0;
const BADGE_ALPHA: (f32, f32) = (0.14, 0.1);
/// Delete button.
const DELETE_PX: f32 = 28.0;
const DELETE_ALPHA: f32 = 0.8;
/// Bytes per RGBA / BGRA pixel.
const RGBA_CHANNELS: usize = 4;
/// Characters of a text entry shown as its title.
const PREVIEW_CHARS: usize = 90;

/// Seconds per minute / hour / day for the "copied … ago" label.
const SECS_PER_MIN: i64 = 60;
const SECS_PER_HOUR: i64 = 60 * SECS_PER_MIN;
const SECS_PER_DAY: i64 = 24 * SECS_PER_HOUR;

fn request(cx: &mut App, command: ClipboardCommand) -> Result<Output, String> {
    let s = hub(cx).read(cx).clipboard_service.clone().ok_or("clipboard service unavailable")?;
    Ok(Output::Pending(s.request(command)))
}

fn send(cx: &mut App, command: ClipboardCommand) {
    if let Some(s) = hub(cx).read(cx).clipboard_service.clone() {
        s.send(command);
    }
}

fn ago(ts: i64) -> String {
    let secs = Local::now().timestamp() - ts;
    match secs {
        ..SECS_PER_MIN => t("now").to_string(),
        SECS_PER_MIN..SECS_PER_HOUR => tf("{n}m", &[("n", &(secs / SECS_PER_MIN))]),
        SECS_PER_HOUR..SECS_PER_DAY => tf("{n}h", &[("n", &(secs / SECS_PER_HOUR))]),
        _ => Local.timestamp_opt(ts, 0).single().map(|d| d.format("%d %b").to_string()).unwrap_or_default(),
    }
}

fn row_h(e: &ClipEntry) -> f32 {
    match e.content {
        ClipContent::Image { .. } => IMAGE_H,
        ClipContent::Text { .. } => TEXT_H,
    }
}

/// What kind of text this looks like (for the leading badge).
fn text_kind(text: &str) -> (&'static str, &'static str) {
    let s = text.trim();
    if s.starts_with("http://") || s.starts_with("https://") {
        ("arrow", t("Link"))
    } else if !s.is_empty() && s.chars().all(|c| c.is_ascii_digit() || " +-().".contains(c)) {
        ("keyboard", t("Number"))
    } else if s.contains('\n') && (s.contains('{') || s.contains(';') || s.contains("fn ") || s.contains("def ")) {
        ("text-scan", t("Code"))
    } else {
        ("note", t("Text"))
    }
}

// ── Per-entry caches ─────────────────────────────────────────────────────

/// Everything about an entry that is costly to derive (a whitespace-folded
/// preview and a char count of possibly huge text), computed once per entry
/// instead of every frame.
struct RowText {
    title: SharedString,
    kind_icon: &'static str,
    kind: &'static str,
    chars: usize,
    mime: SharedString,
}

impl RowText {
    fn of(e: &ClipEntry) -> Self {
        match &e.content {
            ClipContent::Image { mime, .. } => {
                Self { title: t("Image").into(), kind_icon: "image", kind: t("Image"), chars: 0, mime: mime.trim_start_matches("image/").to_uppercase().into() }
            }
            ClipContent::Text { text } => {
                let (kind_icon, kind) = text_kind(text);
                Self { title: e.preview(PREVIEW_CHARS).into(), kind_icon, kind, chars: text.chars().count(), mime: SharedString::default() }
            }
        }
    }
}

/// A downscaled image preview, decoded off the UI thread.
enum Thumb {
    Loading,
    Ready(Arc<RenderImage>),
    Failed,
}

/// A preview plus its LRU bookkeeping.
struct ThumbSlot {
    thumb: Thumb,
    /// Decoded size in bytes (0 until ready).
    bytes: usize,
    /// [`Cache::tick`] of the last render that showed it.
    used: u64,
}

/// Row texts and thumbnails, keyed by entry id. Previews are decoded only
/// for the page on screen, at the tile's physical size capped by
/// [`MAX_THUMB_PX`] (never the full screenshot); their total stays under
/// [`THUMB_BUDGET_BYTES`] (least recently shown evicted first), and they
/// are all dropped shortly after the panel closes — rows keep same-size
/// placeholders, so reopening never reflows.
#[derive(Default)]
struct Cache {
    rows: HashMap<u64, RowText>,
    thumbs: HashMap<u64, ThumbSlot>,
    /// Render counter for LRU.
    tick: u64,
    /// Page of the history on screen (0 = newest).
    page: usize,
    /// Newest entry the bar chip has pre-decoded for.
    newest: Option<u64>,
    /// Pending "drop previews after close" timer (dropping it cancels).
    drop_after_close: Option<Task<()>>,
    /// Views to re-render when a thumbnail lands.
    views: ViewSet,
    /// "Clear all" was clicked once (confirm before this instant).
    confirm_until: Option<Instant>,
}

type SharedCache = Rc<RefCell<Cache>>;

impl Cache {
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
        self.page = 0;
    }
}

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
    for px in rgba.chunks_exact_mut(RGBA_CHANNELS) {
        px.swap(0, 2);
    }
    Some(Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(rgba)])))
}

/// The items of `page`.
fn page_of<T>(items: &[T], page: usize) -> &[T] {
    let start = (page * PAGE).min(items.len());
    &items[start..(start + PAGE).min(items.len())]
}

/// Pinned entries (newest first) and the rest.
fn split(entries: &[ClipEntry]) -> (Vec<&ClipEntry>, Vec<&ClipEntry>) {
    entries.iter().partition(|e| e.pinned)
}

/// What the panel shows: the first `pinned_shown` pinned entries
/// (`clipboard.pinned_shown`), then `page` of the rest.
fn shown_entries(entries: &[ClipEntry], page: usize, pinned_shown: usize) -> Vec<&ClipEntry> {
    let (pinned, loose) = split(entries);
    pinned.into_iter().take(pinned_shown).chain(page_of(&loose, page).iter().copied()).collect()
}

/// Pages in a history of `n` entries.
fn page_count(n: usize) -> usize {
    n.div_ceil(PAGE).max(1)
}

/// Brings the caches in line with the page on screen: drops texts of other
/// rows and previews of forgotten entries, derives missing row texts and
/// returns the images that still need a preview (marked as loading).
fn sync_cache(cache: &SharedCache, entries: &[ClipEntry], pinned_shown: usize) -> Vec<(u64, PathBuf)> {
    let mut c = cache.borrow_mut();
    c.page = c.page.min(page_count(entries.iter().filter(|e| !e.pinned).count()) - 1);
    c.tick += 1;
    let tick = c.tick;
    let shown = shown_entries(entries, c.page, pinned_shown);
    c.rows.retain(|id, _| shown.iter().any(|e| e.id == *id));
    c.thumbs.retain(|id, _| entries.iter().any(|e| e.id == *id));
    let mut pending = Vec::new();
    for e in shown {
        c.rows.entry(e.id).or_insert_with(|| RowText::of(e));
        let ClipContent::Image { path, .. } = &e.content else { continue };
        match c.thumbs.get_mut(&e.id) {
            Some(slot) => slot.used = tick,
            None => {
                c.thumbs.insert(e.id, ThumbSlot { thumb: Thumb::Loading, bytes: 0, used: tick });
                pending.push((e.id, path.clone()));
            }
        }
    }
    pending
}

/// Decodes previews off the UI thread at `scale` and re-renders the panel
/// as each lands.
fn decode_thumbs(cache: &SharedCache, pending: Vec<(u64, PathBuf)>, scale: f32, cx: &mut App) {
    let (w, h) = thumb_px(scale);
    for (id, path) in pending {
        let task = cx.background_executor().spawn(async move { decode_thumb(path, w, h) });
        let cache = cache.clone();
        cx.spawn(async move |cx| {
            let decoded = task.await;
            let views = {
                let mut c = cache.borrow_mut();
                // Ignore results for entries dropped meanwhile (closed
                // panel, evicted, forgotten).
                if let Some(slot) = c.thumbs.get_mut(&id) {
                    match decoded {
                        Some(image) => {
                            slot.bytes = w as usize * h as usize * RGBA_CHANNELS;
                            slot.thumb = Thumb::Ready(image);
                        }
                        None => slot.thumb = Thumb::Failed,
                    }
                    let keep: Vec<u64> = c.thumbs.iter().filter(|(_, t)| t.used == c.tick).map(|(id, _)| *id).collect();
                    c.enforce_budget(&keep);
                }
                c.views.clone()
            };
            views.notify_async(cx);
        })
        .detach();
    }
}

/// Syncs the caches with the hub's history and starts any decodes.
fn refresh_cache(cache: &SharedCache, scale: f32, cx: &mut App) {
    let pinned_shown = crate::state::config(cx).read(cx).get().clipboard.pinned_shown;
    let pending = sync_cache(cache, &hub(cx).read(cx).clipboard.entries, pinned_shown);
    if !pending.is_empty() {
        decode_thumbs(cache, pending, scale, cx);
    }
}

/// Follows the panel's open state: cancels a pending drop when it opens,
/// schedules dropping every preview once it has closed (after the close
/// animation, so it never flashes placeholders).
fn follow_panel(cache: &SharedCache, open: bool, cx: &mut App) {
    let mut c = cache.borrow_mut();
    if open {
        c.drop_after_close = None;
        return;
    }
    if c.thumbs.is_empty() && c.page == 0 || c.drop_after_close.is_some() {
        return;
    }
    let weak = Rc::downgrade(cache);
    c.drop_after_close = Some(cx.spawn(async move |cx| {
        cx.background_executor().timer(DROP_AFTER_CLOSE).await;
        if let Some(cache) = weak.upgrade() {
            let mut c = cache.borrow_mut();
            c.drop_previews();
            c.drop_after_close = None;
        }
    }));
}

/// Per-render options of the rows.
#[derive(Copy, Clone)]
struct RowOptions {
    /// Click pastes (right-click copies) instead of the other way round.
    click_pastes: bool,
    /// An image editor command is set (shows the edit button on images).
    can_edit: bool,
}

fn entry_row(i: usize, e: &ClipEntry, cache: &Cache, palette: &MandaraPalette, opts: RowOptions) -> AnyElement {
    let id = e.id;
    let pinned = e.pinned;
    let is_image = matches!(e.content, ClipContent::Image { .. });
    let accent = readable(palette.primary, palette);
    let hover = wash(WASH_SOFT, palette);
    let row = cache.rows.get(&id);
    let (title, meta) = match (row, &e.content) {
        (Some(r), ClipContent::Image { .. }) => (r.title.clone(), SharedString::from(format!("{} · {}", r.mime, ago(e.copied_at)))),
        (Some(r), ClipContent::Text { .. }) => {
            (r.title.clone(), tf("{kind} · {chars} chars · {ago}", &[("kind", &r.kind), ("chars", &r.chars), ("ago", &ago(e.copied_at))]).into())
        }
        (None, _) => (SharedString::default(), SharedString::default()),
    };
    let lead = match &e.content {
        ClipContent::Image { .. } => {
            let tile = div().w(px(THUMB_W_PX)).h(px(THUMB_H_PX)).flex_none().rounded(px(crate::style::radius(THUMB_RADIUS_PX)));
            match cache.thumbs.get(&id) {
                Some(ThumbSlot { thumb: Thumb::Ready(image), .. }) => {
                    img(ImageSource::Render(image.clone())).w(px(THUMB_W_PX)).h(px(THUMB_H_PX)).flex_none().rounded(px(crate::style::radius(THUMB_RADIUS_PX))).object_fit(ObjectFit::Cover).into_any_element()
                }
                // Same-size skeleton while decoding: the row never reflows.
                Some(ThumbSlot { thumb: Thumb::Loading, .. }) | None => tile.bg(wash(WASH_SOFT, palette).hsla()).into_any_element(),
                Some(ThumbSlot { thumb: Thumb::Failed, .. }) => tile.bg(wash(WASH_SOFT, palette).hsla()).flex().items_center().justify_center().child(icon("image", BADGE_ICON_PX, palette.on_surface_variant)).into_any_element(),
            }
        }
        ClipContent::Text { .. } => div()
            .size(px(BADGE_PX))
            .flex_none()
            .rounded(px(crate::style::radius(BADGE_RADIUS_PX)))
            .bg(accent.with_alpha(alpha(BADGE_ALPHA, palette)).hsla())
            .flex()
            .items_center()
            .justify_center()
            .child(icon(row.map_or("note", |r| r.kind_icon), BADGE_ICON_PX, accent))
            .into_any_element(),
    };
    div()
        .id(("clip", i))
        .h(px(row_h(e)))
        .px(px(ROW_PAD_X_PX))
        .rounded(px(crate::style::radius(ROW_RADIUS_PX)))
        .flex()
        .items_center()
        .gap(px(ROW_GAP_PX))
        .cursor_pointer()
        .hover(move |s| s.bg(hover.hsla()))
        .child(lead)
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .gap(px(ROW_TEXT_GAP_PX))
                .child(div().text_size(px(ROW_TITLE_PX)).font_weight(FontWeight::MEDIUM).truncate().child(title))
                .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(meta)),
        )
        .when(is_image && opts.can_edit, |el| {
            el.child(icon_button(("clip-edit", i), "crop", ROW_BUTTON_PX, palette.on_surface_variant.with_alpha(DELETE_ALPHA), None, move |_, cx| {
                cx.stop_propagation();
                shell(cx).update(cx, |s, cx| s.set_panel(None, cx));
                if let Err(err) = edit_image(id, cx) {
                    tracing::warn!("clipboard edit: {err}");
                }
            }))
        })
        .child(icon_button(
            ("clip-pin", i),
            if pinned { "pin-off" } else { "pin" },
            ROW_BUTTON_PX,
            if pinned { readable(palette.primary, palette) } else { palette.on_surface_variant.with_alpha(DELETE_ALPHA) },
            None,
            move |_, cx| {
                cx.stop_propagation();
                send(cx, ClipboardCommand::Pin { id, pinned: !pinned });
            },
        ))
        .child(icon_button(("clip-del", i), "trash", DELETE_PX, palette.on_surface_variant.with_alpha(DELETE_ALPHA), None, move |_, cx| {
            // Don't let the row's click (copy) fire too.
            cx.stop_propagation();
            send(cx, ClipboardCommand::Delete(id));
        }))
        .on_click(move |_, _, cx| {
            shell(cx).update(cx, |s, cx| s.set_panel(None, cx));
            send(cx, ClipboardCommand::Restore { id, paste: opts.click_pastes });
        })
        .on_mouse_down(MouseButton::Right, move |_, _, cx| {
            shell(cx).update(cx, |s, cx| s.set_panel(None, cx));
            send(cx, ClipboardCommand::Restore { id, paste: !opts.click_pastes });
        })
        .into_any_element()
}

/// `‹ 9–16 / 40 ›`: steps through older pages of the history.
fn pager(cache: &SharedCache, page: usize, n: usize, palette: &MandaraPalette) -> AnyElement {
    let last = page_count(n) - 1;
    let step = |id: &'static str, glyph: &'static str, enabled: bool, to: usize| {
        let cache = cache.clone();
        div()
            .id(id)
            .px(px(PAGER_PAD_X_PX))
            .rounded_full()
            .text_color(if enabled { palette.on_surface.hsla() } else { palette.on_surface_variant.with_alpha(PAGER_DISABLED_ALPHA).hsla() })
            .when(enabled, |el| {
                el.cursor_pointer().hover(|s| s.bg(wash(WASH_SOFT, palette).hsla())).on_click(move |_, _, cx| {
                    let views = {
                        let mut c = cache.borrow_mut();
                        c.page = to;
                        c.views.clone()
                    };
                    views.notify(cx);
                })
            })
            .child(glyph)
    };
    let first = page * PAGE + 1;
    let to = ((page + 1) * PAGE).min(n);
    div()
        .flex()
        .items_center()
        .gap(px(PAGER_GAP_PX))
        .font_features(crate::fonts::tabular_nums())
        .text_size(px(text::LABEL))
        .text_color(palette.on_surface_variant.hsla())
        .child(step("clip-newer", "‹", page > 0, page.saturating_sub(1)))
        .child(format!("{first}–{to} / {n}"))
        .child(step("clip-older", "›", page < last, (page + 1).min(last)))
        .into_any_element()
}

fn panel_view(p: PanelCx, size_cell: Rc<Cell<gpui::Size<gpui::Pixels>>>, cache: SharedCache) -> impl Fn(&MandaraPalette, &mut Window, &mut App) -> AnyElement {
    move |palette, window, cx| {
        cache.borrow().views.track(window);
        refresh_cache(&cache, window.scale_factor(), cx);
        let cx: &App = cx;
        let entries = &hub(cx).read(cx).clipboard.entries;
        let settings = &crate::state::config(cx).read(cx).get().clipboard;
        let opts = RowOptions { click_pastes: settings.click_action == mandara_config::workflow::ClipClick::Paste, can_edit: !settings.image_command.trim().is_empty() };
        let (confirm_clear, recording) = (settings.confirm_clear, settings.enabled);
        let cache_ref = cache.clone();
        let cache = cache.borrow();
        let accent = readable(palette.primary, palette);
        let (pinned, loose) = split(entries);
        let pinned: Vec<&ClipEntry> = pinned.into_iter().take(settings.pinned_shown).collect();
        let page = page_of(&loose, cache.page);
        let block = |rows: &[&ClipEntry]| if rows.is_empty() { 0.0 } else { SECTION_H_PX + LIST_GAP + rows.iter().map(|e| row_h(e)).sum::<f32>() };
        let list_h: f32 = if pinned.is_empty() && page.is_empty() { EMPTY_H } else { block(&pinned) + block(page) + if pinned.is_empty() || page.is_empty() { 0.0 } else { GAP } };
        size_cell.set(size(px(PANEL_W), px(PAD * 2.0 + HEADER_H + GAP + list_h)));
        let n = entries.len();
        let armed = cache.confirm_until.is_some_and(|t| Instant::now() < t);
        let clear_cache = cache_ref.clone();
        let clear = (!loose.is_empty()).then(|| {
            pill_button("clip-clear", Some("trash"), if armed { t("Confirm clear") } else { t("Clear all") }, palette.urgent_coral, palette, move |_, cx| {
                let views = {
                    let mut c = clear_cache.borrow_mut();
                    let armed = c.confirm_until.is_some_and(|t| Instant::now() < t);
                    if confirm_clear && !armed {
                        c.confirm_until = Some(Instant::now() + CONFIRM_WINDOW);
                    } else {
                        c.confirm_until = None;
                        send(cx, ClipboardCommand::Clear { pinned: false });
                    }
                    c.views.clone()
                };
                views.notify(cx);
            })
            .into_any_element()
        });
        let subtitle: SharedString = if !recording {
            t("History paused").into()
        } else if n == 0 {
            t("History is empty").into()
        } else if opts.click_pastes {
            tf("{n} items · right-click copies", &[("n", &n)]).into()
        } else {
            tf("{n} items · right-click pastes", &[("n", &n)]).into()
        };
        let body = if pinned.is_empty() && page.is_empty() {
            empty_state("clipboard", t("Nothing copied yet"), t("Copied text and images appear here"), EMPTY_H, palette).into_any_element()
        } else {
            let mut col = div().flex().flex_col().gap(px(LIST_GAP));
            if !pinned.is_empty() {
                col = col
                    .child(section(t("PINNED"), palette, None))
                    .child(div().flex().flex_col().children(pinned.iter().enumerate().map(|(i, e)| entry_row(i, e, &cache, palette, opts))));
            }
            if !page.is_empty() {
                let at = pinned.len();
                col = col
                    .when(!pinned.is_empty(), |el| el.child(div().h(px(GAP - LIST_GAP))))
                    .child(section(t("RECENT"), palette, (loose.len() > PAGE).then(|| pager(&cache_ref, cache.page, loose.len(), palette))))
                    .child(div().flex().flex_col().children(page.iter().enumerate().map(|(i, e)| entry_row(at + i, e, &cache, palette, opts))));
            }
            col.into_any_element()
        };
        div()
            .size_full()
            .p(px(PAD))
            .flex()
            .flex_col()
            .child(reveal(&p, 0, header("clipboard", accent, t("Clipboard"), subtitle, palette, clear)))
            .child(div().h(px(GAP)))
            .child(reveal(&p, 1, body))
            .into_any_element()
    }
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Clipboard"), tk("Clipboard history with images"), Some(Zone::Right)), |window, cx, panel_cx| {
        let cache = SharedCache::default();
        let bar_cache = cache.clone();
        let bar = fn_view(
            None,
            move |palette, window, cx| {
                // Warm the first page's previews when something new is
                // copied, so the panel usually opens with them decoded.
                let newest = hub(cx).read(cx).clipboard.entries.first().map(|e| e.id);
                let changed = {
                    let mut c = bar_cache.borrow_mut();
                    let changed = c.newest != newest;
                    c.newest = newest;
                    changed && c.page == 0
                };
                if changed {
                    refresh_cache(&bar_cache, window.scale_factor(), cx);
                }
                div().flex().items_center().child(icon("clipboard", crate::style::bar_icon(BAR_ICON_PX), palette.on_surface)).into_any_element()
            },
            window,
            cx,
        );
        // Drop decoded previews once the panel has closed.
        let follow = cache.clone();
        cx.observe(&shell(cx), move |shell, cx| {
            let open = shell.read(cx).panel.as_deref() == Some(ID);
            follow_panel(&follow, open, cx);
        })
        .detach();
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(INITIAL_H))));
        let panel = fn_view(None, panel_view(panel_cx.clone(), size_cell.clone(), cache), window, cx);
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), ..PluginInstance::empty() }
    });
    r.command(ID, CommandSpec::new("clipboard list", tk("Clipboard history, newest first"), |_, cx| Ok(json(&hub(cx).read(cx).clipboard.entries))));
    r.command(
        ID,
        CommandSpec::new_output("clipboard copy", tk("Copy a history entry back (by id), optionally pasting it"), |a, cx| {
            let id: u64 = a.str(0)?.parse().map_err(|_| "id must be a number (see `clipboard list`)")?;
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
        CommandSpec::new_output("clipboard clear", tk("Forget the history (pinned entries stay unless `all`)"), |a, cx| request(cx, ClipboardCommand::Clear { pinned: a.get(0) == Some("all") }))
            .arg(ArgSpec::optional("scope", ArgKind::Enum, tk("`all` also forgets pinned entries")).choices(&["all"])),
    );
    for (path, summary, pinned) in [("clipboard pin", tk("Pin an entry (survives clears)"), true), ("clipboard unpin", tk("Unpin an entry"), false)] {
        r.command(
            ID,
            CommandSpec::new_output(path, summary, move |a, cx| {
                let id: u64 = a.str(0)?.parse().map_err(|_| "id must be a number (see `clipboard list`)")?;
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
            Ok(json(serde_json::json!({ "recording": on })))
        })
        .arg(ArgSpec::required("state", ArgKind::Enum, tk("on, off or toggle")).choices(&["on", "off", "toggle"])),
    );
    r.command(
        ID,
        CommandSpec::new("clipboard edit", tk("Open a copied image in your image editor (`[clipboard] image_command`)"), |a, cx| {
            let id: u64 = a.str(0)?.parse().map_err(|_| "id must be a number (see `clipboard list`)")?;
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

    #[test]
    fn previews_are_capped_and_paged() {
        // 104×68 logical at 1.5× → 156×102 physical, under the cap.
        assert_eq!(thumb_px(1.5), (156, 102));
        // At 4× the longest side is capped.
        let (w, h) = thumb_px(4.0);
        assert_eq!(w as f32, MAX_THUMB_PX);
        assert!(h < w);
        assert_eq!(page_count(0), 1);
        assert_eq!(page_count(PAGE + 1), 2);
    }

    #[test]
    fn classifies_text_entries() {
        assert_eq!(text_kind("https://example.com").0, "arrow");
        assert_eq!(text_kind("+20 100 123 4567").0, "keyboard");
        assert_eq!(text_kind("fn main() {\n}").0, "text-scan");
        assert_eq!(text_kind("hello").0, "note");
    }
}
