# Worker thread B — wire the §5c debug toggles

You are one of three parallel workers on **unendlich**, a Rust voxel/dragon game
engine (Vulkan, Linux/Asahi, FP32). A coordinator portioned you this task from
the project docs. Work only from this brief and the docs it cites.

## Your environment (this OVERRIDES the stale "you cannot compile" note in `handoff/SESSION_HANDOFF.md §0`)
- You **can compile and run.** `cargo` + `rustc` 1.75 are installed.
- You have **llvm-pipe / lavapipe**, the **test harnesses** (`harness/run.sh`),
  the static checkers in `handoff/`, and the **docs** — read them.
- Repo root is `unendlich/`; it is the ground truth.
- **Cargo.lock is v4** (needs cargo ≥1.78; you have 1.75). Reversible build:
  `cp Cargo.lock /tmp/lock.v4; rm -f Cargo.lock; cargo <cmd>; cp /tmp/lock.v4 Cargo.lock`.
  Restore the v4 lock before handing back.
- A default `cargo build -p unendlich-engine` (gpu only) compiles the non-render
  code; the `--features render` path needs glfw/system deps — for render-gated
  edits, lean on `handoff/check_delimiters.py` + careful reading. Run
  `harness/run.sh` from the **repo root**.
- Stay in scope. If blocked or ambiguous, stop and report back — don't guess.

## Read first
- `handoff/SESSION_HANDOFF.md §5c` — the list of unwired toggles **with their
  exact sites**. This is your spec.
- `crates/unendlich-engine/src/debug_settings.rs` — the `DEBUG` atomics already
  exist (`disable_ssr/bloom/shadows`, `freeze_lod/freeze_time/pause_sim`,
  `mesher_mid_gpu`, `neutral_tint`, `show_chunk_borders`). You are NOT changing
  this file's declarations — you are adding the **one-line reads** at the
  behaviour sites that consume them.

## Task — add a single atomic read at each behaviour site
Read each atomic with `Ordering::Relaxed` and make it **default to current
behaviour when false** (only change behaviour when the toggle is set):
- `freeze_time` → guard the `sky_state.tick(dt)` / time-of-day advance in the
  bin loop (`src/bin/unendlich_world.rs`).
- `freeze_lod` → guard the LOD update in the bin loop.
- `pause_sim` → guard the sim tick in the bin loop.
- `mesher_mid_gpu` → **OR** into the `UNENDLICH_GPU_MESHER_MID` env read in
  `src/render/meshing_pool.rs` (~line 547).
- `neutral_tint` → **OR** into the `UNENDLICH_NEUTRAL_TINT` env read in the bin.
- `disable_bloom` / `disable_ssr` / `disable_shadows` → at the site where each
  effect's intensity is set (or its pass is recorded), force intensity to 0 /
  skip the pass when the toggle is set. Find these in the post-process pipelines
  and the shadow-pass setup.
- `show_chunk_borders` → **optional/stretch.** It needs a debug AABB/line draw
  and has no hook yet. If there's no cheap existing debug-draw path, leave a
  clear `// TODO(show_chunk_borders): needs debug AABB draw` and do NOT fake it.

**Hard constraints**
- **Do NOT touch the froxel-fill dispatch region** of `world_renderer.rs` (search
  for `froxel_fill_pipeline` / `cmd_dispatch` — the coordinator owns that block).
  The `disable_shadows` gate belongs at the **shadow pass** setup, not there.
- **Do NOT reorder `CameraUbo`** fields.
- These are surgical one-liners — **do not refactor** the surrounding code.

## Validate
```
handoff/check_delimiters.py <each .rs you edited>     # must be balanced
cp Cargo.lock /tmp/lock.v4; rm -f Cargo.lock
cargo build -p unendlich-engine                       # confirms non-render reads compile
cp /tmp/lock.v4 Cargo.lock
bash harness/run.sh                                   # from repo root → green
```
For render-gated sites you can't fully build in-sandbox, state that clearly in
your report and rely on the delimiter check + a careful read of the surrounding
types.

## Definition of done
- Each wired toggle **changes behaviour when set** and is a **no-op when unset**.
- Crate builds (gpu surface); delimiters balanced; `harness/run.sh` green; no
  `CameraUbo` reorder; froxel region untouched.
- Hand back an **overlay** (only the files you edited, repo-relative paths) + a
  short report listing each toggle, its site (file:line), and how you verified
  it. Note any toggle you left as a TODO and why. Exclude `target/`.
