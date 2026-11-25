//! Fitted gallery pictures, rendered at their destination resolution.
//! Sample data belongs to this preview path; live desktop scenes never use it.

use chrono::Local;
use mandara_config::desktop_widgets::{Anchor, DesktopWidget, WidgetKind};
use mandara_theme::MandaraPalette;
use tiny_skia::{Pixmap, PixmapPaint, Transform};

use super::{EventItem, MediaInfo, OutputCx, WidgetScene};
use crate::backdrop::paint::FontNames;
use crate::lock::paint::TextPainter;

mod faith;
pub mod info;

/// Keep the plate, antialiasing and shadows clear of the tile's rim.
const CONTENT_FRACTION: f32 = 0.88;
/// Tiny labels may need more zoom than a desktop placement permits; cap
/// malformed metrics without imposing the saved placement’s scale limit.
const MAX_FIT_SCALE: f32 = 64.0;
/// Cover art follows tile density, bounded above the widest 4× tile.
const COVER_MAX_PX: u32 = 1024;
/// The sample player is paused a minute into a three-minute track.
const TRACK_MS: i64 = 180_000;
const POSITION_MS: i64 = 60_000;
/// An illustrative slider position, confined to labelled gallery previews.
const SAMPLE_LEVEL: f32 = 0.65;

