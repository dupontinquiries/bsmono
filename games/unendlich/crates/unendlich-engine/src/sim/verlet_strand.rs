//! `sim::verlet_strand` — a Verlet-integrated strand of points with distance
//! constraints, gravity, damping and pinned ends.
//!
//! A reusable soft strand for dragon wing-membrane ribs, tails, vines and ropes.
//! Each [`step`] integrates the free points (Verlet:
//! `x' = x + (x − prev)·(1 − damping) + g·dt²`) then relaxes the inter-point
//! distance constraints toward [`Strand::rest_len`] for `iterations` passes
//! (Gauss–Seidel). Pinned points are never moved — neither by integration nor by
//! constraint relaxation — so a pinned endpoint stays exactly put.
//!
//! Stable (no NaN / explosion) for sane inputs: damping is clamped to `[0,1]`,
//! non-finite `dt`/gravity are neutralised, and coincident points are skipped to
//! avoid division by zero.
//!
//! Pure: data in, data out. No `crate::render`, no `ash`/`vk`, no IO, no clock,
//! no RNG, no `.unwrap()`/`.expect()` in non-test code; compiles clean under
//! `cargo check --release` (no `cfg(test)`/`cfg(debug_assertions)` gated helpers).

/// A strand of points. `points` are current positions, `prev` the previous
/// positions (Verlet velocity is `points − prev`), `rest_len` the target segment
/// length, and `pinned[i]` holds point `i` fixed. All three vecs are the same
/// length.
#[derive(Clone, Debug, PartialEq)]
pub struct Strand {
    pub points: Vec<[f32; 3]>,
    pub prev: Vec<[f32; 3]>,
    pub rest_len: f32,
    pub pinned: Vec<bool>,
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v) // r343 dedup
}

