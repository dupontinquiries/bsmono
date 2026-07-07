# Performance levers — what to dial when

Distilled from the 2026-05-28/29 hardware-test sessions on M2 Ultra / 192 GB
unified memory. The architecture is in a state where **memory pressure is no
longer the binding constraint**, and the remaining frame-time cost is GPU
rasterization in the shadow + main passes. This page lays out the levers in
the order you reach for them and what they're known to cost.

## Memory ceiling — already solved if you set this

| env | default | what it does |
|---|---|---|
| `UNENDLICH_OCTREE_POOL_MB` | 256 | GPU octree pool capacity. 256 MiB is a dGPU baseline. On Apple Silicon / 24 GB+ dGPUs raise to 2048+; the user's 20480 (clamped to 16384) ran fine. |

After this, `gpu_oom_fallback` should be ≈ 0 and `octree pool` utilization in
the `gpu_mem:` line should sit well below 100%.

## Frame-time levers (GPU-bound, post-memory-fix)

In observed-cost order. Numbers below are p50s from a real RD=12 run.

### Shadow pass (~22 ms at RD=12, default settings)

The shadow pass costs roughly the same as the main pass because it draws the
same chunk set (camera-frustum-culled, then re-culled to the light frustum).
Cheapest knobs:

| env | default | effect |
|---|---|---|
| `UNENDLICH_SHADOW_CHUNK_STRIDE` | `1` | Set to `2` or `3` to temporally amortise: only `1/N` of the chunks render to the shadow map per frame, the rest keep last-frame depth. Cuts shadow cost roughly linearly (`stride=2` ≈ half), at the cost of a 1–2-frame lag on shadow updates for fast-moving sun or fast camera translation. Cascade shifts force a full re-render anyway, so this is mostly invisible in steady state. |
| `UNENDLICH_SHADOW_MAP_SIZE` | `2048` | Drop to `1024` for a 4× cut in fragment work. Shadows get softer / less sharp at boundaries; for a voxel game this is usually fine. |

### Main pass (~22 ms at RD=12)

| env | default | effect |
|---|---|---|
| `UNENDLICH_RENDER_DISTANCE` | `6` | The biggest single lever. Each +1 in render distance roughly triples the visible chunk count. |
| `UNENDLICH_LOD_BANDS` | `8,16,32` | Pull bands inward (`3,6,9`) to push more chunks into coarse LODs. Mesher cost drops; main-pass tri count drops; visual fidelity at distance drops. |

### Mesher (CPU side; only on near LOD when GPU_MESHER=2)

| env | default | effect |
|---|---|---|
| `UNENDLICH_GPU_MESHER` | `0` | `2` routes mid/far/vfar through the GPU mesher. Near stays CPU. |
| `UNENDLICH_GPU_DECORATE` | `0` | GPU-side decoration (trees, plants). With `GPU_MESHER=2`, this is the path that keeps near-LOD mesh cost off the CPU thread. |

## Diagnostic levers (no perf impact, just visibility)

| env | default | effect |
|---|---|---|
| `UNENDLICH_STRICT_SHADERS` | `0` | Set `1` to fail at startup if any committed `.spv` doesn't match its source. Use after a shader-touching brick to validate hot-shaders rebuilt cleanly. |
| `UNENDLICH_OCTREE_VERIFY` | `0` | Run the GPU↔CPU octree parity probe on shutdown. Catches silent drift. |

## Recommended preset combos

### "Performance" — for cruising the world at high RD
```
UNENDLICH_RENDER_DISTANCE=16
UNENDLICH_LOD_BANDS=3,6,9
UNENDLICH_SHADOW_CHUNK_STRIDE=2
UNENDLICH_SHADOW_MAP_SIZE=1024
UNENDLICH_OCTREE_POOL_MB=2048
UNENDLICH_GPU_MESHER=2
UNENDLICH_GPU_DECORATE=1
UNENDLICH_DEVICE_LOCAL_MESH=1
```

### "Quality" — closer-in, high fidelity
```
UNENDLICH_RENDER_DISTANCE=8
UNENDLICH_LOD_BANDS=8,16,32
UNENDLICH_SHADOW_CHUNK_STRIDE=1
UNENDLICH_SHADOW_MAP_SIZE=4096
UNENDLICH_OCTREE_POOL_MB=2048
UNENDLICH_GPU_MESHER=2
UNENDLICH_GPU_DECORATE=1
```

