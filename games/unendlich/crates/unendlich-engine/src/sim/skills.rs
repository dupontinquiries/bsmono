//! Skill allocation — an inventory of RPG skills the player pours experience into.
//! Designed to *slightly* elevate and *distinguish*, not to power-creep:
//!
//! - **Slight**: each level grants a small bonus with diminishing returns, capped.
//!   Maxing a skill is +15% total (increments 5,4,3,2,1) — meaningful, not absurd.
//! - **Distinguishing**: level cost escalates (triangular), so a normal XP budget
//!   can max only a skill or two. You can't be good at everything, so allocation
//!   becomes identity. `respec` lets players re-explore builds.
//!
//! Each skill scales a lever in a system already built: Might → attack magnitude
//! (combat), Attunement → affliction intensity, Warding → shield integrity, Vigor
//! → max health, Fleetness → move speed, Artisan → crafting yield.
//!
//! Deterministic fixed-point — rollback-safe.

use crate::sim::fixed::Fixed;

/// Highest level any skill can reach.
pub const MAX_LEVEL: u8 = 5;
/// Base XP for the first level; each further level costs a multiple of this.
pub const BASE_COST: u64 = 100;
/// Per-level bonus increments, in percent. Diminishing on purpose — total +15%.
const INCREMENTS: [u16; MAX_LEVEL as usize] = [5, 4, 3, 2, 1];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Skill {
    Might,
    Attunement,
    Warding,
    Vigor,
    Fleetness,
    Artisan,
}

impl Skill {
    pub const ALL: [Skill; 6] = [
        Skill::Might,
        Skill::Attunement,
        Skill::Warding,
        Skill::Vigor,
        Skill::Fleetness,
        Skill::Artisan,
    ];

    fn idx(self) -> usize {
        match self {
            Skill::Might => 0,
            Skill::Attunement => 1,
            Skill::Warding => 2,
            Skill::Vigor => 3,
            Skill::Fleetness => 4,
            Skill::Artisan => 5,
        }
    }
}

/// A player's invested skills and unspent experience.
#[derive(Debug, Clone)]
pub struct SkillBook {
    levels: [u8; 6],
    xp: u64,
    spent: u64,
}

impl Default for SkillBook {
    fn default() -> Self {
        Self { levels: [0; 6], xp: 0, spent: 0 }
    }
}

impl SkillBook {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn gain_xp(&mut self, amount: u64) {
        self.xp = self.xp.saturating_add(amount);
    }

    pub fn xp(&self) -> u64 {
        self.xp
    }

    pub fn level(&self, s: Skill) -> u8 {
        self.levels[s.idx()]
    }

    pub fn total_levels(&self) -> u32 {
        self.levels.iter().map(|&l| l as u32).sum()
    }

    /// XP to raise `s` one level — escalating (triangular), `None` if maxed.
    pub fn cost_for_next(&self, s: Skill) -> Option<u64> {
        let lvl = self.levels[s.idx()];
        if lvl >= MAX_LEVEL {
            None
        } else {
            Some(BASE_COST * (lvl as u64 + 1))
        }
    }

    pub fn can_invest(&self, s: Skill) -> bool {
        matches!(self.cost_for_next(s), Some(c) if self.xp >= c)
    }

    /// Spend XP to raise `s` by one level. Returns false if maxed or too poor.
    pub fn invest(&mut self, s: Skill) -> bool {
        match self.cost_for_next(s) {
            Some(c) if self.xp >= c => {
                self.xp -= c;
                self.spent += c;
                self.levels[s.idx()] += 1;
                true
            }
            _ => false,
        }
    }

    /// The skill's bonus in percent (0 at level 0, 15 at max).
    pub fn bonus_percent(&self, s: Skill) -> u16 {
        let lvl = self.levels[s.idx()] as usize;
        INCREMENTS.iter().take(lvl).sum()
    }

    /// The skill's effect as a multiplier (1.0 .. 1.15).
    pub fn bonus(&self, s: Skill) -> Fixed {
        Fixed::ONE + Fixed::from_int(self.bonus_percent(s) as i64) / Fixed::from_int(100)
    }

