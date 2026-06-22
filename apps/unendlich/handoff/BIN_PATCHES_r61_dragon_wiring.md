# BIN PATCHES r61 — make dragons fly, use their brains, drop the old ones, show the soft wings

All four asks land here. The engine side is done and gated (see modules below); these are
the compile-on-your-end hooks in `unendlich_world.rs` (the bin can't compile in-sandbox:
glfw/winit). APIs cited are real and tested.

Engine modules now in the tree (r61):
- `sim::dragon_brain`  — one `think()` per tick that composes activity + ground/air mode + flight + thirst.
- `sim::mob_limb_girth` — leg-thickness QA (generalized; dragons via `is_leg_joint`). The rig fix (legs no longer fatten while walking) already landed in `dragon_forge::skin`.
- `sim::dragon_forge::skin_with_flex` — wing-membrane sag apply path (drop-in for `skin`).
- `sim::dragon_thirst` — hydration vital (already composed inside `dragon_brain`).
- `sim::dragon_wing_flex` — verlet wing solver (step per tick; feed offsets to `skin_with_flex`).

---

## 1. Old dragons still appear → route ALL world dragon spawns through the forge path

The lib is already correct: `spawn::spawn(CreatureKind::Dragon, pos)` and
`spawn::spawn_seeded(CreatureKind::Dragon, seed, pos)` both attach a forge model
(`.with_forge(..)`). The old box dragon is `spawn::spawn_beta_dragon` only.

The bin must stop minting dragons via the blueprint/egg path. Find any of:
```rust
EggSpec::chosen(Arche::Dragon, seed).hatch()           // → from_blueprint → OLD box dragon
spawn::from_blueprint(&dragon_blueprint, pos)           // OLD
spawn::spawn_beta_dragon(pos)                            // OLD (beta gate only)
```
and replace the world-spawn ones with:
```rust
// varied per seed (egg/breeding), forge-rendered:
let dragon = spawn::spawn_seeded(CreatureKind::Dragon, seed, pos);
// or position-derived genome:
let dragon = spawn::spawn(CreatureKind::Dragon, pos);
```
`creature_viewer.rs` may keep `EggSpec` for the design viewer — that's the viewer, not the
world. The world spawner + any `UNENDLICH_SPAWN_ARCHE=dragon` path must use `spawn::*`.
Verify on a fresh dragon: `creature.forge().is_some()` must be true.

## 2. Dragons fly + use their brains → one BrainState per dragon, `think()` each tick

At spawn, build a brain from the dragon's genome and carry it on the bin-side creature
record (not the engine `Creature`):
```rust
use unendlich_engine::sim::dragon_brain::{self, BrainState, BrainSenses};
let mut brain = BrainState::new(&genome);   // genome you spawned the dragon with
```
Each fixed tick, gather perception and step the brain:
```rust
let senses = BrainSenses {
    threat, aggression, health_frac,
    food_in_reach, water_nearby,                 // water_nearby: a river_flow channel cell in reach
    day_phase,                                   // 0=midnight .. 0.25=noon
    heard,                                       // dragon_hearing::Reaction
    target_above, target_below, target_dist2, airborne_threat,
    target_altitude, updraft,                    // cruise alt + thermal lift (m, m/s)
    desired_heading,                             // your steering toward the goal (rad)
    exertion01,                                  // extra load; flying is added internally
    ..Default::default()
};
let out = dragon_brain::think(&mut brain, &senses, dt);
```
Apply `out`:
- **Flight vs ground**: `out.mode` is `MovementMode::Air|Ground`. When `Air`, place the body at
  `out.altitude` (m above ground) and drive the wingbeat amplitude from `out.flap01` (0..1).
- **Move**: steer toward `out.move_heading` at effort `out.move_speed01`; if `out.wants_sprint`,
  use `dragon_locomotion::sprint_step`, else `dragon_locomotion::step`.
- **Rig pose**: set `ForgeAnim.wing_pose = out.wing_pose` (Folded grounded, Beating on
  takeoff/climb/land, Spread cruising, Flared soaring/gliding, Aggressive diving). Advance
  `ForgeAnim.gait_phase` by ground distance travelled this tick; set head yaw/pitch toward target.
- **Drink**: if `out.seek_water`, steer to the nearest water cell instead of food; when adjacent,
  call your drink action (the brain's `BrainState.hydration` recovers via `Hydration::drink`).
- **Activity** (`out.activity`) is the high-level state for VFX/UI (Sleeping/Idle/Forage/Feeding/Alert).

The brain is deterministic and owns vitality + hydration internally — do NOT double-tick them.

## 3. Show the soft wings → step WingFlex per tick, render via `skin_with_flex`

Carry a wing solver per dragon and step it with the rigid wing endpoints from the rig:
```rust
use unendlich_engine::sim::dragon_wing_flex::{WingFlex, FlexParams};
let mut wings = WingFlex::new(wing_span, 7);          // span in model units; ~7 samples
let fp = FlexParams::default();
// each tick, after you know the animated rigid wing line (root + tip, model space):
let (offs_l, offs_r) = wings.step((root_l, tip_l), (root_r, tip_r), dt, &fp);
// stash offs_l/offs_r on the creature record for the render pass.
```
In the forge render path (`world/entity.rs::forge_to_vertices`, currently calls
`dragon_forge::skin(model, &anim)` at line ~1165), switch to:
```rust
let boxes = dragon_forge::skin_with_flex(model, &anim, &offs_l, &offs_r);
```
where `offs_l/offs_r` are the creature's current wing offsets (empty slices ⇒ identical to
`skin`, so the change is safe before you wire the step). The post-pass droops each wing box
by its membrane sag at its span fraction (pinned at root + tip).

## 4. Already handled by the above
- **Thirst** rides inside `dragon_brain` (drink urge + thirst-beats-hunger priority).
- **Leg girth**: the rig fix shipped in `skin`; for a runtime guard, every ~5 ticks you may call
  `mob_limb_girth::exceeds_girth(rest_boxes, animated_boxes, dragon_forge::is_leg_joint, 1.1)`
  in a debug build and log if true (it should never fire now).

## Verify after wiring
- A spawned dragon has a forge model (no box silhouette).
- Idle dragons walk/forage on the ground with folded wings, slim legs.
- Provoked / airborne-threatened dragons take off, climb, and beat/spread/flare their wings.
- Parched dragons near a river break off to drink.
- Wing membranes sag and lag under the wingbeat (verlet flex), pinned at root and tip.
