//! path_memory — an obstacle-aware pathing learner. A creature keeps a coarse
//! compass of 16 heading bins, each with a success score, plus an estimate of how
//! much clearance it needs. Moving freely rewards the heading it used; getting
//! blocked penalizes that heading, widens the clearance estimate, and asks the
//! body to back up and resurvey. To choose where to go it blends a goal bias with
//! the learned bin scores and shuns the bins it keeps hitting things on — so it
//! learns its way around an obstacle instead of grinding into it. Consumes the
//! contacts from `touch_rays` (passed in as a blocked flag). Pure + deterministic.

use std::f32::consts::{PI, TAU};

/// Number of heading bins around the compass.
pub const BINS: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurveyAction {
    /// Keep going — the way was clear.
    Proceed,
    /// Blocked: back up and re-survey before re-choosing.
    BackUpAndResurvey,
}

#[derive(Clone, Debug)]
pub struct PathMemory {
    /// Per-bin confidence, ~0 (always blocked) .. SCORE_MAX (reliably clear).
    score: [f32; BINS],
    /// Estimated clearance radius needed (blocks); grows when stuck.
    clearance: f32,
    base_clearance: f32,
}

const SCORE_MAX: f32 = 1.0;
const SCORE_MIN: f32 = 0.02;
const REWARD: f32 = 0.10;
const PENALTY: f32 = 0.45; // multiplicative shrink on a block (fast shun)
const CLEAR_GROW: f32 = 0.25;
const CLEAR_MAX_MULT: f32 = 4.0; // clearance never exceeds base × this

fn wrap_tau(a: f32) -> f32 {
    let mut x = a % TAU;
    if x < 0.0 {
        x += TAU;
    }
    x
}

/// The bin index a heading (radians) falls in.
pub fn bin_of(heading: f32) -> usize {
    let frac = wrap_tau(heading) / TAU;
    ((frac * BINS as f32) as usize) % BINS
}

/// The center heading (radians, in `[0, TAU)`) of a bin.
pub fn bin_heading(bin: usize) -> f32 {
    (bin as f32 + 0.5) / BINS as f32 * TAU
}

/// Shortest angular distance between two headings, in `[0, PI]`.
fn ang_diff(a: f32, b: f32) -> f32 {
    let d = wrap_tau(a - b);
    if d > PI {
        TAU - d
    } else {
        d
    }
}

impl PathMemory {
    pub fn new(base_clearance: f32) -> PathMemory {
        let bc = base_clearance.max(0.1);
        PathMemory { score: [SCORE_MAX; BINS], clearance: bc, base_clearance: bc }
    }

    pub fn clearance(&self) -> f32 {
        self.clearance
    }
    pub fn score_of(&self, heading: f32) -> f32 {
        self.score[bin_of(heading)]
    }

    /// The body moved freely along `heading`: reward that bin and let the
    /// clearance relax back toward its base.
    pub fn reward(&mut self, heading: f32) {
        let b = bin_of(heading);
        self.score[b] = (self.score[b] + REWARD).min(SCORE_MAX);
        self.clearance = (self.clearance - CLEAR_GROW * 0.5).max(self.base_clearance);
    }

    /// The body was blocked going `heading` (a `touch_rays` contact): shun that
    /// bin, widen the clearance estimate (bounded), and tell the caller to back up
    /// and resurvey.
    pub fn penalize(&mut self, heading: f32) -> SurveyAction {
        let b = bin_of(heading);
        self.score[b] = (self.score[b] * PENALTY).max(SCORE_MIN);
        self.clearance = (self.clearance + CLEAR_GROW).min(self.base_clearance * CLEAR_MAX_MULT);
        SurveyAction::BackUpAndResurvey
    }

    /// Choose a heading toward `goal_heading`, blending the goal bias with learned
    /// bin scores. With all bins equally clear this returns essentially the goal
    /// heading; where the goal direction has been blocked it veers to the best
    /// nearby clear bin.
    pub fn choose(&self, goal_heading: f32) -> f32 {
        let mut best_bin = bin_of(goal_heading);
        let mut best_w = f32::MIN;
        for b in 0..BINS {
            // goal affinity in [0,1]: 1 facing the goal, 0 opposite.
            let aff = 0.5 * (1.0 + ang_diff(bin_heading(b), goal_heading).cos());
            let w = aff * self.score[b];
            if w > best_w {
                best_w = w;
                best_bin = b;
            }
        }
        bin_heading(best_bin)
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
    fn a_repeatedly_blocked_heading_falls_below_a_free_one() {
        let mut pm = PathMemory::new(1.0);
        let blocked = 0.0;
        let free = PI; // opposite
        for _ in 0..5 {
            pm.penalize(blocked);
        }
        assert!(pm.score_of(blocked) < pm.score_of(free), "blocked {} < free {}", pm.score_of(blocked), pm.score_of(free));
    }

    #[test]
    fn clearance_grows_but_is_bounded_when_stuck() {
        let mut pm = PathMemory::new(1.0);
        for _ in 0..100 {
            pm.penalize(0.0);
        }
        assert!(pm.clearance() > 1.0, "clearance grew");
        assert!(pm.clearance() <= 1.0 * 4.0 + 1e-6, "clearance bounded: {}", pm.clearance());
    }

    #[test]
    fn goal_bias_is_preserved_when_all_clear() {
        let pm = PathMemory::new(1.0); // fresh: all bins max score
        let goal = bin_heading(5);
        let chosen = pm.choose(goal);
        assert_eq!(bin_of(chosen), 5, "with everything clear, heads to the goal bin");
    }

    #[test]
    fn it_converges_away_from_a_blocked_goal_direction() {
        let mut pm = PathMemory::new(1.0);
        let goal = bin_heading(0);
        for _ in 0..8 {
            pm.penalize(goal);
        }
        let chosen = pm.choose(goal);
        assert_ne!(bin_of(chosen), 0, "after repeated blocks it stops choosing the goal bin");
        // and it picks a nearby bin, not the opposite side
        assert!(ang_diff(chosen, goal) < PI * 0.6, "veers to a near alternative, not backward");
    }

    #[test]
    fn reward_relaxes_clearance_back_toward_base() {
        let mut pm = PathMemory::new(1.0);
        for _ in 0..10 {
            pm.penalize(0.0);
        }
        let stuck = pm.clearance();
        for _ in 0..100 {
            pm.reward(0.0);
        }
        assert!(pm.clearance() < stuck, "moving freely relaxes clearance");
        assert!(pm.clearance() >= 1.0 - 1e-6, "never below base");
    }

    #[test]
    fn prop_scores_and_clearance_stay_bounded_and_finite() {
        let mut seed = 0x9A7A_1234u32;
        for _ in 0..2000 {
            let mut pm = PathMemory::new(0.5 + lcg(&mut seed) * 2.0);
            for _ in 0..60 {
                let h = lcg(&mut seed) * TAU;
                if lcg(&mut seed) < 0.5 {
                    pm.reward(h);
                } else {
                    let _ = pm.penalize(h);
                }
            }
            for b in 0..BINS {
                let s = pm.score[b];
                assert!(s.is_finite() && s >= SCORE_MIN - 1e-6 && s <= SCORE_MAX + 1e-6, "score bounded: {s}");
            }
            assert!(pm.clearance().is_finite() && pm.clearance() <= pm.base_clearance * 4.0 + 1e-6, "clearance bounded");
            let c = pm.choose(lcg(&mut seed) * TAU);
            assert!(c.is_finite(), "chosen heading finite");
        }
    }
}
