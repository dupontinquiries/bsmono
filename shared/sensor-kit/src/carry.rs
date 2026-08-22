//! carry — where on the body the phone is, as a state machine with explicit exits.
//!
//! Each state has an **entry guard** and a separate **exit guard**, deliberately asymmetric
//! (hysteresis): you enter `ArmSwing` above 3 m/s² of dynamic RMS but only leave below 2, so the
//! trough between two strides doesn't flap the state. A transition additionally needs the exit to
//! fire *and* one candidate's entry to hold for `dwell` consecutive windows — with a ~2 s window
//! and 0.5 s hop, a switch costs ~1.5 s of consistent evidence.
//!
//! The exits are not just debouncing: they are the loggable boundary events. A gait log is a
//! sequence of `(t, from → to)` transitions, and the *exit* timestamp is when a carry segment's
//! step-count confidence stops applying — which is exactly what downstream correction needs
//! ("the 14:02–14:19 segment was Pocket; retune, recount").
//!
//! States and their signatures (device axes: x right, y up along the long edge, z out of screen):
//!
//! * `Still`      — no motion energy. Table, cradle, or a pocket while seated.
//! * `HandHeld`   — the elbow-bent reading pose: gravity mostly +z (screen up-ish), modest
//!                  energy, no arc.
//! * `ArmSwing`   — extended arm, swinging at stride rate: high energy AND the gravity direction
//!                  sweeps a wide arc each stride. The arc is what a pocket never shows.
//! * `PocketWaist`— trouser/waist pocket: device near-vertical (gravity ±y-dominant), walking
//!                  cadence, moderate-to-high energy, small arc.
//! * `StowedBack` — bag/backpack near the spine: cadence present but *damped* (the strap and
//!                  padding eat the impact), gravity direction stable and arbitrary.
//! * `Unknown`    — exit fired, no candidate stabilised. Honest state, not an error.
//!
//! Honesty note: `PocketWaist` vs `StowedBack` is the weak boundary. Both are torso-coupled;
//! the discriminators here (y-dominance, energy band) are priors, not physics. Real thresholds
//! want tuning against labelled walks on the target devices — the guards are fields, not
//! constants, for exactly that.

use crate::gait::Features;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CarryState {
    Still,
    HandHeld,
    ArmSwing,
    PocketWaist,
    StowedBack,
    Unknown,
}

impl CarryState {
    pub fn as_str(self) -> &'static str {
        match self {
            CarryState::Still => "still",
            CarryState::HandHeld => "hand-held",
            CarryState::ArmSwing => "arm-swing",
            CarryState::PocketWaist => "pocket-waist",
            CarryState::StowedBack => "stowed-back",
            CarryState::Unknown => "unknown",
        }
    }
    /// Compact stable encoding for the routine learner's Markov alphabet.
    pub fn code(self) -> u8 {
        match self {
            CarryState::Still => 0,
            CarryState::HandHeld => 1,
            CarryState::ArmSwing => 2,
            CarryState::PocketWaist => 3,
            CarryState::StowedBack => 4,
            CarryState::Unknown => 5,
        }
    }
    pub const COUNT: u8 = 6;
}

/// A completed transition: the exit and the entry, as one loggable edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CarryTransition {
    pub from: CarryState,
    pub to: CarryState,
}

/// All guard thresholds, tunable. Units: m/s² for energies, degrees for sway, fraction of |g|
/// for axis dominance.
#[derive(Debug, Clone, Copy)]
pub struct CarryGuards {
    pub still_enter_rms: f64,
    pub still_exit_rms: f64,
    pub hand_rms: (f64, f64),
    pub hand_z_frac: f64,
    pub hand_max_sway: f64,
    /// Above this dynamic RMS, a cadenced window can't be HandHeld (see the entry guard).
    pub hand_walk_max_rms: f64,
    pub swing_enter_rms: f64,
    pub swing_exit_rms: f64,
    pub swing_min_sway: f64,
    pub swing_exit_sway: f64,
    pub pocket_y_frac: f64,
    pub pocket_rms: (f64, f64),
    pub stowed_rms: (f64, f64),
    pub stowed_max_sway: f64,
}

