//! A post and its comments: the post first (shown at once from the list),
//! then comments as they arrive, folded and expanded in place. Only rows on
//! screen are built.

use std::rc::Rc;

use gpui::{AnyElement, App, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, list, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui;
use mandara_i18n::{t, tf};
use mandara_services::reddit::{Kind, Post};
use mandara_theme::{Color, MandaraPalette};

use super::Settings;
use super::images::MAX_ASPECT;
use super::model::{self, HEAD_ITEMS, Model, Reader, Row, Shared};
use super::panel::{MEDIA_W, age, badge, changed, count, covered, open_link, picture, problem, stat};
use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::kit::{alpha, icon_button, text, tokens};
use crate::markdown::RenderCx;
use crate::plugins::panel_fx::{glass_card, header, pill_button, readable};

const BUTTON_PX: f32 = 30.0;
/// Each reply level moves this far in, up to the deepest shown indent.
const INDENT_PX: f32 = 12.0;
const MAX_INDENT_LEVELS: u8 = 8;
const THREAD_LINE_PX: f32 = 2.0;
const MEDIA_RADIUS_PX: f32 = 14.0;
/// Pictures without a known size are drawn this tall, relative to width.
const DEFAULT_ASPECT: f32 = 0.6;
const GALLERY_BUTTON_PX: f32 = 28.0;

fn with_model<R>(cx: &mut App, f: impl FnOnce(&mut Model) -> R) -> R {
    let shared = model::model();
    let result = f(&mut shared.borrow_mut());
    changed(&shared, cx);
    result
}

/// The colour of a reply level's thread line.
fn level_color(depth: u8, p: &MandaraPalette) -> Color {
    match depth % 3 { 0 => readable(p.primary, p), 1 => readable(p.secondary, p), _ => readable(p.tertiary, p) }
}

pub(super) fn render(shared: &Shared, settings: &Rc<Settings>, p: &MandaraPalette, scale: f32, _: &mut Window, cx: &mut App) -> AnyElement {
    let mono = crate::fonts::fonts(cx).mono;
    let (post, link) = {
        let m = shared.borrow();
        let Some(reader) = &m.reader else { return div().into_any_element() };
        (reader.post.clone(), reader.post.as_ref().map(|post| mandara_services::reddit::web_link(&post.permalink)))
    };
    let accent = readable(p.primary, p);
    let back = div().id("reddit-back").focusable().tab_index(0).rounded_full()
        .child(icon_button("reddit-back-button", crate::bidi::dir_icon("chevron-left"), BUTTON_PX, p.on_surface_variant, None, |_, cx| with_model(cx, Model::back)));
    let title: SharedString = post.as_ref().map_or_else(|| t("Reddit").into(), |post| format!("r/{}", post.subreddit).into());
    let subtitle: SharedString = post.as_ref().map_or_else(SharedString::default, |post| format!("u/{}", post.author).into());
    let mut actions = div().row().items_center().gap(px(tokens::XS));
    if let Some(link) = link {
        let copy = link.clone();
        actions = actions
            .child(icon_button("reddit-open-web", "external", BUTTON_PX, p.on_surface_variant, None, move |_, _| open_link(link.clone())))
            .child(icon_button("reddit-copy-link", "copy", BUTTON_PX, p.on_surface_variant, None, move |_, cx| {
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(copy.clone()));
                with_model(cx, |m| m.flash.show(true, t("Link copied")));
            }));
    }
    actions = actions.child(icon_button("reddit-reload-post", "refresh", BUTTON_PX, p.on_surface_variant, None, |_, cx| {
        with_model(cx, |m| if let Some(want) = m.want { m.reload_post(want) });
    }));
    let top = div().row().items_center().gap(px(tokens::SM)).flex_none().child(back)
        .child(div().flex_1().min_w_0().child(header("messages", accent, title, subtitle, p, Some(actions.into_any_element()))));
    let state = shared.borrow().reader.as_ref().map(|reader| reader.list.clone());
    let Some(state) = state else { return top.into_any_element() };
    let rows = {
        let shared = shared.clone();
        let settings = settings.clone();
        let palette = p.clone();
        let now = chrono::Utc::now().timestamp();
        let lang = mandara_i18n::language();
        list(state, move |index, _, cx| {
            let row = mandara_i18n::with_language(Some(lang), || {
                let mut m = shared.borrow_mut();
                match index {
                    0 => post_block(&mut m, &settings, &palette, scale, now, &mono),
                    1 => comments_header(&m, &palette),
                    _ => comment_row(&m, index - HEAD_ITEMS, &palette, now, &mono),
                }
            });
            if index == 0 { model::pump_soon(&shared, cx); }
            row
        }).w_full().flex_1()
    };
    div().flex_1().min_h_0().flex().flex_col().gap(px(tokens::MD)).child(top).child(rows).into_any_element()
}

