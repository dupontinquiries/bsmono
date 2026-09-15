//! anim — which gait a species uses, and how hard.
//!
//! This module used to contain a whole pose solver. It does not any more, and
//! the reason is worth recording: `unendlich-engine` already had one, better
//! than the one written here, and it has now been extracted to the shared
//! `creature-rig` crate. Its header diagnoses clawbound's exact bug in the
//! same words this module was written to fix — *"a leg that translates
//! instead of pivoting at the hip never plants"*.
//!
//! What was genuinely missing there, and is what remains here:
//!
//! * **Gait taxonomy.** `creature_rig::anim_rig` applies one diagonal-pair
//!   pattern (plus a `hop` flag). Real quadrupeds change *footfall pattern*
//!   with speed, not just frequency.
//! * **Species builds.** A bison must never bound and a mammoth must never
//!   leave a walk. That is a fact about animals, not about posing boxes, so
//!   it belongs to the game rather than to the rig.
//!
//! So: this picks the gait and shapes the drive; `creature-rig` does the
//! geometry. [`Anim::motion_input`] is the join.
//!
//! ## What clawbound still cannot express, honestly
//!
//! `MotionInput` has no footfall-phase input — it hard-codes diagonal pairs,
//! with `hop` for both-legs-together. So [`Gait::Trot`] and [`Gait::Bound`]
//! map exactly, and **`Walk`, `Pace` and `Canter` currently render as a
//! trot**. Getting them needs a phase-offset array threaded into
//! `anim_rig::pose`, which is a small upstream change to a crate two games now
//! share — deliberately not made as a side effect of this extraction. The
//! selection logic below is already correct, so that change lights it up
//! without touching this file.

use creature_rig::anim_rig::{MotionInput, SPEED_SATURATE};
use creature_rig::sim::liveliness::MotionState;

/// A quadruped footfall pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gait {
    /// 4-beat lateral sequence. Slow, always three feet down — what heavy
    /// animals use and what reads as "walking".
    Walk,
    /// 2-beat diagonal pairs. The default cruise, and the only pattern
    /// `anim_rig` natively draws.
    Trot,
    /// 2-beat lateral pairs. A stalking cat's creep.
    Pace,
    /// 3-beat, one lead.
    Canter,
    /// Front pair then hind pair. The cat sprint, and what a dash is.
    Bound,
}

impl Gait {
    /// Does this pattern move both legs of a pair together?
    ///
    /// The one bit of the taxonomy `anim_rig` can currently act on.
    pub fn is_paired(self) -> bool {
        matches!(self, Gait::Bound)
    }

    /// Fraction of the cycle a foot is on the ground. Faster gaits have a
    /// real suspension phase; a walk never leaves the floor.
    pub fn duty(self) -> f32 {
        match self {
            Gait::Walk => 0.65,
            Gait::Trot => 0.50,
            Gait::Pace => 0.50,
            Gait::Canter => 0.40,
            Gait::Bound => 0.32,
        }
    }

    /// How much limb swing this gait wants, as a multiplier on the rig's
    /// `swing_scale`. A bound throws the legs; a walk does not.
    pub fn swing_scale(self) -> f32 {
        match self {
            Gait::Walk => 0.75,
            Gait::Trot => 1.00,
            Gait::Pace => 0.85,
            Gait::Canter => 1.15,
            Gait::Bound => 1.35,
        }
    }
}

/// A species' body and gait ladder.
///
/// Speeds are absolute world units/sec rather than normalised, because a
/// bison and a hare moving at the same 8 u/s are doing genuinely different
/// things.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Build {
    /// Shoulder height, world units. Sets stride length.
    pub height: f32,
    pub length: f32,
    /// 0 = light, 1 = bison-heavy.
    pub heft: f32,
    pub walk_to: f32,
    pub trot_to: f32,
    /// The gait above `trot_to`.
    pub top_gait: Gait,
    pub can_pace: bool,
}

impl Build {
    pub const fn feline() -> Build {
        Build { height: 1.1, length: 2.2, heft: 0.35, walk_to: 2.5, trot_to: 7.0, top_gait: Gait::Bound, can_pace: true }
    }
    pub const fn mini_cat() -> Build {
        Build { height: 0.6, length: 1.2, heft: 0.20, walk_to: 2.0, trot_to: 6.0, top_gait: Gait::Bound, can_pace: true }
    }
    pub const fn hare() -> Build {
        Build { height: 0.4, length: 0.7, heft: 0.10, walk_to: 1.5, trot_to: 3.0, top_gait: Gait::Bound, can_pace: false }
    }
    pub const fn stag() -> Build {
        Build { height: 1.5, length: 2.4, heft: 0.45, walk_to: 2.5, trot_to: 8.0, top_gait: Gait::Canter, can_pace: false }
    }
    /// Bison-class: heavy, walks and trots, never bounds.
    pub const fn megafauna() -> Build {
        Build { height: 2.4, length: 4.2, heft: 1.0, walk_to: 2.0, trot_to: 6.0, top_gait: Gait::Canter, can_pace: false }
    }
    /// Mammoth-class. Never leaves a walk: a suspension phase on something
    /// this heavy reads as weightless, which is the opposite of what the
    /// silhouette promises.
    pub const fn colossus() -> Build {
        Build { height: 4.0, length: 7.0, heft: 1.0, walk_to: 3.0, trot_to: f32::INFINITY, top_gait: Gait::Walk, can_pace: false }
    }

