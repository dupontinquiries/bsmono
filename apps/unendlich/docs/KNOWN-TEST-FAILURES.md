# Known test failures (non-GPU lib suite)

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
