//! `sim::gamemode_traits` — gamemodes as named sets of atomic capability traits.
//!
//! From `07_Flora_and_World/gamemode_trait_system.docx`. The architecture
//! decouples *what a gamemode is called* from *what it does*: every behaviour
//! that differs between modes is the consequence of one boolean trait being on or
//! off — there are **no hidden mode-specific code branches**. New modes are just
//! trait combinations; traits can be toggled individually at runtime; and trait
//! interaction rules are documented once per trait, not once per mode.
//!
//! ## Traits
//! | Trait          | Category   | Effect                                             |
//! |----------------|------------|----------------------------------------------------|
//! | BUILD          | World      | Can place and break blocks                         |
//! | HEALTH         | Survival   | Has a mutable health pool; can be damaged/killed   |
//! | HUNGER         | Survival   | Has a mutable hunger pool; depletes over time      |
//! | MOBTARGET      | AI         | Hostile mobs passively detect and attack           |
//! | FLY            | Movement   | Can toggle sustained flight                        |
//! | IMMUNE         | Survival   | Immune to all damage/knockback/negative status     |
//! | NOTARGET       | AI         | Mobs cannot acquire this player, even if provoked  |
//! | INVISTOPLAYERS | Visibility | Invisible to other players' clients                |
//! | PASSTHROUGH    | Movement   | Moves through solid blocks; no collision           |
//! | VISION         | Perception | Full brightness, fog suppression, entity X-ray     |
//!
//! ## Mode–Trait Matrix (`●` on / `○` off)
//! ```text
//! Trait        SURVIVAL ADVENTURE CREATIVE CRASH-DUMMY
//! BUILD           ●        ○         ●         ●
//! HEALTH          ●        ●         ○         ○
//! HUNGER          ●        ●         ○         ○
//! MOBTARGET       ●        ●         ○         ●
//! FLY             ○        ○         ●         ●
//! IMMUNE          ○        ○         ●         ●
//! NOTARGET        ○        ○         ●         ○
//! ```
//! INVISTOPLAYERS / PASSTHROUGH / VISION are not in any base mode — they're
//! administrative/developmental overlays applied at runtime on top of a mode.

/// One atomic capability trait. The discriminant is the bit index in [`TraitSet`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u16)]
pub enum Trait {
    Build = 0,
    Health = 1,
    Hunger = 2,
    MobTarget = 3,
    Fly = 4,
    Immune = 5,
    NoTarget = 6,
    InvisToPlayers = 7,
    PassThrough = 8,
    Vision = 9,
}

impl Trait {
    pub const ALL: [Trait; 10] = [
        Trait::Build, Trait::Health, Trait::Hunger, Trait::MobTarget, Trait::Fly,
        Trait::Immune, Trait::NoTarget, Trait::InvisToPlayers, Trait::PassThrough, Trait::Vision,
    ];
    #[inline]
    const fn bit(self) -> u16 {
        1u16 << (self as u16)
    }
    /// True for the three administrative/dev overlay traits (not in any base mode).
    pub const fn is_overlay(self) -> bool {
        matches!(self, Trait::InvisToPlayers | Trait::PassThrough | Trait::Vision)
    }
    pub const fn name(self) -> &'static str {
        match self {
            Trait::Build => "BUILD",
            Trait::Health => "HEALTH",
            Trait::Hunger => "HUNGER",
            Trait::MobTarget => "MOBTARGET",
            Trait::Fly => "FLY",
            Trait::Immune => "IMMUNE",
            Trait::NoTarget => "NOTARGET",
            Trait::InvisToPlayers => "INVISTOPLAYERS",
            Trait::PassThrough => "PASSTHROUGH",
            Trait::Vision => "VISION",
        }
    }
}

/// A set of active traits (bitset). Modes and runtime overlays both produce these.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct TraitSet(u16);

