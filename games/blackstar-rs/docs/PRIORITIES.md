# PRIORITIES — getting to a game you can play

Written after a playtest that said, correctly: *"the game feels mostly the
same and still hasn't progressed to the point where I am even walking around
with a working robot nor in my spaceship being able to land on planets nor
being able to play campaign missions."*

## The honest accounting

That is right, and it is worth being precise about why rather than
apologising. Measured, in `main.rs` — the actual game loop:

| module | what it is | references in `main.rs` |
|---|---|---|
| `patrol` | the destination list — where you can go | **6** ✓ |
| `procgen` | world generation for those destinations | **0** |
| `foothold` | landing, approach, taking a site | **0** |
| `drop` | the descent | reachable via the starmap ✓ |
| `campaign` | missions | 1 |
| `raid` | raids | 1 |

The campaign stack is **complete, tested, and not plugged in**. It runs
entirely under `--sim`, `--terrain`, `--arch` and the other headless drivers,
which is why every session has ended with a number of passing tests and no
change to what happens when you press play.

I built a lot of things that are verifiable without a screen, and verifiable
without a screen turns out to correlate almost perfectly with *not in the
game*. The drivers are real work — the guidance comparison, the depth
analysis, the archaeology layer — and none of them is a step toward walking
around.

## What actually blocks each of the three things asked for

### 1. Walking around as a working robot

The transform exists (`xform.rs`), the chassis exist, and the seating bugs are
fixed. What is missing is that **the player is still a Rocket League car**:
`physics.rs` has one body with `half_h: 0.19`, tuned around a 38 cm chassis,
and `scale.rs` says a bot is 8.5 m. `board.rs` measured the consequence — a
human moves at 0.45 of a bot's pace and the maps are built for neither.

- [ ] **This is not a wiring job — `--rescale` prices it.** `apply_car_dims`
      already threads a `size` through every dimension, but `cfg car_size` is
      clamped to 0.55–2.0 and a bot needs **22.4x**. Raising the clamp is not
      the fix: at 22x every constant tuned around the 0.38 m body is wrong,
      and wrong by DIFFERENT EXPONENTS — speeds go as √k, accelerations do not
      move at all, mass goes as k³. Scaling everything by k gives a bot that
      accelerates 22x too hard.
      Then the harder half: Froude fixes the exponents and cannot fix the
      input. The RL car does 61 body-lengths per second; scaled, the bot does
      12.8, against a cheetah's 4. The feel numbers have to be RE-DERIVED, not
      scaled. A day of work and a playtest.
- [ ] `board.rs::Avatar` is the model for the switch; wire it after the
      constants are settled, not before.
- [ ] Legged locomotion at all. Everything today rolls. `cling.rs` knows about
      gaits, `cybertronian.rs` names five of them, and the physics has wheels.
- [ ] Third-person camera for a walking body — `scale::camera_for` computes it
      and `camera.rs` only reads the near plane from it.

### 2. A ship you can fly and land

- [x] **`patrol::ROUTE` is readable in-game.** `Station::Starmap` sits on the
      airlock — the one hub room that had no console — and lists all eighteen
      destinations with their gravity and surface, gated on the missions that
      reach them. Unreachable ones are GREYED rather than hidden, per
      `menu.rs`'s own note that "why can't I press this" beats a row that is
      not there. Pressing one says *"course laid for EARTH — 1.00g, Sandstone,
      weather. flying it is not wired yet"*, because a console that implies
      the loop is finished when it is not is worse than one that admits it.
- [x] **Flying the course, in simulation.** Pressing a destination now runs
      `patrol::Destination -> mapsim::ground_for -> drop::Drop` in sequence —
      the first time those three have run together rather than in three
      separate drivers. Earth reports *"down at 7 m from the mark at 45 m/s"*,
      and a test asserts a sixth of a g arrives slower, so the destination's
      physics genuinely reach the outcome.
- [ ] Flying it with a renderer attached. The sim resolves it; the boundary is
      the same one the missions have always sat behind.
- [ ] `procgen::World::generate(seed)` makes a landable world. It is a
      *different type* from `world::World`, which is the demo-map sandbox —
      see `docs/UNWIRING.md`. Something has to bridge or replace.
- [ ] `foothold::assess` already decides where you can put down and what it
      costs. It has never been asked at runtime.
- [ ] `drop.rs` has the descent physics; `--terrain drop` flies it headlessly.

### 2b. The ship as a place — DONE at the simulation layer

**Sized in occupant-heights, not metres.** A 70 m hull is a reasonable
starship for a person and 184 body-heights for the 0.38 m body this engine
flies. `HULL_HEIGHTS = 40` gives 15 m today and automatically 340 m the day
the player becomes an 8.5 m bot — which is what "Transformer scale" means,
since it is a RATIO rather than a length. One number, no second one to keep in
sync, and the rescale cannot miss it.

The ship was the right thing to build before the scale work, because none of
`--rescale`'s problem applies indoors: the rooms are authored, the scale is
fixed, and moving between them is a position and a walkable check.

- [x] **`interior.rs` is reachable.** It built a complete walkable ship —
      eight rooms, corridors, doors that seal, an SDF, fixture positions — and
      `main.rs` referenced it **zero times**. `aboard.rs` is the join.
- [x] **Consoles live somewhere.** `Station::cell()` has named a room since
      the stations landed and nothing used it. Being IN the room is what opens
      a console; there is no activation radius, because a radius is a number
      to get wrong and "you are not in there" is a reason a player can see.
