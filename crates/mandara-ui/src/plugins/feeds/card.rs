//! The Feeds sidebar card: the newest unread articles across your feeds
//! (or one folder), each with its feed's monogram and colour, how long
//! ago it came out and an unread dot. A click opens it in the panel's
//! reader. The card never downloads: it reads the open panel's history,
//! or the saved history on disk (off the UI thread) when the panel is
//! closed, and lets go of its rows when the sidebar hides.

use std::rc::Rc;

use gpui::{AnyElement, App, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_services::feeds::{Cache, Subscription};
use mandara_theme::{Color, MandaraPalette};

use super::super::extras::{monogram, name_color};
use super::super::panel_fx::{WASH_SOFT, pill_button, readable, wash};
use super::library::{Filter, Library, Scope, host};
use super::panel::ago;
use super::{ID, cached_settings, hub};
use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::kit::{card, text, tokens};

/// Articles the card lists at most.
pub const CARD_ITEMS: usize = 5;
const ROW_RADIUS: f32 = 12.0;
const ROW_PAD_X: f32 = 8.0;
const ROW_PAD_Y: f32 = 7.0;
const MONOGRAM: f32 = 22.0;
const HEAD_ICON: f32 = 15.0;
const DOT: f32 = 6.0;
const META_GAP: f32 = 5.0;
const COUNT_H: f32 = 18.0;
const COUNT_PAD_X: f32 = 7.0;

/// One row of the card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Headline {
    pub url: String,
    pub link: String,
    pub title: String,
    pub feed: String,
    /// Your colour for the feed (`#rrggbb`), or empty for the automatic one.
    pub color: String,
    pub when: i64,
}

/// What the card shows: cached rows with the data revision and folder
/// they were made for.
#[derive(Default)]
pub(super) struct CardData {
    /// `(history revision, subscriptions fingerprint, folder)` of the rows.
    key: Option<(u64, u64, Rc<str>)>,
    pub items: Rc<[Headline]>,
    pub unread: usize,
    loading: bool,
    /// Bumped when the rows are released; a late load is then ignored.
    generation: u64,
}

impl CardData {
    /// Let go of the rows (the sidebar hid).
    pub fn release(&mut self) {
        self.key = None;
        self.items = Rc::from([]);
        self.unread = 0;
        self.loading = false;
        self.generation = self.generation.wrapping_add(1);
    }
}

fn scope(folder: &str) -> Scope { if folder.is_empty() { Scope::All } else { Scope::Folder(folder.into()) } }

/// The card's rows from the open panel's history.
pub(super) fn from_library(lib: &mut Library, folder: &str) -> (Vec<Headline>, usize) {
    let scope = scope(folder);
    let items = lib.list(&scope, Filter::Unread, "");
    let rows = items.iter().take(CARD_ITEMS).filter_map(|item| lib.entry(*item)).map(|(row, entry)| Headline {
        url: row.sub.url.clone(), link: entry.link.clone(), title: entry.title.clone(), feed: row.name().to_owned(), color: row.sub.color.clone(), when: entry.when(),
    }).collect();
    (rows, lib.unread_in(&scope))
}

/// The card's rows from the saved history (blocking; run off the UI
/// thread). Reads one feed at a time and keeps only the newest rows.
pub(super) fn from_disk(cache: &Cache, subs: &[Subscription], folder: &str) -> (Vec<Headline>, usize) {
    let mut rows: Vec<Headline> = Vec::with_capacity(CARD_ITEMS + 1);
    let mut unread = 0;
    for sub in subs.iter().filter(|sub| folder.is_empty() || sub.folder == folder) {
        let Ok(Some(snapshot)) = cache.load(&sub.url) else { continue };
        let feed = if !sub.title.is_empty() { sub.title.clone() } else if !snapshot.feed.title.is_empty() { snapshot.feed.title.clone() } else { host(&sub.url).to_owned() };
        for entry in snapshot.feed.entries.iter().filter(|entry| !snapshot.read.contains(&entry.link)) {
            unread += 1;
            if rows.len() == CARD_ITEMS && rows.last().is_some_and(|last| last.when >= entry.when()) { continue; }
            rows.push(Headline { url: sub.url.clone(), link: entry.link.clone(), title: entry.title.clone(), feed: feed.clone(), color: sub.color.clone(), when: entry.when() });
            rows.sort_by_key(|row| std::cmp::Reverse(row.when));
            rows.truncate(CARD_ITEMS);
        }
    }
    (rows, unread)
}

