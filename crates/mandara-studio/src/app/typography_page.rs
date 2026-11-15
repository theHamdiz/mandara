//! Studio → Typography: a live specimen of the four families (each drawn
//! in its own face, at the chosen weight and scale) over the settings.

use gpui::{AnyElement, Context, FontWeight, IntoElement, ParentElement, SharedString, Styled, div, prelude::FluentBuilder as _, px};
use mandara_config::ShellConfig;
use mandara_core::ToGpui;
use mandara_i18n::t;
use mandara_theme::MandaraPalette;
use mandara_ui::bidi::Bidi as _;
use mandara_ui::components::surfaces::{CardSurface as _, Fill};

use super::{StudioView, grid, grouped};
use crate::registry::{self, Section};
use crate::tokens::*;

/// The specimen tiles: four across, two by two, or a column.
const SPECIMEN_GRID: CardGrid = CardGrid { min_w: 220.0, max_w: 1000.0, gap: BENTO_GAP_PX };
/// A tile's large glyph pair and its glyph line.
const SPECIMEN_GLYPH_PX: f32 = 46.0;
const SPECIMEN_LINE_PX: f32 = 15.0;
/// Glyph specimens, not words: the shapes that tell faces apart (no
/// language to translate). Latin, Arabic and the monospace tells.
const LATIN_PAIR: &str = "Aa";
const LATIN_LINE: &str = "Aa Bb Cc Gg Rr 0123456789";
const ARABIC_PAIR: &str = "أب";
const ARABIC_LINE: &str = "أ ب ج د هـ و ز ح ط ١٢٣٤٥";
const MONO_PAIR: &str = "{}";
const MONO_LINE: &str = "0O 1lI rn m {} [] => 0123";

impl StudioView {
    pub(super) fn typography_page(&mut self, cfg: &ShellConfig, p: &MandaraPalette, cx: &mut Context<Self>) -> AnyElement {
        let fonts = mandara_ui::fonts::fonts(cx);
        let weight = FontWeight(f32::from(cfg.typography.weight));
        let tiles = [
            (fonts.ui.clone(), t("Interface font"), LATIN_PAIR, LATIN_LINE, weight, false),
            (fonts.display.clone(), t("Display font"), LATIN_PAIR, LATIN_LINE, FontWeight::MEDIUM, false),
            (fonts.arabic.clone(), t("Arabic font"), ARABIC_PAIR, ARABIC_LINE, weight, true),
            (fonts.mono.clone(), t("Monospace font"), MONO_PAIR, MONO_LINE, FontWeight::NORMAL, false),
        ];
        let fit = grid::fit(self.page.column_w, SPECIMEN_GRID).balanced(tiles.len());
        let line = grouped::hairline(p);
        let cards = tiles.into_iter().map(|(family, role, pair, sample, weight, rtl)| {
            div()
                .flex_1()
                .h_full()
                .clipped_card_surface(grouped::radius(), Fill(p.surface_container, None), Vec::new(), p)
                .p(px(SPACE_18_PX))
                .row()
                .flex_col()
                .gap(px(SPACE_10_PX))
                .child(
                    div()
                        .row()
                        .items_end()
                        .justify_between()
                        .gap(px(SPACE_10_PX))
                        .child(div().font_family(family.clone()).text_size(px(SPECIMEN_GLYPH_PX)).font_weight(weight).line_height(px(SPECIMEN_GLYPH_PX)).child(pair))
                        .child(div().text_size(px(TEXT_11_5_PX)).text_color(p.on_surface_variant.hsla()).child(role)),
                )
                .child(div().w_full().truncate().text_start().text_size(px(TEXT_15_PX)).font_weight(FontWeight::SEMIBOLD).child(family.clone()))
                .child(div().h(px(BORDER_1_PX)).w_full().bg(line))
                .child(
                    div()
                        .w_full()
                        .truncate()
                        .font_family(family)
                        .font_weight(weight)
                        .text_size(px(SPECIMEN_LINE_PX * cfg.typography.scale.clamp(SCALE_MIN, SCALE_MAX)))
                        .text_color(p.on_surface_variant.hsla())
                        .when(rtl, |el| el.text_end())
                        .when(!rtl, |el| el.text_start())
                        .child(SharedString::new_static(sample)),
                )
                .into_any_element()
        });
        // Straight under the page title: the families' own fold follows.
        let specimen = fit.render(cards);
        let rows = self.rows(registry::in_section(Section::Typography), cfg, p, cx);
        div().row().flex_col().gap(px(GROUP_SECTION_GAP_PX)).child(specimen).child(rows).into_any_element()
    }
}

/// The specimen line follows the text scale within its setting's range.
const SCALE_MIN: f32 = 0.75;
const SCALE_MAX: f32 = 1.5;
