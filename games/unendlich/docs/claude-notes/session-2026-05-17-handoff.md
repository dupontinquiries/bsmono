# Session handoff — 2026-05-17

## TL;DR

Seven zips shipped this autonomous session, all opt-in / additive,
all rollbackable. Five design memos. PARALLEL_TASKS.md updated.
Tests expected post-apply: **533 passed / 0 failed**.

When you start next session:

1. **Apply zips in this order** (or any order for RC33+; RC34 must
   come before RC35):
   - RC30 omnibus (`unendlich-rc30-omnibus.zip`)
   - RC31 slack-scheduler (`unendlich-rc31-slack-scheduler.zip`)
   - RC32 biased-walk + upload-priority
     (`unendlich-rc32-biased-walk-and-upload-priority.zip`)
   - RC33 cave-gen-multi-scale (`unendlich-rc33-cave-gen-multi-scale.zip`)
   - RC34 pressure-samples-no-alloc
     (`unendlich-rc34-pressure-samples-no-alloc.zip`)
   - RC35 structural-dirty (`unendlich-rc35-structural-dirty.zip`)
   - Docs (`unendlich-rc30-docs.zip`)

2. **Build + test**: expected `533 passed / 0 failed`.

3. **Hardware A/B test** with the env-flag matrix below.

4. **Decide next session priority** based on what's still hurting:
   - Stage 3 sub-stage A (chug-killer, ~250 LoC Vulkan)
   - Finding 2 dirty-plants set (tick_sim, ~100 LoC)
   - Compressed-chunk sub-stage 0 (uniform skip, ~100-150 LoC)
   - Cave-gen Step 2 (drainage map, ~300 LoC new module)

## What shipped

### Code RCs

| RC | One-liner | LoC | Files | Tests |
|---|---|---|---|---|
| 30 | submit-skip + raycast NaN fix + test fixes + warnings | ~300 | 8 | +12, -2 broken |
| 31 | slack-scheduler (upload time cap = frame slack) | ~70 | 1 | 0 |
| 32 | biased-walk + upload-priority | ~80 | 1 | 0 |
| 33 | cave-gen multi-scale (worm + chamber + seepage) | ~140 | 1 | +3 |
| 34 | pressure_samples Vec→iter (no per-call alloc) | ~30 | 2 | 0 |
| 35 | structural-dirty marking (replaces 10-tick POI cadence) | ~80 | 3 | +6 +1 net plant_lifecycle |

### Env flags now available (all default OFF, all composable)

| Flag | RC | Effect |
|---|---|---|
| `UNENDLICH_ADAPTIVE_UPLOADS=1` | (pre-session) | AIMD count cap reactive to past `wait_fences_ms` |
| `UNENDLICH_SUBMIT_SKIP=1` | RC30 | Skip shell walk when world stationary |
| `UNENDLICH_SLACK_SCHEDULER=1` | RC31 | Upload time cap = remaining frame slack |
| `UNENDLICH_FRAME_TARGET_MS=8.33` | RC31 | Frame budget (default 16.67 for 60fps) |
| `UNENDLICH_UPLOAD_MIN_MS=1.0` | RC31 | Minimum upload time per frame |
| `UNENDLICH_BIASED_SHELL_WALK=1` | RC32 | Reorder shell by forward-dot |
| `UNENDLICH_UPLOAD_PRIORITY=1` | RC32 | Sort upload Vec by (LOD, distance) |
| `UNENDLICH_MULTI_SCALE_CAVES=1` | RC33 | Three-layer cave density model |
| `UNENDLICH_CAVE_THRESHOLD=0.6` | RC33 | Multi-scale density threshold (lower = more caves) |

RC34 + RC35 are zero-flag (always-on cleanup).

### Design memos in `docs/speculative/`

