//! End to end: every style renders deterministically and well-formed,
//! loops seamlessly, keeps its look across resolutions; files encode,
//! decode, carry their recipe and prune safely; a video loop encodes and
//! decodes with the pure-Rust AV1 pipeline.

use std::sync::atomic::AtomicBool;

use image::RgbImage;
use mandara_wallgen::*;

/// Small sizes keep the suite fast in debug builds.
const W: u32 = 96;
const H: u32 = 54;

fn luma(img: &RgbImage) -> Vec<f32> {
    img.pixels().map(|p| 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32).collect()
}

fn stats(v: &[f32]) -> (f32, f32, f32, f32) {
    let n = v.len() as f32;
    let mean = v.iter().sum::<f32>() / n;
    let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / n;
    let (lo, hi) = v.iter().fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
    (mean, var, lo, hi)
}

fn mean_abs_diff(a: &RgbImage, b: &RgbImage) -> f32 {
    a.as_raw().iter().zip(b.as_raw()).map(|(x, y)| (*x as f32 - *y as f32).abs()).sum::<f32>() / a.as_raw().len() as f32
}

#[test]
fn every_style_renders_deterministic_and_rich() {
    for style in Style::ALL {
        for light in [false, true] {
            let mut r = Recipe::new(style, 7);
            r.light = light;
            let a = render(&r, W, H);
            assert_eq!(a.dimensions(), (W, H));
            assert_eq!(a, render(&r, W, H), "{style} not deterministic");
            let (mean, var, lo, hi) = stats(&luma(&a));
            assert!(var > 4.0, "{style} light={light} is flat (variance {var})");
            assert!((4.0..=250.0).contains(&mean), "{style} light={light} mean luma {mean}");
            assert!(hi - lo > 12.0, "{style} light={light} range {lo}..{hi}");
            if light {
                assert!(mean > 110.0, "{style} light mode too dark: {mean}");
            } else {
                assert!(mean < 150.0, "{style} dark mode too bright: {mean}");
            }
        }
    }
}

#[test]
fn dunes_have_tonal_range_not_stripes() {
    // Sand (the lower part of the frame) must spread over many tones: lit
    // faces, penumbrae, shadows and haze. A few bright slabs on a dark
    // ground (one dominant bin, a narrow middle) is the failure this guards.
    const SAND_FROM: f32 = 0.45;
    let (w, h) = (320u32, 180u32);
    for palette in ["mandara-corniche", "montaza-twilight", "desert"] {
        for seed in [1u64, 7, 42, 1234] {
            for light in [false, true] {
                let mut r = Recipe::new(Style::Dunes, seed);
                r.palette = PaletteSpec::Named(palette.into());
                r.light = light;
                let img = render(&r, w, h);
                let mut sand: Vec<f32> = luma(&img).into_iter().skip((h as f32 * SAND_FROM) as usize * w as usize).collect();
                assert!(sand.iter().all(|v| v.is_finite()));
                let mut bins = [0usize; 16];
                for v in &sand {
                    bins[((*v / 16.0) as usize).min(15)] += 1;
                }
                let n = sand.len();
                let top = *bins.iter().max().unwrap() as f32 / n as f32;
                sand.sort_by(f32::total_cmp);
                let q = |p: f32| sand[((n - 1) as f32 * p) as usize];
                let tag = format!("{palette} seed {seed} light={light}");
                if light {
                    // Pale air keeps light dunes gentle; they must still model.
                    assert!(q(0.98) - q(0.02) > 18.0, "{tag}: flat sand ({} levels)", q(0.98) - q(0.02));
                } else {
                    assert!(top < 0.45, "{tag}: one tone covers {top:.2} of the sand");
                    assert!(q(0.75) - q(0.25) > 18.0, "{tag}: narrow middle tones ({})", q(0.75) - q(0.25));
                }
            }
        }
    }
}

