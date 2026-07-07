# OWNERSHIP — who owns what, and how to route a request

The point of this file: **route work by reading this map, not by remembering
which chat "owned" an area.** The thread split was sound — focused context per
area is what kept the modules composable (the reconcile was almost purely
additive). What broke was using *your memory* as the router across three
parallel chats. This map is the router instead.

## Areas (by directory / concern)

| Area | Paths it owns | Was thread |
|---|---|---|
| **SIM** — locomotion, body/anatomy, creature AI, per-roamer tick | `crates/unendlich-engine/src/sim/**`, `creature*.rs`, `spawn.rs`, the roamer hooks in the bin | 01 |
| **ATMO/RENDER** — fog/moisture, shaders, render passes, descriptors/UBO, the render half of any feature | `render/**`, `shaders/**`, `moisture_*.rs`, `assets/shaders/**`, the `.spv` pipeline | 02 |
| **WORLD/PERF** — octree, meshing, chunk streaming, upload/LOD policy, plant genetics & ecology, exec-tier offload, dragons/emission cores | `world/**`, `sim/exec_tier.rs`, `sim/emission.rs`, plant/genetics modules | 03 |
| **MAIN** — the integration surface only you can verify | `bin/unendlich_world.rs`, `lib.rs` + the `mod.rs` declaration files, `Cargo.toml`, feature gates, the in-game A/B | (shared / yours) |
| **NET** — multiplayer (NEW) | rollback netcode, peer/session model, DHT routing + hole punching (reference: the `v1-r3` SnapClone/Jami app) | (new) |

> **Cross-cutting concerns** (not a single directory — split into core+wiring like
> any feature): **GPU-RUNTIME** (GPU slicing, kanal subscriber coordinators, 500 Hz
> pacing) and **DEVICE-TIERING** (M2 Ultra ↔ Fairphone 5 capability→budget scaling).
> Their pure policy cores are buildable now; their plumbing is integration work.

## The routing rule

1. Is it a **pure core** (a logic/sim module that takes data in, returns data
   out, no `crate::render` / `ash` / `vk`)? → it belongs to the area that owns its
   directory, and it can be **built and unit-tested in the sandbox** today.
2. Does it touch the **renderer, shaders, descriptors, or a UBO**? → **ATMO/RENDER**,
   and it is **integration-tier** — only verifiable by `cargo build --features
   render,gpu` + the in-game A/B (RULESET R3/R7/R16).
3. Does it touch the **bin loop, module declarations, or feature gates**? → **MAIN**
   (you), integration-tier.
4. Most "features" are **not one area** (see below) — split them first, then route
   each piece.

## Cross-cutting features must be split, not assigned

This is the trap that made it messy: a single idea spans areas, so it can't live
in one thread. Decompose into a pure core + its wiring before routing. Examples
straight from the backlog:

- **Bees** = a `Bee` creature kind (**SIM**) + `pollination` carrier wiring
  (**WORLD/PERF**) + the cold-sensitive `chill`/diurnal `rest_cycle` driven by
  real climate temperature (**ATMO/RENDER** supplies temp) + the kind enum lives
  in MAIN's `creature_design`. Four areas, one idea.
- **Player-recentering fog** = `MoistureField::recenter` (a **pure core**,
  testable now) → then the window rect moves into free UBO slots
  (**ATMO/RENDER**, integration).
- **Stage-tint animation** = `stage_tint::transition_tint` CPU ref (done) → the
  per-cell tint buffer + fragment lerp (**ATMO/RENDER**) → *then delete the
  transition-remesh sweep* in `chunk_manager` (**WORLD/PERF**).
- **Dragon emission** = `emission` core (done) → the world/render adapter draining
  it into `thermal::HeatField` (**WORLD/PERF** for the field, **ATMO/RENDER** for
  the HDR/bloom/in-scatter consumer).

**Rule of thumb:** the pure core and its wiring are *different work items in
different areas*. Build + test the core first (sandbox, fast, verifiable); queue
the wiring against MAIN/ATMO for an integration pass.

## The seam that explains everything

Across all three TODO lists, the dominant state is **"pure core built & tested —
needs wiring."** The sandbox can verify cores; only the M2 can verify wiring.
That seam — not the thread boundaries — is the real structure of this project.
Organize around it: a **build-and-test-cores loop** (any area, in the sandbox)
feeding a **periodic integration pass** (MAIN, on your tree). See `BACKLOG.md`,
which is ordered by that seam.
