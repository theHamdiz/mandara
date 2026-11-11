//! The Sketch transition's outline: a bounded set of polylines traced from
//! a small copy of the new wallpaper (at most [`SAMPLE_MAX`] px on its long
//! side). The subject's contour comes from its depth mask when Mandara has
//! one; the rest from edge detection (Sobel, thinned by non-maximum
//! suppression, two thresholds picked from the image's own edge strengths,
//! then chained into lines). Everything here is a pure function of its
//! input: the same pictures give the same strokes, in the same order.
//!
//! The result is small and fixed in size: at most [`MAX_POINTS`] points in
//! [`MAX_STROKES`] strokes (≈ 64 KiB with the fill grid), in `0..1` image
//! coordinates, plus a coarse grid saying which stroke is nearest each
//! cell and how far it is (what the fill styles reveal by).

use image::{GrayImage, RgbImage};
use mandara_theme::Color;

/// Longest side of the copy outlines are traced in, px.
pub const SAMPLE_MAX: u32 = 320;
/// Most points and strokes an outline keeps.
pub const MAX_POINTS: usize = 4096;
pub const MAX_STROKES: usize = 384;
/// The fill grid, `(columns, rows)` over the image.
pub const GRID: (u32, u32) = (24, 14);
/// Shorter lines are noise (sample px); the subject's contour may be shorter.
const MIN_EDGE_PX: f32 = 10.0;
const MIN_SUBJECT_PX: f32 = 6.0;
/// Simplification tolerance (sample px).
const SIMPLIFY_PX: f32 = 0.6;
/// Passes of a [1 2 1] average over each line's points before it is
/// simplified: pixel chains are staircases, eight times larger on screen.
const SMOOTH_PASSES: usize = 3;
/// Share of the thinned edge pixels that count as strong edges (lines start
/// only there), and the weak threshold as a share of the strong one.
const STRONG_SHARE: f32 = 0.12;
const WEAK_RATIO: f32 = 0.45;
/// Weakest gradient that can be an edge at all (Sobel magnitude of 8-bit
/// luminance, `0..≈1442`): keeps flat or noisy pictures from inventing lines.
const MIN_STRONG: f32 = 60.0;
/// Histogram bins for the strong threshold.
const BINS: usize = 256;
const MAG_MAX: f32 = 1443.0;
/// A subject covering less or more of the frame than this is no outline.
const SUBJECT_COVER: (f32, f32) = (0.005, 0.95);
/// Mask alpha at or above this is the subject.
const SUBJECT_ALPHA: u8 = 128;
/// Hue bins for the dominant colour, and the lightness it is lifted to
/// (lines must read over the dark wash).
const HUE_BINS: usize = 12;
const INK_LIGHTNESS: f32 = 0.72;

/// One stroke: points `start..end` of [`Outline::points`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Run {
    pub start: u32,
    pub end: u32,
    /// Part of the subject's contour (else a background edge).
    pub subject: bool,
    /// Length in sample px.
    pub length: f32,
}

/// Which stroke is nearest a fill-grid cell, and how far (`0..=1`, the
/// farthest cell is 1).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Cell {
    pub stroke: u16,
    pub dist: f32,
}

/// A traced outline, shared by every output showing the wallpaper.
#[derive(Debug, PartialEq)]
pub struct Outline {
    /// `(x, y)` in `0..1` of the image.
    pub points: Box<[[f32; 2]]>,
    /// Length from its stroke's first point to each point, sample px.
    pub along: Box<[f32]>,
    pub runs: Box<[Run]>,
    /// [`GRID`] cells, row by row (empty without strokes).
    pub cells: Box<[Cell]>,
    /// Size of the sample the outline was traced in (its aspect).
    pub size: (u32, u32),
    /// The picture's dominant colour, lifted to read as a line.
    pub ink: Color,
}

impl Outline {
    /// Whether there is anything to draw.
    pub fn is_empty(&self) -> bool {
        self.runs.is_empty()
    }

