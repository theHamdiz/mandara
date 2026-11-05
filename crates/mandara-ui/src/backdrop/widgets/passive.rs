//! Cached, non-interactive widget painting beneath authentication controls.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};
use std::sync::atomic::{AtomicUsize, Ordering};

use chrono::Local;
use mandara_config::preauth::Widgets;
use mandara_services::weather::{WeatherQuery, WeatherService};
use mandara_theme::MandaraPalette;
use tiny_skia::{Pixmap, PixmapMut, PixmapPaint, Transform};

use super::{Forecast, OutputCx, WidgetScene};
use crate::backdrop::paint::FontNames;
use crate::lock::paint::TextPainter;

/// Keep the transparent cache within a 4K frame. Larger outputs scale the
/// same vector layout up without keeping a second full-resolution canvas.
const MAX_WIDTH: u32 = 3840;
const MAX_HEIGHT: u32 = 2160;
/// Static faces change at minute boundaries, not on every animation frame.
const SECONDS_PER_MINUTE: i64 = 60;
/// Public forecasts share the existing weather service and its limits.
const WEATHER_REFRESH_MINUTES: u32 = 15;
/// Distinct output sizes retained during a resize or on several monitors.
const MAX_CACHES: usize = 8;
/// Painting shares one font system; keep its waiting work bounded too.
const MAX_PENDING: usize = 2;

pub fn ticks_seconds(cfg: &Widgets) -> bool {
    cfg.items.iter().any(|w| w.enabled && matches!(&w.kind, mandara_config::desktop_widgets::WidgetKind::Clock(c) if super::ticks_seconds(c) || (c.face == mandara_config::desktop_widgets::ClockFace::Template && c.lines.iter().any(|line| line.text.contains("{ss}")))))
}

fn stamp(cfg: &Widgets) -> i64 {
    Local::now().timestamp().div_euclid(if ticks_seconds(cfg) { 1 } else { SECONDS_PER_MINUTE })
}

fn frame(size: (u32, u32)) -> (u32, u32) {
    let scale = (MAX_WIDTH as f64 / f64::from(size.0.max(1))).min(MAX_HEIGHT as f64 / f64::from(size.1.max(1))).min(1.0);
    ((f64::from(size.0.max(1)) * scale).round().max(1.0) as u32, (f64::from(size.1.max(1)) * scale).round().max(1.0) as u32)
}

#[derive(Clone, PartialEq)]
struct PaintSpec {
    cfg: Widgets,
    fonts: FontNames,
    palette: MandaraPalette,
    minute: i64,
    h24: bool,
    lang: mandara_i18n::Lang,
    forecast: Option<Forecast>,
}

#[derive(Default)]
struct Cache {
    wanted: Option<PaintSpec>,
    completed: Option<PaintSpec>,
    pending: Option<(PaintSpec, mpsc::Receiver<Option<Pixmap>>)>,
    pixels: Option<Pixmap>,
    image: Option<Arc<gpui::RenderImage>>,
    used: u64,
}

/// Counts actual workers, including requests retired during an edit.
struct Job(Arc<AtomicUsize>);
impl Drop for Job {
    fn drop(&mut self) { self.0.fetch_sub(1, Ordering::Relaxed); }
}

/// No input handlers, user data loaders or periodic work when empty.
#[derive(Default)]
pub struct Canvas {
    cfg: Option<Widgets>,
    caches: HashMap<(u32, u32), Cache>,
    painter: Arc<Mutex<Option<TextPainter>>>,
    weather: Option<(WeatherQuery, WeatherService)>,
    forecast: Option<Forecast>,
    visits: u64,
    trash: Vec<Arc<gpui::RenderImage>>,
    jobs: Arc<AtomicUsize>,
}

impl Canvas {
    pub fn clear(&mut self) {
        if self.cfg.is_none() { return; }
        self.cfg = None;
        self.clear_caches();
        self.weather = None;
        self.forecast = None;
        self.painter = Arc::default();
    }

    pub fn pending(&self) -> bool {
        self.caches.values().any(|c| c.pending.is_some() || c.wanted != c.completed)
    }

    fn clear_caches(&mut self) {
        self.trash.extend(self.caches.drain().filter_map(|(_, cache)| cache.image));
    }

    pub fn take_trash(&mut self) -> Vec<Arc<gpui::RenderImage>> {
        std::mem::take(&mut self.trash)
    }

