# Perf: the meshing chug + the steady frame cost (r248 analysis)

> Based on the operator's richer capture (subsystem profiler: gpu/cpu_zones/
> mesher/tick alongside the per-zone percentiles). Two SEPARATE problems.

> ⚠️ CORRECTION (r249): the cave-skip fix proposed in this section was WRONG and
> has been reverted. Far regions must still compute caves/density because that
> terrain feeds regional simulation (water/wind/storms/clouds/population) that
> propagates between regions — it is not render-only waste. See
> `PERF_FINDING_far_region_corrected.md` for the correct framing (compute the
> coarse terrain ONCE and share it between mesh + sim; cache/throttle the far
> mesh rebuild; never skip the computation the sim needs).

## Problem 1 — the MESHING CHUG (operator's headline; the big spikes)
The subsystem `mesher:` line is the smoking gun:
```
mesher: near=0.65ms×85  mid=0.00ms×61  far=11.20ms×77
mesher: far=12.70ms×26  ...  far=12.94ms×35   (every window with far activity)
```
**Far chunks cost ~11–13ms EACH to mesh**, dozens per 10s window. That is the
chug. Root cause, traced in code:
- Far/VeryFar chunks mesh on the CPU via `mesh_chunk_far_virtual`
  (`render/mesher.rs`), the "reverse-regen" path that **re-runs the world
  generator's density + cave sampling per grid cell** instead of reading blocks.
- At factor 4 that's 8³=512 generator samples/chunk, but each sample is
  multi-octave noise + cave evaluation — so ~11ms/chunk. The MESHING is cheap;
  the GENERATOR SAMPLING inside it is not.
- These run on worker threads (good — not directly on the main thread), but in
  BURSTS (far=×77), and the completed-mesh **upload + install lands on the main
  thread inside game_update**, so a burst of far chunks → frame spikes.
- Note `max_verts=64735` is the NEAR octree GPU mesher (dense near terrain),
  NOT the far path — that part is fine (GPU, ~sub-ms dispatch).

Fix directions (all need a run to validate; ranked):
1. **Cache/skip the generator samples in `mesh_chunk_far_virtual`.** The far
   regen recomputes density+caves that the generator already produced when the
   chunk first loaded. If the per-column heightmap / density field can be
   cached (or the far mesh derived from the already-downsampled chunk via
   `mesh_chunk_downsampled` instead of re-sampling the generator), the 11ms
   collapses. `mesh_chunk_downsampled` (the non-virtual path) reads the existing
   blocks and is the cheaper sibling — preferring it for Far when the chunk is
   already materialised avoids the regen entirely.
2. **Throttle far-mesh submission** so a burst can't stack: cap far-tier
   submissions per frame (a far-specific analogue of the upload budget), so the
   completed-install never lands 77 far chunks in a narrow window.
3. **Coarsen far further** (factor 4→6/8) — fewer samples, fewer verts; at RD4
   the far ring is barely resolvable anyway.

## Problem 2 — the STEADY ~16.5ms game_update p50 (the low median ceiling)
Separate from the spikes: `game_update` p50≈16.5ms holds STEADY even in windows
with `<no chunks meshed>`. None of the subsystem timers account for it
(creature_mesh measured 0.03ms; AI scans microbenched nanoscale; draw 1.9ms;
gpu async; cpu_zones ~2ms and those are inside draw). So ~14ms of every median
frame is uninstrumented per-frame game logic.
- The remaining suspect is the big **per-creature update loop** (~1000 lines:
  dragon_observe / rest_cycle / creature_grime / posture / stumble / leap /
  sound_masking, over ~200 creatures). Many subsystems × 200 creatures, each a
  few µs, sums to the ~14ms.
- r248 wires a **`roamer_tick` zone** around that whole loop to confirm. If
  `roamer_tick` p50 ≈ 14ms → that's it, and the fix is to put the per-creature
  subsystems behind the SAME `ai_budget` distance-band gate the loop already has
  for *some* of its work (so far creatures run their full subsystem stack every
  4–8 frames, not every frame), plus a hard cap on the active-roamer count
  whose full stack runs per frame.

## creature_mesh: confirmed NOT the spike, but worth fixing anyway
The r247 `creature_mesh` zone read 0.03ms p50 here — so the per-frame rebuild is
NOT the current bottleneck on this hardware. BUT (operator: lower-end hardware
adds up): the block still regenerates ~200 creatures' geometry into fresh Vecs +
re-uploads every frame unconditionally. The dirty-flag fix (cache, rebuild on
change, reuse the Vecs) remains worthwhile as a low-end-hardware win — just not
the priority the spikes are. Deferred to an M2-verified change (it alters the
upload).

## Status / honesty
- The far-mesh 11ms and the generator-sampling cause are READ FROM CODE +
  CONFIRMED by the mesher timing line. The fix is not yet applied (the
  caching/downsample-swap changes what far chunks render and must be validated
  on a real build).
- The steady-16ms attribution to the roamer loop is a HYPOTHESIS the new
  `roamer_tick` zone will confirm or refute on the next run.
- Both are real; the meshing one is the operator's felt chug and the priority.
