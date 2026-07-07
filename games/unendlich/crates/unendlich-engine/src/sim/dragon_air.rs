//! `sim::dragon_air` — the airborne behaviour state machine: a dragon takes off,
//! climbs, cruises, rides thermals (soars), glides to save its wings, dives to
//! attack, and lands. It's the air mirror of the ground batch
//! (`dragon_locomotion`/`dragon_activity`) and composes the flight aerodynamics
//! (`dragon_flight`: lift/drag/stall/mode) with `stamina` (flapping drains, soaring
//! and gliding recover). Phases hold rather than flicker — a tired dragon glides
//! until its wings recover instead of stuttering between flap and glide each tick.
//! Pure, deterministic, panic-free.

use crate::sim::dragon_flight::{flight_step, params_from, stall_speed, FlightMode, FlightParams};
use crate::sim::dragon_archetype::Genome;
use crate::sim::stamina::Stamina;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlightPhase {
    Grounded,
    TakingOff,
    Climbing,
    Cruising,
    Soaring,
    Gliding,
    Diving,
    Landing,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AirState {
    pub phase: FlightPhase,
    pub airspeed: f32,      // m/s
    pub altitude: f32,      // m above ground
    pub vertical_rate: f32, // m/s (climb +, sink −)
    pub stamina: Stamina,
    pub flap01: f32,        // wing effort this tick (drives the wingbeat in the rig)
    pub mode: FlightMode,   // aero mode from dragon_flight (for tells / VFX)
}

impl AirState {
    pub fn grounded() -> Self {
        AirState {
            phase: FlightPhase::Grounded,
            airspeed: 0.0,
            altitude: 0.0,
            vertical_rate: 0.0,
            stamina: Stamina::full(),
            flap01: 0.0,
            mode: FlightMode::Level,
        }
    }
    pub fn is_airborne(&self) -> bool {
        !matches!(self.phase, FlightPhase::Grounded)
    }
}

/// What the dragon wants to do in the air this tick.
#[derive(Clone, Copy, Debug)]
pub struct AirIntent {
    pub wants_fly: bool,
    pub wants_land: bool,
    /// Cruise altitude to seek (m above ground).
    pub target_altitude: f32,
    /// Vertical air speed at the dragon from thermals / ridge lift (m/s).
    pub updraft: f32,
    /// Commit to an attack dive.
    pub dive: bool,
}

const GROUND_ALT: f32 = 0.6;
const LANDING_ALT: f32 = 12.0;
const PULLOUT_ALT: f32 = 8.0;
const ALT_TOL: f32 = 3.0;
const UPDRAFT_SOAR: f32 = 1.5; // strong enough to ride
const UPDRAFT_MIN: f32 = 0.4; // below this a thermal is spent

// per-phase wing effort
fn phase_flap(p: FlightPhase) -> f32 {
    match p {
        FlightPhase::Grounded => 0.0,
        FlightPhase::TakingOff => 1.0,
        FlightPhase::Climbing => 0.9,
        FlightPhase::Cruising => 0.45,
        FlightPhase::Soaring => 0.08,
        FlightPhase::Gliding => 0.0,
        FlightPhase::Diving => 0.15,
        FlightPhase::Landing => 0.3,
    }
}

// frame-rate-independent approach toward a target
fn approach(cur: f32, target: f32, rate: f32, dt: f32) -> f32 {
    let k = 1.0 - (-rate.max(0.0) * dt.max(0.0)).exp();
    cur + (target - cur) * k
}

/// Advance the airborne behaviour one tick.
pub fn step(prev: AirState, params: &FlightParams, intent: &AirIntent, dt: f32) -> AirState {
    let dt = dt.max(0.0);
    let mut st = prev;
    let stall = stall_speed(params);

    // ── phase transitions ────────────────────────────────────────────────────
    let next = match st.phase {
        FlightPhase::Grounded => {
            if intent.wants_fly && st.stamina.level > 0.2 {
                FlightPhase::TakingOff
            } else {
                FlightPhase::Grounded
            }
        }
        FlightPhase::TakingOff => {
            if !intent.wants_fly {
                FlightPhase::Landing
            } else if st.airspeed >= stall && st.altitude > 1.0 {
                FlightPhase::Climbing
            } else {
                FlightPhase::TakingOff
            }
        }
        FlightPhase::Climbing => {
            if st.airspeed < stall * 0.9 || st.stamina.level < 0.1 {
                FlightPhase::Gliding // real stall, or wings spent
            } else if intent.dive {
                FlightPhase::Diving
            } else if intent.wants_land {
                FlightPhase::Landing
            } else if st.altitude >= intent.target_altitude - ALT_TOL {
                FlightPhase::Cruising
            } else if st.stamina.level < 0.3 {
                FlightPhase::Cruising // tiring — level off to recover, drift up in cruise
            } else {
                FlightPhase::Climbing
            }
        }
        FlightPhase::Cruising => {
            if intent.wants_land {
                FlightPhase::Landing
            } else if intent.dive {
                FlightPhase::Diving
            } else if st.stamina.level < 0.1 || st.airspeed < stall {
                FlightPhase::Gliding
            } else if intent.updraft > UPDRAFT_SOAR {
                FlightPhase::Soaring
            } else {
                FlightPhase::Cruising
            }
        }
        FlightPhase::Soaring => {
            if intent.dive {
                FlightPhase::Diving
            } else if intent.wants_land {
                FlightPhase::Landing
            } else if intent.updraft < UPDRAFT_MIN {
                if st.stamina.level > 0.3 { FlightPhase::Cruising } else { FlightPhase::Gliding }
            } else {
                FlightPhase::Soaring
            }
        }
        FlightPhase::Gliding => {
            if intent.dive {
                FlightPhase::Diving
            } else if intent.updraft > UPDRAFT_SOAR {
                FlightPhase::Soaring // caught a thermal — stop sinking
            } else if intent.wants_land && st.altitude < LANDING_ALT {
                FlightPhase::Landing
            } else if st.altitude <= GROUND_ALT {
                FlightPhase::Landing
            } else if intent.wants_fly && st.stamina.level > 0.5 && st.altitude < intent.target_altitude - ALT_TOL {
                FlightPhase::Climbing // wings recovered — power back up
            } else {
                FlightPhase::Gliding
            }
        }
        FlightPhase::Diving => {
            if !intent.dive || st.altitude < PULLOUT_ALT {
                if st.stamina.level > 0.3 { FlightPhase::Cruising } else { FlightPhase::Gliding }
            } else {
                FlightPhase::Diving
            }
        }
        FlightPhase::Landing => {
            if st.altitude <= GROUND_ALT {
                FlightPhase::Grounded
            } else if intent.wants_fly && !intent.wants_land {
                FlightPhase::Climbing // aborted landing
            } else {
                FlightPhase::Landing
            }
        }
    };
    st.phase = next;

    // ── effort, stamina, aero ─────────────────────────────────────────────────
    st.flap01 = phase_flap(st.phase);
    let flapping_hard = st.flap01 > 0.5;
    st.stamina.tick(flapping_hard, dt); // drains when urge, regens otherwise

    // ── airspeed target by phase ──────────────────────────────────────────────
    let cruise_speed = (stall * 1.6).max(6.0);
    let target_speed = match st.phase {
        FlightPhase::Grounded => 0.0,
        FlightPhase::TakingOff => stall * 1.2,
        FlightPhase::Climbing => stall * 1.3,
        FlightPhase::Cruising => cruise_speed,
        FlightPhase::Soaring => stall * 1.4,
        FlightPhase::Gliding => stall * 1.5, // best-glide
        FlightPhase::Diving => cruise_speed * 2.2, // gravity-fed
        FlightPhase::Landing => stall * 1.05,
    };
    st.airspeed = approach(st.airspeed, target_speed, 1.4, dt).max(0.0);

    // ── vertical rate target by phase (+ thermals) ────────────────────────────
    let target_vr = match st.phase {
        FlightPhase::Grounded => 0.0,
        FlightPhase::TakingOff => 3.0 * st.stamina.level,
        FlightPhase::Climbing => 4.0 * st.stamina.level + intent.updraft,
        FlightPhase::Cruising => {
            // hold target altitude
            (intent.target_altitude - st.altitude).clamp(-2.0, 2.0) + intent.updraft * 0.5
        }
        FlightPhase::Soaring => intent.updraft,
        FlightPhase::Gliding => -1.6 + intent.updraft,
        FlightPhase::Diving => -14.0,
        FlightPhase::Landing => {
            // flare: gentler the closer to the ground
            -(st.altitude.min(6.0) * 0.5 + 0.4)
        }
    };
    st.vertical_rate = approach(st.vertical_rate, target_vr, 2.0, dt);
    st.altitude = (st.altitude + st.vertical_rate * dt).max(0.0);
    if st.phase == FlightPhase::Grounded {
        st.altitude = 0.0;
        st.vertical_rate = 0.0;
        st.airspeed = 0.0;
    }

    // aero readout from the flight model (mode/stall tell for VFX + rig)
    st.mode = flight_step(params, st.airspeed, st.flap01, dt).mode;
    st
}

/// Convenience for the bin: build params straight from a genome.
pub fn params(g: &Genome) -> FlightParams {
    params_from(g)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p() -> FlightParams { params_from(&Genome::from_seed(3)) }
    fn intent() -> AirIntent {
        AirIntent { wants_fly: true, wants_land: false, target_altitude: 80.0, updraft: 0.0, dive: false }
    }

    #[test]
    fn takes_off_when_it_wants_to_fly() {
        let s = step(AirState::grounded(), &p(), &intent(), 0.2);
        assert_eq!(s.phase, FlightPhase::TakingOff);
        assert!(s.flap01 > 0.5 && s.airspeed > 0.0);
    }

    #[test]
    fn full_climb_to_cruise_then_land() {
        let params = p();
        let mut st = AirState::grounded();
        let mut reached_cruise = false;
        let mut max_alt = 0.0f32;
        for _ in 0..1500 {
            st = step(st, &params, &intent(), 0.1);
            reached_cruise |= st.phase == FlightPhase::Cruising;
            max_alt = max_alt.max(st.altitude);
            if max_alt > 60.0 {
                break;
            }
        }
        assert!(reached_cruise, "levels off into cruise");
        assert!(max_alt > 50.0, "gains real altitude: {max_alt}");
        let land = AirIntent { wants_fly: false, wants_land: true, ..intent() };
        let mut landed = false;
        for _ in 0..3000 {
            st = step(st, &params, &land, 0.1);
            if st.phase == FlightPhase::Grounded {
                landed = true;
                break;
            }
        }
        assert!(landed, "should descend and land");
        assert!(st.altitude <= GROUND_ALT);
    }

    #[test]
    fn soars_on_a_thermal_and_recovers_stamina() {
        let params = p();
        let cruise = (stall_speed(&params) * 1.6).max(6.0);
        let mut st = AirState { phase: FlightPhase::Cruising, airspeed: cruise, altitude: 80.0, vertical_rate: 0.0, stamina: Stamina::new(0.5), flap01: 0.45, mode: FlightMode::Level };
        let thermal = AirIntent { updraft: 3.0, ..intent() };
        st = step(st, &params, &thermal, 0.2);
        assert_eq!(st.phase, FlightPhase::Soaring);
        let s0 = st.stamina.level;
        for _ in 0..30 {
            st = step(st, &params, &thermal, 0.2);
        }
        assert!(st.stamina.level > s0, "soaring recovers the wings");
        assert!(st.altitude >= 80.0, "a thermal carries it up");
    }

    #[test]
    fn exhausted_wings_glide_and_descend() {
        let params = p();
        let mut st = AirState { phase: FlightPhase::Climbing, airspeed: 14.0, altitude: 100.0, vertical_rate: 2.0, stamina: Stamina::new(0.05), flap01: 0.9, mode: FlightMode::Climbing };
        st = step(st, &params, &intent(), 0.2);
        assert_eq!(st.phase, FlightPhase::Gliding, "no stamina ⇒ glide");
        let a0 = st.altitude;
        for _ in 0..20 {
            st = step(st, &params, &AirIntent { wants_fly: false, ..intent() }, 0.2);
        }
        assert!(st.altitude < a0, "a glide trades altitude");
        assert!(st.stamina.level > 0.05, "and rests the wings");
    }

    #[test]
    fn dives_then_pulls_out_low() {
        let params = p();
        let cruise = (stall_speed(&params) * 1.6).max(6.0);
        let mut st = AirState { phase: FlightPhase::Cruising, airspeed: cruise, altitude: 120.0, vertical_rate: 0.0, stamina: Stamina::full(), flap01: 0.45, mode: FlightMode::Level };
        let dive = AirIntent { dive: true, ..intent() };
        st = step(st, &params, &dive, 0.1);
        assert_eq!(st.phase, FlightPhase::Diving);
        let mut pulled_out = false;
        for _ in 0..2000 {
            st = step(st, &params, &dive, 0.1);
            if st.phase != FlightPhase::Diving {
                pulled_out = true;
                break;
            }
        }
        assert!(pulled_out, "pulls out before hitting the ground");
        assert!(st.altitude < PULLOUT_ALT + 5.0);
    }

    #[test]
    fn bounded_and_deterministic() {
        let params = p();
        let mut st = AirState::grounded();
        for i in 0..500 {
            let it = AirIntent { updraft: (i % 7) as f32 * 0.5, dive: i % 50 == 0, ..intent() };
            st = step(st, &params, &it, 0.1);
            assert!(st.altitude >= 0.0 && st.altitude.is_finite());
            assert!(st.airspeed >= 0.0 && st.airspeed.is_finite());
            assert!((0.0..=1.0).contains(&st.stamina.level));
        }
        let a = step(AirState::grounded(), &params, &intent(), 0.1);
        let b = step(AirState::grounded(), &params, &intent(), 0.1);
        assert_eq!(a, b);
    }
}
