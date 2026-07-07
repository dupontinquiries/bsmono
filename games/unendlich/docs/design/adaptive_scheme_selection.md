# Adaptive Scheme Selection: Sim-It vs. Vector, with a Sim-Prefill Landing

> See also `vector_chaining_and_timewarp.md` — deep jumps (e.g. 1M years) exceed any single vector's fidelity, so a per-vector VECTOR_MAX_SPAN cap forces CHAINING; convergence-merge collapses quiescent stretches; the chain plays as a timestep animation that doubles as the per-link benchmark harness.

> Status: design (operator notes, r258). Extends `sim_vectors_and_correction_deltas.md`
> (sim vectors), `hybrid_step_model.md` (R2-plan/R1-fill), `deeptime_convergence_classes.md`.
> Resolves the open question the prior docs hand-waved: *when do you use long-form
> at all vs. a sim vector* — and how to land precisely. The operator's framing:
> "the sim vector should be this long if these are the systems it's modeling; if
> that's too large, sim it all; if bigger, use the vector and drive to a time
> right before where you want to land and sim into it with a sim prefill to cover
> building and growth systems."

---

## 1. The core move: a sim vector has a natural LENGTH, set by what it models

A sim vector isn't "always the right tool" or "always span the whole gap." It has
a **fidelity length** — the longest span it can cover before its approximation
error exceeds tolerance — and that length is a **function of the systems it
models**:
- A span dominated by **convergent (Class B)** channels (water settling, fog
  raining out) → the vector can be *long*: those channels reach their attractor
  and stay, so one vector covers a huge span accurately.
- A span with active **path-dependent (Class C)** channels (genome under moving
  selection, populations migrating, a dragon mid-build) → the vector's accurate
  length is *short*: the trajectory diverges from any single jump quickly, so the
  vector can only faithfully cover a small span before it needs re-anchoring.

So the vector's length is **min over its modeled systems of each system's
fidelity horizon** — exactly generalizing `fire_field`'s existing
`max_useful = (1/burn_per_tick)*4` cap (one system's "past here it converged"
bound) to "past here *some* modeled system has drifted out of tolerance." The
most path-dependent active system sets the vector length, the same way the most
path-dependent channel set the waypoint density in `hybrid_step_model.md` §3.

---

## 2. The three regimes (the scheme selector)

Given a requested span `[now → target]` and the set of systems active in the
region, pick the cheapest scheme that meets tolerance:

```
estimate: cost_longform(span, systems)      // O(ticks) exact
          fidelity_len(systems)             // how long ONE vector stays accurate
          n_vectors = ceil(span / fidelity_len)

REGIME A — SMALL SPAN:  span ≤ sim_it_threshold
   → JUST SIM IT (long-form). The vector's setup/commit overhead isn't worth it
     for a short hop; exact is both simplest and (at small N) cheapest. This is
     the "if that is too large [a vector for a tiny span], sim it all" case —
     don't reach for the machinery when the gap is small.

REGIME B — MEDIUM SPAN:  one vector covers it within tolerance
   → SINGLE SIM VECTOR. fidelity_len ≥ span, so one committed vector jumps it.

REGIME C — LARGE SPAN:  span ≫ fidelity_len
   → CHAINED VECTORS + PREFILL LANDING (the operator's key construction, §3).
     Drive across the bulk with vectors (or merged waypoints), but DON'T jump all
     the way to target — stop at a "landing approach" point shortly before it,
     then SIM-PREFILL the final stretch exactly.
```

The selector is itself a cheap, deterministic function of `(span, active system
set)` — so every machine picks the same regime (determinism: the *choice* is
committed in the sim vector, like the waypoint plan).

---

## 3. The sim-prefill landing (the important part)

This is the operator's central insight and it resolves a real tension: **a cheap
vector jump is great for the bulk, but the moment you LAND needs discrete,
path-dependent detail that a statistical jump can't place precisely.** Specifically
the **building & growth systems** — is the dragon's structure actually there,
mid-construction? has the forest grown into exactly this shape? — these leave
**discrete marks** (`sim_vectors_and_correction_deltas.md` §2) that must be
*materialized*, not approximated, at the point you actually observe.

So the landing is two-phase:
1. **Drive (cheap):** vector-jump from `now` to a **landing-approach tick**
   `target - prefill_len` — covering the bulk with the coarse/convergent path +
   correction deltas. Cheap, because the bulk is mostly convergent or aggregate.