impl Default for CarryGuards {
    fn default() -> Self {
        CarryGuards {
            still_enter_rms: 0.4,
            still_exit_rms: 0.7,
            hand_rms: (0.15, 2.5),
            hand_z_frac: 0.6,
            hand_max_sway: 20.0,
            hand_walk_max_rms: 1.0,
            swing_enter_rms: 3.0,
            swing_exit_rms: 2.0,
            swing_min_sway: 30.0,
            swing_exit_sway: 18.0,
            pocket_y_frac: 0.7,
            pocket_rms: (1.2, 9.0),
            stowed_rms: (0.6, 2.2),
            stowed_max_sway: 15.0,
        }
    }
}

impl CarryGuards {
    /// Entry guard: does this window look like `s` from a standing start?
    fn enters(&self, s: CarryState, f: &Features) -> bool {
        let gmag = f.gravity.mag().max(1e-6);
        match s {
            CarryState::Still => f.rms_dyn < self.still_enter_rms,
            CarryState::ArmSwing => {
                f.rms_dyn > self.swing_enter_rms
                    && f.sway_deg > self.swing_min_sway
                    && f.cadence_hz.is_some()
            }
            CarryState::PocketWaist => {
                f.gravity.y.abs() / gmag > self.pocket_y_frac
                    && f.cadence_hz.is_some()
                    && f.rms_dyn >= self.pocket_rms.0
                    && f.rms_dyn <= self.pocket_rms.1
                    && f.sway_deg < self.swing_min_sway
            }
            CarryState::HandHeld => {
                // A hand claims a *cadenced* window only when it is strongly damped (the
                // texting-while-walking case, rms ≲ 1): a bag riding at a z-tilt also shows
                // cadence in the hand energy band, and gravity axis alone can't separate them.
                f.gravity.z.abs() / gmag > self.hand_z_frac
                    && f.rms_dyn >= self.hand_rms.0
                    && f.rms_dyn <= self.hand_rms.1
                    && f.sway_deg < self.hand_max_sway
                    && (f.cadence_hz.is_none() || f.rms_dyn < self.hand_walk_max_rms)
            }
            CarryState::StowedBack => {
                f.cadence_hz.is_some()
                    && f.rms_dyn >= self.stowed_rms.0
                    && f.rms_dyn <= self.stowed_rms.1
                    && f.sway_deg < self.stowed_max_sway
                    && f.gravity.y.abs() / gmag <= self.pocket_y_frac
            }
            CarryState::Unknown => false, // Unknown is fallen into, never chosen
        }
    }

    /// Exit guard: has the evidence for staying in `s` collapsed? Thresholds offset from the
    /// entry guards (looser) so the boundary has width.
    fn exits(&self, s: CarryState, f: &Features) -> bool {
        let gmag = f.gravity.mag().max(1e-6);
        match s {
            CarryState::Still => f.rms_dyn > self.still_exit_rms,
            CarryState::ArmSwing => {
                f.rms_dyn < self.swing_exit_rms || f.sway_deg < self.swing_exit_sway
            }
            CarryState::PocketWaist => {
                f.gravity.y.abs() / gmag < self.pocket_y_frac - 0.2
                    || (f.cadence_hz.is_none() && f.rms_dyn < self.still_exit_rms)
                    || f.sway_deg > self.swing_min_sway
            }
            CarryState::HandHeld => {
                f.gravity.z.abs() / gmag < self.hand_z_frac - 0.2
                    || f.rms_dyn > self.hand_rms.1 + 1.0
                    || f.sway_deg > self.hand_max_sway + 10.0
            }
            CarryState::StowedBack => {
                f.rms_dyn > self.stowed_rms.1 + 1.0
                    || (f.cadence_hz.is_none() && f.rms_dyn < self.still_enter_rms)
                    || f.gravity.y.abs() / gmag > self.pocket_y_frac
            }
            CarryState::Unknown => true, // always willing to leave Unknown
        }
    }
}

