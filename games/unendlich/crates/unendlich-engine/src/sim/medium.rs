//! Ambient-medium forces — the shared system behind "wind pushes you through
//! the air" and "current carries you through water", plus buoyancy. A body
//! immersed in a medium feels the same two things, just with different
//! constants:
//!
//! 1. **Drag toward the medium's velocity.** A still medium merely damps you; a
//!    moving one (wind, current) carries you along. `dv = k·(medium − body)`
//!    per axis, where `k = drag · submerged`.
//! 2. **Buoyancy** (vertical) from the density contrast and how submerged you
//!    are — reuses `fluid::buoyancy_accel`.
//!
//! Air is a light, low-density medium (you barely feel drag, no buoyancy);
//! water is heavy and dense (strong drag, real buoyancy). Same equations.
//! Deterministic `Fixed`; unit-tested. The wind/current *velocity* is supplied
//! by the caller (0 = still), so this stays a pure function of its inputs.

use crate::sim::fixed::{Fixed, Vec3};
use crate::sim::fluid::{buoyancy_accel, density_ratio};

/// A surrounding medium: how dense it is (for buoyancy) and how hard it drags
/// a body toward its own motion.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Medium {
    /// Density in the same arbitrary units as body density (air ≈ 1, water ≈ 1000).
    pub density: i64,
    /// Fraction of the body↔medium velocity gap closed per tick, `[0,1]`.
    pub drag: Fixed,
}

impl Medium {
    /// Thin, fast: you feel wind only as a gentle push, and there's no lift.
    pub const AIR: Medium = Medium { density: 1, drag: Fixed::from_frac(2, 100) };
    /// Heavy, draggy: water resists motion strongly and floats you.
    pub const WATER: Medium = Medium { density: 1000, drag: Fixed::from_frac(30, 100) };
}

/// Drag delta-v toward the medium's velocity, scaled by submersion. The shared
/// wind/current term: a still medium (`medium_vel = 0`) just damps the body;
/// a moving one carries it. `submerged` in `[0,1]` (in air, pass 1).
pub fn drag_dv(body_vel: Vec3, medium_vel: Vec3, drag: Fixed, submerged: Fixed) -> Vec3 {
    let k = drag * submerged;
    Vec3 {
        x: (medium_vel.x - body_vel.x) * k,
        y: (medium_vel.y - body_vel.y) * k,
        z: (medium_vel.z - body_vel.z) * k,
    }
}

/// Vertical buoyancy delta-v **relative to plain gravity**: add this to a body
/// that has already had `gravity` applied this tick, and its net vertical accel
/// becomes the buoyancy-adjusted value. Equals `−gravity · ratio · submerged`
/// (gravity is negative/down, so a positive result lifts).
///
/// - body lighter than the medium → rises;
/// - neutral (equal density) → cancels gravity when fully submerged (hovers);
/// - heavier → still sinks, but slower.
pub fn buoyancy_dv(submerged: Fixed, medium_density: i64, body_density: i64, gravity: Fixed) -> Fixed {
    let ratio = density_ratio(medium_density, body_density);
    buoyancy_accel(submerged, ratio, gravity) - gravity
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3 {
            x: Fixed::from_frac((x * 1000.0) as i64, 1000),
            y: Fixed::from_frac((y * 1000.0) as i64, 1000),
            z: Fixed::from_frac((z * 1000.0) as i64, 1000),
        }
    }

    #[test]
    fn still_medium_damps_toward_zero() {
        // No current: drag pulls a moving body's velocity toward zero.
        let dv = drag_dv(v(2.0, 0.0, -1.0), v(0.0, 0.0, 0.0), Medium::WATER.drag, Fixed::ONE);
        assert!(dv.x < Fixed::ZERO, "moving +x → drag is −x");
        assert!(dv.z > Fixed::ZERO, "moving −z → drag is +z");
    }

    #[test]
    fn moving_medium_carries_the_body() {
        // Current in +x, body still → pushed +x (wind/current advection).
        let dv = drag_dv(v(0.0, 0.0, 0.0), v(3.0, 0.0, 0.0), Medium::WATER.drag, Fixed::ONE);
        assert!(dv.x > Fixed::ZERO, "current carries the body downstream");
    }

    #[test]
    fn submersion_scales_drag() {
        let half = drag_dv(v(2.0, 0.0, 0.0), v(0.0, 0.0, 0.0), Medium::WATER.drag, Fixed::from_frac(1, 2));
        let full = drag_dv(v(2.0, 0.0, 0.0), v(0.0, 0.0, 0.0), Medium::WATER.drag, Fixed::ONE);
        assert!(full.x.abs() > half.x.abs(), "more submerged → more drag");
    }

    #[test]
    fn air_drag_is_far_weaker_than_water() {
        let air = drag_dv(v(5.0, 0.0, 0.0), v(0.0, 0.0, 0.0), Medium::AIR.drag, Fixed::ONE);
        let water = drag_dv(v(5.0, 0.0, 0.0), v(0.0, 0.0, 0.0), Medium::WATER.drag, Fixed::ONE);
        assert!(water.x.abs() > air.x.abs(), "water drags much harder than air");
    }

    #[test]
    fn buoyancy_sign_tracks_density_contrast() {
        let g = Fixed::from_frac(-2, 100); // small downward gravity
        // Light body (900) in water (1000): rises → positive (upward) dv.
        let light = buoyancy_dv(Fixed::ONE, 1000, 900, g);
        assert!(light > Fixed::ZERO, "light body floats up");
        // Dense body (1200) in water: net still sinks → buoyancy dv smaller than
        // the full anti-gravity it'd need, so net vy stays downward.
        let dense = buoyancy_dv(Fixed::ONE, 1000, 1200, g);
        assert!(dense < light, "denser body gets less lift");
        // Neutral body: fully submerged buoyancy exactly cancels gravity.
        let neutral = buoyancy_dv(Fixed::ONE, 1000, 1000, g);
        assert!((neutral + g).abs() < Fixed::from_frac(1, 1000), "neutral hovers (net ~0)");
        // Out of the water (submerged 0): no buoyancy at all.
        assert_eq!(buoyancy_dv(Fixed::ZERO, 1000, 900, g), Fixed::ZERO);
    }
}
