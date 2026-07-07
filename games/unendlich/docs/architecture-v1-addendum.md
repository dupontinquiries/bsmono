# Architecture v1 — addendum (items, scheduling, determinism, noise)

**Status:** Captures the conversation that followed the filing of
`architecture-v1.md`, before that doc gets revised into v2. This is
not the v2 doc; this is the accumulated material that v2 should
integrate.

When v2 of the architecture doc is drafted (next session), it should
incorporate this material into the appropriate sections of
`architecture-v1.md` and supersede this addendum.

---

## A1. Items: trait + context pattern (refines §3.5)

Items are Rust types implementing a trait. The trait has lifecycle
hook methods (`on_right_click`, `on_hit_entity`, `on_tick`, etc.).
The engine has a registry mapping `ItemId → Box<dyn Item>` and
dispatches hooks dynamically.

Common item categories (Sword, Tool, Bow) are composition traits or
helper structs that items own. No deep inheritance hierarchy; Rust
idiomatic.

### A1.1 The `ItemContext`

Every hook receives a `&mut ItemContext` exposing operations the
item is allowed to perform: read world state, write predicted
edits to the overlay, query player state, schedule sounds, spawn
entities, query/set cooldowns, damage the item-stack.

The context is the **API surface**. Its design is the design of
what items can do. It's expected to grow over time as items demand
new operations.

### A1.2 Read batching

Item code that reads many positions in a small region (sphere of
candidates, line cast, AOE check) should NOT call `ctx.block_at(pos)`
in a loop — each call walks overlay-then-octree.

Instead, item code requests a region read:

```rust
let region = ctx.read_region(center, Region::Sphere(4));
for c in sphere_iter {
    let here = region.get(c);  // O(1) array lookup
    // ...
}
```

The region read consults overlay-then-octree once for the whole
region. The per-block lookup inside the loop is a buffer indexing
operation. This amortizes overlay cost over the item's reads.

### A1.3 Write batching (transactions)

Item code that emits multiple edits should batch them into a
transaction:

```rust
ctx.transaction(|tx| {
    for c in &candidates {
        tx.set_block(c.up(), here_below);
        tx.set_block(*c, here_below2);
    }
});
```

The transaction accumulates edits; on exit, they're committed to
the overlay as one entry (or one batch of entries). One overlay
update, one GPU notification.

### A1.4 Reference: Minecraft examples

The conversation referenced concrete Minecraft items
(`EarthWielder`, `LavaWielder`, `ParkourBase` with wall-running) as
exemplars of the item-authoring experience the engine should
support. The trait + context pattern reproduces what those items
need:

- `worldIn.getBlockState(pos)` → `ctx.block_at(pos)` (via region read).
- `worldIn.setBlockState(pos, state)` → `tx.set_block(pos, block)`.
- `player.addVelocity(x, y, z)` → `ctx.apply_velocity(player_id, vec)`.
- `worldIn.addEntity(...)` → `ctx.spawn_entity(...)`.
- `Shapes.makeSphere(...)` → `ctx.shape_sphere(...)` returning an
  iterator over positions.
- Cooldown tracker, sound playing, NBT read/write all map to
  context operations.

---

## A2. Items as data: upload models (refines §3.5, ties to §3.1)

The item's behavior is expressed by a description that compiles
to BOTH a CPU form (Rust code path) and a GPU form (compute
shader). The two forms produce bit-identical outputs given the
same inputs.

The engine generates an **upload model** for each item — a per-item
struct describing the data the GPU form reads as input. Example:

```rust
#[upload_model]
struct ConeBlastUploadModel {
    origin: [f32; 3],
    direction: [f32; 3],
    half_angle: f32,
    seed: u32,
    count: u32,
}
```

The build tool extracts this from the item's description, generates
the CPU packing code, generates the GLSL binding declarations, and
ensures the two stay in sync.

### A2.1 Cross-compilation strategy

Approach is a **restricted Rust subset** with macros enforcing the
restrictions. Item code:

