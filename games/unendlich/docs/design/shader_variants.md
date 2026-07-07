# Per-Chunk Shader Variants (experimental)

> Part of the perf work — see `PERF_PROFILING.md`, `shader_pass_notes.md`,
> `mesh_vision_scheduling.md`.

Compile several versions of `chunk.frag` with expensive features removed, then
bind the lightest variant a chunk actually needs. A far, flat, waterless chunk
doesn't pay for water reflections, a point-light loop, full fog steps, or
shadow PCF it can't meaningfully show — minimal quality loss, per-fragment
saving that scales with how much screen those chunks cover. This extends the
shader's EXISTING compile-time toggle pattern (`#if PBR_QUALITY` /
`#if PBR_NORMAL_MAP`); it is not new architecture.

## Status (honest split)
- **Selector model** (`sim::shader_variant`, 7 tests): pure, always built,
  done. Chunk properties → cheapest `ChunkVariant` → `-D` defines + a cache key.
- **Shader flags** (`chunk.frag`): all five declared, defaulting to current
  behaviour (an undefined build is byte-identical to pre-r244). **Two are fully
  WIRED and verified** to change the compiled shader safely:
  - `CHUNK_SHADOWS=0` → skips the shadow-map PCF block, `shadow_factor = 1.0`
    (fully lit). Clean: the no-shadow fallback already existed.
  - `CHUNK_FOG_STEPS=N` → the fog march sample count (the `const int FOG_STEPS`).
  **Three are DECLARED but NOT YET wired** into the shading code:
  `CHUNK_FOG`, `CHUNK_WATER`, `CHUNK_DYNAMIC_LIGHTS`. The fog and water paths in
  `chunk.frag` are *interwoven* through the final `color` blending (fog touches
  ~10 lines across 770–1091; water shading feeds specular/normal earlier), so
  wrapping them in `#if` correctly needs care against a running build — doing it
  blind risks compiling-but-rendering-wrong. They compile today (the macro is
  defined) but don't yet remove their work. Wiring them is the next step.
- **Pipeline integration** (`shader-variants` feature, off): compiling the N
  variant pipelines and binding per chunk is the gated runtime work — not done.

All variant flag combinations currently compile + pass `spirv-val`
(`tools/validate_shaders.sh`, plus the explicit matrix in the r244 ledger).

## Selector rules (`sim::shader_variant::select`)
Conservative — a feature is dropped only when it cannot meaningfully show:
- **water**: kept iff the chunk contains water (a distant lake still needs it).
- **dynamic_lights**: kept iff a dynamic/point light is in range.
- **shadows**: kept iff within shadow-casting distance (distant self-shadows are
  sub-pixel).
- **fog**: always on (aerial perspective matters at distance too), but
  `fog_steps` drops with the distance band (near 6 → mid 3 → far 2), never < 1.

`ChunkVariant::HIGH` (all on, 6 steps) == the un-gated shader, so anything the
rules keep is unchanged. `MAX_VARIANTS` bounds the pipeline cache (24).

## Integration recipe (the `shader-variants`-gated part)
1. **Compile the variant pipelines.** At startup (or lazily on first need),
   compile `chunk.frag` once per distinct `ChunkVariant` the world uses, passing
   `variant.defines()` to the shader compiler (glslc accepts `-D`; the build.rs
   path or a runtime shaderc). Key the pipelines by `variant.key()`.
2. **Tag each chunk.** When meshing/streaming a chunk, record `has_water`
   (any water voxel — the mesher already knows the block set), and at draw time
   derive `band` (distance), `has_dynamic_light` (query the light set near the
   chunk AABB), `within_shadow_dist` (distance vs shadow cull dist, which the
   shadow system already has as `UNENDLICH_SHADOW_CULL_DIST`).
3. **Bind per chunk / per batch.** Call `select(info, params)` → look up the
   pipeline by key → bind it for that chunk's draw. With the indirect-draw path
   (`chunk_indirect`), group chunks by variant key and issue one indirect-count
   draw per variant (a few binds per frame, not per chunk).
4. **Gate it** under `#[cfg(feature = "shader-variants")]`; the single-pipeline
   path stays default until validated.

## How to test (needs a build + the feature)
Build with the feature, `UNENDLICH_FRAME_PROFILE=1`, and look at scenes dominated by
far/flat/dry terrain (a desert vista, open plains): the `draw` zone p99 should
drop (cheaper fragment shading on those chunks) with no visible change near the
camera. Compare a water-heavy scene (variants keep water) vs a dry one (variants
drop it). If distant terrain looks wrong (missing water sheen on a far lake,
hard shadow edges popping at the shadow-distance boundary), the selector is
dropping a feature too eagerly — tighten the band thresholds.

## Risk
Quality-vs-speed: dropping a feature a chunk *does* need shows a visible seam
(e.g. a far lake rendered as flat blue). The selector is deliberately
conservative (keep-when-unsure), and the boundaries (shadow distance, fog-step
tiers) should be tuned with the profiler + eyes on a running build. Nothing is
on the default path; the engine is unaffected until the gated pipeline work is
built and validated.
