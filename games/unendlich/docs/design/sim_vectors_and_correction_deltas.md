# Sim Vectors: Checkpoint Directors, Convergent Endpoints, and Correction Deltas

> See also `perception_and_population_behavior.md` — real-time social behaviour is PERCEPTION-gated (a creature reacts to attacks it can SEE: help/flee/attack via the arbiter), while population-level behaviour is the deep-time GENETIC channel here, computed statistically over a sim-vector jump (the omniscient herd math), never per-frame. The missing heritable temperament/sociality gene is what makes that literal.

> See also `deeptime_convergence_classes.md` §4.5 — how to DECIDE whether the behaviour/habit channel even needs simulating in a region: structural coupling-reachability first (free), then a coarse pre-bracket, then a full parallel-vector bracket only if ambiguous; cache the verdict per region-type.

> See also `adaptive_scheme_selection.md` — a sim vector has a fidelity LENGTH set by its most path-dependent modeled system; the selector picks sim-it (small span) / single-vector (medium) / drive-then-PREFILL (large), where the drive is cheap+approximate and a short sim-prefill makes the LANDING exact (materializing building/growth so the dragon's structure is really there on arrival).

> Status: design (operator notes, r257). Extends `hybrid_step_model.md` (R2-plan/
> R1-fill), `simulation_guided_seeding.md`, `deeptime_convergence_classes.md`.
> Captures four mechanisms the operator added after seeing the mockup
> (block-placement / structure persistence, population-state approximation with
> 8³ binning, correction-vector deltas, lifespan pop-in), names the checkpoint
> directors **sim vectors**, and answers precisely: "the hybrid graph CHANGES
> with stage count but the alleles/population still CONVERGE — is that good news
> for determinism at scale?"

---

## 0. The determinism question — the careful answer (yes, with one sharp caveat)

**Observation (from the mockup):** at 19 days / storm 1.0, sweeping waypoints from
1→19 stages leaves the *endpoints* (allele mean, population) converging to ~the
same place, but the *graph* (the path of intermediate nodes) changes.

**Is that good for determinism at scale? Mostly yes — but only if you say what
kind of "same" you mean, because two very different properties are hiding here:**

1. **Endpoint robustness / attractor stability (what you're seeing) — GOOD.**
   The convergent channels relax to the same attractor regardless of how finely
   you staged the fill. This is *stability*: the answer is insensitive to the
   discretization. That's exactly the property that makes coarsening *safe* — you
   can spend fewer stages and still land in the right place. Genuinely good news,
   and it's the justification for the whole hybrid: convergent channels tolerate
   coarse stepping.

2. **Bit-exact determinism (NOT what changing-graph shows) — a SEPARATE thing.**
   "Deterministic" in the netcode sense (`deterministic_cross_region_sim.md`)
   means *the same chosen configuration replays to the bit*. It does NOT mean
   "different stage counts give the same result." Two different waypoint counts
   are two different *computations* — they SHOULD be allowed to differ, and the
   fact that they *nearly agree* is robustness, not determinism.

**Why conflating them is dangerous:** if you treat "the endpoints converge no
matter the stage count" as "it's deterministic, so stage count doesn't matter for
replay," you'd let two machines pick *different* stage counts and assume they
agree. They agree *approximately* (robustness), not *exactly* (determinism) — and
approximate agreement desyncs a bit-exact lockstep within a few steps. So:

- **For determinism:** the stage count (and the whole waypoint plan) must be
  **committed and identical on every machine** — it's part of the seed lineage
  (`hybrid_step_model.md` §2). Replay re-uses the committed plan; it does not
  re-pick stages.
- **The convergence you observed then becomes a SAFETY MARGIN, not the
  determinism mechanism:** because the result is robust to stage count, small
  differences (a machine that legitimately must re-stage, a checkpoint taken at a
  different granularity) produce only small divergence that the **correction
  deltas (§3) and residual snap back** — instead of snowballing. Robustness buys
  *forgiveness*; commitment buys *exactness*. You want both.

**One-line answer:** yes, it's good news — the attractor stability means coarse
staging is *safe and the error stays bounded*, which is what makes deep-time
scale affordable. But it is **robustness, not determinism**; determinism still
requires committing the plan. Robustness is what keeps the committed-plan replay
*cheap and self-correcting*, not what replaces it.

(Caveat the mockup itself: the *endpoints* converge but watch the path-dependent
channels' intermediate values — at very low stage counts the allele trajectory's
*shape* is lost even though its endpoint is close. Endpoint convergence ≠
trajectory fidelity; the genome case from `hybrid_step_model.md` §3 still needs
enough stages for the path, even though the final number looks stable.)

---

## 1. Sim Vectors — the checkpoint directors

Name for the things that direct what happens at/between checkpoints:
**sim vectors.** A sim vector is the committed, deterministic instruction that
carries a region from one checkpoint to the next — the unit the R2 planner emits
and the R1 fill consumes. It is *data in the seed lineage*, not a live decision.

A sim vector carries:
- **the span** it covers (from-tick → to-tick),
- **the seed lineage key** (`hash(seed, region, tick, digest(state))`),
- **the waypoint plan** (stage boundaries — the committed stage count, so replay
  is exact),
- **the correction delta(s)** that fired in this span (§3),
- **structure/event commitments** that must be applied (§2 — "dragon built here").

"Sim vector" because it's directional (carries state forward along a committed
heading) and it's what you scrub along. The checkpoint graph is a graph of states
*connected by sim vectors*.

---

## 2. Structure & event persistence (the block-placement / "dragon still building" case)

The operator's missing piece: deep-time isn't only *fields* (water/fog/genes) —
it's also **discrete events that leave permanent marks**: a dragon building a
structure, a player's block placement, a lair dug, a dam that diverts a river.
These can't be a statistical blur; they're **edits with provenance**.

Mechanism: discrete world-changing events are **commitments attached to a sim
vector**, applied at the checkpoint, with a *condition* evaluated against the
coarse state:
- *"is the dragon still alive and building here?"* → the sim vector carries the
  dragon's lifespan + a build-rate; at the checkpoint, if the conditioned coarse
  state says the dragon survived (its lifespan didn't expire, its territory's
  food/water held), **apply the next increment of its structure** as a committed
  edit (`+N blocks at this footprint`). If it died, stop applying.
- *"did the water all flow out? → population is thirsty"* → the water channel's
  converged value is a **trigger**: a coarse predicate (`water < dry_threshold`)
  fires a population-stress event committed to the vector, which feeds the
  population channel's next step. The event is the coupling, evaluated once at the
  checkpoint, not every tick.

Key: these are **conditional committed edits** — deterministic (the condition is a
pure function of the committed coarse state), permanent (they enter the edit-log,
so they survive like player edits in `storage.rs`), and cheap (evaluated per
checkpoint, not per tick). Structure-building over deep time = "apply this
footprint increment each checkpoint the builder is still alive," which is a
handful of edits per era, not a simulation.

---

## 3. Correction deltas / correction vectors (the operator's "don't blur angles that never change")

The sharp idea: **don't statistically blur every dimension — only the ones that
actually changed.** Most of a region's state is unchanged or convergent across a
span; blurring all of it loses precision on the axes that *did* move for no
benefit. Instead:

- Carry the region's state forward by the **coarse/convergent path** (cheap), and
- track a **correction delta** only for dimensions with a *new response* to a new
  condition. "It got dry" is a new condition → add a correction vector to the
  population/thirst dimension for this span; leave every other dimension on its
  unchanged convergent track.
- The sim vector then = (convergent baseline) + (sparse correction deltas for the
  dimensions that responded). You "only measure new responses with correction
  vectors," so the dimensions that never change are never re-estimated and never
  blurred.

This is the deep-time analogue of "persist only deltas" (`storage.rs`) applied to
*dynamics* instead of blocks: the baseline is the cheap convergent evolution, and
the corrections are the sparse, real, measured responses to new conditions. It
keeps the statistical approximation from smearing the many axes that are actually
stable — directly the operator's concern.

---

## 4. Population-state approximation: genome summary + 8³ binning redistribution

The operator's concretization of the genome problem (`hybrid_step_model.md` §3):
instead of fully re-simulating a population, **summarize its genetic makeup and
redistribute via a single coarse step with 8³ spatial binning + counts.**

- **Summarize**: a population's genome over a region collapses to a compact
  **distribution** — per-gene mean + variance (or a small histogram), plus counts
  by 8×8×8 spatial bin. Not per-individual; an aggregate.
- **Redistribute (one step)**: to advance, run *one* coarse step that:
  - shifts each bin's genome summary by the selection+drift the conditioned state
    implies (the moving optimum from §0's caveat),
  - moves counts between adjacent bins for **spreading** (a faster-growing tree
    gene covers more area → its bin counts diffuse outward, including across
    region boundaries — the "spreading that genome to other regions" case),
  - re-pops **named/long-lived individuals** explicitly (§5), aggregate for the
    rest.
- **Why 8³**: coarse enough to be cheap and stable (you're not tracking every
  organism), fine enough to preserve *where* a gene is winning so spread is
  spatial, not global. The bin grid is the population's tier-D field.

So *"did the trees evolve a faster-growth gene? then they cover more area and
spread that genome to neighbors"* becomes: the gene's bin summary shifts toward
the faster allele (selection), its counts diffuse to adjacent bins (spread), and
the cross-region edges carry the summary into neighbors (propagation) — **one
coarse step on a binned summary, not a full regrow sim.** This is the population
counterpart of the field fast-forward, and it stays stable because it's an
aggregate redistribution, not per-organism chaos.

> Honest note: 8³ is a starting granularity to validate, not a proven constant.
> Like every LOD bucket here, the right bin size is empirical — measured against
> the long-form once the population sim runs. It may differ per species (a herd
> vs. a forest) or per gene's spread rate.

---

## 4.5. Behavioral habit: a fast individual channel that fights the genetic trend

The genome channel (§4) evolves on a **slow population timescale** — selection and
drift over generations. But individual animals also carry **learned behavioral
habit** that evolves on a **fast within-lifetime timescale**, and crucially the
two are coupled and can **oppose** each other. This is a distinct path-dependent
channel the timebending model has to carry separately — you cannot re-derive
behavior from the genome after a jump, because habit is not genetic.

The concrete case (now in code, gated behind `UNENDLICH_ARBITER`): a creature has
a `combat_habit` in [0,1] that **strengthens each time it commits to a fight** and
**decays slowly** otherwise; it biases effective aggression upward. So a
battle-hardened individual keeps fighting even when its genome/temperament — and
the population trend — would say flee. **Habitual animals fight the population
genetic trend.** A population can be genetically drifting toward timidity while a
cohort of veterans stays aggressive because they *learned* to, and that learned
layer is invisible to the genome summary.

Why this matters for the jump:
- **Two clocks, not one.** The genetic channel coarsens with depth (§4, generations
  blur into a distribution); the habit channel turns over much faster (a lifetime,
  often a few in-game days) AND resets at death — a newborn inherits genes, not its
  parent's scars. So a jump must advance them on **different cadences**: genome via
  the 8³ summary redistribution, habit via its own fast relaxation toward the
  behavior the era's conditions reinforce.
- **They can diverge, so you can't collapse one into the other.** If you derived
  behavior purely from the evolved genome you'd erase the veteran cohort; if you
  froze habit you'd keep ghosts fighting long after the threat that trained them is
  gone. The jump carries habit as its **own correction-delta dimension** (§3): blur
  it only where it actually responded (regions that saw conflict), leave it at
  baseline elsewhere.
- **Hysteresis is the same phenomenon at the tick scale.** The arbiter's
  per-individual `prev_behavior` bonus (don't flip-flop on near-ties) and the
  `combat_habit` accumulator are the **short- and medium-memory ends of one
  spectrum**: hysteresis = seconds (don't thrash), habit = a lifetime (learned
  disposition), genome = generations (heritable temperament). A faithful deep-time
  sim models all three, on their own clocks, with habit explicitly able to run
  counter to genome.

**Lifespan interaction (ties to §5):** still-alive individuals carry their habit
exactly (popped in verbatim — a veteran is still a veteran on arrival); turned-over
cohorts are estimated from genome + the era's reinforcement, NOT from the dead
individuals' habit (it died with them). So the habit channel rides the
**alive=exact / turned-over=estimated** split, but estimates the cohort's *baseline*
habit from conditions, not inheritance.

> Honest status: `combat_habit` + hysteresis are implemented in the live arbiter
> (fast end). The genome-vs-habit opposition as a *deep-time jump metric* — two
> cadences, habit as its own correction-delta, baseline-habit estimation for new
> cohorts — is design here, gated on the population sim running. There is also no
> heritable temperament gene yet (the genome carries color/size/senses/speed); a
> `temperament` locus is the missing piece that would make "fights the genetic
> trend" literal rather than habit-vs-a-proxy. Filed.

---

## 5. Lifespan pop-in: keep the living, estimate the unborn

The operator's closing mechanism, and it's the clean bridge between aggregate and
individual: when materializing a region after a deep-time jump,
- **entities whose lifespan is still alive at the target tick are popped in
  explicitly** — their committed state (the named dragon, the elder of the herd)
  is restored exactly, because they're in the edit-log / sim-vector commitments,
  not blurred into the aggregate;
- **entities whose lifespan ended during the span are estimated as new
  generations** — the aggregate genome summary (§4) produces the replacement
  population; you don't track the births individually, you *estimate the cohort*
  from the distribution.

This splits the population cleanly by the determinism classes:
- **named / long-lived / still-alive** → Class-C-exact, committed, popped in
  verbatim (their structure-building per §2 continues);
- **short-lived / anonymous / turned-over** → aggregate, estimated from the binned
  summary (§4).

So "pop in the entities with lifespans that are still alive and estimate new
generations for the ones below" = restore the committed survivors exactly,
synthesize the turned-over cohort from the summary. The expensive exactness is
spent only on the entities that persist long enough to matter.

---

## 6. How it all assembles (one region, one deep-time jump)

```
checkpoint A (committed state + digest)
   │  sim vector  =  span + seed-key + waypoint plan
   │                 + correction deltas (only the dimensions that responded)
   │                 + structure/event commitments (conditional edits)
   ▼
R1 fill (coarse sub-steps; convergent channels relax, Class-C channels stepped
         per waypoint; genome summary redistributed on the 8³ bins)
   │  at each checkpoint: evaluate conditions → apply committed edits
   │  (dragon alive? +structure increment.  water dry? +thirst delta.)
   ▼
checkpoint B (new committed state + digest)  ──merge if digest matches a seen bucket
   │
   ▼  ... on materialize: pop in still-alive committed entities exactly;
          estimate turned-over cohort from the binned genome summary.
```

Everything here is **committed data replayed deterministically** (sim vectors in
the seed lineage), with the convergent-endpoint robustness from §0 as the safety
margin that keeps small unavoidable differences from snowballing.

---

## 7. What to carry now + invariant

**Cheap now (door-openers, zero cost):**
- Reserve the **sim vector** as the unit the planner emits + the fill consumes,
  carrying {span, seed-key, waypoint plan, correction deltas, event commitments}.
  Even before deep-time, structure-building and field coupling can be expressed as
  conditional committed edits on a vector.
- Population state: define the **genome summary** (per-gene mean/variance + 8³ bin
  counts) as the persisted population form from the start — it's what both the
  redistribution step and the storage model want.
- Tag dimensions as **convergent vs responding** so correction deltas only track
  the responding ones (the §3 sparsity).

**Invariant:**
- **Convergent-endpoint stability is a SAFETY MARGIN, not a determinism
  guarantee.** Determinism = the committed sim vector (incl. its waypoint plan)
  replays bit-exact; the robustness only bounds the error of *unavoidable*
  differences and lets correction deltas heal them. Never let two machines pick
  different plans and assume agreement.
- **Discrete marks (structures, deaths, builds) are committed edits with
  provenance, never statistical blur.** Only the *aggregate, anonymous, turned-
  over* part of a population is summarized; survivors and structures are exact.
- **Blur only what responded.** Dimensions that didn't change are carried on their
  convergent track, never re-estimated — correction deltas are sparse by design.
