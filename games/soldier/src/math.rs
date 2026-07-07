//! math.rs — a tiny 3D vector type and helpers.
//!
//! Mirrors `logscope::spatial::Vec3` (the reference renderer's vector), widened with the operations an
//! FPS needs: length, normalize, lerp, and a 2D ground-plane projection for movement/collision on the XZ
//! grid. Everything is `f32` and `Copy`; no external deps.

use std::ops::{Add, Mul, Neg, Sub};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 { x: 0.0, y: 0.0, z: 0.0 };
    pub const UP: Vec3 = Vec3 { x: 0.0, y: 1.0, z: 0.0 };

    #[inline]
    pub const fn new(x: f32, y: f32, z: f32) -> Self {
        Self { x, y, z }
    }
    #[inline]
    pub fn splat(s: f32) -> Self {
        Self { x: s, y: s, z: s }
    }
    #[inline]
    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    #[inline]
    pub fn cross(self, o: Vec3) -> Vec3 {
        Vec3::new(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    #[inline]
    pub fn len_sq(self) -> f32 {
        self.dot(self)
    }
    #[inline]
    pub fn len(self) -> f32 {
        self.len_sq().sqrt()
    }
    /// Normalized, or +Y if degenerate (matches the reference's `norm`).
    #[inline]
    pub fn normalized(self) -> Vec3 {
        let l = self.len();
        if l > 1e-6 {
            self * (1.0 / l)
        } else {
            Vec3::UP
        }
    }
    #[inline]
    pub fn lerp(self, o: Vec3, t: f32) -> Vec3 {
        self + (o - self) * t
    }
    #[inline]
    pub fn with_y(self, y: f32) -> Vec3 {
        Vec3::new(self.x, y, self.z)
    }
    /// Flatten onto the XZ ground plane (used by movement & collision, which are 2D on the tile grid).
    #[inline]
    pub fn xz(self) -> Vec2 {
        Vec2 { x: self.x, y: self.z }
    }
    #[inline]
    pub fn distance(self, o: Vec3) -> f32 {
        (self - o).len()
    }
}

impl Add for Vec3 {
    type Output = Vec3;
    #[inline]
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x + o.x, self.y + o.y, self.z + o.z)
    }
}
impl Sub for Vec3 {
    type Output = Vec3;
    #[inline]
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::new(self.x - o.x, self.y - o.y, self.z - o.z)
    }
}
impl Mul<f32> for Vec3 {
    type Output = Vec3;
    #[inline]
    fn mul(self, s: f32) -> Vec3 {
        Vec3::new(self.x * s, self.y * s, self.z * s)
    }
}
impl Neg for Vec3 {
    type Output = Vec3;
    #[inline]
    fn neg(self) -> Vec3 {
        Vec3::new(-self.x, -self.y, -self.z)
    }
}

/// A 2D vector for ground-plane (XZ) math: movement, collision, the minimap.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}
impl Vec2 {
    #[inline]
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
    #[inline]
    pub fn len(self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
    #[inline]
    pub fn dist(self, o: Vec2) -> f32 {
        Vec2::new(self.x - o.x, self.y - o.y).len()
    }
    /// Back to a ground-plane Vec3 at height `y`.
    #[inline]
    pub fn to_v3(self, y: f32) -> Vec3 {
        Vec3::new(self.x, y, self.y)
    }
}

#[inline]
pub fn clampf(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cross_is_right_handed() {
        // x cross y = z
        let r = Vec3::new(1.0, 0.0, 0.0).cross(Vec3::new(0.0, 1.0, 0.0));
        assert!((r - Vec3::new(0.0, 0.0, 1.0)).len() < 1e-6);
    }

    #[test]
    fn normalize_unit_and_degenerate() {
        let n = Vec3::new(0.0, 0.0, 5.0).normalized();
        assert!((n.len() - 1.0).abs() < 1e-6);
        // zero vector falls back to +Y (never NaN)
        assert_eq!(Vec3::ZERO.normalized(), Vec3::UP);
    }

    #[test]
    fn lerp_midpoint() {
        let m = Vec3::ZERO.lerp(Vec3::new(2.0, 4.0, 6.0), 0.5);
        assert!((m - Vec3::new(1.0, 2.0, 3.0)).len() < 1e-6);
    }
}

/// An axis-aligned bounding box, used for chunk/mesh culling.
#[derive(Clone, Copy, Debug)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    /// An empty box that grows to fit points fed to `expand`.
    pub fn empty() -> Self {
        Aabb { min: Vec3::splat(f32::INFINITY), max: Vec3::splat(f32::NEG_INFINITY) }
    }
    pub fn expand(&mut self, p: Vec3) {
        self.min.x = self.min.x.min(p.x);
        self.min.y = self.min.y.min(p.y);
        self.min.z = self.min.z.min(p.z);
        self.max.x = self.max.x.max(p.x);
        self.max.y = self.max.y.max(p.y);
        self.max.z = self.max.z.max(p.z);
    }
    pub fn is_valid(&self) -> bool {
        self.min.x <= self.max.x
    }
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }
    /// The 8 corners — used to test the box against the camera frustum.
    pub fn corners(&self) -> [Vec3; 8] {
        let (a, b) = (self.min, self.max);
        [
            Vec3::new(a.x, a.y, a.z),
            Vec3::new(b.x, a.y, a.z),
            Vec3::new(a.x, b.y, a.z),
            Vec3::new(b.x, b.y, a.z),
            Vec3::new(a.x, a.y, b.z),
            Vec3::new(b.x, a.y, b.z),
            Vec3::new(a.x, b.y, b.z),
            Vec3::new(b.x, b.y, b.z),
        ]
    }
}

#[cfg(test)]
mod aabb_tests {
    use super::*;
    #[test]
    fn expands_to_fit() {
        let mut b = Aabb::empty();
        assert!(!b.is_valid());
        b.expand(Vec3::new(-1.0, 0.0, 2.0));
        b.expand(Vec3::new(3.0, 4.0, -1.0));
        assert!(b.is_valid());
        assert!((b.center() - Vec3::new(1.0, 2.0, 0.5)).len() < 1e-6);
        assert_eq!(b.corners().len(), 8);
    }
}


