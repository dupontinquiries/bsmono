//! `world::water_edge_flow` — water flow across the shared edge between two adjacent
//! chunk water fields.
//!
//! [`WaterField::settle_step`](crate::world::water_field::WaterField) has closed chunk
//! boundaries, so within one chunk water can't leave it. Rivers and lakes that span
//! chunks need flux across the seam: this is that pass. For each pair of columns along
//! the shared edge, water moves from the higher water *surface* (`ground + depth`) to the
//! lower one, a fraction of the difference per tick (the same equalize-toward-the-mean
//! rule `settle_step` uses internally), bounded by the donor column's depth so it never
//! goes negative. What one side loses the other gains, so total volume across the pair is
//! conserved exactly.
//!
//! Only `+X` and `+Z` directions are needed: every interior seam is the `+X`/`+Z` edge of
//! exactly one chunk (the lower-coordinate neighbor), so iterating chunks and flowing each
//! one's `+X`/`+Z` edge covers every seam once. Because a chunk shares edges with several
//! neighbors, a parallel schedule must avoid mutating a chunk from two seams at once
//! (e.g. a red/black pass over seam parity); this module provides the per-seam primitive,
//! not that schedule.

use crate::constants::CHUNK_SIZE;
use crate::world::water_field::WaterField;

#[inline]
fn idx(x: usize, z: usize) -> usize {
    x + z * CHUNK_SIZE as usize
}

/// Which seam of chunk `a` the neighbor `b` sits on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    /// `b` is the `+X` neighbor: `a`'s `x = CHUNK_SIZE-1` column meets `b`'s `x = 0`.
    PosX,
    /// `b` is the `+Z` neighbor: `a`'s `z = CHUNK_SIZE-1` row meets `b`'s `z = 0`.
    PosZ,
}

/// Flow water across the seam between chunk `a` and its neighbor `b` (on side `edge`).
/// `rate` in `[0,1]` is the fraction of each column-pair's surface difference moved this
/// tick. Volume-conserving across the pair; depths stay non-negative.
pub fn flow_edge(
    a: &mut WaterField,
    a_ground: &[f32],
    b: &mut WaterField,
    b_ground: &[f32],
    edge: Edge,
    rate: f32,
) {
    let s = CHUNK_SIZE as usize;
    let rate = rate.clamp(0.0, 1.0);
    for k in 0..s {
        let (ia, ib) = match edge {
            Edge::PosX => (idx(s - 1, k), idx(0, k)),
            Edge::PosZ => (idx(k, s - 1), idx(k, 0)),
        };
        let surf_a = a_ground[ia] + a.depth[ia];
        let surf_b = b_ground[ib] + b.depth[ib];
        let diff = surf_a - surf_b;
        // move toward the mean (half the difference, scaled), bounded by the donor depth
        let want = rate * diff.abs() * 0.5;
        if diff > 0.0 {
            let moved = want.min(a.depth[ia]).max(0.0);
            a.depth[ia] -= moved;
            b.depth[ib] += moved;
        } else if diff < 0.0 {
            let moved = want.min(b.depth[ib]).max(0.0);
            b.depth[ib] -= moved;
            a.depth[ia] += moved;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::water_field::WATER_AREA;

    fn flat(h: f32) -> Vec<f32> {
        vec![h; WATER_AREA]
    }

    #[test]
    fn flows_high_to_low_and_conserves_volume() {
        // A full of water, B dry, flat equal ground → water crosses A→B
        let mut a = WaterField { depth: vec![10.0; WATER_AREA] };
        let mut b = WaterField::empty();
        let (ag, bg) = (flat(0.0), flat(0.0));
        let before = a.total_volume() + b.total_volume();
        flow_edge(&mut a, &ag, &mut b, &bg, Edge::PosX, 1.0);
        // A's right edge dropped, B's left edge rose
        assert!(a.depth[idx(CHUNK_SIZE as usize - 1, 0)] < 10.0, "donor edge falls");
        assert!(b.depth[idx(0, 0)] > 0.0, "receiver edge rises");
        let after = a.total_volume() + b.total_volume();
        assert!((before - after).abs() < 1e-3, "edge flow conserves total volume");
    }

    #[test]
    fn equal_surfaces_no_flow() {
        let mut a = WaterField { depth: vec![3.0; WATER_AREA] };
        let mut b = WaterField { depth: vec![3.0; WATER_AREA] };
        let snap_a = a.depth.clone();
        flow_edge(&mut a, &flat(0.0), &mut b, &flat(0.0), Edge::PosZ, 1.0);
        assert_eq!(a.depth, snap_a, "equal surfaces ⇒ no transfer");
    }

    #[test]
    fn ground_height_counts_toward_surface() {
        // B's column is empty but sits on higher ground than A's full column:
        // if ground+depth surfaces are equal, no flow.
        let mut a = WaterField { depth: vec![5.0; WATER_AREA] }; // surface 0+5 = 5
        let mut b = WaterField::empty(); // surface 5+0 = 5
        let ag = flat(0.0);
        let bg = flat(5.0);
        let snap_a = a.depth.clone();
        flow_edge(&mut a, &ag, &mut b, &bg, Edge::PosX, 1.0);
        assert_eq!(a.depth, snap_a, "surface (ground+depth) equality blocks flow uphill");
    }

    #[test]
    fn depth_never_negative() {
        let mut a = WaterField { depth: vec![0.2; WATER_AREA] };
        let mut b = WaterField::empty();
        flow_edge(&mut a, &flat(0.0), &mut b, &flat(0.0), Edge::PosX, 1.0);
        assert!(a.depth.iter().all(|&d| d >= 0.0) && b.depth.iter().all(|&d| d >= 0.0));
    }

    #[test]
    fn repeated_flow_equalizes() {
        // run many edge flows + nothing else: the two edge columns converge
        let mut a = WaterField { depth: vec![8.0; WATER_AREA] };
        let mut b = WaterField::empty();
        let (ag, bg) = (flat(0.0), flat(0.0));
        for _ in 0..200 {
            flow_edge(&mut a, &ag, &mut b, &bg, Edge::PosX, 0.5);
        }
        let ea = a.depth[idx(CHUNK_SIZE as usize - 1, 0)];
        let eb = b.depth[idx(0, 0)];
        assert!((ea - eb).abs() < 0.1, "edge columns converge to equal depth ({ea} vs {eb})");
    }
}
