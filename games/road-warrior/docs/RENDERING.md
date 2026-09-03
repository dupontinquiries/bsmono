# Road Warrior rendering

What the shader draws and why, with the assumptions each piece rests on.
Written because the lighting is baked, and baking is only correct under a
condition that is easy to break without noticing.

## The load-bearing assumption: the camera is fixed

The car sits at a constant screen position (`CAR_SCREEN_X`, 0.32 across)
and the world scrolls under it. The projection is orthographic. The sun
does not move.

That makes "how much sun reaches this point" and "how much sky can this
point see" properties of the **terrain alone**. They do not depend on
where the player is, which way anything faces, or what frame it is. So
they are computed once per level and looked up thereafter.

**A moving or rotating camera breaks this and the bake has to go.** If
someone adds a camera that pans, tilts, or follows the car vertically,
`core::lighting` stops being valid and the rays have to be cast per
frame — or the design has to change. This is the first thing to check
before touching the camera.

## Rays, traced once

`core::lighting` casts two 1D fields along the track:

- `sun_visibility` marches from the surface toward the sun and reports
  how much gets through. **Soft, not a hit test**: a ray clearing terrain
  by a hair is nearly blocked, one clearing by metres is open. A hard
  edge on a heightfield this coarse reads as an artifact, and softening
  costs nothing when it runs once.
- `sky_openness` finds the horizon angle each way and takes the remaining
  wedge. This is the ambient term that darkens a pit floor at noon.

`BakedLighting::bake` samples both across the track at load
(`LIGHT_SAMPLES`, 256 — finer than the 64 columns the viewport shows, so
shader interpolation is never the limiting factor).

A test asserts the cache agrees with a direct trace at five positions.
That is the only property that makes a cache worth having, and it is the
one to keep green if the tracing changes.

`STEP` is 0.5 world units, chosen against an 8-unit ramp as the smallest
feature. **Curved or finer terrain needs a smaller step**, and the bake
cost scales with it — see BACKLOG.md.

## What crosses to the GPU

One uniform per viewport column, interleaved:

    (height, sun visibility, sky openness, unused)

Interleaved rather than three packed arrays because the shader reads all
three at every column, so a mis-set stride can only be wrong in one
place.

`VIEW_SPAN_WORLD` and `CAR_SCREEN_X` are shared constants, not duplicated
numbers. The sample under the car's column must be the ground the car is
standing on; two copies of either value puts the wheels above or below
their own terrain. A test pins that alignment at five positions.

### Gaps are a sentinel, and that is deliberate

A Gap samples as `NO_GROUND` (-1e4), not as the previous height. The
first version held the last height across gaps to avoid a cliff artifact,
and a screenshot showed why that was wrong: **the pit rendered as a solid
plateau you could watch the car drive onto**, with the drop appearing at
the far edge. Hiding the hazard a level is built around is worse than any
cliff.

-1e4 rather than a small negative because real terrain goes to -4 in
Scrapyard Gauntlet; the sentinel has to be unmistakable.

## Materials

`core::material` maps a car's **armour stat** to its surface response,
following `unendlich-engine`'s `armor_reflect`: one gameplay number
drives how the surface answers light, so what you read on the HUD is what
you see on the car.

Reflectivity never reaches zero and the specular exponent is bounded at
both ends. A fully matte car loses its silhouette against the ground when
sky fill is the only light left; a sub-pixel highlight flickers as the
car rotates.

The two values ride in `SceneUniforms`' former padding slots rather than
growing the struct — the 48-byte size already matches what the shader
expects.

### The car is a silhouette, so it has no real normal

The body is treated as a cylinder lying along its length: the normal
sweeps across the body and rotates with the chassis, which is why a car
pitched nose-up on a ramp visibly catches the sun differently. Specular
is placed by the normal alone, because a view vector is meaningless in an
orthographic 2D scene.

`across` is measured against **whichever region the pixel is in** — body
or cabin. Using the body's half-height everywhere put the cabin, which
sits above it, permanently at `across = 1`, tilting its normal fully away
from the sun and rendering it near-black. The car had a hole where its
cabin should be. The boss has the same two-box shape and the same trap.

### The car's ambient does not use the baked sky term

Deliberate. The bake describes points **on the ground**; sampling it for
a car in the air would dim it every time it drove past a wall it was not
inside.

### The boss's telegraph flash is unlit

Also deliberate, and commented at the branch. The telegraph is a
readability signal, not a surface: an incoming attack must look identical
whether the boss stands in sun or in the shadow of a ridge. Shading it
would make the warning dimmest exactly where the scene is hardest to
read.

## Verifying changes

The game builds and runs headless — see
[docs/agents/HEADLESS_GUI_TESTING.md](../../docs/agents/HEADLESS_GUI_TESTING.md).
Every visual claim in this file was checked by screenshot, and several
were wrong before that: the terrain was invisible, the pit was a plateau,
the cabin was black, and the boss floated above sloped ground. None of
those failed a test.
