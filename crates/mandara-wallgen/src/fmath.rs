//! Deterministic `f32` math. The standard library's `sin`, `exp`, `powf`
//! and friends call the platform's libm, whose last bits differ between
//! glibc, musl and macOS; every transcendental on the per-pixel path goes
//! through these instead, built from IEEE-exact operations only (add, mul,
//! `sqrt`, `floor`, `round` and bit casts; never `mul_add`, which is a
//! libm call on baseline x86-64), so a recipe renders
//! the same pixels on every machine.

use std::f32::consts::{FRAC_PI_2, PI, TAU};

/// `log2(e)`, for `exp` through `exp2`.
const LOG2_E: f32 = std::f32::consts::LOG2_E;
/// `ln(2)`, to turn the natural-log series of `log2` into base 2.
const LN_2: f32 = std::f32::consts::LN_2;
/// Arguments of `exp2` are clamped to this range (the `f32` exponent span).
const EXP2_RANGE: f32 = 126.0;

/// Sine of `x` (radians), accurate to ~1e-7 over any range a renderer uses.
#[inline]
pub fn sin(x: f32) -> f32 {
    // Turns, folded into [-0.25, 0.25] where the series converges fast.
    let mut t = x * (1.0 / TAU);
    t -= t.round();
    if t > 0.25 {
        t = 0.5 - t;
    } else if t < -0.25 {
        t = -0.5 - t;
    }
    let y = t * TAU;
    let y2 = y * y;
    // Taylor series to y^11: error below 4e-8 on [-π/2, π/2].
    let p = (-1.0 / 39_916_800.0f32) * y2 + 1.0 / 362_880.0;
    let p = p * y2 - 1.0 / 5040.0;
    let p = p * y2 + 1.0 / 120.0;
    let p = p * y2 - 1.0 / 6.0;
    let p = p * y2 + 1.0;
    y * p
}

/// Cosine of `x` (radians).
#[inline]
pub fn cos(x: f32) -> f32 {
    sin(x + FRAC_PI_2)
}

/// `2^x`.
#[inline]
pub fn exp2(x: f32) -> f32 {
    let x = x.clamp(-EXP2_RANGE, EXP2_RANGE);
    let k = x.round();
    let f = x - k;
    // Taylor series of 2^f = e^(f·ln 2) to f^7 on [-0.5, 0.5] (error < 1e-8).
    let p = 1.525_273e-5f32 * f + 1.540_353e-4;
    let p = p * f + 1.333_356e-3;
    let p = p * f + 9.618_129e-3;
    let p = p * f + 5.550_411e-2;
    let p = p * f + 0.240_226_5;
    let p = p * f + 0.693_147_2;
    let p = p * f + 1.0;
    p * f32::from_bits(((k as i32 + 127) as u32) << 23)
}

/// `e^x`.
#[inline]
pub fn exp(x: f32) -> f32 {
    exp2(x * LOG2_E)
}

/// `log2(x)` for `x > 0` (non-positive input gives a large negative value).
#[inline]
pub fn log2(x: f32) -> f32 {
    if x <= 0.0 {
        return -EXP2_RANGE;
    }
    let bits = x.to_bits();
    let e = ((bits >> 23) & 0xFF) as i32 - 127;
    // Mantissa in [1, 2), re-centred to [√½, √2) for a fast series.
    let mut m = f32::from_bits((bits & 0x007F_FFFF) | 0x3F80_0000);
    let mut e = e as f32;
    if m > std::f32::consts::SQRT_2 {
        m *= 0.5;
        e += 1.0;
    }
    // ln(m) = 2·atanh(s), s = (m − 1)/(m + 1), |s| < 0.172.
    let s = (m - 1.0) / (m + 1.0);
    let s2 = s * s;
    let p = (2.0 / 9.0f32) * s2 + 2.0 / 7.0;
    let p = p * s2 + 2.0 / 5.0;
    let p = p * s2 + 2.0 / 3.0;
    let p = p * s2 + 2.0;
    (s * p) * 1.0 / LN_2 + e
}

/// `x^y` for `x ≥ 0`.
#[inline]
pub fn pow(x: f32, y: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    exp2(y * log2(x))
}

/// Real cube root (sign-preserving): a bit-trick seed refined by Newton.
#[inline]
pub fn cbrt(x: f32) -> f32 {
    if x == 0.0 {
        return 0.0;
    }
    let a = x.abs();
    // Divide the exponent by three in the bit pattern (Kahan's trick).
    let mut y = f32::from_bits(a.to_bits() / 3 + 709_921_077);
    for _ in 0..3 {
        y = y - (y * y * y - a) / (3.0 * y * y);
    }
    y.copysign(x)
}

/// `atan2(y, x)` in radians, (−π, π], error ~1e-5.
#[inline]
pub fn atan2(y: f32, x: f32) -> f32 {
    if x == 0.0 && y == 0.0 {
        return 0.0;
    }
    let (ax, ay) = (x.abs(), y.abs());
    let (num, den, swap) = if ay > ax { (ax, ay, true) } else { (ay, ax, false) };
    let z = num / den;
    let z2 = z * z;
    // Odd minimax polynomial for atan on [0, 1].
    let p = -0.011_721_2f32 * z2 + 0.052_653_32;
    let p = p * z2 - 0.116_432_87;
    let p = p * z2 + 0.193_543_46;
    let p = p * z2 - 0.332_623_47;
    let p = p * z2 + 0.999_977_26;
    let mut r = z * p;
    if swap {
        r = FRAC_PI_2 - r;
    }
    if x < 0.0 {
        r = PI - r;
    }
    if y < 0.0 { -r } else { r }
}

/// Hyperbolic tangent (soft clipping).
#[inline]
pub fn tanh(x: f32) -> f32 {
    let e = exp(-2.0 * x.abs());
    ((1.0 - e) / (1.0 + e)).copysign(x)
}

/// Hermite smoothstep of `x` between `a` and `b`.
#[inline]
pub fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Linear interpolation.
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    (b - a) * t + a
}

/// Fractional part in [0, 1).
#[inline]
pub fn fract(x: f32) -> f32 {
    x - x.floor()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_libm_closely() {
        for i in -2000..2000 {
            let x = i as f32 * 0.0173;
            // Range reduction costs an ulp of x/2π: tolerance grows with |x|.
            let tol = 2e-6 + 2e-7 * x.abs();
            assert!((sin(x) - x.sin()).abs() < tol, "sin {x}");
            assert!((cos(x) - x.cos()).abs() < tol, "cos {x}");
            let e = (i as f32) * 0.004;
            assert!((exp(e) / e.exp() - 1.0).abs() < 1e-5, "exp {e}");
            let p = (i + 2001) as f32 * 0.01;
            assert!((log2(p) - p.log2()).abs() < 1e-5, "log2 {p}");
            assert!((cbrt(x) - x.cbrt()).abs() < 1e-5 * x.abs().max(1.0), "cbrt {x}");
            let (yy, xx) = (sin(x * 3.0), cos(x * 1.7));
            assert!((atan2(yy, xx) - yy.atan2(xx)).abs() < 3e-5, "atan2 {yy} {xx}");
        }
        assert!((pow(0.5, 2.4) - 0.5f32.powf(2.4)).abs() < 1e-6);
        assert!((tanh(0.7) - 0.7f32.tanh()).abs() < 1e-6);
    }
}
