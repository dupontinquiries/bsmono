# SECTION: Game shell — menu, wizard, settings, multiplayer entry

## 1. What this section is

The game shell is everything that wraps the gameplay loop: the title menu, the
new-game wizard, saved-world catalog, settings, pause overlay, title music, and
the entry points into singleplayer / host / join. Player-facing goal: launch
the binary, land on a title screen with music, pick or create a world (or join
someone else's), and drop into the world — then ESC back out to pause/quit
cleanly. Almost all *decision* logic is pure `sim::` and already merged; what's
owed is the engine-side wiring (deferred world-gen from the wizard seed, input
gating, audio backend, the MP transport glue) plus a two-bin de-dup.

Derives from `handoff/UNENDLICH_FULLGAME.md` (integration map) and
`handoff/UNENDLICH_FULLGAME_BIN.md` (the `unendlich-fullgame` binary, r72/r73).

## 2. Already built (merged, in `crates/unendlich-engine/src/sim/`)

- **`game_flow.rs`** — top-level FSM. `AppState{MainMenu,Loading,InGame,Paused,Exiting}`,
  `GameFlow::new(has_save,slot_count)`, `.state()`, `.apply(AppCmd)->AppEvent`, `.title()`.
- **`title_flow.rs`** — title-screen sub-FSM. `Screen`, `MenuId`, `TitleCmd`,
  `TitleEvent`, `TitleFlow::new(has_save,slot_count)`, `.apply(TitleCmd)`.
- **`menu_screen_layout.rs`** — `layout(screen, viewport)` produces the item
  rects for the active screen; consumes `title_flow::Screen` + `menu_nav::NavDir`.
- **`menu_nav.rs`** — focus/cursor FSM. `MenuNav`, `move_focus(NavDir)`,
  `activate()->NavEvent`, `back()`, `focused_id()`, `depth()`. (coordinator-built r57)
- **`new_world_config.rs`** — `NewWorldConfig`, `WorldSpec{name,seed,mode}`,
  `SeedChoice`, `valid_world_name`, `resolve_seed(choice,entropy)`,
  `validate(cfg)`, `to_spec(cfg,entropy)->Result<WorldSpec>`.
- **`new_game_wizard.rs`** — stepped Name→Mode→Seed→Confirm wizard over
  `new_world_config` + catalog dup-name check. `NewGameWizard::new()`, `step()`,
  `set_name/set_mode/set_seed`, `can_advance()`, `next/back`, `at_confirm`,
  `summary()`, `finish(entropy,&catalog)->WorldSpec`. (merged this round, r74)
- **`world_catalog.rs`** — saved-world list. `WorldCatalog`, `WorldEntry`,
  `GameMode`, `add/remove`, `by_recent()`, `slot_count()`, `has_save()`.
- **`settings_model.rs`** — `Settings`, `SettingField`, `defaults()`, `clamp()`,
  `adjust()`, and a draft/commit `SettingsEditor` (`draft/committed/apply/revert/dirty`).
- **`pause_overlay.rs`** — pause-menu FSM: `PauseScreen{Root,Settings,
  ConfirmQuitToMenu,ConfirmQuitToDesktop}`, `apply(PauseCmd)->PauseEvent`,
  confirm-gated quits. (coordinator-built r63)
- **`music_director.rs`** — `cue_for(AppState)->MusicCue`, `MusicState::step(cue,dt_ms)`,
  `.asset()->Option<&path>`; full theme in menu, ducked in loading/paused, faded in-game.
- **`session_mode.rs`** — MP decision layer: `SessionRole{Local,Host,Client}`,
  `WorldAdvert`, `ServerBrowser` (`observe/prune/by_recent/select/selected`),
  `ConnectPlan`, plus Host/Join sub-FSMs. (built r67)
- **`net_candidates.rs` / `net_rendezvous.rs`** — LAN-first candidate ordering +
  transport pick, and the join FSM/codec. (built r40/r58)
- **Bin (`src/bin/unendlich_fullgame.rs`)** — title menu gate over the proven
  `unendlich_world` loop; menu panel + wizard panel in `WorldRenderer`'s
  ui-overlay; music stepped per frame. Singleplayer→wizard works; **Create logs
  the WorldSpec but does not yet regenerate worldgen from its seed** (deferred).

## 3. The big sub-sections still to build

### 3a. Deferred world-gen from the wizard seed  *(the key restructure)*
**Scope:** today Create drops into the pre-streamed spawn world; wire the
generator to consume `WorldSpec.seed` so a new world actually reflects the
wizard's choices. **Touches:** the bin's world-setup ordering (defer streaming
until after the wizard resolves a spec), `world::gpu_generator`/spawn seed plumb.
**Deps:** `new_game_wizard.finish()` (done). **Pure?** No — bin + worldgen seam,
compile-on-end.

### 3b. Input gating during menus
**Scope:** wrap player-control update in `if !renderer.menu_open && !renderer.wizard_open`
so movement/place-break don't apply behind the menu. **Touches:** bin input
dispatch. **Pure?** No — bin, trivial, compile-on-end.

### 3c. ESC → pause
**Scope:** ESC sets `menu_open`/opens `pause_overlay`; route Resume/Settings/
Quit-to-menu through the merged `pause_overlay` FSM. **Touches:** bin key
handling + renderer overlay. **Deps:** `pause_overlay` (done). **Pure?** No — bin.

### 3d. Audio backend for music
**Scope:** add one dependency (`rodio` or `kira`); play `fg_music.asset()` at
`fg_music.volume01`, react to track changes. **Touches:** bin only; the cue/fade
math is already pure in `music_director`. **Pure?** No — bin + new dep, compile-on-end.

### 3e. Wizard name TextEdit
**Scope:** bind an egui `TextEdit` to `new_game_wizard.set_name`; today name
defaults to "New World". **Touches:** wizard panel (ui-overlay). **Pure?** No — render/bin.

### 3f. Hotbar / inventory wiring
**Scope:** surface the existing `inventory_grid`/`inventory_ops`/`item` models in
the HUD hotbar + inventory screen with real move/merge/split. **Touches:**
`inventory_screen.rs`, the egui HUD. **Deps:** `inventory_ops` (done). **Pure?**
Mixed — any new selection/cursor logic can be a pure module; the egui binding is render.

### 3g. Multiplayer Host/Join glue
**Scope:** wire `session_mode` (ServerBrowser/JoinFlow/HostFlow) + `net_candidates`/
`net_rendezvous` per `UNENDLICH_FULLGAME.md`; extend `run_world` to take
`SessionRole`; client world-streaming (the one large new subsystem). Chunk wire
format per `COMPRESSION_DECISION.md` (seed for pristine, lzma'd voxels for edited).
**Touches:** bin sockets/DHT, `run_world` seam, a new client-stream module.
**Deps:** all `net_*`/`session_mode` (done). **Pure?** Decisions pure (done);
transport/stream not.

### 3h. Two-bin de-dup (shared `run_world`)
**Scope:** extract the ~4.7k-line loop shared by `unendlich-world` and
`unendlich-fullgame` into `app::world_session::run_world(spec,settings,role)`;
make both bins thin callers. **Touches:** bin refactor. **Pure?** No — bin.

## 4. Prioritized roadmap (each ≈ one worker thread)

1. **3a deferred world-gen** *(bin/worldgen, compile-on-end)* — unblocks "new
   world is actually new"; the headline restructure. Do first.
2. **3b input gating** *(bin, tiny)* — clean pause behind menus; near-free.
3. **3c ESC→pause** *(bin)* — depends on 3b; gives quit-to-menu in-session.
4. **3d audio backend** *(bin + dep)* — independent; can run in parallel with 1–3.
5. **3e wizard TextEdit** *(render/bin)* — independent polish; parallelizable.
6. **3f hotbar/inventory** — split into a **pure** selection/cursor module
   (dispatchable now) + a render binding (compile-on-end).
7. **3h two-bin de-dup** *(bin refactor)* — do after 3a–3c stabilize the menu path,
   so the extracted `run_world` already carries the role param 3g needs.
8. **3g multiplayer** — largest; gated behind 3h's `run_world(role)` seam. Stage:
   (i) `SessionRole` into `run_world`, (ii) Host announce+admit, (iii) Join browser+
   connect, (iv) client world-streaming module. Decision logic already pure/merged.

Unblock chain: 3b→3c; 3h→3g(i); 3a is standalone and highest-value.

## 5. Open questions / decisions

- **Audio crate:** `rodio` (symphonia/Opus) vs `kira`? Operator picks; one dep either way.
- **3a ordering:** confirm world streaming may be fully deferred to post-wizard
  without breaking the live-backdrop menu (the backdrop currently streams the
  spawn world behind the menu).
- **MP identity:** `UNENDLICH_FULLGAME.md` ties identity to the "unendlich
  username" and turns the DHT on only for "Open as server" — confirm the username
  source and the SP-DHT-off invariant before 3g.
- **Catalog persistence:** `world_catalog` is an in-memory model; where do saved
  worlds serialize to on disk (slot files)? Needed before Load is real.
- **egui version drift:** BIN doc warns of egui-0.34 signature tweaks on the
  panels — operator confirms on their toolchain.
