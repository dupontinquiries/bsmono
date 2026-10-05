//! Player save state: unlocked cars and campaign position. Deliberately has
//! no filesystem code — `core` stays headless and testable; the shell decides
//! *where* a save file lives (XDG data dir, etc.) and just hands the string
//! contents to `SaveState::from_json` / gets a string back from `to_json_pretty`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use crate::level::LevelData;
use crate::vehicle::CarKind;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveState {
    unlocked_cars: Vec<CarKind>,
    pub campaign_level_index: usize,
    /// Best completion time per level, keyed by level name. `BTreeMap` rather
    /// than `HashMap` purely so saved JSON has a stable key order — easier to
    /// diff/review a save file by hand, no functional difference otherwise.
    best_times: BTreeMap<String, f32>,
    /// The M key's state. A player who muted the game expects it to stay muted; the first thing a
    /// relaunch did was blast the engine bed at full volume. `default` so a save from before the field
    /// existed still loads (unmuted).
    #[serde(default)]
    pub muted: bool,
    /// F11's state: borderless fullscreen, remembered like the mute.
    #[serde(default)]
    pub fullscreen: bool,
}

impl SaveState {
    /// A fresh save: only the starting car available, campaign at level 0.
    /// RedSedan is the starting car to match `RoadWarriorApp`'s default in
    /// the shell — if that default ever changes, this should move with it.
    pub fn new() -> Self {
        Self {
            unlocked_cars: vec![CarKind::RedSedan],
            campaign_level_index: 0,
            best_times: BTreeMap::new(),
            muted: false,
            fullscreen: false,
        }
    }

    pub fn from_json(s: &str) -> serde_json::Result<Self> {
        serde_json::from_str(s)
    }

