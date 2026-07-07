# ===== HANDOFF — START HERE (R27 tail, coordinator at limits) =====
This coordinator is **out of limits and can no longer fan out worker threads.**
Next coordinator inherits the pipeline. Read COORDINATOR_BOOTSTRAP.md first, then this block.
**FIRST RUN:** self-administer `handoff/BENCHMARK_COORDINATOR.md` before real work (it's a
behaviour check built from a real trap — confirms you reach for docs+compiler, not priors).
Hand each new worker `handoff/BENCHMARK_THREAD.md` as the equivalent check.

## What landed this session (all compile-validated; in the rolled zip)
- **Default-flip pass** — the proven perf cluster is now DEFAULT-ON (kill-switch `=0` each):
  PREFETCH, GPU_MESHER=2, DEVICE_LOCAL_MESH, SHADOW_CACHE, REMESH_GATE, SKY_CULL(margin1),
  ADAPTIVE_UPLOADS, SUBMIT_SKIP, SLACK_SCHEDULER, BIASED_SHELL_WALK, UPLOAD_PRIORITY. Code default
  + runtime_flags registry `default:` strings both updated. Validated: headless+window compile;
  chunk_manager 26 / mesh_buffer_pool 41 / shadow 40 / deferred 5 / gpu_chunk_pool+lod_upload 18
  green on lavapipe; HARNESS GREEN. Bin-only toggles + GPU_MESHER are compile+reason validated,
  M2 game-loop is final confirm. A bare `cargo run` now gets the fast path; rd24 profile is now
  mostly numeric-knob pinning. (docs/SESSION_R27_CHANGES.md §10)
- **New flags:** UNENDLICH_GPU_GEN_BATCH (loader follows generator.batch_size()), UNENDLICH_PROFILE
  (=rd24). **thread_tool.rs** revamp (extract/scan/merge). **B2/T6-lib/T08** perf+wingskin.
- **Bins compile IN-SANDBOX now** (was M2-only): install glfw deps via apt (see BOOTSTRAP "bins"),
  then `cargo check -p unendlich-engine --features window --bins`. M2 only needed to RUN.
- **Doc pattern adopted** (top of docs/GLOSSARY.md): overview docs lead with the recommended value
  (default in parens); tiered Recommended/Knobs/Diagnostics/Fallback; NO "legacy"/"deprecated"
  (beta → "fallback / earlier path"); inline code comments keep history. NOT yet propagated to
  PERF-LEVERS.md or a `tier` field on the Flag registry — optional next step.

## Open findings to action (raised this session, NOT yet done)
1. **constants.rs is mostly *necessary*, with crutch remnants to prune:**
   - `GPU_BATCH_SIZE` const is now **DEAD** (unreferenced after the env-flag move). If the user's
     checkout matches the rolled tree, their `GPU_BATCH_SIZE=256` edit is a **no-op** — the live
     knob is `UNENDLICH_GPU_GEN_BATCH` (default 16) / profile 256. Remove the const or wire it as
     the flag's default.
   - The `RENDER_DISTANCE` doc block is **duplicated** (the "IMPORTANT: shared between…" paragraph
     appears twice, ~lines 30–43). Cosmetic; dedupe.
   - `RENDER_DISTANCE` the const lingers "for old tests" — fine, but candidates that are genuine
     compile-time invariants (CHUNK_SIZE/CHUNK_VOLUME — used in array sizing) MUST stay const.
     Physics/player/window values are fine as const (not per-run perf levers). Rule of thumb:
     per-run-tunable → env flag; structural/array-sizing → const; physical constants → const.
