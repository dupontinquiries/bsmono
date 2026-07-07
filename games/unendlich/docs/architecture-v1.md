# Unendlich engine architecture v1 — destination design

**Status:** v1. Captures the architectural direction articulated across
sessions through the end of v0.24.6-gpu-mesh-bench. The user is
preparing a v2 with additional postulations; this v1 is the substrate
that v2 builds on or revises.

**This document is not an implementation plan.** It describes the
destination architecture and the reasoning behind it. The staged
shipping plan (which increments land in what order, with what
falsification criteria) is a separate concern — to be drafted after
v2 of this doc lands.

**Origin:** This direction emerged from the chug arc (increments
42-48). After exhausting CPU-side optimizations for shadow rendering
on AGX/Honeykrisp, the conversation pivoted to "what does the engine
look like if we don't fight the platform" — and from there into
"what if simulation and rendering share the GPU substrate."

---

## 1. The destination in one paragraph

The world is GPU-resident as an octree. CPU is the orchestrator, edit
source, and signal processor for inputs. Predicted edits live in a
small CPU-side overlay that the GPU consults during rendering and
meshing. Predictions have confidence values that decay with rollback
or promote to canonical state on confirmation. Player inputs are
treated as continuous signals — filtered, characterized for
predictability, and used to extrapolate near-future actions. Items
and abilities are operations expressed against the world+entity
substrate; simple ones run on CPU, complex ones (raycasts,
region-of-effect) run as GPU compute. Entities are first-class
parallel to world state with their own overlay.

The unifying principle: **state lives where it's computed against.**
Bulk world state on GPU. Transient predictions on CPU. The boundary
between speculation and authority is the boundary between memory
spaces.

---

## 2. Why this direction

### 2.1 What the chug arc taught us

The arc spanned increments 42-48 (chug-debug-1 through shadow-stride).
The chug at rd=8 was characterized as:
- Shadow GPU time dominant (96% of frames over 8.3ms budget at 2048²).
- NOT fragment-bound (4× area reduction = 5% time reduction).
- NOT upload-contention-bound (643 high-upload frames vs 73 chugs).
- NOT multi-queue-fixable (M2 has single queue family).
- Driven by shadow rendering 16× more chunks than main (cull_count
  data showed mean ratio 16:1).

The fix for that specific chug is shadow LOD or cull-tightening (Layer
B-adjacent work, filed). But the deeper lesson:

**CPU-era abstractions don't compose well with TBDR + AGX firmware.**
Each chug intervention we tried (multi-queue, smaller shadow map,
upload bytes budget, temporal shadow stride) hit a platform constraint
we didn't fully understand. The fixes that would work on immediate-mode
discrete GPUs don't help on AGX. Specifically:
- LOAD render pass on TBDR pays full tile-cache traffic per pass.
- Per-submit floor is ~550us with high variance (measured in
  v0.24.6-gpu-mesh-bench).
- DVFS / TLB / firmware scheduling produces fixed-cost stalls we
  can't influence from user-space.

The chug isn't a bug. It's the platform's signal that the data flow is
wrong-shaped for the hardware. The architecture below is "data flow
re-shaped to fit the hardware we have."

### 2.2 The future-physics test

Question that crystallized the direction: "Will this bridge fall this
tick due to weathering and erosion?"

Dense grids don't know what a bridge is. To answer the question on
dense data requires building a connectivity graph from scratch every
tick. **Octrees encode connectivity at every scale by construction.**
Stress propagation, weathering, fluid flow, growth, decay, structural
collapse — all want graph-walk operations on something octree-shaped.

If you build the engine on dense grids and bolt octrees on later for
simulation, you pay for both representations forever. Octree as the
canonical world structure is cleaner.

### 2.3 The PvP test

Two players placing blocks at 10Hz in adjacent chunks. The throughput
isn't the problem — CPU mesher handles it. The problem is edit-to-
pixel latency. Every link between "input event" and "pixel changes"
adds latency. Competitive PvP players notice 16ms.

The destination design minimizes edit-to-pixel latency by:
- Local prediction (no server round-trip for visible feedback)
- Predicted state lives at the closest possible place to the renderer
  (CPU overlay, sampled by shaders that need it)
- Canonical state changes happen out-of-band (confirm-then-promote)

