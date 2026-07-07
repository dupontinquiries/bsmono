//! `sim::salvage_yield` — deterministic item disassembly into materials.
//!
//! [`salvage`] maps a [`SalvageInput`] to a [`SalvageYield`] using only
//! saturating integer arithmetic — no RNG, no clock, no cross-imports beyond
//! the input struct. Any random variation on top (lucky salvage, guild perks)
//! is the caller's job.
//!
//! # Yield formulae
//!
//! ```text
//! common_units = floor(base_material_units * condition).saturating
//! rare_units   = rarity * rarity_scale_per_tier   (rises with rarity tier)
//! essence      = (affix_count * affix_scale)
//!              + (rarity * rarity_essence_scale)
//!              + (item_level / level_essence_divisor)
//! ```
//!
//! All arithmetic is saturating so absurdly large inputs produce `u32::MAX`
//! rather than wrapping or panicking.
//!
//! # Relation to companion modules
//!
//! `rarity_tier` maps a quality score to a tier; here the caller already holds
//! the tier as a raw `u8`. `affix_count` computes how many affixes an item has;
//! here the caller holds the count directly. `item_durability` tracks wear;
//! `condition` is the normalised form of that (`durability / max_durability`).

// ── Scale constants (tune here, never in the formula) ─────────────────────────

/// Common units per `base_material_units` point at full condition (×1).
/// At condition 0 the product is 0; at condition 1 it is `base_material_units`.
/// Internal: we multiply by this constant (== 1) so the formula is uniform.
const COMMON_SCALE: u32 = 1;

/// Rare units awarded per rarity tier (linear with rarity).
const RARE_PER_RARITY: u32 = 3;

/// Essence awarded per affix.
const ESSENCE_PER_AFFIX: u32 = 2;

/// Essence awarded per rarity tier.
const ESSENCE_PER_RARITY: u32 = 4;

/// Essence awarded per this many item levels (integer divide, no partial credit).
const LEVEL_ESSENCE_DIVISOR: u16 = 20;

// ── Types ─────────────────────────────────────────────────────────────────────

/// All the properties of an item that determine salvage yield.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SalvageInput {
    /// Rarity tier: 0 = common, higher = rarer. Matches `rarity_tier` ordering.
    pub rarity: u8,
    /// Number of affixes (from `affix_count`).
    pub affix_count: u8,
    /// Condition 0.0 (broken) to 1.0 (pristine). Clamped internally.
    pub condition: f32,
    /// The raw material potential of this item type at full condition.
    pub base_material_units: u32,
    /// Item level. Contributes mildly to essence.
    pub item_level: u16,
}

/// The materials produced when disassembling an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SalvageYield {
    /// Bulk crafting material (scales with `condition` and `base_material_units`).
    pub common_units: u32,
    /// High-quality material (scales with `rarity`).
    pub rare_units: u32,
    /// Magical essence (scales with `affix_count`, `rarity`, `item_level`).
    pub essence: u32,
}

// ── Core function ─────────────────────────────────────────────────────────────

