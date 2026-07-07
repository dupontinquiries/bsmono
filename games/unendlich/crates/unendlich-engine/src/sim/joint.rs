//! Articulated joints whose motion *emerges* from a material's friction profile
//! instead of authored animation. The defining idea: friction is a function of
//! the joint's current angle. A "rusted" or "woody" joint has detents — angular
//! teeth where resistance spikes — so under a steady drive torque it doesn't sweep
//! smoothly; it breaks free in the valleys, decelerates into the next tooth, and
//! catches, lurching like a stiff creature working itself loose. A smooth joint
//! has flat friction and just sweeps. Same drive, different material → completely
//! different movement, with zero keyframes.
//!
//! This is stick-slip friction: static friction (the breakaway force, varying by
//! angle) holds the joint until drive exceeds it; once moving, lower kinetic
//! friction applies, and kinetic friction can only *stop* motion, never reverse
//! it. A tree-dragon's branches each get a [`JointMaterial`]; driving them
//! produces the creaking, unsticking motion for free.
//!
//! Deterministic fixed-point — rollback-safe.

use crate::sim::fixed::Fixed;

fn sign(x: Fixed) -> Fixed {
    if x > Fixed::ZERO {
        Fixed::ONE
    } else if x < Fixed::ZERO {
        Fixed::ZERO - Fixed::ONE
    } else {
        Fixed::ZERO
    }
}

/// The friction character of a joint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JointMaterial {
    /// Always-present resistance — the breakaway floor in a friction valley.
    pub base_friction: Fixed,
    /// Extra static friction at the detent peaks (the "teeth"). 0 = smooth.
    pub detent_amp: Fixed,
    /// Teeth per unit angle — higher is bumpier.
    pub detents: Fixed,
    /// Kinetic friction as a fraction of static (<1 → real stick-slip).
    pub kinetic_scale: Fixed,
    /// Velocity damping.
    pub damping: Fixed,
}

impl JointMaterial {
    /// A free-swinging joint: flat low friction, no detents.
    pub fn smooth() -> Self {
        JointMaterial {
            base_friction: Fixed::from_frac(1, 2),
            detent_amp: Fixed::ZERO,
            detents: Fixed::ONE,
            kinetic_scale: Fixed::from_frac(9, 10),
            damping: Fixed::from_frac(1, 10),
        }
    }

    /// A seized, rusted joint: high breakaway, deep detents — catches hard.
    pub fn rusted() -> Self {
        JointMaterial {
            base_friction: Fixed::from_int(2),
            detent_amp: Fixed::from_int(10),
            detents: Fixed::from_int(3),
            kinetic_scale: Fixed::from_frac(4, 5),
            damping: Fixed::ZERO,
        }
    }

    /// Periodic 0..1 bump: 0 in the valleys, 1 at the detent peaks.
    fn detent_bump(&self, angle: Fixed) -> Fixed {
        let phase = angle * self.detents;
        let floor = Fixed::from_int(phase.floor_int());
        let frac = phase - floor; // [0, 1)
        let two_frac = frac * Fixed::from_int(2);
        let t = two_frac - Fixed::ONE; // [-1, 1)
        Fixed::ONE - t.abs() // peak 1 at frac=0.5
    }

