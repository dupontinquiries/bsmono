//! `sim::item_appraise` — a single trade VALUE for an item (round R30).
//!
//! The value sibling of `item_resolve` (loot) and `craft_resolve` (crafting):
//! one pure, deterministic function, [`appraise`], that prices an item from its
//! attributes via documented, monotone weightings. **No RNG** — value is a
//! function of the inputs alone.
//!
//! It **composes** the existing item cores rather than reimplementing them:
//! * `enchant_apply::apply` — the enchant value multiplier (base `1.0`,
//!   `+per_level`, capped), so more enchant ⇒ more value, bounded.
//! * `item_durability::apply_wear` — normalizes `(cur,max)` and reports the
//!   broken flag, giving the remaining-durability multiplier (broken items
//!   appraise well below intact ones).
//! * `item_modifier::MAX_MODS` — the cap on how many modifiers add value.
//! * `craft_quality::quality` — produces the `quality` input (its `[0,1]`
//!   output domain) upstream; the appraiser consumes that domain directly.
//!
//! VALUE = `base · qualityMult · enchantMult · durabilityMult · modMult ·
//! rarityMult`, each factor non-negative and non-decreasing in its attribute,
//! so the result is finite, `≥ 0`, and monotone in quality / enchant /
//! durability / modifier-count / rarity.
//!
//! Pure: logic in, data out — no `crate::render`, no GPU, no I/O, no clock, no
//! RNG, no `.unwrap()`/`.expect()` in non-test code.

use crate::sim::{enchant_apply, item_durability, item_modifier};

// ── documented weighting constants (R20: all noise-free, fixed) ──
/// Quality maps `[0,1]` → `[0.5, 2.0]×`.
const QUALITY_LO: f32 = 0.5;
const QUALITY_SPAN: f32 = 1.5;
/// Enchant: `1.0×` at level 0, `+0.15×` per level, capped at `4.0×`.
const ENCHANT_PER_LEVEL: f32 = 0.15;
const ENCHANT_CAP: f32 = 4.0;
/// Durability: intact `1.0×`, scaling down to `DUR_FLOOR` as it wears; a *broken*
/// item is floored lower still so it always appraises below the same item intact.
const DUR_FLOOR: f32 = 0.25;
const BROKEN_FLOOR: f32 = 0.10;
/// Each modifier (up to `item_modifier::MAX_MODS`) adds `+0.20×`.
const MOD_PER: f32 = 0.20;
/// Rarity multiplier table, strictly increasing, indexed by tier `0..=5`.
const RARITY_MULT: [f32; 6] = [1.0, 1.4, 2.0, 3.0, 4.5, 7.0];

/// Attributes of the item being appraised.
#[derive(Clone, Debug)]
pub struct AppraiseInput<'a> {
    /// Intrinsic base trade value (clamped `≥ 0`).
    pub base_value: f32,
    /// Rarity tier `0..=5` (the `rarity_tier` domain); higher is worth more.
    pub rarity: u8,
    /// Item quality `[0,1]` (the `craft_quality::quality` output domain).
    pub quality: f32,
    /// Enchant level.
    pub enchant_level: u8,
    /// Current durability.
    pub durability_cur: i32,
    /// Max durability; `≤ 0` means the item has no durability system (no penalty).
    pub durability_max: i32,
    /// The item's modifier ids (only the count matters, capped at `MAX_MODS`).
    pub modifiers: &'a [u16],
}

fn clamp01(x: f32) -> f32 {
    crate::util::mathx::clamp01(x) // r343 dedup
}

/// The durability multiplier `[BROKEN_FLOOR or DUR_FLOOR .. 1.0]`, via
/// `item_durability`'s clamp + broken semantics. `max ≤ 0` ⇒ not applicable ⇒ 1.0.
fn durability_mult(cur: i32, max: i32) -> f32 {
    if max <= 0 {
        return 1.0; // no durability system — don't penalise
    }
    // delta 0: clamp cur into [0,max] and learn whether it's broken.
    let (clamped, broke) = item_durability::apply_wear(cur, max, 0);
    if broke {
        BROKEN_FLOOR
    } else {
        let frac = clamped as f32 / max as f32; // (0,1]
        DUR_FLOOR + (1.0 - DUR_FLOOR) * frac
    }
}

