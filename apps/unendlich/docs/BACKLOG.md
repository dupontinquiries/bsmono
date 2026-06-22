# BACKLOG.md — unendlich open items

Living list of open work, with enough technical context that any
session (Claude or human) can pick an item up cold. Newest / highest
signal at top. When an item ships, move it to a `## Done` section with
its tag, or delete it if the ship readme already covers it.

---

## P1 — Weather / seasons / water path

Full roadmap in `docs/CLIMATE_PATH.md`. Status:

- **Seasons (temporal axis): started (v0.24.NEXT+4).** `world/season.rs`
  + GPU-path render-tint, flag `UNENDLICH_SEASONS`. Worldgen-safe. Next:
  CPU-mesher tint parity; live transition; latitude; snow line.
- **Weather (rain/lightning/snow): atmospheric + phase physics (v0.24.NEXT+5,+8).**
  `world/weather.rs` (precipitation + rain↔snow by temperature + flicker
  lightning) → renderer darkens (rain) / whitens (snow) + storm wind, flag
  `UNENDLICH_WEATHER`. Next: share seasonal delta into weather temperature
  (winter→snow in temperate biomes); snow accumulation (`Snow`/`SnowBlock`
  on ground — worldgen/sim); particles, bolt geometry, thunder, wetness
  (render-side).
- **Water (field-driven fluid): foundation + bridge (v0.24.NEXT+6/+7).**
  `world/water_field.rs` — per-column depth heightfield + volume-conserving
  `settle_step` (steps 1–2), plus `seed_from_chunk` / `ground_heights_from_chunk`
  (the worldgen-seed + floor bridge). Pure + tested, fully inert. **Next is
  blocked**: a persistent per-chunk field + per-tick settle needs the
  chunk-manager/sim-scheduler files (not in this crate slice) and a place
  for field side-state (`Chunk` has none). Then step 3 (mesher reads field
  surface height) becomes the first visible payoff, then cross-chunk flow,
  erosion, evaporation/rain cycle.
- **Weather fronts (emergent): downstream.** Needs wind-vector-field +
  couplings + froxel fog. Current weather is per-location stochastic;
  built so the front model can replace it without changing consumers.

---

## P0 — GPU mesher overflow → missing meshes (FIXED v0.24.NEXT+9)

**Root cause:** `INDEX_CAPACITY` (65_535) was set equal to
`VERTEX_CAPACITY`, but indices run ~1.5–3× vertices, so dense foliage
chunks (observed v=42112 / i=117924) index-overflowed → empty mesh →
hole. Confirmed: CPU-mesher diag run had no holes.
**Fix:** raised `INDEX_CAPACITY` → 131_072, plus a CPU-mesher fallback on
overflow (`UNENDLICH_GPU_OVERFLOW_FALLBACK`, default on). Holes filled
regardless of density.
**Still open (separate bugs, need `chunk_mesh_octree.comp`):**
- **Magenta foliage** — GPU per-cell color path or climate tint. Confirm
  via `UNENDLICH_GPU_MESHER=0` (CPU) A/B.
- **Missing top faces** on rendered chunks — GPU top-boundary neighbor
  sampling.
- **LOD overlap** beyond the hole-reveal effect — LOD-band management.

---

## P1 — GPU mesher feature parity (block sides + decoration lighting)

**Partially shipped (v0.24.NEXT+2-gpu-decorate):** grass tufts, water
surfaces, and lava surfaces now run as CPU post-passes on GPU-meshed
chunks behind `UNENDLICH_GPU_DECORATE=1`. **Remaining:**
- **Per-face block color** (grass-top-green / dirt-sides). Computed in
  `chunk_mesh_octree.comp` — needs the shader (not in any zip yet).
  This is the "sides of blocks" half of the original report.
- **Decoration sun lighting.** Tufts/water/lava are currently lit
  neutral (`sky: None`) because `Sky`'s Clone-ability couldn't be
  confirmed (sky.rs out of scope). One-line follow-up: add `sky` to
  `OctreeGpuMeshRequest`, pass `Some(&r.sky)`.
- **Mid-LOD decoration density.** Full-res tuft density on a
  stride-decimated Mid base mesh may look mismatched. Evaluate in
  playtest; gate decoration to Near-only if needed (one line).

---

## P2 — (historical) original GPU-parity finding

**Symptom (reported):** grass not appearing; sides of blocks wrong.

**Root cause (confirmed v0.24.NEXT+1 session):** the octree GPU mesher
(`chunk_mesh_octree.comp`, dispatched by `gpu_meshing_octree.rs`) emits
only solid block faces, one color per cell. Three CPU-mesher post-passes
have no GPU equivalent and are skipped for every Near/Mid chunk when
`UNENDLICH_GPU_MESHER=2` (+`_MID=1`):

| Feature                      | CPU mesher (mesher.rs)         | GPU mesher        |
| ---------------------------- | ------------------------------ | ----------------- |
| Grass tufts / sub-voxel deco | `decorate_chunk` (L1670)       | **missing**       |
| Water surface tops           | `emit_water_surface` (L1677)   | **missing** (skips water) |
| Lava surface tops            | `emit_lava_surface` (L1683)    | **missing**       |
| Per-face block color         | `block_color_per_face` (L843)  | **single color/cell** |

