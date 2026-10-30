//! A compact one-line card standing in for the settings of an opt-in plugin
//! that is off (Themes → Cursor, Wallpaper → Wallhaven): the plugin's tile,
//! "<Name>: turn on in Plugins", and a Turn on button. The name opens the
//! plugin's store page. Pages stay short until someone wants the feature.

use gpui::{AnyElement, ClickEvent, Context, FontWeight, InteractiveElement, IntoElement, ParentElement, SharedString, StatefulInteractiveElement, Styled, div, px};
use mandara_core::ToGpui as _;
use mandara_i18n::{t, tf};
use mandara_plugins::PluginRegistry;
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::plugins::lifecycle::Action;

use super::StudioView;
use super::compositor_page::button;
use super::plugin_modal::ModalView;
use crate::tokens::*;

/// The plugin tile beside the line.
const OFFER_TILE_PX: f32 = DIM_30_PX;

impl StudioView {
    /// The offer card for opt-in plugin `id` (nothing if it isn't registered).
    pub(super) fn plugin_offer(&mut self, id: &'static str, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let Some(manifest) = cx.global::<PluginRegistry>().manifests().find(|m| m.id == id).cloned() else { return div().into_any_element() };
        let name = manifest.display_name();
        div()
            .id(SharedString::from(format!("offer-{id}")))
            .rounded(px(RADIUS_16_PX))
            .bg(p.surface_container.hsla())
            .px(px(SPACE_12_PX))
            .py(px(SPACE_8_PX))
            .row()
            .items_center()
            .gap(px(SPACE_12_PX))
            .child(super::plugin_tile(&manifest, OFFER_TILE_PX, true, 1.0, p))
            .child(
                div()
                    .id(SharedString::from(format!("offer-{id}-details")))
                    .flex_1()
                    .min_w(px(0.0))
                    .truncate()
                    .text_start()
                    .cursor_pointer()
                    .text_size(px(TEXT_13_PX))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(p.on_surface_variant.hsla())
                    .hover(|s| s.text_color(p.on_surface.hsla()))
                    .child(tf("{plugin}: turn on in Plugins", &[("plugin", &name)]))
                    .on_click(cx.listener(move |this, _: &ClickEvent, window, cx| this.open_plugin(id, ModalView::Detail, window, cx))),
            )
            .child(button(SharedString::from(format!("offer-{id}-on")), "check", t("Turn on"), false, p).tab_index(0).on_click(cx.listener(move |this, _: &ClickEvent, _, cx| this.plugin_lifecycle(id, Action::Enable, cx))))
            .into_any_element()
    }
}
