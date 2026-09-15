# Boot-load GPU-guard crash — investigation status & handoff

> Status as of this doc: crash is diagnosed in detail, several real
> contributing causes are fixed, but the crash still reproduces. The
> remaining fix is scoped but **not yet built** — see "Next fix" at the
> bottom. Written for a fresh session to pick up without re-deriving
> everything here from scratch.

## The symptom

`unendlich-fullgame` self-stops during the boot chunk-load burst (first
~10-30 real seconds), always via the same `gpu_guard` message:

```
[gpu-guard] the GPU took N.Ns on this frame (main pass N.Ns) with everything
sheddable already off. Stopping before the driver's watchdog kills the
device -- on this system that takes down every other application too.
```

This is a deliberate self-stop (see `voxel-tools/src/gpu_guard.rs`'s own
doc), not a Vulkan error or panic -- it fires because a frame's GPU time
crossed `hard_us` (1.5s) with the shed ladder (Shadows → Volumetrics →
Distance → Exhausted) already fully spent. It reproduces reliably on:
Apple M2 Ultra, Asahi/Mesa 25.3.6 ("Honeykrisp" driver), 194GB unified
RAM, `UNENDLICH_GPU_MESHER=2` (octree GPU mesher), with or without
`ai-gpu` / `mesh-vision-sched`.

## What's confirmed and fixed this investigation

1. **`decorate_chunk_styled`'s weathered-top subdivision** was, before any
   fix, the dominant single-chunk triangle cost -- confirmed via a live
   diagnostic (`worst_mesh_since_last_log`, added to the periodic FPS log;
   see `render/meshing_pool.rs`'s `take_worst_chunk_diag_for_live_log` and
   `render/mesher.rs`'s `MeshDiagCounts`). `UNENDLICH_TOP_SUBDIVISION=2`
   plus a later **distance+facing-aware** scaling
   (`top_subdivision_for_block`, reusing `sim::mesh_vision`'s
   `ViewCone`/`MeshUrgency`) cut this dramatically -- confirmed by direct
   before/after log comparison (147968 tri → 4232 tri on the same
   diagnosed chunk shape).
2. **Plant/decoration stamping** (`decorate_chunk_styled`'s
   `plant_stamps`) was a real secondary contributor, capped via
   `UNENDLICH_MAX_PLANT_STAMPS` and later given the same distance+facing
   scaling (`plant_stamp_budget_for_chunk`).
3. **`gpu_guard`'s calm-recovery windup**: idle frames (GPU cost ~0, e.g.
   a still-loading world with nothing to draw) were counting toward the
   240-frame "safe to restore shadows" threshold, causing a real
   shed→restore→re-punish oscillation visible directly in session logs
   (shadow cost re-spiking to 500-700ms multiple times shortly after
   being shed). Fixed with a `MIN_INFORMATIVE_US` floor -- see
   `voxel-tools/src/gpu_guard.rs`.
4. **`Shed::Volumetrics` was a no-op** -- gpu_guard logged "dropping
   volumetric lighting" but nothing checked the shed level; the froxel
   fill compute pass ran unconditionally regardless. Wired for real.
