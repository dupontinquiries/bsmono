//! `vegetation_scatter` — deterministic plant-placement density.
//!
//! Worker-03 / WORLD-PERF pure core, round 12. Decides how thickly to scatter
//! flora: a biome base density modulated up by moisture and down by slope, then
//! a per-cell hash decides whether a given cell actually gets a plant. Stateless
//! and seed-deterministic, so the same world always scatters identically.
//! Pairs with `plant_genetics` (this picks the *where*; genetics picks *what*).
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Base scatter density per biome (unknown ids get a mid value).
pub fn biome_base(biome: u8) -> f32 {
    // r336-fix: the old ad-hoc 0..4 id scheme didn't match the real `Biome` enum, so
    // Ocean (id 11) and everything >=5 fell to a mid default and scattered grass into the
    // sea. Use the canonical per-`Biome` density (oceans + caves = 0). Invalid id → 0.
    use crate::world::biomes::Biome;
    Biome::try_from(biome as u32).map(|b| b.flora_density()).unwrap_or(0.0)
}

/// Scatter density in [0,1]: rises with moisture, falls with slope.
pub fn density(biome: u8, moisture: f32, slope: f32) -> f32 {
    let base = clamp01(biome_base(biome));
    let m = clamp01(moisture);
    let s = clamp01(slope);
    clamp01(base * m * (1.0 - s))
}

/// Whether a plant is placed at cell `(cellx, cellz)` given a `density` and
/// seed. Deterministic per cell; `density ≤ 0` never places, `density ≥ 1`
/// always places.
pub fn place(cellx: i32, cellz: i32, density: f32, seed: u64) -> bool {
    let r = unit(hash2(cellx, cellz, seed));
    r < clamp01(density)
}

fn hash2(x: i32, z: i32, seed: u64) -> u64 {
    let mut h = seed ^ 0x9E37_79B9_7F4A_7C15;
    h = mix(h ^ (x as u32 as u64).wrapping_mul(0x85EB_CA6B));
    h = mix(h ^ (z as u32 as u64).wrapping_mul(0xC2B2_AE35));
    h
}

fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

fn unit(h: u64) -> f32 {
    ((h >> 40) as f32) / ((1u64 << 24) as f32)
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wetter_is_denser() {
        assert!(density(1, 0.8, 0.0) > density(1, 0.2, 0.0));
    }

    #[test]
    fn steeper_is_sparser() {
        assert!(density(1, 0.5, 0.1) > density(1, 0.5, 0.9));
    }

    #[test]
    fn density_in_unit_range() {
        for &b in &[0u8, 1, 2, 3, 4, 200] {
            for mi in 0..=10 {
                for si in 0..=10 {
                    let d = density(b, mi as f32 / 10.0, si as f32 / 10.0);
                    assert!(d >= 0.0 && d <= 1.0);
                }
            }
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(density(1, 0.6, 0.3), density(1, 0.6, 0.3));
        assert_eq!(place(12, -7, 0.5, 99), place(12, -7, 0.5, 99));
    }

    #[test]
    fn place_stable_per_cell() {
        for _ in 0..3 {
            assert_eq!(place(3, 4, 0.5, 1), place(3, 4, 0.5, 1));
        }
        assert!(!place(0, 0, 0.0, 7)); // never at density 0
        assert!(place(0, 0, 1.0, 7)); // always at density 1
    }

    #[test]
    fn seed_sensitive() {
        let mut differ = false;
        for x in 0..500 {
            if place(x, 0, 0.5, 1) != place(x, 0, 0.5, 2) {
                differ = true;
                break;
            }
        }
        assert!(differ, "two seeds scattered identically");
    }

    #[test]
    fn density_roughly_controls_fraction() {
        // Over many cells, ~density fraction get a plant.
        let span = 200;
        let mut hits = 0;
        for x in 0..span {
            for z in 0..span {
                if place(x, z, 0.3, 5) {
                    hits += 1;
                }
            }
        }
        let frac = hits as f32 / (span * span) as f32;
        assert!(frac > 0.2 && frac < 0.4, "fraction {frac} far from 0.3");
    }
}
