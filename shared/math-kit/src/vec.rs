//! Vec2 / Vec3 — f32, plain-old-data, deterministic.
//!
//! The `Vec3` method core (`add`/`sub`/`scale`/`dot`/`cross`/`len`/`norm`) is
//! clawbound math.rs **verbatim** — same formulas, same `1e-6` norm guard that
//! returns `self` unnormalized for near-zero vectors — so clawbound adopts this
//! type with zero behavior change (its 206-test suite is the parity proof).
//! On top of that core sit the soldier/logscope vocabularies (`len_sq`,
//! `normalized`, `lerp`, `dist`, `splat`, `with_y`, `xz`, operator overloads)
//! so those migrations are mechanical too.

use std::ops::{Add, AddAssign, Div, Mul, Neg, Sub, SubAssign};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

/// Shorthand constructor, mirroring `v3`.
#[inline]
pub fn v2(x: f32, y: f32) -> Vec2 {
    Vec2 { x, y }
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    #[inline]
    pub fn new(x: f32, y: f32) -> Vec2 {
        v2(x, y)
    }
    #[inline]
    pub fn splat(v: f32) -> Vec2 {
        v2(v, v)
    }
    #[inline]
    pub fn dot(self, o: Vec2) -> f32 {
        self.x * o.x + self.y * o.y
    }
    /// 2D cross product (z of the 3D cross) — signed parallelogram area.
    #[inline]
    pub fn cross(self, o: Vec2) -> f32 {
        self.x * o.y - self.y * o.x
    }
    #[inline]
    pub fn len_sq(self) -> f32 {
        self.dot(self)
    }
    #[inline]
    pub fn len(self) -> f32 {
        self.len_sq().sqrt()
    }
    /// Unit vector; returns `self` unchanged when `len ≤ 1e-6` (clawbound's guard).
    #[inline]
    pub fn norm(self) -> Vec2 {
        let l = self.len();
        if l > 1e-6 {
            self * (1.0 / l)
        } else {
            self
        }
    }
    /// Alias for [`Vec2::norm`] (soldier's name).
    #[inline]
    pub fn normalized(self) -> Vec2 {
        self.norm()
    }
    #[inline]
    pub fn lerp(self, o: Vec2, t: f32) -> Vec2 {
        self + (o - self) * t
    }
    #[inline]
    pub fn dist_sq(self, o: Vec2) -> f32 {
        (o - self).len_sq()
    }
    #[inline]
    pub fn dist(self, o: Vec2) -> f32 {
        (o - self).len()
    }
    /// Counter-clockwise perpendicular.
    #[inline]
    pub fn perp(self) -> Vec2 {
        v2(-self.y, self.x)
    }
    /// Angle from +x axis, radians in (−π, π].
    #[inline]
    pub fn angle(self) -> f32 {
        self.y.atan2(self.x)
    }
    /// Unit vector at `a` radians from +x.
    #[inline]
    pub fn from_angle(a: f32) -> Vec2 {
        v2(a.cos(), a.sin())
    }
    /// Rotate counter-clockwise by `a` radians.
    #[inline]
    pub fn rotated(self, a: f32) -> Vec2 {
        let (s, c) = a.sin_cos();
        v2(self.x * c - self.y * s, self.x * s + self.y * c)
    }
    #[inline]
    pub fn min(self, o: Vec2) -> Vec2 {
        v2(self.x.min(o.x), self.y.min(o.y))
    }
    #[inline]
    pub fn max(self, o: Vec2) -> Vec2 {
        v2(self.x.max(o.x), self.y.max(o.y))
    }
    #[inline]
    pub fn abs(self) -> Vec2 {
        v2(self.x.abs(), self.y.abs())
    }
    #[inline]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
    #[inline]
    pub fn to_array(self) -> [f32; 2] {
        [self.x, self.y]
    }
}

impl From<[f32; 2]> for Vec2 {
    #[inline]
    fn from(a: [f32; 2]) -> Vec2 {
        v2(a[0], a[1])
    }
}
impl From<Vec2> for [f32; 2] {
    #[inline]
    fn from(v: Vec2) -> [f32; 2] {
        v.to_array()
    }
}

