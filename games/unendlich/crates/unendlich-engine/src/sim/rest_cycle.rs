//! rest_cycle — every creature keeps a daily rhythm of activity and rest, not
//! just dragons. Read off the sky's day phase (0 sunrise · .25 noon · .5 sunset ·
//! .75 midnight) plus a species rhythm, the body's fatigue, and the chill of its
//! surroundings. Diurnal birds fold up at night; nocturnal listeners wake then;
//! crepuscular fauna stir at dawn and dusk and lull through midday and midnight;
//! cathemeral dragons keep a low territorial watch around the clock with a midday
//! basking rise. Bees are diurnal but also cold-sensitive — the `chill` term
//! sends them home early on a cold afternoon. Pure; the brain turns a high
//! `rest_urge` into a [`crate::sim::liveliness::MotionState::Resting`] pose and
//! suppresses wandering.

use std::f32::consts::TAU;

/// A species' daily activity pattern.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rhythm {
    /// Active by day, rests at night (birds, bees, big grazers).
    Diurnal,
    /// Active at night, rests by day (listeners, ambush hunters).
    Nocturnal,
    /// Active at dawn and dusk; lulls at midday and midnight (skittish fauna).
    Crepuscular,
    /// Active around the clock at a low watch, with a midday basking rise
    /// (territorial dragons).
    Cathemeral,
}

/// Rest urge from the time of day alone, 0 (fully active) … 1 (wants to rest),
/// for `rhythm` at `day_phase` in `[0, 1)`.
pub fn daily_rest_urge(rhythm: Rhythm, day_phase: f32) -> f32 {
    let p = day_phase.rem_euclid(1.0);
    match rhythm {
        // peaks at midnight (.75), troughs at noon (.25)
        Rhythm::Diurnal => 0.5 + 0.5 * (TAU * (p - 0.75)).cos(),
        // the mirror image: peaks at noon, troughs at midnight
        Rhythm::Nocturnal => 0.5 + 0.5 * (TAU * (p - 0.25)).cos(),
        // double-rate: rest at midday and midnight, active at dawn and dusk
        Rhythm::Crepuscular => 0.5 - 0.5 * (2.0 * TAU * p).cos(),
        // low baseline watch with a gentle midday basking rise
        Rhythm::Cathemeral => 0.15 + 0.25 * (TAU * (p - 0.25)).cos().max(0.0),
    }
}

/// Combined rest urge: the daily rhythm, plus fatigue and cold pushing toward rest
/// on top of it — either can tip a creature into resting even off-schedule. All
/// inputs in `[0, 1]` (`fatigue` 1 = exhausted, `chill` 1 = freezing for this
/// species). Clamped to `[0, 1]`.
pub fn rest_urge(rhythm: Rhythm, day_phase: f32, fatigue: f32, chill: f32) -> f32 {
    let daily = daily_rest_urge(rhythm, day_phase);
    (daily + 0.6 * fatigue.clamp(0.0, 1.0) + 0.5 * chill.clamp(0.0, 1.0)).min(1.0)
}

/// Threshold helper: should the creature settle down to rest now?
pub fn wants_rest(rhythm: Rhythm, day_phase: f32, fatigue: f32, chill: f32) -> bool {
    rest_urge(rhythm, day_phase, fatigue, chill) >= 0.6
}

/// The rhythm for a named creature species.
pub fn rhythm_for(kind: crate::item::CreatureKind) -> Rhythm {
    use crate::item::CreatureKind::*;
    match kind {
        Dragon => Rhythm::Cathemeral,
        Spaeher => Rhythm::Diurnal,   // a sentinel bird
        Lauscher => Rhythm::Nocturnal, // listens through the dark
        Fauna => Rhythm::Crepuscular,
        Dinosaur => Rhythm::Diurnal,
    }
}

/// The rhythm for an ambient sim mob kind (birds fly by day, grazers are
/// crepuscular fauna, seekers hunt at night).
pub fn rhythm_for_mob(kind: crate::sim::mob::MobKind) -> Rhythm {
    use crate::sim::mob::MobKind::*;
    match kind {
        Flyer => Rhythm::Diurnal,
        Grazer => Rhythm::Crepuscular,
        Seeker => Rhythm::Nocturnal,
        Walker => Rhythm::Cathemeral,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.02
    }

    #[test]
    fn diurnal_rests_at_night_works_by_day() {
        assert!(near(daily_rest_urge(Rhythm::Diurnal, 0.25), 0.0), "active at noon");
        assert!(near(daily_rest_urge(Rhythm::Diurnal, 0.75), 1.0), "asleep at midnight");
    }

    #[test]
    fn nocturnal_is_the_mirror_of_diurnal() {
        for &p in &[0.0, 0.1, 0.25, 0.4, 0.6, 0.75, 0.9] {
            let d = daily_rest_urge(Rhythm::Diurnal, p);
            let n = daily_rest_urge(Rhythm::Nocturnal, p);
            assert!(near(d + n, 1.0), "diurnal+nocturnal=1 at {p}: {d}+{n}");
        }
    }

    #[test]
    fn crepuscular_stirs_at_dawn_and_dusk() {
        assert!(near(daily_rest_urge(Rhythm::Crepuscular, 0.0), 0.0), "active at dawn");
        assert!(near(daily_rest_urge(Rhythm::Crepuscular, 0.5), 0.0), "active at dusk");
        assert!(daily_rest_urge(Rhythm::Crepuscular, 0.25) > 0.9, "lulls at midday");
        assert!(daily_rest_urge(Rhythm::Crepuscular, 0.75) > 0.9, "lulls at midnight");
    }

    #[test]
    fn cathemeral_keeps_a_low_watch_with_a_midday_rise() {
        let midday = daily_rest_urge(Rhythm::Cathemeral, 0.25);
        let midnight = daily_rest_urge(Rhythm::Cathemeral, 0.75);
        assert!(midday > midnight, "basks more at midday: {midday} vs {midnight}");
        // never forced to rest by the clock alone
        for i in 0..100 {
            let p = i as f32 / 100.0;
            assert!(daily_rest_urge(Rhythm::Cathemeral, p) < 0.6, "stays on watch at {p}");
        }
    }

    #[test]
    fn fatigue_and_chill_push_toward_rest() {
        // a diurnal bird at noon is wide awake...
        assert!(!wants_rest(Rhythm::Diurnal, 0.25, 0.0, 0.0));
        // ...but exhaustion forces a rest even at noon...
        assert!(wants_rest(Rhythm::Diurnal, 0.25, 1.0, 0.0), "exhausted bird rests at noon");
        // ...and a cold snap sends a bee-like diurnal forager home early.
        assert!(wants_rest(Rhythm::Diurnal, 0.4, 0.2, 1.0), "cold forager rests");
        assert!(rest_urge(Rhythm::Diurnal, 0.75, 1.0, 1.0) <= 1.0, "clamped");
    }

    #[test]
    fn species_map_to_sensible_rhythms() {
        use crate::item::CreatureKind;
        assert_eq!(rhythm_for(CreatureKind::Dragon), Rhythm::Cathemeral);
        assert_eq!(rhythm_for(CreatureKind::Spaeher), Rhythm::Diurnal);
        assert_eq!(rhythm_for(CreatureKind::Lauscher), Rhythm::Nocturnal);
        assert_eq!(rhythm_for(CreatureKind::Fauna), Rhythm::Crepuscular);
    }
}
