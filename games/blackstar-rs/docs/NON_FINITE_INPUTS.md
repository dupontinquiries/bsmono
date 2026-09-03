# Non-finite inputs in the geometry and animation modules

A survey of six modules, done by probing their public functions with
NaN, infinity, zero and negative values rather than by reading them.
Four had defects. Two did not, and the difference between them is the
useful part of this note.

## Why probe at all

These modules are well tested. `anim.rs` has fourteen tests covering
gait ranking, walk/run hysteresis, phase continuity across a gait change
and the legs staying half a cycle apart. `skeleton.rs` has twenty.
Between them they answer every behavioural question worth asking.

They are also all driven, in those tests, through helpers that supply
sensible values — which is exactly right for testing behaviour and blind
to what happens at the edges. Every defect below sat in a function whose
behaviour was already correct and covered.

Probing each module took a couple of minutes.

## What was found

| module | verdict |
|---|---|
| `anim.rs` | negative `dt` inverted the pose; NaN `dt`/`speed` reached `Pose` |
| `orient.rs` | non-orthonormal axes skewed or scaled; degenerate axes gave a NaN quaternion |
| `skeleton.rs` | **panic** on a non-finite limb; NaN `t` gave a NaN quaternion |
| `xform.rs` | NaN in any argument spread through the returned box |
| `compact.rs` | clean |
| `partdetect.rs` | clean |

The panic is the only crash. The rest are quieter and, for a renderer,
mostly worse to diagnose:

- **A negative `weight` does not damp a pose, it applies the inverse of
  one.** The body leans backwards out of its own stride.
- **A NaN transform does not render wrong, it renders nothing.** "The
  model disappeared sometimes" is a much harder bug report to act on
  than "the model is sideways".
- **A box with NaN extents fails every comparison it is later used in,
  including the bounds checks meant to catch it.**

## The mechanism, in three of the four

`f32::clamp` does not launder a NaN. `NAN.clamp(0.0, 1.0)` is `NaN`.

Every one of those functions *looked* guarded because it clamped its
input, and the clamp was doing nothing for the case that mattered. If
you take one thing from this note, take that.

The fourth, `orient.rs`, is a different shape: `a.cross(b).normalize()`
where `a` and `b` may be parallel divides by zero. Any code that builds
a basis from two vectors it did not construct itself has this.

## What the clean two do differently

`compact.rs` and `partdetect.rs` were written defensively from the
start, and the pattern is consistent:

- `compact::displace` returns the input position unchanged when `t` is
  not usable, rather than computing with it.
- `compact::cell_centre` returns the point unchanged for a zero or NaN
  cell size.
- `partdetect::elongation_of_points` returns 0 for an empty or
  non-finite point set.
- `partdetect::ground_fraction` returns 0.5 — a deliberate neutral —
  for an empty set, rather than 0 or a division by zero.

In each case there is an answer that means *nothing usable was supplied*
and it is the same answer the function already gives for a legitimately
empty case. That is what makes these guards cheap: they do not invent a
new behaviour, they route a bad input to an existing one.

## The rule

Anything taking a `f32`, `Vec3` or `Quat` from outside — a clock, a
physics step, a mesh nobody in this repo authored — states what it does
with a non-finite value, and has a test that pins it.

Prefer an answer the function already gives. `Quat::IDENTITY` for a limb
that cannot swing, the input unchanged for a box that cannot be resized,
zero for a measurement with nothing to measure. Inventing a new failure
mode to handle a bad input is how you get two bugs instead of one.

## Where this has not been done

`align.rs`, `meshgen.rs` and `model.rs` have not been probed. `model.rs`
is the interesting one: it is the loader, so it is where a corrupt mesh
arrives first, and everything above is downstream of it.
