//! sim::flight_phase — flight-phase FSM for any flying creature.
//!
//! From `flying_animal_behaviour §2.2` (bird FSM) generalised to dragons and all
//! flying fauna: a flier moves through phases — takeoff, climb, cruise (flap),
//! soar (thermal), glide, and, when hunting, **approach → stoop** (the raptor
//! dive). `dragon_mode` already picks air-vs-ground and flags a dive *only when
//! prey is already below* — which is why dragons that cruise low never set up a
//! stoop and rarely swoop. This module adds the missing piece: on spotting prey
//! it commands a **climb to a stoop height above the target**, then dives once
//! positioned above and aligned. It outputs the phase, the target altitude to
//! seek (which *rises* while setting up a stoop), and the dive flag. Pure,
//! deterministic, std-only; reusable by `dragon_brain` and fauna flight alike.

/// A flight phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlightPhase {
    /// On the ground / perched.
    Ground,
    /// Leaving the ground, below stall+margin.
    Takeoff,
    /// Gaining altitude toward the target height.
    Climb,
    /// Powered level flight at cruise altitude.
    Cruise,
    /// Passive thermal soaring (gaining height on an updraft).
    Soar,
    /// Descending glide (losing height, saving energy).
    Glide,
    /// Closing on prey, climbing to stoop height above it.
    Approach,
    /// Diving attack on prey below.
    Stoop,
    /// Controlled descent to land (low energy / no reason to stay up).
    Descend,
}

/// What the flier perceives this tick.
#[derive(Clone, Copy, Debug)]
pub struct FlightSenses {
    pub grounded: bool,
    pub wants_fly: bool,
    pub airspeed: f32,
    pub stall_speed: f32,
    /// Height above ground (m).
    pub altitude: f32,
    /// Desired cruise height when not hunting (m).
    pub cruise_altitude: f32,
    /// Flight energy/stamina reserve, `0..=1`.
    pub energy01: f32,
    /// Thermal/updraft strength at the flier (m/s), `>= 0`.
    pub updraft: f32,
    /// Currently has a prey target.
    pub hunting: bool,
    /// Prey height above ground (m).
    pub prey_altitude: f32,
    /// Horizontal distance to prey (m).
    pub prey_range: f32,
    /// The flier is above the prey (height set up for a stoop).
    pub above_prey: bool,
    /// The flier is roughly over/aligned with the prey (dive would connect).
    pub aligned: bool,
}

/// The resolved flight intent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlightIntent {
    pub phase: FlightPhase,
    /// Altitude (above ground) to seek this tick.
    pub target_altitude: f32,
    /// Commit a diving attack.
    pub dive: bool,
}

/// How far above the prey to climb before stooping.
pub const STOOP_HEIGHT: f32 = 25.0;
/// Horizontal range within which to commit to a hunt (set up a stoop).
pub const STOOP_RANGE: f32 = 60.0;
/// Energy below which the flier must come down.
pub const LAND_ENERGY: f32 = 0.10;
/// Takeoff clears once airspeed exceeds stall + this margin.
pub const TAKEOFF_MARGIN: f32 = 2.0;