5. **`mesh_cache` was almost certainly falsely blacklisted** by
   coincidental temporal overlap with one of these same crashes (the
   trial-blacklist heuristic in `safe_tune.rs` can't distinguish "this
   setting caused the crash" from "this setting happened to be on trial
   when something unrelated crashed"). `UNENDLICH_RESET_SAFE_TUNE=1`
   added as an explicit, honest escape hatch.
6. **Tested and ruled out**: a single 2GB Vulkan index buffer allocation
   (`device_budget::mesh_pool_mb`'s ceiling, live on any >64GB unified-RAM
   machine) as a driver-quirk crash cause. `UNENDLICH_MESH_INDEX_POOL_MB=512`
   reproduced the identical crash -- same triangle buildup, same shed
   sequence, same abort point. **Not the mechanism here.** Worth keeping
   as a known lever for other hardware (a real `maxMemoryAllocationSize`
   query would be the properly general fix, not yet built -- see "Also
   worth doing" below), but this investigation's crash is not that.

## What's still causing the crash

With per-chunk cost now well-controlled (no single chunk looks
pathological in recent logs -- worst chunks are 15-25K tris, versus
150K+ before any of the above), the crash has shifted from "one monster
chunk" to **pure aggregate load**: `rendering` climbs past 100
simultaneously-live chunks during the boot burst, and total scene
geometry (`total_idx`/3) reaches 300-400K+ triangles before the GPU-time
budget is blown -- from many *individually reasonable* chunks, not a
bad one.

**`mesh-vision-sched` (the cone-based mesh-defer gate) does not fix this
specific scenario.** It was active (`--features ...,mesh-vision-sched`)
in the run that produced the aggregate-load numbers above and made no
visible difference. Working theory, not yet instrumented to confirm:
spawn is open sky with a wide default FOV, so most nearby chunks are
genuinely within the player's cone at boot -- there's little for
occlusion/facing-based deferral to defer. This is not a bug in
`mesh-vision-sched`; it's a scenario the tool wasn't built for (it should
still help a lot once actually walking around, especially in caves,
where its occlusion check has real solid geometry to work with).

**`world::gen_falloff` does not fix this either**, and says so in its
own doc comment: "these smooth *generation load*; they do **not**
address GPU draw cost." It paces CPU-side chunk generation requests, a
genuinely different and already-solved problem, not mesh/draw volume.

**Checked and confirmed absent**: no existing mechanism anywhere in this
codebase ramps render distance or mesh-request volume specifically
during boot to control GPU draw cost. `UNENDLICH_WARMUP_MS` is a
measurement-only warmup (delays when perf stats start recording, changes
no actual behavior). `warmup_full_render` in `world_renderer.rs` is
shadow-pass-specific, unrelated to render distance.

## Next fix (scoped, not yet built)

**A boot-time render-distance / mesh-request ramp**, independent of both
cone occlusion and generation pacing: request a much smaller effective
render distance for the first several seconds (or until frame time
proves stable), then relax back to the configured render distance. This
directly attacks the actual failure shape ("too much needed geometry
arrives in the same few seconds") rather than trying to cull it after
the fact once it's already all requested.

Design note: model the ramp's recovery on `gpu_guard`'s own already-
proven "ease in, don't oscillate" pattern (slow calm-based give-back,
fixed in this investigation) rather than inventing new hysteresis timing
from scratch -- same shape of problem (don't flip a state back and forth
based on noisy signals), already-solved once in this exact codebase.

Where this would likely hook in: `chunk_manager.rs`'s shell-walk (the
same place `mesh-vision-sched`'s per-chunk urgency check and the
`sky_cull` heuristic already live) is probably the natural point to cap
the *requested* radius early on, separate from `UNENDLICH_RENDER_DISTANCE`
itself. Not investigated in detail -- this is the scoping starting
point for the next session, not a final design.

## Also worth doing (lower priority, flagged not built)

- **`Shed::Distance` wiring** -- gpu_guard's third shed tier is still a
  no-op like Volumetrics was. A real implementation exists as an option
  (`apply_chunk_occlusion`/`clear_chunk_occlusion`, a CPU-side per-slot
  occlusion flag the GPU cull shader already respects, no shader changes
  needed) but composing it correctly with the existing cached occlusion-
  flood feature (`unendlich_fullgame.rs`, gated behind its own debug
  toggle) needs real care against silently breaking that existing
  optimization -- deferred for exactly that reason, not attempted blind.
- **A real `maxMemoryAllocationSize` Vulkan query**, clamping mesh-pool
  buffer sizes against the actual driver-reported limit instead of only
  a RAM-size proxy -- general hardening for other hardware, not needed
  for this specific investigation's crash (see "ruled out" above), not
  built because it's raw Vulkan FFI in `render/context.rs`, a higher-risk
  category than the pure-Rust logic elsewhere in this investigation.
- **Skylight/block-light pipeline wiring** -- a separate, unrelated
  finding from this same investigation: `light.rs` has a complete,
  tested BFS skylight+colored-block-light flood-fill (including
  incremental `add_light`/`remove_light` for edits) that is never called
  from chunk generation or consumed by the mesher. Live effect: every
  block face, including deep cave interiors, gets lit by raw sun-
  direction math with no occlusion test at all ("sun leak" underground).
  Real, scoped, cross-cutting fix -- see conversation history around the
  cave-lighting discussion for the staged plan (skylight-only first, no
  color, persisted-or-not decision, mesher consumption point). Not
  started.

## Testing notes for whoever picks this up

- `UNENDLICH_RESET_SAFE_TUNE=1` is worth including in every test run at
  this point, so a coincidental blacklist from an unrelated crash doesn't
  contaminate results.
- The live `worst_mesh_since_last_log` diagnostic line only covers the
  CPU greedy mesher path, not `UNENDLICH_MESHER=binary` or the octree GPU
  mesher's own internal cost -- if `UNENDLICH_GPU_MESHER=2` chunks stop
  showing up in that line, that's not evidence they're free, just evidence
  they're not instrumented.
- `ai-gpu` (pulls in `apps/aperture`'s local-LLM engine, unrelated to
  chunk meshing) produced a pink-screen visual artifact in one session,
  not reproduced/investigated further -- worth isolating separately from
  this crash if it recurs, since it's a plausible but distinct GPU-
  resource-sharing bug between two independent engines on the same
  device.