    pub fn gait_for(&self, speed: f32) -> Gait {
        if speed <= self.walk_to {
            if self.can_pace && speed < self.walk_to * 0.45 {
                Gait::Pace
            } else {
                Gait::Walk
            }
        } else if speed <= self.trot_to {
            Gait::Trot
        } else {
            self.top_gait
        }
    }

    /// Stride frequency (cycles/sec).
    ///
    /// Derived from stride LENGTH, not picked per gait, so the feet keep up
    /// with the ground instead of skating — and so a colossus takes slow
    /// enormous steps while a hare buzzes, from one rule.
    pub fn stride_hz(&self, speed: f32) -> f32 {
        let stride_len = (self.height * 1.6).max(0.2);
        (speed / stride_len).clamp(0.0, 12.0)
    }
}

/// Per-creature animation state: the gait phase and the smoothing that keeps
/// a gait change from snapping.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anim {
    pub build: Build,
    /// Gait phase in RADIANS, matching `anim_rig::MotionInput::gait_phase`.
    pub gait_phase: f32,
    /// Seconds, for the rig's breath cycle.
    pub time: f32,
    speed_s: f32,
    gait: Gait,
}

impl Anim {
    pub fn new(build: Build) -> Anim {
        Anim { build, gait_phase: 0.0, time: 0.0, speed_s: 0.0, gait: Gait::Walk }
    }

    pub fn gait(&self) -> Gait {
        self.gait
    }
    pub fn smoothed_speed(&self) -> f32 {
        self.speed_s
    }

    /// Advance the phase. `airborne` freezes it — feet do not cycle in the
    /// air, they reach for the ground.
    pub fn advance(&mut self, dt: f32, speed: f32, airborne: bool) {
        let dt = dt.clamp(0.0, 0.25);
        self.time += dt;
        // Smooth before selecting: raw speed crossing a threshold every frame
        // in a fight makes the legs stutter between patterns, and the
        // creature should commit to a gait for a beat.
        self.speed_s += (speed - self.speed_s) * (dt * 6.0).min(1.0);
        self.gait = self.build.gait_for(self.speed_s);
        if !airborne {
            let hz = self.build.stride_hz(self.speed_s);
            self.gait_phase =
                (self.gait_phase + hz * dt * std::f32::consts::TAU) % std::f32::consts::TAU;
        }
    }