    /// Breakaway (static) friction at a given angle.
    pub fn static_friction(&self, angle: Fixed) -> Fixed {
        self.base_friction + self.detent_amp * self.detent_bump(angle)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Held by static friction; didn't move.
    Stuck,
    /// Broke free / kept moving this step.
    Slipping,
    /// Was moving, friction brought it to rest at a detent.
    Caught,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepOutcome {
    pub phase: Phase,
    /// This step the joint broke free from rest.
    pub broke_free: bool,
    /// This step kinetic friction halted it.
    pub caught: bool,
}

/// A single articulated joint.
#[derive(Debug, Clone, Copy)]
pub struct Joint {
    pub angle: Fixed,
    pub velocity: Fixed,
    pub material: JointMaterial,
}

impl Joint {
    pub fn new(material: JointMaterial, angle: Fixed) -> Self {
        Joint { angle, velocity: Fixed::ZERO, material }
    }

    /// Advance one step under `drive` torque over `dt`.
    pub fn step(&mut self, drive: Fixed, dt: Fixed) -> StepOutcome {
        let eps = Fixed::from_frac(1, 1000);
        let fs = self.material.static_friction(self.angle);
        let at_rest = self.velocity.abs() < eps;

        // Held: at rest and drive can't beat breakaway friction here.
        if at_rest && drive.abs() <= fs {
            self.velocity = Fixed::ZERO;
            return StepOutcome { phase: Phase::Stuck, broke_free: false, caught: false };
        }

        let broke_free = at_rest; // we were stopped and drive exceeded fs
        let fk = fs * self.material.kinetic_scale;
        let motion_dir = sign(if at_rest { drive } else { self.velocity });
        let friction = fk * motion_dir; // opposes the direction of travel
        let net = drive - friction - self.material.damping * self.velocity;
        let mut new_v = self.velocity + net * dt;

        // Kinetic friction can stop but not reverse: clamp a sign flip to rest.
        let mut caught = false;
        if !at_rest && sign(self.velocity) != Fixed::ZERO && sign(new_v) != sign(self.velocity) {
            new_v = Fixed::ZERO;
            caught = true;
        }
        self.velocity = new_v;
        self.angle = self.angle + self.velocity * dt;

        if self.velocity.abs() < eps && drive.abs() <= self.material.static_friction(self.angle) {
            self.velocity = Fixed::ZERO;
            return StepOutcome { phase: Phase::Caught, broke_free, caught: true };
        }
        StepOutcome { phase: Phase::Slipping, broke_free, caught }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dt() -> Fixed {
        Fixed::from_frac(1, 10)
    }

    #[test]
    fn friction_varies_with_angle_for_rusted_but_is_flat_for_smooth() {
        let r = JointMaterial::rusted();
        let valley = r.static_friction(Fixed::ZERO); // bump 0
        let peak = r.static_friction(Fixed::from_frac(1, 6)); // bump 1 (3 detents)
        assert!(peak > valley, "rusted: a tooth resists more than a valley: {peak:?} > {valley:?}");

        let s = JointMaterial::smooth();
        assert_eq!(
            s.static_friction(Fixed::ZERO),
            s.static_friction(Fixed::from_frac(1, 6)),
            "smooth friction is the same at every angle"
        );
    }

    #[test]
    fn the_same_drive_locks_on_a_tooth_but_breaks_free_in_a_valley() {
        // The headline: behaviour is a function of the joint's current angle.
        let mat = JointMaterial::rusted(); // valley static=2, peak static=12
        let drive = Fixed::from_int(5);

        let mut valley = Joint::new(mat, Fixed::ZERO);
        assert_ne!(valley.step(drive, dt()).phase, Phase::Stuck, "5 > valley(2) → frees");

        let mut tooth = Joint::new(mat, Fixed::from_frac(1, 6));
        assert_eq!(tooth.step(drive, dt()).phase, Phase::Stuck, "5 < tooth(12) → locked");
    }

    #[test]
    fn a_weak_drive_leaves_a_rusted_joint_locked() {
        let mut j = Joint::new(JointMaterial::rusted(), Fixed::ZERO);
        let weak = Fixed::ONE; // < base_friction 2
        for _ in 0..100 {
            assert_eq!(j.step(weak, dt()).phase, Phase::Stuck);
        }
        assert_eq!(j.angle, Fixed::ZERO, "never budged");
    }

    #[test]
    fn a_strong_drive_powers_through_the_teeth() {
        let mut j = Joint::new(JointMaterial::rusted(), Fixed::ZERO);
        let strong = Fixed::from_int(30); // well above peak static 12
        for _ in 0..50 {
            j.step(strong, dt());
        }
        assert!(j.angle > Fixed::ONE, "swept past several teeth: {:?}", j.angle);
    }

    #[test]
    fn kinetic_friction_never_drives_the_joint_backwards() {
        let mut j = Joint::new(JointMaterial::rusted(), Fixed::ZERO);
        let drive = Fixed::from_int(8);
        let neg_eps = Fixed::ZERO - Fixed::from_frac(1, 1000);
        for _ in 0..200 {
            j.step(drive, dt());
            assert!(j.velocity >= neg_eps, "friction stops, never reverses: {:?}", j.velocity);
        }
    }

    #[test]
    fn a_rusted_joint_ripples_while_a_smooth_one_sweeps_evenly() {
        let run = |mat: JointMaterial, drive: Fixed| -> Vec<Fixed> {
            let mut j = Joint::new(mat, Fixed::ZERO);
            (0..60).map(|_| { j.step(drive, dt()); j.velocity }).collect()
        };
        let dips = |v: &[Fixed]| v.windows(2).filter(|w| w[1] < w[0]).count();

        let drive = Fixed::from_int(8); // between rusted valley/peak kinetic
        let rusted = run(JointMaterial::rusted(), drive);
        let smooth = run(JointMaterial::smooth(), drive);
        assert!(dips(&rusted) >= 1, "the rusted joint decelerates at teeth (bumpy)");
        assert_eq!(dips(&smooth), 0, "the smooth joint only ever speeds up to terminal");
    }

    #[test]
    fn motion_is_deterministic() {
        let go = || {
            let mut j = Joint::new(JointMaterial::rusted(), Fixed::ZERO);
            for _ in 0..40 {
                j.step(Fixed::from_int(9), dt());
            }
            (j.angle, j.velocity)
        };
        assert_eq!(go(), go(), "same material + drive → identical trajectory");
    }
}

// ── service registration (S3) ──
use crate::sim::service_registry::{Service, ServiceCtx, ServiceEntry, SERVICES};
use linkme::distributed_slice;

struct JointService(Joint);
impl Service for JointService {
    fn name(&self) -> &'static str {
        "joint"
    }
    fn tick(&mut self, _ctx: &ServiceCtx) {
        // neutral: no drive torque; a fixed 1/60 s step (Fixed dt, not ctx.dt f32)
        let _ = self.0.step(Fixed::ZERO, Fixed::from_frac(1, 60));
    }
}
#[distributed_slice(SERVICES)]
static JOINT_SERVICE: ServiceEntry =
    ServiceEntry { name: "joint", make: || Box::new(JointService(Joint::new(JointMaterial::smooth(), Fixed::ZERO))) };
