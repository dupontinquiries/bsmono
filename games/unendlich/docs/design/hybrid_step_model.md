# Hybrid Step Model: R2 Plans Waypoints, R1 Fills — and Why Genomes Force It

> See also `adaptive_scheme_selection.md` — when to use the hybrid at all: short spans just sim long-form (no vector overhead); the vector's length is min over modeled systems' fidelity horizons; large spans drive cheaply then sim-prefill the final stretch for an exact landing.

> See also `sim_vectors_and_correction_deltas.md` — names the checkpoint directors **sim vectors**, adds structure/event persistence (committed conditional edits, e.g. "dragon still building"), correction-delta sparsity (blur only dimensions that responded), 8³-binned genome-summary redistribution, lifespan pop-in, AND the careful answer to "graph changes but endpoints converge" (robustness ≠ determinism).

> Status: design (operator notes, r257). Companion to the interactive sandbox
> `hybrid_step_model_mockup.html`. Builds on `simulation_guided_seeding.md`
> (R1/R2 readings), `deeptime_convergence_classes.md` (Class B/C),
> `deterministic_cross_region_sim.md` (determinism layers). Resolves: "can we
> have a hybrid step model that uses R2 to guide smaller staged R1 steps, building
> a graph of states that can converge at intermediates — and how do genome/
> population dynamics fit, since pure R1 loses too much resolution there?"

---

## 1. The hybrid in one line

**R2 is a planner, not an oracle. R1 is the authoritative fill.**
- **R2 (the planner)** proposes *waypoints* — coarse target/anchor states the
  region passes through over the span. This proposal can be cheap and even a
  search, because it is not trusted as truth.
- **R1 (the fill)** integrates deterministically *between* waypoints — seeded by
  `hash(seed, region, tick, digest(state))` — at a **coarse sub-step cadence**
  (a few exact evaluations per stage, each advancing several ticks), so it does
  far fewer exact steps than long-form while still producing the real,
  reproducible trajectory.
- The waypoints form a **graph of intermediate states**; intermediates that land
  in the same coarse bucket (same `digest`) are **merged** — the "convergent
  intermediates" collapse, so you carry fewer distinct states.

