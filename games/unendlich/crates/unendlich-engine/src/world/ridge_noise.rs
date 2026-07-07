//! `ridge_noise` — ridged multifractal for mountain ridgelines.
//!
//! Worker-03 / WORLD pure core, round 16. Sums octaves of *ridged* value noise
//! (`1 − |2n − 1|`, which creases into a sharp crest where the underlying noise
//! crosses ½) into a value in [0,1]. Higher octaves add finer ridges; the sum is
//! amplitude-weighted and normalized so the range stays bounded regardless of
//! octave count. Sample warped coords (`terrain_warp`) for natural ridgelines.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const MAX_OCTAVES: u32 = 12;

/// Ridged multifractal at `(x, z)` over `octaves` octaves, in [0,1].
pub fn ridge(x: f32, z: f32, octaves: u32, seed: u64) -> f32 {
    let o = octaves.clamp(1, MAX_OCTAVES);
    let mut freq = 1.0f32;
    let mut amp = 0.5f32;
    let mut sum = 0.0f32;
    let mut norm = 0.0f32;
    for i in 0..o {
        let n = value_noise(x * freq, z * freq, seed ^ (i as u64).wrapping_mul(0x9E37_79B9));
        let r = 1.0 - (2.0 * n - 1.0).abs(); // [0,1], crest where n≈0.5
        sum += r * amp;
        norm += amp;
        freq *= 2.0;
        amp *= 0.5;
    }
    if norm > 0.0 {
        clamp01(sum / norm)
    } else {
        0.0
    }
}

fn value_noise(x: f32, z: f32, seed: u64) -> f32 {
    let x0 = x.floor();
    let z0 = z.floor();
    let xf = x - x0;
    let zf = z - z0;
    let (ix, iz) = (x0 as i32, z0 as i32);
    let v00 = corner(ix, iz, seed);
    let v10 = corner(ix + 1, iz, seed);
    let v01 = corner(ix, iz + 1, seed);
    let v11 = corner(ix + 1, iz + 1, seed);
    let sx = smoothstep(xf);
    let sz = smoothstep(zf);
    lerp(lerp(v00, v10, sx), lerp(v01, v11, sx), sz)
}

fn corner(ix: i32, iz: i32, seed: u64) -> f32 {
    let mut h = seed ^ 0x9E37_79B9_7F4A_7C15;
    h = mix(h ^ (ix as u32 as u64).wrapping_mul(0x85EB_CA6B));
    h = mix(h ^ (iz as u32 as u64).wrapping_mul(0xC2B2_AE35));
    ((h >> 40) as f32) / ((1u64 << 24) as f32)
}
fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
fn smoothstep(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    crate::util::mathx::lerp(a, b, t) // r342 dedup
}
fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn in_unit_range() {
        for i in 0..40 {
            for j in 0..40 {
                let v = ridge(i as f32 * 0.3, j as f32 * 0.3, 5, 7);
                assert!(v >= 0.0 && v <= 1.0);
            }
        }
    }

    #[test]
    fn octave_count_bounded() {
        // octaves 0 → clamped to 1; huge → clamped to MAX; both valid.
        assert!((0.0..=1.0).contains(&ridge(1.0, 1.0, 0, 3)));
        assert!((0.0..=1.0).contains(&ridge(1.0, 1.0, 1000, 3)));
    }

    #[test]
    fn ridges_present() {
        // Over a scan, crests reach high and valleys go low → real ridges.
        let mut hi = 0.0f32;
        let mut lo = 1.0f32;
        for i in 0..400 {
            let v = ridge(i as f32 * 0.05, 12.3, 4, 5);
            hi = hi.max(v);
            lo = lo.min(v);
        }
        assert!(hi > 0.7, "no ridge crest found (max {hi})");
        assert!(lo < 0.5, "no valleys (min {lo})");
    }

    #[test]
    fn deterministic() {
        assert_eq!(ridge(3.3, 4.4, 6, 9), ridge(3.3, 4.4, 6, 9));
    }

    #[test]
    fn seed_sensitive() {
        assert_ne!(ridge(3.3, 4.4, 6, 1), ridge(3.3, 4.4, 6, 2));
    }

    #[test]
    fn continuity() {
        let a = ridge(50.0, 50.0, 4, 1);
        let b = ridge(50.01, 50.0, 4, 1);
        assert!((a - b).abs() < 0.2, "ridge value jumped");
    }
}
