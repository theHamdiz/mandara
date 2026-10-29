//! YUV → BGRA conversion for decoded video frames.
//!
//! Integer fixed-point math with coefficients derived from the stream's
//! matrix (BT.709 / BT.601 / BT.2020) and range (limited / full). Handles
//! 4:2:0, 4:2:2, 4:4:4 and monochrome at 8–12 bits, optionally halving the
//! resolution on the fly (a 2×2 luma box filter) so a 4K clip can be shown
//! on a slow machine at a quarter of the conversion and upload cost. Large
//! frames are converted in horizontal bands across all cores.

/// Colour matrix of the stream (`matrix_coefficients` in the sequence header).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Matrix {
    Bt709,
    Bt601,
    Bt2020,
}

impl Matrix {
    /// Luma weights (Kr, Kb).
    fn weights(self) -> (f32, f32) {
        match self {
            Matrix::Bt709 => (0.2126, 0.0722),
            Matrix::Bt601 => (0.299, 0.114),
            Matrix::Bt2020 => (0.2627, 0.0593),
        }
    }
}

/// Chroma subsampling.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Layout {
    I400,
    I420,
    I422,
    I444,
}

impl Layout {
    /// (horizontal, vertical) chroma shift.
    fn shifts(self) -> (usize, usize) {
        match self {
            Layout::I420 => (1, 1),
            Layout::I422 => (1, 0),
            Layout::I444 | Layout::I400 => (0, 0),
        }
    }
}

/// A borrowed planar frame. Strides are in samples (not bytes).
pub struct YuvFrame<'a, S> {
    pub width: usize,
    pub height: usize,
    pub layout: Layout,
    pub y: &'a [S],
    pub u: &'a [S],
    pub v: &'a [S],
    pub y_stride: usize,
    pub uv_stride: usize,
    /// Bits per sample (8, 10 or 12).
    pub bits: u32,
    pub full_range: bool,
    pub matrix: Matrix,
}

/// A sample type (u8 for 8-bit streams, u16 for 10/12-bit).
pub trait Sample: Copy + Send + Sync {
    fn get(self) -> i32;
}

impl Sample for u8 {
    #[inline(always)]
    fn get(self) -> i32 {
        self as i32
    }
}

impl Sample for u16 {
    #[inline(always)]
    fn get(self) -> i32 {
        self as i32
    }
}

/// Fixed-point (Q14) conversion coefficients for 8-bit-normalized samples.
#[derive(Copy, Clone, Debug)]
struct Coeffs {
    y_mul: i32,
    y_off: i32,
    rv: i32,
    gu: i32,
    gv: i32,
    bu: i32,
}

const Q: i32 = 14;

impl Coeffs {
    fn new(matrix: Matrix, full: bool) -> Self {
        let (kr, kb) = matrix.weights();
        let kg = 1.0 - kr - kb;
        let (ys, cs, yo) = if full { (1.0, 1.0, 0) } else { (255.0 / 219.0, 255.0 / 224.0, 16) };
        let q = (1 << Q) as f32;
        let rv = 2.0 * (1.0 - kr) * cs;
        let bu = 2.0 * (1.0 - kb) * cs;
        let gu = 2.0 * (1.0 - kb) * kb / kg * cs;
        let gv = 2.0 * (1.0 - kr) * kr / kg * cs;
        Coeffs {
            y_mul: (ys * q).round() as i32,
            y_off: yo,
            rv: (rv * q).round() as i32,
            gu: (gu * q).round() as i32,
            gv: (gv * q).round() as i32,
            bu: (bu * q).round() as i32,
        }
    }
}

#[inline(always)]
fn clamp8(v: i32) -> u8 {
    (v >> Q).clamp(0, 255) as u8
}

/// Output size for a frame converted with `half`.
pub fn output_size(width: usize, height: usize, half: bool) -> (usize, usize) {
    if half { ((width / 2).max(1), (height / 2).max(1)) } else { (width, height) }
}