impl TraitSet {
    pub const fn empty() -> Self {
        TraitSet(0)
    }
    /// Build from a slice of traits (const-friendly).
    pub const fn of(ts: &[Trait]) -> Self {
        let mut bits = 0u16;
        let mut i = 0;
        while i < ts.len() {
            bits |= ts[i].bit();
            i += 1;
        }
        TraitSet(bits)
    }
    #[inline]
    pub const fn has(self, t: Trait) -> bool {
        self.0 & t.bit() != 0
    }
    #[inline]
    pub const fn with(self, t: Trait) -> Self {
        TraitSet(self.0 | t.bit())
    }
    #[inline]
    pub const fn without(self, t: Trait) -> Self {
        TraitSet(self.0 & !t.bit())
    }
    #[inline]
    pub const fn set(self, t: Trait, on: bool) -> Self {
        if on { self.with(t) } else { self.without(t) }
    }
    pub const fn raw(self) -> u16 {
        self.0
    }

    // ── derived capability semantics (trait interaction rules live HERE, once) ──

    /// Can place/break blocks.
    pub const fn can_build(self) -> bool {
        self.has(Trait::Build)
    }
    /// Actually takes damage: HEALTH present AND not IMMUNE. (IMMUNE cancels damage
    /// even for a health-bearing player — the Crash Dummy case.)
    pub const fn takes_damage(self) -> bool {
        self.has(Trait::Health) && !self.has(Trait::Immune)
    }
    /// Hunger actually depletes: HUNGER present AND not IMMUNE.
    pub const fn depletes_hunger(self) -> bool {
        self.has(Trait::Hunger) && !self.has(Trait::Immune)
    }
    /// Hostile mobs will target: MOBTARGET present AND not NOTARGET. (NOTARGET wins
    /// even against provocation.)
    pub const fn mobs_target(self) -> bool {
        self.has(Trait::MobTarget) && !self.has(Trait::NoTarget)
    }
    /// Can toggle sustained flight.
    pub const fn can_fly(self) -> bool {
        self.has(Trait::Fly)
    }
    /// No block collision (noclip).
    pub const fn no_collision(self) -> bool {
        self.has(Trait::PassThrough)
    }
    /// Full-bright / fog-suppressed / X-ray perception overlay.
    pub const fn full_vision(self) -> bool {
        self.has(Trait::Vision)
    }
    /// Hidden from other players' clients (still visible server-side).
    pub const fn invisible_to_players(self) -> bool {
        self.has(Trait::InvisToPlayers)
    }
    /// Inside the survival system at all (has health or hunger).
    pub const fn in_survival_system(self) -> bool {
        self.has(Trait::Health) || self.has(Trait::Hunger)
    }
}

/// The four base gamemodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Mode {
    Survival,
    Adventure,
    Creative,
    CrashDummy,
}

impl Mode {
    /// The base trait column for this mode (before any runtime overlays).
    pub const fn traits(self) -> TraitSet {
        match self {
            // Fully consequential: build, health, hunger, hostile world.
            Mode::Survival => TraitSet::of(&[
                Trait::Build, Trait::Health, Trait::Hunger, Trait::MobTarget,
            ]),
            // Guided experience: survival needs + hostile world, but building locked.
            Mode::Adventure => TraitSet::of(&[Trait::Health, Trait::Hunger, Trait::MobTarget]),
            // Uninhibited construction: outside survival entirely; fly + immune + ignored.
            Mode::Creative => {
                TraitSet::of(&[Trait::Build, Trait::Fly, Trait::Immune, Trait::NoTarget])
            }
            // Stress-test: immune + fly + build, but mobs DO target (damage cancelled).
            Mode::CrashDummy => {
                TraitSet::of(&[Trait::Build, Trait::MobTarget, Trait::Fly, Trait::Immune])
            }
        }
    }
    pub const fn name(self) -> &'static str {
        match self {
            Mode::Survival => "SURVIVAL",
            Mode::Adventure => "ADVENTURE",
            Mode::Creative => "CREATIVE",
            Mode::CrashDummy => "CRASH DUMMY",
        }
    }
}

/// A player's live capability = base mode traits + runtime overlay traits. The
/// overlays (INVISTOPLAYERS / PASSTHROUGH / VISION, or admin-forced others) are
/// layered on top, per the doc's "applied at runtime via command or permission".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capability {
    pub mode: Mode,
    pub overlay: TraitSet,
}