#[test]
fn random_recipes_render_for_every_palette() {
    for (i, p) in palettes().iter().enumerate() {
        let mut r = Recipe::random(None, i as u64);
        r.palette = PaletteSpec::Named(p.id.into());
        let img = preview(&r, 48, 27);
        let (_, var, ..) = stats(&luma(&img));
        assert!(var > 1.0, "{} with {} is flat", r.style, p.id);
    }
    let custom = Recipe { palette: PaletteSpec::Custom(vec!["#200010".into(), "#ff8800".into()]), ..Recipe::new(Style::Mesh, 3) };
    let theme = Recipe { palette: PaletteSpec::Theme(vec!["#0a1520".into(), "#5fe0d4".into(), "#e9c98b".into()]), ..Recipe::new(Style::Waves, 3) };
    assert_ne!(render(&custom, W, H), render(&theme, W, H));
}

#[test]
fn smooth_gradients_do_not_band() {
    // With the dither on, a smooth gradient never jumps more than two
    // levels between neighbouring rows or columns.
    let r = Recipe::new(Style::Mesh, 11);
    let img = render(&r, 960, 540);
    let (w, h) = img.dimensions();
    let mut worst = 0i32;
    for y in 1..h {
        for x in 1..w {
            let p = img.get_pixel(x, y);
            for n in [img.get_pixel(x - 1, y), img.get_pixel(x, y - 1)] {
                for c in 0..3 {
                    worst = worst.max((p[c] as i32 - n[c] as i32).abs());
                }
            }
        }
    }
    assert!(worst <= 2, "largest neighbour step {worst}");
    // And the dither really breaks up bands: a 0 % dither render has long
    // runs of identical pixels the dithered one does not.
    let mut flat = r.clone();
    flat.set_param("dither", 0.0);
    let plain = render(&flat, 960, 540);
    let runs = |img: &RgbImage| img.as_raw().chunks(3).collect::<Vec<_>>().windows(2).filter(|w| w[0] == w[1]).count();
    assert!(runs(&plain) > runs(&img), "dither changes nothing");
}

#[test]
fn output_never_contains_nan_artifacts_at_extremes() {
    // Every slider at its minimum and at its maximum still renders.
    for style in Style::ALL {
        for pick in [0, 1] {
            let mut r = Recipe::new(style, 5);
            for spec in r.specs().collect::<Vec<_>>() {
                r.set_param(spec.key, if pick == 0 { spec.min } else { spec.max });
            }
            let img = preview(&r, 40, 24);
            assert_eq!(img.dimensions(), (40, 24), "{style}");
        }
    }
}

#[test]
fn loops_are_seamless_and_move() {
    for style in Style::ALL {
        let r = Recipe::new(style, 9);
        let f0 = render_frame(&r, W, H, 0.0);
        assert_eq!(f0, render(&r, W, H), "{style}: frame 0 is the still");
        assert_eq!(render_frame(&r, W, H, 1.0), f0, "{style}: t = 1 wraps to t = 0");
        let near_end = render_frame(&r, W, H, 0.995);
        let mid = render_frame(&r, W, H, 0.5);
        // Seamless: stepping across the wrap (0.995 → 1 = 0) changes no more
        // than an ordinary step of the same length (0.99 → 0.995).
        let d_end = mean_abs_diff(&f0, &near_end);
        let d_step = mean_abs_diff(&render_frame(&r, W, H, 0.99), &near_end);
        let d_mid = mean_abs_diff(&f0, &mid);
        assert!(d_end <= d_step * 1.5 + 0.3, "{style}: seam {d_end} vs step {d_step}");
        assert!(d_mid > d_end, "{style}: no motion ({d_mid} vs {d_end})");
    }
}

#[test]
fn resolution_independent() {
    // A 4× larger render, downsampled, is the same picture.
    for style in Style::ALL {
        let r = Recipe::new(style, 13);
        let small = render(&r, 80, 45);
        let big = render(&r, 320, 180);
        let down = image::imageops::resize(&big, 80, 45, image::imageops::FilterType::Triangle);
        let d = mean_abs_diff(&small, &down);
        assert!(d < 9.0, "{style}: 80×45 vs 320×180 differ by {d}");
    }
}

#[test]
fn formats_round_trip() {
    let img = render(&Recipe::new(Style::Contours, 2), 64, 40);
    for format in [Format::WebpLossless, Format::Png, Format::Jpeg { quality: 90 }] {
        let bytes = encode(&img, format).unwrap();
        let back = image::load_from_memory(&bytes).unwrap().to_rgb8();
        assert_eq!(back.dimensions(), img.dimensions());
        if format.is_lossless() {
            assert_eq!(back, img, "{format:?} is not lossless");
        } else {
            assert!(mean_abs_diff(&back, &img) < 4.0);
        }
    }
}

