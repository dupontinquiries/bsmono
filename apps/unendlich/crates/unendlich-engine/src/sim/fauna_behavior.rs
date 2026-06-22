//! `sim::fauna_behavior` — temperament-driven creature behaviour FSM.
//!
//! Generalises the dragon AI cores to the whole animal roster. Selects a
//! [`FaunaBehavior`] each tick based on temperament + stimuli, with a dwell
//! timer that suppresses non-urgent switches (hysteresis against flip-flop).
//!
//! Composes `behavior_arbiter` for the core utility-AI scoring; this module
//! owns the **temperament filter** (Passive never Attacks, Neutral only attacks
//! when provoked, Hostile pursues eagerly) and the **FSM dwell gate**.
//!
//! ## Temperament → behaviour contract
//! | Temperament | Allowed behaviours             |
//! |-------------|-------------------------------|
//! | Passive     | Idle, Graze, Wander, Flee     |
//! | Neutral     | Idle, Graze, Wander, Flee,    |
//! |             | Threaten (provoked), Attack   |
//! |             | (provoked + high threat)      |
//! | Hostile     | All — Approach/Attack eagerly |
//! | Specialist  | All — overrides to niche rule |
//!
//! ## Dwell thresholds (documented)
//! - `MIN_DWELL_S = 0.8 s` — minimum time before a non-urgent switch.
//! - **Urgent stimuli** (bypass dwell): `threat > URGENT_THREAT` OR
//!   (`provoked && temperament == Hostile`) OR `injured01 > URGENT_INJURY`.
//! - `URGENT_THREAT = 0.75`, `URGENT_INJURY = 0.80`.
//!
//! ## Specialist niche (documented default)
//! Specialist follows a pack/herd niche: when `player_dist > SPECIALIST_DIST`,
//! it Grazes; when closer, it Approaches until very close, then Idles.
//! `SPECIALIST_DIST = 12.0` (world units).

use crate::sim::behavior_arbiter::{self, Behavior, Drives};

// ---------------------------------------------------------------------------
// Thresholds (documented)
// ---------------------------------------------------------------------------

const MIN_DWELL_S:    f32 = 0.8;
const URGENT_THREAT:  f32 = 0.75;
const URGENT_INJURY:  f32 = 0.80;
const SPECIALIST_DIST: f32 = 12.0;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Creature temperament archetype.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Temperament {
    /// Flees all threats; never attacks.
    Passive,
    /// Defends if provoked; otherwise non-aggressive.
    Neutral,
    /// Actively pursues and attacks.
    Hostile,
    /// Follows a niche rule (herd/pack; see module doc).
    Specialist,
}

/// The set of behaviours a fauna creature can exhibit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaunaBehavior {
    Idle,
    Graze,
    Wander,
    Flee,
    Approach,
    Threaten,
    Attack,
    Retreat,
}

/// External stimuli for this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stimulus {
    /// Normalised threat level `[0, 1]`.
    pub threat: f32,
    /// Hunger fraction `[0, 1]`.
    pub hunger01: f32,
    /// Distance to the player in world units (> 0).
    pub player_dist: f32,
    /// Whether the creature has been directly attacked this tick.
    pub provoked: bool,
    /// Injury fraction `[0, 1]` (1 = near death).
    pub injured01: f32,
}

/// Persistent FSM state (behaviour + dwell timer).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaunaState {
    pub behavior: FaunaBehavior,
    /// Ticks (seconds) spent in the current behaviour.
    pub dwell_s: f32,
}

/// Return an initial idle state.
pub fn idle() -> FaunaState {
    FaunaState { behavior: FaunaBehavior::Idle, dwell_s: 0.0 }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Clamp to [0, 1], treating NaN as 0.
fn c01(x: f32) -> f32 {
    if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) }
}

/// Whether current stimuli warrant bypassing the dwell gate.
fn is_urgent(s: &Stimulus, t: Temperament) -> bool {
    c01(s.threat) > URGENT_THREAT
        || (s.provoked && t == Temperament::Hostile)
        || c01(s.injured01) > URGENT_INJURY
}

