# Dispatch packet (r94) — 7 threads, ~half features / ~half service-wiring

Coordinator is handling **forge locomotion** (claw/drag zones, foot-planting, gait
modes, wing poses) in-house — verifiable via the collage — so no thread touches the
forge. Each thread below states its landing zone + headless verification path. A
return that can't be verified the stated way is not mergeable.

## Feature threads (4)

**F1 — menu vignette scenes.** The 3D set per menu: Singleplayer = tree + fauna,
Multiplayer = a bending dragon with random `Genome`, Settings = waterfall. Flat
push-in framing (a `CinematicPath` per menu — dolly, not orbit).
Landing: `sim::title_cinematic` (+per-menu paths), vignette content = `dragon_forge`
+ scene props. Verify: extend `examples/flythrough_snapshot.rs` into a per-menu
vignette collage (camera framing + scene), reviewer eyeballs it.

**F2 — menu interaction + translucent panels in the renderer.** Adopt
`menu_view::title_flythrough_ui` in the egui overlay (translucent 3D-feel panels),
wire submenu transitions, map clicks → `AppInput`. Landing: renderer egui pass + the
bin. Verify: the existing menu snapshot examples for layout; on-device is compile-on-end.

**F3 — DHT join (multiplayer).** Peer discovery + join. Landing: a **service in
`sim::service_registry`** + the Multiplayer flow. Verify: the join logic MUST be a
pure state machine `step(state, msg) -> (state, msgs)` unit-tested with simulated
messages (no sockets); socket I/O is a compile-on-end shell. Register the service so
the smoke test ticks it. Pure-core-or-reject.

**F4 — worlds with seeds (generator lifecycle).** Make `TerrainGenerator`
re-creatable post-menu from `WorldInit.seed`; reset the chunk manager on launch; pick
spawn from the new surface. Spec: `handoff/SEED_WORLDGEN_r93.md`. Verify:
`examples/heightmap_collage.rs` is ground truth that seed drives terrain; the bin step
is "Create uses the launch seed" (two fixed seeds → different spawn).

## Service-wiring threads (3) — the registry bulk

Pattern (the template is the `stamina` block in `sim/stamina.rs`): for each service,
add an adapter `impl Service` + a `#[distributed_slice(SERVICES)]` entry **in the
service's own file**, mapping `ServiceCtx` onto its `update`/`tick`/`step` (neutral
inputs where a real signal isn't wired yet), and write/keep a behavior harness.
Verify: the `service_registry` smoke test ticks ALL registered services panic-free
(collective wiring check); each service's own test covers behavior. Already done:
stamina, camera_tremble, dragon_thirst, affliction — do NOT redo these.

**S1 — dragon/body/locomotion:** dragon, dragon_wing_flex, locomotion_sim, swim,
forage_sim, fauna_behavior.

**S2 — player/combat/physics:** player_bending, player_views, carry, ragdoll,
sentinel, driver.

**S3 — world/items/misc:** grove_decay, item_entity, joint, lauscher_lair,
particulate_field, rollback, state.

NOTE for S-threads: a service whose `update` needs context the registry doesn't carry
yet (world refs, input) still registers + ticks with neutral inputs — that proves
WIRING. Feeding real signals is a later pass (the `ServiceCtx` grows). Do not block on it.