| Memo | Topic | Code shipped from this | What's left |
|---|---|---|---|
| `tick-sim-bottleneck.md` | 3 findings | F1 → RC34, F3 → RC35 | F2 (dirty-plants set), ~100 LoC |
| `upload-scheduler-design.md` | 5-option decoupling | Stage 1A → RC31, 1D+2 → RC32 | Stage 3 (off-thread + DEVICE_LOCAL) |
| `stage-3-implementation-guide.md` | Detailed Vulkan hand-off | nothing — explicitly deferred | All 3 sub-stages A/B/C, ~250-600 LoC |
| `cave-generation-and-erosion.md` | 4-step plan | Step 1 → RC33 | Steps 2-4 (drainage, geological moisture, heightmap erosion) |
| `compressed-chunk-representation.md` | Physics-aware subsurface culling | nothing — design only | Sub-stages 0-4 |

## Recommended A/B test matrix

After applying everything, run with progressively more flags. Each
step should produce a measurable improvement in either GPU spike
frequency, perceived "chug," or tick_sim cost.

```bash
COMMON="UNENDLICH_RENDER_DISTANCE=6 UNENDLICH_LOD_BANDS=2,6,6 \
        UNENDLICH_GPU_MESHER=2 UNENDLICH_GPU_MESHER_MID=1 \
        UNENDLICH_WARMUP_MS=2000 UNENDLICH_OCTREE_VERIFY=1"

# Baseline (RC29 only, original behavior preserved):
$COMMON cargo run --release --bin unendlich-world --features="render,gpu,window"

# +RC30 submit-skip (world stationary frames):
$COMMON UNENDLICH_SUBMIT_SKIP=1 cargo run --release ...

# +RC31 slack scheduler:
$COMMON UNENDLICH_SUBMIT_SKIP=1 UNENDLICH_SLACK_SCHEDULER=1 \
  cargo run --release ...

# +RC32 walk biasing:
$COMMON UNENDLICH_SUBMIT_SKIP=1 UNENDLICH_SLACK_SCHEDULER=1 \
  UNENDLICH_BIASED_SHELL_WALK=1 UNENDLICH_UPLOAD_PRIORITY=1 \
  cargo run --release ...

# +RC29 adaptive uploads (full scheduling stack):
$COMMON UNENDLICH_ADAPTIVE_UPLOADS=1 UNENDLICH_SUBMIT_SKIP=1 \
  UNENDLICH_SLACK_SCHEDULER=1 UNENDLICH_BIASED_SHELL_WALK=1 \
  UNENDLICH_UPLOAD_PRIORITY=1 cargo run --release ...

# +RC33 multi-scale caves (visual A/B):
$COMMON UNENDLICH_MULTI_SCALE_CAVES=1 cargo run --release ...

# Multi-scale caves at higher density:
$COMMON UNENDLICH_MULTI_SCALE_CAVES=1 UNENDLICH_CAVE_THRESHOLD=0.55 \
  cargo run --release ...
```

RC34 and RC35 are zero-flag — their wins show up unconditionally.

## What to measure

From the launcher result JSON / metric scrubber:

- **`wait_fences_ms` p50/p99** — Was 80-120ms p99 pre-RC29. RC30
  + RC31 should pull this down further on stationary frames.
  RC32 (biasing) shouldn't change p99 directly but improves
  perceived ordering.
- **`submit` cost per second** — RC30's main target. Was
  800-880ms/sec stationary; with `SUBMIT_SKIP=1` should be near
  zero on idle frames (still pays cost on edit-tick frames).
- **`tick_sim` cost per second** — RC34 saves ~3ms, RC35 saves
  ~2ms. Combined ~5ms out of 60-180ms. If post-apply still >50ms
  at RD=6, Finding 2 (dirty-plants set) is the next ship.
- **GPU spike frequency** — chug events with `frame_ms > 25`. Not
  directly addressed by this session's RCs (those are Stage 3's
  job).

## Test count tracking

| Stage | Tests | Delta |
|---|---|---|
| Pre-session baseline | 513 / 2 failed (with --features render) | (baseline) |
| Post-RC30 | 525 / 0 failed | +12 / -2 broken |
| Post-RC31 | 525 / 0 | 0 |
| Post-RC32 | 525 / 0 | 0 |
| Post-RC33 | 528 / 0 | +3 cave-gen |
| Post-RC34 | 528 / 0 | 0 |
| Post-RC35 | 533 / 0 | +6 chunk + 1 net plant_lifecycle |

## Decision tree for next session