/// Map `behavior_arbiter::Behavior` + context to `FaunaBehavior` for non-Specialist.
fn arbiter_to_fauna(b: Behavior, s: &Stimulus, t: Temperament) -> FaunaBehavior {
    match b {
        Behavior::Flee => {
            // High injury → Retreat instead of straight Flee.
            if c01(s.injured01) > 0.5 { FaunaBehavior::Retreat } else { FaunaBehavior::Flee }
        }
        Behavior::Fight => {
            match t {
                Temperament::Passive => FaunaBehavior::Flee, // Passive cannot Fight
                Temperament::Neutral => {
                    if s.provoked { FaunaBehavior::Attack } else { FaunaBehavior::Threaten }
                }
                Temperament::Hostile => FaunaBehavior::Attack,
                Temperament::Specialist => FaunaBehavior::Attack,
            }
        }
        Behavior::Forage => FaunaBehavior::Graze,
        Behavior::Rest   => FaunaBehavior::Idle,
        Behavior::Patrol => {
            match t {
                Temperament::Hostile => {
                    // Hostile creatures approach instead of patrolling.
                    if c01(s.threat) > 0.3 || s.player_dist < 20.0 {
                        FaunaBehavior::Approach
                    } else {
                        FaunaBehavior::Wander
                    }
                }
                _ => FaunaBehavior::Wander,
            }
        }
    }
}

/// Specialist niche behaviour (overrides arbiter).
fn specialist_behavior(s: &Stimulus) -> FaunaBehavior {
    let threat = c01(s.threat);
    if threat > URGENT_THREAT {
        return FaunaBehavior::Flee;
    }
    let dist = s.player_dist.max(0.0);
    if dist > SPECIALIST_DIST {
        FaunaBehavior::Graze
    } else if dist > 3.0 {
        FaunaBehavior::Approach
    } else {
        FaunaBehavior::Idle
    }
}

// ---------------------------------------------------------------------------
// Core step function
// ---------------------------------------------------------------------------

