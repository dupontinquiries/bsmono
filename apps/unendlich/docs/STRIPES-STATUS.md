# Worldgen striping — status: ALL CLOSED (worldgen causes + the memory-pressure fallback stripe)

> **RESOLVED — confirmed 2026-05-26 (kesslerd).** No stripes on either a bare
> run (`cargo run --release --bin unendlich-world --features
> render,hot-shaders,window,gpu,ui-overlay`) or a heavily-loaded flag set
> (RENDER_DISTANCE=4, LOD_BANDS=3,6,9, GPU_MESHER=2 + MID, ADAPTIVE_UPLOADS,
> DEVICE_LOCAL_MESH, GPU_DECORATE, SEASONS/WEATHER, etc.). Crucially that run
> also had **`UNENDLICH_STRICT_SHADERS=1`**, so the committed spv matched source
> — i.e. the cave-flood density gate and the quintic fade in
> `chunk_generation.comp` both compiled and validated. The earlier "second,
> different stripe" (the OOM CPU-fallback band) is now closed too.
>
> **What closed it (the chain):**
> 1. `indirect-pool-tunable` — `UNENDLICH_MAX_DRAWS` raises the indirect-draw
>    slot pool so high render distance stops dropping chunks → no HOLES.
> 2. `lod-telemetry` + `UNENDLICH_LOD_BANDS` — tighter LOD bands cut the chunk
>    count under the device-memory ceiling → fewer GPU-mesher OOM fallbacks.
> 3. `stripe-fix-fallback-tint` — when a chunk *does* OOM-fall back to the CPU
>    greedy mesher, it now bakes the climate tint (`world_model` threaded through
>    `OctreeGpuMeshRequest`), so a fallback band is tinted like its GPU
>    neighbours instead of a flat untinted stripe.
> 4. `noise-quintic-smoothstep` — C2 fade removed the faint grid-aligned creasing
>    that read as fine banding on slopes/caves.
>
> The two original *worldgen* causes (below) remain closed and the parity lint
> still proves CPU/GPU generate identical voxels. This doc is kept as the record
> of the whole class so it cannot silently recur.

The original "terraces with sky showing through" striping had two independent
worldgen root causes. Both are closed; this doc records the evidence and the guard
added so that *class* cannot silently recur.

## Cause 1 — detail amplitude (math). CLOSED.
A 3D detail term with amplitude ~0.5 had a vertical slope comparable to the
`vertical_bias` gradient, so density crossed zero multiple times as `y` rose →
stacked solid/air slabs. Fixed by dropping the amplitude to `0.15` (≈3× margin),
so density is monotonic through the surface → one boundary.

- CPU reference: `world/world_model.rs::density` → `vertical_bias + detail * 0.15`.
- GPU worldgen: `shaders/chunk_generation.comp::model_density_with_target` →
  `vertical_bias + detail * 0.15`.

The two are byte-identical in the amplitude and in the noise/continental/hills/
sea-level helpers (the comp is annotated "must match … byte-for-byte", RC39/40).
Keep amp ≤ 0.15; ~0.3 stays safe, ~0.4+ reintroduces striping.

## Cause 2 — stale committed `.spv` (build). CLOSED.
The engine ships precompiled SPIR-V in `assets/shaders/*.spv` so it runs without a
shader compiler. If a `.comp` was edited (e.g. the amp fix) on a machine with no
`glslc`/`glslangValidator`, build.rs could not recompile and fell back to the OLD
`.spv` → correct source, wrong binary, stripes return.

build.rs already guards this with a SHA-256 sidecar (`*.spv.hash`): it recompiles
whenever the source hash != sidecar, and prints a loud box when the compiler is
missing AND a stale `.spv` is detected.

### Audit (this turn)
Hashing every source against its committed sidecar: **all 13 committed core
shaders match — zero stale**, including `chunk_generation.comp`. Matches the clean
terrain in-game.

### Gap found + fixed
Four shaders shipped a committed `.spv` but were **absent from build.rs's `SHADERS`
list**, so build.rs never hash-checked them — editing their source would silently
leave a stale `.spv` (the exact recurrence mechanism). Added to the list:
`chunk_mesh_octree.comp`, `octree_probe.comp`, `entity.vert`, `entity.frag`.

### New guard
`UNENDLICH_STRICT_SHADERS=1` turns the stale-`.spv` warning into a hard build
failure (panic in build.rs). Off by default (the no-compiler ship-spv path still
works); set it in CI / before a release to guarantee no stale shaders ship.

### Source-level parity lint
`tools/density_parity_lint.py` fingerprints the parity-critical magic numbers in
BOTH paths, in two checks: **density** (`VERTICAL_BIAS_SCALE`, continental/hills
scales, detail frequency/octaves/amplitude, clamp bounds — `world_model.rs::
density` vs `chunk_generation.comp`) and **noise** (hash avalanche + axis
multipliers, the hash→float divisor, fbm lacunarity/gain — `noise.rs` vs the
comp's inlined noise; a drift here corrupts terrain/caves/biomes/trees alike). It
fails (exit 1) on any divergence. The staleness guard catches a stale binary;
this catches a *source* drift. Currently reports PARITY (15/15 fields); verified
it flags simulated density-amp and noise-multiplier drifts. Run in CI.

## Residual / unrelated
- The new UI shaders (`egui.vert`, `egui.frag`, `ui_downsample.comp`,
  `ui_blur.comp`) have **no committed `.spv`** (compiled locally where a compiler
  exists). Building the `ui-overlay` feature therefore requires glslc/glslang. The
  default build (feature off) is unaffected.
- Density `amp` is the knob to watch; do not raise past ~0.3 without re-checking
  for monotonic crossing.
