//! `sim::threat_assess` — which of the things around a creature is the worst
//! threat right now?
//!
//! Each threat is `(distance, power)`. [`threat_score`] turns that into a single
//! danger number — `power / (1 + distance)` — so a threat is scarier the
//! stronger and the nearer it is, and [`top_threat`] returns the index of the
//! most dangerous one (or `None` if there are none). Ties resolve to the lowest
//! index, so the choice is stable and deterministic.
//!
//! Pure: a divide and a max-scan, no clock, no randomness, no cross-imports.

/// Danger score for a threat at `dist` with `power`. Non-negative; `0` for a
/// powerless threat, decreasing with distance. Inputs are floored at `0`.
pub fn threat_score(dist: f32, power: f32) -> f32 {
    let d = if dist > 0.0 { dist } else { 0.0 };
    let p = if power > 0.0 { power } else { 0.0 };
    p / (1.0 + d)
}

/// Index of the highest-scoring threat, or `None` if the slice is empty.
/// Lowest index wins ties.
pub fn top_threat(threats: &[(f32, f32)]) -> Option<usize> {
    let mut best: Option<(f32, usize)> = None;
    let mut i = 0;
    while i < threats.len() {
        let (d, p) = threats[i];
        let s = threat_score(d, p);
        match best {
            Some((bs, _)) if bs >= s => {}
            _ => best = Some((s, i)),
        }
        i += 1;
    }
    match best {
        Some((_, idx)) => Some(idx),
        None => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn empty_is_none() {
        assert_eq!(top_threat(&[]), None);
    }

    #[test]
    fn nearer_and_stronger_wins() {
        // index 1 is both closer and stronger
        let t = [(10.0, 5.0), (1.0, 9.0), (8.0, 4.0)];
        assert_eq!(top_threat(&t), Some(1));
    }

    #[test]
    fn far_weak_ignored_relative_to_near_strong() {
        // a near, strong threat beats a far, weak one
        let t = [(0.5, 8.0), (50.0, 2.0)];
        assert_eq!(top_threat(&t), Some(0));
    }

    #[test]
    fn ties_resolve_to_lowest_index() {
        let t = [(2.0, 3.0), (2.0, 3.0), (2.0, 3.0)];
        assert_eq!(top_threat(&t), Some(0));
    }

    #[test]
    fn scores_are_bounded_and_nonnegative() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..4000 {
            let d = (lcg(&mut seed) % 200_001) as f32 / 100.0 - 1000.0;
            let p = (lcg(&mut seed) % 200_001) as f32 / 100.0 - 1000.0;
            let s = threat_score(d, p);
            assert!(s >= 0.0, "negative score {}", s);
            assert!(s.is_finite(), "non-finite score");
            // never exceeds the (clamped) power
            let pc = if p > 0.0 { p } else { 0.0 };
            assert!(s <= pc + 1e-3, "score {} > power {}", s, pc);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..2000 {
            let n = (lcg(&mut seed) % 10) as usize;
            let mut t = Vec::new();
            for _ in 0..n {
                let d = (lcg(&mut seed) % 100_001) as f32 / 1000.0;
                let p = (lcg(&mut seed) % 100_001) as f32 / 1000.0;
                t.push((d, p));
            }
            assert_eq!(top_threat(&t), top_threat(&t));
        }
    }
}
