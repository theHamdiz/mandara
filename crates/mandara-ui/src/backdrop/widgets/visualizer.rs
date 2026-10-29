//! The plane visualizer: spectrum bars, mirrored bars, a filled wave strip
//! or radial spikes painted into a plane sized tightly to the shape.
//!
//! Per frame it is one path fill: every bar is a quad appended to one
//! reused [`PathBuilder`] (`Path::clear` hands its memory back), filled
//! with a cached OKLCH gradient paint. No per-frame allocation beyond
//! tiny-skia's rasterizer scratch.

use std::f32::consts::{FRAC_PI_2, TAU};

use mandara_config::desktop_widgets::{PlaneVisualizer, VisualizerShape, ranges};
use mandara_theme::Color;
use tiny_skia::{FillRule, GradientStop, LinearGradient, Paint, PathBuilder, PixmapMut, Point, SpreadMode, Transform};

use crate::lock::paint as lp;

/// Hue turn of the gradient's far end, degrees (CLAUDE.md accent harmony).
const GRADIENT_HUE_TURN: f32 = 25.0;
/// Chroma boost of the far end.
const GRADIENT_CHROMA_GAIN: f32 = 1.15;
/// Gradient stops (sampled in OKLCH so the blend never greys out).
const GRADIENT_STOPS: usize = 4;
/// Shortest bar, fraction of the height (a resting line stays visible).
const MIN_LEVEL: f32 = 0.025;
/// Wave strip points per bar.
const WAVE_POINTS_PER_BAR: usize = 2;
/// Anti-aliasing margin around the shape, reference px.
const AA_MARGIN_PX: f32 = 2.0;

/// The shape's geometry in plane pixels.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Geometry {
    pub shape: VisualizerShape,
    pub bars: usize,
    /// Line length (or inner radius for radial), tallest bar, physical px.
    pub length: f32,
    pub height: f32,
    pub gap: f32,
    pub angle: f32,
    /// Plane size and the shape's centre in it.
    pub size: (f32, f32),
    pub center: (f32, f32),
}

impl Geometry {
    pub fn new(v: &PlaneVisualizer, k: f32) -> Self {
        let l = v.length.clamp(ranges::VISUALIZER_LENGTH_PX.0, ranges::VISUALIZER_LENGTH_PX.1) * k;
        let h = v.height.clamp(ranges::VISUALIZER_HEIGHT_PX.0, ranges::VISUALIZER_HEIGHT_PX.1) * k;
        let angle = v.angle.clamp(ranges::ANGLE_DEG.0, ranges::ANGLE_DEG.1);
        let margin = AA_MARGIN_PX * k * 2.0;
        let size = match v.shape {
            VisualizerShape::Radial => {
                let d = 2.0 * (l / 2.0 + h) + margin;
                (d, d)
            }
            _ => {
                let (w, hh) = super::draw::rotated_size((l, h), angle);
                (w + margin, hh + margin)
            }
        };
        Self {
            shape: v.shape,
            bars: v.bars() as usize,
            length: l,
            height: h,
            gap: v.gap.clamp(ranges::VISUALIZER_GAP.0, ranges::VISUALIZER_GAP.1),
            angle,
            size,
            center: (size.0 / 2.0, size.1 / 2.0),
        }
    }

    /// Maps a point of the unrotated shape (centred on the origin) into the
    /// plane.
    fn place(&self, (x, y): (f32, f32)) -> (f32, f32) {
        let a = self.angle.to_radians();
        let (s, c) = a.sin_cos();
        (self.center.0 + x * c - y * s, self.center.1 + x * s + y * c)
    }

