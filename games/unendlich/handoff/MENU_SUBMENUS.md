# Title menu → Singleplayer / Multiplayer submenus (r656)

## What changed
The flat title menu (Singleplayer / Load World / Host World / Join Server / …)
now groups into two play-mode submenus:

- **Singleplayer** → Create World · Load World · Bending Tutorial · Back
- **Multiplayer** → Host World · Join Server · Back   (this screen already existed)

Main menu is now: Singleplayer · Multiplayer · Settings · Character · Quit.

## Wiring (the whole chain)
- `sim/title_flow.rs` (PURE, tested): added `Screen::Singleplayer`,
  `MenuId::Singleplayer` + `MenuId::Tutorial`, `TitleEvent::StartTutorial`.
  `select()` is now honoured from **Title and Singleplayer** screens; Singleplayer
  navigates into the submenu, Tutorial emits `StartTutorial` (screen unchanged),
  Create/Load behave as before but reachable from the submenu. 9 tests green
  (`rustc --test` standalone).
- `menu_controller.rs` / `game_flow.rs` — **unchanged**: the `other =>` and
  `_ =>` fallbacks already forward the new `MenuId`s to the flow and ignore the
  new event. Nothing to update there.
- `bin/unendlich_fullgame.rs`: menu-pick `8 → Select(Singleplayer)`,
  `9 → Select(Tutorial)`; `Screen::Singleplayer → menu_screen_kind 4`.
- `render/world_renderer.rs`: main-menu buttons regrouped; new `4 => { … }` arm
  draws the Singleplayer submenu. Create still opens the new-game wizard exactly
  as it did from the old main menu (the wizard-open gating is unchanged — only the
  button moved), so that path carries no new risk.

## The one stub — Bending Tutorial launch
The button is wired end-to-end and the FSM produces `TitleEvent::StartTutorial`,
but **nothing loads a practice world yet** — `menu_controller::select`'s `other`
arm applies the command and drops the event, so clicking Tutorial is currently a
no-op (stays on the submenu). To make it launch:
1. Give `menu_controller::select` an explicit `MenuId::Tutorial` arm that returns a
   new `MenuOutcome::StartTutorial` (instead of falling through `other`).
2. Handle that outcome in the bin where other `MenuOutcome`s drive the app FSM,
   booting a practice world. The bending-debug practice ladder from r645
   (`sim::tutorial_ladder` + `tools/tutorial_ladder_run.rs`) is the content; it
   needs an actual world-load path, which doesn't exist yet.

## Verify on build
title_flow is compile-verified. The bin + render edits are delimiter-checked but
need your build to compile-confirm. Quick playtest: main menu shows two play-mode
buttons; Singleplayer → Create/Load/Tutorial/Back; Multiplayer → Host/Join/Back;
Back returns to the title from each.
