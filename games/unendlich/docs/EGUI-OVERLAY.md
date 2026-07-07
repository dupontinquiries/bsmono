# egui overlay + in-game frosted dialog — integration contract

How the kit's egui UI (and the frosted blur) gets drawn over the 3D world.
Status: **deps + feature wired (`ui-overlay`, off by default); renderer is the
next implementation work.** This is the spec it's built against.

## Decision: hand-roll, don't pull `egui-ash-renderer`

| | engine / kit | `egui-ash-renderer` (all maintained vers) |
|---|---|---|
| ash | **0.37** | **0.38** |
| egui | **0.34** (kit + dashboard) | **0.33** (`^0.33`, excludes 0.34) |

Using the crate would force *two* migrations — bump ash across the 2700-line
renderer **and** downgrade egui across kit + dashboard. We hand-roll a minimal
egui renderer for **egui 0.34 + ash 0.37** instead: no migrations, same versions
everything already uses, and consistent with the engine's from-scratch raw-ash
approach. Gated behind the `ui-overlay` feature (pulls `egui` + `unendlich-ui`),
off by default so the normal build is untouched.

## Module: `render/egui_overlay.rs` (new, `#[cfg(feature = "ui-overlay")]`)

Owns: a persistent `egui::Context`; the Vulkan resources (one graphics pipeline,
grown-as-needed host-visible vertex/index buffers, a linear sampler, a descriptor
set per egui texture); and per-frame paint.

Per frame:
1. **Input** — build `egui::RawInput`: `screen_rect` from the framebuffer extent,
   `pixels_per_point` from window content scale, and pointer/scroll/key events
   translated from GLFW `WindowEvent`s (the same stream `src/input.rs` handles —
   see mapping below). For the hello-world box, pointer pos + buttons + screen
   rect + dt suffice.
2. **Run** — `let out = ctx.run(raw, |ctx| build_ui(ctx))`. `build_ui` is where we
   `set_surface(Surface::frosted_blur(blur_tex, tint))` and draw the box.
3. **Tessellate** — `let prims = ctx.tessellate(out.shapes, out.pixels_per_point)`
   → `Vec<ClippedPrimitive>` (each = a `Mesh` { vertices, indices, texture_id } +
   a clip `Rect`).
4. **Textures** — apply `out.textures_delta`: each `(TextureId, ImageDelta)` →
   create/patch an RGBA8 `vk::Image` + view + descriptor. The font atlas is a
   *managed* id; the blur target is a *user* id we register once (re-register on
   resize). Free ids in `textures_delta.free`.
5. **Upload + draw** — copy all prim vertices/indices into the dynamic buffers;
   then for each prim: `cmd_set_scissor` = `clip_rect * ppp` clamped to the
   framebuffer, bind the prim's texture descriptor, `cmd_draw_indexed` its range.

