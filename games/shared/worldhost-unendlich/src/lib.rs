//! WORLDHOST FOR UNENDLICH — the voxel half of the seam.
//!
//! blackstar-rs's world is a tile with an SDF; unendlich's is streamed voxel
//! chunks. `worldhost::WorldHost` is the list of questions the game asks that
//! both can answer, and this crate is the second answer. It is the only crate
//! that knows both sides: the engine does not import the game's trait and the
//! game does not import the engine.
//!
//! ## Two layers, because one of them is testable and the other is a build
//!
//! An earlier draft of this file called `ChunkManager` directly, which made
//! every line of it — the cell mapping, the surface convention, the clearance
//! step — unverifiable without compiling a 326k-line Vulkan engine. The
//! geometry does not need the engine to be right, so it no longer waits for
//! it:
//!
//! * [`VoxelSource`] is what this adapter actually needs from a voxel world:
//!   three questions, no types from either side.
//! * [`Host`] implements `WorldHost` over any `VoxelSource`, in pure
//!   arithmetic, and its tests run against a fake in milliseconds.
//! * The `engine` feature adds `impl VoxelSource for ChunkManager` and
//!   nothing else. That impl is thin enough to read in one screen, which is
//!   the point: the part that cannot be tested here is the part with no logic
//!   in it.
//!
//! ## UNLOADED IS NOT AIR — the bug this rewrite fixes
//!
//! `ChunkManager::block_at` returns `BlockId::Air` for a chunk that is not
//! resident (`chunks.get(&coord)` misses → `Air`). A `clearance` built on
//! `block_at` alone therefore reports FREE SPACE over any column the streamer
//! has not reached yet, and a body walking toward the edge of the loaded
//! region falls through the world. The engine knows the difference —
//! `is_loaded_at` — and this adapter now asks.
//!
//! [`Unloaded`] is the policy, and it defaults to `Solid`: refusing to move
//! into unstreamed world is recoverable, falling through it is not. A caller
//! that would rather see through the gap (a distant camera, a debug view) can
//! choose `Air` deliberately.
//!
//! ## The mapping
//!
//! | question | unendlich |
//! |---|---|
//! | `ground_h(x, z)` | `surface_y_at`, top solid block, plus one |
//! | `clearance(p)` | `block_at`/`is_loaded_at` → a step, not a distance |
//! | `line_of_sight(a, b)` | `raycast` with the engine's own blocking predicate |
//! | `ground_plane()` | true — a voxel world always has a column under you |
//! | `gravity_dir(p)` | `-Y` (the trait default): unendlich has one down |
//! | `platform_vel` | `None` for now; moving structures are not streamed yet |
//! | `origin()` | zero — the chunk manager IS the plane; there is no window |
//!
//! Why `origin` is zero is the whole point of the exercise: blackstar-rs
//! pages a tile because its world is a tile, and a game system that asked
//! `origin()` to shift its positions on a page simply never sees a page here.
//!
//! ## Build status
//!
//! COMPILES against the engine (`--features engine`, unendlich-engine at
//! r746, rustc 1.96). The `impl VoxelSource for ChunkManager` below type-
//! checks: `is_loaded_at`, `block_at().is_solid()`, `surface_y_at` and
//! `raycast` are all real signatures, not read off a file and hoped for.
//! What has NOT happened is running it against a streaming world with
//! terrain in it, so the numbers are still claims:
//!
//! `BLOCK_M` (the block-to-metre scale) is a dial and 1.0 is a placeholder;
//! `SURFACE_SEARCH`'s range should probably be the world's own build limits
//! rather than a constant; and `raycast`'s hit is assumed to mean "blocked",
//! which its `block_is_blocking` predicate supports by name but which has not
//! been observed. None of the three is load-bearing for the geometry below.

use glam::Vec3;
use worldhost::WorldHost;

