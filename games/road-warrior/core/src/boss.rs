//! Boss encounters: each boss has phases gated by health thresholds, and an
//! attack pattern that telegraphs before it fires — matching the read-and-react
//! boss design the original games were built around, rather than a bullet-sponge.

use crate::vehicle::Vehicle;

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum AttackKind {
    /// Ground-level obstacle the player must jump or flip over.
    GroundSlam,
    /// Projectile arcing toward the player's predicted position.
    Lobbed,
    /// Boss charges directly at the player at high speed.
    Charge,
}

#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct Attack {
    pub kind: AttackKind,
    /// Seconds of telegraph before the attack becomes active/dangerous.
    pub telegraph: f32,
    pub cooldown: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BossPhase {
    /// Above phase_2_threshold health: slower, single-attack pattern.
    Opening,
    /// Below phase_2_threshold: faster attacks, may combo two in a row.
    Enraged,
    Defeated,
}

#[derive(Debug, Clone)]
pub struct Boss {
    pub name: String,
    pub max_health: f32,
    pub health: f32,
    pub phase_2_threshold: f32, // fraction of max_health, e.g. 0.4
    pub pos_x: f32,
    pub attacks: Vec<Attack>,
    current_attack: Option<(usize, f32)>, // (attack index, time remaining in telegraph)
    attack_timer: f32,
    ram_cooldown: f32,
}

impl Boss {
    pub fn new(name: impl Into<String>, max_health: f32, phase_2_threshold: f32, attacks: Vec<Attack>) -> Self {
        // AN UNFIGHTABLE BOSS MUST NOT BE AN UNBEATABLE ONE.
        //
        // These numbers come from level JSON, which is hand-edited today
        // and generated tomorrow (see BACKLOG.md). A non-finite
        // `max_health` made the boss INVINCIBLE and silent about it:
        // `phase()` decides Defeated with `health <= 0.0`, and `NaN <= 0.0`
        // is false, so the boss never dies — and `is_level_complete`
        // requires the boss dead, so the campaign stops there permanently.
        //
        // Zero is the answer, because a boss with no health is already
        // Defeated and that is an existing, handled state: the level
        // completes without a fight. A level whose boss data is broken
        // should be finishable, not a wall.
        let max_health = if max_health.is_finite() && max_health > 0.0 { max_health } else { 0.0 };
        // A non-finite threshold never compares true, so the boss never
        // enrages. Zero keeps it in its opening phase for the whole fight,
        // which is the same outcome stated deliberately.
        let phase_2_threshold = if phase_2_threshold.is_finite() {
            phase_2_threshold.clamp(0.0, 1.0)
        } else {
            0.0
        };
        Self {
            name: name.into(),
            max_health,
            health: max_health,
            phase_2_threshold,
            pos_x: 0.0,
            attacks,
            current_attack: None,
            attack_timer: 0.0,
            ram_cooldown: 0.0,
        }
    }

    pub fn phase(&self) -> BossPhase {
        if self.health <= 0.0 {
            BossPhase::Defeated
        } else if self.health / self.max_health <= self.phase_2_threshold {
            BossPhase::Enraged
        } else {
            BossPhase::Opening
        }
    }

    pub fn apply_damage(&mut self, amount: f32) {
        self.health = (self.health - amount).max(0.0);
    }

    /// Advance the attack pattern. Returns `Some(AttackKind)` the instant an
    /// attack goes live (telegraph finished) so the caller can spawn its hazard.
    pub fn step(&mut self, dt: f32) -> Option<AttackKind> {
        self.ram_cooldown = (self.ram_cooldown - dt).max(0.0);

        if self.phase() == BossPhase::Defeated || self.attacks.is_empty() {
            return None;
        }

        if let Some((idx, remaining)) = self.current_attack {
            let remaining = remaining - dt;
            if remaining <= 0.0 {
                self.current_attack = None;
                self.attack_timer = self.attacks[idx].cooldown;
                return Some(self.attacks[idx].kind);
            }
            self.current_attack = Some((idx, remaining));
            return None;
        }

        self.attack_timer -= dt;
        if self.attack_timer <= 0.0 {
            // Enraged phase picks from the full pattern set; Opening phase
            // sticks to the first (intended to be the simplest) attack.
            let idx = if self.phase() == BossPhase::Enraged {
                (self.attacks.len() - 1).min(1)
            } else {
                0
            };
            let telegraph = self.attacks[idx].telegraph;
            self.current_attack = Some((idx, telegraph));
        }
        None
    }

    /// True while a telegraph is active — shell can flash the boss or the
    /// danger zone so the attack reads as reactable, not a surprise hit.
    pub fn is_telegraphing(&self) -> bool {
        self.current_attack.is_some()
    }

    /// Apply ramming damage if the cooldown has expired. Returns whether the
    /// hit actually landed, so the caller can trigger hit-feedback (screen
    /// shake, sound) only on real hits, not on every frame of contact.
    pub fn try_take_ram_damage(&mut self, amount: f32) -> bool {
        if self.ram_cooldown > 0.0 || self.phase() == BossPhase::Defeated {
            return false;
        }
        self.apply_damage(amount);
        self.ram_cooldown = 0.5; // seconds between ram hits registering
        true
    }
}

