//! RAID LOG — which wardens have fallen, and what that has set in motion.
//!
//! The nineteen raids become one long decision only if the game REMEMBERS.
//! This is that memory: the cleared set, the shard count it implies, the
//! seal state those shards drive, and the two things a player actually
//! asks of it — "what should I fight next" and "what have I learned".
//!
//! Persistence rides the save file's `key value` format as a single
//! `wardens` line of comma-separated boss ids. Ids, not indices: the
//! roster can reorder or grow without silently corrupting old saves, and
//! an id the roster no longer knows is dropped on load rather than kept
//! as a ghost. Lenient in, canonical out — the same contract as the rest
//! of `save.rs`.

use crate::raidcon::{boss, seal_state, Boss, SealState, BOSSES};

/// The record of the campaign against the lock.
#[derive(Clone, Default)]
pub struct RaidLog {
    /// cleared wardens, roster order, no duplicates
    cleared: Vec<&'static str>,
}

impl RaidLog {
    /// Parse the save line. Unknown ids are dropped, duplicates collapse,
    /// order normalizes to the roster's — so whatever a hand-edited or
    /// stale file says, what loads is something the game can reason about.
    pub fn from_line(line: &str) -> RaidLog {
        let mut log = RaidLog::default();
        for id in line.split(',').map(str::trim).filter(|s| !s.is_empty()) {
            if let Some(b) = boss(id) {
                log.record(b.id);
            }
        }
        log
    }

    /// The save line: canonical, roster-ordered.
    pub fn line(&self) -> String {
        self.cleared.join(",")
    }

    /// Bank a clear. Returns whether it was NEW — the caller's cue to
    /// announce a shard rather than shrug at a repeat.
    pub fn record(&mut self, id: &str) -> bool {
        let Some(b) = boss(id) else { return false };
        if self.cleared.contains(&b.id) {
            return false;
        }
        self.cleared.push(b.id);
        // roster order, so the line and the codex read in one order
        self.cleared.sort_by_key(|c| BOSSES.iter().position(|x| x.id == *c));
        true
    }

    pub fn is_cleared(&self, id: &str) -> bool {
        self.cleared.contains(&id)
    }

    /// Shards recovered. One per warden; the seal counts these.
    pub fn shards(&self) -> u32 {
        self.cleared.len() as u32
    }

    pub fn seal(&self) -> SealState {
        seal_state(self.shards())
    }

    /// The wardens whose relics — and shard lines — the player has earned.
    pub fn recovered(&self) -> impl Iterator<Item = &'static Boss> + '_ {
        BOSSES.iter().filter(|b| self.is_cleared(b.id))
    }

    /// What to fight next: the LADDER — the lowest-tier warden still
    /// standing, roster order breaking ties. A recommendation, not a gate;
    /// the roster stays open because that is what makes the sweep a
    /// decision rather than a sequence.
    pub fn next_up(&self) -> Option<&'static Boss> {
        BOSSES
            .iter()
            .filter(|b| !self.is_cleared(b.id))
            .min_by_key(|b| (b.tier, BOSSES.iter().position(|x| x.id == b.id)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_roundtrips_and_stays_canonical() {
        let mut log = RaidLog::default();
        assert!(log.record("gigadrill"));
        assert!(log.record("speakerman"));
        assert!(!log.record("gigadrill"), "a repeat clear banked a second shard");
        // canonical order is roster order regardless of clear order
        assert_eq!(log.line(), "speakerman,gigadrill");
        let back = RaidLog::from_line(&log.line());
        assert_eq!(back.line(), log.line());
        assert_eq!(back.shards(), 2);
    }

    #[test]
    fn stale_and_mangled_ids_drop_on_load() {
        // a roster edit or a hand-edited file must not corrupt the log
        let log = RaidLog::from_line(" gigadrill , not_a_warden,, gigadrill ,observer");
        assert_eq!(log.shards(), 2);
        assert!(log.is_cleared("gigadrill") && log.is_cleared("observer"));
        assert_eq!(RaidLog::from_line("").shards(), 0);
    }

    #[test]
    fn the_seal_follows_the_log() {
        let mut log = RaidLog::default();
        assert_eq!(log.seal(), SealState::Dormant);
        for b in BOSSES {
            log.record(b.id);
        }
        assert_eq!(log.shards(), 19);
        assert_eq!(log.seal(), SealState::Open);
        assert!(log.next_up().is_none(), "a full sweep still had a next");
    }

    #[test]
    fn the_ladder_recommends_upward() {
        // next_up climbs tiers as clears accumulate, and never recommends
        // a fallen warden
        let mut log = RaidLog::default();
        let mut last_tier = 0;
        while let Some(b) = log.next_up() {
            assert!(b.tier >= last_tier, "{} stepped the ladder down", b.id);
            assert!(!log.is_cleared(b.id));
            last_tier = b.tier;
            log.record(b.id);
        }
        assert_eq!(log.shards(), 19, "the ladder should walk the whole roster");
    }

    #[test]
    fn recovered_relics_are_exactly_the_cleared_ones() {
        let mut log = RaidLog::default();
        log.record("sigillum");
        let got: Vec<&str> = log.recovered().map(|b| b.id).collect();
        assert_eq!(got, vec!["sigillum"]);
    }
}