/// Decide the flight phase, target altitude, and dive flag.
pub fn update(s: &FlightSenses) -> FlightIntent {
    // On the ground: stay unless launching.
    if s.grounded {
        let phase = if s.wants_fly { FlightPhase::Takeoff } else { FlightPhase::Ground };
        return FlightIntent { phase, target_altitude: s.cruise_altitude, dive: false };
    }

    // Just off the ground and still slow ⇒ takeoff climb-out.
    if s.airspeed < s.stall_speed + TAKEOFF_MARGIN {
        return FlightIntent { phase: FlightPhase::Takeoff, target_altitude: s.cruise_altitude, dive: false };
    }

    // Exhausted ⇒ get down.
    if s.energy01 < LAND_ENERGY {
        return FlightIntent { phase: FlightPhase::Descend, target_altitude: 0.0, dive: false };
    }

    // Hunting within range: set up and execute a stoop.
    if s.hunting && s.prey_range <= STOOP_RANGE {
        if s.above_prey && s.aligned {
            // positioned overhead ⇒ dive onto the prey
            return FlightIntent { phase: FlightPhase::Stoop, target_altitude: s.prey_altitude, dive: true };
        }
        // not yet overhead ⇒ climb to stoop height above the prey (fly HIGHER)
        let stoop_alt = s.prey_altitude + STOOP_HEIGHT;
        return FlightIntent { phase: FlightPhase::Approach, target_altitude: stoop_alt, dive: false };
    }

    // Not hunting: manage cruise altitude with thermals.
    if s.altitude < s.cruise_altitude - 1.0 {
        // gain height — soar if there's lift to ride, else power-climb
        if s.updraft > 0.5 {
            FlightIntent { phase: FlightPhase::Soar, target_altitude: s.cruise_altitude, dive: false }
        } else {
            FlightIntent { phase: FlightPhase::Climb, target_altitude: s.cruise_altitude, dive: false }
        }
    } else if s.altitude > s.cruise_altitude + 1.0 && s.updraft <= 0.5 {
        FlightIntent { phase: FlightPhase::Glide, target_altitude: s.cruise_altitude, dive: false }
    } else {
        FlightIntent { phase: FlightPhase::Cruise, target_altitude: s.cruise_altitude, dive: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base() -> FlightSenses {
        FlightSenses {
            grounded: false,
            wants_fly: true,
            airspeed: 20.0,
            stall_speed: 8.0,
            altitude: 40.0,
            cruise_altitude: 40.0,
            energy01: 0.8,
            updraft: 0.0,
            hunting: false,
            prey_altitude: 0.0,
            prey_range: 999.0,
            above_prey: false,
            aligned: false,
        }
    }

    #[test]
    fn grounded_stays_or_takes_off() {
        let mut s = base();
        s.grounded = true;
        s.wants_fly = false;
        assert_eq!(update(&s).phase, FlightPhase::Ground);
        s.wants_fly = true;
        assert_eq!(update(&s).phase, FlightPhase::Takeoff);
    }

    #[test]
    fn slow_after_launch_is_takeoff() {
        let mut s = base();
        s.airspeed = s.stall_speed + 1.0; // below margin
        assert_eq!(update(&s).phase, FlightPhase::Takeoff);
    }

    #[test]
    fn cruises_at_altitude_when_not_hunting() {
        assert_eq!(update(&base()).phase, FlightPhase::Cruise);
    }

    #[test]
    fn hunting_climbs_above_the_prey_first() {
        // THE FIX for "flies too low / doesn't swoop": on spotting low prey the
        // flier climbs to prey + STOOP_HEIGHT rather than staying at its level.
        let mut s = base();
        s.hunting = true;
        s.prey_altitude = 5.0;
        s.prey_range = 40.0;
        s.above_prey = false;
        let intent = update(&s);
        assert_eq!(intent.phase, FlightPhase::Approach);
        assert!((intent.target_altitude - (5.0 + STOOP_HEIGHT)).abs() < 1e-4, "climb to stoop height");
        assert!(intent.target_altitude > s.cruise_altitude - 30.0); // clearly seeking height above prey
        assert!(!intent.dive);
    }

    #[test]
    fn stoops_once_positioned_above_and_aligned() {
        let mut s = base();
        s.hunting = true;
        s.prey_altitude = 5.0;
        s.prey_range = 20.0;
        s.above_prey = true;
        s.aligned = true;
        let intent = update(&s);
        assert_eq!(intent.phase, FlightPhase::Stoop);
        assert!(intent.dive, "commits the dive");
        assert!((intent.target_altitude - 5.0).abs() < 1e-4, "dives toward the prey");
    }

    #[test]
    fn out_of_range_prey_does_not_trigger_a_stoop() {
        let mut s = base();
        s.hunting = true;
        s.prey_range = STOOP_RANGE + 10.0;
        assert_eq!(update(&s).phase, FlightPhase::Cruise, "too far ⇒ keep cruising (close first)");
    }

    #[test]
    fn low_energy_forces_descent() {
        let mut s = base();
        s.energy01 = 0.05;
        s.hunting = true;
        s.prey_range = 10.0;
        s.above_prey = true;
        s.aligned = true;
        assert_eq!(update(&s).phase, FlightPhase::Descend, "exhaustion overrides the hunt");
    }

    #[test]
    fn soars_on_a_thermal_when_climbing() {
        let mut s = base();
        s.altitude = 20.0; // below cruise
        s.updraft = 1.0; // lift available
        assert_eq!(update(&s).phase, FlightPhase::Soar);
        s.updraft = 0.0;
        assert_eq!(update(&s).phase, FlightPhase::Climb, "no lift ⇒ power-climb");
    }

    #[test]
    fn glides_down_when_too_high_without_lift() {
        let mut s = base();
        s.altitude = s.cruise_altitude + 10.0;
        assert_eq!(update(&s).phase, FlightPhase::Glide);
    }

    #[test]
    fn deterministic() {
        let s = base();
        assert_eq!(update(&s), update(&s));
    }
}
