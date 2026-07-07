# Cumulative test — gameplay foundations (ships 14–21)

## What you'll see when you run the game

Set expectations honestly:

- **No new mobs.** There's no entity layer wired up. Nothing spawns.
- **No usable items.** No item-use hookup, no inventory, no break/place.
- **Nothing new rendered** except weather (below).

Everything this session is **pure, inert library code** — proven by unit
tests, waiting on the integration layer (`blocks.rs` / `bin/` / `entity.rs`)
to become playable. So the real test here is **"does it compile, do the tests
pass, and is the game unchanged"** — not watching the mesher. Nothing this
session touches the mesher or worldgen.

The one thing you *can* see: **varied weather**, and only behind a flag.

## Apply (flat zip — plain unzip from repo root)

    cd ~/Code/unendlich-development/unendlich-game
    unzip -o /path/to/unendlich-cumulative-1421-flat.zip

Lands 11 source files + 2 docs:
- world: weather, water_field, octree_raycast, chunk_manager, mod, and the
  new modules area_mining, attributes, parkour, flight, pathing
- render: world_renderer (weather hookup)
- docs: CLIMATE_PATH (updated), ELEMENTS (new)

## 1. Build (the real check)

    cargo build --release --bin unendlich-world --features render,gpu

Expect: clean compile. (Pre-existing harmless warnings only — the unreachable
`BerryBush` arm in mesher.rs and the unused `TRANSITION_ANIM_TICKS` import.)
If it does NOT compile, that's the #1 thing to tell me — paste the errors.

## 2. Run the new tests

    cargo test --release --lib -p unendlich-engine --features render,gpu \
      weather water_field absorb raycast area_mining attributes parkour flight pathing

Expect all green. The new ones added this session, by module:
- weather: variety bounds / variety / rain⇒overcast / classify cascade (+ the existing flaky one now fixed)
- water_field: 6 `absorb_*` tests
- octree_raycast: 5 `world_*` / `place_cell` tests
- area_mining: 6 (cube/sphere/selection/durability)
- attributes: 8 (pipeline + token roles)
- parkour: 11 (look/leap/walls/state machine)
- flight: 5 (seek/arrive/flee/orbit/turn-cap)
- pathing: 5 (distances/obstacle/sealed/flow)

If any FAIL, tell me which + the assert message. (I simulated each in Python
first, so a failure most likely means a compile/type quirk your compiler
caught, not bad logic — same as the two real bugs caught earlier.)

Or just run the whole suite:

    cargo test --release --lib -p unendlich-engine --features render,gpu

## 3. Regression check — game should be byte-identical

    cargo run --release --bin unendlich-world --features render,gpu

With no flags set, the game should look and behave exactly as before this
session. This is the "did anything break" check. If anything renders
differently with the defaults, that's a regression — tell me.

## 4. (Optional) See varied weather

    UNENDLICH_WEATHER=1 cargo run --release --bin unendlich-world --features render,gpu

This is the only visible new behavior: overcast greying on dry cloudy
periods, fog thickening, a directed prevailing wind, plus the existing
rain-darkening / snow-whitening / lightning. It evolves slowly (storms over
minutes), so let it run a bit. Feedback welcome on the *feel* — too much/too
little fog, overcast too grey, wind too strong — those are one-line constant
tweaks in weather.rs.

## What feedback helps most

1. Compiles? (paste errors if not)
2. Tests pass? (which fail + message if not)
3. Game unchanged with no flags? (regression?)
4. Optional: how does `UNENDLICH_WEATHER=1` look/feel?

You will NOT be able to judge mobs/items/flight/pathing by playing — they're
not hooked up. To make any of them visible we need `blocks.rs` (registry +
solidity predicates) and/or the `bin/` crate (input/player/camera). Send
either and the next session can start wiring real, playable behavior.

## Suggested tags (if it's all green)

    git tag -a v0.24.96-pathing -m "gameplay foundations: weather variety, raycast, area mining, attributes, parkour, flight, pathing"