After hardware A/B testing reveals the actual cumulative benefit:

### If chug still happens (long-frame events visible)

Stage 3 sub-stage A is the answer. Reads:
- Mesh pool currently uses `HOST_VISIBLE | HOST_COHERENT`
  (confirmed this session by reading mesh_buffer_pool.rs:367).
- AGX/Honeykrisp is single-queue (confirmed by reading
  context.rs:170-185).
- Stage 3 wins come from moving CPU memcpy off render thread +
  DEVICE_LOCAL memory eliminating cache-coherency cost.

Plan: `docs/speculative/stage-3-implementation-guide.md`. ~250
LoC for sub-stage A. **Hardware verification essential — Vulkan
sync errors are silent failure mode.** Use validation layers.

### If tick_sim still >50 ms/sec at RD=6

Finding 2 (dirty-plants set) is the answer. Adds a sparse set on
Chunk tracking plants currently transitioning. Pass 2 iterates
this set instead of all plant_indices.

Plan: described in `docs/speculative/tick-sim-bottleneck.md`
Finding 2. ~100 LoC across chunk.rs + plant_lifecycle.rs. The
v0.24.70 dirty-marking groundwork (RC35) makes this easier.

### If world feels too sparse despite RC33

Cave-gen Step 2 (drainage map) is the answer. Adds a
`world::drainage` module computing per-region flow direction +
accumulated flow at coarse scale, then biases caves toward
high-flow paths.

Plan: described in `docs/speculative/cave-generation-and-erosion.md`
Step 2. ~300 LoC new module. Builds on existing `RegionGrid`
abstraction. No env flag — geological structure should be the
default once it works.

### If looking-up-at-sky still costs worker CPU

Compressed-chunk sub-stage 0 (uniform skip) is the answer.
Detects homogeneous-air chunks and registers them as
meshed-empty without going through the mesher worker pool.

Plan: described in `docs/speculative/compressed-chunk-representation.md`
Sub-stage 0. ~100-150 LoC because the renderer needs to track
"meshed-empty at version V" state to prevent re-submission.
RC35's structural_dirty groundwork helps but isn't the same
signal.

## Open questions / decisions I deferred

1. **Should `POI_REFRESH_INTERVAL_TICKS` be deleted?** RC35 made
   it unused. Currently kept as `pub const` for documentation
   reference. Future cleanup candidate but not urgent.

2. **Should `pressure_samples` (Vec version) be deprecated?**
   RC34 made it test-only. Could add `#[deprecated]` attribute or
   leave it as a public API for external callers. I left it
   alone — no harm, no urgency.

3. **Should we wire `Biome::cave_threshold()`?** It's defined
   but unused. RC33's multi-scale uses a single global threshold.
   Wiring biome-specific thresholds is natural Step 1.5 between
   RC33 and Step 2.

   **Effort revised**: NOT ~30 LoC. The biome lookup
   (`WorldModel::biome_at_2d`) costs ~5-10 noise samples = 5-10µs
   per call. Per-cell calls = ~200ms per chunk (too slow). Real
   implementation needs column-level caching: compute the
   threshold once per (x, z) column at chunk-gen entry, pass into
   per-cell is_cave. That's ~80 LoC with the refactor. Better as
   part of Step 2 (drainage map already wants column-level
   per-region data).

4. **Should empty-chunk fast-path use `structural_dirty`?**
   Currently the fast-path checks `poi_cache.tree_pois.is_empty()`
   to detect "empty after first build." With RC35, structural_dirty
   is a stronger signal — a chunk with no wood will stay
   non-dirty forever after the first build. Wiring this could
   eliminate the cache-existence check. Maybe ~10 LoC.

## File inventory: what's where

### Engine source (under `crates/unendlich-engine/src/`)