/// Title, badges, picture or gallery, link or video, and the self text.
fn post_block(m: &mut Model, settings: &Settings, p: &MandaraPalette, scale: f32, now: i64, mono: &SharedString) -> AnyElement {
    let Some(reader) = &m.reader else { return div().into_any_element() };
    let Some(post) = reader.post.clone() else {
        return div().pb(px(tokens::MD)).child(match &reader.error {
            Some(error) => problem(error, p, |_, cx| with_model(cx, |m| if let Some(want) = m.want { m.reload_post(want) })),
            None => div().text_size(px(text::BODY_S)).text_color(p.on_surface_variant.hsla()).child(t("Opening the post…")).into_any_element(),
        }).into_any_element();
    };
    let revealed = reader.revealed;
    let gallery = reader.gallery.min(post.media.len().saturating_sub(1));
    let is_covered = covered(&post, settings, revealed);
    let mut column = div().w_full().pb(px(tokens::MD)).flex().flex_col().gap(px(tokens::SM))
        .child(div().text_size(px(text::TITLE)).font_weight(FontWeight::SEMIBOLD).child(crate::bidi::paragraph(post.title.to_string())));
    let mut badges = div().row().items_center().gap(px(tokens::SM)).flex_wrap()
        .when(!post.limited, |el| el.child(stat("arrow-up", count(post.score), p)).child(stat("message", count(post.comments as i64), p)))
        .child(stat("clock", age(post.created, now), p));
    if let Some(flair) = &post.flair { badges = badges.child(badge(None, flair.to_string(), p.tertiary, p)); }
    if post.nsfw { badges = badges.child(badge(None, t("NSFW"), p.urgent_coral, p)); }
    if post.spoiler { badges = badges.child(badge(None, t("Spoiler"), p.warning_amber, p)); }
    if post.locked { badges = badges.child(badge(Some("lock"), t("Locked"), p.on_surface_variant, p)); }
    column = column.child(badges);
    if settings.images && let Some(image) = post.media.get(gallery) {
        let aspect = if image.width > 0 { (image.height as f32 / image.width as f32).min(MAX_ASPECT) } else { DEFAULT_ASPECT };
        let height = (MEDIA_W * aspect).round();
        let shown = picture(m, &image.url, MEDIA_W, height, is_covered, scale, MEDIA_RADIUS_PX, p);
        let mut frame = div().relative().w(px(MEDIA_W)).h(px(height)).flex_none().child(shown);
        if is_covered {
            frame = frame.child(div().absolute().inset_0().flex().items_center().justify_center()
                .child(pill_button("reddit-reveal-media", Some("eye"), t("Show"), readable(p.primary, p), p, |_, cx| with_model(cx, |m| if let Some(r) = &mut m.reader { r.revealed = true })).focusable().tab_index(0)));
        }
        if post.media.len() > 1 {
            let last = post.media.len() - 1;
            frame = frame.child(div().absolute().bottom(px(tokens::SM)).left_0().right_0().flex().justify_center()
                .child(div().row().items_center().gap(px(tokens::SM)).px(px(tokens::SM)).rounded_full().bg(p.surface_container.with_alpha(alpha::SECONDARY_TEXT).hsla())
                    .child(icon_button("reddit-gallery-prev", crate::bidi::dir_icon("chevron-left"), GALLERY_BUTTON_PX, p.on_surface, None, move |_, cx| with_model(cx, |m| if let Some(r) = &mut m.reader { r.gallery = r.gallery.saturating_sub(1) })))
                    .child(div().font_features(crate::fonts::tabular_nums()).text_size(px(text::LABEL)).child(format!("{} / {}", gallery + 1, last + 1)))
                    .child(icon_button("reddit-gallery-next", crate::bidi::dir_icon("chevron-right"), GALLERY_BUTTON_PX, p.on_surface, None, move |_, cx| with_model(cx, |m| if let Some(r) = &mut m.reader { r.gallery = (r.gallery + 1).min(last) })))));
        }
        column = column.child(frame);
    }
    match post.kind {
        Kind::Video => column = column.child(outside(&post, "play", t("Videos play in your browser"), t("Open video"), p)),
        Kind::Link => column = column.child(outside(&post, "external", post.domain.to_string(), t("Open link"), p)),
        _ => {}
    }
    // Borrowed again here: the picture above needed the model mutably.
    if let Some(body) = m.reader.as_ref().and_then(|reader| reader.body.as_ref()).filter(|body| !body.is_empty()) {
        if is_covered {
            column = column.child(glass_card(p).p(px(tokens::MD)).row().items_center().gap(px(tokens::SM))
                .child(icon("eye-off", crate::kit::metrics::TILE_ICON, p.on_surface_variant))
                .child(div().flex_1().text_size(px(text::BODY_S)).text_color(p.on_surface_variant.hsla()).child(t("This post's text is covered")))
                .child(pill_button("reddit-reveal-text", Some("eye"), t("Show"), readable(p.primary, p), p, |_, cx| with_model(cx, |m| if let Some(r) = &mut m.reader { r.revealed = true })).focusable().tab_index(0)));
        } else {
            let rcx = RenderCx { id: SharedString::from(format!("reddit-post-{}", post.id)), on_toggle: None, mono: mono.clone() };
            column = column.child(crate::markdown::render(body.shown(revealed || settings.spoilers == super::Gate::Show), p, &rcx));
            if body.has_spoilers() && !revealed && settings.spoilers != super::Gate::Show {
                column = column.child(pill_button("reddit-reveal-spoilers", Some("eye"), t("Reveal spoilers"), readable(p.primary, p), p, |_, cx| with_model(cx, |m| if let Some(r) = &mut m.reader { r.revealed = true })).focusable().tab_index(0));
            }
        }
    }
    column.into_any_element()
}

