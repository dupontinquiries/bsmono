# r96 — depth-of-field: shader ready, wiring spec

`handoff/shaders-staging/dof.frag` is a complete DoF post-process (staged OUT of
`src/shaders/` so it doesn't trip the shader-artifact tracking tests while unwired —
move it into `src/shaders/` as step 1 of wiring), VALIDATED in-sandbox
(glslangValidator → SPIR-V + spirv-val both clean). It is NOT yet in build.rs
SHADERS[] or wired into a pass — that's the render step below, kept separate so it
can't disturb the current descriptor layouts until deliberately added.

## What it does
Fullscreen pass (present.vert). Per pixel: linearize depth → circle-of-confusion
from distance to a focus plane → 16-tap disk blur scaled by CoC. In-focus pixels
(CoC < 0.5 texel) pass through untouched (cheap). Bokeh-ish, single pass.

## Wiring (render, compile-on-end)
1. Add `"shaders/dof.frag"` to build.rs SHADERS[] (it auto-compiles to .spv; already
   validates).
2. New post-FX pass slotted AFTER bloom composite, BEFORE present/tonemap, gated by
   UNENDLICH_POSTFX (same gate bloom/SSR use).
3. Descriptor set 0: binding 0 = HDR scene color sampler, binding 1 = scene depth
   sampler. This is a NEW set for a NEW pass — it does NOT touch entity/chunk
   descriptor layouts (so it won't trip the descriptor_gate on existing passes).
4. Push constants (32 bytes): texel=1/size, focus_dist, focus_range, max_coc (e.g.
   start 6.0 texels), near, far, reverse_z (1.0 if reversed-Z — check the depth
   convention in camera.rs / the depth attachment).
5. Draw: same 3-vertex fullscreen triangle as bloom (no vertex buffer).

## Verify on-device
Toggle UNENDLICH_POSTFX; sweep focus_dist; distant terrain should blur while the
focus band stays sharp. A focus_range of ~0 with a large max_coc makes the effect
obvious for a first check.
