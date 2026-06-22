//! `water_flow_tiles` — a persistent, sparse store of water flow vectors, keyed by
//! `TILE_SIZE`-block tiles. Phase 2 of "flowing water as a shader".
//!
//! The dynamic water sim produces a flow field over a window around the player every tick
//! (`water_flow_field`). That window moves with the player, so on its own the flow data
//! resets as you walk away. This store fixes that: each tick the window's vectors are
//! `ingest`ed into the tiles they fall on, and those tiles PERSIST. Walk along a river and
//! the whole length keeps its current; the renderer (later phase) samples tiles to steer
//! the surface waves.
//!
//! Two background behaviours keep it bounded and honest:
//!   * `decay` fades tiles the sim is no longer refreshing (you walked off), so a current
//!     that stopped being simulated eases to still rather than streaming forever.
//!   * `evict` drops tiles that are too old or too far from the player.
//!
//! Tiles align to the chunk grid (`TILE_SIZE` = 32) so a later GPU upload is one
//! texture-tile per chunk footprint.

use std::collections::HashMap;

pub const TILE_SIZE: i32 = 32;
pub const TILE_AREA: usize = (TILE_SIZE * TILE_SIZE) as usize;

#[inline]
fn tile_of(world: i32) -> i32 {
    world.div_euclid(TILE_SIZE)
}
#[inline]
fn local_of(world: i32) -> usize {
    world.rem_euclid(TILE_SIZE) as usize
}
#[inline]
fn cell_idx(lx: usize, lz: usize) -> usize {
    lx + lz * TILE_SIZE as usize
}

#[derive(Clone)]
pub struct FlowTile {
    pub vx: Vec<f32>, // TILE_AREA, row-major lx + lz*TILE_SIZE
    pub vz: Vec<f32>,
    pub depth: Vec<f32>, // r432: water depth (blocks) per cell; -1 = never written here
    pub last_update: u64, // tick/time of the most recent ingest
}

impl FlowTile {
    fn empty(now: u64) -> Self {
        Self {
            vx: vec![0.0; TILE_AREA],
            vz: vec![0.0; TILE_AREA],
            depth: vec![-1.0; TILE_AREA], // -1 until a real depth is ingested
            last_update: now,
        }
    }
}

#[derive(Clone, Default)]
pub struct FlowTileStore {
    tiles: HashMap<(i32, i32), FlowTile>,
}

impl FlowTileStore {
    pub fn new() -> Self {
        Self {
            tiles: HashMap::new(),
        }
    }

    pub fn tile_count(&self) -> usize {
        self.tiles.len()
    }

    /// Write a window of flow vectors into the tiles they fall on, stamping `now`.
    /// `origin_x/z` is the world position of column (0,0); `side` is the window edge.
    pub fn ingest(&mut self, origin_x: i32, origin_z: i32, side: usize, vx: &[f32], vz: &[f32], now: u64) {
        debug_assert_eq!(vx.len(), side * side);
        debug_assert_eq!(vz.len(), side * side);
        for z in 0..side {
            let wz = origin_z + z as i32;
            for x in 0..side {
                let wx = origin_x + x as i32;
                let src = x + z * side;
                let key = (tile_of(wx), tile_of(wz));
                let tile = self
                    .tiles
                    .entry(key)
                    .or_insert_with(|| FlowTile::empty(now));
                let dst = cell_idx(local_of(wx), local_of(wz));
                tile.vx[dst] = vx[src];
                tile.vz[dst] = vz[src];
                tile.last_update = now;
            }
        }
    }

    /// Flow vector at a world column (0 if no tile covers it).
    pub fn sample(&self, wx: i32, wz: i32) -> (f32, f32) {
        match self.tiles.get(&(tile_of(wx), tile_of(wz))) {
            Some(t) => {
                let i = cell_idx(local_of(wx), local_of(wz));
                (t.vx[i], t.vz[i])
            }
            None => (0.0, 0.0),
        }
    }

