//! The sidebar card: the first few posts of one community. It asks for
//! them only while it is shown, at most every few minutes, through the same
//! worker (and limited mode's one-a-minute queue) as the panel.

use std::time::{Duration, Instant};

use gpui::{AnyElement, App, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, Window, div, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui;
use mandara_i18n::t;
use mandara_services::reddit::{Listing, Query, Subreddit};
use mandara_theme::MandaraPalette;

use super::model::{self, Card};
use super::panel::{age, count, hidden, stat};
use super::{Settings, current};
use crate::bidi::Bidi as _;
use crate::kit::{alpha, text, tokens};
use crate::plugins::panel_fx::{EXPIRY_MARGIN, glass_card, icon_chip, readable, redraw_in};

/// Posts the card shows.
pub(super) const CARD_POSTS: usize = 5;
/// The card refreshes no more often than this while shown.
const CARD_REFRESH: Duration = Duration::from_secs(600);
const CHIP_PX: f32 = 28.0;

/// The community the card shows: its own setting, else your first one.
pub(super) fn community(settings: &Settings) -> Option<Subreddit> {
    settings.card.clone().or_else(|| settings.subscriptions.first().cloned())
}

/// The posts the card lists: the first shown ones, NSFW gate applied.
pub(super) fn shown<'a>(page: &'a Listing, settings: &'a Settings) -> impl Iterator<Item = &'a mandara_services::reddit::Post> + 'a {
    page.posts.iter().filter(|post| !hidden(post, settings)).take(CARD_POSTS)
}

pub(super) fn render(p: &MandaraPalette, window: &mut Window, cx: &mut App) -> AnyElement {
    let settings = current(cx);
    let shared = model::model();
    shared.borrow().views.track(window);
    let Some(sub) = community(&settings) else { return div().into_any_element() };
    let want = super::want(window.scale_factor());
    let query = Query { subreddit: sub.clone(), sort: settings.sort, span: settings.span, search: None };
    let next = shared.borrow_mut().want_card(query, want, CARD_REFRESH, Instant::now());
    if let Some(next) = next { redraw_in(next + EXPIRY_MARGIN, window, cx); }
    model::pump(&shared, cx);
    let accent = readable(p.primary, p);
    let m = shared.borrow();
    let Card { page, loading, error, .. } = &m.card;
    let limited = page.as_ref().is_some_and(|page| page.limited);
    let open_sub = sub.clone();
    let mut column = glass_card(p).p(px(tokens::MD)).flex().flex_col().gap(px(tokens::SM))
        .child(div().id("reddit-card-head").cursor_pointer().row().items_center().gap(px(tokens::SM))
            .child(icon_chip("messages", accent, CHIP_PX))
            .child(div().flex_1().min_w_0().flex().flex_col()
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).truncate().child(sub.to_string()))
                .child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(if limited { t("Limited mode").to_string() } else { tr_sort(settings.sort) })))
            .on_click(move |_, _, cx| { let _ = super::open(open_sub.as_str(), cx); }));
    let now = chrono::Utc::now().timestamp();
    match page {
        Some(page) => {
            for (index, post) in shown(page, &settings).enumerate() {
                // The click finds its post in the shared page; nothing is copied per frame.
                let (page_ref, id) = (page.clone(), post.id.clone());
                let mut meta = div().row().items_center().gap(px(tokens::SM)).text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla());
                if !post.limited { meta = meta.child(stat("arrow-up", count(post.score), p)).child(stat("message", count(post.comments as i64), p)); }
                meta = meta.child(age(post.created, now));
                column = column.child(div().id(SharedString::from(format!("reddit-card-{index}"))).focusable().tab_index(0).cursor_pointer()
                    .rounded(px(crate::style::radius(tokens::SM))).px(px(tokens::XS)).py(px(tokens::XS))
                    .hover(|s| s.bg(p.primary.with_alpha(alpha::HOVER).hsla()))
                    .flex().flex_col().gap(px(tokens::XS))
                    .child(div().text_size(px(text::BODY_S)).font_weight(FontWeight::MEDIUM).line_clamp(2).child(crate::bidi::paragraph(post.title.to_string())))
                    .child(meta)
                    .on_click(move |_, _, cx| {
                        let shared = model::model();
                        let want = shared.borrow().want.unwrap_or(super::want(1.0));
                        if let Some(post) = page_ref.posts.iter().find(|post| post.id == id) { shared.borrow_mut().open_known(post, want); }
                        if let Err(error) = super::reveal(&shared, cx) { tracing::info!("reddit: {error}"); }
                    }));
            }
        }
        None => {
            let line = match (error, loading) {
                (Some(error), _) => error.to_string(),
                (None, _) if m.waiting_until.is_some() => t("Waiting for Reddit's one-a-minute limit…").into(),
                _ => t("Fetching posts…").into(),
            };
            column = column.child(div().text_size(px(text::LABEL)).text_color(p.on_surface_variant.hsla()).child(line));
        }
    }
    column.when(limited, |el| el.child(div().text_size(px(text::CAPTION)).text_color(p.on_surface_variant.hsla())
        .child(t("Scores and comment counts need your own Reddit app (Studio → Plugins → Reddit)")))).into_any_element()
}

fn tr_sort(sort: mandara_services::reddit::Sort) -> String { mandara_i18n::tr(super::panel::sort_label(sort)).to_string() }