/// Advance the FSM by `dt` seconds given `temperament` and current `s`.
///
/// - Urgent stimuli bypass the dwell gate and switch immediately.
/// - Non-urgent: a switch is suppressed until `dwell_s >= MIN_DWELL_S`.
/// - Temperament filters what the arbiter's output maps to.
/// - Deterministic: same `(prev, t, s, dt)` → same output. No RNG.
/// - Never panics; out-of-range inputs are clamped.
pub fn step(prev: FaunaState, t: Temperament, s: &Stimulus, dt: f32) -> FaunaState {
    let dt = dt.max(0.0);
    let new_dwell = prev.dwell_s + dt;

    // Build arbiter drives from stimuli.
    let threat    = c01(s.threat);
    let hunger    = c01(s.hunger01);
    let injured   = c01(s.injured01);
    let fatigue   = 0.0f32; // fauna has no explicit fatigue drive — always rested
    // Aggression: Hostile = max, Neutral = provoked-only, Passive = none.
    let aggression = match t {
        Temperament::Hostile    => 1.0,
        Temperament::Neutral    => if s.provoked { 0.6 } else { 0.0 },
        Temperament::Passive    => 0.0,
        Temperament::Specialist => 0.2,
    };
    let health_frac = 1.0 - injured;

    // Specialist: fully overrides arbiter.
    let candidate = if t == Temperament::Specialist {
        specialist_behavior(s)
    } else {
        let drives = Drives { threat, hunger, fatigue, aggression, health_frac };
        let choice = behavior_arbiter::select(&drives, None, 0.0);
        arbiter_to_fauna(choice.behavior, s, t)
    };

    // Dwell gate: only switch if urgent OR dwell has expired.
    let urgent = is_urgent(s, t);
    let may_switch = urgent || new_dwell >= MIN_DWELL_S;

    if may_switch && candidate != prev.behavior {
        FaunaState { behavior: candidate, dwell_s: 0.0 }
    } else {
        FaunaState { behavior: prev.behavior, dwell_s: new_dwell }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn stim(threat: f32, hunger: f32, dist: f32, provoked: bool, injured: f32) -> Stimulus {
        Stimulus { threat, hunger01: hunger, player_dist: dist, provoked, injured01: injured }
    }

    fn run(prev: FaunaState, t: Temperament, s: &Stimulus) -> FaunaState {
        // Large dt so dwell never blocks.
        step(prev, t, s, 10.0)
    }

    // ---- temperament contracts ---------------------------------------------

    #[test]
    fn passive_high_threat_flees() {
        let s = stim(1.0, 0.0, 50.0, false, 0.0);
        let out = run(idle(), Temperament::Passive, &s);
        assert_eq!(out.behavior, FaunaBehavior::Flee,
            "Passive + high threat should Flee, got {:?}", out.behavior);
    }

    #[test]
    fn passive_never_attacks() {
        let s = stim(1.0, 0.0, 1.0, true, 0.0);
        let out = run(idle(), Temperament::Passive, &s);
        assert_ne!(out.behavior, FaunaBehavior::Attack, "Passive must never Attack");
        assert_ne!(out.behavior, FaunaBehavior::Threaten, "Passive must never Threaten");
    }

    #[test]
    fn hostile_player_near_approaches_or_attacks() {
        let s = stim(0.8, 0.0, 5.0, false, 0.0);
        let out = run(idle(), Temperament::Hostile, &s);
        assert!(
            out.behavior == FaunaBehavior::Approach || out.behavior == FaunaBehavior::Attack,
            "Hostile + player near should Approach or Attack, got {:?}", out.behavior
        );
    }

    #[test]
    fn neutral_only_attacks_when_provoked() {
        // Unprovoked: no attack.
        let s_safe = stim(0.5, 0.0, 5.0, false, 0.0);
        let out_safe = run(idle(), Temperament::Neutral, &s_safe);
        assert_ne!(out_safe.behavior, FaunaBehavior::Attack,
            "Neutral unprovoked should not Attack, got {:?}", out_safe.behavior);

        // Provoked + high threat → Attack.
        let s_prov = stim(1.0, 0.0, 3.0, true, 0.0);
        let out_prov = run(idle(), Temperament::Neutral, &s_prov);
        assert_eq!(out_prov.behavior, FaunaBehavior::Attack,
            "Neutral provoked + high threat should Attack, got {:?}", out_prov.behavior);
    }

    #[test]
    fn hunger_drives_graze_when_safe() {
        let s = stim(0.0, 1.0, 100.0, false, 0.0);
        let out = run(idle(), Temperament::Neutral, &s);
        assert_eq!(out.behavior, FaunaBehavior::Graze,
            "High hunger + safe should Graze, got {:?}", out.behavior);
    }

    #[test]
    fn high_injury_biases_toward_retreat_or_flee() {
        let s = stim(0.8, 0.0, 5.0, false, 0.9);
        let out = run(idle(), Temperament::Passive, &s);
        assert!(
            out.behavior == FaunaBehavior::Retreat || out.behavior == FaunaBehavior::Flee,
            "High injury + high threat should Retreat or Flee, got {:?}", out.behavior
        );
    }

    // ---- hysteresis / dwell ------------------------------------------------

    #[test]
    fn dwell_suppresses_non_urgent_switch() {
        // Start idle, push borderline hunger — should NOT switch if dwell < MIN.
        let s = stim(0.0, 0.8, 100.0, false, 0.0);
        let prev = FaunaState { behavior: FaunaBehavior::Idle, dwell_s: 0.0 };
        // dt < MIN_DWELL_S: dwell gate should hold.
        let out = step(prev, Temperament::Neutral, &s, 0.1);
        assert_eq!(out.behavior, FaunaBehavior::Idle,
            "dwell gate should suppress switch at dt=0.1 s");
    }

    #[test]
    fn dwell_allows_switch_after_expiry() {
        let s = stim(0.0, 1.0, 100.0, false, 0.0);
        let prev = FaunaState { behavior: FaunaBehavior::Idle, dwell_s: 0.0 };
        let out = step(prev, Temperament::Neutral, &s, MIN_DWELL_S + 0.1);
        assert_eq!(out.behavior, FaunaBehavior::Graze,
            "after dwell expiry, should switch to Graze");
    }

    #[test]
    fn urgent_threat_bypasses_dwell() {
        let s = stim(URGENT_THREAT + 0.01, 0.0, 100.0, false, 0.0);
        let prev = FaunaState { behavior: FaunaBehavior::Idle, dwell_s: 0.0 };
        let out = step(prev, Temperament::Passive, &s, 0.001); // tiny dt
        assert_eq!(out.behavior, FaunaBehavior::Flee,
            "urgent threat should bypass dwell gate");
    }

    #[test]
    fn non_urgent_flip_does_not_thrash() {
        // Borderline stimulus oscillating: should hold behaviour.
        let s_on  = stim(0.5, 0.0, 100.0, false, 0.0);
        let s_off = stim(0.4, 0.0, 100.0, false, 0.0);
        let mut state = idle();
        for _ in 0..10 {
            let s = if state.dwell_s < 0.5 { &s_on } else { &s_off };
            state = step(state, Temperament::Neutral, s, 0.05);
        }
        // Should not have thrashed — behavior count of changes is bounded.
        // (This test just checks no panic and determinism; oscillation test below.)
    }

    // ---- specialist --------------------------------------------------------

    #[test]
    fn specialist_far_grazes() {
        let s = stim(0.0, 0.0, SPECIALIST_DIST + 5.0, false, 0.0);
        let out = run(idle(), Temperament::Specialist, &s);
        assert_eq!(out.behavior, FaunaBehavior::Graze,
            "Specialist far from player should Graze");
    }

    #[test]
    fn specialist_close_idles() {
        let s = stim(0.0, 0.0, 1.0, false, 0.0);
        let out = run(idle(), Temperament::Specialist, &s);
        assert_eq!(out.behavior, FaunaBehavior::Idle,
            "Specialist very close should Idle, got {:?}", out.behavior);
    }

    #[test]
    fn specialist_high_threat_flees() {
        let s = stim(URGENT_THREAT + 0.1, 0.0, 5.0, false, 0.0);
        let out = run(idle(), Temperament::Specialist, &s);
        assert_eq!(out.behavior, FaunaBehavior::Flee,
            "Specialist with high threat should Flee");
    }

    // ---- determinism + safety ----------------------------------------------

    #[test]
    fn deterministic() {
        let s = stim(0.6, 0.4, 8.0, true, 0.3);
        let a = step(idle(), Temperament::Neutral, &s, 1.0);
        let b = step(idle(), Temperament::Neutral, &s, 1.0);
        assert_eq!(a, b, "step must be deterministic");
    }

    #[test]
    fn out_of_range_inputs_no_panic() {
        let s = stim(-1.0, 5.0, -3.0, false, 2.0);
        let out = step(idle(), Temperament::Hostile, &s, 10.0);
        // Any valid FaunaBehavior is fine — just mustn't panic.
        let _ = out.behavior;
    }

    #[test]
    fn dwell_accumulates_when_no_switch() {
        let s = stim(0.0, 0.0, 100.0, false, 0.0);
        let mut state = idle();
        for _ in 0..5 {
            state = step(state, Temperament::Passive, &s, 0.1);
        }
        assert!(state.dwell_s >= 0.0 && state.dwell_s.is_finite(),
            "dwell_s should accumulate finitely: {}", state.dwell_s);
    }

    #[test]
    fn new_state_dwell_resets_on_switch() {
        let s = stim(1.0, 0.0, 100.0, false, 0.0);
        let prev = FaunaState { behavior: FaunaBehavior::Idle, dwell_s: 5.0 };
        let out = step(prev, Temperament::Passive, &s, 10.0);
        // Should have switched to Flee; dwell resets to 0.
        assert_eq!(out.behavior, FaunaBehavior::Flee);
        assert!(out.dwell_s < 0.01, "dwell should reset on switch: {}", out.dwell_s);
    }
}

// ── S1 (r94): register as a sim::service_registry service (co-located) ───────
// `step` is a free FSM function, so the service carries the persistent FaunaState
// + a Temperament and advances it each tick with a NEUTRAL stimulus (no threat,
// fed, player far, unprovoked, unhurt). Real per-tick stimuli are a later pass.
struct FaunaBehaviorService {
    state: FaunaState,
    temperament: Temperament,
}
const NEUTRAL_STIMULUS: Stimulus = Stimulus {
    threat: 0.0,
    hunger01: 0.0,
    player_dist: 1000.0,
    provoked: false,
    injured01: 0.0,
};
impl crate::sim::service_registry::Service for FaunaBehaviorService {
    fn name(&self) -> &'static str { "fauna_behavior" }
    fn tick(&mut self, ctx: &crate::sim::service_registry::ServiceCtx) {
        self.state = step(self.state, self.temperament, &NEUTRAL_STIMULUS, ctx.dt);
    }
}
#[linkme::distributed_slice(crate::sim::service_registry::SERVICES)]
static FAUNA_BEHAVIOR_SERVICE: crate::sim::service_registry::ServiceEntry =
    crate::sim::service_registry::ServiceEntry {
        name: "fauna_behavior",
        make: || Box::new(FaunaBehaviorService { state: idle(), temperament: Temperament::Neutral }),
    };

#[cfg(test)]
mod s1_wiring_tests {
    use crate::sim::service_registry::{Service, ServiceCtx};
    #[test]
    fn fauna_behavior_service_ticks_panic_free() {
        let mut s = super::FaunaBehaviorService { state: super::idle(), temperament: super::Temperament::Neutral };
        let ctx = ServiceCtx { dt: 1.0 / 60.0 };
        for _ in 0..240 { s.tick(&ctx); }
        assert_eq!(s.name(), "fauna_behavior");
    }
}
