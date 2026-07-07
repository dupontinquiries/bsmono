//! `sim::shadow_refresh_plan` — temporal planner that spreads shadow re-renders.
//!
//! Real M2 data: shadow-pass p50 is 0.5ms but p99 is ~29.6ms — cascades
//! re-render in clusters on the same frame, spiking the tail. This planner picks
//! which casters to re-render *this* frame: the most overdue first, capped at
//! `max_refresh`, so the spiky cost becomes a flat per-frame trickle while every
//! caster still refreshes within ~`stride` frames. It honours the existing
//! `UNENDLICH_SHADOW_CHUNK_STRIDE` / `SHADOW_CACHE` flags (the caller passes the
//! stride). Pure, deterministic — the frame counter *is* the clock, no
//! wall-clock (R1/R6); standalone `rustc --test`-able, release-safe.

/// One shadow-casting chunk and when it last refreshed.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct ShadowChunk {
    pub id: u32,
    /// Frame index of its last shadow re-render.
    pub last_refresh_frame: u32,
    /// Re-render priority (near / large casters score higher).
    pub importance: f32,
}

/// Choose up to `max_refresh` casters to re-render on `frame`, writing their ids
/// into `out` (cleared first). A caster is *overdue* when it hasn't refreshed for
/// at least `stride` frames; only overdue casters are chosen, most-overdue first,
/// then most-important, then lowest id (a total, deterministic order). Frame
/// wraparound is handled with wrapping subtraction, so the planner is safe across
/// the `u32` frame-counter rollover.
pub fn plan(chunks: &[ShadowChunk], frame: u32, stride: u32, max_refresh: usize, out: &mut Vec<u32>) {
    out.clear();
    if max_refresh == 0 || chunks.is_empty() {
        return;
    }

    // age via wrapping_sub so a wrapped frame counter still measures elapsed frames
    let mut cand: Vec<(u32, &ShadowChunk)> = chunks
        .iter()
        .map(|c| (frame.wrapping_sub(c.last_refresh_frame), c))
        .filter(|(age, _)| *age >= stride)
        .collect();

    // most overdue (age desc) → most important (desc) → lowest id (asc).
    // total_cmp gives a total order over importance incl. NaN, so the sort is
    // deterministic regardless of input.
    cand.sort_by(|(age_a, a), (age_b, b)| {
        age_b
            .cmp(age_a)
            .then_with(|| b.importance.total_cmp(&a.importance))
            .then_with(|| a.id.cmp(&b.id))
    });

    for (_, c) in cand.iter().take(max_refresh) {
        out.push(c.id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ch(id: u32, last: u32, imp: f32) -> ShadowChunk {
        ShadowChunk { id, last_refresh_frame: last, importance: imp }
    }

    #[test]
    fn never_exceeds_max_refresh() {
        // 10 casters all long overdue, cap 3
        let chunks: Vec<ShadowChunk> = (0..10).map(|i| ch(i, 0, i as f32)).collect();
        let mut out = Vec::new();
        plan(&chunks, 100, 5, 3, &mut out);
        assert_eq!(out.len(), 3);
    }

    #[test]
    fn overdue_prioritised_over_fresh() {
        // id 1 overdue (last 0, age 100), id 2 fresh (last 99, age 1)
        let chunks = [ch(1, 0, 0.1), ch(2, 99, 9.0)];
        let mut out = Vec::new();
        plan(&chunks, 100, 5, 1, &mut out);
        assert_eq!(out, vec![1], "overdue should win even over a more important fresh caster");
    }

    #[test]
    fn ties_broken_by_importance_then_id() {
        // same age (all last 0, frame 100): importance discriminates, then id
        let chunks = [ch(5, 0, 0.5), ch(7, 0, 0.9), ch(3, 0, 0.9)];
        let mut out = Vec::new();
        plan(&chunks, 100, 5, 3, &mut out);
        // importance 0.9 first (ids 3 & 7, lower id first), then 0.5
        assert_eq!(out, vec![3, 7, 5]);
    }

    #[test]
    fn most_overdue_first() {
        // different ages; most overdue should lead regardless of importance
        let chunks = [ch(1, 90, 9.0), ch(2, 10, 0.1)]; // frame 100: ages 10 vs 90
        let mut out = Vec::new();
        plan(&chunks, 100, 5, 2, &mut out);
        assert_eq!(out, vec![2, 1], "the more overdue caster leads");
    }

    #[test]
    fn nothing_overdue_selects_fewer() {
        // all refreshed last frame (age 1 < stride 5)
        let chunks = [ch(1, 99, 1.0), ch(2, 99, 2.0)];
        let mut out = Vec::new();
        plan(&chunks, 100, 5, 4, &mut out);
        assert!(out.is_empty(), "nothing overdue ⇒ select none");
    }

    #[test]
    fn out_cleared_first_and_empty_input() {
        let mut out = vec![999, 998]; // junk
        plan(&[], 10, 5, 3, &mut out);
        assert!(out.is_empty(), "empty input + clear ⇒ empty out");
        // junk also cleared when there IS input
        let mut out2 = vec![42];
        plan(&[ch(1, 0, 1.0)], 100, 5, 3, &mut out2);
        assert_eq!(out2, vec![1], "prior contents must be cleared");
    }

    #[test]
    fn frame_wraparound_no_panic() {
        // last refresh just before u32 wrap; frame wrapped to 2 → age 4 (>= stride 3)
        let chunks = [ch(1, u32::MAX - 1, 1.0)];
        let mut out = Vec::new();
        plan(&chunks, 2, 3, 1, &mut out);
        assert_eq!(out, vec![1], "wraparound age should be measured correctly");
    }

    #[test]
    fn deterministic_over_many() {
        let mut s = 0x37FA_u64; // documented seed (R20)
        let mut rng = || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
            s
        };
        for _ in 0..1000 {
            let n = (rng() % 40) as u32;
            let chunks: Vec<ShadowChunk> = (0..n)
                .map(|i| ch(i, (rng() % 200) as u32, (rng() % 1000) as f32 / 100.0))
                .collect();
            let frame = (rng() % 1000) as u32;
            let stride = (rng() % 10) as u32;
            let maxr = (rng() % 8) as usize;
            let mut a = Vec::new();
            let mut b = Vec::new();
            plan(&chunks, frame, stride, maxr, &mut a);
            plan(&chunks, frame, stride, maxr, &mut b);
            assert_eq!(a, b, "non-deterministic");
            assert!(a.len() <= maxr, "exceeded max_refresh");
        }
    }
}
