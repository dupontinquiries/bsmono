//! `sim::dragon_brain` — the integration layer that makes a dragon *fly and use
//! its brain*. It composes the standalone cores —
//! [`dragon_activity`] (what to do), [`dragon_mode`] (ground vs air),
//! [`dragon_air`] (flight integration), [`dragon_thirst`] (drink drive), and the
//! wing-pose mapping — into a single per-tick [`think`] call. The bin holds one
//! [`BrainState`] per dragon, feeds perception in as [`BrainSenses`], and applies
//! the [`BrainOut`] (move toward `move_heading` at `move_speed01`, set the rig's
//! `wing_pose`/`flap01`, place it at `altitude`). Pure: dt + senses in, no clock/IO.

use crate::sim::dragon_activity::{self, Activity, ActivityState, Senses};
use crate::sim::dragon_air::{self, AirState, FlightPhase};
use crate::sim::dragon_archetype::Genome;
use crate::sim::dragon_flight::FlightParams;
use crate::sim::dragon_forge::WingPose;
use crate::sim::dragon_hearing::Reaction;
use crate::sim::dragon_mode::{self, ModeSenses, ModeState, MovementMode};
use crate::sim::dragon_thirst::{self, Hydration};
use crate::sim::rest_cycle::Rhythm;

/// All the carried state for one dragon's mind + flight. Build once per dragon.
pub struct BrainState {
    pub mode: ModeState,
    pub air: AirState,
    pub activity: ActivityState,
    pub hydration: Hydration,
    pub rhythm: Rhythm,
    params: FlightParams,
}

impl BrainState {
    /// A fresh, grounded, well-watered dragon with flight envelope from its genome.
    pub fn new(g: &Genome) -> Self {
        BrainState {
            mode: ModeState::ground(),
            air: AirState::grounded(),
            activity: ActivityState::fresh(),
            hydration: Hydration::full(),
            rhythm: Rhythm::Cathemeral, // territorial dragon rhythm
            params: dragon_air::params(g),
        }
    }
}

/// Perception the bin supplies each tick (what the dragon can sense + its goal).
#[derive(Clone, Copy)]
pub struct BrainSenses {
    pub threat: f32,
    pub aggression: f32,
    pub health_frac: f32,
    /// Food is right here to eat.
    pub food_in_reach: bool,
    /// A drinkable water cell (e.g. a `river_flow` channel) is within reach.
    pub water_nearby: bool,
    /// Day fraction [0,1) (0 = midnight, 0.25 = noon).
    pub day_phase: f32,
    pub heard: Reaction,
    pub target_above: bool,
    pub target_below: bool,
    pub target_dist2: f32,
    pub airborne_threat: bool,
    /// Cruise altitude to seek when flying (m above ground).
    pub target_altitude: f32,
    /// Vertical lift from thermals/ridge (m/s).
    pub updraft: f32,
    /// The bin's steering heading toward the current goal (radians).
    pub desired_heading: f32,
    /// External exertion load 0..1 (terrain, carrying) — flying adds to this.
    pub exertion01: f32,
}

impl Default for BrainSenses {
    fn default() -> Self {
        BrainSenses {
            threat: 0.0,
            aggression: 0.0,
            health_frac: 1.0,
            food_in_reach: false,
            water_nearby: false,
            day_phase: 0.25,
            heard: Reaction::silent(),
            target_above: false,
            target_below: false,
            target_dist2: 0.0,
            airborne_threat: false,
            target_altitude: 0.0,
            updraft: 0.0,
            desired_heading: 0.0,
            exertion01: 0.0,
        }
    }
}

/// The decision the bin applies this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrainOut {
    pub mode: MovementMode,
    pub flight_phase: FlightPhase,
    pub wing_pose: WingPose,
    pub activity: Activity,
    /// Heading to move along (radians).
    pub move_heading: f32,
    /// Ground/air movement effort 0..1.
    pub move_speed01: f32,
    pub wants_sprint: bool,
    /// Break off to drink (thirst beats hunger and water is in reach).
    pub seek_water: bool,
    /// Wingbeat effort this tick — drives the wing animation amplitude.
    pub flap01: f32,
    /// Height above ground the flight model wants the body at (m).
    pub altitude: f32,
}

fn wing_pose_for(phase: FlightPhase) -> WingPose {
    match phase {
        FlightPhase::Grounded => WingPose::Folded,
        FlightPhase::TakingOff | FlightPhase::Climbing | FlightPhase::Landing => WingPose::Beating,
        FlightPhase::Cruising => WingPose::Spread,
        FlightPhase::Soaring | FlightPhase::Gliding => WingPose::Flared,
        FlightPhase::Diving => WingPose::Aggressive,
    }
}

