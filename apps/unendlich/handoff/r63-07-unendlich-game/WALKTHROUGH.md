# r63-07 · unendlich-game — walkthrough, run_world signature, extraction recipe

BIN-GLUE thread. Render/window — **not buildable in-sandbox**; these files are
authored to compile on the integration tree where r63-01..06 are merged. Below:
the signature I settled on, the mechanical extraction recipe, the menu design,
and an honest list of what resisted extraction / is missing from this base tree.

## Files in this deliverable
- `src/app/mod.rs`                     — NEW. `#[cfg(feature="render")] pub mod world_session;`
- `src/app/world_session.rs`           — NEW. `ExitReason`, `WorldSpec`, `run_world` seam.
- `src/bin/unendlich_game.rs`          — NEW. Menu→gameplay shell bin.
- `src/bin/unendlich_world_THIN_main.rs.txt` — the lib hook (A), Cargo entry (B), and
                                         thin world `main` (C) to apply.

## run_world signature (settled)
```rust
// src/app/world_session.rs   (#[cfg(feature = "render")])
pub enum ExitReason { QuitToMenu, QuitToDesktop }
pub struct WorldSpec { pub seed: u32, pub name: String, pub save_dir: Option<PathBuf> }
pub fn run_world(spec: &WorldSpec, settings: &Settings) -> ExitReason
```
`Settings` is the existing `settings_schema::Settings`. `WorldSpec` is defined
minimally here so the engine refactor compiles standalone; if a richer
`world::WorldSpec` arrives from a sibling thread, re-export it and delete this
one — `run_world(&WorldSpec, &Settings)` is unaffected.

## Extraction recipe (mechanical)
`bin/unendlich_world.rs::main` is one monolithic fn (lines 134..4289; loop at
1257; window-close at 1266 & 1348). Split at the end of the arg/env preamble:

1. **Keep in the bin** (lines ~139..~205): help/version, `UNENDLICH_PROFILE`
   expansion, and `--test-spec` loading + `[env]` application + result-writer
   arming. These mutate process env / arm shutdown side-effects the loop relies
   on, so they run in `main` *before* `run_world`.
2. **Move into `run_world`** (lines ~205..4289): the generator/window/Vulkan
   setup and the `while !window.should_close()` loop. Three edits:
   - **config:** `TerrainGenerator::new(WORLD_SEED_DEFAULT, …)` → `…(spec.seed, …)`;
     world-root/save path uses `spec.save_dir`; `runtime_render_distance()`
     overridden by `settings.graphics.render_distance` (+ other Settings fields).
   - **exit:** add `let mut quit: Option<ExitReason> = None;` in the loop scope;
     after the loop `return quit.unwrap_or(ExitReason::QuitToDesktop);`.
   - **pause overlay:** drive `sim::ingame_menu::IngameMenu` from Esc; freeze
     `tick_simulation` while `menu.is_paused()` (same pattern as the §5c
     `freeze_*` toggles already wired into the loop); on its `SaveAndQuit` row
     set `quit = Some(QuitToMenu)` + `window.set_should_close(true)`, and add an
     explicit "Quit to Desktop" row → `QuitToDesktop`.
3. **Thin `main`** (snippet C): preamble, then `run_world(&WorldSpec::default_world(),
   &Settings::default())`. `unendlich-world` behaviour is unchanged.

## unendlich-game menu loop
Per the brief, control logic is pure models; the bin is glue + one thin render
adapter (`MenuApp`):
1. `WorldCatalog::scan(~/.unendlich/worlds/)` → `GameFlow::new(has_save, slot_count)`.
2. `SettingsEditor` from `settings.toml` (or `settings_model::defaults()`).
3. Per frame: `menu_screen_layout::layout_for_title(flow.title_screen(), viewport)`
   → draw → `menu_nav::resolve(layout, input)` → `TitleCmd` → `game_flow::apply`.
4. `AppEvent::EnterLoading|BeginPlay` → resolve `WorldSpec`
   (New Game: `new_world_config::to_spec`; Load/Continue: a `WorldCatalog` entry)
   → `run_world(&spec, &settings)`.
5. On return, `game_flow::on_exit(reason)` → `QuitToMenu` re-enters the menu
   (re-scan catalog), `QuitToDesktop` exits the shell loop.
Multiplayer routes to the `net_rendezvous`/`net_candidates` browser; split-screen
reuses `viewport_split`.

## Present in this base tree (verified)
`sim::title_flow` (r57), `sim::menu_nav`, `sim::viewport_split`,
`sim::net_rendezvous`, `sim::net_candidates`, `sim::ingame_menu`,
`settings_schema::Settings`.

## ABSENT here — coordinator must merge before building (resisted "compose")
These sibling outputs (r63-01..06) are referenced by `unendlich_game.rs` and are
NOT in this base tree; the bin documents each assumed signature inline:
`sim::game_flow::{GameFlow, AppEvent}`, `world::world_catalog::WorldCatalog`,
`sim::settings_model::{SettingsEditor, defaults}`,
`sim::menu_screen_layout::{layout_for_title, Layout, Viewport}`,
`sim::new_world_config::{NewWorldConfig, to_spec}`.
If any signature differs, reconcile at the call sites in `unendlich_game.rs`
(all confined to `main` + the `MenuApp` adapter).

## What resisted extraction (report-back)
- **No existing pause→menu seam.** The world loop currently only does
  `window.set_should_close(true)` (→ process exit). `QuitToMenu` is *new* wiring:
  the `ingame_menu` overlay + the `quit: Option<ExitReason>` flag described above.
  This is the one behavioural addition, not a pure move.
- **`--test-spec` coupling.** The spec loader applies an `[env]` table to process
  env and arms a result-file writer at shutdown. I kept this in the bin `main`
  (before `run_world`) so the moved body is unchanged; if `run_world` should own
  test-spec mode, pass the `LoadedSpec` in instead (signature would gain a param).
- **Hardcoded `WORLD_SEED_DEFAULT` + env-driven config.** The old body reads seed
  and many `UNENDLICH_*` env vars directly. The recipe routes seed/render-distance
  through `WorldSpec`/`Settings`; the remaining `UNENDLICH_*` reads can stay
  (env still wins) until folded into `WorldSpec`/`Settings` in a later pass.
- **Build status:** not compiled here (render/window unavailable in-sandbox, and
  6 sibling modules absent). `world_session.rs` carries a one-line seam shim so
  the lib compiles before the body move; replace it with the moved body on merge.
  Gates to run on the integration tree: `cargo check --release -p unendlich-engine
  --features render`, both bins via `--features render,window`, and `harness/run.sh`.
