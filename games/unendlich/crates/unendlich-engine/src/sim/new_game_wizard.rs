//! `sim::new_game_wizard` — the backend for the "New World" wizard the fullgame
//! menu runs *before* any world is generated. It is a small stepped state machine
//! over [`new_world_config::NewWorldConfig`] (name → mode → seed → confirm) that
//! validates each step and, only on confirmation, resolves a [`WorldSpec`] the bin
//! then hands to worldgen. Pure: no IO/clock/RNG — the bin supplies entropy for a
//! Random seed and the catalog for the duplicate-name check.

use crate::sim::new_world_config::{
    to_spec, valid_world_name, NewWorldConfig, SeedChoice, WorldSpec,
};
use crate::sim::command_parse::GameMode;
use crate::sim::world_catalog::WorldCatalog;

/// The ordered steps of the wizard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WizardStep {
    Name,
    Mode,
    Seed,
    Confirm,
}

impl WizardStep {
    pub const ORDER: [WizardStep; 4] =
        [WizardStep::Name, WizardStep::Mode, WizardStep::Seed, WizardStep::Confirm];

    pub fn index(self) -> u8 {
        match self {
            WizardStep::Name => 0,
            WizardStep::Mode => 1,
            WizardStep::Seed => 2,
            WizardStep::Confirm => 3,
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            WizardStep::Name => "Name your world",
            WizardStep::Mode => "Choose a mode",
            WizardStep::Seed => "World seed",
            WizardStep::Confirm => "Ready to create",
        }
    }

    fn next(self) -> WizardStep {
        match self {
            WizardStep::Name => WizardStep::Mode,
            WizardStep::Mode => WizardStep::Seed,
            WizardStep::Seed => WizardStep::Confirm,
            WizardStep::Confirm => WizardStep::Confirm,
        }
    }

    fn prev(self) -> WizardStep {
        match self {
            WizardStep::Name => WizardStep::Name,
            WizardStep::Mode => WizardStep::Name,
            WizardStep::Seed => WizardStep::Mode,
            WizardStep::Confirm => WizardStep::Seed,
        }
    }
}

/// The wizard: the step cursor + the config being assembled.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewGameWizard {
    step: WizardStep,
    cfg: NewWorldConfig,
}

impl Default for NewGameWizard {
    fn default() -> Self {
        NewGameWizard {
            step: WizardStep::Name,
            cfg: NewWorldConfig {
                name: "New World".to_string(),
                seed: SeedChoice::Random,
                mode: GameMode::Survival,
            },
        }
    }
}

impl NewGameWizard {
    pub fn new() -> NewGameWizard {
        NewGameWizard::default()
    }

    pub fn step(&self) -> WizardStep {
        self.step
    }
    pub fn config(&self) -> &NewWorldConfig {
        &self.cfg
    }

    // ── per-step edits (the UI calls these as the player types/picks) ──
    pub fn set_name(&mut self, name: &str) {
        self.cfg.name = name.to_string();
    }
    pub fn set_mode(&mut self, mode: GameMode) {
        self.cfg.mode = mode;
    }
    pub fn set_seed(&mut self, seed: SeedChoice) {
        self.cfg.seed = seed;
    }