/// Metres per block. The engine's unit is one block; the game's is the metre
/// of a 0.38 m car. A DIAL: at 1.0 the district's own scale factor carries
/// the difference; when the body is a robot rather than a car this becomes
/// the larger block that makes buildings come apart in big pieces.
pub const BLOCK_M: f32 = 1.0;

/// How far up and down `ground_h` looks for the surface, in blocks.
pub const SURFACE_SEARCH: i32 = 256;

/// What is in a cell. The third case is the one `block_at` cannot express.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cell {
    Air,
    Solid,
    /// The chunk is not resident. NOT the same as air, and treating it as air
    /// is how a body falls out of the world at the streaming frontier.
    Unloaded,
}

/// How an unresident cell is answered to the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Unloaded {
    /// Refuse to move into it. The default: a body stopped at the frontier
    /// can be recovered; one that fell through it cannot.
    #[default]
    Solid,
    /// See and move through it. For a camera or a debug view that would
    /// rather have a hole than a wall.
    Air,
}

/// The three things this adapter needs from a voxel world. Implemented for
/// `ChunkManager` behind the `engine` feature, and for a fake in the tests.
pub trait VoxelSource {
    /// What is at this block coordinate.
    fn cell(&self, bx: i32, by: i32, bz: i32) -> Cell;

    /// Index of the topmost ground block in this column, within the range.
    /// `None` when the column cannot be sampled.
    fn surface_y(&self, bx: i32, bz: i32, y_min: i32, y_max: i32) -> Option<i32>;

    /// Is the segment from `origin` along `dir` for `max_t` BLOCKS blocked.
    /// Block units, because that is what the engine's raycast takes.
    fn ray_blocked(&self, origin: [f32; 3], dir: [f32; 3], max_t: f32) -> bool;
}

/// A voxel world seen through the seam. Borrowed, not owned: the engine keeps
/// streaming; the game only asks.
pub struct Host<'a, S: VoxelSource> {
    pub voxels: &'a S,
    /// Height the game treats as ground where no column can be sampled, so a
    /// body over unstreamed world stands rather than falls forever.
    pub fallback_y: f32,
    /// How unresident cells answer `clearance`.
    pub unloaded: Unloaded,
}

impl<'a, S: VoxelSource> Host<'a, S> {
    pub fn new(voxels: &'a S) -> Host<'a, S> {
        Host { voxels, fallback_y: 0.0, unloaded: Unloaded::default() }
    }
    /// See through the streaming frontier instead of stopping at it.
    pub fn seeing_through_unloaded(mut self) -> Self {
        self.unloaded = Unloaded::Air;
        self
    }
    fn block(v: f32) -> i32 {
        (v / BLOCK_M).floor() as i32
    }
}

impl<S: VoxelSource> WorldHost for Host<'_, S> {
    fn ground_h(&self, x: f32, z: f32) -> f32 {
        match self.voxels.surface_y(Self::block(x), Self::block(z), -SURFACE_SEARCH, SURFACE_SEARCH)
        {
            // the top solid block's upper face is one block above its index
            Some(y) => (y + 1) as f32 * BLOCK_M,
            None => self.fallback_y,
        }
    }

    fn clearance(&self, p: Vec3) -> f32 {
        // A STEP, NOT A DISTANCE. The trait permits this explicitly: callers
        // only ever ask whether a point is clear and how far they may step.
        // A true distance field over voxels would be a chamfer pass the
        // engine does not keep.
        let c = self.voxels.cell(Self::block(p.x), Self::block(p.y), Self::block(p.z));
        let solid = match c {
            Cell::Solid => true,
            Cell::Air => false,
            Cell::Unloaded => self.unloaded == Unloaded::Solid,
        };
        if solid {
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
        !self.voxels.ray_blocked(
            [a.x / BLOCK_M, a.y / BLOCK_M, a.z / BLOCK_M],
            [dir.x, dir.y, dir.z],
            len / BLOCK_M,
        )
    }

    /// A voxel world always has a column under you — there is no field-world
    /// case here, so `ground_h` always means something.
    fn ground_plane(&self) -> bool {
        true
    }
}

// ---------------------------------------------------------------- engine ---
//
// The only part that needs the engine, kept to one impl with no logic in it
// so that what cannot be tested here also cannot be wrong in an interesting
// way.
#[cfg(feature = "engine")]
mod engine {
    use super::{Cell, VoxelSource};
    use unendlich_engine::world::chunk_manager::ChunkManager;

