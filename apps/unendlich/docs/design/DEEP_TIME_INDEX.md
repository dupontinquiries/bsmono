# Deep-Time & Dynamic-World Design — Index & Through-Line

> Storage/world-load integration: `timebending_storage_integration.md` — how the machinery plugs into storage.rs + the world menu (timebenders load any time = deterministic recompute of tick T; tick-stamped edit log + sparse checkpoints, reusing the reconstructive load path).

> Implementation scoping for Step 1: `STEP1_SCOPING_regiongrid.md` (RegionGrid already implements the share-coarse-field pattern → Step 1 is an extension of tested code, gated on a GPU run + ideally Step 0).

> Status: index / overview (r260). Map of the design arc developed across r251–r260
> (operator design conversation). Ten docs, one coherent architecture. This is the
> entry point: read this first, then the doc for whatever layer you're touching.
> Everything here is **design** — gated on the dynamic systems actually running;
> nothing in this arc is wired yet. The per-doc "cheap things now" are consolidated
> in §4 so the zero-cost door-openers are in one place.

---

## 1. The one-sentence through-line

**Cost should be proportional to what actually *diverges*, bounded by how far
effects actually *propagate*, and deep time is bounded by what actually
*converges* — with the journey allowed to be approximate as long as the *landing*
is exact and every *choice* is committed so it replays deterministically.**

Every doc below is one facet of that sentence.

---

## 2. The arc, in dependency order

1. **`dynamic_world_architecture.md`** — the spine. Separate **Q1 "compute now +
   mark dirty"** from **Q2 "needed for someone's vision."** Three vision consumers
   (player / mob / sim). The **tiered mesher** (A full mesh → B downsampled → C
   mob-proxy/fast-noise → D coarse regional field = the shared substrate). Maps the
   dynamic behaviours (lakes→caves, erosion, regrowth, lightning→fire→migration).
   *Everything else builds on the compute-vs-vision split + tier-D field.*

2. **`statistical_timebending_and_storage.md`** — why deep time + the **storage
   tension**: born-procedural delta storage breaks once sim diverges; timebending
   pays for itself as *infrastructure* (regenerate-on-load via fast-forward, not
   store-everything). The dual-path benchmark (fast vs long-form).

3. **`timebending_set_algebra.md`** — the data model: timebending = **rebase + set
   algebra** (intersection/union/difference) over a branch DAG, bounded by the
   `region_graph` coupling closure. Content-addressed, structurally-shared regions
   → branch cost ∝ divergence. (git-for-world-state.)

4. **`deeptime_convergence_classes.md`** — depth bounding by **class, not a global
   flag**: A Procedural/resettable (O(1) eval), B Convergent (snap at horizon —
   fire/grass already do this), C Path-dependent (statistical, coarsen with depth).
   The three operator options ("reset / 100yr steady-state / LOD edges") are these
   three classes.

5. **`deterministic_cross_region_sim.md`** — determinism is **layered**: bit-exact
   (Fixed-point — already in `sim/fixed.rs`, but the field sims use f32 today) →
   **ordering** (double-buffer + tick-stamped total-ordered edge messages — the
   hard new piece for storms/hunting) → composition (N det steps = det span) →
   approximation (seeded PRNG + residual) → consensus.

6. **`simulation_guided_seeding.md`** — make the statistical jump **state-aware**:
   key the PRNG on `(seed, region, tick, digest(state))` so a drought vs. flood
   draw different outcomes. Reading A (state→seed) safe; Reading B (outcome→seed)
   biased/desyncs. The digest = the set-algebra content identity (build once).

7. **`hybrid_step_model.md`** (+ `hybrid_step_model_mockup.html`) — **R2 plans
   waypoints, R1 fills** (deterministic, coarse sub-steps). Genome/population is
   Class C and **can't be one R1 jump** (moving fitness optimum + N-dependent
   drift = path-dependent) → Class-C channels set **waypoint density**; Class-B
   ride the fill for free. Mockup shows hybrid ~10× better residual than pure R1 at
   88–97% fewer exact steps.

8. **`sim_vectors_and_correction_deltas.md`** — names the checkpoint directors
   **sim vectors** {span, seed-key, waypoint plan, correction deltas, event
   commitments}. **Structure/event persistence** (dragon-still-building = committed
   conditional edits, never blur). **Correction deltas** (blur only dimensions that
   *responded*). **8³ genome-summary redistribution** (one coarse step: shift +
   diffuse + propagate). **Lifespan pop-in** (alive = exact, turned-over =
   estimated). *Crucial caveat:* endpoint convergence under coarsening is a **safety
   margin (robustness), NOT determinism** — still commit the plan.

9. **`adaptive_scheme_selection.md`** — *when* to use what: a vector has a fidelity
   **length** = min over modeled systems' horizons. **Sim-it** (small span) /
   **single vector** (medium) / **drive-then-prefill** (large). The **sim-prefill
   landing**: drive cheaply to `target − prefill_len`, then sim exactly into the
   target so building/growth is *materialized*, not estimated. Journey approximate,
   landing exact.

10. **`vector_chaining_and_timewarp.md`** — deep jumps (1M yr) exceed any single
    vector → **`VECTOR_MAX_SPAN` cap forces chaining**; convergence-merge collapses
    quiescent stretches (cost ∝ distinct vectors). The **timestep animation** that
    presents a deep scrub IS the chain playing (merged=fast, active=slow), and its
    per-link checkpoints **ARE the benchmark assert points**. One structure: scrub
    UX + approximation + test suite.

