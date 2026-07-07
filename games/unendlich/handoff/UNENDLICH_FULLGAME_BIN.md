# unendlich-fullgame — the binary (r72)

`--bin unendlich-fullgame` now exists. Build/run on your toolchain (the ui-overlay feature
is REQUIRED — the menu panel lives there):

```
cargo run --release -p unendlich-engine --features render,window,ui-overlay --bin unendlich-fullgame
```

## What it is
Based on `unendlich_world.rs` (the proven gameplay loop — world gen, mobs, items, the egui
inventory/HUD, physics, all of it) with a **title menu gate** wired in. So items/mobs/worldgen/UI
are exactly the working engine; the new part is the menu shell.

## What's wired
- **Title menu** (in `WorldRenderer`'s ui-overlay, lib): a centered `menu_open` panel with
  **Singleplayer / Host World / Join Server / Quit**. The renderer writes the click to
  `menu_choice` (1..4); the bin reads + resets it.
- **Bin flow**: opens the menu at startup (cursor freed, look frozen, world streaming behind it as
  a live backdrop). Click **Singleplayer** → captures the cursor and drops into the world. **Quit** →
  closes the window. **Host/Join** → currently enter the local world (see TODO) so the path is playable.
- **Title music**: `sim::music_director` drives the cue + fade from app state (full theme in menu,
  fades out into gameplay). Carried in `fg_music`; the bin steps it each frame.

## Verified vs compile-on-end
- **Verified in-sandbox**: the engine lib compiles with the new `menu_open`/`menu_choice` fields
  (gpu + render), full lib test 6423 pass / 3 known fails, HARNESS GREEN.
- **Compile-on-end (your toolchain)**: the menu *panel* (ui-overlay → window → glfw can't build
  here) and the bin edits. They use only verified APIs — the panel mirrors the existing settings
  panel (`egui::Window::new().anchor().show()` + `unendlich_ui::set_theme`); the bin uses the
  compiled `music_director`, the confirmed `renderer.menu_open/menu_choice/ui_pointer_active`
  fields, and glfw calls already in the file. Expect at most minor egui-0.34 signature tweaks.

## Iteration points (intended next, all small)
1. **Gate input while the menu is open** — movement/place-break still apply behind the menu. Wrap
   the player-control update in `if !renderer.menu_open { … }` for a clean pause.
2. **ESC → pause menu** — set `renderer.menu_open = true` on a pause key so you can quit-to-menu.
3. **Audio backend** — add `rodio`/`kira`; play `fg_music.asset()` at `fg_music.volume01`.
4. **Host/Join MP** — wire `sim::session_mode` (ServerBrowser/JoinFlow/HostFlow) + `net_candidates`/
   `net_rendezvous` per UNENDLICH_FULLGAME.md; chunk wire format per COMPRESSION_DECISION.md
   (seed for pristine chunks, lzma'd voxels for edited — never the octree).

## Tech debt
The two bins (`unendlich-world`, `unendlich-fullgame`) now duplicate the ~4.7k-line loop. Once the
menu path is stable, extract the loop into a shared module (`run_world(opts)`) and make both bins
thin callers — keeps the gameplay single-sourced.

---
## r73 — new-game wizard wired (world load still deferred)
- **Backend (verified):** `sim::new_game_wizard::NewGameWizard` — stepped Name → Mode → Seed →
  Confirm over `new_world_config` (+ catalog dup-name check). `next/back/can_advance/at_confirm`,
  `set_name/set_mode/set_seed`, `summary()`, and `finish(entropy,&catalog) -> WorldSpec`. 6 tests.
- **Flow:** title menu → **Singleplayer** opens the wizard (`renderer.wizard_open`, `menu_open=false`);
  the world is NOT generated. The bin owns the `NewGameWizard`, feeds the renderer's display fields
  (`wizard_title/summary/can_next/at_confirm`), and reads `wizard_action` (1 Back, 2 Next, 3 Create,
  4 Cancel, 5 cycle Mode, 6 toggle Seed). **Create** resolves a `WorldSpec` and logs it; **Cancel**
  returns to the menu. Title music keeps playing through the wizard.
- **Deliberately deferred (your call — "not loading the world yet"):** Create does NOT yet regenerate
  worldgen from `spec.seed`; it logs the spec and drops into the existing spawn world. Wiring the
  generator to the wizard's seed is the next step (needs deferring world setup until after the wizard).
- **Compile-on-end:** the wizard panel (renderer ui-overlay) + the bin wizard handling. Verified the
  lib compiles with the new renderer fields; the panel/bin use only confirmed APIs (mirrors the menu
  panel; bin uses the compiled `new_game_wizard`). Name editing is a follow-up (defaults to "New
  World"; add an egui `TextEdit` bound to `set_name` when you wire text input).

---
## r75 — sim::menu_controller (the unified menu backend)
`MenuController` replaces the bin's loose `menu_open`/`wizard_open`/`menu_choice` flags with one
object that owns the screen FSM (`game_flow`/`title_flow`) AND the sub-state behind each screen
(new-game wizard, saved-world list via `world_catalog`, `SettingsEditor`, multiplayer `ServerBrowser`).
6 tests; pure (entropy passed at commit).

### Bin usage (compile-on-end)
```rust
let mut menu = menu_controller::MenuController::new(catalog, settings); // or ::with_defaults()
// each menu frame: render menu.screen() + its sub-state; map input → calls:
menu.select(MenuId::NewGame);          // opens the wizard IN-MENU (world NOT loaded)
if let Some(w) = menu.wizard_mut() { /* drive set_name/next/back; render w.summary() */ }
match menu.confirm(entropy) {          // commit current screen
    MenuOutcome::StartWorld(spec) => { /* state()==Loading now → generate, then menu.world_ready() */ }
    MenuOutcome::LoadWorld(slot)  => { /* load saved slot */ }
    MenuOutcome::HostWorld | MenuOutcome::JoinSelected => { /* session_mode */ }
    MenuOutcome::QuitToDesktop    => break,
    MenuOutcome::None => {}
}
menu.select_slot(n);                   // on LoadSlots → MenuOutcome::LoadWorld(n)
menu.back();                           // wizard→title, or screen→title
// in-game: menu.pause()/resume()/quit_to_menu() drive the outer AppState FSM (Paused/InGame/MainMenu)
```
Key property (your "not loading the world yet"): selecting New Game opens the wizard while
`state()` stays `MainMenu`; only `confirm()` → `StartWorld` moves to `Loading`. The renderer's
`menu_open`/`wizard_open` panels become a single panel switch on `menu.screen()`.

---
## r78 — world_launch::resolve_load (completes the menu→world load path)
`world_launch::plan(outcome, current)` already maps a `MenuOutcome` → `LaunchPlan`. For a load,
`LaunchPlan::LoadLocal(slot)` carries only the slot; the bin now resolves the saved world's
generator inputs centrally:
```rust
match world_launch::plan(&outcome, current_spec.as_ref()) {
    LaunchPlan::StartLocal(init) | LaunchPlan::Host(init) => generate_world(init.seed, init.mode, &init.name),
    LaunchPlan::LoadLocal(slot) => {
        if let Some(init) = world_launch::resolve_load(slot, &catalog) {
            // saved seed regenerates base terrain; saved chunk diffs apply on top
            load_world(init.seed, init.mode, &init.name, slot);
        }
    }
    LaunchPlan::Join => session_join(), LaunchPlan::Quit => break, LaunchPlan::Stay => {}
}
```
`resolve_load` also bridges the two GameMode types (`world_catalog::GameMode` 2-variant →
`command_parse::GameMode` 4-variant) via `mode_from_catalog`, so the whole pipeline speaks the
canonical rules mode.