### 2.4 The "merge function and visuals" point

Traditional engines separate simulation (CPU) from rendering (GPU)
with data flowing one direction (CPU → GPU each frame). This creates
duplication: physics computes block stress, renderer separately
computes shadows of the same blocks, mesher separately computes
geometry of the same blocks, lighting separately computes per-face
contribution.

In a GPU-resident world, **one substrate is queried by all consumers.**
The mesher samples the same octree the physics samples the same
octree the renderer samples. Material coloration from climate fields
is the same coloration physics-based weathering reads. The
duplication collapses because the data is in one place.

---

## 3. Components of the destination

### 3.1 Octree-resident world state

**Location:** GPU memory.

**Representation:** Flat-buffer octree. Nodes are array indices,
not pointers. Sparse representation — homogeneous subtrees collapse
to a single internal node. Leaves carry block type (and possibly
density / climate / state flags depending on Camp 1 vs Camp 2; see
§7).

**Layout test-harness-tunable per system.** The exact flat-buffer
encoding (depth-first vs breadth-first node ordering, child pointer
encoding, alignment) is a tuning parameter, not an architectural
constant. Different GPUs have different cache behaviors; the test
harness sweeps configurations.

**Why GPU:** Mesher reads it, renderer reads it, physics reads it,
ray operations read it. Putting it where the consumers are eliminates
CPU↔GPU traffic for derived state. CPU still authors edits and reads
small samples for orchestration; never streams the whole world.

**Open questions for v2:**
- Authority during edit conflict (CPU edit vs simulation edit vs
  network edit) — currently presumed to use overlay confidence
  resolution; needs concrete semantics.
- Save/load format. Octree serialization is a known problem but not
  trivial. Defer to its own design pass.
- Migration from current dense Chunk storage. Current
  `Chunk.explicit_indices` sparse-edit pattern is conceptually similar
  but uses dense underlying storage; migration is gradual not atomic.

### 3.2 CPU overlay for predicted / pending edits

**Location:** CPU memory.

**Representation:** Indexed collection of edit operations. Each entry
carries:
- **Shape:** point, sphere, axis-aligned box, line segment, ... A
  small fixed set extends as items need new shapes.
- **Effect:** block-type-set, block-type-replace, predicate-based
  transformation. Region-level operations are first-class (a sphere
  of stone is one entry, not 120 entries).
- **Confidence:** float in [0, 1]. Drives visual treatment and
  rollback policy.
- **Timestamp / origin:** when did this enter the overlay; from what
  source (player input, AI tick, network message); when does it
  expire absent confirmation.

