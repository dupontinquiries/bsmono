//! game_mode — pure gamemode model + capability table. WID `r335-gamemode`.
//!
//! `command_parse::GameMode` is the single source of truth for the *enum* (it
//! drives the `/gamemode` text command). This module wraps it with a switchable
//! **state** a button/menu can flip (`toggle`/`set`/`cycle`) and a per-mode
//! **capability table** systems read to decide what a mode allows — creative
//! inventory (which is what surfaces `item::legendary_palette()` in the picker),
//! flight, instant break, invulnerability, and legendary spawning.
//!
//! Pure, deterministic, panic-free (SHARED_RULES §1): the state is plain data and
//! every method is a total function of `(state, arg)`.

pub use crate::sim::command_parse::GameMode;

/// What a gamemode permits. Read by gameplay systems and the creative picker.
/// All-false in Survival; everything in Creative.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GameModeCaps {
    /// Creative item picker is available — this is the gate the inventory screen
    /// checks before showing `item::legendary_palette()` (and the block/egg tabs).
    pub creative_inventory: bool,
    /// Free flight (no fall, no ground requirement).
    pub flight: bool,
    /// Blocks break in one hit (no mining time / tool gate).
    pub instant_break: bool,
    /// Takes no damage.
    pub invulnerable: bool,
    /// May spawn legendaries directly (creative-only authoring).
    pub spawn_legendaries: bool,
}

impl GameModeCaps {
    const NONE: GameModeCaps = GameModeCaps {
        creative_inventory: false,
        flight: false,
        instant_break: false,
        invulnerable: false,
        spawn_legendaries: false,
    };
}

/// The per-mode capability table.
///
/// - **Survival** — nothing special (the baseline play mode).
/// - **Creative** — everything: creative inventory (incl. the legendary palette),
///   flight, instant break, invulnerable, may spawn legendaries.
/// - **Adventure** — play-only: interact with the world but no free building or
///   creative tooling, so the same all-false table as Survival on these axes.
/// - **Spectator** — noclip observer: flies and is untouchable, but cannot act on
///   the world (no inventory, no breaking, no spawning).
pub fn caps(mode: GameMode) -> GameModeCaps {
    match mode {
        GameMode::Survival => GameModeCaps::NONE,
        GameMode::Creative => GameModeCaps {
            creative_inventory: true,
            flight: true,
            instant_break: true,
            invulnerable: true,
            spawn_legendaries: true,
        },
        GameMode::Adventure => GameModeCaps::NONE,
        GameMode::Spectator => GameModeCaps {
            flight: true,
            invulnerable: true,
            ..GameModeCaps::NONE
        },
    }
}

/// All modes in `cycle()` order.
const CYCLE_ORDER: [GameMode; 4] = [
    GameMode::Survival,
    GameMode::Creative,
    GameMode::Adventure,
    GameMode::Spectator,
];

/// The live gamemode a button/menu toggles and systems read.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct GameModeState {
    pub current: GameMode,
}

impl GameModeState {
    /// Start in an explicit mode.
    pub fn new(mode: GameMode) -> Self {
        Self { current: mode }
    }

    /// Capabilities of the current mode.
    pub fn caps(self) -> GameModeCaps {
        caps(self.current)
    }

    /// Set the current mode explicitly (menu pick / `/gamemode` command).
    pub fn set(&mut self, mode: GameMode) {
        self.current = mode;
    }

    /// The Survival↔Creative button. `Creative → Survival`; any other mode →
    /// `Creative`. So on the two main modes it round-trips
    /// (`Survival⇄Creative`), and from `Adventure`/`Spectator` the first press
    /// enters Creative (the next returns to Survival) — the button always lands
    /// you in one of the two main modes.
    pub fn toggle(&mut self) {
        self.current = match self.current {
            GameMode::Creative => GameMode::Survival,
            _ => GameMode::Creative,
        };
    }

