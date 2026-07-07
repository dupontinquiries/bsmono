# Deep multi-scale caves — WIRED & OPERATOR-CONFIRMED (r116b/c)

Status: caves confirmed working in the fullgame on hardware (operator run,
2026-06-11). This doc is the merge kit: what the feature is, exactly which
changes carry it, and how to graft it onto any other tree (e.g. a parallel
worker's branch) if that tree diverges.

## What it is

`world/cave_field.rs` (already in the repo since r102, unchanged) is a pure,
deterministic, depth-stratified cave carve:
- SHALLOW (surface .. -64): thin worm tunnels (intersection of two ridged
  3-D noise iso-surfaces => connected tubes, not specks) + sparse chambers.
- CHASM (-64 .. -512): fatter tunnels, bigger chambers, near-vertical fault
  fissures from a ridged 2-D field.
- DEEP (-512 .. -1024 floor): widest tunnels, biggest chambers.
- Surface integrity by construction: nothing carved within 6 blocks of the
  surface (SURFACE_BUFFER) and a smooth taper at the world floor.

## Why it was dark for months

The fullgame generates terrain on the **GPU** (`src/shaders/
chunk_generation.comp`); `cave_field` existed only in the CPU generator, and
even there behind `UNENDLICH_MULTI_SCALE_CAVES=1` which nobody set. Flipping
the env default (r116) changed nothing visible because the CPU path doesn't
produce the rendered voxels. The actual fix (r116b) is a GLSL port.

## The two changes that carry the feature (CAVES-STANDALONE.patch)

1. `src/shaders/chunk_generation.comp`
   - New block after `model_is_cave`: constants `CF_*`, helpers
     `cf_ridged_3d/cf_ridged_2d/cf_inv_down/cf_depth_gate`, and
     `bool cave_field_at(uint seed, float x, float y, float z, float surface_y)`
     — a structure-identical f32 port of `cave_field::cave_at` reusing the
     shader's existing CPU-symmetric `noise_2d`/`noise_3d`.
   - Three call sites OR it in, all using the per-column `surface_target`
     that RC39 already caches:
     a) the main solidity test (`bool is_cave = model_is_cave(...) ||
        cave_field_at(world_seed, dxf, dyf, dzf, surface_target);`)
     b) the `above_solid` surface-decoration probe (`dyf + 1.0`)
     c) the 3-block `near_surface` probe loop (`dyf + probe + 1.0`)
     All three must change together or surface grass/dirt decoration will
     disagree with the carve.

2. `src/world/cpu_generator.rs`
   - `multi_scale_caves_enabled()` default flipped to TRUE
     (`UNENDLICH_MULTI_SCALE_CAVES=0` opts out). Keeps the CPU generator
     (headless proofs, surface probes, any CPU-generated chunks) carving the
     same world the GPU does.

## Merging onto a diverged tree

- Try `patch -p1 < CAVES-STANDALONE.patch` first.
- If the .comp hunk fails: paste the `// ── r116b: deep multi-scale cave
  field` block verbatim after that tree's `model_is_cave`, then apply the
  three call-site edits above by hand (search `model_is_cave(` — there are
  exactly three non-definition uses in the generation kernel).
- If the cpu_generator hunk fails: flip the `.unwrap_or(false)` in
  `multi_scale_caves_enabled` to `.unwrap_or(true)` and invert the value
  test to `!(v == "0" || v.eq_ignore_ascii_case("false"))`.

## Build gotcha (the one that bites)

`build.rs` compiles `chunk_generation.comp` to SPIR-V only if glslang or
shaderc is installed; otherwise the build silently uses shipped pre-compiled
SPIR-V and you get NO caves with no error. After merging, confirm the build
log shows the shader recompiling. If the other tree vendors `.spv` files,
delete/regenerate them.

## Knobs

- `UNENDLICH_MULTI_SCALE_CAVES=0` — disables the CPU side only (the GLSL is
  unconditional for CPU/GPU symmetry; a GPU kill switch would need a push
  constant — not done, by design, to keep the generators identical).
- Density/morphology tuning lives in the `CF_*` constants (shader) and the
  matching constants in `world/cave_field.rs`; change BOTH or the generators
  desync.
