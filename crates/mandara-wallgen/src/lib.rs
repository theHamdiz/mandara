//! Mandara's procedural wallpaper engine.
//!
//! A [`Recipe`] (style, seed, palette, parameters, light or dark) fully
//! determines a wallpaper. [`render`] turns it into pixels at any size:
//! deterministic (same recipe and size, same pixels on every machine) and
//! resolution-independent (a 1080p and an 8K render are the same picture).
//! [`save`] writes it as compact lossless WebP (or PNG / JPEG) with a
//! hidden recipe sidecar so it can be re-rendered or remixed later, and
//! [`render_video`] writes a seamless AV1 loop.
//!
//! * [`Style`]: twelve styles, each with its own [`ParamSpec`] table, plus
//!   [`COMMON_PARAMS`] (tone, color, finish, framing, motion) shared by all.
//! * [`PaletteSpec`]: curated [`palettes`], custom hex stops or the live
//!   shell theme; every blend happens in OKLab.
//! * Scheduling helpers: [`next_due`] and [`prune`].
//!
//! The engine has no UI dependency; the wallpaper-generator plugin and
//! Studio build on it.

mod color;
mod dither;
mod fmath;
mod noise;
mod output;
mod palette;
mod params;
mod recipe;
mod render;
mod resolution;
mod schedule;
mod style;
mod styles;
mod video;
mod webp;

use image::RgbImage;

pub use output::{DEFAULT_JPEG_QUALITY, Format, Generated, OutputOptions, encode, encode_resolved, is_generated, prune, recipe_for, sidecar_path};
pub use palette::{DEFAULT_PALETTE, MAX_STOPS, MIN_STOPS, NamedPalette, PALETTES, PaletteSpec, named_palette, palettes, parse_hex};
pub use params::{COMMON_PARAMS, ParamSpec};
pub use recipe::{RECIPE_VERSION, Recipe};
pub use resolution::Resolution;
pub use schedule::{is_due, next_due};
pub use style::Style;
pub use video::{VideoOptions, render_video};

/// Renders `recipe` at `width × height` (each at least 1).
pub fn render(recipe: &Recipe, width: u32, height: u32) -> RgbImage {
    render::render_rgb8(recipe, width, height, 0.0, false)
}

/// A fast render for live previews (≈320 × 180 in milliseconds). It is
/// the same picture as [`render`], skipping refinements (edge
/// supersampling) that only show at full size.
pub fn preview(recipe: &Recipe, width: u32, height: u32) -> RgbImage {
    render::render_rgb8(recipe, width, height, 0.0, true)
}

/// Frame of the recipe's seamless loop at phase `t ∈ [0, 1)`;
/// `render_frame(r, w, h, 0.0) == render(r, w, h)` and `t = 1` equals `t = 0`.
pub fn render_frame(recipe: &Recipe, width: u32, height: u32, t: f32) -> RgbImage {
    render::render_rgb8(recipe, width, height, t, false)
}

/// Renders `recipe` at `options.resolution` and writes it to
/// `options.dir` (atomically), with its recipe sidecar.
pub fn save(recipe: &Recipe, options: &OutputOptions) -> Result<Generated, String> {
    let (width, height) = options.resolution.size();
    let img = render(recipe, width, height);
    // `Lossless` resolves to WebP or PNG here, and names the file by it.
    let (bytes, format) = encode_resolved(&img, options.format)?;
    drop(img);
    let stem = options.name.clone().unwrap_or_else(|| output::default_stem(recipe, width, height));
    let path = output::unique_path(&options.dir, &stem, format.extension());
    output::write_atomic(&path, &bytes).map_err(|e| output::io_error(&path, &e))?;
    let recipe_path = output::write_sidecar(&path, recipe, width, height, "image")?;
    Ok(Generated { path, recipe_path, width, height, bytes: bytes.len() as u64 })
}