- [x] **Power failure means something.** From the bridge with the doors shut,
      1 of 7 consoles is reachable. `interior.rs` has modelled sealing since
      it landed and could not stop anybody, because nothing asked where you
      were.
- [x] **There is geometry now.** `aboard::hull_mesh` builds one bevelled box
      per room and per OPEN corridor at their real positions — 1,020 triangles
      for a 70 m hull, which is nothing. Shut corridors are omitted, so the
      mesh and `reachable_from` agree about what is passable instead of being
      two answers to the same question.
- [x] **The ship is in the scene.** Uploaded once at launch and drawn every
      frame at the world origin — 26 m across, 10.3 m tall, one draw call.
      Drive to 0,0 and look.
      **What to check:** that it is there at all; that it is not inside the
      terrain; that the rooms read as rooms rather than one block. If it is
      invisible, the mesh slot or the draw is wrong; if it is a grey lump, the
      lighting params `[0.35, 0.0, 0.25, 0.0]` are the dial.
- [x] **`V` opens the console you are standing in.** It opened the Armory
      unconditionally before, because nothing could say where you were.
      Outside the ship it says so rather than opening an arbitrary one — a hub
      you can reach from anywhere is a menu.
- [x] **Tab walks reachable consoles only.** It cycled all seven regardless of
      sealed doors, which made `depower` cosmetic.
- [x] **Entering a room says what you can do there** — a toast on the
      transition, not a HUD line reprinted sixty times a second.
- [x] **It is solid.** `World::park` folds the interior's SDF into the world
      query, broad-phased on the hull box. Drive at it and you should stop;
      drive through a doorway and you should be inside. Both the draw and the
      collision read `ground_lift`, so what you see is what you hit — there is
      a test, because two transforms for one object is exactly how those end
      up a metre apart.
- [~] (was) The upload path exists and nothing calls it. `Aboard::upload_hull`
      registers the mesh with `Gpu::add_mesh` and remembers the slot,
      re-uploading when the doors move. It type-checks — `wgpu` is not
      feature-gated, so `render.rs` is in the build the tests run against —
      and that is as far as verification goes without a device.
      **Remaining: two lines.** Call it once on boarding, and `push` the slot
      with `Mat4::IDENTITY` in the draw loop the way the terrain chunks do
      (`main.rs`, the `M_TERRAIN_0 + ci` block). Both want eyes on the result,
      which is why I stopped here rather than writing them blind.

### 2c. The ship as a Transformer — NOT STARTED

The ship is meant to be its own Transformer and is currently a box that holds
rooms. The catalogue has the pieces:

- **31 ship models**, 21 of them `spaceship` class.
- `imported_spaceship_praetor_aka_spaceship_with_partial_interior` — a ship
  model that already HAS an interior, which is the hard part.
- `imported_reference_ship_parts_for_transformation_aka_star_sparrow_modular_spaceship`
  — parts authored for a ship that folds. That is the reference for this.

What it needs that nothing else has: `xform.rs` folds a chassis, and a ship
folding is the same machine at a different scale with rooms inside it that
have to go somewhere. That last clause is the real work — a room is a carved
void and a folded ship's voids move.

### 3. Campaign missions you can play

- [x] **A sortie flies to its destination before it resolves.** The Bridge
      offers the next mission, the board advances, and the report now reads
      *"the_pluto_line — clean drop on PLUTO — nightfall CLEARED"*. The chain
      is `MISSIONS.dest -> patrol -> ground_for -> drop -> MissionRun`.
- [x] **Arriving badly costs something.** A scattered landing takes up to 35%
      off the engagement's power via `MissionRun::arrived_badly` — a named
      method rather than a public field, because `power` is resolved once and
      the invariant is that only a named cost moves it. This is the first
      thing in the game where getting somewhere badly has a consequence at the
      far end.
- [ ] The mission itself is still `MissionRun::resolve` — a simulation, not
      something you play. That is the renderer boundary, and it is the same
      one the whole campaign sits behind.
- [ ] `stations.rs` gives the ship consoles that plot and resolve — the
      Chartroom loop works — but resolution is a simulation, not a mission.
      The renderer is the boundary and always has been.

## The order I would actually do it in

1. **One mission, end to end, badly** — DONE in simulation. Five sorties run
   in sequence from the Bridge, each flying to its own world:

       the_pluto_line     — clean drop on PLUTO — sortie CLEARED
       the_red_extraction — clean drop on MARS  — sortie CLEARED
       the_beach_opens    — clean drop on EARTH — sortie CLEARED

   The board advances, the drop reads the mission's destination, and a bad
   landing costs the engagement. What has never happened is any of it with a
   renderer attached — which is still the honest boundary and is now the ONLY
   one.
2. **The player body at two scales**, because everything else is unplayable
   until walking works.
3. **The starmap**, so `patrol` becomes reachable.
4. **Delete the demo maps** per `docs/UNWIRING.md`, which unblocks the real
   gravity model and the space map.
5. Everything else.

## What to stop doing

Adding modules with drivers. There are now thirty-odd `--flags` and the number
of things you can do by pressing play has not moved. A new system should be
justified by a mission that needs it, not by being measurable.

## Standing caveats

- Audio needs `libasound2-dev`; `--no-default-features` builds silent.
- The two visual fixes this session (skipped transform frame, `ride_h`
  seating) are reasoned from code and pinned with tests on the pure logic.
  **Not verified on a GPU** — first thing to check on device.
- Balance numbers throughout are guesses and say so; `--tune guesses` lists
  the ones with nothing behind them.