/// Converts `frame` to tightly packed BGRA (`A = 255`), at half resolution
/// when `half` is set. Returns `(bgra, width, height)`.
pub fn to_bgra<S: Sample>(frame: &YuvFrame<'_, S>, half: bool) -> (Vec<u8>, usize, usize) {
    let (ow, oh) = output_size(frame.width, frame.height, half);
    let mut out = vec![0u8; ow * oh * 4];
    let threads = if ow * oh >= 512 * 512 { std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 16) } else { 1 };
    let band = oh.div_ceil(threads).max(1);
    let coeffs = Coeffs::new(frame.matrix, frame.full_range);
    if threads == 1 {
        convert_rows(frame, half, coeffs, 0, &mut out, ow);
    } else {
        std::thread::scope(|scope| {
            for (i, chunk) in out.chunks_mut(ow * 4 * band).enumerate() {
                scope.spawn(move || convert_rows(frame, half, coeffs, i * band, chunk, ow));
            }
        });
    }
    (out, ow, oh)
}

/// Converts the output rows starting at `row0` into `out` (whole rows).
fn convert_rows<S: Sample>(f: &YuvFrame<'_, S>, half: bool, c: Coeffs, row0: usize, out: &mut [u8], ow: usize) {
    let shift = f.bits.saturating_sub(8);
    let (sx, sy) = f.layout.shifts();
    let mono = f.layout == Layout::I400;
    let rows = out.len() / (ow * 4);
    let (w, h) = (f.width, f.height);
    let cw = (w + (1 << sx) - 1) >> sx;
    let ch = (h + (1 << sy) - 1) >> sy;
    let sample = |plane: &[S], stride: usize, x: usize, y: usize| -> i32 { plane.get(y * stride + x).map_or(0, |s| s.get() >> shift) };
    for r in 0..rows {
        let oy = row0 + r;
        let line = &mut out[r * ow * 4..(r + 1) * ow * 4];
        // Source luma row(s) for this output row.
        let (ya, yb) = if half { ((oy * 2).min(h - 1), (oy * 2 + 1).min(h - 1)) } else { (oy.min(h - 1), oy.min(h - 1)) };
        let crow = if half { ((oy * 2) >> sy).min(ch.saturating_sub(1)) } else { (oy >> sy).min(ch.saturating_sub(1)) };
        let yrow_a = &f.y[(ya * f.y_stride).min(f.y.len())..];
        let yrow_b = &f.y[(yb * f.y_stride).min(f.y.len())..];
        for ox in 0..ow {
            let luma = if half {
                let x0 = (ox * 2).min(w - 1);
                let x1 = (ox * 2 + 1).min(w - 1);
                let s = |row: &[S], x: usize| row.get(x).map_or(0, |v| v.get() >> shift);
                (s(yrow_a, x0) + s(yrow_a, x1) + s(yrow_b, x0) + s(yrow_b, x1) + 2) >> 2
            } else {
                yrow_a.get(ox.min(w - 1)).map_or(0, |v| v.get() >> shift)
            };
            let yv = (luma - c.y_off) * c.y_mul + (1 << (Q - 1));
            let px = &mut line[ox * 4..ox * 4 + 4];
            if mono {
                let g = clamp8(yv);
                px.copy_from_slice(&[g, g, g, 255]);
                continue;
            }
            let cx = if half { ((ox * 2) >> sx).min(cw.saturating_sub(1)) } else { (ox >> sx).min(cw.saturating_sub(1)) };
            let u = sample(f.u, f.uv_stride, cx, crow) - 128;
            let v = sample(f.v, f.uv_stride, cx, crow) - 128;
            px[0] = clamp8(yv + c.bu * u);
            px[1] = clamp8(yv - c.gu * u - c.gv * v);
            px[2] = clamp8(yv + c.rv * v);
            px[3] = 255;
        }
    }
}

// ── Region conversion into a caller's buffer ─────────────────────────────

/// Byte order of destination pixels: `Bgra` is `wl_shm` `ARGB8888` on
/// little-endian, `Rgba` is `ABGR8888` (and tiny-skia's pixmaps).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PixelOrder {
    Bgra,
    Rgba,
}

/// A source rectangle in picture pixels.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct Crop {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

impl Crop {
    /// The part of `self` inside a `width × height` picture (at least 1 × 1).
    pub fn clamp_to(self, width: u32, height: u32) -> Crop {
        let x = self.x.min(width.saturating_sub(1));
        let y = self.y.min(height.saturating_sub(1));
        Crop { x, y, w: self.w.clamp(1, width.saturating_sub(x).max(1)), h: self.h.clamp(1, height.saturating_sub(y).max(1)) }
    }
}

