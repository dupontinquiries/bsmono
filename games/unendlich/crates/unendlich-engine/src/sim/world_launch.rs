//! `sim::world_launch` — the seam between the menu and the world loop. The menu
//! emits a [`MenuOutcome`](crate::sim::menu_controller::MenuOutcome); the bin needs
//! a concrete instruction with the *resolved generator inputs* (seed/mode/name) so
//! that the wizard's chosen seed actually drives world generation (the previously
//! deferred "world-gen-from-seed" piece). This module turns one into the other.
//! Pure: a total function over the outcome + the currently-loaded world.

use crate::sim::command_parse::GameMode;
use crate::sim::menu_controller::MenuOutcome;
use crate::sim::new_world_config::WorldSpec;
use crate::sim::world_catalog::{GameMode as CatMode, WorldCatalog};

/// The exact inputs the world generator needs to stand up a world. The bin reads
/// `seed` to seed the generator, `mode` to set rules, `name` for the save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorldInit {
    pub seed: u64,
    pub mode: GameMode,
    pub name: String,
    /// True when the world is being stood up for network hosting.
    pub multiplayer: bool,
}

impl WorldInit {
    /// Resolve generator inputs from a finished [`WorldSpec`] (seed already chosen).
    pub fn from_spec(spec: &WorldSpec, multiplayer: bool) -> WorldInit {
        WorldInit {
            seed: spec.seed,
            mode: spec.mode,
            name: spec.name.clone(),
            multiplayer,
        }
    }
}

/// What the bin's world loop should do next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchPlan {
    /// Stay in the menu (no commit).
    Stay,
    /// Generate + enter a fresh local world from these inputs.
    StartLocal(WorldInit),
    /// Load the saved world in this slot.
    LoadLocal(u8),
    /// Stand up the world for hosting, then announce it (see `session_mode::HostFlow`).
    Host(WorldInit),
    /// Connect to the server selected in the menu's browser (`session_mode::JoinFlow`).
    Join,
    /// Leave to desktop.
    Quit,
}

/// Map a menu outcome to a launch plan.
///
/// `current` is the spec of the world the player is already in (if any); it lets
/// `HostWorld` host the loaded world. A `HostWorld` with no current world falls
/// back to `Stay` (nothing to host yet) rather than inventing a world.
pub fn plan(outcome: &MenuOutcome, current: Option<&WorldSpec>) -> LaunchPlan {
    match outcome {
        MenuOutcome::None => LaunchPlan::Stay,
        MenuOutcome::StartWorld(spec) => LaunchPlan::StartLocal(WorldInit::from_spec(spec, false)),
        MenuOutcome::LoadWorld(slot) => LaunchPlan::LoadLocal(*slot),
        MenuOutcome::HostWorld => match current {
            Some(spec) => LaunchPlan::Host(WorldInit::from_spec(spec, true)),
            None => LaunchPlan::Stay,
        },
        MenuOutcome::JoinSelected => LaunchPlan::Join,
        MenuOutcome::QuitToDesktop => LaunchPlan::Quit,
    }
}

/// Map the catalog's 2-variant mode (what a saved world records) to the canonical
/// 4-variant rules mode the generator/world loop uses.
pub fn mode_from_catalog(m: CatMode) -> GameMode {
    match m {
        CatMode::Creative => GameMode::Creative,
        CatMode::Survival => GameMode::Survival,
    }
}

/// Resolve a `LaunchPlan::LoadLocal(slot)` into full generator inputs. A saved
/// world's recorded seed still drives base-terrain regeneration (saved chunk diffs
/// are applied on top), so the bin needs the seed even for a load — this completes
/// the load path the same way [`WorldInit::from_spec`] completes a new world.
/// Returns `None` if the slot is empty.
pub fn resolve_load(slot: u8, catalog: &WorldCatalog) -> Option<WorldInit> {
    catalog.entries().get(slot as usize).map(|e| WorldInit {
        seed: e.seed,
        mode: mode_from_catalog(e.mode),
        name: e.name.clone(),
        multiplayer: false,
    })
}

/// True when the plan requires generating a world locally (the bin must run the
/// generator before play can begin).
pub fn needs_generation(plan: &LaunchPlan) -> bool {
    matches!(plan, LaunchPlan::StartLocal(_) | LaunchPlan::Host(_))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::new_world_config::WorldSpec;

    fn spec(name: &str, seed: u64) -> WorldSpec {
        WorldSpec { name: name.into(), seed, mode: GameMode::Survival }
    }

    #[test]
    fn start_world_carries_the_resolved_seed_into_generation() {
        let s = spec("Vale", 0xABCD);
        let p = plan(&MenuOutcome::StartWorld(s.clone()), None);
        assert_eq!(
            p,
            LaunchPlan::StartLocal(WorldInit {
                seed: 0xABCD,
                mode: GameMode::Survival,
                name: "Vale".into(),
                multiplayer: false,
            })
        );
        assert!(needs_generation(&p));
    }

    #[test]
    fn load_maps_to_slot() {
        assert_eq!(plan(&MenuOutcome::LoadWorld(2), None), LaunchPlan::LoadLocal(2));
        assert!(!needs_generation(&LaunchPlan::LoadLocal(2)));
    }

    #[test]
    fn host_uses_the_current_world_and_flags_multiplayer() {
        let s = spec("Shared", 7);
        match plan(&MenuOutcome::HostWorld, Some(&s)) {
            LaunchPlan::Host(init) => {
                assert_eq!(init.seed, 7);
                assert!(init.multiplayer);
            }
            other => panic!("expected Host, got {other:?}"),
        }
    }

    #[test]
    fn host_without_a_world_stays_put() {
        assert_eq!(plan(&MenuOutcome::HostWorld, None), LaunchPlan::Stay);
    }

    #[test]
    fn join_and_quit_and_none() {
        assert_eq!(plan(&MenuOutcome::JoinSelected, None), LaunchPlan::Join);
        assert_eq!(plan(&MenuOutcome::QuitToDesktop, None), LaunchPlan::Quit);
        assert_eq!(plan(&MenuOutcome::None, None), LaunchPlan::Stay);
    }

    #[test]
    fn resolve_load_carries_the_saved_seed_and_maps_mode() {
        use crate::sim::world_catalog::{GameMode as CatMode, WorldCatalog, WorldEntry};
        let mut cat = WorldCatalog::new();
        cat.add(WorldEntry {
            name: "Old Save".into(),
            seed: 0x5EED,
            mode: CatMode::Creative,
            created_tick: 0,
            last_played_tick: 0,
        });
        let init = resolve_load(0, &cat).expect("slot 0 occupied");
        assert_eq!(init.seed, 0x5EED);
        assert_eq!(init.mode, GameMode::Creative); // 2-variant → canonical 4-variant
        assert_eq!(init.name, "Old Save");
        assert!(!init.multiplayer);
        assert_eq!(resolve_load(9, &cat), None); // empty slot
    }
}