    /// The quads of `levels` (one per bar, `0..=1`) as corner lists, in
    /// plane pixels — the shape's whole geometry (tests and painting).
    pub fn quads(&self, levels: &[f32], mut out: impl FnMut([(f32, f32); 4])) {
        let n = levels.len().max(1);
        match self.shape {
            VisualizerShape::Bars | VisualizerShape::Mirrored => {
                let slot = self.length / n as f32;
                let bw = slot * (1.0 - self.gap);
                for (i, lv) in levels.iter().enumerate() {
                    let h = self.height * lv.clamp(MIN_LEVEL, 1.0);
                    let x0 = -self.length / 2.0 + i as f32 * slot + (slot - bw) / 2.0;
                    let (top, bottom) = match self.shape {
                        // Rising from the baseline at the bottom of the box.
                        VisualizerShape::Bars => (self.height / 2.0 - h, self.height / 2.0),
                        _ => (-h / 2.0, h / 2.0),
                    };
                    out([self.place((x0, top)), self.place((x0 + bw, top)), self.place((x0 + bw, bottom)), self.place((x0, bottom))]);
                }
            }
            VisualizerShape::Radial => {
                let r = self.length / 2.0;
                let slot = TAU / n as f32;
                let half = slot * (1.0 - self.gap) / 2.0;
                let rot = self.angle.to_radians() - FRAC_PI_2;
                for (i, lv) in levels.iter().enumerate() {
                    let a = rot + i as f32 * slot;
                    let outer = r + self.height * lv.clamp(MIN_LEVEL, 1.0);
                    let p = |rad: f32, ang: f32| (self.center.0 + rad * ang.cos(), self.center.1 + rad * ang.sin());
                    out([p(r, a - half), p(outer, a - half), p(outer, a + half), p(r, a + half)]);
                }
            }
            // The wave is one polygon (see `wave_points`).
            VisualizerShape::Wave => {}
        }
    }

    /// The wave strip's outline: the top edge left to right, then the
    /// bottom edge back (mirrored about the line).
    pub fn wave_points(&self, levels: &[f32], mut out: impl FnMut((f32, f32))) {
        let n = levels.len();
        if n < 2 {
            return;
        }
        let pts = n * WAVE_POINTS_PER_BAR;
        let at = |j: usize| {
            let f = j as f32 / (pts - 1) as f32;
            let pos = f * (n - 1) as f32;
            let (i, t) = (pos.floor() as usize, pos.fract());
            let a = levels[i.min(n - 1)];
            let b = levels[(i + 1).min(n - 1)];
            // Smooth between bars, tapering to the ends.
            let level = a + (b - a) * (t * t * (3.0 - 2.0 * t));
            let taper = (f * std::f32::consts::PI).sin();
            (-self.length / 2.0 + f * self.length, self.height / 2.0 * level.clamp(MIN_LEVEL, 1.0) * taper.max(MIN_LEVEL))
        };
        for j in 0..pts {
            let (x, h) = at(j);
            out(self.place((x, -h)));
        }
        for j in (0..pts).rev() {
            let (x, h) = at(j);
            out(self.place((x, h)));
        }
    }
}

/// Spectrum bands resampled to `n` bars (linear), written into `out`.
pub fn resample(bands: &[f32], out: &mut [f32]) {
    let n = out.len();
    if bands.is_empty() {
        out.iter_mut().for_each(|v| *v = 0.0);
        return;
    }
    let last = (bands.len() - 1) as f32;
    for (i, v) in out.iter_mut().enumerate() {
        let pos = if n > 1 { i as f32 / (n - 1) as f32 * last } else { 0.0 };
        let (j, t) = (pos.floor() as usize, pos.fract());
        let a = bands[j.min(bands.len() - 1)];
        let b = bands[(j + 1).min(bands.len() - 1)];
        *v = (a + (b - a) * t).clamp(0.0, 1.0);
    }
}

