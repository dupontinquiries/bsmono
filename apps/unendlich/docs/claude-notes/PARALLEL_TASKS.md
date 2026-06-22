# Parallel-task tracker (as of RC30 ship)

Current snapshot of what's in-flight, done this session, and queued
for future work. Designed to be a single-page status for picking
back up next session.

## Just shipped this session

### Applied + validated on hardware

- **RC29 v0.24.63-adaptive-uploads** — AIMD upload controller,
  env-gated `UNENDLICH_ADAPTIVE_UPLOADS=1`. Validated: the controller
  fired exactly as designed during initial load (64→8→128, 42
  adjustments). Steady-state correctly silent at ceiling 128.
- **RC24-28 entity scaffold (combined)** — 1570 LoC `world/entity.rs`,
  39 tests passing. Module compiles + tests pass; not yet wired into
  the runtime.

### Just shipped, pending user apply + test

- **RC30 omnibus** (`unendlich-rc30-omnibus.zip`, 8 files):
  - submit-skip RC (env `UNENDLICH_SUBMIT_SKIP=1`): caches
    "world is stationary" signal to skip the 2197-chunk shell walk.
    Expected drop: cpu.submit ~800ms/sec → ~0ms/sec while stationary.
    Canonical readme in `docs/claude-notes/v0.24.64-submit-skip-readme.md`.
  - meshing_pool.rs test sites: added `octree: None` to 8 sites.
    Unblocks `cargo test --features render --lib`.
  - octree.rs test typo: `since_tick: 11` → `since_tick: 1`.
  - octree_raycast.rs: REAL BUG — NaN from 0*INF in slab_intersect
    when ray parallel to axis AND origin on slab face. Fixed.
  - 4 unused-import / unused-mut warnings.

- **RC31 slack-scheduler** (`unendlich-rc31-slack-scheduler.zip`, 1 file):
  - Stage 1 Option A from the upload-scheduler design memo.
  - Env-gated `UNENDLICH_SLACK_SCHEDULER=1`. Tightens per-frame
    upload time budget based on remaining slack to frame target.
  - Composes with RC29 (count cap, GPU-pressure reactive) and RC30
    (submit skip, world-stationary reactive). Each targets a
    different metric.
  - Stacks ON TOP of RC30; the bin file is a strict superset.

- **RC32 biased-walk + upload-priority** (`unendlich-rc32-biased-walk-and-upload-priority.zip`, 1 file):
  - Stage 1 Option D + Stage 2 from the design memo.
  - Two independent env-gated flags packaged together:
    - `UNENDLICH_BIASED_SHELL_WALK=1` — reorder shell walk by
      forward dot product (chunks player is looking at first).
    - `UNENDLICH_UPLOAD_PRIORITY=1` — sort drained upload Vec
      by (LOD, distance from player).
  - Both zero-added-latency. Stacks on RC31.

- **RC33 cave-gen-multi-scale** (`unendlich-rc33-cave-gen-multi-scale.zip`, 1 file):
  - Step 1 of the cave-generation-and-erosion design memo.
  - Multi-scale density model (worm tunnels + chambers +
    bedrock seepage), opt-in via `UNENDLICH_MULTI_SCALE_CAVES=1`.
  - Threshold tunable via `UNENDLICH_CAVE_THRESHOLD` (default 0.6).
  - Single file (`world/world_model.rs`); INDEPENDENT of
    RC30-32 (different module). Apply order doesn't matter.
  - Byte-identical existing-world behavior when flag is off.

- **RC34 pressure-samples-no-alloc** (`unendlich-rc34-pressure-samples-no-alloc.zip`, 2 files):
  - Finding 1 from the tick_sim bottleneck investigation.
  - Adds `pressure_samples_iter` (non-allocating iterator),
    switches plant_lifecycle's four hot-loop sites. Vec
    version preserved for tests.
  - Two files (`world/pressure_sampler.rs` +
    `world/plant_lifecycle.rs`); independent of all other RCs.
  - ~3 ms/sec saved out of tick_sim's 60-180 ms/sec.

