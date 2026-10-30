//! Liquid metaball geometry: capsules on one axis merged with a polynomial
//! smooth minimum of their signed distance fields, traced into outline
//! polygons for GPUI paths.
//!
//! Coordinates are `(u, v)`: `u` runs along the strip, `v` across it with
//! `0` on the strip's centre line. Every capsule is centred on that line, so
//! the merged field is symmetric in `v` and grows with `|v|` (each capsule's
//! distance does, and `smin` is non-decreasing in both arguments). The shape
//! is therefore, at each `u`, the band `|v| < e(u)`, and tracing it only
//! needs the half-extent `e(u)` at a row of samples.

use std::ops::Range;

/// Bisection steps for a half-extent: a 22 px pill resolves to ~1/200 px.
const EXTENT_BISECT_STEPS: u32 = 12;
/// Bisection steps for a tip on the axis (searched over one sample step).
const TIP_BISECT_STEPS: u32 = 12;
/// Samples per contour are kept within these bounds: enough for a round cap
/// on the tiniest blob, and a ceiling so a runaway span can't stall a frame.
const MIN_CONTOUR_SAMPLES: usize = 8;
const MAX_CONTOUR_SAMPLES: usize = 1024;
/// A contour narrower than this (a blob collapsing to nothing) is skipped.
const MIN_CONTOUR_LEN: f32 = 1.0e-3;

/// A stadium centred on the axis: the segment `a..=b` (along `u`) inflated by
/// `radius`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Capsule {
    pub a: f32,
    pub b: f32,
    pub radius: f32,
}

impl Capsule {
    /// The capsule filling the span `start..end` of a strip `2 × half_thickness`
    /// thick. A span shorter than the strip is thick becomes a circle of the
    /// span's diameter, so a blob grows out of (and shrinks into) a dot.
    pub fn from_span(start: f32, end: f32, half_thickness: f32) -> Self {
        let (s, e) = if start <= end { (start, end) } else { (end, start) };
        let radius = half_thickness.min((e - s) * 0.5).max(0.0);
        Self { a: s + radius, b: e - radius, radius }
    }

    /// The leading edge along the axis.
    pub fn start(&self) -> f32 {
        self.a - self.radius
    }

    /// The trailing edge along the axis.
    pub fn end(&self) -> f32 {
        self.b + self.radius
    }

    /// Signed distance from `(u, v)`: negative inside, zero on the edge.
    pub fn sdf(&self, u: f32, v: f32) -> f32 {
        let du = (self.a - u).max(u - self.b).max(0.0);
        du.hypot(v) - self.radius
    }
}

/// Polynomial smooth minimum (Quílez): equals `min(a, b)` once the two differ
/// by `k` or more and dips at most `k / 4` below it where they meet, which is
/// what fuses two nearby shapes into one with a rounded waist.
pub fn smin(a: f32, b: f32, k: f32) -> f32 {
    if k <= 0.0 {
        return a.min(b);
    }
    let h = (k - (a - b).abs()).max(0.0) / k;
    a.min(b) - h * h * k * 0.25
}

/// The merged field of `caps` at `(u, v)`.
pub fn field(caps: &[Capsule], k: f32, u: f32, v: f32) -> f32 {
    let mut caps = caps.iter();
    let Some(first) = caps.next() else { return f32::INFINITY };
    caps.fold(first.sdf(u, v), |d, c| smin(d, c.sdf(u, v), k))
}

/// Half the merged shape's thickness at `u`, at most `cap` (overlapping
/// blobs would otherwise bulge `k / 4` past the strip).
pub fn half_extent(caps: &[Capsule], k: f32, cap: f32, u: f32) -> f32 {
    if field(caps, k, u, 0.0) >= 0.0 {
        return 0.0;
    }
    if field(caps, k, u, cap) <= 0.0 {
        return cap;
    }
    let (mut lo, mut hi) = (0.0, cap);
    for _ in 0..EXTENT_BISECT_STEPS {
        let mid = (lo + hi) * 0.5;
        if field(caps, k, u, mid) < 0.0 { lo = mid } else { hi = mid }
    }
    (lo + hi) * 0.5
}