    /// Build the rig's input for this frame.
    ///
    /// `turn` is normalised `[-1,1]` (+ = right), matching `anim_rig`.
    /// `impact` is landing squash `[0,1]`.
    pub fn motion_input(&self, turn: f32, gaze: (f32, f32), impact: f32, state: MotionState, progress: f32) -> MotionInput {
        MotionInput {
            gait_phase: self.gait_phase,
            speed: self.speed_s,
            // Saturate relative to THIS animal's own top gait rather than the
            // rig's global constant: otherwise a colossus at its flat-out
            // 3 u/s registers as barely moving and never swings its legs,
            // while a hare pins the scale permanently.
            max_speed: self.build.trot_to.min(SPEED_SATURATE * 2.0).max(1.0),
            leg_health: 1.0,
            flying: false,
            fly_power: 0.0,
            turn: turn.clamp(-1.0, 1.0),
            gaze_yaw: gaze.0,
            gaze_pitch: gaze.1,
            state,
            progress,
            time: self.time,
            impact: impact.clamp(0.0, 1.0),
            settle: 0.0,
            strike: 0.0,
            hop: self.gait.is_paired(),
            // Heavier animals swing less freely, on top of the gait's own
            // drive.
            swing_scale: self.gait.swing_scale() * (1.0 - 0.25 * self.build.heft),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bison must never bound and a mammoth must never leave a walk. This
    /// is the whole reason this layer exists on top of the shared rig.
    #[test]
    fn heavy_builds_cap_below_bounding() {
        assert_eq!(Build::megafauna().gait_for(50.0), Gait::Canter);
        assert_eq!(Build::colossus().gait_for(50.0), Gait::Walk);
        assert_eq!(Build::feline().gait_for(50.0), Gait::Bound);
    }

    #[test]
    fn gait_changes_up_with_speed() {
        let c = Build::feline();
        assert_eq!(c.gait_for(0.5), Gait::Pace, "a slow cat creeps");
        assert_eq!(c.gait_for(2.0), Gait::Walk);
        assert_eq!(c.gait_for(5.0), Gait::Trot);
        assert_eq!(c.gait_for(10.0), Gait::Bound);
    }

    /// Stride frequency from stride length, or the feet skate.
    #[test]
    fn small_builds_step_faster_at_the_same_speed() {
        let v = 5.0;
        assert!(Build::hare().stride_hz(v) > Build::feline().stride_hz(v));
        assert!(Build::feline().stride_hz(v) > Build::colossus().stride_hz(v));
        assert_eq!(Build::feline().stride_hz(0.0), 0.0);
    }

    /// Only a bound pairs the legs; that is the one bit of the taxonomy the
    /// rig can act on today.
    #[test]
    fn only_bounding_sets_hop() {
        assert!(Gait::Bound.is_paired());
        for g in [Gait::Walk, Gait::Trot, Gait::Pace, Gait::Canter] {
            assert!(!g.is_paired(), "{g:?} must not hop");
        }
    }

    #[test]
    fn duty_falls_as_the_gait_quickens() {
        assert!(Gait::Walk.duty() > Gait::Trot.duty());
        assert!(Gait::Trot.duty() > Gait::Canter.duty());
        assert!(Gait::Canter.duty() > Gait::Bound.duty());
        assert!(Gait::Walk.duty() > 0.5, "a walk always has feet down");
        assert!(Gait::Bound.duty() < 0.5, "a bound has a suspension phase");
    }

    /// Feet must not cycle off the ground.
    #[test]
    fn airborne_freezes_the_phase() {
        let mut a = Anim::new(Build::feline());
        a.advance(0.1, 8.0, false);
        let moved = a.gait_phase;
        assert!(moved > 0.0);
        for _ in 0..20 {
            a.advance(1.0 / 60.0, 8.0, true);
        }
        assert_eq!(a.gait_phase, moved, "phase must freeze in the air");
    }

    /// Phase must stay in `[0, TAU)` — it feeds a rig that takes radians and
    /// runs for a whole match.
    #[test]
    fn phase_stays_bounded_over_a_long_match() {
        let mut a = Anim::new(Build::feline());
        for _ in 0..(60 * 60 * 10) {
            a.advance(1.0 / 60.0, 9.0, false);
        }
        assert!(a.gait_phase.is_finite());
        assert!((0.0..std::f32::consts::TAU).contains(&a.gait_phase), "got {}", a.gait_phase);
    }

    /// A gait change must not snap: smoothing means the selected gait lags a
    /// step change in speed rather than flipping on the frame.
    #[test]
    fn speed_is_smoothed_before_the_gait_is_chosen() {
        let mut a = Anim::new(Build::feline());
        a.advance(1.0 / 60.0, 20.0, false);
        assert!(a.smoothed_speed() < 20.0, "must not jump to the new speed");
        assert_ne!(a.gait(), Gait::Bound, "gait lags the step change");
        for _ in 0..120 {
            a.advance(1.0 / 60.0, 20.0, false);
        }
        assert_eq!(a.gait(), Gait::Bound, "and then commits");
    }

    /// `max_speed` must be per-animal, or a colossus flat-out reads as
    /// standing still and never swings a leg.
    #[test]
    fn saturation_speed_is_per_build_not_global() {
        let colossus = Anim::new(Build::colossus()).motion_input(0.0, (0.0, 0.0), 0.0, MotionState::Idle, 0.0);
        let cat = Anim::new(Build::feline()).motion_input(0.0, (0.0, 0.0), 0.0, MotionState::Idle, 0.0);
        assert!(colossus.max_speed < cat.max_speed);
        assert!(colossus.max_speed >= 1.0, "never zero, which would divide out");
    }

    /// Heavier animals swing less freely than light ones in the same gait.
    #[test]
    fn heft_damps_the_swing() {
        let mut light = Anim::new(Build::mini_cat());
        let mut heavy = Anim::new(Build::megafauna());
        for _ in 0..200 {
            light.advance(1.0 / 60.0, 4.0, false);
            heavy.advance(1.0 / 60.0, 4.0, false);
        }
        let l = light.motion_input(0.0, (0.0, 0.0), 0.0, MotionState::Idle, 0.0);
        let h = heavy.motion_input(0.0, (0.0, 0.0), 0.0, MotionState::Idle, 0.0);
        assert!(l.swing_scale > h.swing_scale, "light {} vs heavy {}", l.swing_scale, h.swing_scale);
    }

    /// Inputs the rig clamps anyway are clamped here too, so a caller passing
    /// a raw yaw rate cannot push the rig outside its tested range.
    #[test]
    fn turn_and_impact_are_clamped() {
        let a = Anim::new(Build::feline());
        let m = a.motion_input(9.0, (0.0, 0.0), 5.0, MotionState::Idle, 0.0);
        assert_eq!(m.turn, 1.0);
        assert_eq!(m.impact, 1.0);
        let m = a.motion_input(-9.0, (0.0, 0.0), -5.0, MotionState::Idle, 0.0);
        assert_eq!(m.turn, -1.0);
        assert_eq!(m.impact, 0.0);
    }

    /// A pathological dt must not corrupt the phase.
    #[test]
    fn a_long_frame_is_survivable() {
        let mut a = Anim::new(Build::feline());
        for _ in 0..10 {
            a.advance(5.0, 9.0, false);
            assert!(a.gait_phase.is_finite());
            assert!((0.0..std::f32::consts::TAU).contains(&a.gait_phase));
        }
    }
}
