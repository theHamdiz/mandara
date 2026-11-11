//! The GPU wallpaper's Sketch transition: the new wallpaper's outline is
//! drawn first as strokes revealed along their length (its subject first
//! when Mandara has a depth mask for it), then the image fills in (a fade,
//! a watercolour bleed outward from the lines, or a wipe that follows the
//! strokes), and the lines fade away (or stay as a faint overlay).
//!
//! The outline ([`outline`]) is traced once per wallpaper and size, in the
//! same background job that prepares the new image's display copy
//! ([`for_wallpaper`]): from a copy at most 320 px long, made from the
//! image that job decodes anyway, and dropped as soon as the strokes
//! exist. A wallpaper's outline is kept in a small cache ([`CACHE_ENTRIES`]
//! outlines of ≤ 64 KiB each) keyed by the file (path, size, modification
//! time), the shown copy, the fill and the mask used, so changing back and
//! forth traces nothing again. The transition starts when that job
//! finishes, so it never waits for its outline.
//!
//! Each frame is a pure function of the progress: strokes are stroked
//! paths up to their revealed length, the fill is clips of the new image's
//! layer (the same texture, no copies). At progress 1 only the new image
//! is left (and, when [`SketchOptions::keep_outline`] is on, the outline
//! at [`KEEP_ALPHA`], which is static: it never asks for frames).
//!
//! The software wallpaper (the default renderer) draws Sketch as a fade
//! (`WallpaperTransition::software`), and reduced motion makes it a plain
//! fade here too.

pub mod outline;

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use gpui::{AnyElement, Bounds, Hsla, IntoElement, ParentElement, Pixels, Styled, canvas, div, point, px};
use mandara_config::desktop::{SketchFill, SketchInk, SketchOptions, SketchOrder, WallpaperFill};
use mandara_core::ToGpui as _;
use mandara_theme::Color;

pub use outline::Outline;
use outline::GRID;

/// Outlines kept for wallpapers changed back to.
pub const CACHE_ENTRIES: usize = 4;
/// Alpha of the outline left over the wallpaper ([`SketchOptions::keep_outline`]).
pub const KEEP_ALPHA: f32 = 0.16;
/// A stroke draws over this share of the drawing phase per share of the
/// outline's length it holds, within these bounds.
const SPAN_PER_LENGTH: f32 = 3.0;
const SPAN_BOUNDS: (f32, f32) = (0.12, 0.6);
/// The glow under the lines: width (× the line) and alpha.
const GLOW_WIDTH: f32 = 4.0;
const GLOW_ALPHA: f32 = 0.3;
/// The watercolour bleed: how far its pigment spreads (× the line) and
/// how strong it starts.
const BLEED_WIDTH: f32 = 14.0;
const BLEED_ALPHA: f32 = 0.22;
/// The wash over the old image while the lines are drawn: its alpha and
/// the share of the drawing phase it takes to settle.
const WASH_ALPHA: f32 = 0.55;
pub const WASH_RISE: f32 = 0.3;
/// The fill reveals cells this much of its phase apart (nearest the lines
/// first for the bleed, in drawing order for the wipe); each cell takes
/// the rest.
const BLEED_SPREAD: f32 = 0.45;
const WIPE_SPREAD: f32 = 0.5;
/// Under the cells the whole new image rises to this alpha over the fill,
/// so neighbouring cells differ by little.
const BASE_SHARE: f32 = 0.5;
/// Points stroked per path (each path's tessellation is indexed by u16).
const CHUNK_POINTS: usize = 2048;
/// Line colours of the Ink and White choices, and the washes under them.
const INK_BLACK: (u8, u8, u8) = (0x1b, 0x1a, 0x1f);
const PAPER: (u8, u8, u8) = (0xf3, 0xee, 0xe4);
const NIGHT: (u8, u8, u8) = (0x0a, 0x0c, 0x10);
/// Lines darker than this (OK lightness) get the paper wash.
const DARK_LINE: f32 = 0.5;

// ── Preparation (background) ─────────────────────────────────────────────

/// What a wallpaper change asks for, read on the UI thread when it starts.
#[derive(Clone, Debug)]
pub struct Want {
    pub fill: WallpaperFill,
    /// The depth effect's mask request (`None` with the effect off).
    pub depth: Option<crate::depth::Request>,
}

