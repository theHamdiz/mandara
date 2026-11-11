//! The screen a Studio tab shows while the opt-in plugin behind it is off
//! (Themes → Cursors, Themes → Icons): the plugin's cover, its name and
//! description, what it will do and what it never touches, and one button
//! that switches it on (the plugin pool's own Enable). Once on, the tab
//! turns into its gallery live.
//!
//! API: describe the plugin with a [`Gate`] (two short lists of `tk`
//! text) and call [`StudioView::plugin_gate`] from the tab's render:
//!
//! ```ignore
//! const GATE: Gate = Gate { will: &[tk("…")], wont: &[tk("…")] };
//! if !cfg.plugins.opted_in(ID) { return self.plugin_gate(ID, &GATE, p, window, cx); }
//! ```
//!
//! The cover is decoded once at the size it is drawn, off the UI thread,
//! and dropped by [`release`] (Studio calls it when you leave the tab).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use gpui::{AnyElement, App, ClickEvent, Context, FontWeight, Global, InteractiveElement, IntoElement, ParentElement, RenderImage, SharedString, StatefulInteractiveElement, Styled, StyledImage as _, Window, div, img, prelude::FluentBuilder as _, px};
use mandara_core::ToGpui as _;
use mandara_i18n::{t, tf, tr};
use mandara_plugins::PluginRegistry;
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::icons::icon;
use mandara_ui::plugins::lifecycle::Action;

use super::StudioView;
use super::compositor_page::button;
use super::plugin_modal::ModalView;
use crate::tokens::*;

/// What a plugin will do when switched on and what it leaves alone (`tk`
/// text; shown with `tr`).
pub(super) struct Gate {
    pub will: &'static [&'static str],
    pub wont: &'static [&'static str],
}

/// Cover and text sit side by side from this column width.
const SPLIT_MIN_W_PX: f32 = 2.0 * DIM_380_PX;
/// The cover's share of a side-by-side row, and its widest.
const COVER_SHARE: f32 = 0.46;
const COVER_MAX_W_PX: f32 = 2.0 * DIM_280_PX;
const COVER_ASPECT: f32 = 9.0 / 16.0;
/// Decodes are keyed to this step of device pixels, so a small resize
/// keeps the picture it has.
const DECODE_STEP_PX: u32 = 64;
/// The plugin tile shown when it has no cover.
const TILE_PX: f32 = DIM_56_PX;
/// The cover's placeholder gradient while it decodes.
const DIAGONAL_DEG: f32 = 135.0;

#[derive(Default)]
struct GateArt {
    /// One cover per plugin, at the size last drawn.
    covers: HashMap<&'static str, ((u32, u32), Arc<RenderImage>)>,
    loading: HashSet<(&'static str, u32, u32)>,
}
impl Global for GateArt {}

/// Drops the decoded covers (and their textures).
pub(super) fn release(cx: &mut App) {
    let art = std::mem::take(cx.default_global::<GateArt>());
    for (_, (_, image)) in art.covers {
        cx.drop_image(image, None);
    }
}

fn step(v: f32) -> u32 {
    ((v.max(1.0) as u32).div_ceil(DECODE_STEP_PX)) * DECODE_STEP_PX
}

impl StudioView {
    /// The cover of `id` for a `w × h` box at `scale`, decoded off the UI
    /// thread on first use (`None` until ready).
    fn gate_cover(id: &'static str, w: f32, h: f32, scale: f32, cx: &mut Context<Self>) -> Option<Arc<RenderImage>> {
        let size = (step(w * scale), step(h * scale));
        let art = cx.default_global::<GateArt>();
        let current = art.covers.get(id).map(|(have, image)| (*have, image.clone()));
        // The right size, or one already on its way: draw what there is.
        if current.as_ref().is_some_and(|(have, _)| *have == size) || !art.loading.insert((id, size.0, size.1)) {
            return current.map(|(_, image)| image);
        }
        let task = cx.background_executor().spawn(async move { mandara_ui::plugin_art::cover_thumb(id, size.0, size.1) });
        cx.spawn(async move |this, cx| {
            let image = task.await;
            let _ = this.update(cx, |_, cx| {
                let art = cx.default_global::<GateArt>();
                // Left (released) since: the late picture is dropped.
                if !art.loading.remove(&(id, size.0, size.1)) {
                    return;
                }
                if let Some(image) = image
                    && let Some((_, old)) = art.covers.insert(id, (size, image))
                {
                    cx.drop_image(old, None);
                }
                cx.notify();
            });
        })
        .detach();
        current.map(|(_, image)| image)
    }

