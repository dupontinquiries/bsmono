# SURFACERUN KIT — far-render rollout (coordinator → your machine)

Goal: stop the octree pool from being the wall. The pure core (`world/upload_repr`:
`classify` / `project_residency` / `srun_bytes`) is built and tested; the GPU mesh
paths exist (`chunk_mesh_octree.comp`, `chunk_mesh_runs.comp`, `chunk_generation.comp`).
What's left is **integration-tier wiring** (Vulkan + cargo + the 1800-test suite) —
yours, per R7. This kit sequences it lowest-risk first, with the de-risk already done.

## The de-risk (done here, faithful to the real `classify`)

Projection over a shell-area chunk model calibrated to your run log
(164 KB/octree-chunk, LOD bands 12/24/48, observed worst_col_runs≈14):

```
radius  chunks   all-octree(today)   routed: octreeMB / proc%
r≈30    2827     453 MB  (OVERFLOWS)  77 MB   / 35%   <- your RD-10 run
r=48    7238     1159 MB              77 MB   / 73%
r=64    12868    2061 MB              77 MB   / 83%
r=96    28953    4637 MB              77 MB   / 91%
```

Octree pool stays ~77 MB at **every** radius (only the Near band is octree). Re-run /
re-tune the assumptions with `handoff/surfacerun/surfacerun_telemetry.rs`
(`rustc -O … && ./a.out`). **But get the REAL numbers from Step 1 before wiring GPU.**

## Step 0 — immediate relief (no code, do first)
Raise `UNENDLICH_OCTREE_POOL_MB` 256 → 1024+ (unified memory; mesh buffers already sit
at 1 GB/3 GB with headroom). This alone should kill most of the OOM-fallback spike tail
at your current RD. `world/device_tier` (round 2, now merged) is the systematic version —
wire it to set the pool/RD/LOD budget per machine (M2 up, Fairphone down).

## Step 1 — live residency telemetry (low-risk; confirms the projection on real data)
Call the existing pure API on the actual loaded set each FPS-log window — **no GPU
change**, just measurement. Sketch (CPU only; place near the existing memory-log line):
```rust
use unendlich_engine::world::upload_repr::{ChunkStats, UploadPolicy, project_residency};
let policy = UploadPolicy::default();
let stats = loaded_chunks.iter().map(|c| {
    let srun = c.surface_run();                 // engine already computes this for compression
    ChunkStats::from_surface_run(&srun, c.lod(), c.is_edited())
});
let proj = project_residency(stats, &policy, octree_per_chunk_kb as u64 * 1024);
log::info!("residency_projection: octree={}MB ({} ch) srun={}KB ({}) proc={} | all-octree baseline={}MB",
    proj.octree_bytes >> 20, proj.octree_chunks,
    proj.surface_run_bytes >> 10, proj.surface_run_chunks, proj.procedural_chunks,
    proj.all_octree_baseline_bytes(octree_per_chunk_kb as u64 * 1024) >> 20);
```
Run at RD 16/32/64 (or higher) and compare `octree=…MB` to your pool. Expect ~the table
above. This is the go/no-go gate for Steps 2–3.

## Step 2 — wire the SurfaceRun (Mid band) upload path
In the upload path where you currently always upload the octree, branch on `classify`:
```rust
match upload_repr::classify(&stats, &policy) {
    UploadRepr::Octree     => upload_octree(&chunk),                 // existing path
    UploadRepr::SurfaceRun => upload_runs(&chunk.surface_run()),     // NEW: runs buffer → chunk_mesh_runs.comp
    UploadRepr::Procedural => { /* Step 3 */ }
}
```
- `chunk_mesh_runs.comp` already exists — point a descriptor at the runs buffer and
  dispatch it for SurfaceRun chunks (mirror the octree mesher's dispatch).
- Keep the octree path untouched for the Near band. A/B with `procedural_from` pushed
  out so only SurfaceRun (no Procedural) is exercised first.
- R2/R16: if you touch any `.comp`, regenerate its `.spv` (`handoff/force_recompile_shader.sh`).

## Step 3 — wire the Procedural path (Far/VFar untouched chunks)
For `UploadRepr::Procedural`: upload nothing; let the GPU regenerate from seed on demand
(`chunk_generation.comp`). This is the bulk of the memory win (35–91% of chunks above).
Gate behind a flag; verify untouched-far chunks render identically to their octree form
(byte-identical regeneration is the precondition `classify` relies on via `!edited`).

## Sequencing & safety
0 pool bump (now) → 1 telemetry (confirm) → 2 SurfaceRun/Mid (A/B) → 3 Procedural/Far (A/B,
flagged) → re-baseline `perf_check.py` from a clean run (the committed baseline predates the
fog march). Each step is independently revertible. `device_tier` feeds Step 0's budget.

## Verified here vs. yours
- Verified here (sandbox): the policy (`classify`/`project_residency`) + the projection above.
- Yours (integration): the upload-path branch, the runs-buffer descriptor + `chunk_mesh_runs.comp`
  dispatch, the Procedural skip, and the in-engine telemetry numbers. None of this compiles
  without Vulkan, so it's the integration pass — but it's now de-risked and sequenced.
