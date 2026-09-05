//! A staging layer between the tools and the real world.
//!
//! WHY THIS EXISTS -- a finding from reading unendlich's
//! `ChunkManager::set_block` before wiring anything to it:
//!
//! ```text
//! let mut new_chunk = entry.value().clone_for_edit();   // clones 32^3 BlockIds
//! new_chunk.set(lx, ly, lz, block);
//! self.replace_edited_chunk(new_chunk, (lx, ly, lz));   // + octree rebuild
//! ...invalidate_for_edit... bump_neighbor_if_edge x3
//! ```
//!
//! That is copy-on-write per BLOCK. A modest 500-block tower would
//! clone 500 chunks (16 million BlockId copies), rebuild 500 octrees,
//! and bump ~1500 neighbour versions -- for an edit that touches two
//! or three chunks. There is no batch entry point in the tree.
//!
//! So the tools do not write to the world directly. They write here:
//! an overlay that reads through to the real world and buffers every
//! change in memory. At the end, `drain_by_chunk` hands back the edits
//! grouped by chunk, so the engine can clone each affected chunk ONCE.
//!
//! Three things fall out for free:
//!
//! * **Dry runs never touch the world.** Preview is just "do not
//!   flush", instead of apply-then-reverse.
//! * **Undo is the overlay's previous values**, already recorded.
//! * **The animator gets its block list** without a second pass.

use std::collections::HashMap;
use crate::VoxelWorld;

/// Wraps a read-only view of the real world and buffers writes.
pub struct StagedWorld<'w, W: VoxelWorld> {
    base: &'w W,
    /// x,y,z -> (new, previous)
    edits: HashMap<(i32, i32, i32), (u16, u16)>,
}

impl<'w, W: VoxelWorld> StagedWorld<'w, W> {
    pub fn new(base: &'w W) -> Self {
        Self { base, edits: HashMap::new() }
    }

    pub fn pending(&self) -> usize { self.edits.len() }

    pub fn is_empty(&self) -> bool { self.edits.is_empty() }

    /// Discard everything staged. This is what a rejected plan does.
    pub fn discard(&mut self) { self.edits.clear(); }

    /// Edits grouped by chunk, ready for one clone-and-apply each.
    /// Returns `(chunk_coord, [(x, y, z, block)])`, sorted for
    /// determinism. Consumes the staged set.
    pub fn drain_by_chunk(&mut self, chunk_size: i32)
        -> Vec<((i32, i32, i32), Vec<(i32, i32, i32, u16)>)> {
        let mut by: HashMap<(i32, i32, i32), Vec<(i32, i32, i32, u16)>> = HashMap::new();
        for (&(x, y, z), &(new, _prev)) in self.edits.iter() {
            let c = (x.div_euclid(chunk_size), y.div_euclid(chunk_size), z.div_euclid(chunk_size));
            by.entry(c).or_default().push((x, y, z, new));
        }
        self.edits.clear();
        let mut out: Vec<_> = by.into_iter().collect();
        out.sort_by_key(|(c, _)| *c);
        for (_, v) in out.iter_mut() { v.sort_by_key(|&(x, y, z, _)| (y, z, x)); }
        out
    }

    /// Drop every staged write without applying it. The hive path takes
    /// a snapshot and owns the changes from then on; leaving them here
    /// would apply them a second time on the next flush.
    pub fn clear(&mut self) { self.edits.clear(); }

    /// Every staged write, without consuming them. The flush path uses
    /// this to remember what a partial flush could not place.
    pub fn snapshot(&self) -> Vec<crate::weather::Change> {
        let mut v: Vec<_> = self.edits.iter()
            .map(|(&(x, y, z), &(new, _))| crate::weather::Change { at: (x, y, z), to: new })
            .collect();
        v.sort_by_key(|c| c.at);
        v
    }

    /// Previous values, for an undo that does not need the edit log.
    pub fn inverse(&self) -> Vec<(i32, i32, i32, u16)> {
        let mut v: Vec<_> = self.edits.iter().map(|(&(x, y, z), &(_, p))| (x, y, z, p)).collect();
        v.sort_by_key(|&(x, y, z, _)| (y, z, x));
        v
    }

    /// How many chunks a flush would touch. The number that matters:
    /// it is the count of `clone_for_edit` calls the engine will make.
    pub fn chunks_touched(&self, chunk_size: i32) -> usize {
        let mut s = std::collections::BTreeSet::new();
        for &(x, y, z) in self.edits.keys() {
            s.insert((x.div_euclid(chunk_size), y.div_euclid(chunk_size), z.div_euclid(chunk_size)));
        }
        s.len()
    }
}

