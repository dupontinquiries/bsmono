//! `sim::combat_resolve` — PvE hit resolution: turn an attack value, a defence
//! value, and a crit roll into a concrete [`Hit`] (damage, knockback, whether it
//! crit). Pairs with `fall_impact` as the other source of incoming force; this
//! is the creature-vs-creature side and is **PvE only** — there is no
//! player-target path here, so it can't be used to grief other players.
//!
//! The shape:
//! - damage rises with `atk` and falls with `def` (`atk / (1 + def)`), so it is
//!   monotonic each way and never negative;
//! - a crit (`crit` roll at or above [`CRIT_THRESHOLD`]) multiplies damage by
//!   [`CRIT_MULT`] and sets the flag;
//! - knockback scales with damage but is clamped to [`KNOCKBACK_MAX`], so a huge
//!   hit can't launch a creature arbitrarily far.
//!
//! Pure and deterministic: simple arithmetic, no clock, no randomness.

use crate::sim::{crit_roll, damage_after_armor, hit_chance, overkill_damage, resist_apply};

/// Crit roll at or above this value lands a critical hit.
pub const CRIT_THRESHOLD: f32 = 0.80;
/// Damage multiplier on a critical hit.
pub const CRIT_MULT: f32 = 2.0;
/// How much knockback one point of damage produces (before clamping).
pub const KNOCKBACK_PER_DAMAGE: f32 = 0.5;
/// Maximum knockback any single hit can impart.
pub const KNOCKBACK_MAX: f32 = 10.0;

/// The resolved outcome of one hit.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hit {
    /// Damage dealt, `>= 0`.
    pub damage: f32,
    /// Knockback impulse, `0 ..= KNOCKBACK_MAX`.
    pub knockback: f32,
    /// Whether the hit was a critical.
    pub crit: bool,
}

fn clampf(x: f32, lo: f32, hi: f32) -> f32 {
    let x = if x > lo { x } else { lo };
    if x < hi {
        x
    } else {
        hi
    }
}

/// Resolve a hit from an attack value, a defence value, and a crit roll.
pub fn resolve(atk: f32, def: f32, crit: f32) -> Hit {
    let atk = if atk > 0.0 { atk } else { 0.0 };
    let def = if def > 0.0 { def } else { 0.0 };

    let base = atk / (1.0 + def);
    let is_crit = crit >= CRIT_THRESHOLD;
    let damage = if is_crit { base * CRIT_MULT } else { base };
    let damage = if damage > 0.0 { damage } else { 0.0 };

    let knockback = clampf(damage * KNOCKBACK_PER_DAMAGE, 0.0, KNOCKBACK_MAX);

    Hit {
        damage,
        knockback,
        crit: is_crit,
    }
}

/// One PvE attack's inputs for the full pipeline. Like [`resolve`] this is the
/// creature-vs-creature path only — no player-target route, so it can't be used
/// to grief other players. `hit_roll`/`crit_roll` are caller-supplied `[0,1)`
/// rolls (kept out so this stays pure/deterministic).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Attack {
    pub accuracy: f32,
    pub evasion: f32,
    pub hit_roll: f32,
    pub base_damage: f32,
    pub crit_roll: f32,
    pub crit_chance: f32,
    pub crit_mult: f32,
    pub armor: f32,
    /// Armor softening constant `k` for `damage_after_armor::mitigated`.
    pub armor_k: f32,
    /// Elemental resist `[0,1]`.
    pub resist: f32,
    /// Target's current HP, for the overkill spill.
    pub target_hp: f32,
}

/// The full resolved outcome: whether it landed/crit, the damage applied after
/// armor + resist, and overkill spill past the target's HP.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FullHit {
    pub landed: bool,
    pub crit: bool,
    pub damage: f32,
    pub overkill: f32,
}

