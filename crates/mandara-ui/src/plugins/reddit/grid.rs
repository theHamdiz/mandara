//! The grid layout: image-first tiles, two or three to a row.

use std::rc::Rc;

use gpui::{AnyElement, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, list, px};
use mandara_core::ToGpui;
use mandara_i18n::t;
use mandara_services::reddit::{Kind, Post};
use mandara_theme::MandaraPalette;

use super::model::{self, Model, Shared};
use super::panel::{badge, changed, count, covered, footer, hidden, picture, stat};
use super::{Gate, Layout, Settings};
use crate::bidi::Bidi as _;
use crate::icons::icon;
use crate::kit::{alpha, text, tokens};
use crate::plugins::panel_fx::{WASH_SOFT, glass_card, wash};

/// A tile's picture is this tall relative to its width.
const TILE_ASPECT: f32 = 0.75;
const TILE_RADIUS_PX: f32 = 14.0;
const TILE_KIND_ICON_PX: f32 = 22.0;

/// The icon of a layout on the panel's switch.
pub(super) fn layout_icon(layout: Layout) -> &'static str {
    match layout { Layout::Cards => "note", Layout::Compact => "list-check", Layout::Grid => "tiles" }
}

/// The posts as tiles; only rows on screen are built.
pub(super) fn grid_list(m: &mut Model, shared: &Shared, settings: &Rc<Settings>, p: &MandaraPalette, scale: f32, width: f32) -> AnyElement {
    let columns = model::grid_columns(width);
    let tile = model::tile_width(width, columns);
    {
        let Model { grid, feed, .. } = m;
        grid.sync(feed, |post| hidden(post, settings), settings.nsfw == Gate::Hide, columns);
    }
    let state = m.grid.list.clone();
    let shared = shared.clone();
    let settings = settings.clone();
    let palette = p.clone();
    let lang = mandara_i18n::language();
    list(state, move |index, _, cx| {
        let row = mandara_i18n::with_language(Some(lang), || {
            let mut m = shared.borrow_mut();
            if index >= m.grid.rows() { return footer(&m, &palette); }
            let mut line = div().w_full().pb(px(model::GRID_GAP)).row().gap(px(model::GRID_GAP));
            let mut column = 0;
            while let Some(&feed_row) = m.grid.row(index).get(column) {
                column += 1;
                let Some((page, at)) = m.feed.locate(feed_row as usize) else { continue };
                let post = &page.posts[at];
                let revealed = m.revealed.contains(&post.id);
                line = line.child(tile_element(&mut m, post, &settings, revealed, &palette, scale, tile, feed_row as usize));
            }
            line.into_any_element()
        });
        model::pump_soon(&shared, cx);
        row
    }).w_full().flex_1().into_any_element()
}

/// One post as a tile: its picture first, then its title and a meta line.
fn tile_element(m: &mut Model, post: &Post, settings: &Settings, revealed: bool, p: &MandaraPalette, scale: f32, width: f32, index: usize) -> AnyElement {
    let covered = covered(post, settings, revealed);
    let height = (width * TILE_ASPECT).round();
    let image = post.media.first().or(post.thumb.as_ref()).filter(|_| settings.images);
    let shown = match image {
        Some(image) => picture(m, &image.url, width, height, covered, scale, TILE_RADIUS_PX, p),
        None => div().w(px(width)).h(px(height)).flex_none().rounded(px(crate::style::radius(TILE_RADIUS_PX))).bg(wash(WASH_SOFT, p).hsla())
            .flex().items_center().justify_center()
            .child(icon(match post.kind { Kind::Video => "play", Kind::Link => "external", Kind::Text => "note", Kind::Image | Kind::Gallery => "image" }, TILE_KIND_ICON_PX, p.on_surface_variant))
            .into_any_element(),
    };
    let mut marks = div().absolute().top(px(tokens::SM)).left(px(tokens::SM)).row().gap(px(tokens::XS));
    if post.nsfw { marks = marks.child(badge(None, t("NSFW"), p.urgent_coral, p)); }
    if post.spoiler { marks = marks.child(badge(None, t("Spoiler"), p.warning_amber, p)); }
    if post.kind == Kind::Gallery { marks = marks.child(badge(Some("image"), post.media.len().to_string(), p.secondary, p)); }
    let mut meta = div().row().items_center().gap(px(tokens::SM)).text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).min_w_0()
        .child(div().truncate().min_w_0().child(format!("r/{}", post.subreddit)));
    if !post.limited {
        meta = meta.child(stat("arrow-up", count(post.score), p)).child(stat("message", count(post.comments as i64), p));
    }
    let id = post.id.clone();
    let hover = p.primary.with_alpha(alpha::HOVER).hsla();
    let focus = p.primary.with_alpha(alpha::SELECTED).hsla();
    glass_card(p).id(SharedString::from(format!("reddit-tile-{index}"))).focusable().tab_index(0).cursor_pointer()
        .w(px(width)).flex_none().flex().flex_col().gap(px(tokens::XS)).pb(px(tokens::SM))
        .hover(move |s| s.bg(hover)).focus_visible(move |s| s.bg(focus))
        .child(div().relative().child(shown).child(marks))
        .child(div().px(px(tokens::SM)).text_size(px(text::BODY_S)).font_weight(FontWeight::SEMIBOLD).line_clamp(2).child(crate::bidi::paragraph(post.title.to_string())))
        .child(div().px(px(tokens::SM)).child(meta))
        .on_click(move |_, _, cx| {
            let shared = model::model();
            let want = shared.borrow().want;
            if let Some(want) = want { shared.borrow_mut().open_post(&id, want); }
            changed(&shared, cx);
        })
        .into_any_element()
}
