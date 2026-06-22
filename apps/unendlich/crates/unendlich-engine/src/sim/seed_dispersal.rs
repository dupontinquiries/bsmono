//! `sim::seed_dispersal` — where a plant's seeds land, given its dispersal genes
//! and the local wind.
//!
//! Pure and seeded-deterministic (R1/R20): [`disperse`] takes a parent's
//! dispersal genes (spread radius + seed count), a wind vector `(x, z)`, and a
//! `u64` seed, and returns a list of `(dx, dz)` landing offsets around the
//! parent. No global RNG — all randomness comes from the passed seed, so the
//! same inputs always produce the same scatter. This is **seeds**, distinct from
//! `pollination`/`pollination_route` (pollen). The flora-site layer consumes the
//! offsets; this module only produces them.
//!
//! The base scatter is a uniform-area disc of the spread radius (symmetric when
//! there's no wind). Wind biases each seed's reach by how aligned its direction
//! is with the wind — downwind seeds travel farther, upwind seeds shorter — so
//! the *mean* landing shifts downwind while every seed still stays within the
//! spread radius. Standalone `rustc --test`-able; no clock, no IO, no
//! `unwrap`/`expect` outside tests.

use std::f32::consts::PI;

/// A plant's heritable seed-dispersal genes.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DispersalGenes {
    /// Maximum spread radius in blocks (offsets never exceed this).
    pub spread_radius: f32,
    /// Number of seeds released.
    pub count: u32,
}

/// How strongly wind skews reach, per unit of (clamped) wind magnitude.
const WIND_BIAS: f32 = 0.6;

fn hash01(seed: u64, salt: u64) -> f32 {
    let mut z = seed
        .wrapping_add(salt.wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 40) as f32 / ((1u64 << 24) as f32)
}

/// Deterministic seed-landing offsets `(dx, dz)` around a parent. Count equals
/// `genes.count`; every offset is within `genes.spread_radius`; wind shifts the
/// mean landing downwind; zero wind yields a symmetric spread.
pub fn disperse(genes: DispersalGenes, wind: [f32; 2], seed: u64) -> Vec<[f32; 2]> {
    let radius = genes.spread_radius.max(0.0);
    let mut out = Vec::with_capacity(genes.count as usize);

    // wind unit direction + clamped strength (0 wind ⇒ no bias ⇒ symmetric)
    let wlen = (wind[0] * wind[0] + wind[1] * wind[1]).sqrt();
    let (wx, wz, strength) = if wlen > 1e-6 {
        (wind[0] / wlen, wind[1] / wlen, wlen.min(1.0) * WIND_BIAS)
    } else {
        (0.0, 0.0, 0.0)
    };

    for i in 0..genes.count as u64 {
        let u = hash01(seed, i * 2);
        let theta = hash01(seed, i * 2 + 1) * 2.0 * PI;
        let (dx, dz) = (theta.cos(), theta.sin()); // unit direction
        let base_r = radius * u.sqrt(); // uniform over the disc area
        let align = dx * wx + dz * wz; // -1 (upwind) .. 1 (downwind)
        let scale = (1.0 + strength * align).max(0.0);
        let r = (base_r * scale).max(0.0).min(radius); // never exceed the radius
        out.push([dx * r, dz * r]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mag(o: [f32; 2]) -> f32 {
        (o[0] * o[0] + o[1] * o[1]).sqrt()
    }
    fn genes(r: f32, c: u32) -> DispersalGenes {
        DispersalGenes { spread_radius: r, count: c }
    }

    #[test]
    fn deterministic_same_seed() {
        let g = genes(8.0, 64);
        assert_eq!(disperse(g, [1.0, 0.5], 12345), disperse(g, [1.0, 0.5], 12345));
        // different seed generally differs
        assert!(disperse(g, [1.0, 0.5], 1) != disperse(g, [1.0, 0.5], 2));
    }

    #[test]
    fn count_matches_gene() {
        for c in [0u32, 1, 5, 50, 500] {
            assert_eq!(disperse(genes(6.0, c), [0.3, -0.2], 7).len(), c as usize);
        }
    }

    #[test]
    fn all_within_max_radius() {
        let mut s = 0x30FA_u64; // documented seed (R20)
        for _ in 0..400 {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
            let r = (hash01(s, 1) * 20.0).max(0.01);
            let wind = [hash01(s, 2) * 4.0 - 2.0, hash01(s, 3) * 4.0 - 2.0];
            for o in disperse(genes(r, 32), wind, s) {
                assert!(o[0].is_finite() && o[1].is_finite(), "non-finite offset");
                assert!(mag(o) <= r + 1e-4, "offset {} exceeds radius {}", mag(o), r);
            }
        }
    }

    #[test]
    fn wind_biases_mean_downwind() {
        // strong wind along +x ⇒ mean dx > 0 and dominant along the wind axis
        let g = genes(10.0, 4000);
        let offs = disperse(g, [1.0, 0.0], 99);
        let n = offs.len() as f32;
        let mean_x: f32 = offs.iter().map(|o| o[0]).sum::<f32>() / n;
        let mean_z: f32 = offs.iter().map(|o| o[1]).sum::<f32>() / n;
        assert!(mean_x > 0.1, "mean did not shift downwind: mean_x={}", mean_x);
        assert!(mean_x > mean_z.abs(), "bias not along the wind axis");
    }

    #[test]
    fn zero_wind_is_symmetric() {
        let g = genes(10.0, 4000);
        let offs = disperse(g, [0.0, 0.0], 99);
        let n = offs.len() as f32;
        let mean_x: f32 = offs.iter().map(|o| o[0]).sum::<f32>() / n;
        let mean_z: f32 = offs.iter().map(|o| o[1]).sum::<f32>() / n;
        // mean should sit near the parent (both axes) — symmetric scatter
        assert!(mean_x.abs() < 0.4 && mean_z.abs() < 0.4, "not symmetric: ({}, {})", mean_x, mean_z);
        // and the spread reaches both signs on each axis
        assert!(offs.iter().any(|o| o[0] > 1.0) && offs.iter().any(|o| o[0] < -1.0));
        assert!(offs.iter().any(|o| o[1] > 1.0) && offs.iter().any(|o| o[1] < -1.0));
    }

    #[test]
    fn radius_zero_and_empty_guards() {
        // zero radius ⇒ all seeds land exactly on the parent
        for o in disperse(genes(0.0, 16), [1.0, 1.0], 3) {
            assert_eq!(o, [0.0, 0.0]);
        }
        // zero count ⇒ empty, and negative radius is clamped (no panic)
        assert!(disperse(genes(5.0, 0), [1.0, 0.0], 3).is_empty());
        for o in disperse(genes(-5.0, 8), [1.0, 0.0], 3) {
            assert_eq!(o, [0.0, 0.0]);
        }
    }
}
