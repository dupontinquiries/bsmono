//! VALENCE — power a bot can point to.
//!
//! The Destiny Light loop, adapted and renamed for what it is here: the
//! squad are Machines, and what a mission banks in their frames is
//! valence. Progression is not a number the game hands you, it is the
//! sum of what you have actually recovered. Missions bank
//! a little, wardens bank more (a relic is a relic), the weekly nightfall
//! banks a trickle. Content advertises a RECOMMENDED light, and the squad
//! scales against the difference — generously above it, brutally below
//! it, capped both ways so neither direction becomes the whole game.
//!
//! What this buys the campaign: trilogy two stops being a tuning problem
//! and becomes a progression statement. A fresh squad walking into film 4
//! at storyline valence is fine; the same squad rushed there under-light is
//! SUPPOSED to bounce, and the number tells them why — the same honesty
//! Destiny's "recommended 280" bought with one glyph.
//!
//! One deliberate asymmetry: only the PLAYER side scales. The opposition
//! is the yardstick; a yardstick that moves teaches nothing.

use crate::campaign::CampaignLog;
use crate::content::Mission;
use crate::raidlog::RaidLog;

/// What each source banks. Fifteen missions and nineteen wardens put the
/// storyline ceiling at 30 + 57; nightfalls are the weekly trickle past it.
pub const PER_MISSION: u32 = 2;
pub const PER_WARDEN: u32 = 3;
pub const PER_NIGHTFALL: u32 = 2;

/// The one number, derived — never stored, so it cannot drift from the
/// logs that justify it.
pub fn valence(camp: &CampaignLog, raids: &RaidLog, nightfalls: u32) -> u32 {
    camp.done() as u32 * PER_MISSION + raids.shards() * PER_WARDEN + nightfalls * PER_NIGHTFALL
}

/// What a mission expects: deliberately BELOW what the road provides, so
/// a squad that simply plays in order arrives over-recommendation and
/// feels it — Destiny's storyline let you out-level it too, and the
/// slack is what makes the number a warning rather than a treadmill.
/// The films' escalation (raiders, warband, shielded coalition) eats
/// most of the surplus back; the sequence test holds the balance.
pub fn rec_valence(m: &Mission) -> u32 {
    match m.film {
        1 => 0,
        2 => 2,
        3 => 6,
        4 => 10,
        5 => 14,
        _ => 16,
    }
}

/// The scaling curve: 5% per point of difference, clamped. The floor is
/// harsh on purpose — running dry must feel like a wall, not a tax — and
/// the ceiling keeps an overcharged squad from deleting the game. The ceiling
/// sits where film six's finale needs the road's FULL surplus: the last
/// two missions are the one stretch that spends everything the storyline
/// banked, which is what a finale is for.
pub fn scale(valence: u32, rec: u32) -> f32 {
    let delta = valence as f32 - rec as f32;
    (1.0 + 0.05 * delta).clamp(0.60, 1.50)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::MISSIONS;

    #[test]
    fn valence_is_the_sum_of_what_was_actually_recovered() {
        let mut camp = CampaignLog::default();
        let mut raids = RaidLog::default();
        assert_eq!(valence(&camp, &raids, 0), 0);
        camp.record(MISSIONS[0].id);
        raids.record("gigadrill");
        assert_eq!(valence(&camp, &raids, 1), PER_MISSION + PER_WARDEN + PER_NIGHTFALL);
    }

    #[test]
    fn the_curve_is_clamped_both_ways() {
        assert_eq!(scale(10, 10), 1.0);
        assert!(scale(0, 30) >= 0.60 - 1e-6 && scale(0, 30) < 0.61, "the floor moved");
        assert!(scale(90, 0) <= 1.50 + 1e-6 && scale(90, 0) > 1.49, "the ceiling moved");
        assert!(scale(12, 10) > 1.0 && scale(8, 10) < 1.0);
    }

    #[test]
    fn storyline_valence_meets_every_recommendation() {
        // a player who clears the campaign IN ORDER arrives at each film
        // at or above its recommended valence — the road is enough, wardens
        // and nightfalls are gravy. This pins rec_valence to PER_MISSION.
        let mut camp = CampaignLog::default();
        let raids = RaidLog::default();
        for m in MISSIONS {
            assert!(
                valence(&camp, &raids, 0) >= rec_valence(m),
                "{} recommends {} but the road only provides {}",
                m.id,
                rec_valence(m),
                valence(&camp, &raids, 0)
            );
            camp.record(m.id);
        }
    }
}
