//! Composes rendered samples into one 4 × 3 contact sheet for review:
//!
//! ```text
//! cargo run --release -p mandara-wallgen --example sheet -- <suffix>   # e.g. "-light" or "-r3"
//! ```
//!
//! Reads `<style><suffix>.png` from `target/wallgen-samples/` (or
//! `$MANDARA_WALLGEN_SAMPLES`) and writes `sheet<suffix>.png` there.

use mandara_wallgen::Style;

/// Columns of the sheet and the size of each cell.
const COLUMNS: u32 = 4;
const CELL: (u32, u32) = (480, 270);

fn main() {
    let suffix = std::env::args().nth(1).unwrap_or_default();
    let dir = std::env::var_os("MANDARA_WALLGEN_SAMPLES").map(std::path::PathBuf::from).unwrap_or_else(|| std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/wallgen-samples"));
    let rows = (Style::ALL.len() as u32).div_ceil(COLUMNS);
    let mut sheet = image::RgbImage::new(CELL.0 * COLUMNS, CELL.1 * rows);
    for (i, s) in Style::ALL.iter().enumerate() {
        let Ok(img) = image::open(dir.join(format!("{}{suffix}.png", s.id()))) else { continue };
        let cell = image::imageops::resize(&img.to_rgb8(), CELL.0, CELL.1, image::imageops::FilterType::Triangle);
        image::imageops::replace(&mut sheet, &cell, (i as u32 % COLUMNS * CELL.0) as i64, (i as u32 / COLUMNS * CELL.1) as i64);
    }
    sheet.save(dir.join(format!("sheet{suffix}.png"))).unwrap();
}
