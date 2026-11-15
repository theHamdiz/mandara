//! Renders the moon-phase example's moon: one image per phase for each
//! hemisphere (`../assets/moon-n-NN.png`, `moon-s-NN.png`). Original,
//! procedural art: the near side's seas placed where they are on the real
//! moon, a few named craters and a field of small ones as a height map, lit
//! by the sun for each phase with relief along the terminator, warm
//! moonlight on the lit side and faint earthshine on the dark side.
//!
//! `cargo run --release` in this folder rewrites the images.

use std::f32::consts::PI;
use std::path::Path;

/// Image side (px) and supersamples per axis.
const SIZE: u32 = 176;
const SS: u32 = 3;
/// Phases rendered across one synodic month.
const PHASES: usize = 24;

/// A sea: centre (longitude, latitude) and radii, in degrees.
const SEAS: &[(f32, f32, f32, f32)] = &[
    (-55.0, 15.0, 24.0, 34.0), // Oceanus Procellarum
    (-17.0, 33.0, 20.0, 15.0), // Mare Imbrium
    (17.0, 28.0, 10.5, 10.0),  // Mare Serenitatis
    (30.0, 8.0, 14.5, 11.0),   // Mare Tranquillitatis
    (59.0, 17.0, 7.5, 6.5),    // Mare Crisium
    (51.0, -7.0, 8.5, 12.0),   // Mare Fecunditatis
    (35.0, -15.0, 5.5, 5.5),   // Mare Nectaris
    (-17.0, -21.0, 13.0, 10.0),// Mare Nubium
    (-39.0, -24.0, 5.5, 5.5),  // Mare Humorum
    (-8.0, 56.0, 22.0, 4.0),   // Mare Frigoris (west)
    (18.0, 58.0, 14.0, 3.5),   // Mare Frigoris (east)
    (4.0, 13.0, 6.0, 4.5),     // Mare Vaporum
    (-28.0, -7.0, 10.0, 7.0),  // Mare Cognitum / Insularum
    (2.0, 3.0, 4.0, 3.0),      // Sinus Medii
];

/// Named craters: (longitude, latitude, radius in degrees, rays).
const NAMED: &[(f32, f32, f32, bool)] = &[
    (-11.0, -43.0, 2.6, true),  // Tycho
    (-20.0, 10.0, 2.9, true),   // Copernicus
    (-38.0, 8.0, 1.5, true),    // Kepler
    (-47.0, 24.0, 1.1, true),   // Aristarchus
    (-9.0, 51.0, 3.2, false),   // Plato
    (-14.0, -58.0, 6.5, false), // Clavius
    (-3.0, -16.0, 4.0, false),  // Ptolemaeus
    (26.0, -11.0, 3.0, false),  // Theophilus
];

/// Highland and sea colours (linear), moonlight's warmth and earthshine.
const HIGHLAND: [f32; 3] = [0.88, 0.80, 0.63];
const SEA: [f32; 3] = [0.25, 0.25, 0.27];
const SUNLIGHT: [f32; 3] = [1.15, 1.08, 0.92];
const EARTHSHINE: [f32; 3] = [0.20, 0.26, 0.36];
/// The dark side's base colour and how much earthshine reveals of the seas.
const NIGHT: [f32; 3] = [0.010, 0.013, 0.019];
const EARTHSHINE_GAIN: f32 = 0.075;
/// Exposure of the tone curve (higher is brighter).
const TONE: f32 = 1.35;
/// How far seas spread past their outlines.
const SEA_SPREAD: f32 = 1.18;
/// Relief strength (normal tilt per unit height slope).
const RELIEF: f32 = 1.6;

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Deterministic hash noise.
fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263) ^ seed.wrapping_mul(2_246_822_519);
    h = (h ^ (h >> 13)).wrapping_mul(1_274_126_177);
    ((h ^ (h >> 16)) & 0xffff) as f32 / 65_535.0
}

fn value_noise(x: f32, y: f32, seed: u32) -> f32 {
    let (xi, yi) = (x.floor() as i32, y.floor() as i32);
    let (fx, fy) = (x - x.floor(), y - y.floor());
    let (sx, sy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let a = hash(xi, yi, seed) + (hash(xi + 1, yi, seed) - hash(xi, yi, seed)) * sx;
    let b = hash(xi, yi + 1, seed) + (hash(xi + 1, yi + 1, seed) - hash(xi, yi + 1, seed)) * sx;
    a + (b - a) * sy
}

fn fbm(x: f32, y: f32, seed: u32, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut freq, mut norm) = (0.0, 1.0, 1.0, 0.0);
    for o in 0..octaves {
        sum += amp * value_noise(x * freq, y * freq, seed + o);
        norm += amp;
        amp *= 0.5;
        freq *= 2.03;
    }
    sum / norm
}