    /// Validation error for the CURRENT step, if any (drives a disabled "Next").
    pub fn step_error(&self) -> Option<&'static str> {
        match self.step {
            WizardStep::Name => valid_world_name(&self.cfg.name).err(),
            // Mode + Seed always have a valid value (defaults + closed choices).
            WizardStep::Mode | WizardStep::Seed => None,
            // Confirm re-checks the whole config (the name is the only constraint).
            WizardStep::Confirm => valid_world_name(&self.cfg.name).err(),
        }
    }

    /// Can the wizard advance from the current step?
    pub fn can_advance(&self) -> bool {
        self.step != WizardStep::Confirm && self.step_error().is_none()
    }

    pub fn next(&mut self) -> bool {
        if self.can_advance() {
            self.step = self.step.next();
            true
        } else {
            false
        }
    }

    pub fn back(&mut self) -> bool {
        if self.step == WizardStep::Name {
            return false;
        }
        self.step = self.step.prev();
        true
    }

    pub fn at_confirm(&self) -> bool {
        self.step == WizardStep::Confirm
    }

    /// Can we finish? Valid config AND the name isn't already taken in `catalog`.
    /// The catalog check needs external state, so it lives here (not in validate()).
    pub fn can_finish(&self, _catalog: &WorldCatalog) -> Result<(), &'static str> {
        // r291: a world's identity is its ID, not its name — the name is just a
        // display title, so duplicate titles are allowed (two worlds can both be
        // "New World"). Only the name's *validity* is checked here; uniqueness is
        // no longer required (this was the silent Create dead-end: the catalog's
        // default-named entry collided with a fresh default-named world).
        valid_world_name(&self.cfg.name)?;
        Ok(())
    }

    /// Resolve the finished wizard into a [`WorldSpec`]. `entropy` feeds a Random
    /// seed (the bin passes a frame/time value, keeping this pure). Only valid at
    /// (or before) Confirm with a unique, legal name. The caller loads the world
    /// AFTER this returns — the wizard never touches worldgen itself.
    pub fn finish(&self, entropy: u64, catalog: &WorldCatalog) -> Result<WorldSpec, &'static str> {
        self.can_finish(catalog)?;
        to_spec(&self.cfg, entropy)
    }

    /// A human-readable summary for the Confirm screen.
    pub fn summary(&self) -> String {
        let mode = match self.cfg.mode {
            GameMode::Creative => "Creative",
            GameMode::Survival => "Survival",
            GameMode::Adventure => "Adventure",
            GameMode::Spectator => "Spectator",
        };
        let seed = match &self.cfg.seed {
            SeedChoice::Random => "random".to_string(),
            SeedChoice::Fixed(s) => format!("seed {s}"),
            SeedChoice::Text(t) => format!("seed \"{}\"", t.trim()),
        };
        format!("{} · {} · {}", self.cfg.name.trim(), mode, seed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_at_name_with_a_valid_default() {
        let w = NewGameWizard::new();
        assert_eq!(w.step(), WizardStep::Name);
        assert!(w.step_error().is_none(), "default name is valid");
        assert!(w.can_advance());
    }

    #[test]
    fn invalid_name_blocks_advance() {
        let mut w = NewGameWizard::new();
        w.set_name("");
        assert!(w.step_error().is_some());
        assert!(!w.can_advance());
        assert!(!w.next(), "cannot advance past an invalid name");
        assert_eq!(w.step(), WizardStep::Name);
    }

    #[test]
    fn walks_name_to_confirm_and_back() {
        let mut w = NewGameWizard::new();
        w.set_name("Vale");
        assert!(w.next() && w.step() == WizardStep::Mode);
        w.set_mode(GameMode::Creative);
        assert!(w.next() && w.step() == WizardStep::Seed);
        w.set_seed(SeedChoice::Text("greenheart".into()));
        assert!(w.next() && w.step() == WizardStep::Confirm);
        assert!(w.at_confirm());
        assert!(!w.next(), "Confirm is terminal");
        // back unwinds step by step
        assert!(w.back() && w.step() == WizardStep::Seed);
        assert!(w.back() && w.step() == WizardStep::Mode);
        assert!(w.back() && w.step() == WizardStep::Name);
        assert!(!w.back(), "Name is the first step");
    }

    #[test]
    fn finish_resolves_a_spec_and_respects_seed_choice() {
        let cat = WorldCatalog::new();
        let mut w = NewGameWizard::new();
        w.set_name("Deeps");
        w.set_mode(GameMode::Creative);

        // Random uses the supplied entropy
        w.set_seed(SeedChoice::Random);
        let spec = w.finish(0xABCD, &cat).expect("random spec");
        assert_eq!(spec.name, "Deeps");
        assert_eq!(spec.seed, 0xABCD);
        assert_eq!(spec.mode, GameMode::Creative);

        // Fixed passes through; entropy ignored
        w.set_seed(SeedChoice::Fixed(42));
        assert_eq!(w.finish(999, &cat).unwrap().seed, 42);

        // Text is a deterministic hash, independent of entropy
        w.set_seed(SeedChoice::Text("oak".into()));
        let a = w.finish(1, &cat).unwrap().seed;
        let b = w.finish(2, &cat).unwrap().seed;
        assert_eq!(a, b, "text seed is deterministic");
    }

    #[test]
    fn duplicate_name_blocks_finish() {
        use crate::sim::world_catalog::WorldEntry;
        let mut cat = WorldCatalog::new();
        cat.add(WorldEntry {
            name: "Vale".into(),
            seed: 1,
            mode: crate::sim::world_catalog::GameMode::Survival,
            created_tick: 0,
            last_played_tick: 0,
        });
        let mut w = NewGameWizard::new();
        w.set_name("Vale");
        assert!(w.can_finish(&cat).is_err(), "name already taken");
        assert!(w.finish(7, &cat).is_err());
        // a unique name finishes
        w.set_name("Vale Two");
        assert!(w.can_finish(&cat).is_ok());
        assert!(w.finish(7, &cat).is_ok());
    }

    #[test]
    fn summary_reads_cleanly() {
        let mut w = NewGameWizard::new();
        w.set_name("  Vale  ");
        w.set_mode(GameMode::Creative);
        w.set_seed(SeedChoice::Fixed(7));
        let s = w.summary();
        assert!(s.contains("Vale") && s.contains("Creative") && s.contains("7"));
    }
}
