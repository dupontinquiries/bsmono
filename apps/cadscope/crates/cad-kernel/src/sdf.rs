//! sdf.rs — the f-rep truth. Every CADscope solid is an analytic signed distance tree: exact
//! primitives at the leaves, CSG (and smooth *Blend*, our fillet-reading operator) at the nodes,
//! rigid transforms stored INVERTED so evaluation is a straight fold with no per-eval inversions.
//!
//! Two invariants this module promises (and tests):
//!   • `eval` is an exact distance for primitives and hard CSG of disjoint-interior operands, and a
//!     LOWER-BOUNDED distance everywhere else (never overestimates) — which is precisely what the
//!     sphere-tracer in `ray.rs` and the mesher (M1) require to be correct.
//!   • `normal` is the normalized central-difference gradient, defined everywhere `eval` is.
//!
//! This is skydribble's "every shape is both a mesh instance and an analytic SDF" promoted from a
//! collision trick to the modeling kernel. Deformers (taper/twist/bend) join in M1 with explicit
//! Lipschitz compensation; see CADSCOPE_PROMPT.md §5.

use glam::{Quat, Vec2, Vec3};

/// A signed-distance tree. Distances are in model units (mm by convention — `cad-forge` documents it).
#[derive(Clone, Debug, PartialEq)]
pub enum Sdf {
    /// Sphere of radius `r`, centered at the origin.
    Sphere { r: f32 },
    /// Axis-aligned box with half-extents `half`.
    Cuboid { half: Vec3 },
    /// Y-axis cylinder: radius `r`, half-height `hh`.
    Cylinder { r: f32, hh: f32 },
    /// Y-axis capsule: radius `r`, cylindrical half-height `hh` (total height `2*hh + 2*r`).
    Capsule { r: f32, hh: f32 },
    /// Torus around +Y: `major` ring radius, `minor` tube radius.
    Torus { major: f32, minor: f32 },
    /// Translate the child by `off`.
    Translate { off: Vec3, node: Box<Sdf> },
    /// Rotate the child; `inv` is the INVERSE rotation (stored inverted, applied to the sample point).
    Rotate { inv: Quat, node: Box<Sdf> },
    /// Hard union (min).
    Union { a: Box<Sdf>, b: Box<Sdf> },
    /// Hard intersection (max). Lower bound, exact on the common boundary.
    Intersect { a: Box<Sdf>, b: Box<Sdf> },
    /// Hard difference a − b (max(a, −b)). Lower bound, exact on the surviving surface.
    Subtract { a: Box<Sdf>, b: Box<Sdf> },
    /// Smooth union with blend radius `k` — the operator the UI sells as *Blend* (fillet-ish).
    /// Polynomial smooth-min; bounded by `min(a,b)` from above and `min(a,b) − k/4` from below.
    Blend { a: Box<Sdf>, b: Box<Sdf>, k: f32 },
    /// Rounded shell of thickness `t` around the child's surface (|d| − t): hollowing in one node.
    Shell { t: f32, node: Box<Sdf> },
}

impl Sdf {
    /// Signed distance from `p` to the solid: negative inside, positive outside.
    pub fn eval(&self, p: Vec3) -> f32 {
        match self {
            Sdf::Sphere { r } => p.length() - r,
            Sdf::Cuboid { half } => {
                let q = p.abs() - *half;
                q.max(Vec3::ZERO).length() + q.x.max(q.y.max(q.z)).min(0.0)
            }
            Sdf::Cylinder { r, hh } => {
                let d = Vec2::new(Vec2::new(p.x, p.z).length() - r, p.y.abs() - hh);
                d.x.max(d.y).min(0.0) + d.max(Vec2::ZERO).length()
            }
            Sdf::Capsule { r, hh } => {
                let y = p.y.clamp(-hh, *hh);
                (p - Vec3::new(0.0, y, 0.0)).length() - r
            }
            Sdf::Torus { major, minor } => {
                let q = Vec2::new(Vec2::new(p.x, p.z).length() - major, p.y);
                q.length() - minor
            }
            Sdf::Translate { off, node } => node.eval(p - *off),
            Sdf::Rotate { inv, node } => node.eval(*inv * p),
            Sdf::Union { a, b } => a.eval(p).min(b.eval(p)),
            Sdf::Intersect { a, b } => a.eval(p).max(b.eval(p)),
            Sdf::Subtract { a, b } => a.eval(p).max(-b.eval(p)),
            Sdf::Blend { a, b, k } => {
                let (da, db) = (a.eval(p), b.eval(p));
                if *k <= 0.0 {
                    return da.min(db);
                }
                let h = (0.5 + 0.5 * (db - da) / k).clamp(0.0, 1.0);
                db + (da - db) * h - k * h * (1.0 - h)
            }
            Sdf::Shell { t, node } => node.eval(p).abs() - t,
        }
    }