    /// Collect completed work; the caller decides when to redraw.
    pub fn poll(&mut self) -> bool {
        let mut changed = false;
        if let Some((_, weather)) = &self.weather {
            while let Ok(state) = weather.updates.try_recv() {
                let forecast = state.weather.as_ref().map(|w| Forecast::from_weather(w, self.cfg.as_ref().and_then(|c| c.weather.as_ref()).map_or("", |w| w.place.as_str())));
                if self.forecast != forecast { self.forecast = forecast; changed = true; }
            }
        }
        for cache in self.caches.values_mut() {
            changed |= cache.wanted.as_ref().is_some_and(|spec| spec.minute != stamp(&spec.cfg));
            changed |= cache.pending.is_none() && cache.wanted != cache.completed && self.jobs.load(Ordering::Relaxed) < MAX_PENDING;
            let result = cache.pending.as_ref().and_then(|(_, rx)| match rx.try_recv() {
                Ok(pixels) => Some(pixels),
                Err(mpsc::TryRecvError::Disconnected) => Some(None),
                Err(mpsc::TryRecvError::Empty) => None,
            });
            if let Some(pixels) = result {
                let (spec, _) = cache.pending.take().unwrap();
                if cache.wanted.as_ref() == Some(&spec) {
                    cache.pixels = pixels;
                    if let Some(image) = cache.image.take() { self.trash.push(image); }
                    cache.completed = Some(spec);
                    changed = true;
                }
            }
        }
        changed
    }

    pub fn update(&mut self, cfg: &Widgets, size: (u32, u32), fonts: &FontNames, palette: &MandaraPalette, h24: bool, allow_network: bool) {
        if !cfg.items.iter().any(|w| w.enabled && mandara_config::preauth::supported(&w.kind)) {
            if self.cfg.is_some() { self.clear(); }
            return;
        }
        if self.cfg.as_ref() != Some(cfg) {
            self.clear_caches();
            self.forecast = None;
            self.weather = None;
            self.cfg = Some(cfg.clone());
        }
        let query = weather_query(cfg, allow_network);
        if self.weather.as_ref().map(|(q, _)| *q) != query {
            self.weather = query.map(|q| (q, mandara_services::weather::spawn_weather_service(q)));
            self.forecast = None;
        }
        self.poll();
        let size = frame(size);
        if !self.caches.contains_key(&size) && self.caches.len() >= MAX_CACHES {
            let oldest = self.caches.iter().filter(|(_, c)| c.pending.is_none()).min_by_key(|(_, c)| c.used).map(|(size, _)| *size);
            match oldest {
                Some(oldest) => if let Some(image) = self.caches.remove(&oldest).and_then(|c| c.image) { self.trash.push(image); },
                None => return,
            }
        }
        self.visits = self.visits.wrapping_add(1);
        let pending = self.jobs.load(Ordering::Relaxed);
        let minute = stamp(cfg);
        let cache = self.caches.entry(size).or_default();
        cache.used = self.visits;
        // Compare borrowed values before allocating a new request.
        let same = cache.wanted.as_ref().is_some_and(|s| s.cfg == *cfg && s.fonts == *fonts && s.palette == *palette && s.minute == minute && s.h24 == h24 && s.lang == mandara_i18n::language() && s.forecast == self.forecast);
        if !same {
            cache.wanted = Some(PaintSpec { cfg: cfg.clone(), fonts: fonts.clone(), palette: palette.clone(), minute, h24, lang: mandara_i18n::language(), forecast: self.forecast.clone() });
        }
        if pending >= MAX_PENDING || cache.pending.is_some() || cache.completed == cache.wanted { return; }
        let spec = cache.wanted.clone().unwrap();
        let input = spec.clone();
        let painter = self.painter.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        self.jobs.fetch_add(1, Ordering::Relaxed);
        let permit = Job(self.jobs.clone());
        match std::thread::Builder::new().name("auth-widgets".into()).spawn(move || {
            let _permit = permit;
            let pixels = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let mut text = painter.lock().unwrap_or_else(|e| e.into_inner());
                paint(&input, size, text.get_or_insert_with(TextPainter::new))
            })).ok().flatten();
            let _ = tx.send(pixels);
        }) {
            Ok(_) => cache.pending = Some((spec, rx)),
            Err(error) => { tracing::warn!("painting authentication widgets: {error}"); cache.completed = Some(spec); }
        }
    }

    pub fn draw(&self, target: &mut PixmapMut<'_>, opacity: f32) {
        let Some(pixels) = self.caches.get(&frame((target.width(), target.height()))).and_then(|c| c.pixels.as_ref()) else { return };
        let transform = Transform::from_scale(target.width() as f32 / pixels.width() as f32, target.height() as f32 / pixels.height() as f32);
        target.draw_pixmap(0, 0, pixels.as_ref(), &PixmapPaint { opacity: opacity.clamp(0.0, 1.0), ..Default::default() }, transform, None);
    }

    pub fn image(&mut self, size: (u32, u32)) -> Option<Arc<gpui::RenderImage>> {
        let cache = self.caches.get_mut(&frame(size))?;
        if cache.image.is_none() {
            cache.image = super::straight(cache.pixels.as_ref()?.clone()).map(super::render_image);
        }
        cache.image.clone()
    }
}

