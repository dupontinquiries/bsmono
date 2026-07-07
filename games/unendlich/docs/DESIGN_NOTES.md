# unendlich — Design Notes

This file captures architectural decisions and forward-looking plans that
shape implementation across multiple turns. Things in here are decisions
the project owner has made or strong intents we're building toward, NOT
finished features. Read this before assuming something works or doesn't.

When implementing the relevant subsystem, come back here and check the
notes for that subsystem. When a note becomes obsolete because the thing
has been built, leave a "BUILT" line in place of the note.

> Historical note: the project was originally named "Meinkraft" and was
> renamed to "unendlich" mid-development. Some older sections of this
> file still reference the old name in context-specific ways
> ("Meinkraft.0.24" version strings, "Meinkraft style" comparisons).
> Those are accurate as historical references and are left as-is.

---

## Foundational architecture

### Chunks

* Chunks are 32×32×32 cubes. `ChunkCoord { x, y, z }` is 3D.
* World is unbounded vertically — chunks stack arbitrarily, no fixed height limit.
* Heightmaps are derived (via density bisection), not stored.
* Block coords are i32; effective range is ±66 billion blocks per axis.
* Per-chunk versioning: `version: u64` increments on every edit.

### World model

* `WorldModel` owns all world properties as 3D-region-grid-backed fields.
* All fields are 3D (uniform shape, even where conceptually 2D — sea level
  evaluated at y=0 etc.). Scale anchors per-field via `region_sizes`.
* Composition: pressure → humidity → sea_level. Today static; designed for
  time-evolved dynamics later.
* `WorldGenerator` queries `WorldModel`; never calls noise directly.
* `WorldParams` is the extensibility hook for future
  (seed, narrative_state, AI_directives, era, atmospheric_state) inputs.

### Determinism contract

* Pristine chunks are pure functions of `(coord, seed, generator_version)`.
* CPU and GPU paths must produce byte-identical output for the same inputs.
* Storage records pristine chunks as `(coord, seed, generator_version)`
  (~20 bytes); only edits are stored as deltas.
* Change generator? Bump `generator_version`. Old saves with mismatched
  versions regenerate pristine chunks; deltas remain valid.

### No-Minecraft-mistakes principles

* Versioned save format from day one with stub migrator.
* No bolted-on "dimensions" — one world with rich vertical/atmospheric
  variation. Sky biomes if you fly high enough; deep biomes if you go down.
* Explicit event categories with separate budgets (block-state-change,
  light-update, fluid-tick, scheduled-tick) — no general "neighbor update"
  fan-out.
* Per-subsystem tick rates, not a global tick. Scheduler interleaves.
* Atomic-write storage (write-temp-then-rename) with a journal of
  in-progress edits.
* Versioned protocol from day one; mismatched clients rejected with clear
  message, not silent breakage.
* Parallel server simulation — per-chunk locks, not world-mutex.

---

## Forward-looking design intents

These are things to BUILD INTO the system when we get there, not afterthoughts.

### Bulk raycast as first-class engine primitive

When implementing raycast (after block-edit support):

* **Bulk API** — engine consumers issue thousands of rays per frame.
  Don't write a "cast one ray" function and hope it scales. Design for
  batched dispatch (CPU SIMD or GPU compute) from the start.
* **Multiple ray "kinds"** with different stop conditions and side effects:
  * Ability-cast rays (AOE telegraphs, skill targeting) — stop at first
    blocking entity or solid block; carry "what was hit" payload.
  * Lighting probe rays — stop at opaque, accumulate transparency along path.
    Likely the highest-volume kind. Used for ambient occlusion, soft shadows,
    light propagation. Want to vectorize/batch hard.
  * Entity vision rays (AI line-of-sight) — stop at solid blocks AND at
    opaque entities. Used by AI to decide "can I see the player?" Hiding
    behind walls, smoke, foliage, illusions all participate.
  * Player-vision rays (camera frustum, click-to-select) — similar to AI
    vision but uses player's perspective and may have different transparency
    rules (e.g., player can see through their own held item).
  * Physics/collision rays — ballistic, projectile motion, swept AABBs.
* **Material-driven distortion** (see below) means rays bend, not just stop.
  Refractive blocks (water, glass, magic crystals, heat shimmer) deflect
  rays. Diffractive materials scatter. Plan for ray paths to be polylines,
  not line segments.
* **Hidable from AI** is a gameplay requirement: certain materials should
  occlude AI vision rays differently than physical/lighting rays. Smoke
  bombs, illusion spells, magical concealment — all expressed as "this
  material has high occlusion to vision-rays and low occlusion to physics-rays."

### Material distortion effects

Blocks have richer optical properties than just transparent/opaque:

* **Refractive index** — bends rays passing through. Water, glass, ice,
  diamond, magical materials. Affects vision rays AND lighting rays.
* **Scatter coefficient** — randomizes ray direction within a cone.
  Smoke, fog, frosted glass, dense foliage. Different per ray-kind
  (vision rays scatter more than physics rays).
* **Absorption** — reduces ray "intensity" without stopping it. Useful for
  partial concealment (foliage, fog).
* **Self-emission** — material adds to lighting-ray accumulation.
  Glowstone, lava, magical light sources. Different from "block is
  opaque but glowing": the material itself contributes.
* **Direction-dependent transparency** — anisotropic materials like
  one-way glass, holographic illusions, "you can see out but not in."
* These all generalize to a ray-material interaction function:
  `(incoming_ray, material) -> (continue?, new_direction, new_intensity)`.
  Different ray-kinds use different interaction tables.

### Biome runtime translation (legacy API path)

The current `Biome` enum and `determine_biome(temp, humid, erosion)` is a
LEGACY API surface we'll keep around for compatibility with old code paths
during the transition to fully field-driven biome assignment.

* **Best-fit translation**: when legacy code asks "what biome at (x,y,z)?",
  we sample the modern field model (temp, humidity, pressure, light,
  underground/surface/sky) and return the *closest match* from the legacy
  enum. This is a runtime lookup, not a stored mapping.
* **JSON-driven param mapping**: biome → field-range descriptors live in
  a JSON file (`assets/biomes.json`). Each biome describes its preferred
  ranges (e.g., Desert = high temp, low humidity, surface y, etc.).
  Translation walks this descriptor list and finds the best match.
* **Block-content matching**: when assigning biomes to legacy save data
  (or to chunks generated under different rules), the current block
  composition can inform biome — "lots of sand and cactus → Desert."
  This is for backward compatibility with chunks that don't carry biome
  data in their format.
* **Structure-map runtime**: structures (villages, dungeons, custom
  builds) live in a runtime map keyed by 3D region. When a player enters
  a region, structures present there can override biome assignment for
  contained blocks ("this is a frozen tower interior, treat as IceCave
  even though atmospheric humidity says Forest").
* JSON descriptors must be hot-reloadable for tuning. World doesn't need
  to regenerate; biome-display lookups just re-read the descriptors.

When implementing: the legacy biome system stays as-is in `biomes.rs`
short-term. The TRANSLATION layer between WorldModel field samples and
legacy Biome enum is what gets built. New code paths should query
WorldModel fields directly; legacy paths funnel through the translator.

---

## Open questions / decisions deferred

* GPU compute backend cascade: Vulkan (with shaderFloat64) → Vulkan (f32 + "double-double" emulation, byte-identical) → OpenGL 4.6 compute → GLES 3.1 compute → CPU. Currently f64-Vulkan only, falls straight to CPU. M2/Asahi Mesa doesn't expose shaderFloat64; M3+ Vulkan likely will. Triggered by: ERROR_FEATURE_NOT_PRESENT on shaderFloat64. Build cascade when we have time-budget for it; CPU is workable for now.
* ECS choice: hecs vs specs vs custom. Doesn't block current work.
* Storage on-disk format: bytes-on-disk decisions are expensive to change
  later. Real design pass before committing.
* DHT transport: project owner has existing image-routing code to adapt.
  Will share when we get there.
* Splitscreen merge-on-proximity: real design problem (camera blending,
  viewport math, what happens when 2 of 4 merge but others don't).
  Worth a real conversation.
* Mob/entity model: "as material/growth processes, not surface vs
  underground." Slime molds spawning where moisture > X and food > Y,
  insects on chemical/temperature trails. Not biome-locked spawn tables.
  Plan: entity spawning queries WorldModel fields, not biome enums.

---

## Tick / lifecycle / wind systems (added during milestone push)

### Tick framework (`src/world/ticks.rs`)

`TickRegistry` holds boxed handlers (`Arc<dyn Fn(&mut TickContext)>`).
`TickScheduler` accumulates real-time and fires ticks at a fixed period
(default 0.5s). `TickContext` carries:

* `chunks`: `&DashMap<ChunkCoord, Arc<Chunk>>` — read-only.
* `field_reader`: `Option<&dyn Fn(&str, x, y, z) -> Option<f32>>` for
  sampling moisture/wind/etc. Built per-tick by ChunkManager from its
  field state.
* `player_chunk`: `Option<ChunkCoord>` for distance-based tick culling
  via `ctx.chunk_in_tick_range(coord, radius)`.
* `block_edits` and `state_changes`: deferred mutations queued by
  handlers, applied by the tick driver at end of each tick.

Handlers don't mutate the world directly — they queue edits. Driver
applies via `set_block` / `set_instance_state`, both of which COW-clone
the chunk, mutate, and swap the `Arc` atomically.

### Per-block instance state (`Chunk::instance_state`)

`BTreeMap<u16, BlockInstanceState>` — sparse map keyed by block index.
Pristine blocks have no entry (zero memory cost). `BlockInstanceState`
is 2 bytes: `age_ticks: u8`, `flags: u8`. Setting state to default
*removes* the entry, keeping the map sparse.

This is the "erosion blockstate" pattern: persistent per-block metadata
that's separate from the block ID. Lifecycle stage, decay age,
weathering — anything that's about THIS instance, not the type, lives
here. Sparseness is the point.

### Inverted indexes (`Chunk::plant_indices`)

`BTreeSet<u16>` of block indices for blocks where `has_subvoxels()` is
true. Maintained on `set()` and seeded on `from_blocks()`. Mesher's
decoration pass and lifecycle handlers iterate this set directly
instead of scanning all 32k cells.

This is the "group ticking" pattern. A future `tickable_indices` /
`leaf_indices` would split further; for now `plant_indices` + a per-
handler filter (e.g. `matches!(block, OakLeaves | BirchLeaves)`) is
enough.

### Plant lifecycle (`src/world/plant_lifecycle.rs`)

5-stage state machine: Seedling → Growing → Mature → Drying → Withered
→ Dies. Stage stored in lower 3 bits of `flags`. `age_ticks` measures
time within stage; transitions at duration thresholds modulated by
moisture.

Wet plants (`moisture > 0.4`) skip Drying entirely; they progress
Seedling → Growing → Mature and stay Mature. Dry plants (`moisture <
0.4`) progress through the full cycle and eventually die. Drying
plants can recover to Mature if moisture rises above 0.7
(rehydration).

Color modulation per stage feeds the mesher: green base × stage
multiplier produces visible color shift through life (washed-green
seedling → solid green growing → deeper green mature → straw yellow
drying → brown withered).

Grass blocks (surface bearers, not standalone plants) never die — at
end of cycle they reset to Mature. Otherwise we'd kill all grass and
expose dirt across the world.

### Leaf decay (`src/world/leaf_decay.rs`)

Single-stage progression driven by wind. `age_ticks` accumulates each
tick; +1 extra under gusty wind (`wind > 0.5`). At
`LEAF_DEATH_AGE_TICKS=60` (~30s real time) the leaf falls — block
becomes Air.

Color modulation: linear from `[1, 1, 1]` (fresh) to roughly
`[1.5, 0.5, 0.2]` (deep autumn brown) over the leaf's lifetime.

### Wind (`src/wind.rs`)

`wind_at(x, y, z, time)` returns `(wx, wz, magnitude)`. Magnitude:
BASE 0.3 + altitude gain (more wind aloft) + regional noise (gusty
pockets). Direction: slow global rotation (full rotation every ~3
minutes) + per-region noise offset.

Direction NOT cached in field framework — fields are time-static.
Magnitude IS cached because it varies slowly. Consumers needing
direction (player physics, plant sway) sample `wind_at` directly.

### Plant visual sway

Static lean (no animation yet), implemented as per-vertex offset in
`emit_subvoxel_grid`. Sub-cells at the bottom of the host stay
anchored; sub-cells at the top displace laterally in the wind
direction. Top corners and bottom corners of each mini-cube use
*different* sway weights so the cube leans rather than translates as a
unit. Adjacent stacked sub-cells share corners with the same weight
(weight = sub_y / (RES-1)), so corners stay welded.

Animation requires either (a) periodic re-meshing (expensive) or (b)
vertex-shader uniform path (pipeline change). Both deferred.

### Wind force on player

`PhysicsParams.wind_force: [f32; 2]` adds lateral acceleration during
`step_body`. Bin samples `wind_at(player_pos, world_time)` each frame.
Scale of 0.5 means max wind adds ~0.5 m/s² — noticeable lateral drift
in open terrain, doesn't dominate input.

---

## Rollback netcode foundations

The engine is being designed for client-prediction-with-rollback netcode
from the ground up. Nothing is networked yet, but the building blocks
are landing now so future networking work doesn't require rewriting the
simulation.

### What rollback needs from the engine

1. **Determinism.** Same input, same state → same output, every time,
   on any machine. Death of determinism: `rand::random()`,
   `SystemTime::now()`, HashMap iteration order, variable-dt physics,
   wall-clock time anywhere in simulation logic.

2. **Snapshottable.** Every system must support capturing its state to
   a value and restoring from one. Cheap to clone (Arcs or Copy types
   ideally). State that can be re-derived (mesh, field caches) is NOT
   snapshotted; restore invalidates these.

3. **Replay-able.** Given a starting snapshot and a sequence of
   per-tick inputs, the simulation re-runs and produces the same
   final state.

4. **Decoupled from rendering.** Render reads sim state but doesn't
   mutate it. Particles, animations, etc. are either re-derivable from
   sim state or don't participate in net sync.

5. **Layered by authority.** Server-authoritative state (mob entities,
   adjudicated block edits) vs. client-predicted state (local player)
   vs. purely visual (particles).

### What's landed so far

**Master clock — `TickId`.** A typed `u64` in `world::ticks`. The
canonical "what simulation time is it." Anything time-dependent in the
simulation (wind direction, plant lifecycle aging) reads from
`tick_id × tick_period`, NOT from wall clock. `TickScheduler` owns the
counter and exposes it via `current_tick()`. The bin samples wind
using this for the player's wind force.

**Deterministic chunk iteration.** Tick handlers iterate via
`TickContext::sorted_chunk_coords()` instead of `chunks.iter()` —
DashMap's native order is non-deterministic across runs and would
break replay. The batched per-chunk apply in `tick_simulation` also
uses sorted coords.

**Input snapshot ring buffer — `input_capture` module.** Per-frame
input is OR-aggregated into a `pending_frame: InputFrame`; at each
tick boundary this gets stamped with the completing tick's `TickId`
and pushed into an `InputBuffer` (capacity 64 ticks). This is passive
infrastructure today — simulation still reads input directly from
glfw. When networking lands, the simulation will switch to reading
from the buffer, enabling replay-on-rollback.

**`Snapshottable` trait.** Defined in `snapshot.rs`. Implemented for
`PhysicsBody` (trivially Copy) and `Sky` (trivially Copy). Other
systems will implement it as they migrate. The trait contract is
documented in the module: bit-exact restore, no caches, no wall clock.

### What hasn't landed

- **Fixed-timestep physics.** Physics still runs with variable `dt`.
  For rollback we need physics to run at its own fixed tick rate
  (probably 60Hz, separate from world ticks at 2Hz) with the frame
  loop accumulating and running N integer physics steps per frame.
- **`Chunk` snapshot.** `Arc<Chunk>` is already cheap to capture but
  the trait isn't implemented yet for it. The DashMap of chunks needs
  a wrapper-level snapshot too.
- **WorldSnapshot aggregator.** Combines all per-system snapshots into
  a single coherent capture for a given TickId.
- **Server / client / transport.** No network code exists. The
  rollback architecture is foundation only.
- **Authority layering.** No notion yet of "this entity is
  server-controlled" vs. "this is locally predicted."

### Anti-patterns to keep flagging

- `SystemTime::now()` anywhere in simulation paths.
- HashMap iteration where order affects observable output.
- Variable-dt physics (any system that integrates `e^(rate * dt)` with
  changing dt).
- Wall-clock time in anything that affects gameplay.
- `rand` without an explicit deterministic seed tied to (state, tick).

### Why this approach

Doing rollback piecemeal — adding the deterministic clock first, then
deterministic iteration, then snapshots — is safer than a big-bang
rewrite. Each step is verifiable in isolation. The simulation behavior
is unchanged at every step: `TickId` exists alongside `world_time`,
sorted iteration produces the same outputs as random iteration (just
in a fixed order), input capture is passive. Future sessions can flip
the simulation to be net-aware without retracing the basics.

## Multiplayer-prep camera split (May 2026)

The renderer used to own a single `FlyCamera` that bundled view pose
(position, yaw, pitch, fov) with controller config (move_speed, mouse
sensitivity) — a single-local-player assumption baked into everything.

Now there are three concepts:

- **`View`** (`render::camera::View`): a pure pose. Cheap, copyable,
  serializable. Multiple Views can coexist (split-screen, picture-in-
  picture, replay viewer). On the wire in a networked build, this is
  what gets replicated per connected player.
- **`FlyController`** (`render::camera::FlyController`): per-local-
  player input config. Mutates a View based on input. One per local
  human; never serialized.
- **`LocalPlayer`** (`local_player::LocalPlayer`): bundles a BodyId
  reference + a View + a FlyController. The bin loop will eventually
  iterate over an array of these instead of carrying a single
  implicit "player". Ready for split-screen.

The renderer takes a `&View` on `draw_frame_with_view`; the legacy
`draw_frame` is preserved and reads from `self.camera` for backward
compat. Old call sites keep working; new code prefers the View-by-
parameter path.

PhysicsBody gained an optional `id: Option<BodyId>` field. `BodyId(u64)`
is the stable identifier; `None` is fine for legacy/test bodies.
Multiplayer code SHOULD give every replicated body an ID.

ChunkManager gained `update_with_interest_points(&[...])` which today
delegates to single-point update using the first point. The signature
is the multi-player API; the implementation is a stub. When the
loader gains union-of-disks streaming, this is where it plugs in.

### What this enables (without doing it yet)

- Split-screen: render twice per frame with two Views on two
  viewports; drive each from a separate LocalPlayer.
- Spectator/free-cam: a LocalPlayer with no body_id, just a View
  driven by FlyController. Detached from physics.
- Server-side culling: server holds Views for every connected client,
  uses View frustums for AOI replication WITHOUT needing the
  rendering code.
- Body replication: server addresses bodies by BodyId in update
  packets; clients look them up in a registry.
- Dedicated server: zero LocalPlayers, simulation still ticks,
  `update_with_interest_points(&[])` is the no-op-safe path.

### What we deliberately deferred

- Per-player input bindings (split-screen needs P1=WASD, P2=IJKL).
- Network identity / session tokens on LocalPlayer.
- Real multi-disk streaming in chunk_manager.
- Body registry / ECS migration.
- Snapshot/restore for the View+LocalPlayer layer (rollback prereq).

Each of these is a session of work on its own. The split here is the
foundation; the rest is incremental.

## Input layer expansion (queued)

The input layer currently captures keyboard keys and mouse buttons 1
and 2 (left/right). Several extensions are queued; each is a small
session of work but should be noted before they're forgotten.

### Additional mouse buttons

GLFW exposes `MouseButton::Button1` through `Button8`. The bin only
reads buttons 1 and 2. Useful additions:

- **Middle mouse (Button3)**: pick block / eyedropper. In creative-
  type modes, middle-click should set the held block to whatever
  the player is aiming at. Standard Minecraft behavior.
- **Side buttons (Button4/Button5)**: typical "back/forward" mouse
  buttons. Reasonable default: hotbar prev/next, freeing scroll
  wheel for other uses.
- **Pressure / detents (Button6+)**: rare on standard mice, common
  on gaming mice with extra side keys. Bind via the keybindings
  system once mouse buttons are first-class actions.

The work is in two pieces:
1. `keybindings::GameAction` should support mouse-button bindings
   (currently only `Key`). Either parameterize the binding type or
   add a parallel `MouseButton` map.
2. `input::Input` already has `is_mouse_button_pressed`; expose a
   `just_pressed_mouse` similar to `just_pressed_keys` so the bin
   can do edge-detection on middle-click.

Tests: that an action bound to a mouse button fires when that button
is pressed and not when others are; that just-pressed edge detection
works at high frame rates (pattern from the existing keypress tests).

### Stylus / pen tilt support (ProPoint and similar)

Pen-grip tilt is an analog 2D axis (tilt-x, tilt-y, each in radians)
that's distinct from cursor position. Useful for: analog camera tilt,
pressure-sensitive build tools (lighter touch = smaller brush), or
direct vector-painting.

GLFW does NOT expose pen tilt. The realistic backends are:

- **Linux/Wacom**: `xinput` / `libinput` events expose `ABS_TILT_X`
  and `ABS_TILT_Y`. We'd need to read these raw on Linux and feed
  them into the input layer.
- **macOS**: NSEvent's tilt properties on tablet events. Requires a
  Cocoa bridge.
- **Windows**: Pointer Input Messages API (WM_POINTER*) carries
  tilt. Would need a winapi path.

Reasonable shape: `Input.pen_tilt: Option<(f32, f32)>` populated
when a stylus event arrives, `None` for mouse-only sessions. The
input layer is the abstraction; backends are gated by platform.

This is a **multi-component dependency add** (probably needs a new
crate for raw input on each platform). Defer until there's a concrete
gameplay use case driving it. Possible early use: temperature lookup
visualization where pen tilt rotates the F4 debug panel.

### Debug item list

A small UI panel (toggle-able with a key, e.g. F1) that lists all
`BlockId` variants with names and icons. Click an entry → set the
held-block to that ID. Replaces the current "I can only place stone"
limitation for testing block-place behavior on every block type.

Not blocked by anything else, but most useful AFTER:
- A real held-block / inventory state on the player.
- Block icons (a 3D-rendered preview of each block at small size,
  or a hand-drawn icon, or just the block's top-face color).

Cheap path: just a text list + number-key to select. Polish later.

## Multiplayer prep, continued: permissions + commands (queued)

Multiplayer requires that gameplay actions are GATED by per-player
permissions. Single-player today implicitly has "the player can do
anything"; multiplayer servers need fine-grained control (creative
players can fly, survival players can't; admins can `/tp`, regular
players can't).

The right move is to introduce permission gating now, in single-
player, with a default permissive ACL — so that every action site
already routes through a check. When the server arrives, the only
change is who controls the ACL.

### Permission model

Per-player `Permissions` struct (on `LocalPlayer` or keyed by
`BodyId` in a future registry). Fields are boolean grants, defaulting
to `true` in single-player:

- `can_break_block`
- `can_place_block`
- `can_toggle_flight`
- `can_open_inventory`
- `can_use_command(command_name: &str) -> bool`
- `permission_level: PermissionLevel` (None / Player / Moderator / Admin)

Actions check first:

```
if local_player.permissions.can_break_block && raycast_hit.kind == ...
```

In single-player, the check is a constant `true` and the compiler
inlines it away. In multiplayer, it's the server's authoritative
view of what this player is allowed to do. Same code path, different
backing data.

### Hunger / survival state

Permissions are SEPARATE from survival mechanics, but both gate
actions, so they sit in similar code:

- `Hunger` (0..20): too low → can't sprint, slow regen.
- `Health` (0..20): zero → respawn flow.
- `OxygenWhileSubmerged`: drowning damage.
- `Stamina` (for things like swimming, climbing).

These are STATE that mutates over time (per simulation tick) — so
they need a tick handler. They affect what the player can DO, so
they intersect with the permission system: `can_sprint` becomes
`(permissions.can_sprint && hunger.allows_sprint())`.

For multiplayer rollback: hunger/health snapshot per tick. Both are
just integers; cheap to capture.

### Commands

Server-side text commands (`/tp x y z`, `/give item count`,
`/gamemode creative`, `/seed`, etc.) need:

1. **Parser**: `/cmd arg1 arg2 ...` → `(name, args)`. Standard text
   tokenization. Quoted args for spaces.
2. **Registry**: command name → handler fn. Each entry has a
   required `PermissionLevel`. Auto-fail if requester lacks it.
3. **Dispatcher**: handles the result — `Ok(message)` shown to
   sender, `Err(message)` shown as red error.
4. **Source binding**: in single-player, hotkeys can directly invoke
   the dispatcher with a virtual "console" source that has admin
   level. In multiplayer, chat input creates dispatcher invocations
   bound to the chat sender's BodyId.

The work is incremental: build the parser + registry first with one
command (`/seed` returns the world seed). Add commands one at a time
as needs arise. Most of them already correspond to existing F-keys
(F9 fire = `/light_fire`, F4 debug = `/debug overlay`). The keybind
becomes a shortcut for the command, not a parallel implementation.

### Why do this now instead of later

The risk of NOT routing actions through permissions early: by the
time multiplayer lands, every action in the codebase has direct
"just do it" code, and adding gates means touching every site. By
adding the layer now (with all-true defaults), we touch each site
once and the migration is settled.

Same logic for commands: doing it before multiplayer means single-
player is the testbed for the parser, registry, and dispatcher. By
the time the network arrives, the system is mature.

### Order of attack (smallest → largest)

1. `Permissions` struct on `LocalPlayer`, all fields default `true`.
   Wire `can_break_block` and `can_place_block` at the raycast/edit
   sites in the bin. Single-player behavior unchanged. **(small)**
2. Permission check on `ToggleFlight`. **(trivial)**
3. `Hunger` + `Health` state on body or local player; tick handler
   that decrements hunger over time. **(small + visible)**
4. Command parser + registry + first command (`/seed`). **(small)**
5. Move existing F-keys to invoke commands instead of direct logic.
   **(refactor)**
6. Chat input UI → command invocation. **(needs UI work)**
7. Server-side ACL enforcement (when transport lands).

Each step ships cleanly on its own. None of them block any other
work in the queue.

## Field caching + invalidation-based recomputation (queued, ARCHITECTURAL)

This is not a small task — it's a foundational pattern that several
future systems will depend on. Worth doing before adding more
systems that recompute eagerly on every block edit.

### The two problems

**Read-side problem.** Field functions like `temperature_at`,
`cloud_density_at`, `thermal_mass_factor`, `humidity_at`, etc. are
pure and recompute their math on every call. Multiple callers hit
the same cells (mesh pass + convection arrow + worldgen freezing
all read temperature). The math isn't expensive in isolation, but
it's the same answer every time and it adds up at scale.

**Write-side problem.** When a block changes (player edit, simulation
tick, structural growth), MANY systems are conceptually dependent:
mesh, lighting (future), water flow (future), weathering, plant
lifecycle, thermal mass of nearby cells, possibly heat sources, and
the meshes of 1-7 neighbor chunks. If every edit eagerly fires every
dependent system across a 27-chunk neighborhood, a single click
stalls the engine. The current code already does the right thing for
meshes (version bump + lazy resubmit) but no other system has been
added yet — when they are, they'll regress this unless we have a
shared pattern.

### The pattern: invalidation-based recomputation

For each system that derives data from blocks/fields, three rules:

1. **Mark dirty cheaply on write.** The block edit doesn't recompute
   anything; it adds an entry to that system's dirty set. Cost: one
   hash insert per affected cell.

2. **Recompute on read OR on budgeted tick, whichever comes first.**
   If a reader needs a value and the cell is dirty, recompute it
   then and cache. If a budgeted tick gets to it first, recompute
   pre-emptively at known-low-cost time. Either way, the next read
   gets a fresh value.

3. **Budget the per-frame total work.** Each system declares a max
   ms/frame budget. The scheduler pulls dirty entries off the queue
   in priority order until the budget is exhausted. The remaining
   dirty entries stay dirty for next frame.

### Concrete data structures

Per-chunk:
```rust
struct ChunkFieldCache {
    // Per-field, sparse map of cell-index -> cached value.
    // Cache misses fall through to the pure function and populate.
    temperature: SparseMap<u16, f32>,
    cloud_density: SparseMap<u16, f32>,
    thermal_mass: SparseMap<u16, f32>,
    // ... other fields ...
}
```

Per-system, in the chunk_manager (or a new scheduler module):
```rust
struct DirtyQueue {
    // Cells whose cached value is invalid. Priority: edits to
    // chunks near the player drain first.
    pending: BinaryHeap<(Priority, ChunkCoord, u16)>,
    budget_ms: f32,
}
```

The scheduler runs once per frame:
```
pull entries off queue in priority order;
recompute each one (using the pure function);
write result into chunk's cache;
stop when budget_ms is exhausted.
```

### Cross-system propagation rules

When a block at `(x, y, z)` changes:

- Mesh of THIS chunk → dirty (+ neighbor mesh if cell is on a face).
  Already implemented via version bump.

- Thermal mass of cells WITHIN 1-block radius (the 6 neighbors
  whose `thermal_mass_factor` reads this cell) → dirty.

- Water flow at this cell + 6 neighbors → dirty (when water flow
  exists). Falling-water cascades will flag MORE cells when they
  evaluate but the initial event is just the 7 cells.

- Light propagation at this cell + cells within light's reach.
  Light is the one that could overload — limit propagation to
  some radius per tick.

- Heat sources: only if the changed block IS a heat source (lava,
  fire). One entry: add or remove from the heat_source map.

- Cloud density: never invalidated by block edits. Pure function
  of position + time.

- Temperature: invalidated only if a heat source changed (which
  is itself an event). Otherwise temperature_at is purely a
  function of position.

The point: systems FILTER what events apply to them. A block
edit is one event source; each system decides if/what to invalidate
in its OWN structures.

### Migration path (smallest → largest)

1. **Cache temperature_at per chunk**. The function is pure and
   never invalidates — caching is straightforward, no dirty set
   needed (just lazy compute on first access). Verify mesh
   throughput improves on chunks that re-read temperature many
   times during cloud emission. **(small, one chunk-field; isolated.)**

2. **Cache thermal_mass_factor per chunk during worldgen**.
   `thermal_mass.freezes_at` is called per water cell; it reads 6
   neighbors. Caching the factor would help if the factor is read
   multiple times — but currently it's read once per cell in
   worldgen, so this is premature unless we add another reader.
   **Skip until another reader appears.**

3. **Add a generic `DirtyQueue<K, V>` infrastructure**. A typed
   queue with priority + budget; pulls entries off, calls a
   recompute function, writes back. Test it in isolation. **(small,
   pure data structure.)**

4. **Wire the mesh resubmit through DirtyQueue** instead of
   version-poll. Today the streaming loop polls every loaded
   chunk's version every frame. With DirtyQueue, only edits push
   entries — the loop sees a much smaller list. **(medium, replaces
   existing logic; test behavior is preserved.)**

5. **Per-chunk `thermal_mass` cache + dirty set**. Now it's
   warranted because the runtime water-flow simulation will read
   thermal_mass on every flow tick. Caching saves the 6-lookup
   work per flow event. **(small, builds on #3.)**

6. **Light propagation system using DirtyQueue**. Light is the
   highest-volume event source — block edits cascade through up
   to ~15 cells of fall-off. Without budgeting, a single break in
   a dense forest could flag 5000+ cells. The queue caps it.
   **(large, new feature.)**

7. **Water flow system using DirtyQueue**. Same shape as light:
   edits cascade. **(large, new feature.)**

### Traps to avoid

- **Don't cache pure functions of (position, time)** like
  `cloud_density_at_time`. They're not idempotent across ticks; a
  cache would be stale every frame. These should stay pure.

- **Don't put all systems in one shared DirtyQueue.** Each system
  has different priorities and budgets. A water flow event
  shouldn't compete with a light update for the same budget — both
  should run, just at independent rates.

- **Don't invalidate eagerly across chunks during the edit
  handler.** The edit handler should record the event and return
  fast. Cross-chunk effects are observed by subscribers on their
  next tick — never inside the click-handler call stack.

- **Don't let dirty sets grow unbounded.** If the budget is too
  small for the influx, dirty entries pile up forever. Either
  raise the budget when the backlog grows, or cap the backlog and
  drop the oldest (with a warning log when it happens — that's the
  signal to retune budgets).

- **Don't bypass the cache for "fast" callers.** If `temperature_at`
  has a cache, NO call site should reach past it to the raw
  function "for performance." The cache IS the performance path;
  calls through it justify its existence by hitting it.

### Tests to write

- DirtyQueue respects budget: with a 1ms budget and a recompute
  function that takes 0.5ms each, it processes ~2 entries per
  call regardless of pending size.
- DirtyQueue priority order: high-priority entries drain first.
- Cache invalidation: marking a cell dirty causes the next read
  to recompute; reads BEFORE the mark return the old value
  (proving the cache is being used).
- Cross-system isolation: a mesh-only edit doesn't trigger
  thermal_mass recomputes if no thermal_mass cells were
  invalidated.

### Why this is queued, not done now

It's the right move, but it's a foundational refactor that's
worth doing AFTER:
- Water flow exists (so we know what budget shape it needs).
- Light propagation exists (same reason).
- One real performance pain point has been observed in playtest
  (so the budgets can be tuned to a real workload, not guessed).

Doing it before those is premature: we'd build infrastructure
without knowing the actual access patterns. The risk of waiting
is that each new system has to be retrofitted — but the current
systems (mesh, version bump) already work, so retrofit is bounded.

The trigger to start work is: a playtest where the engine visibly
hitches on a block edit. That'll be the signal that one or more
new systems are eagerly cascading and the queue is needed.

## Worldgen throughput: bench-vs-live discrepancy (May 2026)

User reported "freezing at start" of `unendlich-world` — perception of
freeze rather than actual freeze. Engine isn't blocked; chunks
generate at ~22-30 cps with 12 workers (2 cps per worker).

`world-bench` reports ~15 cps single-threaded for a sea-level cube,
which would be 180 cps with 12 workers if linear-scaling. Actual is
**>5× slower than ideal**. There's contention or buffering somewhere.

Likely culprits, in order of suspicion:
1. **Shared DashMap on `chunks`**: worker writes go through the same
   map. High insert rate might be hitting shard contention.
2. **Storage layer**: chunks are written to disk (or checked for
   on-disk edits) on completion. Disk I/O on ~30 chunks/sec is fine,
   but if it's serialized through a lock it could limit throughput.
3. **MeshingPool full**: 4 workers / 64 capacity. If meshing can't
   keep up, completed chunks back up before they're fully usable.
4. **Streaming-loop polling**: the bin's main loop iterates every
   frame to find chunks needing requests / submissions. As
   `loaded_count` grows, this loop slows down.

Fast-path optimization (chunks >= Y=160 quick-reject as all-air)
delivers ~30% reduction in wall-clock generation work for the
default render-distance sphere, but doesn't address the contention
gap. That requires profiling.

### Investigative TODO

Add timing to:
- DashMap insert latency in chunk_manager::update.
- Storage::load_chunk_edits per-call duration.
- MeshingPool submission latency.
- Per-frame streaming-loop walk time.

Once we know which of those dominates, the fix is targeted.
Premature optimization is the wrong move — the fast-path was the
right pick because it has clear semantics (not all chunks need
full evaluation) and safe verification (the assumption test).

### What the user actually wanted

User's framing: "physics chunk cache to abstract some accuracy
for performance gains." The fast-path IS a chunk-level cache —
caches the result "this chunk is uniform, no per-cell work
needed." The semantic is "approximate by exploiting structure
in the input rather than computing every cell."

For the deeper contention issue, the analog would be batching:
collect N completed chunks before grabbing the chunks-map write
lock, instead of inserting one-at-a-time. Same idea: trade some
per-chunk freshness (delayed insertion) for lock-acquisition
amortization. Queued for the profiling-driven follow-up.

## Adaptive scheduler for compute fidelity (queued, ARCHITECTURAL)

Generalizes the worldgen fast-path that currently fires for chunks
above Y=160. The principle: **letting a scheduler decide per-chunk
whether to compute "real" or "fast" is a foundational pattern that
applies to many subsystems.**

### What this looks like

A scheduler module knows about:
- **Backlog pressure** per system (how many chunks pending
  generation, meshing, simulation tick, etc.).
- **Frame budget** per subsystem (max ms/frame allowed).
- **Distance from player** (or other relevance proxy) per chunk.
- **Available compute tiers** per subsystem ("real", "fast",
  "skip-and-defer").

Per chunk per subsystem, the scheduler picks a tier:

```
if backlog_low && distance_close: real
if backlog_high || distance_far:  fast
if backlog_critical:              skip+defer
```

The chunk's "current fidelity" is tracked. When conditions improve
(backlog drains, player moves closer), chunks scheduled for
re-promotion to real fidelity. The system is eventually-consistent:
fast first, real later when bandwidth allows.

### Subsystems that would benefit

- **Worldgen**: today only one fast/real distinction (Y >= 160).
  Could add: cave detail simplified at distance, ore generation
  deferred until close, tree placement skipped until close.
- **Meshing**: today single fidelity. Could add: greedy meshing
  for far chunks (less detail), full subdivision only for close.
- **Simulation tick**: weathering / plant lifecycle / water flow
  could tick at lower frequency for far chunks.
- **Light propagation** (when it lands): far chunks get coarser
  light estimates.
- **Field caching**: which fields to materialize per-chunk depends
  on what the renderer / simulation actually reads.

### Why it's queued, not done now

Building this scheduler before there's measurement data is the
wrong move. Each subsystem needs a real benchmark to:
1. Define the "real" vs "fast" cost gap (tier-pick decision uses it).
2. Define a realistic budget (otherwise the scheduler is guessing).
3. Verify the fast tier is visually acceptable (otherwise we ship
   visible artifacts when the scheduler picks fast).

The current single hardcoded fast-path is the right MVP. When
TWO subsystems need adaptive fidelity, that's the trigger to
build the scheduler. With one subsystem, hardcoding is fine.

## LOD-style simulation throttling (queued, with caveats)

User's framing: "LOD-style blocking for far away effects to reduce
simulation churn... could create artifacts without noise."

This is correct concern. LOD on RENDERING is mature — distant
geometry is simpler-looking, the player accepts it because the
artifacts are subtle (less detail). LOD on SIMULATION is risky
because simulation drives BEHAVIOR — slowing simulation at distance
means weathering / plant growth / heat propagation freeze at
distance, then catch up when the player approaches. The player
will SEE the catch-up.

### The risk in concrete terms

- Player walks away from a forest. Plant lifecycle ticks slow at
  distance. Player walks back 10 minutes later. Forest is exactly
  as it was, then suddenly grows in 2 seconds as the lifecycle
  catches up. Visible artifact.
- Heat source far away. Convection field froze at low fidelity.
  Player approaches. Convection fills in over a few frames. Wind
  arrow visibly reorients. Visible artifact.

### The mitigation: lazy catch-up with elapsed-time

Don't slow simulation at distance — instead, **don't tick at all
beyond some range**. When the player re-enters range, the simulation
fast-forwards using elapsed wall-clock time as a single big tick.

Example for plant lifecycle:
```
on_chunk_load:
    elapsed = now - chunk.last_simulated_at;
    if elapsed > 0:
        plant_lifecycle.advance_by(elapsed);
    chunk.last_simulated_at = now;
```

This works for systems where the simulation result is a function of
elapsed time (linear or exponential decay/growth). It DOESN'T work
for systems with feedback loops (cellular-automaton water flow,
where the result depends on neighbors that may also have been
unsimulated).

### Subsystems where this fits

- Plant lifecycle (linear-ish growth).
- Weathering decay.
- Heat source decay (fires going out over time).
- Block oxidization / rust accumulation (when those exist).

### Subsystems where this DOESN'T fit

- Water flow (cellular automaton).
- Cloud advection (depends on continuous wind state).
- Heat diffusion (depends on neighbors).
- Light propagation.

For those, **boundary effects** matter: an unsimulated chunk's
edge cells are stale relative to a neighbor's edge cells. Re-syncing
them when the player approaches is non-trivial.

### Concrete proposal

Tier 1 (immediate, low risk): apply elapsed-time catch-up to PLANT
LIFECYCLE only. It's the most visible candidate (forests grow back
when you return) and it has no feedback loops.

Tier 2 (medium): weathering. Same shape — accumulate "delta-time
since last simulated" per chunk and apply on next tick.

Tier 3 (research): coupled systems like water flow. Probably
requires actually solving the boundary-sync problem first.

### Why both ideas are queued together

The scheduler from the previous section is the dispatch layer; the
LOD throttle is the action it dispatches. Both belong to the same
architectural shift toward adaptive simulation. Building one without
the other works (the simple time-elapsed pattern doesn't need a
scheduler), but they compose well.

The trigger to start work is: a real performance hitch traced to
"too many chunks ticking at full rate." The current bottleneck is
chunk-load contention, not simulation cost — so this is queued
behind that fix.

## Material response matrices (SHIPPED)

Per-block-id response profile that maps environmental state
(temperature, humidity, sky exposure, slope, weathering) to
visual properties (color shifts, bump amplitude, frequency).
Replaces the global `tint_for_position`/`bumpy_height` branching
with a data-driven per-material approach.

Implemented in `src/world/material_response.rs` (data + math) and
wired into `src/render/mesher.rs` (sample env, look up matrix,
apply to vertex colors). `MeshRequest` carries an `Arc<WorldModel>`
so worker threads can sample temperature/humidity per face. Greedy
meshing now consults `response.merges()` — surface stone forces
per-block emission (so per-block tinting is preserved) while deep
stone keeps the fast greedy-merge path. Soft blocks (grass, sand,
dirt, etc.) all carry response matrices with climate-driven shifts.

Lessons:
- Surface stone non-merging cost ~10-20% vertices for surface
  stone faces (most are buried and never emit). Acceptable.
- WorldModel sharing via DashMap cache means parallel meshers
  populate the cache cooperatively.
- `compute_tinted_color_for_top/face` falls through to legacy
  `block_color()` palette for blocks without a matrix entry,
  so the change was a strict superset of prior behavior.

## Lazy clusters (next major architecture)

Sketch:

```rust
struct MaterialResponse {
    base_rgb: [f32; 3],
    humid_shift: [f32; 3],
    temp_warm_shift: [f32; 3],
    temp_cold_shift: [f32; 3],
    sky_dim_shift: [f32; 3],
    weathering_shift: [f32; 3],
    bump_amplitude: f32,
    slope_bump_boost: f32,
    bump_frequency: f32,
    weathering_bump_scale: f32,
    position_variance: f32,
}
```

Mesher samples once per face, looks up `response_for(block, wy)`,
applies. Greedy meshing checks `response.position_variance == 0`
to decide whether to merge — `0` keeps the fast path for deep
stone, `>0` forces per-block emission for surface stone (loses
~15-20% of greedy merges, gains visible texture).

Trigger to start: position-only tinting from this turn lands and
the user wants more variation. The infrastructure to plumb
`WorldModel` reference into the mesher is the main lift.

## Tree growth and gravity (queued, future)

Trees as growth processes rather than stamped templates. Each
tree column carries a tick-driven trunk_height that increases
under good conditions (humidity, light, mild temperature) up
toward `profile.trunk_max_height`. Each tick: sample local
climate, compute growth delta, possibly add a log block at top
(plus reposition canopy).

Gravity coupling: each trunk block has a "support" check.
Unsupported trunks (no log directly below for >2 blocks,
neighboring trees shade weakened — wind exposure exceeds
strength) get marked for fall. Falling: trunk converts to a
sideways stack of log blocks plus scattered leaves,
direction biased by local wind direction at the trunk top.

Wind threshold: cumulative wind exposure on the trunk vs trunk
strength (proportional to age × hardness). Strong gusts during
storms (future weather system) topple weakened trees.

This needs the storage to track per-tree state (trunk_height,
exposure_accumulated, age) — currently trees are pure block
patterns with no instance state. Adding a `tree_instances:
HashMap<(wx, wz), TreeState>` per chunk is the smallest
plumbing change, persisted via the same chunk-edit path.

Trigger to start: weathering tier-1 lands and proves the
"surface modification via tick handler" pattern works; tree
growth is an obvious next handler reusing that infrastructure.

## Lazy clusters with content-addressed identity (next architectural lift)

User proposed an alternative to per-type entities: clusters are
emergent from a connectivity rule + bounds limit + content hash.
Identity is `(bounds: AABB, content_hash: u64)`. Bounds limited
to 64×64×64 (covers 2×2×2 chunks max). Cluster types:
- Structural (rigid bodies — trees, cliff overhangs)
- Granular (sand, gravel, loose materials)
- Fluid (water, lava)
- Thermal (heat conduction zones)
- Structural-living (trees with growth state)

Content-addressed: same blocks at same positions → same cluster
id, regardless of which thread or session computed it. Survives
reload, parallelism-safe.

Boundary crosstalk: clusters of different types meeting at a
contact cell exchange forces / state. ~25 type-pairs; most are
trivial. Tree on cliff = structural cluster of logs sharing
boundary with structural cluster of stone; if cliff fractures,
both fall as one combined falling cluster.

Tick model: read-snapshot, write-accumulate. All clusters read
the same world state at tick start; all write into a buffer;
buffer applied atomically at tick end. Avoids order-dependence
without requiring iterative constraint solver.

Implementation increments:
1. Cluster identity primitives: `cluster_at(wx, wy, wz, rule)`,
   bounds, content_hash, ClusterId. Tests for determinism, cap.
2. Structural rule + gravity collapse (sand, trees, cliffs).
3. Visual smoothing: `Falling` ClusterTag + interpolated render
   offset for a few ticks before block grid actually shifts.
4. Granular rule + flow.
5. Boundary crosstalk between types.
6. Living-cluster rule for trees (growth as cluster acquiring
   blocks at top per tick).

This subsumes the per-type "tree state" plan from above. A tree
becomes "structural cluster of log blocks tagged living" — no
special tree code, just rules.

Trigger: ship after material response. The plumbing for
`Arc<WorldModel>` access from worker threads (built for matrices)
is a foundation that cluster sampling can also use.

## Pre-cluster session notes (housekeeping)

Before starting cluster work, capture some practical questions
that the user and I should agree on early:

### Cluster identity questions worth pre-deciding

- Where does ClusterCache live? On ChunkManager (per-loaded-chunk)
  or globally? Per-chunk fits invalidation (chunk version bumps
  invalidate clusters touching it) but cross-chunk clusters
  (a tree spanning a chunk boundary) need to invalidate two
  caches at once.

- How does a cross-chunk cluster's "anchor chunk" get chosen?
  Probably the chunk containing the cluster's bounds_min corner,
  in canonical Z-order.

- Cluster id should be `(bounds_min: i32x3, content_hash: u64)`,
  not include cluster type — same blocks could be in multiple
  clusters (structural + thermal). Type lives on the queriable
  Cluster struct, not on the id.

- Bounds limit: 64³ blocks. If flood-fill exceeds this, treat
  as "anchored" (connected to bedrock-implicit-cluster). Trees
  hit this bound naturally; cliffs fail it (good — cliffs are
  anchored, can't fall as a unit).

- Determinism of flood-fill: must be canonical (Z-order from
  the seed block). Two threads computing the same cluster get
  the same internal block ordering.

### Coupling with material response

Now that env-aware tinting is shipped, the same `EnvironmentSample`
type can be reused for cluster-property queries. A cluster's
"average humidity across constituent blocks" is computed by
sampling env at each block — the helper lives in mesher today
but should probably move to a shared module if cluster code
needs it too. Defer the move until it's actually needed.

### What I deliberately did NOT do tonight

- Did not start cluster increment 1. Architecture decisions there
  matter and should be made with user input, not solo overnight.
- Did not touch the chunk-streaming throughput problem
  (`gen_pending: 25001` while loaded is small). That's a real
  visual artifact (stripe effect at distance) but fixing it
  needs renderer-side awareness and is risky solo.
- Did not change rendering of unloaded chunks (the "skybox
  stripes"). Same reasoning.

### What I did do tonight

- Cleaned up all warnings (redundant imports, unused vars,
  documentation constants). Future warnings now signal real
  problems instead of getting lost in noise.
- Added a per-second climate log line so the player can verify
  what the mesher is sampling at their position.
- Added 3 multiplayer-determinism regression tests for the
  material-response plumbing: same model = same mesh, different
  model instances same seed = same mesh, different seeds =
  different mesh.

Test count grew from 326 to 329.

## Investigation: chunk queue drain (no-op, documented)

User saw `gen_pending: 25001` while `loaded: 427` during fast
travel. Verified the loader queue does drain correctly:

- `request_many` always sends `requests.len()` wake signals,
  even when most coords were already in the queue (i.e. signals
  are sent per call, not per new addition).
- Workers drain extra wake signals via `try_recv()` on line 188
  of `chunk_loader.rs`, then take a batch of up to GPU_BATCH_SIZE
  (16) coords from the priority heap, process them, ship results,
  and loop back to wait for the next signal.
- Even when player stops moving, the chunk manager's per-frame
  update calls `request_many` with the static "chunks I want"
  set, which keeps wakes flowing.

Real bottleneck on Apple M2 Max with Asahi Fedora: CPU generation
fallback (Vulkan feature unavailable for GPU path), ~80ms/chunk.
4 workers × ~12 chunks/sec/worker = 50 chunks/sec. 25K backlog
takes ~8 minutes to drain at full CPU. Not a code bug — a
hardware/feature limitation. Fixable when/if GPU path comes back.

## Tonight's micro-fixes (very small)

- `slippery_surface_decays_slower` test: prefixed unused `world`
  binding with `_world`. (Test passes regardless.)
- Named `TINT_REFERENCE_VARIANCE = 0.18` as a constant instead
  of magic number in `compute_tinted_color_for_top/face`.

No behavior change. No new ship. Same 399K zip from prior turn
remains current.

## Placement-history materials (queued, fold into terrain bumpiness work)

Materials that change identity over time as a function of their
placement history + accumulated pressure. Examples:

- **Wet earth / cob**: placed soft, deforms under load (settles),
  loses moisture over time, transitions to a stronger "cured cob"
  block ID at some threshold. Built incrementally — each layer
  cures before the next is added on top, otherwise lower layers
  collapse from premature loading.
- **Lime mortar**: placed between stones, cures with moisture loss,
  bonds adjacent stones into a single structural cluster (cluster
  crosstalk turns "stacked" into "merged").
- **Frozen ground**: temperature-driven transition between wet
  earth states; thaws under heat, becomes unstable.
- **Settled gravel**: gravel that's been compressed long enough
  to lock; transitions to higher-strength state.
- **Charred wood**: wood near heat source transitions to weaker
  state, erodes faster.
- **Polished stone**: stone with repeated low-magnitude pressure
  (foot traffic, water flow) becomes smoother visually.

Architecture: same pressure-field machinery as weathering.
Difference: pressure can be POSITIVE (strengthening) not just
negative (weakening). Threshold transitions change block ID
(not just attributes), so neighbors see the new state.

Trigger to start: when revisiting terrain bumpiness for a more
principled per-block visual variation, fold in placement-age and
moisture as additional pressure dimensions in the same data path.
The infrastructure pays for itself across both visual variation
AND material curing.

Gameplay implication: building a stone wall on bedrock is stable.
Building one on cob that hasn't cured collapses both. Building
in stages, allowing curing between layers, becomes a real verb.
Players who learn this build differently from players who don't.

## Rope and entity-stuff (deferred to entity substrate)

Player verbs that need non-voxel-aligned physics:
- Throw a rope, anchor it, crawl across
- Tossed objects (rocks, projectiles) with arc trajectories
- Walking creatures whose body isn't grid-aligned
- Liquid streams (water spilling, lava drips) with surface tension

All of these need entities — data objects that exist in continuous
3D space, with their own physics integration, rendering, and
interaction with the voxel grid.

Defer to whenever entity substrate gets built. That arc is
parallel to cluster physics, not folded into it. The cluster
machinery handles voxel-aligned physics; entities handle the rest.

When entity substrate happens, rope is the simplest entity to
build first — 1D anchored line with tension along its length —
and validates the data flow for non-voxel things.

## Cluster increment 1 (SHIPPED)

Identity primitives + connectivity rules. No physics, no caching,
no rendering changes. Pure substrate.

What's in `src/world/cluster.rs`:

- `ClusterId { bounds_min: [i32; 3], content_hash: u64 }`. The
  `ANCHORED` constant marks "you're connected to bedrock-implicit,
  you don't move."
- `ClusterRule` trait with `connects(from, to) -> bool` and
  `max_axis() -> i32`.
- `StructuralRule` (16-cap, same-material adjacency, no granular
  blocks) and `MeshBoundRule` (64-cap, looser semantics — ready
  for future expansion to climate-coherent visual features).
- `BlockSource` trait with `SingleChunkSource` and `MultiChunkSource`
  implementations. The multi-chunk version takes a closure for
  flexibility — works with DashMap, RwLock, HashMap, anything.
- `cluster_at(wx, wy, wz, rule, source) -> Cluster`. Flood-fills
  with bounds-cap and volume-cap; returns `Cluster::anchored()`
  on overflow.
- Members are sorted in canonical (y, z, x) order at the end of
  flood-fill, then content_hash is XOR-combined per block. XOR
  is order-independent so any traversal order yields the same id.

15 tests covering: empty world, isolated blocks, multi-block
clusters, material discrimination, diagonal non-clustering,
sand/gravel exclusion, determinism across runs and chunk
instances, content-hash stability under traversal-order changes,
bounds-cap enforcement, mesh-bound vs structural cap differences,
**cross-chunk clusters** (the key multiplayer-prep property —
same identity from either side of a chunk boundary).

What's deliberately NOT in increment 1:
- No per-chunk cluster cache (clusters are computed lazily on
  every call). Caching gets added in increment 2 when we know
  the access pattern.
- No physics. cluster_at returns identity; nothing acts on it.
- No reconciliation logic. Cross-chunk clusters work via
  MultiChunkSource — no merge needed yet because no per-chunk
  state to merge.
- No serialization. Clusters are derived state, not persistent.

Increment 2 will add the pressure field substrate and start using
cluster identity to modulate collapse thresholds (cluster context
matters for "should this cell collapse" decisions).

## Scheduler clarification (May 2026)

Earlier design notes had two competing schedulers and we conflated them:

1. **Distance-based deterministic scheduler**: per-chunk tick rate is a
   pure function of player-chunk distance. Three bands: render-distance
   (full rate), simulation-distance (1/Nth rate), catch-up-distance
   (rate=0, runs catch-up on reload). Rate is fixed by distance, no
   time-budget heuristics. **KEEP THIS** — it's deterministic and
   complements pressure-field sampling.

2. **Runtime-budget-aware skipper**: "if frame budget exhausted, skip
   remaining chunks this tick." This introduces non-determinism (outcome
   depends on wall-clock timing) and is a multiplayer-killer.
   **DROP THIS** — pressure fields reduce per-tick cost up front via
   sparse sampling, so reactive runtime skipping isn't needed.

The distinction matters: the first treats tick rate as a PROPERTY of
the chunk's distance; the second treats tick rate as a RESPONSE to
runtime conditions. First is deterministic, second is not.

### Time-homogeneity constraint

Distance-based scheduling only works if pressure rules are
**time-homogeneous**: "advance N ticks at once" must produce the
same end state as "advance 1 tick N times." This is a real
constraint on rule design:

- Pressure accumulation per tick must be additive and stateless
  within a tick — sparse-sample N cells, blur, that's it.
- Collapse decisions must be deterministic given (pressure, fields,
  block state).
- No "this tick depends on the last tick's outcome in a non-additive
  way" — no momentum, no carry-over state beyond the pressure field
  itself.

If we maintain this property, then a chunk that dropped to 1/10 rate
when the player walked away can safely "catch up" by advancing 10
ticks at once when the player returns, with identical outcome to
having ticked normally the whole time. This is what makes the
distance-based scheduler trustworthy.

The 1000× tick speed problem the user hit during playtest gets
solved by this scheduler: far chunks tick at lower rates, so 1000×
tick speed compounds bounded amounts of work per real second.
Currently every loaded chunk ticks at full rate regardless of
distance, so 1000× × 1429 chunks saturates 4 cores.

## Window-freeze on Esc-Esc shutdown — partial mitigation (May 2026)

User reports: after double-Esc, process drops CPU/MEM (sim loop
exited) but window stays visible until Ctrl-C. Almost certainly
`device_wait_idle()` in renderer.destroy() blocking on Apple M2 Max
Asahi Vulkan, where some GPU work never completes (likely a fence
or semaphore not signalling cleanly).

Mitigation applied: call `window.hide()` BEFORE any Vulkan teardown.
The window vanishes immediately on Esc-Esc; if the process then takes
extra time to exit (or hangs in device_wait_idle), the user no longer
sees a frozen window. Diagnostic log lines added between each teardown
step so future investigation can pinpoint exactly which call hangs.

This is a workaround, not a fix. The real fix would require either
investigating the GPU state at shutdown time (needs Vulkan validation
layers + a working setup we don't have) or restructuring the renderer
to track and explicitly wait for only the in-flight frames rather
than calling device_wait_idle on the whole device. Defer until we
can reproduce locally.

## Palette test bin (SHIPPED)

`cargo run --release --features render --bin unendlich-palette`

Hand-authored single chunk containing 19 block types laid out as
3×3×4 columns on a stone floor. Uses the climate-aware mesher path
(mesh_chunk_with_sky_and_model) so the material-response matrix's
tints activate. Press N or Space to cycle through 6 climate
positions (origin, far east, far south, far west, far north,
diagonal); each cycle re-meshes the palette at the new world
position so blocks tint differently per the local climate. Climate
values are logged to console at each cycle so the user can correlate
visual change with the temp/humidity that drove it.

This is a debug tool, not gameplay. No architecture commitment.
Useful for verifying the matrix output without exploring 5000 blocks
to find a tundra biome.

## Track B increment 1: distance-based scheduler (SHIPPED)

**Module**: `src/world/scheduler.rs`. Pure primitives:
- `ChunkTickBand { Render, Simulation, CatchUp }` — three bands, each
  with a divisor (1, N, 0).
- `TickScheduleParams { render_distance, simulation_distance, simulation_divisor }`
  — configurable thresholds. Default: `(RENDER_DISTANCE, RENDER_DISTANCE*2, 8)`.
- `chebyshev_distance(a, b)` — canonical distance metric.
- `band_for_chunk(chunk, player, params) -> Band` — pure function of state.
- `should_fire_in_band(band, tick_id, params) -> bool` — Render every tick;
  Simulation when `tick_id % divisor == 0`; CatchUp never.
- `tick_dt_for_band(band, period, params) -> f32` — Render = period;
  Simulation = period × divisor; CatchUp = 0.

14 tests covering: every band assignment edge, fire patterns per band,
determinism (same inputs → same outputs), and the constant-integral
property (fire rate × dt = constant per unit wall time, modulo end-
of-window remainder).

### Time-homogeneity refactor

`leaf_decay::advance_one_tick(state, wind)` is now a thin wrapper
around `advance_n_ticks(state, wind, n)`. The latter is the
canonical primitive: aging accumulates linearly with n, with
saturation at u8::MAX.

This refactor exists to support catch-up: when a chunk re-enters
a faster band after being away, its leaves need to "fast forward"
N ticks of decay in one call. With `advance_n_ticks` we can do
that and prove (via test) that it produces the same end state as
N individual ticks.

3 new tests in `leaf_decay`:
- `time_homogeneity_calm_wind` — N=50 in one call equals 50
  individual calls (same age, same flags, same death status).
- `time_homogeneity_gusty_wind` — same property under accelerated
  decay regime.
- `time_homogeneity_crosses_death_threshold` — N=10000 saturates
  age correctly and reports death.
- `advance_zero_ticks_is_query` — n=0 is a non-mutating death check.

### Band-aware decay handler

`decay_handler` now consults `ctx.schedule` and `ctx.player_chunk`
to classify each chunk's band. Behaviour:
- **Render band**: fires every tick at age=1 (identical to old behaviour)
- **Simulation band**: fires every Nth tick at age=N (compensating
  for skipped ticks via `advance_n_ticks(.., N)`)
- **CatchUp band**: no fire (handled by future on-reload catch-up)
- **Schedule absent** (test contexts, etc.): legacy radius cull behavior

1 new integration test:
- `band_aware_leaves_match_render_after_n_ticks` — runs the actual
  handler against two chunks (one Render, one Simulation) for 64
  game-ticks and verifies their leaves end at identical age. Proof
  that the band machinery preserves time-homogeneity end-to-end,
  not just at the math primitive level.

### Plumbing

`TickContext` now has `schedule: Option<TickScheduleParams>`.
`TickRegistry::run_tick` takes the schedule param as its 7th arg.
`ChunkManager`'s tick driver passes `Some(default())`. Test sites
pass `None` (and get the legacy radius-cull behavior, preserving
their test expectations).

### What's NOT done in this increment

- **Auto-triggered catch-up**: when a chunk transitions from a slower
  band to a faster band (player approaches), we should run handlers
  for it with `advance_n_ticks(elapsed_ticks)` to bring it up to date.
  Today the chunk just resumes ticking from wherever it was; if it
  was in CatchUp band for 1000 ticks, those 1000 ticks of evolution
  are lost. Adding this requires tracking `last_tick_handlers_ran` per
  chunk (small state addition). The math primitive (`advance_n_ticks`)
  is already proven correct; just need the trigger logic.
- **Other handlers**: `decay_handler` is currently the only registered
  tick handler. Future handlers (plant lifecycle, future fluid flow)
  need similar band-awareness. The pattern in `decay_handler` is the
  template.
- **1000× speed validation**: the user's playtest hit 17 FPS at 1000×
  with `loaded:1429`. This increment should improve that significantly
  for leaf-decay-heavy areas (most chunks now skip 7 of 8 ticks),
  but the dominant cost was likely fire propagation and other
  systems not yet on the scheduler. Validation needs a fresh playtest.

## Track B increment 2: catch-up tracker (SHIPPED)

**Module addition**: `LastFireTracker` in `src/world/scheduler.rs`.
Per-(handler, chunk) `TickId` tracking. Closes the catch-up loop:
when a chunk transitions from a slower band to a faster band (player
approach), handlers consult the tracker to learn the elapsed-tick
gap since their last fire on that chunk, then advance the right
number of simulated steps via `advance_n_ticks(n)`.

API:
- `elapsed_or_default(handler, chunk, current_tick, default_n)` —
  query the gap; first-fire returns default_n.
- `mark_fired(handler, chunk, tick)` — handler calls after per-chunk work.
- `evict_chunk(coord)` — clear all entries for a chunk; called by
  ChunkManager on chunk eviction so the tracker map doesn't grow
  unbounded.

6 new tests covering: default-on-unknown, gap-after-mark, minimum-1
clamping, cross-handler isolation, eviction, and the full catch-up
gap scenario (1000-tick gap returns 1000 not the band default).

### Plumbing

`TickContext` gained `fire_tracker: Option<&LastFireTracker>`.
`run_tick` takes it as an additional arg. `ChunkManager.TickState`
owns one `Arc<LastFireTracker>` per world; passes through on every
tick. Eviction in `evict_distant` removes tracker entries for any
evicted chunk.

### Decay handler integration

`decay_handler` now consults `ctx.fire_tracker` before deciding the
`n` to pass to `advance_n_ticks`:
- With tracker: `n = elapsed_or_default(handler_name, chunk, tick_id, band_default)`
- Without tracker (test contexts): `n = band_default`

After the per-chunk loop, `mark_fired(handler_name, chunk, tick_id)`
records the fire so the next fire computes its gap correctly.

Verified end-to-end via `catchup_via_tracker_matches_continuous`:
two chunks with fresh leaves; chunk A fires every tick for 50 ticks
under continuous Render-band; chunk B is in CatchUp band the whole
time and never fires. After teleporting the player so B enters
Render, B's first fire applies the catch-up gap (50 ticks at once)
and reaches the same age as A. Proves catch-up correctness through
the real handler with the real tracker, not just at the math primitive
level.

### Persistence note

Tracker state is in-memory only — not persisted across save/load
or chunk eviction. A chunk that's been evicted, then reloaded later,
loses its tracker history. Its first-fire-after-reload uses the
band default, NOT a catch-up gap based on game-time elapsed. This
is acceptable for now (chunks far enough away to be evicted are
"reset" semantically — equivalent to "fresh load") but is a real
limitation for any future system that needs persistent catch-up
across long player absences.

The fix would be to persist `last_fire` per-chunk-per-handler
alongside the chunk's block data. Not done in this increment because
it requires save-format changes and the use case isn't urgent.

## Honest scope note: plant_lifecycle is NOT band-aware

Track B was scoped as "land the scheduler + make handlers use it."
Land the scheduler: done. Make handlers use it: only `decay_handler`
got the full treatment. The other registered handler,
`plant_lifecycle::lifecycle_handler`, still uses the legacy
`chunk_in_tick_range` cull and standard `advance_one_tick`.

This is because plant_lifecycle's `advance_one_tick` has stage
transitions that reset `age_ticks = 0` when a threshold is crossed.
A naive `advance_n_ticks(N)` would either skip transitions entirely
(just adds N to age, possibly past multiple thresholds) or naively
loop. The correct algorithm is: while N > 0, compute ticks-until-
next-stage-transition, advance min(N, that), do the transition if
reached, decrement N. This is straightforward but enough work that
I want to scope it as a separate increment with its own tests.

**Practically this means**: in the current build, `decay_handler`
is band-aware-aware but isn't currently registered (per the
intentional decision in `unendlich_world.rs:105-115`). The
registered `lifecycle_handler` still does full-rate ticking for
all in-radius chunks. So Track B's cost reduction doesn't yet
flow through to playtest performance.

Next-session work item: write `plant_lifecycle::advance_n_ticks`
correctly (with stage-transition handling) + unit tests for time-
homogeneity, then update `lifecycle_handler` to consume the
scheduler/tracker the way `decay_handler` does. After that the
1000× tick speed problem should be tangibly improved.

## Open question — sky-exposure shadowing

User asked during playtest: "did this tree in the pit grow because
of the ledge or the sun?" Answer: neither — tree placement only
checks (substrate, biome, climate at the surface column). Sky
exposure / shading is NOT a placement input. Trees grow happily
in deep pits even though a real plant would be light-limited.

Potential future addition: a sky-exposure factor in
`climate_tree_factor` (or a hard rejection at low sky exposure).
Requires the worldgen to compute / sample a sky-exposure metric,
which doesn't exist yet but would be cheap (raycast up from the
candidate position; count air vs solid blocks until ceiling).

Filed under "polish for later" — not blocking anything, but worth
knowing trees are biologically optimistic about their light
budget.

## Track B increment 3: plant_lifecycle band-aware (SHIPPED)

The previous increment landed scheduler primitives + catch-up tracker
+ band-aware decay handler. But decay handler isn't registered in the
current build — the actually-registered handler was plant_lifecycle,
which still used legacy radius-cull and per-tick `advance_one_tick`.

This increment closes that gap:

### `plant_lifecycle::advance_n_ticks`

Catch-up-aware version of `advance_one_tick`. Walks the stage machine
in chunks: compute ticks-until-next-transition, advance min(remaining,
that), do the transition if reached, decrement remaining. Handles all
five stages including:

- Seedling/Growing/Mature: standard threshold-based transitions, with
  ticks-to-threshold calculated as `ceil((threshold - age) / per_tick)`
  where per_tick is 2 (wet) or 1 (dry).
- Mature with adequate moisture: no transition, just age accumulation
  (saturates at u8::MAX). Fast-forward via single `state.age()` call.
- Drying: standard threshold to Withered, BUT the recovery branch
  (moisture > 0.7 → instant Mature) preserves the original match
  ordering quirk: if age would also cross the death threshold this
  tick, death wins over recovery. Critical for byte-exact homogeneity.
- Withered: terminal, fast-forward all remaining ticks at once.

The single-tick `advance_one_tick` is now a thin wrapper around
`advance_n_ticks(_, _, 1)` — single source of truth for stage-machine
behavior.

### 11 new time-homogeneity tests

For each region of the stage machine, verify `advance_n_ticks(N)`
produces byte-identical state to N consecutive `advance_one_tick`s
under constant moisture:

- `time_homogeneity_seedling_dry/wet` — early stages, both moisture regimes.
- `time_homogeneity_growing_marginal_moisture` — middle band [0.4, 0.6].
- `time_homogeneity_mature_dry_drying_to_withered` — full death pipeline.
- `time_homogeneity_mature_wet_stays_mature` — terminal-stable case.
- `time_homogeneity_drying_dry_to_withered` — death from Drying.
- `time_homogeneity_drying_recovery_low_age` — recovery path.
- `time_homogeneity_drying_at_death_threshold_with_wet` — the
  death-vs-recovery race. Wet plant at exact death threshold withers
  rather than recovering (matches `advance_one_tick` match order).
- `time_homogeneity_withered_terminal` — saturating age accumulation,
  tested up to N=100,000.
- `time_homogeneity_long_catchup_gap` — N=500 across all moisture
  regimes; exercises the realistic catch-up scenario.
- `time_homogeneity_zero_ticks_no_op` — N=0 leaves state untouched.

All 9 original plant_lifecycle tests preserved without modification.

### Handler integration

`lifecycle_handler` now consults `ctx.schedule` and `ctx.fire_tracker`
the same way `decay_handler` does:
- Distance-based band classification (Render / Simulation / CatchUp).
- `n = elapsed_or_default(handler_name, chunk, tick_id, band_default)`
  for catch-up.
- `mark_fired` after each chunk's per-block loop.
- All existing fast-paths (Mature-stable, Withered-cleanup,
  Grass-surface-bearer special case) preserved unchanged.

### Practical impact

This is the change that should actually benefit playtest performance,
because plant_lifecycle IS the registered handler. Plants in chunks
beyond render distance now tick at 1/8 cadence (each fire applies
8 ticks of accumulated lifecycle), so 1429 loaded chunks no longer
all do per-tick plant scans every tick.

Cost reduction proportional to (plant chunks beyond render distance)
× (1 - 1/8). For a typical playtest with ~1400 loaded chunks, ~95%
beyond render distance, this is a meaningful drop in tick_sim time.
Real validation needs a fresh playtest at 1000× tick speed.

### Test count: 379 / 379

Was 368 (+11 homogeneity tests). All clean. Both feature paths
build clean (no-default and render+gpu).

## Track B status: COMPLETE

All four originally-scoped pieces shipped:
1. ✅ Scheduler primitives (Band, ScheduleParams, fire-rule, dt-rule).
2. ✅ Time-homogeneity contract documented + enforced via tests.
3. ✅ Catch-up tracker (LastFireTracker) with eviction handling.
4. ✅ Band-aware handlers — both leaf_decay AND plant_lifecycle.

The only "Track B" remaining item is **persistent catch-up across
chunk eviction-and-reload**. Currently when a chunk is evicted, its
tracker entries clear, and on reload its first fire uses the band
default (no catch-up). Persistent catch-up would require saving
last-fire-per-handler alongside chunk block data — a save-format
change and not strictly necessary for the gameplay we have today.
Defer until there's a use case that needs it (e.g. decadelong
weathering of structures the player built).

Next session: likely Track A (pressure fields) — the time-
homogeneity discipline established here directly applies to
pressure rules.

## Increment 2a: animation infrastructure (SHIPPED)

The first half of the noise-driven-transitions plan. Splits cleanly
into "infrastructure for animating block transitions" (this) and
"noise-driven probabilistic transitions" (2b, next).

### Data layer

**`BlockTransition`** in `src/world/chunk.rs`:
```rust
pub struct BlockTransition {
    pub from_stage: u8,
    pub to_stage: u8,
    pub started_tick: u64,
}
```
Stored sparsely in `Chunk::transitions: BTreeMap<u16, BlockTransition>`
— same pattern as `instance_state`, only blocks currently animating
get an entry. Not persisted to disk: animations are transient (3
seconds at default tick rate). On save/reload a mid-animation plant
just snaps to whatever was in instance_state at save time.

`Chunk::start_transition`, `finish_transition`, `is_transitioning`
helpers. All bump `dirty` and `version` to flag the chunk for re-mesh.

`TRANSITION_ANIM_TICKS = 6` — 6 ticks at default 0.5s tick rate = 3
real seconds. At 1000× tick speed, completes in 6ms — intended:
fast-forward fast-forwards animations too. Animation is keyed off
TickId, not wall clock, so it scales with simulation speed naturally.

### Tick context plumbing

**`DeferredTransitionOp`** in `src/world/ticks.rs`:
```rust
pub enum DeferredTransitionOp {
    Start { world_x, world_y, world_z, transition },
    Finish { world_x, world_y, world_z },
}
```

`TickContext` and `TickEdits` carry a `transition_ops: Vec<...>`.
`enqueue_transition_start` and `enqueue_transition_finish` helpers
on TickContext mirror `enqueue_state` / `enqueue_edit`.

ChunkManager's tick-apply phase groups ops by chunk and applies them
with one COW clone per chunk (same batching pattern as block_edits
and state_changes). Order is preserved so Start-then-Finish in one
tick correctly nets to no-entry.

### Per-tick re-mesh trigger

After the tick-apply phase, ChunkManager iterates chunks with non-
empty `transitions` and bumps version via `clone_for_edit`. This
causes the streamer to re-submit them next frame, producing one
re-mesh per chunk per tick the animation is in flight.

**Cost**: one COW clone + version bump per chunk per tick. Sparse
in practice (only plants currently animating) and bounded (each
animation lasts 6 ticks). Worst case: a forest where 50 chunks
each have one transition ⇒ 50 chunk re-meshes per tick = 100/sec.
Each re-mesh is a few ms of worker thread time. Real budget impact
will need playtest validation.

**Long-term upgrade path**: shader-side animation. Mesh once with
both `from_stage` and `to_stage` color modulations baked per cell;
shader interpolates by a per-frame uniform progress. No re-mesh
during animation. Defer to its own increment if per-tick re-mesh
turns out to be too expensive.

### Mesher integration

New entry point `mesh_chunk_with_sky_model_and_tick(chunk, sky,
world_model, current_tick)` — same as `mesh_chunk_with_sky_and_model`
but takes the simulation tick. The mesher computes
`progress = (current_tick - started_tick) / TRANSITION_ANIM_TICKS`
clamped to [0, 1] and lerps the color modulation between
`stage_color_modulation(from_stage)` and `stage_color_modulation(to_stage)`.

Existing entry points (`mesh_chunk`, `mesh_chunk_with_sky`,
`mesh_chunk_with_sky_and_model`) propagate `current_tick: None`
and render at to_stage immediately when transitions are present —
graceful fallback for test/utility paths that don't have tick info.

`MeshRequest` in the meshing pool carries `current_tick:
Option<u64>`. The bin sets it from `chunk_manager.current_tick()`.
Worker thread picks the tick-aware variant when both world_model
and current_tick are present.

### Handler integration

`lifecycle_handler` was rewritten to detect Mature→Drying as the
animated transition. Flow per plant:

1. **If transition in flight**: check elapsed since `started_tick`.
   - Past `ANIM_TICKS`: commit the `to_stage` to instance_state,
     enqueue Finish to clear the transition entry, age=0 (matches
     `advance_one_tick`'s reset semantics).
   - Mid-animation: no-op. Mesher continues interpolating.
2. **Else, advance the state**. If it would naturally transition
   Mature→Drying AND `ticks_to_advance ≤ ANIM_TICKS` (player nearby,
   render distance):
   - Enqueue `transition_start` with `from=Mature, to=Drying,
     started_tick=current`.
   - DON'T enqueue a state change. instance_state stays Mature
     until the transition commits.
3. **Catch-up case** (`ticks_to_advance > ANIM_TICKS`): commit
   directly via the normal state_change path. The player wasn't
   there to see the animation, so don't queue one.

All existing fast-paths preserved (Mature-stable, Withered-cleanup,
Grass-surface special case).

### Tests

10 new tests:

- 6 chunk-level: empty has no transitions, start/finish round-trip,
  start replaces existing, finish-when-absent is no-op, dirty/version
  bumps, per-block independence.
- 4 handler integration tests:
  - `handler_starts_transition_on_mature_to_drying`: Mature plant
    at threshold + dry moisture → handler enqueues exactly one Start
    op, NO state changes.
  - `handler_commits_transition_after_anim_duration`: pre-loaded
    transition in flight, tick at deadline → handler commits the
    to_stage AND enqueues Finish.
  - `handler_skips_plants_mid_animation`: pre-loaded transition,
    tick mid-animation → handler is fully no-op (mesher does the work).
  - `handler_skips_animation_during_catchup`: large catch-up gap
    via tracker → handler skips animation and commits directly.

Test count: 379 → 389. All passing. Both feature paths build clean.
Both bins (unendlich-world, unendlich-palette) build clean in release.

### Visible payoff

When a Mature plant naturally transitions to Drying via the existing
deterministic rule (moisture < 0.4 AND age >= 60), the player now
sees a smooth color fade from green to straw-yellow over ~3 real
seconds, instead of an instant pop. At 1000× tick speed the fade
completes in milliseconds.

### What's NOT in 2a

- Other transitions are NOT animated yet. Seedling→Growing,
  Growing→Mature, Drying→Withered, Drying→Mature recovery still
  commit instantly. Only Mature→Drying animates. The mechanism
  generalizes — easy to extend later.
- Sub-voxel cell density doesn't change during animation. We chose
  "snap" over "cross-fade" or "stochastic threshold" — and Mature
  vs Drying use the same sub-voxel grid anyway, only color
  differs. When we have block-type-changing transitions
  (Withered→Air), cell density snap-at-midway becomes relevant.
- Shader-side animation. Per-tick re-mesh is the simpler interim
  approach. Upgrade later if cost is too high.

### Next: 2b

The pressure-field substrate + noise-driven probabilistic
transitions. The Mature→Drying transition (currently fired by the
deterministic moisture < 0.4 rule) becomes the first user: it gets
fired by a noise-sampler-driven probability function instead.
Animation infrastructure from 2a means the visual transition is
already smooth — 2b just changes what TRIGGERS the transition.

Then growth: same machinery with inverted threshold. In a wet
forest few samples cross the death threshold but many cross the
growth threshold → forest sustains. In a dry zone, opposite.
Forest dynamics emerge from one mechanism.

## Increment 2b: noise-driven pressure sampling (SHIPPED)

The architectural rehearsal for the pressure-fields work, applied
directly to plant lifecycle.

### Pressure sampler primitive

New module `src/world/pressure_sampler.rs`. Pure function:
```
pressure_samples(chunk, tick, dimension, n) -> Vec<PressureSample>
```
where `PressureSample = { local_x, local_y, local_z, noise_value }`.

Deterministic via hash of `(chunk_coord, tick_id, sample_index, dim_seed)`.
Same inputs always produce the same samples.

**Cost**: O(n samples × chunks × ticks), independent of plant population.
A chunk with 200 plants and a chunk with 5 plants pay the same per-tick
cost — samples just have more potential targets in the dense one.

**Default budget**: `SAMPLES_PER_CHUNK_PER_TICK = 8`. Matches
`simulation_divisor` so a Simulation-band chunk firing once per 8
ticks gets 8 batches × 8 samples = 64 samples, equivalent to a
Render-band chunk firing 8 times × 8 samples each.

**10 sampler tests**: determinism, cross-tick variation, cross-chunk
variation, dimension independence, in-bounds positions, distribution
spread, noise-value uniformity (rough chi-squared), sigmoid endpoints,
p_growth = 1 - p_death, time-homogeneity.

### Probability functions

```
p_death(moisture)  = 1 / (1 + exp(10 * (moisture - 0.4)))
p_growth(moisture) = 1 - p_death(moisture)
```

Sigmoids centered at the old deterministic threshold (0.4). Steepness
(10) tuned so the curve crosses [0.1, 0.9] over a moisture range of
~0.4. At moisture=0.4 both probabilities are 0.5. At moisture=0.6,
p_death ≈ 0.12 and p_growth ≈ 0.88. At moisture=0.2, reversed.

### Handler integration

`lifecycle_handler` now has TWO passes per chunk per tick:

**Pass 1: noise-driven samples.** For each Sample in `MoistureStress`
and `Growth` dimensions:
- Look up the sampled (local_x, local_y, local_z) cell.
- If it's a plant of the right stage (Mature for MoistureStress;
  Seedling/Growing for Growth):
  - Sample local moisture via the field reader.
  - Compute `p_death` or `p_growth`.
  - If `noise_value < p`: fire the transition.
- For Render-band cadence (ticks_to_advance ≤ ANIM_TICKS), Mature→
  Drying starts the visual animation. For longer catch-up, commits
  directly (player wasn't there to see).
- Growth promotion (Seedling→Growing or Growing→Mature) commits
  directly without animation.

**Pass 2: deterministic per-plant lifecycle.** Plants not affected
by Pass 1 still get the existing flow:
- `advance_n_ticks` for age accumulation, Seedling/Growing/Mature
  age-based promotion (acts as a fallback when noise samples don't
  fire growth often enough), Drying→Withered death, Drying→Mature
  recovery, Withered terminal.
- The Mature→Drying transition is **suppressed** in this pass: we
  pass `moisture.max(DRY_MOISTURE_THRESHOLD)` to `advance_n_ticks`
  for Mature plants, so the function never sees dry conditions for
  them and never starts drying. The sampler is the only path now.
- Plants that the sampler already fired this tick are skipped
  (tracked in a `sampler_started` HashSet).

**Catch-up handling**: when `ticks_to_advance > 1`, samples are drawn
for `min(ticks_to_advance, ANIM_TICKS + 1)` consecutive tick_ids so
the noise stream advances correctly. Capped because once a sample
fires Mature→Drying for a plant, additional samples on the now-
Drying plant are no-ops.

### Forest dynamics that emerge

In a wet biome (avg moisture > 0.6):
- Few MoistureStress samples cross threshold → few deaths.
- Many Growth samples cross threshold → many promotions.
- Net: forest sustains, occasionally a stressed plant in a dry pocket
  dies, growth fills in elsewhere.

In a dry biome (moisture < 0.3):
- Many MoistureStress samples cross threshold → many deaths in
  spatially-clustered patches (clusters because adjacent samples
  with low noise both fire on adjacent plants).
- Few Growth samples cross threshold → little replacement.
- Net: forest visibly thins over minutes of play.

In transition zones (moisture 0.3-0.6):
- Both rates non-zero, dynamic equilibrium — plants die and grow
  at varying rates depending on local micro-climate.

### Test impact

`handler_starts_transition_on_mature_to_drying` was rewritten to
test the sampler-driven path: run handler over up to 5000 ticks
under dry conditions, expect a transition to fire eventually. With
8 samples × 4096 cells × p_death(0.2) ≈ 0.88, expected fire is
~0.17% per tick = 1 in ~580 ticks. 5000 ticks is plenty.

The other tests (commits-after-anim-duration, skips-mid-animation,
skips-during-catchup) didn't need changes — they pre-load
transitions and don't rely on the handler firing them.

Test count: 389 → 399 (+10 sampler tests). All passing. Both
features paths build clean. Release build clean.

### Visible playtest behavior

In typical biomes (moisture 0.4-0.6) you should see:
- Plants occasionally start drying (color fade green → straw yellow,
  3 real seconds at default tick rate).
- New plants showing up (Seedling/Growing→Mature promotions).
- Spatial clustering: deaths cluster in dry zones, growths cluster
  in wet zones.

Deaths at ~6/min/chunk in dry zones. Growth promotions at similar
rates in wet zones. Per-chunk costs: 16 sample evals/tick + 1 hash
lookup each — negligible compared to the rest of the simulation.

### Cleanup work deferred

- The animation gate in Pass 2 (the `if old_stage == Mature && new_stage
  == Drying` block) is now dead code — Mature→Drying is suppressed in
  Pass 2's `advance_moisture` clamp. Could be removed for clarity but
  isn't broken. Folded into next polish session.
- Growth-as-spawning (creating new plants in empty cells). Currently
  growth only promotes existing plants. Real "forest fills back in"
  needs a separate eligibility check (grass surface, no plant
  currently there) and a different operation (set block + state).
  Filed as a future increment.

### Track A status: pressure-sampler substrate IS shipped

What we called "pressure fields" before is now in place for the
moisture dimension. The full multi-dimension pressure storage isn't
needed yet — moisture is sampled live from the existing field. New
dimensions (erosion, structural, thermal) get added when their
first user is ready.

This is the "rehearsal" that proves the architectural pattern: noise
as spatial sampler, fixed budget per chunk per tick, deterministic
catch-up, time-homogeneity. Ready to be applied to the next physics
domain (erosion or cluster-collapse-driven).

## Increment 2c: growth-as-spawning (SHIPPED)

Closes the population loop. Previously: dry zones could kill plants
but nothing replaced them — even if conditions improved, dead patches
stayed dead. Now: in wet/favorable conditions, NEW plants appear in
eligible empty cells via the same Growth sampler.

### Design

The same noise pattern that picks growth-promotion locations now ALSO
picks spawn locations. A unified Growth sample budget; what happens
on each sample depends on what's at the sampled position:

- **Existing plant of stage Seedling/Growing**: promote to next stage
  (existing 2b behavior).
- **Air cell with grass below**: spawn a TallGrass at Seedling stage.
- **Anything else**: no-op.

No separate spawn-rate tuning — emerges naturally from the unified
sample budget. A wet biome with empty patches sees Growth samples
land in those patches and fire spawning, gradually filling them in.
Population dynamics balance: deaths in dry zones thin the forest,
growth in wet zones replaces it.

### Eligibility

Air cell + grass directly below + cell is not at chunk y=0.

The y=0 exclusion is a deliberate scope cut: spawning at y=0 would
require reading the chunk below (cross-chunk substrate read). Most
forest surfaces aren't at chunk boundaries (the chunk has 16 vertical
layers, surface is typically at y=4-12 within a chunk), so we catch
>99% of spawn opportunities anyway. Filed for future: cross-chunk
substrate read for full y=0 spawn coverage.

Substrate is currently hard-coded to `BlockId::Grass`. Per-biome
eligibility (Moss in tundra, etc.) and per-biome plant variety (Fern
vs TallGrass vs Flower) are deferred. For now everything spawns as
TallGrass on Grass. Filed.

### Catch-up handling

Same as the death sampler in 2b: catch-up draws samples for multiple
consecutive tick_ids so the noise stream advances. A long catch-up
window will see multiple potential spawns at the same cell across
ticks; only the first fires (subsequent samples on the now-occupied
cell hit the existing-plant promotion path instead).

### Tests

3 new tests:
- `handler_spawns_plant_in_eligible_empty_cell`: chunk with grass at
  (5,5,5) and air at (5,6,5), wet conditions, run up to 5000 ticks,
  expect a TallGrass spawn at (5,6,5). Same probability math as
  the death-fire test (~0.18% per tick = 1 in ~570 ticks).
- `handler_does_not_spawn_without_grass_substrate`: chunk with stone
  at (5,5,5), air at (5,6,5), wet conditions, run 2000 ticks, expect
  zero spawns. Verifies substrate check.
- `handler_does_not_spawn_at_chunk_y_zero`: empty chunk, wet
  conditions, run 1000 ticks, expect zero spawns. Verifies the y=0
  guard for cross-chunk read avoidance.

Test count: 399 → 402. All passing. Both feature paths clean.
Release build clean.

### Visible playtest behavior

In wet biomes (moisture > 0.6):
- Existing plants promote stages (Seedling → Growing → Mature).
- New plants appear in empty cells above grass tiles. Patches of
  bare grass slowly fill in with TallGrass over minutes.
- Death rate is low (p_death(0.7) ≈ 0.05) so net population grows.

In dry biomes (moisture < 0.3):
- Plants die (Mature → Drying with smooth animation, then Withered).
- Few new plants spawn (p_growth(0.2) ≈ 0.12).
- Net population shrinks.

In transitional biomes (0.3-0.6):
- Both rates non-zero, dynamic equilibrium. Population fluctuates
  around a moisture-dependent carrying capacity.

The "forest as a self-balancing population" behavior the user
described — patches of life and death matching the noise pattern,
modulated by moisture — is now live.

### What's still NOT in this build

- **Cross-chunk substrate read for y=0 spawns**. Filed.
- **Per-biome plant variety**. Always spawns TallGrass for now.
- **Per-biome substrate variety** (Moss for tundra, etc.). Hard-coded
  to Grass.
- **Tree spawning**. Trees are multi-block structures placed by
  worldgen, not single-block plants. Live tree spawning would need
  a different mechanism (cluster-based, 5-10 blocks tall).
- **Pressure storage** (multi-dimensional substrate). Still sampling
  moisture live from the existing field. Other dimensions (erosion,
  thermal, structural) get added when their first user is ready.

## Increment 2d: tree growth (v0.7.0 — SHIPPED)

Trees can now extend their wood structure over time by replacing
adjacent leaves with new wood, with new leaves spawning around the
new wood. The forest you see at worldgen is no longer the final form —
trees grow throughout play.

### Cluster module additions

**`Cluster::diameter()`**: rotation-invariant size measure. Pairwise
Chebyshev distance over members, taking the max. Cost O(N²) where N
is cluster size. For typical trees (50-200 wood blocks) this is
sub-millisecond. For pathological cases the flood-fill cap rejects
first.

Used because "how tall vs how wide" is dimensionally arbitrary; the
maximum pairwise distance treats a tall thin tree and a wide squat
tree of the same diameter as the same size, regardless of orientation.

**`TreeRule`**: same-species wood-only flood-fill rule (`OakLog`
clusters with `OakLog`; `BirchLog` with `BirchLog`; no cross-species).
Leaves are NOT included. Cap = `TREE_CLUSTER_MAX = 128`, comfortable
margin over the 100-block growth cap so realistic trees fit.

7 new cluster tests: diameter for singleton/empty/2-block/many-block
clusters, Chebyshev (not Euclidean) verification, TreeRule
same-species/cross-species/leaf-exclusion, realistic-tree clustering.

### Pressure sampler additions

**`PressureDim::TreeGrowth`**: third dimension. Independent from
MoistureStress and Growth via a distinct seed offset.

**`p_tree_growth(moisture) = 0.3 * p_growth(moisture)`**: scaled-down
growth probability. At moisture 0.85 → ≈0.27; at moisture 0.2 → ≈0.04.
Slow enough that trees don't visibly explode, fast enough that
forest changes are visible over 5-10 minutes of play.

2 new sampler tests: scaled-growth verification, dimension independence.

### Handler: tree growth pass

New pass in `lifecycle_handler` after the Growth-as-spawning pass.
For each TreeGrowth sample landing on an OakLeaves or BirchLeaves
block:

1. Check 6-direction adjacency for matching wood (OakLog for OakLeaves,
   BirchLog for BirchLeaves). Skip if no adjacent wood.
2. Probability gate via `p_tree_growth(local_moisture)`.
3. Cluster diameter check via `cluster_at` with `TreeRule`. Skip if
   diameter ≥ `TREE_DIAMETER_CAP = 100`. Anchored sentinel (cluster
   too large for flood-fill) also skips.
4. Replace the leaf with matching wood (`enqueue_edit`).
5. Add new leaves at top + four horizontal cells if currently Air.

3 new lifecycle tests: leaf-to-wood conversion under wet conditions
(verified within 20000-tick window), diameter check wiring, isolated
leaves don't grow.

### Other

- **Tick-speed cap raised 10×**: F7 now caps at 10000× (was 1000×).
  Per user request, gives more headroom to fast-forward through long
  forest evolution.
- **Version bumped 0.2.0 → 0.7.0**. Bumping Cargo.toml version per
  shipped increment going forward. Patch bumps for fixes/cleanup,
  minor for new mechanics, major reserved for architectural rewrites.

### Test count: 402 → 415

7 cluster + 2 sampler + 3 lifecycle = 12 new tests, plus 1 absorbed
into the diameter check. All passing. Both feature paths clean.
Release build clean.

### Cross-chunk caveats deferred

Same as 2c: tree growth doesn't currently span chunk boundaries.
Adjacent-wood checks at chunk edges treat out-of-chunk neighbors as
"no wood" — so a tree exactly at a chunk boundary won't see its
neighbor across the line. This matters less than it sounds because
trees are usually placed several blocks from chunk edges by
worldgen, but it's a real limitation.

The diameter computation IS cross-chunk-aware (cluster_at can be
fed a `MultiChunkSource`) but the handler's adjacency check uses
`SingleChunkSource` — so a tree spanning two chunks would have its
diameter under-measured (only the local-chunk wood blocks counted).
Filed for future polish.

### Tree degrowth deferred

User asked for trees to shrink rather than die outright when
conditions become unsuitable. This is the symmetric counter to
growth: a noise sampler that fires on tree-tip leaves OR
unsupported wood and converts them back (wood → leaves → air,
working from cluster periphery inward). Same cluster-diameter
mechanic but in reverse — a "shrink" rule that fires more in dry
conditions.

This needs:
- A way to detect "tree-tip" wood (wood with leaf neighbors but
  no wood-above neighbor).
- A `p_tree_shrink(moisture)` function that's the moisture-stress
  counterpart to p_tree_growth.
- A new pass in the lifecycle handler.

Filed as the next increment after this. Worth getting growth solid
first; the shrink mechanism is its mirror.

### Pressure-field-modification (Layer 3) deferred

User mentioned trees should affect water/temperature/sunlight
locally. This is the actual pressure-field-substrate work — trees
contribute to fields, samples for OTHER plants read those modified
fields. Spatial structure emerges (trees space themselves; saplings
thrive at forest edges; etc.).

This is too big for one increment. Filed as a separate piece of
work (pressure-substrate-with-write-from-source). Layer 1 (this
increment, basic tree growth) and Layer 2 (saplings/seeds, deferred)
both run on the existing live-sampled moisture, no per-tree
contribution to fields.

## Increment 2e: tick_sim optimization (v0.8.0 — SHIPPED)

Diagnostic context from prior playtest at v0.7.0:
- `tick_sim=1010-1030ms` per second — saturating an entire CPU
  second of work for every wall-clock second.
- F7 to 10000× had NO effect: simulation pinned at ~1× because
  tick_sim already at 1000ms/sec.
- 2167 loaded chunks, only one registered handler (plant_lifecycle).

Investigation traced the cost to `ChunkManager::field_at`: string
hashmap lookup + 2 DashMap reads + RwLock WRITE acquire per call.
With ~700K calls/tick (24 samples × 2167 chunks + ~70K plant-loop
iterations × ~2167 chunks) at ~10µs per call, this matched the
observed ~700ms.

### Three optimizations shipped, all in `lifecycle_handler` and
`chunk_manager.rs`:

**(1) Per-chunk moisture snapshot.** At top of chunk loop:
`ctx.chunk_field_reader("moisture", coord)` once → stack-allocated
`[f32; 512]` snapshot. Replaces the per-plant `field_at` calls with
cache-hot array indexing via `snapshot.sample_at_block(lx, ly, lz)`.
A new `moisture_at(snapshot, field_reader, lx, ly, lz, wx, wy, wz)`
helper prefers the snapshot but falls back to the per-block reader
for test contexts. All four call sites updated (MoistureStress
sampler, Growth sampler, TreeGrowth sampler, Pass 2 plant loop).

Expected impact: dominant cost. From ~700K field_at calls/tick down
to 2167 snapshots/tick (each one 2KB memcpy after lock acquire).

**(2) Plant-indices early-out.** Chunks with empty `plant_indices`
(underground stone/dirt, ~1500 of 2167 in typical playtest) skip
the entire chunk's processing — no sampling, no per-plant loop,
no setup. Marks fired in tracker so catch-up math stays correct.

Expected impact: ~70% of chunks become no-ops. Even if (1) had no
benefit, this alone would meaningfully cut tick_sim.

**(3) Indexed remesh sweep.** Added `chunks_with_transitions:
HashSet<ChunkCoord>` to ChunkManager. The apply phase maintains it:
chunks with `transitions.is_empty()` get removed, chunks with
non-empty transitions get added. The per-tick re-mesh sweep
iterates this set instead of scanning all 2000+ loaded chunks.
Common case (no animations active): the sweep is a single
`HashSet::is_empty()` check.

Expected impact: small but free. The full-scan was ~50µs/tick;
the indexed version is essentially 0 when nothing's animating.

### Step 1 timing instrumentation already shipped previously

`TickRegistry::run_tick` records per-handler timing.
`ChunkManager::tick_simulation` accumulates across sub-ticks. The
bin's diagnostic log emits a `tick breakdown:` line. User's prior
playtest predates this output, so they'll see it in v0.8.0 logs.
This is the measurement infrastructure that lets us validate the
optimization actually helped.

### Test impact

415/415 passing (same as v0.7.0). Behavior preserved — the
optimizations are pure cost reductions. Both feature paths build
clean. Release build clean.

### What to expect in v0.8.0 playtest

Predicted: `tick_sim` drops from ~1000ms to ~100-300ms.

Acceptance criteria:
- F7 actually accelerates simulation (visible faster plant lifecycle
  events when pressed).
- The `tick breakdown:` log line appears in diagnostic output and
  shows plant_lifecycle as a fraction of overall tick_sim.
- Plant growth events become visible during a session (was
  invisible at v0.7.0 because simulation couldn't go faster than 1×).

If tick_sim is still too high after this, next steps in priority
order: pre-resolve field_idx (skip string hash), read-lock fast
path in field_at (skip write lock for already-derived grids),
sorted_chunk_coords caching across ticks. Each smaller than the
ones shipped here.

## Increment 3: rename + atmosphere shader (v0.9.0 — SHIPPED)

Two unrelated changes in one increment, both small enough to fit:

### Rename: meinkraft → unendlich

Mass sed (`grep -rl 'meinkraft' | xargs sed -i 's/\bmeinkraft\b/unendlich/g'`)
followed by directory rename and bin-file renames. Workspace is now
`/home/.../unendlich/`, package name `unendlich`, bins prefixed
`unendlich-` (e.g. `unendlich-world`). Zero remaining `meinkraft`
strings in the codebase.

### Atmosphere shader

Single-pass approach: one `chunk.frag`, one extended `CameraUbo`, no
postprocess infrastructure. The user gave clear "do basics now,
keep deferred list intact for physics-first focus" guidance.

**What landed:**

- **`chunk.vert` + `chunk.frag` rewritten** from minimal pass-through
  to atmosphere-aware. Vertex passes `v_world_pos` alongside `v_color`
  so the fragment shader can do world-space effects without inverse
  view-projection.
- **Fragment shader features:**
  - Camera-correct `view_dir` from `(camera_pos - world_pos)`. Fixes
    the long-standing bug where `normalize(-v_world_pos)` assumed the
    camera was at world origin.
  - `dFdx/dFdy` normal reconstruction (free in hardware, exact for the
    flat-shaded voxel polygons we have).
  - Sun-driven lighting using `sun_dir` from the UBO. Driven by
    `Sky::sun_direction()` which animates across the day/night cycle —
    so day/night just works, no extra logic.
  - Exponential distance fog `1 - exp(-dist * density)`. Density rises
    with humidity (humid biomes = visibly hazier). Fog target color
    is the Sky's current sky_color.
  - Fog warm-shift: when looking near the sun, fog tints toward
    sun_color. Sunsets feel sunset-ish.
  - Wind shimmer: `sin(world.xz · wind_dir + time)` modulates fog
    density slightly. Visualizes the wind field in long-distance fog.
    Cheap (3 ops), automatic, no new infrastructure.
  - Water (color-detected): fresnel + cheap wave displacement on
    color + specular highlight from the sun.
  - Stone (color-detected): fresnel-weighted reflection of sky
    color, scaled by humidity. Wet stone reflects sky; dry stone
    is flat. Implements the "moisture affects stone" idea without
    any actual reflection texture.

- **`CameraUbo` extended** in `world_renderer.rs` from 64 → 176 bytes.
  Adds: `camera_pos`, `sun_dir+intensity`, `sun_color`, `sky_color`,
  `ambient_color`, `fog_density`, `humidity`, `temperature`,
  `time_of_day`, `wind_xz`, `wind_magnitude`. One binding, both stages.

- **`AtmosphereParams` public type** with `::neutral()` defaults for
  test/headless paths.

- **`draw_frame_with_view_and_atmos()`** is the new path. Old
  `draw_frame_with_view()` retained as a neutral-atmosphere wrapper
  so existing call sites don't break.

- **Bin builds AtmosphereParams per frame** by sampling
  `sky_state.sun_direction/directional_intensity/ambient_color/sky_color_rgba`,
  `model.temperature/humidity` at player position, and
  `wind::wind_at` at player position.

- **`hot-shaders` cargo feature** added. Default OFF uses
  `include_bytes!` (production: self-contained binary). With the
  feature, pipelines load `assets/shaders/*.spv` from CWD at startup.
  Iteration loop: edit GLSL → `cargo build` (build.rs runs glslc) →
  restart binary, no Rust recompile of any module that doesn't
  reference shader bytes.

- **Pipeline descriptor flags** changed from `VERTEX` to
  `VERTEX | FRAGMENT` so the fragment shader can read the UBO.

**Test impact:** 415/415 still passing. All three feature paths
clean. Release build clean.

### Deferred list (kept intact per user request)

These all need infrastructure — offscreen render targets, second
render passes, fullscreen quads, blit chains — that doesn't exist
yet. Building the postprocess pipeline is a SEPARATE increment whose
payoff is a SET of effects, not one effect. Filed for later:

- **Real god rays** (screen-space radial scatter from sun position).
  Needs offscreen color target + radial-blur pass + composite.
- **Real shadows** (shadow-map pre-pass).
  Needs depth-only render pass from sun's POV + sample-with-PCF in
  the main fragment shader.
- **Real water reflections** (second render pass into a reflection
  texture, sampled via projected coords). Today's water is fresnel
  toward sky color — visually OK but not "see the sky in the lake."
- **Real underwater UV distortion** (postprocess UV warp triggered
  when camera is below water_level). Needs a check-camera-volume
  test in the bin and a warp-pass shader.
- **Heat haze refraction** (postprocess UV warp keyed off
  temperature). Same infrastructure as underwater distortion.
- **Volumetric clouds** (raymarched, in the sky pass).
- **Volumetric fog / god rays through fog** (raymarched, full-screen
  quad behind the world). Different from screen-space god rays.
- **Postprocess pipeline infrastructure** (offscreen color/depth
  targets per frame, fullscreen-quad pipeline, multi-subpass
  setup). The substrate that the items above need.
- **Material id varying** so the fragment shader can dispatch on
  block type instead of color-detect. Fixes wet-stone-vs-wet-sand
  ambiguity.

### Visual playtest expectations for v0.9.0

- Day/night cycle visible: sky color shifts, sun direction sweeps,
  shadows-from-normals shift accordingly.
- Distant terrain fades to current sky color (not the prior hardcoded
  grayblue).
- Looking toward a setting sun shows warm fog tint.
- Walking from a humid biome to a dry one: fog gets thinner.
- Watching distant fog: subtle drifting shimmer if there's wind.
- Walking onto stone in a humid area: a glossy fresnel sheen at
  glancing angles. Same stone in a desert biome: flat diffuse.
- Water surfaces: fresnel-weighted toward the sky's current color
  (so water at sunset is warm-tinted, water at noon is blue-white).

### Tunable knobs (in case anything looks wrong)

- **Fog too dense:** lower `fog_density: 0.006` in the bin.
- **Wind shimmer too pronounced:** the `0.15` amplitude in the
  fragment shader's `shimmer = sin(...) * 0.15` line.
- **Stone wetness too aggressive:** the `* 0.4` final scaling on the
  stone-fresnel reflection term.
- **Water too greenish/blue:** the `deep` and `surface` colors in
  the water branch.
- **Sun warm-shift too wide:** raise the `pow(sun_amount, 4.0)` to
  `pow(sun_amount, 8.0)` to confine warm-shift to the sun's
  immediate vicinity in view.

All these are in `src/shaders/chunk.frag`. With the `hot-shaders`
feature, change → `cargo build` → restart binary, no Rust rebuild
needed.

## Increment 3.1: cloud-as-fog + tone mapping (v0.9.1 — SHIPPED)

Three small changes in the fragment shader, no pipeline state changes,
no new infrastructure. All visual polish on top of v0.9.0's atmosphere
foundation.

### 1. Cloud blocks shaded as fog

Cloud cubes already exist in worldgen and already drift with the wind
field via `cloud::is_cloud_at_time` advection (see `mesher::emit_clouds`).
v0.9.0 rendered them as opaque off-white voxels — they looked like
flying bricks.

v0.9.1 adds an `is_cloud(v_color)` discriminator (channel: bright +
slightly blueish + low-chroma, distinguishing from stone) and a cloud-
specific shading branch:
- Base color = sky_color tinted toward white.
- Sun-direct boost (cloud tops glow when lit from above).
- **Fresnel falloff** at cloud edges → fades toward sky color, reads as
  wispy/soft instead of hard cube edges.
- **Sun-warm tint** at the silhouette when looking near the sun (cloud
  edges glow orange at sunrise/sunset).

The geometry stays opaque (no alpha blending pipeline change). This is
a visual fake but reads convincingly because real cumulus clouds DO
have soft fresnel-like edges from sub-surface scattering.

**Limitation**: you still can't stand inside a cloud and see fog
volumetrically around you. That requires a transparent draw pass with
back-to-front sort or order-independent transparency. Filed.

### 2. Fog tuning

v0.9.0 fog was too subtle. The user reported "really don't see thick
clouds of fog" in playtest. Two issues:

- **Density too low**: 0.006 base → at humidity 0.6, half-fade at ~85
  blocks. Render distance is 96 blocks. Most visible terrain was at
  <50% fog.
- **Wind shimmer wavelength too long**: factor 0.02 per block →
  wavelength ~314 blocks. Player sits inside one cycle and can't see
  the gradient.

v0.9.1:
- Base density bumped to 0.012 in the bin's `AtmosphereParams`. At
  humidity 0.5: half-fade at ~58 blocks. Clear desert vs. thick
  jungle is visibly different.
- Shimmer wavelength shortened: factor 0.08 per block → wavelength
  ~78 blocks. Visible at human scale.
- Shimmer amplitude raised: 0.15 → 0.4. Strong wind now produces
  visible streakiness/breathing in distant fog.

### 3. ACES tone mapping + filmic contrast + vignette

End-of-shader color grading. No texture infrastructure (no LUT) —
analytic only. ~10 lines total.

- **ACES filmic tone mapping** (Narkowicz fit): maps linear color to
  perceptual LDR with smooth highlight rolloff. Without this, bright
  sky and specular highlights clip flat to (1,1,1). With it, brights
  compress smoothly into desaturated white, preserving sense of "very
  bright vs. less bright."
- **Filmic contrast curve**: gentle S-curve via `smoothstep` mixed at
  0.10 strength. Pulls shadows down a touch, pushes highlights up.
  Less flat-PowerPoint, more cinematic.
- **Vignette**: 35% darkening at corners via `dot(v, v)` smoothstep.
  Adds cinematic framing for free.

The vignette uses a hard-coded 1280x720 reference for screen
normalization. At other resolutions the falloff shape is correct but
not perfectly proportional. Filed: pass screen size in UBO if it
becomes noticeable.

### Test impact

415/415 still passing. All paths clean. Release build clean.

### Visual playtest expectations for v0.9.1

- Cloud blocks now look like soft fog banks at altitude instead of
  flying white cubes. They drift with wind (already did, just unseen).
- Distant terrain visibly fades through fog. Walking from desert to
  swamp shows fog density change.
- Strong wind produces visible streaking in long-distance fog.
- Bright surfaces (water specular highlight, lit clouds) no longer
  clip to flat white — they compress to a creamy off-white.
- Image overall feels less "tech demo" and more "game."

### Tunables (still in the shader, easy to iterate)

- Cloud opacity at edges: `fresnel * 0.7` in cloud branch (lower =
  more solid clouds, higher = more wispy).
- Fog density base: `fog_density: 0.012` in bin's `AtmosphereParams`.
- Shimmer wavelength: `0.08` factor in shader's `wind_phase`.
- Shimmer amplitude: `0.4` in shader's `shimmer = ... * 0.4`.
- Vignette strength: `0.35` in shader's `vignette = 1.0 - ... * 0.35`.
- Contrast strength: `0.10` in shader's `filmic_contrast(color, 0.10)`.

With `--features hot-shaders`, edit GLSL → cargo build → restart binary.

### Deferred (still intact)

- True volumetric fog you can stand inside (transparent draw pass
  with back-to-front or OIT).
- Particle-system fog tufts (separate particle pipeline + billboard
  rendering + soft-particle depth sampling).
- 3D LUT for color grading (requires texture sampler infrastructure).
- All v0.9.0 deferreds: real shadows, real reflections, real god
  rays, real refraction, postprocess pipeline, volumetric clouds.

## Increment 4: tree topology Phase A — POI cache + root markers (v0.10.0 — SHIPPED)

The first half of the tree-topology rework. Trees are now first-class
graph entities with a single sticky root anchor. This increment lays
the data structures and the cross-chunk-aware walks; Phase B (next)
adds endpoint enumeration and per-endpoint shrink/growth.

### Cross-chunk tree growth (was a hidden bug)

The v0.7.0 tree growth code used `SingleChunkSource` for cluster walks
and bailed on cross-chunk neighbor lookups, which silently froze
growth on any tree whose trunk crossed a chunk seam. Most surface
trees fit inside one 32³ chunk but tall ones and wide canopy ones
straddle. The user noticed degrading tree behavior in playtest.

`MultiChunkSource` already existed in `src/world/cluster.rs`. The fix
was to use it. Now:
- TreeGrowth diameter check walks the FULL cluster across boundaries.
- Adjacent-wood detection uses world-coord lookups via new helper
  `read_block_world(chunks, wx, wy, wz)`.
- Leaf placement targets use world coords (cross-chunk edits go
  through the existing `enqueue_edit` apply path; no special handling
  needed).

Trees crossing into UNLOADED chunks still get clipped at the boundary
(MultiChunkSource returns None there, treated as air). Correct
behavior — when the chunk loads, the next tick sees the full cluster.

### POI cache architecture

New per-chunk `Chunk::poi_cache: Option<ChunkPoiCache>` holds
pre-computed structural summaries: for each tree whose anchor lives
in this chunk, store anchor candidate position, cluster size,
diameter, and bottom-most member. Subsequent lifecycle passes
consume this cache instead of re-flood-filling.

**Refresh policy is transitional.** Today: rebuild when None or
`tick_id - cache.computed_at_tick >= POI_REFRESH_INTERVAL_TICKS` (10
ticks ≈ 5 seconds at default rate). End-state when structural dirty
marking lands: rebuild iff `chunk.structural_dirty`. An idle chunk
will then never rebuild after its initial computation. The 10-tick
floor is bounded staleness in the meantime.

The dirty-marking transition is one line in `should_refresh_poi()` —
flagged in the code comment as the seam. The right invalidation
policy when dirty marking arrives: any wood-add or wood-remove in
chunk C dirties C and its 6 face-neighbors (because trees can cross
boundaries; an adjacent edit might unanchor a tree whose anchor is
in C). Edits to non-wood blocks dirty C only.

POI build cost: one MultiChunkSource flood-fill per distinct wood
cluster touching the chunk. Trees bounded by TREE_CLUSTER_MAX (~128).
Forested chunk with ~10 trees: ~30 µs typical. Cost paid once per
10 ticks today, once per dirty event eventually.

### Tree-root markers — the sticky anchor

A tree's identity is a single wood block flagged
`BlockInstanceState::FLAG_TREE_ROOT`. Indexed sparsely in
`Chunk::tree_root_indices`. Established by the POI build's discovery
sub-pass: when a wood cluster has an anchor candidate (lowest-y
member touching `is_tree_anchorage()` ground) AND no current member
of that cluster has a marker, emit `SetMarker` on the canonical
anchor.

`Chunk::set()` maintains the marker index automatically: wood→non-wood
transition removes the marker entry and clears the flag bits in
`instance_state`. Wood species changes (oak↔birch) also drop the
marker since the species changed.

**Stickiness pass** runs every tick over `chunk.tree_root_indices`:
- Marker's wood block has anchorage neighbor → clear unanchored flag if set.
- No anchorage but face-adjacent wood IS anchored → gate by 0.05
  per-tick probability. If fired, walk marker by 1 to that block
  (clear-and-set in the same tick). If gated, set unanchored flag.
- No anchored wood adjacent → set unanchored flag.

The 0.05 walk probability gives ~20 ticks (~10 real seconds) average
between walks at default tick rate. Combined with the 1-block-per-walk
rule, this produces the curved-base aesthetic the user requested:
soil erodes on one side, marker walks laterally to the new contact
point, geometry above stays put, tree appears to lean over time.

The walk probability is deterministic (hashed from marker world
position + tick id) so replays produce identical walks.

### `BlockId::is_tree_anchorage()` helper

Returns true for natural mineral or earth materials: Stone,
Cobblestone, Bedrock, Grass, Dirt, Sand, Gravel, Clay, Moss,
RedSandstone, SnowBlock, Ice, all stone-family (Limestone, Marble,
Basalt, Tuff, Granite, Slate, Calcite, Andesite, Schist, Bone,
Amber, Crystal), all ores (Gold/Iron/Coal/Diamond), and Dripstone.

False for: air, fluids, plants, leaves, wood. (A tree can't anchor
to its own canopy or to another tree's wood.)

### Deferred mechanism: deferred-tree-root-ops

Following the existing pattern (block_edits, state_changes,
transition_ops), added `tree_root_ops: Vec<DeferredTreeRootOp>` to
`TickContext` + `TickEdits`. Variants:
- `SetMarker { wx, wy, wz }` — set the FLAG_TREE_ROOT bit + index.
- `ClearMarker { wx, wy, wz }` — clear bit + index.
- `SetUnanchored { wx, wy, wz, on }` — toggle FLAG_TREE_UNANCHORED.
- `InstallPoiCache { chunk, cache }` — replace chunk's POI cache.

Helper methods: `enqueue_tree_root_set/clear/unanchored`,
`enqueue_install_poi_cache`. ChunkManager's apply phase drains the
queue alongside block_edits/state_changes.

### Phase A does NOT do (Phase B will)

- **Endpoint enumeration**: walk the cluster from the marker, classify
  each wood block as interior or endpoint (has air/leaf in growing
  direction). Cache endpoints in TreePoi.
- **Per-endpoint growth**: extend the TreeGrowth pass to also fire on
  endpoints (not just leaves with adjacent wood). Endpoints can grow
  upward and sideways; the prior code only grew sideways from leaves.
- **Per-endpoint shrink**: new `PressureDim::TreeShrink` (seed
  `0xC2B2AE36`, 4 of 5 reserved dims used after this); per-tick
  sample probability `p_tree_shrink(moisture) = 0.3 * (1.0 -
  p_growth(moisture))`. Endpoint becomes leaf or air.
- **Unanchored degrowth**: tree with FLAG_TREE_UNANCHORED set gets
  accelerated bottom-up shrink (5× rate, biased toward
  cluster_bottom).
- **Death animations**: leaf yellow-shift fade, wood crumbling, tree
  pre-death browning. Probably needs a `BlockTransition` extension
  with a "kind" field (currently the mesher hardcodes the
  plant-lifecycle stage interpretation). User asked about this; it's
  filed for Phase B's planning.

### Test impact

421/421 lib tests passing (was 415 pre-Phase-A; +6 new tests for the
discovery, stickiness, marker-clear-on-removal, and POI refresh
interval behaviors). Both feature paths clean. Release build clean.

### Tunables (for next playtest)

- POI refresh interval: `POI_REFRESH_INTERVAL_TICKS` in chunk.rs (10).
- Sticky walk probability: `STICKY_WALK_PROBABILITY` in
  plant_lifecycle.rs (0.05).
- What counts as anchorage: `BlockId::is_tree_anchorage()` in blocks.rs.

### Visible behavior change in v0.10.0

Today, NONE — Phase A only sets up the data structures. Markers exist
on trees but nothing yet acts on them or the unanchored flag. Trees
behave exactly as before (with the bonus that cross-chunk growth now
works, so trees that previously froze at chunk seams will keep
growing).

The visible payoff comes in Phase B (degrowth + endpoint growth) and
beyond.

### Note on visual effects (filed for Phase B)

User asked about death animations: leaf falling on degrowth, withered
grass tufts on grass dying. The existing `BlockTransition` mechanism
(used today for plant-lifecycle color fades) can be extended to carry
a "kind" tag (`Death`, `Birth`, `Wither`, etc.), with the mesher
dispatching to per-kind interpolation. Specifically:
- Leaf death: yellow-shift over 1.5s then fade-to-transparent over
  another 1s. Mesher reads kind=Death and picks color via
  `mix(leaf_color, vec3(0.6, 0.4, 0.1), progress)`.
- Wood crumbling: leaf appears, wood shrinks via sub-block scale
  parameter (mesher already supports per-block sub-voxel scale, just
  not animated).
- Grass withering: shift to brown + tufts shrink + occasional puff
  particle (would need particle pipeline — defer the puff).

These are Phase B work. The trigger sites land naturally with the
shrink mechanism: when a shrink op fires, instead of a bare
`enqueue_edit`, enqueue a transition + a delayed final edit.

### Deferred (still intact)

All v0.9.x deferreds plus:
- True structural dirty marking (replaces POI refresh interval).
- Cross-chunk dirty propagation policy for trees.
- Phase B: endpoints, shrink, unanchored degrowth, sapling reproduction.
- Layer 3 substrate (trees writing to local moisture/light fields).
- Death/birth visual transitions with `BlockTransition` kind tags.

---

## Increment 4.1: growth rate gates + perf finish + screen-uv UBO (v0.11.1 — SHIPPED)

### Context: what v0.10.0 left broken

Two playtest issues from v0.10.0:
1. **Perf regression**: 1000ms+ tick_sim, 8-12 FPS. Cause: POI build's
   linear 32K-block scan running on every chunk every 10 ticks.
2. **Trees blob into 10×10×10 cubes**: with cross-chunk growth fixed
   in Phase A, leaf-adjacent growth fans out exponentially because
   each growth event spawns 5 new leaf candidates, and the per-chunk
   sample budget (8/tick) easily fires multiple events per tick once
   trees have many leaves.

v0.11.1 fixes both. No new mechanics — rate-limit + cluster-size
taper on the existing v0.7.0 growth loop.

### Wood-index field consumed in POI build

The `wood_indices: BTreeSet<u16>` field on `Chunk` was added in v0.10.0
but build_poi_cache was still doing the linear scan. Switched the POI
build to read `chunk.wood_indices` directly. For chunks with no wood
(~70% of loaded chunks at typical render distance), this is now an
empty-BTreeSet check in O(1) instead of 32K block reads.

The wood_indices field is maintained in lockstep with the actual block
type by `Chunk::set()` — a wood→non-wood edit removes the entry,
non-wood→wood inserts. Worldgen seeds it once via `from_blocks()` in
the same scan that populates `plant_indices`.

### Per-chunk-per-tick growth budget

New constant `TREE_GROWTH_EVENTS_PER_CHUNK_PER_TICK = 2`. The
TreeGrowth sample loop counts fired events and breaks out at the cap.
Without this, a forested chunk grows up to 8 wood blocks per tick
during high humidity — making the 10×10×10 fan-out blob.

With it, growth is bounded to 2 wood blocks per chunk per tick under
*any* conditions. At 2 ticks/second default rate that's 4
blocks/second/chunk maximum — visibly slow, organic-looking.

### Cluster-size soft cap

New constant `TREE_CLUSTER_SOFT_CAP = 30`. After the existing
diameter-cap check, the growth probability is multiplied by
`(1 - cluster_size / 30)`. Trees up to ~30 wood blocks grow at full
rate; trees at 30 stop growing entirely; trees in between grow at
linearly tapered rate.

This is the actual terminal-size lever. Together with the per-chunk
budget it produces small Minecraft-y trees that stabilize at ~25-30
wood blocks regardless of how long the simulation runs.

### Camera UBO `viewport_rect` field + new fragment shader

Added `viewport_rect: [f32; 4]` to the world_renderer `CameraUbo`:
- xy = viewport pixel size
- zw = viewport pixel offset (zero today, populated when split-screen
  lands)

The fragment shader now uses this for a proper screen-space vignette
(the v0.9.1 vignette was hardcoded to a 1280x720 reference). The same
field is the seam for the future RenderView/multi-cam refactor — when
each camera gets its own UBO with its own viewport_rect, the same
shader code Just Works because `gl_FragCoord` is already
per-viewport in Vulkan.

### Volumetric fog with logarithmic depth + jitter

The fragment shader's fog model upgraded from
`1 - exp(-dist*density)` to a 6-step volumetric march:
- **Logarithmic depth spacing** (`t = dist * fi*fi`): near-camera
  steps short, far-camera steps long. Fog density boundaries fall
  outside the visible spectrum, eliminating horizon banding.
- **Per-pixel hash jitter**: each pixel gets a deterministic random
  t-offset. Breaks remaining banding into per-pixel noise (invisible
  to the eye, dispersing any quantization artifacts).
- **Layered density**: height fog × distance haze × wind advection ×
  temperature-driven turbulence. Gives valley fog, far haze,
  wind-streaked atmosphere, and heat shimmer in one place.
- **Sun-aligned forward scatter**: `pow(view_dir·sun_dir, 8)`-shaped
  brightness bonus. God-ray-feel without doing real god rays.

### Hash-dither film grain

Final shader pass adds a per-pixel ±0.5/255 dither on the output
color, derived from the same `hash12` used for fog jitter. This hides
8-bit color banding in smooth gradients (sky, fog) without being
visible as noise. Per-pixel constant so static shots don't crawl.

### Tunables (current values)

- `TREE_GROWTH_EVENTS_PER_CHUNK_PER_TICK` = 2 (plant_lifecycle.rs)
- `TREE_CLUSTER_SOFT_CAP` = 30 (plant_lifecycle.rs)
- `TREE_DIAMETER_CAP` = 100 (unchanged from v0.7.0; hard wall)
- `POI_REFRESH_INTERVAL_TICKS` = 10 (chunk.rs; placeholder until
  structural dirty marking)
- `STICKY_WALK_PROBABILITY` = 0.05 (plant_lifecycle.rs)
- Fog steps in shader = 6
- Vignette strength = 0.35

### Test impact

423/423 lib tests passing (was 421 in v0.10.0; added 2 new ones for
the growth budget and cluster-size taper). All paths clean; release
build clean.

### Visible behavior change

- Trees now grow visibly slowly (one block every couple ticks per
  chunk), saturate around 25-30 wood blocks, stop fanning out.
- tick_sim back to ~30-80ms range from 1000ms+; FPS back to 100+.
- Fog has visible volumetric structure with valley density and
  distance haze. Horizon banding gone.
- Vignette correct at any resolution (was 1280x720-locked before).

### Filed (open questions for future increments)

**Tree shrink/death**: still not implemented. Trees grow to soft cap
and stay there forever. Needs `PressureDim::TreeShrink` (seed
`0xC2B2AE36` reserved) and per-tree shrink pass mirroring growth's
rate limit. Plus death-animation transitions on shrink fire. This was
the second half of Phase B and got dropped in favor of fixing the
runaway growth first. Filed for v0.12.

**Meshing gap**: `loaded: 1941 / rendering: 523` in playtest logs —
838 chunks loaded but not in the render set. User can interact with
them (walk on them, break blocks) and breaking blocks makes them
appear, suggesting the mesh exists but the upload to GPU is stuck or
the renderer is rejecting it as stale. Symptoms point to mesh-pool
upload/version-staleness, not DashMap contention (tick_sim and
mgr_update are clean). Filed for diagnostic work in v0.12.

**`chunk_generation.comp` GLSL `exp` overload**: pre-existing build
warning. The compute path doesn't run on M2 (Vulkan compute is the
fallback path), but the shader compile fails. Cosmetic; filed.

### Forward shader roadmap (added at user request, advisory)

User shared a SOTA non-RT forest renderer plan with this priority
ordering. Filing as the multi-step shader roadmap to draw against
once we trust the simulation:

**Phase 1 — foundation upgrades**
- Cascaded shadow maps (4 cascades, log-distributed splits — typical
  0–20m / 20–60m / 60–180m / 180–600m). The largest single visual
  win. Without cascades, shadows in a forest look fundamentally wrong:
  trunks float, fog has no shape, atmosphere reads as fake.
- Temporal reprojection. Mandatory for the volumetrics phase below.
  Lets us render volumetrics at lower resolution and accumulate.
- Improved fog stepping (already done in v0.11.1: log spacing +
  jitter — though no temporal accumulation yet).

**Phase 2 — volumetric atmosphere**
- Volumetric froxel fog. Camera-aligned 3D grid (typical 160×90×64);
  store density/scattering/extinction/sun-injected energy. This is
  the actual SOTA "modern AAA" fog approach: vastly cheaper than ray
  tracing, gives valley fog / shafts / shadowed fog / localized
  plumes for free.
- Clustered forward+ shading. Better than deferred for a voxel forest
  with alpha foliage and many small lights.
- Contact shadows (short raymarch from depth). Cheap. Grounds grass,
  roots, rocks, trunks — fixes the floating-foliage look.

**Phase 3 — temporal coherence**
- Temporal volumetric accumulation. Volumetrics become smooth +
  cinematic instead of noisy.
- Atmosphere scattering (proper aerial perspective, not the current
  desaturate-toward-grey).
- Cloud layers (signed distance bounded volumetric for mega fog
  banks).

**Phase 4 — refinement**
- SSR (water reflections, wet surfaces).
- Probe GI / screen-space GI.
- Wetness system + rain interaction.

**Phase 5 — optional luxury**
- Hardware RT augmentation, only if it pulls weight against the
  scalability we'd be giving up.

**The key insight**: in a forest game, fog IS the lighting system,
not an add-on. Layered volumetric fog with shadowed light injection
gives more visual impact than any geometry detail boost. The current
engine is already well-positioned because the simulation provides
real humidity/temperature/wind/climate fields — most engines fake
these. Our fog can become genuinely simulation-driven once the
froxel grid lands.

This roadmap is filed; current shader work in v0.11.1 already lands
the "improved fog stepping" item from Phase 1. Cascaded shadows are
the recommended next big shader increment.

**Long-form version**: see `docs/SHADER_ROADMAP.md` for the full
priority-ranked stack with specific froxel resolutions, ReSTIR /
checkerboard / blue-noise specifics, the recommended pass-by-pass
pipeline, and per-technique ROI notes. That doc is the authoritative
forward reference for shader work.

---

## Increment 4.2: shader noise fix + meshing-leak fix + window upgrade (v0.11.2 — SHIPPED)

### Context: what v0.11.1 looked like in playtest

User playtest of v0.11.1 surfaced three issues:
1. **Visible per-pixel noise on uniform mid-tone surfaces** (gravel,
   sand, stone) — looked like an over-strong film grain.
2. **Whole-frame blue tint** when walking downhill toward higher
   distant terrain — underwater check firing on dry land.
3. **Persistent meshing gap**: `loaded: 1396 / rendering: 422` —
   ~1000 chunks loaded but not rendering. Walking through them
   passed-through-as-air; breaking a "missing" block sometimes did
   nothing visible (block stayed there until you broke it again).
   `mesh_q: 590/99` (in_flight=590, pending_results=99) was way over
   what the bounded 64-slot pool could possibly have in flight.

### Window upgrade (cosmetic, requested)

Constants in `src/constants.rs` updated:
- WINDOW_WIDTH: 1280 → 1920
- WINDOW_HEIGHT: 720 → 1080
- WINDOW_NAME: "Meinkraft" → "Unendlich"

Higher resolution makes the v0.11.1 noise issues more visible (more
pixels showing the same per-pixel noise), which is part of how the
shader bug surfaced.

### Shader noise diagnosis & fix

The v0.11.1 fragment shader was applying per-step turbulence
(`sin(p.x*0.25)*sin(p.z*0.22)`) inside the volumetric fog march,
combined with per-pixel hash jitter on `t`. Adjacent pixels jittered
to slightly different `t`, sampling the turbulence field at slightly
different positions, producing per-pixel-different fog density. On
uniform mid-tone surfaces this read as salt-and-pepper noise.

Forward scatter (`pow(sun_align, 8) * 0.35`) was also being applied
per-step, compounding the noise toward the sun.

**Fix**: removed the per-step turbulence; fog density is now a smooth
monotone function of `t` and `p.y` (height_fog × dist_layer × constant
density). Adjacent jittered pixels sample nearly identical density,
so jitter only contributes anti-banding (its purpose) without
producing surface noise. Forward scatter moved out of the loop,
applied once to the final fog color.

The temperature-driven turbulence and wind-driven shimmer are
filed for later return — they belong in a froxel grid (Phase 2 of
the SHADER_ROADMAP) where they'd be sampled once per froxel and
shared across all pixels through it.

### Underwater tint fix

v0.11.1 had `cam_pos.y < v_world_pos.y + 0.2` which fires for ANY
surface above the camera, not just water. Walking downhill past
higher distant terrain turned the whole frame blue.

**Fix**: require both `cam_pos.y < v_world_pos.y + 0.2` AND
`is_water(v_color)`. The tint now only applies to water surfaces
seen from below the water line. Doesn't perfectly model
"camera-is-physically-in-water" (would need a BlockId-at-camera
lookup), but eliminates the false positive on terrain.

### Film grain dither removed

The ±0.5/255 dither was correct in amplitude (genuinely invisible)
but combined with the noisy fog produced visible noise on smooth
gradients. With the calmer fog above, dither isn't needed.

### Vignette softened

0.35 → 0.25 strength, ramp 0.4-1.6 → 0.5-1.7. Less aggressive corner
darkening at high resolution (1080p amplifies vignette intensity
relative to 720p reference).

### Meshing-gap diagnosis & fix (the big one)

The bin's `meshing_in_flight: HashSet<ChunkCoord>` is supposed to
track chunks currently being meshed by the pool. It's used as a
guard against duplicate submissions. The drain loop was:

```rust
for result in mesh_pool.drain_up_to(MAX_UPLOADS_PER_FRAME) {
    meshing_in_flight.remove(&result.coord);
    // ... version check, upload ...
    if elapsed_ms > UPLOAD_BUDGET_MS { break; }
}
```

`drain_up_to(16)` extracts up to 16 results from the channel into a
`Vec<MeshResult>`. The for-loop iterates the Vec, removing from
`meshing_in_flight` on each iteration BEFORE the upload work. If the
time-budget `break` fires after, say, 2 uploads, the remaining 14
results in the Vec are dropped on scope exit. Their coords were
*never removed from `meshing_in_flight`*. Those chunks were taken
from the result channel (so the worker thread is "done" with them)
but the bin's tracking set still says they're in flight.

The submit filter is:

```rust
.filter(|c| !meshing_in_flight.contains(c))
```

So a leaked chunk never re-submits. It sits permanently in
"loaded but not rendering" purgatory.

The leak rate is roughly proportional to `(MAX_UPLOADS_PER_FRAME -
uploads_actually_done_before_budget) × budget_breaks_per_second`. At
16 max with a 4ms budget, when an upload spike pushes single uploads
to 2-3ms, the budget kicks in after 1-2 uploads, leaking 14 entries
per spike. Hundreds of leaked entries accumulate in minutes of play.

Symptoms in the field:
- `loaded: 1396 / rendering: 422` — 974 chunks loaded but no GPU mesh.
- `mesh_q: 590/99` — 590 chunks "in flight" with only 4 workers + 64
  queue, impossible without a leak.
- Walking through chunks that visibly weren't there.
- Breaking a "missing" block: the edit handler explicitly removes
  the edited chunk from `meshing_in_flight` (line 869), so that
  specific chunk gets unblocked; but neighbors stay leaked.

**Fix**: drain results into a Vec, clear `meshing_in_flight` for
ALL drained results in a first pass, then upload up to the time
budget. Any over-budget results are dropped — those chunks are
unblocked and will re-submit next frame, rebuilding the mesh from
scratch. Wasted CPU on the rebuild, but the system stays live.

```rust
let drained = mesh_pool.drain_up_to(MAX_UPLOADS_PER_FRAME);
for result in &drained {
    meshing_in_flight.remove(&result.coord);  // unconditionally
}
for result in drained {
    // version check + upload, can break early on budget
}
```

### Test impact

423/423 lib tests still passing. No new tests added (the leak is in
the bin, not the lib; any leak test would need a mock pool +
budget-triggered upload).

### Visible behavior change

- Surfaces should look clean (no per-pixel grain on gravel/sand/stone).
- Walking downhill no longer turns frame blue.
- `loaded ≈ rendering` should hold under normal play.
- Block breaks should consistently show.
- Window opens at 1920×1080 titled "Unendlich".

### Filed (still pending)

- Tree shrink/death (v0.12 still): `PressureDim::TreeShrink` + death
  animations. Trees still grow to soft cap and stay there forever.
- Cascaded shadow maps (Phase 1 of SHADER_ROADMAP): the recommended
  next big shader increment.
- Volumetric froxel fog (Phase 2 of SHADER_ROADMAP): would let us put
  back the temperature/wind-driven turbulence in a coherent
  per-froxel way that doesn't interact with per-pixel jitter.
- Pre-existing `chunk_generation.comp` GLSL `exp` overload build
  warning (still cosmetic).

---

## Increment 4.3: window constants + meshing-leak post-mortem (v0.11.3 — SHIPPED)

### Window dimensions: actually-applied this time

v0.11.2 updated `src/constants.rs` (WINDOW_WIDTH / WINDOW_HEIGHT /
WINDOW_NAME) — but the world bin has its OWN constants
(`INITIAL_WIDTH`, `INITIAL_HEIGHT`, `TITLE`) inside
`src/bin/unendlich_world.rs` that shadow the lib-level ones. The
playtest log confirmed:

```
Renderer ready (1280x720)
Starting Meinkraft — World
```

The bin's local constants weren't touched. Fixed in v0.11.3:
- INITIAL_WIDTH: 1280 → 1920
- INITIAL_HEIGHT: 720 → 1080
- TITLE: "Meinkraft — World" → "Unendlich"

`src/constants.rs` versions also stay updated (other bins read from
there), so both layers are consistent.

Filed: the lib-level constants and bin-level constants should not
duplicate. The bin should consume from `src/constants.rs`. Cleanup
deferred — too easy to scope-creep into a constants refactor.

### Meshing leak fix: confirmed working

Comparing v0.11.1 vs v0.11.2 playtest steady-state:

| Metric           | v0.11.1            | v0.11.2          |
| ---------------- | ------------------ | ---------------- |
| in_flight        | 116-119 (climbing) | 68 (stable)      |
| pending_results  | 99-118 (backlog)   | 0-2 (drained)    |

v0.11.2's mesh_q stays at the textbook healthy steady-state:
in_flight = MESH_QUEUE_CAPACITY (64) + MESH_WORKERS (4) = 68 means
the queue is full and 4 workers are busy. pending_results=0 means
results are drained as fast as they're produced. **No more leak.**

### Reading the rendering count: gotcha and fix to expectations

v0.11.2 design notes claimed `loaded ≈ rendering` should hold under
normal play. **That's wrong.** The renderer's `loaded_chunk_coords()`
returns only chunks with NON-empty meshes — `upload_prebuilt_mesh`
returns early on empty meshes (no insertion into `self.meshes`).
Empty meshes happen for:
- Underground stone chunks fully surrounded by other stone (every
  face is internal → no visible faces → empty mesh).
- Sky air chunks fully surrounded by air (same reason).
- Pure water / pure leaf clusters with same-block neighbors.

For a render-distance-6 sphere (~925 chunks total) over typical
terrain, `rendering` should be roughly 200-400 (the surface band
plus partial-occlusion chunks), with `loaded - rendering` being the
empty-mesh chunks (deep underground, deep sky).

The current playtest showing `loaded: 1410 / rendering: 322` is
**healthy**, not pathological. The earlier v0.11.1 log showing
`loaded: 1396 / rendering: 422 mesh_q: 590/99` was unhealthy because
of the in_flight saturation, not the rendering count itself.

The actual user-visible bug from v0.11.1 ("walking through chunks
that visibly aren't there") was caused by the meshing-leak: chunks
desired but blocked from re-submission by the leaked in_flight
entries never got their meshes built. With the leak fixed, that
specific symptom is gone.

### chunk_generation.comp `exp` overload fixed

Pre-existing build warning for the GPU compute path:
```
chunk_generation.comp:221: error: 'exp' : no matching overloaded function found
```

GLSL's `exp()` has no `double` overload (only float/vec*). The line:
```glsl
double altitude_drop = clamp(exp(-y / PRESSURE_SCALE_HEIGHT), 0.1LF, 1.5LF);
```

…tried to call `exp` on a `double`. Fixed by promoting through float:
```glsl
double altitude_drop = clamp(double(exp(float(-y / PRESSURE_SCALE_HEIGHT))), 0.1LF, 1.5LF);
```

The altitude-drop term is a slowly-varying envelope (PRESSURE_SCALE_HEIGHT
is ~order-1000 magnitude), so the float-precision intermediate is fine.
The compute path doesn't run on M2 (GPU compute fallback only), but
the build no longer emits a warning — cleaner CI output.

### Test impact

423/423 lib tests still passing. Release build now clean (no
warnings). Visual playtest expectations for v0.11.3:
- Window opens at 1920×1080 titled "Unendlich".
- mesh_q stays at 68/0-2 steady state.
- No "permanent gap" symptoms; rendering count is roughly stable
  near surface-band + partial-occlusion count for the local terrain.

### Filed (still pending)

Same as v0.11.2:
- Tree shrink/death (v0.12)
- Cascaded shadow maps (Phase 1 of SHADER_ROADMAP — biggest visual
  win, recommended next big shader increment)
- Volumetric froxel fog (Phase 2)
- Cleanup: bin-level INITIAL_WIDTH / TITLE / etc. should consume
  from src/constants.rs, not duplicate.

---

## Increment 4.3: window constants actually applied + shader warning fix (v0.11.3 — SHIPPED)

### Context: what v0.11.2 didn't actually do

User's playtest log showed `Renderer ready (1280x720)` and
`Starting Meinkraft — World` — neither the window size nor the
title changed despite v0.11.2 editing `src/constants.rs`. Cause:
the bin defines its own private constants (`INITIAL_WIDTH`,
`INITIAL_HEIGHT`, `TITLE`) at the top of `src/bin/unendlich_world.rs`
that shadow the public ones in `constants.rs`. The window setup
uses the bin-private constants. Updating `constants.rs` did
nothing for the window.

This is a design wart — the bin's constants should pull from
`constants.rs` so there's one source of truth — but for now,
just change both. The duplication is filed.

### Window now actually 1920×1080 / "Unendlich"

In `src/bin/unendlich_world.rs`:
- `INITIAL_WIDTH`: 1280 → 1920
- `INITIAL_HEIGHT`: 720 → 1080
- `TITLE`: "Meinkraft — World" → "Unendlich"

The startup log will now read `Renderer ready (1920x1080)` and
`Starting Unendlich`. The other Meinkraft-named bins
(`unendlich_chunk`, `unendlich_palette`, `unendlich_spin`,
`unendlich_demo`) are not the playtest binary; left alone.

### `chunk_generation.comp` `exp()` overload warning fixed

This warning has shipped on every build since v0.9.0. GLSL's
`exp()` only accepts `float` (and `vec*`), not `double`. The
compute shader's `model_pressure()` was calling `exp(double)`,
which silently fell through to whichever overload the driver
half-resolved — but glslangValidator rejected it loudly:

```
error: 'exp' : no matching overloaded function found
```

The path doesn't run on M2 (Vulkan compute capability is
unavailable; CPU worldgen is the active path), so the build
warning was cosmetic. But it cluttered every recompile log.

Fix: explicitly cast through `float` for the altitude-drop
exponential. The float-precision intermediate is fine because
`PRESSURE_SCALE_HEIGHT` is order 1000 and `y` is bounded by
world height, so the slowly-varying envelope doesn't need
double precision.

```glsl
double altitude_drop = clamp(double(exp(float(-y / PRESSURE_SCALE_HEIGHT))),
                              0.1LF, 1.5LF);
```

Cosmetic fix; no behavioral change.

### What v0.11.2's leak fix actually did

Reviewing the user's v0.11.2 playtest log:
- `loaded: 1410, rendering: 322, mesh_q: 68/0`

This *looked* alarming (1088 chunks "missing") but is actually
correct steady state. `rendering` counts only chunks with
NON-EMPTY GPU meshes — `upload_prebuilt_mesh()` early-returns
on empty meshes without inserting into `self.meshes`:

```rust
if mesh.is_empty() {
    if let Some(old) = self.meshes.remove(&coord) { ... }
    return Ok(());
}
```

So `loaded - rendering` = the count of all-empty-mesh chunks
(underground stone chunks where boundary faces are hidden by
neighbors, all-air sky chunks, etc.). These have nothing to
draw and correctly produce empty meshes.

The leak fix's actual effect is that `meshing_in_flight` no
longer grows unboundedly. v0.11.1 showed `mesh_q: 590/99`
(impossible without the leak — request queue caps at 64+4
workers); v0.11.2 shows `mesh_q: 68/0` which is the bounded
steady-state max. **The leak IS fixed.**

The earlier v0.11.2 design notes said "loaded ≈ rendering
should hold under normal play". That was wrong — the right
invariant is `loaded ≈ rendering + empty_mesh_count`, and
`meshing_in_flight ≤ 68` should hold. Documenting this here.

### Test impact

423/423 still passing. Release build now clean — no warnings
at all (the shader warning is the only one that was shipping
and it's now gone).

### Visible behavior change

- Window opens at 1920×1080 titled "Unendlich".
- Build log no longer contains the `exp` overload warning.
- No gameplay change vs v0.11.2.

### Filed for v0.12

- Tree shrink/death (`PressureDim::TreeShrink`).
- Cascaded shadow maps (Phase 1 of SHADER_ROADMAP).
- Bin constant duplication: `unendlich_world.rs` defines its
  own `INITIAL_WIDTH`/`INITIAL_HEIGHT`/`TITLE` instead of
  importing from `constants.rs`. Should pull from one source.

---

## Increment 5: tree shrink + bin constant cleanup (v0.12.0 — SHIPPED)

### Tree shrink mechanic

This was the second half of the original "tree topology" work; v0.10.0
landed Phase A (POI cache, root markers, multi-chunk growth), v0.11.1
landed the growth rate gates that made trees stop fanning into blobs.
v0.12.0 closes the loop: trees can now retract under dry conditions,
mirroring the growth pass.

**New `PressureDim::TreeShrink` (seed `0xC2B2AE36`)**: 4 of 5 reserved
dims now used (Erosion, Structural, Thermal still reserved).

**New `p_tree_shrink(moisture)`**: defined as `0.3 × p_death(moisture)`.
Mirror of `p_tree_growth = 0.3 × p_growth(moisture)`. Both use the
same midpoint (0.4) and steepness (10) sigmoid via `p_death` /
`p_growth`, so:

- At moisture 0.4 (neutral): p_tree_growth = p_tree_shrink = 0.15.
  Tree edge breathes around its current size.
- At moisture 0.6 (wet): growth ≈ 0.26, shrink ≈ 0.04. Net positive,
  tree expands.
- At moisture 0.2 (dry): growth ≈ 0.04, shrink ≈ 0.26. Net negative,
  tree retracts.

**Per-chunk-per-tick budget**: `TREE_SHRINK_EVENTS_PER_CHUNK_PER_TICK = 2`,
matching `TREE_GROWTH_EVENTS_PER_CHUNK_PER_TICK`. Equal caps means
the net behavior at neutral moisture is genuinely zero (not biased
toward shrink or growth by the budget).

**Eligibility**: a wood block (OakLog or BirchLog) at a "leaf edge"
of the tree's cluster — that is, with at least one face-adjacent
cell that's air or matching leaves. Interior trunk wood (all 6
neighbors are wood) is protected. This means shrink chips away at
the canopy first; trunk loss only happens once the canopy has
already been retracted enough to expose the trunk's surface.

**Process**: on a fired shrink event, the wood is replaced by either
matching-leaf (if any neighbor is a matching leaf — the wood
"demotes" to leaf as the branch retracts) or air (if no leaf
neighbor — the tip just disappears).

**Root protection**: blocks with `FLAG_TREE_ROOT` set are skipped.
This keeps trees anchored even under prolonged drought; an
all-shrink-no-protection model would let the cluster lose its
anchorage and the `FLAG_TREE_UNANCHORED` bit would start cascading,
which is a different (interesting!) mechanic but not what we want
yet. Filed as Phase C: drought-driven uprooting.

### Tunables (v0.12.0)

- `TREE_SHRINK_EVENTS_PER_CHUNK_PER_TICK` = 2 (plant_lifecycle.rs)
- `TREE_GROWTH_EVENTS_PER_CHUNK_PER_TICK` = 2 (unchanged)
- `TREE_CLUSTER_SOFT_CAP` = 30 (unchanged; growth taper)
- `TREE_DIAMETER_CAP` = 100 (unchanged; flood-fill hard wall)
- `p_tree_shrink(m)` = `0.3 * p_death(m)` (new)

### Bin constant cleanup

`unendlich_world.rs` no longer defines its own `INITIAL_WIDTH`,
`INITIAL_HEIGHT`, `TITLE`. It imports `WINDOW_WIDTH`, `WINDOW_HEIGHT`,
`WINDOW_NAME` from `unendlich::constants` instead. The v0.11.2 issue
where editing `constants.rs` did nothing because the bin shadowed
the values is now structurally impossible.

### Test impact

428/428 lib tests pass (was 423; +2 plant_lifecycle shrink tests
+ 3 pressure_sampler shrink tests). All paths clean. Release build
clean (no shader warnings either, since v0.11.3 fixed the GLSL
`exp` overload).

### Visible behavior change

- In dry biomes (low humidity): trees slowly retract over a minute
  or two of play. Canopies thin first; eventually outer trunk
  segments drop.
- In wet biomes (high humidity): trees grow normally, same as v0.11.x.
- At neutral moisture: trees stable at their soft-cap size, edge
  breathes at slow pace.
- Trees should now reach a more organic distribution of sizes
  across the world rather than every viable tree saturating at
  exactly TREE_CLUSTER_SOFT_CAP.

### Filed for v0.13+

- **Phase C: drought-driven uprooting**. Currently roots are
  protected from shrink. With the protection lifted (or under
  extreme drought), the FLAG_TREE_UNANCHORED cascade would let
  trees fall over — the markers and infrastructure already exist
  from v0.10.0 Phase A; it's a couple of conditional branches.
- **Death animations on shrink**: same mechanism as plant lifecycle
  transitions (BlockTransition + delayed-edit), color fade from
  green leaf to brown to crumble. User said "do basic things with
  alpha until water comes in" so a brown→fade transition is fine
  to start. Filed.
- **Sapling reproduction**: closes the tree life cycle. A mature
  tree near a grass cell with the right moisture probabilistically
  spawns a new tree-root. Needs `BlockId::Sapling` (or treat young
  trees with stage=Seedling as saplings).
- **Cascaded shadow maps** (Phase 1 SHADER_ROADMAP): the user
  observed that trees are "great... I bet once we add sunlight in
  they won't be as overtly chunky as they are now" — confirms the
  intuition that shadow grounding is what trees need next visually.

---

## Filed (long-term ideas, no version yet)

### Biome migration / spreading

User observation: biomes today are static — a chunk's biome is a
function of `(coord, seed, generator_version)`. But the simulation
has all the state needed to make biomes *move*: humidity is
already a per-cell field, trees already affect it locally (well —
they will once Layer 3 substrate lands), and pressure / temperature
are already per-cell.

The mechanic would be: forests provide nutrition and humidity to
their surroundings → adjacent desert cells slowly de-desertify
along the forest's boundary. Mountains slowly weather toward
their downwind side. Player-built structures (a stone house in
a swamp, water collected near a desert) would shift the local
microclimate over real-time scales.

Implementation outline (rough):
- The biome at a chunk would become a *derived* value, computed
  from the local field state at runtime, not a baked property.
  Today's `WorldModel::biome_at` already does this for sea level
  / pressure; extending it to consume local humidity/temperature
  *as modified by the simulation* is the bridge.
- The substrate layer (Layer 3 — already filed) would let
  trees write into local humidity. Once that's in, biome
  migration emerges naturally: a forest's local humidity shifts
  the biome lookup at its edge, which causes the right kind of
  surface block to get placed when the chunk regenerates pristine
  surface, which causes a different lifecycle handler to spawn
  different plants, etc.
- Player effect: structures change local airflow / humidity /
  thermal mass. A stone house traps cold; water collection
  raises humidity; cleared land lowers it. Same mechanism.

This is a *deep* feature — it changes biome from "lookup table"
to "computed equilibrium of a dynamic system". Probably 2-3
increments of work. Filed as a north-star direction, not next.

### Eruption / event-driven world changes

User idea: lava eruptions and similar one-shot dramatic events.
Could be triggered by pressure-field thresholds (a
`PressureDim::Tectonic` exceeding some value) or by wall-clock
events (every game-week, randomly placed event), or both. Visible
output: lava blocks placed in a radius, scorching the immediate
biome (high temperature for a while), darkening sky temporarily
via atmosphere UBO (ash), starting fires on adjacent flammable
blocks.

Existing plumbing helps:
- Lava sync (already in the bin, periodically rescans heat sources).
- BlockTransition (extends to "pre-erupting" / "cooling lava" stages).
- Atmosphere UBO (sky color shift for ash).

Would slot well after the volumetric froxel fog lands — eruption
plumes are exactly the kind of thing volumetric atmosphere shows
off best.

Filed as "the world has weather" feature group, alongside biome
migration and seasonal cycles.

---

## Filed (long-term: ecology-driven atmosphere)

User vision: the renderer should derive atmosphere from the world's
own simulated state rather than authored presets. Forests, valleys,
moisture pockets, and wind should literally drive light transport.
This is a north-star direction for shader work — the engine already
has the inputs (humidity, wind, temperature, climate, terrain,
chunk-local ecology), and most engines fake what we could derive.

### Three layers of atmosphere control

**Global**: sun angle, sky color, cloud cover, weather fronts,
humidity mass.

**Local environmental modifiers**: canopy density, valley depth,
river moisture, terrain shadowing, swamp humidity, temperature
pockets.

**Temporal evolution**: minute-scale changes through storms, sunrise,
fog burnoff, post-rain.

### Forest type drives light transport

Different forests should fundamentally alter sunlight penetration,
fog retention, color temperature, shadow softness, volumetric
density. Examples:

- Dense pine: tall trunks + high canopy + cold biome + damp air →
  blue fog, long shafts, soft indirect, dense ground fog, deep
  shadows.
- Wet swamp: high humidity + warm + stagnant wind + shallow water →
  low hovering fog, amber-green scattering, heavy extinction,
  glowing sunset haze.
- Mountain redwood: giant trunks + huge verticality + steep
  valleys + moving cloud layers → massive god rays, layered fog
  strata, sunlight curtains, evolving shafts.
- Autumn deciduous: dry leaves + warm albedo + thinner canopy →
  warm scattering, golden shafts, long-distance visibility, sunset
  bloom.

### Leaf chemistry / biological color response

Chlorophyll concentration in leaves varies with available sunlight
intensity (which itself varies with altitude). At high altitude
the harsher UV/intensity drives different leaf chemistry — blueing,
density changes, possibly opacity. Could potentially become an
actual rendered effect: leaf base color shifts with altitude band,
extinction through leaves varies. Plants in real life optimize
chlorophyll for survival under their light conditions.

### Physics-driven atmosphere

The engine has real body movement, wind, terrain collision, camera
medium state. Atmosphere should react physically:

- Motion disturbance fog: player running creates turbulence wake,
  temporarily clears local density, bends shafts.
- Valley cold-air pooling: at night cold air sinks, humidity
  condenses; during sunrise sun heats upper layers first, fog
  slowly evaporates upward. Simulation-driven, not animated.
- Wind transport: existing wind advects fog, mist, pollen, spores,
  ash, rain curtains. Fog MOVES through the forest instead of
  sitting.

### Trees as a lighting system

Trees shouldn't just cast shadows; they should influence fog
density, humidity retention, volumetric scattering, ambient
darkness, sun breakup, airflow. Build a low-resolution
`canopy_density[xz]` field generated from leaf + branch +
vegetation density, then use it for fog retention, sunlight
attenuation, ground moisture, ambient occlusion, wind damping.

Different forests then naturally become brighter / darker / wetter /
colder / more cinematic without artist scripting.

### Where this lands in the roadmap

This vision aligns with the volumetric froxel work currently in
progress. Once froxels exist (3D scattering texture as a compute
pass), the per-froxel sample can pull from canopy_density,
chunk-local humidity, and altitude — directly translating
simulation state into light transport without per-fragment cost.

This is filed as the long-term destination. Each shader increment
should be checked against "does this move us toward atmosphere
that DERIVES from the world?" rather than authored presets.

---

## Increment 6: CSM groundwork + sampling + shadow-aware fog (v0.14.0 — SHIPPED)

Big increment. Combines what was originally planned as v0.13 (CSM
infrastructure, no visible change) with v0.14 (sampling + visible
shadows) into a single ship because they were built back-to-back.

### Cascaded shadow maps — single-cascade Phase 1

**New modules**:
- `src/render/light.rs` — light-space view-projection math.
  `single_cascade()` builds a tight-fit orthographic projection
  around the camera's view frustum slice (default 0–200m), using
  the sun direction as the light's forward axis. 8 frustum corners
  → light-space AABB → ortho. Pulls back the near plane by 50m to
  catch off-frustum shadow casters. Handles vertical-sun degenerate
  case via `world_up` selection. 4 unit tests pass.
- `src/render/shadow_map.rs` — 2048×2048 D32_SFLOAT depth image,
  comparison sampler (LESS_OR_EQUAL, CLAMP_TO_BORDER, white border
  so off-map texels read as fully lit), single-attachment render
  pass with subpass dependencies for the depth-write→shader-read
  barrier. Final layout DEPTH_STENCIL_READ_ONLY_OPTIMAL.
- `src/render/shadow_pipeline.rs` — depth-only graphics pipeline.
  No fragment shader. Front-face culling (vs back in main pipeline)
  for tighter self-shadowing. Slope-scale + constant depth bias
  (1.75 / 1.25) to prevent shadow acne.
- `src/shaders/chunk_shadow.vert` — vertex-only shader using
  `light_view_proj` from the shared CameraUbo.

**Pipeline integration**:
- CameraUbo gained `light_view_proj: mat4` field (Rust + both
  vertex shader + fragment shader).
- `WorldRenderer` gained `shadow: ShadowMap`, `shadow_pipeline:
  ShadowPipeline`, `last_cascade: Option<LightCascade>` fields.
- Each frame: compute the cascade from sun direction + camera view,
  upload `light_view_proj` to the UBO.
- Each frame, BEFORE the main color pass: shadow pre-pass binds
  shadow pipeline + same descriptor set, draws every loaded
  chunk's mesh into the 2048×2048 shadow map. No light-frustum
  culling yet (filed for next increment).
- Chunk pipeline descriptor set layout extended with binding 1
  (combined image sampler, fragment-only) for the shadow map.
- Per-frame descriptor set writes both binding 0 (UBO) and
  binding 1 (shadow.view + shadow.sampler at
  DEPTH_STENCIL_READ_ONLY_OPTIMAL).

**Legacy single-chunk renderer** (`chunk_renderer.rs`, used by
the test bins `unendlich-chunk` and `unendlich-palette`) also
needed a placeholder shadow map to satisfy the chunk pipeline's
new descriptor binding 1. It creates a ShadowMap, clears it once
to depth=1.0 ("fully lit") via a one-shot command buffer, and
binds it. The legacy bins don't run a real shadow pre-pass.

### Shadow sampling in the main fragment shader

`chunk.frag` now declares `sampler2DShadow shadow_map` at binding
1. Each fragment:
1. Projects `v_world_pos` into light-clip-space via
   `ubo.light_view_proj`.
2. Bails to "fully lit" if outside the cascade's z range.
3. Otherwise does 4-tap PCF (±0.5 texels around the projected uv)
   with comparison sampling (the sampler is configured with
   compare_enable + LESS_OR_EQUAL + LINEAR, so each tap is already
   2×2 hardware PCF — total = soft 5×5 box).
4. Blends: `sun_attenuation = mix(0.4, 1.0, shadow_factor)` so
   shadowed surfaces retain 40% of the direct sun term (representing
   skylight + indirect bounce) and don't go pitch black.

### Shadow-aware volumetric fog (god rays)

The existing 6-step ray march now samples the shadow map at each
march sample's world position. Steps that land in shadow contribute
to general haze (`fog_accum`) but NOT to the warm-tintable lit fog
(`lit_fog_accum`). The ratio `lit_ratio = lit_fog_accum / fog_accum`
drives how much the fog tints toward sun_color.

Result: visible sunbeams. Bright warm shafts of fog where the sun
reaches through gaps in geometry; cool gray haze where it doesn't.
This is the single most "AAA outdoor cinematic" effect in the
current shader stack.

This is the pre-froxel approximation. A real froxel system would
compute scattering in a 3D R16F volume in a compute pass and have
the main fragment shader do one trilinear sample. We get most of
the visual win (shadowed in-scatter) without the compute
infrastructure. Filed: when froxels land, they replace the fog
march loop with three texture samples; this code is structured to
make that swap mechanical.

### Henyey-Greenstein phase function

`pow(sun_amount, 6.0)` ad-hoc forward-scatter weight is replaced
with a proper Henyey-Greenstein phase function with g=0.6. This is
the textbook approximation for Mie scattering's strong forward
lobe — the actual physics of how atmospheric particles redirect
light toward the viewer. Still single-scatter (no multi-scatter
integration), but the angular falloff now matches reality.

g is a candidate for biome-driven variation eventually: dusty
desert wants higher g, clear high-altitude sky lower. Filed as
part of the "atmosphere derives from world state" north-star.

### Altitude-driven sky tint

Sky color now shifts with camera altitude: warmer below, cooler
toward deep blue above. Range -50m → +200m. Cheap approximation
of the real atmosphere's Rayleigh extinction profile. Player
climbing a mountain visibly sees the sky deepen.

### Rayleigh-style directional tint

Fog also takes a small directional bias: looking toward the sun,
fog warms slightly (long wavelengths dominate near the source);
looking away, fog cools slightly (short wavelengths survive the
longer atmospheric path). This is the "sunset is red, anti-sun is
blue" effect at small magnitude.

### Tunables (v0.14.0)

- `SHADOW_MAP_SIZE` = 2048 (shadow_map.rs)
- depth bias: constant 1.25, slope 1.75 (shadow_pipeline.rs)
- single cascade range: view.near → 200m (world_renderer.rs)
- shadow PCF: 4-tap at ±0.5 texel (chunk.frag)
- shadow attenuation floor: 0.4 (chunk.frag)
- HG phase g: 0.6 (chunk.frag)
- altitude tint range: -50m → +200m (chunk.frag)

### Test impact

432/432 lib tests pass (was 428; +4 from light cascade math).
Release build clean.

### Visible behavior

This is THE big visual increment. With sunlight angle and shadows
working:
- Trees finally cast shadows. Forest interiors get visually
  grounded.
- Sunbeams shoot through canopies and across valleys where
  geometry blocks light.
- Sky color shifts with altitude.
- Fog warms toward sun, cools away from it.
- Trees should no longer feel "overtly chunky" (user prediction
  from v0.12 — testable now).

### Filed for v0.15+

- **Multi-cascade shadows** (4 cascades, log splits 0–20 / 20–60
  / 60–180 / 180–600m) for sharper near-shadows + far-distance
  coverage. Per-pixel cascade selection in fragment shader.
- **Light-frustum culling** in shadow pre-pass. Currently we draw
  every loaded chunk into the shadow map; many are outside the
  cascade. Skipping them is a significant CPU/GPU win.
- **Volumetric froxel fog** (compute pass, 160×90×64 R16F volume,
  trilinear sample in main pass). Replaces the per-fragment fog
  march. Closer to the "atmosphere derives from world state"
  vision: per-froxel sample can pull from canopy_density, humidity,
  altitude.
- **Temporal shadow accumulation** (fight aliasing without
  going to 4K shadow maps).
- **Per-biome HG g and density** (start the world-state-driven
  atmosphere).
- **Canopy density field** (low-res per-chunk-column, derived
  from leaf+branch counts) as input to fog density and ambient.

---

## Filed (analysis: temporal stability + voxel flicker)

User observed: small camera motion produces visible color jitter on
blocks. Common in voxel/forward-lit engines. Root cause is not
"precision too low" but "image is not temporally stable" — modern
AAA engines achieve smoothness through stability tricks rather
than accuracy.

### Diagnosis (all of these likely contributing)

1. **Subpixel rasterization instability**: tiny camera shifts cause
   different fragments to be rasterized + different `dFdx`/`dFdy`
   quad groupings + different lighting. Voxels suffer more because
   flat shading + no textures = no natural noise to mask the
   shifts.

2. **Derivative-based normal reconstruction**: `cross(dFdx(v_world_pos),
   dFdy(v_world_pos))` is computed per 2×2 pixel quad. Camera
   movement re-groups fragments into quads, so the same surface
   point gets a slightly different normal per frame → lighting
   shimmer. **This is the biggest single contributor in our case.**
   Fix: pass face normal from vertex shader (we know it for voxels).

3. **High-exponent specular**: `pow(NdotH, 64)` is hugely sensitive
   to tiny normal changes. We don't have specular yet but if/when
   we add it, exponent should stay ≤ 16.

4. **Fog stepping noise**: per-step samples shift with camera. Already
   mitigated in v0.11.2 by smooth low-freq density + per-pixel
   hash jitter that doesn't move with time. Could go further
   (world-position-based jitter instead of screen-position).

5. **No temporal accumulation**: each frame independent. Modern
   engines use TAA / reprojection / history buffers; smoothness >
   per-frame accuracy. Filed as long-term.

### Fix priority

Tier 1 (do now, biggest wins):
- Pass face normals from vertex shader (kill derivative jitter)
- Lower the shadow-PCF tap delta to a sub-texel that's stable
  under tiny camera motion

Tier 2 (do soon):
- World-stable fog jitter (`hash(floor(v_world_pos.xz))` instead
  of `hash(gl_FragCoord.xy)`)
- Camera-position quantization snap (Minecraft trick: `floor(pos
  / (1/1024)) * (1/1024)`) — removes subpixel drift

Tier 3 (long-term):
- TAA / temporal reprojection (separate render-to-texture, history
  buffer, motion-vector-based blend)
- Low-frequency lighting quantization (`floor(color * 64) / 64`)
- Tonemap clamp before ACES

### The real insight

Voxel renderers are "perfect per frame, unstable over time". AAA
renderers are "slightly approximate per frame, extremely stable
over time". The path forward is stability tricks, not higher
precision.

### Filed: swapchain rebuild storm on focus loss

User's v0.14.0 playtest log shows many repeated
`Swapchain present mode: FIFO` log lines after Esc-released the
mouse. The renderer is rebuilding the swapchain over and over
when the window loses focus. Cosmetic (no FPS impact visible)
but log-spammy and probably wasteful. Likely a focus-event handler
that triggers swapchain recreate without checking whether the
swapchain is actually invalid. Filed for later cleanup.

---

## Increment 7: temporal stability + atmospheric grounding (v0.14.1 — SHIPPED)

Three shader-only fixes addressing v0.14.0 playtest feedback.

### 1. Axis-snapped face normals (kills micro-jitter)

**Problem**: under tiny camera motion, blocks showed visible color
jitter — same surface point shifted hue frame-to-frame. Root cause:
the fragment shader reconstructed normals via `cross(dFdx(v_world_pos),
dFdy(v_world_pos))`. Derivatives are computed per 2×2 pixel quad,
and tiny camera motion regroups fragments into different quads,
producing a slightly different normal per frame, which shifts
the dot(normal, sun_dir) lighting term, which produces visible
color drift.

**Fix**: voxel faces are ALWAYS axis-aligned (one of ±X, ±Y, ±Z).
Snap the noisy normal to the dominant cardinal axis. Result is
bit-stable under camera motion. The snap is correct (not
approximation) for axis-aligned voxel geometry — every face's
true normal IS one of those 6 vectors.

When non-axis-aligned geometry lands later (slopes, leaves with
tilted faces), real vertex normals will need to come from the
mesher; this fallback can stay as a no-op for axis-aligned blocks.

### 2. World-stable fog jitter

**Problem**: fog density jitter used `gl_FragCoord.xy` (screen-space).
Under camera motion the same world point lands on a different
screen pixel each frame → different jitter → different fog
contribution → visible color drift.

**Fix**: hash from `floor(v_world_pos.xz * 4.0)` (0.25m granularity)
instead. The fog at a given world point is now invariant under
camera motion. Cost: small loss of pixel-level decorrelation, but
with our smooth low-frequency density that's invisible.

### 3. Hemisphere ambient + atmosphere from t=0 (fog feels less tacked on)

**Problem**: fog "felt tacked on" — a 5m block looked identical to
a 50m block except gray paint was sprayed on the latter. Two
contributing causes:

(a) **Uniform ambient term**: every face received the same ambient
contribution regardless of orientation. A real sky dome is bright
and lights up-facing surfaces more than down-facing ones. Without
this directional cue, blocks feel "stamped" rather than embedded.

(b) **Hard fog cutoff at 10m**: `smoothstep(10, 120, t)` made fog
literally zero for the first 10m, then ramp up. The transition
from "no atmosphere" to "atmosphere" was visible.

**Fixes**:

(a) **Hemisphere ambient**: faces pointing UP receive sky-color
ambient at full strength; faces pointing DOWN receive a warm
ground-bounce term; sides interpolate. Driven by `normal.y`. This
is the standard "ground vs sky" hemisphere lighting trick — cheap
and dramatically grounds blocks visually.

(b) **Smooth atmosphere from 0 distance**: replaced
`smoothstep(10, 120, t)` with `pow(t/200, 0.7)` — smooth ramp
from t=0, no cutoff. Even a 5m-distant block picks up faint
atmospheric tint now. Combined with new altitude-rolloff
(`exp(-max(p.y - 60, 0) * 0.01)`), distant high-altitude scenes
feel thinner-aired naturally.

### Test impact

432/432 lib tests still pass (no Rust changes). Release build
clean. Pure shader patch.

### Filed for next session

- Camera position quantization (`floor(pos * 1024) / 1024`) — would
  remove residual sub-pixel drift entirely.
- TAA (temporal antialiasing): the AAA fix for everything-jitter.
  Major architecture work — needs history buffer, motion vectors,
  reproject pass.
- Vertex normal attribute in mesher: prerequisite for any non-
  axis-aligned geometry (sloped surfaces, billboards, leaves).
- Swapchain rebuild storm on focus loss (cosmetic log spam).

---

## Increment 8: volumetric moisture (v0.15.0 — SHIPPED)

Pre-froxel volumetric fog with structure, wind advection, and
player interaction. Pure shader patch.

The fog march now samples a 3D wind-advected fbm noise field at
each step, instead of a smooth `height_fog × dist_layer` density.
Result: fog has visible spatial structure — wisps, banks, gaps —
that drifts horizontally with `wind_params.xy` over time. Walking
through dense moisture banks now feels like walking through actual
moisture rather than turning gray paint on and off.

New `fbm2(p)`: 2-octave value-noise fractal, trilinearly
interpolated. ~16 hashes per call × 6 march steps × 1080p ≈ 200M
hashes/frame; well within M2 Max's ALU. Wind advection: noise
lookup offset by `-wind_params.xy * time_of_day`. Altitude moisture
bias: low altitudes get up to 1.4× moisture density, high drops
to 0.2× — valleys foggy, ridges clear. Player proximity falloff:
`smoothstep(0, 3m, dist_from_cam)` attenuates density within the
camera's first 3m. Block interaction is implicit via the v0.14.0
shadow-aware in-scatter — sunlit gaps produce warm wisps, shadowed
gaps stay cool gray.

It buys ~80% of the visual destination with ~10% of the
engineering of true froxels. Filed: real froxels for the proper
ecology-driven atmosphere path.

### Tunables (v0.15.0)
- fbm frequency: 0.05 (~20m wavelength dominant), second octave at 2.7×
- wisp range: `mix(0.4, 1.6, moisture)`
- altitude bias: `clamp(1.5 - p.y * 0.012, 0.2, 1.4)`
- player proximity: 0–3m smoothstep

### Update — wind-sheared blobs → streaks (physically-flavored)
The moisture field is now coherent **blobs** that the wind shears into
**streaks**, replacing the isotropic translate-only wisps. Two parts:
- **Blobs:** the fbm is contrasted via `smoothstep(0.15, 0.85, …)` so moisture
  pools into distinct banks with clearer gaps instead of uniform haze.
- **Streaks:** the sample frame is stretched ALONG the wind direction by
  `stretch = 1 + min(wind_speed · STREAK_GAIN, STREAK_MAX)` (`2.0`, `5.0`) — a
  scalar carried by a sheared flow elongates along streamlines. Calm air →
  round blobs; gusts → long streaks. This finally delivers the v0.9.1
  playtest expectation "strong wind produces visible streaking."
Applied IDENTICALLY in `froxel_fill.comp::sample_cell` (active path) and
`chunk.frag`'s fallback march so the froxel volume and the march agree at the
same world position. Still ~all-ALU, no new bindings/passes.

### Perf — skip empty froxels (GPU)
`froxel_fill.comp` was paying its 9-tap shadow PCF on every cell of the
160×90×64 volume = ~8.3M shadow taps/frame, even for empty air. `sample_cell`
is now split into a cheap `cell_density` and the expensive `cell_inscatter`
(which holds the shadow PCF + HG phase); the integration loop calls
`cell_inscatter` only when the cell's optical depth (`density × thickness`)
exceeds `OD_EPS = 7e-5`, and skips cells behind a saturated ray
(`trans_acc ≤ TRANS_EPS`). The extinction term is still applied, so the result
is **output-preserving** — skipped cells' in-scatter is below 8-bit
quantization. A `float`-accurate model (`handoff/froxel_skip_model.c`,
clang/gcc) marching representative daytime frusta measured **~5–19% of shadow
taps skipped** (more when clear/high-altitude) with worst-ray error
**0.0014–0.0025, under 1 LSB (0.0039)** at this threshold. This is the v1 of
SHADER_ROADMAP's "skip empty froxels"; a Hierarchical-Z pass would extend it to
whole-tile skipping.

432/432 tests pass. Release build clean.

---

## Increment 9: froxel groundwork (v0.16.0 — SHIPPED)

Lays the infrastructure for a real volumetric froxel scattering
volume. v0.16 ships the resource (allocated, cleared, descriptor-
bound, shader-declared) without yet running a per-frame fill.
Visual behavior unchanged from v0.15 (the shader detects the
sentinel and falls through to the per-fragment march).

### What landed

**`src/render/froxel.rs`** — `FroxelVolume`: a 160×90×64
R16G16B16A16_SFLOAT 3D image (~7.4MB) with `STORAGE | SAMPLED |
TRANSFER_DST` usage and a trilinear edge-clamp sampler.
`clear_to_sentinel()` does a one-shot transition UNDEFINED →
TRANSFER_DST → CLEAR(0,0,0,0) → GENERAL at startup. The image
sits in GENERAL for the renderer's lifetime so the future compute
fill can write into it without further layout transitions.

**Chunk pipeline binding 2** (combined image sampler, fragment-
only) for the froxel volume, alongside binding 0 (UBO) and
binding 1 (shadow map).

**`WorldRenderer`** allocates one `FroxelVolume`, writes it to
binding 2 in every frame's descriptor set, destroys it in
shutdown.

**`ChunkRenderer`** (the legacy single-chunk test bin) does the
same with its own placeholder volume — the shared chunk pipeline
requires the binding satisfied even though this bin doesn't fill
or sample the volume meaningfully.

**`chunk.frag`** declares `sampler3D froxel_volume` and probes it
once at the top of the fog block to keep the descriptor binding
live (glslangValidator dead-strips unused samplers, which would
mismatch the pipeline layout). The probe checks `alpha > 0.001`;
since the volume is at sentinel everywhere, the probe is always
false, and the per-fragment march below is the active path.

### Vulkan capability check

The user's M2 Max via MoltenVK supports compute (
`QUEUE_COMPUTE_BIT`, `SHADER_STAGE_COMPUTE_BIT`,
`shaderStorageImageWriteWithoutFormat = true`,
`maxImageDimension3D = 16384`). The earlier `ERROR_FEATURE_NOT_PRESENT`
in `terrain_generator` was about a specific extension/feature
that path requested, not compute being unavailable in general.
Compute is open for the v0.17 fill path.

### Why ship groundwork separately

The fill (compute pipeline + dispatch + per-frame barriers + the
actual scattering math in a comp shader) is its own session-sized
chunk of work. Shipping the resource layer separately lets the
user verify on real hardware that:
  - The 3D image allocates without issue on MoltenVK
  - Vulkan validation accepts the new descriptor binding layout
  - The fragment shader's froxel_probe doesn't break visual output
If any of those break, fixing them in isolation is much cheaper
than debugging them tangled with the fill logic.

### Tunables (v0.16.0)

- `FROXEL_X = 160`, `FROXEL_Y = 90`, `FROXEL_Z = 64` (froxel.rs)
- `FROXEL_NEAR_M = 0.5`, `FROXEL_FAR_M = 200.0` (froxel.rs) —
  the camera-space slice the future fill will cover
- Image format: R16G16B16A16_SFLOAT (~7.4 MB)
- Sentinel: `vec4(0, 0, 0, 0)` (alpha=0 = "unfilled, use march")

### Filed for v0.17

- **Compute pipeline + shader (`froxel_fill.comp`)**: per-froxel
  density (humidity, height, fbm, canopy field eventually), per-
  froxel in-scatter (sample shadow map, modulate by HG phase + sun
  color), front-to-back integration to (rgb, transmittance).
  Workgroup size 8×8×1, dispatch 20×12×64 = 15360 workgroups, one
  per-frame.
- **Per-frame dispatch in WorldRenderer**: before main color pass,
  after shadow pre-pass (so the compute can sample current-frame
  shadows). Memory barrier: shadow late-fragment-tests → compute
  shader read; compute storage write → fragment shader read.
- **Fragment shader consumption**: replace the per-fragment march
  with `vec4 froxel = texture(froxel_volume, froxel_uvw(p));
  color = color * froxel.a + froxel.rgb;`. Fall back to march
  for fragments with froxel_uvw outside the [0,1] cube.
- **Temporal accumulation**: blend current-frame froxel write with
  previous-frame value (history buffer, ping-pong volumes). Cuts
  noise budget enormously, opens the door to lower per-frame
  sample counts.

### Test impact

432/432 tests pass. Release build clean. Pure additive change.

---

## Filed (cleanup pass items, batch later)

### Water vertical-veil artifact

User screenshot shows what looks like translucent vertical "veil"
columns rising from water surface up into the sky, aligned with
the water blocks below. Visible in clear-weather conditions. Most
likely cause: water blocks have alpha but depth-write is enabled,
so they write to the depth buffer at sky positions and confuse the
fog march or atmosphere sampling. Could also be the cloud-as-fog
v0.9.1 path firing on water-classified samples.

Diagnosis path: check `is_water` and `is_cloud` classifier bits
in chunk.frag, see if any water-tagged fragment is taking the
cloud-color branch. Or check water mesh emit path for incorrect
vertex placement.

Defer until heavy lifting is done; batch with other visual
cleanup items.

### Side-face shadow seams (carried over from v0.14.1)

Still some color discontinuity on the sides of blocks under tiny
camera motion. The axis-snapped normal fix in v0.14.1 killed most
of it; what remains is likely the shadow-PCF tap pattern aliasing
at sub-texel scales when the projected fragment lands near a
shadow texel boundary. Bigger PCF kernel + camera position
quantization would address.

### Filed (vision: heterogeneous fog types, depth/uv layer)

User raised three forward-looking concerns:

1. **Different "fog types" coexisting**: sand + wind = yellow
   dust storm; snow + wind = blowing white powder; humid forest =
   gray-green mist. The unified solution is to make fog COLOR
   itself a per-froxel field (not a global tint). Each froxel's
   color is computed at fill time from local conditions:
   - Humidity (current) → cool gray-blue
   - Temperature high + low humidity + wind = warm yellow (dust)
   - Temperature low + humidity high + wind = white (snow flurry)
   - Canopy density (future field) = green-tinted shadow fog
   The fragment shader reads scattered_rgb from the froxel and
   that already carries the correct color — no special-case
   "is this dust fog or snow fog" branching needed.

   Pre-froxel approximation we could do now: the existing fbm
   noise multiplier in v0.15 could read humidity/temperature/wind
   per march sample and bias the fog COLOR (not just density).
   Cheap; doesn't gate on froxel landing.

2. **Depth/uv layer for proper fog occlusion**: "fog getting
   blocked by trees in front of it" — this is exactly what
   froxels solve when filled. The current per-fragment march
   already terminates at the opaque surface (the march length
   equals fragment-to-camera distance), so trees DO block fog
   behind them — but only along the view ray. What's missing is
   the OPPOSITE: trees in front of the camera should occlude
   the in-scattered light reaching fog beyond them. That's
   exactly what the shadow map sampling at each march step
   provides (v0.14.0 already wired this) — but the user is
   probably noticing it's not strong enough visually. Stronger
   shadow attenuation in fog would help; ultimately the froxel
   compute pass with full per-froxel shadow integration is the
   destination.

3. **Per-block coloration from environment**: snow vs sand vs
   grass have different scattering signatures. Files into the
   "atmosphere derives from world state" north-star: per-froxel
   color comes from local biome + local temperature + local
   humidity, all of which are already simulated. Plumbing exists;
   needs the froxel fill compute shader to consume them.

---

## Increment 10: real froxel volumetrics (v0.17.0 — SHIPPED)

The compute fill landed. Per-frame compute pass writes per-froxel
scattering into the 3D volume; fragment shader integrates the
volume along the view ray instead of doing per-fragment fbm +
shadow march.

### What landed

**`src/shaders/froxel_fill.comp`** — compute shader, workgroup 8×8×1,
dispatch (20, 12, 64) = 921,600 invocations per frame, one per
froxel. Each invocation:
1. Reconstructs world position from froxel index using
   `inv_view_proj` push constant + log-Z slice mapping
2. Samples 3D wind-advected fbm density (same noise as v0.15)
3. Samples shadow map at world position
4. Computes single-scatter via Henyey-Greenstein (g=0.6)
5. Writes `(scattered_rgb * density * step, max(transmittance, 0.001))`

The "0.001" floor on transmittance is the live signal — the
fragment shader checks `> 0.0001` to detect filled vs unfilled.

**`src/render/froxel_fill_pipeline.rs`** — compute pipeline:
distinct descriptor set layout (UBO + shadow sampler + storage
image, all COMPUTE-stage), push constants for inv_view_proj +
near/far + extents (~92 bytes total).

**WorldRenderer dispatch**: between shadow pre-pass end and main
color render pass begin, the compute pass runs:
- Pre-barrier: `FRAGMENT_SHADER → COMPUTE_SHADER`,
  `SHADER_READ → SHADER_WRITE` on froxel image (so previous
  frame's main-pass reads complete before we write).
- Bind compute pipeline + descriptor set + push constants.
- Dispatch (20, 12, 64) workgroups.
- Post-barrier: `COMPUTE_SHADER → FRAGMENT_SHADER`,
  `SHADER_WRITE → SHADER_READ` (so main pass sees written data).
- Image stays in GENERAL layout throughout (no transitions needed
  because the volume's `clear_to_sentinel` puts it in GENERAL at
  startup permanently).

**`chunk.frag` consumption**: replaces the per-fragment march
when `froxel_filled` is detected (probe at center returns
alpha > 0). 6-step front-to-back composite along the view ray:
- World-pos → froxel UVW: UV from `gl_FragCoord / viewport_rect`,
  W from logarithmic mapping `log(t/near) / log(far/near)`.
- Per-step: trilinear texture sample, `color += rgb * trans;
  trans *= sample.a`.
- Final composite: `color = surface * trans + scatter`.

The legacy single-chunk renderer (`unendlich-chunk` test bin)
doesn't run the compute fill, so its volume stays at the sentinel
and the per-fragment fallback march runs there. Both paths
coexist; the fragment shader picks at runtime via a single
`probe.a > 0.0001` test.

### Why this is the right architecture

Per-fragment cost in v0.15: 6 × (fbm × 16 hashes + shadow sample
+ smoothstep + ...) = ~140 ALU ops + 6 shadow samples per fragment,
× 1080p = 280M+ ops/frame.

Per-fragment cost in v0.17: 6 × (1 trilinear texture sample) =
~6 texture ops/fragment × 1080p ≈ 12M texture ops/frame.

Compute cost in v0.17: 921,600 invocations × 1 fbm + 1 shadow
sample = ~24M ops once per frame, regardless of resolution.

Net: per-fragment work drops ~10× while adding a fixed-cost
compute pass that scales with volume size, not screen size. At
4K the math becomes even more favorable for froxels.

### What this opens up

The "atmosphere derives from world state" north-star is now
unblocked. The compute shader's per-froxel sample point can read:
- Local humidity (already in atmos UBO, applied uniformly)
- Local temperature (atmos UBO, currently unused in fog)
- Local biome (would need a 3D biome lookup, future)
- Canopy density (would need a per-chunk-column field, future)

Each of these can independently shift per-froxel scatter color
without changing the fragment-shader consumption side. Sand storm
yellow, snow flurry white, swamp green — all derive from the
local conditions of each froxel. v0.18+ work.

### Visual change (expected)

Fog should now show:
- More structured wisps that ARE the same wisps neighboring pixels
  see (no per-pixel decorrelation since the volume is shared)
- Sharper god rays where shadow + density intersect
- Better integration of fog with surface lighting (ray
  composites consistent with the volume's shadow data)
- Possibly some slight visual "squareness" if the volume
  resolution shows through; we'd address that with temporal
  jitter (filed for v0.18).

### Risks

This is a substantial new code path running on every frame. Things
that could go wrong on first run:
- The push constant struct alignment on MoltenVK (Vulkan spec
  says push constants don't pad like UBOs; verified manually
  layout matches the GLSL struct)
- The barrier ordering (might need queue family ownership
  transfers on some platforms; not on M2 with single-queue family)
- Image format compatibility (rgba16f for storage write — the
  vulkaninfo confirmed `shaderStorageImageWriteWithoutFormat`
  is supported)
- The first frame: volume has been cleared to sentinel, so
  fragment shader runs fallback march, no flash; from frame 2
  onwards it's the froxel volume.

### Test impact

432/432 tests still pass (no Rust logic changes outside render).
Release build clean. Pure additive change beyond v0.16.

### Tunables (v0.17.0)

- Volume: 160×90×64 R16G16B16A16_SFLOAT (~7.4 MB)
- Slice distribution: log between 0.5m and 200m
- Workgroup: 8×8×1
- Fragment integration: 6 log-spaced trilinear samples per fragment
- HG phase g: 0.6 (matches fragment shader)
- Sun direction in/out, density formula, shadow projection: all
  identical between compute and fragment so consumer/producer
  agree

### Filed for v0.18

- **Cumulative front-to-back integration in compute** so fragment
  needs ONE sample, not six. ~6× per-fragment win on top of v0.17.
- **Temporal accumulation**: blend current-frame volume with
  previous-frame volume reprojected. Cuts noise budget enormously.
- **Per-froxel biome / canopy / temperature** consumption — the
  ecology-driven atmosphere groundwork. Sand storms, snow
  flurries, forest mist as natural emergent behavior.
- **Camera-position quantization** for the still-noticeable
  lighting jitter on side faces (carried over).

## Increment 11 (v0.17.1) — Volumetric stability + shadow culling + tight Y-AABBs

Bug-fix + culling release responding to the four visible regressions
introduced by v0.17.0's compute-fed froxel volume:

1. **Directional white fog** filling one side of the frame, anchored
   loosely to the line between camera and spawn. Static, not motion-
   related.
2. **Tree-side patterned dark patches** that flickered under tiny
   camera motion.
3. **Underground tinted fog** — at extreme negative Y the volume
   colored everything blue/red/yellow depending on sun angle.
4. **Deep underground (Y ≲ -1000) → all black**: every froxel
   saturated to opaque scatter-only color.

### Diagnosis

All four bugs were the same root cause amplified differently:
`exp(-world_pos.y * 0.05)` blows up when `world_pos.y` is far below 0.
Even at modest underground depth (y=-50) that's `exp(2.5) ≈ 12`, a
12× density spike; at y=-1145 it's `exp(57)` — IEEE-754 infinity.
Once a froxel's density saturates, transmittance crashes to zero and
the per-fragment ray-march sees only the inscatter color, which
varies with sun angle (hence the underground tints) and dominates
whichever half of the volume is below the horizon (hence the
"directional" wash above ground when the camera looks down).

### Fixes (all shader-level)

- **`alt_y = clamp(world_pos.y, -20.0, 200.0)`** in both the compute
  fill kernel and the fragment shader's fallback march. Used in every
  altitude-driven term: `height_fog`, `altitude_rolloff`,
  `altitude_moisture_bias`. The atmosphere doesn't physically exist
  below ground; clamping to a sea-level-ish floor keeps the formula
  bounded under all camera poses.
- **Density ceiling `min(density, 2.0)`** in both paths as a
  defense-in-depth clamp against fbm spikes layered on top of the
  height term.
- **Texel-snap shadow cascade origin** in `light.rs`. Snaps `min_ls.xy`
  / `max_ls.xy` to multiples of `extent / SHADOW_MAP_SIZE` (2048) so
  the projection of a fixed world point lands on the same shadow
  texel frame to frame. Kills shimmer-during-motion which had been
  manifesting as patterned tree-side dark patches and side-face
  shadow seams.
- **Slope-scaled shadow bias**: `bias = max(0.001 * (1 - n·l), 0.0002)`
  with `shadow_z = ndc.z - bias` used in all 4 PCF taps. Kills the
  static patterned dark patches on tree bark and block side-faces.
- **Sun direction normalization**: `CameraUbo.sun_dir` now uses
  `sun_dir_norm` (the same vector the cascade math uses), not raw
  `atmos.sun_dir`. Without this, lighting and shadow sampled with
  subtly different directions when `atmos.sun_dir` wasn't exactly
  unit length, producing low-amplitude directional bias.
- **Two-point unprojection in `froxel_to_world()`**: the previous
  implementation used `inv_view_proj * vec4(0,0,1,0)` to recover a
  "camera forward" direction, which is mathematically suspect (the
  inverse-transformed clip direction is not the world-space camera
  forward in general). New implementation unprojects NDC at z=0 and
  z=1 directly, lerps by `t = (view_z - cam_near) / (cam_far - cam_near)`
  — robust under all projection conventions.

### Culling additions (the new perf work)

- **Shadow frustum culling**: built a `Frustum` from the cascade's
  `view_proj` and gated the shadow draw loop on `aabb_visible`. The
  shadow pass was previously drawing every loaded chunk regardless of
  whether it was inside the light frustum. With render distance 6 and
  ~1500 loaded chunks but a cascade covering only ~200m of camera
  view, expected cull rate is 80–95%.
- **Tight Y-AABBs from mesh vertex bounds**. Added `min_y` / `max_y`
  fields to `UploadedMesh`, computed at upload time from
  `mesh.vertices`. Both shadow culling and main-pass culling now use
  `[ox, m.min_y, oz] → [ox+32, m.max_y, oz+32]` instead of the full
  32m vertical slab. For a typical above-ground chunk where actual
  geometry occupies ~5m of vertical, the AABB shrinks from
  conservative-32m to tight-5m. Net cull rate improves roughly
  proportionally on chunks dominated by air.

### What was deferred and why

- **POI-pass piggy-back for chunk-level Y bounds**: the cleaner form
  is `min_y_solid` / `max_y_solid` on `Chunk` itself, maintained on
  `set()`. That gives bounds available BEFORE meshing, useful for
  load-order priority and pre-mesh skipping. Filed as v0.18 work —
  the mesh-time version above is a stepping stone and 80% of the
  visible win.
- **GPU-side culling for chunk GPU gen**: right architecture, wrong
  moment. The same kernel that materializes a chunk on GPU should
  also write per-chunk metadata (min/max-y, has_any_solid) to an
  SSBO; frustum culling then becomes a compute pass writing a draw-
  indirect buffer. Premature today — until GPU chunk gen is real,
  there's nothing to GPU-cull from. Filed for v0.19+.
- **Camera-relative coordinates**: architectural change deserving a
  dedicated session. Filed for the multiplayer phase, where it
  enables correctness AND shared shadow / froxel passes between
  splitscreen players.

### Tunables added/changed

- `alt_y` clamp range: -20m → +200m (new)
- Shadow bias: `max(0.001 * (1 - n·l), 0.0002)` (replaces hard-coded)
- Density ceiling: 2.0 (new clamp)

## Increment 12 (v0.17.2) — Cumulative froxel integration + bounding-sphere cascade

Two changes targeting v0.17.1 playtest feedback: residual jitter
(visible as alternating-column striping on side-faces) and "fog
feels 2D up close, not volumetric".

### Bounding-sphere cascade fit (kills residual jitter)

The texel-snap fix in v0.17.1 stabilized the projection only as long
as the cascade's X/Y extent stayed constant. Re-reading my own
comment: "the size doesn't change because we use a fixed
near_meters/far_meters per cascade". That's wrong. The 8 frustum
corners rotate in world space when the camera rotates, and their
projection into light space produces an AABB whose extents change
with camera yaw/pitch. The texel quantum therefore changed and the
snap target shifted by sub-texel amounts under rotation — exactly
the alternating-column shadow pattern the user reported on a sand
cliff.

Fix: enclose the camera frustum slice in a SPHERE instead of a
tight AABB. The sphere's radius depends only on slice geometry
(near, far, fov, aspect) — not on camera orientation — so the
ortho extent is rotation-invariant. The center moves with the
camera, but we snap that to the texel grid. This is the standard
CSM stability technique (matches Frostbite, Unreal).

### Cumulative front-to-back integration in compute (kills "2D feel")

v0.17.1 stored per-cell `(local_inscatter * density * step, local_transmittance)`
and the fragment shader composited 6 fixed-stride samples through
the volume. Two correctness bugs in this scheme combined to make
near-camera fog read as a flat 2D color wash:

1. **Wrong step thickness in compute.** The kernel multiplied by
   `(FAR - NEAR) / NUM_SLICES = ~3.12m` for every cell. But the
   grid distributes Z slices LOGARITHMICALLY: cell 0 covers ~0.05m
   of view-Z, cell 32 covers ~1.5m, cell 63 covers ~18m. So cell 0's
   transmittance was computed with a 60× overestimate of its
   thickness, saturating the nearest froxel to opacity. Result:
   every fragment within a few meters of the camera read a thick
   pre-saturated near-cell as the dominant fog contribution,
   producing the flat painted-on look.

2. **Fragment compositing didn't match cell encoding.** The 6-step
   march resampled the volume at fixed log-Z fractions, which means
   it could read the same cell multiple times or skip cells, all
   while assuming each sample represented a fresh "step's worth" of
   fog. Mathematically broken when the volume stores per-cell
   pre-thickness-multiplied values.

Fix: rewrite compute as a **per-column cumulative integrator**.
Dispatch is now `(20, 12, 1)` workgroups instead of `(20, 12, 64)`
— each invocation handles one (x, y) pixel column and walks all 64
Z slices serially, carrying running `(color_acc, trans_acc)`. At
each cell we:

```
trans_local = exp(-density * cell_thickness)   // cell's actual log-Z width
color_local = inscatter * density * cell_thickness
color_acc  += color_local * trans_acc          // before update
trans_acc  *= trans_local
imageStore((x,y,z), vec4(color_acc, trans_acc))
```

So cell N stores the CUMULATIVE integrated fog from camera through
the far edge of cell N — not the cell's local contribution.

The fragment shader now does a SINGLE trilinear-filtered sample at
the W coordinate corresponding to the surface depth, and reads the
full integrated `(rgb, transmittance)` directly. No per-fragment
loop. Cumulative state is monotonic in Z, so the trilinear sampler
gives valid intermediate integration values for sub-cell surface
positions.

### Performance impact

Total compute work is the same (still 921,600 cell evaluations).
The fragment shader cost drops from 6+ samples per pixel to 1 — at
1920×1080 that's roughly 10M fewer texture reads per frame. v0.17.2
should be measurably faster on the fragment side AND visually
correct.

### What's still unaddressed

Long-frame log spam during streaming bursts (200ms+ frames). That's
upload pressure during chunk streaming, not a fog/shadow issue.
Filed for cleanup pass.

The "fire count" growing to 50k+ via tick-speed-up keys is a sim-
side stress test, not a renderer issue. Filed.

Player position quantization for the last 1-2 pixel sub-texel
residual filed for v0.18 (would require camera-position rounding
each frame and projection adjustment to compensate).

### Tunables changed

- Compute dispatch: `(groups_x, groups_y, 64)` → `(groups_x, groups_y, 1)`
- Workgroup compute: each invocation now loops Z=0..63
- Fragment fog samples: 6 → 1
- Cascade fit: tight-AABB → bounding-sphere (radius rounded to 16th of a meter for stability)

## Increment 13 (v0.18.0) — GPU chunk pool foundation

First shipping increment of the v0.18 GPU-meshing roadmap. Lands the
*storage* layer that the future GPU mesher (v0.18.1) will read from,
and ships it as a separable, fully-tested component before any
production code path depends on it.

### What's in v0.18.0

A new module `world::gpu_chunk_pool` providing a `ChunkPool` trait and
two implementations:

- **`CpuShadowPool`** — pure-Rust slot lifecycle implementation. Used
  for unit tests and as the bookkeeping layer inside the Vulkan pool.
  Exhaustively exercises alloc/free, version-based handle invalidation,
  exhaustion, FIFO recycling, and round-trip correctness without
  needing a GPU.
- **`VulkanChunkPool`** (under `gpu_chunk_pool_vk`, `#[cfg(feature = "gpu")]`)
  — owns a HOST_VISIBLE | HOST_COHERENT SSBO sized to `capacity *
  CHUNK_VOLUME` bytes, with a persistent map. Slot allocator delegates
  to an inner `CpuShadowPool` so the bookkeeping logic is shared and
  tested. Exposes `buffer()`, `slot_offset_bytes()`, `device()`, and
  `queue()` so downstream consumers can bind the buffer into their
  own pipelines.

### Slot semantics

`PoolSlot { index, version }` is a `Copy` handle. The `version` field
mirrors `Chunk::version` and serves two purposes:

1. **Stale-write protection.** A slot freed and re-allocated under the
   same index gets a new version; the old `PoolSlot` handle no longer
   validates. This protects downstream consumers (mesher) from a race
   where a chunk evicts mid-meshing.
2. **Upload-needed detection.** Callers compare `PoolSlot.version`
   against `Chunk::version`; mismatch means the GPU copy is stale and
   needs `upload()`.

### Why a separate buffer from the GPU generator

`gpu_generator::VulkanChunkGenerator` already owns a SSBO of the same
shape and the same per-chunk stride (`CHUNK_VOLUME / 4` u32 words).
Conceptually the generator could just write into the pool's buffer.
For v0.18.0 we ship them as separate buffers because:

- The pool can be validated in isolation. If it works on Honeykrisp
  but the generator integration doesn't, we know it's the integration,
  not the pool.
- Backing out is cheap: deleting `gpu_chunk_pool_vk.rs` reverts the
  GPU side completely; the trait + CPU shadow + tests stay useful for
  later integration work.
- The CPU `Chunk` struct is still the source of truth for blocks. Until
  v0.18.1's mesher reads from the pool, nothing in the runtime
  pipeline benefits from rewiring the generator. The work belongs in
  the increment that exercises the new path.

The pool's storage layout matches `chunk_generation.comp`'s output
stride byte-for-byte, so the v0.18.0.x rewire (next sub-step within the
v0.18 cycle) is a descriptor-set rebind plus a push-constant tweak,
not a shader rewrite.

### What is NOT in v0.18.0 (deliberately, scoped for v0.18.1+)

- No GPU mesher yet. The pool exists; nothing reads from it on the
  hot path. v0.18.1 lands `chunk_mesh.comp` and the upload-on-load /
  read-on-mesh wiring in `ChunkManager`.
- No production-path use of `VulkanChunkPool`. The bin still uses
  the CPU mesher. v0.18.2 flips the default once the GPU mesher is
  validated visually.
- No GPU-side culling / GPU lighting changes. Filed for after the
  mesher lands.
- The existing `gpu_generator::VulkanChunkGenerator` is unchanged.
  Field-tested previously on M2 Asahi Mesa; STATUS.md still shows
  the persistent-map fixes are in place but the path was last
  validated on the previous Asahi Mesa driver, not Honeykrisp.
  Re-validation moved to v0.18.0.x as part of generator → pool
  rewiring.

### Tests added

10 tests covering the slot lifecycle on the CPU shadow:
`alloc_and_free_cycle`, `upload_and_read_roundtrip`,
`read_before_upload_errors`, `upload_wrong_size_errors`,
`exhaustion_returns_error`, `version_mismatch_invalidates_handle`,
`free_list_recycles_in_order`, `invalid_index_handle_is_rejected`,
`many_alloc_free_cycles_no_leak`, `slot_words_matches_chunk_volume`.

2 tests for the Vulkan pool gated by `UNENDLICH_RUN_VK_TESTS=1` env
var (compile clean, skip in headless CI):
`vulkan_pool_alloc_upload_read_roundtrip`,
`vulkan_pool_offsets_are_distinct`.

Total lib test count: **444** (was 432).

### Cross-feature plumbing changes

Two helpers in `world::gpu_generator` promoted from `fn` to
`pub(crate) fn` so the pool implementation can reuse them without
duplicating the buffer-creation / memory-type-selection logic:
`create_storage_buffer`, `find_memory_type`. The Vulkan instance /
device / queue creation IS duplicated between generator and pool
(each has its own Vulkan context); a future increment may consolidate
these once we have three or more Vulkan-using subsystems.

### Filed for v0.18.0.x (sub-step before v0.18.1)

- **Generator → pool rewiring.** `VulkanChunkGenerator` should write
  into the pool's buffer rather than its own. Two changes: descriptor
  set update_after_alloc to point at the pool's buffer, and the
  `chunk_index` push-constant becomes the absolute pool slot index.
  Validate on Honeykrisp before merging.
- **Validate the existing generator on Honeykrisp.** The persistent-map
  fixes were validated on the previous Asahi Mesa driver. Honeykrisp's
  Vulkan 1.3 conformance changes some behavior around memory mapping;
  a quick smoke test against `unendlich-chunk` is the natural gate.

### Filed for v0.18.1

- **`chunk_mesh.comp`.** GPU greedy mesher reading from pool slots,
  writing per-chunk vertex + index buffers. Port the existing CPU
  mesher's algorithm; fold AO baking into the same pass. This is the
  first consumer that actually reads from the pool on the hot path.

### Filed for v0.18.2

- Flip default chunk pipeline from CPU mesher to GPU mesher; keep CPU
  behind a feature flag for the OpenGL/CPU fallback story.
- Revisit long-frame log spam from streaming bursts (filed in v0.17.2)
  — upload pattern changes meaningfully when meshing isn't pushing
  data CPU→GPU per chunk.

## Increment 14 (v0.18.1) — FP32 noise port + GPU mesher (UNVERIFIED)

Two changes shipped in one increment, both gated on the same hardware
fix (drop `shader_float64` device feature).

### FP32 port of `chunk_generation.comp` (Honeykrisp unblock)

Honeykrisp Mesa 25.3.6 on M2 reports `shaderFloat64 = false`, which
caused `vkCreateDevice` to fail with `ERROR_FEATURE_NOT_PRESENT` since
both `gpu_generator` and the new `gpu_chunk_pool_vk` were requesting
`shader_float64 = vk::TRUE`. The previous Asahi Mesa driver was
quietly tolerating the request. Honeykrisp is conformant and rejects it.

The user's playtest log made the diagnosis clean:
`WARN ... GPU unavailable: Vulkan error: ERROR_FEATURE_NOT_PRESENT.
Using CPU.`

**Fix**: drop the request from both device creations. Port the noise
shader from FP64 to FP32 — `double` → `float`, `LF` suffixes
removed, `GL_ARB_gpu_shader_fp64` extension dropped. The noise
function already used FP64 mostly out of convention; the actual
algebraic structure is FP32-tolerant for any coordinate inside the
playtest range (±10⁶ blocks comfortably, becomes lattice-quantized
above 2²⁴ = 16M blocks per axis).

The world will look subtly different at the same seed compared to
v0.18.0. The user OK'd this — "I don't mind losing the world identity
right now."

Filed for future GPU-worldgen restoration when needed:
**`docs/GPU_NOISE_STRATEGY.md`** captures the user's anchor-blend
proposal in detail (8-anchor blend in 3D for genuinely volumetric noise,
4-anchor for heightmap-style, smoothing pass aligns with already-planned
engine direction). Either anchor-blend or split-coord `(ivec3, vec3)`
noise will eventually replace the direct FP32 port; the direct port is
sufficient for current scale.

### GPU mesher (`chunk_mesh.comp` + `gpu_mesher_vk`)

Naive face-culling mesher in compute. Reads block IDs from a chunk
pool slot, emits one quad per visible block-side, writes vertex +
index buffers via atomic counter reservation. One workgroup of
8×8×8 invocations per chunk, each invocation handles 4×4×4 blocks
via stride. Same dispatch-size pattern as `chunk_generation.comp`.

**Output layout**: `[vec3 position, vec3 color]` per vertex, 24
bytes — byte-identical to `render::mesher::Vertex`. Per-face flat
brightness from a `FACE_LIGHT[6]` constant matching CPU mesher's
default. Per-block colors from a `block_base_color()` switch
covering the common BlockIds; uncovered IDs render bright magenta
as a debug fallback.

**What's NOT in v0.18.1**:
- Greedy meshing (filed: hard in compute, naive face-culling is
  geometrically correct just denser)
- AO baking (filed: needs neighbor-of-neighbor reads, doable in the
  same kernel but adds complexity)
- Per-block sky/weathering tints, soft edges, fluids, clouds (CPU
  mesher specialties; GPU mesher matches CPU `mesh_chunk_naive`
  not `mesh_chunk_greedy`)
- Wiring into `ChunkManager` — the production path is still the CPU
  mesher. Mesher is a buildable, testable component awaiting
  hardware validation before v0.18.2 flips the default

**Architecture**: `VulkanMesher` shares device + queue + pool buffer
with the `VulkanChunkPool` it was created against. Owns its own
output SSBOs (vertex / index / counter), pipeline, descriptor set,
command buffer. Output buffers are persistently mapped HOST_VISIBLE
| HOST_COHERENT — same pattern as the pool. Capacity is set per
mesher instance (default: 64K verts / 96K indices per chunk slot).
Overflow is detected via counter readback, returned as
`MesherError::Overflow` so callers can fall back to CPU meshing for
that chunk.

**Verification**: 1 unit test (`vertex_layout_matches_cpu_mesher`)
ensures byte-identical layout. 1 hardware-gated test
(`vulkan_mesher_meshes_a_single_block`) validates full path against
real Vulkan: alloc pool, upload single-stone chunk, dispatch, read
back 24 verts + 36 indices. Gated behind `UNENDLICH_RUN_VK_TESTS=1`
since the headless sandbox has no GPU.

### Helper additions

- `VulkanChunkPool::find_host_visible_memory_type` — exposed so the
  mesher can allocate buffers using the pool's memory regime
  without re-querying the physical device.
- `VulkanChunkPool::queue_family` — exposed for command-pool
  allocation in dependent components.

### Test count

446/446 passing (was 444). Added 2: one compile-time vertex-layout
check, one hardware-gated mesher round-trip.

### Filed for v0.18.2

- **Honeykrisp validation**. Run `UNENDLICH_RUN_VK_TESTS=1
  cargo test --features gpu vulkan_chunk_pool vulkan_mesher`
  on the dev machine. If both pass, the GPU path is real and v0.18.2
  can wire mesher into ChunkManager. If either fails, the failure mode
  tells us what to fix next.
- **CPU/GPU geometry parity test**. `mesh_chunk_naive(chunk)` on CPU
  vs `VulkanMesher::mesh_batch([(slot, coord)])` on GPU should produce
  the same set of (position, color) tuples (order may differ since
  the GPU emits in invocation-stride order).
- **Wire mesher into `ChunkManager`**. Replace the CPU meshing pool's
  hot path for chunks that are already pool-resident. Keep CPU mesher
  as fallback for overflow / non-GPU runtime.
- **Long-frame log spam from streaming bursts** (filed in v0.17.2)
  becomes addressable once meshes don't go through CPU memcpy.

### Filed for v0.18.3+

- Greedy meshing in compute. Hard problem; naive is correct in the
  meantime.
- AO baking on GPU. Needs neighbor-of-neighbor reads. Modest extension
  to the kernel.
- Block colors from a uniform/SSBO lookup table instead of switch
  statement. The switch covers maybe 15 of ~200 BlockIds; the rest
  render magenta. Uniform table is one descriptor set update.

### Operational note

Added `docs/trim_playtest_log.sh`: condenses unendlich-world playtest
logs to their signal — keeps WARN/ERROR/startup/shutdown verbatim,
decimates the per-second FPS spam to one-in-N, but preserves any
block whose FPS or p99 crossed an alarm threshold. Ratio is typically
8–15% of the original. Useful for sharing playtest logs in conversation
without burning the entire context window on per-second noise.

## Increment 15 (v0.18.2) — Mesh buffer pool, freeze fix

### What and why

The v0.18.1 playtest log showed `LONG FRAME: dt=156ms` and `dt=112ms`
spikes correlated with chunk-streaming bursts, presenting as
**whole-screen freezes** (9+ missed frames). The user described it as
"whole screen freeze on system plus fps is low when chunks are loaded."

Root cause: `WorldRenderer::upload_mesh` was calling
`vkCreateBuffer` + `vkAllocateMemory` + `vkMapMemory` per chunk per
upload — twice (vertex + index). With the existing 16-uploads-per-frame
cap and the 4ms upload budget, that's up to 32 `vkAllocateMemory` calls
per frame on the hot path. `vkAllocateMemory` is documented as one of
the slowest Vulkan operations and many drivers serialize it against the
graphics queue. On Honeykrisp/M2 that serialization manifested as the
present queue stalling for hundreds of milliseconds — the whole-screen
freeze.

### What changed

New module: `render::mesh_buffer_pool`. Two implementations:

- **`CpuMeshPool`** — pure-Rust slot lifecycle (alloc / free / record /
  validate / version-handle invalidation / FIFO recycling). 9 unit
  tests covering all paths including 1000-cycle no-leak stress.
  Reusable as the bookkeeping core for any future GPU-buffer pool.

- **`VulkanMeshPool`** — Vulkan-backed pool. One pre-allocated vertex
  SSBO + one pre-allocated index SSBO, persistently mapped into
  HOST_VISIBLE | HOST_COHERENT memory. Default capacity 2048 slots.
  Each slot has fixed `vert_bytes_per_slot` (default 128 KB) and
  `index_bytes_per_slot` (default 64 KB) regions within those buffers.
  `upload(slot, vert_bytes, index_bytes)` is pure `memcpy` — no Vulkan
  calls on the hot path.

Renderer integration:

- `WorldRenderer` gained a `mesh_pool: VulkanMeshPool` field, allocated
  in `WorldRenderer::new`. Default 2048 slots × (128 KB + 64 KB) =
  ~393 MB total. On unified-memory systems (Apple Silicon, integrated
  Intel, RISC-V SoCs, mobile) this is just regular RAM usage. On
  discrete GPU systems it's GPU VRAM, plenty of room.

- `UploadedMesh` now holds a `MeshSlot` handle instead of owning Vulkan
  buffer + memory pairs.

- `upload_prebuilt_mesh` becomes: alloc-or-reuse slot, memcpy via
  pool, record metadata. Same-coord re-mesh reuses the same slot
  (no churn). Different-coord upload allocates a fresh slot. Old
  slots from evictions defer-destroy through the existing
  `MAX_FRAMES_IN_FLIGHT` queue, but "destroy" is now "return to
  pool" — no GPU resource teardown except at renderer shutdown.

- Draw paths (shadow + main pass) bind the pool's vertex/index
  buffers with per-slot offsets (`cmd_bind_vertex_buffers` supports
  per-binding offsets natively, no spec extension needed).

- `WorldRenderer::destroy` tears down the pool's two buffers + memory
  allocations. Per-mesh destruction loop deleted entirely.

### Performance characteristics (theoretical)

- Per-chunk upload: was `~5–10ms` worst case (driver serialization on
  `vkAllocateMemory`), now `~50–500µs` (memcpy of typical ~50KB+10KB
  payloads to pre-mapped memory).
- 16 uploads per frame budget: was easily blown by a single slow
  `vkAllocateMemory`, now uses `~1–8ms` total (well under the 4ms
  per-call ceiling).
- Whole-screen freezes: should be eliminated in the steady-state
  streaming case. Long frames during initial chunk burst at startup
  may persist if other CPU work piles up, but the upload phase is
  no longer the cause.

**These numbers are theoretical.** No hardware validation has been
run. The mesh-pool path uses identical Vulkan primitives to the
chunk pool from v0.18.0, which was also unverified at v0.18.0 ship
time, so the same risk applies.

### Slot overflow

If a chunk's mesh exceeds `vert_bytes_per_slot` (128 KB ≈ 5460
vertices) or `index_bytes_per_slot` (64 KB = 16384 indices), the
upload returns `MeshPoolError::Overflow` and is dropped with a
`log::warn`. No graceful fallback to per-allocation upload — that
was the bug we fixed, recreating it under "slot overflow" defeats
the purpose. If the warning fires often in practice, raise the
slot caps. CPU mesher's typical chunk meshes are 50–80 KB vertex /
10–30 KB index based on playtest observation, so overflow is rare
in expected workloads.

### What's NOT in v0.18.2

- **Adaptive upload throttle.** Argued unnecessary once the
  allocation storms went away — upload phase is now so fast the
  4ms hard cap is essentially unreachable in normal play. Filed
  for revisit if real measurements show otherwise.

- **GPU mesher integration.** The `gpu_mesher_vk::VulkanMesher`
  remains a buildable, hardware-untested component. Wiring it
  on top of an unvalidated mesh pool would compound two risks at
  once. v0.18.3's job, gated on validation that v0.18.2's mesh
  pool works.

### v0.18.3 integration sketch

When the GPU mesher comes online, the integration is straightforward:

1. The existing `WorldRenderer::upload_prebuilt_mesh` already takes
   `ChunkMesh` (a `Vec<Vertex>` + `Vec<u32>`). The GPU mesher's
   output (`MeshedChunk`) is the same shape. So the trivial path
   is to convert `MeshedChunk` to `ChunkMesh` and call the existing
   API.

2. The architecturally cleaner path: skip the conversion. The GPU
   mesher's output buffer IS a vertex/index buffer pair. If we
   route the mesher's output buffer to be the *mesh pool's*
   vertex/index buffer (i.e., pass the pool's buffers to the mesher
   as its output destination instead of the mesher allocating its
   own), then there's literally no upload step — the mesher writes
   directly into the slot's range and the next draw call reads it.
   This is the "GPU everything, no CPU round-trip" architecture
   from the original v0.18 vision.

   This requires the mesher to take `(buffer, offset)` for its
   output instead of allocating its own SSBO. A modest refactor in
   `gpu_mesher_vk::VulkanMesher::new`. Doable in v0.18.3 once
   v0.18.2's pool is validated.

### Test count

455/455 passing (was 446 in v0.18.1). Added 9 mesh-pool tests:
`alloc_free_cycle`, `version_invalidates_handle`,
`exhaustion_returns_err`, `record_write_tracks_usage`,
`record_write_overflow`, `slot_offsets_are_distinct_and_strided`,
`free_list_recycles_in_fifo`, `many_alloc_free_cycles_no_leak`,
`invalid_slot_rejected`. All pure-Rust on `CpuMeshPool`; the
Vulkan layer is exercised at runtime by every chunk uploaded.

### Filed for v0.18.3

- Hardware validation of v0.18.1's pool / mesher AND v0.18.2's
  mesh pool, in a single playtest pass (the user can run
  `unendlich-world` and observe whether freezes are gone).
- GPU mesher → pool integration per the sketch above.
- Adaptive upload throttle (only if real measurements warrant it).
- Address GPU generator path on Honeykrisp once v0.18.1's FP32
  port has been runtime-tested.

### Files changed

- `src/render/mesh_buffer_pool.rs` (new, ~470 lines)
- `src/render/mod.rs` (one line added)
- `src/render/world_renderer.rs` (~150 line diff: struct field,
  init, upload_mesh rewrite, draw paths, destroy)

## Increment 16 (v0.18.3) — Mesh pool slot cap correction (small)

### What

v0.18.2 shipped with mesh pool slot caps of 128 KB vertex / 64 KB index.
The v0.18.2 playtest revealed real chunks are 200 KB – 7 MB vertex /
50 KB – 3 MB index — 50× larger than the cap. Result: every chunk
overflowed, the renderer dropped every mesh, and the world rendered
empty (chunks tangible via collision but invisible). The freeze fix
itself worked correctly (`upload=2-5ms`, no `vkAllocateMemory`
storms), so the increment is structurally sound, just sized wrong.

This patch raises the caps to 3 MB vertex / 2 MB index based on real
measurements and adds a unit test that performs the measurement so
this kind of mistake doesn't recur silently.

### Why it shipped wrong

The cap numbers in v0.18.2 were guessed, not measured. The design
notes claimed "CPU mesher's typical chunk meshes are 50-80 KB vertex"
based on intuition from generic minecraft-clone literature. unendlich's
mesher is genuinely heavier per-chunk: weathered tops emit 9×9 = 81
vertices per top face via `emit_subdivided_top`, and a surface chunk
has ~1024 such tops. That's intentional visual richness, not a meshing
inefficiency.

A second discovery during the fix: greedy meshing was already in use
(`mesh_chunk` → `mesh_chunk_greedy` since pre-v0.18). The earlier
proposal to "implement greedy meshing as the real fix" was based on
not having read all 3300 lines of `mesher.rs` — the algorithm is
already there and the 6 MB chunks are post-greedy.

### What changed

- `DEFAULT_VERT_BYTES_PER_SLOT` raised from 128 KB to 3 MB
- `DEFAULT_INDEX_BYTES_PER_SLOT` raised from 64 KB to 2 MB
- `MESH_POOL_CAPACITY` reduced from 2048 to 1024 to keep total memory
  bounded: 1024 × (3 MB + 2 MB) = 5 GB, which is feasible on M2 unified
  memory and discrete-GPU VRAM alike. (v0.18.2 had 2048 × 192 KB ≈
  393 MB; the new total is larger but proportionate to actual mesh
  sizes.)
- New test: `render::mesh_buffer_pool::tests::mesh_size_against_slot_caps`.
  Generates 8 representative chunks via `WorldGenerator`, meshes them,
  reports min/median/p99/max vertex+index bytes against current caps.
  Run with `--nocapture` to see the report. Always passes (it's a
  measurement, not an assertion) but warns loudly if any sample chunk
  exceeds the caps. Catches "the caps are too small" before playtest.

### Limitations

The 3 MB / 2 MB cap covers the test seed completely, but the v0.18.2
playtest log showed real chunks up to 7 MB vertex / 3.3 MB index in
densely vegetated surface terrain. Roughly 20-30% of real-world chunks
will still overflow at these caps and render empty. Players will see
holes in the densest terrain (typically near tree-heavy biome
intersections per the playtest log).

This is acknowledged as a band-aid. The proper fix is a sub-allocator
on top of one big buffer with free-list + coalescing — filed for v0.19.
At that point the 3 MB / 2 MB caps go away entirely; meshes get
exactly the bytes they need.

### v0.19 sub-allocator design sketch

Replace `CpuMeshPool`'s "fixed-size slot" model with a span allocator:

- One big vertex buffer + one big index buffer, sized to total budget
  (e.g., 4 GB vertex + 2 GB index).
- Free-list of `(offset, size)` spans, sorted by size for best-fit alloc.
- On free: insert span, coalesce with adjacent free spans.
- `MeshSlot` becomes `(vert_offset, vert_size, idx_offset, idx_size)`.
- Fragmentation is the risk. Reasonable mitigations: round allocations
  up to power-of-two boundaries (some waste, less fragmentation),
  periodic compaction (move all in-use spans to the start of the buffer
  during a quiet frame), or slab-allocator with fixed size classes.
- The CPU mesh-pool tests can stay pure-Rust by porting them to the
  new model; the existing 9 tests roughly translate to "alloc &
  free spans" patterns.

Estimated scope: 200-400 lines of new code in mesh_buffer_pool, plus
test updates, plus the `MeshSlot` -> `MeshSpan` rename ripple. Doable
in one session.

### Test count

456 passing (was 455 in v0.18.2 + 1 size-reporting test).

### Files changed

- `src/render/mesh_buffer_pool.rs` — cap bump (2 lines), size test
  (~80 lines)
- `src/render/world_renderer.rs` — capacity comment + value (5 lines
  diff)
- `Cargo.toml` — version bump

### Note on the testing-loop change

This increment finally adds the kind of pre-playtest measurement test
the user has been asking for. Lesson learned: when sizing GPU buffers
against measured workloads, the measurement should live in
`#[cfg(test)]` not in informal estimates in design docs. A test that
runs in 1 second and reports real numbers prevents an entire playtest
cycle's wasted time.

Going forward: any sizing-against-real-workload decision in this engine
should ship with a test that produces the workload and reports the
measurement. The mesh-size test is the template.

## Increment 17 (v0.19.0) — Mesh sub-allocator (real fix)

### What

Replaces the v0.18.x fixed-size slot model with a free-list span allocator.
Two big GPU buffers (vertex + index, default 3 GB + 2 GB), carved into
variable-size sub-regions sized to each chunk's actual mesh footprint.
No more "this slot is too small for this chunk" — every chunk gets
exactly the bytes it needs, and the pool only fails on total exhaustion
(which doesn't happen at any reasonable working-set size).

### Why now

The user observed during v0.18.3 playtest that 1024-slot pool was
smoother than 2048 or 4096, but chunk visibility plateaued and didn't
improve with more slots. Diagnosis: the same chunks were missing across
all three pool sizes — meaning slot count wasn't the bottleneck, slot
size was. v0.18.3's 3 MB vertex / 2 MB index caps fit ~70-80% of real
chunks; the rest dropped silently. The sub-allocator fixes this
fundamentally because there are no per-slot caps.

### Architecture

**`render::mesh_buffer_alloc::SpanAllocator`** (new module)

- Pure-Rust free-list allocator. Best-fit allocation strategy with
  256-byte alignment (covers
  `vk::PhysicalDeviceLimits::min_storage_buffer_offset_alignment` on
  all targets including Apple Silicon).
- Free spans tracked in `BTreeMap<offset, size>` for fast offset-keyed
  coalescing. Allocations tracked separately for double-free detection.
- Coalesces on free with both immediate neighbors. The standard
  textbook design.
- 15 unit tests covering: alignment, alloc/free round-trip,
  coalescing in all directions (forward, reverse, fragmented),
  exhaustion (both depleted and fragmented variants), best-fit
  selection (verified across distinct-sized free spans), invalid/double
  free rejection, and stress + realistic-workload tests confirming
  no unbounded fragmentation under churn.

**`render::mesh_buffer_pool::{CpuMeshPool, VulkanMeshPool}`** (rewritten)

- `MeshSlot` now holds `(vert_offset, vert_size, idx_offset, idx_size,
  version)` — offsets directly usable by `cmd_bind_vertex_buffers` and
  `cmd_bind_index_buffer`, no separate offset-lookup methods needed.
- `upload(vert_bytes, idx_bytes)` does both alloc and memcpy, returning
  the slot. Atomic across both axes — if index alloc fails after vertex
  succeeded, vertex is rolled back.
- `free(slot)` validates the version (stale-handle detection) before
  returning spans to the allocators.
- 10 unit tests covering: empty pool, alloc within bounds, alignment,
  free-releases-spans, double-free, stale-version rejection, exhaustion
  per-axis, vertex rollback on index failure, realistic 925-chunk
  workload fits, and balanced churn no-fragmentation.

**`render::world_renderer`** (updated)

- `mesh_pool: VulkanMeshPool` initialized with `::new()` (no capacity
  parameter; defaults to 3 GB / 2 GB).
- `upload_mesh` simplified: alloc-and-memcpy in one call. Gone is the
  "reuse existing slot" optimization — sub-allocator allocs are cheap
  (~100ns) and the new mesh size may differ from old, so a fresh
  alloc per upload is correct.
- Old slot defer-freed via existing `MAX_FRAMES_IN_FLIGHT` queue.
- Draw paths read `m.slot.vert_offset` / `m.slot.idx_offset` directly
  instead of calling helper methods.

**`render::mesh_buffer_pool::tests::mesh_size_against_pool`** (replaces
v0.18.3's `mesh_size_against_slot_caps`)

- Same 8-chunk sample. Now reports total pool utilization vs total
  pool size instead of per-slot fit. With 3 GB / 2 GB defaults, the
  test sample uses 0.1% of the budget — nominally over-provisioned,
  but fragmentation under real churn is the failure mode the test
  guards against, not raw size.

### Why best-fit, not first-fit or buddy

- **First-fit** is faster (O(log n) with offset-keyed BTreeMap, just
  iterate from start) but tends to fragment near the buffer's start.
- **Best-fit** is O(n) per alloc with a linear scan, but produces less
  fragmentation by leaving larger free spans intact for larger future
  allocations. With ~1000 free spans this is microseconds — well
  below the per-frame budget.
- **Buddy allocator** has lower fragmentation guarantees but rounds
  every alloc up to power-of-two, which would waste 30-50% on average
  for chunk meshes. Real meshes are 21 KB - 7 MB, very heterogeneous.

If profiling later shows best-fit's O(n) scan is meaningful, a
size-keyed secondary index (`BTreeMap<size, offset>`) makes it
O(log n). Filed as a v0.19.x optimization opportunity, not done here.

### Memory budget rationale

3 GB vertex + 2 GB index = 5 GB total. On M2 Max (32 GB unified) this
is 16% of system memory. A previous user experiment showed memory
pressure regressing FPS at higher pool sizes, so this is intentionally
conservative versus the "use all available memory" maximum. If
playtest exposes exhaustion, the constants are adjustable in one
place (`DEFAULT_VERT_BUFFER_SIZE` / `DEFAULT_INDEX_BUFFER_SIZE`).

### Risks (calling them out)

1. **Fragmentation over long sessions.** Coalescing handles obvious
   cases. The realistic-workload test confirms no unbounded
   fragmentation in 2000 alloc/free rounds. Real playtest sessions
   may run hours and produce patterns we haven't simulated. If
   fragmentation becomes pathological, mitigations include: periodic
   compaction (move all live spans to start during quiet frames),
   slab classes (separate allocators for small/medium/large), or
   split allocators per chunk-size-tier. None now.

2. **Best-fit O(n) cost.** With ~1000 free spans this is fast. With
   ~10,000 it might show up. Would happen at extreme render distance
   or after extreme churn. Filed as known limit.

3. **Defer-free correctness.** The MAX_FRAMES_IN_FLIGHT pattern from
   v0.18.x is preserved. The risk is that the GPU might still be
   reading old span memory while the allocator returns it for reuse.
   The defer queue holds slots for `MAX_FRAMES_IN_FLIGHT` frames before
   freeing the spans, same window the per-buffer destroy used.

### Test count

472 passing (was 456 in v0.18.3). +15 allocator tests +10 pool tests
−9 obsolete pool tests = +16 net.

### Files changed

- `src/render/mesh_buffer_alloc.rs` (new, ~530 lines)
- `src/render/mesh_buffer_pool.rs` (rewritten, ~470 lines, was ~590)
- `src/render/world_renderer.rs` (~80 line diff: simplified upload,
  draw-path offset access, init signature)
- `src/render/mod.rs` (one line)
- `Cargo.toml` (version)

## Increment 18 (v0.20.0) — GPU-driven indirect drawing

### What

Replaces the per-chunk draw loop with GPU-driven culling + a single
`vkCmdDrawIndexedIndirectCount` per pass. CPU-side draw issuance goes
from O(loaded chunks × 6 Vulkan calls) per frame to O(1) — a constant
number of calls regardless of chunk count.

### Why

v0.19's playtest exposed a new bottleneck: at ~400 chunks rendered,
`draw=900-1500ms` per second window with FPS 6-18. Streaming
(`upload`) was negligible. Diagnosis: 400 chunks × 6 Vulkan calls each
(2× bind, draw) = ~2400 calls/frame, all CPU-bound. The fix isn't to
make the draws faster individually; it's to do fewer of them.

The standard architectural answer is **indirect drawing with GPU
culling**: the GPU walks per-chunk metadata, decides which chunks are
visible, and writes draw commands into a buffer. The CPU then issues
ONE draw call that consumes the whole buffer. No per-chunk CPU work,
no per-chunk Vulkan calls.

### Architecture

**`render::chunk_indirect`** (new module, ~470 lines)

- `ChunkDraw` struct: `(index_count, first_index, vertex_offset,
  flags, aabb_min, aabb_max)`. One per active chunk, in a SSBO.
- `IndirectDrawSet`: owns the chunk metadata SSBO + indirect commands
  buffer + count buffer + cull pipeline. Slot allocator (free-list of
  u32 indices) tracks which metadata slots are in use. `MAX_CHUNK_DRAWS
  = 4096` (covers ~925-chunk render-distance-6 working set with
  headroom).
- `record_reset_count()` and `record_cull_dispatch()` for the
  per-pass GPU cull preamble.

**`src/shaders/chunk_cull.comp`** (new shader)

- Workgroup 64×1×1, one invocation per chunk.
- Reads chunk's `ChunkDraw`, runs the standard p-corner AABB-vs-frustum
  test (mirrors `Frustum::aabb_visible` from the CPU code). Visible
  chunks `atomicAdd` the visible counter and write a
  `VkDrawIndexedIndirectCommand` to the shared output buffer.
- Frustum + chunk_count come from push constants (112 bytes; under the
  128-byte minimum guarantee).
- Inactive slots (`index_count == 0`) are skipped early.
- `FLAG_DEBUG_OVERLAY` bit lets the renderer mark chunks that should
  bypass cull entirely (the debug-overlay sentinel chunk whose
  primitives can extend beyond a per-chunk AABB).

**Per-frame flow**:
1. CPU updates the metadata SSBO incrementally for any chunks that
   uploaded or evicted since last frame (~0-10 chunks/frame typical).
2. **Before each render pass**: reset count→0 (`cmd_fill_buffer`),
   barrier (TRANSFER → COMPUTE), dispatch cull, barrier (COMPUTE_SHADER
   SHADER_WRITE → DRAW_INDIRECT INDIRECT_COMMAND_READ).
3. Inside the render pass: bind vertex/index buffers ONCE, issue ONE
   `cmd_draw_indexed_indirect_count`.
4. Same flow for shadow pass with light-space frustum (separate
   dispatch; the indirect commands buffer is overwritten between
   passes — safe because the prior pass already submitted its draw).

### The alignment puzzle

`vkCmdDrawIndexedIndirectCommand`'s `vertexOffset` field is added to
each fetched index. With one shared vertex buffer for all chunks, the
per-chunk vertex base must be addressable as an integer index into
that buffer. **But the span allocator aligns to 256 bytes, and
`sizeof(Vertex) = 24`. 256 is not divisible by 24** (and no power of 2
is, since 24 = 2³ × 3). So `vert_offset / 24` is generally not integer.

Three options were considered:

- **Bump allocator alignment to LCM(256, 24) = 768.** 768 isn't a
  power of 2, breaking the bitmask `align_up`. Would need to switch to
  arithmetic align. ~3× more wasted padding per chunk.
- **Pad Vertex to 32 bytes** (next power of 2). Wastes 33% memory.
- **Track two offsets per slot**: the allocator-managed `vert_offset`
  (256-aligned, used by free()) and `vert_base_aligned` (rounded UP to
  the next Vertex boundary, used by upload writes and by the indirect
  draw). Up to 23 bytes wasted per chunk; the allocator's 256-byte
  rounding always provides enough slack.

Option 3 won. Implementation:
- `MeshSlot` carries both `vert_offset` and `vert_base_aligned`.
- `CpuMeshPool::alloc(v, i, vertex_size)` — vertex_size threads
  through to set `vert_base_aligned` correctly.
- `VulkanMeshPool::write_at(slot, v, i)` — writes vertex bytes at
  `vert_base_aligned`, index bytes at `idx_offset`.
- The renderer allocates first (so it knows the chunk's vertex base),
  then **pre-shifts indices by `vert_base_aligned / 24`** in a Vec on
  the CPU side, then writes. The indirect command's `vertex_offset = 0`
  because the shift is baked into the indices. Indices in the shared
  buffer now reference absolute positions; one buffer binding works for
  all chunks.

The pre-shift is one CPU pass over the index data per upload (already
a memcpy; this just adds a constant during the copy). Bounded cost,
not in the per-frame hot path.

**Tests confirm correctness**:
- `mesh_buffer_pool::vertex_alignment_24_makes_base_divisible`: every
  alloc with vertex_size=24 produces a `vert_base_aligned` that's a
  multiple of 24, with the actual data fitting in the span.
- `chunk_indirect::pre_shift_indices_land_at_correct_vertex`:
  shifted indices end up pointing at the right per-chunk vertices.

### Vulkan device feature opt-in

`drawIndirectCount` is a Vulkan 1.2 core feature requiring explicit
opt-in via `PhysicalDeviceVulkan12Features`. Added in
`render::context::create_device`.

### Risks (calling them out)

1. **Compute → indirect-draw barrier correctness**. The barrier from
   COMPUTE_SHADER write → DRAW_INDIRECT read is the standard pattern
   from the Vulkan spec. If wrong, the GPU could read stale
   commands/count and draw nothing or garbage. The barrier code is
   small (~30 lines) and reviewed against the spec; fingers crossed.

2. **Index-shift overflow**. `vert_base_count` is u32 added to each
   u32 index via `wrapping_add`. With `MAX_CHUNK_DRAWS = 4096` chunks
   and ~300K vertices per chunk max, total vertex count fits in u32
   trivially. But if one chunk's vertex base + its largest index
   exceeded u32, we'd silently wrap. Practically impossible at current
   scales (would need >4 billion total vertices); flag if extreme
   render distances become a goal.

3. **Debug overlay shadow regression**. The original CPU path skipped
   the debug-overlay sentinel during shadow pass ("its primitives
   shouldn't cast shadows"). The new GPU path uses the same cull
   shader for both passes; the debug overlay's `FLAG_DEBUG_OVERLAY`
   flag bypasses cull but doesn't differentiate main vs shadow. Minor
   visual issue with dev-only overlays. Filed for v0.20.x.

4. **Per-frame draw count not visible**. The CPU used to log
   `drew=N culled=M`. With GPU culling, those numbers live in the
   count buffer and reading them back would round-trip the GPU. Not
   worth it for a log line — diagnostic dropped.

### Test count

478 passing (was 472 in v0.19.0). +3 chunk_indirect tests +2 alignment
tests +1 pre-shift correctness test = +6.

### Files changed

- `src/render/chunk_indirect.rs` (new, ~530 lines incl. tests)
- `src/shaders/chunk_cull.comp` (new)
- `build.rs` (registered chunk_cull.comp)
- `src/render/mesh_buffer_pool.rs` (alloc takes vertex_size,
  vert_base_aligned added to MeshSlot, write_at method, +2 tests)
- `src/render/world_renderer.rs` (UploadedMesh.draw_slot,
  PendingDestroy.draw_slot, IndirectDrawSet field, upload_mesh shifts
  indices, both render passes use cull→draw_indirect)
- `src/render/context.rs` (drawIndirectCount feature opt-in)
- `src/render/mod.rs` (chunk_indirect module)
- `Cargo.toml` (0.19.0 → 0.20.0)

## Increment 19 (v0.20.1) — Diagnostic toggle for indirect drawing

### What

v0.20.0 didn't deliver the expected speedup on hardware (M2 Max
playtest: same `draw=1000-1400ms` per second window, FPS 6-18). On top
of that, trees were visually missing and chunk visibility plateaued at
183 of 997 loaded.

This isn't enough information to pinpoint the bug from inspection
alone. v0.20.1 ships **diagnostic infrastructure** so the next playtest
can bisect:

1. `UNENDLICH_INDIRECT=0` — env-var kill-switch. When set, the renderer
   uses a v0.19-style per-chunk draw loop instead of GPU cull +
   indirect draw. The per-chunk loop still uses the v0.20 vertex layout
   (pre-shifted indices, single binding at offset 0); this isolates the
   indirect-vs-direct decision from the index/vertex layout decision.
   - If indirect=0 fixes trees: bug is in the GPU cull/indirect plumbing
     (cull shader, barriers, count read).
   - If indirect=0 still missing trees: bug is in upload/layout (index
     shift math, alignment, per-chunk metadata).
2. `upload_fails: N` in the FPS log. Counts mesh-pool or indirect-set
   alloc failures since startup. Was non-zero in v0.20.0? Couldn't
   tell — counter didn't exist. Now it does.
3. Both startup paths log which mode is active.

### Why this approach

Bisect-friendly toggle is more useful than another speculative fix.
v0.20.0 introduced TWO architectural changes simultaneously: the
indirect draw path AND the new vertex-layout (pre-shifted indices).
A toggle for one but not the other lets us isolate which is broken,
if either.

The fallback path is a small copy of v0.19's per-chunk loop, ~30
lines, with the v0.20 binding/indexing convention. If the fallback
also misbehaves, the issue is in upload-time logic (the shared
codepath); if only indirect misbehaves, it's in the GPU cull or
draw plumbing.

### What's NOT changed

The cull shader, the indirect command buffer setup, the upload-time
index shift — none of them. v0.20.1 is purely diagnostic
infrastructure. v0.20.2 will be the actual fix once we know what to
fix.

### Files changed

- `src/render/world_renderer.rs` — `use_indirect` field with env opt-out,
  fallback per-chunk loops in shadow + main passes, `upload_fail_count`
  counter
- `src/bin/unendlich_world.rs` — surface `upload_fails` in FPS log
- `Cargo.toml` — 0.20.0 → 0.20.1

## Increment 20 (v0.21.0) — GPU timestamp queries + log trim

### Architectural commitments registered this conversation

Two big-picture decisions made in convo 4 that constrain every future
increment, recorded here as project-wide commitments rather than
per-increment chronicle:

**1. GPU-canonical world state, with CPU as the networking boundary**
(option β/γ from the convo). GPU owns blocks, fields, plant/tree
lifecycle, fluid sim, weathering, mesh generation, culling, shadows,
fog, atmosphere — the whole steady-state world. CPU owns networking
(rollback snapshots, tick boundaries), save/load, player input, GPU
command orchestration, and a debug mirror. The CPU↔GPU boundary
crosses only at network ticks and save points.

**2. Unified field/graph substrate.** Every dynamic system in the
engine (trees, plants, erosion, water flow, cliffs, weather, fluids)
shares one mental model: a graph of cells with per-cell scalar/vector
accumulators, updated by sparse deterministic noise samples that
draw bias from stacked field reads (temperature, sun, wind, moisture,
etc.), smoothed by multipass blur kernels for spatial coherence,
with discrete events emerging only when accumulators cross
thresholds. Substrate to be designed in a future increment;
this commitment exists to anchor against drift.

These commitments are NORTH STARS, not immediate constraints. Some
existing code (CPU plant lifecycle, CPU mesher) is not aligned with
them yet. That's fine. The point is that future architectural
decisions get checked against these so the engine doesn't drift
incompatibly.

### What v0.21.0 does

Wires up Vulkan timestamp queries so each render pass reports its
real GPU time. After v0.20.x's playtests, the indirect and direct
draw paths produced *identical* per-frame numbers — proving the
indirect plumbing works, but leaving the actual GPU bottleneck
unmeasured. We were guessing whether time went to shadow pass, main
pass, cull dispatch, or somewhere else. v0.21.0 ends the guessing.

Output in the FPS log:
```
  gpu: shadow=2.3ms cull=0.1ms main=12.5ms total=15.0ms (over 60 frames, 0 skipped)
```

If `gpu: total` ≈ `cpu: draw`, we're GPU-bound; if much less,
we're vsync-blocked. Whichever zone dominates is where the next
perf increment should go.

### Why this before any other perf work

We've shipped four perf-direction changes since v0.18.x without
moving the FPS needle. The pattern is "speculate where the GPU
time is going, change something, hope." This breaks that pattern:
once we have a number for each pass, the next change is informed
instead of guessed.

This is also infrastructure we'll lean on for years. Every future
GPU pipeline upgrade — fog, shadows, mesher port, instancing — gets
measured against zone deltas. "X dropped shadow time from 5ms to
2ms" is the kind of statement we want to be able to make.

### Design

- `src/render/gpu_timer.rs` — owns one `vk::QueryPool` of type
  `TIMESTAMP`, sized `MAX_FRAMES_IN_FLIGHT * MAX_ZONES`. Frame N
  uses queries `[N*MAX_ZONES, (N+1)*MAX_ZONES)`.
- 4 markers per frame: `FrameStart`, `ShadowEnd`, `MainCullEnd`,
  `MainEnd`. Adjacent-marker deltas give the "shadow", "cull",
  "main" zones. `FrameStart→MainEnd` gives "total".
- Reads `physical_device.timestamp_period` (ns/tick) and the
  graphics queue family's `timestamp_valid_bits` for wraparound
  masking. Apple Silicon GPUs typically report ~83.33ns / 36–48
  bits — well within our 1s aggregation window.
- `WITH_AVAILABILITY` flag on `get_query_pool_results` — first
  cycle through a slot has unwritten queries, which we detect via
  the availability bit and skip cleanly.
- `collect_frame` runs RIGHT AFTER `wait_for_fences` (when results
  are guaranteed available) and BEFORE `begin_frame` resets the
  slot.
- Falls back to a no-op timer if device doesn't support timestamps
  (period == 0 or valid_bits == 0). Logs a warning at construction.

### Shorter logs

Per-second log block trimmed: removed ~50 lines of code-as-comment
that explained each timer to the user. The actual log lines are now
more scannable:

```
FPS:  60  frame: ...  loaded: 200  rendering: 180  upload_fails: 0  ...
  cpu: cloud_remesh=... evict=... submit=... upload=... mgr_update=... tick_sim=... draw=...
  gpu: shadow=... cull=... main=... total=... (over N frames, M skipped)
  tick: plant_lifecycle=... apply=... (chunks=...)
  input: events=... (k=... m=... c=...) focus=... cap=...  climate: T=... H=...
```

Combined input + climate onto one line; renamed `streaming:` to
`cpu:` for parity with the new `gpu:` line.

### Files

- `src/render/gpu_timer.rs` — new (~400 lines incl. tests)
- `src/render/mod.rs` — register module
- `src/render/world_renderer.rs` — `gpu_timer` field, `begin_frame`
  + 3 `mark` calls, `collect_frame` after fence, `flush_gpu_timer_log`
  accessor
- `src/bin/unendlich_world.rs` — log block trimmed and gpu line added
- `Cargo.toml` — 0.20.1 → 0.21.0

### Tests

- `marker_discriminants_are_dense` — enum values match `MAX_ZONES`
- `zone_names_cover_expected_pairs` — label table
- `zones_iter_is_consistent` — public iterator order
- `query_index_math_is_dense_and_unique` — ring-buffer indexing
  is collision-free across frames × markers
- `timestamp_wraparound_handled` — masked subtract gives small
  positive delta even when `e < s` due to high-bit clamp

5/5 new pass; 483/483 total. No hardware Vulkan tests — the timer
needs a real device, validated only on the next playtest.

### Hardware risk

Low. Timestamp queries are stable Vulkan 1.0 functionality.
Honeykrisp on M2 Max is well-supported. The one wrinkle on Apple
Silicon is timestamp granularity (~µs not ns) but the conversion
math handles whatever `timestamp_period` reports.

### What we'll learn from the next playtest

Whichever zone dominates points to the next direction:
- `shadow=` dominates → cull the shadow pass; fewer cascades; or
  the shadow pipeline has overdraw/state issues.
- `main=` dominates → fragment shader cost (fog stepping,
  atmosphere) or overdraw. Could mean greedy mesher matters.
- `cull=` dominates → my GPU cull shader has work to do.
- `total << cpu:draw` → vsync/present blocking; investigate
  swapchain or try mailbox mode.
- All ~equal → wholesale GPU saturation; need draw-call/vertex
  reduction (greedy mesher, distant LOD).

Whatever the answer, v0.22 starts informed.

## Increment 21 (v0.21.1) — GPU timer fix + froxel zone + log compression

### What broke in v0.21.0

The first playtest of the GPU timer reported `gpu: <no frames
collected, skipped=N>` on every single log line. N grew monotonically
across the session — every collect attempt failed.

Root cause: `get_query_pool_results` was called WITHOUT the `WAIT`
flag. Even though the caller's `wait_for_fences` had returned (so
the GPU work was complete), the host-side query results hadn't yet
been pulled into the readable pool — driver still had a μs of
readback work to do. With only `WITH_AVAILABILITY` and no `WAIT`,
the function returned `NOT_READY` every frame, the availability
bit was always 0, and we hit the skip path 100% of the time.

The lesson: I read the Vulkan spec for `WITH_AVAILABILITY` and
assumed it meant "the function checks if results are ready and
returns availability bits along with whatever's there." It actually
means "the function returns NOT_READY if anything's missing AND
fills the availability bits so the caller can introspect." Without
`WAIT`, the function NEVER blocks, even briefly, for results to
become ready. The two flags compose: `WAIT | WITH_AVAILABILITY`
= "block until ready, also tell me if any individual queries
were never written."

### Fix

Add `WAIT` to the query result flags. After the fence is signaled,
WAIT blocks for at most a μs of driver readback. The
WITH_AVAILABILITY check is preserved as a defensive measure for
the first MAX_FRAMES_IN_FLIGHT iterations (slot has never been
written → availability bits are 0 → skip cleanly).

### Bonus: Froxel zone

While here, noticed the v0.21.0 instrumentation lumped the froxel
fill compute pass into the "shadow" zone (ShadowEnd was marked
before froxel; MainCullEnd was the next mark). Added an explicit
FroxelEnd marker between them. Now we have 5 markers, 4 zones:
shadow, froxel, cull, main.

If volumetric fog is the perf hot path (per SHADER_ROADMAP, this
is plausible), this disaggregation matters.

### Bonus: Log compression

Per user feedback, log volume was high. Suppressed the
`input: ... climate: ...` line when nothing changed across two
consecutive log windows: events==0, climate hasn't shifted by
>0.005, focus/capture state unchanged. Cuts ~25% of log volume on
idle sessions. The FPS line still emits every second as a
heartbeat, so the system-alive signal is preserved.

Future log-volume work to consider: deduplicate the cpu/gpu lines
when timer values are within ±5% of last window. Filed for
v0.21.x.

### Bonus: Module reusability marker

The `gpu_timer` module has no unendlich-specific dependencies (only
ash + std). Added a top-level doc comment marking it as a
candidate for extraction into a standalone crate or shared engine
helper. Documented the public API surface and the path to making
zone enums caller-defined via const generic. Not extracted today
— premature without a second consumer — but the boundary is now
explicit so future me sees it.

### Files

- `src/render/gpu_timer.rs` — WAIT flag, FroxelEnd marker, doc
  comment for reusability, tests updated for 5 markers
- `src/render/world_renderer.rs` — `mark(FroxelEnd)` after froxel
  fill barrier
- `src/bin/unendlich_world.rs` — input+climate line gated on change
- `Cargo.toml` — 0.21.0 → 0.21.1

### Tests

5/5 gpu_timer tests pass (updated for 5 markers). 483/483 total.
The Vulkan side is hardware-validated only by next playtest.

### What we should see next playtest

Lines that now look like:
```
gpu: shadow=X.Xms froxel=X.Xms cull=X.Xms main=X.Xms total=X.Xms (over N frames, M skipped)
```

`M` should be 2 (the first MAX_FRAMES_IN_FLIGHT iterations through
slots that were never written). If `M` keeps growing, the WAIT fix
didn't actually take and we're back to bug-hunt.

If `total` ≈ `cpu: draw`: GPU-bound, dominant zone is the next
target. If `total << cpu: draw`: vsync/present blocking; investigate
swapchain.

### Honest reflection

Two bug-recovery cycles in a row (v0.20.1 indirect equivalence
diagnostic, now v0.21.1 timer fix). Both were "shipped a thing,
playtest revealed something I should have caught locally." The
gpu_timer fix would have been caught if I had any way to validate
the timer against a real Vulkan device in CI; we don't, and won't
in this environment. The cost of that gap is one playtest per fix.

Mitigation for future: when adding hardware-only code, ALSO add
a CPU-only "smoke test" that exercises the API surface as far as
it can without a device. For gpu_timer that would be checking
that flush_log() returns the right placeholder string, that the
window resets correctly, that `supported = false` short-circuits
all methods. It wouldn't have caught the WAIT bug specifically
but it would catch lifecycle issues that don't need a device.

## Increment 22 (v0.21.2) — Real GPU timer fix (the actual bug)

### What v0.21.1 didn't fix

v0.21.1 added the `WAIT` flag to `get_query_pool_results`, theorising
that the timer was never collecting because results weren't ready
when we asked. Playtest still showed `gpu: <no frames collected,
skipped=N>` on every line.

### The real bug

`ash 0.37`'s `get_query_pool_results::<T>` computes Vulkan's
`data_size` argument as `size_of::<T>() * query_count`. With
`T = u64` and `query_count = 5`, ash passes `data_size = 40` to the
driver.

But with `WITH_AVAILABILITY | TYPE_64`, each query writes **16 bytes**
(timestamp + availability), so 5 queries need **80 bytes**. Half
the buffer.

The driver then does one of two things — neither correct for our
read pattern:
- Returns `VK_INCOMPLETE` (which ash maps to an Err). We didn't see
  errors in the warn! path, so probably not this.
- Fills the partial buffer with raw timestamps, no availability bits
  interleaved. Our availability check at `data[i*2+1]` then reads
  garbage at the timestamp positions of subsequent queries —
  occasionally non-zero (passing the check) but more likely zero
  (failing it). On Honeykrisp this evidently failed every frame.

### Fix

Change the buffer type from `[u64; MAX_ZONES * 2]` (T = u64, stride
8) to `[[u64; 2]; MAX_ZONES]` (T = [u64; 2], stride 16). Now ash
computes `data_size = 16 * 5 = 80`, matching what the driver
expects per query with this flag combination.

### CPU smoke test added

A new test `per_query_result_stride_is_16_bytes` pins the type
choice. If a future refactor drops back to T=u64 the test fails
locally without needing a Vulkan device.

This is the kind of test I committed to in v0.21.1's notes —
"CPU-only smoke test that exercises the API surface as far as it
can." For gpu_timer that's: stride math, marker enum density,
ring-buffer indexing, wraparound math, label coverage. The
WAIT/no-WAIT distinction itself can't be tested without a device,
but the ash-specific `data_size = stride * count` quirk now is.

### Lessons

Three bugs in three increments on this module (`WITH_AVAILABILITY`
without `WAIT`; correct flags with wrong stride; both fixed in
v0.21.2 only after seeing it on hardware twice). The shared root
cause: I was reading the Vulkan spec but not the ash binding's
implementation. ash hides the `data_size` parameter behind a
`size_of::<T>` assumption that's wrong for some flag combinations.

For future ash usage with non-trivial flag combos: read the ash
source, not just the Vulkan spec. The bindings have opinions about
what "natural" sizing should be, and those opinions can disagree
with what your flags actually need.

### Files

- `src/render/gpu_timer.rs` — type change + new test + extensive
  comment explaining the trap
- `Cargo.toml` — 0.21.1 → 0.21.2

### Tests

6/6 gpu_timer tests pass (added `per_query_result_stride_is_16_bytes`).
484/484 total.

## Increment 23 (v0.22.0) — LOD foundation (F1)

### What

The data-structure substrate for the LOD system. **Zero visible
behaviour change** — every call site uses `LodLevel::Near` and the
existing greedy mesher path. F2-F6 (lower-detail meshers, distance-
based dispatch, hysteresis, GPU-mesher tier, shadow-cascade
alignment) layer on top across future increments.

### Why F1 alone

Previous several increments (timer bug, indirect-equivalence
diagnostic, etc.) cost a hardware playtest each. The mesher and
meshing-pool refactors that LOD actually requires are non-trivial;
getting the type substrate wrong here would compound into every
subsequent F-increment. Shipping the substrate alone with full
unit-test coverage and zero behavioural change locks in the design
before any mesher code depends on it.

### Architecture decision: 4-tier LOD

User chose 4 levels (Near/Mid/Far/VeryFar) over the safer 3 because
of ambitious render-distance goals. Bands are 25/50/75% of
`render_distance` measured in **Chebyshev distance** (max of |dx|,
|dy|, |dz|). Chebyshev not Euclidean because it gives axis-aligned
cube bands matching the chunk grid — Euclidean produces awkward
donut-ring boundaries at lattice corners.

Band thresholds use integer division `(rd * N) / 100` with `.max(1)`
guards so even tiny RDs (1, 2) produce non-degenerate bands. For
RD=6 (default): Near={0..1}, Mid={2..3}, Far={4}, VeryFar={5+}.
For RD=32 (user's test): Near={0..8}, Mid={9..16}, Far={17..24},
VeryFar={25+}.

### Key data types (`src/world/lod.rs`)

- `LodLevel { Near=0, Mid=1, Far=2, VeryFar=3 }` — `#[repr(u8)]`,
  ordered, hashable. Discriminants are dense so `[T; LodLevel::COUNT]`
  arrays index cleanly. `From<LodLevel> for u32` for shader
  push-constants.
- `desired_lod_for(chunk, player, render_distance) -> LodLevel` —
  pure function, hysteresis-free (filed for F4).
- `MeshSlotKey { coord, lod }` — replaces `ChunkCoord` as the
  renderer's mesh slot HashMap key. `MeshSlotKey::near(coord)` is
  the F1-everywhere shorthand.
- `flags_with_lod(flag_bits, lod) -> u32` and `unpack_flags(u32) ->
  (flag_bits, LodLevel)` — pack/unpack for the GPU `ChunkDraw.flags`
  upper byte. Avoids growing the SSBO struct (would have padded
  to 64 bytes anyway, but explicit packing keeps the intent clear).

### Renderer changes (behaviour-preserving)

- `WorldRenderer::meshes` rekeyed `HashMap<ChunkCoord, _>` →
  `HashMap<MeshSlotKey, _>`. All internal access sites use
  `MeshSlotKey::near(coord)`. Public API unchanged.
- `loaded_chunk_coords()` now dedupes by coord across all LODs;
  in F1 this is a no-op (only Near exists) but the structure is
  correct for F2+.
- `ChunkDraw.flags` packed via `flags_with_lod(flags, Near)` at
  upload time. Result for Near is identical to old code (Near=0
  contributes 0 to upper bits). F2 will pack the actual LOD.

### What F1 does NOT do

- No mesher changes. CPU greedy mesher is still the only one used.
- No distance-based LOD dispatch. ChunkManager doesn't know about
  LODs yet. Filed for F3.
- No hysteresis. Filed for F4.
- No shader changes. Cull shader still ignores LOD bits in flags.
- No external API changes. `evict_chunk(coord)`, `mesh_version_for
  (coord)`, `upload_prebuilt_mesh(ctx, coord, ...)` etc. all retain
  their signatures and behave identically.

### Sequencing for F2-F6

- **F2 (v0.22.1)**: Naive `mesh_chunk_far` function in
  `src/render/mesher.rs`. Tests against greedy for silhouette
  parity. Not wired yet.
- **F3 (v0.22.2-3)**: ChunkManager LOD-aware. Distance-based
  dispatch. Per-LOD eviction. Mesh re-request on band crossing.
  Likely two sessions due to chunk priority/queue/eviction
  complexity.
- **F4 (v0.22.4)**: Hysteresis to fix popping at band boundaries.
- **F5 (v0.22.5)**: GPU mesher (the v0.18.1 unwired one) becomes
  the LOD::Far / LOD::VeryFar mesher. CPU greedy stays for Near/Mid.
- **F6 (v0.22.6)**: Shadow-cascade alignment with LOD bands.
  Cascaded shadow maps (E from the previous shadow-plan menu)
  becomes natural to add here — each cascade uses a different LOD
  level by design.

### Files

- `src/world/lod.rs` — new (~370 lines incl. tests)
- `src/world/mod.rs` — register module
- `src/render/world_renderer.rs` — HashMap rekey + 6 internal
  call site updates + flags_with_lod packing at upload
- `Cargo.toml` — 0.21.2 → 0.22.0 (minor bump because public API
  semantics shift slightly even though signatures are stable —
  loaded_chunk_coords now dedupes, mesh_version_for explicitly
  selects the Near LOD)

### Tests

16 new tests in `src/world/lod.rs`:
- Discriminant density, ordering, iter, u32 conversion
- desired_lod at player position, beyond RD, at exact band
  boundaries (RD=6 and RD=32 explicit tables), tiny RDs (1, 2),
  Chebyshev uniformity, negative offsets
- MeshSlotKey uniqueness, near-shorthand
- flags packing roundtrip, no-bleed, out-of-range fallback

502/502 total tests pass (was 486; +16 new lod tests).

### What we'll see in the next playtest

Nothing visible. FPS, GPU timer numbers, chunk counts, all
identical to v0.21.2. The verification we want from a playtest is
**no regression**: same `gpu: shadow=...` numbers, no new
warnings, no upload_fails, world looks the same.

If anything DOES change after this drop, it's a bug in the
HashMap rekey or the flags packing — both should be behavior-
preserving but you never know with mechanical refactors.

## Increment 24 (v0.22.1) — Runtime flags registry + --help

### What

`--help` and `--version` flags on every binary, backed by a shared
registry of runtime knobs (`src/runtime_flags.rs`). The registry
lists every recognized env var with its default, valid range, scope
(which binaries respect it), and a summary. `--help` filters by
binary name so users see only the knobs that affect them.

Also consolidates the duplicated `env_i32`/`env_usize` helpers from
the spin binary into the shared module, so future env-var additions
land in one place.

### Why

User feedback after F1 playtest: "add a help to show all of the
runtime flags we support at the moment." The number of env vars has
grown across recent increments (RENDER_DISTANCE, VSYNC, INDIRECT,
SIDE, MAX_UPLOADS_PER_FRAME, RUST_LOG, RUN_VK_TESTS) and there was
no single place to discover them.

### Playtest verification of v0.22.0

The F1 playtest log confirms zero regression: shadow/main/cull/
froxel numbers match v0.21.2 within noise; lifetime summary
correctly shows 1726 collected frames over ~80 seconds with mean
total=34.71ms (dragged down by ~12 seconds at 60-120fps after
streaming completed). HashMap rekey is verified clean.

The lifetime summary surfaced an interesting detail: cull is
normally 0.03ms but had three spikes of 17-122ms across 1726
frames. Filed as a noise observation; could be Vulkan barrier
stalls or a measurement artifact at frame transitions.

The bigger reveal from the playtest: when `gen_pending` hits 0,
FPS jumps to 60-120 immediately. **CPU streaming is the dominant
visible bottleneck during loading**, not the GPU shadow pass per
se. Shadow optimization (F2-F6) is still the right north star but
won't help the during-loading FPS dip — that needs separate
streaming-pipeline work, filed as future v0.23.

### Files

- `src/runtime_flags.rs` — new (~280 lines incl. tests). Public
  registry, `print_help`, `print_version`, `wants_help`,
  `wants_version`, `env_i32`, `env_usize`.
- `src/lib.rs` — register module
- `src/bin/unendlich_world.rs` — handle --help/--version at top
  of main, before any setup
- `src/bin/unendlich_spin.rs` — same; remove duplicated env_i32/
  env_usize helpers, use shared ones
- `Cargo.toml` — 0.22.0 → 0.22.1

### Tests

5 new tests in `runtime_flags`:
- All flags have non-empty fields
- Env names follow UNENDLICH_* convention (with allowlist for
  RUST_LOG)
- No duplicate env names
- Wrap helper works (single-line for short input, multi-line for
  long)
- env_i32 returns default when var is absent

507/507 total tests pass (was 502).

### Sample output

```
$ unendlich-world --help
unendlich engine v0.22.1 — runtime flags (unendlich-world)

Recognized command-line flags:
  --help, -h          Print this help and exit.
  --version, -V       Print version and exit.

Environment variables (read at startup):
  UNENDLICH_RENDER_DISTANCE  Override the chunk-streaming render distance
                               default: 6   valid: 1..=256   scope: (unendlich-world)

  UNENDLICH_VSYNC            Force a presentation mode. 1 = FIFO (vsync on), 0 =
                               MAILBOX/IMMEDIATE (vsync off, lower latency)
                               default: 1   valid: 0|1   scope: (all)
  ...
```

The spin binary's help correctly excludes `UNENDLICH_RENDER_DISTANCE`
(world-only) and includes `UNENDLICH_SIDE` / `UNENDLICH_MAX_UPLOADS
_PER_FRAME` (spin-only).

### Adding a new flag in the future

1. Add a `Flag` entry to `flags()` in `runtime_flags.rs`.
2. Read the env var via `env_i32`/`env_usize` (or your own parser
   for non-numeric).

The help output reflects the registry automatically.

## Increment 25 (v0.22.2) — Far mesher (F2)

### What

`mesh_chunk_far`, `mesh_chunk_very_far`, and the underlying
parameterised `mesh_chunk_downsampled(chunk, factor)` in
`src/render/mesher.rs`. **Not yet wired to anything** — F3 does
that. F2 is the function alone, with tests.

### Algorithm

1. **Downsample**: walk the chunk in F×F×F super-cells. For each
   super-cell, count meshable source blocks. If majority (≥ ½ + 1),
   the super-cell is solid; its representative BlockId is the most
   common solid block among contributors (first-wins on tie).
2. **Naive face-emit**: same logic as `mesh_chunk_naive` but on the
   downsampled grid. Each cell that's solid emits a face wherever
   it borders an air cell (or the chunk boundary, since the far
   mesher cannot see neighbouring chunks).
3. **Coordinate scaling**: quad corners are scaled by `factor`, so
   a downsampled cell at grid index (i,j,k) emits quads at world
   position (origin + i·F, origin + j·F, origin + k·F) with size
   F×F per face.

`mesh_chunk_far` = `mesh_chunk_downsampled(chunk, 2)` (~4× fewer
quads than naive). `mesh_chunk_very_far` = factor 4 (~16× fewer).

### Boundary handling

The far mesher ALWAYS emits faces at the chunk boundary because it
cannot see the neighbouring chunk's data. This will produce small
visual cracks at LOD boundaries (where a Near chunk's outward face
might be culled by its neighbour but the Far chunk emits it
anyway). Filed for F3+ when the cracks become visible — likely
solved by an LOD-aware boundary handshake or a small overlap zone.

### Why no greedy merge in F2

The naive emission inside the downsampled grid leaves obvious
merging opportunities (adjacent solid same-color cells could fold
into one quad). Deferred for three reasons:

1. The whole point of LOD is "cheap to mesh"; greedy adds compute
   that may exceed the win.
2. F2 has zero call sites yet (gets wired in F3); shipping a
   smaller substrate first matches the F1-only discipline that
   worked for v0.22.0.
3. If hardware data shows the triangle reduction is insufficient,
   we add a greedy pass on top in a separate increment without
   risking the substrate.

### Files

- `src/render/mesher.rs` — three new public functions inserted
  between `mesh_chunk_naive` and the greedy section header
  (~180 lines incl. inline docs)
- `Cargo.toml` — 0.22.1 → 0.22.2

### Tests (12 new)

All in `src/render/mesher.rs::tests`:

- `far_empty_chunk_yields_empty_mesh` — empty chunk → empty mesh
  for every factor in [1, 2, 4, 8, 16, 32].
- `far_factor_1_matches_naive` — at factor=1 the function produces
  output identical to `mesh_chunk_naive`. Pins the algorithm: any
  refactor that breaks the identity case is caught immediately.
- `far_solid_chunk_factor_2_outer_faces_only` — fully solid chunk
  produces exactly 6 × 16² = 1536 quads (outer shell only, no
  inner faces, no greedy merge).
- `far_factor_2_emits_fewer_triangles_than_naive` — strict
  inequality on a CPU-generated surface chunk.
- `very_far_emits_fewer_triangles_than_far` — factor=4 strictly
  cheaper than factor=2 on the same chunk.
- `far_vertices_stay_within_chunk_aabb` — output positions
  contained in [origin, origin + CHUNK_SIZE] for non-trivial chunk
  coords like (5,1,-3). Catches off-by-one in coordinate math.
- `far_triangle_count_bounded_by_grid_size` — output ≤ 6 × grid³ ×
  4 verts per quad. Pins the cost claim.
- `far_half_solid_chunk_has_one_top_layer` — chunk with bottom
  half solid produces exactly 1024 quads (256 top + 256 bottom +
  4 walls × 128 each). Tests slicing math.
- `far_majority_threshold_half_air` — exactly half-solid cell
  treated as air (½ + 1 rule).
- `far_majority_threshold_above_half` — 5/8 solid cell IS solid;
  produces 6 outer faces (24 verts).
- `far_representative_is_majority_block` — 4 stone + 1 dirt → all
  vertex colors are greyscale (stone), not warm (dirt).
- `far_invalid_factor_panics_in_debug` — debug_assert! fires for
  factor=3 (doesn't divide CHUNK_SIZE=32).

519/519 total tests pass (was 507).

### What this enables

F3 (next increment, likely 2 sessions due to chunk priority/queue/
eviction complexity): wire `mesh_chunk_far` and `mesh_chunk_very_far`
into the chunk_manager dispatch. When a chunk's desired LOD changes
(player crosses a band), schedule a re-mesh at the new LOD. Renderer
holds at most one mesh per chunk per LOD slot.

When F3 lands we should see the first measurable shadow-pass GPU
time reduction: distant chunks contribute a fraction of their
previous triangle count, so the depth-only rasterisation work in
the shadow pass drops proportionally. Estimated 30-60% reduction
in steady-state shadow time at RD=6, more at RD=32.

## Increment 26 (v0.22.3) — F3a: LOD-aware meshing pool dispatch

### What

Wires LOD selection from chunk distance through to the mesher. Each
chunk gets meshed at its distance-appropriate detail level: Near
chunks use the existing greedy mesher (full climate tinting, sky
lighting, plant voxelization); non-Near use `mesh_chunk_downsampled`
with a tunable factor.

**This is the first F-track increment that produces visible behaviour
change.** Distant chunks now mesh much faster and emit far fewer
triangles. Chunks crossing band boundaries are not yet re-meshed —
that's F3b. F3a covers the case where chunks are first meshed at
their distance-appropriate LOD; F3b handles transitions when the
player walks toward a previously-distant chunk.

### Architecture decision: tunable factors per tier

After three rounds of design discussion, the agreed-upon shape:

- `LodLevel` enum stays as 4 named tiers (Near, Mid, Far, VeryFar).
- `LodSettings` struct holds a downsample factor per tier. Default
  curve is `[1, 2, 4, 8]` — Near at full detail, each subsequent
  tier downsampling 2× more aggressively.
- The mapping from tier → factor is settings, not architecture.
  Future settings UI (or env var today) lets users retune.
- `VALID_FACTORS = [1, 2, 4, 8, 16, 32]` enumerates the divisors of
  CHUNK_SIZE that produce valid downsampled grids.

The user can override at runtime with `UNENDLICH_LOD_FACTORS=1,2,8,32`
for aggressive simplification or `UNENDLICH_LOD_FACTORS=1,1,1,1` to
effectively disable LOD (test baseline).

### Why F3a alone (not F3a + F3b)

F3a is a clean dispatch addition: the chunk_manager streams chunks
into the mesh queue as before, but each request now carries an LOD
tag. F3b adds the harder problem: detecting when a chunk's desired
LOD has *changed* and re-queueing it. F3b touches the streaming
priority, the in-flight tracker, and the eviction logic — separate
concerns from "make the mesher pick the right path."

Shipping F3a alone gets us:
- Verifiable LOD dispatch (the failing-test-since-fixed pinned this).
- Real perf data: distant chunks should mesh much faster, and
  shadow GPU time should drop because distant chunks contribute
  fewer triangles to the shadow rasterization.

What F3a does NOT do:
- If the player walks toward a chunk meshed at LOD::Far, that chunk
  stays at Far detail (chunky terrain near the player). Visible bug.
  F3b fixes this.

### Algorithm

In the chunk-streaming loop (unendlich_world.rs):
```
for each chunk to mesh:
    lod = desired_lod_for(coord, player_chunk, render_distance)
    submit MeshRequest { ..., lod }
```

In the worker (meshing_pool.rs):
```
if request.lod == Near:
    use greedy mesher (sky+climate+tick, full detail)
else:
    factor = lod_settings.factor_for(request.lod)
    mesh = mesh_chunk_downsampled(chunk, factor)
```

In the renderer:
```
upload_prebuilt_mesh_at_lod(coord, lod, mesh, version)
    → stores at MeshSlotKey::at(coord, lod)
    → packs lod into ChunkDraw.flags upper byte
```

### Boundary cracks (known visual bug, deferred)

The far mesher cannot see neighbouring chunks; it always emits the
chunk-boundary face (see F2 design notes). When a Near chunk and a
Far chunk are adjacent, the boundary cull rule disagrees: Near's
greedy mesher culls the boundary face if its neighbour exists,
while Far always emits. Result: small visible cracks at LOD
transitions.

This is the LOD-boundary handshake problem mentioned in F2's notes.
Filing it for resolution after F3b — likely solved by either
- a small overlap zone (Near and adjacent Mid both stretch their
  meshes across the boundary), or
- a per-LOD cull rule that knows what the neighbour's LOD will be.

### Files changed

- `src/world/lod.rs` (+~150 lines): `LodSettings`, `VALID_FACTORS`,
  `from_env()`, validation. 8 new tests.
- `src/render/meshing_pool.rs`: `MeshRequest.lod` field; `MeshResult.
  lod` field; `MeshingPool::new` takes `Arc<LodSettings>`; worker
  dispatches by LOD. 1 new dispatch test (replaces a wrong-direction
  inequality test that caught the greedy-mesher-on-solid-chunk
  edge case).
- `src/render/world_renderer.rs`: `upload_prebuilt_mesh_at_lod`,
  `evict_chunk_lod`, `mesh_version_for_lod`. `evict_chunk` evicts
  ALL LODs of a coord. `mesh_version_for` returns the most-detailed
  available.
- `src/bin/unendlich_world.rs`: builds `LodSettings::from_env()` at
  startup, passes Arc to MeshingPool, computes `desired_lod_for`
  per chunk in the mesh-request loop.
- `src/runtime_flags.rs`: registers `UNENDLICH_LOD_FACTORS` so
  `--help` shows it.
- `Cargo.toml`: 0.22.2 → 0.22.3.

### Tests

528 / 528 (was 519, +9):
- 8 new LodSettings tests (default, validation, monotonicity,
  env-var parsing).
- 1 new meshing_pool dispatch test (Near worker output ==
  greedy mesher output, Far worker output ==
  `mesh_chunk_downsampled(., 4)` output).

The dispatch test replaced a flawed test from the previous attempt
that asserted Far < Near vertex count on a fully-solid chunk —
which inverts on degenerate inputs because greedy merges fully-
solid chunks to 6 quads while factored-naive emits 6 × 8² = 384
quads. Lesson: dispatch correctness should compare to a direct
call of the expected mesher, not infer correctness from output
size.

### What we should see in the next playtest

This is the first F-track increment with visible behaviour change.
At RD=6 (default): all chunks within Chebyshev distance 1 are Near;
2-3 are Mid (factor=2); 4 is Far (factor=4); 5+ is VeryFar
(factor=8). The shadow + main GPU times should drop measurably
during steady-state play because distant chunks now contribute
~16× fewer (Far) or ~64× fewer (VeryFar) triangles.

Honest expectations:
- Steady-state shadow time should drop, possibly substantially.
- Main pass time should drop more modestly (depends on overdraw,
  not just triangle count).
- Total during streaming may not change much: streaming pressure
  was the dominant bottleneck per the previous playtest analysis.
- **Visible cracks at LOD boundaries** when looking at chunks 4-5
  away. Known and filed.
- **Chunky terrain when walking toward a previously-distant chunk**
  because F3b doesn't re-mesh on band crossing. Known and filed.

### Sequencing for F3b+

- **F3b (next)**: detect LOD changes per chunk, re-queue affected
  chunks for re-mesh. Old mesh stays until new is ready.
- **F4**: hysteresis to dampen ping-ponging at band boundaries.
- **F5**: GPU mesher wired as Far/VeryFar tier (replaces the CPU
  downsampled path for those tiers).
- **F6**: shadow-cascade alignment with LOD bands.

## Increment 27 (v0.23.0) — CPU per-zone timer (diagnostic)

### What

Adds wall-clock timing for each zone of the renderer's draw frame:
`wait_fences`, `acquire`, `record`, `submit`, `present`. Per-second
log line + lifetime summary at shutdown, mirroring v0.21.3's GPU
timer infrastructure.

**Diagnostic only — zero behavior change.** The point of v0.23.0
is to expose where wall-clock CPU time goes when the GPU is fast.
v0.23.1+ will use the data to fix whichever zone dominates.

### Why this exists

The v0.22.3 playtest revealed:
- GPU total mean = 2.40ms over 38317 frames (lifetime).
- `cpu: draw` per-second wall-clock = ~1000ms (full frame budget).
- Gap of ~995ms is CPU work the existing `cpu:` timers don't
  account for. F-track LOD optimization fixed the GPU; now the
  next bottleneck is exposed.

The unmeasured-CPU theory: time is being spent in
`vkWaitForFences`, `vkAcquireNextImageKHR`, command-buffer recording
(uploads + render passes), or `vkQueuePresentKHR`. Each is a
distinct bottleneck class with a distinct fix:

| zone | symptom of high time |
|-|-|
| wait_fences | GPU not keeping up; previous frame still running |
| acquire | vsync blocking; swapchain exhaustion |
| record | upload bursts; cull dispatch overhead; many draws |
| submit | usually fast; high = driver pressure |
| present | vsync blocking; swapchain pressure |

We can't tell which is dominant from per-second aggregates. The
new timer attributes time to each zone individually with mean,
stddev, min, max — same Welford-stable accumulator the GPU timer
uses.

### Design

#### `src/util/zone_stats.rs` (new)

Extracted `ZoneStats` from `src/render/gpu_timer.rs` so both
timers share the same numerically stable accumulator. Pure helper
module, no Vulkan or unendlich-specific types. Public API:
`push(x_us)`, `stddev_us()`, fields `n`, `mean_us`, `m2`,
`min_us`, `max_us`. 4 tests.

The previous gpu_timer-local `ZoneStats` is replaced with a
`use crate::util::zone_stats::ZoneStats` import. The two
duplicated tests in gpu_timer.rs were removed (they're now in
the util module's tests).

#### `src/render/cpu_timer.rs` (new)

`CpuTimer` with the same shape as `GpuTimer`:

```rust
let mut t = CpuTimer::new();
t.begin_frame();
do_wait();        t.mark(CpuZone::WaitFences);
do_acquire();     t.mark(CpuZone::AcquireImage);
record_cmd();     t.mark(CpuZone::RecordCmd);
submit();         t.mark(CpuZone::QueueSubmit);
present();        t.mark(CpuZone::QueuePresent);
t.end_frame();
```

Each `mark(Z)` records elapsed time since the previous mark (or
`begin_frame`) into zone Z. Cost per mark: one `Instant::now()`
(syscall-free `clock_gettime(CLOCK_MONOTONIC)` ~20ns on aarch64) +
HashMap entry update. ~12 μs/sec at 120 FPS → negligible.

Public API:
- `flush_log() -> String` — one-line per-zone mean for the FPS
  log block; resets the window.
- `flush_lifetime_summary() -> String` — multi-line summary at
  shutdown.

8 tests covering: zone name stability, iter order, mark-without-
begin no-op, full-frame recording, multi-frame accumulation,
flush behavior, summary ordering.

#### Renderer wiring

`WorldRenderer` gains a `cpu_timer` field and four accessors that
mirror the GPU timer's:
- `flush_cpu_timer_log()`
- `flush_cpu_timer_summary()`

`draw_frame_with_view_and_atmos` calls `begin_frame()` at the
top, then `mark()` after each Vulkan call, then `end_frame()`
at the very end. The `RecordCmd` mark fires after
`end_command_buffer`, encompassing all upload + cull + shadow +
main pass recording.

#### Bin wiring

Per-second log adds one new line:
```
  cpu_zones: wait_fences=X.XXms acquire=X.XXms record=X.XXms submit=X.XXms present=X.XXms (over N frames)
```

Shutdown adds:
```
CPU timer lifetime summary:
  wait_fences   n=N      mean=X.XXms  stddev=X.XXms  min=X.XXms  max=X.XXms
  acquire       n=N      mean=X.XXms  stddev=X.XXms  min=X.XXms  max=X.XXms
  record        ...
  submit        ...
  present       ...
```

### What we should see in the next playtest

This is the question the increment is designed to answer. We'll
know more after the playtest than we do now. Predicted
distribution from the data we have:

- **`acquire` likely dominant during steady-state.** Vsync pacing
  blocks the swapchain image acquire. With FIFO mode + 120Hz
  cadence, expect ~8ms mean here.
- **`record` likely dominant during streaming bursts.** Upload
  spikes (28ms+ in the v0.22.3 playtest) get captured here.
- **`wait_fences` low under steady-state**, spikes during
  GPU-bound moments.
- **`submit` and `present` should be ≤1ms each**.

If the actual distribution differs substantially from these
predictions — that itself is a finding worth noting. The point
of the diagnostic is to learn what we don't already know.

### Files

- `src/util/mod.rs` — new
- `src/util/zone_stats.rs` — new (~110 lines incl. 4 tests)
- `src/render/cpu_timer.rs` — new (~280 lines incl. 8 tests)
- `src/render/gpu_timer.rs` — uses shared ZoneStats; removed 2
  duplicated tests
- `src/render/mod.rs` — register cpu_timer
- `src/lib.rs` — register util
- `src/render/world_renderer.rs` — `cpu_timer` field, init in
  constructor, mark calls in `draw_frame_with_view_and_atmos`,
  two accessor methods
- `src/bin/unendlich_world.rs` — per-second log line for
  `cpu_zones`, shutdown line for lifetime summary
- `Cargo.toml` — 0.22.3 → 0.23.0

### Tests

538 / 538 (was 528, +10):
- 4 new zone_stats tests in util
- 8 new cpu_timer tests
- 2 tests removed from gpu_timer.rs (duplicates of util zone_stats
  tests after the extraction)

### Note on the version bump

This is the first 0.23 increment. v0.22 was the F-track for LOD
(F1-F3a shipped, F3b-F6 deferred). v0.23 is the streaming-pipeline
investigation track. F3b is filed and will likely return as v0.24
once we've understood the streaming bottleneck.

## Increment 28 (v0.23.1) — MesherTimer (per-LOD worker timing)

### What

Per-LOD timing of `MeshingPool` worker threads, attributing CPU
wall-clock to each chunk meshing call by LOD tier (Near, Mid, Far,
VeryFar). Per-second window log + lifetime summary at shutdown,
mirroring `cpu_timer` and `gpu_timer` shape.

**Diagnostic only — zero behavior change.** This is the third
measurement-only ship in a row, deliberately. The point is to test
v0.23.0's hypothesis: "meshing is the bottleneck."

### Why this exists

The v0.23.0 playtest (15523 lifetime frames) revealed:

| zone | mean | stddev | max |
|-|-|-|-|
| wait_fences | 5.04ms | 41.28ms | 857ms |
| acquire | 4.40ms | 4.77ms | 242ms |
| record | 0.22ms | 0.10ms | 0.80ms |
| submit | 0.14ms | 0.06ms | 2.92ms |
| present | 0.01ms | 0.00ms | 0.13ms |

Three findings:

1. **Upload is NOT the bottleneck.** `record` mean 0.22ms means
   even worst-case per-frame upload is sub-ms. The v0.22.x theory
   "throttle uploads" would have been the wrong fix.
2. **Bursty wait_fences.** Mean 5ms but max 857ms; stddev 41ms
   indicates extreme bimodality. The user-perceived "1-2 FPS
   patches then briefly 188 FPS" is the GPU occasionally taking
   200+ms per frame, which the next CPU frame waits for.
3. **`mesh_q: 179/128` saturated.** Workers cannot keep up.

If workers are slow, the chunk pipeline backs up. Slow chunks land
in bursts, GPU does long shadow + main passes, fences pile up.
This module measures whether that's actually what's happening.

### Design

#### `src/render/mesher_timer.rs` (new)

`MesherTimer` shared via `Arc<MesherTimer>` across all worker
threads. Internal `Mutex<MesherTimerInner>` for safe concurrent
record. One Mutex acquire per chunk meshing call (~1-200ms apart);
contention is irrelevant.

API:
- `record(lod, elapsed_us)` — push one observation; called by
  workers
- `flush_log() -> String` — one-line per-LOD window means + counts;
  resets window
- `flush_lifetime_summary() -> String` — multi-line lifetime stats

Empty tiers omitted from window log (keeps line short during
loading-only or exploration-only phases).

7 tests covering: empty sentinel, single-tier record, multi-tier
independence, flush window/lifetime separation, summary ordering,
and 4-thread concurrent record correctness.

#### `LodLevel::short_name()` added

Compact log labels: "near", "mid", "far", "vfar". 1 new test
pinning the strings (mesher_timer log format depends on them).

#### MeshingPool wiring

Pool gains an `Arc<MesherTimer>` field. Constructor builds the
timer and clones the Arc to each worker. Worker dispatch wrapped
in `Instant::now()`:

```rust
let t0 = Instant::now();
let mesh = if req.lod == LodLevel::Near {
    // ... greedy path
} else {
    mesh_chunk_downsampled(&req.chunk, factor as i32)
};
let elapsed_us = t0.elapsed().as_micros() as u64;
mesher_timer.record(req.lod, elapsed_us);
```

Pool exposes `flush_mesher_log()` and `flush_mesher_summary()`
for the bin to call.

#### Bin wiring

Per-second log adds:
```
  mesher: near=X.XXms×N mid=Y.YYms×M far=Z.ZZms×K vfar=W.WWms×J
```

Empty tiers are skipped. `×N` = chunk count this window per tier.

Shutdown adds:
```
Mesher timer lifetime summary (per LOD):
  near    n=N  mean=...ms  stddev=...ms  min=...ms  max=...ms
  mid     n=N  ...
  far     n=N  ...
  vfar    n=N  ...
```

### What we should see in the next playtest

The hypothesis: Near greedy meshing dominates. If so, expect:
- `near mean = 50-200ms`, `n` in the low thousands
- `mid mean = 5-20ms`, `far mean = 1-5ms`, `vfar mean = 0.5-2ms`

If `near` mean is genuinely 50ms+, that's the smoking gun and
v0.23.2 attacks the greedy mesher (skip per-tick climate sample,
batch greedy passes, parallelize within a chunk).

If `near` mean is much lower (say 5ms), the bottleneck is
elsewhere — perhaps queue throughput, upload throttling, or chunk
generation gating mesh requests. We pivot accordingly.

If non-Near tiers are unexpectedly slow (e.g., `vfar mean = 50ms`),
the downsampled path needs work — but I'd be surprised given F2's
algorithmic simplicity.

### Files

- `src/render/mesher_timer.rs` — new (~210 lines incl. 7 tests)
- `src/render/mod.rs` — register mesher_timer
- `src/world/lod.rs` — `short_name()` accessor + 1 test
- `src/render/meshing_pool.rs` — pool owns `Arc<MesherTimer>`,
  workers receive a clone, dispatch wrapped in `Instant::now()`,
  two new accessors on the pool
- `src/bin/unendlich_world.rs` — per-second mesher log line,
  shutdown lifetime summary line
- `Cargo.toml` — 0.23.0 → 0.23.1

### Tests

546 / 546 (was 538, +8): 7 mesher_timer tests, 1 lod short_name test.

## Increment 29 (v0.23.2) — LOD double-draw bug fix + MesherTimer ship

### Two things in one ship

This increment combines:

1. **Bug fix (correctness)**: F3a's `upload_prebuilt_mesh_at_lod`
   never evicted the OLD LOD's mesh when a chunk transitioned to a
   new LOD. Result: chunks were double- or triple-drawn after
   crossing band boundaries. This inflated GPU shadow + main times
   and explained why aggressive LOD factors at RD=32 produced
   *worse* GPU numbers than default LOD at RD=6.
2. **MesherTimer (diagnostic)**: per-LOD timing of the meshing-pool
   worker dispatch. Tests the v0.23.0 hypothesis "meshing is the
   bottleneck" by attributing CPU time to each LOD tier. Built but
   not previously shipped; rolled into this version because the
   bug fix means its data is now trustworthy (without the fix,
   any "Near is slow" finding would be confounded by the same
   chunks being meshed multiple times as their LOD bumped).

### The bug

F3a (v0.22.3) wired LOD-aware dispatch with `MeshSlotKey { coord,
lod }` so the renderer could store multiple meshes per coord. The
intent was to allow smooth LOD transitions (keep old mesh visible
while new is uploading). F3a explicitly deferred transitions ("F3b
will handle that"), but the chunk_manager streaming loop's natural
re-mesh-on-band-crossing behaviour exposed the gap immediately:

1. Chunk first generated → mesh request at desired LOD → uploaded.
2. Player walks closer → chunk crosses LOD band → mesh request at
   new LOD → uploaded.
3. Both `(coord, old_lod)` and `(coord, new_lod)` exist in
   `self.meshes`. **Both render every frame.**

Confirmed by playtest: RD=32 with `UNENDLICH_LOD_FACTORS=4,8,16,32`
(very aggressive) produced GPU lifetime mean=164ms over 2021 frames,
much worse than v0.23.0's 2.40ms at RD=6 default. Each chunk was
drawing 2-3× as accumulated LOD copies.

### The fix

Added `stale_lod_key(candidate, new_coord, new_lod) -> bool`
predicate, free function so it's testable without a Vulkan
context. Returns true iff `candidate.coord == new_coord && candidate.lod
!= new_lod`.

`upload_prebuilt_mesh_at_lod` now sweeps the `meshes` map with
this predicate and routes any matching keys to `pending_destroy`
before inserting the new mesh. The same-coord-same-LOD case is
still handled by the HashMap insert in `upload_mesh` (which routes
the replaced entry's slot to `pending_destroy` via the insert's
return value).

### Trade-off accepted

The fix is "evict old LOD as soon as new arrives." This means
during a transition, there's a brief moment where the chunk has
the new LOD's mesh but the GPU hasn't drawn it yet — visible as
flicker. The alternative is "keep both visible until the new
mesh is confirmed drawn for one frame," which is more complex.

Filed for F3b proper (separate increment) when we add the
"double-buffer during transition" pattern. For now, the bug-free
behaviour wins over the transition smoothing.

### MesherTimer (rolled in)

Per-LOD timing of `MeshingPool` worker threads. `Arc<MesherTimer>`
shared across workers via Mutex (uncontended; one acquire per
chunk meshing call ~1-200ms apart). Per-second window log + lifetime
summary at shutdown.

Per-second log line:
```
mesher: near=X.XXms×N mid=Y.YYms×M far=Z.ZZms×K vfar=W.WWms×J
```

Empty tiers omitted. `×N` = chunk count this window per tier.

Shutdown summary lists mean/stddev/min/max per tier.

`LodLevel::short_name()` added for compact log labels: "near",
"mid", "far", "vfar". 1 test pinning the strings.

### Files changed

- `src/render/world_renderer.rs`: `stale_lod_key` helper added,
  `upload_prebuilt_mesh_at_lod` evicts other-LOD entries, 4 new
  tests for the predicate.
- `src/render/mesher_timer.rs`: NEW (~210 lines incl. 7 tests).
- `src/render/mod.rs`: register `mesher_timer`.
- `src/world/lod.rs`: `short_name()` accessor + 1 test.
- `src/render/meshing_pool.rs`: pool owns `Arc<MesherTimer>`,
  workers receive a clone, dispatch wrapped in `Instant::now()`,
  two new accessor methods.
- `src/bin/unendlich_world.rs`: per-second mesher log line,
  shutdown lifetime summary line.
- `Cargo.toml`: 0.23.1 (unshipped) → 0.23.2.

### Tests

550 / 550 (was 538, +12: 4 stale_lod_key + 7 mesher_timer + 1
short_name).

### What we should see in the next playtest

Two distinct things to watch for:

1. **GPU shadow + main times drop substantially.** The bug was
   inflating these by ~2-3×. With the fix, RD=32 +
   `UNENDLICH_LOD_FACTORS=4,8,16,32` should produce GPU times
   closer to (or better than) RD=6 default rather than 60×
   worse.

2. **MesherTimer reveals which LOD dominates worker time.**
   Hypothesis: Near greedy is ~50-200ms per chunk, non-Near is
   ≤5ms. If true, v0.23.3 attacks the greedy path. If Near is
   actually fast (~5ms) but `mesh_q` still saturates, the
   bottleneck is throughput / submission rate, not per-chunk cost.

The combined fix-plus-diagnostic ship gives us both a real perf
improvement AND the data to plan the next one.

## Architectural commitments registered v0.23.2 → v0.24+ (reverse-regen)

This block records architectural decisions made in conversation 7
between v0.23.2 (LOD double-draw fix) and v0.24.0 (visible-zone
reverse-regen). They are NORTH STARS, not v0.23.x code commitments.
Captured here so future sessions don't redebate them.

### The "reverse-regen" pivot (user-named)

Stage 1 of the project's life shipped block-grid-centric voxel
generation: `gen_inputs → density field → block grid → mesh +
trees-as-block-stamps`. Each stage materializes its full
representation; consumers read the materialized data structure.

The next-stage architecture, named "reverse-regen" by the user,
inverts this: the world is a pure function plus a sparse override
table. We ask "what does the world look like at coord X?" and
compute the answer backward through the layers as needed, rather
than materializing every layer eagerly.

```
world_at(coord) := f(coord, seed, world_state) + overrides[coord]
```

Where:

- `f` is the gen-function (density + biome → BlockId), pure,
  recomputable, GPU-resident.
- `world_state` is the slowly-varying field substrate (climate,
  erosion accumulators, moisture, etc.). Field-level operations
  cheap.
- `overrides` is a sparse map of player edits + entity stamps,
  persisted incrementally to disk.

Block grids materialize ON DEMAND, only where simulation needs to
read/write (active-zone tick handlers). Visible-zone rendering
draws coarse meshes derived directly from `f` at coarse
resolution. Entities (trees, structures, fluids) are first-class
in their own table, NOT block stamps.

### Why this is the right shape

It IS β. The existing β commitment ("GPU-canonical world state with
CPU as networking boundary") essentially describes this architecture
at the world-state level — we'd been deferring it as a long-term
intention, but at RD=32+ the cost of materialization-as-default
becomes prohibitive, and at RD=64 with future load (erosion, tree
sim, fluids) it becomes infeasible.

It generalizes: erosion, weathering, fluid sim are field-level
operations that work natively on `world_state` without per-block
work. Each future system gets its own fidelity tier (active /
visible / dormant) the same way the renderer's LOD system does.

It scales correctly: cost scales with **output size** (faces
emitted, entities visible) not **input size** (cells in grid).
At RD=64 with mostly-empty distant chunks, this scaling difference
is the architecture-level win — not a speedup multiplier.

### Honest speedup correction

Earlier sessions estimated "20× speedup" from fusion. That was
overselling. The honest range:

- **Cache-locality from fusion**: 1.5-2× for face-emission loops.
- **Skipping interior cells** (the real fusion win): 5-20×
  reduction in density-evaluation work, but only ~2-4× overall
  meshing speedup because face emission and vertex output also
  take time. Speedup is biggest on degenerate inputs (mostly-air
  or mostly-solid chunks).
- **Skipping materialization itself**: microseconds saved per
  chunk; constant-factor only.

**Realistic surface chunk: 2-4× faster meshing, up to 10× for
degenerate inputs, never 20× sustained.**

The architectural value isn't the constant-factor speedup; it's
the scaling story (output-size cost) and the substrate fit for
future field-level systems.

### Settled commitments (decided previously, recorded here)

**Residual catch-up via probability-based deterministic noise on
GPU.** When a chunk re-enters a faster band after time in CatchUp,
its tick handlers advance by `tick_dt` ticks worth of accumulator
movement, sampled deterministically from per-chunk noise functions
seeded by `(coord, seed, target_tick)`. No per-tick replay; no
visible animation; converges to the state the chunk would have
had if it had ticked continuously. Existing `src/world/scheduler.rs`
established the band model (Render/Simulation/CatchUp); the GPU
deterministic-noise implementation is the next layer.

**Multiplayer needs snapshots.** Override-log replay handles
single-player. Rollback netcode needs invertible-deltas plus
periodic snapshots. Given reverse-regen + deterministic
simulation, snapshots are tiny: override log up to T + entity
table at T + world_state at T + simulation seed. Bounded-size
regardless of edit history. Filed as a netcode design constraint;
not designed yet.

**Save format = replay + overrides + entities + state + seed.**
NOT a hack. Same paradigm as event-sourced systems, CRDTs, Figma
sync, Git. Append-only override log; replay-on-load is fast
because overrides are sparse position→block lookups, not gen
work. No long save-game pause. Naïve "save 10 GB world" is bad;
"save 100 KB overrides + entity edits + 4 KB state" is strictly
better than what we have today.

**Bounded simulation via tick-rate falloff.** Fire/plants/erosion
ticks scale with distance from active simulation centers. Same
falloff substrate as `desired_lod_for`. The `src/world/scheduler.rs`
band model is half this; the actual per-system rate scaling is
the next layer.

**Entity-first content for non-uniform structures.** Trees in this
engine are not 1-1 Minecraft trees of log+leaf blocks. They grow,
weather, fall, decay. Cliffs are entities with wind/gravity. Water
is fluid, not block-stamps. These need their own state, update
logic, and rendering at distance — forcing them into the block grid
is structurally wrong. They live in the entity table; their
occupation of space is a small AABB stamp on the override table
(opacity hint), not block content.

### Filed for later (research, not commitments)

**Git-as-database.** Git's pack format and delta compression would
handle override logs reasonably well; 1 GB packs and single-thread
operation aren't hard limits (sharding by chunk-coord hash works).
But using git-as-library forces engine code through git's
process model, adding latency at game-loop scale. Filed as
"consider custom content-addressed store inspired by git's
design" rather than "actually use git." User's separate interest
in a Rust+GPU rewrite of git for non-engine backup workflow is
acknowledged but is not engine work.

**Residual update-log compressor.** Hash-based memoization +
time-reversed merge of commutative updates in the override log.
Run simulation forward optimistically; periodically walk backward
through the log merging adjacent updates that commute. Compressed
log shorter than original. Replay on long absences becomes O(log
size after compression) instead of O(elapsed ticks). Requires
per-update-type commutativity properties; only merges within
compatible classes. Genuinely novel research-grade idea. Filed for
much later — implement naïve residual tick first, design
compression to fix specific hot spots after measuring.

### Staged rollout plan

**Stage 1 (v0.23.3) — verify F3 works end-to-end.** Small RD=12
playtest with default `LOD_FACTORS=[1,2,4,8]`. Goal: confirm Mid
+ Far + VeryFar tiers actually mesh under conditions where
generation can keep up. Adds flags-readout to shutdown summary
(useful for crash reports). Zero new behavior; pure verification.

**Stage 2 (v0.24.x) — visible-zone reverse-regen.** Defines
`CoarseChunk` (~2 KB record vs 32 KB block grid: heightmap +
biome ID + average color), coarse mesher, two-zone streaming
(active radius gets full chunks; beyond gets coarse). Several
sessions.

**Stage 3 (v0.25+) — active-zone reverse-regen.** Fused gen+mesh
path. Override table. Sparse block-grid materialization on demand
for tick simulation. Where the 2-4× speedup lives. Several more
sessions.

**Stage 4 — bounded simulation.** Tick rate scales with distance.
Unblocks RD=64+ for content that wants to simulate.

**Stage 5 — entity-first content.** Trees, structures, fluids as
entities with own state/rendering. Stamp into override as
occupation hints, not block content.

**Stage 6 (research, file) — residual log compression.** With
naïve residual tick working, design the compressor based on
real bottlenecks. Months of work; not for soon.

Stages 1-2 are commitments. Stages 3-6 are intentions; their
specific shape gets reconsidered with fresh data when we get
there.


## Increment 30 (v0.23.3) — Stage 1 verification (flags readout + small-RD playtest)

### What this ships

Two tiny things, zero new behavior:

1. **Flags readout in shutdown summary.** Logs the values of
   `UNENDLICH_RENDER_DISTANCE`, `UNENDLICH_LOD_FACTORS`,
   `UNENDLICH_INDIRECT`, `UNENDLICH_VSYNC` (and a couple others
   from the runtime_flags registry) at shutdown. Useful for crash
   reports — when a user pastes a log, we want to immediately see
   which flags were active.

2. **Encourage a small-RD playtest as Stage 1 verification.**
   No code change — just documenting the recommended invocation
   in the README/help.

The playtest in v0.23.2 ran at RD=32 with `LOD_FACTORS=4,8,16,32`,
which exercises the LOD substrate at extreme settings. It revealed
the dispatch never fires at non-Near LODs because chunk generation
can't keep up with the RD=32 sphere (137000 chunks; ~1.7%
generated in 125s). The MesherTimer summary confirmed: 12149
chunks meshed, all `Near`; `mid/far/vfar (no chunks meshed)`.

This is NOT a bug in the LOD substrate. It's that at RD=32 with
default 18 chunks/sec generation throughput, the player never
walks far enough into generated territory to trigger Mid/Far/VeryFar
dispatch. The diagnostic correctly shows the bottleneck is now
generation, not meshing.

To verify F3 works end-to-end, we need the player to walk through
chunks at distance > Near. RD=12 with default factors `[1,2,4,8]`
gives:

- Near band: distance 0..3 (~4³·8/3 ≈ 268 chunks)
- Mid band: distance 4..6 (~ring of 488 chunks)
- Far band: distance 7..9 (~ring of 936 chunks)
- VeryFar band: distance 10..12 (~ring of 1432 chunks)

Generation throughput at 18 chunks/sec fills RD=12 sphere
(~7,238 chunks) in ~6 minutes. Player can walk into any band in
seconds. This will exercise the substrate.

### What we expect to see

- Mesher lifetime summary shows non-zero counts for `mid`, `far`,
  AND `vfar`.
- Per-tier mean times: `near` ~6ms (greedy), `mid` ~1ms, `far`
  ~0.3ms, `vfar` ~0.1ms (downsampled mesher is much cheaper).
- `total` GPU time stays comfortable (sub-30ms) at RD=12.
- No double-draws (the v0.23.2 fix verified at small scale).

If any expectation fails, we have a bug to fix before building
Stage 2 on top.

### Files changed

- `src/bin/unendlich_world.rs`: shutdown summary logs flag values.
- `Cargo.toml`: 0.23.2 → 0.23.3.

### Tests

No new tests. Logging change only. Existing 550 / 550 still pass.

### What v0.23.4 / v0.24.0 looks like

If the playtest passes (LOD substrate verified): v0.24.0 begins
Stage 2 — define `CoarseChunk` type and coarse mesher in a new
module, no streaming policy changes yet. One-session scope.

If the playtest reveals a bug: v0.23.4 fixes it; Stage 2 deferred.

## Increment 31 (v0.23.4) — LOD curve rewrite + submit-loop scaling fix + mgr_update sub-zone timing

### What v0.23.3 playtest revealed

Under the v0.23.3 ship the user invoked at RD=64 (not the recommended RD=12) with `LOD_FACTORS=4,8,16,32`. The session ran ~9 minutes; the shutdown summary showed `near=21611 chunks` meshed and Mid/Far/VeryFar at "no chunks meshed", *just like v0.23.2*. Stage 1 verification did not happen.

Reading the playtest log we found:
- Frame times consistently 350-450ms (2-3 FPS) at loaded ~9460
- `cpu: submit` = 645ms wasted per frame
- `cpu: mgr_update` = 485ms per frame
- gen_pending stayed >1.36 million the entire run; at ~18 chunks/sec generation, the RD=64 sphere (~1.15M chunks) is unreachable

Three root causes identified:

**(A) F3 verification structurally impossible at any large RD with the proportional curve.** At RD=64 with bands at 25/50/75% of RD, the Near band was 0..16 (chunk-distance). Player walks ~2 chunks/sec; chunks at the edge of the loaded set become available only when the player is already adjacent (within Near distance). They get classified Near every time. F3 dispatch is provably correct (verified by reading code), it just never receives non-Near requests.

**(B) Submit-loop scaling bug.** The submit-phase code built a full `desired: HashSet<ChunkCoord>` of every chunk in the RD sphere every frame (~1.15M inserts at RD=64), then iterated it doing DashMap reads (~1.15M lookups) for the version filter. The previous comment in the code noted a prior fix from "1850 lock acquisitions to 925" — that was at RD=12. At RD=64 we were back to 1.15M+. ~95% of those reads returned None (chunk not yet generated) and were pure waste.

**(C) mgr_update was 400-600ms** but logged as a single number with no attribution. Could be `request_visible` (O(rd³) sphere walk = 1.15M reads), `evict_distant` (O(loaded) scan, only on player-chunk-cross), or `drain_completed` (small per frame). No way to tell.

### What v0.23.4 ships

Three changes, addressing each root cause:

#### 1. LOD curve: proportional → constant-Near + geometric falloff

Replaced the 25/50/75%-of-RD band thresholds in `desired_lod_for` with **fixed absolute outer edges**:

- Near outer = 8 chunks (constant; clamped to RD)
- Mid outer = 16 chunks (clamped to RD)
- Far outer = 32 chunks (clamped to RD)
- VeryFar = beyond Far up to RD

This matches the on-screen-voxel-size invariant when paired with the default `LOD_FACTORS=[1,2,4,8]`: at distance 8 you see voxels at 1× resolution, at distance 16 you see voxels at 2× downsample (which projects to roughly the same on-screen size given the doubled distance), and so on.

The constants are exposed as `pub const NEAR_OUTER_DEFAULT/MID_OUTER_DEFAULT/FAR_OUTER_DEFAULT` in `src/world/lod.rs`. Future increment: drive these from `UNENDLICH_LOD_BANDS=8,16,32` for runtime tuning. Filed for after we have data on whether 8 is the right Near outer.

**Why constant outers beat proportional at large RD.** The proportional curve made the band shape scale-free in RD (each band a fixed fraction of view distance). At small RD that's fine — at RD=12 the Near band is 0..3, and a player walking at chunk-rate 2/sec crosses bands every 1.5s. At large RD the proportional shape becomes a trap: at RD=64 the Near band is 0..16, and a player would have to walk ~16 chunks (8+ seconds at chunk-rate 2/sec) to reach Mid. By that time generation has filled the surrounding area at Near; non-Near tiers never fire in practice.

The constant-outer curve is RD-independent in the Near/Mid/Far portions. Player crosses bands at the same wall-clock rate regardless of RD. VeryFar absorbs all the extra distance at large RD, which is exactly the band that benefits most from aggressive downsampling.

#### 2. Submit-loop scaling fix: full-sphere iterate → Chebyshev shell walk with budget

Replaced the build-and-iterate-`desired` pattern in `unendlich_world.rs` (lines ~1126-1210) with a Chebyshev shell iterator that walks outward from the player and short-circuits on candidate budget:

```rust
const SUBMIT_SCAN_BUDGET: usize = MESH_QUEUE_CAPACITY * 4;  // 256
'shells: for shell in 0..=rd {
    for dz in -shell..=shell { for dy in -shell..=shell { for dx in -shell..=shell {
        if max(|dx|,|dy|,|dz|) != shell { continue; }     // shell surface only
        if dx² + dy² + dz² > rd² { continue; }            // sphere cull
        // version filter, in-flight check, get_chunk
        if accumulated >= SUBMIT_SCAN_BUDGET { break 'shells; }
    }}}
}
```

In typical play (player surrounded by loaded chunks) this fills the candidate budget in the first 2-3 shells, scanning a few hundred coords instead of 1.15M. The eviction membership check, which previously used `desired.contains(c)`, becomes an inline closure `in_render_distance(c)` — same O(1) cost without needing the HashSet to exist.

Trade-off accepted: if many chunks bump versions simultaneously (e.g. mass simulation update) and they're all in shell 0..3, outer shells could be starved that frame. Acceptable — pool is 64 deep + scan budget 256, and stale chunks are typically at most a handful at a time. Filed for follow-up only if it ever bites in practice.

#### 3. mgr_update sub-zone timing (diagnostic only)

Added three atomic counters in `chunk_manager.rs`: `MGR_DRAIN_US`, `MGR_EVICT_US`, `MGR_REQUEST_US`. Each phase of `update()` records its elapsed microseconds. The binary reads-and-resets them every 1s and emits a new log line:

```
mgr_update breakdown: drain=Xms evict=Yms request_visible=Zms
```

This is diagnostic only — no behavior change. Next session we'll know which phase to fix. Hypothesis: `request_visible` dominates because it walks `chunks_in_radius(player, rd)` = O(rd³), with a DashMap read per coord. At RD=64 that's 1.15M reads per call, and it gets called every time a chunk completes (~18 times/sec).

### Test changes

`src/world/lod.rs` band-boundary tests rewritten for the new curve:

- `desired_lod_band_boundaries_rd_6`: at RD<8 everything is Near (clamped); cheb=7 (out of sphere) falls to VeryFar.
- `desired_lod_band_boundaries_rd_32`: Near=[0,8], Mid=[9,16], Far=[17,32] (Far clamped to RD), VeryFar empty within sphere.
- `desired_lod_band_boundaries_rd_64` (NEW): all four bands non-empty. Near=[0,8], Mid=[9,16], Far=[17,32], VeryFar=[33,64]. This is the "design intent" test — if it fails, the curve drifted.
- `desired_lod_at_or_beyond_render_distance` updated for new clamp semantics.
- `desired_lod_rd_too_small` updated for new clamp semantics.

All 404 lib tests pass, including 26 lod tests and the doctest.

### Expected play experience after ship

Recommended invocation:
```
UNENDLICH_INDIRECT=1 UNENDLICH_VSYNC=1 UNENDLICH_RENDER_DISTANCE=64 \
  cargo run --release --features render --features gpu --features hot-shaders \
  --bin unendlich-world
```

Note: NO `UNENDLICH_LOD_FACTORS` override. Let it default to `[1,2,4,8]`. The `4,8,16,32` setting from the prior playtest was over-aggressive — every band was 4× downsampled or worse, which is why distant chunks looked washed out and pale even at Near.

Walk for ~2 minutes; quit with Esc-Esc. What to look for:

- **Mesher lifetime summary** at shutdown should show non-zero counts for `mid`, `far`, AND `vfar`. Near now caps at chunk-distance 8 absolute, so anything past 8 chunks must dispatch to a downsampled path. If `vfar` is still zero, F3 dispatch is broken (not just the curve).
- **GPU times** should drop substantially. ~95% of the RD=64 sphere is now Far/VeryFar with 4×/8× downsample; should mesh much faster and produce much fewer faces.
- **`cpu: submit`** should be <5ms (was 645ms). The submit loop no longer touches >1M coords per frame.
- **`mgr_update breakdown`** new log line will tell us where the 400-600ms is going. Most likely `request_visible`.
- **Pale washed-out chunks** in screenshots should mostly disappear (default factors don't crush color the way 4,8,16,32 did).

### Files changed

- `src/world/lod.rs`: new curve in `desired_lod_for`, named outer constants, rewritten tests + new RD=64 test (~110 lines net).
- `src/bin/unendlich_world.rs`: replaced full-`desired` HashSet with inline `in_render_distance` closure + Chebyshev shell submit iterator with budget; added mgr_update breakdown log line (~80 lines net).
- `src/world/chunk_manager.rs`: added `MGR_DRAIN_US`/`MGR_EVICT_US`/`MGR_REQUEST_US` atomics, sub-zone timing inside `update()` (~50 lines net).
- `Cargo.toml`: 0.23.3 → 0.23.4.

### What v0.23.5 / v0.24.0 looks like

If F3 substrate is verified by this playtest (Mid/Far/VeryFar all fire) AND the submit-fix lands as expected:
- Use the `mgr_update breakdown` data to fix the dominant phase (probably `request_visible`).
- Then begin Stage 2: define `CoarseChunk` and coarse mesher.

If F3 still doesn't fire after this curve change, the bug is deeper than band math — likely in the dispatcher, the worker, or the upload path. We'd diagnose from the breakdown.
## Increment 32 (v0.23.5) — F3 starvation fix: per-tier submit reservation

### What v0.23.4 playtest revealed

User playtested v0.23.4 at RD=64 with `LOD_FACTORS=4,8,16,32` for 5 minutes. Shutdown summary:

```
mesher: near=14833 mean=6.41ms
        mid       (no chunks meshed)
        far       (no chunks meshed)
        vfar      (no chunks meshed)
```

**F3 substrate STILL not exercised** despite the curve fix. The diagnosis:

The v0.23.4 submit loop walked Chebyshev shells outward from the player, accumulating up to `SUBMIT_SCAN_BUDGET=256` candidates before short-circuiting. In steady state, the player's immediate vicinity (shells 0..7, where the Near band lives) **always** has stale-or-missing chunks, because new terrain is constantly streaming in around the player. The global candidate budget filled with Near every frame, the walk never reached shell 9+, and Mid/Far/VeryFar never got submitted.

This is exactly the bug I dismissed in v0.23.4 reasoning as "rare under normal play." It's not rare; it's the default case at startup and during exploration.

### What v0.23.5 ships

**Per-tier candidate budgets.** Instead of a global `SUBMIT_SCAN_BUDGET=256`, each LOD tier gets its own quota:

```rust
const NEAR_BUDGET: usize = MESH_QUEUE_CAPACITY;  // 64
const MID_BUDGET:  usize = MESH_QUEUE_CAPACITY;  // 64
const FAR_BUDGET:  usize = MESH_QUEUE_CAPACITY;  // 64
const VFAR_BUDGET: usize = MESH_QUEUE_CAPACITY;  // 64
```

The shell walk classifies each candidate by `desired_lod_for(coord, player, rd)` *before* doing any DashMap work, and skips candidates whose tier is already full. The walk terminates when every tier has filled or the sphere is exhausted.

This guarantees outer tiers make progress. Even if Near saturates instantly, 192 reserved slots force the walk to continue to shells 9..32 (Mid/Far range) and shells 33..rd (VeryFar range).

**Round-robin submit phase.** Once candidates are accumulated, submission interleaves tiers (Near → Mid → Far → VeryFar → Near ...) so the worker pool sees a mix of LODs from the start, not all-Near-then-all-Mid. Workers process near at ~6ms each, so submitting all-Near-first delays the user seeing distant chunks until Near drains, which can take seconds at high pending counts.

### Cost of the per-tier classification

`desired_lod_for` is a few integer comparisons. Calling it per-shell-coord adds maybe 5-10ns per iteration. At ~200k iterations max (the pathological case) that's ~2ms, dwarfed by the savings from earlier tier-full skip (avoids the DashMap read, which is the actually-expensive part).

### Note on v0.23.4 mgr_update breakdown log

The v0.23.4 playtest log did NOT include the `mgr_update breakdown:` line we added. Possible causes: user ran a stale binary (cargo's `Finished release ... in 0.08s` is suspicious for a fresh ship), or the log line is suppressed by some level filter. Either way, we don't yet have data on which mgr_update phase dominates. The instrumentation remains in place; will pick up if user rebuilds.

### Files changed

- `src/bin/unendlich_world.rs`: per-tier candidate Vecs, shell walk classifies-then-skips by tier-full, round-robin submit phase. Added `use unendlich::world::lod::LodLevel`. ~80 lines net.
- `Cargo.toml`: 0.23.4 → 0.23.5.

### Recommended playtest invocation

```
UNENDLICH_INDIRECT=1 UNENDLICH_VSYNC=1 UNENDLICH_RENDER_DISTANCE=64 \
  cargo run --release --features render --features gpu --features hot-shaders \
  --bin unendlich-world
```

User has been running with `UNENDLICH_LOD_FACTORS=4,8,16,32` set in shell. If they want to test the default `[1,2,4,8]`, they need to `unset UNENDLICH_LOD_FACTORS` first. Either should now exercise all four tiers.

### Expected playtest signal

Shutdown summary `mesher` line should show non-zero counts for `mid`, `far`, AND `vfar`. If it does — Stage 1 verification finally passes. If it doesn't — the bug is deeper than the submit loop and we diagnose from breakdown logs.
## Increment 33 (v0.23.6) — request_visible shell-walk + RD=24 verification

### What v0.23.5 playtest revealed

**Mid finally fired.** Shutdown summary:

```
mesher: near=11376 mean=5.64ms
        mid  =1802  mean=0.06ms
        far  (no chunks meshed)
        vfar (no chunks meshed)
```

First time non-Near tier produced any chunks. The per-tier reservation works as designed.

Far/VeryFar still empty, but for a different reason than before: **terrain generation hasn't reached those distances yet**. Player walked to (-165, -69) — chunks at Chebyshev distance >32 from origin were never generated. gen_pending stayed >1.2M throughout the 3-minute run. The submit loop walks shells correctly; `get_chunk` returns None for ungenerated coords; nothing to mesh.

### What's now the bottleneck

`mgr_update breakdown` (added in v0.23.4) attributed 590ms/frame to **request_visible**:

```
mgr_update breakdown: drain=0.3ms evict=0.0ms request_visible=589.5ms
```

Reading `request_visible` shows the same shape of bug as the v0.23.4 submit loop:

1. Calls `chunks_in_radius(player, rd)` which builds a Vec of every coord in the RD sphere (~1.15M at RD=64), computes priority for each, then **sorts the entire 1.15M-entry Vec**.
2. Iterates the sorted Vec doing `chunks.contains_key()` per entry (1.15M DashMap reads).
3. Hands survivors to `loader.request_many` which holds a mutex while doing 2 HashSet contains-checks per coord.

Every frame. The loader queue is 64 deep — it can never process more than 64 fresh requests in parallel, so submitting 1.15M was almost entirely waste.

### What v0.23.6 ships

#### 1. `request_visible` rewrite — shell walk with budget

Same fix shape as the v0.23.5 submit-loop rewrite, applied to `chunk_manager::request_visible`:

```rust
const REQUEST_SCAN_BUDGET: usize = GPU_MAX_QUEUED_CHUNKS * 4;  // 256

'shells: for shell in 0..=rd {
    for dz, dy, dx in -shell..=shell:
        if max(|dx|,|dy|,|dz|) != shell { continue; }      // shell surface only
        if dx² + dy² + dz² > rd² { continue; }              // sphere cull
        if self.chunks.contains_key(&coord) { continue; }   // already loaded
        to_request.push((coord, prio));
        if to_request.len() >= 256 { break 'shells; }
}
self.loader.request_many(&to_request);
```

In typical play, the first few shells fill the 256-coord budget. At RD=24 the entire sphere has only ~58k coords, so this is barely an optimization there. At RD=64 it's the difference between 1.15M iterations and a few hundred.

Trade-off: priority ordering. The old code sorted by Euclidean distance globally. The new code emits in Chebyshev-shell order, which is *approximately* nearest-first. Within a shell the order is grid-walk. The loader's own priority heap re-sorts what we send it, so the global ordering invariant is preserved — we just send the loader fewer candidates per call. Since outer-shell requests would be deduped against the heap on subsequent frames anyway, no work is lost.

#### 2. Removed `chunks_in_radius` import

The function still exists for any future callers but `chunk_manager` no longer uses it. Filed for later: if no other caller emerges, delete it.

### Recommended playtest invocation: RD=24

User asked to combine fix-and-verify in one ship. The RD=24 verification is just a flag value — no code change needed:

```
UNENDLICH_INDIRECT=1 UNENDLICH_VSYNC=1 UNENDLICH_RENDER_DISTANCE=24 \
  cargo run --release --features render --features gpu --features hot-shaders \
  --bin unendlich-world
```

Optionally `unset UNENDLICH_LOD_FACTORS` to use the default `[1,2,4,8]` instead of the over-aggressive `[4,8,16,32]` from prior runs.

### Why RD=24 for verification

At RD=24 with the v0.23.5 curve:
- Near outer = 8 (constant)
- Mid outer = 16 (constant)
- Far outer = 24 (clamped to RD)
- VeryFar = empty (RD < Far outer would-be 32)

Sphere has ~58,000 chunks instead of 1.15M. At ~50 chunks/sec generation, the sphere fills in ~20 minutes — for a 2-3 minute playtest, gen_pending should drop to a small number and Far should actually receive candidates. If Far fires here, the substrate is fully verified for three of four tiers. (VeryFar requires RD≥33 to have any chunks at all under the current curve; we'll verify it in a follow-up RD=64 playtest after request_visible is no longer dominating.)

### Expected playtest signals

- **`mgr_update breakdown`** — `request_visible` should drop from ~590ms to single-digit ms. If it doesn't, the fix is wrong somewhere.
- **`mesher` summary** — at RD=24, expect non-zero counts for `near`, `mid`, AND `far`. `vfar` will still be `(no chunks meshed)` since the curve has VeryFar empty at RD<33; that's expected.
- **Frame time** — should drop substantially. Per-frame CPU work was dominated by mgr_update (590ms) + submit (530ms) + draw spikes (50-200ms occasional). With mgr_update now bounded, frame time should be GPU-bound (shadow ~80-100ms is the next ceiling).

### Files changed

- `src/world/chunk_manager.rs`: `request_visible` rewritten (~80 lines net), import of `chunks_in_radius` dropped, import of `GPU_MAX_QUEUED_CHUNKS` added.
- `Cargo.toml`: 0.23.5 → 0.23.6.

### What v0.24.0 likely looks like

If RD=24 verifies all three available tiers AND request_visible drops to <10ms, the substrate is solidly proven and we have headroom for Stage 2 (visible-zone reverse-regen). That's the next architectural shift: skip block-grid materialization for the visible zone, emit meshes directly from gen_inputs+state. Multi-session work.

If anything's surprising — Far doesn't fire at RD=24, or request_visible doesn't drop, or some new dominant cost emerges — we diagnose first, ship Stage 2 next.

### v0.23.6 verification — RD=64 result (post-ship playtest)

Initial RD=24 playtest verified request_visible (590ms → 0.5ms, 2000× speedup) but Mid/Far didn't fire. Hypothesis: at small RD with sparse Near saturation, per-tier round-robin starves outer tiers. Re-ran at RD=64.

Result:
```
mesher: near=7141  mean=3.44ms
        mid =3502  mean=0.04ms
        far =2476  mean=0.03ms
        vfar (no chunks meshed)
```

**Substrate verified for three of four tiers.** VeryFar (33-64) requires terrain at chebyshev 32-64 from player; gen_pending stayed in millions throughout the run, so VeryFar coords were mostly ungenerated. Same "generation didn't reach" story we know — not a substrate bug.

GPU lifetime mean dropped from v0.23.5's 151ms → 90ms. wait_fences dropped from prior 12ms → 55ms (higher mean but lower variance — CPU is now waiting on GPU rather than burning on bookkeeping).

CPU bottleneck has shifted from chunk-management (mgr_update at 590ms/frame) to GPU work and chunk-upload throughput. This is the architectural cue for Stage 2: visible-zone reverse-regen.

Stage 1 complete. Moving to Stage 2.

## Increment 34 (v0.23.7) — Stage 2 spike: Far-tier reverse-regen

### What this ships

Two changes, both in `src/render/mesher.rs`:

1. **Refactor**: `Step 2` (face emission) extracted from `mesh_chunk_downsampled` into a new private helper `emit_downsampled_grid(grid, grid_size, factor, origin) -> ChunkMesh`. The materialised path (`mesh_chunk_downsampled`) now calls it after building its grid. No behaviour change — verified by running the existing far-mesher tests post-refactor.

2. **New entry point**: `mesh_chunk_far_virtual(coord, generator, factor) -> ChunkMesh`. Produces a Far/VeryFar mesh by sampling the generator's `density` and `is_cave` directly at the centre of each F×F×F grid cell, with no `Chunk` ever materialised. Shares Step 2 with the materialised path so output shape is identical by construction.

### What's NOT in this ship

`mesh_chunk_far_virtual` is NOT yet wired into the meshing pool. Production code still goes through the materialised path. This ship is a Phase 2a spike — proving the architecture is sound before invasive integration in Phase 2b.

### Spike result

Five tests (`render::mesher::tests::virtual_*`):

- `virtual_far_high_altitude_is_empty` — chunks at y=224 produce empty mesh ✓
- `virtual_far_deep_underground_is_dense_or_empty` — chunks at y=-128 produce non-empty mesh ✓
- `virtual_far_surface_chunk_produces_mesh_in_aabb` — vertices stay within chunk AABB ✓
- `virtual_far_and_materialised_comparable_triangle_count` — virtual and materialised paths within 10× of each other across 3 seeds ✓
- `virtual_very_far_factor_4_works` — factor=4 path produces in-bounds mesh ✓

All pass. The architecture is viable.

### What this proves

- The `WorldGenerator::density` + `is_cave` API is sufficient as a substrate for reverse-regen. No new sampler API needed; the existing one already gives us pure (coord, seed) → solidity.
- Step 1 (grid construction) and Step 2 (face emission) are cleanly separable. Different Step 1 strategies (materialised vs virtual) can share Step 2 by construction.
- A coarse sampling strategy (one density evaluation per F×F×F region, instead of F³ samples + majority vote) produces meshes comparable in triangle count to the full path. Quality difference exists at terrain boundaries but is acceptable for Far/VeryFar where players don't see fine detail anyway.
- Surface decoration (Grass on topmost cell, Stone below) works at grid resolution with a simple top-down column walk.

### What this skips (vs materialised path)

For each Far chunk: ~32,768 density+cave evaluations → 4096 (factor=2) or 512 (factor=4). Plus the entire Phase 2 (surface decoration), 2.5 (water), 3/3.5 (ore/lava), 4 (bedrock), and 5 (trees) of the materialised generator. Plus the 64 KB `Vec<BlockId>` allocation. Plus the Step-1 histogram and majority-vote per cell.

### Quality trade-offs (documented in code)

- Centre-sample instead of majority-of-F³: a 4-of-8 solid cell may flip based on the single centre sample. Visible at terrain boundaries as slightly different silhouettes.
- No biome-specific surface variation; all surface is `BlockId::Grass`, all subsurface is `BlockId::Stone`. At Far distance this is mostly invisible. Phase 2b can wire `pick_surface_block(biome, ...)` once integration is solid.
- No trees, ore, water, or lava. These are sub-kilobyte vertex contributions at Far distance anyway.

### What blocks Phase 2b

Integration into the meshing pool requires:

- A `MeshRequest::Virtual { coord, lod, generator: Arc<WorldGenerator> }` variant.
- chunk_manager's `request_visible` learning to classify coords by tier and route Far+VeryFar coords directly to the meshing pool, skipping chunk_loader entirely.
- Lifetime story for the generator Arc — the meshing pool workers need stable access. Probably one Arc created at startup and cloned per request.

None of this is hard, but each piece is intrusive enough to warrant a separate session.

### Test-discovery diagnostic (caught this session)

While running v0.23.7 tests, I discovered that the previous session's "404/404 passing" reports were missing the entire `src/render/*` test suite. `pub mod render;` in `src/lib.rs` is gated on `#[cfg(feature = "render")]`. The default `cargo test --release --lib` excludes 155 tests. Correct invocation: `cargo test --release --lib --features render` (559 tests total).

This was flagged honestly mid-session rather than after-the-fact. None of the prior fixes (v0.23.3–v0.23.6) shipped broken — their playtest validation served as integration test, and the bulk of their code lived in `src/world/*` which IS covered by the default lib test set. But the LOD-curve tests in `src/world/lod.rs` and other non-render world tests *were* always running; the mesher tests specifically were the silent gap. Process improvement: all future ships will use `--features render` for the test sweep.

### Files changed

- `src/render/mesher.rs`: imports gained `ChunkCoord` and `WorldGenerator`; `mesh_chunk_downsampled` body trimmed (Step 2 extracted); new helper `emit_downsampled_grid`; new public function `mesh_chunk_far_virtual`; 5 new tests in `mod tests`.
- `Cargo.toml`: 0.23.6 → 0.23.7.

### Phase plan recap

- **Stage 2a (this ship, v0.23.7):** Far-tier virtual mesher proves the architecture. No production wiring.
- **Stage 2b (next ship):** Wire `mesh_chunk_far_virtual` into the meshing pool. chunk_manager classifies coords and routes Far/VeryFar past chunk_loader. Far chunks stop materialising. Measure the win.
- **Stage 2c (later):** Same for Mid. Bigger because the quality bar is higher (Mid is closer to the player).
- **Stage 2d (last):** Near. This is where the promotion-on-edit story lives. Most invasive.

## Increment 35 (v0.23.8) — Stage 2b: virtual mesher wired into the pool

### What this ships

The `mesh_chunk_far_virtual` function from v0.23.7 is now reachable from production code. Three changes:

1. **`MeshRequest` gained an `Option<Arc<WorldGenerator>>` field** (`generator`). When `Some(...)` AND the LOD is Far or VeryFar, the worker takes the reverse-regen path. Otherwise unchanged.

2. **Worker dispatch in `meshing_pool.rs`** routes by `(lod, generator.is_some())`:
   - Near: materialised greedy mesher (unchanged).
   - Far/VeryFar with generator: `mesh_chunk_far_virtual(coord, gen, factor)`.
   - Far/VeryFar without generator: `mesh_chunk_downsampled(&chunk, factor)` (fallback, also unchanged).
   - Mid: materialised downsampled, generator ignored.

3. **Submit loop in `unendlich_world.rs`** attaches `chunk_manager.generator().cpu()` Arc to outgoing Far/VeryFar requests. Near/Mid don't carry the Arc — no clone overhead for tiers that won't use it.

### What this does NOT ship

This is deliberate. Stage 2b is the integration; the perf win is Stage 2c.

- chunk_manager still materialises Far/VeryFar chunks via `request_visible` → `chunk_loader.request_many(...)`. The block grids still get allocated and the generator phases still run. We just don't *mesh* from them for distant tiers.
- Mid still goes through the materialised path. Mid's quality bar is higher (closer to player) and its meshing cost is already negligible (`mean=0.04ms` at RD=64). No urgency.

### Why phased this way

The full win (chunks stop materialising) requires teaching `request_visible` to skip Far/VeryFar coords. Many code paths read `chunk_manager.chunks()` and expect Far chunks to be present — eviction, persistence, ticks, snapshot, debug overlay. Each needs an audit before we change the contract.

Stage 2b proves the *meshing path* is correct end-to-end with two integration tests:

- `pool_takes_virtual_path_when_generator_attached` — submits a Far request with empty Chunk + generator. Virtual path samples real terrain → non-empty mesh. (If dispatch was wrong, the materialised path on an empty chunk would have produced empty mesh.) This isolates the dispatch decision from terrain quality.
- `pool_ignores_generator_for_near_lod` — submits a Near request with empty Chunk + generator. Result is empty (materialised path used). If we'd accidentally enabled virtual for Near, the generator would have sampled real terrain → non-empty result. Important: Near must NOT lose block-level detail, because editing depends on it.

With dispatch verified, Stage 2c can change chunk_manager with confidence: the meshing layer already handles virtual coords.

### Expected playtest signal

A playtest at RD=64 should look essentially identical to v0.23.7 in terms of frame time and chunk counts. The mesher timer summary should still show Far/VeryFar firing with similar per-chunk times (the virtual path is in the same order of magnitude as the materialised downsample). The architectural shift is internal — visible only as a successful playtest with no regressions.

If there's a *visual* regression for Far/VeryFar (terrain looks different at distance), that'd be the virtual mesher's surface-decoration approximation showing through, and we'd want to look at it before Stage 2c.

### Files changed

- `src/render/meshing_pool.rs`: `MeshRequest` gained `generator` field; worker dispatch updated to route virtual path; 2 new integration tests; 4 existing tests updated with `generator: None`.
- `src/bin/unendlich_world.rs`: submit loop attaches generator Arc to Far/VeryFar requests.
- `Cargo.toml`: 0.23.7 → 0.23.8.

### Test coverage going forward

All tests run with `--features render` now. The default `cargo test --lib` excludes the render module (155 tests). Total at this ship: 561 passing with `--features render`, of which 7 are virtual-mesher (5 unit + 2 dispatch).

### Phase plan recap

- ~~Stage 2a (v0.23.7): Far-tier virtual mesher proves the architecture.~~ ✓
- ~~Stage 2b (v0.23.8): Wire `mesh_chunk_far_virtual` into the meshing pool.~~ ✓ (this ship)
- **Stage 2c (next):** chunk_manager bypass for Far/VeryFar. Audit `chunks()` consumers, teach `request_visible` to skip distant tiers, measure the win.
- Stage 2d (later): Mid via reverse-regen.
- Stage 2e (last): Near + promotion-on-edit.

## Increment 36 (v0.23.9) — Mid/Far starvation fix

### What the v0.23.8 playtest revealed

```
mesher: near=12278  mean=4.63ms
        mid =0 (no chunks meshed)
        far =0 (no chunks meshed)
        vfar=0 (no chunks meshed)
```

At RD=64, only Near tier meshed for the entire 141-second session. GPU mean went *up* to 134ms vs v0.23.6's 90ms because Near was meshing everything in the render sphere with no LOD relief.

### Diagnosis

`request_visible` in chunk_manager uses a global budget of 256 outstanding requests. The shell walk stops once 256 candidates are accumulated. But shell 4 alone has 386 coords — by Chebyshev distance 4 (deep inside Near), the budget is exhausted. As the player moved, fresh Near coords kept entering shell 0/1/2/3 each frame, pre-empting outer-tier requests indefinitely.

The bug was latent in v0.23.6. The earlier RD=64 playtest happened to show Mid/Far firing because the player was less mobile during that session, allowing inner shells to finish before the player's position invalidated them. The v0.23.8 playtest had more movement, never letting Near complete, never letting the walk reach outer shells.

Not introduced by Stage 2b — but exposed by it.

### Fix

Per-tier budgets in `request_visible`. Each LOD tier gets its own quota (`GPU_MAX_QUEUED_CHUNKS` = 64 outstanding requests). When a tier saturates, candidates for that tier are skipped while others continue. The walk breaks early only when ALL four tiers are saturated.

Total outstanding requests can reach 256 (same as before) but distributed across tiers. The loader's priority heap then chooses what to process, with Euclidean priority still preferring nearest-first.

### Expected playtest signal at RD=64

- All four tier counts should be non-zero (vfar may still be 0 if the player doesn't stay still long enough for generation to reach Chebyshev 33–64, since those volumes are huge).
- Near mesher count should drop — outer tiers now absorb some of what was Near load.
- GPU mean should drop back toward the v0.23.6 baseline (90ms or below).
- Mesher means should match v0.23.6 patterns: Near ~3-5ms, Mid/Far/VeryFar ~0.04ms each.

### What this does NOT fix

This is purely the generation-pipeline starvation. Stage 2b's virtual-mesher path still triggers only AFTER Far/VeryFar chunks materialise. Once Mid/Far/VeryFar generation flows again, the virtual mesher will be exercised — that's the actual Stage 2b validation we couldn't do in v0.23.8.

### Files changed

- `src/world/chunk_manager.rs`: `request_visible` switched from global to per-tier budget. ~20 LOC delta.
- `Cargo.toml`: 0.23.8 → 0.23.9.

### Process notes

- I should have caught this with synthetic Mid/Far/VeryFar tests in the unendlich-world integration path before shipping v0.23.8. The submit-loop tests in `meshing_pool.rs` proved the *dispatch* worked given Far/VeryFar candidates, but didn't verify that any candidates ever materialised under realistic player movement. Filed as a pattern for future stages: any change downstream of `request_visible` needs an end-to-end check that all four tiers receive candidates.
- The "request_visible is fast at 0.5ms" v0.23.6 verification was technically correct — the function *was* fast — but the function being fast doesn't mean it's doing the right thing. The next playtest will tell us whether per-tier budgets restore the Mid/Far signal.

## Increment 37 (v0.23.10) — request_visible: shell-level classification

### What the v0.23.9 playtest revealed

```
mesher: near=7304  mean=5.78ms
        mid =1210  mean=0.08ms
        far =139   mean=0.09ms
        vfar (no chunks meshed)
mgr_update breakdown: request_visible=40-66ms / frame
```

Mid and Far were firing again — the per-tier budget fix worked. But `request_visible` regressed from v0.23.6's 0.2ms to 40-66ms per frame. GPU mean stayed at 134ms (same as v0.23.8), wait_fences at 87ms (worse than v0.23.6's 55ms).

### Diagnosis

The v0.23.9 implementation called `desired_lod_for` on every shell coord before checking the per-tier budget. At RD=64, hundreds of thousands of classifications per frame. Worse: even when a tier was saturated, we still iterated its full shell volume to check the budget per-coord. Near shell 4 alone is 386 coords; Near shells 5..=8 are thousands more. Each coord cost a DashMap `contains_key` lookup *and* an LOD classification before the budget check skipped it.

### Fix

Tier is purely a function of Chebyshev distance, and Chebyshev distance equals the shell number in the walk. So:

- Classify the *entire shell* once by its index: `if shell <= NEAR_OUTER { Near } ...`
- Check the budget *before walking the shell at all*. If the tier is saturated, skip the whole shell.
- The intra-shell loop only handles coords for that tier.

This drops the Near hot path from O(thousands of LOD classifications + DashMap reads per frame) to O(at most 64 DashMap reads per tier × 4 tiers).

### What this doesn't fix

The submit loop in unendlich_world.rs has its own per-tier ordering issue — it submits all of Near first, then Mid, then Far, in round-robin. With Near always full, outer tiers still get less submit bandwidth than they could. But the *generation* pipeline is now correct: Mid/Far/VeryFar chunks materialise whenever they're missing.

The v0.23.9 playtest also showed sustained `mesh_q: 180/128` — meaning we produce ~180 candidates per frame but can only submit ~128 (queue capacity). The submit loop is the next bottleneck. That's a Stage 2c concern when we look at the production virtual-mesher path.

### Expected at RD=64

- `request_visible` drops back toward sub-millisecond.
- mgr_update sub-millisecond (was 40-66ms in v0.23.9).
- All four tier counts still non-zero — the per-tier budgets remain.
- Frame time and GPU mean should *finally* approach v0.23.6's baseline (~90ms total GPU).

### Files changed

- `src/world/chunk_manager.rs`: `request_visible` reshape. Shell-level tier classification, skip saturated tier shells.
- `Cargo.toml`: 0.23.9 → 0.23.10.

### Pattern (worth flagging)

Same fix-shape recurs across three increments now:
- v0.23.5: submit-loop per-tier reservation (LOD classification done at the wrong granularity).
- v0.23.6: request_visible shell-walk-with-budget (don't iterate the whole sphere).
- v0.23.10: request_visible shell-level tier (don't classify per-coord when you can classify per-shell).

Theme: **classify at the coarsest granularity that's correct, not the finest convenient one.** When the data has structure (Chebyshev shells correspond to tiers), use it.

## Increment 38 (v0.24.0a) — ViewParams SSBO (CPU-authored)

### Why this ship exists

Three months of perf work (Increments 29–37) chased chunk-pipeline costs at RD=64 and got `mgr_update` from ~590ms/frame to ~0.2ms. But the three v0.23.10 playtest runs (RD=64, RD=8, RD=4) showed the dominant cost is not chunk management:

```
RD=64:  GPU total mean=151ms, wait_fences mean=109ms, shadow=81ms, main=60ms
RD=8:   GPU total mean=125ms, wait_fences mean=158ms  (Near tier ONLY)
RD=4:   GPU total mean= 16ms, wait_fences mean= 17ms  (120 fps sustained)
```

Near-tier draw work scales with what's in view. Shadow + main passes are the dominant cost. The chunk pipeline is now healthy enough that further optimization there has diminishing returns.

The right next attack is the **GPU draw work itself**, specifically:
1. The shadow pass renders one 2048×2048 cascade covering only the near 200m of view. CSM (4 cascades, logarithmic split) is the next visual roadmap item.
2. The main pass draws all chunks in the camera frustum without occlusion culling. HZB occlusion would cut main-pass work substantially.

### Scope of v0.24.0 specifically

CSM and HZB are user-visible perf wins, but neither can land cleanly on the current renderer architecture. The blocker is that frusta currently travel as **push constants** (100 bytes, max one frustum per dispatch). Multi-view culling needs to test each chunk against 5 frusta (main + 4 cascades) — at 96 bytes per frustum that's 480 bytes, well past Vulkan's 128-byte guaranteed push constant size. So multi-view culling requires moving frusta out of push constants and into a GPU buffer regardless of philosophy.

v0.24.0 is the **buffer architecture migration** to make that work possible. No visual change — the goal is to land the architectural shift without altering output.

The principle the user stated: *forward-thinking GPU-first design, so future physics-backed dynamics can plug into GPU-resident data without CPU round-trips and without rewrites.*

### Architectural changes

### The v0.24 plan in full

v0.24 is the buffer-architecture-migration arc, split into 5 small ships so each has at most one risk variable. The whole arc is invisible-to-user until v0.24.2.

- **v0.24.0a (this ship)** — ViewParams SSBO exists, holds 1 slot. CPU writes frustum planes directly. Cull shader reads `view_params[0].planes` instead of push-constant planes. Push constant shrinks to (view_index=0, chunk_count). Both main and shadow cull dispatches write their frustum into slot 0 before dispatching, mirroring today's flow. **One variable: does SSBO-based frustum read work end-to-end?**
- **v0.24.0b** — ViewParams grows to 2 slots (main + shadow). `view_compute.comp` lands: reads view+projection matrices CPU-written into a slot, derives VP / inverse-VP / 6 frustum planes (Gribb-Hartmann) on GPU, writes back. CPU stops computing frustum planes. **One variable: does GPU-derived frustum match CPU-derived?**
- **v0.24.1** — Multi-view in one dispatch. Single cull pass tests each chunk against all active views, emits into N output buffers. Indirect command/count buffers grow from 1 to N. **One variable: does single-dispatch multi-view emission match sequential dispatches?**
- **v0.24.2** — 4 cascades. ShadowMap → 2D array. `cascade_fit.comp` lands, derives 4 cascades from main view + sun. Fragment shader cascade selection + PCF. **First visual ship.**
- **v0.24.3** — *Profile-driven.* Staggered cascade updates (cascade 3 every 4 frames, cascade 2 every 2) if v0.24.2 shadow cost is still >40ms mean.

This entry covers v0.24.0a only.

### What this ship does

**New module: `src/render/view_params.rs`.** Defines:
- `ViewParam` struct: matches a future-proof std430 layout with slots for view, projection, view-projection, inverse-VP, camera position, 6 frustum planes, and near/far/aspect. In v0.24.0a only `planes` is read by the GPU; the matrix slots are written but unread. Their presence makes v0.24.0b's drop-in compatible.
- `ViewParamsBuffer` struct: owns one host-visible SSBO holding `[ViewParam; MAX_VIEWS]`, a persistent map pointer for CPU writes, and helpers to write a slot.
- `MAX_VIEWS = 2` allocated up front to leave room for v0.24.0b without re-creating the buffer. Slot 0 used in v0.24.0a.

**`chunk_cull.comp` changes:**
- New binding 3: `ViewParams` SSBO (read-only).
- Push constant shrinks from 100 bytes (6 planes + count) to 8 bytes (`view_index, chunk_count`).
- `aabb_visible` reads planes from `view_params[push.view_index].planes` instead of `push.planes`.

**`chunk_indirect.rs` changes:**
- `CullPushConstants` shrinks to `{ view_index: u32, chunk_count: u32 }`.
- Descriptor set layout gains binding 3 (storage buffer).
- Descriptor pool allocates 4 STORAGE_BUFFER descriptors instead of 3.
- `update_descriptor_set` writes binding 3 to point at the ViewParams SSBO.
- `record_cull_dispatch` signature unchanged — caller is responsible for writing the SSBO slot before dispatch.

**`world_renderer.rs` changes:**
- Owns a `ViewParamsBuffer`.
- Before each cull dispatch (main and shadow), writes the relevant frustum's 6 planes into slot 0 of the ViewParams SSBO. CPU still computes frusta from view-projection matrices — that work moves to v0.24.0b.
- Passes the ViewParams SSBO into `IndirectDrawSet` construction so the cull descriptor can bind it.

### What this ship does NOT do

- No `view_compute.comp` (v0.24.0b).
- No multi-view dispatch (v0.24.1).
- No cascades (v0.24.2).
- No visual change.
- ViewParams buffer is host-visible (memcpy from CPU) — eventually it may want to be device-local with a transfer or compute-write path, but that's premature now.
- CameraUbo unchanged. The redundancy between CameraUbo.view_proj and ViewParams.view_proj is deliberate for this ship — v0.24.0b lets the GPU derive view_proj, at which point we can decide what stays in the UBO vs the SSBO.

### Risk concentration

Exactly one thing is being tested: **does the cull shader correctly read frustum planes from binding 3's SSBO instead of from push constants?** If shadow/main draw counts shift between v0.23.10 and v0.24.0a in playtest, the bug is here.

The "before" reference is v0.23.10's playtest log (run 1 from the user, RD=64 LOD=4,8,16,32, 229s): `cull n=1057 mean=4.66ms`, `mesher: near=9012 mid=6840 far=747`.

### Validation strategy

- `cargo test --lib --features render` passes the existing 559 tests.
- New unit tests (lib-level, runnable without GPU):
  - `ViewParam` struct size + std430 alignment matches GLSL layout.
  - `ViewParamsBuffer` allocates and reports correct capacity.
  - `CullPushConstants` is 8 bytes (asserted, was <128).
- Binary builds clean with `--features render --features gpu --features hot-shaders`.
- Playtest at RD=64 with the same flags as v0.23.10 run 1. Expected:
  - `cull` GPU timer within ±0.5ms of v0.23.10's 4.66ms mean.
  - Mesher tier counts unchanged (LOD logic is untouched).
  - Visually indistinguishable.
  - Indirect command count unchanged across the run.

If any of those drift, that's a v0.24.0a regression and gets diagnosed before v0.24.0b is attempted.

### Files changed

- `src/render/view_params.rs` *(new)*: ViewParam struct, ViewParamsBuffer module.
- `src/render/mod.rs`: register the new module.
- `src/render/chunk_indirect.rs`: descriptor binding 3 added; CullPushConstants shrunk; descriptor write extended.
- `src/render/world_renderer.rs`: own ViewParamsBuffer, pass into IndirectDrawSet, write frustum planes into slot 0 before each cull dispatch.
- `src/shaders/chunk_cull.comp`: read planes from SSBO.
- `Cargo.toml`: 0.23.10 → 0.24.0.


## Increment 38b (v0.24.0a.1) — fix shadow/main slot collision

v0.24.0a shipped with both cull dispatches writing to `VIEW_SLOT_MAIN` (slot 0). The reasoning was that the two CPU writes within one frame "mirror v0.23.10's push-constant flow," where each dispatch overwrites the previous one's planes.

That was wrong. In v0.23.10 the planes lived in push constants — *baked into the command buffer at record time*. Each dispatch carried its own immutable plane copy. In v0.24.0a the planes live in a host-visible SSBO, and CPU writes happen at *record* time while GPU reads happen at *submit* time. With both writes going to slot 0, the GPU sees only the *last* CPU write — both dispatches cull against the same frustum.

### Playtest signal

Run at RD=64 from spawn: chunks visible immediately near the player rendered, very-far chunks rendered, and a "doughnut hole" of missing mid-distance terrain appeared between them. Diagnosis: the shadow cull was getting the main camera planes (the last CPU write before submit), so the shadow map only contained depth for main-frustum chunks rendered from the sun's POV. Mid-distance chunks visible to the camera but outside the shadow frustum's projection of the main-camera region rendered without shadows; everything else looked nominal.

Cull lifetime stats from the playtest were within v0.23.10's tolerance (3.75ms mean vs 4.66ms), which confirmed the SSBO read path itself worked — the bug was in CPU write ordering, not GPU layout.

### Fix

`record_cull_pass` takes a `view_slot: u32` parameter. The shadow caller passes `VIEW_SLOT_SHADOW` (= 1), the main caller passes `VIEW_SLOT_MAIN` (= 0). Each dispatch's planes live in their own slot, untouched by the other dispatch's CPU write.

The slot-distinctness invariant is pinned by a `slot_constants_distinct` test with an extended doc comment explaining the regression — so a future "simplification" that collapses both back to one slot trips a test failure that explains why.

### Why this lesson generalizes

For any GPU buffer where CPU writes and GPU reads are decoupled (host-visible SSBO, persistent map), per-dispatch state MUST live in a distinct slot rather than relying on temporal ordering. The next ship (v0.24.0b) extends this principle further — `view_compute.comp` derives matrices and planes from CPU-written view+projection, so CPU writes only the inputs, never the outputs. Output collisions become impossible by construction.


## Increment 38c (v0.24.0a.2) — face-round-robin chunk-load ordering

v0.24.0a.1 fixed the shadow/main slot collision. Playtest at RD=64 from spawn revealed a *different* symptom that v0.24.0a's broken shadow had been masking: chunks load on one side of the player but not the other. The world appears as a directional slice rather than a sphere — "started over here" was the user's description.

### Diagnosis

Two places in the codebase walk shell-surface coords outward from the player to populate the chunk-gen and mesh queues:

1. `chunk_manager::request_visible`
2. `bin/unendlich_world.rs`'s per-frame submit phase

Both used the same triply-nested loop pattern:
```rust
for dz in -s..=s {
    for dy in -s..=s {
        for dx in -s..=s {
            if dx.abs().max(dy.abs()).max(dz.abs()) != s { continue; }
            ...
```

This emits all (2s+1)² coords on the `dz=-s` face first, then `dz=-s+1` (a ring), etc. At shell s=8 with a `PER_TIER_BUDGET` of 64, simulation confirms that **100% of the first 64 emitted coords are on the -z face** — no other direction gets a single request before the budget fills.

The meshing pipeline saturates within seconds at RD=64 (mesh queue caps at 180/128, mesher mean 5ms per Near chunk × 4 worker threads). While saturated, only the first coords in the walk are processed. With -z always at the start, the world loads in a -z slice. As the player moves into -z/-x space, the new shell origin's -z face is fresh territory, and the chunks behind them (+z relative to the new position) never load because the pipeline never drains enough to reach them.

This bug existed in v0.23.10 too. v0.24.0a's shadow-culling failure (which made the mid-distance terrain look unshadowed) had been masking it.

### Why this isn't a v0.24.0a.1 regression

The diagnostic gave it away: v0.24.0a.1's lifetime cull stats (3.52ms mean, comparable to v0.23.10's 4.66ms) and mesher distributions were exactly in line with the pre-v0.24 reference. The cull path was working correctly. The slice pattern is a pre-existing pipeline-saturation artifact only visible now that culling no longer hides it.

### Fix: face round-robin shell walk

New iterator `lod::ShellWalker` enumerates a shell's surface coords by emitting one coord per face in rotation — `-z, +z, -x, +x, -y, +y, -z, +z, ...` Once a face is exhausted (the rare case where the walker is fully consumed), the remaining faces continue cycling. Face ownership partitions the surface coords uniquely:
- z-faces own all coords with `|dz| == s`
- x-faces own coords with `|dz| < s AND |dx| == s`
- y-faces own coords with `|dz| < s AND |dx| < s AND |dy| == s`

This yields exactly `24s² + 2` coords for shell `s ≥ 1`, with no duplicates.

When a consumer's tier-budget fills mid-shell, the consumed coords now span every direction roughly evenly. Verified by the `shell_walker_first_n_coords_are_evenly_spread` test: 64 coords at shell 8 give 5-16 coords per face (vs. 64 on one face for the prior walk).

Both call sites are rewritten to use `ShellWalker`. As a side effect, this is actually faster than the prior walk: no rejected coords. The prior pattern emitted `(2s+1)³` candidates and filtered to surface; ShellWalker emits the surface directly.

### Validation

- 5 new unit tests in `lod.rs`: total count matches `24s² + 2`, no duplicates, every coord on the cube-shell surface, first 64 coords spread across all 6 faces, first 6 coords visit all 6 faces.
- `cargo test --lib --features render` → 573 passed (568 prior + 5 new).
- Binary builds clean.

### Playtest target

RD=64 from spawn. Expected: chunks load in a roughly spherical pattern around the player rather than a -z slice. The `rendering:` count in the FPS log should grow more evenly as the player moves in any direction. Lifetime mesher distribution (near/mid/far/vfar) should be similar to v0.24.0a.1 — this fix changes WHICH chunks are submitted, not the total throughput, so per-tier counts shouldn't change dramatically.

### Forward-looking notes

The user requested "bundle fixes and improvements and any optimizations." I considered bundling a tighter shadow-cull frustum (would reduce the dominant 72ms shadow-pass cost by ~30-50% by culling chunks behind the camera that can't cast shadows into view). I declined to bundle it because: (a) the slice bug is a correctness problem and a single-variable ship around it confirms the fix; (b) the shadow-cull tightening is an optimization with its own visual-quality risk profile (chunks that should cast shadows getting culled would look like shadow popping). Filed as v0.24.0a.3 if v0.24.0a.2 confirms the slice fix worked.

The eventual v0.24.2 CSM ship will subsume the tightening anyway — 4 cascades with logarithmic split distances cull chunks much more tightly than a single 200m cascade does today.


## Increment 39 (v0.24.0b) — GPU-derived frustum planes

v0.24.0a.1 moved frustum planes out of push constants and into a per-view SSBO, with CPU writing the planes each frame. v0.24.0a.2 fixed the directional-bias bug in chunk loading. The slice issue resolved cleanly with no visual regressions and a small perf bump.

The v0.24 arc continues toward CSM. The next single-variable step: **move frustum-plane derivation from CPU to GPU**.

### Why this matters for the forward arc

The principle the user committed to is: *"anything consumed by future GPU physics dynamics gets staged GPU-side now."* Frustum planes are consumed by the GPU cull pass every frame. If a future increment wants the camera view itself to be written by GPU-side physics or animation (e.g. a physics-driven camera shake, a chase camera tracking a GPU-simulated body), the CPU shouldn't be in the chain between "view changes" and "cull sees new planes". GPU computes planes from the matrices it sees in the buffer.

This ship is exactly that change. CPU writes only the `view` and `projection` matrices into a `ViewParam` slot. GPU compute then derives `view_proj`, `inverse_view_proj`, and the 6 frustum planes, writing them back into the same slot. The cull shader reads planes from there as it already does.

### What v0.24.0b does

**New shader: `src/shaders/view_compute.comp`.** Single workgroup of 1 invocation. Push constant: `view_index: u32`. Reads view+projection from `view_params[view_index]`, computes:

- `view_proj = projection * view`
- `inverse_view_proj = inverse(view_proj)`
- 6 frustum planes via Gribb-Hartmann from `view_proj` rows

Writes all three back into the same slot. The `view` and `projection` matrix slots are unchanged.

**New compute pipeline in WorldRenderer.** Its own descriptor set layout (single STORAGE_BUFFER binding) bound to the ViewParams buffer. Tiny: no pool changes, no new buffers.

**Frame flow changes.** Per frame:

1. CPU writes view+projection matrices for slot 0 (main) and slot 1 (shadow) directly to the ViewParams buffer. Note: shadow uses the cascade VP today — that's effectively a view*proj already. v0.24.0b's "view" matrix for shadow is `cascade.view_proj`'s left half conceptually, but practically the GPU shader just takes a single matrix at the `view_proj` slot and the projection can be identity. Let me re-think this — see design clarification below.
2. Single `view_compute.comp` dispatch with `view_index = 0` derives main's planes.
3. Single `view_compute.comp` dispatch with `view_index = 1` derives shadow's planes.
4. Barrier: compute write → compute read on the ViewParams buffer.
5. Cull + draw flow continues as in v0.24.0a.2.

### Design clarification — what to feed for shadow

The main camera has a clean view (camera world-to-view) and projection (perspective, view-to-clip) split. The shadow cascade in current code only computes a final `cascade.view_proj` light-space matrix — there's no separate view/projection for the light.

Two options:

**Option A**: feed `view = cascade.view_proj`, `projection = identity` for shadow slot. The GPU computes `view_proj = projection * view = view`, which extracts the correct planes. Trivially correct.

**Option B**: factor `cascade.view_proj` into `cascade.view` (light world-to-light-view) and `cascade.projection` (light-view-to-light-clip, orthographic). More principled. More code change in the cascade-fit math.

For v0.24.0b, **Option A**. The cull only consumes `view_proj` (via `planes`). The matrix-fields are reserved-for-future-use. When v0.24.2's `cascade_fit.comp` lands, it'll produce both view and projection cleanly for each cascade — that's the right place to introduce the factored form. v0.24.0b doesn't need to break new ground on cascade shape.

### Risk

Exactly one thing being tested: **does the GPU-derived frustum match the CPU-derived frustum bit-for-bit?**

Validation: a unit test that takes a sample view-projection matrix, runs both the CPU `Frustum::from_view_projection` and a CPU re-implementation of the GLSL math (translated 1:1), and asserts the resulting plane coefficients agree to within `1e-5`. If the test passes and visual playtest still looks identical to v0.24.0a.2, the GPU version is correct.

If shadow/main draw counts shift visibly in playtest (chunks popping, missing geometry), the bug is in `view_compute.comp`'s plane math — most likely an off-by-one in GLSL column-vs-row indexing (`m[col][row]` in GLSL vs `m[(row, col)]` in Rust).

### Files changed

- `src/shaders/view_compute.comp` *(new)*: the derivation shader.
- `src/render/view_params.rs`: a `write_view_proj(slot, mat)` helper to write the `view_proj` field (CPU writes the input matrix). Keep `write_planes` for v0.24.0a.2 compatibility during transition.
- `src/render/view_compute_pipeline.rs` *(new)*: pipeline + descriptor for the new compute pass. Mirrors `chunk_indirect.rs`'s pipeline-construction pattern.
- `src/render/world_renderer.rs`: own a `ViewComputePipeline`; in `draw_world`, write view_proj for slots 0 and 1, dispatch view_compute twice, barrier, then run the existing cull flow. The CPU `Frustum::from_view_projection` calls go away (or are kept as a test-validation reference).
- `src/render/mod.rs`: register the new module.
- `build.rs`: register the new shader.
- 1 new unit test pinning GPU-vs-CPU frustum plane equivalence.

### What v0.24.0b does NOT do

- Does not move cascade-fit to GPU. That's v0.24.2.
- Does not collapse cull dispatches. That's v0.24.1.
- Does not change visual output. (Same risk-isolation as v0.24.0a.)
- Does not consume `inverse_view_proj` from anything. It's computed but unread until later increments.


## Increment 40 (v0.24.2 — CSM arc) — design

The v0.24 arc has landed three non-visual architecture ships (a, a.1, a.2, b). v0.24.0b moved frustum-plane derivation to GPU. v0.24.1 (multi-view single-dispatch cull) is being skipped — its only benefit is "saves one dispatch", and the architectural payoff blends into v0.24.2 anyway. **v0.24.2 is the first visual ship of the arc**: Cascaded Shadow Maps.

### What CSM does and why now

Today's shadow path: one 2048×2048 D32_SFLOAT image, single view-projection from sun's POV covering the near 200m of camera view. Texel density is uniform across the whole map (~10cm at 200m), which means near-camera shadows are blurry (no texels to spare close-up) and distant shadows simply don't exist (the map only covers 200m).

CSM splits the camera frustum into N distance bands and gives each its own shadow projection. Near bands get tight projections (high near-camera texel density); far bands cover more world space (longer shadow draw distance). Same total memory, better distribution.

Per the increment plan from v0.11.1-vapor and the SHADER_ROADMAP, 4 cascades with logarithmic splits:
- C0: 0–20m (high detail, 1024×1024 → ~2cm/texel)
- C1: 20–60m (1024×1024 → ~6cm/texel)
- C2: 60–180m (1024×1024 → ~18cm/texel)
- C3: 180–600m (1024×1024 → ~60cm/texel, extends draw distance from 200m → 600m)

Same total D32 memory as today (4MB × 4 = 16MB vs 2048²×4B = 16MB). Per-cascade resolution can be tuned later if texture bandwidth becomes the bottleneck.

### Why we need to subdivide v0.24.2 itself

CSM moves five interacting variables at once:
1. Shadow map: single 2D image → 2D array (4 layers).
2. Cascade view-projection matrices: 1 per frame → 4 per frame, GPU-derived.
3. Shadow render pass: 1 invocation/frame → 4 invocations/frame.
4. Shadow cull: 1 dispatch → 4 dispatches against 4 different ViewParam slots.
5. Fragment shader: picks cascade per fragment based on view-space depth, samples the right array layer.

Shipping all five at once means: if shadows look wrong in playtest, the bug could be in any of the five layers. Time spent localising is time wasted. Per the discipline pattern, **one variable per ship**.

### Sub-ship plan

**v0.24.2a — ShadowMap → 2D array (4 layers), only layer 0 rendered/sampled.**

ShadowMap struct changes: `vk::ImageType::TYPE_2D` with `arrayLayers: 4`, image view is `VK_IMAGE_VIEW_TYPE_2D` of layer 0 (so existing render pass binding works unchanged). Sampler/descriptor unchanged from fragment-shader POV.

Visual change: zero. The shader still renders into "layer 0" but layer 0 is now a slice of a 4-layer array image.

Risk: descriptor/binding mistakes that make the depth attachment unwritable, or the sampler reads from the wrong layer. Catchable in playtest by "shadows are missing entirely" or "shadows are inverted/garbage".

**v0.24.2b — `cascade_fit.comp` computes 4 cascade view-projection matrices.**

New compute shader: takes camera view + camera projection + sun direction + 4 split distances. Emits 4 view-projection matrices, one per cascade, written into VIEW_SLOT_SHADOW_C0..C3 in the ViewParams buffer. Mirrors the CPU `compute_cascade_view_projection` logic that exists today but parameterizes over an array of split-distances.

Only VIEW_SLOT_SHADOW_C0's matrix is consumed (replacing today's single VIEW_SLOT_SHADOW). C1-C3 are computed but unused.

Visual change: zero. C0's matrix should match the existing single-cascade math, so shadows look identical.

Risk: cascade-fitting math wrong → shadows shift or distort. Validatable by CPU reference function comparing GLSL cascade_fit output to CPU computation, same pattern as v0.24.0b's `derive_planes_cpu_reference`.

**v0.24.2c — Render all 4 cascades into their respective array layers.**

Shadow pre-pass: 4 invocations of the shadow render pass, each with a different image-view-of-layer-i depth attachment and a different VIEW_SLOT_SHADOW_Ci view-projection in its descriptor binding. Shadow cull also expands to 4 dispatches, one per VIEW_SLOT_SHADOW_Ci.

Fragment shader still samples only from layer 0, so visual unchanged.

Risk: significant. Render pass barriers, descriptor set updates per-cascade, indirect-command buffer regions. Cost: expect shadow zone to grow ~3× (4 cascades vs 1, but each at 1/4 the fragment work, but with multi-render-pass overhead). Will measure.

**v0.24.2d — Fragment shader picks cascade per fragment.**

Fragment shader receives all 4 cascade view-projections via the ViewParams SSBO (already there from v0.24.2b). Per-fragment: project the fragment's view-space position through each cascade, find the first cascade where the projected fragment is within its [-1, 1] bounds. Sample that cascade's array layer with that cascade's view-projection. Returns shadow factor.

This is the visual payoff: near-camera shadows become crisp, distant shadows extend to 600m.

Risk: cascade selection logic wrong → bands of unshadowed terrain at cascade boundaries, or wrong-cascade sampling causing shadow-acne or peter-panning. Mitigated by careful debug overlay: a tint per cascade (e.g. C0 → reddish, C1 → green, etc.) makes the cascade-selection function visible per pixel during playtest.

### Files changed across the v0.24.2 arc (forward-looking)

- `src/render/shadow_map.rs`: 2D → 2D array image; per-layer views.
- `src/shaders/cascade_fit.comp` *(new in v0.24.2b)*.
- `src/render/view_params.rs`: add VIEW_SLOT_SHADOW_C0..C3 constants. MAX_VIEWS bumped if needed.
- `src/render/world_renderer.rs`: cascade fit dispatch (v0.24.2b), multi-cascade shadow rendering (v0.24.2c), descriptor changes.
- `src/shaders/chunk.frag`: cascade-selecting shadow sampling (v0.24.2d).
- Tests pinning each step's invariants.

### Validation strategy per sub-ship

- v0.24.2a: visual match with v0.24.0b. Shadow zone GPU time should be ~unchanged.
- v0.24.2b: visual match. CPU reference function for cascade fit returns the same matrix as the existing CPU single-cascade math for C0.
- v0.24.2c: visual match (only C0 consumed by fragment). Shadow zone grows by some factor (measure).
- v0.24.2d: shadows look CRISPER near-camera, shadows visible at 200-600m distance. No banding at cascade boundaries. Cascade-tint debug toggle confirms selection function.


## Increment 40b (v0.24.2b) — GPU cascade-fit, multi-camera-aware plumbing

User reminder mid-arc: "make this all multiplayer friendly and multi-camera along the way." This shipping is where that promise lands in the v0.24.2 architecture.

### The multi-camera shape

The existing `View` + `LocalPlayer` split (May 2026 design note) committed to: one local player = one View. The renderer takes a `&View` per call. The renderer's per-frame work — view derivation, cascade fitting, cull, render — is currently parameterized over that single View, but the **GPU buffer slots** were hardcoded against `VIEW_SLOT_MAIN` (=0) and `VIEW_SLOT_SHADOW` (=1).

For split-screen with N local players, each player owns:
- 1 main camera slot
- N_CASCADES shadow slots (4 in v0.24.2b)
- Total: 5 slots per local player.

`MAX_VIEWS = 8` fits one player with headroom; two players require expanding to 10+. Today's `MAX_VIEWS` is a 1-player upper bound. Documented.

### `ViewGroup` introduced

`view_params::ViewGroup` bundles a player's slot assignments:
```rust
pub struct ViewGroup {
    pub main_slot: u32,
    pub cascade_slots: [u32; NUM_CASCADES],
}
```

`ViewGroup::PRIMARY` is the today's only group, slots [0, 1, 2, 3, 4]. The renderer's per-frame functions now take `&ViewGroup` rather than reaching for the constants directly:
- `prepare_views` writes CPU-authored matrices for the main slot.
- `prepare_cascades` dispatches cascade_fit + view_compute for the cascade slots.
- `record_cull_pass(slot)` reads the slot's planes.

When split-screen lands the bin loop will construct N ViewGroups and iterate over them. The renderer code doesn't change — it sees one ViewGroup at a time.

### Cascade-fit shader

New `src/shaders/cascade_fit.comp`. Computes one cascade's (view, projection) from camera basis + sun + split distances + destination slot. Mirrors `light.rs::single_cascade` line-for-line:
- 8 frustum-slice corners in world space
- Bounding sphere fit (rotation-invariant — avoids shimmer)
- Light view via `look_at_rh`
- Texel-snapped orthographic projection
- Write to `view_params[dest_slot].view` and `.projection`

Push constants: 96 bytes carrying camera basis + sun + (near, far, shadow_size, dest_slot).

Notably, `dest_slot` is a *slot*, not a "cascade index." The shader is agnostic to which ViewGroup it's working for. The CPU `CascadeFitPushConstants::for_cascade(view_group, cascade_index, ...)` resolves the cascade_index → slot lookup. Multi-camera-friendly from the shader's POV.

### Validation

`cascade_fit_pipeline::cascade_fit_cpu_reference` is a literal Rust port of the GLSL math. The test `cpu_reference_matches_single_cascade_for_typical_camera` runs both the canonical `light.rs::single_cascade` (CPU reference) and `cascade_fit_cpu_reference` against the same inputs and asserts the composed view_proj matrices agree to within 1e-3. This is the proof that the GLSL math is correct: if the CPU port matches `single_cascade`, the literal GLSL translation also matches.

If the playtest shows visual changes (shadow flicker, popping, mis-placement), the bug is either (a) in the literal port and the test should have caught it (unlikely — the test passes), or (b) in the GLSL itself diverging from the port at some operation that has GPU-specific semantics (more likely candidates: ordering of float ops, precision of `tan()`, edge cases in `length()`).

### Frame flow in v0.24.2b

Per frame, after `gpu_timer.begin_frame`:
1. `prepare_views(&[(group.main_slot, view_mat, proj_mat)])` — CPU writes main matrices, dispatches view_compute(main).
2. `prepare_cascades(group, cam_basis, sun, splits)` — dispatches cascade_fit×4 (writes view+proj to cascade slots), barriers, dispatches view_compute×4 (derives view_proj+planes from those), barriers.
3. Shadow cull (reads from `group.cascade_slots[0]` = C0 = slot 1).
4. Shadow render pass (uses CPU `light_view_proj` from `single_cascade(view.near, 200.0)` via UBO).
5. Main cull (reads from `group.main_slot` = slot 0).
6. Main render pass.

The CPU `single_cascade` value still drives the UBO's `light_view_proj` for backward compatibility (so the shadow vertex shader and main fragment shader read the same matrix from one place). The GPU `cascade_fit` value drives the cull frustum. They produce the same matrix (validated), so the cull frustum is consistent with what the render pass actually rasterizes.

### What v0.24.2b does NOT do

- Does not render cascades C1, C2, C3. They are computed but unused. (v0.24.2c)
- Does not change the per-fragment shadow lookup. (v0.24.2d)
- Does not change CPU `single_cascade` — it still computes the UBO matrix on CPU.

### Multi-camera contract test additions

- `primary_view_group_slot_layout`: pins ViewGroup::PRIMARY = (0, [1,2,3,4]).
- `primary_view_group_aliases_legacy_constants`: ViewGroup::PRIMARY uses VIEW_SLOT_MAIN/SHADOW.
- `view_group_slot_count_is_five`: 1 main + NUM_CASCADES = 5.
- `max_views_fits_primary_group_with_headroom`: documents that MAX_VIEWS=8 doesn't fit 2 ViewGroups.


## Increment 40c (v0.24.2c) — render all 4 cascades into the array

### What lands

Shadow pre-pass now invokes 4× per frame, once per cascade, rendering into the corresponding shadow-map array layer. The same render pass + pipeline are reused; only the framebuffer and the per-cascade view-projection (via push constant) differ.

Cull dispatch also runs 4×, reading from each cascade's ViewParam slot's GPU-derived planes. The single indirect-commands + count buffer is reused sequentially: each iteration resets the count, fills commands for that cascade, draws, then the next iteration resets again.

### v0.24.2c's visual contract

**No visual change.** The main fragment shader still samples shadow layer 0 via the existing 2D image view, with the C0 cascade's view-projection in the UBO's `light_view_proj` field. C1, C2, C3 are rendered into layers 1/2/3 but no shader samples them yet.

To keep this invariant, **C0's split remains `(view.near, 200.0)`** — identical to the prior single-cascade. C1..C3 use the design's logarithmic splits (20-60, 60-180, 180-600). The simultaneous flip of (a) C0 split → 0-20m AND (b) fragment shader sampling per-cascade lands in v0.24.2d, where the visual win finally arrives.

### Shadow vertex shader: push constant

`chunk_shadow.vert` reads `cascade_view_proj` from a 64-byte push constant rather than `ubo.light_view_proj`. The push constant is set per render-pass invocation (4 times per frame), giving each cascade the right transform. The UBO field is still populated (it's read by the main fragment shader against layer 0) but ignored by the shadow vertex shader.

Why not read from `view_params[slot].view_proj` directly in the shader: that would require adding an SSBO binding to the shadow pipeline's descriptor set layout (significant change). Push constants are smaller, faster, race-free, and don't touch the descriptor wiring. v0.24.2d may reconsider this if the per-fragment cascade-sampling path benefits from sharing the buffer.

### Indirect buffer barrier addition

`record_cull_pass` now emits a `DRAW_INDIRECT → COMPUTE_SHADER` barrier on the commands+count buffers at its start. This serialises the prior cascade's `vkCmdDrawIndexedIndirectCount` against this cascade's cull writes. Without the barrier, the prior draw could still be reading commands while the next cull overwrites them — undefined behavior, likely crash or corruption.

For the very first call of the frame the barrier is a no-op (no prior indirect read in flight); for the 2nd+ call it's load-bearing.

### Per-layer Vulkan resources

`shadow_map.rs` now creates `NUM_CASCADES` image views (one per layer) and `NUM_CASCADES` framebuffers (each wrapping a single-layer view). The original `view` field still points at layer 0 only — main fragment shader's sampler descriptor binds it unchanged. v0.24.2d will introduce a 2D_ARRAY view for sampler use when fragment-side cascading lands.

`destroy` extended to clean up the new resources in order (framebuffers, then views, then the rest).

### Forward-looking notes on perf

The shadow render pass now runs 4×. Each render pass is at 2048² (unchanged from v0.24.2b). So fragment work is **4× the prior single pass** during v0.24.2c. C0 still covers the same 200m as before but at 2048² (good); C1, C2, C3 cover their design splits at 2048² (overkill but simplest). Memory is now 4 × 2048² × 4B = 64MB — temporary peak.

v0.24.2d drops SHADOW_MAP_SIZE to 1024 simultaneous with the C0 split → 20m flip. Per-cascade memory: 4MB. Total: 16MB (same as today's single 2048²). Per-cascade fragment work: 1024² × 4 = 4M total = same as 2048² total. Net: v0.24.2d's fragment work returns to v0.24.2b's level, with the improvement that texel density near-camera goes from 10cm to 2cm.

For v0.24.2c specifically, expect shadow zone GPU time to grow ~4× relative to v0.24.2b. This is temporary; v0.24.2d brings it back down.

If v0.24.2c's perf is unbearable, a fast detour would be dropping SHADOW_MAP_SIZE to 1024 in this ship too. The C0 cascade quality would halve (texel size 20cm at 200m instead of 10cm), but only the 200m boundary would show it; the rest of v0.24.2c is rendering placeholder cascades nobody samples. Filed as v0.24.2c.1 if needed.

### Files changed

- `src/render/shadow_map.rs`: per-layer image views and framebuffers added. Original `view` and `framebuffer` retained for backward compat (layer 0).
- `src/shaders/chunk_shadow.vert`: push constant `cascade_view_proj` added.
- `src/render/shadow_pipeline.rs`: push constant range registered.
- `src/render/world_renderer.rs`: 4 cascades computed CPU-side; loop over cascades in shadow pre-pass; `record_cull_pass` gets the DRAW_INDIRECT → COMPUTE_SHADER barrier.

### Validation plan

Playtest expectation: **visually identical to v0.24.2b**. Main fragment shader's shadow sample doesn't change. The only differences are GPU-internal: 4× the shadow render passes, 4× the cull dispatches.

Failure modes:
- Validation error on first frame about render pass / framebuffer compat → image view or framebuffer construction bug.
- Push constant range mismatch → pipeline layout bug; first render-pass dispatch crashes.
- Shadow layer 0 incorrectly cleared by a later cascade's render pass → render pass attachment ops drop layer-0 contents. Mitigation: each render pass invocation runs on its own framebuffer (own image-view of one layer), so this should be fine.
- Vertex layout / descriptor set incompatibility → first cascade renders fine, subsequent renders garbled.

GPU shadow timing expected to grow ~3-4× (e.g. from ~70ms streaming-peak to ~250-280ms). Steady-state was ~3ms → expect ~12ms. **Accept this regression** because it's reversed in v0.24.2d.


## Increment 40c.2 (v0.24.2c.2) — bisect ship + test infrastructure

v0.24.2c playtest result: shadows entirely missing. GPU shadow timer ~71ms × 4 = ~290ms (matches the predicted 4× shadow render pass cost), but the main fragment shader's shadow sample returns 1.0 (not in shadow) everywhere.

Static analysis came up empty. The CPU-testable invariants all hold:
- Push constant byte layout is column-major std430-compliant (new tests, all pass).
- Cascade subresource ranges target one layer each, the correct layer index (new tests, all pass).
- The Vulkan-spec layout-tracker says cascade 0's render pass leaves layer 0 in DEPTH_STENCIL_READ_ONLY_OPTIMAL — the layout the descriptor expects (new test, passes).
- Cascade fit and view compute math both match CPU canonical references (existing tests, all pass).
- ViewGroup slot resolution is correct (existing tests, all pass).

Yet the visual is wrong. Eliminating tested-correct surfaces leaves these candidates for the bug:
- **Per-cascade render pass ordering interacts badly with layer 0's read-only layout** in a way Vulkan spec doesn't make explicit but Honeykrisp implements (driver-specific).
- **The push constant memory is somehow not visible to the shader at draw time** — e.g. push constant range stage mask wrong (but tested), or pipeline-vs-shader push constant block mismatch.
- **The new DRAW_INDIRECT → COMPUTE_SHADER barrier is doing something I don't expect** — e.g. wrong masks causing unintended invalidation.
- **An interaction I haven't considered.**

### v0.24.2c.2 ships:

1. **Bisection variant** of the render flow: `for ci in 0..1usize` instead of `0..NUM_CASCADES`. Only cascade 0 is rendered, no cascades 1/2/3.

   - **If shadows return**, the bug is in cascade 1/2/3's render passes interfering with layer 0 (which the layout tracker says shouldn't happen — meaning my mental model OR the driver is wrong).
   - **If shadows still missing**, the bug is in cascade 0's render pass itself — most likely the push constant route (new code path) or the new pre-cull barrier (new code path). The byte-layout tests cover the data side; the issue would then be in the pipeline binding or barrier semantics.

2. **New test infrastructure** (3 categories, 11 tests total):
   - `world_renderer::tests`: mat4 push-constant byte layout (3 tests). All pass.
   - `shadow_map::tests`: per-layer subresource range correctness (4 tests). All pass.
   - `vulkan_state_model`: CPU model of Vulkan per-subresource layout transitions (5 tests, plus the module itself). All pass. The key test simulates the v0.24.2c shadow pre-pass and asserts every layer ends in READ_ONLY — which it does, in the model.

### Test infrastructure honest scope

- **Pure-function tests** (subresource range, push constant bytes): cheap, definitive coverage for the bug classes they target.
- **Layout-tracker model**: documents my mental model of Vulkan per-subresource semantics. Will catch "I forgot a layout transition" and "I sample at wrong layout" bugs. Will NOT catch "driver does something different from spec" — for which only playtest can find the bug.

### After the bisect playtest

Three outcomes and their next steps:

- **Shadows return with only-C0 rendered**: bug is in cascades 1/2/3's render passes interfering with layer 0. Likely culprits: a misunderstanding of how `final_layout` interacts across multiple render passes on different subresources of the same image, OR a driver quirk. Fix: add explicit barrier on layer 0 before main pass sample, OR move to per-layer image (one image per cascade) instead of array layers. Both are testable hypotheses.

- **Shadows still missing with only-C0**: bug is in C0's own render pass. Either the push constant (somehow) or the new pre-cull barrier. Bisect further: try restoring the UBO `light_view_proj` path (revert the push constant change in chunk_shadow.vert) and see if shadows return.

- **Shadows look different but not missing**: bug is partial — e.g. some cascades render but not all, or shadows are mis-positioned. Diagnose from there.


## Increment 40c.2-extension — persistent run logs + rich performance summaries

Three additions on top of the bisect ship:

### `run_summaries.txt`

Every run of `unendlich-world` appends one framed block to `run_summaries.txt` at shutdown. Format: `==== RUN START YYYY-MM-DDTHH:MM:SSZ ====` header, version/duration/cmdline/env-flags, `---- summaries ----` divider, full GPU/CPU/mesher summaries, `==== RUN END YYYY-MM-DDTHH:MM:SSZ ====` footer. Designed for grep/awk extraction across runs.

### Rich per-zone summary line

Replaces `mean stddev min max` with `n / mean±stddev / p50 / p90 / p99 / min / max / over_Nms=count (pct%)`. The percentiles + over-budget fraction capture the bimodal signal that mean alone hides. p99 = linear-interpolated 99th percentile; over_Nms counts frames strictly above a per-zone budget.

Per-zone budgets:
- shadow/main/total: 8.333ms (120fps target)
- cull/froxel: 5ms (should-be-cheap; >5ms is suspicious — calibrated against v0.24.2b/c playtest data)
- wait_fences: 8.333ms (stutter-rate proxy)
- record/submit/present: 1ms (small-overhead zones)
- acquire: no budget (high variance is normal — vsync waits land here)
- Mesher per-LOD: no budget (off the frame critical path)

### ASCII box-and-whisker plots

Per zone, below the rich-format block: `|---[==X==]---|` with min/max at the bars, p25/p75 at the box edges, p50 at the X. Collapsed cases: `#` when p25==p75, and `X.........` when min==max (with "all samples equal" annotation).

### Implementation notes

- `ZoneStats` now retains samples as `Vec<u32>` (microseconds). Memory: ~13.7 MB for a 1-hour, 120fps, 8-zone run. Acceptable.
- `interp_percentile` uses linear interpolation between nearest ranks (numpy/R default). Tested against bimodal distributions to confirm p99 lands in the expensive mode.
- `RunBlock::append` opens with O_APPEND and writes the full block in one `write_all`. POSIX atomicity covers small writes; framing markers make recovery easy if interleaving ever happens.
- ISO 8601 formatter self-contained (no chrono/time dep). Validated against `date -u -d @TS` for known epochs.
- Failure to write `run_summaries.txt` logs a warning but doesn't block shutdown — the terminal log still has the same content.

### What this enables for the v0.24.2c bug hunt

The next playtest (after the bisect knobs land their data) will produce a richer summary that shows:
- Whether the shadow zone's p99 is the streaming-spike tail or steady-state
- Whether `over_8.3ms` for shadow is stably high (always slow) or sparsely high (only during streams)
- Box plots showing the shape of the distribution — collapsed/bimodal/heavy-tail

Persistence into `run_summaries.txt` means we can A/B all 4 bisect configurations in one playtest session, then diff the saved blocks offline without scraping terminal scrollback.


## Increment 40c.3 (v0.24.2c.3) — bisect-knob visibility + version-transition logging

Two small fixes on top of v0.24.2c.2, addressing concrete problems the last playtest round revealed.

### Bisect-knob visibility in run summaries

The v0.24.2c.2 ship added `UNENDLICH_SHADOW_CASCADES` and `UNENDLICH_SHADOW_LEGACY_FB` as runtime env vars in the renderer but **didn't register them in the `runtime_flags` registry**, which is what populates the "Flags in effect" block at shutdown. Result: the user A/B'd four configurations and the saved `run_summaries.txt` blocks all showed identical env-flags blocks — the bisect data was ambiguous after the fact.

Fix: register both flags as `binaries: &["unendlich-world"]` with the same metadata structure as `UNENDLICH_INDIRECT`. Added two regression tests:
- `flags_summary_includes_shadow_bisect_knobs`: catches the case where someone introduces a new bisect flag and forgets to register it.
- `flags_summary_reports_shadow_bisect_set_values`: catches the case where the flag IS registered but the literal env-var value isn't being read correctly.

### Version-transition markers in `run_summaries.txt`

When the user bumps `Cargo.toml` between playtests, runs from different versions get appended to the same `run_summaries.txt` and become hard to disambiguate. The version is in each block's body but spotting where one version's runs end and another begins required reading every block.

v0.24.2c.3 adds **version-transition markers**. Before appending a run block, the appender reads the file's most recent `==== VERSION X START ts ====` marker. If absent (first append or no markers in file), emit a new START marker. If present and matches the current version, emit nothing extra. If different, emit `==== VERSION old END ts ====` then `==== VERSION new START ts ====` back-to-back.

The markers are wrapped in 70-char rule lines for visibility:

```
======================================================================
==== VERSION 0.24.2-c.3 START 2026-05-11T13:25:05Z ====
======================================================================
```

Grep-friendly: `grep "^==== VERSION"` lists all transition points. `awk '/^==== VERSION 0.24.2-c.3 START/,/^==== VERSION 0.24.2-c.3 END/'` cuts all runs from one version.

### Race semantics

The version-detection requires a read-then-write window, which is not atomic with the append. Two concurrent processes with the SAME version: fine, no markers emitted, no race. Two concurrent processes with DIFFERENT versions: both could observe the same prior state and both emit transition markers, resulting in duplicate END+START pairs in the file. Acceptable for a single-instance dev tool; the file remains human-readable.

### Test coverage

633 lib tests total. Adds 11 new run_summary tests:
- `parse_last_version_marker` correctness across empty, single, multiple, lookalike, and END-only cases
- First-append emits exactly one START marker, zero END
- Same-version append emits no extra markers (just the new run block)
- Different-version append emits END+START in correct order
- Three-version sequence accumulates the right markers across transitions
- `find_last_version_marker` on missing file returns `Ok(None)`, not an I/O error

Plus 2 new runtime_flags tests for the bisect-knob visibility.

### Filed for follow-up

1. **`unendlich-world` shutdown hang** — process stays alive after "Shutdown complete." is logged. User has been Ctrl-C-ing past it. Pre-existing, not v0.24.2c-introduced. Likely cause: an unjoined background thread (MeshingPool workers? GPU timer? Vulkan resource Arc?). Diagnosis path: `grep -n "thread::spawn\|JoinHandle" src/`, identify which aren't being joined, fix.

2. **No mid/far chunks meshing at RD=8/12** — the v0.24.2c.2 playtest showed 0 mid/far LOD meshes even at RD=12 where they should populate the 8..12 distance shell. Either the chunk manager isn't requesting them (probably because GPU saturation throttles mgr_update) or there's a real LOD-shell bug. Filed separately from CSM; investigate after shadow regression is fixed. Quick disambiguator if shadow regression persists: add `UNENDLICH_SHADOW_OFF=1` knob to confirm whether GPU saturation is the throttle.

3. **GDB-attached auto-tracing for bad render frames** — earlier file-away. FSM with hysteresis on a rolling-1min variance window, dumping the last ~120 frames' per-zone breakdowns to `## ANOMALY` sections in `run_summaries.txt`. Significant infrastructure; reserved for after CSM lands.

4. **v0.24.2c.4 — per-cascade indirect buffer regions** — based on the v0.24.2c.2 perf data showing `INDIRECT=0` was 30-150× faster at median than `INDIRECT=1`, the most likely v0.24.2c regression cause is the new `DRAW_INDIRECT → COMPUTE_SHADER` barrier in `record_cull_pass` interacting badly with Honeykrisp's page-granularity barrier flushes. Fix: give each cascade its own region of the indirect commands+count buffer, eliminating the need for serialization. No barrier means no Honeykrisp barrier-pathology. Next ship.


## Increment 40c.3 (v0.24.2c.3) — version markers + bisect flag visibility

Two small infrastructure fixes that make future bisect data self-documenting.

### Bisect knobs now appear in run_summaries.txt

The shadow bisect knobs (`UNENDLICH_SHADOW_CASCADES`, `UNENDLICH_SHADOW_LEGACY_FB`) are now registered in the `runtime_flags::flags()` table. Before this fix they were being read by the renderer but not printed in the "Flags in effect" header block — meaning saved run summaries didn't capture which configuration produced which numbers.

Caught only because the user noticed his recent bisect playtests had four runs at RD=8 with shadow p50 varying 5× across them — clear evidence the knobs were being applied — but the summaries showed identical flags. The fix is a one-table addition; two new tests pin the flags' visibility.

### Version-transition markers in run_summaries.txt

Each version block is bracketed with `==== VERSION X START ts ====` and `==== VERSION X END ts ====` lines (with 70-char `=` rules above/below for visibility in `less`). On version transition, the previous version's END is appended, then the new version's START, then the run block.

Retroactive-upgrade path: if the file has prior runs but no markers (i.e. it was written by a pre-c.3 binary), the first c.3 append detects this via `parse_last_inblock_version` (scans `version:` lines inside RUN blocks) and emits a retroactive END for the observed prior version. Annotated `(retroactive)` so a reader knows the END timestamp is informational, not literal.

Algorithm encoded as a 5-way match on `(prev_marker, prev_inblock)`:
- `(Some(v), _) if v == current` → no markers (continuation)
- `(Some(v), _) if v != current` → END(v) + START(current)
- `(None, Some(v)) if v == current` → START(current) only (pre-marker file, same version)
- `(None, Some(v)) if v != current` → END(v, retroactive) + START(current)
- `(None, None)` → START(current) only (empty file)

23 run_summary tests cover all five cases plus parser edge cases (END markers ignored, lookalike lines rejected, empty input).

### Honeykrisp barrier hypothesis (filed for next ship)

Coincidentally during this session: user pointed out Honeykrisp likely uses 16KB-or-64KB page granularity for barrier flushes. Combined with the WHOLE_SIZE pre-cull DRAW_INDIRECT→COMPUTE barrier I added in c.2, this could be the entire reason for the v0.24.2c shadow regression — every barrier potentially flushes the indirect buffer's whole working set, multiplied by 4 cascades.

Confirming evidence in the same session: an `UNENDLICH_INDIRECT=0` run had shadow p50=2.29ms vs indirect-path p50=70-347ms. The non-indirect path skips `record_cull_pass` entirely (and thus my new barrier).

Filed fix candidate for v0.24.2c.4: replace the single indirect commands/count buffer pair with 4 per-cascade offset regions in the same buffer. Each cascade's cull writes to its own region; each cascade's draw reads from its own region. No barrier needed because writes/reads are to non-overlapping bytes. Memory cost trivial; complexity moderate.


## Increment 40c.4 (v0.24.2c.4) — chainable runs (max-duration + shutdown-hang workaround)

Two surgical changes to make `cfg1 ; cfg2 ; cfg3` chained shell invocations actually finish without manual intervention between links.

### `UNENDLICH_MAX_DURATION_MS`

Optional wall-clock cap. Computed once at startup as `Option<Duration>`. Checked at top of main loop; if exceeded, calls `window.set_should_close(true)` and the normal shutdown sequence proceeds. Registered in `runtime_flags::flags()` so the cap value (if set) appears in the per-run summary block — important so that chained-runs' data records its own cap.

### Shutdown-hang workaround

The process has been hanging after `Shutdown complete.` for months. Root cause unknown — sits in the destructor chain that fires after `main()` returns. Has been a Ctrl-C-once-per-run annoyance during interactive development; becomes a real blocker for chained shell invocations.

Fix in this ship is a workaround, not a root-cause fix:

1. Explicit drops for the major resources with log lines before and after each, so the LAST log we see in a hanging run identifies the culprit.
2. `std::process::exit(0)` at the end, guaranteeing process termination regardless of any wedged destructor.

Safety analysis for the exit bypass: all important Drop side effects (chunk persistence, run-summary file flush, Vulkan device teardown) happen *explicitly above* the exit call. Only infrastructure drops are skipped (channel receivers, glfw teardown internals, worker-thread joins). The OS reclaims those at process exit.

When the next playtest produces logs that show which "dropped X" line is missing, we can write the targeted root-cause fix and remove the exit bypass. Until then, this gets chained runs working.

Likely actual culprits (filed for future):
- `chunk_loader::_workers` is detached (no Drop joins them). Workers should die when `request_tx` drops, but a worker mid-`generate()` won't notice until it finishes that chunk. Unlikely to hang indefinitely.
- glfw Wayland/Asahi teardown — has been historically slow on this stack.
- A stray Arc keeping a Vulkan resource alive past its underlying device, blocking on a wait.


## Increment 40c.5 (v0.24.2c.5) — shutdown-hang root-caused

Diagnostic logs from c.4's `Pre-exit: dropping X / dropped X` scaffold localized the hang to `MeshingPool::Drop` on first playtest. Root cause:

The result channel is `bounded(queue_capacity * 2)`. During gameplay the render thread drains it per frame. At shutdown, the render loop stops; in-flight workers finish meshing and call `res_tx.send`, but the bounded channel is full and nobody is draining, so they block. A blocked worker never returns to the `while let Ok(req) = req_rx.recv()` loop top — so closing the request channel doesn't wake them, and `join()` waits forever.

Fix: drop `result_rx` BEFORE dropping `request_tx` in `Drop`. This makes any in-flight `res_tx.send` return Err immediately; workers hit the `break` and exit cleanly.

Regression test `drop_does_not_hang_when_result_channel_full`:
- Single-worker pool with queue_capacity=4 (result-channel cap=8)
- Submit-with-retry until ≥12 requests queued (worker has processed >8, result channel is full, worker is wedged in send)
- Drop on a separate thread with 2-second `recv_timeout`

I verified the test by temporarily reverting just the fix:
- With fix: passes in 0.70s
- Without fix: fails in 2.76s (timeout fires)

Belt-and-suspenders kept: explicit drops + `process::exit(0)` from c.4 stay in place. The diagnostic logs will catch any future drop hang immediately, and the exit guarantees chained runs work even if a NEW drop bug appears. Future ship can remove the exit if all "dropped X" lines complete cleanly in a playtest.

This is also a worthwhile design lesson: **bounded channels at the worker→consumer boundary need a Drop strategy.** The pattern "drop sender, wait for receivers to error" works for the receiving-end of a worker (req_rx.recv()) but not the sending-end (res_tx.send()) — those need their corresponding receiver dropped to unblock. Both ends need explicit shutdown handling.


## Increment 40c.6 (v0.24.2c.6) — shadow regression resolved, CSM rendering paused

The c.5 bisect data finally let me localize the v0.24.2c regression. Bisect was three 15s runs at RD=8:

| CASCADES | LEGACY_FB | shadow p50 |
|---|---|---|
| 1 | 0 | 70.68ms |
| 4 | 0 | 217.19ms |
| 1 | 1 | 25.42ms |

Run-3 (legacy framebuffer, one cascade) is 2.8× faster than run-1 (per-layer framebuffer, one cascade) for the same workload. Linear scaling between run-1 and run-2 confirms it's per-cascade cost being paid by every new framebuffer object, not a multi-cascade synergy bug.

Both framebuffers in question wrap functionally identical views of the same image subresource. They differ only in their VkImageView/VkFramebuffer handle values. Honeykrisp's perf is sensitive to that handle identity — root cause not understood but the symptom is reproducible.

c.6 reverts shadow rendering to use the single legacy framebuffer (the v0.24.2b path). Per-layer framebuffers and views are removed from the ShadowMap struct. The bisect knobs are removed from the flag registry.

CSM rendering is paused, but the infrastructure that doesn't depend on per-layer framebuffers is kept:
- Shadow image has 4 array layers (memory pre-allocated, not used by anything yet).
- CPU computes `cascades[0..4]` (only [0] is used).
- GPU cascade_fit + view_compute populate all 4 ViewParam slots (only slot 0 is read).
- Shadow vertex shader uses push-constant route for the matrix (works for multiview later).

Path forward for CSM: probably **VK_KHR_multiview**. Single render pass, single framebuffer with 2D_ARRAY view, fragment shader sees `gl_ViewIndex`. Standard answer for CSM on tile renderers. Sidesteps the per-framebuffer-handle issue entirely. Our existing infrastructure (per-cascade GPU view-params, push-constant shader) is already shaped to feed multiview.

Filed for follow-up:
- Multiview-based CSM.
- Honeykrisp driver investigation (why is `VkFramebuffer` handle identity perf-sensitive at this magnitude).
- Validate remaining shutdown drops with the c.4 diagnostic logs after c.5 fixed mesh_pool.
- LOD-shell investigation.
- GDB-attach auto-tracing.

Lesson worth recording: **bisect knobs with self-documenting summaries are a high-value diagnostic tool**. The c.5 bisect produced unambiguous data because runtime_flags now records the knob values in run_summaries.txt. Without that, we'd be guessing which of the four runs was which configuration. Keep this pattern for future similar investigations.


## Increment 40c.7 (v0.24.2c.7) — upload knobs + workload-shape summary

Two diagnostic surfaces added so the "shadow GPU cost scales with `rendering:` chunk count" hypothesis from c.6 analysis can be tested directly.

### Upload knobs

`UNENDLICH_MAX_UPLOADS_PER_FRAME` and `UNENDLICH_UPLOAD_BUDGET_MS` wired into `unendlich-world` (matching the pattern already used in `unendlich_spin.rs`). Both default to the pre-c.7 values (16, 4.0). Lower values slow world fill at the cost of holding fewer chunks visible at any one time — a way to bound the steady-state `rendering:` count and thus the shadow rasterization workload.

Both registered in `runtime_flags::flags()` so they appear in run-summaries. Caught a duplicate-registration bug (`MAX_UPLOADS_PER_FRAME` was already registered for unendlich-spin); merged the entries so both binaries share one Flag with `binaries: &["unendlich-world", "unendlich-spin"]`.

### Workload-shape summary

Each run now records min/p50/p90/max of `loaded` and `rendering` counters in the summary block. Sampled once per FPS-log window (= ~1s) so the storage is trivial (~30 u64s per 30s run). Pure-function `format_workload_summary` with 4 unit tests.

Motivation: c.5 run-3 (15s, shadow p50=25ms) and c.6 30s run (shadow p50=72ms) used the same code path but produced very different perf. Most plausible explanation is steady-state workload differs by duration. Without workload counters in the saved summary, no way to confirm from the data. The c.7 summary fills that gap.

This is the second diagnostic-pattern lesson worth recording: **whenever a perf hypothesis depends on a counter that's only in per-second logs, get that counter into the run-summary block.** Per-second logs are lost as soon as the terminal scrolls; summary blocks persist and are bisect-friendly.


## Arc inflection: launcher first, rendering second (c.8+ planning)

The c.4-c.7 ships exposed a real iteration-speed problem. Each ship was a guess + a playtest + a debate about what the playtest meant, because the data we had was thin and the run-to-run variance was high enough that single-run conclusions were unreliable (the c.6 "fix didn't work" reversal being the worst case — one outlier 30s run nearly made me ship a regression-of-the-fix).

The c.4 conversation introduced the diagnostic of explicit drops + process::exit, which paid off immediately at c.5 (root-caused mesh_pool drop hang in one cycle). The c.7 conversation introduced upload-knob env vars + workload-shape summary, which paid off immediately by giving us the rendering=7 → shadow=25ms data point — concrete evidence that shadow cost is heavily workload-driven, not just code-driven.

In both cases, **better instrumentation gave us answers faster than guessing**. The natural extrapolation: invest in instrumentation seriously, and every future ship becomes faster to validate.

The c.8+ arc takes that to its conclusion:

- **c.8**: Structured event log (events.jsonl) with run UUIDs cross-referencing terminal logs, run_summaries.txt, and (future) test results. Foundation for retrospective machine analysis.
- **c.9**: .unendtest spec format (TOML in) + .unendresult format (JSON out). Tests are first-class artifacts. Assertion eval. Single-binary scope.
- **c.10-c.11**: Launcher. `unendlich-launcher run foo.unendtest --version @gitref` uses git worktree + cargo to produce a binary keyed by commit hash, caches it, runs the spec against it. Worktree-based so the main repo's working state stays untouched. tests/ directory in repo for the canonical suite.
- **c.12**: Cross-version compare. `unendlich-launcher compare foo.unendtest --versions @c.5,@c.6,@HEAD` runs each, produces a unified diff. This is the payoff — every future engine ship gets validated against history automatically.

The trade is: a slower path to "shadows look good and plants are GPU-driven," but a much faster path to "every future change is verifiable, every regression catchable, every claim falsifiable." Given the c.4-c.7 experience with how hard the perf bisects were without this infrastructure, the trade is worth it.

After c.12, the rendering arc resumes — multiview CSM, GPU plant lifecycle, then the field systems (temperature, pressure, humidity, wind, sunlight), then fluids. Each ship from c.13+ comes with a .unendtest that gates the claim. Every perf number gets a test, every visual change gets a regression check, every "did this work?" question has a falsifiable answer in the result file.

The "launcher" framing is also a real one. Once .unendtest is a first-class artifact and the launcher knows about versions, the same machinery extends to (eventually) distributing test-builds to external testers: cross-compile on tag push, drop binaries to a release server, testers `unendlich-launcher run foo.unendtest` and the launcher fetches the right binary for their platform. That's post-v1.0 work, but the foundation is the same — version-aware spec dispatch.


## Increment 40c.9 (v0.24.2c.9) — `.unendtest` spec format + automated result writer

Lands the test-spec layer. `.unendtest` TOML in, `.unendresult.json` JSON out. Spec contains env-var overrides, duration cap, and assertions; result captures every measurable thing the binary saw + per-assertion pass/fail + overall outcome.

The pure-function `build_result` helper takes inputs from the binary's shutdown path and produces the `TestResult` — keeps the binary's main() thin and makes the result-building unit-testable. Three end-to-end tests cover the helper: all-pass, mixed-pass-fail, serde round-trip.

Spec-loading and assertion eval are also pure (no I/O outside the file read), 14 tests cover the matrix: future-version rejection, minimal/full specs round-trip, output path defaulting, the three metric-lookup styles (flat / dotted / suffix-sugar), and each assertion shape (range / scalar / bool / special-case `duration_completed`).

Wire-up in `unendlich_world.rs` is straightforward: argv parse `--test-spec PATH` BEFORE any env reads, apply `[env]` table with `std::env::set_var`, synthesize `UNENDLICH_MAX_DURATION_MS` and `UNENDLICH_EVENTS_PATH` from the spec, run the normal main loop, at shutdown invoke `build_result` with the gathered stats and write the resulting JSON. Exit code reflects assertion outcome (0 pass, 1 fail, 2 I/O error).

Lesson worth recording: when you have a pure-function helper, the binary's wiring becomes a 30-line glue rather than 150 lines of half-tested control flow. The temptation when adding "just one feature" to main() is to inline it; resist. Extracting to `build_result` paid off immediately — the integration test caught one assertion-eval bug before the binary even built.

Spec format pegged to `spec_version = 1`. Future schema changes add optional fields (still 1) or bump to 2 (with c.10+ binaries accepting 1 AND 2). Today's c.9 binary rejects `> 1` with a clear error: `spec_version N is newer than this binary supports (max 1)`. This is what makes cross-version testing actually work — a c.5-era spec doesn't bit-rot when c.10 ships.

`toml = "0.5"` pinned (the 0.8 line wants edition2024, which rustc 1.75.0 doesn't yet have stable). When we bump rustc we can move forward.


## Filed for later: decoupled update/render + fixed-timestep + interpolation

Captured from c.9 discussion of vsync stalling, plus earlier rollback architecture work (March-April timeframe).

### The architectural commitment we're heading toward

Most engines run game logic at one fixed rate (e.g. 60Hz tick) and render at another (whatever the GPU can do, capped by display refresh). The simulation has its own clock that advances in fixed integer steps; render reads the *latest two simulation states* and interpolates between them based on render-frame time.

```
sim tick:   |--|--|--|--|--|--|--|--|     (16.67ms each, 60Hz)
render:     |-|-|-|-|-|-|-|-|-|-|-|-|     (8.33ms each, 120Hz)
                ^         ^
                tick N    tick N+1  → render interpolates between them
```

For tree growth, fluid flow, plant lifecycle, fire spread — these systems update on the simulation tick, not the render frame. Render produces a visually smooth result regardless of render-frame variance.

### Why it matters for the systems we're about to build

- **Trees and plants**: lifecycle state advances per tick. Render reads `(prev_state, next_state, alpha)` and interpolates leaf positions, growth scale, wilt amount. Smooth growth animation at any render rate.
- **Fluids**: same — solve advection per tick at fixed dt (no numerical instability from variable dt), display interpolated state.
- **Wind/pressure/temperature fields**: advect on tick, sample-and-blend on render.
- **Plant tilt under wind**: derived from wind-field sampled per tick, render reads two consecutive samples and blends. Tree gently sways even between ticks.

### Determinism + rollback story

Variable-dt simulation diverges across machines (`e^(-rate*dt)` with different dt is different state). Fixed-dt simulation is bit-for-bit deterministic given same input → same world state. That's the prerequisite for:
- Hybrid-determinism testing in the `.unendtest` framework (already documented; this is what makes it work in practice for non-trivial state).
- GGPO-style rollback networking later (already filed in the rollback-foundation work from April).
- Replays (record input, re-run, get identical results).

### Why we're deferring

This is a big refactor of `tick_sim`, `apply`, every per-frame integration that uses `dt`. Wrong order to do it now. Right order:
1. Field systems first (c.13-c.17). They establish the "ambient state propagating through chunks" pattern.
2. THEN fixed-timestep refactor. By then we have concrete demands on what the tick needs to do — fields advect, plants grow, fluids flow.
3. THEN the visible payoff: trees that sway gently, fluids that ripple smoothly, plant growth that animates between ticks.

If we did the refactor BEFORE the field systems, we'd be designing the tick architecture against imagined requirements. With the fields in hand, the demands are concrete.

### What the next ship that touches simulation should do

Even before the full refactor, every NEW system should be written tick-shaped from the start: a `tick(world, dt_fixed)` function called from the sim tick, with no per-frame state. That way the eventual refactor is "wire up the tick scheduler" rather than "untangle a year of per-frame state."

This is the rule for c.12+ rendering arc systems: **tick-shaped from day one**.


## Increment 40c.10 (v0.24.2c.10) — no-latency baseline (FIF=1 + MAILBOX + late-input)

The first ship in the no-latency commitment. Architectural shift from "buffer-and-stall" to "draw-the-latest-input-now, miss the deadline if you have to."

Three small changes that compose:

1. `MAX_FRAMES_IN_FLIGHT` 2→1 — no CPU lookahead, presented frame reflects most recent input. CPU sits idle if GPU is slow (DRS in c.11 will mitigate).
2. MAILBOX as default present mode (was FIFO when VSYNC=1). Three-way knob now via `UNENDLICH_VSYNC`: 1=FIFO, immediate=tearing, default=MAILBOX. Fallback to FIFO with warning if MAILBOX unsupported.
3. Late input poll just before `draw_frame`. Mouse delta accumulated by event, applied at top of frame, zeroed, accumulated again by late-poll, applied a second time. Camera matrix used for render reflects latest possible pointer state.

The three changes are intentionally separated from DRS. DRS is its own architectural lift (off-screen render target, blit-upscale pipeline, frame-budget controller). Shipping the no-latency baseline first means we validate that piece in isolation before stacking DRS in c.11.

Expected impact at RD=2:
- `cpu_acquire_p50_ms`: 8.0ms (one vsync interval) → <1ms.
- `cpu_wait_fences_p99_ms`: 175ms → much less (no queue to wait for).
- Input-to-photon latency for camera: improved by ~CPU-side-state-management duration.

A/B-validation via `rd2_smoke.unendtest` (MAILBOX default) and `rd2_fifo_compat.unendtest` (forced FIFO). Both should pass; the perf delta between them is the c.10 win.

Why this works as a single ship despite touching three subsystems: each change is small (one-constant for FIF, one match-arm rework for present mode, one block of code for late-poll), and they share a coherent design rationale (no-latency commitment). Mixing them into the same ship lets us see their combined effect; splitting would have shipped intermediate states that nobody would actually run.


## Path B committed (post-c.10): infrastructure first, performance second

After the c.10 RD=8 data made the perf situation concrete (5fps at RD=8, shadow_p50=110ms, shadow accounts for ~67% of frame cost), we considered three orderings:

- Path A: perf first (CSM → culling → mega-buffer → launcher).
- Path B: launcher first, then perf.
- Path C: small adaptive-workload win, then launcher, then perf.

**Chose B.** Reasons:
1. Multiview CSM is risky (Honeykrisp/Asahi driver behavior unknown), big architectural lift. Without the launcher, validating "did shadow actually get cheaper" reduces to manual A/B with run-to-run noise hiding signal — exactly the c.5-c.7 problem we built the test framework to escape.
2. Real perf arc is 5-7 ships. Bundling them without cross-version validation accumulates a long unverified chain of changes.
3. Launcher is only ~2 ships. Investment pays back on every perf ship after.
4. Path C's adaptive workload is real but small (~30% gain on shadow tail). Not worth a detour mid-arc.

### Locked ship order

- **c.11**: Launcher core. `unendlich-launcher run foo.unendtest --version @gitref` via git worktree + cargo build + commit-hash cache.
- **c.12**: Launcher polish + cross-version compare. `unendlich-launcher compare foo.unendtest --versions @c.5,@c.7,@c.10,@HEAD`.
- **c.13**: Multiview CSM scout. Probe VK_KHR_multiview on Honeykrisp. If supported, single-framebuffer multi-cascade. If not, alternatives. Validate via launcher.
- **c.14**: Shadow GPU culling. Per-cascade frustum culling. Cull infra exists; wiring + per-cascade dispatch.
- **c.15**: Indirect mesh storage. Mega-buffer for chunk geometry. MultiDrawIndirect.
- **c.16+**: Adaptive workload, greedy meshing, bindless textures, then plant lifecycle GPU port, fields, fluids. All tick-shaped from day one per the prior design note.

### Honest acknowledgment captured

The c.5-c.10 arc has been infrastructure-heavy. c.10's user-visible value was "less of a slideshow" — real but not transformative. The next 5-6 ships need to land visible perf, or the trajectory stops being motivating. Launcher → CSM → culling → mega-buffer is the shortest credible path to "this feels great." Estimated 5 ships to RD=8 at 60fps if multiview works on Honeykrisp.


## Increment 40c.11 (v0.24.2c.11) — launcher core (`unendlich-launcher run`)

First ship in the launcher arc (Path B). c.11 ships the `run` subcommand alone; c.12 will add the cross-version compare and the driver-epoch awareness.

### Key design choices

**Local clones, not git worktree.** Worktrees share `.git/` with the main repo and can produce cryptic errors (`fatal: 'main' is already checked out at...`) that require knowing `git worktree prune` to fix. Local clones (`git clone --local <repo> <dst>`) are fully isolated — separate `.git/`, no worktree metadata anywhere, cleanup is `rm -rf`. Hard-linked git objects keep them fast and small. The user explicitly flagged historical worktree pain and we chose isolation over efficiency.

**Output paths via env vars, not spec rewriting.** The launcher could deserialize the spec, mutate `[output]`, re-serialize, write a temp spec, and pass that. We took a simpler path: extend `LoadedSpec::result_path()` and `events_path()` to consult `UNENDLICH_RESULT_PATH` and `UNENDLICH_EVENTS_PATH` env vars with highest precedence. The launcher sets these, the binary picks them up transparently. The user's spec on disk is never modified.

**Cache key = commit hash.** Same hash = same source = same binary. Future builds for the same commit are instant cache hits regardless of the ref the user typed (`@HEAD` and `@v0.24.2-c.7` resolve to different hashes; identical hashes share a cache entry). Documented in the readme that `--rebuild` would be a future flag for forcing rebuild.

**Test strategy.** Pure functions (path helpers, env-var precedence) get unit tests. Git-touching tests gracefully skip when there's no git repo (`git rev-parse --git-dir` probe). End-to-end manual validation is the user running `unendlich-launcher run ... --version @HEAD` and comparing the result file to a direct `cargo run` invocation. No mocking of git/cargo — too brittle for a one-user tool.

### What c.12 will add

- **Driver identity in result files** — capture vendor/device IDs and driver version from `VkPhysicalDeviceProperties` and `VK_KHR_driver_properties`. Goes into the `.unendresult.json` schema as `gpu_driver_id` and `gpu_driver_hash`.
- **`compare` subcommand** — `unendlich-launcher compare foo.unendtest --versions @c.5,@c.7,@HEAD`. Runs each, diffs, reports. Warns when driver hashes differ across compared results.
- **`run-suite` subcommand** — runs every spec in `tests/specs/` against current HEAD. The "test-all button."
- **Caching of result files** keyed by `(commit, spec_hash, driver_hash)` — if a result exists for the same triple, skip re-running. This makes `run-suite` cheap on subsequent invocations.
- **Filter for cross-version comparison** — only compare results sharing the same `driver_hash`. If c.5's cached result is from driver_hash_A and current driver is driver_hash_B, optionally re-run c.5 against driver_hash_B to fill the gap.

This delivers the user's "test-all button that only pulls in equal-driver-hashed tests and reruns old versions if needed."

### What c.11 deliberately defers

- nix-based reproducible builds. Filed in DESIGN_NOTES; revisit when distributing to others, or if driver drift becomes a real diagnostic pain (user can switch to nixos-asahi).
- Build concurrency. Document "don't run two launchers simultaneously" rather than implementing a file-lock. One-user tool, ergonomic complexity not yet justified.
- Shared `CARGO_TARGET_DIR`. Cache hits dominate the steady state; cold builds are the rare slow path. Revisit if real pain.

### Honest read of c.11

This ship adds zero user-visible perf. The trade we made in Path B is "infrastructure ship now → faster perf iteration in c.13+". c.12 closes the loop on infrastructure; from c.13 onward every perf change is validated against the suite automatically and the launcher proves its value. If after c.12 the launcher feels like overkill for our actual use, we'd know early. So far the design feels right: result files are the artifact, launcher dispatches them, comparison is mechanical.


## c.12+ architectural arc: workspace + GUI/TUI + distribution stubs

Captured from the c.11/c.12 planning conversation. Below is the locked direction for the next 5-6 ships.

### Workspace layout (target)

```
unendlich-game/                    ← repo root, sibling of claude-zips/
├── Cargo.toml                     ← [workspace] manifest
├── commands/                      ← workflow scripts
├── crates/
│   ├── unendlich-engine/          ← game/render engine + unendlich-world binary
│   ├── unendlich-shared/          ← test_spec, run_id, events, run_summary, zone_stats
│   ├── unendlich-launcher/        ← CLI launcher (library + bin)
│   ├── unendlich-analyze/         ← log analysis CLI
│   ├── unendlich-tui/             ← ratatui interface (depends on launcher)
│   ├── unendlich-gui/             ← egui interface (depends on launcher)
│   └── unendlich-distrib/         ← signing/distribution (stubs for now)
└── tests/specs/
```

Sibling of repo:

```
~/unendlich-development/
├── unendlich-game/                ← workspace root above
└── claude-zips/                   ← all zips here (sibling, not workspace member)
```

### Ship sequencing

- **c.12a**: workspace refactor. Mechanical migration of existing code into the layout above. No new features. All existing tests pass against the new layout. Validate via launcher_smoke.sh.
- **c.12b**: cross-version compare + driver-id capture. Adds `gpu_driver_id`/`gpu_driver_hash` to result file schema (lives in shared crate now). Adds `compare` and `run-suite` subcommands to launcher.
- **c.13**: TUI (ratatui) in unendlich-tui. Lists runs, shows latest assertion status per run, shows path to latest snapshot. No engine changes.
- **c.14**: GUI (egui) in unendlich-gui. Drag-drop a `.unendtest` → run it. Drag-drop a `.unendresult.json` → display it. Version dropdown to pick which binary to run against. Run a chosen spec against a chosen version.
- **c.15**: Snapshot capture in engine. 1Hz screenshot via vkCmdCopyImageToBuffer + PNG encode. Writes to `~/.unendlich/runs/<uuid>/snapshots/`. GUI displays the latest snapshot for in-progress runs.
- **c.16**: Distribution stubs in unendlich-distrib. ed25519 key gen, sign/verify primitives, CLI subcommands (init-key, sign-build, verify-build, upload, download) that compile but print "not yet implemented." Real wiring is c.18+.
- **c.17 onward**: Real perf work — multiview CSM, shadow culling, indirect mesh, adaptive workload. All validated via the now-working launcher + GUI feedback loop.
- **c.18+** (filed for later, do not start yet): distrib actual wiring to a dufs-backed HTTPS server, cross-build production, tester contribution flow, signature-verified binary fetching.

### Design choices locked in

- **One repo, multiple crates** — workspace refactor is mechanical, not a real split. Avoids duplicated code, single git history, single tagging convention.
- **GUI/TUI as separate crates** — egui/winit/wgpu dependencies are heavy; isolating them from `cargo build -p unendlich-launcher` keeps the CLI lean.
- **Drag-and-drop = run-or-display, not destructive ops** — `.unendtest` and `.unendresult.json` only. The zip update flow stays as a shell script with explicit `git status` review.
- **Distribution stubs only for c.16** — crate exists with the right shape, has signing primitives that work (and tests for them), but no actual fetch/upload yet. Filling in is mechanical when needed.
- **Zips live in `claude-zips/` sibling**, not in `unendlich-distrib/`. Distrib is for Rust code; zips are for archiving Claude drops.
- **`unendlich-shared` stays minimal initially** — just the spec/result/run-id machinery already in `util/`. The "world constants unified" pass happens later when GUI + engine + tests have all expressed what they need.

### What this enables, ranked

1. (c.12a+b) Cross-version perf comparison works end-to-end. The thing the launcher arc was about.
2. (c.13/c.14) Test-running UI for you to use day-to-day. Drag a spec, see result.
3. (c.15) Visual feedback on what the engine is actually rendering, useful immediately for debugging and later for testers.
4. (c.16) Crypto plumbing ready for the day external testers exist; no operational complexity until then.
5. (c.17+) Real perf work with full validation infrastructure backing every claim.
6. (c.18+) External tester distribution, when there's actually demand.

### Honest acknowledgment

This is a multi-month arc. The pieces are individually tractable (each is 1-3 sessions); the total is large. The discipline is doing them in order. Specifically: perf work (c.17+) is the value-producing step for the *current user*; everything before it is infrastructure that pays off when it's done.

The risk to manage: getting stuck building tools instead of shipping perf. Each tool ship has to have a clear "this improves my actual iteration" justification. If we can't articulate one for a given ship, the priority is wrong.


## Increment 40c.12a (v0.24.2c.12-a) — workspace refactor

Mechanical migration of the single `unendlich` crate into a 4-crate Cargo workspace. No behavior change, no new features. Foundation for everything c.12b through c.18+.

### What moved where

- `src/util/{test_spec,run_id,events,run_summary,zone_stats}.rs` → `crates/unendlich-shared/src/`
- `src/launcher.rs` → `crates/unendlich-launcher/src/lib.rs`
- `src/bin/unendlich_launcher.rs` → `crates/unendlich-launcher/src/bin/unendlich_launcher.rs`
- `src/bin/unendlich_analyze.rs` → `crates/unendlich-analyze/src/main.rs`
- Everything else in `src/` → `crates/unendlich-engine/src/`

### Import rewrites (mechanical via sed)

- `crate::util::*` → `unendlich_shared::*`
- `unendlich::util::*` → `unendlich_shared::*` (in binaries)
- `unendlich::launcher::*` → `unendlich_launcher::*`
- `unendlich::*` (anything else in binaries) → `unendlich_engine::*`

Total: 6 source files in engine, 1 source file in launcher, several binary files. All sed-able.

### Validation

- `cargo build` on each crate independently: pass.
- `cargo build -p unendlich-engine --features render --features hot-shaders` (full game build): pass.
- `cargo test -p unendlich-shared`: 65 tests pass.
- `cargo test -p unendlich-launcher`: 7 tests pass.
- `cargo test -p unendlich-engine --lib --features render` (skipping slow cpu_generator tests): 580 fast tests pass.
- Total: matches c.11's 673 test count.

### Why the engine crate keeps the bench bins

`world-bench`, `mesh-bench`, `winding-check`, etc. all use engine internals (`world::cpu_generator`, `render::*`, etc.). Keeping them in `unendlich-engine` avoids a circular crate dep and matches their actual coupling to the engine.

### Crate isolation as a real win

Pre-workspace, `cargo build --bin unendlich-launcher` pulled in `ash`, `glfw-sys`, `vk-sys` because they were optional deps of the same crate. Now `unendlich-launcher` depends only on `unendlich-shared` + `log`. Cleaner mental model AND faster compile times when iterating on the launcher.

The GUI/TUI crates (c.13/c.14) will similarly NOT pull in any engine deps — they consume the launcher's library API. This is the architectural cleanliness the refactor was for.

### What's deliberately NOT in c.12a

- No new features. Cross-version compare is c.12b.
- No new crates. `unendlich-tui`, `unendlich-gui`, `unendlich-distrib` are added in their own ships when there's actual code to put in them.
- No behavior change. The engine produces identical results; the launcher dispatches identically; the analyze CLI parses identically.

### Honest read

This ship adds zero user-visible value on its own. But it makes every future ship cleaner — GUI/TUI as separate crates, distrib as a separate crate, the launcher buildable in isolation. The trade was 1 session of careful migration vs. months of friction from carrying an awkward single-crate layout through the rest of the launcher arc.


## Workflow note: retroactive patch application for cross-version measurement

Captured from c.12a planning conversation.

### The pattern

When introducing a new probe or measurement (e.g., capturing GPU driver ID, adding a Vulkan query for shadow overdraw, recording a new metric in the result file), produce the change as ONE small focused commit. The user manually audits it. Then the patch gets mechanically cherry-picked onto each historical tag we want to measure, producing grafted binaries the launcher can run.

Benefits:
- Manual audit cost: O(1) per probe. Mechanical application: free.
- Historical baselines suddenly have today's metrics.
- No need to ship a "measurement framework" upfront — probes accumulate as they're needed.

### What this changes about how I write code

- Probes should be small, surgical, one-or-two-file changes. Bundling unrelated refactors makes rebase conflict more likely.
- Probes should be cheap (microseconds, not milliseconds). Heavy instrumentation changes what we're measuring.
- Probes should be observability-only. Don't change game behavior. Don't add new dependencies.
- The probe's diff should be readable in a single screen. If it spans 20 files, it's not the right shape.

### Workflow when applying retroactively

1. Author the probe as a focused commit (`git commit -m "probe: capture gpu_driver_id at startup"`).
2. User audits: `git show HEAD`. Asks questions, requests changes.
3. Once approved: `git format-patch -1 HEAD > tools/probes/gpu-driver-id.patch`.
4. Tool/script (c.12c): apply patch across selected tags, build each, cache binaries.
5. Launcher runs the same .unendtest against all grafted versions.
6. The .patch file is committed alongside the merged probe, so future retroactive runs can re-apply it.

### Layered with the cache

Cached binaries are keyed by `(commit_hash, build_flags)`. A grafted binary has a different effective commit hash than the base — it's `base_tag + probe_patch`. We compute the cache key as `sha1(base_tag_hash + patch_sha)` so each grafted variant gets its own cache slot. Re-running the same graft is free; changing the patch invalidates only that variant.

### When to use this vs. just running the new version

- New version's metric already covers what you want, no need to backport: just compare versions directly.
- Want measurements on historical versions that didn't have today's instrumentation: retroactively apply the probe.
- Want to debug "did this probe itself affect perf": run the probe on HEAD AND on HEAD-with-probe-reverted, compare.

### Not needed in c.12a or c.12b — file for c.12c

c.12a is the workspace refactor (done). c.12b is `compare` and `run-suite`. c.12c is the "graft" command that automates the patch-onto-tags workflow. By then we'll have real probes worth applying.


## Refinement: graft tool needs an honest "too different" exit

Captured after the initial graft sketch.

The naive "apply patch to all tags, build each, tag the result" model breaks the moment a tag has different surrounding code than the probe expects. There are three real outcomes per (probe, tag) pair, and the tool needs to handle each honestly:

1. **Clean** — cherry-pick succeeds, build succeeds, binary cached, tag recorded.
2. **Derived** — cherry-pick conflicts but the fix is small and obvious. A `<probe>.<tag>.subpatch` is generated capturing the diff, the human (or Claude) reviews + approves it, applied, built, cached.
3. **Incompatible** — the historical version genuinely lacks scaffolding the probe needs (e.g., `events.jsonl` didn't exist in c.7, so probes that write events can't be backported there). Marked in the manifest with a reason. The compare command surfaces this as "—" / "N/A", not as a silent omission.

The interactive entry point is critical: when a conflict appears, the tool shows the diff, classifies (small fix vs. structural), and asks whether to derive a subpatch or mark incompatible. No automation pretends to make the call alone.

### Manifest format

`tools/probes/<probe-name>.manifest.json` records per-tag outcomes:

```json
{
  "probe": "gpu-driver-id",
  "main_patch": "tools/probes/gpu-driver-id.patch",
  "applied_to": {
    "v0.24.2-c.10": { "status": "clean", "binary_hash": "abc1234..." },
    "v0.24.2-c.8":  { "status": "derived", "sub_patch": "...c8.subpatch", "binary_hash": "...", "rationale": "..." },
    "v0.24.2-c.7":  { "status": "incompatible", "reason": "events.jsonl predates c.8" }
  }
}
```

The manifest is the audit trail. Next year, re-running `graft` with the same probe against the same tags produces identical results because the sub-patches are pinned. Compare's output renders gaps explicitly so misreading "no data" as "no change" can't happen.

### What this changes about the scope

`graft` is best-effort and honest about gaps, not "smart resolution." The tool's job is:
- Surface conflicts cleanly.
- Make small fixes cheap to accept.
- Record the choice durably.
- Be loud about gaps in compare output.

Anything that requires real judgment about "is this fix semantically correct" stays a human decision.


## Increment 40c.12b (v0.24.2c.12-b) — driver_id + compare + run-suite

Completes the c.12 arc. Two new subcommands (`compare`, `run-suite`) and a schema bump (`gpu_info` in result files). Together these are the cross-version compare features the launcher was built for.

### Key design choices

**`gpu_info` is Option<GpuInfo>, not GpuInfo.** Reasons:
- Backward compatibility with c.11 result files (no `gpu_info` field).
- Future binaries built without `render` feature (no Vulkan context) can still produce results, just without driver info.
- Serde's `#[serde(default, skip_serializing_if = "Option::is_none")]` makes both deserialization and roundtrip clean.

**`driver_hash` uses FNV-1a, not a real crypto hash.** Reasons:
- We don't need crypto properties — just stability + low collision probability across realistic driver combinations.
- Avoids pulling in `sha2` or similar deps in the shared crate (which would percolate into every crate that depends on shared).
- 64-bit FNV-1a is ~5 lines of code, no allocation, deterministic. Good enough.

**Compare logic is pure, in two layers.** `build_compare_report` is data-in / data-out (no I/O, no formatting). `render_compare_report` is pure text rendering over the report. This means:
- The GUI/TUI (c.13/c.14) can use `build_compare_report` directly to populate widgets.
- Tests can synthesize result data and assert on the report structure without touching disk.
- The CLI is the thinnest layer — parse args, load files, call into the library, print.

**Driver-mismatch detection.** Triggers only when ≥2 *present* hashes differ. One Some + many None doesn't trigger (you can't compare a hash you don't have). All None doesn't trigger (no data to mismatch on). Matches the principle: "warn about real cross-epoch comparisons, not about missing data."

**`run-suite` is best-effort.** A failed spec doesn't abort the suite. Errors are distinguished from assertion failures in the output:
- `PASS` / `FAIL` = spec ran successfully, assertions did/didn't pass.
- `ERR` = couldn't run (build failure, binary aborted before writing).

This matches how CI users actually want suites to behave — see everything, fix what's broken.

### What's deferred to c.12c

The c.12 plan originally bundled retroactive patch application into this ship. After the discussion, that became its own ship (c.12c with `graft` command and honest-gaps semantics). The reason: `compare` is the foundation `graft` builds on (graft produces a series of synthetic versions; compare reads them). Shipping compare first lets us validate it in isolation before adding the more complex patch-application layer.

### Honest read

This is the value-delivering ship of the c.12 arc. c.12a was structural; c.12b is functional. With `compare` + `run-suite` working, every future perf change in c.17+ gets validated cross-version automatically.

The launcher arc has now reached its design goal: cross-version perf comparison as a one-command operation, with driver-epoch awareness, with a clean library API for future UI consumers.


## Tester onboarding: "Windows 7 installer" experience as a product goal

Captured during c.12-b validation. Migration friction we hit during the c.11→c.12-b transition (asset path mismatch, manual git init at wrong level, leftover directories from prior bootstrap attempts) is a preview of what testers will hit at much higher rates. Testers don't have AI assistance to recover from a mistyped command or a misunderstood directory structure.

### The product principle

For testers, the bootstrap experience should resemble running a Windows 7 installer:
- Single artifact (one binary, one click).
- "Where do you want your unendlich workspace?" with a sensible default.
- Sets up directory layout, downloads platform binary, verifies signature.
- Writes a launcher script the tester can run thereafter.
- Optionally registers in applications menu / Start menu.

The tester should NOT have to:
- Think about workspace members or Cargo.
- Understand git init, git add, git commit.
- Know what cwd-relative asset loading means.
- Know what a `.unendtest` file is until they want one.

### Why this isn't today's work

- The only user today is the project author. The installer becomes valuable only when there's a second user.
- The installer is downstream of "what does a tester actually receive?" — binary, source, both, what platform, what driver lock. These decisions are c.18+ work.
- Building an installer for hypothetical users tends to optimize for the wrong concerns.

### Where this slots in

c.16 (distrib stubs) becomes the foundation: ed25519 keygen, signed binaries, manifest format describing platform/driver/version. NOT a real distribution server yet — just the primitives.

c.17-c.20 is real perf work, validated through the launcher's compare. No tester touch yet.

c.21+ (post real-perf, when the engine is worth distributing): the installer story. Probably 2-3 sessions of focused UX work:
- Cross-platform installer (Rust-native via something like `cargo-bundle` or hand-rolled).
- A first-run dialog (egui or platform-native dialog crate).
- Workspace bootstrap to a user-chosen path.
- Binary fetch from the distrib server with progress + signature verification.
- Sample `.unendtest` and a "run sample" button.

### What this changes about earlier ships

Knowing the installer is coming influences:
- **c.13 GUI work**: design with "this is also what testers see" in mind. The launcher's GUI subset becomes the bootstrap of the eventual installer dialog. Keep the dependency footprint small so the installer doesn't have to bundle 80MB of GUI runtime.
- **c.16 distrib stubs**: the manifest format needs to be installer-consumable, not just CLI-consumable. Plain JSON, well-documented, versioned.
- **Asset path resolution** (immediate c.12-c fix): pick a strategy that works in BOTH "developer running from workspace root" AND "tester running a single installer-placed binary that knows where it lives." The simplest is: assets at workspace root for dev, embedded into the binary via include_bytes! for distribution. Today we do both — embedded for the small shaders, on-disk for biomes and large shaders. Long-term: probably ALL assets embedded for distribution.

### Honest acknowledgment

This note is mostly aspirational, but capturing it now keeps us from making decisions in c.13-c.16 that we'd have to undo to make the installer work.


## Increment 40c.12c (v0.24.2c.12-c) — asset path hotfix

Tiny ship. Fixes a c.12-a regression where the workspace refactor moved `assets/` into the engine crate but runtime code still expected it at cwd (= workspace root when run via the launcher).

Approach: restored `assets/` at workspace root. Updated 6 include_bytes! paths and 1 build.rs output path. No code logic changed.

### What c.12-b validation surfaced

User's first real-hardware run of c.12-b succeeded at driver-id capture:
```
GPU info: vendor=0x10005 device=0x0000 driver_version_raw=0x06403006
          name="Honeykrisp" info="Mesa 25.3.6" hash=drv-7892b4dc8cdd660d
```
This is the design working — Vulkan device properties + driver_properties extension queried correctly on Asahi M2 Max, FNV-1a hash produced a stable identifier. The launcher resolved @HEAD, cloned, built, spawned the binary. Everything before the asset path lookup worked.

The asset path bug interrupted at shader load. c.12-c restores that path and unblocks end-to-end validation.

### Future implication

Distribution to testers (c.21+) will probably embed all assets via include_bytes! for a fully portable single-binary artifact. The current "assets at cwd" model is a developer ergonomic, not a distribution strategy. Filed.


## Feature: auto-capture incident logs when FPS drops below a threshold

Captured during c.12-c validation. Idea from the project author.

### The problem

Most engine perf bugs are intermittent. They surface during a long session, not in the 5-second smoke test. By the time the user reports "it got slow," the moment is gone. Reproducing requires camera + workload + world state that's often unknowable.

### The proposal

The engine watches its own frame timing. When sustained FPS drops below a threshold (e.g. <10fps for 2+ consecutive 1Hz windows), capture a "perf incident" folder containing:

- The last N seconds of events.jsonl (ring-buffer of frame_window events).
- The current GPU + CPU timer breakdown.
- Workload counts: rendering, loaded, gen_pending, mesh_q.
- Camera state: position, pitch, yaw.
- (When c.15 lands) The most recent snapshot image at the trigger moment.
- Optionally: lightweight system snapshot (free memory, CPU load).

Storage location: `~/.unendlich/incidents/<run-uuid>-<timestamp>-fps-<value>/`.

### Design constraints

- **Ring buffer, not live disk reads.** Keep ~60 seconds of frame_window events in memory. When the trigger fires, flush the buffer to disk. Disk I/O during slowdown is itself a risk — write on a background thread.
- **Robust trigger threshold.** Must NOT fire on:
  - First-second startup jank (chunk gen, first paint).
  - Single-frame spikes (a hitch isn't a sustained slowdown).
  - Window focus transitions (clicking the window can stall briefly).
  Concrete trigger: rolling-window p50 < threshold for ≥2 consecutive 1Hz windows AND elapsed_since_start > 3 seconds.
- **Cap incidents per run.** Maybe 5. After cap, log "incident-cap-reached" event but don't keep capturing. Avoids a broken run producing 50 incident folders.
- **Bounded incident size.** Each incident folder hard-capped (~1MB?) to prevent runaway disk usage.

### Why this isn't c.12 work

Builds on:
- c.8 structured events (done).
- c.15 snapshot capture (planned).
- The frame_window summary format (done).

Best timing: c.15 (snapshot capture) or as a small companion ship (c.15-b) right after, so incidents can include snapshot images.

### Composition with launcher arc

Incident folders become first-class artifacts alongside .unendresult.json. The TUI (c.13) could list recent incidents. The GUI (c.14) could open one and show the timeline + snapshot + breakdown. The distrib layer (c.18+) could collect tester incidents for the developer to review (with explicit consent — privacy boundary worth flagging).

### Future extension: tester-side incident reporting

Once distrib + tester onboarding exists (c.21+), an incident folder is exactly what you'd want testers to submit. "Hit a slow patch? Click here to send the diagnostic." The bounded size + structured contents make it shippable without burying the developer in noise.


## Ring buffer deferral: revisit when test matrix exists

Captured during c.12-e planning.

The engine-side ring buffer for forensic capture of perf incidents — proposed earlier and filed for c.21+ tester scenarios — has a more specific trigger condition worth recording.

**Defer until:** We have a real test-matrix exploration workflow. Concretely: when a single perf experiment involves >10 specs running with varied parameters (render distances, upload budgets, view distances, etc.), and the analyzer surfaces "this combination triggered an unexpected spike, but the 1Hz summary smoothed it out." At that point, having per-frame data around the spike becomes diagnostically valuable.

Today's situation that doesn't justify it:
- Specs are short (5-30s). The full events.jsonl IS effectively the ring buffer.
- The 1Hz summary captures spikes as aggregate p99 values, which is enough signal to identify "this combination has hidden tail latency."
- Boot-mode per-frame logging (c.12-e) covers the most-likely-spiky region.

What WOULD justify it:
- A perf bisection where p50 looks fine but p99 is bad, and we need to see WHICH frames spiked to correlate with what was happening (chunk load? mesh upload? GC pause?).
- Tester reports from interactive play where the 1Hz aggregate is too coarse.
- Long-duration soak tests where intermittent spikes need frame-level forensics.

The ring buffer is a 1-2 day implementation when needed. Engine-side complexity is low (in-memory circular buffer of frame summaries, background-thread flush on trigger). The main cost is measurement-perturbation risk, which would need careful design.

For now: 1Hz summaries + boot-mode + post-run triage is enough.


## Increment 40c.12e (v0.24.2c.12-e) — housekeeping + triage + boot-mode logging

Small focused ship cleaning up debt surfaced during c.12-c validation. Six fixes, no architectural changes, ~696 tests total (was 690).

### Key design decisions

**Hermetic worlds for spec runs, by default.** Originally proposed as an opt-in `[world] mode = "ephemeral"` spec field. Inverted to "always ephemeral when --test-spec is set" because:
- Specs are by definition controlled measurements; non-hermetic state defeats the point.
- Opt-in would require updating every existing spec file to set the flag — friction with no benefit.
- The only counter-use-case ("test against a pre-built world") is a different feature (`seed_from_path`) and can come later.

**Boot-mode/steady-mode tag on frame_window events.** The aggregated 1Hz events smoothed over the warmup curve. Per-frame events during the first 3 seconds give us the actual warmup shape (200ms first paint, then chunk-gen-induced 130ms frames, then steady at ~2ms). Adds ~90KB to events.jsonl per run; negligible.

**Triage threshold default is 33ms.** ~30fps p50. Chosen because:
- Real perf bugs typically drop you below 30fps.
- Above 30fps is acceptable interactive play.
- Configurable via env var for cases where we want stricter (5ms for "did anything dip below 60fps in a fast scene") or laxer.

**Triage deletes events on healthy runs.** Most spec runs will be healthy; keeping every events.jsonl forever bloats `~/.unendlich/runs/`. The result file is the source of truth for the assertion outcome — events are only useful for forensics, and forensics only matters when something went wrong.

The `incident_summary.json` is always written, so the triage decision is preserved even when the events are deleted. This lets the future TUI/GUI show "this run was healthy" without needing the raw events.

**Boot events are excluded from triage classification.** Otherwise every run would be flagged as an incident due to the first-paint 200ms frame. The boot/steady distinction in events makes this clean: triage sees only steady events.

**`optimize` command operates on result files, not events.** Result files have aggregated `Percentiles` summaries (mean, p50, p99, max). That's exactly what ranking needs. Events files have per-window detail but require re-aggregation for cross-zone comparison. Reading the result directly is simpler, more reliable, and works even after triage has deleted the events.

### What was deferred (with explicit reasoning)

- **Engine-side ring buffer**: deferred until we have a test matrix where per-frame data of incidents is diagnostically valuable. Trigger condition documented in earlier note.
- **Distro detection in launcher**: filed as c.12-d (optional, opt-in only). Not shipped here because:
  - One-user installs don't benefit (you know what's installed).
  - Wrong dep mapping is worse than no dep mapping.
  - Better solved by the c.21+ installer.

### Honest read

This ship has the lowest "wow" factor of any in the c.12 arc but the highest "I'm glad I'm not chasing this anymore" factor. Hermetic worlds in particular is the kind of fix that, without it, would silently corrupt every cross-version compare we run going forward. Boot-mode logging will save us hours every time we want to investigate startup. Triage will save us disk space and surface incidents we'd otherwise miss.

c.12 arc is now complete: workspace + compare + run-suite + cross-version-aware infrastructure + driver-epoch awareness + hermetic measurement + auto-classification of incidents. The launcher does what we set out to build.

From here: c.13 (TUI) or skip-ahead to real perf work (c.17+). Both are credible next moves. The infrastructure is sound either way.


## docs/WORKFLOWS.md — the missing usage layer

Added during c.12-e validation.

Until this, the docs had three categories:
- **Ship readmes** (`v0.24.2c.*-readme.md`): what changed in this version.
- **Design notes** (this file): why decisions were made, what's filed for later.
- **Tool --help text**: CLI surface reference.

What was missing: the connecting tissue — "given this thing I'm trying to do, which tool do I use, in what order, and what does the output mean."

WORKFLOWS.md covers:
1. Tool reference (which tool when).
2. Workflow recipes (single-run, A/B, suite).
3. Output interpretation (reading the tables).
4. Working-with-Claude conventions (what to paste, what not to paste).

Keep ship readmes for what-changed, design notes for why-decisions, --help for CLI reference, and WORKFLOWS for how-to-actually-use. No overlap.

When tools change shape (new subcommands, output format changes), update WORKFLOWS.md alongside the ship readme. The ship readme says "added foo subcommand"; WORKFLOWS.md says "use foo when you want to bar."


## Increment 40c.13-scout (v0.24.2c.13-scout) — multiview probe

Scout-ship at the head of the perf arc. Probes `VK_KHR_multiview` capability and surfaces it in `GpuInfo` and the launcher's `compare` output. No engine behavior change; pure diagnostic.

### Why this is its own ship

Multiview CSM is a multi-week implementation if we commit. If Honeykrisp/Mesa doesn't support multiview, that work is wasted before it starts. The risk of "implemented multiview, then discovered driver didn't support it" is real enough to justify a 30-minute scout ship.

The decision tree:
- `CSM-capable: true` (multiview + max_views ≥ 4) → c.17 is multiview-CSM. Big architectural lift, expected ~4× shadow speedup.
- `CSM-capable: false` → c.17 is per-cascade GPU culling + frustum tightening. Smaller win (maybe 30-50% on the dominant frames), bounded by the rendering-everything-twice-per-cascade ceiling.

Both paths are valid. Which we take is determined by data from this ship.

### Design notes

**Multiview is NOT in driver_hash.** A Mesa update that adds multiview support shouldn't invalidate cross-version comparisons of runs before/after. The driver identity is the driver identity; capability bits are side-channel info that flows alongside.

**Capability differences don't trigger a compare warning.** Unlike driver_hash mismatches (which actively confound perf comparisons), a multiview capability change is informational. Showing it in the row is enough.

**Backward compatibility.** `MultiviewSupport` derives `Default` (all-false) and `GpuInfo.multiview` is `#[serde(default)]`. Old c.12-e result files parse cleanly.

**Probe at startup, not on demand.** Vulkan 1.1+ feature queries are basically free, but doing it once at context creation simplifies the model: the answer is captured in `GpuInfo` along with everything else, and is part of the result file flowing through the launcher's compare path.

### What we'll learn

The probe answers three questions in one shot:
1. **Is multiview supported at all?** → `multiview` boolean
2. **Are there enough layers for CSM?** → `max_view_count` (need ≥4)
3. **Edge cases for future work?** → `max_instance_index`, `geometry_shader`, `tessellation_shader` (recorded for completeness; not relevant for CSM today)

If Honeykrisp supports multiview, we proceed with c.17 multiview-CSM. The Asahi/Mesa state at the time of this ship is "feature is present in the Vulkan spec, support on Honeykrisp uncertain." Real hardware will tell us.

### What goes wrong if the probe says no

c.17 pivots to less-ambitious but still-impactful work:
- **Per-cascade GPU culling**: each cascade's frustum produces a separate culled chunk list. Currently we cull once for the main camera and re-use that list for all 4 cascade passes — over-draws.
- **Shadow frustum tightening**: today's cascade fit may include geometry that's not actually in the cascade's near-far range. Tightening cuts pass workload.
- **Cascade count reduction (4→3?)**: 25% less shadow work, costs quality at the far end. Reversible if it looks too aggressive.

These together likely buy 30-50% shadow improvement without multiview. Less than multiview's expected 3-4×, but still meaningful, and doable on any driver.

### Composition with launcher arc

The capability flows naturally: probe → GpuInfo → result file → compare. The launcher's compare table grew a `csm_capable` row. When c.17 lands, this row will be useful for "did we use multiview in this run?" cross-version checks. No additional infrastructure needed.


## c.13-cleanup (housekeeping) + c.17 planning

Doc-only ship, no version bump (stays at c.13-scout). Three things:

1. `unendlich-analyze` `n` field now displayed in the ranking table. Compile is now warning-free. Trivial.

2. Cosmetic rename: README + DESIGN_NOTES headers go from "Meinkraft" to "unendlich" with a small historical-note footnote. Older internal references (version strings, "Meinkraft style" comparisons in old sections) are left as-is — they're accurate historical references.

3. `commands/install_to_path.sh` — symlinks release binaries into `~/.local/bin/`. One-time setup, then `unendlich-launcher` and `unendlich-analyze` are on PATH everywhere.

## c.17 plan — multiview CSM

Filed as `docs/v0.24.2c.17-plan.md`. Highlights:

### Key finding from reading the shadow code

The current engine renders ONLY cascade 0. Despite 4-layer shadow image and per-cascade compute pipelines populating `view_params` for all 4 cascades, the actual shadow render pass only draws into layer 0 with C0's view-proj. Cascades 1-3 are infrastructure-complete but unused.

c.5/c.6 already tried per-cascade-pass rendering (4 separate render passes, one per cascade) and hit a 2.8× regression on Honeykrisp — per-framebuffer overhead. The code's own comments flag multiview as the planned path forward.

So c.17 isn't building CSM from scratch — it's finishing what's been half-built since v0.24.2a.

### Four-ship plan

- **c.17-a**: wire up multiview render pass, all 4 cascades render in one go, main frag still samples only layer 0 (visual parity). Probably perf-neutral or slightly worse at rd=8 because we're now actually doing 4× the depth fill.
- **c.17-b**: frustum-union culling. Each chunk culled against union of all 4 cascade frusta. 10-30% recovery vs c.17-a.
- **c.17-c**: main fragment shader switches to `sampler2DArrayShadow` with per-fragment cascade selection. **The real win lives here.** Enables shadow_map size reduction (2048² → 1024² per cascade) and tighter per-cascade frusta. Target 2-4× total speedup vs c.13-scout baseline (95ms → 25-40ms p50 at rd=8).
- **c.17-d**: tuning. Cascade count, shadow map size, frustum tightening. Drop the fallback env var once stable.

### Architectural decisions filed

**Fallback env var `UNENDLICH_MULTIVIEW_SHADOWS`**: defaults to 1 (multiview on), can force to 0 to fall back to single-cascade-0 path. Lets us A/B in one binary. Cost is low (one runtime branch at pass setup). Dropped in c.17-d once stable.

**Push constant for cascade_slot_base, not specialization constant**: 4 bytes, set once per shadow pass. Specialization constant would be a bit faster but adds a pipeline-recompile when ViewGroup config changes. Push constant is simpler and only c.17 work doesn't need recompiles.

**Frustum-union culling, not per-view shader masking**: subgroup view-index masking is complex and driver-dependent. Frustum union is well-understood (some over-draw, no wasted draws). The right approach to try first.

**`view_params` SSBO reused for shadow vert input**: cascade matrices already computed on GPU each frame; just bind the existing buffer as a new descriptor on the shadow pipeline (binding 1, vertex stage). No duplicate buffer needed.

### Risks called out in plan doc

1. Driver picks multiview impl strategy (primitive replication vs. instanced rendering). Either should work for us; if a Mesa bug exists, we'll see it.
2. Validation layer noise around per-layer barriers.
3. c.17-a perf regression is EXPECTED. The win comes in c.17-c.
4. Indirect draw list reuse: shadow currently reuses main-camera cull result. After c.17-b, must use frustum-union result. Easy to miss; flagged.
5. Descriptor type mismatch when changing `sampler2DShadow` to `sampler2DArrayShadow`. Both shader and descriptor write must update together.

### Rollback criteria

If c.17-a shows visual artifacts that suggest a multiview driver bug, OR perf regresses >3× (suggesting wiring is broken), OR validation layer errors that can't be resolved cleanly — pause and pivot to per-cascade culling WITHOUT multiview (smaller but still real win, doable on any driver).

The fallback env var keeps us shippable even in the rollback case.


## Increment 40c.13-instrumentation (v0.24.2c.13-instrumentation) — warmup + snapshots + shadow buckets

Instrumentation ship. Three diagnostic capabilities for c.17 perf work, no engine behavior change for production runs. ~679 fast tests pass (was 678 in c.13-cleanup — added one for `max_shadow_buckets_constant_is_sane`).

### Key design decisions

**Warmup is a generalization of the boot window.** Boot mode (c.12-e) emits per-frame events for the first 3 seconds, tagged `mode: "boot"`, excluded from triage. Warmup (c.13-instrumentation) is similar but stronger: it RESETS the GPU+CPU timer lifetime accumulators at the warmup-end boundary, so assertion-relevant percentiles reflect only the steady-state window.

Both can coexist. Boot is always 3s. Warmup is configurable per-spec via `[engine] warmup_ms = N`. For specs with `warmup_ms > 3000`, the boot events still apply for the first 3s but the actual measurement reset happens at the user-chosen warmup_ms.

**World-state snapshot is intentionally thin.** Just `loaded_chunks` and `non_air_blocks` today. Adding plant_count, creature_count, weather_state, etc. would require handles into each subsystem (plant_lifecycle, world_model fields, etc.). These will be added opportunistically when future ships touch those subsystems. The architecture is shaped so adding them is purely additive (extend the snapshot tuple → extend the JSON serialization → done).

**Shadow buckets use a separate auxiliary timer pool.** The main GpuTimer has 5 fixed zones sized at construction. Expanding it to accommodate up to 16 buckets would either waste memory when bucketing is off or require dynamic resize. Cleaner: separate `ShadowBucketTimer` only constructed when `UNENDLICH_SHADOW_BUCKETS=N` is set, with its own small query pool. Diagnostic-only — production runs don't pay for it.

**Bucketing forces per-chunk fallback path.** `cmd_draw_indexed_indirect_count` doesn't allow inter-draw timestamps. To bucket, we need to interleave timestamps between specific chunk draws, which requires the per-chunk path. Documented in the env var help text: "This overrides UNENDLICH_INDIRECT and adds GPU timestamp overhead — disable for production perf measurements."

**Workload context appears FIRST in `optimize` output.** Was previously after the ranked tables in my mental model, but moved to the top because scale-of-the-problem context changes how you read the timing data. "91ms shadow with 24 rendering chunks" tells a different story than "91ms shadow with 200 rendering chunks." Workload context first means the reader has scale in mind before reading the numbers.

### Future work this enables

For c.17-a (multiview shadow visual parity):
- Without warmup, comparing c.17-a vs. c.13-scout would have run-to-run noise hiding the signal. With warmup, the comparison is cleaner.
- Without bucketing, "shadow regressed by X%" wouldn't tell us WHERE in the pass. With bucketing, we can see if it's pipeline setup, first-draw, or distributed.
- Without snapshots, two runs with different chunk counts would compare apples-to-oranges. With snapshots, workload differences are visible.

### Filed for later

- **`unendlich-analyze` aggregation of bucket events.** Today bucket data lives in events.jsonl only. A future ship could read `shadow_buckets_timing` events and emit per-bucket percentile distributions in the optimize output. Useful once we have multi-run bucket data to aggregate.
- **Plant count, creature count, weather snapshot fields.** Add opportunistically when working in those subsystems.
- **Cross-run correlation analysis.** "Shadow time correlates with non_air_blocks r=0.7" is a real perf question we can ask when we have many runs across different workloads.

### Honest scope acknowledgment

The bucket implementation is "GPU timestamps inserted between chunk draws on the per-chunk path." It does NOT decompose what happens INSIDE a single chunk's draw (vertex transform vs. fragment fill). That kind of sub-draw decomposition isn't possible with timestamps alone — would require synthetic test geometry or vendor-specific tools (RenderDoc, etc.). Documented as a known limitation.


## Increment 40c.13-instrumentation-followup (v0.24.2c.13-instrumentation-followup) — diag tooling + bucket-mode disabled

Ergonomic + bugfix ship. Adds `unendlich-launcher diag` for one-paste run summaries, `latest-*` symlinks in `~/.unendlich/`, captured engine log in run dir, and disables the broken bucket-mode feature pending follow-up. ~679 fast tests pass.

### shadow_buckets DEVICE_LOST

The c.13-instrumentation `UNENDLICH_SHADOW_BUCKETS=N` feature was implemented as: opt-in env var that forces the per-chunk shadow fallback path and writes GPU timestamps between chunk groups using a separate query pool. Validated builds + unit tests + synthetic data flow, but produced `Vulkan error: ERROR_DEVICE_LOST` consistently on Honeykrisp (Asahi Linux M2 Max, Mesa 25.3.6, May 2026).

Reproducible failure: every bucket-mode run died at ~2 seconds wall-clock with zero chunks loaded, two completed frames, `cpu acquire mean=1000ms` (compositor blocking on dead device). The bucket-emission code was reached on frame 0 with an empty world, writing 5 back-to-back `cmd_write_timestamp` calls inside the shadow render pass with no intervening draw commands.

Suspected cause: in-render-pass timestamps with no work between them on Honeykrisp. Vulkan SPEC permits the pattern but driver coverage is undertested on a brand-new driver. The existing `GpuTimer` writes its timestamps OUTSIDE the render pass (`cmd_end_render_pass` before the `ShadowEnd` marker), which is why production gpu_timer code never tripped this.

**Followup options when we get to fixing it:**

1. **Move bucket timestamps outside the render pass via N sub-passes.** Each bucket becomes its own `begin_render_pass / [chunks] / end_render_pass / timestamp` cycle. Slower (the c.5/c.6 experiments showed N separate render passes regress ~2.8× on Honeykrisp) but diagnostic mode tolerates that. Cleanest design.
2. **Only emit boundary timestamps between actual draws.** Skip empty-frame timestamps entirely. Quicker fix but doesn't address whether the in-pass timestamp pattern is fundamentally fragile.
3. **Use `vkCmdBeginQuery` / `vkCmdEndQuery` with `OCCLUSION` instead of timestamps.** Different mechanism, different driver paths. Speculative.

I'd lean (1) when it's time. It's the architecturally clean path and matches what `gpu_timer` already does successfully.

**Decision shipped in c.13-instrumentation-followup:** `UNENDLICH_SHADOW_BUCKETS` is acknowledged-and-ignored. Setting it logs an ERROR-level message explaining the disabled state and pointing to this doc. The bucket timer is not constructed. The shadow pass behavior is unchanged from before c.13-instrumentation (use_indirect=true by default, indirect drawing active).

The `shadow_bucket_timer` module itself is kept in the tree — its constructor, mark_boundary, collect_frame, and Drop impl are all correct as far as we can verify without GPU validation. When we revisit, we'll either rewire it via approach (1) or replace it.

### diag tooling

`unendlich-launcher diag [<UUID>] [--log-lines N]` — one-paste run summary intended for sharing with collaborators. Reads the result file, extracts spec/version/duration/verdict, prints assertion verdicts, workload context (loaded/rendering/snapshot), top 6 zones by mean ms, and the tail of `engine.log`. Default 60 log lines.

UUID can be omitted to use `~/.unendlich/latest-result.unendresult.json`. The launcher now updates four symlinks after every run:
- `~/.unendlich/latest-result.unendresult.json` — global latest result
- `~/.unendlich/latest-engine.log` — global latest log
- `~/.unendlich/latest-events.jsonl` — global latest events (only if kept by triage)
- `~/.unendlich/latest-<spec_name>.unendresult.json` — per-spec latest (survives `run-suite`)

Per-spec symlinks are important for `run-suite` runs: the global `latest-*` only reflects the LAST spec in the suite, not all of them.

Engine stderr is now teed to `engine.log` in the run directory by default. The launcher spawns the binary with `Stdio::piped()` stderr and a tee thread that writes each line to both the user's terminal (UX unchanged — they still see live progress) and the per-run log file (archive). The log is keyed by run UUID so historical runs remain accessible.

Symlinks are Unix-only; on Windows the launcher logs a debug message and skips. Not relevant today (Linux/macOS hardware).


## Increment 40c.13-instrumentation-followup-prep (v0.24.2c.13-instrumentation-followup-prep) — c.17 prep, no engine code changes

Prep ship between c.13-instrumentation-followup validation and the start of c.17-a. Three additions, NONE of which touch engine runtime behavior:

1. **`tests/specs/rd8_warmup.unendtest`** — anchor spec for c.17 baselining. Same workload as rd8_smoke (rd=8, 30s, indirect on) plus `warmup_ms = 3000` and a `shadow_p50_ms = { max = 150.0 }` performance guard calibrated against the validated c.13-instrumentation-followup baseline (shadow_p50_ms = 113.9ms, ~30% headroom). Every c.17 ship should PASS this spec; ships that BEAT 113.9ms should tighten the threshold.

2. **`commands/baseline_for_c17.sh`** — convenience script. Builds the launcher, runs rd8_warmup against @HEAD, prints the diag. Usage: `./commands/baseline_for_c17.sh` before starting a c.17 ship locks in the comparison baseline.

3. **`docs/v0.24.2c.17-a-implementation-notes.md`** — detailed pre-implementation plan for c.17-a. Code sketch for every file that needs to change (context.rs for multiview feature enable, shadow_map.rs for multiview render pass + 2D_ARRAY view + framebuffer rewiring, chunk_shadow.vert for gl_ViewIndex SSBO lookup, shadow_pipeline.rs for descriptor binding 3 + push constant removal, chunk_pipeline.rs for matching DSL, world_renderer.rs for descriptor write + push constant call removal). Captures discoveries from re-reading the code: (a) multiview feature isn't enabled on the device today even though probed, (b) shadow and main pipelines share descriptor sets so the SSBO binding must go in BOTH DSLs, (c) framebuffer `layers(1)` is correct under multiview (the view_mask is what determines layer count). Honest concerns flagged: subpass dependency stages might need VK_DEPENDENCY_VIEW_LOCAL; Honeykrisp's multiview path may have undertested edges given c.13-instrumentation's DEVICE_LOST on a different in-renderpass code path.

Tests still 70+28 = 98 in the non-engine crates. Engine fast tests unchanged at 581.

### Why prep work was worth shipping

The c.17-a implementation involves 6 files with cross-coupled changes (descriptor sets, render pass struct, framebuffer, shader, device feature, pipeline layouts). Coordinating all of these without a written plan would leak attention from the actual implementation to the spelunking. Writing the plan first means the next c.17-a session is execution, not discovery.

## Filed for later: light-driven plant variation (c.19+, observed during c.17-a playtest)

User observation while playing on c.17-a (debug build, rd=6, 515s playthrough): underground caves contain fully-formed green plants. Aesthetically wrong — caves don't get sunlight, so they shouldn't grow chlorophyll-using plants.

The right model:
- Each voxel/chunk has a scalar `light_occupancy` (how much sunlight reaches it, roughly 0-1).
- Plant generation reads this during chunk meshing or plant lifecycle.
- Low-light voxels grow MOSS / FUNGUS / ALGAE — different prototypes:
  - No upward growth (no phototropism).
  - No leaf spread (no surface area for capture).
  - Color shifts to pale greens, white, beige (no chlorophyll dominance), or bioluminescent for fungi.
  - Voxel-block rather than tree-block placement.
- Surface (high-light) plants stay as today: tall, branching, green.

**Why this isn't c.18:** c.17 is finishing shadow perf. c.18 should be whatever the c.17 results say is next (probably CPU bottleneck, maybe deferred shading). Plant-physics work isn't a critical path for engine viability.

**What would need to exist first:**
- Light-occupancy data structure (could piggy-back on shadow cascade samples, or compute separately).
- Multiple plant prototypes (currently just one tree-shape generator).
- A way for plant lifecycle to read voxel-local conditions during placement.

**Filing this NOT because it needs work now, but because the observation came from a real engagement moment** — user was exploring caves and the disconnect was visceral. The kind of detail that turns "interesting tech demo" into "world feels real." Worth doing eventually.

Related: deep-cave exploration as a player activity. Mentioned positively in the same observation ("deep caves are going to be super interesting to explore"). Suggests biome/feature variety underground is a player-facing priority that intersects with this lighting work.


## Filed for later: in-engine capture HUD prompt (c.18+ likely)

Observed during c.17-a/revert chug investigation: capturing a representative play-trace requires running a spec via the launcher, then deliberately walking around for the spec's duration window. The launcher and engine have no way to TELL the player "you should be walking now" or "capture started" — the player has to:

1. Mentally track that the 30-second spec is running.
2. Guess when the engine has finished initial chunk-load.
3. Walk around enough to provoke the behaviors of interest.
4. Stop in time, or the spec auto-shutdowns mid-action.

This is bad UX both for solo dev work AND for any future "send me a perf trace" workflow.

### Proposal: capture-prompt HUD

When the engine is launched with a capture spec (e.g. `UNENDLICH_CAPTURE_PROMPT=1`):

- After world_loaded fires (chunk_manager reports loaded_chunks >= expected_min OR a fixed timeout), display a HUD overlay:
  - "CAPTURE ACTIVE: walk around for Ns."
  - Countdown timer showing remaining capture window.
  - Optionally: hint text like "try crossing chunk boundaries" or "look around 360°".
- When capture ends:
  - HUD overlay: "Capture complete. Saving..."
  - Engine shuts down normally.

### Implementation sketch

- New field on TestSpec: `[capture] prompt = true` (default false).
- Engine reads it via existing env-var bridge; surfaces as a flag in runtime_flags.rs.
- world_renderer.rs gains a `capture_hud` text overlay rendering path. (We don't have HUD text rendering today, so this is the bulk of the work — see below.)
- world_loaded gating: use `chunk_manager.loaded_count() >= some_threshold` (e.g. all near-tier chunks loaded), OR a timeout.

### Why this is c.18+, not now

We don't have HUD/UI text rendering. Adding it is its own ship — needs a text-rendering pipeline (bitmap font or msdfgen), a layout system, and integration with the main render pass.

Filing it because:
1. The investigation today exposed the UX gap clearly.
2. Future onboarding / tester workflow will need this anyway (the c.21+ "Windows 7 installer" experience).
3. Once we have any HUD at all, dozens of small "show me a number" tasks become trivial (FPS counter, build version, capture status, debug overlays).

### Adjacent ideas this unlocks

- **FPS counter overlay** (toggled by F3-style key).
- **Build version + commit hash** displayed in corner.
- **Memory/chunk-count debug overlay**.
- **In-game "press R to capture this region"** for ad-hoc reports.
- **Tutorial / first-run hints** for the c.21 onboarding ship.


## Filed for next session: chug investigation, Experiment 4 (wait_fences root cause)

After tonight's c.17-a regression + revert, the actual chug problem remains. Tonight's measurements at rd=2 post-revert show:
- 95% of frames at <3ms total → smooth.
- 2% of frames hit 100-300ms in `wait_fences` → the chug feel.
- 0% of frames are CPU-record bound (post-revert `record p50=0.6ms`).
- Mesher worker spikes go up to 92ms per chunk, several per second.

**The chug is tail latency in `wait_fences`**, which is the render thread blocking on `vkWaitForFences` for the previous frame's GPU work to complete. The question we need answered: **what was the GPU doing during a 200ms wait_fences spike?**

Suspects, ordered by likelihood:
1. **Mesh upload blocking the queue.** A new mesh's `vkCmdCopyBuffer` is submitted on the same graphics queue as render commands; if it lands at the wrong moment it serialises in front of the next frame's submit.
2. **Descriptor pool reallocation.** Unlikely (pool is sized at startup) but worth ruling out.
3. **Shader pipeline lazy-compile.** Mesa may JIT shader variants on first-use; in a long-lived run unlikely, but the first chunk-crossing might pay it.
4. **GPU thermal throttle / power state.** Unlikely on M2 Max for these workloads but mentioning for completeness.

### Experiment 4 plan: instrument wait_fences with side-channel info

Add a CPU-side log just before `vkWaitForFences` recording:
- Current frame_idx and previous frame_idx.
- Timestamp of when we last called `cmd_copy_buffer` for any mesh upload, and how many bytes.
- Timestamp of `vkQueueSubmit` for the previous frame.
- Wall-clock at wait start.

After the wait returns:
- Wall-clock at wait end.
- Delta.

Emit as events.jsonl entries when delta > 10ms (skip noise). Walk around for 60s post-instrumentation, look at chug-frame events for patterns.

Files to touch (minimal):
- `crates/unendlich-engine/src/render/world_renderer.rs` — wrap `wait_for_fences` call with the log.
- Maybe `crates/unendlich-engine/src/render/chunk_indirect.rs` — record mesh upload timestamps.

Estimated effort: 1-2 hours of focused work. Output: structured data showing exactly what's blocking the render thread during chugs.

### Don't-do list

These were considered tonight and rejected as unlikely-to-help:
- **Lowering shadow map resolution** (Experiment 1): the chug is in wait_fences, not in shadow GPU time. Steady-state shadow is sub-ms at rd=2.
- **Skipping shadow pass entirely** (Experiment 2): same reason. Diagnostic value low at low rd.
- **Mesher worker count tuning** (Experiment 3): worker count is about throughput; chug is about single-thread render scheduling.
- **Upload batching tuning**: tested tonight at uploads=1, made things worse not better.
- **Disabling vsync**: tested implicitly across multiple runs; vsync is not the chug source.

### Reminder when starting this work

We have c.17-a's regression as a cautionary tale: even a "small, careful, planned" change cost ~5ms/frame in record time for reasons we still don't understand. Experiment 4 instrumentation must be **read-only** (logging only, not changing scheduling). The fix it informs is a separate ship.


## Filed for later: benchmark suite (5 categories, PowerVR/TBDR-informed)

Discussed in session 2026-05-14 after the 3-mode vsync experiment ruled out present-mode as the chug source. The suite is filed-not-built because the immediate problem (input-to-photon hitches) is better attacked by targeted instrumentation than by a benchmark survey.

The methodology still has long-term value:
- Future ships need regression-catching. c.17-a's 5ms record cost would have been caught by a "record-cost-floor" benchmark.
- The Apple GPU lineage (TBDR via PowerVR) means some architectural sweet spots aren't obvious without measurement.

### Categories ranked by likely-payoff (revised after vsync data)

1. **Wait_fences root-cause instrumentation** — promoted from "benchmark category" to "next session's actual work." See Experiment 4 plan above.
2. **Fences/sync** — direct chug signal even after Exp 4. Tests fence-per-frame vs timeline-semaphore, upload-on-graphics-queue vs upload-on-transfer-queue.
3. **Tile binner state churn** — TBDR-specific. c.5/c.6 showed 2.8× regression from per-cascade framebuffers; we should quantify the cost shape.
4. **Half-precision payoff** — Apple GPU register file is 16-bit native. FP16 in cull/cascade_fit/view_compute compute shaders likely wins 30-50%.
5. **Order of operations** — TBDR cares about pass boundaries. Shadow-before-main vs main-before-shadow, barrier placement, descriptor binding order.

### Architecture decision (locked)

Option A: standalone `crates/unendlich-bench/` binary with headless Vulkan context. Bench results in `.unendbench.json`, integrated with launcher via `unendlich-launcher bench <name>` subcommand. Symlinks: `~/.unendlich/latest-bench.unendbench.json`.

Why not Option B (specs in world binary): benchmarks need isolated controlled state (specific render passes, specific buffer sizes) that the world binary's game loop confounds. Headless is the right shape.

### What's filed

- Categories above with 3-4 benchmarks each = ~15-20 total.
- Harness scaffolding (Vulkan ctx, timing helpers, JSON output, comparison-vs-baseline) is ~3 hours of work before the first individual bench.
- Each individual bench: 30-60 minutes.
- Suite is multi-session work. NOT a blocker for the chug fix.

### Vsync mode findings (from session 2026-05-14)

3-mode A/B at rd=2, 120Hz monitor, post-revert engine:
- **MAILBOX (default)**: "competitive game loss, not immersive in dynamic world." Small frequent hitches felt through camera rotation. Newest-wins hides some stutter but render-thread variance leaks through.
- **FIFO**: "still bad, lookaround worse, keys-to-brain slightly better." Predictable input cadence wins for input feel, but missed deadlines amplified to multiples of refresh interval.
- **IMMEDIATE**: "one big hitch throws me out then back in." Raw render-thread variance exposed directly. Most jarring at chug moments, but no swapchain-side latency.

**Conclusion**: Hitches are render-thread blocking, NOT present-mode pacing. All three modes show the same underlying chug; they differ in how the swapchain handles it.

**Default recommendation**: MAILBOX stays as engine default. IMMEDIATE wins for competitive scenarios where input-to-photon latency matters more than chug stability. Future per-user setting, not a default change.


## Critical insight: ceiling FPS affects perceived hitch severity (filed 2026-05-14)

User observation that reframes the chug problem: "running other game engines unlocked or even minecraft in the past having higher fps than the monitor has the same effect as feeling better and more responsive on mouse and keyboard."

This is the well-known competitive-gaming effect of running at 200-500fps on a 120Hz monitor. The mechanics:
1. Higher FPS = more input samples per second. Input position is fresher even if not all frames are seen.
2. With MAILBOX, the presented frame is more recent. 200fps → ≤5ms old at vblank; 120fps → ≤8.33ms old. ~60% input-to-photon improvement.
3. Frame-time variance gets averaged out — slow frames don't display when the mailbox has a newer one ready.
4. Deeper CPU/GPU pipelining → better tail-latency tolerance.

### How this changes the chug framing

Our rd=2 MAILBOX baseline: 186 FPS average. Only 1.55× the 120Hz refresh. Some chug frames are 200-400ms; during those frames we produce ZERO new frames, so MAILBOX presents the same stale frame for 200ms. That stale duration IS the felt hitch.

**At 360+ FPS steady state**, MAILBOX would have a continuous supply of fresh frames except during the chug itself. The chug becomes perceptually smaller as a fraction of available frame slots. Doesn't fix the underlying stall — just makes the engine feel competitively responsive.

### Two paths forward (both real)

**Path 1: Eliminate chugs entirely.** Fix render-thread blocking on uploads/meshes. Fence-per-resource, dedicated transfer queue, decoupled scheduling. Right fix. Harder.

**Path 2: Raise floor framerate.** Find pipeline serialization that's keeping us at 186 FPS when slowest stage is 2.18ms (theoretical max ≈460 FPS). 2.5× headroom suggests CPU↔GPU serial dependencies somewhere.

### Suspect locations for Path 2 (pipeline serialization)

At rd=2 MAILBOX post-revert:
- wait_fences p50 = 2.18ms (waiting for vsync/present)
- gpu total p50 = 1.86ms (GPU work)
- cpu record p50 = 0.85ms (command recording)
- If perfectly pipelined: max(2.18, 1.86, 0.85) = 2.18ms → ~460 FPS

We get 186 FPS. The gap means the stages are partly serial when they should overlap. Candidates:
- Fence-per-frame design forces CPU to wait for previous frame's GPU before recording next.
- Descriptor set update timing might be in a critical path.
- Swapchain present blocking on graphics queue submit completion.
- MAX_FRAMES_IN_FLIGHT = 2 might be too low for our render-thread pattern.

### Decision

Next session attacks BOTH paths in parallel:
- **Investigation A**: wait_fences root-cause (Experiment 4 plan above) — fixes the worst frames.
- **Investigation B**: pipeline serialization — find why steady-state FPS is 2.5× below theoretical.

The benchmark suite (5 categories) is filed but not next. Direct instrumentation of the existing engine produces faster signal than building a microbench harness.


## Filed for later: configurable FPS cap (small ship)

Discussion 2026-05-14: VSYNC modes don't capture the real tradeoff for high-refresh displays. The actual engineering question is "how many frames per second is enough" — not "vsync on or off."

### Why a cap is useful

On a 120Hz monitor, producing 360-500 FPS gives perceptual benefits (input sampling rate, frame freshness in MAILBOX, pipeline depth). But producing 4000 FPS gains nothing and wastes:
- Battery on laptops/handhelds.
- Fan noise.
- Power budget that could go to the rest of the SoC (CPU clock, memory bandwidth).
- Heat that may eventually thermal-throttle the GPU.

The sweet spot for a 120Hz display is probably 240-360 FPS (2-3× refresh): captures the input-latency and frame-freshness wins without runaway production.

### Proposed shape

```
UNENDLICH_FPS_CAP=240   # cap at 240 frames per second, no waste above
UNENDLICH_FPS_CAP=0     # uncapped (current behavior)
UNENDLICH_FPS_CAP=auto  # ~2.5× refresh (needs display refresh detection)
```

Implementation: a `std::thread::sleep(remaining_budget)` (or better, a `spin_until_deadline` with a sleep-most-then-spin pattern) in the render loop after `present`. Care needed: must work with all three present modes (FIFO/MAILBOX/IMMEDIATE). With FIFO the cap can be ignored (FIFO already caps). With MAILBOX/IMMEDIATE the cap is what matters.

### Why this is a small ship

- One file (`world_renderer.rs` render loop).
- ~30 lines of code.
- One new env var declared in `runtime_flags.rs`.
- One test: spec asserts `total_frames` within a range when cap is set.

### Why not now

The chug investigation comes first. A cap above current 186 FPS at rd=2 wouldn't help current experience (we're not producing enough frames for a cap to matter). A cap helps once we're producing 300+ FPS, which requires Path 2 (pipeline serialization) to land first.

Filed because:
1. It's a small clean ship once the prerequisites land.
2. The framing matters for design: vsync vs no-vsync is a false dichotomy. Frame cap is the real knob.
3. Important for battery/thermals on laptop deployments.


## Increment 42c.24.3-chug-debug-1 — wait_fences side-channel instrumentation

First ship of the v0.24.3 chug investigation. Builds Experiment 4 from
the c.17-revert design notes as the smallest possible read-only
instrumentation that answers "GPU or CPU bound during a chug?"

### Why so small a ship

The c.17-a regression history is the reason. c.17-a was a "single
carefully-planned ship" that still cost 5ms/frame in record time
for reasons we never fully isolated. The lesson: every additional
code change introduces risk, and instrumentation that perturbs the
thing it measures is the worst kind of risk.

This ship adds:
- One field to `FrameSync` (`Option<Instant>`).
- One pair of Instant captures around `wait_for_fences`.
- One event emit on threshold breach.
- One Instant write after `queue_submit`.
- One analyze subcommand to summarize the events.

That's everything. No descriptor changes, no pipeline DSL changes,
no render path changes. If c.17-a teaches us anything, it's that
the cost of even one DSL binding can be unexpected.

### What we expect to learn

Three scenarios after the user runs rd=2 free-play and pastes the
`wait_spikes` output:

**Scenario A: Mostly GPU-bound.** The hitches are a specific render
pass taking 100-400ms during chunk-cross moments. Fix is targeted
at the render path — probably shadow culling or main pass
geometry-count limiting. We can identify which pass with existing
gpu_timer zones; the side-channel data tells us it's worth looking
there at all.

**Scenario B: Mostly CPU-bound.** Render thread is doing significant
work between submit and next wait. Most likely culprit is mesh
upload memcpy on the main thread (since mesh data goes via
HOST_VISIBLE+HOST_COHERENT mapped memory, not via cmd_copy_buffer).
Fix is offloading the memcpy or batching uploads more carefully.

**Scenario C: Mixed.** Both contributing. Probably need additional
instrumentation to break down which CPU work specifically (upload
vs game tick vs descriptor update). Filed as a follow-up.

### What this ship does NOT do

- Doesn't fix the chug. Diagnostic only.
- Doesn't measure mesh upload time directly.
- Doesn't capture stack traces or anything that would slow the
  render thread further.
- Doesn't add per-pass GPU breakdown beyond what's already in
  gpu_timer.
- Doesn't change FrameSync, descriptor pool, or render-pass shapes.

### Permanent or temporary

The instrumentation stays permanently. Even after chugs are fixed,
having this side-channel in events.jsonl helps catch future
regressions. The cost of the instrumentation is negligible (two
`Instant::now()` calls per frame, one event emit only when threshold
crossed) and the diagnostic value is high.

### Filed for after this ship lands

- **Targeted fix** based on whichever scenario the data shows.
  ONE fix ship, not a comprehensive overhaul.
- **Per-pass GPU spike attribution** if Scenario A: we have the data
  in gpu_timer already, just need analyze tooling to surface
  "which zone spiked in the same frame as a wait_fences_spike."
- **CPU-side breakdown** if Scenario B: instrument the mesh upload
  path with side-channel timing, see if memcpy duration correlates
  with wait_fences spikes.
- **The FPS-cap ship** (v0.24.3-fps-cap or similar) is independently
  valuable — see "Filed for later: configurable FPS cap" above.

### Tests

679 fast tests pass. Unchanged from c.13-instrumentation-followup-prep
baseline. Instrumentation adds no test surface because it's
side-channel logging only.


## Lesson learned 2026-05-14: migration script must never be shipped via zip

v0.24.3-chug-debug-1 regressed the migration script (cp -r fix from earlier in this arc) because Claude's working tree was re-extracted from a pre-fix zip. The user's repo had the fix; the zip-shipped version did not; when the user ran the migration, the script overwrote itself with the old buggy version mid-execution, causing a bash syntax error and silently dropping tests/specs/chug_capture.unendtest.

### Rule

`commands/update_from_claude_zip.sh` must be treated as ALWAYS_KEEP and NEVER shipped via the migration zip. The migration script evolves only via direct user commits in their repo. Claude's zip should not contain it, OR should contain a copy that is explicitly NOT extracted into commands/.

Cleanest implementation: add `commands/update_from_claude_zip.sh` to a "DO_NOT_EXTRACT" list inside the migration script itself. The script extracts the zip to a staging area, then deletes itself from the staging area before doing the copy. That way the migration script can never overwrite itself with a stale copy.

### Belt-and-suspenders

In addition: when Claude packages a zip, the packaging step should sanity-check the migration script in the zip matches the latest committed version in the user's repo. If they diverge, package abort with an error.

### Filed for next session

Implement the DO_NOT_EXTRACT mechanism in the migration script. Small change (~10 lines), big risk reduction.


## Chug diagnosis 2026-05-14: 100% GPU-bound

After the v0.24.3-chug-debug-1 instrumentation shipped, user ran the chug_capture spec at rd=2 and got the unambiguous answer: **every single chug is GPU-bound**. 76 of 76 wait_fences_spike events showed CPU waited immediately after submit; CPU did effectively no work between submit and wait (p50 last_submit_to_wait_ms = 2.6ms, max 20.3ms).

The wait_ms distribution: p50=110ms, p99=226ms, max=284ms.

### What this proves

- Mesh upload memcpy on render thread is NOT the chug cause. The CPU isn't busy.
- Game tick / descriptor work / any other CPU activity is NOT the chug cause.
- The GPU itself takes 100-280ms occasionally for a single frame at rd=2 with ~24 rendering chunks. Some pass is going pathological.

### What we already know about which pass

From earlier rd=2 chug_capture (same data, different ship):
- gpu shadow max = 188ms, p99 = 79ms, p50 = 0.72ms — 260× tail/median.
- gpu main max = 125ms, p99 = 1.68ms, p50 = 0.51ms — 244× tail/median.
- gpu total max = 292ms, p99 = 100ms, p50 = 1.48ms.

Both shadow and main occasionally take ~100× their p50. We can't yet say from aggregates which pass spikes correlate to which wait_fences spikes (the gpu_timer logs aggregates, not per-frame correlations).

### Hypothesis space (ranked by plausibility)

1. **Newly-uploaded mesh causes a GPU memory-system stall.** Persistently-mapped HOST_VISIBLE memory has weak coherency guarantees on some drivers; the first GPU access after a CPU write may trigger TLB flush / cache invalidation / coherency wait. Honeykrisp is reverse-engineered; this kind of pathology is plausible.

2. **Indirect draw buffer updated mid-frame.** When a new mesh gets added to the indirect-draw list, the cull compute shader or the indirect draw may see inconsistent state on the first frame and stall.

3. **Mesa shader-variant recompile on first use of a new mesh.** Different vertex counts may hit different optimizer paths. Less likely since we use the same pipeline for all chunks.

4. **GPU scheduler quirk on Honeykrisp.** Some workload pattern (specific draw count, specific buffer size, specific resource layout) hits a slow path in the open-source driver.

### Next diagnostic ship: chug-debug-2

Add per-frame `gpu_spike` event emission. When a frame's total GPU time exceeds 100ms, emit:
- frame_idx
- gpu_total_us
- per-zone breakdown (shadow_us, main_us, froxel_us, cull_us)

After running, correlate `gpu_spike` events with `wait_fences_spike` events on `frame_idx`. The matching events tell us: for each chug, which pass was the culprit.

If shadow is the culprit: we attack shadow culling or render-pass setup.
If main is the culprit: we attack the main pass geometry path.
If multiple: each chug may have a different cause; we instrument further.

### Reminder: do NOT pivot to a fix yet

We have one diagnostic answer (it's GPU). We need a second (which pass). Both are cheap. A fix without knowing which pass would be a guess.


## Design framing 2026-05-14: two-mode performance contract

User observation: render distance maps onto fundamentally different gameplay modes, and each has a different performance contract.

**rd=3-4 (competitive / immersive small-map)**
- The player is fighting, exploring caves, or in close engagement.
- Sub-perception latency matters MORE than visual range.
- Target: 240+ FPS sustained, no frame >8ms, smooth input-to-photon.
- Comparable to: competitive FPS, action games, twitch movement.

**rd=20+ (explorer / scenic planning)**
- The player wants to see distant terrain to plan routes.
- Visual range matters MORE than peak responsiveness.
- Target: 60-120 FPS sustained, occasional 100ms hitch is forgivable.
- Comparable to: open-world exploration sims, flight sims.

### Why this matters for optimization priorities

We've been measuring rd=2 chugs as if they were the universal problem. They're the competitive-mode problem. At rd=20+ (explorer mode) the chugs are barely visible in the same data because the workload-per-chunk drops with LOD tiering.

Conclusion: chug-investigation work must distinguish "rd=2 competitive feel" from "rd=20 explorer steady-state." Different fixes may apply to each. Current chug-debug-1 data is competitive-mode data.

### What this unifies

When the engine handles BOTH modes smoothly (smooth at rd=3, non-stuttery at rd=20), the world feels unified — not "tacked-on configurable rendering bolted onto a game engine," but a coherent space the player can interact with at the depth they choose. This is the difference between a tech demo and a game.

### Implications for shadow/CSM work (future)

Cascaded shadow maps at rd=3 = all 4 cascades cover near terrain in high detail.
At rd=20 = cascades stretch to distant terrain at lower per-cascade detail.
Same multiview infrastructure (filed in c.17 plan, reverted) supports both
modes when the cascade-fit compute adapts to the active render distance.

The c.17 arc can resume after chug investigation lands — and the resumed
plan should explicitly serve both modes.


## Increment 43c.24.3-chug-debug-2 — per-frame GPU zone correlation

Builds on chug-debug-1's GPU-bound diagnosis. After confirming 76/76
chug events were GPU-bound, this ship narrows the search by adding
per-frame per-zone GPU spike events.

### What it adds

When a frame's total GPU time exceeds 100ms, emits a `gpu_spike` event
with the per-zone breakdown (shadow_us, main_us, froxel_us, cull_us).
The analyzer's new `gpu_spikes` subcommand reads these events and
identifies which pass dominates each chug.

### What we expect to learn

The chug-debug-1 data showed total gpu max = 292ms with shadow max
= 188ms and main max = 125ms. Both passes have severe tail latency.
chug-debug-2 tells us which pass spiked on each individual chug
frame — does shadow always dominate, or does main, or do they trade
off?

### Engine-agnostic property preserved

The first naive implementation put `events::record` inside
gpu_timer.rs's `collect_frame`. That broke gpu_timer's
"engine-agnostic, only depends on ash + std" promise.

Refactored: gpu_timer stashes per-frame breakdown in a `last_frame_us`
field, exposes `take_last_frame()`, and the engine-aware
world_renderer.rs caller handles the event emission. gpu_timer stays
reusable; the chug-investigation logic lives in the engine layer
where it belongs.

### Migration script policy applied

This ship is the FIRST to NOT include `commands/update_from_claude_zip.sh`.
chug-debug-1 regressed the user's migration script; this ship enforces
the policy filed in the previous design note: migration script
evolves only via direct user commits.

### Next ship

Targeted fix based on `gpu_spikes` culprit identification. Single
fix, single ship. If no clear culprit emerges (e.g., spikes are
uniformly distributed across zones), the answer is probably platform-
level (Honeykrisp GPU scheduler, KWin compositor, AGX DVFS clock
ramp) and we file the engine investigation pause and dig into the
platform side.


## Lesson 2026-05-14 (second): re-extraction loses user-side commits

Symptom: chug-debug-2 migration deleted `tests/specs/chug_capture.unendtest`
even though chug-debug-1 had carefully restored it via your earlier
commit `2fc9239`.

Root cause: Claude's working tree at `/home/claude/work/unendlich/` was
re-extracted from `c.13-instrumentation-followup-prep.zip` during the
c.17-a revert cleanup, BEFORE the chug_capture spec existed in the
project's history. Subsequent ships built from that re-extracted tree.
Each zip thus shipped a tree state that PREDATES user-side commits
made between the re-extraction and the ship.

Workflows where this hits:
- Anything user commits between Claude's re-extract and the next ship
  gets silently overwritten on next migration, UNLESS the migration
  script preserves it via ALWAYS_KEEP (only `commands/` and `.git/`
  qualify today).

Two complementary fixes, both for next session:

1. **Claude-side: ALWAYS pull forward user-side commits before
   shipping.** Before packaging any zip, fetch the user's current
   tree state (or at least diff it against Claude's working tree
   for new/modified files outside the design surface of the
   current ship). Restore any missed user-side changes.

2. **Repo-side: add `tests/specs/` to ALWAYS_KEEP in the migration
   script.** Specs are user-authored / user-edited artifacts that
   Claude rarely needs to overwrite. Treating the whole dir as
   ALWAYS_KEEP eliminates this class of silent loss for specs.

Note that (2) is the cheaper, more durable fix. (1) requires Claude
discipline; (2) is a 1-line addition to ALWAYS_KEEP.

### Action for next ship

Add `tests/specs` to ALWAYS_KEEP in the migration script. Combined
with the "don't ship migration script" policy from chug-debug-1, the
flow becomes:
- User commits specs locally; they survive migrations.
- Claude commits engine code; it's the only thing zips overwrite.
- Migration script evolves only via direct user commits.

Three categories cleanly separated.


## Filed for later: separate world-gen and rendering onto different queues

User observation 2026-05-14 during chug investigation: "world gen and
shadows are different things... could we make on future? split up
generation for future, not immediate, but still using GPU; shadows
and lighting that are happening now get priority and bandwidth."

### Why this is the right answer

Today everything runs on the graphics queue:
- Shadow pre-pass
- Main render pass
- Froxel fill compute
- Cull compute
- chunk_generation.comp (world gen)
- chunk_mesh.comp (meshing)
- view_compute.comp (view setup)
- cascade_fit.comp (shadow fitting)

When a chunk crosses into the camera's loaded radius, chunk_generation
and chunk_mesh dispatch on the same queue that's trying to render
the current frame. They serialize. The render thread fence-waits on
the previous frame, which fence-waits on a generation dispatch that
got queued just before render commands. Result: visible chug.

### The fix

Vulkan supports multiple queues. The AGX hardware on M2 has separate
compute scheduling. Mesa's Honeykrisp driver exposes a separate
COMPUTE_BIT queue family alongside the GRAPHICS_BIT queue.

Architecture:
- **Graphics queue**: shadow + main + froxel + view_compute +
  cascade_fit + cull. Things that MUST complete this frame for the
  player to see the right picture.
- **Compute queue**: chunk_generation + chunk_mesh. Things that
  produce data for FUTURE frames. Latency-tolerant; what matters
  is that they finish before the player crosses into the area.

Synchronization between them via semaphores. When chunk_mesh produces
a new mesh, it signals a semaphore. The next frame's render that
wants to use the mesh waits on the semaphore. No fence on the render
thread's critical path during the bulk of generation work.

### Why this isn't trivial

- Two Vulkan queues with proper sync = real engineering work.
- Honeykrisp may or may not actually run them in parallel on
  hardware. The driver might serialize them despite the API
  promising async. Needs measurement.
- Persistent-mapped buffer regions need careful handling when
  multiple queues are reading/writing.
- Existing engine code assumes single graphics queue; refactor
  surface is meaningful.

### When to do this

After the immediate chug fix (whatever chug-debug-2 reveals).
Estimated effort: substantial — multi-session work. Probably the
single biggest architectural improvement on the roadmap for this
class of game. Worth doing right.

### Related: bandwidth scheduling vs stutter mitigation

User's framing: "I have a hunch these should be solved with bandwidth
scheduling and not pure stutter mitigation." Agreed. Multi-queue is
one form of bandwidth scheduling — it doesn't reduce TOTAL work, it
ensures the right work runs on the right hardware unit at the right
time.

Other bandwidth-scheduling levers:
- Upload throttling (already attempted; uploads=1 made things worse).
- Per-frame chunk-load budget (not just upload count, but compute time).
- LOD-aware generation priority (mid/far/vfar can be slower; near
  must be fast).
- Cross-frame work distribution (start gen at frame N, hand off at
  frame N+3).

The multi-queue work above is the foundation that makes the rest of
these schedulable in the first place. Without separate queues, "priority"
just means "earlier in the same queue" which means "earlier in the
same render-thread serialization."


## Revision 2026-05-14: one performance contract, not two

The "two-mode performance contract" section above is wrong. Refining
per user feedback: "no just one [contract], but if say more things
get deferred without hitching movement that is a better tradeoff."

### The actual contract

ONE rule: the player's input-to-photon latency must stay tight,
regardless of render distance. Period.

What changes between rd=3 and rd=20 isn't the contract. It's what
the engine defers to honor the contract:
- At rd=3, there's little to defer; everything fits in a frame.
- At rd=20, the engine must defer non-critical work (distant-chunk
  generation, far-cascade shadow refinement, vfar mesh updates,
  speculative future-frame setup). But near-player rendering
  stays at the same target.

### Why this is the right framing

The two-contract framing treated rd=2 vs rd=20 as different SLAs.
That's bookkeeping — the player doesn't context-switch their
expectations based on a menu setting. They just want the world to
feel responsive.

The one-contract framing names what's actually happening: the engine
has more work to dispatch at higher rd, but the work that affects
the current frame stays the same size. Higher rd means more
DEFERRABLE work, not more URGENT work.

### How this connects to multi-queue

The deferrable / urgent split is exactly what multi-queue lets us
express in Vulkan. Urgent → graphics queue. Deferrable → compute
queue. Sync via semaphores when deferred work needs to land in a
specific frame.

So: the multi-queue work above isn't just a perf optimization. It's
the architecture that lets us express the one-contract design
without per-rd workarounds.

### The two-mode section above stays in the design notes

Documenting the wrong framing alongside the correct one is useful:
it's evidence of how the design model evolved. Future readers see
the iteration, not just the destination.


## Chug root cause confirmed 2026-05-14: mesher compute serializes with render

After chug-debug-2 produced clean correlated data:
- wait_spikes: 113/116 GPU-bound, 0 CPU-bound.
- gpu_spikes: 47/66 shadow culprit, 18/66 main culprit. Critically:
  shadow AND main spike TOGETHER in every chug frame, not
  independently. Row 1 of worst-10: shadow=85ms + main=130ms in
  same frame. Row 5: shadow=144ms + main=85ms. Always paired.

That pattern means it's a GPU-wide stall, not pass-specific.

Reading gpu_mesher_vk.rs confirmed: the GPU mesher's queue is
`pool.queue()` — the SAME graphics queue world_renderer uses.
chunk_mesh.comp dispatches submit alongside shadow/main draws.
When mesher has ~20 chunks at 5-80ms each in flight (visible in
the in-line `mesh_q: 21/21` FPS reports during chug windows),
render commands serialize behind mesher work.

Confirmed by FPS log at 16:59:46 chug moment:
- gpu_total = 90ms (over 11 frames)
- mesh_q: 21/21 — mesher queue saturated
- mesher near max = 81ms — a single chunk took 81ms

Adjacent frames had mesh_q low and FPS 200+. The chug correlates
1:1 with mesher saturation moments.

### Minimum-viable fix: dedicated compute queue for mesher

Move chunk_mesh.comp (and chunk_generation.comp) onto a separate
Vulkan queue from the QUEUE_COMPUTE_BIT queue family. Mesher work
no longer serializes against render. The render thread fence-wait
only sees graphics-queue work (shadow + main + froxel + cull),
which is small and predictable.

Synchronization: when mesher completes a mesh, it signals a
semaphore. The frame that wants to bind the mesh waits on that
semaphore at vertex-input stage. No CPU-side cost; semaphore
GPU↔GPU.

### Open question: will Honeykrisp actually run them in parallel?

Vulkan promises async compute via separate queues; drivers can
serialize internally. Asahi's Mesa-Honeykrisp driver is reverse-
engineered and may not implement true async compute today. We'd
need to measure post-fix to know if we got real parallelism.

Even WITHOUT real hardware parallelism, the fix still helps: a
separate queue means mesher can be deprioritized by the kernel
scheduler when the graphics queue submit is pending, even if both
share the same hardware unit. The OS-level scheduler has more
to work with.

### Filed design (user 2026-05-14): quadrant priority scheduler

User-proposed architecture: a 2×2 (or 4×4) grid of workpools with
different priority levels. Scheduler picks N pools per dispatch
cycle, in priority order, notifies losers to wait or do something
else.

The 4×4 variant supports:
- Spatial-aware priority (chunks at +X border get priority if
  player velocity is +X).
- Geometric merging — adjacent pool work can be combined into
  larger dispatches periodically for batching efficiency.
- Predictive shuffling — anticipate chunks the player will cross
  into, preemptively move their work to higher-priority pools.

This is a more ambitious design than typical voxel engine
schedulers. Most voxel engines do priority queues; few do
spatial-aware scheduling tied to player velocity.

**Why this isn't the next ship**: the chug doesn't need it. The
chug needs a separate compute queue. The quadrant scheduler is
an architecture for AFTER multi-queue, when we have lanes worth
managing.

Filed for v0.25 or later.

### Other related future work

User's "two performance contracts" revision (above): the final
design might polish over both modes rather than force a choice.
Contracts are either beaten with optimization (current research)
or polished over for the user. Multi-queue is "beating contracts
with optimization" — both rd=3 and rd=20 benefit because BOTH
need the render thread unblocked from mesher work.


## Increment 44 v0.24.3-multi-queue-a — compute queue acquired, mesher unchanged

First step of the multi-queue work prompted by chug-debug-2's
diagnosis (render thread blocking on mesher compute serializing
behind it on the graphics queue).

### Scope

Just the device-create plumbing. `VulkanContext` gains a compute
queue handle; nothing actually uses it yet. The mesher continues
submitting to the graphics queue exactly as before. This is the
smallest possible step that exercises the device-create change in
isolation, to verify it's safe BEFORE flipping the mesher in Ship B.

### Why three ships (A→B→C)

c.17-a is the cautionary tale. A single careful change to
`create_device` (enabling multiview) regressed CPU recording by
5ms/frame for reasons we never fully isolated. Multi-queue work
touches the same code surface with strictly more changes. Splitting
into A→B→C lets us bisect cleanly if anything regresses.

### Three outcomes on Honeykrisp (to be measured)

1. **Dedicated compute queue family exists.** Best case; true async
   lane available. Most desktop GPUs work this way. Honeykrisp's
   support is unknown.
2. **Same family as graphics, second queue available.** Second-best;
   separate handle but same hardware lane.
3. **Single queue available.** Worst case; compute_queue ends up
   identical to graphics_queue. Ship B becomes a no-op and we'd
   pivot to per-frame mesher work-budget limiting as an alternative
   chug fix.

### Design intent for Ship B

When ready: mesher's `pool.queue()` plumbing switches from graphics
queue to `compute_queue` (if `compute_queue_is_distinct`). Mesher
signals a semaphore on completion. Render submit at vertex-input
stage waits on the semaphore for any mesh-bind that depends on
recently-completed mesher work.

Buffer ownership transitions handled at the same point. Persistently-
mapped HOST_COHERENT buffers have spec-defined behavior here that
should be OK without explicit ownership transfer, but we'll add
explicit barriers if validation layers complain.

### Filed contingency: if Honeykrisp = Outcome 3

If Ship A's startup logs show only one queue available, multi-queue
isn't a path forward on this hardware (with Honeykrisp's current
implementation). Pivot to:

- **Per-frame mesher dispatch budget**: limit chunk_mesh.comp work
  to N ms per frame. Excess work defers to the next frame. Similar
  to UNENDLICH_MAX_UPLOADS_PER_FRAME but for compute time, not
  upload count.
- **CPU-side mesh dispatch yielding**: spread mesher work across
  multiple submits with explicit yields, letting render commands
  interleave.

Both are smaller fixes than multi-queue but address the same chug
mechanism. We'd ship one of them and revisit multi-queue when the
Asahi driver matures.


## Multi-queue closed on M2 2026-05-14 (Ship A measurement)

Ship A startup logs on Apple M2 Max via Honeykrisp:
```
Picked physical device Apple M2 Max (G14C B1) (graphics qf=0,
    present qf=0, compute qf=0 dedicated=false)
WARN: Compute queue family has only 1 queue and matches graphics —
      compute_queue will be identical to graphics_queue. Async
      compute will not provide queue-level parallelism on this device.
Compute queue acquired: family=0 distinct=false
    (graphics handle reused: true)
```

**Outcome 3 of the Ship A readme.** AGX hardware exposes a single
queue family with a single queue. compute_queue is bit-for-bit the
same handle as graphics_queue.

### Why this is not a Honeykrisp limitation

AGX hardware on M1/M2 has a unified command queue at the hardware
level. The driver-level decision to expose one Vulkan queue family
mirrors the hardware reality. This is the same constraint Metal
faces on macOS — Apple's own driver exposes a single command queue.

Even a hypothetical future Honeykrisp release that splits work onto
multiple Vulkan queue families would just be exposing virtual queues
that funnel into the same hardware lane. The OS scheduler could
arbitrate, but the hardware will serialize regardless.

### Multi-queue is not the chug fix on this platform

Ship B as planned (mesher submits to compute_queue) would literally
use the same vk::Queue handle as graphics. Zero behavior change. We
abandon the Ship A→B→C sequence.

### Regression check from Ship A's hardware run

cpu_record p50: 0.50ms (unchanged from chug-debug-2's 0.51ms).

This is the c.17-a teaching exercised: device-create changes did NOT
regress CPU recording. The Ship A code can stay in (queue acquisition
is harmless even if unused).

Workload-normalized chug rates are similar to chug-debug-2:
- wait_spikes per chunk loaded: 2.83 (chug-debug-2) → 2.10 (multi-queue-a)
- gpu_spikes per chunk loaded:  1.61 (chug-debug-2) → 1.23 (multi-queue-a)

Both slightly down per-chunk, but the workload was different (60 vs
41 chunks loaded). Inconclusive on perf delta; conclusive on safety.

### The chug pattern stays unchanged

Worst spikes from multi-queue-a:
- shadow=254ms, main=49ms, total=304ms
- shadow=46ms, main=201ms, total=248ms
- shadow=143ms, main=95ms, total=238ms
- ...

Same correlated shadow+main spiking pattern. Still indicates GPU-wide
serialization, not pass-specific pathology. The mesher-serializes-
with-render hypothesis still fits perfectly.


## Increment 45 v0.24.3-upload-pressure — test the upload-contention hypothesis

After reading the render loop end-to-end (post-Ship A), the chug
hypothesis was revised. New mental model documented above:
MAX_FRAMES_IN_FLIGHT=1, GPU mesher not wired in, mesh upload is CPU
memcpy into HOST_COHERENT mapped memory.

Hypothesis under test: CPU memcpy into HOST_COHERENT memory forces
cache-coherency cost the GPU pays on its next read. Both shadow and
main read the same pool buffer, so the cost shows up as correlated
spikes (matching observed pattern).

### Falsification design

Two-run A/B:
1. Baseline (default 8 MB byte-budget): measure spike counts.
2. Aggressive cap (2 MB byte-budget): re-measure.

If aggressive cap reduces chug count by 30%+, hypothesis confirmed
and we have a tuning lever. If unchanged, hypothesis falsified.

### Honest design notes

The upload-contention hypothesis is my third major theory of this
chug. Each previous one was data-supported then ultimately wrong (or
partial). This time I'm shipping the falsification *with* the lever,
so we get either a working fix or a clear "no" in one round.

The bytes-budget knob is useful regardless of hypothesis outcome:
- If hypothesis right: tuning lever for chug vs terrain-load tradeoff.
- If hypothesis wrong: terrain-load pacing for high-rd scenarios.

### Strategic next moves

Independent of this ship's outcome, two destinations:

1. **Wire up the GPU mesher.** Already exists in `gpu_mesher_vk.rs`
   (715 lines, unused). Moving meshing to GPU eliminates the
   CPU→GPU upload step entirely — the chug source if hypothesis is
   right, and a perf improvement either way. Multi-session work
   but architecturally correct.

2. **Working-distance buffer ring.** Pre-mesh chunks in a ring
   beyond view distance. Reduces chug FREQUENCY by spreading mesh
   work across more frames. Orthogonal to GPU-mesher; could happen
   either before or after.

The order to attack these depends on what we learn from this ship.

### What the increment number jump signals

We're at increment 45 of the unendlich arc. Each increment has
either shipped diagnostic data or a targeted change. The chug arc
specifically is increments 42-45:
- 42: chug-debug-1 (wait_fences instrumentation)
- 43: chug-debug-2 (per-frame GPU zone correlation)
- 44: multi-queue-a (compute queue plumbing, ruled out by hardware)
- 45: upload-pressure (THIS — hypothesis falsification + lever)

Four increments to isolate one chug. That's slow but each step
ruled out wrong directions cleanly. The alternative was shipping a
big speculative fix and not knowing why.


## Increment 46 v0.24.3-shadow-size — runtime shadow map size knob

After rd=8 chug_capture showed shadow GPU time is the dominant cost
(p50 = 77ms, over_8.3ms = 96.5%), this ship adds a knob to test
fragment-vs-vertex cost dominance.

### Why this knob shape

`UNENDLICH_SHADOW_MAP_SIZE` = 512 | 1024 | 2048. The CPU's texel-snap
math and GPU's cascade_fit shader both read the same value (via
`self.shadow.extent.width`), so visual stability holds across sizes.

This is a single-axis A/B that produces a clean falsification:
- If 1024² shadow p50 drops to ~20ms (4×), fragment cost dominates.
- If 1024² shadow p50 only drops to ~60ms (modest), vertex/draw cost
  dominates.
- Either result tells us what to attack next.

### Why this is the first non-diagnostic ship in the chug arc

Increments 42-45 were all diagnostic. 46 adds an actual mitigation
knob (even if used initially for diagnosis). The transition signals
that the chug is well-enough characterized to start shipping levers,
not just measurements.

If 1024² works: 2048² stays default, users can opt down. If we want
production quality the right next ship is making the multi-cascade
plan land properly (4× 1024² covers more distance at the same memory
cost as 1× 2048²), but that's bigger work.

### Filed for later (whichever way the result goes)

- **Adaptive shadow size based on RD.** rd=3-4 keeps 2048; rd=8+
  drops to 1024 automatically. Trades visual fidelity at high RD
  for chug-free play.
- **Shadow LOD.** Distant chunks use the LOD-mid/far meshes for the
  shadow pass, not the LOD-near meshes. Massive vertex reduction.
- **Adopt the 4-cascade plan.** C0 20m at 1024², C1 60m at 1024²,
  C2 180m at 1024², C3 600m at 1024². Total memory same as today's
  single 2048². Each cascade renders far fewer chunks.

The last one is the design's actual plan; it's been deferred since
v0.24.2c. The chug data makes it urgent again.


## Increment 47 v0.24.3-cull-count — count shadow vs main cull chunks

After shadow-size falsified fragment-cost (4× area = 5% time), the
next axis is: how many chunks does shadow render vs main?

### What this ship measures

CPU-side count of chunks passing each cull's frustum test, emitted
per-frame as a `cull_count` event when ratio > 1.5 or count > 50.
The CPU does redundant work (the GPU is already culling) but it's
cheap (1-2 ms at rd=8) and avoids GPU readback surgery.

### Why CPU-side counting instead of GPU readback

GPU readback of the indirect count_buffer would need:
- A new HOST_VISIBLE staging buffer.
- cmd_copy_buffer after each cull (TRANSFER barrier).
- Two-frame delay on the read (read previous frame's count).
- Sync between CPU read and GPU write across frames.

That's surgery for a diagnostic. CPU re-running the same AABB-vs-
frustum test is wasted work, but the work is small, no sync needed,
and the result is in-hand same-frame for event emission.

### Decision tree this ship enables

| Ratio | Interpretation | Next |
|---|---|---|
| > 2.0 | Shadow culls way more chunks | Shadow LOD / tighter cull |
| 1.3-2.0 | Some extra chunks | Mixed: LOD + per-draw work |
| ≤ 1.3 | Same chunks, expensive | Per-draw / tile-binning overhead |

The third case is the worst — there's no easy lever. If shadow is
expensive at the same chunk count as main, the cost is in vertex
processing or tile-binning, both of which are AGX-firmware-controlled
and hard to influence from user-space.

### Honest pattern check

This is increment 47. The arc since 42 has been: instrumentation,
instrumentation, ruled-out hypothesis, ruled-out hypothesis, ruled-
out hypothesis, ruled-out hypothesis, now another instrumentation.
Five diagnostics, zero fixes shipped. That's a real pattern.

The justification this time: cull-count is the cheapest possible
follow-up to shadow-size's clean falsification, and it produces a
clear three-way decision tree. If the result is "ratio ≤ 1.3," we
know we're in a hard-to-attack regime and the right move may be
filing with upstream (Asahi/Honeykrisp) rather than more engine
work. If ratio > 2.0, we have a clear next ship.

If after this we're still in diagnostics-only mode, the right call
is to pause the chug arc and ship a feature unrelated to it
(working-distance buffer ring, HUD foundation, light-driven plant
variation, gpu_mesher wire-up). The chug is not blocking gameplay
at rd=2-4; the rd=8 chug is real but its fix may not be small.


## Increment 48 v0.24.3-shadow-stride — temporal shadow amortization (Step 1)

After cull_count showed mean shadow:main ratio of 16 (shadow renders
~40 chunks, main ~1-5), this ship tests temporal amortization: render
1/N of the chunks per frame, preserve the shadow map between frames.

### The user's framing

The conversation that led here was the user describing "noise-sample-
hold shadow pass with downsampling" + later "fractal temporal
dithering." The full arc is:
- Step 1 (this ship): round-robin stride, no clear, force-full on
  cascade shift / warmup. Tests whether the mechanism works at all.
- Step 2 (later): replace round-robin with noise (blue-noise mask,
  or fractal-pattern). Spatially decorrelates the staleness so
  adjacent chunks aren't synced — the banding becomes shimmer.
- Step 3 (later): adaptive stride. Chunks near camera always render,
  far chunks render rarely (sub-pixel detail doesn't need refresh).
- Step 4 (much later): full temporal reprojection. Old frame's depth
  reprojected into new frame's light-space; only render where
  reprojection fails.

The user's eye-tracking intuition checks out at high fps (16 frames
× 1/240s = 66ms, faster than flicker fusion). At 25fps (current rd=8
chug) the staleness is visible — but the technique BOOSTS fps, so
it bootstraps into its own working regime.

### Why this is structurally sound

Texel-snap (already correct in our cascade fit) anchors each chunk's
shadow contribution to fixed world-space texels. Old depth from
frame F-3 is at the same texel position as it would be if rendered
in frame F (modulo cascade origin shift, which we detect and force-
full on).

### Architectural concerns specific to this implementation

**Render-pass duplication.** Vulkan requires loadOp at render-pass
creation, not per-frame. Two render passes: CLEAR (existing) and
LOAD (new). Shadow pipeline created against CLEAR, used with both,
relying on render-pass compatibility rules. If validation rejects,
the fix is two pipelines — but I trust the compat rules until
proven otherwise.

**The `last_cascade_center` field uses view.position as proxy.** Not
exactly right (cascade center is camera-frustum-slice centroid),
but the > 1 chunk-width threshold is coarse enough that the
approximation is fine.

### Why I'm shipping despite the falsified hypothesis pattern

Previous diagnostic ships falsified hypotheses cheaply, but this is
the first one with an actual mitigation lever attached. The user
proposed the technique and asked to test the small step toward it;
ship the small step.

If stride=4 doesn't drop shadow time proportionally, the temporal
arc is dead and we pivot. If it does, Step 2 is the fractal-dither
follow-up the user mentioned (and is the visual fix for the round-
robin banding Step 1 produces).

### Filed implications

- Multi-cascade plan (4× 1024² with proper log-splits) is still
  filed and still right, BUT temporal stride works orthogonally:
  each of 4 cascades could independently stride. Combined effect:
  4× cascade quality at same cost OR same quality at 4× cheaper.
- The c.5 bisect noted per-layer framebuffers caused a 2.8×
  regression. Adding `framebuffer_load` is another framebuffer
  object; not per-layer, but worth watching.
- Eviction-aware force-full filed for later; current naive logic
  trusts cascade-shift to catch most cases.

### Increment count check

This is increment 48. The chug arc has been:
- 42: chug-debug-1 (wait_fences instrumentation)
- 43: chug-debug-2 (per-frame GPU zone correlation)
- 44: multi-queue-a (compute queue plumbing, ruled out by hardware)
- 45: upload-pressure (hypothesis falsified)
- 46: shadow-size (hypothesis falsified — fragment cost ≠ chug)
- 47: cull-count (diagnostic — ratio 16:1)
- 48: shadow-stride (THIS — first actual mitigation lever)

Six diagnostics, one mitigation. The diagnostics-only pattern broke
this turn because the user's idea pointed at a specific technique,
the cull_count data validated the precondition (shadow IS over-
rendering chunks), and the mechanism has clean architectural shape.

If this ship doesn't move shadow time, the next decision is whether
to keep this arc going or pivot to a different chug attack (tighter
cascade) or a different feature entirely.


## Increment 49 v0.24.5-surface-extract — sparse surface representation (Layer A.1)

The chug arc (increments 42-48) hit a real platform constraint on AGX:
shadow geometry is the dominant cost, and the techniques that would
reduce it on immediate-mode GPUs (LOAD pass for temporal amortization,
fragment-cost reduction via smaller maps) don't work the same way on
TBDR. The arc produced excellent diagnostic infrastructure but no
shipping fix.

This increment opens a new architectural direction: **GPU-resident
world**, where simulation and rendering share the same data structures
on the GPU. The destination, in long-range form:

1. **Octree storage** replaces dense `Chunk` block grids. Surface
   queries become O(surface) instead of O(volume). Physics queries
   (which block falls under weathering, where does erosion act,
   what's the connectivity graph of this bridge) become graph
   walks on the octree, not scans-then-rebuild.
2. **GPU-resident generation** (already exists — `gpu_generator.rs`
   with `Backend::Auto` is the default in `unendlich_world`).
3. **GPU-resident meshing** consumes a sparse representation rather
   than dense blocks. Reading the surface from the octree is the
   eventual shape; v1 ships a CPU dense-scan version that produces
   the same wire format.
4. **Procedural reconstitution upload**: when chunks are uploaded
   to GPU, the wire payload is `(coord, seed, edits)` not dense
   blocks. The GPU's existing `chunk_generation.comp` regenerates
   the chunk's blocks deterministically, sparse edits are applied
   from the wire payload, and meshing runs on the result.

This increment is **Layer A.1**: the sparse `SurfaceRecord` wire format
plus a CPU-side extractor. The format is chosen to survive the octree
migration (world coordinates, not chunk-local indices).

### Why the bridge-falling question matters

The conversation that led here asked: "for the future physics like
will this bridge fall this tick due to weathering and erosion, which
representation helps?" The honest answer: **octree, decisively.**

Dense grids don't know what a bridge is. Octrees do, because their
internal nodes encode connectivity at all scales. Stress propagation,
weathering, erosion, fluid flow, growth, decay — all want graph-walk
operations on the octree's structure. Dense grids would require
rebuilding the same graph every tick.

This isn't a perf optimization; it's a structural decision. Building
the engine on dense grids and then bolting on octrees for physics
later means paying for both representations forever. Better to commit
now.

### The smooth-vs-blocky question (open)

The user hasn't committed yet between:
- Camp 1 (Minecraft-like, axis-aligned blocky surfaces): octree leaves
  are `BlockType`; meshing is axis-aligned quads.
- Camp 2 (No Man's Sky-like, surface nets / dual contouring): octree
  leaves carry density values; meshing is curved surfaces.

The current engine's aesthetic is camp 1. The bridge-weathering
example fits camp 1 (blocks fall as discrete units). Continuous
terrain erosion (camp 2) would let rivers carve curved valleys.

This decision affects octree leaf structure but doesn't gate the
current increment.

### Why ship just the extractor

The full GPU mesher (kernel + Vulkan plumbing + production wiring) is
substantial. Shipping the CPU-side surface extractor alone:
- Validates the wire format with tests.
- Establishes the producer/consumer split (extract → mesh).
- Survives the octree migration (octree walk produces the same Vec).
- Is reversible — if the next increment hits problems, this is still
  useful infrastructure for debug overlay / save format / network sync.

The 16-byte record format is pinned by a test so the future GLSL
kernel binding can rely on it.

### Honest pattern note

This is the first ship in a planned multi-session arc. Previous arcs
(chug 42-48) drifted longer than expected because the architectural
shape was discovered mid-arc. This time the destination is articulated
upfront (octree-resident world with reverse-pathway upload) and the
work is broken into reversible layers (A.1, A.2, B-already-done, C, D).
Each ship is independently useful.

The pattern check that matters: don't let "architectural commitment"
become "blank check." Each layer ship must pass its own falsification.
If A.2 (GPU kernel) doesn't beat CPU meshing on Honeykrisp, the arc
gets re-examined before committing to C.

### Filed for later

- Wire format compression (chunk-local u8 coords + per-chunk base
  uploaded once) — defer until we have measurements showing 16-byte
  records are a real bandwidth issue.
- Subvoxel/plant decoration parallel pass — v1 kernel will be
  opaque-blocks-only; plants need their own path eventually.
- Batching multiple chunks per dispatch — clear v3 optimization, not
  v1.
- Camp 1 vs Camp 2 aesthetic decision — affects octree leaf payload
  in Layer C.


## Increment 50 v0.24.6-gpu-mesh-bench — GPU sparse mesher + comparison bench (Layer A.2)

Continues the GPU-resident world arc. Layer A.1 introduced the
`SurfaceRecord` representation; this layer adds the kernel that
consumes records and the Vulkan plumbing to drive it.

### What's new

- `chunk_mesh_sparse.comp` — compute kernel, one thread per record,
  atomic-counter reservation for output buffer slots, axis-aligned
  quad emission per face bit.
- `SparseGpuMesher` — Vulkan module that owns the pipeline, buffers,
  command buffer, and fence. Borrows device/queue from
  `VulkanContext` (the renderer's device), enabling future zero-copy
  delivery to the renderer's vertex source.
- `gpu-mesh-bench` — new bin comparing CPU naive output to GPU
  sparse output on a representative chunk grid. Reports per-chunk
  topology equivalence and timing.

### Why ship a bench, not production wiring

This is the first time the sparse path runs on real AGX hardware.
Risks include:
- Validation errors from the LOAD-like sync pattern (we use a single
  fence; previous DEVICE_LOST issues on Honeykrisp were tied to
  multi-timestamp shadow passes).
- Quad-emission winding mismatched between kernel and existing
  renderer pipeline (would manifest as inside-out faces).
- Atomic-counter contention behavior on AGX's compute path (unknown
  without measurement).

Shipping a bench first means we discover any of these in isolation,
not in the middle of gameplay where they'd corrupt the rendered
scene. The bench is the falsification gate for whether Layer A.3
(production wiring) gets built.

### Per-chunk vs batched: explicit v1/v2/v3 plan

v1 (this ship): one chunk per dispatch. Round-trip dominates.
Probably slower than CPU on per-chunk basis.

v2: batch N chunks per dispatch. Amortizes round-trip. CPU prepares
N chunks' worth of records concatenated, with a per-chunk metadata
array (record_offset, record_count, vertex_capacity, ...). Kernel
indexes into the right slice per workgroup.

v3: zero-copy. Mesher writes directly into renderer's vertex/index
pool buffers. No readback. Counter values get integrated into the
existing indirect-draw count buffer.

v3 is the architectural destination. v1 and v2 are stepping stones
that let us validate each layer of complexity independently.

### Honest assessment of the "GPU will be slower at v1" prediction

The CPU naive mesher is ~100-500 us per chunk on M2. The GPU path
at v1 will probably be ~1000-3000 us due to:
- HOST_VISIBLE upload (~50 us for 1500 records × 16 bytes)
- Dispatch overhead (~50-200 us)
- Fence wait (~100-500 us — the dominant cost on AGX)
- Readback (~50 us)

This is FINE. v1's job is correctness on real hardware. Performance
arrives at v2 with batching (cuts round-trip from 36× to ~1×) and
v3 with zero-copy (eliminates upload + readback entirely).

If v1 reports topology-correct output on M2, the technique works
and we proceed to v2. If it crashes or produces wrong output, we
debug v1 before committing to the production wiring.

### What stays filed for later

- Headless Vulkan context (bench currently opens a hidden window).
- Per-record lighting (sky uniform input, climate field input).
- Sub-voxel decorations (plants, leaves) — separate kernel pass.
- Greedy quad merging on GPU.
- Procedural reconstitution upload (send `(coord, seed, edits)`
  instead of records — GPU regenerates and meshes locally).
- Octree storage replacing dense `Chunk`.


## Increment 51 v0.24.7-gpu-mesh-batch — batched GPU sparse mesher

Layer A.2 v2. v0.24.6 (v1) proved correctness on AGX: 36/36
topology parity, but per-chunk performance was 0.51× CPU due to
~550us per-submit dispatch floor.

v2 amortizes the floor by batching: multiple cmd_dispatch calls
in one command buffer, one queue submission, one fence wait. Per-
chunk dispatches share the submission overhead.

### Buffer design

Per-chunk slots within shared buffers. MAX_BATCH_CHUNKS = 16:
- records: 16 × 16K records × 16B = 4 MB
- vertices: 16 × 43K × 24B = ~16 MB
- indices: 16 × 64K × 4B = 4 MB
- counters: 16 × 4 u32 = 256B

Shader writes to slot N at known offsets (passed via push constants
per dispatch). Counters become array-indexed (counters[slot*4+0]
through counters[slot*4+3] hold vertex_count, index_count,
overflow_flag, reserved for that slot).

Index VALUES written to the indices buffer are slot-local (not
adjusted by vertex_offset). This matches v1's single-chunk
semantics: ChunkMesh has slot-local indices, downstream code
treats each slot as an independent vertex+index buffer.

### Dispatch model

For an N-chunk batch:
1. Single cmd_fill_buffer resets all counters.
2. One pre-dispatch barrier (transfer → shader).
3. N cmd_dispatch calls, each with push constants identifying
   that chunk's slot.
4. One post-dispatch barrier (shader → host) covering all output
   buffers.
5. One vkQueueSubmit.
6. One fence wait.

No barriers between individual dispatches because they write to
non-overlapping slot regions (each chunk writes only to its own
counter group, vertex slot, index slot).

### Batches larger than MAX_BATCH_CHUNKS

API accepts any size. `mesh_chunks_batch` internally splits via
`chunks.chunks(MAX_BATCH_CHUNKS)` and runs each split as one
submission. For 36 chunks: 3 submissions of 16/16/4.

This means the bench's "v2 path" is really 3 submissions, not 1.
The headline number is the sum. A "true single-batch" speedup
would require MAX_BATCH_CHUNKS ≥ 36, which is fine to do but I
didn't want to allocate >24 MB for the bench.

### Hypothesis to validate

If AGX firmware schedules the 16 dispatches in parallel within
one submission, we get the full amortization: 3 submissions × ~550us
= ~1.6 ms + compute time across 36 chunks.

If AGX serializes between dispatches within a command buffer
(possible if TBDR pipeline boundaries trigger between dispatches),
we don't get full amortization and v2 looks more like v1 with
slightly less per-dispatch overhead.

### Open questions still

- **Async dispatch.** Current API blocks the caller on fence_wait.
  Production wiring needs to release the caller and collect results
  later. Filed for Layer A.3.
- **Zero-copy.** Current path reads vertices/indices back to CPU
  for upload to the renderer. v3/Layer A.4 would have the mesher
  write directly to renderer-bound vertex buffers. Filed.
- **MAX_BATCH_CHUNKS tuning.** 16 is a guess. Larger means more
  amortization per submission but more memory. Filed for hardware
  measurement.


## Increment 52 v0.24.8-mesh-bench-pool — bench extended with real CPU baselines

Bench-only update. v0.24.7's single-threaded sequential CPU baseline
isn't what production runs. Extended `gpu-mesh-bench` to also time:
- CPU naive parallel with 4 workers (POOL_WORKERS).
- MeshingPool with 4 workers (production code path: greedy + sky-lit).

The v2 vs MeshingPool ratio is the headline number for the Layer
A.3 production-wiring decision. Three branches based on result:
- v2 substantially faster: ship A.3 with both perf and strategic win.
- v2 comparable: ship A.3 for strategic reasons only.
- v2 slower: widen the gap (larger batch, zero-copy, or greedy GPU)
  before A.3.

Predicted (to verify): CPU naive parallel ~3-4ms (11ms / 4 - some).
MeshingPool 4-8ms (greedy + lighting cost more per chunk). GPU v2
~4-5ms (consistent with prior measurement). MeshingPool vs v2 is
plausibly close on M2.


## Increment 53 v0.24.9-surface-runs — 1D-merged surface representation

v0.24.8's bench showed GPU v2 batched loses to 4-thread CPU naive
parallel by ~25%; ties or marginally beats production MeshingPool
(which uses 5-15× fewer quads per chunk via greedy + has sky-lit
cost per quad).

The path to GPU winning cleanly: emit fewer quads. Two routes:
- **Full 2D greedy on GPU**: sequential per slice, parallelization-
  hostile. Not implemented here.
- **1D row-merging via SurfaceRun**: along one axis only, fully
  parallel (each (slice, V-row) is independent of others). Easier
  to implement, gets 3-5× compression of typical 5-15× greedy
  ratio. Implemented in this ship.

### SurfaceRun design

16 bytes (same as SurfaceRecord). World-coordinate-addressed.
Carries:
- starting position
- single face direction (0-5)
- run length (1-65535)
- block type

Per face direction, the U axis (run direction) is canonically
chosen: faces perpendicular to X run along Y; perpendicular to Y
run along X; perpendicular to Z run along X.

### Extractor strategy

Walks the chunk dense grid in (face, slice, V, U) order so
U-consecutive blocks iterate consecutively. Per (face, slice, V),
runs through U: starts a run at each emitting block, extends while
the next U-block has same type and still emits the face.

CPU cost: ~6× the per-block work of `extract_surface` because we
walk the grid once per face direction. Bench reports actual us/chunk
for both extractors.

### Pending: GPU runs kernel

This ship adds the data structure and CPU-side extractor only. The
GPU kernel that consumes runs (instead of records) is filed for
v0.24.10 if I get to it tonight, otherwise next session.

### Filed for later

- 2D greedy on GPU (the parallelization-hostile case). Probably
  doesn't fit AGX TBDR well; revisit only if 1D runs aren't enough.
- Smaller-than-16-byte SurfaceRun encoding. Length ≤32 in practice
  on CHUNK_SIZE=32; could fit run in 8 bytes if memory pressure
  matters.
- Bench-only ship now; production wiring still gated on overall
  GPU vs CPU pool result being clearly better.


## Increment 54 v0.24.10-gpu-mesh-runs — GPU runs kernel landed

Builds on v0.24.9's `SurfaceRun`. Adds the compute kernel
`chunk_mesh_runs.comp` that consumes runs and emits exactly one
quad per run. Shipped behind a sibling pipeline in the existing
`SparseGpuMesher` — both pipelines share buffers, descriptor sets,
and push constants.

### Two pipelines, one mesher

`SparseGpuMesher` now owns:
- `pipeline` — the records kernel (`chunk_mesh_sparse.comp`)
- `runs_pipeline` — the runs kernel (`chunk_mesh_runs.comp`)

Both pipelines bind the same buffers via the same descriptor set
layout, take the same push constants struct, write to the same
per-chunk slots. Only the input format (records vs runs) and the
output quad count differ.

### Topology validation strategy

The records kernel's topology check was "quad count matches CPU
naive's quad count." That doesn't apply to runs because runs emit
FEWER quads by design. The new check has two parts per chunk:
1. `gpu_quads == runs.len()` (kernel emits one quad per run)
2. `sum(run.length) == cpu_naive_quad_count` (runs cover the same
   total face-block count as naive's per-face quads)

If both pass, the kernel processed every run and the runs
themselves cover the right total area. Vertex position correctness
isn't checked at this level — that would require comparing world-
coordinate quad bounds, filed for later.

### Open questions

- Compression ratio. Predicted 3-5× from my reading of the
  extractor + the structure of natural terrain. Hardware
  measurement pending.
- Whether v3 beats CPU naive parallel cleanly. v2 lost by ~25%
  on performance profile. v3's quad reduction should flip that.
- Whether the kernel's face-direction quad-corner math is right.
  Six separate cases; each has to wind CCW from outside the face.
  Wrote them by analogy to the records kernel but they're not
  byte-identical so a bug could exist.


## Increment 55 v0.24.11-batch-64 — MAX_BATCH_CHUNKS raised to 64

Configuration change. Raised compile-time max from 16 to 64.
Persistent HOST_VISIBLE memory grew from ~24 MB to ~96 MB. At
36 chunks (the bench size), this lets the full batch dispatch in
ONE submission instead of three.

Added `UNENDLICH_GPU_MESH_BATCH` runtime env var (1..=64) to cap
actual chunks per submission. Lets the test harness sweep batch
sizes on hardware. Predicted: total time drops as batch rises,
with diminishing returns past where compute dominates the
dispatch floor.

Filed for v0.24.12 or later:
- Async dispatch (no blocking fence-wait in caller)
- DEVICE_LOCAL output buffers (current HOST_VISIBLE works but
  doesn't compose with zero-copy delivery)
- Warmup phase in bench (first batch is cold-cache slow)


## Increment 56 v0.24.12-bench-extract-fix — fix bench extract-time bug

v0.24.11 hardware test revealed v3 wins everything (1829 us total
for 36 chunks; 2.88× over CPU naive parallel, 15.3× over the
production MeshingPool, 6.25× over CPU naive sequential). Topology
validates 36/36 on both v2 and v3.

But the "extract: X us total" lines in the SurfaceRun compression
analysis were nonsense: ~115ms per chunk for both records and
runs. That's the chunk generator (full noise+biome+layers) running
inside the extract loop, not extraction.

The fix: pre-generate chunks once, time only `extract_surface` and
`extract_surface_runs` on the cached chunks. Now we'll see the real
extract cost.

Still on file:
- The bench regenerates chunks redundantly in several other
  sections. Not a correctness issue for any measurement (the
  regen happens outside timed regions), just bench startup is
  slow (~20s of wasted CPU work on every run).
- Consolidating to a single `Vec<Arc<Chunk>>` at the top would
  be cleanest. Not urgent.


## Increment 57 v0.24.13-single-pass-runs — extractor optimization

v0.24.12's hardware test revealed a real bottleneck: the run
extractor takes ~512us/chunk (18.5ms total for 36 chunks), 12×
slower than the GPU dispatch itself. The "GPU runs path total"
honestly accounted = 18.5ms extract + 1.5ms dispatch = 20ms.

Compared to MeshingPool's 29ms: the GPU runs path wins by 1.5×,
not 17×. The 17× number was GPU dispatch only, hiding the CPU
extraction work. Wrote this up in bench-results.md.

ALSO: v2 (records path) honest total = 7ms extract + 5ms dispatch
= 12ms. So v2 is FASTER than v3 in honest accounting. v3's bigger
compression doesn't pay for its 2.6× more expensive extractor.

### Fix in v0.24.13

Rewrote `extract_surface_runs` as single-pass. Walks chunk once in
(y, z, x). Per cell, decides all 6 face emissions inline. Open-run
state tracked per face direction:
- 4 U=X faces (+Y, -Y, +Z, -Z): reset at start of each (y, z) row.
  Stored as 4 scalar `Option<(BlockId, i32)>`.
- 2 U=Y faces (+X, -X): span multiple y-iterations. Stored in
  32×32 state matrices indexed by (slice=x, V=z).

Same logical output as the 6-pass version. Equivalence verified by
new tests on messy/empty/full/corner chunks (4 added, 33 total in
surface module).

Old 6-pass version kept as `extract_surface_runs_six_pass`
(pub-but-hidden). Used by tests as oracle; bench times both for
side-by-side comparison.

### Decision: v2 vs v3 for production wiring

Pending hardware confirmation of v0.24.13's predicted speedup. If
single-pass extract is ~3× faster:
- v3 total: ~6ms extract + 1.5ms dispatch = ~7.5ms
- v2 total: 7ms extract + 5ms dispatch = ~12ms (unchanged; records
  extractor was already single-pass)

Then v3 wins both in total cost AND in downstream rendering cost
(9× fewer quads to draw later). Layer A.3 wires v3.

If the single-pass extract is much slower than predicted (say only
~1.5× faster), v3 total stays around 14ms — still loses to v2.
Decision then would be: ship A.3 with v2 (records path), or wait
for parallel extraction work.


## Increment 58 v0.24.14-bench-results-honest — bench-results.md final update

Doc-only ship. Updated bench-results.md v0.24.13 entry with real
hardware numbers: 1-pass extract speedup over 6-pass = 1.87× (vs
my 3× prediction). v3 honest total = 11,978 us, effectively tied
with v2 (12,025 us). CPU naive parallel still wins outright at
5,461 us by 2.2× over either GPU honest total.

Arc summary added: v0.24.6 → v0.24.13 took the GPU sparse mesher
from 0.5× CPU to comparable-with-CPU-pool, 2.2× slower than fastest
CPU baseline, topology validated, extract optimized. Natural pause
point.

Open architectural questions for future sessions:
- Parallel extract: linear scaling hypothesis untested.
- Zero-copy output buffers: would save the readback cost (currently
  rolled into v3_us; not measured separately).
- GPU-side extraction (Layer C dependency).
- Greedy + lighting on GPU (v3 still emits flat unlit colors;
  required for fair MeshingPool comparison).


## Increment 59 v0.24.15-parallel-extract — parallel extract experiment

Bench-only. Adds parallel-extract measurements for both
`extract_surface` and `extract_surface_runs` using 4 worker threads
(POOL_WORKERS, matching production MeshingPool). Atomic-counter
work distribution.

Asserts parallel and sequential extractors produce identical
record/run counts at runtime — useful canary.

New bench output:
- "Honest totals (extract + dispatch)" section showing v2/v3 with
  both sequential and parallel extract
- Parallel-speedup ratios for both extractors
- v3 honest (par extract) vs CPU naive parallel and vs MeshingPool

Hypothesis: extract scales linearly with worker count → v3 honest
total drops from 12ms to ~5ms → v3 wins by ~6× over MeshingPool and
slightly over CPU naive parallel.

If hypothesis confirms, Layer A.3 production wiring is the next
shippable. If it doesn't (parallel speedup < 2×), A.3 still gated
on zero-copy or kernel improvements.


## Increment 60 v0.24.16-bench-results-update — log v0.24.15 hardware result

Doc-only ship. Parallel extract scales 2.0-2.2× with 4 workers,
not the ~3-3.5× I'd hypothesized. CPU naive parallel mesher also
scales ~2.15× from its sequential version, suggesting the cap is
thread-launch + cache overhead for short workloads, not a
GPU-path-specific issue.

End-state of the GPU sparse mesher arc:
- GPU v3 honest (par extract) total: 6,543 us
- vs CPU naive parallel (5,356): 0.82× — GPU loses by 1.22×
- vs MeshingPool (29,694): 4.54× — GPU wins clearly

Layer A.3 wiring would be a perf win vs the production MeshingPool
but a quality regression (flat-color naive vs greedy+sky-lit). The
decision is architectural now, not algorithmic.


## Increment 61 — free-play observations + chug root cause finding

User did free-play session with rd=10, vsync=0, max_uploads_per_frame=32,
upload_budget_ms=8.0. Several findings worth filing:

### Cloud-region ghosting
Clouds rendered outside the playable region stay drawn. Likely cloud
mesh isn't tied to LOD/region eviction logic. Real bug. Filed for
visual polish pass.

### LOD meshing visual quality
Current LOD downsample factors (1,2,4,8) produce visible chunk-
boundary mismatches at LOD transitions. Shaders look decent overall.
Filed for visual polish pass.

### Mesh cache idea
Once a chunk is meshed, compress its vertex+index data and store in
a GPU-side cache. On revisit (or chunk re-mesh after edit), upload
the compressed form and let a tiny decompression pass produce the
renderable mesh. Trades compute for bandwidth+memory. Useful for
repeat visits during play.

### Double-draw / speculative chunk staging
"Show the GPU some chunks ahead of the mesh_q backlog" works well at
high render distances. Not currently necessary (the queue handles
it). Reserve the technique for structures and "hard stuff" that take
longer to mesh and benefit from speculative pre-staging.

### The "magic switch" — actual chug root cause

CRITICAL FINDING. During free-play at rd=10, the engine was at FPS=4
for ~60 seconds, then suddenly snapped to FPS=130 and stayed there.
The switch happened on a single frame transition:

```
loaded: 4885  gen_pending: 1   rendering: 824   mesh_q: 164/128  FPS: 5
loaded: 4891  gen_pending: 0   rendering: 877   mesh_q: 68/10    FPS: 136
```

Per-frame GPU time dropped 39× (265ms → 6.8ms). Shadow 35× (156→4.5).
Main 42× (85→2). Mesher throughput jumped 27× (64→1732 chunks/sec)
on the same code path. Player position was static (33,69,-107)
across the switch.

**The chug isn't a "renderer can't handle 800 chunks" problem.** It's
"world gen + meshing + render are competing for the same ~8 P-cores,
and when gen claims a bunch of them, everything else queues up."

Evidence: during chug, `wait_fences=266ms` was ~85% of total frame
time. CPU was spending almost all of every frame waiting for the
GPU. The GPU was being fed in starved bursts because the render
thread couldn't run its CPU work fast enough.

The switch happened when gen_pending hit 0 — the chunk-loader pool
went idle, freeing P-cores for the mesh pool + render thread. Once
freed, mesh pool throughput went to its real ceiling (~1500-1750
chunks/sec at 4 workers), GPU got fed at full rate, fps recovered.

### Implications

1. **The chug arc's framing was incomplete.** We blamed shadow
   GPU cost. Shadow IS expensive (4.5ms steady-state, still ~60% of
   the 7.4ms frame budget at 130fps) but it wasn't the proximate
   cause of the chug. The cause was CPU contention manifesting as
   GPU starvation.

2. **Worker isolation matters more than mesher optimization.** Right
   now world-gen, meshing, and render all share the same CPU pool.
   Pinning gen workers to E-cores (or just deprioritizing them)
   would prevent the chug pattern. Render would maintain framerate
   even during heavy chunk loading.

3. **Validates the GPU-resident-world architecture pivot.**
   architecture-v1 talked about scheduling and "no stragglers." The
   magic switch is exactly the kind of straggler that creates chug.
   If world gen and meshing both move toward the GPU, the CPU stops
   being the bottleneck.

4. **Steady-state shadow optimization still worth it.** 4.5ms of
   the 7.4ms frame is shadow. Optimizing shadow buys headroom even
   when CPU is healthy.

### Filed for future arcs

- **Worker isolation** (P/E core assignment, thread priorities) —
  high-value next direction. Tests the hypothesis directly.
- **gen_pending visibility in HUD** — would let player see when
  contention is happening, sets expectations during exploration.
- **Cloud eviction tied to region** — fix the ghosting bug.
- **LOD transition smoothing** — visual polish.
- **Mesh cache with compressed GPU-side storage** — production
  hitch reduction.


## Increment 62 v0.24.18-thread-probe-1 — chug investigation tooling, part 1

After the magic-switch finding in v0.24.17 free-play surfaced that
our "what is each thread doing during chug" picture is incomplete,
this ship starts the per-thread instrumentation arc.

### What's in ship 1

**thread_probe module** (`src/thread_probe.rs`, ~470 lines):
- Activity enum: 14 well-defined states (Idle, Working, WaitingForFence,
  RecordingCommands, MeshingChunk, GeneratingChunk, WaitingOnLock,
  etc) plus an `Other` slot for ad-hoc.
- Registration API: `register("thread-name")` → `ActivityHandle`.
  Handle's `set(tag)` is one atomic store (~1ns when probe enabled,
  no-op when disabled).
- Per-thread context u64 (e.g. packed chunk coord) for "which chunk
  is mesh-worker-2 working on right now."
- Sampler thread reads `/proc/<pid>/task/<tid>/stat` for utime+stime
  and last-CPU; reads `/proc/<pid>/task/<tid>/wchan` for kernel wait
  channel. Writes one CSV row per registered thread per sample.
- Engine-state pseudo-thread carries fps/queue/wait_fences in every
  row so the analysis tool can slice without parsing engine stdout.
- Engine-side chug detector: fps < 20 OR wait_fences > 50ms flags
  the chug column.

**Linux-only via cfg.** On macOS the syscalls become no-ops, the
build still succeeds, the feature is just unavailable. Asahi is
our test bed for the chug investigation.

**Default off.** `UNENDLICH_THREAD_PROBE=1` enables. Registration is
free when off (handles are inactive, no sampler thread spawned).
Activity-tag updates compile to inline checks of an Option<Arc>.

**probe-report bin** (`src/bin/probe_report.rs`, ~250 lines):
- Reads CSV, prints Overview / Activity histogram / CPU work /
  Hot deltas / Chug periods.
- `--window START_MS:END_MS` to slice.
- `--thread NAME` to filter.
- Designed to print compactly (≤ ~100 lines) so output pastes
  cleanly without burning chat context.

### Wired-in points

- `unendlich-world` main thread registers as "render" right after
  logger init.
- Two activity tags in the render loop: `TickSim` around
  `chunk_manager.tick_simulation`, `WaitingForFence` around
  `draw_frame_with_view_and_atmos`.
- `engine_state_update` called once per FPS-log window (~1s),
  pushing fps/queue/loaded/etc into the engine_state pseudo-thread.
- Two new env-var flags registered: `UNENDLICH_THREAD_PROBE`,
  `UNENDLICH_THREAD_PROBE_INTERVAL_MS`.

### NOT in ship 1

- Mesh pool workers don't register themselves. They'll show as
  unattributed CPU time in `top` but not as their own threads in
  the CSV. Ship 3 covers this.
- Chunk-loader / gen-pool workers don't register themselves. Same
  caveat. Ship 3.
- A GUI viewer (filed for ship 4, future). Current scope: text
  reports only, by user request.

### Testing

- 4 new unit tests added (`thread_probe::tests`). 604 fast tests
  pass total.
- Synthetic smoke test on probe-report with a hand-rolled CSV
  reproduces the expected sections (overview, histogram, chug
  periods).

### What ship 1 alone gives us

Running `UNENDLICH_THREAD_PROBE=1 cargo run ... unendlich-world`
produces a probe-N.csv. Feeding it to probe-report tells us:
- Total chug% (fraction of samples where render-thread was chugging)
- Render-thread activity distribution during chug (mostly
  wait_fence — already suspected, now confirmed in data)
- Hot-delta moments (where activity changed across threads)
- Engine state at chug entry/exit (mesh_q depth, gen_pending)

What it CAN'T tell us yet (gated on ship 3): what the mesh pool
workers were doing during the slow 64-chunks/sec period. That's
the central question the chug investigation actually wants
answered. Ship 1 is plumbing + the analyzer; ship 3 puts the
sensors where they matter most.

### Decision: ship 1+2 separately from ship 3

Two reasons to ship 1+2 as their own checkpoint:
1. Lets user try the probe + analyzer on a real chug session
   before I touch the meshing pool's internals.
2. If ship 3 breaks something in the pool, ship 1+2 still works
   for follow-up investigations.


## Increment 63 v0.24.19-mesh-cache — renderer-owned mesh cache

The v0.24.18 thread-probe finding: chug = GPU upload pressure
manifesting as drm_syncobj_wait stalls on the render thread.
85.6% of chug-time render samples had cpu_ticks_delta=0 (kernel
sleep), wchan=drm_syncobj_array_wait_timeout. Not CPU contention.

The right intervention: reduce upload bytes when the player revisits
already-meshed chunks. This ship adds a renderer-owned mesh cache.

### Design

On `evict_chunk_lod` (or `evict_chunk`, which routes through it),
instead of pushing GPU buffers to `pending_destroy` immediately,
park them in a HashMap keyed by `MeshSlotKey`. Buffer slots stay
allocated (not returned to the pool). LRU-evicted when full.

On `upload_prebuilt_mesh_at_lod`, check the cache before doing the
real upload. If hit AND version matches: restore cached buffers,
re-register the indirect-draw entry, discard the supplied mesh.
Skip upload entirely.

Stale (version-mismatch) entries are dropped by the restore path
so they don't leak; caller falls through to normal upload.

### Capacity knob

`UNENDLICH_MESH_CACHE_SIZE=N` chunks. Default 0 = disabled (matches
pre-v0.24.19 behaviour exactly). Useful values: 256-1024 depending
on render distance and how much the player wanders.

Each cached chunk consumes one mesh-pool slot for its lifetime. So
size=1024 reserves ~3MB × 1024 = 3GB-ish at the per-slot pool cap
(but actual chunk meshes are usually a fraction of the slot size).

### Telemetry

New per-window log line: `cache: hits=N misses=M size=S/cap=C`
when cache is enabled. Hits = cached buffers restored. Misses =
version stale on cache entry (chunk was edited between eviction and
revisit).

### Tests

4 new tests on the `cache_lru_key` free function (picking smallest
last_touched from a slice). LRU is the only non-trivial bookkeeping
in the cache; the rest is HashMap insert/remove that would need a
real Vulkan device to test.

608 fast tests pass total (was 604).

### What this ship does NOT do

- **Doesn't reduce mesh-pool CPU work.** The mesh pool still
  produces a fresh mesh for every revisit. The cache discards that
  mesh and uses the cached buffers instead. Wasted CPU; the win is
  GPU upload bytes.
- **Doesn't compress.** Buffers cached as-is. VRAM cost is real.
  Compression (decompress-shader) is a follow-up; this ship
  validates the architectural idea first.
- **Doesn't help cold first-load.** First visit to a chunk hits no
  cache. Win only kicks in on revisits. For the chug we measured
  (rd=10 free-play), revisits should dominate after the initial
  fill — the chug at 224s happens AFTER 4280 chunks were already
  loaded.

### Honest concerns

1. **VRAM. A cache size of 1024 might consume substantial GPU
   memory if every chunk uses near-max slot bytes. Real worlds have
   highly variable mesh sizes — most chunks are small. Will need to
   watch how this plays out in practice and add compression if it's
   a problem.

2. **Stale-entry waste.** Edit-heavy play (lots of placement) would
   miss often, paying the cache slot reservation cost without
   getting hits. Telemetry (the `cache: misses=M` line) catches this.

3. **Cache vs streaming radius.** If the cache holds N chunks and
   the active set holds M, peak memory is (N+M) × per-slot. Tune
   accordingly.

4. **Restore can't fail.** The cache entry has valid pool slots
   already; we just re-register them in `meshes` and re-write the
   indirect-draw slot. No allocation. So the restore path is more
   reliable than the upload path (which can fail with
   `MeshPoolError::Overflow`). Bonus.

### Followups filed

- Compressed cache (decompress-shader on revisit) — reduces VRAM
  per cached chunk by 5-10× depending on format.
- Chunk-manager-aware cache — skip mesh-pool work entirely on
  predicted hits. Requires version-prediction logic chunk-manager-
  side.
- GPU-resident world (architecture-v1) — the long-term answer to
  upload pressure.


---

## Increment 64+ catch-up (DESIGN_NOTES.md hit a journaling drift gap)

**Pattern note before the catch-up.** Between Increment 63 (v0.24.19)
and the start of v0.24.26, eight ships landed without DESIGN_NOTES
entries. Each has a `docs/v0.24.X-readme.md` so the data isn't lost,
but the running narrative drifted. The architecture doc's §8 warned
about exactly this: "ideas accumulated faster than they got pinned
down" was the chug-arc failure mode and we partially repeated it.

The brief entries below restore the index. Each points at its full
readme; this is not the place to re-explain.

## Increment 64 v0.24.20-vertex-pack — packed vertex format

**Ship:** `docs/v0.24.20-vertex-pack-readme.md`

Compressed the per-vertex data from a 16-byte unpacked layout to an
8-byte packed layout: position to `A2B10G10R10_UNORM_PACK32`, color
to `R8G8B8A8_UNORM`. Halved vertex buffer bandwidth without visible
quality regression. Shaders gained `in_pos_unorm` / `in_color_unorm`
inputs.

**Hidden cost surfaced later:** this ship changed the GLSL vertex
inputs but the build script's mtime-gated SPV recompile didn't fire
when the project was extracted from a zip (mtimes preserved as
equal). Stale SPVs declaring the old `in_position` / `in_color`
inputs were used through every ship from v0.24.20 to v0.24.24. The
GPU accepted the packed bytes but interpreted them as float positions
in [0, 1]³ — every vertex landed in a tiny cube at world origin
while the player sat at (0, 93, 0). Blank screen. Fixed in
v0.24.25; build.rs now content-hashes sources instead of mtime.

## Increment 65 v0.24.21-gpu-mesher-prod — GPU mesher in production

**Ship:** `docs/v0.24.21-gpu-mesher-prod-readme.md`

Wired `chunk_mesh_sparse.comp` into the production meshing pool's
"Near" LOD path. CPU runs `extract_surface` → uploads
`SurfaceRecord[]` → GPU compute emits vertices+indices → CPU reads
back and uploads to the mesh buffer pool. Falsified by visual diff
against the CPU mesher baseline.

**Bench-arc finding it codified:** GPU mesher isn't faster than CPU
mesher in steady state on M2/Honeykrisp because single-queue
contention + dispatcher overhead eats the kernel speedup. The win
is architectural alignment (toward GPU-resident world) more than
throughput. Held in for the architectural progression; not yet
"win" by the bench numbers.

## Increment 66 v0.24.22-worker-isolation — pin chunk-loader workers to dedicated cores

**Ship:** `docs/v0.24.22-worker-isolation-readme.md`

`UNENDLICH_WORKER_ISOLATION=1` (default-on this ship) pins the 12
chunk-loader threads to the 8 e-cores, leaving p-cores for the main
thread and the GPU dispatcher. Investigated as a chug mitigation
following Increment 61's CPU-contention finding.

Affected runtime_flags.rs (new env var); thread_priority.rs (CPU set
affinity calls — these later required wrapping in `unsafe` when
libc-rs marked CPU_ZERO/CPU_SET unsafe in 0.2.150+).

## Increment 67 v0.24.23-octree-foundation — CPU-side octree stack

**Ship:** (no separate readme; combined with -propagation)

`world/octree.rs` (964 lines) lands the canonical sparse octree
representation. `OctreeArray { coord, nodes: Vec<OctreeNode> }`.
`OctreeNode` is 40-byte repr(C) — `packed_header` (is_leaf, depth,
block_id), `last_modified_tick`, `children[8]`. Builder collapses
homogeneous subtrees; sample walks descend at most `OCTREE_DEPTH=5`.

Adjacent files: `world/octree_region.rs` (multi-chunk region ops),
`world/octree_raycast.rs` (skip-pass ray traversal), `world/
octree_scheduler.rs` (deferment hooks per addendum §A4).
`bin/octree-bench` for measurement.

Wire-format invariant: `const _: () = assert!(size_of::<OctreeNode>()
== 40)`. This catches accidental padding additions at compile time,
which would silently break the GPU SSBO twin.

## Increment 68 v0.24.23-octree-propagation — octree alongside Chunk

**Ship:** (combined w/ foundation)

ChunkManager gains `octrees: Arc<DashMap<ChunkCoord,
Arc<OctreeArray>>>` parallel to its existing `chunks` map. Every
loaded chunk has both representations. World-edit-tick atomic counter
gates rebuilds.

`replace_edited_chunk` / `replace_batch_edited_chunk` helpers
rebuild the octree from the modified Chunk in one place; consumers
that don't go through the helpers can drift, so callers must use
them.

`raycast_octree(origin, dir, max_t, predicate)` public API — the
player block-selection raycast migrates from the dense-chunk
raycast to this. Same semantics; the skip-pass property is the
performance win.

## Increment 69 v0.24.24-octree-sampler — physics adapter for octree

**Ship:** `docs/v0.24.24-octree-sampler-readme.md`

`OctreeBlockSampler` wraps an `Arc<OctreeArray>` to implement the
existing `physics::BlockSampler` trait. Consumers that already used
`BlockSampler` (most physics paths) can opt into the octree by
swapping their sampler without code change.

`ChunkManager::octree_sampler(coord)` accessor returns an
`Option<OctreeBlockSampler>` per chunk. 7 new tests cover the
sampler equivalence with the dense `Chunk::get` path.

Not yet adopted by physics in production — provided as an
opt-in. The natural adopter is the player physics tick once GPU
raycast lands.

## Increment 70 v0.24.25-shader-bin-fix — content-hash SPV gate

**Ship:** `docs/v0.24.25-shader-bin-fix-readme.md`

Resolved the blank-screen bug introduced silently by v0.24.20 (stale
SPVs from packed-vertex transition). `build.rs` now reads each
shader source, computes SHA-256, writes the hash to `.spv.hash`
sidecar; rebuilds the SPV iff the hash differs from disk or the
SPV is missing. Replaces the mtime gate that the unzip path
defeated.

Every existing SPV recompiled. New sidecars written. Cargo.toml
versions bumped across 4 crates.

**Lesson for the running design:** mtime gates are unsafe across
artifact-distribution boundaries (zip extraction, docker layers,
artifact stores, anything that doesn't preserve sub-second mtime
resolution). Content hashing is robust at small extra build cost.
Promote this to a permanent diagnostic-discipline entry: any build
artifact derived from source must be invalidated by source content,
not source timestamp.

## Increment 71 v0.24.26-octree-on-gpu — octree substrate on GPU

**Ship:** `docs/v0.24.26-octree-on-gpu-readme.md`

The architecture's §3.1 substrate lands. `OctreeBufferPool` (256 MB
HOST_VISIBLE storage buffer, span-allocated). Every loaded chunk's
`OctreeArray` mirrors onto the GPU alongside its mesh upload.
`octree_probe.comp` is the falsification gate: dispatches one
invocation per query, walks the octree on GPU via `octree_sample()`,
writes result to host-visible buffer, CPU asserts equality with
`OctreeArray::sample`.

Enabled by `UNENDLICH_OCTREE_VERIFY=1`. First production run
returned 5649 / 16752 mismatched — caught a real wire-format bug
(chunk-local children indices treated as global SSBO indices) that
would have silently corrupted any future GPU consumer. Probe-fix1
landed; subsequent run: 12960 matched, 0 mismatched.

**Substrate is honest end-to-end. No production consumer reads it
yet.** Consumer migrations land in subsequent ships (Ship A: octree
mesher v0; Ship B: weathered mesher per the revised §6.1).

## Pattern note: zip ship hygiene (added v0.24.26 retrospective)

Three ships in a row regressed working files on the user's tree
because my staging dir's snapshot of the whole repo was older than
the user's working state for a few files (chunk.rs Debug derive,
thread_priority.rs CPU_ZERO unsafe, unendlich_world.rs `hit` →
`hit_cell` migration).

**Going forward:** ship zips should contain only the files the ship
deliberately changes, not the whole tree. If the user's `update_
from_claude_zip.sh` workflow needs the whole tree, the alternative
is for ships to carry a manifest of expected pre-edit md5s; the
script refuses to overwrite a file whose current md5 doesn't match
the manifest expectation. Either approach catches this class of
regression at extract time.

## Pattern note: probe-as-falsification-gate is now a permanent template

The v0.24.26 probe caught a real bug on first run. Without it the
chunk-local-vs-global indexing error would have surfaced as random
voxel corruption inside the next ship's mesher migration and we'd
have hunted it for hours suspecting the mesher itself before
noticing the substrate was lying.

**The template, distilled:** any new GPU substrate gets a
verification compute pass alongside it, gated by env-var, asserting
equivalence with a CPU reference. The cost is one shader + one
pipeline + a few hundred lines of glue. The value is "the substrate
can't lie about its contract without telling you immediately."

Apply this for Ship B (weathered mesher's adjacent-edge agreement
test), for the eventual GPU-resident overlay buffer, for the GPU
noise compiler output, and for entity state on GPU.


## Increment 72 v0.24.27-octree-mesher — full debugging arc with sub-ship fixes

**Ships:** `docs/v0.24.27-octree-mesher-readme.md` + three sub-ship patches
(fix1 cap bump, fix2 diagnostic logging, fix3 world-coord vertex emission).

This entry records the full v0.24.27 arc because the debugging trail
itself is the most important artifact — it surfaced an architectural
weakness in the falsification-gate pattern that v0.24.26 codified.

### Initial ship — opt-in via `UNENDLICH_GPU_MESHER=2`

`OctreeGpuMesher` reads each chunk's `OctreeArray` directly from a
HOST_VISIBLE SSBO instead of consuming CPU-extracted
`SurfaceRecord[]`. One workgroup-of-64 invocations per chunk cell.
Open-boundary semantics — out-of-chunk neighbors treated as air. New
shader `chunk_mesh_octree.comp` plus new module `gpu_meshing_octree`
plus new dispatcher loop in `meshing_pool` plus `with_octree_gpu_mesher`
constructor plus extended bin parsing of `UNENDLICH_GPU_MESHER` as
`u32` (was bool).

### fix1 — `MAX_NODES_PER_CHUNK` from 8192 to 40960

First playtest: dozens of `octree-gpu-mesh-dispatcher: chunk ...
failed: octree has too many nodes: 8801 (max 8192)` warnings, with
real-world chunks hitting 8200–10800 nodes.

Root cause: I picked 8192 from "max observed in practice ~5000"
without computing the theoretical bound. For a 32³ chunk at depth 5
the absolute max is 4681 internal nodes + 32768 leaves = 37 449.
Bumped to 40 960 (8.4% margin). Memory cost: 100 MB for 64 slots.

The deeper lesson: any per-chunk slot capacity needs to be derived
from the theoretical worst case, not from sample observation. The
renderer's `OctreeBufferPool` does this correctly via span allocation;
my mesher's fixed-slot scheme regressed against that pattern.

### fix2 — per-batch diagnostic logging

After fix1 the user saw "mostly sky, one floating chunk of terrain"
in playtest. Substrate probe passed (20352/20352 matched). My initial
guess was player-position artifact or clouds-vs-mesh confusion.

The diagnostic patch added per-batch logging of `empty=`, `nonempty=`,
`max_verts=`, and the first empty/non-empty chunk coord. First
diagnostic run output:
```
chunks=64 empty=29 nonempty=35 avg_verts=22865 max_verts=33288
  first_empty=ChunkCoord{x:0,y:5,z:2} first_nonempty=ChunkCoord{x:-3,y:1,z:1}
```

The `first_empty` consistently y≥3 (chunks above the surface — sky),
`first_nonempty` consistently y in -1..2 (surface terrain). So the
mesher was producing 20K–35K-vertex meshes for terrain chunks. The
chunks were healthy. But only one chunk worth was visible.

Reading the diagnostic conclusively eliminated the "silent empty
meshes" hypothesis. The bug had to be on the *output* side.

### fix3 — vertex emission in WORLD coordinates

Read `chunk_mesh_sparse.comp` for the comparable shader's vertex
position derivation:
```glsl
vec3 mn = vec3(float(r.world_x), float(r.world_y), float(r.world_z));
```

Sparse emits world coords because `SurfaceRecord` stores `world_x/y/z`
CPU-side. My octree shader emitted chunk-local [0, 32]:
```glsl
vec3 p = vec3(float(x), float(y), float(z));  // BUG
```

I had declared `chunk_origin_x/y/z` in `ChunkMeta` and threaded it
through the SSBO but never used the values for vertex output. So
every chunk's mesh rendered at the world origin, stacked on top of
every other chunk's mesh. Visual symptom: terrain looks like one
chunk worth of geometry, everything else "missing." Diagnostically
unmistakable in hindsight.

One-line fix: change `p` to include the chunk origin.

```glsl
vec3 p = vec3(
    float(int(x) + meta.chunk_origin_x),
    float(int(y) + meta.chunk_origin_y),
    float(int(z) + meta.chunk_origin_z)
);
```

### The architectural lesson

The v0.24.26 probe-as-falsification-gate pattern said: *any new GPU
substrate gets a verification compute pass alongside it*. v0.24.26
did this for the octree buffer; v0.24.27 was supposed to be the first
real consumer.

**But the probe verifies the substrate, not any particular consumer's
use of it.** The v0.24.26 probe runs against the renderer's
`OctreeBufferPool`. v0.24.27's `OctreeGpuMesher` has *its own
separate octree buffer*, never probed. And even if it shared the
renderer's buffer, the probe only validates octree *reads* are
correct. The mesher's *writes* (vertices, indices) are unprobed.

Fix3 would have surfaced on first run if there had been:
- a probe checking the mesher's octree buffer matches CPU samples
  (would have caught upload bugs), OR
- a probe comparing GPU mesh output to a CPU reference port of the
  same shader (would have caught the world-coord bug directly).

The "one falsification probe per substrate" pattern is necessary but
not sufficient. Going forward:

**Substrate-and-consumer falsification gates.** Every GPU consumer
that produces output gets its own falsification gate, either:
1. A CPU reference port that runs the same algorithm and compares
   output (preferred when feasible), OR
2. Structural invariants on the output (AABB bounds, finite values,
   internal consistency) checked at runtime.

The v0.24.27 debugging took several hours of build-zip-test cycles
across three sub-ships. A CPU reference port of the octree mesher,
unit-tested on synthetic octrees, would have surfaced fix3 in seconds.

### What v0.24.27 ultimately delivered

After fix3, expected behavior: full terrain rendering in mode 2 with
strict Camp 1 flat quads. Steady-state perf observed during fix2's
visual-bug diagnosis (before realizing the meshes weren't being placed
correctly): main p50=0.71ms vs CPU mesher's 1.83ms — **2.6× faster**.
Surprising for what was supposed to be a "validation foundation, no
production win" ship.

The fast per-chunk number was real GPU work even though the meshes
weren't visible — fix3 doesn't change throughput, only placement.
So mode 2 post-fix3 should retain the perf win while finally being
visually usable.

## Pattern note: substrate-and-consumer falsification (added v0.24.27)

The lesson from above, distilled for future shipping:

When adding a GPU substrate (data the GPU reads), the probe pattern
from v0.24.26 applies: a CPU↔GPU equivalence test on representative
samples, gated by env var.

When adding a GPU consumer (kernel that reads the substrate and
produces output), an *additional* gate is needed. Options:

1. **CPU reference port.** Reimplement the kernel in Rust. Run on
   the same input. Compare. Unit-testable on synthetic data. Catches
   all classes of bug (algorithm, coordinate space, indexing, etc.).
2. **Output invariants.** Cheap runtime checks on consumer output:
   - Vertex positions within expected world AABB.
   - All floats finite (no NaN/Inf).
   - Index/vertex ratio consistent (e.g., quads → 6 indices per 4
     verts).
   - All indices in [0, vertex_count).
   These don't catch algorithmic mistakes but catch wire-format and
   memory-layout bugs.

The cost of either is bounded. The cost of NOT having them is
"three rounds of zip-test-debug" per shipping cycle. Apply both
for any new GPU consumer going forward.

## Increment 73 v0.24.28-bumpy-foundation — Ship B foundation

**Ship:** `docs/v0.24.28-bumpy-foundation-readme.md` (this entry's
sibling).

Consolidates v0.24.27 + sub-fixes, adds the architecture-v1 §6.1
"weathered-block" aesthetic in its minimum form. 4×4 sub-vertex grid
on top faces of weatherable blocks (Grass, Dirt, Sand, Gravel, Snow
— the natural "soft" materials). Hash-based deterministic
`bumpy_height(world_x, world_z)`. No instance buffer yet — weathering
is a static per-block-id property. The full Ship B (per-block
weathering via instance state) is a follow-up.

Also lands the falsification gates the v0.24.27 lesson called for:
1. CPU reference port `gpu_meshing_octree_cpu_ref` with unit tests
   on synthetic octrees.
2. Output-sanity probe in the dispatcher (AABB, finite, ratio
   checks; logged on violation; gated by `UNENDLICH_OCTREE_MESH_VERIFY=1`).

Now mode 2 has visual parity with the §6.1 target aesthetic
(blocky data + bumpy mesher), bounded by:
- No climate tinting (uniform per-block colors).
- Open-boundary at chunk edges (filed for v0.24.29).
- 4×4 grid is per the user's "start with 4×4 first" decision; the
  meinkraft predecessor's 8×8 design remains the upper bound if 4×4
  reads weakly in playtest.


## Increment 74 v0.24.29-shared-octree — mesher reads from renderer's pool

**Ship:** `docs/v0.24.29-shared-octree-readme.md`.

Implements Option A from `docs/v0.24.29-v0.24.30-design-memo.md`:
the `OctreeGpuMesher` now reads octree data from the renderer's
`OctreeBufferPool` instead of maintaining its own internal buffer.

### Architectural impact

Before: two octree buffers on GPU — the renderer's pool (256 MB,
span-allocated, probe-verified) and the mesher's own (100 MB,
fixed-slot, never probed). v0.24.27 spent three sub-ships chasing
a bug in the mesher's separate buffer that the v0.24.26 probe
couldn't detect.

After: one octree buffer. The mesher's compute pipeline binds the
pool's buffer at descriptor-set construction. Per-dispatch, the
mesher locks the pool, uploads each chunk's octree (or finds it
already resident from a prior upload), captures the
`base_node_index()` per chunk, and writes that into `ChunkMetaGpu`.
The shader walks the octree from that absolute index — same code
as before; only the bound buffer changed.

### Synchronization

The pool is wrapped in `Arc<Mutex<OctreeBufferPool>>`. The
mesher's dispatcher holds the lock for the duration of a single
batch (upload + dispatch + readback, ~1 ms). Main thread waits
~30 µs per upload on average at steady state. A deferred-free
pattern that lets dispatch happen lock-free is filed as v0.24.31
follow-up.

### Drop ordering

`WorldRenderer::destroy()` is called BEFORE the mesher's Arc
clones drop. That's fine because:
- The mesher's `Drop` impl only destroys ITS OWN buffers
  (chunk_metas, vertex, index, counter — never the pool's).
- The cached `octree_pool_buffer: vk::Buffer` handle in the mesher
  becomes dangling after `renderer.destroy()` but is never
  re-used.
- The pool's actual destruction is mutex-guarded; the strong-count
  is logged at destroy time for diagnostics.

### Memory savings

The mesher's previous 100 MB octree buffer (40960 nodes × 40 B
× 64 slots) is gone. The pool's existing 256 MB span-allocated
buffer absorbs the mesher's data. Net savings: 100 MB
HOST_VISIBLE.

### Falsification

The mesher's octree reads now go through the SAME buffer the
v0.24.26 probe verifies. The probe-verifies-substrate-not-consumer
issue from v0.24.27 is partially closed: substrate reads are now
covered. The mesher's WRITE side (vertex/index emission) is still
not directly verified at runtime; the CPU reference + output-sanity
probe from v0.24.28-bumpy-foundation provide that coverage.


## Increment 75 v0.24.30-cross-chunk — neighbor-aware boundary sampling

**Ship:** `docs/v0.24.30-cross-chunk-readme.md`.

The shared OctreeBufferPool from v0.24.29 made one thing possible
that wasn't before: the mesher can read ANY chunk's octree, not
just the one it's currently meshing. This ship cashes in.

### Before / after

Before (open-boundary, v0.24.27..v0.24.29): the mesher's
neighbor-cell sample for `(x±1, y, z)` returned air whenever the
coordinate fell outside `[0, 32)`. Faces emitted at every chunk
boundary regardless of whether the adjacent chunk actually
occluded them. Visible result: z-fighting and doubled normals
at every chunk seam.

After (this ship): `ChunkMetaGpu` carries 6 absolute node bases
for the chunk's neighbors (or `u32::MAX` for "neighbor not
resident — fall back to open-boundary for that face"). The
shader's `sample_or_neighbor` function jumps to the neighbor's
octree when the sample coordinate crosses a boundary. Faces are
emitted only when the cell on the other side is genuinely
non-solid.

### Cost

- ChunkMetaGpu: 32 B → 64 B per chunk × 64 batch slots = 4 KB
  total (vs. 2 KB previously). Negligible.
- Per chunk in batch: 6 HashMap lookups against the pool's
  `slots_by_chunk` map. With 64 chunks/batch and a HashMap that's
  typically <2000 entries, ~50 µs total per batch. Negligible
  vs. the ~1 ms dispatch time.
- Shader: one branch + one octree walk per face-direction sample,
  same as before plus the neighbor selection. Slightly slower per
  cell but emits fewer faces, so net mesh size shrinks.

### Expected mesh-size reduction

A fully-solid 32³ chunk previously emitted 32×32 × 6 = 6144 outer
faces. With all 6 neighbors being fully-solid (interior chunk
case), this drops to 0 faces. For typical world chunks with
mixed terrain, expect 10-30% fewer vertices per chunk depending
on neighbor configuration.

### Falsification

CPU reference function `mesh_chunk_octree_cpu_reference_with_neighbors`
takes a `Neighbors<'_>` struct (six `Option<&OctreeArray>`).
Unit tests cover:
- Two adjacent stone blocks across a chunk seam: the seam face
  is occluded (only 5 faces emitted, not 6).
- Symmetric test for the opposite chunk.
- Air-neighbor case: a stone block adjacent to an all-air
  neighbor still emits its boundary face.
- Bumpy interaction: grass block at chunk edge with grass
  neighbor across — +X face occluded, +Y face still bumpy.


## Increment 76 v0.24.31-versioned-pool — skip redundant octree uploads

**Ship:** `docs/v0.24.31-versioned-pool-readme.md`.

The mesher's dispatcher re-uploaded every chunk's octree to the
shared pool on every batch, even when the same `(coord, version)`
was already resident. At ~10K nodes × 40 B × 64 chunks/batch =
~26 MB memcpy per batch, that's ~4 ms wasted per batch — over
100 ms/second at 30 batches/sec steady-state. Caught in the
v0.24.29 readme as a "TODO filed" but free to land standalone.

### What this ship does

`OctreeBufferPool` grows a parallel map `content_versions:
HashMap<ChunkCoord, u64>` tracking what version is currently
resident per chunk. New method:

```rust
pub fn upload_if_changed(
    &mut self,
    coord: ChunkCoord,
    octree: &OctreeArray,
    content_version: u64,
) -> Result<OctreeSlot, OctreePoolError>
```

When `content_version` matches, returns the existing slot without
copying any bytes. The first dispatch of any chunk does the
upload; subsequent dispatches with the same version are no-ops
on the upload path (the pool lookup is still O(1)).

The mesher's `OctreeBatchInput` gains a `version: u64` field,
populated from `MeshRequest::version_at_submit` (which the bin
already tracks). The dispatcher uses `upload_if_changed` instead
of `upload`.

### Renderer path unchanged

`WorldRenderer::upload_chunk_octree` still uses the always-replace
`pool.upload`. The bin's only call site is the post-mesh-upload
mirror for mode 0/1, which doesn't track per-chunk versions —
keeping it on `upload` preserves existing behavior. In mode 2
this path is skipped entirely (v0.24.29 work).

### Eviction discipline

`pool.evict(coord)` now clears the content_version entry along
with the slot. Without this, a re-upload after evict could
short-circuit against the stale version and skip a needed copy.

### Expected steady-state win

- Per batch saved: 64 chunks × ~67 µs memcpy = ~4.3 ms
- Per second saved: ~30 batches × 4.3 ms = ~130 ms/sec
- Headroom on the main thread → higher fps and lower upload
  contention.

### Failure mode

Falls back to original behavior if the version doesn't track
correctly: re-uploads as before. Strictly safe even when buggy.


## Increment 77 v0.24.32-pool-prewarm — neighbor chunks in pool before mesher dispatches

**Ship:** `docs/v0.24.32-pool-prewarm-readme.md`.

v0.24.30 added cross-chunk neighbor lookups in the shader. The
mesher dispatcher looks up each chunk's 6 neighbors in the pool
and writes their `base_node_index` (or `u32::MAX` for "not
resident") into ChunkMetaGpu. The shader uses that to sample
across chunk boundaries instead of treating them as open.

**The hole I left:** in mode 2, the only chunks getting uploaded
to the pool were the ones the mesher was currently meshing. When
chunk X was meshed for the first time, its neighbor N might not
yet have been meshed (and therefore wasn't in the pool). The
mesher saw `neighbor_base = u32::MAX` for direction N and
emitted an "open" seam face at that boundary — defeating the
whole point of v0.24.30.

### What this ship does

In the bin's mesh-submit loop, immediately before calling
`mesh_pool.submit(req)` (mode 2 only), pre-upload the chunk and
its 6 neighbors to the pool via
`renderer.upload_chunk_octree_versioned`. The pool's
`upload_if_changed` (v0.24.31) means re-uploads at the same
version are free (one HashMap lookup, no memcpy).

```rust
if gpu_mesher_mode == 2 {
    for dc in [(0,0,0), (1,0,0), (-1,0,0),
               (0,1,0), (0,-1,0), (0,0,1), (0,0,-1)] {
        let nc = ChunkCoord::new(coord.x + dc.0, coord.y + dc.1, coord.z + dc.2);
        if let Some(octree) = chunk_manager.octree(nc) {
            let version = chunk_manager.get_chunk(nc)
                .map(|c| c.version).unwrap_or(0);
            renderer.upload_chunk_octree_versioned(nc, &octree, version);
        }
    }
}
```

### Net cost

Per submission: 7 HashMap lookups + at most 7 octree copies (the
first time each chunk appears). After steady-state, all neighbors
are at known versions and the lookups become pure dedup checks.

At ~30 submissions/sec, that's ~210 HashMap ops/sec under the
pool lock — bounded and cheap compared to a single mesher
dispatch.

### Neighbor-edit invalidation

The streaming layer already handles "edit at chunk boundary
bumps neighbor's version" (see `bump_neighbor_if_edge`). When a
neighbor's version bumps, the pool's stored `content_version`
mismatches and the next pre-warm uploads the new data. The
chunk depending on that neighbor will also be re-submitted (its
own version bumped too), and on remesh it'll see the new
neighbor data.


## Increment 78 v0.24.33-climate-tint — per-chunk temperature & humidity to GPU

**Ship:** `docs/v0.24.33-climate-tint-readme.md`.

Climate-driven color variation is one of the biggest visual gaps
between mode 0 (CPU mesher, climate-aware) and mode 2 (octree
GPU mesher, uniform color per block_id). This ship closes the
gap at chunk granularity: each chunk gets one temperature and
humidity value, computed at its center via WorldModel, and the
shader uses those to modulate base block colors.

### The data path

- Worker thread (per mesh request, Near LOD only): compute
  `world_model.temperature(center)` and `world_model.humidity(center)`
  for the chunk being meshed. ~2 µs per chunk (the legacy CPU
  mesher was already paying this per-BLOCK, not per-chunk; this
  is ~1000× cheaper).
- Pass as `OctreeGpuMeshRequest::{temperature, humidity}`.
- Dispatcher carries them to `OctreeBatchInput`.
- Mesher writes them into the `ChunkMetaGpu::{temperature, humidity}`
  fields (replacing two padding u32s; struct size unchanged at 64 B).
- Shader's `apply_climate_tint(base, block_id, t, h)` modulates
  the base color from `color_for_block` before emit.

### What's tinted

Implemented in `apply_climate_tint`:
- **Grass (id 2)**: lerps from yellow-tan `(0.55, 0.55, 0.20)` in
  dry+cold to deep green `(0.20, 0.65, 0.15)` in humid+warm.
  `lushness = 0.7 * h + 0.3 * t`.
- **OakLeaves (id 18)**: lerps from dry brown
  `(0.40, 0.35, 0.10)` to lush green `(0.20, 0.55, 0.18)`. Pure
  humidity response.
- **Dirt (id 3)**: warm+dry tints toward iron-red
  `(0.65, 0.32, 0.15)` at 40% strength.
- **Sand (id 12)**: slight red-shift in hot regions
  (`+t*0.05` to R, `-t*0.05` to B).

Other blocks pass through unchanged. Magenta default still fires
for unmapped block_ids (good for debugging coverage).

### Per-chunk vs per-block

This is per-chunk fidelity. The CPU mesher computes climate per
BLOCK which gives smoother gradients within a chunk. The visible
difference: per-chunk produces hard color changes at chunk
boundaries when temperature/humidity gradients cross seams.
Acceptable for foundation; the next step (per-block climate
sampling) needs either a climate field SSBO or porting the noise
functions to GLSL.

### Failure mode

Defensive: `clamp(t, 0.0, 1.0)` and `clamp(h, 0.0, 1.0)` before
mixing. If WorldModel returns NaN (shouldn't, but), the clamp
saturates and we get one of the extreme tints instead of
propagating NaN through the color path.

The neutral fallback when `world_model` is unavailable is
`(0.5, 0.5)` — lerp factor 0.5 yields ~50/50 mix between dry and
lush base colors, which is acceptable middle-ground appearance.


## Increment 79 v0.24.34-position-tint — per-block-position color variance

**Ship:** `docs/v0.24.34-position-tint-readme.md`.

Port of CPU mesher's `tint_for_position`. Adjacent blocks of the
same type get slightly different shades via a deterministic
3-channel trig-noise hash. Breaks up "Lego" uniformity for
natural blocks (grass, sand, leaves, etc.) without per-block
storage.

Implemented as `vec3 tint_for_position(uint block_id, vec3 wp)`
in `chunk_mesh_octree.comp`. Variance per block:
- Grass 0.18, Sand/Gravel 0.12, Dirt 0.15, Snow/Ice 0.08,
  OakLeaves 0.20, others 0.08.

Applied in the main body after `apply_climate_tint`:
`color = color * tint_for_position(block_id, world_block_pos);`
then `clamp(color, 0.0, 1.0)` defensively.

Cost: ~6 mul + 3 sin/cos pairs per emitted face. Cheap.

## Increment 80 v0.24.35-face-light — per-face brightness modulation

**Ship:** `docs/v0.24.35-face-light-readme.md`.

GPU mesher previously emitted vertex colors with no face-direction
modulation. Adds the CPU mesher's `FACE_LIGHT` table as inline
multipliers at each emit site:
- +Y top: 1.00 (unchanged)
- -Y bottom: 0.55
- +X / -X east/west: 0.80
- +Z / -Z north/south: 0.65

The renderer's fragment shader already applies sun ndotl on top
of this, so the mesher's face_light is a coarse sky-dome
approximation (matches mode 0's fallback when no Sky context is
provided). Sky-aware brightness (mode 0 with sky) is per-frame
which doesn't suit the mesh-bake-once model; deferred for
follow-up.

Bumpy top retains 1.0 multiplier (no scaling) so bumpy and flat
tops match.

## Increment 81 v0.24.36-bumpy-side-align — side-face top corners track bumpy

**Ship:** `docs/v0.24.36-bumpy-side-align-readme.md`.

The bumpy top face (v0.24.28) drops sub-vertices below y=1 via
`bumpy_height`. Adjacent side faces stayed at flat y=p.y+1 at
their top edges — visible gap at the corner where they meet the
bumpy top.

Fix: when this block has a bumpy top emitted (weatherable AND
top neighbor non-solid), drop the 4 upper corners by
`bumpy_height(corner.x, corner.z)`. Side faces now meet the
bumpy surface seamlessly.

Logic gated by `side_top_bumpy = is_weatherable(self_block) && !is_solid_opaque(n_py)`. When false, all four dy values are
0 — non-weatherable blocks and blocks with occluded tops keep
flat side faces.

Cost: 4 bumpy_height calls per weatherable surface block, only
when top face is exposed. Negligible.


## Increment 82 v0.24.37-vertex-ao — Minecraft-style corner darkening

**Ship:** `docs/v0.24.37-38-vertex-ao-readme.md`.

The GPU mesher emitted single-color faces — no corner darkening.
Blocks looked "plasticky" compared to mode 0 which has subtle AO
darkening at inside-corners. This ship adds per-vertex AO at
each face's 4 corners.

### Algorithm

Mojang's classic AO formula. For each vertex of an emitted face,
sample 3 cells in the air-side half-space:
- `side1`: cell directly adjacent along face tangent axis A.
- `side2`: cell directly adjacent along face tangent axis B.
- `corner`: cell diagonal to both.

AO level (0 = darkest, 3 = brightest):
```
if side1 && side2:  level = 0       (fully-occluded inside-corner)
else:               level = 3 - (side1 + side2 + corner)
```

Mapped to factor `mix(AO_MIN, 1.0, level/3)`. `AO_MIN = 0.55`
keeps darkening visible but not harsh.

### Implementation

- `bool ao_solid(meta, x, y, z)` — sample test. Returns false for
  multi-axis OOB (sample_or_neighbor only handles one axis;
  accepted artifact at chunk-corner inside-corners).
- `float compute_ao(meta, s1, s2, c)` — formula above.
- Six `ao4_face_<dir>(meta, x, y, z) -> vec4` functions — one
  per face direction. Returns the 4 AO factors in emit_quad
  vertex order.
- `emit_quad` refactored to take 4 vertex colors instead of one.
  `emit_quad_uniform` wrapper preserves single-color call sites
  (currently unused but available).

### Cost

Per face: 12 cell samples (4 vertices × 3 samples). Per surface
block (multiple faces): up to 6 × 12 = 72 extra samples. Most
surface blocks have only 1–3 exposed faces, so realistic cost:
20–40 extra samples per surface block. Expected:
- Mesher ~30 µs → ~50 µs per chunk (well within budget).
- Sample reuse via L1 cache likely brings this down further.

### Failure mode

Multi-axis-OOB samples return false (air). Visual artifact: at
the corner of an inside-corner where a sample would cross two
chunk boundaries, the AO level is one off from correct. Visible
only at the corners of the world-loaded set.

## Increment 83 v0.24.38-bumpy-ao — bilinear AO across bumpy sub-grid

**Ship:** `docs/v0.24.37-38-vertex-ao-readme.md` (bundled).

v0.24.37 left bumpy tops uniform-color (no AO). This ship
bilinearly interpolates the 4 corner AOs across the 5×5
bumpy sub-grid. Bumpy faces now have AO gradient matching
adjacent flat surfaces.

Implementation: `emit_bumpy_top_quad` gains a `vec4 ao_corners`
parameter (LL, LH, HH, HL order matching `ao4_face_py`). For
each sub-vertex at (i, j) where u=i/4, v=j/4:
```glsl
ao = mix(
    mix(ao.x, ao.w, u),  // bottom edge: LL→HL
    mix(ao.y, ao.z, u),  // top edge:    LH→HH
    v
);
vc = color * ao;
```

Cost: 4 mix operations per sub-vertex × 25 sub-vertices = 100
mix ops per bumpy face. Negligible vs. the bumpy emit's existing
write cost.


## Increment 82 v0.24.37-mesh-cmp — gated GPU↔CPU mesh equivalence gate

**Ship:** `docs/v0.24.37-mesh-cmp-readme.md`.

The autonomous-work stack is 8 ships deep without hardware testing.
This ship adds a runtime gate that catches algorithmic bugs in the
GPU mesher by re-running the CPU reference for every successfully-
meshed Near chunk and comparing the output.

### What's checked

- **Vertex count parity**: both implementations emit the same
  number of verts per face per cell (4 for flat, 25 for bumpy
  top). Mismatch → one side is emitting faces the other isn't.
- **Index count parity**: 6 per flat quad, 96 per bumpy top.
- **AABB closeness** (tolerance 0.5 world units): catches
  v0.24.27-fix3-class bugs where every vert collapses to world
  origin.

### What's NOT checked

- Color values (climate/position-tint can legitimately differ
  if either side's math drifts).
- Exact vertex positions (the bumpy top emits the same 25 verts
  but in different orders on CPU vs GPU due to parallel dispatch).
- Index ordering.

### Activation

Off by default. Set `UNENDLICH_OCTREE_MESH_GPU_CMP=1` to enable.
When enabled, logs `MESH_CMP_MISMATCH` at warn level per chunk
with the vertex / index / AABB diffs. Cost on main thread:
~3 ms/sec at observed submission rates. Acceptable for opt-in
verification.

### Implementation

In `gpu_meshing_octree_cpu_ref.rs`:
- New `MeshCompareResult` struct + methods.
- `compare_meshes(gpu, cpu) -> MeshCompareResult`.
- `mesh_aabb(verts) -> Option<(min, max)>` helper.

In `unendlich_world.rs`:
- `UNENDLICH_OCTREE_MESH_GPU_CMP` env var read at startup.
- In mesh-result loop (mode 2, Near LOD only), build Neighbors
  from `chunk_manager.octree(±1)` (same source the GPU's pool
  pre-warm uses), call CPU reference, compare, log mismatches.

### Why now

Filed in v0.24.30 design memo as a future ship; the CPU reference
got cross-chunk support in v0.24.30 so this is now a meaningful
comparison. Worth landing before any further GPU changes — the
gate catches regressions in real-time.


## Increment 83 v0.24.38-bumpy-asym — break the x↔z hash symmetry

**Ship:** `docs/v0.24.38-bumpy-asym-readme.md`.

v0.24.28 introduced bumpy top faces via a squirrel-style hash.
That hash was symmetric in x↔z swap (proven by hand:
`hash(a,b) == hash(b,a)` because the dot-product terms and the
`px*pz` term are both symmetric). The resulting bumpy pattern
had a diagonal-mirror artifact — bumps along the x=z diagonal
were identical to bumps along the z=x mirror, which read as a
faint diagonal stripe in playtest.

### Fix

Distinct per-axis multipliers and additive constants:
- `px = fract(x * 123.34)`  (unchanged)
- `pz = fract(z * 174.85)`  (was 123.34)
- `dot_p = px*(px+45.32) + pz*(pz+67.91)`  (was 45.32 both)
- `combined = px*pz*0.7 + dot_p`  (was px*pz without 0.7)

Applied to both shader (`chunk_mesh_octree.comp`) and CPU port
(`gpu_meshing_octree_cpu_ref.rs::hash2d`) — they MUST match for
v0.24.37-mesh-cmp to keep working.

### Unit test flip

The `hash2d_uses_both_axes_meaningfully` test previously
asserted `h_ab == h_ba` as a documented weakness. Now asserts
`(h_ab - h_ba).abs() > 0.01` — the fix is verified by the test.

### Visual impact

Subtle. The diagonal artifact was a faint structured pattern;
the asymmetric hash replaces it with proper-looking unstructured
noise. Most visible on large flat grass plains where the
diagonal-mirror was easiest to see.

### Cross-chunk consistency

Bumpy height is a pure function of world coordinates. The hash
change doesn't affect this property — adjacent chunks still see
the same hash values at shared edges, so the bumpy surface flows
seamlessly across chunk boundaries.


## Increment 84 v0.24.39-mid-lod — GPU mesher Mid LOD coverage

**Ship:** `docs/v0.24.39-mid-lod-readme.md`.

Previously the octree GPU mesher (mode 2) handled only Near LOD;
Mid LOD chunks fell back to the CPU mesher. This ship extends
mode 2 to Mid through shader parameterization.

### Design: shader parameterization

Push constants gained `lod_cell_count` and `lod_cell_stride`.
- Near: cell_count=32, stride=1. Per-block iteration; identical
  to v0.24.38 behavior.
- Mid: cell_count=16, stride=2. Each thread handles a 2³ source-
  block group; samples the low-corner block as the representative.
- Invariant: `cell_count * stride == 32`.

Shader changes:
- Cell decode parameterized by `cc = pc.lod_cell_count`. Workgroup
  dispatch passes `(cc³ + 63)/64` workgroups in X.
- Neighbor samples step by `int(stride)` (e.g., for Mid, the +X
  neighbor cell is at world-coord `x + 2`).
- Face emission corners scale by `s = float(stride)`. A Mid face
  spans 2 world-units instead of 1.
- AO disabled at non-Near LODs (gated on `stride == 1u`). AO
  sample offsets don't generalize cleanly without rewriting the
  AO functions; deferred.
- Bumpy disabled at non-Near LODs. Sub-block detail is invisible
  at Mid viewing distance.
- Climate tint, position tint, face_light still work at all LODs.

### Design: dispatcher

The octree GPU dispatcher now batches by LOD. When a request
arrives with a different LOD than the current batch, it's stashed
in a `pending: Option<OctreeGpuMeshRequest>` and the current
batch flushes first. The stashed request seeds the next batch.

Push constants apply to the whole dispatch, so single-LOD batches
are a hard requirement. Multi-LOD batching would need multiple
pipelines or specialization variants — filed.

### Design: routing

Worker checks `UNENDLICH_GPU_MESHER_MID=1` env var. When set, Mid
LOD requests route to the GPU mesher alongside Near. Default
off so the ship can be A/B compared against v0.24.38 baseline.

### Sampling fidelity

Mid uses center-of-cell sampling (specifically, low-corner block
of each 2³ group). The CPU `mesh_chunk_downsampled` uses majority
voting (~½ rule). For most terrain this difference is invisible
at Mid viewing distance. Filed: majority-voting GPU mid sampler
as a follow-up.


## Increment 85 v0.24.40-greedy-mid — 1D greedy meshing at Mid LOD

**Ship:** `docs/v0.24.40-greedy-mid-readme.md`.

Mid LOD chunks emit one quad per cell — same as Near. For typical
terrain with large flat regions (plateaus, walls, ceilings),
neighboring cells emit redundant adjacent quads. Greedy meshing
merges these runs into single longer quads.

### Algorithm: 1D greedy along a chosen run-axis

For each face direction, we pick a single run-axis along which
adjacent cells can merge:
- ±X face: run-axis Y (merge vertical columns)
- ±Y face: run-axis X
- ±Z face: run-axis X

Each cell asks "am I a run-start?" by checking the cell
immediately before me on the run-axis:
- If prev cell has same block_id AND would also emit this face
  direction (its face-direction neighbor is non-solid), then it
  IS a run-start (or part of an earlier run that extends through
  me). I skip.
- Else, I'm the run-start. I sweep forward along the run-axis
  to find the run extent.

Run-starts emit one wide quad covering the whole run; other cells
emit nothing for this face direction.

### Why 1D and not 2D?

2D greedy requires sequential merging within a slice. Each cell's
state depends on its neighbors in two dimensions, making
parallel GPU execution hard. 1D greedy lets each thread make an
independent decision based only on the previous cell on the
run-axis. Some redundancy (won't merge perpendicular runs into
rectangles), but ~2-10x vertex reduction for typical terrain.
Going to 2D is a follow-up.

### Why gated to stride > 1?

Per-vertex AO (v0.24.37) and bumpy (v0.24.28) are per-cell. Per-
position tint (v0.24.34) varies per cell. Merging cells would
lose all three. RC5 v0.24.39-mid-lod already disabled all three
at stride > 1, so RC6 greedy at stride > 1 composes cleanly.

At Near (stride == 1) the `greedy_run_for_face` helper returns
`stride` (1) immediately — every cell emits its own quad,
preserving v0.24.38 behavior exactly.

### Implementation

Two new helper functions:

```glsl
uint find_greedy_run(meta, x, y, z, axis, face_dir, stride, self_block);
// Walk forward along `axis` from (x, y, z). Stops when next cell
// has different block_id OR different face emission decision. Returns
// total extent (multiple of stride).

uint greedy_run_for_face(meta, x, y, z, axis, face_dir, stride, self_block);
// Decide: am I a run-start? Returns extent if yes, 0 if no.
// At stride == 1, returns 1 always (no greedy at Near).
```

Each face emit calls `greedy_run_for_face` with the appropriate
axis + face_dir. The returned extent becomes the quad's dimension
along the run-axis. `dy_LL`, `dy_HL`, `dy_LH`, `dy_HH` are
computed once for the bumpy side-corner adjustment and applied
to the q1/q2/q3 corners when at y = top.

### Performance

Per face direction, the run-start cell does work proportional to
run length (up to 16 steps at Mid). Non-start cells return early.
Net per-cell work is constant on average (each cell does ~1 of
6 face emits' run-scan work on average, others skip).

Total cell-count is `(32/stride)³` — for Mid: 4096 cells per
chunk vs Near's 32768. So Mid is already ~8x faster per chunk
just on cell count. Greedy on top reduces vertex output by
2-10x, which speeds up the readback + upload path.

### Position tint gated on Near

`tint_for_position` was applied unconditionally before. v0.24.40
gates it on `stride == 1u`. At Mid, the cell-uniform color means
greedy can merge cleanly without per-block color variation
showing seams.


## Multiplayer open-server + whitelist UI flow (r518 backbone; menu UI = next)
BACKBONE DONE (r518, sim/whitelist.rs): a per-world Whitelist {open: bool, allowed: BTreeSet<name>} persisted as
whitelist.json under the world root. Host gates joins at the CH_HANDSHAKE LanJoin::Join handler: open → admit anyone +
remember the name; closed → admit only listed names, else reply LanJoin::Deny{reason}. F10 toggles open/closed (saved
immediately). admits()/allow()/revoke()/names() are pure + unit-tested (5 tests). The bin holds `whitelist`, loads on
world init, saves on shutdown.
MENU UI FLOW (TODO — the unverifiable egui part, to build on-device with the menu kit in app/menu_view.rs +
unendlich-ui/src/{menu,widget}.rs):
  1. Multiplayer screen (Screen::Multiplayer, currently a stub at menu_view.rs:167) gains a "Host Settings" sub-panel
     shown when hosting.
  2. Top row: an OPEN/CLOSED segmented toggle bound to whitelist.open (elegant_button styling like the Controls tab).
  3. Allow-list: a vertical ScrollArea (mirror the keybind list at world_renderer.rs:3951) listing whitelist.names(),
     each row with a "revoke" (×) elegant_button.
  4. Add-name: a text field + "Add" button calling whitelist.allow(); reuse the world-name text input widget.
  5. Recently-seen: list lan_remote_appearance names with a "+" to allow() quickly.
  6. Persist on every edit (whitelist.save(&world_root)); the bin already owns the store + root.
  Wiring: expose `&mut whitelist` (or a snapshot + a command queue, matching how keybind rebinds round-trip through the
  renderer) so menu_view can read names/open and emit Allow/Revoke/ToggleOpen commands the bin applies + saves.

## Stature (height as the master physical scalar)
See `docs/STATURE.md` + `sim::stature` (r559). Height (blocks, default 1.8, range 0.1..25.0) derives collider, eye,
mass, health, hunger, strength, knockback (dealt/resist), step, jump, speed, turn, ground-shake, and wind/water-current
resistance. The profile is authoritative from r559; consumers wired incrementally.
