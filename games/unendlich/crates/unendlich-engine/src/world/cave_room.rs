//! `cave_room` — carve an ellipsoid chamber along a cave.
//!
//! Worker-03 / WORLD pure core, round 17. Where a tunnel (`cave_carve`, R11)
//! reaches a node it can open into a chamber; this is the chamber's inside test:
//! a per-axis ellipsoid so rooms can be tall, wide, or flat. A zero radius on an
//! axis degenerates that axis to a plane constraint, so all-zero radii collapse
//! to a single point.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Is point `p` inside the ellipsoid chamber at `center` with per-axis `radii`?
/// `Σ ((p−center)/radii)² ≤ 1`. A zero radius requires an exact match on that
/// axis (degenerate plane / point).
pub fn carved(p: [f32; 3], center: [f32; 3], radii: [f32; 3]) -> bool {
    let mut acc = 0.0f32;
    for i in 0..3 {
        let d = p[i] - center[i];
        if radii[i] > 0.0 {
            let n = d / radii[i];
            acc += n * n;
        } else if d != 0.0 {
            return false; // degenerate axis: must lie exactly on center
        }
    }
    acc <= 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    const C: [f32; 3] = [100.0, 50.0, 100.0];

    #[test]
    fn center_is_inside() {
        assert!(carved(C, C, [10.0, 6.0, 10.0]));
    }

    #[test]
    fn outside_radii_is_outside() {
        assert!(!carved([130.0, 50.0, 100.0], C, [10.0, 6.0, 10.0]));
        assert!(!carved([100.0, 70.0, 100.0], C, [10.0, 6.0, 10.0]));
    }

    #[test]
    fn axis_scaling_is_anisotropic() {
        let radii = [20.0, 4.0, 4.0]; // long on x, short on y/z
        assert!(carved([C[0] + 18.0, C[1], C[2]], C, radii)); // far on x → in
        assert!(!carved([C[0], C[1] + 8.0, C[2]], C, radii)); // same dist on y → out
        // exactly on the x boundary is inside (≤ 1)
        assert!(carved([C[0] + 20.0, C[1], C[2]], C, radii));
    }

    #[test]
    fn degenerate_radii_is_a_point() {
        assert!(carved(C, C, [0.0, 0.0, 0.0]));
        assert!(!carved([C[0] + 1.0, C[1], C[2]], C, [0.0, 0.0, 0.0]));
    }

    #[test]
    fn symmetric_about_center() {
        let radii = [12.0, 8.0, 10.0];
        let off = [5.0, 3.0, 7.0];
        let plus = [C[0] + off[0], C[1] + off[1], C[2] + off[2]];
        let minus = [C[0] - off[0], C[1] - off[1], C[2] - off[2]];
        assert_eq!(carved(plus, C, radii), carved(minus, C, radii));
    }

    #[test]
    fn deterministic() {
        let r = [9.0, 9.0, 9.0];
        assert_eq!(carved([105.0, 52.0, 103.0], C, r), carved([105.0, 52.0, 103.0], C, r));
    }
}
