# SSR / post-FX — path forward

The plan for the screen-space work, structured so I can do it as a big
autonomous workload validated by the harness, and you verify in one pass per
milestone instead of eyeballing every subtle change.

## Design principle: one gate, zero regression

Everything new lives behind **`UNENDLICH_POSTFX`** (default OFF). When off, the
renderer is byte-identical to today (straight to the swapchain, tonemap inline
in `chunk.frag`). When on, the scene renders to an offscreen HDR target and a
fullscreen present pass tonemaps it to the swapchain. `UNENDLICH_SSR` and
`UNENDLICH_BLOOM` are sub-toggles that require `POSTFX`.

Why: I'm building the Vulkan plumbing blind. Gating it means a wiring bug can't
regress your normal play — you just don't set the flag — and "identical with the
flag on" is the acceptance test for the foundation. This matches your opt-in RC
style.

## Where the render loop is today (grounding)

In `world_renderer.rs` the frame is: acquire (~3035) → begin cmd (~3052) →
**shadow pass** (~3243–3413) → froxel compute dispatch (~3460–3532) → **main
pass** begin (~3675): bind chunk pipeline, draw chunks indirect (~3701), draw
entities (~3744), egui paints, end pass (~3765) → submit → present (~3810). The
chunk/entity/egui pipelines are all created against the swapchain-format render
pass; `depth.rs` is `DEPTH_STENCIL_ATTACHMENT` only (not sampleable).

## Increment 1 — offscreen foundation (the plumbing)

Goal: with `POSTFX=1`, the scene looks identical; with it off, nothing changes.

File changes:
1. **`render/depth.rs`** — add `SAMPLED` usage + a `vk::Sampler`; make
   `pick_depth_format` require `SAMPLED_IMAGE | DEPTH_STENCIL_ATTACHMENT`. (Safe
   even when unused.)
2. **`render/offscreen.rs`** (new, modeled on `depth.rs`) — an
   `R16G16B16A16_SFLOAT` image + memory + view + linear sampler,
   `COLOR_ATTACHMENT | SAMPLED`. Built only when `POSTFX` is on.
3. **`render/swapchain.rs`** — add `create_offscreen_render_pass(hdr_fmt,
   depth_fmt)` (color final layout `SHADER_READ_ONLY_OPTIMAL`) and
   `create_present_render_pass(swap_fmt)` (color only). Reuse the existing
   attachment-builder patterns.
4. **`render/present_pipeline.rs`** (new) — a pipeline for
   `present.vert`/`present.frag` (already shipped): no vertex input, one
   descriptor set (`binding 0 = scene_hdr`; depth added in inc. 3), against the
   present render pass.
5. **`chunk.frag`** — gate the final tonemap/grade/vignette on a new UBO flag
   (e.g. `atmos_params`-adjacent `postfx_active`): when set, output **linear
   HDR** (skip the tonemap block); `present.frag` does it. Default 0 ⇒ identical.
6. **`world_renderer.rs`** — when `POSTFX`: build offscreen target + the two
   passes + present pipeline; create chunk/entity pipelines against the
   **offscreen** pass and egui against the **present** pass; in the loop, run the
   main pass into the offscreen framebuffer, barrier offscreen color + depth to
   `SHADER_READ_ONLY`, then the present pass (draw 3 verts sampling `scene_hdr`,
   then egui on top). When not `POSTFX`: today's path verbatim.

Verification: `bash toolkit/harness/run.sh` (real compile + cargo), then run the
game with and without `UNENDLICH_POSTFX=1` — the two should be visually
indistinguishable. That's the whole acceptance bar; no effect math yet.

Risk: this is the blind part. The gate makes it safe to ship; if `POSTFX=1` is
black/wrong we iterate on exactly this list while your normal play is untouched.

## Increment 2 — reconstruction (math, GPU-free validation)

Pure functions (depth+uv+inv_proj → view pos; neighbor view-pos → normal;
view→screen project), mirrored in `present.frag`. CPU-reference test
`crates/vk-harness/tests/ssr_reconstruct.rs` round-trips a known view-pos and a
known-plane normal — runs in `cargo test`, no GPU. This is where the harness
earns its keep: the math is proven before it ever renders.

## Increment 3 — SSR ray-march + composite

Screen-space march in `present.frag` using `scene_depth` (binding 1) + a
reflectivity mask (water). CPU-reference test `ssr_raymarch.rs` validates
hit/miss + reflected-uv on synthetic depth. Reflection fresnel-blended into the
linear color **before** the present tonemap, only where reflective.

## Bloom — the quick visible payoff

Once increment 1 lands, bloom is cheap and the most visible single effect:
bright-pass the offscreen HDR → `ui_downsample`/`ui_blur` chain (already in the
tree) → additive composite in `present.frag` before tonemap. Gated by
`UNENDLICH_BLOOM`. Recommend slotting this in right after increment 1 so you get
a dramatic, visible result before the SSR math.

## Status

Shipped now (inert until wired, so no behavior change): `present.vert`,
`present.frag`, and the ruleset contract above. Next autonomous chunk: increment
1's Rust (depth sampleable, `offscreen.rs`, the two render passes, present
pipeline, gated render loop) + the `chunk.frag` conditional tonemap — then I come
back with it as one `POSTFX`-gated brick for a single A/B verification.
