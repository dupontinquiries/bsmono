//! CYBERTRONIAN JOINTS — not a pin through two plates.
//!
//! The measurement that led here: on `racer_grit`, 71 of 78 fitted joints
//! have a pivot more than a quarter of the smaller joined part away from
//! that part. Blackstar 37/46, dawnshade 47/51. When ninety per cent of your
//! joints are wrong, the problem is usually not ninety per cent of your
//! joints — it is the model of what a joint IS.
//!
//! A shared-axis pin is unforgiving by construction: the pivot must sit
//! exactly on the seam, because anything else opens a gap the instant the
//! joint moves. Nothing that has to survive being shot at is built that way.
//! Real machines put a boot over the hinge; real bodies put tissue across
//! it; and a Cybertronian, which is both, would do more.
//!
//! So a joint here has three parts, and each one earns its place by solving
//! something the pin cannot:
//!
//!   * **ROTATOR** — the load path. A hinge with a little axial travel, so
//!     it settles under weight. Not much: this is a bearing that carries a
//!     tonne, not a spring. But the small amount matters, because a leg that
//!     compresses a centimetre when the foot lands is a leg that reads as
//!     carrying something, and one that does not reads as a puppet.
//!
//!   * **MUSCLE** — compressive actuator in the piece itself rather than at
//!     the joint. It contracts to drive the rotation, and it SHORTENS the
//!     part slightly while it does. That is the tell that separates a
//!     powered limb from an animated one: under torque the segment thickens
//!     and draws in, and when the torque goes it lengthens again.
//!
//!   * **SLEEVE** — the connective span across the seam, and the practical
//!     one. It stretches to cover whatever distance is between the two
//!     pieces, which means a pivot that is 30% off does not TEAR, it just
//!     stretches its sleeve. This is what turns a measurement of ninety per
//!     cent broken into a question of how much slack to allow.
//!
//! That last point is the whole argument. Fixing 71 pivots by hand is a
//! week of work that has to be redone for every new import. Giving joints
//! tolerance and something to span it fixes the class.

use glam::Vec3;

/// A load-bearing rotator: a hinge with suspension.
#[derive(Clone, Copy, Debug)]
pub struct Rotator {
    /// hinge axis, unit
    pub axis: Vec3,
    /// how far it can settle along the axis, in metres. Small on purpose.
    pub travel: f32,
    /// newtons per metre of travel
    pub stiffness: f32,
    /// 0..1 — how fast oscillation dies. A joint that rings is a broken one.
    pub damping: f32,
}

impl Default for Rotator {
    fn default() -> Rotator {
        Rotator { axis: Vec3::X, travel: 0.018, stiffness: 90_000.0, damping: 0.72 }
    }
}

impl Rotator {
    /// How far it settles under this load, in metres. Clamped to `travel`:
    /// past that it is metal on metal and stops.
    pub fn settle(&self, load_n: f32) -> f32 {
        (load_n.max(0.0) / self.stiffness).min(self.travel)
    }
    /// True when the rotator is bottomed out — worth a sound and a jolt.
    pub fn bottomed(&self, load_n: f32) -> bool {
        load_n.max(0.0) / self.stiffness >= self.travel
    }
}

/// A compressive actuator inside a segment.
#[derive(Clone, Copy, Debug)]
pub struct Muscle {
    /// length at rest, metres
    pub rest: f32,
    /// most it can shorten, as a fraction of rest
    pub max_contract: f32,
    /// how much torque it takes to fully contract, N·m
    pub torque_full: f32,
}

impl Default for Muscle {
    fn default() -> Muscle {
        Muscle { rest: 1.0, max_contract: 0.06, torque_full: 2_400.0 }
    }
}

impl Muscle {
    /// Length under this torque. Shortens as it works.
    pub fn length(&self, torque_nm: f32) -> f32 {
        let f = (torque_nm.abs() / self.torque_full).clamp(0.0, 1.0);
        self.rest * (1.0 - self.max_contract * f)
    }
    /// Cross-section swell, as a scale factor — volume is conserved, so what
    /// it loses in length it gains around the middle. This is the thing that
    /// makes a limb look like it is pulling rather than rotating.
    pub fn girth(&self, torque_nm: f32) -> f32 {
        let l = self.length(torque_nm) / self.rest.max(1e-6);
        (1.0 / l).sqrt()
    }
}

