//! Fast collision primitives — the broad-phase API. These are the cheap
//! bounding checks you run *first* (overlap tests, point containment, ray and
//! segment picks) before any expensive narrow-phase resolution. Deliberately
//! allocation-free, branch-light, and `f32`: this is the query/culling layer,
//! not the authoritative `Fixed` physics in `sim::physics`.
//!
//! Shapes use the same `min`/`max` convention as `world::entity::Aabb` so they
//! interoperate. More refined primitives (OBB, capsule, swept tests, GJK) can
//! layer on top; these cover the 90% case: AABB-vs-AABB, sphere-vs-sphere,
//! sphere-vs-AABB, point containment, ray-AABB (picking / line-of-sight), and
//! segment-AABB (short sweeps).
//!
//! Everything here is pure and unit-tested.

#[inline]
fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Axis-aligned bounding box, `min`..`max` per axis.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

impl Aabb {
    pub fn new(min: [f32; 3], max: [f32; 3]) -> Self {
        Self { min, max }
    }

    /// From a center and per-axis half-extents.
    pub fn from_center_half(center: [f32; 3], half: [f32; 3]) -> Self {
        Self {
            min: [center[0] - half[0], center[1] - half[1], center[2] - half[2]],
            max: [center[0] + half[0], center[1] + half[1], center[2] + half[2]],
        }
    }

    pub fn center(&self) -> [f32; 3] {
        [
            0.5 * (self.min[0] + self.max[0]),
            0.5 * (self.min[1] + self.max[1]),
            0.5 * (self.min[2] + self.max[2]),
        ]
    }

    pub fn half_extents(&self) -> [f32; 3] {
        [
            0.5 * (self.max[0] - self.min[0]),
            0.5 * (self.max[1] - self.min[1]),
            0.5 * (self.max[2] - self.min[2]),
        ]
    }

    /// Point containment (inclusive of the surface).
    pub fn contains(&self, p: [f32; 3]) -> bool {
        (0..3).all(|i| p[i] >= self.min[i] && p[i] <= self.max[i])
    }

    /// AABB-vs-AABB overlap (inclusive — touching faces count as overlap).
    /// The classic 3-axis separating-axis test: boxes overlap iff they overlap
    /// on every axis.
    pub fn intersects(&self, other: &Aabb) -> bool {
        (0..3).all(|i| self.min[i] <= other.max[i] && self.max[i] >= other.min[i])
    }

    /// Grow the box by `margin` on every side (e.g. for a fat broad-phase
    /// proxy or a swept-motion bound). Negative shrinks.
    pub fn expanded(&self, margin: f32) -> Aabb {
        Aabb {
            min: [self.min[0] - margin, self.min[1] - margin, self.min[2] - margin],
            max: [self.max[0] + margin, self.max[1] + margin, self.max[2] + margin],
        }
    }

    /// Smallest AABB enclosing both — the BVH/merge operation.
    pub fn union(&self, other: &Aabb) -> Aabb {
        Aabb {
            min: [
                self.min[0].min(other.min[0]),
                self.min[1].min(other.min[1]),
                self.min[2].min(other.min[2]),
            ],
            max: [
                self.max[0].max(other.max[0]),
                self.max[1].max(other.max[1]),
                self.max[2].max(other.max[2]),
            ],
        }
    }

    /// Surface area — the cost metric for a Surface-Area-Heuristic BVH later.
    pub fn surface_area(&self) -> f32 {
        let d = sub(self.max, self.min);
        2.0 * (d[0] * d[1] + d[1] * d[2] + d[2] * d[0])
    }

    /// The point on or inside the box closest to `p` (clamp per axis). Equals
    /// `p` when `p` is inside.
    pub fn closest_point(&self, p: [f32; 3]) -> [f32; 3] {
        [
            p[0].clamp(self.min[0], self.max[0]),
            p[1].clamp(self.min[1], self.max[1]),
            p[2].clamp(self.min[2], self.max[2]),
        ]
    }

