# BIN PATCHES — dragon ground locomotion + observation
# (frame-loop call sites; the logic is in tested lib cores.)

## Locomotion — `sim::dragon_locomotion`
Cache params once per dragon (from its genome), keep a `GroundState` on each:
    use unendlich_engine::sim::dragon_locomotion::{params_from, step, GroundState};
    let lp = params_from(&genome);                 // at spawn
    // ground state can live alongside the creature; seed from current yaw:
    let mut gs = GroundState::at(cr.yaw);

Each tick, feed the AI/player's desired heading + speed (from wander / pursuit /
flee — your existing brain picks these):
    let out = step(&lp, gs, desired_heading, desired_speed, dt);
    gs = out.state;
    cr.yaw = gs.heading;                            // rate-limited turn (no snap)
    cr.gait_phase = gs.gait_phase;                  // legs step in cadence with speed
    // move the body forward `out.forward` metres along the heading:
    cr.body.position[0] += out.forward * cr.yaw.cos();
    cr.body.position[2] += out.forward * cr.yaw.sin();
    // bank the body into the turn (feeds the existing posture model):
    let posture = unendlich_engine::sim::body_posture::target_from_feet(
        foot_h, track_width, wheelbase, out.turn_rate);
    cr.settle_posture(posture, 6.0, dt);

This REPLACES the old ad-hoc `cr.gait_phase = (cr.gait_phase + dist*2.5) % TAU`
(unendlich_world.rs ~1957). `out.gait` (Idle/Walk/Trot/Gallop) is available if you
want gait-specific footfall sounds or dust.

Minimal variant (keep your own physics): just use `step` for `cr.yaw` (turn limit)
and `cr.gait_phase` (stride cadence), and ignore `out.forward`.

## Observation — `sim::dragon_observe`
Build a stimulus list each tick from what the dragon can perceive (prey, threats,
sound sources — reuse `sound_masking::can_hear`, `threat_assess`, perception):
    use unendlich_engine::sim::dragon_observe::{observe, Stimulus, GazeParams, Gaze};
    let stimuli: Vec<Stimulus> = visible.iter().map(|e| Stimulus {
        bearing: wrap(angle_to(e) - cr.yaw),        // relative to body heading
        salience: e.salience,                       // prey/threat/loudness 0..1
        dist2: e.dist2,
    }).collect();
    let gp = GazeParams { alertness01, max_yaw: 1.4, track_half_life: 0.18 };
    let mut gaze = Gaze { yaw: cr.gaze_yaw, pitch: cr.gaze_pitch, neck_bend: 0.0 };
    let o = observe(&gp, gaze, &stimuli, anim_time, dt);
    cr.gaze_yaw = o.gaze.yaw;                        // NEW: the rig turns the head
    cr.gaze_pitch = o.gaze.pitch;                    //      and neck toward this
    // o.attending: Option<usize> — index of the thing it's watching (drive AI focus)

`cr.gaze_yaw/gaze_pitch` are now read by the forge rig, so the dragon visibly
turns its head to track what it attends to, and idle-sweeps when nothing is
interesting. Leave them at 0 to keep the old anim_time idle-scan.

## Still advance the idle clock (from the earlier rig patch)
    cr.anim_time += dt;   // wing-beat / jaw / breathing, independent of locomotion
