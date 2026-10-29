//! Language switcher: the interface language as a compact chip in the bar;
//! the panel shows the current language in its own script and every
//! supported language to switch to instantly, plus the keyboard layout.

use std::cell::Cell;
use std::rc::Rc;

use gpui::{AnyElement, FontWeight, InteractiveElement, IntoElement, ParentElement, StatefulInteractiveElement, Styled, div, prelude::FluentBuilder as _, px, size};
use mandara_config::Locale;
use mandara_core::ToGpui;
use mandara_i18n::{Lang, t, tf};
use mandara_plugins::{ArgKind, ArgSpec, CommandSpec, PanelSpec, PluginInstance, PluginManifest, PluginRegistry, Zone};
use mandara_theme::MandaraPalette;
use serde_json::json;

use super::fn_view;
use super::panel_fx::ANGLE_DIAGONAL_DEG;
use crate::fonts::fonts;
use crate::hub::hub;
use crate::icons::icon;
use crate::kit::{reveal, text, tokens};
use crate::state::{config, update_config};

pub const ID: &str = "language";

// Language options.
const OPTION_H_PX: f32 = 52.0;
const OPTION_PAD_X_PX: f32 = 12.0;
const OPTION_RADIUS_PX: f32 = 14.0;
const OPTION_GAP_PX: f32 = 10.0;
const OPTION_BADGE_PX: f32 = 30.0;
const OPTION_BADGE_RADIUS_PX: f32 = 10.0;
/// Selected / idle / hover fills and the selected outline.
const SELECTED_ALPHA: f32 = 0.14;
const IDLE_ALPHA: f32 = 0.04;
const HOVER_ALPHA: f32 = 0.10;
const OUTLINE_ALPHA: f32 = 0.55;
const CHECK_PX: f32 = 15.0;
/// Bar chip.
const BAR_GAP_PX: f32 = 5.0;
const BAR_ICON_PX: f32 = 15.0;
const BADGE_TEXT_PX: f32 = 10.5;
// Hero card.
const HERO_RADIUS_PX: f32 = 18.0;
const HERO_PAD_PX: f32 = 16.0;
const HERO_GAP_PX: f32 = 14.0;
const HERO_FILL: (f32, f32) = (0.22, 0.14);
const HERO_BADGE_PX: f32 = 54.0;
const HERO_BADGE_RADIUS_PX: f32 = 16.0;
const HERO_BADGE_TEXT_PX: f32 = 20.0;
const HERO_SAMPLE_PX: f32 = 22.0;
const HERO_TEXT_GAP_PX: f32 = 2.0;
const RTL_PILL_PAD_X_PX: f32 = 8.0;
const RTL_PILL_H_PX: f32 = 22.0;
const RTL_PILL_ALPHA: f32 = 0.08;
/// Language grid columns.
const GRID_COLS: u16 = 2;
// Keyboard row.
const KB_RADIUS_PX: f32 = 14.0;
const KB_PAD_X_PX: f32 = 12.0;
const KB_H_PX: f32 = 48.0;
const KB_GAP_PX: f32 = 10.0;
const KB_ICON_PX: f32 = 16.0;
const KB_BUTTON_H_PX: f32 = 30.0;
const HEADER_ICON_PX: f32 = 17.0;
// Panel height: padding, header row, hero, one option row stride (option +
// gap), the formula's trim of the extra row, and the keyboard row stride.
const PANEL_W_PX: f32 = 400.0;
const INITIAL_H_PX: f32 = 560.0;
const HEADER_ROW_PX: f32 = 24.0;
const HERO_H_PX: f32 = 86.0;
const OPTION_STRIDE_PX: f32 = 60.0;
const GRID_TRIM_PX: f32 = 44.0;
const KB_STRIDE_PX: f32 = 60.0;

/// A short badge for the bar: the code, or the script's own glyphs where
/// that reads better.
fn badge(lang: Lang) -> &'static str {
    match lang {
        Lang::Ar => "ع",
        Lang::Zh => "中",
        Lang::Ja => "日",
        Lang::Hi => "हि",
        Lang::Ru => "RU",
        Lang::En => "EN",
        Lang::Es => "ES",
        Lang::Fr => "FR",
        Lang::De => "DE",
        Lang::Tr => "TR",
    }
}

fn set(locale: Locale, cx: &mut gpui::App) {
    update_config(cx, |c| c.general.locale = locale);
}

