# BIN PATCHES — dragon spawn variation + size (and feet)

## THE SIZE / VARIATION BUG (root cause)
With `UNENDLICH_SPAWN_ARCHE=dragon`, every spawn currently goes through the FIXED
picker blueprint:
    let bp = creature_design::EggSpec::chosen(arche, wseed).hatch();
    (CreatureKind::Dragon, spawn::from_blueprint(&bp, s.pos))
`chosen(Arche::Dragon, ..)` pins the body plan to one design, so the body — and
therefore the size — is identical for every dragon no matter what `wseed` is. The
genome forge path (which rolls the full body-design space per individual) is bypassed.
That's why "they're all the same size."

## FIX — route dragons through the genome forge path
Replace that branch (for the dragon archetype) with the varied genome spawn:
    let (kind, mut cr) = if let Some(arche) = spawn_arche {
        if matches!(arche, Arche::Dragon) {
            // per-individual genome → varied proportions, size, element, and the
            // forge rig. unique seed per spawn already in `wseed`.
            (CreatureKind::Dragon, unendlich_engine::spawn::spawn_dragon_arche(wseed, s.pos))
        } else {
            let bp = creature_design::EggSpec::chosen(arche, wseed).hatch();
            (CreatureKind::Dragon, unendlich_engine::spawn::from_blueprint(&bp, s.pos))
        }
    } else {
        (s.kind, unendlich_engine::spawn::spawn(s.kind, s.pos))
    };
- `spawn_dragon_arche(wseed, pos)` = `spawn_dragon_with_genome(pos, &dragon_archetype::Genome::from_seed(wseed))`,
  which sets `body.half_extents` from the genome (now a wide 0.6×..1.7× range) and
  attaches the forge model. Each `wseed` is already unique per spawn location, so
  sizes and shapes now span the full range.
- For breeding-built populations, use `dragon_population::founders` /
  `next_generation` to source the genomes instead of independent seeds.

## FEET — never rig/contact wings as feet
The forge rig now separates wings from legs (a height-line barrier). Ground contact
and posture must use leg joints only:
    use unendlich_engine::sim::dragon_forge::is_leg_joint;
    let feet: Vec<_> = skinned.iter().filter(|b| is_leg_joint(b.joint)).collect();
    // grounded_y / posture: lowest LEG box, not the lowest box overall (which could
    // be a drooping wing).
Wings still collide/interact — just not as the main feet.

## Already handled in the lib (no bin work)
- Breathing, blinking, and head micro-motion ride on `creature.anim_time` (already
  advanced each frame) — `skin()` produces them automatically now.
- Limbs deform with linear-blend skinning (taper from a static root to a full-swing
  tip), so they no longer come apart into disjoint cubes.
