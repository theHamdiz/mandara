//! The current reminder, its displayed meaning and citations, in the same crown.

use gpui::{AnyElement, Window};
use mandara_i18n::t;
use mandara_services::jannah::{catalog, meanings};
use mandara_theme::MandaraPalette;

use super::panel::{
    ARCH_ALPHA, ARCH_INNER_SHARE, ARCH_INSET, HERO_ANGLE_DEG, HERO_DARK, HERO_LIGHT,
    HERO_PRIMARY_SHARE, LATTICE_ALPHA, MEANING_LINE_EM, tone,
};
use crate::backdrop::paint::FontNames;
use crate::plugins::design_share::{self, Block, Document, Family, Ornament};
use crate::plugins::panel_fx::alpha;

pub(super) fn button(
    view: &design_share::View,
    flash: crate::plugins::extras::Flash,
    palette: &MandaraPalette,
    key: &str,
    meaning: Option<meanings::Meaning>,
    full: bool,
    arabic_px: f32,
    radius: f32,
) -> AnyElement {
    let (key, design_palette) = (key.to_string(), palette.clone());
    design_share::button(
        "jannah-image",
        view,
        flash,
        palette,
        move |cx, window: &Window| {
            let fonts = FontNames::from(&crate::fonts::fonts(cx));
            document(
                &key,
                &meaning,
                full,
                arabic_px,
                radius,
                &design_palette,
                fonts,
                window.scale_factor(),
            )
        },
    )
}

fn document(
    key: &str,
    meaning: &Option<meanings::Meaning>,
    full: bool,
    arabic_px: f32,
    radius: f32,
    palette: &MandaraPalette,
    fonts: FontNames,
    scale: f32,
) -> Result<Document, String> {
    let catalog = catalog::catalog();
    let item = catalog
        .find(key)
        .and_then(|id| catalog.get(id))
        .ok_or_else(|| t("Nothing to show").to_string())?;
    let width = super::panel::SHARE_WIDTH;
    let crown_w = width - ARCH_INSET * 2.0;
    let crown_scale = crown_w / super::art::CROWN_VIEW.0;
    let crown_height = super::art::CROWN_VIEW.1 * crown_scale;
    let side = ARCH_INSET
        + (super::art::CROWN_FOOT + super::art::CROWN_INNER) * crown_scale
        + super::panel::SHARE_SIDE;
    let accent = super::gold(palette);
    let tones = if palette.is_dark {
        HERO_DARK
    } else {
        HERO_LIGHT
    };
    let emerald = palette
        .success_emerald
        .mix(palette.primary, HERO_PRIMARY_SHARE);
    let mut doc = Document {
        width,
        scale,
        radius,
        padding: [
            ARCH_INSET + crown_height * super::panel::SHARE_TOP,
            side,
            ARCH_INSET + super::panel::SHARE_BOTTOM,
            side,
        ],
        background: (
            tone(emerald, tones[0]),
            tone(palette.tertiary.mix(emerald, HERO_PRIMARY_SHARE), tones[1]),
        ),
        angle: HERO_ANGLE_DEG,
        accent,
        lattice_alpha: alpha(LATTICE_ALPHA, palette),
        ornament: Ornament::Crown {
            inset: ARCH_INSET,
            alpha: alpha(ARCH_ALPHA, palette),
            inner_alpha: ARCH_INNER_SHARE,
        },
        palette: palette.clone(),
        fonts,
        blocks: Vec::with_capacity(6),
    };
    let block = |text, family, size, color, gap| Block {
        text,
        family,
        size,
        line_height: size
            * if matches!(family, Family::Scripture) {
                super::ARABIC_LINE_EM
            } else {
                MEANING_LINE_EM
            },
        weight: 400,
        tabular: false,
        align: Some(cosmic_text::Align::Center),
        color,
        gap,
    };
    doc.blocks.push(block(
        super::panel::words(item),
        Family::Scripture,
        arabic_px,
        palette.on_surface,
        super::panel::SHARE_GAP,
    ));
    if let Some(meaning) = &meaning {
        doc.blocks.push(block(
            meaning.text.clone(),
            Family::Ui,
            crate::kit::text::BODY,
            palette.on_surface,
            crate::kit::tokens::XS,
        ));
        let credit = if meaning.ai {
            Some(t("AI rendering of the meaning").to_string())
        } else {
            (!meaning.credit.is_empty()).then(|| format!("— {}", meaning.credit))
        };
        if let Some(credit) = credit {
            doc.blocks.push(block(
                credit,
                Family::Ui,
                crate::kit::text::LABEL,
                palette.on_surface_variant,
                super::panel::SHARE_GAP,
            ));
        }
    }
    if full && !item.is_verse() && item.full_arabic() != item.arabic {
        doc.blocks.push(block(
            t("With its chain of narrators").into(),
            Family::Ui,
            crate::kit::text::LABEL,
            palette.on_surface_variant,
            crate::kit::tokens::XS,
        ));
        doc.blocks.push(block(
            item.full_arabic().into(),
            Family::Scripture,
            super::panel::SHARE_CHAIN_PX,
            palette.on_surface_variant,
            super::panel::SHARE_GAP,
        ));
    }
    let mut reference = block(
        catalog::citation(item, super::arabic_ui()),
        Family::Ui,
        crate::kit::text::LABEL,
        accent,
        0.0,
    );
    reference.tabular = true;
    doc.blocks.push(reference);
    Ok(doc)
}

