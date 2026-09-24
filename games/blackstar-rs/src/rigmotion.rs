//! RIG MOTION — the bridge from the gait machine to a driven 22-bone rig.
//!
//! `anim.rs` decides WHICH gait a robot is in and computes body motion, but
//! its own notes say it: the limb channels are unconsumed, because the bodies
//! are pose strips, not rigs. `boneforge::motion` is the rig side -- walk and
//! run by Froude number, jumps from leg force and gravity, foot IK that keeps
//! planted feet planted and knees from bending back, aimed arms, weapon hands,
//! ranged fire. This module feeds it the engine's own truth, once per frame:
//!
//! * SPEED sets the gait. The walk <-> run setting is solved from the actual
//!   ground speed, Fr = v^2 / (g L), so the stride matches how fast the body
//!   really moves -- planted feet stay planted -- and at the same speed a
//!   low-gravity world gives a higher Froude number: the lope.
//! * GRAVITY is the engine's: `World::gravity_field` x the arena setting x
//!   `physics::GRAV`. Units match: the engine is metres (100 uu = 1 m) and
//!   the rig centimetres, so everything crosses by x100.
//! * DAMAGE is `sections::Body::limp()`: its SIGN picks the leg (positive is
//!   the RIGHT leg hurt), its size the strength.
//! * The engine owns WHERE the body is; the rig's own travel is removed.

use boneforge::bot;
use boneforge::motion::{self, Clip, Params, Side, Skeleton};
use boneforge::rig::{Joint, Pose as RigPose};

use crate::anim::Gait;

/// One frame of the driven rig.
///
/// Not yet read by the renderer -- the bodies are still pose strips; skinning
/// a bot to these joints is the next step -- hence the allow.
#[allow(dead_code)]
#[derive(Clone, Debug)]
pub struct Frame {
    pub clip: Clip,
    pub pose: RigPose,
    pub joints: Vec<Joint>,
    /// height above the grounded pose (a flight arc, a hover bob), in metres
    pub air: f32,
    /// the walk <-> run setting that produced it (0..1)
    pub gait: f32,
    pub info: String,
}

pub struct RigMotion {
    rig: Skeleton,
    stand: RigPose,
    par: Params,
    /// accumulated gait phase in cycles, so the stride does not jump when the
    /// speed (and with it the cycle length) changes
    phase: f32,
    clock: f32,
    last: Clip,
}

/// Froude number for walk (slider 0) and run (slider 1), log-spaced between.
const FR_WALK: f32 = 0.25;
const FR_RUN: f32 = 1.6;

/// The walk <-> run setting whose speed is `speed_cm` at this gravity.
pub fn gait_for_speed(speed_cm: f32, g_cm: f32, leg_cm: f32) -> f32 {
    let fr = speed_cm * speed_cm / (g_cm.max(1.0) * leg_cm.max(1.0));
    ((fr.max(1e-6) / FR_WALK).ln() / (FR_RUN / FR_WALK).ln()).clamp(0.0, 1.0)
}

impl RigMotion {
    /// A rig for a robot `height_m` tall.
    pub fn new(height_m: f32) -> RigMotion {
        let h = (height_m * 100.0).max(50.0);
        let bones = bot::skeleton(h);
        let stand = bot::stand(&bones, h);
        let leg = bones.iter().filter(|b| b.name == "thigh_l" || b.name == "shin_l").map(|b| b.len).sum::<f32>();
        RigMotion { rig: Skeleton(bones), stand, par: Params { leg_len: leg, ..Params::default() },
                    phase: 0.0, clock: 0.0, last: Clip::Idle }
    }

    /// Advance one frame. `speed` and `vert` are the engine's (m/s), `g_mul`
    /// its gravity multiplier at the body, `limp` is `Body::limp()`.
    pub fn update(&mut self, dt: f32, gait: Gait, speed: f32, g_mul: f32, limp: f32) -> Frame {
        let dt = if dt.is_finite() { dt.max(0.0) } else { 0.0 };
        let speed_cm = if speed.is_finite() { speed.max(0.0) * 100.0 } else { 0.0 };
        let g_cm = crate::physics::GRAV * 100.0 * if g_mul.is_finite() { g_mul.max(0.0) } else { 1.0 };
        self.par.g = g_cm.max(0.02 * 981.0);
        let side = if limp >= 0.0 { Side::R } else { Side::L };
        self.par.limp = (side, if limp.is_finite() { limp.abs().min(1.0) } else { 0.0 });
        let zero_g = g_cm < 0.05 * 981.0;
        let clip = if zero_g { Clip::Float } else {
            match gait {
                Gait::Walk | Gait::Run => Clip::Locomote,
                Gait::Boost => Clip::Glide,
                Gait::Fall => Clip::Float,
                Gait::Idle | Gait::Land | Gait::Climb | Gait::Crouch | Gait::WallHug => Clip::Idle,
            }
        };
        if clip != self.last { self.clock = 0.0; self.last = clip; }
        self.clock += dt;
        let t = if clip == Clip::Locomote {
            self.par.gait = gait_for_speed(speed_cm, self.par.g, self.par.leg_len);
            // advance the PHASE at this speed's cycle, then ask for the time
            // that gives it -- a changing cycle cannot make the stride jump
            let froude = FR_WALK * (FR_RUN / FR_WALK).powf(self.par.gait);
            let e = { let x = self.par.gait; x * x * (3.0 - 2.0 * x) };
            let cyc = motion::gait(&self.par, froude, 0.6 + (0.34 - 0.6) * e).cycle;
            self.phase = (self.phase + dt / cyc.max(1e-3)).rem_euclid(1000.0);
            self.phase * cyc
        } else { self.clock };
        let out = motion::pose(clip, t, &self.stand, &self.rig, &self.par);
        let mut pose = out.pose;
        pose.root_pos = self.stand.root_pos;                // the engine owns the position
        let joints = motion::Rig::solve(&self.rig, &pose);
        Frame { clip, pose, joints, air: out.air / 100.0, gait: self.par.gait, info: out.info }
    }