/// A destination buffer of `width × height` 4-byte pixels, rows `stride`
/// bytes apart.
pub struct Target<'a> {
    pub data: &'a mut [u8],
    pub width: u32,
    pub height: u32,
    pub stride: usize,
    pub order: PixelOrder,
}

/// How a crop maps onto a target.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Sampling {
    /// Same size: one source pixel per target pixel.
    Direct,
    /// Exactly twice the target size: a 2 × 2 luma box filter.
    Half,
    /// Anything else: nearest source pixel (centre-aligned).
    Nearest,
}

/// The sampling [`convert_region`] uses for `crop` onto a `dw × dh` target.
pub fn sampling(crop: Crop, dw: u32, dh: u32) -> Sampling {
    if crop.w == dw && crop.h == dh {
        Sampling::Direct
    } else if crop.w == dw * 2 && crop.h == dh * 2 {
        Sampling::Half
    } else {
        Sampling::Nearest
    }
}

/// Fixed-point fraction bits of the nearest-sampling step.
const STEP_BITS: u32 = 16;

/// Converts `crop` of `frame` into `dst`, scaled to the target's size
/// ([`sampling`]), opaque, in the target's byte order. Rows are split across
/// up to `threads` threads. Nothing is allocated.
pub fn convert_region<S: Sample>(frame: &YuvFrame<'_, S>, crop: Crop, dst: &mut Target<'_>, threads: usize) {
    let (dw, dh) = (dst.width as usize, dst.height as usize);
    if dw == 0 || dh == 0 || frame.width == 0 || frame.height == 0 || dst.stride < dw * 4 || dst.data.len() < dst.stride * (dh - 1) + dw * 4 {
        return;
    }
    let crop = crop.clamp_to(frame.width as u32, frame.height as u32);
    let mode = sampling(crop, dst.width, dst.height);
    let c = Coeffs::new(frame.matrix, frame.full_range);
    let stride = dst.stride;
    let rgba = dst.order == PixelOrder::Rgba;
    let threads = threads.clamp(1, dh);
    let band = dh.div_ceil(threads);
    let run = |row0: usize, rows: usize, out: &mut [u8]| {
        if rgba {
            region_rows::<S, true>(frame, crop, mode, c, row0, rows, out, stride, dw, dh);
        } else {
            region_rows::<S, false>(frame, crop, mode, c, row0, rows, out, stride, dw, dh);
        }
    };
    if threads == 1 {
        run(0, dh, dst.data);
        return;
    }
    std::thread::scope(|scope| {
        let mut rest: &mut [u8] = dst.data;
        let mut row0 = 0;
        while row0 < dh {
            let rows = band.min(dh - row0);
            let take = if row0 + rows == dh { rest.len() } else { (stride * rows).min(rest.len()) };
            let (chunk, tail) = std::mem::take(&mut rest).split_at_mut(take);
            rest = tail;
            let run = &run;
            scope.spawn(move || run(row0, rows, chunk));
            row0 += rows;
        }
    });
}

#[inline(always)]
fn put<const RGBA: bool>(px: &mut [u8], b: u8, g: u8, r: u8) {
    if RGBA {
        px[0] = r;
        px[2] = b;
    } else {
        px[0] = b;
        px[2] = r;
    }
    px[1] = g;
    px[3] = 255;
}

