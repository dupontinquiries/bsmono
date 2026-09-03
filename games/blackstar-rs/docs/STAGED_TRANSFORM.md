# The staged transform

A six-stage decomposition of bot → vehicle, and what each stage needs
from the code. Companion to [STOWING.md](STOWING.md), which measures why
a single-step fold cannot work.

## The stages

1. **Bot form.**
2. **Get the skeleton into position.**
3. **Pull the skeleton to hoist state.**
4. **Stop being a skeleton** — detach limbs, do folds, rotate pieces,
   smaller body parts start moving.
5. **Large cube innards → smaller cube innards.** Pieces are
   high-technology and appear to *fuse into the same volume*.
6. **Vehicle assembly** — pull in the vehicle components, pop away the
   helpers, assemble the shell.

## Why this decomposition answers the measurement

[STOWING.md](STOWING.md) found the bay is roughly an order of magnitude
smaller than the limbs, and that `skeleton::compact` conserves volume
exactly, so no placement strategy closes that gap.

**Stage 5 is where the order of magnitude comes from, and making it an
explicit stage is what makes the pipeline honest.** It is not physics and
does not pretend to be — it is a declared conceit with a visual language
("high technology", fusion) attached. Everything before it can be
rigid-body motion that obeys ordinary rules, and everything after it can
assume the volume problem is solved. A single-step fold has to smuggle
the same trick in invisibly, which is why it reads as parts
interpenetrating.

The required factor is measurable per chassis: `fold_audit --cavity`
reports `cavity / limb volume`, currently 0.03–0.12, so stage 5 has to
supply between 8x and 30x. **That is a number a design can be held to**,
and it is why the caveat in STOWING.md about the cavity figure not having
converged matters — it sets the target.

## What already exists

More than expected. Checked, not assumed:

**Stages are authorable as poses, today.** The loader enumerates pose
directories with `read_dir`, sorts them, and accepts any count ≥ 2
(main.rs:1922). Chassis currently ship five (`f00`..`f04`). A seven-pose
chassis for a six-stage transform needs no code change.

**Detach and pop are already implemented.** `align::unify` builds a union
prim list across the strip: where a part is absent from a pose, a
placeholder holds the nearest present copy *collapsed to its centroid*.
A straight per-index lerp then makes appearing parts scale in from a
point and departing parts scale out — and it is C0 at every pose
boundary, so there is no pop. That is stage 4.1 (detach limbs) and stage
6.3 (pop away helpers), working now.

**Scale-about-centroid is a volume change.** The same mechanism that
shrinks a departing part to nothing can shrink a part to something
*smaller but present*. Stage 5's fusion is expressible in the existing
lerp as authored geometry — the innards get smaller across a segment —
without new machinery.

**Per-stage timing exists as segments.** `xform::Morph` lerps piecewise
across the strip, so each stage already has its own segment of the
timeline.

**The vehicle-end stow exists.** `skeleton::fold_pack` and the `tuck`
crossfade are stage 6.1.

## What is missing

**~~Stage duration is uniform.~~ DONE.** `xform::stage_time` remaps the
fold parameter so each segment gets its own share, set by
`Morph::stage_weights`. `None` is uniform and reproduces the previous
mapping exactly, so no existing chassis changes. Degenerate weights fall
back to uniform rather than distorting the fold: a transform that runs
evenly is a tuning mistake someone notices, one that jumps or stalls
looks like a crash. Seven tests, including monotonicity — a fold that
goes backwards mid-transform reads as a glitch.

Nothing sets the weights yet. That is the energy budget below, and it is
now a matter of choosing numbers rather than writing machinery.

**Rotation during a segment travels a chord.** The blend is per-vertex
linear, so a piece that rotates as it moves cuts across the arc instead
of following it. `fold_audit`'s excursion metric already measures exactly
this and finds 8 of 254 chassis with a part leaving the body mid-fold,
worst at t=0.50 — the midpoint, which is where a chord deviates most.
**Stage 4.3 (rotate pieces) will hit this hard**, and the audit is
already the regression test for it.

**Fusion needs to read as fusion.** Geometry shrinking is a scale;
"magically fusing into the same volume" is a shading and VFX problem.
Nothing in the renderer does this today.

**There is no energy concept at all.**

## The energy budget

Making unfold speed cost energy fits this decomposition better than it
fits a single fold, because the stages have genuinely different costs:

- Stages 1–3 are rigid-body posing and are cheap.
- Stage 4 is the expensive mechanical work.
- Stage 5 is the high-technology step and is where "spend more to go
  faster" is most legible — it is already the stage that is asking the
  audience to accept something.

A cheap first prototype is a per-segment duration curve driven by a
single energy scalar, with no new geometry: high energy compresses stages
4 and 5, low energy stretches them. That alone would give the tactical
choice its shape, and it can be built before any shard physics exists.

## Suggested order

1. ~~Per-segment durations.~~ Done — `xform::stage_time`.
2. **A seven-pose test chassis** authoring the six stages, to find what
   the strip cannot express before building anything for it.
3. **Watch stage 4.3 with `fold_audit --worst`.** Rotation is where the
   chord problem bites, and there is already a number for it.
4. **Stage 5's visual language.** The geometry is authorable now; the
   fusion read is not.
5. **Shard physics**, if the shard route from STOWING.md is taken — the
   most expensive item, and the one the other four do not depend on.
