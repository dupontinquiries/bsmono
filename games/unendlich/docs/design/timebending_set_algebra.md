# Timebending as Rebase + Bounded Set Algebra over a Branch DAG

> See also `deeptime_convergence_classes.md` §4.5 — perturbation BRACKETING (parallel sim vectors with extreme channel values) is how you DETERMINE whether a channel converges: it's literally the difference operation here, empty divergence ⇒ merge ⇒ the channel is irrelevant. Cheaper pre-filters (coupling-graph reachability via rollback_closure, coarse pre-bracket, sensitivity bound) prove irrelevance without the full parallel sim.

> See also `simulation_guided_seeding.md` — the per-region content identity defined here doubles as the conditioning key for state-aware statistical jumps (one digest serves branch-sharing, dirty detection, and seed conditioning).

> See also `deterministic_cross_region_sim.md` §5 — the "voting among consumers" reconciliation is a conflict-resolution policy on the divergence set (canonical branch / region ownership), and only meaningful once the sim is deterministic (vote on policy, never on physics).

> See also `deeptime_convergence_classes.md` — the convergence classes are what keep a branch's divergence set FINITE as time→∞ (Class A free, Class B snaps at its horizon, Class C statistically coarsened).

> Status: design / theory framing (operator notes, r252). Sharpens what
> `timeline_branching.md` models (the branch DAG) and what
> `statistical_timebending_and_storage.md` needs (reconstructable region state)
> into the actual computer-science shape of the problem: timebending is **base
> rewriting + set operations (intersection / union / difference) over bounded
> regions in a history graph.** This is a framing doc, not a buildable spec —
> but it names the structures so the data model can be designed right.

---

## 1. The reframe: it's not "rewind," it's rebase + merge

"Rewind the world" undersells it. What's actually happening:

- An edit in the past **rebases** the world's history onto a new base (the
  edited state at the fork tick), and everything downstream re-derives from
  there — exactly like rebasing a commit and replaying the patches on top.
- Because the pre-edit future still "happened" (`timeline_branching.md`: we keep
  it, stale), histories form a **DAG of branches**, not a line. Forks can target
  any tick on any branch, including ancestors (looping).
- The interesting work is then **comparing and combining branches**: which parts
  of the world are *shared* across branches (untouched by the divergence), which
  *conflict* (the same region evolved differently), and which are *unique* to one
  branch. That is **set algebra over bounded regions** — intersection, union,
  difference — keyed by region and tick.

So the three operations you named map directly:
- **Intersection** → regions two branches share unchanged (the common base; can
  be stored once and pointed at by both — structural sharing).
- **Union** → the full footprint a merge/scrub must consider (everything any
  branch touched in the span).
- **Difference / unique** → the *divergence set* of a branch: the minimal data
  that distinguishes it from its parent. This is the thing you actually have to
  store or recompute per branch; keeping it minimal is the whole game.

---

## 2. This generalizes machinery the engine ALREADY has

The linear case is built: `world/rollback_scope.rs::rollback_closure(changed,
edges, min_coupling, max_hops)` computes the **transitive set of regions coupled
to a change**, over the `region_graph::Edge` coupling graph, **bounded** by
`max_hops` (the blast radius) and `min_coupling`. `rollback_fraction` reports how
much of the world that set is. That's already:
- a **bounded** region set (max_hops caps the blast radius),
- computed over a **graph** (region coupling edges),
- the "what must be re-simulated" **closure** of a change.

Timebending is the multi-branch generalization:
- linear rollback = one branch, one closure, replace-because-more-correct;
- timebending = many branches, **set operations between their closures**,
  keep-because-divergent.

The `max_hops` bound is exactly the "bounded world merging" instinct: a past edit
does not dirty the entire world — its effect propagates only so far through the
coupling graph before it's negligible. The divergence set of a branch is
**bounded by the coupling-graph closure of its fork edit**, not the whole world.
That's what makes the set algebra tractable instead of O(world).

---

## 3. The data structures this implies (where the "datatype structuring" lands)

