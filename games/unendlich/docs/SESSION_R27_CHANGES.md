# SESSION R27 — CHANGES & STATE (handoff to next coordinator)

Round theme: **fix the render stall** + stand up a **glfw-free headless build** so render-side
code can be validated in-sandbox (lavapipe). Below is what changed, where, how it was
validated, and what's still open. Pairs with the R27 block in `COORDINATOR_BOOTSTRAP.md`.

## 1. The render stall — diagnosis (the central problem)
Symptom: a ~26–40ms spike on the frame-head ("shadow") GPU timer during streaming bursts,
co-occurring with a worker-mesh `near max ~30ms` that is ~270× its p50 (≈0.1ms) — i.e. a
WAIT, not compute.

Established (across many run logs + this round's code reading):
- The "shadow" timer = the whole FRAME HEAD, not the shadow pass (persists at 2048²/512²/
  stride1/stride2). Not shadow raster.
- Working set is tiny (mesh ~1–7MB, octree ~0.1–1MB) → NOT bandwidth → compression won't
  fix THIS spike.
- Near-meshing is ASYNC (worker pool). The mesh worker path holds **no shared lock** (only a
  timer mutex). So the 30ms isn't lock contention inside the mesh.
- **Root cause for the on-demand mesh staying on the critical path:** the bin mesh-candidate
  loop walks only `player_chunk ± render_distance`. Prefetch made chunk DATA ready ahead
  (kills generation latency) but MESHING still fired at rd-shell-entry ≈ the moment of need.
- Residual 30ms (after moving the mesh ahead) is most consistent with **burst CPU/scheduling
  pressure** (many chunks load+mesh+upload in one window inflating worker wall-time AND the
  frame head together). Fix = SPREAD work over time, not chase one bottleneck.

## 2. The fix (assembled; end-to-end verification gated on M2 or T09)
Three connected pieces, all in the base:
1. **Deferrable upload lane** — `crates/unendlich-engine/src/render/mesh_buffer_pool.rs`:
   `write_at_deferrable` (prefetch lane), `pending_deferrable_vert/idx_copies`, budgeted
   `flush_pending_copies` via pure `count_within_budget(sizes, budget, already_spent)->usize`
   (6/6 standalone tests). Env `UNENDLICH_MESH_DEFER_BUDGET_BYTES` (default 1MiB; 0=drain-all).
   Visible writes flush immediately; prefetch writes spread + carry. Staging offsets identity-
   mapped to device → partial drain safe. **Lavapipe-validated** (submit + queue_wait_idle +
   full drain).
2. **chunk_manager prefetch (predictive LOADING)** — `world/chunk_manager.rs`:
   `request_prefetch` + `last_player_world`; env `UNENDLICH_PREFETCH` (default OFF),
   `_COUNT` (16), `_LOOKAHEAD` (16). Wires `world::prefetch_predict::prefetch(...)`.
3. **mesh-shell-extension (predictive MESHING)** — the missing link:
   - `world/chunk_manager.rs::predicted_mesh_ahead(&self) -> Vec<ChunkCoord>` — returns
     motion-predicted LOADED chunks (prefetch ring ∩ resident), gated on prefetch-enabled +
     moving; new field `last_vel_chunks` cached each `update()`. **Validated headless**: unit
     test `predicted_mesh_ahead_returns_loaded_chunks_along_motion`; chunk_manager 26 tests green.
   - **bin fold-in** — `src/bin/unendlich_world.rs`, after the shell walk / before submit:
     iterates `predicted_mesh_ahead()`, mirrors the exact per-coord filters (tier-full,
     `meshing_in_flight`, `get_chunk`, `mesh_or_empty_version_for_lod`) + a candidate-
     membership dedup, pushes to tier by `desired_lod_for`. **M2-COMPILE-ONLY** (bin needs
     `window`). Validated in-sandbox by inspection only (brace/paren balanced, symbols in
     scope, mirrors lines ~2742–2790). Patch: `mesh-shell-extension--bin-foldin.patch`.

Supporting model: `handoff/prefetch_lookahead_model.c` — proves required
LOOKAHEAD(frames)=L_ms·fps/1000 (speed cancels); defaults hide ≤133ms latency at 120fps.
Raise LOOKAHEAD (not COUNT) if mesh latency grows.

## 3. Headless build (T06) — what unblocked in-sandbox render validation
Feature split: `render→render-core→gpu`, `window=[glfw,vk-sys]`, `headless=[render-core]`.
`VulkanContext::new_headless()` + `create_offscreen_target()`; no glfw in the headless dep
tree (verified). Smoke test `creates_headless_device` passes on lavapipe.
Consequence: bins (need `window`/glfw/cmake) are **M2-compile-only**; lib + render module
compile + test here. See `docs/GLFW_DECOUPLING.md`.

## 4. Merge log (this round)
| item | files | validated | note |
|---|---|---|---|
| T05 mesh-ahead upload | world_renderer.rs (`*_deferrable`, `chunk_visible`) | headless compile | |
| T06 headless | Cargo.toml, context.rs, offscreen.rs | headless compile + lavapipe smoke | unblocks the rest |
| T07 (partial) | input_codes.rs only | compile | winit migration DEFERRED |
| T08 dragon | creature_viewer/creature_design/wing_membrane | gpu compile + 9 tests | wingskin NOT wired → PENDING |
| hh T1 | world_renderer (shadow-buf hoist) | headless | |
| hh T2 | octree-evict batch (one lock/tick) | headless | |
| hh T3 | chunk_visible per-frame frustum cache | headless | |
| hh T4 | bin prefetch scratch reuse (thread_local) | M2 (bin) | |
| hh T5 | env-cache | **HELD/REVERTED** | cached runtime-tunable flags; broke tunability+tests |
| hh T8 | world_renderer ~2008 octree_pool lock narrow | headless compile | identical set, shorter hold |
| hh T9 | world_renderer ~3929 hoist per-chunk vtx/idx bind | headless compile | Vulkan bind-state persists → identical draw |
| hh T10 | world_renderer evict_stale_lods O(total)→O(#LODs) | headless compile + equivalence proof | supersedes T7-bonus B1 |
| hh T7 | bin: hoist 4 *_cands Vecs (clear+drain) | M2 (bin) | compatible w/ mesh-shell fold-in |
| mesh-shell-ext | chunk_manager helper + bin fold-in | helper headless+test; bin M2 | the stall fix's missing link |

## 5. STILL OPEN (next coordinator picks up)
- **T6** (headlesshorseman in-loop env-read cache) — still dispatched, not returned.
- **T09** (headless `WorldRenderer` vs offscreen target) — still dispatched. Returning it
  enables in-sandbox end-to-end stall ordering repro on lavapipe (logic/ordering, not exact
  GPU timings).
- **T7-bonus B2** — `world_renderer` ~3510: per-frame `visible_meshes: Vec<_>` collect on the
  `!use_indirect` shadow path, only needed when `bucket_active` (diagnostic, usually off).
  Skip the collect otherwise. Headless-validatable; good quick win. (B3/B4/B5 lower value.)
- **T08 wingskin wiring** — consume `wing_membrane` mesh in a renderer (it doesn't draw yet).
- **M2 confirm**: apply `mesh-shell-extension--bin-foldin.patch` + the T7 bin change on M2 and
  confirm `cargo build`; then measure whether the frame-head spike drops.

## 6. Standing engineering rules reaffirmed this round
Verify, don't assume — this round caught/owned: a false "builds clean" claim about render-
gated code, a duplicate prefetch_predict module, a mis-scoped T5, and T10's equivalence
(verified `stale_lod_key` ≡ same-coord/other-LOD before trusting it). Don't ship unvalidated
render-side code as "done"; factor validatable logic into headless/gpu helpers; label M2-only
parts honestly.

## 7. FINISHING PASS (renderer perf + little things) — all in-sandbox validated
- **B2 (renderer perf, DONE)** `world_renderer.rs` ~3528: the `!use_indirect` shadow path
  no longer allocates the per-frame `visible_meshes: Vec`. Refactored to two closures
  (`visible` predicate + `draw1`) and a branch: normal path iterates+draws live (zero alloc);
  the bucket-timer diagnostic path (normally off) keeps the collect for boundary math —
  behavior identical. Headless compile-clean.
- **T6-lib (renderer perf, DONE)** `meshing_pool.rs`: hoisted `UNENDLICH_GPU_MESHER_MID` +
  `UNENDLICH_NEUTRAL_TINT` reads OUT of `worker_loop`'s `while recv()` loop (were re-read via
  env::var per dequeued request during streaming) to thread-start. NOTE: T6's brief pointed at
  meshing_pool:910, but that's one-time dispatcher *setup* (read-once) — NOT hot. The real hot
  reads were the per-request ones at 547/570; those are what got hoisted. Launch A/B flags, not
  per-frame tunables, so safe. Headless compile-clean. (T6's bin-side reads remain M2-only; the
  T6 worker thread is still out but the lib win is done correctly.)
- **T08 wingskin (converter DONE; full draw still gated)** `wing_membrane.rs`:
  added `membrane_from_box(center, half, span_axis, detail, opacity_gene) -> WingMembrane`,
  which derives a membrane fan from a wing *box* and pulls alpha from
  `wing_opacity(gene)` — so `wing_membrane` is now consumed (was referenced only by lib.rs)
  via the single entry point a renderer would call. Test
  `membrane_from_box_fills_box_and_tracks_opacity` passes (wing_membrane now 10 tests).
  **HONEST BLOCKER for the actual wingskin DRAW:** there is no creature GPU renderer in-tree
  (`creature_viewer` is a CPU/test projection layer; `world::entity` builds opaque boxes via
  `push_box` and `EntityVertex` has no alpha channel). Drawing translucent wingskin needs
  either a creature render path or an alpha-capable entity vertex/material — a real feature,
  not a wire. `membrane_from_box` is the ready hook for when that lands.

## Open items now (updated)
- T6 (worker) + T09 (worker) returns to merge when they arrive.
- M2: build-confirm the two bin patches (mesh-shell-extension fold-in + T7 *_cands hoist).
- Wingskin DRAW: needs a creature renderer / alpha entity material (see above) — larger feature.

## 8. PERF SWEEP CLOSE-OUT (this pass)
Audited the remaining T7-bonus items to a decision rather than leaving them vague:
- **B1** — superseded by T10 (merged). Done.
- **B2** — done (section 7). Also removed a redundant `unsafe` block the compiler flagged in
  the new `draw1` closure (closure inherits the enclosing block here). Headless warning-clean.
- **B3** — already addressed in-tree: `world_renderer::loaded_chunk_coords` already returns
  `impl Iterator` (not an owned `Vec`); its one bin caller `.collect()`s only because eviction
  needs to own the snapshot, and that path is eviction-gated, not per-frame. No action.
- **B4 (meshing_pool `drain_completed`/`drain_up_to`)** — real per-frame `Vec`, but the only
  non-test callers are **bins** (`unendlich_world.rs`, `gpu_mesh_bench.rs`) → M2-only; can't
  validate a callsite change in-sandbox, and the audit rated it low value. Left for M2.
- **B4-lib (DONE, headless-clean)** — the one lib-side per-tick drain: `chunk_loader` exposed
  `drain_completed_iter() -> impl Iterator` (`chunk_rx.try_iter()` with no intermediate `Vec`);
  switched the two per-tick `chunk_manager` callsites (702, 875) that immediately `for`-loop the
  result. `drain_completed` (Vec form) kept for count/retain callers. Same items/order.
- **B5 (bin lava-scan Vecs)** — bin, M2-only; deferred (also "verify hotness first").

Net: every in-sandbox, headless-validatable per-frame/-tick allocation from the audit is now
closed (B1/B2/B3/B4-lib). What's left is genuinely gated: B4-meshing_pool + B5 are M2-bin-only,
and the central frame-head-spike fix still needs M2 or the T09 return to verify end-to-end.

## 8. SANDBOX UNBLOCK — bins now compile here (window feature)
Installed glfw-sys's build deps via the allowed apt mirrors (cmake + X11/GL dev headers; see
COORDINATOR_BOOTSTRAP.md for the exact command). `cargo check -p unendlich-engine --features
window --bins` then builds glfw-sys (cmake compiles GLFW from C) and checks the bin — ~22s,
clean. Consequence: the two changes previously labeled "M2-compile-only" are now
COMPILE-VALIDATED in-sandbox over the current tree:
- mesh-shell-extension bin fold-in (`predicted_mesh_ahead()` call at unendlich_world.rs:2815)
- T7 *_cands hoist (clear() at 2699-2702, drain(..) at 2873-2876)
Both compile clean under `--features window`. M2 is still required only to RUN the bin (needs a
real display/GPU; lavapipe is headless-only) and thus to MEASURE the frame-head spike drop.
The "bins are M2-compile-only" rule is retired — compile-check bins in-sandbox going forward.

## 9. Flag codification + doc pattern (R27 perf-tuning tail)
New flags this round (all registered in runtime_flags.rs so they show in --help + "Flags in effect"):
- **UNENDLICH_GPU_GEN_BATCH** (default 16) — GPU terrain-gen batch size; the chunk loader's
  take-size now follows `generator.batch_size()`, so this one knob controls world-fill rate.
  (Was a hardcoded 16 in the bin + the GPU_BATCH_SIZE const; the bin literal is now this env.)
- **UNENDLICH_PROFILE=<name>** — expands a codified flag bundle early in main (set-if-unset, so
  explicit env vars win). First profile: `rd24` (the settled RD=24 quality cluster). Collapses a
  ~25-flag command to ~3. Defined in `runtime_flags::profile()`.
- Fixed the stale `(default: 16)` label on UNENDLICH_MAX_UPLOADS_PER_FRAME (real world default is
  64; 16 was the spin binary's). The label now distinguishes both binaries.

**Doc pattern adopted (see top of GLOSSARY.md › Perf / stall env flags):**
- Overview docs lead with the **recommended** value, not the shipped default; where they differ,
  default shown in (parens).
- Flags are tiered: Recommended (→ the profile) · Per-run knobs · Diagnostics · Fallback/earlier
  paths.
- We do NOT say "legacy" or "deprecated" — the engine is beta and evolving, so older code paths
  are framed as **fallbacks / earlier paths kept for A/B and safety**, in their own section.
- Division of labor: inline CODE comments keep the full history (why a path exists, what it
  superseded); OVERVIEW docs only point at the current best. Don't strip code-comment history.
- Applied to GLOSSARY.md as the worked example. NOT yet propagated to PERF-LEVERS.md, code-comment
  passes, or a `tier` field on the Flag registry — those are the next steps if the pattern sticks.

## 10. Default-flip pass (saner defaults — flag barrage removed)
Promoted the proven perf cluster from opt-in to **default-on** (each keeps a `=0` kill-switch),
so a bare run gets the fast path without the env barrage. Flipped (code default + runtime_flags
registry `default:` string both updated to stay honest in --help / flags-in-effect):
- chunk_manager: PREFETCH (→on), REMESH_GATE (→on), SKY_CULL (→on, margin 1; `=0` off).
- bin unendlich_world: ADAPTIVE_UPLOADS, SUBMIT_SKIP, SLACK_SCHEDULER, BIASED_SHELL_WALK,
  UPLOAD_PRIORITY (all → "on unless =0"); GPU_MESHER default 0→2 (octree GPU mesher; construction
  failure degrades to CPU, so safe as a default).
- mesh_buffer_pool: DEVICE_LOCAL_MESH (→on). world_renderer: SHADOW_CACHE (→on).
Left as knobs (NOT flipped): SHADOW_CHUNK_STRIDE (=1 — changes shadow output), GPU_MESHER_MID
(separate A/B), GPU_GEN_BATCH/MESH_BATCH (numeric/memory; profile sets them).
Validated: headless + window compile clean; chunk_manager 26, mesh_buffer_pool 41, shadow 40,
deferred 5, gpu_chunk_pool/lod_upload 18 all green on lavapipe; HARNESS GREEN; no test asserts an
old default. Bin-only toggles (the five above + GPU_MESHER) are compile-validated + reasoned-safe;
M2 game-loop run is the final confirm. rd24 profile is now mostly about pinning numeric knobs.