/// A link or video post: where it goes and a button to go there.
fn outside(post: &Post, icon_name: &'static str, label: impl Into<SharedString>, action: impl Into<SharedString>, p: &MandaraPalette) -> AnyElement {
    let url = post.url.as_deref().map_or_else(|| mandara_services::reddit::web_link(&post.permalink), str::to_owned);
    glass_card(p).p(px(tokens::MD)).row().items_center().gap(px(tokens::MD))
        .child(icon(icon_name, crate::kit::metrics::TILE_ICON, readable(p.secondary, p)))
        .child(div().flex_1().min_w_0().text_size(px(text::BODY_S)).truncate().child(label.into()))
        .child(pill_button("reddit-outside", Some("external"), action, readable(p.primary, p), p, move |_, _| open_link(url.clone())).focusable().tab_index(0))
        .into_any_element()
}

/// "N comments", with loading and error states for the thread.
fn comments_header(m: &Model, p: &MandaraPalette) -> AnyElement {
    let Some(reader) = &m.reader else { return div().into_any_element() };
    let total = reader.post.as_ref().map_or(0, |post| post.comments);
    let mut column = div().w_full().pb(px(tokens::SM)).flex().flex_col().gap(px(tokens::XS))
        .child(div().row().items_center().gap(px(tokens::SM))
            .child(icon("message", crate::kit::metrics::TILE_ICON, readable(p.primary, p)))
            .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).child(if reader.limited { t("Comments").to_string() } else { tf("{count} comments", &[("count", &count(total as i64))]) })));
    if reader.loading {
        column = column.child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(t("Loading comments…")));
    } else if let Some(error) = &reader.error && reader.post.is_some() {
        column = column.child(div().text_size(px(text::LABEL)).text_color(readable(p.warning_amber, p).hsla()).child(error.to_string()));
    } else if reader.rows.is_empty() {
        column = column.child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(t("No comments yet")));
    }
    if reader.limited && !reader.loading {
        column = column.child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(t("Limited mode shows comments as one list, without scores or replies' structure")));
    }
    if reader.truncated {
        column = column.child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(t("This thread is very long; open it on Reddit to read the rest")));
    }
    column.into_any_element()
}