    /// Bytes held (for the cache's accounting and the tests).
    pub fn bytes(&self) -> usize {
        std::mem::size_of::<Self>() + std::mem::size_of_val(&*self.points) + std::mem::size_of_val(&*self.along) + std::mem::size_of_val(&*self.runs) + std::mem::size_of_val(&*self.cells)
    }
}

/// The size a sample of an image of `w × h` is traced at.
pub fn sample_size(w: u32, h: u32) -> (u32, u32) {
    let long = w.max(h).max(1);
    if long <= SAMPLE_MAX {
        return (w.max(1), h.max(1));
    }
    let k = SAMPLE_MAX as f32 / long as f32;
    (((w as f32 * k).round() as u32).max(1), ((h as f32 * k).round() as u32).max(1))
}

/// Traces the outline of `luma` (the sample), its subject from `mask`
/// (subject alpha, the same size) when given. `colour` is a small copy of
/// the picture for the dominant colour.
pub fn extract(luma: &GrayImage, mask: Option<&GrayImage>, colour: Option<&RgbImage>) -> Outline {
    let (w, h) = luma.dimensions();
    let mut lines = Lines::default();
    let mask = mask.filter(|m| m.dimensions() == (w, h));
    let mut contour = vec![0u8; (w * h) as usize];
    if let Some(m) = mask
        && subject_contour(m, &mut contour)
    {
        trace(&mut contour.clone(), w, h, true, &mut lines);
    } else {
        contour.iter_mut().for_each(|c| *c = 0);
    }
    let mut edges = edge_map(luma);
    // The subject's contour is drawn already: no second line beside it.
    if mask.is_some() {
        suppress_near(&mut edges, &contour, w, h);
    }
    drop(contour);
    trace(&mut edges, w, h, false, &mut lines);
    drop(edges);
    let (points, along, runs) = lines.select(w, h);
    let cells = grid(&points, &runs, w, h);
    Outline { points, along, runs, cells, size: (w, h), ink: colour.map(dominant).unwrap_or(Color::oklch(INK_LIGHTNESS, 0.12, 200.0)) }
}

// ── Subject contour ──────────────────────────────────────────────────────

/// Marks (with 2) the subject's boundary pixels in `out`: inside the mask
/// with a 4-neighbour outside it. The frame's own edge is no boundary (a
/// subject cut by the frame is not outlined along it). `false` when the
/// subject is too small or fills the frame.
fn subject_contour(mask: &GrayImage, out: &mut [u8]) -> bool {
    let (w, h) = mask.dimensions();
    let inside = |x: u32, y: u32| mask.get_pixel(x, y).0[0] >= SUBJECT_ALPHA;
    let covered = mask.pixels().filter(|p| p.0[0] >= SUBJECT_ALPHA).count() as f32 / (w * h).max(1) as f32;
    if !(SUBJECT_COVER.0..=SUBJECT_COVER.1).contains(&covered) {
        return false;
    }
    let mut any = false;
    for y in 0..h {
        for x in 0..w {
            if !inside(x, y) {
                continue;
            }
            let edge = (x > 0 && !inside(x - 1, y)) || (x + 1 < w && !inside(x + 1, y)) || (y > 0 && !inside(x, y - 1)) || (y + 1 < h && !inside(x, y + 1));
            if edge {
                out[(y * w + x) as usize] = 2;
                any = true;
            }
        }
    }
    any
}

/// Clears edge pixels within one pixel of the subject's contour.
fn suppress_near(edges: &mut [u8], contour: &[u8], w: u32, h: u32) {
    for y in 0..h {
        for x in 0..w {
            if contour[(y * w + x) as usize] == 0 {
                continue;
            }
            for (nx, ny) in neighbours(x, y, w, h) {
                edges[(ny * w + nx) as usize] = 0;
            }
            edges[(y * w + x) as usize] = 0;
        }
    }
}

