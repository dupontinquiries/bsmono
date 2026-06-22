# STRUCTURES — how creatures build

*Design doc · Worker T01 · round 15. Prose + sketches, no code. Describes a
system that builds on existing sim cores; nothing here is implemented yet.*

## Goal

Some creatures should leave **deterministic, durable marks on the world**: bees
raise comb, wood-dragons terraform keystone groves, and a rare *architect* lineage
designs idiosyncratic structures with a recognizable style. Building must roll back
and replay identically on every peer, and must never let a creature grief the world
(no unbounded excavation, no spawn-camping terrain).

This sits on top of cores that already exist: `genetics` / `egg_genome` (heritable
bytes), `batch_gene` (genome → phenotype, r14), `grove_decay` (grove vitality, r10),
`neighbor_grid` (cheap local queries), and the world-edit path, where block edits
already enter the simulation as **deterministic events in the input stream** rather
than as direct mutation (`world_authority`).

## The architect / builder gene (§6A)

A builder is described by three heritable things, all decoded from the genome the
same way `batch_gene` decodes phenotypes:

- **Aesthetic vector** — a small fixed-length `[f32; N]` that biases *what* gets
  built (symmetry, height bias, openness, ornamentation, palette). It is the
  structure's "taste".
- **Capability budget** — hard caps on *how much*: blocks edited per tick, total
  blocks over a lifetime, maximum reach from the body, and an allow-list of block
  kinds it may place or remove. The budget is the anti-grief boundary (see below).
- **Plan cursor** — where the creature is in its current structure, so building is
  *incremental*: one bounded edit per build-tick, resumable after a rollback.

Each build-tick the creature proposes at most one edit (place/remove one block, or a
tiny fixed group) chosen by a pure function of `(aesthetic_vector, plan_cursor,
local occupancy from neighbor_grid, tick)`. The proposal is emitted as a world-edit
event; `world_authority` applies it. Because the proposal is a pure function of
already-deterministic inputs, every peer proposes the same edit on the same tick.

### Deterministic aesthetic-vector update

The aesthetic vector drifts slowly so a builder's style evolves without becoming
random. Each update is `aesthetic += rate · nudge(seed, tick)` then clamped, where
`nudge` is a seeded hash of `(creature_id, tick)` — no clock, no RNG, no float
nondeterminism beyond the same `f32` ops every peer runs. Reinforcement (a finished
structure that survived, a grove that thrived) biases the nudge toward the choices
that worked, so style is heritable *and* learned, but always replayable.

## Bee hives — comb on a lattice

Comb is the constrained case: cells sit on a fixed hexagonal **lattice** anchored at
the hive origin, so a bee never chooses free coordinates — it chooses the next empty
lattice cell adjacent to existing comb (nearest-first via `neighbor_grid`). This
makes hives dense, regular, and trivially deterministic, and caps growth naturally
at the lattice the colony can reach. The aesthetic vector barely applies here; the
capability budget still does.

## Dragon keystone groves

A wood-dragon terraforms a **keystone grove**: a patch of habitat it grows by
placing foliage/soil blocks within its budget, raising the grove's vitality. Vitality
is exactly the `grove_decay` (r10) bar — it climbs while the dragon (the grower) is
present and decays when the dragon dies or leaves, eventually returning the terrain
toward its prior state. So a grove is a *living* structure: built incrementally,
sustained by presence, and reclaimed on death rather than left as permanent scar.

## Capability bounds (anti-grief)

Building can only ever be safe if the budget is enforced *before* the edit reaches
the world, not after:

- per-tick and lifetime block caps, checked at proposal time;
- a reach limit (no editing far from the body);
- a block-kind allow-list (no touching player builds, bedrock, or spawn-protected
  regions);
- groves and comb decay/revert, so even a maxed-out builder leaves nothing permanent
  it had to be "cleaned up".

Because edits flow through `world_authority` as events, these checks live at one
choke point and apply equally to every builder.

## Open questions

- Should the aesthetic vector be a single genome trait set, or split into
  "inherited taste" vs "learned taste" with different drift rates?
- Lattice hives are easy; do free-form architect structures need a small grammar
  (an L-system over the aesthetic vector) or is greedy adjacency enough?
- How do two builders sharing a region arbitrate overlapping plans deterministically
  — first-by-entity-id, or a spatial claim in `neighbor_grid`?
- What's the right reclaim curve for a grove whose dragon died mid-build — instant
  decay-start, or a grace period?
- Should capability budgets be visible to players (a tameable, inspectable stat) or
  hidden engine tuning?