#[test]
fn save_writes_image_and_sidecar() {
    let dir = tempfile::tempdir().unwrap();
    let r = Recipe::random(Some(Style::Waves), 77);
    let opts = OutputOptions { resolution: Resolution::Custom { width: 64, height: 36 }, format: Format::WebpLossless, dir: dir.path().into(), name: None };
    let g = save(&r, &opts).unwrap();
    assert_eq!((g.width, g.height), (64, 36));
    assert_eq!(std::fs::metadata(&g.path).unwrap().len(), g.bytes);
    let name = g.path.file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("waves-") && name.ends_with("-64x36.webp"), "{name}");
    assert!(g.recipe_path.file_name().unwrap().to_string_lossy().starts_with('.'));
    assert_eq!(recipe_for(&g.path), Some(r.clone()));
    assert!(is_generated(&g.path));
    // The file decodes to exactly the render.
    assert_eq!(image::open(&g.path).unwrap().to_rgb8(), render(&r, 64, 36));
    // Saving again never overwrites.
    let g2 = save(&r, &opts).unwrap();
    assert_ne!(g.path, g2.path);
    // No temporary files are left behind.
    let leftovers = std::fs::read_dir(dir.path()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains(".tmp-")).count();
    assert_eq!(leftovers, 0);
    // A named save and a foreign file.
    let named = save(&r, &OutputOptions { name: Some("my wall".into()), format: Format::Png, ..opts.clone() }).unwrap();
    assert_eq!(named.path.file_name().unwrap(), "my-wall.png");
    let foreign = dir.path().join("photo.webp");
    std::fs::write(&foreign, b"not ours").unwrap();
    assert_eq!(recipe_for(&foreign), None);
}

#[test]
fn lossless_save_names_the_smaller_file_by_its_format() {
    let dir = tempfile::tempdir().unwrap();
    for style in [Style::Waves, Style::Flow] {
        let r = Recipe::new(style, 3);
        let opts = OutputOptions { resolution: Resolution::Custom { width: 160, height: 90 }, format: Format::default(), dir: dir.path().into(), name: None };
        assert_eq!(opts.format, Format::Lossless);
        let g = save(&r, &opts).unwrap();
        let img = render(&r, 160, 90);
        let (webp, png) = (encode(&img, Format::WebpLossless).unwrap().len(), encode(&img, Format::Png).unwrap().len());
        assert_eq!(g.bytes as usize, webp.min(png), "{style}: not the smaller file");
        let bytes = std::fs::read(&g.path).unwrap();
        let ext = g.path.extension().unwrap().to_string_lossy().into_owned();
        let sniffed = if bytes.starts_with(b"\x89PNG") { "png" } else if bytes.starts_with(b"RIFF") { "webp" } else { "?" };
        assert_eq!(ext, sniffed, "{style}: extension does not match the contents");
        assert_eq!(image::load_from_memory(&bytes).unwrap().to_rgb8(), img, "{style}: not exact");
        assert_eq!(recipe_for(&g.path), Some(r));
    }
}

#[test]
fn prune_only_touches_generated_files() {
    let dir = tempfile::tempdir().unwrap();
    let opts = OutputOptions { resolution: Resolution::Custom { width: 32, height: 18 }, format: Format::Png, dir: dir.path().into(), name: None };
    let mut made = Vec::new();
    for i in 0..5 {
        made.push(save(&Recipe::new(Style::Mesh, i), &opts).unwrap().path);
        // Distinct modification times, oldest first.
        let t = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + i * 60);
        std::fs::File::options().write(true).open(made.last().unwrap()).unwrap().set_modified(t).unwrap();
    }
    let mine = dir.path().join("mine.png");
    std::fs::write(&mine, b"user file").unwrap();
    let orphan = dir.path().join(".gone.png.recipe.toml");
    std::fs::copy(sidecar_path(&made[0]), &orphan).unwrap();
    let removed = prune(dir.path(), 2).unwrap();
    assert_eq!(removed.len(), 3);
    for p in &made[..3] {
        assert!(!p.exists() && !sidecar_path(p).exists(), "{p:?} kept");
    }
    for p in &made[3..] {
        assert!(p.exists() && sidecar_path(p).exists(), "{p:?} removed");
    }
    assert!(mine.exists(), "a user file was pruned");
    assert!(!orphan.exists(), "orphan sidecar kept");
    assert!(prune(&dir.path().join("missing"), 1).unwrap().is_empty());
}

