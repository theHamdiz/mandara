//! Export the selected ayah from the same immutable book the panel draws.

use super::panel::Ctx;
use crate::backdrop::paint::FontNames;
use crate::plugins::design_share::{self, Block, Document, Family, Ornament};
use crate::plugins::jannah::{
    gold,
    panel::{
        ARCH_ALPHA, ARCH_INNER_SHARE, ARCH_INSET, HERO_ANGLE_DEG, LATTICE_ALPHA, MEANING_LINE_EM,
    },
};
use crate::plugins::panel_fx::alpha;
use gpui::{AnyElement, App, Window};
use mandara_i18n::t;

pub(super) enum Style {
    Today(String),
    Reader { meaning: bool },
}

pub(super) fn button(
    id: &'static str,
    c: &Ctx<'_>,
    global: u16,
    width: f32,
    style: Style,
) -> AnyElement {
    let (book, palette, scripture, size, radius) = (
        std::sync::Arc::downgrade(&c.book),
        c.palette.clone(),
        c.scripture.clone(),
        c.arabic_px,
        c.radius,
    );
    let (view, flash) = {
        let ui = c.ui.borrow();
        (ui.share.clone(), ui.flash.clone())
    };
    design_share::button(
        id,
        &view,
        flash,
        c.palette,
        c.notified,
        move |cx: &mut App, window: &Window| {
            let mut fonts = FontNames::from(&crate::fonts::fonts(cx));
            fonts.scripture = scripture.to_string();
            let book = book
                .upgrade()
                .ok_or_else(|| t("This entry is no longer available").to_string())?;
            document(
                &book,
                global,
                width,
                &style,
                &palette,
                size,
                radius,
                fonts,
                window.scale_factor(),
            )
        },
    )
}

fn document(
    book: &super::Book,
    global: u16,
    width: f32,
    style: &Style,
    palette: &mandara_theme::MandaraPalette,
    size: f32,
    radius: f32,
    fonts: FontNames,
    scale: f32,
) -> Result<Document, String> {
    let (Some(reference), Some(ayah)) = (book.q.at(global), book.q.ayah_at(global)) else {
        return Err(t("Nothing to show").into());
    };
    let accent = gold(palette);
    let today = matches!(&style, Style::Today(_));
    let meaning_shown = match style {
        Style::Today(_) => !super::arabic_ui(),
        Style::Reader { meaning } => *meaning,
    };
    let crown_w = width - ARCH_INSET * 2.0;
    let crown_scale = crown_w / crate::plugins::jannah::art::CROWN_VIEW.0;
    let crown_height = crate::plugins::jannah::art::CROWN_VIEW.1 * crown_scale;
    // Same text inset as the visible crown. The reader retains its plain page layout.
    let side = if today {
        ARCH_INSET
            + (crate::plugins::jannah::art::CROWN_FOOT + crate::plugins::jannah::art::CROWN_INNER)
                * crown_scale
            + super::panel::SHARE_SIDE
    } else {
        super::reader::SHARE_PAD
    };
    let mut doc = Document {
        width,
        scale,
        radius,
        padding: [
            if today {
                ARCH_INSET + crown_height * super::panel::SHARE_TOP
            } else {
                side
            },
            side,
            if today {
                ARCH_INSET + super::panel::SHARE_BOTTOM
            } else {
                side
            },
            side,
        ],
        background: if today {
            super::panel::share_ground(palette)
        } else {
            (palette.surface_container, palette.surface_container)
        },
        angle: HERO_ANGLE_DEG,
        accent,
        lattice_alpha: if today {
            alpha(LATTICE_ALPHA, palette)
        } else {
            0.0
        },
        ornament: if today {
            Ornament::Crown {
                inset: ARCH_INSET,
                alpha: alpha(ARCH_ALPHA, palette),
                inner_alpha: ARCH_INNER_SHARE,
            }
        } else {
            Ornament::Plain
        },
        palette: palette.clone(),
        fonts,
        blocks: Vec::with_capacity(4),
    };
    let block = |text: String, family, size, line_height, color, weight, align, gap| Block {
        text,
        family,
        size,
        line_height,
        color,
        weight,
        tabular: matches!(family, Family::Ui) && weight >= 600,
        align,
        gap,
    };
    if let Style::Today(label) = &style {
        doc.blocks.push(block(
            label.clone(),
            Family::Ui,
            crate::kit::text::LABEL,
            crate::kit::text::LABEL * MEANING_LINE_EM,
            accent,
            700,
            Some(cosmic_text::Align::Center),
            super::panel::SHARE_GAP,
        ));
    }
    doc.blocks.push(block(
        format!("{}\u{a0}{}", ayah.text, super::ayah_mark(reference.ayah)),
        Family::Scripture,
        if today {
            super::panel::SHARE_ARABIC_PX
        } else {
            size
        },
        if today {
            super::panel::SHARE_ARABIC_PX * super::ARABIC_LINE_EM
        } else {
            size * super::ARABIC_LINE_EM
        },
        palette.on_surface,
        400,
        today.then_some(cosmic_text::Align::Center),
        super::panel::SHARE_GAP,
    ));
    if let Some(meaning) = book.meaning_of(global).filter(|_| meaning_shown) {
        doc.blocks.push(block(
            meaning.into(),
            Family::Ui,
            crate::kit::text::BODY,
            crate::kit::text::BODY * MEANING_LINE_EM,
            palette.on_surface_variant,
            400,
            today.then_some(cosmic_text::Align::Center),
            super::panel::SHARE_GAP,
        ));
    }
    doc.blocks.push(block(
        book.reference(global),
        Family::Ui,
        crate::kit::text::LABEL,
        crate::kit::text::LABEL * MEANING_LINE_EM,
        accent,
        600,
        Some(cosmic_text::Align::Center),
        0.0,
    ));
    Ok(doc)
}

