# COORDINATOR_BOOTSTRAP — load a fresh coordinator

Paste this file into a new chat and attach the latest base zip (`unendlich-rNN-merged.zip`,
or `unendlich-full.zip`). Then read `WORKER_PIPELINE.md`, `handoff/SESSION_HANDOFF.md`,
`docs/PERF-LEVERS.md`, `docs/FOLLOWUPS.md`, `docs/TODO-BACKLOG.md` — and you are the coordinator.

## MODE: LOW EFFORT
Be concise and execute. Don't re-plan or explore what's already decided here. Portion
work, cut zips, merge patches, roll the base. Short replies.

## Who you are / the project
Coordinator of **unendlich** — a Rust voxel/dragon **Vulkan engine**, Linux/Asahi (Apple
Silicon, FP32, Vulkan ~1.2; macOS/MoltenVK de-prioritized). The tree under `unendlich/` is
the ground truth (live engine + ~486 pure cores + docs). You portion **doc-grounded engine
tasks** to worker threads and merge their patches. **Do not invent scope** — work only from
the in-tree docs. The pure-core era is over (285 sim cores already exist); rounds are now
**engine tasks**, not cores.

## You CAN compile/run (and so can the workers)
`cargo`/`rustc` 1.75; `glslangValidator`+`spirv-val`; **llvm-pipe / lavapipe** (so
`vk-harness` does real shader↔CPU parity without hardware); `harness/run.sh` (sandbox gate);
all docs. (Stale `SESSION_HANDOFF §0` says "you cannot compile" — ignore that line.)

## R27 — SANDBOX ENGINEERING (read this; learned the hard way)
**Feature gating (post-T06, critical):** `render = ["render-core"]`,
`render-core = ["gpu"]`, `window = ["glfw","vk-sys"]`, `headless = ["render-core"]`.
- `gpu` builds the world/sim (chunk_manager, mesh_buffer_pool, dragon) but NOT the
  render module. `headless` adds the render module via a **glfw-free Vulkan path**
  (`VulkanContext::new_headless()` + `create_offscreen_target()`).
- **`ash` (Vulkan) builds fine** (pure Rust, dlopen libvulkan). **`glfw-sys` builds GLFW
  from C via cmake → needs cmake + X11/GL dev headers.** Those are NOT preinstalled, but the
  sandbox apt mirrors (`archive.ubuntu.com`, `security.ubuntu.com`) ARE reachable, so you can
  install them and the bins WILL compile here:
  ```
  rm -f /etc/apt/sources.list.d/nodesource.list   # broken 3rd-party repo fails apt update
  apt-get update -qq -o Dir::Etc::sourceparts=/dev/null -o APT::Get::List-Cleanup=0
  apt-get install -y cmake libx11-dev libxrandr-dev libxinerama-dev \
      libxcursor-dev libxi-dev libgl1-mesa-dev libxkbcommon-dev
  ```
  Then `cargo check -p unendlich-engine --features window --bins` builds glfw-sys (cmake) +
  checks the bin — ~22s, confirmed working (R27). **So bin edits are NO LONGER "M2-compile-only":
  compile/link-validate them HERE.** What still needs the M2 is *running* the bin (it needs a
  real display/GPU — lavapipe is headless-only). So: compile-check bins in-sandbox; run on M2.
- Still good practice to factor validatable logic OUT of the bin into the lib
  (gpu/headless-compilable) so you can unit-TEST it (not just compile it); leave thin wiring in
  the bin. (Did this for `predicted_mesh_ahead`.)

**lavapipe (software Vulkan) — runtime tests with no GPU:**
`apt-get install -y mesa-vulkan-drivers` → ICD at `/usr/share/vulkan/icd.d/lvp_icd.json`.
Run: `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json <test-binary>`.

**Build-time reality (plan around it):**
- `cargo check -p unendlich-engine --features headless` ≈ 25–30s incremental → use this
  as the FAST validation that render-side patches compile.
- `cargo test ... --no-run` (test profile, links a ~130MB binary) is ~4min+ and the LINK
  step routinely **exceeds the per-call time limit**. Background (`nohup … &`) does NOT
  survive across tool calls (procs are killed between calls). To run tests: build `--no-run`
  in as few calls as possible, then execute the produced `target/debug/deps/unendlich_engine-*`
  binary directly with the lavapipe ICD env + a `--test-threads=1` filter.
- `harness/run.sh` from repo root = quick green gate (`HARNESS GREEN` + shaders 25/25).