    /// Ray-vs-AABB via the slab method. `dir` need not be normalized; the
    /// returned value is the parameter `t` along `dir` at the entry point, in
    /// the same units as `dir`'s length. Returns `Some(0.0)` if the origin is
    /// already inside, and `None` if the ray misses or only hits behind the
    /// origin. Axis-parallel rays are handled via infinities from `1/dir`.
    pub fn ray(&self, origin: [f32; 3], dir: [f32; 3]) -> Option<f32> {
        let mut tmin = f32::NEG_INFINITY;
        let mut tmax = f32::INFINITY;
        for i in 0..3 {
            let inv = 1.0 / dir[i];
            let mut t1 = (self.min[i] - origin[i]) * inv;
            let mut t2 = (self.max[i] - origin[i]) * inv;
            if t1 > t2 {
                std::mem::swap(&mut t1, &mut t2);
            }
            // For a parallel ray outside the slab, t1/t2 are ±inf with the
            // origin outside → tmin becomes +inf or tmax -inf → rejected below.
            tmin = tmin.max(t1);
            tmax = tmax.min(t2);
        }
        if tmax < tmin.max(0.0) {
            return None; // no overlap, or the hit is entirely behind the origin
        }
        Some(tmin.max(0.0))
    }

    /// Segment (p0→p1) vs AABB — true if the finite segment touches the box.
    /// A short sweep / line-of-sight check.
    pub fn intersects_segment(&self, p0: [f32; 3], p1: [f32; 3]) -> bool {
        let dir = sub(p1, p0);
        match self.ray(p0, dir) {
            Some(t) => t <= 1.0, // dir spans the whole segment, so t in [0,1]
            None => false,
        }
    }

    /// Sphere-vs-AABB: the sphere overlaps the box iff the box's closest point
    /// is within `radius`.
    pub fn intersects_sphere(&self, s: &Sphere) -> bool {
        let c = self.closest_point(s.center);
        let d = sub(s.center, c);
        dot(d, d) <= s.radius * s.radius
    }
}

/// A bounding sphere — the cheapest overlap proxy of all.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sphere {
    pub center: [f32; 3],
    pub radius: f32,
}

impl Sphere {
    pub fn new(center: [f32; 3], radius: f32) -> Self {
        Self { center, radius }
    }

    pub fn contains(&self, p: [f32; 3]) -> bool {
        let d = sub(p, self.center);
        dot(d, d) <= self.radius * self.radius
    }

    /// Sphere-vs-sphere overlap (inclusive). Squared-distance compare — no sqrt.
    pub fn intersects(&self, other: &Sphere) -> bool {
        let d = sub(self.center, other.center);
        let r = self.radius + other.radius;
        dot(d, d) <= r * r
    }

