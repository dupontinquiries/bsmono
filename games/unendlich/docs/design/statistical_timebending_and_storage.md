# Statistical Long-Time Simulation, Timebending as a Stress Test, and the Storage Problem

> See also `timebending_storage_integration.md` — maps this storage tension onto the REAL files (storage.rs born-procedural+deltas, save_slots.rs name→seed) and the world-menu feature: "load any time" = compute tick T from the nearest checkpoint via the sim-vector chain; storage gains a time axis (tick-stamped edit LOG + sparse coarse-state checkpoints), never the time bulk.

> See also `vector_chaining_and_timewarp.md` — the dual-path benchmark gets per-segment assert points for free from the vector chain's checkpoints (localizes which era drifts), and the timestep animation is its visualization.

> See also `deeptime_convergence_classes.md` — the Step-3 state classification made concrete as three convergence classes (Procedural / Convergent / Path-dependent), which decide depth-bounding per-system instead of a global max-depth flag.

> See also `timebending_set_algebra.md` — the divergence-set framing makes the Step-3 "what to keep vs collapse" question concrete: unique set = keep, shared (intersection) = free by structural sharing.

> Status: design / forward-looking (operator notes, r251). Builds on
> `dynamic_world_architecture.md` (the compute-vs-vision split + tiered mesher),
> `timeline_branching.md` (the branching-timeline model), and `batch_field_lod.md`
> (closed-form vs iterated field catch-up). This doc records a dependency chain
> and an architectural tension, NOT a buildable spec yet. The ordering matters:
> several of these are gated on earlier ones being real.

---

## 1. The end goal — scrub through time and watch the world fly by

The vision: **scrub fast-forward or reverse through a world** and see everything
fly by in a timewarp — water flowing, storms building and discharging, forests
burning and regrowing, populations rising and migrating — all accelerated. Two
things make this powerful:

- **As a feature** it's timebending taken to its natural conclusion: not just
  editing the past and re-running, but *watching* arbitrary stretches of world
  history play forward or backward at speed.
- **As a stress test** it's ideal: it exercises every dynamic system at once,
  across many ticks, far faster than real-time play — so it surfaces both
  correctness bugs (does the fast path match the slow path?) and performance
  cliffs (where does throughput collapse?) that normal play would take hours to
  hit.

---

## 2. The statistical + residual idea

The problem with "fly through 1000 years" is that simulating every tick of every
system for every region is O(ticks × regions × systems) — fine for a few near
regions in real-time, ruinous for a whole world over deep time.

**The idea:** for long stretches, don't simulate tick-by-tick — **collapse many
ticks into a statistical jump plus a residual.**
- **Statistical part:** use the probability distribution of a system's evolution
  to jump its state forward many ticks in closed form (or near-closed-form),
  instead of iterating. "Over 10,000 ticks at this moisture and fuel, this
  forest's burned-area distribution is X; sample the outcome" rather than
  stepping the fire front 10,000 times.
- **Residual part:** keep the *difference* between the statistical estimate and
  what an exact sim would produce, so the fast path stays anchored to ground
  truth and errors don't accumulate silently across jumps.

This is a **generalization of what `batch_field_lod.md` already does**: grass has
a *closed-form* `fast_forward(n)` that exactly equals n front ticks (linear
channels); fire *iterates* because it's nonlinear, but on a coarse field and
capped at convergence. The statistical/residual approach extends that spectrum:
- linear systems → closed-form jump (already have this for grass),
- convergent nonlinear systems → iterate-on-coarse-field, capped (already have
  this for fire),
- **stochastic / chaotic systems → statistical jump + residual** (the new tier).

### The built-in benchmark
The residual gives us a **test suite for free**: run the same span both ways —
the statistical/residual fast path AND the full tick-by-tick long-form — and
assert they agree within tolerance, while measuring both for speed. That's the
"benchmark against the long-form to make sure both are fast and accurate" the
operator wants. The timewarp scrub IS the harness: scrubbing a span runs the
fast path; a debug toggle runs the same span long-form for comparison.

### What makes this HARD (the honest gate)
The statistical collapse is gated on **knowing what to throw away and what to
keep.** You can only collapse the parts of a system whose fine-grained tick
history doesn't matter to any future observer. That requires:
- **classifying state by its observability and reversibility** — which state is
  (a) needed exactly going forward, (b) summarizable to a distribution, (c)
  discardable entirely;
- **datatype structuring in the codebase** to make that classification explicit
  and enforceable — today a system's state is just its fields; there's no marker
  saying "this channel is path-dependent, that one is memoryless." Without that
  structure, you can't safely decide what to collapse, and a wrong choice
  silently corrupts the world.

So this is real work in the *type system / data model*, not just an algorithm.
It cannot come first.

---

## 3. The dependency chain (why this is last, not first)

The operator was explicit: **"first we need to get the other stuff working in
order to get to that point."** The ordering, from `dynamic_world_architecture.md`
forward:

