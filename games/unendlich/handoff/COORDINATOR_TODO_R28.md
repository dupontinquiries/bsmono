# ===== HANDOFF — START HERE (R28 open, fresh coordinator bootstrapped) =====
Supersedes the R27 tail in `COORDINATOR_TODO.md` (kept for history). Base is still
`unendlich-r27-merged` until the next merge rolls `-r28-merged`. Read order if you
inherit THIS block: `COORDINATOR_BOOTSTRAP.md` → `docs/GLOSSARY.md` → this file →
`docs/SESSION_R27_CHANGES.md`.

## Ingest benchmark — SELF-ADMINISTERED, PASS (handoff/BENCHMARK_COORDINATOR.md)
Seed prompt: *"bump the GPU generation batch to 256 so the world fills faster."*
- ☑ Read docs first (GLOSSARY "Perf/stall env flags" + R27 START-HERE), not memory.
- ☑ Verified against code, didn't trust the prose: `grep GPU_BATCH_SIZE crates --include=*.rs`
  → defined once (`constants.rs:90`, =16), **zero** non-definition references → the const is
  **DEAD**. Editing it is a **no-op**; the live knob is `UNENDLICH_GPU_GEN_BATCH`
  (`bin/unendlich_world.rs:309`), and `chunk_loader.rs:227` follows `generator.batch_size()`.
- ☑ Cleared a false positive by checking, not asserting: the `"256"` for `GPU_GEN_BATCH` at
  `runtime_flags.rs:76` is inside the **`rd24` profile preset**, not the base defaults; the base
  `default: "16"` (line ~109) is correct and matches the GLOSSARY. No registry inconsistency.
- ☑ Isolated harness-error from real bug: `harness/run.sh` prints HARNESS RED **here only because
  this fresh sandbox has no toolchain** (see below) — the compile/test tiers shell out to `rustc`.
  The pure-python checks pass (R8 delimiters OK, R4 winding OK). Not a code break.

## SANDBOX REALITY (this fresh container) — fix before the first real merge
`rustc`, `cargo`, `glslangValidator`, `spirv-val` are **ABSENT** here; only `python3` is present.
So the compile/test/shader gate is **not live yet**. The coordinator IS the compile gate
(BOOTSTRAP), so before merging anything render-side, install the toolchain:
- Bundled in the upload: `handoff/cargo-workaround/rustup.sh` (+ a 121 MB `rust-1.96…zip`, which
  I excluded from the working extraction to stay lean — re-extract it from the upload when needed).
- Then the usual: Cargo.lock-v4 workaround → `cargo check -p unendlich-engine --features headless`
  (+ `--features window --bins` after the apt glfw deps in BOOTSTRAP "bins") → affected tests on the
  lavapipe ICD → `harness/run.sh` GREEN. lavapipe: `apt-get install -y mesa-vulkan-drivers`.
Until that's done, I can portion/cut/route and do python-tier + read/grep verification, but I must
label any "compiles" claim as PENDING-toolchain.

## Area map — CONFIRMED unchanged (OWNERSHIP.md)
Streams stable: **T01 sim · T02 atmo · T03 world · T04 temporal · T05 mesh**; **MAIN** = bin/lib/
mod-decls/feature-gates (operator/integration only). OFF-LIMITS to workers (coordinator owns):
`src/shaders/froxel_fill.comp`, `src/shaders/chunk.frag`. Pure-core era is over (285 sim cores
exist) — rounds are now **doc-grounded engine tasks**, disjoint + tool-validatable.

## Inherited open items (from R27 tail — all GROUNDED against this tree)
- **Returns still OUT, merge when they arrive:**
  - **T6** (headlesshorseman) in-loop env-read cache — *caution:* R27 HELD T5 for caching
    runtime-tunable flags (defeats tuning + tests). Accept T6 only for reads that are NOT
    per-run-tunable, or once-per-frame-read-once; bounce it otherwise with that reason.
  - **T09** headless `WorldRenderer` vs an offscreen target — high value: it would let the
    end-to-end stall **spike-drop be verified IN-SANDBOX** (ordering repro) instead of M2-gated.
- **B2 — ALREADY DONE (verified R28, do not re-fix).** The R27 note listed "skip the `visible_meshes`
  collect when `!bucket_active`" as open, but `render/world_renderer.rs:3567` already has it: the
  `!bucket_active` branch draws straight from `self.meshes.iter()` with NO allocation; the `collect()`
  at 3574 only runs on the diagnostic `bucket_active` path. Implementing B2 would be a no-op.