/// The FSM. Feed it one [`Features`] per window; it returns `Some(CarryTransition)` on the window
/// that completes a switch.
#[derive(Debug, Clone)]
pub struct CarryFsm {
    state: CarryState,
    guards: CarryGuards,
    /// Consecutive windows a switch has to survive.
    dwell: u32,
    candidate: CarryState,
    candidate_streak: u32,
    exit_streak: u32,
}

/// Candidate priority when several entry guards fire at once: the more specific signature wins.
/// ArmSwing before Pocket (the arc is decisive), Pocket before Hand (axis dominance beats pose),
/// Stowed last among the moving states (its band nests inside the others).
const PRIORITY: [CarryState; 5] = [
    CarryState::Still,
    CarryState::ArmSwing,
    CarryState::PocketWaist,
    CarryState::HandHeld,
    CarryState::StowedBack,
];

impl CarryFsm {
    pub fn new(guards: CarryGuards, dwell: u32) -> CarryFsm {
        CarryFsm {
            state: CarryState::Unknown,
            guards,
            dwell: dwell.max(1),
            candidate: CarryState::Unknown,
            candidate_streak: 0,
            exit_streak: 0,
        }
    }

    pub fn state(&self) -> CarryState {
        self.state
    }

    pub fn step(&mut self, f: &Features) -> Option<CarryTransition> {
        // Raw candidate this window, by priority; Unknown when nothing matches.
        let raw = PRIORITY
            .iter()
            .copied()
            .find(|s| self.guards.enters(*s, f))
            .unwrap_or(CarryState::Unknown);

        if raw == self.candidate {
            self.candidate_streak += 1;
        } else {
            self.candidate = raw;
            self.candidate_streak = 1;
        }

        if self.guards.exits(self.state, f) {
            self.exit_streak += 1;
        } else {
            self.exit_streak = 0;
        }

        // A switch needs: our exit held for `dwell`, and either a stable different candidate
        // (→ that state) or a stable non-match (→ Unknown).
        if self.exit_streak >= self.dwell
            && self.candidate_streak >= self.dwell
            && self.candidate != self.state
        {
            let t = CarryTransition { from: self.state, to: self.candidate };
            self.state = self.candidate;
            self.exit_streak = 0;
            return Some(t);
        }
        None
    }
}