struct Crater {
    lon: f32,
    lat: f32,
    r: f32,
    rays: bool,
}

/// The surface: sea fraction, height and albedo at (longitude, latitude).
struct Surface {
    craters: Vec<Crater>,
}

impl Surface {
    fn new() -> Self {
        let mut craters: Vec<Crater> = NAMED.iter().map(|&(lon, lat, r, rays)| Crater { lon, lat, r, rays }).collect();
        // A field of small craters, mostly in the highlands.
        let mut seed = 0x9e37_79b9_u32;
        let mut next = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed as f32 / u32::MAX as f32
        };
        while craters.len() < 190 {
            let (lon, lat) = (next() * 180.0 - 90.0, (next() * 2.0 - 1.0).asin().to_degrees());
            let r = 0.3 + 2.2 * next().powf(4.0);
            let s = Self { craters: Vec::new() };
            if s.sea(lon, lat) > 0.6 && next() > 0.25 {
                continue;
            }
            craters.push(Crater { lon, lat, r, rays: false });
        }
        Self { craters }
    }

    /// 0 in the highlands, 1 inside a sea, with ragged shores.
    fn sea(&self, lon: f32, lat: f32) -> f32 {
        let warp = (fbm(lon * 0.05, lat * 0.05, 7, 5) - 0.5) * 1.3 + (fbm(lon * 0.12, lat * 0.12, 9, 4) - 0.5) * 0.25;
        let mut m: f32 = 0.0;
        for &(clon, clat, rx, ry) in SEAS {
            // A little larger than the outlines above, so neighbouring
            // seas join as they do on the real moon.
            let dx = (lon - clon) / (rx * SEA_SPREAD);
            let dy = (lat - clat) / (ry * SEA_SPREAD);
            let d = (dx * dx + dy * dy).sqrt() + warp;
            m = m.max(1.0 - smoothstep(0.62, 1.2, d));
        }
        m
    }

    /// Height and extra brightness (rays, fresh rims) from the craters.
    fn craters(&self, lon: f32, lat: f32) -> (f32, f32) {
        let (mut h, mut bright) = (0.0, 0.0);
        let coslat = lat.to_radians().cos().max(0.2);
        for c in &self.craters {
            let dx = (lon - c.lon) * coslat;
            let dy = lat - c.lat;
            let d = (dx * dx + dy * dy).sqrt() / c.r;
            if c.rays && d < 9.0 {
                // Bright ejecta rays fading with distance.
                let angle = dy.atan2(dx);
                let rays = (value_noise(angle * 5.0 + 11.0, c.r, 3) - 0.55).max(0.0) * 2.2;
                bright += rays * (1.0 - d / 9.0).powf(2.0) * 0.5 + (1.0 - smoothstep(0.9, 2.2, d)) * 0.35;
            }
            if d > 1.6 {
                continue;
            }
            // A bowl with a raised rim, fading out into the ejecta.
            let bowl = if d < 1.0 { (d * d - 1.0) * 0.45 } else { 0.0 };
            let rim = (-((d - 1.0) / 0.28).powi(2)).exp() * 0.22;
            h += (bowl + rim) * c.r * 0.045;
        }
        (h, bright)
    }

    fn height(&self, lon: f32, lat: f32) -> f32 {
        let sea = self.sea(lon, lat);
        let rough = (fbm(lon * 0.35, lat * 0.35, 21, 5) - 0.5) * 0.05 * (1.0 - 0.7 * sea);
        self.craters(lon, lat).0 + rough - sea * 0.008
    }

    fn albedo(&self, lon: f32, lat: f32) -> [f32; 3] {
        let sea = self.sea(lon, lat);
        let (_, bright) = self.craters(lon, lat);
        let grain = 0.93 + 0.14 * fbm(lon * 0.9, lat * 0.9, 41, 4);
        let shade = 0.8 + 0.4 * fbm(lon * 0.06, lat * 0.06, 53, 4);
        // The seas' own darker and lighter lavas.
        let lava = 0.85 + 0.3 * fbm(lon * 0.15, lat * 0.15, 61, 3);
        let mut c = [0.0; 3];
        for i in 0..3 {
            c[i] = ((HIGHLAND[i] * (1.0 - sea) * shade + SEA[i] * sea * lava) * grain + bright * 0.45).min(1.2);
        }
        c
    }
}