    /// Apply a skill's multiplier to a base value. Exact integer math (no
    /// fixed-point drift): `base * (100 + pct) / 100`.
    pub fn scaled(&self, s: Skill, base: u16) -> u16 {
        let pct = self.bonus_percent(s) as u64;
        ((base as u64 * (100 + pct)) / 100).min(u16::MAX as u64) as u16
    }

    /// Refund every invested point back into the XP pool and reset to level 0.
    pub fn respec(&mut self) {
        self.xp += self.spent;
        self.spent = 0;
        self.levels = [0; 6];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn investing_raises_a_level_and_spends_xp() {
        let mut b = SkillBook::new();
        b.gain_xp(100);
        assert!(b.invest(Skill::Might));
        assert_eq!(b.level(Skill::Might), 1);
        assert_eq!(b.xp(), 0, "spent the 100");
    }

    #[test]
    fn cannot_invest_without_enough_xp() {
        let mut b = SkillBook::new();
        b.gain_xp(99);
        assert!(!b.can_invest(Skill::Might));
        assert!(!b.invest(Skill::Might));
        assert_eq!(b.level(Skill::Might), 0);
    }

    #[test]
    fn level_cost_escalates() {
        let mut b = SkillBook::new();
        assert_eq!(b.cost_for_next(Skill::Vigor), Some(100));
        b.gain_xp(10_000);
        b.invest(Skill::Vigor);
        assert_eq!(b.cost_for_next(Skill::Vigor), Some(200));
        b.invest(Skill::Vigor);
        assert_eq!(b.cost_for_next(Skill::Vigor), Some(300));
    }

    #[test]
    fn a_skill_caps_at_max_level() {
        let mut b = SkillBook::new();
        b.gain_xp(100_000);
        for _ in 0..10 {
            b.invest(Skill::Warding);
        }
        assert_eq!(b.level(Skill::Warding), MAX_LEVEL);
        assert_eq!(b.cost_for_next(Skill::Warding), None, "maxed → no further cost");
        assert!(!b.invest(Skill::Warding));
    }

    #[test]
    fn the_bonus_is_modest_and_diminishing() {
        let mut b = SkillBook::new();
        b.gain_xp(100_000);
        let mut prev_delta = u16::MAX;
        let mut last = 0u16;
        for _ in 0..MAX_LEVEL {
            b.invest(Skill::Might);
            let now = b.bonus_percent(Skill::Might);
            let delta = now - last;
            assert!(delta <= prev_delta, "increments diminish: {delta} <= {prev_delta}");
            prev_delta = delta;
            last = now;
        }
        assert_eq!(b.bonus_percent(Skill::Might), 15, "maxed is only +15% — slight, not absurd");
    }

    #[test]
    fn scaled_applies_a_slight_multiplier() {
        let mut b = SkillBook::new();
        assert_eq!(b.scaled(Skill::Might, 100), 100, "no levels → no change");
        b.gain_xp(100_000);
        for _ in 0..MAX_LEVEL {
            b.invest(Skill::Might);
        }
        assert_eq!(b.scaled(Skill::Might, 100), 115, "maxed Might: 100 -> 115");
    }

    #[test]
    fn a_normal_budget_forces_specialization() {
        // 1500 XP = exactly the cost to max one skill (100+200+300+400+500).
        let mut b = SkillBook::new();
        b.gain_xp(1500);
        for _ in 0..MAX_LEVEL {
            assert!(b.invest(Skill::Might));
        }
        assert_eq!(b.level(Skill::Might), MAX_LEVEL);
        assert_eq!(b.xp(), 0);
        // Nothing left for a second skill — you had to choose who you are.
        assert!(!b.can_invest(Skill::Attunement));
        // Maxing all six would cost 9000 — far beyond a 1500 budget.
    }

    #[test]
    fn respec_refunds_everything() {
        let mut b = SkillBook::new();
        b.gain_xp(1500);
        for _ in 0..MAX_LEVEL {
            b.invest(Skill::Fleetness);
        }
        assert_eq!(b.xp(), 0);
        b.respec();
        assert_eq!(b.xp(), 1500, "all invested XP refunded");
        assert_eq!(b.total_levels(), 0, "levels reset for a fresh build");
    }
}
