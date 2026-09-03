//! The seam between a boss fight and everything else: `vehicle`, `track`, and
//! `boss` each stay ignorant of one another. This module is the only place
//! that knows "ramming deals damage" and "an active attack that overlaps the
//! vehicle hurts it" — game-rules glue, not physics or AI.

use crate::boss::{attack_hits_vehicle, Boss};
use crate::track::Track;
use crate::vehicle::Vehicle;

const RAM_SPEED_THRESHOLD: f32 = 8.0; // m/s; below this, contact doesn't count as a ram
const RAM_CONTACT_RADIUS: f32 = 2.0; // world units

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EncounterOutcome {
    pub boss_damage_dealt: f32,
    pub player_damage_taken: f32,
    pub boss_defeated_this_tick: bool,
}

/// Advance vehicle + boss by `dt` against `track`, resolving one tick of
/// combat. Returns what happened so the shell can trigger feedback (hit
/// flash, sound, screen shake) without needing to poll deltas itself.
pub fn advance(
    vehicle: &mut Vehicle,
    track: &Track,
    boss: &mut Boss,
    dt: f32,
    throttle: f32,
    tilt: f32,
    nitro_active: bool,
    gravity: f32,
) -> EncounterOutcome {
    let was_defeated = boss.phase() == crate::boss::BossPhase::Defeated;

    crate::sim::advance(vehicle, track, dt, throttle, tilt, nitro_active, gravity);
    let fired_attack = boss.step(dt);

    let mut outcome = EncounterOutcome::default();

    // Ramming: fast contact with the boss deals damage to it, gated by
    // Boss's own cooldown so holding contact isn't a DPS exploit.
    let dx = (vehicle.pos_x - boss.pos_x).abs();
    if dx < RAM_CONTACT_RADIUS && vehicle.vel_x.abs() > RAM_SPEED_THRESHOLD {
        let ram_damage = vehicle.vel_x.abs() * vehicle.stats.mass / 400.0;
        if boss.try_take_ram_damage(ram_damage) {
            outcome.boss_damage_dealt = ram_damage;
        }
    }

    // A ramming vehicle also takes a little damage itself — hitting a boss
    // head-on isn't free, matching the original's risk/reward on aggressive play.
    if outcome.boss_damage_dealt > 0.0 {
        let self_damage = outcome.boss_damage_dealt * 0.15;
        vehicle.health = (vehicle.health - self_damage).max(0.0);
    }

    if let Some(kind) = fired_attack {
        if attack_hits_vehicle(kind, boss.pos_x, vehicle) {
            let damage = match kind {
                crate::boss::AttackKind::GroundSlam => 20.0,
                crate::boss::AttackKind::Charge => 30.0,
                crate::boss::AttackKind::Lobbed => 15.0,
            };
            vehicle.health = (vehicle.health - damage).max(0.0);
            outcome.player_damage_taken = damage;
        }
    }

    if !was_defeated && boss.phase() == crate::boss::BossPhase::Defeated {
        outcome.boss_defeated_this_tick = true;
    }

    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::boss::{Attack, AttackKind};
    use crate::track::Segment;
    use crate::vehicle::CarKind;

    fn flat_track() -> Track {
        Track::new("Arena", vec![Segment::Flat { length: 100.0, height: 0.0 }])
    }

    fn weak_boss() -> Boss {
        Boss::new("Practice Dummy", 100.0, 0.4, vec![])
    }

    #[test]
    fn ramming_at_speed_damages_boss() {
        let track = flat_track();
        let mut boss = weak_boss();
        boss.pos_x = 5.0;
        let mut v = Vehicle::new(CarKind::Behemoth);
        v.pos_x = 4.5;
        v.vel_x = 20.0; // well above ram threshold

        let outcome = advance(&mut v, &track, &mut boss, 1.0 / 60.0, 1.0, 0.0, false, -9.8);
        assert!(outcome.boss_damage_dealt > 0.0, "fast contact should ram the boss");
        assert!(boss.health < boss.max_health);
    }

    #[test]
    fn slow_contact_does_not_count_as_ram() {
        let track = flat_track();
        let mut boss = weak_boss();
        boss.pos_x = 5.0;
        let mut v = Vehicle::new(CarKind::Behemoth);
        v.pos_x = 4.9;
        v.vel_x = 1.0; // below RAM_SPEED_THRESHOLD

        let outcome = advance(&mut v, &track, &mut boss, 1.0 / 60.0, 0.1, 0.0, false, -9.8);
        assert_eq!(outcome.boss_damage_dealt, 0.0, "slow contact shouldn't count as a ram");
        assert_eq!(boss.health, boss.max_health);
    }

    #[test]
    fn ramming_costs_the_vehicle_some_health() {
        let track = flat_track();
        let mut boss = weak_boss();
        boss.pos_x = 5.0;
        let mut v = Vehicle::new(CarKind::Behemoth);
        v.pos_x = 4.5;
        v.vel_x = 20.0;
        let starting_health = v.health;

        advance(&mut v, &track, &mut boss, 1.0 / 60.0, 1.0, 0.0, false, -9.8);
        assert!(v.health < starting_health, "ramming should cost the vehicle some health too");
    }

    #[test]
    fn active_attack_overlapping_vehicle_deals_damage() {
        let track = flat_track();
        let mut boss = Boss::new(
            "Attacker",
            500.0,
            0.4,
            vec![Attack { kind: AttackKind::Charge, telegraph: 0.0, cooldown: 5.0 }],
        );
        boss.pos_x = 5.0;
        let mut v = Vehicle::new(CarKind::Pillager);
        v.pos_x = 5.5; // within Charge's hit radius
        v.vel_x = 0.0; // not fast enough to ram, isolating the attack-damage path
        let starting_health = v.health;

        // telegraph is 0, but Boss::step needs one tick to arm the attack
        // (set up the countdown) and a second tick to see it expire and fire —
        // consistent with how the telegraph state machine is written elsewhere.
        advance(&mut v, &track, &mut boss, 1.0 / 60.0, 0.0, 0.0, false, -9.8);
        let outcome = advance(&mut v, &track, &mut boss, 1.0 / 60.0, 0.0, 0.0, false, -9.8);
        assert!(outcome.player_damage_taken > 0.0, "an active attack overlapping the vehicle should deal damage");
        assert!(v.health < starting_health);
    }

    #[test]
    fn defeating_boss_reports_it_only_once() {
        let track = flat_track();
        let mut boss = weak_boss();
        boss.health = 1.0; // one ram from a Behemoth will finish it
        boss.pos_x = 5.0;
        let mut v = Vehicle::new(CarKind::Behemoth);
        v.pos_x = 4.5;
        v.vel_x = 20.0;

        let first = advance(&mut v, &track, &mut boss, 1.0 / 60.0, 1.0, 0.0, false, -9.8);
        assert!(first.boss_defeated_this_tick, "boss should be reported defeated the tick it dies");

        let second = advance(&mut v, &track, &mut boss, 1.0 / 60.0, 1.0, 0.0, false, -9.8);
        assert!(!second.boss_defeated_this_tick, "defeat should only be reported once, not every subsequent tick");
    }
}

