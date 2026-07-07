//! turtle — a giant turtle's slow behavior policy. Three states: **Wander** (amble
//! gently), **Bask** (sit still when warm and unthreatened), and **Retract** (pull
//! into the shell when threatened — motion stops). A pure, deterministic state
//! transition + the speed multiplier each state allows. OPTIONAL task module;
//! property-testable (deterministic, bounded), not visual. Pure, std-only.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurtleState {
    Wander,
    Bask,
    Retract,
}

/// Thresholds for the policy.
#[derive(Clone, Copy, Debug)]
pub struct TurtleParams {
    /// Threat level (0..1) at/above which the turtle retracts.
    pub threat_retract: f32,
    /// Warmth (0..1) at/above which a safe turtle basks instead of wandering.
    pub warmth_bask: f32,
    /// Top speed multiplier while wandering (0..1 of base creature speed).
    pub wander_speed: f32,
}

impl Default for TurtleParams {
    fn default() -> TurtleParams {
        TurtleParams { threat_retract: 0.5, warmth_bask: 0.6, wander_speed: 0.25 }
    }
}

/// Decide the next state from the current one and the senses. Hysteresis: once
/// retracted, the turtle only re-emerges when threat falls well below the trigger
/// (`0.6 ×`), so it doesn't flicker in and out at the threshold.
pub fn next_state(state: TurtleState, threat: f32, warmth: f32, p: &TurtleParams) -> TurtleState {
    let threat = threat.clamp(0.0, 1.0);
    let warmth = warmth.clamp(0.0, 1.0);
    if state == TurtleState::Retract {
        // stay tucked until clearly safe
        if threat > p.threat_retract * 0.6 {
            return TurtleState::Retract;
        }
    } else if threat >= p.threat_retract {
        return TurtleState::Retract;
    }
    // safe: bask if warm, else amble
    if warmth >= p.warmth_bask {
        TurtleState::Bask
    } else {
        TurtleState::Wander
    }
}

/// The speed multiplier a state allows (0 = stationary).
pub fn speed_mult(state: TurtleState, p: &TurtleParams) -> f32 {
    match state {
        TurtleState::Wander => p.wander_speed.clamp(0.0, 1.0),
        TurtleState::Bask => 0.0,
        TurtleState::Retract => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> f32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) as f32) / ((1u32 << 24) as f32)
    }

    #[test]
    fn high_threat_retracts_and_stops() {
        let p = TurtleParams::default();
        let s = next_state(TurtleState::Wander, 0.9, 0.5, &p);
        assert_eq!(s, TurtleState::Retract);
        assert_eq!(speed_mult(s, &p), 0.0, "retracted turtle does not move");
    }

    #[test]
    fn warm_and_safe_basks_cool_and_safe_wanders() {
        let p = TurtleParams::default();
        assert_eq!(next_state(TurtleState::Wander, 0.0, 0.9, &p), TurtleState::Bask);
        assert_eq!(next_state(TurtleState::Bask, 0.0, 0.1, &p), TurtleState::Wander);
    }

    #[test]
    fn retract_has_hysteresis() {
        let p = TurtleParams::default();
        // just below the trigger but above 0.6× → stays retracted
        let mid = p.threat_retract * 0.8;
        assert_eq!(next_state(TurtleState::Retract, mid, 0.9, &p), TurtleState::Retract);
        // well below → re-emerges
        assert_ne!(next_state(TurtleState::Retract, 0.1, 0.9, &p), TurtleState::Retract);
    }

    #[test]
    fn prop_deterministic_and_bounded() {
        let p = TurtleParams::default();
        let states = [TurtleState::Wander, TurtleState::Bask, TurtleState::Retract];
        let mut seed = 0x7012_7012u32;
        for _ in 0..5000 {
            let st = states[(lcg(&mut seed) * 3.0) as usize % 3];
            let threat = lcg(&mut seed) * 2.0 - 0.5;
            let warmth = lcg(&mut seed) * 2.0 - 0.5;
            let a = next_state(st, threat, warmth, &p);
            let b = next_state(st, threat, warmth, &p);
            assert_eq!(a, b, "deterministic");
            let m = speed_mult(a, &p);
            assert!(m >= 0.0 && m <= 1.0 && m.is_finite(), "speed mult bounded: {m}");
        }
    }
}