/// Latitude / longitude (degrees) of the visible point at (x, y), z > 0.
fn lonlat(x: f32, y: f32, z: f32) -> (f32, f32) {
    (x.atan2(z).to_degrees(), y.clamp(-1.0, 1.0).asin().to_degrees())
}

fn to_srgb(v: f32) -> u8 {
    let v = v.clamp(0.0, 1.0);
    let s = if v <= 0.003_130_8 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
    (s * 255.0).round() as u8
}

/// The moon `phase` (0..1 of a synodic month) seen from the north.
fn render(surface: &Surface, phase: f32) -> image::RgbaImage {
    let angle = phase * 2.0 * PI;
    let sun = [angle.sin(), 0.0, -angle.cos()];
    let mut img = image::RgbaImage::new(SIZE, SIZE);
    let step = 2.0 / (SIZE * SS) as f32;
    let e = step;
    for py in 0..SIZE {
        for px in 0..SIZE {
            let (mut rgb, mut alpha) = ([0.0f32; 3], 0.0f32);
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = ((px * SS + sx) as f32 + 0.5) * step - 1.0;
                    let y = 1.0 - ((py * SS + sy) as f32 + 0.5) * step;
                    let r2 = x * x + y * y;
                    if r2 >= 1.0 {
                        continue;
                    }
                    let z = (1.0 - r2).sqrt();
                    let (lon, lat) = lonlat(x, y, z);
                    // Relief: tilt the sphere's normal by the height slope.
                    let h = surface.height(lon, lat);
                    let (lx, ly) = lonlat((x + e).min(0.999), y, (1.0 - ((x + e).min(0.999)).powi(2) - y * y).max(1e-4).sqrt());
                    let (ux, uy) = lonlat(x, (y + e).min(0.999), (1.0 - x * x - ((y + e).min(0.999)).powi(2)).max(1e-4).sqrt());
                    let hx = (surface.height(lx, ly) - h) / e;
                    let hy = (surface.height(ux, uy) - h) / e;
                    let mut n = [x - RELIEF * hx * z, y - RELIEF * hy * z, z];
                    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                    n.iter_mut().for_each(|v| *v /= len);
                    let mu0_smooth = x * sun[0] + y * sun[1] + z * sun[2];
                    // Relief shows where the sun grazes (near quarter and along
                    // the terminator) and fades out towards full moon.
                    let grazing = 1.0 - angle.cos().abs().powf(0.5) * (-angle.cos()).max(0.0).powf(0.25);
                    let tilted = n[0] * sun[0] + n[1] * sun[1] + n[2] * sun[2];
                    let mu0 = (mu0_smooth + (tilted - mu0_smooth) * (0.2 + 0.8 * grazing)).max(0.0);
                    // Lommel-Seeliger with a little Lambert: the moon stays
                    // bright to its limb but shows relief near the terminator.
                    let ls = if mu0 > 0.0 { 2.0 * mu0 / (mu0 + z) } else { 0.0 };
                    let light = (0.62 * ls + 0.38 * mu0 * 1.6) * smoothstep(-0.035, 0.05, mu0_smooth);
                    let albedo = surface.albedo(lon, lat);
                    for i in 0..3 {
                        // A soft shoulder: bright highlands roll off instead of clipping.
                        let day = 1.0 - (-(albedo[i] * SUNLIGHT[i] * light) * TONE).exp();
                        let night = NIGHT[i] + albedo[i] * EARTHSHINE[i] * EARTHSHINE_GAIN;
                        rgb[i] += day.max(night);
                    }
                    alpha += 1.0;
                }
            }
            let n = (SS * SS) as f32;
            if alpha > 0.0 {
                let a = alpha / n;
                let c = rgb.map(|v| v / alpha);
                img.put_pixel(px, py, image::Rgba([to_srgb(c[0]), to_srgb(c[1]), to_srgb(c[2]), (a * 255.0).round() as u8]));
            }
        }
    }
    img
}

fn main() {
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../assets");
    std::fs::create_dir_all(&out).expect("assets folder");
    let surface = Surface::new();
    for i in 0..PHASES {
        let north = render(&surface, i as f32 / PHASES as f32);
        // From the south the moon is upside down: the same view turned.
        let south = image::imageops::rotate180(&north);
        north.save(out.join(format!("moon-n-{i:02}.png"))).expect("write");
        south.save(out.join(format!("moon-s-{i:02}.png"))).expect("write");
        eprintln!("phase {i:02}");
    }
}