2. **Dragon model swap (asked: "quick to swap to the webpage's new models?"):** FEASIBLE & mostly
   mechanical, with caveats. The dragon is an 8-part box-list at **spawn.rs:29–39**
   (`Anatomy::new().with(PartState::new(Part::Wing, hp, offset, half_extents))…`). The three.js
   page `tools/inspector_template.html` renders the SAME blueprint shape (boxes w/ role ids
   8=wing,7=leg) via `const DATA = __BLUEPRINTS__` — i.e. it's GENERATED FROM the engine (one-way
   engine→viewer; there's no viewer→engine importer). So reshaping = hand-editing the PartState
   offsets/half-extents/part-roles in spawn.rs to match the new box layout + updating the box-count
   tests (`a_blueprint_bakes_one_box_each` asserts verts=boxes*8, `creature_geometry_is_a_box_per_body_and_intact_part`
   asserts body+8). ~an hour if it stays box-parts with existing Part roles. NOT quick if "new
   models" = imported mesh assets (engine has NO glTF/mesh loader, boxes only) or if it needs
   translucent wings (EntityVertex has no alpha — the T08 wingskin-DRAW blocker is still open).
3. **BACKLOG (user-requested): GPU memory partitioner** — region-partitioned mesh/chunk pools +
   queue sectioning keyed by LOD/sector, keep player-sector+predicted-next warm. Design-first,
   bench before edit. See docs/TODO-BACKLOG.md. Evidence: flat single-arena pools; record=54ms
   smells like un-batched per-chunk uploads; 8GB octree reservation wasteful on M2 unified memory.

## Superseded below (kept for history, per the no-"legacy" doc pattern)
- "TODO (deferred): suggest the perf-optimization thread batch" — its trigger (headless tooling +
  in-sandbox bins) is now MET, but this coordinator can no longer fan out, so it passes to you.
- "mesh-shell-extension STATUS" part 2 marked M2-COMPILE-ONLY — bins now compile in-sandbox AND the
  fix was confirmed in the live M2 perf loop (near-mesh stall resolved via prefetch→GPU path).

# ============================================================

