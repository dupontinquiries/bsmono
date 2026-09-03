# fold_audit

`examples/fold_audit.rs` — measures a transform's geometry across the
whole fold, and the vehicle-mode silhouette at rest.

    cargo run --example fold_audit -- <bots dir> --sweep-all
    cargo run --example fold_audit -- <bots dir> --worst 8
    cargo run --example fold_audit -- <bots dir> --unify
    cargo run --example fold_audit -- <pose dir> --cavity
    cargo run --example fold_audit -- <pose dir> --colors

## Not the only one

[`src/foldaudit.rs`](../src/foldaudit.rs) already measures a fold —
self-intersection, volume collapse, velocity spikes, NaN — with nine
tests, and is used by `foldsim` and `examples/refusal_audit.rs`. **Run
that one first.** It is older, tested, and integrated.

This harness answers different questions: where parts travel, whether
they float free, how much robot shows in vehicle mode, and whether the
hull has anywhere to put it. They are complements, and this one exists
partly because its author did not follow up a grep hit that pointed at
the other.

## The loader rule

Every geometric measurement here goes through `load_strip`, which
mirrors main.rs's `build_morph`: `load_raw_at` for the span, then
`load_raw` at **one shared scale** across the strip.

This matters more than it sounds. The harness originally used
`load_raw_unoriented` — neither oriented nor consistently scaled — and
every number it produced described a pipeline the game does not run. It
was internally consistent and reproducible the whole time. The
contradiction only surfaced when `refusal_audit`, which mirrors the real
build, reported five unify refusals against this harness's zero.

`dump_colors` is the single exception and says so: a prim's colour does
not depend on orientation.

## Metrics, and how far to trust each

| metric | status |
|---|---|
| `--unify` refusal count | **trustworthy** — a direct call, no derived geometry |
| excursion | **trustworthy** — validated against a fixture with a known answer |
| island | trustworthy as a *ranking*; the absolute gap depends on prim density |
| exposure | **indicative only** — see below |
| cavity | **not converged** — see below |
| chord error | **computed, deliberately not printed** — see below |

### excursion

How far past its own endpoint offsets a prim's centroid travels
mid-fold, in body radii. Nine of 249 chassis are over the flag, most at
t=0.50 exactly.

Validated: `xform::staged_pipeline` builds a 90° swing and measures the
midpoint at 0.707 of its radius — 29% inside the arc — which is what a
chord does to a quarter circle. The fleet numbers and the fixture agree
on mechanism.

### exposure

Fraction of limb samples visible from outside, by depth test over three
orthographic views. Fleet median 63%.

Two caveats, both real:

- **"Limb" is whatever `skeletonize` separates from the core, which is
  not "robot part".** On a car, wheels and wings are limbs and are
  supposed to be visible. A chassis whose vehicle pose is already a robot
  scores high by definition. `--worst` prints this warning above its own
  output.
- Three earlier versions of this metric were wrong in the same direction
  (they flattered the code). The current one at least agrees with a
  render on the case that motivated it.

### cavity

`--cavity` voxelises the hull, flood-fills from outside, and reports the
enclosed interior against limb volume. **The number has not converged:**
0.07 at a 40³ grid, 0.12 at 64³, still climbing, because coarse
voxelisation leaks and finer grids find more enclosed space. Push the
resolution before acting on the magnitude. The ordering across chassis is
more stable than the value.

### chord error

An estimate of what a polar blend (direction and radius blended
separately) would recover. Computed and **not printed**, because three
confounds are uncontrolled: placeholder prims collapsed to a point, a
body centroid that drifts between poses, and radial motions for which the
chord is the correct path. The trustworthy number for the chord problem
is the fixture one above.

## Why so many caveats

Six metrics, four of which needed correcting after they were first
reported. The pattern across the session was consistent: **measurements
built on a fixture with a known answer held up; measurements taken
straight across the fleet needed rework every time.** Where a number here
is trusted, it is because something with a known answer agrees with it.