To do set ops over (region, tick) efficiently across a DAG, the state model
needs structure it doesn't have today:

- **Region-keyed, tick-versioned state with content identity.** Each region's
  state at a tick needs an identity (a hash / version) so "do these two branches
  share this region unchanged?" is an O(1) identity compare, not a deep diff.
  This is the classic **persistent (immutable) data structure** move: branches
  share unchanged sub-trees by pointer, and only divergent nodes are distinct —
  structural sharing makes intersection cheap and storage small. (Same idea as a
  git tree, or a hash-consed octree.)

- **The branch DAG carries, per branch, its divergence set** = the regions whose
  identity differs from the parent (the rollback_closure of its fork edit). Union
  across branches = union of divergence sets; intersection = the complement
  (shared base); unique = a branch's divergence set minus the others'.

- **Bounded merge** = when reconciling branches (e.g. a multiplayer authority
  merge per `tick_authority_regions.md`, or collapsing a loop), only the union of
  divergence sets within the coupling-bounded blast radius is examined; the
  shared base is untouched by construction.

- **Conflict = same region, two branch identities, neither an ancestor of the
  other.** Resolution policy (last-writer / authority-wins / deterministic merge)
  is a separate decision, but *detecting* conflicts is just identity compare over
  the union set.

This is the same `(seed, gen_version, edit-log, tick, checkpoints)`
reconstructability invariant from `statistical_timebending_and_storage.md`, now
with the added requirement that **region state be content-addressed and
structurally shared** so the set algebra is identity-compare, not deep-compare.

---

## 4. Known theory this rhymes with (so we don't reinvent it)

Worth borrowing vocabulary/structure from, when this gets built:
- **Version control / rebase + 3-way merge** — the branch DAG, fork points,
  conflict = divergent edits to the same object, structural sharing of unchanged
  trees. The mental model is "git for world-state, keyed by region×tick."
- **Persistent / immutable data structures + hash-consing** — make intersection
  and "is this shared?" O(1) by content identity; make storage proportional to
  *divergence*, not total state.
- **CRDTs / operational transforms** — if branches must merge *automatically*
  (esp. multiplayer authority reconciliation), the per-region state may want a
  merge function with the right algebraic properties (commutativity/idempotence)
  so order-of-arrival doesn't matter. Not all world state can be a clean CRDT —
  that's another axis of the Step-3 state classification.
- **Interval / range trees + bounded BVH** — to query "which regions did branch B
  touch in tick span [t0,t1]" and to bound the blast radius spatially.

---

## 5. Why this is the right hard problem (and still gated)

This is the unifying abstraction under the last three docs:
- **dynamic world** needs regions to evolve and be reconstructable → content
  identity per region.
- **statistical timebending** needs to know what to keep vs collapse →
  divergence sets tell you exactly what's unique (keep) vs shared (free).
- **storage/IO** needs to not store everything → structural sharing means you
  store the divergence sets, not the world; intersection is shared by pointer.

So the "giant graph + set algebra" framing isn't a tangent — it's the **data
model that makes all three affordable at once.** A branch's cost (storage,
merge, scrub) becomes proportional to its *divergence*, bounded by the coupling
graph, which is exactly the property you want.

**Still gated, same as before:** this is the shape Step 3 ("state classification
+ datatype structuring") should aim for, but it's only designable once the
dynamic systems actually run and we can measure real divergence sets — how far a
typical edit's blast radius reaches through the coupling graph, how much of a
region's state is content-stable vs churning. Build the systems (Step 0–2)
first; let the real divergence behavior inform the identity/sharing scheme rather
than guessing the granularity up front.

### One concrete thing to carry now (cheap)
The linear `rollback_closure` + `region_graph` already exist and already produce
bounded change-sets. When dynamic systems are added, keep their cross-region
effects expressed as **`region_graph::Edge` couplings** (not ad-hoc reaches), so
the same closure machinery measures their blast radius. That keeps the door open
to the multi-branch set algebra later, at no extra cost now — every system that
routes its coupling through the region graph is automatically a citizen of the
eventual merge algebra.