    /// Like `ingest`, but exponentially blends new vectors with the stored ones
    /// (`alpha` = weight of the new sample, 0..1) so the visible current is smooth instead
    /// of jittering each tick. `alpha = 1.0` is identical to `ingest`.
    pub fn ingest_blend(
        &mut self,
        origin_x: i32,
        origin_z: i32,
        side: usize,
        vx: &[f32],
        vz: &[f32],
        depth: &[f32],
        now: u64,
        alpha: f32,
    ) {
        let a = alpha.clamp(0.0, 1.0);
        let ia = 1.0 - a;
        for z in 0..side {
            let wz = origin_z + z as i32;
            for x in 0..side {
                let wx = origin_x + x as i32;
                let src = x + z * side;
                let key = (tile_of(wx), tile_of(wz));
                let tile = self.tiles.entry(key).or_insert_with(|| FlowTile::empty(now));
                let dst = cell_idx(local_of(wx), local_of(wz));
                tile.vx[dst] = tile.vx[dst] * ia + vx[src] * a;
                tile.vz[dst] = tile.vz[dst] * ia + vz[src] * a;
                // depth: first write replaces the -1 sentinel; later writes blend.
                tile.depth[dst] = if tile.depth[dst] < 0.0 {
                    depth[src]
                } else {
                    tile.depth[dst] * ia + depth[src] * a
                };
                tile.last_update = now;
            }
        }
    }

    /// Water depth (blocks) at a world cell, or -1 if no tile / never written.
    pub fn sample_depth(&self, wx: i32, wz: i32) -> f32 {
        match self.tiles.get(&(tile_of(wx), tile_of(wz))) {
            Some(t) => t.depth[cell_idx(local_of(wx), local_of(wz))],
            None => -1.0,
        }
    }

    /// Pack a `cells × cells` depth grid (1 cell per f32, row-major) centred on the player.
    /// Cells with no data are -1 so the shader falls back to its angle estimate there.
    /// Returns `([origin_x, origin_z, blocks_per_cell], cells)`.
    pub fn pack_depth_grid(
        &self,
        center_x: i32,
        center_z: i32,
        cells: usize,
        blocks_per_cell: i32,
    ) -> ([f32; 3], Vec<f32>) {
        let span = cells as i32 * blocks_per_cell;
        let origin_x = center_x - span / 2;
        let origin_z = center_z - span / 2;
        let mut grid = vec![-1.0f32; cells * cells];
        for cz in 0..cells {
            for cx in 0..cells {
                let sx = origin_x + cx as i32 * blocks_per_cell + blocks_per_cell / 2;
                let sz = origin_z + cz as i32 * blocks_per_cell + blocks_per_cell / 2;
                grid[cx + cz * cells] = self.sample_depth(sx, sz);
            }
        }
        ([origin_x as f32, origin_z as f32, blocks_per_cell as f32], grid)
    }

    /// Fade tiles that were NOT refreshed at `now` toward still, by `factor` (0..1).
    /// Tiles touched this tick keep their fresh vectors.
    pub fn decay(&mut self, now: u64, factor: f32) {
        let f = factor.clamp(0.0, 1.0);
        for tile in self.tiles.values_mut() {
            if tile.last_update == now {
                continue;
            }
            for v in &mut tile.vx {
                *v *= f;
            }
            for v in &mut tile.vz {
                *v *= f;
            }
        }
    }

    /// Drop tiles older than `max_age` (in the same units as `now`) or farther than
    /// `keep_radius` tiles from `(player_tx, player_tz)`. Returns how many were removed.
    pub fn evict(&mut self, now: u64, max_age: u64, player_tx: i32, player_tz: i32, keep_radius: i32) -> usize {
        let before = self.tiles.len();
        self.tiles.retain(|&(tx, tz), tile| {
            let age = now.saturating_sub(tile.last_update);
            let dx = (tx - player_tx).abs();
            let dz = (tz - player_tz).abs();
            age <= max_age && dx <= keep_radius && dz <= keep_radius
        });
        before - self.tiles.len()
    }

