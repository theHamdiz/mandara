//! Procedural demo images: mesh-gradient and layered-landscape wallpapers,
//! sunset album covers. Deterministic (seeded grain), rendered row-parallel.

use image::{Rgb, RgbImage};

/// Linear RGB-ish working color, 0–255 per channel.
pub type Color = [f32; 3];

/// Standard deviation of the anti-banding grain, in 8-bit steps.
const GRAIN: f32 = 1.2;
/// Radius of a landscape's sun glow (fraction of the height).
const GLOW_RADIUS: f32 = 0.22;
/// How strongly the sun glow tints the sky.
const GLOW_STRENGTH: f32 = 0.8;
/// How much a ridge darkens with depth below its crest.
const RIDGE_SHADE: f32 = 0.12;
/// Relative amplitude and frequency of a ridge's second harmonic.
const HARMONIC_AMP: f32 = 0.45;
const HARMONIC_FREQ: f32 = 2.3;
const HARMONIC_PHASE: f32 = 1.7;
/// Uniform samples summed per grain value (Irwin–Hall ≈ normal).
const GRAIN_SAMPLES: u32 = 4;

/// `#rrggbb` → color.
pub const fn hex(s: &str) -> Color {
    let b = s.as_bytes();
    const fn nib(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => 0,
        }
    }
    const fn ch(b: &[u8], i: usize) -> f32 {
        (nib(b[i]) * 16 + nib(b[i + 1])) as f32
    }
    [ch(b, 1), ch(b, 3), ch(b, 5)]
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}

fn scale(c: Color, k: f32) -> Color {
    [c[0] * k, c[1] * k, c[2] * k]
}

