//! Water field — the field-driven fluid foundation (roadmap steps 1–2
//! in `docs/CLIMATE_PATH.md`): continuous per-column water depth plus a
//! volume-conserving settle/flow pass. State only; rendering (mesher
//! reads the surface height) and cross-chunk flow are later steps.
//!
//! # Model
//!
//! A per-column heightfield: `depth[x,z]` is the water column depth (in
//! blocks, fractional allowed) standing on the terrain at column
//! `(x, z)`. The water *surface* height of a column is
//! `ground[x,z] + depth[x,z]`. Water flows from higher surfaces to lower
//! neighbors until surfaces equalize (a lake finds its level). This is
//! the standard lightweight "shallow water / heightfield" representation
//! — it can't model overhangs or waterfalls (those want a 3D cell field,
//! a later step), but it's exactly right for lakes, oceans, and filling
//! surface depressions, which is what step 1 targets.
//!
//! Indexing matches `Chunk`: `idx = x + z * CHUNK_SIZE`, `x` fastest.
//!
//! # Determinism & conservation
//!
//! `settle_step` is deterministic and conserves total volume exactly
//! (every transfer is subtracted from one column and added to a neighbor
//! by the same amount) while keeping depth non-negative (a column never
//! sends more water than it holds). Boundaries are closed (no flux out of
//! the chunk) — cross-chunk flow is a filed follow-up, mirroring how the
//! mesher gained cross-chunk neighbor sampling.

use crate::constants::CHUNK_SIZE;
use crate::world::blocks::BlockId;
use crate::world::chunk::Chunk;

/// Columns per chunk (32×32 = 1024 at CHUNK_SIZE=32). 4 KB per chunk as
/// `f32` — far lighter than a full 3D field (which would be 128 KB).
pub const WATER_AREA: usize = (CHUNK_SIZE * CHUNK_SIZE) as usize;

/// Default flood-fill reach (in BFS rings) and volume capacity of an
/// absorbent ("sponge") interaction — see [`WaterField::absorb_at`]. These
/// mirror the original block-BFS limits this is ported from (a depth-6
/// flood, ~128 cells of capacity), adapted to the column heightfield.
pub const SPONGE_RADIUS: u32 = 6;
pub const SPONGE_CAPACITY: f64 = 128.0;

/// Per-column water depth field for one chunk.
#[derive(Clone, Debug)]
pub struct WaterField {
    /// Water column depth per `(x, z)`, indexed `x + z*CHUNK_SIZE`.
    /// `0.0` = dry. Always `>= 0`.
    pub depth: Vec<f32>,
}

#[inline]
fn idx(x: usize, z: usize) -> usize {
    x + z * CHUNK_SIZE as usize
}

impl WaterField {
    /// A dry field (all columns zero).
    pub fn empty() -> Self {
        Self { depth: vec![0.0; WATER_AREA] }
    }

    /// Seed a field from a chunk's fluid blocks: each column's depth is
    /// the count of `fluid` cells in that column (its water volume in
    /// block units). This is the "seed from worldgen" step — the field
    /// the simulation starts from before settling. Pure; the invariant
    /// `total_volume() == (number of fluid blocks in the chunk)` holds by
    /// construction, so settling (which conserves volume) preserves the
    /// world's water mass.
    ///
    /// Nothing calls this yet — it's the bridge the sim integration will
    /// use to initialize a persistent per-chunk field. Reading is via the
    /// public `Chunk::get`, so this stays in the `world` layer (no
    /// dependency on the render-side mesher).
    pub fn seed_from_chunk(chunk: &Chunk, fluid: BlockId) -> WaterField {
        let s = CHUNK_SIZE as i32;
        let mut f = WaterField::empty();
        for z in 0..s {
            for x in 0..s {
                let mut depth = 0.0f32;
                for y in 0..s {
                    if chunk.get(x, y, z) == fluid {
                        depth += 1.0;
                    }
                }
                if depth > 0.0 {
                    f.set(x as usize, z as usize, depth);
                }
            }
        }
        f
    }

    #[inline]
    pub fn at(&self, x: usize, z: usize) -> f32 {
        self.depth[idx(x, z)]
    }

