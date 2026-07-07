# Character movement: step-up & variable jump

**Status (2026-05-31):** both features pure, deterministic, and **fully
unit-tested** — no machine needed to trust the logic. The only thing hardware
adds is *feel* (whether the two constants below are tuned to taste). One
small arc, split out from the larger mob-rendering work because it stands alone.

## The kernel both player and mobs share

`sim::physics::step_character(&Character, &Input, &world) -> Character` is the
per-entity physics tick. Order: ground friction → horizontal control → gravity →
jump → body drag → buoyancy → `move_and_collide`. **Mobs run this exact same
function** (no separate movement path — an AI's intent is fed in as `Input` just
like a player's), so anything added here applies to both.

`move_and_collide` is per-axis swept resolution: sweep X, then Z, then Y, each
clamping the body's AABB against solid voxels via `resolve_axis`, which returns a
direction flag when it stops you against a wall. That flag is what made both
features below possible.

## Step-up (patch 218)

**Problem:** walking into a single-block rise (terrain step, stair, low ledge)
clamped horizontal motion dead — you had to jump every block.

**Fix:** a grounded body that a wall just stopped tries to climb it, in one tick,
before the vertical pass:

1. Only if it was **grounded** at tick start and X or Z reported blocked.
2. **Headroom check** — the lift (by `STEP_HEIGHT`) must not drive the head into
   a ceiling at the start column.
3. Re-run the horizontal sweep at the **raised** level. If the obstacle is still
   in the way there, it's too tall — abort (this is what refuses a two-block
   wall).
4. **Settle** the feet back down onto the ledge top, accepting the step only if
   the body actually ends up *higher* than it started.

On success it keeps the horizontal velocity (so motion stays smooth, the wall
doesn't kill your speed) and rests on the ledge. `try_step_up` is pure.

`STEP_HEIGHT = 1.1` → clears exactly one full block, never two.

**Tests:** climbs a one-block plateau and keeps walking; refuses a two-block
wall (stops at the face, stays at floor level); refuses the climb when a ceiling
one block above the head blocks the lift. The pre-existing wall/edge/ceiling/
foliage/buoyancy tests are unchanged — the wall test's column is five blocks
tall, so step-up correctly declines it.

## Variable jump height (patch 219)

**Problem:** every jump launched the full arc; no tap-vs-hold control.

**Fix:** after the ground-jump branch, if a body is airborne, still rising
(`vel.y > 0`), and the jump button is **not** held, scale upward velocity by
`JUMP_CUT`. Applied per rising tick while released, so an early release decays
the ascent quickly toward the apex.

`JUMP_CUT = 0.5` → a tap reaches roughly half the apex of a hold.

Button state is already part of `Input`, which is snapshotted under rollback, so
this is fully deterministic and replays are unaffected.

**Test:** from the same takeoff, a tapped jump peaks lower than a held jump while
still leaving the ground.

## Tuning (the only machine-dependent part)

| Constant | File | Now | Raise it to… | Lower it to… |
|---|---|---|---|---|
| `STEP_HEIGHT` | `sim/physics.rs` | `1.1` | climb taller ledges (≥2.0 = two blocks) | only slabs / sub-block rises |
| `JUMP_CUT` | `sim/physics.rs` | `0.5` | softer cut (taller minimum hop) | sharper cut (tinier tap hop) |

Both are single-line changes; the tests assert *behaviour* (climbs one / refuses
two; tap < hold), not exact heights, so retuning won't fight the suite.
