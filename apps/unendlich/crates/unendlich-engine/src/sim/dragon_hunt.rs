//! `sim::dragon_hunt` — the predation loop that ties the dragon's systems into
//! actual hunting: it watches prey through the eyes, decides to hunt only when
//! hungry, stalks, pursues a fleer, climbs to stoop on prey below, strikes in
//! range, then feeds and disengages. It doesn't move or attack itself — it emits
//! the intents the other cores already consume (`wants_close` → locomotion/air,
//! `wants_stoop` → `dragon_mode`/`dragon_air` dive, `wants_strike` + `aim_bearing`
//! → breath/`dragon_bending_ai`). Phases hold via a dwell timer so it doesn't
//! flicker. Pure, deterministic, panic-free.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HuntPhase {
    Search,
    Stalk,
    Pursue,
    Stoop,
    Strike,
    Feed,
    Disengage,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HuntState {
    pub phase: HuntPhase,
    dwell: f32,
    lost: f32, // seconds prey has been undetected
}

impl HuntState {
    pub fn idle() -> Self {
        HuntState { phase: HuntPhase::Search, dwell: MIN_DWELL, lost: 0.0 }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Prey {
    pub detected: bool,
    pub bearing: f32, // relative to body heading (rad)
    pub dist2: f32,
    pub rel_height: f32, // + above, − below the dragon
    pub fleeing: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct HuntSenses {
    pub prey: Prey,
    pub hunger01: f32,  // 1 − vitality.energy
    pub stamina01: f32,
    pub airborne: bool,
    pub in_strike_range: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct HuntStep {
    pub state: HuntState,
    pub wants_close: bool, // drive toward the prey (ground/air)
    pub wants_stoop: bool, // dive from above
    pub wants_strike: bool, // fire breath / bite this tick
    pub aim_bearing: f32,
}

const MIN_DWELL: f32 = 1.0;
const HUNT_HUNGER: f32 = 0.4; // won't bother hunting unless this hungry
const STOOP_HEIGHT: f32 = 6.0; // prey this far below ⇒ a stoop is worth it
const LOST_GIVEUP: f32 = 3.0; // give up after this long with no sighting

/// Advance the hunt one tick.
pub fn step(prev: HuntState, s: &HuntSenses, dt: f32) -> HuntStep {
    let dt = dt.max(0.0);
    let mut st = prev;
    st.dwell = (st.dwell + dt).min(MIN_DWELL);
    st.lost = if s.prey.detected { 0.0 } else { st.lost + dt };

    let hungry = s.hunger01 >= HUNT_HUNGER;
    let prey = &s.prey;
    let can_stoop = s.airborne && prey.rel_height < -STOOP_HEIGHT && s.stamina01 > 0.2;

    let candidate = match st.phase {
        HuntPhase::Search => {
            if hungry && prey.detected {
                HuntPhase::Stalk
            } else {
                HuntPhase::Search
            }
        }
        HuntPhase::Stalk => {
            if !prey.detected && st.lost > LOST_GIVEUP {
                HuntPhase::Disengage
            } else if s.in_strike_range {
                HuntPhase::Strike
            } else if can_stoop {
                HuntPhase::Stoop
            } else if prey.fleeing {
                HuntPhase::Pursue
            } else {
                HuntPhase::Stalk
            }
        }
        HuntPhase::Pursue => {
            if !prey.detected && st.lost > LOST_GIVEUP {
                HuntPhase::Disengage
            } else if s.in_strike_range {
                HuntPhase::Strike
            } else if can_stoop {
                HuntPhase::Stoop
            } else {
                HuntPhase::Pursue
            }
        }
        HuntPhase::Stoop => {
            if s.in_strike_range {
                HuntPhase::Strike
            } else if !prey.detected && st.lost > LOST_GIVEUP {
                HuntPhase::Disengage
            } else if !s.airborne || s.stamina01 < 0.1 {
                HuntPhase::Pursue // pulled out / on the ground now
            } else {
                HuntPhase::Stoop
            }
        }
        HuntPhase::Strike => {
            if s.in_strike_range {
                // landed the hit ⇒ feed; else fall back to chasing
                if !prey.detected || !prey.fleeing {
                    HuntPhase::Feed
                } else {
                    HuntPhase::Pursue
                }
            } else {
                HuntPhase::Pursue
            }
        }
        HuntPhase::Feed => {
            if s.hunger01 < 0.15 {
                HuntPhase::Disengage // sated
            } else if !s.in_strike_range && prey.detected && prey.fleeing {
                HuntPhase::Pursue // the meal got up and ran
            } else {
                HuntPhase::Feed
            }
        }
        HuntPhase::Disengage => HuntPhase::Search,
    };

    // urgent transitions (strike windows, giving up) ignore the dwell hold
    let urgent = matches!(candidate, HuntPhase::Strike | HuntPhase::Disengage)
        || st.phase == HuntPhase::Strike;
    if candidate != st.phase && (urgent || st.dwell >= MIN_DWELL) {
        st.phase = candidate;
        st.dwell = 0.0;
    }

    let wants_close = matches!(st.phase, HuntPhase::Stalk | HuntPhase::Pursue);
    let wants_stoop = st.phase == HuntPhase::Stoop;
    let wants_strike = st.phase == HuntPhase::Strike && s.in_strike_range;
    HuntStep {
        state: st,
        wants_close,
        wants_stoop,
        wants_strike,
        aim_bearing: prey.bearing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prey(detected: bool, dist2: f32, rel_h: f32, fleeing: bool) -> Prey {
        Prey { detected, bearing: 0.3, dist2, rel_height: rel_h, fleeing }
    }
    fn senses(p: Prey, hunger: f32) -> HuntSenses {
        HuntSenses { prey: p, hunger01: hunger, stamina01: 1.0, airborne: false, in_strike_range: false }
    }

    #[test]
    fn full_hunger_locks_onto_prey_and_stalks() {
        let s = senses(prey(true, 400.0, 0.0, false), 0.8);
        let r = step(HuntState::idle(), &s, 0.2);
        assert_eq!(r.state.phase, HuntPhase::Stalk);
        assert!(r.wants_close);
    }

    #[test]
    fn not_hungry_does_not_hunt() {
        let s = senses(prey(true, 400.0, 0.0, false), 0.1);
        let r = step(HuntState::idle(), &s, 0.2);
        assert_eq!(r.state.phase, HuntPhase::Search);
        assert!(!r.wants_close && !r.wants_strike);
    }

    #[test]
    fn pursues_a_fleeing_meal() {
        let mut st = HuntState { phase: HuntPhase::Stalk, dwell: MIN_DWELL, lost: 0.0 };
        let s = senses(prey(true, 400.0, 0.0, true), 0.8);
        st = step(st, &s, 0.2).state;
        assert_eq!(st.phase, HuntPhase::Pursue);
    }

    #[test]
    fn stoops_on_prey_below_when_airborne() {
        let mut s = senses(prey(true, 900.0, -20.0, false), 0.8);
        s.airborne = true;
        let r = step(HuntState { phase: HuntPhase::Stalk, dwell: MIN_DWELL, lost: 0.0 }, &s, 0.2);
        assert_eq!(r.state.phase, HuntPhase::Stoop);
        assert!(r.wants_stoop);
    }

    #[test]
    fn strikes_in_range_then_feeds() {
        let mut s = senses(prey(true, 4.0, 0.0, false), 0.8);
        s.in_strike_range = true;
        let r = step(HuntState { phase: HuntPhase::Pursue, dwell: MIN_DWELL, lost: 0.0 }, &s, 0.1);
        assert_eq!(r.state.phase, HuntPhase::Strike);
        assert!(r.wants_strike);
        // next tick (prey now subdued) ⇒ feed
        let mut s2 = s;
        s2.prey.detected = false;
        let r2 = step(r.state, &s2, 0.1);
        assert_eq!(r2.state.phase, HuntPhase::Feed);
    }

    #[test]
    fn gives_up_when_prey_is_lost() {
        let mut st = HuntState { phase: HuntPhase::Pursue, dwell: MIN_DWELL, lost: 0.0 };
        let s = senses(prey(false, 0.0, 0.0, false), 0.8);
        for _ in 0..40 {
            st = step(st, &s, 0.2).state;
        }
        assert!(matches!(st.phase, HuntPhase::Disengage | HuntPhase::Search), "lost prey ⇒ give up");
    }

    #[test]
    fn feeding_until_sated_then_disengages() {
        let mut s = senses(prey(true, 4.0, 0.0, false), 0.1); // already nearly full
        s.in_strike_range = true;
        let r = step(HuntState { phase: HuntPhase::Feed, dwell: MIN_DWELL, lost: 0.0 }, &s, 0.2);
        assert_eq!(r.state.phase, HuntPhase::Disengage);
    }

    #[test]
    fn deterministic() {
        let s = senses(prey(true, 400.0, 0.0, false), 0.8);
        let a = step(HuntState::idle(), &s, 0.2);
        let b = step(HuntState::idle(), &s, 0.2);
        assert_eq!(a.state, b.state);
    }
}
