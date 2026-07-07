# Decoupling render from glfw — two approaches

## Why

`render = ["gpu", "window", "vk-sys"]` and `window = ["glfw"]`, so anything
building the render module pulls the `glfw` crate, whose `glfw-sys 4.0.0+3.3.5`
companion **compiles GLFW 3.3.5 from C via cmake** in its build script. That
build needs X11/Wayland dev headers and a working cmake invocation, which fails
in headless/CI/sandbox environments. Net effect: the full renderer can't be
compiled (let alone run) anywhere without a display toolchain — which blocks
headless validation of any render-side change.

The glfw use splits into exactly two concerns:

1. **Input / keybinds** — `input.rs`, `keybindings.rs` (`glfw::Key`,
   `glfw::MouseButton`, `glfw::Action`, `glfw::Modifiers`).
2. **Surface + swapchain** — `render/context.rs` holds `glfw::Glfw` /
   `glfw::Window` and creates the Vulkan surface via
   `glfw.get_required_instance_extensions()` + `glfwCreateWindowSurface`.
   (`world_renderer.rs` has a little window/present coupling too.)

`ash` itself is pure-Rust bindings that `dlopen` libvulkan at runtime — **no
cmake, no C build** — which is why the `gpu` feature compiles fine headless.
So the blocker is glfw specifically, not Vulkan.

Precedent already in tree: `render_meshref_shim.rs` is a glfw-free render shim
used for the headless mesh-parity tests. Both approaches below can crib from it.

## Approach A — `headless` offscreen feature (smaller, unblocks CI/validation)

Add a feature that builds the render module **without glfw at all**, rendering
into an offscreen `VkImage` instead of a window swapchain.

- Restructure the feature graph so `render` no longer *forces* `window`
  (e.g. `render-core` = the module minus surface/swapchain/input; `render` =
  `render-core` + `window`; new `headless` = `render-core` + offscreen target).
- `render/context.rs`: behind `headless`, skip surface creation entirely;
  allocate an offscreen color/depth target; the present path becomes a no-op
  (or an optional readback for screenshots/tests).
- `input.rs` / `keybindings.rs`: feature-gate or stub under `headless` (no
  device to read input from).
- **Keep the windowed glfw path byte-for-byte when `window` is enabled.**

Win: `cargo check --features render,headless` builds with **no `glfw-sys` in the
dep tree** (verify with `cargo tree`), so render-side changes can be compiled —
and run on lavapipe — without a display. Smaller blast radius; no new windowing
dep. Does NOT give you a real window.

## Approach B — glfw → winit + ash-window (full migration, keeps a window)

Replace the windowing backend with pure-Rust `winit` and create the Vulkan
surface with `ash-window` (or raw `ash` surface extensions).

- Cargo: drop the `glfw` dep; add `winit` + `raw-window-handle` + `ash-window`.
  Neither pulls a C build / cmake; winit `dlopen`s X11/Wayland at runtime, so it
  compiles headless and only needs a display to actually open a window.
- `render/context.rs`: `get_required_instance_extensions` →
  `ash_window::enumerate_required_extensions`; `glfwCreateWindowSurface` →
  `ash_window::create_surface`.
- Input: map winit `WindowEvent`/`KeyEvent` to the existing `input.rs` /
  `keybindings.rs` enums — easiest via a neutral keycode enum that both the old
  glfw path and the new winit path translate into, so keybindings.rs stops
  depending on `glfw::Key` directly.
- Keep window lifecycle + keybinds behaviourally equivalent.

Win: a real window with **no cmake/glfw-sys build**, and the renderer compiles
headless too (winit is build-clean without a display). Bigger change (input
translation + event-loop reshape); winit's ownership model (event loop owns the
window) differs from glfw's poll loop, so the bin's main loop needs adjusting.

## How they're being run

Two worker threads build A and B independently against the same base, so both
can be tested and compared. They are NOT mutually exclusive long-term — A is the
fast unblock for headless validation; B is the "actually move off glfw" play.
A third thread keeps developing the predictive/compressed upload queue
(`docs/PREDICTIVE_UPLOAD_QUEUE.md`) in parallel.

---
## r328: migration COMPLETE for the input core
The Approach-B neutral vocabulary (`input_codes::{Keycode, MouseBtn, KeyAction, InputEvent}`) is now
USED, not just defined. `input.rs`, `keybindings.rs`, and `keybind_ui.rs` no longer name a backend key
type — they consume `InputEvent`/`Keycode`/`MouseBtn`. glfw survives only as (1) the feature-gated
`glfw_conv` adapter in `input_codes.rs` and (2) the bin's event loop, which translates
`glfw::WindowEvent` into `InputEvent` at the boundary. Result: the whole input + keybind system compiles
and unit-tests with no glfw — `cargo test --lib --features "render-core egui"` runs `input::` and
`keybindings::` tests that were previously impossible to link. Swapping glfw for winit is now a matter of
finishing the (already-stubbed) `winit` conversion path and the bin event loop; nothing in the core
changes.
