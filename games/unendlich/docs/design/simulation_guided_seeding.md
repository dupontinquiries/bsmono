# Simulation-Guided Seed Selection from Region-Derived Keys

> See also `hybrid_step_model.md` + `hybrid_step_model_mockup.html` — the hybrid that uses R2 as a committed PLANNER (safe) staging coarse deterministic R1 fills, and why path-dependent genome/population dynamics force it (pure R1 loses the trajectory).

> Status: design (operator notes, r255). Sharpens an idea: derive the seed that
> drives a region's statistical jump from the region's *current simulated state*,
> not just its position+tick. Builds on `deterministic_cross_region_sim.md` (the
> seeded `(seed,region,tick)` PRNG) and `timebending_set_algebra.md` (content
> identity per region). The idea is good — with one sharp edge between a powerful
> reading and a determinism-breaking one. This doc pins both.

---

## 1. The idea, precisely

Today's plan (`deterministic_cross_region_sim.md` §4) keys the statistical jump's
PRNG on `(world_seed, region_id, tick)` — **blind to what the region actually
is.** Two regions with the same id+tick but wildly different state (a drought vs.
a saturated floodplain) would draw from the *same* distribution. That's
deterministic but **state-unaware** — it samples "a generic outcome," not "an
outcome appropriate to these conditions."

**Simulation-guided seeding** adds the region's state to the key:
```
jump_seed = hash(world_seed, region_id, tick, digest(region_state))
```
where `digest(region_state)` is a content hash of the region's coarse sim fields
(humidity, instability, fuel, water level, population, …). Now the statistical
outcome is **conditioned on the real state**: the drought region and the
floodplain draw *different* seeds → *different* outcome distributions, each
appropriate to its actual conditions — while staying perfectly reproducible
(same state digest → same draw on every machine, every replay).

This is the difference between "sample a random storm" and "sample the storm this
region's actual humidity+instability would produce." It makes the cheap
statistical path **track the conditioned distribution** instead of a flat prior.

---

## 2. Why this is genuinely powerful (what it buys)

- **Conditioned approximation, not blind.** The whole point of the statistical
  jump (Class C in `deeptime_convergence_classes.md`) is to sample a plausible
  outcome without simulating the trajectory. State-conditioned seeding makes
  "plausible" mean *plausible given these conditions* — the approximation is
  drawn from `P(outcome | region_state)`, which is the correct distribution,
  rather than `P(outcome)` marginalized over all states. Strictly more accurate
  for the same cost.
- **It reuses machinery two other docs already need.** The `digest(region_state)`
  is exactly the **content identity per region** the set-algebra doc requires
  (for "do two branches share this region?"). So one hash serves three roles:
  (a) branch-sharing identity, (b) the dirty-detection signal, (c) the
  conditioning key here. Build it once.
- **It makes the statistical path self-consistent with the exact path.** If the
  exact sim would evolve a region differently based on its state, the
  state-conditioned jump *also* diverges based on state — so the residual
  (dual-path benchmark) stays smaller, because the fast path isn't ignoring the
  very inputs the slow path responds to.
- **Cross-region events get conditioned too.** A storm crossing A→B draws its
  B-side continuation seeded by *B's* state digest, so the same storm behaves
  differently entering a wet vs. dry neighbor — emergent, but deterministic.

---

## 3. The sharp edge: two readings, only one is safe

**Reading A — state CONDITIONS the seed (safe, powerful).** The state digest is a
deterministic *input* to seed derivation. The region's state is whatever the sim
produced; we hash it and use it to pick the draw. No search, no choice — a pure
function `state → seed → outcome`. **This is the good idea. Do this.**

**Reading B — the sim SEARCHES for a seed that gives a "good"/convergent outcome
(unsafe, breaks determinism unless very carefully bounded).** "Try seeds until
one converges / looks right" is a runtime *search*, and it smuggles in three
problems:
1. **Determinism**: a search is only deterministic if its iteration order, its
   termination condition, and its scoring are all bit-exact and identical on
   every machine. Easy to get subtly wrong (float scores, wall-clock budgets,
   "good enough" thresholds) → desync.