pub(super) fn command(
    reference: &str,
    cx: &mut gpui::App,
) -> Result<mandara_plugins::Output, String> {
    let item = super::find(reference)?;
    let settings = super::settings(cx);
    let shared = super::model(cx);
    let meaning = settings
        .translation
        .then(|| super::meaning_of(&shared.borrow(), item))
        .flatten();
    let palette = mandara_core::theme_entity(cx)
        .read(cx)
        .sample(std::time::Instant::now())
        .0;
    let fonts = FontNames::from(&crate::fonts::fonts(cx));
    let doc = document(
        &item.key,
        &meaning,
        true,
        settings.arabic_size.hero_px(),
        crate::style::card_radius(),
        &palette,
        fonts,
        1.0,
    )?;
    design_share::command(doc, cx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reminders_keep_the_full_displayed_meaning_credit_and_citation() {
        let _language = mandara_i18n::scoped(Some(mandara_i18n::Lang::En));
        let text = "A complete paragraph.\n\n".repeat(90) + "The final words.";
        let meaning = meanings::Meaning {
            text: text.clone(),
            credit: "The displayed translator".into(),
            ai: false,
        };
        let palette = mandara_theme::engine::default_palette(true);
        let fonts = FontNames::new("Inter", "Inter", "Alexandria", "JetBrains Mono");
        let doc = document(
            "89:30",
            &Some(meaning),
            false,
            25.0,
            16.0,
            &palette,
            fonts.clone(),
            1.0,
        )
        .unwrap();
        let catalog = catalog::catalog();
        let item = catalog.get(catalog.find("89:30").unwrap()).unwrap();
        assert_eq!(doc.blocks[0].text, super::super::panel::words(item));
        assert!(doc.blocks.iter().any(|block| block.text == text));
        assert!(
            doc.blocks
                .iter()
                .any(|block| block.text == "— The displayed translator")
        );
        assert_eq!(
            doc.blocks.last().unwrap().text,
            catalog::citation(item, false)
        );
        assert!(doc.blocks.last().unwrap().tabular);
        assert!(
            doc.blocks[..doc.blocks.len() - 1]
                .iter()
                .all(|block| !block.tabular)
        );
        let no_meaning = document(
            "89:30",
            &None,
            false,
            25.0,
            16.0,
            &palette,
            fonts.clone(),
            1.0,
        )
        .unwrap();
        assert_eq!(no_meaning.blocks.len(), 2);
        assert!(
            document(
                "missing-entry",
                &None,
                false,
                25.0,
                16.0,
                &palette,
                fonts,
                1.0
            )
            .is_err()
        );
    }

    #[test]
    fn an_expanded_hadith_keeps_its_complete_chain() {
        let catalog = catalog::catalog();
        let item = catalog
            .hadith_ids()
            .filter_map(|id| catalog.get(id))
            .find(|item| item.full_arabic() != item.arabic)
            .expect("the catalog contains a narrated chain");
        let palette = mandara_theme::engine::default_palette(false);
        let fonts = FontNames::new("Inter", "Inter", "Alexandria", "JetBrains Mono");
        let full = document(
            &item.key,
            &None,
            true,
            25.0,
            16.0,
            &palette,
            fonts.clone(),
            1.0,
        )
        .unwrap();
        assert!(
            full.blocks
                .iter()
                .any(|block| block.text == item.full_arabic())
        );
        let collapsed =
            document(&item.key, &None, false, 25.0, 16.0, &palette, fonts, 1.0).unwrap();
        assert!(
            !collapsed
                .blocks
                .iter()
                .any(|block| block.text == item.full_arabic())
        );
    }
}
