# Cosmic top bar — status and TODOs

Base commit for this work: `cc9cbaa0b` ("voiceterm-egui: mount bar_pages...").
6 commits on top, bundled in `topbar-session.bundle` (see below). Apply with:

```
git bundle verify topbar-session.bundle
git fetch topbar-session.bundle HEAD:topbar-work   # or: git pull topbar-session.bundle HEAD
```

or, if this repo's `cc9cbaa0b` isn't present locally, `git bundle` will refuse —
in that case use the six `.patch` files instead (`git am *.patch` in commit order).

## What exists, and where

- `shared/systemcompkit` — protocol tracking + the one real Wayland-facing
  layer (`wayland_io`, feature-gated, off by default).
  - `layer.rs`: `LayerSurfaceIntent` with `top_panel`/`bottom_panel`/`side_panel`
    constructors (laptop top bar / desktop bottom bar / mobile sidebar shapes).
  - `wayland_io.rs`: `LayerSurface` — connects, requests a layer-shell surface,
    handles configure/ack/closed. `request_resize()` does a live resize on an
    already-mapped surface (no destroy/recreate). `paint_argb8888()` is a
    **pure-Rust, zero-C-library** CPU paint path via `wl_shm` — verified with
    `ldd` to link no `libwayland-*.so` when built without wayland-client's
    `system` feature.
  - Tests: 27, all pure (no compositor needed). `cargo test --features wayland_io`
    also compiles the real Wayland code path but nothing there runs against a
    live compositor — there's no sandbox with one.

- `shared/topbar-core` — headless bar state, no egui, no wayland-client in
  its default build.
  - `tile.rs`: `unify()` merges favorited launcher entries + running windows
    into one keyed `TileItem` list (a pinned-and-running app is one tile).
  - `category.rs`: `TileCategory` (Launcher/Taskbar/SystemMonitor — the last
    has no producer yet).
  - `placement.rs`: `Placement` (LaptopTopBar/MobileThumbSidebar/DesktopBottomBar)
    — form factor → layer intent + which categories it shows.
  - `quality.rs`: `RenderTier`/`RenderQuality` — hardware vs software-emulated
    vs no-GPU, and the two things that get turned off on the weak tiers
    (animation, translucency).
  - `bar.rs`: `Bar` — collapsed/expanded + the unified `hatch_core::anim::Board`
    for tile motion (reused unmodified from `hatch`).
  - `runtime.rs`: `Runtime<S: BarSurface>` — orchestrates surface + Bar +
    Placement + Placement-filtering + resize-on-toggle. `BarSurface` is the
    transport seam; `systemcompkit::wayland_io::LayerSurface` implements it
    behind this crate's own `wayland_io` feature.
  - Tests: 19, all against a fake surface, no compositor needed.

- `shared/topbar-render` — the GPU rendering path: egui-on-Vulkan via `wgpu`,
  raw-window-handle straight onto the layer-shell `WlSurface`, no winit.
  - `surface.rs`: `VulkanSurface::new()` — hardware adapter first, falls back
    to `force_fallback_adapter` (lavapipe) before giving up; classifies
    whichever adapter answered into a `RenderTier` via `AdapterInfo::device_type`.
  - Tests: 2, pure (constructed `AdapterInfo` fixtures, no live adapter).
  - Requires wayland-client's `"system"` feature (real `libwayland-client.so`
    link) — this is the one crate in the stack that actually needs it, and it
    doesn't spread to the others (confirmed: `systemcompkit`'s own default
    build stays pure-Rust regardless of what `topbar-render` links, since
    there's no root Cargo workspace unifying features across these
    independently-built crates).

None of these four crates share a root workspace `Cargo.toml` — each is built
from its own directory. That's deliberate (matches the rest of this monorepo)
but means `cargo test`/`cargo build` needs to run once per crate directory,
not once at a repo root.

## Real, load-bearing findings from this session (not just code)

- **VK_KHR_wayland_surface needs `libwayland-client.so` regardless of language.**
  Confirmed via ARM's own open-source Vulkan WSI layer listing it as a hard
  build dependency. Choosing Vulkan over GL doesn't avoid the C library, it
  just relocates who links it (the driver, not necessarily your own code) —
  but `topbar-render` links it directly too, since `raw-window-handle` needs
  a real `wl_display*`/`wl_proxy*`, which only exists via wayland-client's
  `"system"` (libwayland) backend, not its default pure-Rust one.
- **Lima (PinePhone's Mali-400/450) has no Vulkan driver and never will** —
  documented as a hardware limitation, not a maturity gap. Not currently
  acted on further since PinePhone isn't actually owned/targeted hardware;
  Fairphone 5's Adreno 690 (Turnip) has real hardware Vulkan, so Vulkan
  stays the primary path. Recorded here in case a Lima-class device becomes
  a real target later — `RenderTier`/`paint_argb8888` are the fallback route
  already built for that case.
- **`wl_shm` needs none of the above.** It's plain wire protocol (an fd
  passed over the same socket everything else here already uses) — the
  actual "stay in pure Rust, work identically on every device regardless of
  GPU driver" path, verified with `ldd` rather than assumed.

## TODOs, roughly in order of what blocks what

1. **No `BarPainter` trait / no paint loop.** `Runtime` orchestrates
   surface+state but nothing turns `Bar::poses()` into pixels yet on either
   path (`VulkanSurface` via `egui_wgpu::Renderer`, or `paint_argb8888` via a
   hand-rolled software rasterizer for whichever primitives a bar actually
   needs — rects, text, icons). This is the next concrete blocker for
   anything appearing on a real screen.
2. **No app binary.** All four crates are libraries; nothing has a `main()`
   that connects, ticks a `Runtime`, and paints a frame.
3. **Live-resize is unexercised.** `request_resize()`/`VulkanSurface::reconfigure()`
   compile against the real APIs but have never run against a live compositor.
4. **The notch.** `Placement::LaptopTopBar`'s doc flags this directly: no
   protocol here surfaces real display-cutout geometry, so "notch-sharing"
   is currently just an ordinary rectangular top bar. Needs a real geometry
   source before it can be anything else — not attempted.
5. **`TileCategory::SystemMonitor` has no producer.** `Placement::DesktopBottomBar`
   already declares it visible; nothing generates a tile for it yet (CPU/mem/
   battery/network readouts).
6. **Buffer reuse.** `paint_argb8888` allocates a fresh `tempfile`+`wl_shm_pool`
   +`wl_buffer` on every call — correct (the `Release` handler destroys each
   buffer once the compositor's done with it) but wasteful for a real
   per-frame loop. Double-buffering is real, undone follow-up work.
7. **The plasma-mobile "surface file not found" report** (musl-related per a
   secondhand read) is noted but not investigated — no reproduction, no
   access to that system from here. Worth checking whether it's a
   dynamic-linker (`dlopen` search path) mismatch between musl and glibc for
   `libwayland-client.so`, given how much of this session turned out to hinge
   on exactly which Wayland backend/linking mode is in play.

## Verification ceiling, stated plainly

Nothing in this codebase has run against a real Wayland compositor or real
GPU hardware. Everything marked "compiles clean" or "tested" in the commit
messages means: real dependency versions, real API shapes (several wrong
guesses were caught and fixed against the actual compiler/toolchain, not
memory), and — where the code is pure — real unit tests. It does not mean
verified on a phone. That remains the actual next milestone once a paint
loop exists to try it with.