/// A forecast is useful only when a visible widget actually draws it.
fn weather_query(cfg: &Widgets, allow_network: bool) -> Option<WeatherQuery> {
    use mandara_config::desktop_widgets::WidgetKind;
    let visible = cfg.items.iter().any(|w| w.enabled && match &w.kind {
        WidgetKind::Weather(_) => true,
        WidgetKind::Clock(clock) => clock.shows_weather(),
        _ => false,
    });
    cfg.weather.as_ref().filter(|w| w.valid() && allow_network && visible)
        .map(|w| WeatherQuery { latitude: w.latitude, longitude: w.longitude, refresh_minutes: WEATHER_REFRESH_MINUTES })
}

fn tone(palette: &MandaraPalette) -> image::RgbImage {
    let [r, g, b, _] = palette.surface_container.to_rgba8();
    image::RgbImage::from_pixel(1, 1, image::Rgb([r, g, b]))
}

fn scene(cfg: &Widgets, spec: &PaintSpec) -> WidgetScene {
    let mut scene = WidgetScene {
        items: cfg.items.clone(),
        on_surface: spec.palette.on_surface,
        surface: spec.palette.surface_container,
        units: cfg.units,
        h24: spec.h24,
        rtl: spec.lang.is_rtl(),
        forecast: spec.forecast.clone(),
        world_cities: mandara_services::world_clock::DEFAULT_CITIES.iter().map(|city| (*city).into()).collect(),
        weather: spec.forecast.as_ref().map(|f| (f.icon, cfg.units.format_degrees(f.temp))),
        ..Default::default()
    };
    scene.data.place = cfg.weather.as_ref().filter(|w| w.valid()).map(|w| (w.latitude, w.longitude));
    scene.data.palette = super::super::palette_colors(&spec.palette);
    scene
}

