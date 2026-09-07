//! Binding `voxel_tools` to this engine.
//!
//! Two problems, both resolved by reading `chunk_manager.rs` rather
//! than guessing:
//!
//! **1. `ChunkManager::set_block` takes `&self`,** because `chunks` is
//! an `Arc<DashMap<..>>` and the interior mutability lives there. The
//! `VoxelWorld` trait wants `&mut self`. No trait change is needed: a
//! newtype holding `&ChunkManager` can offer `&mut self` methods that
//! call through to the `&self` ones. The `&mut` is on the wrapper, not
//! on the manager.
//!
//! **2. Per-block writes are copy-on-write per block.** Each
//! `set_block` clones a whole `CHUNK_SIZE^3` chunk, rebuilds the
//! octree and bumps neighbours. Generated structures write hundreds of
//! blocks across a handful of chunks, so the tools stage their writes
//! (`voxel_tools::staged::StagedWorld`) and flush them through
//! `ChunkManager::set_blocks`, which clones once per chunk.
//!
//! The read path is direct: `block_at` is cheap and the tools read far
//! more than they write.

use crate::world::blocks::BlockId;
use crate::world::chunk_manager::ChunkManager;
use voxel_tools::staged::StagedWorld;
use voxel_tools::VoxelWorld;

/// Read-only adapter over the live world. Writes through this type go
/// nowhere — it exists so `StagedWorld` has something to read from.
pub struct WorldView<'a>(pub &'a ChunkManager);

/// The world as a DRONE sees it: an owned, Send read handle. Writes
/// are refused -- a drone stages into a StagedWorld layered over this,
/// and the queen applies the result on the frame thread.
pub struct DroneWorld(pub crate::world::chunk_manager::DroneView);

impl VoxelWorld for DroneWorld {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        self.0.block_at(x, y, z) as u8 as u16
    }
    fn set(&mut self, _x: i32, _y: i32, _z: i32, _b: u16) -> bool { false }
    fn y_bounds(&self) -> (i32, i32) { (-4096, 8192) }
}

impl<'a> VoxelWorld for WorldView<'a> {
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        self.0.block_at(x, y, z) as u16
    }
    fn set(&mut self, _x: i32, _y: i32, _z: i32, _b: u16) -> bool {
        // Deliberately inert. Everything writes to a StagedWorld and
        // lands via `flush`; a direct write here would be the
        // per-block copy-on-write path this module exists to avoid.
        debug_assert!(false, "write through WorldView: use StagedWorld + flush");
        false
    }
    fn y_bounds(&self) -> (i32, i32) {
        // There is no world-height constant in this engine: the world is
        // vertically unbounded and `is_sky_chunk` simply declines to load
        // chunks far above the terrain. The (-64, 320) written here first
        // was invented, and it would have silently refused every write
        // above y=320 — a tower on a 300-block peak would place its base
        // and stop.
        //
        // So this is a runaway guard, not a world limit. The real bound
        // is `set_block`'s wild-coordinate watchdog at 30 million; this
        // is generous enough never to clip a real build and tight enough
        // that a model emitting y=900000 is caught here rather than
        // spending a second in the chunk map first.
        (-4096, 8192)
    }
}

/// Numeric id back to `BlockId`. `voxel_tools` speaks `u16` so it can
/// stay dependency-free; this is the one place the two vocabularies
/// meet.
///
/// `BlockId` is `#[repr(u8)]` with a `TryFrom<u8>` whose error is the
/// unrecognised byte (blocks.rs:710). Unknown or out-of-range ids
/// become Air rather than panicking: a model can emit anything, and a
/// bad id should be a missing block, not a crash. The caller sees the
/// substitution in the skipped count, not in a stack trace.
/// A crate block id to a game one.
///
/// An unknown id used to become AIR, silently. That is the worst
/// possible fallback: the block does not appear, nothing is logged,
/// and the structure has a hole exactly where the interesting material
/// was meant to be. Two of the strata profile's banding colours -- 172
/// and 179 -- were doing this, so every sedimentary canyon was cut
/// with gaps in its bands.
///
/// Unknown ids become STONE, which is visible, solid and breakable,
/// and the first few are named in the log so the mapping can be fixed
/// rather than silently tolerated.
pub fn block_from_u16(v: u16) -> BlockId {
    if v == 0 { return BlockId::Air; }
    match u8::try_from(v).ok().and_then(|b| BlockId::try_from(b).ok()) {
        Some(b) => b,
        None => {
            use std::sync::atomic::{AtomicUsize, Ordering};
            static SAID: AtomicUsize = AtomicUsize::new(0);
            if SAID.fetch_add(1, Ordering::Relaxed) < 8 {
                log::warn!("[paintbrush] block id {v} is not a game block; using stone. \
                            A silent Air here is a hole in the build.");
            }
            BlockId::Stone
        }
    }
}

/// Apply everything staged, one chunk clone per affected chunk.
/// Returns `(applied, skipped, chunks_touched)`.
pub fn flush(cm: &ChunkManager, staged: &mut StagedWorld<'_, WorldView<'_>>, chunk_size: i32)
    -> (usize, usize, usize) {
    let groups = staged.drain_by_chunk(chunk_size);
    let chunks = groups.len();
    let mut edits: Vec<(i32, i32, i32, BlockId)> = Vec::new();
    for (_coord, blocks) in groups {
        edits.extend(blocks.into_iter().map(|(x, y, z, b)| (x, y, z, block_from_u16(b))));
    }
    let (applied, skipped) = cm.set_blocks(&edits);
    (applied, skipped, chunks)
}

/// Chunks a flush would touch, for the mesher's dirty list. Call
/// before `flush`, which consumes the staged set.
pub fn dirty_chunks(staged: &StagedWorld<'_, WorldView<'_>>, chunk_size: i32) -> usize {
    staged.chunks_touched(chunk_size)
}
