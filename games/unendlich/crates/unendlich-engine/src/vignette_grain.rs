//! vignette_grain.rs — pure postfx vignette + film grain.
//!
//! A radial vignette multiplier (1 at the centre, darkening toward the corners)
//! and a deterministic film-grain offset in `[-amount, amount]`. Pure,
//! deterministic, panic-free (R1/R11/R21).

/// Deterministic hash of three floats -> `[0,1)`. Uses raw bit patterns so the
/// same inputs always give the same value (no clock).
fn hash_f(a: f32, b: f32, c: f32) -> f32 {
    let mut h = 0x9E37_79B9_7F4A_7C15u64;
    for bits in [a.to_bits(), b.to_bits(), c.to_bits()] {
        h ^= bits as u64;
        h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        h ^= h >> 31;
    }
    (h >> 40) as f32 / ((1u64 << 24) as f32)
}

/// Vignette multiplier for screen `uv` (each in `[0,1]`). `1.0` at the centre,
/// falling toward the corners as `strength` (clamped `>= 0`) increases. Result
/// is in `[0,1]`.
pub fn vignette(uv: [f32; 2], strength: f32) -> f32 {
    let dx = uv[0] - 0.5;
    let dy = uv[1] - 0.5;
    // normalized squared radius: 0 at centre, 1 at a corner
    let r2 = (dx * dx + dy * dy) / 0.5;
    let s = strength.max(0.0);
    (1.0 - s * r2).max(0.0).min(1.0)
}

/// Film-grain offset for screen `uv` at time `t`, in `[-amount, amount]`.
/// Deterministic for a given `(uv, t)`.
pub fn grain(uv: [f32; 2], t: f32, amount: f32) -> f32 {
    let h = hash_f(uv[0], uv[1], t); // [0,1)
    (h * 2.0 - 1.0) * amount.abs() // [-amount, amount)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn center_is_one_edges_darker() {
        let center = vignette([0.5, 0.5], 0.8);
        assert!((center - 1.0).abs() < 1e-6, "centre not 1: {}", center);
        assert!(vignette([0.0, 0.0], 0.8) < center, "corner not darker");
        assert!(vignette([0.5, 0.0], 0.8) < center, "edge not darker");
    }

    #[test]
    fn strength_scales_darkening() {
        let corner_weak = vignette([0.0, 0.0], 0.3);
        let corner_strong = vignette([0.0, 0.0], 0.9);
        assert!(corner_strong < corner_weak, "more strength should darken more");
    }

    #[test]
    fn center_is_brightest() {
        let center = vignette([0.5, 0.5], 0.6);
        let mut seed = 0x5160_0001u32;
        let mut lcg = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / ((1u32 << 24) as f32)
        };
        for _ in 0..1000 {
            let uv = [lcg(), lcg()];
            assert!(vignette(uv, 0.6) <= center + 1e-6, "something brighter than centre");
        }
    }

    #[test]
    fn vignette_bounded() {
        let mut seed = 0x5160_0002u32;
        let mut lcg = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / ((1u32 << 24) as f32)
        };
        for _ in 0..2000 {
            let v = vignette([lcg() * 1.5 - 0.25, lcg() * 1.5 - 0.25], lcg() * 5.0);
            assert!((0.0..=1.0).contains(&v), "vignette oob {}", v);
        }
    }

    #[test]
    fn grain_within_amount_band() {
        let mut seed = 0x6711_0001u32;
        let mut lcg = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / ((1u32 << 24) as f32)
        };
        for _ in 0..5000 {
            let uv = [lcg(), lcg()];
            let t = lcg() * 100.0;
            let amount = lcg() * 0.5;
            let g = grain(uv, t, amount);
            assert!(g >= -amount - 1e-6 && g <= amount + 1e-6, "grain {} outside ±{}", g, amount);
        }
    }

    #[test]
    fn grain_deterministic_per_uv_t() {
        assert_eq!(grain([0.3, 0.7], 1.5, 0.2), grain([0.3, 0.7], 1.5, 0.2));
        // different t generally differs
        assert!(grain([0.3, 0.7], 1.5, 0.2) != grain([0.3, 0.7], 2.5, 0.2));
    }
}