impl Want {
    /// The outline a change under `w` needs, if any: Sketch is the
    /// transition and the image is drawn once (a tiled one is faded).
    pub fn of(w: &mandara_config::WallpaperConfig) -> Option<Self> {
        (w.transition == mandara_config::desktop::WallpaperTransition::Sketch && w.fill != WallpaperFill::Tile).then(|| Self { fill: w.fill, depth: crate::depth::Request::of(w) })
    }
}

/// A file's identity: path, length and modification time.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp(PathBuf, u64, Option<SystemTime>);

impl Stamp {
    fn of(path: &Path) -> Self {
        let meta = std::fs::metadata(path).ok();
        Self(path.to_path_buf(), meta.as_ref().map_or(0, |m| m.len()), meta.and_then(|m| m.modified().ok()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    source: Stamp,
    shown: Stamp,
    size: (u32, u32),
    fill: WallpaperFill,
    mask: Option<Stamp>,
}

static CACHE: Mutex<VecDeque<(Key, Arc<Outline>)>> = Mutex::new(VecDeque::new());

fn cached(key: &Key) -> Option<Arc<Outline>> {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    let at = cache.iter().position(|(k, _)| k == key)?;
    // Most recently used last.
    let entry = cache.remove(at)?;
    let outline = entry.1.clone();
    cache.push_back(entry);
    Some(outline)
}

fn remember(key: Key, outline: Arc<Outline>) {
    let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    cache.retain(|(k, _)| *k != key);
    while cache.len() >= CACHE_ENTRIES {
        cache.pop_front();
    }
    cache.push_back((key, outline));
}

/// The subject mask Mandara already has for `source`: yours, the one the
/// depth effect's current mode made, or one the hand-tuned mode made
/// before. Nothing is generated here (the depth effect does that, once,
/// after the wallpaper is shown), so a wallpaper's first change is
/// outlined from its edges alone.
fn existing_mask(source: &Path, depth: Option<crate::depth::Request>) -> Option<PathBuf> {
    use crate::depth::{Request, generated_path, user_mask_path};
    if let Some(user) = user_mask_path(source).filter(|p| p.is_file()) {
        return Some(user);
    }
    let hand = Request { mode: mandara_config::DepthMode::HandTuned, split: None };
    depth.into_iter().chain([hand]).map(|r| generated_path(source, r)).find(|p| p.is_file())
}

/// `mask` (any size) as subject alpha aligned to a `size` sample of the
/// shown image: centre-cropped like a cover display copy (`cropped`), else
/// stretched. Shrunk first, so only the decoded mask is ever large.
fn load_mask(mask: &Path, size: (u32, u32), cropped: bool) -> Option<image::GrayImage> {
    let img = image::open(mask).map_err(|e| tracing::debug!("sketch mask {}: {e}", mask.display())).ok()?;
    let alpha = img.color().has_alpha();
    let small = if cropped { crate::wallpaper::cover(&img, size.0, size.1) } else { crate::wallpaper::shrink(&img, size.0, size.1) };
    drop(img);
    // Like the depth effect's masks: real transparency, else a matte.
    if alpha {
        let rgba = small.to_rgba8();
        if rgba.pixels().any(|p| p.0[3] < 250) {
            let (w, h) = rgba.dimensions();
            let raw: Vec<u8> = rgba.as_raw().as_chunks::<4>().0.iter().map(|p| p[3]).collect();
            return image::GrayImage::from_raw(w, h, raw);
        }
    }
    Some(small.to_luma8())
}

/// The outline of wallpaper `source`, shown as `shown` (`decoded`, consumed
/// and dropped as soon as the sample exists), from the cache or traced now.
/// Blocking: run on the background job that prepared `shown`. `tone` is
/// the job's small colour copy (the dominant colour comes from it).
pub fn for_wallpaper(want: &Want, source: &Path, shown: &Path, decoded: image::DynamicImage, tone: Option<&image::RgbImage>) -> Option<Arc<Outline>> {
    let size = outline::sample_size(decoded.width(), decoded.height());
    let cropped = shown != source;
    let mask = existing_mask(source, want.depth).filter(|_| match want.fill {
        WallpaperFill::Cover | WallpaperFill::Contain | WallpaperFill::Stretch => true,
        // A centre crop of a larger picture no longer lines up with its mask.
        WallpaperFill::Center => !cropped,
        WallpaperFill::Tile => false,
    });
    let key = Key { source: Stamp::of(source), shown: Stamp::of(shown), size, fill: want.fill, mask: mask.as_deref().map(Stamp::of) };
    if let Some(hit) = cached(&key) {
        return Some(hit);
    }
    let luma = crate::wallpaper::shrink(&decoded, size.0, size.1).to_luma8();
    drop(decoded);
    let alpha = mask.as_deref().and_then(|m| load_mask(m, size, cropped && want.fill == WallpaperFill::Cover));
    let started = std::time::Instant::now();
    let traced = Arc::new(outline::extract(&luma, alpha.as_ref(), tone));
    tracing::debug!("sketch outline of {}: {} strokes, {} points, mask {}, {:?}", source.display(), traced.runs.len(), traced.points.len(), alpha.is_some(), started.elapsed());
    drop((luma, alpha));
    remember(key, traced.clone());
    Some(traced)
}

// ── Plan (once per transition, UI thread) ────────────────────────────────

/// When each stroke draws and which way, and when each fill cell of the
/// wipe opens: fixed for one transition.
#[derive(Debug, PartialEq)]
pub struct Plan {
    /// Start and length of each stroke within the drawing phase (`0..1`).
    start: Box<[f32]>,
    span: Box<[f32]>,
    /// Drawn from its last point to its first.
    reverse: Box<[bool]>,
    /// The bleed's and the wipe's delay per [`GRID`] cell (`0..1`),
    /// softened across neighbours so the cells blend.
    bleed: Box<[f32]>,
    wipe: Box<[f32]>,
}

/// `delays` (a [`GRID`]) averaged with their 8 neighbours, then rescaled to
/// `0..1`.
fn soften(delays: &[f32]) -> Box<[f32]> {
    let (gx, gy) = (GRID.0 as i32, GRID.1 as i32);
    if delays.len() != (gx * gy) as usize {
        return delays.into();
    }
    let mut out: Box<[f32]> = (0..gx * gy)
        .map(|i| {
            let (r, c) = (i / gx, i % gx);
            let (mut sum, mut n) = (0.0, 0.0);
            for (dr, dc) in [(-1, -1), (-1, 0), (-1, 1), (0, -1), (0, 0), (0, 1), (1, -1), (1, 0), (1, 1)] {
                let (rr, cc) = (r + dr, c + dc);
                if (0..gy).contains(&rr) && (0..gx).contains(&cc) {
                    sum += delays[(rr * gx + cc) as usize];
                    n += 1.0;
                }
            }
            sum / n
        })
        .collect();
    let (lo, hi) = out.iter().fold((f32::MAX, f32::MIN), |(lo, hi), d| (lo.min(*d), hi.max(*d)));
    let range = (hi - lo).max(f32::EPSILON);
    out.iter_mut().for_each(|d| *d = (*d - lo) / range);
    out
}

/// The order for `outline` under `order`, `focus` being where Center and
/// Pointer start (in `0..1` of the image).
pub fn plan(o: &Outline, order: SketchOrder, focus: [f32; 2]) -> Plan {
    let (sw, sh) = (o.size.0 as f32, o.size.1 as f32);
    let px_of = |p: [f32; 2]| [p[0] * sw, p[1] * sh];
    let f = px_of(focus);
    let away = |p: [f32; 2]| {
        let q = px_of(p);
        (q[0] - f[0]).powi(2) + (q[1] - f[1]).powi(2)
    };
    let n = o.runs.len();
    let mut keys: Vec<(f32, u32)> = Vec::with_capacity(n);
    let mut reverse = vec![false; n].into_boxed_slice();
    for (i, r) in o.runs.iter().enumerate() {
        let pts = &o.points[r.start as usize..r.end as usize];
        let (first, last) = (pts[0], pts[pts.len() - 1]);
        let key = match order {
            SketchOrder::Subject | SketchOrder::TopDown => {
                reverse[i] = first[1] > last[1];
                let top = pts.iter().map(|p| p[1]).fold(f32::MAX, f32::min);
                // Subject strokes sort before every background one.
                if order == SketchOrder::Subject && !r.subject { top + 2.0 } else { top }
            }
            SketchOrder::Center | SketchOrder::Pointer => {
                reverse[i] = away(first) > away(last);
                pts.iter().map(|p| away(*p)).fold(f32::MAX, f32::min)
            }
        };
        keys.push((key, i as u32));
    }
    keys.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    let total = o.runs.iter().map(|r| r.length).sum::<f32>().max(f32::EPSILON);
    let mut start = vec![0.0; n].into_boxed_slice();
    let mut span = vec![0.0; n].into_boxed_slice();
    let mut before = 0.0;
    for &(_, i) in &keys {
        let r = &o.runs[i as usize];
        let s = (r.length / total * SPAN_PER_LENGTH).clamp(SPAN_BOUNDS.0, SPAN_BOUNDS.1);
        span[i as usize] = s;
        start[i as usize] = (before / total).min(1.0) * (1.0 - s);
        before += r.length;
    }
    let wipe: Vec<f32> = o.cells.iter().map(|c| start[c.stroke as usize] + span[c.stroke as usize] * 0.5 + c.dist * 0.25).collect();
    let bleed: Vec<f32> = o.cells.iter().map(|c| c.dist).collect();
    Plan { start, span, reverse, bleed: soften(&bleed), wipe: soften(&wipe) }
}

// ── Frames ───────────────────────────────────────────────────────────────

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t)
}

/// The two phases at linear progress `t`: how far the drawing and the
/// fill are (`0..1` each).
pub fn phases(t: f32, draw: f32) -> (f32, f32) {
    let t = t.clamp(0.0, 1.0);
    let draw = draw.clamp(0.05, 0.95);
    ((t / draw).min(1.0), ((t - draw) / (1.0 - draw)).clamp(0.0, 1.0))
}

/// How far the image has filled in at linear progress `t` (eased `0..1`):
/// what anything arriving with the new image (its subject layer) follows.
pub fn fill_ramp(t: f32, draw: f32) -> f32 {
    smooth(phases(t, draw).1)
}

/// Share of stroke `i` drawn at drawing progress `u`.
pub fn revealed(plan: &Plan, i: usize, u: f32) -> f32 {
    ease_out((u - plan.start[i]) / plan.span[i].max(f32::EPSILON))
}

/// The line colour for `ink`, and the wash laid over the old image under
/// it (paper for dark lines, night for light ones).
pub fn colours(ink: SketchInk, accent: Color, outline: &Outline) -> (Color, Color) {
    let rgb = |(r, g, b): (u8, u8, u8)| Color::from_rgba8(r, g, b, 255);
    let line = match ink {
        SketchInk::Accent => accent.with_alpha(1.0),
        SketchInk::Ink => rgb(INK_BLACK),
        SketchInk::White => rgb((0xff, 0xff, 0xff)),
        SketchInk::Wallpaper => outline.ink,
    };
    (line, if line.lightness() < DARK_LINE { rgb(PAPER) } else { rgb(NIGHT) })
}

/// Where an image of `size` px lands for `fill` in viewport `vp` (logical
/// px, output `scale`): `(x, y, w, h)`.
pub fn image_rect(fill: WallpaperFill, size: (u32, u32), (vw, vh): (f32, f32), scale: f32) -> (f32, f32, f32, f32) {
    let (iw, ih) = (size.0.max(1) as f32, size.1.max(1) as f32);
    let fit = |k: f32| ((vw - iw * k) / 2.0, (vh - ih * k) / 2.0, iw * k, ih * k);
    match fill {
        WallpaperFill::Cover => fit((vw / iw).max(vh / ih)),
        WallpaperFill::Contain => fit((vw / iw).min(vh / ih)),
        WallpaperFill::Stretch | WallpaperFill::Tile => (0.0, 0.0, vw, vh),
        WallpaperFill::Center => fit(1.0 / scale.max(f32::EPSILON)),
    }
}

/// One stroke layer of a frame: width and colour (alpha included).
#[derive(Copy, Clone)]
struct Pen {
    width: f32,
    colour: Hsla,
}

/// Strokes `outline` (each up to its revealed share; all of it without a
/// plan) with up to three pens, widest first, in `rect`.
fn stroke_canvas(outline: Arc<Outline>, plan: Option<Rc<Plan>>, u: f32, pens: [Option<Pen>; 3], rect: (f32, f32, f32, f32)) -> AnyElement {
    canvas(
        |_, _, _| {},
        move |bounds: Bounds<Pixels>, (), window, _| {
            let origin = (f32::from(bounds.origin.x) + rect.0, f32::from(bounds.origin.y) + rect.1);
            stroke_paths(&outline, plan.as_deref(), u, &pens, origin, (rect.2, rect.3), |path, colour| window.paint_path(path, colour));
        },
    )
    .absolute()
    .top_0()
    .left_0()
    .size_full()
    .into_any_element()
}

/// Builds the stroked paths of `outline` at drawing progress `u` (every
/// stroke in full without a plan) for each pen, an image of `size` at
/// `origin`, handing each to `paint` (at most [`CHUNK_POINTS`] points per
/// path; within a chunk the pens go widest first).
fn stroke_paths(outline: &Outline, plan: Option<&Plan>, u: f32, pens: &[Option<Pen>; 3], origin: (f32, f32), size: (f32, f32), mut paint: impl FnMut(gpui::Path<Pixels>, Hsla)) {
    let at = |p: [f32; 2]| point(px(origin.0 + p[0] * size.0), px(origin.1 + p[1] * size.1));
    let new = |pen: &Option<Pen>| pen.map(|p| gpui::PathBuilder::stroke(px(p.width)));
    let mut builders = [new(&pens[0]), new(&pens[1]), new(&pens[2])];
    let mut emitted = 0usize;
    let mut flush = |builders: &mut [Option<gpui::PathBuilder>; 3]| {
        for (b, pen) in builders.iter_mut().zip(pens) {
            if let (Some(done), Some(pen)) = (b.take(), pen)
                && let Ok(path) = done.build()
            {
                paint(path, pen.colour);
            }
            *b = new(pen);
        }
    };
    for (i, r) in outline.runs.iter().enumerate() {
        let share = plan.map_or(1.0, |p| revealed(p, i, u));
        if share <= 0.0 {
            continue;
        }
        let (s, e) = (r.start as usize, r.end as usize);
        let (pts, along) = (&outline.points[s..e], &outline.along[s..e]);
        let reverse = plan.is_some_and(|p| p.reverse[i]);
        if emitted + pts.len() > CHUNK_POINTS {
            flush(&mut builders);
            emitted = 0;
        }
        emitted += pts.len();
        let target = share * r.length;
        let mut put = |p: [f32; 2], first: bool| {
            for b in builders.iter_mut().flatten() {
                if first { b.move_to(at(p)) } else { b.line_to(at(p)) }
            }
        };
        let n = pts.len();
        // Distance from the drawing's start to point `k`.
        let dist = |k: usize| if reverse { r.length - along[k] } else { along[k] };
        let index = |j: usize| if reverse { n - 1 - j } else { j };
        put(pts[index(0)], true);
        for j in 1..n {
            let (a, b) = (index(j - 1), index(j));
            if dist(b) <= target {
                put(pts[b], false);
                continue;
            }
            let seg = (dist(b) - dist(a)).max(f32::EPSILON);
            let k = ((target - dist(a)) / seg).clamp(0.0, 1.0);
            put([pts[a][0] + (pts[b][0] - pts[a][0]) * k, pts[a][1] + (pts[b][1] - pts[a][1]) * k], false);
            break;
        }
    }
    flush(&mut builders);
}

/// Everything a Sketch frame needs besides its layers.
pub struct Frame<'a> {
    pub outline: &'a Arc<Outline>,
    pub plan: &'a Rc<Plan>,
    pub options: &'a SketchOptions,
    pub line: Color,
    pub wash: Color,
    /// Linear progress `0..1`.
    pub t: f32,
    /// The image's rect in the viewport ([`image_rect`]).
    pub rect: (f32, f32, f32, f32),
    pub vp: (f32, f32),
}

