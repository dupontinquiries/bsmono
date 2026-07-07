//! `batch_flow_route` — batched flow-direction routing (perf bonus).
//!
//! Worker-03 / WORLD pure core, round 22. Routes many cells in one call,
//! producing **identical** directions to the scalar reference (a verbatim mirror
//! of R19 `flow_route`).
//!
//! PERF: flow direction is computed for every cell when building the drainage
//! graph (a full heightfield per region) ahead of `flow_accumulation`. Batching
//! walks a contiguous `(center, neighbors)` slice with a fixed 4-way scan;
//! because each cell runs the exact scalar `flow_one`, the directions match
//! per-cell calls — a batched fast path can't reroute water differently from a
//! scalar fallback.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Scalar reference — identical to `flow_route::flow_dir` (R19). Returns the
/// lowest strictly-lower neighbor index `0..=3` (N,E,S,W) or `4` for a sink.
pub fn flow_one(h_center: f32, neigh: [f32; 4]) -> u8 {
    let mut best_idx: u8 = 4;
    let mut best_h = h_center;
    for i in 0..4 {
        if neigh[i] < best_h {
            best_h = neigh[i];
            best_idx = i as u8;
        }
    }
    best_idx
}

/// Batched flow routing over `(center, neighbors)` cells.
/// Identical to calling `flow_one` per cell.
pub fn flow_batch(cells: &[(f32, [f32; 4])]) -> Vec<u8> {
    cells.iter().map(|&(hc, neigh)| flow_one(hc, neigh)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_matches_scalar() {
        let cells = [
            (5.0f32, [9.0, 2.0, 9.0, 9.0]),
            (0.0, [1.0, 2.0, 3.0, 4.0]), // sink
            (5.0, [1.0, 9.0, 1.0, 9.0]), // tie → idx 0
            (3.0, [3.0, 3.0, 3.0, 3.0]), // flat → sink
        ];
        let batch = flow_batch(&cells);
        for (i, &(hc, n)) in cells.iter().enumerate() {
            assert_eq!(batch[i], flow_one(hc, n), "dir drift at {i}");
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(flow_batch(&[]).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let one = flow_one(5.0, [4.0, 3.0, 2.0, 1.0]);
        assert_eq!(flow_batch(&[(5.0, [4.0, 3.0, 2.0, 1.0])]), vec![one]);
    }

    #[test]
    fn dirs_bounded() {
        for d in flow_batch(&[(1.0, [0.0, 2.0, 3.0, 4.0]), (9.0, [9.0, 9.0, 9.0, 9.0])]) {
            assert!(d <= 4);
        }
    }

    #[test]
    fn deterministic() {
        let cells = [(5.0f32, [1.0, 2.0, 3.0, 4.0]), (5.0, [1.0, 2.0, 3.0, 4.0])];
        assert_eq!(flow_batch(&cells), flow_batch(&cells));
    }

    #[test]
    fn covers_each_direction_and_sink() {
        let cells = [
            (5.0f32, [1.0, 9.0, 9.0, 9.0]), // N
            (5.0, [9.0, 1.0, 9.0, 9.0]),    // E
            (5.0, [9.0, 9.0, 1.0, 9.0]),    // S
            (5.0, [9.0, 9.0, 9.0, 1.0]),    // W
            (0.0, [5.0, 5.0, 5.0, 5.0]),    // sink
        ];
        assert_eq!(flow_batch(&cells), vec![0, 1, 2, 3, 4]);
    }
}