    /// Outward surface normal at `p` via central differences of `eval`.
    pub fn normal(&self, p: Vec3) -> Vec3 {
        // Step scaled a touch above f32 noise at CAD magnitudes; the tracer converges to |d|<1e-3,
        // so 5e-4 keeps the stencil symmetric around the located surface.
        const E: f32 = 5e-4;
        let g = Vec3::new(
            self.eval(p + Vec3::X * E) - self.eval(p - Vec3::X * E),
            self.eval(p + Vec3::Y * E) - self.eval(p - Vec3::Y * E),
            self.eval(p + Vec3::Z * E) - self.eval(p - Vec3::Z * E),
        );
        g.normalize_or(Vec3::Y)
    }

    /// Conservative axis-aligned bounds. Unlike `bound_radius` this tracks placement, so the
    /// mesher can grid only where the solid actually is — the difference between meshing a bracket
    /// and meshing the empty cube of space it was translated into.
    pub fn aabb(&self) -> (Vec3, Vec3) {
        match self {
            Sdf::Sphere { r } => (Vec3::splat(-r), Vec3::splat(*r)),
            Sdf::Cuboid { half } => (-*half, *half),
            Sdf::Cylinder { r, hh } => (Vec3::new(-r, -hh, -r), Vec3::new(*r, *hh, *r)),
            Sdf::Capsule { r, hh } => (Vec3::new(-r, -(hh + r), -r), Vec3::new(*r, hh + r, *r)),
            Sdf::Torus { major, minor } => {
                let x = major + minor;
                (Vec3::new(-x, -minor, -x), Vec3::new(x, *minor, x))
            }
            Sdf::Translate { off, node } => {
                let (lo, hi) = node.aabb();
                (lo + *off, hi + *off)
            }
            Sdf::Rotate { inv, node } => {
                // Rotate the child's eight corners and re-bound. `inv` is stored inverted, so the
                // forward rotation is its inverse.
                let (lo, hi) = node.aabb();
                let rot = inv.inverse();
                let (mut nlo, mut nhi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
                for c in 0..8 {
                    let p = Vec3::new(
                        if c & 1 == 0 { lo.x } else { hi.x },
                        if c & 2 == 0 { lo.y } else { hi.y },
                        if c & 4 == 0 { lo.z } else { hi.z },
                    );
                    let q = rot * p;
                    nlo = nlo.min(q);
                    nhi = nhi.max(q);
                }
                (nlo, nhi)
            }
            Sdf::Union { a, b } => {
                let (al, ah) = a.aabb();
                let (bl, bh) = b.aabb();
                (al.min(bl), ah.max(bh))
            }
            Sdf::Blend { a, b, k } => {
                // Smooth-min pulls the surface outward by at most k/4, and the fillet fairing can
                // reach a little further where the operands meet; k is the honest allowance.
                let (al, ah) = a.aabb();
                let (bl, bh) = b.aabb();
                (al.min(bl) - Vec3::splat(*k), ah.max(bh) + Vec3::splat(*k))
            }
            Sdf::Intersect { a, b } => {
                let (al, ah) = a.aabb();
                let (bl, bh) = b.aabb();
                (al.max(bl), ah.min(bh))
            }
            Sdf::Subtract { a, .. } => a.aabb(),
            Sdf::Shell { t, node } => {
                let (lo, hi) = node.aabb();
                (lo - Vec3::splat(*t), hi + Vec3::splat(*t))
            }
        }
    }

    /// Loose bounding radius from the origin — enough for the tracer's far clip and `frame`.
    /// Conservative by construction (sums where it must); tightness is not a promise.
    pub fn bound_radius(&self) -> f32 {
        match self {
            Sdf::Sphere { r } => *r,
            Sdf::Cuboid { half } => half.length(),
            Sdf::Cylinder { r, hh } | Sdf::Capsule { r, hh } => (r * r + hh * hh).sqrt() + r,
            Sdf::Torus { major, minor } => major + minor,
            Sdf::Translate { off, node } => off.length() + node.bound_radius(),
            Sdf::Rotate { node, .. } => node.bound_radius(),
            Sdf::Union { a, b } | Sdf::Blend { a, b, .. } => a.bound_radius().max(b.bound_radius()),
            Sdf::Intersect { a, b } => a.bound_radius().min(b.bound_radius()),
            Sdf::Subtract { a, .. } => a.bound_radius(),
            Sdf::Shell { t, node } => node.bound_radius() + t,
        }
    }
}

// ── ergonomic constructors (the vocabulary cad-forge ops speak) ───────────────────────────────
pub fn sphere(r: f32) -> Sdf { Sdf::Sphere { r } }
pub fn cuboid(half: Vec3) -> Sdf { Sdf::Cuboid { half } }
pub fn cylinder(r: f32, hh: f32) -> Sdf { Sdf::Cylinder { r, hh } }
pub fn capsule(r: f32, hh: f32) -> Sdf { Sdf::Capsule { r, hh } }
pub fn torus(major: f32, minor: f32) -> Sdf { Sdf::Torus { major, minor } }
pub fn translate(off: Vec3, node: Sdf) -> Sdf { Sdf::Translate { off, node: Box::new(node) } }
pub fn rotate(rot: Quat, node: Sdf) -> Sdf { Sdf::Rotate { inv: rot.inverse(), node: Box::new(node) } }
pub fn union(a: Sdf, b: Sdf) -> Sdf { Sdf::Union { a: Box::new(a), b: Box::new(b) } }
pub fn intersect(a: Sdf, b: Sdf) -> Sdf { Sdf::Intersect { a: Box::new(a), b: Box::new(b) } }
pub fn subtract(a: Sdf, b: Sdf) -> Sdf { Sdf::Subtract { a: Box::new(a), b: Box::new(b) } }
pub fn blend(a: Sdf, b: Sdf, k: f32) -> Sdf { Sdf::Blend { a: Box::new(a), b: Box::new(b), k } }
pub fn shell(t: f32, node: Sdf) -> Sdf { Sdf::Shell { t, node: Box::new(node) } }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sphere_is_exact() {
        let s = sphere(2.0);
        assert!((s.eval(Vec3::new(5.0, 0.0, 0.0)) - 3.0).abs() < 1e-6);
        assert!((s.eval(Vec3::ZERO) + 2.0).abs() < 1e-6);
    }

