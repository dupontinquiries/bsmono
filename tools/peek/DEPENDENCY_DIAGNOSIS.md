# Dependency / composition diagnosis (peek + wiring-audit)

## The numbers (crates/unendlich-engine/src/sim, 2026-06)
- 439 sim modules; **257 (59%) have zero references outside their own file**.
- The `unendlich-fullgame` bin references **23** of them.
- ~40 of the orphans are **dead services** (own state + a `new/step/tick/apply`
  lifecycle) — systems that should be running but are wired to nothing:
  dragon_brain, dragon_locomotion, dragon_hunt, player_bending, bending_runtime,
  popdyn, projectile, swim, world_authority, perception_field, mob_memory,
  pause_overlay, menu_router, and the menu chain below.
- The rest of the orphans are **pure kernels** (math/transform fns) — expected to
  sit uncalled until something composes them; not a bug.

## Why this kept happening (the process hole)
Composition lives in the **bin** (`unendlich_fullgame.rs`) and the **renderer**,
which require `glfw`/`ui-overlay` system libs and so **cannot compile in the build
sandbox**. Every "wire X into the bin" step was therefore unverifiable and got
deferred to a handoff doc — so modules accreted, each green in isolation, while the
running game never adopted them. The menu is the clearest case:

    menu_controller  →  world_launch  →  title_cinematic
    (each referenced only by the next; none reached the bin, which still drives
     raw menu_open/menu_choice/wizard_open u32 flags)

There are also **four overlapping menu abstractions**: `sim::main_menu`,
`sim::menu_controller`, `world::title_flow`/`game_flow`, and the bin's loose flags.
No single source of truth.

## Proposal (matches dependency-graph.md's 3-tier model)
1. **Tier 1 — pure kernels** (~85%): leave as plain libraries. Do NOT register.
2. **Tier 2 — services** (state + lifecycle): one registration path via a
   `linkme`/`inventory` distributed slice, e.g. `#[service]` → a `SERVICES` slice
   the app core iterates. A file appearing in the build self-registers; the audit
   below fails CI if a `#[service]` is never scheduled.
3. **Tier 3 — tick-graph nodes** (`#[node] #[reads/writes]`): build.rs/proc-macro
   emits the execution order from the reads/writes sets (the dependency-graph
   compiler). Only the handful of top-level ticks need this.

## The key enabler: make composition testable
Move the bin's composition OUT of the untestable bin into a lib **app core**
(`engine::app`) that the thin bin just calls: `app.tick(input, dt) -> Frame`.
The app core wires services + the tick graph in *library* code that compiles and
tests in the sandbox. The bin shrinks to: create window → forward input → call
`app.tick` → present. Then "is the menu wired?" becomes a lib test, not a thing
only discoverable by running the GUI.

## Standing tools (this drop)
- `peek` — fast line/bucket/glob viewer (`peek -h src/**/*.rs:5xx`). Dogfooded.
- `wiring-audit.sh <src> ` — the census above; run it to watch the orphan/dead-
  service counts move as systems get composed. Wire it into CI as a regression gate
  once the Tier-2 registry exists (fail if dead_services increases).
