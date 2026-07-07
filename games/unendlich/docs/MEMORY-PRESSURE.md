# Device-memory pressure: the real cause of the stripe, holes, and "missing textures"

## The log that cracked it

A run produced, in a **contiguous band** of chunks (x −5..−15, z −13..−21, y 2–3):

```
overflowed GPU mesher (ERROR_OUT_OF_DEVICE_MEMORY); CPU-fallback meshing to avoid a hole
Indirect draw slot pool exhausted (in_use=4096/4096)
```

Two distinct failures, one cause: **too many resident chunks for the device.**

## The causal chain (confirmed from source, not guessed)

1. The scene has more drawable chunks than fit. At the default render distance 6
   there are ~925 drawn chunks (per the `chunk_indirect.rs` comment); the slot
   pool `MAX_CHUNK_DRAWS = 4096` was sized to exceed that. Hitting **4096/4096**
   means the run was at a **much higher render distance** (RD ≥ ~16 — see the
   estimator).
2. **Slot exhaustion** (`chunk_indirect.rs`): a chunk that can't get a draw slot
   is not drawn → an actual **HOLE**.
3. **Mesher OOM** (`meshing_pool.rs` ~1074): when the GPU mesher hits
   `ERROR_OUT_OF_DEVICE_MEMORY`, the chunk falls back to the CPU greedy mesher,
   called as `mesh_chunk_greedy(&chunk, None, None, tick)`. The `None, None` are
   `sky` and `world_model` — so the fallback chunk is meshed **without the GPU
   path's climate tint and with neutral lighting** (the code comment says exactly
   this). A band of OOM chunks therefore renders as flat, untinted terrain next to
   tinted GPU neighbours → the visible **STRIPE**, and the "missing texture" look.

## What this is NOT

It is **not** a worldgen-divergence bug. `tools/density_parity_lint.py` proves the
CPU and GPU generators produce byte-identical density and value-noise — the voxels
are the same. The divergence is purely in **meshing/shading** (GPU mesher vs CPU
fallback), triggered by memory pressure. The earlier "stripes closed" note in
`STRIPES-STATUS.md` was over-claimed and is corrected there.

## What the estimator shows (`tools/gpu_memory_estimate.py`)

Grounded in the real constants (CHUNK_SIZE 32, MAX_CHUNK_DRAWS 4096,
PackedVertex 8 B, ~5.5 vertical layers from the 925-@-RD6 anchor):

| RD | drawn chunks | slots (pool 4096) | mesh mem | verdict |
|---:|---:|---|---:|---|
| 6  | 925   | 925/4096   | 49 MiB  | ok |
| 12 | 3421  | 3421/4096  | 183 MiB | ok |
| 13 | ~4000 | ~4000/4096 | ~210 MiB| ok (the ceiling) |
| 16 | 5961  | **+1865 over** | 318 MiB | OOM/HOLES |
| 20 | 9201  | **+5105 over** | 491 MiB | OOM/HOLES |

Key insight: the **slot pool is the binding constraint long before mesh memory is.**
At RD 16 the persistent mesh buffers are only ~318 MiB, yet draws overflow 4096 by
1865. So the `OUT_OF_DEVICE_MEMORY` is coming from the mesher's **transient scratch
/ slot-indexed buffers**, not fat vertex data. Vertices are already packed to 8 B,
so "compress the vertices" yields less than expected — the lever is **chunk count**.

## Fixes, cheapest first

1. **Lower render distance — zero code, immediate.** `UNENDLICH_RENDER_DISTANCE=13`
   (or less) keeps drawn chunks under 4096. If the stripe and holes vanish, the
   diagnosis is confirmed. This is the recommended first test.
2. **Raise `MAX_CHUNK_DRAWS`** — only addresses the HOLES (slot exhaustion), not
   the mesher OOM, and costs more indirect-buffer memory. Do this only after a
   memory profiler shows headroom.
3. **Tighter LOD / eviction** — the proper fix for large render distances: coarse-
   mesh or drop far chunks so `drawn` grows sub-quadratically with RD. The engine
   already has mid-LOD (v0.24.39) and chunk eviction (v0.24.NEXT). **The LOD bands
   are already runtime-tunable** via `UNENDLICH_LOD_BANDS=near,mid,far` (defaults
   `8,16,32`, in chunks). Pulling them inward — e.g. `UNENDLICH_LOD_BANDS=4,8,12` —
   demotes far chunks to coarser LODs, cutting their mesh memory and easing the
   mesher OOM that drives the stripe. The run-summary `memory:` line now reports
   `lod_near/mid/far/vfar` chunk counts (shown in the dashboard Memory panel), so
   you can watch `lod_near` (the expensive full-detail chunks) fall as you pull the
   bands in, and confirm `gpu_oom_fallback` drops with it. Knob, gauge, verify.
4. **Thread climate tint into the CPU fallback** — a *symptom* fix that kills the
   STRIPE even when OOM happens, by passing `world_model`/`sky` to
   `mesh_chunk_greedy` so fallback chunks tint identically. Deferred: the dispatcher
   thread doesn't currently hold `world_model`, and the CPU tint must match the GPU
   shader's tint exactly or it trades one stripe for another. Needs the build loop.
5. **Further compression** — vertices are 8 B already; the bigger target is the
   **index buffer** (~1.5 indices/vertex × 4 B). A 16-bit index path for chunks
   under 64 k vertices roughly halves index memory. Architectural; later.

## Telemetry the user asked for (the plan)

**Memory profiler in the dashboard.** Today the slot count (`in_use=N/4096`) and
the OOM events only hit the stderr log, not `run-summaries.txt`. Plan:
- Engine: each run, sample `VkPhysicalDeviceMemoryProperties` budget (or the
  `VK_EXT_memory_budget` heap usage) + the slot-pool high-water mark + a count of
  OOM-fallback chunks, and emit them as metrics into the run summary.
- Dashboard: a memory panel — heap used vs budget over the run, slot-pool
  high-water vs 4096, and an OOM-fallback counter (which, if > 0, flags the stripe).
This turns "why is it striping" into a glanceable gauge.

**Compress runs.** `run-summaries.txt` grows unbounded with plaintext. Plan: write
each run as a compressed record (the dashboard already exists to decompress + render
it, so plaintext is no longer needed for human reading). Pair with the
**sub-2-second-run deletion** the user wants — drop runs shorter than 2 s so transient
startup/crash runs don't spam new run-ids. Both live in the engine's run-summary
writer + the dashboard's `parse.rs` (add a gzip path).
