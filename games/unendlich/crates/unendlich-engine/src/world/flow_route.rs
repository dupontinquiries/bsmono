//! `flow_route` — steepest-descent flow direction for a cell.
//!
//! Worker-03 / WORLD pure core, round 19. Given a cell's height and its four
//! orthogonal neighbor heights, returns which neighbor water flows to (the
//! lowest one strictly below the cell), or `4 = sink` when nothing is lower.
//! This is the single-cell routing decision behind `flow_accumulation` (R8) and
//! the river network.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Flow direction for a cell: index `0..=3` of the lowest strictly-lower
/// neighbor (`neigh` ordered N,E,S,W), or `4` if the cell is a sink (no lower
/// neighbor). Ties resolve to the lowest index.
pub fn flow_dir(h_center: f32, neigh: [f32; 4]) -> u8 {
    let mut best_idx: u8 = 4; // sink
    let mut best_h = h_center;
    for i in 0..4 {
        if neigh[i] < best_h {
            best_h = neigh[i];
            best_idx = i as u8;
        }
    }
    best_idx
}

/// True if the cell is a sink (no strictly-lower neighbor).
pub fn is_sink(h_center: f32, neigh: [f32; 4]) -> bool {
    flow_dir(h_center, neigh) == 4
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lowest_neighbor_wins() {
        // N,E,S,W = [9, 2, 9, 9] → east (index 1) is lowest.
        assert_eq!(flow_dir(5.0, [9.0, 2.0, 9.0, 9.0]), 1);
        // west lowest
        assert_eq!(flow_dir(5.0, [9.0, 9.0, 9.0, 1.0]), 3);
    }

    #[test]
    fn sink_when_none_lower() {
        assert_eq!(flow_dir(0.0, [1.0, 2.0, 3.0, 4.0]), 4);
        assert!(is_sink(0.0, [0.0, 1.0, 5.0, 9.0])); // equal isn't lower
    }

    #[test]
    fn ties_resolve_to_lowest_index() {
        // Two equal-lowest neighbors (index 0 and 2) → index 0.
        assert_eq!(flow_dir(5.0, [1.0, 9.0, 1.0, 9.0]), 0);
    }

    #[test]
    fn flat_is_a_sink() {
        assert_eq!(flow_dir(3.0, [3.0, 3.0, 3.0, 3.0]), 4);
    }

    #[test]
    fn result_bounded() {
        for c in 0..10 {
            let n = [c as f32, (c + 1) as f32, (c - 1) as f32, c as f32];
            assert!(flow_dir(c as f32, n) <= 4);
        }
    }

    #[test]
    fn deterministic() {
        assert_eq!(flow_dir(5.0, [4.0, 3.0, 2.0, 1.0]), flow_dir(5.0, [4.0, 3.0, 2.0, 1.0]));
    }
}
