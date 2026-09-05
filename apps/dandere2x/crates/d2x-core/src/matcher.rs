//! Where in the previous frame does this block of the next frame come from?
//!
//! Every matcher gets the gate's threshold as a `limit` and may stop scoring
//! a candidate the moment it exceeds it — a rejected candidate usually costs
//! one or two rows of a block, not the whole block.
//!
//! Upstream ships exactly [`Stationary`] (its "exhaustive" search tests one
//! point). [`Diamond`] is the motion search upstream started and never
//! finished; [`Exhaustive`] is the reference the other two are checked
//! against in tests.

use crate::frame::{block_sse_bounded, Frame};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Candidate {
    pub src_x: u32,
    pub src_y: u32,
    pub sse: u64,
}

pub trait BlockMatcher: Send + Sync {
    /// Best source position in `prev` for the `size`×`size` block of `next`
    /// at `(dst_x, dst_y)`, among candidates with `sse <= limit`. `None`
    /// if nothing qualifies.
    fn search(&self, prev: &Frame, next: &Frame, dst_x: u32, dst_y: u32, size: u32, limit: u64) -> Option<Candidate>;
    /// [`search`](Self::search) with a motion hint: the block's source is
    /// expected near `(dst_x + hint.0, dst_y + hint.1)`. A local search
    /// starts there and, failing that, falls back to the unhinted search
    /// (a moving actor does not follow the camera). Matchers without a
    /// notion of a start ignore the hint.
    #[allow(clippy::too_many_arguments)]
    fn search_from(&self, prev: &Frame, next: &Frame, dst_x: u32, dst_y: u32, size: u32, limit: u64, hint: (i64, i64)) -> Option<Candidate> {
        let _ = hint;
        self.search(prev, next, dst_x, dst_y, size, limit)
    }
    fn name(&self) -> &'static str;
}

#[inline]
fn test(prev: &Frame, next: &Frame, dst_x: u32, dst_y: u32, sx: i64, sy: i64, size: u32, limit: u64) -> Option<Candidate> {
    if sx < 0 || sy < 0 {
        return None;
    }
    let (sx, sy) = (sx as u32, sy as u32);
    if !prev.block_in_bounds(sx, sy, size) {
        return None;
    }
    block_sse_bounded(next, dst_x, dst_y, prev, sx, sy, size, limit).map(|sse| Candidate { src_x: sx, src_y: sy, sse })
}

/// Zero-motion only. What upstream does.
#[derive(Debug, Default, Clone, Copy)]
pub struct Stationary;

impl BlockMatcher for Stationary {
    fn search(&self, prev: &Frame, next: &Frame, dst_x: u32, dst_y: u32, size: u32, limit: u64) -> Option<Candidate> {
        test(prev, next, dst_x, dst_y, dst_x as i64, dst_y as i64, size, limit)
    }
    fn name(&self) -> &'static str {
        "stationary"
    }
}

/// Every offset in `[-radius, radius]²`. Quadratic; the reference for the
/// heuristic searches, and a reasonable choice for very small radii.
#[derive(Debug, Clone, Copy)]
pub struct Exhaustive {
    pub radius: u32,
}

impl BlockMatcher for Exhaustive {
    fn search(&self, prev: &Frame, next: &Frame, dst_x: u32, dst_y: u32, size: u32, limit: u64) -> Option<Candidate> {
        let r = self.radius as i64;
        let mut best: Option<Candidate> = None;
        let mut lim = limit;
        for dy in -r..=r {
            for dx in -r..=r {
                if let Some(c) = test(prev, next, dst_x, dst_y, dst_x as i64 + dx, dst_y as i64 + dy, size, lim) {
                    if best.map_or(true, |b| c.sse < b.sse) {
                        lim = c.sse; // every later candidate must beat this
                        best = Some(c);
                    }
                }
            }
        }
        best
    }
    fn name(&self) -> &'static str {
        "exhaustive"
    }
}