    /// Pack a coarse `cells × cells` flow grid centred on `(center_x, center_z)`, each cell
    /// `blocks_per_cell` wide, for upload as a uniform. Two cells per vec4 (`.xy` = even
    /// cell, `.zw` = odd) to halve the buffer. Returns
    /// `([origin_x, origin_z, blocks_per_cell, 1.0], grid)`; the `1.0` is the shader enable
    /// flag. Each cell samples the store at its centre — wide + low-res by design.
    pub fn pack_grid(
        &self,
        center_x: i32,
        center_z: i32,
        cells: usize,
        blocks_per_cell: i32,
    ) -> ([f32; 4], Vec<[f32; 4]>) {
        let span = cells as i32 * blocks_per_cell;
        let origin_x = center_x - span / 2;
        let origin_z = center_z - span / 2;
        let n = cells * cells;
        let mut grid = vec![[0.0f32; 4]; n.div_ceil(2)];
        for cz in 0..cells {
            for cx in 0..cells {
                let sx = origin_x + cx as i32 * blocks_per_cell + blocks_per_cell / 2;
                let sz = origin_z + cz as i32 * blocks_per_cell + blocks_per_cell / 2;
                let (fx, fz) = self.sample(sx, sz);
                let cell = cx + cz * cells;
                let v = &mut grid[cell / 2];
                if cell % 2 == 0 {
                    v[0] = fx;
                    v[1] = fz;
                } else {
                    v[2] = fx;
                    v[3] = fz;
                }
            }
        }
        (
            [origin_x as f32, origin_z as f32, blocks_per_cell as f32, 1.0],
            grid,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ingest_then_sample_roundtrips() {
        let mut s = FlowTileStore::new();
        let side = 2usize;
        // window anchored at world (10, 20): columns (10,20),(11,20),(10,21),(11,21)
        let vx = vec![0.5, 0.0, 0.0, 0.0];
        let vz = vec![0.0, 0.0, 0.0, -0.3];
        s.ingest(10, 20, side, &vx, &vz, 100);
        assert_eq!(s.sample(10, 20), (0.5, 0.0));
        assert_eq!(s.sample(11, 21), (0.0, -0.3));
        assert_eq!(s.sample(11, 20), (0.0, 0.0));
    }

    #[test]
    fn absent_tile_samples_zero() {
        let s = FlowTileStore::new();
        assert_eq!(s.sample(-9999, 9999), (0.0, 0.0));
    }

    #[test]
    fn persists_across_a_moved_window() {
        let mut s = FlowTileStore::new();
        // first window writes a current at world (5,5)
        let mut vx = vec![0.0; 4];
        vx[0] = 0.9;
        s.ingest(5, 5, 2, &vx, &vec![0.0; 4], 1);
        // a later, disjoint window elsewhere doesn't disturb the first tile's data
        s.ingest(200, 200, 2, &vec![0.0; 4], &vec![0.0; 4], 2);
        assert_eq!(s.sample(5, 5), (0.9, 0.0)); // still there
    }

    #[test]
    fn spans_tile_boundary_correctly() {
        let mut s = FlowTileStore::new();
        // window straddling x=32 (tile 0 -> tile 1)
        let side = 2usize;
        let vx = vec![1.0, 2.0, 3.0, 4.0]; // (31,0),(32,0),(31,1),(32,1)
        s.ingest(31, 0, side, &vx, &vec![0.0; 4], 1);
        assert_eq!(s.sample(31, 0).0, 1.0); // tile (0,0)
        assert_eq!(s.sample(32, 0).0, 2.0); // tile (1,0)
        assert_eq!(s.tile_count(), 2);
    }

    #[test]
    fn decay_fades_stale_tiles_only() {
        let mut s = FlowTileStore::new();
        s.ingest(5, 5, 1, &vec![1.0], &vec![0.0], 1); // tile A at t=1
        s.ingest(200, 5, 1, &vec![1.0], &vec![0.0], 2); // tile B at t=2 (fresh)
        s.decay(2, 0.5); // now=2: A is stale -> halved, B fresh -> unchanged
        assert!((s.sample(5, 5).0 - 0.5).abs() < 1e-6);
        assert!((s.sample(200, 5).0 - 1.0).abs() < 1e-6);
    }

    #[test]
    fn evict_drops_far_and_old() {
        let mut s = FlowTileStore::new();
        s.ingest(5, 5, 1, &vec![1.0], &vec![0.0], 100); // tile (0,0), recent
        s.ingest(5000, 5, 1, &vec![1.0], &vec![0.0], 100); // tile (156,0), far
        s.ingest(40, 5, 1, &vec![1.0], &vec![0.0], 1); // tile (1,0), old
        // player at tile (0,0); keep within 4 tiles, max age 50
        let removed = s.evict(100, 50, 0, 0, 4);
        assert_eq!(removed, 2); // far one + old one gone
        assert_eq!(s.sample(5, 5), (1.0, 0.0)); // near+recent kept
    }

    #[test]
    fn pack_grid_coverage_and_centre() {
        let mut s = FlowTileStore::new();
        // 4x4 grid, 32 blocks/cell, centred at (0,0): span 128, origin (-64,-64).
        // put a flow at the centre of cell (2,2): sx = -64 + 2*32 + 16 = 16.
        s.ingest(16, 16, 1, &vec![0.7], &vec![-0.4], 1);
        let (cov, grid) = s.pack_grid(0, 0, 4, 32);
        assert_eq!(cov, [-64.0, -64.0, 32.0, 1.0]);
        assert_eq!(grid.len(), 8); // 16 cells / 2
        // cell index = cx + cz*cells = 2 + 2*4 = 10 -> vec4[5], EVEN -> .xy
        assert!((grid[5][0] - 0.7).abs() < 1e-6);
        assert!((grid[5][1] - (-0.4)).abs() < 1e-6);
    }

    #[test]
    fn pack_grid_empty_is_zero() {
        let s = FlowTileStore::new();
        let (cov, grid) = s.pack_grid(100, 100, 8, 16);
        assert_eq!(cov[3], 1.0); // enable flag still set
        assert!(grid.iter().all(|v| *v == [0.0; 4]));
    }

    #[test]
    fn ingest_blend_smooths_toward_new() {
        let mut s = FlowTileStore::new();
        // first sample establishes 1.0; second sample of 0.0 with alpha 0.5 -> 0.5
        s.ingest_blend(0, 0, 1, &[1.0], &[0.0], &[2.0], 1, 1.0); // alpha 1 == plain ingest
        assert!((s.sample(0, 0).0 - 1.0).abs() < 1e-6);
        s.ingest_blend(0, 0, 1, &[0.0], &[0.0], &[2.0], 2, 0.5);
        assert!((s.sample(0, 0).0 - 0.5).abs() < 1e-6);
        s.ingest_blend(0, 0, 1, &[0.0], &[0.0], &[2.0], 3, 0.5);
        assert!((s.sample(0, 0).0 - 0.25).abs() < 1e-6); // keeps easing toward 0
    }

    #[test]
    fn ingest_blend_alpha_one_matches_ingest() {
        let mut a = FlowTileStore::new();
        let mut b = FlowTileStore::new();
        a.ingest(5, 5, 1, &[0.7], &[-0.2], 1);
        b.ingest_blend(5, 5, 1, &[0.7], &[-0.2], &[1.0], 1, 1.0);
        assert_eq!(a.sample(5, 5), b.sample(5, 5));
    }
    #[test]
    fn depth_roundtrips_and_missing_is_negative() {
        let mut s = FlowTileStore::new();
        let side = 2;
        let vx = vec![0.0; 4];
        let vz = vec![0.0; 4];
        let dep = vec![1.0, 3.0, 5.0, 7.0];
        s.ingest_blend(10, 20, side, &vx, &vz, &dep, 100, 1.0);
        assert!((s.sample_depth(10, 20) - 1.0).abs() < 1e-6);
        assert!((s.sample_depth(11, 21) - 7.0).abs() < 1e-6);
        // never-written cell far away → -1 sentinel
        assert!(s.sample_depth(9000, 9000) < 0.0);
    }

    #[test]
    fn pack_depth_grid_marks_empty_as_negative() {
        let s = FlowTileStore::new();
        let (cov, grid) = s.pack_depth_grid(0, 0, 4, 8);
        assert_eq!(cov[2], 8.0);
        assert_eq!(grid.len(), 16);
        assert!(grid.iter().all(|&d| d < 0.0)); // empty store → all no-data
    }
}
