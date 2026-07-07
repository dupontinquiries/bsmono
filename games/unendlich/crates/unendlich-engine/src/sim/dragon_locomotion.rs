//! `sim::dragon_locomotion` — the ground-movement runtime: turn-rate-limited
//! heading changes, speed accel/decel, and a gait-phase cadence so the legs step
//! in time with how fast the dragon is actually moving. Genome sets stride,
//! top speed and agility. Feeds the forge rig (`gait_phase` → leg swing) and
//! `body_posture` (the turn rate banks the body). Pure, deterministic.

use crate::sim::dragon_archetype::Genome;
use std::f32::consts::TAU;

#[derive(Clone, Copy, Debug)]
pub struct LocoParams {
    pub max_speed: f32, // m/s
    pub accel: f32,     // m/s²
    pub turn_rate: f32, // rad/s (max yaw change)
    pub stride: f32,    // metres per full gait cycle (one TAU of gait_phase)
}

/// Ground movement state carried on the creature between ticks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GroundState {
    pub heading: f32,
    pub speed: f32,
    pub gait_phase: f32,
}

impl GroundState {
    pub fn at(heading: f32) -> Self {
        GroundState { heading, speed: 0.0, gait_phase: 0.0 }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GaitKind { Idle, Walk, Trot, Gallop }

#[derive(Clone, Copy, Debug)]
pub struct LocoStep {
    pub state: GroundState,
    /// Actual angular velocity applied this tick (rad/s) — feeds posture bank.
    pub turn_rate: f32,
    /// Distance moved forward this tick (for the position integrator).
    pub forward: f32,
    pub gait: GaitKind,
}

/// Genome → locomotion params: longer legs stride further and run faster; bulkier
/// dragons are a touch slower and turn wider.
pub fn params_from(g: &Genome) -> LocoParams {
    let leg = g.leg_len.clamp(0.0, 1.0);
    let bulk = g.bulk.clamp(0.0, 1.0);
    let max_speed = (2.5 + leg * 4.0) * (1.0 - 0.2 * bulk);
    LocoParams {
        max_speed,
        accel: max_speed * 1.5,
        turn_rate: 1.2 + (1.0 - bulk) * 1.6,
        stride: 1.0 + leg * 2.0,
    }
}

fn wrap_pi(a: f32) -> f32 {
    let mut x = (a + std::f32::consts::PI).rem_euclid(TAU) - std::f32::consts::PI;
    if x <= -std::f32::consts::PI {
        x += TAU;
    }
    x
}

/// One locomotion tick toward a desired heading + speed.
pub fn step(p: &LocoParams, s: GroundState, desired_heading: f32, desired_speed: f32, dt: f32) -> LocoStep {
    if dt <= 0.0 {
        return LocoStep { state: s, turn_rate: 0.0, forward: 0.0, gait: gait_kind(p, s.speed) };
    }
    // turn: rate-limited toward the desired heading (no instant snap)
    let want = wrap_pi(desired_heading - s.heading);
    let max_turn = p.turn_rate.max(0.0) * dt;
    let applied = want.clamp(-max_turn, max_turn);
    let heading = wrap_pi(s.heading + applied);
    let turn_rate = applied / dt;

    // speed: accelerate/decelerate toward the (clamped) desired speed
    let target = desired_speed.clamp(0.0, p.max_speed.max(0.0));
    let dv = (target - s.speed).clamp(-p.accel.max(0.0) * dt, p.accel.max(0.0) * dt);
    let speed = (s.speed + dv).max(0.0);

    // step cadence: legs advance one full cycle per `stride` metres travelled, so
    // a faster dragon steps faster and a stopped one holds its feet still.
    let forward = speed * dt;
    let stride = p.stride.max(0.1);
    let gait_phase = (s.gait_phase + (forward / stride) * TAU).rem_euclid(TAU);

    LocoStep {
        state: GroundState { heading, speed, gait_phase },
        turn_rate,
        forward,
        gait: gait_kind(p, speed),
    }
}

fn gait_kind(p: &LocoParams, speed: f32) -> GaitKind {
    let m = p.max_speed.max(1e-3);
    let f = speed / m;
    if speed < 0.05 {
        GaitKind::Idle
    } else if f < 0.4 {
        GaitKind::Walk
    } else if f < 0.8 {
        GaitKind::Trot
    } else {
        GaitKind::Gallop
    }
}

#[derive(Clone, Copy, Debug)]
pub struct SprintStep {
    pub loco: LocoStep,
    /// True while the dragon is actually sprinting (stamina permitting).
    pub sprinting: bool,
}

/// Locomotion tick with a sprint gate, ported onto [`crate::sim::stamina`]. When
/// `wants_sprint` and stamina allows, the dragon may exceed cruise `max_speed` by
/// `SPRINT_MULT` and burns stamina; otherwise it caps at `max_speed` and stamina
/// recovers. Gait reads `Gallop` whenever sprinting. With `wants_sprint == false`
/// this is identical to [`step`] (and stamina regenerates).
pub fn sprint_step(
    p: &LocoParams,
    s: GroundState,
    desired_heading: f32,
    desired_speed: f32,
    wants_sprint: bool,
    stamina: &mut crate::sim::stamina::Stamina,
    dt: f32,
) -> SprintStep {
    let mult = stamina.tick(wants_sprint, dt.max(0.0)); // drains/regens; 1.0 or SPRINT_MULT
    let sprinting = mult > 1.0 + 1e-3;
    let pp = LocoParams { max_speed: p.max_speed * mult, accel: p.accel * mult, ..*p };
    let want = if sprinting { pp.max_speed } else { desired_speed };
    let mut loco = step(&pp, s, desired_heading, want, dt);
    if sprinting {
        loco.gait = GaitKind::Gallop;
    }
    SprintStep { loco, sprinting }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    fn p() -> LocoParams { params_from(&Genome::from_seed(3)) }

    #[test]
    fn sprint_exceeds_cruise_then_falls_back_when_winded() {
        use crate::sim::stamina::Stamina;
        let p = p();
        let mut st = Stamina::full();
        let mut s = GroundState::at(0.0);
        // sprint flat-out until stamina runs down
        let mut peak = 0.0f32;
        for _ in 0..600 {
            let r = sprint_step(&p, s, 0.0, p.max_speed, true, &mut st, 0.05);
            s = r.loco.state;
            peak = peak.max(s.speed);
        }
        assert!(peak > p.max_speed * 1.2, "a fresh sprint should beat cruise top speed");
        assert!(st.level < crate::sim::stamina::SPRINT_FLOOR + 1e-3, "sprinting drains stamina to the floor");
        // winded: keep asking to sprint, but it can't be sustained — only short
        // bursts at the floor, and speed settles back near cruise.
        let mut sprint_ticks = 0;
        for _ in 0..200 {
            let r = sprint_step(&p, s, 0.0, p.max_speed, true, &mut st, 0.05);
            s = r.loco.state;
            if r.sprinting {
                sprint_ticks += 1;
            }
        }
        assert!(sprint_ticks * 3 < 200, "winded: sprint is only intermittent, never sustained");
        assert!(s.speed <= p.max_speed * 1.15, "winded dragon settles near cruise, not sprint speed");

        // not asking to sprint regenerates stamina and matches plain step
        let before = st.level;
        let _ = sprint_step(&p, GroundState::at(0.0), 0.0, 0.0, false, &mut st, 0.5);
        assert!(st.level > before, "resting regenerates stamina");
    }

    #[test]
    fn turning_is_rate_limited() {        let lp = p();
        let s = GroundState::at(0.0);
        // ask for a half-turn in a 16ms tick — only turn_rate*dt is allowed
        let r = step(&lp, s, PI, 0.0, 0.016);
        let turned = (r.state.heading).abs();
        assert!(turned <= lp.turn_rate * 0.016 + 1e-4, "no instant snap");
        assert!(turned > 0.0, "but it does begin turning");
    }

    #[test]
    fn heading_converges_to_desired() {
        let lp = p();
        let mut s = GroundState::at(0.0);
        for _ in 0..600 {
            s = step(&lp, s, 1.0, 0.0, 0.016).state;
        }
        assert!((s.heading - 1.0).abs() < 0.05, "heading reaches target, got {}", s.heading);
    }

    #[test]
    fn speed_accelerates_and_caps() {
        let lp = p();
        let mut s = GroundState::at(0.0);
        for _ in 0..600 {
            s = step(&lp, s, 0.0, 999.0, 0.016).state;
        }
        assert!((s.speed - lp.max_speed).abs() < 0.05, "tops out at max_speed");
        // decelerates back to 0
        for _ in 0..600 {
            s = step(&lp, s, 0.0, 0.0, 0.016).state;
        }
        assert!(s.speed < 0.05);
    }

    #[test]
    fn legs_step_when_moving_and_freeze_when_stopped() {
        let lp = p();
        let moving = step(&lp, GroundState { heading: 0.0, speed: 3.0, gait_phase: 0.0 }, 0.0, 3.0, 0.1);
        assert!(moving.state.gait_phase > 0.0, "legs advance while moving");
        let stopped = step(&lp, GroundState { heading: 0.0, speed: 0.0, gait_phase: 1.3 }, 0.0, 0.0, 0.1);
        assert_eq!(stopped.state.gait_phase, 1.3, "feet hold still when stopped");
        assert_eq!(stopped.gait, GaitKind::Idle);
    }

    #[test]
    fn longer_stride_advances_phase_less_per_distance() {
        let short = LocoParams { max_speed: 6.0, accel: 99.0, turn_rate: 2.0, stride: 1.0 };
        let long = LocoParams { stride: 4.0, ..short };
        let s = GroundState { heading: 0.0, speed: 4.0, gait_phase: 0.0 };
        let a = step(&short, s, 0.0, 4.0, 0.1).state.gait_phase;
        let b = step(&long, s, 0.0, 4.0, 0.1).state.gait_phase;
        assert!(b < a, "a longer stride means fewer steps over the same distance");
    }

    #[test]
    fn gait_kind_scales_with_speed() {
        let lp = p();
        assert_eq!(gait_kind(&lp, 0.0), GaitKind::Idle);
        assert_eq!(gait_kind(&lp, lp.max_speed * 0.2), GaitKind::Walk);
        assert_eq!(gait_kind(&lp, lp.max_speed * 0.6), GaitKind::Trot);
        assert_eq!(gait_kind(&lp, lp.max_speed), GaitKind::Gallop);
    }

    #[test]
    fn finite_and_deterministic() {
        let lp = p();
        let s = GroundState::at(0.3);
        let a = step(&lp, s, 1.0, 4.0, 0.016);
        let b = step(&lp, s, 1.0, 4.0, 0.016);
        assert_eq!(a.state, b.state);
        assert!(a.state.heading.is_finite() && a.state.speed.is_finite() && a.state.gait_phase.is_finite());
        // dt = 0 is a no-op
        assert_eq!(step(&lp, s, 1.0, 4.0, 0.0).state, s);
    }
}
