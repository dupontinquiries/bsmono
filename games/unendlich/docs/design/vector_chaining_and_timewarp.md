# Vector Chaining: A Max-Span Cap, the Timestep Animation, and the Benchmark

> Status: design (operator notes, r259). Extends `adaptive_scheme_selection.md`
> (regime C: drive-then-prefill), `sim_vectors_and_correction_deltas.md` (sim
> vectors), `timebending_set_algebra.md` (convergence merging). Resolves: deep
> jumps (e.g. timebend 1,000,000 years) must use **more than one vector** — a
> per-vector max-span cap forces chaining; the chaining is hidden in-game by a
> **timestep animation**, and that same animation IS the benchmark harness.

---

## 1. Why one vector can't span a million years (the cap)

`adaptive_scheme_selection.md` §1: a sim vector has a fidelity length =
min over its modeled systems' horizons. A million-year jump vastly exceeds any
real system's horizon, so a single vector covering it would be pure fabrication —
its error unbounded. The honest move is the operator's: **cap a vector's span and
chain vectors** to cover the rest.

- **Per-vector max-span cap** `VECTOR_MAX_SPAN` (in addition to the
  system-derived `fidelity_len`). The effective per-vector length is
  `min(fidelity_len(systems), VECTOR_MAX_SPAN)`. The cap is a hard ceiling so even
  an all-convergent region (long `fidelity_len`) still re-anchors periodically —
  bounding drift and giving the chain regular checkpoints.
- **A deep jump = a CHAIN of vectors:** `n = ceil(span / per_vector_len)` vectors,
  each carrying its segment, each re-anchored to a committed checkpoint at its
  end. The chain terminates with the **sim-prefill landing** (regime C) into the
  target tick.

```
 -1,000,000 yr ──V1──▶•──V2──▶•── … ──Vn-1──▶• ──prefill──▶ target
                 cap   cap         cap        exact landing
                 (each vector ≤ min(fidelity_len, VECTOR_MAX_SPAN))
```

This is the per-region generalization of `fire_field`'s existing per-system cap
(`max_useful = (1/burn_per_tick)*4` — "re-evaluate before this drifts"): the
vector cap says "re-anchor before *any* modeled system drifts, and at most every
VECTOR_MAX_SPAN regardless."

---

## 2. Convergence merging makes the chain cheap (not n × full cost)

A million-year chain sounds expensive, but `timebending_set_algebra.md` /
`sim_vectors_and_correction_deltas.md` already provide the collapse: **chained
vectors whose end-checkpoints land in the same coarse bucket (digest) MERGE.**

