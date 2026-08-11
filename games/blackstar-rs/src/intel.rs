//! THE ARRAY LOGS — three quiet hints, found together.
//!
//! Source: `TF_DOMINION_SHIELD_HINTS.md`, the screenplay's design note
//! for how the shield boundary should surface across Film 2 — never as
//! a plot point, as weather: "The Array sees it. No one else can. No
//! one else does." Three sightings, each fainter and stranger than the
//! last, that only Blackstar ever reads, right up until his own
//! post-fight beat where he "pulls up all three Array readings side by
//! side for the first time."
//!
//! That framing — a private log, aggregated and read together, not
//! narrated scene by scene — is what makes this a natural find rather
//! than a forced one: the player never has Blackstar's POV during
//! Film 1 or 2, so these were never player-visible in-fiction. They
//! become visible exactly once, at the one moment the fiction already
//! puts the player inside his own systems — boarding his flagship
//! (`content.rs`'s `the_ashes_court`, "board the king's own ship; take
//! the court from the throne room"). The three readings are sitting
//! there together, same as the doc describes Blackstar finding them,
//! because the player is standing where he stood.
//!
//! The prose below is original, written in this game's own terse
//! house style (see `content.rs`'s own doc comment: "ORIGINAL terse
//! prose — the story's beats and names, no script text") — paraphrased
//! from the treatment's beats, not quoted from it.

use crate::campaign::CampaignLog;

pub struct Fragment {
    pub act: u8,
    pub timestamp: &'static str,
    pub text: &'static str,
}

pub const FRAGMENTS: [Fragment; 3] = [
    Fragment {
        act: 1,
        timestamp: "outer rim, probe response",
        text: "geometry, not a signal. a faint lattice at the edge of the heliosphere, filed \
               and forgotten between tactical calls.",
    },
    Fragment {
        act: 2,
        timestamp: "earth, urban containment",
        text: "the corrupted stopped hunting for four seconds and oriented outward -- toward \
               the same edge. something even a broken echo of unicron recognises as real.",
    },
    Fragment {
        act: 3,
        timestamp: "the lhc, mid-fight",
        text: "signal strength up a third time. clarity high enough the array stops calling it \
               noise and starts calling it structure. a sentence started and never finished: \
               'every civilization that reached this far --'",
    },
];

/// The mission whose clearing puts the player inside Blackstar's own
/// systems — see `content.rs`'s `the_ashes_court`.
pub const REVEAL_AFTER: &str = "the_ashes_court";

pub fn available(campaign: &CampaignLog) -> bool {
    campaign.is_cleared(REVEAL_AFTER)
}

/// The three readings, together, exactly as Blackstar himself first
/// laid them out — or nothing, if the ship has not been boarded yet.
pub fn read(campaign: &CampaignLog) -> Option<&'static [Fragment; 3]> {
    available(campaign).then_some(&FRAGMENTS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::MISSIONS;

    #[test]
    fn the_logs_stay_dark_until_the_flagship_falls() {
        let empty = CampaignLog::default();
        assert!(!available(&empty));
        assert!(read(&empty).is_none());

        let mut cleared = CampaignLog::default();
        cleared.record(REVEAL_AFTER);
        assert!(available(&cleared));
        let frags = read(&cleared).unwrap();
        assert_eq!(frags.len(), 3);
        // in order, and none of them blank
        for (i, f) in frags.iter().enumerate() {
            assert_eq!(f.act as usize, i + 1);
            assert!(!f.text.is_empty());
        }
    }

    #[test]
    fn the_reveal_mission_is_a_real_late_mission_and_not_the_last_one() {
        // the logs are meant to land BEFORE the campaign's own finale,
        // same as the doc: Blackstar reads them and still has a choice
        // to make about what comes next
        let m = MISSIONS.iter().find(|m| m.id == REVEAL_AFTER).expect("the reveal mission must exist");
        assert!(MISSIONS.iter().any(|o| o.order > m.order), "the flagship boarding is already the last mission");
    }

    #[test]
    fn clearing_something_else_does_not_leak_the_logs() {
        let mut cleared = CampaignLog::default();
        for id in ["the_pluto_line", "the_scavenger_king", "the_red_extraction", "the_beach_opens"] {
            cleared.record(id);
        }
        assert!(!available(&cleared), "an unrelated mission unlocked the flagship's own data");
    }
}