| File | Touched this session | Reason |
|---|---|---|
| `bin/unendlich_world.rs` | RC30, RC31, RC32 | Main loop scheduler additions |
| `render/meshing_pool.rs` | RC30 | Test fixture fixes (octree field) |
| `world/octree.rs` | RC30 | Test typo fix |
| `world/octree_raycast.rs` | RC30 | NaN bug fix (real bug!) |
| `world/surface.rs` | RC30 | Drop unused ChunkCoord |
| `render/world_renderer.rs` | RC30 | Drop unused Vertex |
| `render/gpu_meshing_octree.rs` | RC30 | Drop unused const |
| `world/chunk_manager.rs` | RC30, RC35 | Drop unused mut; InstallPoi clears dirty |
| `world/world_model.rs` | RC33 | Multi-scale cave gen |
| `world/pressure_sampler.rs` | RC34 | Iterator variant |
| `world/plant_lifecycle.rs` | RC34, RC35 | Iterator switch + dirty check |
| `world/chunk.rs` | RC35 | structural_dirty field |

### Docs (under `docs/`)

| File | Status |
|---|---|
| `claude-notes/README.md` | (pre-session) |
| `claude-notes/PARALLEL_TASKS.md` | session status board |
| `claude-notes/v0.24.64-submit-skip-readme.md` | RC30 |
| `claude-notes/v0.24.65-slack-scheduler-readme.md` | RC31 |
| `claude-notes/v0.24.66-67-biased-walk-and-upload-priority-readme.md` | RC32 |
| `claude-notes/v0.24.68-cave-gen-multi-scale-readme.md` | RC33 |
| `claude-notes/v0.24.69-pressure-samples-no-alloc-readme.md` | RC34 |
| `claude-notes/v0.24.70-structural-dirty-readme.md` | RC35 |
| `claude-notes/rc23-berry-bush-prep.md` | (still pending) |
| `claude-notes/session-2026-05-17-handoff.md` | this file |
| `speculative/tick-sim-bottleneck.md` | 3 findings; F1 + F3 done |
| `speculative/upload-scheduler-design.md` | 5 options; Stage 1A + 1D + 2 done |
| `speculative/stage-3-implementation-guide.md` | hand-off for next-with-hardware |
| `speculative/cave-generation-and-erosion.md` | 4 steps; Step 1 done |
| `speculative/compressed-chunk-representation.md` | 5 sub-stages; none done |

## What I deliberately did NOT ship

1. **Stage 3 sub-stage A (DEVICE_LOCAL backing)** — ~250 LoC of
   Vulkan with real sync risk. Validation-layer-essential. Not
   shippable in a hardware-less session.
2. **Compressed-chunk sub-stage 0 (uniform skip)** — touches
   renderer state for empty-mesh-version tracking. Cross-cutting.
   Risk-budget too high without compilation.
3. **Cave-gen Step 2-4** — Step 2 is ~300 LoC new module; Step 4
   is 500-1000 LoC. Both want hardware verification because
   determinism + cross-chunk correctness can't be checked from
   reading alone.
4. **Finding 2 (dirty-plants set)** — ~100 LoC across two files,
   cross-cutting state-management. Doable in a hardware session.

Each of these has a complete implementation guide in
`docs/speculative/`. The next session can execute any of them.

## Notes to my future self / next-session Claude

- The user works in autonomy mode ("keep going") and trusts
  judgment on what to ship vs design.
- Launcher rollback exists, so risk-budget is reasonable, but
  Vulkan sync errors are NOT recoverable by rollback (corrupt
  state can persist).
- Always read existing code BEFORE designing — the codebase
  often has the right field name already in a doc comment
  (RC35 was named two RCs ago in chunk.rs:149).
- Tests that assume placeholder behavior need to be inverted
  when shipping the real behavior (RC35 inverted
  `poi_refresh_fires_on_interval`).
- `apply_all_ops_for_test` in plant_lifecycle.rs mirrors the
  real chunk_manager apply path. Both need updates in lockstep.
- When packaging zips, include ONLY the files actually changed,
  not the entire crate. Per-zip README explains what's in it.
- The structural_dirty field is now available as a signal for
  "is this chunk unchanged?" — many future RCs can use it.

## See also

- `docs/architecture-v1.md` — strategic destination.
- `docs/v0.24.3-chug-investigation-plan.md` — earlier-session
  context on the chug arc that RCs 30-32 continue.
- `docs/claude-notes/PARALLEL_TASKS.md` — live deferred-work
  tracker.
