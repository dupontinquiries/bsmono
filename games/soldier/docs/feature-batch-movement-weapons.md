# Feature batch — movement, jetpack, exotics, creatures

A large gameplay pass. Summary of what changed and where.

## Movement tuning & new states (`src/player.rs`)
- **Sprint toned down** from a 47% boost over walk to ~22% (`SPRINT_SPEED` 10.6 → 8.8) — applies in every mode.
- **Dive toned down**: shorter (0.62s → 0.5s), lower hop (4.6 → 3.4), and a gentler lunge (1.7× → 1.35× sprint).
- **Crawl / prone** (`prone_t`, `Input::crawl`): hold to go fully prone — lowest eye height, smallest hitbox
  (`body_height` shrinks further), a slow crawl (~60% speed penalty on top of crouch), and no sprinting while
  prone. Bound to **shift** by default at the app layer (sprint moved off shift onto its keybind; the super
  ability moved off X to Z to free things up). Getting up/down is a deliberate, slightly slow commitment.
- **Crouch/prone animation**: the third-person body feeds `crouch_t.max(prone_t)` into `Pose::moving` so the
  figure visibly sinks and bends.

## Jetpack rebalance + ion VFX (`src/player.rs`, `src/particles.rs`, `src/game.rs`, `src/app.rs`)
- The jetpack was unlimited hover. Now it burns a **charge/fuel bar** (`jet_fuel`): ~1.4s of continuous thrust,
  refilling over ~2.5s on the ground. Empty = no lift until it recovers — burst mobility, not free flight.
- **Ion-blast VFX**: a new `Kind::Ion` particle and `Particles::ion_thrust` emit a downward cyan-white plume
  from twin nozzles while thrusting, plus a pulsing blue point-light under the player. A HUD **ION THRUST**
  gauge shows the charge (pulsing while firing).
- **Hunt-mode separation**: The Hunt now strips the jetpack perk entirely (`player.jetpack = false`), because
  bringing it in unbalanced the mode. Hunt movement comes from the creature/role kit, not loadout perks.

## New exotic creatures for The Hunt (`src/creatures.rs`, `src/enemy.rs`, `src/game.rs`)
- **Storm Kraken** — a flying lightning octopus-kraken: hovers (reduced gravity when played as predator), the
  longest reach of any creature, and its strikes **arc lightning** (ion sparks + a flash light). Distinct body:
  a stretched mantle, eight swaying tentacles, glowing eyes.
- **Widow** — a creepy giant spider: the fastest creature, a fanged medium-reach lunge. Distinct body: a
  bulbous marked abdomen, eight two-segment splayed legs, a fang/eye cluster.
- Both plug into the existing creature stat interface (`speed_mul/reach/damage/strike_cd/lunge/color`) plus new
  `flying()` and `electric()` flags, and the survivor-hunt AI takes the chosen creature's silhouette.

## Exotic weapons (`src/weapons.rs`, `src/game.rs`, `src/app.rs`)
Three new weapons with genuinely new mechanics, added to `WeaponKind`, `ALL`, `PRIMARIES`, all match arms, and
with distinct viewmodels/colours:
- **TESLA-ARC (Arc Coil)** — `chain: 3`. The hitscan bolt connects then **arcs** to up to three nearby enemies,
  each jump doing 0.7× the previous damage with an electric tracer + ion spark. Rewards clustered targets.
- **CRYO-LANCE** — `pierce: true`. A precise beam that **passes through** every enemy on its line, hitting all
  of them at full damage. Line up a row and skewer it.
- **VOID-MAUL** — a heavy exotic **melee**: longer reach than the EM sword (3.6), brutal damage (180), slow
  swings.
`WeaponDef` gained `chain: u32` and `pierce: bool` fields to drive these.

## Tests
New tests cover: prone (lowers body, slows, blocks sprint, stands back up), jetpack fuel (drains thrusting,
cuts out empty, recharges grounded), hunt strips the jetpack, the two exotic creatures' signatures, and the
Arc Coil chain + Cryo Lance pierce actually hitting a second enemy. All green except the long-standing
pre-existing buoyancy failure in `player.rs` (untouched here).

## Still open from the request (not yet done this pass)
More assets/textures, connected textures, sandbank improvement on the quake map, building mesh-deformation /
destructibility, per-map unique live events, and the loadout/character-view editor polish. These are the next
targets; this pass focused on movement feel, the jetpack balance the user flagged, the creatures, and exotics.
