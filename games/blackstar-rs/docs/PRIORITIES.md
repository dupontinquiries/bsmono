# PRIORITIES

Rewritten at the end of a long session. The previous version accreted as
things got done and became hard to read; this is the state as it actually is.
The old one is in git if the reasoning is wanted.

## Where the game is

A playtest note partway through this session said, correctly: *"the game feels
mostly the same and still hasn't progressed to the point where I am even
walking around with a working robot nor in my spaceship being able to land on
planets nor being able to play campaign missions."*

That was true and is less true. What changed:

- The ship is **in the world** — drawn, solid, walkable, with doors that open
  as you approach and consoles that only work in the room they live in.
- The **starmap flies you somewhere** and loads the map for it.
- A **sortie flies its drop** before it resolves, and a bad landing costs the
  engagement.
- Two real physics bugs fixed: cars seated at the waist rather than the feet,
  and corridors drawn through doors the router called shut.

What has NOT changed: the mission itself is `MissionRun::resolve`, a
simulation. That is the boundary and it is now the only one.

## 1. The renderer boundary — the whole remaining list

Everything underneath is built and tested. What is missing is that almost none
of it is something you *do* rather than something that *resolves*.

- [ ] **The mission is a simulation.** `MissionRun::resolve` decides a sortie
      in one call. Playing one means a loaded world, an objective, an enemy
      and a way to fail — none of which exists as gameplay.
- [ ] **The drop is a number.** `drop.rs` flies a real descent and reports a
      miss distance. Nobody flies it.
- [ ] **`foothold::assess`** decides where you can put down and what it costs,
      and has never been asked at runtime.
- [ ] **`procgen::World::generate(seed)`** makes a landable world and is a
      DIFFERENT TYPE from `world::World`. Something has to bridge or replace —
      see `docs/UNWIRING.md`.

## 2. Walking as a robot — blocked on a decision, not on work

`--rescale` priced this. It is not a wiring job:

- `cfg car_size` clamps to 0.55–2.0 and a bot needs **22.4x**.
- Raising the clamp is wrong: every constant is off by a DIFFERENT exponent.
  Froude — length `k`, time `√k`, speed `√k`, **acceleration 1**, mass `k³`.
  Multiplying everything by `k` gives a bot that accelerates 22x too hard.
- And Froude fixes the exponents, not the input. The RL car does 61
  body-lengths per second; scaled, a bot does 12.8, against a cheetah's 4. The
  feel numbers have to be **re-derived**, not scaled.

- [ ] Decide a scale, apply the exponents, re-derive the feel. A day and a
      playtest.
- [ ] `board.rs::Avatar` models the two-scale switch. Wire it AFTER the
      constants — wiring first produces a 22x car and looks like the scale
      system is broken.
- [ ] Legged locomotion at all. Everything rolls. `cling.rs` knows five gaits,
      `cybertronian.rs` names them, the physics has wheels.

## 3. The ship as a Transformer — not started

The ship is meant to be its own Transformer and is a box that holds rooms.

- `imported_spaceship_praetor_aka_spaceship_with_partial_interior` — a ship
  model that already HAS an interior.
- `imported_reference_ship_parts_for_transformation_aka_star_sparrow_modular_spaceship`
  — parts authored for a ship that folds.
- 31 ship models, 21 `spaceship` class.

`xform.rs` folds a chassis, so the machine exists. **The new problem is that a
room is a carved void and a folding ship's voids have to go somewhere.** That
is the design work; the animation is not.

## 4. Built and unfinished

- [ ] **The voxel GAN has no training loop.** Critics, diagnostics, part
      families, the assembler score and the authoring surface exist and are
      tested. Training needs GPU hours. `--voxel`, `--assemble`,
      `--trainwatch`, `--bench`.
- [ ] **Reversed-Z is analysis only.** `--terrain depth` measures 3934x at the
      tower shot and nothing applies it. Same for logarithmic depth, which
      loses to reversed-Z at every range tested.
- [ ] **The gravity model is measured, not applied.** `--gravity` prices four
      errors including an INVERTED tube profile — spin gravity is zero on the
      axis, not maximum. Unblocked now the demo maps are deletable.
- [ ] **The demo maps are still wired.** `docs/UNWIRING.md` has the survey and
      the order; 44% of `world.rs` is per-map content.
- [ ] **Kuiper still has a floor** (`amp: 0.15`) and a test asserts it does, so
      removing it arrives with a failing test rather than silently.
- [ ] **342 warnings.** Mostly dead code from systems built and never called.
      Worth a pass once the renderer work settles — some of them are things
      that SHOULD be called.

## 5. Known-wrong, cheap, not done

- [ ] `style_for` maps destinations onto skydribble's eighteen styles by
      physics. None of them is Mars. Stopgap with its name on it.
- [ ] The ship parks at the world origin on every map. It should be somewhere
      the map chose.
- [ ] Two visual fixes from early in the session — the skipped transform frame
      and `ride_h` seating — are reasoned from code and pinned on pure logic.
      **Never verified on a GPU.**

## How to work on this

`tools/fastcheck` is the fast edit-test loop; `cargo ailite` is vendored at
`<repo>/tools/cargo-ailite`, so a fresh clone needs no setup. `skydribble --help` lists
about thirty drivers; each module has one and they are the fastest way to see
what a system does. The build prints `[ship] ...` to stderr at startup, which
settles "is the wiring live" in one line.

Two habits this session earned the hard way:

1. **A regression test written after the fix, by someone looking at the fixed
   code, will pass and prove nothing.** Put the bug back and watch it fail.
   Mine didn't, twice.
2. **A number written in metres does not move when the thing it measures
   does.** Three separate bugs this session — the hull, the corridor
   clearance, and a test asserting 15.2 — were all this one mistake.
   `--parity` scans for the general case.
