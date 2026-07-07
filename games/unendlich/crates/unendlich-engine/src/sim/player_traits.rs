//! sim::player_traits — the gamemode **trait** layer (gamemode_trait_system).
//!
//! The doc's architecture decouples *what a mode allows* from the mode itself:
//! capabilities are ten atomic **traits** a player either has or lacks, and a
//! gamemode is just a named bundle of them (the Mode-Trait Matrix §1.2). This is
//! that layer — a `TraitSet` bitset, the per-mode default bundles, and the
//! resolved capabilities after the interaction/precedence rules (§ interaction:
//! IMMUNE overrides HEALTH, NOTARGET overrides MOBTARGET, FLY suppresses fall
//! damage). It complements `sim::game_mode` (which exposes a coarser 5-flag caps
//! table over Survival/Creative/Adventure/Spectator); this uses the doc's own
//! four modes, including **Crash Dummy**, and its finer trait set lets scripted
//! transitions add a single trait (e.g. VISION onto Survival) without changing
//! mode. Pure, deterministic, std-only.

/// The ten capability traits (§1.1).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Trait {
    /// Place and break blocks.
    Build,
    /// Has a damageable health pool.
    Health,
    /// Has a depleting hunger pool.
    Hunger,
    /// Hostile mobs may target this player.
    MobTarget,
    /// Toggle sustained flight.
    Fly,
    /// Immune to all damage / knockback / negative effects.
    Immune,
    /// Mobs cannot acquire this player as a target.
    NoTarget,
    /// Invisible to other players' clients.
    InvisToPlayers,
    /// Move through solid blocks (noclip).
    PassThrough,
    /// Full brightness / fog suppression / see-through observation.
    Vision,
}

impl Trait {
    pub const ALL: [Trait; 10] = [
        Trait::Build,
        Trait::Health,
        Trait::Hunger,
        Trait::MobTarget,
        Trait::Fly,
        Trait::Immune,
        Trait::NoTarget,
        Trait::InvisToPlayers,
        Trait::PassThrough,
        Trait::Vision,
    ];
    fn bit(self) -> u16 {
        1u16 << (self as u16)
    }
}

/// A set of traits (bitset over the ten).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct TraitSet {
    bits: u16,
}

impl TraitSet {
    pub const fn empty() -> TraitSet {
        TraitSet { bits: 0 }
    }
    pub fn of(traits: &[Trait]) -> TraitSet {
        let mut s = TraitSet::empty();
        for &t in traits {
            s.insert(t);
        }
        s
    }
    pub fn has(self, t: Trait) -> bool {
        self.bits & t.bit() != 0
    }
    pub fn insert(&mut self, t: Trait) {
        self.bits |= t.bit();
    }
    pub fn remove(&mut self, t: Trait) {
        self.bits &= !t.bit();
    }
    pub fn with(mut self, t: Trait) -> TraitSet {
        self.insert(t);
        self
    }
    pub fn without(mut self, t: Trait) -> TraitSet {
        self.remove(t);
        self
    }
    pub fn union(self, other: TraitSet) -> TraitSet {
        TraitSet { bits: self.bits | other.bits }
    }
    pub fn count(self) -> u32 {
        self.bits.count_ones()
    }
    pub fn iter(self) -> impl Iterator<Item = Trait> {
        Trait::ALL.into_iter().filter(move |&t| self.has(t))
    }
}

/// The doc's four gamemodes (§2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GameplayMode {
    Survival,
    Adventure,
    Creative,
    CrashDummy,
}

impl GameplayMode {
    pub fn label(self) -> &'static str {
        match self {
            GameplayMode::Survival => "Survival",
            GameplayMode::Adventure => "Adventure",
            GameplayMode::Creative => "Creative",
            GameplayMode::CrashDummy => "Crash Dummy",
        }
    }
}

/// The Mode-Trait Matrix (§1.2): the default traits each mode grants.
pub fn traits_for(mode: GameplayMode) -> TraitSet {
    use Trait::*;
    match mode {
        // full survival: build, health, hunger, and mobs hunt you
        GameplayMode::Survival => TraitSet::of(&[Build, Health, Hunger, MobTarget]),
        // play-only: survival body, but no free building
        GameplayMode::Adventure => TraitSet::of(&[Health, Hunger, MobTarget]),
        // author mode: build, fly, immune, and mobs ignore you
        GameplayMode::Creative => TraitSet::of(&[Build, Fly, Immune, NoTarget]),
        // test dummy: build/fly/immune BUT mobs still target it (its defining trait)
        GameplayMode::CrashDummy => TraitSet::of(&[Build, Fly, Immune, MobTarget]),
    }
}

// ── Resolved capabilities (apply the interaction/precedence rules) ────────────

