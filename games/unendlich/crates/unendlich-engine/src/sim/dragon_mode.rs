//! `sim::dragon_mode` — the keystone that decides whether a dragon should be on
//! the ground (run `dragon_locomotion`) or in the air (run `dragon_air`), and
//! builds the `AirIntent` the flight FSM consumes. It reads what the dragon is
//! doing (`dragon_activity::Activity`) and what it perceives, then commits to a
//! mode — with a dwell hold so it doesn't take off and land every tick (takeoff
//! and landing are expensive; flapping between them looks broken). Pure,
//! deterministic, panic-free.

use crate::sim::dragon_activity::Activity;
use crate::sim::dragon_air::AirIntent;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MovementMode {
    Ground,
    Air,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModeState {
    pub mode: MovementMode,
    dwell: f32,
}

impl ModeState {
    pub fn ground() -> Self {
        ModeState { mode: MovementMode::Ground, dwell: MIN_MODE_DWELL }
    }
    pub fn air() -> Self {
        ModeState { mode: MovementMode::Air, dwell: MIN_MODE_DWELL }
    }
}

/// What the dragon perceives, for the mode decision.
#[derive(Clone, Copy, Debug)]
pub struct ModeSenses {
    pub activity: Activity,
    pub threat: f32,
    /// A target/prey is above the dragon (favours climbing to engage).
    pub target_above: bool,
    /// A target is below (a stoop/dive opportunity).
    pub target_below: bool,
    pub target_dist2: f32,
    /// The threat itself is airborne (must fly to fight or flee it).
    pub airborne_threat: bool,
    /// Wing reserve (0..1) — exhausted ⇒ must come down.
    pub stamina01: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct ModeStep {
    pub state: ModeState,
    pub air: AirIntent,
}

const MIN_MODE_DWELL: f32 = 3.0; // seconds to hold a mode before a non-urgent switch
const FLEE_THREAT: f32 = 0.6;
const STAMINA_LAND: f32 = 0.1;
const CHASE_RANGE2: f32 = 2500.0; // beyond this, take wing to close the gap

/// Decide the movement mode and the flight intent for this tick. `target_altitude`
/// and `updraft` are environmental (cruise height + thermals) and pass straight
/// through into the returned `AirIntent`.
pub fn decide(prev: ModeState, s: &ModeSenses, target_altitude: f32, updraft: f32, dt: f32) -> ModeStep {
    let mut st = prev;
    st.dwell = (st.dwell + dt.max(0.0)).min(MIN_MODE_DWELL);

    let exhausted = s.stamina01 < STAMINA_LAND;
    let want_air = if exhausted {
        false // can't sustain flight — must be / get on the ground
    } else {
        match s.activity {
            // grounded pursuits keep it down
            Activity::Sleeping | Activity::Feeding | Activity::Forage | Activity::Idle => false,
            // alert: take to the air to face a flier, flee a serious ground threat,
            // or chase a target that's above or too far to reach on foot
            Activity::Alert => {
                s.airborne_threat
                    || s.threat >= FLEE_THREAT
                    || s.target_above
                    || s.target_below
                    || s.target_dist2 > CHASE_RANGE2
            }
        }
    };
    let want_mode = if want_air { MovementMode::Air } else { MovementMode::Ground };

    // Urgent transitions bypass the dwell hold.
    let urgent = exhausted
        || (want_air && (s.airborne_threat || s.threat >= FLEE_THREAT));
    if want_mode != st.mode && (urgent || st.dwell >= MIN_MODE_DWELL) {
        st.mode = want_mode;
        st.dwell = 0.0;
    }

    let in_air = st.mode == MovementMode::Air;
    let air = AirIntent {
        wants_fly: in_air && !exhausted,
        wants_land: !in_air || exhausted,
        target_altitude,
        updraft,
        dive: in_air && s.target_below && matches!(s.activity, Activity::Alert),
    };
    ModeStep { state: st, air }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn senses(activity: Activity) -> ModeSenses {
        ModeSenses {
            activity,
            threat: 0.0,
            target_above: false,
            target_below: false,
            target_dist2: 100.0,
            airborne_threat: false,
            stamina01: 1.0,
        }
    }

    #[test]
    fn grounded_pursuits_stay_grounded() {
        for a in [Activity::Feeding, Activity::Forage, Activity::Sleeping, Activity::Idle] {
            let r = decide(ModeState::ground(), &senses(a), 80.0, 0.0, 0.2);
            assert_eq!(r.state.mode, MovementMode::Ground, "{a:?} should stay grounded");
            assert!(!r.air.wants_fly && r.air.wants_land);
        }
    }

    #[test]
    fn airborne_threat_launches_immediately() {
        let mut s = senses(Activity::Alert);
        s.airborne_threat = true;
        let r = decide(ModeState::ground(), &s, 80.0, 0.0, 0.05);
        assert_eq!(r.state.mode, MovementMode::Air, "must fly to face a flier");
        assert!(r.air.wants_fly);
    }

    #[test]
    fn panics_into_the_air_from_a_serious_ground_threat() {
        let mut s = senses(Activity::Alert);
        s.threat = 0.8;
        let r = decide(ModeState::ground(), &s, 80.0, 0.0, 0.05);
        assert_eq!(r.state.mode, MovementMode::Air);
    }

    #[test]
    fn dives_at_a_target_below_while_airborne_and_alert() {
        let mut s = senses(Activity::Alert);
        s.target_below = true;
        let r = decide(ModeState::air(), &s, 80.0, 0.0, 0.1);
        assert!(r.air.dive, "a target below + airborne + alert ⇒ stoop");
    }

    #[test]
    fn exhaustion_forces_a_landing() {
        let mut s = senses(Activity::Alert);
        s.airborne_threat = true; // would normally want air
        s.stamina01 = 0.05; // but it's spent
        let r = decide(ModeState::air(), &s, 80.0, 0.0, 0.1);
        assert!(r.air.wants_land && !r.air.wants_fly, "spent wings must come down");
    }

    #[test]
    fn does_not_flap_modes_on_a_marginal_chase_distance() {
        // Alert, no urgent flags, distance flapping around the chase threshold.
        let mut st = ModeState::ground();
        let mut changes = 0;
        let mut prev = st.mode;
        for i in 0..80 {
            let mut s = senses(Activity::Alert);
            s.target_dist2 = if i % 2 == 0 { 2600.0 } else { 2400.0 }; // straddle CHASE_RANGE2
            let r = decide(st, &s, 80.0, 0.0, 0.1);
            st = r.state;
            if st.mode != prev {
                changes += 1;
                prev = st.mode;
            }
        }
        // 8s at dt=0.1 with a 3s dwell ⇒ a couple of switches, not one per tick.
        assert!(changes <= 4, "mode should be stable, got {changes} switches");
    }

    #[test]
    fn deterministic() {
        let s = senses(Activity::Alert);
        let a = decide(ModeState::ground(), &s, 80.0, 1.0, 0.1);
        let b = decide(ModeState::ground(), &s, 80.0, 1.0, 0.1);
        assert_eq!(a.state, b.state);
        assert_eq!(a.air.wants_fly, b.air.wants_fly);
    }
}
