# r63 — world-loop cleanup, surfaced dark code, headless-GPU diagnosis

## Edits made to `crates/unendlich-engine/src/bin/unendlich_world.rs` (compile on your end)

1. **Giant creatures removed.** The `test_creatures` vec spawned an oversized demo Dragon
   plus three Späher at spawn ("its large body clears the ground"). Now an **empty vec** —
   real creatures come only from the streamed `SpawnStreamer` ecosystem. The loop that
   renders/updates `test_creatures` is unchanged (iterates an empty vec = no-op).

2. **Sentinel flock removed.** The `[Alarmed, Calm, Alarmed]` test flock was three static
   Späher with `SentinelAgent`s. `sentinel_flock` is now an **empty `Flock::new()`**, so the
   per-frame `tick` + the `Sentinel flock postures: {:?}` readout are no-ops (the readout now
   prints `[]`). Re-enable by adding `SentinelAgent`s back.

3. **Block-id placement enabled.** Right-click placement was hardcoded to `BlockId::Stone`.
   Added a `PLACE_PALETTE: [BlockId; 9]` (Stone, Dirt, Cobblestone, OakLog, OakLeaves, Sand,
   Glass, OakPlanks, Water) and a `selected_block` selected by the existing **Hotbar1..9**
   keybinds (number keys). Placement now uses `selected_block`; the log names the block placed.

These three are safe gating/additive edits (no loop-structure changes), but the bin can't be
compiled in-sandbox (glfw/winit), so verify the build on your toolchain.

## Surfaced dark / test code in the loop (your call — NOT removed)

- **Test Critters** (`entity_store.spawn(EntityKind::Critter, …)` ×3 at spawn, ~line 480):
  demo mobs "so mob rendering is visible." Small, not giant — left in. Remove if you want a
  clean spawn.
- **Test sim-mobs Seeker/Grazer** (`test_mobs`, ~line 527; rendered via `mobs_to_vertices`
  ~line 3570): demo predator/prey exercising the sim→render bridge. Left in; same call.
- **`#[allow(dead_code)]`** at ~line 4293: a dead helper kept around — candidate for deletion.
- Env knobs (`UNENDLICH_*`: PROFILE, GPU_GEN_BATCH, NO_FOG, WARN_SNIPE, WORKER_ISOLATION,
  upload budgets) are legitimate runtime tunables, not dark code — left as-is.
- "for now" hacks worth a follow-up: cross-chunk seam re-mesh on edit (~line 2412 region)
  drops neighbors from in-flight rather than version-bumping them; hardcoded inventory was the
  Stone-place (now fixed via the palette).

## Headless GPU benches (`gpu-mesh-bench`, `unendlich-spin`, `unendlich-chunk`) — why faking a window doesn't unblock me, and what does

`gpu-mesh-bench` already opens a **hidden** GLFW window (`Visible(false)`, `NoApi`) solely to
hand `VulkanContext` a surface. The blockers in the build sandbox are:
1. **No display server** — GLFW `create_window` fails outright (no X11/Wayland), so even a
   hidden, zero-size window can't be created. Faking the *size* doesn't help; *creation* fails.
2. **`VulkanContext` requires a surface/swapchain** — it picks a present queue and builds a
   swapchain, both of which need a surface.

The real unblock is a **headless Vulkan path**: create the instance+device WITHOUT
`VK_KHR_surface`, render to an offscreen `VkImage` (no swapchain), and skip present. lavapipe
(the sandbox's software ICD, already proven by GPU GATE GREEN) supports surfaceless rendering.
With a `VulkanContext::headless()` + a `--headless` flag on `gpu-mesh-bench`/`unendlich-spin`
(drop GLFW, render to an image, time the passes), these would run in CI/sandbox on lavapipe and
emit the same diagnostic events that `unendlich-analyze` (the `-analyze`/diag tool) consumes —
closing the loop so GPU perf is measurable here, not only on the M2. This is a bounded render-
layer change (compiles under `render`, no `window`), and is the right next infra thread if you
want sandbox-side GPU numbers. Until then: CPU perf via `perf-probe` (sandbox), GPU perf via the
window benches on your M2 + `unendlich-analyze` on the captured logs.
