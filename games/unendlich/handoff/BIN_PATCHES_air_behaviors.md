# BIN PATCHES — dragon air behaviours (`sim::dragon_air`)
# Frame-loop wiring; flight logic is in the tested lib core. Complements the
# ground batch (dragon_locomotion / dragon_activity).

## One AirState per dragon; pick ground vs air by phase
    use unendlich_engine::sim::dragon_air::{step as air_step, params as air_params, AirState, AirIntent, FlightPhase};
    let fparams = air_params(&genome);     // from the genome's wings/mass
    let mut air = AirState::grounded();
    // each frame:
    air = air_step(air, &fparams, &intent, dt);
    if air.is_airborne() {
        // drive position from air.airspeed (forward) + air.vertical_rate (up);
        // skip the ground locomotion step this frame.
    } else {
        // on the ground → run dragon_locomotion as before.
    }

## Build AirIntent from the AI / activity
    let intent = AirIntent {
        wants_fly: want_to_be_airborne,         // fleeing, hunting flying prey, patrolling territory
        wants_land: tired_or_arrived,           // low stamina + no threat, or reached a perch
        target_altitude: patrol_altitude,       // cruise height for this dragon
        updraft: wind_field::updraft_at(pos),   // thermals / ridge lift (see note)
        dive: attacking_target_below,           // commit to a stoop on prey
    };
- `wants_fly` can come from `dragon_activity` (e.g. Alert + airborne threat, or a
  hunt) and `dive` from `dragon_bending_ai`/attack when the attended target is below.

## Rig + VFX
- `air.flap01` (0..1) drives the wingbeat amplitude/rate in the forge rig (replaces a
  fixed wing animation while airborne); `Gliding`/`Soaring` ⇒ wings held out, near 0.
- `air.mode` (`Stalled`/`Gliding`/`Level`/`Climbing` from dragon_flight) is a good tell
  for VFX (wingtip vortices on hard climbs, etc).
- `air.phase` picks the body pose: TakingOff/Climbing nose-up, Diving nose-down,
  Landing flare, Soaring level with spread wings.

## Thermals/updraft (optional, makes soaring real)
`updraft` is the vertical air speed at the dragon. Cheapest source: a positive bias
over warm/low terrain + ridge lift where wind hits a slope (read `wind_field` +
terrain normal). Without it the dragon still flies; it just can't soar to save its
wings and will glide/flap-cycle instead.

## Notes
- Stamina recovers while Soaring/Gliding/Grounded and drains while flapping hard
  (TakingOff/Climbing); a tiring dragon levels off to Cruise (cheap flapping) and only
  Glides when truly spent — so it doesn't stutter between flap and glide.
- Phases hold (no per-tick flicker) the same way the ground activity FSM does.