/// The elements of a Sketch frame: the old image under a wash, the new
/// one filling in, the lines on top. At `t` = 1 only the new image (and
/// the kept outline) remains.
pub fn compose(old: &dyn Fn() -> AnyElement, new: &dyn Fn() -> AnyElement, f: Frame) -> Vec<AnyElement> {
    let mut out = Vec::new();
    let (u, fill) = phases(f.t, f.options.draw());
    if f.t >= 1.0 {
        out.push(new());
        if f.options.keep_outline {
            out.push(kept(f.outline, f.line, f.options, f.rect));
        }
        return out;
    }
    out.push(old());
    out.push(div().absolute().inset_0().size_full().bg(f.wash.with_alpha(WASH_ALPHA * smooth(u / WASH_RISE)).hsla()).into_any_element());
    if fill > 0.0 {
        match f.options.fill_style {
            SketchFill::Fade => out.push(div().absolute().inset_0().size_full().opacity(smooth(fill)).child(new()).into_any_element()),
            SketchFill::Bleed => cells(&mut out, new, f.plan.bleed.iter().copied(), BLEED_SPREAD, fill, f.rect, f.vp),
            SketchFill::Wipe => cells(&mut out, new, f.plan.wipe.iter().copied(), WIPE_SPREAD, fill, f.rect, f.vp),
        }
    }
    // The lines fade as the image arrives (down to the kept outline).
    let rest = if f.options.keep_outline { KEEP_ALPHA } else { 0.0 };
    let alpha = 1.0 - smooth(fill) * (1.0 - rest);
    let w = f.options.line_px();
    let pen = |width: f32, a: f32| (a > 0.0).then(|| Pen { width, colour: f.line.with_alpha(a).hsla() });
    let bleed = (f.options.fill_style == SketchFill::Bleed && fill > 0.0).then(|| pen(w * (2.0 + BLEED_WIDTH * ease_out(fill)), BLEED_ALPHA * (1.0 - fill))).flatten();
    let glow = f.options.glow.then(|| pen(w * GLOW_WIDTH, GLOW_ALPHA * alpha)).flatten();
    out.push(stroke_canvas(f.outline.clone(), Some(f.plan.clone()), u, [bleed, glow, pen(w, alpha)], f.rect));
    out
}

