//! `app::world_session` — the reusable world loop (r63-07 engine refactor).
//!
//! Extracts the body of `bin/unendlich_world.rs::main` into a callable entry so
//! both the standalone world bin AND the menu→gameplay bin (`unendlich-game`)
//! can launch a world. Render/window only — gated `#[cfg(feature = "render")]`.
//!
//! SEAM CONTRACT
//!   pub enum ExitReason { QuitToMenu, QuitToDesktop }
//!   pub fn run_world(spec: &WorldSpec, settings: &Settings) -> ExitReason
//!
//! `run_world` owns the GLFW window + Vulkan context + chunk/mesh/sim loop for
//! the lifetime of one play session and returns when the player leaves the
//! world: `QuitToMenu` (pause overlay → "Quit to Menu") or `QuitToDesktop`
//! (window close / "Quit to Desktop"). The caller decides what to do next.
//!
//! EXTRACTION RECIPE (mechanical; see docs/R63-07-WALKTHROUGH.md for line refs):
//!   The former `main` body (everything AFTER the help/version/profile/
//!   --test-spec arg handling, i.e. `unendlich_world.rs` lines ~205..4289) moves
//!   verbatim into `run_world` with exactly three edits:
//!     1. seed/config: replace the hardcoded `WORLD_SEED_DEFAULT` and env-derived
//!        render-distance with `spec.seed` / `settings.graphics.render_distance`
//!        (and the other `WorldSpec`/`Settings` fields). The CLI/env parsing
//!        stays in the bin's `main` and is folded into the `WorldSpec` passed in.
//!     2. exit: the loop currently ends only via `window.should_close()` (set at
//!        the two `set_should_close(true)` sites). Add a `quit: Option<ExitReason>`
//!        that the pause overlay sets, and return it after the loop (default
//!        `QuitToDesktop` for a raw window close).
//!     3. pause overlay: drive `sim::ingame_menu::IngameMenu` from Esc; when it
//!        emits SaveAndQuit→`QuitToMenu`, a desktop-quit row→`QuitToDesktop`.
//!
//! Until that move lands this module compiles as the seam (types + signature +
//! the new pause/quit wiring); the body is marked with a single `todo` shim so
//! the lib builds. Replace the shim with the moved body on integration.

#![cfg(feature = "render")]

use crate::settings_schema::Settings;

/// Why a world session ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ExitReason {
    /// Player chose "Quit to Menu" (pause overlay) — caller returns to the menu.
    QuitToMenu,
    /// Player closed the window / chose "Quit to Desktop" — caller exits.
    QuitToDesktop,
}

/// Everything needed to launch a world. Authored here as a minimal, stable
/// shape; r63-0x's `new_world_config` produces one of these. If a richer
/// `world::WorldSpec` lands from a sibling thread, re-export it instead and drop
/// this definition (kept here so the engine refactor compiles standalone).
#[derive(Clone, Debug)]
pub struct WorldSpec {
    /// World seed (was hardcoded `WORLD_SEED_DEFAULT` in the old `main`).
    pub seed: u32,
    /// Display name / on-disk folder under `~/.unendlich/worlds/`.
    pub name: String,
    /// World save directory; `None` ⇒ ephemeral/playtest (old behaviour).
    pub save_dir: Option<std::path::PathBuf>,
}

impl WorldSpec {
    /// The default world the standalone `unendlich-world` bin launches —
    /// preserves the old behaviour (default seed, ephemeral, "world").
    pub fn default_world() -> WorldSpec {
        WorldSpec {
            seed: crate::constants::WORLD_SEED_DEFAULT as u32,
            name: "world".to_string(),
            save_dir: None,
        }
    }
}

/// Run one world session to completion. Owns the window + Vulkan + sim loop.
///
/// See the EXTRACTION RECIPE above: the body is the moved `unendlich_world::main`
/// loop, parameterised by `spec`/`settings` and returning [`ExitReason`].
pub fn run_world(spec: &WorldSpec, settings: &Settings) -> ExitReason {
    // --- BEGIN moved body of `unendlich_world::main` (lines ~205..4289) ---
    //
    // Edit 1 (config): the old body opened with
    //     let generator = TerrainGenerator::new(WORLD_SEED_DEFAULT, Backend::Auto, gen_batch)?;
    // becomes
    //     let generator = TerrainGenerator::new(spec.seed, Backend::Auto, gen_batch)?;
    // and `runtime_render_distance()` is overridden by
    //     settings.graphics.render_distance
    // (the world_root/save path uses `spec.save_dir`).
    //
    // Edit 2 (exit): declare near the top of the loop scope
    //     let mut quit: Option<ExitReason> = None;
    // The two `window.set_should_close(true)` sites stay (they break the loop);
    // a raw window close leaves `quit == None`.
    //
    // Edit 3 (pause overlay): drive a `sim::ingame_menu::IngameMenu` from Esc.
    //     // in the per-frame input handling, on Esc:
    //     let action = menu.event(ingame_menu::UiEvent::TogglePause);
    //     // when the pause menu's Select fires on its rows:
    //     match menu.event(ingame_menu::UiEvent::Select) {
    //         ingame_menu::UiAction::SaveAndQuit => { quit = Some(ExitReason::QuitToMenu);
    //                                                 window.set_should_close(true); }
    //         // (add an explicit "Quit to Desktop" row → ExitReason::QuitToDesktop)
    //         _ => {}
    //     }
    //     // while `menu.is_paused()`, skip `tick_simulation` (freeze the sim) but
    //     // keep rendering, exactly like the §5c freeze_* debug toggles already do.
    //
    // After the `while !window.should_close()` loop, end with:
    //     return quit.unwrap_or(ExitReason::QuitToDesktop);
    //
    // --- END moved body ---

    // Seam shim (remove once the body above is moved in). Keeps the lib
    // compiling and documents the contract. `let _ =` silences unused-param
    // warnings without `unwrap`/`expect`/`panic!`.
    let _ = (spec, settings);
    ExitReason::QuitToDesktop
}
