//! `sim::horn_type` — what kind of horns a creature grows, from its horn gene
//! and a rarity roll (see `HORN_SYSTEM`).
//!
//! [`horn_type`] maps a `gene` (`0..1`, roughly how horn-heavy the lineage is)
//! and a `rarity_roll` (`0..1`) to a [`HornArchetype`]: a tier, a pair count, a
//! branch factor, and a charge affinity. A low gene gives a common, sparse rack
//! (few pairs); higher genes climb through uncommon and rare tiers; the **exotic**
//! tier — with its huge pair counts — only unlocks on a rare roll, so it can't be
//! reached by gene alone. Pair count is banded per tier and rises with the gene.
//!
//! Pure and deterministic: band lookups and clamps — no clock, no randomness, no
//! cross-imports.

/// Rarity roll at or above which the exotic tier can be reached.
pub const EXOTIC_RARITY: f32 = 0.9;
/// Charge-affinity classes (`0 ..= AFFINITY_COUNT-1`).
pub const AFFINITY_COUNT: u8 = 5;

/// A rolled horn archetype.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HornArchetype {
    /// Rarity tier: 0 common, 1 uncommon, 2 rare, 3 exotic.
    pub tier: u8,
    /// Number of horn pairs (banded by tier).
    pub pairs: u8,
    /// How much the horns branch, `0..1`.
    pub branch_factor: f32,
    /// Charge affinity class, `0 ..= AFFINITY_COUNT-1`.
    pub charge_affinity: u8,
}

fn clamp01(x: f32) -> f32 {
    let x = if x > 0.0 { x } else { 0.0 };
    if x < 1.0 {
        x
    } else {
        1.0
    }
}

/// Inclusive pair-count band `(lo, hi)` for a tier.
fn band(tier: u8) -> (u8, u8) {
    match tier {
        0 => (0, 4),
        1 => (2, 8),
        2 => (4, 12),
        _ => (8, 24), // exotic
    }
}

/// Roll a horn archetype from a gene and a rarity roll.
pub fn horn_type(gene: f32, rarity_roll: f32) -> HornArchetype {
    let g = clamp01(gene);

    // base tier from the gene (0..=3)
    let raw_tier = (g * 4.0) as u8;
    let gene_tier = if raw_tier > 3 { 3 } else { raw_tier };

    // exotic only unlocks on a rare roll; otherwise cap at rare (tier 2)
    let tier = if gene_tier >= 3 && rarity_roll < EXOTIC_RARITY {
        2
    } else {
        gene_tier
    };

    // pairs within the tier's band, scaled by the gene
    let (lo, hi) = band(tier);
    let span = (hi - lo) as f32;
    let pairs = lo + (g * span).round() as u8;

    HornArchetype {
        tier,
        pairs,
        branch_factor: g,
        charge_affinity: ((g * AFFINITY_COUNT as f32) as u8).min(AFFINITY_COUNT - 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn low_gene_is_common_with_few_pairs() {
        let h = horn_type(0.1, 0.5);
        assert_eq!(h.tier, 0);
        assert!(h.pairs <= 4);
    }

    #[test]
    fn exotic_only_on_rare_roll() {
        // high gene, common roll → capped at rare (tier 2), not exotic
        assert_eq!(horn_type(0.95, 0.1).tier, 2);
        // high gene, rare roll → exotic
        assert_eq!(horn_type(0.95, 0.95).tier, 3);
    }

    #[test]
    fn pairs_in_band_per_tier() {
        let mut seed = 0x5111_u32;
        for _ in 0..5000 {
            let gene = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let roll = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let h = horn_type(gene, roll);
            let (lo, hi) = band(h.tier);
            assert!(h.pairs >= lo && h.pairs <= hi, "{} not in [{},{}]", h.pairs, lo, hi);
        }
    }

    #[test]
    fn affinity_and_branch_bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..4000 {
            let gene = (lcg(&mut seed) % 300_001) as f32 / 100_000.0 - 1.0; // out-of-range too
            let roll = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let h = horn_type(gene, roll);
            assert!(h.charge_affinity < AFFINITY_COUNT);
            assert!(h.branch_factor >= 0.0 && h.branch_factor <= 1.0);
            assert!(h.tier <= 3);
        }
    }

    #[test]
    fn pairs_monotone_in_gene() {
        // sweep gene with a rare roll so all tiers are reachable
        let mut prev = horn_type(0.0, 1.0).pairs;
        let mut i = 1;
        while i <= 1000 {
            let gene = i as f32 / 1000.0;
            let p = horn_type(gene, 1.0).pairs;
            assert!(p >= prev, "pairs dropped at gene {}: {} -> {}", gene, prev, p);
            prev = p;
            i += 1;
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let gene = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let roll = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            assert_eq!(horn_type(gene, roll), horn_type(gene, roll));
        }
    }
}
