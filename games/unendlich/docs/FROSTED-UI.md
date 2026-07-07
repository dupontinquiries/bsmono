# Frosted-UI passthrough — pass contract

How a UI panel drawn over the 3D world shows the **blurred** world behind it.
This documents the GPU pass and the egui hand-off so the engine wiring (step 3)
is unambiguous. Status: **shaders + kit consumption landed; render-graph wiring
is step 3.**

## Pipeline (per frame)

```
scene color (full res, after tonemap/post, before UI)
        │
        ▼  ui_downsample.comp        4-tap box, full → half res
   half[0]
        │
        ▼  ui_blur.comp  dir=(1,0)   9-tap Gaussian (horizontal)
   half[1]
        │
        ▼  ui_blur.comp  dir=(0,1)   9-tap Gaussian (vertical)
   blur   ──► registered as an egui TextureId ──► Surface::frosted_blur(tex, tint)
```

Downsampling first makes a small 9-tap Gaussian cover a wide screen radius for
near-free cost. egui samples the half-res `blur` with a linear sampler, so the
upsample is implicit (no extra pass).

## Resources

- `half[0]`, `half[1]`: two half-res `rgba8` storage images (ping-pong). Created
  at `swapchain_extent / 2`, **recreated on resize**.
- Source: the scene color image, read through a **linear** `sampler2D`.
- One linear sampler (clamp-to-edge).
- The `blur` target is `half[1]` (after the vertical pass).

## Dispatch contract (matches the shaders)

| pass | shader | bindings | push constants | dispatch |
|------|--------|----------|----------------|----------|
| downsample | `ui_downsample.comp` | 0: scene color (sampler2D), 1: `half[0]` (rgba8 image) | `ivec2 dst_size` | `ceil(half_w/8), ceil(half_h/8), 1` |
| blur H | `ui_blur.comp` | 0: `half[0]`, 1: `half[1]` | `ivec2 size, ivec2 dir=(1,0)` | same |
| blur V | `ui_blur.comp` | 0: `half[1]`, 1: `half[0]` | `ivec2 size, ivec2 dir=(0,1)` | same |

(Final blurred result ends in `half[0]` with the H→V order above; pick whichever
ping-pong end is the final write and register *that* one. Insert image memory
barriers between passes: each dst write → next src read is `SHADER_WRITE →
SHADER_READ`, `GENERAL`/`SHADER_READ_ONLY_OPTIMAL` as appropriate.)

Run the three dispatches **after** the scene color is resolved and **before** the
egui UI pass, so the texture is ready when egui records.

## egui hand-off

1. Register the final blur image with the egui-Vulkan integration to get a
   `TextureId` (e.g. the integration's `register_user_texture(view, sampler)`).
   The image view + sampler are stable across frames (only re-register on resize),
   so the `TextureId` can be cached.
2. Before drawing frosted UI: `unendlich_ui::set_surface(ctx,
   Surface::frosted_blur(tex, tint))`. A good `tint` is `PANEL` at ~0.62 alpha
   (`PANEL.gamma_multiply(0.62)`) — enough wash for text legibility, still reads
   as glass.
3. A frosted container that knows its rect calls
   `unendlich_ui::frosted_backdrop(ui, rect, corner)` **before** its `card`
   content; that samples `blur` at `rect / screen_rect` and lays the tint over it.
   `card`'s own fill is transparent under a textured frosted surface
   (`surface_fill`), so the backdrop shows through.

## Notes / open items for step 3

- **egui mount**: `items/debug_ui.rs` is written to take a `&mut egui::Ui` but the
  egui-on-Vulkan integration may not be wired yet. Standing that up (the render
  pass that draws egui's meshes + the texture registration above) is the bulk of
  step 3; the frosted box is small on top.
- **Rounded corners**: `frosted_backdrop` paints the blur into a square rect; the
  card's rounded border sits on top, so corners show a sliver of un-rounded blur.
  Acceptable for hello-world; clip to a rounded rect later if it reads poorly.
- **Dynamic-rect panels**: a fixed/anchored box knows its rect up front. For a
  panel whose size depends on content, paint the backdrop using the previous
  frame's rect (one-frame lag, imperceptible) rather than guessing.
