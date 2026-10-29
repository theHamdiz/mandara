//! Renders one sample of every style (or the named ones) to
//! `target/wallgen-samples/` (or `$MANDARA_WALLGEN_SAMPLES`) for looking at:
//!
//! ```text
//! cargo run --release -p mandara-wallgen --example samples -- [style…] [--seed N] [--palette ID] [--light] [--size WxH] [--random]
//! ```

use std::time::Instant;

use mandara_wallgen::{PaletteSpec, Recipe, Style, render};

/// Contrast gain of `--stretch` crops.
const STRETCH: f32 = 10.0;

fn main() {
    let mut args = std::env::args().skip(1);
    let (mut styles, mut seed, mut palette, mut light, mut size, mut random, mut stretch) = (Vec::new(), 1u64, None, false, (1920u32, 1080u32), false, false);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--seed" => seed = args.next().and_then(|s| s.parse().ok()).unwrap_or(1),
            "--palette" => palette = args.next(),
            "--light" => light = true,
            "--random" => random = true,
            // Also write a contrast-stretched centre crop: 8-bit steps become
            // visible, so banding (or its absence) can be checked by eye.
            "--stretch" => stretch = true,
            "--size" => {
                if let Some((w, h)) = args.next().and_then(|s| s.split_once('x').map(|(w, h)| (w.parse().unwrap_or(1920), h.parse().unwrap_or(1080)))) {
                    size = (w, h);
                }
            }
            id => styles.extend(Style::from_id(id)),
        }
    }
    if styles.is_empty() {
        styles = Style::ALL.to_vec();
    }
    // `MANDARA_WALLGEN_SAMPLES` overrides the output folder.
    let dir = std::env::var_os("MANDARA_WALLGEN_SAMPLES").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/wallgen-samples"));
    std::fs::create_dir_all(&dir).unwrap();
    for style in styles {
        let mut r = if random { Recipe::random(Some(style), seed) } else { Recipe::new(style, seed) };
        if let Some(p) = &palette {
            r.palette = PaletteSpec::Named(p.clone());
        }
        r.light = light;
        let t0 = Instant::now();
        let img = render(&r, size.0, size.1);
        let ms = t0.elapsed().as_secs_f64() * 1000.0;
        let name = format!("{}{}{}.png", style.id(), if light { "-light" } else { "" }, if random { format!("-r{seed}") } else { String::new() });
        img.save(dir.join(&name)).unwrap();
        if stretch {
            let (w, h) = img.dimensions();
            let crop = image::imageops::crop_imm(&img, w / 4, h / 4, w / 2, h / 2).to_image();
            let mean: f32 = crop.as_raw().iter().map(|&v| v as f32).sum::<f32>() / crop.as_raw().len() as f32;
            let stretched = image::RgbImage::from_fn(crop.width(), crop.height(), |x, y| image::Rgb(crop.get_pixel(x, y).0.map(|v| ((v as f32 - mean) * STRETCH + 128.0).clamp(0.0, 255.0) as u8)));
            stretched.save(dir.join(format!("stretch-{name}"))).unwrap();
        }
        println!("{name:<32} {ms:8.1} ms  palette={:?}", r.palette);
    }
}
