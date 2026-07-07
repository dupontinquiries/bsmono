//! `sim::combat_stance` — unified combat stance compositor.
//!
//! Composes three existing pure cores into a single [`Stance`] decision:
//! - [`stance_bias::bias`]          — aggressive↔defensive lean from overall HP.
//! - [`batch_guard_side::present_one`] — which side to present to the threat.
//! - [`engage_distance::standoff`]  — preferred standoff distance.
//!
//! This module contains NO duplicated math from those cores; it only feeds
//! their inputs and assembles their outputs into one struct. See
//! `handoff/COMBAT_STANCE.md` for the design rationale.
//!
//! ## Input
//! All fields of [`CreatureState`] are plain scalars — no render, no GPU, no IO.
//! HP values are raw (not fractions); distances are in world units.
//!
//! ## Output [`Stance`]
//! - `lean`         — `[-1, +1]`: −1 = fully defensive, +1 = fully aggressive.
//! - `present_side` — `{-1, 0, +1}`: −1 = show left, +1 = show right, 0 = square.
//! - `standoff`     — preferred distance to threat (≥ melee).

use crate::sim::stance_bias;
use crate::sim::batch_guard_side;
use crate::sim::engage_distance;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Snapshot of a creature's combat-relevant state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CreatureState {
    /// Total current HP across all parts.
    pub overall_hp: f32,
    /// Max total HP (must be > 0; clamped internally).
    pub max_hp: f32,
    /// Current HP of the left-side limbs (arm/leg/wing).
    pub left_hp: f32,
    /// Current HP of the right-side limbs.
    pub right_hp: f32,
    /// Fraction of head/torso exposed to the threat `[0, 1]`: 0 = fully covered,
    /// 1 = vitals fully forward.
    pub vital_exposure: f32,
    /// Normalised threat level from `threat_assess` `[0, 1]`.
    pub threat: f32,
    /// Preferred engagement distance at neutral threat/exposure (world units, > 0).
    pub base_distance: f32,
    /// Minimum (melee) distance — standoff is never below this.
    pub melee_distance: f32,
}

/// The unified stance decision for one creature this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Stance {
    /// Aggressive↔defensive lean in `[-1, +1]`.
    /// −1 = fully defensive (hurt, side-on, back off).
    /// +1 = fully aggressive (healthy, square, close in).
    pub lean: f32,
    /// Which side to present to the threat: −1 = left, 0 = square, +1 = right.
    pub present_side: i8,
    /// Preferred distance to threat in world units (≥ melee_distance).
    pub standoff: f32,
}

// ---------------------------------------------------------------------------
// Hysteresis constant (avoids side-jitter when limbs are close in HP).
// Tunable; this value is ≈5 % of a typical 100-HP limb budget.
// ---------------------------------------------------------------------------
const SIDE_HYSTERESIS: f32 = 5.0;

// ---------------------------------------------------------------------------
// Core function
// ---------------------------------------------------------------------------

