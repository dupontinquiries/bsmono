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

* **Fields are cheap, and this file used to claim otherwise.** An earlier
  revision said a registered `f32` field costs 4 bytes per CELL — 128 KB per
  chunk. That is wrong. `fields.rs` sets `FIELD_RESOLUTION = 8`, so a field
  grid is 8³ = 512 samples per 32³ chunk: **2 KB**, with `sample_at_block`
  mapping a block coordinate onto it by dividing by 32. Fields are a coarse
  overlay, not a per-voxel store, and they are not the memory problem.
  Corrected because the mistaken figure was being used as an argument against
  a smaller block, and it was not a real one.
* **The weather sim is per-cell work**, so it pays the multiplier as compute.
  Still a reason for caution, though a smaller one than the line above
  implied.

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

**The storage does NOT already exist, and this file used to say it did.** The
field system looked like the answer — `ChunkManager::field_at`,
`FieldRegistry`, `chunk_field_snapshot`, the machinery already carrying
temperature and humidity. It is not, for the reason above: at
`FIELD_RESOLUTION = 8` a field grid is FOUR TIMES COARSER than the block grid
it overlays. Density must be at least per-block or it cannot say where inside
a block the surface sits, which is the entire point. Register `density` as a
`FieldDef` and you get a working, useless, blurry answer.

**What it actually needs: a per-block scalar beside the block ids** — and `u8`,
not `f32`. The extractor's own measured precision bound is about 0.2 cells
(`isosurface`'s `a_sphere_comes_out_round`); 8 bits gives 1/256 of a cell,
two orders of magnitude finer than the method can use. That puts density at
32 KB per chunk, the same as the block ids, so chunk memory roughly DOUBLES
rather than quintupling. `isosurface::Field` is a trait over `f32`, so a `u8`
store maps onto it at sample time with no change to the extractor.

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

**This cost existed at any block size**, which was the reason to change one
thing at a time. It is now paid.

## Sequence

1. Add the `density` field. No storage-model change, no visual change.
2. Replace the terrain extractor with surface nets / dual contouring reading
   density. Keep greedy meshing for anything built out of discrete blocks.
3. ~~Solve LOD stitching.~~ **DONE — `games/shared/isosurface`.** Expected to
   be the bulk of the work; was about forty lines and a composition rule. A
   boundary cell facing a coarser neighbour is meshed on the NEIGHBOUR'S cell
   grid (`Seams`), so both sides interpolate the same samples: 1.46 cells of
   disagreement down to 0.14. The remainder is structural — abutting chunks
   own different cells either side of the plane — and closes completely when
   the fine chunk meshes one coarse footprint PAST the shared plane, at which
   point it owns the neighbour's cell, computes the identical vertex, and
   emits the bridging quads. Every coarse vertex matches to within 1e-3. What
   is left is SCHEDULING: which chunk owns the overlap, and re-meshing when a
   neighbour's stride changes.
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
