# Road Warrior backlog

Raised in conversation, not started. Recorded here so it survives the
session rather than living in a chat log.

## Menus, borrowed from unendlich's UI

The game currently has no menus at all: it boots straight into whatever
level the save points at, with the car chosen by a heuristic over
unlocked cars (see the comment in `main.rs` — there is no car-select
screen, which is why Behemoth and Pillager were unlockable but never
driveable).

Wanted:

- single player
- multiplayer over LAN — **check bsgames integration**, it already
  solves discovery and lobby for the other games in this repo, and a
  second implementation would be the wrong move
- map select
- car select — this one also closes an existing gap rather than adding a
  new feature
- weapon select
- gas and flip bindings, up and down

`games/unendlich/crates/unendlich-ui` is the reference. Its engine
crate's style — one small pure module per concept, tested headlessly — is
already what `road_warrior_core` follows, so the UI layer should be
borrowable without dragging the rest of unendlich in.

## Maps that curve, and maps that surprise

Two related asks.

**Curves.** `Track` is currently `Flat | Ramp | Gap`, all straight
segments — a ramp is a linear interpolation between two heights. Real
drive lines curve. This means either a new `Segment` variant with a
curve parameter, or replacing the segment list with a sampled
heightfield that segments compile down to. The second is more work and
would make the terrain shader's job easier, since it already samples
`height_at` per column and does not care how the height was produced.

Note the knock-on: `lighting.rs` bakes sun visibility and sky openness by
ray-marching `height_at`, so it gets curved terrain for free — but the
bake's `STEP` of 0.5 world units was chosen against an 8-unit ramp being
the smallest feature. Curves with tighter radii need a finer step, and
the bake cost scales with it.

**Random maps.** A generator with interesting noise, producing varied
heights, so a run tests adaptability rather than memorisation. The point
is explicitly that the map is a surprise every time.

Design notes worth keeping in mind when this is picked up:

- It has to stay a function of a seed. `BakedLighting` is computed once
  per level at load, and a map that cannot be reproduced from a seed
  cannot be baked, shared over LAN, or put on a leaderboard.
- Generated maps need a completability check. Level one is already
  provably completable only because `sim.rs`'s `level_one_playability`
  tests measure it: full throttle clears the gap, a third of a second of
  tilt lands clean. A generator that emits a gap the fastest car cannot
  clear produces an unwinnable run, and only a test that actually
  simulates the jump will catch it.
- The fuel budget is the other constraint. Running dry is now a level
  retry rather than a softlock, but a generated map longer than a tank
  is still unplayable. `Vehicle::gas_capacity / gas_burn_rate` times
  `top_speed` is the reachable distance; a generator should respect it.