/// Converts target rows `row0 .. row0 + rows` into `out` (row `row0` first).
#[allow(clippy::too_many_arguments)]
fn region_rows<S: Sample, const RGBA: bool>(f: &YuvFrame<'_, S>, crop: Crop, mode: Sampling, c: Coeffs, row0: usize, rows: usize, out: &mut [u8], stride: usize, dw: usize, dh: usize) {
    let shift = f.bits.saturating_sub(8);
    let (sxs, sys) = f.layout.shifts();
    let mono = f.layout == Layout::I400;
    let (w, h) = (f.width, f.height);
    let cw = ((w + (1 << sxs) - 1) >> sxs).max(1);
    let ch = ((h + (1 << sys) - 1) >> sys).max(1);
    let (cx0, cy0, cwid, chei) = (crop.x as usize, crop.y as usize, crop.w as usize, crop.h as usize);
    let x_step = ((cwid as u64) << STEP_BITS) / dw as u64;
    for r in 0..rows {
        let oy = row0 + r;
        let line = &mut out[r * stride..r * stride + dw * 4];
        let (ya, yb) = match mode {
            Sampling::Direct => (cy0 + oy, cy0 + oy),
            Sampling::Half => (cy0 + oy * 2, (cy0 + oy * 2 + 1).min(h - 1)),
            Sampling::Nearest => {
                let y = cy0 + ((2 * oy + 1) * chei) / (2 * dh);
                (y, y)
            }
        };
        let (ya, yb) = (ya.min(h - 1), yb.min(h - 1));
        let crow = (ya >> sys).min(ch - 1);
        let yrow_a = row_of(f.y, f.y_stride, ya);
        let yrow_b = row_of(f.y, f.y_stride, yb);
        let urow = row_of(f.u, f.uv_stride, crow);
        let vrow = row_of(f.v, f.uv_stride, crow);
        // Fast path: 1:1, horizontally subsampled chroma aligned to a pair.
        if mode == Sampling::Direct && !mono && sxs == 1 && cx0 % 2 == 0 && yrow_a.len() >= cx0 + dw {
            let need = dw.div_ceil(2);
            let (u0, u1) = (cx0 / 2, cx0 / 2 + need);
            if urow.len() >= u1 && vrow.len() >= u1 {
                row_pairs::<S, RGBA>(&yrow_a[cx0..cx0 + dw], &urow[u0..u1], &vrow[u0..u1], line, c, shift);
                continue;
            }
        }
        let luma = |row: &[S], x: usize| row.get(x).map_or(0, |v| v.get() >> shift);
        let mut xfp = ((cx0 as u64) << STEP_BITS) + x_step / 2;
        for ox in 0..dw {
            let sx = match mode {
                Sampling::Direct => cx0 + ox,
                Sampling::Half => cx0 + ox * 2,
                Sampling::Nearest => {
                    let x = (xfp >> STEP_BITS) as usize;
                    xfp += x_step;
                    x
                }
            }
            .min(w - 1);
            let l = if mode == Sampling::Half {
                let x1 = (sx + 1).min(w - 1);
                (luma(yrow_a, sx) + luma(yrow_a, x1) + luma(yrow_b, sx) + luma(yrow_b, x1) + 2) >> 2
            } else {
                luma(yrow_a, sx)
            };
            let yv = (l - c.y_off) * c.y_mul + (1 << (Q - 1));
            let px = &mut line[ox * 4..ox * 4 + 4];
            if mono {
                let g = clamp8(yv);
                put::<RGBA>(px, g, g, g);
                continue;
            }
            let cx = (sx >> sxs).min(cw - 1);
            let u = luma(urow, cx) - 128;
            let v = luma(vrow, cx) - 128;
            put::<RGBA>(px, clamp8(yv + c.bu * u), clamp8(yv - c.gu * u - c.gv * v), clamp8(yv + c.rv * v));
        }
    }
}

/// Row `y` of a plane onward (empty past the end).
#[inline(always)]
fn row_of<S>(plane: &[S], stride: usize, y: usize) -> &[S] {
    &plane[(y * stride).min(plane.len())..]
}