impl Capability {
    pub const fn new(mode: Mode) -> Self {
        Capability { mode, overlay: TraitSet::empty() }
    }
    /// Effective trait set: mode column OR overlay bits.
    pub const fn effective(self) -> TraitSet {
        TraitSet(self.mode.traits().raw() | self.overlay.raw())
    }
    /// Toggle a runtime overlay trait on/off (does not mutate the base mode).
    pub fn set_overlay(&mut self, t: Trait, on: bool) {
        self.overlay = self.overlay.set(t, on);
    }
    /// Switch mode, preserving any runtime overlays.
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matrix_matches_doc() {
        let s = Mode::Survival.traits();
        assert!(s.has(Trait::Build) && s.has(Trait::Health) && s.has(Trait::Hunger) && s.has(Trait::MobTarget));
        assert!(!s.has(Trait::Fly) && !s.has(Trait::Immune) && !s.has(Trait::NoTarget));

        let a = Mode::Adventure.traits();
        assert!(!a.has(Trait::Build)); // building locked in adventure
        assert!(a.has(Trait::Health) && a.has(Trait::Hunger) && a.has(Trait::MobTarget));

        let c = Mode::Creative.traits();
        assert!(c.has(Trait::Build) && c.has(Trait::Fly) && c.has(Trait::Immune) && c.has(Trait::NoTarget));
        assert!(!c.has(Trait::Health) && !c.has(Trait::Hunger) && !c.has(Trait::MobTarget));

        let d = Mode::CrashDummy.traits();
        assert!(d.has(Trait::Build) && d.has(Trait::MobTarget) && d.has(Trait::Fly) && d.has(Trait::Immune));
        assert!(!d.has(Trait::NoTarget) && !d.has(Trait::Health));

        // Overlay traits are in NO base mode.
        for m in [Mode::Survival, Mode::Adventure, Mode::Creative, Mode::CrashDummy] {
            let t = m.traits();
            assert!(!t.has(Trait::InvisToPlayers) && !t.has(Trait::PassThrough) && !t.has(Trait::Vision));
        }
    }

    #[test]
    fn interaction_rules() {
        // Survival takes damage + depletes hunger + is targeted.
        let s = Mode::Survival.traits();
        assert!(s.takes_damage() && s.depletes_hunger() && s.mobs_target());
        assert!(s.in_survival_system());

        // Creative: outside survival, immune, ignored by mobs.
        let c = Mode::Creative.traits();
        assert!(!c.takes_damage() && !c.mobs_target() && !c.in_survival_system());
        assert!(c.can_build() && c.can_fly());

        // Crash Dummy: mobs DO target it (MOBTARGET, no NOTARGET), but IMMUNE cancels
        // the damage — targeted yet unkillable.
        let d = Mode::CrashDummy.traits();
        assert!(d.mobs_target(), "crash dummy is targeted by mobs");
        assert!(!d.takes_damage(), "but immune cancels the damage");
    }

    #[test]
    fn overlays_layer_on_top() {
        let mut cap = Capability::new(Mode::Survival);
        assert!(!cap.effective().no_collision() && !cap.effective().full_vision());
        // Admin grants noclip + X-ray vision on top of survival.
        cap.set_overlay(Trait::PassThrough, true);
        cap.set_overlay(Trait::Vision, true);
        assert!(cap.effective().no_collision() && cap.effective().full_vision());
        // Base survival semantics still hold underneath.
        assert!(cap.effective().takes_damage() && cap.effective().can_build());
        // Switching mode preserves overlays.
        cap.set_mode(Mode::Creative);
        assert!(cap.effective().no_collision() && cap.effective().can_fly());
    }

    #[test]
    fn trait_bits_unique() {
        let mut seen = 0u16;
        for t in Trait::ALL {
            assert_eq!(seen & t.bit(), 0, "duplicate bit for {:?}", t);
            seen |= t.bit();
        }
    }
}