- No heap allocation (no Vec, Box, HashMap, etc.).
- No dynamic dispatch (no `dyn Trait`).
- No recursion.
- No panics in execution paths (validation only).
- No unconstrained loops (loop bounds known at compile time, or via
  iteration over fixed-size structures).
- Only calls APIs in the engine's "GPU-compilable item API."

These restrictions are checked at build time by a custom lint or
macro. Item authors get clear errors when they violate.

### A2.2 What gets to be "an item"

Not all items are GPU-compilable. Some items legitimately need CPU
features (network requests, file I/O, integration with external
systems). These items declare themselves CPU-only and don't get a
GPU form. They cannot run as batched compute dispatches, but they
still go through the item registry and hook system.

The architecture's first-class assumption is that **most items are
GPU-compilable**. Items that aren't are exceptions, and the engine
schedules them differently (CPU thread pool, lower scheduling
priority, no batching).

### A2.3 Batching from upload models

When N items of the same type activate in the same tick, the engine
packs N upload models into a buffer and dispatches one compute call
with N workgroups. Each workgroup reads its slot, executes the
item's GPU form, writes results.

This addresses the per-submit dispatch floor (~550us on AGX) — paid
once for N items, not N times.

### A2.4 Open questions for v2

- **Modding interface.** The trait+context pattern is Rust; modders
  would need to write Rust. A scripting / DSL layer above the trait
  is filed for later.
- **Cross-item composition** (enchantments, status effects modifying
  item behavior). Probably handled by a modifier-chain pattern at
  the context level — modifiers transform the context before the
  item sees it.
- **Hot reload.** Can items be reloaded at runtime without engine
  restart? Affects how cross-compilation is integrated.

---

## A3. Functional units (new — extends §3.5)

Items at the next level of decomposition are **functional units**:
pure-ish operations over world+entity state with explicit inputs
and outputs.

### A3.1 The unit catalog

The engine maintains a library of functional units. Examples:

- `raycast(origin, direction, max_distance) → Hit`
- `sphere_positions(center, radius) → Iterator<BlockPos>`
- `filter_by_predicate(positions, predicate) → Iterator<BlockPos>`
- `apply_block_transform(positions, transform) → Vec<EditOp>`
- `apply_explosion(center, radius, world) → Vec<EditOp> + Vec<EntitySpawn>`
- `propagate_ballistic(entity, world, dt) → NewPosition + Option<Collision>`
- `propagate_beam(origin, direction, world) → Path + Termination`

Each is a pure function: same inputs, same outputs. Each has CPU
and GPU implementations. Each is independently invocable.

### A3.2 Items as compositions

An item's behavior is a composition of functional unit invocations.
The composition itself can be data (a small program of unit calls)
or code (Rust calling units in sequence). v1 of the implementation
is code; later versions may move to data.

### A3.3 Why this matters

- **Profileable.** Cost is attributable to specific units, not
  amortized across "the item system."
- **Schedulable.** Units have explicit inputs/outputs; the scheduler
  can reason about dependencies, batching, deferral.
- **Substrate-agnostic.** A unit's CPU and GPU implementations are
  interchangeable; the engine picks based on cost.
- **Deterministic.** Units with explicit inputs produce explicit
  outputs; no hidden state, no implicit randomness.

### A3.4 Propagation as repeated invocation

Items that propagate over time (arrows flying, fires spreading,
lasers extending) are sequences of unit invocations across ticks.
Each tick: the propagating entity's "advance" unit reads current
state, produces next-tick state. The engine drives the sequence.

Propagation chains are dependency graphs: `arrow.advance(t)` →
`arrow.advance(t+1)` → ... → `arrow.impact()` → `explosion(...)`.
The scheduler resolves the graph each tick.

### A3.5 Tick-level read-write semantics

All functional units within a tick read from the **start-of-tick
state**. All writes accumulate into a delta buffer. At end-of-tick,
deltas merge into canonical state for next tick.

