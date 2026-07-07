//! `sim::legendary_loot` — the pure drop-selection core for legendary weapons.
//!
//! WID: `r330-swords`. Closes the loot half of "get the legendary swords into
//! the game": given a roll seed and a drop context, decide **whether** a
//! legendary drops and, if so, produce the compact [`LegendarySpec`] the
//! inventory/loot system carries. The full stat-block is never built here —
//! `LegendarySpec::weapon()` reconstructs it on demand via
//! `legendary_weapon::forge`, keeping the forge the single source of truth.
//!
//! ## Purity (SHARED_RULES §1)
//! No RNG, no clock, no IO. The only entropy is the **passed-in** `roll_seed`,
//! expanded with splitmix64 (the same mix `legendary_weapon` uses) into
//! independent per-decision streams. No `unwrap`/`expect`/`panic!`; all float
//! math is clamped/finite. Deterministic: same `(roll_seed, ctx)` → same result.
//!
//! ## How the decision is made
//! 1. `chance = drop_chance(base_rate, luck, rarity_penalty)` (reuses
//!    `sim::drop_chance`). `chance <= 0` → never drops; `chance >= 1` → always.
//! 2. **Rarity gate:** a seeded quality score → `rarity_tier::tier(score)`
//!    (0..5). If that tier is below `min_tier`, no legendary drops — this is the
//!    "only rare enough rolls become legendaries" gate. The surviving tier
//!    becomes the weapon's `tier`.
//! 3. If it passes the gate and wins the chance roll, class (1 of 8) and element
//!    (mod 6) are drawn from further splitmix streams, and the forge seed is a
//!    decorrelated mix of `roll_seed` so the item's stats don't echo the roll.

use crate::item::LegendarySpec;
use crate::sim::drop_chance::drop_chance;
use crate::sim::legendary_weapon::WeaponClass;
use crate::sim::rarity_tier::tier;

// ── splitmix64 (same constants as `legendary_weapon::sm64`, which is private) ──
fn sm64(seed: u64) -> u64 {
    let mut z = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Map a sm64 output to [0, 1) — top 24 bits, matching `legendary_weapon`.
fn sm64_f32(v: u64) -> f32 {
    (v >> 40) as f32 / (1u64 << 24) as f32
}

// Distinct decision salts (one per draw, so the streams are independent).
const SALT_ROLL: u64 = 0x10D7_0000_0001_0001; // the drop hit/miss roll
const SALT_SCORE: u64 = 0x5C0E_0000_0002_0002; // quality score → rarity tier
const SALT_CLASS: u64 = 0xC1A5_0000_0003_0003; // weapon class pick
const SALT_ELEM: u64 = 0xE1E0_0000_0004_0004; // element pick
const SALT_WSEED: u64 = 0x5EED_0000_0005_0005; // the forge seed for the weapon

/// Inputs the drop site supplies. Pure data; all fields are clamped on use.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DropContext {
    /// Base legendary drop rate in [0,1] before luck/penalty.
    pub base_rate: f32,
    /// Luck boost (>= 0); raises the effective chance.
    pub luck: f32,
    /// Rarity penalty in [0,1]; lowers the effective chance.
    pub rarity_penalty: f32,
    /// Rarity gate: only emit a legendary whose rolled tier is >= this (0..5).
    /// Use 0 for "no gate" (chance alone decides), 4 for "legendary-tier+ only".
    pub min_tier: u8,
}

impl DropContext {
    /// A convenience: chance-only context with no rarity gate.
    pub fn ungated(base_rate: f32, luck: f32, rarity_penalty: f32) -> Self {
        Self { base_rate, luck, rarity_penalty, min_tier: 0 }
    }
}

/// Map a raw draw to one of the 8 weapon classes.
fn class_from(v: u64) -> WeaponClass {
    match v % 8 {
        0 => WeaponClass::Dagger,
        1 => WeaponClass::Blade,
        2 => WeaponClass::Greatsword,
        3 => WeaponClass::Axe,
        4 => WeaponClass::Greataxe,
        5 => WeaponClass::Spear,
        6 => WeaponClass::Staff,
        _ => WeaponClass::Wand,
    }
}