#[allow(clippy::too_many_arguments)]
fn option(id: impl Into<gpui::ElementId>, lang: Lang, label_native: String, label_sub: String, selected: bool, locale: Locale, palette: &MandaraPalette, arabic_font: gpui::SharedString) -> AnyElement {
    let rtl = lang.is_rtl();
    div()
        .id(id)
        .h(px(OPTION_H_PX))
        .px(px(OPTION_PAD_X_PX))
        .rounded(px(crate::style::radius(OPTION_RADIUS_PX)))
        .flex()
        .items_center()
        .gap(px(OPTION_GAP_PX))
        .cursor_pointer()
        .bg(if selected { palette.primary.with_alpha(SELECTED_ALPHA).hsla() } else { palette.on_surface.with_alpha(IDLE_ALPHA).hsla() })
        .when(selected, |el| el.border_1().border_color(palette.primary.with_alpha(OUTLINE_ALPHA).hsla()))
        .hover(|s| s.bg(palette.primary.with_alpha(HOVER_ALPHA).hsla()))
        .child(
            div()
                .size(px(OPTION_BADGE_PX))
                .flex_none()
                .rounded(px(crate::style::radius(OPTION_BADGE_RADIUS_PX)))
                .flex()
                .items_center()
                .justify_center()
                .bg(if selected { palette.primary.hsla() } else { palette.surface_container_high.hsla() })
                .text_color(if selected { palette.on_primary.hsla() } else { palette.on_surface_variant.hsla() })
                .text_size(px(text::BODY_S))
                .font_weight(FontWeight::BOLD)
                .when(rtl, |el| el.font_family(arabic_font.clone()))
                .child(badge(lang)),
        )
        .child(
            div()
                .flex_1()
                .min_w(px(0.0))
                .flex()
                .flex_col()
                .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).truncate().when(rtl, |el| el.font_family(arabic_font.clone())).child(label_native))
                .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).truncate().child(label_sub)),
        )
        .when(selected, |el| el.child(icon("check", CHECK_PX, palette.primary)))
        .on_click(move |_, _, cx| set(locale, cx))
        .into_any_element()
}

