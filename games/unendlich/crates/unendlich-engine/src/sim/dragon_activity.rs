//! `sim::dragon_activity` — the grounded behaviour loop: a dragon idles, forages,
//! eats, sleeps, wakes, and goes alert, driven by its energy/fatigue, the time of
//! day, threats, and the noises it hears. Composes `behavior_arbiter` (the drive
//! arbiter), `rest_cycle` (when to sleep for its rhythm), `feeding::Vitality`
//! (energy), and `dragon_hearing::Reaction` (wake / orient). Pure, deterministic.

use crate::sim::behavior_arbiter::{select, Behavior, Drives};
use crate::sim::dragon_hearing::Reaction;
use crate::sim::feeding::Vitality;
use crate::sim::rest_cycle::{rest_urge, Rhythm};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Activity { Idle, Forage, Feeding, Sleeping, Alert }

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ActivityState {
    pub activity: Activity,
    pub asleep: bool,
    pub vitality: Vitality,
    pub fatigue: f32, // 0 fresh .. 1 exhausted
    pub alert: f32,   // 0 calm .. 1 alarmed
    // Anti-jitter memory (not for external use): the last arbiter pick (so the
    // current behaviour keeps a hysteresis bonus) and how long we've held the
    // current activity (temporal fixation — a minimum dwell before switching).
    last_behavior: Option<Behavior>,
    dwell: f32,
}

impl ActivityState {
    pub fn fresh() -> Self {
        ActivityState {
            activity: Activity::Idle,
            asleep: false,
            vitality: Vitality::full(),
            fatigue: 0.0,
            alert: 0.0,
            last_behavior: None,
            dwell: MIN_DWELL, // a fresh creature may commit to its first activity at once
        }
    }
    /// Hunger 0..1 (the inverse of energy) for the arbiter.
    pub fn hunger(&self) -> f32 {
        1.0 - self.vitality.energy
    }
}

/// What the dragon currently perceives this tick.
#[derive(Clone, Copy, Debug)]
pub struct Senses {
    pub threat: f32,
    pub aggression: f32,
    pub health_frac: f32,
    /// Food is right here to eat.
    pub food_in_reach: bool,
    /// Day fraction in [0,1) (0 = midnight, 0.25 = noon … matches `rest_cycle`).
    pub day_phase: f32,
    pub heard: Reaction,
}

const FATIGUE_GAIN: f32 = 0.02; // per second awake
const FATIGUE_RECOVER: f32 = 0.10; // per second asleep
const ENERGY_DECAY_AWAKE: f32 = 0.010;
const ENERGY_DECAY_SLEEP: f32 = 0.004;
const FEED_RATE: f32 = 0.25; // energy/sec while eating
const ALERT_DECAY: f32 = 0.5; // per second
/// Bonus the arbiter gives the *current* behaviour so near-ties don't flip-flop.
const BEHAVIOR_HYSTERESIS: f32 = 0.15;
/// Minimum seconds in an activity before a non-urgent switch (temporal fixation).
const MIN_DWELL: f32 = 1.5;

