//! Scalar interpolation, easing, and angle helpers.
//!
//! `lerp` is the one function this repo has re-written most often (soldier
//! alone carries ~10 private copies across its examples). The easing family
//! covers the fleet's UI habits — logscope/paperbound's "150 ms ink expand
//! with a whisker of overshoot" is `ease_out_back`.

/// Linear interpolation: `a` at `t = 0`, `b` at `t = 1`. `t` is NOT clamped.
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Inverse lerp: where does `x` sit between `a` and `b`? (0 at `a`, 1 at `b`.)
/// Returns 0 when `a == b`. Not clamped.
#[inline]
pub fn inv_lerp(a: f32, b: f32, x: f32) -> f32 {
    let d = b - a;
    if d == 0.0 {
        0.0
    } else {
        (x - a) / d
    }
}

/// Map `x` from range [`a0`,`b0`] into [`a1`,`b1`] (not clamped).
#[inline]
pub fn remap(x: f32, a0: f32, b0: f32, a1: f32, b1: f32) -> f32 {
    lerp(a1, b1, inv_lerp(a0, b0, x))
}

/// Clamp to [0, 1].
#[inline]
pub fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

/// Hermite `t²(3−2t)` on an already-unit input — the `vsmooth`/`smooth` of
/// soldier mesh.rs and logscope texgen.rs. `t` is NOT clamped (matches them).
#[inline]
pub fn smooth01(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

/// GLSL-style smoothstep: 0 below `e0`, 1 above `e1`, hermite between.
#[inline]
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    smooth01(clamp01(inv_lerp(e0, e1, x)))
}

/// Perlin's quintic `t³(t(6t−15)+10)` — C² continuous. `t` is NOT clamped.
#[inline]
pub fn smoother01(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}

// ── easing (unit-domain: t in [0,1] → [0,1]; not clamped) ────────────────────

#[inline]
pub fn ease_in_quad(t: f32) -> f32 {
    t * t
}
#[inline]
pub fn ease_out_quad(t: f32) -> f32 {
    1.0 - (1.0 - t) * (1.0 - t)
}
#[inline]
pub fn ease_in_out_quad(t: f32) -> f32 {
    if t < 0.5 {
        2.0 * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(2) / 2.0
    }
}
#[inline]
pub fn ease_in_cubic(t: f32) -> f32 {
    t * t * t
}
#[inline]
pub fn ease_out_cubic(t: f32) -> f32 {
    1.0 - (1.0 - t).powi(3)
}
#[inline]
pub fn ease_in_out_cubic(t: f32) -> f32 {
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

/// Ease-out with a small overshoot past 1 before settling — the "decisive pop"
/// used by ink-expand UI. `s` is the overshoot strength; 1.70158 is the classic
/// ~10% back constant.
#[inline]
pub fn ease_out_back(t: f32) -> f32 {
    const S: f32 = 1.70158;
    let u = t - 1.0;
    1.0 + u * u * ((S + 1.0) * u + S)
}

// ── angles ───────────────────────────────────────────────────────────────────

/// Wrap an angle (radians) into (−π, π].
#[inline]
pub fn wrap_pi(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let mut a = a % TAU;
    if a > PI {
        a -= TAU;
    }
    if a <= -PI {
        a += TAU;
    }
    a
}

/// Interpolate between two angles along the SHORTEST arc.
#[inline]
pub fn lerp_angle(a: f32, b: f32, t: f32) -> f32 {
    a + wrap_pi(b - a) * t
}

/// `|a − b| ≤ eps`.
#[inline]
pub fn approx_eq(a: f32, b: f32, eps: f32) -> bool {
    (a - b).abs() <= eps
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn lerp_family() {
        assert_eq!(lerp(2.0, 6.0, 0.25), 3.0);
        assert_eq!(inv_lerp(2.0, 6.0, 3.0), 0.25);
        assert_eq!(inv_lerp(5.0, 5.0, 9.0), 0.0); // degenerate range
        assert_eq!(remap(3.0, 2.0, 6.0, 10.0, 20.0), 12.5);
        // lerp/inv_lerp are inverses on a proper range
        for i in 0..=10 {
            let t = i as f32 / 10.0;
            assert!(approx_eq(inv_lerp(-3.0, 9.0, lerp(-3.0, 9.0, t)), t, 1e-6));
        }
    }

    #[test]
    fn steps_hit_edges_and_midpoint() {
        for f in [smooth01 as fn(f32) -> f32, smoother01] {
            assert_eq!(f(0.0), 0.0);
            assert_eq!(f(1.0), 1.0);
            assert!(approx_eq(f(0.5), 0.5, 1e-6));
        }
        assert_eq!(smoothstep(2.0, 4.0, 1.0), 0.0); // clamps below
        assert_eq!(smoothstep(2.0, 4.0, 9.0), 1.0); // clamps above
        assert!(approx_eq(smoothstep(2.0, 4.0, 3.0), 0.5, 1e-6));
    }

    #[test]
    fn easing_endpoints_and_monotonicity() {
        let fns: [fn(f32) -> f32; 7] = [
            ease_in_quad,
            ease_out_quad,
            ease_in_out_quad,
            ease_in_cubic,
            ease_out_cubic,
            ease_in_out_cubic,
            ease_out_back,
        ];
        for f in fns {
            assert!(approx_eq(f(0.0), 0.0, 1e-6));
            assert!(approx_eq(f(1.0), 1.0, 1e-6));
        }
        // all but ease_out_back are monotone; out_back overshoots by design
        for f in &fns[..6] {
            let mut prev = f(0.0);
            for i in 1..=100 {
                let v = f(i as f32 / 100.0);
                assert!(v >= prev - 1e-6, "non-monotone");
                prev = v;
            }
        }
        // the overshoot that makes ease_out_back "pop"
        assert!(ease_out_back(0.8) > 1.0);
    }

    #[test]
    fn angle_wrapping_takes_the_short_way() {
        assert!(approx_eq(wrap_pi(3.0 * PI), PI, 1e-5));
        // −3π sits ON the branch boundary; f32 remainder may land an ulp either
        // side, so only the magnitude is pinned there
        assert!(approx_eq(wrap_pi(-3.0 * PI).abs(), PI, 1e-5));
        // …but strictly inside the range, wrapping is exact: −3π+0.2 ≡ −π+0.2
        assert!(approx_eq(wrap_pi(-3.0 * PI + 0.2), -PI + 0.2, 1e-5));
        // 350° → 10°: short arc passes through 0, not 180
        let a = 350f32.to_radians();
        let b = 10f32.to_radians();
        let mid = lerp_angle(a, b, 0.5);
        assert!(approx_eq(wrap_pi(mid), 0.0, 1e-5));
    }
}
