# BACKLOG_INBOX — operator thought-stream, parsed (2026-06-04)

Raw musings captured and decomposed so they're concrete and you don't have to
hold them in your head. Each line: **[AREA]** idea → *core* (sandbox-buildable
now) vs *wiring* (your machine) vs *design* (needs a decision first), plus deps.
The coordinator folds these into `BACKLOG.md` as rounds are scheduled.

This stream adds a **new area, NET (multiplayer)**, and two cross-cutting
concerns, **GPU-RUNTIME** and **DEVICE-TIERING**, noted in `OWNERSHIP.md`.

## SIM — creatures, items, body

- **Dragon genetics.** *core:* trait-set + `signature_from(genome, element, brain)`
  (reuse the `ItemGenes` pattern) — pure, buildable now. *wiring:* anatomy-seeded
  emission origins; render consumer (ATMO). Dep: existing `emission`, `genetics`.
- **Evolving items.** *core:* `item_genetics` (genome, `mutate`, `breed`,
  expression → stats) — pure. *wiring:* apply genome to item entities + the
  sentient-blade brain (SIM/MAIN). Dep: `sim/item_entity`.
- **Mob ↔ environment deformation.** *core:* collision-aware mesh deform (terrain
  contact → squash/bend response), composes with `deform`/`flex_bend`/`foot_ik`
  (round 1) and `ragdoll` — pure. *wiring:* entity vertex pass (ATMO).
- **More creatures.** Bee / songbird / monkeys already on `BACKLOG.md`; add kinds
  to `creature_design` (MAIN spec), ecology cores in WORLD.

## WORLD — worldgen, structures, dimensions, perf scaling

- **Mountains via the physics system.** *core:* an uplift/orogeny field (pure
  tectonic/erosion model producing elevation deltas), + a pure structure-placement
  policy. *wiring:* feed into `cpu_generator`/octree gen (WORLD+MAIN). *design:*
  how "through physics" composes with the existing voxel gen. Big; start with the
  pure uplift core.
- **Structures.** *core:* deterministic structure templates + placement scoring
  (pure). *wiring:* stamp into the world-edit/gen path.
- **Dimensions + portals (groundwork).** *design first:* a world-instance
  abstraction (multiple independent voxel worlds) + portal link records. *core:*
  the dimension/portal data model + linking logic (pure). *wiring:* streaming +
  render per-instance (later, large).
- **Device tiering (M2 Ultra ↔ Fairphone 5).** *core:* a pure
  `capability → budgets` policy (render distance, octree pool MB, LOD
  aggressiveness, GPU dispatch budgets, tick targets) keyed off detected RAM / GPU
  cores. Pure, buildable now, and high-leverage. Ties to `lod_upload`,
  `recompaction` (round 1), `exec_tier`, `STAGE_*_DURATION_TICKS`.

## ATMO/RENDER — shaders, cameras, mob rendering, textures

- **Mobs in the shader pass.** Today mobs render "on top." *wiring:* fold creature
  geometry into the world/chunk pass so they're depth-correct and lit. *core:* a
  mob instance-buffer packer (pure data layout). Integration-heavy (your machine).
- **Splitscreen multi-cam.** *core:* a pure layout solver — N players + screen dims
  → viewport rects with dynamic spacing / tiling (and orientation). Pure, buildable
  now. *wiring:* N camera passes + per-view UBOs (ATMO, integration).
- **Photorealistic shaders.** *core:* small data cores (tonemap curve tables, BRDF
  LUT generation, fog/scatter constants). *wiring/design:* PBR lighting, the look
  pass — mostly shader work on your machine. Dep: the fog port kit already staged.
- **Textures.** Mostly content + a pure atlas-packing policy core; ongoing.

## NET — multiplayer (NEW AREA)

- **Rollback netcode.** *core:* a deterministic rollback core — ring of input
  frames, state save/restore, prediction + resimulation on correction (GGPO-style).
  This *needs* the sim to be deterministic, which the pure-module discipline already
  enforces (tick `dt` + seeded noise, no wall-clock/RNG) — so it's a natural fit and
  largely sandbox-testable against a toy deterministic sim. *wiring:* hook real sim
  state snapshot/restore (MAIN), the slowest/biggest integration.
- **DHT routing + hole punching.** Transport for the above. **Reference exists:**
  your working Qt/C++ app (`v1-r3` tarball — SnapClone + Jami/`camera-bridge`,
  OpenDHT + ICE). *core:* a pure session/peer-id + connection-state model. *wiring:*
  the actual DHT + NAT traversal transport (port/adapt the proven approach). *design:*
  how peer ids map to game sessions; authority model with rollback.

## GPU-RUNTIME — fast dispatch, 500 Hz, stutter reduction (cross-cutting)

- **GPU slicing + subscriber coordinators.** Abstraction layers to partition GPU
  work, with coordinators piping data to thread subscribers over a fast channel
  (kanal) to cut stutters and target ~500 Hz. *core (buildable now):* the pure
  scheduling/pacing policy — per-frame dispatch budget allocation across slices, a
  frame-pacing controller targeting a tick rate, backpressure rules. *wiring:* the
  actual kanal plumbing + GPU submission (your machine, integration). Composes with
  device tiering (budgets per tier) and `exec_tier`.

---

### Suggested next rounds (coordinator proposal — operator confirms priority)

These are the **pure cores buildable now**, disjoint by area, ready to become
worker briefs:

