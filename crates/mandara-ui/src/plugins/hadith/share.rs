//! Resolve the exact displayed hadith without copying its complete text every render.

use gpui::{AnyElement, Window};
use mandara_i18n::t;
use mandara_services::hadith::{Grade, GradeClass, Library};
use mandara_theme::MandaraPalette;
use std::{cell::RefCell, rc::Weak, sync::Arc};

use crate::backdrop::paint::FontNames;
use crate::plugins::design_share::{self, Block, Document, Family, Ornament};
use crate::plugins::jannah::{
    gold,
    panel::{HERO_ANGLE_DEG, LATTICE_ALPHA, MEANING_LINE_EM},
};
use crate::plugins::panel_fx::alpha;

#[derive(Clone)]
pub(super) enum Source {
    Daily {
        model: Weak<RefCell<super::Model>>,
        key: String,
    },
    Library {
        library: std::sync::Weak<Library>,
        index: u32,
    },
}

struct Entry<'a> {
    reference: &'a str,
    words: &'a str,
    chain: Option<(&'a str, &'a str)>,
    meaning: Option<&'a str>,
    narrator: Option<&'a str>,
    english: bool,
    translator: Option<&'a str>,
    class: Option<GradeClass>,
    grades: &'a [Grade],
}

impl Source {
    fn resolve<R>(&self, build: impl FnOnce(Entry<'_>) -> R) -> Option<R> {
        match self {
            Self::Daily { model, key } => {
                let model = model.upgrade()?;
                let model = model.borrow();
                let card = model
                    .state
                    .today
                    .as_ref()
                    .map(|today| &today.card)
                    .filter(|card| card.key == *key)
                    .or_else(|| {
                        model
                            .peeked
                            .as_ref()
                            .and_then(|(_, card)| card.as_ref())
                            .filter(|card| card.key == *key)
                    })?;
                let grades: Vec<_> = card
                    .grade
                    .iter()
                    .filter(|grade| !grade.grade.is_empty())
                    .map(|grade| Grade::new(&grade.grader, &grade.grade))
                    .collect();
                Some(build(Entry {
                    reference: &card.reference,
                    words: &card.words,
                    chain: None,
                    meaning: card.meaning.as_deref(),
                    narrator: card.narrator.as_deref(),
                    english: card.english,
                    translator: None,
                    class: card.grade.as_ref().map(|grade| grade.class),
                    grades: &grades,
                }))
            }
            Self::Library { library, index } => {
                let library = library.upgrade()?;
                let hadith = library.get(*index)?;
                let meaning = library.meaning.as_ref();
                Some(build(Entry {
                    reference: &hadith.reference,
                    words: hadith.words(),
                    chain: Some(hadith.chain()),
                    meaning: hadith.meaning_body(),
                    narrator: hadith.narrator.as_deref(),
                    english: meaning.is_some_and(|meaning| meaning.fallback),
                    translator: meaning.and_then(|meaning| meaning.translator.as_deref()),
                    class: hadith.class(library.collection),
                    grades: &hadith.grades,
                }))
            }
        }
    }
}

pub(super) fn button(
    c: &super::panel::Ctx<'_>,
    shown: &super::reader::Shown<'_>,
    hero: bool,
) -> AnyElement {
    let (source, label, collection, chain) = (
        shown.source.clone(),
        shown.label.clone(),
        shown.collection,
        shown.chain.is_some(),
    );
    let (palette, family, width, size, radius, show_meaning) = (
        c.palette.clone(),
        c.hadith_font.clone(),
        c.width,
        c.arabic_px,
        c.radius,
        c.show_meaning,
    );
    let (view, flash) = {
        let ui = c.ui.borrow();
        (ui.share.clone(), ui.flash.clone())
    };
    design_share::button(
        "hadith-image",
        &view,
        flash,
        c.palette,
        c.notified,
        move |cx, window: &Window| {
            let mut fonts = FontNames::from(&crate::fonts::fonts(cx));
            fonts.hadith = family.to_string();
            source
                .resolve(|entry| {
                    document(
                        entry,
                        Design {
                            palette: palette.clone(),
                            fonts,
                            width,
                            size,
                            radius,
                            show_meaning,
                            chain,
                            hero,
                            label: label.clone(),
                            collection,
                            scale: window.scale_factor(),
                        },
                    )
                })
                .ok_or_else(|| t("This entry is no longer available").to_string())
        },
    )
}

struct Design {
    palette: MandaraPalette,
    fonts: FontNames,
    width: f32,
    size: f32,
    radius: f32,
    show_meaning: bool,
    chain: bool,
    hero: bool,
    label: String,
    collection: mandara_services::hadith::Collection,
    scale: f32,
}

fn document(entry: Entry<'_>, design: Design) -> Document {
    let Design {
        palette,
        fonts,
        width,
        size,
        radius,
        show_meaning,
        chain,
        hero,
        label,
        collection,
        scale,
    } = design;
    let accent = gold(&palette);
    let mut doc = Document {
        width,
        scale,
        radius,
        padding: [
            super::reader::SHARE_PAD_TOP,
            super::reader::SHARE_PAD_X,
            super::reader::SHARE_PAD_BOTTOM,
            super::reader::SHARE_PAD_X,
        ],
        background: super::reader::share_ground(&palette),
        angle: HERO_ANGLE_DEG,
        accent,
        lattice_alpha: alpha(LATTICE_ALPHA, &palette),
        ornament: Ornament::Manuscript {
            width_share: super::reader::SHARE_HEADPIECE_SHARE,
            dawn_top: super::reader::SHARE_DAWN_TOP,
            top: super::reader::SHARE_PAD_TOP
                + super::reader::SHARE_LABEL_LINE
                + super::reader::SHARE_GAP,
        },
        palette: palette.clone(),
        fonts,
        blocks: Vec::with_capacity(entry.grades.len().saturating_add(8)),
    };
    let block = |text, family, size, color, align, gap| Block {
        text,
        family,
        size,
        line_height: size
            * if matches!(family, Family::Hadith) {
                super::ARABIC_LINE_EM
            } else {
                MEANING_LINE_EM
            },
        weight: 400,
        tabular: false,
        align,
        color,
        gap,
    };
    let grade = entry.class.map(|class| t(super::grade_label(class)));
    let heading = grade.map_or_else(|| label.clone(), |grade| format!("{label} · {grade}"));
    let mut head = block(
        heading,
        Family::Ui,
        super::reader::SHARE_LABEL_PX,
        accent,
        None,
        super::reader::SHARE_GAP * 2.0
            + width * super::reader::SHARE_HEADPIECE_SHARE * super::art::HEADPIECE_VIEW.1
                / super::art::HEADPIECE_VIEW.0,
    );
    head.line_height = super::reader::SHARE_LABEL_LINE;
    head.weight = 700;
    head.tabular = true;
    doc.blocks.push(head);
    let chain = entry.chain.filter(|_| chain);
    if let Some((before, _)) = chain.filter(|(before, _)| !before.is_empty()) {
        doc.blocks.push(block(
            before.into(),
            Family::Hadith,
            super::reader::SHARE_CHAIN_PX,
            palette.on_surface_variant,
            Some(cosmic_text::Align::Right),
            super::reader::SHARE_GAP,
        ));
    }
    doc.blocks.push(block(
        entry.words.into(),
        Family::Hadith,
        size,
        palette.on_surface,
        Some(cosmic_text::Align::Center),
        super::reader::SHARE_GAP,
    ));
    if let Some((_, after)) = chain.filter(|(_, after)| !after.is_empty()) {
        doc.blocks.push(block(
            after.into(),
            Family::Hadith,
            super::reader::SHARE_CHAIN_PX,
            palette.on_surface_variant,
            Some(cosmic_text::Align::Right),
            super::reader::SHARE_GAP,
        ));
    }
    if show_meaning {
        if let Some(narrator) = entry.narrator {
            doc.blocks.push(block(
                mandara_i18n::tf("Narrated by {narrator}", &[("narrator", &narrator)]),
                Family::Ui,
                crate::kit::text::LABEL,
                palette.on_surface_variant,
                Some(cosmic_text::Align::Center),
                super::reader::SHARE_GAP,
            ));
        }
        if let Some(meaning) = entry.meaning.filter(|meaning| !meaning.trim().is_empty()) {
            let align = (meaning.chars().count() <= super::reader::SHARE_CENTRED_MAX)
                .then_some(cosmic_text::Align::Center);
            doc.blocks.push(block(
                meaning.into(),
                Family::Ui,
                crate::kit::text::BODY,
                palette.on_surface,
                align,
                super::reader::SHARE_GAP,
            ));
            if !hero {
                let credit = if entry.english {
                    Some(t("Meaning in English").to_string())
                } else {
                    entry.translator.map(|translator| format!("— {translator}"))
                };
                if let Some(credit) = credit {
                    doc.blocks.push(block(
                        credit,
                        Family::Ui,
                        crate::kit::text::LABEL,
                        palette.on_surface_variant,
                        Some(cosmic_text::Align::Center),
                        super::reader::SHARE_GAP,
                    ));
                }
            }
        }
    }
    if !hero && entry.grades.len() > 1 {
        for grade in entry.grades {
            doc.blocks.push(block(
                format!("{} · {}", grade.grade, grade.grader),
                Family::Ui,
                crate::kit::text::LABEL,
                super::grade_color(grade.class, &palette),
                Some(cosmic_text::Align::Center),
                crate::kit::tokens::XS,
            ));
        }
    }
    let mut reference = block(
        super::citation(collection, entry.reference),
        Family::Ui,
        crate::kit::text::LABEL,
        accent,
        Some(cosmic_text::Align::Center),
        0.0,
    );
    reference.tabular = true;
    doc.blocks.push(reference);
    doc
}

pub(super) fn command(
    reference: &str,
    cx: &mut gpui::App,
) -> Result<mandara_plugins::Output, String> {
    let (collection, reference) =
        mandara_services::hadith::refs::parse(reference).ok_or_else(|| {
            mandara_i18n::tf(
                "Can't read {reference}: try `bukhari 1` or `muslim 2699a`",
                &[("reference", &reference)],
            )
        })?;
    let settings = super::settings(cx);
    let language = super::ui_code(cx);
    let design = Design {
        palette: mandara_core::theme_entity(cx)
            .read(cx)
            .sample(std::time::Instant::now())
            .0,
        fonts: FontNames::from(&crate::fonts::fonts(cx)),
        width: super::panel::SHARE_WIDTH,
        size: settings.arabic_size.hero_px(),
        radius: crate::style::card_radius(),
        show_meaning: super::wants_meaning(&settings),
        chain: !settings.hide_chain,
        hero: false,
        label: String::new(),
        collection,
        scale: 1.0,
    };
    design_share::command_job(
        move || {
            let (loaded, index) = super::find(collection, &reference, language)?;
            let mut design = design;
            design.label = loaded
                .library
                .book_of(index)
                .map(|book| book.name.to_uppercase())
                .unwrap_or_default();
            Source::Library {
                library: Arc::downgrade(&loaded.library),
                index,
            }
            .resolve(|entry| document(entry, design))
            .ok_or_else(|| t("This entry is no longer available").to_string())
        },
        cx,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_services::hadith::{Collection, collections, data};

    fn library() -> Arc<Library> {
        let count = collections::info(Collection::Nawawi).entries;
        let edition = |translated: bool| {
            let hadiths: Vec<_> = (1..=count).map(|number| serde_json::json!({"hadithnumber":number,"text":if translated { "First paragraph.\n\n".repeat(50) + "The very last meaning words." } else { "كلماتٌ عربيةٌ ".repeat(100) + "آخرُ الكلماتِ" },"reference":{"book":1,"hadith":number}})).collect();
            data::parse(&serde_json::json!({"metadata":{"name":"test edition","sections":{"1":"test book"}},"hadiths":hadiths}).to_string()).unwrap()
        };
        Arc::new(
            Library::build(
                Collection::Nawawi,
                edition(false),
                Some((
                    edition(true),
                    data::MeaningInfo {
                        lang: "eng".into(),
                        translator: Some("The displayed translator".into()),
                        fallback: false,
                    },
                )),
            )
            .unwrap(),
        )
    }

    #[test]
    fn a_captured_library_entry_preserves_all_arabic_and_meaning_paragraphs() {
        let library = library();
        let source = Source::Library {
            library: Arc::downgrade(&library),
            index: 0,
        };
        let palette = mandara_theme::engine::default_palette(true);
        let make_design = || Design {
            palette: palette.clone(),
            fonts: FontNames::new("Inter", "Inter", "Alexandria", "JetBrains Mono"),
            width: 452.0,
            size: 25.0,
            radius: 16.0,
            show_meaning: true,
            chain: true,
            hero: false,
            label: "THE DISPLAYED ENTRY".into(),
            collection: Collection::Nawawi,
            scale: 1.0,
        };
        let doc = source
            .resolve(|entry| document(entry, make_design()))
            .unwrap();
        let hadith = library.get(0).unwrap();
        assert!(doc.blocks.iter().any(|block| block.text == hadith.words()));
        assert!(
            doc.blocks
                .iter()
                .any(|block| Some(block.text.as_str()) == hadith.meaning_body())
        );
        assert!(
            doc.blocks
                .iter()
                .any(|block| block.text == "— The displayed translator")
        );
        assert_eq!(
            doc.blocks.last().unwrap().text,
            super::super::citation(Collection::Nawawi, &hadith.reference)
        );
        assert!(doc.blocks.first().unwrap().tabular && doc.blocks.last().unwrap().tabular);
        assert!(
            doc.blocks[1..doc.blocks.len() - 1]
                .iter()
                .all(|block| !block.tabular)
        );
        assert!(
            Source::Library {
                library: Arc::downgrade(&library),
                index: u32::MAX
            }
            .resolve(|_| ())
            .is_none()
        );
        drop(library);
        assert!(
            source.resolve(|_| ()).is_none(),
            "the button does not keep an entire old collection alive"
        );
        assert!(
            Source::Daily {
                model: Weak::new(),
                key: "nawawi:1".into()
            }
            .resolve(|_| ())
            .is_none(),
            "a discarded daily source cannot select another entry"
        );
    }
}
