//! sim::resonance — the character panel's gear score (Resonance Rating).
//!
//! From `design_library/08/character_panel_reference §6–7`: the panel shows a
//! single **POWER** number (Resonance Rating) that aggregates every equipped
//! item's genome, plus a **Resilience** score (0–10) that maps to physical
//! damage reduction. This is the pure model behind those two numbers, built on
//! the item genome pieces already in the engine (`item_genetics`, `infusion`,
//! `armor_set`): the caller reads each equipped item's genome fields, this
//! sums them into POWER and turns the armour's resilience into a mitigation
//! fraction. Deterministic, std-only.

/// Each sway mark on an item adds this to the Resonance Rating (per the doc).
pub const SWAY_MARK_BONUS: f32 = 12.0;
/// Each active enchantment adds this to the Resonance Rating.
pub const ENCHANTMENT_BONUS: f32 = 25.0;
/// Resilience is scored out of ten.
pub const MAX_RESILIENCE: f32 = 10.0;

/// One equipped item's contribution inputs to the Resonance Rating. The caller
/// fills these from the item's genome (`item_genetics`/`infusion`): purity and
/// resonance depth in `[0, 1]`, sway marks `0..=3`, plus its enchantment count.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GearPiece {
    pub base_item_level: f32,
    pub material_purity: f32,
    pub resonance_depth: f32,
    pub sway_marks: u8,
    pub enchantments: u8,
}

impl GearPiece {
    /// An empty/absent slot contributes nothing.
    pub fn empty() -> GearPiece {
        GearPiece { base_item_level: 0.0, material_purity: 0.0, resonance_depth: 0.0, sway_marks: 0, enchantments: 0 }
    }
}

/// The Resonance contribution of a single item:
/// `level × purity × (1 + resonance_depth)` + sway-mark and enchantment bonuses.
pub fn item_resonance(p: &GearPiece) -> f32 {
    let purity = p.material_purity.clamp(0.0, 1.0);
    let depth = p.resonance_depth.max(0.0);
    let base = p.base_item_level.max(0.0) * purity * (1.0 + depth);
    base + p.sway_marks as f32 * SWAY_MARK_BONUS + p.enchantments as f32 * ENCHANTMENT_BONUS
}

/// The panel's **POWER** number: the summed, rounded Resonance of all equipped
/// items.
pub fn resonance_rating(gear: &[GearPiece]) -> u32 {
    gear.iter().map(item_resonance).sum::<f32>().round().max(0.0) as u32
}

/// Physical damage reduction from a Resilience score (0–10), anchored to the
/// doc's example (resilience 8 ⇒ 18% less physical damage) as a linear map:
/// `reduction = resilience × 0.0225`, so 10 ⇒ 22.5%. Clamped to `[0, 1)`.
pub fn resilience_damage_reduction(resilience: f32) -> f32 {
    let r = resilience.clamp(0.0, MAX_RESILIENCE);
    (r * 0.0225).clamp(0.0, 0.9)
}

/// Apply `resilience`'s mitigation to an incoming physical `damage`.
pub fn mitigate(damage: f32, resilience: f32) -> f32 {
    damage.max(0.0) * (1.0 - resilience_damage_reduction(resilience))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(level: f32, purity: f32, depth: f32, sway: u8, ench: u8) -> GearPiece {
        GearPiece { base_item_level: level, material_purity: purity, resonance_depth: depth, sway_marks: sway, enchantments: ench }
    }

    #[test]
    fn item_resonance_follows_the_formula() {
        // 100 × 0.8 × (1 + 0.5) = 120, + 2 sway (24) + 1 ench (25) = 169
        let p = piece(100.0, 0.8, 0.5, 2, 1);
        let expected = 100.0 * 0.8 * 1.5 + 2.0 * SWAY_MARK_BONUS + 1.0 * ENCHANTMENT_BONUS;
        assert!((item_resonance(&p) - expected).abs() < 1e-4);
        assert!((expected - 169.0).abs() < 1e-4);
    }

    #[test]
    fn purity_and_depth_and_bonuses_each_raise_the_score() {
        let base = piece(100.0, 0.5, 0.0, 0, 0);
        assert!(item_resonance(&piece(100.0, 1.0, 0.0, 0, 0)) > item_resonance(&base), "more purity ⇒ more power");
        assert!(item_resonance(&piece(100.0, 0.5, 0.5, 0, 0)) > item_resonance(&base), "more resonance depth ⇒ more");
        assert!(item_resonance(&piece(100.0, 0.5, 0.0, 3, 0)) > item_resonance(&base), "sway marks add");
        assert!(item_resonance(&piece(100.0, 0.5, 0.0, 0, 2)) > item_resonance(&base), "enchantments add");
    }

    #[test]
    fn power_sums_the_loadout() {
        let gear = [
            piece(120.0, 1.0, 0.0, 0, 0),
            piece(80.0, 1.0, 0.0, 0, 0),
            piece(200.0, 1.0, 0.0, 0, 0),
        ];
        assert_eq!(resonance_rating(&gear), 400);
    }

    #[test]
    fn empty_slots_and_empty_loadout_score_zero() {
        assert_eq!(item_resonance(&GearPiece::empty()), 0.0);
        assert_eq!(resonance_rating(&[]), 0);
        assert_eq!(resonance_rating(&[GearPiece::empty(), GearPiece::empty()]), 0);
    }

    #[test]
    fn a_higher_tier_slot_outscores_a_lower_one() {
        // mirrors the doc: an Epic head (level ~106) beats a common one (~41).
        assert!(item_resonance(&piece(106.0, 1.0, 0.1, 1, 0)) > item_resonance(&piece(41.0, 1.0, 0.0, 0, 0)));
    }

    #[test]
    fn resilience_maps_to_the_documented_reduction() {
        assert!((resilience_damage_reduction(8.0) - 0.18).abs() < 1e-4, "doc anchor: 8 ⇒ 18%");
        assert_eq!(resilience_damage_reduction(0.0), 0.0);
        assert!((resilience_damage_reduction(10.0) - 0.225).abs() < 1e-4);
        assert!(resilience_damage_reduction(999.0) <= 0.9, "clamped");
        assert_eq!(resilience_damage_reduction(-5.0), 0.0, "clamped up");
    }

    #[test]
    fn mitigate_reduces_incoming_damage() {
        // 100 damage at resilience 8 ⇒ 18% off ⇒ 82
        assert!((mitigate(100.0, 8.0) - 82.0).abs() < 1e-3);
        assert_eq!(mitigate(50.0, 0.0), 50.0, "no resilience ⇒ full damage");
    }

    #[test]
    fn scoring_is_deterministic() {
        let g = [piece(100.0, 0.7, 0.3, 2, 1)];
        assert_eq!(resonance_rating(&g), resonance_rating(&g));
        assert_eq!(resilience_damage_reduction(6.5), resilience_damage_reduction(6.5));
    }
}