/// Refresh the card's rows when the data, the subscriptions or the folder
/// changed: straight from the panel's history when it is loaded, else from
/// disk off the UI thread.
fn sync(settings: &super::Settings, cx: &mut App) {
    let hub = hub();
    let folder = &settings.card_folder;
    let load = {
        let mut h = hub.borrow_mut();
        if h.card.key.as_ref().is_some_and(|(rev, subs, f)| *rev == h.rev && *subs == settings.subs_key && f == folder) { return; }
        let key = (h.rev, settings.subs_key, folder.clone());
        if let Some(lib) = h.lib.as_mut() {
            // The history follows the subscriptions first (as when the panel opens).
            let changed = lib.sync(&settings.subs);
            let (rows, unread) = from_library(lib, folder);
            if changed { h.due_checked = None; }
            h.card.items = rows.into();
            h.card.unread = unread;
            h.card.key = Some(key);
            return;
        }
        if h.card.loading { return; }
        h.card.loading = true;
        (key, h.card.generation)
    };
    let ((rev, subs_key, folder), generation) = load;
    let subs = settings.subs.to_vec();
    let wanted = folder.to_string();
    cx.spawn(async move |cx| {
        let (rows, unread) = mandara_services::runtime::unblock(move || from_disk(&Cache::default(), &subs, &wanted)).await;
        let views = {
            let mut h = hub.borrow_mut();
            if h.card.generation != generation { return; }
            h.card.loading = false;
            h.card.items = rows.into();
            h.card.unread = unread;
            h.card.key = Some((rev, subs_key, folder));
            h.views.clone()
        };
        views.notify_async(cx);
    }).detach();
}

/// Open an article from the card: the panel opens and shows it in the reader.
fn open(url: String, link: String, cx: &mut App) {
    hub().borrow_mut().pending_open = Some((url, link));
    crate::state::shell(cx).update(cx, |shell, cx| shell.set_panel(Some(ID.into()), cx));
    super::opened(cx);
}

fn tint(row: &Headline, palette: &MandaraPalette) -> Color {
    Color::from_hex(&row.color).map(|c| readable(c, palette)).unwrap_or_else(|_| name_color(&row.feed, palette))
}