egui vertex = `{ pos: [f32;2] (points), uv: [f32;2], color: [u8;4] srgba }`. A
push constant carries `screen_size_in_points`; `egui.vert` maps points → NDC,
`egui.frag` samples the texture × vertex color (egui's sRGB convention). Both
GLSL files go in `src/shaders/` + `build.rs` SHADERS, like the blur shaders.

## Frame flow / hook points (`draw_frame_with_view_and_atmos`)

World still renders into the swapchain; the main pass ends at
`world_renderer.rs:3403`, submit/present at 3431/3448. Compute must run **outside**
a render pass and egui draws **inside** one, so:

```
... world main pass ...                       (ends ~3403)
copy swapchain color  → sampled image C        (needs swapchain TRANSFER_SRC usage)
ui_downsample.comp     C → half[0]             ┐ blur (compute, no render pass)
ui_blur.comp dir=(1,0) half[0] → half[1]       │ → final blurred = B
ui_blur.comp dir=(0,1) half[1] → half[0] (=B)  ┘   (image barriers between)
begin UI render pass on the swapchain (LOAD, keep the world)
  egui_overlay.paint(cmd, ...)                 (frosted panels sample B)
end UI render pass
submit + present                               (3431 / 3448, unchanged)
```

**Blur source.** First cut: copy swapchain → sampled `C` after the world pass
(requires adding `TRANSFER_SRC` to swapchain image usage in `swapchain.rs`, and a
layout dance: present→transfer-src→sample). Cleaner long-term: render the world to
an offscreen color target, blit it to the swapchain as the base, and let frosted
panels sample its blurred copy — but that restructures the world pass, so it's
deferred.

## GLFW `WindowEvent` → `egui::Event`

| GLFW | egui |
|------|------|
| `CursorPos(x,y)` | `PointerMoved(pos / ppp)` |
| `MouseButton(b, action, mods)` | `PointerButton { pos, button, pressed, modifiers }` |
| `Scroll(dx,dy)` | `MouseWheel` / `Scroll` |
| `Key`, `Char` | `Key`, `Text` (deferred; box needs none) |
| framebuffer size | `screen_rect = (0,0)..(w,h)/ppp` |

Tap the same stream `input.rs::handle_event` consumes (pass each `WindowEvent` to
the overlay too), or read `InputState`'s cursor/button snapshot for a
pointer-only first cut.

## The hello-world box

In `build_ui`: `set_surface(frosted_blur(B, PANEL.gamma_multiply(0.62)))`, then an
`egui::Area` anchored center (or at the player's projected screen position via the
camera view-projection) with a fixed rect; call
`unendlich_ui::frosted_backdrop(ui, area_rect, r(12))` then a `card`/`dialog` with
"hello, world." Fixed rect = no chicken-and-egg for the backdrop.

## Implementation order (next turns, each build-and-screenshot)

1. `egui.vert`/`egui.frag` + the renderer module compiling with an empty UI.
2. Input mapping + a plain (Solid) test panel over the world — proves egui draws.
3. Swapchain `TRANSFER_SRC` + the copy + 3 blur dispatches + the frosted box.
4. Anchor the box to the player's projected screen position.

## Open risks (the build-loop will surface these)

- The hand-rolled renderer is the genuinely unverifiable-blind part: font-atlas
  upload format, sRGB handling, scissor/clip math, buffer growth.
- Swapchain `TRANSFER_SRC` usage + the copy's layout transitions.
- `egui_debug` now compiles when enabled; `debug_ui.rs` may need egui-0.34 nudges.

## Step 2 status (this turn): wired into unendlich-world

`paint()` was split into **`prepare`** (pre-pass: run egui, upload font atlas — a
queue submit, so it must be outside any render pass — tessellate, fill V/I
buffers, stash a `Vec<DrawCmd>` in `self.pending`) and **`record`** (in-pass: pure
command recording of the stashed draws — safe between begin/end render pass). The
egui pipeline also gained a disabled depth-stencil state (test/write off, compare
ALWAYS) so it is subpass-0 compatible with the world's depth-bearing main pass.

`WorldRenderer` (all `#[cfg(feature="ui-overlay")]`): owns an `egui_overlay`
field, created in `new()` against the main `render_pass`; `draw_frame_..` calls
`prepare(..)` early (after `process_pending_destroys`, before the command buffer
begins) with a minimal `RawInput { screen_rect, .. }` and a `build_ui` that draws
`unendlich_ui::card` in a fixed `Area`; `record(cmd)` runs just before
`cmd_end_render_pass`; `destroy()` tears it down first under `device_wait_idle`.
The existing `unsafe impl Send for WorldRenderer` covers the overlay's `*mut u8`.

### RESOLVED — per-FIF vertex/index buffer ring
The overlay now holds a `Vec<FrameBuffers>` (one vertex+index pair per
frame-in-flight slot) instead of a single shared pair. `EguiOverlay::new` takes
the FIF count; `prepare(slot, …)` writes ring slot `slot` and `record` replays
from it. The renderer passes `frame_idx = current_frame` and — critically — calls
`prepare` AFTER `wait_for_fences(frames[frame_idx].in_flight)`, so the previous
GPU use of that slot is guaranteed complete before the buffer is overwritten. No
more write-while-in-use; dynamic/animated UI is now safe.