fn neighbours(x: u32, y: u32, w: u32, h: u32) -> impl Iterator<Item = (u32, u32)> {
    DIRS.iter().filter_map(move |&(dx, dy)| {
        let (nx, ny) = (x as i32 + dx, y as i32 + dy);
        (nx >= 0 && ny >= 0 && (nx as u32) < w && (ny as u32) < h).then_some((nx as u32, ny as u32))
    })
}

// ── Edges ────────────────────────────────────────────────────────────────

/// Thinned edges of `luma`: 2 strong, 1 weak, 0 none.
fn edge_map(luma: &GrayImage) -> Vec<u8> {
    let (w, h) = luma.dimensions();
    let (wu, hu) = (w as usize, h as usize);
    let mut out = vec![0u8; wu * hu];
    if w < 3 || h < 3 {
        return out;
    }
    // A light blur ([1 2 1]², fixed point) so grain is no edge.
    let src = luma.as_raw();
    let at = |x: usize, y: usize| src[y * wu + x] as u32;
    let mut soft = vec![0u16; wu * hu];
    for y in 0..hu {
        let (y0, y2) = (y.saturating_sub(1), (y + 1).min(hu - 1));
        for x in 0..wu {
            let (x0, x2) = (x.saturating_sub(1), (x + 1).min(wu - 1));
            let row = |yy: usize| at(x0, yy) + 2 * at(x, yy) + at(x2, yy);
            soft[y * wu + x] = ((row(y0) + 2 * row(y) + row(y2) + 8) / 16) as u16;
        }
    }
    // Sobel magnitude and direction sector (0: horizontal gradient, 1: 45°,
    // 2: vertical, 3: 135°).
    let mut mag = vec![0f32; wu * hu];
    let mut dir = vec![0u8; wu * hu];
    let s = |x: usize, y: usize| soft[y * wu + x] as i32;
    for y in 1..hu - 1 {
        for x in 1..wu - 1 {
            let gx = s(x + 1, y - 1) + 2 * s(x + 1, y) + s(x + 1, y + 1) - s(x - 1, y - 1) - 2 * s(x - 1, y) - s(x - 1, y + 1);
            let gy = s(x - 1, y + 1) + 2 * s(x, y + 1) + s(x + 1, y + 1) - s(x - 1, y - 1) - 2 * s(x, y - 1) - s(x + 1, y - 1);
            let i = y * wu + x;
            mag[i] = ((gx * gx + gy * gy) as f32).sqrt();
            let a = (gy as f32).atan2(gx as f32).to_degrees().rem_euclid(180.0);
            dir[i] = (((a + 22.5) / 45.0) as u8) % 4;
        }
    }
    drop(soft);
    // Thin to one pixel: keep a pixel only where it beats its neighbours
    // across the edge (ties go to the first in raster order).
    let mut thin = vec![0f32; wu * hu];
    let mut hist = [0u32; BINS];
    let mut survivors = 0u32;
    for y in 1..hu - 1 {
        for x in 1..wu - 1 {
            let i = y * wu + x;
            let m = mag[i];
            if m < MIN_STRONG * WEAK_RATIO {
                continue;
            }
            let (a, b) = match dir[i] {
                0 => (i - 1, i + 1),
                1 => (i - wu - 1, i + wu + 1),
                2 => (i - wu, i + wu),
                _ => (i - wu + 1, i + wu - 1),
            };
            if m > mag[a] && m >= mag[b] {
                thin[i] = m;
                hist[((m / MAG_MAX * BINS as f32) as usize).min(BINS - 1)] += 1;
                survivors += 1;
            }
        }
    }
    drop(mag);
    drop(dir);
    // The strong threshold keeps the strongest share of the thin edges.
    let keep = (survivors as f32 * STRONG_SHARE).ceil() as u32;
    let mut seen = 0;
    let mut bin = BINS;
    while bin > 0 && seen < keep {
        bin -= 1;
        seen += hist[bin];
    }
    let strong = (bin as f32 / BINS as f32 * MAG_MAX).max(MIN_STRONG);
    let weak = strong * WEAK_RATIO;
    for (o, m) in out.iter_mut().zip(&thin) {
        *o = if *m >= strong { 2 } else if *m >= weak { 1 } else { 0 };
    }
    out
}