/// The card.
pub(super) fn render(palette: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let hub = hub();
    hub.borrow().views.track(window);
    let settings = cached_settings(cx);
    let folder = settings.card_folder.clone();
    if !settings.subs.is_empty() { sync(&settings, cx); }
    let (items, unread) = { let h = hub.borrow(); (h.card.items.clone(), h.card.unread) };
    let accent = readable(palette.primary, palette);
    let now = chrono::Utc::now().timestamp();
    let title: SharedString = if folder.is_empty() { t("Feeds").into() } else { folder.to_string().into() };
    let head = div().id("feeds-card-head").row().items_center().gap(px(tokens::SM)).cursor_pointer()
        .child(icon("rss", HEAD_ICON, accent))
        .child(div().flex_1().min_w(px(0.0)).truncate().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(title))
        .when(unread > 0, |el| el.child(div().h(px(COUNT_H)).px(px(COUNT_PAD_X)).rounded_full().bg(accent.with_alpha(super::super::panel_fx::alpha(super::super::panel_fx::TINT_FILL, palette)).hsla())
            .flex().items_center().text_size(px(text::CAPTION)).font_weight(FontWeight::SEMIBOLD).font_features(crate::fonts::tabular_nums()).text_color(accent.hsla())
            .child(tf("{count} unread", &[("count", &unread)]))))
        .on_click(|_, _, cx| crate::state::shell(cx).update(cx, |shell, cx| shell.set_panel(Some(ID.into()), cx)));
    let mut body = div().flex().flex_col().gap(px(tokens::XS)).child(head);
    if settings.subs.is_empty() {
        body = body.child(div().pt(px(tokens::SM)).row().child(pill_button("feeds-card-add", Some("plus"), t("Add feeds"), accent, palette, |_, cx| crate::state::open_studio_at(Some("plugins/feeds/settings".into()), cx))));
    } else if items.is_empty() {
        body = body.child(div().py(px(tokens::SM)).row().items_center().gap(px(tokens::SM)).text_size(px(text::BODY_S)).text_color(palette.on_surface_variant.hsla())
            .child(icon("check", HEAD_ICON, palette.on_surface_variant)).child(t("All caught up")));
    }
    let hover = wash(WASH_SOFT, palette);
    for (index, row) in items.iter().enumerate() {
        let color = tint(row, palette);
        let initial: String = row.feed.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_uppercase().collect()).unwrap_or_default();
        let (url, link) = (row.url.clone(), row.link.clone());
        body = body.child(div().id(("feeds-card-row", index)).px(px(ROW_PAD_X)).py(px(ROW_PAD_Y)).rounded(px(crate::style::radius(ROW_RADIUS))).cursor_pointer()
            .hover(move |s| s.bg(hover.hsla())).row().items_start().gap(px(tokens::SM))
            .child(monogram(&initial, color, MONOGRAM))
            .child(div().flex_1().min_w(px(0.0)).flex().flex_col()
                .child(div().row().items_center().gap(px(META_GAP)).text_size(px(text::CAPTION)).text_color(palette.on_surface_variant.hsla())
                    .child(div().size(px(DOT)).flex_none().rounded_full().bg(color.hsla()))
                    .child(div().min_w(px(0.0)).truncate().font_weight(FontWeight::MEDIUM).text_color(color.hsla()).child(row.feed.clone()))
                    .child("·")
                    .child(div().flex_none().font_features(crate::fonts::tabular_nums()).child(ago(row.when, now))))
                .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).line_clamp(2).child(crate::bidi::paragraph(row.title.clone()))))
            .on_click(move |_, _, cx| open(url.clone(), link.clone(), cx)));
    }
    card(palette).child(body).into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_services::feeds::{Entry, Feed, Limits, Snapshot, Source};

    struct Fixed(Vec<(&'static str, i64)>);
    impl Source for Fixed {
        fn fetch(&self, url: &str, _: Option<&Feed>) -> Result<Option<Feed>, mandara_services::feeds::Failure> {
            Ok(Some(Feed { url: url.into(), title: "Paper".into(), entries: self.0.iter().map(|(t, w)| Entry { id: (*t).into(), title: (*t).into(), link: format!("{url}/{t}"), published: Some(*w), summary: String::new(), image: None, author: None, seen: *w }).collect(), ..Feed::default() }))
        }
    }

    #[test]
    fn disk_rows_are_the_newest_unread_across_feeds_or_one_folder() {
        let dir = tempfile::tempdir().unwrap();
        let cache = Cache::at(dir.path().into());
        let now = chrono::Utc::now().timestamp();
        let a = "https://a.example/feed";
        let b = "https://b.example/feed";
        cache.refresh(&Fixed((0..8).map(|n| (["a0", "a1", "a2", "a3", "a4", "a5", "a6", "a7"][n], now - n as i64 * 100)).collect()), a, None, Limits::default(), true).unwrap();
        cache.refresh(&Fixed(vec![("b0", now - 50), ("b1", now - 10_000)]), b, None, Limits::default(), true).unwrap();
        cache.mark_read(a, "https://a.example/feed/a0").unwrap();
        let subs = vec![Subscription { url: a.into(), folder: "News".into(), ..Subscription::default() }, Subscription { url: b.into(), title: "Bee".into(), ..Subscription::default() }];
        let (rows, unread) = from_disk(&cache, &subs, "");
        assert_eq!(unread, 9);
        assert_eq!(rows.iter().map(|r| r.title.as_str()).collect::<Vec<_>>(), ["b0", "a1", "a2", "a3", "a4"], "newest unread first, read ones skipped");
        assert_eq!(rows[0].feed, "Bee");
        assert_eq!(rows[1].feed, "Paper");
        let (rows, unread) = from_disk(&cache, &subs, "News");
        assert_eq!(unread, 7);
        assert!(rows.iter().all(|r| r.url == a));
        assert!(from_disk(&cache, &subs, "Nothing").0.is_empty());
    }

    #[test]
    fn library_rows_follow_the_panel_and_releasing_empties_the_card() {
        let mut lib = Library::default();
        let sub = Subscription { url: "https://c.example".into(), folder: "Tech".into(), ..Subscription::default() };
        lib.sync(std::slice::from_ref(&sub));
        lib.set(std::sync::Arc::new(Snapshot { feed: Feed { url: sub.url.clone(), title: "Cee".into(), entries: (0..7).map(|n| Entry { id: n.to_string(), title: format!("c{n}"), link: format!("https://c.example/{n}"), published: Some(1_800_000_000 - n), summary: String::new(), image: None, author: None, seen: 0 }).collect(), ..Feed::default() }, ..Snapshot::default() }));
        let (rows, unread) = from_library(&mut lib, "Tech");
        assert_eq!((rows.len(), unread), (CARD_ITEMS, 7));
        assert!(from_library(&mut lib, "Other").0.is_empty());
        let mut data = CardData { items: rows.into(), unread, ..CardData::default() };
        let generation = data.generation;
        data.release();
        assert!(data.items.is_empty() && data.generation != generation);
    }
}
