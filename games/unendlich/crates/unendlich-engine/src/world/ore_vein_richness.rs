//! `ore_vein_richness` — ore density by depth and rarity.
//!
//! Worker-03 / WORLD pure core, round 17. A richness multiplier in [0,1]:
//! richness rises with depth to a peak band then tapers (the sweet-spot depth
//! for a given ore), and scales down with the ore's rarity. Pairs with
//! `ore_distribution` (R9, the spawn gate) and `mineral_vein_trace` (R12, the
//! vein shape) — this weights how rich a hit is.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

const PEAK_DEPTH: f32 = 48.0; // depth of maximum richness
const PEAK_WIDTH: f32 = 32.0; // spread of the rich band

/// Richness in [0,1] at `depth` for an ore of `rarity` (0 = common, 1 = rarest).
/// Peaks at `PEAK_DEPTH`; rarer ores yield a lower ceiling.
pub fn richness(depth: f32, rarity: f32) -> f32 {
    let r = clamp01(rarity);
    let d = depth.max(0.0);
    let x = (d - PEAK_DEPTH) / PEAK_WIDTH;
    let depth_factor = (-(x * x)).exp(); // gaussian bump, 1 at the peak
    let rarity_factor = 1.0 - r; // rarer ⇒ lower
    clamp01(depth_factor * rarity_factor)
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deeper_richer_up_to_a_peak() {
        let shallow = richness(5.0, 0.0);
        let peak = richness(PEAK_DEPTH, 0.0);
        let very_deep = richness(PEAK_DEPTH + 4.0 * PEAK_WIDTH, 0.0);
        assert!(peak > shallow, "should rise toward the peak");
        assert!(peak > very_deep, "should taper past the peak");
    }

    #[test]
    fn rarer_is_lower() {
        assert!(richness(PEAK_DEPTH, 0.2) > richness(PEAK_DEPTH, 0.8));
    }

    #[test]
    fn bounded() {
        for d in 0..300 {
            for ri in 0..=10 {
                let v = richness(d as f32, ri as f32 / 10.0);
                assert!(v >= 0.0 && v <= 1.0);
            }
        }
    }

    #[test]
    fn monotone_rising_region() {
        // On [0, PEAK_DEPTH], richness is non-decreasing in depth (rarity fixed).
        let mut prev = -1.0f32;
        let mut d = 0.0;
        while d <= PEAK_DEPTH {
            let v = richness(d, 0.0);
            assert!(v >= prev - 1e-6, "richness dipped before the peak");
            prev = v;
            d += 1.0;
        }
    }

    #[test]
    fn rarity_zero_edge_reaches_peak_one() {
        assert!((richness(PEAK_DEPTH, 0.0) - 1.0).abs() < 1e-5);
    }

    #[test]
    fn deterministic() {
        assert_eq!(richness(40.0, 0.3), richness(40.0, 0.3));
    }
}