// ── Chaining ─────────────────────────────────────────────────────────────

/// The 8 neighbours, clockwise from east.
const DIRS: [(i32, i32); 8] = [(1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1), (0, -1), (1, -1)];

/// Lines found so far, before selection: pixel points, flat.
#[derive(Default)]
struct Lines {
    points: Vec<[f32; 2]>,
    /// `(start, end, subject)` into `points`.
    runs: Vec<(usize, usize, bool)>,
}

/// Chains the marked pixels of `map` (consumed: cleared as visited) into
/// lines, each starting at a strong pixel (2) and running on through weak
/// ones (1), simplified.
fn trace(map: &mut [u8], w: u32, h: u32, subject: bool, lines: &mut Lines) {
    let mut back: Vec<[u32; 2]> = Vec::new();
    let mut forward: Vec<[u32; 2]> = Vec::new();
    let mut chain: Vec<[f32; 2]> = Vec::new();
    let mut scratch: Vec<[f32; 2]> = Vec::new();
    for y in 0..h {
        for x in 0..w {
            if map[(y * w + x) as usize] != 2 {
                continue;
            }
            map[(y * w + x) as usize] = 0;
            walk(map, w, h, [x, y], &mut forward);
            walk(map, w, h, [x, y], &mut back);
            chain.clear();
            chain.extend(back.iter().rev().chain(std::iter::once(&[x, y])).chain(forward.iter()).map(|p| [p[0] as f32, p[1] as f32]));
            let start = lines.points.len();
            relax(&mut chain, &mut scratch);
            simplify(&chain, &mut lines.points);
            let end = lines.points.len();
            if end - start >= 2 {
                lines.runs.push((start, end, subject));
            } else {
                lines.points.truncate(start);
            }
        }
    }
}

/// Follows marked pixels from `from` into `out` (cleared first), keeping
/// the heading where it can.
fn walk(map: &mut [u8], w: u32, h: u32, from: [u32; 2], out: &mut Vec<[u32; 2]>) {
    out.clear();
    let mut cur = from;
    let mut heading: Option<usize> = None;
    loop {
        let mut next = None;
        for k in 0..8 {
            // The heading first, then turning gently either way.
            let d = match heading {
                Some(hd) => (hd + [0, 1, 7, 2, 6, 3, 5, 4][k]) % 8,
                None => k,
            };
            let (nx, ny) = (cur[0] as i32 + DIRS[d].0, cur[1] as i32 + DIRS[d].1);
            if nx < 0 || ny < 0 || nx as u32 >= w || ny as u32 >= h {
                continue;
            }
            let i = (ny as u32 * w + nx as u32) as usize;
            if map[i] != 0 {
                map[i] = 0;
                next = Some(([nx as u32, ny as u32], d));
                break;
            }
        }
        let Some((p, d)) = next else { break };
        out.push(p);
        cur = p;
        heading = Some(d);
    }
}

/// Smooths `chain` in place ([`SMOOTH_PASSES`] of a [1 2 1] average, its
/// ends fixed), `scratch` holding the previous pass.
fn relax(chain: &mut [[f32; 2]], scratch: &mut Vec<[f32; 2]>) {
    let n = chain.len();
    if n < 3 {
        return;
    }
    for _ in 0..SMOOTH_PASSES {
        scratch.clear();
        scratch.extend_from_slice(chain);
        for i in 1..n - 1 {
            let (a, b, c) = (scratch[i - 1], scratch[i], scratch[i + 1]);
            chain[i] = [(a[0] + 2.0 * b[0] + c[0]) / 4.0, (a[1] + 2.0 * b[1] + c[1]) / 4.0];
        }
    }
}

