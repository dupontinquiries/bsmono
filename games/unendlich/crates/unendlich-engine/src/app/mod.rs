//! `engine::app` — the **app core**: the one place game composition lives, in
//! *library* code that compiles and tests in the sandbox. The bin that owns the
//! window (glfw/ui-overlay, un-compilable here) shrinks to a thin shell:
//! create window → gather input → `App::tick(input)` → render `app.menu()` /
//! the world → present. Because composition now lives in the library, "is the
//! menu actually wired?" is a unit test instead of something only discoverable
//! by running the GUI — which is exactly the hole that let the menu rot.
//!
//! ## What a "service" is here
//! A service is just a stateful subsystem the core owns and drives each tick —
//! it has state + a lifecycle (`new` / `tick`), unlike the pure kernels that make
//! up most of the tree. The menu (`MenuController`) is the first one. Right now
//! the core holds its services as explicit fields (easy to read, no macro magic);
//! once there are many, a `#[service]` self-registration layer can discover them
//! without hand-adding fields. We prove the *pattern* first, on the thing that
//! broke, then scale the registration.

#[cfg(feature = "egui")]
pub mod menu_view;

pub mod multiplayer_svc;

use crate::sim::affinity_access::Fields;
use crate::sim::bending_runtime::{BenderState, BenderTick, GestureInput};
use crate::sim::command_parse::GameMode;
use crate::sim::game_flow::{AppEvent, AppState};
use crate::sim::title_cinematic::{CameraPose, CinematicPath};
use crate::sim::menu_controller::{MenuController, MenuOutcome};
use crate::sim::new_world_config::SeedChoice;
use crate::sim::settings_model::Settings;
use crate::sim::title_flow::{MenuId, Screen};
use crate::sim::world_catalog::WorldCatalog;
pub use crate::sim::world_launch::{self, LaunchPlan};

/// One frame of player intent from the shell. Menu-phase inputs for now; gameplay
/// inputs join here as more services move into the core.
#[derive(Clone, Debug, PartialEq)]
pub enum AppInput {
    None,
    Select(MenuId),
    Back,
    Confirm,
    SelectSlot(u8),
    WizardSetName(String),
    WizardSetMode(GameMode),
    WizardSetSeed(SeedChoice),
    WizardNext,
    WizardBack,
    Pause,
    Resume,
    /// r178: pause menu Save & Quit — InGame/Paused -> MainMenu (title root).
    QuitToTitle,
    /// r679: launch the Bending Tutorial directly (no wizard) with a fixed seed.
    StartTutorial(u64),
}

/// What the core wants the shell to do after a tick.
#[derive(Clone, Debug, PartialEq)]
pub enum AppStep {
    /// Stay in the menu; the shell renders `app.menu().screen()` + its sub-state.
    Menu,
    /// Begin/load/host/join a world (generator inputs already resolved).
    Launch(LaunchPlan),
    /// Leave to desktop.
    Quit,
}

/// The app core. Owns the services and composes them. Fully testable.
pub struct App {
    /// Service #1: the menu backend (screens, wizard, world list, settings, browser).
    menu: MenuController,
    /// Service #2: the player's bending runtime (coherence resource + technique
    /// resolution). A *gameplay* service — it only ticks when the phase whitelists
    /// it (see [`App::gameplay_tick`]).
    bending: BenderState,
    /// Title-screen cinematic: a looping camera orbit + a clock the menu fly-in
    /// reads. Advanced every frame via [`App::advance`].
    cinematic: CinematicPath,
    cinematic_elapsed: f32,
    // ── r118 gameplay services (ported from the parallel worker's Phase 1,
    //    adapted to this tree's real APIs) ──────────────────────────────────
    /// Player inventory + built-in hotbar (`items::Inventory::standard()`:
    /// slots, `selected`/`set_selected` for the hotbar cursor).
    inventory: crate::items::Inventory,
    /// Items lying on the ground, with proximity pickup (`collect_near`).
    drops: crate::items::drops::DropField,
    /// Sprint fuel. The shell ticks it (`tick(urge, dt)` -> speed multiplier)
    /// and gates sprint on `can_sprint()`.
    stamina: crate::sim::stamina::Stamina,
    /// r121 (worker Phase 4): hunger. The shell ticks the drain (2x while
    /// swimming) and `feed()`s it on the eat key.
    vitality: crate::sim::feeding::Vitality,
    /// r121: is the player currently in water (shell-detected via block query).
    swimming: bool,
    /// Fleet online play (bsgames handoff + host/join control plane) — see
    /// [`multiplayer_svc::MultiplayerSvc`]. Built from `BSGAMES_HANDOFF_*` at startup; the shell
    /// reads `handoff_started` to skip the title screen toward the match.
    pub multiplayer: multiplayer_svc::MultiplayerSvc,
}