- **Coordinator-owned hygiene (do directly; gated on a compile):**
  - Remove/repoint the **dead `GPU_BATCH_SIZE`** const (`constants.rs:90`, 0 refs) — either delete
    or wire it as `UNENDLICH_GPU_GEN_BATCH`'s default. If the operator's checkout still has a hand
    `GPU_BATCH_SIZE=256` edit, **tell them it's a no-op.**
  - Dedupe the **duplicated `RENDER_DISTANCE` doc block** in `constants.rs` (the "shared between…"
    IMPORTANT paragraph appears twice, lines 30 & 40). Cosmetic.
  - Keep CHUNK_SIZE/CHUNK_VOLUME as `const` (array-sizing). Rule: per-run-tunable→env flag;
    structural/array-sizing→const; physical→const.
- **Integration-tier (operator/M2 only):**
  - Apply + **run**-confirm the two bin patches from R27 (mesh-shell fold-in, T7 `*_cands` hoist).
    They're compile-validated; M2 is needed only to RUN/measure. (Patch files are not in this base;
    they live in the operator's `changes/` — re-cut from the bin source if lost.)
  - **T08 wingskin**: `wing_membrane.rs` exists + is in `lib.rs` but is consumed by **no** renderer.
    Blocked on an `EntityVertex` alpha decision (no alpha channel today). Queue against ATMO+MAIN;
    don't hand the shader files to a worker.
- **Bigger, design-first (queue, never edit blind):**
  - **GPU memory partitioner** (user-requested, `docs/TODO-BACKLOG.md`): region-partitioned mesh/
    chunk pools + queue sectioning keyed by LOD/sector, keep player-sector + predicted-next warm.
    Evidence: flat single-arena pools; a 54 ms record smells like un-batched per-chunk uploads;
    8 GB octree reservation wasteful on M2 unified memory. **Bench before edit**; produce a design
    doc + CPU-reference (R22) first. → T03/world.
  - **Dragon model swap** (user asked "quick to swap to the webpage's new models?"): mechanical —
    hand-edit the 8-part PartState box list at `spawn.rs:29–39` + update the box-count tests
    (`a_blueprint_bakes_one_box_each`, `creature_geometry_is_a_box_per_body_and_intact_part`). ~1 hr
    **iff** it stays box-parts with existing Part roles. **Blocked**: the viewer is one-way
    (engine→viewer; no importer), so the operator must supply the **target box layout**. NOT quick if
    it means imported mesh assets (no glTF loader) or translucent wings (the T08 alpha blocker). → T01.

## Proposed next round (R28) — for operator approval before I cut zips
Disjoint, doc-grounded, headless-validatable; froxel_fill/chunk.frag stay coordinator-owned.
1. **headlesshorseman B2** → skip `visible_meshes` collect when `!bucket_active`. (render-side, ready)
2. **T03/world** → GPU memory partitioner **design + CPU-reference only** (no pool edits yet; bench
   harness + numbers first).
3. **T01/sim** → dragon box-layout swap — *held* until the operator pastes the target box layout.
4. Coordinator does the **constants.rs hygiene** itself once the toolchain is installed (compile to
   confirm the dead-const removal).
Outstanding worker returns to fold in on arrival: **T6** (with the held-T5 caution), **T09**.

## GPU STALL / CHUG — R28 audit (coordinator working this directly)
Counters (this base): `perf_baseline.json` total p50 **7.01ms**, p99 **92.82ms**, over-budget **3.0%**,
tick_apply p50 **18.1ms**. Hardware log: GPU mesher already won big (mesher.near 21.43→0.11ms); residual
pain is **CPU/scheduling burst**, not GPU-kernel time (`cpu.wait_fences` max ~130ms; total max spikes 83ms).
- **Cheap no-code lever already spent:** raising the octree pool moved the wall pool-bound → raster-bound
  at RD24. Don't re-pull it.
