# The science behind the maps

The maps and the procedural patrol worlds are not styled from imagination. Each
landform is gated by a physical rule taken from planetary science — mostly from
what orbiters and landers actually measured — so a world's landscape follows from
its gravity, its atmosphere and its distance from the star rather than being drawn
from a bag of scenery.

The point is not realism for its own sake. It is that **a world assembled by a
script announces itself**: an airless moon with a sandstorm, a dwarf planet with a
kilometre of standing water. Gating on physics is the cheapest way to stop that
happening, and it has the side effect that every world is internally consistent,
which is what makes a place feel like a place.

Implementation: `rust/assetkit/src/formations.rs`, emitted to
`engine_src/exengine/planet_forms.h`. The generator refuses to write if any world
comes out self-contradictory.

---

## The rules, and where they come from

### Crater regime — gravity sets the shape of an impact

Below a certain diameter an impact leaves a simple bowl. Above it, the walls
slump and the floor rebounds into a central peak. **That transition diameter
scales roughly as 1/g**: about 15–20 km on the Moon (0.16 g), around 7 km on Mars
(0.38 g), 3–4 km on Earth. This is one of the best-established relationships in
impact cratering, worked out from lunar mapping and confirmed across every solid
body since.

In the generator: a body below 0.5 g gets `simple_craters` — bowls, sharp rims,
no central peaks. Heavier bodies get `complex_craters` with terracing.

Practically, it means a small moon reads as pocked and even, while a heavier world
has a few enormous scarred basins. Different silhouettes, from one number.

### Lava tubes — roof stability also scales as 1/g

A lava tube's roof can only span so far before it collapses under its own weight.
Lower gravity, wider span. Terrestrial tubes reach perhaps 30 m; **lunar tubes
have been modelled as stable at up to several kilometres wide**, and gravity data
from GRAIL supports large void spaces beneath the maria.

In the generator: `lava_tubes` only appear below 0.55 g and only on non-icy
worlds. They are the caverns you can fly through, open at skylights where the roof
did give way.

### Aeolian features need moving air, not just air

Dunes and yardangs require **saltation** — grains bouncing along the surface,
driven by wind shear. No atmosphere, no saltation, no dunes, however much dust the
surface has. Mars has both (HiRISE has imaged active dune migration); the Moon has
neither despite being buried in regolith.

This gate caught a real bug. An earlier version tested only for *airlessness*, and
happily gave dune fields to a world whose own description read "superfluid, no
weather" — plenty of fluid, no wind. Saltation needs a shear velocity, not a
medium. The check now quotes the atmosphere back in the error.

### Cryovolcanism — only where volatiles stay solid

Water, ammonia and methane behave as rock in the outer system. Erupted, they come
up as slurry and freeze as they fall. Cassini's observation of the Enceladus
plumes and Voyager 2's of Triton established this as a real geological process,
not a curiosity.

In the generator: `cryo_vents`, `sublimation_pits` and `chaos_terrain` are gated to
outer and deep orbits. Chaos terrain — broken plates tilted at every angle,
refrozen between — additionally needs low gravity to keep the blocks standing.

### Lobate scarps — small bodies cool fast and shrink

As a planet's interior cools it contracts, and the crust fails in thrust faults
that run for hundreds of kilometres. **Mercury is the type example**; MESSENGER's
global imaging showed contraction far greater than pre-flyby estimates.

In the generator: `lobate_scarps` need gravity below 0.7 and a radius below
8,000 km — small enough to have cooled through.

### Tessera — repeated deformation without breaking

Where gravity is high and the interior stays hot, crust deforms over and over
rather than fracturing cleanly, leaving ridges crosscutting in two directions.
Magellan's radar mapping of Venus is where this became a named terrain type.

In the generator: `tessera` above 1.4 g.

### Talus — gravity changes the runout, not the angle

This one is counterintuitive and worth stating plainly. **The angle of repose is
set by friction, not weight**, so it is close to 33–37° almost regardless of
gravity — a scree slope on the Moon stands at roughly the same angle as one in the
Alps. What gravity changes is how far the debris *travels* before it stops.

In the generator: low gravity gives `long_talus` — aprons thrown far from the
cliff, thin and wide. High gravity gives `short_talus`, piled tight against the
foot. Same rock, same angle, completely different landscape.

---

## Consistency checks

The generator refuses to write output if any world contains:

  * aeolian features with nothing to move sediment
  * cryovolcanism in a warm orbit
  * both crater regimes at once
  * both talus regimes at once
  * kilometre lava tubes above 0.55 g

It also warns if more than two formations appear on **every** world, since that
means the gating has stopped discriminating and the worlds will read the same.

## Does it change play, or only description?

Feel is derived from the same physics: gravity scale, fog density, wind strength,
terrain relief, roughness, feature density and step hazard, in
`rust/assetkit/src/planets.rs`. Two deliberate departures from strict realism:

  * **Gravity is compressed toward 1.** A true 0.16 g moonlet leaves the player
    airborne for whole seconds, which reads as a broken jump rather than as low
    gravity. The range is compressed to keep the difference legible.
  * **Relief is dominated by gravity, not size.** The first version multiplied a
    radius term by a gravity term and they cancelled almost exactly — moonlet
    0.64, terran 0.71, giant 0.82, so every world had the same skyline. Rewritten
    so gravity leads: relief now spans 0.81 to 3.09, and small low-gravity bodies
    get the extreme topography they should.

A spread check reports the mean pairwise distance across the normalised feel
vector. It reads 1.03 across ten worlds. Below 0.25 the tool says the derivation
has stopped separating them — the line between generation and decoration.

## The hand-authored maps

The sixteen fixed maps in `content/maps.json` predate this system and are authored
rather than derived, but they follow the same instinct — each has one landform
idea and an atmosphere that agrees with it:

    kuiper       void_edge       shepherd moons + a frozen debris braid
    unicron      devourer_red    the maw; horizon curves the wrong way
    trench       abyssal_cyan    rift dais + monoliths
    pluto        far_sun_dark    containment core
    frostline    cold_blue       ice shelves
    emberpit     lava_glow       lava pools

`kuiper` and `unicron` were added late, when a check found the raid plan pointing
at two maps that had never been defined — two of three raid phases were loading an
undefined theme. Shepherd moons are real: small bodies whose gravity confines a
ring or debris band into a sharp edge, as at Saturn.