- Over deep time most regions reach a **convergent / steady regime** (Class B
  channels at their attractor, populations at carrying capacity, genomes near the
  moving optimum). Once there, consecutive vectors produce the *same coarse
  digest* → they merge into a single repeated edge ("nothing distinct happened for
  900,000 years"), so the chain's *distinct* node count is tiny even though its
  span is enormous.
- The chain only stays *un-merged* (full per-vector cost) across spans where
  something path-dependent is actually changing — which is correct: that's where
  the resolution is needed. Deep time in a quiescent region collapses to a handful
  of distinct vectors + a long "steady" run; deep time across an active era keeps
  its vectors.

So cost ∝ **distinct (un-merged) vectors**, not raw `n` — the same
"cost ∝ divergence" principle from the set-algebra doc, applied to the time chain.
A million years where little changes is nearly free; a million years of active
upheaval costs what the upheaval costs.

---

## 3. The timestep animation: the cap isn't hidden shame, it's the feature

The operator's framing: the chaining "can be hidden in game by a timestep
animation." Reframe — it's not hiding a limitation, it's the **natural
presentation of deep scrubbing**, and the chain boundaries are exactly the
animation's frames:

- A deep timebend/scrub **plays the chain**: each vector's end-checkpoint is a
  **keyframe**; materializing checkpoint→checkpoint is a **timewarp frame**. You
  literally watch the world fly by — water rising, forests advancing, storms
  flashing, structures growing — one vector-segment per animation beat.
- **Merged (steady) stretches fast-forward visually** (the animation speeds
  through "nothing distinct for 900k years"); **active eras play slower** (more
  distinct vectors = more keyframes = more to show). The animation's pacing is
  *driven by the divergence* — it naturally lingers where history was eventful and
  blurs past where it was quiescent. That's a better scrub UX than a uniform
  fast-forward, and it falls out of the chain for free.
- The prefill landing is the **final settle frame**: the animation decelerates into
  the exact materialized target (the dragon's tower snapping to its real
  half-built state on arrival).

So `VECTOR_MAX_SPAN` sets the animation's *granularity* (max time per keyframe);
the merge structure sets its *pacing*. The cap that makes the sim honest is the
same parameter that makes the scrub legible.

---

## 4. The chain IS the benchmark harness

The dual-path benchmark (`statistical_timebending_and_storage.md`) wants to run a
span both fast and long-form and compare. The vector chain gives this structure
for free, at every checkpoint:

- **Per-vector residual:** each vector's end-checkpoint can be compared against a
  long-form run of that segment → a residual *per link*, not just end-to-end. So
  the benchmark localizes *which* segment (which era, which systems) the
  approximation drifts on, instead of one opaque end-of-chain number.
- **The animation doubles as the test visualization:** playing the chain with the
  long-form overlaid (the mockup's three-line idea, `hybrid_step_model_mockup.html`)
  shows divergence accumulating per keyframe — a visible, debuggable benchmark.
- **Checkpoints are natural assert points:** "at checkpoint k, fast-path digest
  vs. long-form digest within tolerance?" If a link fails, its vector's modeled
  systems were mis-horizoned (the cap or `fidelity_len` was too long for that era)
  → shorten that link. Self-tuning falls out: links that fail tolerance get a
  smaller cap.
- **Determinism check rides along:** replay the same committed chain twice → must
  be bit-identical (the §0 determinism contract from
  `sim_vectors_and_correction_deltas.md`); the per-link checkpoints make a
  desync's *location* obvious.

So "satisfy our benchmark" = the chain's per-link checkpoints ARE the benchmark's
assert points, and the timestep animation IS its visualization. One structure
serves the scrub feature, the perf approximation, and the test suite.

---

## 5. How it composes (the full deep-jump path)

```
choose_scheme(now, target=now-1e6yr, systems)
  span ≫ fidelity_len  → regime C, but span ≫ VECTOR_MAX_SPAN too →
  CHAIN: n = ceil(span / min(fidelity_len, VECTOR_MAX_SPAN)) vectors
    each Vi: drive its segment (cheap; convergent baseline + correction deltas);
             commit end-checkpoint + digest;
             merge if digest == a seen bucket (steady stretches collapse);
             [benchmark hook] optional long-form compare → per-link residual
    final:   sim-prefill into target (exact; materialize structures/growth/entities)
  present:   play the chain as a timestep animation
             (merged stretches fast, active eras slow, decelerate into prefill)
```

Everything reuses prior pieces: regime C (drive-then-prefill), the digest +
convergence merge (set algebra), correction deltas + structure commitments + 8³
genome redistribution per segment, and the committed-plan determinism. The only
new element is the **`VECTOR_MAX_SPAN` cap** that forces chaining and sets the
animation granularity.

---

## 6. Cheap things now + invariant

**Cheap now:**
- Add `VECTOR_MAX_SPAN` as the per-vector hard ceiling alongside the per-system
  `fidelity_len`; effective length = `min(...)`. A chain is just
  `ceil(span / eff_len)` committed vectors with merge.
- Make the per-link checkpoint a **benchmark assert point** from the start (digest
  compare slot), so the dual-path test is per-segment, not just end-to-end — and a
  failing link auto-shortens its cap.
- Treat the chain as the **animation timeline**: each committed checkpoint is a
  keyframe; merged runs collapse to fast-forward; prefill is the settle.

**Invariant:**
- **No single vector spans more than `min(fidelity_len, VECTOR_MAX_SPAN)`.** Deep
  jumps chain; a vector that claims to honestly cover a million years is a bug.
- **Cost is proportional to DISTINCT (un-merged) vectors, not raw span.** Quiescent
  deep time collapses via convergence merge; only active eras cost.
- **The cap, the animation granularity, and the benchmark assert cadence are the
  SAME checkpoints.** One chain structure serves scrub presentation, approximation,
  and the test suite — don't build three separate mechanisms.