/// Simple collision check between a vehicle and the boss's hazard footprint,
/// used by the caller after `Boss::step` returns an active attack.
pub fn attack_hits_vehicle(attack: AttackKind, boss_x: f32, vehicle: &Vehicle) -> bool {
    let dx = (vehicle.pos_x - boss_x).abs();
    match attack {
        AttackKind::GroundSlam => dx < 4.0 && !vehicle.airborne,
        AttackKind::Charge => dx < 6.0,
        AttackKind::Lobbed => dx < 3.0 && vehicle.pos_y < 1.0,
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn a_boss_with_broken_health_data_is_beatable_not_invincible() {
        // `phase()` decides Defeated with `health <= 0.0`, and NaN fails
        // that test, so a NaN max_health produced a boss that could never
        // die — and `is_level_complete` requires a dead boss, so the
        // campaign would stop at that level forever.
        for max in [f32::NAN, f32::INFINITY, 0.0, -100.0] {
            let b = Boss::new("Broken", max, 0.4, vec![]);
            assert!(b.health.is_finite(), "max={max} gave health {}", b.health);
            assert_eq!(b.phase(), BossPhase::Defeated, "max={max} left a boss that cannot be killed");
        }
    }

    #[test]
    fn a_broken_phase_threshold_does_not_wedge_the_phase() {
        let mut b = Boss::new("B", 100.0, f32::NAN, vec![]);
        assert_eq!(b.phase(), BossPhase::Opening);
        b.apply_damage(99.9);
        // With a sanitised threshold the phase is well-defined at every
        // health; before, every comparison against NaN was false.
        assert!(matches!(b.phase(), BossPhase::Opening | BossPhase::Enraged));
    }

    #[test]
    fn a_healthy_boss_is_unaffected_by_the_guards() {
        let b = Boss::new("Real", 1000.0, 0.4, vec![]);
        assert_eq!(b.max_health, 1000.0);
        assert_eq!(b.health, 1000.0);
        assert_eq!(b.phase(), BossPhase::Opening);
    }
    use super::*;
    use crate::vehicle::CarKind;

    fn sample_boss() -> Boss {
        Boss::new(
            "The Behemoth's Rival",
            1000.0,
            0.4,
            vec![
                Attack { kind: AttackKind::GroundSlam, telegraph: 1.0, cooldown: 2.0 },
                Attack { kind: AttackKind::Charge, telegraph: 0.5, cooldown: 1.5 },
            ],
        )
    }

    #[test]
    fn opening_phase_only_uses_first_attack() {
        let mut boss = sample_boss();
        let mut fired = vec![];
        for _ in 0..600 {
            if let Some(kind) = boss.step(1.0 / 60.0) {
                fired.push(kind);
            }
        }
        assert!(!fired.is_empty(), "boss should fire at least one attack in 10s");
        assert!(fired.iter().all(|k| *k == AttackKind::GroundSlam), "opening phase should stick to first attack");
    }

    #[test]
    fn enraged_phase_can_use_second_attack() {
        let mut boss = sample_boss();
        boss.health = boss.max_health * 0.3; // force enraged
        let mut fired = vec![];
        for _ in 0..600 {
            if let Some(kind) = boss.step(1.0 / 60.0) {
                fired.push(kind);
            }
        }
        assert!(fired.contains(&AttackKind::Charge), "enraged phase should use the harder attack");
    }

    #[test]
    fn damage_transitions_phase_and_defeat() {
        let mut boss = sample_boss();
        assert_eq!(boss.phase(), BossPhase::Opening);
        boss.apply_damage(700.0);
        assert_eq!(boss.phase(), BossPhase::Enraged);
        boss.apply_damage(1000.0);
        assert_eq!(boss.phase(), BossPhase::Defeated);
        assert_eq!(boss.step(1.0), None, "defeated boss should not attack");
    }

    #[test]
    fn ground_slam_only_hits_grounded_vehicle() {
        let mut v = Vehicle::new(CarKind::Pillager);
        v.pos_x = 1.0;
        v.airborne = false;
        assert!(attack_hits_vehicle(AttackKind::GroundSlam, 0.0, &v));
        v.airborne = true;
        assert!(!attack_hits_vehicle(AttackKind::GroundSlam, 0.0, &v), "airborne vehicle should dodge a ground slam");
    }

    #[test]
    fn ram_damage_respects_cooldown() {
        let mut boss = sample_boss();
        assert!(boss.try_take_ram_damage(50.0), "first ram should land");
        assert_eq!(boss.health, 950.0);
        assert!(!boss.try_take_ram_damage(50.0), "second ram immediately after should be on cooldown");
        assert_eq!(boss.health, 950.0, "health should be unchanged while on cooldown");

        boss.step(0.6); // advance past the 0.5s cooldown
        assert!(boss.try_take_ram_damage(50.0), "ram should land again after cooldown expires");
        assert_eq!(boss.health, 900.0);
    }

    #[test]
    fn defeated_boss_takes_no_further_ram_damage() {
        let mut boss = sample_boss();
        boss.apply_damage(boss.max_health);
        assert_eq!(boss.phase(), BossPhase::Defeated);
        assert!(!boss.try_take_ram_damage(50.0), "defeated boss shouldn't take more damage");
    }
}
