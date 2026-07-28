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

/// Cross-section profile for a swept segment. The five cases are exactly the 1-5 keys of
/// `games/blackstar/tools/spine_grower.html` (circle, square, triangle, star, hex), so a spine
/// authored by that grammar means the same thing here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Section {
    #[default]
    Circle,
    Square,
    Triangle,
    Star,
    Hex,
}

impl Section {
    /// The grower's number keys, 1-5.
    pub fn from_digit(d: u32) -> Option<Section> {
        Some(match d {
            1 => Section::Circle,
            2 => Section::Square,
            3 => Section::Triangle,
            4 => Section::Star,
            5 => Section::Hex,
            _ => return None,
        })
    }

    /// 2D distance to the unit profile (circumradius 1) at `q`.
    ///
    /// Polygons are evaluated as the max over their edge half-planes. That is exact outside near a
    /// face and an underestimate near a vertex — which is the safe direction: the tracer and the
    /// mesher require distances that never exceed the truth.
    pub fn dist2d(self, q: Vec2) -> f32 {
        match self {
            Section::Circle => q.length() - 1.0,
            Section::Square => ngon(q, 4),
            Section::Triangle => ngon(q, 3),
            Section::Hex => ngon(q, 6),
            // Two squares at 45 degrees: their union is an eight-pointed star, and a min of two
            // lower bounds is still a lower bound.
            Section::Star => {
                let c = std::f32::consts::FRAC_PI_4.cos();
                let s = std::f32::consts::FRAC_PI_4.sin();
                ngon(q, 4).min(ngon(Vec2::new(q.x * c + q.y * s, -q.x * s + q.y * c), 4))
            }
        }
    }
}

/// Exact distance to a round cone: the convex hull of a sphere `ra` at `a` and `rb` at `b`.
/// Inigo Quilez's construction, which splits on which of the two caps or the tangent side wall is
/// nearest. Degenerate cases (one ball swallowing the other) fall back to the enclosing sphere.
fn round_cone(p: Vec3, a: Vec3, b: Vec3, ra: f32, rb: f32) -> f32 {
    let sign = |v: f32| if v > 0.0 { 1.0 } else if v < 0.0 { -1.0 } else { 0.0 };
    let ba = b - a;
    let l2 = ba.dot(ba);
    let rr = ra - rb;
    let a2 = l2 - rr * rr;
    if l2 < 1e-12 || a2 <= 0.0 {
        return ((p - a).length() - ra).min((p - b).length() - rb);
    }
    let il2 = 1.0 / l2;
    let pa = p - a;
    let y = pa.dot(ba);
    let z = y - l2;
    let x = pa * l2 - ba * y;
    let x2 = x.dot(x);
    let y2 = y * y * l2;
    let z2 = z * z * l2;
    let k = sign(rr) * rr * rr * x2;
    if sign(z) * a2 * z2 > k {
        (x2 + z2).sqrt() * il2 - rb
    } else if sign(y) * a2 * y2 < k {
        (x2 + y2).sqrt() * il2 - ra
    } else {
        ((x2 * a2 * il2).sqrt() + y * rr) * il2 - ra
    }
}

/// Regular n-gon of circumradius 1, as the max over its edge half-planes.
fn ngon(q: Vec2, n: u32) -> f32 {
    let an = std::f32::consts::PI / n as f32;
    let apothem = an.cos();
    let mut d = f32::NEG_INFINITY;
    for i in 0..n {
        let a = std::f32::consts::TAU * i as f32 / n as f32 + an;
        d = d.max(q.dot(Vec2::new(a.cos(), a.sin())) - apothem);
    }
    d
}

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
    /// A segment from `a` to `b` swept with `section`, its radius tapering `ra` → `rb`. `up`
    /// orients the profile about the axis, which is what carries a spine's roll and twist; it is
    /// stored per segment rather than derived, so a chain can hold a continuous frame instead of
    /// flipping wherever a locally-derived basis would.
    Swept { a: Vec3, b: Vec3, ra: f32, rb: f32, section: Section, up: Vec3 },
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
            Sdf::Swept { a, b, ra, rb, section, up } => {
                let ba = *b - *a;
                let len = ba.length();
                if len < 1e-6 {
                    // Degenerate segment: fall back to a ball, which is still a valid bound.
                    return (p - *a).length() - ra.max(*rb);
                }
                // Circular sections get the exact round-cone distance. This matters more than it
                // looks: a spine is a chain of *short* segments, and the generic extrusion form
                // below is dominated by its end-plane term over a short span, reporting a distance
                // an order of magnitude under the truth. Correct, but it would cost the sphere
                // tracer a step for every one it should take.
                if *section == Section::Circle {
                    return round_cone(p, *a, *b, *ra, *rb);
                }
                let u = ba / len;
                let pa = p - *a;
                let y = pa.dot(u);
                let r = (ra + (rb - ra) * (y / len).clamp(0.0, 1.0)).max(1e-5);
                // Profile frame: `up` projected perpendicular to the axis, with a guard for the
                // case where the author handed us an `up` parallel to the direction of travel.
                let mut e1 = *up - u * up.dot(u);
                if e1.length_squared() < 1e-8 {
                    e1 = if u.x.abs() < 0.9 { u.cross(Vec3::X) } else { u.cross(Vec3::Y) };
                }
                let e1 = e1.normalize_or(Vec3::X);
                let e2 = u.cross(e1);
                let perp = pa - u * y;
                let q = Vec2::new(perp.dot(e1), perp.dot(e2));
                let side = section.dist2d(q / r) * r;
                let ends = (-y).max(y - len);
                // Tapering scales the profile along the axis, which steepens the gradient; divide
                // through by the slope's Lipschitz factor so the result stays an underestimate.
                let slope = (ra - rb).abs() / len;
                side.max(ends) / (1.0 + slope * slope).sqrt()
            }
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
            // The profile's circumradius is 1, so nothing reaches beyond r from the axis.
            Sdf::Swept { a, b, ra, rb, .. } => (
                (*a - Vec3::splat(*ra)).min(*b - Vec3::splat(*rb)),
                (*a + Vec3::splat(*ra)).max(*b + Vec3::splat(*rb)),
            ),
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
            Sdf::Swept { a, b, ra, rb, .. } => (a.length() + ra).max(b.length() + rb),
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
pub fn swept(a: Vec3, b: Vec3, ra: f32, rb: f32, section: Section, up: Vec3) -> Sdf {
    Sdf::Swept { a, b, ra, rb, section, up }
}

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