impl Add for Vec2 {
    type Output = Vec2;
    #[inline]
    fn add(self, o: Vec2) -> Vec2 {
        v2(self.x + o.x, self.y + o.y)
    }
}
impl Sub for Vec2 {
    type Output = Vec2;
    #[inline]
    fn sub(self, o: Vec2) -> Vec2 {
        v2(self.x - o.x, self.y - o.y)
    }
}
impl Mul<f32> for Vec2 {
    type Output = Vec2;
    #[inline]
    fn mul(self, s: f32) -> Vec2 {
        v2(self.x * s, self.y * s)
    }
}
impl Div<f32> for Vec2 {
    type Output = Vec2;
    #[inline]
    fn div(self, s: f32) -> Vec2 {
        v2(self.x / s, self.y / s)
    }
}
impl Neg for Vec2 {
    type Output = Vec2;
    #[inline]
    fn neg(self) -> Vec2 {
        v2(-self.x, -self.y)
    }
}
impl AddAssign for Vec2 {
    #[inline]
    fn add_assign(&mut self, o: Vec2) {
        *self = *self + o;
    }
}
impl SubAssign for Vec2 {
    #[inline]
    fn sub_assign(&mut self, o: Vec2) {
        *self = *self - o;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

/// Shorthand constructor — clawbound's spelling, kept verbatim.
#[inline]
pub fn v3(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3 { x, y, z }
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 { x: 0.0, y: 0.0, z: 0.0 };

    #[inline]
    pub fn new(x: f32, y: f32, z: f32) -> Vec3 {
        v3(x, y, z)
    }
    #[inline]
    pub fn splat(v: f32) -> Vec3 {
        v3(v, v, v)
    }

    // ── clawbound math.rs core, verbatim ────────────────────────────────────
    #[inline]
    pub fn add(self, o: Vec3) -> Vec3 {
        v3(self.x + o.x, self.y + o.y, self.z + o.z)
    }
    #[inline]
    pub fn sub(self, o: Vec3) -> Vec3 {
        v3(self.x - o.x, self.y - o.y, self.z - o.z)
    }
    #[inline]
    pub fn scale(self, s: f32) -> Vec3 {
        v3(self.x * s, self.y * s, self.z * s)
    }
    #[inline]
    pub fn dot(self, o: Vec3) -> f32 {
        self.x * o.x + self.y * o.y + self.z * o.z
    }
    #[inline]
    pub fn cross(self, o: Vec3) -> Vec3 {
        v3(
            self.y * o.z - self.z * o.y,
            self.z * o.x - self.x * o.z,
            self.x * o.y - self.y * o.x,
        )
    }
    #[inline]
    pub fn len(self) -> f32 {
        self.dot(self).sqrt()
    }
    /// Unit vector; returns `self` unchanged when `len ≤ 1e-6` (clawbound's guard).
    #[inline]
    pub fn norm(self) -> Vec3 {
        let l = self.len();
        if l > 1e-6 {
            self.scale(1.0 / l)
        } else {
            self
        }
    }

    // ── soldier / logscope vocabulary ───────────────────────────────────────
    #[inline]
    pub fn len_sq(self) -> f32 {
        self.dot(self)
    }
    /// Alias for [`Vec3::norm`] (soldier's name).
    #[inline]
    pub fn normalized(self) -> Vec3 {
        self.norm()
    }
    #[inline]
    pub fn lerp(self, o: Vec3, t: f32) -> Vec3 {
        self.add(o.sub(self).scale(t))
    }
    #[inline]
    pub fn dist_sq(self, o: Vec3) -> f32 {
        o.sub(self).len_sq()
    }
    #[inline]
    pub fn dist(self, o: Vec3) -> f32 {
        o.sub(self).len()
    }
    #[inline]
    pub fn with_x(self, x: f32) -> Vec3 {
        v3(x, self.y, self.z)
    }
    #[inline]
    pub fn with_y(self, y: f32) -> Vec3 {
        v3(self.x, y, self.z)
    }
    #[inline]
    pub fn with_z(self, z: f32) -> Vec3 {
        v3(self.x, self.y, z)
    }
    /// Ground-plane projection (soldier's `xz`).
    #[inline]
    pub fn xz(self) -> Vec2 {
        v2(self.x, self.z)
    }
    #[inline]
    pub fn min(self, o: Vec3) -> Vec3 {
        v3(self.x.min(o.x), self.y.min(o.y), self.z.min(o.z))
    }
    #[inline]
    pub fn max(self, o: Vec3) -> Vec3 {
        v3(self.x.max(o.x), self.y.max(o.y), self.z.max(o.z))
    }
    #[inline]
    pub fn abs(self) -> Vec3 {
        v3(self.x.abs(), self.y.abs(), self.z.abs())
    }
    #[inline]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
    #[inline]
    pub fn to_array(self) -> [f32; 3] {
        [self.x, self.y, self.z]
    }
}

impl From<[f32; 3]> for Vec3 {
    #[inline]
    fn from(a: [f32; 3]) -> Vec3 {
        v3(a[0], a[1], a[2])
    }
}
impl From<Vec3> for [f32; 3] {
    #[inline]
    fn from(v: Vec3) -> [f32; 3] {
        v.to_array()
    }
}

impl Add for Vec3 {
    type Output = Vec3;
    #[inline]
    fn add(self, o: Vec3) -> Vec3 {
        Vec3::add(self, o)
    }
}
impl Sub for Vec3 {
    type Output = Vec3;
    #[inline]
    fn sub(self, o: Vec3) -> Vec3 {
        Vec3::sub(self, o)
    }
}
impl Mul<f32> for Vec3 {
    type Output = Vec3;
    #[inline]
    fn mul(self, s: f32) -> Vec3 {
        self.scale(s)
    }
}
impl Div<f32> for Vec3 {
    type Output = Vec3;
    #[inline]
    fn div(self, s: f32) -> Vec3 {
        self.scale(1.0 / s)
    }
}
impl Neg for Vec3 {
    type Output = Vec3;
    #[inline]
    fn neg(self) -> Vec3 {
        self.scale(-1.0)
    }
}
impl AddAssign for Vec3 {
    #[inline]
    fn add_assign(&mut self, o: Vec3) {
        *self = Vec3::add(*self, o);
    }
}
impl SubAssign for Vec3 {
    #[inline]
    fn sub_assign(&mut self, o: Vec3) {
        *self = Vec3::sub(*self, o);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vec3_core_identities() {
        let a = v3(1.0, 2.0, 3.0);
        let b = v3(-4.0, 0.5, 2.0);
        assert_eq!(a.add(b), a + b);
        assert_eq!(a.sub(b), a - b);
        assert_eq!(a.scale(2.0), a * 2.0);
        // cross ⟂ both inputs
        let c = a.cross(b);
        assert!(c.dot(a).abs() < 1e-4 && c.dot(b).abs() < 1e-4);
        // right-handed basis
        assert_eq!(v3(1.0, 0.0, 0.0).cross(v3(0.0, 1.0, 0.0)), v3(0.0, 0.0, 1.0));
        // |a×b|² + (a·b)² = |a|²|b|² (Lagrange)
        let lhs = c.len_sq() + a.dot(b) * a.dot(b);
        assert!((lhs - a.len_sq() * b.len_sq()).abs() < 1e-3);
    }

    #[test]
    fn norm_guard_matches_clawbound() {
        // near-zero vectors come back UNCHANGED, not NaN
        let tiny = v3(1e-9, 0.0, 0.0);
        assert_eq!(tiny.norm(), tiny);
        assert!((v3(3.0, 0.0, 4.0).norm().len() - 1.0).abs() < 1e-6);
        let t2 = v2(0.0, 1e-9);
        assert_eq!(t2.norm(), t2);
    }

    #[test]
    fn lerp_dist_and_views() {
        let a = v3(0.0, 0.0, 0.0);
        let b = v3(2.0, 4.0, 6.0);
        assert_eq!(a.lerp(b, 0.5), v3(1.0, 2.0, 3.0));
        assert_eq!(a.dist(b), b.len());
        assert_eq!(b.xz(), v2(2.0, 6.0));
        assert_eq!(b.with_y(9.0), v3(2.0, 9.0, 6.0));
        assert_eq!(Vec3::from([1.0, 2.0, 3.0]).to_array(), [1.0, 2.0, 3.0]);
    }

    #[test]
    fn vec2_rotation_and_perp() {
        let e = v2(1.0, 0.0);
        let r = e.rotated(std::f32::consts::FRAC_PI_2);
        assert!(r.dist(v2(0.0, 1.0)) < 1e-6);
        assert_eq!(e.perp(), v2(0.0, 1.0));
        assert!((v2(3.0, 4.0).len() - 5.0).abs() < 1e-6);
        assert!((v2(0.0, 2.0).angle() - std::f32::consts::FRAC_PI_2).abs() < 1e-6);
    }
}
