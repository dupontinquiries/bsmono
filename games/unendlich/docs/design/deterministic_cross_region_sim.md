# Deterministic Cross-Region Long-Time Simulation (storms, hunting, water)

> See also `sim_vectors_and_correction_deltas.md` §0 — convergent-endpoint stability under coarsening is a SAFETY MARGIN (bounds the error of unavoidable differences, lets correction deltas heal them), NOT the determinism mechanism; determinism still requires committing the waypoint plan so every machine replays the same one.

> See also `simulation_guided_seeding.md` — refines the statistical jump's PRNG key from `(seed,region,tick)` to `(seed,region,tick,state_digest)` so the sampled outcome is conditioned on the region's actual state (drought vs flood draw different outcomes), staying deterministic via a Fixed-only coarse stable digest.

> Status: design / hard-problem analysis (operator notes, r254). The question:
> "how do I simulate ~10 days of water physics + weather (mid-scale), with
> cross-region events like storms and hunting, and still get determinism — then
> maybe approximate statistically to cull always-converging candidates, with a
> voting system among the consumers (players)?" Builds on
> `deeptime_convergence_classes.md`, `timebending_set_algebra.md`,
> `statistical_timebending_and_storage.md`. This separates three problems that
> "determinism" conflates, because they have different answers.

---

## 0. Why you're stuck: it's three problems wearing one coat

"Simulate 10 days deterministically" is actually:
1. **Bit-exactness** — same inputs + state → bit-identical result on every
   machine. (Mostly solved in the codebase; not yet for the field sims.)
2. **Cross-region ordering** — when regions affect each other (a storm crossing a
   boundary, a dragon hunting across regions), the *order* effects apply must be
   defined, or two machines reach different states from the same data.
3. **Approximation fidelity** — if you replace exact ticks with a statistical
   jump, "deterministic" now means *the approximation itself is deterministic AND
   stays within bounded error of the exact path.* A different question entirely.

The voting system is a fourth thing — a *reconciliation policy* for when
authorities disagree — which only makes sense once 1–3 are pinned. Trying to
answer all four at once is why it feels intractable. Take them in order.

---

## 1. Bit-exactness — the codebase already shows the answer (extend it)

`sim/fixed.rs` already solved this for the player sim: a 48.16 fixed-point
`Fixed(i64)`, integer-only, **identical on any platform** — because float
`sqrt`/transcendentals/FMA contraction differ by a low bit between CPUs and that
snowballs into desync within a few hundred ticks. "The simulation never touches
`f32`/`f64`; floats appear only in presentation."

**But `fire_field`, `grass_field`, `storm_intensity` use `f32`/`f64` today.**
That's fine while they're presentation-side ambiance, but the moment their state
must agree across machines or be reconstructed identically for a scrub, they're a
desync footgun. So:

- **Decision: any field that participates in shared/reconstructable state must be
  ported to `Fixed`.** Reaction-diffusion, saturation, intensity — all expressible
  in fixed-point. This is mechanical, not novel (the player sim already does
  harder math in `Fixed`), and it's the price of admission for determinism.
- Transcendentals (the noise `fbm`, any `exp`/`pow` in storm/diffusion) need
  **deterministic fixed-point implementations or precomputed tables** — the
  classic approach: a shared integer LUT + fixed-point interpolation, so every
  machine evaluates the same noise to the same bits. The generator's `fbm_3d` is
  the first thing to make Fixed-deterministic if far regions must agree.

This is the foundation; nothing above works without it.

---

## 2. Cross-region ordering — the REAL new problem (storms & hunting)

Your last reasoning missed this and it's the heart of the difficulty. A
per-region field that only reads itself is trivially deterministic. The moment a
**storm crosses from region A into B**, or a **dragon in region A hunts prey in
B**, the result depends on *what order A and B are stepped and how their
interaction is applied.* If A steps first and pushes the storm into B, vs. B
steps first, you get different states — and two machines that disagree on order
desync even with perfect fixed-point math.

The codebase already has the right primitive: `world/rollback_scope.rs` +
`region_graph::Edge` model regions as a **coupling graph** with a bounded
closure. Cross-region determinism is a **graph-scheduling** problem on it:

- **Double-buffer the coupling (the standard cellular-automaton answer).** Every
  region computes tick N+1 reading only tick-N state of itself AND its neighbors,
  writes into a separate buffer, then all swap. Order-independent by construction:
  a storm's flux from A→B is computed from A's tick-N state regardless of whether
  A or B is "processed first," because nobody reads a half-updated neighbor. This
  makes the *whole* cross-region step deterministic and **parallelizable** (the
  async-tick workers in `tick_authority_regions.md` can step regions in any order
  on any thread — the swap is the barrier).
- **Cross-region events (hunt, storm flux) are EDGES carrying tick-stamped
  messages, applied at the swap.** A hunt in A targeting B emits a message
  stamped tick N; B consumes all its inbound messages **in a deterministic order**
  (sort by source region id, then entity id — a total order that's identical on
  every machine) at the swap. No message reads a partially-stepped region; the
  sort makes "two events hit B the same tick" resolve identically everywhere.
- **The blast-radius bound (`max_hops`) still applies:** a storm's cross-region
  influence is bounded by the coupling closure, so the double-buffered step only
  needs the closure, not the world.

This is the piece to design first among the new work, because it's the one that's
*architecturally* hard rather than mechanically tedious. Get the double-buffer +
tick-stamped-edge-messages + total-order-resolution right and cross-region
determinism falls out — for storms, hunting, water flux, migration, all of it,
because they're all "region reads neighbors' last state + applies ordered inbound
events."

---

## 3. The "10 days, mid-scale" question — cadence, not magic