/// Diamond search (Zhu & Ma): a large diamond (9 points, step 2) walks
/// toward the minimum until the centre wins, then a small diamond (5
/// points, step 1) refines. Roughly 15–25 SSEs per block regardless of
/// motion length, against `(2r+1)²` for the exhaustive window.
///
/// It is a local search: it finds the basin it starts in. Motion of more
/// than ~8 px on repetitive texture can land in a neighbouring basin, in
/// which case the block simply fails the gate and is upscaled — never a
/// wrong picture, only a missed saving. Motion-vector predictors from
/// neighbouring blocks would be the next step if that shows up in practice.
#[derive(Debug, Clone, Copy)]
pub struct Diamond {
    /// Give up after this many large-diamond moves (bounds pathological cases).
    pub max_steps: u32,
    /// Never look further than this from the block's own position.
    pub radius: u32,
}

impl Default for Diamond {
    fn default() -> Self {
        Diamond { max_steps: 16, radius: 32 }
    }
}

const LDSP: [(i64, i64); 9] = [(0, 0), (2, 0), (-2, 0), (0, 2), (0, -2), (1, 1), (-1, 1), (1, -1), (-1, -1)];
const SDSP: [(i64, i64); 5] = [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)];

impl Diamond {
    /// The walk, over any probe function — `search` hands it the SSE test;
    /// tests hand it a counter. Every (x, y) is probed AT MOST ONCE: `(0,0)`
    /// is in the large pattern on every step, and after a move four of the
    /// nine new points were the previous step's, so without the visited
    /// list close to half of a moved block's probes were repeats of a
    /// block SSE already known. `visited` is a flat array (at most
    /// 9·max_steps + 5 entries) scanned linearly: a scan of a few dozen
    /// pairs against a 30×30×3 SSE is not a contest.
    pub fn walk(&self, ox: i64, oy: i64, limit: u64, mut eval: impl FnMut(i64, i64, u64) -> Option<Candidate>) -> Option<Candidate> {
        let r = self.radius as i64;
        let within = |x: i64, y: i64| (x - ox).abs() <= r && (y - oy).abs() <= r;
        let mut visited: Vec<(i64, i64)> = Vec::with_capacity(9 * self.max_steps as usize + 6);
        // The walk follows the error *gradient*, so probes are bounded by
        // the best SSE seen so far — not by the gate's threshold. Bounding
        // by the threshold would return `None` for every step that is
        // merely better than the last, and the walk would never move. The
        // threshold is applied once, to the final answer.
        let (mut cx, mut cy) = (ox, oy);
        visited.push((cx, cy));
        let mut best = eval(cx, cy, u64::MAX);
        let mut lim = best.map_or(u64::MAX, |b| b.sse);
        let mut probe = |cx: i64, cy: i64, pattern: &[(i64, i64)], best: &mut Option<Candidate>, lim: &mut u64, visited: &mut Vec<(i64, i64)>| -> (i64, i64) {
            let mut arg = (cx, cy);
            for &(dx, dy) in pattern {
                let (x, y) = (cx + dx, cy + dy);
                if !within(x, y) || visited.contains(&(x, y)) {
                    continue;
                }
                visited.push((x, y));
                if let Some(c) = eval(x, y, *lim) {
                    if best.map_or(true, |b| c.sse < b.sse) {
                        *lim = c.sse;
                        *best = Some(c);
                        arg = (x, y);
                    }
                }
            }
            arg
        };
        for _ in 0..self.max_steps {
            let (nx, ny) = probe(cx, cy, &LDSP, &mut best, &mut lim, &mut visited);
            if (nx, ny) == (cx, cy) {
                break;
            }
            (cx, cy) = (nx, ny);
        }
        probe(cx, cy, &SDSP, &mut best, &mut lim, &mut visited);
        best.filter(|b| b.sse <= limit)
    }
}