impl<'w, W: VoxelWorld> VoxelWorld for StagedWorld<'w, W> {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        match self.edits.get(&(x, y, z)) {
            Some(&(new, _)) => new,
            None => self.base.get(x, y, z),
        }
    }
    fn set(&mut self, x: i32, y: i32, z: i32, block: u16) -> bool {
        let prev = self.get(x, y, z);
        if prev == block { return false; }
        // Keep the ORIGINAL base value if this cell was already staged,
        // so inverse() undoes back to the world rather than to an
        // intermediate value.
        let original = self.edits.get(&(x, y, z)).map(|&(_, p)| p).unwrap_or(prev);
        if block == original {
            // Writing a cell back to what the world already holds is
            // not an edit, it is the cancellation of one. Dropping the
            // entry is what makes a dry run free: `run_plan(dry_run)`
            // applies the plan and then reverses it with set() calls,
            // and without this the overlay ends up holding hundreds of
            // no-op writes that would flush to the engine and clone
            // chunks for nothing. Found by a debug_assert in the
            // engine binding, not by reasoning.
            self.edits.remove(&(x, y, z));
            return true;
        }
        self.edits.insert((x, y, z), (block, original));
        true
    }
    fn y_bounds(&self) -> (i32, i32) { self.base.y_bounds() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct Flat { cells: BTreeMap<(i32, i32, i32), u16> }
    impl Flat { fn new() -> Self { Self { cells: BTreeMap::new() } } }
    impl VoxelWorld for Flat {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.cells.get(&(x, y, z)) { return b; }
            if y <= 64 { 1 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool { self.cells.insert((x, y, z), b); true }
    }

    #[test]
    fn reads_fall_through_writes_are_buffered() {
        let base = Flat::new();
        let mut s = StagedWorld::new(&base);
        assert_eq!(s.get(0, 64, 0), 1, "reads see the real world");
        s.set(0, 70, 0, 58);
        assert_eq!(s.get(0, 70, 0), 58, "reads see staged writes");
        assert_eq!(base.get(0, 70, 0), 0, "the real world is untouched");
        assert_eq!(s.pending(), 1);
    }

    #[test]
    fn drain_groups_by_chunk_so_each_is_cloned_once() {
        let base = Flat::new();
        let mut s = StagedWorld::new(&base);
        // a 40-tall column crosses a chunk boundary at y=96
        for y in 65..105 { s.set(0, y, 0, 58); }
        assert_eq!(s.chunks_touched(32), 2);
        let groups = s.drain_by_chunk(32);
        assert_eq!(groups.len(), 2, "two chunks, not forty blocks");
        let total: usize = groups.iter().map(|(_, v)| v.len()).sum();
        assert_eq!(total, 40);
        for (c, blocks) in &groups {
            for &(x, y, z, _) in blocks {
                assert_eq!((x.div_euclid(32), y.div_euclid(32), z.div_euclid(32)), *c);
            }
        }
        assert!(s.is_empty(), "draining consumes");
    }

    #[test]
    fn restaging_a_cell_keeps_the_original_previous_value() {
        let base = Flat::new();
        let mut s = StagedWorld::new(&base);
        s.set(0, 64, 0, 58);      // was 1 (stone)
        s.set(0, 64, 0, 37);      // now marble
        let inv = s.inverse();
        assert_eq!(inv, vec![(0, 64, 0, 1)], "undo must go back to the world value");
    }

    #[test]
    fn setting_a_cell_to_what_it_already_is_stages_nothing() {
        let base = Flat::new();
        let mut s = StagedWorld::new(&base);
        assert!(!s.set(0, 64, 0, 1), "already stone");
        assert_eq!(s.pending(), 0);
    }

    #[test]
    fn a_real_build_touches_few_chunks_for_many_blocks() {
        use crate::plan::{parse_plan_at, run_plan};
        use crate::VoxelTools;
        let base = Flat::new();
        let mut s = StagedWorld::new(&base);
        let (plan, _) = parse_plan_at(
            "terraform @-5,-5 @5,5 smooth block=stone\ncylinder @0,0 r=4 h=14 block=stone hollow\\n\
             path @4,0 @20,10 w=3 block=cobble\\n", Some((0, 65, 0)));
        let run = { let mut t = VoxelTools::new(&mut s); run_plan(&mut t, &plan, 200, 32, false) };
        assert!(run.blocks_changed > 200, "a real build: {} blocks", run.blocks_changed);
        let chunks = s.chunks_touched(32);
        assert!(chunks <= 6, "{} blocks should touch a handful of chunks, got {chunks}",
                run.blocks_changed);
        // this is the whole point: clone_for_edit calls go from
        // blocks_changed down to chunks
        assert!(chunks * 40 < run.blocks_changed, "batching must be a large win");
    }

    #[test]
    fn discard_is_how_a_rejected_plan_leaves_no_trace() {
        let base = Flat::new();
        let mut s = StagedWorld::new(&base);
        s.set(1, 70, 1, 58);
        s.discard();
        assert!(s.is_empty());
        assert_eq!(s.get(1, 70, 1), 0, "reads fall back through to the world");
    }

    #[test]
    fn a_dry_run_leaves_nothing_staged() {
        use crate::plan::{parse_plan_at, run_plan};
        use crate::VoxelTools;
        let base = Flat::new();
        let mut s = StagedWorld::new(&base);
        let (plan, _) = parse_plan_at(
            "cylinder @0,0 r=3 h=11 block=stone_bricks hollow\n", Some((0, 65, 0)));
        let run = { let mut t = VoxelTools::new(&mut s); run_plan(&mut t, &plan, 200, 32, true) };
        assert!(run.blocks_changed > 100, "the dry run still reports the effect");
        assert_eq!(s.pending(), 0, "but stages nothing: reversal cancels the writes");
        assert_eq!(s.chunks_touched(32), 0, "so a flush would clone no chunks");
    }

    #[test]
    fn writing_a_cell_back_to_its_world_value_cancels_the_edit() {
        let base = Flat::new();
        let mut s = StagedWorld::new(&base);
        s.set(0, 70, 0, 58);
        assert_eq!(s.pending(), 1);
        s.set(0, 70, 0, 0);          // back to air, which is what the world has
        assert_eq!(s.pending(), 0, "cancelled, not recorded as a second edit");
        assert_eq!(s.get(0, 70, 0), 0);
    }
}
