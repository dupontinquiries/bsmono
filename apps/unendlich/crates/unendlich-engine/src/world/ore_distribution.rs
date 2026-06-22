//! `ore_distribution` — deterministic depth-banded ore veins.
//!
//! Worker-03 / WORLD-PERF pure core, round 9. A stateless hash oracle: given a
//! world cell and seed, decide whether ore is present and, if so, its kind and
//! richness. Shallow bands are common but poor; deep bands are rare but rich.
//! No RNG state — purely a hash of `(x, y, z, seed)`, so it's reproducible and
//! samplable in any order (the generator gates it to stone cells).
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Approximate surface height; depth is measured downward from here.
pub const SURFACE_Y: i32 = 64;
/// Vertical size of a depth band.
pub const BAND_HEIGHT: i32 = 32;
/// Spawn chance (permille) at the surface band; deeper bands are rarer.
pub const SURFACE_SPAWN_PERMILLE: u32 = 120;
/// Floor on spawn chance so the deepest bands still produce some ore.
pub const MIN_SPAWN_PERMILLE: u32 = 4;

/// A placed ore.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ore {
    pub kind: u8,
    pub richness: f32,
}

/// Ore (if any) at a world cell. Deeper → rarer but richer.
pub fn ore_at(x: i32, y: i32, z: i32, seed: u64) -> Option<Ore> {
    let band = band_of(y);
    let spawn = spawn_permille(band);
    let roll = (hash3(x, y, z, seed) % 1000) as u32;
    if roll >= spawn {
        return None;
    }
    Some(Ore {
        kind: band.min(7) as u8,
        richness: richness_for(band),
    })
}

/// Depth band of a y-level (0 at/above the surface, increasing downward).
pub fn band_of(y: i32) -> u32 {
    let depth = (SURFACE_Y - y).max(0);
    (depth / BAND_HEIGHT) as u32
}

/// Spawn chance (permille) for a band: halves roughly each band, floored.
pub fn spawn_permille(band: u32) -> u32 {
    (SURFACE_SPAWN_PERMILLE / (band + 1)).max(MIN_SPAWN_PERMILLE)
}

/// Richness for a band, in [0,1]; grows with depth.
pub fn richness_for(band: u32) -> f32 {
    (0.1 + 0.15 * band as f32).min(1.0)
}

fn hash3(x: i32, y: i32, z: i32, seed: u64) -> u64 {
    let mut h = seed ^ 0x9E37_79B9_7F4A_7C15;
    h = mix(h ^ (x as u32 as u64).wrapping_mul(0x85EB_CA6B));
    h = mix(h ^ (y as u32 as u64).wrapping_mul(0xC2B2_AE35));
    h = mix(h ^ (z as u32 as u64).wrapping_mul(0x27D4_EB2F));
    h
}

fn mix(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Count ores + collect richness over a deterministic slab at depth y.
    fn slab(y: i32, seed: u64, span: i32) -> (u32, Vec<f32>) {
        let mut count = 0;
        let mut rich = Vec::new();
        for x in 0..span {
            for z in 0..span {
                if let Some(o) = ore_at(x, y, z, seed) {
                    count += 1;
                    rich.push(o.richness);
                    assert!(o.richness >= 0.0 && o.richness <= 1.0);
                }
            }
        }
        (count, rich)
    }

    #[test]
    fn deterministic_same_coord_seed() {
        for (x, y, z) in [(0, 0, 0), (12, -40, 5), (1000, 60, -7)] {
            assert_eq!(ore_at(x, y, z, 42), ore_at(x, y, z, 42));
        }
    }

    #[test]
    fn deep_favors_richer_than_shallow() {
        // Shallow band 0 (y=SURFACE) vs deep band ~5 (y well below).
        let (_, shallow) = slab(SURFACE_Y, 7, 80);
        let deep_y = SURFACE_Y - BAND_HEIGHT * 5;
        let (_, deep) = slab(deep_y, 7, 200);
        let max_shallow = shallow.iter().cloned().fold(0.0f32, f32::max);
        let min_deep = deep.iter().cloned().fold(1.0f32, f32::min);
        assert!(!deep.is_empty(), "deep slab produced no ore to compare");
        assert!(min_deep > max_shallow, "deep ore not richer than shallow");
    }

    #[test]
    fn surface_is_common_some_and_none() {
        let span = 60;
        let (count, _) = slab(SURFACE_Y, 3, span);
        let total = (span * span) as u32;
        assert!(count > 0, "surface should have ore");
        assert!(count < total, "surface should not be all ore");
        // Surface band richness is the low base value.
        assert!((richness_for(0) - 0.1).abs() < 1e-6);
    }

    #[test]
    fn richness_bounded_and_growing() {
        for band in 0..20u32 {
            let r = richness_for(band);
            assert!(r >= 0.0 && r <= 1.0);
        }
        assert!(richness_for(3) > richness_for(0));
        assert_eq!(richness_for(100), 1.0); // capped
    }

    #[test]
    fn deeper_is_rarer() {
        // Spawn frequency decreases with depth.
        assert!(spawn_permille(0) > spawn_permille(1));
        assert!(spawn_permille(1) >= spawn_permille(2));
        let (surface_count, _) = slab(SURFACE_Y, 9, 100);
        let (deep_count, _) = slab(SURFACE_Y - BAND_HEIGHT * 4, 9, 100);
        assert!(deep_count < surface_count, "deep band should be rarer");
    }

    #[test]
    fn distribution_stable_across_slab() {
        // Recomputing the slab gives identical results (no hidden state).
        let a = slab(-20, 5, 50);
        let b = slab(-20, 5, 50);
        assert_eq!(a.0, b.0);
        assert_eq!(a.1, b.1);
    }

    #[test]
    fn different_seed_may_differ() {
        let mut differ = false;
        for x in 0..200 {
            if ore_at(x, 0, 0, 1) != ore_at(x, 0, 0, 2) {
                differ = true;
                break;
            }
        }
        assert!(differ, "two seeds produced identical fields");
    }
}