/// SplitMix64 step.
fn splitmix(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Approximately standard-normal noise for a pixel.
fn grain(x: u32, y: u32, seed: u64) -> f32 {
    let mut h = splitmix(seed ^ (u64::from(y) << 32 | u64::from(x)));
    let mut sum = 0.0;
    for _ in 0..GRAIN_SAMPLES {
        h = splitmix(h);
        sum += (h >> 40) as f32 / (1u64 << 24) as f32;
    }
    // Irwin–Hall: mean n/2, variance n/12.
    (sum - GRAIN_SAMPLES as f32 / 2.0) * (12.0 / GRAIN_SAMPLES as f32).sqrt()
}

/// Renders `f(u, v)` (normalized coordinates) plus grain into an image,
/// rows split across threads.
fn render(width: u32, height: u32, seed: u64, f: impl Fn(f32, f32) -> Color + Sync) -> RgbImage {
    let mut img = RgbImage::new(width, height);
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let row_bytes = width as usize * 3;
    let rows_per = (height as usize).div_ceil(threads).max(1);
    std::thread::scope(|s| {
        for (chunk_i, chunk) in img.chunks_mut(rows_per * row_bytes).enumerate() {
            let f = &f;
            s.spawn(move || {
                for (r, row) in chunk.chunks_mut(row_bytes).enumerate() {
                    let y = (chunk_i * rows_per + r) as u32;
                    let v = y as f32 / height as f32;
                    for (x, px) in row.chunks_mut(3).enumerate() {
                        let x = x as u32;
                        let c = f(x as f32 / width as f32, v);
                        let n = grain(x, y, seed) * GRAIN;
                        for i in 0..3 {
                            px[i] = (c[i] + n).clamp(0.0, 255.0) as u8;
                        }
                    }
                }
            });
        }
    });
    img
}

/// A soft color blob of a mesh gradient.
pub struct Blob {
    pub x: f32,
    pub y: f32,
    pub radius: f32,
    pub color: Color,
    pub alpha: f32,
}

/// A mesh gradient: a base color with gaussian blobs blended over it.
pub fn mesh(width: u32, height: u32, base: Color, blobs: &[Blob], seed: u64) -> RgbImage {
    let aspect = width as f32 / height as f32;
    render(width, height, seed, |u, v| {
        let mut c = base;
        for b in blobs {
            let d2 = ((u - b.x) * aspect).powi(2) + (v - b.y).powi(2);
            let w = b.alpha * (-d2 / (2.0 * b.radius * b.radius)).exp();
            c = mix(c, b.color, w);
        }
        c
    })
}

/// One ridge of a landscape: a crest line `base + amp·sin(…)` filled below.
pub struct Ridge {
    pub base: f32,
    pub amp: f32,
    pub freq: f32,
    pub phase: f32,
    pub color: Color,
}

/// A sky gradient with a sun glow and ridges in front.
pub fn landscape(width: u32, height: u32, sky: (Color, Color), sun: Color, sun_at: (f32, f32), ridges: &[Ridge], seed: u64) -> RgbImage {
    let aspect = width as f32 / height as f32;
    let tau = std::f32::consts::TAU;
    render(width, height, seed, |u, v| {
        let mut c = mix(sky.0, sky.1, v);
        let d = ((u - sun_at.0) * aspect).hypot(v - sun_at.1) / GLOW_RADIUS;
        c = mix(c, sun, (-d * d).exp() * GLOW_STRENGTH);
        for r in ridges {
            let crest = r.base + r.amp * (u * r.freq * tau + r.phase).sin() + r.amp * HARMONIC_AMP * (u * r.freq * HARMONIC_FREQ * tau + r.phase * HARMONIC_PHASE).sin();
            if v > crest {
                c = scale(r.color, 1.0 - RIDGE_SHADE * (v - crest).clamp(0.0, 1.0));
            }
        }
        c
    })
}

/// Album covers are drawn at this multiple of their size, then downsampled.
const COVER_SUPERSAMPLE: u32 = 2;
/// Horizon height of a cover (fraction of its size).
const COVER_HORIZON: f32 = 0.6;
/// Where the sky gradient turns from its top to its middle color.
const SKY_SPLIT: f32 = 0.7;
/// Sun radius (fraction of the size) and colors.
const SUN_RADIUS: f32 = 0.17;
const SUN: Color = [255.0, 226.0, 150.0];
const SUN_GLOW: Color = [255.0, 214.0, 150.0];
/// The glow: an ellipse centre offset above the horizon, its radius, edge
/// softness and opacity.
const GLOW_LIFT: f32 = 0.08;
const GLOW_ELLIPSE: f32 = 0.28;
const GLOW_SOFTNESS: f32 = 0.35;
const GLOW_ALPHA: f32 = 150.0 / 255.0;
/// The sea: base color, wave bands and their spacing / height.
const SEA: Color = [18.0, 60.0, 110.0];
const WAVES: u32 = 9;
const WAVE_SPACING: f32 = 0.045;
const WAVE_HEIGHT: f32 = 0.012;
/// The first wave band starts this many pixels below the horizon.
const WAVE_OFFSET_PX: f32 = 4.0;
/// Sun reflections: count, spacing, colour and thickness.
const REFLECTIONS: u32 = 6;
const REFLECTION_SPACING: f32 = 0.03;
const REFLECTION: Color = [255.0, 200.0, 130.0];
const REFLECTION_THICKNESS: f32 = 0.008;
/// Thinnest reflection line, in pixels.
const REFLECTION_MIN_PX: f32 = 2.0;
/// The first reflection lies this many pixels below the horizon.
const REFLECTION_START_PX: f32 = 10.0;
/// Half-width of the first reflection (in sun radii) and its taper per line.
const REFLECTION_WIDTH: f32 = 1.2;
const REFLECTION_TAPER: f32 = 0.17;

/// Color of wave band `i` (alternating deep and lighter water).
fn wave_color(i: u32) -> Color {
    let even = i.is_multiple_of(2);
    let i = i as f32;
    if even { [20.0 + i * 2.0, 90.0 - i * 6.0, 140.0 - i * 8.0] } else { [12.0, 40.0 + i * 2.0, 90.0 - i * 4.0] }
}

/// A sunset-over-the-sea album cover in three sky colors.
pub fn cover(size: u32, sky: [Color; 3]) -> RgbImage {
    let s = size * COVER_SUPERSAMPLE;
    let sf = s as f32;
    let hz = sf * COVER_HORIZON;
    let r = sf * SUN_RADIUS;
    let tau = std::f32::consts::TAU;
    let big = render(s, s, u64::from(size), |u, v| {
        let (x, y) = (u * sf, v * sf);
        if y < hz {
            let t = y / hz;
            let mut c = if t < SKY_SPLIT { mix(sky[0], sky[1], t / SKY_SPLIT) } else { mix(sky[1], sky[2], (t - SKY_SPLIT) / (1.0 - SKY_SPLIT)) };
            let d = ((x - sf / 2.0).hypot(y - (hz - sf * GLOW_LIFT))) / (sf * GLOW_ELLIPSE);
            let edge = ((d - (1.0 - GLOW_SOFTNESS)) / (2.0 * GLOW_SOFTNESS)).clamp(0.0, 1.0);
            let glow = GLOW_ALPHA * (1.0 - edge * edge * (3.0 - 2.0 * edge));
            c = mix(c, SUN_GLOW, glow);
            if (x - sf / 2.0).hypot(y - hz) <= r {
                c = SUN;
            }
            return c;
        }
        let mut c = SEA;
        for i in 0..WAVES {
            let fi = i as f32;
            let top = hz + fi * sf * WAVE_SPACING + WAVE_OFFSET_PX + (x / sf * tau * (2 + i % 3) as f32 + fi).sin() * sf * WAVE_HEIGHT;
            if y >= top {
                c = wave_color(i);
            }
        }
        let thick = (sf * REFLECTION_THICKNESS).max(REFLECTION_MIN_PX);
        for i in 0..REFLECTIONS {
            let ry = hz + REFLECTION_START_PX + i as f32 * sf * REFLECTION_SPACING;
            let half = r * (REFLECTION_WIDTH - i as f32 * REFLECTION_TAPER);
            if (y - ry).abs() <= thick / 2.0 && (x - sf / 2.0).abs() <= half {
                c = REFLECTION;
            }
        }
        c
    });
    image::imageops::resize(&big, size, size, image::imageops::FilterType::Lanczos3)
}

/// A solid image (matte backgrounds).
pub fn solid(width: u32, height: u32, value: u8) -> RgbImage {
    RgbImage::from_pixel(width, height, Rgb([value; 3]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex() {
        assert_eq!(hex("#04111f"), [4.0, 17.0, 31.0]);
        assert_eq!(hex("#FFFFFF"), [255.0, 255.0, 255.0]);
    }

    #[test]
    fn grain_is_roughly_standard_normal() {
        let n = 20_000u32;
        let samples: Vec<f32> = (0..n).map(|i| grain(i, i / 7, 3)).collect();
        let mean = samples.iter().sum::<f32>() / n as f32;
        let var = samples.iter().map(|s| (s - mean).powi(2)).sum::<f32>() / n as f32;
        assert!(mean.abs() < 0.05, "mean {mean}");
        assert!((var - 1.0).abs() < 0.1, "variance {var}");
    }

    #[test]
    fn images_are_deterministic_and_sized() {
        let blobs = [Blob { x: 0.3, y: 0.3, radius: 0.3, color: hex("#0d7a74"), alpha: 0.85 }];
        let a = mesh(64, 36, hex("#062a2c"), &blobs, 7);
        assert_eq!(a, mesh(64, 36, hex("#062a2c"), &blobs, 7));
        assert_eq!(a.dimensions(), (64, 36));
        let c = cover(32, [hex("#3a2878"), hex("#ee786e"), hex("#ffc478")]);
        assert_eq!(c.dimensions(), (32, 32));
    }
}
