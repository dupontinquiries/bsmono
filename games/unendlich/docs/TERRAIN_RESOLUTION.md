# Terrain resolution and smoothing — the decision, and the arithmetic behind it

**Question:** the world reads as blocky. Shrink the block?

**Decision:** no, not first. Add a per-cell density scalar and change the
surface extractor. Keep `BLOCK_M` at 1.0. Revisit 0.5 m only against a profile
on ARM, never on principle.

This file exists so the numbers do not have to be rediscovered from memory the
next time someone proposes it.

---

## Why not smaller cells

`CHUNK_SIZE` is 32 (`src/constants.rs`), so a chunk covers 32 m at 1 m blocks.

| block size | cells per chunk | vs today | chunk span |
|---|---|---|---|
| 1.0 m | 32,768 | 1x | 32 m |
| 0.5 m | 262,144 | **8x** | 16 m |
| 0.1 m | 32,768,000 | **1000x** | 3.2 m |

0.1 m is not a tuning question. At one byte per cell that is 32 MB per chunk
against 32 KB, before any field data, and generation, the octree build and the
streamer all scale with it.

**LOD does not rescue this, and it is important to understand why.**
`lod::lod_cell_stride` samples every Nth block **from the full-resolution
source chunk** at mesh time — strides 1/2/4/8 for Near/Mid/Far/Distant. It is
a *mesh* LOD. It reduces triangles and meshing work. It does not reduce
storage, worldgen, or field simulation, all of which run at full resolution
regardless of distance. And the Near band (`NEAR_OUTER_DEFAULT = 8` chunks) is
stride 1 by definition, so the near field pays the multiplier in full.

Two further multipliers people forget:

* **Fields cost more than terrain.** A registered `f32` field is 4 bytes per
  cell against 1 byte for a block id — 128 KB per chunk per field versus 32 KB
  for the blocks. Temperature plus humidity plus a density field already
  outweigh the terrain data by a wide margin, and they scale with cell count
  too.
* **The weather sim is per-cell work**, so it pays the multiplier as compute,
  not just memory. It is an argument for caution here, not reassurance.

## Why the blockiness is not actually a cell-size problem

"No cubes"-style smoothing of a voxel world looks bad because it smooths a
**binary occupancy field**. A cell is solid or it is not, so there is no
information about where inside the cell the surface sits, and the smoother has
to invent it. That is what produces blobby geometry and erratic step heights —
and shrinking cells makes the artefacts smaller without making them stop.

The fix is a scalar per cell. Store a density, extract the isosurface with
surface nets or dual contouring, and the surface can sit anywhere within a
cell. Effective precision stops being the grid and becomes the float. Slopes
become continuous, so the step-height navigation problem disappears rather
than shrinking.

**The storage for this already exists.** The field system is per-cell `f32`
indexed by a registry (`ChunkManager::field_at`, `FieldRegistry`,
`chunk_field_snapshot`) — the same machinery carrying temperature and
humidity. A `density` field is a new registry entry, not new infrastructure.

**What has to change is the extractor.** `render/mesher.rs` is greedy meshing:
it merges quads of the same block type, which is inherently the blocky path.
Greedy meshing and smooth terrain are mutually exclusive. That is the real
decision here, not the block size.

## The hard part, which is not the memory

LOD across an isosurface is a genuinely hard problem. Mismatched-resolution
neighbours produce cracks and holes; the fixes are transvoxel, skirts, or
explicit LOD stitching.

This engine already has the block-era version of the problem solved:
`lod::cull_boundary_face(self_stride, neighbor_stride, neighbor_solid)`
conservatively declines to cull across a stride change. That works for cubes,
where a mismatch leaves a seam of duplicated quads. A smooth surface does not
get off that lightly — a stride mismatch leaves a hole you can see through.

**This cost exists at any block size.** It is the reason to change one thing at
a time.

## Sequence

1. Add the `density` field. No storage-model change, no visual change.
2. Replace the terrain extractor with surface nets / dual contouring reading
   density. Keep greedy meshing for anything built out of discrete blocks.
3. Solve LOD stitching. Expect this to be the bulk of the work.
4. **Measure on ARM** (postmarketOS / Fairphone / PinePhone class).
5. Only then consider 0.5 m, and only with the profile in hand.

Doing 2 and 5 together means a slow or cracked result with no way to tell
which change caused it.

## What this does NOT fix

Terrain goes smooth; anything built out of blocks stays quantised at
`BLOCK_M`. For a realism-directed game that is the right split: voxels are the
ground and the destructible volume, structures and machines are meshes.

If detail is later wanted specifically near the player, the octree already
supports depth-varying resolution, and that is a cheaper lever than flattening
the whole grid.

## Consequences for the seam

Good ones, and they are already half-built in
`games/shared/worldhost-unendlich`:

* `WorldHost::clearance` currently returns a **step** (`+BLOCK_M` in air,
  `-BLOCK_M/2` in solid) because binary occupancy carries no distance. A
  density field is a coarse SDF, so `clearance` could return a real distance.
* That directly improves `blackstar-rs/src/footing.rs`, whose sphere-trace
  currently crawls one cell per iteration against `MAX_STEPS = 64`. With a
  true distance it sphere-traces properly and reaches much further for the
  same cost.
* `ground_h`'s `(y + 1)` top-face convention in the adapter becomes an
  isosurface height, which is more natural, not less. The test
  `the_ground_is_the_top_face_of_the_top_block_not_its_index` is the one that
  will need rewriting when this lands — deliberately, and it says why.