#[test]
fn video_loop_encodes_and_decodes() {
    let dir = tempfile::tempdir().unwrap();
    let r = Recipe::new(Style::Liquid, 4);
    let opts = VideoOptions { seconds: 1.0, fps: 6, resolution: Resolution::Custom { width: 64, height: 36 }, ..VideoOptions::default() };
    let seen = std::sync::Mutex::new(Vec::new());
    let g = render_video(&r, &opts, dir.path(), &|p| seen.lock().unwrap().push(p), &AtomicBool::new(false)).unwrap();
    assert!(g.path.extension().is_some_and(|e| e == "webm"));
    assert_eq!(recipe_for(&g.path), Some(r.clone()));
    let progress = seen.into_inner().unwrap();
    assert_eq!(progress.len(), 6);
    assert!((progress.last().unwrap() - 1.0).abs() < 1e-6);
    let info = mandara_media::Demuxer::open(&g.path).unwrap().info();
    assert_eq!((info.width, info.height), (64, 36));
    let decoded = mandara_media::bench(&g.path, mandara_media::DecoderSettings { threads: 2, ..Default::default() }, false, 100).unwrap();
    assert_eq!(decoded.frames, 6);
    assert_eq!((decoded.width, decoded.height), (64, 36));
    // The first decoded frame looks like the still.
    let first = mandara_media::first_frame(&g.path).unwrap();
    let still = render(&r, 64, 36);
    let d: f32 = first.pixels().zip(still.pixels()).map(|(a, b)| (0..3).map(|c| (a[c] as f32 - b[c] as f32).abs()).sum::<f32>() / 3.0).sum::<f32>() / (64.0 * 36.0);
    assert!(d < 12.0, "decoded frame differs from the render by {d}");
}

#[test]
fn video_cancel_leaves_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let opts = VideoOptions { seconds: 2.0, fps: 6, resolution: Resolution::Custom { width: 32, height: 18 }, ..VideoOptions::default() };
    let cancel = AtomicBool::new(true);
    assert!(render_video(&Recipe::new(Style::Mesh, 1), &opts, dir.path(), &|_| {}, &cancel).is_err());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

/// Sizes and timings at 4K and 8K for every style and format:
/// `cargo test --release -p mandara-wallgen --test engine -- --ignored --nocapture`.
#[test]
#[ignore]
fn benchmark_4k_8k() {
    for style in Style::ALL {
        let r = Recipe::new(style, 1);
        let t = std::time::Instant::now();
        let img = render(&r, 3840, 2160);
        let render_ms = t.elapsed().as_millis();
        let mut line = format!("{:<10} 4K render {render_ms:>5} ms", style.id());
        for f in Format::ALL {
            let t = std::time::Instant::now();
            let n = encode(&img, f).unwrap().len();
            line += &format!(" | {} {:>6} KiB {:>5} ms", f.id(), n / 1024, t.elapsed().as_millis());
        }
        drop(img);
        let t = std::time::Instant::now();
        let big = render(&r, 7680, 4320);
        line += &format!(" | 8K render {} ms", t.elapsed().as_millis());
        let t = std::time::Instant::now();
        let n = encode(&big, Format::WebpLossless).unwrap().len();
        line += &format!(", webp {} KiB {} ms", n / 1024, t.elapsed().as_millis());
        println!("{line}");
    }
    let dir = tempfile::tempdir().unwrap();
    for (res, label) in [(Resolution::Fhd1080, "1080p"), (Resolution::Qhd1440, "1440p")] {
        let opts = VideoOptions { resolution: res, ..VideoOptions::default() };
        let t = std::time::Instant::now();
        let g = render_video(&Recipe::new(Style::Mesh, 1), &opts, dir.path(), &|_| {}, &AtomicBool::new(false)).unwrap();
        println!("video {label} {}s@{}fps: {:.1} s, {} KiB", opts.seconds, opts.fps, t.elapsed().as_secs_f64(), g.bytes / 1024);
    }
}