This makes order-within-tick irrelevant (two units writing to the
same position produce a deterministic merge result). It also makes
the architecture parallelizable — units within a tick have no
data dependencies between each other, only between ticks.

---

## A4. Scheduling: deadlines, bounds, stragglers (new)

Items' functional units have variable cost. Variance creates
stragglers. Stragglers cause stutter. The architecture commits to
treating bounded latency as a first-class property.

### A4.1 Per-unit declarations

Each functional unit invocation declares:

- **Maximum work bound**: worst-case input size for this invocation.
- **Split capability**: can this unit be sliced across ticks? If
  yes, with what granularity.
- **Latency target / deadline**: when does the user perceive this
  needs to be done.
- **Defer-cost**: perceptual cost if this unit is delayed.

These declarations come from the item description (the upload model
or its metadata).

### A4.2 Deadline-driven scheduling

The scheduler runs units in deadline order. Units that fit in
remaining budget run this tick. Units that don't either:

- Get split across ticks (if splittable).
- Get deferred to a later tick (if defer-cost is low).
- Get partially executed with visible incomplete state (if neither
  splittable nor deferrable, the engine displays best-effort).

### A4.3 Speculation fills idle budget

When the scheduler finishes its deadline-bound work with budget
remaining, it runs **speculative units** — units predicted (via
the input filter and confidence model) to be needed soon. If the
prediction holds, the work is already done; commit on actual
invocation. If the prediction misses, discard.

Speculation absorbs variance — stragglers that would have caused
stutter become work-done-in-advance.

### A4.4 Block edits as functional units

The CPU overlay's predicted edits (§3.2 of v1) ARE functional units.
A "set this block" is a small unit. A "sphere of blocks from an
explosion" is a larger unit. The scheduler handles all of them in
the same framework.

The overlay isn't conceptually distinct from the scheduler — the
overlay is the queue of pending unit invocations, with each entry
being a unit-invocation with its inputs.

This unification simplifies the architecture: one scheduler, one
prediction model, one confidence framework. v2 should rewrite the
overlay section in these terms.

### A4.5 The "no stragglers" goal

Acknowledged as aspirational. Real-time scheduling is hard; perfect
straggler avoidance requires either over-provisioning or restricting
the design space. The architectural commitment is:

- Stragglers are **visible** to the engine (not hidden).
- Stragglers are **bounded** by declared work limits.
- When a unit exceeds its budget, the engine **knows** and decides
  what to do (skip, defer, display-with-latency).

Failure modes are designed, not emergent.

---

## A5. Determinism and the distribution library (new)

### A5.1 Items declare determinism class

Each item declares its determinism class:

- **`strict`**: deterministic across machines, bit-identical.
  Required for multiplayer rollback, replay, esports-grade fairness.
  Build tool validates that the item only calls deterministic APIs.
- **`single_machine`**: deterministic within one machine across
  runs. Useful for replay on the same machine. Warnings on
  questionable APIs.
- **`none`**: free to use anything. Cannot participate in
  multiplayer / rollback / replay. Loud warnings at build time.
  Runtime markers prevent activation in those contexts.

The declaration is on the item's struct (attribute, trait selection,
or const). The build tool reads it and enforces accordingly.

### A5.2 No ad-hoc random

The engine does not expose a `rand()` function in the item API.
Items that want randomness use the **deterministic distribution
library**.

### A5.3 The distribution library

A library of deterministic spatial / vectorial / temporal
distributions. Item authors call these instead of writing random
math inline. Examples:

- `sample_cone(direction, half_angle, count, seed) → Vec<Vec3>`
- `sample_sphere_volume(center, radius, count, seed) → Vec<Vec3>`
- `sample_disk(center, normal, radius, count, seed) → Vec<Vec3>`
- `sample_uniform_range(lo..hi, count, seed) → Vec<f32>`
- `scatter_along(start, end, count, jitter, seed) → Vec<Vec3>`
- `branching_walk(start, direction, length, branch_prob, seed) → Tree<Vec3>`

