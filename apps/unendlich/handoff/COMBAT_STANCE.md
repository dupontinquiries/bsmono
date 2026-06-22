# COMBAT_STANCE.md — mobs fight smart: guard the weak side, manage distance

## Idea (from the user, expanded)
A mob in combat should ORIENT and SPACE itself based on its own per-limb health and which part of
it the enemy would hit:
- **Guard side:** turn so the HEALTHIER flank faces the threat — expose healthy limbs to absorb
  hits, keep the damaged / near-dead side away so it isn't finished off. (Stance left vs right.)
- **Distance by exposure:** keep MORE distance when vitals (face, body) would be exposed; close in
  when only a sturdy healthy limb fronts the foe. Protect the head/torso.
- **Turn rate:** rotate to the chosen stance faster under higher threat; ease off when safe.

## Why this rides on the body-part system
Per-limb HP and which part is forward already live in the body-part component
(`handoff/ENTITY_BODY_SEGMENTATION.md`). Stance is just a *policy* over that state, and it composes
with the existing AI cores (`threat_assess`, `flee_decision`, `threat_share`, `herd_cohesion`,
`pack_role`). Crucially, the SAME per-limb state drives part-aware meshing
(`handoff/BODY_PART_MESHING.md`): a chewed-up leg is both kept voxel-fine AND turned away from the
enemy. One source of truth, two payoffs (smarter fights + cheaper, more destructible visuals).

## The policy (cores delegated below)
- **guard_side**(left_hp, right_hp) → present the higher-HP side to the threat (−1 left / +1 right /
  0 square when even). Hysteresis band so it doesn't jitter when the two sides are close.
- **vital_cover**(facing_angle, threat_dir) → 0..1: how well head+torso are shielded by limbs in
  this orientation. Feeds distance.
- **engage_distance**(vital_exposure, threat_level, base) → standoff distance; rises with exposure +
  threat, floored at melee reach.
- **flank_turn_rate**(threat_level, angular_error) → angular speed toward the stance; snappier when
  threatened, damped near the target to avoid overshoot.
- **stance_bias**(overall_hp) → lean aggressive (close, square-on) when healthy vs defensive (more
  distance, side-on) when hurt — a scalar that scales engage_distance + guard_side strength.

## Things to add later (open — building on the user's "will let you add to this")
- **Pack-level stance:** flankers (`pack_role`) circle to the enemy's *exposed* side while the lead
  holds the front; coordinate via `threat_share` so they don't all crowd one flank.
- **Terrain-aware guard:** back the weak side into a wall/cliff (`cliff_detect`, `cave_room`) so it
  physically can't be flanked — turns level geometry into defense.
- **Per-attacker memory:** if a player keeps hammering the left leg, bias `guard_side` to
  over-protect it (decaying memory keyed by attacker).
- **Wing tuck:** fold a damaged wing (animation appendage cores: `appendage_ripple`, `wing_opacity`)
  — couples stance to the anim system and shrinks the wing's hittable area.
- **Vital-threat distance asymmetry:** keep more distance from ranged/breath attackers (they reach
  vitals anyway) but close on pure-melee foes where a healthy limb can tank.

## Delegated cores
- R26 (T01): `guard_side`, `vital_cover`, `engage_distance`, `flank_turn_rate`.
- R27 (T01): `stance_bias` (+ ties into `limb_detail_flag` from BODY_PART_MESHING).
Each pure + ≥6 tests per the ruleset; no `crate::render` import (R1). Deterministic — stance is a
function of HP + geometry, no hidden RNG.
