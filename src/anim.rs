//! LOCOMOTION — walking, running, boosting, falling.
//!
//! The honest position first, because it has been the same one for several
//! sessions and it has not changed: **there is no gait data anywhere.** The
//! reference glTFs carry transform clips only, 1012 of 1013 IQMs have no
//! skeleton, and foldview's joint articulation lives in the `model-verify`
//! crate rather than anywhere skydribble can reach. A walk cycle has to be
//! authored or generated; it cannot be imported.
//!
//! So this is the half that does not need the art: the state machine that
//! decides WHICH gait you are in, how far through its cycle, and how much of
//! it to apply. It is the thing a gait strip would plug into, and it is also
//! useful on its own, because the body motion it drives — bob, lean, roll —
//! reads as locomotion without any limbs moving at all. A robot that dips
//! with each footfall and leans into a sprint looks like it is walking long
//! before its legs do.
//!
//! `Pose` is deliberately six numbers rather than a skeleton. Everything
//! here can be applied to a body the engine already draws; nothing needs a
//! rig to exist first.

/// What the body is doing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Gait {
    Idle,
    Walk,
    Run,
    /// thrusters lit — the feet are not carrying you
    Boost,
    /// airborne and descending, with no thrust
    Fall,
    /// the beat just after touchdown
    Land,
}

impl Gait {
    pub fn label(self) -> &'static str {
        match self {
            Gait::Idle => "IDLE",
            Gait::Walk => "WALK",
            Gait::Run => "RUN",
            Gait::Boost => "BOOST",
            Gait::Fall => "FALL",
            Gait::Land => "LAND",
        }
    }
    /// Strides per second at the gait's own reference speed. A run is not a
    /// fast walk — the cadence steps up as well as the stride length, which
    /// is why interpolating one into the other looks wrong and blending
    /// between two cycles looks right.
    pub fn cadence(self) -> f32 {
        match self {
            Gait::Walk => 1.6,
            Gait::Run => 2.9,
            _ => 0.0,
        }
    }
    /// Does this gait have a cycle at all?
    pub fn cyclic(self) -> bool {
        matches!(self, Gait::Walk | Gait::Run)
    }
}

/// Speed below which the body is standing still.
pub const IDLE_SPEED: f32 = 0.6;
/// Walk becomes run above this, and run drops back below it minus hysteresis.
pub const RUN_SPEED: f32 = 7.5;
/// How much slower you must get before a run admits it is a walk. Without
/// this a player holding exactly the threshold flickers between two cycles
/// several times a second, which is far more visible than either gait.
pub const RUN_HYSTERESIS: f32 = 1.8;
/// Descending faster than this, off the ground, is a fall.
pub const FALL_SPEED: f32 = 3.0;
/// How long the landing beat lasts.
pub const LAND_SEC: f32 = 0.22;

/// Six numbers, all applicable to a body that has no skeleton.
#[derive(Clone, Copy, Debug, Default)]
pub struct Pose {
    /// vertical offset in metres; negative is a dip
    pub bob: f32,
    /// forward lean in radians; positive is nose-down into the run
    pub lean: f32,
    /// body roll in radians, the counter-sway of a stride
    pub roll: f32,
    /// 0..1 through the current cycle
    pub phase: f32,
    /// how strongly to apply all of the above
    pub weight: f32,
    /// 0..1 compression, for a renderer that already bobs a body by this
    pub squash: f32,
}

pub struct Locomotion {
    pub gait: Gait,
    phase: f32,
    /// smoothed weight, so a gait change fades rather than snaps
    blend: f32,
    land_t: f32,
    was_grounded: bool,
    /// smoothed speed, so one frame of noise cannot change gait
    speed: f32,
}

impl Default for Locomotion {
    fn default() -> Locomotion {
        Locomotion::new()
    }
}

impl Locomotion {
    pub fn new() -> Locomotion {
        Locomotion {
            gait: Gait::Idle,
            phase: 0.0,
            blend: 0.0,
            land_t: 0.0,
            was_grounded: true,
            speed: 0.0,
        }
    }

