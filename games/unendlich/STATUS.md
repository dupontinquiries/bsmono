# Project Status

This is an honest accounting of what's verified, what's been written but
not exercised on real hardware, and what's known to be incomplete.

Last refreshed: v0.20.0.

## Verified working

- **c16 rig session (2026-09-12):** engine build fixed (`begin()` referenced a
  `surface` it does not have); `Cargo.lock` caught up. New pure cores:
  `item_model` (hand + slot models), `anim_rig` (oriented per-part posing:
  hip-pivot legs that plant, breath/squash/stretch, tail chain, strike, settle,
  ragdoll blend), `sim::melee` (creature-vs-creature strike loop timed to the
  lunge), `sim::ragdoll_bridge` (`from_anatomy`, `sample_body_local`;
  `ragdoll.rs` is solver-only now). Bin: roamers render via
  `creature_to_vertices_rigged`; Dinosaurs BITE instead of an instant kill;
  deaths are ragdolls; rest beds down; arbiter `fatigue` is the rest urge.
  Harness: `harness/rig/run.sh` (rustc-only, real files by `#[path]`, 138
  tests) → `rig_viewer.html` (three.js). Verified by `cargo check` of lib and
  the fullgame bin (ui-overlay) in the sandbox; NOT verified running. Found:
  roamers never wrote `body.velocity`, so the old mesher's leg swing had
  amplitude 0 — that was the sliding.
- **c16 rig session, part 2:** legendary weapons — each of the 8
  `WeaponClass`es has its own model (`Greatsword`, `Greataxe`, `Wand` added)
  and every element's proc is an ability (`sim::weapon_ability`: frost nova,
  water jet, gale cut, stone spikes, flame wave, lightning arc) fired from
  the EQUIPPED weapon on hit, with burn/chill/stagger statuses ticking on
  roamers. Fauna hatch from the design layer (`spawn::FAUNA_ARCHES`:
  quadruped/critter/canid/bird/heavy), posed by the rig with real roles and
  sketch colours; forge dragons topple on death. Mobile Linux: `mobile_ui`
  detects phone/scale (Phosh etc., portrait framebuffers, env overrides),
  the mouse-emulated finger drives `touch_controls` (stick left, look
  right) with on-screen buttons bound to the real actions, HUD overlay.
  `examples/rig_dump.rs` runs the engine-coupled checks on the REAL lib
  (the sandbox builds `--lib`; only the test crate dies) and dumps every
  archetype for the viewer. All compile-checked (lib + fullgame bin); not
  run on a device.

* **Library compiles clean** under all four feature combos.
* **All 478 unit tests pass** (`cargo test --features render --features gpu --lib`).
  (472 pre-v0.20 + 6 indirect-draw + alignment tests.)
* **Cull shader validates** clean (`spirv-val OK`) and registered in
  `build.rs`.
  The test suite has grown organically with each increment; major
  groupings include `world::*` (worldgen, biomes, climate, noise,
  CPU generator), `world::gpu_chunk_pool` (slot lifecycle), `world::
  plant_lifecycle`, `world::cluster`, `keybindings`, `render::mesher`,
  `render::shadow_map`, and many more.
* **Release build clean** for `unendlich` lib with `--features render
  --features gpu` (`cargo build --release`).
* **All shaders compile clean** with `glslangValidator --target-env
  vulkan1.2` and pass `spirv-val`:
  * `chunk.vert`, `chunk.frag` — main draw, atmosphere, fog, shadows
  * `chunk_shadow.vert` — CSM shadow pass
  * `chunk_generation.comp` — GPU worldgen (in-tree, persistent-map
    fixes verified against Asahi Mesa pre-Honeykrisp; re-validation
    on Honeykrisp pending — see below)
  * `froxel_fill.comp` — volumetric fog froxel grid
  * `triangle.vert`, `triangle.frag` — sanity-check pipeline

## Known-good runtime paths

These have been exercised on hardware in playtest and shipped in
prior increments. See `docs/DESIGN_NOTES.md` for per-increment
architectural notes.

* **`unendlich-world` binary** — full game loop: GLFW window, Vulkan
  swapchain, async chunk streaming with priority queue, CPU mesher,
  CSM shadows, froxel volumetrics (cumulative integration as of
  v0.17.2), atmosphere shader, day/night cycle, plant lifecycle
  ticking with band-scheduling, tree growth + degrowth via tree
  POI cache.
* **`unendlich-triangle` binary** — minimal Vulkan render pipeline.
  Last verified on M2 Ultra Asahi (~4000 FPS, no validation errors,
  clean shutdown). Pre-Honeykrisp; re-verify on the current driver.
* **`unendlich-spin`, `unendlich-chunk`, `unendlich-palette`** —
  diagnostic / single-purpose bins, all compile and boot.
* **`unendlich-demo`** — headless world-gen pipeline exerciser,
  runs sub-millisecond per-tick on the main thread.
* **`world-bench`** — CPU/GPU generator parity benchmark; both
  backends produce byte-identical output.

## GPU compute path (`VulkanChunkGenerator`)