/// Ramer–Douglas–Peucker simplification of `chain` (appended to `out`).
fn simplify(chain: &[[f32; 2]], out: &mut Vec<[f32; 2]>) {
    let n = chain.len();
    if n <= 2 {
        out.extend_from_slice(chain);
        return;
    }
    let mut keep = vec![false; n];
    keep[0] = true;
    keep[n - 1] = true;
    let mut stack = vec![(0usize, n - 1)];
    while let Some((a, b)) = stack.pop() {
        let (pa, pb) = (chain[a], chain[b]);
        let (dx, dy) = (pb[0] - pa[0], pb[1] - pa[1]);
        let len = (dx * dx + dy * dy).sqrt();
        let mut far = (0.0f32, 0usize);
        for (i, p) in chain.iter().enumerate().take(b).skip(a + 1) {
            let d = if len < f32::EPSILON { ((p[0] - pa[0]).powi(2) + (p[1] - pa[1]).powi(2)).sqrt() } else { ((p[0] - pa[0]) * dy - (p[1] - pa[1]) * dx).abs() / len };
            if d > far.0 {
                far = (d, i);
            }
        }
        if far.0 > SIMPLIFY_PX {
            keep[far.1] = true;
            stack.push((a, far.1));
            stack.push((far.1, b));
        }
    }
    out.extend(chain.iter().zip(&keep).filter(|(_, k)| **k).map(|(p, _)| *p));
}

fn polyline_length(points: &[[f32; 2]]) -> f32 {
    points.windows(2).map(|s| ((s[1][0] - s[0][0]).powi(2) + (s[1][1] - s[0][1]).powi(2)).sqrt()).sum()
}

impl Lines {
    /// The strokes to keep within [`MAX_POINTS`] and [`MAX_STROKES`]: the
    /// subject's contour first, then the longest edges, each smoothed once
    /// (corner cutting), normalised to `0..1`.
    fn select(&self, w: u32, h: u32) -> (Box<[[f32; 2]]>, Box<[f32]>, Box<[Run]>) {
        let mut order: Vec<(bool, f32, usize)> = self
            .runs
            .iter()
            .enumerate()
            .map(|(i, &(s, e, subject))| (subject, polyline_length(&self.points[s..e]), i))
            .filter(|&(subject, len, _)| len >= if subject { MIN_SUBJECT_PX } else { MIN_EDGE_PX })
            .collect();
        order.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.total_cmp(&a.1)).then(a.2.cmp(&b.2)));
        let mut points = Vec::new();
        let mut along = Vec::new();
        let mut runs = Vec::new();
        let (fw, fh) = (w.max(1) as f32, h.max(1) as f32);
        for (subject, _, i) in order {
            if runs.len() >= MAX_STROKES {
                break;
            }
            let (s, e, _) = self.runs[i];
            let src = &self.points[s..e];
            // Corner cutting doubles the inner points.
            let n = 2 + (src.len() - 2) * 2;
            if points.len() + n > MAX_POINTS {
                continue;
            }
            let start = points.len();
            let mut push = |p: [f32; 2]| points.push(p);
            push(src[0]);
            for k in 1..src.len() - 1 {
                let (a, b, c) = (src[k - 1], src[k], src[k + 1]);
                push([0.25 * a[0] + 0.75 * b[0], 0.25 * a[1] + 0.75 * b[1]]);
                push([0.75 * b[0] + 0.25 * c[0], 0.75 * b[1] + 0.25 * c[1]]);
            }
            push(src[src.len() - 1]);
            let mut acc = 0.0;
            along.push(0.0);
            for k in start + 1..points.len() {
                let (a, b) = (points[k - 1], points[k]);
                acc += ((b[0] - a[0]).powi(2) + (b[1] - a[1]).powi(2)).sqrt();
                along.push(acc);
            }
            for p in &mut points[start..] {
                *p = [(p[0] + 0.5) / fw, (p[1] + 0.5) / fh];
            }
            runs.push(Run { start: start as u32, end: points.len() as u32, subject, length: acc });
        }
        (points.into_boxed_slice(), along.into_boxed_slice(), runs.into_boxed_slice())
    }
}

// ── Fill grid ────────────────────────────────────────────────────────────