/// One comment or one "more replies" row.
fn comment_row(m: &Model, visible: usize, p: &MandaraPalette, now: i64, mono: &SharedString) -> AnyElement {
    let Some(reader) = &m.reader else { return div().into_any_element() };
    let Some(&index) = reader.visible.get(visible) else { return div().into_any_element() };
    let index = index as usize;
    let Some(row) = reader.rows.get(index) else { return div().into_any_element() };
    let depth = row.depth();
    let indent = INDENT_PX * f32::from(depth.min(MAX_INDENT_LEVELS));
    let line = level_color(depth, p);
    let content = match row {
        Row::Comment(c) => comment(reader, c, index, p, now, mono),
        Row::More(more) => {
            let loading = reader.loading_more.as_deref().is_some_and(|key| Some(key) == row.more_key());
            let label: SharedString = if loading { t("Loading…").into() }
                else if more.children.is_empty() { t("Continue this thread").into() }
                else { tf("{count} more replies", &[("count", &more.count.max(more.children.len() as u32))]).into() };
            pill_button(SharedString::from(format!("reddit-more-{index}")), Some(if more.children.is_empty() { "chevron-right" } else { "chevron-down" }), label, readable(p.primary, p), p,
                move |_, cx| with_model(cx, |m| m.expand(index))).focusable().tab_index(0).into_any_element()
        }
    };
    div().w_full().pb(px(tokens::SM)).row().items_stretch().gap(px(tokens::SM))
        .child(div().w(px(indent)).flex_none())
        .when(depth > 0, |el| el.child(div().w(px(THREAD_LINE_PX)).flex_none().rounded_full().bg(line.with_alpha(alpha::SECONDARY_TEXT).hsla())))
        .child(div().flex_1().min_w_0().child(content))
        .into_any_element()
}

fn comment(reader: &Reader, c: &model::CommentRow, index: usize, p: &MandaraPalette, now: i64, mono: &SharedString) -> AnyElement {
    let folded = reader.collapsed.contains(&c.id);
    let id = c.id.clone();
    let mut head = div().id(SharedString::from(format!("reddit-fold-{index}"))).focusable().tab_index(0).cursor_pointer()
        .row().items_center().gap(px(tokens::XS)).text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla())
        .hover(|s| s.opacity(alpha::SECONDARY_TEXT))
        .child(icon(if folded { crate::bidi::dir_icon("chevron-right") } else { "chevron-down" }, crate::kit::metrics::TILE_ICON * alpha::SECONDARY_TEXT, p.on_surface_variant))
        .child(div().font_weight(FontWeight::SEMIBOLD).text_color(if c.op { readable(p.primary, p).hsla() } else { p.on_surface.hsla() }).truncate().child(format!("u/{}", c.author)));
    if c.op { head = head.child(badge(None, t("OP"), readable(p.primary, p), p)); }
    if c.distinguished { head = head.child(badge(Some("shield"), t("Moderator"), p.success_emerald, p)); }
    if c.stickied { head = head.child(badge(Some("pin"), t("Pinned"), p.success_emerald, p)); }
    if !reader.limited { head = head.child("·").child(if c.score_hidden { div().child(t("score hidden")) } else { stat("arrow-up", count(c.score), p) }); }
    head = head
        .child("·").child(age(c.created, now));
    if c.edited { head = head.child("·").child(t("edited")); }
    if folded {
        let hidden = reader.hidden_under(index);
        if hidden > 0 { head = head.child("·").child(tf("{count} replies hidden", &[("count", &hidden)])); }
    }
    let head = head.on_click(move |_, _, cx| with_model(cx, |m| if let Some(r) = &mut m.reader {
        if !r.collapsed.remove(&id) { r.collapsed.insert(id.clone()); }
        r.relayout();
    }));
    let mut column = div().w_full().flex().flex_col().gap(px(tokens::XS)).child(head);
    if !folded {
        let rcx = RenderCx { id: SharedString::from(format!("reddit-c-{}", c.id)), on_toggle: None, mono: mono.clone() };
        column = column.child(div().text_size(px(text::BODY_S)).child(crate::markdown::render(c.body.shown(reader.revealed), p, &rcx)));
        if c.body.has_spoilers() && !reader.revealed {
            column = column.child(pill_button(SharedString::from(format!("reddit-spoiler-{index}")), Some("eye"), t("Reveal spoilers"), readable(p.primary, p), p,
                |_, cx| with_model(cx, |m| if let Some(r) = &mut m.reader { r.revealed = true })).focusable().tab_index(0));
        }
    }
    column.into_any_element()
}
