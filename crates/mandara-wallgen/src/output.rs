//! Encoding and files: formats, atomic writes, the recipe sidecar that
//! marks a file as generated (and lets it be re-rendered or remixed), and
//! pruning of old generated files.
//!
//! Sidecars live next to their image as `.<file name>.recipe.toml`
//! (hidden, so wallpaper pickers don't list them). Only files with a
//! sidecar are ever pruned.

use std::io::Write;
use std::path::{Path, PathBuf};

use image::RgbImage;
use mandara_i18n::{tf, tk};
use serde::{Deserialize, Serialize};

use crate::recipe::Recipe;
use crate::resolution::Resolution;

/// Default JPEG quality: visually lossless for smooth gradients.
pub const DEFAULT_JPEG_QUALITY: u8 = 92;
/// Sidecar file name: `.` + image file name + this suffix.
const SIDECAR_SUFFIX: &str = ".recipe.toml";
/// Identifies this engine in sidecars.
const ENGINE: &str = concat!("mandara-wallgen ", env!("CARGO_PKG_VERSION"));

/// Image file format.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Format {
    /// Exact pixels in the smallest file: encodes lossless WebP and PNG
    /// side by side and keeps the smaller (which one wins depends on the
    /// picture). [`encode_resolved`] says which it was.
    #[default]
    Lossless,
    /// Lossless WebP: exact pixels.
    WebpLossless,
    /// PNG at maximum compression with adaptive filtering.
    Png,
    /// Baseline JPEG at `quality` (1–100).
    Jpeg { quality: u8 },
}

impl Format {
    pub const ALL: [Format; 4] = [Format::Lossless, Format::WebpLossless, Format::Png, Format::Jpeg { quality: DEFAULT_JPEG_QUALITY }];

    /// File extension. [`Format::Lossless`] is only known after encoding
    /// (see [`encode_resolved`]); before, it names the usual winner, WebP.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Lossless | Format::WebpLossless => "webp",
            Format::Png => "png",
            Format::Jpeg { .. } => "jpg",
        }
    }

    /// Stable id: `lossless`, `webp`, `png`, `jpeg` or `jpeg-<quality>`.
    pub fn id(self) -> String {
        match self {
            Format::Lossless => "lossless".into(),
            Format::WebpLossless => "webp".into(),
            Format::Png => "png".into(),
            Format::Jpeg { quality } if quality == DEFAULT_JPEG_QUALITY => "jpeg".into(),
            Format::Jpeg { quality } => format!("jpeg-{quality}"),
        }
    }

    pub fn from_id(id: &str) -> Option<Format> {
        match id {
            "lossless" => Some(Format::Lossless),
            "webp" => Some(Format::WebpLossless),
            "png" => Some(Format::Png),
            "jpeg" | "jpg" => Some(Format::Jpeg { quality: DEFAULT_JPEG_QUALITY }),
            _ => {
                let q: u8 = id.strip_prefix("jpeg-").or_else(|| id.strip_prefix("jpg-"))?.parse().ok()?;
                (1..=100).contains(&q).then_some(Format::Jpeg { quality: q })
            }
        }
    }

    /// English label, marked for translation.
    pub fn label(self) -> &'static str {
        match self {
            Format::Lossless => tk("Lossless, smallest"),
            Format::WebpLossless => tk("WebP (lossless)"),
            Format::Png => tk("PNG (lossless)"),
            Format::Jpeg { .. } => tk("JPEG"),
        }
    }

    /// Whether decoding gives back exactly the rendered pixels.
    pub fn is_lossless(self) -> bool {
        !matches!(self, Format::Jpeg { .. })
    }
}

/// Where and how [`crate::save`] writes.
#[derive(Clone, Debug, PartialEq)]
pub struct OutputOptions {
    pub resolution: Resolution,
    pub format: Format,
    pub dir: PathBuf,
    /// File stem; default `<style>-<short seed>-<W>x<H>`. A clash gets
    /// `-2`, `-3`, … appended.
    pub name: Option<String>,
}

/// A written wallpaper (image or video).
#[derive(Clone, Debug, PartialEq)]
pub struct Generated {
    pub path: PathBuf,
    /// The recipe sidecar.
    pub recipe_path: PathBuf,
    pub width: u32,
    pub height: u32,
    /// Size of the written file.
    pub bytes: u64,
}

/// What a sidecar records besides the recipe.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SidecarMeta {
    pub engine: String,
    pub file: String,
    pub width: u32,
    pub height: u32,
    /// `"image"` or `"video"`.
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct Sidecar {
    pub generated: SidecarMeta,
    pub recipe: Recipe,
}