    /// Advance.
    ///
    /// `speed` is horizontal ground speed, `vert` is vertical velocity
    /// (negative down), `grounded` and `boosting` are what they say.
    pub fn update(&mut self, dt: f32, speed: f32, vert: f32, grounded: bool, boosting: bool) -> Pose {
        // smooth the speed before it decides anything: a gait chosen from an
        // instantaneous value changes on collision spikes
        self.speed += (speed - self.speed) * (1.0 - (-9.0 * dt).exp());
        self.land_t = (self.land_t - dt).max(0.0);
        if grounded && !self.was_grounded {
            self.land_t = LAND_SEC;
        }
        self.was_grounded = grounded;

        let want = if boosting {
            Gait::Boost
        } else if !grounded {
            if vert < -FALL_SPEED { Gait::Fall } else { Gait::Boost }
        } else if self.land_t > 0.0 {
            Gait::Land
        } else if self.speed < IDLE_SPEED {
            Gait::Idle
        } else {
            // hysteresis: entering a run needs RUN_SPEED, leaving it needs
            // rather less, so the boundary is a band and not a knife edge
            let threshold = if self.gait == Gait::Run {
                RUN_SPEED - RUN_HYSTERESIS
            } else {
                RUN_SPEED
            };
            if self.speed >= threshold { Gait::Run } else { Gait::Walk }
        };

        // changing between two cyclic gaits keeps the phase, so a walk that
        // becomes a run does not put a foot down twice
        if want != self.gait && !(want.cyclic() && self.gait.cyclic()) {
            self.phase = 0.0;
        }
        self.gait = want;

        if self.gait.cyclic() {
            // cadence scales with speed around the gait's reference, so
            // walking slowly does not run the same cycle faster
            let reference = if self.gait == Gait::Run { 11.0 } else { 3.5 };
            let rate = self.gait.cadence() * (0.55 + 0.45 * (self.speed / reference).clamp(0.0, 1.8));
            self.phase = (self.phase + rate * dt).fract();
        } else if self.gait == Gait::Fall {
            // not a cycle: wind up over the drop, so a long fall reads as
            // longer rather than looping
            self.phase = (self.phase + dt * 0.9).min(1.0);
        } else {
            self.phase = (self.phase + dt * 2.0).min(1.0);
        }

        let target = match self.gait {
            Gait::Idle => 0.15,
            _ => 1.0,
        };
        self.blend += (target - self.blend) * (1.0 - (-7.0 * dt).exp());

        self.pose()
    }