Each takes an explicit seed. Each is implemented identically on
CPU and GPU (the build tool generates both forms or shares a
deterministic kernel via the restricted-Rust-subset approach).

### A5.4 Seed source

The engine provides a seed source per item invocation, derived from
`(tick, invocation_id, subscript)`. All three are canonical across
machines. Items request seeds:

```rust
let mut seeds = ctx.seed_source();
let directions = sample_cone(look, 0.15, 8, seeds.next());
let speeds = sample_uniform_range(0.5..1.5, 8, seeds.next());
```

Authors don't think about seed derivation; the source produces
deterministic distinct values.

### A5.5 Escape hatch with declaration

Items that legitimately need non-determinism (debug effects,
single-player-only flair, items with external API integrations)
declare `determinism = "none"` and use a `ctx.nondeterministic_rand()`
API. Build tool emits a verbose warning identifying the item.
Runtime tools surface "this item is non-deterministic" in tooltips
or debug views.

### A5.6 Lazy field evaluation

For volume-distributed randomness (turbulence fields, force fields,
fire fields), the library returns sampleable **field objects**
rather than precomputed buffers:

```rust
let field = TurbulenceField::new(center, params, time, seed);
let velocity_at_p = field.sample(p);  // computes lazily, deterministic
```

This solves the bounded-cost problem: items pay for the samples
they actually use, not for theoretical extent. Fields don't have
to allocate huge buffers; they're functions.

---

## A6. Noise as a GPU-resident field (new — extends §3 components)

Distribution sampling that depends on stable spatial noise (e.g.,
"turbulence is consistent across this region for this duration")
benefits from the noise itself being GPU-resident.

### A6.1 Noise fields as siblings to climate fields

The architecture's climate fields (temperature, humidity, wind —
mentioned in §3.1 and 4 of v1 as GPU-resident state read by
material-response shaders) gain noise siblings:

- Stable scalar noise field (worldgen-style Perlin/Simplex/Worley,
  precomputed for a region).
- Curl-noise vector field (for turbulence, fluid flow visuals).
- Fractal noise field (multi-octave; supports detail at varying
  scales).

These live on GPU as small persistent buffers. Item shaders that
need "random but stable" samples query the noise field instead of
generating new noise on the fly.

### A6.2 The noise compiler

Items describe what shape of noise they need (frequency, octaves,
spatial domain, time dependence). A **noise compiler** at build
time (or at world-load time) produces:

- A sampleable noise artifact (the actual noise buffer or a
  procedural sampler).
- The GLSL function signature for items to query it.
- Bounds and characteristics the scheduler can use for cost
  estimates.

Items don't write noise code; they declare noise needs. The compiler
produces a unified set of noise resources used across all items.

### A6.3 Rollback-pinned noise uploads

Noise refresh (when noise state needs updating — e.g., a turbulence
field advancing in time) is pinned to **rollback boundaries**.
Between rollback boundaries, noise is stable; client and server
agree because they're sampling the same fixed buffer.

On rollback (server disagreement, confidence drop), the noise state
might revert. This is fine because predictions that used the noise
revert with it.

The benefit: noise uploads don't happen every tick. They happen at
rollback boundaries, which are sparse. **Stutter from constant item
spam is avoided** because the underlying noise isn't re-uploaded
on every item invocation.

### A6.4 Item invocations as cheap noise sampling

A spam of fire-spell invocations doesn't trigger N noise uploads.
The noise field is already resident; each invocation samples it.
The per-invocation cost is the sample + the compute work, not the
upload.

This is the key performance answer to "what happens when players
spam items": cost scales with sample-and-compute, not with upload.

### A6.5 Launch-time noise baking (filed for later)

A more ambitious version: at engine launch, simulate likely item
noise profiles and upload them precomputed. Items at runtime
reference the precomputed profiles by ID. Fastest possible item
activation; biggest startup cost.

Filed as a possible future optimization, not v1.

