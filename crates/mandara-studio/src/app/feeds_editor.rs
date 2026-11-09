//! Plugins → Feeds → settings: the subscription manager above the
//! plugin's settings. Add a site or feed address (its feed is found for
//! you), pick from tested starter feeds by topic, rename, file into
//! folders, colour, reorder and remove feeds, and import or export OPML.
//! Discovery and file work run off the UI thread; a reply belongs to the
//! editor generation that asked for it.

use std::rc::Rc;

use gpui::{AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_i18n::{t, tf, tr};
use mandara_services::feeds::{Candidate, Subscription, catalog};
use mandara_theme::{Color, MandaraPalette};
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_ui::plugins::feeds;
use mandara_ui::state::update_config;

use super::compositor_page::{badge, button, card, chip, heading};
use super::{StudioView, form};
use crate::tokens::*;

const ADD_INPUT: &str = "feeds-add";
const OPML_INPUT: &str = "feeds-opml";
/// Where an export goes when no file is typed.
const DEFAULT_OPML: &str = "~/feeds.opml";
/// Feed colours offered: OKLCH hues at one lightness and chroma.
const SWATCH_HUES: [f32; 8] = [25.0, 60.0, 100.0, 150.0, 190.0, 235.0, 280.0, 330.0];
const SWATCH_L: f32 = 0.68;
const SWATCH_C: f32 = 0.15;
/// The colour dot beside each feed.
const DOT_PX: f32 = DIM_12_PX;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Busy { Discovering, Importing, Exporting }

#[derive(Default)]
pub(super) struct Editor {
    generation: u64,
    busy: Option<Busy>,
    /// Feeds a site offers, waiting for you to pick one.
    found: Vec<Candidate>,
    note: Option<(bool, String)>,
    topic: usize,
    /// The subscription whose title, folder and colour are open.
    editing: Option<String>,
}

impl Editor {
    /// Discard replies once settings close.
    pub(super) fn close(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.busy = None;
        self.found.clear();
        self.note = None;
        self.editing = None;
    }

    fn begin(&mut self, busy: Busy) -> Option<u64> {
        if self.busy.is_some() { return None; }
        self.generation = self.generation.wrapping_add(1);
        self.busy = Some(busy);
        self.note = None;
        Some(self.generation)
    }

    fn finish(&mut self, generation: u64) -> bool {
        if generation != self.generation { return false; }
        self.busy = None;
        true
    }
}

/// Add subscriptions not already in the list; returns how many were new.
fn add_all(cfg: &mut ShellConfig, new: impl IntoIterator<Item = Subscription>) -> usize {
    let mut list = feeds::subscriptions(cfg);
    let before = list.len();
    for sub in new {
        if list.len() < feeds::MAX_SUBSCRIPTIONS && !list.iter().any(|seen| seen.url == sub.url) { list.push(sub); }
    }
    let added = list.len() - before;
    feeds::set_subscriptions(cfg, &list);
    added
}

fn edit(cx: &mut gpui::App, change: impl FnOnce(&mut Vec<Subscription>)) {
    update_config(cx, move |c| {
        let mut list = feeds::subscriptions(c);
        change(&mut list);
        feeds::set_subscriptions(c, &list);
    });
}

fn swatch(hue: f32) -> Color { Color::oklch(SWATCH_L, SWATCH_C, hue) }

/// A subscription's colour as shown here (yours, or the automatic one).
fn color_of(sub: &Subscription, p: &MandaraPalette) -> Color {
    Color::from_hex(&sub.color).unwrap_or_else(|_| mandara_ui::plugins::extras::name_color(if sub.title.is_empty() { feeds::host(&sub.url) } else { &sub.title }, p))
}

impl StudioView {
    /// Find the feed at the typed address and add it (or offer the choices).
    fn feeds_add(&mut self, cx: &mut Context<Self>) {
        let typed = form::value(ADD_INPUT, cx);
        if typed.is_empty() { return; }
        let Some(generation) = self.feeds.begin(Busy::Discovering) else { return };
        self.feeds.found.clear();
        form::job(cx, move || mandara_services::feeds::discover(&typed), move |this, result, cx| {
            if !this.feeds.finish(generation) { return; }
            match result {
                Ok(found) if found.len() == 1 => {
                    let first = found.into_iter().next().expect("one candidate");
                    form::clear(ADD_INPUT, cx);
                    this.feeds_subscribe(first, cx);
                }
                Ok(found) => this.feeds.found = found,
                Err(error) => this.feeds.note = Some((true, error)),
            }
        });
        cx.notify();
    }

    fn feeds_subscribe(&mut self, found: Candidate, cx: &mut Context<Self>) {
        let title = found.title.clone();
        let mut added = 0;
        update_config(cx, |c| added = add_all(c, [Subscription { url: found.url, title: found.title, ..Subscription::default() }]));
        self.feeds.found.clear();
        self.feeds.note = Some(if added > 0 { (false, tf("Subscribed to {feed}", &[("feed", &title)])) } else { (true, t("You already follow this feed").into()) });
        cx.notify();
    }

    fn feeds_import(&mut self, cx: &mut Context<Self>) {
        let path = feeds::expand_home(&form::value(OPML_INPUT, cx));
        let Some(generation) = self.feeds.begin(Busy::Importing) else { return };
        form::job(cx, move || feeds::read_opml(&path).and_then(|text| mandara_services::feeds::opml::import(&text)), move |this, result, cx| {
            if !this.feeds.finish(generation) { return; }
            this.feeds.note = Some(match result {
                Ok(found) => {
                    let total = found.len();
                    let mut added = 0;
                    update_config(cx, |c| added = add_all(c, found));
                    (false, tf("Imported {added} of {total} feeds", &[("added", &added), ("total", &total)]))
                }
                Err(error) => (true, error),
            });
        });
        cx.notify();
    }

    fn feeds_export(&mut self, cfg: &ShellConfig, cx: &mut Context<Self>) {
        let typed = form::value(OPML_INPUT, cx);
        let path = feeds::expand_home(if typed.is_empty() { DEFAULT_OPML } else { &typed });
        let list = feeds::subscriptions(cfg);
        let Some(generation) = self.feeds.begin(Busy::Exporting) else { return };
        form::job(cx, move || {
            std::fs::write(&path, mandara_services::feeds::opml::export(&list)).map_err(|error| error.to_string())?;
            Ok((path, list.len()))
        }, move |this, result, _| {
            if !this.feeds.finish(generation) { return; }
            this.feeds.note = Some(match result {
                Ok((path, count)) => (false, tf("Saved {count} feeds to {path}", &[("count", &count), ("path", &path.display())])),
                Err(error) => (true, error),
            });
        });
        cx.notify();
    }

    /// The subscription manager, starter feeds and OPML.
    pub(super) fn feeds_editor(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let list = feeds::subscriptions(cfg);
        let busy = self.feeds.busy.clone();
        let add_field = form::input(ADD_INPUT, t("A website or feed address"), false, cx);
        let adding = busy == Some(Busy::Discovering);
        let mut subs = card(p)
            .child(heading(t("Your feeds"), t("Add a site and its feed is found for you. Order, folders and colours here are how the panel shows them."), p))
            .child(div().row().items_center().gap(px(SPACE_8_PX))
                .child(div().flex_1().min_w(px(0.0)).child(add_field))
                .child(button("feeds-add", if adding { "refresh" } else { "plus" }, if adding { t("Looking…") } else { t("Add") }, true, p)
                    .on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.feeds_add(cx)))));
        if !self.feeds.found.is_empty() {
            let mut choices = div().row().flex_wrap().gap(px(SPACE_8_PX)).child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("This site offers several feeds:")));
            for (index, found) in self.feeds.found.iter().enumerate() {
                let label = if found.title.is_empty() { feeds::host(&found.url).to_owned() } else { found.title.clone() };
                let candidate = found.clone();
                choices = choices.child(chip(SharedString::from(format!("feeds-found-{index}")), label, false, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.feeds_subscribe(candidate.clone(), cx))));
            }
            subs = subs.child(choices);
        }
        if let Some((error, note)) = &self.feeds.note { subs = subs.child(form::note(note, *error, p)); }
        if list.is_empty() {
            subs = subs.child(div().py(px(SPACE_10_PX)).text_size(px(TEXT_12_5_PX)).text_color(p.on_surface_variant.hsla()).child(t("No feeds yet. Add one above or pick from the starter feeds below.")));
        }
        let last = list.len().saturating_sub(1);
        for (index, sub) in list.iter().enumerate() {
            subs = subs.child(self.feed_row(index, last, sub, p, cx));
        }
        subs = subs.child(div().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).child(tf("{count} of {max} feeds", &[("count", &list.len()), ("max", &feeds::MAX_SUBSCRIPTIONS)])));
        div().row().flex_col().gap(px(SPACE_16_PX)).child(subs).child(self.starters(&list, p, cx)).child(self.opml(cfg, p, cx)).into_any_element()
    }

    fn feed_row(&mut self, index: usize, last: usize, sub: &Subscription, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let url = sub.url.clone();
        let open = self.feeds.editing.as_deref() == Some(url.as_str());
        let name = if sub.title.is_empty() { feeds::host(&sub.url).to_owned() } else { sub.title.clone() };
        let small = |id: String, glyph: &'static str, enabled: bool, p: &MandaraPalette| div().id(SharedString::from(id)).size(px(DIM_30_PX)).flex_none().rounded_full().row().items_center().justify_center()
            .when(enabled, |el| el.cursor_pointer().hover(|s| s.bg(p.on_surface.with_alpha(ALPHA_8).hsla())))
            .child(icon(glyph, ICON_14_PX, if enabled { p.on_surface_variant } else { p.on_surface_variant.with_alpha(ALPHA_30) }));
        let (up, down, edit_url, remove) = (url.clone(), url.clone(), url.clone(), url.clone());
        let mut row = div().row().flex_col().gap(px(SPACE_10_PX)).p(px(SPACE_10_PX)).rounded(px(RADIUS_14_PX)).bg(p.surface_container_high.with_alpha(ALPHA_50).hsla())
            .child(div().row().items_center().gap(px(SPACE_10_PX))
                .child(div().size(px(DOT_PX)).flex_none().rounded_full().bg(color_of(sub, p).hsla()))
                .child(div().flex_1().min_w(px(0.0)).row().flex_col()
                    .child(div().text_size(px(TEXT_13_PX)).font_weight(FontWeight::MEDIUM).truncate().child(name))
                    .child(div().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).truncate().child(sub.url.clone())))
                .when(!sub.folder.is_empty(), |el| el.child(badge(sub.folder.clone(), p.tertiary)))
                .child(small(format!("feeds-up-{index}"), "chevron-up", index > 0, p).when(index > 0, |el| el.on_click(cx.listener(move |_, _: &ClickEvent, _, cx| edit(cx, |list| if let Some(at) = list.iter().position(|s| s.url == up) && at > 0 { list.swap(at, at - 1); })))))
                .child(small(format!("feeds-down-{index}"), "chevron-down", index < last, p).when(index < last, |el| el.on_click(cx.listener(move |_, _: &ClickEvent, _, cx| edit(cx, |list| if let Some(at) = list.iter().position(|s| s.url == down) && at + 1 < list.len() { list.swap(at, at + 1); })))))
                .child(small(format!("feeds-edit-{index}"), "pencil", true, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    this.feeds.editing = if this.feeds.editing.as_deref() == Some(edit_url.as_str()) { None } else { Some(edit_url.clone()) };
                    cx.notify();
                })))
                .child(small(format!("feeds-remove-{index}"), "trash", true, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| {
                    let gone = remove.clone();
                    this.list_fields.retain(|key, _| !key.starts_with(&format!("feed|{gone}|")));
                    edit(cx, move |list| list.retain(|s| s.url != gone));
                }))));
        if open {
            let (title_url, folder_url) = (url.clone(), url.clone());
            let title = self.list_field(format!("feed|{url}|title"), &sub.title, t("Name (empty uses the feed's own)"), Rc::new(move |c, v| { let mut list = feeds::subscriptions(c); if let Some(s) = list.iter_mut().find(|s| s.url == title_url) { s.title = v.trim().to_owned(); } feeds::set_subscriptions(c, &list); }), cx);
            let folder = self.list_field(format!("feed|{url}|folder"), &sub.folder, t("Folder (empty for none)"), Rc::new(move |c, v| { let mut list = feeds::subscriptions(c); if let Some(s) = list.iter_mut().find(|s| s.url == folder_url) { s.folder = v.trim().to_owned(); } feeds::set_subscriptions(c, &list); }), cx);
            let mut swatches = div().row().flex_wrap().items_center().gap(px(SPACE_8_PX)).child(div().text_size(px(TEXT_12_PX)).text_color(p.on_surface_variant.hsla()).child(t("Colour")));
            let auto_url = url.clone();
            swatches = swatches.child(chip(SharedString::from(format!("feeds-color-auto-{index}")), t("Automatic"), sub.color.is_empty(), p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| { let u = auto_url.clone(); edit(cx, move |list| if let Some(s) = list.iter_mut().find(|s| s.url == u) { s.color.clear(); }); })));
            for (n, hue) in SWATCH_HUES.into_iter().enumerate() {
                let hex = swatch(hue).to_hex();
                let chosen = sub.color.eq_ignore_ascii_case(&hex);
                let target = url.clone();
                swatches = swatches.child(div().id(SharedString::from(format!("feeds-color-{index}-{n}"))).size(px(DIM_24_PX)).flex_none().rounded_full().cursor_pointer().bg(swatch(hue).hsla())
                    .when(chosen, |el| el.border_2().border_color(p.on_surface.hsla()))
                    .on_click(cx.listener(move |_, _: &ClickEvent, _, cx| { let (u, hex) = (target.clone(), hex.clone()); edit(cx, move |list| if let Some(s) = list.iter_mut().find(|s| s.url == u) { s.color = hex; }); })));
            }
            row = row.child(div().row().when(self.stack_rows, |el| el.flex_col()).gap(px(SPACE_8_PX)).child(div().flex_1().min_w(px(0.0)).child(title)).child(div().flex_1().min_w(px(0.0)).child(folder))).child(swatches);
        }
        row.into_any_element()
    }

    fn starters(&mut self, list: &[Subscription], p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let topic = self.feeds.topic.min(catalog::TOPICS.len() - 1);
        let mut tabs = div().row().flex_wrap().gap(px(SPACE_8_PX));
        for (n, (_, name)) in catalog::TOPICS.iter().enumerate() {
            tabs = tabs.child(chip(SharedString::from(format!("feeds-topic-{n}")), tr(name), n == topic, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| { this.feeds.topic = n; cx.notify(); })));
        }
        let key = catalog::TOPICS[topic].0;
        let mut rows = div().row().flex_col().gap(px(SPACE_6_PX));
        for (n, starter) in catalog::STARTERS.iter().filter(|s| s.topic == key).enumerate() {
            let added = list.iter().any(|sub| sub.url == starter.url);
            let starter = *starter;
            rows = rows.child(div().row().items_center().gap(px(SPACE_10_PX)).px(px(SPACE_10_PX)).py(px(SPACE_6_PX)).rounded(px(RADIUS_12_PX)).bg(p.surface_container_high.with_alpha(ALPHA_40).hsla())
                .child(icon("rss", ICON_14_PX, p.primary))
                .child(div().flex_1().min_w(px(0.0)).row().flex_col()
                    .child(div().text_size(px(TEXT_13_PX)).font_weight(FontWeight::MEDIUM).child(starter.title))
                    .child(div().text_size(px(TEXT_11_PX)).text_color(p.on_surface_variant.hsla()).truncate().child(feeds::host(starter.url).to_owned())))
                .child(if added {
                    badge(t("Added"), p.success_emerald).into_any_element()
                } else {
                    button(SharedString::from(format!("feeds-starter-{n}")), "plus", t("Add"), false, p).on_click(cx.listener(move |_, _: &ClickEvent, _, cx| {
                        let folder = tr(catalog::topic_name(starter.topic));
                        update_config(cx, |c| { add_all(c, [Subscription { url: starter.url.into(), title: starter.title.into(), folder, color: String::new() }]); });
                    })).into_any_element()
                }));
        }
        card(p).child(heading(t("Starter feeds"), tf("Popular feeds by topic, each checked to download and read correctly on {date}.", &[("date", &catalog::VERIFIED)]), p))
            .child(tabs).child(rows).into_any_element()
    }

    fn opml(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let field = form::input(OPML_INPUT, DEFAULT_OPML, false, cx);
        let busy = self.feeds.busy.clone();
        let cfg = cfg.clone();
        card(p).child(heading(t("Import and export"), t("Move your subscriptions between feed readers as an OPML file. Importing adds feeds you don't follow yet, with their folders."), p))
            .child(div().row().items_center().gap(px(SPACE_8_PX)).when(self.stack_rows, |el| el.flex_col())
                .child(div().flex_1().min_w(px(0.0)).child(field))
                .child(button("feeds-import", "arrow-down", if busy == Some(Busy::Importing) { t("Importing…") } else { t("Import") }, false, p).on_click(cx.listener(|this, _: &ClickEvent, _, cx| this.feeds_import(cx))))
                .child(button("feeds-export", "arrow-up", if busy == Some(Busy::Exporting) { t("Saving…") } else { t("Export") }, false, p).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.feeds_export(&cfg, cx)))))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adding_skips_duplicates_and_respects_the_limit() {
        let mut cfg = ShellConfig::default();
        let sub = |n: usize| Subscription { url: format!("https://{n}.example/feed"), ..Subscription::default() };
        assert_eq!(add_all(&mut cfg, [sub(1), sub(2), sub(1)]), 2);
        assert_eq!(add_all(&mut cfg, [sub(2)]), 0);
        assert_eq!(add_all(&mut cfg, (0..feeds::MAX_SUBSCRIPTIONS * 2).map(sub)), feeds::MAX_SUBSCRIPTIONS - 2);
        assert_eq!(feeds::subscriptions(&cfg).len(), feeds::MAX_SUBSCRIPTIONS);
    }

    #[test]
    fn replies_from_a_closed_editor_are_ignored() {
        let mut editor = Editor::default();
        let old = editor.begin(Busy::Discovering).unwrap();
        assert!(editor.begin(Busy::Importing).is_none(), "one job at a time");
        editor.close();
        assert!(!editor.finish(old));
        let new = editor.begin(Busy::Importing).unwrap();
        assert!(editor.finish(new));
        assert!(editor.busy.is_none());
    }

    #[test]
    fn swatches_are_distinct_colours() {
        let hexes: std::collections::HashSet<String> = SWATCH_HUES.into_iter().map(|hue| swatch(hue).to_hex()).collect();
        assert_eq!(hexes.len(), SWATCH_HUES.len());
        assert!(hexes.iter().all(|hex| Color::from_hex(hex).is_ok()));
    }
}