    #[test]
    fn cuboid_faces_edges_inside() {
        let b = cuboid(Vec3::new(1.0, 2.0, 3.0));
        assert!((b.eval(Vec3::new(4.0, 0.0, 0.0)) - 3.0).abs() < 1e-6); // face distance
        let corner = b.eval(Vec3::new(2.0, 3.0, 4.0)); // 1 away on each axis
        assert!((corner - 3f32.sqrt()).abs() < 1e-5);
        assert!(b.eval(Vec3::ZERO) < 0.0); // inside is negative
    }

    #[test]
    fn subtract_carves() {
        // A 1-radius hole through a 2-half cube: origin was inside, now it's outside (in the hole).
        let solid = subtract(cuboid(Vec3::splat(2.0)), cylinder(1.0, 3.0));
        assert!(solid.eval(Vec3::ZERO) > 0.0);
        assert!(solid.eval(Vec3::new(1.6, 0.0, 0.0)) < 0.0); // meat between hole and face survives
    }

    #[test]
    fn blend_is_a_bounded_underestimate_of_min() {
        let a = sphere(1.0);
        let b = translate(Vec3::new(1.5, 0.0, 0.0), sphere(1.0));
        let k = 0.4;
        let s = blend(a.clone(), b.clone(), k);
        for i in 0..80 {
            let p = Vec3::new(-2.0 + i as f32 * 0.05 * 2.2, 0.3, 0.1);
            let hard = a.eval(p).min(b.eval(p));
            let soft = s.eval(p);
            assert!(soft <= hard + 1e-6, "smooth-min above min at {p:?}");
            assert!(soft >= hard - k / 4.0 - 1e-6, "smooth-min below its bound at {p:?}");
        }
    }

    #[test]
    fn normals_are_unit_and_outward() {
        let s = translate(Vec3::new(0.0, 1.0, 0.0), sphere(1.0));
        let p = Vec3::new(0.0, 2.0, 0.0); // top of the sphere
        let n = s.normal(p);
        assert!((n.length() - 1.0).abs() < 1e-3);
        assert!(n.y > 0.99);
    }

    #[test]
    fn rotate_stores_inverse_and_round_trips() {
        let rot = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let s = rotate(rot, cuboid(Vec3::new(2.0, 0.5, 0.5)));
        // The long X axis rotated to +Y: a point far along Y should be near the surface now.
        assert!(s.eval(Vec3::new(0.0, 2.0, 0.0)).abs() < 1e-4);
        assert!(s.eval(Vec3::new(2.0, 0.0, 0.0)) > 1.0);
    }

    #[test]
    fn aabb_tracks_placement_and_contains_the_surface() {
        let s = translate(Vec3::new(10.0, 0.0, 0.0), sphere(2.0));
        let (lo, hi) = s.aabb();
        assert!((lo.x - 8.0).abs() < 1e-5 && (hi.x - 12.0).abs() < 1e-5, "{lo:?} {hi:?}");
        // Sample the box faces: nothing outside the box may be inside the solid.
        for i in 0..40 {
            let t = i as f32 / 39.0;
            let p = Vec3::new(lo.x - 0.01, lo.y + (hi.y - lo.y) * t, 0.0);
            assert!(s.eval(p) > 0.0);
        }
    }

    #[test]
    fn bound_radius_contains_the_surface() {
        let s = translate(Vec3::new(3.0, 0.0, 0.0), torus(2.0, 0.5));
        let r = s.bound_radius();
        // Sample the far rim; it must be inside the bound.
        assert!(r >= 5.5 - 1e-4);
    }
}