/// Use labelled preview data without reading the user’s accounts or media history.
fn sample_scene(widget: &DesktopWidget, palette: &MandaraPalette, cover_px: u32) -> WidgetScene {
    let now = Local::now();
    let mut scene = WidgetScene::preview(vec![widget.clone()]);
    scene.on_surface = palette.on_surface;
    scene.surface = palette.surface_container;
    scene.data.palette = super::super::palette_colors(palette);
    faith::populate(&mut scene, &widget.kind);
    info::populate(&mut scene, widget, Some(palette), cover_px);
    if let WidgetKind::Toggles(toggles) = &widget.kind {
        scene.data.toggles = super::toggle_ids(toggles).into_iter().enumerate().map(|(i, id)| (id, i % 2 == 0)).collect();
    }
    if let WidgetKind::Sliders(sliders) = &widget.kind {
        let levels = if sliders.levels.is_empty() { vec!["volume".into(), "brightness".into()] } else { sliders.levels.clone() };
        scene.data.levels = levels.into_iter().map(|id| (id, SAMPLE_LEVEL)).collect();
    }
    if matches!(widget.kind, WidgetKind::Media(_)) {
        let cover_px = cover_px.clamp(1, COVER_MAX_PX);
        let mut cover = Pixmap::new(cover_px, cover_px).unwrap();
        let [r, g, b, a] = palette.primary.to_rgba8();
        cover.fill(tiny_skia::Color::from_rgba8(r, g, b, a));
        if let Some(icon) = crate::backdrop::paint::icon_pixmap("music", cover_px, palette.on_primary) {
            cover.draw_pixmap(0, 0, icon.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
        }
        scene.data.media = Some(MediaInfo {
            title: mandara_i18n::t("Example track").into(), artist: mandara_i18n::t("Example artist").into(), playing: false,
            position_ms: POSITION_MS, at_ms: now.timestamp_millis(), rate: 1.0, length_ms: TRACK_MS, live: false,
            art: Some(super::layout::Picture { key: super::key((palette.primary.to_rgba8(), palette.on_primary.to_rgba8(), cover_px)), pixmap: std::sync::Arc::new(cover) }),
            lyrics: vec![(0, mandara_i18n::t("Example track").into()), (POSITION_MS, mandara_i18n::t("Preview").into())], lyrics_loading: false,
        });
    }
    if matches!(widget.kind, WidgetKind::Date(_)) {
        scene.data.events.push(EventItem { day: now.date_naive(), last_day: now.date_naive(), start_ms: now.timestamp_millis(), end_ms: now.timestamp_millis() + TRACK_MS, all_day: true, title: mandara_i18n::t("Preview").into(), color: None });
    }
    if matches!(widget.kind, WidgetKind::Sky(_)) {
        scene.data.place = mandara_services::world_clock::DEFAULT_CITIES.first().and_then(|name| mandara_services::world_clock::resolve(name)).and_then(|city| city.coords);
    }
    scene
}

/// Fit the visible vector bounds while reserving space for paint effects.
fn scale_for(size: (u32, u32), bounds: (f32, f32)) -> Option<f32> {
    if !bounds.0.is_finite() || !bounds.1.is_finite() || bounds.0 <= 0.0 || bounds.1 <= 0.0 { return None; }
    Some(((size.0 as f32 / bounds.0).min(size.1 as f32 / bounds.1) * CONTENT_FRACTION).clamp(MAX_FIT_SCALE.recip(), MAX_FIT_SCALE))
}

// Live painters reserve redraw space with transparent marks. A gallery
// picture fits the visible content, including short lyric samples.
fn visible_layout(mut laid: super::Laid) -> super::Laid {
    laid.comp.marks.retain(|mark| mark.opacity > 0.0);
    laid.comp.normalize();
    let bounds = super::draw::rotated_size((laid.comp.size.0 + 2.0 * laid.margin, laid.comp.size.1 + 2.0 * laid.margin), laid.rotation);
    laid.outer.2 = bounds.0;
    laid.outer.3 = bounds.1;
    laid
}

/// Measure the widget, fit its vector layout, then paint once. Enlarging a
/// tiny bitmap after painting would enlarge its pixels and glyph edges too.
pub fn thumbnail(widget: &DesktopWidget, size: (u32, u32), fonts: &FontNames, palette: &MandaraPalette, text: &mut TextPainter) -> Option<image::RgbaImage> {
    let mut widget = widget.clone();
    widget.place.anchor = Anchor::Center;
    widget.place.x = 0.0;
    widget.place.y = 0.0;
    widget.place.scale = 1.0;
    widget.place.displays.clear();
    info::prepare(&mut widget);
    let scene = sample_scene(&widget, palette, size.0.max(size.1));
    let [r, g, b, _] = palette.surface_dim.to_rgba8();
    let tone = image::RgbImage::from_pixel(1, 1, image::Rgb([r, g, b]));
    let names: [&str; 0] = [];
    let output = OutputCx { id: 0, names: &names, logical: (size.0 as f32, size.1 as f32), physical: size, scale120: mandara_core::shm_canvas::SCALE_DENOMINATOR, tone: Some(&tone), primary: palette.primary, fonts, depth: false, subject: None, safe: Default::default() };
    let now = Local::now();
    let fitted_layout = |laid| {
        if matches!(widget.kind, WidgetKind::Visualizer(_)) {
            laid
        } else {
            visible_layout(laid)
        }
    };
    let laid = fitted_layout(super::lay_out(&widget, &output, &scene, None, &now, text)?);
    let bounds = (laid.outer.2, laid.outer.3);
    let fit = scale_for(size, bounds)?;
    let mut canvas = Pixmap::new(size.0, size.1)?;

    // This virtual height changes vector/text scale only; it allocates no
        // full-display bitmap. The actual image remains exactly `size`.
        let fitted = OutputCx { physical: (size.0, (size.1 as f32 * fit).ceil().max(1.0) as u32), ..output };
        let mut laid = fitted_layout(super::lay_out(&widget, &fitted, &scene, None, &now, text)?);
        let (x, y) = ((size.0 as f32 - laid.outer.2) / 2.0, (size.1 as f32 - laid.outer.3) / 2.0);
        laid.origin = (x + (laid.outer.2 - laid.comp.size.0) / 2.0, y + (laid.outer.3 - laid.comp.size.1) / 2.0);
        laid.outer.0 = x;
        laid.outer.1 = y;
        super::compose_laid(&mut canvas.as_mut(), &widget, &output, &scene, &laid, text,
        &super::preview_bands(),
    );
    super::straight(canvas)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visualizer_gallery_pictures_include_the_shared_frame() {
        use mandara_config::desktop_widgets::{Chrome, Look, PlaneVisualizer, Plate};
        let mut text = TextPainter::new();
        let mut widget = DesktopWidget {
            kind: WidgetKind::Visualizer(PlaneVisualizer::default()),
            ..Default::default()
        };
        let palette = mandara_theme::engine::default_palette(true);
        let plain = thumbnail(
            &widget,
            (264, 198),
            &FontNames::default(),
            &palette,
            &mut text,
        )
        .unwrap();
        if let Some(look) = widget.kind.look_mut() {
            *look = Look {
                header: "Spectrum".into(),
                plate: Plate::Glass,
                chrome: Chrome::Window,
                ..Default::default()
            };
        }
        let framed = thumbnail(
            &widget,
            (264, 198),
            &FontNames::default(),
            &palette,
            &mut text,
        )
        .unwrap();
        assert_ne!(plain.as_raw(), framed.as_raw());
        assert!(framed.pixels().any(|pixel| pixel[3] > 0));
    }

    #[test]
    fn fitting_keeps_both_dimensions_in_view_and_rejects_empty_content() {
        assert_eq!(scale_for((528, 396), (264.0, 198.0)), Some(2.0 * CONTENT_FRACTION));
        assert_eq!(scale_for((528, 396), (1056.0, 100.0)), Some(0.5 * CONTENT_FRACTION));
        for bounds in [(0.0, 4.0), (4.0, 0.0), (f32::NAN, 4.0), (4.0, f32::INFINITY)] { assert_eq!(scale_for((528, 396), bounds), None); }
    }

    #[test]
    fn transparent_redraw_reservations_do_not_shrink_the_visible_preview() {
        use super::super::layout::{Composition, InkRole, Mark, MarkKind};
        let rect = |w, h| {
            Mark::new(MarkKind::Rect { x: 0.0, y: 0.0, w, h, radius: 0.0, width: 0.0 }, InkRole::Fg)
        };
        let comp = Composition::of(vec![rect(1000.0, 500.0).dim(0.0), rect(50.0, 30.0)]);
        let laid = super::super::Laid { comp, origin: (0.0, 0.0), k: 1.0, rotation: 0.0, flip: (false, false), outer: (0.0, 0.0, 1000.0, 500.0), margin: 0.0 };
        let visible = visible_layout(laid);
        assert_eq!(visible.comp.size, (50.0, 30.0));
        assert_eq!((visible.outer.2, visible.outer.3), (50.0, 30.0));
    }

    #[test]
    fn only_gallery_scenes_get_sample_media_and_events() {
        use mandara_config::desktop_widgets::{DateWidget, MediaWidget};
        let palette = mandara_theme::engine::default_palette(true);
        let media = DesktopWidget { kind: WidgetKind::Media(MediaWidget::default()), ..Default::default() };
        let scene = sample_scene(&media, &palette, 540);
        assert!(scene.data.media.as_ref().is_some_and(|m| !m.playing && m.art.as_ref().is_some_and(|art| art.pixmap.width() == 540)));
        assert!(WidgetScene::default().data.media.is_none());
        let date = DesktopWidget { kind: WidgetKind::Date(DateWidget::default()), ..Default::default() };
        assert_eq!(sample_scene(&date, &palette, 540).data.events.len(), 1);
        assert!(WidgetScene::default().data.events.is_empty());
    }

    #[test]
    fn gallery_controls_have_examples_without_populating_live_scenes() {
        use mandara_config::desktop_widgets::{SlidersWidget, TogglesWidget};
        let palette = mandara_theme::engine::default_palette(false);
        let toggles = DesktopWidget { kind: WidgetKind::Toggles(TogglesWidget::default()), ..Default::default() };
        let scene = sample_scene(&toggles, &palette, 360);
        assert_eq!(scene.data.toggles.len(), 4);
        assert!(scene.data.toggles.iter().any(|(_, on)| *on));
        assert!(scene.data.toggles.iter().any(|(_, on)| !on));
        assert_eq!(scene.data.palette, super::super::super::palette_colors(&palette));
        let sliders = DesktopWidget { kind: WidgetKind::Sliders(SlidersWidget { levels: vec!["volume".into()], ..Default::default() }), ..Default::default() };
        assert_eq!(sample_scene(&sliders, &palette, 360).data.levels, vec![("volume".into(), SAMPLE_LEVEL)]);
        let live = WidgetScene::default();
        assert!(live.data.toggles.is_empty() && live.data.levels.is_empty() && live.data.palette.is_empty());
    }

    #[test]
    fn palette_examples_keep_all_theme_roles_and_the_widgets_own_language() {
        use super::super::layout::{MarkKind, tests::Fixed};
        use mandara_config::desktop_widgets::{PaletteWidget, TextOptions};
        let palette = mandara_theme::engine::default_palette(true);
        let options = PaletteWidget { locale: TextOptions { language: "ar".into(), ..Default::default() }, ..Default::default() };
        let widget = DesktopWidget { kind: WidgetKind::Palette(options.clone()), ..Default::default() };
        let colors = sample_scene(&widget, &palette, 360).data.palette;
        assert_eq!(colors.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(), ["Primary", "Secondary", "Tertiary", "Surface", "Text", "Urgent", "Success", "Warning"]);
        assert_eq!(colors[0].1, palette.primary.to_rgba_u32() >> 8);
        let language = super::super::lingo::Lingo::of(&options.locale, false, None);
        let layout = super::super::identity::palette(&options, &colors, &language, 1.0, &mut Fixed);
        let primary = mandara_i18n::tr_in(mandara_i18n::Lang::Ar, "Primary");
        assert!(layout.marks.iter().any(|mark| matches!(&mark.kind, MarkKind::Text { text, .. } if text == &primary)));
    }
}