    impl VoxelSource for ChunkManager {
        fn cell(&self, bx: i32, by: i32, bz: i32) -> Cell {
            if !self.is_loaded_at(bx, by, bz) {
                return Cell::Unloaded;
            }
            if self.block_at(bx, by, bz).is_solid() {
                Cell::Solid
            } else {
                Cell::Air
            }
        }
        fn surface_y(&self, bx: i32, bz: i32, y_min: i32, y_max: i32) -> Option<i32> {
            ChunkManager::surface_y_at(self, bx, bz, y_min, y_max)
        }
        fn ray_blocked(&self, origin: [f32; 3], dir: [f32; 3], max_t: f32) -> bool {
            ChunkManager::raycast(self, origin, dir, max_t).is_some()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// A column world: solid at or below `top`, air above, and everything
    /// outside `loaded_x` unresident.
    struct Fake {
        top: i32,
        loaded_x: std::ops::Range<i32>,
        rays: RefCell<Vec<(f32, f32, f32)>>,
        blocked: bool,
    }
    impl Fake {
        fn new(top: i32) -> Fake {
            Fake { top, loaded_x: -1000..1000, rays: RefCell::new(Vec::new()), blocked: false }
        }
    }
    impl VoxelSource for Fake {
        fn cell(&self, bx: i32, by: i32, _bz: i32) -> Cell {
            if !self.loaded_x.contains(&bx) {
                return Cell::Unloaded;
            }
            if by <= self.top {
                Cell::Solid
            } else {
                Cell::Air
            }
        }
        fn surface_y(&self, bx: i32, _bz: i32, _y_min: i32, _y_max: i32) -> Option<i32> {
            if self.loaded_x.contains(&bx) {
                Some(self.top)
            } else {
                None
            }
        }
        fn ray_blocked(&self, origin: [f32; 3], _dir: [f32; 3], max_t: f32) -> bool {
            self.rays.borrow_mut().push((origin[0], origin[1], max_t));
            self.blocked
        }
    }


    #[test]
    fn the_voxel_host_satisfies_the_shared_contract() {
        // THE POINT OF THE SEAM: the same suite that blackstar's tile world
        // runs, run against streamed voxels. If both pass, "swap the engine"
        // is a claim with evidence behind it rather than an intention.
        use worldhost::conformance::{check, Probe};
        let f = Fake::new(10);
        let h = Host::new(&f);
        check(
            &h,
            &Probe {
                open: Vec3::new(0.0, 40.0, 0.0),
                buried: Some(Vec3::new(0.0, 2.0, 0.0)),
                column: (0.0, 0.0),
            },
        );

        // and with the streaming frontier in play, which is the case the tile
        // world does not have at all
        let g = Fake { top: 4, loaded_x: 0..8, ..Fake::new(4) };
        let hg = Host::new(&g);
        check(
            &hg,
            &Probe {
                open: Vec3::new(4.0, 40.0, 0.0),
                buried: Some(Vec3::new(900.0, 20.0, 0.0)),
                column: (4.0, 0.0),
            },
        );
    }

    #[test]
    fn the_ground_is_the_top_face_of_the_top_block_not_its_index() {
        // A block at index 10 is solid from y=10 to y=11; standing on it puts
        // you at 11. Returning 10 buries the body half a block, which is the
        // classic off-by-one when a voxel world is read as a heightmap.
        let f = Fake::new(10);
        let h = Host::new(&f);
        assert_eq!(h.ground_h(0.0, 0.0), 11.0);
        assert_eq!(h.ground_h(-0.5, -0.5), 11.0, "negative coords must floor, not truncate");
    }

    #[test]
    fn an_unloaded_column_stands_on_the_fallback_rather_than_falling() {
        let f = Fake { top: 4, loaded_x: 0..8, ..Fake::new(4) };
        let mut h = Host::new(&f);
        h.fallback_y = 3.0;
        assert_eq!(h.ground_h(2.0, 0.0), 5.0, "loaded column should read the surface");
        assert_eq!(h.ground_h(900.0, 0.0), 3.0, "unloaded column should take the fallback");
    }

    #[test]
    fn unloaded_is_solid_by_default_so_nothing_walks_off_the_frontier() {
        // THE BUG THIS CRATE EXISTS TO NOT HAVE. `block_at` answers Air for a
        // chunk that is not resident, so a clearance built on it alone calls
        // the streaming frontier free space.
        let f = Fake { top: 0, loaded_x: 0..8, ..Fake::new(0) };
        let h = Host::new(&f);
        let past_the_edge = Vec3::new(50.0, 20.0, 0.0);
        assert!(h.clearance(past_the_edge) < 0.0, "unstreamed world reported as free space");
        // and inside the loaded region the same height IS free
        assert!(h.clearance(Vec3::new(4.0, 20.0, 0.0)) > 0.0);
    }

    #[test]
    fn seeing_through_unloaded_is_available_but_must_be_asked_for() {
        let f = Fake { top: 0, loaded_x: 0..8, ..Fake::new(0) };
        let h = Host::new(&f).seeing_through_unloaded();
        assert!(h.clearance(Vec3::new(50.0, 20.0, 0.0)) > 0.0);
    }

    #[test]
    fn clearance_is_a_step_and_solid_reads_negative() {
        let f = Fake::new(10);
        let h = Host::new(&f);
        assert!(h.clearance(Vec3::new(0.0, 3.0, 0.0)) < 0.0, "inside the ground must be negative");
        assert!(h.clearance(Vec3::new(0.0, 30.0, 0.0)) > 0.0, "open air must be positive");
    }

    #[test]
    fn line_of_sight_asks_the_engine_raycast_in_block_units_and_stops_at_the_target() {
        let mut f = Fake::new(0);
        f.blocked = false;
        let h = Host::new(&f);
        let a = Vec3::new(0.0, 20.0, 0.0);
        let b = Vec3::new(30.0, 20.0, 0.0);
        assert!(h.line_of_sight(a, b));
        let (ox, oy, max_t) = f.rays.borrow()[0];
        assert_eq!((ox, oy), (0.0, 20.0));
        assert!(
            (max_t - 30.0).abs() < 1e-3,
            "the ray must stop at the target, not run to infinity: {max_t}"
        );
    }

    #[test]
    fn a_blocked_ray_is_no_sight_and_a_zero_length_look_is_always_clear() {
        let mut f = Fake::new(0);
        f.blocked = true;
        let h = Host::new(&f);
        assert!(!h.line_of_sight(Vec3::new(0.0, 5.0, 0.0), Vec3::new(9.0, 5.0, 0.0)));
        let p = Vec3::new(1.0, 2.0, 3.0);
        assert!(h.line_of_sight(p, p), "looking at yourself must not consult the engine");
        assert_eq!(f.rays.borrow().len(), 1, "the zero-length look issued a raycast");
    }

    #[test]
    fn the_voxel_world_claims_a_ground_plane_and_one_down() {
        let f = Fake::new(0);
        let h = Host::new(&f);
        assert!(h.ground_plane(), "a voxel world always has a column under you");
        assert_eq!(h.gravity_dir(Vec3::new(5.0, 5.0, 5.0)), Vec3::NEG_Y);
        assert_eq!(h.origin(), Vec3::ZERO, "the chunk manager IS the plane; there is no window");
        assert!(h.plazas().is_empty() && h.cover().is_empty() && h.obstacles().is_empty());
    }
}
