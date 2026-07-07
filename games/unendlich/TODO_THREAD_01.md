# TODO — Thread 01 (sim · locomotion · body-physics · creature AI)

Running list of things discussed in this thread but not yet finished. Thread 01
owns `crates/unendlich-engine/src/sim/**`, the creature body/anatomy model, and
the per-roamer tick hooks in the bin. Rendering, meshing, and shaders belong to
the rendering thread and are listed here only as cross-thread handoffs.

Status legend:
- **[wire]** — built and unit-tested in the lib; needs bin/render hookup (which
  can't be verified in the sandbox, only on the M2).
- **[build]** — discussed, designed, but not written yet.
- **[other]** — belongs to another thread.

---

## 1. Built & tested — just needs wiring

These are pure modules with passing tests; the only thing left is the
unverifiable bin/sim-loop/render hookup.

- **[wire] Ragdoll on death/knockout.** `sim::ragdoll` (Verlet nodes/sticks,
  gravity, ground, self-collision) exists. Needs a `ragdoll::from_anatomy(&Creature)`
  bridge (parts → `Node`s, bone offsets → `Stick`s) and a death/knockout trigger
  that flips a creature into ragdoll mode and renders parts at node positions.
- **[wire] Per-state liveliness.** `sim::liveliness` gives `limpness` per
  `MotionState` and a `breath` swell. Needs: the tick to set each creature's
  `MotionState`, and the entity vertex pass to `blend3` the driven pose toward
  the ragdoll result by limpness and scale the torso by `breath`.
- **[wire] Squash & stretch deform.** `sim::deform` (volume-preserving
  `stretch_axis`/`landing_squash`/`motion_stretch`). Needs per-part application in
  the entity vertex pass, fed by landing impact and forward speed.
- **[wire] Circadian rest cycle.** `sim::rest_cycle` (`rest_urge`, `wants_rest`,
  `rhythm_for`). Needs: feed `rest_urge` into the brain's drive arbiter so a tired
  or off-schedule creature beds down; a `Drive::Rest` (or equivalent) that
  suppresses the wander reflex; a `chill` source from local temperature so bees
  (and other cold-sensitive diurnals) go home early.
- **[wire] Dragon / large-body sensing & pathing.** `sim::body_scale::BodyScale`
  (sight ∝ √size, probe span ∝ size, coarse path tile) and
  `collision_probe::coarsened` (NxN tile max-height probe) are built. Wire them
  into the large-creature tick so dragons see farther and probe terrain coarsely
  (cheaper per-tick). NOTE: the size-aware *lean* (`terrain_lean_sized`) is
  already wired; this is the rest of the BodyScale story.
- **[wire] Stamina + sprint.** `sim::stamina` (drain/regen, sprint hysteresis).
  Wire the speed multiplier into the roamer movement tick.
- **[wire] Eased turning.** `sim::turning::ease_yaw` (damped angular spring).
  Wire into the roamer heading update for smooth turns.
- **[wire] Surface cache + cadence.** `sim::surface_cache` and `sim::cadence`
  (perf levers) are built but unused — wire to cut redundant surface probing.

## 2. Animation & body-physics — to build

- **[build] Neck/tail flex-bend.** An S-curve / swish model so serpents, dragons,
  and tails bend through a chain of segments (offered, not built). Composes with
  `deform` and the gait phase.
- **[build] Per-leg catch-force / foot-lock IK.** The proper inverse-kinematics
  foot planting beyond the current `grounding` settle — eliminates the residual
  float noted in `docs/body_physics.md`.
- **[build] Moonwalk residual.** Slight side-strafe still reads as moonwalking;
  `head_steer` decoupled gaze from locomotion but a true strafe-vs-face solver is
  deferred until an advanced movement system.

## 3. Creature AI & ecology — to build

- **[build] Block-level grass foraging.** Actually consume grass *blocks* when a
  creature feeds (current `feeding`/`forage_memory` track cells abstractly).
- **[build] Predator/prey loop.** A full ecology: hunts land, prey is removed,
  populations respond.
- **[build] Fall damage application.** `sim::fall` has the model; apply real
  damage to anatomy/vitality on hard landings.
- **[build] Swim / fly 3D pathing.** Aquatic and aerial navigation (current
  pathing is grounded 2.5D); prerequisite for water creatures and free-flying
  dragons/birds.
- **[build] Bees + hives.** No `Bee` creature kind exists yet. Add the species
  (diurnal + cold-sensitive — the rest cycle already models this via `chill`) and
  hive/swarm behavior.
- **[build] Monkeys.** Want real forests to inhabit first (see plant genetics).
- **[build] Voice genome + Mimic horror.** `docs/sound_genome.md` — the creature
  that learns and mimics other voices.
- **[build] Plant genetics.** Habitat-shaping flora; prerequisite that gives the
  monkeys (and forest fauna) a world worth living in.

## 4. Cross-thread handoffs (rendering thread)

- **[other] Dragon/large-creature render lag.** The measured lag is
  rendering-dominated: at `UNENDLICH_RENDER_DISTANCE=64` the octree pool fills
  (256 MB → OOM fallbacks → CPU-fallback meshing), main pass ~94 ms / shadow
  ~72 ms. Mitigations are env (`UNENDLICH_OCTREE_POOL_MB` up, render distance
  down) or rendering-thread LOD work — not a sim fix.
- **[other] Wire `creature_to_vertices_lod`.** The per-creature part-LOD function
  (drops fine parts, keeps torso + feet) is built; the rendering thread should
  pass a distance-scaled `min_part_half` in the entity pass so distant/large
  creatures shed detail.

## 5. UI / app / infra (big, discussed)

- **[build] Inventory / settings / launcher UI.**
- **[build] Multiplayer.**
- Done: HTML locomotion trace viewer (`viewer/locomotion_viewer.html`) — could be
  extended to play back ragdoll/rest-cycle traces too.

---

### For context — shipped in this thread already
Locomotion (posture/lean incl. size-aware, head-steer gaze decoupling, grounding/
foot settle, stumble, leap, fall sighted-step), `body_collision`, `stamina`,
`turning`, `body_scale`, `collision_probe` (+`coarsened`), `locomotion_sim`
harness + trace viewer, `spawn::from_blueprint` (+`UNENDLICH_SPAWN_ARCHE`),
`deform`, `ragdoll`, `liveliness`, `rest_cycle`, and the `merge_contract` tests.