* **FP32 port complete (v0.18.1).** The compute shader was previously
  FP64; ported to FP32 in v0.18.1 to support drivers that don't expose
  `shaderFloat64` — including Honeykrisp (Apple GPUs), most mobile,
  and likely all near-term RISC-V SiC GPU IP. The world's visual
  character will change subtly at the same seed compared to v0.17.x;
  the user explicitly accepted this. See `docs/GPU_NOISE_STRATEGY.md`
  for the long-term plan if higher-precision GPU worldgen becomes
  needed.
* **Persistent-map architecture is in place** — block memory mapped
  once during `new()`, raw pointer cached, unmapped only in `Drop`.
  Reusable command buffer + fence. `submit_lock: Mutex<()>` for
  thread safety. `Drop` unmaps before destroying buffers. Pre-FP32-port
  field-tested on M2 Ultra Asahi (older Mesa).
* **Honeykrisp re-validation pending.** v0.18.0's playtest log showed
  `ERROR_FEATURE_NOT_PRESENT` from device creation; that's fixed in
  v0.18.1 (drop `shader_float64` request). Generator's full path on
  Honeykrisp now needs hardware verification.
* **Backend::Auto fallback** — when Vulkan fails for any reason at
  init, the generator logs a warning and degrades to the CPU
  generator without aborting.

## v0.18.0 GPU chunk pool

* **`world::gpu_chunk_pool::CpuShadowPool`** — pure-Rust slot
  lifecycle. 10 unit tests covering alloc/free, version-based handle
  invalidation, FIFO recycling, exhaustion, round-trip read/write,
  stress (1000-cycle no-leak).
* **`world::gpu_chunk_pool_vk::VulkanChunkPool`** — Vulkan-backed pool,
  persistent-mapped HOST_VISIBLE | HOST_COHERENT SSBO. Hardware
  validation gated behind `UNENDLICH_RUN_VK_TESTS=1`.

## v0.18.1 GPU mesher

* **`world::gpu_mesher_vk::VulkanMesher`** — naive face-culling mesher
  in compute. Reads from a `VulkanChunkPool` slot, writes vertex +
  index buffers via atomic counter reservation. Per-block flat colors
  + per-face brightness, byte-identical vertex layout to CPU mesher.
* **`src/shaders/chunk_mesh.comp`** — the kernel. SPIR-V validates,
  compiles clean against Vulkan 1.2 SDK target. FP32-only, no extensions.
* **NOT yet wired into `ChunkManager`.** Production meshing path is
  still the CPU mesher in `render::mesher`. Wiring is v0.18.3's work
  once v0.18.2's mesh pool is validated.

## v0.20.0 GPU-driven indirect drawing — new this increment

* **`render::chunk_indirect::IndirectDrawSet`** — chunk metadata SSBO
  (4096 slots), indirect commands buffer, count buffer, cull pipeline.
  Slot allocator + write-through to GPU's persistent map. 4 unit tests
  including pre-shift index correctness.
* **`src/shaders/chunk_cull.comp`** — frustum culling kernel. Reads
  metadata, atomic-emits visible chunks' draw commands. Mirrors the
  CPU `Frustum::aabb_visible` test exactly. Validates clean.
* **`render::context`** — opts in to `drawIndirectCount` Vulkan 1.2
  feature.
* **`render::mesh_buffer_pool`** — `MeshSlot` extended with
  `vert_base_aligned` for the indirect path's vertex-boundary
  alignment requirement. `alloc()` and `write_at()` now expose the
  alignment-aware offsets. 2 new tests confirm the alignment math.
* **`render::world_renderer`** — both render passes (shadow + main)
  rewritten: cull dispatch BEFORE render pass begins, single
  `cmd_draw_indexed_indirect_count` inside. Indices pre-shifted at
  upload time so all chunks share one vertex/index buffer binding.
* **THIS IS THE DRAW-CALL FIX.** v0.19 playtest showed
  `draw=900-1500ms` per second window with ~400 chunks and 6 Vulkan
  calls each. v0.20 reduces that to 3-4 calls per frame regardless of
  chunk count. Real perf improvement pending hardware playtest.

## Earlier history

* v0.18.0: GPU chunk pool foundation
* v0.18.1: FP32 noise port + GPU mesher
* v0.18.2: Mesh buffer pool (eliminates `vkAllocateMemory` storms)
* v0.18.3: Mesh slot cap correction
* v0.19.0: Mesh sub-allocator (replaces fixed slots)

## Written but not yet runtime-validated

* **v0.20.0 indirect drawing on real hardware.** All 478 tests pass
  including correctness tests for the alignment-shift math and shader
  size assertions, but the actual cull → draw flow has not been run
  against a Vulkan device. Real validation: run unendlich-world and
  observe whether (a) chunks render correctly and (b) frame times
  drop with chunk count rendered. Visual regressions to watch for:
  garbage geometry (alignment wrong → wrong vertices), missing chunks
  (cull shader rejecting too aggressively), or no terrain at all
  (barrier wrong → indirect-draw reads stale/zero commands).

