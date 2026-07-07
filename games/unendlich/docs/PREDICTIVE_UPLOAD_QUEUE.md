# Predictive Upload Queue

Hide the on-demand mesh+upload stall (the ~39ms "shadow" spike) by meshing
and uploading chunks *before* the camera arrives, and by spreading prefetch
uploads under a per-frame budget so a burst never stalls one frame's head.

## The bug

The GPU timer's `(FrameStart, ShadowEnd)` zone is labeled "shadow" but is
the **entire frame head**. The spike is not the shadow pass:

- Persists identically at 2048², 512², stride 1, stride 2 → not shadow raster.
- Working set ~7 MB (`mesh_vert_used_mb=4`, `idx=3`, `octree_bytes=117 KB`)
  → 7 MB cannot be a 39ms event → not upload bandwidth.
- `shadow max=39.21ms` co-occurs with `mesher near max=35.41ms` (p50=0.00),
  `near=CPU` → a near chunk meshed **on demand** that frame; the frame head
  stalled waiting on it (contention on the shared octree pool;
  `GPU_POOL_EARLY_UNLOCK=1`).

A latency/contention stall, not throughput. Fix: never mesh/upload a needed
chunk on its arrival frame.

## What already exists (do NOT rebuild)

Most of this is in the tree as pure cores from prior worker rounds — they
are correct and tested but **dormant (not wired into the live loader)**:

- **Prediction**: `world::prefetch_predict::prefetch(player, vel, lookahead, n)`
  — priority-ordered chunk coords along the path of travel (faster ⇒ farther;
  stationary ⇒ surrounding ring). Worker-03, R22.
- **Scheduling**: `world::chunk_schedule::schedule(cands, budget)`,
  `world::gen_estimate`, `world::batch_gen_estimate`, `world::octree_scheduler`
  — budget-aware selection of what to generate/mesh per tick.
- **Compression on upload + GPU decode**: `world::surface_run::SurfaceRunChunk`
  (the ~5× codec; runtime `ratio=5.44`) + `world::upload_repr::classify(...)`
  routing each chunk to a GPU path: `chunk_mesh_octree.comp` (full),
  `chunk_mesh_runs.comp` (~5 KB resident), or `chunk_generation.comp`
  (0 bytes — regenerated from seed). Active under `UNENDLICH_GPU_MESHER=2`.

So "predictive" and "compressed GPU upload" are already implemented. The
missing pieces are (a) a safe deferrable upload lane, and (b) wiring the
dormant cores into the live tick.

## New code — deferrable, budgeted upload lane (`render/mesh_buffer_pool.rs`)

The draw loop binds **every resident mesh each frame**, so a visible chunk's
copy must flush the same frame — deferring it would draw stale device memory.
Only **prefetched, not-yet-visible** chunks may defer. Hence a *tier*:

- `write_at` — unchanged; visible writes, flushed in full (byte-for-byte
  identical to before).
- `write_at_deferrable` — staging memcpy immediately; GPU copy goes in the
  deferrable lane.
- `flush_pending_copies` — drains all visible copies, then folds in deferrable
  copies up to `UNENDLICH_MESH_DEFER_BUDGET_BYTES` (default 1 MiB/frame; `0`
  = flush all immediately), carrying the remainder. Post-copy barrier scoped
  to exactly what flushed this frame.

Safe because staging offsets are identity-mapped to device offsets and stable
across frames, so a carried copy's staging bytes stay valid.

## Wiring — LANDED (opt-in via `UNENDLICH_PREFETCH=1`)

Motion-predictive load-ahead is now wired into `world::chunk_manager::update`
(the live shell walk), finally activating the dormant `prefetch_predict`:

- `ChunkManager` finite-differences the player world position each `update`
  to get a velocity (converted blocks → chunks).
- When moving, it calls `world::prefetch_predict::prefetch(player_chunk, vel,
  lookahead, n)` and requests those ahead-of-path chunks via the existing
  `loader.request_many`, skipping already-resident chunks (`self.chunks
  .contains_key`) and ranking them by the same distance² priority as
  `request_visible` — so visible chunks always win contention (no starvation).

Flags (all default-safe):
- `UNENDLICH_PREFETCH=1` — enable (default OFF; behavior unchanged when unset).
- `UNENDLICH_PREFETCH_COUNT` — chunks requested ahead per update (default 16).
- `UNENDLICH_PREFETCH_LOOKAHEAD` — reach scalar; reach(chunks) = lookahead ×
  speed(chunks/frame) (default 16.0).
- `UNENDLICH_MESH_DEFER_BUDGET_BYTES` — the deferrable upload lane budget
  (default 1 MiB/frame), for the upload-burst side.

### What this fixes, and the honest limit

This gets chunk **generation** started ahead of arrival. Whether it also moves
the expensive near-**mesh** ahead depends on the render-side meshing trigger:
if meshing fires when a chunk is *installed/loaded*, prefetch hides it; if it
fires on *first visibility*, the near-mesh still lands on the arrival frame and
the render-side trigger needs the same predictive treatment. That trigger lives
behind `feature = "render"` (can't compile in the sandbox), so **confirm on the
M2**: run with `UNENDLICH_PREFETCH=1`, fly fast through ungenerated terrain, and
watch whether `near max` / `shadow max` / `total max` in the timer summary drop.

If the spike persists with prefetch on, the meshing is visibility-triggered, and
the next step is to drive the render-side mesh request off the same
`prefetch_predict` output (a render-feature change, M2-side).

## Validation status

- **Deferrable lane** (`mesh_buffer_pool.rs`): this file is behind
  `#[cfg(feature = "render")]`, which does NOT compile under `--features gpu`
  and which needs glfw/cmake to build at all — so it is **NOT compile-validated
  in the sandbox**. The pure budget split (`count_within_budget`) is unit-tested
  (6/6, standalone); the `write_at_deferrable` / flush vk code is verified by
  reading only. Compile it on the M2 (`--features render,...`), or after the
  `headless` feature (T06) lands, which is the prerequisite for validating any
  render-side code here. On by default; tunable via
  `UNENDLICH_MESH_DEFER_BUDGET_BYTES`. Inert until a caller uses
  `write_at_deferrable`.
- **Prediction / scheduling / compression cores**: pre-existing, tested.
- **Runtime confirmation the 39ms spike is gone must happen on the M2** once
  the wiring lands — the sandbox has no GPU. Check: fly fast through
  ungenerated terrain, watch `shadow`/`total` MAX in the GPU-timer summary.

## Note on prior duplication

An earlier pass added a second crate-root `prefetch_predict.rs`; it duplicated
the dormant `world::prefetch_predict` and was removed. Prediction logic belongs
in `world/` (Worker-03 ownership). If the existing coarse path-sampling needs
finer time-to-need neighborhood scoring, that is an enhancement to
`world::prefetch_predict`, not a parallel module.