    /// The gate for opt-in plugin `id` (see the module docs).
    pub(super) fn plugin_gate(&mut self, id: &'static str, gate: &Gate, p: &MandaraPalette, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some((name, description, tile)) = cx.global::<PluginRegistry>().manifest(id).map(|m| (m.display_name(), m.display_description(), super::plugin_tile(m, TILE_PX, true, window.scale_factor(), p))) else {
            return div().into_any_element();
        };
        let fonts = mandara_ui::fonts::fonts(cx);
        let split = self.page.column_w >= SPLIT_MIN_W_PX;
        let cover_w = if split { (self.page.column_w * COVER_SHARE).min(COVER_MAX_W_PX) } else { self.page.column_w.min(COVER_MAX_W_PX) };
        let cover_h = (cover_w * COVER_ASPECT).round();
        let picture = Self::gate_cover(id, cover_w, cover_h, window.scale_factor(), cx);
        let cover = div()
            .w(px(cover_w))
            .h(px(cover_h))
            .flex_none()
            .relative()
            .rounded(px(RADIUS_20_PX))
            .overflow_hidden()
            .bg(mandara_core::oklab_gradient(DIAGONAL_DEG, p.surface_container_high, p.primary_container))
            .children(picture.map(|image| img(image).size_full().object_fit(gpui::ObjectFit::Cover)))
            // Without a cover of its own (a third-party plugin), its tile
            // stands in the middle.
            .when(!mandara_ui::plugin_art::has_art(id), |el| el.child(div().absolute().inset_0().row().items_center().justify_center().child(div().rounded(px(RADIUS_16_PX)).shadow(mandara_core::glass_rim(p.specular_rim_light, p.specular_rim_shadow)).child(tile))));
        let list = |title: String, items: &[&'static str], glyph: &'static str, color: mandara_theme::Color| {
            div()
                .flex_1()
                .min_w(px(DIM_200_PX))
                .row()
                .flex_col()
                .gap(px(SPACE_8_PX))
                .child(div().text_size(px(TEXT_12_PX)).font_weight(FontWeight::SEMIBOLD).text_color(p.on_surface_variant.hsla()).child(title))
                .children(items.iter().map(|item| {
                    div()
                        .row()
                        .items_start()
                        .gap(px(SPACE_8_PX))
                        .child(div().mt(px(SPACE_3_PX)).size(px(DIM_18_PX)).flex_none().rounded_full().bg(color.with_alpha(ALPHA_14).hsla()).row().items_center().justify_center().child(icon(glyph, ICON_11_PX, color)))
                        .child(div().flex_1().min_w(px(0.0)).text_size(px(TEXT_12_5_PX)).line_height(px(LEADING_19_PX)).child(mandara_ui::bidi::paragraph(tr(item))))
                }))
        };
        let text = div()
            .flex_1()
            .min_w(px(0.0))
            .row()
            .flex_col()
            .gap(px(SPACE_14_PX))
            .child(
                div()
                    .row()
                    .items_center()
                    .gap(px(SPACE_6_PX))
                    .child(div().size(px(DIM_8_PX)).rounded_full().bg(p.on_surface_variant.with_alpha(ALPHA_45).hsla()))
                    .child(div().text_size(px(TEXT_11_5_PX)).font_weight(FontWeight::MEDIUM).text_color(p.on_surface_variant.hsla()).child(t("Built-in plugin · off"))),
            )
            .child(div().font_family(fonts.display.clone()).text_size(px(TEXT_26_PX)).font_weight(FontWeight::SEMIBOLD).child(name.clone()))
            .child(div().text_size(px(TEXT_13_5_PX)).line_height(px(LEADING_21_PX)).text_color(p.on_surface_variant.hsla()).child(mandara_ui::bidi::paragraph(description)))
            .child(div().row().flex_wrap().gap(px(SPACE_16_PX)).child(list(t("When it's on").to_string(), gate.will, "check", p.success_emerald)).child(list(t("It never").to_string(), gate.wont, "minus", p.on_surface_variant)))
            .child(
                div()
                    .row()
                    .flex_wrap()
                    .items_center()
                    .gap(px(SPACE_10_PX))
                    .child(button(SharedString::from(format!("gate-{id}-on")), "check", tf("Turn on {plugin}", &[("plugin", &name)]), true, p).tab_index(0).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.plugin_lifecycle(id, Action::Enable, cx))))
                    .child(button(SharedString::from(format!("gate-{id}-details")), "info", t("Details"), false, p).tab_index(0).on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.open_plugin(id, ModalView::Detail, window, cx)))),
            )
            .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.with_alpha(ALPHA_80).hsla()).child(t("You can switch it off again here or in Plugins at any time.")));
        super::theme_galleries::glass_card(SharedString::from(format!("gate-{id}")), p.primary, 0.0, p)
            .p(px(SPACE_18_PX))
            .row()
            .when_flex(split)
            .gap(px(SPACE_22_PX))
            .child(cover)
            .child(text)
            .into_any_element()
    }
}

/// Row when side by side, column otherwise.
trait WhenFlex {
    fn when_flex(self, split: bool) -> Self;
}

impl WhenFlex for gpui::Stateful<gpui::Div> {
    fn when_flex(self, split: bool) -> Self {
        if split { self.items_start() } else { self.flex_col() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_sizes_step_so_small_resizes_keep_the_picture() {
        assert_eq!(step(1.0), DECODE_STEP_PX);
        assert_eq!(step(DECODE_STEP_PX as f32), DECODE_STEP_PX);
        assert_eq!(step(DECODE_STEP_PX as f32 + 1.0), 2 * DECODE_STEP_PX);
        assert_eq!(step(700.0), step(690.0));
    }
}
