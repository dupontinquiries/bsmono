# Headless GPU in the sandbox — WORKING (r64)

The window benches no longer need a window. `VulkanContext::new_headless()` (feature
`headless`) brings up a graphics+compute Vulkan context with **no GLFW, no surface, no
display**, picks the device on capability only, and renders to an offscreen image
(`create_offscreen_target`). Proven on lavapipe in CI/sandbox.

Run:
```
export VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json   # lavapipe
cargo run --release --features headless --bin gpu-headless-probe
```
What it does + sandbox readings (llvmpipe / Mesa 25.2.8):
- headless context up ~58 ms; offscreen 256²/512²/1024² alloc sub-ms.
- **real GPU meshing** via `SparseGpuMesher::mesh_chunk` on generated chunks:
  ~500–1300 us/chunk steady-state (first chunk ~88 ms = pipeline/shader warmup),
  ~6–8k quads/chunk. This is the same path `gpu-mesh-bench` runs, headless.

Why this matters:
- Sandbox-side GPU meshing perf is now measurable (no M2 needed for a first read).
- `gpu-mesh-bench` / `unendlich-spin` can grow a `--headless` arm that swaps
  `VulkanContext::new(&glfw,&window)` for `new_headless()` — the rest (mesher, dispatch,
  readback) is window-independent. The bin's `required-features` would broaden from
  `["render","gpu","window"]` to also build under `["headless"]`.
- Logs from these feed `unendlich-analyze`, and the r62 perf-harness matrix can track
  GPU meshing us/chunk alongside the CPU `perf-probe` stages.

Next steps (not yet done): a real headless draw pass (pipeline → render to offscreen →
readback) for frame-time numbers, and the `--headless` arm on gpu-mesh-bench/spin so the
existing benches double as CI GPU benches.

## Shader micro-bench attempt (r66) — partial

The headless probe now also times per-shader SPIR-V **module ingest** (safe, generic):
chunk_generation ~45us, chunk_mesh_octree ~25us, chunk.frag ~14us, etc. But the useful
metric — compute **pipeline build** (the driver's real shader compile) — needs a pipeline
layout matching each shader's descriptor sets. Building with an empty/mismatched layout
**segfaults lavapipe** (not a graceful error). So a real shader relative-perf micro-bench
requires per-shader layout reflection (parse SPIR-V descriptor bindings → build the matching
layout → time pipeline creation + a dispatch). That's a dedicated thread, not a quick shot —
the headless foundation is ready for it; the layout reflection is the missing piece.
