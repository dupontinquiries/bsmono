//! sim::dragon_energy — the Dragon Energy pool.
//!
//! From `design_library/04/dragon_energy_lifestyle`: a dragon runs on a
//! physiological energy reservoir *distinct from health*. It governs bending
//! output, sustained flight, and how the dragon behaves — a wounded dragon with
//! full energy is still formidable; a healthy one that's depleted can't fly or
//! bend hard. Energy drains with flight/bending/construction and refills by
//! feeding. This module is that pool and the capability gates its six named
//! states imply. Existing `hunger_drive`/`feeding`/`stamina` are different
//! concepts (a drive signal, food selection, short burst) — this is the
//! long-horizon caloric reservoir. Pure, deterministic, std-only.

/// The six energy bands (fraction of max), highest to lowest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EnergyState {
    /// > 85%: full bending, unlimited flight, peak aggression.
    Peak,
    /// 60–85%: normal operation.
    Strong,
    /// 35–60%: foraging begins, flight range −30%, bending 80%.
    Moderate,
    /// 15–35%: slower, flight in bursts, bending 50%, risk-averse.
    Low,
    /// 5–15%: urgent food-seeking, short hops, bending minimal.
    Depleted,
    /// < 5%: cannot fly, desperate — attacks anything of caloric value.
    Critical,
}

/// A dragon's energy reservoir.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DragonEnergy {
    current: f32,
    max: f32,
}

impl DragonEnergy {
    /// A reservoir of capacity `max`, starting full.
    pub fn full(max: f32) -> DragonEnergy {
        let m = max.max(1e-3);
        DragonEnergy { current: m, max: m }
    }

    /// A reservoir at an explicit starting `current` (clamped to `[0, max]`).
    pub fn new(current: f32, max: f32) -> DragonEnergy {
        let m = max.max(1e-3);
        DragonEnergy { current: current.clamp(0.0, m), max: m }
    }

    pub fn current(&self) -> f32 {
        self.current
    }
    pub fn max(&self) -> f32 {
        self.max
    }

    /// Energy as a fraction of capacity, `[0, 1]`.
    pub fn fraction(&self) -> f32 {
        (self.current / self.max).clamp(0.0, 1.0)
    }

    /// The current band.
    pub fn state(&self) -> EnergyState {
        let f = self.fraction();
        if f > 0.85 {
            EnergyState::Peak
        } else if f >= 0.60 {
            EnergyState::Strong
        } else if f >= 0.35 {
            EnergyState::Moderate
        } else if f >= 0.15 {
            EnergyState::Low
        } else if f >= 0.05 {
            EnergyState::Depleted
        } else {
            EnergyState::Critical
        }
    }

    /// Bending output multiplier `[0, 1]` for the current state.
    pub fn bending_output(&self) -> f32 {
        match self.state() {
            EnergyState::Peak | EnergyState::Strong => 1.0,
            EnergyState::Moderate => 0.8,
            EnergyState::Low => 0.5,
            EnergyState::Depleted => 0.15,
            EnergyState::Critical => 0.1,
        }
    }

    /// Flight-range multiplier `[0, 1]`. `0` at Critical — it cannot fly.
    pub fn flight_range_scale(&self) -> f32 {
        match self.state() {
            EnergyState::Peak | EnergyState::Strong => 1.0,
            EnergyState::Moderate => 0.7, // −30%
            EnergyState::Low => 0.3,      // bursts, not soaring
            EnergyState::Depleted => 0.1, // short hops
            EnergyState::Critical => 0.0, // grounded
        }
    }

    /// Can the dragon fly at all? (False only at Critical.)
    pub fn can_fly(&self) -> bool {
        self.state() != EnergyState::Critical
    }

    /// Has foraging behaviour kicked in? (Moderate and below.)
    pub fn seeks_food(&self) -> bool {
        self.state() >= EnergyState::Moderate // enum order: Moderate..Critical
    }

    /// Will it commit to a protracted fight? Risk-averse at Low and below.
    pub fn will_fight_prolonged(&self) -> bool {
        self.state() <= EnergyState::Moderate // Peak/Strong/Moderate yes; Low+ disengage
    }

    /// Critical desperation: attacks anything of caloric value, armoured players
    /// included, and may discharge bending involuntarily.
    pub fn is_desperate(&self) -> bool {
        self.state() == EnergyState::Critical
    }

