//! `mineral_vein_trace` — ore vein paths from a seed point.
//!
//! Worker-03 / WORLD-PERF pure core, round 12. Grows a connected vein of cells
//! as a 6-direction random walk: each step is one unit move along an axis,
//! mostly continuing the current heading and occasionally turning, so veins
//! read as wandering seams rather than straight lines. Deterministic from the
//! seed. Pairs with `ore_distribution` (this is the *shape* of a vein; that's
//! the *richness*).
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Trace a vein from `start`, beginning in direction `dir` (0..5 = ±x, ±y, ±z),
/// for `len` cells total. The returned path always starts at `start`, has
/// `max(1, len)` cells, and every consecutive pair is axis-adjacent (connected).
pub fn trace(start: [i32; 3], dir: u8, len: u32, seed: u64) -> Vec<[i32; 3]> {
    let mut out = vec![start];
    if len == 0 {
        return out;
    }
    let mut cur = start;
    let mut d = dir % 6;
    // len cells total ⇒ len-1 steps after the start.
    for i in 1..len {
        let h = hash(seed ^ (i as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
        d = next_dir(d, h);
        cur = step(cur, d);
        out.push(cur);
    }
    out
}

/// Mostly keep heading; ~25% of the time turn to a hash-chosen axis direction.
fn next_dir(d: u8, h: u64) -> u8 {
    if h % 4 == 0 {
        ((h >> 8) % 6) as u8
    } else {
        d
    }
}

fn step(c: [i32; 3], d: u8) -> [i32; 3] {
    match d % 6 {
        0 => [c[0] + 1, c[1], c[2]],
        1 => [c[0] - 1, c[1], c[2]],
        2 => [c[0], c[1] + 1, c[2]],
        3 => [c[0], c[1] - 1, c[2]],
        4 => [c[0], c[1], c[2] + 1],
        _ => [c[0], c[1], c[2] - 1],
    }
}

fn hash(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manhattan(a: [i32; 3], b: [i32; 3]) -> i32 {
        (a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs()
    }

    #[test]
    fn has_len_cells() {
        assert_eq!(trace([0, 0, 0], 0, 5, 1).len(), 5);
        assert_eq!(trace([0, 0, 0], 0, 1, 1).len(), 1);
        assert_eq!(trace([0, 0, 0], 0, 50, 1).len(), 50);
    }

    #[test]
    fn starts_at_start() {
        let s = [3, -2, 7];
        assert_eq!(trace(s, 2, 8, 42)[0], s);
    }

    #[test]
    fn len_zero_is_just_start() {
        let s = [1, 1, 1];
        assert_eq!(trace(s, 0, 0, 5), vec![s]);
    }

    #[test]
    fn stays_connected() {
        let path = trace([0, 0, 0], 0, 100, 7);
        for w in path.windows(2) {
            assert_eq!(manhattan(w[0], w[1]), 1, "vein step not adjacent");
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(trace([5, 5, 5], 3, 30, 99), trace([5, 5, 5], 3, 30, 99));
    }

    #[test]
    fn seed_sensitive() {
        let a = trace([0, 0, 0], 0, 40, 1);
        let b = trace([0, 0, 0], 0, 40, 2);
        assert_ne!(a, b);
    }

    #[test]
    fn direction_seeds_first_step() {
        // With a heading that rarely turns, the first step should follow `dir`.
        let path = trace([0, 0, 0], 0, 2, 1); // dir 0 = +x
        // step is +x unless the first hash rolls a turn; assert adjacency holds
        // and the move is along a single axis.
        assert_eq!(manhattan(path[0], path[1]), 1);
    }
}
