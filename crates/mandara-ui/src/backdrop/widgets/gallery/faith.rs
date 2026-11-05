//! Local examples for gallery pictures, never for live widget scenes.

use mandara_config::desktop_widgets::{JannahLayout, WidgetKind};
use mandara_i18n::t;
use mandara_services::hadith::collections;
use mandara_services::jannah::{PATHS, catalog};

use super::super::{HadithCard, HadithInfo, HadithProgress, JannahInfo, QuranInfo, WidgetScene};

/// Verbatim Al-Fajr 89:30 from the bundled Tanzil text. Kept small here so
/// a thumbnail does not initialise the complete content catalogue.
const VERSE: &str = "وَٱدْخُلِى جَنَّتِى";
const VERSE_REFERENCE: &str = "Al-Fajr 89:30 · tanzil.net";
const VERSE_REFERENCE_AR: &str = "الفجر ٨٩:٣٠ · tanzil.net";
/// Verbatim words of Sahih al-Bukhari 574, checked against bundled content.
const HADITH: &str = "مَنْ صَلَّى الْبَرْدَيْنِ دَخَلَ الْجَنَّةَ";
const HADITH_NUMBER: u32 = 574;
/// An illustrative reading position near the end of the khatm.
const KHATM_SHARE: f32 = 0.97;
/// Two completed days and the current, unchecked example day.
const WEEK: [bool; 7] = [true, true, false, false, false, false, false];
const TODAY: usize = 2;
/// The collection sample shows one third read; no account state is inspected.
const READING_PARTS: usize = 3;

