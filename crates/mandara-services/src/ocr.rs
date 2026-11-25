//! Text recognition for screenshots ("copy text"), pure Rust: PaddleOCR
//! PP-OCRv5 detection + recognition run by `pure-onnx-ocr` (tract). Latin
//! script covers English, Spanish, French and most European languages;
//! Arabic has its own model. In `Auto` both recognizers read every detected
//! line and the more confident reading wins, so mixed Arabic/English
//! screenshots come out right.
//!
//! Engines are kept warm for two minutes after use, then dropped to give
//! the memory back.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use mandara_i18n::{t, tf, tr};
use pure_onnx_ocr::{DetLimitType, OcrEngine, OcrEngineBuilder, OcrResult};

use crate::models::{self, Model};

/// How often idle OCR engines are checked for release (memory housekeeping).
const REAP_EVERY: Duration = Duration::from_secs(30);

/// Which script(s) to read.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum Script {
    #[default]
    Auto,
    Latin,
    Arabic,
}

impl Script {
    pub fn parse(s: &str) -> Self {
        match s {
            "latin" | "en" | "es" | "fr" => Script::Latin,
            "arabic" | "ar" => Script::Arabic,
            _ => Script::Auto,
        }
    }

    fn recognizers(self) -> &'static [&'static str] {
        match self {
            Script::Auto => &["ocr-latin", "ocr-arabic"],
            Script::Latin => &["ocr-latin"],
            Script::Arabic => &["ocr-arabic"],
        }
    }
}

/// Models `script` needs that aren't installed yet.
pub fn missing_models(script: Script) -> Vec<&'static Model> {
    std::iter::once("ocr-det").chain(script.recognizers().iter().copied()).filter_map(models::find).filter(|m| !m.installed()).collect()
}

const IDLE: Duration = Duration::from_secs(120);

struct Warm {
    engines: HashMap<&'static str, Arc<OcrEngine>>,
    used: Instant,
}

fn warm() -> &'static Mutex<Option<Warm>> {
    static WARM: OnceLock<Mutex<Option<Warm>>> = OnceLock::new();
    WARM.get_or_init(|| {
        // Reaper: frees idle engines.
        std::thread::Builder::new()
            .name("mandara-ocr-reaper".into())
            .spawn(|| loop {
                std::thread::sleep(REAP_EVERY);
                let mut w = warm().lock().unwrap_or_else(|e| e.into_inner());
                if w.as_ref().is_some_and(|w| w.used.elapsed() > IDLE) {
                    *w = None;
                    tracing::debug!("ocr engines released");
                }
            })
            .ok();
        Mutex::new(None)
    })
}

fn engine(rec: &'static str) -> Result<Arc<OcrEngine>, String> {
    let mut guard = warm().lock().unwrap_or_else(|e| e.into_inner());
    let w = guard.get_or_insert_with(|| Warm { engines: HashMap::new(), used: Instant::now() });
    w.used = Instant::now();
    if let Some(e) = w.engines.get(rec) {
        return Ok(e.clone());
    }
    let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).min(8);
    let built = OcrEngineBuilder::new()
        .det_model_dir(models::dir("ocr-det"))
        .rec_model_dir(models::dir(rec))
        // Screenshot text is small: detect near native resolution, capped.
        .det_limit_type(DetLimitType::Max)
        .det_limit_side_len(2560)
        .inference_threads(threads)
        .build()
        .map_err(|e| tf("Could not load the OCR models: {error}", &[("error", &e)]))?;
    let built = Arc::new(built);
    w.engines.insert(rec, built.clone());
    Ok(built)
}

/// A recognized line in image pixels.
#[derive(Clone, Debug)]
struct Line {
    text: String,
    confidence: f32,
    x0: f64,
    x1: f64,
    y0: f64,
    y1: f64,
    arabic: bool,
}

fn has_arabic(s: &str) -> bool {
    s.chars().any(|c| matches!(c as u32, 0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF))
}

/// The Arabic model emits characters in visual (left-to-right) order.
/// PaddleOCR's `pred_reverse`: runs of Latin letters, digits and a few
/// symbols stay intact; everything else is reversed into logical order.
pub fn arabic_logical(visual: &str) -> String {
    let keep = |c: char| c.is_ascii_alphanumeric() || " :*./%+-".contains(c);
    let mut parts: Vec<String> = Vec::new();
    let mut run = String::new();
    for c in visual.chars() {
        if keep(c) {
            run.push(c);
        } else {
            if !run.is_empty() {
                parts.push(std::mem::take(&mut run));
            }
            parts.push(c.to_string());
        }
    }
    if !run.is_empty() {
        parts.push(run);
    }
    parts.reverse();
    parts.concat()
}

fn to_lines(results: Vec<OcrResult>, arabic_model: bool) -> Vec<Line> {
    results
        .into_iter()
        .filter(|r| !r.text.trim().is_empty())
        .map(|r| {
            let pts = &r.bounding_box.exterior().0;
            let (mut x0, mut x1, mut y0, mut y1) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
            for p in pts {
                x0 = x0.min(p.x);
                x1 = x1.max(p.x);
                y0 = y0.min(p.y);
                y1 = y1.max(p.y);
            }
            let text = if arabic_model && has_arabic(&r.text) { arabic_logical(&r.text) } else { r.text.clone() };
            Line { arabic: has_arabic(&text), text: text.trim().to_string(), confidence: r.confidence, x0, x1, y0, y1 }
        })
        .collect()
}

