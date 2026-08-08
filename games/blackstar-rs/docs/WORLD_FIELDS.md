# WORLD FIELDS — one property system, everything downstream of it

Read out of unendlich's design library, checked against what our two engines
already carry, and written down because the idea transfers and the specifics
mostly do not.

## The transferable idea

unendlich's river spec puts it plainly: seven river types, all driven by
**the same environmental property system used throughout the world**, and
"no separate river-specific code branches are required". Flow speed,
turbulence, water clarity, the fog that forms at night, a creature's energy
recovery, the camera shake underwater — all READ from the same fields rather
than each being authored.

Its terrain row is the one that matters for us: *noise function type,
erosion parameters, sediment deposition* are outputs of the property
reading, not inputs chosen by hand. An alpine river gets ridged FBM plus
curl; a glacial one gets large-scale Perlin. Nobody picks that per river —
the properties do.

That is a different architecture from ours and a better one. We currently
have the ingredients in three places that do not talk:

| where | what it holds |
|---|---|
| `src/patrol.rs` | gravity, air, terrain kind, weather, day length, sky |
| `src/terrain.rs` | the physics — dune wavelength, crevasse depth, crater transition, talus runout, all as functions of gravity and material |
| `tools/worldgen.py` | generated worlds, and which chassis can survive them |
| C engine `world.c` | sunlight, size, the map styles |

`terrain.rs` already does the unendlich thing correctly and only for itself:
a dune wavelength is not authored, it is `u*²/g`. What is missing is that
nothing else reads those fields. The sky is a hand-picked enum, the day
length is a hand-typed number, and `worldgen.py` scores chassis off a CLASS
LABEL rather than off the world's properties meeting the bot's measured
parts.

## The fields we actually have, and what should read them

```
gravity        drives  crater regime, tube span, talus runout, fold time
                       (posture.rs), lift viability, jump height
air density    drives  saltation threshold, wing lift, rotor ceiling,
                       drag on every part (partclass aero)
wind           drives  dune wavelength AND whether dunes exist at all
material       drives  crevasse depth, repose angle, fracture spacing
sunlight       drives  — nothing yet. C engine has it; we dropped it.
size           drives  — nothing yet.
```

The last two are the honest gap. The C engine carries sunlight and world
size and our port reads neither, so day/night, thermal wind and the
temperature that decides ice from rock are all unavailable — and thermal
wind is what would make `wind` a derived field rather than a typed number.

## What this changes about the bot side

The point you made about part detection lands here. `worldgen.py` scores a
chassis by its class, which is why fifty-six grimorphs score identically.
The unendlich pattern says the score should be a READING: a world's air
density against a part's measured frontal area and through-flow; its gravity
against the part's measured inertia and reach; its wind against whether the
body has sealed intakes.

Every one of those measurements exists already — `partclass.aero` returns
drag area, through-flow, camber and angle of attack; `mesh_mass` returns
thickness and solidity; `voxel_symmetry` finds rotors. None of them is
plugged into placement. That is the same wiring gap as everywhere else in
this port, and it is the thing that would make a generated world's roster
differ WITHIN a class instead of only between classes.

## What does not transfer

The seven river archetypes, the bending integration, the dragon energy
model — all downstream of a fantasy setting we do not share. Taking the
archetype TABLE would be borrowing conclusions; taking the architecture is
borrowing the reasoning. Only the second is worth anything here.