/// One frame of gameplay context the shell feeds to the whitelisted services.
#[derive(Clone, Copy, Debug)]
pub struct GameplayInput {
    /// Elemental field strengths sampled at the player (drives discipline access).
    pub fields: Fields,
    /// A gesture the player performed this frame, if any.
    pub gesture: Option<GestureInput>,
    pub stillness01: f32,
    pub match01: f32,
    pub spent: f32,
    pub damage: f32,
    pub dt: f32,
}

/// What the whitelisted gameplay services produced this frame.
#[derive(Clone, Copy, Debug, Default)]
pub struct GameplayFrame {
    /// `Some` only when bending was whitelisted (in-game) and ticked.
    pub bending: Option<BenderTick>,
}

impl App {
    pub fn new(catalog: WorldCatalog, settings: Settings) -> App {
        App {
            menu: MenuController::new(catalog, settings),
            bending: BenderState::new(),
            cinematic: CinematicPath::intro(),
            cinematic_elapsed: 0.0,
            inventory: crate::items::Inventory::standard(),
            drops: crate::items::drops::DropField::new(),
            stamina: crate::sim::stamina::Stamina::full(),
            vitality: crate::sim::feeding::Vitality::full(),
            swimming: false,
            multiplayer: multiplayer_svc::MultiplayerSvc::from_env(),
        }
    }

    /// r131: construct with a restored world catalog so the menu's GameFlow
    /// (has_save / slot_count) is consistent with its contents from frame 0.
    /// Post-construction catalog_mut().add() desyncs flow vs catalog — that
    /// broke the menu in r126; shells should use this instead.
    pub fn with_catalog(catalog: WorldCatalog) -> App {
        App {
            menu: MenuController::new(catalog, crate::sim::settings_model::defaults()),
            bending: BenderState::new(),
            cinematic: CinematicPath::intro(),
            cinematic_elapsed: 0.0,
            inventory: crate::items::Inventory::standard(),
            drops: crate::items::drops::DropField::new(),
            stamina: crate::sim::stamina::Stamina::full(),
            vitality: crate::sim::feeding::Vitality::full(),
            swimming: false,
            multiplayer: multiplayer_svc::MultiplayerSvc::from_env(),
        }
    }

    pub fn with_defaults() -> App {
        App {
            menu: MenuController::with_defaults(),
            bending: BenderState::new(),
            cinematic: CinematicPath::intro(),
            cinematic_elapsed: 0.0,
            inventory: crate::items::Inventory::standard(),
            drops: crate::items::drops::DropField::new(),
            stamina: crate::sim::stamina::Stamina::full(),
            vitality: crate::sim::feeding::Vitality::full(),
            swimming: false,
            multiplayer: multiplayer_svc::MultiplayerSvc::from_env(),
        }
    }

    pub fn inventory(&self) -> &crate::items::Inventory { &self.inventory }
    pub fn inventory_mut(&mut self) -> &mut crate::items::Inventory { &mut self.inventory }
    pub fn drops(&self) -> &crate::items::drops::DropField { &self.drops }
    pub fn drops_mut(&mut self) -> &mut crate::items::drops::DropField { &mut self.drops }
    pub fn stamina(&self) -> &crate::sim::stamina::Stamina { &self.stamina }
    pub fn stamina_mut(&mut self) -> &mut crate::sim::stamina::Stamina { &mut self.stamina }
    pub fn vitality(&self) -> &crate::sim::feeding::Vitality { &self.vitality }
    pub fn vitality_mut(&mut self) -> &mut crate::sim::feeding::Vitality { &mut self.vitality }
    pub fn swimming(&self) -> bool { self.swimming }
    pub fn set_swimming(&mut self, s: bool) { self.swimming = s; }

    /// The shell reads this to render the current menu screen + sub-state.
    pub fn menu_mut(&mut self) -> &mut MenuController {
        &mut self.menu
    }

    pub fn menu(&self) -> &MenuController {
        &self.menu
    }