/// Ranges of `caps` (sorted by start) whose fields interact: neighbours
/// closer than `k` blend, farther ones are exactly their plain union, so a
/// group of one is just its capsule.
pub fn groups(caps: &[Capsule], k: f32) -> impl Iterator<Item = Range<usize>> + '_ {
    let mut i = 0;
    std::iter::from_fn(move || {
        let first = caps.get(i)?;
        let start = i;
        let mut reach = first.end();
        i += 1;
        while let Some(next) = caps.get(i) {
            if next.start() - reach >= k {
                break;
            }
            reach = reach.max(next.end());
            i += 1;
        }
        Some(start..i)
    })
}

/// Which screen axis the strip runs along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    Horizontal,
    Vertical,
}

impl Axis {
    /// Screen `[x, y]` of strip point `[u, v]` for a strip whose centre line
    /// sits `cross` px across from `origin`.
    pub fn to_xy(self, origin: [f32; 2], cross: f32, [u, v]: [f32; 2]) -> [f32; 2] {
        match self {
            Self::Horizontal => [origin[0] + u, origin[1] + cross + v],
            Self::Vertical => [origin[0] + cross + v, origin[1] + u],
        }
    }
}

/// The top edge of a contour from [`Outline::contours`], tip to tip (the
/// bottom edge is its mirror image), for drawing it as a triangle strip.
pub fn top_edge(contour: &[[f32; 2]]) -> &[[f32; 2]] {
    &contour[..(contour.len() / 2 + 1).min(contour.len())]
}

/// The outward unit normal at point `i` of a contour from
/// [`Outline::contours`] (central difference of its neighbours; the contour
/// runs clockwise on screen, `v` pointing down).
pub fn normal(contour: &[[f32; 2]], i: usize) -> [f32; 2] {
    let m = contour.len();
    if m < 3 {
        return [0.0, 0.0];
    }
    let (prev, next) = (contour[(i + m - 1) % m], contour[(i + 1) % m]);
    let (du, dv) = (next[0] - prev[0], next[1] - prev[1]);
    let len = du.hypot(dv);
    if len <= f32::EPSILON { [0.0, 0.0] } else { [dv / len, -du / len] }
}

/// Outline polygons of a merged field, in strip coordinates. The buffers
/// are kept between traces, so retracing a moving shape allocates nothing
/// once they have grown.
#[derive(Clone, Debug, Default)]
pub struct Outline {
    points: Vec<[f32; 2]>,
    contours: Vec<Range<usize>>,
}

impl Outline {
    pub fn clear(&mut self) {
        self.points.clear();
        self.contours.clear();
    }

    /// Each closed contour: the top edge left → right, then the bottom edge
    /// right → left (the tips are shared, not repeated).
    pub fn contours(&self) -> impl Iterator<Item = &[[f32; 2]]> {
        self.contours.iter().map(|r| &self.points[r.clone()])
    }

    pub fn is_empty(&self) -> bool {
        self.contours.is_empty()
    }

    /// Traces `caps` (sorted by start), appending one contour per connected
    /// piece. Half-extents are capped at `cap`; samples are about `spacing`
    /// apart along the middle of each piece and cosine-packed towards the
    /// tips, where a round cap turns fastest.
    pub fn trace(&mut self, caps: &[Capsule], k: f32, cap: f32, spacing: f32) {
        let (Some(first), Some(last)) = (caps.first(), caps.iter().map(Capsule::end).reduce(f32::max)) else { return };
        let spacing = spacing.max(f32::EPSILON);
        // Smooth minimum can only add material where shapes are within k.
        let (lo, hi) = (first.start() - k, last + k);
        let inside = |u: f32| field(caps, k, u, 0.0) < 0.0;
        // Bisects the axis crossing between an outside and an inside point.
        let tip = |mut out: f32, mut inn: f32| {
            for _ in 0..TIP_BISECT_STEPS {
                let mid = (out + inn) * 0.5;
                if inside(mid) { inn = mid } else { out = mid }
            }
            (out + inn) * 0.5
        };
        let steps = ((hi - lo) / spacing).ceil().max(1.0) as usize;
        let mut was_inside = false;
        let mut prev = lo;
        let mut piece_start = lo;
        for i in 1..=steps {
            let u = (lo + spacing * i as f32).min(hi);
            let now = inside(u);
            if now && !was_inside {
                piece_start = tip(prev, u);
            } else if !now && was_inside {
                let piece_end = tip(u, prev);
                self.push_piece(caps, k, cap, spacing, piece_start, piece_end);
            }
            was_inside = now;
            prev = u;
        }
    }

