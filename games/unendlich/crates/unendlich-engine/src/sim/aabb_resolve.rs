//! `sim::aabb_resolve` — axis-aligned bounding-box collision math: overlap
//! test, minimum-translation-vector (MTV), and a velocity resolver with
//! restitution + friction.
//!
//! Pure and deterministic: data in, data out. No GPU, IO, clock, or RNG; no
//! `.unwrap()`/`.expect()`. Inputs sanitised; never panics.

/// An axis-aligned bounding box. `min[i] <= max[i]` is expected; callers that
/// pass inverted bounds simply get "no overlap".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

#[inline]
fn san(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}
#[inline]
fn clamp01(x: f32) -> f32 {
    san(x).max(0.0).min(1.0)
}

/// Per-axis open overlap extent (`<= 0` means separated/touching on that axis).
#[inline]
fn axis_overlap(a: &Aabb, b: &Aabb, i: usize) -> f32 {
    let lo = san(a.min[i]).max(san(b.min[i]));
    let hi = san(a.max[i]).min(san(b.max[i]));
    hi - lo
}

/// True iff the boxes overlap with positive volume on all three axes (touching
/// faces do not count).
pub fn overlaps(a: &Aabb, b: &Aabb) -> bool {
    (0..3).all(|i| axis_overlap(a, b, i) > 0.0)
}

/// Minimal push (penetration depth, signed direction) to separate `a` from `b`
/// along axis `i`. `None` if they don't overlap on that axis.
///
/// Moving `a` by `+pen` clears it past `b.max` (`= b.max - a.min`); moving by
/// `-pen` clears it past `b.min` (`= a.max - b.min`). The shorter of the two is
/// the penetration; its direction is the sign. This is correct even when one
/// box contains the other on the axis (where the overlap *extent* would
/// understate the push needed).
#[inline]
fn axis_separation(a: &Aabb, b: &Aabb, i: usize) -> Option<(f32, f32)> {
    let p_pos = san(b.max[i]) - san(a.min[i]); // move +i by this to clear
    let p_neg = san(a.max[i]) - san(b.min[i]); // move -i by this to clear
    if p_pos <= 0.0 || p_neg <= 0.0 {
        return None; // separated (or touching) on this axis
    }
    if p_pos < p_neg {
        Some((p_pos, 1.0))
    } else {
        Some((p_neg, -1.0))
    }
}

/// Minimum translation vector to push `a` out of `b` along the axis of least
/// penetration. `None` when the boxes are not overlapping.
///
/// The returned vector is zero on two axes; on the least-penetration axis its
/// magnitude equals that penetration depth and its sign moves `a` the short way
/// out. Applying it (or slightly more) always separates the boxes — including
/// when one contains the other on an axis.
pub fn mtv(a: &Aabb, b: &Aabb) -> Option<[f32; 3]> {
    let mut best_axis = 0usize;
    let mut best_pen = f32::INFINITY;
    let mut best_sign = 1.0f32;
    for i in 0..3 {
        match axis_separation(a, b, i) {
            None => return None, // separated on this axis -> no collision
            Some((pen, sign)) => {
                if pen < best_pen {
                    best_pen = pen;
                    best_sign = sign;
                    best_axis = i;
                }
            }
        }
    }

    let mut out = [0.0f32; 3];
    out[best_axis] = best_sign * best_pen;
    Some(out)
}