impl BlockMatcher for Diamond {
    fn search(&self, prev: &Frame, next: &Frame, dst_x: u32, dst_y: u32, size: u32, limit: u64) -> Option<Candidate> {
        self.search_from(prev, next, dst_x, dst_y, size, limit, (0, 0))
    }
    /// A pan of more than `radius` (or of more than ~8 px on repetitive
    /// texture) puts the true source outside the basin the unhinted walk
    /// starts in; a global-motion hint from phase correlation puts the
    /// start inside it. The hint is one per frame, so it is right for the
    /// background and wrong for movers — hence the fallback.
    ///
    /// MERGE NOTE: the incoming series gave `walk` a start point so a hint
    /// could seed it; this side had already made `walk` generic over the
    /// probe function with a visited list so no point is tested twice. They
    /// compose — `walk`'s `(ox, oy)` IS the start point — so the seeding is
    /// expressed through the deduped walk rather than either being dropped.
    fn search_from(&self, prev: &Frame, next: &Frame, dst_x: u32, dst_y: u32, size: u32, limit: u64, hint: (i64, i64)) -> Option<Candidate> {
        let eval = |x: i64, y: i64, lim: u64| test(prev, next, dst_x, dst_y, x, y, size, lim);
        if hint != (0, 0) {
            let (sx, sy) = (dst_x as i64 + hint.0, dst_y as i64 + hint.1);
            if sx >= 0 && sy >= 0 && prev.block_in_bounds(sx as u32, sy as u32, size) {
                if let Some(c) = self.walk(sx, sy, limit, eval) {
                    return Some(c);
                }
            }
        }
        let eval = |x: i64, y: i64, lim: u64| test(prev, next, dst_x, dst_y, x, y, size, lim);
        self.walk(dst_x as i64, dst_y as i64, limit, eval)
    }
    fn name(&self) -> &'static str {
        "diamond"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testimg::{shifted, smooth};

    #[test]
    fn stationary_only_tests_zero_motion() {
        let prev = smooth(64, 64, 0);
        let next = shifted(&prev, 3, 0);
        assert!(Stationary.search(&prev, &next, 16, 16, 16, 0).is_none());
        assert_eq!(
            Stationary.search(&prev, &prev, 16, 16, 16, 0),
            Some(Candidate { src_x: 16, src_y: 16, sse: 0 })
        );
    }

    #[test]
    fn exhaustive_finds_exact_shift() {
        let prev = smooth(64, 64, 0);
        let next = shifted(&prev, 3, -2);
        // next block at (24,24) equals prev block at (21,26)
        let c = Exhaustive { radius: 4 }.search(&prev, &next, 24, 24, 16, u64::MAX).unwrap();
        assert_eq!((c.src_x, c.src_y, c.sse), (21, 26, 0));
    }

    #[test]
    fn diamond_finds_exact_shift_and_respects_limit() {
        let prev = smooth(96, 96, 0);
        for &(dx, dy) in &[(3i64, -2i64), (0, 5), (-7, 4), (6, 6), (-1, 0)] {
            let next = shifted(&prev, dx, dy);
            let c = Diamond::default().search(&prev, &next, 32, 32, 16, u64::MAX).unwrap();
            assert_eq!((c.src_x as i64, c.src_y as i64, c.sse), (32 - dx, 32 - dy, 0), "shift {dx},{dy}");
        }
        // A limit of 0 with no exact match anywhere must yield None.
        let noisy = {
            let mut n = shifted(&prev, 3, 3);
            n.add_uniform_noise(4, 9);
            n
        };
        assert!(Diamond::default().search(&prev, &noisy, 32, 32, 16, 0).is_none());
    }