    fn push_piece(&mut self, caps: &[Capsule], k: f32, cap: f32, spacing: f32, t0: f32, t1: f32) {
        let len = t1 - t0;
        if len < MIN_CONTOUR_LEN {
            return;
        }
        // Cosine spacing: the middle gets `spacing`, the tips much less.
        let n = ((len * std::f32::consts::FRAC_PI_2 / spacing).ceil() as usize).clamp(MIN_CONTOUR_SAMPLES, MAX_CONTOUR_SAMPLES);
        let at = |i: usize| t0 + len * 0.5 * (1.0 - (std::f32::consts::PI * i as f32 / n as f32).cos());
        let begin = self.points.len();
        // Top edge, tips included at zero extent.
        self.points.push([t0, 0.0]);
        for i in 1..n {
            let u = at(i);
            self.points.push([u, -half_extent(caps, k, cap, u)]);
        }
        self.points.push([t1, 0.0]);
        // Bottom edge mirrors the top, right to left.
        for i in (begin + 1..begin + n).rev() {
            let [u, v] = self.points[i];
            self.points.push([u, -v]);
        }
        self.contours.push(begin..self.points.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const R: f32 = 11.0;
    const GAP: f32 = 2.0;
    const K: f32 = 14.0;
    const SPACING: f32 = 0.5;

    fn trace(caps: &[Capsule]) -> Outline {
        let mut o = Outline::default();
        o.trace(caps, K, R, SPACING);
        o
    }

    /// The outline's half-extent at `u` (largest |v| among points within a
    /// sample of it), or 0 where no contour covers it.
    fn extent_near(o: &Outline, u: f32) -> f32 {
        o.contours().flatten().filter(|p| (p[0] - u).abs() <= SPACING).map(|p| p[1].abs()).fold(0.0, f32::max)
    }

    #[test]
    fn capsule_sdf_sign_and_distance() {
        let c = Capsule::from_span(0.0, 40.0, R);
        assert_eq!((c.a, c.b, c.radius), (11.0, 29.0, 11.0));
        assert!(c.sdf(20.0, 0.0) < 0.0, "centre is inside");
        assert!((c.sdf(20.0, 0.0) + R).abs() < 1e-6, "centre sits a radius deep");
        assert!(c.sdf(20.0, R).abs() < 1e-6, "flank is on the edge");
        assert!((c.sdf(-5.0, 0.0) - 5.0).abs() < 1e-5, "5 px past the left tip");
        assert!((c.sdf(29.0 + 3.0, 4.0) - (5.0 - R)).abs() < 1e-5, "inside the round cap");
        assert!((c.sdf(20.0, 15.0) - 4.0).abs() < 1e-6, "4 px above the flank");
    }

    #[test]
    fn short_span_is_a_dot() {
        let c = Capsule::from_span(10.0, 14.0, R);
        assert_eq!((c.a, c.b, c.radius), (12.0, 12.0, 2.0));
        assert_eq!((c.start(), c.end()), (10.0, 14.0));
        let flipped = Capsule::from_span(14.0, 10.0, R);
        assert_eq!(c, flipped, "an overshooting spring may cross its edges");
    }

    #[test]
    fn smin_is_below_min_and_blends_near_equality() {
        for (a, b) in [(0.0, 0.0), (1.0, 3.0), (-2.0, 5.0), (4.0, -1.0), (7.0, 7.5)] {
            assert!(smin(a, b, K) <= a.min(b) + 1e-6);
        }
        assert!((smin(3.0, 3.0, K) - (3.0 - K / 4.0)).abs() < 1e-6, "deepest dip at equality");
        assert_eq!(smin(0.0, K, K), 0.0, "exact min once k apart");
        assert_eq!(smin(-5.0, 20.0, K), -5.0);
        assert_eq!(smin(2.0, 1.0, 0.0), 1.0, "k = 0 is a hard min");
        let near = smin(1.0, 1.5, K);
        assert!(near < 1.0 && near > 1.0 - K / 4.0);
    }

    #[test]
    fn adjacent_capsules_fuse_with_a_waist() {
        let caps = [Capsule::from_span(0.0, 22.0, R), Capsule::from_span(22.0 + GAP, 44.0 + GAP, R)];
        assert_eq!(groups(&caps, K).count(), 1);
        let o = trace(&caps);
        assert_eq!(o.contours().count(), 1, "one liquid shape");
        let mid = 22.0 + GAP / 2.0;
        let waist = half_extent(&caps, K, R, mid);
        assert!(waist > 0.0 && waist < R, "waist {waist} is thinner than the pill");
        assert!(waist > R * 0.4, "waist {waist} is a visible bridge, not a thread");
        assert!((extent_near(&o, mid) - waist).abs() < 0.5, "the outline follows the waist");
        let c = o.contours().next().unwrap();
        assert!(c.first().unwrap()[0].abs() < 0.05 && (c[c.len() / 2][0] - (44.0 + GAP)).abs() < 0.6, "tips sit on the pill ends");
    }

    #[test]
    fn capsules_a_pill_apart_stay_separate() {
        let w = 22.0;
        let caps = [Capsule::from_span(0.0, w, R), Capsule::from_span(2.0 * (w + GAP), 3.0 * w + 2.0 * GAP, R)];
        assert_eq!(groups(&caps, K).count(), 2, "no interaction at all");
        let o = trace(&caps);
        assert_eq!(o.contours().count(), 2);
        assert_eq!(half_extent(&caps, K, R, w + GAP + w / 2.0), 0.0, "nothing in the gap");
        assert_eq!(extent_near(&o, w + GAP + w / 2.0), 0.0);
    }

    #[test]
    fn single_capsule_matches_a_stadium() {
        let c = Capsule::from_span(3.0, 63.0, R);
        let o = trace(&[c]);
        assert_eq!(o.contours().count(), 1);
        for &[u, v] in o.contours().next().unwrap() {
            // Distance of the outline point from the true stadium edge.
            assert!(c.sdf(u, v).abs() < 0.5, "({u}, {v}) is {} px off", c.sdf(u, v));
        }
        // And the polygon reaches the full thickness and both tips.
        let pts = o.contours().next().unwrap();
        assert!(pts.iter().any(|p| (p[1].abs() - R).abs() < 0.01));
        let (min_u, max_u) = pts.iter().fold((f32::MAX, f32::MIN), |(a, b), p| (a.min(p[0]), b.max(p[0])));
        assert!((min_u - 3.0).abs() < 0.01 && (max_u - 63.0).abs() < 0.01);
    }

    #[test]
    fn overlap_is_capped_to_the_strip() {
        let caps = [Capsule::from_span(0.0, 30.0, R), Capsule::from_span(10.0, 40.0, R)];
        let o = trace(&caps);
        assert!(o.contours().flatten().all(|p| p[1].abs() <= R + 1e-4));
    }

    #[test]
    fn vertical_strip_is_the_mirror_image() {
        let caps = [Capsule::from_span(0.0, 22.0, R), Capsule::from_span(24.0, 46.0, R)];
        let o = trace(&caps);
        let origin = [5.0, 7.0];
        for &p in o.contours().next().unwrap() {
            let [hx, hy] = Axis::Horizontal.to_xy(origin, R, p);
            let [vx, vy] = Axis::Vertical.to_xy([origin[1], origin[0]], R, p);
            assert_eq!((hx, hy), (vy, vx), "vertical swaps x and y");
        }
    }

    #[test]
    fn normals_point_outwards() {
        let c = Capsule::from_span(0.0, 60.0, R);
        let o = trace(&[c]);
        let contour = o.contours().next().unwrap();
        for (i, &[u, v]) in contour.iter().enumerate() {
            let [nu, nv] = normal(contour, i);
            assert!((nu.hypot(nv) - 1.0).abs() < 1e-4);
            assert!(c.sdf(u + nu, v + nv) > c.sdf(u, v), "normal {i} points out of the shape");
        }
        assert!(normal(contour, 0)[0] < -0.99, "the left tip faces left");
    }

    #[test]
    fn retrace_reuses_buffers() {
        let caps = [Capsule::from_span(0.0, 22.0, R)];
        let mut o = Outline::default();
        o.trace(&caps, K, R, SPACING);
        let cap = o.points.capacity();
        o.clear();
        assert!(o.is_empty());
        o.trace(&caps, K, R, SPACING);
        assert_eq!(o.points.capacity(), cap);
    }
}
