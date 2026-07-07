//! `sim::threat_share` — split a threat value across pack members by how close
//! each is.
//!
//! [`share`] distributes `total` threat over members at the given `dists`, giving
//! nearer members a larger share (weight `1/(1+dist)`) and normalizing so the
//! shares sum back to `total`. Members at equal distance get equal shares; an
//! empty pack yields an empty result. Useful for splitting a creature's aggro or
//! a hazard's danger among the nearby pack.
//!
//! Pure and deterministic: inverse-distance weights and one normalization — no
//! clock, no randomness, no cross-imports.

fn nonneg(x: f32) -> f32 {
    if x > 0.0 {
        x
    } else {
        0.0
    }
}

/// Distribute `total` across members at `dists`, nearer getting more.
///
/// Share `i` is `total · w_i / Σw` with `w_i = 1/(1 + dist_i)`. Empty `dists`
/// returns an empty vector; the shares sum to `total`.
pub fn share(total: f32, dists: &[f32]) -> Vec<f32> {
    if dists.is_empty() {
        return Vec::new();
    }
    let mut weights = Vec::with_capacity(dists.len());
    let mut sum = 0.0f32;
    let mut i = 0;
    while i < dists.len() {
        let w = 1.0 / (1.0 + nonneg(dists[i])); // (0,1], larger when nearer
        weights.push(w);
        sum += w;
        i += 1;
    }
    let mut out = Vec::with_capacity(dists.len());
    let mut j = 0;
    while j < dists.len() {
        out.push(total * weights[j] / sum); // sum > 0 for non-empty
        j += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn total_of(v: &[f32]) -> f32 {
        let mut s = 0.0;
        let mut i = 0;
        while i < v.len() {
            s += v[i];
            i += 1;
        }
        s
    }

    #[test]
    fn sums_to_total() {
        let mut seed = 0x5111_u32;
        for _ in 0..2000 {
            let total = (lcg(&mut seed) % 100_001) as f32 / 100.0;
            let n = 1 + (lcg(&mut seed) % 8) as usize;
            let mut dists = Vec::new();
            for _ in 0..n {
                dists.push((lcg(&mut seed) % 100_001) as f32 / 100.0);
            }
            let s = share(total, &dists);
            assert!((total_of(&s) - total).abs() < 1e-2, "sum {} != {}", total_of(&s), total);
        }
    }

    #[test]
    fn nearer_gets_more() {
        let s = share(100.0, &[1.0, 10.0, 100.0]);
        assert!(s[0] > s[1] && s[1] > s[2], "{:?}", s);
    }

    #[test]
    fn empty_is_empty() {
        assert!(share(100.0, &[]).is_empty());
    }

    #[test]
    fn equal_dists_equal_share() {
        let s = share(90.0, &[5.0, 5.0, 5.0]);
        assert!((s[0] - 30.0).abs() < 1e-3);
        assert!((s[1] - 30.0).abs() < 1e-3);
        assert!((s[2] - 30.0).abs() < 1e-3);
    }

    #[test]
    fn bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..3000 {
            let total = (lcg(&mut seed) % 100_001) as f32 / 100.0;
            let n = 1 + (lcg(&mut seed) % 8) as usize;
            let mut dists = Vec::new();
            for _ in 0..n {
                dists.push((lcg(&mut seed) % 100_001) as f32 / 100.0);
            }
            for v in share(total, &dists) {
                assert!(v >= 0.0 && v <= total + 1e-3, "share {} out of [0,{}]", v, total);
            }
        }
    }

    #[test]
    fn deterministic() {
        let dists = [1.0, 3.0, 7.0, 2.0];
        assert_eq!(share(50.0, &dists), share(50.0, &dists));
    }
}
