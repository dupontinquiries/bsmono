# BIN PATCHES — dragon basic interactions (eat / sleep / wake / hear)
# Frame-loop wiring; logic lives in tested lib cores. Builds on the locomotion +
# observe wiring from BIN_PATCHES_ground_locomotion.md.

## Per dragon: keep an ActivityState
    use unendlich_engine::sim::dragon_activity::{step as activity_step, ActivityState, Senses, Activity};
    use unendlich_engine::sim::rest_cycle::rhythm_for; // or Rhythm::Cathemeral for dragons
    let mut act = ActivityState::fresh();
    let rhythm = rest_cycle::Rhythm::Cathemeral; // territorial dragon default

## Hearing — build the reaction from nearby sound emitters
    use unendlich_engine::sim::dragon_hearing::{react, Noise};
    use unendlich_engine::sim::sound_masking::{Ears, Sound};
    let ambient = env.ambient_sound_floor;          // from env_frame::frame_env
    let ears = Ears { base_range: 50.0, sensitivity: 1.0 + 0.5 * alertness };
    let noises: Vec<Noise> = sound_sources.iter().map(|e| Noise {
        sound: Sound { loudness: e.loudness, freq01: e.freq01 },
        bearing: wrap(angle_to(e) - cr.yaw),
        dist2: e.dist2,
        occlusion: e.occlusion,
    }).collect();
    let heard = react(ears, &noises, ambient, act.asleep);

## Activity — one step decides eat/sleep/wake/alert
    let senses = Senses {
        threat: threat_level,                       // from threat_assess
        aggression, health_frac,
        food_in_reach: nearest_food_dist2 < BITE_RANGE2,
        day_phase,                                  // 0..1, matches rest_cycle
        heard,
    };
    act = activity_step(act, rhythm, &senses, dt);

## Drive the body + rig from the chosen activity
    match act.activity {
        Activity::Sleeping => {
            desired_speed = 0.0;                    // lie still
            cr.gaze_pitch = -0.4;                    // head lowered/resting (rig shows it)
            cr.anim_time += dt * 0.3;                // slow breathing only
        }
        Activity::Feeding => {
            desired_speed = 0.0;                    // stop at the food
            cr.gaze_pitch = -0.5;                    // head down to eat (jaw works on anim_time)
            cr.anim_time += dt;
        }
        Activity::Forage => { desired_heading = bearing_to(food_target); desired_speed = walk_speed; cr.anim_time += dt; }
        Activity::Alert  => { desired_heading = bearing_to(threat_or(heard.bearing)); desired_speed = 0.0; cr.anim_time += dt; }
        Activity::Idle   => { /* wander; desired_speed small */ cr.anim_time += dt; }
    }
    // then the locomotion + observe steps (previous patch). When attending a heard
    // noise, feed `heard.bearing` as a high-salience Stimulus to dragon_observe so
    // the head snaps toward the sound.

## Notes
- `act.vitality.energy` is the hunger meter (eating raises it in `Feeding`); show it
  on a HUD or gate breeding (`feeding::Vitality::can_breed`).
- `act.alert` (0..1) is a good `alertness01` for `dragon_observe`/`head_scan` and for
  `Ears.sensitivity` next tick (alert dragons hear better).
- `act.fatigue` recovers while `Sleeping`; a bang (`heard.should_wake`) or a threat
  wakes it; otherwise it sleeps through its rest phase and wakes at active-time.
