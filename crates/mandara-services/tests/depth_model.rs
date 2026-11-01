//! The wallpaper depth model end to end, run only where the model file is
//! present (`$MANDARA_DEPTH_MODEL`, else the downloaded copy in
//! `~/.local/share/mandara/models/depth-anything-v2-small/model.onnx`):
//! CI and fresh checkouts skip it.

use std::path::PathBuf;
use std::time::Instant;

use image::{DynamicImage, RgbImage};
use mandara_services::depth_model::{self, INFERENCE_BUDGET};

fn model() -> Option<PathBuf> {
    let path = std::env::var_os("MANDARA_DEPTH_MODEL").map(PathBuf::from).unwrap_or_else(depth_model::model_path);
    if path.is_file() {
        Some(path)
    } else {
        eprintln!("skipped: no depth model at {}", path.display());
        None
    }
}

/// A tiny scene: a sky gradient, a far ridge and a near, textured bank of
/// ground with a dark post standing on it.
fn scene(w: u32, h: u32) -> RgbImage {
    RgbImage::from_fn(w, h, |x, y| {
        let (fx, fy) = (x as f32 / w as f32, y as f32 / h as f32);
        let ridge = 0.45 + 0.05 * (fx * 9.0).sin();
        let post = (0.62..0.68).contains(&fx) && fy > 0.3;
        if post {
            image::Rgb([24, 20, 18])
        } else if fy < ridge {
            image::Rgb([(90.0 + 100.0 * fy) as u8, (140.0 + 80.0 * fy) as u8, 230])
        } else if fy < 0.6 {
            image::Rgb([70, 90, 110])
        } else {
            let grain = ((x * 7 + y * 13) % 17) as u8;
            image::Rgb([90 + grain, 70 + grain, 40])
        }
    })
}

#[test]
fn model_orders_a_tiny_scene_far_to_near() {
    let Some(model) = model() else { return };
    let img = DynamicImage::ImageRgb8(scene(192, 108));
    let t = Instant::now();
    let depth = depth_model::estimate(&model, &img, INFERENCE_BUDGET).expect("inference");
    eprintln!("estimated in {:?}", t.elapsed());
    assert_eq!((depth.width, depth.height), (depth_model::INPUT_SIDE, (depth_model::INPUT_SIDE as f32 * 108.0 / 192.0).round() as u32));
    let row_mean = |fy: f32| {
        let y = ((depth.height as f32 * fy) as usize).min(depth.height as usize - 1);
        let w = depth.width as usize;
        depth.data[y * w..(y + 1) * w].iter().sum::<f32>() / w as f32
    };
    assert!(row_mean(0.95) > row_mean(0.05) + 0.3, "ground nearer than sky: {} vs {}", row_mean(0.95), row_mean(0.05));
    // The cut-out keeps the near ground and leaves the sky behind the clock.
    let photo = scene(384, 216);
    let (alpha, split) = depth_model::subject_alpha(&depth, &photo, None).expect("a subject");
    eprintln!("{}", split.describe());
    assert!(alpha.get_pixel(192, 10).0[0] < 32, "sky stays behind");
    assert!(alpha.get_pixel(60, 205).0[0] > 224, "near ground comes in front");
}

/// Writes red overlays of the model's cut-outs for every image in
/// `$MANDARA_DEPTH_DIR` to `$MANDARA_DEPTH_OUT`, for eyeballing edges:
/// `MANDARA_DEPTH_DIR=… MANDARA_DEPTH_OUT=… cargo test -p mandara-services --test depth_model -- --ignored --nocapture`.
#[test]
#[ignore]
fn depth_folder_overlays() {
    let (Some(model), Ok(dir), Ok(out)) = (model(), std::env::var("MANDARA_DEPTH_DIR"), std::env::var("MANDARA_DEPTH_OUT")) else { return };
    let out = PathBuf::from(out);
    std::fs::create_dir_all(&out).unwrap();
    let mut paths: Vec<PathBuf> = std::fs::read_dir(dir).unwrap().filter_map(|e| e.ok().map(|e| e.path())).filter(|p| p.extension().is_some_and(|e| ["png", "jpg", "jpeg", "webp"].contains(&e.to_string_lossy().to_lowercase().as_str()))).collect();
    paths.sort();
    let limit: usize = std::env::var("MANDARA_DEPTH_LIMIT").ok().and_then(|v| v.parse().ok()).unwrap_or(usize::MAX);
    let split: Option<f32> = std::env::var("MANDARA_DEPTH_SPLIT").ok().and_then(|v| v.parse().ok());
    for path in paths.into_iter().take(limit) {
        let Ok(img) = image::open(&path) else { continue };
        let name = path.file_stem().unwrap().to_string_lossy().to_string();
        // Depth maps are kept next to the overlays, so tuning the cut
        // doesn't rerun the model.
        let cached = out.join(format!("{name}.depth.png"));
        let t = Instant::now();
        let depth = match depth_model::DepthMap::load(&cached) {
            Ok(d) => d,
            Err(_) => {
                let d = depth_model::estimate(&model, &img, INFERENCE_BUDGET).unwrap();
                d.save(&cached).unwrap();
                d
            }
        };
        let ms = t.elapsed().as_millis();
        let scale = (1600.0 / img.width().max(img.height()) as f32).min(1.0);
        let photo = img.resize_exact((img.width() as f32 * scale) as u32, (img.height() as f32 * scale) as u32, image::imageops::FilterType::Triangle).to_rgb8();
        match depth_model::subject_alpha(&depth, &photo, split) {
            Ok((alpha, s)) => {
                println!("{name}: {} ({ms} ms)", s.describe());
                let overlay = RgbImage::from_fn(alpha.width(), alpha.height(), |x, y| {
                    let a = alpha.get_pixel(x, y).0[0] as f32 / 255.0;
                    let p = photo.get_pixel(x, y).0;
                    image::Rgb([(p[0] as f32 * (1.0 - 0.5 * a) + 127.0 * a) as u8, (p[1] as f32 * (1.0 - 0.5 * a)) as u8, (p[2] as f32 * (1.0 - 0.5 * a)) as u8])
                });
                overlay.save(out.join(format!("{name}.jpg"))).unwrap();
            }
            Err(why) => println!("{name}: declined: {why} ({ms} ms)"),
        }
    }
}
