# `cull` pass at ~22ms — diagnosis & fix

## What the cull pass is
`shaders/chunk_cull.comp`, a compute pass dispatched once for the main view (64-wide workgroups,
`chunk_count = high_water`). Each invocation handles one chunk slot: reads its AABB, tests it against
the 6 frustum planes, and **if visible** does `atomicAdd(visible_count, 1)` on a single global counter
and writes a `VkDrawIndexedIndirectCommand`. CPU then issues one `drawIndexedIndirectCount`.

## Why it costs ~22ms now
The only work that scales with the **visible** count is the per-visible-chunk global atomic + scattered
command write. Inactive/frustum-culled slots early-out cheaply. So cull cost ∝ visible chunks, and the
single hot `visible_count` atomic serializes on the GPU (especially on Apple/Metal via MoltenVK, where
global-atomic throughput is limited). Evidence in the logs:
- cull p50 went **0.04ms → 22ms** as visible chunks went **1840 → 6491**; meanwhile cull **max stayed
  ~32–37ms across both runs**. I.e. the worst case was always there; heavy scenes just made it the median.
- ~6491 fully-serialized atomics on one address + ~6491 scattered 20-byte writes ≈ the observed cost.
Secondary: `high_water` never shrinks on chunk free (in-code TODO, chunk_indirect.rs:392), so the dispatch
covers every slot ever used (~19,134 invocations here) even when fewer are active — wasted *cheap*
invocations, not the main driver.

## Fix — per-workgroup stream compaction (shipped: outputs/chunk_cull.comp; fix_cull_workgroup_aggregation.patch)
Aggregate within each 64-lane workgroup instead of hitting the global counter per chunk:
1. each lane computes its `emit` bool (all lanes fall through to the barriers — uniform control flow);
2. visible lanes get a local offset via a **shared-memory** atomic (on-chip, cheap);
3. lane 0 does **ONE** `atomicAdd(visible_count, workgroup_total)` to reserve a contiguous block;
4. visible lanes write their command into `block_base + local_offset`.
Global atomics drop from O(visible) (~6491) to O(workgroups) (~300) — ~20× here, up to 64× dense — and
the command writes become contiguous (coalesced). Behavior-preserving: draw order is irrelevant, and the
shadow `stride`/`stride_phase` path, the debug-overlay flag, and `first_instance = slot_index` are all
kept. **Compiles clean to SPIR-V (glslangValidator).** Can't measure on the GPU here — apply the `.comp`
and rebuild; the `hot-shaders` feature regenerates the `.spv` (RULESET R2). Expect cull to fall toward
the froxel/near-zero range.

## Optional secondary
Lower `high_water` on free / compact the slot table (the in-code TODO) to shrink the dispatch — trims the
inactive-invocation overhead; minor next to the atomic fix.

## Pattern note
Same shape as the heat-field fix: a per-item global operation whose cost scaled with count. The remedy is
the same family — aggregate / index so the hot global op stops scaling with N.