pub(super) fn command(reference: &str, cx: &mut App) -> Result<mandara_plugins::Output, String> {
    let (shared, book) = super::need_book(cx)?;
    let selection = super::select(&book, reference)?;
    let mandara_services::quran::Selection::Ayah(reference) = selection else {
        return Err(t("Choose one ayah to copy as an image").into());
    };
    let global = book
        .q
        .global(reference)
        .ok_or_else(|| t("Nothing to show").to_string())?;
    let settings = super::settings(cx);
    let style = Style::Reader {
        meaning: shared.borrow().state.study && !super::arabic_ui(),
    };
    let palette = mandara_core::theme_entity(cx)
        .read(cx)
        .sample(std::time::Instant::now())
        .0;
    let fonts = FontNames::from(&crate::fonts::fonts(cx));
    let doc = document(
        &book,
        global,
        super::panel::INNER_W,
        &style,
        &palette,
        settings.arabic_size.reader_px(),
        crate::style::card_radius(),
        fonts,
        1.0,
    )?;
    design_share::command(doc, cx)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_ayah_keeps_every_word_and_only_the_displayed_meaning() {
        let _language = mandara_i18n::scoped(Some(mandara_i18n::Lang::En));
        let words = "بدايةٌ ".repeat(300) + "الخاتمةُ";
        let meaning = "First paragraph.\n\n".repeat(100) + "The complete final paragraph.";
        let quran = serde_json::from_value(serde_json::json!({"format":1,"edition":"quran-uthmani","bismillah":"","surahs":[{"number":1,"name":"الفاتحة","transliteration":"Al-Fatihah","meaning":"The Opening","revelation":"makki","ayahs":1,"first":1}],"ayahs":[{"text":words,"juz":1,"page":1,"hizb_quarter":1,"sajda":false}],"juz_starts":[{"surah":1,"ayah":1}]})).unwrap();
        let translation = mandara_services::quran::Translation {
            edition: "en.sahih".into(),
            name: "test translator".into(),
            language: "en".into(),
            rtl: false,
            texts: vec![meaning.clone()],
        };
        let book = super::super::Book::new(quran, Some(translation));
        let palette = mandara_theme::engine::default_palette(false);
        let fonts = FontNames::new("Inter", "Inter", "Alexandria", "JetBrains Mono");
        let shown = document(
            &book,
            1,
            440.0,
            &Style::Reader { meaning: true },
            &palette,
            24.0,
            16.0,
            fonts.clone(),
            1.0,
        )
        .unwrap();
        assert_eq!(
            shown.blocks[0].text,
            format!("{words}\u{a0}{}", super::super::ayah_mark(1))
        );
        assert!(shown.blocks.iter().any(|block| block.text == meaning));
        assert_eq!(shown.blocks.last().unwrap().text, book.reference(1));
        assert!(shown.blocks.last().unwrap().tabular);
        assert!(
            shown.blocks[..shown.blocks.len() - 1]
                .iter()
                .all(|block| !block.tabular)
        );
        let hidden = document(
            &book,
            1,
            440.0,
            &Style::Reader { meaning: false },
            &palette,
            24.0,
            16.0,
            fonts,
            1.0,
        )
        .unwrap();
        assert_eq!(hidden.blocks.len(), 2);
        assert!(!hidden.blocks.iter().any(|block| block.text == meaning));
        assert_eq!(
            book.q.ayah_at(1).unwrap().text,
            words,
            "export does not edit the book"
        );
    }
}
