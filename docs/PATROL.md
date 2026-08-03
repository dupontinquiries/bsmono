# Patrol — the expanded brief

The one-line version was: *visit planets, use portals to other planets, fly
into outer space and asteroid fields, visit procedural worlds, and fight
enemies from the film script.* This is that, worked out far enough to build
against, with what already exists marked.

## What already exists, and is doing more work than it looks like

**Portals** (`world.rs`). Real ones: oriented, paired exits, and mounts that
carry their hole with them when they move. They were built for arena tricks
and they are, unmodified, a space bridge. A jump between worlds is a portal
whose far side is generated with a different destination's numbers. Nothing
needs inventing; the bridge is a portal with a passport.

**Gravity fields** (`world.gravity_field`). Already per-position and already
direction-carrying — the camera follows it, the "grounded" test judges
steepness against it, and the tube interiors read as floor all the way
around. A 0.17 g moon and a 0 g asteroid field are values it already accepts.

**Flight** (`physics.rs`). `car.flight` makes a chassis never-grounded with
lift and nose thrust. Deep space is that with the lift term removed.

**Weapons and the grapple** (`weapons.rs`). Twelve verbs plus a hook that
pulls you to whatever it bites. In a field of tumbling rock, the grapple
stops being a weapon accessory and becomes the primary way to move.

**Patrol itself** (`patrol.rs`, this port). Destinations, public events,
archive drip, and the bridge.

## The loop

You are dropped somewhere and the place keeps producing things to do.

* An event spins up 15 s in, then on a cooldown, ~60% faster at night.
* Three kinds — SURGE (purge a quota at the beacon), HOLD (bank 18 s inside
  a 12 m ring), ONSLAUGHT (a heavier quota against a longer clock).
* Quota grows with clears. The world escalates around a player who is
  winning, which is not the same as a difficulty setting and does not feel
  like one.
* Clearing pays energon; every third clear pays a rare heroic drop.
* Missing the clock fails the event and resets the cooldown. Nothing hangs.
* The archive drips a fragment every 55–85 s. This is where the setting is
  delivered — while you are busy, one line at a time, never as a wall.

## The route

Thirteen destinations across four terrain kinds. Gravity is the real number
because `MAP_SCIENCE.md` makes it the driver: crater simple-to-complex
transition scales as 1/g, lava-tube roof span scales as 1/g, and talus
runout diverges wildly even though the angle of repose does not. Weather is
a hard gate — aeolian features need saltation, so moving air, not merely a
medium.

| kind | what it is | movement |
|---|---|---|
| `Surface` | a body with ground and gravity | drive, walk, fly |
| `AsteroidField` | tumbling rock, ~0 g, cover that moves | grapple and thrust |
| `DeepSpace` | no floor, no up | flight only |
| `Procedural` | generated from a seed | whatever it rolled |

Earth, Mars, Luna, Europa, Io, Titan, Glaciem, Carrion Drift, Helix,
Cybertron, the Outer Rim, the Shield Boundary, and Uncharted. Nine come from
the film material rather than being invented — the C engine modelled five
planets and said nothing about what a planet *is*; the lore names Europa's
listening post, Io's tide-forges, Titan's fuel rain, and the three clone
homeworlds, and never had anywhere to put them.

## The enemies

Four factions, all the script's:

* **Hijacks** — the erratic conquerors the coalition shield holds back.
  They belong at the boundary and on anything outside it.
* **Corrupted** — Jhiaxus' experiment. Earth, Europa, Cybertron: the places
  where something is wrong underneath.
* **Blackstar's clans** — they tax the outer rim, so they own Titan, Io and
  Carrion, and they are on the Outer Rim because he is already moving.
* **Coalition** — allies, mostly. Glaciem and Helix.

This is the hook the weapons already want: `Element::advantage` is
implemented and tested and currently called by nothing, because it needs a
defender with an element. An elemental faction is that defender.

## What is not built yet, in the order I would build it

1. **Bridge → world regeneration.** `World::generate` takes a seed, a style
   and a gravity scale. Wiring a destination's numbers into it is the single
   highest-value step and probably a day: it turns thirteen table rows into
   thirteen places.
2. **Zero-g movement.** `flight` minus lift, plus the grapple as the primary
   verb. The asteroid field is the test case and it is the most novel thing
   in the list.
3. **Hostiles.** `bots.rs` already drives AI cars through the same physics.
   A faction is a bot with an element and a spawn rule; `credit_kill` is
   already waiting for them.
4. **Procedural destination.** Generate the destination row itself — gravity,
   weather, faction — then let 1 build the world from it.
5. **Raids as patrol's endgame.** `docs/RAIDS.md` has nineteen bosses with
   phase structure and mechanics; a raid is a patrol destination that does
   not spawn public events.

## The honest state

`patrol.rs` is the loop, the route and the bridge, with ten tests: events
pay out, a missed clock fails, a hold only banks inside the ring, a seeded
patrol replays identically, vacuum has no weather, and the route reaches
past planets to rock, space and generated worlds.

It is not wired to anything. Nothing calls `update`, no destination has been
handed to `World::generate`, and no hostile exists to `credit_kill`. That is
step 1 above, and it wants the world generator rather than more of this
file.