/// Advance the dragon's mind + flight one tick and return what to do.
pub fn think(st: &mut BrainState, s: &BrainSenses, dt: f32) -> BrainOut {
    // 1. High-level activity (owns + ticks vitality, fatigue, alert).
    let senses = Senses {
        threat: s.threat,
        aggression: s.aggression,
        health_frac: s.health_frac,
        food_in_reach: s.food_in_reach,
        day_phase: s.day_phase,
        heard: s.heard,
    };
    st.activity = dragon_activity::step(st.activity, st.rhythm, &senses, dt);

    // 2. Hydration: flying is thirsty work, on top of any external load.
    let airborne = st.air.is_airborne();
    let exert = (s.exertion01 + if airborne { 0.5 } else { 0.0 }).min(1.0);
    st.hydration.tick(dt, exert);

    // 3. Ground-or-air decision (uses the activity + fatigue as a stamina proxy).
    let stamina01 = (1.0 - st.activity.fatigue).max(0.0).min(1.0);
    let mode_senses = ModeSenses {
        activity: st.activity.activity,
        threat: s.threat,
        target_above: s.target_above,
        target_below: s.target_below,
        target_dist2: s.target_dist2,
        airborne_threat: s.airborne_threat,
        stamina01,
    };
    let mode_step = dragon_mode::decide(st.mode, &mode_senses, s.target_altitude, s.updraft, dt);
    st.mode = mode_step.state;

    // 4. Integrate the flight envelope from the mode's air intent.
    st.air = dragon_air::step(st.air, &st.params, &mode_step.air, dt);

    // 5. Drives → movement intent.
    let seek_water = s.water_nearby && dragon_thirst::prefers_water_over_food(st.hydration, &st.activity.vitality);
    let (speed, sprint) = match st.activity.activity {
        Activity::Sleeping | Activity::Idle => (0.0, false),
        Activity::Forage | Activity::Feeding => (0.4, false),
        Activity::Alert => {
            if s.threat > 0.5 {
                (1.0, true) // flee hard
            } else {
                (0.7, s.target_dist2 > 0.0) // close on a target
            }
        }
    };

    BrainOut {
        mode: st.mode.mode,
        flight_phase: st.air.phase,
        wing_pose: wing_pose_for(st.air.phase),
        activity: st.activity.activity,
        move_heading: s.desired_heading,
        move_speed01: speed,
        wants_sprint: sprint,
        seek_water,
        flap01: st.air.flap01,
        altitude: st.air.altitude,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dragon() -> BrainState {
        BrainState::new(&Genome::from_seed(5))
    }

    #[test]
    fn fresh_dragon_starts_grounded_and_folded() {
        let mut st = dragon();
        let out = think(&mut st, &BrainSenses::default(), 0.1);
        assert_eq!(out.mode, MovementMode::Ground);
        assert_eq!(out.flight_phase, FlightPhase::Grounded);
        assert_eq!(out.wing_pose, WingPose::Folded);
        assert!(!st.air.is_airborne());
    }

    #[test]
    fn takes_to_the_air_against_an_airborne_threat_and_beats_its_wings() {
        let mut st = dragon();
        let s = BrainSenses {
            threat: 0.9,
            airborne_threat: true,
            target_above: true,
            target_altitude: 40.0,
            target_dist2: 100.0,
            ..Default::default()
        };
        let mut became_air = false;
        let mut beat = false;
        for _ in 0..200 {
            let out = think(&mut st, &s, 0.1);
            if out.mode == MovementMode::Air {
                became_air = true;
            }
            if matches!(out.wing_pose, WingPose::Beating | WingPose::Spread | WingPose::Flared) {
                beat = true;
            }
        }
        assert!(became_air, "an airborne threat drives it into the air");
        assert!(beat, "airborne ⇒ wings work the air (not folded)");
        assert!(st.air.is_airborne());
    }

    #[test]
    fn calm_daytime_dragon_forages_on_the_ground() {
        let mut st = dragon();
        let s = BrainSenses { day_phase: 0.25, threat: 0.0, ..Default::default() };
        let mut out = think(&mut st, &s, 0.5);
        for _ in 0..40 {
            out = think(&mut st, &s, 0.5);
        }
        assert_eq!(out.mode, MovementMode::Ground);
        // foraging/idle on the ground, wings folded
        assert_eq!(out.wing_pose, WingPose::Folded);
        assert!(out.move_speed01 >= 0.0 && out.move_speed01 <= 1.0);
    }

    #[test]
    fn parched_dragon_with_water_in_reach_breaks_off_to_drink() {
        let mut st = dragon();
        st.hydration = Hydration::new(0.1); // parched
        let s = BrainSenses { water_nearby: true, ..Default::default() };
        let out = think(&mut st, &s, 0.1);
        assert!(out.seek_water, "parched + water nearby ⇒ go drink");
        // and with no water it cannot
        let dry = BrainSenses { water_nearby: false, ..Default::default() };
        let out2 = think(&mut st, &dry, 0.1);
        assert!(!out2.seek_water);
    }

    #[test]
    fn flying_dehydrates_faster_than_resting() {
        let mut flyer = dragon();
        let mut rester = dragon();
        let fly = BrainSenses { threat: 0.9, airborne_threat: true, target_above: true, target_altitude: 40.0, ..Default::default() };
        let calm = BrainSenses::default();
        for _ in 0..120 {
            think(&mut flyer, &fly, 0.2);
            think(&mut rester, &calm, 0.2);
        }
        assert!(flyer.hydration.level < rester.hydration.level, "flight is thirsty work");
    }

    #[test]
    fn deterministic() {
        let s = BrainSenses { threat: 0.6, target_above: true, target_dist2: 50.0, ..Default::default() };
        let mut a = dragon();
        let mut b = dragon();
        for _ in 0..80 {
            let oa = think(&mut a, &s, 0.1);
            let ob = think(&mut b, &s, 0.1);
            assert_eq!(oa, ob);
        }
    }
}

// ── r336-dragon-size-bonus: size-aware behavior ──────────────────────────────
// Ties the restored small→huge size range (sim::dragon_archetype::size_scale) to
// behavior: small dragons are agile but timid, huge dragons turn slowly, hit harder,
// and carry more stamina. Pure modifiers the brain/flight tick multiplies in.

/// Behavior modifiers derived from a dragon's `size_class`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SizeBehavior {
    /// Turn-rate / agility multiplier (small = nimble > 1, huge < 1).
    pub turn_rate_scale: f32,
    /// Added to perceived aggression (huge dragons are bolder).
    pub aggression_bias: f32,
    /// Stamina-pool / endurance multiplier (huge dragons last longer).
    pub stamina_scale: f32,
}