The sandbox shows the payoff numerically: across 7–40 day spans, the hybrid's
residual vs. long-form stays ~0.02 (≈10× better than pure R1's 0.14–0.24) while
doing **88–97% fewer exact steps** than long-form. The win grows with span.

---

## 2. Why this is determinism-safe even though R2 may search

This is the crux that makes using R2 at all legitimate (vs. the "trap" reading):

- The planner's **output (the waypoint plan) is committed to the seed lineage** —
  treated like an edit-log entry in the reconstructable state
  (`statistical_timebending_and_storage.md`). A replay does **not re-run the
  search**; it replays the *committed plan* and re-fills with R1.
- So the search's nondeterminism is "frozen" at commit time, exactly like a
  player's edit is frozen: the world is reconstructable from
  `(seed, gen_version, edit-log, committed-plans, tick)`. The fill is the
  deterministic part; the plan is data.
- This is the difference from the §3 "Reading B trap" in
  `simulation_guided_seeding.md`: there, a *live* per-tick seed search corrupts
  the sample and risks desync. Here, the search runs **once per span, off the hot
  path, and its result is committed** — so the authoritative timeline is the
  deterministic R1 fill over a fixed plan. R2 guides *where* to put the exact
  work; R1 *is* the work.

Litmus: a plan you can write down, commit, and replay is safe. A seed you pick
mid-fill to steer the outcome is not.

---

## 3. Why genome/population dynamics CAN'T be pure R1 (the resolution argument)

This is the operator's key observation, and it's correct. Pure R1 takes **one
conditioned draw for the whole span** — fine for a channel whose endpoint is a
function of its start + a sample. But genome/population evolution is
**path-dependent in a way the endpoint can't recover**:

- **The fitness optimum moves over the span.** Selection pushes the allele
  distribution toward an optimum *set by the current climate* (water/fog), and
  the climate itself changes tick to tick. So the selection pressure at day 3 is
  different from day 9 — the trajectory integrates a *moving target*. A single
  end-state draw conditioned on the start state never sees the intermediate
  pressure, so it lands in the wrong place (and its residual grows with span —
  visible in the sandbox).
- **Drift depends on population size, which depends on the trajectory.** Genetic
  drift scales ~1/√N; N rises and falls with carrying capacity (grass/water) and
  storm culls. A population that dipped after a day-5 storm drifts harder for a
  while — an event a start-conditioned endpoint can't know happened.
- **Selection × reproduction × events compound.** Eating sets N, N sets drift,
  drift + moving-optimum selection sets the allele mean, the allele mean changes
  who survives the next storm. This is a coupled feedback over generations — the
  textbook definition of "the path matters." Collapsing it to one jump
  **discards the generations**, which is the resolution loss the operator named.

So population/genome is squarely **Class C (path-dependent)** from
`deeptime_convergence_classes.md`, and Class C is exactly what the hybrid exists
for: it keeps **one R1 sub-step per generation-ish waypoint**, so the moving
optimum and the N-dependent drift are sampled *along the path*, not just at the
end. You spend a few exact steps to preserve the trajectory's shape, while the
*convergent* channels (water filling a basin, fog raining out) still collapse
cheaply because their endpoint genuinely is their attractor.

### The mixed-channel rule
A region's state has **both** kinds of channel at once:
- **Convergent (Class B):** water level, fog/storm, fuel — collapse to the
  attractor; R1 can jump these, waypoints mostly just confirm convergence.
- **Path-dependent (Class C):** population, allele frequencies, named-entity
  history, anything with selection/drift/migration feedback — these set the
  **waypoint density**: you need enough stages that one R1 sub-step lands per
  meaningful generation/event, or you lose their shape.

So the **number of waypoints is driven by the Class-C channels**, and the Class-B
channels ride along for free inside the same fill. The hybrid is "coarse where
it can be (B), fine where it must be (C)" — per channel, within one region.

---

## 4. The state graph + convergence merging

The waypoints + fills form a DAG (ties into `timebending_set_algebra.md`):
- **Node** = a staged intermediate state (its coarse `digest`).
- **Edge** = an R1 fill segment between waypoints.
- **Merge** = two intermediates with the same coarse digest are the *same node* —
  distinct fill paths that **converged** to the same coarse state collapse, so the
  graph (and the storage/branch divergence set) stays small.
- This is where "a graph of states that can converge at intermediates" lives:
  convergent (Class-B-dominated) regions merge often (few distinct nodes);
  path-dependent (Class-C) regions merge rarely (the trajectory keeps them
  distinct) — which is *correct*, because those are the states you must keep.

Merging is the deep-time bound from `deeptime_convergence_classes.md` applied at
the *intermediate* level instead of only at the final horizon: you don't just snap
the endpoint, you collapse any waypoint that rejoins a state already seen.

---

## 5. How the sandbox maps to the real system (and its caveat)

`hybrid_step_model_mockup.html` runs a toy region (water/fog/fuel/population/
allele) three ways with integer-seeded PRNG so it's bit-reproducible. It is a
**thinking instrument for the step model's shape**, not a measurement — the toy
dynamics are invented to make the tradeoff legible. The real residual/accuracy
numbers come only from the dual-path benchmark against the engine's actual sim.

What it faithfully demonstrates:
- pure R1's residual **grows with span** on the path-dependent channels (loses
  generations);
- the hybrid holds a **small residual at a fraction of long-form's exact-step
  count**, and the gap widens with span;
- **more waypoints → more convergent-intermediate merges + tighter residual**;
- everything is **deterministic across re-runs** (the determinism-layer claim).

What it does NOT prove: the *real* per-channel convergence behavior, the right
waypoint density for the actual genome model, or that any specific channel is
truly Class B vs C — those are empirical, measured once the systems run.

---

## 6. Build implications + cheap things now

- The hybrid is **step 4½** in `deterministic_cross_region_sim.md`'s order: it
  sits on top of the seeded-PRNG statistical jump (step 4), upgrading "one jump"
  to "R2-planned, R1-filled waypoints." Prereqs: `Fixed` port + coarse digest +
  the per-channel Class tag.
- **Per-channel class tags** (`ConvergenceClass` on each field, from
  `deeptime_convergence_classes.md`) directly drive waypoint density: count the
  Class-C channels in a region → that sets the minimum stage count for an
  acceptable residual. Build the tag; the planner reads it.
- **Commit the plan to the reconstructable state** (edit-log-adjacent) from the
  first version, so replays re-fill rather than re-search — this is the
  determinism contract; retrofitting it is painful.
- Keep every channel's `step` able to take a **`dt` multiplier** (coarse
  integration), as the sandbox does — that's what makes the fill cheaper than
  long-form. Grass/fire already have `fast_forward(n)`; this generalizes it to
  "advance dt ticks in one exact evaluation."

### Invariant
The authoritative timeline is always the **deterministic R1 fill over a committed
plan**; R2 only decides *where* the exact sub-steps go. Waypoint density is set by
the **path-dependent (Class-C) channels** — never coarsen below one exact sub-step
per meaningful generation/event, or the genome/population trajectory is lost.
Convergent channels ride the same fill for free and merge at intermediates.
