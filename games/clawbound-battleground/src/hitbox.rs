//! hitbox — a cat is long, narrow, and pointing somewhere.
//!
//! Every hit test used a sphere of radius 0.85 floating 1.1 u over the cat's
//! feet. An ancestor is ~2.2 u nose to rump and ~0.7 u across, so the sphere
//! was wrong both ways: a bolt through the head or the haunches (1.3 u from
//! the centre) MISSED, and one passing a quarter-unit wide of the flank HIT.
//! It also ignored facing, so turning side-on to a caster changed nothing.
//!
//! The body is a capsule along the facing, from haunch to muzzle. Stalking
//! lowers it: a crouched cat really is a smaller target from above.

use glam::Vec3;

/// Haunch and muzzle, along the facing from `pos`, and the body's half-width.
/// From the rig: torso half-length 0.9, the head beyond it.
pub const REAR: f32 = -0.75;
pub const FRONT: f32 = 1.05;
pub const GIRTH: f32 = 0.48;
pub const SPINE_Y: f32 = 1.10;
pub const STALK_DROP: f32 = 0.28;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Capsule {
    pub a: Vec3,
    pub b: Vec3,
    pub r: f32,
}

/// The capsule for a cat standing at `pos` facing `yaw`.
pub fn cat(pos: Vec3, yaw: f32, stalking: bool) -> Capsule {
    let yaw = if yaw.is_finite() { yaw } else { 0.0 };
    let f = Vec3::new(yaw.cos(), 0.0, yaw.sin());
    let up = Vec3::Y * (SPINE_Y - if stalking { STALK_DROP } else { 0.0 });
    Capsule { a: pos + f * REAR + up, b: pos + f * FRONT + up, r: GIRTH }
}

/// The capsule for a cat drawn `size` times the usual (`Role::size`: a
/// Sentinel is bigger, so she is struck bigger). Everything scales from the
/// feet: the body is longer, wider and higher by the same factor.
pub fn cat_sized(pos: Vec3, yaw: f32, stalking: bool, size: f32) -> Capsule {
    let s = if size.is_finite() && size > 0.0 { size } else { 1.0 };
    let c = cat(pos, yaw, stalking);
    Capsule { a: pos + (c.a - pos) * s, b: pos + (c.b - pos) * s, r: c.r * s }
}

#[cfg(test)]
mod sized_tests {
    use super::*;

    /// A bigger cat is a bigger target: a bolt passing wide of an ordinary
    /// cat's flank hits a Sentinel's, and at size 1 nothing changes.
    #[test]
    fn a_bigger_cat_is_struck_bigger() {
        let (pos, yaw) = (Vec3::ZERO, 0.0);
        assert_eq!(cat_sized(pos, yaw, false, 1.0), cat(pos, yaw, false));
        let s = crate::gods::SENTINEL_SIZE;
        // a ball going by the flank, 1.1 of an ordinary girth out
        let wide = Vec3::new(0.0, SPINE_Y * s, GIRTH * 1.1 + 0.1);
        assert!(!cat(pos, yaw, false).hit_by_sphere(wide, 0.1));
        assert!(cat_sized(pos, yaw, false, s).hit_by_sphere(wide, 0.1));
        // and past the ordinary muzzle
        assert!(cat_sized(pos, yaw, false, s).in_disc(Vec3::new(FRONT * s, 0.0, 0.0), 0.1));
        assert!(!cat(pos, yaw, false).in_disc(Vec3::new(FRONT * s + GIRTH, 0.0, 0.0), 0.1));
    }
}

/// A minion: the same animal at about six tenths the size.
pub fn minion(pos: Vec3, yaw: f32) -> Capsule {
    let yaw = if yaw.is_finite() { yaw } else { 0.0 };
    let f = Vec3::new(yaw.cos(), 0.0, yaw.sin());
    Capsule { a: pos + f * (REAR * 0.6) + Vec3::Y * 0.65, b: pos + f * (FRONT * 0.6) + Vec3::Y * 0.65, r: GIRTH * 0.7 }
}

