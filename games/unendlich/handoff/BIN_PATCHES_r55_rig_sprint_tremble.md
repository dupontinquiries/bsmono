# BIN PATCHES — r55: wing pose · sprint · tremble camera · mesh-gap fix

All four landed in **pure sim** + the rig; the bin just feeds them inputs and reads
outputs. None of this compiles in-sandbox (glfw/winit), so apply on your end.

---

## 1. Mesh-gap fix — NOTHING to wire (automatic)

`dragon_forge::skin()` now (a) pivots legs at the hip and the tail at its base
(proximal), so limbs no longer tear from the trunk, and (b) bridges box seams with
a small puff + displacement-proportional stretch while animating. This is internal
to `skin()`; the bin already calls `skin()` via `world::entity::forge_to_vertices`.
**No bin change.** Verified by `sim::mob_pixel_peeper` (the white-bleed detector) —
the full gait × anim-time sweep is under tolerance.

> If you want the peeper as a CI/asset check for *any* voxel mob: it operates on a
> plain `&[ForgeBox]`. `mob_pixel_peeper::worst_bleed(rest_boxes, anim_boxes, 56)`
> returns the torn-seam pixel count; gate new archetypes at `<= 5`.

---

## 2. Wing pose — brain sets `ForgeAnim.wing_pose` per air/behaviour state

`ForgeAnim` gained `wing_pose: WingPose` (defaults to `Spread`, so un-wired
creatures are unchanged). Set it from the dragon's current movement/behaviour
state when you build the anim in `forge_to_vertices` (or carry it on the creature):

```rust
use unendlich_engine::sim::dragon_forge::WingPose;
use unendlich_engine::sim::dragon_air::FlightPhase;

// pick from the air-behaviour FSM (dragon_air) + activity/hunt
let wing_pose = match air.phase {
    FlightPhase::Grounded | FlightPhase::Landing if creature.on_ground => {
        if alert_or_striking { WingPose::Aggressive } else { WingPose::Folded }
    }
    FlightPhase::Landing                       => WingPose::Flared,
    FlightPhase::TakingOff | FlightPhase::Climbing => WingPose::Beating,
    FlightPhase::Cruising  | FlightPhase::Soaring  => WingPose::Beating,
    FlightPhase::Gliding | FlightPhase::Diving     => WingPose::Spread,
    _ => WingPose::Spread,
};

let anim = ForgeAnim {
    gait_phase: creature.gait_phase,
    anim_time:  creature.anim_time,
    head_yaw:   creature.gaze_yaw,
    head_pitch: creature.gaze_pitch,
    wing_pose,
};
```

Rule of thumb the enum encodes: **Folded** = walking on the ground (tucked back+up),
**Aggressive** = threat/strike (mantled high), **Beating** = liftoff/climb/fly
(powered beat), **Flared** = landing (wide brake), **Spread** = glide/idle. A
non-`Spread` pose animates even while standing still (the rig no longer early-returns
to rest for it).

---

## 3. Sprint — call `sprint_step` instead of `step`, carry a `Stamina`

`dragon_locomotion::sprint_step` wraps `step` with a stamina gate. Give the creature
a `stamina: sim::stamina::Stamina` field (`Stamina::full()` on spawn) and a
`wants_sprint: bool` from the brain (flee / closing on prey / player sprint key):

```rust
use unendlich_engine::sim::dragon_locomotion::{sprint_step, GaitKind};

let r = sprint_step(
    &loco_params, creature.ground, desired_heading, desired_speed,
    wants_sprint, &mut creature.stamina, dt,
);
creature.ground = r.loco.state;          // heading / speed / gait_phase
position += forward_vec(r.loco.state.heading) * r.loco.forward;
let sprinting = r.sprinting;             // e.g. play a faster gallop SFX / dust

// gait now reads Gallop while sprinting; map to footfall cadence (see §4)
```

`wants_sprint == false` is byte-identical to the old `step` path and regenerates
stamina, so existing callers can flip to `sprint_step(..., false, &mut st, dt)`
with no behaviour change. Suggested brain wiring: `wants_sprint = behaviour ==
Flee || (hunt.phase == Pursue && stamina.can_sprint())`.

---

## 4. Tremble camera — impulse on events, step per frame, add offset to the camera

`sim::camera_tremble`. Put a `TrembleState` on the camera (`TrembleState::new()`),
fire `impulse` on game events, and add `step(dt)`'s offset to the camera transform
**before** building the view matrix:

```rust
use unendlich_engine::sim::camera_tremble::{TrembleState, Tremor};

// --- on events (anywhere they happen) ---
// footfall: when a leg's gait_phase crosses a foot-plant (gait == Gallop → bigger)
cam.tremble.impulse(Tremor::Footfall, if sprinting { 1.6 } else { 1.0 } * size_scale);
// roar / breath wind-up:
cam.tremble.impulse(Tremor::Roar, 1.0);
// breath blast or melee connecting:
cam.tremble.impulse(Tremor::Impact, hit_strength);   // strength can exceed 1
// wings beating the ground on take-off:
cam.tremble.impulse(Tremor::Liftoff, 1.0);
// touchdown:
cam.tremble.impulse(Tremor::Landing, land_speed01 * 1.5);

// --- each frame, before the view matrix ---
let o = cam.tremble.step(dt);
let eye = base_eye + vec3(o.pos[0], o.pos[1], o.pos[2]);   // world units
let yaw   = base_yaw   + o.yaw;
let pitch = base_pitch + o.pitch;
let roll  = o.roll;                                        // bank the view
let view = view_matrix(eye, yaw, pitch, roll);
```

Footfall detection: a foot plants when its swing phase wraps. Simplest hook — watch
`ground.gait_phase` crossing the half/whole-cycle marks (TAU/2 and TAU) between
frames and fire one `Footfall` per crossing. Scale `strength` by dragon size
(reuse the forge bulk/diag) so a big dragon's steps shake harder. Energy is capped
(`ENERGY_CAP`), so a flurry won't nauseate; the shake decays to exactly zero
(`is_active()` false) so you can skip the offset add when idle.

---

### New/changed pure modules this roll (all gated: dev gpu + release render + HARNESS GREEN)
- `sim::mob_pixel_peeper` (new) — white-bleed mesh-gap detector
- `sim::camera_tremble` (new) — event-driven camera shake
- `sim::dragon_forge` — leg/tail pivot relocation, seam-bridge stretch, `WingPose`
- `sim::dragon_locomotion` — `sprint_step` / `SprintStep`
- `world::entity::forge_to_vertices` — `ForgeAnim { .., ..Default::default() }` (wing_pose)
