# HANDOFF — hh-r332-t1 mesh-build budget

**WID:** `hh-r332-t1` · **Status:** pure core VERIFIED in-sandbox (14/14, rustc 1.96.0).
**Coordinator action:** add the two `runtime_config` knobs (block below) and call
`sim::mesh_budget::admit` at the near-mesh queue drain in `world::chunk_manager`.
**Base note:** built against the `r330b` tree (the dispatch base is `r332-merged`,
which I don't have). The core is a self-contained new file so it merges cleanly;
the `runtime_config` + `chunk_manager` edits below are given as **proposals to
apply onto r332**, since those shared files may have moved — I did not blind-edit
them against the wrong base.

---

## What landed (verified, pure)
`crates/unendlich-engine/src/sim/mesh_budget.rs` (+1 `pub mod` line in `sim/mod.rs`).

API:
```
pub const NUM_LOD, COST_SETUP_US, COST_PER_CELL_NS
estimate_cost(solid_cells: u32, lod: u8) -> u32        // μs proxy, monotone, saturating
struct MeshBudget { max_meshes_per_frame: u32, build_budget_us: u64 }
  ::DEFAULT_MAX_MESHES_PER_FRAME = 8
  ::DEFAULT_MESH_BUILD_BUDGET_MS = 3.0
  ::default_budget()                                   // 8 / 3000 μs
struct PendingMesh { distance: u32, cost_us: u32 } + ::new
admit(pending: &[PendingMesh], budget: MeshBudget) -> Vec<usize>   // indices to build now
```
Guarantees (all unit-tested): count cap respected; cost cap respected except a
lone oversized nearest chunk (progress guarantee); nearest-first, stable/
deterministic; strict priority (never skips a near chunk to pack a far one);
empty/zero-cap → empty; no permanent starvation (queue always drains).

Gate: `rustc --edition 2021 --test mesh_budget.rs` → **14 passed / 0 failed**.

## Why a per-frame *build* budget (not just uploads)
Uploads are already capped (`max_uploads_per_frame`, `upload_budget_ms`), but the
CPU greedy mesh **build** is not — `mesher_timer` near tier p99 104.5 / max 1020.8
ms is one chunk building synchronously inside the drain loop. `admit` bounds how
much build time the drain spends per frame and defers the rest, converting a 640
ms spike into a few-frame fill-in.

---

## (1) runtime_config knobs — additive block to apply

In `RuntimeConfig` (the **live-applicable** group, next to `max_uploads_per_frame`
/ `upload_budget_ms`):
```rust
    // hh-r332-t1: per-frame near-mesh BUILD budget (uploads are already capped;
    // the CPU greedy mesh build was not — see mesher_timer near tier).
    pub max_meshes_per_frame: u32,   // hard count cap on near meshes built per frame
    pub mesh_build_budget_ms: f32,   // cumulative build-cost budget per frame
```
In `from_env()` (mirrors the `u32v` / `f32v` upload-knob seeding exactly):
```rust
    max_meshes_per_frame: u32v("UNENDLICH_MAX_MESHES_PER_FRAME", 8),
    mesh_build_budget_ms: f32v("UNENDLICH_MESH_BUILD_BUDGET_MS", 3.0),
```
Also add both to any other `RuntimeConfig { .. }` literal (`preset()` etc.) — both
are *live* knobs, so presets can scale them (e.g. BatterySaver → 4 / 2.0,
Ultra → 16 / 5.0). Convert to a `MeshBudget` at the call site:
```rust
let budget = sim::mesh_budget::MeshBudget {
    max_meshes_per_frame: cfg.max_meshes_per_frame,
    build_budget_us: (cfg.mesh_build_budget_ms * 1000.0) as u64,
};
```
`mesh_budget` carries the same defaults (`MeshBudget::default_budget()`), so the
core is testable before the config lands and the values can't drift.

## (2) chunk_manager near-mesh drain — call-site

The drain loop in `ChunkManager::update` (`world/chunk_manager.rs`, the
`drain_completed_iter` / re-mesh region ~line 748 onward, and the
transition-remesh path gated by `transition_remesh_due`) currently builds every
ready near mesh inline. Proposed change (do NOT also remove the upload budget —
this stacks in front of it):

1. Instead of meshing each ready near chunk immediately, push a `PendingMesh`
   into a per-frame scratch `Vec`:
   - `distance`: squared block distance player→chunk (the manager already computes
     a priority/distance for `request_visible`; reuse it). Optionally subtract an
     in-view discount so on-screen chunks sort first.
   - `cost_us`: `estimate_cost(solid_cells, lod)` — the mesher exposes a solid-cell
     count (`render::mesher` keeps `solid_count`; surface it as a cheap
     `Chunk::solid_cells()` or pass the count the loader already has). `lod` =
     `LodLevel::index() as u8`.
2. Once collected: `let build_now = mesh_budget::admit(&pending, budget);`
3. Build/upload only the chunks at `build_now` indices this frame; leave the rest
   queued (they reappear next frame and re-rank — a chunk that got nearer is built
   sooner; an oversized one is eventually admitted alone).

Interaction notes:
- **T3 (`mesh_partition`)**: once a neighbour event re-meshes only 1/6 of a chunk,
  feed `estimate_cost` the *sub-mesh's* solid-cell share, not the whole chunk —
  the two land together cleanly (a partial remesh is a cheap `PendingMesh`).
- **`transition_remesh_stride`**: the stride throttle and this budget are
  complementary (stride limits *how often* a transitioning chunk remeshes; the
  budget limits *how much* total build per frame). No conflict; the budget can let
  the stride relax once T3 makes each remesh cheaper.
- Keep `MGR_*` timing accumulators around the admitted builds so the next M2 run
  shows the budget holding the near tail down.

## Verify on-device
1. `cargo test -p unendlich-engine --lib mesh_budget` (pure; should mirror the 14
   standalone tests).
2. After wiring: a fast-descent run (the stutter1 repro) — near-tier max should
   drop from ~1020 ms toward `mesh_build_budget_ms`-bounded multiples; world fills
   in over a few frames instead of hitching. Confirm with `frame_correlation`
   `vvel` that the old spikes correlated with descent.

## Honesty
- Pure core only; no IO/clock/RNG/`unwrap`/`expect`/`panic!`; saturating arithmetic.
- `estimate_cost` is a **coarse proxy** (setup + per-solid-cell, halving per LOD
  tier), not a calibrated timing model — it only needs to *rank* and *bound*. The
  M2 can recalibrate `COST_PER_CELL_NS` against `mesher_timer` without touching the
  scheduler; the unit tests pin behaviour (monotonicity, saturation), not absolute μs.
- `admit` is alloc-light (one index-sort scratch + the result vec, both ≤ queue len),
  not alloc-free — it runs once per frame on the near queue, not per chunk.
- No `runtime_config` / `chunk_manager` edits shipped (base-mismatch caution); both
  are proposals above for on-device application.
