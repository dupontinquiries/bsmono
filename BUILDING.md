# Building `shell`

## Status: compiles clean

As of this update, `cargo check -p shell` and `cargo build -p shell` both
succeed with **zero warnings**, using rustc/cargo 1.96.0. `cargo test
--workspace` passes (56 tests, all in `core` — `shell` has no unit tests of
its own yet). This was verified by actually installing a real toolchain and
running the compiler — not inferred.

Running the binary in a headless sandbox (no Wayland/X11 display, no D-Bus
session) fails as expected, cleanly, with no panics:

```
gyro task exited: I/O error: No such file or directory (os error 2)
Error: WinitEventLoop(Os(OsError { ... "neither WAYLAND_DISPLAY nor
WAYLAND_SOCKET nor DISPLAY is set." }))
```

Both failures are the OS/environment boundary, not a code bug — the gyro
thread degrades gracefully (prints and continues, doesn't crash the app) when
there's no D-Bus session, and winit correctly refuses to open a window with
nowhere to put one. On a real desktop or the Fairphone with a display and
`iio-sensor-proxy` running, both of these should resolve.

## What was actually wrong before this (fixed)

The previous version of this code had never been compiled and had four real
bugs, all now fixed:

1. **The workspace's own library crate was named `core`.** This shadows
   Rust's actual `core` sysroot crate for any macro that generates code
   referencing it — `bytemuck`'s `#[derive(Pod)]` broke with `cannot find
   mem in core`. Renamed the package to `road_warrior_core` (the directory
   is still `core/`, just the crate name changed) — **don't name your own
   crate `core`, `std`, or `alloc`**, even though nothing stops you from trying.
2. `egui_wgpu` and `egui` need to come from `eframe::egui_wgpu` /
   `eframe::egui` (eframe re-exports both) rather than being used as bare
   crate names — `main.rs` was missing that import.
3. `PaintCallbackInfo` lives in `epaint`, re-exported via `egui`, **not**
   via `egui_wgpu` — I'd guessed the wrong module.
4. `eframe::run_native`'s `AppCreator` closure in 0.27 returns `Box<dyn
   App>` directly, not `Result<Box<dyn App>, _>` — that `Result`-wrapping
   API is a newer eframe version I'd apparently pattern-matched from memory
   without checking.

None of these were guessable from documentation alone in the way I found
them — (1) especially only shows up when you actually try to compile,
since nothing about the type signatures looks wrong until a derive macro
resolves the wrong `core`.

## Prerequisites for running (not just compiling) on real hardware

- Wayland or X11 session (a headless server/CI box won't run this, same as
  any GUI app).
- Vulkan loader (`libvulkan1` / `mesa-vulkan-drivers`) for wgpu's backend.
- `iio-sensor-proxy` running for gyro input
  (`systemctl status iio-sensor-proxy`). Confirm the D-Bus property names
  match what `gyro.rs` expects with:
  `busctl --system introspect net.hadess.SensorProxy /net/hadess/SensorProxy`
  — the interface has been stable for years but this was never checked
  against a real instance.

## What's still unverified

Compiling clean means the types line up and the borrow checker is happy —
it says nothing about whether the wgpu pipeline in `render.rs` actually
renders correctly (right blend state, correct fullscreen-triangle math,
etc.) or whether the gyro axis mapping in `gyro.rs` matches how you'll
actually hold the phone. Both need a real display and a real device to find out.

## Release build

`cargo build --release -p shell` succeeds — zero errors, zero warnings, 16.8MB
optimized binary. Confirmed by actually running it under Xvfb: behaves
identically to debug, and the two debug-only env vars (`RW_DEBUG_WEAK_BOSS`,
`RW_DEBUG_LOW_HP`) are structurally inert in release — they're gated behind
`cfg!(debug_assertions)`, which is `false` in a release build regardless of
what's set in the environment. Verified by setting both explicitly and
confirming the boss stayed at full health and HP took normal (not forced)
damage.

## Cargo.lock is included

Deliberately committed (unusual for a pure library, but standard practice for
a binary crate like `shell`): it pins the exact dependency versions actually
built and verified this session — eframe 0.27.2, egui-wgpu 0.27.2, wgpu
0.19.4, zbus 3.15.2, tokio 1.53.1. Building without it would let Cargo
resolve newer compatible patch versions, which could reintroduce bugs like
the WGSL `vec3` uniform-alignment issue already found and fixed here, or
surface new ones I haven't seen. Delete it and re-resolve only if you
specifically want newer patches and are prepared to re-verify.
