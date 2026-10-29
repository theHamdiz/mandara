//! Shows the desktop background briefly on the *overlay* layer and
//! screenshots it with `grim`, to compare the GPUI and software renderers
//! on a desktop that is covered by windows:
//!
//! `cargo run -p mandara-ui --example wallpaper_peek -- soft|gpui OUT.png [SECONDS]`
//!
//! Uses the current config read-only (no hub services: no weather, no
//! audio ring). The overlay takes no keyboard or pointer input.

use std::time::Duration;

use gpui::{App, AppContext as _, px, size};
use mandara_core::LayerSurfaceSpec;
use mandara_core::shm_canvas::CanvasLayer;

/// Default time on screen before the screenshot, seconds.
const DEFAULT_SECONDS: f32 = 2.0;
/// Transition length in `transition` mode, ms (long enough to catch mid-way).
const TRANSITION_MS: u64 = 3000;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = args.first().cloned().unwrap_or_else(|| "soft".into());
    let out = args.get(1).cloned().unwrap_or_else(|| "/tmp/wallpaper-peek.png".into());
    let seconds: f32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(DEFAULT_SECONDS);
    let store = mandara_config::ConfigStore::open_default().expect("config");
    let cfg = store.snapshot();
    mandara_i18n::set_language(cfg.general.locale.lang());
    if mode == "ring-bench" {
        // Cost of one audio-ring frame at 3840 × 2160 @ 1.5 (paint + the
        // copy into a shared-memory-sized buffer), for the CPU estimate.
        use mandara_ui::backdrop::paint::{DecorGeometry, DecorInput, FontNames, paint_ring};
        const FRAMES: u32 = 300;
        let fonts = FontNames { ui: "Inter".into(), display: "Alexandria".into(), arabic: "Cairo".into(), mono: "JetBrains Mono".into() };
        let lines = vec!["07".to_string(), "20".to_string()];
        let ink = mandara_ui::desktop::ink_for(None, mandara_theme::Color::oklch(0.7, 0.12, 200.0));
        let input = DecorInput { logical: (2560.0, 1440.0), scale120: 180, cfg: &cfg.wallpaper, lines: &lines, date: None, ink, fonts: &fonts, safe: Default::default() };
        let mut text = mandara_ui::lock::paint::TextPainter::new();
        let geo = DecorGeometry::new(&input, &mut text);
        let mut pm = tiny_skia::Pixmap::new(geo.ring.w, geo.ring.h).expect("ring plane");
        let mut shm = vec![0u8; pm.data().len()];
        let t = std::time::Instant::now();
        for i in 0..FRAMES {
            let bands: Vec<f32> = (0..32).map(|b| ((i + b) % 17) as f32 / 17.0).collect();
            pm.fill(tiny_skia::Color::TRANSPARENT);
            paint_ring(&mut pm.as_mut(), &geo, &bands, 1.0, ink.accent);
            shm.copy_from_slice(pm.data());
        }
        let ms = t.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64;
        println!("ring plane {}x{}: {ms:.2} ms/frame → {:.1}% of one core at 30 fps", geo.ring.w, geo.ring.h, ms * 30.0 / 10.0);
        let t = std::time::Instant::now();
        for _ in 0..FRAMES {
            pm.fill(tiny_skia::Color::TRANSPARENT);
            shm.copy_from_slice(pm.data());
        }
        println!("  of which clear + copy: {:.2} ms", t.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64);
        return;
    }
    if mode == "widget-bench" {
        // Cost of one plane-visualizer frame per shape at 3840 × 2160 @ 1.5
        // (clear + paint + the copy into a shared-memory-sized buffer), and
        // of one system-monitor sample.
        use mandara_config::desktop_widgets::{PlaneVisualizer, VisualizerShape};
        use mandara_ui::backdrop::widgets::{sysmon, visualizer};
        const FRAMES: u32 = 600;
        const FPS: f64 = 30.0;
        let k = 2160.0 / 1080.0;
        let accent = mandara_theme::Color::oklch(0.75, 0.12, 200.0);
        for (shape, angle) in [(VisualizerShape::Bars, -30.0), (VisualizerShape::Mirrored, 0.0), (VisualizerShape::Wave, 0.0), (VisualizerShape::Radial, 0.0)] {
            let v = PlaneVisualizer { shape, angle, ..PlaneVisualizer::default() };
            let g = visualizer::Geometry::new(&v, k);
            let (w, h) = (g.size.0.ceil() as u32, g.size.1.ceil() as u32);
            let mut pm = tiny_skia::Pixmap::new(w, h).expect("plane");
            let mut shm = vec![0u8; pm.data().len()];
            let mut painter = visualizer::Painter::default();
            let t = std::time::Instant::now();
            for i in 0..FRAMES {
                let bands: Vec<f32> = (0..32).map(|b| ((i + b) % 17) as f32 / 17.0).collect();
                pm.fill(tiny_skia::Color::TRANSPARENT);
                painter.frame(&mut pm.as_mut(), &g, &bands, accent, 1.0);
                shm.copy_from_slice(pm.data());
            }
            let ms = t.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64;
            println!("{shape:?} plane {w}x{h} ({:.2} MiB/buffer): {ms:.3} ms/frame → {:.2}% of one core at 30 fps", (w * h * 4) as f64 / 1048576.0, ms * FPS / 10.0);
        }
        let mut s = sysmon::Sampler::new();
        let mut wants = sysmon::Wants::default();
        wants.add(&mandara_config::desktop_widgets::MonitorRow::ALL);
        let t = std::time::Instant::now();
        for _ in 0..100 {
            s.sample(wants, std::time::Instant::now());
        }
        println!("system monitor sample (all rows): {:.3} ms", t.elapsed().as_secs_f64() * 10.0);
        return;
    }
    if mode == "preview" {
        // Offscreen only: the software painters with a sample date line.
        let size = args.get(2).map(String::as_str).unwrap_or("3840x2160@1.5");
        let now = chrono::Local::now().naive_local();
        let input = mandara_ui::desktop::PreviewInput {
            wallpaper: cfg.wallpaper.path.clone().expect("a wallpaper is set"),
            out: out.clone().into(),
            size: mandara_ui::desktop::parse_size(size).expect("WxH[@scale]"),
            scale: mandara_ui::desktop::parse_scale(size),
            cfg: cfg.wallpaper.clone(),
            lines: mandara_ui::desktop::clock_lines(&now, cfg.wallpaper.clock_style, cfg.general.clock_24h),
            date: Some(mandara_ui::desktop::DateLine {
                gregorian: "Tuesday, 27 October".into(),
                hijri_ar: ("٥".into(), "جمادى الأولى", "١٤٤٨".into()),
                hijri_latin: "5 Jumada al-Ula 1448".into(),
                weather: Some(("cloud-sun", "23°".into(), "Partly cloudy".into())),
            }),
            fonts: mandara_ui::Fonts { ui: "Inter".into(), display: "Alexandria".into(), arabic: "Cairo".into(), mono: "JetBrains Mono".into(), emoji: None },
            primary: mandara_theme::Color::oklch(0.7, 0.12, 200.0),
            widgets: mandara_ui::backdrop::widgets::WidgetScene {
                items: cfg.desktop.widgets.painted().cloned().collect(),
                weather: Some(("cloud-sun", "23°".into())),
                h24: cfg.general.clock_24h,
                rtl: cfg.general.locale.is_arabic(),
                ..Default::default()
            },
        };
        let t = std::time::Instant::now();
        println!("{}", mandara_ui::desktop::render_preview(input).expect("preview"));
        println!("{} ms", t.elapsed().as_millis());
        return;
    }
    let outputs = mandara_core::query_outputs().unwrap_or_default();
    gpui_platform::application().with_assets(mandara_ui::MandaraAssets).run(move |cx: &mut App| {
        cx.set_global(mandara_ui::Fonts::resolve(&cfg.typography, cx));
        mandara_ui::init_state(store, cx);
        let hub = cx.new(|_| mandara_ui::hub::Hub::default());
        cx.set_global(mandara_ui::hub::GlobalHub(hub));
        let output = outputs.first().map(|o| o.name.clone()).unwrap_or_default();
        if mode == "gpui" {
            let (w, h) = outputs.first().map(|o| (o.width as f32, o.height as f32)).unwrap_or((1920.0, 1080.0));
            let mut spec = LayerSurfaceSpec::background(size(px(w), px(h)), cx.displays().first().map(|d| d.id()));
            spec.layer = gpui::layer_shell::Layer::Overlay;
            let monitor = output.clone();
            cx.open_window(spec.window_options(), move |_, cx| cx.new(|cx| mandara_ui::wallpaper::WallpaperView::on_monitor(monitor, cx))).expect("window");
        } else {
            mandara_ui::backdrop::init_on(CanvasLayer::Overlay, |_| eprintln!("software renderer failed"), cx);
        }
        // `transition`: after the first image shows, switch to `NEXT_IMAGE`
        // (env) with `wallpaper.transition` = `TRANSITION` (env) and grab the
        // screen mid-way. Run with a scratch XDG_CONFIG_HOME: this edits config.
        let next = std::env::var("NEXT_IMAGE").ok().filter(|_| mode == "transition");
        let kind = std::env::var("TRANSITION").unwrap_or_else(|_| "wipe".into());
        let switch_output = output.clone();
        cx.spawn(async move |cx| {
            if let Some(next) = next {
                cx.background_executor().timer(Duration::from_secs_f32(seconds)).await;
                cx.update(|cx| {
                    mandara_ui::update_config(cx, |c| {
                        c.wallpaper.transition = serde_json::from_value(serde_json::json!(kind)).unwrap_or_default();
                        c.wallpaper.crossfade_ms = TRANSITION_MS;
                        c.wallpaper.variants.monitors.insert(switch_output.clone(), next.into());
                    })
                });
            }
            cx.background_executor().timer(Duration::from_secs_f32(seconds)).await;
            let status = std::process::Command::new("grim").args(["-l", "1", "-o", &output, &out]).status();
            eprintln!("grim → {out}: {status:?}");
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}
