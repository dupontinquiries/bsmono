//! Axis-aligned bounding boxes (2D + 3D), in the soldier math.rs vocabulary
//! (`empty`/`expand`/`center`/`corners`) plus the containment/overlap tests
//! unendlich's octree/frustum code re-derives.

use crate::vec::{v2, v3, Vec2, Vec3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb2 {
    pub min: Vec2,
    pub max: Vec2,
}

impl Aabb2 {
    /// Inverted-infinite box: expanding it with the first point makes it that point.
    pub fn empty() -> Aabb2 {
        Aabb2 {
            min: Vec2::splat(f32::INFINITY),
            max: Vec2::splat(f32::NEG_INFINITY),
        }
    }
    pub fn new(min: Vec2, max: Vec2) -> Aabb2 {
        Aabb2 { min, max }
    }
    pub fn from_points(pts: impl IntoIterator<Item = Vec2>) -> Aabb2 {
        let mut b = Aabb2::empty();
        for p in pts {
            b.expand(p);
        }
        b
    }
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y
    }
    pub fn expand(&mut self, p: Vec2) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }
    pub fn union(&self, o: &Aabb2) -> Aabb2 {
        Aabb2 { min: self.min.min(o.min), max: self.max.max(o.max) }
    }
    pub fn center(&self) -> Vec2 {
        (self.min + self.max) * 0.5
    }
    pub fn size(&self) -> Vec2 {
        self.max - self.min
    }
    /// Closed containment (boundary counts as inside).
    pub fn contains(&self, p: Vec2) -> bool {
        p.x >= self.min.x && p.x <= self.max.x && p.y >= self.min.y && p.y <= self.max.y
    }
    /// Closed overlap (touching edges count).
    pub fn intersects(&self, o: &Aabb2) -> bool {
        self.min.x <= o.max.x && self.max.x >= o.min.x && self.min.y <= o.max.y && self.max.y >= o.min.y
    }
    /// Grow (or shrink, if negative) by `d` on every side.
    pub fn inflated(&self, d: f32) -> Aabb2 {
        Aabb2 { min: self.min - Vec2::splat(d), max: self.max + Vec2::splat(d) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb3 {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb3 {
    /// Inverted-infinite box: expanding it with the first point makes it that point.
    pub fn empty() -> Aabb3 {
        Aabb3 {
            min: Vec3::splat(f32::INFINITY),
            max: Vec3::splat(f32::NEG_INFINITY),
        }
    }
    pub fn new(min: Vec3, max: Vec3) -> Aabb3 {
        Aabb3 { min, max }
    }
    pub fn from_points(pts: impl IntoIterator<Item = Vec3>) -> Aabb3 {
        let mut b = Aabb3::empty();
        for p in pts {
            b.expand(p);
        }
        b
    }
    pub fn is_empty(&self) -> bool {
        self.min.x > self.max.x || self.min.y > self.max.y || self.min.z > self.max.z
    }
    pub fn expand(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }
    pub fn union(&self, o: &Aabb3) -> Aabb3 {
        Aabb3 { min: self.min.min(o.min), max: self.max.max(o.max) }
    }
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
    pub fn size(&self) -> Vec3 {
        self.max - self.min
    }
    /// Closed containment (boundary counts as inside).
    pub fn contains(&self, p: Vec3) -> bool {
        p.x >= self.min.x
            && p.x <= self.max.x
            && p.y >= self.min.y
            && p.y <= self.max.y
            && p.z >= self.min.z
            && p.z <= self.max.z
    }
    /// Closed overlap (touching faces count).
    pub fn intersects(&self, o: &Aabb3) -> bool {
        self.min.x <= o.max.x
            && self.max.x >= o.min.x
            && self.min.y <= o.max.y
            && self.max.y >= o.min.y
            && self.min.z <= o.max.z
            && self.max.z >= o.min.z
    }
    /// The 8 corners, min-corner first, x fastest then y then z.
    pub fn corners(&self) -> [Vec3; 8] {
        let (a, b) = (self.min, self.max);
        [
            v3(a.x, a.y, a.z),
            v3(b.x, a.y, a.z),
            v3(a.x, b.y, a.z),
            v3(b.x, b.y, a.z),
            v3(a.x, a.y, b.z),
            v3(b.x, a.y, b.z),
            v3(a.x, b.y, b.z),
            v3(b.x, b.y, b.z),
        ]
    }
    /// Grow (or shrink, if negative) by `d` on every side.
    pub fn inflated(&self, d: f32) -> Aabb3 {
        Aabb3 { min: self.min - Vec3::splat(d), max: self.max + Vec3::splat(d) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_then_expand_is_the_point() {
        let mut b = Aabb3::empty();
        assert!(b.is_empty());
        b.expand(v3(1.0, 2.0, 3.0));
        assert!(!b.is_empty());
        assert_eq!(b.min, b.max);
        assert_eq!(b.center(), v3(1.0, 2.0, 3.0));
        b.expand(v3(-1.0, 4.0, 3.0));
        assert_eq!(b.size(), v3(2.0, 2.0, 0.0));
    }

    #[test]
    fn containment_overlap_and_corners() {
        let b = Aabb3::new(v3(0.0, 0.0, 0.0), v3(2.0, 2.0, 2.0));
        assert!(b.contains(v3(2.0, 2.0, 2.0))); // closed boundary
        assert!(!b.contains(v3(2.1, 1.0, 1.0)));
        let o = Aabb3::new(v3(2.0, 0.0, 0.0), v3(3.0, 1.0, 1.0));
        assert!(b.intersects(&o)); // touching faces count
        assert!(!b.intersects(&Aabb3::new(v3(5.0, 5.0, 5.0), v3(6.0, 6.0, 6.0))));
        let cs = b.corners();
        assert_eq!(cs[0], v3(0.0, 0.0, 0.0));
        assert_eq!(cs[7], v3(2.0, 2.0, 2.0));
        // all corners are contained
        assert!(cs.iter().all(|&c| b.contains(c)));
    }

    #[test]
    fn from_points_and_2d() {
        let b = Aabb2::from_points([v2(1.0, 5.0), v2(-2.0, 3.0), v2(0.0, 7.0)]);
        assert_eq!(b.min, v2(-2.0, 3.0));
        assert_eq!(b.max, v2(1.0, 7.0));
        assert!(b.inflated(1.0).contains(v2(-3.0, 8.0)));
        assert_eq!(b.union(&Aabb2::new(v2(9.0, 9.0), v2(10.0, 10.0))).max, v2(10.0, 10.0));
    }
}