---

## A7. ML-style approximation models for speculation (filed for later)

Speculative execution (running an item's GPU form before the player
actually commits to invoking it) is constrained by the cost of the
real item. If the item is expensive, speculating it costs real
budget; wrong predictions waste real work.

A future-direction option: alongside each item's "real" form
(CPU/GPU compiled from description), the build tool produces a
**cheap approximation model** — a small learned function (probably
literally an ML model, small MLP or similar) that approximates the
real item's output given the same inputs.

Speculative execution runs the approximation. If the prediction
holds, the real item runs at commit and replaces the approximation's
output. If the prediction misses, the approximation's output is
discarded with minimal cost.

This is a research direction, not an architectural commitment. Filed
explicitly so v2 doesn't lose track of it. The ML training pipeline
to produce approximations is substantial; not blocking the
foundation.

---

## A8. Updates to the open questions list

Several of v1's §6 open questions get partial answers from this
addendum:

- **6.4 (Item implementation location)**: Items are cross-compiled
  Rust descriptions producing both CPU and GPU forms via upload
  models. Choice of substrate is the scheduler's, not the author's.

- **6.5 (Entity AI tick location)**: Same answer as 6.4 — entity AI
  is functional-unit code that compiles to both substrates. Engine
  schedules per-entity tick on whichever substrate has budget.

- **6.6 (Edit application hot path)**: Edits are functional units.
  The scheduler handles them. Specific batching and GPU integration
  is implementation detail to be designed.

New open questions for v2:

- **A8.1**: How does the build tool validate the restricted-Rust
  subset? Custom lint, procedural macro, or compiler plugin?
- **A8.2**: What's the syntax for declaring upload models, unit
  metadata (deadlines, bounds, split capability), and determinism
  class? Attribute macros, trait selection, or const expressions?
- **A8.3**: Tick boundary semantics — is "tick" a fixed wall-clock
  interval, or game-time interval, or variable-driven? Affects
  scheduler design.
- **A8.4**: How are functional unit dependency graphs resolved?
  Topological sort each tick? Speculation across the graph? This is
  the core scheduler algorithm and needs its own design pass.

---

## A9. Acknowledgment of scope

This addendum + v1 together describe an engine architecture with:

1. GPU-resident octree world
2. Speculation-with-confidence CPU overlay
3. Signal-filtered inputs with input filter
4. Functional unit decomposition
5. Deadline-driven scheduling with speculation
6. Cross-compiled items with upload models
7. Deterministic distribution library
8. GPU-resident noise fields with noise compiler
9. Determinism-class declarations
10. Network rollback via the overlay/scheduler unification
11. Entity state on GPU with parallel overlay
12. (Future) ML-style approximation models for speculation

This is genuinely a complete game engine architecture. Most
pieces exist in production engines somewhere. The combination is
novel.

The implementation is multi-month minimum, multi-year realistic.
Each piece interacts with others; intermediate states will be
partial.

The architecture doc's job is to make this set of decisions
explicit so that implementation increments don't drift. v2 of
the doc (with this addendum integrated) is the long-lived
reference.

---

## Process note

This addendum and v1 of the architecture doc came out of one long
conversation that spanned the chug arc's resolution, the GPU mesher
proof-of-concept, and an architectural pivot. The conversation
produced more material than fit cleanly in a single document; this
addendum captures the surplus.

The pattern that worked: user articulates direction, Claude
synthesizes and pushes back on cracks, user refines or adds. Several
turns of this produced a sharper architecture than either party
alone would have written.

The pattern that nearly didn't work: ideas accumulated faster than
they got pinned down. The risk was code shipping based on incomplete
architecture, repeating the chug arc's "ship many increments before
recognizing the shape" failure. Recovery was explicit: pause coding,
file what we have, return to coding only after architecture is
clear.

For v2 drafting: integrate this addendum's sections into the
appropriate parts of v1, retire this addendum once integrated, and
treat v2 as the single canonical reference going forward.