```
   STEP 0 (now, real-time, near regions)
     dynamic systems actually RUN: water flow, wind, storms w/ electrical
     discharge, populations changing — visible in near regions today.
        │  (this is the prerequisite the operator named: "running far regions
        │   today and seeing things like water flow and wind and storms ...
        │   and populations changing over time")
        ▼
   STEP 1  shared coarse regional terrain field (tier D)
     far regions compute their state from a shared field, not per-consumer
     re-sampling. (Also the far-mesh perf fix.)
        ▼
   STEP 2  far regions advance via field fast-forward (batch-as-field), paced
     the world evolves coarsely everywhere, "found in a plausible present."
        ▼
   STEP 3  state classification + datatype structuring
     mark each system's state by observability/reversibility so we know what
     can be collapsed. (The §2 "hard gate".)
        ▼
   STEP 4  statistical + residual long-time jumps
     collapse deep-time spans; residual keeps it honest.
        ▼
   STEP 5  timewarp scrub UI + the dual-path benchmark
     scrub forward/reverse; fast path vs long-form agreement = the test suite.
```

Each step is only sound once the one above it is real. Trying to do the
statistical collapse (4) before the systems actually run (0) or before state is
classified (3) would be collapsing computation we don't yet understand the shape
of.

---

## 4. The storage / disk-IO tension (the part that forces a decision)

This is the deepest consequence and it's worth stating plainly because it
**changes a premise the engine currently relies on.**

**Today's model** (`storage.rs`): chunks are *born procedural*; only explicit
deltas are persisted. A pristine chunk is 0 bytes — regenerable from `(coord,
seed, generator_version)`. The `.mks` files store `(index, block_id)` edit
tuples over the procedural baseline. This is why the world is cheap to store: in
a mostly-static world, almost everything is pristine.

**What dynamic worlds do to that premise:** once water flows, rivers erode,
forests regrow, and fires reshape terrain *on their own*, the world is no longer
"procedural baseline + a few player edits." **Simulation itself becomes a
diverging delta against the generator** — potentially across *every* region that
has ever ticked, not just where the player built something. Naively, that means:
- either **persist every chunk the sim touched** (storage explodes — the
  born-procedural saving evaporates because nothing stays pristine), or
- **don't persist it** and re-derive it — but you can't re-run the generator to
  get it back, because the state is the result of a long stochastic simulation,
  not a pure function of the seed.

**This is exactly where timebending pays for itself — as infrastructure, not
just a feature.** If the world's evolution can be *recomputed* from
`(seed, generator_version, edit-log, tick-count)` via the same
statistical/residual fast path used for the timewarp, then you don't have to
*store* the simulated state at all — you **regenerate it on load by
fast-forwarding from the last cheap checkpoint.** Storage stays small (seed +
edits + sparse checkpoints), and "loading a region" becomes "fast-forward this
region's fields to the current world-tick" — the same operation as scrubbing.

So the dependency runs the other way too: **the statistical fast path is what
makes dynamic-world persistence affordable.** Without it you're forced to the
storage-explosion branch.

### Secondary win: IO and locking
The operator's point — *"being able to timebend for the sake of world disk IO
and locking would be huge."* If regions are regenerable-by-fast-forward rather
than must-be-read-from-disk:
- **less disk IO**: you read a small checkpoint + edit-log and compute forward,
  instead of streaming large materialized region files;
- **less locking**: a region being fast-forwarded on a worker (per
  `tick_authority_regions.md`'s async-tick) doesn't contend on a shared on-disk
  region file; the authoritative state is computed, not a locked resource. The
  "render the last-ready state until the new one lands" rule already covers the
  latency.

### The cheap-immediate-response requirement
The operator named the real constraint: we need **deferred + merged compute for
the long-range systems AND cheap immediate response.** These pull opposite ways
(collapsing ticks saves compute but a scrub/load must feel instant). The
reconciliation is the tiered model again: the **coarse field (tier D) is cheap
enough to fast-forward on demand** for immediate response, while the **exact
long-form runs only where/when fidelity is needed** (near the player, or the
benchmark path). Checkpoints bound how far any fast-forward has to reach.

---

## 5. What this means for sequencing (the actionable takeaway)

- **Do NOT start on the statistical sim or the storage rework yet.** They are
  gated, by the operator's own framing, on the dynamic systems running for far
  regions first (Step 0–2 of `dynamic_world_architecture.md`).
- **The immediate path is unchanged:** get water flow / wind / storms (with
  electrical discharge) / population change running and *visible*, starting with
  the shared coarse regional terrain field (the doc's build step 1), which is
  also the far-mesh perf fix. That work is what makes everything here reachable.
- **When Step 0–2 are real, the FIRST piece of this doc to build is Step 3
  (state classification / datatype structuring)** — because it's the gate on
  both the statistical collapse AND the regenerable-storage model. It's a
  data-model change, best designed once we can see real far-region evolution and
  measure which state is actually path-dependent.
- **Keep the dual-path benchmark in mind from the start:** every dynamic system
  built in Step 0–2 should have a `fast_forward` that's *checkable against* its
  long-form (as grass/fire already do). That discipline is what makes Step 4–5
  possible later, and it costs little to maintain now.

### Invariant to carry forward
A region's authoritative state must be **reconstructable** from
`(seed, generator_version, edit-log, world-tick, sparse checkpoints)` via the
same fast-forward used for scrubbing. If a system's state can't be made
reconstructable that way, that's a signal it needs the Step-3 classification work
before it can participate in deep-time scrub or regenerable storage — it must not
silently force the storage-explosion branch.