2. **Prefill (exact, short):** from the landing-approach tick, **sim forward
   exactly** into `target` — a short run of full-resolution ticks (`prefill_len`)
   that re-materializes the building/growth systems: apply the structure
   commitments, run the last few generations of growth/spread at full detail, pop
   in the still-alive entities, settle the local fields. You "sim *into* it" so
   the landing is exact even though the journey was approximate.

```
 now ───vector drive (cheap, coarse)───▶ approach ──sim-prefill (exact, short)──▶ target
 [bulk: convergent + aggregate]          [target - prefill_len]   [building/growth materialized]
```

Why this is the right shape:
- The **bulk** is where the approximation pays off (long span, mostly convergent /
  aggregate — cheap to jump).
- The **landing** is where fidelity matters (you're about to render it / a player
  is about to interact) — so you spend exact ticks only on the short final
  stretch, materializing exactly the discrete/path-dependent detail that the jump
  couldn't place.
- `prefill_len` is set by the **slowest building/growth system that must look
  right on arrival** — long enough that the structure increments and the last
  generation of growth/spread land correctly, no longer. (Same "set the resolution
  by the most path-dependent active system" rule, applied to the landing window.)

This is the deep-time analogue of how the renderer already "renders the last-ready
state until the new one lands" (`tick_authority_regions.md`): the vector gets you
*near*, the prefill makes the *arrival* exact.

---

## 4. How the selector composes with everything

- **Vector length (§1)** = min system fidelity horizon → drives `n_vectors` and
  where the chained-vector waypoints fall (`hybrid_step_model.md`).
- **Prefill length (§3)** = slowest building/growth system that must be exact on
  arrival → the short exact tail.
- **Correction deltas** (`sim_vectors_and_correction_deltas.md` §3) ride the drive
  phase: the bulk carries the convergent baseline + sparse corrections; the
  prefill is where the *responses* get exactly resolved.
- **Structure commitments** (§2 there) are *applied* during the prefill (and at
  each driven checkpoint as conditions fire), so the dragon's build is materialized
  by replaying its committed increments over the prefill window.
- **8³ genome redistribution** (§4 there) runs in the drive phase (cheap aggregate
  spread); the prefill re-pops still-alive individuals and estimates the freshest
  cohort exactly, so the landing's population is materialized, not blurred.
- **Determinism:** the regime choice, vector length, and prefill length are all
  deterministic functions of `(span, committed system set)`, committed into the
  sim vector — so every machine drives and prefills identically. The §0 robustness
  of `sim_vectors_and_correction_deltas.md` is the safety margin if a machine must
  legitimately re-stage.

---

## 5. The decision, in one place

```
fn choose_scheme(now, target, systems) -> Scheme {
    let span = target - now;
    if span <= SIM_IT_THRESHOLD            { return Scheme::SimIt; }        // A
    let flen = fidelity_len(systems);       // min over systems' horizons
    let plen = prefill_len(systems);        // slowest build/growth that must land exact
    if span <= flen                         { return Scheme::SingleVector; } // B
    Scheme::DriveThenPrefill {                                               // C
        drive_to: target - plen,            // vector/chained-vector across the bulk
        prefill_ticks: plen,                // exact sim into the landing
    }
}
```
Everything to the left of `drive_to` is cheap approximation; everything from
`drive_to` to `target` is exact materialization. The split point is set by the
systems, not a fixed constant.

---

## 6. Cheap things now + invariant

**Cheap now:**
- Give each system a **fidelity horizon** (how long it stays within tolerance of
  exact) alongside its `ConvergenceClass` tag — `fire_field` already computes one;
  make it a uniform field. `fidelity_len = min` over active systems.
- Give building/growth systems a **prefill requirement** (how many exact ticks
  before arrival they need to look right) — usually their construction/growth
  cadence.
- Make `choose_scheme` a pure function committed into the sim vector, so the
  regime is replayable.

**Invariant:**
- **The journey may be approximate; the landing must be exact.** Always sim-prefill
  the final stretch so building/growth/discrete systems are *materialized* at the
  point of observation, never left as a statistical estimate.
- **The vector's length is bounded by its most path-dependent modeled system**, not
  chosen freely — generalizing `fire_field`'s convergence cap to the whole region.
- **Pick the cheapest scheme that meets tolerance, and commit the choice.** Small
  span → just sim it (no vector overhead); medium → one vector; large → drive then
  prefill. The selector is deterministic and committed, so every machine agrees.