/// The faint outline kept over the wallpaper at rest.
pub fn kept(outline: &Arc<Outline>, line: Color, options: &SketchOptions, rect: (f32, f32, f32, f32)) -> AnyElement {
    stroke_canvas(outline.clone(), None, 1.0, [None, None, Some(Pen { width: options.line_px(), colour: line.with_alpha(KEEP_ALPHA).hsla() })], rect)
}

/// Opacity of a fill cell with delay `d` (`0..1`) at fill progress `fill`.
pub fn cell_alpha(d: f32, spread: f32, fill: f32) -> f32 {
    smooth((fill - d * spread) / (1.0 - spread))
}

/// The new image revealed cell by cell over the image's rect: each grid
/// row's fully open cells merge into one clip, partly open ones are clips
/// of their own, closed ones are skipped. A whole layer rises under them
/// (fully late in the fill) so the cells' seams melt.
fn cells(out: &mut Vec<AnyElement>, new: &dyn Fn() -> AnyElement, delays: impl Iterator<Item = f32>, spread: f32, fill: f32, (rx, ry, rw, rh): (f32, f32, f32, f32), vp: (f32, f32)) {
    let base = (BASE_SHARE * smooth(fill)).max(smooth((fill - 0.5) / 0.5));
    if base > 0.0 {
        out.push(div().absolute().inset_0().size_full().opacity(base).child(new()).into_any_element());
    }
    let (gx, gy) = (GRID.0 as usize, GRID.1 as usize);
    let (cw, ch) = (rw / gx as f32, rh / gy as f32);
    let piece = |c0: usize, c1: usize, r: usize| crate::wallpaper_transitions::piece(new(), (rx + c0 as f32 * cw, ry + r as f32 * ch, (c1 - c0) as f32 * cw + 1.0, ch + 1.0), (0.0, 0.0), vp);
    let mut open: Option<usize> = None;
    for (i, d) in delays.enumerate().take(gx * gy) {
        let (r, c) = (i / gx, i % gx);
        let a = cell_alpha(d, spread, fill);
        if a >= 1.0 {
            open.get_or_insert(c);
        } else {
            if let Some(c0) = open.take() {
                out.push(piece(c0, c, r).into_any_element());
            }
            if a > 0.0 {
                out.push(piece(c, c + 1, r).opacity(a).into_any_element());
            }
        }
        if c + 1 == gx
            && let Some(c0) = open.take()
        {
            out.push(piece(c0, gx, r).into_any_element());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outline() -> Outline {
        let img = image::GrayImage::from_fn(320, 180, |x, y| {
            let disc = ((x as f32 - 110.0).powi(2) + (y as f32 - 90.0).powi(2)).sqrt() < 50.0;
            let bar = (230..250).contains(&x) && (30..150).contains(&y);
            image::Luma([if disc || bar { 230 } else { 20 }])
        });
        let mask = image::GrayImage::from_fn(320, 180, |x, y| image::Luma([if (225..255).contains(&x) && (25..155).contains(&y) { 255 } else { 0 }]));
        outline::extract(&img, Some(&mask), None)
    }

    #[test]
    fn every_stroke_is_drawn_by_the_end_of_the_drawing_phase() {
        let o = outline();
        for order in [SketchOrder::Subject, SketchOrder::TopDown, SketchOrder::Center, SketchOrder::Pointer] {
            let p = plan(&o, order, [0.2, 0.3]);
            assert_eq!(p, plan(&o, order, [0.2, 0.3]), "plans are deterministic");
            for i in 0..o.runs.len() {
                assert_eq!(revealed(&p, i, 0.0), 0.0, "{order:?}: nothing is drawn at the start");
                assert_eq!(revealed(&p, i, 1.0), 1.0, "{order:?}: stroke {i} finishes");
                assert!(p.start[i] >= 0.0 && p.start[i] + p.span[i] <= 1.0 + 1e-5);
            }
            assert!(p.wipe.iter().chain(p.bleed.iter()).all(|d| (0.0..=1.0).contains(d)));
            assert_eq!(p.bleed.len(), o.cells.len());
        }
    }

    #[test]
    fn the_subject_is_drawn_before_the_background() {
        let o = outline();
        assert!(o.runs.iter().any(|r| r.subject) && o.runs.iter().any(|r| !r.subject));
        let p = plan(&o, SketchOrder::Subject, [0.5, 0.5]);
        let subject = o.runs.iter().enumerate().filter(|(_, r)| r.subject).map(|(i, _)| p.start[i]).fold(f32::MAX, f32::min);
        let background = o.runs.iter().enumerate().filter(|(_, r)| !r.subject).map(|(i, _)| p.start[i]).fold(f32::MAX, f32::min);
        assert!(subject < background, "{subject} vs {background}");
        // Top to bottom: the stroke reaching highest starts first.
        let p = plan(&o, SketchOrder::TopDown, [0.5, 0.5]);
        let first = (0..o.runs.len()).min_by(|a, b| p.start[*a].total_cmp(&p.start[*b])).unwrap();
        let top = |i: usize| o.points[o.runs[i].start as usize..o.runs[i].end as usize].iter().map(|q| q[1]).fold(f32::MAX, f32::min);
        assert!((0..o.runs.len()).all(|i| top(first) <= top(i) + 1e-6));
    }

    #[test]
    fn phases_split_the_transition_and_the_fill_ends_open() {
        assert_eq!(phases(0.0, 0.5), (0.0, 0.0));
        assert_eq!(phases(0.5, 0.5), (1.0, 0.0));
        assert_eq!(phases(1.0, 0.5), (1.0, 1.0));
        assert_eq!(phases(0.25, 0.5).1, 0.0, "no fill while drawing");
        for d in [0.0, 0.3, 1.0] {
            assert_eq!(cell_alpha(d, BLEED_SPREAD, 1.0), 1.0, "every cell is open at the end");
            assert_eq!(cell_alpha(d, WIPE_SPREAD, 0.0), 0.0);
        }
        assert!(cell_alpha(0.0, BLEED_SPREAD, 0.3) > cell_alpha(1.0, BLEED_SPREAD, 0.3), "near the lines first");
    }

    #[test]
    fn colours_keep_the_lines_readable() {
        let o = outline();
        let (line, wash) = colours(SketchInk::Ink, Color::hex("#3366ff"), &o);
        assert!(line.lightness() < DARK_LINE && wash.lightness() > 0.8, "ink on paper");
        let (line, wash) = colours(SketchInk::White, Color::hex("#3366ff"), &o);
        assert!(line.lightness() > 0.9 && wash.lightness() < 0.2, "white on night");
        assert_eq!(colours(SketchInk::Wallpaper, Color::hex("#000000"), &o).0, o.ink);
    }

    #[test]
    fn images_land_where_their_fill_puts_them() {
        assert_eq!(image_rect(WallpaperFill::Cover, (1920, 1080), (1920.0, 1080.0), 1.0), (0.0, 0.0, 1920.0, 1080.0));
        let (x, y, w, h) = image_rect(WallpaperFill::Cover, (1000, 1000), (1600.0, 900.0), 1.0);
        assert_eq!((x, w, h), (0.0, 1600.0, 1600.0));
        assert_eq!(y, -350.0);
        let (x, _, w, h) = image_rect(WallpaperFill::Contain, (1000, 1000), (1600.0, 900.0), 1.0);
        assert_eq!((x, w, h), (350.0, 900.0, 900.0));
        assert_eq!(image_rect(WallpaperFill::Center, (800, 600), (1000.0, 1000.0), 2.0), (300.0, 350.0, 400.0, 300.0));
    }

    /// What one frame's strokes cost on the CPU at their largest (every
    /// point, three pens, a 2560 × 1440 screen):
    /// `cargo test --release -p mandara-ui stroke_cost -- --ignored --nocapture`.
    #[test]
    #[ignore = "a measurement; run in release"]
    fn stroke_cost() {
        let blobs = image::GrayImage::from_fn(320, 180, |x, y| {
            let (fx, fy) = (x as f32 / 6.0, y as f32 / 6.0);
            let (ix, iy, tx, ty) = (fx as u32, fy as u32, fx.fract(), fy.fract());
            let v = |i: u32, j: u32| crate::wallpaper_transitions::hash01(5, j * 97 + i);
            let top = v(ix, iy) * (1.0 - tx) + v(ix + 1, iy) * tx;
            let bottom = v(ix, iy + 1) * (1.0 - tx) + v(ix + 1, iy + 1) * tx;
            image::Luma([if top * (1.0 - ty) + bottom * ty > 0.5 { 240 } else { 15 }])
        });
        let started = std::time::Instant::now();
        let o = outline::extract(&blobs, None, None);
        let traced = started.elapsed();
        let p = plan(&o, SketchOrder::Subject, [0.5, 0.5]);
        let pen = |width: f32| Some(Pen { width, colour: gpui::white() });
        let pens = [pen(30.0), pen(8.0), pen(2.0)];
        let frames = 200;
        let mut paths = 0;
        let started = std::time::Instant::now();
        for k in 0..frames {
            stroke_paths(&o, Some(&p), (k as f32 / frames as f32).max(0.5), &pens, (0.0, 0.0), (2560.0, 1440.0), |_, _| paths += 1);
        }
        let per = started.elapsed() / frames;
        eprintln!("trace {traced:?} for {} points in {} strokes; strokes {per:?} per frame ({} paths)", o.points.len(), o.runs.len(), paths / frames as usize);
        assert!(paths > 0);
    }

    #[test]
    fn outlines_are_cached_by_wallpaper_and_size() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("dune.png");
        image::RgbImage::from_fn(640, 360, |x, y| image::Rgb(if ((x as f32 - 300.0).powi(2) + (y as f32 - 180.0).powi(2)).sqrt() < 90.0 { [240, 200, 120] } else { [20, 30, 60] })).save(&src).unwrap();
        let want = Want { fill: WallpaperFill::Cover, depth: None };
        let decode = || image::open(&src).unwrap();
        let a = for_wallpaper(&want, &src, &src, decode(), None).unwrap();
        assert!(!a.is_empty());
        let b = for_wallpaper(&want, &src, &src, decode(), None).unwrap();
        assert!(Arc::ptr_eq(&a, &b), "a repeat change traces nothing again");
        let contain = for_wallpaper(&Want { fill: WallpaperFill::Contain, ..want.clone() }, &src, &src, decode(), None).unwrap();
        assert!(!Arc::ptr_eq(&a, &contain), "the fill is part of the key");
        let cache = CACHE.lock().unwrap();
        assert!(cache.len() <= CACHE_ENTRIES);
    }
}
