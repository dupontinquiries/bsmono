//! `chunk_priority` — load/upload priority for chunks at distance.
//!
//! Worker-03 / WORLD-PERF pure core, round 11. Scores chunks so the streamer
//! services the most useful first: nearer chunks outrank far ones, anything in
//! the view frustum is boosted (the player sees it now), and dirty chunks
//! (edited, awaiting re-mesh/upload) are boosted so visible edits land quickly.
//! `order` sorts a work list by descending priority, stably.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Added when a chunk is inside the view frustum.
pub const FRUSTUM_BOOST: f32 = 1.0;
/// Added when a chunk has pending edits.
pub const DIRTY_BOOST: f32 = 0.5;

/// Priority for a chunk. Higher = service sooner. The distance term is
/// `1/(1+dist)` ∈ (0,1], so nearer chunks score higher; frustum/dirty add
/// fixed boosts. Bounded in `[0, 1 + FRUSTUM_BOOST + DIRTY_BOOST]`.
pub fn priority(dist: f32, in_frustum: bool, dirty: bool) -> f32 {
    let d = if dist.is_nan() || dist < 0.0 { 0.0 } else { dist };
    let mut p = 1.0 / (1.0 + d);
    if in_frustum {
        p += FRUSTUM_BOOST;
    }
    if dirty {
        p += DIRTY_BOOST;
    }
    p
}

/// Stable-sort a work list `(id, priority)` by descending priority. Ties keep
/// their original order (so a secondary key the caller pre-sorted by survives).
pub fn order(items: &mut Vec<(usize, f32)>) {
    items.sort_by(|a, b| {
        if a.1 > b.1 {
            std::cmp::Ordering::Less
        } else if a.1 < b.1 {
            std::cmp::Ordering::Greater
        } else {
            std::cmp::Ordering::Equal
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearer_is_higher() {
        assert!(priority(1.0, false, false) > priority(10.0, false, false));
        assert!(priority(0.0, false, false) > priority(100.0, false, false));
    }

    #[test]
    fn in_frustum_boosts() {
        assert!(priority(5.0, true, false) > priority(5.0, false, false));
    }

    #[test]
    fn dirty_boosts() {
        assert!(priority(5.0, false, true) > priority(5.0, false, false));
        // A near, in-frustum, dirty chunk beats a near plain chunk.
        assert!(priority(5.0, true, true) > priority(5.0, true, false));
    }

    #[test]
    fn bounded() {
        let max = 1.0 + FRUSTUM_BOOST + DIRTY_BOOST;
        for &d in &[0.0f32, 1.0, 50.0, 1e6] {
            for &f in &[true, false] {
                for &dy in &[true, false] {
                    let p = priority(d, f, dy);
                    assert!(p >= 0.0 && p <= max + 1e-6);
                }
            }
        }
    }

    #[test]
    fn order_is_descending() {
        let mut v = vec![(0, 0.2f32), (1, 0.9), (2, 0.5)];
        order(&mut v);
        assert_eq!(v.iter().map(|x| x.0).collect::<Vec<_>>(), vec![1, 2, 0]);
    }

    #[test]
    fn order_is_stable_on_ties() {
        // Equal priorities keep insertion order (ids 5, 3, 8).
        let mut v = vec![(5, 1.0f32), (3, 1.0), (8, 1.0), (2, 2.0)];
        order(&mut v);
        assert_eq!(v.iter().map(|x| x.0).collect::<Vec<_>>(), vec![2, 5, 3, 8]);
    }

    #[test]
    fn deterministic() {
        assert_eq!(priority(3.0, true, false), priority(3.0, true, false));
        let mut a = vec![(0, 0.3f32), (1, 0.3), (2, 0.7)];
        let mut b = a.clone();
        order(&mut a);
        order(&mut b);
        assert_eq!(a, b);
    }
}