Dispatch that bypasses the passes: `meshing_pool.rs:520–558` — GPU-routed
chunks `continue` before the CPU mesher's post-pass block runs.

Note: the RC42.1 "sub-voxel templates validated (787/0)" tag validated
templates in the CPU mesher only. `gpu_meshing_octree.rs` has zero
references to `subvoxel`/`template`/`tuft`/`decorate`.

**Fix options (need a decision + hardware to validate):**

1. **CPU post-decoration pass on GPU-meshed chunks.** After GPU mesh
   readback, run `decorate_chunk` + water + lava on the CPU and append
   the geometry to the GPU mesh. Reuses tested CPU code, no shader work.
   Cost: the GPU dispatch path (`meshing_pool.rs:525+`) currently drops
   the chunk voxel data after routing — it'd need to keep the chunk
   around to readback-time, and the readback path needs an append/merge
   step. Per-face color still wrong (that's shader-side). Partial fix:
   gets tufts/water/lava back, not side colors.

2. **Port decoration + per-face color into `chunk_mesh_octree.comp`.**
   Keeps everything on GPU. Needs the shader (NOT in the cumulative zip
   — request it). Sub-voxel templates need a GPU-readable representation.
   Hardest, but the only path that fixes per-face side colors and keeps
   the perf win.

3. **Hybrid:** per-face color in the shader (small, addresses "sides"),
   tufts/water/lava via CPU post-pass (option 1). Two ships.

**Blocked on:** decision between 1/2/3, and for any shader work, the
`assets/shaders/chunk_mesh_octree.comp` + its `.comp` source. Can't be
validated in the sandbox (no GPU/cargo) — needs a playtest.

---

## P1 — The last stutter

**Symptom (reported):** one remaining frame-time stutter to eliminate.

**Lock analysis (v0.24.NEXT+3 session, code-grounded):**

- **`queue_submit_lock` is NOT the cause.** It's already tightly scoped
  to just the `vkQueueSubmit` call (gpu_meshing_octree.rs ~742–745); the
  fence-wait and readback run without it. The render thread's draw submit
  contends only for the ~µs submit, not the ~1ms batch.
- **Octree pool lock is the documented contention**, held across
  upload+dispatch+(fence-wait+readback). Author rated it ~30µs average at
  steady state — not a stutter on its own. BUT during chunk-storm bursts
  (huge render distance, fresh neighborhoods) many main-thread octree
  uploads stack behind back-to-back GPU mesh batches, and with
  `MAX_FRAMES_IN_FLIGHT=1` that wait lands directly on the frame.
- **v0.24.NEXT+3 ships a flag-gated mitigation:**
  `UNENDLICH_GPU_POOL_EARLY_UNLOCK=1` releases the pool lock right after
  the fence (before readback), shortening the contended window by the
  readback duration. Provably safe (GPU done with pool at fence-signal);
  default off pending A/B. If a capture shows pool-lock waits in the
  stutter, this is the lever.

**Remaining true-stutter suspects (need a launcher capture to rank):**

- Shadow map regeneration spikes (there's `UNENDLICH_SHADOW_CHUNK_STRIDE`
  tuning history — shadow p50 was ~77ms in one capture).
- Per-frame allocation that scales with visible chunk count.
- `MAX_FRAMES_IN_FLIGHT=1`: zero CPU/GPU overlap means *any* single
  expensive frame (a large upload, a shadow refresh, a GC-like sweep)
  shows as a stutter with no buffer to hide behind.

**Next step:** capture with the launcher, `analyze` the frame-time spike's
phase attribution, THEN pick the lever. Don't guess-and-patch — the
above are ranked by suspicion, not measurement.

---

## P2 — Carried-over open items (from RC42.1 README + prior readmes)

- **`read_block_local` coherency on Honeykrisp** (RC41). Speculative
  hardware-specific memory-coherency suspicion. Needs the device or a
  CPU-ref test isolating the suspect shader read pattern.
- **VSYNC plumbing audit.** `UNENDLICH_VSYNC=0` should select
  IMMEDIATE/MAILBOX; the read happens in `swapchain.rs` (not in the
  cumulative). Confirm it's honored; send the file if not.
- **Swapchain `imageCount` reduction.** Fewer buffered frames for lower
  latency. Same file (`swapchain.rs`) needed.

---

## P3 — Doc debt

- Fill in `LAUNCHER.md`'s 🟡 markers once the launcher crate, analyze
  crate, `.unendtest` schema, and `commands/*.sh` scripts are seen.
- Rename the `v0.24.NEXT*` placeholder readmes/tags to real version
  numbers.

## Water top-surface: sub-voxel + shader (operator request, r441)
The top layer of simulated water renders as full solid blocks → blocky/ugly waterline. Want: render the surface
layer as a 2×2 (or finer) sub-voxel grid driven by the fractional depth + the wave/flow fields, with a shader
pass, instead of the all-or-nothing solid-block fallback. Needs the transparent-water mesher split (already
staged) as a prerequisite so the surface layer is its own geometry.

## Dragon spawn density (operator note, r441)
Some seeds spawn far too many dragons for playable gameplay. Add a spawn-density cap / per-region budget.
