//! `block_edit` — the block place/break apply (logic half of the edit path).
//!
//! Worker-03 / WORLD pure core, round 13. Writes one cell of a slab and reports
//! whether it changed plus the dirty AABB to remesh — the edited cell and its
//! one-cell neighborhood (clamped to the slab), since changing a block can flip
//! the visible faces of its neighbors too. No rendering, no octree: just the
//! authoritative voxel write and the remesh bounds. Pairs with `edit_coalesce`.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Result of an edit: did anything change, and (if so) the inclusive remesh
/// AABB in slab coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditResult {
    pub changed: bool,
    pub dirty_min: [usize; 3],
    pub dirty_max: [usize; 3],
}

/// Write `new` at `pos` in `slab` (dimensions `dim = [x,y,z]`, row-major with x
/// fastest). Returns whether the cell changed and the inclusive dirty AABB
/// (`pos ± 1`, clamped to bounds). Out-of-bounds or undersized slabs are a safe
/// no-op (`changed = false`, no panic).
pub fn apply(slab: &mut [u16], dim: [usize; 3], pos: [usize; 3], new: u16) -> EditResult {
    let total = dim[0]
        .checked_mul(dim[1])
        .and_then(|a| a.checked_mul(dim[2]))
        .unwrap_or(0);
    let in_bounds = total > 0
        && slab.len() >= total
        && pos[0] < dim[0]
        && pos[1] < dim[1]
        && pos[2] < dim[2];
    if !in_bounds {
        return EditResult { changed: false, dirty_min: pos, dirty_max: pos };
    }

    let idx = (pos[2] * dim[1] + pos[1]) * dim[0] + pos[0];
    if slab[idx] == new {
        return EditResult { changed: false, dirty_min: pos, dirty_max: pos };
    }
    slab[idx] = new;

    let dirty_min = [
        pos[0].saturating_sub(1),
        pos[1].saturating_sub(1),
        pos[2].saturating_sub(1),
    ];
    let dirty_max = [
        (pos[0] + 1).min(dim[0] - 1),
        (pos[1] + 1).min(dim[1] - 1),
        (pos[2] + 1).min(dim[2] - 1),
    ];
    EditResult { changed: true, dirty_min, dirty_max }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idx(dim: [usize; 3], p: [usize; 3]) -> usize {
        (p[2] * dim[1] + p[1]) * dim[0] + p[0]
    }

    #[test]
    fn place_changes_cell() {
        let dim = [4, 4, 4];
        let mut slab = vec![0u16; 64];
        let r = apply(&mut slab, dim, [1, 2, 3], 7);
        assert!(r.changed);
        assert_eq!(slab[idx(dim, [1, 2, 3])], 7);
    }

    #[test]
    fn break_to_zero_changes() {
        let dim = [4, 4, 4];
        let mut slab = vec![5u16; 64];
        let r = apply(&mut slab, dim, [2, 2, 2], 0);
        assert!(r.changed);
        assert_eq!(slab[idx(dim, [2, 2, 2])], 0);
    }

    #[test]
    fn no_op_when_same() {
        let dim = [4, 4, 4];
        let mut slab = vec![5u16; 64];
        let r = apply(&mut slab, dim, [2, 2, 2], 5);
        assert!(!r.changed);
        assert!(slab.iter().all(|&v| v == 5));
    }

    #[test]
    fn dirty_aabb_is_cell_plus_minus_one_clamped() {
        let dim = [5, 5, 5];
        let mut slab = vec![0u16; 125];
        // interior
        let r = apply(&mut slab, dim, [2, 2, 2], 1);
        assert_eq!(r.dirty_min, [1, 1, 1]);
        assert_eq!(r.dirty_max, [3, 3, 3]);
        // corner clamps low
        let r0 = apply(&mut slab, dim, [0, 0, 0], 1);
        assert_eq!(r0.dirty_min, [0, 0, 0]);
        assert_eq!(r0.dirty_max, [1, 1, 1]);
        // far corner clamps high
        let r1 = apply(&mut slab, dim, [4, 4, 4], 1);
        assert_eq!(r1.dirty_min, [3, 3, 3]);
        assert_eq!(r1.dirty_max, [4, 4, 4]);
    }

    #[test]
    fn out_of_bounds_is_safe() {
        let dim = [4, 4, 4];
        let mut slab = vec![0u16; 64];
        let r = apply(&mut slab, dim, [4, 0, 0], 9); // x == dim.x
        assert!(!r.changed);
        assert!(slab.iter().all(|&v| v == 0));
        // undersized slab
        let mut tiny = vec![0u16; 10];
        let r2 = apply(&mut tiny, dim, [0, 0, 0], 9);
        assert!(!r2.changed);
    }

    #[test]
    fn deterministic() {
        let dim = [4, 4, 4];
        let mut a = vec![0u16; 64];
        let mut b = vec![0u16; 64];
        let ra = apply(&mut a, dim, [1, 1, 1], 3);
        let rb = apply(&mut b, dim, [1, 1, 1], 3);
        assert_eq!(ra, rb);
        assert_eq!(a, b);
    }
}