    #[inline]
    pub fn set(&mut self, x: usize, z: usize, d: f32) {
        self.depth[idx(x, z)] = d.max(0.0);
    }

    /// Total water volume (sum of depths). Invariant under `settle_step`.
    pub fn total_volume(&self) -> f64 {
        self.depth.iter().map(|&d| d as f64).sum()
    }

    /// One relaxation step toward equilibrium over the given ground
    /// heightfield (`ground[x,z]`, same indexing/length as `depth`).
    ///
    /// `rate` in `(0, 1]` controls how aggressively surfaces equalize per
    /// step; `0.5` is stable and non-oscillating. Each column sends water
    /// to lower-surface neighbors, scaled so it never sends more than it
    /// holds — so depth stays `>= 0` and total volume is exactly conserved.
    pub fn settle_step(&mut self, ground: &[f32], rate: f32) {
        debug_assert_eq!(ground.len(), WATER_AREA);
        debug_assert_eq!(self.depth.len(), WATER_AREA);
        let s = CHUNK_SIZE as usize;
        let rate = rate.clamp(0.0, 1.0);

        // Delta accumulator (net change applied after all transfers are
        // computed — Jacobi style, so order-independent / deterministic).
        let mut delta = vec![0.0f32; WATER_AREA];

        for z in 0..s {
            for x in 0..s {
                let c = idx(x, z);
                let dc = self.depth[c];
                if dc <= 0.0 {
                    continue;
                }
                let sc = ground[c] + dc; // this column's surface height

                // Collect in-chunk neighbors (closed boundary).
                let mut neighbors: [usize; 4] = [usize::MAX; 4];
                let mut ncount = 0usize;
                if x + 1 < s { neighbors[ncount] = idx(x + 1, z); ncount += 1; }
                if x > 0     { neighbors[ncount] = idx(x - 1, z); ncount += 1; }
                if z + 1 < s { neighbors[ncount] = idx(x, z + 1); ncount += 1; }
                if z > 0     { neighbors[ncount] = idx(x, z - 1); ncount += 1; }

                // Desired outflux to each lower-surface neighbor. The 0.5
                // factor removes only half the surface difference per step
                // (split with the neighbor) to avoid overshoot/oscillation.
                let mut wants: [f32; 4] = [0.0; 4];
                let mut want_total = 0.0f32;
                for k in 0..ncount {
                    let ni = neighbors[k];
                    let diff = sc - (ground[ni] + self.depth[ni]);
                    if diff > 0.0 {
                        let want = 0.5 * diff * rate;
                        wants[k] = want;
                        want_total += want;
                    }
                }
                if want_total <= 0.0 {
                    continue;
                }
                // Never send more than we hold: scale outflux down if the
                // desired total exceeds the column's depth. Guarantees
                // depth >= 0 and exact volume conservation.
                let scale = if want_total > dc { dc / want_total } else { 1.0 };
                for k in 0..ncount {
                    if wants[k] > 0.0 {
                        let amt = wants[k] * scale;
                        delta[c] -= amt;
                        delta[neighbors[k]] += amt;
                    }
                }
            }
        }

        for i in 0..WATER_AREA {
            self.depth[i] = (self.depth[i] + delta[i]).max(0.0);
        }
    }