### "Headroom" — workstation hardware showing what it can do
```
UNENDLICH_RENDER_DISTANCE=20
UNENDLICH_LOD_BANDS=3,6,9
UNENDLICH_SHADOW_CHUNK_STRIDE=3
UNENDLICH_SHADOW_MAP_SIZE=2048
UNENDLICH_OCTREE_POOL_MB=8192
UNENDLICH_GPU_MESHER=2
UNENDLICH_GPU_DECORATE=1
UNENDLICH_DEVICE_LOCAL_MESH=1
UNENDLICH_BIASED_SHELL_WALK=1
UNENDLICH_UPLOAD_PRIORITY=1
UNENDLICH_ADAPTIVE_UPLOADS=1
```

## Levers that are NOT meaningful right now

These show up in `Flags in effect` but won't move the needle in the
post-memory-fix world:

- `UNENDLICH_MAX_DRAWS` — the 4096 default isn't being hit. Worth raising if
  `indirect_slots_cap` saturates (see the dashboard Memory panel).
- `UNENDLICH_CAVE_THRESHOLD_FBM` — purely a worldgen aesthetic tuning, no
  perf impact.
- `UNENDLICH_MULTI_SCALE_CAVES` — opt-in CPU multi-scale cave path that
  isn't mirrored to the GPU shader; leaving it off keeps CPU↔GPU parity.

## Metric interpretation notes

A `rendering: p50=1446` number in workload-shape is currently the count of
chunks the renderer is *tracking* (meshes ∪ empty_mesh_records ∪ octree
pool slots), not the count of chunks *drawn per frame*. The actual per-frame
draw count is bounded by `self.meshes.len()`, which matches the sum of the
`lod_near/mid/far/vfar` line. A future brick will split these into distinct
`tracked:` and `drawn:` metrics.

The `ratio_octree_over_srun` in `gpu_mem:` shows how much GPU memory would
be saved if the octree representation were replaced by SurfaceRun encoding
GPU-side. Empirically ~30× across multiple worlds; cashing in that win is
the next architectural lift (see FOLLOWUPS.md).

## Run profiles (codified flag bundles) — R27
The flag soup got unwieldy, so blessed bundles live in code: `runtime_flags::profile(name)`.
Set `UNENDLICH_PROFILE=<name>` and the bin expands the bundle EARLY (before any env read),
**set-if-unset** — so any explicit env var you also pass overrides the profile value.

**`rd24`** — the settled RD=24 quality cluster (prefetch on; GPU octree mesher near+mid;
device-local meshes; the upload/scheduler toggles; cheap-stride shadows; tight near LOD band;
gen+mesh+upload batches opened up). Baked so you don't retype ~25 flags. NOT baked (vary per
run): eye-candy (POSTFX/BLOOM/SEASONS/WEATHER/GPU_DECORATE), OCTREE_POOL_MB, SPAWN_ARCHE.

The whole tuned run collapses to:
```
UNENDLICH_PROFILE=rd24 UNENDLICH_OCTREE_POOL_MB=4096 UNENDLICH_SPAWN_ARCHE=dragon \
  cargo run --release --bin unendlich-world --features render,hot-shaders,window,gpu,ui-overlay
```
Override anything by setting it explicitly, e.g. add `UNENDLICH_SHADOW_CHUNK_STRIDE=1` to get
full-detail shadows for one run. Add a profile by extending `profile()` in runtime_flags.rs.

### Pruning follow-up (not done yet — needs harness validation)
Most of the `rd24` toggles are "always 1" in practice (PREFETCH, REMESH_GATE, SKY_CULL,
ADAPTIVE_UPLOADS, SUBMIT_SKIP, SLACK_SCHEDULER, BIASED_SHELL_WALK, UPLOAD_PRIORITY,
SHADOW_CACHE, DEVICE_LOCAL_MESH, GPU_MESHER=2). Each could have its DEFAULT flipped to the
proven value and become a kill-switch (set =0 to disable) instead of an opt-in — which would
shrink even the profile. Doing that changes behavior for every binary + the harness, so promote
them one at a time with `harness/run.sh` green between each, rather than in a batch.
