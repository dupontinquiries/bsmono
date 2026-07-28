//! ray.rs — sphere tracing against the SDF tree. This is M0's picking truth: because the viewport
//! draws meshes generated from the same primitives the tree evaluates, a ray hit here IS "what you
//! see under the finger" — no ID buffer needed until CSG surfaces render (M1; PARITY.md).
//!
//! Correctness leans on `Sdf::eval` never overestimating distance (the module's stated invariant):
//! each step advances by the evaluated distance, so the march can graze but never tunnel.

use glam::Vec3;

use crate::sdf::Sdf;

#[derive(Clone, Copy, Debug)]
pub struct Ray {
    pub origin: Vec3,
    /// Must be normalized; `Ray::new` does it for you.
    pub dir: Vec3,
}

impl Ray {
    pub fn new(origin: Vec3, dir: Vec3) -> Ray {
        Ray { origin, dir: dir.normalize_or(Vec3::NEG_Z) }
    }
    pub fn at(&self, t: f32) -> Vec3 {
        self.origin + self.dir * t
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub t: f32,
    pub pos: Vec3,
    pub normal: Vec3,
}

/// Surface-convergence threshold. Model units are mm; a micron is invisible and cheap.
const EPS: f32 = 1e-3;
const MAX_STEPS: usize = 256;

/// March `ray` against `sdf` up to `max_t`. Returns the first surface hit, or None.
/// A ray STARTING inside the solid reports t = 0 at the origin (useful for probes; the picker's
/// camera is always outside).
pub fn raycast(sdf: &Sdf, ray: &Ray, max_t: f32) -> Option<Hit> {
    let mut t = 0.0f32;
    for _ in 0..MAX_STEPS {
        let p = ray.at(t);
        let d = sdf.eval(p);
        if d < EPS {
            return Some(Hit { t, pos: p, normal: sdf.normal(p) });
        }
        t += d;
        if t > max_t {
            return None;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdf::{cuboid, sphere, subtract, translate, cylinder};

    #[test]
    fn hits_a_sphere_at_the_analytic_t() {
        let s = sphere(1.0);
        let r = Ray::new(Vec3::new(0.0, 0.0, 5.0), Vec3::NEG_Z);
        let h = raycast(&s, &r, 100.0).expect("hit");
        assert!((h.t - 4.0).abs() < 5e-3);
        assert!(h.normal.z > 0.99);
    }

    #[test]
    fn misses_cleanly() {
        let s = sphere(1.0);
        let r = Ray::new(Vec3::new(0.0, 3.0, 5.0), Vec3::NEG_Z);
        assert!(raycast(&s, &r, 100.0).is_none());
    }

    #[test]
    fn sees_through_a_drilled_hole() {
        // Ray down the axis of a subtracted cylinder must pass through the block.
        let block = subtract(cuboid(Vec3::splat(2.0)), cylinder(0.5, 3.0));
        let r = Ray::new(Vec3::new(0.0, 6.0, 0.0), Vec3::NEG_Y);
        assert!(raycast(&block, &r, 12.0).is_none(), "hole should be open");
        // ...but a ray offset onto the meat hits the top face at y=2.
        let r2 = Ray::new(Vec3::new(1.2, 6.0, 0.0), Vec3::NEG_Y);
        let h = raycast(&block, &r2, 12.0).expect("face hit");
        assert!((h.pos.y - 2.0).abs() < 5e-3);
        assert!(h.normal.y > 0.99);
    }

    #[test]
    fn translated_target() {
        let s = translate(Vec3::new(10.0, 0.0, 0.0), sphere(2.0));
        let r = Ray::new(Vec3::ZERO, Vec3::X);
        let h = raycast(&s, &r, 100.0).expect("hit");
        assert!((h.t - 8.0).abs() < 5e-3);
    }
}
