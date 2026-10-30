//! Privacy indicators: a bar chip that appears only while an app uses the
//! microphone, the camera or shares the screen (a glowing coral dot per
//! device), and a panel listing which apps. The data comes from
//! [`crate::alerts`] (the privacy monitor runs only while `[privacy]`
//! indicators are enabled).

use std::cell::Cell;
use std::rc::Rc;

use gpui::{AnyElement, FontWeight, IntoElement, ParentElement, Styled, div, prelude::FluentBuilder as _, px, size};
use mandara_core::{ToGpui, oklab_gradient};
use mandara_i18n::{t, tk};
use mandara_plugins::{CommandSpec, PanelCx, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_services::privacy::{AppUse, PrivacyState};
use mandara_theme::MandaraPalette;

use super::panel_fx::{ANGLE_DIAGONAL_DEG, HEADER_H, SECTION_H_PX, empty_state, glass_card, glow, header, partner, readable, section};
use super::{fn_view, json};
use crate::icons::icon;
use crate::kit::{reveal, text, tokens};

pub const ID: &str = "privacy";

const PANEL_W: f32 = 340.0;
const PAD: f32 = 14.0;
const EMPTY_H: f32 = 150.0;
/// Bar chip: device glyphs and the live dot.
const BAR_ICON_PX: f32 = 14.0;
const BAR_GAP_PX: f32 = 5.0;
const BAR_PAD_X_PX: f32 = 8.0;
const BAR_H_PX: f32 = 20.0;
const BAR_FILL_ALPHA: f32 = 0.16;
const DOT_PX: f32 = 6.0;
const DOT_GLOW_BLUR_PX: f32 = 6.0;
const DOT_GLOW_ALPHA: f32 = 0.6;
/// App rows.
const ROW_H: f32 = 46.0;
const ROW_PAD_X_PX: f32 = 12.0;
const ROW_GAP_PX: f32 = 10.0;
const ROW_GLYPH_PX: f32 = 30.0;
const ROW_ICON_PX: f32 = 15.0;
const ROW_RADIUS_PX: f32 = 12.0;
const ROW_FILL: (f32, f32) = (0.18, 0.08);
const LIST_GAP_PX: f32 = 6.0;
const SECTION_GAP_PX: f32 = 4.0;

/// The three device categories in display order.
fn categories(st: &PrivacyState) -> [(&'static str, &'static str, &[AppUse]); 3] {
    [("mic", t("Microphone"), &st.microphone), ("camera", t("Camera"), &st.camera), ("monitor", t("Screen sharing"), &st.screen_share)]
}

fn app_row(app: &AppUse, glyph: &'static str, palette: &MandaraPalette) -> AnyElement {
    let coral = readable(palette.urgent_coral, palette);
    glass_card(palette)
        .h(px(ROW_H))
        .px(px(ROW_PAD_X_PX))
        .rounded(px(crate::style::radius(ROW_RADIUS_PX)))
        .flex()
        .items_center()
        .gap(px(ROW_GAP_PX))
        .child(
            div()
                .size(px(ROW_GLYPH_PX))
                .flex_none()
                .rounded_full()
                .bg(oklab_gradient(ANGLE_DIAGONAL_DEG, coral.with_alpha(ROW_FILL.0), partner(coral).with_alpha(ROW_FILL.1)))
                .flex()
                .items_center()
                .justify_center()
                .child(icon(glyph, ROW_ICON_PX, coral)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::SEMIBOLD).truncate().child(app.name.clone()))
                .when_some(app.pid, |el, pid| el.child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(format!("PID {pid}")))),
        )
        .into_any_element()
}

pub fn register(r: &mut PluginRegistry) {
    r.register(PluginManifest::builtin(ID, tk("Privacy"), tk("Shows when apps use your microphone, camera or screen"), Some(Zone::Right)).opt_in(), |window, cx, panel_cx| {
        let bar = fn_view(
            None,
            |palette, _, cx| {
                let st = crate::alerts::privacy_state(cx);
                if !st.any() {
                    return div().into_any_element();
                }
                let coral = readable(palette.urgent_coral, palette);
                let mut chip = div().h(px(BAR_H_PX)).px(px(BAR_PAD_X_PX)).rounded_full().bg(coral.with_alpha(BAR_FILL_ALPHA).hsla()).flex().items_center().gap(px(BAR_GAP_PX));
                chip = chip.child(div().size(px(DOT_PX)).rounded_full().bg(coral.hsla()).shadow(vec![glow(coral.with_alpha(DOT_GLOW_ALPHA), DOT_GLOW_BLUR_PX, 0.0, 0.0)]));
                for (glyph, _, apps) in categories(&st) {
                    if !apps.is_empty() {
                        chip = chip.child(icon(glyph, crate::style::bar_icon(BAR_ICON_PX), coral));
                    }
                }
                chip.into_any_element()
            },
            window,
            cx,
        );
        let p: PanelCx = panel_cx.clone();
        let size_cell = Rc::new(Cell::new(size(px(PANEL_W), px(PAD * 2.0 + HEADER_H + tokens::MD + EMPTY_H))));
        let sc = size_cell.clone();
        let panel = fn_view(
            None,
            move |palette, _, cx| {
                let st = crate::alerts::privacy_state(cx);
                let accent = readable(if st.any() { palette.urgent_coral } else { palette.primary }, palette);
                let subtitle = crate::alerts::privacy_summary(&st);
                let mut col = div().flex().flex_col().gap(px(tokens::MD)).child(reveal(&p, 0, header("shield", accent, t("Privacy"), subtitle, palette, None)));
                let mut h = PAD * 2.0 + HEADER_H;
                if !st.any() {
                    h += tokens::MD + EMPTY_H;
                    col = col.child(empty_state("shield", t("All clear"), t("Apps using your microphone, camera or screen show up here"), EMPTY_H, palette));
                } else {
                    for (i, (glyph, label, apps)) in categories(&st).into_iter().enumerate() {
                        if apps.is_empty() {
                            continue;
                        }
                        h += tokens::MD + SECTION_H_PX + SECTION_GAP_PX + apps.len() as f32 * (ROW_H + LIST_GAP_PX);
                        let list = div().flex().flex_col().gap(px(LIST_GAP_PX)).children(apps.iter().map(|a| app_row(a, glyph, palette)));
                        col = col.child(reveal(&p, i + 1, div().flex().flex_col().gap(px(SECTION_GAP_PX)).child(section(label, palette, None)).child(list)));
                    }
                }
                sc.set(size(px(PANEL_W), px(h)));
                div().size_full().p(px(PAD)).child(col).into_any_element()
            },
            window,
            cx,
        );
        PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), sidebar: None, on_click: None, on_secondary_click: None }
    });
    r.command(ID, CommandSpec::new("privacy status", tk("Apps using the microphone, camera or screen right now"), |_, cx| Ok(json(&crate::alerts::privacy_state(cx)))));
}
