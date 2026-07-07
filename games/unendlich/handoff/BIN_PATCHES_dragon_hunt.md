# BIN PATCHES — dragon hunting (`sim::dragon_hunt`)
# The predation loop that drives the other cores; doesn't move/attack itself.

## Per dragon: one HuntState; step it when prey is in play
    use unendlich_engine::sim::dragon_hunt::{step as hunt_step, HuntState, HuntSenses, Prey, HuntPhase};
    let mut hunt = HuntState::idle();

    let hs = HuntSenses {
        prey: Prey {
            detected: prey_seen,                 // from dragon_observe (attended prey)
            bearing: prey_bearing_rel_body,
            dist2: prey_dist2,
            rel_height: prey_y - dragon_y,       // + above, − below
            fleeing: prey_is_fleeing,
        },
        hunger01: 1.0 - act.vitality.energy,     // from dragon_activity
        stamina01: air.stamina.level,
        airborne: air.is_airborne(),
        in_strike_range: prey_dist2 < STRIKE_RANGE2,
    };
    let h = hunt_step(hunt, &hs, dt);
    hunt = h.state;

## Route the emitted intents into the existing cores
    - h.wants_close  → steer locomotion (ground) or set ModeSenses target toward prey; on the
                       ground use dragon_locomotion desired_heading = bearing.
    - h.wants_stoop  → ModeSenses.target_below = true (dragon_mode emits AirIntent.dive → dragon_air Diving).
    - h.wants_strike → fire the weapon at h.aim_bearing: feed it as the attended target into
                       dragon_bending_ai (Perception.has_target + target_bearing) and/or scale
                       breath_attack by dragon_bending::potency.
    - HuntPhase::Feed → let dragon_activity run Feeding (refuels vitality); hunt disengages when sated.

## Notes
- It only hunts when `hunger01 ≥ 0.4`, gives up ~3s after losing sight, and holds phases via a
  dwell timer so it doesn't flicker stalk/pursue/strike.
- Pairs with dragon_activity (which already chooses Forage/Alert): activity says "I'm hunting",
  dragon_hunt sequences the actual stalk/stoop/strike.