- **NET:** rollback core (against a toy deterministic sim) + peer/session-id model.
- **WORLD:** device-tiering `capability→budgets` policy + uplift/orogeny field.
- **ATMO:** splitscreen layout solver + a mob instance-buffer packer.
- **SIM:** `item_genetics` + dragon `signature_from` + collision-aware deform core.

Everything else (mobs-in-pass, PBR look, dimensions/portals, the kanal plumbing,
DHT transport) is wiring/design that lands on your machine via coordinator kits,
not parallel worker chats.

---

# Stream 2 (2026-06-04) — parsed

## NET / AUDIO (new)
- **In-game voice chat.** *wiring/transport:* over the same DHT/hole-punch path as
  rollback (reference: `v1-r3` Jami app). *core:* a pure voice packet/jitter-buffer
  + Opus frame sequencing model (codec via **`audiopus`** — proven in your other
  projects). NET area.
- **Networked physics + reverb spatial-audio engine.** For multiplayer **over the
  network — explicitly NOT local splitscreen.** *core (buildable):* pure spatial
  audio math — distance attenuation, occlusion, early-reflection/reverb send levels
  from geometry, source mixing. *wiring:* the actual audio device + network sync
  (your machine). New **AUDIO** concern (shares NET for the networked part).
- **Asset:** intro soundtrack `koYopper.opus` (stereo, 135 BPM, ~2.5 min) → lives
  under `assets/audio/`. More tracks to come. Audio stack = Opus/`audiopus`.

## SIM — creature cognition (new)
- **Long-term mob memory.** *Being built in ROUND 2* (`sim/mob_memory.rs`): bounded
  store of places/events with long-horizon decay + recall.
- **Dragon landmark memory.** Dragons accrue knowledge of landmarks they fly over —
  *extends* `mob_memory` (feed flight path → remembered landmarks). Core, after R2.
- **Dragon language (linked on hatch/train).** *design first:* a heritable/learnable
  symbol system a dragon develops; "linking" it to the player on hatch + training.
  Meaty — needs a design pass before a core (how symbols form deterministically, how
  linking maps to player commands). Groundwork only for now.

## Speculative (your call — you leaned "probably not")
- **Face-tracking → character faces** (reuse SnapClone camera face tracking to drive
  character faces). Logged, parked; not scheduled.

---

# Round assignments + warmed queue

**ROUND 1 — DONE** (merged, GREEN): flex_bend, foot_ik, strafe_face,
MoistureField::recenter, climate::temp_for, octree_compact, recompaction,
competition, lod_upload.

**ROUND 2 — OUT NOW** (3 threads, from the r1-merged base):
- T01/SIM: `item_genetics`, `dragon_signature`, `mob_memory` (+opt `env_deform`).
- T02/ATMO: `viewport_layout`, `mob_instances` (+opt `tonemap_lut`).
- T03/WORLD: `device_tier`, `orogeny` (+opt `structure_place`).

**ROUND 3 — WARMED** (independent pure cores, ready to cut into briefs when R2
returns; reprioritize freely):
- T01/SIM: dragon **landmark-memory** binder (atop `mob_memory`); item-evolution
  **breeding loop** policy; a pure **neck/tail + env-deform** combiner.
- T02/ATMO: **per-view culling** policy for splitscreen; an **instance LOD** selector
  (atop `mob_instances`); pure **scatter/fog look** constants.
- T03/WORLD: **dimension/portal** data model (groundwork); **erosion pass 2** for
  `orogeny`; **structure templates** library.
- **NET (if you spin up a 4th thread):** deterministic **rollback core** vs a toy
  sim; **peer/session-id** model; **voice jitter-buffer** + Opus frame sequencing.

---

# Stream 3 (2026-06-04) — parsed (ocean / menu / moving platforms)

## MENU / FRONT-END (new binary — keep `unendlich-world` untouched)
- **`unendlich-menu` binary** — menu + world-creation screen + multiplayer setup,
  reusing the existing GUI theme. *core:* pure menu/world-creation state model (+
  the `viewport_layout` core). *wiring:* the binary itself + GUI (MAIN, your tree).
- **Blur behind transparent menus.** *core:* pure blur-kernel/weights generator
  (e.g. Kawase/Gaussian taps). *wiring:* the blur pass (ATMO shader, your tree).

## OCEAN / AQUATIC (big WORLD + SIM theme; fixes "grass grows underground")
- **Aquatic materials (sand, gravel).** *core (now):* depth/slope-aware material
  classifier (WORLD). Today there's no sand/gravel differentiation.
- **Ocean + trenches + bathymetry.** *core (now):* extend round-2 `orogeny` below
  sea level — uplift/erosion → trenches and ocean-floor geography (WORLD).
- **Sea level + ocean fill.** *core (now):* a pure sea-level/fill policy (WORLD).
- **Corals = plants-with-genes.** *core (now):* reuse `plant_genetics` for aquatic
  flora + a biome/depth placement rule so flora is gated by biome (WORLD).
- **Fish schools + giant turtles.** *core (now):* a boids/schooling core atop the
  existing `neighbor_grid` broadphase; a turtle behavior policy (SIM).
- **Swimming mechanics.** *core (now):* 3D aquatic locomotion — buoyancy + drag +
  3D heading (SIM). (This is the long-standing "swim/fly 3D pathing" item.)

## MOVING PLATFORMS (boats, dragon-backs)
- **Attachment transform core.** *core (now):* the general "things riding an entity
  stay fixed in its local frame" math (local↔world transform composition) — the
  shared substrate for boats AND a mini-base on a dragon's back (SIM). Pure +
  property-testable.
