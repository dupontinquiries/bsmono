# Frosted-Glass Blur Backdrop — Integration Notes (r230)

> Part of the menu/sim work — see `MENU_SYSTEM.md` for the overview + status.


The UI side is ready: `Surface::Frosted { blur: Some(tex), tint }` +
`frosted_backdrop` paint a blurred-scene slice behind a panel plus the
edge-weighted glow. The missing piece is producing a blurred full-scene
image and handing egui its `TextureId`. r230 builds the BRIDGE; the
scene-blur pass itself is the remaining render work.

## Done in r230 (compile-verified)
`EguiOverlay::register_external_texture(vk, slot, view, sampler) ->
TextureId` (render/egui_overlay.rs): allocates a descriptor set pointing at
an ENGINE-OWNED image view+sampler and returns `TextureId::User(slot)`.
- The image/memory/view are NOT owned by egui — `EguiTexture { external:
  true }` makes cleanup free only the descriptor set.
- The view must be in `SHADER_READ_ONLY_OPTIMAL` when the egui pass runs.
- Re-call with the same `slot` after a swapchain resize to repoint at the
  new view (old set is freed).

## Remaining work (needs a GPU/runtime test loop)
1. **Scene-blur target.** Produce a blurred copy of the full scene in a
   SHADER_READ_ONLY image. The engine already has the pattern + shaders:
   - `render/offscreen.rs` `OffscreenTarget` = image+memory+view+sampler in
     the scene format (half-res is fine — frosted blur wants softness).
   - `render/bloom.rs` already runs a SEPARABLE GAUSSIAN (ping-pong half-res,
     `assets/shaders/bloom_blur.frag.spv`); `Bloom::write_scene(view,
     sampler)` sets the source and `Bloom::view()` returns the blurred
     result. Two options:
     a. **Cheapest:** reuse bloom's blur pipeline but feed it the SCENE
        (not the bright-pass) into a dedicated half-res target — i.e. a
        second instance of the blur ping-pong seeded from the offscreen
        scene view. Output already lands in SHADER_READ_ONLY.
     b. A small standalone downsample+blur pass over the offscreen scene.
   - Run it only when a menu/overlay is open (no cost in pure gameplay).
2. **Register once + on resize.** After creating the blur target, call
   `overlay.register_external_texture(vk, BLUR_SLOT, blur_view,
   blur_sampler)` and stash the returned `TextureId`. Re-register on
   swapchain recreate (the view changes).
3. **Feed the menu draw.** In world_renderer.rs, where r228 sets
   `Surface::frosted()`, switch to
   `Surface::frosted_blur(blur_tex, PANEL.gamma_multiply(0.74))` when the
   blur target exists. `frosted_backdrop(ui, rect, corner)` then samples it
   behind any panel that calls it (the menu windows use `glass_at` frames
   today; either add a `frosted_backdrop` call with the window's rect, or
   migrate a window to draw its own chrome via `menu::glass_panel` which
   reads the surface).

## Ordering / hazards to verify at runtime
- The blur pass must finish (and its target transition to SHADER_READ_ONLY)
  BEFORE `egui_overlay.record` samples it, within the same frame. egui's
  `prepare` runs pre-pass (uploads), `record` runs inside the main pass —
  the blur must be recorded before the egui draw in that pass, or in a
  prior pass with a barrier.
- Sample the blur with the panel's screen-rect → UV mapping that
  `frosted_backdrop` already computes (`rect / content_rect`).
- The scene at menu time is the menu-world orbit backdrop (title) or the
  frozen world (pause) — both are real color targets, so the blur reads
  fine.

## How to test
Build + run the game (`tools/run-latest.sh`, features render,window,gpu,
ui-overlay,audio). With the scene-blur wired: open the pause/title menu —
the panels should show a softly blurred version of the scene behind them
with a bright edge glow, instead of the flat translucent wash. If panels
go black/garbage behind, the blur target layout or the prepare/record
ordering is off (see hazards above).
