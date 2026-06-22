# Follow-ups (from 2026-05-28 hardware test)

Captured while diagnosing the user's session log + screenshot so the next pass
can land on something specific instead of broad-guessing.

## 1. Broken meshes in `UNENDLICH_GPU_MESHER=2` runs — hypothesis: empty-chunk OOM fallback

Run summary signals:
- `gpu_oom_fallback=162` out of 613 chunks meshed (26% fell back).
- `workload-shape: loaded p50=468, rendering p50=234` — roughly half of loaded
  chunks aren't producing draws. Consistent with chunks meshing to empty
  geometry (not the obvious "hole" the user was looking for, but also not
  rendering).

Hypothesis: under `UNENDLICH_GPU_DECORATE=1` chunks are generated entirely
GPU-side via `chunk_generation.comp`. The CPU-side `Chunk` carried in
`OctreeGpuMeshRequest.chunk` (cloned from `req.chunk` at
`meshing_pool.rs:611`) may not contain the GPU-generated voxels — it'd be
whatever was in `req.chunk` at request time, which for GPU-only generation
could be an empty/default chunk. When the GPU mesher then hits OOM and falls
back to `mesh_chunk_greedy(&r.chunk, …)` at `meshing_pool.rs:1106`, the
fallback sees an empty chunk → emits an empty mesh → the chunk appears as a
hole or geometric breakage at LOD boundaries (the neighbour at a different LOD
*does* have geometry, so the seam reads as broken).

Diagnostic next pass:
- Add a debug log inside the fallback: `r.chunk.is_all_air()` count vs
  `r.chunk` has-content count. If most fallbacks are on all-air CPU chunks
  while GPU-side they had content, the hypothesis is confirmed.
- Fix options: (a) when GPU mesher OOMs, force CPU-regen of the chunk before
  fallback meshing; (b) keep a GPU→CPU writeback of voxels at chunk creation
  when GPU_DECORATE is on; (c) reserve enough device memory headroom that OOM
  doesn't fire (the root cause).

User report needed to confirm: a screenshot of a "broken mesh" chunk + the
coords from the log, so we know whether the failure mode is hole / wrong
geometry / LOD seam crack.

## 2. Entities aren't rendered

`SimState::mobs: Vec<Mob>` exists and `SimDriver::render_pose(i)` returns an
interpolated pose per mob, but there is no Vulkan pipeline that consumes those
poses and emits draw calls. Mobs are simulated but invisible. Wiring a minimal
mob renderer (instanced cube with per-mob model matrix; share the existing
chunk vertex format or add a tiny dedicated pipeline) is its own brick — not a
bug in any of the recent worldgen / UI work.

## 3. Hard biome transitions

Addressed in `brick/biome-boundary-jitter` (this session): high-freq noise on
the climate samples wiggles the threshold lines. Visual confirmation pending.
If the wiggle isn't enough at the cherry/snow boundary specifically, the next
lever is a wider transition band — sample TWO biome decisions in the boundary
region and blend their surface block stochastically.

## 4. Inventory / panel-mode interaction

`brick/input-p-toggle` wired the `p` key, cursor mode, and pointer events. The
remaining work is the panel content itself — a real selectable inventory grid
that responds to clicks on `slot_swatch`. The pointer events are now feeding
egui (PointerMoved + PointerButton), so the egui Response::clicked() should
fire on the picker; the bin just needs to handle the resulting selection.

## 2026-06-04 — `fires: 269000` resolved to heat-source design, not a leak
`fires`=heat_sources.len(). Deduped + eviction-correct ⇒ NOT a leak; load-proportional. Root cause:
`register_lava_heat_sources` emits one source per surface-lava block × O(n) `total_heat_at` linear scan
(called in convection/plant/ice). Fix A shipped (fix_lava_heat_clustering.patch — cluster to ≤8/chunk,
~30× cut). Fix B (recommended): spatial-index HeatSourceMap so queries are O(nearby). Dragon/seasons/
weather do NOT touch heat_sources — the increase was terrain-load. See handoff/HEAT_SOURCES_DIAGNOSIS.md.

## 2026-06-04 — `remesh_gate skipped=0` is EXPECTED, not a bug (correction)
The gate (chunk_manager.rs:1189) only suppresses remeshes when a chunk's ONLY tick-edits are
micro-detail growth (grass/flowers) at a LOD too coarse to draw them, AND there are no
state/transition/tree-root ops. skipped=0 ⇒ no such far micro-detail edits occurred this run (player
was flying, not editing distant foliage). It is NOT aimed at — and cannot reduce — the near-mesher
churn, which is LOAD- and LOD-TRANSITION-driven, not edit-driven. Right levers for that:
  • mesh_tier / GPU near tier → kills the per-mesh CPU cost (near p99=242ms, max=519ms).
  • UNENDLICH_TRANSITION_REMESH_STRIDE (default 1) → raise to thin transition-remesh COUNT.
  • chunk_priority (r11, merged) → bounds the request_visible CPU cost once wired.
Earlier "gate is a no-op despite churn" framing in MESH_TIER_PLAN was conflating edit-remeshes with
load/transition remeshes — struck.

## 2026-06-04 — SurfaceRun memory numbers validate the far-LOD direction (from R17 perf log)
First log with gpu_mem/compression detail: octree_per_chunk_kb=173 vs srun_per_chunk_kb=5 →
ratio_octree_over_srun=33.4×; storage compression ratio=6.15 (worst_col_runs=14). SurfaceRun is in the
engine and 33× more compact per chunk. This quantifies the main/shadow-wall fix: render far (and likely
mid) chunks via SurfaceRun instead of octree-meshed geometry → far less geometry to raster + shadow, far
less GPU memory. cull p50 was ~0 in this run (rendering ~1.9-2.8k) — confirms cull cost ∝ visible count
(the workgroup-aggregation fix targets dense scenes). Next concrete step: SurfaceRun far-LOD integration
sketch against world_renderer (the upload_repr classifier → route by LOD band). See handoff/SURFACERUN_KIT.md.

## 2026-06-04 — 901s RD=24 log: octree pool 96% full + CPU-bound + fires=1M
octree_used_mb=1878/2048 (11929 chunks, 161KB ea; srun 5KB, 31.9×) — near OOM ⇒ SurfaceRun far-LOD is
MEMORY-urgent. Frame CPU-bound (p50≈79ms / GPU total ≈24ms): draw-record + submit + tick_sim scale with
~12k rendering chunks. fires=1,006,633 (apply heat Fix A patch + Fix B heat_source.rs). Plan:
handoff/GPU_COMPUTE_PLAN.md — GPU field-kernel abstraction (weather/genetics/pollination), multi-level
culling, chunk scheduling + fetch/gen predictors. Pure cores queued R22.