/// Advance the behaviour one tick.
pub fn step(prev: ActivityState, rhythm: Rhythm, s: &Senses, dt: f32) -> ActivityState {
    let dt = dt.max(0.0);
    let mut st = prev;

    // Alertness rises to meet a threat or a startle, and decays otherwise.
    let stim = s.threat.max(s.heard.startle01).clamp(0.0, 1.0);
    st.alert = ((st.alert - ALERT_DECAY * dt).max(0.0)).max(stim).clamp(0.0, 1.0);

    // Energy + fatigue: rest recovers fatigue and slows the burn.
    if st.asleep {
        st.fatigue = (st.fatigue - FATIGUE_RECOVER * dt).max(0.0);
        st.vitality.tick(dt, ENERGY_DECAY_SLEEP);
    } else {
        st.fatigue = (st.fatigue + FATIGUE_GAIN * dt).min(1.0);
        st.vitality.tick(dt, ENERGY_DECAY_AWAKE);
    }

    let danger = s.threat > 0.4 || st.alert > 0.6;

    // Sleep / wake transitions.
    if st.asleep {
        // wake on a bang, on danger, or when it's active-time and it's rested
        let active_time = rest_urge(rhythm, s.day_phase, st.fatigue, 0.0) < 0.4;
        if s.heard.should_wake || danger || active_time {
            st.asleep = false;
        }
    } else {
        let safe = s.threat < 0.2 && st.alert < 0.3 && !s.heard.heard;
        let sleepy = rest_urge(rhythm, s.day_phase, st.fatigue, 0.0) >= 0.6;
        if sleepy && safe && !st.vitality.is_starving() {
            st.asleep = true;
        }
    }

    // Choose the activity. The arbiter gives the current behaviour a hysteresis
    // bonus, and a minimum-dwell timer holds the chosen activity for a beat so it
    // can't flicker between near-tied states tick to tick.
    st.dwell = (st.dwell + dt).min(MIN_DWELL);
    let candidate = if st.asleep {
        Activity::Sleeping
    } else {
        let drives = Drives {
            threat: s.threat,
            hunger: st.hunger(),
            fatigue: st.fatigue,
            aggression: s.aggression,
            health_frac: s.health_frac,
        };
        let choice = select(&drives, st.last_behavior, BEHAVIOR_HYSTERESIS);
        st.last_behavior = Some(choice.behavior);
        match choice.behavior {
            Behavior::Flee | Behavior::Fight => Activity::Alert,
            Behavior::Forage => {
                if s.food_in_reach { Activity::Feeding } else { Activity::Forage }
            }
            Behavior::Rest => Activity::Idle, // drowsy but kept awake (sleep handled above)
            Behavior::Patrol => {
                if st.alert > 0.4 { Activity::Alert } else { Activity::Idle }
            }
        }
    };

    // Urgent transitions bypass the dwell hold: danger, and any sleep boundary.
    let urgent = danger
        || candidate == Activity::Alert
        || candidate == Activity::Sleeping
        || st.activity == Activity::Sleeping;
    if candidate != st.activity && (urgent || st.dwell >= MIN_DWELL) {
        st.activity = candidate;
        st.dwell = 0.0;
    }

    // Eating restores energy.
    if st.activity == Activity::Feeding {
        st.vitality.feed(FEED_RATE * dt);
    }

    st
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::dragon_hearing::Reaction;

    fn calm_senses() -> Senses {
        Senses {
            threat: 0.0,
            aggression: 0.3,
            health_frac: 1.0,
            food_in_reach: false,
            day_phase: 0.25, // noon
            heard: Reaction::silent(),
        }
    }

    #[test]
    fn hungry_with_food_eats_and_refuels() {
        let mut st = ActivityState::fresh();
        st.vitality = Vitality::new(0.2); // hungry
        let mut s = calm_senses();
        s.food_in_reach = true;
        let before = st.vitality.energy;
        st = step(st, Rhythm::Diurnal, &s, 0.5);
        assert_eq!(st.activity, Activity::Feeding);
        assert!(st.vitality.energy > before, "eating restores energy");
    }

    #[test]
    fn hungry_without_food_forages() {
        let mut st = ActivityState::fresh();
        st.vitality = Vitality::new(0.2);
        let s = calm_senses(); // no food in reach
        st = step(st, Rhythm::Diurnal, &s, 0.5);
        assert_eq!(st.activity, Activity::Forage);
    }

    #[test]
    fn tired_and_safe_at_night_falls_asleep_and_recovers() {
        let mut st = ActivityState::fresh();
        st.fatigue = 0.7;
        let mut s = calm_senses();
        s.day_phase = 0.75; // midnight: a diurnal dragon wants to sleep
        st = step(st, Rhythm::Diurnal, &s, 0.5);
        assert!(st.asleep && st.activity == Activity::Sleeping, "settles to sleep when safe + sleepy");
        let f0 = st.fatigue;
        for _ in 0..20 {
            st = step(st, Rhythm::Diurnal, &s, 0.5);
        }
        assert!(st.fatigue < f0, "fatigue recovers while asleep");
        assert!(st.asleep, "stays asleep through the night when undisturbed");
    }

    #[test]
    fn a_bang_wakes_the_sleeper() {
        let mut st = ActivityState::fresh();
        st.asleep = true;
        st.activity = Activity::Sleeping;
        let mut s = calm_senses();
        s.day_phase = 0.75;
        s.heard = Reaction { heard: true, loudness: 100.0, bearing: 0.5, startle01: 0.9, should_wake: true };
        st = step(st, Rhythm::Diurnal, &s, 0.1);
        assert!(!st.asleep, "a loud noise wakes it");
        assert!(st.alert > 0.5, "and puts it on alert");
        assert_ne!(st.activity, Activity::Sleeping);
    }

    #[test]
    fn wakes_in_the_morning_when_rested() {
        let mut st = ActivityState::fresh();
        st.asleep = true;
        st.fatigue = 0.05; // rested
        let mut s = calm_senses();
        s.day_phase = 0.25; // noon: active time for a diurnal dragon
        st = step(st, Rhythm::Diurnal, &s, 0.1);
        assert!(!st.asleep, "wakes when it's active-time and rested");
    }

    #[test]
    fn threat_goes_alert_not_sleep() {
        let mut st = ActivityState::fresh();
        st.fatigue = 0.9;
        let mut s = calm_senses();
        s.threat = 0.8;
        s.day_phase = 0.75;
        st = step(st, Rhythm::Diurnal, &s, 0.2);
        assert!(!st.asleep, "won't sleep under threat");
        assert_eq!(st.activity, Activity::Alert);
    }

    #[test]
    fn calm_rested_fed_just_idles() {
        let st = step(ActivityState::fresh(), Rhythm::Diurnal, &calm_senses(), 0.2);
        assert_eq!(st.activity, Activity::Idle);
    }

    #[test]
    fn does_not_jitter_between_states_on_flapping_input() {
        // hungry dragon, but food flickers in/out of reach every tick — naive
        // selection would flip Feeding/Forage each tick; the dwell hold prevents it.
        let mut st = ActivityState::fresh();
        st.vitality = Vitality::new(0.2);
        let mut changes = 0;
        let mut prev = st.activity;
        for i in 0..60 {
            let mut s = calm_senses();
            s.food_in_reach = i % 2 == 0; // flap
            st = step(st, Rhythm::Diurnal, &s, 0.1);
            if st.activity != prev {
                changes += 1;
                prev = st.activity;
            }
        }
        // 60 ticks at dt=0.1 with a 1.5s dwell ⇒ at most a handful of switches,
        // not one per tick.
        assert!(changes <= 5, "activity should be temporally stable, got {changes} switches");
    }

    #[test]
    fn meters_stay_bounded_and_deterministic() {
        let mut st = ActivityState::fresh();
        let s = calm_senses();
        for _ in 0..500 {
            st = step(st, Rhythm::Cathemeral, &s, 0.2);
            assert!((0.0..=1.0).contains(&st.fatigue));
            assert!((0.0..=1.0).contains(&st.alert));
            assert!((0.0..=1.0).contains(&st.vitality.energy));
        }
        let a = step(ActivityState::fresh(), Rhythm::Diurnal, &s, 0.2);
        let b = step(ActivityState::fresh(), Rhythm::Diurnal, &s, 0.2);
        assert_eq!(a, b);
    }
}