/// Resolve a velocity against a surface `normal`.
///
/// Splits `vel` into a component along `normal` and a tangential remainder.
/// The into-surface (negative-along-normal) component is reflected and scaled
/// by `restitution01` (so `0` = no bounce / fully absorbed, `1` = perfect
/// bounce); a velocity already moving away from the surface keeps its normal
/// component. The tangential component is damped by `(1 - friction01)`.
/// A zero-length `normal` returns `vel` unchanged.
pub fn resolve_velocity(vel: [f32; 3], normal: [f32; 3], restitution01: f32, friction01: f32) -> [f32; 3] {
    let v = [san(vel[0]), san(vel[1]), san(vel[2])];
    let mut n = [san(normal[0]), san(normal[1]), san(normal[2])];

    let len2 = n[0] * n[0] + n[1] * n[1] + n[2] * n[2];
    if len2 <= 0.0 {
        return v; // no surface -> unchanged
    }
    let inv_len = 1.0 / len2.sqrt();
    n = [n[0] * inv_len, n[1] * inv_len, n[2] * inv_len];

    let rest = clamp01(restitution01);
    let fric = clamp01(friction01);

    let vn = v[0] * n[0] + v[1] * n[1] + v[2] * n[2]; // signed normal speed
    // normal output: reflect+scale if moving into surface, else keep.
    let n_scale = if vn < 0.0 { -rest * vn } else { vn };

    let mut out = [0.0f32; 3];
    for i in 0..3 {
        let tangential = v[i] - vn * n[i];
        out[i] = tangential * (1.0 - fric) + n_scale * n[i];
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ab(min: [f32; 3], max: [f32; 3]) -> Aabb {
        Aabb { min, max }
    }
    fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
        a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
    }

    #[test]
    fn overlaps_true_and_false() {
        let a = ab([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
        let inside = ab([1.0, 1.0, 1.0], [3.0, 3.0, 3.0]);
        assert!(overlaps(&a, &inside));
        let apart = ab([5.0, 0.0, 0.0], [6.0, 1.0, 1.0]);
        assert!(!overlaps(&a, &apart));
        // touching faces do not count
        let touch = ab([2.0, 0.0, 0.0], [4.0, 2.0, 2.0]);
        assert!(!overlaps(&a, &touch));
    }

    #[test]
    fn mtv_none_when_apart() {
        let a = ab([0.0, 0.0, 0.0], [1.0, 1.0, 1.0]);
        let b = ab([2.0, 2.0, 2.0], [3.0, 3.0, 3.0]);
        assert_eq!(mtv(&a, &b), None);
    }

    #[test]
    fn mtv_axis_sign_and_magnitude() {
        // a overlaps b shallowly on X (a to the left), deeply on Y/Z.
        let a = ab([0.0, 0.0, 0.0], [2.0, 5.0, 5.0]);
        let b = ab([1.5, 0.0, 0.0], [4.0, 5.0, 5.0]); // X overlap = 0.5 (least)
        let m = match mtv(&a, &b) { Some(v) => v, None => panic!("should overlap") };
        // least-penetration axis is X, push a in -X by 0.5
        assert_eq!(m, [-0.5, 0.0, 0.0]);

        // mirror: a to the right of b on X -> push +X
        let a2 = ab([2.5, 0.0, 0.0], [5.0, 5.0, 5.0]);
        let b2 = ab([0.0, 0.0, 0.0], [3.0, 5.0, 5.0]); // X overlap = 0.5
        let m2 = match mtv(&a2, &b2) { Some(v) => v, None => panic!("overlap") };
        assert_eq!(m2, [0.5, 0.0, 0.0]);

        // applying the MTV separates them
        let pushed = ab([a.min[0] + m[0], a.min[1] + m[1], a.min[2] + m[2]],
                        [a.max[0] + m[0], a.max[1] + m[1], a.max[2] + m[2]]);
        assert!(!overlaps(&pushed, &b));
    }

    #[test]
    fn resolve_kills_inward_and_bounces() {
        let n = [0.0, 1.0, 0.0]; // floor normal (up)
        let vel = [3.0, -10.0, 0.0]; // moving down into floor, sliding +x

        // restitution 0, friction 0 -> inward normal speed removed, tangent kept
        let r0 = resolve_velocity(vel, n, 0.0, 0.0);
        assert!((dot(r0, n)).abs() < 1e-6, "inward not removed: {:?}", r0);
        assert!((r0[0] - 3.0).abs() < 1e-6, "tangent changed: {:?}", r0);

        // restitution 1 -> bounces: normal speed reversed, equal magnitude
        let r1 = resolve_velocity(vel, n, 1.0, 0.0);
        assert!((dot(r1, n) - 10.0).abs() < 1e-5, "no full bounce: {:?}", r1);

        // friction 1 -> tangent killed, normal handled
        let rf = resolve_velocity(vel, n, 0.0, 1.0);
        assert!(rf[0].abs() < 1e-6, "tangent not damped: {:?}", rf);
    }

    #[test]
    fn resolve_leaves_outward_velocity_and_degenerate_normal() {
        let n = [0.0, 1.0, 0.0];
        // already moving away from surface (up): normal component preserved
        let up = resolve_velocity([1.0, 5.0, 0.0], n, 0.5, 0.0);
        assert!((dot(up, n) - 5.0).abs() < 1e-6, "outward normal changed: {:?}", up);
        // zero normal -> unchanged
        assert_eq!(resolve_velocity([1.0, 2.0, 3.0], [0.0, 0.0, 0.0], 0.5, 0.5), [1.0, 2.0, 3.0]);
        // non-unit normal is normalised (same result as unit)
        let a = resolve_velocity([3.0, -10.0, 0.0], [0.0, 2.0, 0.0], 0.5, 0.25);
        let b = resolve_velocity([3.0, -10.0, 0.0], [0.0, 1.0, 0.0], 0.5, 0.25);
        for i in 0..3 {
            assert!((a[i] - b[i]).abs() < 1e-5);
        }
    }

    #[test]
    fn resolve_finite_and_params_clamped() {
        let r = resolve_velocity([f32::NAN, -4.0, 1e30], [0.0, 1.0, 0.0], 2.0, -1.0);
        assert!(r.iter().all(|c| c.is_finite()));
        // restitution clamped to 1, friction to 0
        let a = resolve_velocity([2.0, -3.0, 0.0], [0.0, 1.0, 0.0], 2.0, -1.0);
        let b = resolve_velocity([2.0, -3.0, 0.0], [0.0, 1.0, 0.0], 1.0, 0.0);
        for i in 0..3 {
            assert!((a[i] - b[i]).abs() < 1e-6);
        }
    }

    #[test]
    fn deterministic() {
        let a = ab([0.0, 0.0, 0.0], [2.0, 2.0, 2.0]);
        let b = ab([1.0, 1.5, 0.5], [3.0, 3.0, 3.0]);
        assert_eq!(mtv(&a, &b), mtv(&a, &b));
        assert_eq!(resolve_velocity([1.0, -2.0, 3.0], [0.0, 0.0, 1.0], 0.4, 0.2),
                   resolve_velocity([1.0, -2.0, 3.0], [0.0, 0.0, 1.0], 0.4, 0.2));
    }

    #[test]
    fn mtv_seeded_sweep_separates() {
        // deterministic seeded sweep (R20)
        let mut x: u64 = 0xAABB_0054_u64;
        let mut next = || { x = x.wrapping_mul(6364136223846793005).wrapping_add(1); (x >> 33) as u32 };
        let span = |v: u32, lo: f32, hi: f32| lo + (v % 1000) as f32 / 1000.0 * (hi - lo);
        for _ in 0..3000 {
            let amin = [span(next(), -5.0, 5.0), span(next(), -5.0, 5.0), span(next(), -5.0, 5.0)];
            let asz = [span(next(), 0.2, 4.0), span(next(), 0.2, 4.0), span(next(), 0.2, 4.0)];
            let bmin = [span(next(), -5.0, 5.0), span(next(), -5.0, 5.0), span(next(), -5.0, 5.0)];
            let bsz = [span(next(), 0.2, 4.0), span(next(), 0.2, 4.0), span(next(), 0.2, 4.0)];
            let a = ab(amin, [amin[0] + asz[0], amin[1] + asz[1], amin[2] + asz[2]]);
            let b = ab(bmin, [bmin[0] + bsz[0], bmin[1] + bsz[1], bmin[2] + bsz[2]]);
            match mtv(&a, &b) {
                None => assert!(!overlaps(&a, &b), "None but overlapping"),
                Some(m) => {
                    assert!(overlaps(&a, &b), "Some but not overlapping");
                    assert!(m.iter().all(|c| c.is_finite()));
                    // exactly one non-zero axis
                    let nz = m.iter().filter(|&&c| c != 0.0).count();
                    assert!(nz <= 1, "mtv touched >1 axis: {:?}", m);
                    // pushing a by a hair more than the MTV separates the boxes
                    let eps = 1e-3;
                    let push = [m[0] * (1.0 + eps), m[1] * (1.0 + eps), m[2] * (1.0 + eps)];
                    let pa = ab([a.min[0] + push[0], a.min[1] + push[1], a.min[2] + push[2]],
                                [a.max[0] + push[0], a.max[1] + push[1], a.max[2] + push[2]]);
                    assert!(!overlaps(&pa, &b), "MTV did not separate: m={:?}", m);
                }
            }
        }
    }
}