pub fn register(r: &mut PluginRegistry) {
    r.register(
        PluginManifest::builtin(ID, mandara_i18n::tk("Language"), mandara_i18n::tk("Switch the interface language instantly"), Some(Zone::Right)),
        |window, cx, panel_cx| {
            let bar = fn_view(
                None,
                |palette, _, cx| {
                    let lang = config(cx).read(cx).get().general.locale.lang();
                    div()
                        .flex()
                        .items_center()
                        .gap(px(BAR_GAP_PX))
                        .child(icon("globe", crate::style::bar_icon(BAR_ICON_PX), palette.on_surface))
                        .child(
                            div()
                                .text_size(px(BADGE_TEXT_PX))
                                .font_weight(FontWeight::BOLD)
                                .text_color(palette.on_surface_variant.hsla())
                                .when(lang.is_rtl(), |el| el.font_family(fonts(cx).arabic.clone()))
                                .child(badge(lang)),
                        )
                        .into_any_element()
                },
                window,
                cx,
            );
            let p = panel_cx.clone();
            let size_cell = Rc::new(Cell::new(size(px(PANEL_W_PX), px(INITIAL_H_PX))));
            let sc = size_cell.clone();
            let panel = fn_view(
                None,
                move |palette, _, cx| {
                    let locale = config(cx).read(cx).get().general.locale;
                    let current = locale.lang();
                    let system = Lang::from_env();
                    let arabic = fonts(cx).arabic.clone();
                    let layout: gpui::SharedString = hub(cx).read(cx).keyboard_layout.clone().into();
                    let rtl = current.is_rtl();

                    // Hero: the current language, in itself.
                    let hero = div()
                        .rounded(px(crate::style::radius(HERO_RADIUS_PX)))
                        .p(px(HERO_PAD_PX))
                        .flex()
                        .items_center()
                        .gap(px(HERO_GAP_PX))
                        .bg(mandara_core::oklab_gradient(ANGLE_DIAGONAL_DEG, palette.primary.with_alpha(HERO_FILL.0), palette.tertiary.with_alpha(HERO_FILL.1)))
                        .child(
                            div()
                                .size(px(HERO_BADGE_PX))
                                .flex_none()
                                .rounded(px(crate::style::radius(HERO_BADGE_RADIUS_PX)))
                                .flex()
                                .items_center()
                                .justify_center()
                                .bg(palette.primary.hsla())
                                .text_color(palette.on_primary.hsla())
                                .text_size(px(HERO_BADGE_TEXT_PX))
                                .font_weight(FontWeight::BOLD)
                                .when(rtl, |el| el.font_family(arabic.clone()))
                                .child(badge(current)),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.0))
                                .flex()
                                .flex_col()
                                .gap(px(HERO_TEXT_GAP_PX))
                                .child(div().text_size(px(HERO_SAMPLE_PX)).font_weight(FontWeight::SEMIBOLD).when(rtl, |el| el.font_family(arabic.clone())).child(current.sample()))
                                .child(
                                    div()
                                        .text_size(px(text::BODY_S))
                                        .text_color(palette.on_surface_variant.hsla())
                                        .child(if locale == Locale::Auto { tf("{language} · follows your desktop", &[("language", &current.native_name())]) } else { format!("{} · {}", current.native_name(), mandara_i18n::tr(current.english_name())) }),
                                ),
                        )
                        .when(rtl, |el| el.child(div().px(px(RTL_PILL_PAD_X_PX)).h(px(RTL_PILL_H_PX)).rounded_full().flex().items_center().bg(palette.on_surface.with_alpha(RTL_PILL_ALPHA).hsla()).text_size(px(BADGE_TEXT_PX)).font_weight(FontWeight::BOLD).child(t("RTL"))));

                    // Every language, two columns; System first.
                    let mut grid = div().grid().grid_cols(GRID_COLS).gap(px(tokens::SM));
                    grid = grid.child(option(
                        "lang-opt-auto",
                        system,
                        t("System").into(),
                        system.native_name().into(),
                        locale == Locale::Auto,
                        Locale::Auto,
                        palette,
                        arabic.clone(),
                    ));
                    for (i, lang) in Lang::ALL.into_iter().enumerate() {
                        let l = Locale::from_lang(lang);
                        grid = grid.child(option(("lang-opt", i), lang, lang.native_name().into(), mandara_i18n::tr(lang.english_name()), locale == l, l, palette, arabic.clone()));
                    }

                    // Keyboard layout (when the compositor reports it).
                    let keyboard = (!layout.is_empty()).then(|| {
                        div()
                            .rounded(px(crate::style::radius(KB_RADIUS_PX)))
                            .px(px(KB_PAD_X_PX))
                            .h(px(KB_H_PX))
                            .flex()
                            .items_center()
                            .gap(px(KB_GAP_PX))
                            .bg(palette.on_surface.with_alpha(IDLE_ALPHA).hsla())
                            .child(icon("keyboard", KB_ICON_PX, palette.on_surface_variant))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w(px(0.0))
                                    .flex()
                                    .flex_col()
                                    .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(t("Keyboard layout")))
                                    .child(div().text_size(px(text::BODY)).font_weight(FontWeight::MEDIUM).truncate().child(layout.clone())),
                            )
                            .child(
                                div()
                                    .id("lang-next-layout")
                                    .h(px(KB_BUTTON_H_PX))
                                    .px(px(KB_PAD_X_PX))
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .cursor_pointer()
                                    .bg(palette.surface_container_high.hsla())
                                    .text_size(px(text::BODY_S))
                                    .font_weight(FontWeight::MEDIUM)
                                    .hover(|s| s.bg(palette.primary.with_alpha(SELECTED_ALPHA).hsla()))
                                    .child(t("Switch"))
                                    .on_click(|_, _, cx| {
                                        if let Some(b) = hub(cx).read(cx).compositor.clone() {
                                            let _ = b.switch_keyboard_layout();
                                        }
                                    }),
                            )
                    });

                    let header = div()
                        .flex()
                        .items_center()
                        .gap(px(tokens::SM))
                        .child(icon("globe", HEADER_ICON_PX, palette.primary))
                        .child(div().flex_1().font_weight(FontWeight::SEMIBOLD).child(t("Language")))
                        .child(div().text_size(px(text::LABEL)).text_color(palette.on_surface_variant.hsla()).child(t("Applies instantly")));
                    let rows = 1 + (Lang::ALL.len() + 1).div_ceil(2);
                    let h = tokens::LG * 2.0 + HEADER_ROW_PX + tokens::MD + HERO_H_PX + tokens::MD + rows as f32 * OPTION_STRIDE_PX - GRID_TRIM_PX
                        + if layout.is_empty() { 0.0 } else { KB_STRIDE_PX };
                    sc.set(size(px(PANEL_W_PX), px(h)));
                    let blocks: Vec<AnyElement> = [Some(header.into_any_element()), Some(hero.into_any_element()), Some(grid.into_any_element()), keyboard.map(|k| k.into_any_element())].into_iter().flatten().collect();
                    div()
                        .size_full()
                        .p(px(tokens::LG))
                        .flex()
                        .flex_col()
                        .gap(px(tokens::MD))
                        .children(blocks.into_iter().enumerate().map(|(i, b)| reveal(&p, i, b)))
                        .into_any_element()
                },
                window,
                cx,
            );
            PluginInstance { bar: Some(bar), panel: Some(PanelSpec { view: panel, size: size_cell }), ..PluginInstance::empty() }
        },
    );
    r.command(
        ID,
        CommandSpec::new("language set", mandara_i18n::tk("Set the interface language (auto or an ISO code: en, ar, es, fr, de, tr, ru, hi, zh, ja)"), |a, cx| {
            let code = a.get(0).ok_or("expected a language code")?;
            let locale = if code == "auto" { Locale::Auto } else { Locale::from_lang(Lang::from_code(code).ok_or_else(|| format!("unsupported language {code:?}"))?) };
            set(locale, cx);
            Ok(json!({ "language": locale.lang().code(), "auto": locale == Locale::Auto }))
        })
        .arg(ArgSpec::required("code", ArgKind::String, mandara_i18n::tk("auto or a language code")))
        .example("language set ar"),
    );
    r.command(
        ID,
        CommandSpec::new("language list", mandara_i18n::tk("Supported interface languages"), |_, cx| {
            let current = config(cx).read(cx).get().general.locale.lang();
            Ok(json!(Lang::ALL.iter().map(|l| json!({ "code": l.code(), "name": l.native_name(), "english": l.english_name(), "rtl": l.is_rtl(), "current": *l == current })).collect::<Vec<_>>()))
        }),
    );
}
