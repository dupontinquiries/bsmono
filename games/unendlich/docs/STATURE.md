# STATURE.md — height as the master physical scalar

The north star, mirroring `ELEMENTS.md`'s "one rule, everything emerges":
a character's **stature (height in blocks)** is the single dial from which the
rest of their *physical* identity is derived. You don't hand-tune a titan's HP,
mass, step height, and turn radius independently — you set them tall and the
body-scaling laws produce a slow, heavy, ground-shaking giant; you set them
short and the same laws produce a light, nimble, buoyant sprite. Two extra genes
(**bulk** and **density**) and the existing **strength**/health economy fold in,
but height leads.

Default **1.8 blocks** (a person). Range **0.1 → 25.0** (a knee-high familiar to
a building-sized colossus).

```
                 height (blocks, 0.9 .. 15.0, default 1.8)
                          |
        +-----------------+---------- bulk (girth)  ---- density (g/cm³ vs water)
        |                 |                 \              /
     collider          mass = k · h³ · bulk · density   buoyancy = (1 - density)
     + eye height          |
                +----------+-----------+----------+-----------+
                |          |           |          |           |
            max_health  hunger_rate  strength  knockback   knockback
            ∝ mass^a    ∝ mass^¾     ∝ h·str    _dealt       _resist
                                                ∝ strength   ∝ mass
        +-------+--------+---------+----------+-----------+-------------+
        |       |        |         |          |           |             |
   step_height jump_vel move_speed turn_rate ground_shake wind_resist  water_resist
   ∝ h         ∝ √h     ∝ h^0.7   ∝ 1/√h     ∝ mass^½     (thresh ∝ mass, (thresh ∝ mass,
                                                            mult ∝ 1/h)   mult ∝ 1/h)
```

## Why these exponents

Bodies scale by **volume** (mass ∝ h³ at fixed shape), and most "feel" comes
from how forces compare to that mass:

| Attribute            | Law                          | Intuition |
| -------------------- | ---------------------------- | --------- |
| mass                 | `k · h³ · bulk · density`    | volume × girth × material |
| buoyancy             | `1 − density` (signed)       | < water → floats, > water → sinks |
| max_health           | `base · (h/1.8)^2.2 · str`   | bigger = more to kill, super-linear but < mass |
| hunger_rate          | `(mass/base)^0.75`           | Kleiber metabolic scaling — giants burn more |
| strength             | `base · (h/1.8) · str_gene`  | reach (block break/place)             | r569 ✅ LINEAR 6 + 1.2·(h−1.8) so the eye-to-ground ray fits (√ fell short — a 25-block eye couldn't touch the floor) |
| knockback_dealt      | `∝ strength`                 | a giant's shove launches you |
| knockback_resist01   | `mass/(mass+base)`           | heavy bodies barely budge (0..1 negated) |
| step_height          | `base_step · (h/1.8)`        | a giant steps over a wall |
| jump_velocity        | `base · (h/1.8)^0.5`         | taller jumps higher *absolutely*, lower *relative* |
| move_speed           | `base · (h/1.8)^0.7`         | longer stride, sub-linear (mass drags) |
| turn_rate (rad/s)    | `base · (1.8/h)^0.5`         | inertia widens the turn radius |
| ground_shake         | `(mass/base)^0.5`            | per-footfall camera-shake + earthquake + erosion |
| wind_resist thresh   | `(mass/base)` (min gust)     | wind must overcome inertia to budge you |
| wind_resist mult     | `(1.8/h)`                    | exposed area/mass falls with size → less drift |
| water_current thresh | `(mass/base)`                | same shape as wind, in the water-current field |
| water_current mult   | `(1.8/h)`                    | |

`base` quantities are the 1.8-block person, so a default character derives back
to today's tuning (collider 0.6×1.8, eye 1.62, jump 10, walk 4.5, step 1.1) and
nothing changes for them. Everything is `(h/1.8)`-relative so the laws are
shape-anchored at the human.

## Ground-shake → world coupling

`ground_shake` is the bridge into the field systems (the `ELEMENTS.md` substrate):
each footfall of a heavy body is a stimulus scaled by `ground_shake`:
- **camera shake** — a brief positional jitter (own camera + nearby players).
- **earthquake** — a transient added to the same shake the bending/impact code
  already feeds the camera.
- **erosion** — a small weathering / displacement nudge to the blocks under the
  foot (a colossus leaves prints), routed through the same block-edit path
  bending uses. Gated so a 1.8 person contributes ≈0.

## Where it lives

- **Pure core:** `sim::stature` — `StatureProfile` + `derive(height, bulk,
  density, strength) -> StatureProfile`. Pure, deterministic, no I/O, tested
  (monotonicity + the 1.8 anchor). This file is the spec for that module.
- **Source of stature:** `player_character::BuildParams.height` (now in blocks,
  default 1.8, slider 0.1..50.0); `hero_forge::hero_total_height` returns it.
- **Consumers (the wiring):** the bin derives one `StatureProfile` at character
  spawn / re-pick and pushes it into physics (collider, eye, mass), the vitality
  economy (max HP, hunger), movement (speed/jump/step/turn), combat (knockback),
  and the footfall → shake/erosion path.

## Wiring status (updated as rolls land)

| Consumer                              | Status |
| ------------------------------------- | ------ |
| `sim::stature::derive` + tests        | r559 ✅ |
| height default 1.8, range 0.1..50.0   | r559 ✅ |
| semi-log tallness slider + value      | r561 ✅ |
| collider half-extents from profile    | r561 ✅ (resized on character change, feet kept planted) |
| eye height (camera at face) from prof. | r561 ✅ (runtime eye-offset replaces the const at all camera syncs) |
| move_speed / jump / step              | r561 ✅ |
| max_health (→ dmg-resist) / hunger    | r563 ✅ |
| turn                                  | pending (player look is instant mouse — turn_rate is for the avatar/AI) |
| knockback dealt                       | r566 ✅ (punch shove) / resist: pending (no player-melee-kb site yet) |
| ground_shake → camera (land + footfall)   | r566 ✅ |
| ground_shake → stomp erosion          | r571 ✅ (crush capacity ≈ h/8 vs block hardness; pulverises path, weathers harder, bedrock stops; trenches at titan scale) |
| wind / water-current resist           | r566 ✅ (wind_resist on PhysicsParams + current threshold/mult) |
| buoyancy / density → water physics    | r567 ✅ (density tracks the muscle slider; modulates swim float) |

The profile is computed and authoritative from r559; consumers are wired
incrementally (each is a contained edit reading fields that already exist on the
profile), so the laws can be tuned in one place without re-touching call sites.