/// The payoff: step-detector presets per carry position. Numbers are priors sized from the
/// relative damping of each mount point; they want on-device tuning against labelled walks.
pub fn step_tuning_for(state: CarryState) -> crate::gait::StepTuning {
    use crate::gait::StepTuning;
    match state {
        // Damped through bag padding: arm low so soft impacts still count.
        CarryState::StowedBack => StepTuning { th_hi: 0.6, th_lo: 0.2, min_interval_s: 0.3 },
        // Direct hip coupling: crisp impacts, default-ish.
        CarryState::PocketWaist => StepTuning { th_hi: 1.2, th_lo: 0.4, min_interval_s: 0.25 },
        // Swing adds its own large oscillation on top of impacts: arm high or the swing itself
        // double-counts.
        CarryState::ArmSwing => StepTuning { th_hi: 2.5, th_lo: 0.9, min_interval_s: 0.3 },
        // Held hands absorb impact; slightly below default.
        CarryState::HandHeld => StepTuning { th_hi: 0.9, th_lo: 0.3, min_interval_s: 0.25 },
        CarryState::Still | CarryState::Unknown => crate::gait::StepTuning::default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gait::{Features, Sample};

    fn feats(gx: f64, gy: f64, gz: f64, rms: f64, sway: f64, cadence: Option<f64>) -> Features {
        Features {
            gravity: Sample { x: gx, y: gy, z: gz },
            rms_dyn: rms,
            sway_deg: sway,
            cadence_hz: cadence,
        }
    }

    fn still() -> Features {
        feats(0.0, 0.0, 9.8, 0.1, 2.0, None)
    }
    fn pocket_walk() -> Features {
        feats(0.5, 9.7, 1.0, 3.0, 6.0, Some(2.0))
    }
    fn arm_swing() -> Features {
        feats(3.0, 1.0, 9.0, 4.5, 45.0, Some(1.9))
    }
    fn hand_held() -> Features {
        feats(1.0, 2.0, 9.4, 0.6, 8.0, None)
    }
    fn stowed_walk() -> Features {
        feats(5.0, 4.0, 7.0, 1.2, 8.0, Some(1.8))
    }

    #[test]
    fn dwell_gates_a_transition_and_the_edge_carries_both_ends() {
        let mut fsm = CarryFsm::new(CarryGuards::default(), 3);
        // settle into Still
        for _ in 0..3 {
            fsm.step(&still());
        }
        assert_eq!(fsm.state(), CarryState::Still);
        // two windows of pocket-walk: not enough
        assert!(fsm.step(&pocket_walk()).is_none());
        assert!(fsm.step(&pocket_walk()).is_none());
        assert_eq!(fsm.state(), CarryState::Still);
        // third completes, and the transition names the exit AND the entry
        let t = fsm.step(&pocket_walk()).expect("switch on the third window");
        assert_eq!(t, CarryTransition { from: CarryState::Still, to: CarryState::PocketWaist });
    }

    #[test]
    fn one_flap_window_does_not_exit() {
        let mut fsm = CarryFsm::new(CarryGuards::default(), 3);
        for _ in 0..3 {
            fsm.step(&pocket_walk());
        }
        assert_eq!(fsm.state(), CarryState::PocketWaist);
        // a single ambiguous window (stride trough) must not leave Pocket
        assert!(fsm.step(&still()).is_none());
        for _ in 0..3 {
            fsm.step(&pocket_walk());
        }
        assert_eq!(fsm.state(), CarryState::PocketWaist);
    }

    #[test]
    fn exit_with_no_candidate_lands_in_unknown() {
        let mut fsm = CarryFsm::new(CarryGuards::default(), 2);
        for _ in 0..2 {
            fsm.step(&hand_held());
        }
        assert_eq!(fsm.state(), CarryState::HandHeld);
        // energetic chaos: huge rms, huge sway, no cadence → nothing's entry guard fires
        let chaos = feats(4.0, 4.0, 4.0, 6.0, 60.0, None);
        assert!(fsm.step(&chaos).is_none());
        let t = fsm.step(&chaos).expect("falls to Unknown");
        assert_eq!(t.from, CarryState::HandHeld);
        assert_eq!(t.to, CarryState::Unknown);
    }

    #[test]
    fn the_five_signatures_classify_as_themselves() {
        for (f, want) in [
            (still(), CarryState::Still),
            (pocket_walk(), CarryState::PocketWaist),
            (arm_swing(), CarryState::ArmSwing),
            (hand_held(), CarryState::HandHeld),
            (stowed_walk(), CarryState::StowedBack),
        ] {
            let mut fsm = CarryFsm::new(CarryGuards::default(), 2);
            for _ in 0..3 {
                fsm.step(&f);
            }
            assert_eq!(fsm.state(), want, "features {f:?}");
        }
    }

    #[test]
    fn tuning_presets_track_expected_damping_order() {
        // stowed (damped) < hand < pocket < swing (self-oscillating)
        let a = step_tuning_for(CarryState::StowedBack).th_hi;
        let b = step_tuning_for(CarryState::HandHeld).th_hi;
        let c = step_tuning_for(CarryState::PocketWaist).th_hi;
        let d = step_tuning_for(CarryState::ArmSwing).th_hi;
        assert!(a < b && b < c && c < d);
    }

    #[test]
    fn end_to_end_from_synthetic_samples() {
        // Drive the FSM from actual sample windows, not hand-built features: 6 s pocket walk.
        let hz = 50.0;
        let mut w = crate::gait::Windower::new(hz, 2.0, 0.5);
        let mut fsm = CarryFsm::new(CarryGuards::default(), 2);
        let mut reached = false;
        for s in crate::gait::walk(hz, 6.0, 2.0, 3.0, 1) {
            if let Some(f) = w.push(s) {
                fsm.step(&f);
                reached |= fsm.state() == CarryState::PocketWaist;
            }
        }
        assert!(reached, "sample-level pipeline reaches PocketWaist");
    }
}