- **RC35 structural-dirty** (`unendlich-rc35-structural-dirty.zip`, 3 files):
  - Finding 3 from the tick_sim bottleneck investigation.
  - Replaces `POI_REFRESH_INTERVAL_TICKS=10` placeholder cadence
    with `chunk.structural_dirty` flag. Set on wood transitions
    in `Chunk::set()`; cleared on InstallPoi cache install.
  - Three files (`chunk.rs` + `plant_lifecycle.rs` +
    `chunk_manager.rs`). Stacks on RC34 (both touch
    `plant_lifecycle.rs`; RC35 is strict superset).
  - ~2 ms/sec saved + architecturally enables several future
    optimizations (uniform-chunk skip, mesh-cache eviction,
    incremental save).

After user applies: expected `533 passed / 0 failed` (525 from
RC30-32 + 3 cave-gen tests + 6 chunk-dirty tests + 2 new
plant_lifecycle tests − 1 removed cadence test).

Seven scheduling/biasing flags + one cave-gen flag + RC34's
zero-flag tick_sim cleanup + RC35's foundational dirty-marking
work. All A/B-testable independently.

## Deferred (long list, ordered by estimated next-session value)

### High value, ready to design

1. **cpu.tick_sim bottleneck** (60-180ms/sec). 100% of cost is the
   `plant_lifecycle` handler. Investigation in
   `docs/speculative/tick-sim-bottleneck.md`. Three findings from
   2026-05-17 read-only code review:
   - **Finding 1 (SHIPPED as RC34)**: `pressure_samples`
     allocates Vec per call → switched plant_lifecycle's four
     hot-loop sites to a new `pressure_samples_iter` variant.
     ~3 ms/sec saved.
   - **Finding 2 (queued)**: Pass 2 plant iteration visits every
     plant in `chunk.plant_indices` even when nothing transitions.
     A "dirty plants" set (plants currently in flight) would drop
     Pass 2 from O(plants-in-chunk) to O(plants-transitioning) —
     ~95% reduction. ~100 LoC across chunk.rs + plant_lifecycle.
   - **Finding 3 (SHIPPED as RC35)**: replaced
     `POI_REFRESH_INTERVAL_TICKS=10` placeholder cadence with
     explicit `chunk.structural_dirty` marking. Idle chunks never
     rebuild. ~2 ms/sec saved + architecturally enables several
     downstream optimizations (compressed-chunk sub-stage 0,
     mesh-cache eviction, incremental save, render-LOD-without-
     remesh).

