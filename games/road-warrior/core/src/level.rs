//! Data-driven levels: a `LevelData` deserializes from JSON into the same
//! `Segment`/`Attack` types the engine already runs on, then builds a real
//! `Track` and `Boss` from it. Level design becomes authoring a JSON file,
//! not writing Rust — no new code path, just a serialized form of the
//! existing one, so nothing here can drift out of sync with what `sim` and
//! `encounter` actually simulate.

use serde::{Deserialize, Serialize};

use crate::boss::{Attack, Boss};
use crate::track::{Segment, Track};
use crate::vehicle::CarKind;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BossData {
    pub name: String,
    pub max_health: f32,
    pub phase_2_threshold: f32,
    pub attacks: Vec<Attack>,
}

impl BossData {
    pub fn build(&self) -> Boss {
        Boss::new(self.name.clone(), self.max_health, self.phase_2_threshold, self.attacks.clone())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LevelData {
    pub name: String,
    pub segments: Vec<Segment>,
    /// Not every level needs a boss — plain tracks are valid levels too.
    pub boss: Option<BossData>,
    /// Car unlocked by completing this level (defeating the boss, if any,
    /// and reaching the end). `#[serde(default)]` so the two level files
    /// already on disk — authored before this field existed — still parse
    /// as `None` instead of breaking; this is the schema-evolution path any
    /// future field on LevelData should follow.
    #[serde(default)]
    pub unlock_reward: Option<CarKind>,
}

impl LevelData {
    pub fn from_json(s: &str) -> serde_json::Result<Self> {
        serde_json::from_str(s)
    }

    pub fn to_json_pretty(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }

    pub fn build_track(&self) -> Track {
        Track::new(self.name.clone(), self.segments.clone())
    }

    pub fn build_boss(&self) -> Option<Boss> {
        self.boss.as_ref().map(BossData::build)
    }

    /// Builds the track and its boss (if any) together, positioning the boss
    /// a short distance before the end of the track. `Boss::new` defaults to
    /// `pos_x: 0.0` — without this, the boss colocates with the vehicle's
    /// spawn point (also `pos_x: 0.0`) and starts attacking before the
    /// player has driven anywhere. This is the method the shell should
    /// actually call; `build_track`/`build_boss` stay available separately
    /// for tests that don't care about positioning.
    pub fn build(&self) -> (Track, Option<Boss>) {
        let track = self.build_track();
        let boss = self.boss.as_ref().map(|data| {
            let mut boss = data.build();
            boss.pos_x = (track.total_length - 5.0).max(0.0);
            boss
        });
        (track, boss)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boss::AttackKind;

    fn sample_level() -> LevelData {
        LevelData {
            name: "Dust Bowl".to_string(),
            segments: vec![
                Segment::Flat { length: 30.0, height: 0.0 },
                Segment::Ramp { length: 8.0, from_height: 0.0, to_height: 5.0 },
                Segment::Gap { length: 10.0 },
                Segment::Flat { length: 40.0, height: 0.0 },
            ],
            boss: Some(BossData {
                name: "The Behemoth's Rival".to_string(),
                max_health: 1000.0,
                phase_2_threshold: 0.4,
                attacks: vec![
                    Attack { kind: AttackKind::GroundSlam, telegraph: 1.0, cooldown: 2.0 },
                    Attack { kind: AttackKind::Charge, telegraph: 0.5, cooldown: 1.5 },
                ],
            }),
            unlock_reward: Some(crate::vehicle::CarKind::SixWheeler),
        }
    }

    #[test]
    fn round_trips_through_json() {
        let level = sample_level();
        let json = level.to_json_pretty().expect("serialize should succeed");
        let loaded = LevelData::from_json(&json).expect("deserialize should succeed");
        assert_eq!(loaded.name, level.name);
        assert_eq!(loaded.segments.len(), level.segments.len());
        assert!(loaded.boss.is_some());
    }

    #[test]
    fn builds_a_working_track() {
        let level = sample_level();
        let track = level.build_track();
        assert_eq!(track.name, "Dust Bowl");
        // Same total length as summing the authored segments (30+8+10+40).
        assert!((track.total_length - 88.0).abs() < 1e-3);
        assert_eq!(track.height_at(42.0), None, "x=42 falls inside the Gap (38..48); should still produce no ground after round-tripping");
    }

    #[test]
    fn builds_a_working_boss() {
        let level = sample_level();
        let boss = level.build_boss().expect("level has a boss");
        assert_eq!(boss.name, "The Behemoth's Rival");
        assert_eq!(boss.health, 1000.0);
    }

    /// Regression test: `Boss::new` defaults `pos_x` to 0.0, same as a fresh
    /// `Vehicle`'s spawn point. Without `LevelData::build` explicitly
    /// repositioning it, the boss colocates with the player and starts
    /// attacking before they've driven anywhere — confirmed by actually
    /// running the game and watching a stationary vehicle lose HP to a
    /// GroundSlam it never should have been in range of.
    #[test]
    fn build_positions_boss_away_from_the_vehicle_spawn_point() {
        let level = sample_level();
        let (track, boss) = level.build();
        let boss = boss.expect("level has a boss");
        assert!(boss.pos_x > 10.0, "boss should not be colocated with the vehicle's spawn point at x=0");
        assert!(boss.pos_x <= track.total_length, "boss should be within the track, not past its end");
    }

    #[test]
    fn build_clamps_boss_position_on_a_track_shorter_than_the_offset() {
        let level = LevelData {
            name: "Tiny Arena".to_string(),
            segments: vec![Segment::Flat { length: 3.0, height: 0.0 }], // shorter than the 5.0 offset
            boss: Some(BossData {
                name: "Cramped Boss".to_string(),
                max_health: 100.0,
                phase_2_threshold: 0.4,
                attacks: vec![],
            }),
            unlock_reward: None,
        };
        let (_, boss) = level.build();
        assert_eq!(boss.unwrap().pos_x, 0.0, "should clamp to 0 rather than go negative on a very short track");
    }

    #[test]
    fn level_without_boss_builds_none() {
        let mut level = sample_level();
        level.boss = None;
        assert!(level.build_boss().is_none());
    }

    #[test]
    fn malformed_json_returns_error_not_panic() {
        let result = LevelData::from_json("{ not valid json");
        assert!(result.is_err(), "malformed input should error, not panic");
    }

    #[test]
    fn missing_required_field_returns_error() {
        // Valid JSON, but missing the required `segments` field.
        let result = LevelData::from_json(r#"{"name": "Broken Level"}"#);
        assert!(result.is_err(), "missing required fields should be a clean error, not a default-filled struct");
    }

    fn scrapyard_gauntlet_level() -> LevelData {
        // Mirrors shell/assets/scrapyard_gauntlet.json — deliberately more
        // complex than sample_level() (back-to-back gaps, a downhill ramp,
        // negative heights, a three-attack boss) so the smoke test below
        // actually exercises more than the one hand-tuned happy path.
        LevelData {
            name: "Scrapyard Gauntlet".to_string(),
            segments: vec![
                Segment::Flat { length: 15.0, height: 0.0 },
                Segment::Ramp { length: 6.0, from_height: 0.0, to_height: 3.0 },
                Segment::Gap { length: 7.0 },
                Segment::Ramp { length: 4.0, from_height: 3.0, to_height: 6.0 },
                Segment::Gap { length: 9.0 },
                Segment::Flat { length: 12.0, height: 0.0 },
                Segment::Ramp { length: 10.0, from_height: 0.0, to_height: -4.0 },
                Segment::Flat { length: 25.0, height: -4.0 },
            ],
            boss: Some(BossData {
                name: "Junkyard Colossus".to_string(),
                max_health: 1500.0,
                phase_2_threshold: 0.5,
                attacks: vec![
                    Attack { kind: AttackKind::Lobbed, telegraph: 0.8, cooldown: 1.8 },
                    Attack { kind: AttackKind::GroundSlam, telegraph: 0.6, cooldown: 1.5 },
                    Attack { kind: AttackKind::Charge, telegraph: 0.4, cooldown: 1.2 },
                ],
            }),
            unlock_reward: Some(crate::vehicle::CarKind::Behemoth),
        }
    }

    fn rust_canyon_level() -> LevelData {
        // Mirrors shell/assets/rust_canyon.json — the widest single gap in
        // the campaign, and the level that closes the "Pillager is otherwise
        // unreachable" gap caught by the reachability test below.
        LevelData {
            name: "Rust Canyon".to_string(),
            segments: vec![
                Segment::Flat { length: 20.0, height: 0.0 },
                Segment::Ramp { length: 5.0, from_height: 0.0, to_height: 2.0 },
                Segment::Flat { length: 8.0, height: 2.0 },
                Segment::Gap { length: 12.0 },
                Segment::Ramp { length: 6.0, from_height: 0.0, to_height: -3.0 },
                Segment::Flat { length: 30.0, height: -3.0 },
            ],
            boss: Some(BossData {
                name: "The Rustbound Warden".to_string(),
                max_health: 1800.0,
                phase_2_threshold: 0.35,
                attacks: vec![
                    Attack { kind: AttackKind::Charge, telegraph: 0.9, cooldown: 2.0 },
                    Attack { kind: AttackKind::GroundSlam, telegraph: 0.5, cooldown: 1.3 },
                    Attack { kind: AttackKind::Lobbed, telegraph: 0.35, cooldown: 1.0 },
                ],
            }),
            unlock_reward: Some(crate::vehicle::CarKind::Pillager),
        }
    }

    /// Drives a vehicle at full throttle across a whole level's track and
    /// asserts it reaches the far end. Catches the class of authoring bug
    /// unit tests on individual segments can't: a gap too wide for any car's
    /// physics to clear, a track that never terminates, or a NaN creeping in
    /// over a long run — none of which would show up testing one segment at a time.
    fn drive_full_throttle_to_end(level: &LevelData, kind: crate::vehicle::CarKind) {
        let track = level.build_track();
        let mut vehicle = crate::vehicle::Vehicle::new(kind);
        let max_ticks = 60 * 120; // 2 minutes of sim time is generous for any of these tracks
        for _ in 0..max_ticks {
            crate::sim::advance(&mut vehicle, &track, 1.0 / 60.0, 1.0, 0.0, false, -9.8);
            assert!(vehicle.pos_x.is_finite(), "vehicle position went non-finite for {}", level.name);
            if vehicle.pos_x >= track.total_length - 1.0 {
                return;
            }
        }
        panic!(
            "{} ({:?}) never reached the end of the track: stopped at pos_x={} of total_length={}",
            level.name, kind, vehicle.pos_x, track.total_length
        );
    }

    #[test]
    fn dust_bowl_is_completable_by_every_car() {
        let level = sample_level();
        for kind in [
            crate::vehicle::CarKind::Behemoth,
            crate::vehicle::CarKind::SixWheeler,
            crate::vehicle::CarKind::RedSedan,
            crate::vehicle::CarKind::Pillager,
        ] {
            drive_full_throttle_to_end(&level, kind);
        }
    }

    #[test]
    fn scrapyard_gauntlet_is_completable_by_every_car() {
        let level = scrapyard_gauntlet_level();
        for kind in [
            crate::vehicle::CarKind::Behemoth,
            crate::vehicle::CarKind::SixWheeler,
            crate::vehicle::CarKind::RedSedan,
            crate::vehicle::CarKind::Pillager,
        ] {
            drive_full_throttle_to_end(&level, kind);
        }
    }

    #[test]
    fn rust_canyon_is_completable_by_every_car() {
        let level = rust_canyon_level();
        for kind in [
            crate::vehicle::CarKind::Behemoth,
            crate::vehicle::CarKind::SixWheeler,
            crate::vehicle::CarKind::RedSedan,
            crate::vehicle::CarKind::Pillager,
        ] {
            drive_full_throttle_to_end(&level, kind);
        }
    }

    /// Every car should be reachable through some combination of "start with
    /// it" or "unlock it from a level reward" — otherwise a car exists in
    /// code that no player can ever actually drive. This mirrors the real
    /// on-disk campaign's reward assignments, so if shell/assets/campaign.json's
    /// rewards ever change, this duplicated list needs to change with it —
    /// worth eventually reading the real file instead of mirroring it by hand.
    #[test]
    fn every_car_is_reachable_via_starting_car_or_campaign_rewards() {
        use crate::vehicle::CarKind;

        let starting_car = CarKind::RedSedan;
        let campaign_rewards = [
            sample_level().unlock_reward,
            scrapyard_gauntlet_level().unlock_reward,
            rust_canyon_level().unlock_reward,
        ];

        let mut reachable: Vec<CarKind> = vec![starting_car];
        reachable.extend(campaign_rewards.into_iter().flatten());

        for car in CarKind::ALL {
            assert!(
                reachable.contains(&car),
                "{car:?} is not reachable: not the starting car and no level in the campaign unlocks it"
            );
        }
    }
}