    /// Absorb water starting at column `(sx, sz)`, flood-filling outward
    /// through connected wet columns and removing their depth, up to
    /// `capacity` total volume and `max_radius` BFS rings. Returns the
    /// volume actually absorbed.
    ///
    /// This is the field-driven port of an absorbent "sponge" block (your
    /// mod's `SuperSponge`): where the original BFS-removes connected water
    /// *blocks* in 3D (capped at flood depth 6 and ~128 cells), this removes
    /// connected water *column depth* in the 2D heightfield — the vertical
    /// extent is absorbed implicitly by draining the column. Use
    /// [`SPONGE_RADIUS`] / [`SPONGE_CAPACITY`] for the sponge defaults.
    ///
    /// Because the field is authoritative (per `docs/CLIMATE_PATH.md`), a
    /// sponge dropped in a lake doesn't permanently dent it: after
    /// absorbing, [`settle_step`](Self::settle_step) lets the surrounding
    /// water flow back into the drained pocket over the following ticks. The
    /// transient hollow — and a saturated/"wet" sponge once `absorbed > 0` —
    /// are the visible effects, not a hole in the ocean.
    ///
    /// Pure and side-effect-free beyond mutating this field; nothing calls
    /// it yet (the block-behavior hook that will is the integration step,
    /// like `seed_from_chunk`). Conserves accounting exactly: total volume
    /// drops by precisely the returned amount.
    pub fn absorb_at(&mut self, sx: usize, sz: usize, max_radius: u32, capacity: f64) -> f64 {
        let s = CHUNK_SIZE as usize;
        if sx >= s || sz >= s || capacity <= 0.0 {
            return 0.0;
        }
        let mut absorbed = 0.0f64;
        let mut visited = vec![false; WATER_AREA];
        let mut queue: std::collections::VecDeque<(usize, usize, u32)> =
            std::collections::VecDeque::new();
        visited[idx(sx, sz)] = true;
        queue.push_back((sx, sz, 0));

        while let Some((x, z, ring)) = queue.pop_front() {
            if absorbed >= capacity {
                break;
            }
            let c = idx(x, z);
            let d = self.depth[c] as f64;
            if d > 0.0 {
                let take = d.min(capacity - absorbed);
                self.depth[c] = (d - take) as f32;
                absorbed += take;
            }
            if ring >= max_radius {
                continue;
            }
            // Expand to 4-connected neighbors still holding water. Closed
            // chunk boundary (matches settle_step); cross-chunk absorption
            // is a follow-up like cross-chunk flow.
            let nbrs: [(usize, usize); 4] = [
                (x.wrapping_add(1), z),
                (x.wrapping_sub(1), z),
                (x, z.wrapping_add(1)),
                (x, z.wrapping_sub(1)),
            ];
            for (nx, nz) in nbrs {
                if nx < s && nz < s {
                    let ni = idx(nx, nz);
                    if !visited[ni] && self.depth[ni] > 0.0 {
                        visited[ni] = true;
                        queue.push_back((nx, nz, ring + 1));
                    }
                }
            }
        }
        absorbed
    }
}

impl Default for WaterField {
    fn default() -> Self {
        Self::empty()
    }
}