    #[allow(dead_code)]
    pub fn params(&self) -> &Params { &self.par }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finite(f: &Frame) -> bool {
        f.joints.iter().all(|j| j.head.iter().chain(j.tail.iter()).all(|v| v.is_finite())) && f.air.is_finite()
    }

    #[test]
    fn gait_follows_the_engines_state() {
        let mut r = RigMotion::new(3.9);
        assert_eq!(r.update(0.016, Gait::Idle, 0.0, 1.0, 0.0).clip, Clip::Idle);
        assert_eq!(r.update(0.016, Gait::Walk, 2.0, 1.0, 0.0).clip, Clip::Locomote);
        assert_eq!(r.update(0.016, Gait::Boost, 8.0, 1.0, 0.0).clip, Clip::Glide);
        assert_eq!(r.update(0.016, Gait::Fall, 0.0, 1.0, 0.0).clip, Clip::Float);
        assert_eq!(r.update(0.016, Gait::Walk, 2.0, 0.0, 0.0).clip, Clip::Float, "zero gravity floats");
    }

    #[test]
    fn faster_is_further_toward_a_run_and_low_gravity_lopes() {
        let mut r = RigMotion::new(3.9);
        let slow = r.update(0.016, Gait::Walk, 1.5, 1.0, 0.0).gait;
        let fast = r.update(0.016, Gait::Run, 5.0, 1.0, 0.0).gait;
        assert!(fast > slow, "5 m/s is further toward a run than 1.5 m/s");
        // the SAME speed at a fifth of the gravity is a higher Froude number
        let moon = r.update(0.016, Gait::Run, 1.5, 0.2, 0.0).gait;
        assert!(moon > slow, "same speed, lower gravity: a lope ({moon} vs {slow})");
    }

    #[test]
    fn the_stride_does_not_jump_when_the_speed_changes() {
        let mut r = RigMotion::new(3.9);
        let mut prev = r.update(0.016, Gait::Walk, 1.5, 1.0, 0.0);
        let mut worst: f32 = 0.0;
        for i in 1..240 {
            let v = 1.5 + 3.5 * (i as f32 / 240.0);          // speeding up smoothly
            let f = r.update(0.016, Gait::Run, v, 1.0, 0.0);
            let fa = &f.joints[r.rig.0.iter().position(|b| b.name == "foot_l").unwrap()];
            let fb = &prev.joints[r.rig.0.iter().position(|b| b.name == "foot_l").unwrap()];
            worst = worst.max(((fa.head[0] - fb.head[0]).powi(2) + (fa.head[1] - fb.head[1]).powi(2)).sqrt());
            assert!(finite(&f));
            prev = f;
        }
        assert!(worst < 25.0, "an ankle jumped {worst} units in one frame");
    }

    #[test]
    fn damage_picks_the_leg_by_its_sign() {
        let mut r = RigMotion::new(3.9);
        let knee = |r: &mut RigMotion, limp: f32, sd: &str| -> f32 {
            let i = r.rig.0.iter().position(|b| b.name == format!("shin_{sd}")).unwrap();
            (0..120).map(|_| r.update(0.016, Gait::Run, 5.0, 1.0, limp).pose.angles[i][2]).fold(f32::MAX, f32::min)
        };
        assert!(knee(&mut r, 0.0, "l") >= 0.0 && knee(&mut r, 0.0, "r") >= 0.0, "healthy knees never pass straight");
        assert!(knee(&mut r, 0.8, "r") < -5.0, "positive limp() is the RIGHT leg");
        assert!(knee(&mut r, -0.8, "l") < -5.0, "negative limp() is the LEFT leg");
    }

    #[test]
    fn hostile_inputs_give_an_honest_frame() {
        let mut r = RigMotion::new(3.9);
        for (dt, v, g) in [(f32::NAN, 2.0, 1.0), (-0.016, 2.0, 1.0), (0.016, f32::INFINITY, 1.0), (0.016, 2.0, f32::NAN)] {
            assert!(finite(&r.update(dt, Gait::Walk, v, g, 0.0)), "dt {dt} v {v} g {g}");
        }
    }
}