/// Compute the trade value of an item. Always finite and `≥ 0`; non-decreasing
/// in quality, enchant level, remaining durability, modifier count, and rarity.
pub fn appraise(input: &AppraiseInput) -> f32 {
    let base = input.base_value.max(0.0);

    let quality_mult = QUALITY_LO + QUALITY_SPAN * clamp01(input.quality);

    // enchant value multiplier (base 1.0) — monotone in level, capped.
    let enchant_mult = enchant_apply::apply(1.0, input.enchant_level, ENCHANT_PER_LEVEL, ENCHANT_CAP);

    let dur_mult = durability_mult(input.durability_cur, input.durability_max);

    let n_mods = input.modifiers.len().min(item_modifier::MAX_MODS);
    let mod_mult = 1.0 + MOD_PER * n_mods as f32;

    let rarity_mult = RARITY_MULT[(input.rarity as usize).min(RARITY_MULT.len() - 1)];

    let value = base * quality_mult * enchant_mult * dur_mult * mod_mult * rarity_mult;
    if value.is_finite() && value >= 0.0 {
        value
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input<'a>(mods: &'a [u16]) -> AppraiseInput<'a> {
        AppraiseInput {
            base_value: 100.0,
            rarity: 2,
            quality: 0.5,
            enchant_level: 1,
            durability_cur: 80,
            durability_max: 100,
            modifiers: mods,
        }
    }

    #[test]
    fn value_is_finite_and_nonneg() {
        for &base in &[0.0f32, 1.0, 250.0, 1e6] {
            for rarity in 0u8..8 {
                for q in [0.0f32, 0.5, 1.0] {
                    for lvl in [0u8, 3, 200] {
                        for &(cur, max) in &[(0i32, 100i32), (50, 100), (100, 100), (5, 0), (-9, 100)] {
                            let inp = AppraiseInput {
                                base_value: base,
                                rarity,
                                quality: q,
                                enchant_level: lvl,
                                durability_cur: cur,
                                durability_max: max,
                                modifiers: &[1, 2, 3],
                            };
                            let v = appraise(&inp);
                            assert!(v.is_finite() && v >= 0.0, "value {v} not finite/≥0");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn zero_base_is_zero() {
        let mut inp = input(&[1, 2]);
        inp.base_value = 0.0;
        assert_eq!(appraise(&inp), 0.0);
    }

    #[test]
    fn monotone_in_quality() {
        let mods = [1u16];
        let mut lo = input(&mods);
        lo.quality = 0.1;
        let mut hi = input(&mods);
        hi.quality = 0.9;
        assert!(appraise(&hi) > appraise(&lo));
    }

    #[test]
    fn monotone_in_enchant() {
        let mods = [1u16];
        let mut lo = input(&mods);
        lo.enchant_level = 0;
        let mut hi = input(&mods);
        hi.enchant_level = 5;
        assert!(appraise(&hi) >= appraise(&lo));
        assert!(appraise(&hi) > appraise(&lo), "below cap, more enchant is worth more");
    }

    #[test]
    fn monotone_in_durability_and_broken_below_intact() {
        let mods = [1u16];
        let mk = |cur: i32| {
            let mut i = input(&mods);
            i.durability_cur = cur;
            i.durability_max = 100;
            appraise(&i)
        };
        assert!(mk(100) > mk(50), "more remaining durability ⇒ more value");
        assert!(mk(50) > mk(0), "intact worn ⇒ above broken");
        // a broken item (durability 0) is worth less than the same item intact
        assert!(mk(0) < mk(100));
    }

    #[test]
    fn rarity_ordering_respected() {
        let mods = [1u16];
        let mut prev = -1.0f32;
        for rarity in 0u8..=5 {
            let mut i = input(&mods);
            i.rarity = rarity;
            let v = appraise(&i);
            assert!(v > prev, "rarity {rarity} value {v} must exceed previous {prev}");
            prev = v;
        }
    }

    #[test]
    fn more_modifiers_more_value_capped() {
        let none: [u16; 0] = [];
        let three = [1u16, 2, 3];
        let many = [1u16, 2, 3, 4, 5, 6, 7]; // beyond MAX_MODS
        let v0 = appraise(&input(&none));
        let v3 = appraise(&input(&three));
        let vm = appraise(&input(&many));
        assert!(v3 > v0, "modifiers add value");
        // capped at MAX_MODS: 7 mods values the same as exactly MAX_MODS
        let exactly_max: Vec<u16> = (0..item_modifier::MAX_MODS as u16).collect();
        let vcap = appraise(&input(&exactly_max));
        assert!((vm - vcap).abs() < 1e-3, "value caps at MAX_MODS modifiers");
    }

    #[test]
    fn no_durability_system_is_not_penalised() {
        // max <= 0 ⇒ treated as "no durability", full multiplier (not broken).
        let mods = [1u16];
        let mut no_dur = input(&mods);
        no_dur.durability_max = 0;
        no_dur.durability_cur = 0;
        let mut intact = input(&mods);
        intact.durability_cur = 100;
        intact.durability_max = 100;
        assert!((appraise(&no_dur) - appraise(&intact)).abs() < 1e-3);
    }

    #[test]
    fn deterministic() {
        let mods = [1u16, 2];
        assert_eq!(appraise(&input(&mods)), appraise(&input(&mods)));
    }
}
