# B1 — Bughunt report (r102)

Pure-core correctness hunt across `sim/` and `world/`. One real defect fixed with
a fail-before/pass-after regression test; the rest of this file records the
modules audited and found robust (honest negatives), so the next hunt doesn't
re-walk them.

## BUG FIXED — stale mesh upload survives a world reset (no epoch guard)

**Where:** `world::chunk_manager` (`reset_with_generator`, the F4 `rebuild_for_seed`
path) + the async mesher/renderer that consumes mesh results.

**Root cause.** `reset_with_generator` swaps in a new `TerrainGenerator`, rebuilds
the chunk loader, and clears `chunks` / `octrees` / `chunks_with_transitions` so
the world regenerates from the new seed. Meshing is asynchronous, so mesh jobs
issued for the *previous* world can still be in flight when the reset lands. When
such a job completes, its result is geometry for a world that no longer exists —
but nothing distinguished a late, stale result from a fresh one. The renderer
could therefore briefly upload stale geometry into the freshly-regenerated world
(a flash of the old terrain at a chunk coordinate that now holds something else).
There was no epoch/generation concept anywhere in `chunk_manager` (confirmed by
grep) to tell the two apart.

**Fix (pure, in-core).**
- `ChunkManager` gains a monotonic `world_epoch: u64`, initialised to `0`,
  **bumped on every reset** (`reset_with_generator`, which `rebuild_for_seed`
  calls), exposed via `pub fn world_epoch(&self) -> u64`.
- New pure module `world::mesh_epoch` with the decision point the renderer/mesher
  uses: `mesh_result_is_current(result_epoch, current_epoch) -> bool` (accept iff
  equal) plus a `WorldEpoch` newtype with a **saturating** `bumped()` (so a
  practically-impossible overflow can never wrap a fresh world back onto an old
  epoch and silently re-accept stale geometry).

**Tests (fail-before / pass-after).**
- `world::chunk_manager::tests::reset_bumps_world_epoch` — epoch starts at 0 and
  strictly increases across both `reset_with_generator` and `rebuild_for_seed`,
  and the `mesh_epoch` gate rejects a pre-reset epoch while accepting the current
  one. *Fail-before:* `world_epoch()` did not exist and no bump occurred, so the
  staleness was unobservable — the test could not even be written against the old
  API; with the old behavior a late result was indistinguishable from fresh.
- `world::mesh_epoch::tests` (4): `accepts_current_rejects_stale`,
  `bumped_is_strictly_increasing_and_saturating`, `typed_gate_matches_raw`, and
  `models_the_defect_a_late_result_after_reset_is_dropped` (an issued-then-reset
  job's result is dropped; a post-reset job is accepted).

**Handoff (GPU side — cannot compile in the worker sandbox, propose only).**
The mesh pipeline should: (1) record `mgr.world_epoch()` when a mesh job is
*issued* (store it on the job/result), and (2) on completion, drop the result
when `!world::mesh_epoch::mesh_result_is_current(job_epoch, mgr.world_epoch())`
before it reaches the upload path in `render::world_renderer`. The epoch only
ever changes in `reset_with_generator`, so the check is a single integer compare
on the result-intake path. No descriptor/pass changes.

**Verification (run in-sandbox):**
- `cargo test -p unendlich-engine --features gpu --lib mesh_epoch` → 4 passed.
- `cargo test -p unendlich-engine --features gpu --lib reset_bumps_world_epoch` → 1 passed.
- `cargo test -p unendlich-engine --features gpu --lib reset_with_generator_can_round_trip_seeds`
  → 1 passed (no regression to the existing reset behaviour).
- `cargo check -p unendlich-engine --features render` → Finished (render gate; pre-existing warnings only).

---

## Audited and found CORRECT (documented negatives)

These were checked against the bug families the brief named and found robust;
recording them so the hunt isn't repeated.

- **Noise lattice floors** (`world::ridge_noise`, `terrain_warp`,
  `batch_ridge_noise`, `batch_terrain_warp`, `scatter_cluster`): all compute
  `x0 = x.floor()` *before* `x0 as i32`, so the lattice cell and the fractional
  part are correct at negative coordinates (no trunc-toward-zero seam at x=0).
- **`Chunk::block_index(...) as u16`** (`world::plant_lifecycle`): safe —
  `CHUNK_VOLUME = 32³ = 32768 < 65536`, so the index never truncates.
- **World↔chunk/local coordinate math** (`world::chunk`): uses `div_euclid`/
  `rem_euclid` (and masks), not sign-preserving `/`/`%`; correct at negatives.
- **Determinism of gameplay iteration**: `sim::neighbor_grid` iterates fixed
  `dx/dy/dz` ranges + insertion-ordered `Vec`s (never `HashMap` order, per its own
  doc); `sim::world_authority` deliberately uses `BTreeMap` for deterministic
  iteration; `sim::lightning`'s Dijkstra expands a fixed `NEIGHBORS6` array.
- **LOD boundary math** (`world::lod::desired_lod_for`, `lod_bands_for_rd*`):
  Chebyshev distance with inclusive `<=` bands, clamped to render distance, bands
  strictly increasing — no off-by-one.
- **NaN/divide guards**: `sim::boids` *and* its SoA twin `sim::batch_boids` both
  guard separation with `if d > EPS`; `sim::herd_cohesion`, `scent_gradient`,
  `env_deform` normalise with zero-guards; `sim::fluid`'s surface normal has a
  constant `+1.0` y-term so its length is always ≥ 1 (never divides by zero).
- **`mob_memory`** (decay/remember/recall/eviction): salience stays finite and
  non-negative, capacity is bounded, recall returns the most-salient in radius —
  all covered by existing property tests.
- **Unsigned tick deltas**: `sim::ability_cooldown::cd_remaining` is `f32` and
  explicitly clamps clock-skew (`now < last_cast`); `sim::desync_monitor::annotate_frametime`
  guards `e.tick < start_tick` *and* bounds the index. No underflow.
- **`physics`** is fixed-point (`Fixed`), so the f32 NaN/overflow family does not
  apply to the character/world physics step.
