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

/// The panel's "Copy as image" button for reminder `key`. Its meaning is
/// taken from the panel when shown, otherwise prepared when pressed.
pub(super) fn button(
    view: &design_share::View,
    flash: crate::plugins::extras::Flash,
    palette: &MandaraPalette,
    notified: gpui::EntityId,
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
        notified,
        move |cx, window: &Window| {
            let fonts = FontNames::from(&crate::fonts::fonts(cx));
            let loaded;
            let meaning = if meaning.is_some() {
                &meaning
            } else {
                let item = super::find(&key)?;
                let wanted = super::wants_meaning(&super::settings(cx));
                loaded = prepare_meaning(item, wanted, cx)?;
                &loaded
            };
            document(
                &key,
                meaning,
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

/// The reminder's reading card as an export document, at the display's
/// scale.
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

/// A started fetch must not turn an enabled meaning into a silent omission.
fn export_meaning(
    item: &catalog::Item,
    wanted: bool,
    model: &super::Model,
) -> Result<Option<meanings::Meaning>, String> {
    if !wanted || super::arabic_ui() {
        return Ok(None);
    }
    if let Some(meaning) =
        super::meaning_of(model, item).filter(|meaning| !meaning.text.trim().is_empty())
    {
        return Ok(Some(meaning));
    }
    Err(match &model.fetch {
        super::Fetch::Idle | super::Fetch::Loading
            if item.is_verse() && meanings::edition(mandara_i18n::language().code()).is_some() =>
        {
            t("Loading the meaning…").into()
        }
        super::Fetch::Offline => mandara_services::offline::blocked(),
        super::Fetch::Failed(error) => error.clone(),
        _ => t("The meaning couldn't be loaded").into(),
    })
}

/// The meaning to include, when wanted and the interface isn't Arabic.
/// For a verse it starts loading the meanings; one not ready yet
/// (loading, offline, failed) is an error, never silently left out.
fn prepare_meaning(
    item: &catalog::Item,
    wanted: bool,
    cx: &mut gpui::App,
) -> Result<Option<meanings::Meaning>, String> {
    if !wanted || super::arabic_ui() {
        return Ok(None);
    }
    let shared = super::model(cx);
    if item.is_verse() {
        super::ensure_meanings(&shared, cx);
    }
    export_meaning(item, wanted, &shared.borrow())
}

/// `jannah copy-image <reference>`: copies the reminder's full card, as
/// the button does.
pub(super) fn command(
    reference: &str,
    cx: &mut gpui::App,
) -> Result<mandara_plugins::Output, String> {
    let item = super::find(reference)?;
    let settings = super::settings(cx);
    let meaning = prepare_meaning(item, super::wants_meaning(&settings), cx)?;
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

/// Uses the actual adapter and longest bundled narration in renderer regressions.
#[cfg(test)]
pub(in crate::plugins) fn longest_export_document(dark: bool, scale: f32) -> Document {
    let catalog = catalog::catalog();
    let item = catalog
        .hadith_ids()
        .filter_map(|id| catalog.get(id))
        .max_by_key(|item| item.full_arabic().len())
        .unwrap();
    document(
        &item.key,
        &None,
        true,
        super::ArabicSize::Medium.hero_px(),
        mandara_config::surfaces::defaults::CARD_RADIUS_PX,
        &mandara_theme::engine::default_palette(dark),
        FontNames::new("Inter", "Inter", "Alexandria", "JetBrains Mono"),
        scale,
    )
    .unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(fetch: super::super::Fetch) -> super::super::Model {
        super::super::Model { edition: Some("en.sahih"), fetch, ..super::super::Model::new(Default::default()) }
    }

    #[test]
    fn a_started_fetch_is_not_a_loaded_export_meaning() {
        let _language = mandara_i18n::scoped(Some(mandara_i18n::Lang::En));
        let catalog = catalog::catalog();
        let item = catalog.get(catalog.find("89:30").unwrap()).unwrap();
        for fetch in [super::super::Fetch::Idle, super::super::Fetch::Loading] {
            assert_eq!(
                export_meaning(item, true, &model(fetch)).unwrap_err(),
                t("Loading the meaning…")
            );
        }
        let meanings = [((89, 30), "The available meaning.".into())]
            .into_iter()
            .collect();
        let ready = model(super::super::Fetch::Ready(std::sync::Arc::new(meanings)));
        assert_eq!(
            export_meaning(item, true, &ready).unwrap().unwrap().text,
            "The available meaning."
        );
        assert!(export_meaning(item, false, &ready).unwrap().is_none());
        let _arabic = mandara_i18n::scoped(Some(mandara_i18n::Lang::Ar));
        assert!(export_meaning(item, true, &ready).unwrap().is_none());
    }

    #[test]
    fn unavailable_meanings_report_why_without_exporting_arabic_silently() {
        let _language = mandara_i18n::scoped(Some(mandara_i18n::Lang::En));
        let catalog = catalog::catalog();
        let item = catalog.get(catalog.find("89:30").unwrap()).unwrap();
        assert_eq!(
            export_meaning(item, true, &model(super::super::Fetch::Offline)).unwrap_err(),
            mandara_services::offline::blocked()
        );
        assert_eq!(
            export_meaning(
                item,
                true,
                &model(super::super::Fetch::Failed("provider failed".into()))
            )
            .unwrap_err(),
            "provider failed"
        );
        let empty = model(super::super::Fetch::Ready(Default::default()));
        assert_eq!(
            export_meaning(item, true, &empty).unwrap_err(),
            t("The meaning couldn't be loaded")
        );
        let hadith = catalog.get(catalog.find("bukhari-6094").unwrap()).unwrap();
        assert!(
            export_meaning(hadith, true, &model(super::super::Fetch::Loading))
                .unwrap()
                .is_some()
        );
        let _unsupported = mandara_i18n::scoped(Some(mandara_i18n::Lang::Te));
        assert_eq!(
            export_meaning(item, true, &model(super::super::Fetch::Idle)).unwrap_err(),
            t("The meaning couldn't be loaded")
        );
    }

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