    #[test]
    fn diamond_agrees_with_exhaustive_on_shifts_within_radius() {
        let prev = smooth(80, 80, 5);
        for &(dx, dy) in &[(2i64, 1i64), (-3, -3), (4, 0), (5, -6)] {
            let next = shifted(&prev, dx, dy);
            let e = Exhaustive { radius: 8 }.search(&prev, &next, 24, 24, 16, u64::MAX).unwrap();
            let d = Diamond { max_steps: 16, radius: 8 }.search(&prev, &next, 24, 24, 16, u64::MAX).unwrap();
            assert_eq!(e.sse, 0);
            assert_eq!(d.sse, 0, "diamond missed shift {dx},{dy}: {d:?}");
            assert_eq!((e.src_x, e.src_y), (d.src_x, d.src_y));
        }
    }

    /// A shift beyond the radius is unreachable unhinted and exact with the
    /// right hint; a wrong hint falls back to the unhinted answer.
    #[test]
    fn diamond_hint_reaches_past_the_radius_and_falls_back() {
        let prev = smooth(160, 96, 7);
        let next = shifted(&prev, -40, 3); // next[x] = prev[x + 40]
        let d = Diamond { max_steps: 16, radius: 8 };
        assert!(d.search(&prev, &next, 64, 32, 16, u64::MAX).map_or(true, |c| c.sse > 0));
        let c = d.search_from(&prev, &next, 64, 32, 16, u64::MAX, (40, -3)).unwrap();
        assert_eq!((c.src_x, c.src_y, c.sse), (104, 29, 0));
        // Wrong hint: the seeded walk fails the limit, the fallback finds a
        // small shift the unhinted walk can reach.
        let next2 = shifted(&prev, 2, 1);
        let c = d.search_from(&prev, &next2, 64, 32, 16, 0, (40, 0)).unwrap();
        assert_eq!((c.src_x, c.src_y, c.sse), (62, 31, 0));
    }

    /// Diamond never returns something exhaustive would reject, and on a
    /// smooth surface it should rarely be worse. Measured, not asserted
    /// beyond a loose bound, because DS is a heuristic.
    #[test]
    fn diamond_is_close_to_exhaustive_under_noise() {
        let prev = smooth(96, 96, 2);
        let mut next = shifted(&prev, 4, -3);
        next.add_uniform_noise(3, 11);
        let mut worse = 0;
        for by in 1..5u32 {
            for bx in 1..5u32 {
                let (x, y) = (bx * 16, by * 16);
                let e = Exhaustive { radius: 8 }.search(&prev, &next, x, y, 16, u64::MAX).unwrap();
                let d = Diamond { max_steps: 16, radius: 8 }.search(&prev, &next, x, y, 16, u64::MAX).unwrap();
                assert!(d.sse >= e.sse);
                if d.sse > e.sse {
                    worse += 1;
                }
            }
        }
        assert!(worse <= 3, "diamond worse than exhaustive on {worse}/16 blocks");
    }
    /// The visited list pays: a block that moved (6, 4) is found with far
    /// fewer probes than the pattern count would suggest, and no (x, y) is
    /// ever probed twice. Counted through `walk`'s probe closure on a
    /// synthetic error surface (SSE = squared distance from the true
    /// source), so this is the walk's arithmetic and nothing else.
    #[test]
    fn the_diamond_walk_never_probes_a_point_twice() {
        let d = Diamond { max_steps: 16, radius: 32 };
        let (tx, ty) = (106i64, 84i64); // where the block really came from
        let mut probes: Vec<(i64, i64)> = Vec::new();
        let found = d.walk(100, 80, u64::MAX, |x, y, _| {
            probes.push((x, y));
            let sse = ((x - tx).pow(2) + (y - ty).pow(2)) as u64 * 10;
            Some(Candidate { src_x: x as u32, src_y: y as u32, sse })
        }).unwrap();
        assert_eq!((found.src_x, found.src_y), (106, 84));
        let mut sorted = probes.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), probes.len(), "a point was probed twice: {probes:?}");
        // Without the list: 9 per step × 4 steps + 5, plus the centre. With it,
        // every overlap between consecutive diamonds is skipped.
        assert!(probes.len() <= 30, "{} probes for a (6,4) move", probes.len());
    }

}