    pub fn to_json_pretty(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    pub fn is_unlocked(&self, car: CarKind) -> bool {
        self.unlocked_cars.contains(&car)
    }

    pub fn unlocked_cars(&self) -> &[CarKind] {
        &self.unlocked_cars
    }

    /// Idempotent — unlocking an already-unlocked car is a no-op, not a duplicate.
    pub fn unlock_car(&mut self, car: CarKind) {
        if !self.is_unlocked(car) {
            self.unlocked_cars.push(car);
        }
    }

    /// Applies a completed level's reward, if it has one. Returns the newly
    /// unlocked car so the shell can show an "unlocked!" notification,
    /// distinguishing a real new unlock from re-completing an old level.
    pub fn apply_level_reward(&mut self, level: &LevelData) -> Option<CarKind> {
        match level.unlock_reward {
            Some(car) if !self.is_unlocked(car) => {
                self.unlock_car(car);
                Some(car)
            }
            _ => None,
        }
    }

    pub fn best_time(&self, level_name: &str) -> Option<f32> {
        self.best_times.get(level_name).copied()
    }

    /// Records a completion time if it's better than (or the first for) the
    /// given level. Returns `true` if this became the new best, so the shell
    /// can show a "new record!" flourish only when one actually happened.
    /// A tied time is *not* a new record — deliberately strict, since a tie
    /// isn't an improvement and shouldn't trigger the same celebration.
    pub fn record_time(&mut self, level_name: &str, time: f32) -> bool {
        // A BEST TIME IS A FINITE, POSITIVE DURATION.
        //
        // Without this, a NaN was stored AND reported as a new record:
        // `time >= existing` is false for NaN, so it fell through to the
        // insert arm. It then reached the save file on disk and the HUD,
        // where "best 45.2s" became "best NaN".
        //
        // Note the asymmetry that made it worse: the `>=` test rejects a
        // slower time, and NaN fails every comparison, so a bad value
        // looked exactly like a very fast one.
        //
        // Returning false is already this function's answer for "not a new
        // record", so an unusable time routes to an existing behaviour and
        // leaves whatever was there before intact. This matters more once
        // times are shared -- the LAN and leaderboard work in BACKLOG.md
        // would otherwise propagate one bad float to everyone.
        if !time.is_finite() || time <= 0.0 {
            return false;
        }
        match self.best_times.get(level_name) {
            Some(&existing) if time >= existing => false,
            _ => {
                self.best_times.insert(level_name.to_string(), time);
                true
            }
        }
    }
}

impl Default for SaveState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_non_finite_time_is_not_a_record() {
        // NaN fails every comparison, so `time >= existing` was false and
        // it fell through to the insert — stored, reported as a record,
        // written to disk and shown in the HUD.
        let mut save = SaveState::new();
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -5.0] {
            assert!(!save.record_time("Dust Bowl", bad), "{bad} was accepted as a best time");
            assert_eq!(save.best_time("Dust Bowl"), None, "{bad} was stored");
        }
    }

    #[test]
    fn a_bad_time_does_not_overwrite_a_good_one() {
        let mut save = SaveState::new();
        assert!(save.record_time("Dust Bowl", 42.0));
        for bad in [f32::NAN, f32::INFINITY, -1.0] {
            assert!(!save.record_time("Dust Bowl", bad));
            assert_eq!(save.best_time("Dust Bowl"), Some(42.0), "{bad} clobbered a real record");
        }
    }
    use super::*;
    use crate::track::Segment;

    fn level_with_reward(car: CarKind) -> LevelData {
        LevelData {
            name: "Reward Level".to_string(),
            segments: vec![Segment::Flat { length: 10.0, height: 0.0 }],
            boss: None,
            unlock_reward: Some(car),
        }
    }

    #[test]
    fn fresh_save_only_has_starting_car() {
        let save = SaveState::new();
        assert!(save.is_unlocked(CarKind::RedSedan));
        assert!(!save.is_unlocked(CarKind::Behemoth));
        assert!(!save.is_unlocked(CarKind::SixWheeler));
        assert_eq!(save.campaign_level_index, 0);
    }

    #[test]
    fn unlocking_a_car_is_idempotent() {
        let mut save = SaveState::new();
        save.unlock_car(CarKind::Behemoth);
        save.unlock_car(CarKind::Behemoth);
        assert_eq!(save.unlocked_cars().iter().filter(|&&c| c == CarKind::Behemoth).count(), 1);
    }

    #[test]
    fn apply_level_reward_unlocks_new_car_and_reports_it() {
        let mut save = SaveState::new();
        let level = level_with_reward(CarKind::SixWheeler);
        let unlocked = save.apply_level_reward(&level);
        assert_eq!(unlocked, Some(CarKind::SixWheeler));
        assert!(save.is_unlocked(CarKind::SixWheeler));
    }

    #[test]
    fn reapplying_an_already_unlocked_reward_reports_nothing_new() {
        let mut save = SaveState::new();
        let level = level_with_reward(CarKind::SixWheeler);
        save.apply_level_reward(&level);
        let second = save.apply_level_reward(&level);
        assert_eq!(second, None, "re-completing a level shouldn't report a fresh unlock");
    }

    #[test]
    fn level_with_no_reward_unlocks_nothing() {
        let mut save = SaveState::new();
        let level = LevelData {
            name: "No Reward".to_string(),
            segments: vec![Segment::Flat { length: 10.0, height: 0.0 }],
            boss: None,
            unlock_reward: None,
        };
        assert_eq!(save.apply_level_reward(&level), None);
    }

    #[test]
    fn round_trips_through_json() {
        let mut save = SaveState::new();
        save.unlock_car(CarKind::Pillager);
        save.campaign_level_index = 2;

        let json = save.to_json_pretty().unwrap();
        let loaded = SaveState::from_json(&json).unwrap();
        assert!(loaded.is_unlocked(CarKind::Pillager));
        assert!(loaded.is_unlocked(CarKind::RedSedan));
        assert_eq!(loaded.campaign_level_index, 2);
    }

    #[test]
    fn mute_survives_the_save_and_an_old_save_loads_unmuted() {
        let mut save = SaveState::new();
        save.muted = true;
        save.fullscreen = true;
        let loaded = SaveState::from_json(&save.to_json_pretty().unwrap()).unwrap();
        assert!(loaded.muted && loaded.fullscreen);
        // a save written before the field existed
        let old = r#"{"unlocked_cars": ["RedSedan"], "campaign_level_index": 1, "best_times": {}}"#;
        let loaded = SaveState::from_json(old).expect("a pre-mute save still parses");
        assert!(!loaded.muted && !loaded.fullscreen);
        assert_eq!(loaded.campaign_level_index, 1);
    }

    #[test]
    fn malformed_save_json_errors_instead_of_panicking() {
        assert!(SaveState::from_json("not json at all").is_err());
    }

    /// A save file from before `unlock_reward` existed had no way to have
    /// unlocked anything from it — this just documents that old LevelData
    /// (loaded via its own `#[serde(default)]`) naturally yields `None`
    /// rewards, so old level files can't retroactively grant unlocks.
    #[test]
    fn level_data_missing_unlock_reward_field_defaults_to_none() {
        let json = r#"{"name": "Old Level", "segments": [{"Flat": {"length": 10.0, "height": 0.0}}], "boss": null}"#;
        let level: LevelData = serde_json::from_str(json).expect("old-format level JSON should still parse");
        assert_eq!(level.unlock_reward, None);
    }

    #[test]
    fn first_time_recorded_is_always_a_new_best() {
        let mut save = SaveState::new();
        assert!(save.record_time("Dust Bowl", 45.2));
        assert_eq!(save.best_time("Dust Bowl"), Some(45.2));
    }

    #[test]
    fn faster_time_overwrites_the_record() {
        let mut save = SaveState::new();
        save.record_time("Dust Bowl", 45.2);
        assert!(save.record_time("Dust Bowl", 38.9), "a faster time should be a new best");
        assert_eq!(save.best_time("Dust Bowl"), Some(38.9));
    }

    #[test]
    fn slower_time_does_not_overwrite_the_record() {
        let mut save = SaveState::new();
        save.record_time("Dust Bowl", 38.9);
        assert!(!save.record_time("Dust Bowl", 45.2), "a slower time shouldn't count as a new best");
        assert_eq!(save.best_time("Dust Bowl"), Some(38.9), "the original best should be untouched");
    }

    #[test]
    fn exact_tie_is_not_reported_as_a_new_record() {
        let mut save = SaveState::new();
        save.record_time("Dust Bowl", 38.9);
        assert!(!save.record_time("Dust Bowl", 38.9), "a tie isn't an improvement, shouldn't trigger a new-record flourish");
    }

    #[test]
    fn best_time_for_unplayed_level_is_none() {
        let save = SaveState::new();
        assert_eq!(save.best_time("Never Played"), None);
    }

    #[test]
    fn best_times_round_trip_through_json() {
        let mut save = SaveState::new();
        save.record_time("Dust Bowl", 38.9);
        save.record_time("Scrapyard Gauntlet", 61.4);

        let json = save.to_json_pretty().unwrap();
        let loaded = SaveState::from_json(&json).unwrap();
        assert_eq!(loaded.best_time("Dust Bowl"), Some(38.9));
        assert_eq!(loaded.best_time("Scrapyard Gauntlet"), Some(61.4));
    }
}

