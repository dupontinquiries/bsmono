# DISPATCH PACKET — r340 (7 threads · shared-library candidate hunt)

Seven threads, one per code cluster, hunting **shared-library extraction candidates** — duplicated
logic that should be consolidated into one shared module. The coordinator will receive all seven,
build an **overlap matrix**, dedup, and **compound-merge** the best version of each candidate. All
read `docs/dispatch_r340/SHARED_RULES.md` FIRST. Base = `unendlich-r339-missing-pieces-CHECKED.zip`.

## What every thread delivers
1. A **CANDIDATE REPORT** (`SHAREDLIB_REPORT-<wid>.md`): for each duplicated/consolidatable pattern
   in your cluster — name it, list the call sites (file:line), name the shared module it should live
   in (`util::*`, `sim::net::codec`, `world::field_ops`, …), and say whether you **extracted** it or
   only **propose** it. **Flag cross-cluster overlaps** ("this clamp helper likely also surfaces in
   sl-math") so the coordinator can dedup — assume the other six threads exist and may propose the
   same thing.
2. **Extracted modules** (optional but preferred when clean): a NEW self-contained pure module with
   the consolidated helper + ≥6 tests, and the call sites it would replace listed (don't rewrite all
   call sites — propose; the coordinator does the sweep so overlaps merge once).
3. A **PERF BONUS** (see below), additive, with a parity proof.

Keep extractions **small, named, self-contained** so the coordinator can pick the best of two
overlapping versions without untangling a mega-refactor.

## TWO HARD CONSTRAINTS (apply to every thread — violating either fails the return)
- **RESPECT THE PHYSICS FIELDS UNDERNEATH.** This engine simulates *continuous* physics fields in
  FP32/FP64 — temperature, moisture, wind, water, fire, storm, perception, navigation (see the
  `*_field` modules + `world::{fields, field_step, field_diffuse_ref, field_brush}`). These are real
  scalar/vector fields that diffuse/advect/integrate, **not** per-block flags. Do NOT flatten a field
  into booleans, do NOT change its numeric precision, do NOT assume discrete per-tick block updates.
  Any consolidation must preserve field semantics + bit-level numerics (prove parity).
- **NO MINECRAFT-STYLE ASSUMPTIONS.** This is not a block-type-per-cell game. It has sub-voxel
  geometry, continuous fields, real physics (buoyancy/flow/diffusion/advection), FP32/FP64 terrain,
  and dragons with genome+brain. Do NOT assume: 16³ chunks as a gameplay unit, instant block edits,
  flood-fill block lighting, block-grid pathfinding, static "water blocks", a fixed 6-face cube world,
  or biome==block-palette. When in doubt, read the field/sim code before consolidating.

## PERF BONUS (one per thread; additive, behind the shared-lib work)
Pick the one that fits your cluster:
- **(a) CPU-native multithreading** via the EXISTING pure primitive `util::task_lanes`
  (`map_pure`, `map_pure_threshold(items, min_parallel, f)`, `compute_patches`/`apply_patches`).
  Parallelize ONE data-parallel hot loop in your cluster: make the per-item kernel `Send + Sync` with
  no shared mutable state, route it through `task_lanes`, and **prove identical results to the serial
  path** (a parity test over a fixed input). Don't introduce locks in the hot path.
- **(b) Better GPU compression** (the GPU cluster): tighter packing/quantization of GPU-bound data
  (octree nodes, sparse mesh, q8/q4 weights) — CPU-side packing only; **shaders are off-limits**
  (`froxel_fill.comp`/`chunk.frag`), and any quantization change must stay byte-parity with its oracle.
Keep bonuses *small wins*; a perf hack that changes a physics result is rejected — prove parity.

**Merge order (coordinator):** sl-math → sl-batch → sl-fields → sl-net → sl-mesh → sl-worldgen → sl-gpu
(foundational helpers first; the coordinator de-overlaps as it goes).

---

## T1 `r340-sl-math` — math/util consolidation
**Cluster:** `util::{mathx, vecmath, simvec, fastmath, batch_kernel, bbox, hash_util}` + the ~161
clamp/lerp/smoothstep sites the r332 mathx refactor began.
**Find:** duplicated scalar/vector helpers across mathx↔vecmath↔fastmath↔simvec (e.g. multiple
clamp/lerp/normalize/dot impls), and remaining inline clamp/lerp sites that should call `mathx`.
**Extract/propose:** the canonical home for each (likely `util::mathx` scalar + `util::simvec`
batched). **Bonus (a):** confirm `simvec`/`batch_kernel` kernels are `task_lanes`-parallelizable and
add a parallel-vs-serial parity test for one batched op.
**RESPECT:** physics fields rely on these for FP32/FP64 exactness — any "faster" math must be
bit-identical (fastmath approximations stay opt-in, never silently swapped into a field path).

## T2 `r340-sl-batch` — the 67 `batch_*` kernels
**Cluster:** `sim/batch_*.rs` (67 modules) + `util::batch_kernel` (`map_into`).
**Find:** the shared map-over-slice boilerplate every `batch_*` repeats. **Extract/propose:** collapse
the boilerplate onto `batch_kernel` (a generic kernel + per-effect closure) — report which of the 67
fold cleanly and which have real per-kernel logic to keep. **Bonus (a):** route the heaviest batch
kernel through `task_lanes::map_pure_threshold` and prove serial==parallel.
**RESPECT:** each `batch_*` reads entity/physics state — preserve exact per-entity results; **no
Minecraft entity assumptions** (these drive dragons/mobs with continuous state, not block-mobs).

## T3 `r340-sl-fields` — physics field operators (HIGH-CARE)
**Cluster:** `world::{fields, field_step, field_diffuse_ref, field_brush}` + `*_field` modules
(`moisture_field`, `wind_field`, `fire_field`, `storm_field`, `water_field`, `particulate_field`,
`perception_field`, `cave_field`, `ember_field`, `canopy_field_shift`).
**Find:** the diffusion / advection / brush-stencil / clamp-to-bounds math these duplicate.
**Extract/propose:** a shared `world::field_ops` (e.g. `diffuse`, `advect`, `brush_stamp`,
`step_bounded`) the individual fields call. **Bonus (a):** parallelize one field's grid update via
`task_lanes` with a parity test.
**RESPECT (this is THE field thread):** preserve continuous-field semantics + FP precision EXACTLY —
a field is a sampled scalar/vector grid that evolves by PDE-like stencils, not a block array. Prove
the consolidated `field_ops` reproduces each field's current output bit-for-bit on a fixed input.

## T4 `r340-sl-net` — net codec consolidation
**Cluster:** `sim::net::{wire, chunk_stream, entity_snapshot, world_advert, world_presence}`.
**Find:** every one of these re-implements the bounded `Reader` (`take/u8/u16/u32/u64`) + the
`magic 0x554E`/version frame header + length-capped string/vec reads.
**Extract/propose:** a shared `sim::net::codec` (`Reader`, `Writer`, `frame_header`/`check_header`,
capped `read_str`/`read_vec`). **Bonus (a):** a small alloc-free encode path (write into a reused
buffer) with a round-trip parity test.
**RESPECT:** wire byte layouts are EXACT — the consolidated codec must produce byte-identical frames
(the existing per-module tests must still pass verbatim). Don't change magic/version/field order.

## T5 `r340-sl-mesh` — mesh primitive consolidation
**Cluster:** `render::{mesher, gpu_meshing_sparse}` + `sim::{mesh_partition, mesh_budget}`.
**Find:** shared quad emission / face-normal / greedy-merge / AABB logic duplicated across the
mesher and the partition/budget cores. **Extract/propose:** shared mesh primitives (face-normal
table — the canonical `FACE_NORMALS`, quad struct, greedy-merge helper). **Bonus (a) or (b):**
either parallelize per-chunk meshing via `task_lanes` (parity test), OR tighter GPU mesh packing in
`gpu_meshing_sparse` (byte-parity).
**RESPECT:** the world is sub-voxel + field-decorated (foliage_sway, water surface), **not** a
6-face cube grid — keep sub-voxel + the directional sub-mesh scheme (r332 `mesh_partition`) intact.

## T6 `r340-sl-worldgen` — worldgen primitive consolidation
**Cluster:** `world::{cpu_generator, biomes, flora_placement, vegetation_scatter, structure_place,
structure_grammar, rare_structure, plant_blueprint}`.
**Find:** duplicated noise / density-curve / deterministic-hash / placement-roll logic.
**Extract/propose:** shared `world::worldgen_noise` / density helpers (and route the r336 `Biome::
flora_density` consumers through it). **Bonus (a):** parallelize per-region/per-chunk generation via
`task_lanes` with a determinism+parity test (same seed → same world, serial==parallel).
**RESPECT:** terrain + climate are CONTINUOUS FIELDS (FP height, temperature/moisture) sampled to
decide biomes — **not** a Minecraft biome-palette per block. Preserve the field-based generation and
exact seeded determinism.

## T7 `r340-sl-gpu` — GPU buffer/compression consolidation (the GPU-compression thread)
**Cluster:** `render::{mesh_buffer_pool, octree_buffer, chunk_indirect, q8, neural_gemm_pipeline,
gpu_meshing_sparse}`.
**Find:** shared GPU-buffer management (span/free-list alloc, upload staging, indirect-arg packing)
duplicated across the buffer pools + the quantization/packing patterns. **Extract/propose:** a shared
`render::gpu_pack` (or buffer-pool trait). **Bonus (b) — GPU compression (primary for this thread):**
tighter packing of GPU-bound data — octree node compression, sparse-mesh word packing, or q4 vs q8
weight density — CPU-side packing with a documented layout + exact unpack round-trip. **Shaders are
off-limits** (`froxel_fill.comp`/`chunk.frag`); q8/q4 changes must stay byte-parity with the oracle.
**RESPECT:** the renderer is field-driven (froxel volumetrics, neural shading) — don't break the
data the fields/shaders consume; compression is lossless-or-prove-the-bound.

---

### Coordinator checklist (compound merge)
- [ ] Cut `worker-<WID>-r340.zip` = base + `WORKER_ID` + that thread's brief + `SHARED_RULES.md`.
- [ ] On return: collate all 7 `SHAREDLIB_REPORT`s → an **overlap matrix** (which candidate proposed
      by ≥2 threads). For each overlap pick ONE canonical version, merge it, then sweep the union of
      call sites once. Gate each consolidated module @1.96; verify every RESPECT parity claim (field
      bit-parity, wire byte-parity, quantize byte-parity, serial==parallel). Roll `unendlich-r341-
      sharedlib-merged.zip`.
- [ ] Reject any return that flattened a field, changed wire/quantize bytes, or made a block-grid
      assumption — note it in the ledger.