/// Per-widget painter state, reused across frames.
#[derive(Default)]
pub struct Painter {
    builder: Option<PathBuilder>,
    levels: Vec<f32>,
    paint: Option<(u64, Paint<'static>)>,
}

impl Painter {
    /// Paints one frame of `bands` into `pm` (already cleared).
    pub fn frame(&mut self, pm: &mut PixmapMut<'_>, g: &Geometry, bands: &[f32], accent: Color, opacity: f32) {
        self.levels.resize(g.bars, 0.0);
        if g.shape == VisualizerShape::Radial {
            // Bass at the top, the spectrum mirrored down both sides.
            let half = g.bars.div_ceil(2);
            resample(bands, &mut self.levels[..half]);
            for j in 0..g.bars - half {
                self.levels[g.bars - 1 - j] = self.levels[j + 1];
            }
        } else {
            resample(bands, &mut self.levels);
        }
        let key = super::key((g.size.0 as u32, g.size.1 as u32, g.angle.to_bits(), g.shape, accent.to_rgba8(), opacity.to_bits()));
        if self.paint.as_ref().is_none_or(|(k, _)| *k != key) {
            self.paint = Some((key, gradient(g, accent, opacity)));
        }
        let mut pb = self.builder.take().unwrap_or_default();
        if g.shape == VisualizerShape::Wave {
            let mut first = true;
            g.wave_points(&self.levels, |(x, y)| {
                if first {
                    pb.move_to(x, y);
                    first = false;
                } else {
                    pb.line_to(x, y);
                }
            });
            pb.close();
        } else {
            g.quads(&self.levels, |q| {
                pb.move_to(q[0].0, q[0].1);
                pb.line_to(q[1].0, q[1].1);
                pb.line_to(q[2].0, q[2].1);
                pb.line_to(q[3].0, q[3].1);
                pb.close();
            });
        }
        match pb.finish() {
            Some(path) => {
                if let Some((_, paint)) = &self.paint {
                    pm.fill_path(&path, paint, FillRule::Winding, Transform::identity(), None);
                }
                self.builder = Some(path.clear());
            }
            None => self.builder = Some(PathBuilder::new()),
        }
    }
}

/// The accent → hue-turned accent gradient along the shape.
fn gradient(g: &Geometry, accent: Color, opacity: f32) -> Paint<'static> {
    let end = Color::oklch(accent.lightness(), accent.chroma() * GRADIENT_CHROMA_GAIN, accent.hue() + GRADIENT_HUE_TURN);
    let stops: Vec<GradientStop> = (0..GRADIENT_STOPS)
        .map(|i| {
            let t = i as f32 / (GRADIENT_STOPS - 1) as f32;
            let c = Color::oklch(accent.lightness() + (end.lightness() - accent.lightness()) * t, accent.chroma() + (end.chroma() - accent.chroma()) * t, accent.hue() + GRADIENT_HUE_TURN * t);
            GradientStop::new(t, lp::skia(c, opacity))
        })
        .collect();
    let (a, b) = match g.shape {
        // Radial: inner to outer edge reads best as a vertical sweep.
        VisualizerShape::Radial => ((g.center.0, 0.0), (g.center.0, g.size.1)),
        _ => (g.place((-g.length / 2.0, 0.0)), g.place((g.length / 2.0, 0.0))),
    };
    let shader = LinearGradient::new(Point::from_xy(a.0, a.1), Point::from_xy(b.0, b.1), stops, SpreadMode::Pad, Transform::identity());
    let mut paint = Paint { anti_alias: true, ..Paint::default() };
    match shader {
        Some(s) => paint.shader = s,
        None => paint.set_color(lp::skia(accent, opacity)),
    }
    paint
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geom(shape: VisualizerShape, angle: f32) -> Geometry {
        Geometry::new(&PlaneVisualizer { shape, angle, bars: 10, length: 400.0, height: 100.0, gap: 0.5, ..Default::default() }, 1.0)
    }

    fn all_inside(g: &Geometry, levels: &[f32]) -> bool {
        let mut ok = true;
        let mut check = |p: (f32, f32)| ok &= p.0 >= -0.01 && p.1 >= -0.01 && p.0 <= g.size.0 + 0.01 && p.1 <= g.size.1 + 0.01;
        g.quads(levels, |q| q.iter().for_each(|p| check(*p)));
        g.wave_points(levels, &mut check);
        ok
    }

