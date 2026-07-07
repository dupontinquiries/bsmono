# Dispatch packet (r91) — verification contracts for the parallel threads

The rule that keeps threads from re-orphaning: **every return must prove itself
in-sandbox (no bin), and land in a known seam.** Below, per thread area, is the
landing zone + the headless verification path. A return that can't be verified the
stated way is not mergeable — say so and it gets re-cut.

## A. Menus / submenus — themed 3D vignettes the camera frames
Vision: translucent 3D menu panels; camera pushes *through* them into a framed
scene (flat cinematic framing, not orbit — a dolly/push-in). One vignette per menu:
- **Singleplayer** — a tree with fauna (peaceful nature set).
- **Multiplayer** — a dragon bending, with *random genetics* (random `Genome` seed
  per view, so each load shows a different dragon).
- **Settings** — a peaceful waterfall.

Landing zone: `sim::title_cinematic` (a `CinematicPath` per menu — push-in keyframes,
not the orbit), `app::menu_view` (panel layout + per-menu scene selection), `App`
(which vignette is active follows `menu.screen()`). Vignette content = `dragon_forge`
models + simple scene props.

Verify headless: extend `examples/flythrough_snapshot.rs` — render each menu's
vignette + its camera framing into a labeled collage (like menu_flythrough.png), and
print the camera path per menu. Reviewer (operator) eyeballs the collage. The egui
panel translucency + the on-device camera are compile-on-end, but layout, framing,
timing, and the per-menu scene are all proven in the collage first.

## B. DHT joining — multiplayer peer discovery / join
Landing zone: a **service in `sim::service_registry`** (a `DhtJoin` service: peer
table + join state machine) + the `App` multiplayer flow (Multiplayer menu →
join/host).

Verify headless: **split the testable protocol from the I/O.** The join logic must
be a PURE state machine — `step(state, incoming_msg) -> (state, outgoing_msgs)` —
unit-tested by feeding simulated messages (no real sockets): bootstrap, find-node,
join, peer-table convergence, timeout/retry. Real socket send/recv is a thin shell
that's compile-on-end. A return whose join logic can't be ticked without a network
is not mergeable — push back and require the pure-core split. Register the service so
the registry smoke test ticks it.

## C. Worlds with various seeds — seed→worldgen
Landing zone: the generator lifecycle. Today `TerrainGenerator::new(WORLD_SEED_DEFAULT)`
is built ONCE at startup before the menu; make it **re-creatable post-menu, keyed by
the launch seed**, and have `App::enter_game` carry the `WorldInit.seed` into a fresh
generator. (This is also TODO #2.)

Verify headless: render a **heightmap/biome collage** — generate the world from 6
different seeds, rasterize each as a top-down heightmap (or a cross-section) into a
labeled grid (reuse the sim_collage PNG encoder + the new digit font). Different seeds
must visibly differ. This is fully in-sandbox (pure worldgen, no bin) — the first
truly visible menu→world payoff, and Claude can pre-check it before you ever build.

## What the coordinator (here) is doing in parallel
Building the `linkme` service registry (DONE: `sim::service_registry`, 4 services
registered incl. one co-located in `stamina.rs`, smoke test green) + this contract
set. Services from thread B (and any of the other 22) drop a `#[distributed_slice(
SERVICES)]` block in their own file; the smoke test verifies the wiring of all at once.
