//! Campaign: an ordered list of levels plus the "did the player actually
//! finish this one" rule. Kept separate from `level.rs` because a level's
//! *shape* (track + boss data) and its *completion criteria* are different
//! concerns — a level format shouldn't need to change just because the rule
//! for "finished" changes (e.g. adding a time trial mode later).

use serde::{Deserialize, Serialize};

use crate::boss::{Boss, BossPhase};
use crate::level::LevelData;
use crate::track::Track;
use crate::vehicle::Vehicle;

/// A level is complete when the vehicle reaches the far end of the track,
/// AND its boss (if any) is defeated — matching the original's structure of
/// "drive the gauntlet, then beat what's waiting at the end."
pub fn is_level_complete(vehicle: &Vehicle, track: &Track, boss: Option<&Boss>) -> bool {
    let reached_end = vehicle.pos_x >= track.total_length - 1.0;
    let boss_down = boss.map_or(true, |b| b.phase() == BossPhase::Defeated);
    reached_end && boss_down
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Campaign {
    pub levels: Vec<LevelData>,
    current: usize,
}

impl Campaign {
    pub fn new(levels: Vec<LevelData>) -> Self {
        Self { levels, current: 0 }
    }

    pub fn from_json(s: &str) -> serde_json::Result<Self> {
        let levels: Vec<LevelData> = serde_json::from_str(s)?;
        Ok(Self::new(levels))
    }

    pub fn to_json_pretty(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(&self.levels)
    }

    pub fn current_level(&self) -> Option<&LevelData> {
        self.levels.get(self.current)
    }

    pub fn current_index(&self) -> usize {
        self.current
    }

    pub fn is_finished(&self) -> bool {
        self.current >= self.levels.len()
    }

    /// Jumps directly to `index` (e.g. restoring from a save file). Clamps to
    /// the valid range rather than panicking on a corrupt/out-of-range save.
    pub fn seek(&mut self, index: usize) {
        self.current = index.min(self.levels.len());
    }

    /// Moves to the next level. Returns `false` (and does nothing) if this
    /// was already the last level — the caller checks `is_finished()` to
    /// tell "campaign complete" from "load failed."
    pub fn advance(&mut self) -> bool {
        if self.current + 1 < self.levels.len() {
            self.current += 1;
            true
        } else {
            // Still move past the end so is_finished() becomes true; there's
            // no valid "current" level once the last one is done.
            self.current = self.levels.len();
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boss::{Attack, AttackKind, Boss};
    use crate::level::BossData;
    use crate::track::Segment;
    use crate::vehicle::CarKind;

    fn two_level_campaign() -> Campaign {
        Campaign::new(vec![
            LevelData {
                name: "Level One".to_string(),
                segments: vec![Segment::Flat { length: 20.0, height: 0.0 }],
                boss: None,
                unlock_reward: None,
            },
            LevelData {
                name: "Level Two".to_string(),
                segments: vec![Segment::Flat { length: 20.0, height: 0.0 }],
                boss: Some(BossData {
                    name: "Gatekeeper".to_string(),
                    max_health: 50.0,
                    phase_2_threshold: 0.4,
                    attacks: vec![Attack { kind: AttackKind::Charge, telegraph: 1.0, cooldown: 2.0 }],
                }),
                unlock_reward: Some(CarKind::SixWheeler),
            },
        ])
    }

    #[test]
    fn starts_on_first_level() {
        let campaign = two_level_campaign();
        assert_eq!(campaign.current_index(), 0);
        assert_eq!(campaign.current_level().unwrap().name, "Level One");
        assert!(!campaign.is_finished());
    }

    #[test]
    fn advance_moves_to_next_level_and_reports_true() {
        let mut campaign = two_level_campaign();
        assert!(campaign.advance());
        assert_eq!(campaign.current_level().unwrap().name, "Level Two");
        assert!(!campaign.is_finished());
    }

    #[test]
    fn advancing_past_the_last_level_finishes_the_campaign() {
        let mut campaign = two_level_campaign();
        campaign.advance(); // -> Level Two
        assert!(!campaign.advance(), "advancing past the last level should report false");
        assert!(campaign.is_finished());
        assert!(campaign.current_level().is_none(), "no current level once finished");
    }

    #[test]
    fn round_trips_through_json() {
        let campaign = two_level_campaign();
        let json = campaign.to_json_pretty().unwrap();
        let loaded = Campaign::from_json(&json).unwrap();
        assert_eq!(loaded.levels.len(), 2);
        assert_eq!(loaded.current_level().unwrap().name, "Level One");
    }

    #[test]
    fn level_without_boss_completes_on_reaching_the_end() {
        let level = LevelData {
            name: "No Boss".to_string(),
            segments: vec![Segment::Flat { length: 20.0, height: 0.0 }],
            boss: None,
            unlock_reward: None,
        };
        let track = level.build_track();
        let mut v = Vehicle::new(CarKind::Pillager);
        v.pos_x = 19.5;
        assert!(is_level_complete(&v, &track, None), "reaching the end with no boss should complete the level");
    }

    #[test]
    fn level_with_undefeated_boss_is_not_complete_even_at_the_end() {
        let boss = Boss::new("Blocker", 100.0, 0.4, vec![]);
        let track = Track::new("Test", vec![Segment::Flat { length: 20.0, height: 0.0 }]);
        let mut v = Vehicle::new(CarKind::Pillager);
        v.pos_x = 19.5;
        assert!(!is_level_complete(&v, &track, Some(&boss)), "reaching the end shouldn't count if the boss is still alive");
    }

    #[test]
    fn level_with_defeated_boss_completes_at_the_end() {
        let mut boss = Boss::new("Blocker", 100.0, 0.4, vec![]);
        boss.apply_damage(100.0);
        let track = Track::new("Test", vec![Segment::Flat { length: 20.0, height: 0.0 }]);
        let mut v = Vehicle::new(CarKind::Pillager);
        v.pos_x = 19.5;
        assert!(is_level_complete(&v, &track, Some(&boss)));
    }

    #[test]
    fn seek_jumps_to_a_given_level() {
        let mut campaign = two_level_campaign();
        campaign.seek(1);
        assert_eq!(campaign.current_level().unwrap().name, "Level Two");
    }

    #[test]
    fn seek_past_the_end_clamps_to_finished() {
        let mut campaign = two_level_campaign();
        campaign.seek(99);
        assert!(campaign.is_finished(), "seeking past the end should clamp, not panic or leave a dangling index");
    }
}
