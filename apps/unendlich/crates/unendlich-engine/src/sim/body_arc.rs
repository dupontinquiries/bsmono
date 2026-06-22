//! `sim::body_arc` — bending a long body through a turn (per
//! `ENTITY_BODY_SEGMENTATION`). A serpent, a dragon's neck, a centipede: when a
//! large body turns, the head leads and each following spine segment lags a
//! little, so the body sweeps through an arc instead of pivoting rigidly.
//!
//! [`arc`] distributes a commanded `turn` (radians) across `n` spine segments,
//! returning the per-segment yaw from head (index 0) to tail. The head takes the
//! largest share and the tail the least; the per-segment yaws **sum to the
//! commanded turn**, so the whole body ends up facing the right way. A bigger
//! `size` spreads the turn more unevenly (a long body curves more visibly); a
//! single segment just takes the whole turn.
//!
//! Presentation-tier and pure: head-weighted normalized weights, no clock, no
//! randomness, no cross-imports.

fn clampf(x: f32, lo: f32, hi: f32) -> f32 {
    let x = if x > lo { x } else { lo };
    if x < hi {
        x
    } else {
        hi
    }
}

/// Per-segment yaw (head first) distributing `turn` across `n` segments.
///
/// Head-weighted so segment 0 leads and the tail lags; the returned values sum
/// to `turn`. `size` (clamped to `0..10`) controls how head-heavy the spread is.
/// `n == 0` returns empty; `n == 1` returns `[turn]`.
pub fn arc(turn: f32, n: usize, size: f32) -> Vec<f32> {
    if n == 0 {
        return Vec::new();
    }
    if n == 1 {
        return vec![turn];
    }

    let spread = clampf(size, 0.0, 10.0) * 0.5; // bigger body → more head-heavy
    let denom = (n - 1) as f32;

    let mut weights = Vec::with_capacity(n);
    let mut sum = 0.0f32;
    let mut i = 0;
    while i < n {
        let frac = i as f32 / denom; // 0 at head → 1 at tail
        let w = 1.0 + spread * (1.0 - frac); // head heaviest
        weights.push(w);
        sum += w;
        i += 1;
    }

    let mut out = Vec::with_capacity(n);
    let mut j = 0;
    while j < n {
        out.push(turn * weights[j] / sum);
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

    #[test]
    fn head_leads_tail() {
        let a = arc(1.0, 5, 2.0);
        assert!(a[0] > a[4], "head {} should lead tail {}", a[0], a[4]);
        // strictly decreasing head→tail
        let mut i = 1;
        while i < a.len() {
            assert!(a[i] <= a[i - 1] + 1e-7, "not monotone at {}", i);
            i += 1;
        }
    }

    #[test]
    fn segments_sum_to_turn() {
        let mut seed = 0x5111_u32;
        for _ in 0..3000 {
            let turn = (lcg(&mut seed) % 200_001) as f32 / 100_000.0 - 1.0; // [-1,1]
            let n = 1 + (lcg(&mut seed) % 12) as usize;
            let size = (lcg(&mut seed) % 100_001) as f32 / 10_000.0;
            let a = arc(turn, n, size);
            let mut s = 0.0f32;
            for v in &a {
                s += *v;
            }
            assert!((s - turn).abs() < 1e-4, "sum {} != turn {}", s, turn);
        }
    }

    #[test]
    fn bigger_size_spreads_more() {
        let small = arc(1.0, 6, 0.5);
        let big = arc(1.0, 6, 6.0);
        let spread_small = small[0] - small[5];
        let spread_big = big[0] - big[5];
        assert!(spread_big > spread_small, "{} !> {}", spread_big, spread_small);
    }

    #[test]
    fn n_one_is_trivial() {
        assert_eq!(arc(0.7, 1, 5.0), vec![0.7]);
        assert!(arc(0.5, 0, 5.0).is_empty());
    }

    #[test]
    fn bounded_by_turn() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..3000 {
            let turn = (lcg(&mut seed) % 200_001) as f32 / 100_000.0 - 1.0;
            let n = 1 + (lcg(&mut seed) % 12) as usize;
            let size = (lcg(&mut seed) % 100_001) as f32 / 10_000.0;
            for v in arc(turn, n, size) {
                assert!(v.abs() <= turn.abs() + 1e-5, "segment {} exceeds turn {}", v, turn);
            }
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..2000 {
            let turn = (lcg(&mut seed) % 200_001) as f32 / 100_000.0 - 1.0;
            let n = (lcg(&mut seed) % 12) as usize;
            let size = (lcg(&mut seed) % 100_001) as f32 / 10_000.0;
            assert_eq!(arc(turn, n, size), arc(turn, n, size));
        }
    }
}
