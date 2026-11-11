//! A full-width card standing in for a tab that belongs to an opt-in
//! plugin which is off (Themes → Boot): the plugin's cover, its name and
//! one line, what it will and won't do, and Turn on (the same action as
//! the plugin pool's switch). Once the plugin is on, the tab's real
//! content replaces it on the next frame.
//!
//! API: describe the plugin with a [`Gate`] (id, title, description,
//! bullets; the cover comes from the plugin's embedded art) and call
//! [`StudioView::plugin_gate`]. [`release`] drops the decoded covers.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{AnyElement, ClickEvent, Context, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, RenderImage, SharedString, StatefulInteractiveElement, Styled, StyledImage as _, div, img, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui as _;
use mandara_i18n::{t, tr};
use mandara_plugins::PluginRegistry;
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_ui::plugins::lifecycle::Action;

use super::StudioView;
use super::compositor_page::button;
use super::plugin_modal::ModalView;
use crate::tokens::*;

/// One opt-in plugin's gate.
pub(super) struct Gate {
    pub id: &'static str,
    /// `tk` text.
    pub title: &'static str,
    /// `tk` text: one line.
    pub description: &'static str,
    /// `(icon, tk text)`: what turning it on does and doesn't do.
    pub bullets: &'static [(&'static str, &'static str)],
}

/// The cover is decoded at this width (device px; 2× the drawn width at
/// most, so it stays sharp on HiDPI without decoding per scale).
const COVER_DECODE_W: u32 = 960;
const COVER_DECODE_H: u32 = 540;
/// The card is split (cover beside the text) from this column width.
const SPLIT_FROM_PX: f32 = 2.0 * DIM_380_PX;
/// The cover's share of a split card.
const COVER_SHARE: f32 = 0.48;
const COVER_ASPECT: f32 = 9.0 / 16.0;
const DIAGONAL_DEG: f32 = 135.0;
const TILE_PX: f32 = DIM_44_PX;

#[derive(Default)]
struct GateArt {
    covers: HashMap<&'static str, Arc<RenderImage>>,
    loading: HashSet<&'static str>,
}
impl Global for GateArt {}

fn art(cx: &mut gpui::App) -> &mut GateArt {
    cx.default_global::<GateArt>()
}

/// Drops the decoded covers and their textures (leaving the page).
pub(super) fn release(cx: &mut gpui::App) {
    let a = art(cx);
    a.loading.clear();
    let covers: Vec<Arc<RenderImage>> = a.covers.drain().map(|(_, c)| c).collect();
    a.covers.shrink_to_fit();
    for c in covers {
        cx.drop_image(c, None);
    }
}

impl StudioView {
    /// The cover of `id`, decoded off the UI thread on first use.
    fn gate_cover(id: &'static str, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        if let Some(c) = art(cx).covers.get(id) {
            return Some(c.clone());
        }
        if !art(cx).loading.insert(id) {
            return None;
        }
        let task = cx.background_executor().spawn(async move { mandara_ui::plugin_art::cover_thumb(id, COVER_DECODE_W, COVER_DECODE_H) });
        cx.spawn(async move |this, cx| {
            let cover = task.await;
            let _ = this.update(cx, |_, cx| {
                // Left the page meanwhile: the late result is dropped.
                if !art(cx).loading.remove(id) {
                    if let Some(c) = cover {
                        cx.drop_image(c, None);
                    }
                    return;
                }
                if let Some(c) = cover {
                    art(cx).covers.insert(id, c);
                }
                cx.notify();
            });
        })
        .detach();
        None
    }

    /// The gate for `gate` (nothing when its plugin isn't registered).
    pub(super) fn plugin_gate(&mut self, gate: &'static Gate, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let id = gate.id;
        let Some(tile) = cx.global::<PluginRegistry>().manifest(id).map(|m| super::plugin_tile(m, TILE_PX, true, 1.0, p)) else { return div().into_any_element() };
        let display = mandara_ui::fonts::fonts(cx).display.clone();
        let split = self.page.column_w >= SPLIT_FROM_PX && !self.stack_rows;
        let cover_w = if split { (self.page.column_w * COVER_SHARE).floor() } else { self.page.column_w };
        let cover_h = (cover_w * COVER_ASPECT).floor();
        let cover = match Self::gate_cover(id, cx) {
            Some(image) => img(image).w(px(cover_w)).h(px(cover_h)).object_fit(gpui::ObjectFit::Cover).into_any_element(),
            None => div().size_full().bg(mandara_core::oklab_gradient(DIAGONAL_DEG, p.surface_container_high, p.primary_container)).into_any_element(),
        };
        let cover = div().w(px(cover_w)).h(px(cover_h)).flex_none().rounded(px(RADIUS_20_PX)).overflow_hidden().child(cover);
        let bullets = div().row().flex_col().gap(px(SPACE_10_PX)).children(gate.bullets.iter().map(|(glyph, text)| {
            div()
                .row()
                .items_start()
                .gap(px(SPACE_10_PX))
                .child(div().size(px(DIM_26_PX)).flex_none().rounded_full().bg(p.primary.with_alpha(ALPHA_14).hsla()).row().items_center().justify_center().child(icon(glyph, ICON_13_PX, p.primary)))
                .child(div().flex_1().min_w(px(0.0)).pt(px(SPACE_4_PX)).text_size(px(TEXT_13_PX)).line_height(px(LEADING_19_PX)).child(mandara_ui::bidi::paragraph(tr(text))))
        }));
        let on = button(SharedString::from(format!("gate-{id}-on")), "check", mandara_i18n::tf("Turn on {plugin}", &[("plugin", &tr(gate.title))]), true, p).tab_index(0).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.plugin_lifecycle(id, Action::Enable, cx)));
        let details = div()
            .id(SharedString::from(format!("gate-{id}-details")))
            .cursor_pointer()
            .text_size(px(TEXT_12_5_PX))
            .text_color(p.on_surface_variant.hsla())
            .hover(|s| s.text_color(p.on_surface.hsla()))
            .child(t("Details and permissions"))
            .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.open_plugin(id, ModalView::Detail, window, cx)));
        let text = div()
            .flex_1()
            .min_w(px(0.0))
            .row()
            .flex_col()
            .gap(px(SPACE_16_PX))
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_12_PX))
                    .child(tile)
                    .child(div().row().flex_col().gap(px(SPACE_3_PX)).child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.primary.hsla()).child(t("Off: nothing here runs yet"))).child(div().font_family(display).text_size(px(TEXT_24_PX)).font_weight(FontWeight::SEMIBOLD).child(tr(gate.title)))),
            )
            .child(div().text_size(px(TEXT_14_PX)).line_height(px(LEADING_21_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(tr(gate.description))))
            .child(bullets)
            .child(div().row().flex_wrap().items_center().gap(px(SPACE_16_PX)).child(on).child(details));
        div()
            .id(SharedString::from(format!("gate-{id}")))
            .rounded(px(RADIUS_26_PX))
            .p(px(SPACE_16_PX))
            .bg(mandara_core::oklab_gradient(DIAGONAL_DEG, p.surface_container, p.surface_container.mix(p.primary_container, ALPHA_35)))
            .border_1()
            .border_color(p.specular_rim_light.with_alpha(ALPHA_14).hsla())
            .row()
            .when(!split, |el| el.flex_col())
            .items_center()
            .gap(px(SPACE_24_PX))
            .child(cover)
            .child(text)
            .into_any_element()
    }
}