/// The span across the seam. This is the part that forgives a bad pivot.
#[derive(Clone, Copy, Debug)]
pub struct Sleeve {
    /// radius of the boot, metres
    pub radius: f32,
    /// how far it can stretch beyond its rest span before it has failed
    pub max_stretch: f32,
    /// rest span between the two anchors
    pub rest_span: f32,
}

impl Default for Sleeve {
    fn default() -> Sleeve {
        Sleeve { radius: 0.06, max_stretch: 0.55, rest_span: 0.12 }
    }
}

impl Sleeve {
    /// Can it cover a seam this wide?
    pub fn covers(&self, span: f32) -> bool {
        span <= self.rest_span * (1.0 + self.max_stretch)
    }
    /// 0 at rest, 1 at the limit. Past 1 the joint is visibly torn.
    pub fn strain(&self, span: f32) -> f32 {
        let slack = self.rest_span * self.max_stretch;
        if slack <= 1e-6 {
            return 0.0;
        }
        ((span - self.rest_span) / slack).max(0.0)
    }
    /// It thins as it stretches, because it is a tube of fixed material.
    pub fn radius_at(&self, span: f32) -> f32 {
        let s = 1.0 + self.strain(span) * self.max_stretch;
        self.radius / s.sqrt()
    }
}

/// A whole joint.
#[derive(Clone, Copy, Debug, Default)]
pub struct CyberJoint {
    pub pivot: Vec3,
    pub rotator: Rotator,
    pub muscle: Muscle,
    pub sleeve: Sleeve,
}

/// What a joint is doing this frame, for the renderer and the audio.
#[derive(Clone, Copy, Debug, Default)]
pub struct JointState {
    /// axial settle under load, metres
    pub settle: f32,
    /// segment length after contraction
    pub segment_len: f32,
    /// cross-section scale
    pub girth: f32,
    /// seam width the sleeve is covering
    pub span: f32,
    /// 0..1 sleeve strain; >1 means the sleeve has failed and it LOOKS torn
    pub strain: f32,
    pub bottomed: bool,
}

impl CyberJoint {
    /// Build one from a fitted pivot and the two things it joins.
    ///
    /// `parent_near` and `child_near` are the distances from the pivot to the
    /// nearest geometry of each side — exactly what `foldview --joints`
    /// reports. The sleeve is sized to SPAN them, so a badly placed pivot
    /// produces a longer boot rather than a hole.
    pub fn fitted(pivot: Vec3, axis: Vec3, parent_near: f32, child_near: f32, part_size: f32) -> CyberJoint {
        let span = parent_near + child_near;
        CyberJoint {
            pivot,
            rotator: Rotator {
                axis: axis.normalize_or(Vec3::X),
                travel: (part_size * 0.02).clamp(0.004, 0.05),
                ..Rotator::default()
            },
            muscle: Muscle { rest: part_size.max(0.05), ..Muscle::default() },
            sleeve: Sleeve {
                // sized to what it actually has to cover, with headroom
                rest_span: span.max(part_size * 0.06),
                radius: (part_size * 0.12).max(0.01),
                max_stretch: 0.55,
            },
        }
    }

    pub fn solve(&self, load_n: f32, torque_nm: f32, seam_span: f32) -> JointState {
        JointState {
            settle: self.rotator.settle(load_n),
            segment_len: self.muscle.length(torque_nm),
            girth: self.muscle.girth(torque_nm),
            span: seam_span,
            strain: self.sleeve.strain(seam_span),
            bottomed: self.rotator.bottomed(load_n),
        }
    }

