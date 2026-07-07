//! Branch physics — a light bending model so we know which limbs bear a climbing
//! cat and which snap. A branch is a tapered round beam cantilevered from the
//! trunk; a point load (the cat) at distance `d` along it creates a bending
//! moment `M = w·d`, and the beam resists in proportion to r³ (section modulus).
//! Snap risk = moment / strength — thin, long, far-out limbs give first.

use crate::math::{v3, Vec3};

pub const G: f32 = 9.8;

#[derive(Clone, Copy)]
pub struct Branch {
    pub a: Vec3,   // attach point (on trunk / parent) — the cantilever base
    pub b: Vec3,   // tip
    pub r: f32,    // base radius
}

impl Branch {
    pub fn len(&self) -> f32 { self.b.sub(self.a).len() }
    pub fn point(&self, along: f32) -> Vec3 { self.a.add(self.b.sub(self.a).scale(along)) }

    /// Self-mass of the limb (tapered wood), for future load-chaining.
    pub fn mass(&self) -> f32 {
        std::f32::consts::PI * self.r * self.r * self.len() * 350.0
    }

    /// Snap risk in [0, ∞); ≥1 means the limb fails. `load_kg` at fraction
    /// `along` (0=base … 1=tip). Simplified bending: risk ∝ w·d / r³.
    pub fn snap_risk(&self, load_kg: f32, along: f32) -> f32 {
        let d = self.len() * along.clamp(0.0, 1.0);
        let w = load_kg * G;
        4.0e-5 * w * d / self.r.powi(3).max(1e-6)
    }

    /// The safest fraction a `load_kg` cat can climb before risk hits 1.0.
    pub fn safe_reach(&self, load_kg: f32) -> f32 {
        // solve snap_risk(load, along)=1 for along
        let w = load_kg * G;
        let denom = 4.0e-5 * w * self.len();
        if denom <= 0.0 { return 1.0; }
        (self.r.powi(3) / denom).clamp(0.0, 1.0)
    }
}

pub fn tip(a: Vec3, dir: Vec3, len: f32) -> Vec3 { a.add(dir.norm().scale(len)) }
pub fn _touch() -> Vec3 { v3(0.0, 0.0, 0.0) }