/// Encodes an image.
pub fn encode(img: &RgbImage, format: Format) -> Result<Vec<u8>, String> {
    encode_resolved(img, format).map(|(bytes, _)| bytes)
}

/// Encodes an image and says which concrete format it is in: the format
/// asked for, or for [`Format::Lossless`] whichever of lossless WebP and
/// PNG came out smaller (both encode at once, on two threads; a tie keeps
/// WebP).
pub fn encode_resolved(img: &RgbImage, format: Format) -> Result<(Vec<u8>, Format), String> {
    if format != Format::Lossless {
        return encode_one(img, format).map(|bytes| (bytes, format));
    }
    let (webp, png) = std::thread::scope(|s| {
        let png = s.spawn(|| encode_one(img, Format::Png));
        let webp = encode_one(img, Format::WebpLossless);
        (webp, png.join().unwrap_or_else(|panic| std::panic::resume_unwind(panic)))
    });
    match (webp, png) {
        (Ok(w), Ok(p)) if p.len() < w.len() => Ok((p, Format::Png)),
        (Ok(w), _) => Ok((w, Format::WebpLossless)),
        (Err(_), Ok(p)) => Ok((p, Format::Png)),
        (Err(e), Err(_)) => Err(e),
    }
}

/// Encodes in one concrete format.
fn encode_one(img: &RgbImage, format: Format) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    let err = |e: image::ImageError| tf("Could not encode the wallpaper: {error}", &[("error", &e.to_string())]);
    match format {
        Format::Lossless => return encode_resolved(img, format).map(|(bytes, _)| bytes),
        Format::WebpLossless => {
            out = crate::webp::encode_rgb(img.as_raw(), img.width(), img.height())?;
        }
        Format::Png => {
            use image::ImageEncoder;
            image::codecs::png::PngEncoder::new_with_quality(&mut out, image::codecs::png::CompressionType::Best, image::codecs::png::FilterType::Adaptive)
                .write_image(img.as_raw(), img.width(), img.height(), image::ExtendedColorType::Rgb8)
                .map_err(err)?;
        }
        Format::Jpeg { quality } => {
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, quality.clamp(1, 100)).encode_image(img).map_err(err)?;
        }
    }
    Ok(out)
}

/// The sidecar path of a generated file.
pub fn sidecar_path(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!(".{name}{SIDECAR_SUFFIX}"))
}

fn read_sidecar(path: &Path) -> Option<Sidecar> {
    let text = std::fs::read_to_string(sidecar_path(path)).ok()?;
    let mut s: Sidecar = toml::from_str(&text).ok()?;
    s.recipe.sanitize();
    (s.generated.file == path.file_name()?.to_string_lossy()).then_some(s)
}

/// The recipe a generated file was made from (`None` for any other file).
pub fn recipe_for(path: &Path) -> Option<Recipe> {
    read_sidecar(path).map(|s| s.recipe)
}

/// Whether this engine wrote `path` (it has a matching sidecar).
pub fn is_generated(path: &Path) -> bool {
    read_sidecar(path).is_some()
}

/// Writes `bytes` to `path` through a temporary file in the same
/// directory and a rename, so a reader never sees a partial file.
pub(crate) fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let tmp = temp_path(path);
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// A hidden temporary name next to `path`, unique per process.
pub(crate) fn temp_path(path: &Path) -> PathBuf {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!(".{name}.tmp-{}", std::process::id()))
}

/// Only letters, digits, `-`, `_` and `.` survive in a file stem.
fn clean_stem(s: &str) -> String {
    let s: String = s.chars().map(|c| if c.is_alphanumeric() || matches!(c, '-' | '_' | '.') { c } else { '-' }).collect();
    let s = s.trim_matches(|c| c == '.' || c == '-').to_string();
    if s.is_empty() { "wallpaper".into() } else { s }
}

/// A free path `dir/<stem>.<ext>`, suffixing `-2`, `-3`, … on clashes.
pub(crate) fn unique_path(dir: &Path, stem: &str, ext: &str) -> PathBuf {
    let stem = clean_stem(stem);
    let first = dir.join(format!("{stem}.{ext}"));
    if !first.exists() {
        return first;
    }
    (2..).map(|i| dir.join(format!("{stem}-{i}.{ext}"))).find(|p| !p.exists()).unwrap_or(first)
}

/// The default file stem for a recipe at a size.
pub(crate) fn default_stem(recipe: &Recipe, width: u32, height: u32) -> String {
    format!("{}-{}-{width}x{height}", recipe.style.id(), recipe.short_seed())
}