/// Compute the deterministic salvage yield for `input`.
///
/// All arithmetic saturates at `u32::MAX`; never panics.
pub fn salvage(input: &SalvageInput) -> SalvageYield {
    let condition = input.condition.clamp(0.0, 1.0);

    // common_units: base * condition, floor, saturating.
    // Multiply base by COMMON_SCALE (== 1) first so the formula shape is clear
    // and the constant can be tuned without touching the arithmetic.
    let common_units = {
        let scaled_base = input.base_material_units.saturating_mul(COMMON_SCALE);
        // Multiply by condition (a [0,1] float). Cast via u64 to avoid
        // intermediate f32 overflow on large base values.
        let units_f = scaled_base as f64 * condition as f64;
        // Floor + saturating cast to u32.
        let floored = units_f.floor() as u64;
        floored.min(u32::MAX as u64) as u32
    };

    // rare_units: linear with rarity tier.
    let rare_units = (input.rarity as u32).saturating_mul(RARE_PER_RARITY);

    // essence: affix + rarity + level contributions, all saturating.
    let affix_essence  = (input.affix_count as u32).saturating_mul(ESSENCE_PER_AFFIX);
    let rarity_essence = (input.rarity as u32).saturating_mul(ESSENCE_PER_RARITY);
    let level_essence  = (input.item_level / LEVEL_ESSENCE_DIVISOR.max(1)) as u32;

    let essence = affix_essence
        .saturating_add(rarity_essence)
        .saturating_add(level_essence);

    SalvageYield { common_units, rare_units, essence }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn input(
        rarity: u8, affix_count: u8, condition: f32,
        base_material_units: u32, item_level: u16,
    ) -> SalvageInput {
        SalvageInput { rarity, affix_count, condition, base_material_units, item_level }
    }

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    // ── Condition boundary ──

    #[test]
    fn condition_zero_gives_zero_common() {
        let y = salvage(&input(0, 0, 0.0, 100, 10));
        assert_eq!(y.common_units, 0);
    }

    #[test]
    fn condition_one_gives_full_common() {
        let y = salvage(&input(0, 0, 1.0, 100, 10));
        assert_eq!(y.common_units, 100);
    }

    #[test]
    fn base_material_zero_gives_zero_common() {
        let y = salvage(&input(3, 2, 1.0, 0, 50));
        assert_eq!(y.common_units, 0);
    }

    // ── Monotonicity: common_units non-decreasing with condition ──

    #[test]
    fn common_nondecreasing_with_condition() {
        let base = 200u32;
        let mut prev = 0u32;
        let mut cond = 0.0f32;
        while cond <= 1.0 {
            let y = salvage(&input(0, 0, cond, base, 0));
            assert!(y.common_units >= prev,
                "common_units decreased: {} → {} at cond {}", prev, y.common_units, cond);
            prev = y.common_units;
            cond += 0.05;
        }
    }

    // ── Monotonicity: rare_units non-decreasing with rarity ──

    #[test]
    fn rare_nondecreasing_with_rarity() {
        let mut prev = 0u32;
        for r in 0u8..=20 {
            let y = salvage(&input(r, 0, 1.0, 50, 10));
            assert!(y.rare_units >= prev,
                "rare_units decreased at rarity {}: {} → {}", r, prev, y.rare_units);
            prev = y.rare_units;
        }
    }

    // ── Monotonicity: essence non-decreasing with rarity ──

    #[test]
    fn essence_nondecreasing_with_rarity() {
        let mut prev = 0u32;
        for r in 0u8..=20 {
            let y = salvage(&input(r, 2, 0.5, 50, 10));
            assert!(y.essence >= prev,
                "essence decreased at rarity {}: {} → {}", r, prev, y.essence);
            prev = y.essence;
        }
    }

    // ── Monotonicity: essence non-decreasing with affix_count ──

    #[test]
    fn essence_nondecreasing_with_affix_count() {
        let mut prev = 0u32;
        for a in 0u8..=20 {
            let y = salvage(&input(2, a, 0.5, 50, 10));
            assert!(y.essence >= prev,
                "essence decreased at affix_count {}: {} → {}", a, prev, y.essence);
            prev = y.essence;
        }
    }

    // ── Monotonicity: essence rises with item_level ──

    #[test]
    fn essence_nondecreasing_with_item_level() {
        let mut prev = 0u32;
        let mut lvl = 0u16;
        while lvl < 500 {
            let y = salvage(&input(1, 1, 0.5, 50, lvl));
            assert!(y.essence >= prev,
                "essence decreased at level {}: {} → {}", lvl, prev, y.essence);
            prev = y.essence;
            lvl += 20;
        }
    }

    // ── Higher rarity ⇒ more rare_units and essence (strict for nonzero scale) ──

    #[test]
    fn higher_rarity_gives_more_rare_and_essence() {
        let y_low  = salvage(&input(1, 1, 0.5, 50, 10));
        let y_high = salvage(&input(5, 1, 0.5, 50, 10));
        assert!(y_high.rare_units > y_low.rare_units);
        assert!(y_high.essence    >= y_low.essence);
    }

    // ── Saturation: huge inputs do not overflow ──

    #[test]
    fn huge_inputs_saturate_not_overflow() {
        let y = salvage(&input(255, 255, 1.0, u32::MAX, u16::MAX));
        // All fields must be finite u32 values — just checking no panic and
        // that they are at most u32::MAX (trivially true but explicit).
        assert!(y.common_units <= u32::MAX);
        assert!(y.rare_units   <= u32::MAX);
        assert!(y.essence      <= u32::MAX);
    }

    #[test]
    fn base_max_condition_one_saturates_gracefully() {
        let y1 = salvage(&input(0, 0, 1.0, u32::MAX, 0));
        let y2 = salvage(&input(0, 0, 1.0, u32::MAX, 0));
        assert_eq!(y1, y2);
        assert_eq!(y1.common_units, u32::MAX);
    }

    // ── Property: condition clamped outside [0,1] ──

    #[test]
    fn condition_clamped_above_one() {
        let y_clamped = salvage(&input(0, 0, 1.0,  80, 0));
        let y_above   = salvage(&input(0, 0, 99.0, 80, 0));
        assert_eq!(y_clamped, y_above);
    }

    #[test]
    fn condition_clamped_below_zero() {
        let y_clamped = salvage(&input(0, 0,  0.0, 80, 0));
        let y_below   = salvage(&input(0, 0, -9.0, 80, 0));
        assert_eq!(y_clamped, y_below);
    }

    // ── Determinism ──

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..2000 {
            let r     = (lcg(&mut seed) % 10) as u8;
            let a     = (lcg(&mut seed) % 10) as u8;
            let cond  = (lcg(&mut seed) % 101) as f32 / 100.0;
            let base  = lcg(&mut seed);
            let level = (lcg(&mut seed) % 500) as u16;
            let inp = input(r, a, cond, base, level);
            assert_eq!(salvage(&inp), salvage(&inp));
        }
    }

    // ── Property sweep: all fields are defined (no NaN / panic) ──

    #[test]
    fn no_panic_for_varied_inputs() {
        let mut seed = 0xA1CF_u32;
        for _ in 0..3000 {
            let r     = (lcg(&mut seed) % 256) as u8;
            let a     = (lcg(&mut seed) % 256) as u8;
            let cond  = (lcg(&mut seed) % 101) as f32 / 100.0;
            let base  = lcg(&mut seed);
            let level = (lcg(&mut seed) % 65536) as u16;
            let y = salvage(&input(r, a, cond, base, level));
            let _ = y.common_units + y.rare_units + y.essence; // no panic
        }
    }

    // ── Specific spot checks ──

    #[test]
    fn common_pristine_item_level_one() {
        // rarity=0, affix=0, condition=1.0, base=1, level=0
        let y = salvage(&input(0, 0, 1.0, 1, 0));
        assert_eq!(y.common_units, 1);
        assert_eq!(y.rare_units, 0);
        assert_eq!(y.essence, 0);
    }

    #[test]
    fn rare_item_no_affixes() {
        // rarity=3 → rare_units = 3*3 = 9, essence = 0+3*4+0 = 12
        let y = salvage(&input(3, 0, 0.5, 100, 0));
        assert_eq!(y.rare_units, 9);
        assert_eq!(y.essence, 12);
    }

    #[test]
    fn affix_and_level_contribute_to_essence() {
        // rarity=0, affix=3 → 3*2=6 essence; level=40 → 40/20=2 essence; total=8
        let y = salvage(&input(0, 3, 0.5, 0, 40));
        assert_eq!(y.essence, 8);
    }
}