    pub fn state(&self) -> AppState {
        self.menu.state()
    }

    pub fn screen(&self) -> Screen {
        self.menu.screen()
    }

    /// Advance the title cinematic clock (called every frame by the shell, with
    /// real dt). The camera pose + menu fly-in read from this.
    pub fn advance(&mut self, dt: f32) {
        self.cinematic_elapsed += dt.max(0.0);
    }

    /// Seconds the title cinematic has been running (drives the menu fly-in).
    pub fn cinematic_elapsed(&self) -> f32 {
        self.cinematic_elapsed
    }

    /// The current cinematic camera pose — the renderer flies the title camera to
    /// this each frame (eye/target/fov), giving the "flythrough" behind the menu.
    pub fn camera_pose(&self) -> CameraPose {
        self.cinematic.sample(self.cinematic_elapsed)
    }

    /// Advance the app by one frame of intent. `entropy` seeds a random world seed
    /// at commit time (the shell passes a frame/time value).
    pub fn tick(&mut self, input: AppInput, entropy: u64) -> AppStep {
        let outcome = self.apply_menu_input(input, entropy);

        // Map the menu outcome → an app-level step via world_launch. THIS is the
        // wiring that used to be a dead chain (menu_controller → world_launch →
        // nothing); it now runs in one testable place.
        let plan = world_launch::plan(&outcome, None);
        let plan = match plan {
            // A saved slot only carries its index; resolve seed/mode from the catalog
            // (its seed still drives base-terrain regen) into full generator inputs.
            LaunchPlan::LoadLocal(slot) => match world_launch::resolve_load(slot, self.menu.catalog()) {
                Some(init) => LaunchPlan::StartLocal(init),
                None => LaunchPlan::Stay,
            },
            other => other,
        };
        match plan {
            LaunchPlan::Stay => AppStep::Menu,
            LaunchPlan::Quit => AppStep::Quit,
            p => AppStep::Launch(p),
        }
    }

    fn apply_menu_input(&mut self, input: AppInput, entropy: u64) -> MenuOutcome {
        match input {
            AppInput::None => MenuOutcome::None,
            AppInput::Select(id) => self.menu.select(id),
            AppInput::Back => self.menu.back(),
            AppInput::Confirm => self.menu.confirm(entropy),
            AppInput::SelectSlot(s) => self.menu.select_slot(s),
            AppInput::WizardSetName(n) => {
                if let Some(w) = self.menu.wizard_mut() {
                    w.set_name(&n);
                }
                MenuOutcome::None
            }
            AppInput::WizardSetMode(m) => {
                if let Some(w) = self.menu.wizard_mut() {
                    w.set_mode(m);
                }
                MenuOutcome::None
            }
            AppInput::WizardSetSeed(s) => {
                if let Some(w) = self.menu.wizard_mut() {
                    w.set_seed(s);
                }
                MenuOutcome::None
            }
            AppInput::WizardNext => {
                if let Some(w) = self.menu.wizard_mut() {
                    w.next();
                }
                MenuOutcome::None
            }
            AppInput::WizardBack => {
                if let Some(w) = self.menu.wizard_mut() {
                    w.back();
                }
                MenuOutcome::None
            }
            AppInput::Pause => {
                self.menu.pause();
                MenuOutcome::None
            }
            AppInput::Resume => {
                self.menu.resume();
                MenuOutcome::None
            }
            // r178: the pause menu's Save & Quit — the FSM transition that
            // r167's renderer-only overlay skipped (InGame stayed InGame,
            // so the "title" relaunched fresh worlds on every click).
            AppInput::QuitToTitle => {
                self.menu.quit_to_menu();
                MenuOutcome::None
            }
            // r679: direct tutorial launch — no wizard, no seed page.
            AppInput::StartTutorial(seed) => self.menu.start_tutorial(seed),
        }
    }

    /// The shell calls this once the world has finished loading: it advances the
    /// phase Loading → InGame so the gameplay services become whitelisted.
    pub fn enter_game(&mut self) -> bool {
        matches!(self.menu.world_ready(), AppEvent::BeginPlay)
    }

