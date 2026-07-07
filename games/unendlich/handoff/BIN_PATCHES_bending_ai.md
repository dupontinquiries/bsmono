# BIN PATCHES — dragon bending AI, gesture arcs, abilities, variation
# Frame-loop wiring; all logic is in tested lib cores. Builds on
# BIN_PATCHES_bending.md (capability/potency) and the observe/activity patches.

## 1. Targeting through the EYES (not a precise solver)
The dragon may only bend at what it is *looking at*. Take the target straight from
`dragon_observe` (the attended stimulus + the gaze bearing), not from a global
aim-at-player query:
    use unendlich_engine::sim::dragon_bending_ai::{step as bend_ai, BendIntent, Perception};
    let obs = /* ObserveStep from dragon_observe this tick */;
    let perc = Perception {
        has_target: obs.attending.is_some(),         // eyes locked on something
        target_bearing: cr.gaze_yaw,                  // where the eyes point (relative to body)
        target_dist2,
        willing: (act.alert).max(threat01),           // dragon_activity alert / threat
        alertness01: act.alert,                       // sharper ⇒ truer aim
    };
    let res_tier = resonance_state::tier(&bender.resonance);
    let s = bend_ai(intent, &genome, ambient, env_weight, res_tier, &perc, frame_seed, dt);
    intent = s.intent;
- `step` charges only while the eyes hold a target and it's willing; otherwise the
  wind-up relaxes. The release `aim_bearing` is the gaze bearing **plus natural
  error** (`aim_error`) that widens when the dragon is calm or the target is far —
  so it misses like a creature, never like a turret.

## 2. The gesture-arc equivalent (genetics-governed) + the telegraph
`signature_arc(&genome)` is the dragon's analogue of the player's gesture quadruple
(`gesture_resolve` Stance/Gesture/Directional/Element) — chosen by its body:
winged ⇒ Aerial, heavy ⇒ Surge, long tail ⇒ Sweep, long legs ⇒ Ground. Drive the
**wind-up** off `intent.charge01` (0→1) so there's a visible tell before the cast:
rear back / inhale / glow the maw as it climbs, snap on release. Same grammar as
the player, so dragons and players read the same.

## 3. Special abilities = element archetype × technique (field brushes)
Per docs/ELEMENTS.md an element is a *field brush*; the released `TechniqueShape`
(`kind` = element, `scale`, `range`, `radial`) scaled by `potency` says how hard to
perturb the field. The named abilities fall straight out of the grammar:
- **Earth pillar** — Earth + Rooted + ChargeRelease + Ground/Radial → push the
  terrain-height field up under the aim point (radial ⇒ ring of pillars).
- **Fire ash smokescreen** — Fire + Surge + ChargeRelease + Sweep → inject heat +
  particulate (an obscurant) along a wide arc; the climate field carries the cloud.
- **Water** — Flow + Sustain + Pull → draw the water/moisture field toward the dragon.
- **Air** — Aerial + ChargeRelease + Lift → push the wind/pressure field upward.
On `s.release`, map `technique.kind` to the field the brush writes, the aim to
`aim_bearing`, and the magnitude to `release.potency * technique.scale`.

## 4. MORE variation — unique seeds + bred populations
The genome already rolls the *whole* body-design space per individual; the only fix
is to stop reusing seeds. Give every spawned dragon its own seed:
    use unendlich_engine::sim::dragon_population::{founders, next_generation};
    let pop = founders(world_seed ^ region_id, 24);     // 24 distinct dragons
    for (g, pos) in pop.iter().zip(spawn_points) {
        world.spawn(spawn::spawn_dragon_with_genome(pos, g));
    }
- Do **not** call `spawn_dragon_element(fixed_seed, elem, ..)` for every dragon of an
  element — that's what makes them clones. Use unique seeds (or the population above).
- As dragons breed, feed survivors into `next_generation(&survivors, n, seed)` so
  traits recombine and the population drifts — variety builds over generations.