**Population sources:**
- Player input (predicted from current cursor / motion / click state).
- Input-filter extrapolations (chain predictions; see §3.4).
- Entity AI ticks (pig's predicted next-tick eat-action).
- Speculative physics (predicted next-tick structural collapse).
- Network-pending edits (this client sent to server, awaiting confirm).

**Population sinks (entries leave the overlay):**
- Confirmation: promote to canonical state (send to GPU as a real
  edit), remove from overlay.
- Rollback: server / authority disagrees, or confidence has decayed
  below visible threshold; remove without promotion.
- Expiry: timestamp + TTL exceeded without confirmation; remove and
  optionally retry.

**GPU consultation pattern:**
The overlay is a small buffer the GPU reads via shader access.
Shaders that sample world state query the overlay first
(few-microsecond cost given typical sizes), then octree. The overlay
buffer is updated by CPU on each frame's prep phase.

**Sizing:** Typical overlay during interactive play is 10-100 entries.
A heavy PvP fight might be 200-500. Hard upper bound enforced (say
4096 entries); when exceeded, oldest low-confidence entries get
dropped to make room. Bounded buffer for predictable GPU access.

**Open questions for v2:**
- Hash structure vs flat array vs spatial partition for GPU lookup.
  Flat array with linear scan is fine at 100 entries; degrades at
  1000+. Architecture should not assume one fixed structure.
- Region-edit shape extensibility. Adding a new shape (cone, cylinder)
  requires shader changes. Frequency of shape additions affects
  whether we want a fixed set vs an interpreter.

### 3.3 Confidence model

**Confidence is a first-class data type carried with predicted state.**
Range [0, 1]. Visual treatment is a function of confidence.

**Sources of confidence values:**
- Input-filter analysis (smoothness of recent input → predictability).
- Predicate evaluations (does the predicted edit pass game rules?).
- Network round-trip estimation (lower confidence at higher ping).
- Per-effect-type priors (block placement: 0.99 base; chain
  prediction: 0.7 base; AI extrapolation: 0.85 base).

**Confidence transitions:**
- Decays over time absent confirmation.
- Promotes to 1.0 on confirmation, then entry exits overlay to
  canonical state.
- Drops to 0 on rollback (entry removed).

**Visual treatment:**
- Confidence > 0.95: rendered as confirmed state. Solid.
- Confidence 0.7-0.95: rendered slightly differently (subtle tint,
  slightly reduced opacity, or just-faintly-translucent). Player
  reads this as "this is happening but not 100% confirmed."
- Confidence 0.4-0.7: rendered as ghosted / wireframe / partial.
  Player reads "this is anticipated."
- Confidence < 0.4: not rendered at all. Speculative, kept for
  internal use (chain prediction lookahead) but invisible.

**Open questions for v2:**
- Whether the visual treatment is actually a player-facing feature
  or an internal-only debug aid that gets clamped to "show or don't
  show" in production. Affects UI tone.
- How confidence interacts with multiplayer (different players have
  different views of the same prediction; do they all share visual
  treatment, or is each client's view local?).

### 3.4 Signal-filtered inputs

**Premise:** Player inputs are smooth functions over time. Cursor
rotation, position, velocity, click cadence, modifier state — each
has structure. Sampling and characterizing the structure enables
forward extrapolation.

**Filter outputs per input dimension:**
- Mean over recent window.
- Variance over recent window.
- First derivative (rate of change).
- Second derivative (acceleration).
- A scalar "smoothness" / "predictability" measure (variance of the
  first derivative, or similar).

**Use of filter outputs:**
- Forward extrapolation: given current value + derivatives, predict
  value at +1, +2, +3 frames.
- Confidence weighting: high smoothness → trust extrapolation more
  → predict aggressively. Low smoothness → conservative.
- Chain prediction: sweep cursor over blocks, extrapolate which
  blocks the cursor will reach in the next N frames, predict edits
  to those blocks.

**Conceptual model:** input filter outputs are themselves a signal
that gets re-sampled — varying-coefficient function. The system's
trust in its own prediction is itself a time-varying property.

**Open questions for v2:**
- Window size for the filter (too short = noisy; too long = laggy).
- How smoothness is exposed to the rest of the system (is it just
  a confidence multiplier? Or is it queryable separately?).
- Whether different input dimensions need different window sizes.

### 3.5 Items, abilities, and operations

**Premise:** Items / abilities / spells / weapons are operations
expressed against the world+entity substrate. Each item is a small
program that, when activated, queries state and produces effects.

**Primitive operations:**
- `raycast(origin, direction)` → hit position + entity hit.
- `apply-velocity(entity, vector)` → velocity overlay entry for
  entity.
- `set-blocks(shape, block_type)` → world overlay entry with the
  given shape and effect.
- `teleport(entity, position)` → position overlay entry.
- `query-block-at(position)` → block type (via overlay-then-canonical
  lookup).
- `query-entities-in(shape)` → list of entities (via spatial index).

**Item implementations:**
Items compose these primitives. Simple items are short sequences.
Complex items (area-of-effect, conditional cascades) are longer.

**Execution location:**
Open question — see §6.4. Three candidate locations: CPU code (item
authored as Rust), GPU compute (item authored as GLSL), interpreted
DSL (item authored as data, interpreted by a generic GPU runtime).
Hybrid is likely. Decision depends on item complexity and frequency.

**Open questions for v2:**
- Item authoring experience. Coding each item is reasonable; a DSL
  is more flexible; data-driven items support modding.
- Cost model for items. Simple items should be free; complex items
  shouldn't crash performance.
- Interaction between items and prediction. An item that affects 100
  blocks: do all 100 enter the overlay as predicted? Just confirmed?
  The prediction has to handle bulk effects gracefully.

### 3.6 Entities

**Location:** GPU memory (parallel to world state).

**Representation:** Per-entity state including position, velocity,
rotation, health, type, custom per-type fields. Stored in flat
buffers indexable by entity ID.

**Entity overlay:** Parallel to world overlay. Predicted entity
changes (predicted positions, predicted health changes from
incoming damage) live in the overlay. Confirmed changes promote.

**Entity ticks:** AI / behavior runs each tick. Open question (§6.5)
whether ticks run on CPU (reading GPU state via small queries) or
on GPU (compute shaders).

**Spatial indexing:** Entities need spatial queries ("entities in
sphere," "entities visible from position"). Loosely-coupled spatial
index updated each tick. Could share infrastructure with the
octree's leaf-locator (since entities also live in space).

**Open questions for v2:**
- Entity-world interaction model. A pig eating grass: pig entity's
  AI produces a "set-block-at(pig.target, Air)" overlay entry. Does
  the entity's AI run before or after world simulation each tick?
  Tick ordering is real.
- Entity-entity interaction. Combat, collision, social behavior.
  Conventional approach uses event queues; GPU approach might use
  spatial-query-based broadcasts.
- Entity persistence. Player-owned entities (pets, vehicles) need
  to outlive chunk unloads.

### 3.7 Network model

**Premise:** The architecture's overlay-with-confidence pattern is
the same pattern as networked-game rollback prediction. Single-
player and networked-multiplayer use the same code paths; only the
"authority" differs.

**Authority spectrum:**
- **Single-player local:** Authority is the simulation thread. Each
  tick promotes confirmed overlay entries. No network.
- **Local multiplayer (shared screen):** Authority is one designated
  client. Other clients send predictions through the same overlay
  to the authority, which confirms or rejects.
- **Networked multiplayer:** Authority is a server (could be a
  player's machine or a dedicated server). Clients predict, server
  confirms, clients reconcile.

**Reconciliation:** Server response either confirms (entry promotes
to canonical state on all clients) or rejects (overlay entry on the
predicting client is removed). Other clients see only confirmed state
unless they also have predictions of their own.

**Open questions for v2:**
- Network protocol granularity. Per-edit, per-tick batch, per-entity
  delta?
- Anti-cheat. Client predictions are not trusted; server must
  validate every confirmation against game rules.
- State sync. New client joining mid-game: how does world state
  transfer? Octree serialization plus replay of recent confirmed
  edits is plausible. Defer to specific design pass.

---

## 4. The user's articulations (preserved verbatim where possible)

These are direct quotes or close paraphrases of what the user
expressed during the session that led here. Preserved because the
user's framing is sharper than my synthesis in places.

**On chug findings driving the pivot:**
> "Chasing the chug gave us a lot of surprises because we are used
> to coding on the CPU and it is unpracticed to write for GPU."

**On the destination architecture:**
> "Some of the effects with our graphics stack should be particles
> or terrain coloration based on physics, not just a duplicate
> structure that does no optimisation because older engines did it
> this way... it's about merging the function and the visuals so
> that both can be pushed and work together to make a live world."

**On representation at the CPU/GPU boundary:**
> "I think that representing the chunks as blocks before hitting
> the GPU at all might be one of the mistakes we are making."

> "if we can represent as something that doesn't stress caches by
> definition, like a chunk and instead is some other compressed way
> of representing a chunk that is easy for a CPU to write, easy to
> pipe to memory (so not hotpath) and easy for a greedy or localized
> algorithm on the smaller GPU cores and cache sizes"

**On test-harness-driven layout tuning:**
> "[Memory layout for GPU as flat-buffer rather than pointers] this
> sounds like something our test harness could tune to each system"

**On PvP-style edits and prediction:**
> "in a pvp block building fight when players are near they will
> be placing all the time so do we fake immediate block places
> around them and submit our chunk updates to the rollback system
> for multiplayer or do we come up with another abstraction for
> player interactions?"

**On the symmetry between players and environment entities:**
> "do we want the player to live in a CPU chunk and that is the
> only CPU chunk? probably not because then everything fall apart
> when we add a pig that eats grass (environment-interacting
> entities)"

**On greedy prediction:**
> "we could do greedy client prediction since we pay the cost
> anyways"

**On signal-filtered inputs:**
> "we can track player rotation variance as a vector over time
> which is a function so we can turn that function into a varying
> function and use it to average out samples and try to get most
> of them right"

**On items as operations:**
> "items that rely on if I hit this player make a sphere of blocks
> around them. if my ray lands on this break a block, if my ray
> lands here teleport here, if my ray is in this direction apply
> velocity to my player and therefore my vector"

---

## 5. What survives from the current codebase

Not everything gets rewritten. The existing codebase has substantial
infrastructure that fits the new direction:

**Survives largely intact:**
- `world::cpu_generator::WorldGenerator` and
  `world::gpu_generator::VulkanChunkGenerator`. The reverse-pathway
  capability ("byte-identical chunks from `(coord, seed)`") is
  foundational to the GPU-resident world plus procedural
  reconstitution upload. Already works.
- `world::surface::SurfaceRecord` and `extract_surface` (v0.24.5).
  Wire format for surface representation; survives the octree
  migration because the consumer is the GPU mesher kernel and the
  format is world-coordinate-addressed.
- `render::gpu_meshing_sparse::SparseGpuMesher` (v0.24.6). The
  Vulkan plumbing for dispatching the mesh kernel. Survives;
  internals evolve (batching, zero-copy, reading from overlay).
- Diagnostic infrastructure: GpuTimer, CpuTimer, event side-channel,
  unendlich-analyze. These are platform-agnostic and survive
  unchanged.
- The launcher / spec / harness infrastructure. Survives unchanged.

**Survives with substantial changes:**
- `world::chunk::Chunk` and `ChunkManager`. The dense grid is the
  current canonical CPU representation; in the destination it's
  either replaced by octree storage or relegated to an edit-log
  with octree-cached derived state. The `version`, `explicit_indices`,
  `dirty` fields generalize to overlay-entry semantics; the basic
  ideas survive but the implementation changes.
- `render::mesher` (CPU mesher). Lives on as fallback path and as
  reference for topology equivalence. Eventually retired once GPU
  path covers all LOD tiers.

**Probably retired:**
- The standalone `world::gpu_chunk_pool_vk` and `world::gpu_mesher_vk`.
  The "dense block grid uploaded per chunk" pattern is the architectural
  mistake the user identified. The new sparse path (surface records
  + sparse GPU mesher) supersedes it.
- The chug-arc instrumentation events (shadow_stride, cull_count,
  upload_pressure, gpu_spike, wait_fences_spike). Keep them in
  unendlich-analyze; emit them only when the relevant diagnostic
  features are enabled. They're not in the hot path of the new
  architecture, but they're useful retroactively.

---

## 6. Open architectural questions for v2

These are decisions that haven't been made yet. v2 of this doc is
the place to make them or to explicitly defer them with rationale.

### 6.1 Camp 1 (blocky) vs Camp 2 (smooth/SDF) octree leaves

Blocky aesthetic = simpler kernels, discrete physics, current visual
style preserved.

Smooth (SDF / density / surface-nets) = curved erosion-friendly
surfaces, continuous physics, different visual style.

Affects octree leaf payload (single block ID vs (block ID, density)),
mesher algorithm (axis-aligned quads vs marching cubes / surface
nets), and what physics looks like (block-discrete vs continuous).

### 6.2 Overlay GPU-lookup structure

Linear scan over flat buffer (simple, fine to ~100 entries).
Hash table on GPU (better at scale, more code).
Spatial partition (best at scale, most code).

Probably linear-scan flat array initially; revisit if entries
regularly exceed a few hundred.

### 6.3 Confidence visual treatment in production

Whether predicted-but-unconfirmed state is visibly distinct from
canonical state to the player. Different game aesthetics make
different choices.

### 6.4 Item implementation location

CPU code, GPU compute shader, or interpreted GPU DSL. Hybrid is
likely. The split depends on item frequency and complexity. Modding
support biases toward DSL. Performance biases toward shaders.
Authoring ergonomics biases toward CPU code.

### 6.5 Entity AI tick location

CPU (current pattern; pigs ticked by CPU code reading some state)
or GPU compute (compute shaders update entity state, with CPU
orchestrating).

GPU is appealing for large mob counts. CPU is simpler. Hybrid (CPU
orchestrates, GPU computes batches) is likely.

### 6.6 Edit application: hot path

When the CPU confirms an overlay entry and wants to push it to
canonical state, the GPU has to integrate the edit into the octree.
What's the right primitive?
- CPU sends a structured "edit command" → GPU shader updates the
  octree node + invalidates affected mesh regions.
- CPU pre-computes which octree nodes need to be touched and sends
  them as targeted updates.
- CPU sends the edit and the GPU walks the octree on its own.

This is the hottest path in the architecture and needs careful design.

### 6.7 Octree memory layout

Flat-buffer encoding: depth-first vs breadth-first, child-pointer
encoding, node alignment, branching factor. User flagged this as
"test harness can tune per system." Architecture is layout-agnostic
but the chosen default affects performance significantly.

### 6.8 Save/load format

Octree on disk is non-trivial. Procedural reconstitution + edit log
is one option (save only seed + edits; regenerate at load time).
Hybrid (cache materialized octree of frequently-loaded regions;
regenerate the rest on demand) is another.

### 6.9 Multiplayer state sync

New client joining mid-game: how does world state transfer?
Procedural reconstitution + edit log replay is plausible. Defer
to specific design pass.

### 6.10 What's the minimum viable subset to ship as one increment?

This is the staging-plan question. Not in scope for v1 of this doc;
in scope for v2 or the implementation plan that follows.

---

## 7. What this doc explicitly does NOT say

- Specific data structures for the octree (node layout, branching
  factor, leaf encoding). Defer.
- Specific GLSL shader for octree traversal. Defer.
- Specific overlay buffer binding layout. Defer.
- Specific confidence value formulas. Defer.
- Specific input filter window sizes / cutoff frequencies. Defer.
- Specific network protocol. Defer.
- A timeline. The implementation is multi-session work. Pace TBD.

---

## 8. Process notes (meta — for the architect)

This doc is v1. It captures conversation through end of v0.24.6.
The user is preparing v2 with additional postulations.

When v2 lands, it should either revise this doc in place or supersede
it. Doc-style choice TBD. The important property is that we have ONE
canonical architecture doc, not three drafts with overlapping content.

The session pattern that produced this doc:
1. Chug arc exhaustion → architectural pivot needed.
2. Multiple rounds of "user articulates broad direction" + "Claude
   surfaces tradeoffs and crack-finds."
3. Ideas accumulated faster than they got pinned down. The risk was
   shipping code based on incomplete architecture.
4. User explicitly invoked "let's slow down and document" before
   more code.

This is healthy. The chug arc's mistake was shipping 7 increments
before recognizing the platform-level constraint. The recovery is to
explicitly write things down before committing more code.

Reasonable next steps after v2 of this doc lands:
- Implementation plan (which milestone, what falsification criteria,
  what reversibility).
- One concrete "minimum viable" milestone definition.
- A "this is the next code ship" decision based on the plan.

Until v2 lands, no new code ships in this direction. Either we ship
unrelated work (HUD, working-distance buffer, plant variation) or
we pause.

---

## Appendix A: glossary

- **Canonical state**: Authoritative world state. Lives on GPU as
  octree. Edits are confirmed before reaching canonical state.
- **Confidence**: float [0,1] attached to overlay entries; drives
  visual treatment and rollback behavior.
- **Confirmation**: Process by which a predicted overlay entry is
  promoted to canonical state.
- **Greedy prediction**: Predicting more aggressively than will be
  committed, with the expectation that confidence/rollback handles
  the over-prediction.
- **Overlay**: CPU-side collection of pending / predicted edits.
  Sampled by GPU shaders during render and mesh.
- **Reverse-pathway**: The deterministic property of `(coord, seed)
  → chunk blocks` that enables sending tiny edit messages instead of
  full chunk data, because the receiver can regenerate the base.
- **Rollback**: Removing an overlay entry without promoting to
  canonical state. Triggered by server disagreement, confidence
  decay, or expiry.
- **Signal-filtered inputs**: Player input treated as continuous
  smooth functions over time, characterized statistically, used to
  extrapolate forward.

## Appendix B: related design notes increments

- Increment 49 (v0.24.5): Sparse surface representation. The
  `SurfaceRecord` format.
- Increment 50 (v0.24.6): GPU sparse mesher + comparison bench.
  Layer A.2 validation that the GPU mesher path works on AGX.
- (Pending) Increment 51: Layer A.3 production wiring of GPU mesher.
  Specifically gated on this architecture doc's resolution: does
  Layer A.3 land before or after overlay design? Open in v2.
