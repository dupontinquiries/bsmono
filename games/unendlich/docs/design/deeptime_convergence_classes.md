# Deep-Time Depth Bounding: Convergence Classes, not a Global Max-Depth

> See also `hybrid_step_model.md` — per-channel Class tags drive WAYPOINT DENSITY in the hybrid step model: Class-C channels (genome/population) set the minimum stage count; Class-B channels ride the same fill for free and merge at intermediates.

> See also `deterministic_cross_region_sim.md` — how cross-region events (storms/hunting) stay deterministic, and how Class-B "always-converges" candidates are the cull target (snap-at-horizon + seeded PRNG + residual check).

> Status: design decision (operator notes, r253). Resolves the question "do we
> use a max-depth flag (e.g. 100 years → steady-state → branch ends) or LOD-style
> statistical edges that compute less over long spans?" Extends
> `timebending_set_algebra.md` (divergence sets) and
> `statistical_timebending_and_storage.md` (the fast-forward spectrum). The code
> already answers most of this; this doc makes the answer explicit.

---

## TL;DR of the decision
**Both of your ideas are right, but one is a special case of the other.** Don't
make a single global max-depth flag. Instead, classify each generator/system by
**how it behaves under deep time**, and let that class decide what happens past
the horizon. The "100-years-then-steady-state" flag is just the *convergent*
class; "LOD statistical edges" is the *propagating* class. A global flag would be
wrong because the engine already contains systems of BOTH kinds, and forcing one
rule on both either freezes things that should keep moving or wastes compute on
things that have already settled.

---

## 1. Why a single global flag is wrong (what the code shows)

Two facts from the codebase decide this:

- **The terrain generator is PURE.** `world_model::density(x,y,z,t)` /
  `is_cave(...)` are functions of `(position, seed, time)` via `fbm_3d(...,
  seed, ...)` — no accumulated mutable state. A region with **no edits and no
  divergent simulation is byte-identical at year 0 and year 1,000,000.** There
  is literally nothing to carry, so for pure-procedural regions, "travel a
  million years back" *is* a reset — recompute from the seed. Free.

- **But some systems genuinely PROPAGATE and accumulate.** `Structure::
  erode_eras` iterates erosion per era (the deep-past structure weathers
  forward); `fire_field` consumes fuel monotonically; populations migrate; a
  river that carved a new channel changed the *edit-log-relative* state. These
  have history that a reset would destroy.

A single max-depth-then-reset flag would corrupt the second kind; a single
"always propagate" rule would waste unbounded compute on the first kind (which
has already converged or never diverged). So the bound must be **per-system**,
keyed to behavior.

---

## 2. The three convergence classes (this is the datatype structuring)

Tag every generator/field/system with one of these. This is the concrete,
buildable form of the "Step-3 state classification" the other docs kept
deferring to — and it's small enough to add as an enum on each system.

### Class A — **Procedural / Resettable** (no history)
Pure function of `(pos, seed, time)`. Deep-time cost is **O(1)**: you don't
fast-forward, you *evaluate at the target time*. Traveling 1M years back =
recompute from seed. No checkpoint, no divergence set, no storage.
- Examples: base terrain `density`/`is_cave`, biome/temperature/humidity fields,
  anything derived purely from noise + the clock.
- Depth behavior: **unbounded for free** — there's no "depth" to bound.

### Class B — **Convergent** (history that reaches a steady state)
Accumulates, but provably settles: the state stops changing meaningfully after
some horizon. This is your "100 years → steady-state flag → stop." The engine
ALREADY does this: `fire_field` caps catch-up at `~4/burn_per_tick` because fuel
is monotonic → converges to burnt-out; grass saturates at 1.0.
- Examples: fire (burns out), grass regrowth (saturates), forest infill toward a
  climax community, erosion toward a talus-stable profile, a lake filling to its
  basin level.
- Depth behavior: **fast-forward to the convergence horizon, then SNAP to the
  steady state and stop iterating.** Past the horizon, store only the converged
  value (tiny) — not the trajectory. The "branch ends" idea = the branch's
  divergence set stops growing once every Class-B system in it has converged.
- The horizon is **per-system and derived, not a magic 100**: it's the system's
  own convergence bound (fire already computes it from `burn_per_tick`). 100
  years is a reasonable *default ceiling* / safety cap, but each system should
  expose its real convergence time so we snap as early as correctness allows.