/// For every [`GRID`] cell, the nearest stroke and its distance: strokes are
/// drawn into a `w × h` id map, then spread outward breadth-first (8
/// neighbours, so the distance is in steps), and each cell reads its centre.
fn grid(points: &[[f32; 2]], runs: &[Run], w: u32, h: u32) -> Box<[Cell]> {
    if runs.is_empty() {
        return Box::new([]);
    }
    let (wu, hu) = (w as usize, h as usize);
    let mut id = vec![u16::MAX; wu * hu];
    let mut dist = vec![u16::MAX; wu * hu];
    let mut queue = std::collections::VecDeque::new();
    let (fw, fh) = (w as f32, h as f32);
    for (r, run) in runs.iter().enumerate() {
        let pts = &points[run.start as usize..run.end as usize];
        for seg in pts.windows(2) {
            let (a, b) = ([seg[0][0] * fw - 0.5, seg[0][1] * fh - 0.5], [seg[1][0] * fw - 0.5, seg[1][1] * fh - 0.5]);
            let steps = ((b[0] - a[0]).abs().max((b[1] - a[1]).abs()).ceil() as usize).max(1);
            for k in 0..=steps {
                let t = k as f32 / steps as f32;
                let (x, y) = ((a[0] + (b[0] - a[0]) * t).round(), (a[1] + (b[1] - a[1]) * t).round());
                if x < 0.0 || y < 0.0 || x >= fw || y >= fh {
                    continue;
                }
                let i = y as usize * wu + x as usize;
                if dist[i] == u16::MAX {
                    dist[i] = 0;
                    id[i] = r as u16;
                    queue.push_back(i);
                }
            }
        }
    }
    while let Some(i) = queue.pop_front() {
        let (x, y) = ((i % wu) as u32, (i / wu) as u32);
        for (nx, ny) in neighbours(x, y, w, h) {
            let j = ny as usize * wu + nx as usize;
            if dist[j] == u16::MAX {
                dist[j] = dist[i] + 1;
                id[j] = id[i];
                queue.push_back(j);
            }
        }
    }
    let (gx, gy) = GRID;
    let mut cells: Vec<Cell> = Vec::with_capacity((gx * gy) as usize);
    for r in 0..gy {
        for c in 0..gx {
            let x = (((c as f32 + 0.5) / gx as f32 * fw) as usize).min(wu - 1);
            let y = (((r as f32 + 0.5) / gy as f32 * fh) as usize).min(hu - 1);
            let i = y * wu + x;
            cells.push(Cell { stroke: id[i].min(runs.len() as u16 - 1), dist: dist[i] as f32 });
        }
    }
    let far = cells.iter().map(|c| c.dist).fold(1.0f32, f32::max);
    for c in &mut cells {
        c.dist /= far;
    }
    cells.into_boxed_slice()
}

// ── Colour ───────────────────────────────────────────────────────────────