    #[test]
    fn shapes_stay_inside_their_tight_plane() {
        let full = [1.0; 10];
        for shape in [VisualizerShape::Bars, VisualizerShape::Mirrored, VisualizerShape::Wave, VisualizerShape::Radial] {
            for angle in [0.0, -30.0, 45.0, 90.0] {
                let g = geom(shape, angle);
                assert!(all_inside(&g, &full), "{shape:?} at {angle}°");
            }
        }
        let flat = geom(VisualizerShape::Bars, 0.0);
        assert!(flat.size.0 < 400.0 + 10.0 && flat.size.1 < 100.0 + 10.0, "tight: {:?}", flat.size);
        let diag = geom(VisualizerShape::Bars, 45.0);
        assert!(diag.size.0 > 300.0 && diag.size.1 > 300.0, "a diagonal line needs a square-ish plane");
    }

    #[test]
    fn bars_rise_from_the_baseline_along_the_angle() {
        let g = geom(VisualizerShape::Bars, 0.0);
        let mut quads = Vec::new();
        g.quads(&[0.0, 1.0], |q| quads.push(q));
        assert_eq!(quads.len(), 2);
        let base = g.center.1 + 50.0;
        assert!((quads[0][2].1 - base).abs() < 1e-3 && (quads[1][2].1 - base).abs() < 1e-3, "same baseline");
        assert!((quads[1][0].1 - (base - 100.0)).abs() < 1e-3, "a full bar is the full height");
        assert!(quads[0][0].1 > quads[1][0].1, "a silent bar keeps a sliver");
        // Rotated 90°: the line runs down, the bars grow sideways.
        let v = geom(VisualizerShape::Bars, 90.0);
        let mut q = Vec::new();
        v.quads(&[1.0, 1.0], |x| q.push(x));
        assert!(q[1][0].1 > q[0][0].1, "second bar further down the line");
    }

    #[test]
    fn radial_spikes_ring_the_centre() {
        let g = geom(VisualizerShape::Radial, 0.0);
        let mut quads = Vec::new();
        g.quads(&[0.5; 10], |q| quads.push(q));
        assert_eq!(quads.len(), 10);
        for q in &quads {
            let d = |p: (f32, f32)| ((p.0 - g.center.0).powi(2) + (p.1 - g.center.1).powi(2)).sqrt();
            assert!((d(q[0]) - 200.0).abs() < 1e-2 && (d(q[1]) - 250.0).abs() < 1e-2);
        }
        // The first spike points up.
        assert!(quads[0][1].1 < g.center.1 - 200.0);
    }

    #[test]
    fn resamples_bands_to_bars() {
        let mut out = [0.0; 5];
        resample(&[0.0, 1.0], &mut out);
        assert_eq!(out, [0.0, 0.25, 0.5, 0.75, 1.0]);
        resample(&[], &mut out);
        assert!(out.iter().all(|v| *v == 0.0));
    }

    #[test]
    fn frames_reuse_the_path_memory() {
        let g = geom(VisualizerShape::Mirrored, -30.0);
        let mut pm = tiny_skia::Pixmap::new(g.size.0.ceil() as u32, g.size.1.ceil() as u32).unwrap();
        let mut p = Painter::default();
        let accent = Color::oklch(0.75, 0.12, 200.0);
        p.frame(&mut pm.as_mut(), &g, &[0.4; 32], accent, 1.0);
        assert!(pm.data().as_chunks::<4>().0.iter().any(|px| px[3] > 0), "painted");
        let cap = p.levels.capacity();
        for _ in 0..5 {
            p.frame(&mut pm.as_mut(), &g, &[0.9; 32], accent, 1.0);
        }
        assert_eq!(p.levels.capacity(), cap);
        assert!(p.builder.is_some() && p.paint.is_some());
    }
}