- **Per-frame hot path is already allocation-lean (verified against code, not the note):** B2 done (3567),
  frustum cache done (`chunk_visible` 1340–1367, rebuilds only on key change), T8 octree-pool lock narrowed
  (~2008), T9 per-chunk vtx/idx bind hoisted, T10 O(#LODs) stale-LOD evict, T7 *_cands hoisted. The remaining
  `collect`s in `world_renderer.rs` are on per-upload / per-evict / UI paths (e.g. `cache_evict_lru`:1746 —
  feeds the *unit-tested* `cache_lru_key`; rewriting to `min_by_key` would change a tested tie-break for a
  tiny unverifiable gain → left alone), NOT the per-frame critical path.
- **THE GATE:** end-to-end spike-drop can only be *observed/verified* on the **M2 run** (real timings;
  lavapipe gives none) or via **T09** (headless `WorldRenderer` vs an offscreen target → in-sandbox ordering
  repro: does the predicted mesh land resident pre-visibility). **T09 is the unblocker and is still OUT.**
- **Next stall actions (in order):** (1) merge **T09** when it returns, then run the in-sandbox ordering
  repro; (2) failing that, the operator runs the assembled fix on M2 and pastes the run summary (I'll mine it
  into `reference.jsonl`). (3) Separately, **tick_apply p50 18.1ms** is a felt-stutter metric (R18) on the SIM
  side, not the render stall — watch it but don't conflate it. No speculative micro-edit is warranted here;
  the merged micro-wins + the assembled prefetch→ahead-mesh→deferrable-upload fix cover the in-sandbox lever.

## R28 dispatched (this session): DRG dragons-material-genes · PLG plant-gene-traits · ITM item_resolve ·
INV inventory_ops · SHD temporal_accumulate (CPU ref). 4 of 5 append to `sim/mod.rs` (DRG/ITM/INV/SHD),
PLG to `world/mod.rs` — those single-line `pub mod` appends are the only overlap; reconcile additively on
merge. Each is a NEW file + tests; froxel_fill.comp/chunk.frag handed to nobody.

## Dispatch mechanics (ready to execute on approval)
Per BOOTSTRAP: cut `worker-T0X-r28.zip` = this base + root `WORKER_ID` (`T0X-R28`) +
`WORKER_BRIEF.md` (LOW EFFORT, one scoped task, RETURN_KIT steps), `target/` excluded. Build the
extractor first (`rustc tools/thread_tool.rs -o /tmp/tt`) — needs the toolchain installed. Merge
returns low-conflict order, `harness/run.sh` + `cargo check --features headless` after each, roll
`unendlich-r28-merged.zip` + a fresh `MANIFEST.sha256`.

## R28 MERGE LOG — DRG + SHD + ITM landed → unendlich-r28-merged (PLG, INV still out)
Returns arrived for 3 of 5 threads; **PLG (plant_gene_traits) and INV (inventory_ops) NOT returned.**
- **Provenance (benchmark trap #1):** DRG and SHD each came back as TWO non-identical takes (the `__1_`
  download suffix is NOT a recency marker — DRG's `__1_` was *earlier*, SHD's *later*). Content-diffed
  both; neither was a stale re-upload — they were two real implementations. Picked on merit:
  - **DRG = `__1_`** — uniform `express()` one-gene-per-core (matches brief), named const ranges,
    physical reflectivity 0.02–0.10, 1000-step property sweeps. (Other take coupled armor+gloss in one fn.)
  - **SHD = non-`__1_`** — accumulates **RGBA** generically (rgb inscatter + `.a` extinction); the `__1_`
    take was RGB-only, leaving froxel density un-stabilised. (Borrow `__1_`'s decomposed public helpers
    when mirroring into the shader.)
- **Re-verified myself (didn't trust status blocks):** rustc `--test` per module — armor_reflect 3,
  wing_opacity 3, scale_gloss 3, temporal_accumulate 7. item_resolve compiled against the REAL sibling
  cores (minimal `sim` crate, 9 leaf cores + resolver) → 63 tests incl. its own 8. All 9 referenced core
  fns confirmed present. ITM panics are test-only; `resolve()` returns `Option`.
- **Gates:** delimiter R8 OK ×5; render-cfg R1b OK; `harness/run.sh` GREEN; **`cargo check -p
  unendlich-engine --features gpu` Finished** (only pre-existing `winit`/dead-code warnings, no errors).
- **sim/mod.rs:** 5 `pub mod` appended additively (DRG×3, SHD, ITM) — the one shared-file friction,
  reconciled idempotently.
- **Wiring still the coordinator's (integration, NOT done by this merge):** dragon material genes →
  entity material shader (and `wing_opacity`→membrane alpha still needs the EntityVertex-alpha / T08
  decision); `temporal_accumulate` → mirror into `froxel_fill.comp` + prove lavapipe parity (R22/R24);
  `item_resolve` → world-drop/`item_entity` hookup.
- Base rolled: **unendlich-r28-merged.zip** + fresh MANIFEST.sha256. Toolchain now installed in-sandbox
  (rust 1.96 via the bundled offline bundle) → coordinator compile gate is live.
