//! Gravity anomalies — the floating-island regions where the world's pull goes
//! strange. Near a gravity island the downward pull weakens, cancels, and can
//! invert: pads lift off, dropped items and water drift upward, a staircase of
//! blocks wobbles up into the sky, and you can climb onto a hollow little
//! mini-planet. Everywhere else gravity is ordinary.
//!
//! The model is a scalar **gravity scale** at a point — `1.0` normal, `0.0`
//! weightless, negative = inverted (things fall *up*). Multiply the base
//! `physics::GRAVITY_PER_TICK` (and the buoyancy gravity) by this to make the
//! anomaly felt; the same scale tells the world which loose voxels/items should
//! drift skyward. A `GravityIsland` from `world::sparse_features` becomes an
//! anomaly here.
//!
//! `f32` field math; quantize to `Fixed` at the deterministic physics call site.

/// A point of anomalous gravity (a gravity-island core).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GravityAnomaly {
    pub center: [f32; 3],
    pub radius: f32,
    /// How much it subtracts from the gravity scale at its centre. 1.0 cancels
    /// gravity; >1 inverts it (lift); 0.5 just lightens it.
    pub strength: f32,
}

impl GravityAnomaly {
    pub fn new(center: [f32; 3], radius: f32, strength: f32) -> GravityAnomaly {
        GravityAnomaly { center, radius, strength }
    }
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let (dx, dy, dz) = (a[0] - b[0], a[1] - b[1], a[2] - b[2]);
    (dx * dx + dy * dy + dz * dz).sqrt()
}

/// Local gravity scale at `pos`: starts at `1.0`, each anomaly within range
/// subtracts `strength × falloff` (smooth, `1` at the centre to `0` at the
/// edge). Can go negative — that's the inverted lift on a floating island.
pub fn gravity_scale_at(pos: [f32; 3], anomalies: &[GravityAnomaly]) -> f32 {
    let mut scale = 1.0;
    for a in anomalies {
        if a.radius <= 0.0 {
            continue;
        }
        let d = dist(pos, a.center);
        if d < a.radius {
            let t = 1.0 - d / a.radius; // 1 at centre → 0 at edge
            let falloff = t * t; // smooth ease so the boundary isn't a hard step
            scale -= a.strength * falloff;
        }
    }
    scale
}

/// Effective downward gravity = `base × gravity_scale_at`. Negative result =
/// upward pull (things rise).
pub fn effective_gravity(base: f32, pos: [f32; 3], anomalies: &[GravityAnomaly]) -> f32 {
    base * gravity_scale_at(pos, anomalies)
}

/// Does gravity lift here (scale below zero)? Loose voxels, water, and dropped
/// items at such a point drift upward.
pub fn lifts(pos: [f32; 3], anomalies: &[GravityAnomaly]) -> bool {
    gravity_scale_at(pos, anomalies) < 0.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn anomaly() -> GravityAnomaly {
        GravityAnomaly::new([0.0, 100.0, 0.0], 30.0, 1.2) // a strong, inverting island
    }

    #[test]
    fn normal_gravity_far_from_any_anomaly() {
        assert_eq!(gravity_scale_at([1000.0, 0.0, 1000.0], &[anomaly()]), 1.0);
        assert_eq!(gravity_scale_at([0.0, 0.0, 0.0], &[]), 1.0, "no anomalies → normal");
    }

    #[test]
    fn gravity_weakens_and_inverts_toward_the_core() {
        let a = [anomaly()];
        let centre = gravity_scale_at([0.0, 100.0, 0.0], &a);
        let edge = gravity_scale_at([0.0, 129.0, 0.0], &a); // just inside the rim
        assert!(centre < 0.0, "inverted at the core (strength 1.2): {centre}");
        assert!(edge > centre && edge <= 1.0, "weaker toward the core: edge {edge} > centre {centre}");
        assert!(lifts([0.0, 100.0, 0.0], &a), "things drift up at the core");
        assert!(!lifts([0.0, 129.0, 0.0], &a), "but not near the rim");
    }

    #[test]
    fn a_gentle_anomaly_only_lightens() {
        let soft = [GravityAnomaly::new([0.0, 0.0, 0.0], 20.0, 0.5)];
        let s = gravity_scale_at([0.0, 0.0, 0.0], &soft);
        assert!(s > 0.0 && s < 1.0, "lighter but still downward: {s}");
        assert!(!lifts([0.0, 0.0, 0.0], &soft));
    }

    #[test]
    fn effective_gravity_scales_and_anomalies_stack() {
        // Base downward gravity is negative (toward -Y) by convention.
        let g = -2.0;
        let none = effective_gravity(g, [500.0, 0.0, 0.0], &[]);
        assert_eq!(none, g, "untouched far away");
        // Two overlapping anomalies sum their pull-reduction.
        let two = [
            GravityAnomaly::new([0.0, 0.0, 0.0], 40.0, 0.6),
            GravityAnomaly::new([0.0, 0.0, 0.0], 40.0, 0.6),
        ];
        let stacked = gravity_scale_at([0.0, 0.0, 0.0], &two);
        let single = gravity_scale_at([0.0, 0.0, 0.0], &two[..1]);
        assert!(stacked < single, "overlapping anomalies stack ({stacked} < {single})");
    }
}
