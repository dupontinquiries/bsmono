//! cloud_noise.rs — pure FBM cloud-density field.
//!
//! Fractal Brownian motion over a deterministic 3D value-noise basis, normalized
//! to `[0,1]`, plus a coverage remap that turns a density into a cloud fraction
//! above a threshold. C1-continuous (smoothstep interpolation), seedable, and
//! bounded regardless of octave count. Pure, deterministic, panic-free
//! (R1/R11/R21).

use crate::util::mathx::lerp; // r330-refactor-math: shared exact lerp

/// Integer hash of a lattice point -> `[0,1)`. Deterministic; no clock.
fn hash3(ix: i32, iy: i32, iz: i32, seed: u64) -> f32 {
    let mut h = seed;
    h ^= (ix as i64 as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h = (h ^ (h >> 29)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= (iy as i64 as u64).wrapping_mul(0x94D0_49BB_1331_11EB);
    h = (h ^ (h >> 32)).wrapping_mul(0xD6E8_FEB8_6659_FD93);
    h ^= (iz as i64 as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F);
    h ^= h >> 31;
    (h >> 40) as f32 / ((1u64 << 24) as f32)
}

fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// Trilinearly-interpolated value noise in `[0,1]`, C1-continuous.
fn value_noise(x: f32, y: f32, z: f32, seed: u64) -> f32 {
    let xf = x.floor();
    let yf = y.floor();
    let zf = z.floor();
    let (ix, iy, iz) = (xf as i32, yf as i32, zf as i32);
    let (fx, fy, fz) = (smoothstep(x - xf), smoothstep(y - yf), smoothstep(z - zf));

    let c = |dx: i32, dy: i32, dz: i32| hash3(ix + dx, iy + dy, iz + dz, seed);

    let x00 = lerp(c(0, 0, 0), c(1, 0, 0), fx);
    let x10 = lerp(c(0, 1, 0), c(1, 1, 0), fx);
    let x01 = lerp(c(0, 0, 1), c(1, 0, 1), fx);
    let x11 = lerp(c(0, 1, 1), c(1, 1, 1), fx);
    let y0 = lerp(x00, x10, fy);
    let y1 = lerp(x01, x11, fy);
    lerp(y0, y1, fz)
}

/// FBM cloud density at `(x,y,z)` in `[0,1]`. Sums `octaves` value-noise layers
/// with halving amplitude / doubling frequency, normalized so the result stays
/// in `[0,1]` for any octave count. `octaves` is treated as at least 1 and
/// capped at 16.
pub fn fbm(x: f32, y: f32, z: f32, octaves: u32, seed: u64) -> f32 {
    let oct = octaves.max(1).min(16);
    let mut amp = 0.5f32;
    let mut freq = 1.0f32;
    let mut sum = 0.0f32;
    let mut norm = 0.0f32;
    for o in 0..oct {
        let s = seed ^ (o as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        sum += amp * value_noise(x * freq, y * freq, z * freq, s);
        norm += amp;
        amp *= 0.5;
        freq *= 2.0;
    }
    if norm > 0.0 {
        crate::util::mathx::saturate(sum / norm)
    } else {
        0.0
    }
}

/// Coverage fraction for a density `d` above `thresh`: `0` at/below the
/// threshold, ramping linearly to `1` at `d == 1`. Monotone in `d`.
pub fn coverage(d: f32, thresh: f32) -> f32 {
    let t = crate::util::mathx::saturate(thresh);
    let denom = (1.0 - t).max(1e-4);
    crate::util::mathx::saturate((d - t) / denom)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*seed >> 8) as f32 / ((1u32 << 24) as f32)
    }

    #[test]
    fn output_in_unit_range() {
        let mut seed = 0xC10D_0001u32;
        for _ in 0..2000 {
            let x = lcg(&mut seed) * 200.0 - 100.0;
            let y = lcg(&mut seed) * 200.0 - 100.0;
            let z = lcg(&mut seed) * 200.0 - 100.0;
            let oct = (lcg(&mut seed) * 8.0) as u32;
            let v = fbm(x, y, z, oct, 1234);
            assert!((0.0..=1.0).contains(&v), "fbm oob {}", v);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(fbm(1.5, 2.5, 3.5, 5, 42), fbm(1.5, 2.5, 3.5, 5, 42));
    }

    #[test]
    fn more_octaves_stays_bounded() {
        for oct in 1..=16u32 {
            let v = fbm(12.3, 4.5, 6.7, oct, 99);
            assert!((0.0..=1.0).contains(&v), "octaves {} -> oob {}", oct, v);
        }
    }

    #[test]
    fn coverage_thresholds() {
        assert_eq!(coverage(0.2, 0.5), 0.0, "below threshold -> 0");
        assert_eq!(coverage(0.5, 0.5), 0.0, "at threshold -> 0");
        assert!(coverage(0.75, 0.5) > 0.0 && coverage(0.75, 0.5) < 1.0);
        assert!((coverage(1.0, 0.5) - 1.0).abs() < 1e-6, "d=1 -> full coverage");
        // monotone in d
        let mut prev = -1.0;
        let mut d = 0.0;
        while d <= 1.0 {
            let c = coverage(d, 0.3);
            assert!(c >= prev - 1e-6);
            prev = c;
            d += 0.01;
        }
    }

    #[test]
    fn continuity_small_step_small_change() {
        let seed = 7;
        let eps = 1e-4f32;
        let mut s = 0xAB_u32;
        for _ in 0..200 {
            let x = lcg(&mut s) * 50.0;
            let y = lcg(&mut s) * 50.0;
            let z = lcg(&mut s) * 50.0;
            let a = fbm(x, y, z, 4, seed);
            let b = fbm(x + eps, y, z, 4, seed);
            assert!((a - b).abs() < 1e-2, "discontinuity: {} vs {}", a, b);
        }
    }

    #[test]
    fn seed_sensitive() {
        // at least one point differs between two seeds
        let mut differ = false;
        for i in 0..16 {
            let p = i as f32 * 1.37;
            if (fbm(p, p * 0.5, p * 0.25, 4, 1) - fbm(p, p * 0.5, p * 0.25, 4, 2)).abs() > 1e-4 {
                differ = true;
                break;
            }
        }
        assert!(differ, "fbm not sensitive to seed");
    }
}
