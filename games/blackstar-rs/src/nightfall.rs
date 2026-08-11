//! NIGHTFALL — the week's mission, run hot.
//!
//! Straight from Destiny 1: one known mission, remixed by modifiers,
//! once a week. The remix here is an ELEMENT BURN (that element runs at
//! 2.5x for both sides — bring it and fear it) plus, on alternating
//! weeks, EXPOSURE (no field repair between waves: every wave is paid
//! for in the hp you have left).
//!
//! Determinism does the scheduling: the week number is days-since-epoch
//! over seven, so every player on the planet gets the same card without
//! a server existing anywhere. The card cycles missions and burns on
//! coprime periods (15 and 7), so the same pairing repeats only every
//! 105 weeks — two years of distinct Tuesdays from two modular indexes.
//!
//! The lockout rides the save as a `nightfalls` line of week numbers:
//! cleared weeks bank energon (`energon::PER_NIGHTFALL`) and cannot be
//! farmed twice. Ids-not-indices does not apply — a week IS its number.

use crate::content::{Mission, MISSIONS};
use crate::weapons::{Element, ELEMENTS};

/// The week's card.
#[derive(Clone, Copy)]
pub struct Nightfall {
    pub week: u64,
    pub mission: &'static Mission,
    pub burn: Element,
    /// no breather repair this week
    pub exposure: bool,
}

/// Weeks since the epoch, UTC. The one place wall-clock time enters, so
/// everything downstream stays testable by passing a week directly.
pub fn week_now() -> u64 {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    secs / (7 * 24 * 3600)
}

/// The card for a given week — pure, so tests and the sim agree by
/// construction.
pub fn card(week: u64) -> Nightfall {
    Nightfall {
        week,
        mission: &MISSIONS[(week % MISSIONS.len() as u64) as usize],
        burn: ELEMENTS[(week % ELEMENTS.len() as u64) as usize],
        exposure: week % 2 == 0,
    }
}

/// The lockout, parsed leniently from the save's `nightfalls` line.
#[derive(Clone, Default)]
pub struct NightfallLog {
    weeks: Vec<u64>,
}

impl NightfallLog {
    pub fn from_line(line: &str) -> NightfallLog {
        let mut log = NightfallLog::default();
        for w in line.split(',').filter_map(|s| s.trim().parse().ok()) {
            log.record(w);
        }
        log
    }
    pub fn line(&self) -> String {
        self.weeks.iter().map(|w| w.to_string()).collect::<Vec<_>>().join(",")
    }
    /// Bank a week. Returns whether it was new — the lockout.
    pub fn record(&mut self, week: u64) -> bool {
        if self.weeks.contains(&week) {
            return false;
        }
        self.weeks.push(week);
        self.weeks.sort_unstable();
        true
    }
    pub fn cleared(&self, week: u64) -> bool {
        self.weeks.contains(&week)
    }
    /// How many nightfalls have been run down — the light trickle.
    pub fn count(&self) -> u32 {
        self.weeks.len() as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_card_is_deterministic_and_cycles_on_coprime_periods() {
        assert_eq!(card(7).mission.id, card(7).mission.id);
        assert_eq!(card(7).burn, card(7).burn);
        // 15 missions x 7 burns: the pairing has period 105
        let (a, b) = (card(3), card(3 + 105));
        assert_eq!(a.mission.id, b.mission.id);
        assert_eq!(a.burn, b.burn);
        // and inside the period, adjacent weeks change BOTH dials
        assert_ne!(card(3).mission.id, card(4).mission.id);
        assert_ne!(card(3).burn, card(4).burn);
    }

    #[test]
    fn the_lockout_holds_and_roundtrips() {
        let mut log = NightfallLog::from_line("9, 4,junk,4");
        assert_eq!(log.count(), 2);
        assert!(log.cleared(4) && log.cleared(9) && !log.cleared(5));
        assert!(!log.record(4), "a cleared week banked twice");
        assert!(log.record(5));
        assert_eq!(NightfallLog::from_line(&log.line()).count(), 3);
    }
}
