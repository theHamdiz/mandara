//! The GPU wallpaper's transitions beyond a plain fade: every piece is a
//! clip of the old or new image layer (same texture, no copies), placed by
//! a pure function of the eased progress and a per-transition seed. Seams,
//! slices and shards come from an integer hash of their index, so nothing
//! is stored or allocated beyond the frame's own elements, and a
//! transition looks the same at 60 and 144 Hz. At progress 1 every
//! transition leaves exactly the new image in place.

use gpui::{AnyElement, IntoElement, ParentElement, Styled, div, px, rgba};
use mandara_config::desktop::WallpaperTransition;

use crate::style::ClipRounded as _;

/// Rows of a Tear, slices of a Glitch, columns of a Tide.
const TEAR_ROWS: u32 = 36;
const GLITCH_SLICES: u32 = 20;
const TIDE_COLUMNS: u32 = 32;
/// Shatter shards and Dissolve tiles, `(columns, rows)`.
const SHARDS: (u32, u32) = (8, 5);
const TILES: (u32, u32) = (16, 9);
/// How far a Tear's seam wanders from the middle, as a share of the width.
const TEAR_JAG: f32 = 0.06;
/// The rip spreads from the middle row outward over this share of the time.
const TEAR_SPREAD: f32 = 0.3;
/// Largest sideways shear of a Glitch slice, as a share of the width.
const GLITCH_SHEAR: f32 = 0.07;
/// A Glitch's colour split: offset (px) and tint alpha.
const GLITCH_SPLIT_PX: f32 = 6.0;
const GLITCH_SPLIT_ALPHA: f32 = 0.14;
/// A Tide's wave height, as a share of the height.
const TIDE_WAVE: f32 = 0.06;
/// Shards burst outward this far (px) and fall this share of the height.
const SHARD_BURST_PX: f32 = 140.0;
const SHARD_FALL: f32 = 0.7;
/// The last shard / tile starts this late in the transition.
const SHARD_DELAY: f32 = 0.4;
const TILE_DELAY: f32 = 0.75;
/// A Ripple's soft rim: width (px) and alpha.
const RIPPLE_RIM_PX: f32 = 2.0;
const RIPPLE_RIM_ALPHA: f32 = 0.35;

/// A stable pseudo-random value in `0..1` for piece `i` of a transition.
pub fn hash01(seed: u32, i: u32) -> f32 {
    let mut x = seed ^ i.wrapping_mul(0x9e37_79b9);
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^= x >> 16;
    (x >> 8) as f32 / (1u32 << 24) as f32
}

