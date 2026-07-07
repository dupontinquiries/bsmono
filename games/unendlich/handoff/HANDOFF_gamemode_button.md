# HANDOFF — `r335-gamemode` (button / keybind + caps reads, done on-device)

Pure half delivered: `sim::game_mode` — `GameModeState{current}` (`toggle`/`set`/`cycle`),
`GameModeCaps`, `caps(mode)`. The bin holds one `GameModeState` and reads `caps()`; the UI flips it.
None of this is wired here (SHARED_RULES §2) — proposed below.

## 1. Where the state lives
Hold a single `GameModeState` in the bin's per-session player/world state (next to the inventory /
creative-tab state in `world_renderer`). Initialise from the loaded world (`GameModeState::new(mode)`
or `::default()` = Survival). The `/gamemode <m>` text command already parsed by
`command_parse` should call `state.set(m)` so the button and the command stay in sync (single
source of truth — both mutate the one `GameModeState`).

## 2. The button + keybind → `toggle()`
- **Button:** a HUD/pause/settings entry labelled `Gamemode: {current:?}` that calls
  `state.toggle()` (Survival⇄Creative — the operator's ask). If you want the full rotation, bind a
  secondary control to `state.cycle()` (Survival→Creative→Adventure→Spectator).
- **Keybind:** bind a key to `state.toggle()`. Pick a non-conflicting key (the existing keymap is
  bin-side); a chord (e.g. `F4`) avoids clashing with movement. Re-render the button label after the
  call since `current` changed.
- **Menu host:** T-menus exposes a placeholder gamemode entry slot in its submenu tree — wire that
  entry's activation to `state.toggle()` (or a 4-way picker calling `state.set(..)`).

## 3. Where systems read `caps()`
`let c = state.caps();` then:
- **Creative inventory / legendary palette:** gate the creative picker on `c.creative_inventory`.
  In `world_renderer`, the inventory/creative-tab UI should only offer the creative tabs (Items /
  SpawnEggs / **Legendaries** from `r332-swords-creative`) when `c.creative_inventory` is true;
  otherwise show the survival inventory only. The Legendaries tab in particular should be hidden
  unless `c.creative_inventory` (and authoring spawns gated on `c.spawn_legendaries`).
- **Flight:** the movement/controller reads `c.flight` to allow no-fall free flight.
- **Instant break:** the block-break path reads `c.instant_break` to skip mining time / tool gating.
- **Invulnerable:** the damage path reads `c.invulnerable` to ignore incoming damage.
These are deeper bin systems; this module just supplies the table — each consumer reads its bool.

## Notes
- New module only (`sim/game_mode.rs`) + one `pub mod game_mode;` line in `sim/mod.rs`
  (suggested right after `pub mod command_parse;`). No bin/shared edits here.
- `caps()` for Adventure/Spectator is filled in sensibly (Adventure = play-only/all-false on these
  axes; Spectator = flight + invulnerable only). Adjust the table here if the design doc
  (`gamemode_trait_system.extracted.md`) pins different Adventure/Spectator rules — it's one match.
