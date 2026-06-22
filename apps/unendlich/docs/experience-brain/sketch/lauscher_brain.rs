//! lauscher_brain.rs — SKETCH (adapt to your tick loop; not drop-in code).
//!
//! Stage 0 (the stateful FSM) + the Stage 2 hook (budgeted experience tier).
//! Grounded in your existing types: `goals::{Goal, Signals, Disposition, choose}`,
//! `mob_memory::MobMemory`, `Fixed`. Everything is deterministic given
//! (signals, memory, seeded rng) so replays stay bit-exact — keep it that way.
//!
//! What this is: a small per-creature state machine whose states are behavioural
//! MODES. `goals::choose` still runs *inside* a mode to pick the instantaneous act;
//! the FSM supplies the temporal coherence (phases that persist) and the two memory
//! registers that make the lauscher creep closer over a long time.

use crate::sim::goals::{self, Goal, Signals, Disposition};
use crate::sim::mob_memory::MobMemory;
// use crate::Fixed;  // use your fixed-point type for every stored value below

/// Behavioural modes for the hearing-sentinel stalk loop.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    Listen,   // dormant, ears open; a loud signal wakes it
    Orient,   // turn toward the heard source (reuse awareness::best_facing)
    Shadow,   // move toward last-known pos, holding `standoff`
    Observe,  // hold still at range; accumulate curiosity; maybe encroach
    Freeze,   // player's gaze is on it → stop dead (the weeping-angel beat)
    Withdraw, // player too close → retreat to a remembered refuge
}

/// Per-creature stalk state. Persists across ticks — this is the bit the old
/// stateless argmax lacked.
#[derive(Clone, Debug)]
pub struct LauscherBrain {
    pub mode: Mode,
    /// Distance held during Observe. Ratchets inward on uncaught Observe cycles,
    /// snaps outward when seen. THE core "creepy over time" register. (use Fixed)
    pub standoff: f32,
    /// Integrated curiosity (build → peak → habituate), not instantaneous novelty.
    pub curiosity: f32,
    /// Anti-flicker: ticks spent in the current mode.
    pub dwell: u32,
    /// Deterministic per-mob stream; seed from the mob id + world seed.
    pub rng: u64,
}

impl LauscherBrain {
    pub fn new(seed: u64) -> Self {
        Self { mode: Mode::Listen, standoff: 12.0, curiosity: 0.0, dwell: 0, rng: seed }
    }

    /// One reactive tick. Cheap, deterministic. Returns the act to perform and the
    /// target standoff the locomotion layer should respect.
    pub fn step(
        &mut self,
        s: &Signals,
        d: &Disposition,
        mem: &MobMemory,
        self_pos: [f32; 3],
        seen_by_player: bool,   // the NEW signal you must add (player frustum ∋ self)
        player_dist: f32,
    ) -> (Goal, f32 /* target standoff */) {
        self.dwell += 1;

        // --- transitions (signals are the weights; keep thresholds tunable) ---
        let next = match self.mode {
            _ if seen_by_player && self.mode != Mode::Withdraw => {
                // caught looking → freeze, and push the standoff back out (caution)
                self.standoff = (self.standoff + 4.0).min(20.0);
                Mode::Freeze
            }
            _ if player_dist < self.standoff * 0.5 => Mode::Withdraw, // too close
            Mode::Listen if s.novelty > thresh(0.2) || s.threat > thresh(0.2) => Mode::Orient,
            Mode::Orient if self.dwell > 6 => Mode::Shadow,
            Mode::Shadow if player_dist <= self.standoff => Mode::Observe,
            Mode::Observe if self.dwell > 30 && !seen_by_player => {
                // an uncaught observation cycle: curiosity beats caution, encroach
                self.curiosity = (self.curiosity + 0.05 * f(d_curiosity(d))).min(1.0);
                self.standoff = (self.standoff - 0.5 * self.curiosity).max(2.0);
                Mode::Shadow // close the gap a little, then observe again
            }
            Mode::Freeze if !seen_by_player && self.dwell > 12 => Mode::Shadow,
            Mode::Withdraw if player_dist > self.standoff * 1.5 => Mode::Listen,
            m => m, // hold (natural hysteresis via dwell-gated transitions)
        };
        if next != self.mode { self.mode = next; self.dwell = 0; }

        // habituation: curiosity decays once familiar, so it eventually drifts off
        self.curiosity = (self.curiosity - 0.001).max(0.0);

        // --- the instantaneous act: let the existing utility selector pick, but
        //     bias the Signals by the mode so e.g. Observe leans Investigate ---
        let biased = bias_signals_for_mode(s, self.mode);
        let act = goals::choose(&biased, d, mode_to_goal(self.mode));
        (act, self.standoff)
    }
}

/// Stage 2 hook (budgeted — call ONLY under `ai_budget::should_full_tick(..)` for a
/// near `TickBand`, at most once per creature per slow period). Runs one trial of a
/// cheap SURROGATE search and distills the result back into `Disposition` / the
/// FSM thresholds. This is the macro/`evolve_toward` loop, creature-side.
///
/// `surrogate_score` must be CHEAP — for stealth that's geometry only (visibility
/// cone + path check), never the real game sim.
pub fn experience_trial<F: Fn(&Disposition) -> f32>(
    incumbent: &mut Disposition,
    propose: impl Fn(&Disposition, &mut u64) -> Disposition, // mutate (reuse fsm EvoOps idea)
    surrogate_score: F,
    rng: &mut u64,
) {
    let candidate = propose(incumbent, rng);
    if surrogate_score(&candidate) > surrogate_score(incumbent) {
        *incumbent = candidate; // distill: the cheap policy is now better
    }
    // NOTE: validate the whole tier by ABLATION elsewhere (with vs without it),
    // exactly like reference/macros.rs::ablate — don't assume it helps.
}

// ---- helpers (stand-ins; wire to your Fixed + Disposition fields) -------------
fn thresh(_x: f32) -> goals::Fixed { unimplemented!("Fixed::from_frac(...)") }
fn f(_x: f32) -> f32 { _x }
fn d_curiosity(_d: &Disposition) -> f32 { unimplemented!("d.curiosity as f32") }
fn bias_signals_for_mode(s: &Signals, _m: Mode) -> Signals { *s /* nudge novelty/threat per mode */ }
fn mode_to_goal(m: Mode) -> Goal {
    match m {
        Mode::Shadow | Mode::Observe => Goal::Investigate,
        Mode::Withdraw | Mode::Freeze => Goal::Evade,
        Mode::Orient | Mode::Listen => Goal::Patrol,
    }
}
