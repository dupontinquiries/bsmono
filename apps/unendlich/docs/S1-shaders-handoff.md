# S1 — dirt-on-dragons + dynamic lights (handoff for bin wiring)

Two PURE-core deliverables, both verified in-sandbox (rustc --test + glslangValidator/spirv-val
+ headless PNGs). NO bin/descriptor/pass wiring applied (SHARED_RULES §1/§2). Proposed wiring:

## 1. sim::creature_grime  (data only)
- Per creature, keep a `GrimeState` (Default = clean). Each tick call
  `state.accumulate(&GrimeSample{ soil_grime01, wetness01, activity01, soil_tint, dt })`
  where soil_grime01/soil_tint come from the biome under the creature, wetness01 from
  water/rain, activity01 from locomotion speed. (A muddy-biome+running creature dirties;
  wading washes it.)
- The forge skin/material consumes `state.grime_factor01()` + `state.tint()`:
  for each forge box / vertex, weight grime by surface down-ness via `belly_weight(normal_y)`
  and composite with `apply_grime_color(base, normal_y, &state)` (the CPU reference the
  swatch + the .frag share). `grime_frag_glsl()` is the GPU form (push-constant grime+tint,
  v_uv.y = down-ness; darkens + outputs roughness in alpha). PROOF: creature_grime_swatch.png.

## 2. render::dynamic_lights  (codegen + CPU reference)
- Build a `[PointLight]` from emissive sources (forge glow boxes, eyes, fire): pos, color,
  intensity, radius. Per fragment region (chunk/froxel cell), `gather_nearest(center, &lights, N)`
  → the N nearest light indices; upload those N to a `Lights` UBO (layout emitted by
  `dynamic_lights_frag_glsl(N)`), set `frag_world.xyz` = shaded point, `.w` = count.
- The emitted .frag applies inverse-square × radius-window × clamp — IDENTICAL math to the
  `apply_lights()` CPU reference (the test pins them equal). PROOF: dynamic_lights_swatch.png.
- Wiring the UBO/descriptor + adding it to chunk.frag/entity.frag is the coordinator's
  on-device step (descriptors off-limits here). The math + GLSL are ready and validated.

VERIFY (coordinator gate): cargo check --release -p unendlich-engine --features render;
cargo test -p unendlich-engine --features gpu --lib creature_grime dynamic_lights.