2. **Upload-scheduler decoupling** (user-flagged: "decouple jarring
   uploads from the immediate rendering pipeline, NOT with a
   frame-time queue because that adds latency"). Design memo in
   `docs/speculative/upload-scheduler-design.md`. Five options
   surveyed; recommended path is Stage 1 (slack scheduler +
   forward-biased shell walk; small env-gated RCs) → Stage 2
   (priority queue) → Stage 3 (off-thread upload with DEVICE_LOCAL
   backing — the real architectural decoupling).

   **Stage 1 Option A** shipped as RC31 v0.24.65-slack-scheduler.
   Canonical readme: `docs/claude-notes/v0.24.65-slack-scheduler-readme.md`.

   **Stage 1 Option D + Stage 2** shipped as RC32
   v0.24.66/67-biased-walk-and-upload-priority. Canonical readme:
   `docs/claude-notes/v0.24.66-67-biased-walk-and-upload-priority-readme.md`.

   **Stage 3** (off-thread upload + DEVICE_LOCAL backing). NOT
   shipped — requires hardware verification due to Vulkan
   synchronization complexity. Detailed implementation guide
   written to `docs/speculative/stage-3-implementation-guide.md`
   with three sub-stages (A: in-frame DEVICE_LOCAL + staging
   ring; B: off-thread memcpy; C: ring sizing). Each sub-stage
   independently testable on hardware. Verification reads this
   session confirmed:
   - Mesh pool currently uses `HOST_VISIBLE | HOST_COHERENT`
     (cache-coherency hypothesis validated).
   - AGX/Honeykrisp is single-queue (Stage 3 wins come from
     memcpy-off-render-thread and DEVICE_LOCAL, not queue
     parallelism).

   Estimated Stage 3 sub-stage A effort: ~250 LoC across
   mesh_buffer_pool.rs and world_renderer.rs. Should be tackled
   in a session with cargo + hardware available.

3. **Compressed chunk representation + physics-aware subsurface
   culling** (user-flagged: "represent some chunks efficiently and
   be able to get them through upload queues in batches and work
   on the harder stuff locally on the GPU via a deterministic
   data expansion"). Design memo in
   `docs/speculative/compressed-chunk-representation.md`. The
   constraint: chunks the player can't see still need to be
   *queryable* for collision; "cull" means "compressed form" not
   "drop." Significant infrastructure already exists (octree
   collapse, `explicit_indices` sparse-edit pattern, GPU
   procedural generator) — what's missing is the routing layer
   that decides per-chunk what upload representation to use.

   Five sub-stages, each independently shippable:
   - **Sub-stage 0**: uniform-chunk skip in mesh pipeline. The
     smallest stepping-stone — ~100-150 LoC (NOT 50 as the memo
     first claimed; the renderer needs an "empty-mesh-at-version-V"
     state to avoid re-submitting uniform chunks each frame).
     Saves the worker pass for sky and bedrock chunks (~300
     chunks × ~1ms = ~300ms of worker time per sky-load on RD=6).
   - **Sub-stage 1**: `UploadRepr` enum + classifier. Data-only,
     no engine changes. Enables 2-4.
   - **Sub-stage 2**: GPU-side procedural expansion. Wires the
     existing `VulkanChunkGenerator` to a "list of (coord, seed)
     → N chunks in GPU buffer" path. **Depends on Stage 3 of the
     upload-scheduler arc** (DEVICE_LOCAL backing).
   - **Sub-stage 3**: GPU-side delta application for procedural
     chunks with few edits.
   - **Sub-stage 4**: uniform-chunk physics fast-path.

   Recommended shipping order: RC33 (sub-stage 0) → RC34 (Stage 3
   sub-stage A) → RC35 (Stage 3 sub-stage B) → RC36 (compressed-
   repr sub-stage 1) → RC37 (sub-stage 2) → RC38 (sub-stage 3)
   → RC39 (sub-stage 4). Each independently testable.

4. **Cave generation + moisture-driven erosion** (user-flagged
   2026-05-17: "could you add more cave generation to the world
   by creating some sort of physics random moisture erosion...
   something where water/moisture can seep through the ground...
   the question of how to simulate weathering on geological
   timescales but quickly compute it"). Design memo in
   `docs/speculative/cave-generation-and-erosion.md`. Current
   cave gen WAS a single 3D-noise threshold (`noise > 0.78`);
   no chambers, no rivers, no causal narrative. The constraint
   is determinism — per-chunk erosion must match neighbors
   exactly with no global pre-pass. Four steps designed:
   - **Step 1** SHIPPED as RC33 v0.24.68-cave-gen-multi-scale.
     Multi-scale density model (worm + chamber + bedrock seepage),
     opt-in via `UNENDLICH_MULTI_SCALE_CAVES=1`, threshold tunable
     via `UNENDLICH_CAVE_THRESHOLD` (default 0.6). Byte-identical
     existing behavior when flag is off. ~140 LoC including
     tests in `world_model.rs`. Canonical readme:
     `docs/claude-notes/v0.24.68-cave-gen-multi-scale-readme.md`.
   - **Step 2** queued: region-scale drainage map via existing
     `RegionGrid` abstraction. Caves bias toward high-flow
     paths. ~300 LoC, introduces `world::drainage`.
   - **Step 3** queued: `geological_moisture` field (parallels
     existing runtime `moisture` but at long timescale).
     Karst-style cave coupling. ~150 LoC. Plays directly into
     the compressed-chunk arc (procedural chunks stay
     reconstitutable from `(coord, seed)`).
   - **Step 4** queued: heightmap erosion (modifies surface
     terrain via drainage). CPU per-chunk-local (Approach C) or
     GPU batched (Approach D — depends on GPU-resident world
     arc). 500-1000 LoC, biggest payoff.

   Architecturally significant: deterministic erosion is a test
   case for procedural reconstitution. If we can do erosion this
   way, we can do tectonics, climate change, river migration,
   geological_temperature, geological_pressure with the same
   pattern. **Strengthens the compressed-chunk arc**:
   procedurally-eroded chunks are still `UploadRepr::Procedural`
   because the erosion is part of the deterministic generation.

5. **Entity scaffold wiring** into ChunkManager. ~200-400 LoC. The
   module is complete (39 tests passing); wiring means: spawn
   loop in chunk_manager, serialization hookup in the save-game
   path, render pass that uses entity.vert/frag, hit-test
   integration with raycast (newly fixed in RC30). No design memo
   yet but the entity module's docstrings are exhaustive.

### Medium value, prepared but compile-risk

3. **RC23 v0.24.57 BerryBush items**. Zip exists
   (`unendlich-rc23-v0_24_57-berry-items.zip`). Prep walkthrough in
   `docs/claude-notes/rc23-berry-bush-prep.md`. Apply with care
   (shader recompile + exhaustive match locations).

### Medium value, no prep

4. **Async upload via separate command buffer**. The renderer's
   `upload_chunk_mesh` currently records into the main draw cmd
   buffer; a separate transfer cmd buffer would let the upload
   run async with the draw. Lots of plumbing.

5. **AO at Mid LOD**. Currently AO is Near-only. Mid is greedy-mesh
   without vertex AO. Adding AO would be a per-face contribution
   pass in the Mid mesher. Estimated: ~150 LoC + shader changes.

6. **Sky-aware face lighting**. Faces facing the sun are slightly
   brighter than shaded ones. Existing `sky` snapshot in MeshRequest
   carries the data, but the mesher's `block_color` doesn't read
   it. Per-mesh-time bake; would require re-mesh on sun position
   changes (mostly day/night boundary).

### Low priority / speculative

7. **Per-block weathering on GPU**. Move weathering blend factor
   into shader constants. Would let chunks weather visibly without
   re-mesh. Lots of design work; not worth pursuing until 4-6 done.

8. **Plant blueprints on GPU**. Similar to 7 but for plants —
   move plant state into a small per-cell struct that the shader
   samples. Massive complexity gain; only worth it if
   `cpu.tick_sim` doesn't yield enough to the per-chunk-event cache
   approach.

9. **Far/VeryFar GPU coverage**. Currently the octree GPU mesher
   handles Near + Mid (the latter behind `UNENDLICH_GPU_MESHER_MID=1`).
   Far/VeryFar still CPU-side via `mesh_chunk_far_virtual`. The
   virtual path is fast (reverse-regen avoids storing the chunk),
   but pure-GPU would be faster. Not a current bottleneck.

10. **Items / mobs full wiring**. RC23 only does berries; a real
    items system needs inventory + pickup + drop. Mobs need AI.
    Both gated on entity scaffold wiring (item 2 above) first.

11. **Spatial query indexing for entities**. Once mobs exist,
    "find all mobs in this AABB" wants a spatial index. The chunk
    boundary is the natural fit: per-chunk entity list. Implies
    per-chunk serialization hookups too. Tied to item 2.

12. **Entity serialization for save-games**. Save the entity table
    when chunks save. Schema decisions: per-entity blob vs columnar
    + reconstructed-on-load. Tied to item 2.

13. **Multiplayer entity authority**. Server-authoritative; clients
    interpolate. Wholly speculative; gate on items 2 + 10 + 11.

## Pre-existing dead-code warnings (intentional scaffolding)

These were intentionally left alone in the cleanup pass:

- `create_compute_pipeline` (gpu_meshing_sparse.rs:986) — alternate
  pipeline path, kept for benchmarking.
- `WORKGROUPS_PER_CHUNK_X` (gpu_meshing_octree.rs:65) — used in
  comments / future tuning.
- `octree_pool_buffer` (gpu_meshing_octree.rs:284) — field reserved
  for v0.24.31-versioned-pool which is in flight.
- `sample_or_air` (gpu_meshing_octree_cpu_ref.rs:134) — reference
  implementation against which the GPU shader is validated; kept
  alive for diffing.

## Recent transcript navigation

This session's full conversation is in
`/mnt/transcripts/2026-05-18-00-39-29-unendlich-rc29-rc30-stack.txt`
(see journal for prior transcripts).

The user's preferred mode for autonomous work:
- "Keep going" = continue until I'm ready to test
- Little things → fully checked off
- Big tasks → partially completed or split into parallel-task
  buckets (this doc)
- Trust to push forward without ping-ponging when "we're in a good
  spot"
- Iterate in the sandbox, not via per-command back-and-forth
- Env-gated opt-in for new perf features (RC29 and RC30 pattern)