fn finite_or_0(v: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

impl Strand {
    /// A straight strand of `n` evenly spaced points from `start` to `end`, at
    /// rest (`prev == points`, zero velocity), nothing pinned. `rest_len` is the
    /// segment length `|end − start| / (n − 1)`. For `n < 2` the strand is
    /// degenerate (`rest_len = 0`) and [`step`] is a no-op on it.
    pub fn line(start: [f32; 3], end: [f32; 3], n: usize) -> Strand {
        if n == 0 {
            return Strand { points: Vec::new(), prev: Vec::new(), rest_len: 0.0, pinned: Vec::new() };
        }
        if n == 1 {
            return Strand { points: vec![start], prev: vec![start], rest_len: 0.0, pinned: vec![false] };
        }
        let mut points = Vec::with_capacity(n);
        let denom = (n - 1) as f32;
        let mut i = 0;
        while i < n {
            let t = i as f32 / denom;
            points.push([
                start[0] + (end[0] - start[0]) * t,
                start[1] + (end[1] - start[1]) * t,
                start[2] + (end[2] - start[2]) * t,
            ]);
            i += 1;
        }
        let dx = end[0] - start[0];
        let dy = end[1] - start[1];
        let dz = end[2] - start[2];
        let rest_len = (dx * dx + dy * dy + dz * dz).sqrt() / denom;
        let prev = points.clone();
        let pinned = vec![false; n];
        Strand { points, prev, rest_len, pinned }
    }
}

/// Advance the strand one timestep: Verlet-integrate the free points, then relax
/// distance constraints toward `rest_len` for `iterations` passes. Pinned points
/// never move. No-op if the strand has fewer than 2 points or its vecs are
/// mismatched in length.
pub fn step(s: &mut Strand, gravity: [f32; 3], damping01: f32, dt: f32, iterations: u8) {
    let n = s.points.len();
    if n < 2 || s.prev.len() != n || s.pinned.len() != n {
        return; // n<2 ⇒ no-op; mismatched vecs ⇒ refuse rather than panic
    }

    let inv_damp = 1.0 - clamp01(damping01);
    let d = dt * dt;
    let dt2 = if d.is_finite() { d } else { 0.0 };
    let g = [finite_or_0(gravity[0]), finite_or_0(gravity[1]), finite_or_0(gravity[2])];

    // Verlet integration of the free points.
    let mut i = 0;
    while i < n {
        if !s.pinned[i] {
            let p = s.points[i];
            let pr = s.prev[i];
            let mut np = [0.0f32; 3];
            let mut k = 0;
            while k < 3 {
                np[k] = p[k] + (p[k] - pr[k]) * inv_damp + g[k] * dt2;
                k += 1;
            }
            s.points[i] = np;
            s.prev[i] = p;
        }
        i += 1;
    }

    // Distance-constraint relaxation (Gauss–Seidel).
    let rest = s.rest_len;
    let mut it = 0u8;
    while it < iterations {
        let mut a = 0;
        while a + 1 < n {
            let b = a + 1;
            let pa = s.points[a];
            let pb = s.points[b];
            let delta = [pb[0] - pa[0], pb[1] - pa[1], pb[2] - pa[2]];
            let dist = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
            if dist > 1e-9 {
                let diff = (dist - rest) / dist; // signed fractional error
                let a_pinned = s.pinned[a];
                let b_pinned = s.pinned[b];
                if a_pinned && b_pinned {
                    // both fixed: nothing to do
                } else if a_pinned {
                    let mut k = 0;
                    while k < 3 {
                        s.points[b][k] -= delta[k] * diff; // full correction on the free end
                        k += 1;
                    }
                } else if b_pinned {
                    let mut k = 0;
                    while k < 3 {
                        s.points[a][k] += delta[k] * diff;
                        k += 1;
                    }
                } else {
                    let mut k = 0;
                    while k < 3 {
                        let c = delta[k] * 0.5 * diff; // split correction
                        s.points[a][k] += c;
                        s.points[b][k] -= c;
                        k += 1;
                    }
                }
            }
            a += 1;
        }
        it += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
        let d = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
    }
    fn max_velocity(s: &Strand) -> f32 {
        let mut m = 0.0f32;
        for i in 0..s.points.len() {
            let v = dist(s.points[i], s.prev[i]);
            if v > m {
                m = v;
            }
        }
        m
    }
    fn all_finite(s: &Strand) -> bool {
        s.points.iter().all(|p| p.iter().all(|v| v.is_finite()))
            && s.prev.iter().all(|p| p.iter().all(|v| v.is_finite()))
    }

    #[test]
    fn line_constructor_is_evenly_spaced_at_rest() {
        let s = Strand::line([0.0, 0.0, 0.0], [3.0, 0.0, 0.0], 4);
        assert_eq!(s.points.len(), 4);
        assert_eq!(s.prev, s.points, "starts at rest (zero velocity)");
        assert!((s.rest_len - 1.0).abs() < 1e-6, "rest_len = 3/3 = 1");
        assert!(s.pinned.iter().all(|&p| !p));
        for i in 0..3 {
            assert!((dist(s.points[i], s.points[i + 1]) - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn n_less_than_two_is_noop() {
        let mut s0 = Strand::line([0.0; 3], [1.0; 3], 0);
        let before0 = s0.clone();
        step(&mut s0, [0.0, -9.8, 0.0], 0.01, 0.016, 8);
        assert_eq!(s0, before0);

        let mut s1 = Strand::line([1.0, 2.0, 3.0], [4.0, 5.0, 6.0], 1);
        let before1 = s1.clone();
        step(&mut s1, [0.0, -9.8, 0.0], 0.01, 0.016, 8);
        assert_eq!(s1, before1, "single point must not move");
    }

    #[test]
    fn pinned_endpoint_stays_exactly_put() {
        let mut s = Strand::line([0.0, 10.0, 0.0], [0.0, 4.0, 0.0], 7);
        s.pinned[0] = true;
        let anchor = s.points[0];
        for _ in 0..500 {
            step(&mut s, [0.3, -9.8, 0.1], 0.02, 0.016, 12);
        }
        assert_eq!(s.points[0], anchor, "pinned point must stay bit-exactly put");
        assert_eq!(s.prev[0], anchor);

        // pin both ends
        let mut s2 = Strand::line([0.0, 0.0, 0.0], [6.0, 0.0, 0.0], 6);
        s2.pinned[0] = true;
        s2.pinned[5] = true;
        let a0 = s2.points[0];
        let a5 = s2.points[5];
        for _ in 0..500 {
            step(&mut s2, [0.0, -9.8, 0.0], 0.02, 0.016, 16);
        }
        assert_eq!(s2.points[0], a0);
        assert_eq!(s2.points[5], a5);
    }

    #[test]
    fn lengths_converge_to_rest_len_after_relaxation() {
        // strand stretched to 2× rest spacing, top pinned; kill velocity (damping
        // 1, gravity 0) and relax many iterations -> segments reach rest_len.
        let mut s = Strand::line([0.0, 0.0, 0.0], [10.0, 0.0, 0.0], 6); // rest_len 2
        // stretch: spread points to spacing 4 (2× rest)
        for i in 0..6 {
            s.points[i] = [i as f32 * 4.0, 0.0, 0.0];
            s.prev[i] = s.points[i];
        }
        s.pinned[0] = true;
        for _ in 0..200 {
            step(&mut s, [0.0, 0.0, 0.0], 1.0, 0.0, 32);
        }
        for i in 0..5 {
            let seg = dist(s.points[i], s.points[i + 1]);
            assert!((seg - s.rest_len).abs() < 1e-2, "segment {} = {} not ~rest_len {}", i, seg, s.rest_len);
        }
    }

    #[test]
    fn hanging_strand_settles_and_stops() {
        let mut s = Strand::line([0.0, 10.0, 0.0], [0.0, 4.0, 0.0], 8);
        s.pinned[0] = true;
        for _ in 0..8000 {
            step(&mut s, [0.0, -9.8, 0.0], 0.15, 0.016, 24);
        }
        assert!(all_finite(&s));
        // velocity has decayed to near zero (settled)
        assert!(max_velocity(&s) < 1e-2, "strand should stop, max vel = {}", max_velocity(&s));
        // and it hangs taut: segment lengths ~ rest_len
        for i in 0..s.points.len() - 1 {
            let seg = dist(s.points[i], s.points[i + 1]);
            assert!((seg - s.rest_len).abs() < 5e-2, "settled segment {} = {} vs rest {}", i, seg, s.rest_len);
        }
        // hangs below the anchor
        assert!(s.points[s.points.len() - 1][1] < s.points[0][1]);
    }

    #[test]
    fn positions_stay_finite_over_1000_steps() {
        let mut s = Strand::line([0.0, 5.0, 0.0], [5.0, 5.0, 1.0], 12);
        s.pinned[0] = true;
        for _ in 0..1000 {
            step(&mut s, [0.0, -9.8, 0.0], 0.02, 1.0 / 60.0, 10);
            assert!(all_finite(&s));
        }
    }

    #[test]
    fn unpinned_strand_falls_under_gravity() {
        let mut s = Strand::line([0.0, 0.0, 0.0], [4.0, 0.0, 0.0], 5);
        let y_before: Vec<f32> = s.points.iter().map(|p| p[1]).collect();
        step(&mut s, [0.0, -9.8, 0.0], 0.0, 0.1, 4);
        for i in 0..s.points.len() {
            assert!(s.points[i][1] < y_before[i], "point {} should fall", i);
        }
    }

    #[test]
    fn coincident_points_do_not_nan() {
        // all points at the same spot (zero-length segments) must not divide by 0
        let mut s = Strand::line([1.0, 1.0, 1.0], [1.0, 1.0, 1.0], 5); // start==end
        for i in 0..5 {
            s.points[i] = [1.0, 1.0, 1.0];
            s.prev[i] = [1.0, 1.0, 1.0];
        }
        for _ in 0..100 {
            step(&mut s, [0.0, -9.8, 0.0], 0.1, 0.016, 8);
        }
        assert!(all_finite(&s));
    }

    #[test]
    fn deterministic() {
        let make = || {
            let mut s = Strand::line([0.0, 8.0, 0.0], [3.0, 2.0, 1.0], 10);
            s.pinned[0] = true;
            s
        };
        let mut a = make();
        let mut b = make();
        for _ in 0..1000 {
            step(&mut a, [0.1, -9.8, 0.0], 0.05, 0.016, 12);
            step(&mut b, [0.1, -9.8, 0.0], 0.05, 0.016, 12);
        }
        assert_eq!(a, b);
    }

    #[test]
    fn damping_clamped_and_nonfinite_inputs_safe() {
        let mut s = Strand::line([0.0, 5.0, 0.0], [2.0, 5.0, 0.0], 5);
        s.pinned[0] = true;
        // out-of-range damping and non-finite dt/gravity must not explode
        for _ in 0..200 {
            step(&mut s, [f32::NAN, -9.8, f32::INFINITY], 5.0, f32::NAN, 8);
        }
        assert!(all_finite(&s));
    }
}