fn overlap(a: &Line, b: &Line) -> f64 {
    let w = (a.x1.min(b.x1) - a.x0.max(b.x0)).max(0.0);
    let h = (a.y1.min(b.y1) - a.y0.max(b.y0)).max(0.0);
    let inter = w * h;
    let union = (a.x1 - a.x0) * (a.y1 - a.y0) + (b.x1 - b.x0) * (b.y1 - b.y0) - inter;
    if union > 0.0 { inter / union } else { 0.0 }
}

/// Per detected box, keeps the most confident reading across recognizers.
fn merge(mut primary: Vec<Line>, others: Vec<Line>) -> Vec<Line> {
    for o in others {
        match primary.iter_mut().max_by(|a, b| overlap(a, &o).total_cmp(&overlap(b, &o))).filter(|p| overlap(p, &o) > 0.5) {
            Some(p) if o.confidence > p.confidence => *p = o,
            Some(_) => {}
            None => primary.push(o),
        }
    }
    primary
}

/// Lines in reading order: rows top to bottom; within a row left to right,
/// or right to left when the row is mostly Arabic.
fn assemble(mut lines: Vec<Line>) -> String {
    lines.sort_by(|a, b| (a.y0 + a.y1).total_cmp(&(b.y0 + b.y1)));
    let mut rows: Vec<Vec<Line>> = Vec::new();
    for l in lines {
        let cy = (l.y0 + l.y1) / 2.0;
        match rows.last_mut() {
            Some(row) if {
                let r = &row[0];
                let rh = (r.y1 - r.y0).min(l.y1 - l.y0);
                ((r.y0 + r.y1) / 2.0 - cy).abs() < rh * 0.5
            } => row.push(l),
            _ => rows.push(vec![l]),
        }
    }
    rows.into_iter()
        .map(|mut row| {
            let rtl = row.iter().filter(|l| l.arabic).count() * 2 > row.len();
            row.sort_by(|a, b| if rtl { b.x0.total_cmp(&a.x0) } else { a.x0.total_cmp(&b.x0) });
            row.into_iter().map(|l| l.text).collect::<Vec<_>>().join("  ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Reads the text in `img`. Fails with a clear message when models are
/// missing (Studio → Maintenance downloads them).
pub fn recognize(img: &image::RgbaImage, script: Script) -> Result<String, String> {
    let missing = missing_models(script);
    if !missing.is_empty() {
        return Err(tf(
            "Text recognition models aren't installed yet ({models}) — download them in Studio → Maintenance",
            &[("models", &missing.iter().map(|m| tr(m.title)).collect::<Vec<_>>().join(", "))],
        ));
    }
    let mut rgb = image::DynamicImage::ImageRgba8(img.clone()).to_rgb8();
    // Tiny selections read better upscaled.
    if rgb.height() < 48 {
        let s = 48.0 / rgb.height().max(1) as f32;
        rgb = image::imageops::resize(&rgb, (rgb.width() as f32 * s) as u32, 48, image::imageops::FilterType::CatmullRom);
    }
    let dynamic = image::DynamicImage::ImageRgb8(rgb);
    let mut merged: Option<Vec<Line>> = None;
    for rec in script.recognizers() {
        let results = engine(rec)?.run_from_image(&dynamic).map_err(|e| tf("Text recognition failed: {error}", &[("error", &e)]))?;
        let lines = to_lines(results, *rec == "ocr-arabic");
        merged = Some(match merged {
            None => lines,
            Some(prev) => merge(prev, lines),
        });
    }
    let text = assemble(merged.unwrap_or_default());
    if text.trim().is_empty() { Err(t("No text found in the selection").into()) } else { Ok(text) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, x0: f64, y0: f64, conf: f32) -> Line {
        Line { text: text.into(), confidence: conf, x0, x1: x0 + 100.0, y0, y1: y0 + 20.0, arabic: has_arabic(text) }
    }

    #[test]
    fn arabic_visual_to_logical() {
        // Visual "مرحبا" reversed, with a Latin run kept intact.
        assert_eq!(arabic_logical("ابحرم"), "مرحبا");
        // Latin runs (spaces included) move as one block, as in PaddleOCR.
        assert_eq!(arabic_logical("ABC 12 مس"), "سمABC 12 ");
    }

    #[test]
    fn rows_and_rtl_order() {
        let text = assemble(vec![line("world", 120.0, 0.0, 0.9), line("hello", 0.0, 2.0, 0.9), line("سلام", 0.0, 40.0, 0.9), line("مرحبا", 120.0, 41.0, 0.9)]);
        assert_eq!(text, "hello  world\nمرحبا  سلام");
    }

    #[test]
    fn merge_keeps_confident_reading() {
        let latin = vec![line("Hcllo", 0.0, 0.0, 0.4), line("Bonjour", 0.0, 40.0, 0.95)];
        let arabic = vec![line("مرحبا", 2.0, 1.0, 0.9), line("ب", 0.0, 40.0, 0.2)];
        let out = merge(latin, arabic);
        assert_eq!(out[0].text, "مرحبا");
        assert_eq!(out[1].text, "Bonjour");
    }
}