    /// Tick the gameplay services that the CURRENT PHASE whitelists. In `Menu` and
    /// `Paused` the gameplay layer is blacklisted — services are frozen and this
    /// returns an empty frame (this is the "the menu blacklists gameplay" idea,
    /// and the same mechanism a splash/pause uses to freeze the world). Only
    /// `InGame` whitelists them, so bending ticks.
    pub fn gameplay_tick(&mut self, input: GameplayInput) -> GameplayFrame {
        if self.menu.state() != AppState::InGame {
            return GameplayFrame::default();
        }
        let tick = self.bending.update(
            &input.fields,
            input.stillness01,
            input.match01,
            input.spent,
            input.damage,
            input.dt,
            input.gesture,
        );
        GameplayFrame { bending: Some(tick) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::world_catalog::{GameMode as CatMode, WorldCatalog, WorldEntry};

    // THE test the engine was missing: prove the whole menu → world chain is
    // reachable, in library code, without running the GUI.
    #[test]
    fn new_game_flow_reaches_a_launch() {
        let mut app = App::with_defaults();
        assert_eq!(app.tick(AppInput::Select(MenuId::NewGame), 0), AppStep::Menu);
        assert_eq!(app.screen(), Screen::NewGame);
        app.tick(AppInput::WizardSetName("Vale".into()), 0);
        app.tick(AppInput::WizardNext, 0);
        app.tick(AppInput::WizardNext, 0);
        app.tick(AppInput::WizardNext, 0);
        match app.tick(AppInput::Confirm, 0xABCD) {
            AppStep::Launch(LaunchPlan::StartLocal(init)) => {
                assert_eq!(init.name, "Vale");
                assert_eq!(init.seed, 0xABCD);
            }
            other => panic!("expected Launch(StartLocal), got {other:?}"),
        }
    }

    #[test]
    fn load_slot_reaches_a_launch_with_the_saved_seed() {
        let mut cat = WorldCatalog::new();
        cat.add(WorldEntry {
            name: "Old Save".into(),
            seed: 0x5EED,
            mode: CatMode::Survival,
            created_tick: 0,
            last_played_tick: 0,
        });
        let mut app = App::new(cat, crate::sim::settings_model::defaults());
        app.tick(AppInput::Select(MenuId::Load), 0);
        match app.tick(AppInput::SelectSlot(0), 0) {
            AppStep::Launch(LaunchPlan::StartLocal(init)) => assert_eq!(init.seed, 0x5EED),
            other => panic!("expected Launch with the saved seed, got {other:?}"),
        }
    }

    #[test]
    fn quit_reaches_quit() {
        let mut app = App::with_defaults();
        app.tick(AppInput::Select(MenuId::Quit), 0); // → ConfirmQuit screen
        assert_eq!(app.tick(AppInput::Confirm, 0), AppStep::Quit);
    }

    #[test]
    fn back_out_of_wizard_returns_to_title() {
        let mut app = App::with_defaults();
        app.tick(AppInput::Select(MenuId::NewGame), 0);
        assert_eq!(app.screen(), Screen::NewGame);
        assert_eq!(app.tick(AppInput::Back, 0), AppStep::Menu);
        assert_eq!(app.screen(), Screen::Title);
    }

    #[test]
    fn gameplay_is_blacklisted_in_menu_whitelisted_in_game() {
        use crate::sim::affinity_access::Fields;
        let mut app = App::with_defaults();
        let fields = Fields { h: 1.0, r: 0.0, w: 0.0, p: 0.0, l: 0.0, s: 0.0, b: 0.0 };
        let inp = GameplayInput {
            fields,
            gesture: None,
            stillness01: 1.0,
            match01: 1.0,
            spent: 0.0,
            damage: 0.0,
            dt: 0.1,
        };
        // In the menu, gameplay services are frozen.
        assert!(app.gameplay_tick(inp).bending.is_none(), "bending frozen in menu");
        // Walk into a game.
        app.tick(AppInput::Select(MenuId::NewGame), 0);
        app.tick(AppInput::WizardSetName("B".into()), 0);
        app.tick(AppInput::WizardNext, 0);
        app.tick(AppInput::WizardNext, 0);
        app.tick(AppInput::WizardNext, 0);
        app.tick(AppInput::Confirm, 1); // → Loading
        assert!(app.enter_game(), "Loading → InGame");
        // Now bending is whitelisted and ticks; coherence builds over a few frames.
        let mut last = 0.0;
        for _ in 0..10 {
            let f = app.gameplay_tick(inp);
            let t = f.bending.expect("bending ticks in game");
            last = t.coherence;
        }
        assert!(last > 0.0, "resonance accrues while playing (got {last})");
    }
}