/// Per-column ground (floor) height for settling a fluid field over a
/// chunk's terrain: the height of the topmost cell that is neither air
/// nor the fluid, as a count from the chunk floor (topmost ground `y` + 1).
/// `0.0` means an empty or fluid-only column. Same indexing/length as
/// [`WaterField::depth`]. This is the `ground` argument
/// [`WaterField::settle_step`] needs.
///
/// APPROXIMATION: `BlockId`'s solidity predicates aren't part of this
/// crate slice (blocks.rs lives elsewhere), so "ground" here is "anything
/// that isn't `Air` or `fluid`". That folds the *other* fluid and thin
/// plants (grass tufts, flowers) into ground. For settling lakes/oceans
/// over solid terrain — the step-1 target — this is accurate; the sim
/// integration can swap in a real `is_solid` predicate when it has one.
pub fn ground_heights_from_chunk(chunk: &Chunk, fluid: BlockId) -> Vec<f32> {
    let s = CHUNK_SIZE as i32;
    let mut g = vec![0.0f32; WATER_AREA];
    for z in 0..s {
        for x in 0..s {
            let mut top = 0i32; // topmost ground y + 1
            for y in 0..s {
                let b = chunk.get(x, y, z);
                if b != BlockId::Air && b != fluid {
                    top = y + 1;
                }
            }
            g[(x + z * s) as usize] = top as f32;
        }
    }
    g
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_ground(h: f32) -> Vec<f32> {
        vec![h; WATER_AREA]
    }

    #[test]
    fn empty_field_is_dry() {
        let w = WaterField::empty();
        assert_eq!(w.total_volume(), 0.0);
        assert!(w.depth.iter().all(|&d| d == 0.0));
    }

    #[test]
    fn settle_conserves_volume_on_flat_ground() {
        let mut w = WaterField::empty();
        w.set(16, 16, 100.0); // a tall column in the middle
        let ground = flat_ground(0.0);
        let v0 = w.total_volume();
        for _ in 0..200 {
            w.settle_step(&ground, 0.5);
        }
        let v1 = w.total_volume();
        assert!((v0 - v1).abs() < 1e-3, "volume drifted: {} -> {}", v0, v1);
    }

    #[test]
    fn water_spreads_from_a_spike_on_flat_ground() {
        let mut w = WaterField::empty();
        w.set(16, 16, 50.0);
        let ground = flat_ground(0.0);
        for _ in 0..500 {
            w.settle_step(&ground, 0.5);
        }
        // The spike should have spread to neighbors (center lower, edges
        // wetter than they started).
        assert!(w.at(16, 16) < 50.0, "center should have drained, got {}", w.at(16, 16));
        assert!(w.at(15, 16) > 0.0 || w.at(17, 16) > 0.0,
            "water should have spread to neighbors");
    }

    #[test]
    fn depth_stays_non_negative() {
        let mut w = WaterField::empty();
        w.set(16, 16, 10.0);
        // Sloped ground (drains toward +x).
        let mut ground = vec![0.0f32; WATER_AREA];
        let s = CHUNK_SIZE as usize;
        for z in 0..s {
            for x in 0..s {
                ground[x + z * s] = (s - x) as f32; // higher at -x, lower at +x
            }
        }
        for _ in 0..1000 {
            w.settle_step(&ground, 1.0);
            assert!(w.depth.iter().all(|&d| d >= 0.0), "negative depth appeared");
        }
    }

    #[test]
    fn water_flows_downhill() {
        // A slope descending in +x; water poured at the high (-x) end
        // should accumulate toward the low (+x) end.
        let s = CHUNK_SIZE as usize;
        let mut ground = vec![0.0f32; WATER_AREA];
        for z in 0..s {
            for x in 0..s {
                ground[x + z * s] = (s - x) as f32;
            }
        }
        let mut w = WaterField::empty();
        // Pour along the highest column.
        for z in 0..s {
            w.set(0, z, 20.0);
        }
        let low_before: f32 = (0..s).map(|z| w.at(s - 1, z)).sum();
        for _ in 0..2000 {
            w.settle_step(&ground, 0.5);
        }
        let low_after: f32 = (0..s).map(|z| w.at(s - 1, z)).sum();
        assert!(low_after > low_before,
            "water should accumulate downhill: low end {} -> {}", low_before, low_after);
    }

    #[test]
    fn settle_is_deterministic() {
        let ground = flat_ground(0.0);
        let mut a = WaterField::empty();
        let mut b = WaterField::empty();
        a.set(10, 12, 30.0);
        b.set(10, 12, 30.0);
        for _ in 0..50 {
            a.settle_step(&ground, 0.5);
            b.settle_step(&ground, 0.5);
        }
        assert_eq!(a.depth, b.depth);
    }

    #[test]
    fn seed_from_chunk_counts_fluid_per_column() {
        use crate::world::chunk::{Chunk, ChunkCoord};
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        c.set(4, 2, 7, BlockId::Water);
        c.set(4, 3, 7, BlockId::Water);
        c.set(4, 4, 7, BlockId::Water);
        let f = WaterField::seed_from_chunk(&c, BlockId::Water);
        assert_eq!(f.at(4, 7), 3.0, "column should hold 3 water units");
        assert_eq!(f.at(0, 0), 0.0, "dry column stays dry");
        assert_eq!(f.total_volume(), 3.0, "volume == fluid block count");
    }

    #[test]
    fn ground_heights_find_topmost_solid_under_water() {
        use crate::world::chunk::{Chunk, ChunkCoord};
        let s = CHUNK_SIZE as usize;
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        c.set(2, 5, 9, BlockId::Stone); // ground top at y=5 -> height 6
        c.set(2, 6, 9, BlockId::Water); // water above is not ground
        let g = ground_heights_from_chunk(&c, BlockId::Water);
        assert_eq!(g[2 + 9 * s], 6.0, "ground height = topmost solid y + 1");
        assert_eq!(g[0], 0.0, "empty column has zero ground");
    }

    #[test]
    fn seed_then_settle_conserves_seeded_volume() {
        use crate::world::chunk::{Chunk, ChunkCoord};
        let mut c = Chunk::empty(ChunkCoord::new(0, 0, 0));
        for y in 0..5 {
            c.set(8, y, 8, BlockId::Water);
        }
        let mut f = WaterField::seed_from_chunk(&c, BlockId::Water);
        let ground = ground_heights_from_chunk(&c, BlockId::Water);
        let v0 = f.total_volume();
        assert_eq!(v0, 5.0);
        for _ in 0..100 {
            f.settle_step(&ground, 0.5);
        }
        assert!((f.total_volume() - v0).abs() < 1e-3,
            "seed→settle must conserve volume: {} -> {}", v0, f.total_volume());
    }

    // ── Absorbent ("sponge") interaction — absorb_at ────────────────────

    fn uniform(d: f32) -> WaterField {
        let mut f = WaterField::empty();
        for v in f.depth.iter_mut() {
            *v = d;
        }
        f
    }

    #[test]
    fn absorb_fills_taxicab_disk_within_radius() {
        // Uniform wet field, big capacity → BFS fills the taxicab disk of
        // the given radius: 2r²+2r+1 columns (= 85 at r=6), each depth 1.
        let mut f = uniform(1.0);
        let a = f.absorb_at(16, 16, 6, 1000.0);
        assert!((a - 85.0).abs() < 1e-6, "taxicab disk r=6 should be 85, got {}", a);
    }

    #[test]
    fn absorb_respects_capacity() {
        let mut f = uniform(1.0);
        let v0 = f.total_volume();
        let a = f.absorb_at(16, 16, SPONGE_RADIUS, 50.0);
        assert!((a - 50.0).abs() < 1e-6, "capped at 50, got {}", a);
        assert!((f.total_volume() - (v0 - 50.0)).abs() < 1e-6);
    }

    #[test]
    fn absorb_takes_fractional_last_column() {
        let mut f = uniform(1.0);
        let a = f.absorb_at(16, 16, SPONGE_RADIUS, 49.5);
        assert!((a - 49.5).abs() < 1e-5, "fractional cap, got {}", a);
    }

    #[test]
    fn absorb_is_blocked_by_dry_gap() {
        // Dry moat at taxicab distance 2 confines absorption to the 5
        // connected inside columns; outside water is unreachable.
        let mut f = WaterField::empty();
        for z in 0..CHUNK_SIZE as usize {
            for x in 0..CHUNK_SIZE as usize {
                let td = (x as i32 - 16).abs() + (z as i32 - 16).abs();
                if td != 2 {
                    f.set(x, z, 1.0);
                }
            }
        }
        let a = f.absorb_at(16, 16, 6, 1000.0);
        assert!((a - 5.0).abs() < 1e-6, "moat should confine to 5 cols, got {}", a);
    }

    #[test]
    fn absorb_conserves_accounting() {
        let mut f = WaterField::empty();
        f.set(16, 16, 3.0);
        f.set(17, 16, 2.0);
        f.set(16, 17, 1.5);
        let v0 = f.total_volume();
        let a = f.absorb_at(16, 16, 6, 1000.0);
        assert!((v0 - f.total_volume() - a).abs() < 1e-6, "volume drop must equal absorbed");
        assert!(f.total_volume() < 1e-6, "all connected water absorbed");
    }

    #[test]
    fn absorb_then_settle_refills_the_pocket() {
        // Field is authoritative: a sponge in a lake clears a transient
        // pocket, then surrounding water flows back when it settles.
        let mut f = uniform(2.0);
        let ground = vec![0.0f32; WATER_AREA];
        f.absorb_at(16, 16, 3, 1000.0);
        let drained = f.at(16, 16);
        assert!(drained < 0.01, "center should be drained, got {}", drained);
        for _ in 0..200 {
            f.settle_step(&ground, 0.5);
        }
        assert!(f.at(16, 16) > 0.1, "water should flow back into the pocket");
    }
}