    /// Spend up to `cost` energy; returns the amount actually spent (capped by
    /// what's left). Never goes negative.
    pub fn expend(&mut self, cost: f32) -> f32 {
        let c = cost.max(0.0).min(self.current);
        self.current -= c;
        c
    }

    /// Can the dragon afford `cost` outright?
    pub fn can_afford(&self, cost: f32) -> bool {
        self.current >= cost.max(0.0)
    }

    /// Recover up to `amount` from feeding; returns the amount actually gained
    /// (capped by remaining capacity). Never exceeds `max`.
    pub fn feed(&mut self, amount: f32) -> f32 {
        let room = self.max - self.current;
        let g = amount.max(0.0).min(room);
        self.current += g;
        g
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(frac: f32) -> DragonEnergy {
        DragonEnergy::new(frac, 1.0)
    }

    #[test]
    fn thresholds_map_to_the_six_states() {
        assert_eq!(at(1.0).state(), EnergyState::Peak);
        assert_eq!(at(0.90).state(), EnergyState::Peak);
        assert_eq!(at(0.85).state(), EnergyState::Strong, "0.85 is the top of Strong");
        assert_eq!(at(0.60).state(), EnergyState::Strong);
        assert_eq!(at(0.59).state(), EnergyState::Moderate);
        assert_eq!(at(0.35).state(), EnergyState::Moderate);
        assert_eq!(at(0.34).state(), EnergyState::Low);
        assert_eq!(at(0.15).state(), EnergyState::Low);
        assert_eq!(at(0.14).state(), EnergyState::Depleted);
        assert_eq!(at(0.05).state(), EnergyState::Depleted);
        assert_eq!(at(0.04).state(), EnergyState::Critical);
        assert_eq!(at(0.0).state(), EnergyState::Critical);
    }

    #[test]
    fn capability_gates_follow_the_state() {
        assert_eq!(at(0.95).bending_output(), 1.0);
        assert_eq!(at(0.50).bending_output(), 0.8);
        assert_eq!(at(0.20).bending_output(), 0.5);
        assert!(at(0.02).bending_output() < 0.2);
        assert_eq!(at(0.50).flight_range_scale(), 0.7, "Moderate = −30%");
        assert_eq!(at(0.02).flight_range_scale(), 0.0, "Critical grounded");
    }

    #[test]
    fn only_critical_cannot_fly() {
        assert!(at(0.10).can_fly(), "Depleted can still hop");
        assert!(!at(0.02).can_fly(), "Critical cannot fly");
    }

    #[test]
    fn foraging_starts_at_moderate() {
        assert!(!at(0.90).seeks_food());
        assert!(!at(0.70).seeks_food());
        assert!(at(0.50).seeks_food(), "Moderate forages");
        assert!(at(0.10).seeks_food());
    }

    #[test]
    fn risk_aversion_below_moderate() {
        assert!(at(0.90).will_fight_prolonged());
        assert!(at(0.50).will_fight_prolonged(), "Moderate still commits");
        assert!(!at(0.20).will_fight_prolonged(), "Low disengages");
        assert!(!at(0.02).will_fight_prolonged());
    }

    #[test]
    fn only_critical_is_desperate() {
        assert!(!at(0.10).is_desperate());
        assert!(at(0.02).is_desperate());
    }

    #[test]
    fn expend_drains_without_going_negative() {
        let mut e = DragonEnergy::new(30.0, 100.0);
        assert_eq!(e.expend(10.0), 10.0);
        assert_eq!(e.current(), 20.0);
        assert_eq!(e.expend(999.0), 20.0, "can only spend what's left");
        assert_eq!(e.current(), 0.0);
        assert!(e.is_desperate());
    }

    #[test]
    fn feeding_refills_without_overflow() {
        let mut e = DragonEnergy::new(10.0, 100.0);
        assert_eq!(e.feed(50.0), 50.0);
        assert_eq!(e.current(), 60.0);
        assert_eq!(e.feed(999.0), 40.0, "only tops up to max");
        assert_eq!(e.current(), 100.0);
        assert_eq!(e.state(), EnergyState::Peak);
    }

    #[test]
    fn can_afford_matches_the_reserve() {
        let e = DragonEnergy::new(25.0, 100.0);
        assert!(e.can_afford(25.0));
        assert!(!e.can_afford(25.1));
    }

    #[test]
    fn state_is_deterministic() {
        let e = at(0.42);
        assert_eq!(e.state(), e.state());
        assert_eq!(e.bending_output(), e.bending_output());
    }
}
