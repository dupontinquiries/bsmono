# Known test failures (non-GPU lib suite)

## Current state (2026-09-24, c20unified): 8828 passed, 0 failed, 12 ignored

`cargo test -p unendlich-engine --no-default-features --lib` builds and runs in a 3 GB
sandbox **with one build job** (`CARGO_BUILD_JOBS=1`): the test crate's rustc peaks near
2.6 GB. That is almost certainly why earlier sessions reported the test crate "dying" and
fell back to `cargo check --lib` -- and why the four failures below went unnoticed.

Four failures on first run, all fixed:
- `shader_artifacts::build_list_tracks_every_source_shader` and
  `core_shaders_ship_precompiled_spirv`: an uncompiled proposal shader committed into
  `src/shaders/` (`a2e8568a9`); moved to `shaders_proposed/` (`08ffc680f`).
- `world::tree_form::glsl_transliteration_matches_rust_exactly`: the Rust Dome canopy became
  round on top (`77b9ead7f`) and the GPU generator's GLSL twin stayed a cone -- one seed built
  different trees on the CPU and GPU paths. Shader fixed and its `.spv` regenerated
  (`ad9f3d4e6`).
- `mobile_ui::tests::pointer_adapter_owns_a_button_press_and_routes_the_rest_to_touch`: the
  test pinned the latch that `35d2285ca` removed (the stuck mine button); now pins the
  recovery (`2bbc77224`).

### Update (c20unified, leaf templates merged): 9372 passed, 3 failed (default features)

- `world::mesh_tri_regression::sample_grid_worst_case_stays_under_budget` -- still over, much
  less: the worst sample chunk was 443,922 triangles; canopy depth (`e92b8b962`) and merged
  leaf templates (411 -> 160 quads per leaf block, no visual change) bring that chunk to
  181,430, and the grid's worst is now 213,550. Two sources remain: leaf stamps (512 x ~320)
  and, independently, weathered-top subdivision (chunk (0,2,0): 55,296 quads, ~110K triangles,
  no leaves involved). The 90K budget is self-described as uncalibrated.
- `render::mesher::tests::isolated_soft_block_sags_more_than_buried` -- a MASKED failure, now
  visible. It averaged every vertex over the block, grass tufts included, and the tufts on the
  buried block carried it past its 0.10 threshold. Measured on the block's own top surface
  (undecorated, the lowest point -- the corner the note says sags ~0.225): isolated 8.786
  (sag 0.214, as the note says) but buried 8.837 (sag 0.163), where the test expects "near
  9.0". The claim that a buried soft block barely sags does not hold for the current mesher;
  a soft-block question, not a template one. The threshold was NOT lowered.
- `render::mesher_binary::tests::cross_tufts_are_eight_vertices_and_double_sided` -- as below.

### With the DEFAULT features (render tests included): 9208 passed, 4 failed of 9212

Once the engine's default build compiled again (f19420473), its render tests ran for the first
time since at least 2026-09-12 (`cargo test -p unendlich-engine --lib`, one build job: rustc
peaks near 2.98 GB). Of the four failures:
- `render::meshing_pool::tests::pool_dispatches_far_lod_to_downsampled_mesher` and
  `pool_takes_virtual_path_when_generator_attached`: stale -- they read `result.mesh`, which
  `pack_and_shed` (cb251c389) empties on purpose; the geometry is in `result.packed`. Fixed
  (d7bc95360).
- `world::mesh_tri_regression::tests::sample_grid_worst_case_stays_under_budget`: **real, left
  failing on purpose.** 443,922 triangles in one chunk against a 90,000 budget, ~410K of them
  foliage: ~800 triangles per stamped leaf block, 512 stamps allowed per chunk. Measured and
  explained in 79ab94223; the fix is a detail-versus-cost decision (the stamp cap, culling
  faces hidden inside templates, or skipping enclosed leaves).
- `render::mesher_binary::tests::cross_tufts_are_eight_vertices_and_double_sided`: **open.**
  With `TuftStyle::Cross` on a 5-block grass slab no tuft is emitted. The binary mesher is off
  by default (`UNENDLICH_MESHER=binary`), so the shipped game is not affected. Hypothesis, NOT
  confirmed: `decorate_chunk_styled` charges the per-chunk stamp budget (512) before its
  surface / air-above / probability checks, so the slab's 512 lowest (buried) grass blocks
  spend the whole budget and the surface layer is never reached. If so, the budget counts
  candidates, not stamps, which its own doc (`plant_stamps`: "plants actually given ...
  detail") says it should not.

`sim::smoke_gpu::pure_decay_monotonic_decrease` (section 2 below) PASSES in this run. No
commit in this repository touches it after this document was written, so the fix, if it was
one, predates the in-tree landing (349acb36e); the section is kept as history.

---

Surfaced 2026-05-29 once a Rust toolchain became available in-sandbox and the
full `cargo test -p unendlich-engine --no-default-features --lib` could run
(896 tests). Of the original 4 failures, 2 were fixed immediately
(`surface_y_finds_terrain` → cave-threshold recalibration;
`advances_sixty_ticks_per_second_regardless_of_refresh` → alpha f32-rounding
clamp). A third (`sim::foveation::scatter_error_shrinks_with_budget`) was fixed
2026-05-29 — see below. The remaining 1 is tracked here. None exists at base
`88b5542`; all were added by intermediate branch work and asserted a property
the implementation doesn't satisfy as originally written.

## 1. `sim::foveation::scatter_error_shrinks_with_budget` — FIXED 2026-05-29

The test asserted that a coarser foveation budget is never *more* accurate than
a finer one **for a single column**: `err(2) >= err(8) >= err(32)`. That
invariant is mathematically false: `integrate_foveated` reconstructs the
scatter integral from `b` merged Z-groups, and like any Riemann-sum refinement
a coarse partition can land closer to the exact integral than a finer one by
coincidence (aliasing). The transmittance channel *is* exact at any budget
(`transmittance_exact_at_any_budget` passes); only the scatter channel is an
approximation.

Resolution: the meaningful, true property — the one foveation actually relies
on — is that error decreases with budget **in aggregate** over the many columns
a frame integrates, not per single column. The test now averages absolute
scatter error over 64 distinct media columns and asserts (a) full budget
(`b == nz`) is exact (≤1e-4) and (b) mean error is non-increasing as budget
rises. This holds and is the correct statement of the invariant. The integrator
was not changed (its per-group exact-τ / length-weighted-source math is sound).

## 2. `sim::smoke_gpu::pure_decay_monotonic_decrease`

Steps the CPU smoke reference 20 times with `decay = 0.7` and asserts (a) total
mass never grows (passes) and (b) `prev < 1.0` at the end —
`should have decayed substantially`. Fails on (b): after 20 steps the mass has
not dropped below 1.0.

Likely cause: either the initial mass is larger than the test author assumed,
or `step_smoke`'s decay term isn't applied per-step the way the assertion
expects (e.g. decay is a *retention* factor vs a *loss* factor, or it's gated
by a dt the test doesn't pass). The monotone-decrease half passing while the
magnitude half fails points at a rate/scaling mismatch, not a sign error.
Needs a smoke-subsystem pass to confirm the intended decay semantics, then
either fix `step_smoke` or correct the test's expected magnitude.

## Running the suite

```
cargo test -p unendlich-engine --no-default-features --lib
```

`--no-default-features` drops the `gpu`/`ash` dependency so the pure-logic
tests build without a Vulkan loader. The render/GPU paths still require the
full feature set + hardware and are not covered here.