/// Does the player actually take damage? HEALTH present AND not IMMUNE
/// (IMMUNE overrides HEALTH — damage is cancelled before the HP check).
pub fn takes_damage(set: TraitSet) -> bool {
    set.has(Trait::Health) && !set.has(Trait::Immune)
}

/// Can hostile mobs target the player? MOBTARGET present AND not NOTARGET
/// (NOTARGET overrides MOBTARGET).
pub fn is_targetable(set: TraitSet) -> bool {
    set.has(Trait::MobTarget) && !set.has(Trait::NoTarget)
}

/// Does the player accumulate fall damage? Only if damageable AND not flying
/// (FLY suppresses fall damage regardless of HEALTH).
pub fn takes_fall_damage(set: TraitSet) -> bool {
    takes_damage(set) && !set.has(Trait::Fly)
}

/// Can the player place/break blocks?
pub fn can_build(set: TraitSet) -> bool {
    set.has(Trait::Build)
}

/// Can the player fly?
pub fn can_fly(set: TraitSet) -> bool {
    set.has(Trait::Fly)
}

/// Does the player clip through solid blocks?
pub fn noclip(set: TraitSet) -> bool {
    set.has(Trait::PassThrough)
}

/// Is the player's hunger pool active?
pub fn has_hunger(set: TraitSet) -> bool {
    set.has(Trait::Hunger) && !set.has(Trait::Immune)
}

#[cfg(test)]
mod tests {
    use super::*;
    use Trait::*;

    #[test]
    fn trait_set_ops() {
        let s = TraitSet::of(&[Build, Fly]);
        assert!(s.has(Build) && s.has(Fly));
        assert!(!s.has(Immune));
        assert_eq!(s.count(), 2);
        assert!(s.with(Immune).has(Immune));
        assert!(!s.without(Build).has(Build));
        assert_eq!(s.iter().count(), 2);
    }

    #[test]
    fn matrix_matches_the_doc() {
        // BUILD: ● ○ ● ●
        assert!(traits_for(GameplayMode::Survival).has(Build));
        assert!(!traits_for(GameplayMode::Adventure).has(Build));
        assert!(traits_for(GameplayMode::Creative).has(Build));
        assert!(traits_for(GameplayMode::CrashDummy).has(Build));
        // FLY / IMMUNE: ○ ○ ● ●
        for m in [GameplayMode::Creative, GameplayMode::CrashDummy] {
            assert!(traits_for(m).has(Fly) && traits_for(m).has(Immune));
        }
        for m in [GameplayMode::Survival, GameplayMode::Adventure] {
            assert!(!traits_for(m).has(Fly) && !traits_for(m).has(Immune));
        }
        // NOTARGET: ○ ○ ● ○  (Creative only)
        assert!(traits_for(GameplayMode::Creative).has(NoTarget));
        assert!(!traits_for(GameplayMode::CrashDummy).has(NoTarget));
    }

    #[test]
    fn crash_dummy_is_targeted_but_immune() {
        let cd = traits_for(GameplayMode::CrashDummy);
        assert!(is_targetable(cd), "its defining trait: mobs DO target it");
        assert!(!takes_damage(cd), "but IMMUNE cancels the damage");
    }

    #[test]
    fn immune_overrides_health() {
        let survivor = traits_for(GameplayMode::Survival);
        assert!(takes_damage(survivor));
        let armored = survivor.with(Immune);
        assert!(!takes_damage(armored), "IMMUNE wins over HEALTH");
    }

    #[test]
    fn notarget_overrides_mobtarget() {
        let s = TraitSet::of(&[MobTarget, NoTarget]);
        assert!(!is_targetable(s), "NOTARGET wins");
    }

    #[test]
    fn fly_suppresses_fall_damage() {
        let survivor = traits_for(GameplayMode::Survival);
        assert!(takes_fall_damage(survivor));
        assert!(!takes_fall_damage(survivor.with(Fly)), "flight suppresses fall damage");
    }

    #[test]
    fn creative_is_untouchable_noncorporeal_builder() {
        let c = traits_for(GameplayMode::Creative);
        assert!(can_build(c) && can_fly(c));
        assert!(!takes_damage(c) && !is_targetable(c) && !has_hunger(c));
    }

    #[test]
    fn additive_vision_trait_without_changing_mode() {
        // "Survival observer edge case": add VISION onto Survival.
        let observer = traits_for(GameplayMode::Survival).with(Vision);
        assert!(observer.has(Vision));
        assert!(takes_damage(observer), "still a normal survival body otherwise");
    }

    #[test]
    fn deterministic() {
        assert_eq!(traits_for(GameplayMode::CrashDummy), traits_for(GameplayMode::CrashDummy));
    }
}
