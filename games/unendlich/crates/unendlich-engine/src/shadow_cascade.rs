//! shadow_cascade.rs — pure cascaded-shadow split distances.
//!
//! The practical split scheme (PSSM): cascade boundaries are a blend of a
//! uniform split (even in view depth) and a logarithmic split (even in
//! perspective), controlled by `lambda`. Plus a lookup from depth to cascade
//! index. This is the shadow-pass perf lever. Pure, deterministic, panic-free
//! (R1/R11/R21).

/// Far-end split distance of each of the `n` cascades, from `near` to `far`.
/// `lambda` blends uniform (`0`) and logarithmic (`1`) splits. The returned
/// `Vec` has `n` ascending values, all in `(near, far]`, with the last equal to
/// `far`. `n == 0` yields an empty `Vec`.
pub fn splits(near: f32, far: f32, n: u32, lambda: f32) -> Vec<f32> {
    if n == 0 {
        return Vec::new();
    }
    let near = near.max(1e-4);
    let far = far.max(near * (1.0 + 1e-4));
    let lambda = lambda.max(0.0).min(1.0);
    let mut out = Vec::with_capacity(n as usize);
    for i in 1..=n {
        let f = i as f32 / n as f32;
        let c_uniform = near + (far - near) * f;
        let c_log = near * (far / near).powf(f);
        out.push(lambda * c_log + (1.0 - lambda) * c_uniform);
    }
    out
}

/// Cascade index for a view-space `depth`: the first cascade whose far split is
/// `>= depth`. Depths beyond the last split clamp to the last cascade; an empty
/// `splits` slice yields `0`.
pub fn cascade_of(depth: f32, splits: &[f32]) -> u32 {
    if splits.is_empty() {
        return 0;
    }
    for (i, &s) in splits.iter().enumerate() {
        if depth <= s {
            return i as u32;
        }
    }
    (splits.len() - 1) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn n_splits_ascending_in_range() {
        let (near, far) = (0.5f32, 500.0f32);
        for n in 1..=8u32 {
            let s = splits(near, far, n, 0.5);
            assert_eq!(s.len(), n as usize);
            let mut prev = near;
            for &v in &s {
                assert!(v > prev - 1e-3, "not ascending: {} after {}", v, prev);
                assert!(v >= near - 1e-3 && v <= far + 1e-2, "out of range: {}", v);
                prev = v;
            }
            assert!((s[s.len() - 1] - far).abs() < 1e-1, "last split should be far");
        }
    }

    #[test]
    fn lambda_zero_is_uniform() {
        let s = splits(0.0, 400.0, 4, 0.0);
        // equal spacing
        let d0 = s[0];
        for i in 1..s.len() {
            assert!(((s[i] - s[i - 1]) - d0).abs() < 1e-2, "uniform spacing broken");
        }
    }

    #[test]
    fn lambda_one_is_logarithmic() {
        let s = splits(1.0, 1000.0, 5, 1.0);
        // constant ratio between successive splits
        let r0 = s[0]; // == near*(far/near)^(1/n); ratio == s[i]/s[i-1]
        for i in 1..s.len() {
            let r = s[i] / s[i - 1];
            assert!((r - r0).abs() / r0 < 1e-2, "log ratio not constant: {} vs {}", r, r0);
        }
    }

    #[test]
    fn cascade_of_monotonic_and_clamps() {
        let s = splits(1.0, 100.0, 4, 0.5);
        let mut prev = 0u32;
        let mut d = 0.0f32;
        while d <= 200.0 {
            let c = cascade_of(d, &s);
            assert!(c >= prev, "cascade decreased at depth {}", d);
            assert!((c as usize) < s.len());
            prev = c;
            d += 0.5;
        }
        assert_eq!(cascade_of(-5.0, &s), 0, "below near -> cascade 0");
        assert_eq!(cascade_of(1e9, &s), (s.len() - 1) as u32, "beyond far -> last");
    }

    #[test]
    fn single_cascade_case() {
        let s = splits(1.0, 100.0, 1, 0.5);
        assert_eq!(s.len(), 1);
        assert!((s[0] - 100.0).abs() < 1e-1);
        assert_eq!(cascade_of(10.0, &s), 0);
        assert_eq!(cascade_of(1000.0, &s), 0);
    }

    #[test]
    fn deterministic() {
        assert_eq!(splits(0.5, 300.0, 4, 0.7), splits(0.5, 300.0, 4, 0.7));
        let s = splits(0.5, 300.0, 4, 0.7);
        assert_eq!(cascade_of(42.0, &s), cascade_of(42.0, &s));
    }
}