"Not too small scale, not too large" = pick the **field cell size and tick rate**
so 10 days is affordable at the fidelity that matters, then make the fast path
*exactly* a multiple of the slow step (the grass/fire precedent):

- Water + weather at **region-field resolution** (coarse cells, not per-voxel) on
  a **slow tick** (minutes of world-time per step, not 1/60s) is the "mid-scale"
  band. 10 days at, say, one step per world-minute = ~14,400 steps on a coarse
  field — tractable, and far cheaper than per-voxel per-frame.
- Determinism over 10 days then = problems 1+2 holding for 14,400 coarse steps.
  Fixed-point (no drift) + double-buffered ordering (no order desync) = the same
  14,400-step result on every machine, every replay. **There is no additional
  magic for "10 days" beyond 1+2 holding step-to-step** — determinism composes:
  if one step is deterministic, N steps are.

The trap is trying to simulate 10 days at *frame* resolution or *voxel* scale.
The answer to "not too small, not too large" is the **tiered representation**
from `dynamic_world_architecture.md`: deep-time water/weather runs on tier-D
coarse fields at a slow tick; the per-voxel/per-frame detail is only materialized
near the player on demand.

---

## 4. Statistical culling of "always-converges" candidates

Now the statistical part, scoped correctly by the convergence classes
(`deeptime_convergence_classes.md`):

- **Class B (convergent) cross-region events are the cull target.** A storm that
  will rain itself out, a fire that will burn its fuel and stop — these *always
  converge* to a known end state. Past their convergence horizon you don't
  simulate the trajectory; you **snap to the converged outcome.** That's the
  "cull candidates that always converge" — they're Class B, and culling = snap-
  at-horizon. Deterministic because the horizon and the converged value are
  deterministic functions of the inputs.
- **The statistical jump must itself be deterministic.** "Sample the outcome
  distribution" can't use a wall-clock RNG — it uses a **seeded, tick-and-region-
  keyed PRNG** (hash of `(seed, region_id, tick)` → the same draw on every
  machine). So the approximation is reproducible: same span, same coarse outcome,
  everywhere. This is how you get determinism AND statistics — the randomness is
  pseudo, keyed by the same deterministic inputs.
- **Residual keeps it honest (the dual-path benchmark):** run a span both
  statistically and long-form; the residual is the bounded error. If a
  "converging" candidate's residual exceeds tolerance, it was mis-classified as
  Class B and is actually Class C (needs the trajectory) — fix the class.

So "approximate statistically to cull always-converging candidates" =
**Class-B-snap with a seeded PRNG for any stochastic outcome, residual-checked.**

---

## 5. The voting system — reconciliation, only after 1–4

The "voting among consumers (players)" is a **conflict-resolution policy for the
set algebra** (`timebending_set_algebra.md`), and it only has meaning once the
sim is deterministic — because:

- **If the sim is deterministic, peers shouldn't disagree** given the same
  authoritative inputs. Voting is NOT for "which physics result is right" (fixed-
  point + ordering already guarantee one answer). Voting is for the genuinely
  *ambiguous* cases the divergence set surfaces:
  - which branch is canonical after independent edits (multiplayer timebending),
  - which authority owns a contested region (the min-rule in
    `tick_authority_regions.md` is one such policy),
  - tie-breaks the deterministic total-order can't decide on its own (e.g. two
    players edit the same region's past at the "same" world-tick).
- **Voting resolves DIVERGENCE, not COMPUTATION.** A clean design keeps the
  physics non-voted (deterministic → one answer) and votes only on the
  *policy* questions: canonical branch, region ownership, edit precedence. That
  keeps the common path cheap (no consensus traffic for ordinary ticks) and
  reserves the expensive agreement protocol for actual forks.

This is why it's last: a voting layer over a *non-deterministic* sim would be
trying to paper over desync with consensus (expensive, fragile). A voting layer
over a deterministic sim is small and principled — it only fires at branch
points, which the divergence set already identifies.

---

## 6. The order to actually build this (and the cheap things now)

Strict dependency order — each is unsound without the prior:
1. **Port shared-state fields to `Fixed`** (+ deterministic noise LUT). Bit-
   exactness. Mechanical, do it as each field becomes shared.
2. **Double-buffered cross-region step + tick-stamped edge messages + total-order
   resolution** on the `region_graph`. Cross-region determinism (storms/hunting).
   *The architecturally hard piece — design it carefully.*
3. **Coarse-field, slow-tick cadence** for water/weather (tier-D). Makes "10 days"
   affordable; determinism composes over the steps from 1+2.
4. **Class-B snap + seeded PRNG + residual check** for statistical culling of
   always-converging cross-region events.
5. **Voting only on divergence-set policy questions** (canonical branch, region
   ownership), not on physics.

**Cheap things to carry NOW (zero cost, keep the door open):**
- New dynamic systems: do the math in `Fixed` from the start where feasible, or
  at least keep it *portable to Fixed* (no irrational constants baked into the
  algorithm shape). Cheaper than retrofitting.
- Express cross-region effects as `region_graph::Edge` couplings (already the
  recurring ask) so the double-buffer step in (2) has the graph it needs.
- Any stochastic choice uses a **seeded, `(seed,region,tick)`-keyed** draw, never
  a wall clock — even in the throwaway near-time version. Retrofitting RNG keys
  is painful; doing it right initially is free.

### Invariant
Determinism is **layered, not monolithic**: bit-exactness (fixed-point) →
ordering (double-buffer + total-order edges) → composition (N deterministic steps
= deterministic span) → approximation (seeded PRNG + residual bound) → consensus
(vote only on policy, never on physics). A break at any layer desyncs; each layer
is only meaningful once the one below holds. "10 days of deterministic weather"
is layers 1–3 holding for the step count — no extra mechanism, because
determinism composes.