/// The picture's dominant colour: the hue holding the most chroma, its
/// mean, lifted to [`INK_LIGHTNESS`] so it reads as a line; a grey picture
/// gives a light grey.
pub fn dominant(rgb: &RgbImage) -> Color {
    let mut weight = [0f32; HUE_BINS];
    let mut sum = [[0f32; 3]; HUE_BINS];
    for p in rgb.pixels() {
        let c = Color::from_rgba8(p.0[0], p.0[1], p.0[2], 255);
        let chroma = c.chroma();
        let l = c.lightness();
        // Near-black and near-white pixels carry no colour worth drawing.
        if chroma < 0.03 || !(0.15..=0.95).contains(&l) {
            continue;
        }
        let bin = ((c.hue().rem_euclid(360.0) / 360.0 * HUE_BINS as f32) as usize).min(HUE_BINS - 1);
        weight[bin] += chroma;
        for (acc, v) in sum[bin].iter_mut().zip(p.0) {
            *acc += v as f32 * chroma;
        }
    }
    let (best, w) = weight.iter().enumerate().fold((0, 0.0f32), |acc, (i, &w)| if w > acc.1 { (i, w) } else { acc });
    if w <= f32::EPSILON {
        return Color::oklch(INK_LIGHTNESS + 0.1, 0.0, 0.0);
    }
    let avg = |k: usize| (sum[best][k] / w).round().clamp(0.0, 255.0) as u8;
    let c = Color::from_rgba8(avg(0), avg(1), avg(2), 255);
    c.with_lightness(c.lightness().max(INK_LIGHTNESS)).with_chroma(c.chroma().max(0.08)).gamut_mapped()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dark picture with a bright disc and a bright bar.
    fn scene(w: u32, h: u32) -> GrayImage {
        GrayImage::from_fn(w, h, |x, y| {
            let (dx, dy) = (x as f32 - w as f32 * 0.35, y as f32 - h as f32 * 0.5);
            let disc = (dx * dx + dy * dy).sqrt() < h as f32 * 0.25;
            let bar = x > w * 3 / 4 && x < w * 3 / 4 + 12 && y > h / 5 && y < h * 4 / 5;
            image::Luma([if disc || bar { 220 } else { 30 }])
        })
    }

    fn noise(w: u32, h: u32, seed: u32) -> GrayImage {
        GrayImage::from_fn(w, h, |x, y| image::Luma([(crate::wallpaper_transitions::hash01(seed, y * w + x) * 255.0) as u8]))
    }

    fn check_bounds(o: &Outline) {
        assert!(o.points.len() <= MAX_POINTS, "{} points", o.points.len());
        assert!(o.runs.len() <= MAX_STROKES);
        assert_eq!(o.points.len(), o.along.len());
        for r in o.runs.iter() {
            assert!(r.end > r.start + 1 && r.end as usize <= o.points.len());
            assert_eq!(o.along[r.start as usize], 0.0);
            assert!((o.along[r.end as usize - 1] - r.length).abs() < 1e-3);
            assert!(o.along[r.start as usize..r.end as usize].windows(2).all(|w| w[1] >= w[0]), "lengths grow along a stroke");
        }
        assert!(o.points.iter().all(|p| (0.0..=1.0).contains(&p[0]) && (0.0..=1.0).contains(&p[1])));
        if !o.is_empty() {
            assert_eq!(o.cells.len(), (GRID.0 * GRID.1) as usize);
            assert!(o.cells.iter().all(|c| (c.stroke as usize) < o.runs.len() && (0.0..=1.0).contains(&c.dist)));
        }
    }

    #[test]
    fn samples_fit_the_cap_and_keep_the_aspect() {
        assert_eq!(sample_size(3840, 2160), (320, 180));
        assert_eq!(sample_size(1080, 1920), (180, 320));
        assert_eq!(sample_size(200, 100), (200, 100));
        assert_eq!(sample_size(0, 0), (1, 1));
    }

    #[test]
    fn outlines_follow_the_edges_and_are_deterministic() {
        let img = scene(320, 180);
        let a = extract(&img, None, None);
        check_bounds(&a);
        assert!(!a.is_empty(), "a disc and a bar have edges");
        assert!(a.runs.iter().all(|r| !r.subject), "no mask, no subject strokes");
        // Every point sits on the disc's rim or the bar's sides.
        for p in a.points.iter() {
            let (x, y) = (p[0] * 320.0, p[1] * 180.0);
            let rim = (((x - 112.0).powi(2) + (y - 90.0).powi(2)).sqrt() - 45.0).abs() < 4.0;
            let bar = (x > 236.0 && x < 256.0) && (y > 32.0 && y < 148.0);
            assert!(rim || bar, "stray point at {x},{y}");
        }
        assert_eq!(a, extract(&img, None, None), "the same picture gives the same outline");
        assert!(a.bytes() < 128 * 1024, "{} bytes", a.bytes());
    }

    #[test]
    fn a_mask_outlines_the_subject_first() {
        let img = scene(320, 180);
        let mask = GrayImage::from_fn(320, 180, |x, y| image::Luma([if x > 140 && x < 200 && y > 60 { 255 } else { 0 }]));
        let o = extract(&img, Some(&mask), None);
        check_bounds(&o);
        let first = o.runs.first().expect("strokes");
        assert!(first.subject, "the subject's contour comes first");
        let subject: Vec<&Run> = o.runs.iter().filter(|r| r.subject).collect();
        assert!(!subject.is_empty() && o.runs.iter().any(|r| !r.subject), "subject and background");
        assert!(o.runs.iter().skip_while(|r| r.subject).all(|r| !r.subject), "subject strokes are listed first");
        for r in subject {
            for p in &o.points[r.start as usize..r.end as usize] {
                let (x, y) = (p[0] * 320.0, p[1] * 180.0);
                assert!((138.0..=202.0).contains(&x) && y >= 58.0, "subject point off the mask at {x},{y}");
            }
        }
        // A mask that fills the frame (or is empty) is no subject.
        let full = GrayImage::from_pixel(320, 180, image::Luma([255]));
        assert!(extract(&img, Some(&full), None).runs.iter().all(|r| !r.subject));
        // A mask of the wrong size is ignored.
        assert_eq!(extract(&img, Some(&GrayImage::new(10, 10)), None), extract(&img, None, None));
    }

    #[test]
    fn busy_and_flat_pictures_stay_bounded() {
        for seed in 0..3 {
            let o = extract(&noise(320, 180, seed), None, None);
            check_bounds(&o);
            assert!(o.points.len() <= MAX_POINTS);
        }
        // Hundreds of wiggly blobs: far more edge than the budget.
        let blobs = GrayImage::from_fn(320, 320, |x, y| {
            let (fx, fy) = (x as f32 / 6.0, y as f32 / 6.0);
            let (ix, iy) = (fx as u32, fy as u32);
            let (tx, ty) = (fx.fract(), fy.fract());
            let v = |i: u32, j: u32| crate::wallpaper_transitions::hash01(5, j * 97 + i);
            let top = v(ix, iy) * (1.0 - tx) + v(ix + 1, iy) * tx;
            let bottom = v(ix, iy + 1) * (1.0 - tx) + v(ix + 1, iy + 1) * tx;
            image::Luma([if top * (1.0 - ty) + bottom * ty > 0.5 { 240 } else { 15 }])
        });
        let o = extract(&blobs, None, None);
        check_bounds(&o);
        assert!(o.points.len() > MAX_POINTS / 2 || o.runs.len() == MAX_STROKES, "the budget is used: {} points, {} strokes", o.points.len(), o.runs.len());
        let flat = extract(&GrayImage::from_pixel(320, 180, image::Luma([128])), None, None);
        assert!(flat.is_empty() && flat.cells.is_empty(), "a flat picture has no outline");
        let tiny = extract(&GrayImage::new(2, 2), None, None);
        assert!(tiny.is_empty());
    }

    #[test]
    fn fill_cells_point_at_their_nearest_stroke() {
        let o = extract(&scene(320, 180), None, None);
        let (gx, _) = GRID;
        // The cell on the disc's rim is nearer a stroke than a far corner.
        let rim = o.cells[(7 * gx + 3) as usize];
        let corner = o.cells[0];
        assert!(rim.dist < corner.dist, "{rim:?} vs {corner:?}");
        assert!(o.cells.iter().any(|c| c.dist == 1.0), "distances are normalised to the farthest cell");
    }

    #[test]
    fn the_dominant_colour_is_the_most_colourful_hue() {
        let rgb = RgbImage::from_fn(64, 36, |x, _| image::Rgb(if x < 40 { [30, 30, 30] } else { [200, 40, 40] }));
        let c = dominant(&rgb);
        let [r, g, b, _] = c.to_rgba8();
        assert!(r > g && r > b, "red wins: {r} {g} {b}");
        assert!(c.lightness() >= INK_LIGHTNESS - 0.02);
        let grey = dominant(&RgbImage::from_pixel(8, 8, image::Rgb([90, 90, 90])));
        assert!(grey.chroma() < 0.02);
    }
}