/// Map a `size_class` (0..=`dragon_archetype::SIZE_CLASS_MAX`) to behavior modifiers.
/// Monotone in size; finite; `size_class` is clamped to the valid range.
pub fn size_behavior(size_class: u8) -> SizeBehavior {
    let maxc = crate::sim::dragon_archetype::SIZE_CLASS_MAX as f32;
    let t = if maxc > 0.0 {
        (size_class.min(crate::sim::dragon_archetype::SIZE_CLASS_MAX) as f32) / maxc
    } else {
        0.0
    };
    // t=0 (smallest) → agile/timid/lean; t=1 (hugest) → ponderous/bold/enduring.
    SizeBehavior {
        turn_rate_scale: 1.35 - 0.70 * t, // 1.35 → 0.65
        aggression_bias: 0.30 * t,        // 0.0 → 0.30
        stamina_scale: 0.80 + 0.80 * t,   // 0.80 → 1.60
    }
}

#[cfg(test)]
mod r336_size_behavior_tests {
    use super::size_behavior;
    use crate::sim::dragon_archetype::SIZE_CLASS_MAX;

    #[test]
    fn small_is_agile_huge_is_ponderous_and_bold() {
        let small = size_behavior(0);
        let huge = size_behavior(SIZE_CLASS_MAX);
        assert!(small.turn_rate_scale > huge.turn_rate_scale, "small turns faster");
        assert!(huge.aggression_bias > small.aggression_bias, "huge is bolder");
        assert!(huge.stamina_scale > small.stamina_scale, "huge has more stamina");
        // all finite + positive
        for b in [small, huge] {
            assert!(b.turn_rate_scale.is_finite() && b.turn_rate_scale > 0.0);
            assert!(b.stamina_scale.is_finite() && b.stamina_scale > 0.0);
            assert!(b.aggression_bias.is_finite() && b.aggression_bias >= 0.0);
        }
    }

    #[test]
    fn monotone_and_clamped() {
        let mut prev = size_behavior(0).turn_rate_scale;
        for c in 1..=SIZE_CLASS_MAX {
            let cur = size_behavior(c).turn_rate_scale;
            assert!(cur <= prev + 1e-6, "turn_rate decreases monotonically with size");
            prev = cur;
        }
        // out-of-range clamps to the largest bucket (no panic, no overshoot)
        assert_eq!(size_behavior(255), size_behavior(SIZE_CLASS_MAX));
    }
}