### Class C — **Path-dependent / Chaotic** (history that never fully settles)
Genuinely keeps evolving and the fine trajectory can matter: ongoing migration
driven by a wandering climate, a river system that keeps re-routing, named-entity
histories (this specific dragon's territory). Can't be reset (Class A) and
doesn't converge (Class B).
- Depth behavior: **this is where the LOD statistical edges go.** Past the
  near-time horizon, replace exact ticks with a **statistical/residual jump**
  (the spectrum from `statistical_timebending_and_storage.md`): coarsen the
  representation and sample the outcome distribution, because the *exact* path a
  million years ago doesn't matter — only a plausible aggregate does. Compute
  drops with depth (coarser, cheaper sampling the further back you go) WITHOUT
  freezing. Storage = sparse checkpoints + the divergence residual.

---

## 3. So the answer to the two options

- **"Max-depth 100 years → steady-state → branch ends"** = the **Class B** rule.
  Correct and cheap *for convergent systems*, which is most of the ambient world
  (fire, grass, erosion, infill, lakes). Generalize it: not a fixed 100, but each
  system's own convergence horizon, with 100y as a default safety ceiling. Once
  all Class-B systems in a branch have converged and there are no Class-C systems
  still diverging, the branch's divergence set is frozen → "the branch ends" in
  the sense that nothing past that adds new state to store or merge.

- **"LOD statistical edges, less compute over long periods"** = the **Class C**
  rule. Correct *for path-dependent systems*, where you can't snap to a steady
  state but the exact deep-past detail is unobservable, so you spend less compute
  the further back you go. This is exactly the LOD-by-depth instinct, applied to
  the systems that actually need to keep moving.

- **"Traveling 1M years back just resets the world for lots of generators"** =
  the **Class A** rule, and it's the common case — most of the terrain is pure,
  so the reset is *correct and literally free*. "But maybe some generators
  propagate differently" = those are Class B (converge) or Class C (statistical).

Your three sentences map exactly onto the three classes. The design is: **let the
class decide**, don't pick one rule for the whole world.

---

## 4. How depth bounding + the set algebra interact

From `timebending_set_algebra.md`, a branch's cost is proportional to its
**divergence set**. The convergence classes bound that set over time:
- Class A contributes **nothing** to the divergence set (identical to base by
  construction — shared by pointer, never stored).
- Class B contributes a **bounded, shrinking** divergence: it grows until the
  convergence horizon, then collapses to a single converged value. The branch's
  Class-B divergence is bounded by the *latest* convergence horizon among its
  systems.
- Class C contributes the **only unbounded-in-principle** divergence, and that's
  exactly why it's the one we approximate statistically (coarser with depth) —
  to keep even its contribution sub-linear in time.

So depth bounding isn't a separate mechanism from the set algebra — it's **what
keeps the divergence set finite as time → ∞.** A global max-depth flag would bound
it by truncation (lossy, corrupts Class C); convergence classes bound it by
*correctness* (Class A free, Class B snaps when truly settled, Class C
approximated only where unobservable).

---

## 4.5. How do you DETERMINE a system's class? (perturbation bracketing + cheaper pre-filters)

The classes above are asserted, but the practical question is: *how do you know
whether a channel (e.g. animal behaviour) actually changes a region's outcome, or
washes out?* The honest definitive test is the operator's idea —
**perturbation bracketing**: spawn parallel sim vectors from the same start with
**drastically different** values of the channel (max-aggro vs. min-aggro, or
"behaviour live" vs. "behaviour frozen"), run them, and compare endpoints.
- **Endpoints converge** (digests match within tolerance) → the channel is
  **Class B for this region/depth**: it doesn't change the coarse outcome, so a
  jump can skip simulating it and use the cheap path.
- **Endpoints diverge** → the channel is **Class C here**: it matters, keep
  simulating it (or keep both branches).

This is exactly the **set-algebra difference** operation (`timebending_set_algebra.md`):
the two vectors are branches; their *divergence set* is the answer. Empty
divergence ⇒ merge them ⇒ the channel is irrelevant. It's a bisimulation /
adversarial-bracket test: if the extremes meet, everything between them does too.

**The catch, and the key efficiency.** A full parallel bracket costs 2× (or N×) a
vector — too expensive to run per jump. The fix is **amortization: you pay it once
to ESTABLISH the class for a region-TYPE, then cache the verdict** as a committed
fact ("behaviour is Class B for grassland regions out to 50 yr"). After that,
every jump for that region-type reads the cached class instead of re-bracketing.
The bracket is how you *learn* the class; the class is what you *reuse*.

**Cheaper ways (run cheapest-first; only fall through to the full bracket):**

1. **Structural reachability (free — no simulation).** A channel can only change
   a region's coarse outcome if it has an outgoing **coupling edge** that survives
   coarsening (behaviour → grazing → vegetation → water retention → …). Run
   `rollback_closure` (already exists, `world/rollback_scope.rs`) from the
   channel's node over `region_graph::Edge` with the coarse `min_coupling`
   threshold: if the channel reaches nothing observable within the horizon, it
   **provably cannot matter** — Class B by graph analysis, zero sims. This catches
   the common case (most local behaviour doesn't propagate) for free.

2. **Coarse pre-bracket (cheap probe).** If structurally it *could* matter, run the
   bracket at a **much coarser resolution / shorter span** first. If even the cheap
   extremes converge, you're done; only escalate to a full-resolution bracket when
   the coarse one is ambiguous. Multi-resolution, like the tiered mesher.

3. **Sensitivity bound (one analysis, not a second sim).** If you know the
   per-tick coupling gains, bound the **maximum** the channel can perturb the
   outputs and propagate that over the span (a Lipschitz / interval-arithmetic
   bound). If the worst-case perturbation is below tolerance, you've **proven**
   convergence without running the second sim at all. Cheaper than a bracket,
   needs the gains.

4. **Variance probe (statistical).** Perturb the channel's seed slightly and
   measure output variance over a short run; low variance ⇒ insensitive. Cheaper
   than the extremes, but probabilistic — use as a pre-filter, not a proof.

5. **Full parallel bracket** — only when 1–4 are ambiguous. Definitive, and its
   result upgrades the cache.

**Determinism.** The bracket must be deterministic (fixed extremes, fixed seeds)
so its verdict is reproducible and **committable** — the class becomes a committed
fact in the seed lineage, like a sim vector's plan, so every machine agrees which
channels it may skip. (This also makes the bracket a natural **benchmark assert**,
`vector_chaining_and_timewarp.md` §4: the parallel vectors with a long-form overlay
*are* the convergence test.)

**Behaviour specifically (ties to `sim_vectors` §4.5).** Habit/aggression usually
fails test 1 in placid regions (no surviving coupling → free Class B) and only
needs the real bracket where behaviour plausibly drives population → ecology (a
predator cohort that over-hunts a region). So the expensive test fires rarely, and
only where it might actually change history — which is the whole "cost ∝ what
diverges" principle applied to *deciding what to simulate at all*.



**Gated, same as the prior docs:** this needs the dynamic systems running so we
can see which are genuinely Class C vs. Class B in practice (e.g. does forest
infill actually converge, or does climate keep it churning → Class C?). The
class of a system is an *empirical* property to confirm by measuring real
divergence behavior, not assert up front.

**Cheap things to carry now (zero extra cost, keeps the door open):**
1. When adding any dynamic system, give it a **`fast_forward(n)` that's checkable
   against its long-form** (grass/fire already do) — this is also how you *detect*
   its class: if `fast_forward` saturates/caps, it's Class B; if it's a pure
   eval, Class A; if neither, Class C.
2. Tag each system with a provisional **`ConvergenceClass`** enum
   (`Procedural | Convergent { horizon } | PathDependent`). Start with the honest
   guess; let measurement correct it. This single enum is the seed of the Step-3
   datatype structuring and costs nothing to add now.
3. Keep cross-region effects as `region_graph::Edge` couplings (from
   `timebending_set_algebra.md`) so blast radius — and thus the spatial bound on
   each class's divergence — is measured by the existing `rollback_closure`.

### Invariant
Depth is bounded by **convergence, not truncation.** A branch never simply
"forgets" deep history by hitting a frame number; it forgets Class-A history
because it's regenerable, Class-B history because it provably converged, and
Class-C *detail* because it's coarsened to an unobservable-fidelity statistical
summary. If a system would lose *observable* state at depth, it's mis-classified
— fix the class, don't truncate.