fn paint(spec: &PaintSpec, size: (u32, u32), text: &mut TextPainter) -> Option<Pixmap> {
    let mut canvas = Pixmap::new(size.0, size.1)?;
    let mut cfg = spec.cfg.clone();
    cfg.normalize();
    let mut scene = scene(&cfg, spec);
    let names: [&str; 0] = [];
    let tone = tone(&spec.palette);
    let output = OutputCx { id: 0, names: &names, logical: (size.0 as f32, size.1 as f32), physical: size, scale120: mandara_core::shm_canvas::SCALE_DENOMINATOR, tone: Some(&tone), primary: spec.palette.primary, fonts: &spec.fonts, depth: false, subject: None, safe: Default::default() };
    let now = Local::now();
    // Public calendars use civil midnight; no private prayer settings are read.
    scene.hijri = Some(super::lingo::hijri_of(now.date_naive()));
    for widget in cfg.items.iter().filter(|w| w.enabled) {
        super::compose(&mut canvas.as_mut(), widget, &output, &scene, None, text, &now, &[]);
    }
    Some(canvas)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forecasts_require_a_visible_weather_view_and_public_consent() {
        use mandara_config::desktop_widgets::{ClockFace, ClockWidget, DesktopWidget, WeatherWidget, WidgetKind};
        let mut cfg = Widgets { weather: Some(mandara_config::preauth::PublicWeather { latitude: 31.2, longitude: 29.9, place: "Demo".into() }), ..Default::default() };
        for face in ClockFace::ALL {
            cfg.items = vec![DesktopWidget { kind: WidgetKind::Clock(ClockWidget { face, weather: true, ..Default::default() }), ..Default::default() }];
            assert_eq!(weather_query(&cfg, true).is_some(), face == ClockFace::DayBlock, "{face:?}");
            assert!(weather_query(&cfg, false).is_none());
        }
        cfg.items = vec![DesktopWidget { kind: WidgetKind::Weather(WeatherWidget::default()), ..Default::default() }];
        assert!(weather_query(&cfg, true).is_some());
        cfg.items[0].enabled = false;
        assert!(weather_query(&cfg, true).is_none());
        cfg.items[0] = DesktopWidget { kind: WidgetKind::Clock(ClockWidget { face: ClockFace::DayBlock, weather: false, ..Default::default() }), ..Default::default() };
        assert!(weather_query(&cfg, true).is_none());
        cfg.weather = None;
        assert!(weather_query(&cfg, true).is_none());
    }

    #[test]
    fn large_outputs_keep_their_aspect_and_a_bounded_cache() {
        assert_eq!(frame((1920, 1080)), (1920, 1080));
        assert_eq!(frame((7680, 4320)), (3840, 2160));
        assert_eq!(frame((1080, 3840)), (608, 2160));
    }

    #[test]
    fn empty_widgets_allocate_no_font_system_or_worker() {
        let mut canvas = Canvas::default();
        canvas.clear();
        assert!(!canvas.pending());
        assert!(canvas.painter.lock().unwrap().is_none());
        assert!(canvas.weather.is_none());
        assert!(canvas.caches.is_empty());
    }

    #[test]
    fn a_replaced_request_cannot_publish_old_pixels() {
        let cfg = Widgets { items: vec![Default::default()], ..Default::default() };
        let desired = PaintSpec { cfg: cfg.clone(), fonts: Default::default(), palette: mandara_theme::engine::default_palette(true), minute: stamp(&cfg), h24: true, lang: mandara_i18n::Lang::En, forecast: None };
        let mut old = desired.clone();
        old.cfg.items[0].place.x = 0.25;
        let (tx, rx) = mpsc::channel();
        let mut canvas = Canvas::default();
        canvas.caches.insert((640, 480), Cache { wanted: Some(desired), pending: Some((old, rx)), ..Default::default() });
        tx.send(Pixmap::new(1, 1)).unwrap();
        canvas.poll();
        let cache = &canvas.caches[&(640, 480)];
        assert!(cache.pending.is_none());
        assert!(cache.pixels.is_none());
        assert!(cache.completed.is_none());
    }

    #[test]
    fn hidden_seconds_do_not_keep_static_faces_ticking() {
        use mandara_config::desktop_widgets::{ClockFace, ClockWidget, DesktopWidget, WidgetKind};
        let mut cfg = Widgets { items: vec![DesktopWidget { kind: WidgetKind::Clock(ClockWidget { face: ClockFace::Nixie, seconds: true, ..Default::default() }), ..Default::default() }], ..Default::default() };
        assert!(!ticks_seconds(&cfg));
        if let WidgetKind::Clock(c) = &mut cfg.items[0].kind { c.face = ClockFace::Analog; }
        assert!(ticks_seconds(&cfg));
        cfg.items[0].enabled = false;
        assert!(!ticks_seconds(&cfg));
    }
    #[test]
    fn public_world_clocks_show_default_cities_and_preserve_explicit_lists() {
        use mandara_config::desktop_widgets::{DesktopWidget, WidgetKind, WorldClockWidget};
        let cfg = Widgets { items: vec![DesktopWidget { kind: WidgetKind::WorldClock(WorldClockWidget::default()), ..Default::default() }], ..Default::default() };
        let spec = PaintSpec { cfg: cfg.clone(), fonts: Default::default(), palette: mandara_theme::engine::default_palette(true), minute: stamp(&cfg), h24: true, lang: mandara_i18n::Lang::En, forecast: None };
        let scene = scene(&cfg, &spec);
        let now = Local::now();
        let cities = super::super::texts::cities(&WorldClockWidget::default(), &scene, &now);
        assert_eq!(cities.len(), mandara_services::world_clock::DEFAULT_CITIES.len());
        let explicit = WorldClockWidget { cities: vec!["Tokyo".into()], ..Default::default() };
        let cities = super::super::texts::cities(&explicit, &scene, &now);
        assert_eq!(cities.len(), 1);
        assert_eq!(cities[0].name, "Tokyo");
        assert!(scene.data.todos.is_empty() && scene.data.notes.is_empty() && scene.data.events.is_empty());
    }

    #[test]
    fn automatic_ink_follows_the_authentication_theme() {
        for dark in [false, true] {
            let palette = mandara_theme::engine::default_palette(dark);
            let auto = crate::desktop::ink_for(Some(palette.surface_container), palette.primary);
            assert!(auto.fg.contrast_ratio(palette.surface_container) >= 4.5);
            let sample = tone(&palette).get_pixel(0, 0).0;
            let [r, g, b, _] = palette.surface_container.to_rgba8();
            assert_eq!(sample, [r, g, b]);
        }
    }

}