- **Boats.** buildable structure → entity (uses the attachment core + a buoyancy
  core). *wiring:* structure→entity conversion (SIM/WORLD).
- **Dragon-back mini-base.** structures attached to a large moving creature (uses
  the same attachment core). *design:* interaction/anchoring rules.

## Round-3 queue — UPDATED with stream-3 (pure cores, ready to brief)
- **T01/SIM:** fish-schooling/boids core · swim-locomotion core · moving-platform
  attachment-transform core. (dragon landmark-memory binder still queued.)
- **T02/ATMO:** blur-kernel generator · menu/world-creation layout core ·
  per-view culling for splitscreen.
- **T03/WORLD:** aquatic material classifier · bathymetry/trench extension of
  `orogeny` · coral genome + biome-gated flora placement.
- **NET (4th thread, if spun up):** rollback core · peer/session-id · voice
  jitter-buffer.

---

# Stream 4 (2026-06-04) — parsed (big-mob pathing, survival, food, footprints)

## SIM — big-mob terrain interaction & learning
- **Dragon ↔ terrain fit/bend/break.** *core (now):* `sim/squeeze.rs` — given the
  mob's size-aware bounding (from `body_scale`) + local block solidity → `Pass /
  MustBend(angle) / Stuck`. *core (now):* `sim/block_yield.rs` —
  `breakable(block_strength, body_size, strength_gene, element) -> Option<BreakFx>`,
  where `element` can make the block do special things (fire→melt, etc.). Stuck →
  damage. *wiring:* apply break + damage on the world-edit/entity path.
- **PROPOSAL — pain/success pathing learner** (`sim/path_memory.rs`, pure, the thing
  you asked me to design): the mob keeps a small **ranking over heading bins** (e.g. 16
  directions, optionally keyed by a coarse local-context hash) plus an **estimated
  clearance radius**. Each tick it tries a heading:
    - moved freely → **reward** that bin (score up, others decay);
    - blocked / didn't fit → **penalize** that bin, raise the clearance estimate
      (it learns it's bigger than it thought here), and emit a **back-up + re-survey**
      action instead of grinding forward.
  Heading choice = goal-ward bias × bin score, avoiding low-score bins. Over k blocked
  attempts a heading drops out of contention, so it stops retrying what doesn't fit and
  learns routes that do. Fully pure + deterministic (no RNG / seeded), so it's a clean
  worker core. *Props:* a repeatedly-blocked heading's score falls monotonically below a
  free one; clearance grows on stuck and is bounded; goal bias preserved when
  unobstructed; converges (after k blocks that heading isn't chosen). Composes with
  `mob_memory` (round 2) and the squeeze/break cores above.
- **BUG — big dragons tilt / "come off the world."** Likely size-unaware grounding:
  lean/contact computed at body center, not over the footprint. *core (now):*
  `sim/large_body_grounding.rs` — sample terrain under several footprint points, fit a
  contact plane, clamp lean magnitude by size. *Props:* flat ground ⇒ zero lean; lean
  bounded; contact height = max over footprint samples (no floating). *wiring:* feed the
  entity transform (replaces the current single-point lean for large bodies).
- **Footprints + weathering.** *core (now):* `sim/footprints.rs` — ring buffer of marks
  (pos, age, depth∝size, kind) with weathering decay (age + weather → fade), bigger mobs
  leave deeper/longer-lasting prints. Pure. *wiring:* decal render (ATMO).

## SIM/WORLD — food & gentle survival
- **Edible tree genetics.** *core (now):* `sim/food_value.rs` — plant/tree genome →
  food yield + nutrition (extends the plant-genetics line; build standalone). *wiring:*
  fruit drops → item; eating restores survival stats.
- **Gentle survival stats** (food / water / temperature). *core (now):*
  `sim/survival.rs` — slow decay tuned to ~15 min game-time per meaningful drop, mild
  non-fight-altering penalties, **PvP-safe floor** (never reduces combat-relevant stats
  below a clamp). Pure + deterministic. *Props:* decay rate within the 15-min target;
  penalties bounded; in PvP mode combat stats never drop below the floor; temperature
  ties to `climate::temp_for` (round 2).

## Round-4 queue (pure cores, ready to brief — many answer this stream)
- **T01/SIM:** `path_memory` (the learner) · `squeeze` + `block_yield` (dragon-terrain) ·
  `large_body_grounding` (the tilt bug). (footprints/food/survival queued next.)
- **T02/ATMO:** decal/footprint render params core · the mobs-in-shader-pass instance
  groundwork (continues `mob_instances`) · PBR/look constants.
- **T03/WORLD:** coral genome (aquatic flora genes) · structure templates · the
  dimension/portal data model.

---

# Stream 5 (2026-06-04) — parsed (GPU abstraction, winding bug, perception/stealth)

## GPU abstraction layers (cross-cutting: GPU-RUNTIME — mostly integration-tier)
- **Span-allocator property tests.** ✅ **shipped as round-4 `world/span_alloc.rs`** —
  pure best-fit allocator + free-coalescing + invariants under random alloc/free.
- **`render/validation.rs` — typed Vulkan guards.** Wrap descriptor lifetimes / buffer
  usage-flag consistency / image-layout transitions in Rust types so misuse is a compile
  error, not a validation-layer crash. *Integration* (imports ash). Codifies the patterns
  the RULESET already enforces by convention (R3/R9/R16). Design first.
- **SPIR-V reflection test pattern.** Load each `.spv`, reflect (spirv-reflect), assert the
  descriptor layout matches what the Rust pipeline binds. Catches "changed a binding number
  in GLSL, forgot the Rust side" — the R16 class, but for full descriptor sets, not just the
  UBO. *Harness extension* (needs the crate + built shaders → integration tier; the sandbox
  R16 parity check is the pure stand-in).
- **★ Typed pipeline builder (the one you're most excited about).** A builder whose *type*
  encodes which descriptor sets / push-constants a pipeline expects, so bind sites must
  provide matching types — "build components without losing perf, iterate like crazy, add
  world-driven dynamics, keep reproducibility." Real work (~weeks). *Phased path:* (1) a
  typed descriptor-set-layout registry (the R9 free-slot map as types); (2) a builder that
  validates set/push-constant compatibility at construction (runtime first); (3) lift the
  checks into the type system (phantom types per layout) so mismatches don't compile. Pure
  layers (1)+(the checking logic) can be sandbox-tested; the ash binding is integration.
  This is the natural home for "balance dynamism vs the engine's determinism needs."

## Rendering bugs
- **BUG — winding / nonsolid faces.** Must set winding_variant=2 (or 5) at start or terrain
  has holes / missing faces; but 2/5 then *boxes in* nonsolids (leaves get sealed faces).
  Two coupled faults: (a) the **default winding order is wrong** → make the correct variant
  the default (R4 octree-winding guard territory); (b) face-culling treats **nonsolid
  neighbours as occluders** → a nonsolid (leaf/cutout) should emit a face against air and
  against *different* materials, but not seal against the same nonsolid, and shouldn't be
  treated as solid by its neighbours. Fix (a) and (b) separately. *Integration*
  (octree.rs / mesher + the R4 winding check); sandbox covers winding order only.
- **Fog still not visible (acknowledged).** FOG_PORT is staged but not applied — correct,
  it waits for a render-integration pass. `handoff/fog_port/*` + chunk.frag.spv regen (R2)
  are ready when we get there.

## SIM — perception, stealth, blind creatures
- **Touch/feel sensors.** ✅ **shipped as round-4 `sim/touch_rays.rs`** (the "feeling is the
  first step of pain" substrate; feeds path_memory; gives blind creatures + big-dragon blind
  spots a way to gauge the world).
- **Dragon perception + player stealth.** *core (proposable):* `sim/perception.rs` — an
  awareness model from noise + line-of-sight + a **size-based blind-spot cone** (big dragons
  literally can't see what's beneath/behind them). Gameplay: you *hear* a dragon coming →
  crouch under cover → it walks through, feeling with touch_rays but not seeing you. Pure +
  testable (awareness rises with noise/LOS, falls in blind spots; deterministic).
- **Blind cave dragon (genetics).** A dragon archetype with sight gene disabled, navigating
  by touch_rays + sound (perception) — i.e. echolocation-ish. A genetics flag on the dragon
  genome (ties to `dragon_signature`); scary by design. *core:* small gene add + lean on
  touch_rays/perception.
- **Footprints mimic body-part impact shape.** ✅ **shipped as round-4 `footprint_decal.rs`**
  (impact outline by body part + weathering).
- **Dragon silhouette variation (blocky → distinct).** ✅ **delivered: `dragon_directions.html`**
  — 10 procedural side-profile directions to react to; the chosen 2-3 become the
  `creature/body` parameter set.

---

# Stream 6 (2026-06-04) — parsed (volumetric weather, voxel water, GPU nav-fields, brain)

## ATMO — volumetric clouds, storms, lightning, tornados
Proposed pipeline (keeps it on the GPU as fields, CPU only steers it cheaply):
- **Coarse CPU weather sim → GPU field → shader visuals.** A low-res grid of **storm
  cells** (humidity/charge/updraft) advected by wind on the CPU each slow tick; uploaded
  as a 3D density+charge field; the existing **froxel volume** raymarches it for cloud
  shading (you already pay froxel cost — ~0.2 ms p50 — so volumetric cloud is an extension,
  not a new pass). *core (now):* `storm_field` — the cell update (humidity→condensation→
  precip, charge build, dissipation). Pure + deterministic. *wiring:* upload the field +
  cloud raymarch in the froxel shader.
- **Lightning.** *core (now):* `lightning_path` — procedural bolt geometry from a charged
  cell: a branching random-walk to ground (cloud-to-ground) and a variant that stays in the
  cloud layer ("the ones that run around in the clouds"). Pure (seeded), returns a segment
  tree. *wiring:* a thin emissive line pass + a light flash; trigger when a cell's charge
  crosses threshold.
- **Tornados (rare).** *core:* `tornado_vortex` — a vortex velocity field spawned rarely
  from a strong cell; feeds debris/particle advection. Pure. Gate spawn-rate hard.

## WORLD — voxel water physics + freezing
- **Flowing gravity water as a cellular automaton.** *core (now):* `water_cell` — one CA
  step: water moves to lower then equal-level neighbours, conserves mass except at sources/
  sinks, settles to a flat pool. **Freezing**: cells freeze/thaw by `climate::temp_for`
  (round 2) below/above a threshold (ice = solid until thaw). Pure + deterministic;
  *props:* total water conserved off sources; monotone settling; freeze iff temp<thresh.
- **The high-source-flow problem (you flagged it).** Two options, your call:
  (a) **weather-fed (gameplay-friendly, slightly unrealistic):** rain from the storm
  system tops up exposed water + spring cells, so rivers stay fed without true infinite
  pressure — ties water to weather, self-regulating, no draining. *core:* `river_source`
  (rain-rate → source replenish, capped).
  (b) **realistic:** explicit infinite **source blocks** + a pressure/head term so high
  sources push downhill. Heavier to tune, can oscillate. — Recommendation: ship (a) for
  consistent gameplay flow; offer (b) as a worldgen option for "realistic" springs.

## SIM — big-mob traversal + GPU nav-fields (ties to the experience brain)
- **BUG — big dragons can't step large cliffs.** Step-height is baked for a fixed mob
  size. *core (now):* `step_height` — `max_step(body_size)` so a big dragon clears a
  4-block lip a small mob can't; feeds path_memory's clearance + large_body_grounding.
  *props:* monotone in size; bounded.
- **★ GPU nav-fields + brain API (your "GPU pathing pre-maps").** This is the pathing
  version of the brain guide's core rule (*you cannot microsim per frame*): compute the
  expensive thing **once, shared, deduplicated** (a flow-field / distance-field over
  walkable terrain toward goals, on the GPU), and let each mob do a **cheap reactive
  query** at frame rate. *core (now):* `flow_field` — the pure grid pathfinder (BFS/eikonal
  from goal cells over a walkability grid → per-cell flow direction); this is exactly what a
  compute shader would run, validated on CPU. *core (now):* `nav_query` — the reactive
  sampler a brain calls: field + mob pos/size → heading, blended with `path_memory`'s
  learned bins + `touch_rays` contacts. Functional, dedup (one field, many readers).
  *wiring:* lift `flow_field` into a compute pass; `nav_query` stays CPU/reactive.

## RENDER — cheap mesh-seam blending (you flagged disconnects)
- **Hide chunk-boundary cracks cheaply.** Three cheap options, in order of bang/effort:
  (1) **LOD skirts** — drop a thin vertical apron at chunk edges so cracks are occluded
  (cheapest, standard, no smoothing math); (2) **cross-seam normal averaging** — average
  normals across the boundary so lighting doesn't betray the seam; (3) **vertex-colour
  blend** — blend colours across the join to kill the hard line. *core (now):*
  `mesh_smooth` — the pure kernel for (2)+(3): given edge-vertex rings, produce
  averaged normals + blended colours (a one-ring Laplacian step, clamped). *wiring:* skirts
  + applying the kernel at seams live in the mesher. Note this interacts with the winding /
  nonsolid-face bug (stream 5) — fix winding first so there are fewer real holes to mask.

## Experience brain (guide folded into the base at docs/experience-brain/)
- The two-tier architecture (reactive FSM + rare budgeted experience tier, ablated) is now
  a base doc + a DECISIONS entry. Built cores already slot in: `mob_memory`=objectives,
  `touch_rays`→Signals, `path_memory`=reactive learner. **Next SIM piece = `perception`**
  (the "am I being looked at" + size blind-spot signal the guide names as the Freeze-beat
  linchpin; enables crouch-to-hide and the blind cave dragon). Stage-4 dragon elemental
  surrogate ties to `block_yield`'s element field.

## Done this stream
- **Dragon silhouette variety.** ✅ `dragon_directions.html` now 13 directions incl. a
  Game-of-Thrones wyvern (Drogon), a three-necked Hydra, and a heraldic gold-and-gules wyrm.

---

# Stream 7 (2026-06-04) — parsed (block edit bug, LOD gap/fidelity, integration start)

## RENDER/WORLD — block break/place broken (HIGH — fast + accurate)
Editing blocks is broken and must be fast + accurate. Plan (see
`handoff/LOD_SHADOW_ENTITY_KIT.md` §F):
- **Accurate pick:** a pure **`voxel_raycast`** core (Amanatides–Woo DDA) → exact hit voxel +
  hit face (place on the adjacent cell, break the hit cell). Pure, testable; queued as the
  next core. *props:* ray that hits returns the first solid along it; face is the entered
  one; misses return None; deterministic.
- **Fast apply:** local **synchronous** remesh of the edited chunk (+ boundary neighbour on
  an edge), not via the async queue, so the edit shows that frame.
- Likely root cause is wrong-face selection, a stale (un-remeshed) chunk, or the winding/
  nonsolid-face bug hiding the new faces — **fix winding first**, then raycast, then remesh.

## LOD — the chunk gap + fidelity (in the integration kit, §A)
- The dark seam in the screenshot = a LOD-transition crack. Fix = **LOD skirts** (thin edge
  aprons; cheapest) + `mesh_smooth` (R5) on the seam. Exact T-junction stitching is the
  expensive alternative.
- **Raise fidelity** via `LOD_BANDS` 12,24,48→16,32,64, a softer factor ramp 1,2,4,8→1,2,3,4,
  and `LOD_FIDELITY` 0.8→0.9 — but mesh fidelity costs raster, so prefer **Mid-as-runs**
  (SurfaceRun) which keeps per-voxel detail at low memory + fewer draws.

## RENDER — the integration you greenlit (now an active kit)
`handoff/LOD_SHADOW_ENTITY_KIT.md`: sequenced LOD/SurfaceRun far-bands → shadow pass
(`SHADOW_BUCKETS` + LOD-paired casters) → the shader-pairing table (LOD×pass→pipeline) →
**mob/entity/item instancing** (entity instance buffer fed by `large_body_grounding` for the
tilt/ground-clip fix, `mob_lod` for detail, `mob_instances`/genetics for the body) + footprint
decals. Grounded in the RD=24 numbers (main 101 ms + shadow 73 ms, memory fine).

## Done / delivered this stream
- **Dragon types** ✅ `dragon_directions.html` now **15** — added a reared, proud **Smaug**
  upright wyrm (long arched neck) and a sleek **Saphira/Eragon** four-legged sail-wing.

---

# Stream 8 (2026-06-04) — parsed (gameplay/UI systems, commands, inventory, ports)

## MAIN/bin — remove the sentinels (concrete)
The "lauscher" sentinel flock is `sim/sentinel.rs` (`Sentinel`/`Posture`), declared at
`sim/mod.rs:107`. In the **world-bin** (`unendlich-world` main): remove the flock spawn, the
per-tick `sentinel.tick()` update, and the `"Sentinel flock postures: …"` log
(`grep -rn 'Sentinel flock postures\|Sentinel::new\|sentinel' <world-bin>/src`). The
`sim/sentinel.rs` module can stay (harmless, unused) or be deleted. Note: when the experience
brain lands, the lauscher comes back via that path, not this hardcoded flock.

## GAMEPLAY — commands, inventory, crafting (pure cores, round 6)
- **`command_parse`** — string → typed command (`/find structure|mob|biome|water`,
  `/settime`, `/whitelist add|remove`, `/locate`). Commands can dispatch to actions OR open a
  GUI. Pure parser/grammar → enum. *props:* round-trips known commands, rejects/None on
  garbage, deterministic. *wiring:* the dispatch + GUI-open hooks + the find/locate search.
- **`inventory_grid`** — slot grid (rows×cols) for storage + player inventory: add / move /
  swap / stack / split, capacity + max-stack rules. Pure. *props:* item count conserved
  across moves; never exceeds max-stack; full grid rejects adds; deterministic.
- **`crafting`** — recipe match (shaped/shapeless over a grid) → result; ties to
  `item_genetics` (r2). Pure. *props:* a matching pattern yields its result, non-match yields
  none, deterministic. *(opt `recipe_book` = index/search recipes by ingredient.)*

## UI/CONFIG — settings, world wizard, servers, whitelist (pure models, round 6)
- **`settings_schema`** — typed settings model + validation/defaults/clamp (graphics, audio,
  controls). Pure. *wiring:* the settings page renders this; the GUI uses `menu_layout`(r3) +
  `blur_kernel`(r3). 
- **`world_params`** — world-creation wizard parameters (seed, size, biome mix, cave density,
  weather) + validation. Pure. *wiring:* the wizard UI + worldgen reads it.
- **`whitelist`** — ID set ops (add/remove/contains, parse a list, normalize). Pure. *props:*
  set semantics, idempotent add, parse round-trips. *(opt `server_connect` = address/port
  parse + validate + whitelist check.)*
- *wiring:* the server-connect / open-server / whitelist UIs are the `unendlich-menu` binary
  (separate-bin idea from stream 1) + these models.

## WORLD — voxel pick + rare giant caves (round 6)
- **`voxel_raycast`** — DDA (Amanatides–Woo) grid traversal → exact hit voxel + hit face;
  the accurate side of the block break/place fix (stream 7). Pure. *props:* first solid along
  the ray; entered face; misses → None; deterministic.
- **`giant_cave`** — rare giant cave-structure placement: a low-probability gate + a large
  cavern template (deterministic from seed). Ties to `structure_place`(r2)/`structure_template`
  (r4) and the cave system. Pure. *props:* spawn-rate within the rare bound; sites deterministic;
  cavern bounds clamped. *(opt `locate` = scan a sampled region for nearest matching
  structure/biome/mob, for `/find`.)*

## Parallel / downtime track (low priority — need GPU, not pure cores)
- **macOS shader porting.** Asahi/Mac target. MoltenVK (Vulkan-on-Metal) is the low-effort
  path with caveats; native Metal is a big port. A good parallel thread when there's downtime.
- **Fog experimentation.** `handoff/fog_port/*` is staged + `chunk.frag.spv` regen (R2) ready;
  apply during a render-integration pass and tune the froxel march.
- **Already in-tree (don't duplicate):** `sim/navfield_gpu.rs` + `world/pathing.rs` exist —
  `flow_field`/`nav_query` (r5) are the pure, CPU-validated pieces that should feed/validate
  those, and `step_height`(r5) feeds the pathing.

---

# Stream 9 (2026-06-04) — process + art direction + doc findings

## Art — the ideal dragon is bulkier (viewer v3)
Feedback: current shapes are too thin; the target is **fat / bulky / armored, shorter legs,
wider more widespread wings** — and "not any of these yet" (still searching). Done:
`dragon_directions.html` now has **live sliders** (Bulk / Legs / Wings / Armor) so the
proportions can be dialed in, defaults shifted bulkier, plus a **Juggernaut** archetype as an
anchor of that direction (16 total). When the right dial settings are found, those become the
`creature/body` gene ranges.

## Process — worker refill plan + doc-reading (now `WORKER_PIPELINE.md`)
Codified: the round cycle, a **read-the-docs-first** step (scan PERF-LEVERS / FOLLOWUPS /
TODO-BACKLOG / SHADER_ROADMAP + existing modules before queuing, to avoid duplicating what's
already in-tree), a prioritized **READY QUEUE**, and the rule that a round is only worth
cutting at ≥6 genuinely pure cores (else the work is integration-tier).

## Process — bonus micro-opt lane (the user's idea)
With many threads, hand out small batch/dedup tasks that one thread would never prioritize.
Queued: **`batch_eyes`** (batch `eyes::sees`), **`batch_perception`** (batch
`awareness::best_facing` + being-looked-at), **`batch_settle`** (batch posture/grounding),
and noise/climate-sample dedup. Hard rule: each must prove **bit-parity** with the scalar
version via a property test, so it can't change behaviour.

## Doc findings (from reading the in-tree docs — corrections to earlier advice)
- **Shadow lever correction.** `PERF-LEVERS.md` says the implemented cheap shadow wins are
  **`SHADOW_MAP_SIZE` 2048→1024 (4× fragment cut)** and `SHADOW_CHUNK_STRIDE` 2→3 — *not*
  `SHADOW_BUCKETS`, which appears to be an unimplemented/no-op flag. Use map-size + stride at
  RD=24 first (shadow is 58 ms).
- **Chunk-gap dual cause.** `FOLLOWUPS.md` documents a hole/seam mechanism: under
  `GPU_DECORATE=1`, a GPU-mesher OOM falls back to CPU greedy meshing on a CPU `Chunk` that
  may be empty (GPU-only generation) → empty mesh → seam at the LOD boundary. With your pool
  now at 2048 and `gpu_oom_fallback=0`, that path is quiet, so the **remaining** gap is the
  genuine LOD-transition crack → `lod_skirt` + `mesh_smooth`. (If gaps return when OOM>0, it's
  the empty-chunk fallback — fix per FOLLOWUPS: CPU-regen before fallback.)
- **Leaf-boxing = TODO #4.** The "winding boxes in leaves" bug is the `is_solid` →
  narrower-predicate split (`produces_face_against` etc.) in `TODO-BACKLOG.md` #4: a leaf
  shouldn't seal a face against another leaf. Queued the *pure* predicate core
  (`block_predicates`); the mesher/light call-site migration is integration (build-loop, not
  blind — TODO flags it high-risk).

---

# Stream 10 (2026-06-04) — glowing emissive, is_opaque, shader-reasoning, prewarm

## RENDER/SIM — glowing eyes (horror) + glowing necks (dragons)
Yes, this is cheap once entities go through the entity pass (kit §E). Emissive is nearly free:
a per-vertex / per-instance **emissive {color, intensity}** added in the fragment shader (no
extra lighting), and the **bloom** you already run (`UNENDLICH_BLOOM=1`) turns it into glow.
- **Do mobs touch shaders today?** Not properly yet — in the screenshot they render outside a
  real entity pass. The path is kit §E: an entity instance buffer → `entity.frag`. Emissive is
  an extra vertex attribute (eye/neck regions tagged) or a per-instance mask. **Efficient:**
  one instanced draw per archetype, emissive adds a constant add in the fragment, bloom is one
  pass you already pay — so glow scales with instances×parts (`mob_lod`), not per-pixel work.
- *core (queued):* **`emissive`** — pure `genetics/element → {color, intensity, region-mask}`
  (eyes for horror mobs, neck for a dragon's elemental). The shader sample + bloom tap is the
  integration half.

## WORLD — predicate split is `is_opaque`, not `is_solid` (user refinement)
Sharper than TODO #4: for light-shaft optimization + leaves, the right call is **`is_opaque`/
`blocks_light`** (does it occlude light?), distinct from `is_solid` (collision). A leaf is
solid-ish for collision but **not opaque** → it shouldn't seal faces against other leaves and
shouldn't block light shafts. `block_predicates` now centers `is_opaque` +
`produces_face_against` + `blocks_collision` + `is_meshable_volume`.

## Shader-reasoning lane (parallel-thread, sandbox-doable)
The user wants shader reasoning/tests explored in parallel. Shader code needs Vulkan, but the
reasoning is testable: `spirv_reflect_check` (descriptor↔pipeline-binding match), an
emissive→tonemap→bloom math core (predictable glow, no blowout), and a documented per-frame
cost model for the entity-instancing path so "is it efficient" has a number. (In
`WORKER_PIPELINE.md`.)

## Process — prewarming workers
`prewarm-worker-base.zip` = current base + an **onboarding brief** (explicit Rust install →
`run.sh` GREEN → "PREWARMED, await round"). Spin up extra worker chats with it so they're
rust-ready and harness-green *before* a round arrives; when the live three expire, send the
next round brief into a prewarmed one. (The "new 01 chat installing rust" is exactly this
working.)

## Status
- **R6 partial merged:** T02 (settings_schema, world_params, whitelist, server_connect) + T03
  (voxel_raycast, giant_cave, locate) — 41 tests, GREEN. **T01 (command_parse, inventory_grid,
  crafting) incoming next turn** to complete R6.

---

# Stream 11 (2026-06-04) — body segmentation, worker-task variety, perf (RD=24)

## SIM/animation — dragon body when wired (see handoff/ENTITY_BODY_SEGMENTATION.md)
Large bodies arc through turns (yaw distributed across the spine, head-leads/tail-lags, amplitude
∝ size) and lerp over cliffs with per-section amplitude from (a) distance-to-nearest-leg and
(b) the cliffside air-gap (1-block air-vs-solid). Queued pure cores: `body_arc`, `segment_lerp`
(compose with flex_bend / large_body_grounding / foot_ik). Wiring is M2.

## Worker-task variety (per user) — now flowing into rounds
Beyond feature cores, the additive model also fits: **block-editing** (place/break apply + dirty
bounds — the long-flagged broken block place/break, logic half), **microoptimizations** (batched
ops that must prove BIT-PARITY vs a scalar reference), the **typed pipeline-key abstraction**
(LOD×pass×material → checked key — the flagship), and **test-buildout/audit cores** (pure
invariant-checkers, e.g. octree well-formedness). Audits/cleanups/abstractions that touch SHARED
files stay COORDINATOR work (done on the merged base between rounds) to keep worker patches collision-free.

## Perf — RD=24 run (94s)
- **fires=89746** — standout. Confirm what "fires" counts (weather/lightning/lava/fire-spread) and
  that it isn't runaway; at this count it's a plausible tick_sim(25.9ms)/draw(122ms) sink.
- **SHADOW_MAP_SIZE=2048** (default — the 1024 win is NOT applied). shadow p50=49ms, 96% over budget.
  Set 1024 for ~4× fewer shadow fragments (cheapest remaining lever).
- **main p50=72ms, 98% over budget** — raster-bound at distance. The SurfaceRun far-LOD / Mid-as-runs
  path (handoff/LOD_SHADOW_ENTITY_KIT.md §B) is the real fix, not a flag.
- **near mesher tail**: p99=209ms, max=333ms (CPU; GPU_MESHER=2 offloads only mid/far). Consider
  near→GPU or a smaller near band (LOD_BANDS first split is 12).
- cull spiky (p90=22ms); memory healthy (octree 306/2048MB, oom_fallback=0).

---

# Stream 12 (2026-06-04) — missing mesh tier + perf (RD=24, shadow=1024, dragon spawned)

## RENDER/world — the mesh tier right after near (user) → handoff/MESH_TIER_PLAN.md
near=CPU is the meshing hotspot (p99=242ms, max=519ms, 11726 ops); mid=GPU ≈0ms; no high-detail
GPU tier between them. Fix: a GPU near tier (GPU_MESHER=3) keeping only the edited/innermost ring on
CPU; stopgap = narrow LOD_BANDS first split; also audit remesh_gate (skipped=0 → suppressing nothing).
Queued pure core `mesh_tier` (tier + CPU/GPU + LOD-factor decision).

## Perf deltas this run
- SHADOW_MAP_SIZE=1024 + GPU_MESHER_MID=1 are now applied. But this run is far heavier (high
  altitude, loaded 8975 / rendering 4005 vs 3784/1840), so wall-clock is worse, not better — main
  p50=74ms, shadow p50=68ms. shadow is now **geometry-bound** (2× the rendering chunks), not
  fragment-bound — the map-size win is real but swamped by draw count. cull also spiking (p90=23ms,
  24.6% over 5ms) at 4005 chunks.
- **fires=269000** — TRIPLED from 89746, and this run only added SPAWN_ARCHE=dragon + SEASONS +
  WEATHER. Fires should NOT scale with a dragon spawn. This now looks like a genuine leak/runaway
  (weather ignition that spreads and never extinguishes?). Highest-suspicion sink. Find what
  increments the fire set and whether anything ever removes entries.
- **mgr_update request_visible=131ms** — a new big CPU cost (visible-set computation at 8975 loaded).
  Pairs with chunk_priority (r11) once that's wired to bound the visible request.

---

# Stream 13 (2026-06-04) — dragon genome lock-in, animation system, two optimizations
- **Genome = viewer space** (DRAGON_GENOME.md): the dragon_forge_3d ranges are the canonical genome;
  add MATERIAL genes (armor→reflectivity, wing_opacity) + ANIMATION genes (breathe/blink/vocal/ripple).
- **Animation system** (ENTITY_ANIMATION.md): breathe (chest), vocal_pose (mouth+neck on sound),
  claw_curl, appendage_ripple, blink; wings = thin voxel sheets webbing the skeleton with a
  transparency gene; armor→reflectivity in shaders; smoke/ember/dust effect fields.
- **batch_gene** (user): batched genome→phenotype expression speeds mob spawning — a threaded
  microopt (BIT-PARITY vs scalar). r14.
- **anim_cadence / lerp downsampling** (user): update slow motions every N frames + interpolate,
  fast motions full-rate → reduces GPU/animation load, esp. distant/idle creatures. r14.

---

# Stream 14 (2026-06-04) — horn system (user idea-stream)
Horns → a functional subsystem (handoff/HORN_SYSTEM.md): common 0-4 vs **rare exotic 8-24** with
**fractal branching** geometry; exotic horns **charge abilities** (rate+capacity scale with
complexity; discharge fires an element/obsession-tied power); horn form **encodes the obsession
genome** (hoard/hunt/build/territory/song) which also biases behavior. New genes: horn_type
(quantized, rarity-gated), horn_fractal (branch_factor/depth/sweep), horn_charge (ability/rate/cap),
obsession (drive weights). Pure cores queued R20: horn_type, horn_fractal, horn_charge, obsession.
Viewer follow-up: extend the horns dial to type+count(0-24)+fractal and render branching.

---

# Stream 15 (2026-06-04) — GPU compute abstraction + culling/scheduling/prediction (user)
From the 901s log (octree 96% full, CPU-bound, fires 1M) + idea-stream. See handoff/GPU_COMPUTE_PLAN.md.
- Abstract existing GPU compute (chunk_cull/view_compute/GPU mesher) into a reusable field-kernel
  framework; move weather/water field updates to GPU (grid stencil — GPU-perfect), then plant/creature
  genetics expression + popdyn + pollination as populations scale.
- Multi-level culling (coarse region tier before per-chunk cull); chunk scheduling (budget-aware);
  fast fetch/gen predictors (velocity-based prefetch).
- URGENT integration: SurfaceRun far-LOD routing (octree pool near OOM). Apply heat Fix A/B + cull fix.
- Pure cores R22: popdyn_step, pollen_spread, plant_gene_express, ability_aura, region_cull, lod_morph,
  light_falloff, prefetch_predict, chunk_schedule, field_step, gen_estimate + batch_* bonuses.