/// Writes the sidecar for a generated file.
pub(crate) fn write_sidecar(path: &Path, recipe: &Recipe, width: u32, height: u32, kind: &str) -> Result<PathBuf, String> {
    let sidecar = Sidecar {
        generated: SidecarMeta { engine: ENGINE.into(), file: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), width, height, kind: kind.into() },
        recipe: recipe.clone(),
    };
    let text = toml::to_string_pretty(&sidecar).map_err(|e| e.to_string())?;
    let sp = sidecar_path(path);
    write_atomic(&sp, text.as_bytes()).map_err(|e| io_error(&sp, &e))?;
    Ok(sp)
}

pub(crate) fn io_error(path: &Path, e: &std::io::Error) -> String {
    tf("Could not write {path}: {error}", &[("path", &path.display().to_string()), ("error", &e.to_string())])
}

/// Deletes the oldest generated files in `dir` beyond the newest `keep`
/// (by modification time), with their sidecars, plus sidecars whose file
/// is gone. Files without a sidecar are never touched. Returns the
/// removed wallpaper paths.
pub fn prune(dir: &Path, keep: usize) -> Result<Vec<PathBuf>, String> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_error(dir, &e)),
    };
    let mut generated: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(file) = name.strip_prefix('.').and_then(|n| n.strip_suffix(SIDECAR_SUFFIX)) else { continue };
        let path = dir.join(file);
        if read_sidecar(&path).is_none() {
            // Not ours, or our sidecar for a file that's gone.
            if !path.exists() && toml::from_str::<Sidecar>(&std::fs::read_to_string(entry.path()).unwrap_or_default()).is_ok() {
                let _ = std::fs::remove_file(entry.path());
            }
            continue;
        }
        let modified = std::fs::metadata(&path).and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        generated.push((modified, path));
    }
    // Newest first; ties broken by name so the order is stable.
    generated.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    let mut removed = Vec::new();
    for (_, path) in generated.into_iter().skip(keep) {
        std::fs::remove_file(&path).map_err(|e| io_error(&path, &e))?;
        let _ = std::fs::remove_file(sidecar_path(&path));
        removed.push(path);
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_ids() {
        for f in Format::ALL {
            assert_eq!(Format::from_id(&f.id()), Some(f));
        }
        assert_eq!(Format::from_id("jpeg-80"), Some(Format::Jpeg { quality: 80 }));
        assert_eq!(Format::from_id("jpeg-0"), None);
        assert_eq!(Format::from_id("gif"), None);
        assert!(Format::WebpLossless.is_lossless() && !Format::Jpeg { quality: 90 }.is_lossless());
        assert!(Format::Lossless.is_lossless());
        assert_eq!(Format::default(), Format::Lossless);
        assert_eq!(Format::from_id("lossless"), Some(Format::Lossless));
    }

    #[test]
    fn lossless_keeps_the_smaller_exact_file() {
        // Two pictures that favour different codecs: a smooth gradient and
        // hashed noise.
        let smooth = RgbImage::from_fn(96, 64, |x, y| image::Rgb([(x * 2) as u8, (y * 3) as u8, 128]));
        let noisy = RgbImage::from_fn(96, 64, |x, y| {
            let h = crate::noise::hash3(x as i32, y as i32, 0, 7);
            image::Rgb([h as u8, (h >> 8) as u8, (h >> 16) as u8])
        });
        for img in [smooth, noisy] {
            let (bytes, chosen) = encode_resolved(&img, Format::Lossless).unwrap();
            let webp = encode(&img, Format::WebpLossless).unwrap();
            let png = encode(&img, Format::Png).unwrap();
            assert_eq!(bytes.len(), webp.len().min(png.len()), "not the smaller of the two");
            assert_eq!(chosen, if png.len() < webp.len() { Format::Png } else { Format::WebpLossless });
            assert_eq!(bytes, encode(&img, Format::Lossless).unwrap());
            assert_eq!(image::load_from_memory(&bytes).unwrap().to_rgb8(), img, "not exact");
        }
    }

    #[test]
    fn stems_are_clean_and_unique() {
        assert_eq!(clean_stem("../evil/name"), "evil-name");
        assert_eq!(clean_stem("..."), "wallpaper");
        let dir = tempfile::tempdir().unwrap();
        let a = unique_path(dir.path(), "x", "webp");
        std::fs::write(&a, b"1").unwrap();
        let b = unique_path(dir.path(), "x", "webp");
        assert_eq!(b.file_name().unwrap(), "x-2.webp");
        assert_eq!(sidecar_path(&a).file_name().unwrap(), ".x.webp.recipe.toml");
    }
}