/// One row, two luma samples per chroma sample (4:2:0 / 4:2:2, 1:1): no
/// bounds checks in the loop body, so it vectorizes.
#[inline(always)]
fn row_pairs<S: Sample, const RGBA: bool>(y: &[S], u: &[S], v: &[S], out: &mut [u8], c: Coeffs, shift: u32) {
    let one = |yv: S, ruv: i32, guv: i32, buv: i32, px: &mut [u8]| {
        let yv = ((yv.get() >> shift) - c.y_off) * c.y_mul + (1 << (Q - 1));
        put::<RGBA>(px, clamp8(yv + buv), clamp8(yv - guv), clamp8(yv + ruv));
    };
    let pairs = y.len() / 2;
    for (((o, yy), uu), vv) in out[..pairs * 8].chunks_exact_mut(8).zip(y.chunks_exact(2)).zip(u.iter()).zip(v.iter()) {
        let (cu, cv) = ((uu.get() >> shift) - 128, (vv.get() >> shift) - 128);
        let (ruv, guv, buv) = (c.rv * cv, c.gu * cu + c.gv * cv, c.bu * cu);
        let (a, b) = o.split_at_mut(4);
        one(yy[0], ruv, guv, buv, a);
        one(yy[1], ruv, guv, buv, b);
    }
    if y.len() % 2 == 1
        && let (Some(yv), Some(uu), Some(vv)) = (y.last(), u.get(pairs), v.get(pairs))
    {
        let (cu, cv) = ((uu.get() >> shift) - 128, (vv.get() >> shift) - 128);
        let n = y.len();
        one(*yv, c.rv * cv, c.gu * cu + c.gv * cv, c.bu * cu, &mut out[(n - 1) * 4..n * 4]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame<'a>(y: &'a [u8], u: &'a [u8], v: &'a [u8], w: usize, h: usize, full: bool, m: Matrix) -> YuvFrame<'a, u8> {
        YuvFrame { width: w, height: h, layout: Layout::I420, y, u, v, y_stride: w, uv_stride: w / 2, bits: 8, full_range: full, matrix: m }
    }

    #[test]
    fn limited_range_black_white_and_gray() {
        for (luma, want) in [(16u8, 0u8), (235, 255), (126, 128)] {
            let y = vec![luma; 16];
            let c = vec![128u8; 4];
            let (out, w, h) = to_bgra(&frame(&y, &c, &c, 4, 4, false, Matrix::Bt709), false);
            assert_eq!((w, h), (4, 4));
            for px in out.chunks_exact(4) {
                for ch in &px[..3] {
                    assert!((*ch as i32 - want as i32).abs() <= 1, "Y={luma}: got {px:?}, want {want}");
                }
                assert_eq!(px[3], 255);
            }
        }
    }

    #[test]
    fn bt709_primaries_round_trip() {
        // Encode pure colours with the BT.709 limited-range forward matrix
        // (the same one recording.rs uses) and convert back.
        let enc = |r: i32, g: i32, b: i32| -> (u8, u8, u8) {
            let y = ((47 * r + 157 * g + 16 * b + 128) >> 8) + 16;
            let u = ((-26 * r - 87 * g + 112 * b + 128) >> 8) + 128;
            let v = ((112 * r - 102 * g - 10 * b + 128) >> 8) + 128;
            (y as u8, u as u8, v as u8)
        };
        for rgb in [(255, 0, 0), (0, 255, 0), (0, 0, 255), (200, 120, 40), (10, 80, 160)] {
            let (y, u, v) = enc(rgb.0, rgb.1, rgb.2);
            let (yp, up, vp) = (vec![y; 4], vec![u; 1], vec![v; 1]);
            let (out, ..) = to_bgra(&frame(&yp, &up, &vp, 2, 2, false, Matrix::Bt709), false);
            let (b, g, r) = (out[0] as i32, out[1] as i32, out[2] as i32);
            assert!((r - rgb.0).abs() <= 6 && (g - rgb.1).abs() <= 6 && (b - rgb.2).abs() <= 6, "{rgb:?} → ({r}, {g}, {b})");
        }
    }

    #[test]
    fn full_range_and_bt601() {
        let y = vec![255u8; 4];
        let c = vec![128u8; 1];
        let (out, ..) = to_bgra(&frame(&y, &c, &c, 2, 2, true, Matrix::Bt601), false);
        assert_eq!(&out[..4], &[255, 255, 255, 255]);
        // Strong red in BT.601 full range: Y=76, U=85, V=255.
        let (y, u, v) = (vec![76u8; 4], vec![85u8; 1], vec![255u8; 1]);
        let (out, ..) = to_bgra(&frame(&y, &u, &v, 2, 2, true, Matrix::Bt601), false);
        assert!(out[2] >= 250 && out[1] <= 5 && out[0] <= 5, "{:?}", &out[..4]);
    }

    #[test]
    fn half_resolution_averages_luma() {
        // 4×2 luma: left 2×2 block black, right block white.
        let y = [16u8, 16, 235, 235, 16, 16, 235, 235];
        let c = [128u8; 2];
        let f = YuvFrame { width: 4, height: 2, layout: Layout::I420, y: &y, u: &c, v: &c, y_stride: 4, uv_stride: 2, bits: 8, full_range: false, matrix: Matrix::Bt709 };
        let (out, w, h) = to_bgra(&f, true);
        assert_eq!((w, h), (2, 1));
        assert!(out[0] <= 1 && out[4] >= 254, "{out:?}");
    }

    #[test]
    fn ten_bit_samples_are_normalized() {
        let y = vec![940u16; 4]; // 10-bit limited white
        let c = vec![512u16; 1];
        let f = YuvFrame { width: 2, height: 2, layout: Layout::I420, y: &y, u: &c, v: &c, y_stride: 2, uv_stride: 1, bits: 10, full_range: false, matrix: Matrix::Bt2020 };
        let (out, ..) = to_bgra(&f, false);
        assert!(out[..3].iter().all(|v| *v >= 254), "{:?}", &out[..4]);
    }

    #[test]
    fn large_frames_convert_in_bands_identically() {
        let (w, h) = (640usize, 480usize);
        let y: Vec<u8> = (0..w * h).map(|i| (16 + (i * 7) % 219) as u8).collect();
        let u: Vec<u8> = (0..w * h / 4).map(|i| (16 + (i * 3) % 224) as u8).collect();
        let v: Vec<u8> = (0..w * h / 4).map(|i| (16 + (i * 5) % 224) as u8).collect();
        let f = frame(&y, &u, &v, w, h, false, Matrix::Bt709);
        let (banded, ..) = to_bgra(&f, false);
        let mut single = vec![0u8; w * h * 4];
        convert_rows(&f, false, Coeffs::new(Matrix::Bt709, false), 0, &mut single, w);
        assert_eq!(banded, single);
    }

    fn planes(w: usize, h: usize) -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
        let y: Vec<u8> = (0..w * h).map(|i| (16 + (i * 7 + i / w * 3) % 219) as u8).collect();
        let u: Vec<u8> = (0..cw * ch).map(|i| (16 + (i * 3) % 224) as u8).collect();
        let v: Vec<u8> = (0..cw * ch).map(|i| (16 + (i * 5) % 224) as u8).collect();
        (y, u, v)
    }

    fn yuv<'a>(y: &'a [u8], u: &'a [u8], v: &'a [u8], w: usize, h: usize) -> YuvFrame<'a, u8> {
        YuvFrame { width: w, height: h, layout: Layout::I420, y, u, v, y_stride: w, uv_stride: w.div_ceil(2), bits: 8, full_range: false, matrix: Matrix::Bt709 }
    }

    fn region(f: &YuvFrame<'_, u8>, crop: Crop, dw: u32, dh: u32, order: PixelOrder, threads: usize) -> Vec<u8> {
        let mut data = vec![0u8; dw as usize * dh as usize * 4];
        convert_region(f, crop, &mut Target { data: &mut data, width: dw, height: dh, stride: dw as usize * 4, order }, threads);
        data
    }

    #[test]
    fn region_direct_matches_whole_frame_conversion() {
        // Odd sizes exercise the pair loop's tail.
        for (w, h) in [(64usize, 48usize), (33, 17), (2, 2)] {
            let (y, u, v) = planes(w, h);
            let f = yuv(&y, &u, &v, w, h);
            let (whole, ..) = to_bgra(&f, false);
            let full = Crop { x: 0, y: 0, w: w as u32, h: h as u32 };
            assert_eq!(region(&f, full, w as u32, h as u32, PixelOrder::Bgra, 1), whole, "{w}x{h}");
        }
    }

    #[test]
    fn region_crop_is_a_window_of_the_frame() {
        let (w, h) = (40usize, 30usize);
        let (y, u, v) = planes(w, h);
        let f = yuv(&y, &u, &v, w, h);
        let (whole, ..) = to_bgra(&f, false);
        // Even and odd x offsets (odd takes the generic path).
        for x0 in [4usize, 5] {
            let (y0, cw, chh) = (6usize, 20usize, 12usize);
            let got = region(&f, Crop { x: x0 as u32, y: y0 as u32, w: cw as u32, h: chh as u32 }, cw as u32, chh as u32, PixelOrder::Bgra, 1);
            for r in 0..chh {
                let want = &whole[((y0 + r) * w + x0) * 4..((y0 + r) * w + x0 + cw) * 4];
                assert_eq!(&got[r * cw * 4..(r + 1) * cw * 4], want, "x0={x0} row {r}");
            }
        }
    }

    #[test]
    fn region_rgba_order_swaps_red_and_blue() {
        let (y, u, v) = planes(8, 8);
        let f = yuv(&y, &u, &v, 8, 8);
        let full = Crop { x: 0, y: 0, w: 8, h: 8 };
        let bgra = region(&f, full, 8, 8, PixelOrder::Bgra, 1);
        let rgba = region(&f, full, 8, 8, PixelOrder::Rgba, 1);
        for (a, b) in bgra.chunks_exact(4).zip(rgba.chunks_exact(4)) {
            assert_eq!([a[2], a[1], a[0], a[3]], [b[0], b[1], b[2], b[3]]);
        }
    }

    #[test]
    fn region_half_matches_half_conversion() {
        let (w, h) = (64usize, 40usize);
        let (y, u, v) = planes(w, h);
        let f = yuv(&y, &u, &v, w, h);
        let (half, hw, hh) = to_bgra(&f, true);
        assert_eq!(sampling(Crop { x: 0, y: 0, w: 64, h: 40 }, 32, 20), Sampling::Half);
        assert_eq!(region(&f, Crop { x: 0, y: 0, w: w as u32, h: h as u32 }, hw as u32, hh as u32, PixelOrder::Bgra, 1), half);
    }

    #[test]
    fn region_nearest_upscale_repeats_pixels() {
        let (w, h) = (4usize, 4usize);
        let (y, u, v) = planes(w, h);
        let f = yuv(&y, &u, &v, w, h);
        let full = Crop { x: 0, y: 0, w: 4, h: 4 };
        let src = region(&f, full, 4, 4, PixelOrder::Bgra, 1);
        let big = region(&f, full, 8, 8, PixelOrder::Bgra, 1);
        assert_eq!(sampling(full, 8, 8), Sampling::Nearest);
        for oy in 0..8 {
            for ox in 0..8 {
                let (sx, sy) = (ox / 2, oy / 2);
                assert_eq!(&big[(oy * 8 + ox) * 4..(oy * 8 + ox) * 4 + 4], &src[(sy * 4 + sx) * 4..(sy * 4 + sx) * 4 + 4], "({ox}, {oy})");
            }
        }
    }

    #[test]
    fn region_bands_and_stride_padding() {
        let (w, h) = (96usize, 70usize);
        let (y, u, v) = planes(w, h);
        let f = yuv(&y, &u, &v, w, h);
        let full = Crop { x: 0, y: 0, w: w as u32, h: h as u32 };
        let single = region(&f, full, 48, 35, PixelOrder::Rgba, 1);
        assert_eq!(region(&f, full, 48, 35, PixelOrder::Rgba, 4), single);
        // Padded rows: the padding is left alone.
        let stride = 48 * 4 + 12;
        let mut data = vec![7u8; stride * 35];
        convert_region(&f, full, &mut Target { data: &mut data, width: 48, height: 35, stride, order: PixelOrder::Rgba }, 3);
        for r in 0..35 {
            assert_eq!(&data[r * stride..r * stride + 48 * 4], &single[r * 48 * 4..(r + 1) * 48 * 4]);
            assert!(data[r * stride + 48 * 4..(r + 1) * stride].iter().all(|b| *b == 7));
        }
    }

    #[test]
    fn region_ignores_undersized_targets_and_clamps_crops() {
        let (y, u, v) = planes(8, 8);
        let f = yuv(&y, &u, &v, 8, 8);
        let mut tiny = vec![1u8; 4];
        convert_region(&f, Crop { x: 0, y: 0, w: 8, h: 8 }, &mut Target { data: &mut tiny, width: 8, height: 8, stride: 32, order: PixelOrder::Bgra }, 1);
        assert_eq!(tiny, [1, 1, 1, 1]);
        assert_eq!(Crop { x: 6, y: 7, w: 10, h: 10 }.clamp_to(8, 8), Crop { x: 6, y: 7, w: 2, h: 1 });
    }
}
