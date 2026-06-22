# Three-thread dispatch — coordinator map (2026-06-04)

You (the human) paste each `WORKER_*.md` into its own fresh Claude thread. Each
brief is self-contained. This file is your overview + how the coordinator merges
what comes back.

## The threads

| Thread | Task (from the docs) | Primary files | Tool it leans on |
|---|---|---|---|
| **A** | Spatial-index `HeatSourceMap` so `total_heat_at` is O(nearby), not O(n) | `crates/unendlich-engine/src/heat_source.rs` | `rustc --test` (pure-module), `cargo build` |
| **B** | Wire the unwired `§5c` debug toggles (freeze_time/lod, pause_sim, mesher_mid_gpu, neutral_tint, disable_bloom/ssr/shadows) | `bin/unendlich_world.rs`, `render/meshing_pool.rs`, post-process/shadow setup | `cargo build`, `harness/run.sh` |
| **C** | Add a **lavapipe parity test** for the froxel-fog density model (GPU vs CPU oracle) | `crates/vk-harness/tests/froxel_parity.rs` (NEW) | **llvm-pipe / lavapipe** via `vk-harness` |

Doc each draws from: A → `docs/FOLLOWUPS.md` (2026-06-04 heat note) + `handoff/HEAT_SOURCES_DIAGNOSIS.md`. B → `handoff/SESSION_HANDOFF.md §5c`. C → `docs/RENDER-PARITY.md` + `handoff/GPU_HARNESS.md` + `crates/vk-harness/BENCH.md`.

## Conflict map (why these three don't collide)

- **A** is a single isolated module (`heat_source.rs`). Nobody else touches it.
- **B** edits the bin loop + `meshing_pool.rs` + the post-process/shadow *setup*
  sites. It is told to **stay out of the froxel-fill dispatch region** of
  `world_renderer.rs` (coordinator-owned).
- **C** only **adds** a new test file (and may expose a `cell_density` CPU
  mirror). It must **not edit any shader**.
- **Coordinator-owned, off-limits to all three:** `froxel_fill.comp`,
  `chunk.frag` (just changed: blobs→streaks + empty-froxel skip).

## Merge order (low → high conflict risk)

1. **C** — new file, zero conflict. Merge first; its green lavapipe test also
   regression-guards the coordinator's froxel work.
2. **A** — isolated module. Merge second; re-run its parity test from repo root.
3. **B** — touches shared bin/world files; merge last and eyeball the
   effect-gate edits against the current `world_renderer.rs`.

After each merge: apply the overlay at the `unendlich/` root, run
`harness/run.sh` **from the repo root** (green), `cargo build -p unendlich-engine`
(use the v4-lock workaround below), then rebuild the FULL zip (root `unendlich/`,
exclude `target/`).

## Shared rules (every thread already has these in its brief)

- They **can compile and run** — this overrides the stale "you cannot compile"
  line in `SESSION_HANDOFF §0`. They have `cargo`/`rustc` 1.75,
  `glslangValidator`+`spirv-val`, **llvm-pipe/lavapipe** (so `vk-harness` does
  real GPU↔CPU parity), the **harnesses**, and the **docs**.
- **Cargo.lock is v4** (needs cargo ≥1.78; they have 1.75). Reversible workaround:
  `cp Cargo.lock /tmp/lock.v4; rm -f Cargo.lock; cargo <cmd>; cp /tmp/lock.v4 Cargo.lock`.
  Restore the v4 lock before handing back.
- After editing a shader: regenerate committed `.spv`+`.hash` (the ".spv recompile trap").
- **Never reorder `CameraUbo`** — append or push-constant.
- Run `harness/run.sh` from the **repo root** (relative paths).
- **Scope discipline:** work only from the brief + cited docs; do not invent
  features; if blocked/ambiguous, stop and report back.
- Hand back: an **overlay** (changed files only, repo-relative paths) + a short
  report (what changed, how validated, blockers). Exclude `target/`.