/// Compute a unified [`Stance`] for a creature by composing the three cores.
///
/// - Out-of-range inputs are handled by each sub-core (they each clamp).
/// - No allocation, no RNG, no IO. Deterministic: same state → same stance.
pub fn combat_stance(s: CreatureState) -> Stance {
    // 1. Aggressive↔defensive lean from overall HP.
    let lean = stance_bias::bias(s.overall_hp, s.max_hp);

    // 2. Which side to present: healthier side faces the threat.
    //    `lean` modulates hysteresis: defensive (lean < 0) → smaller hysteresis
    //    (commit more strongly to guarding the weaker side); aggressive → wider
    //    band (stay square more often when healthy).
    let effective_hyst = SIDE_HYSTERESIS * (1.0 + lean).max(0.0);
    let present_side = batch_guard_side::present_one(s.left_hp, s.right_hp, effective_hyst);

    // 3. Standoff distance, scaled by lean: defensive creatures back off further.
    //    lean in [-1,+1] → scale in [0.5, 1.5]: hurt = up to 1.5× base, healthy = 0.5×.
    let distance_scale = 1.0 - lean * 0.5;   // lean=+1 → 0.5, lean=-1 → 1.5
    let scaled_base = s.base_distance * distance_scale;
    let standoff = engage_distance::standoff(
        s.vital_exposure,
        s.threat,
        scaled_base,
        s.melee_distance,
    );

    Stance { lean, present_side, standoff }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn full_hp() -> CreatureState {
        CreatureState {
            overall_hp: 100.0, max_hp: 100.0,
            left_hp: 50.0, right_hp: 50.0,
            vital_exposure: 0.1, threat: 0.2,
            base_distance: 10.0, melee_distance: 2.0,
        }
    }

    fn hurt() -> CreatureState {
        CreatureState {
            overall_hp: 15.0, max_hp: 100.0,
            left_hp: 5.0, right_hp: 30.0,
            vital_exposure: 0.8, threat: 0.9,
            base_distance: 10.0, melee_distance: 2.0,
        }
    }

    #[test]
    fn full_hp_is_aggressive() {
        let st = combat_stance(full_hp());
        assert!(st.lean > 0.5, "full-HP lean should be aggressive: {}", st.lean);
    }

    #[test]
    fn hurt_high_threat_is_defensive() {
        let st = combat_stance(hurt());
        assert!(st.lean < -0.5, "hurt lean should be defensive: {}", st.lean);
    }

    #[test]
    fn hurt_high_threat_larger_standoff() {
        let st_full = combat_stance(full_hp());
        let st_hurt = combat_stance(hurt());
        assert!(st_hurt.standoff > st_full.standoff,
            "hurt standoff {} should exceed full-hp standoff {}", st_hurt.standoff, st_full.standoff);
    }

    #[test]
    fn full_hp_even_sides_is_square() {
        // Both sides equal HP and full health (wide hysteresis) → square
        let st = combat_stance(full_hp());
        assert_eq!(st.present_side, 0, "equal sides at full HP should be square");
    }

    #[test]
    fn present_side_follows_healthier_side() {
        // right_hp (30) >> left_hp (5) → present right side (+1)
        let st = combat_stance(hurt());
        assert_eq!(st.present_side, 1,
            "should present right (healthier) side, got {}", st.present_side);
    }

    #[test]
    fn present_side_left_when_left_healthier() {
        let s = CreatureState {
            left_hp: 80.0, right_hp: 10.0,
            overall_hp: 50.0, max_hp: 100.0,
            vital_exposure: 0.5, threat: 0.5,
            base_distance: 10.0, melee_distance: 2.0,
        };
        let st = combat_stance(s);
        assert_eq!(st.present_side, -1, "left healthier → present left (-1), got {}", st.present_side);
    }

    #[test]
    fn standoff_never_below_melee() {
        for melee in [0.0f32, 1.0, 5.0, 20.0] {
            let s = CreatureState { melee_distance: melee, ..full_hp() };
            let st = combat_stance(s);
            assert!(st.standoff >= melee - 1e-5,
                "standoff {} below melee {} ", st.standoff, melee);
        }
    }

    #[test]
    fn lean_in_range() {
        for s in [full_hp(), hurt()] {
            let st = combat_stance(s);
            assert!(st.lean >= -1.0 && st.lean <= 1.0,
                "lean {} out of [-1,1]", st.lean);
        }
    }

    #[test]
    fn present_side_in_range() {
        for s in [full_hp(), hurt()] {
            let st = combat_stance(s);
            assert!((-1i8..=1).contains(&st.present_side),
                "present_side {} out of {{-1,0,1}}", st.present_side);
        }
    }

    #[test]
    fn deterministic() {
        let s = hurt();
        let a = combat_stance(s);
        let b = combat_stance(s);
        assert_eq!(a.lean.to_bits(),         b.lean.to_bits());
        assert_eq!(a.present_side,           b.present_side);
        assert_eq!(a.standoff.to_bits(),     b.standoff.to_bits());
    }

    #[test]
    fn standoff_finite_and_positive() {
        for s in [full_hp(), hurt()] {
            let st = combat_stance(s);
            assert!(st.standoff.is_finite(), "standoff not finite");
            assert!(st.standoff >= 0.0,      "standoff negative");
        }
    }

    #[test]
    fn higher_threat_increases_standoff() {
        let lo = CreatureState { threat: 0.1, ..hurt() };
        let hi = CreatureState { threat: 0.9, ..hurt() };
        assert!(combat_stance(hi).standoff >= combat_stance(lo).standoff,
            "higher threat should not decrease standoff");
    }
}