/// Populate only the requested faith widget. These are static content and
/// metadata lookups; no plugin instance, downloader or reading store is opened.
pub(super) fn populate(scene: &mut WidgetScene, kind: &WidgetKind) {
    match kind {
        WidgetKind::Jannah(options) => {
            let step = &PATHS[0];
            let mut sample = JannahInfo {
                step_title: step.title.into(), step_line: step.line.into(), week: WEEK, today: TODAY,
                ..Default::default()
            };
            match options.layout {
                JannahLayout::Quote => {
                    sample.arabic = HADITH.into();
                    sample.reference = format!("{} {HADITH_NUMBER}", t("Sahih al-Bukhari"));
                    sample.reference_ar = format!("صحيح البخاري {}", catalog::arabic_digits(&HADITH_NUMBER.to_string()));
                }
                JannahLayout::Verse => {
                    sample.arabic = VERSE.into();
                    sample.ayah_mark = Some(catalog::ayah_mark(30));
                    sample.reference = VERSE_REFERENCE.into();
                    sample.reference_ar = VERSE_REFERENCE_AR.into();
                }
                JannahLayout::Step => {}
            }
            scene.data.jannah = Some(sample);
        }
        WidgetKind::Hadith(options) => {
            let collection = collections::info(options.collection);
            scene.data.hadith = Some(HadithInfo {
                today: Some(HadithCard {
                    words: HADITH.into(), reference: format!("{} {HADITH_NUMBER}", t("Sahih al-Bukhari")),
                    reference_ar: format!("صحيح البخاري {}", catalog::arabic_digits(&HADITH_NUMBER.to_string())),
                    ..Default::default()
                }),
                progress: vec![HadithProgress {
                    collection: options.collection, title: collection.title.into(), arabic: collection.arabic.into(),
                    read: collection.hadith / READING_PARTS, total: collection.hadith, downloaded: true, last: None,
                }],
                ..Default::default()
            });
        }
        WidgetKind::Quran(_) => {
            scene.data.quran = Some(QuranInfo {
                arabic: VERSE.into(), ayah_mark: catalog::ayah_mark(30),
                reference: VERSE_REFERENCE.into(), reference_ar: VERSE_REFERENCE_AR.into(),
                progress: KHATM_SHARE, surah: "Al-Fajr".into(), surah_ar: "الفجر".into(), ayah: 30, ayahs: 30, juz: 30,
                ..Default::default()
            });
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mandara_config::desktop_widgets::{DesktopWidget, HadithLayout, HadithWidget, JannahWidget, QuranLayout, QuranWidget};

    fn examples() -> [(&'static str, WidgetKind); 7] {
        [
            ("jannah-verse", WidgetKind::Jannah(JannahWidget { layout: JannahLayout::Verse, ..Default::default() })),
            ("jannah-quote", WidgetKind::Jannah(JannahWidget { layout: JannahLayout::Quote, ..Default::default() })),
            ("jannah-step", WidgetKind::Jannah(JannahWidget { layout: JannahLayout::Step, ..Default::default() })),
            ("hadith-day", WidgetKind::Hadith(HadithWidget { layout: HadithLayout::Hadith, ..Default::default() })),
            ("hadith-progress", WidgetKind::Hadith(HadithWidget { layout: HadithLayout::Collection, ..Default::default() })),
            ("quran-ayah", WidgetKind::Quran(QuranWidget { layout: QuranLayout::Ayah, ..Default::default() })),
            ("quran-progress", WidgetKind::Quran(QuranWidget { layout: QuranLayout::Progress, ..Default::default() })),
        ]
    }

    #[test]
    fn scripture_examples_match_the_bundled_sources_verbatim() {
        let catalog = catalog::catalog();
        assert_eq!(catalog.get(catalog.find("89:30").unwrap()).unwrap().arabic, VERSE);
        assert_eq!(catalog.get(catalog.find("bukhari-574").unwrap()).unwrap().arabic, HADITH);
    }

    #[test]
    fn all_seven_gallery_layouts_get_only_their_own_example_data() {
        let palette = mandara_theme::engine::default_palette(false);
        for (_, kind) in examples() {
            let widget = DesktopWidget { kind, ..Default::default() };
            let scene = super::super::sample_scene(&widget, &palette, 400);
            let count = usize::from(scene.data.jannah.is_some()) + usize::from(scene.data.hadith.is_some()) + usize::from(scene.data.quran.is_some());
            assert_eq!(count, 1);
        }
        let live = WidgetScene::default();
        assert!(live.data.jannah.is_none() && live.data.hadith.is_none() && live.data.quran.is_none());
    }

    #[test]
    fn seven_thumbnails_use_the_actual_painters_at_light_and_dark_density() {
        use std::hash::{Hash as _, Hasher as _};
        let fonts = crate::backdrop::paint::FontNames::new("Inter", "Alexandria", "Amiri", "JetBrains Mono");
        let mut painter = crate::lock::paint::TextPainter::new();
        for dark in [false, true] {
            let palette = mandara_theme::engine::default_palette(dark);
            let mut pictures = std::collections::HashMap::new();
            for (name, kind) in examples() {
                let widget = DesktopWidget { kind, ..Default::default() };
                let image = super::super::thumbnail(&widget, (400, 300), &fonts, &palette, &mut painter).expect(name);
                assert_eq!(image.dimensions(), (400, 300));
                assert!(image.pixels().filter(|pixel| pixel.0[3] > 0).count() > image.width() as usize, "{name}: empty thumbnail");
                let mut hash = std::collections::hash_map::DefaultHasher::new();
                image.as_raw().hash(&mut hash);
                pictures.insert(name, hash.finish());
                if let Some(path) = std::env::var_os("MANDARA_FAITH_PREVIEW_DUMP") {
                    let path = std::path::PathBuf::from(path);
                    std::fs::create_dir_all(&path).unwrap();
                    image.save(path.join(format!("{name}-{}.png", if dark { "dark" } else { "light" }))).unwrap();
                }
            }
            for (first, second) in [("jannah-verse", "jannah-quote"), ("jannah-quote", "jannah-step"), ("hadith-day", "hadith-progress"), ("quran-ayah", "quran-progress")] {
                assert_ne!(pictures[first], pictures[second], "{first} and {second} must show their own layouts");
            }
        }
    }
}
