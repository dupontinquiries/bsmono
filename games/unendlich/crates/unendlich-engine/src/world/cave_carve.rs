//! `cave_carve` — 3D tunnel carving along a cave-network edge (capsule SDF).
//!
//! Worker-03 / WORLD-PERF pure core, round 11. Completes the cave system: given
//! a tunnel segment `a→b` from `cave_network`, decide whether a world point lies
//! inside the carved tunnel. The tunnel is a capsule (a cylinder with rounded
//! caps), and `radius_at` lets the bore bulge toward the middle so tunnels open
//! into chambers rather than reading as uniform pipes.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Is point `p` inside the capsule tunnel of constant `radius` around segment
/// `a→b`? (Distance from `p` to the segment ≤ radius — rounded end caps.)
pub fn carved(p: [f32; 3], a: [f32; 3], b: [f32; 3], radius: f32) -> bool {
    segment_distance(p, a, b) <= radius.max(0.0)
}

/// Tunnel radius at parameter `t ∈ [0,1]` along the segment: `base` plus a
/// parabolic `bulge` that peaks at the midpoint and vanishes at the ends.
/// `t` is clamped to `[0,1]`.
pub fn radius_at(t: f32, base: f32, bulge: f32) -> f32 {
    let tc = clamp01(t);
    let m = 2.0 * tc - 1.0; // -1 at ends, 0 at middle
    base + bulge * (1.0 - m * m)
}

/// Distance from `p` to segment `a→b`.
fn segment_distance(p: [f32; 3], a: [f32; 3], b: [f32; 3]) -> f32 {
    let ab = sub(b, a);
    let ap = sub(p, a);
    let denom = dot(ab, ab);
    let t = if denom > 0.0 {
        clamp01(dot(ap, ab) / denom)
    } else {
        0.0
    };
    let proj = [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t];
    let d = sub(p, proj);
    dot(d, d).sqrt()
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}
fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn point_near_segment_is_inside() {
        let a = [0.0, 0.0, 0.0];
        let b = [10.0, 0.0, 0.0];
        assert!(carved([5.0, 0.0, 0.0], a, b, 2.0)); // on the axis
        assert!(carved([5.0, 1.5, 0.0], a, b, 2.0)); // within radius
    }

    #[test]
    fn point_beyond_radius_is_outside() {
        let a = [0.0, 0.0, 0.0];
        let b = [10.0, 0.0, 0.0];
        assert!(!carved([5.0, 5.0, 0.0], a, b, 2.0));
    }

    #[test]
    fn endpoints_are_capped() {
        let a = [0.0, 0.0, 0.0];
        let b = [10.0, 0.0, 0.0];
        // Just past endpoint a, within radius → inside the rounded cap.
        assert!(carved([-1.0, 0.0, 0.0], a, b, 2.0));
        // Far past the endpoint, beyond radius → outside.
        assert!(!carved([-5.0, 0.0, 0.0], a, b, 2.0));
    }

    #[test]
    fn bulge_widens_the_middle() {
        let mid = radius_at(0.5, 3.0, 4.0);
        let end = radius_at(0.0, 3.0, 4.0);
        assert!(mid > end);
        assert!((end - 3.0).abs() < 1e-6); // ends == base
        assert!((mid - 7.0).abs() < 1e-6); // base + bulge
    }

    #[test]
    fn t_is_clamped() {
        assert_eq!(radius_at(-1.0, 2.0, 5.0), radius_at(0.0, 2.0, 5.0));
        assert_eq!(radius_at(2.0, 2.0, 5.0), radius_at(1.0, 2.0, 5.0));
    }

    #[test]
    fn deterministic() {
        let a = [1.0, 2.0, 3.0];
        let b = [4.0, 0.0, -2.0];
        assert_eq!(carved([2.0, 1.0, 1.0], a, b, 2.5), carved([2.0, 1.0, 1.0], a, b, 2.5));
        assert_eq!(radius_at(0.3, 1.0, 2.0), radius_at(0.3, 1.0, 2.0));
    }

    #[test]
    fn degenerate_segment_is_a_sphere() {
        let a = [0.0, 0.0, 0.0];
        assert!(carved([0.5, 0.0, 0.0], a, a, 1.0));
        assert!(!carved([2.0, 0.0, 0.0], a, a, 1.0));
    }
}
