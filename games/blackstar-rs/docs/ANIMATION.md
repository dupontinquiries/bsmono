# THE ANIMATION LAYERS

Six things move a body, they were built at different times, and they have
never had a defined relationship. Most of the "the bot is sideways / in the
ground / spinning its knees" bugs are two of these disagreeing rather than
any one of them being wrong.

## What is stacked, bottom to top

1. **Asset frame** — one quaternion per chassis, from an `orient.json`
   sidecar or guessed (`model::strip_guess`). Shared by every pose in the
   strip, because a per-pose frame makes the fold tumble.
2. **Fold morph** — `xform::Morph`, vertex-blended keyframes between poses.
   There are no bones here: positions lerp, and the hitbox follows through
   `bounds_at`.
3. **Per-pose settle** — `Model::settle`, the leftover uprighting when an
   artist authored the robot standing along a different axis from the one
   the vehicle drives along. Ramped by `Xform::eased()` so the journey is
   untouched and only the destination moves.
4. **Part transforms** — per prim in the draw path: wheel spin, detected
   rollers, turret yaw, `tuck` (robot) and `pack` (vehicle) from
   `skeleton`.
5. **Procedural pose** — `anim::Pose`: bob, lean, roll, `leg_l/r`,
   `arm_l/r`, gait selection including the damage gaits. **Bob and lean
   reach the body; the limb channels reach nothing.** A pose strip is a bag
   of vertices, so "swing the leg" has no address to send the rotation to.
6. **Physics body state** — `ride_h`, hitbox, `sections::Body::limp()`
   feeding gait, `driveline::Handling`.

## TODO 1 — bones: the shapes exist, the joints do not

The meshes are already there (ribs, legs, arms, feet). What is missing is
the ARTICULATION: a hierarchy of pivots, a parent-child chain, and a
vertex-to-bone mapping.

`skeleton.rs` already decomposes any body into a core plus appendages with
roots and tips — it just names them generically ("limb, reach 0.86"). The
work is to teach it the blueprint:

* below the core, fore and aft, paired left/right -> `leg_l` / `leg_r`,
  root = hip pivot
* outboard at shoulder height -> `arm_l` / `arm_r`, root = shoulder pivot
* the rest stays generic

Then `anim::Pose`'s limb channels bind to those roots and stop being
computed and discarded. This is an extension of something that exists, not
a new system.

Open question: whether a limb needs a two-bone chain (thigh/shin) to look
right, or whether one rotation about the hip is enough at the scales and
camera distances this game uses. Measure before building the chain.

## TODO 2 — precedence, and the correction that matters

Working order, damage first:

1. **Damage** overrides everything. A wrecked body does not do anything
   else gracefully.
2. **Gait** (bot) or **rotators** (vehicle), selected by mode.
3. **Fold**.

### The correction: fold must COMPOSE with motion, not gate it

My first proposal was to fade gait in as the fold finishes — a body
mid-transformation would not try to walk. That is wrong, and it forbids the
best thing the system can do.

**Transforming while moving is the point.** Folding mid-vault, landing as a
robot and running out of it, changing form to clear an obstacle and
changing back on the far side — combat as parkour. A rule that silences
motion during a fold makes that impossible by construction, and "get the
basics working" is not a reason to build the wrong shape.

So: the fold is a state the body passes THROUGH while still moving, not a
modal animation that owns the body. Consequences to design for:

* The limb channels must remain addressable mid-fold. Where a leg IS
  halfway through a fold is a question the pose-delta rig has to answer,
  not one to dodge by muting the layer.
* Gait blends by what the body currently HAS to walk on, not by fold
  progress. A body whose legs have deployed can run on them at
  `eased() == 0.4`.
* `settle` (layer 3) keeps its ramp — that one really is about the
  destination and is not a motion layer.
* Ground contact has to track the fold continuously. The blended `ride_h`
  bug (fixed: the stretch that `redistribute_length_to_height` applies to
  the hitbox was not applied to the ride) is the same class of problem and
  will recur wherever a mid-fold quantity is derived from an end pose.

## TODO 3 — rotators: the turret is one case of three

`turret.rs` is named for tanks and should not be. A rotator is a pivot, an
axis, and a source for its angle:

| part | axis | angle from |
| --- | --- | --- |
| tires, rollers, treads | lateral | distance travelled |
| propellers, thrusters | along thrust | throttle |
| turrets | vertical | the look vector |

Same transform, three inputs. Generalise to `rotators.rs`, keep the
detection, and treads finally get their scroll (which needs a UV scroll in
the shader or per-link geometry — a rendering problem, not a detection
one, and the indices are already exposed).

## TODO 4 — damage reactions and stagger

`Crouch` and `WallHug` exist and are driven by `limp()`. Missing: the
impulse response — a hit that knocks a body off its stride and recovers,
rather than a state that switches. Wants the rig from TODO 1, since a
stagger is a limb event.

## Standing diagnostics

* `--orient check <bot>` prints, per pose: the frame the LOADER applies and
  where it came from, `ride_h` vs `half_h` with what the gap does,
  `skeleton::report`, and the fittings.
* Layer bugs look alike from a screenshot. The question that localises them
  fastest is whether the WRONG thing is the same in vehicle and robot mode
  ("one frame, wrong") or different ("the destination is wrong"), and
  whether the fold between them is coherent ("the poses agree").
