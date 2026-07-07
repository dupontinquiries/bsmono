# Timeline Branching & Timebending

> See also `timebending_set_algebra.md` — the branch DAG here, viewed as rebase + bounded set algebra (intersection/union/difference over region×tick), and the content-addressed/structurally-shared data model that makes branch cost proportional to divergence.

> See also `statistical_timebending_and_storage.md` — pushing timebending to deep-time scrub via statistical/residual tick-collapse, with the dual-path (fast vs long-form) benchmark, and the storage/IO consequences.

> Part of the menu/sim work — see `MENU_SYSTEM.md` for the overview + status.


A new sim module — `crates/unendlich-engine/src/sim/timeline.rs` — that
layers **branching timelines** on top of the rollback model so a
*timebender* can reach into the deep past, edit the world, and spawn a new
history that supersedes the old one. Pure, deterministic, headless-tested.

## The idea

Linear rollback (`sim/rollback.rs`) keeps ONE trusted timeline and re-sims
it forward when a *present* input corrects a *recent* predicted tick — the
resim *replaces* the old one because it's strictly more correct.

Timebending is the same machinery taken to the extreme, with one twist: a
deliberate edit to the deep past is a **divergence**, not a correction. The
pre-edit future genuinely happened, so we keep it. The edit therefore
**forks a new branch** at the edit tick, the parent's downstream becomes
**stale** (kept as dead history), and the branch becomes the live world.

```
  root:  o──o──o──o──o──o──o   (was head)
                │
                └──X──o──o──o   (branch, LIVE)     X = "build a tower at year 0"
  root ticks after the fork are now STALE.
```

## Model (`Timeline`)

A `Timeline` is a tree (really a DAG of fork points) of `Branch`es; branch 0
is the original un-bent timeline.

- `Branch { id, parent: Option<(BranchId, Tick)>, edit: Option<Structure>,
  start, head, stale }` — where it forked from + when, the edit that spawned
  it, its lived span `[start, head]`, and whether it's been superseded.
- `Timeline::new(start, head)` — a fresh single-branch history.
- `timebend(from, at_tick, edit) -> Option<BranchId>` — **the core op.** Go
  back to `at_tick` on branch `from`, place `edit`, fork a new live branch.
  Rejects an `at_tick` outside the parent's lived span. "Build a structure
  1000 years ago" = `timebend(live, head - 1000*ERA, Structure::tower(..))`.
- `advance(id, ticks)` — simulate a branch forward (its head moves).
- `project_structure(id, ticks_per_era)` / `project_live_structure(..)` —
  take the branch's edit and erode it once per era of the branch's age,
  returning the weathered structure *as it appears now*.
- `lineage(id)` — the root-first chain of fork points (for visualising).
- `live()`, `branch_count()`, `branch(id)`, `branches()`.

### Staleness + looping

Forking invalidates the parent's *future past the fork*, and the invalidation
**cascades**: any branch rooted on that now-dead segment is transitively
stale (`mark_stale_below`). Because a fork can target any tick on any branch —
including an ancestor — histories loop and nest into a DAG rather than a
single line. A fork exactly at a branch's head is a clean continuation and
does NOT staleify the parent (there was no future to kill).

## Decay / erosion

A structure raised in the deep past must arrive weathered. `Structure` holds
a small heightfield (`w * h`, row-major) plus per-era `talus`/`rate`, and
`erode_eras(n)` runs `world::thermal_erosion::erode_step` `n` times.

Key honesty point: thermal erosion **conserves mass** — it slumps material to
lower neighbours, it doesn't shed volume. So weathering shows up as lost
**relief**, not lost mass:
- `mass()` — sum of heights (≈ constant across erosion).
- `peak()` — tallest surviving point (falls as spires slump).
- `relief()` — peak minus mean; the honest "how worn" metric, trending to 0
  (flat ruin) as a branch ages.
- `Structure::tower(..)` builds a peaked form with real relief (erodes
  visibly); `Structure::raised(..)` is a flat slab (little relief, barely
  erodes) — pick per material.

So: timebend → build a tower at year 0 → run the branch 1000 years →
`project_live_structure` returns the tower with its peak slumped and relief
smoothed, while its mass is conserved (it slumped outward into a mound). The
older the branch, the more worn the ruin.

## What's proven (tests, all green)

`cargo test -p unendlich-engine --lib --features render,window,gpu,
ui-overlay,audio timeline::`
- `timebend_builds_branch_that_erodes_and_staleifies_the_old` — the headline:
  back to year 0, raise a tower, run 1000 years; relief + peak drop, mass
  conserved, ruin persists, old timeline stale, branch live.
- `forking_a_fork_cascades_staleness` — fork a fork, then re-fork the root;
  staleness cascades to all dependents rooted on the dead segment (looping).
- `lineage_traces_fork_points` — lineage walks root→leaf fork points.
- `timebend_outside_span_is_rejected` — can't bend to a time a branch never
  saw.
- `fork_at_head_does_not_staleify_parent` — a fork at the head is a clean
  continuation.

## Not yet wired (deliberately — this is the model + test)

- **Hook to the live sim.** `Branch.edit`/`advance` are an abstract history
  right now; binding a branch's head to actual `SimState`/world chunks (so
  the live branch IS the running world, and switching `live` swaps what the
  player sees) is the integration step. The rollback engine's deterministic
  `step` is the natural substrate.
- **Region-scoped resim.** When a branch forks, only the regions coupled to
  the edit need re-simulating forward — reuse `world::rollback_scope::
  rollback_closure` to bound the blast radius rather than resimming the whole
  world per branch.
- **Structure → world.** `project_structure`'s heightfield needs stamping
  back into voxel chunks (and its erosion reflected in the terrain) at the
  branch head.
- **Timebender agency + UI.** Who can bend, the cost/limits, and surfacing
  the branch DAG in the world-history menu (the mockup's `history_point`
  timeline is the natural home — each branch a lane, forks as diamonds).
