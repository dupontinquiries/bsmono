# Why the menu/overlay can't compile in the build sandbox (deferred)

## The blocker is windowing build-deps, NOT rendering
- `--features ui-overlay` pulls in `--features window` → `glfw-sys`, which builds
  native GLFW and links system display libs (X11/Wayland + GL dev headers) and
  needs `cmake`. The sandbox has none of these and the hosts aren't allow-listed
  to fetch them, so the bin/overlay can't even *build* here.
- **llvmpipe/lavapipe does NOT solve this.** lavapipe is a *software Vulkan*
  driver — it's what the headless GPU gate already uses to render + validate
  shaders in-sandbox. Rendering isn't the problem. The problem is the *window/
  build* dependency (glfw native build), which sits above Vulkan.

So today the menu *logic* is testable (now in `engine::app`), but the menu
*drawing* (egui through glfw) is only compilable on the dev machine.

## The real fix to explore later: decouple egui tessellation from glfw
egui is backend-agnostic — it tessellates UI state into meshes + a font atlas
independent of any window. If the overlay's egui pass rendered those meshes
through the **existing Vulkan harness to an offscreen image** (no glfw window),
then:
- the overlay would compile in-sandbox (no glfw-sys), and
- the menu could be *rendered headlessly* under lavapipe and pixel-checked
  (the descriptor_gate harness already renders offscreen).

Concretely: a `render-overlay-offscreen` feature that swaps the glfw surface for
an offscreen `VkImage` target and feeds egui's `ClippedPrimitive`s through the
harness. The glfw window stays a thin bin-only shell. This mirrors the app-core
split (testable logic in lib, un-testable window in the bin) at the render layer.

Deferred by request — documented so we can come back to it.
