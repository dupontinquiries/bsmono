//! WORLDHOST FOR UNENDLICH — the voxel half of the seam.
//!
//! **Status: written against the engine's source, not compiled.** The
//! session that wrote this could read `chunk_manager.rs` but not afford
//! to build a 344k-line Vulkan engine on one core, so every call below
//! is a claim about an API read off the file and three things want
//! checking the first time it is built: the block-to-metre scale
//! (`BLOCK_M`), whether `surface_y_at`'s y-range should be the chunk's
//! own, and whether `raycast`'s hit means "blocked" for the purposes of
//! line of sight (it uses `block_is_blocking`, which is the right
//! predicate by name). The crate sits outside both workspaces on
//! purpose: nothing depends on it until it has compiled once.
//!
//! The mapping:
//!
//! | question | unendlich |
//! |---|---|
//! | `ground_h(x, z)` | `ChunkManager::surface_y_at(x, z, y_min, y_max)`, the top solid block, plus one |
//! | `clearance(p)` | `block_at` → `BlockId::is_solid`: a step, `+BLOCK_M` in air, `-BLOCK_M/2` in stone |
//! | `line_of_sight(a, b)` | `ChunkManager::raycast` with the engine's own blocking predicate |
//! | `origin()` | zero — the chunk manager IS the plane; there is no window |
//! | `plazas/cover/obstacles` | empty until the district lattice feeds a structure placer |
//!
//! Why `origin` is zero: this is the whole point. blackstar-rs pages a
//! tile because its world is a tile; unendlich's world is already the
//! plane, streamed. A game system that asked `origin()` to shift its
//! positions on a page simply never sees a page here.

use glam::Vec3;
use unendlich_engine::world::chunk_manager::ChunkManager;
use worldhost::WorldHost;

/// Metres per block. The engine's unit is one block; the game's is the
/// metre of a 0.38 m car. Until the body is re-derived this is 1.0 and
/// the district's `DISTRICT_K` carries the scale; when the body is a
/// robot this is the 4 m block that makes buildings come apart in
/// Dark-of-the-Moon pieces. A DIAL, like `DISTRICT_K`.
pub const BLOCK_M: f32 = 1.0;

/// How far up and down `ground_h` looks for the surface, in blocks.
pub const SURFACE_SEARCH: i32 = 256;

/// A `ChunkManager` seen through the seam. Borrowed, not owned: the
/// engine keeps streaming; the game only asks.
pub struct Host<'a> {
    pub chunks: &'a ChunkManager,
    /// height the game treats as ground when no chunk is loaded there —
    /// the plane's nominal sea level; a body over an unloaded chunk
    /// stands on this rather than falling forever
    pub fallback_y: f32,
}

impl<'a> Host<'a> {
    pub fn new(chunks: &'a ChunkManager) -> Host<'a> {
        Host { chunks, fallback_y: 0.0 }
    }
}

impl WorldHost for Host<'_> {
    fn ground_h(&self, x: f32, z: f32) -> f32 {
        let bx = (x / BLOCK_M).floor() as i32;
        let bz = (z / BLOCK_M).floor() as i32;
        match self.chunks.surface_y_at(bx, bz, -SURFACE_SEARCH, SURFACE_SEARCH) {
            // the top solid block's upper face is one block above its index
            Some(y) => (y + 1) as f32 * BLOCK_M,
            None => self.fallback_y,
        }
    }

    fn clearance(&self, p: Vec3) -> f32 {
        let b = self.chunks.block_at((p.x / BLOCK_M).floor() as i32, (p.y / BLOCK_M).floor() as i32, (p.z / BLOCK_M).floor() as i32);
        if b.is_solid() {
            -BLOCK_M * 0.5
        } else {
            BLOCK_M
        }
    }

    fn line_of_sight(&self, a: Vec3, b: Vec3) -> bool {
        let d = b - a;
        let len = d.length();
        if len < 1e-3 {
            return true;
        }
        let dir = d / len;
        // a hit before the target is something in the way
        match self.chunks.raycast([a.x / BLOCK_M, a.y / BLOCK_M, a.z / BLOCK_M], [dir.x, dir.y, dir.z], len / BLOCK_M) {
            Some(_) => false,
            None => true,
        }
    }
}