    /// Advance to the next mode in [`CYCLE_ORDER`] (wraps). A full lap of four
    /// calls returns to the starting mode.
    pub fn cycle(&mut self) {
        // `position` always finds the mode (every variant is in CYCLE_ORDER);
        // the `None` arm keeps this total without `unwrap`.
        let i = match CYCLE_ORDER.iter().position(|&m| m == self.current) {
            Some(i) => i,
            None => 0,
        };
        self.current = CYCLE_ORDER[(i + 1) % CYCLE_ORDER.len()];
    }
}

impl Default for GameModeState {
    /// Worlds start in Survival.
    fn default() -> Self {
        Self { current: GameMode::Survival }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_round_trips_on_the_survival_creative_pair() {
        let mut s = GameModeState::new(GameMode::Survival);
        s.toggle();
        assert_eq!(s.current, GameMode::Creative);
        s.toggle();
        assert_eq!(s.current, GameMode::Survival);

        let mut c = GameModeState::new(GameMode::Creative);
        c.toggle();
        assert_eq!(c.current, GameMode::Survival);
        c.toggle();
        assert_eq!(c.current, GameMode::Creative);
    }

    #[test]
    fn toggle_from_other_modes_enters_creative_then_survival() {
        for start in [GameMode::Adventure, GameMode::Spectator] {
            let mut s = GameModeState::new(start);
            s.toggle();
            assert_eq!(s.current, GameMode::Creative, "first toggle should enter Creative from {start:?}");
            s.toggle();
            assert_eq!(s.current, GameMode::Survival, "second toggle should land Survival");
        }
    }

    #[test]
    fn cycle_advances_in_order_and_a_full_lap_round_trips() {
        let mut s = GameModeState::new(GameMode::Survival);
        s.cycle();
        assert_eq!(s.current, GameMode::Creative);
        s.cycle();
        assert_eq!(s.current, GameMode::Adventure);
        s.cycle();
        assert_eq!(s.current, GameMode::Spectator);
        s.cycle();
        assert_eq!(s.current, GameMode::Survival); // wrapped — full lap

        // a full lap returns to start from ANY mode
        for start in CYCLE_ORDER {
            let mut t = GameModeState::new(start);
            for _ in 0..CYCLE_ORDER.len() {
                t.cycle();
            }
            assert_eq!(t.current, start, "4 cycles should return to {start:?}");
        }
    }

    #[test]
    fn set_assigns_the_mode() {
        let mut s = GameModeState::default();
        assert_eq!(s.current, GameMode::Survival);
        s.set(GameMode::Spectator);
        assert_eq!(s.current, GameMode::Spectator);
    }

    #[test]
    fn caps_differ_per_mode() {
        assert_eq!(caps(GameMode::Survival), GameModeCaps::NONE);
        let c = caps(GameMode::Creative);
        assert!(c.creative_inventory && c.flight && c.instant_break && c.invulnerable && c.spawn_legendaries);
        assert_ne!(caps(GameMode::Survival), caps(GameMode::Creative));
        // Spectator: flies + untouchable, but cannot act on the world.
        let sp = caps(GameMode::Spectator);
        assert!(sp.flight && sp.invulnerable);
        assert!(!sp.creative_inventory && !sp.instant_break && !sp.spawn_legendaries);
        // Adventure is play-only on these axes.
        assert_eq!(caps(GameMode::Adventure), GameModeCaps::NONE);
    }

    #[test]
    fn creative_gates_the_legendary_palette_and_survival_does_not() {
        // The creative picker shows item::legendary_palette() iff creative_inventory.
        assert!(caps(GameMode::Creative).creative_inventory);
        assert!(caps(GameMode::Creative).spawn_legendaries);
        assert!(!caps(GameMode::Survival).creative_inventory);
        assert!(!caps(GameMode::Adventure).creative_inventory);
        assert!(!caps(GameMode::Spectator).creative_inventory);
    }

    #[test]
    fn caps_is_a_pure_deterministic_function() {
        for m in CYCLE_ORDER {
            assert_eq!(caps(m), caps(m));
        }
        // state mutation is deterministic too
        let mut a = GameModeState::default();
        let mut b = GameModeState::default();
        for _ in 0..7 {
            a.cycle();
            b.cycle();
        }
        assert_eq!(a.current, b.current);
    }
}
