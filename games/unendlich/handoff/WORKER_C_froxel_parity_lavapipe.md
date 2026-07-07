# Worker thread C — froxel-fog parity test on llvm-pipe (vk-harness)

You are one of three parallel workers on **unendlich**, a Rust voxel/dragon game
engine (Vulkan, Linux/Asahi, FP32). A coordinator portioned you this task from
the project docs. Work only from this brief and the docs it cites.

## Your environment (this OVERRIDES the stale "you cannot compile" note in `handoff/SESSION_HANDOFF.md §0`)
- You **can compile and run.** `cargo` + `rustc` 1.75, `glslangValidator`+`spirv-val`
  are installed.
- You have **llvm-pipe / lavapipe** (Mesa's software Vulkan device) — this is the
  point of your task: `vk-harness` runs **real shader↔CPU parity on the GPU path
  without hardware**. You also have the **test harnesses** and the **docs**.
- Repo root is `unendlich/`; it is the ground truth.
- **Cargo.lock is v4** (needs cargo ≥1.78; you have 1.75). Reversible test runs:
  `cp Cargo.lock /tmp/lock.v4; rm -f Cargo.lock; <run>; cp /tmp/lock.v4 Cargo.lock`.
  Restore the v4 lock before handing back.
- Stay in scope. If blocked or ambiguous, stop and report back — don't guess.

## Read first
- `docs/RENDER-PARITY.md` — the CPU-oracle-vs-GPU philosophy.
- `handoff/GPU_HARNESS.md` — how the GPU harness works.
- `crates/vk-harness/BENCH.md` — the **exact lavapipe invocation**
  (`VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json cargo test -p vk-harness …`).
- Existing tests as templates: `crates/vk-harness/tests/compute.rs` (compute →
  SSBO readback), `mesh_parity.rs` / `octree_parity.rs` (CPU oracle vs GPU),
  `chunk_shaders.rs` (inline GLSL + pixel asserts). Reuse `vk_harness`'s helpers.
- The shader under test: `crates/unendlich-engine/src/shaders/froxel_fill.comp`.
  Recently changed: (a) wind-advected **blobs→streaks** moisture, (b) an
  **empty-froxel optical-depth early-out** (`OD_EPS=7e-5`). **There is no parity
  test for it.** You add one.

## Lavapipe sanity (do this first)
```
ls /usr/share/vulkan/icd.d/lvp_icd.json 2>/dev/null \
  || find / -name 'lvp_icd.json' 2>/dev/null \
  || (apt-get update && apt-get install -y mesa-vulkan-drivers)
```
Use the path you find as `VK_ICD_FILENAMES`. Confirm an existing test runs first,
e.g. `VK_ICD_FILENAMES=<path> cargo test -p vk-harness --test compute -- --nocapture`
(with the lock workaround). If lavapipe genuinely isn't available, stop and
report that as the blocker.

## Task — add `crates/vk-harness/tests/froxel_parity.rs`
Validate the froxel fill's **deterministic density half** (`cell_density` — the
`hash13`/`value_noise_3d`/`fbm2` + altitude/height-fog/rolloff + wind advect +
shear-stretch + blob `smoothstep` + proximity, clamped to 2.0) on the GPU vs a
CPU oracle:
1. **CPU oracle:** port `cell_density` into the test in **f32**, copied exactly
   from `froxel_fill.comp` (same constants: `MOISTURE_FREQ=0.05`,
   `STREAK_GAIN=2.0`, `STREAK_MAX=5.0`, blob `smoothstep(0.15,0.85,·)`).
2. **GPU side:** inline a small `.comp` (à la `compute.rs`) containing the
   **same** `hash13/value_noise_3d/fbm2/cell_density` code, fed an SSBO of query
   `vec4` world positions + a tiny uniform/push for `camera_pos`, `wind_xy`,
   `time`, `humidity`, `base_density`; write one density per invocation to an
   output SSBO. (Inlining the function under test is the robust path; dispatching
   the whole `froxel_fill` and reading back the 3D image is a fine stretch goal.)
3. **Assert parity** at a grid of world positions spanning altitudes (e.g.
   −10…200) and distances, for **three wind cases**: `wind=0` (round blobs),
   moderate wind, and strong wind (streak frame). Tolerance: lavapipe is fp32 so
   use a small abs+rel epsilon (e.g. `abs ≤ 1e-4` or `rel ≤ 1e-3`); document the
   max observed error.
4. **Stretch:** assert the empty-skip predicate — for the proximity-cleared near
   field (first ~3 m), `density * step_through < OD_EPS` — mirroring the logic in
   `handoff/froxel_skip_model.c` as a Rust unit assertion (no GPU needed).

**Hard constraint:** do **NOT** edit any shader. You only **add** the test (and,
if it helps, a small CPU `cell_density` mirror inside the test file — do not
modify `froxel_fill.comp`).

## Validate / run
```
cp Cargo.lock /tmp/lock.v4; rm -f Cargo.lock
VK_ICD_FILENAMES=<lvp_icd.json path> cargo test -p vk-harness --test froxel_parity -- --nocapture
cp /tmp/lock.v4 Cargo.lock
```
Match the harness setup the existing tests use (device/queue/pipeline creation
via `vk_harness`). Keep the test self-contained.

## Definition of done
- A new **green lavapipe test** proving GPU `cell_density` matches the CPU oracle
  within tolerance across the sampled domain and all three wind cases; report the
  max observed error.
- No shader source modified; follows existing `vk-harness/tests` conventions.
- Hand back an **overlay** (`crates/vk-harness/tests/froxel_parity.rs`, plus any
  `Cargo.toml` test wiring you needed) + a short report (what you assert, the
  epsilon + max error, the lavapipe path you used, any blockers). Exclude `target/`.