## Written but not yet runtime-validated

* **`gpu_chunk_pool_vk::VulkanChunkPool` on real hardware.** Code
  compiles and the layout matches the existing GPU generator's
  output stride, but the alloc/upload/read round-trip has not been
  exercised against a live Vulkan device. The `UNENDLICH_RUN_VK_TESTS`
  gate is the path to do that; it's the natural smoke test before
  the v0.18.0.x generator rewiring.
* **GPU dispatch on Honeykrisp.** The generator's persistent-map
  pattern was validated on pre-Honeykrisp Asahi. Re-validation is
  filed and is the gate for v0.18.1 work.
  UPDATE (later session): re-validation found the answer was "don't,
  yet, by default" rather than "re-verify". 46 telemetry runs
  (events.jsonl) on an M2 Ultra with `GEN_BACKEND=auto` all had the
  GPU terrain generator on, and all had main-pass medians of 0.6-4s —
  the second frame of one run spent 217ms in the main pass with two
  chunks loaded, before there was anything to draw. The 2026-09-01
  perf bisect (docs/claude-notes/perfbisect-desktop-findings.md) had
  already measured why: the generator's record->submit->wait_for_
  fences batches sit on the SAME hardware queue the frame renders on
  (0.5-1.6s per batch), and the GPU path was 0.63-0.98x the CPU path
  regardless of the queue contention. The honeykrisp runtime profile
  has said `GEN_BACKEND=cpu` since the bisect, but `Backend::Auto`
  never consulted it — it was "use the GPU if a device exists", full
  stop. Fixed: `Backend::Auto` now reads the driver id via
  `VK_KHR_driver_properties` before creating a device
  (`world::gpu_generator::auto_policy`, a pure function keyed on the
  driver, not the queue shape) and declines on Honeykrisp with a
  reason and an override (`UNENDLICH_GEN_BACKEND=gpu` still forces
  it). Exposed to Settings as a three-way Auto/CPU/GPU control,
  live-togglable without a relaunch when a GPU generator already
  exists. Separately, the shader's own per-voxel cost was cut: the
  surface-biome scan (`biome_at_3d_gpu`) ran once per exposed voxel
  with a `y` constant for the whole chunk, carrying the shader's one
  expensive loop (up to 128 y-values of multi-octave 3D noise) far
  more often than needed; it is now cached once per column in shared
  memory. Verified bit-exact (`gen_parity`, new bin: GPU block-id hash
  unchanged before/after on lavapipe) — NOT yet verified for wall time
  on Honeykrisp itself. The number that would close this item: the
  generator's own per-batch log line with `UNENDLICH_GEN_BACKEND=gpu`,
  compared against the 0.5-1.6s above. If it's now tens of ms, the
  `auto_policy` decline can be lifted (it's a one-line change with its
  own tests) and this bug class is actually closed; if not, the
  generator's single-queue serialization is the next thing to fix,
  not just its per-voxel cost.

## Known limitations / TODO

* **No GPU mesher yet.** v0.18.1 work. The CPU mesher in `render::
  mesher` is the production path.
* **No GPU-side culling.** Filed; depends on GPU mesher landing
  first because culling is most useful when the meshed-data lifetime
  is GPU-resident.
* **No Wayland-native window backend.** The build disables GLFW's
  Wayland backend (`-DGLFW_BUILD_WAYLAND=OFF`). On X11 or XWayland
  it works fine. winit migration is filed for later.
* **No cross-build setup.** Currently only builds for the host
  triple. Filed for after v0.18 ships; targets are Asahi (aarch64-
  linux-gnu, dev), Intel x86_64-linux-gnu, RISC-V (riscv64gc-linux-
  gnu, future hardware), Android aarch64 (QCOM mobile, much later).
  Linux-only by deliberate scope.
* **STATUS.md was previously stale.** Rewritten in v0.18.0; will
  be updated alongside design-notes per increment going forward.

## Build dependencies for reproducing

```bash
# Rust 1.75 or newer (apt's rustc 1.75 works; rustup is preferred
# but optional)
sudo apt install rustc cargo
# or:
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Shader compiler (GLSL -> SPIR-V) and validator
sudo apt install glslang-tools spirv-tools

# GLFW build deps (render feature)
sudo apt install cmake \
  libxrandr-dev libxinerama-dev libxcursor-dev libxi-dev \
  libgl1-mesa-dev libxkbcommon-dev

# Vulkan runtime (only needed at run time, not at build time)
sudo apt install libvulkan1 mesa-vulkan-drivers
# Asahi Linux: Honeykrisp ICD ships with the Asahi Mesa packages
```

## Verification commands

```bash
# Quick build sanity, no GPU code
cargo check --no-default-features --lib

# Full feature matrix
cargo check --features render --features gpu --all-targets

# Full test suite (includes the new pool tests)
cargo test --features render --lib

# Vulkan-runtime pool tests (requires a working Vulkan ICD)
UNENDLICH_RUN_VK_TESTS=1 cargo test --features gpu vulkan_chunk_pool

# Release build
cargo build --release --features render --features gpu
```