fn smooth(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// `layer` clipped to the rect `(x, y, w, h)` of the viewport, the clip
/// (and the image in it) moved by `(dx, dy)`.
pub(crate) fn piece(layer: AnyElement, (x, y, w, h): (f32, f32, f32, f32), (dx, dy): (f32, f32), (vw, vh): (f32, f32)) -> gpui::Div {
    div().absolute().left(px(x + dx)).top(px(y + dy)).w(px(w.max(0.0))).h(px(h.max(0.0))).overflow_hidden().child(div().absolute().left(px(-x)).top(px(-y)).w(px(vw)).h(px(vh)).child(layer))
}

/// The seam of a Tear at row `i`: x in px.
pub fn tear_seam(seed: u32, i: u32, vw: f32) -> f32 {
    let jag = (hash01(seed, i) - 0.5) * 2.0 * TEAR_JAG + (i as f32 * 0.9).sin() * TEAR_JAG * 0.35;
    vw * (0.5 + jag)
}

/// How far row `i` of a Tear has peeled at progress `t` (0..1).
pub fn tear_peel(t: f32, i: u32) -> f32 {
    let mid = (TEAR_ROWS - 1) as f32 / 2.0;
    let delay = (i as f32 - mid).abs() / mid * TEAR_SPREAD;
    smooth((t - delay) / (1.0 - TEAR_SPREAD))
}

/// A Glitch slice's sideways shift at progress `t`: it rises and falls over
/// the old image's half, then the new image's slices settle back to 0.
pub fn glitch_shift(seed: u32, i: u32, t: f32, vw: f32) -> (f32, f32) {
    let k = (hash01(seed, i) - 0.5) * 2.0 * GLITCH_SHEAR * vw;
    let old = k * (std::f32::consts::PI * (t / 0.55).clamp(0.0, 1.0)).sin();
    let settle = ((1.0 - t) / 0.5).clamp(0.0, 1.0);
    (old, -k * settle * settle)
}

/// A shard's `(dx, dy, opacity)` at progress `t`, bursting away from
/// `origin` and falling.
pub fn shard_motion(seed: u32, i: u32, centre: (f32, f32), origin: (f32, f32), (vw, vh): (f32, f32), t: f32) -> (f32, f32, f32) {
    let (ox, oy) = (centre.0 - origin.0, centre.1 - origin.1);
    let far = (vw * vw + vh * vh).sqrt().max(1.0);
    let dist = (ox * ox + oy * oy).sqrt();
    let delay = dist / far * SHARD_DELAY + hash01(seed, i) * 0.05;
    let u = ((t - delay) / (1.0 - SHARD_DELAY)).clamp(0.0, 1.0);
    let (ux, uy) = if dist > 0.5 { (ox / dist, oy / dist) } else { (0.0, -1.0) };
    let burst = SHARD_BURST_PX * (1.0 - (1.0 - u) * (1.0 - u)) * (0.6 + hash01(seed, i + 977) * 0.8);
    (ux * burst, uy * burst + u * u * vh * SHARD_FALL, 1.0 - u)
}

/// The elements of transition `kind` at eased progress `t` (0..1): the old
/// image (when there is one) and the new one, in viewport `vp`. `origin`
/// is where a Ripple or Shatter starts, `seed` varies the seams per change.
#[allow(clippy::too_many_arguments)]
pub fn compose(old: Option<&dyn Fn() -> AnyElement>, new: &dyn Fn() -> AnyElement, kind: WallpaperTransition, t: f32, vp: (f32, f32), rtl: bool, origin: (f32, f32), seed: u32) -> Vec<AnyElement> {
    let (vw, vh) = vp;
    let t = t.clamp(0.0, 1.0);
    let mut out: Vec<AnyElement> = Vec::new();
    let Some(old) = old.filter(|_| t < 1.0) else {
        out.push(new());
        return out;
    };
    match kind {
        // Sketch is drawn by `wallpaper_sketch`; here (no outline) it fades.
        WallpaperTransition::Fade | WallpaperTransition::Wipe | WallpaperTransition::Grow | WallpaperTransition::Sketch | WallpaperTransition::None => {
            out.push(old());
            out.push(crate::wallpaper_fill::reveal(new(), kind, t, vp, rtl));
        }
        WallpaperTransition::Ripple => {
            out.push(old());
            let reach = [(0.0, 0.0), (vw, 0.0), (0.0, vh), (vw, vh)].iter().map(|(x, y)| ((x - origin.0).powi(2) + (y - origin.1).powi(2)).sqrt()).fold(0.0_f32, f32::max);
            let r = reach * t;
            let (x, y) = (origin.0 - r, origin.1 - r);
            out.push(
                div()
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .size(px(2.0 * r))
                    .rounded_full()
                    .clip_rounded()
                    .border(px(RIPPLE_RIM_PX))
                    .border_color(gpui::white().opacity(RIPPLE_RIM_ALPHA * (1.0 - t)))
                    .child(div().absolute().left(px(-x)).top(px(-y)).w(px(vw)).h(px(vh)).child(new()))
                    .into_any_element(),
            );
        }
        WallpaperTransition::Tide => {
            out.push(old());
            let col = vw / TIDE_COLUMNS as f32;
            let wave = TIDE_WAVE * vh * (std::f32::consts::PI * t).sin();
            for i in 0..TIDE_COLUMNS {
                let phase = i as f32 * 0.55 + t * 9.0;
                let edge = (vh * (1.0 - t) + wave * phase.sin() - wave).clamp(0.0, vh);
                out.push(piece(new(), (i as f32 * col, edge, col + 1.0, vh - edge), (0.0, 0.0), vp).into_any_element());
            }
        }
        WallpaperTransition::Tear => {
            out.push(new());
            let row = vh / TEAR_ROWS as f32;
            for i in 0..TEAR_ROWS {
                let seam = tear_seam(seed, i, vw);
                let p = tear_peel(t, i);
                let y = i as f32 * row;
                // The halves part, and droop a little as they peel.
                let droop = p * p * row * 3.0 * hash01(seed, i + 311);
                let fade = 1.0 - p * 0.25;
                out.push(piece(old(), (0.0, y, seam, row + 1.0), (-p * (seam + 24.0), droop), vp).opacity(fade).into_any_element());
                out.push(piece(old(), (seam, y, vw - seam, row + 1.0), (p * (vw - seam + 24.0), droop), vp).opacity(fade).into_any_element());
            }
        }
        WallpaperTransition::Glitch => {
            let slice = vh / GLITCH_SLICES as f32;
            // The old image's slices give way to the new one's around the middle.
            let swap = smooth((t - 0.42) / 0.16);
            for i in 0..GLITCH_SLICES {
                let (shift_old, shift_new) = glitch_shift(seed, i, t, vw);
                let y = i as f32 * slice;
                let rect = (0.0, y, vw, slice + 1.0);
                if swap < 1.0 {
                    out.push(piece(old(), rect, (shift_old, 0.0), vp).opacity(1.0 - swap).into_any_element());
                }
                if swap > 0.0 {
                    out.push(piece(new(), rect, (shift_new, 0.0), vp).opacity(swap).into_any_element());
                }
                // The colour split: faint red and cyan edges where a slice shears.
                let shear = if swap < 0.5 { shift_old } else { shift_new };
                if shear.abs() > GLITCH_SPLIT_PX {
                    let a = (GLITCH_SPLIT_ALPHA * 255.0) as u32;
                    out.push(div().absolute().left(px(shear - GLITCH_SPLIT_PX)).top(px(y)).w(px(vw)).h(px(slice)).bg(rgba(0xff30_3000 | a)).into_any_element());
                    out.push(div().absolute().left(px(shear + GLITCH_SPLIT_PX)).top(px(y)).w(px(vw)).h(px(slice)).bg(rgba(0x30e0_ff00 | a)).into_any_element());
                }
            }
        }
        WallpaperTransition::Shatter => {
            out.push(new());
            let (cols, rows) = SHARDS;
            let (sw, sh) = (vw / cols as f32, vh / rows as f32);
            for r in 0..rows {
                for c in 0..cols {
                    let i = r * cols + c;
                    // Jittered shard edges, shared by neighbours so no gaps open at rest.
                    let (x, y) = (c as f32 * sw, r as f32 * sh);
                    let (dx, dy, alpha) = shard_motion(seed, i, (x + sw / 2.0, y + sh / 2.0), origin, vp, t);
                    if alpha > 0.0 {
                        out.push(piece(old(), (x, y, sw + 1.0, sh + 1.0), (dx, dy), vp).opacity(alpha).into_any_element());
                    }
                }
            }
        }
        WallpaperTransition::Dissolve => {
            out.push(old());
            let (cols, rows) = TILES;
            let (tw, th) = (vw / cols as f32, vh / rows as f32);
            for r in 0..rows {
                for c in 0..cols {
                    let d = hash01(seed, r * cols + c) * TILE_DELAY;
                    let a = smooth((t - d) / (1.0 - TILE_DELAY));
                    if a > 0.0 {
                        out.push(piece(new(), (c as f32 * tw, r as f32 * th, tw + 1.0, th + 1.0), (0.0, 0.0), vp).opacity(a).into_any_element());
                    }
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pieces_are_deterministic_and_bounded() {
        for i in 0..200 {
            let h = hash01(7, i);
            assert!((0.0..1.0).contains(&h));
            assert_eq!(h, hash01(7, i));
        }
        assert_ne!(hash01(1, 3), hash01(2, 3), "the seed varies the pieces");
        for i in 0..TEAR_ROWS {
            let s = tear_seam(42, i, 1000.0);
            assert!((1000.0 * (0.5 - TEAR_JAG * 1.35)..=1000.0 * (0.5 + TEAR_JAG * 1.35)).contains(&s));
        }
    }

    #[test]
    fn every_transition_ends_on_the_new_image_in_place() {
        for i in 0..TEAR_ROWS {
            assert_eq!(tear_peel(0.0, i), 0.0);
            assert_eq!(tear_peel(1.0, i), 1.0);
            let mut last = 0.0;
            for k in 0..=50 {
                let p = tear_peel(k as f32 / 50.0, i);
                assert!(p >= last - 1e-6, "a rip never closes again");
                last = p;
            }
        }
        for i in 0..GLITCH_SLICES {
            let (_, settle) = glitch_shift(9, i, 1.0, 2560.0);
            assert_eq!(settle, 0.0);
            let (old, _) = glitch_shift(9, i, 0.0, 2560.0);
            assert_eq!(old, 0.0);
            for k in 0..=20 {
                let (a, b) = glitch_shift(9, i, k as f32 / 20.0, 2560.0);
                assert!(a.abs() <= GLITCH_SHEAR * 2560.0 + 0.01 && b.abs() <= GLITCH_SHEAR * 2560.0 + 0.01);
            }
        }
        for i in 0..SHARDS.0 * SHARDS.1 {
            let (dx, dy, a) = shard_motion(3, i, (500.0, 300.0), (1280.0, 720.0), (2560.0, 1440.0), 1.0);
            assert_eq!(a, 0.0, "every shard is gone at the end");
            assert!(dx.is_finite() && dy.is_finite());
            assert_eq!(shard_motion(3, i, (500.0, 300.0), (1280.0, 720.0), (2560.0, 1440.0), 0.0).2, 1.0);
        }
        // A shard on the origin still leaves (upward first).
        assert!(shard_motion(3, 0, (10.0, 10.0), (10.0, 10.0), (100.0, 100.0), 0.5).2 < 1.0);
    }
}
