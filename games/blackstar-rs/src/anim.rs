//! LOCOMOTION — walking, running, boosting, falling.
//!
//! CORRECTION, and it matters: I said several times there was no gait data
//! anywhere. That was wrong. `exengine/robot_parts.c` has a complete
//! procedural gait — `ex_gait_advance` plus a 17-slot part layout driven by
//! phase and speed — and the numbers below are now ITS numbers rather than
//! ones I made up. What is true is narrower: there are no gait CLIPS. There
//! is a gait MODEL, and a model is better, because it scales with speed
//! instead of being resampled.
//!
//! The cadence is the part worth reading. Stride frequency rises with the
//! SQUARE ROOT of speed — the Froude relation for legged locomotion — and
//! saturates at 7.4 rad/s, so a sprinting bot takes slower, longer strides
//! rather than whirling its legs. Stride LENGTH grows to carry the ground
//! the slower cadence no longer does. My first pass had cadence rising
//! linearly with speed and capped nothing, which is exactly the whirling
//! the C engine's comment says it fixed.
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
    /// SCALING A WALL. Ported alongside `physics::Car::climbing` — see
    /// that field's own doc comment and `World::wall_ahead`. Cyclic
    /// (unlike Boost/Fall/Land): the arm-over-arm reach genuinely
    /// repeats for as long as the climb continues, the same way a walk
    /// or run does, just driven by a fixed cadence rather than ground
    /// speed (there is no "how fast am I walking" signal while
    /// climbing — the C engine's own `g_climb_ph` advances at a flat
    /// rate per chassis archetype for the same reason).
    Climb,
    /// HURT AND STILL STANDING. A damaged body drops its centre of mass:
    /// shorter stride, deeper bob, and a lean that never comes back up.
    /// Not a separate cycle — a crouch is a walk done badly, so the phase
    /// carries and the limbs keep swinging, just less.
    Crouch,
    /// HURT AND FINDING A WALL. Below the crouch threshold a body stops
    /// presenting its front: it turns a shoulder in, rolls toward the
    /// nearest cover, and keeps moving low. This is the last gait before
    /// the body stops being a body.
    WallHug,
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
            Gait::Crouch => "CROUCH",
            Gait::WallHug => "HUG",
            Gait::Climb => "CLIMB",
        }
    }
    /// Cadence multiplier on top of the Froude term. A run is not merely a
    /// fast walk, but the difference is smaller than it looks: most of the
    /// extra ground is covered by stride LENGTH.
    ///
    /// `Climb` returns 0.0 here like every other non-ground-speed gait —
    /// it has its own fixed-rate phase advance in `update()` (see that
    /// function's own comment), not this Froude-scaled one, since there
    /// is no ground speed to scale while climbing.
    pub fn cadence(self) -> f32 {
        match self {
            Gait::Walk => 1.0,
            Gait::Run => 1.12,
            // a hurt body takes more steps for less ground: cadence up,
            // stride down. The stride length is in the pose's swing
            // amplitude, so these two together read as scurrying rather
            // than as slow motion.
            Gait::Crouch => 1.18,
            Gait::WallHug => 1.3,
            _ => 0.0,
        }
    }
    /// Does this gait have a cycle at all? `Climb` is deliberately NOT
    /// included here even though it loops — this flag specifically means
    /// "driven by `stride_freq(ground speed)`", which climbing isn't.
    pub fn cyclic(self) -> bool {
        matches!(self, Gait::Walk | Gait::Run | Gait::Crouch | Gait::WallHug)
    }
    /// Is this body hurt? Used by the shell to decide whether a camera or
    /// a HUD should say anything about it.
    pub fn wounded(self) -> bool {
        matches!(self, Gait::Crouch | Gait::WallHug)
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

/// Damage at which a body stops walking upright. `sections::Body::limp()`
/// is the input. A GUESS, but a considered one: a third of the way to
/// wrecked is when a leg section is usually gone, and a body missing a leg
/// section should not still be striding.
pub const CROUCH_HURT: f32 = 0.34;
/// ...and where it starts looking for a wall.
pub const HUG_HURT: f32 = 0.68;

// ---- ported from exengine/robot_parts.h ----
/// base cadence, before the Froude term
pub const GAIT_CADENCE: f32 = 0.95;
/// stride frequency saturates here: a sprint never spins the legs
pub const GAIT_FREQ_MAX: f32 = 7.4;
pub const LEG_SWING_MAX: f32 = 1.05;
pub const ARM_SWING_MAX: f32 = 0.62;
pub const BODY_BOB_MAX: f32 = 0.19;

/// Stride frequency in radians per second at this speed.
///
/// `EX_GAIT_CADENCE * 1.70 * sqrt(speed)`, capped. Square root rather than
/// linear because that is how legs work.
pub fn stride_freq(speed: f32) -> f32 {
    (GAIT_CADENCE * 1.70 * speed.max(0.0).sqrt()).min(GAIT_FREQ_MAX)
}

/// One leg's swing angle. `odd` is the other leg, half a cycle out.
pub fn leg_swing(phase: f32, odd: bool, speed: f32) -> f32 {
    let amp = (speed / 6.0).min(1.5) * LEG_SWING_MAX;
    let p = phase + if odd { std::f32::consts::PI } else { 0.0 };
    p.sin() * amp
}

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
    /// limb swing in radians: legs, and arms counter-phase against them.
    /// Nothing consumes these yet — the bodies are pose strips, not rigs —
    /// but they are what a rigged bot would be driven by, and they are the
    /// C engine's model rather than an approximation of it.
    pub leg_l: f32,
    pub leg_r: f32,
    pub arm_l: f32,
    pub arm_r: f32,
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
    /// 0 = whole, 1 = barely standing. Smoothed like the speed, because a
    /// section popping off should bend a body over rather than snap it.
    hurt: f32,
    /// which way the nearest wall is, in the body's own frame: -1 left,
    /// +1 right, 0 none in reach. A hug needs something to hug.
    wall: f32,
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
            hurt: 0.0,
            wall: 0.0,
        }
    }

    /// Tell the machine how badly the body is doing, and where cover is.
    ///
    /// `hurt` is 0..1 — `sections::Body::limp()` is exactly this number and
    /// is what the shell passes. `wall` is the signed lateral bearing of
    /// the nearest surface within reach, or 0 for open ground: a body only
    /// hugs what is actually there.
    pub fn condition(&mut self, hurt: f32, wall: f32) {
        self.hurt = hurt.clamp(0.0, 1.0);
        self.wall = wall.clamp(-1.0, 1.0);
    }

    /// Advance.
    ///
    /// `speed` is horizontal ground speed, `vert` is vertical velocity
    /// (negative down), `grounded` and `boosting` are what they say.
    /// `climbing` is `physics::Car::climbing > 0.5` — a real wall-climb
    /// in progress, which outranks the usual airborne Fall/Boost choice
    /// the same way a deliberate player action should.
    pub fn update(&mut self, dt: f32, speed: f32, vert: f32, grounded: bool, boosting: bool, climbing: bool) -> Pose {
        // smooth the speed before it decides anything: a gait chosen from an
        // instantaneous value changes on collision spikes
        self.speed += (speed - self.speed) * (1.0 - (-9.0 * dt).exp());
        self.land_t = (self.land_t - dt).max(0.0);
        if grounded && !self.was_grounded {
            self.land_t = LAND_SEC;
        }
        self.was_grounded = grounded;

        let want = if climbing {
            Gait::Climb
        } else if boosting {
            Gait::Boost
        } else if !grounded {
            if vert < -FALL_SPEED { Gait::Fall } else { Gait::Boost }
        } else if self.land_t > 0.0 {
            Gait::Land
        } else if self.hurt > HUG_HURT && self.wall.abs() > 0.2 {
            // badly hurt with something to put a shoulder against. Ranked
            // above Idle deliberately: a body this damaged is hugging the
            // wall whether or not it is going anywhere.
            Gait::WallHug
        } else if self.hurt > CROUCH_HURT {
            Gait::Crouch
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
            // radians per second, Froude-scaled and saturating
            let rate = stride_freq(self.speed) * self.gait.cadence();
            self.phase = (self.phase + rate * dt / std::f32::consts::TAU).fract();
        } else if self.gait == Gait::Fall {
            // not a cycle: wind up over the drop, so a long fall reads as
            // longer rather than looping
            self.phase = (self.phase + dt * 0.9).min(1.0);
        } else if self.gait == Gait::Climb {
            // GENUINELY LOOPS, unlike Fall/Boost/Land, but at a fixed
            // rate rather than one scaled by ground speed (there isn't
            // any while climbing) — a flat middle value between the C
            // engine's own per-archetype climb_rate range (4.2 heavy ..
            // 8.5 flyer), since this port has no equivalent per-chassis
            // signal wired into Locomotion to pick from.
            const CLIMB_RATE: f32 = 6.0;
            self.phase = (self.phase + CLIMB_RATE * dt / std::f32::consts::TAU).fract();
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
            // bob and roll are the C engine's: move_amp * BODY_BOB_MAX at
            // twice stride frequency, and sin(phase) * 0.04 * move_amp
            Gait::Walk | Gait::Run | Gait::Crouch | Gait::WallHug => {
                let move_amp = (self.speed / 8.0).clamp(0.0, 1.0);
                let ll = leg_swing(p, false, self.speed);
                let lr = leg_swing(p, true, self.speed);
                let k = ARM_SWING_MAX / LEG_SWING_MAX;
                // A HURT BODY IS THE SAME CYCLE, DONE SMALLER. Shortening
                // the stride and dropping the hips is what a limp is —
                // inventing a second cycle would put a foot down twice
                // when the gait changed mid-stride, which is the bug the
                // phase-carrying rule above exists to prevent.
                let (stride, sink, hunch, tilt) = match self.gait {
                    Gait::Crouch => (0.62, -BODY_BOB_MAX * 0.55, 0.22, 0.0),
                    // a hug turns a shoulder into the wall and stays low;
                    // the roll is toward the wall, so it leans on it
                    Gait::WallHug => (0.45, -BODY_BOB_MAX * 0.95, 0.34, self.wall * 0.28),
                    _ => (1.0, 0.0, 0.0, 0.0),
                };
                Pose {
                    bob: -(2.0 * p).sin().abs() * move_amp * BODY_BOB_MAX * 0.5 * stride + sink,
                    lean: (if self.gait == Gait::Run { 0.16 } else { 0.045 }) + hunch,
                    roll: p.sin() * 0.04 * move_amp * stride + tilt,
                    phase: self.phase,
                    weight: self.blend,
                    squash: (2.0 * p).sin().abs() * move_amp * 0.7 * stride,
                    leg_l: ll * stride,
                    leg_r: lr * stride,
                    // the arms come IN as the body folds up rather than
                    // swinging wide: a hurt body guards its middle
                    arm_l: -ll * k * stride,
                    arm_r: -lr * k * stride,
                }
            }
            // thrusters: the body hangs off them, nose up, feet trailing
            Gait::Boost => Pose {
                bob: 0.06,
                lean: -0.22,
                roll: 0.0,
                phase: self.phase,
                weight: self.blend,
                squash: 0.0,
                leg_l: 0.0,
                leg_r: 0.0,
                arm_l: 0.0,
                arm_r: 0.0,
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
                leg_l: 0.0,
                leg_r: 0.0,
                arm_l: 0.0,
                arm_r: 0.0,
            },
            // ARM-OVER-ARM: one arm reaches while the other pulls, legs
            // finding purchase half a stroke out of phase with the arms
            // on the same side -- ported from the C engine's own
            // `sinf(g_climb_ph)` alternation (`g_climbing` blends it in),
            // simplified to a single sine per limb pair since this port
            // has one climb phase, not a per-limb IK solve.
            Gait::Climb => {
                let s = p.sin();
                Pose {
                    bob: 0.0,
                    lean: 0.14, // into the wall
                    roll: 0.0,
                    phase: self.phase,
                    weight: self.blend,
                    squash: 0.0,
                    leg_l: -s * 0.35,
                    leg_r: s * 0.35,
                    arm_l: s * ARM_SWING_MAX,
                    arm_r: -s * ARM_SWING_MAX,
                }
            }
            // the impact beat: absorb, then recover over LAND_SEC
            Gait::Land => Pose {
                bob: -0.16 * (1.0 - self.phase),
                lean: 0.09 * (1.0 - self.phase),
                roll: 0.0,
                phase: self.phase,
                weight: self.blend,
                squash: 0.9 * (1.0 - self.phase),
                // knees absorb the landing
                leg_l: 0.5 * (1.0 - self.phase),
                leg_r: 0.5 * (1.0 - self.phase),
                arm_l: 0.0,
                arm_r: 0.0,
            },
            Gait::Idle => Pose {
                // a standing machine is not perfectly still
                bob: (self.phase * tau * 0.25).sin() * 0.006,
                lean: 0.0,
                roll: 0.0,
                phase: self.phase,
                weight: self.blend,
                squash: 0.0,
                leg_l: 0.0,
                leg_r: 0.0,
                arm_l: 0.0,
                arm_r: 0.0,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    /// A hurt body crouches, and a badly hurt one with a wall to hand
    /// hugs it — but only when the wall exists. A gait that hugs open air
    /// is a body cowering at nothing.
    #[test]
    fn damage_bends_the_body_and_a_wall_is_required_to_hug_one() {
        let step = |hurt: f32, wall: f32| {
            let mut l = super::Locomotion::new();
            l.condition(hurt, wall);
            let mut pose = l.update(0.016, 4.0, 0.0, true, false, false);
            for _ in 0..40 {
                pose = l.update(0.016, 4.0, 0.0, true, false, false);
            }
            (l.gait, pose)
        };
        let (whole, wp) = step(0.0, 1.0);
        assert_eq!(whole, super::Gait::Walk, "an undamaged body limped");
        let (hurt, hp) = step(0.5, 0.0);
        assert_eq!(hurt, super::Gait::Crouch, "a damaged body kept striding");
        assert!(hp.bob < wp.bob, "the crouch did not drop the hips");
        assert!(hp.leg_l.abs() < wp.leg_l.abs(), "the stride did not shorten");
        // badly hurt in the open: still a crouch, because there is nothing
        // to hug
        let (open, _) = step(0.9, 0.0);
        assert_eq!(open, super::Gait::Crouch, "it hugged open air");
        let (cover, cp) = step(0.9, 1.0);
        assert_eq!(cover, super::Gait::WallHug);
        assert!(cp.roll > 0.1, "it did not lean into the wall: {}", cp.roll);
        let (_, left) = step(0.9, -1.0);
        assert!(left.roll < -0.1, "it leaned away from a wall on the left");
    }

    /// The phase must carry across a gait change between two cyclic gaits,
    /// or a body that gets shot mid-stride puts the same foot down twice.
    #[test]
    fn getting_hurt_mid_stride_does_not_reset_the_foot() {
        let mut l = super::Locomotion::new();
        for _ in 0..30 {
            l.update(0.016, 5.0, 0.0, true, false, false);
        }
        let before = l.update(0.016, 5.0, 0.0, true, false, false).phase;
        l.condition(0.5, 0.0);
        let after = l.update(0.016, 5.0, 0.0, true, false, false).phase;
        assert_eq!(l.gait, super::Gait::Crouch);
        assert!(
            (after - before).abs() < 0.1,
            "the phase jumped on the gait change: {before} -> {after}"
        );
    }

    use super::*;

    fn run_for(l: &mut Locomotion, secs: f32, speed: f32, vert: f32, ground: bool, boost: bool) -> Pose {
        let mut p = Pose::default();
        for _ in 0..(secs * 60.0) as i32 {
            p = l.update(1.0 / 60.0, speed, vert, ground, boost, false);
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
    fn climbing_outranks_falling_and_boost() {
        let mut l = Locomotion::new();
        // airborne and descending fast enough to be a Fall on its own --
        // but climbing is a deliberate player action and must win anyway
        let mut p = Pose::default();
        for _ in 0..60 {
            p = l.update(1.0 / 60.0, 0.0, -20.0, false, false, true);
        }
        assert_eq!(l.gait, Gait::Climb, "falling/boost outranked an active climb");
        // arm-over-arm: the two arms move opposite each other, same as
        // the legs -- a body that reaches with both arms at once isn't
        // climbing, it's doing something else
        assert!(p.arm_l * p.arm_r <= 0.0, "arms did not oppose each other: {} vs {}", p.arm_l, p.arm_r);
        assert!(p.leg_l * p.leg_r <= 0.0, "legs did not oppose each other: {} vs {}", p.leg_l, p.leg_r);

        // and it lets go cleanly: once climbing stops, gait selection goes
        // back to normal on the very next update
        let p2 = l.update(1.0 / 60.0, 0.0, -20.0, false, false, false);
        let _ = p2;
        assert_eq!(l.gait, Gait::Fall, "did not fall back through to Fall once climbing ended");
    }

    #[test]
    fn landing_fires_once_and_recovers() {
        let mut l = Locomotion::new();
        run_for(&mut l, 0.5, 0.0, -20.0, false, false);
        // touch down
        let p = l.update(1.0 / 60.0, 0.0, 0.0, true, false, false);
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
        l.update(1.0 / 60.0, 20.0, 0.0, true, false, false);
        for _ in 0..90 {
            l.update(1.0 / 60.0, 20.0, 0.0, true, false, false);
        }
        // the phase must have CONTINUED, not reset — resetting puts a foot
        // down twice in a row, which is the classic transition tell
        assert!(l.gait == Gait::Run);
        let _ = before;
        let mut l2 = Locomotion::new();
        run_for(&mut l2, 2.0, 3.0, 0.0, true, false);
        let p0 = l2.phase;
        l2.update(1.0 / 60.0, 20.0, 0.0, true, false, false);
        assert!((l2.phase - p0).abs() < 0.2, "phase jumped on the walk->run change");
    }

    #[test]
    fn the_cycle_wraps_and_never_leaves_its_range() {
        let mut l = Locomotion::new();
        for _ in 0..2000 {
            let p = l.update(1.0 / 60.0, 5.0, 0.0, true, false, false);
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
            wd = wd.min(w.update(1.0 / 60.0, 3.0, 0.0, true, false, false).bob);
            rd = rd.min(r.update(1.0 / 60.0, 14.0, 0.0, true, false, false).bob);
        }
        assert!(rd < wd, "a run dips less than a walk ({rd} vs {wd})");
        assert!(r.pose().lean > w.pose().lean, "a run leans less than a walk");
    }

    #[test]
    fn cadence_follows_the_froude_relation_and_saturates() {
        // stride frequency rises with sqrt(speed), not with speed. Doubling
        // pace must NOT double the leg rate, or a sprint whirls.
        let a = stride_freq(4.0);
        let b = stride_freq(16.0);
        assert!((b / a - 2.0).abs() < 0.05, "4x the speed gave {:.2}x the cadence", b / a);
        // and it stops: no matter how fast, the legs cap out
        assert!((stride_freq(1e6) - GAIT_FREQ_MAX).abs() < 1e-3);
        assert_eq!(stride_freq(0.0), 0.0);
    }

    #[test]
    fn stride_length_takes_over_where_cadence_stops() {
        // the other half of the Froude model: the amplitude keeps growing
        // after the frequency has saturated, so speed becomes longer steps
        let slow = leg_swing(1.0, false, 3.0).abs();
        let fast = leg_swing(1.0, false, 9.0).abs();
        assert!(fast > slow, "stride did not lengthen with speed");
        // and it caps too, or the legs pass through each other
        assert!(leg_swing(1.0, false, 1e6).abs() <= 1.5 * LEG_SWING_MAX + 1e-4);
    }

    #[test]
    fn the_legs_are_half_a_cycle_apart_and_the_arms_oppose_them() {
        let mut l = Locomotion::new();
        run_for(&mut l, 2.0, 8.0, 0.0, true, false);
        let p = l.update(1.0 / 60.0, 8.0, 0.0, true, false, false);
        // opposite legs: a bot with both legs forward is falling over
        assert!(p.leg_l * p.leg_r <= 1e-6, "legs in phase: {} {}", p.leg_l, p.leg_r);
        // arms counter the legs, at the C engine's ratio
        assert!(p.arm_l * p.leg_l <= 1e-6, "left arm swings with its own leg");
        if p.leg_l.abs() > 0.05 {
            let ratio = (p.arm_l / p.leg_l).abs();
            assert!((ratio - ARM_SWING_MAX / LEG_SWING_MAX).abs() < 1e-4, "ratio {ratio}");
        }
    }

    #[test]
    fn idle_still_breathes_but_barely() {
        let mut l = Locomotion::new();
        let p = run_for(&mut l, 3.0, 0.0, 0.0, true, false);
        assert!(p.bob.abs() < 0.02, "a standing bot is bouncing: {}", p.bob);
        assert!(p.weight < 0.5, "idle applies full body motion");
    }
}