## TODO (deferred): suggest the perf-optimization thread batch
Trigger: ONCE the headless tooling is built AND shareable (a headless
frame-driver that runs on lavapipe, packaged so workers can use it).
Then surface this to the user verbatim-ish:

  "Spin up five threads to tackle small but helpful performance optimizations we
   can merge. Dispatch them named `headlesshorseman--TNUM-INSTRUCTIONS` (so
   they're easy to spot in the downloads folder); have each return a
   `headlesshorseman--TNUM-SOLUTION` zip."

Notes for when it fires:
- Each thread = one small, independently-mergeable perf win (no big refactors).
- Give each the headless frame-driver so they can self-measure on lavapipe
  (ordering/correctness; not exact GPU timings).
- Candidate small wins to seed the 5: (1) hoist the redundant per-iter
  vertex/index rebind in the shadow fallback loop; (2) batch the per-coord
  octree-pool evict lock into one lock per tick; (3) cache Frustum build in
  chunk_visible instead of rebuilding per call; (4) reuse the surface-run
  buffer across prefetch classify calls; (5) collapse the 9-tap shadow PCF to a
  1-tap blue-noise sample behind a flag.

## mesh-shell-extension (render-stall fix) — STATUS (this round)
ROOT CAUSE pinned: the bin mesh-candidate loop walks only player_chunk±rd, so
prefetch made chunk DATA ready ahead (kills gen latency) but MESHING still fired
at rd-shell-entry ≈ moment of need → the ~30ms async near-mesh stayed on the
critical path. (No shared lock in the mesh worker path; data tiny → not bw/lock;
residual burst-pressure handled by spreading: prefetch + deferrable + T1–T10.)

FIX (two parts):
 1. chunk_manager::predicted_mesh_ahead(&self)->Vec<ChunkCoord>  [DONE+VALIDATED]
    - returns motion-predicted LOADED chunks (prefetch ring ∩ resident), gated on
      prefetch_enabled + moving. New field last_vel_chunks cached each update().
    - compiles headless; unit test predicted_mesh_ahead_returns_loaded_chunks_along_motion
      passes; chunk_manager now 26 tests green.
 2. bin fold-in (unendlich_world.rs, after shell walk ~2792, before submit phase)
    [M2-COMPILE-ONLY — bin needs `window`/glfw; cannot build in sandbox]
    - iterates predicted_mesh_ahead(), mirrors the exact per-coord filters
      (tier-full, meshing_in_flight, get_chunk, mesh_or_empty_version_for_lod) +
      an explicit cands-membership dedup, pushes to tier by desired_lod_for.
    - VALIDATION DONE IN-SANDBOX: brace/paren balanced, all symbols in scope,
      mirrors lines 2742–2790 verbatim. PENDING: M2 `cargo build` to confirm.
    - patch: mesh-shell-extension--bin-foldin.patch
VERIFY (gated): real spike-drop needs M2 run, or T09 headless WorldRenderer for
in-sandbox ordering repro (does the predicted mesh land resident pre-visibility).

## MERGE LOG — headlesshorseman T7/T8/T9/T10 (this round)
- T8 (narrow octree_pool lock): MERGED. world_renderer ~2008. Snapshot Copy
  coords under lock, hash into set outside. Identical set; less lock hold.
  Headless compile-clean.
- T9 (hoist per-chunk vtx/idx bind out of fallback draw loop): MERGED.
  world_renderer ~3929. Vulkan bind-state persists -> identical draw; removes
  redundant per-chunk binds. Headless compile-clean.
- T10 (O(total)->O(#LODs) stale-LOD evict): MERGED. world_renderer: new
  evict_stale_lods() replaces two self.meshes.keys().filter().collect() scans
  on the per-upload path. Equivalence VERIFIED (stale_lod_key == same-coord/
  other-lod; LodLevel::iter + MeshSlotKey::at confirmed). Headless compile-clean.
  NOTE: supersedes T7 bonus B1 (which only suggested this).
- T7 (hoist 4 *_cands Vecs out of frame loop; clear()+drain()): MERGED.
  bin unendlich_world.rs. 4 allocs/frame -> 0. Compatible with mesh-shell
  fold-in (clear before shell walk, push in fold-in, drain at submit).
  M2-COMPILE-ONLY (bin needs window) — like the fold-in; not sandbox-compiled.
- VALIDATION NOTE: render-side trio (T8/T9/T10) validated via `cargo check
  --features headless` (clean) + correctness-by-inspection. Full lavapipe test
  *suite* build exceeds sandbox per-call time; deferred. These three are
  behavior-preserving by construction and don't touch the upload paths the
  existing deferrable tests cover. HARNESS GREEN.
- STILL OUT: T6 (in-loop env-read cache). 
- T7 BONUS to cherry-pick (not yet done): B2 = per-frame visible_meshes collect
  on the !use_indirect shadow path (world_renderer ~3510), only needed when
  bucket_active — skip the collect otherwise. Headless-validatable; good next.
  B3/B4/B5 lower value (non-per-frame / API-boundary).

## HANDOFF (R27 close) — for the next coordinator
Context window is full; spawn a fresh coordinator with the updated base.
- READ FIRST: COORDINATOR_BOOTSTRAP.md (R27 block) + docs/GLOSSARY.md.
- WHAT CHANGED: docs/SESSION_R27_CHANGES.md (full merge log + open items + stall diagnosis).
- LOOKUP TOOL (save tokens): tools/xref.sh  (sym|env|doc|find|file). Use instead of broad greps.
- FAN-OUT/FOLD-IN: tools/thread_tool.rs (build: rustc -O tools/thread_tool.rs -o /tmp/tt) +
  tools/THREAD_TOOL.md + tools/threads.example.txt. extract (zip|embed) → merge → super zip.
- OPEN: T6 + T09 worker returns (not yet arrived); M2 build-confirm the two bin
  patches (mesh-shell-extension--bin-foldin.patch + T7's *_cands hoist).

## UPDATE (R27 finish): bins compile in-sandbox now
After `apt-get install cmake libx11-dev libxrandr-dev libxinerama-dev libxcursor-dev libxi-dev
libgl1-mesa-dev libxkbcommon-dev`, `cargo check --features window --bins` is clean (22s, glfw-sys
builds via cmake). The mesh-shell fold-in + T7 hoist are now COMPILE-VALIDATED here (were
"M2-compile-pending"). M2 still needed only to RUN/measure (display/GPU). Bin edits are no longer
compile-only on M2 — check them in-sandbox.