---

## 3. The build order (strict; each unsound without the prior)

From `deterministic_cross_region_sim.md` §6, extended by the later docs:

```
0  dynamic systems RUN (near, real-time): water/wind/storms+discharge/populations
     visible — the prerequisite for everything (dynamic_world_architecture Step 0)
1  shared coarse regional terrain field (tier D)   ← also the far-mesh perf fix
2  far regions advance via field fast-forward, paced
3  state classification + datatype structuring: ConvergenceClass + fidelity_horizon
     per system; coarse Fixed digest per region (= set-algebra content identity)
4  Fixed-point port of shared field sims + deterministic noise; double-buffered
     cross-region step + tick-stamped total-ordered edges  ← the hard determinism piece
4½ seeded statistical jump → state-conditioned (digest key) → hybrid R2-plan/R1-fill
5  sim vectors: correction deltas, structure commitments, 8³ genome summary,
     lifespan pop-in; adaptive scheme selection + sim-prefill landing
6  vector chaining (VECTOR_MAX_SPAN) + convergence merge; timestep animation =
     benchmark harness; voting only on divergence-set policy (never physics)
```

Step 1 is the concrete first move and it doubles as the far-mesh chug fix
(`PERF_FINDING_far_region_corrected.md`).

---

## 4. Consolidated "cheap things to do NOW" (zero-cost door-openers)

These cost ~nothing today and keep every later option open. Do them as the
relevant code is touched, even before deep-time:

- **Route every cross-region effect through `region_graph::Edge` couplings** (not
  ad-hoc reaches) → `rollback_closure` measures blast radius for free; the system
  becomes a citizen of the set-algebra + double-buffer later. *(set-algebra,
  determinism)*
- **Tag each dynamic system with `ConvergenceClass`** (`Procedural | Convergent{
  horizon} | PathDependent`) **and a `fidelity_horizon`** (fire_field already
  computes one — make it uniform). These drive waypoint density, vector length, and
  the scheme selector. *(convergence-classes, hybrid, adaptive-selection)*
- **Do new sim math in `Fixed`** (or at least portable — no irrational constants
  baked into the algorithm shape). Retrofitting is the expensive path. *(determinism)*
- **Every stochastic choice keyed by `(seed, region, tick[, digest])`**, never a
  wall clock — even in throwaway near-time versions. *(seeding, determinism)*
- **Define the per-region coarse `Fixed` digest** (fixed field order, stable
  versioned hash, not DefaultHasher) — one digest serves branch-sharing, dirty
  detection, AND seed conditioning. *(set-algebra, seeding)*
- **Define the population's persisted form as a genome summary** (per-gene
  mean/variance + 8³ bin counts) from the start — the redistribution step and the
  storage model both want it. *(sim-vectors)*
- **Give each dynamic system a `fast_forward(n)` / `dt`-multiplier step that's
  checkable against its long-form** (grass/fire already do) — this is the
  dual-path benchmark unit AND it auto-detects the system's class (saturates=B,
  pure-eval=A, neither=C). *(hybrid, convergence-classes, benchmark)*
- **Building/growth systems carry a `prefill_requirement`** (exact ticks before
  arrival to look right) and express their deep-time effect as **conditional
  committed edits** on a sim vector. *(sim-vectors, adaptive-selection)*

---

## 5. The load-bearing invariants (don't regress these)

- **Never gate a simulation computation on player visibility.** Vision picks the
  rendered *tier*, never *whether the sim runs*. *(dynamic-world)*
- **One source of truth per region; many tiered views.** Disagreement = a tier is
  re-sampling instead of reading the shared field. *(dynamic-world)*
- **Far ≠ frozen; depth bounded by convergence, not truncation.** *(convergence-classes)*
- **Determinism is layered** (bit-exact → ordering → composition → approximation →
  consensus); a break at any layer desyncs. *(determinism)*
- **State conditions the seed (`state→seed`); the desired outcome never does
  (`outcome→seed`).** *(seeding)*
- **The authoritative timeline is the deterministic R1 fill over a committed plan;
  R2 only decides where exact work goes.** *(hybrid)*
- **Endpoint stability under coarsening is a safety margin, not determinism** —
  commit the plan. **Discrete marks (structures/deaths/builds) are committed edits,
  never statistical blur. Blur only what responded.** *(sim-vectors)*
- **The journey may be approximate; the LANDING must be exact** (always
  sim-prefill). *(adaptive-selection)*
- **No vector spans > `min(fidelity_len, VECTOR_MAX_SPAN)`; cost ∝ distinct
  un-merged vectors; the cap, the animation granularity, and the benchmark assert
  cadence are the SAME checkpoints.** *(vector-chaining)*

---

## 6. Status

All ten docs are **design, CHECKED as docs** (the one mockup,
`hybrid_step_model_mockup.html`, is syntax-validated + smoke-tested under node).
**None is wired.** The arc is gated on the dynamic systems running (Step 0); the
concrete first implementation move is **Step 1 — the shared coarse regional
terrain field**, which is simultaneously the far-mesh performance fix. Until then,
the §4 cheap door-openers are the only things worth doing, and they cost nearly
nothing.