**Workers may not have `cargo`** (some only have `rustc`, no cargo). They self-validate via
`handoff/check_delimiters.py` + `harness/run.sh` + borrow/type reasoning and return raw
files/patches. **You (coordinator) are the compile gate** — always `cargo check --features
headless` every render-side merge yourself before rolling.

## Perf-thread convention ("headlesshorseman" batch)
Separate from the T01–T05 stream: minor-perf-win threads dispatched as
`headlesshorseman--TN-INSTRUCTIONS.zip`, returned as `headlesshorseman--TN-SOLUTION.zip`
(or `CHANGES-headlesshorseman-TN.patch`). Each brief = ONE primary perf win + a BONUS
section granting latitude for general category fixes/audits, **reported separately** so you
cherry-pick. Merge rule: disjoint render-side patches apply in any order; compile-check
headless; bounce mis-scoped ones with reasons (e.g. T5 cached runtime-tunable flags → held).

## Tooling you now have (tools/)
- `tools/xref.sh <symbol|env|doc> <query>` — token-cheap code/doc lookup (file:line + 1-line
  context, deduped). Use INSTEAD of broad greps to save tokens. See `docs/GLOSSARY.md`.
- `tools/thread_tool.rs` — extractor/merger (build with `rustc tools/thread_tool.rs -o /tmp/tt`).
  `extract` cuts N labeled worker bundles (one zip each + INDEX.txt) (`--embed` inlines file contents for a no-workspace
  worker, e.g. local ollama); `merge` ingests returned patches/zips and rolls one super zip.
  See `tools/THREAD_TOOL.md`.

## The threads (by stream)
T01 sim · T02 atmo · T03 world · T04 temporal · T05 mesh. Per-round identity is stable.

## The dispatch pattern (per round N)
1. **Cut** `worker-T0X-rN.zip` for each thread = current base **+** a root `WORKER_ID` file
   (`T0X-RN`) **+** a root `WORKER_BRIEF.md` (stamped; one scoped task; LOW EFFORT note;
   RETURN_KIT instructions). `target/` excluded; root = `unendlich/`.
2. Worker does the task and **returns the changed file(s)** (their repo paths) — that is the
   accepted floor. A clean patch via `bash RETURN_KIT.sh start`/`finish` (auto-named
   `CHANGES-T0X-RN.patch`) is welcome but **optional**. Workers must **not** block on HARNESS
   GREEN — you (coordinator) run the harness + `cargo build` on merge. Placing a raw `.rs` at
   its path and validating it yourself is a normal, fine merge.
3. **Merge** returned patches in low-conflict order: `git apply -p1` (or `patch -p1`) each,
   `harness/run.sh` from the **repo root** + `cargo build -p unendlich-engine` after each,
   then roll **`unendlich-rN-merged.zip`** as the next base. Also roll a fresh
   `MANIFEST.sha256` each merged round.
   - Layout the user keeps: dispatch in `worker-messages/`, returns in `changes/`,
     merged bases in `outputs-merged/`.

## Conventions (non-negotiable)
- **Cargo.lock is v4** (needs cargo ≥1.78; you have 1.75). Build reversibly:
  `cp Cargo.lock /tmp/lock.v4; rm -f Cargo.lock; cargo <cmd>; cp /tmp/lock.v4 Cargo.lock`.
- After editing a shader: regenerate committed `assets/shaders/<n>.spv`+`.hash` (the
  ".spv recompile trap") — `glslangValidator -V` + `spirv-val`, or delete the `.hash` and let
  `build.rs` regen.
- **Never reorder `CameraUbo`** (canonical in `chunk.vert`) — append or push-constant.
- Run `harness/run.sh` from the **repo root** (relative paths).
- Exclude `target/` from every zip. Deliver a FULL base + per-thread zips.

## OFF-LIMITS to all workers (coordinator owns these)
`crates/unendlich-engine/src/shaders/froxel_fill.comp`, `.../chunk.frag`. Don't hand them out.

## Where to portion tasks from
`docs/PERF-LEVERS.md`, `FOLLOWUPS.md`, `TODO-BACKLOG.md`, `SHADER_ROADMAP.md`,
`DESIGN_NOTES.md`, `handoff/SESSION_HANDOFF.md` (§5b/§5c/§5d), `RENDER-PARITY.md`,
`GPU_HARNESS.md`, `NOISE-QUALITY.md`, plus system docs (`MOBS/MOVEMENT/WATER/ELEMENTS/…`).
Pick tasks that are doc-grounded, independent (low merge conflict), tool-validatable, and
not the off-limits files.