    /// Convenience mirror of [`Aabb::intersects_sphere`].
    pub fn intersects_aabb(&self, a: &Aabb) -> bool {
        a.intersects_sphere(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unit() -> Aabb {
        Aabb::new([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])
    }

    #[test]
    fn aabb_overlap_cases() {
        let a = unit();
        assert!(a.intersects(&Aabb::new([0.5, 0.5, 0.5], [2.0, 2.0, 2.0])), "overlapping");
        assert!(a.intersects(&Aabb::new([1.0, 0.0, 0.0], [2.0, 1.0, 1.0])), "touching faces");
        assert!(!a.intersects(&Aabb::new([1.1, 0.0, 0.0], [2.0, 1.0, 1.0])), "gap on x");
        assert!(!a.intersects(&Aabb::new([0.0, 5.0, 0.0], [1.0, 6.0, 1.0])), "gap on y");
        // Containment is overlap.
        assert!(a.intersects(&Aabb::from_center_half([0.5, 0.5, 0.5], [0.1, 0.1, 0.1])));
    }

    #[test]
    fn point_containment_and_closest() {
        let a = unit();
        assert!(a.contains([0.5, 0.5, 0.5]));
        assert!(a.contains([0.0, 1.0, 0.5]), "on the surface");
        assert!(!a.contains([1.5, 0.5, 0.5]));
        // Closest point clamps to the box; inside returns the point itself.
        assert_eq!(a.closest_point([2.0, -1.0, 0.5]), [1.0, 0.0, 0.5]);
        assert_eq!(a.closest_point([0.3, 0.3, 0.3]), [0.3, 0.3, 0.3]);
    }

    #[test]
    fn center_half_roundtrip_and_metrics() {
        let a = Aabb::from_center_half([2.0, 3.0, 4.0], [1.0, 0.5, 2.0]);
        assert_eq!(a.center(), [2.0, 3.0, 4.0]);
        assert_eq!(a.half_extents(), [1.0, 0.5, 2.0]);
        // 2*(2*1 + 1*4 + 4*2) for dims (2,1,4): SA = 2*(2+4+8) = 28.
        assert!((a.surface_area() - 28.0).abs() < 1e-4);
        let u = unit().union(&Aabb::new([2.0, 2.0, 2.0], [3.0, 3.0, 3.0]));
        assert_eq!(u, Aabb::new([0.0, 0.0, 0.0], [3.0, 3.0, 3.0]));
        assert_eq!(unit().expanded(0.5), Aabb::new([-0.5, -0.5, -0.5], [1.5, 1.5, 1.5]));
    }

    #[test]
    fn ray_aabb_hit_miss_inside_behind() {
        let a = unit();
        // Straight hit from -x: entry at x=0, origin at -2 → t=2 along +x dir.
        assert_eq!(a.ray([-2.0, 0.5, 0.5], [1.0, 0.0, 0.0]), Some(2.0));
        // Miss: passes above the box.
        assert_eq!(a.ray([-2.0, 5.0, 0.5], [1.0, 0.0, 0.0]), None);
        // Origin inside → t = 0.
        assert_eq!(a.ray([0.5, 0.5, 0.5], [1.0, 0.0, 0.0]), Some(0.0));
        // Pointing away from the box → behind-only → None.
        assert_eq!(a.ray([-2.0, 0.5, 0.5], [-1.0, 0.0, 0.0]), None);
        // Diagonal hit corner region.
        assert!(a.ray([-1.0, -1.0, 0.5], [1.0, 1.0, 0.0]).is_some());
    }

    #[test]
    fn segment_aabb() {
        let a = unit();
        // Segment that crosses the box.
        assert!(a.intersects_segment([-1.0, 0.5, 0.5], [2.0, 0.5, 0.5]));
        // Segment that stops short of the box (ends before reaching it).
        assert!(!a.intersects_segment([-1.0, 0.5, 0.5], [-0.1, 0.5, 0.5]));
        // Segment entirely past/beside the box.
        assert!(!a.intersects_segment([-1.0, 5.0, 0.5], [2.0, 5.0, 0.5]));
    }

    #[test]
    fn sphere_overlaps() {
        let s = Sphere::new([0.0, 0.0, 0.0], 1.0);
        assert!(s.intersects(&Sphere::new([1.5, 0.0, 0.0], 1.0)), "spheres overlap");
        assert!(!s.intersects(&Sphere::new([3.0, 0.0, 0.0], 1.0)), "spheres apart");
        assert!(s.contains([0.5, 0.5, 0.5]));
        assert!(!s.contains([1.0, 1.0, 1.0]));
    }

    #[test]
    fn sphere_aabb_overlap() {
        let a = unit();
        // Sphere centered just off the +x face, within radius.
        assert!(a.intersects_sphere(&Sphere::new([1.2, 0.5, 0.5], 0.3)), "near the face");
        assert!(Sphere::new([1.2, 0.5, 0.5], 0.3).intersects_aabb(&a), "mirror API");
        // Just out of reach of the nearest corner.
        assert!(!a.intersects_sphere(&Sphere::new([2.0, 2.0, 2.0], 1.0)), "corner miss");
        // Center inside the box → always overlapping.
        assert!(a.intersects_sphere(&Sphere::new([0.5, 0.5, 0.5], 0.01)));
    }
}