    /// Is this joint presentable — i.e. will it read as connected?
    pub fn intact(&self, seam_span: f32) -> bool {
        self.sleeve.covers(seam_span)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_rotator_settles_under_load_and_then_stops() {
        let r = Rotator::default();
        assert_eq!(r.settle(0.0), 0.0);
        let light = r.settle(1_000.0);
        let heavy = r.settle(9_000.0);
        assert!(heavy > light, "more weight did not sink it further");
        // and it bottoms out rather than sinking forever — it is a bearing,
        // not a pogo stick
        assert!((r.settle(1e9) - r.travel).abs() < 1e-6);
        assert!(r.bottomed(1e9) && !r.bottomed(0.0));
        // the travel is small: a leg dips a centimetre, it does not squat
        assert!(r.travel < 0.03);
    }

    #[test]
    fn a_muscle_shortens_as_it_works_and_thickens_as_it_shortens() {
        let m = Muscle::default();
        assert!((m.length(0.0) - m.rest).abs() < 1e-6);
        let worked = m.length(m.torque_full);
        assert!(worked < m.rest, "it did not contract");
        assert!((worked - m.rest * (1.0 - m.max_contract)).abs() < 1e-5);
        // volume conserved: shorter means fatter
        assert!(m.girth(m.torque_full) > m.girth(0.0));
        assert!((m.girth(0.0) - 1.0).abs() < 1e-6);
        // and it cannot contract past its limit however hard it is driven
        assert!((m.length(1e9) - worked).abs() < 1e-5);
    }

    #[test]
    fn a_sleeve_covers_a_seam_a_pin_would_tear() {
        let s = Sleeve { rest_span: 0.10, max_stretch: 0.55, radius: 0.05 };
        assert!(s.covers(0.10), "it cannot even cover its own rest span");
        assert!(s.covers(0.15), "a 50% gap tore a sleeve rated for 55%");
        assert!(!s.covers(0.30), "a sleeve covered three times its span");
        // strain reads 0 at rest and 1 at the limit
        assert!(s.strain(0.10).abs() < 1e-6);
        assert!((s.strain(0.155) - 1.0).abs() < 0.05);
        // and it thins as it pulls, because it is a tube of fixed material
        assert!(s.radius_at(0.15) < s.radius_at(0.10));
    }

    #[test]
    fn a_badly_placed_pivot_gets_a_longer_boot_not_a_hole() {
        // THE POINT. foldview reports pivot-to-geometry distances; the joint
        // is built to SPAN them, so a 30%-off pivot produces a longer sleeve
        // rather than a gap.
        let part = 0.40;
        let good = CyberJoint::fitted(Vec3::ZERO, Vec3::X, 0.005, 0.005, part);
        let bad = CyberJoint::fitted(Vec3::ZERO, Vec3::X, 0.12, 0.10, part);
        assert!(bad.sleeve.rest_span > good.sleeve.rest_span * 3.0);
        // and both read as connected at the seam they were built for
        assert!(good.intact(0.01));
        assert!(bad.intact(0.22), "the fitted sleeve could not cover its own seam");
    }

    #[test]
    fn a_seam_far_past_the_sleeve_is_honestly_reported_as_torn() {
        // tolerance is not the same as pretending. Past the stretch limit it
        // says so, which is what a repair tool would sort on.
        let j = CyberJoint::fitted(Vec3::ZERO, Vec3::X, 0.01, 0.01, 0.4);
        assert!(!j.intact(2.0));
        assert!(j.solve(0.0, 0.0, 2.0).strain > 1.0);
    }

    #[test]
    fn a_bigger_part_gets_a_bigger_everything() {
        let small = CyberJoint::fitted(Vec3::ZERO, Vec3::X, 0.002, 0.002, 0.05);
        let large = CyberJoint::fitted(Vec3::ZERO, Vec3::X, 0.002, 0.002, 2.0);
        assert!(large.rotator.travel > small.rotator.travel);
        assert!(large.muscle.rest > small.muscle.rest);
        assert!(large.sleeve.radius > small.sleeve.radius);
        // but travel stays sane at both ends: a finger does not sink 4 cm
        // and a leg does not need to
        assert!(small.rotator.travel >= 0.004 && large.rotator.travel <= 0.05);
    }

    #[test]
    fn solving_gives_the_renderer_everything_at_once() {
        let j = CyberJoint::fitted(Vec3::ZERO, Vec3::Y, 0.02, 0.02, 0.5);
        let idle = j.solve(0.0, 0.0, j.sleeve.rest_span);
        let working = j.solve(8_000.0, 2_400.0, j.sleeve.rest_span * 1.2);
        assert_eq!(idle.settle, 0.0);
        assert!(working.settle > 0.0, "loaded joint did not settle");
        assert!(working.segment_len < idle.segment_len, "loaded muscle did not contract");
        assert!(working.girth > idle.girth);
        assert!(working.strain > idle.strain);
    }
}
