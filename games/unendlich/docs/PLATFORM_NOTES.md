# PLATFORM_NOTES.md — hardware and driver findings

A living document of what we've learned about the target platforms
through actual testing. Each finding cites the run + data that
established it. New findings get appended; old findings are not
edited (so we can see the history of what we believed when).

The goal is twofold:
1. Avoid relearning the same lesson twice when a future investigation
   touches the same platform-level concern.
2. Have honest evidence ready when filing upstream bug reports or
   when a new contributor asks "why don't we just do X?"

---

## Primary test platform

- **Hardware**: Apple M2 Max
- **OS**: Asahi Linux (Fedora Asahi Remix)
- **Vulkan driver**: Honeykrisp (Mesa 25.3.6, May 2026)
- **Compositor**: KWin (Wayland)
- **Display**: 120 Hz
- **GPU architecture**: AGX (Apple's TBDR, derived from PowerVR lineage)

This is the platform every chug-investigation finding below was
gathered on. Findings may or may not generalize to other GPUs;
each section notes when behavior is platform-specific.

---

## Finding 1: Single queue family, no multi-queue parallelism

**Established**: v0.24.3-multi-queue-a (increment 44), 2026-05-14.

**Claim**: AGX exposes only one Vulkan queue family on Honeykrisp. We
cannot use multi-queue submission to parallelize compute (e.g. async
meshing) and graphics work.

**Evidence**:
```
compute qf=0 dedicated=false
compute_queue_is_distinct: false
```
The compute queue handle is identical to the graphics queue handle
after `vkGetDeviceQueue`. At the hardware level AGX has a unified
command queue (matching Apple Metal's design); the driver doesn't
fake multi-queue.

**Architectural implication**: Plans that assumed async work via
separate queues (mesher on compute queue, render on graphics queue)
are infeasible. Submissions interleave on the single queue family
in submission order.

**What we tried instead**: CPU-side async meshing via worker threads
(already in place via `render/meshing_pool.rs`). Upload still happens
on the render thread but is bounded by `UNENDLICH_UPLOAD_BUDGET_MS`
and `UNENDLICH_UPLOAD_BYTES_BUDGET`.

**Cross-reference**: Search for "Outcome 3" in DESIGN_NOTES.md.

---

## Finding 2: Mesh upload contention is NOT the chug source

**Established**: v0.24.3-upload-pressure (increment 45), 2026-05-14.

**Claim**: Per-frame heavy CPU memcpy into HOST_COHERENT mapped GPU
memory does not cause the chug spikes. The "memcpy invalidates GPU
cache lines → next read stalls" hypothesis is falsified.

**Evidence**: A/B test with `UNENDLICH_UPLOAD_BYTES_BUDGET=8MB`
(default) vs `2MB` (aggressive cap):

| Run | upload_pressure events | gpu_spike events | ratio |
|-----|------------------------|------------------|-------|
| baseline | 643 | 73 | 8.8× |
| aggressive cap (2MB) | (more events, perceived MORE chugs) | similar | — |

Heavy uploads happen routinely without causing chug. ~9× as many
upload-pressure events as chugs. Tightening the budget made chugs
*worse* (more uploads needed → more frames where any work happens
at all), not better.

**Architectural implication**: Don't pursue upload-side fixes
(staging buffers, ring uploads, deferred uploads) as chug mitigations.
The bytes budget is a useful **terrain-load pacing** knob — it
controls how quickly the world fills in — but it's not a perf knob.

**What we kept**: `UNENDLICH_UPLOAD_BYTES_BUDGET` env var, default
8MB. Useful for tuning terrain-load behavior vs frame-time variance,
not for chug.

---

## Finding 3: Shadow is NOT fragment-bound

**Established**: v0.24.3-shadow-size (increment 46), 2026-05-14.

**Claim**: Shadow GPU cost does not scale with shadow map area.
Reducing the depth target from 2048² to 1024² (4× fewer fragments
to write) produces ~5% time reduction. The cost is upstream of
fragment shading.

**Evidence**: A/B at rd=8:

| Shadow map | shadow p50 | shadow over_8.3ms |
|------------|-----------|-------------------|
| 2048² (default) | 77 ms | 96.5% |
| 1024² | 73 ms | 96.0% |

Only ~5% reduction from 4× area reduction.

**Architectural implication**: Shadow cost is vertex/draw-bound
(vertex transform, primitive setup, tile binning), not fragment-bound.
Lowering shadow resolution alone won't fix the chug. The lever has
to be reducing chunks processed, not pixels written.

**What we kept**: `UNENDLICH_SHADOW_MAP_SIZE` env var (512 / 1024 /
2048, default 2048). Available as a quality knob but not a perf one.

---

## Finding 4: Shadow culls 16× more chunks than main

**Established**: v0.24.3-cull-count (increment 47), 2026-05-14.

**Claim**: The shadow pass's sun-aligned ortho frustum admits ~16×
as many chunks as the camera's perspective frustum. Shadow renders
effectively every loaded chunk; main renders 1-5 chunks.

**Evidence**: CPU-side cull-count event at rd=8 (mean ratio 16.05):

```
Worst 10 frames (by shadow_count):
    shadow      main     total     ratio
        44         1        44     44.00
        43         2        43     21.50
        43         1        43     43.00
        ...
```

When the camera is looking at ~1-5 chunks (player surrounded by
sky / facing a partially-loaded direction), the shadow cull still
admits ~40 chunks because the sun-aligned ortho covers everything
in a wide horizontal band around the camera.

**Architectural implication**: The chug at rd=8 is "shadow has same
per-chunk cost as main but renders 16× more chunks." Fixes that
attack chunk count are the right direction:

- Tighter cascade (shorter far distance)
- Multi-cascade with proper splits (each cascade is small)
- Shadow LOD (distant chunks use coarse meshes for shadow)
- Camera-extruded shadow cull (only render chunks whose shadow
  could land in the camera frustum)

**What we kept**: `UNENDLICH_CULL_COUNT_FRACTION` not added; the
diagnostic event remains permanent at low cost.

---

## Finding 5: AGX TBDR makes loadOp=LOAD expensive on shadow maps

**Established**: v0.24.3-shadow-stride (increment 48), 2026-05-15.

**Claim**: On AGX, beginning a render pass with `loadOp=LOAD`
incurs significant per-frame memory traffic cost: each tile must
read the existing attachment contents from main memory into tile
cache before any draws can proceed. For a 2048² depth buffer this
is ~16MB of memory traffic per pass, every frame.

This makes temporal-shadow techniques (render 1/N chunks per frame,
preserve previous frames' depth) structurally inefficient on AGX
even though they work fine on immediate-mode GPUs.

**Evidence**: At rd=8 with `UNENDLICH_SHADOW_CHUNK_STRIDE=8`,
stride engaged on 96.5% of frames (228/230). Geometry per frame
reduced by ~8×. Expected shadow time: ~10ms. Actual:

| Setting | shadow p50 | shadow mean | total p50 |
|---------|-----------|-------------|-----------|
| stride=1 (CLEAR) baseline | 77 ms | 78 ms | 98 ms |
| stride=8 (LOAD) | 70 ms | 58 ms | 109 ms |

Shadow time dropped only ~9% despite 8× less geometry. The LOAD
pass absorbed nearly all the geometry savings.

Note: comparison is workload-contaminated (walking vs flying made
the stride=8 run have 2× the loaded chunks). Even so, per-chunk
shadow cost DOUBLED in the stride=8 run, opposite of what stride
amortization should produce.

**Architectural implication**: Temporal shadow as a chug mitigation
on AGX requires a different mechanism than "don't clear, render
subset." Options that might work:

- Multiple smaller LOAD passes won't help (each pays its own LOAD
  cost; the fixed cost is per-pass, not per-MB).
- Subpass merging with shadow as a subpass of the main pass could
  avoid the LOAD entirely, but the shadow map must be sampled
  later, breaking the merge.
- The architecturally-correct path is **render less geometry per
  pass** so a single CLEAR pass is fast enough. This is the
  multi-cascade direction (4× cascades each covering less area).

**Cross-reference**: RasterGrid article on TBR GPUs
(https://www.rastergrid.com/blog/gpu-tech/2021/07/gpu-architecture-types-explained/):
"even depth-only rendering requires the geometry data to be written
out to memory and consumed by the rasterizer" on TBR.

**What we kept**: `UNENDLICH_SHADOW_CHUNK_STRIDE` env var (1-16,
default 1 = current behavior). Available but not useful as a
default on AGX. May be useful on future targets (discrete GPUs
where LOAD is cheaper).

---

## Finding 6: MAX_FRAMES_IN_FLIGHT=1 means CPU pays full GPU spike

**Established**: end-to-end render loop read during
v0.24.3-multi-queue-a (increment 44), 2026-05-14.

**Claim**: With `MAX_FRAMES_IN_FLIGHT = 1` (current default), there
is no pipeline buffer between CPU and GPU. When GPU produces a
200ms shadow spike, the CPU sits in `vkWaitForFences` for the full
200ms before it can start the next frame. The chug magnitude
*is* the GPU spike magnitude.

**Evidence**: Both the wait_fences_spike and gpu_spike instrumentation
consistently show wait_ms ≈ shadow_ms + main_ms across the entire
chug arc. CPU never overlaps with GPU because there's no slot to
overlap into.

**Architectural implication**: FIF=2 would let the CPU prepare frame
F+1 while the GPU finishes frame F. A 200ms shadow spike would still
happen on the GPU side, but the CPU could keep working — the user
might see the frame F+1 image only when the GPU's frame F finishes,
but input is still being processed throughout. Net effect: hides
GPU spikes at a +1 frame latency cost.

**Status**: NOT implemented. FIF=1 is a deliberate "no-latency"
design choice. Reconsidering FIF=2 is filed as a tradeoff lever for
"explorer mode" play at high rd where chugs are inevitable.

---

## Finding 7: 4-cascade rendering regresses on Honeykrisp (per-layer FB path)

**Established**: v0.24.2c.5 bisect, before this arc.

**Claim**: An implementation of 4-cascade shadow rendering using
per-layer framebuffers (one framebuffer per cascade layer) was 2.8×
slower than the single-cascade path on Honeykrisp. Root cause
unknown.

**Evidence**: Inline comment in `shadow_map.rs`:
> "v0.24.2c.6 reverted to using this single framebuffer after a c.5
> bisect found that per-layer framebuffers caused a 2.8× shadow-cost
> regression on Honeykrisp (root cause not yet understood — filed
> for driver-investigation. The path forward for CSM is probably
> multiview rather than per-layer framebuffers)."

**Architectural implication**: The design's filed 4-cascade plan
(C0 20m / C1 60m / C2 180m / C3 600m at 1024² each) is the correct
architectural answer to the chug, but the obvious implementation
path is blocked by this regression. The probable workaround is
**multiview** — render all cascades in a single render pass using
view layers, sidestepping per-layer framebuffers entirely.

**Status**: Filed; not yet attempted. The chug arc didn't get this
far because the diagnostic phase took most of the budget.

---

## Open hypotheses (not yet tested)

The chug arc ran out of small-ship momentum at finding 5. These
remain plausible candidate mechanisms / fixes, not tested:

1. **DVFS clock-ramp tax**: AGX firmware does opaque DVFS. The
   "first work after idle pays a clock-ramp cost" hypothesis was
   raised but never tested. A diagnostic would be a "keepalive
   compute dispatch" that runs every N ms to prevent the GPU from
   clocking down, then re-measuring chug rate. If chugs decrease,
   DVFS was the mechanism.

2. **Firmware command scheduling latency**: Periodic firmware-side
   housekeeping (e.g. memory mapping, page table updates) could
   cause occasional fixed-cost stalls regardless of submitted work.
   No way to test from user-space without driver instrumentation.

3. **AGX TLB miss on cascade origin shift**: When the cascade
   moves enough that a different memory region is accessed, TLB
   miss cost. Speculative.

These are filed in case a future investigator wants to pick them up.

---

## Approach options for the chug, by status

These are the levers we identified during the chug arc. Status as
of 2026-05-15.

### Tested

| Approach | Result | Status |
|---|---|---|
| Multi-queue compute | Single QF on AGX | Closed (Finding 1) |
| Cap upload bytes | Doesn't correlate | Closed (Finding 2) |
| Lower shadow resolution | 5% improvement | Closed (Finding 3) |
| Temporal shadow stride | LOAD pass too expensive | Closed for AGX (Finding 5) |

### Filed, not yet tested

- **Tighter single cascade** (drop cascade far from 200m to ~100m).
  Smallest possible test. Predicted impact: shadow ~halves; visible
  pop-in at the new boundary. Would clearly validate "chug scales
  with chunks rendered" without architectural surgery.

- **4-cascade with multiview** (per Finding 7's note). The design's
  actual plan. Substantial work; likely 1-2 sessions of focused
  effort. Sidesteps the c.5 per-layer FB regression by using a
  single multiview render pass.

- **Camera-extruded shadow cull**: Cull chunks against
  (camera_frustum ∪ extrude(camera_frustum, -sun_dir)). Only chunks
  whose shadows could land in the camera frustum render. Geometrically
  correct, medium complexity, requires shader changes.

- **Shadow LOD**: Distant chunks use LOD-far meshes for shadow
  rendering. Massive vertex reduction at far cascades (paired with
  multi-cascade, lower cascades get LOD-near, far cascades get
  LOD-far). Requires separate per-cascade indirect buffers.

- **MAX_FRAMES_IN_FLIGHT=2**: Hide GPU spikes behind +1 frame
  latency. Trivial change (constant + a few buffer-count adjustments).
  Trades against the "no-latency" design philosophy. Useful at
  high rd where chugs are inevitable; not useful at low rd where
  there's no chug to hide.

### Orthogonal but valuable

- **Working-distance buffer ring**: Pre-mesh chunks beyond view
  distance so they're ready when the player crosses in. Addresses
  the "chunks loading underneath me" perception separate from chug.
  User-described concept, filed in DESIGN_NOTES.

- **GPU mesher (gpu_mesher_vk.rs wire-up)**: Eliminates the CPU
  meshing path entirely, freeing mesh worker threads and removing
  CPU→GPU upload traffic. Multi-session architectural work; 715
  lines of dead code already in tree, never wired in.

---

## Procedural notes (for next investigator)

- Run `chug_capture.unendtest` (rd=2) for **detection** — confirms
  the engine compiles and the spike pattern exists.
- Run `chug_capture_rd8.unendtest` (rd=8) for **characterization** —
  the rd=8 workload is where shadow cost overwhelms everything else.
  rd=2 hides the signal.
- Walk, don't fly, when testing temporal/cache-warming techniques.
  Fly mode triggers cascade-shift force-fulls and chunk-loader
  saturation, contaminating perf comparisons.
- Spec env vars OVERRIDE command-line env vars. To override the spec's
  rd or shadow size: edit the spec file, don't prefix the command.
- Each ship's data is in `~/.unendlich/runs/r-*/events.jsonl` and
  `~/.unendlich/latest-events.jsonl`. The analyzer subcommands
  (`wait_spikes`, `gpu_spikes`, `cull_count`, `shadow_stride`,
  `upload_pressure`) summarize them.