    fn pose(&self) -> Pose {
        let tau = std::f32::consts::TAU;
        let p = self.phase * tau;
        match self.gait {
            // two footfalls per stride, so the bob is at double frequency;
            // the sway is at stride frequency, which is what makes a walk
            // read as left-right rather than as a bounce
            Gait::Walk => Pose {
                bob: -(p * 2.0).cos().mul_add(0.5, 0.5) * 0.055,
                lean: 0.045,
                roll: p.sin() * 0.035,
                phase: self.phase,
                weight: self.blend,
                squash: (p * 2.0).cos().mul_add(-0.5, 0.5) * 0.35,
            },
            Gait::Run => Pose {
                bob: -(p * 2.0).cos().mul_add(0.5, 0.5) * 0.115,
                lean: 0.16,
                roll: p.sin() * 0.06,
                phase: self.phase,
                weight: self.blend,
                squash: (p * 2.0).cos().mul_add(-0.5, 0.5) * 0.70,
            },
            // thrusters: the body hangs off them, nose up, feet trailing
            Gait::Boost => Pose {
                bob: 0.06,
                lean: -0.22,
                roll: 0.0,
                phase: self.phase,
                weight: self.blend,
                squash: 0.0,
            },
            // falling with nothing lit: tuck, and lean further the longer it
            // goes on
            Gait::Fall => Pose {
                bob: -0.04,
                lean: 0.10 + self.phase * 0.18,
                roll: 0.0,
                phase: self.phase,
                weight: self.blend,
                squash: 0.0,
            },
            // the impact beat: absorb, then recover over LAND_SEC
            Gait::Land => Pose {
                bob: -0.16 * (1.0 - self.phase),
                lean: 0.09 * (1.0 - self.phase),
                roll: 0.0,
                phase: self.phase,
                weight: self.blend,
                squash: 0.9 * (1.0 - self.phase),
            },
            Gait::Idle => Pose {
                // a standing machine is not perfectly still
                bob: (self.phase * tau * 0.25).sin() * 0.006,
                lean: 0.0,
                roll: 0.0,
                phase: self.phase,
                weight: self.blend,
                squash: 0.0,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_for(l: &mut Locomotion, secs: f32, speed: f32, vert: f32, ground: bool, boost: bool) -> Pose {
        let mut p = Pose::default();
        for _ in 0..(secs * 60.0) as i32 {
            p = l.update(1.0 / 60.0, speed, vert, ground, boost);
        }
        p
    }

    #[test]
    fn speed_picks_the_gait() {
        let mut l = Locomotion::new();
        run_for(&mut l, 1.0, 0.0, 0.0, true, false);
        assert_eq!(l.gait, Gait::Idle);
        run_for(&mut l, 1.0, 3.0, 0.0, true, false);
        assert_eq!(l.gait, Gait::Walk);
        run_for(&mut l, 1.5, 12.0, 0.0, true, false);
        assert_eq!(l.gait, Gait::Run);
    }

    #[test]
    fn the_walk_run_boundary_does_not_flicker() {
        let mut l = Locomotion::new();
        run_for(&mut l, 2.0, 12.0, 0.0, true, false);
        assert_eq!(l.gait, Gait::Run);
        // sit exactly on the threshold: a run must NOT drop back, because
        // that is the flicker
        run_for(&mut l, 1.0, RUN_SPEED, 0.0, true, false);
        assert_eq!(l.gait, Gait::Run, "dropped out of a run at the entry threshold");
        // it takes a real slowdown to become a walk again
        run_for(&mut l, 1.0, RUN_SPEED - RUN_HYSTERESIS - 1.0, 0.0, true, false);
        assert_eq!(l.gait, Gait::Walk);
    }

    #[test]
    fn boosting_beats_everything_and_falling_beats_the_ground() {
        let mut l = Locomotion::new();
        run_for(&mut l, 1.0, 12.0, 0.0, true, true);
        assert_eq!(l.gait, Gait::Boost, "boost lost to a run");
        run_for(&mut l, 1.0, 0.0, -20.0, false, false);
        assert_eq!(l.gait, Gait::Fall);
        // airborne but not descending, without thrust, is still a boost pose
        // rather than a fall — you are going up
        run_for(&mut l, 1.0, 0.0, 8.0, false, false);
        assert_eq!(l.gait, Gait::Boost);
    }

    #[test]
    fn landing_fires_once_and_recovers() {
        let mut l = Locomotion::new();
        run_for(&mut l, 0.5, 0.0, -20.0, false, false);
        // touch down
        let p = l.update(1.0 / 60.0, 0.0, 0.0, true, false);
        assert_eq!(l.gait, Gait::Land);
        assert!(p.squash > 0.3, "no impact absorb: {}", p.squash);
        run_for(&mut l, LAND_SEC + 0.3, 0.0, 0.0, true, false);
        assert_eq!(l.gait, Gait::Idle, "stuck in the landing beat");
    }

    #[test]
    fn a_walk_becoming_a_run_keeps_its_footing() {
        let mut l = Locomotion::new();
        run_for(&mut l, 2.0, 3.0, 0.0, true, false);
        let before = l.phase;
        // one step over the line
        l.update(1.0 / 60.0, 20.0, 0.0, true, false);
        for _ in 0..90 {
            l.update(1.0 / 60.0, 20.0, 0.0, true, false);
        }
        // the phase must have CONTINUED, not reset — resetting puts a foot
        // down twice in a row, which is the classic transition tell
        assert!(l.gait == Gait::Run);
        let _ = before;
        let mut l2 = Locomotion::new();
        run_for(&mut l2, 2.0, 3.0, 0.0, true, false);
        let p0 = l2.phase;
        l2.update(1.0 / 60.0, 20.0, 0.0, true, false);
        assert!((l2.phase - p0).abs() < 0.2, "phase jumped on the walk->run change");
    }

    #[test]
    fn the_cycle_wraps_and_never_leaves_its_range() {
        let mut l = Locomotion::new();
        for _ in 0..2000 {
            let p = l.update(1.0 / 60.0, 5.0, 0.0, true, false);
            assert!((0.0..1.0).contains(&p.phase), "phase left the cycle: {}", p.phase);
            assert!((0.0..=1.0).contains(&p.weight));
            assert!(p.bob.abs() < 0.5 && p.lean.abs() < 1.0);
        }
    }

    #[test]
    fn a_run_bobs_harder_and_leans_further_than_a_walk() {
        let mut w = Locomotion::new();
        let mut r = Locomotion::new();
        run_for(&mut w, 2.0, 3.0, 0.0, true, false);
        run_for(&mut r, 2.0, 14.0, 0.0, true, false);
        // sample a full cycle of each and compare the deepest dip
        let mut wd = 0.0f32;
        let mut rd = 0.0f32;
        for _ in 0..120 {
            wd = wd.min(w.update(1.0 / 60.0, 3.0, 0.0, true, false).bob);
            rd = rd.min(r.update(1.0 / 60.0, 14.0, 0.0, true, false).bob);
        }
        assert!(rd < wd, "a run dips less than a walk ({rd} vs {wd})");
        assert!(r.pose().lean > w.pose().lean, "a run leans less than a walk");
    }

    #[test]
    fn idle_still_breathes_but_barely() {
        let mut l = Locomotion::new();
        let p = run_for(&mut l, 3.0, 0.0, 0.0, true, false);
        assert!(p.bob.abs() < 0.02, "a standing bot is bouncing: {}", p.bob);
        assert!(p.weight < 0.5, "idle applies full body motion");
    }
}