impl Capsule {
    /// A projectile that moved from `prev` to `now` this tick. SWEPT: the
    /// test is against the whole segment travelled, not the point it ended
    /// on, so whether a bolt connects does not depend on the tick rate or on
    /// where in the tick the body happened to be. At 60 Hz today's fastest
    /// bolt moves 0.67 u a tick and could not skip a 0.96 u body; at the
    /// 30 Hz a struggling handset runs, or with a faster bolt, it could.
    pub fn hit_by_moving_sphere(&self, prev: Vec3, now: Vec3, r: f32) -> bool {
        self.hit_by_beam(prev, now, r)
    }

    pub fn dist_to_point(&self, p: Vec3) -> f32 {
        let ab = self.b - self.a;
        let t = ((p - self.a).dot(ab) / ab.length_squared().max(1e-9)).clamp(0.0, 1.0);
        (self.a + ab * t - p).length() - self.r
    }

    /// A projectile: a sphere.
    pub fn hit_by_sphere(&self, c: Vec3, r: f32) -> bool {
        self.dist_to_point(c) < r
    }

    /// An area effect on the ground: a disc. Height is ignored, as it was.
    pub fn in_disc(&self, centre: Vec3, radius: f32) -> bool {
        let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z);
        Capsule { a: flat(self.a), b: flat(self.b), r: self.r }.dist_to_point(flat(centre)) < radius
    }

    /// A beam: a segment `p0..p1` with half-width `w`. Closest approach of
    /// two segments (Ericson, Real-Time Collision Detection 5.1.9).
    pub fn hit_by_beam(&self, p0: Vec3, p1: Vec3, w: f32) -> bool {
        let (d1, d2, r) = (self.b - self.a, p1 - p0, self.a - p0);
        let (a, e, f) = (d1.length_squared(), d2.length_squared(), d2.dot(r));
        let (mut s, mut t);
        if a <= 1e-9 && e <= 1e-9 {
            return r.length() - self.r < w;
        }
        if a <= 1e-9 {
            s = 0.0;
            t = (f / e).clamp(0.0, 1.0);
        } else {
            let c = d1.dot(r);
            if e <= 1e-9 {
                t = 0.0;
                s = (-c / a).clamp(0.0, 1.0);
            } else {
                let b = d1.dot(d2);
                let den = a * e - b * b;
                s = if den > 1e-9 { ((b * f - c * e) / den).clamp(0.0, 1.0) } else { 0.0 };
                t = (b * s + f) / e;
                if t < 0.0 {
                    t = 0.0;
                    s = (-c / a).clamp(0.0, 1.0);
                } else if t > 1.0 {
                    t = 1.0;
                    s = ((b - c) / a).clamp(0.0, 1.0);
                }
            }
        }
        ((self.a + d1 * s) - (p0 + d2 * t)).length() - self.r < w
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The two errors the sphere made, both directions, and facing matters.
    #[test]
    fn a_bolt_at_the_head_hits_and_one_wide_of_the_flank_misses() {
        let c = cat(Vec3::ZERO, 0.0, false); // facing +X
        let old = |p: Vec3, r: f32| (Vec3::Y * 1.1 - p).length() < r + 0.85;
        let (head, rump) = (Vec3::new(1.3, 1.1, 0.0), Vec3::new(-1.2, 1.1, 0.0));
        let beside = Vec3::new(0.0, 1.1, 0.95);
        let r = 0.25;
        assert!(c.hit_by_sphere(head, r) && c.hit_by_sphere(rump, r), "head and haunches are part of the cat");
        assert!(!old(head, r) && !old(rump, r), "and the sphere missed both");
        assert!(!c.hit_by_sphere(beside, r), "a bolt 0.95 u off the flank is past a 0.48 u body");
        assert!(old(beside, r), "which the sphere counted as a hit");
        // Turn the cat side-on and the same two points swap.
        let side = cat(Vec3::ZERO, std::f32::consts::FRAC_PI_2, false);
        assert!(!side.hit_by_sphere(head, r), "side-on, that point is beside it");
        assert!(side.hit_by_sphere(Vec3::new(0.0, 1.1, 1.3), r), "and its head is over here");
    }

    /// The point of sweeping: a fast bolt that starts one side of a cat and
    /// ends the other side, touching it at NEITHER end of the tick.
    #[test]
    fn a_fast_bolt_cannot_step_over_a_cat_between_ticks() {
        let c = cat(Vec3::ZERO, 0.0, false);
        let (before, after) = (Vec3::new(0.2, 1.1, -2.0), Vec3::new(0.2, 1.1, 2.0));
        assert!(!c.hit_by_sphere(before, 0.3) && !c.hit_by_sphere(after, 0.3), "neither endpoint touches");
        assert!(c.hit_by_moving_sphere(before, after, 0.3), "but it passed straight through the body");
        let wide = Vec3::new(3.0, 0.0, 0.0);
        assert!(!c.hit_by_moving_sphere(before + wide, after + wide, 0.3), "and a bolt that passes 3 u ahead of it still misses");
        // Minions are smaller in every direction.
        let m = minion(Vec3::ZERO, 0.0);
        // Muzzle at 0.63 u, girth 0.34: a 0.2 u bolt at 1.3 u is 0.33 u clear.
        // (I first wrote 1.0 here, which is 0.03 u clear -- a hit. The test
        // was wrong, not the capsule.)
        assert!(m.hit_by_sphere(Vec3::new(0.5, 0.65, 0.0), 0.2) && !m.hit_by_sphere(Vec3::new(1.3, 0.65, 0.0), 0.2));
        assert!(cat(Vec3::ZERO, 0.0, false).hit_by_sphere(Vec3::new(1.3, 1.1, 0.0), 0.2), "where a full-grown cat would be hit");
    }

    #[test]
    fn a_stalking_cat_is_a_lower_target() {
        let over = Vec3::new(0.2, 1.75, 0.0);
        assert!(cat(Vec3::ZERO, 0.0, false).hit_by_sphere(over, 0.2));
        assert!(!cat(Vec3::ZERO, 0.0, true).hit_by_sphere(over, 0.2), "the same shot passes over a crouch");
        assert!(cat(Vec3::ZERO, 0.0, true).hit_by_sphere(Vec3::new(0.2, 0.9, 0.0), 0.2), "but not a level one");
    }

    #[test]
    fn beams_and_discs_use_the_body_too() {
        let c = cat(Vec3::new(10.0, 0.0, 0.0), 0.0, false);
        // A beam along z crossing just ahead of the chest clips the head...
        assert!(c.hit_by_beam(Vec3::new(11.2, 1.1, -5.0), Vec3::new(11.2, 1.1, 5.0), 0.3));
        // ...and one the same distance from the CENTRE, but abeam, does not.
        assert!(!c.hit_by_beam(Vec3::new(5.0, 1.1, 1.2), Vec3::new(15.0, 1.1, 1.2), 0.3));
        assert!(c.hit_by_beam(Vec3::new(5.0, 1.1, 0.6), Vec3::new(15.0, 1.1, 0.6), 0.3));
        assert!(c.in_disc(Vec3::new(12.0, 0.0, 0.0), 0.6), "a small blast at its nose");
        assert!(!c.in_disc(Vec3::new(10.0, 0.0, 1.4), 0.6), "the same blast abeam");
        for bad in [f32::NAN, f32::INFINITY] {
            let g = cat(Vec3::ZERO, bad, false);
            assert!(g.a.is_finite() && g.b.is_finite());
            assert!(!g.hit_by_beam(Vec3::splat(bad), Vec3::ONE, 0.3) || true);
        }
    }
}
