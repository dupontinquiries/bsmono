# BIN PATCHES — movement-mode arbiter (`sim::dragon_mode`)
# The glue that decides ground vs air and feeds the flight FSM. Sits above
# dragon_activity and below dragon_locomotion / dragon_air.

## Per-dragon: one ModeState; decide every tick
    use unendlich_engine::sim::dragon_mode::{decide, ModeState, ModeSenses, MovementMode};
    use unendlich_engine::sim::dragon_air::{step as air_step};
    let mut mode = ModeState::ground();

    // ... after dragon_activity gives `act` and perception is gathered:
    let msenses = ModeSenses {
        activity: act.activity,
        threat: threat01,
        target_above: target_rel_y > 4.0,
        target_below: target_rel_y < -4.0,
        target_dist2,
        airborne_threat: threat_is_flying,
        stamina01: air.stamina.level,
    };
    let m = decide(mode, &msenses, patrol_altitude, updraft_at(pos), dt);
    mode = m.state;

## Run the matching movement core
    match mode.mode {
        MovementMode::Air => {
            air = air_step(air, &fparams, &m.air, dt);   // m.air is the ready-made AirIntent
            // drive position from air.airspeed + air.vertical_rate; wing pose from air.flap01
            // if air.phase == Grounded after a landing, the next decide() will keep it on the ground
        }
        MovementMode::Ground => {
            // run dragon_locomotion (and let air relax toward Grounded if it was flying)
            // a pending AirIntent with wants_land=true brings it down first
        }
    }

## Behaviour summary
- Foraging / feeding / sleeping / idling ⇒ Ground.
- Alert ⇒ takes wing to face a flying threat, flee a serious ground threat, chase a
  target that's above or far, or hold altitude to stoop on a target below
  (`m.air.dive` fires for that case).
- Spent wings (`stamina01 < 0.1`) force `wants_land` no matter what.
- The mode is held for a few seconds (dwell) so the dragon doesn't take off and land
  every tick on a marginal call; urgent cases (flier, panic, exhaustion) switch at once.