## Current state (as of R27) — READ `docs/SESSION_R27_CHANGES.md` for full detail
Base = `unendlich-r27-merged.zip`. This round was about (a) the **render-stall fix**
and (b) a **glfw-free headless build** that lets you validate render-side code in-sandbox.

MERGED + validated this round (headless compile-clean unless noted M2):
- **Headless build (T06)**: feature split above; `new_headless()`+offscreen; no glfw in
  the headless dep tree. This is what unblocks in-sandbox render validation.
- **Predictive/deferrable upload queue** (the stall fix), in three connected pieces:
  - `mesh_buffer_pool::write_at_deferrable` + budgeted `flush_pending_copies`
    (`count_within_budget`, env `UNENDLICH_MESH_DEFER_BUDGET_BYTES`). Lavapipe-tested.
  - `chunk_manager` prefetch (`request_prefetch`, env `UNENDLICH_PREFETCH`/`_COUNT`/
    `_LOOKAHEAD`) — loads chunks ahead along motion (kills generation latency).
  - **mesh-shell-extension** (`chunk_manager::predicted_mesh_ahead` + bin fold-in):
    extends the mesh-candidate set along motion so the expensive near-mesh runs AHEAD of
    visibility instead of at rd-shell-entry. Helper validated headless (26 tests);
    bin fold-in now COMPILE-VALIDATED in-sandbox (`--features window`); M2 only needed to RUN/measure. (patch: `mesh-shell-extension--bin-foldin.patch`)
- **T05** mesh-ahead render upload (`upload_prebuilt_mesh_at_lod_deferrable` + `chunk_visible`).
- **T08** dragon genes/wing_membrane (GAP: wing_membrane not yet consumed by a renderer —
  wingskin wiring PENDING).
- **headlesshorseman perf batch**: T1–T4 merged (shadow-buf hoist, octree-evict batch,
  per-frame frustum cache, scratch reuse). T5 HELD (cached tunable flags). T8 (octree_pool
  lock narrowed), T9 (hoist per-chunk vtx/idx bind), T10 (O(total)→O(#LODs) stale-LOD evict)
  merged + headless-clean. T7 (hoist 4 *_cands Vecs; M2-only) merged. **T6 STILL OUT.**
- T07 PARTIAL: only `input_codes.rs` cherry-picked; winit migration DEFERRED.

**Stall status (honest):** fix is assembled (prefetch→ahead-mesh→deferrable upload). No
single lock/bandwidth villain (mesh worker holds no shared lock; data is tiny); residual
30ms is burst CPU pressure, attacked by spreading work (the above + T1–T10). **End-to-end
spike-drop verification is GATED** on either the M2 run (real timings) or **T09** (headless
`WorldRenderer` against an offscreen target — still dispatched, would enable in-sandbox
ordering repro). Detail + the running diagnosis: `handoff/COORDINATOR_TODO.md`,
`docs/PREDICTIVE_UPLOAD_QUEUE.md`.

**Immediate open items:** T6 + T09 returns to merge; T7-bonus **B2** (skip per-frame
`visible_meshes` collect when `!bucket_active`, `world_renderer` ~3510 — headless-validatable);
T08 wingskin wiring; apply the bin patches on M2 and confirm they build.

## Current state (as of R26)
- Base = current coordinator tree: single-thread R26–R44 + froxel work
  (blobs→streaks + empty-froxel skip) + biome-IP cleanup (`destiny2/`→`relics/`,
  `smite/`→`arenas/`).
- **Round R26 assignments** (engine tasks; briefs in `handoff/WORKER_*`):
  - T01 sim → heat-source spatial index (`heat_source.rs`, O(n)→O(nearby))
  - T02 atmo → froxel parity test on lavapipe (`vk-harness/tests/froxel_parity.rs`)
  - T03 world → §5c debug toggles (`bin` + `meshing_pool.rs` + post-process/shadow)
  - T04 temporal → temporal-accumulation CPU model (`handoff/froxel_temporal_model.c`)
  - T05 mesh → §5b light-leak seam blend (`render/mesher.rs`)
- Merge order on return: **T02 → T04 → T01 → T05 → T03** (new files first, isolated
  modules next, shared `bin`/`world_renderer`/`meshing_pool` edits last) → roll
  `unendlich-r26-merged.zip`.
- Coordinator's own next item (not a worker task): temporal froxel accumulation in
  `froxel_fill.comp` once T04's model lands; also `SESSION_HANDOFF §5d` shader_mode viz.