/// Decide whether a legendary drops for this `(roll_seed, ctx)`, returning the
/// `LegendarySpec` if so. Deterministic and panic-free.
///
/// - `chance <= 0` → always `None`. `chance >= 1` with `min_tier == 0` → always
///   `Some`. Between, a seeded roll decides.
/// - The rarity gate can still veto a winning chance roll if the rolled tier is
///   below `min_tier`.
pub fn roll(roll_seed: u64, ctx: DropContext) -> Option<LegendarySpec> {
    let chance = drop_chance(ctx.base_rate, ctx.luck, ctx.rarity_penalty);
    if chance <= 0.0 {
        return None; // 0% never drops
    }

    // Rarity gate: a seeded quality score → tier; gate, then keep as weapon tier.
    let score = sm64_f32(sm64(roll_seed ^ SALT_SCORE));
    let rolled_tier = tier(score);
    if rolled_tier < ctx.min_tier {
        return None; // not rare enough this time
    }

    // The hit/miss roll. `chance >= 1` short-circuits to a guaranteed hit so the
    // "100% always" contract holds exactly (no float edge at u == 1.0).
    if chance < 1.0 {
        let u = sm64_f32(sm64(roll_seed ^ SALT_ROLL));
        if u >= chance {
            return None; // rolled a miss
        }
    }

    Some(LegendarySpec {
        seed: sm64(roll_seed ^ SALT_WSEED),
        class: class_from(sm64(roll_seed ^ SALT_CLASS)),
        element_idx: (sm64(roll_seed ^ SALT_ELEM) % 6) as u8,
        tier: rolled_tier,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_chance_never_drops() {
        let ctx = DropContext::ungated(0.0, 5.0, 0.0); // base 0 → chance 0
        for s in 0u64..256 {
            assert!(roll(s, ctx).is_none());
        }
        // Full penalty also zeroes the chance.
        let ctx2 = DropContext::ungated(0.9, 5.0, 1.0);
        for s in 0u64..256 {
            assert!(roll(s, ctx2).is_none());
        }
    }

    #[test]
    fn full_chance_no_gate_always_drops() {
        let ctx = DropContext::ungated(1.0, 0.0, 0.0); // chance clamps to 1
        for s in 0u64..256 {
            assert!(roll(s, ctx).is_some(), "100% with no gate must always drop (seed {s})");
        }
    }

    #[test]
    fn deterministic() {
        let ctx = DropContext::ungated(0.5, 1.0, 0.2);
        for s in [0u64, 1, 42, 0xABCD_EF01, u64::MAX] {
            assert_eq!(roll(s, ctx), roll(s, ctx));
        }
    }

    #[test]
    fn rarity_gate_can_veto_even_at_full_chance() {
        // min_tier 5 (mythic) at 100% chance: only the seeds whose quality score
        // rolls a 5 survive. So we expect SOME None and SOME Some across seeds —
        // proving the gate is actually consulted (not bypassed by full chance).
        let ctx = DropContext { base_rate: 1.0, luck: 0.0, rarity_penalty: 0.0, min_tier: 5 };
        let mut dropped = 0;
        let mut vetoed = 0;
        for s in 0u64..2000 {
            match roll(s, ctx) {
                Some(spec) => {
                    dropped += 1;
                    assert_eq!(spec.tier, 5, "a gated drop carries the gated tier");
                }
                None => vetoed += 1,
            }
        }
        assert!(dropped > 0, "some high-rarity rolls should pass the gate");
        assert!(vetoed > 0, "the gate should veto sub-tier rolls at full chance");
    }

    #[test]
    fn different_seeds_give_different_specs() {
        let ctx = DropContext::ungated(1.0, 0.0, 0.0);
        let a = roll(1, ctx).expect("drops");
        let b = roll(2, ctx).expect("drops");
        // Different seeds should (overwhelmingly) differ somewhere in the tuple.
        assert!(
            a.seed != b.seed || a.class != b.class || a.element_idx != b.element_idx || a.tier != b.tier,
            "distinct seeds produced identical specs"
        );
    }

    #[test]
    fn produced_spec_forges_a_finite_weapon() {
        let ctx = DropContext::ungated(1.0, 0.0, 0.0);
        for s in 0u64..256 {
            if let Some(spec) = roll(s, ctx) {
                let w = spec.weapon();
                assert!(w.base_damage.is_finite() && w.base_damage > 0.0);
                assert!(spec.element_idx < 6);
                // class is one of the 8 — exhaustively constructible, so just
                // assert the name is non-empty (proves it mapped to a real class).
                assert!(!spec.class_name().is_empty());
            }
        }
    }

    #[test]
    fn higher_luck_never_lowers_the_effective_chance() {
        // Monotonicity sanity at the boundary: across many seeds, more luck
        // drops at least as often (chance is non-decreasing in luck).
        let count = |luck: f32| {
            (0u64..2000)
                .filter(|&s| roll(s, DropContext::ungated(0.2, luck, 0.0)).is_some())
                .count()
        };
        assert!(count(2.0) >= count(0.0), "more luck should not drop fewer legendaries");
    }
}