2. **It re-introduces the gate it was meant to avoid**: to know a seed gives a
   "convergent" outcome you have to *evaluate* the outcome — which is the
   computation the jump was supposed to skip. Searching seeds to avoid simulating
   can cost more than simulating.
3. **Bias**: selecting seeds for an outcome property (convergence, niceness)
   makes the sample **no longer drawn from the true distribution** — you've
   secretly conditioned on the result, which corrupts the statistics the residual
   benchmark assumes. The fast path would look fast-and-accurate against itself
   while being biased away from the long-form.

So: **state-conditioned seeding YES (Reading A); seed-search-for-outcome NO
(Reading B)** — unless a future, carefully-bounded variant proves its search is
deterministic AND cheaper than simulating AND unbiased (a high bar; treat as a
separate research question, not part of the base design).

A useful litmus: *if the region's state determines the seed, it's Reading A
(safe). If the desired outcome determines the seed, it's Reading B (biased).*

---

## 4. Determinism requirements for Reading A (so it can't drift)

The digest is now part of the determinism boundary, so it inherits every rule
from `deterministic_cross_region_sim.md`:

- **Digest only over `Fixed` state, in a fixed field order.** The hash must be of
  the deterministic fixed-point fields, serialized in a defined order — never
  over `f32`/`f64` (which can differ by a low bit) and never over hash-map
  iteration order (nondeterministic). Same state bits → same digest, everywhere.
- **Digest the COARSE field, not per-voxel detail.** Conditioning is on the
  region's aggregate state (the tier-D field from `dynamic_world_architecture.md`),
  so the digest is small and stable: it changes when the region *meaningfully*
  changes, not on every micro-fluctuation. (Bonus: this is the same granularity
  that makes the set-algebra "is this shared?" identity useful.)
- **Order-safe under the double-buffer.** The digest for tick N's jump is taken
  from the **tick-N (pre-swap) state**, like every other cross-region read
  (§2 of the determinism doc), so it never hashes a half-updated region. The jump
  reads tick-N state + tick-N digest, writes tick N+1, swaps.
- **Stable hash function.** A fixed, versioned integer hash (e.g. a documented
  FNV/xx variant with pinned constants) — not `std::hash::DefaultHasher` (whose
  output isn't guaranteed stable across versions/platforms). Bump a digest-format
  version if it ever changes, exactly like `GENERATOR_VERSION` does for saves.

---

## 5. How it slots into the existing plan (no new layer, a better key)

This isn't a new subsystem — it's a **better key for the PRNG already specified**:

- `deterministic_cross_region_sim.md` §4 said the statistical jump uses a seeded
  `(seed, region, tick)` PRNG. **Change that key to
  `(seed, region, tick, state_digest)`.** That's the entire mechanical change to
  the jump.
- The `state_digest` is the **content identity** the set-algebra doc (§3) already
  wants per region. Build that hash once; use it for branch-sharing, dirty
  detection, AND jump conditioning.
- Convergence classes (`deeptime_convergence_classes.md`) are unaffected in
  structure: Class A still resets (no state to digest beyond seed+pos), Class B
  still snaps at its horizon (its converged value can even *be* part of the
  digest), Class C's jump is now **state-conditioned** instead of blind — a pure
  accuracy upgrade to the same mechanism.

### Build order impact
Slots into the determinism build order as a refinement of step 4 (statistical
jump): once the seeded PRNG exists, extend its key with the region state digest.
Prerequisite: the `Fixed` port (step 1) and the coarse-field digest must exist —
which the set-algebra content-identity work provides anyway.

### Cheap thing to carry now
When you add the coarse regional fields, define their **`Fixed`, fixed-order
serialization + a stable digest** from the start (the set-algebra doc needs it
regardless). Then state-conditioned seeding is "feed that digest into the jump
key" — nearly free — and you've avoided retrofitting a digest later.

---

## 6. Invariant
A statistical outcome is conditioned by **state that already exists** (Reading A:
`state → seed`), never selected to produce a **desired outcome** (Reading B:
`outcome → seed`). The region's state digest is a deterministic input to seed
derivation — `Fixed`-only, coarse, fixed-order, stable-hash, read pre-swap. If
the seed ever depends on evaluating the outcome it's meant to produce, that's a
bias/determinism break, not this design.