/// Full PvE attack resolution, composing the per-stage cores in order:
/// `hit_chance` (land?) → `crit_roll::crit` (crit multiplier) →
/// `damage_after_armor::mitigated` (armor) → `resist_apply::after_resist`
/// (element) → `overkill_damage::overkill` (spill). A miss yields zero damage.
/// Pure and deterministic; pairs the previously-disjoint combat primitives the
/// minimal [`resolve`] doesn't use. This uses an `armor` mitigation model (not
/// the `def` divisor of [`resolve`]), so the two don't double-mitigate.
pub fn resolve_full(a: &Attack) -> FullHit {
    let p_hit = hit_chance::hit_chance(a.accuracy, a.evasion);
    if !(a.hit_roll < p_hit) {
        return FullHit { landed: false, crit: false, damage: 0.0, overkill: 0.0 };
    }
    let cm = crit_roll::crit(a.crit_roll, a.crit_chance, a.crit_mult);
    let raw = a.base_damage.max(0.0) * cm;
    let after_armor = damage_after_armor::mitigated(raw, a.armor, a.armor_k);
    let damage = resist_apply::after_resist(after_armor, a.resist);
    let overkill = overkill_damage::overkill(damage, a.target_hp);
    FullHit { landed: true, crit: cm > 1.0, damage, overkill }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn pos(seed: &mut u32) -> f32 {
        (lcg(seed) % 100_001) as f32 / 1000.0 // [0, 100]
    }

    #[test]
    fn higher_attack_deals_more_damage() {
        let mut seed = 0xA77C_u32;
        for _ in 0..3000 {
            let def = pos(&mut seed);
            let crit = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let a1 = pos(&mut seed);
            let a2 = pos(&mut seed);
            let lo = a1.min(a2);
            let hi = a1.max(a2);
            let d_lo = resolve(lo, def, crit).damage;
            let d_hi = resolve(hi, def, crit).damage;
            assert!(d_hi >= d_lo, "{} < {}", d_hi, d_lo);
        }
    }

    #[test]
    fn defence_reduces_damage() {
        let mut seed = 0xDEFE_u32;
        for _ in 0..3000 {
            let atk = pos(&mut seed) + 1.0; // ensure positive damage
            let crit = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let r1 = pos(&mut seed);
            let r2 = pos(&mut seed);
            let lo = r1.min(r2);
            let hi = r1.max(r2);
            let d_lo = resolve(atk, lo, crit).damage;
            let d_hi = resolve(atk, hi, crit).damage;
            assert!(d_hi <= d_lo, "more defence raised damage");
        }
    }

    #[test]
    fn crit_only_past_threshold_and_hits_harder() {
        let atk = 10.0;
        let def = 1.0;
        let below = resolve(atk, def, CRIT_THRESHOLD - 0.01);
        let at = resolve(atk, def, CRIT_THRESHOLD);
        assert!(!below.crit);
        assert!(at.crit);
        assert!(at.damage > below.damage, "crit should hit harder");
        assert!((at.damage - below.damage * CRIT_MULT).abs() < 1e-4);
    }

    #[test]
    fn damage_never_negative() {
        let mut seed = 0xD006_u32;
        for _ in 0..4000 {
            // include negative atk/def to probe the clamps
            let atk = pos(&mut seed) - 20.0;
            let def = pos(&mut seed) - 20.0;
            let crit = (lcg(&mut seed) % 200_001) as f32 / 100_000.0 - 0.5;
            assert!(resolve(atk, def, crit).damage >= 0.0);
        }
    }

    #[test]
    fn knockback_is_bounded() {
        let mut seed = 0x9B0C_u32;
        for _ in 0..4000 {
            let atk = pos(&mut seed) * 100.0; // can be very large
            let def = pos(&mut seed);
            let crit = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let kb = resolve(atk, def, crit).knockback;
            assert!(kb >= 0.0 && kb <= KNOCKBACK_MAX, "knockback {}", kb);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xC077_u32;
        for _ in 0..3000 {
            let atk = pos(&mut seed);
            let def = pos(&mut seed);
            let crit = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            assert_eq!(resolve(atk, def, crit), resolve(atk, def, crit));
        }
    }

    fn attack() -> Attack {
        Attack {
            accuracy: 100.0, evasion: 0.0, hit_roll: 0.0, base_damage: 100.0,
            crit_roll: 1.0, crit_chance: 0.0, crit_mult: 2.0,
            armor: 0.0, armor_k: 50.0, resist: 0.0, target_hp: 1e9,
        }
    }

    #[test]
    fn full_miss_deals_nothing() {
        let mut a = attack();
        a.accuracy = 0.0; a.evasion = 100.0; a.hit_roll = 0.5; // hit_chance 0 ⇒ miss
        let h = resolve_full(&a);
        assert!(!h.landed && h.damage == 0.0 && h.overkill == 0.0);
    }

    #[test]
    fn full_clean_hit_is_full_base_damage() {
        let h = resolve_full(&attack());
        assert!(h.landed && !h.crit);
        assert!((h.damage - 100.0).abs() < 1e-3, "no armor/resist ⇒ full base damage");
    }

    #[test]
    fn full_crit_multiplies_damage() {
        let mut c = attack();
        c.crit_chance = 1.0; c.crit_roll = 0.0;
        let dc = resolve_full(&c);
        let dnc = resolve_full(&attack());
        assert!(dc.crit && dc.damage > dnc.damage);
        assert!((dc.damage - 2.0 * dnc.damage).abs() < 1e-3, "crit_mult 2.0 doubles damage");
    }

    #[test]
    fn full_armor_and_resist_reduce_damage() {
        let mut ar = attack(); ar.armor = 100.0;
        assert!(resolve_full(&ar).damage < resolve_full(&attack()).damage, "armor mitigates");
        let mut rs = attack(); rs.resist = 0.5;
        assert!((resolve_full(&rs).damage - 50.0).abs() < 1e-3, "50% resist halves damage");
    }

    #[test]
    fn full_overkill_spills_past_hp() {
        let mut a = attack(); a.target_hp = 30.0; // 100 dmg vs 30 hp
        assert!((resolve_full(&a).overkill - 70.0).abs() < 1e-3, "100 dmg vs 30 hp ⇒ 70 overkill");
        assert_eq!(resolve_full(&attack()).overkill, 0.0, "huge hp ⇒ no spill");
    }

    #[test]
    fn full_deterministic() {
        assert_eq!(resolve_full(&attack()), resolve_full(&attack()));
    }
}
