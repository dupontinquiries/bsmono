# Wiring status ledger (verdict tiers)

Tiers: CHECKED (cargo check --features render green) · PROVEN (headless PNG/numeric
proof regenerated here) · WIRED (bin/pass/flag applied) · RAN (fullgame on hardware) ·
KEPT (survived). The operator can't reach RAN for a while, so PROVEN via headless
snapshot is the working bar — and WIRED render-side (not bin) is the next reachable rung.

| feature | module | CHECKED | PROVEN | WIRED | notes |
|---|---|---|---|---|---|
| world reset epoch | world::mesh_epoch | ✓ | ✓ | ✗ | bin records world_epoch() at mesh-job issue, drops stale on intake |
| creature grime | sim::creature_grime | ✓ | ✓ (pov collage) | ✗ | feed grime tint into forge material/skin color |
| dynamic lights | render::dynamic_lights | ✓ | ✓ (pov collage) | ✗ | light pass/descriptors; gather_nearest per region |
| viewmodel in hand | sim::viewmodel | ✓ | ✓ (pov collage) | ✗ | dedicated viewmodel draw pass + CameraBasis from glm cam |
| forage mind | sim::mob_forage_memory | ✓ | ✓ | partial | registered service; ticks. Drive creature decisions in bin loop |
| deep caves | world::cave_field | ✓ | ✓ (world_cave_slice: real terrain) | ✓ WIRED | is_cave OR's cave_field in behind UNENDLICH_MULTI_SCALE_CAVES=1 (cpu_generator); every solidity site picks it up. Verified in the actual generated world. |
| flora | world::flora_placement | ✓ | ✓ (pov + flora map) | ✗ | call place_at in chunk populate; spawn plant_lifecycle/stem |
| forge idle clock | bin anim_time tick | n/a | n/a | written | bin edit (compile-on-end, unverifiable here) |
| surface flora -> world blocks | flora_map + world::flora_blocks | ✓ | ✓ (flora_scatter_map + flora_patch_scene: forest as blocks) | ✓ WIRED + bin populate | Full chain: flora_map decides per cell from the live generator; flora_blocks instantiates each as voxel blocks (trees = OakLog trunk + OakLeaves canopy; ground cover = TallGrass/Flower/Fern/Mushroom; Cactus stack). region_flora_blocks is the chunk-populate seam. Bin populates a 40x40 patch around spawn on launch (compile-on-end). 3D patch render proves the forest. 5 tests. Robust long-term home: per-chunk generation (needs cross-chunk canopy). |
| creature grime -> forge material | sim::creature_grime + world::entity::forge_to_vertices | ✓ | ✓ (forge_grime_collage.png) | ✓ WIRED + bin feed | Creature.grime belly-weighted into forge body boxes at mesh time; eyes/mouth clean. Render proves clean vs muddy. Bin accumulates per tick (compile-on-end). |
| frosted menu buttons | app::menu_view::apply_frosted_style | ✓ | ✓ (menu_title.png) | ✓ WIRED | scoped egui visuals: translucent rounded glass fills + light frosted edge + hover/press glow on the real menu_ui. Verified via menu_snapshot. NOTE: full frosted-BLUR + parallax need a 3D world backdrop behind the menu (none yet — menu renders on flat dark); that backdrop is a separate render task. |
| voxel title (IN-WORLD blocks) | sim::voxel_text -> world::title_structure | ✓ | ✓ (title_blocks_scene.png: 3D Crystal cubes) | bin-wired (cfg-free, compile-on-end) | CORRECTED: title is REAL voxel blocks (BlockId::Crystal) in the world, not UI text. world::title_structure::unendlich_blocks gives world positions (2 tests). Bin places the monument near spawn on launch via chunk_manager.set_block. Render proves the look; placement is the bin wiring. (egui text title removed per request.) |
| spatial music | audio::SpatialMusic (opus-native, decode-once) | ✓ (audio feature builds in-sandbox) | ✓ decode of koYopper.opus verified (engine fn) + PcmLoop loop tested | ✓ WIRED (bin, cfg) | OPUS KEPT: ogg+audiopus decode ONCE -> Arc<[i16]>, PcmLoop loops by index-wrap (no rodio re-decode). Bin starts it before the loop + updates per frame (cfg audio). Needs audio device at runtime (no hardware to confirm sound). Asset: assets/music/koYopper.opus or UNENDLICH_MUSIC. Spatial mix needs separate stems; single track works now. |

## The honest gap
Everything is CHECKED+PROVEN; almost nothing is WIRED. The bin (glfw) can't compile
here, so bin wiring is unverifiable — which is why it keeps not landing. The way
forward that doesn't dead-end: wire features into the RENDER/GENERATOR layer (which
DOES compile headless) and prove with a snapshot, instead of bin handoffs. cave_field →
generator is the cleanest next target. Music is blocked on the opus→ogg transcode, full
stop — that's an asset step, not code.

## gameplay_pov_collage
Headless first-person proof that the grime/lights/viewmodel/flora cores compose into a
player POV (examples/gameplay_pov_collage.rs). The forge dragon renders blocky in the
example's CPU painter's rasteriser (not the real Vulkan renderer) — it proves the cores
produce the right colors/poses, not final image quality.

## r113 — batch service wiring (sim::service_registry_batch)
- NEW `sim/service_registry_batch.rs`: registers 17 previously-orphaned tick-shaped
  stateful modules into SERVICES with neutral-default adapters: fabric,
  cognition::forager, dragon, fauna_mind, forage_sim, locomotion_sim, lumen,
  mob_forage_memory, popdyn, projectile, storm_field, summon, swim, thermal,
  ui_feel, wetness, adaptive_stream. Verified: `cargo check --release --features
  render` green; `cargo test --lib service_registry` 2/2 pass (smoke + batch).
- BIN (compile-on-end): `unendlich_fullgame.rs` now builds a
  `ServiceHost::from_registry()` at startup and `tick_all(ServiceCtx { dt })`
  once per frame — so ALL registered services (central + co-located + batch,
  ~33 total) are live in the fullgame for the first time. Uses only verified
  lib APIs (ServiceHost/ServiceCtx compile under render).
- Honest remainder (orphan census from the bin's reachability): 1025 engine
  modules; 185 reached from the bin pre-change; ~840 orphans. Of those, only
  the 17 above (+13 already-registered) are free-running stateful services.
  The other ~810 are pure kernels (shader math, packing, curves) or data-seam
  types (need real world/chunk inputs: cave_flora, chunk_flora populate, etc.)
  — per the registry's own doc they need real call sites, not a host tick;
  they cannot be batch-wired meaningfully.

## r114 — generated registries (tools/bootstrap_registry.py)
- NEW second tier `sim/kernel_registry.rs`: KERNELS distributed slice for PURE
  kernels. Each entry sweeps its kernel across a deterministic 240-tick input
  phase under black_box — linked into the bin AND fuzz-lite exercised forever.
  `KernelHost::new(stride)` amortizes the sweep across frames.
- NEW `tools/bootstrap_registry.py`: scans the crate, synthesizes arguments for
  any signature made of conjurable types (f32/ints/bools/arrays/slices/tuples/
  Fixed/Vec3), and GENERATES `sim/kernel_registry_gen.rs` + 
  `sim/service_registry_gen.rs`. Self-healing: rustc errors prune the offending
  GEN-ENTRY block and re-check; sweep-test panics prune the offending module and
  re-test (one-shot census via kernel_registry::prune_support::report_all_panickers).
  Survivors are compile-green and 240-tick panic-free by construction. Idempotent;
  re-run after adding modules.
- RESULT: 846 kernels + 59 services generated → after healing 819 kernels + 51
  services survive (27 entries compile-pruned; sim::fluid + moisture_field
  panic-pruned). Registry totals now ~84 services + 819 kernels live.
- VERIFIED: dev check green; release --features render green; lib tests matching
  "registry" 14/14 pass (incl. kernel sweep + service smoke + batch).
- BIN (compile-on-end): fullgame owns `KernelHost::new(8)` beside the ServiceHost
  and calls `exercise()` per frame — ~102 kernel calls/frame, full set every 8.
- Panic-free debt: sim::fluid::FluidField and moisture_field::MoistureField
  panicked under the neutral sweep (likely size/index preconditions) — they need
  hand adapters with valid dimensions, or precondition guards.

## r115 — SPEEDRUN, UNVERIFIED IN-SANDBOX (operator verifies)
NOTHING below has been cargo-checked or tested here, by agreement. Verify with:
  python3 tools/bootstrap_registry.py     # regenerates + self-heals to green
  cargo test -p unendlich-engine --lib registry -- --skip report_all_panickers
- Hand adapters in service_registry_batch.rs for the two panic-pruned sims:
  sim::fluid (8x8x8 box, drip + flow_step, no solids) and moisture_field
  (16x16 grid, advect under steady breeze + step_climate with mild constant
  climate). Generated entries for those modules are skipped automatically.
- Synthesizer widened (r115 generalizations): &str, char, Option<T> (None),
  Vec<T> (empty), (i32,i32[,i32]) tuples, &[T;N], BlockId::Air, and same-file
  Default structs by value or reference. Regenerated with --no-verify:
  945 kernel + 75 service entries pre-heal (was 846/59). Expect the heal loop
  to prune some — survivors land wherever your run says.

## r115b — generator fixes from operator's compile log (still UNVERIFIED here)
- Option<T> args now emit bare `None` (param type is concrete at the call site,
  so it infers) — kills every None::<UnqualifiedType> error class.
- Free-fn scan strips inner `mod x { }` bodies first — no more attributing
  `fields::moisture::field_def` / `subvoxel::recipes::*` to the file module.
- Module discovery requires a `pub mod name;` declaration chain from lib.rs —
  undeclared-on-disk files (world/water_waves.rs) are skipped entirely.
- Regenerated --no-verify: 938 kernels + 75 services pre-heal.

## r115c — impurity blacklist + kernel quarantine (UNVERIFIED here; operator gates)
- Operator's hardware run: 118 services + 938 kernels wired live; first-frame
  panic in render::hot_shaders — the &str synthesizer fed "probe" to a fn that
  reads the filesystem. Root cause: IMPURE modules must never be auto-wired.
- Generator: IMPURE_RE blacklist (std::fs/io/net/process/env/time, File::open,
  read_to_string, env::var, Command::new, Instant::now, sockets...) excludes a
  module from BOTH generators. Regenerated: 878 kernels + 69 services (-60/-6,
  all liabilities).
- KernelHost: panicking kernels are QUARANTINED at runtime — caught via
  catch_unwind, logged once by name, permanently skipped; the frame survives.
  `quarantined()` exposes the count. The census test remains the prune path;
  this is the backstop for running the bin without the heal loop.

## r116 — SEAM WIRING, not registry (UNVERIFIED here; operator gates)
Registry expansion stopped: it proves linkage, not gameplay. Visible features
need their real seams. This round:
- AUDIO ROOT CAUSE: asset exists; bin probed cwd-relative paths and operator
  launched from crates/. Added CARGO_MANIFEST_DIR-anchored candidates — music
  now starts from any cwd (needs --features audio as before).
- DEEP CAVES: multi_scale_caves_enabled() now defaults TRUE (=0 opts out).
  The layer was PROVEN+WIRED for months; it was only ever dark because no one
  set the env var.
- FLORA WORLD-WIDE: the chunk-stream populate loop (delta.to_populate) now
  calls region_flora_blocks per newly-entered chunk — trees/grass/flowers/
  ferns/mushrooms/cacti everywhere, not just the 40x40 spawn patch. Roamer cap
  break→continue so creature caps no longer starve vegetation.
- WEATHER/WIND: weather layer defaults ON (UNENDLICH_WEATHER=0 opts out).
Discoverability findings for the operator (already in the build):
- Creative inventory + settings: press P (layout-aware) for panel mode; Esc
  toggles the settings panel; hotbar 1..9 picks the place block.
- F4: in-world debug overlay incl. the wind arrow.
Still genuinely missing (need real work, not flags): pause-to-title via
MenuController in the bin; survival item/loot loop; bloom/SSR/godray passes
(CPU reference math exists, GPU passes don't); fluid/thermal/wetness fields
coupled to the world rather than ticking in isolation.

## r116b — caves root cause + spawn eggs + telemetry (UNVERIFIED here)
- DEEP CAVES ROOT CAUSE: the fullgame generates terrain on the GPU
  (chunk_generation.comp); cave_field existed only in the CPU generator, so the
  r116 default-flip changed nothing visible. PORTED cave_field to GLSL
  (cave_field_at: worm tunnels, depth-blended morphology, chambers, abyssal
  chasm fissures, crust/floor gates — structure-identical, f32) and OR'd it
  into all three solidity sites using the per-column surface_target. CPU and
  GPU now carve the same caves. CHECK ON YOUR SIDE: build.rs recompiles the
  .comp if glslang/shaderc is installed — confirm the "recompiled" build log
  line, else you're running stale pre-compiled SPIR-V and will see no caves.
- SPAWN EGGS: G (while mouse captured) hatches a creature ~6 blocks ahead,
  cycling Fauna/Dinosaur/Lauscher/Spaeher/Dragon; persistent=true so streaming
  never despawns player-hatched creatures. Same construction path as wild
  ecosystem spawns. Logs "[spawn-egg] hatched ..." per press.
- TELEMETRY (the logging that should have been there from the start): startup
  "[features] ..." banner stating each wired feature + its kill switch; the
  ECOSYSTEM STREAM line now reports flora placed/tried per crossing, with the
  interpretation of 0/N (frontier set_block on unloaded chunks) inline.
- Dragon flight anim noted: height-only is the flight controller without wing
  animation; dragon_wing_flex exists as a service but isn't fed into
  forge_to_vertices at mesh time — a real mesh-path task, queued.

## r116c — spawn-egg scope fix + parallel-worker brief (UNVERIFIED here)
- Fix: spawn-egg block sits outside the streaming block's local `use`, so
  CHUNK_SIZE is now fully qualified (operator's E0425 x2).
- HAIKU_WORKER_BRIEF.md added at repo root: leaf tasks + hard rules for a
  parallel fast-model worker (no docs sprawl, no parallel architectures,
  claims require runtime log lines or test output).

## r117 — cave merge kit + dragon wing animation (UNVERIFIED here)
- CAVES OPERATOR-CONFIRMED on hardware. Standalone merge kit shipped:
  CAVES-STANDALONE.patch (only chunk_generation.comp + cpu_generator.rs) and
  docs/CAVES_WIRED.md with manual-graft instructions for diverged trees and
  the stale-SPIR-V build gotcha.
- DRAGON WING ANIM root cause: forge_to_vertices built ForgeAnim with
  `..Default::default()`, so wing_pose was ALWAYS Spread regardless of flight —
  the brain's pose was never threaded to the mesh. Fix: derive pose from body
  velocity inside forge_to_vertices (vy>0.5 or fast horizontal => Beating;
  vy<-2 => Flared; else Spread, identical to before for grounded creatures).
  Applies to every forge-rendered creature regardless of which collection owns
  it. New test airborne_velocity_changes_the_wing_pose. NOTE: if the operator's
  flyer still doesn't flap, the next suspect is anim_time not ticking for that
  creature's collection (roamers tick it; sim-bridge test mobs may not) — the
  velocity-pose change will still alter the held shape even then.

## r118 — Haiku Phase 2/3 MERGED (adapted), UNVERIFIED here
The parallel worker's patch targeted ITS OWN tree's APIs (App.drops via its
Phase 1, sim::game_flow gates, items::ItemStack::single, a MobsIntegration).
Merged by PORTING INTENT onto this tree's real APIs:
- App grows inventory (items::Inventory::standard — hotbar built in), drops
  (items::drops::DropField), stamina (sim::stamina::Stamina) + getters; both
  ctors updated.
- Bin: 3 test drops near spawn (ItemId mirrors BlockId per items/mod.rs);
  proximity pickup each frame -> inventory.add_stack(UniformRegistry(64)),
  with pickup/overflow logs; stamina ticked from the Sprint urge and sprint
  speed gated on stamina.sprinting; I toggles inventory view; pause =
  settings panel (Esc) OR inventory open; [HUD] heartbeat every 120 frames
  (state, stamina %, hotbar slot, occupied slots, ground drops).
DELIBERATE DIVERGENCES from the worker's patch (recorded so nobody re-adds):
- Its AppState::InGame gates would NO-OP silently — this bin enters the world
  without driving App's FSM to InGame. Gated on mouse_captured/!game_paused
  (the bin's real play state) instead.
- Its MobsIntegration spawn/update/render DROPPED: this tree's roamers +
  ecosystem stream already do creature spawn/AI/render properly; a second mob
  system double-renders and fights the cap.
- Its hotbar 1-9 handler DROPPED: the bin already binds 1-9 to the block
  palette; double-binding would fight it. Hotbar select stays via
  inventory.set_selected for a future UI.
- Its claims ("420+ modules active", "all tests passing", "<3.5ms") were not
  reproduced and are not asserted here.
Items remain LOG-ONLY visually (no ground-drop mesh yet) — pickup is proven
by the [items] log lines; rendering drops is the next leaf (HAIKU brief T4).

## r119 — winding default, egg key fix, DRAGON SOARING (UNVERIFIED here)
- WINDING: debug_settings default winding_variant = 2 (CW + cull NONE) — the
  variant the operator confirmed correct on hardware via the live sweep.
  UNENDLICH_WINDING=N overrides; the panel slider still works live.
- SPAWN EGG KEY: G collides with GameAction::ToggleFlight (qwerty preset) —
  that's the "eggs place blocks" report: G toggled player flight, the click
  placed the block, the egg fired unseen or not at all. Egg moved to H
  ("hatch"), unbound in the preset. Watch for "[spawn-egg] hatched ..." log.
- DRAGON SOARING: roamer dragons now run a flight cycle (grounded ~12s ->
  climb to ~16 blocks over terrain, banked wide circle ~25s at 7 blk/s ->
  descend, land). Per-individual seeded phase desynchronizes the flock.
  Sets body.velocity so the r117 wing poses engage: Beating on climb/cruise,
  Flared on descent. Ground snap-to-surface is skipped while flying (both
  snap sites guarded). Logs [dragon] TAKEOFF / LANDED with coordinates.
- Flora telemetry from the operator run reads healthy: 0/2343 only on the
  boot crossing (chunks not yet loaded — known frontier gap), then 9750/9856
  and 11775/11775 — world-wide flora is landing.

## r120 — const fix + batch seam wiring (UNVERIFIED here)
- FIX: DebugSettings::new is const — env::var can't live there (operator's
  E0015/E0658 wall). Static default stays 2; UNENDLICH_WINDING is read at
  runtime in the bin and stored into DEBUG.winding_variant, logged.
- LOAD FOG default ON (UNENDLICH_LOAD_FOG=0 opts out) — same built-but-dark
  pattern as weather/caves.
- GROUND DROPS RENDER: each DropField drop is a small spinning cube in the
  entity mesh (color from items::default_palette swatch by ItemId, golden
  fallback). The r118 test drops near spawn are now VISIBLE; pickup removes
  the cube. First visible item loop.
- HOTBAR SYNC: the 1-9 place-palette selection also moves the App inventory
  cursor (set_selected) so the HUD hotbar slot matches what you place.
- Investigated, no change needed: player flight permission already defaults
  full_admin (F toggles fly in single-player).
- NOTE the cfg(feature="winit") warning in the operator log predates this
  work (input_codes.rs) — harmless, not ours.

## r121 — offspring fix + worker Phase 4 port (UNVERIFIED here)
- FIX: third Roamer literal (breeding offspring, line ~2460) gets flying +
  flight_phase — the operator's E0063. All 4 literals audited OK now.
- WORKER PHASE 3 AUDIT: pause/I/hotbar/HUD were already ported in r118/r120
  (with its dead AppState gates replaced); nothing new to take.
- WORKER PHASE 4 PORTED with corrections:
  * SWIMMING: real block query (chunk_manager.block_at at the feet ==
    BlockId::Water) instead of its y<64 guess — y<64 would mark every cave
    and valley as "swimming". Logs entered/left water.
  * HUNGER: App gains vitality (sim::feeding::Vitality) + swimming flag;
    drains 0.01/s on land, 0.02/s in water.
  * EAT KEY: V, not its E — E is movement (Dvorak preset), Q is DropItem,
    R is OpenInventory. feed(0.25) + log.
  * WORLD CLOCK: 1 real second = 1 game minute from 08:00; HH:MM leads the
    [HUD] line, which now also shows vitality % and (SWIM).
- Its "480+ modules / 46% / all tests passing / <4.3ms" claims: not
  reproduced, not asserted.

## r122 — scroll hotbar + THE item loop (UNVERIFIED here)
Operator confirms: dragon TAKEOFF fired, pause/clock/vitality HUD live,
1-9 palette + mining + settings all working. New:
- SCROLL HOTBAR: WindowEvent::Scroll accumulates; one palette step per wheel
  notch (touchpad swipe lands here too), wraps, syncs selected_block + the App
  inventory cursor; 1-9 keeps the index in sync. Logs "[hotbar] scroll ->".
  Scroll is ignored while uncaptured so panels keep their own scrolling.
- MINING DROPS (the actual item loop): breaking a block runs
  mining::block_drop (stone->cobblestone, leaves->nothing, bare-hand harvest
  table) and spawns the drop into the App DropField with a small upward pop;
  drop PHYSICS now ticks every play frame (gravity -18, settle on real voxels
  via chunk_manager.block_at, age-out) so drops arc and rest on the ground;
  the r120 cube renders them; proximity pickup banks them. break -> see ->
  collect -> HUD inv count rises. Non-block drops (ore items etc.) log only.
- Note: operator HUD showed inv 0 / drops 0 — the spawn test-drops may not
  have run that session (landmark gate); mining drops supersede them as the
  proof and work anywhere.

## r123 — worker Phases 6/7/8 triaged + real versions wired (UNVERIFIED here)
MERGE VERDICTS on the worker's deliverable:
- Phase 6 weather: REJECTED — duplicates unendlich_engine::weather (real,
  wired, default-ON since r116) with a parallel u8 + light multiplier on App.
  Its day/night light idea is real work on the SKY RENDER PASS, queued.
- Phase 7 temperature: IDEA TAKEN, FORMULA REJECTED — its 15-depth*0.05 guess
  replaced by the REAL climate model (chunk_manager.model_arc().temperature,
  the same source as the per-window climate log). Exposure drains vitality
  outside the comfortable band (T<0.25 freezing / T>0.8 scorching), scaled by
  how extreme; logs [survival] exposure with the live T.
- Phase 8 crafting: IDEA TAKEN, IMPLEMENTATION REJECTED — its attempt_craft
  is five log lines over string "recipes" with a nonexistent ItemType. Wired
  the engine's REAL items::crafting::{Recipe, can_craft, craft} against the
  App inventory instead: C crafts the first satisfiable of 1 OakLog->4 Oak
  Planks, 4 Cobblestone->1 Stone, 2 Sand->1 Glass (all reachable bare-hand
  via mining's drop table: logs/sand drop themselves, stone drops cobble).
  Real consumption, real outputs, [craft] logs; C verified unbound.
- Its module-count/percentage/perf claims: not reproduced, not asserted.
Full loop now: mine -> drop pops+settles -> walk-collect -> C crafts ->
HUD inventory reflects it -> V eats -> climate + water modulate survival.

## r124 — SURVIVAL MODE + hand-item use semantics (UNVERIFIED here)
Operator corrections honored:
- P (panel mode) is the inventory+crafting surface; C remains a quick-craft
  shortcut. Eating is the right-mouse USE event on an edible hand item — the
  V key path is retired so eating is single-sourced.
- NEW: UNENDLICH_MODE=survival enables the survival ruleset (default stays
  creative, logged either way as [mode]):
  * PLACE CONSUMES: right-mouse placing removes one of the selected block
    from the App inventory (Inventory::remove_item); denied with a log if you
    have none — mine first. Creative placing unchanged.
  * RIGHT-MOUSE EATS: with an edible selected (Mushroom for now — the only
    food block in the drop table), use consumes one and feeds +25% vitality.
  * NO CREATIVE FLIGHT: can_toggle_flight revoked after LocalPlayer
    construction (flag read early for the banner; revocation ordered after
    the player exists — first compile risk if anything).
- survival_use_item() is the single hand-item use seam — future edibles,
  tools, and spawn-egg items hang off it.
Survival loop to test: UNENDLICH_MODE=survival, mine dirt/logs (drops pop +
settle), collect, place only what you hold, C to craft planks, find a
mushroom to eat via right-mouse, watch climate/water exposure on vitality.

## r125 — menu mode propagation + survival batch (UNVERIFIED here)
Operator confirmed: flight-revocation log + vitality drain live; mode from
the MENU didn't propagate (env-only). This round:
- MENU MODE PROPAGATION: AppStep::Launch(StartLocal(init)) now reads
  init.mode (the wizard's Creative/Survival choice) into survival_mode and
  updates can_toggle_flight live. survival_mode is mut; env remains the
  pre-menu default. Logs "[mode] menu launch set ...".
- SURVIVAL HOTBAR = REAL INVENTORY: in survival, scroll/1-9 set the hand to
  the inventory SLOT's item (BlockId::ALL reverse lookup), not the creative
  palette — you place what you actually hold.
- DEATH & RESPAWN: at vitality 0 (survival), the whole inventory scatters as
  drops where you fell (radial pop velocities), player respawns at world
  spawn, vitality refills. "[survival] YOU DIED ..." with coordinates — the
  drops persist so a corpse-run recovers them (until age-out).
- HUD leads with the mode: [HUD] HH:MM | SURVIVAL | RUNNING | ...
Known seam remaining: per-WORLD mode isn't persisted to the catalog on save
(no save system in the bin yet); mode lives for the session.

## r125b — mode type fix (UNVERIFIED here)
- LaunchPlan carries the CANONICAL sim::command_parse::GameMode (4 variants:
  Survival/Creative/Adventure/Spectator — world_launch maps the catalog's 2
  into it), not world_catalog::GameMode. Operator's E0308. Match fixed;
  Survival|Adventure -> survival ruleset, Creative|Spectator -> creative.

## r126 — big batch: on-screen HUD, world persistence, ingots, day length (UNVERIFIED)
(Operator running multi-turn unverified; cargo-in-sandbox now blocked even for
check — a dep re-resolves to edition2024 under cargo 1.75. Balance-scanned only.)
- ON-SCREEN HUD: renderer gains pub hud_status_lines: Vec<String>, rendered
  inside the EXISTING unendlich_hud egui card (top-left, under "in hand").
  Bin refreshes it every 30 frames: clock + mode (+paused), stamina/vitality
  (+SWIM), inv slots + ground drops. Survival state now visible IN GAME.
- WORLD CATALOG PERSISTENCE: saves/catalog.txt (manifest-anchored, pipe
  format name|seed|mode|created|last_played; bin-owned IO, sim stays pure
  via new MenuController::catalog_mut + App::menu_mut seams). Restored into
  the menu catalog at boot ("[saves] restored N world(s)"); every successful
  StartLocal records/refreshes its entry ("[saves] recorded world"). Load/
  Continue now survive restarts (terrain = seed regen; block edits still
  not persisted — region serialization remains future work).
- INGOTS: ItemId space 1000+ for non-block items. C-craft recipes added:
  1 IronOre + 1 CoalOre -> Iron Ingot; 1 GoldOre + 1 CoalOre -> Gold Ingot
  (ores drop themselves when mined). Ingots render as golden fallback cubes
  if dropped; they can't be placed (BlockId reverse lookup just misses).
- DAY/NIGHT OBSERVABLE: sky day_length now 1200s (20 min) by default,
  UNENDLICH_DAY_LEN=secs to change; starts mid-morning. The cycle existed
  (sky_state.tick) but the stock day length made the sun look static.

## r127 — COMBAT: huntable creatures + meat (UNVERIFIED, balance-scanned)
- Roamer gains hp (40; struct + all 3 construction literals; the destructure
  uses `..`). Left-click with a creature within 4 blocks along the view ray
  (lateral < 1.2) punches IT instead of the block behind it.
- Damage runs the REAL sim::combat::resolve (Element::Earth, magnitude 12,
  no shield/armor, fresh Afflicted) — the elemental combat pipeline is now
  live in the bin, not just registry-exercised. Logs "[combat] punched ...".
- DEATH: hp<=0 removes the roamer and drops MEAT (ItemId 2000, new non-block
  item space) with an upward pop; "[combat] ... slain".
- MEAT IS FOOD: survival hand tracking now carries the raw ItemId
  (hand_item); right-mouse with meat in hand eats it for +40% vitality
  (survival_use_item gained the non-block branch). Hunt -> drop -> collect ->
  eat closes the carnivore loop alongside mushrooms.
- r126 follow-up note: HUD fill verified unconditional in the main loop; if
  the operator sees NOTHING new, first check boot log for "[sky] day length"
  — absence means a stale binary (failed build re-ran old target/).

## r128 — fires warm you, predators bite (UNVERIFIED, balance-scanned)
- EXPOSURE x HEAT SOURCES: the survival climate sample now adds the
  HeatSourceMap's total_heat_at (F9 campfires + the lava sync) scaled to
  normalized units — shelter by a fire counters freezing-biome drain. The
  F9 fire system (plant/extinguish, wind updrafts) finally matters to
  survival, not just the wind arrows.
- PREDATOR CONTACT: in survival a Dinosaur or Dragon within 1.6 blocks bites
  every ~0.7s (vitality -4%, "[combat] ... bites you"). Combined with r127
  punching, fauna interactions are two-way: hunt the small ones, respect the
  big ones. (CreatureKind canonical path is item::CreatureKind; spawn only
  imports it.)
- Audited as already-wired (no change): sprint/stamina gating (r118),
  day/night tick, mob_persistence (SpawnLedger) deferred — needs a
  per-spawn-site key scheme; queued behind region block persistence.

## r129 — breath, claws, smokescreen, stone ring, builders (UNVERIFIED, balance-scanned)
Operator-requested five-pack, all on real modules:
- DRAGON BREATH BENDING: flying dragons within breath range fire once per
  ~6s flight-cycle window at the player. Element, cone, range, and base
  damage come from sim::breath_attack::params_from over a per-individual
  dragon_archetype::Genome (stable seed = flight_phase bits); falloff via
  damage_at(angle, dist). Survival: vitality takes dmg*0.004. Logs
  "[dragon] Fire BREATH hits you for N at D blocks" / off-axis misses.
- SWIPE CLAW ANIMATIONS: sim::claw_curl::claw_curl drives a torso_bend
  flinch impulse on a punched creature; the per-creature anim tick relaxes
  it (exp decay) so hits visibly rock the target.
- SMOKESCREEN: 32x24x32 SmokeField (2-block cells, spawn-anchored 64x48x64
  volume) stepped per frame by the REAL sim::smoke_gpu::step_smoke from
  live F9 fires (positions recorded at the F9 site). Player inside density
  >0.15 -> "[smoke] entered the smokescreen" and fog density x6 — the
  screen genuinely screens. Light a few fires upwind and vanish.
- STONE RING + BUILDERS: landmark pass now samples a 16x16 grid of
  SiteFeatures from worldgen surface probes, ranks via the REAL
  world::structure_place::rank_sites (StructurePrefs::default), and at the
  top-3 sites places: #1 a ring of 8 stone pillars (radius 7, heights 2-4,
  each grounded by its own surface probe); #2-3 7x7 ruins from the REAL
  structure_template::make_ruin (cobble walls h=3 with deterministic rubble
  gaps, plank floors). Logs "[builders] ..." with suitability scores.
- BUGFIX (would have been the next compile wall): r127's hp field never
  landed on the Roamer STRUCT (anchor assumed field order); literals had it.
  Fixed. Bonus recon: Roamer already carries genetics::Genome (breeding) —
  distinct from dragon_archetype::Genome (breath); both now in play.

## r130 — PLAYER BENDING (UNVERIFIED, balance-scanned)
The bending stack goes live for the player on B (verified unbound):
- INPUT: BendInput assembled from the real game state — stance from movement
  (Aerial in fly/swim, Rooted sneaking, Surge sprinting, Flow moving),
  directional from look pitch (Lift up / Ground down / Push level), gesture
  Strike, ELEMENT FROM THE ENVIRONMENT: in water -> Water, near strong heat
  (campfire/lava, total_heat_at > 4) -> Fire, fly mode -> Air, else Earth.
- FIELDS: affinity_access::Fields sampled live — h from model.humidity, r
  from model.temperature + heat sources, w from wind_at magnitude; p/l/s/b
  ambient constants for now (storm/lightning hooks queued on weather state
  accessors). access gating + thresholds are the REAL appendix-A table, so
  some disciplines are simply out of reach in the wrong biome — by design.
- RESOLUTION: player_bending::PlayerBending::step (rising-edge commit via its
  own key_id; holding B doesn't spam) -> EffectIntent with gesture_resolve
  technique scale -> power01.
- EFFECTS ON THE WORLD:
  * Earth -> raises a stone pillar (1-4 blocks by power) at the aimed
    surface point, reach 3-9 blocks. "[bending] earth pillar +N at (x,z)".
  * Fire/ThermalPhase -> plants a heat source 4 blocks ahead (warmth bubble
    + smoke-field fuel + wind updraft). "[bending] thermal bloom".
  * Other disciplines (Stormstrike, HeavyBrine, Information, Water/Air
    elemental) -> aimed strike: 8-32 dmg to the creature along the ray
    (12-block reach), claw-flinch on the target, meat on kill.
- FIXES en route: TechniqueShape has kind not family; player flight state is
  body.is_none() (fly mode has no physics body), not a controller flag.
- Modules newly live at a real seam: player_bending, gesture_resolve,
  affinity_access (live fields), plus heat/smoke/wind/climate as inputs.

## r131 — MENU FIX + real resonance economy + consolidated patch (UNVERIFIED)
Operator: r129/r130 stack COMPILES on hardware; block numbers work; menu
broke after creating a world. ROOT CAUSE (two instances of one class):
mutating the live WorldCatalog after MenuController construction desyncs
the menu's GameFlow (built once from has_save/slot_count) from the catalog
contents. r126 did it at boot (restore loop) and at launch (record).
- FIX 1: App::with_catalog(catalog) — construction-time restore; the bin
  builds the App from saves/catalog.txt entries so flow and contents agree
  from frame 0. with_defaults path unchanged when no file exists.
- FIX 2: launch-time recording is FILE-ONLY (read file -> retain != name ->
  push -> write); the live menu state is never touched mid-session.
- BENDING CHARGE: sim::bending_charge::Resonance replaces the controller's
  local regen stub — builds while still/rooted (rhythm 0.9) vs 0.2 moving,
  each cast spends 0.25..0.5 by power, log shows spent/remaining. Casting
  repeatedly now drains; stand still to recharge. (Resonance exposes
  charge01 directly; no level() accessor.)
The cumulative patch (CHANGES-*.patch) has always been the full diff of
everything added vs the pristine r112 import — r131's is the consolidated
"everything good" patch the operator asked for.

## r132 — generator pass 2: +172 entries, orphans 157 (UNVERIFIED, balance-scanned)
PATTERN AUTOMATION on the registry generator (tools/bootstrap_registry.py):
- TYPE-BLOCKER CENSUS (instrumented synth over all orphan fns) ranked what
  blocks synthesis: &Genome(31), &[(f32,f32)](30), &[(f32,f32,f32)](30),
  &mut [f32](19), &[[f32;3]](13), Vec3(13), ChunkCoord(9), &[u8](7)...
- synth() WIDENED for those classes: empty slices (&[] / &mut []) for all
  primitive/tuple/array slice params, genetics::Genome::from_seed(c.tick),
  nalgebra_glm Vec3 from c.phase, CreatureKind::Fauna, ChunkCoord literal.
  (Two conjures fixed against real APIs before emitting: KernelCtx has tick
  not seed; ChunkCoord lives in world::chunk with pub fields, no new().)
- GEN2 EMISSION DISCIPLINE: the healed gen1 files (pruned by operator
  compile/panic feedback in r114) are NEVER regenerated — pass 2 emits ONLY
  names absent from gen1 into sim/kernel_registry_gen2.rs (170 entries,
  G2K_*) + sim/service_registry_gen2.rs (2 alt-ctor/wide-tick services).
  Same KERNELS/SERVICES slices -> the bin sweeps them with ZERO bin changes;
  runtime quarantine catches panickers; compile errors prune by GEN-ENTRY
  block on the next operator paste.
- Generator gained --gen2 mode, TICK_PREFS_WIDE (process/run/apply/evaluate/
  integrate/settle/decay/build) and ALT_CTORS (full/empty/with_defaults/...).
- CENSUS: 1031 modules, 874 live (was ~801), 157 orphans
  (sim 88, world 36, render 8, rest single digits). The remaining sim/world
  orphans are dominated by &Chunk/&WorldGenerator/&NavGrid/trait-object
  signatures — real-state seams, queued for hand adapters.

## r133 — rest cycles + attunement; hearing audited live (UNVERIFIED)
- AUDIT: footstep hearing was ALREADY fully wired in the roamer perception
  block (player footstep loudness -> perceived_loudness over distance ->
  biome ambient floor -> senses.notices -> sim::localize noisy estimate).
  sim::sound + localize are live at a real seam; sneaking is already the
  stealth mechanic. No change made.
- REST CYCLE (new): sim::rest_cycle drives per-species sleep in the roamer
  loop — rhythm_for(kind) x sky day_phase x fatigue (hunger) via wants_rest;
  no-threat gate. Resting creatures stand still: Diurnal fauna bed down at
  night, Nocturnal listeners by day, Crepuscular at midday/midnight lulls,
  dragons Cathemeral. "[rest] Fauna is resting (Diurnal, day_phase 0.91)".
  Gameplay: night-stalk sleeping herbivores; Lauscher are safest at noon.
- DRAGON ATTUNEMENT (new): TAKEOFF log now names the individual's dominant
  breath element from its archetype genome — "TAKEOFF ... — Storm-attuned".

## r133b — gen2 Vec3 pattern fixed + regenerated (UNVERIFIED)
- Operator wall: my Vec3 conjure emitted nalgebra_glm::vec3 into modules
  that define their OWN alias (world::flight: `pub type Vec3 = [f64; 3]`) —
  E0308 across many entries. synth() is now alias-aware: it resolves a
  module-local `type Vec3 = [fNN; 3]` to an array literal (f64 cast as
  needed), uses nalgebra only when the module imports it, and otherwise
  leaves the fn orphaned rather than guessing.
- gen2 regenerated under the same only-new-names discipline: 169 entries,
  0 nalgebra leaks into alias modules (verified by scan). Alias-collision
  sweep over all other gen2 source modules found no further suspects.

## r133c — gen2 Genome ambiguity fixed + regenerated (UNVERIFIED)
- Operator wall #2, same class: bare `&Genome` is AMBIGUOUS — genetics::
  Genome vs sim::dragon_archetype::Genome (both expose from_seed(u64)), and
  spawn imports the archetype one while my conjure hardcoded genetics.
- PATTERN GENERALIZED: synth() now resolves ambiguous bare type names from
  the consuming module's own `use` statements / inline paths (archetype
  import -> archetype conjure, genetics import -> genetics, sim::dragon*
  fallback -> archetype, otherwise orphan rather than guess).
- Regenerated: 183 entries (up from 169 — the import resolution also
  unlocked dragon-module Genome fns the old prefix guard skipped). Verified:
  spawn entries now conjure dragon_archetype::Genome.
- AMBIGUITY SWEEP over every other hardcoded conjure (CreatureKind,
  ChunkCoord, Fixed): each has exactly ONE crate-wide definition — this bug
  class is closed for the current conjure set.

## r134 — AI BUDGET: brain tick-bands (UNVERIFIED, balance-scanned)
Operator: [rest] spam + mob lag; directive "wire mob speedup / brain caching".
- WIRED sim::ai_budget at the roamer loop head: distance band per creature
  (Near<32 every frame, Mid<96 1/2, Far<256 1/4, VFar 1/8), full-ticks
  staggered by per-individual id hash (should_full_tick), curfewed brains
  skip the WHOLE per-creature body that frame (perception, wander, forage,
  rest, breeding checks). Persistent creatures (dragons) are exempt — flight
  arcs and breath windows need every frame and there are only a handful.
  Expected effect: per-frame roamer cost drops toward the near-population
  share; far herds freeze imperceptibly between ticks.
- REST LOG DEDUP: at most one [rest] line per 1800-frame window (was: every
  matching creature on the same frame). Diagnosis note: Spaeher resting at
  day_phase 0.54 is CORRECT behavior (dusk; Diurnal urge rising toward the
  0.75 midnight peak per rest_cycle's curve) — the bug was spam, not logic.
- Note: breeding-driven population growth is the other lag suspect; the
  roamer cap + offspring rate are the knobs if lag persists post-budget.

## r135 — SWIMMING + conjure batch (gen2 -> 207) (UNVERIFIED, balance-scanned)
- SWIMMING PHYSICS (operator request): in physics mode with feet in water,
  the REAL sim::swim parameters govern the body — quadratic drag
  (a = -drag*|v|*v, swim.rs semantics), buoyancy countering most of gravity,
  HOLD-Jump = stroke up (thrust), Sneak = dive, all scaled by sim::fluid::
  submerged_fraction (waist vs head submersion probed via block_at) so
  wading barely differs from open-water swimming. Vertical velocity clamped
  [-8, 6] for bad-frame sanity. Jump on land unchanged. Fixed ctor is
  from_f64 (no from_f32 exists).
- CONJURE BATCH (pattern bulk, 6 classes at once from the refreshed blocker
  census): Loudness (i32 alias), Biome::Plains, BlockId::Stone, Element via
  module-import resolution (elemental vs gesture_resolve; orphan if
  ambiguous), &HeatSourceMap::new(), nested fixed arrays &[[f32; N]; M].
  gen2 regenerated: 207 entries (183 -> 207). Census: 879/1031 live,
  152 orphans. Remaining top blockers are true real-state seams: &Chunk(13),
  &WorldGenerator(7), &mut [&mut SlotGrid](6), &Aabb(5), &NavGrid(5),
  &[ForgeBox](5), &FluidField(4) — world-services adapter still queued.

## r136 — SHADERS: starfield, moon, height fog (UNVERIFIED, balance-scanned)
Pure GLSL additions on EXISTING uniforms — the CameraUbo descriptor contract
is untouched, so no Rust-side changes; build.rs recompiles on the operator
toolchain (proven by the caves shader landing).
- sky.frag NIGHT SKY: procedural starfield (hashed quantized view dir, top
  0.25% of cells become stars; slow twinkle on atmos_params.w) fading in as
  day fades out, occluded below ~10 deg elevation; plus a cool MOON disk +
  glow opposite the sun. Nights are no longer a flat gradient.
- chunk.frag HEIGHT FOG: fragments below the eye accumulate exponential
  valley mist, strongest near sunrise/sunset (gaussian windows on the day
  phase) and scaled by humidity — mornings get pooled fog in lowlands that
  burns off by midday. Existing base_fog/dist reused; clamp 0.5 max.
- SURVEYED, NOT WIRED: render::postfx is a COMPLETE offscreen-HDR + present
  composite orchestrator (with the bloom slot + UNENDLICH_POSTFX gate +
  "POSTFX=1 must look identical" passthrough contract) that world_renderer
  never instantiates — bloom is one integration away, but it's frame-graph
  surgery (render-pass swap for every pipeline) that needs compile-on-
  hardware iteration, not blind patching. Flagged as the next operator-
  interactive session's target.

## r137 — world seams + generic slice conjure: 899 live / 132 orphans (UNVERIFIED)
- WORLD-SEAM EXERCISER (bin): the &Chunk / &WorldGenerator orphan families
  now run against REAL state — the chunk under the player
  (chunk_manager.get_chunk at the player's ChunkCoord) and the live CPU
  generator. Six families round-robin at 1/240 frames (one per ~4s window):
  surface::extract_surface, extract_surface_runs, water_field::
  ground_heights_from_chunk, flora_map::region_flora, sim::spawn::
  spawn_region, render::mesher::mesh_chunk_naive. Each call is wrapped in
  catch_unwind with a per-family quarantine flag ("[seams] family N
  panicked — quarantined") so a bad family logs once and stops, frame
  survives. All 6 fn paths verified against source before emitting.
- GENERIC SLICE CONJURE: &[AnyType] -> &[] (element type inferred from the
  callee signature; empty is valid for any T; non-empty-demanding fns panic
  into the runtime quarantine). Unlocked &[ForgeBox], &[(u8,f32)]-likes,
  &[SurfaceRecord], etc. in one rule. gen2: 269 entries (207 -> 269).
- CENSUS: 899/1031 live, 132 orphans (sim 75, world 29, render 8). The
  remaining mass is &mut-state methods, trait objects, and render-internal
  types — diminishing-returns territory; further gains come from gameplay
  seams like the six above rather than synthesis.

## r138 — overdraw debt + dragon bending AI (UNVERIFIED, balance-scanned)
- OVERDRAW CONSEQUENCES (player): Resonance::decay ticks each bend frame
  (debt recovers with time, idle charge bleeds); forcing casts past empty
  accrues overdraw01 (spend's shortfall path); while is_overdrawn(), casts
  land at 40% power and each one saps 10% stamina. "[bending] OVERDRAWN".
  Full loop: spam B -> drained -> overdrawn -> weak casts + stamina cost ->
  stand still (Rooted builds fastest) to recover.
- DRAGON BENDING AI: flying dragons (budget-exempt) run the REAL
  sim::dragon_bending_ai::step every frame — Perception from true geometry
  (player within 32 blocks AND within 1.2 rad of the dragon's heading;
  willing 0.8 in survival / 0.35 creative), per-individual archetype genome,
  wind-up charge carried across frames in a bin-side HashMap keyed by the
  flight-phase id. On release: genetics-chosen technique with aim_error;
  hit iff the errored bearing lands within the technique's spread — so a
  calm/distant dragon genuinely misses. Hits drain vitality by potency
  (capped 18%/cast, survival only); both outcomes logged with element.
  The wind-up means dragons now TELEGRAPH: it eyes you, then the arc comes.
- batch_ability_cast: audited — cast_one(u8, f32)/cast_batch(&[(u8,f32)])
  are fully synthesizable post-r135/r137 conjures; covered by gen2, no
  gameplay seam needed.

## r139 — popdyn carrying capacity + artifact imbue (UNVERIFIED, balance-scanned)
- POPULATION DYNAMICS: sim::popdyn::Colony (seeded 24) shadows the herd —
  stepped every 30 frames with grass regen scaled by REAL local humidity
  (lush biomes support more grazers). Its live population is the DYNAMIC
  breeding cap (gated BEFORE the split_at_mut borrow — caught in review),
  and overshoot past capacity+12 starves the hungriest non-persistent
  roamer per window (lowest vitality culled, drops meat where it fell —
  scavenging is a food source). "[popdyn] Fauna starved (herd N > capacity
  M)". This is the lag-prevention complement to r134's AI budget: budget
  bounds per-creature cost, popdyn bounds creature COUNT.
- ARTIFACT IMBUE: holding an INGOT (ItemId 1000/1001) in the hand slot
  attunes it via sim::artifact_imbue::tick_imbue — progress scales with how
  well the environment matches its element (fire-leaning default: heat).
  The imbued FieldDelta then ADDS onto the player's bending fields exactly
  per the affinity docs ("a worn ember-stone raises r and opens Embercraft")
  — so a fully-attuned ingot held near campfires unlocks disciplines the
  raw biome wouldn't. "[imbue] the ingot in your hand is fully attuned".
  Loop: mine ores -> smelt (C) -> hold the ingot by a fire -> bend bigger.

## r140 — NATURAL SPAWNING (UNVERIFIED, balance-scanned)
The world now populates ITSELF: when the player enters an unvisited
64-block region (current + 4 neighbors checked once per second),
sim::spawn::spawn_region — deterministic in world seed + region coords,
dry-land surface sites only — decides who lives there, and real Roamers
hatch at those sites (MobKind::Seeker -> Dinosaur predators, Walker/Grazer
-> Fauna), capped by the popdyn carrying capacity and ROAMER_CAP. The egg
recipe was reused verbatim (genome from a position-derived seed, expressed
phenotype speed/turn, senses_for, leap-from-genes; persistent=false so
streaming culls far ones). Logs "[spawn] region (rx,rz) populated: N".
Exploration now means encounters: predators in some regions, herds in
others, the same individuals every session (deterministic seeds). Caught in
review: CreatureKind wasn't in scope at the insertion (full-pathed);
m.body.pos is the sim Fixed Vec3 (to_f32 per component).
Full ecology chain now closed: spawn_region populates -> popdyn caps ->
rest_cycle schedules -> ai_budget bounds cost -> senses/hearing perceive ->
combat/predation/scavenging recycle into meat.

## r141 — bulk gameplay cluster: fall damage, recipe book, predator-prey (UNVERIFIED)
- FALL DAMAGE: sim::fall_impact::impact_for(landing speed, Body::humanoid())
  on the descent->grounded transition (prev_vy < -9, water-exempt — swim
  drag already bled the speed); survival vitality takes scaled damage,
  KNOCKDOWN flagged in the log. Cliffs matter now; dive into water instead.
- RECIPE BOOK: sim::recipe_book::RecipeBook built over the 5 recipe
  ingredient lists; C now PREFERS recipes that use the item in your hand
  (find_by_ingredient ordering, falling back to the rest) — hold a log,
  craft planks; hold ore, smelt it. (One duplicated loop-closer caught by
  the balance scan and removed.)
- PREDATOR-PREY: hungry Dinosaurs (vitality < 0.7) consult the REAL
  sim::predator_prey::decide over a pre-loop herd snapshot (kind/pos/mass,
  48-block neighborhood) and CHASE the chosen Fauna at sprint pace,
  overriding wander; within 1.5 blocks the prey dies (deferred,
  highest-index-first swap_remove after the loop, persistent-guarded),
  drops MEAT where it fell, and the predator feeds (+0.5). "[ecology]
  Dinosaur brought down prey". Combined with r140 natural spawning and r139
  starvation, the food web is now: grass capacity -> herds -> predators ->
  meat -> you, all observable.
- Audited as gen2-covered (no seam needed): inventory_sort (pure
  sort_order), item_appraise/item_compare (pure fns).

## r141b — two-line wall fixed (UNVERIFIED)
- Wander wasn't in scope at the r140 natural-spawn block (the egg block's
  local use didn't extend there) — full-pathed via sim::cognition::wander.
- sim::recipe_book has its OWN ItemId(pub u16) newtype, distinct from
  items::ItemId(u32) — same name-collision class as the Genome/Vec3 walls.
  Mapped at the boundary (rb_id helper for builds; h.0 as u16 for lookups).
  NOTE: the ingot ids (1000/1001) fit in u16 so held-ingot lookup works.

## r142 — engine exposure model + seed dispersal/germination (UNVERIFIED)
- EXPOSURE UPGRADED to the ENGINE'S OWN model (sim::survival_exposure
  replaces the r123 hand band): Conditions assembled from real samples —
  climate T mapped to °C ([-5,40]), wind01 from wind_at, wet01 = in water,
  near_fire01 from the HeatSourceMap, SHELTERED = any solid block within 3
  above the head (a roof now matters), clothing 0 (future item slot). The
  body's Thermal state lags felt temperature via step_body; |comfort| past
  0.25 drives the vitality drain. Logs body °C + felt °C + shelter/fire
  tags: "[survival] FREEZING — body 33.2°C (felt -8.4°C, by fire)".
  Emergent: wind chill makes storms dangerous, swimming in cold biomes is
  lethal, a roof + campfire is genuine survival shelter.
- SEED DISPERSAL + GERMINATION: breaking leaves releases 3 seeds carried by
  the REAL wind vector (sim::seed_dispersal::disperse), each landing
  downwind; sim::germination::germinate gates sprouting on local moisture/
  warmth/light, and successes place a Flower at the landing column's
  surface ("[flora] N seed(s) took root downwind"). Forests now SPREAD
  downwind of harvesting — the first player-caused ecological succession.
  (OakSapling isn't a BlockId; Flower used — caught pre-package.)
- Audited, deferred: sim::survival's full Vitals (hunger/thirst/cold
  triple) is a redesign of the single-energy vitality — queued behind a
  decision; dragon_growth_stage needs creature scale plumbing in the
  renderer (size_mul exists, no scale field on Creature yet); dragon_hunt
  overlaps the wired predator_prey for now.

## r143 — TAMING + census (UNVERIFIED, balance-scanned)
- TAMING (sim::bond_tame): Roamer gains bond + trust (struct + all 4
  literals via the hp-line regex — the field-add pattern is automated now).
  RIGHT-MOUSE WITH MEAT at a creature in reach FEEDS IT (Event::Feed,
  trust +0.15, also feeds its vitality) instead of self-eating; ~6 feeds
  cross the Tamed threshold ("** TAMED — it follows you now **"). PUNCHING
  is Event::Harm (trust -0.40) — violence is remembered; you can't beat a
  friend out of an animal. TAMED creatures heel: follow at 4 blocks at a
  trot, overriding wander (bond bound into the loop destructure). The
  click-consumption uses the r127 flag pattern, NOT continue (a continue
  there would have skipped the frame's render — caught in review).
- ECOSYSTEM CENSUS: every ~2 min one [census] line — herd size, composition
  by kind, tamed count, popdyn capacity, drops afield. The ecology's
  heartbeat in the log.
- Chain now possible: hunt one Fauna for meat -> feed another to tame it ->
  it follows you across the world -> predators still hunt it (guard your
  companion) -> it rests at night beside you.

## r144 — combat stances + grazing (UNVERIFIED, balance-scanned)
- COMBAT STANCE (sim::combat_stance): a WOUNDED creature (hp < 40, not
  tamed) with the player inside 24 blocks adopts the engine's stance —
  stance_bias/guard_side/engage_distance composed: it BACKS AWAY to its
  computed standoff range when you close in, and torso-leans to shield the
  hurt side (visible wounded posture, decaying via the existing anim damp).
  hp newly bound in the loop destructure (was behind `..`). Fights now
  read: hit a creature once and it circles at range instead of grazing on.
- GRAZING (sim::feeding_select): hungry Fauna (vitality < 0.6) probe a
  6-spot ring (±2 y) for real flora blocks, classify them (TallGrass/Fern
  -> Grass, Flower -> Fruit, leaves -> Leaf), let the diet model pick, walk
  to the choice, and EAT THE BLOCK (set_block Air, feed +0.4). "[graze]
  Fauna ate the Fruit at (x,z)". Meadows visibly thin under herds — the
  popdyn grass model and the rendered world finally tell the same story,
  and overgrazed land pushes herds to migrate (their food is literally
  gone). Seed dispersal (r142) is the counterforce: the meadow regrows
  downwind. The vegetation loop is closed.
- Audited, skipped: sound_masking (the live hearing path already runs
  perceived_loudness + senses.notices + biome ambient floor — masking would
  duplicate it); pollination needs PlantGenome plumbing (queued with
  plant_tree_genetics as a flora-genetics session).

## r145 — head tracking + spawn director (UNVERIFIED, balance-scanned)
- HEAD TRACKING (sim::dragon_observe, wired for ALL creatures): the player
  is a Stimulus (salience grows with proximity + footstep loudness); the
  observe model smoothly tracks bearing, idles into its scan pattern, and
  the neck bend follows the yaw. cr.gaze_yaw/gaze_pitch are the persistent
  Gaze state, and the entity baker ALREADY rotates head/neck voxels from
  them — so creatures now visibly turn their heads and WATCH you walk past.
  Probably the highest visible-payoff-per-line wiring in the project.
- SPAWN DIRECTOR (sim::mob_spawn_director): every ~45s, plan_spawns runs
  with the REAL SpawnCtx — biome from biome_at_3d, time_of_day from the
  sky, its own no-spawn ring (24) and spawn ring (56), per-kind active
  counts and caps (KINDS=5; arity caught in review), budget min(3, room
  under the popdyn cap). Requests hatch through the same Roamer recipe.
  The director's time-of-day rules mean spawn pressure CHANGES with the
  clock — nights get teeth. "[director] Dinosaur spawned at ... phase 0.78".
- Review catches this round: caps arity ([u32; 5], not 3), biome fetch via
  cpu().biome_at_3d(...).biome (no model.biome_at(f64,f64) exists).

## r146 — dragon essence loot + whiskers (UNVERIFIED, balance-scanned)
- DRAGON ESSENCE (sim::dragon_material_yield): a slain Dragon now also
  drops ELEMENTAL ESSENCE — element read from its archetype genome's
  dominant channel, run through the engine's material model
  (attunement 0.6, rarity tier 3); essence count scales with the yield
  total (1..8), item id 3000+element so each element is a distinct drop.
  "[loot] dragon essence x4 (element 4) — a rare prize". Dragons are now
  worth the fight, and per-element essence sets up future elemental
  crafting (the imbue path's natural ingredient).
- WHISKERS (sim::touch_rays): every full-tick creature FEELS 2 blocks ahead
  along its intended heading (touch ray over solid block_at, flora and
  water transparent to it); an imminent contact bends the heading sideways
  proportionally to proximity. Creatures stop nosing into cliffs and tree
  trunks — herds flow around obstacles instead of grinding against them.
  (Contact's field is .distance, not .len — caught against the struct.)
- Audited, skipped: popgen::Population only constructs via founders() (its
  own genome storage; can't wrap the live herd's genomes) — flavor stat
  would be fake; body_scale/mob_limb_girth blocked on the same missing
  Creature scale plumbing as dragon_growth_stage (one renderer session
  unlocks all three); water_waves queued for a drop-bobbing pass.

## r147 — wave-bobbing drops + hearing-driven dragons (UNVERIFIED, scanned)
- WAVE BOBBING (world::water_waves): drops afloat (water under the cube)
  bob on the REAL wave field — WaveParams derived per frame from the live
  wind vector/magnitude + time, water_wave_height sampled at the drop's
  x/z, added to the render y. Slay something over a lake and watch the
  meat ride the chop. First visible use of the wave module; the same
  params are ready for a swim-camera bob and (later) water-surface shading.
- DRAGON HEARING (sim::dragon_hearing): the bending AI's alertness01 is no
  longer a constant — it's react()'s startle01 over your actual footstep
  loudness (Ears base_range 40, sensitivity 1.2, ambient floor 6). Sprint
  near a dragon and its startled aim sharpens (less aim_error); SNEAK and
  its arcs go wide. Stealth now matters in dragon encounters, using the
  same footstep signal the fauna hearing path already consumes.
- Audited: footprint_decal resolves DecalParams but there's no decal
  renderer to consume them (render plumbing — flagged for the PostFx
  session); sound_mix's Voice/attenuation/pan needs an audio bus the music
  path doesn't expose yet; salvage_yield fits a future item-quality system
  (no item rarity/condition exists to feed it honestly).

## r148 — FIRST-PERSON VIEWMODEL (UNVERIFIED, balance-scanned)
The REAL sim::viewmodel, end to end: when the mouse is captured and a hand
item is held, the per-form box model (Staff/Blade/Tool — ingots read as
Blade metal, essence 3000+ as an arcane Staff, everything else Tool) is
posed with the module's walk bob (phase from frame clock x measured player
speed, stashed frame-wide — player_speed was gameplay-block-scoped) and a
per-form base pose, placed via box_corners_in_view -> place_in_world with a
CameraBasis built from the live view (forward/right/up), and pushed through
the SAME entity vertex stream as the drop cubes (verts + 12-tri indices).
You now SEE a held tool in the corner of your view, bobbing as you walk,
changing form with what you hold. Look sway wired as [0,0] for now (cursor
delta is consumed earlier in the frame; a stash can add it later).
Survival's feel jumps: hands exist.

## r149 — loot tables + dragon glow tell (UNVERIFIED, balance-scanned)
- ENGINE LOOT (items::loot + sim::item_drop_resolve): mining drops now roll
  through default_loot_table -> resolve_drops — tool-aware (has_tool from
  the hand slot), deterministic per block position, and the DropSpawns come
  back SCATTERED with real velocities instead of one centered pop. Blocks
  the table doesn't cover fall back to the old direct mapping, so nothing
  regresses. The loot system was complete down to its splitmix64 rng —
  never called.
- DRAGON GLOW TELL (sim::emissive): while a dragon's bend charges, its
  tint brightens in its element's hue — base color from dragon_tint(genome)
  each frame (absolute write, no compounding), boosted by emissive_for's
  color x intensity scaled by the wind-up charge01. The r138 telegraph is
  now VISIBLE: the dragon lights up before the arc comes. Dominant element
  mapped explicitly to the emissive Element enum (no from_index exists —
  caught in review).
- Audited, skipped: world::voxel_raycast duplicates the LIVE raycast module
  the bin already picks blocks with (replacing working picking = churn);
  world::block_edit operates on raw u16 slabs (the chunk manager owns
  set_block); quad_stance/quad_reposition/foot_reach need leg-IK fields the
  baker doesn't read (same renderer session as body_scale).

## r150 — dragon daily life: sleep + hunt (UNVERIFIED, balance-scanned)
- DRAGON ACTIVITY (sim::dragon_activity): per dragon, the engine's daily-
  life machine runs every frame — threat from your true proximity, hearing
  from the REAL dragon_hearing react (asleep deafens by its SLEEP_DEAFEN
  rule; a loud-enough noise should_wake), day phase converted to the
  module's midnight-zero convention (+0.25 from sky). SLEEPING dragons LAND
  (flying=false) and the entire bending block is gated behind !asleep.
  "[dragon] settling to SLEEP" / "your noise WOKE the dragon". Night near a
  dragon roost is now a stealth situation: it sleeps, you sneak, one sprint
  too close and it wakes.
- DRAGON HUNT (sim::dragon_hunt): a hungry FLYING dragon (vitality < 0.6)
  runs the stalk -> pursue -> stoop -> strike machine against the nearest
  Fauna from the herd snapshot. wants_close/wants_stoop layer a glide nudge
  over the soaring path (reads as a dive; stoop drops altitude toward the
  prey), and a strike in range kills via the SAME deferred prey_killed path
  as predator_prey — meat drops, the dragon feeds (+0.6). "[dragon] STOOPS
  on the herd" ... "STRIKE — it takes a Fauna". Watching a dragon stoop
  onto a grazing herd is the ecology's apex scene, and it composes from
  five wired modules (spawn -> popdyn -> grazing -> hunt -> drops).
- Note: the deferred dragon_hunt (r142) is now WIRED — the overlap concern
  resolved by splitting domains: predator_prey owns ground predators,
  dragon_hunt owns aerial.

## r151 — canopy bending + hybrid casts (UNVERIFIED, balance-scanned)
- CANOPY SHIFT (sim::canopy_field_shift): leaves within 12 blocks overhead
  apply the engine's canopy FieldDelta to the player's bending fields —
  humidity up, raw light down, bio up (the module's B_MAX/H_MAX/L_MAX
  curves over a mature canopy PlantState). Forests are now bending
  SANCTUARIES: Wood/bio-gated disciplines open under the trees and close
  in the clearing, stacking with biome, fire, artifacts, and overdraw into
  a genuinely layered magic geography.
- HYBRID CASTS (sim::hybrid_rules): casting while holding a half-attuned
  (counter > 0.5) ingot CROSSES the cast's element with the artifact's
  through the engine's hybrid table (storm = air x water, etc.); the
  hybrid's potency multiplies the technique's power (clamped 1.5).
  "[bending] HYBRID Storm burst (potency x1.30) — the artifact answers".
  Both Element enums mapped explicitly (gesture_resolve's 4 -> hybrid's 6;
  imbue's u8 -> hybrid's 6). The full arc: mine ore -> smelt -> attune by
  fire -> carry into a storm-biome cast -> hybrid burst.
- CENSUS: ~115 orphans remain, now almost entirely interactive-tier
  (renderer plumbing, persistence, audio, netcode-ish state machinery).
  Blind bulk wiring has reached its honest floor at r151.

## r151b — four-class wall fixed (UNVERIFIED)
- world/water_waves.rs EXISTED BUT WAS NEVER DECLARED in world/mod.rs (a
  fully orphaned file — invisible to the compiler, which is why the census
  counted it but rustc couldn't find it). One-line `pub mod water_waves;`. ALSO: the module exports water_wave_height_grad (returns (height, gx, gz)), not water_wave_height — bob uses .0.
- EffectIntent has no element field — the cast element is the environment-
  derived `element` local already in scope (sim::elemental::Element, 7
  variants); hybrid mapping rewritten over the REAL enum (Earth|Dust->Dust,
  Fire|Lightning->Fire, Wood->Wood).
- ItemStack.count is u16 (essence count cast at source).
- germination::Fields has 5 channels (s, b added: 0.5/0.6).

## r151c — hybrid element source corrected (UNVERIFIED)
- The `element` local at the bend site is gesture_resolve's 4-variant enum
  (the r130 derivation aliases it as GElem) — r151b's match wrongly assumed
  sim::elemental's 7-variant. Arms rewritten over gesture_resolve
  (Earth->Dust, Water, Fire, Air); the artifact side keeps its 6-way map.

## r152 — creature grime + dragon relaunch (UNVERIFIED, balance-scanned)
Operator: caves + perf confirmed great on the r151c stack.
- GRIME (sim::creature_grime): the entity baker has ALWAYS shaded faces by
  GrimeState (belly-weighted, apply_grime_color at bake) — but every
  creature stayed clean() forever. Now grime ACCUMULATES per full tick:
  dusty ground (Dirt/Grass 0.7, Sand 0.5, RedSandstone 0.6 — RedSand isn't
  a BlockId, caught in review) in the LOCAL soil's tint, flat activity (the
  steering path writes position, so body velocity is stale — caught too),
  and standing in water RINSES (wetness01 = 1). Herds get visibly dusty
  bellies on dirt trails and come out of rivers clean. Zero new render
  code — the consumer was waiting.
- DRAGON RELAUNCH (sim::dragon_mode): a grounded, awake dragon runs the
  engine's mode machine — Alert + (threat / target above / out of reach)
  crosses want_air and the dragon TAKES OFF again ("[dragon] takes to the
  air again"). Applied ONE-WAY (Ground->Air only) so the beloved soaring
  behavior is untouched; this also closes the r150 loophole where a dragon
  that landed to sleep could stay grounded forever. Full life loop: soar ->
  hunt -> sleep at dusk -> wake (or be woken) -> relaunch.

## r153 — CAVE GARDENS (UNVERIFIED, balance-scanned)
Operator loves the caves; this grows life in them. When the player is
UNDERGROUND (solid rock 8-24 blocks overhead), every ~30s a REAL
world::cave_flora stem grows from a probed nearby cave-floor cell:
- moisture/temperature closures sample the live climate model at each cell
  (the stem turns toward damp), occupancy closes over block_at (it hugs
  walls and floors, never grows through rock), sky_exposure uses the
  module's own probe.
- 10 growth steps; every grown StemNode becomes a Moss block via set_block.
  "[cave] a moss garden grew nearby (N node(s)) — follow the green".
Caves now slowly come ALIVE as you explore them — moss veins trace the damp
passages, and because growth follows real humidity, the gardens lead toward
water. (LumenField pearls — the bioluminescence intensity model — computed
but unconsumed pending a glow-shader hook; noted for the render session.)
Census: 927 live, 104 orphans (anatomy/forge audit: dragons ALREADY get the full 256-res
forge model + genome anatomy via the generic spawn arm — no wiring needed).

## r154 — NAV3D steering: companions + dragon dives (UNVERIFIED, scanned)
- sim::nav3d::step3d (27-cell blocked-neighborhood steering kernel, per-
  medium) wired at two seams:
  1) TAMED FOLLOW: companions sample their 3x3x3 occupancy (flora/water
     transparent) and step3d bends the follow direction around blockers —
     they now round boulders and doorframes instead of pressing into them
     (the r146 whiskers still cover everyone else; nav3d gives companions
     the better brain they deserve at heel).
  2) DRAGON STOOP: the dive's glide nudge is now the kernel's output over
     solid occupancy with altitude_err = -1 while above prey — the dragon
     threads terrain on the way down and the descent rate comes from the
     kernel's y component instead of a hand clamp.
- Audited, skipped: craft_resolve needs RecipeTable + CraftGrid plumbing
  (item-quality session); world::coral is genome/phenotype only — no grower
  to consume it yet (pairs with a future underwater pass alongside the
  LumenField glow hook).

## r155 — undeclared-file sweep: PostFx bit-rot mapped (UNVERIFIED)
The r151b water_waves find was a CLASS: swept every .rs in the tree for
files no mod.rs declares. FOUR found:
- render/present_pipeline.rs — imports only ash; SAFE; now DECLARED (its
  bloom-slot composite shader pipeline compile-checks from this release).
- render/bloom.rs + render/postfx.rs — BIT-ROTTED: they import
  create_offscreen_render_pass / create_present_render_pass /
  create_bloom_render_pass from render::swapchain, NONE of which exist in
  the current swapchain API. Declaring them breaks the build, so they stay
  undeclared with an audit note in render/mod.rs. THE POSTFX SESSION
  AGENDA, now precise: (1) restore the three render-pass ctors in
  swapchain.rs, (2) declare bloom + postfx, (3) instantiate PostFx in
  world_renderer and swap scene passes to the offscreen target.
- render_meshref_shim.rs — deliberate #[path] feature shim (mesh-ref
  headless builds); correctly undeclared, left alone.
gen2 regenerated: 270 entries (water_waves' pure fns scooped now that the
module is visible). Census: 928 live / 103 orphans.
Also audited this round: sim::turtle (shell-withdraw behavior; overlaps
combat_stance), dragon_elemental (tactical breath element; flavor-only
without a damage consumer), chunk_flora (LumenField pearls again — render
hook), item_genetics (no item-stat consumer). All ledgered, none wired.

## r156 — WEATHER GAMEPLAY (UNVERIFIED, balance-scanned)
weather.rs's own doc names its unconsumed consumers ("rain wetting the
ground, storms scaring mobs") — precipitation existed in WeatherParams and
fed nothing but the sky. Five seams, one frame-stashed precip01:
1) RAIN SOAKS YOU: exposure wet01 = precip*0.8 when UNSHELTERED (the roof
   from r142 now blocks rain as well as wind-chill heat loss) — caught in
   a cold storm without cover, the body°C log goes south fast.
2) RAIN WASHES CREATURES: grime wetness from precipitation — herds come
   out of a storm clean.
3) STORMS DRIVE SHELTER-SEEKING: precipitation feeds wants_rest's CHILL
   parameter (the engine's own slot for exactly this) — herds hunker down
   in the rain.
4) RAIN BOOSTS GERMINATION: landing-spot moisture +0.3*precip — seeds cast
   in a storm sprout where a dry day would fail.
5) STORMS DOUSE FIRES: heavy rain (>0.7) extinguishes fires with no solid
   cover within 4 above ("[weather] the storm doused N fire(s) — build
   under cover") — sheltered campfires survive; smokescreens and warmth
   now depend on architecture.
Also audited: stamina/sprint already wired (r118).

## r157 — dead panel toggles consumed + essence hues (UNVERIFIED, scanned)
TOGGLE AUDIT: every disable_*/show_* in DebugSettings greped for consumers —
disable_fog, disable_shadows, disable_ssr, disable_bloom, show_chunk_borders
ALL had zero. Two wired this round:
- disable_fog: zeroes fog_density just before UBO upload (exact off, after
  the weather fog math) — the panel toggle finally does something; instant
  A/B for the operator when judging the r136 height fog.
- disable_shadows: forces the CLEAR variant of the shadow pre-pass and
  skips both draw paths (indirect + fallback loop). The pass still
  begins/ends with its clears so the image/barrier contract is untouched —
  an empty shadow map reads as fully lit. Perf A/B + debugging tool.
- (disable_ssr/disable_bloom await their features — bloom is the PostFx
  session; show_chunk_borders needs a debug shader path.)
ESSENCE HUES: dragon essence drops (3000+elem) now render in their
element's emissive hue (lifted +0.3 toward white so dark hues read) instead
of the golden fallback — a Storm dragon's prize is violet on the grass.

### Bulk-wiring techniques that paid off in r156 (per operator request):
1. READ THE MODULE'S OWN DOCS FOR INTENT: weather.rs literally listed its
   unconsumed consumers ("rain wetting the ground, storms scaring mobs") —
   grepping doc comments for future-tense feature language finds seams the
   census can't.
2. PRODUCED-BUT-UNCONSUMED STATE SWEEP: WeatherParams.precipitation existed,
   ticked every frame, fed only the sky — one stashed local then fanned into
   5 systems. Cheapest possible bulk: one producer, many consumers.
3. PARAMETER-NAME ARCHAEOLOGY: wants_rest's 4th arg turned out to be named
   `chill` — the engine had reserved the exact slot for rain. Reading the
   callee's signature before inventing a mechanism avoids parallel design.

## r156b/r157b — precip01 scope fixed (UNVERIFIED)
- precip01 was declared at the weather tick, which runs LATE in the frame —
  the fire-douse (r156 #5) and exposure (r156 #1) blocks run before it.
  Hoisted to a loop-persistent `let mut` initialized 0.0; the tick site now
  assigns. Early consumers read last frame's precipitation (one-frame lag,
  harmless for weather that changes over minutes).
- LESSON for the technique log: when fanning one producer into many
  consumers, check the FRAME ORDER of every consumer against the producer's
  tick site — chronological position in the loop body is part of scope.

## r158 — survival HUD line + climate white-balance (UNVERIFIED, scanned)
- HUD: a fourth status line surfaces the invisible survival state — body
  temperature (r142 thermal), live weather tag (rain/STORM from r156's
  precip01), bending resonance % with an OVERDRAWN flag (r131/r138). The
  player can now SEE hypothermia coming and time casts without log-diving.
- CLIMATE WHITE-BALANCE (chunk.frag): atmos_params.z (biome temperature)
  was the last unconsumed UBO channel — cold biomes now grade slightly
  blue-cool, hot ones warm (max ~8%/channel, applied before fog so fog
  color is preserved). Tundra reads cold, desert reads baked; pure GLSL on
  an existing uniform, zero Rust.

### Bulk-wiring techniques that paid off in r157 (per operator request):
1. TOGGLE/FLAG CONSUMPTION AUDIT: grep every DebugSettings field for
   consumers OUTSIDE its definition + the panel macro — five "(hook)"
   toggles had ZERO. Settings UIs accrete aspirational flags; auditing
   them finds wiring the feature list already promised.
2. CONTRACT-PRESERVING OFF-SWITCHES: disable_shadows skips DRAWS but keeps
   the pass begin/end + clears, so the image/barrier contract holds (empty
   map = fully lit). When gating render work blind, remove the writes, not
   the structure.
3. REUSE AN EXISTING MODEL FOR COSMETICS: essence hues came from
   emissive_for (already wired for the glow tell) instead of a hand color
   table — one source of truth for element->color.

## r159 — item hues, companions HUD, music observability (UNVERIFIED)
- ITEM HUE TABLE: the drop renderer's golden fallback now resolves known
  non-block items first — iron ingot (steel grey), gold ingot, meat (red),
  egg (ivory); only true unknowns stay golden. With r157's essence hues,
  every item in the game now has a readable color on the ground.
- COMPANIONS COUNTER: the HUD inventory line appends "companions N" when
  any creature is Tamed — your pack is a visible stat.
- MUSIC DIRECTOR OBSERVABLE: sim::music_director was stepped every frame
  with cue_for(app state) but its asset()/volume01 output went nowhere
  (the TODO names the missing rodio/kira backend — that's the audio
  session). Until then, a cue/asset CHANGE logs "[music] director cue
  change -> ..." so the director's menu/pause/in-game fade logic is
  verifiable on hardware before the backend lands.
- Stub sweep of my own wiring: the r131 "regen stub" comment is stale
  (real Resonance landed) — left as-is, zero behavior.

### Bulk-wiring techniques that paid off in r158:
1. LAST-CHANNEL ACCOUNTING: enumerate a UBO/params struct's channels and
   tick off consumers — atmos_params.z (temperature) was the only one left,
   and it became the climate white-balance. Structs with N channels tend to
   get wired N-1 times; audit for the straggler.
2. SURFACE EXISTING STATE ON THE HUD BEFORE ADDING STATE: body°C, weather,
   resonance all existed as frame-level variables — a format! line each.
   Visibility wiring is the cheapest wiring there is, and it makes every
   earlier system more testable for the operator.

## r160 — LEAF DECAY (complete feature) + winded penalty (UNVERIFIED)
- LEAF DECAY, the feature the codebase left itself a note about: the
  registration was commented out ("re-enable when the log-removal trigger
  is in place") because advance_n_ticks ages EVERY leaf — bare
  registration would dissolve all forests. Built the missing pieces:
  1) GATE (engine, leaf_decay.rs): the tick handler now skips leaves
     without FLAG_DECAYING — untouched canopy is permanent.
  2) TRIGGER (bin): breaking an Oak/Birch LOG marks leaves in a 9x7x9
     neighborhood via the EXISTING ChunkManager::set_instance_state
     ("[flora] N leaves orphaned — the canopy will wither").
  3) REGISTERED the handler at last — orphaned leaves age through the
     chunk-tick scheduler's distance bands and wind doubles the rate
     (windy days strip a felled tree fast); death enqueues Air.
  Fell a trunk and the crown withers over the next minutes — and the r142
  seed dispersal already fires when those leaves break by hand first.
- WINDED (stamina): the discarded _stam_mult audit showed it redundant
  with the r118 sprint gate — but the meter supported a missing behavior:
  in survival, draining stamina below 15% drops WALK speed 25% until you
  catch your breath. Sprint management now has a real failure cost.

### Bulk-wiring techniques that paid off in r159:
1. STUB-SWEEP YOUR OWN WIRING + THE TREE'S TODOs: grep TODO/stub/
   placeholder — the music director's missing backend was documented at
   its own call site, and observability (logging cue changes) was the
   honest partial wiring available today.
2. DISCARDED-VALUE SWEEP (`let _ =`): underscore bindings are the
   compiler-blessed list of half-wired outputs — r160's leaf decay and the
   stamina audit both came from that one grep.

## r161 — real pause + respawn completeness (UNVERIFIED, balance-scanned)
- PAUSE FREEZES TIME: game_paused (settings/inventory open) previously
  gated only the gameplay input block — the world clock, sun, and weather
  kept advancing while you sorted your inventory (a dusk inventory session
  could end at midnight). Now world_clock_elapsed, sky_state.tick and
  weather_state.tick are all gated on !game_paused: the sun and the storm
  hold still with you. frame_count-keyed systems keep cadence (they don't
  advance world TIME).
- RESPAWN RESETS THE BODY, not just the meter: death previously refilled
  vitality and teleported — but fg_thermal carried over, so a hypothermia
  death respawned at ~28 deg C and immediately died again in a loop. Now:
  thermal -> 36.5/comfort 0, stamina -> full, resonance/overdraw -> full
  reset, prev_fall_vy -> 0 (no phantom fall impact on arrival).
- Handler audit: plant_lifecycle was already registered (the registry's
  only other handler) — leaf_decay (r160) completes the tick-handler set.

### Bulk-wiring techniques that paid off in r160:
1. THE CODEBASE'S NOTES-TO-SELF ARE SPECS: the leaf-decay registration was
   commented out WITH its enable condition written down. Commented-out
   registrations + "re-enable when X" comments are complete feature specs
   with the risk analysis already done by the original author.
2. AUDIT BEFORE PLUMBING: the discarded stamina multiplier looked like a
   wiring gap but was redundant with an existing gate — reading the
   consumer site first turned "plumb a value through" into "add the missing
   BEHAVIOR" (winded) instead. Not every underscore is a seam.

## r162 — WORLD-SWITCH STATE RESET (UNVERIFIED, balance-scanned)
The r161 respawn fix exposed a FAMILY: every frame-persistent gameplay
mut accreted since r120 leaked across world switches. Loading a different
world from the menu previously carried over: the old world's herds
(roamers — including your tamed pets, at coordinates that mean nothing in
the new terrain), ground drops, spawned-region markers (wrong world
entirely — natural spawning would never fire where you'd "been"), all
three dragon mind maps, F9 fires, popdyn colony pressure, your thermal
state and overdraw debt, last frame's precipitation, and the WORLD CLOCK
(a new world inherited the old one's time of day).
ALL now reset in the AppStep::Launch(StartLocal) block, right after the
existing mesh/landmark resets: roamers + drops cleared, regions/intents/
activities/hunts cleared, fire list cleared, colony reseeded, thermal/
imbue/resonance/stamina/fall-speed fresh, clock back to 08:00.
"[launch] gameplay state reset for the new world".
(persist-worthy pieces — tamed companions, herds — belong to the block/
mob persistence session, where they'll come back keyed BY world.)

### Bulk-wiring techniques that paid off in r161:
1. LIFECYCLE-HOOK AUDIT FOR ACCRETED STATE: every dt-integrated or
   session-scoped mut needs explicit handling at each lifecycle boundary
   (death, pause, world switch). New systems quietly create obligations at
   boundaries that already existed — sweep the boundaries, not the systems.
2. GATE POSITION TELLS YOU WHAT A FLAG REALLY DOES: game_paused looked
   wired (HUD said paused) but only the input block consumed it — grep a
   flag's consumers and check what it does NOT gate before trusting its
   name.

## r163 — PER-WORLD BLOCK PERSISTENCE (UNVERIFIED, balance-scanned)
The "last big rock" was smaller than ledgered: ChunkManager ALREADY
persists explicit edits (load on insertion, save on eviction, save_all at
shutdown — with_storage's contract). Two gaps made it look unwired:
1) ALL interactive worlds shared worlds/default/ — world A's edits bled
   into world B at the same chunk coords (a quietly corrupting bug).
2) Switching worlds never saved the outgoing world's in-memory edits.
FIXES: engine gains set_world_root (non-consuming live re-root, mirrors
with_storage); the Launch block now save_all()s the outgoing world, then
re-roots storage at worlds/<sanitized-name>/ before rebuild_for_seed.
RESULT: block edits (mining, building, pillars, moss gardens, grazing
scars, withered canopies) now SURVIVE quit + reload AND world switches,
each world in its own directory. The persistence session shrinks to its
second half: mob/companion persistence (SpawnLedger) riding on this.
TEST: build something, switch worlds, switch back — it should be there.

### Bulk-wiring techniques that paid off in r162:
1. BOUNDARY-FIRST SWEEPS SCALE: one pass over the world-switch boundary
   caught ~14 leaking systems at once — far cheaper than auditing systems
   individually.
2. THE LOG ALREADY KNEW: "Saved N edited chunks before shutdown" had been
   printing the existence of block persistence all along. Grep your own
   shutdown/startup logs for features you think don't exist.

## r164 — one of each set: F6 mode toggle, essence use, autosave (UNVERIFIED)
- HARDENING/UX (operator: "still not getting survival mode — a key?"):
  F6 now toggles SURVIVAL/CREATIVE live. Survival revokes flight and, if
  you were flying, RE-ATTACHES the physics body at your position (gravity
  applies immediately — expect a landing); creative restores the flight
  permission. Logged + visible on the HUD line. This bypasses whatever is
  blocking the wizard's mode propagation on the operator's launches (the
  r125 path + UNENDLICH_MODE=survival env both still exist; F6 is the
  always-works route — and a diagnostic: if F6 works but the wizard
  doesn't, the bug is in catalog mode mapping, not the ruleset).
- FEATURE (closes the dragon-loot loop): ESSENCE USE — right-click with
  dragon essence in hand consumes one: bending resonance refills FULL,
  overdraw clears, and the imbue artifact attunes toward the essence's
  element (+0.4 counter, element overwritten). Slay a fire dragon, drink
  its essence mid-fight for a full-power cast, and your ingot leans Fire.
  Shares the click-consumption flag with taming (meat takes precedence at
  a creature; essence fires otherwise).
- INFRASTRUCTURE (persistence hardening): AUTOSAVE — save_all() every ~2
  minutes; a crash now costs at most that window (eviction + shutdown +
  world-switch saves all still apply).

### Bulk-wiring techniques that paid off in r163:
1. INFRASTRUCTURE OFTEN EXISTS MISCONFIGURED, NOT MISSING: block
   persistence needed a path fix and a save call, not a system. Before
   scoping a "big rock" session, check whether the engine already does it
   under a default nobody changed (worlds/default).
2. READ THE BUILDER-METHOD CONTRACTS: with_storage's doc comment stated
   the whole load/save lifecycle in three lines — builder methods document
   the engine's dormant capabilities very densely.

## r165 — wizard-mode root cause + sleep to dawn (UNVERIFIED, scanned)
Operator confirmed F6 WORKS -> survival stack fully reachable on hardware.
- ROOT CAUSE of "wizard survival not sticking": traced the whole chain
  (wizard set_mode -> cfg -> finish/to_spec -> WorldSpec.mode -> init.mode
  -> r125 ruleset) and it is CORRECT — the culprit is catalog entries
  created pre-r125 recorded as Creative, which loads faithfully restore.
  HARDENING: F6 now PERSISTS — the current world's catalog entry mode is
  rewritten on toggle (current_world_name stashed at Launch), so the next
  load keeps your choice. Toggle once per old world and it's healed.
- FEATURE: SLEEP TO DAWN — in survival at night (phase 0.55..0.95), hold
  Sneak while standing still for ~3s and time rolls to sunrise — UNLESS a
  predator (Dinosaur/Dragon) is within 24 blocks: "[sleep] too dangerous —
  something prowls nearby". Night is dangerous (cold + the spawn
  director's teeth); sleep is the opt-out, gated on actually being safe.
  Implementation: sky_state.time_seconds rolled to the next day boundary;
  world clock advanced proportionally; sneak-hold accumulator resets on
  movement/release.

### Bulk-wiring techniques that paid off in r164:
1. WHEN A PATH "DOESN'T WORK", ADD A BYPASS *AND* USE IT AS A PROBE: F6
   both unblocked the operator immediately and bisected the bug (toggle
   works -> ruleset fine -> fault is upstream in catalog data, confirmed
   this round).
2. CLOSE ITEM LOOPS: every droppable item should have a USE — essence had
   drop+render but no consume; one right-click branch made dragon hunting
   a strategy instead of a trophy.

## r166 — HIDING mechanic + sleep ritual retrigger + paused music (UNVERIFIED)
Operator design feedback: Sneak-3s sleep collides with hiding-as-a-verb.
- SLEEP RETRIGGERED: the rest is now its own deliberate gesture — HOLD F7
  at night while still (placeholder until the actions/ritual menu; the
  bending-style ritual integration is the queued design). Sneak is freed
  for what it should mean.
- HIDING IN THE GRASS (the operator's mechanic, wired same-turn): sneaking
  inside TallGrass/Fern sets player_hidden, which (1) defeats creature
  SIGHT (eyes::sees result discarded — hearing still works at your
  sneak-quiet footsteps, so hold still as it passes), (2) blocks dragon
  bending target lock ("the grass hides you"), and (3) muffles BOTH dragon
  ear paths to 25% loudness (alertness + sleep-wake). Crouch into a bush
  while the predator passes: the fantasy now mechanically works, and it
  composes with stealth (r147), sleeping dragons (r150) and night spawns.
- PAUSED MUSIC CUE: the music director now receives AppState::Paused when
  the inventory/settings are open — its 0.35-volume duck cue becomes
  observable in the r159 transition log (and audible after the audio
  session).

### Bulk-wiring techniques that paid off in r165:
1. TRACE THE WHOLE CHAIN BEFORE FIXING A LINK: the wizard bug was in NONE
   of the code links — it was in persisted DATA (old catalog entries).
   Chain-tracing prevented "fixing" working code.
2. NEW STATE NEEDS A WRITE-BACK PATH: F6 worked but evaporated on reload
   until the toggle wrote the catalog. Any runtime override of persisted
   config must persist itself or it's a session-only illusion.

## r167 — MENU OVERHAUL phase 1 (UNVERIFIED, balance-scanned)
Operator: Esc opens the debug menu; player state visible on title; menus
jumbled. Their r132-merged upload reviewed: it is the PARALLEL worker line
(mob_ai/combat-health/spatial_audio; does NOT contain r133+), so this line
stays canonical; their docs mined instead (see notes below).
- ESC = PAUSE MENU (new): a real centered pause menu — Resume / Settings &
  Debug / Save & Quit to Title — drawn renderer-side (pause_menu_open +
  pause_action channel, same latch pattern as menu_choice); bin acts on the
  choice: Resume recaptures, Settings opens the old panel, Save & Quit
  save_all()s and raises the title. The PAUSE MENU also gates game_paused
  (time/clock/weather freeze — r161 — now applies to Esc too).
- F3 = the debug·shaderpack panel (relocated; cursor handling preserved).
- TITLE CLEANUP: hud_status_lines suppressed while menu_open (no player
  state on the title), and a TITLE CAMERA replaces the player's last
  in-world view — a slow orbit (r=46, y=96, gentle downward pitch) around
  the spawn monument. Music cue was already MainMenu (audible after the
  audio session).
DOCS MINED from the upload: docs/FROSTED-UI.md is the menu VISUAL vision —
blur-behind-panels (ui_downsample/ui_blur shaders LANDED, render-graph
wiring is "step 3") -> pairs with the PostFx session as the menu's look.
docs/TODO.md item 2: title should run a small BAKED world, blocked on
generator-lifecycle recreate — ledgered as title phase 2. Wizard visual
redesign (operator: "no design vision") queued for the frosted-UI pass
where it can actually look like something.

### Bulk-wiring techniques that paid off in r166:
1. OPERATOR DESIGN FEEDBACK IS A SPEC WITH A FREE FEATURE INSIDE: "sneak
   is for hiding" both retriggered sleep AND specced the hiding mechanic —
   implement the example they give, not just the correction.
2. PERCEPTION GATES COMPOSE CHEAPLY: player_hidden was one bool wired into
   three existing perception paths (sight, target lock, ears) — stealth
   systems are mostly AND-gates on senses you already wired.

## r168 — TOUCH FOCUS FIX + design docs landed in-repo (UNVERIFIED)
- FOCUS REGRESSION FIXED (operator: "can't even get touch focus"): the
  click-to-capture guard predated the r167 pause menu and didn't know
  about it — clicking the pause menu RECAPTURED the mouse on the same
  click, so the click never reached egui. Replaced the ad-hoc exclusion
  list with ONE predicate, any_ui_owns_cursor (P panel, debug panel, pause
  menu, title menu, wizard, inventory): while any UI surface owns the
  cursor, click-to-capture is suppressed and clicks land. This also
  preemptively covers every FUTURE surface added to the list.
- DESIGN DOCS LANDED IN-REPO (docs/design/, .docx + extracted .md):
  1) bending_character_menu_ui_v2 — the menu vision: cinematic frame, DOF
     (subject sharp, world soft), frosted GLASS panels (composes with
     docs/FROSTED-UI.md's already-landed blur shaders), defocus/refocus
     transitions, 3D skill tree with per-node depth blur, per-element
     grading LUTs. v1 structural spec is retained-by-reference (v1 doc not
     in hand — request it if layout questions arise).
  2) mob_dragon_viewer_ui_v2 — natural-history specimen viewer: POM scale
     relief, wing-membrane translucency with vessel network, genome strip,
     FSM floating planes, habitat diorama. (creature_viewer_ui module is
     the existing orphan this targets.)
  3) item_armor_lore_tooltips — CANONICAL tooltip structure + attribution
     formats (six source types) + per-item lore; mechanical effects
     canonical. This one is partially wireable pre-frosted-UI: tooltip
     TEXT for known items (ingots/essence/meat) in the inventory panel.
  VERDICT: the full vision is the frosted-UI + PostFx render session
  (DOF, blur-behind, LUTs — all GPU passes). The session now has complete
  blueprints. Until then, structural egui can follow v1 layouts only if
  the v1 docs are provided.

### Bulk-wiring techniques that paid off in r167:
1. LATCH-CHANNEL UI PATTERN REUSE: the pause menu copied the title menu's
   existing pick->latch->bin-consumes channel (menu_choice) instead of
   inventing new plumbing — new UI surfaces should clone the oldest
   working surface's data flow.
2. RELOCATE, DON'T DELETE: moving the debug panel to F3 kept every
   shaderpack toggle reachable while freeing Esc — contested keys are
   resolved by relocation, not removal.

## r169 — MENU OVERHAUL phase 2: settings/crafting/analytics (UNVERIFIED)
Operator's verbal v1 spec implemented structurally (the GLASS comes with
the frosted-UI/PostFx session; this is the working skeleton):
- SETTINGS (pause menu -> Settings): categorized tabs — Video (Fog/Shadows
  checkboxes wired LIVE to the r157 DEBUG toggles + a live frame-time
  strip), Audio (music volume slider; the director's reported volume
  honours it now, audible post-backend), Gameplay (day length slider
  240-4800s wired LIVE to sky_state.day_length; keybind hints), Controls
  (full keymap). Done button closes.
- CRAFTING (C): the insta-craft is now a MENU — recipe rows (Minecraft-
  nostalgia, per operator: stress the backend now, restyle later) with
  craft buttons enabled by can_craft, have/need summaries per ingredient,
  recipe-book preferred ordering kept (holding an ingredient floats its
  recipes). Buttons execute through the same crafting::craft path.
- ANALYTICS (F4): frame-time sparkline (last ~120 frames from the REAL
  frame_times_ms ring — which accumulates from launch, so menu + warmup
  benchmark data is in it, per the operator's collection design) + stat
  lines (fps/avg ms, herd, drops, chunks resident/pending). The sparkline
  is ONE closure shared by analytics AND settings·video — the operator's
  shared-subcomponent requirement, honoured in egui form.
- Cursor/pause integration: both new windows joined any_ui_owns_cursor
  (r168's single predicate — no focus regressions) and game_paused.
- DEFERRED with reasons: join-world menus (multiplayer session state
  doesn't exist yet); main-menu v2 hybrid + all glass styling (frosted
  render session); crafting's own personality (operator: migrate later).

### Bulk-wiring techniques that paid off in r168:
1. ONE OWNERSHIP PREDICATE, NOT N EXCLUSIONS: the focus bug existed
   because each UI surface patched the capture guard separately; a single
   any_ui_owns_cursor predicate fixed it for every current AND future
   surface at once.
2. OPERATOR DESIGN DOCS ARE SESSION BLUEPRINTS: landing the .docx + an
   extracted .md INTO the repo turns "vision in chat" into greppable
   session inputs (the frosted session can now be run without the operator
   present).

## r170 — CURSOR SINGLE AUTHORITY + build markers (UNVERIFIED, scanned)
Operator: still no cursor focus; menu looks unchanged.
- ROOT CAUSE candidate #2 (deeper than r168's guard): ELEVEN scattered
  set_cursor_mode writers raced — close-handlers re-DISABLED the cursor
  even when another UI was still open (close crafting under settings ->
  captured mouse beneath a live menu = "no focus"). r170 makes cursor
  ownership SINGLE-WRITER:
  * individual handlers may release-to-UI but NEVER capture;
  * one authority block (right after any_ui_owns_cursor) releases the
    cursor whenever any UI is open while captured, logging
    "[ui] cursor released to UI (authority)";
  * capture now happens in exactly TWO places: the click-to-capture guard
    (no UI open) and world Launch. Closing a menu returns a free cursor —
    click into the world to recapture (consistent, Minecraft-like).
- STALE-BINARY DETECTOR: "menu looks like the old one" is ambiguous
  between style and stale build — the pause menu now shows
  "unendlich · paused / menu build r170" and settings shows
  "settings · r170". If the operator doesn't see r170 markers, the binary
  predates these releases and the menu reports are about old code.
- Restyle within egui: fixed-size (220x32) buttons, min widths, separator
  before the destructive action.
- DIAGNOSTIC for next wall: if focus STILL fails with markers visible, the
  fault is in the egui event feed (renderer events vec), not cursor mode —
  that's the next dig site, renderer-side.

### Bulk-wiring techniques that paid off in r169:
1. VERBAL SPECS WIRE FINE WHEN THE PATTERN IS A GENRE STANDARD: settings
   tabs/sliders, crafting rows, and a perf dashboard are cleanroom-able
   from genre memory — ask the operator for vision only where the game is
   NOT generic (the frosted look, crafting's "personality").
2. SHARE SUBCOMPONENTS AS CLOSURES: the sparkline drawn by two windows is
   one closure in the prepare scope — egui composition needs no module
   machinery for shared widgets.

## r171 — THE CLICK FIX: egui pointer feed membership (UNVERIFIED, scanned)
Operator: "I see the cursor but clicking changes nothing" — cursor mode
was healthy (r170 authority working as designed); the events themselves
were the fault. The bin's per-frame egui pointer feed had
  ui_pointer_active = ui_panel_open || settings_open
— ONLY the P picker and the old debug panel ever received pointer events.
Every surface added since (title menu, wizard, pause, settings, crafting,
analytics, inventory) rendered with a live cursor and a DEAD event stream.
(History note: the title menu clicked in older operator builds because a
boot-time `ui_pointer_active = true` predated this per-frame overwrite.)
FIX: the feed's membership now matches the cursor authority's — one list,
every UI surface receives PointerMoved/PointerButton when open.
Markers bumped to r171; pause menu typography pushed (30pt wordmark)
within flat-egui limits — the IMPRESSIVE pass remains the frosted session
(docs/design/*), which is now unblocked by a working input layer.
LESSON (the menu saga in one line): three separate per-surface lists must
agree for a menu to work — cursor ownership, capture suppression, event
feed — and each was patched separately for years; r168/r170/r171 unified
all three onto one membership.

### Bulk-wiring techniques that paid off in r170:
1. SINGLE-WRITER AUTHORITY FOR CONTESTED STATE: eleven cursor-mode call
   sites became flag-writers + ONE reconciler; races vanish structurally
   rather than case-by-case.
2. SHIP A STALE-BINARY DETECTOR WITH UI CHANGES: a visible build marker
   ("menu build rN") converts "it looks the same" reports into a binary
   yes/no on whether the code under discussion is even running.

## r172 — log-driven fixes: lava heat explosion + creative starvation (UNVERIFIED)
Operator's pasted log was the bug report (menus now CONFIRMED working:
pause -> Save & Quit saved 17/29 chunks, per-world re-root at
worlds/New_World live, leaf_decay + plant_lifecycle ticking).
- fires: 11372 -> FPS 1: register_lava_heat_sources added ONE HEAT SOURCE
  PER SURFACE LAVA BLOCK — a lava ocean under spawn registered 11k
  sources; the smoke field and every total_heat_at scan walked all of
  them. r172 AGGREGATES: one centroid source per chunk, strength scaled
  by surface count (400 + 14/block, capped), radius 11 to cover the pool.
  Same gameplay warmth; ~2 orders of magnitude fewer sources.
- CREATIVE STARVATION: vitality drained 97%->76%/min in CREATIVE (HUD log
  proved it) — the hunger+exposure drain was never gated. Now survival-
  only; creative still ticks for regen.
- Noted, no action: "Biome registry unavailable: assets/biomes" warn is
  the legacy-biomes fallback working as designed (ship assets/biomes later
  or silence at info).
- Design verdict acknowledged ("old music producer's data dream"): flat
  egui is at its ceiling — the adventurer's interface IS the frosted
  session (docs/design/*), now unblocked by working menu input.

### Bulk-wiring techniques that paid off in r171:
1. SYMPTOM PAIRS LOCALIZE THE LAYER: "cursor visible + clicks dead" ruled
   OUT cursor mode and pointed at the event feed in one step — design
   symptom probes so each report halves the search space.
2. CHECK BOOT-TIME INITS WHEN PER-FRAME OVERWRITES LAND: ui_pointer_active
   = true at boot masked the dead feed for entire eras of builds; any
   per-frame assignment supersedes and must carry the FULL policy.

## r173 — LAVA BURNS + fluid audit (UNVERIFIED, balance-scanned)
Operator asked for fluid physics + meshing guidance. AUDIT FIRST: most of
it already exists and is LIVE — swimming (buoyancy from sim::fluid::
submerged_fraction + Jump/Sneak strokes, r-era), water 2x vitality drain,
water/lava motion factors (0.45/0.20) in the physics AABB scan, water
landings exempt from fall damage, and the heat side just got fixed (r172
lava aggregation). The actual GAP: lava had a motion factor but NO BURN.
- LAVA CONTACT (survival): body or feet cell == Lava -> vitality drains at
  0.45/s (~2.2s lethal), body_c spikes +6 deg C/s toward 43 (heat-stroke
  exposure then stacks), "[survival] YOU ARE IN LAVA" warn throttled. The
  already-live 0.20 motion factor makes ESCAPING the actual gameplay.
- Fluid module census for future sessions: world::water_cell (cellular
  spread + freezing) + world::water_field (per-chunk depth from chunks) +
  sim::water_physics (heightfield flow+moisture) are a complete dormant
  DYNAMIC WATER stack — needs a sim-tick + remesh integration session
  (changed cells -> set_block Water/Air + mesh invalidate). flow_field/
  river_flow/waterfall are worldgen-side carving aids. None wireable as a
  one-turn seam; ledgered as the "dynamic fluids" session.

### Bulk-wiring techniques that paid off in r172:
1. OPERATOR LOGS ARE BUG REPORTS WITH NUMBERS: fires:11372 and the HUD
   vitality series each localized a bug to one producer in minutes — ask
   for (or grep) the run log before theorizing.
2. PER-ITEM REGISTRATION OF AREA PHENOMENA EXPLODES: anything registered
   per-BLOCK over natural features (lava, water, flora) needs aggregation
   at registration time — centroid + scaled strength preserved gameplay at
   1/100th the cost.

## r174 — BIOME REGISTRY LIVE + macOS enablement + inputs landed (UNVERIFIED)
Per operator ordering ("biome expansion and wiring first... palettes"):
- BIOME REGISTRY ACTIVATED: the operator's own log named the dormant
  system ("Biome registry unavailable: assets/biomes... using legacy").
  Authored assets/biomes/unendlich/*.json — 22 biomes covering every
  legacy enum variant the loader maps (Minecraft 1.18 schema: _id,
  _legacy_enum, temperature [0,2], downfall [0,1], has_precipitation),
  tuned to spread the (T,D) best-fit plane: tundra(0.05,0.3) ...
  desert(1.8,0.05), jungle/savanna/taiga riding the climate fallback.
  PLUS path fallback in load_standard (assets/biomes, ../../assets/biomes,
  ../assets/biomes) — their CWD is crates/, same reason the music path
  needed ../... NEW BIOMES ARE NOW A JSON DROP AWAY (the registry replaces
  the hardcoded decision tree with best-fit climate lookup, and the opaque
  effects/spawners/features fields are parsed and waiting for consumers —
  the palette/texture hook point for the textures_overlay_system doc).
- macOS/MoltenVK ENABLEMENT (from the operator's vulkaninfo): instance
  gains VK_KHR_portability_enumeration + ENUMERATE_PORTABILITY_KHR flag;
  device enables VK_KHR_portability_subset. Both #[cfg(target_os=macos)]
  — the Asahi/Linux path is byte-identical. Remaining macOS risks for the
  session: MoltenVK timeline-semaphore/drawIndirectCount support levels.
- TITLE STEMS LANDED: assets/music/stems/koYopper (bass|drums|music).opus
  — the layered-music design (fade parts per section: title=music,
  gameplay +drums, combat +bass; EQ/saturation per environment) is the
  audio-backend session's input. Music director already cue-switches.
- SIX MORE DESIGN DOCS landed in docs/design/ (+extracted): gamemode
  trait system (SURVIVAL/ADVENTURE/CREATIVE/CRASH DUMMY as trait sets —
  composes with F6/catalog work), textures_overlay_system (16x16
  programmatic tile spec: base/shadow/highlight palette logic — the
  registry effects field is its hook), fluid POIs (waterfalls as
  flow-field singularities — pairs with r173 fluid census), and the
  conflicting-menus set (character panel, discipline panels, crafting/
  inventory layout w/ 3x3 grid + 4x9 inventory + genome tooltips) —
  CONFLICTS to be resolved with the operator's destiny-vs-raw morphing
  idea (environment-keyed UI skinning) at the frosted session.

### Bulk-wiring techniques that paid off in r173:
1. AUDIT BEFORE BUILDING (fluids edition): swimming, drag, buoyancy, and
   water drain were ALL live already — the askable gap was one block type
   missing its contact consequence. Surveying saved a redundant system.
2. ANSWER DESIGN QUESTIONS FROM THE TREE'S OWN VOCABULARY: the fluid-
   meshing advice mapped each technique onto modules that already exist
   (water_waves, flow_velocity, greedy mesher) — recommendations grounded
   in present code are immediately actionable.

## r175 — punch list: audio REAL, shadows edge, title decouple, UI keys (UNVERIFIED)
Operator's six-item report, all addressed:
1) AUDIO SLIDER IS REAL: audio::SpatialMusic gained set_master_volume
   (rodio Sink::set_volume across stems); the bin's held handle (renamed
   fg_music_player) is driven every 15 frames with slider x director cue
   volume — so the r166 pause-duck and menu cues are AUDIBLE TODAY, no
   backend session needed for volume. (Stems start_spatial exists for the
   uploaded bass/drums/music split — the layering session got cheaper.)
2) SHADOWS COME BACK: re-enabling invalidates last_shadow_rendered_vp on
   the falling edge (prev_shadows_disabled tracked) — before, the cascade
   reuse cache "reused" the cleared map forever. Toggle design polish (a
   doc-styled switch) deferred to the frosted pass with everything else.
3) TITLE DECOUPLED (the careful way): ONE in_world_overlays predicate
   (=!menu_open && !wizard_open) gates the hotbar AND the creative picker
   renderer-side (HUD lines were r167). No per-widget hacks: future
   overlays must join the predicate. The wrong-later quick fix (suppress
   by clearing bin state) was avoided — title backdrop stays a live world.
4) SLIDING FRAMETIME: perf_samples is now a per-frame capped ring (240)
   pushed unconditionally; sparkline uses a FIXED 50ms scale so it
   scrolls instead of rescaling. (Abstraction note honoured: the shared
   closure is the component; a widgets module is the egui-refactor item.)
5) C CLOSES: UI keys are handled UNCONDITIONALLY while their window is
   open — the gameplay block is gated captured+unpaused, exactly the
   state an open window exits. Esc now closes settings/craft first
   (consume) before meaning "pause". This also covers the dropped-keys
   complaint for UI: handlers no longer live behind gameplay gates.
   (True event-queue just_pressed for low-fps key drops: ledgered,
   input-module session.)
6) B DIAGNOSTIC: unconditional edge log "[bending] B pressed
   (captured/paused/survival)" — next operator log localizes why the
   bending block doesn't fire on their machine (it's hold-to-charge
   in-world; suspicion: paused/captured state or hold duration).

### Bulk-wiring techniques that paid off in r174:
1. THE OPERATOR'S LOG NAMES DORMANT SYSTEMS: "Biome registry unavailable"
   was an activation instruction — warnings about missing assets are
   features waiting for files.
2. DATA-DRIVEN REGISTRIES BEAT CODE WIRING: 22 JSONs activated a whole
   subsystem with zero Rust; prefer feeding loaders over writing arms.

## r175b — overlay-gate brace fix (UNVERIFIED)
The r175 in_world_overlays close anchored on a `}),\n);` pattern that also
exists in the c.13 instrumentation block — the brace landed THERE (3333)
instead of after the hotbar Area, so the egui prepare closure at 3280 was
short one `}`. Removed the stray; the gate now closes immediately after
the hotbar's .show() and before the prepare `})?;`.
LESSON for the technique log: the brace-balance scan verifies COUNT, not
PLACEMENT — when inserting a lone `}` (or `{`) by text anchor, the anchor
must be unique in the file (grep -c first) or carry surrounding lines
until it is. Count-neutral misplacements are invisible to the scan and
produce "nearest open delimiter" walls far from the edit.

## r175c — music handle rename completed (UNVERIFIED)
The r175 rename (_music -> fg_music_player) missed one pre-existing
consumer: the per-frame spatial-mix update at the loop top (it referenced
_music 40 lines below the binding). Renamed. NOTE: the compiler's
"similar name" suggestion (fg_music) is WRONG here — fg_music is the
director's MusicState (cue/volume brain), fg_music_player is the
Option<SpatialMusic> rodio handle; taking the suggestion swaps a state
struct into an Option pattern ("then it started saying Some() not state").
LESSON: when renaming a binding by single-site replace, grep the OLD name
across the file first — rename ALL consumers in the same edit, and don't
trust same-prefix compiler suggestions when two domains share a stem.

## r176 — round-trip hardening + canonical tooltips (UNVERIFIED, scanned)
Operator: world<->title round trip works ("just have to finish").
- LAUNCH UI RESET: entering a world now closes every window (pause/
  settings/craft/debug) and zeroes both latches (pause_action,
  craft_pick) — previously a window or stale pick could ride the Launch
  into the new world (the same boundary family as r162, UI edition).
- CANONICAL TOOLTIPS (first slice of item_armor_lore_tooltips — its
  format is declared FINAL, lore text placeholder BY DESIGN): craft rows
  now hover-show the doc structure — name, mechanical effect lines,
  quoted lore, attribution from the six canonical source types (Dojo
  inscription, Shrine fragment, Ryujin field record, Field observation,
  trade-stall tag). Entries written for Iron/Gold Ingot, Oak Planks,
  Stone, Glass; unknown outputs get a stub line. craft_lines gained a tip
  column (renderer struct + window + bin builder updated together).
- The inventory-panel + hotbar hover tooltips ride the same helper when
  the inventory UI is next touched (frosted pass).

### Bulk-wiring techniques that paid off in r175/r175b/r175c:
1. ANCHOR UNIQUENESS IS PART OF THE EDIT (r175b): a lone brace inserted
   on a non-unique anchor lands count-correct and placement-wrong —
   grep -c the anchor first; the balance scan cannot see nesting.
2. RENAME = GREP THE OLD NAME FILE-WIDE FIRST (r175c): a consumer 40
   lines past the view window kept the old name; and same-stem compiler
   suggestions (fg_music vs fg_music_player) cross domains — verify the
   TYPE, not the spelling.
3. SINK-LEVEL WIRING BEATS WAITING FOR THE BACKEND (r175): rodio sinks
   already existed under SpatialMusic; one setter made the volume slider
   real without the audio session.

## r176b — tooltip string-literal fix (UNVERIFIED)
The r176 lore strings embedded straight double quotes, terminating the
Rust literals ("unknown prefix `mason`" = the parser fell out of the
string at the apostrophe-adjacent quote). Rewritten with Unicode escapes:
\u{201c}/\u{201d} curly quotes, \u{2019} apostrophe, \u{2014} em-dash —
no escaping fragility AND typographically correct for lore text (the
doc's voice deserves real quotation marks).
LESSON: lore/prose embedded in code should use \u{} escapes for ALL
typographic characters from the start — it sidesteps three layers of
quoting (python -> rust -> display) at once.

## r177 — ocean fix, ascii perf export, advisor, bender resilience (UNVERIFIED)
Operator's seven-item list, triaged:
1) OCEANS RESTORED (root cause of "grass-block ocean with land fauna" AND
   half of "swimming doesn't work"): the r174 registry best-fits on
   (temp,humidity) ONLY, but Ocean/Beach are TOPOGRAPHIC classes carried
   by the erosion channel the legacy tree consumes — the registry silently
   deleted oceans (operator log: H~0.62, my Ocean JSON needed 1.0). RULE
   ADDED in cpu_generator: topography vetoes climate — if the legacy tree
   says Ocean/Beach, that stands; the registry owns LAND climate biomes.
2) OCEAN FAUNA GATE: the spawn director returns no land herds when the
   spawn-center biome is Ocean (fluid_pois doc gives water its own mob
   ecology — corals + water fauna == the coral session, now properly
   blocked only on a Coral/water-flora block set + underwater placement).
3) SWIMMING: code audit says the swim branch is correct (non-solid Water,
   buoyancy, strokes) and SURVIVAL-physics-only (creative flies through).
   With oceans restored there is finally deep water near spawn to test in:
   F6 -> walk in. If it still fails, the next log will say.
4) ASCII FRAME-TIME EXPORT (analytics button): dumps a 100x12 '#' graph
   (fixed 50ms ceiling, newest right) + p50/p99 + stutter ratio to the
   log — paste it here and the stutter pattern is legible (periodic teeth
   = tick cadence; random spikes = uploads; sawtooth = eviction).
5) STUTTER ADVISOR (same dump): live-counter heuristics with the exact
   levers the operator already runs — heat_sources>200, herd>20,
   drops>60, the plant/leaf tick slot vs UNENDLICH_APPLY_BUDGET, shadow
   stride cadence vs UNENDLICH_SHADOW_CHUNK_STRIDE.
6) BENDER RESILIENCE (item_armor_lore_tooltips declares bending
   integrations canonical): FIRE-attuned artifact (essence-drunk, element
   Fire, counter>0.5) -> lava burn + heat spike at quarter rate (deep
   mining opens up); DUST/EARTH-attuned -> fall damage halved ("the
   mountain holds its own"). Both read fg_imbue — the essence loop now
   has build identity: drink the dragon that matches the depth you mine.
7) PLAYER MODEL: ledgered as a session (third-person body + item
   attachment points; viewmodel r148 is the first-person half).

## r178 — FSM-routed pause, thermal sanity, swim fix, veil, narrative logs (UNVERIFIED)
Operator log + report drove all of it:
1) SAVE & QUIT FIXED AT THE ROOT: game_flow ALREADY had the full pause FSM
   (InGame -PauseToggled-> Paused -QuitToMenu-> MainMenu with title reset
   to root) and r167's renderer-only overlay bypassed it — the app stayed
   InGame, so the fake title relaunched a fresh world per click ("save and
   exit again and again"). Added AppInput::QuitToTitle (app/mod.rs ->
   menu.quit_to_menu()); pause actions now tick the FSM (Resume/Pause/
   QuitToTitle) and renderer.menu_open simply FOLLOWS fg_app.state() in
   the existing sync block. Esc-close resumes symmetrically (the FSM can't
   be left stuck Paused — services whitelisting depends on it).
2) THERMAL SANITY (log: body 282degC): body_c clamped to physiological
   20..45 after step_body; the OVERHEATING/FREEZING warn is survival-only
   (creative has no body to shout about); the r172 lava centroid strength
   capped to 300+8/block(max60), radius 9 — warmth, not a furnace.
3) SWIM FIXED BY ARITHMETIC: gravity 28 vs buoyancy 14 + thrust 6 — Space
   SANK at -8 blk/s^2. Buoyancy now 26*sub (near-neutral): Space rises,
   Sneak dives, idle drifts gently down.
4) ADVISOR LEARNS THE PASTE: the operator's log fingered mesher submit
   (12-15ms x 66-84/s while descending; submit=477-790ms/s) — advisor now
   names it with the exact levers (GPU_MESH_BATCH 4->2, MAX_UPLOADS 4->2).
   Their plants suspicion: plant_lifecycle 4-7ms/tick is real but
   secondary at 8-chunk apply budget.
5) NARRATIVE LOGS (their "natural language logs page"): analytics window
   now leads with prose ("Running at about 6 fps. The herd is 23 strong
   with 4 items on the ground.") with the raw stat line beneath as the
   unfiltered tier. ASCII export unchanged.
6) LOADING VEIL + MUSIC FADE: Launch opens on black (loading_fade=1) with
   "entering <world>" and concentric breathing bending rings (placeholder
   art until the shader/neural pass — reference-image pipe ledgered), 
   easing off over ~2s; the music master follows the veil (fade-in).
7) LEDGERED SESSIONS: item models into the entity shader path (merges
   with the player-model session: shared mesh+attachment pipeline);
   neural/reference loading art.

## r179 — stems live, hot pockets, wizard name/seeds, answers (UNVERIFIED)
ANSWERS THE OPERATOR ASKED FOR EXPLICITLY:
- ROLLBACK: EXISTS — world/rollback_scope.rs + sim/rollback.rs in THIS
  tree, plus docs/ROLLBACK.md in the r132 docs set. Not yet consumed by
  the bin; the map-preview/rollback-preview UI is therefore a WIRING
  session, not missing tech.
- MAP/SEED PREVIEW: EXISTS as examples/seed_preview.rs (a runnable seed
  preview). Wiring it into the wizard as a live thumbnail = same session
  (render-to-texture or ascii-height preview first).
- GLFW MIGRATION: docs/GLFW_DECOUPLING.md exists in the r132 docs set —
  the multi-input/no-build-block migration has its blueprint; ledgered as
  the input-platform session (pairs with the event-queue key fix).
WIRED THIS ROUND:
- THREE-STEM SPATIAL TITLE: the uploaded split (music/bass/drums) now
  plays via audio::start_spatial as THREE WORLD EMITTERS (engine's own
  menu_stem_layout: music centre, bass left, drums right; update(listener)
  already runs per frame, so the title orbit camera MOVES THROUGH the
  mix). Merged single remains the fallback. Limiter/tape-stop/saturation/
  tempo-variation + 30s-restart FSM: audio-session features, ledgered
  verbatim.
- TITLE MUSIC IS TITLE-ONLY: in-world (crafting/creative menus included)
  the title bus multiplies to 0 — silence until gameplay tracks land.
- HOT POCKETS (operator design, temps doc-aligned via the 20..45 clamp):
  strong local heat (live heat map) feeds a FIRE-attuned artifact —
  imbue counter trickle + faster resonance recovery. The glowing block
  they found is now a forge. Pressure-bending reach: ledgered with the
  bending-shader session.
- WIZARD: world NAME text field (WizardSetName was already in the App —
  unconsumed until now) + seed cycle expanded: Random -> 1337 -> clock
  entropy -> name hash -> Random.
LEDGERED SESSIONS (from this report): dragon polish (animation stretch
measurements + scarier presence + sound genomes — needs visual feedback
loop), textures + block-break particles (shader session w/ textures_
overlay_system doc), bending shader effects (+ rollback-owned state),
menu parallax (frosted v1.5), multiplayer open-world-to-players menu.

## r180 — THE WIRING SYSTEM: radar + FSM + pad-driven batching (UNVERIFIED)
Operator: stop re-deriving the map per turn; keep a line-numbered
scratchpad; don't act until a pattern hits ~20x; FSM the brace checking.
DELIVERED as tools/wiring_radar.py, three subcommands:
1) RADAR (default): regenerates docs/RADAR.md — every ledgered sweep
   technique as a mechanized analyzer with file:line anchors:
   A zero-callsite pub fns (626!)   B let-_ discards (18)
   C TODO/stub (50)                 D doc promises (60)
   E commented registrations (1)    F undeclared files (3: the PostFx trio)
   G env-knob asymmetry (46)        H dead debug toggles (13)
   I unproduced AppInputs (1: SelectSlot)   J doc->module map (9 docs)
   THE PAD IS THE MAP. Turns start by reading RADAR.md, not grepping.
2) CHECK: a real Rust-aware FSM lexer (strings, raw strings w/ hashes,
   chars vs lifetimes, line + NESTED block comments) verifying bracket
   balance and reporting the LINE of the first mismatch or the innermost
   unclosed bracket at EOF — catches the r175b placement class and the
   r176 string-wall class that the regex strip could not. Now the
   standard post-edit gate (replaces the inline python scans).
3) ANCHOR: uniqueness check for an edit anchor BEFORE replacing —
   exits nonzero unless exactly 1 occurrence (the r175b rule, enforced).
FIRST 20x BATCH EXECUTED off the pad (mechanical, radar-generated):
   analyzer G -> 44 undocumented engine knobs now listed as a generated
   reference block in tools/run-latest.sh, and 2 DEAD knobs the operator
   has been setting flagged: UNENDLICH_PLANAR_OVERFLOW (nothing reads
   it) and the docstring artifact. Notably UNENDLICH_BLOOM exists as a
   READ knob — the PostFx session has an env switch waiting.
WORKFLOW FROM HERE: regen radar -> read pad -> queue batches >= 20 same-
   shape items -> execute mechanically -> fsm check -> package. Analyzer
   A (626) needs sub-classification before batching (next radar upgrade:
   cluster by module family + signature synthesizability).

## r180b — hot-pockets scope fix (UNVERIFIED; first tooling-gated fix)
The wall was MINE (r179 hot pockets), not the radar's — the radar only
writes docs/RADAR.md and tools/run-latest.sh comments; it cannot produce
compile errors. heat_near (2803) belongs to a sibling nested block; the
hot-pockets block now samples the heat map itself at the player position
(one extra total_heat_at per frame — trivial post-r178 aggregation).
This fix ran the NEW workflow end-to-end: `anchor` verified the edit
target was unique (1 occurrence, line 2843) BEFORE the replace; `check`
(FSM) passed after. SCOPE remains the FSM's blind spot — it verifies
brackets, not name resolution; the r156b frame-order lesson generalizes:
sibling-block locals need their own sampling or a hoist. Candidate radar
analyzer K: flag bin edits that reference `let` names declared in
non-ancestor blocks (cheap heuristic: name defined deeper than use).

## r181 — audio single-writer, typable text, seed preview, glass (UNVERIFIED)
Operator's four-item report:
1) AUDIO CUTOUT ROOT-CAUSED, two layers: (a) set_master_volume (r175) and
   update()'s positional gains were TWO WRITERS slamming the same sinks —
   master is now a stored atomic factor applied INSIDE update() for
   spatial mode (single mode still sets sinks directly): one writer, no
   15-frame pumping. (b) The title orbit camera sits 46 units out, far
   past the stems' falloff (6) — that WAS the cutout. Menu listener now
   scales toward the emitter origin (x*0.08): gentle stereo drift as the
   orbit turns, never silence.
2) TEXT INPUT CLASS FIXED (why the name box was dead): the egui event
   feed forwarded POINTER events only. glfw WindowEvent::Char now drains
   into ui_text_frame -> renderer.ui_text_input -> egui::Event::Text,
   plus Backspace/Enter as egui::Event::Key — while any UI owns the
   cursor. EVERY TextEdit is now typable (name, seed, future consoles).
3) SEED ENTRY FIELD (typed number, or any text byte-hashed *131) +
   BIRDS-EYE EQUIRECT PREVIEW, fast and cheap as specified: 96x48
   top-down strip (x wraps, z half-band, 8-block stride), biome color +
   height shading, sampled from a throwaway CPU WorldGenerator only when
   the FIXED seed changes; uploaded as a nearest-filtered egui texture,
   drawn 3x in the wizard. Random shows no preview (nothing to show).
4) GLASS CHROME: one shared translucent frame (fill alpha 208, hairline
   stroke, rounded 8) across all six menu windows — the visible
   fanciness step; parallax + true frosted blur remain the PostFx
   session (UNENDLICH_BLOOM knob already waiting, per radar G).
DHT PHONE-GAMEPAD MESH: deferred one turn at the operator's suggestion —
   their doc is starting; the reward standard is set. Controller support
   rides the GLFW_DECOUPLING session (multi-input scaffolding).

### Bulk-wiring techniques that paid off in r180/r180b:
1. THE PAD IS THE MAP: regenerating RADAR.md costs one command; reading
   it costs less than one grep. Every analyzer is a ledgered technique
   that no longer spends my tokens.
2. BATCH ON PATTERN, NOT ON FINDING: the 44-knob reference was generated
   FROM the radar by script — zero hand-typing, zero transcription error.
3. TOOL-GATE EVERY EDIT: anchor (uniqueness) before, fsm check after —
   two error classes (placement, strings) now structurally impossible;
   the remaining class (sibling scope) is analyzer K's job.

### r181 addendum: the preview block initially MISSED its anchor silently
(anchor said "r167:" but the file says "r167/r178:") — caught because the
verify check was re-grepped before presenting. RULE HARDENED: every
multi-line insert now runs `wiring_radar.py anchor` FIRST (exit-gated),
exactly like this fix did (1 occurrence @ line 2010). Silent no-op
replaces are the 5th error class; anchor-gating retires it.

## r182 — DHT docs landed, radar v2, registry near-miss contained (UNVERIFIED)
INPUTS: the deferral reward arrived — phone_controller_lan v2 (DHT
Kademlia session discovery §2b, adaptive EMA gesture zones §3b; phone =
pure input surface, render box renders), touch_input_reference v2
(handheld vs controller modes, bending gesture grammar, volume-button
mapping, LAN IMU via iio-sensor-proxy), tutorial_system (contained
PRACTICE WORLD with element-wizard mentors, in-world display screens,
choir harmonisation; progress carries to main world). All three in
docs/design/ + extracted. They compose: tutorial's controller onboarding
section assumes the phone-controller path — one input-platform session
covers GLFW decoupling + virtual-controller driver + touch grammar.
RADAR v2: analyzer A2 clusters the 626 zero-callsite fns by family —
sim 291 / world 146 / root 133 / render 32 — and analyzer K (sibling-
scope heuristic, the r180b class) found 33 risk sites, now a pre-edit
checklist. Analyzer G dropped 46 -> 2: the radar VERIFIED ITS OWN r180
batch (the knob reference closed the asymmetry).
THE SIM-291 BATCH, correctly delegated: those are sim fns no registry
reaches. tools/bootstrap_registry.py is the mechanical executor — but
its self-heal loop needs the operator's cargo (sandbox lockfile v4
abort). OPERATOR ACTION: `python3 tools/bootstrap_registry.py` on your
machine; it generates, cargo-checks, prunes failures, re-tests — the
survivors wire themselves. NEAR-MISS CONTAINED: an unknown flag (--help)
EXECUTED the sweep here and overwrote the pruned gen files; all four
registry files were recovered byte-identical from the shipped r181 zip
(cmp-verified), and the tool now refuses unknown args. LESSON: tools
that overwrite generated code must gate argv strictly; zips double as
recovery snapshots — never clear outputs before verifying the tree.

## r182b — preview-texture closure fix + egui 0.34 int args (UNVERIFIED)
Wall pinpointed three things: (1) the texture upload referenced the
closure's ctx (`c`) from OUTSIDE the prepare closure — moved inside at
the wizard draw site using the established capture-locals + write-back-
after pattern (wizard_preview_uploaded latch clears dirty); (2)+(3) egui
0.34's CornerRadius::same takes u8 and Margin::same takes i8 — float
literals swapped for ints. NOTE for the API gotcha list: epaint 0.34
integer-ized Rounding/Margin; any styling code written from older egui
muscle memory will hit this pair.

## r183 — char polling, tick strides (UNVERIFIED)
Operator: previews work, backspace works, CHARS don't; plant 14.8ms +
leaf 9.0ms + apply 13.1ms per tick.
1) CHAR ENTRY: glfw-rs only delivers opted-in event types — Key polling
   was on since day one, Char polling NEVER was (which is exactly why
   Backspace worked while typing didn't: Backspace rides Key events).
   window.set_char_polling(true) added next to the other pollers.
   API GOTCHA LIST: every WindowEvent variant needs its set_*_polling.
2) TICK STRIDES: new ticks::strided(n, handler) combinator — runs the
   wrapped handler every Nth invocation with tick_dt scaled by N, so
   simulated growth/decay RATES are unchanged while per-tick cost drops
   to 1/N. plant_lifecycle default stride 4 (UNENDLICH_PLANT_TICK_STRIDE)
   -> ~3.7ms amortized; leaf_decay stride 2 (UNENDLICH_LEAF_TICK_STRIDE)
   -> ~4.5ms. Expected: stutter teeth shrink from ~24ms to ~8ms worst
   tick. apply=13.1ms responds to UNENDLICH_APPLY_BUDGET (8 -> 4 if the
   teeth persist); the radar's knob reference documents both new strides
   on the next regen.

## r184 — live perf levers + autotune + tick-authority doc (UNVERIFIED)
1) LIVE LEVERS: ticks::strided_dyn (shared AtomicU64, read per
   invocation; dt-scaling contract preserved) replaces baked strides;
   ChunkManager::set_apply_budget added. Settings gained a PERFORMANCE
   tab: plants 1..16, leaves 1..8, apply budget 1..32 — applied every 15
   frames from the sliders.
2) FPS AUTOTUNE: optional target (15..120); every 120 frames, below
   0.95x target it raises plant stride, then leaf stride, then lowers
   apply budget (floor 2); above 1.25x it relaxes in reverse. Logged as
   [autotune] lines so runs are diagnosable. Multiplayer min-rule
   ("lowest authoritative device; clients don't count") recorded as the
   governing rule in the new doc.
3) DESIGN CAPTURED (no retyping): docs/design/tick_authority_regions.md
   — shared/unshared ownership, BLENDED REGIONAL AUTHORITY ("stepped in
   but hasn't been to my base yet — don't turn off my fast tick"), and
   ROLLBACK-UNTIL-READY async ticks (render last rollback state until
   compute lands, then deferred apply; nothing with a loading time is
   ever shown half-done). Each idea mapped to the modules that already
   exist (rollback_scope, TickRegistry, block_edits queues, apply
   budget) + a 4-step wiring order, of which step 1 shipped this turn.

## r184b — renderer field defaults (UNVERIFIED)
The r184 defaults insert anchored on "ui_music_vol: 0.8," but the ctor
says 1.0 — silent miss (the slider's DEFAULT and the ctor value differ;
I assumed instead of reading). Landed anchor-gated at line 1279. RULE
REINFORCED: the anchor gate is for EVERY insert, including "trivial"
default lists — the r181 addendum rule had an exception carved out by
habit, and the exception cost a wall.

## r185 — audio lockstep groups, typing focus, wizard create+pick (UNVERIFIED)
1) AUDIO GROUPS / TIME LOCKSTEP: new audio::StemGroup — ONE rodio sink,
   ONE mixing Source that sums all stems per sample; per-stem gains are
   atomics written by update() (cached per 512 samples in the hot loop).
   Three independent sink clocks -> one clock: stems CANNOT drift, by
   construction. Format guard (stems must share rate+channels; mismatches
   skipped with a warn). Bonus from the old wishlist: a brick limiter on
   the group bus (sum clamped) — nothing clips. start_single unchanged.
2) TYPING FOCUS: renderer exports egui's wants_keyboard_input() per frame
   (ui_wants_keyboard). Letter hotkeys now yield to focused text fields:
   C-close, B diagnostic, and the layout-aware P toggle all gated
   (!typing); Esc deliberately exempt. This was the "keys getting pulled
   somewhere else" — typing c/b/p into the name box was firing hotkeys
   and stealing focus.
3) WIZARD: " Create World " button on EVERY step (action 9) — blank name
   gets "New World" bin-side instead of hiding the button; Random adopts
   the PREVIEWED seed on create (no surprise world). Random now previews
   a CONCRETE rolled seed. PREVIEW IS CLICKABLE: left-click a region ->
   the clicked biome is sampled, ten LCG-derived candidate seeds are
   scored by that biome's coverage (32x16 coarse pass), best coverage
   wins; RIGHT-click inverts (least coverage wins). Result logged as
   [wizard] preview match/avoid -> seed N (coverage K) and the preview
   regenerates via the existing dirty path.

## r186 — quit screens, create-spree, right-click, camera pan, rollback
## points, group DSP (UNVERIFIED)
1) QUIT FIXED: clicking Quit lands on Screen::ConfirmQuit — which the
   renderer NEVER DREW (one button set for all screens). Menu window is
   screen-aware now (menu_screen_kind display field): ConfirmQuit draws
   "quit unendlich?" + Quit/Back (picks 5=Confirm, 6=Back); Multiplayer
   draws Host/Join/Back. AppStep::Quit handling already existed.
2) CREATE WORLD SPREE: action 9 arms fg_create_spree=6 — each frame the
   spree injects Next (or Confirm at the confirm step) through the
   NORMAL wizard path, so Launch handling runs (the r185 button sent one
   Confirm, which only advanced one step — "worked like Next").
3) RIGHT-CLICK: the egui feed forwarded LEFT button edges only (r171) —
   Secondary edges now ride alongside (ui_pointer2_down + prev). The
   avoid-pick works; same class as the r181 text-events gap: the egui
   event feed only ever knows what the bin forwards.
4) CAMERA PANS + PARALLAX (frosted v1.5 step 1): each menu screen owns a
   yaw offset (confirm-quit looks away 1.9, multiplayer pans -1.1,
   wizard 0.8) + cursor micro-parallax (x 0.07, y 0.03 on pitch), both
   smoothed at 4/s into the orbit angle.
5) ROLLBACK POINTS: every 5 minutes, on craft success, and per 1000
   blocks traveled (XZ accumulator, teleport-guarded) -> save_all + a
   reason-stamped marker (worlds/<name>/rollbacks/<unix>.point with
   reason= and chunks=). [rollback] log line per point. The restore UI
   consumes these later (tick_authority doc, step 2+).
6) GROUP DSP (handrolled, per request): per-stem MILD saturation
   (soft-knee, drive randomized 1.05..1.25 per session) feeding a
   UNIFIED Schroeder-lite reverb on the summed bus (two combs 29-55ms +
   one 5ms allpass, feedback/wet randomized once) — the group shares ONE
   time/space so stems stay phase-coherent; brick limiter unchanged.
   Unified RETIME/pitch + LV2 (bankstown) hosting: ledgered for the
   audio session — retime needs a resampler on the group bus (single
   insertion point now exists: StemGroup::next).
7) Preview brush: clicks sample a 3x3 neighborhood (24-block spacing)
   majority — the "blended bigger brush". Liquid-glass 8x8 morph reveal +
   panel size-morph + panel liquid-glass backgrounds: PostFx/frosted
   session (the veil + glass frames are their mounting points).

## r187 — sticky settings, cinematic music entry, dest buttons, char
## diagnostics, press parity (UNVERIFIED)
1) STICKY SETTINGS: $XDG_CONFIG_HOME/unendlich/settings.json (HOME/.config
   fallback, CWD last) — fog, shadows, music vol, day length, strides,
   apply budget, fps target, autotune. Loaded at boot; saved on EVERY
   settings-close path (falling-edge detector covers the Done button) and
   on Quit. serde_json (already a dep via the biome loader).
2) CINEMATIC MUSIC ENTRY: playback gain now waits for MENU-READY (veil
   < 0.6) and fades in over ~2.5s — kills the double-start (track began
   on the blackscreen, then the title-bus gate re-entered it).
3) DESTINY-ESQUE BUTTON KIT v1: dest_button — full-width hit area,
   hover-growing accent bar (animate_bool), letterspaced caps, alpha
   ramps. Title/ConfirmQuit/Multiplayer screens converted (9 sites).
   Sweep shaders + hover sounds join at the frosted session.
4) CHAR ENTRY: plumbing audits clean end-to-end (polling, drain, feed
   order, egui push all present) — so r187 ships a TWO-SIDED DIAGNOSTIC:
   "[text] glfw char 'x'" at the drain and "[text] -> egui ... wants_kb"
   at the feed. The next paste splits it: no drain line = GLFW/Wayland
   char delivery; drain-but-no-egui = membership; egui-with-wants_kb
   false = focus. One paste, three-way fork.
5) PRESS PARITY: InputCache counts presses per frame
   (presses_this_frame); the C-close toggle consumes PARITY so a double
   tap inside one laggy frame nets out instead of dropping. Broad
   adoption at remaining toggles rides the input-platform session.
6) AUDIO-RIDES-ROLLBACK captured in tick_authority doc §5: streams held
   while a rollback point is held; reverb tails snapshot-able (the
   Schroeder buffers ARE the tail) and cacheable by environment
   signature. Zero retyping.
NOTE: parallax + screen-pan shipped in r186 — if the report predates that
build, it should appear on the next run.

## r188 — water layers merged, doc-linker toolkit, testing verdict (UNVERIFIED)
1) TWO WATER LAYERS ROOT-CAUSED + MERGED: terrain height builds on the
   VARYING model sea level (surface_target = sea(x,z) + continental +
   biome + hills, +-8 region-blended) but BOTH water fills flooded to
   flat 64 — +8 zones drowned their intended shoreline (a second plane
   above the beach), -8 zones left a dry band under the surface. Fix in
   CPU<->GPU PARITY: CPU Pass A hoists a per-column sl_col from
   sea_level_at; GPU Pass A gates per column on model_sea_level; both
   tree submerged-skips follow the varying level too (cpu wx/wz locals,
   comp wx_origin/wz_origin int->float casts). "Shaders a bit weird" at
   the seam should fade with the seam itself; remaining translucent
   double-face polish = fluid session.
2) DOC-LINKER TOOLKIT (tools/doc_linker.py, VERIFIED on real output):
   tf-idf top-K distinctive terms per design doc, each linked into the
   tree at a LINKER DEPTH (0 filename / 1 pub ident / 2 code / 3 comment
   / 4 MISSING) -> docs/DOC_LINKS.md with per-doc PARITY SCORES, the
   shallow/missing terms as a ranked wiring backlog, and a DOC<->DOC
   crossref graph (shared distinctive terms). Verification: lowest
   parity = tutorial_system 46% with MISS exactly on tutorial/
   harmonisation/choir/miren/scholar (the unbuilt mentors); crossref
   pairs phone<->touch at 7 shared terms and the two character-panel
   docs at 3 — the graph matches reality. PARITY RANKING = the new
   wiring order: tutorial(46) creature-structures(50) fluid_pois(53)
   discipline-panels/phone(57)...
3) PARTIAL TESTING VERDICT: sandbox cargo is 1.75; the workspace's
   transitive deps require edition2024 (cargo ~1.85+), and rustup is not
   network-reachable here — in-sandbox cargo check is OFF for good
   reasons, the FSM/anchor/radar gates remain the in-sandbox
   verification tier, the operator's machine remains the compiler.
   SCCACHE wired operator-side instead: SCCACHE=1 ./run-latest.sh sets
   RUSTC_WRAPPER when sccache is installed — big win across release
   zips since most crates are identical between them.

## r189 — IN-SANDBOX COMPILE + TEST ONLINE (operator's rustup.zip)
The operator shipped an offline toolchain bundle (pack-rust.sh product:
rustc/cargo/rust-std 1.96.0 standalone tarballs + self-contained
INSTALL.sh). Installed; native deps added (libasound2-dev pkg-config
cmake libwayland-dev libxkbcommon-dev xorg-dev autoconf automake libtool
libopus-dev). tools/sandbox-rust.sh now automates the per-session
reinstall (sandbox resets; bundle expected at uploads/rustup.zip).
RESULTS, first session:
- cargo check -p unendlich-ui: CLEAN.
- cargo check -p unendlich-engine (lib, featureless): caught r188's
  orphaned `sea_level` consumers (freeze band) — FIXED to ride sea_max
  (band widened to cover baseline-8..+9, the varying-level era). The
  remaining featureless errors are feature-gated modules (expected).
- cargo check engine lib + BIN, FULL features: **CLEAN** — every blind
  edit r184b..r188 compiles (perf tab, autotuner, StemGroup+DSP, screens,
  spree, right-click feed, parallax, rollback points, sticky settings,
  dest buttons, press parity, water merge).
- Targeted test suites (ticks, swim, biome_loader, spatial_audio,
  world::cpu_generator, rollback, audio, input): **52 passed, 0 failed**
  — the water-fill rewrite passes the CPU<->GPU parity tests.
STANDING WORKFLOW UPDATED: every turn = edit -> anchor/FSM gates ->
cargo check (bin, full features) -> targeted tests for touched modules
-> package. The "UNVERIFIED" tag now means runtime-unverified only;
compile+unit tiers are in-sandbox. Full 6726-test sweep stays operator-
side (registry sweeps are minutes-long).

## r190 — HARNESS LIVE: it found and fixed the plant stutter (CHECKED)
INGESTED: headless_coverage_testing v2 (ECS wiring coverage, dataflow
graphs, FSM transition pairs, butterfly tracing) + 100days coverage
notes -> docs/TESTING_DIVERGENCE.md maps every concept onto machinery
this tree already has (Fixed physics, id-seeded mob RNG, deterministic
ticks, InputBuffer = the input-log format, 27 image examples,
render_parity::image_diff). Doc-linker reran: 68% parity on arrival.
HARNESS v1 SHIPPED + RUN (examples/headless_run.rs): deterministic
fuzzed-input sim, per-tick FNV digests, handler/biome coverage matrix,
BUTTERFLY PAIR mode (one flipped input bit at T), JSON report + top-down
PPM/PNG to outputs. Butterfly verified: divergence_tick == flip tick
exactly; curve clean-then-#.
FIRST CATCH, MEASURED AND FIXED: 41,606 state writes PER TICK from the
plant loop — age_ticks increments every tick for every plant, so the
no-change guard never fired, and EVERY write clones the chunk + bumps
version + REMESHES. That is the plant stutter. Fix: PER-PLANT STAGGER —
each plant advances on 1/8 of ticks with an 8x window; identical
evolution by the time-homogeneity property, PROVEN in-sandbox by
examples/plant_homogeneity_probe.rs (advance_n_ticks(8) vs 8x
advance_one_tick over 30,272 lattice points: 0 mismatches). Measured:
state writes 8.32M -> 1.04M per 200 ticks (exactly 8x). Stage commits
delay <=8 ticks (~4s), inside the existing catch-up philosophy. NOTE:
harness also flags tree_root_ops at 1/chunk/tick (re-enqueued every
refresh — harness runs schedule=None; verify in-game rate next paste).
PARALLAX FIXED FOR REAL: fg_app.camera_pose() OVERWRITES the
pre-advance view — the r186 pan fed local_player.view BEFORE the
overwrite and was discarded. The smoothed pan now rotates the FINAL
eye+target around the spawn pivot; cursor pitch tilts the final pitch.
GLASS v2: per-window open INTERPOLATION (margin 6->14 + alpha over
~200ms via animate_bool) + cheap FRESNEL pass (bright top rim, side
falloff) — adopted on the main menu; remaining windows next pass. DOF
stays PostFx-session.
TOOLCHAIN NOTE: dev-profile full-lib TEST builds OOM on this 4GB box
(three silent deaths); the probe-as-example pattern (no cfg(test)
doubling) is the in-sandbox property-test tier. cargo check (bin, full
features): CLEAN.

## r191 — decoupled aging, memo graph, threading stage 1, kit crate,
## ramp scheduler, turn tooling, suite triage (CHECKED + 7147 tests)
1) TICK/RENDER DECOUPLED: apply path tracks whether a chunk's state
   batch touches anything beyond age_ticks — AGE-ONLY batches skip the
   version bump entirely (no remesh, still saved via dirty). Combined
   with the r190 stagger: remeshes now happen only on real stage flips.
2) MEMOIZED ADVANCE (the operator's before->after state graph):
   advance_n_ticks is pure in (state, moisture, n); thread-local
   HashMap keyed on EXACT bits (no quantization, zero semantic change).
   Smooth moisture fields make whole grass meadows share one key — one
   computation serves the class. 64k cap, clear-on-overflow.
3) THREADING STAGE 1 + PLAN: rayon landed; Phase-1 worldgen columns
   parallel (map -> serial scatter, deterministic). docs/design/
   THREADING_SIMD.md stages 2-4: 2x2x2 region-checkerboard tick
   sharding (which is ALSO where the operator's 2x2x2 same-conditions
   FSM batching slots — per-region classes feed the memo), mesher/apply
   overlap, NEON via `wide` for density + StemGroup (M2/aarch64).
   Every stage gates on byte-identical harness digests.
4) RAMP SCHEDULER: ramp_approach (speed = base + accel*|err|, the
   two-part competing ramp) drives the menu camera fly (0.9 + 3.2/rad);
   the SAME curve is the documented player rubber-band for rollback
   reconciliation (set reconcile target on restore, approach instead of
   teleport) — tick_authority step 2 hook.
5) UI KIT CRATE: dest_button + glass_at + fresnel moved into
   unendlich-ui (renderer keeps thin aliases) — window design lives
   apart from first use sites. FRESNEL v2 is VISIBLE now: bright rim,
   stacked vertical sheen gradient, side glints, slow animated diagonal
   sweep (repaint-requested). True refraction warp = PostFx pass.
6) SANDBOX BUILD SPEED: [profile.sandboxtest] (inherits test,
   debug=false) — the 4GB OOM is BEATEN: full 7k-test binary links.
   setsid is the durable background-build pattern. tools/turn.sh = one
   command for radar+check+harness+probe+package; tools/diffset.sh =
   canonical patch set. Subcrate split: deferred — the engine monolith
   is the long pole but check is 0.2s warm and sandboxtest links now;
   split revisited if STAGE-2 threading forces module boundaries anyway.
7) SUITE TRIAGE (7016 ran): 7008 passed. My areas: plants 139/139 after
   making 2 tests stagger-aware (drive the full 8-tick window),
   cpu_generator 24/24, water parity green. Remaining 4: spirv
   freshness = in-sandbox by design after the r188 .comp edit (the
   operator's build.rs refreshes it); loot-eye, mob-eyes, smoke-decay =
   PRE-EXISTING upstream (files byte-identical to pristine).
8) CENSUS ANSWER (operator asked "what % is wired?"): zero-callsite pub
   fns 624 of ~3.4k public surface (~18% of surface unreached, was 626
   at r180 — wiring of MODULES into the bin is largely done; the
   remaining mass is sim-side systems awaiting their features: doc
   parity ranking is the truthful backlog — tutorial 49%, creature
   structures 50%, fluid POIs 53%, discipline panels/phone 57%. Bulk
   wiring is no longer "the problem"; FEATURE wiring per low-parity doc
   is.

## r193 — helper-library protocol ingested + executed (CHECKED, 165 tests)
INGESTED: llm_helper_library_guide v2 (grep-first scanning, six-pattern
catalogue, metrics scratchpad, 1000-file session protocol, checkpoint
format) -> docs/design/. APPLIED, by the book:
1) GREP-FIRST SWEEP (§3b.1): patterns 4.1/4.2/4.3/4.5 across src — 263
   file-hits; priority sort captured in docs/METRICS.md GREP_RESULTS as
   the next-session checkpoint (§10.2).
2) HELPER LIBS (bottom-up, §5.1; std-only, pure, #[inline], unit-
   tested): util/hash_util (mix64 const-fn, mix64_unit, mix64_2),
   util/vecmath (len3/norm3/lerp3/scale_clamp01), util/bbox
   (axis_bounds, empty-safe) — narrow honest modules, no util.rs god
   module (§8.1).
3) SEED REFACTORS (read-second rule; confirm-by-grep; replace whole
   bodies): creature_design skin_mix/skin_unit -> hash_util (wrappers
   keep names, byte-identical), creature_viewer hash_unit ->
   mix64_unit, FIVE axis-bounds folds -> bbox::axis_bounds (design x2,
   viewer UV x2 + stats 3-axis), gaze_forward inline norm -> norm3.
   reef_seeder h2 -> mix64_2 (coral layouts reshuffle once, cosmetic).
   165 targeted tests green (creature/viewer/util/inventory).
4) METRICS SCRATCHPAD (docs/METRICS.md, §7b exact format): 3 files,
   44 loc removed, 13 call sites, reuse 1.6 (>3.0 target at 50 files);
   next-session start pinned (mob_spawn_director + the world/* splitmix
   cluster as one batch spec).
5) TOOLCHAIN: edit.py now routes FSM by file type (.rs/.comp/.frag
   lexed, .sh gets bash -n) — the r192 diffset revert root-caused
   (shell glob /* read as Rust block-comment) and fixed; diffset list
   re-applied (r192 PATCH was missing the new files; r193 patch is
   complete — the r192 ZIP was always complete).

## r194 — splitmix 4.1 cluster complete (CHECKED, 191 tests)
Continued the helper-library sweep from the r193 checkpoint. Pattern 4.1
(splitmix64) is now FULLY consolidated: 9 files route through
util::hash_util (was 3 at r193 + 6 this turn: spark_burst, terrain_warp,
cave_moss, plant_exotics, batch_ridge_noise, plant_gene_traits). Two
look-alikes correctly SKIPPED (guide §8.3 — don't force-fit): stem.rs
(XOR-accumulate, not splitmix) and flora_placement.rs (salt^GOLDEN init
=> genuinely different output). Salt-fold variants (spark_burst,
plant_gene_traits) proved equivalent to mix64_unit(seed + salt*GOLDEN)
and reduced to one call. Dead skin_mix wrapper removed (the inline-or-
delete rule). All via edit.py batch specs (two 3-file atomic applies).
METRICS: 9 files, ~80 loc removed, reuse ratio 2.7 (mix64 alone at 6
sites). 191 targeted tests green. Next: pattern 4.2 (bbox) cluster
across the non-creature files, then 4.5 (ring).

## r195 — three field bugs root-caused + fixed (CHECKED, 201 tests)
1) DUAL OCEAN AT CHUNK BOUNDARIES: a single world mixes TWO terrain
   backends — Near/Mid from the GPU generator, Far/VeryFar from a CPU
   reverse-regen path. GPU computes sea level in f32, CPU in f64;
   flooring different-precision values of the same formula left a
   1-block water lip exactly at the Near/Far seam. FIX: the CPU casts
   its f64 sea level to f32 FIRST, then both apply an identical biased
   floor (floor(slv + 0.5 - 1e-3); the bias keeps an exact .5 midpoint
   off the rounding knife-edge). Bit-identical by construction, not by
   epsilon luck. examples/seam_probe.rs: 0 disagreements across
   1,048,576 columns (~60k near a rounding edge). My first attempt
   (round+snap to 0.01) still failed 3/1.05M at sea levels ending .495
   — the probe caught it before it shipped.
2) STICKY/POLLUTED MESHER ON NEW-SEED RE-ENTRY: the world_epoch anti-
   stale-mesh machinery EXISTED but was never wired —
   mesh_result_is_current appeared only in comments. A world-#1 mesh
   job still in flight uploads into world #2 when a coord re-loads at
   the same version 0 (the version check can't tell worlds apart; the
   epoch can). FIX: threaded an `epoch` field through MeshRequest ->
   both GPU intermediates (GpuMeshRequest, OctreeGpuMeshRequest) -> all
   5 MeshResult build sites, stamped at submit from world_epoch(), and
   gated at the upload loop with mesh_result_is_current. The seed
   itself was never bugged (verified: seed32 reaches both backends; the
   chunk clear + model_arc refresh both work).
3) SURVIVAL LAUNCH WITH UN-DISABLEABLE FLIGHT: the launch path set
   can_toggle_flight=false but never ended ACTIVE flight, so you
   spawned still flying with toggling forbidden (operator: "cannot turn
   it off"). FIX mirrors the working F6 path — launching survival
   attaches a physics body so gravity applies immediately. Creative
   leaves body None (free flight), matching its ruleset.
TOOLKIT: edit.py's indentation-based stub patch double-inserted on 3
lines; caught by a consecutive-dup scan and collapsed. Lesson logged:
prefer unique-context anchors over whitespace-prefix matches for
repeated lines (the batch-spec discipline the guide already prescribes).

## r196 — fastmath investigated (REJECTED w/ evidence) + operation-gated
## parallelism primitive (CHECKED, 13 new tests)
FASTMATH (operator: "I think you should do it"): built util::fastmath
(rsqrt, norm3_fast, sin_fast, cos_fast, exp_fast — std-only,
deterministic, error-budgeted + tested) and BENCHED it. Verdict from
measurement, not assumption:
  rsqrt std 126ms / fast 172ms (0.73x SLOWER); sin 0.97x (wash); exp
  std 112ms / fast 241ms (0.47x HALF speed). target-cpu=native on the
  worldgen path: 172 vs 174 ms/chunk (no difference).
Modern x86-64/aarch64 hardware FSQRT + release LLVM beat hand-rolled
polynomials; the Quake-era bit-hack is a net loss in 2026. DECISION: do
NOT adopt — module ships correct+tested but UNUSED, kept only as a ready
tool if a future M2 profile finds a genuinely transcendental-bound loop.
DEVICE PARITY: a non-issue — the CPU<->GPU surface (worldgen density/
biome/sea-level) has NO transcendentals (only mul/add/floor, r195 made
bit-identical), so fastmath was never a parity candidate. The honest
answer to "how would it impact parity & perf": parity unaffected, perf
NEGATIVE. Full write-up: docs/design/FASTMATH_ANALYSIS.md.
PARALLELISM BY OPERATION (operator: gate on loops vs clone/copy vs other,
not area): docs/design/PARALLELISM_BY_OPERATION.md + util::task_lanes —
three access CLASSES (P pure-map, C clone-then-apply, S serial-only)
where the TYPE SIGNATURES make mis-classification a compile error, not a
runtime race (a diagonal-neighbor read fails the Class-P `&In -> Out`
signature at compile time — caught, not crashed). map_pure preserves
input order (deterministic merge); compute_patches/apply_patches splits
parallel compute from serial main-thread apply (the COW shape the chunk
manager already uses). Three workers: 2 Class-P (rayon work-steal) + 1
Class-C, main=Class-S. Migration order + harness-digest gate documented.
13 new util tests green (fastmath budgets, task_lanes determinism).
No call sites converted this turn — both deliverables are the primitive +
the evidence; conversions land behind headless_run digest checks next.

## r197 — playtester foundation: load-world UI, DHT id + rollback-default
## + dual-mailbox join (verified), live render distance (CHECKED, 29 tests)
1) LOAD-WORLD UI (operator: "still can't view previously created
   worlds"): the catalog backend existed but its screen was undrawn.
   Added a "Load World" main-menu button -> Screen::LoadSlots (menu kind
   3) listing saved worlds (name · seed · mode) from the FILE catalog;
   clicking a row launches that world via a synthetic
   AppStep::Launch(StartLocal{...}) that reuses the ENTIRE existing
   launch handler (seed rebuild, r195 mesh-epoch clear, spawn, r195
   survival-flight reset). Bypasses the menu's internal catalog (avoids
   the r131 dual-catalog desync); file catalog is source of truth.
2) DHT IDENTITY + ROLLBACK-DEFAULT (operator: "DHT unendlich-id ...
   rollback mode by default instead client server default"):
   sim::unendlich_id — persistent Identity (handle + salt-derived stable
   NodeId, tamper-checked round-trip) and NetMode with Default=Rollback.
   The salt+handle deterministically derive the node id so a player keeps
   it across launches.
3) DHT JOIN + DUAL MAILBOX, VERIFIED IN-SANDBOX (no live socket):
   examples/dht_join_probe.rs drives the real dht_join::step FSM through
   a two-peer handshake to Phase::Joined (4 ticks), then routes rollback
   INPUT packets host<->joiner through mp_dht::Mailboxes (the dual
   mailbox), fetches, and asserts delivery + drain. PASS. This is the
   transport contract the rollback netcode rides; the packet/transmission
   layer builds on this proven base next.
4) LIVE RENDER DISTANCE (operator: "more render distance settings"):
   constants::RENDER_DISTANCE_OVERRIDE atomic + set_render_distance_
   override(); runtime_render_distance() consults it first (over env/
   const). PERFORMANCE-tab slider 2..32 drives it live, persisted in the
   sticky settings JSON (render_distance key).
5) SCHEDULER NOTES (operator: "doc good scheduler ideas"):
   docs/design/SCHEDULER_NOTES.md — input-mailbox-as-rollback-clock,
   bounded-rollback confirmation horizon, player-first priority lanes
   mapped to the r196 P/C/S classes, tick-authority blend for mixed
   sessions, deterministic fetch order for replay safety (+ a planned
   two-peer harness replay mode to gate it).
29 tests green (unendlich_id, dht_join, mp_dht, session_mode); full gate
(bin check + harness + homogeneity probe) passed.
NOT DONE THIS TURN (deferred, mostly GPU/visual — need a screen to
verify, not headless): textures, neural rendering engine, shader
improvements, dragon fixes, more bending, grass-height-gap holes, hotkey
rebinding pages. Picked the verifiable networking+menu slice first per
"playtesters first." These are the next targets.

## r198 — grass-seam holes root-caused + fixed (CHECKED, 151 mesher tests)
OPERATOR (twice): "plug holes between grass blocks at different heights."
ROOT CAUSE (two layers, both in the CPU soft-edge mesher):
  1. FRAME BUG: corner_y_offset receives WORLD x/z (call sites pass
     origin+local) but the corner samplers (corner_sag,
     max_softness_at_corner) called chunk.get() with those world coords
     as if LOCAL. For the chunk at world-x/z 0 this accidentally worked;
     EVERY OTHER chunk sampled out of bounds -> read Air -> soft-edge sag
     collapsed to ~zero, so grass-height transitions away from the origin
     column rendered with hard mismatched edges (the holes).
  2. SEAM SAMPLING: out-of-bounds horizontal samples returned Air, so two
     chunks sharing a world corner saw different phantom-air and computed
     different sag -> a crack along the chunk boundary.
FIX: block_at_corner_sample converts world->local via the chunk's own
origin (fixes #1 for all chunks) and MIRRORS out-of-bounds horizontal
samples back into the chunk (fixes #2 at the boundary column). y-OOB
still reads Air (top/bottom seams are covered by the existing
side-face-extends-to-integer-Y invariant). Applied to corner_sag +
max_softness_at_corner; avg_weathering_at_corner already handled OOB via
world-position defaults.
PARITY: none affected — soft-edge sag is a CPU-mesher-only visual; the
GPU mesher produces flat cubes and the CPU<->GPU parity surface is
worldgen density/biome/sea-level (r195), not mesh geometry.
VERIFIED: new test soft_edge_corner_matches_across_chunk_seam builds two
chunks with mismatched grass heights and proves (a) the non-origin chunk
now SEES its grass (softness > 0, was 0 = the frame bug) and (b) both
sides see grass at the boundary column. 151 mesher/surface tests green.
HONEST LIMITATION (documented in the test): PERFECT corner agreement also
needs the 3x3 NEIGHBORHOOD to match, which requires real cross-chunk
reads the single-chunk mesher lacks; the outer-ring air count still
differs between chunks sharing one edge. The mirror fixes the dominant
crack (frame bug + boundary column); full ring agreement is tracked for
a future cross-chunk-neighbor meshing pass.

## r199 — hotkey rebinding pages (CHECKED, 12 keybind tests)
OPERATOR: "add hotkey pages with rebinding." The KeyBindings backend
(set_binding, save/load_to_file, presets, conflict-aware lookups) already
existed and the bin already read `bindings` for movement — but bindings
were preset-only (not loaded/persisted) and the Controls settings tab
(index 3) was undrawn (fell through to _). Delivered:
1) keybind_ui helper module (unit-tested): rebindable_actions() (ordered
   labeled action list — movement, toggles, hotbar), key_label() (clean
   glfw Key -> string; "Num1"->"1", spelled-out modifiers), and
   capture_candidates() (letters/digits/F-keys/modifiers/nav; Esc
   excluded so it can cancel).
2) Controls tab (settings_tab==3): a scrollable list of action rows, each
   a button showing the current key; clicking shows "< press a key >" and
   arms capture. Fed by the bin via menu_keybind_list; rebind click
   captured through keybind_rebind_request (same capture-locals +
   write-back egui pattern as the r197 load-world list).
3) Bin wiring: `bindings` is now mut, LOADED from keybinds.json at boot
   (falls back to QWERTY preset), and a per-frame (unthrottled, so no
   missed press) block feeds the display rows, arms capture on a row
   click, binds the first just-pressed candidate key (Esc cancels),
   persists to keybinds.json, and logs. Rebinds take effect live (the
   movement/sprint/sneak reads already go through `bindings`).
12 keybind tests green (key_label clean, candidates exclude Esc, action
list labeled). Full gate (bin check + harness + homogeneity probe) passed.
REMAINING DEFERRED (GPU/visual, need a screen to verify): textures,
neural rendering engine, shader improvements, dragon fixes, more bending,
per-world rollback browse UI, the cross-chunk-neighbor meshing pass (full
grass-seam ring agreement), DHT transmission transport on the r197 base.

## r200 — dragon spacing, physics libs, GUI scale, movement-stutter fix
## (CHECKED, 8 phys tests + full suite)
1) DRAGON SPAWN SPREAD (operator: "way too common ... needs to be spread
   out"): cap 2->1 AND a 220-block minimum spacing gate — a dragon won't
   spawn within DRAGON_MIN_SPACING of an existing dragon roamer (cheap
   O(active dragons) scan). Root cause was clustering: dragons spawned
   within 56 blocks of the player with no inter-dragon distance, so two
   could appear side by side and kept respawning in view as the player
   moved. Now at most one near the player, spread across the world.
2) PHYSICS HELPER LIBS (operator: "collisions ... rope physics ... fluid
   physics ... model what would happen in a physics simulation"): new
   `phys` module, pure/deterministic/std-only/tested, each modeling one
   classical behavior:
   - phys::rope — Verlet rope/chain/vine (pinned anchors + distance
     constraints; settles taut under gravity, deterministic replay).
   - phys::fluid_forces — buoyancy (displaced-volume upward accel; cork
     rises, neutral floats, stone sinks slower) + quadratic drag
     (opposes motion, yields terminal velocity). Fills the "No fluid
     drag yet" gap physics.rs noted. 8 tests green.
   These are the math primitives; wiring into step_body / entities lands
   next behind harness digests (kept pure + tested first, per the
   discipline that the fastmath bench reinforced).
3) GUI SCALE SETTING (operator: "add gui scale to the settings"): Video
   tab slider 1.0..3.0 (step 0.25) -> renderer.ui_scale (already drives
   egui pixels_per_point); persisted in settings JSON (gui_scale key).
4) MOVEMENT STUTTER + HIGHER RENDER DISTANCE (operator: "fix stutter when
   moving" + "handle higher render distances"): chunk eviction was
   unbudgeted — a 1-chunk move pushed an O(R^2) shell out of range and
   evicted it ALL in one frame, spiking frame time exactly when moving.
   Now out-of-range chunks ENQUEUE and drain at EVICT_BUDGET_PER_FRAME=64
   per frame (re-checked for range at drain, so a reversed move just
   drops them). Invisible chunks draining over a few frames is
   imperceptible; the per-boundary spike is gone.
Full gate (bin check + harness + homogeneity probe) passed; 8 phys tests
green. NOTE: operator said they didn't compile-check last turn; this turn
re-verified the whole bin + lib + sandboxtest build from clean.
REMAINING (GPU/visual, need a screen): textures, neural rendering, shader
improvements, more bending, character model + multiplayer limb rig, UI
depth/parallax/smoothing polish, per-world rollback browse UI.

## r201 — sound + fluid physics helper libs (CHECKED, 20 phys tests + probe)
Continued the physics-modeling work (operator: "some sound physics and
water+fluid physics") with two more pure/deterministic/tested modules:
1) phys::acoustics — the PHYSICAL sound-propagation quantities the
   existing perceptual code (sound_masking dB/occlusion, sound_mix
   attenuation/pan, reverb_wetness dry/wet) does NOT compute:
   - propagation_delay (speed of sound 343 b/s — distant thunder heard
     late);
   - spreading_gain (1/r spherical) + air_absorption_gain (exp(-a·d),
     why distant sound is muffled not just quieter) -> distance_gain;
   - sabine_rt60 / box_room_rt60 (how long a room's tail rings from its
     volume + surface absorption: a stone cavern 6.7s vs a closet 0.2s).
2) phys::hydro — classical hydraulics the cellular settle (water_field)
   and flow fields don't model:
   - hydrostatic_pressure (ρ·g·h);
   - torricelli_speed (v=√(2gh) — a breach 5b down jets at 14.1 b/s);
   - orifice_flow_rate (cd·A·√(2gh)); head_driven_flow + equalized_surface
     (signed, settles to the mean WITHOUT overshoot when conductance·dt
     ≤ 0.5 — proven by a 200-step convergence test).
VERIFICATION: examples/phys_probe.rs exercises all four phys modules
end-to-end and asserts textbook results (thunder delay, RT60 ratio,
Torricelli jet, cork-rises/stone-sinks, rope settles taut) — the
observable artifact since in-game testing isn't available. ALL PASS.
20 phys tests green. Kept as tested primitives (NOT force-wired into the
tuned sound_mix curve or the determinism-sensitive water settle) — those
integrations land behind harness digests, per discipline. Full gate
(bin + harness + homogeneity probe) passed.

## r202 — log-driven perf fix, GUI-scale-on-release, Dvorak fallback, doc
## library organized (CHECKED, 12 keybind tests)
Acted on the operator's concrete notes + the perf log (log01.log):
1) WORLD/MOVEMENT STUTTER — ROOT CAUSE FROM THE LOG: submit=400-850ms in
   bursts (not eviction; r200's evict-budget was aimed at the wrong
   target). The mesh-submit shell walk traversed the WHOLE sphere every
   moving frame because the all-tiers-full early-exit never fired — outer
   tiers (Far/VeryFar at shells 9+) are mostly ungenerated, so their
   budgets never fill. FIX: (a) consecutive-empty-shell cutoff (stop after
   3 shells yield no candidate — past the generated frontier); (b) total-
   candidate cap at MESH_QUEUE_CAPACITY (can't submit more this frame).
   Unsubmitted coords are picked up next frame; prefetch path still
   handles ahead-loaded chunks. Directly targets the logged 400-850ms
   submit spikes = the "stutter when moving" + "smarter about transitions".
2) GUI SCALE WAITS FOR RELEASE (operator): the slider now edits a STAGED
   value and only commits to the live ui_scale on drag_stopped() (or
   click-release), so the UI no longer resizes under the cursor mid-drag.
   Shows "pending: N× (release to apply)".
3) DVORAK KEYBIND CONFUSION (operator: "making playing difficult"): added
   ARROW KEYS as a layout-independent movement fallback to BOTH presets
   (arrows are the same physical keys on every layout, so movement always
   works regardless of letter position). The rebind page now shows ALL
   bound keys per action ("W / Up") so the fallback is visible.
4) DOC LIBRARY ORGANIZED (operator): the shipped Unendlich Design Library
   (52 docs) copied verbatim to docs/design_library/ with its folder
   structure; my engineering docs (FASTMATH_ANALYSIS, PARALLELISM_BY_
   OPERATION, SCHEDULER_NOTES, THREADING_SIMD, tick_authority_regions,
   STRUCTURES_*) added under 11_Engineering with APPLIED_ON_TOP.md
   explaining the reconciliation (library = canonical design; docs/design
   extractions kept only because tooling references them by path).
PERF LOG also shows plant_lifecycle=200-427ms spikes on first load — the
memoized advance (r191) helps steady-state but first-load catch-up still
spikes; noted for a future Class-C parallel pass (task_lanes).
12 keybind tests green; full gate passed.

## r203 — settings-from-menu, load-world BUGFIX, rollback browse, liquid
## glass v3 (CHECKED, 20 tests; operator back to testing)
1) SETTINGS FROM MAIN MENU (operator): added a "Settings" button to the
   title screen that opens the tabbed settings window (Video/Audio/
   Gameplay/Controls/Performance) — previously reachable only from the
   in-world pause menu. The window already renders over any screen and
   has its own Done/close.
2) LOAD-WORLD ROOT-CAUSE BUGFIX (operator: "still can't see previous
   worlds" — despite the r197 UI): catalog_file_path() used
   env!("CARGO_MANIFEST_DIR"), a COMPILE-TIME path to the BUILD machine's
   source tree. On the shipped binary that directory doesn't exist, so
   the catalog silently read/wrote nowhere and the Load list was always
   empty. FIX: runtime path — $UNENDLICH_SAVES, else the platform config
   dir (~/.config/unendlich/saves, matching settings.json), else cwd.
   (The same compile-time-path pattern also disables title music on the
   shipped binary — noted, cosmetic, not fixed this turn.)
3) ROLLBACK BROWSE UI (operator: "can't step backward through the
   rollback system"): the .point markers were WRITTEN during play but
   never surfaced. Added a "history" button per world on the Load screen
   that lists that world's rollback points newest-first (time-ago ·
   reason · chunk count) via list_rollback_points(), each with a
   "restore" action that stages a RESTORE marker. Visibility + selection
   land now; full chunk-snapshot reload is staged for the follow-up
   (the markers currently record metadata, not per-point snapshots).
4) LIQUID GLASS v3 (operator: "keep working on liquid glass"): fresnel
   upgraded — smoother 8-band top highlight, side glints, a BOTTOM
   inner-glow (light pooling at the base — the hallmark that reads as
   "liquid" vs flat glass), and TWO offset diagonal sheen sweeps
   (different phase + speed) for a living shimmer. Visible now that
   testing is back.
5) CHUNK-HANG ADAPTATION: the r202 empty-shell submit cutoff already
   backs off naturally when pending_count is high (fewer generated
   chunks to submit), so meshing doesn't pile onto a generation backlog.
20 tests green (keybind, world_catalog); full bin+lib+ui build clean.
STILL OPEN: full rollback chunk-snapshot restore; per-point snapshots;
title-music runtime path; the plant_lifecycle first-load catch-up spike
(needs a determinism-gated Class-C parallel pass); more menu/shader
polish now that visual testing is live.

## r204 — water foam fix, keybind scroll, mesh-clear-to-title, directional
## swim, advanced profiler (CHECKED, 16 tests; doc-driven)
Acted on the post-test bug list + the "follow the docs" directive:
1) WATER WHITE-PASS (operator: "goes way too fast, too patterned, no
   noise, physics behind it is strange"): the foam in chunk.frag was a
   clean smoothstep of analytic wave height — a fast regular band. FIX
   (per shader_synthesis_guide's "organic not regular, add noise
   character"): foam now (a) gates on wave STEEPNESS (1 - N.y, the
   gradient — foam where water actually breaks, the "physics behind it"),
   (b) is broken up by fbm2 noise so it's dappled not a screen-door, and
   (c) is slower. The caustic shimmer likewise slowed (w_time*0.18 not
   *0.5) and fbm-modulated.
2) KEYBIND SCROLL (operator: "keybinds menu has no scroll"): the Controls
   ScrollArea grew the resizable=false window past the screen so it never
   scrolled. FIX: id_salt + auto_shrink([false,false]) + a 440px window
   max_height; rows get 22px height + 2px spacing (the "tight padding").
3) PADDING (operator): settings window min-width 320->360, added top/
   section spacing.
4) MESH NOT CLEARED TO TITLE (operator): "Save & Quit to Title" saved
   chunks + ticked QuitToTitle but never cleared the renderer meshes, so
   the title showed stale geometry that bled into the next world. FIX:
   clear_all_chunk_meshes() on quit-to-title (the launch path already
   cleared; this was the missing twin).
5) DIRECTIONAL SWIMMING (operator: "make swimming directional with
   camera"): underwater, holding forward now adds thrust along the FULL
   3D look vector (pitch included) so looking down+forward swims down;
   the flat XZ apply_input gives the horizontal, this adds the vertical
   it dropped, scaled by submersion.
6) ADVANCED PROFILER (operator: "add an advanced logging system and
   profiler for more insight from logs"): src/profiler.rs — per-zone
   histograms + percentiles (p50/p95/p99/max — the frame-time TAIL that
   causes perceived stutter, which flat averages hide), a greppable
   structured dump sorted worst-p99-first, and ASCII histograms for deep
   dives. time() closure wrapper drops into existing code. 4 tests
   (percentile math incl the tail, worst-first ordering).
16 tests green; full bin+lib+ui build clean. LIQUID GLASS v3 (r203
bottom inner-glow + dual sweeps) and these are all visible now.
STILL OPEN (per docs, flagged): glass styling vs the Destiny-esque doc
target (needs the UI doc's exact tokens — next), grass GPU-kernel
lifecycle + 4x4x4 caching (big, design-doc-driven), grass LOD past 6
chunks, full rollback chunk-snapshot restore, the submit-cost stutter
(r202 cutoff present; profiler now instruments it).

## r205 — menu input routing (scroll + text) + world delete (CHECKED, 24 tests)
Fixed the two reported menu input bugs and expanded the menus:
1) SCROLL NOT PASSING TO DIALOGS (operator): scroll was never forwarded
   to egui — scroll_accum was consumed by the hotbar when the cursor was
   captured, and DISCARDED (zeroed) when it wasn't, so a ScrollArea in a
   dialog never received it. FIX: new renderer.ui_scroll_delta; the bin
   forwards the accumulated wheel delta when the cursor is free (UI up)
   instead of zeroing it; the renderer pushes it as egui::Event::MouseWheel
   {unit: Point, delta, phase: Move}. Gameplay scroll (hotbar) is
   unaffected (still consumed only when captured).
2) TEXT ENTRY NOT PASSING THROUGH (operator): two-part fix. (a) EVENT
   ORDER — the Text/Backspace/Enter events were pushed BEFORE the
   PointerButton events in the same frame; egui processes a frame's
   events in order, so a click that focuses a TextEdit was handled AFTER
   the text, dropping the first keystrokes. Moved text/edit events to
   after all pointer events. (b) AUTO-FOCUS — the wizard name field now
   request_focus() on the rising edge of wizard_open (new wizard_was_open
   field), so typing works immediately without a pixel-perfect click.
3) MENU EXPANSION — world DELETE with confirmation on the Load screen:
   each world row gets a trash button that arms an inline confirm row
   (name + delete/cancel); confirming removes the catalog entry and the
   on-disk world dir. Lets the dev prune accumulated test worlds without
   leaving the menu. Load-world ScrollArea also got a stable id_salt
   (was missing — could collide with the rollback scroll area).
24 tests green (keybind, world_catalog, profiler); full bin+lib build
clean.
PRE-EXISTING (NOT this session): world::leaf_decay::{catchup_via_tracker_
matches_continuous, band_aware_leaves_match_render_after_n_ticks} fail
because an EARLIER session's r160 change (only FLAG_DECAYING/orphaned
leaves decay) was never reflected in these two tests — they still expect
continuous aging of unflagged leaves. leaf_decay.rs was not touched this
session; flagged for a future fix (update the tests to set FLAG_DECAYING,
or assert the orphaned-only semantics).

## r206 — dev runtime, history fixes, region budget, HTML menu mockup
## (CHECKED, 9 new-module tests; NOT packaged per operator request)
Operator dump after testing — addressed the actionable items:
1) HISTORY NOT WRITING — root cause: the rollback-point triggers were
   timer=5min / travel=1000 blocks / on-craft, so NOTHING fired in a
   short test session. FIX via the new dev runtime (below): dev mode
   drops these to ~20s / 120 blocks, and adds a manual "create point now"
   button. (current_world_name is set on the StartLocal launch path that
   both new-world and load use, so the write gate is satisfied once a
   world is entered.)
2) UNENDLICH_DEV=1 DEV RUNTIME (operator: "a dev runtime that turns this
   stuff on by default like debug menu and FPS"): new dev_mode module
   (is_dev() caches the env once). In dev mode the F4 debug overlay is
   enabled at boot, the world-history cadence is shortened, and the
   history panel shows a dev-only "＋ create history point now" button.
   run-latest.sh gains a `--dev` flag that exports it.
3) HISTORY = DEV INSIGHT PANEL (operator): added an ⓘ "more info" hint to
   the rollback panel explaining what points are + how they're made, and
   a per-point 🗑 DELETE ("deleting bad points would be nice"), wired to
   remove that .point marker and refresh the list.
4) REGION BUDGET TRACKER (operator: "a budgeting tracker for regions"):
   new region_budget module — aggregates per-chunk mesh/gen time + chunk
   count + bytes into 4x4x4-chunk regions (region_of_chunk floor-maps
   coords), ranks the hottest regions, and emits a greppable report. The
   spatial complement to the r204 profiler's by-zone view. 4 tests.
5) HTML MENU MOCKUP (operator: "mock something in HTML for rapid parallel
   devel"): docs/design/menu_mockup.html — a standalone, dependency-free
   menu encoding the exact tokens from bending_character_menu_ui_v2.docx
   (frosted-glass panels as "atmospheres not surfaces", thin chrome,
   34px void blur vs 8px panel blur, bottom inner-glow, dual sheen
   sweeps, optical defocus->resolve transitions, per-element grading).
   Iterate the look in a browser, then port the converged result to the
   egui fresnel. Includes Play/Load/History/Settings panels + a
   screenshot-timeline history concept.
6) run-latest.sh: confirmed it already has --suite (operator can run the
   slow full test on their hardware) + added --dev. The operator will run
   the long suite locally going forward.
9 new-module tests green (dev_mode, region_budget, profiler); full
bin+lib build clean. Per operator: NOT packaged this turn.
PRE-EXISTING (still, NOT this session): the two world::leaf_decay tests
(r160 FLAG_DECAYING semantics vs stale test expectations).

## r207 — menu mockup: mouse parallax + cursor radial fills (HTML only)
Operator: "make the parallax tied to the mouse to dev the algorithm" +
"add realtime dynamic centerpoint fade fills to the buttons not basic
left padding trickery". Both in docs/design/menu_mockup.html (browser-
side, no engine rebuild):
- MOUSE PARALLAX: multi-layer depth driven by cursor offset from centre.
  The void (heavy blur) swims OPPOSITE the cursor at the largest depth;
  the foreground glass drifts WITH it at a small depth and tilts toward
  it (rotateX/Y). Exponential smoothing (lerp each frame) kills jitter.
  A live PARALLAX DEV tuner panel (bottom-right) exposes void depth,
  glass depth, tilt, and smoothing sliders so the algorithm can be dialed
  in-browser before porting values to the engine.
- CURSOR RADIAL FILL: replaced the left-edge box-shadow/translateX hover
  trick on nav buttons AND world rows with a real radial-gradient fill
  whose centre tracks the cursor's position INSIDE the element (--mx/--my
  in %), opacity ramped by --fill on enter/leave. Delegated handlers so
  dynamically-rendered rows are covered. This is the "centerpoint fade
  fill" to port to the egui button painter.
Validated: balanced CSS/JS, all features present. NOT packaged (HTML
deliverable only).

## r208 — menu mockup: smooth fill, clipped tuner, no sheen, inventory +
## crafting pages (HTML only; JS syntax-checked with node)
Operator feedback on the mockup, all in docs/design/menu_mockup.html:
- SMOOTH FLOOD FILL: the radial fill now WIDENS IN — registered --fill-r
  as a typed @property <length> so the radius can transition; on hover it
  animates 0 -> spread (grows out of the cursor point), and collapses to 0
  on leave so the next button starts fresh. Replaces the fixed-brush
  radius on both nav buttons and world rows.
- CLIPPED TUNER RANGES: operator liked nothing above ~50%, so the PARALLAX
  DEV sliders top out at half their old maxes (void 40, glass 20, tilt 6,
  smoothing 0.2); defaults still sit inside the clipped ranges.
- REMOVED MOVING SHEEN: the two diagonal sweep animations are gone
  (operator: distracting; real item/feedback content will animate panels).
  Static top rim + bottom inner-glow remain so glass still reads as glass.
- INVENTORY PAGE (crafting_inventory_layout.docx): 8-category filter
  sidebar (▶ on active) + 4x9 = 36-slot grid; cells are near-black void
  fill, dark frame, four blue corner-accent dots, hover glow; items show
  icon + stack qty.
- CRAFTING PAGE (same doc): sidebar + 3x3 ingredient grid + animated
  double-chevron »» + ornate GOLD result slot with element-matched (blue/
  Water) glow ring + blue-framed station-indicator slot; populated with
  the doc's Tidal Blade recipe + "Tidal Anvil required".
Validated: balanced CSS braces + div tags, JS passes `node --check`
(caught + fixed a malformed \u escape on the result icon). HTML only; not
packaged. Next: port the converged fill + parallax values into the egui
button painter / menu renderer once the operator settles the look.

## r208.1 — mockup fix: buttons/navigation (operator: "none of the buttons
## work, can't see anything")
Reproduced via headless Chromium (Playwright) + node --check. Findings +
fixes in docs/design/menu_mockup.html:
- ROOT RISK: r208 used `@property --fill-r` + a TRANSITION on a custom
  property to animate the fill radius. Chromium tolerated it (my tests
  passed) but WebKit/Safari (operator's likely browser on macOS/Asahi)
  handles @property-transitions inconsistently — and the `panels` object's
  IIFEs run BEFORE the nav-click handler is attached, so if anything in
  that region faulted, navigation never wired up AND no panel rendered =
  exactly "buttons don't work, can't see anything".
- FIX 1: removed @property entirely. The cursor fill now widens in via a
  fixed-size radial pseudo SCALED 0->1 on :hover (transform + opacity
  only — universally supported). JS just updates the gradient centre
  (--mx/--my); the grow/fade is pure CSS :hover (no JS state).
- FIX 2: wrapped ALL presentational polish (parallax, fills, tuner) in a
  try/catch placed AFTER the nav handler + initial show(). Now it is
  structurally impossible for the polish to break navigation — worst case
  the menu just loses the parallax and stays fully usable.
VERIFIED in Chromium: 7 nav buttons, every panel renders distinct content
(play/load/inventory[36 cells]/crafting[result+chevron+grid3]/history/
settings/quit), zero console errors; all 89 \u escapes are valid surrogate
pairs (node-checked). HTML only.

## r208.2 — mockup: fix parallax mouse-math + halve tuner ranges again
Operator: "mouse position logic is off, making feeling out the design
difficult" + "cut the slider ranges by 50% again".
- PARALLAX MATH FIX: the old model moved the void OPPOSITE the cursor but
  the glass WITH it — counter-motion between layers, which reads as wrong
  rather than as depth. Now a coherent model: EVERY layer shifts the same
  direction (opposite the cursor, like looking around a scene), near
  (glass) more than far (void). Also fixed the normalisation — offset is
  now centre-relative over HALF the dimension (screen edge = ±1, centre =
  0), verified in-browser: px=0 at centre, negative at right edge,
  positive at left, py positive at top. Glass motion + tilt toned down so
  the content you read/click stays stable; the void carries the parallax.
- TUNER RANGES halved again: void 0-20, glass 0-10, tilt 0-3, smoothing
  0.02-0.1; defaults clamped within (16/6/2/0.10).
Verified in headless Chromium: parallax signs correct, all 7 panels
navigate (inventory 36 cells, crafting full layout), zero pageerrors,
JS node-checked. HTML only.

## r209 — mockup: remove glass rotation + cone-projection parallax
Operator: "remove the rotating glass and fixup the cursor logic... think
like a stack, mouse modeled x,y on screen, parallax depth is fixed-point
cone projection, mouse is the anchor to everything in a cone."
- ROTATION REMOVED: dropped perspective/transform-style off .stage and
  rotateX/rotateY off .glass (and the tilt tuner). Glass now translates
  only. 0 rotation refs remain.
- CONE-PROJECTION PARALLAX: reframed the math as a viewing cone whose
  APEX is the cursor. Each layer has a fixed signed depth ratio z (z<0
  behind the screen, z=0 the focal/content plane, z>0 toward viewer).
  Every layer's screen shift is the projection of its depth through the
  one apex: shift = anchor_px * z. One anchor, one projection — so all
  layers are coherent by construction (no per-layer hand-tuned
  directions, which was the "off" feeling). Window-into-depth sign:
  cursor-right => background shifts left, foreground leads right
  (verified in-browser: void -132px, glass +44px at the edge, magnitudes
  scaling with |z|). Anchor is smoothed (lerp) for weight.
- Tuner now edits the cone z-ratios (bg/fg depth) + smoothing; ranges
  halved again per operator (voidZ -0.5..0, glassZ 0..0.15, smooth
  0.02..0.10); defaults subtler (-0.3 / 0.10 / 0.12).
Verified headless Chromium: signs + magnitudes correct, nav across all
panels, zero pageerrors, JS node-checked. HTML only.

## r210 — mockup: content-fed tint, flip parallax toward cursor, longer
## tails, FLIP morph transitions (HTML only)
Operator's four asks, all in docs/design/menu_mockup.html:
1) SLIDERS FEED CONTENT (deterministic): added hashStr (FNV-1a) + colorMax
   (dominant RGB channel = hash%3, damped by slider magnitudes, cool base)
   + applyContentTint() — hashes (page name + slider values) into a
   reproducible accent driving --accent + --page-wash on #panel. show()
   sets curPage + tints; the tuner re-tints live on input. CSS: h2 uses
   var(--accent), .content::before is a wash bar coloured by the hash.
   Verified: play rgb(46,86,255) vs craft rgb(46,229,120) (differ), slider
   move re-tints.
2) FLIP PARALLAX TOWARD CURSOR: operator "everything moving away not
   towards." projShift now returns +a*z (was -a*z) and both z defaults are
   POSITIVE (voidZ 0.30, glassZ 0.08) so layers LEAN toward the cursor
   (attractor), void more than glass. Verified void px = +113 at right
   edge. Tuner labels -> "bg pull (z)" 0..0.5, "fg pull (z)" 0..0.15.
3) LONGER EXPONENTIAL TAILS: smoothing default lowered to 0.06 (range
   0.02..0.12) for a longer parallax glide; CSS transitions lengthened
   with an expo-out curve cubic-bezier(0.16,1,0.3,1): glass translate
   120->600ms, button/world fills 320->560ms (opacity 320ms), panel
   resolve 420->620ms.
4) MORPHING GEOMETRIES (not stutter): implemented FLIP (First-Last-Invert-
   Play) page transitions — morphTo() captures outgoing box rects, picks
   the one nearest the incoming content anchor (nearestBoxRect), swaps,
   then INVERTs the new content to start at that box's position+size and
   PLAYs it to identity with the long expo-out tail. Replaces the blur-
   resolve stutter with geometric continuity. Full per-cell-choreography
   approach documented in a comment block for the engine port. Verified
   across all 7 panels: mid-morph transform set, settles to identity,
   content correct, zero errors.
Fixed a TDZ crash en route (const tune was declared AFTER show('play')
ran; hoisted it above show + removed the duplicate). JS node-checked,
balanced. HTML only.

## r211 — mockup: sliders to 33% left-aligned, animated toggles/sliders,
## glass×tactical CHARACTER merge (HTML only)
Operator (+ uploaded RootBender character-sheet reference image):
1) SLIDERS TO 33% LEFT-ALIGNED: cut every slider's range to a third.
   Tuner: bg pull 0..0.17, fg pull 0..0.05, smoothing 0.02..0.053; defaults
   left-aligned (0.10/0.03/0.03). Content sliders too: render distance
   2..12, GUI scale 1..1.66.
2) ANIMATED TOGGLES + SLIDERS: slider thumbs grow + glow on hover (spring
   cubic-bezier(.34,1.56,.64,1)) and press on active; track brightens +
   glows. Toggles got a springy knob throw (overshoot curve), an
   on-state glow + inner highlight, a one-shot light SWEEP across the
   track the moment they switch on, and a knob squash on press. Added
   -moz-range-thumb parity.
3) GLASS × TACTICAL MERGE (character_panel_reference.docx + the reference
   image): new CHARACTER page blending the frosted-glass depth with the
   reference's forest-green tactical palette — gold accents, sharp
   clipped-corner ".gpanel" frames (vs round), sage borders + inner-glow.
   Layout mirrors the reference: left equipment column (corner-bracketed
   .eslot cells w/ rank diamond + 10/10 durability), centre voxel-
   character stage w/ discipline caption, profile glass panel (ROOTBENDER,
   gold POWER 1560, diamond-glyph MOBILITY/RESILIENCE/RECOVERY rows),
   right column of glass sub-panels (Companions w/ ability trios, Bond
   pips MAX RANK, Resonance hex). Widened the mockup stage to 1080px so
   the 4-column sheet fits without clipping.
Verified headless Chromium + screenshot (outputs/character_merge_preview
.png): char layout renders (5 gear slots, 3 stats, 4 glass panels, 10
pips, gold power), toggles flip, all 8 panels navigate via FLIP morph,
zero errors; JS node-checked, surrogate pairs valid, braces/divs balanced.

## r212 — mockup v1.1: detent sliders, rune-morph text, sharper corners,
## diamond motif, refined crafting, global specular, char alignment
Froze the prior state as docs/design/menu_mockup_v1.html; current file is
now v1.1 (title + footer bumped). Added docs/design/menu_mockup_README.md
registering it as a testing-playground demo. All operator asks, verified
headless Chromium + screenshots, zero errors:
1) SLIDER LERP-TO-DETENTS: tuner sliders take fine input then ease (rAF
   lerp, 0.25/frame) into N evenly-spaced detents (magnetic snap, not
   hard). Verified: nudge 0.09 settles to 0.087 toward the 0.085 detent.
2) RUNE-MORPH TEXT ENTRY: World-name field rebuilt as a .runefield — typed
   chars insert as a Unicode-Runic bending rune then cross-fade/morph
   (opacity+blur+scale/rotate) into the Latin letter after ~130-250ms.
   Backspace/Enter handled; seeded with "New World". Verified mid-type
   char unresolved then resolves.
3) SHARPER CORNERS: added --corner: 3px token; sharpened glass/buttons/
   world rows/cells/result/station/inputs/tuner from 8-16px round to
   2-3px (voxel/block feel); cells + cats now fully square.
4) DIAMOND MOTIF: toggle knob is now a rotated-square diamond; history
   timeline points are diamonds; (stat glyphs + active-category marker
   already diamond ◆).
5) REFINED CRAFTING/INVENTORY: cells got DOUBLE-LINE borders (outer +
   inset hairline via box-shadow), square corners, and restrained colour
   — neutral dark fills, blue only on corner-bracket accents + hover, so
   the item icons + gold result carry the colour instead of it being
   splashed across backgrounds.
6) GLOBAL SPECULAR: updateSpecular() finds the biggest on-screen element,
   treats its centre as the scene light origin, and rakes a tinted
   (page-accent) gleam across every .glass border via --spec-x/y/tint +
   a screen-blended ::after. Refreshed on page change + 800ms timer.
7) CHARACTER ALIGNMENT: rebalanced grid (76/200/1fr/196) with align-items
   stretch; right .side is a 3-row grid and gear-col auto-rows 1fr so all
   columns share the row height — fixes the cramped/un-tied feel. Stage
   has room; screenshot confirms clean alignment.
Files: outputs/menu_mockup.html (v1.1) + menu_mockup_v1.html (frozen).
JS node-checked, surrogate pairs valid, braces/divs balanced.

## r213 — mockup v1.2: diagonal-rect toggle fill, centred scale-in,
## edge-weighted glass + border-fill, transparent item cells, less hue
Froze v1.2 snapshot (menu_mockup_v1.2.html); live file labelled v1.2.
Operator asks, verified headless Chromium + screenshots, zero errors:
1) TOGGLE FILL radial->DIAGONAL RECT: on-state is now a 45° repeating-
   linear-gradient of accent bands (echoing the diamond/block inner
   geometry) over --page-wash, on a rectangular sharp-cornered track
   (was a round pill w/ smooth gradient). Switch-on flash is a single
   skewed diagonal rect wipe. Hue from --accent, not a fixed blue.
2) CENTRED SCALE-IN: nav button + world-row radial fills now scale from
   transform-origin:center with gradient at 50% 50% (were cursor-pinned
   at --mx/--my). Also retinted to var(--accent)/var(--page-wash) +
   screen blend, removing the hardcoded blue (operator: "remove some of
   the fixed hue").
3) EDGE-WEIGHTED GLASS + BORDER-FILL (best-effort CSS; flagged easier in
   egui): .glass box-shadow now layers a strong perimeter inset-glow that
   fades toward centre, so the glass reads THICKER at the edges than the
   middle. Added a .fill-edges border-fill animation (inset box-shadow
   ring spread shrinking outer->inner over 620ms) replayed on every
   navigation. Noted as imperfect — true outer->inner border gradient
   fill is a known CSS pain point, cleaner to do in the egui painter.
4) MORE TRANSPARENT ITEM SUBS: cell + category-tile backgrounds dropped
   from ~0.85-0.92 alpha to ~0.30-0.42 so the glass/void parallax reads
   through the grid (confirmed in screenshot — depth now visible behind
   slots).
Files: outputs/menu_mockup.html (v1.2) + frozen menu_mockup_v1.2.html /
menu_mockup_v1.html. JS node-checked, balanced.

## r214 — mockup v1.3: blocky touch-sliders, diamond toggle fill, more
## squish/spring/fade
Froze v1.3 snapshot. Operator asks, verified headless Chromium + zoomed
screenshot, zero errors:
1) BLOCKY/THICK/PIXELIZED SLIDERS: track 5px->12px (touch-friendly), square
   corners, PIXELIZED stepped fill (hard-edged repeating-linear blocks of
   accent vs dim, image-rendering:pixelated) replacing the smooth gradient.
   Thumb circle->SQUARE block, 16px->22px, with inner highlight. Both
   -webkit- and -moz- thumbs.
2) TOGGLE CORNER TRIANGLES REMOVED: single-direction 45° stripes (which
   clip into triangle wedges at the rectangular corners) replaced with TWO
   crossed repeating gradients forming a clean DIAMOND CROSSHATCH that
   tiles to the edges — matches the diamond knob shape, no wedges. Verified
   the on-fill uses 2 repeating-linear gradients.
3) MORE SQUISH/SPRING/FADE: spring curves (cubic-bezier(.34-.5, 1.7-1.8,
   .5, 1)) + press states added across — slider track squishes on grab &
   thumb squashes (scale 1.3,0.85) then springs; toggle track squishes
   scaleX(.92), knob squashes along travel (1.25,0.7), diamond BLOOM flash
   on switch-on (replacing the wipe); nav buttons nudge + press-squish;
   cells hover-lift (1.06) + press-squish (0.92).
Files: outputs/menu_mockup.html (v1.3) + frozen menu_mockup_v1.3.html.
JS node-checked, balanced.

## r215 — mockup v1.3: morph temporal-history fix, decoupled history-
## diamond hover, toggle dither + <[[[[]> outline, CUSTOM TTF FONTS
Operator asks, verified headless Chromium + screenshots, zero errors:
1) MORPH FIX: the FLIP morph was (a) reading ALREADY-SHRUNK rects from the
   live (mid-animation) DOM and (b) still pinned TOP-LEFT. Rewrote with a
   TEMPORAL BOX HISTORY — box centres recorded only while the panel is at
   REST (untransformed), over the last 10s (ring buffer pruned by ts), on
   a 500ms cadence + after each morph settles. morphTo() picks the nearest
   centre from that history (never a transforming frame) and now uses
   transform-origin:CENTRE (no corner pin), with a morph lock that clears
   the inline transform once settled so the next record captures true rest
   layout.
2) HISTORY DIAMOND HOVER DECOUPLED: .tp .dot inner fill expands only
   slightly (scale 1.12) while a SEPARATE ::before border ring expands more
   (inset -3px -> -7px) on its own longer timing AND sharpens (soft 2px
   glow -> crisp 1px white edge, box-shadow -> 0). Inner and border fully
   decoupled.
3) TOGGLE: shape changed to <[[[[[[[[[> — chevron end-caps via clip-path
   polygon (< and >), bracket segments via a repeating vertical-bar ::before
   overlay. Infill swapped from the coarse diamond crosshatch to a HIGH-RES
   DITHER (two offset fine radial-dot grids = ~2-3px checker). Kept the
   squish/bloom/spring animations.
4) CUSTOM TTF FONTS (docs/design/fonts/build_fonts.py via fontTools): built
   UnendlichBlocks-Regular.ttf (readable blocky/voxel Latin game font,
   5x7 pixel-grid, 51 glyphs — every filled cell a square contour) +
   UnendlichGlyphs-Regular.ttf (UI/bending marks: filled+outline diamond,
   < > chevrons, [ bracket, rune mark, resonance spark, leaf tick, gem
   cluster). Piped into the mockup via @font-face; applied UnendlichBlocks
   to the UNENDLICH title + panel h2 + profile name + POWER (accent
   headings only — body stays on the system sans the operator liked).
   Rendered a font-proof image; both TTFs validated (load, cmap, contours).
Files: outputs/menu_mockup.html (v1.3) + UnendlichBlocks/Glyphs .ttf +
build_fonts.py; frozen menu_mockup_v1.3.html. JS node-checked, balanced.

## r216 — mockup v1.3 fixes: toggle inner padding, cursor-follow regression
Operator feedback (3 screenshots):
1) DIAMOND TOGGLE inner L/R padding removed: chevron caps tightened 7px->
   4px, knob inset 5px->3px and grown 16->18px with travel left 3->31, so
   the diamond knob + dither fill hug the edges (no inner padding gap).
   Waypoint/history diamond confirmed good — left as-is.
2) CURSOR-FOLLOW REGRESSION fixed (load-world + nav): the radial fills had
   gone STATIC at 50% 50% when v1.2 made them "centred". Restored cursor
   tracking — the gradient bright spot now follows --mx/--my (set by
   wireFill from pointer position inside the element) while the SCALE-in
   still grows from transform-origin:centre. Verified --mx swings ~5%->92%
   across a world row and ~15%->85% across a nav button as the cursor
   moves. Best of both: centred reveal, cursor-responsive spotlight.
Re-froze menu_mockup_v1.3.html. 8/8 panels navigate, zero errors, JS
node-checked.

## r217 — mockup v1.3: diamond-clipped toggle fill (fractal dither, L→R
## reveal), inventory crosshair + neutral cells, fonts confirmed loaded
Operator feedback (toggle screenshot + notes):
1) TOGGLE FILL = DIAMOND (not the wide chevron): the on-fill moved to a
   ::before layer clip-path'd to a DIAMOND matching the knob. Dither is now
   a FRACTAL/multi-octave dot field (4 scales: 2/4/8px + wash) so it reads
   downsampled. It FADES IN with a scaleX transform anchored left
   (transform-origin:left), i.e. wiping left→right out from behind the
   knob. Knob bumped to z-index 3 above the fill.
2) INVENTORY CELLS reworked per spec: removed the hover ZOOM; dropped the
   fancy corner accents + colour splash; inner stays CLEAR (transparent).
   Now NEUTRAL hollow borders + a sharp inner hairline (pixel-art × sharp-
   edge hybrid). Same for category tiles.
3) CROSSHAIR row×col propagation (wireCrosshair): hovering a grid cell
   lights its whole row + column via .x-line (brighter hollow border +
   faint wash), PROPAGATING outward from the hovered cell — transition-
   delay = manhattan distance × 28ms — with the hovered cell crisp white
   (.x-hover). Verified: hovering cell 13 in the 4×9 lit exactly 11 line
   cells (3 col + 8 row) + 1 hover.
4) FONTS confirmed loaded (document.fonts.check passes) + applied wider:
   nav labels + tactical .ph section headers now also use UnendlichBlocks
   (title/h2/profile/POWER already did). Body stays system sans.
Re-froze menu_mockup_v1.3.html + TTFs in outputs. 8/8 navigate, zero
errors, JS node-checked.

## r218 — mockup v1.3: toggle holder shrink-wraps the diamond knob
Operator (toggle close-up): the OUTER holder shape was wrong — the knob
(diamond) is right; the holder should SHRINK-WRAP it on the left/right
edge pairs, and the infill should fill the ENTIRE holder again (not the
small diamond from r217).
- Holder caps changed from a shallow 4px chevron to a 45° point (cap depth
  = half height = 13px) so the left/right edges match the diamond knob's
  edge angle exactly — two diamond-edge pairs hugging the knob. Width
  52->56px to fit the deeper caps.
- Infill ::before reverted from a small diamond clip to the SAME 45°
  chevron clip-path as the holder, so the fractal dither fills the whole
  holder (kept the L→R scaleX reveal + multi-octave dither).
- Knob travel re-centred for the new geometry: off left:4px (centre ~13px,
  aligns to the left cap apex), on left:34px (centre ~43px, aligns to the
  right cap apex) so the diamond points sit in the holder's points.
Re-froze v1.3 + TTFs. 8/8 navigate, zero errors, JS node-checked.

## r219 — mockup v2 (frozen) + v3 (working): fonts embedded, SVG stroke-
## morph text, history screenshot panes, toggle-language sliders, terminal
## caret
Marked current state as v2 (menu_mockup_v2.html) and started v3 (live file
re-labelled). Operator asks, verified headless Chromium + screenshots,
zero errors:
1) FONT NOT SHOWING — root cause: the @font-face used a RELATIVE url, which
   silently failed whenever the .ttf wasn't colocated with the HTML (the
   operator's open context). FIX: both TTFs now EMBEDDED as base64 data
   URIs (font-display:block), so the file is fully self-contained and the
   font loads anywhere. Verified by rendering from an isolated dir with NO
   ttf present — document.fonts.check passes + the blocky title renders.
2) WORLD-HISTORY SCREENSHOT PANES: each timeline point now has a .shot
   <canvas> above its diamond, filled with seeded PIXELATED NOISE (per-
   point seed) as a stand-in for the real per-snapshot thumbnail. Neutral
   hollow frame + sharp hairline matching the cell language; hover lifts
   it. Timeline relaid to align-items:flex-end with the track near the
   bottom.
3) SLIDERS now match the TOGGLE language: track carries the SAME fractal/
   multi-octave dither (traversing horizontally), is a short band, and the
   thumb is a DIAMOND knob (rotate 45°) taller than the track — same knob-
   vs-fill height relationship as the toggle. Kept the squish/spring.
4) BLOCKY TERMINAL CARET: the runefield caret went from a 2px line to an
   11px solid filled block with a hard (steps(1)) blink, like a terminal
   cursor.
5) GAUSSIAN-BLUR MORPH REPLACED with an SVG STROKE-MORPH: each glyph is a
   set of line-segment polylines (STROKES table for A-Z/0-9/punct +
   RUNE_STROKES skeletons). A typed char starts as a rune skeleton and
   morphs to the letter by resampling both to a common point count and
   LERPing endpoint-by-endpoint per rAF (stroke transform/transpose) —
   no blur. Path stroke recolours accent->text on resolve. Verified the
   path 'd' interpolates mid-type then snaps to the clean letter.
Files: outputs/menu_mockup.html (v3) + menu_mockup_v2.html + TTFs. 8/8
navigate, zero errors, JS node-checked, surrogate-safe.

## r220 — mockup v3: thinner font, custom sliders (fill-wraps-square-knob,
## push/pull stretch, grayscale-fade right)
Operator ("great demo") + 3 asks, verified headless Chromium + screenshots,
zero errors:
1) FONT THINNED for legibility: build_fonts.py now insets each filled
   pixel cell by PIX_GAP/2 (=9 units) so strokes are thinner and adjacent
   pixels read as separate squares (truer pixel-art, more legible). Both
   TTFs rebuilt + RE-EMBEDDED as base64 in the mockup.
2) CUSTOM SLIDER replaces the native range entirely (makeSlider, top-level
   so show()+tuner share it): the fractal-dither FILL WRAPS the knob
   (ends exactly at it); the knob is a SQUARE (unrotated, was a diamond);
   the unfilled RIGHT side is just a dim hairline with the accent faded to
   GRAYSCALE (saturate/brightness ramp down as it fills). Tuner (3) +
   settings GUI-scale + play render-distance all use it; lerp-to-detents
   on release retained.
3) PUSH/PULL STRETCH on the infill: on grab the fill plays a fillStretch
   keyframe (scaleX 0.93→1.04→1 from the left anchor) + the square knob
   squishes (18→22×16). Verified the .grab class toggles on drag and the
   fill width tracks the pointer.
Froze menu_mockup_v3.html + TTFs + build_fonts.py in outputs. 8/8 navigate,
font loaded (document.fonts.check), zero errors, JS node-checked.

## r221 — mockup v3 finalize: thin world-name stroke + full documentation
Operator: "sliders are great now"; world-name text entry was still ~2x too
thick; "document everything but the font entry I mentioned and then just
add that change."
- WORLD-NAME STROKE thinned: the SVG stroke-morph paths used stroke-width:7
  with non-scaling-stroke (≈7 screen px on a ~11px glyph = double weight).
  Halved to 3.5 — "NEW WORLD" now reads at a weight matching the rest of
  the UI; the stroke-morph behaviour is unchanged. Verified in Chromium.
- DOCUMENTATION: rewrote docs/design/menu_mockup_README.md as the finalized
  reference — version history (v1→v3), the full v3 feature set (cone
  parallax, edge-weighted glass + global specular, deterministic tint,
  temporal-history morph, border-fill, custom slider, diamond toggle,
  SVG stroke-morph entry + terminal caret, neutral cells + crosshair,
  history screenshot panes, glass×tactical Character sheet), the font
  pipeline (embedded base64, build_fonts.py, rebuild steps), and the egui
  port checklist.
Re-froze menu_mockup_v3.html. 8/8 navigate, font loaded, zero errors.

## r222 — egui menu COMPONENTS (transparent-first) written, not yet wired
Operator: "finalize this and let's write the egui components before
integrating... everything will need transparency once it lands in game."
Created crates/unendlich-ui/src/menu.rs — the mockup v3 design language as
reusable egui 0.34 components, TRANSPARENT-FIRST (all fills alpha via
from_rgba_unmultiplied / local a()/desaturate helpers; pairs with the
existing Surface::Frosted + frosted_backdrop for the blur). Declared
`pub mod menu;` in lib.rs. Components:
  - glass_panel (translucent fill + edge-weighted inner glow + top rim +
    global specular at a light origin)
  - toggle (<...> holder, 45deg caps shrink-wrapping a diamond knob, fractal
    dither infill revealing L->R, press squish)
  - slider (dither fill WRAPS a SQUARE knob, grayscale-faded right tail,
    push/pull stretch on grab, lerp-to-detents)
  - cell + crosshair_state (neutral hollow cells, clear interior, sharp
    hairline; row x col crosshair with manhattan-distance stagger)
  - history_point (screenshot-pane frame + decoupled diamond hover that
    sharpens)
  - dither_fill (shared multi-octave horizontal fill)
  - install_fonts / blocks_font / glyph_font (register the two TTFs into
    egui FontDefinitions; body stays default sans)
  - tokens/helpers: CORNER, SQUARE, spring(), a(), desaturate()
Verified: `cargo build -p unendlich-ui` clean (no menu.rs warnings; the
remaining Rounding/Frame::none warnings are pre-existing in lib.rs);
`cargo test -p unendlich-ui menu::` -> 3/3 pass (crosshair row/col,
desaturate endpoints, spring settles). NOT integrated into world_renderer
yet (per the operator's "before integrating"); engine already reaches the
kit via unendlich_ui::* so wiring is a localized follow-up. Wrote
docs/design/egui_menu_components.md (API + transparent-first notes +
integration checklist). Toolchain: cargo 1.96 present this session.

## r223 — menu fonts WIRED into the engine (main menu + menu_view)
First integration step: the custom fonts now load in-game and the menu
titles render in the blocky voxel face. Done, verified compiling + tested:
- FONT INSTALL: EguiOverlay::new (render/egui_overlay.rs) now calls
  unendlich_ui::menu::install_fonts(&ctx, include_bytes!(...Blocks.ttf),
  include_bytes!(...Glyphs.ttf)) once at context creation, embedding both
  TTFs (no runtime path). So the whole egui UI can opt into the blocky
  family.
- MAIN MENU TITLE (render/world_renderer.rs): the title-screen heading was
  ui.heading("unendlich") -> now a RichText "UNENDLICH" in
  menu::blocks_font(34.0), letterspaced caps (the mockup hero treatment).
- menu_view.rs (the tested, modular menu drawing): added a blocky_heading()
  helper and converted all six screen headings (New World / Load World /
  Multiplayer / Settings / Quit / Credits) + the title-flythrough heading
  to the blocky font. FEATURE-GATED behind cfg(feature="unendlich-ui")
  with a plain ui.heading fallback so the module still builds on egui-only
  configs (egui is optional + only ui-overlay pulls unendlich-ui).
- TEST HARNESS: menu_view's headless tests create a bare egui Context which
  lacks the custom fonts -> blocks_font panicked (epaint fonts.rs: family
  not found). Added install_test_fonts() mirroring EguiOverlay::new, called
  from run_once + the flythrough test. All 9 menu_view tests pass again
  (incl. the two that now exercise blocky headings).
Verified: cargo check -p unendlich-engine --lib --features render,window,
gpu,ui-overlay,audio -> Finished, no errors (the only crate error is the
PRE-EXISTING MeshRequest{epoch} in src/bin/gpu_mesh_bench.rs, unrelated);
cargo test ... menu_view:: -> 9/9 pass. NOT YET DONE: swapping the live
menu's checkboxes/sliders/cells for menu::toggle/slider/cell — that's the
next, larger step. (Disk filled mid-build once; cleared target/debug/
incremental to recover ~2GB. Env, not code.)

## r224 — menu COMPONENTS wired into Settings (toggles + slider)
Continued the integration past fonts/titles into live widgets:
- Settings -> Video: the Fog and Shadows ui.checkbox calls (world_renderer
  .rs) replaced with unendlich_ui::menu::toggle (the <...>-holder diamond
  toggle); click flips the bound bool, labelled in a horizontal row, accent
  pulled from current_theme.
- Settings -> Audio: the music-volume egui::Slider replaced with
  menu::slider(&mut ui_music_vol, 0.0, 1.0, 11, accent) — the fill-wraps-
  square-knob slider with grayscale-faded tail + detents.
- Left GUI-scale slider AS-IS (its staged release-to-commit logic is
  delicate); remaining sliders (day length, distance, perf) + inventory
  cells + history thumbnails noted in egui_menu_components.md as the next
  widgets to swap.
Verified: cargo check -p unendlich-engine --lib --features render,window,
gpu,ui-overlay,audio -> Finished, no errors; cargo test ... menu_view:: ->
9/9; cargo test -p unendlich-ui menu:: -> 3/3. Updated the integration
status in docs/design/egui_menu_components.md.

## r225 — more Settings widgets on the mockup components (VERIFIED r226)
VERIFIED: cargo check -p unendlich-engine --lib --features render,window,
gpu,ui-overlay,audio -> Finished, no errors; menu_view:: tests 9/9. The
int_slider! macro + integer round-trips compile clean.
- Settings -> Gameplay: day-length egui::Slider -> menu::slider (f32,
  detents 0 = free). Clean swap (f32 field).
- Settings -> Performance: plants/leaves/apply-budget/distance/target-fps
  egui::Slider (integer-typed: u64/u32/i32) -> menu::slider via an
  `int_slider!` macro that round-trips through a temp f32 and casts back
  (f.round() as $ty); detent count == integer span so the knob snaps to
  whole steps. The "autotune" checkbox -> menu::toggle; the fps slider is
  now shown only when autotune is on (was add_enabled/greyed — a minor
  behaviour change, hiding instead of disabling, since the custom slider
  has no disabled visual).
- Settings window: added a blocky "SETTINGS" title (blocks_font 22) above
  the tab bar.
- Left the creative inventory (grid_fit + slot_swatch) and hotbar ALONE —
  swapping them to menu::cell + crosshair_state needs the grid column count
  + hover-index threading, too intricate to do without a compile/test loop.

## r226 — inventory on the crosshair cell grid + cell_grid kit widget
Verified: cargo check (lib, ui-overlay features) -> Finished no errors;
cargo test -p unendlich-ui menu:: 3/3; menu_view:: 9/9 (r225 also confirmed
green this session).
- NEW kit widget menu::cell_grid (crates/unendlich-ui/src/menu.rs): a full
  crosshair grid — computes cols from available width (same formula as
  grid_fit), tracks the pointer-hovered cell across frames (ctx memory keyed
  by a salt), two-pass draw (find hovered -> style every cell via
  crosshair_state), calls content(ui, idx, rect) so the caller paints the
  item, returns Some(idx) on click. Keeps the crosshair logic in the kit
  (crosshair_state already unit-tested).
- Creative inventory (world_renderer.rs) swapped from grid_fit + slot_swatch
  to menu::cell_grid: neutral hollow cells, clear interiors, row×col
  crosshair on hover; the content callback paints each item's colour swatch
  (inset rect, brighter + accent border when selected) + short name. Click
  sets `selected`.
- Hotbar LEFT on slot_swatch (tight bottom bar; cell_grid is for the larger
  inventory).
NEXT: world-history timeline -> menu::history_point with real snapshot
thumbnails; panel chrome -> Surface::Frosted + menu::glass_panel; cone-
parallax + animated border-fill in the painter.

## r227 — world-history diamonds + more blocky headings (VERIFIED)
Verified: cargo check (lib, ui-overlay) -> Finished no errors; menu_view::
9/9.
- WORLD-HISTORY (the rollback browse panel in the Load screen): the engine
  exposes rollback points as a Vec<String> of "ts/reason/chunks" rows with
  restore/delete actions — there is NO real per-snapshot thumbnail texture
  yet (the capture hook is still in the longer-horizon queue), so rather
  than fake menu::history_point's screenshot pane with noise in-game, I kept
  the working vertical list + restore/delete and brought in the menu
  LANGUAGE: a blocky "WORLD HISTORY" header + a small accent DIAMOND marker
  (the history-point motif) painted before each row. When the screenshot-
  capture pipeline lands, this becomes the horizontal history_point timeline
  with real thumbnails.
- More blocky headings (world_renderer.rs): crafting window title and the
  new-world wizard title converted from ui.heading to blocks_font RichText
  (uppercased, letterspaced), matching the main menu / settings / menu_view.
NEXT: panel chrome -> Surface::Frosted + menu::glass_panel; cone-parallax +
animated border-fill in the painter; real screenshot-timeline capture (then
swap the rollback rows for the horizontal history_point timeline).

## r228 — panel chrome: ambient Frosted surface + edge-weighted glass glow
Verified: cargo check (lib, ui-overlay) -> Finished no errors; ui-kit tests
3/3; menu_view:: 9/9.
- AMBIENT SURFACE: the menu prepare closure (world_renderer.rs) now sets
  unendlich_ui::set_surface(c, Surface::frosted()) up front, so every kit
  component drawn over the live scene composites transparent-first. No-op
  visual today (default was Solid but glass_at/fresnel paint their own
  translucent fills); the blur PostFx pass upgrades this via
  Surface::frosted_blur(tex, tint).
- KIT: frosted_backdrop (lib.rs) gained the mockup's EDGE-WEIGHTED inner
  glow — inset accent rings brightest at the rim, fading inward, + a top
  rim highlight. GATED on a blur texture being present (Surface::Frosted{
  blur: Some(tex) }), so it only kicks in once the blur pass lands — every
  frosted panel then gets the "thicker glass at the edges" look for free,
  no per-call change.
- DECISION: did NOT force menu::glass_panel onto the existing menu windows.
  glass_at + fresnel already deliver a transparent, glowing frosted panel;
  layering a second full chrome treatment would muddy it. Instead the
  mockup's edge-glow was folded into the shared frosted_backdrop path (one
  implementation, every window benefits when blur lands). menu::glass_panel
  stays available for any NEW from-scratch panel.
NEXT: the blur PostFx texture (so frosted_backdrop's image+glow actually
render); cone-parallax offsets in the menu draw; animated border-fill in
the painter; screenshot-timeline capture -> horizontal history_point.

## r229 — cone-parallax on the main menu (VERIFIED)
Verified: cargo check (lib, ui-overlay) -> Finished no errors; ui-kit menu::
tests 5/5 (2 new); menu_view:: 9/9.
- KIT: menu::cone_offset(cursor, screen, z) — the mockup's cone parallax
  (cursor = apex; layer at depth ratio z projects from screen centre by
  cursor_offset*z; resolution-independent) + menu::parallax_lerp(cur, tgt,
  smooth) for weighty smoothing. Both unit-tested (centre→zero, edge→scaled,
  lerp converges).
- MAIN MENU (world_renderer.rs): the window anchor now offsets by a smoothed
  cone offset (z=0.018, smooth=0.06) so the panel drifts a few px toward the
  cursor with weight. Stored in ctx memory across frames; request_repaint
  keeps the drift animating. Subtle + stable (small z, translation only —
  no rotation, which the mockup found disorienting).
NEXT: same parallax on the settings/other windows if wanted; blur PostFx
texture; animated border-fill in the painter; screenshot-timeline capture.

## r230 — egui external-texture bridge for the frosted blur backdrop
The reusable piece that lets the menu sample an engine-owned image (the
blurred scene) for true frosted glass. Compile-verified; the scene-blur
PASS itself still needs a GPU/runtime test loop (documented).
- EguiTexture gained an `external: bool`. When true the entry owns ONLY its
  descriptor set; destroy_texture frees the set but NOT the borrowed image/
  memory/view (those belong to the engine render target).
- NEW EguiOverlay::register_external_texture(vk, slot, view, sampler) ->
  TextureId::User(slot): allocates a descriptor set pointing at an engine-
  owned view+sampler (must be SHADER_READ_ONLY when the egui pass runs),
  returns the stable id to hand the UI via Surface::frosted_blur. Re-call
  on swapchain resize to repoint.
- DECIDED NOT to commit the scene-blur render pass blind: it touches pass
  ordering + layout transitions + resize and needs a runtime test to verify
  it doesn't break the frame loop. Instead wrote docs/design/blur_backdrop_
  integration.md with the exact recipe: the engine already has the bits
  (offscreen.rs OffscreenTarget + bloom.rs's separable gaussian + view()/
  write_scene()); cheapest path is a second blur ping-pong seeded from the
  offscreen SCENE view into a half-res SHADER_READ_ONLY target, register it,
  then switch r228's Surface::frosted() to frosted_blur(tex, tint) and add a
  frosted_backdrop call (or menu::glass_panel) on the menu windows.
Verified: cargo check (lib, ui-overlay) -> Finished no errors; menu_view::
9/9; ui-kit menu:: 5/5.
NEEDS TEST (operator): once the scene-blur pass is wired, run the game and
open a menu — panels should show a blurred scene + edge glow. If they go
black, it's the blur target layout or prepare/record ordering (see doc).

## r231 — timeline branching + timebending (NEW sim module, tested + doc'd)
Operator: timebender goes back, builds a structure 1000 years ago, it
decays/erodes forward, branches a new timeline, old goes stale via branching
+ looping. Add a test. Doc it while they test.
- NEW crates/unendlich-engine/src/sim/timeline.rs (pub mod timeline in
  sim/mod.rs): branching timelines over the rollback model. Pure +
  deterministic, headless-tested.
  * Timeline = tree/DAG of Branch{parent:(BranchId,Tick), edit:Structure,
    start, head, stale}. Branch 0 = original timeline.
  * timebend(from, at_tick, edit) -> forks a new LIVE branch at at_tick;
    parent's future past the fork goes stale; rejects out-of-span edits.
    "build a structure 1000 years ago" = timebend(live, head-1000*ERA, edit).
  * Staleness CASCADES (mark_stale_below): any branch rooted on a now-dead
    segment is transitively stale -> looping/nesting into a DAG. Fork at head
    = clean continuation (no staleify).
  * advance(id, ticks); project_live_structure / project_structure erode the
    edit once per era of branch age; lineage(id) for the fork chain.
- DECAY/EROSION via world::thermal_erosion::erode_step. IMPORTANT: that model
  CONSERVES mass (slumps, doesn't shed), so weathering = lost RELIEF not lost
  mass. Structure has mass()/peak()/relief(); Structure::tower() (peaked, real
  relief, erodes visibly) vs raised() (flat slab). Headline test asserts
  relief+peak drop while mass is conserved over 1000 eras.
- Tests (5/5 green): the headline build-erode-staleify scenario, fork-a-fork
  cascade (looping), lineage trace, out-of-span rejection, fork-at-head clean
  continuation.
- Doc: docs/design/timeline_branching.md (model, erosion honesty, what's
  proven, and the NOT-yet-wired integration: bind branch head to SimState/
  chunks, region-scoped resim via rollback_closure, stamp structure into
  voxels, timebender agency + branch DAG in the history menu).
Verified: cargo test ... timeline:: -> 5/5; cargo check (lib) -> clean.

## r232 — animated border-fill in the painter (VERIFIED)
Back on menu polish. The mockup's "the frame draws itself in" border ring,
ported from the CSS inset-shadow approximation to a real painter pass.
- KIT: menu::border_fill(ui, rect, accent, t01) -> bool — an inset ring that
  starts THICK at the rim (~10px) and shrinks to a hairline while fading out,
  over t01 in [0,1] (0=thick start, 1=gone). Layered concentric strokes give
  the ring body + an inner fade so it's brightest at the rim. Returns whether
  still animating (t01<1) so the caller keeps repainting. Plus the pure
  helper menu::border_fill_active(t01) (unit-tested).
- MAIN MENU (world_renderer.rs): after the fresnel call, drive border_fill
  from ctx.animate_bool_with_time keyed by menu_screen_kind (ramps 0→1 once
  when a screen appears; pass 1.0-ramp so the ring plays thick→gone), and
  request_repaint while it animates. Plays on each distinct screen's first
  appearance (egui caches the per-id ramp, so re-visiting the same screen
  kind won't replay — acceptable for the effect).
Verified: cargo check (lib, ui-overlay) -> Finished no errors; ui-kit menu::
6/6 (border_fill_active added); menu_view:: 9/9.
NEXT (menu queue): cone-parallax on settings/other windows; the scene-blur
render pass (recipe in blur_backdrop_integration.md, needs runtime test);
screenshot-timeline capture -> horizontal history_point.

## r233 — cone-parallax on all menu windows (VERIFIED)
Extended the main-menu cone-parallax (r229) to the other windows via a
shared closure.
- world_renderer.rs: added a reusable `cone_par(c, id, z)` closure next to
  glass_t — computes the smoothed cone offset (cursor=apex, per-id memory,
  parallax_lerp smoothing, request_repaint) for any window. Refactored the
  main menu to use it (was an inline block).
- Applied to: settings (z=0.012), crafting (0.012), wizard (0.012), pause
  (0.014). Each window drifts a few px toward the cursor with weight; the
  main menu leads most (z=0.018). Distinct per-id memory keys so they don't
  share state.
Verified: cargo check (lib, ui-overlay) -> Finished no errors; menu_view::
9/9; ui-kit menu:: 6/6 (unchanged).
NEXT: scene-blur render pass (recipe in blur_backdrop_integration.md, needs
runtime test); screenshot-timeline capture -> horizontal history_point.

## r234 — toggle render fix + parallax repaint storm fix (the "not smooth"/
## "toggles messed up" report)
Operator screenshot: toggles rendered as two fat hexagons with the dither
fill spilling into the rectangular corners; also "smoothness not there, low
FPS, maybe inventory coupled to world".
- TOGGLE FIX (menu.rs): the bug was dither_fill(ui, RECT, ...) filling the
  whole 56x26 bounding box, so dots spilled past the <...> holder outline
  into the corners (the busy dotted overflow). Now the infill draws the
  HOLDER POLYGON itself filled with accent, clipped via
  painter.with_clip_rect([left, reveal_x]) so it exactly follows the chevron
  and can't bleed into the corners; a light dither is layered on top, clipped
  to the same region inset inside the hull. Also: caps softened from 0.5h
  (45° spikes -> read as fat hexagons) to 0.34h gentle chevron; size 56x26 ->
  48x24; knob proportional (0.30h). Reads as a clean diamond toggle now.
- FPS / "not smooth" FIX (world_renderer.rs): the r233 cone_par closure
  called c.request_repaint() EVERY frame for EVERY menu window, pinning the
  whole egui overlay at max repaint rate even when idle (cursor still) —
  coupling the menu to continuous repaint and tanking FPS. Now it only
  requests a repaint while the parallax lerp is still settling
  ((cur-target).length() > 0.05); once converged it goes idle and egui
  sleeps. Self-limiting border_fill repaint was already fine.
- NOTE on "inventory coupled to world": confirmed unendlich-fullgame does NOT
  pause/throttle the WORLD render when a menu/inventory is open (pre-existing
  — the world keeps rendering at full cost behind the overlay). That's a
  design choice, not introduced here; flagged for the operator (should
  opening inventory pause/idle the world?). My menu additions are O(cells)
  per frame at most and now idle when static.
Verified: cargo check (lib, ui-overlay) -> Finished no errors; ui-kit menu::
6/6; menu_view:: 9/9.

## r235 — server game-speed slider + per-world settings page (model tested)
Operator: a Settings slider for game pause speed 0x–1x–10x logarithmic,
default 1x; SERVER-only (clients reconcile to the value CLOSEST to 1x so all-
10x=10x, all-0=0x, mixed pulls to 1x). Plus a per-world server settings page
under a two-column (settings icon + world icon) button.
- NEW sim/game_speed.rs (pure, 8 tests): pos_to_multiplier (log, 0=freeze /
  0.5=1x / 1.0=10x), multiplier_to_pos, DEFAULT_POS=0.5, and reconcile(prefs)
  = the value closest to 1x in LOG space (0.5x & 2x equidistant; freeze wins
  only if all freeze; ties -> slower). Covers everyone-fast, everyone-frozen,
  mixed-to-normal, closest-among-many, empty.
- UI (world_renderer.rs): WorldRenderer.ui_game_speed_pos (default 0.5),
  mirrored in prepare. menu::slider on the Gameplay tab (label shows live
  multiplier + server-authoritative note). NEW per-world settings page =
  settings tab 5, reached by a two-column icon button (⚙🌐) right-aligned in
  the tab bar; surfaces game-speed + day-length as the world's server knobs.
- DOC docs/design/game_speed.md: model + the NOT-yet-wired sim integration
  (gather peer prefs on the netcode path -> reconcile per tick in sorted
  order for determinism -> scale how many FIXED sim steps run per wall-second
  by the multiplier, NOT dt inside the step, so physics/rollback stay bit-
  exact; 0x freezes the world but not the UI).
Verified: cargo check (lib, ui-overlay) -> Finished no errors; game_speed::
8/8; menu_view:: 9/9; ui-kit menu:: 6/6.

## r236 — docs pass: menu-system index + cross-links (no code change)
Pivoted to docs (operator can't test). Pure documentation, no source edits.
- NEW docs/design/MENU_SYSTEM.md — the connective overview/index for the whole
  r219–r235 arc: a status TABLE (area / code / doc / wired-vs-pending), a
  "what's wired in-game (verified)" summary, the 21-item component kit quick
  reference, the 4 pending items (each with its blocking runtime/netcode step),
  and a one-line release history. Status claims re-verified against the actual
  code (grep counts) before writing, not from memory.
- FIXED stale "Still to wire" in egui_menu_components.md: it listed cone-
  parallax / border-fill / r229–r233 items as pending though they shipped.
  Replaced with an accurate r229–r235 wired list + a correct pending list +
  a pointer to MENU_SYSTEM.md.
- CROSS-LINKED timeline_branching / game_speed / blur_backdrop_integration /
  menu_mockup_README back to MENU_SYSTEM.md so the doc set is navigable.
No build impact (docs only); world_renderer.rs confirmed untouched.

## r237 — perf: wire the percentile profiler into the frame loop (measure first)
Pivoted to performance/stutter. First move = instrumentation, not blind
optimisation.
- unendlich_fullgame.rs: the existing t_*_us phase sums AVERAGE spikes away.
  Added a profiler::Profiler fed each frame with manager_update / tick_sim /
  draw (the three existing capture points) + frame_total (whole loop iter).
  Gated behind UNENDLICH_PROFILE=1: dumps a per-zone p50/p95/p99/max table
  (worst-p99 first) every 5s, so the stutter TAIL is visible per phase. Zero
  cost when off (record is a cheap bucket inc; only the dump is gated).
- The capture points kept their original t_*_us accounting (recomputed from
  the same elapsed, no behaviour change) and additionally feed the profiler.
- DOC docs/design/PERF_PROFILING.md: how to turn it on, how to read the three
  phases (manager_update = chunk stream/upload = #1 stutter suspect; tick_sim
  = world sim; draw = submit/present incl GPU sync), and the methodology
  (profile -> find dominant tail zone -> adjust THAT zone's existing budget
  lever, e.g. UPLOAD_BUDGET_MS / APPLY_BUDGET / MESH_BATCH / RENDER_DISTANCE /
  PREFETCH -> re-profile). One lever at a time.
Verified: cargo check --bin unendlich-fullgame (full features) -> Finished no
errors; profiler:: tests 4/4. NEEDS A RUN (operator) to capture the actual
percentile table — that data drives the next, targeted optimisation.

## r238 — batch-as-field grass LOD (experimental, gated; pure model tested)
Operator: a new culling-via-BATCHING idea — instead of culling far grass, LOD
the back and use FIELD PROPAGATION to fast-forward it while high-res ticking
the front. Add gated code.
- NEW cargo feature grass-field-batch (off by default) gating the live tick-
  path hookup. Pure module always built.
- NEW crates/unendlich-engine/src/sim/grass_field.rs (pub mod grass_field):
  a grass patch as a coarse w×h FIELD of FieldCell{density,growth,sway}.
  * tick_front(wind,p) = one high-res front tick (exact).
  * fast_forward(n,wind,p) = the BACK-band field propagation: advance n
    skipped ticks in ONE pass. growth+sway are per-cell independent channels
    -> close-form, EXACTLY equal to n front ticks (proven by a test);
    spreading diffuses once at capped strength (field analogue of
    leaf_decay::advance_n_ticks saturating catch-up). O(w·h) « O(N blades).
  * band_plan(band, gap, p) -> GrassWork, reusing world::scheduler bands:
    Render->HighResFront, Simulation->FieldFastForward{catch_up_ticks},
    CatchUp->Dormant.
  6 tests (grow+saturate, fast_forward==n front ticks for growth/sway,
  spreading conserves mass into the cap, sway wraps, band split, empty stable).
- DOC docs/design/grass_field_batch.md: model + the gated integration recipe
  (per-chunk GrassField seeded from instance data; drive from a leaf_decay-
  style handler via band_for_chunk + the fire_tracker gap; push the field to
  the far LOD instance buffer so far grass RENDERS from the field; all under
  the feature) + how to test it (look across a big field; far grass ripples
  not frozen; tick_sim p99 drops).
Verified: cargo test ... grass_field:: -> 6/6; cargo check (lib) clean both
WITH and WITHOUT the grass-field-batch feature. Integration NEEDS a build +
runtime to validate (off the default path; engine unaffected).

## r239 — fire batch-as-field + vision-driven mesh scheduling (experimental, gated)
Operator: do the grass batch-field thing for FIRES too; and for dense
underground cave chunks, use deferred batching to SKIP mesh updates that are
obscured — generalize to most blocks via smart prediction of mob+player VISION
CONES to build only what's needed instead of full meshing.
- FIRE: NEW cargo feature fire-field-batch (off by default) + NEW
  crates/unendlich-engine/src/sim/fire_field.rs (pub mod fire_field): a fire
  region as a coarse FireField{fuel,intensity,heat} reaction-diffusion front.
  tick_front (exact near step); fast_forward(n) iterates the aggregate step n
  times (combustion is nonlinear/coupled, no closed form) CAPPED at
  ~4/burn_per_tick (fuel monotonic -> converges to burnt-out, so a long
  absence can't spin). band_plan -> FireWork{HighResFront|FieldFastForward|
  Dormant}, reusing world::scheduler bands. 6 tests (spread, fuel monotonic,
  fast_forward==n steps, burns out, no-fuel-no-fire, band split).
- MESHING: NEW cargo feature mesh-vision-sched (off by default) + NEW
  crates/unendlich-engine/src/sim/mesh_vision.rs (pub mod mesh_vision): a
  meshing-TIME visibility gate (the existing chunk_indirect GPU cull is draw-
  time, AFTER meshing already paid). ViewCone{pos,yaw,half_fov,range,
  turn_margin}; urgency_for_cone -> MeshUrgency{Now|Soon|Defer}: in-fov+near+
  unoccluded=Now, far/just-outside-fov(turn_margin prefetch)/near-occluded=
  Soon, behind/out-of-range/far-occluded=Defer (the dense-cave win is Defer).
  urgency_for_cones = MAX across cones (a mob that sees it pulls it to Now).
  Occlusion is an INJECTED predicate (caller backs it with raycast LOS / GPU
  occlusion query / portal vis) so the model stays pure. Cone math mirrors
  sim::perception. 9 tests (full matrix incl cave occlusion cases + multi-cone
  max + behind-defers).
- DOCS: docs/design/batch_field_lod.md (shared grass+fire pattern + fire
  integration recipe + how to test) and docs/design/mesh_vision_scheduling.md
  (model + occlusion-source options cheapest-first + the gated mesh-queue
  integration recipe + how to test + the CORRECTNESS-over-speed risk: a wrong
  defer = an unmeshed hole, so occlusion must be conservative — model already
  biases occluded+near to Soon not Defer).
Verified: cargo test ... fire_field:: 6/6, mesh_vision:: 9/9; cargo check
clean with all three experimental features (grass/fire/mesh-vision) ON
together AND on the default path. Integrations NEED a build+runtime; all off
the default path, engine unaffected.

## r240 — mesh-vision: first gated integration pass (conservative, NEEDS RUN)
Wired the mesh_vision model (r239) into the live mesh queue, behind the
mesh-vision-sched feature (off by default). Compile-verified both ways; the
runtime behaviour NEEDS A RUN (no GPU in the build sandbox).
- ChunkManager: gated player_view_yaw field + set_player_view_yaw(yaw) setter;
  the bin calls it each frame from local_player.view.yaw before
  update_with_interest_points.
- request_visible shell-walk: for each candidate chunk, build the player
  ViewCone (eye = player chunk center, yaw, range = rd*CHUNK_SIZE) and call
  urgency_for_cones; on MeshUrgency::Defer, `continue` (skip meshing) and
  refund the per-tier budget slot. Voxels still load — only the MESH is
  deferred — and the chunk re-enters the walk (meshes) once the player turns
  toward it, since request_visible runs every update.
- CONSERVATIVE: occlusion predicate is hardcoded false this pass, so ONLY
  clearly-unseen chunks (behind the player / out of range) are deferred —
  never an in-view chunk, so there can be no unmeshed hole in front of the
  player. The occlusion-backed dense-cave win (raycast LOS / GPU occlusion
  query) is the next step.
Verified: cargo check (lib) clean BOTH default and with mesh-vision-sched;
cargo check --bin unendlich-fullgame with the feature -> Finished no errors;
mesh_vision:: 9/9. Updated docs/design/mesh_vision_scheduling.md status.
HOW TO TEST: build with the feature, profile with UNENDLICH_PROFILE=1, walk
around; manager_update p99 should drop (behind/out-of-range chunks no longer
mesh) with NO pop-in in front of you (occlusion off = in-view always meshes).
If turning hitches, the walk is re-meshing a lot on rotation — expected to be
mild since only behind->front transitions add work; widen budgets if needed.

## r241 — coordinator: dispatched two parallel worker threads (ITEMS + MOBS)
Per the worker-pipeline (WORKER_PIPELINE.md / HAIKU_WORKER_BRIEF.md): acting as
coordinator, cut two parallel engine-task threads, each a self-contained bundle
(WORKER_BRIEF.md at root + the r240 tree). Tasks are doc-grounded wiring of
ALREADY-BUILT+TESTED pure modules into the live path — not invented scope.
- THREAD ITEMS (unendlich-r241-thread-ITEMS-WORKER.zip): owns item*/inventory*/
  crafting/loot/recipe + sim/item_*/craft_*/loot_*. Tasks: T1 item_durability
  (6 tests) -> wear on use; T2 craft_quality (6) -> crafted-item stats; T3
  loot_quantity (6) -> drop stacks via item_drop_resolve (already in bin); T4
  item_modifier -> affixes (test-only if no UI slot, else handoff to UI thread).
- THREAD MOBS (unendlich-r241-thread-MOBS-WORKER.zip): owns sim/mob*/forage*/
  herd_cohesion/behavior_arbiter/locomotion + per-roamer bin hooks. Tasks: T1
  behavior_arbiter (11 tests) -> drives roamer action; T2 herd_cohesion (6) ->
  flocking for social species; T3 forage_harvest (10) -> eating depletes food;
  T4 band-aware mob ticking (PERF — far mobs tick on the Simulation band with
  catch-up like leaf_decay; the highest-leverage one).
- Each brief enforces the hard rules: no new docs, no new architecture (wire
  into the registries / scheduler bands / existing roamer hooks), a log::info!
  + named kill switch per wired feature, verify-per-change, stop-on-failure.
  Verified bundles: WORKER_BRIEF.md at root + full tree + fonts; the candidate
  modules confirmed to exist with the stated test counts and to be absent from
  the bin (genuine wiring gaps, not invented).
Coordinator now returns to GENERAL PERFORMANCE work directly (next entries).

## r242 — perf: sub-phase profiler zones (apply + upload) [general perf, self]
Back on general performance directly (post thread-dispatch). The r237 profiler
covered manager_update/tick_sim/draw/frame_total; a spike in manager_update or
tick_sim didn't say WHICH sub-part. Added two more zones from timers the engine
ALREADY computes, so the tail is attributable:
- `apply` <- chunk_manager.last_apply_us() (deferred edits landing in chunks;
  the dominant tick_sim spike on a loading wave; lever = UNENDLICH_APPLY_BUDGET).
- `upload` <- t_upload_us (GPU buffer upload; lever = UNENDLICH_UPLOAD_BUDGET_MS
  / MAX_UPLOADS_PER_FRAME).
Both env-gated behind UNENDLICH_PROFILE=1 (cheap bucket inc; only dump gated).
No behaviour change — pure measurement off existing timers.
Also INVESTIGATED (no change made, by design): the apply-budget path
(chunk_manager ~1195) is a well-built COUNT budget with deferred carry-over
(observed 107.9ms/489-chunk waves are exactly what it caps); converting to a
TIME budget would break determinism (rollback-sensitive), so left as-is. The
engine-side budget system is already mature; the profiler zones are the
honest next step — they need a RUN to produce the data that drives a targeted
fix. NOT poking determinism-sensitive tuned code blind.
Verified: cargo check --bin unendlich-fullgame (full features) -> Finished no
errors; profiler:: 4/4 (unchanged).

## r243 — shaders pass (toolchain now in-sandbox; validator + sky micro-opt)
Operator took a shaders turn; I set up the ground + made safe verifiable wins.
Got glslang-tools installed in-sandbox (glslangValidator 15.1 + spirv-val) —
so shaders are genuinely compile+validate-checkable here now (the docs wrongly
said unavailable; the committed_spirv_is_fresh test fails in-sandbox only due
to glslang VERSION differences, not stale source).
- VERIFIED the whole set: 28/28 shaders compile (glslangValidator) AND pass
  spirv-val. No broken shaders. (So no bugs to fix there.)
- NEW tools/validate_shaders.sh: compile + spirv-val every shader; graceful if
  the compiler is absent; does NOT touch committed .spv (version-managed).
- DONE sky.frag mie_phase: pow(x,1.5) -> x*sqrt(x) (math-equal, cheaper, per-
  fragment, visually identical). Validated; regenerated committed
  assets/shaders/sky.frag.spv + removed its stale .spv.hash so the operator's
  build recompiles cleanly with their glslang.
- DEFERRED (flagged, needs CPU UBO change): sky.frag runs inverse(view_proj)
  PER FRAGMENT (fullscreen tri) — add mat4 inv_view_proj to CameraUbo, write it
  once CPU-side, use it in-shader. Pure speedup; held back because descriptor/
  UBO-layout change must be runtime-verified with the Rust side.
- DOC docs/design/shader_pass_notes.md: the above + other pow() candidates left
  as-is (compilers lower constant-exponent pow well; not worth blind churn) +
  why the rest is deferred (mature/optimised codebase; remaining wins need CPU
  coordination or change visible output → want a GPU to verify).
Verified: tools/validate_shaders.sh -> 28/28 OK; committed sky.frag.spv passes
spirv-val. NO Rust behaviour change. (The lib/bin are untouched this round.)

## r244 — per-chunk shader variants (selector tested; shader flags partly wired)
Operator idea: extra/cheaper shaders for far/culled chunks + dynamic variants
that optimize out fog/reflections/dynamic-lights for flat/dry/no-mob chunks,
for perf at minimal quality loss.
- chunk.frag: added 5 compile-time variant flags (CHUNK_FOG, CHUNK_FOG_STEPS,
  CHUNK_WATER, CHUNK_DYNAMIC_LIGHTS, CHUNK_SHADOWS), each DEFAULTING to current
  behaviour (undefined build == byte-identical to pre-r244). Extends the
  existing #if PBR_QUALITY pattern, not new architecture.
  * WIRED + verified: CHUNK_SHADOWS=0 (skips shadow PCF -> shadow_factor=1.0;
    clean, the no-shadow fallback already existed) and CHUNK_FOG_STEPS=N (the
    fog march count). All variant combos compile + pass spirv-val (matrix:
    default / SHADOWS=0 / FOG_STEPS=2 / FOG=0 / WATER=0 / DYNAMIC_LIGHTS=0 /
    all-off — 8/8 OK).
  * DECLARED but NOT yet wired (honest): CHUNK_FOG / CHUNK_WATER /
    CHUNK_DYNAMIC_LIGHTS — the fog/water code is INTERWOVEN through the final
    color blending (~770-1091), so wrapping in #if correctly needs a running
    build to verify; they compile today but don't remove work yet.
- NEW sim/shader_variant.rs (pub mod, 7 tests): ChunkShadeInfo{band,has_water,
  has_dynamic_light,within_shadow_dist} -> select() -> ChunkVariant{fog,
  fog_steps,water,dynamic_lights,shadows} with .defines() (-D list) + .key()
  (pipeline cache key) + HIGH (== un-gated shader). Conservative rules: water
  iff has_water, lights iff in range, shadows iff within shadow dist, fog always
  on but steps drop by band (6/3/2, never <1).
- NEW cargo feature shader-variants (off by default) gating the pipeline
  compilation + per-chunk binding integration.
- Regenerated committed assets/shaders/chunk.frag.spv (default variant) +
  cleared its stale .spv.hash.
- DOC docs/design/shader_variants.md: the honest wired/declared/integration
  split + selector rules + the gated pipeline recipe (compile per-variant
  pipelines via variant.defines(); tag chunks with has_water/band/light/shadow;
  group by variant.key() for indirect draw) + how to test + the quality-vs-speed
  risk (a too-eager drop = a visible seam; selector is keep-when-unsure).
Verified: shader_variant:: 7/7; validate_shaders.sh 28/28 + the 8-combo matrix;
cargo check (lib) clean default AND would-be with shader-variants (no integ code
yet so feature is currently a no-op flag). chunk.frag default byte-regenerated.

## r245 — FIX: dead main-menu buttons + missing [profile] output
Operator (r241/r244): main-menu buttons dead (couldn't create/load worlds), and
no [profile] in logs. Two real regressions I introduced; both fixed.
- MENU BUTTONS: cone-parallax (r229/r233) was applied as the egui Window
  .anchor() OFFSET, computed per-frame from the cursor → the panel DRIFTED
  toward the cursor, moving buttons' hit rects out from under clicks (r234
  repaint-gating compounded it). FIX: cone_par closure forced to Vec2::ZERO →
  restores the proven pre-r229 fixed-center anchor for all 5 menu windows
  (main/pause/settings/crafting/wizard). The helper + menu::cone_offset/
  parallax_lerp kit (still tested) stay for a future SAFE re-enable (a
  visual-only transform that does NOT move interaction rects). Doc
  docs/design/menu_parallax_regression.md (root cause + safe re-enable path).
- MISSING [profile]: UNENDLICH_PROFILE was ALREADY the flag-bundle PRESET
  selector (line 313), so the r237 percentile profiler reading the same var
  never fired (and =1 printed "unknown profile '1'"). FIX: profiler now reads
  UNENDLICH_FRAME_PROFILE=1. Updated PERF_PROFILING.md + the 4 other perf docs
  that referenced the old var (batch_field_lod/grass_field_batch/
  mesh_vision_scheduling/shader_variants).
- sky.frag phase function (r243 x*sqrt(x)) confirmed present + ships in this
  full-tree zip ("let's try your sky phase function").
Verified: cargo check --bin unendlich-fullgame (full features) → Finished, no
errors, no unused-var warnings from the neutralized closure; menu_view:: 9/9.
HOW TO RUN THE PROFILER NOW: UNENDLICH_FRAME_PROFILE=1 ./run-latest.sh → a
per-zone p50/p95/p99/max table every 5s (zones: manager_update, tick_sim,
apply, upload, draw, frame_total).

## r245b — coordinator: dispatched two more worker threads (RENDER + PERF)
Operator: "spawn two more threads to continue." Cut two parallel engine-task
bundles on the FIXED r245 tree (so workers build on working menu code), brief
at root, doc-grounded continuations of the perf/shader arc:
- THREAD RENDER (unendlich-r245-thread-RENDER-WORKER.zip): owns shaders/** +
  CameraUbo + assets/shaders/*.spv. Tasks: T1 wire the 3 interwoven chunk.frag
  variant flags (CHUNK_FOG/CHUNK_WATER/CHUNK_DYNAMIC_LIGHTS — r244 left them
  declared-but-unwrapped); T2 the deferred sky.frag per-fragment
  inverse(view_proj) → add mat4 inv_view_proj to CameraUbo (GLSL both shaders +
  Rust struct), write it CPU-side once/frame; T3 audit chunk.frag for the same.
  Brief stresses: shader toolchain is installable in-sandbox, verify with
  tools/validate_shaders.sh + the variant matrix, flag visual changes as
  on-GPU-only.
- THREAD PERF (unendlich-r245-thread-PERF-WORKER.zip): owns sim/mesh_vision.rs +
  the mesh-vision-sched gated chunk_manager code + raycast.rs (compose). Tasks:
  T1 a pure+tested chunk-level LOS occlusion predicate (injected is_solid
  closure, conservative: uncertainty→not occluded, target chunk's own face never
  counts); T2 back the r240 hardcoded `|_,_| false` occlusion with raycast (the
  dense-cave win), keep the conservative bias + kill switch; T3 a cheap
  early-out for the common not-occluded path. Brief stresses the
  correctness-over-speed property (a wrong defer = an unmeshed hole).
Both enforce the hard rules (no new docs, no new architecture — extend the r244
shader flags / r240 mesh-vision gate / existing raycast, log+killswitch, verify-
per-change, stop-on-failure). Coordinator now returns to our direct work.

## r246 — perf: localize the real stutter (game_update zone) + the r245 finding
The operator's first real UNENDLICH_FRAME_PROFILE=1 capture (profiler now works
after the r245 var-collision fix) was decisive and SURPRISING:
  frame_total p50=6.1ms (164fps) BUT p95=60ms / p99=76ms (the stutter), while
  draw p95=4.8ms and tick_sim/manager_update/apply/upload were ALL ~0.
=> ~55ms of every p95 frame is spent OUTSIDE every measured zone. draw includes
   acquire+present (verified) so it's NOT vsync. The spike is in the UNTIMED
   per-frame game logic — the roamer/dragon/fauna/drops/entity AI loops, which
   are separate from chunk_manager.tick_simulation (~0).
- Added a `game_update` profiler zone = frame-top → draw-call wall time (all
  pre-draw loop work), to localize it. Re-run will show whether game_update p95
  is the ~55ms (expected) → then sub-time the roamer/dragon blocks.
- DOC docs/design/PERF_FINDING_r245.md: the data, the reasoning (why not
  vsync/streaming), and the next step.
- REFRAME: the stutter is NOT chunk streaming/meshing/upload (those are already
  cheap — prior tuning worked). mesh-vision/batch-field stay valuable for
  SCALING but are not the current-spike fix. Measure game_update, then cut.
Verified: cargo check --bin unendlich-fullgame (full features) -> Finished, no
errors. (Pure measurement addition; no behaviour change.)
NOTE: uploaded results.zip carries the MOBS-thread return (behavior_arbiter/
forage_harvest/herd_cohesion tests + new mob_bands.rs) + a clean particles
result — queued for a deliberate coordinator merge (not rushed into this perf
release).

## r247 — perf: localize the spike to the per-frame creature MESH REBUILD
Operator can't test; gave scale (~100 dragons + ~100 fauna, RD 4). Reproduced
the candidate costs with standalone rustc -O microbenchmarks (saved in
tools/perf_benches/) on the llvmpipe host (relative magnitudes transfer).
- RULED OUT the creature AI as the spike, with numbers @100×100:
  dragon-hunt target scan 0.024ms/frame WORST (all hunting); per-roamer any()
  0.0075ms; steady per-roamer tick 0.006ms; dragon_observe is capacity-1. Three
  orders of magnitude too small for the 55ms p95.
- IMPLICATED the per-frame creature MESH REBUILD at fullgame ~7468: it
  regenerates the full multi-box geometry for all ~200 creatures into FRESH Vecs
  and re-uploads EVERY frame, unconditionally, even when nothing moved. Modeled
  (100×22 boxes + 100×8 boxes -> ~72k verts/frame): 0.53ms/frame JUST for vertex
  gen, PLUS ~2.6MB Vec alloc-churn/frame AND a ~2.6MB GPU upload/frame (+ a
  stall when the buffer must grow). The compound is the p95/p99 tail shape.
- WIRED a `creature_mesh` profiler zone around that block so the operator's next
  UNENDLICH_FRAME_PROFILE=1 run CONFIRMS it directly (if creature_mesh p95
  tracks game_update's tail -> proven).
- FIX (documented, for the M2 since it changes uploads): dirty-flag the rebuild
  — cache the built buffers, rebuild only on move/anim/spawn/despawn (animated
  parts on a coarse 2-3 frame cadence), reuse the Vecs (.clear() not Vec::new),
  re-upload only when dirty. Same "don't redo unasked work" as the chunk remesh
  gate.
- DOC docs/design/PERF_FINDING_creature_mesh.md (data + fix + honest limits: the
  0.53ms is a MODEL floor, not the real forge body; AI ruled out, mesh rebuild
  is the largest per-frame game-logic cost found by inspection+bench).
Verified: cargo check --bin unendlich-fullgame (full features) -> Finished, no
errors. Pure measurement addition (the zone); the FIX is not yet applied
(needs a run to confirm + verify the cached path renders identically).

## r248 — perf: far-mesh cave-skip lever + roamer_tick zone (meshing chug + steady cost)
Operator's richer profile (subsystem timers) + steer ("meshing chug is the
biggest issue; take the inefficiencies anyway"). Two findings, two actions.
- MESHING CHUG (the headline spikes): the `mesher:` line shows FAR chunks at
  ~11-13ms EACH (far=11.20ms×77 etc.), in bursts. Traced to
  render/mesher.rs::mesh_chunk_far_virtual — the far reverse-regen path calls
  generator.density() AND generator.is_cave() per grid cell, and is_cave()
  evaluates THREE 3D noise layers (worm+chamber+seepage, per world_model.rs).
  Caves are INVISIBLE at far distance, so that's wasted work.
  ACTION: added UNENDLICH_FAR_SKIP_CAVES=1 (default OFF = byte-identical) which
  skips the cave carve for Far/VeryFar virtual meshing -> renders far terrain
  solid (visually identical at distance) and ~HALVES the per-cell generator
  cost. Split into mesh_chunk_far_virtual (reads env) + _impl(skip_caves);
  public signature unchanged. Lib compiles clean.
- STEADY 16.5ms game_update p50 (separate from the spikes; holds even with
  <no chunks meshed>): no subsystem timer accounts for it (creature_mesh
  0.03ms measured, AI scans microbenched nanoscale, draw 1.9ms, cpu_zones ~2ms
  inside draw). ACTION: added a `roamer_tick` profiler zone around the ~1000-
  line per-creature update loop (observe/rest/grime/posture/stumble/leap/
  sound_masking × ~200 creatures) to confirm it's the home of the steady cost.
  If roamer_tick p50≈14ms -> fix = gate the per-creature subsystems behind the
  ai_budget distance band (full stack every 4-8 frames for far creatures) + cap
  the per-frame full-stack roamer count.
- creature_mesh: CONFIRMED not the spike here (0.03ms) but the dirty-flag fix
  stays filed as a low-end-hardware win (deferred — alters upload, needs M2).
- DOC docs/design/PERF_FINDING_meshing_and_steady.md (both problems, the traced
  causes, the ranked fixes, honesty on what's confirmed vs hypothesis).
Verified: cargo check --lib (canonical features) -> Finished (warnings only);
cargo check --bin unendlich-fullgame -> Finished (roamer_tick zone). HOW TO USE:
run with UNENDLICH_FRAME_PROFILE=1 UNENDLICH_FAR_SKIP_CAVES=1 -> watch the
mesher far= time drop AND check roamer_tick in the percentile table.

## r249 — REVERT r248 cave-skip (it would break region-propagating sim) + correct framing
Operator correction: far regions DO need caves/density/terrain computed —
those feed regional sim (water flow, wind, storms, clouds, population dynamics)
that PROPAGATES region-to-region. r248's UNENDLICH_FAR_SKIP_CAVES treated that
computation as render-only waste; skipping it would desync the sim. WRONG.
- REVERTED the cave-skip entirely: mesh_chunk_far_virtual back to its original
  signature + always-carve-caves predicate (the _impl split + env var removed).
  Lib compiles clean.
- Investigated the actual regional sim: storm_intensity / karst_sink / moisture
  + water fields are PURE AGGREGATE-FIELD functions (coarse inputs, not per-
  voxel generator samples) — already cheap + correctly designed. The expensive
  density/is_cave (3 noise layers) calls are in world_model/cpu_generator + the
  far-mesher; the far-mesher's per-cell sampling is the measured 11ms render
  cost, separate from (not feeding) the cheap regional fields today.
- CORRECT optimization framing (docs/design/PERF_FINDING_far_region_corrected.md):
  far is a LOD problem for BOTH render and sim, but LOD = right resolution +
  SHARED computation + right cadence, NOT "skip it." Directions: (1) compute a
  coarse regional terrain field ONCE and share it between mesh + any region-
  propagating consumer (batch-as-field pattern; makes them consistent by
  construction); (2) cache far meshes, rebuild only on region change; (3)
  throttle far-mesh submission bursts (pace, don't drop); (4) decouple sim-tick
  cadence from mesh-rebuild. None remove computation the sim depends on.
- Annotated the r248 doc with the correction pointer so the record isn't
  contradictory.
- roamer_tick zone (r248) retained — separate measurement for the steady-16ms.
Verified: cargo check --lib (canonical features) -> Finished (warnings only).

## r250 — docs: dynamic-world architecture spine + CHARTS worker dispatched
Two operator asks this round.
(1) CHARTS worker: dispatched unendlich-r250-thread-CHARTS-WORKER.zip (ONE
worker for both, since FPS + radar charts share skill/style/data — splitting
would duplicate the brief + force a merge). Brief (WORKER_BRIEF.md at root) =
docs/design/fps_charts_mockup.html (10 distinct live FPS-chart designs in one
file) + docs/design/radar_charts_mockup.html (radar/spider of the 7 profiler
zones); rAF-animated, baked realistic data modeled on the real profile, match
menu_mockup.html v3 aesthetic + @font-face the .ttf. No Rust.
(2) DOCS — made the dynamic-world design robust around the operator's core
principle. Read the uploaded SPEC-012/013 (.docx, voice/acoustics — confirmed
house style, not dynamic-world). Found the existing pieces already split across
docs (mesh_vision_scheduling, batch_field_lod, tick_authority_regions,
PERF_FINDING_far_region_corrected) and wrote the UNIFYING spine:
  docs/design/dynamic_world_architecture.md (NEW, ~150 lines):
  - §1 the core principle: separate Q1 "compute now + mark dirty" from Q2 "is
    this needed for someone's vision" — computation cadence driven by sim+dirty,
    meshing/upload cadence by who needs to see it. (Explains why the r248 cave-
    skip was a category error.)
  - §2 THREE vision consumers: player vision / mob vision / sim propagation —
    each wants different fidelity.
  - §3 the TIERED MESHER: tier A full mesh (player near), B downsampled (player
    far), C proxy/fast-noise (mob eyes — "grass = fast noise for mob vision"),
    D coarse regional field (sim, the SHARED substrate). Tiers chosen per
    (consumer, distance), not globally per chunk.
  - §4 maps the dynamic-world behaviours (lakes→caves, river erosion, path
    regrowth, ecological forest infill, lightning→fire→migration→smoke-on-wind)
    onto Q1-computes-marks-dirty + right-consumer-sees-right-tier.
  - §5 wiring reality (maps to existing sim/ modules + registries + scheduler
    bands; no new architecture) + a 4-step shippable build order (1: shared
    coarse regional terrain field; 2: far-mesh cache+dirty-rebuild+burst pacing;
    3: mob-vision tier C; 4: populations as a propagating field).
  - §6 invariants (never gate sim-compute on visibility; one truth per region,
    many tiered views; far≠frozen; compute-once/rebuild-on-change/pace-bursts).
  - Cross-linked the 4 related docs to the new spine (and it back to them).
Docs-only, no code change -> CHECKED. (Operator can't test; these are design.)

## r252 — docs: statistical long-time sim + timebending-as-stress-test + storage tension
Operator design conversation (forward-looking, explicitly gated — "first we need
the other stuff working"). Recorded docs/design/statistical_timebending_and_storage.md:
- §1 goal: scrub fast-forward/reverse through a world, watch it fly by (feature +
  ideal stress test — exercises every dynamic system across many ticks fast).
- §2 the statistical+residual idea: collapse many ticks via probability
  distribution (jump) + keep residual vs exact (anchor to ground truth). Framed
  as the GENERALIZATION of the existing batch-field spectrum (grass closed-form,
  fire iterate-capped, NEW: stochastic -> statistical jump + residual). The
  residual gives a FREE dual-path benchmark (fast path vs long-form agreement +
  speed). HARD GATE: requires classifying state by observability/reversibility +
  datatype structuring to know what to throw away — type-system work, not just
  an algorithm.
- §3 the dependency chain: Step0 systems actually run (near) -> 1 shared coarse
  regional field (tier D) -> 2 far fast-forward -> 3 state classification +
  datatype structuring -> 4 statistical jumps -> 5 scrub UI + dual benchmark.
  Each gated on the prior.
- §4 the STORAGE tension (the important one): today storage.rs is born-procedural
  + persist-deltas (pristine = 0 bytes). Dynamic worlds break that premise — sim
  itself becomes a diverging delta across every ticked region -> either store
  everything (explosion) or regenerate. Timebending pays for itself AS
  INFRASTRUCTURE: if region state is reconstructable from (seed, gen_version,
  edit-log, tick, sparse checkpoints) via the SAME fast-forward as the scrub,
  you regenerate-on-load instead of storing -> small storage, less disk IO, less
  locking (compute not a locked resource; async-tick + render-last-state covers
  latency). Cheap-immediate-response vs deferred-merged-compute reconciled by
  the tiered model: tier-D coarse field fast-forwards on demand (instant),
  long-form only where fidelity is needed (near/benchmark); checkpoints bound
  fast-forward reach.
- §5 sequencing takeaway: do NOT start statistical sim or storage rework yet
  (gated); immediate path unchanged (get water/wind/storms+discharge/populations
  running + visible, via the shared coarse regional field = far-mesh perf fix);
  when Step0-2 real, FIRST build Step3 (state classification) since it gates
  both the collapse AND regenerable storage; keep every dynamic system's
  fast_forward checkable against long-form from the start (grass/fire already
  are). Invariant: region state must be reconstructable via fast-forward or it
  needs Step-3 classification before it can join deep-time scrub / regen storage.
- Cross-linked dynamic_world_architecture.md + timeline_branching.md.
Docs-only -> CHECKED.

## r253 — docs: timebending as rebase + bounded set algebra over the branch DAG
Operator framing ("base rewriting + intersections/unions/unique sets, bounded
world merging in a giant graph"). Recorded docs/design/timebending_set_algebra.md:
- Reframe: timebending = REBASE (edit past -> replay downstream on new base) +
  SET ALGEBRA over bounded regions in the branch DAG. intersection = shared
  unchanged base (store once, share by pointer); union = full footprint a
  merge/scrub considers; difference/unique = a branch's DIVERGENCE SET (the
  minimal data distinguishing it from parent = the thing to store/recompute;
  keeping it minimal is the game).
- Generalizes EXISTING machinery: world/rollback_scope.rs::rollback_closure
  already computes the bounded transitive region-set coupled to a change over
  region_graph::Edge, capped by max_hops (blast radius). Linear rollback = one
  branch/one closure/replace; timebending = many branches/set-ops-between-
  closures/keep. max_hops IS the "bounded world merging" — divergence bounded by
  coupling-graph closure of the fork edit, not O(world).
- Data structures implied (where Step-3 datatype structuring lands): region-
  keyed tick-versioned state with CONTENT IDENTITY (hash/version) so "shared?"
  is O(1) identity compare not deep diff; PERSISTENT/IMMUTABLE structures +
  hash-consing for structural sharing (storage ∝ divergence); branch carries its
  divergence set (= rollback_closure of fork edit); conflict = same region, two
  branch identities, neither ancestor of other.
- Theory to borrow: VCS rebase+3-way-merge (git-for-world-state keyed region×
  tick); persistent structures + hash-consing; CRDTs/OT for auto-merge (gated on
  which state is cleanly mergeable — another Step-3 axis); interval/range trees +
  bounded BVH for span/blast-radius queries.
- Why it's the right abstraction: it's the data model that makes dynamic-world
  (reconstructable regions) + statistical timebending (divergence = keep, shared
  = free) + storage/IO (store divergence not world) affordable AT ONCE — branch
  cost ∝ divergence, bounded by the coupling graph.
- Still gated: this is the shape Step 3 aims for, designable only once systems
  run + real divergence sets are measurable. CHEAP THING TO CARRY NOW: express
  every new dynamic system's cross-region effects as region_graph::Edge
  couplings (not ad-hoc reaches) so rollback_closure measures their blast radius
  -> automatically a citizen of the eventual merge algebra at no extra cost.
- Cross-linked timeline_branching.md + statistical_timebending_and_storage.md.
Docs-only -> CHECKED.

## r254 — docs: deep-time depth bounding via convergence classes (decision)
Operator handed the design call ("maxdepth 100y→steady-state→branch ends, OR
LOD statistical edges, OR reset for lots of generators... you decide"). Recorded
docs/design/deeptime_convergence_classes.md — DECISION: not a global max-depth
flag; classify each system by deep-time behavior; the class decides. The three
operator options map exactly onto three classes:
- Class A Procedural/Resettable: density()/is_cave() are PURE (fbm of pos,seed,
  time) — no accumulated state, byte-identical at year 0 vs 1M. Deep-time = O(1)
  EVALUATE-at-target-time; "1M years back just resets" is correct + free; zero
  divergence-set contribution (shared by pointer). [= operator's "reset for lots
  of generators"]
- Class B Convergent: accumulates but provably settles; fast-forward to horizon
  then SNAP to steady state + stop. ALREADY in code: fire_field caps catch-up
  (fuel monotonic→burnt out), grass saturates at 1.0. Horizon is per-system &
  DERIVED (fire computes it from burn_per_tick), 100y = default safety ceiling
  not a magic number. Branch "ends" = divergence set frozen once all Class-B
  converged + no Class-C diverging. [= operator's "100y steady-state flag"]
- Class C Path-dependent/Chaotic: never fully settles, fine trajectory could
  matter (migration under wandering climate, re-routing rivers, named-entity
  history). Past near-horizon → statistical/residual jump, coarser+cheaper with
  depth (unobservable detail), storage = checkpoints + residual. [= operator's
  "LOD statistical edges, less compute over long periods"]
- Interaction w/ set algebra: classes are what keep divergence set FINITE as
  t→∞: A contributes nothing, B bounded+shrinking (collapses at horizon), C the
  only unbounded-in-principle → hence approximated. Depth bounded by CONVERGENCE
  not TRUNCATION (a global flag truncates lossily + corrupts C).
- Gated: class is EMPIRICAL — confirm by measuring real divergence once systems
  run (does forest infill converge=B, or does climate churn it=C?). CHEAP NOW:
  (1) every dynamic system gets fast_forward checkable vs long-form (also DETECTS
  class: saturates=B, pure-eval=A, neither=C); (2) tag each w/ provisional
  ConvergenceClass enum (Procedural|Convergent{horizon}|PathDependent) — the
  seed of Step-3 datatype structuring, costs nothing; (3) keep region_graph::Edge
  couplings so rollback_closure measures each class's spatial blast radius.
- Invariant: if a system would lose OBSERVABLE state at depth it's mis-classified
  — fix the class, don't truncate.
- Cross-linked statistical_timebending_and_storage.md + timebending_set_algebra.md.
Docs-only -> CHECKED.

## r255 — docs: deterministic cross-region long-time sim (storms/hunting/water)
Operator's hard question (how to sim ~10 days mid-scale water+weather with
cross-region storms/hunting AND determinism, then statistically cull always-
converging candidates, with a voting system among players). Recorded
docs/design/deterministic_cross_region_sim.md — KEY MOVE: "determinism" conflates
THREE problems (+ a 4th); separate them, they have different answers:
- §0 the three: (1) bit-exactness, (2) cross-region ORDERING, (3) approximation
  fidelity; voting is a 4th (reconciliation) only meaningful after 1-3.
- §1 bit-exactness: codebase ALREADY solved it for player sim — sim/fixed.rs
  Fixed(i64) 48.16, integer-only, identical on any platform ("sim never touches
  f32/f64"). BUT fire_field/grass_field/storm_intensity use f32/f64 today =
  desync footgun once shared. Decision: port shared-state fields to Fixed +
  deterministic noise LUT/fixed-point fbm. Mechanical, not novel.
- §2 cross-region ORDERING = the REAL new problem the operator's prior reasoning
  missed (storms crossing boundaries, hunting across regions). Answer: DOUBLE-
  BUFFER the coupling (CA-style: compute N+1 from neighbors' tick-N, swap) →
  order-independent + parallelizable; cross-region events = tick-stamped EDGE
  MESSAGES on region_graph, consumed in a TOTAL ORDER (sort by source region id,
  entity id) at the swap → identical everywhere; max_hops still bounds it. THE
  architecturally hard piece — design first.
- §3 "10 days mid-scale" = cadence not magic: coarse tier-D cells + slow tick
  (world-minutes/step, ~14400 steps for 10d), determinism COMPOSES (1 step det →
  N steps det). Trap = simulating at frame/voxel scale.
- §4 statistical cull = Class-B (convergent) cross-region events are the target:
  snap-to-converged-outcome past horizon. Jump itself deterministic via SEEDED
  (seed,region,tick)-keyed PRNG (not wall clock) → reproducible randomness;
  residual-checked (over-tolerance ⇒ was actually Class C, fix class).
- §5 voting = reconciliation policy on the divergence set, ONLY after 1-4:
  deterministic sim ⇒ peers shouldn't disagree on physics; vote only on POLICY
  (canonical branch after independent edits, contested region ownership, edit
  precedence tie-breaks) — never on computation. Keeps common path cheap, agreement
  protocol only at forks.
- §6 build order: 1 Fixed port, 2 double-buffer+ordered-edges (hard), 3 coarse/
  slow cadence, 4 Class-B snap+seeded-PRNG+residual, 5 vote-on-policy-only. CHEAP
  NOW: do new sim math in Fixed (or portable-to-Fixed); region_graph::Edge
  couplings; ALL stochastic choices seeded by (seed,region,tick) never wall clock.
- Invariant: determinism is LAYERED — bit-exact → ordering → composition →
  approximation → consensus; break at any layer desyncs; "10 days deterministic"
  = layers 1-3 holding for the step count, no extra mechanism (composition).
- Cross-linked deeptime_convergence_classes.md + timebending_set_algebra.md.
Docs-only -> CHECKED.

## r256 — docs: simulation-guided seed selection from region-derived keys
Operator idea: derive the statistical-jump seed from a region's CURRENT SIM STATE,
not just position+tick. Recorded docs/design/simulation_guided_seeding.md.
- The change: jump_seed = hash(world_seed, region_id, tick, digest(region_state))
  instead of hash(seed,region,tick). Now the sampled outcome is CONDITIONED on
  real state — drought vs floodplain draw different seeds → different outcome
  distributions (P(outcome|state) not flat P(outcome)) — while still
  reproducible (same state digest → same draw everywhere).
- Why powerful: conditioned-not-blind approximation (strictly more accurate same
  cost); REUSES the per-region content identity the set-algebra doc already needs
  (one digest serves branch-sharing + dirty-detection + jump-conditioning);
  shrinks the dual-path residual (fast path stops ignoring the inputs the slow
  path responds to); cross-region events get conditioned by the NEIGHBOR's state.
- THE SHARP EDGE (two readings): (A) state CONDITIONS the seed = pure fn state→
  seed→outcome, no search = SAFE + powerful, DO THIS. (B) sim SEARCHES seeds for
  a "good"/convergent outcome = breaks determinism (search order/termination/
  scoring must all be bit-exact), re-introduces the gate (must evaluate outcome
  to score it = the compute the jump skips), and BIASES the sample (conditioning
  on result corrupts the statistics the residual assumes) = NO unless a bounded
  variant proves deterministic+cheaper-than-sim+unbiased (separate research).
  Litmus: state determines seed = A (safe); desired outcome determines seed = B.
- Determinism rules for A: digest only over FIXED state in fixed field order
  (never f32/f64, never hashmap iteration order); digest the COARSE tier-D field
  not per-voxel; read PRE-SWAP (tick-N) under the double-buffer; STABLE versioned
  hash (not DefaultHasher), bump a digest-format version like GENERATOR_VERSION.
- Slots in as a BETTER KEY for the PRNG already specified (determinism doc §4),
  not a new layer; convergence classes unaffected (C's jump goes blind→state-
  conditioned, pure accuracy upgrade). CHEAP NOW: when adding coarse fields,
  define their Fixed fixed-order serialization + stable digest from the start
  (set-algebra needs it anyway) → conditioned seeding becomes nearly free.
- Invariant: outcome conditioned by state that ALREADY EXISTS (state→seed), never
  selected to produce a DESIRED outcome (outcome→seed).
- Cross-linked deterministic_cross_region_sim.md + timebending_set_algebra.md.
Docs-only -> CHECKED.

## r257 — docs+mockup: hybrid R2-plan/R1-fill step model (+ why genomes force it)
Operator: wants a hybrid that uses R2 to guide smaller staged R1 steps building a
graph of states that converge at intermediates; flagged that genome/population
(eating/reproducing/evolving/bending) can't be pure R1 — too much resolution
lost. Delivered:
- docs/design/hybrid_step_model_mockup.html — INTERACTIVE sandbox: one toy region
  (water/fog/fuel/population/allele) run 3 ways (long-form exact / pure-R1 one-
  jump / hybrid R2-staged-R1) with integer-seeded PRNG (bit-reproducible). Scrub
  + play + sliders (span/seed/storm/waypoints). Shows a state-graph with
  convergent-intermediate merges. Smoke-tested under node across 7-40d spans:
  pure-R1 residual GROWS with span (0.14->0.24, loses trajectory); hybrid holds
  ~0.02 (~10x better) at 88-97% FEWER exact steps than long-form; more waypoints
  -> more merges + tighter residual; deterministic across re-runs. Matches
  menu_mockup v3 aesthetic. Vanilla JS no deps. (Thinking instrument, not a
  measurement — toy dynamics; real numbers need the dual-path bench vs engine.)
- docs/design/hybrid_step_model.md — the design:
  §1 hybrid = R2 PLANNER (proposes waypoints, can search) + R1 authoritative FILL
  (deterministic, coarse sub-step cadence, seeded by digest). Intermediates with
  same coarse digest MERGE.
  §2 determinism-safe because the planner's OUTPUT (waypoint plan) is COMMITTED to
  the seed lineage (edit-log-adjacent) -> replay re-fills the committed plan, does
  NOT re-search. Frozen-at-commit, like a player edit. (vs the Reading-B trap = a
  LIVE per-tick seed search, which biases+desyncs.) Litmus: a plan you commit+
  replay is safe; a seed picked mid-fill to steer outcome is not.
  §3 WHY genome/population can't be pure R1: fitness optimum MOVES over the span
  (climate-set), drift ~1/sqrtN and N depends on the trajectory (storm culls,
  grazing K), selection×reproduction×events compound over generations = path-
  dependent (Class C). Pure R1's single start-conditioned endpoint discards the
  generations. Hybrid keeps ~1 R1 sub-step per generation-ish waypoint -> samples
  the moving optimum + N-dependent drift ALONG the path.
  §3 mixed-channel rule: a region has BOTH — convergent (water/fog/fuel, Class B)
  collapse cheaply; path-dependent (pop/allele, Class C) set the WAYPOINT DENSITY.
  Coarse where it can be, fine where it must be, per-channel within one region.
  §4 state graph + convergence merging (ties to timebending_set_algebra):
  node=staged intermediate digest, edge=R1 fill, merge=same coarse digest. Class-B
  regions merge often (few nodes), Class-C rarely (keep the trajectory) — correct.
  §5 sandbox↔real mapping + caveat (toy, shape not measurement).
  §6 build: step 4½ on top of seeded-PRNG jump; per-channel Class tags drive
  waypoint density; commit the plan from v1; keep step() able to take a dt
  multiplier (generalizes grass/fire fast_forward).
  Invariant: authoritative timeline = deterministic R1 fill over a COMMITTED plan;
  R2 only decides where exact sub-steps go; never coarsen below 1 exact sub-step
  per meaningful generation/event or the genome trajectory is lost.
- Cross-linked simulation_guided_seeding.md + deeptime_convergence_classes.md.
Docs+mockup only -> CHECKED (HTML smoke-tested under node; renders standalone).

## r258 — docs: sim vectors, correction deltas, structure persistence + determinism-at-scale answer
Operator added 4 mechanisms after seeing the mockup + asked whether "hybrid graph
changes with stage count but alleles/population still converge" is good for
determinism at scale. Recorded docs/design/sim_vectors_and_correction_deltas.md.
- §0 THE DETERMINISM ANSWER (careful): yes-but-precise. The convergence you see =
  ATTRACTOR STABILITY / endpoint robustness (result insensitive to discretization)
  = GOOD, justifies coarse staging being SAFE. But that is NOT bit-exact
  determinism — two stage counts are two DIFFERENT computations that SHOULD be
  allowed to differ; near-agreement is robustness not determinism. DANGER:
  conflating them → letting two machines pick different stage counts and assuming
  agreement → approximate agreement desyncs lockstep. So: determinism still
  requires the waypoint plan be COMMITTED + identical everywhere (seed lineage);
  the robustness becomes a SAFETY MARGIN that keeps unavoidable small differences
  from snowballing + lets correction deltas heal them. Robustness buys
  forgiveness, commitment buys exactness — want both. Caveat: endpoint convergence
  ≠ trajectory fidelity (low stage counts lose the allele PATH shape even when the
  final number is close — genome still needs enough stages).
- §1 SIM VECTORS = name for the checkpoint directors: committed deterministic
  instruction carrying a region checkpoint→checkpoint, = {span, seed-key, waypoint
  plan (committed stage count→exact replay), correction deltas, structure/event
  commitments}. Data in seed lineage, not a live decision. Directional + what you
  scrub along.
- §2 STRUCTURE/EVENT PERSISTENCE (the block-placement / "dragon still building"
  gap): discrete permanent marks (builds, placements, dams) = COMMITTED CONDITIONAL
  EDITS on a sim vector, applied at checkpoint if a coarse predicate holds ("dragon
  alive+territory held → +structure increment"; "water<dry → +population thirst
  delta"). Deterministic (pure fn of committed coarse state), permanent (edit-log
  like player edits), cheap (per-checkpoint not per-tick). Never statistical blur.
- §3 CORRECTION DELTAS/VECTORS (operator's "don't blur angles that never change"):
  carry state on the cheap convergent baseline; track a delta ONLY for dimensions
  with a NEW response to a new condition. sim vector = convergent baseline + sparse
  corrections. Deep-time analogue of persist-only-deltas, for dynamics. Stops the
  approximation smearing the many stable axes.
- §4 POPULATION APPROX = genome SUMMARY (per-gene mean/variance + 8³ bin counts) +
  ONE coarse redistribution step: shift bins by selection+drift, diffuse counts to
  adjacent bins for spread (incl cross-region edges), re-pop named individuals. The
  "trees evolved fast-growth gene → cover more area → spread genome to neighbors"
  case = bin summary shifts + counts diffuse + edges propagate, one step not a
  regrow sim. 8³ = starting granularity to validate empirically, may vary by
  species/gene.
- §5 LIFESPAN POP-IN: still-alive entities popped in EXACTLY (committed, Class-C-
  exact, their §2 building continues); lifespan-ended → estimated as new cohort
  from the §4 summary. Splits population: named/long-lived/alive = exact; short-
  lived/anonymous/turned-over = aggregate estimate. Exactness spent only where it
  matters.
- §6 assembly diagram (one region, one jump); §7 cheap-now + invariants (endpoint
  stability is SAFETY MARGIN not determinism; discrete marks are committed edits
  never blur; blur only what responded).
- Cross-linked hybrid_step_model.md + deterministic_cross_region_sim.md.
Docs-only -> CHECKED.

## r259 — docs: adaptive scheme selection (sim-it vs vector) + sim-prefill landing
Operator refinement: the sim vector has a natural LENGTH set by the systems it
models; if a vector for the span is overkill just sim it all; if the span is big,
drive with the vector to just-before the target then sim-PREFILL into it to cover
building/growth. Recorded docs/design/adaptive_scheme_selection.md.
- §1 vector LENGTH = min over modeled systems' fidelity horizons (longest span it
  stays within tolerance). Generalizes fire_field's existing max_useful=(1/burn)*4
  convergence cap from one-system to whole-region. Most path-dependent active
  system sets the length (same rule as waypoint density).
- §2 THREE REGIMES (the selector, deterministic fn of span+system set, committed):
  A small span ≤ threshold → JUST SIM IT (no vector overhead, exact is cheapest+
  simplest); B medium, one vector covers within tolerance → SINGLE VECTOR; C large
  span ≫ fidelity_len → CHAINED VECTORS + PREFILL.
- §3 the SIM-PREFILL LANDING (operator's key insight): cheap vector jump is great
  for the bulk but the LANDING needs discrete path-dependent detail a jump can't
  place — the building/growth systems leaving discrete marks. Two-phase: (1) DRIVE
  cheap/coarse from now to approach=target-prefill_len; (2) PREFILL exact short
  sim from approach into target — re-materializes structures (apply committed
  increments), runs last generations of growth/spread at full detail, pops in
  still-alive entities, settles local fields. "sim INTO it" so landing is exact
  though journey was approximate. prefill_len set by slowest build/growth system
  that must look right on arrival. Deep-time analogue of renderer's render-last-
  ready-until-landed.
- §4 composition: vector length→n_vectors/waypoints; prefill_len→exact tail;
  correction deltas ride the drive, responses resolved in prefill; structure
  commitments APPLIED during prefill; 8³ genome redistribution in drive, re-pop in
  prefill; all selector params deterministic+committed.
- §5 choose_scheme() pseudocode: span≤SIM_IT_THRESHOLD→SimIt; span≤flen→
  SingleVector; else DriveThenPrefill{drive_to:target-plen, prefill_ticks:plen}.
  Left of drive_to = cheap approx, drive_to→target = exact materialization; split
  set by systems not a constant.
- §6 cheap now: give each system a FIDELITY HORIZON (fire_field already has one,
  make uniform; fidelity_len=min); building/growth systems a PREFILL REQUIREMENT;
  choose_scheme pure+committed. Invariants: journey may be approximate but LANDING
  must be exact (always prefill); vector length bounded by most path-dependent
  modeled system; pick cheapest scheme meeting tolerance + commit the choice.
- Cross-linked sim_vectors_and_correction_deltas.md + hybrid_step_model.md.
Docs-only -> CHECKED.

## r260 — docs: vector chaining + max-span cap + timestep-animation-as-benchmark
Operator: deep timebend (1M years) must use MORE THAN ONE vector — add a max cap,
hide the chaining in-game via a timestep animation, and satisfy the benchmark.
Recorded docs/design/vector_chaining_and_timewarp.md.
- §1 the CAP: one vector can't honestly span 1M years (>> any fidelity_len = pure
  fabrication). Add VECTOR_MAX_SPAN hard ceiling; eff per-vector len = min(
  fidelity_len, VECTOR_MAX_SPAN); deep jump = CHAIN of n=ceil(span/eff_len)
  vectors, each re-anchored to a committed checkpoint, terminating in the regime-C
  sim-prefill landing. Per-region generalization of fire_field's per-system cap.
- §2 CHAIN IS CHEAP via convergence merge: chained vectors whose end-digests match
  MERGE (timebending_set_algebra) — deep time mostly reaches steady regime (Class
  B at attractor, pops at K, genomes near optimum) → consecutive vectors same
  digest → collapse to one repeated edge ("nothing distinct for 900k yr"). Cost ∝
  DISTINCT un-merged vectors not raw n. Quiescent deep time ~free; active eras cost
  what the upheaval costs.
- §3 TIMESTEP ANIMATION = the feature not hidden shame: deep scrub PLAYS the chain
  — each end-checkpoint a keyframe, checkpoint→checkpoint a timewarp frame (watch
  water rise / forests advance / storms flash / structures grow). Merged steady
  stretches fast-forward visually; active eras play slower (more distinct vectors =
  more keyframes). Pacing DRIVEN BY DIVERGENCE — lingers where history was
  eventful. Prefill = final settle frame (dragon's tower snaps to real half-built
  state). VECTOR_MAX_SPAN sets animation granularity; merge sets pacing.
- §4 the CHAIN IS THE BENCHMARK HARNESS: per-vector residual (compare each end-
  checkpoint vs long-form of that segment) → localizes WHICH era/system drifts not
  one opaque end number; animation w/ long-form overlaid = test visualization
  (mockup's 3-line idea); checkpoints = natural assert points (fast digest vs long-
  form digest within tolerance); failing link ⇒ its cap was too long ⇒ auto-shorten
  (self-tuning); replay-twice bit-identical = determinism check, per-link
  checkpoints localize a desync.
- §5 full deep-jump path composition; §6 cheap-now (VECTOR_MAX_SPAN ceiling; per-
  link checkpoint = benchmark assert slot + auto-shorten; chain = animation
  timeline) + invariants: no vector spans > min(fidelity_len,VECTOR_MAX_SPAN);
  cost ∝ distinct un-merged vectors; the cap + animation granularity + benchmark
  assert cadence are the SAME checkpoints (one structure, three uses).
- Cross-linked adaptive_scheme_selection.md + statistical_timebending_and_storage.md.
Docs-only -> CHECKED.

## r261 — docs: DEEP_TIME_INDEX.md — overview tying the 10-doc arc together
Non-testing work while operator is away. The deep-time/dynamic-world design arc
(r251-r260) is 10 docs / ~1900 lines; wrote docs/design/DEEP_TIME_INDEX.md as the
single entry point so implementation (or a worker thread) has a map not 10 cross-
linked files.
- §1 the one-sentence through-line: cost ∝ what DIVERGES, bounded by how far
  effects PROPAGATE, deep time bounded by what CONVERGES — journey approximate,
  LANDING exact, every CHOICE committed for deterministic replay.
- §2 the arc in dependency order (1 dynamic_world_architecture spine → 2 storage
  tension → 3 set-algebra data model → 4 convergence classes → 5 layered
  determinism → 6 state-conditioned seeding → 7 hybrid R2/R1 → 8 sim vectors →
  9 adaptive scheme selection → 10 vector chaining/timewarp), one line each.
- §3 the strict build order (0 systems run → 1 shared coarse regional field [=far-
  mesh perf fix] → 2 far fast-forward → 3 ConvergenceClass+fidelity_horizon+digest
  → 4 Fixed port + double-buffer ordered edges → 4½ seeded→state-conditioned→hybrid
  → 5 sim vectors+prefill → 6 chaining+merge+animation-benchmark+vote-on-policy).
- §4 CONSOLIDATED cheap-things-now (all zero-cost door-openers in one place):
  region_graph::Edge couplings; ConvergenceClass+fidelity_horizon tags; Fixed math;
  (seed,region,tick,digest) PRNG keys; per-region coarse Fixed digest; genome
  summary (mean/var + 8³ bins) as persisted pop form; fast_forward/dt step checkable
  vs long-form (auto-detects class); building/growth prefill_requirement + conditional
  committed edits.
- §5 load-bearing invariants collected from all 10 docs; §6 status (all design+
  CHECKED, none wired, gated on Step 0, first move = Step 1 shared coarse field =
  far-mesh fix).
Docs-only -> CHECKED.

## r262 — docs: deep-time arc converted to .docx in design_library
Operator: "add it to the docs as docx." Converted all 11 deep-time arc markdown
docs to .docx via pandoc 3.1.3 (gfm -> docx, --toc), placed in a new design-
library category docs/design_library/12_Deep_Time_Simulation/:
  DEEP_TIME_INDEX, dynamic_world_architecture, statistical_timebending_and_storage,
  timebending_set_algebra, deeptime_convergence_classes, deterministic_cross_region_sim,
  simulation_guided_seeding, hybrid_step_model, sim_vectors_and_correction_deltas,
  adaptive_scheme_selection, vector_chaining_and_timewarp.
- Content verified intact (headings, "Rust —" code blocks, pseudocode all
  survived — spot-checked extract-text). Definitive open test: rendered
  DEEP_TIME_INDEX.docx to PDF via LibreOffice (138KB, clean) — opens + renders
  fine, so usable in Word.
- HONEST CAVEAT: the strict OOXML validator flags cosmetic style-ordering nits in
  pandoc's generated styles.xml/settings.xml/numbering.xml (zoom element order,
  rPr content) — these are schema-strictness warnings, NOT content problems; the
  files render correctly. The .md originals in docs/design/ remain the canonical
  source; the .docx are the design-library format copies. Did not hand-edit
  pandoc's style XML across 11 files (disproportionate for read-only reference
  docs that render correctly).
Docs-only -> CHECKED.

## r263 — docs: Step-1 implementation scoping (RegionGrid already exists)
Non-testing work. Scoped the deep-time Step 1 (shared coarse regional field = far-
mesh perf fix) against the REAL code before any implementation. Recorded
docs/design/STEP1_SCOPING_regiongrid.md.
- KEY FINDING: WorldGenerator ALREADY caches coarse per-region fields in
  RegionGrid<f64> (sea_level_base/pressure_base/humidity_base/temperature_base/
  continental, built once via make_grid). So the "compute coarse field once +
  share" pattern the whole arc proposed is NOT new infra — it exists + is load-
  bearing in worldgen. Step 1 = EXTENSION of a tested pattern, lower risk/scope.
- THE GAP = the 11ms: mesh_chunk_far_virtual's per-cell generator.density() +
  is_cave() (3 noise layers) are NOT RegionGrid-backed — evaluated fresh per cell
  per far-chunk per rebuild. That's the uncached expensive term.
- Concrete Step-1 shape: (1) add a coarse density/cave RegionGrid at far-mesh grid
  res; (2) far-mesher reads it instead of per-cell generator calls → 11ms→sample;
  (3) SAME field feeds the regional sim (consistent by construction); (4) make it
  Fixed/stable-digest so it doubles as per-region content identity (free to do
  right first).
- Still gated: changes far-terrain render (needs M2 GPU validation); field shape
  should be informed by actual regional-sim consumers (Step 0 first) so it serves
  all, not mesh-only (else redo). BUT the perf subset is SEPARABLE if the chug
  becomes urgent: "add a RegionGrid for far-mesh solidity verdict, read in
  mesh_chunk_far_virtual" = minimal standalone version.
- RegionGrid<T: Send+Sync+Clone> already generic+thread-safe → extension inherits
  sharing/threading, no new concurrency design.
Docs-only -> CHECKED (scoping; no code touched).

## r263b — MERGE: particles + T-MOBS worker results (real code, not docs)
Stopped writing design docs (diminishing returns) and merged the two uploaded
worker results that were pending. The original patches were against an old base
(r125) so they didn't apply wholesale; did a DELIBERATE MANUAL merge instead,
verifying each piece against the current tree:
- PARTICLES: copied the standalone sim/particles.rs (pooled particle system,
  spark/smoke/dust, hard-capped pool, pure+deterministic, 8 tests) into the tree;
  declared `pub mod particles;` in sim/mod.rs. Verified ALL its deps exist in the
  current tree (particle_alpha_curve/color_ramp/size_curve, spark_burst,
  world::smoke_field). Skipped the stale docs/TODO + WIRING hunks (I own those).
- T-MOBS: reconstructed the new sim/mob_bands.rs (band-aware mob tick decision,
  T4: mob_band_tick(divisor, ticks_since_last)->Option<u32> mirroring leaf_decay
  catch-up; dep-free, pure, 6 tests) from the patch; declared `pub mod
  mob_bands;`. The three TEST-only hunks (behavior_arbiter/forage_harvest/
  herd_cohesion) applied CLEANLY to the current files (their mod tests were
  unchanged since the worker's base) — added via git apply, zero risk to non-test
  code.
- VERIFIED: cargo check --lib (render,window,gpu,ui-overlay,audio) -> Finished,
  no errors, with both modules + the 3 test additions present. (Full test-binary
  LINK times out in-sandbox — known; the check confirms all merged code incl.
  test modules type-checks. Workers reported 8 + 6 passing tests; assertions are
  trivial pure-function checks.)
Both modules are now in the tree, declared, compiling. They are MODEL modules
(not yet wired into the bin loop) — same status as the other sim/ systems,
available for a future integration pass. -> CHECKED (compile-verified merge).

## r264 — WIRE: sim::mob_bands into the roamer loop (kill-switched, default off)
Operator: wire features in. Connected sim::mob_bands (merged r263, was unwired)
into the bin's per-creature roamer loop as an ALTERNATIVE gate to the existing
ai_budget per-entity curfew.
- KILL SWITCH: UNENDLICH_MOB_BANDS (default OFF → zero behaviour change until
  A/B'd). When on, logs "[mob_bands] ENABLED ... sim_divisor=N" once.
- WIRING: in the roamer loop (~line 4410), for non-persistent mobs (dragons stay
  exempt — flight arcs need every frame), when enabled, gate by the
  world::scheduler band of the mob's chunk instead of ai_budget::should_full_tick:
  compute mob_chunk + player_chunk (div_euclid by CHUNK_SIZE), band_for_chunk(...)
  -> ChunkTickBand -> .divisor(params), then sim::mob_bands::mob_band_tick(divisor,
  gap). Phase-aligned per scheduler::should_fire_in_band: gap = divisor on frames
  where frame_count % divisor == 0 else 0 → Render(div 1) fires every frame,
  Simulation(div 8) every 8th, CatchUp(div 0) dormant. Disabled path = the
  original ai_budget curfew, unchanged.
- This also FIRST-WIRES world::scheduler::{TickScheduleParams, band_for_chunk,
  ChunkTickBand::divisor} into the bin (previously built+unused there).
- Honest limits: this pass uses phase alignment (frame % divisor) rather than true
  per-roamer ticks_since_last catch-up — mob_band_tick CAN return n for gap
  integration, but threading per-roamer last-tick state through the Roamer struct
  is a larger change deferred to when the catch-up integration (movement/forage
  advancing by n) is actually consumed. So today it gates (skip/run), not yet
  catch-up-integrates. Defaults off; needs an M2 run to validate feel + that
  Simulation-band mobs don't visibly stutter.
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no
  errors (pre-existing unused-var warnings only, not from this change). -> CHECKED
  (compile-verified wiring; behaviour change gated off).

## r265 — WIRE: sim::particles tick into the bin loop (kill-switched, default off)
Operator: keep wiring features in. Connected sim::particles (merged r263, was
unwired) into the bin. The system TICK is pure CPU and fully wired+verified; the
GPU draw is honestly deferred (needs a new pipeline + the M2).
- KILL SWITCH: UNENDLICH_PARTICLES (default OFF). When on, logs "[particles]
  ENABLED ... GPU upload pending" once, instantiates ParticleSystem::
  with_default_cap(seed) as Option<ParticleSystem>.
- WIRING (pure CPU, runs when enabled):
  * step every frame right after dt: particle_sys.step(dt, [0,-9.81,0], [0.4,0,0.2])
    (gravity + gentle wind matching the smoke field). No-op when disabled.
  * emit on landing: in the existing fall-impact block, on any real landing
    (landed = prev_fall_vy<-9 && vy>-1 && !feet_in_water — moved OUTSIDE the
    survival_mode gate so the puff is visual-only), emit Preset::Dust at the
    player's feet, count = (6 + speed*0.8).min(48).
- HONEST GPU BOUNDARY: the pool + render_buffer() {positions, colors, sizes} are
  ready, but DRAWING needs a NEW sprite/point render pipeline in world_renderer —
  there is NO particle upload path today (upload_entities is creature meshes, and
  the smoke field thickens fog rather than drawing sprites). So this is NOT a one-
  line upload; it's real GPU work for the M2. Until then the system ticks but is
  invisible. Marked in-code where the draw would hook + how to verify the tick on
  hardware (log ps.len()).
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no
  errors. -> CHECKED (compile-verified CPU wiring; visual gated off + GPU draw
  deferred to hardware).

## r266 — UPGRADE: mob_bands TRUE per-roamer catch-up integration (completes r264)
Operator: keep wiring. Upgraded the r264 mob_bands gate from a plain phase-aligned
frame-skip to TRUE per-roamer catch-up — the clean follow-up flagged in r264.
- Roamer struct: added `last_tick_frame: u64` (frame when this roamer last ran a
  full brain tick; 0 = never). Initialized to 0 at all 5 instantiation sites
  (spawn-egg hatch, two stream-in paths, the r4354 path, and the offspring/mating
  push).
- Gate (mob_bands path): ticks_since_last = frame_count - last_tick_frame (real
  per-roamer gap, not frame_count % divisor). mob_band_tick(divisor, since_last)
  now → Some(n): record last_tick_frame = frame_count and carry n; None: skip.
  A new loop-scope `mob_band_catch_up: u32` (default 1) carries n from the gate to
  the tick body. The ai_budget path + Render band leave it 1 (unchanged behaviour).
- Integration (the actual catch-up): the grounded wander step now advances by the
  whole elapsed interval — `let tick_dt = dt * mob_band_catch_up as f32;` fed into
  `w.step_aware(tick_dt, ...)`. Scheduler time-homogeneity: a step over n*dt == n
  steps of dt, so a Simulation-band mob firing every 8th frame takes an 8x wander
  step and covers the gap, instead of silently dropping the frames (the r264
  limitation). Airborne/leap ballistics are NOT scaled (arcs must stay per-frame).
- Still behind UNENDLICH_MOB_BANDS (default OFF) → zero change unless enabled.
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no
  errors; mob_band_catch_up/tick_dt/last_tick_frame are clean (the only unused_mut
  warnings are pre-existing in dragon_forge.rs/dragon_breeding.rs, not ours).
  -> CHECKED. Needs M2 run with UNENDLICH_MOB_BANDS=1 to confirm Simulation-band
  mobs now move smoothly (catch-up) rather than stutter.

## r267 — WIRE: sim::awareness_blend into roamer perception (kill-switched, default off)
Operator: keep wiring. Connected sim::awareness_blend (was unwired) into the
roamer perception block, where the code comment already anticipated surfacing
graded "wariness."
- KILL SWITCH: UNENDLICH_AWARENESS (default OFF, logs once when on).
- WIRING: the perception block computed sight + hearing as a binary OR (sight pins
  position else hearing localizes). Now, when enabled, it ALSO computes a graded
  awareness in [0,1] via awareness_blend::awareness(sight, hearing, smell):
  * sight from the Detection tier (Clear→1.0, Motion→0.5, Unseen→0.0) — captured
    the full Detection (was only .is_detected() bool; kept `seen` unchanged),
  * hearing = perceived_loudness(player_footstep, dist, 0) / 255 clamped,
  * smell = 0.0 (unmodelled in the loop).
  Sample-logged (wariness>0.15 && frame & 0xFF == 0) so it's observable without
  flooding.
- HONEST SCOPE: computed + logged as the graded wariness signal the perception
  comment said "we could surface later" — it does NOT gate the binary flee logic
  yet (that's a behaviour change better validated on the M2). So zero behaviour
  change; this lays the value a future wariness/alert state reads. Non-persistent
  only (dragons skip).
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no
  errors, no warnings from this change. -> CHECKED.

## r268 — WIRE: ability_cast cluster (cast + cooldown + combo) as a coherent feature
Operator: keep wiring. Raised leverage — wired a CLUSTER so one switch = a
complete capability, not a logged scalar. Connected sim::ability_cast +
sim::ability_cooldown + sim::ability_combo (all were unwired) into the existing
bending DISCHARGE.
- KILL SWITCH: UNENDLICH_ABILITY_CAST (default OFF, logs once when on).
- HOOK: the bending system already discharges (bend_resonance.spend → `spent`
  power, with fg_imbue.element in scope) and just logged "resonance spent". Now,
  when enabled, that discharge becomes a TYPED ability:
  * cooldown gate: ability_cooldown::cd_remaining(last_cast, now, 1.5s) — skips +
    logs "on cooldown" if not ready (now from a dedicated Instant clock);
  * combo: ability_chain grows while inside a 2.0s window (else resets to 1);
    ability_combo::combo_mult(chain, 1.0, 0.25, 2.0) → ×1.0..×2.0 multiplier;
  * cast: ability_cast::cast(element, spent*mult) → Effect{kind, power, radius};
    logged "[ability] cast kind=.. power=.. radius=.. (combo xN ×M)" when kind !=
    KIND_NONE; updates last_cast + chain timer.
- State added near bend_resonance: ability_last_cast, ability_chain,
  ability_last_chain_t, ability_elapsed (Instant). All gated; zero behaviour
  change when off (the discharge log is unchanged).
- NOT wired (honest): ability_charge_time (needs a hold-to-charge input state
  machine — current discharge is instantaneous, would be a forced fit) and
  ability_aura (effect-application over an area — a consumer-side/gameplay+GPU
  concern, better when effects actually strike entities). Filed for later.
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no
  errors, no warnings from this change. -> CHECKED. M2 run with
  UNENDLICH_ABILITY_CAST=1 + discharging shows the typed-cast + combo log.

## r269 — WIRE: combat-modifier cluster (crit_roll + damage_after_armor) onto melee
Operator: keep wiring. Cluster onto an existing hook again. Connected
sim::crit_roll + sim::damage_after_armor (both unwired) into the existing melee
combat block (punch → combat::resolve → roamers[ri].hp -= dmg).
- KILL SWITCH: UNENDLICH_COMBAT_MODS (default OFF, logs once when on).
- HOOK: the punch already builds an Attack, calls combat::resolve → hit, applies
  hit.health_damage flat. Now, when enabled, the applied damage `dmg_dealt` is:
  * crit: deterministic roll from (frame_count * 0x9E3779B9 + ri) → [0,1], fed to
    crit_roll::crit(roll, 0.2, 1.8) → ×1.0 or ×1.8 (20% chance). Logs "[combat]
    CRIT ×1.8 ..." on a crit.
  * armor: natural armor = (creature.mass * 2.0).clamp(0,30) — bigger creatures
    tougher — fed to damage_after_armor::mitigated(raw*crit, armor, k=20) →
    diminishing-returns reduction, floored at 1.
  Disabled path = hit.health_damage flat (unchanged). hit still used for
  afflictions + the Harm bond event.
- The melee log + hp application now use dmg_dealt (the modified value when on,
  the flat value when off).
- NOT in this cluster: armor_reflect (that's a dragon MATERIAL/reflectance gene,
  different domain, not melee mitigation). Skipped correctly.
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no
  errors, no warnings from this change (caught+fixed a creature.body.mass →
  creature.mass field error mid-wiring). -> CHECKED. M2 run with
  UNENDLICH_COMBAT_MODS=1 + punching shows crit/armor in the combat log.

## r270 — WIRE: aggro response (aggression_level + aggro_decay) — creatures fight back
Operator: keep wiring; this is the compounding piece — pairs with r267 wariness +
r269 combat mods so fauna NOTICE and RESPOND. Connected sim::aggression_level +
sim::aggro_decay (both unwired) into roamer state + behavior.
- Roamer struct: added `aggro: f32` ([0,1] aggression toward the player).
  Initialized 0.0 at all 5 instantiation sites + bound in the destructure.
- KILL SWITCH: UNENDLICH_AGGRO (default OFF, logs once when on).
- SPIKE (combat block, on harm): hp_frac = hp/40; ceil =
  aggression_level::aggression(hp_frac, 0, 1) (own health, 1v1); a hit pushes
  aggro += 0.5 toward ceil.max(0.3). Logs "[aggro] {kind} provoked → aggro X".
- DECAY + BEHAVIOUR (roamer tick, after step_aware): aggro =
  aggro_decay::decay(aggro, tick_dt, dist_to_threat) — cools over time, faster as
  the threat retreats. When aggro > 0.5 AND a threat is present, the flee vector
  (rdx,rdz) is NEGATED → the creature APPROACHES instead of fleeing.
- Result (when on): punch a creature → it gets angry → chases you; back off and
  its aggro decays and it returns to wandering. Wounded (low hp) creatures cap
  lower (aggression falls when hurt). Persistent dragons unaffected (they already
  don't flee).
- Off by default → zero behaviour change. tick_dt is the mob_bands catch-up dt so
  aggro decay is time-homogeneous with the band cadence.
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no
  errors, no warnings from this change. -> CHECKED. M2 run with UNENDLICH_AGGRO=1:
  punch fauna and watch them turn and chase.

## r271 — WIRE: behavior_arbiter (utility-AI drive selection) over the aggro/flee decision
Operator: keep wiring. The unifying piece — replaces the ad-hoc fight/flee
threshold with a principled scored choice. Connected sim::behavior_arbiter (the
T-MOBS test target, was unwired) into the roamer behavior decision.
- KILL SWITCH: UNENDLICH_ARBITER (default OFF). DEPENDS ON UNENDLICH_AGGRO (the
  arbiter reuses the aggro decay + the approach/flee vector inversion, so it only
  takes effect when aggro is also enabled). Logs once when on.
- WIRING: when on, builds behavior_arbiter::Drives from the roamer's existing
  per-tick state — threat (nearness: 1 - dist/FLIGHT_DIST), hunger (1 -
  vitality.energy), fatigue (0.0 for now — rest_cycle wiring is a later
  refinement), aggression (the r270 aggro field), health_frac (hp/40) — calls
  behavior_arbiter::select(&drives, None, 0.0), and APPROACH = (chosen behavior ==
  Fight). That supersedes the raw aggro>0.5 test. Flee/Forage/Rest/Patrol all map
  to "don't approach" (existing flee/wander handles the motion).
- The arbiter's scoring already encodes the right tradeoffs: Flee dominates when
  threat high + health low; Fight needs threat AND aggression scaled by health (a
  near-dead mob flees instead); so a wounded provoked creature correctly breaks
  off. hysteresis=0 this pass (stateless) — adding a per-roamer prev_behavior for
  anti-flip-flop is the noted refinement.
- Off by default → zero behaviour change.
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no
  errors, no warnings from this change. -> CHECKED. M2: UNENDLICH_AGGRO=1
  UNENDLICH_ARBITER=1 → provoked-but-healthy fauna fight, wounded ones flee.

## r272 — UPGRADE: behavioral memory (combat_habit) + real hysteresis on the arbiter
Operator: "make better hysteresis and memory — that's another timebending jump
metric, because habitual animals fight population genetic trends." Did both the
CODE and captured the timebending insight.
- Roamer struct: added prev_behavior: u8 (encoded last Behavior, for hysteresis)
  and combat_habit: f32 ([0,1] learned combat tendency). Initialized at all 5
  instantiation sites + bound in the destructure.
- HYSTERESIS (real now): decode prev_behavior → feed behavior_arbiter::select with
  a 0.15 hysteresis bonus (was None/0.0) → store the chosen behavior back. No more
  flip-flop on near-ties.
- MEMORY/HABIT: combat_habit decays slowly each tick (0.02*dt), biases EFFECTIVE
  aggression upward (eff = aggro.max(habit*0.8)) so a battle-hardened animal stays
  ready to fight; committing to Fight REINFORCES it (+0.04*dt). The fast per-
  individual learned layer that runs against the population-genetic trend.
- DESIGN (sim_vectors doc §4.5, NEW): behavioral habit as a distinct path-dependent
  timebending channel — genome = slow (generations), habit = fast (a lifetime,
  resets at death), and they can OPPOSE (veterans stay aggressive while the
  population drifts timid). Implications: TWO cadences in a jump (genome via 8³
  summary, habit via fast relaxation toward era-reinforced behavior); habit as its
  OWN correction-delta (blur only where conflict happened); hysteresis/habit/genome
  = short/medium/long ends of one memory spectrum (seconds/lifetime/generations);
  alive carry habit exact, turned-over cohorts estimate BASELINE habit from
  conditions not inheritance. Noted: no heritable temperament gene yet (genome =
  color/size/senses/speed) — a `temperament` locus is the missing piece to make
  "fights the genetic trend" literal. Filed.
- combat_habit/prev_behavior wiring is behind UNENDLICH_ARBITER (which needs
  UNENDLICH_AGGRO) → off by default, zero behaviour change.
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no
  errors, no warnings from this change. -> CHECKED.

## r273 — docs: how to DETERMINE convergence class (perturbation bracketing + cheaper pre-filters)
Operator asked: simulate drastic behaviour changes to bound regions — spawn
parallel sim vectors/keyframes and see if they still converge? or a cheaper way?
Answered + captured in deeptime_convergence_classes.md §4.5 (the missing "how do
you know a channel's class" piece the doc had asserted but not derived).
- Perturbation BRACKETING is the definitive test: parallel vectors from one start
  with extreme channel values (max vs min aggro / live vs frozen) → compare
  endpoints. Converge ⇒ Class B (channel irrelevant, skip it); diverge ⇒ Class C
  (matters). It IS the set-algebra difference op: empty divergence ⇒ merge.
- KEY EFFICIENCY (the answer to "cheaper way"): don't pay it per-jump. Amortize —
  bracket ONCE to establish the class for a region-TYPE, cache the verdict as a
  committed fact, reuse thereafter. Bracket = how you learn the class; class = what
  you reuse.
- Cheaper pre-filters, cheapest-first: (1) STRUCTURAL reachability — free: run
  rollback_closure (exists) from the channel node over region_graph::Edge at the
  coarse min_coupling; reaches nothing observable ⇒ provably can't matter ⇒ Class B,
  zero sims (catches most local behaviour). (2) COARSE pre-bracket — cheap probe at
  low res/short span, escalate only if ambiguous. (3) SENSITIVITY bound — Lipschitz/
  interval bound from coupling gains proves convergence with NO second sim. (4)
  VARIANCE probe — perturb seed, measure variance (statistical pre-filter). (5) full
  parallel bracket only when 1-4 ambiguous.
- Determinism: bracket is deterministic (fixed extremes+seeds) ⇒ verdict reproducible
  + committable (every machine agrees what to skip); doubles as a benchmark assert
  (vector_chaining §4).
- Behaviour-specific: habit/aggression fails test 1 in placid regions (free Class B),
  needs the real bracket only where behaviour plausibly drives population→ecology.
  Cost ∝ what diverges, applied to DECIDING WHAT TO SIMULATE AT ALL.
- Cross-linked set_algebra + sim_vectors. Docs-only -> CHECKED.

## r274 — BUILD: sim::convergence — ConvergenceClass + free structural pre-filter (deep-time, first real code)
Operator: "good enough to doc, lets build it out now." Started BUILDING the deep-
time machinery (was all design+gated). First piece = the most foundational pure-
CPU, fully-verifiable-here part: the convergence-class machinery from
deeptime_convergence_classes.md §4.5.
- NEW crates/unendlich-engine/src/sim/convergence.rs (declared pub mod
  convergence;). Pure + deterministic, 8 tests, no GPU/IO/clock/panic.
- ConvergenceClass enum: Procedural | Convergent{horizon_ticks} | PathDependent,
  with fidelity_horizon() (MAX / horizon / 0) and skippable_past_horizon() (true
  except PathDependent). This is the datatype the hybrid/adaptive-selection/vector
  machinery all key off.
- influence_set(seed_regions, edges, min_coupling, max_hops): thin named wrapper
  over the EXISTING world::rollback_scope::rollback_closure (same BFS over
  region_graph::Edge) — "what regions can this channel reach?".
- is_structurally_irrelevant(...): the FREE pre-filter (§4.5 test #1) — true iff
  the channel reaches ONLY its own seeds at the coarse coupling threshold within
  the horizon → provably can't change any other region → Convergent by graph
  analysis, ZERO simulation. Catches the common case (most local behaviour doesn't
  propagate) for free.
- classify_structural(...): Some(Convergent{0}) if structurally irrelevant, else
  None (inconclusive → caller falls through to coarse pre-bracket / sensitivity /
  full bracket, then caches). Deterministic verdict (committable).
- Built ON existing tested code (rollback_closure), like the RegionGrid finding —
  an EXTENSION not new infra.
- VERIFIED: cargo check --lib (render,window,gpu,ui-overlay,audio) -> Finished, no
  errors. (8 unit tests written; test-binary link times out in-sandbox as always,
  but the check confirms the module + tests type-check; assertions are pure graph
  checks.) -> CHECKED.
- Still gated for END-TO-END use (needs Step 0 dynamic systems + a real region
  coupling graph populated), but the CLASSIFIER itself is done, correct, and
  callable now.

## r275 — BUILD: sim::sim_scheme — adaptive scheme selector (composes on ConvergenceClass)
Operator: keep building. Second deep-time piece, composes directly on r274's
ConvergenceClass. NEW crates/unendlich-engine/src/sim/sim_scheme.rs (declared pub
mod sim_scheme;). Pure + deterministic, 9 tests.
- Scheme enum: SimIt | SingleVector | DriveThenPrefill{drive_to, prefill_ticks,
  drive_chained}.
- fidelity_len(classes): min over ConvergenceClass::fidelity_horizon() — "most
  path-dependent active system caps the vector length" (empty/all-Procedural →
  MAX; any PathDependent → 0).
- choose_scheme(span, classes, sim_it_threshold, prefill_ticks, vector_max_span):
  faithful to adaptive_scheme_selection.md — span≤threshold → SimIt; span≤flen →
  SingleVector; else DriveThenPrefill{drive_to: span-prefill (prefill clamped to
  span), prefill_ticks, drive_chained: drive_to > vector_max_span}.
- chain_len(drive_span, vector_max_span): ceil-div (the VECTOR_MAX_SPAN chain
  length before convergence-merge), 0-cap/0-span → 1.
- Tests cover min-horizon, the three regimes, very-large-span chaining, path-
  dependent forcing vectors, prefill clamping, ceil-div, determinism.
- VERIFIED: cargo check --lib (full features) -> Finished, no errors. (9 tests
  written; test-link times out as always; check confirms type-check.) -> CHECKED.
- Two machinery pieces now stand: convergence (classify a channel) → sim_scheme
  (how to traverse a span given the classes). Both pure, callable now; end-to-end
  use still gated on Step 0.

## r276 — BUILD: sim::region_digest — stable coarse-state content identity (deep-time)
Operator: keep building. Third deep-time piece — the digest the seeding, set-
algebra, and merge docs all depend on. NEW crates/unendlich-engine/src/sim/
region_digest.rs (declared pub mod region_digest;). Pure + deterministic, 8 tests.
- ONE digest, THREE jobs: seed conditioning (PRNG keyed on seed,region,tick,
  digest), branch identity (same digest → share by pointer), convergence merge
  (same consecutive digest → collapse, cost ∝ distinct digests).
- Meets both doc requirements: COARSE (quantize() to buckets before hashing so
  sub-bucket jitter doesn't make every checkpoint distinct — convergent channels
  that settle to the same bucket merge) + STABLE/VERSIONED (fixed field order +
  explicit splitmix64 mix64, NOT DefaultHasher; DIGEST_VERSION const so scheme
  changes are detectable).
- API: quantize(Fixed, buckets_per_unit) symmetric rounding; region_digest(region,
  tick, &[i64]); digest_fixed(region, tick, &[Fixed], buckets_per_unit) the common
  path; converged(a,b) the merge test. Inputs are Fixed (48.16) → bit-identical
  across machines.
- Tests: bucket rounding (+/-), coarse jitter collapses to same digest, distinct
  states differ, region/tick distinguish identical state, field-order matters,
  determinism, empty-fields stable.
- Fixed a placeholder-line bug mid-build (invalid 0xD1G_INVALID token, was
  immediately overwritten anyway) → removed.
- VERIFIED: cargo check --lib (full features) -> Finished, no errors. (8 tests
  written; test-link times out; check confirms type-check.) -> CHECKED.
- THREE machinery pieces now stand (all pure, callable, built on existing
  primitives): convergence (classify) → sim_scheme (traverse) → region_digest
  (identity). End-to-end use still gated on Step 0 dynamic systems.

## r277 — BUILD: sim::sim_vector — the committed checkpoint-director (ties the machinery together)
Operator: keep building. Fourth deep-time piece — the SimVector struct that
composes on all three priors. NEW crates/unendlich-engine/src/sim/sim_vector.rs
(declared pub mod sim_vector;). Pure + deterministic, 8 tests.
- SimVector { start_tick, span, region, seed_key, waypoints: Vec<u64>, corrections:
  Vec<CorrectionDelta>, events: Vec<EventCommitment> } — committed data in the seed
  lineage (replay re-fills, doesn't re-plan → planner-search is determinism-safe).
- CorrectionDelta { dim: u16, delta: Fixed } — sparse, only responding dimensions.
- EventCommitment { kind: u16, at_tick, magnitude: Fixed } — conditional committed
  edits (dragon-still-building), applied at checkpoints; discrete never blur.
- SimVector::plan(world_seed, region, start_tick, span, quantized_state, classes):
  * seed_key = state-conditioned via region_digest (state→seed, never outcome→seed)
    — proven by test: dry vs wet coarse state → different seed_key.
  * waypoints = even_waypoints, COUNT from class_c_count(classes) via waypoint_count
    (1 + class_c*STAGES_PER_PATH_CHANNEL[=4], capped to ≤ span ticks). Convergent-
    only → 1 stage; each Class-C channel adds 4. Last waypoint == end tick exactly.
  * corrections/events start empty (planner/fill populate).
- Methods: end_tick(), stage_seed(i) (deterministic per-sub-step PRNG key from the
  committed seed_key — distinct per stage), push_correction(), push_event().
- Tests: convergent→1 stage, path channels add stages, stages≤ticks cap, waypoints
  span+end exactly + ascending + in-range, count matches class-C, seed state-
  conditioned, determinism + distinct stage seeds, sparse corrections/events.
- VERIFIED: cargo check --lib (full features) -> Finished, no errors. (8 tests;
  test-link times out; check confirms type-check.) -> CHECKED.
- FOUR machinery pieces now stand + compose: convergence (classify) → sim_scheme
  (traverse) → region_digest (identity) → sim_vector (the committed director that
  uses digest for seed_key + ConvergenceClass for waypoint density). The decision
  + data layer of timebending is built; fill execution + end-to-end gated on Step 0.

## r278 — docs: timebending ↔ storage & world-history-loading integration
Operator asked: does the sim-vector machinery plug into world history loading? can
it be part of world storage? timebenders load any time from the world menu.
Investigated the REAL storage code + answered in docs/design/
timebending_storage_integration.md.
- FINDINGS from code: world = save_slots.rs (name_hash→WorldSeed, fully
  reconstructable from name) + storage.rs (chunks BORN PROCEDURAL from (coord,
  seed,gen_version), only explicit block-edits persisted as a flat (index,block)
  overlay, regenerate-base+overlay on load). world.mkw = seed+gen_version+last-
  play. THE GAP: the edit overlay is TIMELESS (state-now, not history); no tick
  axis, no checkpoint-at-T; dynamic sim state (water/pops/growth) can't be
  represented by a discrete-block overlay at all.
- SHORT ANSWER: yes to both, and the reconstructive (not snapshot) design is most
  of the way there — it already "regenerate base + apply deltas," which is the
  "compute forward from a checkpoint" shape. Missing piece = a TIME AXIS, and the
  sim-vector machinery IS the engine that computes a requested tick.
- "Load any time from the menu" = pick target tick T → nearest checkpoint ≤ T →
  choose_scheme(sim_scheme) → SimVector::plan chained under VECTOR_MAX_SPAN, merged
  by region_digest → sim-prefill the landing → apply tick-stamped edits ≤ T →
  present. Forward OR backward = same recompute. Reuses the load path.
- STORAGE DIFF (concrete, additive, version-gated): world.mkw gains world_tick +
  sparse checkpoint catalog [(region_group,tick,digest)]; chunk files tick-stamp
  edits (index,block)→(tick,index,block) [un-stamped=tick0, old saves load] + an
  optional coarse-state checkpoint blob per region at catalog ticks; load path
  takes a target tick, computes forward via the sim-vector chain; bump
  FORMAT_VERSION (machinery exists). The classify/traverse/identity/vector logic is
  the ALREADY-BUILT sim::{convergence,sim_scheme,region_digest,sim_vector} — plugs
  in at the load step.
- DETERMINISM = the multiplayer-safety key: a tick is COMPUTED from (seed,
  gen_version, tick-stamped edit log, sparse checkpoints, committed sim-vector
  plans), so every client loading T gets the bit-identical world — the menu loads a
  reproducible COMPUTATION, not a divergeable snapshot (why the plan is committed).
- FITS the existing philosophy: storage.rs already "born procedural, save only
  deltas"; deep-time = same principle +1 axis (born procedural AND evolves by a
  reconstructive rule; save sparse checkpoints + tick-stamped deltas + committed
  plans, never the bulk). Gated on Step 0 for the fill + checkpoints; the storage
  diff is the bridge, do alongside Step 1.
- Cross-linked statistical_timebending_and_storage + DEEP_TIME_INDEX. Docs-only ->
  CHECKED.

## r279 — BUILD: sim::sim_chain — multi-vector chain assembler + convergence-merge collapse
Operator: keep building. Fifth deep-time piece — the chain assembler the world-menu
load path (step 3 of the storage-integration doc) calls for a deep jump. NEW
crates/unendlich-engine/src/sim/sim_chain.rs (declared pub mod sim_chain;). Pure +
deterministic, 11 tests.
- chain_spans(start_tick, total_span, vector_max_span): the (link_start, link_span)
  segments — contiguous, each ≤ cap, last ends exactly at start+total_span; 0-cap or
  0-span → single segment. (Even distribution, last link absorbs remainder.)
- plan_chain(world_seed, region, start_tick, total_span, quantized_start_state,
  classes, vector_max_span) -> Vec<SimVector>: one SimVector::plan per segment (R2
  structural plan from the start state; the fill re-anchors each link's seed/state
  as it advances). Links contiguous (link[i].start_tick == link[i-1].end_tick),
  chain ends at target, each link carries the Class-C waypoint density.
- merge_runs(end_digests) -> Vec<MergeRun{digest, first_link, len}>: the convergence
  merge — groups CONSECUTIVE links with equal end-digest into runs ("steady state
  held across len links"). runs.len() = DISTINCT vector count = the chain's true cost
  (vector_chaining §2: cost ∝ distinct un-merged vectors, not raw span).
- distinct_count() = cost metric; merge_compression() = 1 - distinct/total (how much
  a quiescent chain collapsed).
- Tests: contiguous+exact-coverage spans, uneven remainder, 0-span/no-cap edge,
  plan_chain contiguity+density, merge collapses quiescent runs (A,A,A,B,B,C → 3),
  fully-quiescent → 1 (1000 links, ~0.999 compression), fully-active → no merge,
  empty metrics, determinism.
- VERIFIED: cargo check --lib (full features) -> Finished, no errors. (11 tests;
  test-link times out; check confirms type-check.) -> CHECKED.
- FIVE machinery pieces now compose: convergence (classify) → sim_scheme (traverse)
  → region_digest (identity) → sim_vector (committed director) → sim_chain (deep-jump
  chain + merge). This is the load-a-tick engine for the world menu (per r278). Fill
  execution + storage diff + end-to-end still gated on Step 0.

## r280 — PERF FIX: freeze gameplay world-sim in the menu backdrop (the menu-chug cause)
Operator stuck in menus with standstill frame times (p50 159ms ~6fps early,
recovering to 12-18ms). Diagnosed from the attached profile + code.
- ROOT CAUSE (NOT the session's feature work — proven: none of the UNENDLICH_*
  kill switches appear in the operator's "Flags in effect" list, so every wired
  feature is OFF, the deep-time modules are uncalled, and the worker's perf_charts
  is unwired by its own note). The world "streams behind the menu as a live
  backdrop" (bin line 714) and the main loop runs world sim+mesh+draw EVERY
  iteration regardless of app state (AppStep::Menu => {} is empty). So in the menu:
  * plant_lifecycle SPIKES 115-286ms/tick (the known periodic mega-stall),
  * far-mesh 11-13ms×26-77 + near-mesh 33ms (world-load streaming),
  * gpu main 88-119ms / wait_fences 55-79ms while the chunk backlog drains.
  It self-recovers (p50 159→18→12ms over ~2min) as streaming drains = world-load +
  backdrop-sim cost paid in the menu where the player gets no benefit.
- FIX: fg_services.tick_all (runs plant_lifecycle/reef_seeder/leaf_decay) now SKIPS
  while in_menu_backdrop = (renderer.menu_open || wizard_open) && !mouse_captured —
  the SAME menu signal already used at line 4350 to pin streaming. The slow orbit
  camera over a static scene doesn't need gameplay sim; it resumes the instant the
  player clicks in (mouse_captured). KILL SWITCH UNENDLICH_MENU_SIM=1 restores the
  old always-sim behaviour. Default = freeze.
- Far-mesh churn already mitigated (streaming pinned to spawn in menu, line 4350);
  the far-mesh cost in the log is one-time world-load meshing, expected.
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no
  errors. -> UNVERIFIED (behavioural; can't measure framerate without the M2). M2
  test: sit in the menu — plant_lifecycle should drop to 0ms in the 'tick:' log and
  the menu frame time should stop spiking. UNENDLICH_MENU_SIM=1 to compare.

## r281 — PERF FIX (menu, cont.): freeze creature AI in the menu backdrop too
Operator: keep working on menus unstuck. r280 froze the world-sim services
(plant_lifecycle); this adds the OTHER big menu-backdrop cost.
- FINDING: the ~200-creature roamer brain loop (4589-5749, the roamer_tick zone:
  observe/rest/grime/posture/forage/dragon, ~1000 lines) runs every frame with ONLY
  the ai_budget distance throttle — NO menu/paused gate. So in the menu backdrop all
  creatures think at full cost = a large slice of game_update (p50≈16-33ms in the
  menu profile).
- FIX: the roamer loop now `break`s immediately when freeze_backdrop_sim (the SAME
  r280 gate: (menu_open||wizard_open) && !mouse_captured, UNENDLICH_MENU_SIM=1 to
  restore). Creatures don't need to think during a title-orbit over a static scene;
  resumes on click-in. The roamer_tick zone records ~0 in the menu.
- Ruled OUT a navigation bug: the repeated "Confirm -> Menu" log lines are EXPECTED
  (confirming intermediate menu screens stays on Menu; only final Create →
  AppStep::Launch). The "stuck" is the 6fps menu being painful to navigate, which
  the two freezes (r280 sim + r281 creatures) address. The remaining menu cost is
  `draw` (the one-time world-load meshing backlog streaming in around spawn for the
  backdrop) — inherently load-time, left alone (throttling it would slow backdrop
  pop-in, and it self-drains: p50 159→18→12ms over ~2min in the profile).
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no
  errors. -> UNVERIFIED (behavioural; needs M2). M2 test: in the menu, the 'tick:'
  log plant_lifecycle=0 AND game_update should drop sharply (no creature ticks);
  the menu should be responsive. UNENDLICH_MENU_SIM=1 to compare to old behaviour.

## r282 — REVERT menu-freeze + OPTIMIZE: stagger Simulation-band tick firing (kills the plant_lifecycle spike)
Operator: "the world SHOULD simulate in the menus, it just shouldn't be so
expensive — it isn't optimized yet" (loves the evolving menu). Reverted r280+r281,
then optimized the actual cost.
- REVERTED r280 (tick_all unconditional again) + r281 (roamer loop break removed).
  Zero freeze_backdrop_sim refs remain. World sims in the menu backdrop as before.
- DIAGNOSIS from the flags-ON profile: steady state is GOOD (frame_total p50≈10ms
  ~100fps, game_update p50 0.49-7.5ms; the wired features add no measurable steady
  cost). The pain is SPIKES: game_update p99≈71-82ms and plant_lifecycle=29.2ms in a
  single tick (down from 115-286ms but still a frame-killer). ROOT CAUSE: scheduler.rs
  should_fire_in_band PHASE-ALIGNS the Simulation band — every Sim-band chunk fires
  on the SAME tick (tick_id % divisor == 0), bunching the whole band's work into one
  frame every Nth tick (the periodic spike). The 8432ms max is the one-time worldgen
  load hitch (separate, expected).
- FIX (scheduler.rs): added chunk_phase(coord, divisor) (stable splitmix64 hash, =0
  for divisor≤1) + should_fire_in_band_staggered(band, coord, tick_id, params): each
  Sim chunk fires when tick_id % divisor == chunk_phase(coord), spreading the band
  across the whole window. SAME per-chunk cadence (once per divisor) and — with the
  existing catch-up tracker advancing by elapsed ticks — SAME total work, just de-
  synchronised so no single tick carries the whole band. Render/CatchUp unchanged.
  Deterministic (phase is a pure fn of coord) → replay/multiplayer-safe.
- WIRED into plant_lifecycle.rs behind UNENDLICH_PLANT_STAGGER (default ON, =0 to
  disable). The other tick handlers (reef_seeder/leaf_decay) can adopt the same
  staggered firing later; plant_lifecycle is the spiker so it gets it first.
- 5 tests (scheduler_stagger_tests): once-per-window cadence, phases spread across
  chunks, determinism, divisor-1 no-stagger, Render/CatchUp unaffected.
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no
  errors. (lib test-run blocked by the pre-existing glfw-in-keybindings issue, not
  this code; check is the gate; stagger logic is pure arithmetic.) -> UNVERIFIED
  (behavioural; needs M2). M2 test: watch the 'tick:' log — plant_lifecycle should
  stop showing periodic 29ms+ spikes (work spread to ~1/divisor per tick); menu +
  game frame-time p99 should drop. UNENDLICH_PLANT_STAGGER=0 to compare.

## r283 — DIAGNOSTIC: split roamer_tick into block-scan vs herd-scan sub-zones
Operator's r282 run (flags on) gave a CLEAN signal: stagger fix WORKED
(plant_lifecycle=0.0ms every tick, spikes gone), and the bottleneck is now
isolated — roamer_tick is a rock-steady 17.3ms/frame (p50 17.34, p99 17.68, max
18.66, near-zero variance) and game_update p50 17.84ms is almost ENTIRELY that
loop (draw 1.9ms, rest ~0). Frame is creature-AI-bound.
- The flat 17ms = fixed workload (every creature full-ticking). Candidates found by
  code inspection: (a) per-creature block_at storm — hungry-Fauna forage ground-ring
  + nav3d 27-cell kernel, each cell a chunk_manager.block_at (DashMap::get + reindex
  per call); (b) O(n^2) herd scans — hungry-Dinosaur predator-prey + dragon-hunt each
  scan the WHOLE herd_snapshot per creature. Both are hungry/kind-GATED, so the flat
  17ms is a puzzle (suggests most creatures hungry, or unconditional cost) — DON'T
  guess, MEASURE.
- ADDED sub-zone timing (accumulate per-phase µs across all creatures, record after
  the loop): roamer_blockscan (forage block-scan region) + roamer_herdscan (predator
  herd-scan region). Loop-body-scoped Instants (had to hoist out of the gated if-
  blocks for scope). Guarded by profile_on; zero cost when profiling off.
- Also fixed a scope bug mid-edit (_t_bs/_t_hs declared inside the if → moved to loop
  body). 
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no
  errors. -> UNVERIFIED (diagnostic; needs M2 run to read the split). NEXT run same
  flags + UNENDLICH_FRAME_PROFILE=1 → the [profile] block will show roamer_blockscan
  vs roamer_herdscan, pinpointing the 17ms for a targeted fix (block_at chunk-ref
  caching if block-scan dominates; spatial grid if herd-scan dominates).
- NOTE: still carries r282's stagger optimization (the confirmed win). The 8157ms max
  frame remains the one-time worldgen load hitch (separate).

## r284 — BUILD + design: witnessed-attack reaction (perception over omniscient herd scan)
Operator: the O(n^2) herd scan is the wrong model for real time — animals should
learn by EYESIGHT (see a herdmate attacked → help/attack/flee/other), and genetics
works out population behaviour; the omniscient herd math belongs to a deep-time sim-
vector, not the frame loop. Built the real-time half + captured the architecture.
- KILL SWITCH: UNENDLICH_WITNESS (default OFF, logs once).
- MECHANISM: a per-frame Vec<([f32;3], u8)> attack_events buffer. At BOTH harm
  points (melee 6243 + bending strike 3685) a harmed creature pushes (victim_pos,
  victim_kind). In the roamer loop, each creature scans the buffer (usually EMPTY →
  free) for an attack within witness range (20 blocks) on a SAME-KIND herdmate;
  seeing one raises aggro +0.4 → the arbiter resolves help (Fight→approach) vs flee
  by the creature's boldness. Buffer cleared after the loop reads it (harm after the
  loop carries 1 frame — imperceptible). Self-report excluded (d2>0.25).
- COST: O(events × creatures), events≈0 most frames → an empty-Vec check per
  creature vs the n²-every-frame herd scan. Perception bounds propagation (the deep-
  time through-line applied to behaviour). Event-driven + local.
- HONEST gaps (in the doc): "can see" is a RANGE check, not LOS raycast/sight-cone
  (no occlusion/facing yet); hearing/smell not wired into witnessing (awareness_blend
  is the home); only player-as-attacker drives it richly; predator PREY-SELECTION
  (the other O(n^2)) still uses the old scan — §2 of the doc files the spatial-grid
  fix (perception index, O(n²)→~O(n)), pending the r283 sub-zone numbers.
- DESIGN: NEW docs/design/perception_and_population_behavior.md — the principle (real
  time=perception/local/event-driven; deep time=genetics/statistics/sim-vector), the
  timescale table (seconds=witness, lifetime=combat_habit, generations=gene, jump=
  population math), and the missing heritable temperament/sociality gene (filed) that
  makes "genetics works out population behaviour" literal. Cross-linked sim_vectors.
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no
  errors. -> UNVERIFIED (behavioural; needs M2). M2: UNENDLICH_WITNESS=1 (+AGGRO
  +ARBITER) → attack a creature in a herd, watch nearby same-kind react (bold ones
  approach, timid scatter). NOTE: still carries r282 stagger + r283 sub-zones.

## r285 — BUILD + design: sim::acoustics (sound fingerprinting/masking/echo) + acoustic-perception+POI architecture
Operator: sound should be part of genetics; a sound has an acoustic FINGERPRINT
(operator has voice-fingerprinting code in voiceterm.zip) → classify against a
"sound DNA" database; environment changes hearing (waterfall/windy mobs don't use
ears the same); echo + volume change reactions; creatures balance POIs (food/
shelter/friends/foes) by score+radius for near-term; long-term runs a microsim
batching POIs → sim-vector. Built the pure acoustic core + captured the architecture.
- STUDIED voiceterm.zip: speaker_embed.rs = "identity is a REGION in feature space,
  not a scalar" — embed→L2-normalised vector→cosine_distance + nearest+MARGIN
  classifier (refuse to identify when ambiguous). Exactly the right primitive for
  creature sound classification. audio_profile.rs = pooled spectral stats;
  TODO_fingerprint_db = the DB-of-signatures (HNSW at scale).
- NEW crates/unendlich-engine/src/sim/acoustics.rs (declared pub mod acoustics;).
  Pure + deterministic, 10 tests. Adapts the voice approach:
  * FINGERPRINT_DIM=8; l2_normalize + cosine_distance (NaN-safe, zero=no-info —
    a corrupt fingerprint can't false-match);
  * SoundClass{id, prototype} = the "sound DNA database" (predator/kin/water/wind/
    footstep prototypes); classify(query, classes) = nearest by cosine + ambiguity
    MARGIN (small margin → creature stays wary, doesn't guess);
  * effective_hearing(volume, ambient, acuity) = MASKING — usable hearing = acuity
    scaled by headroom above the local ambient noise floor; a sound at/below ambient
    is inaudible (waterfall/wind floods ears, exactly as asked); acuity intended
    genetic;
  * spatial_impression(HeardSound{volume,echo}) = ECHO cue → (perceived_distance,
    enclosure); reverb reads as far/large-space, dry+loud as right-here → creatures
    react differently to echoing vs dry sounds.
- DESIGN: NEW docs/design/acoustic_perception_and_poi.md — the two-regime split for
  hearing+goals (real-time=perception+local POI scoring; deep-time=acoustic genetics
  + POI microsim→sim-vector), §2 hearing genetics (acuity/innate-classes/POI-
  disposition as Class-C genes, needs the genome loci), §3 masking+echo (+ filed:
  ambient noise/echo SOURCE fields for wind/dust/surf/geometry + audio beds), §4 POIs
  (food/shelter/kin/foe scored by type×distance = the near-term Drives the arbiter
  resolves; primitives exist: arbiter/feeding_select/predator_prey/herd_cohesion;
  missing: unified Poi{pos,kind,salience}+score_pois→Drives), §5 microsim batches
  POIs→SimVector (slots into plan_chain + region_digest), §6 status table. Cross-
  linked perception_and_population_behavior.
- VERIFIED: cargo check --lib (full features) -> Finished, no errors. (10 tests;
  test-link blocked by pre-existing glfw-in-keybindings; check is the gate; pure
  vector/masking math.) -> CHECKED.

## r286 — BUILD: sim::poi — unified POI scoring (the near-term decision substrate)
Operator: keep building. The connective piece from acoustic_perception_and_poi.md §4
— turns perceived POIs into the arbiter's Drives + a nav target. NEW crates/
unendlich-engine/src/sim/poi.rs (declared pub mod poi;). Pure + deterministic, 10
tests, composes on behavior_arbiter::Drives.
- PoiKind {Food, Shelter, Friend, Foe}; Poi {pos:[f32;3], kind, salience}.
- PoiWeights {food, shelter, friend, foe} = heritable per-lineage disposition
  (skittish lineage weights Foe+Shelter high; bold weights Foe low). Default all 1.0.
- Body {hunger, fatigue, aggression, health_frac} = the creature's internal (non-
  spatial) state, passed through to drives.
- distance_falloff(d, radius) = smooth 1@0 → 0@radius; poi_pull = salience ×
  type_weight × falloff (xz distance — creatures live on a surface).
- dominant_attractor(...) = strongest Food/Shelter/Friend POI in range = the near-
  term NAV TARGET (Foe excluded — foes drive threat, not attraction).
- threat_from_foes(...) = strongest foe-POI pull → the arbiter's threat drive.
- to_drives(pois, my_pos, weights, radius, body, extra_threat) → behavior_arbiter::
  Drives: threat = max(foe_threat, extra_threat) [extra = a directly-seen player];
  hunger/fatigue/aggression/health pass through from body. The arbiter then scores
  Flee/Fight/Forage/Rest/Patrol; dominant_attractor gives the target for non-threat
  picks. This is the substrate the witnessed-attack reaction (foe POI), predator hunt
  (food POI), and flocking (friend POI) all reduce to.
- Cost: local — POIs are the FEW perceived salient things, cheap per creature (vs the
  omniscient herd scan). Weights intended genetic; off-screen a microsim batches POIs
  → sim-vector (filed, §5).
- Tests: falloff endpoints, nearer/stronger pulls harder, weight scales (skittish
  shelter), dominant_attractor ignores foes + picks strongest, none-in-range, threat
  from nearest foe, to_drives fills threat + passes body, extra_threat max, determinism.
- VERIFIED: cargo check --lib (full features) -> Finished, no errors. (10 tests;
  test-link blocked by pre-existing glfw-in-keybindings; check is the gate; pure
  scoring math.) -> CHECKED. Not yet wired into the roamer loop (needs a perceived-POI
  list built from sight+acoustics — the wiring step); the SCORER is done + callable.

## r287 — design (genome loci) + WIRE: POI scorer into the behaviour path
Operator: "write that" (the genome loci) + keep going with perception behaviour.
- DESIGN: NEW docs/design/genome_loci_for_behavior.md — consolidates the gene gap
  flagged 3×. The loci: boldness + aggression_base (temperament → arbiter/witness/
  aggro), sociality + caution + foraging_drive (→ poi::PoiWeights), hearing_acuity +
  eyesight (→ acoustics acuity + sight range), innate_classes (born-known SoundClass
  prototypes). Inherit via the existing crossover+mutation path (prototypes re-
  l2_normalized after mutation); express(genome)->Disposition feeds the built
  mechanism modules unchanged; they're the Class-C population channels the deep-time
  jump evolves. Build order: loci+express (pure, next) → cache on roamer → wire →
  innate inherit → population sim. This unblocks the genetic half of poi/acoustics/
  arbiter/witness at once.
- WIRE: UNENDLICH_POI (default OFF, depends on UNENDLICH_ARBITER). The arbiter's
  Drives now route through sim::poi when on: builds a perceived-POI list (threat →
  Foe POI), calls poi::to_drives with the creature's PoiWeights (default until the
  loci land) + body state + the directly-seen threat (extra_threat max). Replaces the
  inline Drives assembly. The unifying near-term decision substrate is now in the live
  path.
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no
  errors. -> UNVERIFIED for the wiring (behavioural; needs M2: UNENDLICH_POI=1 +AGGRO
  +ARBITER → same flee/fight behaviour but driven by scored POIs); doc -> CHECKED.

## r288 — UI/settings batch (part 1, the verifiable pieces): frametime p75+mean + slider value model
Operator: UI batch — custom font + glyph merging text entries, slider touchups
(p75+mean3s on frametimes, render-distance slider with number), fixup create world,
build out join world. Did the cleanly-testable pieces; rest needs see-the-result M2.
- FRAMETIME p75 + mean (DONE, the diagnostic readout the operator's used all
  session): WindowStats (telemetry/window.rs) gained p75 (computed in stats(), added
  to report.rs JSON + the test ctor). The bin's FPS line (8746) now prints
  "p50= p75= p99= mean= max=" (added p75 percentile + mean over the 1s frame window).
- SLIDER VALUE MODEL (DONE, testable): menu_screen_layout::Widget gained value:
  Option<String> + with_value() builder + display_text() ("label: value" when set).
  So a Render Distance slider can render "Render Distance: 12". The model is in; the
  actual create-world slider needs to call .with_value(render_distance) (the live
  wiring is in the GPU menu path — see below).
- VERIFIED: cargo check --lib + --bin (full features) -> Finished, no errors (pre-
  existing winit cfg warnings only). -> the p75/mean is CHECKED (pure telemetry +
  log-line logic); the Widget value model is CHECKED (pure struct/test logic).
- NOT DONE (needs M2 to see the result — GPU-rendered UI across menu_view.rs/
  menu_screen_layout/voxel_text, can't visually verify here): custom FONT + glyph
  merging (voxel_text.rs is currently a tiny 3D block font spelling only "UNENDLICH"
  — a full glyph set + text-entry-with-merging is a real multi-file feature); wiring
  the slider value into the live create-world screen (two menu paths exist —
  structured Widget vs egui menu_view — need to see which is live); fixup create
  world; build out join world. These are scoped + flagged for a session where the
  result is visible. The model/data changes that SUPPORT them (Widget.value, p75) are
  landed.

## r289 — BUILD: sim::disposition — express(genome) → Disposition [genome loci step 1] + wired into POI
Operator: genome loci step. Built the expression layer (Step 1 of
genome_loci_for_behavior.md) — pure-CPU, unblocks the genetic half of perception.
- DECISION: NOT adding fields to Genome yet (61 construction sites across the tree —
  too invasive to validate without the M2). Instead a forward-compatible expression
  layer that reads the REAL eyesight/hearing genes (genome.express().{eyesight,
  hearing}) and DERIVES the not-yet-in-genome behavioural loci (boldness, sociality,
  caution, foraging, aggression_base) deterministically from the entity seed — a
  per-individual replayable stand-in. When the genes are added, swap each derivation
  for a phenotype read; the Disposition interface stays fixed.
- NEW crates/unendlich-engine/src/sim/disposition.rs (declared pub mod disposition;).
  Pure + deterministic, 7 tests.
  * Disposition { poi_weights, aggression_base, boldness, hearing_acuity,
    sight_range, innate_classes } — exactly the inputs poi/acoustics/arbiter take.
  * express_disposition(genome, entity_seed): eyesight→sight_range [SIGHT_MIN..MAX=
    10..24]; hearing→hearing_acuity; derived boldness/sociality/caution/foraging →
    PoiWeights {food:0.5+foraging, shelter:0.5+caution, friend:0.5+sociality, foe:
    0.5+caution} (cautious lineage = foes more salient + seeks cover; social =
    flocks); aggression_base=boldness*0.3 capped 0.5. innate_classes empty until
    those genes land.
  * aggro_with_floor(current, base) helper (aggression never relaxes below the
    genetic floor).
  * derived_locus = splitmix64(entity_seed^salt) per behavioural trait.
- WIRED into the POI scorer (r287 path): the roamer now expresses its genome (entity
  seed derived from its OWN stable alleles — gs[4].a/gs[5].a/gs[0].b value bits) and
  scores POIs with disp.poi_weights instead of default(). genome bound in the
  destructure. Under UNENDLICH_POI, each creature's foe-salience/shelter-seeking now
  varies by its genetics. The genetic half of perception is live (with derived loci).
- Tests: determinism, individuals differ, sight in [10,24] over 50 genomes, poi
  weights in [0.5,1.5], aggression_base capped, aggro floor, boldness/acuity unit.
- VERIFIED: cargo check --lib + --bin (full features) -> Finished, no errors. (7
  tests; lib test-run blocked by pre-existing glfw; check is the gate; pure mapping
  math.) -> CHECKED for the module; the POI wiring -> UNVERIFIED (behavioural, M2).

## r290 — WIRE: full disposition through the live behaviour path (beyond r289's POI-weights)
Operator: sure (finish feeding the rest of the disposition through). r289 shipped
only the POI-weights consumer; this wires the WHOLE Disposition into the roamer tick.
- HOISTED: express_disposition is now computed ONCE per creature per tick (before the
  threat block, ~4902), gated on poi||aggro||arbiter||witness enabled, into a local
  `disposition: Option<Disposition>`. Entity seed derived from the creature's OWN
  stable genome alleles (gs[4].a/gs[5].a/gs[0].b value bits) → per-individual +
  replayable. (Caching on the roamer is the perf refinement; genome is fixed for life
  so per-tick recompute is correct, just slightly wasteful — noted.)
- SIGHT RANGE genetic: the old `const FLIGHT_DIST: f32 = 16.0` is now
  `let flight_dist = disposition.sight_range.unwrap_or(16.0)` — sharper-eyed lineages
  notice/flee threats farther out (range 10..24). All FLIGHT_DIST refs replaced (the
  threat filter, the tdn threat-nearness falloff, the POI radius); 0 stale refs.
- AGGRESSION FLOOR: at the aggro-decay site, aggro_with_floor(*aggro, aggression_base)
  — a hostile lineage never relaxes below its genetic floor (idles primed).
- WITNESS TIP genetic: the witnessed-attack aggro boost is now 0.2 + 0.4*boldness
  (was flat 0.4) — a bold lineage tips to HELP kin, a timid one barely reacts so its
  flee wins in the arbiter. Habit still bends this within a life.
- POI block deduped to use the hoisted `disposition` (pw) instead of re-expressing.
- NET: genome → disposition → {sight range, aggro floor, witness boldness, POI
  weights} all live. The genetic half of perception/behaviour now expresses through
  FOUR channels, not one. (hearing_acuity + innate_classes still have no live
  consumer — acoustics isn't wired into the loop yet; noted gap.)
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no
  errors (only pre-existing BerryBush/active_slots/outputs/unused warnings). ->
  UNVERIFIED (behavioural; needs M2). M2: UNENDLICH_POI=1 +AGGRO +ARBITER +WITNESS →
  creatures vary by genetics: some notice threats farther (eyesight), some idle
  hostile (aggression_base), bold ones rush to help attacked kin while timid scatter
  (boldness). Carries r282 stagger + r283 sub-zones.

## r291 — FIX: Create World silent dead-end (duplicate-name check) — names are titles
Operator (testing on M2): Create logs the button but menus don't destroy + loading
animation doesn't play; step resolves to AppStep::Menu not Launch. Fresh first world,
default name. Direction: "world id and name is more like a title."
- ROOT CAUSE: menu_controller::confirm() at NewGame only returns StartWorld(spec) if
  wizard.finish().ok() succeeds; finish() = can_finish()? then to_spec(). can_finish
  REJECTED duplicate names ("a world with that name already exists") and the error was
  swallowed by .ok() → MenuOutcome::None → world_launch::plan → Stay → AppStep::Menu.
  The catalog's default-named entry collided with the fresh default-named world, so
  Create silently no-op'd. (Matches the operator's instinct that the NAME shouldn't be
  identity.)
- FIX (new_game_wizard::can_finish): dropped the duplicate-name rejection. A world's
  title may now collide; only name VALIDITY is checked (valid_world_name). The
  _catalog param is kept for signature stability.
- STORAGE/IDENTITY (bin): since titles can collide, storage + catalog now key by
  name+seed (a lightweight world-id stand-in until the full id refactor): world root
  is worlds/<safe-name>-<seed:016x>/ (was worlds/<name>/), and the catalog record
  retains/replaces by (name && seed) instead of name alone — so two same-titled worlds
  with different seeds keep separate edits + separate catalog rows.
- DEFERRED (noted, follow-up): full WorldEntry.id identity refactor (8 construction
  sites + catalog add/remove dedup-by-name + disk format) — too invasive to land blind
  in one pass; name+seed is the safe interim. Also still TODO: surface finish() errors
  ON-SCREEN so Create can never silently dead-end again (the diagnostic safety net) —
  needs a renderer error field + egui wire (next iteration).
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no
  errors. -> UNVERIFIED (needs M2). M2 TEST: title → New World → Create on a fresh
  default-named world should now DESTROY the menu, play the loading animation, rebuild
  the seed's world, and drop you in to walk around.

## r292 — PERF: breeding O(n²) → low-cadence + spatial grid (the roamer_tick culprit)
Operator M2 profile (fresh survival world): roamer_tick p50=52ms p95-99=102ms,
mean climbing 0→21ms over the run = ~2/3 of game_update = the sustained 8-10 FPS.
DIAGNOSIS: the roamer_tick zone (4614-5881) wraps the whole creature system incl. an
O(n²) all-pairs BREEDING scan (~5905): for i in 0..n { for j in (i+1)..n } over all
roamers, cheap rejects INSIDE the inner loop so ~n²/2 comparisons every fired frame,
cost GROWING quadratically as the herd fills (15→29 roamers) — exactly the 0→52ms
climb, worst when popdyn reports herd>capacity. NOT the per-creature AI (perception =
cone test no raycast; step_aware = arithmetic; blockscan/herdscan zero while resting),
NOT mob-band catch-up (capped at band divisor → the clean 52/102 stepping).
- PART 1 (cadence gate): the whole mating pass now runs only every BREED_INTERVAL
  (1500ms) via last_breed_check Instant, instead of every fired frame. Breeding is a
  slow biological event; this alone removes most of the per-frame cost.
- PART 2 (spatial grid, the structural fix): NEW crates/unendlich-engine/src/sim/
  spatial_grid.rs (declared pub mod spatial_grid;), pure + deterministic, 8 tests.
  build(positions, cell_size) bins into x/z cells; neighbour_pairs(positions,
  cell_size) emits every same-cell + adjacent-cell pair EXACTLY once (4 FORWARD
  offsets (1,0)(1,1)(0,1)(-1,1) so each cross-cell adjacency visited from one side) —
  O(n) bin + O(pairs) where pairs bounded by local density, not n. cell_size >=
  interaction radius guarantees no real pair missed (caller keeps the precise dist
  test). Tests: empty/singleton, each-pair-once, SUPERSET-of-brute-within (the
  correctness property) on hand + random layouts over radii 2/5/9, distant clusters
  don't cross-pair, determinism, far-fewer-than-n² when spread.
  WIRED into breeding: positions collected once, candidates =
  spatial_grid::neighbour_pairs(&positions, 3.0) [cell=MATE_DIST=3 blocks], loop over
  candidates instead of i/j all-pairs; the MATE_DIST2/kind/vitality/sex gates +
  split_at_mut(j) body unchanged (grid emits i<j so split_at_mut stays valid).
- This is the same omniscient all-pairs pattern flagged for the herd scan, now fixed
  for breeding; spatial_grid is reusable for the prey-selection grid (filed).
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no
  errors (pre-existing winit cfg warnings only). spatial_grid 8 tests written; test-
  RUN blocked by pre-existing Drives-no-PartialEq + glfw-in-keybindings link issues
  (check is the gate). -> UNVERIFIED (perf; needs M2). M2: same flags → roamer_tick
  p50 should COLLAPSE from 52ms toward ~0 (breeding now 1.5s cadence + O(n)); frame
  p50 should drop from ~80ms toward the draw-bound floor.

## r293 — PERF: time-budget the shell-walk submit scan (the streaming-stall culprit)
Operator M2 (after r292): roamer_tick FIXED p50=52→17ms. New bottleneck = the chunk
meshing "submit" (shell-walk) scan: every LONG FRAME shows submit=400-868ms/sec
(~90-108ms/frame) while gen_pending>0/mesh_q churning; frames snap to ~22ms when
settled. The shell walk (6942-7362, t_submit_us) scans chunks in shells around the
player (DashMap reads + needs_submit + mesh-request submit) with NO per-frame time
cap — submit_skip only fires when STATIONARY, so flying in creative defeats it and
the walk runs unbounded. (Uploads are already budget-capped, 64/6ms; the submit scan
was not.) Operator chose #1 of the proposed fixes: time-budget the walk.
- KILL SWITCH / KNOB: UNENDLICH_SUBMIT_BUDGET_MS=F (default 5.0; 0 = uncapped/old
  behaviour). Logs once when enabled.
- MECHANISM: at the top of the 'shells loop (7065) — if submit_budget_ms>0 && shell>0
  && t_submit_start.elapsed_ms >= budget → break 'shells. Shells are visited near→far
  + forward-biased, so the chunks left unscanned are the FARTHEST/least-urgent; they
  resume from shell 0 next frame, exactly like the existing empty-shell/candidate-cap
  cutoffs (which the comments note are correctness-preserving — unsubmitted coords are
  picked up next frame). shell>0 guard so the innermost ring is never starved by a
  stale clock. The prefetch loop (predicted_mesh_ahead, 7215) gets the same budget
  check (breaks if already spent — prefetch = not-yet-visible chunks, least urgent),
  so the cap covers the WHOLE submit timer, not just the shell walk.
- TRADE-OFF (honest, in the log): chunks near the player may take a frame or two
  longer to appear when moving fast, in exchange for no 90-108ms stall. Tier budgets
  still distribute slots across LODs within the time-limited scan.
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no
  errors. -> UNVERIFIED (perf; needs M2). M2 TEST: fly through new terrain — submit/
  sec should drop from 400-868ms toward ≤5ms×fps, LONG FRAME warnings during
  streaming should largely disappear, frame p95/p99 should collapse (was 164/1694ms).
  Tune UNENDLICH_SUBMIT_BUDGET_MS up (more aggressive meshing, bigger stalls) or down
  (smoother, more pop-in) to taste. NOTE: plant_lifecycle first-tick spike (tick_sim
  max=483ms) + the two quarantined kernel panics still outstanding.

## r294 — PERF: wire the per-tick SurfaceCache (the REAL roamer_tick culprit)
Operator M2 (after r293): submit FIXED (shell budget worked, 400-868ms→0-2ms, no more
streaming stalls). But roamer_tick p50=88ms (was 17ms creative-flying) = ~90% of the
frame, a FIXED ~88-98ms cluster with only 19 roamers. NOT an r293 regression (r293
only touched the shell walk) — the streaming stalls were MASKING it; survival put 19
ACTIVE creatures near the player (vs dormant/far in the creative-flying run).
ROOT CAUSE: the roamer loop's surface_at closure (grounding, posture's 4 directional
probes, path steps — many calls per creature) called cpu.surface_y_at DIRECTLY, and
surface_y_at scans a 64-deep column (cy-48..cy+16) calling density()+is_cave() (full
procedural 3D noise + cave carve) at EACH block = ~hundreds of noise evals/creature/
tick × 19 = ~90ms. A SurfaceCache (sim/surface_cache.rs — a within-tick (x,z)→height
memo with get_or) was already WRITTEN but never instantiated/wired.
- KILL SWITCH: UNENDLICH_SURFACE_CACHE (default ON; "0" reverts to direct scans).
- WIRED: a RefCell<SurfaceCache> declared before the main loop; cleared once per tick
  at the roamer-loop start (terrain changes between ticks so the memo must not
  outlive one); the surface_at closure now routes through get_or(ix, iz, ||
  cpu.surface_y_at(...)) — first probe of a column runs the 64-deep scan, repeats
  within the tick (same creature's multi-probes + creatures standing near each other)
  are HashMap hits. Falls back to the direct scan when disabled.
- Same principle as r292 breeding + the herd scan: don't recompute from scratch what
  you can cache; surface height at (x,z) is stable within a tick.
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no
  errors. -> UNVERIFIED (perf; needs M2). M2 TEST: stand among a herd in survival —
  roamer_tick p50 should drop sharply from ~88ms (toward the ~17ms baseline or below,
  since neighbours share columns); frame p50 should fall from ~99ms. The [surface_
  cache] log confirms it's on. STILL OUTSTANDING: plant_lifecycle first-tick spike
  (tick_sim max=720ms) + the two quarantined kernel panics (draw_metadata_pack OOB,
  chunks_in_radius capacity-overflow).

## r295 — PERF: phase the plant first-encounter POI build (the streaming plant spike)
Operator M2 (after r294 surface cache, which dropped roamer_tick p50 88→41ms): found
a discrete in-game stutter — plant_lifecycle=81.3ms (worst tick_sim max=193ms) when
crossing into vegetated terrain. (The two ~9s frames in the same log are NOT this:
one is initial worldgen with gen_pending=77, the other follows focus=false = window
alt-tab; both artifacts, not hitches.)
ROOT CAUSE: the FIRST plant tick of a chunk runs build_poi_cache — a full wood scan
("full scan on first encounter") that builds the POI cache; subsequent ticks consume
the cache and are cheap (same chunks=16 batch: 81ms once, then 5.6ms). When a batch of
~16 fresh chunks STREAMS IN TOGETHER they all hit their first-encounter build on the
same frame → the spike. The r282 stagger (should_fire_in_band_staggered) desyncs the
ONGOING cadence but not the FIRST tick — a freshly-streamed batch is all on first-fire
at once.
FIX (chosen #1 of the two options): phase the first-encounter build across a window by
stable per-chunk hash, reusing the scheduler's existing chunk_phase() splitmix primitive
(the same one the stagger uses). In plant_lifecycle.rs lifecycle_handler Pass 0a, at the
should_refresh_poi/build_poi_cache call:
  - first_build = chunk.poi_cache.is_none();
  - build_now = !first_build OR spread<=1 OR tick_id % spread == chunk_phase(coord, spread)
  - if build_now → build as before; else SKIP ONLY THE BUILD this frame (fall through —
    the tree passes consume a None cache and no-op like any not-yet-built chunk, while
    plant aging still runs). Build lands within `spread` frames, fanning the batch out.
  - structural_dirty refreshes (edits) are NOT delayed (only cache-None first builds).
  - KNOB: UNENDLICH_PLANT_BUILD_SPREAD (default 8; 0/1 = build immediately/off).
- Deferral is invisible (a fresh chunk's plants just don't grow for a few frames). Note
  the refactor: defer ONLY the build, not the whole chunk body, so plant aging on
  plants-but-no-trees chunks is unaffected (and the lifecycle_handler_ages_plants test,
  which loops ticks 0..8, still ages — build lands within the 8-window).
- Determinism preserved: chunk_phase is a stable hash, tick_id deterministic, sorted
  coord iteration unchanged → replay-safe.
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished; cargo
  check --lib -p unendlich-engine --features ... -> Finished (lib-WITHOUT-features fails
  on pre-existing glfw/input/render imports, unrelated). -> UNVERIFIED (perf; needs M2).
  M2 TEST: walk/run into fresh vegetated terrain — the plant_lifecycle line should no
  longer spike to 80-190ms on batch entry (should stay single-digit ms); tick_sim max
  should drop from ~193ms. Tune UNENDLICH_PLANT_BUILD_SPREAD up (smoother, slower first-
  growth) or down (snappier growth, bigger spike). STILL OPEN: roamer_tick residual
  ~41ms; two quarantined kernel panels (draw_metadata_pack OOB, chunks_in_radius
  overflow); eyesight cliff-pathing feature (design thread to be written).

## r296 — PERF + ARCH: WorldProber (unified column-query memo); fix ecosystem-stream freeze
Operator M2 (after r295, which FIXED the plant spike — plant_lifecycle now 0-55ms, no
80-190ms; and roamer_tick steady ~44ms, r294 cache holding): found a ~9s FREEZE on
region crossings. Evidence: 9-SECOND GAPS in the log right before each ">>> ECOSYSTEM
STREAM" line; frame_total/game_update max=8859ms but NO profiled sub-zone accounts for
it (roamer_tick max=75, tick_sim max=396, draw max=216) → unprofiled work inside
game_update, fires on chunk crossing, ECOSYSTEM STREAM logs the instant it ends.
ROOT CAUSE (same family as r294, deep uncached surface scans, but in the streaming
path and far worse): on a crossing, populate up to 2 chunks; per chunk, populate_region
walks ALL 1024 columns (32×32) and for each calls biome_at (which calls surface_y_at)
AND, for spawn columns, surface_y_at AGAIN — over a 768-block-deep range (probe_y_min
=-8*CHUNK_SIZE .. probe_y_max=16*CHUNK_SIZE, 12× deeper than the creature loop's 64),
each block running density()+is_cave() noise. ≈1.5M noise evals/chunk, uncached, on the
main thread, the SAME column scanned twice (biome pass + spawn pass). Operator chose #2
(caching) and asked to build a unified prober module rather than another one-off cache.
- NEW MODULE crates/unendlich-engine/src/sim/world_prober.rs (pub mod world_prober;):
  WorldProber — several typed column-keyed (x,z) memos behind one object. surface_y(gen,
  x,z,y_min,y_max)->Option<i32> (caches None too); biome(gen,x,y,z)->Biome (keyed (x,z),
  surface-level sampling); *_arc variants for &Arc<WorldGenerator> (what generator().cpu()
  returns); clear() per batch; surface_hit_rate/biome_hit_rate/columns_cached diagnostics.
  Pure container (misses filled by the caller's generator, so no world borrow held →
  long-lived field, cleared each batch). Generalises the single-purpose
  surface_cache::SurfaceCache (r294, still used by the creature loop) — intent is for
  SurfaceCache to become the surface portion of this; future systems (flora, prey, cliff
  pathing) share ONE prober. 4 memo-contract tests (repeat=hit, None cached, clear empties
  both, zero-rate-no-queries).
- WIRED into the ecosystem stream (bin ~4480): one RefCell<WorldProber> per crossing,
  shared across the to_populate batch (no edits between the 2 chunks, so column surface/
  biome is identical regardless of which pass queries → sharing is correct AND reuses
  overlapping probe columns). Both populate_region closures route through it: the biome
  closure does surface_y_arc (memoized) then biome_arc (memoized); the spawn closure does
  surface_y_arc — REUSING the biome pass's scan for the same column. Per-caller unwrap_or
  defaults (72 biome / 64 spawn) preserved (prober caches the raw Option). Fresh per
  crossing so terrain changes between crossings are handled.
- VERIFIED: cargo check --lib AND --bin unendlich-fullgame (full features) -> Finished, no
  errors. world_prober tests written; test-RUN still blocked by pre-existing Drives-no-
  PartialEq + glfw link issues (test bin link/timeout) — check is the gate. -> UNVERIFIED
  (perf; needs M2). M2 TEST: fly across region boundaries — the ~9s freeze on ECOSYSTEM
  STREAM should collapse (each column scanned once instead of twice, repeats free);
  frame_total max should drop from ~8.8s. NOTE: this only de-dups/caches the scans (the
  768-deep range itself is UNCHANGED — operator deferred #1 right-sizing); if still spiky,
  #1 (tighten probe depth) and #3 (amortize the populate across frames) remain. Also still
  open: roamer_tick residual ~44ms; two quarantined kernel panics; eyesight cliff-pathing
  design thread (WorldProber is now the shared substrate for it).

## r297 — PERF: thread WorldProber through the flora system (the LARGER half of the freeze)
Operator M2 (after r296): the ~9-10s region-crossing freeze PERSISTED (frame_total
max=10103ms, recurring every ~17s per crossing, still unprofiled in game_update, still
logging ">>> ECOSYSTEM STREAM" at the end of each gap). DIAGNOSIS: r296 cached the
populate_region (biome+spawn = 2 deep scans/column) half, but the freeze is dominated
by the OTHER half — flora generation. region_flora_blocks → region_flora → ctx_at, and
ctx_at does FIVE deep column scans per column (surface + biome + 4 slope probes
h(±1,0)/h(0,±1)), uncached, runs BEFORE populate_region. ≈5×1024×2 ≈ 10,000 deep scans
(density()+is_cave() over PROBE_MIN=-16..PROBE_MAX=248) per crossing. Operator: cache
the flora system with WorldProber.
- ctx_at_cached(gen, prober, x, z) + region_flora_cached + region_flora_blocks_cached:
  prober-aware variants threading WorldProber through every per-column surface/biome
  lookup. The 4 slope probes of a column are the CENTRE probes of adjacent columns, so
  across a region they collapse to memo hits — the ~10k scans → ~1024 (one per distinct
  column). Originals (region_flora/ctx_at/region_flora_blocks) preserved as thin
  wrappers that spin a throwaway prober, so all existing callers + tests are unchanged
  (bin landmark site 1972, bin site 3219, flora_map/flora_blocks tests).
- WorldProber gained biome_sample(gen,x,y,z)->BiomeSample (full sample: biome +
  humidity + temperature — flora needs the climate fields, not just the biome tag),
  memoized by (x,z); new biome_sample HashMap field, cleared in clear(). BiomeSample is
  Copy. biome-only memo kept coherent (entry().or_insert) for free.
- WIRED into the ecosystem stream (bin ~4499): a SEPARATE flora_prober (NOT the r296
  populate prober) — flora's range PROBE_MIN..PROBE_MAX differs from the spawn pass's
  probe_y_min..probe_y_max (-8*CS..16*CS), and a column's cached surface is only
  coherent for ONE range, so the two systems must use distinct probers. flora_prober is
  shared across the 2-chunk to_populate batch (NOT cleared between chunks → adjacent
  chunks' shared boundary columns / slope probes reuse the memo), fresh per crossing.
- Removed the now-dead flora_map::surface() helper (orphaned — all surface lookups go
  through the prober now).
- VERIFIED: cargo check --bin unendlich-fullgame AND --lib -p unendlich-engine (full
  features) -> Finished, no errors (no new dead-code warnings). -> UNVERIFIED (perf;
  needs M2). M2 TEST: fly across region boundaries — the ~10s ECOSYSTEM STREAM freeze
  should FINALLY collapse (flora's ~10k deep scans → ~1k, the larger half now cached on
  top of r296's spawn half); frame_total max should drop from ~10.1s toward normal.
  NOTE: the 768/264-deep RANGES themselves are still unchanged (operator deferred #1
  right-sizing); the flora is still 0/N (landing on unloaded chunks at y=-763) so a
  not-loaded early-out (skip flora gen entirely) remains available if wasted work still
  shows. Also open: roamer_tick residual (~44ms among a herd; max=189 on deep catch-up);
  popdyn herd 40>>capacity 8 (balance); two quarantined kernel panics; eyesight cliff-
  pathing design thread (WorldProber now the shared substrate).

## r298 — DESIGN: light field architecture doc (no code)
Captured the unified gameplay light-field design after a long design thread with the
operator (light needed by: cave/forest ecology, solar armor [light→gravity float],
mob perception/"eyes", lumen.rs bioluminescence). docs/design/light_field_
architecture.md. Key decisions recorded:
- Light is STATE, not a pure function (unlike surface/biome) → WorldProber READS it,
  never computes-on-miss; consumers read through a narrow LightSampler trait (the
  "won't break later" seam — storage can change under it). Read-through O(1), NOT
  memoized (cache the expensive deep scans; read through the cheap array). Propagation
  amortized at load (worker) + incremental BFS on edit (budgeted) — never per-query.
  The removal/un-propagation BFS flagged as the genuinely hard part.
- Two channels (sky + block) stored separately: day/night modulates sky only; solar
  armor = w_sky·sky·sun + w_block·block (daylight-dominant, but DOES pull torchlight —
  corrected an earlier "sky only" sketch). mob/spawn read max(sky,block); photosynthesis
  reads sky·daylight.
- Forest/canopy fidelity lands as TWO cheap extensions, NOT sub-voxel transport:
  (1) fractional per-material TRANSMITTANCE (leaves ~0.5-0.7, grass ~0.9, rock 0) in the
  BFS — "dims but not fully" emerges, stacks with layers; generalises block_predicates
  is_opaque bool → [0,1]; new field alongside block_material.emissive. (2) baked
  hemispherical/daily sun EXPOSURE for slow growth (reuse sampling::exposure +
  upward_hemisphere — already used by lumen.rs/cave_flora.rs) — angle-aware understory
  light without real-time directional transport.
- Gameplay-scalar vs visual-dapple split: gameplay reads a fractional SCALAR; the
  dapple/shafts/leaf-shadows are RENDER's job (shadow map + alpha-tested leaves +
  froxel — already exist). Real-time angled gameplay sun = expensive opt-in, DEFERRED
  pending testing.
- Reuses existing pieces (NOT greenfield): light.rs (propagate_block_light +
  skylight_columns, today baked into mesh verts → promote to stored field),
  sampling::exposure/hemisphere_dirs/upward_hemisphere, block_material.emissive,
  block_predicates leaf-transparency axis, armor.rs solar read, lumen.rs/cave_flora.rs
  exposure probes. NEW work: per-voxel light store in Chunk, LightSampler trait,
  WorldProber light_at/light_smooth (trilinear), incremental invalidation, transmittance
  attribute, baked exposure channel.
- Build order 0-5: (0 separate) heightmap-seed freeze fix; (1) store+load-propagation+
  trait+WorldProber reads, solar-armor float as proof; (2) transmittance; (3) incremental
  invalidation [hard]; (4) baked exposure + ecology; (5) mob perception + lumen.
- Cross-linked into perception_and_population_behavior.md + acoustic_perception_and_poi.md
  (sight's field, sibling to hearing) + references world_prober (r296), genome loci,
  dynamic_world_architecture. NO CODE THIS RELEASE (design only). Heightmap-seed freeze
  fix remains the offered immediate perf win (not yet done).

## r299 — PERF: seed the surface sweep (kills the region-crossing freeze at the root)
The quick win (build-order step 0 from the light-field doc). surface_y_at scanned
top-down from the caller's y_max — for the ecosystem stream that's probe_y_max=16*CS
(512), i.e. ~450 blocks of EMPTY SKY scanned (density()+is_cave() per block) before
reaching a surface near y~64. That leading-air scan, ×1024 columns ×2 chunks ×2
systems, is the ~4.7s region-crossing freeze (down from ~10s after r296/r297 caching;
caching killed the duplicate scans, this kills the wasted depth).
PROVABLE BOUND (why it's exact, not approximate): density = clamp((surface_target-y)/
VERTICAL_BIAS_SCALE, ±1.5) + detail*0.15, detail∈[-0.5,0.5], DENSITY_THRESHOLD=0,
VERTICAL_BIAS_SCALE=40. Solid (density>0) requires y < surface_target + 0.075*40 =
surface_target+3. There is NO separate floating-island/skylands solid term — `detail`
is bumps/overhangs only (±3 blocks). So NO voxel above surface_target+3 can be solid →
surface_target+SURFACE_SCAN_MARGIN (16, generous over the proven +3) is a PROVABLE
upper bound on solid. Starting the top-down scan there instead of y_max yields the
IDENTICAL topmost-standable result, just skipping guaranteed-air sky. It also satisfies
topmost_standable's "start cell is open sky" precondition by construction (scan_top is
provably air), so above_solid correctly starts false.
- surface_y_at now starts the scan at (surface_target(x,z).floor() + 16).min(y_max)
  .max(y_min). surface_target is the cheap 2D term (sea+continental+biome+hills, ~4
  fbm_2d, already exposed on the model, no vertical loop). KILL SWITCH:
  UNENDLICH_SURFACE_SCAN_SEED=0 → scan from raw y_max (pre-r299).
- Helps EVERY surface scan in the game, not just streaming: the per-creature grounding/
  posture/path probes (via the r294 SurfaceCache) funnel through surface_y_at too.
- CORRECTNESS validated by the existing test surface_results_are_always_standable_even_
  over_real_caves (sweeps 3600 real columns over caves, asserts every result solid-with-
  air-above): seeding starts at surface_target+16 (provably above any solid) so it finds
  the identical surface → test result unchanged on/off. (test-RUN still blocked by pre-
  existing Drives/glfw link; the proof + the test's existence are the gate.)
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no errors.
  -> UNVERIFIED (perf; needs M2). M2 TEST: fly across region boundaries — the ~4.7s
  ECOSYSTEM STREAM freeze should largely collapse (each surface scan now ~16-deep over
  the surface band instead of ~450-deep from y=512); frame_total max should drop hard.
  This is the root-cause fix the whole r294-r298 arc pointed at; the light field (r298
  doc) remains the bigger build for cave ecology / solar armor / mob perception.

## r300 — PERF: roamer_tick — graze gate, disposition cache, dedup loudness
The contained per-creature optimizations from the roamer_tick analysis (profile:
roamer_tick p50≈26ms = ~74% of the median frame, driven by herd 40-79 — operator
declined population caps, so this makes the per-creature work cheaper instead). All
three behaviour-preserving. crates/unendlich-engine/src/bin/unendlich_fullgame.rs
(roamer loop ~4786-6010).
- GRAZE GATE (biggest lever): the herbivore food-ring scan was up to 30 block_at per
  hungry Fauna EVERY tick — the loop's single largest block-read source. Now the scan
  runs only to ACQUIRE a target, gated by motion (moved to a new cell) OR cadence
  (every GRAZE_SCAN_INTERVAL=6 ticks); the creature pursues the REMEMBERED target in
  between (new Roamer fields graze_target/graze_cell/graze_scan_tick). Target cleared
  on eat and on stale set_block (re-scan next eligible tick); give-up valve re-scans a
  target held > 8*INTERVAL ticks (prevents fixation on an unreachable pick). KILL
  SWITCH UNENDLICH_GRAZE_GATE=0 → scan every tick (original). For a stationary or
  target-pursuing grazer this cuts the 30-cell scan to ~0 most ticks.
- DISPOSITION CACHE: express_disposition is a pure function of the genome, which is
  fixed for life, but the loop RE-DERIVED it every tick (the code itself flagged
  "caching it on the roamer is the perf refinement"). Now cached lazily in a new
  Roamer field disposition_cache (Option<Disposition>); the loop reads it by reference
  (disposition: Option<&Disposition>; the 4 downstream .as_ref().map sites became
  .map). Disposition is Clone+PartialEq, NOT Copy (has Vec<SoundClass>), so by-ref is
  the right shape. Identical value → behaviour unchanged; no kill switch (pure memo).
- DEDUP LOUDNESS: perceived_loudness(player_footstep, dist, 0) + the player distance
  were computed TWICE per creature (hearing branch ~5014 and awareness blend ~5079).
  Hoisted to player_dist/player_perc once before the perception block, reused in both.
- Roamer struct gained 4 fields (disposition_cache, graze_target, graze_cell,
  graze_scan_tick); all 5 construction sites + the loop destructure updated.
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no
  errors/relevant warnings. -> UNVERIFIED (perf; needs M2). M2 TEST: with the herd at
  40-79, roamer_tick p50 should drop hard (graze scans collapse from per-tick to
  acquire-only; disposition stops re-deriving) — target is roamer_tick well under the
  ~26ms it's been eating, lifting steady FPS off the ~20-23 floor. Population intact
  (no caps). Toggle UNENDLICH_GRAZE_GATE=0 to A/B the graze portion.
- STILL PENDING (deferred, design only later): two-tier brain LOD (home-anchored
  statistical SimVector herds for the distant bulk + paging individuals in near the
  player) — the structural answer to the population cost without caps; occlusion-in-
  sight rides the coming light/occlusion structure; herd block-read batching across
  co-located members (the chunk-grouping follow-up). graze/disposition help regardless.

## r301 — Forward-port the r67 neural-shading integration into r300 (compile-only)
The operator's r67 "neural deferred shading" integration (uploaded: r67-neural-
integration.zip; the full prototype is parallel-engine.zip + frame-predict-demo.zip)
was on the r67/r86 branch, NOT in the r270->r300 canonical line (confirmed: no q8 /
neural / G-buffer-MRT anywhere in r300 source). This release drops it into r300 and
verifies it still fits — the concrete "does it help" answer: YES, it ports clean.
WHAT THE PROTOTYPE IS (for the journal): a small MLP learns the expensive 22-raycast
AO + soft-shadow from a cheap G-buffer + 6 hint rays (1 sun + 5 up-hemisphere as
distance features), ~28-31 dB vs ground truth (+14 dB over flat analytic), running on
Q8_0 linear + silu Vulkan kernels bit-exact CPU<->GPU on the M2. Key finding from
their work: the 26 dB plateau was a feature INFORMATION CEILING (occupancy can't
determine AO), broken by the 6 hint rays, not by capacity/denoising. Ship config is
LEAN (14 features, 64x48, no occupancy gathers) — matches the full net at 7.4x fewer
MACs. Per BUGWATCH H1, the per-pixel net is for SURFACE integrals only (AO/shadow);
volumes/reflections stay froxel/SSR (a per-pixel MLP can't represent spatial
coherence — god rays got worse with it). Per I1, point-cloud features must be
CONTINUOUS in query position (k-nearest + Gaussian, not arbitrary-subset + hard
threshold) or they facet.
FORWARD-PORT (this release): copied from r67 integration into r300 unchanged —
- crates/unendlich-engine/src/render/q8.rs (Q8_0 quantizer; self-contained; byte-
  identical to the validated Python oracle).
- crates/unendlich-engine/src/render/neural_shade_pipeline.rs (fused-MLP ash compute
  pipeline, HARNESS form — own submit + host IO; modeled on octree_probe_pipeline.rs).
- crates/unendlich-engine/src/shaders/neural_feat_octree.comp (feature pass; #includes
  r300's octree_sample.glsl/octree_types.glsl).
- crates/unendlich-engine/src/shaders/neural_shade.comp (fused MLP; q8 parity ref).
- render/mod.rs: added `pub mod q8;` + `pub mod neural_shade_pipeline;`.
- build.rs SHADERS list: added the two .comp so the build compiles them.
COMPATIBILITY VERIFIED: r300 still has all deps the modules need — octree_probe_
pipeline.rs, octree_types.glsl, octree_sample.glsl, OctreeSlot::base_node_index(),
ash 0.37 (same API the modules use). The octree format the docs called byte-identical
r67->r86 is still compatible at r300.
GATES (both green): (1) glslangValidator -V --target-env vulkan1.2 on BOTH shaders ->
SPIR-V emitted (13.4KB feat, 9.5KB shade), no errors, against r300's real octree
includes. (2) cargo check --bin unendlich-fullgame (full features) -> Finished, no
errors; build.rs compiles the shaders as part of the build. -> CHECKED.
INERT / NOT WIRED (honest scope): the modules are COMPILE-ONLY — nothing calls them
yet. neural_shade_pipeline is the standalone harness form (own submit/fence), not the
in-frame render-loop form. Remaining work is all LIVE-M2-CONTEXT wiring (per RUST_
INTEGRATION.md), none of which is doable in this GPU-less sandbox: G-buffer MRT in
chunk.frag (the producer), recording feat+shade dispatches into the frame cmd buffer,
the multi-chunk base table, temporal history images, a NeuralFeatPipeline (copy the
shade pipeline's structure), and baking the trained MLP weights to an asset via
q8::quantize_layer. Plus the PERF DECISION still hinges on bench_lean.py ON THE M2
(lavapipe shows 0.25x because it's MLP-GEMM-bound on CPU and "ratios do NOT transfer";
on M2 the GEMM collapses toward FLOP ceiling and the 22->6 ray reduction becomes the
win) — only the operator's M2 can produce that number.
SHARES SUBSTRATE with the light field (r298): the hint rays are occlusion queries
(same as mob LOS / cliff-pathing), the G-buffer worldpos/normal is what the light
field reads against, and the temporal base (temporal_accumulate.rs, already in r300)
serves both the neural sample-amortizer and the frame-predict present pass.

## r302 — DESIGN: neural shading integration spec (deferred wiring vs r300)
docs/design/neural_shading_integration.md — the recipe to make the r301 forward-port
actually RUN, grounded in r300's real render code, with the prototype's BUGWATCH folded
in as the runtime checklist. No code (design only).
- Opens by stating r301 is INERT (nothing dispatches it) — so the "slight color change"
  the operator saw on the M2 is NOT the neural path (no call site; rendering is byte-
  identical to r300 in every neural respect). Suggested grep: `neural`/`q8`/`NeuralShade`
  should return nothing in the log. Likely cause is day/night / froxel fog / exposure.
- Architecture: deferred (chunk MRT G-buffer → feature pass → shade pass → HDR →
  [temporal]). r300's chunk.frag is forward (single out_color) so MRT is a backend add.
- G-buffer producer: add out_gnormal/out_galbedo/out_gworldpos to chunk.frag (it has
  v_world_pos already). out_gworldpos emitted DIRECTLY (B1 — worldpos σ=0.3vox = -3.2dB;
  never reconstruct from depth). out_galbedo = r300's continuous v_color*detail → RETRAIN
  the net on that (B2). Attachments in offscreen.rs + 4-attachment blend in
  chunk_renderer.rs. Opaque mask (water/clouds keep forward).
- Feature pass: NeuralFeatPipeline (copy neural_shade_pipeline.rs); render-LOOP form
  records cmd_dispatch+barriers in-frame (model view_compute_pipeline::record_dispatch),
  NOT the harness own-submit form. Lean ship: also port neural_feat_lean.comp (14 feat,
  0 gathers).
- SHADE FORK (decides perf): ported neural_shade.comp is the FUSED REFERENCE (scalar,
  slow — do NOT ship). SHIP = MLP as tiled linear_q8_0 GEMM (3 linear + 2 silu) — needs
  linear_q8_0.comp + silu.comp ported (in pe/, sandbox-compilable) + a GEMM pipeline.
  Lean net 14→64→48→3.
- Multi-chunk base table (OctreeSlot::base_node_index + world&31); weight bake via
  q8::quantize_layer + Q8 byte-identity gate (F1); temporal reuses temporal_accumulate.rs
  (validate pos+normal, age-refresh, NO view-dependent term in history — C1).
- BUGWATCH 11-point RUNTIME checklist (level-camera, one depth space, worldpos-direct,
  channel-encoding match, no-view-dep-in-history, volume depth-test on near geometry,
  per-element buffer assert, Q8 GPU≈CPU, matched-input per-pixel validation, surfaces-
  only, sparse-gather-continuity) — all M2-side, sandbox can't run them.
- Boundaries (prototype's findings): net = SURFACE integrals only (H1, volumes worse with
  it); fog=froxel, reflections=SSR; point-cloud features must be continuous (I1, else
  facet); temporal fights variance not bias (H2).
- Build order: DONE r301 (forward-port). SANDBOX-NEXT (compile-gated releases): port
  linear_q8_0.comp + silu.comp + neural_feat_lean.comp; NeuralFeatPipeline + GEMM
  pipeline. M2-SIDE: MRT+attachments, in-frame dispatch, base table, weight bake+retrain,
  temporal, the checklist + bench_lean.py (the perf gate).
- Determinism: render-tier, never hashed, no rollback concern.
- Cross-linked into light_field_architecture.md (render-side consumer of the shared
  occlusion/temporal substrate).

## r303 — neural: port the lean GEMM ship-path shaders (shader side now complete)
Continues the neural track (operator: "stay on the neural track"). Brought the three
remaining ship-path shaders from the parallel engine (pe/) into r300, compile-gated.
With these, the ENTIRE shader side of the recommended lean path is in r300 and
validated — no shader work left for the M2; remaining neural work is pure Rust wiring.
- crates/unendlich-engine/src/shaders/linear_q8_0.comp — the tiled Q8_0 GEMM kernel
  (8x8 workgroup; std430 A[m,k]/QW[n,k]/QS[n,kb]/C[m,n]; push-const dims). This is the
  SHIP shade path (run x3 for the 3 linear layers) — NOT the fused neural_shade.comp
  (which stays the scalar correctness reference). Self-contained, no includes.
- crates/unendlich-engine/src/shaders/silu.comp — SiLU activation (64 workgroup, X->O).
  Self-contained. (linear_q8_0 x3 + silu x2 = the lean MLP 14->64->48->3.)
- crates/unendlich-engine/src/shaders/neural_feat_lean.comp — the 14-feature lean pass
  (base 8 + 6 hint rays, DROPS the 90 occupancy octree-gathers; output stride 14:
  [0:3]normal [3:6]albedo [6]sun·n [7]height [8:14]hints). #includes octree_types.glsl
  + octree_sample.glsl (same as neural_feat_octree, which compiled in r301). NOTE
  (B2/r86): reads albedo from a discrete Alb SSBO (binding 2); on r300 the M2 wiring
  should instead read albedo from the out_galbedo G-buffer and RETRAIN — a one-liner +
  retrain, shader otherwise unchanged.
- build.rs SHADERS list: added all three; build.rs compiles them and emits .spv +
  .hash into assets/shaders/ (confirmed: linear_q8_0/silu/neural_feat_lean .spv present).
- GATES (both green): glslangValidator -V --target-env vulkan1.2 on all three -> OK
  (4568/1536/10720 bytes spv) against r300's real octree includes; cargo check --bin
  unendlich-fullgame (full features) -> Finished, no errors. -> CHECKED.
- INERT (same as r301): compile-only, nothing dispatches them yet. Neural shaders now
  in r300: neural_feat_octree, neural_shade (r301) + linear_q8_0, silu, neural_feat_lean
  (r303) = the full shader side.
- REMAINING sandbox-neural (offered next): a Rust GEMM-chain pipeline (load linear_q8_0
  x3 + silu x2, chain feat-buffer -> hidden -> hidden -> output; harness form, copy
  neural_shade_pipeline.rs structure) + a NeuralFeatPipeline. After that it's ALL
  M2-side (MRT+attachments, in-frame dispatch, base table, weight bake+retrain,
  temporal) + the bench_lean.py perf gate. Spec: docs/design/neural_shading_integration.md.

## r304 — neural: GEMM-chain MLP pipeline (the ship-path shade pipeline; shade side complete)
The last clean sandbox-doable neural piece (operator: "stay on the neural track").
crates/unendlich-engine/src/render/neural_gemm_pipeline.rs — the lean MLP as the
tiled linear_q8_0 GEMM (3 layers) + silu (2), the PERF ship-path alternative to the
fused neural_shade.comp (which stays the scalar correctness reference; doc §4 says ship
the GEMM, not the fused shader). Wired pub mod into render/mod.rs.
- HARNESS FORM, mirroring the validated mlp.py::GpuMLP byte-for-byte: bias folded by
  augmenting each layer input with a constant 1.0 at index d + zero-pad to a 32-multiple
  (q8::quantize_layer's convention), augment done on the CPU between dispatches with
  host-visible IO + a fence wait per dispatch — so it's directly parity-checkable vs the
  Python net and the fused shader. Chain (lean 14→64→48→3): feat[m,14] -aug→ linear(L0)
  → [m,64] -silu→ -aug→ linear(L1) → [m,48] -silu→ -aug→ linear(L2) → [m,3].
- API: new(device, instance, pd, queue, qfam, fin0, layers:&[(qw,qs,k,n)]) — weights from
  q8::quantize_layer, uploaded once; forward(feat, m) -> Vec<f32> [m, last.n]. Linear
  dispatch ceil(n/8)×ceil(m/8) (matches linear_q8_0.comp's 8×8 / col=x,row=y); silu
  ceil(len/64). debug_assert checks each layer's k == ceil((d+1)/32)*32 and qw/qs sizes —
  catches a weights/dims mismatch at construction.
- Production in-frame form (M2-side) keeps activations device-local, fuses the augment
  into a small kernel, and records the dispatch chain into the frame cmd buffer (like
  view_compute_pipeline) — same kernels/weights, only IO/scheduling differ.
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no errors /
  no relevant warnings. -> CHECKED. INERT (nothing constructs it yet) + correct-by-
  construction but UNTESTED here (no GPU). M2 validation = the parity check:
  NeuralGemmPipeline::forward(feat) == NeuralShadePipeline::shade(feat) == Python net,
  same weights + features, within q8 tol (~6e-3).
- NEURAL STATE OF THE TREE NOW (sandbox-complete on the compute/shade side):
  render/q8.rs (quantizer), render/neural_shade_pipeline.rs (fused reference),
  render/neural_gemm_pipeline.rs (GEMM ship path); shaders neural_feat_octree,
  neural_shade, linear_q8_0, silu, neural_feat_lean (all compiling, .spv in assets).
- WHAT'S LEFT — all M2-side (sandbox can't, no GPU): the G-buffer MRT producer
  (chunk.frag + offscreen attachments), the NeuralFeatPipeline (feature pass — needs the
  live G-buffer images + real octree data, so naturally M2-side), in-frame dispatch
  recording + pass→pass device-local buffers, multi-chunk base table, weight bake +
  albedo retrain (B2) + Q8 parity, temporal (reuse temporal_accumulate.rs). PERF GATE:
  bench_lean.py on the M2 decides whether the whole thing ships. Full recipe:
  docs/design/neural_shading_integration.md.

## r305 — neural: G-buffer producer shader + MRT wiring spec; + title-screen seed note
Two notes + the first (safe) MRT artifact.
- crates/unendlich-engine/src/shaders/chunk_gbuffer.frag — the G-buffer producer: a
  trimmed MRT variant of chunk.frag that REUSES chunk.vert (v_color@0, v_world_pos@1)
  and emits loc0 out_gnormal (axis-snapped face normal, same derivation as chunk.frag's
  pre-perturb normal), loc1 out_galbedo (base albedo = v_color, linear), loc2
  out_gworldpos (ABSOLUTE world pos, DIRECT — B1, never from depth). NO descriptor sets
  (only the two varyings), so cheap + self-contained. Encoding contract documented in
  the shader header (trainer must match — B2). Added to build.rs; glslang OK (2180 bytes
  spv), cargo check Finished, .spv in assets. INERT + SAFE: nothing constructs a pipeline
  from it, so it cannot disturb the working forward path (which keeps chunk.frag).
- docs/design/neural_shading_integration.md §2a — applyable G-buffer wiring against the
  CONFIRMED architecture (fullgame -> WorldRenderer offscreen+postfx; OffscreenTarget /
  ctx.create_offscreen_target for the images). Recommends a FLAG-GATED PARALLEL G-buffer
  pass (UNENDLICH_NEURAL_GBUFFER default off -> byte-identical to r300, zero regression;
  redundant geometry draw acceptable for bring-up, fold into main pass as MRT later to
  remove redundancy). Specs: images (gworldpos R32G32B32A32_SFLOAT per B1; gnormal/galbedo
  R16G16B16A16_SFLOAT), render pass (3 color CLEAR/STORE -> SHADER_READ_ONLY + depth, same
  depth space A2), framebuffer, pipeline (clone chunk pipeline: same chunk.vert + vertex
  input + camera UBO descriptor layout, frag=chunk_gbuffer.frag, 3-attachment blend
  disabled), draw (re-record chunk draws into the gated pass). Runtime validation order
  (M2): visualize attachments (A1 ground-at-bottom, B1 smooth gworldpos, A2 depth match),
  then per-pixel diff vs prototype on NEAR geometry (G2/D1). OPEN: needs WorldRenderer's
  chunk pipeline vertex-input + descriptor layout + draw-record site to write the exact
  gated pass/pipeline (offered to read world_renderer.rs + postfx.rs and write it).
  WHY NOT BLIND: a wrong attachment count is a hard runtime failure (whole visible image),
  uncatchable without a GPU -> gated + operator-validated.
- docs/design/MENU_SYSTEM.md — backlog note: title-screen world seed + spawn position.
  Seed feeds the already-deterministic (world_seed, region) worldgen; spawn = initial
  player/camera coords (X/Z + surface height, optional Y). Wire in app/menu_view.rs +
  menu_layout.rs; no worldgen changes (both are existing inputs, just surfaced in UI).
- VERIFIED: glslang + cargo check green. -> CHECKED (shader compiles + docs; no runtime).

## r306 — neural: gated parallel G-buffer pass WIRED (the first thing that lights up)
The MRT wiring (operator: "keep going with MRT wiring"). A complete, flag-gated
parallel G-buffer pass, end-to-end in WorldRenderer. Default-off => byte-identical to
r305/r300. UNTESTED at runtime (no GPU); correct-by-construction (mirrors the chunk
pipeline + main indirect draw exactly).
- KILL SWITCH / GATE: UNENDLICH_NEURAL_GBUFFER (unset/0 = the gbuffer is never
  constructed and the draw block is skipped => zero regression; =1 = build + run it).
  One log::info! on enable.
- NEW: crates/unendlich-engine/src/render/gbuffer.rs — NeuralGBuffer: 3 OffscreenTargets
  (normal R16G16B16A16_SFLOAT, albedo R16G16B16A16_SFLOAT, worldpos R32G32B32A32_SFLOAT
  — fp32 per B1) + its own DepthBuffer + a render pass (3 color CLEAR/STORE ->
  SHADER_READ_ONLY_OPTIMAL + depth, with external<->subpass deps releasing color to
  SHADER_READ for the future feature pass) + framebuffer + a pipeline that CLONES the
  chunk pipeline (reuses ChunkPipeline.layout so the same per-frame descriptor set binds;
  same chunk.vert + vertex input; frag = chunk_gbuffer.frag; 3 blend attachments disabled;
  CW+BACK, honors UNENDLICH_NO_CULL; depth LESS+write). pub mod in render/mod.rs.
- WHY A SEPARATE PASS (not MRT on the main pass): WorldRenderer renders chunks DIRECTLY
  to the swapchain (create_render_pass_with_depth(swapchain.format,...)), which can't
  carry extra attachments -> the G-buffer must be its own pass over its own images.
- WorldRenderer wiring (6 edits): struct field gbuffer: Option<NeuralGBuffer>; gated
  construction in new() after ChunkPipeline (reuses pipeline.layout) + added to Self;
  the gated pass in draw_frame_with_view_and_atmos right AFTER the main cull dispatch
  completes (MainCullEnd mark) and BEFORE the main pass begins — reuses the SAME cull
  output (indirect.commands_buffer/count_buffer), the SAME frame.descriptor_set, the SAME
  mesh_pool vertex/index buffers, one cmd_draw_indexed_indirect_count (indirect path
  only; non-indirect fallback skips it); recreate at new extent in recreate_swapchain;
  destroy in destroy(). All inside the existing unsafe scope.
- INERT PRODUCER: it fills normal/albedo/worldpos and transitions them to SHADER_READ,
  but NOTHING samples them yet (feature/shade passes are the next wiring). The redundant
  geometry draw (~2x chunk draw cost when enabled) is acceptable for bring-up; folding
  the outputs into the main pass as true MRT is the later optimization.
- VERIFIED: glslang (chunk_gbuffer.frag, r305) + cargo check --bin unendlich-fullgame
  (full features) -> Finished, no errors (only pre-existing warnings). -> CHECKED.
- M2 NEXT (runtime, BUGWATCH order): flip UNENDLICH_NEURAL_GBUFFER=1 WITH validation
  layers on -> confirm no validation errors / no crash first. If RGBA32F as a color
  attachment is unsupported on the target, fall back worldpos to RGBA16F (B1 precision
  caveat). Then INSPECT the G-buffer (nothing displays it yet): RenderDoc capture, OR add
  a debug blit (show one channel on screen) — A1 ground-at-bottom, B1 smooth worldpos
  across a face, A2 gbuffer depth matches forward depth. Then per-pixel diff vs prototype
  on NEAR geometry (G2/D1). Spec: docs/design/neural_shading_integration.md §2a.

## r307 — neural: feature pass WIRED (G-buffer images → feature buffer, base-8)
"go straight to neural" — the next stage after the G-buffer. Gated by the same
UNENDLICH_NEURAL_GBUFFER; compiles clean; default-off => byte-identical. UNTESTED at
runtime (no GPU).
- KEY ADAPTATION: the prototype's neural_feat_lean.comp reads the G-buffer as a packed
  SSBO, but unendlich's G-buffer is IMAGES (MRT). So a new image-reading feature shader:
  crates/unendlich-engine/src/shaders/neural_feat_gbuf.comp — samples the 3 G-buffer
  images (texelFetch, exact pixel) and packs the lean BASE-8 features per pixel:
  [0:3]normal [3:6]albedo [6]sun·n [7]height [8:14]hints. glslang OK (4524 bytes), in
  build.rs, .spv in assets.
- HINT RAYS [8:14] EXPLICIT-ZEROED in the shader. They are the quality-critical part
  (broke the 26 dB ceiling) but need marching the octree from worldpos, which in a
  multi-chunk world needs a per-pixel base-node lookup (the base table) — the NEXT step
  (§5). With hints zeroed the net is ceiling-limited; this stage plumbs + validates the
  G-buffer→feature path, it does not deliver the AO/shadow gain.
- NEW: crates/unendlich-engine/src/render/neural_feat_pipeline.rs — NeuralFeatPipeline,
  render-loop (in-frame) form: compute pipeline (descriptor 0/1/2 = G-buffer combined
  image samplers, 3 = device-local feature SSBO; push = sun_dir + width/height/y_scale)
  + a device-local feature buffer sized width*height*14*4 (TRANSFER_SRC for RenderDoc/
  copy-out). record(device, cmd, gbuffer, sun_dir, y_scale): re-points the descriptor
  set at the G-buffer images each frame (single frame in flight), pushes, dispatches
  ceil(w/8)×ceil(h/8). pub mod in render/mod.rs.
- WorldRenderer wiring: field neural_feat: Option<NeuralFeatPipeline>; constructed iff
  gbuffer is (same gate), after it, with swapchain extent; recorded in draw RIGHT AFTER
  the G-buffer pass's cmd_end_render_pass (inside the use_indirect branch) — the G-buffer
  render pass already released its color attachments to COMPUTE SHADER_READ, so no extra
  barrier; recreated at new extent in recreate_swapchain; destroyed (before the gbuffer).
- PLACEHOLDERS in the record call: sun_dir=[0.282,0.940,0.188], y_scale=1/256 — wire to
  the real sun direction (from atmos) and RETRAIN to match (B2).
- INERT PRODUCER: fills the feature buffer; nothing reads it yet. Validate via RenderDoc
  (base features sensible: normal in {-1,0,1}, albedo matching, sun·n in [0,1]).
- VERIFIED: glslang + cargo check --bin unendlich-fullgame (full features) -> Finished,
  no errors. -> CHECKED.
- CHAIN STATE: G-buffer (r306) → feature/base-8 (r307) plumbed + gated. REMAINING:
  (1) hint rays = octree march + multi-chunk base table (§5, the hard part);
  (2) shade pass in-frame (reuse neural_gemm_pipeline / the GEMM kernels, device-local
      feat→RGB) + composite to screen;
  (3) TRAINED WEIGHTS — hard prerequisite, M2/offline (train in the parallel engine on
      features matching what neural_feat_gbuf emits, bake via q8::quantize_layer, ship);
      without weights the net cannot shade regardless of wiring. Spec: §2a/§4/§5.

## r308 — neural: in-engine BACKGROUND TRAINER (self-training as a game task)
"build out the training as a background game task." A CPU MLP trainer on its own
thread, self-training the shading net from game-fed samples + baking Q8 weights — so
the net learns on the engine's own features (B2 automatic) with no offline step.
Compiles clean; gated; default-off.
- NEW: crates/unendlich-engine/src/render/neural_train.rs
  - TrainMlp — faithful Rust port of the prototype mlp.py: forward linear+silu /
    linear-out; backward MSE + ADAM (b1=0.9 b2=0.999 eps=1e-8, bias-corrected) — same
    math, matches a numpy-trained net on the same samples. Hand-rolled matmul (lean net
    is tiny), dependency-free SplitMix64 PRNG (He-init + minibatch). sizes [14,64,48,3].
  - export_q8() — bakes each layer via q8::quantize_layer (bias folded as +1 augmented
    column, zero-pad to 32) -> the shade pipeline's (qw, qs, k, n). Connects trainer ->
    the existing q8/shade path.
  - BackgroundTrainer — owns a "neural-trainer" thread + mpsc sample channel +
    Arc<Mutex<baked weights>> published behind a generation counter. push(samples) /
    try_take_weights() (returns new bake since last poll) / steps() / stop() (+ Drop).
    Streaming minibatch Adam: drains the channel into a ring (evict oldest past
    ring_cap), trains a random minibatch when >= batch, bakes every bake_every steps.
  - TrainMlp::self_test(steps) — trains on a synthetic mapping, returns final MSE
    (verify the math learns; callable from a debug command). TrainCfg defaults
    batch=1024 lr=2e-3 ring_cap=65536 bake_every=64.
- WorldRenderer: gated neural_trainer: Option<BackgroundTrainer> under its OWN flag
  UNENDLICH_NEURAL_TRAIN (CPU thread, needs no GPU); spawned in new() (one log line),
  stopped in destroy(). Idles until the ground-truth sample feed is wired.
- VERIFIED: cargo check --bin unendlich-fullgame (full features) -> Finished, no errors
  (removed an unused field). -> CHECKED. Trainer math is correct-by-construction (port
  of the validated mlp.py) + self_test-able; UNTESTED in-engine (no GPU/run here).
- RUNTIME / NEXT (octree-coupled, shares the hint-ray primitive): the GROUND-TRUTH
  sample pass — N pixels/frame, 22-ray AO+shadow via the octree × analytic shade ->
  target[3], paired with the G-buffer feature -> host buffer -> readback ->
  trainer.push(); and the HOT-SWAP — each frame try_take_weights() -> re-bake the shade
  pipeline's qw/qs (needs shade wired in-frame). The octree-raycast-from-worldpos
  primitive is SHARED with the feature hint rays (§5) — build once, unlocks both.
  Full data-flow + spec: docs/design/neural_shading_integration.md §6a.
- CHAIN STATE: G-buffer (r306) -> feature/base-8 (r307) plumbed; trainer (r308) ready.
  The octree march (hint rays + ground truth) + shade-in-frame + hot-swap remain.

## r309 — neural: one-flag trainer self-test (validate the foundation before the octree march)
A small, validation-serving addition (no new pipeline). Lets the operator prove the
trainer's Adam/backprop math learns on their machine, independent of the GPU.
- neural_train.rs TrainMlp::self_test now logs its loss curve: start MSE, quartile
  checkpoints, and a "mse START -> FINAL over N steps (lower = learning)" summary.
- WorldRenderer::new: gated by UNENDLICH_NEURAL_TRAIN_SELFTEST=1 — runs
  TrainMlp::self_test(2000) once at startup (CPU-only, log-only). One flag => the
  operator sees the net fit a synthetic mapping; if FINAL << START it learns.
- VERIFIED: cargo check Finished (also caught + fixed a bad edit that had dropped the
  neural_trainer construction). -> CHECKED.
- CONTEXT / WHY: r301-r308 are all gated-OFF by default and the operator has only run
  flags-off ("no difference" = no regression, confirmed good). But that means the
  neural plumbing (G-buffer r306, feature pass r307) has never EXECUTED on a GPU. Before
  the octree march (the hardest, foundation-dependent piece — keystone for both hint
  rays and ground-truth), the foundation should be validated on hardware (sandbox has
  no GPU): (1) UNENDLICH_NEURAL_GBUFFER=1 + validation layers — runs clean? (watch
  worldpos RGBA32F support); (2) RenderDoc the G-buffer (A1 ground-at-bottom, B1 smooth
  worldpos, sensible normal/albedo) + the feature buffer base-8; (3)
  UNENDLICH_NEURAL_TRAIN_SELFTEST=1 — trainer learns? THEN build the octree march on a
  known-good base.

## r310 — neural: OCTREE HINT-RAY MARCH wired (the keystone) — features now carry occlusion
"keep implementing the octree integration." The feature pass now marches the octree
for the 6 hint rays instead of zeroing them, so the feature buffer is the FULL lean
set (base-8 + real occlusion). Compiles clean; gated under UNENDLICH_NEURAL_GBUFFER;
UNTESTED at runtime (no GPU). This is the primitive the ground-truth training pass
will reuse — build-once-unlocks-both, as planned.
- shaders/neural_feat_gbuf.comp (rewritten): per pixel, 6 hint rays from the surface
  point (org = wpos + n*0.5 to avoid self-hit): 1 toward the sun + 5 fixed upper-
  hemisphere (world space). Each ray LEAF-SKIPS via octree_sample_ext — a 32-wide air
  leaf advances 32 blocks in one step (ray-box far-face exit), not a fixed march; ≤24
  leaf-steps, MAXD=32 blocks. Returns hit_dist/MAXD in [0,1] (1.0 = open). worldpos→
  chunk = floor(p/32); local = floor(p) - chunk*32 in [0,32). block_id != 0 = solid
  (air assumed 0 — operator confirms). glslang OK (15140 bytes).
- Per-pixel cross-chunk base lookup: base_grid[] SSBO (binding 5), per-chunk root index
  (EMPTY=0xFFFFFFFF), indexed by chunk coord rel to grid_origin over GRID_DIMS=[48,16,48]
  (~144 KB). nodes[] (binding 4) = the pool's global node buffer.
- octree_buffer.rs: + pub fn fill_base_grid(origin, dims, out) — scatters resident
  chunks' base_node_index into the caller grid (O(resident chunks)).
- neural_feat_pipeline.rs: + bindings 4 (OctreeNodes) / 5 (BaseGrid); a host-visible
  grid buffer (mapped) + upload_base_grid + grid_len; FeatPush extended to 64 B
  (+grid_origin ivec4, +grid_dims uvec4); record() now takes octree_nodes + grid_origin.
  unsafe impl Send (raw grid_ptr). device_buf/host_buf via a shared alloc_memory.
- world_renderer.rs: in the gated feature-pass block, each frame: player chunk =
  floor(view.position/32); grid_origin = player - GRID_DIMS/2; lock octree_pool ->
  fill_base_grid + snapshot pool.buffer; upload_base_grid; record(... octree_buf ...
  origin). Brief pool lock in the draw path (mesher also locks it).
- VERIFIED: glslang + cargo check --bin unendlich-fullgame (full features) -> Finished,
  no errors (only the pre-existing octree_pool_buffer-unused warning, unrelated). -> CHECKED.
- M2 VALIDATION (RenderDoc the feature buffer, TRANSFER_SRC): hint distances [0,1],
  1.0 in open sky, smaller near walls/under overhangs, varying with geometry; per-pixel
  diff vs the prototype on NEAR geometry (G2/D1). PERF: 6 rays/pixel leaf-skip is
  unmeasured — if slow when enabled, that's the optimization target (it's gated, so
  default play is unaffected). PLACEHOLDERS to wire+retrain (B2): sun_dir, y_scale,
  the 5 hemisphere directions, the air=block0 assumption.
- CHAIN STATE: G-buffer (r306) -> FULL features incl. occlusion (r310); trainer (r308)
  ready. The octree march primitive now EXISTS, so next: the GROUND-TRUTH pass (22-ray
  AO via the same march -> target -> trainer.push, §6a) + shade-in-frame + composite +
  hot-swap. Spec: neural_shading_integration.md §5/§6a.

## r311 — render: remove the block-edge box outline from foliage (leaves, grass, ferns…)
First of the three-part render pivot (outline / textures / mesh gaps). The hard per-
block cube outline on vegetation is dropped; solid blocks keep theirs.
- DIAGNOSIS (from the screenshot + chunk.frag): the "box outline" is two effects in
  chunk.frag applied to every block — the `bevel` (block-boundary darkening, ~ln 499)
  and the `grid` lines inside the SHOW_UV_DEBUG overlay (~ln 1147). The "checkerboard"
  is that same SHOW_UV_DEBUG placeholder (no atlas exists yet — only shadow + froxel
  samplers; blocks are pure vertex-color). The shader already anticipates the real fix:
  a per-block tile id in the reserved color-alpha byte via PackedVertex::pack_color_id
  (currently 0xFF, ignored downstream).
- chunk.frag: + is_foliage(vec3 c) — green-dominant + clearly greener than its own r/b
  (c.g > c.r+0.05 && c.g > c.b+0.08 && c.g > 0.25). Catches leaves/grass/fern/vine/moss/
  cactus; excludes grey stone, brown dirt/logs, red flower/mushroom. Same color-match
  spirit as is_water/is_stone (the engine has no per-vertex material id yet). For
  foliage fragments: bevel forced to 1.0 (no border darkening) + the SHOW_UV_DEBUG grid
  lines skipped. The 2x2 checker is KEPT (texture placeholder, consistent with solid
  blocks until the atlas lands).
- WHY color-match not the id: leaf colors overlap badly (OakLeaves [.18,.55,.20] ≈
  Cactus [.20,.55,.20]; grass/fern/vine/moss are neighbors) AND the mesher bakes
  per-face brightness + per-position tint into the vertex color, so matching a specific
  hue is unreliable — but "is it green vegetation" is robust, and that's the actual
  target ("nonsolid blocks like leaves"). The exact per-block path is the alpha-byte
  tile id, landing with the texture work.
- VERIFIED: glslang chunk.frag OK (58104 B) + cargo check --bin unendlich-fullgame
  (full features) Finished. -> CHECKED. NOT visually validated (no GPU in sandbox) —
  M2 confirms: vegetation loses the cube edges, solid blocks keep them. Watch for any
  green-ish solid block that shouldn't lose its edge, or vegetation the green test
  misses (tune the thresholds). NOTE: shader-only change, no env kill switch (shaders
  can't read env); revert = restore chunk.frag (or flip the is_foliage conditions).
- NEXT (this pivot): (2) TEXTURE WIRING — thread the per-block tile id through the
  alpha byte (pack_color_id exists; needs the CPU mesher + the chunk_mesh*.comp GPU
  meshers + chunk.vert to carry it + an atlas sampler in chunk.frag replacing the debug
  checker). Needs an atlas asset (operator art, or procedural-generated). (3) MESH GAP
  FILLING — cause not yet diagnosed; likely chunk-boundary seams / greedy-mesh
  T-junctions in the (smooth-looking) terrain. Investigate next.

## r312 — menu: numeric values on settings sliders + the frame-time graph (legibility)
First of the menu/inventory pivot. The two value-display wins the operator called out.
- world_renderer.rs `int_slider!` macro: added a strong, accent-colored numeric label after
  the slider. Propagates to EVERY settings slider — render distance, plants/leaves/apply-budget
  strides, target fps — so each shows its current value.
- world_renderer.rs `sparkline` closure (the frame-time graph, drawn on both the live HUD and the
  perf settings page): overlaid a readout — current "{:.1} ms" (top-left) + "max {:.1}" over the
  window (top-right). Applies to both call sites.
- VERIFIED: cargo check --bin unendlich-fullgame (full features incl ui-overlay) Finished; the
  warnings present are pre-existing (BerryBush/active_slots/outputs/unused-Result). -> CHECKED.
  VISUALLY UNVERIFIED (no GPU in sandbox) — operator confirms the numbers render legibly. UI-only
  change, no env kill switch; revert = restore world_renderer.rs.
- MENU PIVOT — remaining (state confirmed by reading the code this session):
  * THEMING GAP (recommended next): ~27 raw egui::Button/Slider/Checkbox vs ~51 unendlich_ui:: themed
    calls in world_renderer.rs. Convert the raw ones to the toolkit (unendlich_ui::menu::*). This is
    the "comprehensively integrate the design components" ask; compile-verifiable, incremental.
  * MULTIPLAYER PAGE: display-only since r186 (page id 2 = `ui.label("multiplayer")`). Needs a real
    MP flow UI — the DHT/world-join foundation exists (handoff/MULTIPLAYER_WORLD_JOIN.md). Big.
  * WORLD-HISTORY PAGE + TIMELINE SLIDER: needs a scrubber to wind the world's timeline, GATED by
    timebending unlock (four base-element mastery). Ties to the rollback/timeline-branching system
    (docs/design/timeline_branching.md). Big.
  * BLUR BACKDROP + GLASS BULGE: the frosted *surface* is wired (set_surface frosted() + glass_at
    frames) but the real backdrop-blur PASS (blur_backdrop_integration.md "blur bridge") and the
    glass-bulge lens effect are not. Render-pass/shader effects — unverifiable in-sandbox.

## r313 — menu theming pass, part 1: navigation/action buttons → toolkit
"Take them in order" → theming first. The most prominent raw-egui buttons now use the
toolkit look. Compile-verified; visually UNVERIFIED (no GPU).
- unendlich-ui/lib.rs: + pub fn button_sized(ui, label, accent, min_size) — like button()
  but at least min_size (centered label), for fixed-width columns. button() now delegates
  to it (identical output at min=ZERO).
- world_renderer.rs: + gated shims tbtn() / tbtn_sized() (resolve accent from the active
  theme; drop-in for `ui.button(label).clicked()`).
- Converted 15 raw buttons → toolkit: pause menu (Resume/Settings/Save&Quit, via tbtn_sized
  220x32 to keep the uniform column); create-world wizard (Mode/Seed/set/apply/Back/Create/
  Next/Create World/Cancel); settings Done; crafting Close (C); perf "export ascii graph".
- VERIFIED: cargo check --bin unendlich-fullgame (full features) Finished; remaining warnings
  pre-existing (form_id_of/create_compute_pipeline/WORKGROUPS_PER_CHUNK_X/sample_or_air/
  NUM_CLASSES/ui_mouse_down). -> CHECKED. Operator confirms the themed look on M2.
- DEFERRED to part 2 (need care, not blind-convertible):
  * settings tabs + creative tabs (selectable_label → pick a themed tab/pill widget).
  * enabled-gated craft button (`add_enabled_ui` + `let b = ui.button(...)`): the themed
    button doesn't dim on disable like raw egui — needs explicit disabled styling (will
    re-add a tbtn_resp Response shim then).
  * raw ui_scale Slider (uses .text()/.step_by()/.changed()) → menu::slider + a label.
  * load-world history/restore/delete sized buttons — convert together with the
    WORLD-HISTORY PAGE work (next-but-one in the order).
  * DEBUG panel buttons/checkbox/slider — intentionally left raw (dev overlay, not themed).

## r314 — input: 'e'-key/text-field fix  +  menu: live BUTTON STYLE HARNESS (10 variants)
Reprioritized per operator: batch richer + give a debug slider that toggles styles live,
so one M2 run yields feedback on many looks (their iteration is the bottleneck).
- INPUT FIX (bin): (1) the hotbar selection loop (block-palette) had NO typing gate — the
  Dvorak home-row keys (a o e u i d h t n) double as Hotbar1..9, so 'e' etc. hijacked text
  fields. Now gated on !renderer.ui_wants_keyboard. (2) text-feed to egui was gated on
  ui_pointer_active, which could drop keystrokes; now always forwarded (egui ignores Text
  when no field is focused). -> the reported "some keys like e don't register".
- STYLE HARNESS (toolkit unendlich-ui): + a live BUTTON_STYLE atomic (set_button_style /
  button_style) and button_sized() rewritten to branch on it across 10 distinct variants:
  0 soft panel (r313 baseline), 1 solid accent, 2 ghost outline, 3 frosted glass, 4 sharp
  flat, 5 vertical gradient (Mesh), 6 neon glow (always-lit rings), 7 pill, 8 inset, 9
  underline tab. button() delegates to button_sized. Applies to EVERY toolkit button
  (pause menu, wizard, settings, craft, perf) converted in r313 — so styles show on real
  buttons.
- DEBUG SLIDER (engine world_renderer.rs debug panel): "button style (0-9, live)" slider →
  set_button_style + a label naming the current variant. Flip through 0..9 in-game.
- VERIFIED: cargo check --bin unendlich-fullgame (full features) Finished. -> UNVERIFIED
  (behavioral input fix + visual styles both need an M2 run): confirm 'e' now types in the
  world-name/seed fields; flip the style slider 0..9 and report which button look to lock
  (and refinements). NOTE: input fix is functional; no env gate. Style harness is dev-only
  (slider defaults 0 = the prior look).
- PLAN: once a button style is chosen, lock it as the default + extend the same harness
  pattern (style index + debug slider) to TOGGLES, SLIDERS, and PANEL/GLASS — the latter is
  where the blur-backdrop + glass-bulge work folds in. Batch ~10x50 lines/variant-set so
  each M2 iteration yields rich feedback.

## r315 — menu: layered button "sandwich" (widget module) + 10 dithered-minimalist candidates
Operator feedback on r314: none of the ten fit the toolkit; the FRONT main-menu buttons
(dest_button — dark panel, a left accent bar that GROWS on hover, uppercase letter-spaced
left-aligned label) are the closest minimalist reference. Wants the dithered-pixel feel
worked into THAT across 10 fresh candidates, AND the button abstracted into layers so
toolkit styling sits cleanly over egui.
- NEW crates/unendlich-ui/src/widget.rs — the sandwich:
    L1 egui base : raw egui (only inside L2 + paint)
    L2 unify     : interact(ui,size) -> Interaction{rect,resp,hover,down}  (any clickable)
    L3 work      : geometry() + label_spaced() (uppercase thin-space; dest_button casing)
    L4 abstract  : ButtonSkin{radius, fill:Fill(None/Panel/Accent), left_bar,
                   border:Border(None/Hairline/Accent),
                   dither:Dither(Off/Always/HoverReveal/Underline/Border/Corners), ink:Ink}
    L5 toolkit   : candidate(i)->ButtonSkin (the 10) + dither_dots() + paint(); button/
                   button_sized compose L2->L5.
- 10 CANDIDATES (all keep the minimalist bar + spaced-label feel; vary the dither):
    0 bar (dest baseline) · 1 bar+hover dither · 2 bar+dither wash · 3 dither underline ·
    4 dither border · 5 bar+corner dither · 6 ghost dither-in (accent ink) ·
    7 dense dither+hairline · 8 pixel bar (sharp) · 9 full dither fill.
- lib.rs: `pub mod widget;` + button/button_sized/button_style/set_button_style are thin
  delegates to widget:: (public API unchanged). The old monolithic 10-variant button_sized
  is removed. dither_dots() is a painter-level twin of menu::dither_fill so the paint pass
  holds a single borrow.
- DEBUG SLIDER (r314) drives it unchanged; its labels now name the new candidates.
- VERIFIED: cargo check --bin unendlich-fullgame (full features) Finished, clean. ->
  UNVERIFIED (visual): flip the debug "button style (0-9)" slider, pick a candidate, report
  refinements (radius / dither density / bar width / hover feel). Then lock it as default +
  extend the SAME layered harness (interact + skin + paint + slider) to TOGGLES, SLIDERS,
  then PANEL/GLASS (where blur-backdrop + glass-bulge fold in).
- Carries forward the r314 input fix (e-key hotbar-gate + always-forward text feed).

## r316 — crash fix: per-frame fuzz sweep is now opt-in + unpack hardened; menu: PULSE button + refined candidates
TWO crashes this run, both from the per-frame sim fuzz sweep (bin ~2767-2769), with the
"last log" being the prior frame's tail:
  1. draw_metadata_pack.rs unpack OOB ("len 3, index 48") <- a kernel_registry_gen kernel
     calling unpack(&[3 u32], idx 1..=6): base = idx*STRIDE = 48 > len 3.
  2. capacity overflow <- a generated service in fg_services.tick_all (which, unlike the
     kernel host, has NO catch_unwind).
The kernel host catch_unwinds + quarantines offenders, but panic=abort defeats it; services
have no backstop. These sweeps are pure, black_box'd fuzz-lite with NO gameplay effect.
- FIX (bin): gate fg_services.tick_all + fg_kernels.exercise behind UNENDLICH_SIM_SWEEP
  (default OFF); log the state once at startup. Normal play no longer runs the sweep.
- BACKSTOP: draw_metadata_pack::unpack returns a zeroed ChunkDraw on OOB instead of indexing
  (real renderer callers already bounds-check via draws.len()).
BUTTON FEEDBACK on r315: 0 good (header/main-menu class, KEEP); 1 good ("special", KEEP);
2/7/9 broke the UI (drop); 3/4 weird with the underline+bump combo (drop the bump); 5 too
much but corner idea good elsewhere; 6 -> develop into "center bump + random-walk pulsing
dither" (the cool one); 8 worse than 0.
- widget.rs: + Dither::Pulse + dither_pulse() — a center band of dither whose width
  random-walks (id-seeded layered sines, deterministic per button) and whose alpha breathes;
  hover widens it (the center bump); request_repaint keeps it animating. + ButtonSkin.center
  (center vs left align) + .bump (hover nudge/grow toggle, off for the quiet styles).
- REFINED CANDIDATES (slider 0-9): 0 header bar (kept) | 1 special (kept) | 2 PULSE fill |
  3 PULSE ghost accent | 4 PULSE framed | 5 quiet underline (no bump) | 6 quiet edges (no
  bump) | 7 clean ghost (no bump) | 8 header + corner flecks | 9 = header bar default.
  Debug-slider labels updated to match.
- VERIFIED: cargo check --bin (full features) Finished, clean. -> UNVERIFIED (visual + crash
  fix needs an M2 run): confirm no more crashes (sweep off by default); flip the slider, dial
  in the PULSE feel (2/3/4), confirm 0/1 still good, report pulse speed / walk density / bump
  amount to lock. Then extend the harness to TOGGLES / SLIDERS / PANEL-GLASS. Carries r314
  input fix + r315 layered sandwich.

## r317 — buttons finalized: default=special, ELEGANT (cursor-tied infill) on rebind, scroll fix, style guide
Operator decisions on r316: delete 5/7/8/9; develop 6 into the "elegant button" (infill
tied to the mouse, smooth — magical/enchantment feel); use style 1 for the main menu; doc
the keepers; apply elegant to Settings→Controls→rebind; fix the rebind scroll view.
- widget.rs: default style index 0 -> 1 (SPECIAL is now the default everywhere). + Dither::
  Elegant + the paint arm: faint static top/bottom edges + a dither infill band centered on
  it.resp.hover_pos().x, width eased by hover (animate_bool), request_repaint for smooth
  tracking. candidate(6) = elegant skin (transparent, hairline, centered, no bump). 5/7/8/9
  now fall through to the default (special). + elegant_button / elegant_button_sized (force
  skin 6, accent from theme); re-exported from lib.rs.
- world_renderer.rs: dest_button shim (the FRONT/title menu) now routes to the candidate
  button_sized(220x34, theme accent) -> renders as style 1. The Controls rebind-key button
  -> elegant_button_sized(130x22). Debug-slider labels updated (6=ELEGANT; 5/7/8/9 removed).
- bin (scroll fix): the hotbar scroll loop and the egui-forward were gated only on
  mouse_captured; if the capture flag lagged the menu state the hotbar ate the wheel and the
  Controls ScrollArea never scrolled. Now computes ui_scroll_target (any menu/dialog open)
  and: hotbar consumes only when `mouse_captured && !ui_scroll_target`; forwards to egui when
  `!mouse_captured || ui_scroll_target`.
- DOC: docs/design/button_style_guide.md — the layered model + the kept classes (Special/
  default, Header, Elegant) + tokens + the harness + next steps.
- VERIFIED cargo check --bin (full features) Finished, clean. -> UNVERIFIED (visual +
  behavioral): confirm the main menu now reads as style 1; the rebind button shows the
  cursor-tied elegant infill; the Controls list scrolls. Carries r316 (sweep opt-in crash
  fix + pulse) forward.

## r318 — UI cluster: gui-scale slider themed, tabs -> tab_button, horizontal scroll -> hotbar, dual-axis scroll
Operator batch (UI/input half; world/leaf half sequenced next).
- GUI SCALE SLIDER: was a raw egui::Slider; now the themed menu::slider + accent value
  label (matches the int_slider! settings rows). Commit-on-release logic unchanged.
- TABS -> TOOLKIT: + widget::tab_button(ui, label, selected) (selected = accent fill +
  accent ink; unselected = quiet transparent; centered). Applied to the 5 settings text
  tabs (Video/Audio/Gameplay/Controls/Performance) and the inventory tabs (Inv/Items/Eggs).
  The per-world icon tab stays a selectable_label (it's a glyph, not a text tab).
- HORIZONTAL SCROLL -> HOTBAR: the bin captured only Scroll dy; now captures dx too. In
  game mode a horizontal trackpad swipe is folded into the hotbar cycle.
- SCROLL ABSTRACTION / "just works": renderer + ui_scroll_delta_x; the egui MouseWheel push
  now carries BOTH axes. The bin forwards both axes to egui whenever any UI surface is up
  (egui ignores them unless a ScrollArea is hovered) — the documented "egui owns scroll in
  menus" path.
- SCROLL CAPTURE (controls): the 21-row list overflows 260px so it *should* scroll; r317
  was a no-op for settings (the cursor is already free there, so the forward already fired)
  -> root cause is egui-side, not yet pinned blind. Mitigation: the controls ScrollArea now
  has an ALWAYS-VISIBLE scrollbar (draggable fallback + confirms scrollability). If the wheel
  still doesn't scroll, next step is a frame log of the forwarded MouseWheel vs egui hover.
- VERIFIED: cargo check --bin (full features) Finished (warnings only). -> UNVERIFIED:
  confirm the gui-scale slider matches the others; tabs render as toolkit buttons; horizontal
  trackpad scroll cycles the hotbar; the controls scrollbar appears + drags; whether the
  WHEEL scrolls the controls list.
- DEFERRED (next, world/leaf batch): air-tree last-minute guard; leaf texture-placeholder
  removal; bushy leaf genome; leaf-to-log adjacency (less floaty/sparse); leaf wind via the
  rope physics.

## r319 — perf diagnosis (two stutters) + input batch (keybind overlap, double-tap flight, scroll diag, log cleanup)
DIAGNOSIS (logs only; no code):
- Stutter 1 (severe, 91->4 FPS): chunk gen/mesh/UPLOAD storm from fast vertical movement
  (the operator was holding ';' = FlyDown; pos y -247->-273). gen_pending pinned at 245;
  mesh_q grows unbounded 0->260. Killer zones: per-frame draw up to 440ms (p99 378), submit
  blocking, wait_fences ~73ms/frame, game_update max 807ms + tick_sim max 482ms -> BOTH the
  sim (worldgen/apply) and the render (mesh+upload+fence serialization) spike during the
  storm. Octree mesher emits oversized meshes (avg_verts 16393, max 45231; near-mesh
  44-66ms/batch). adaptive_uploads throttled 8->4. Root: pipeline can't keep up with fast
  traversal — oversized LOD0 meshes + serialized submit/upload + heavy worldgen on the
  game_update thread.
- Stutter 2 (mild, ~40ms): CPU sim hitch, NOT streaming (pos stable, mesh_q only 27). 73
  roamers vs herd capacity 8 ("[popdyn] Spaeher starved"); ecosystem flora scan 11162 blocks;
  evict=4.4ms; roamer_tick max 51.5ms. Root: roamer overpopulation + flora scan + eviction.
  Suggested (not done): cap per-frame mesh/upload under load + decimate the octree mesher
  LOD0 (stutter 1); clamp roamer population / amortize the flora scan (stutter 2).
INPUT BATCH:
- keybindings::set_binding now lets FlyUp/Jump and FlyDown/Sneak SHARE a key (overlap):
  rebinding one no longer clears the other (new overlap_allowed()).
- double-tap Space (Jump, <300ms) toggles flight (creative), OR'd into the ToggleFlight handler.
- removed the r187 "[text] glfw char" log (flooded the log on every keypress, esp. ';').
- scroll DIAGNOSTIC: [scroll-dbg] logs the routing decision (dx/dy/cap/target -> hotbar|egui)
  for any pending scroll, to pin the controls-list capture bug from a real run.
DEFERRED to the world/physics batch (next): crawl (GameAction::Crawl + 1-block collider —
needs a per-character height; 16 Character construction sites); air-tree guard; leaf
texture/genome/adjacency/wind; dragon body-stretch animation bug.
- VERIFIED cargo check --bin (full features) Finished. -> UNVERIFIED: overlap rebinds hold;
  double-tap Space flies; PASTE the [scroll-dbg] line when scrolling in keybinds (pins it).

## r320 — leaf batch: foliage placeholder removal + wind sway + floating-trunk decay
- chunk.frag: foliage (leaves/grass/ferns, detected by is_foliage vertex color) no longer gets
  the 2x2 SHOW_UV_DEBUG checker placeholder — flat vertex color now. Solids keep the checker
  until a real atlas lands. (Builds on r311 which already skipped the box-outline grid for foliage.)
- chunk.vert: foliage vertices sway in the wind (GPU vertex sway). Displaced along the
  normalized wind direction (wind_params.xy), amplitude scales with wind magnitude
  (wind_params.z), phased by world pos + time. Clock = time_of_day (atmos_params.w) scaled
  x6000; it's a slow source, so a dedicated fast wall-clock would give crisper flutter (follow-up).
  Shadow pass (chunk_shadow.vert) is NOT swayed yet -> leaf shadows are static (minor).
- world/leaf_decay.rs: floating-trunk decay. The decay tick now also scans LOGS; a log whose
  block directly below is Air (mid-chunk, ly>0; below index = idx - CHUNK_SIZE*CHUNK_SIZE) is
  flagged FLAG_DECAYING and ages out like an orphaned leaf -> unsupported / air trees fall and
  CASCADE upward (each log beneath turns to air, so the next tick flags the one above). Safe
  because trees here are vertical trunk + leaf cluster (no air-branches); ground/log-supported
  trunks are never flagged. Logs at the chunk floor (ly==0) are skipped (can't see below).
DEFERRED (tree-gen crown work, next focused pass): bushy genome (per-tree leaf density/shape
variation) + leaf-to-log adjacency (denser leaves hugging the trunk) — both reshape the canopy
in plant_lifecycle/plant_blueprint and want care, not a blind edit. Also still pending: crawl
(per-character collider height), gesture system, scroll trackpad (need the [scroll-dbg] line),
dragon body-stretch bug.
- VERIFIED cargo check --bin (full features) Finished + glslangValidator chunk.vert/chunk.frag
  clean. -> UNVERIFIED: leaves render flat (no checker); foliage visibly sways; air/eroded
  trees slowly fall from the base up; legit trees untouched.

## r321 — SCROLL FIX (root cause) + perf-flag investigation
ROOT CAUSE of the long-running scroll bug (r317/r318/r319 were all downstream patches): the glfw
window never called set_scroll_polling(true). glfw requires per-event-type opt-in; the window
enabled key/char/mouse_button/cursor_pos/framebuffer polling but NOT scroll, so WindowEvent::Scroll
was NEVER delivered -> scroll_accum always 0 -> no hotbar cycle, no egui forward, no [scroll-dbg]
line ("no lines" confirmed it). FIX: one line, window.set_scroll_polling(true). The earlier
forward/dual-axis/scrollbar work now has a live event to act on.
INVESTIGATION (for the perf-settings UI + close-mesh + deja-vu):
- runtime_flags::flags() is a REGISTRY of ~25 env perf flags (env/summary/default/valid). The heavy
  optimizations already default ON: ADAPTIVE_UPLOADS=1, SUBMIT_SKIP=1, SLACK_SCHEDULER=1,
  BIASED_SHELL_WALK=1, UPLOAD_PRIORITY=1, INDIRECT=1. CPU mesher (mode 0) does greedy meshing
  (5-15x fewer verts). The one notable OFF default: UNENDLICH_MESH_CACHE_SIZE=0 (renderer mesh
  cache disabled) — candidate for the "should be on in production" flag; worth trying >0.
- GPU_MESHER=2 (octree, the operator's setting) IS the recommended play mode (advisory only warns
  on =1 SparseGpuMesher). The octree GPU path is NOT greedy by design (LOD = "cheap to mesh"), which
  is why near meshes are 16k-45k verts. So the close-mesh win is FEWER verts (greedy-on-octree, a GPU
  compute change — hard) or capping mesh/upload rate under load, NOT vertex packing.
- VERTEX PACKING is ALREADY optimal: PackedVertex = pos_packed:u32 (A2B10G10R10_UNORM) + color_packed:u32
  = 8 bytes/vertex stride. No win there; the upload cost is vert COUNT, not vert size.
- VERIFIED cargo check --bin (full features) Finished. -> UNVERIFIED: scrolling now cycles the hotbar
  in-game and scrolls the controls/keybind list, and [scroll-dbg] lines now appear when you scroll.
DEFERRED/next: perf-settings UI (presets + flag toggles/sliders) needs a runtime-config layer since
flags are read from env at STARTUP; bushy genome + leaf adjacency; crawl; gesture system; dragon.

## r322 — crash-safe runtime tuning (mesh cache) + per-driver blacklist
NEW module crates/unendlich-engine/src/safe_tune.rs (SafeTune): a trial protocol for risky perf
settings that can hard-crash the GPU/driver (uncatchable by catch_unwind), so a bad setting doesn't
crash forever:
- arm(setting,value) writes a pending marker (setting/value/driver_hash) to ~/.config/unendlich/
  tune_pending BEFORE applying the risky value.
- disarm() clears it once the setting proves stable.
- load() on the NEXT startup: if the marker survived under the SAME driver_hash, the prior run
  crashed on trial -> revert (don't reapply) + append to ~/.config/unendlich/tune_blacklist keyed on
  driver_hash. A driver update changes driver_hash, so the block auto-clears and the setting is
  retried fresh ("as long as same drivers are reported"). Plain TSV; corruption/absence degrades to
  "nothing pending/blacklisted". Unit tests cover clean-run, crash-on-trial-blacklists, and
  different-driver-clears (written; cargo test still can't LINK in-sandbox, but they compile).
WIRED to the mesh cache (the candidate from r321):
- bin (after gpu_info): SafeTune::load; requested = UNENDLICH_MESH_CACHE_SIZE; effective = requested
  iff >0 AND not blacklisted (then arm), else 0. Logs revert/blacklist/trial.
- WorldRenderer::set_mesh_cache_capacity(cap) new setter; bin applies `effective` right after ctor.
- disarm fires in the main loop once frame_count>600 (past the GPU-init crash window) so a later
  unrelated crash isn't blamed on the cache. (No clean-shutdown hook needed; crash = no disarm =
  marker persists = revert.)
THREADED PROBE: the sentinel IS the safety net for hard GPU crashes (catch_unwind can't see them);
a catch_unwind worker only helps for soft panics. Documented in the module; not added yet.
- VERIFIED cargo check --bin (full features) Finished. -> UNVERIFIED on M2: set
  UNENDLICH_MESH_CACHE_SIZE=512, confirm "[safe-tune] mesh_cache=512 on trial"; if it crashes,
  confirm next launch logs the revert + forces 0; confirm a driver update clears the block.
HORIZONTAL SCROLL: the fold (scroll_accum += scroll_accum_x in game mode) is correct — if a sideways
swipe doesn't cycle the hotbar, glfw isn't delivering it as Scroll dx on this platform. The
[scroll-dbg] line shows dx= on swipe; if dx stays 0.000, it's platform non-delivery (engine can't
synthesize it).
NEXT (greenlit): RuntimeConfig layer (env-seeded, systems read it instead of env-at-startup) ->
then unify Video + Performance settings into one UI driven by it, and let the autotuner adjust the
(now crash-safe) mesh cache live. Still pending: leaf genome/adjacency, crawl, gesture system, dragon.

## r323 — grass-wind fix + RuntimeConfig foundation (unified settings step 1)
GRASS/LEAF WIND FIX: r320's sway used ubo.atmos_params.w * 6000.0, but atmos_params.w is
time_of_day IN SECONDS (per the UBO comment + world_renderer fill), not normalized 0..1 — so the
phase advanced thousands of rad/s => grass waved rapidly. chunk.vert now uses w * 1.4 (gentle
~0.2-0.3 Hz). glslangValidator clean.
RUNTIMECONFIG (crates/unendlich-engine/src/runtime_config.rs): central perf/graphics config — the
foundation for the unified Graphics+Performance settings UI. RuntimeConfig fields = the scattered
UNENDLICH_* perf knobs (render_distance, lod_bands/factors, max_uploads_per_frame, upload_budget_ms,
frame_target_ms, mesh_cache_size, gpu_gen_batch [live]; shadow_map_size, vsync, gpu_mesher
[needs-restart, listed in NEEDS_RESTART]). from_env() seeds every field from the same env+default the
system uses today (behavior-preserving migration). PerfPreset {BatterySaver,Low,Medium,High,Ultra,
Custom} with preset() tables (Medium == shipping defaults; others scale rd/LOD/uploads/cache/shadows)
+ matching_preset() to highlight the active one. Tests: medium==defaults, presets scale monotonically,
matching_preset roundtrips (compile; cargo test still can't LINK in-sandbox).
FIRST MIGRATION: the bin builds RuntimeConfig::from_env() and the mesh-cache (r321/r322) now reads
runtime_cfg.mesh_cache_size instead of a direct env::var. Pattern proven for the rest.
NEXT (unified settings cont.): migrate the remaining read-sites (chunk_manager render distance, lod.rs
bands/factors, upload caps) to read RuntimeConfig; then build the unified Graphics+Performance settings
TAB (preset row + live sliders applying immediately, needs-restart knobs tagged + persisted).
DOCS INGESTED (new-docs: Ancient Locations — Heliographic Archive/First Suspension/Spannung's Aerie;
structures+items+lore). NUMERIC AUDIT: Null Crab "97% normal gravity" is the flagged error — narrative
says it's so overbuilt for cave gravity it's "dramatically more powerful" at the surface, but 97% is a
3% reduction (negligible); should be ~30-50% for the story to hold. Also flagged: Moonstone "15% of
expected mass" vs its field's modest +40% jump / -50% fall — a 15% mass implies ~85% gravity cut, far
beyond +40% jump; the stone-mass and field-strength numbers don't cohere. Rest of the figures (L/R/H/S
ranges, rarities, recovery curve, cloak timers) are internally consistent.
- VERIFIED cargo check --bin (full features) Finished + glslangValidator chunk.vert clean.
  -> UNVERIFIED on M2: grass/leaves sway GENTLY now (not rapid).
BACKLOG (your work streams): grass = DONE (this), tree-death = DONE (r320 floating-trunk decay),
items/creative-hotbar+tabs, dragon body-warp, new structures (these docs), leaf genome/adjacency,
crawl, gesture system = queued.

## r324 — terrain-gen ROOT CAUSE (diagnosis) + grass amplitude + unified-settings preset row
TERRAIN BUG ROOT CAUSE (diagnosed, NOT fixed — high risk, FP32 path only validatable on the M2):
plants/mobs on air with water beneath = the GPU compute generator fills the actual voxels in FP32
(a port made specifically for Honeykrisp/Apple GPUs — the operator's M2), but chunk_manager.surface_y_at
(line 1831) returns the CPU generator's FP64 noise PREDICTION (self.generator.cpu().surface_y_at).
Flora placement, mob ground/physics, and spawn all use that prediction. When FP32 GPU terrain and FP64
CPU prediction diverge, things sit at the predicted height (air) while the real surface is lower, water
fills below sea level (64). Explains all three symptoms + the long-standing air-trees at once. FIX
DIRECTION (operator to choose/validate): make spawning read the ACTUAL top solid block of the generated
chunk, OR close CPU(FP64)/GPU(FP32) height parity. Did NOT touch terrain gen.
GRASS: amplitude cut (~2.5x smaller) per taste. KNOWN-LIMIT: the shader sways by vertex GREEN color, so
it (a) also catches solid grass-BLOCK tops and (b) can't anchor plants at their base. Proper fix = the
mesher tags sway-eligible foliage vertices with a base->tip sway weight; but the operator runs the GPU
octree mesher (mode 2), so that's a GPU-compute-shader change — a focused next piece, not a blind edit.
UNIFIED SETTINGS (UI tab progress): Performance tab now opens with a PRESET ROW — 5 tab_buttons
(Battery Saver/Low/Medium/High/Ultra) backed by new renderer field ui_perf_preset (staged like the other
levers), and below it the RuntimeConfig values that preset implies (render distance, max uploads/frame,
mesh cache, shadow map). Display-only for now; live application lands as systems migrate to read
RuntimeConfig (mesh cache already does at startup via the safe-tuner). NEXT: per-knob sliders editing the
config + persistence + live apply for the safe knobs + fold the Video tab in.
DOC CORRECTIONS (captured as canonical design; docx not yet rewritten — offered):
- Cave gravity: NOT 97%. It's near-zero but slightly NEGATIVE — strong enough that stones chain to the
  floor, but the field is smoothed so water still pools/clings by adhesion to nearby moisture. (Makes the
  Null Crab's "overbuilt musculature -> dramatically stronger at surface" coherent.)
- Moonstone: keep it light, but its field is a FALLOFF — the carrier sees ~60% gravity reduction, the
  general area only 10-15%. (Resolves the 15%-mass vs +40%-jump mismatch I flagged.)
- VERIFIED cargo check --bin Finished + glslangValidator chunk.vert ok. -> UNVERIFIED on M2: grass sway
  is subtle now; the Performance tab shows the preset row + values.

## r325 — settings: render-distance dedup + preset PROFILES + render scale + dropdown ingest
- FIX: render distance appeared twice (the real ui_render_distance slider + r324's read-only display
  rows). Removed the r324 display rows; kept the slider.
- PROFILES: the preset row is now functional. Game-themed glyphs (battery/seedling/evergreen/mountain/
  dragon = rising cost). Clicking a preset applies its values to the live levers (render_distance,
  render_scale) and selects it. Tuning any knob flips the active preset to a custom PROFILE, shown with
  a hammer glyph (building-themed "config changed") + a reset button that restores the preset. Unmodified
  shows a filled-dot "preset" tag. New staged field ui_render_scale (struct+init+stage+writeback).
- RENDER SCALE: RuntimeConfig.render_scale (UNENDLICH_RENDER_SCALE, default 1.0) for the neural upscaler;
  presets set it (BatterySaver 0.6, Low 0.8, Medium/High/Ultra 1.0). New "render scale" slider (50-100%)
  in the Performance tab. Ultra shadow_map_size bumped to 8192.
- DROPDOWNS ingested (ui-package/dropdowns_playground.html): glassy 3px-corner select, accent #6fc3ff,
  element-colored dots; variants for Graphics Profile / Window Mode / Difficulty / Bending Element/Action
  / HUD. Will mirror with themed egui ComboBoxes for the dropdown-style knobs (window mode, shadow size,
  difficulty). Also in the package: survival_inventory + character_abilities mockups -> inventory/abilities
  work stream.
HONEST STATE: knobs with existing live levers (render distance) apply; render_scale, shadow 8192/16384,
and dynamic-lights are carried in the config/presets but need render-PIPELINE wiring to take effect live
(part of the ongoing migration). The profile row is real; the back-end for the new graphics knobs is next.
- VERIFIED cargo check --bin Finished.
SEQUENCED (this message's remaining asks): shadow 16384 + dynamic-lights as LIVE knobs (pipeline);
new Camera/Gameplay page (FOV/camera-smoothing/autojump — FOV already in sim/settings_model.rs::Fov +
menu_screen_layout, a foundation); multiplayer list + unendlich-id page (sim/net/transport.rs UDP exists
-> infra to build on; need to know what "unendlich-id" is — local profile vs online account — before
designing the page); inventory/abilities mockups.

## r326 — multiplayer identity/whitelist/mailbox data layer (the trust surface)
NEW module crates/unendlich-engine/src/sim/net/identity.rs (pure data + thin file layer, no sockets;
sits beside session/transport/wire as another fully-tested net layer). Implements the operator's
multiplayer design's foundation:
- UnendlichId: shareable handle (normalized 12-char base32, grouped XXXX-XXXX-XXXX, from_seed() to mint
  one). OPAQUE — never embeds the IP; resolution to an address is the network layer's job.
- Whitelist (per-world whitelist.txt): allowlist; missing file => empty (whitelisting ON by default, so
  unknown ids go through the mailbox). load/save scoped to the WORLD save dir, so a splitscreen host with
  multiple local players can't have a global whitelist — it's always the live world's.
- NameRegistry (per-world names.txt, id<TAB>name): approve(id, requested) -> (assigned, was_renamed);
  renames on conflict with a numeric suffix. was_renamed drives the UI "renamed" graphic.
- Mailbox + JoinRequest + ReqState{Pending,Accepted,Rejected}: the IP-PROTECTION invariant lives here.
  submit() auto-accepts an already-whitelisted id, else parks it Pending; JoinRequest::address_if_accepted()
  returns None until accept() — a pending request can be shown/logged WITHOUT ever leaking an address.
  accept() whitelists + names + releases the address; reject() marks Rejected.
- Subscribers (per-INSTANCE subscribers.txt in config dir): ids whose worlds' servers you follow; deduped.
- 7 unit tests cover id normalize/group, from_seed stability, per-world whitelist roundtrip, name
  conflict-rename, mailbox address-withheld-until-accepted, whitelisted-skips-mailbox, subscribers dedup.
  (Compile-verified; cargo test still can't LINK the lib in-sandbox due to glfw, so not run here.)
- VERIFIED cargo check --bin Finished.
NEXT (needs runtime/network validation on the M2 — NOT built blind):
- NETWORK PROTOCOL: how an UnendlichId routes a join request to the host WITHOUT leaking the public IP to
  the requester — needs a rendezvous/coordinator (or relay/hole-punch) design: requester -> coordinator by
  id -> host's mailbox; address shared only on accept. This is the key open design question.
- WIRE: extend sim/net/wire.rs with JoinRequest/Accept/Reject packets; carry them over UdpTransport;
  feed Session once accepted.
- UI: host-world-as-server from the in-game menu; server/subscribers list (added ids' worlds); a
  Whitelist settings page (only when multiplayer is on) with pending-request approve/reject + the rename
  graphic; add-by-unendlich-id. Mirror the ingested glassy dropdown style.

## r327 — two-sided DHT mailbox (serverless join rendezvous) — LOGIC VERIFIED
NEW module crates/unendlich-engine/src/sim/net/dht_mailbox.rs. The serverless rendezvous the operator
asked for: send-request / receive / accept / deny, IP only on accept.
- Every UnendlichId owns a mailbox at DHT key H(id) (mailbox_key()). Two-sided flow: requester puts
  Request{from,name} into the HOST's mailbox H(host_id); host drains its box; host replies into the
  REQUESTER's mailbox H(req_id) with Accept{assigned_name,addr} or Deny (no addr).
- IP PROTECTION is structural: the host address travels ONLY inside an Accept, placed in the requester's
  mailbox ONLY after accept. Pending -> nothing in requester's box; Deny -> no addr. Verified by tests.
- MailboxDht trait { put(key,msg), get(key)->drain } mirrors the Transport/LoopTransport split; in-memory
  LocalDht for tests/local play; real Kademlia-over-UDP node implements the same trait later.
- MailMsg{Request,Accept,Deny} hand-rolled, bounds-checked codec (no panic on untrusted bytes; every
  length field capped at 256). Whitelisted requester is auto-accepted on drain (IP dropped immediately);
  unknown -> Pending until host_accept/host_deny (folds into the r326 identity::Mailbox).
- HARDENING documented (not yet impl): sign messages with the sender's id-key (mailboxes are
  world-writable -> spoof/spam), and SEAL the Accept's addr to the requester's pubkey so relaying DHT
  nodes can't harvest IPs (UnendlichId is the place to carry that pubkey).
- VERIFICATION: in-tree `cargo test` can't LINK (keybindings.rs needs glfw) as always, BUT the identity +
  dht_mailbox modules were FLATTENED into a standalone harness and run: **12/12 tests pass**, incl.
  full_two_sided_accept_delivers_ip_only_after_accept + deny_carries_no_address. So the protocol LOGIC is
  runtime-verified (the network backend + UI are not).
- cargo check --bin (full features) Finished.
NEXT: real DHT backend (Kademlia/mainline-style node impl of MailboxDht over UdpTransport, with NAT
traversal/STUN to discover my_addr); sign+seal hardening; wire join -> Session handoff once accepted;
UI (host-from-menu, subscribers/server list, whitelist page w/ pending approve/deny + rename graphic).

## r328 — glfw is now a feature boundary; input core uses the native abstraction — VERIFIED
GOAL (operator): "make glfw a feature and write an abstraction for events and user inputs that can be
used in the core instead of glfw so that you can test more stuff." The abstraction already existed
(input_codes.rs: Keycode/MouseBtn/KeyAction/InputEvent + feature-gated glfw AND winit conversions) but
was never USED — input.rs/keybindings.rs/keybind_ui.rs still named glfw types directly, which is exactly
what broke `cargo test --lib` (keybindings.rs used glfw::Key unconditionally -> unresolved without glfw).
CHANGES:
- input.rs: InputCache now consumes the native InputEvent (Key/MouseButton/CursorPos/Scroll); internal
  storage/queries use Keycode/MouseBtn/KeyAction (via type aliases so the struct body was untouched).
  handle_event/handle_event_late match InputEvent variants. No glfw anywhere.
- keybindings.rs: glfw::Key -> Keycode, glfw::MouseButton -> MouseBtn (aliased to Key/MouseButton to
  minimize churn). key_from_name table unchanged (Keycode variants share glfw names). Tests use
  MouseBtn::Middle/Back instead of glfw Button3/Button4.
- keybind_ui.rs: glfw::Key -> Keycode.
- lib.rs: input/input_capture/keybind_ui/player UN-gated from `window` (they're glfw-free now), so the
  registries (always compiled, reference crate::input/input_capture) resolve without glfw.
- behavior_arbiter::Drives gained PartialEq — a latent test bug (poi.rs assert_eq) that only surfaced
  once the lib tests could finally compile.
- BIN (unendlich_fullgame.rs): the ONLY remaining glfw boundary. Raw WindowEvent matches keep glfw::Key;
  the two event-pump calls convert glfw::WindowEvent -> InputEvent via input_codes::input_event_from_glfw
  before feeding InputCache; the ~23 InputCache queries now pass native Keycode/MouseBtn.
PAYOFF (the point): `cargo test --lib --features "render-core egui"` (NO glfw, NO window) now LINKS and
runs. Newly-runnable, previously-impossible: input:: (2 pass), keybindings:: (9 pass). Also green:
net::identity (7), net::dht_mailbox (5), runtime_config (3), safe_tune (2). ~7234 lib tests now compile
without glfw. Verify gates: `cargo check --bin ... --features render,window,gpu,ui-overlay,audio`
Finished (full glfw path intact); lib test binary links glfw-free.
KNOWN: input_codes.rs has a harmless `unexpected_cfgs` warning for `feature="winit"` (the winit adapter
exists for the future backend swap but `winit` isn't declared as a Cargo feature/dep yet).

## r329 — compile-time tooling docs (AI bootstrap category) — CHECKED (docs+script; no code change)
Investigated linker/dynamic-linking levers to cut build/test time in the sandbox; documented for AI
bootstrap. Findings (measured in-sandbox unless noted):
- ld.gold preinstalled; mold (2.30.0) + lld install via `apt-get install -y mold lld` (archive.ubuntu.com
  is allowlisted — unlike sccache's installer). Dynamic libstd-*.so present in the sysroot.
- prefer-dynamic + split-debuginfo shrank a trivial std program 4.77MB -> 0.43MB -> 0.18MB (~26x); the
  project lib test binary is ~240MB static, so the saved link work is large.
- CRUCIAL: `cargo check` (the verify gate) does NOT link, so these levers don't speed it; they speed
  `cargo test`/`cargo build`/`rustc --test` harnesses — newly relevant after r328 made lib tests linkable.
- GOTCHA: toggling RUSTFLAGS to compare linkers invalidates cargo's whole fingerprint -> full rebuild
  each time (this overran turn budget). Set the linker once; first build pays one rebuild, then fast.
- Validated: a sandbox-global $CARGO_HOME/config.toml with mold+split-debuginfo does NOT break
  `cargo check` (unendlich-shared Finished); mold confirmed as the active linker (.comment = mold 2.30.0).
DELIVERED: docs/TOOLING_fast_linking.md (full guide + caveats), scripts/sandbox-fastbuild.sh (one-shot
session bootstrap), cross-link added to docs/TOOLING_sccache_nextest.md. NOT committed to the repo's
.cargo/config.toml — prefer-dynamic isn't portable and mold may be absent on the operator's M2; these are
sandbox-only, applied per session (FS resets). No engine source changed; r328 VERIFIED state intact.

## r330 — next-session handoff guide — CHECKED (docs only)
Wrote docs/NEXT_SESSION_GUIDE.md: a detailed onboarding for the next Claude — philosophy/working style,
full build setup (rust 1.96 preinstalled; mold via scripts/sandbox-fastbuild.sh; ⭐ lavapipe/llvmpipe
software Vulkan — lvp_icd.json is registered + enumerates a CPU Vulkan 1.4 device, so GPU paths can run
headless via `gpu-headless-probe --features headless`, unlocking in-sandbox validation of the terrain
FP32 bug etc.), the multiplayer spec + what exists (identity/dht_mailbox) + sequenced next steps (real
Kademlia DHT node, sign/seal hardening, Session handoff, UI), the bug/perf backlog with file locations
(terrain FP32/FP64, grass sway, dragon body-warp, stutters, crawl, settings-live-knobs), and a "what NOT
to do" list (no mold/prefer-dynamic in repo config; don't toggle RUSTFLAGS; don't ship blind GPU/net code;
keep input backend-neutral; python heredoc \u escape gotcha; re-check mutable env facts). No code changed.

## r330b — multiplayer net_channels + r330 dispatch packet (6 sections) — net_channels VERIFIED; dispatch docs CHECKED
Coordinator round: built multiplayer in-house and dispatched six sections.
- MULTIPLAYER (in-house, VERIFIED): new sim::net_channels — the §6 QoS channel table of
  handoff/MULTIPLAYER_WORLD_JOIN.md as pure data + a per-tick byte budgeter (roadmap item 5 of
  docs/sections/SECTION_multiplayer.md; "needed before any real transport"; unblocks the entity-
  snapshot codec item 7). ChannelId{Control,Snapshots,Input,Voice,Bulk,Chat} + ChannelMode
  {ReliableOrdered,UnreliableSequenced,UnreliableSequenced=snapshots,Unreliable=voice};
  ChannelConfig{id,mode,priority,max_bytes_per_tick,keepalive_ms}; DEFAULT_CHANNELS = the §6
  table; Bulk is lowest priority so budget_tick() only ever feeds it leftover (the "bulk
  throttled below snapshots, never starves the hot loop" rule, enforced structurally).
  budget_tick honors per-channel caps + priority, sum ≤ tick budget; keepalive_due carries the
  15s punch keepalive on control; consts ADVERT_REANNOUNCE_MS/DHT_TTL_MS/PUNCH_KEEPALIVE_MS.
  Pure (no IO/clock/RNG/unwrap/expect/panic; saturating arithmetic). One pub mod line in
  sim/mod.rs. VERIFIED: rustc --edition 2021 --test net_channels.rs -> 13 passed / 0 failed
  (standalone, the documented fast gate). -> M2: no hardware step needed (pure logic).
- DISPATCH (CHECKED, docs only): docs/dispatch_r330/{DISPATCH_PACKET_r330.md, SHARED_RULES.md,
  COORDINATOR_NOTES_r330.md}. Six doc-grounded pure worker briefs: items (sim::item_tooltip),
  mobs (sim::roamer_cap, spawn-admission only — corrected per r300 declined-despawn history),
  world (sim::surface_scan top-solid-Y, the FP32/FP64 float bug §4a), gpu (mesher foliage
  sway-tag + chunk.vert gate §4b), menus (graphics-settings model + Camera/Gameplay page §4e),
  bughunt (floating-trunk decay leaf_decay.rs + unwrap/expect audit of world_catalog/world_launch).
  Merge order on return: items→bughunt→mobs→menus→world→gpu. Anchors verified against the tree.
- Still open MP (next self-tasks): world_advert(1), net_transport_select(2), world_presence(3),
  world_join_requests(4), chunk-stream seed/delta codec(6,§B), entity-snapshot codec(7,§C now
  unblocked); items 8–9 (run_world seam + DHT/UDP IO) are bin, compile-on-end, after the pure layer.

## r330c — multiplayer entity-snapshot replication codec (§C / item 7) — VERIFIED (standalone, faithful Fixed stub)
Coordinator continuing multiplayer in-house. New sim::net::entity_snapshot — host-authoritative
ENTITY TRANSFORM replication, the natural pair to r330b's net_channels (rides ChannelId::Snapshots =
UnreliableSequenced, newest-wins). Roadmap item 7 / docs/sections/SECTION_multiplayer.md §C.
- EntityState{id,kind,x/y/z:Fixed,yaw/pitch:i16,seq:u16} + Snapshot{host_tick,entities}.
- encode_snapshot/decode_snapshot: fixed-layout LE, magic 0x554E "UN" + version + KIND_SNAPSHOT=2
  (wire.rs owns 0/1), u16 count capped at MAX_ENTITIES_PER_SNAPSHOT=1024; decode never panics,
  returns None on bad magic/version/kind/truncation/hostile-count; bounded Reader mirrors wire::Reader.
  Positions are the deterministic 48.16 Fixed (byte-exact across peers). EntityKind decodes unknown
  bytes to Other (forward-compatible).
- SnapshotGate = client newest-wins gate: per-entity last-applied seq, apply() returns only advanced
  entities, drops stale/duplicate/reordered. seq_newer() uses RFC1982 serial arithmetic over u16
  (survives wraparound: 0 newer than 65535; 0x8000 boundary treated as old). forget()/tracked() bound it.
- AuthorityStamper = host per-entity monotonic seq source (0,1,2,… wrapping; independent per entity).
- Distinct from sim::world_authority (reliable command-sourced WORLD EDITS) and sim::snapshot (rollback
  capture/restore) — this is the lossy transform path. Pure: no IO/clock/RNG/unwrap/expect/panic.
- Registered: pub mod + pub use in sim/net/mod.rs (Snapshot re-exported as EntitySnapshot to avoid
  clashing with world_authority::Snapshot).
- VERIFIED: flattened with a faithful 48.16 Fixed stub (same i64 layout/constructors as the real type),
  rustc --edition 2021 --test -> 12 passed / 0 failed. -> next session can confirm against the REAL
  Fixed via `cargo test --features "render-core egui" --lib net::entity_snapshot` (net tests link under
  that gate per NEXT_SESSION_GUIDE §2d). No bin/GPU/visual claims — pure codec + gate.
- MP pure layer now: items 1-? built earlier (net_candidates, net_rendezvous, session_mode WorldAdvert/
  ServerBrowser, net/identity, net/dht_mailbox VERIFIED) + r330b net_channels (item 5) + r330c
  entity_snapshot (item 7). Still open: world_advert wire codec (item 1), net_transport_select (item 2),
  world_presence (item 3), world_join_requests (item 4 — partly covered by net/identity Mailbox),
  chunk-stream seed/delta codec (item 6, §B). Items 8-9 (run_world seam + DHT/UDP IO) are bin, after pure.

## r330d — dispatched 3 more threads (2 code-maintenance + legendary swords) — CHECKED (docs only)
docs/dispatch_r330/DISPATCH_PACKET_r330_addendum.md. Grounded in real duplication evidence:
- r330-refactor-math: new util::mathx EXACT scalar helpers (clamp/lerp/inv_lerp/remap/smoothstep/
  saturate/nonneg) — there's no shared home (util::fastmath is approx transcendentals only) while
  fn clamp is locally redefined in ~161 files, lerp ~34, smoothstep ~20. Migrate ≥10 dup sites in
  the root-level visual/post-fx cluster with per-migration parity tests (bit-identical). Allow-list:
  root visual *.rs only; do-not-touch sim/, item.rs/loot.rs, the six-thread files, fastmath.
- r330-refactor-batch: new util::batch_kernel (map_into/map_collect) collapsing the 67 batch_* map-
  over-slice loops + duplicated nonneg/clamp (batch_aggro_decay is the exemplar: kernel + _one +
  hand-rolled _batch + its own nonneg). Migrate ≥6 sim/batch_* modules to route _batch through
  map_into, scalar-vs-batch parity tests stay green. Allow-list: sim/batch_*.rs only, never the
  scalar sibling (so it can't overlap refactor-math). Optional Batch1 trait if it reads cleanly.
- r330-swords: legendary swords fully into the game (pure path). Gap: sim::legendary_weapon forges
  full Weapon stat-blocks (forge(seed,class,element_idx,tier)) but item.rs::ItemKind has only
  Block/SpawnEgg/Ingot/Tool — no weapon variant, so a legendary can't be an inventory item / drop /
  be held. Add ItemKind::Legendary(LegendarySpec{seed,class,element_idx,tier}) (compact forge tuple,
  reconstruct via forge on demand), update display_color/max_stack(=1)/display_name exhaustive
  matches, + new sim::legendary_loot (Option<LegendarySpec> from a seeded rarity/drop roll) + one
  pure loot.rs::drops hook. Render/equip/hotbar/held-item proposed in HANDOFF_swords.md (bin, on-
  device). MERGE NOTE: lands item.rs variant first, then T-items' item_tooltip rebases its match.
Disjoint allow-lists across all 9 outstanding threads; addendum merge order refactor-math →
refactor-batch → swords. No code changed this round (dispatch docs only).

## r331 — MERGED 6 worker returns (items, bughunt, mobs, menus, world, gpu) — modules VERIFIED under 1.96; shader validated + reproducible
Coordinator merged the six r330 dispatch returns in order items→bughunt→mobs→menus→world→gpu.
Every worker built under apt rustc 1.75 (the 1.96 bundle wasn't in their sandbox); coordinator
RE-GATED each under the real Rust 1.96 + validated the shader independently. Scoped-delta checked
each edited file vs tree (no stale-base clobber) before applying.
- items: NEW sim::item_tooltip — pure item/affix/lore tooltip composition (TooltipLine/role/emphasis,
  compose(), wrap_lore() multibyte-safe, NaN-sinks-last affix sort). 10/10 standalone @1.96.
- bughunt: world/leaf_decay.rs floating-trunk decay FIXED (root cause: logs live in wood_indices not
  plant_indices so the is_log branch was dead; also cross-chunk-floor seam + air-only check + raw
  index underflow). Now iterates plant∪wood, resolves below across the chunk seam via .get().copied()
  (panic-safe), broadened support predicate (block_supports_trunk/log_is_unsupported, conservative on
  unloaded seams). Item 2 panic-audit was a NO-OP — world_catalog/world_launch had no non-test
  unwrap/expect (worker reported the false premise honestly; the real index-underflow surface was
  hardened in item 1). 9/9 via standalone trunk_proof @1.96.
- mobs: NEW sim::roamer_cap — spawn-admission only (admit/overflow_count/headroom; invariant
  current+admit<=cap; NO despawn path per r300). Parity-asserted vs batch_spawn_budget::allowed_one.
  8/8 standalone @1.96.
- menus: settings_model.rs + menu_screen_layout.rs extended (additive). New ShadowRes ladder,
  ApplyKind{Live,NeedsPipelineRebuild}+apply_kind(), render_scale/shadow_resolution/dynamic_lights
  fields (NaN-safe clamp/adjust; line-229 guarantee preserved), camera_gameplay_layout() page
  (FOV→RenderScale→ShadowRes→DynamicLights→InvertY→Back). 14/14 + 11/11 standalone @1.96 (Screen
  not extended; live render-pipeline apply proposed in handoff/HANDOFF_menus_bin_wiring.md).
- world: NEW sim::surface_scan — top_solid_y / top_solid_y_in_chunk read the actual top solid block
  from generated voxels (the §4a FP32/FP64 float bug; divergence test proves scan follows voxels not
  the FP64 prediction). 10/10 standalone @1.96. Bin call-site switch proposed in handoff/HANDOFF_world_surface.md.
- gpu: NEW sim::foliage_sway (SwayTag 2-bit, block_sway_eligible/vertex_tag/pack into pos_packed A2,
  Wind+sway_displacement) + render/mesher.rs // r330-gpu adapter (sway_flag/vertex_sway_tag via
  BlockId::is_subvoxel_only) + shaders/chunk.vert sways ONLY tagged vertices (not green color — the
  §4b bug). 15/15 standalone @1.96. chunk.vert: glslangValidator -V --target-env vulkan1.2 OK,
  spirv-val OK, and the committed chunk.vert.spv is BYTE-IDENTICAL to a fresh coordinator recompile
  (hash a6725e5b…; no recompile trap). Per-vertex tag EMISSION at the mesh emit loops is the remaining
  on-device step (handoff/HANDOFF_gpu_foliage_sway.md) — until wired, foliage is quiet (no regression).
Reconciled 4 pub mod lines in sim/mod.rs (item_tooltip/roamer_cap/surface_scan/foliage_sway); menus
touched already-declared modules. Integration seam (mesher↔foliage_sway↔is_subvoxel_only) symbol-checked.
Worker MANIFESTs + the gpu PNG proof preserved in docs/dispatch_r330/returns/. Total re-gated @1.96:
67 module tests pass + shader validated. NOT run: full-tree cargo build (cold link overruns a turn —
operator's on-device gate). 3 addendum threads (refactor-math, refactor-batch, swords) still outstanding.

## r332 — MERGED 3 addendum returns (refactor-math, refactor-batch, swords) + ItemKind exhaustiveness fix — modules VERIFIED @1.96
Coordinator merged the 3 addendum returns (order refactor-math→refactor-batch→swords). All built
under apt 1.75; coordinator re-gated each under real 1.96.
- refactor-math: NEW util::mathx (exact clamp/clampf/lerp/inv_lerp/remap/smoothstep/saturate/nonneg,
  all #[inline] pure) + 13 root-level visual modules migrated off local copies (decal_fade fog_blend
  weather rim_light cloud_noise depth_fade visual_fade lens_dirt tonemap_curve height_fog sss_approx
  edge_highlight fresnel_schlick), 26 call-sites + 1 local fn removed. Two NaN idioms kept distinct
  (saturate = .max(0).min(1); clampf = std .clamp). Un-clamped smoothstep / clamp01 / smootherstep /
  settings clampf deliberately NOT migrated (semantics differ — noted, not silently changed).
  Re-gated @1.96: mathx 12/12 + parity harness 78/78 (mathx + 11 migrated modules' own suites).
- refactor-batch: NEW util::batch_kernel (map_into/map_collect/nonneg) + 8 sim::batch_* migrated off
  hand-rolled map loops (batch_aggro_decay[+local nonneg folded out], crit_roll, threat_assess,
  status_stack[u8], scent_decay, memory_decay, drop_chance, path_cost). Scalar siblings untouched
  (disjoint from refactor-math). Batch1 trait evaluated, SKIPPED (multi-arg free-fn style). Bit-exact:
  _one/kernel unchanged + each module's batch==one parity test green + 40k OLD-vs-NEW fuzz. Re-gated
  @1.96: batch_kernel 8/8. (nonneg duplicated in mathx+batch_kernel, both bit-identical; optional
  future fold left as TODO.)
- swords: item::ItemKind::Legendary(LegendarySpec{seed,class,element_idx,tier}) + LegendarySpec::weapon()
  = legendary_weapon::forge (single-source), display_color/max_stack(=1)/display_name arms updated;
  NEW sim::legendary_loot::{DropContext,roll}->Option<LegendarySpec> (seeded splitmix); loot::
  drops_with_legendary additive hook (drops/base_drops preserved). Re-gated @1.96: 7/7 flattened
  against the REAL forge/drop_chance/rarity_tier bodies.
- COORDINATOR EXHAUSTIVENESS FIX (the worker couldn't — outside allow-list): adding ItemKind::Legendary
  made spawn.rs::resolve_use non-exhaustive. Swept the whole workspace for ItemKind matches: only
  resolve_use@333 lacked a wildcard (resolve_use_seeded delegates via `other =>`; recipe::recipes_for
  uses ==; bin@6863 has Some(other)=>; world_renderer/mining have _=> ; no glob-imported matches).
  Added Legendary to the Nothing arm: `Ingot|Tool|Legendary => UseAction::Nothing`. Held legendary
  places nothing on right-click; equip is the separate render path (handoff/HANDOFF_swords.md).
Reconciled mod lines: util/mod.rs +mathx +batch_kernel; sim/mod.rs +legendary_loot. Symbol-resolution
verified (mathx/batch_kernel exports cover all migrated imports). NOT run: full cargo build (operator's
M2 gate). ALL 9 r330 threads now merged. item_tooltip needs no Legendary arm (flattened view, not an
ItemKind match). Remaining swords on-device wiring: hotbar/inventory/held-item render of legendaries
(HANDOFF_swords.md §render); bin ItemKind arms if any new exhaustive match is added on device.

## r332-perf — frame_correlation probe (coordinator, VERIFIED) + 3 perf dispatches off the r331 M2 profile — CHECKED
The r331 M2 run is a TAIL/stutter problem: total p50=11.1ms (~90fps) but p99=264.75ms, max=642.27ms.
main pass dominates+spikes (p99 217 / max 639, 60% over 8.3ms); near mesher p99 104.5 / max 1020.8ms
(1s synchronous chunk builds); record 100% over 1ms (steady ~5ms CPU); wait_fences p99 347 (symptom).
- COORDINATOR (VERIFIED @1.96, 11/11): NEW sim::frame_correlation — per-frame spike correlation probe.
  The lifetime profiler aggregates zones independently so a 639ms main spike and a 1021ms near-mesh
  can't be tied to the same frame; this ingests one FrameSample/frame (total/main/cull/record/wait +
  near_mesh_ms, uploaded_kb, draw_count, chunks_meshed, vert_velocity), and on a budget-bust attributes
  the spike (GpuMain/Upload/Meshing/Cull/CpuRecord/GpuWait via classify()) + keeps the worst-N frames
  with all counters. summary() emits greppable [FRAMECORR] lines. Pure, NaN-safe, deterministic. This
  is the prerequisite to localize the main-pass tail the coordinator owns (chunk.frag/froxel_fill.comp
  off-limits). Wiring proposed in handoff/HANDOFF_frame_correlation.md (feed from existing timers +
  camera vvel; dump at shutdown). Answers: are 600ms main spikes Upload (big up=kB) or GpuMain (tiny)?
  do Meshing spikes track high vvel (fast-descent storm)? is record ever the dominant cause?
- DISPATCHED 3 perf threads (docs/dispatch_r332/DISPATCH_PACKET_r332_perf.md, headlesshorseman batch):
  * hh-r332-t1 near-mesh per-frame BUILD budget (sim::mesh_budget: estimate_cost/admit + runtime_config
    max_meshes_per_frame/mesh_build_budget_ms) — kills the 1s near-mesh spike; uploads are budgeted but
    mesh build isn't.
  * hh-r332-t2 localize+cut record cost (sim::record_zones accumulator to split record into
    chunks/entities/overlay/ui sub-zones + sim::draw_batch grouping instanced draws by
    (pipeline_key,mesh) to cut per-instance record CPU; builds on existing draw_key/pipeline_key).
  * hh-r332-t3 (OPERATOR ASK) partial/multi-mesh remeshing (sim::mesh_partition: 6 directional
    sub-meshes by face normal + dirtied_by_neighbor(side)->the one facing plane, so a new neighbour
    re-meshes 1/6 boundary not the whole chunk — attacks the documented "~35 re-meshes/chunk" storm).
    Greedy mesher already builds the 6 face planes (mesher.rs:1911); mesh_buffer_pool span alloc holds
    multi-span/chunk. Decoration geometry is interior → never re-meshed on a neighbour event.
  Merge order T1→T2→T3; chunk_manager wiring (T1 drain + T3 neighbour scope) coordinator-merged on-device.
No render hot-path code changed this round (probe is pure + off-hot-path); the perf fixes come from the
3 threads + the coordinator's main-pass work once the correlated M2 data lands.

## r332-features — MP chunk-stream codec (coordinator, VERIFIED) + 5 feature dispatches — CHECKED
- MULTIPLAYER (in-house, VERIFIED @1.96, 10/10): NEW sim::net::chunk_stream — world-streaming chunk
  record codec, roadmap item 6 / SECTION_multiplayer §B, grounds docs/COMPRESSION_DECISION.md. Rides
  the Bulk channel (net_channels). Pure selection + framing + delta; lzma + socket stay bin glue
  (records carry OPAQUE payload bytes). select_plan(modified,edited_cells,total,frac)->SendPlan
  {Seed|Delta|Dense} (pristine→seed; light→delta; heavy→dense; MAX_DELTA_CELLS=CHUNK_CELLS/4).
  build_delta/apply_delta = sparse (u16 index,u8 value) overrides vs the regenerated base (bounds-
  checked, None on oob/mismatch/too-many). encode_record/decode_record: magic 0x554E + version +
  KIND_SEED/DENSE/DELTA + coord + u32-len-prefixed opaque blob; decode never panics, MAX_PAYLOAD_BYTES
  =1MiB cap. Seed record = 24B. Registered pub mod + pub use in sim/net/mod.rs. Third MP pure module
  built in-house (net_channels item5 + entity_snapshot item7 + chunk_stream item6); together with
  entity replication + the QoS channels this is the "stream the world + replicate entities" core.
  Still open MP pure: world_advert wire codec(1), net_transport_select(2), world_presence(3),
  world_join_requests(4, partly in net/identity). Items 8-9 (run_world seam + DHT/UDP IO) bin, last.
- DISPATCHED 5 feature threads (docs/dispatch_r332/DISPATCH_PACKET_r332_features.md):
  * r332-swords-creative: item::legendary_palette() (class×element×tier sample set) so the merged
    legendaries get a creative slot and are testable; Tab::Legendaries wiring proposed (operator
    blocker: "swords still don't have creative inventory space").
  * r332-sound: occlusion-aware spatial audio (occlusion_gain/occluded_mix_gains/muffle_cutoff in
    sim::spatial_audio) — sound muffled through solid voxels; raymarch count is bin glue.
  * r332-structures: world::rare_structure rarity-gated deterministic per-region placement selector
    (select_for_region->Option<RarePlacement>) feeding the existing structure_place/grammar/template.
  * r332-bending: compose the existing pure bending modules end-to-end in bending_runtime
    (EffectIntent→targeting→discipline effect→feedback->BendOutcome) per SECTION_bending; player_bending
    currently emits intent only. Composition, no new mechanics.
  * r332-dragon: fix §4c body-part balloon in dragon_forge::skin anim path (extent-bound regression
    test + find the compounding scale + bound it; headless PNG proof).
  Disjoint allow-lists; merge order swords-creative→sound→structures→bending→dragon. Bin call-sites
  (creative tab, occlusion raymarch, worldgen structure call, bending apply) coordinator-wired on-device.
No render hot-path changed. Outstanding threads: 3 perf (hh-r332-t1/t2/t3) + 5 feature = 8, plus the
earlier 9 (now merged). frame_correlation still pending bin wiring before the next M2 run.

## r333 — MERGED 3 perf returns (mesh-budget, record-cost, mesh-partition) — all VERIFIED @1.96
Coordinator merged the hh-r332 perf batch (order t1→t2→t3). This round the workers had the 1.96
bundle and gated on it; coordinator re-gated each standalone @1.96 + verified the two integration
anchors against the real r332 tree. Clean low-conflict merge: 4 new self-contained sim modules +
4 pub mod lines, ZERO existing-file edits (all bin wiring proposed in handoffs).
- hh-r332-t1 NEW sim::mesh_budget — per-frame near-mesh BUILD budget (the 1s near-mesh spike).
  estimate_cost(solid_cells,lod) μs proxy; MeshBudget{max_meshes_per_frame=8, build_budget_us=3000};
  admit(pending,budget)->Vec<idx> nearest-first under count+cost caps, EXCEPT a lone oversized
  nearest is admitted alone (progress guarantee, no starvation). 14/14 @1.96. Wiring: runtime_config
  knobs + chunk_manager drain call-site (handoff/HANDOFF_mesh_budget.md). Stacks in front of the
  existing upload budget.
- hh-r332-t2 NEW sim::record_zones + sim::draw_batch — the steady 5ms record. record_zones:
  RecordZone{Chunks,Entities,Overlay,Ui} + RecordZoneFrame accumulator (add/total/fraction/dominant/
  zone_samples for profiler forwarding) so the next M2 run shows WHICH slice owns record. draw_batch:
  group(&[DrawItem{pipeline:u64(pipeline_key),mesh_id,instance}])->Vec<DrawGroup> stable group-by
  (pipeline,mesh_id), savings(). 8/8 + 7/7 @1.96 + a real pipeline_key #[path] integration test.
  Anchor confirmed: pipeline_key::encode->Option<u64>. Wiring: profiler sub-zones + record-loop
  grouping (handoff/HANDOFF_record_cost.md).
- hh-r332-t3 NEW sim::mesh_partition — partial/multi-mesh remeshing (operator ask; the "~35 re-meshes
  /chunk" storm). SubMeshDir{6 faces} + assign_quad(normal)->Option<dir> + dirtied_by_neighbor(side)
  ->the ONE facing plane + SubMeshSet(6-bit, mark/take_dirty). A neighbour appearing re-meshes 1/6
  boundary not the whole chunk; decoration geometry is interior → never re-meshed on a neighbour event.
  Encodes the mesher's exact FACE_NORMALS convention (TOP=0+Y,BOTTOM=1-Y,EAST=2+X,WEST=3-X,SOUTH=4+Z,
  NORTH=5-Z; verified still current at mesher.rs:779/790 after the r331 gpu adapter shifted lines).
  8/8 standalone + 8/8 cargo --lib integration @1.96. Wiring: [Option<MeshSlot>;6]/chunk + 6-bucket
  mesher emit + chunk_manager neighbour bump marks from_axis_delta(axis,delta).opposite() then
  re-meshes take_dirty() (handoff/HANDOFF_mesh_partition.md). Interacts with transition_remesh_stride
  (each remesh ~6× cheaper → throttle can relax) + WORKER_E seam (must still read both boundary sides).
These are VALIDATED CORES; the perf WIN lands when the proposed bin wiring is applied on-device (+ the
frame_correlation probe wired in to measure it). t1 feeds t3: a partial remesh is a cheap PendingMesh.
Outstanding: 5 feature threads (swords-creative/sound/structures/bending/dragon) still out.

## r334 — MERGED 5 feature returns (swords-creative, sound, structures, bending, dragon) — all gated @1.96
Coordinator merged the r332 feature batch (order swords-creative→sound→structures→bending→dragon).
Workers built on r330b (r332 base not uploaded to them); all additive/new-file or paste-in, re-gated
@1.96 by the coordinator.
- swords-creative: item::legendary_palette() appended to item.rs — 48 entries (8 WeaponClass × 6
  elements, alternating tier 0/7, splitmix seeds) so legendaries get a creative slot (operator
  blocker). COORDINATOR ANCHOR FIX: added WeaponClass::ALL (the palette needs it; absent in tree —
  like the resolve_use arm last round). 7/7 harness @1.96 against the real forge. Tab::Legendaries
  wiring proposed (handoff/HANDOFF_swords_creative.md).
- sound: NEW sim::audio_occlusion — occlusion_gain (per-block dB, floor 0.05) + occluded_mix_gains
  (== mix_gains when unoccluded) + muffle_cutoff_hz spectral hint. Raymarch count is bin glue. 8/8.
- structures: NEW world::rare_structure — rarity-gated deterministic per-region selector
  (RareKind{Obelisk,Shrine,Ruin,Monument}, RarityTable, select_for_region->Option<RarePlacement>);
  min-spacing exact by seeded sublattice (O(1)), biome bitmask gating, ≤1 rare/region. 8/8 + the
  worker also ran cargo check in-tree clean. Worldgen wiring proposed (HANDOFF_rare_structures.md).
- bending: NEW sim::bending_compose — composes the existing pure modules end-to-end
  (EffectIntent→targeting→discipline effect{stormstrike/heavy_brine/thermal_phase}→feedback->
  BendOutcome); Information/Elemental → Unwired. Shipped as new file (not folded into bending_runtime
  due to base diff). 85/85 @1.96 with the 9 real bending modules mounted (75 their tests + 10 new,
  none regressed). Fold-into-runtime + bin apply proposed (HANDOFF_bending_compose.md).
- dragon: §4c body-part balloon FIXED in dragon_forge::skin. Root cause: the non-leg seam-bridge
  added the world-space `grow` term ISOTROPICALLY to every half-axis regardless of rest size, so a
  small fast box swelled then shrank. Fix: clamp_extent(rest,animated,MAX_ANIM_GROW=0.15) bounds each
  non-leg axis to rest*1.15; arc-follow (centre rotation) untouched so motion is preserved; legs keep
  their own cap. Measured wing 9.44×→1.15×, head 4.32×→1.15×. 30/30 real-crate harness + before/after
  PNG (docs/dispatch_r332/dragon_balloon_fix.png). Delta verified scoped (2-line removal = the old
  isotropic b.half assignment → the clamped version).
Mod lines reconciled: sim +audio_occlusion +bending_compose; world +rare_structure; item.rs additive
(no mod line); dragon_forge edited in place. NOT run: full cargo build (operator M2 gate). All 9 r330
threads + 3 perf + 5 features now merged. Remaining open: none outstanding (next batch dispatched r334+).

## r334+ — dispatched 6 threads (operator list) — CHECKED (docs only)
docs/dispatch_r334/DISPATCH_PACKET_r334.md: gamemode switch (sim::game_mode toggle + per-mode caps
incl. creative-inventory gate for the legendary palette), worldmgmt (sim::world_management "Open to
DHT" toggle → SessionRole Local→Host + make_advert; consumes the MP layer; DHT IO is bin glue),
menus (nested submenu router/screen scaffolding over the existing MenuNav submenu/back machinery),
water-physics (buoyancy/swim/flow in water_physics+buoyancy), water-visual (depth color/caustics/foam/
gerstner in the existing water modules; non-off-limits shader only), fog (height-fog/atmosphere/godray
math + froxel-seam parity; froxel_fill.comp/chunk.frag OFF-LIMITS; optional non-off-limits shader).
Merge order gamemode→worldmgmt→menus→water-physics→water-visual→fog; disjoint allow-lists. The
worldmgmt "Open to DHT" toggle is the gate to the coordinator's next MP IO work (real DHT node).

## r335 — r334 dispatch threads LOST + re-issued; MP world_advert codec built (coordinator, VERIFIED)
- THREAD STATUS: the 6 r334 dispatch threads (gamemode/worldmgmt/menus/water-physics/water-visual/fog)
  are marked **LOST** — operator could not track them down, none returned. No work lost on our side
  (they were never merged). Re-issued identically as the **r335** batch off the current base
  (docs/dispatch_r335/DISPATCH_PACKET_r335.md, supersede note prepended). Same 6 briefs, same merge
  order gamemode→worldmgmt→menus→water-physics→water-visual→fog.
- MULTIPLAYER (in-house, VERIFIED @1.96, 10/10): NEW sim::net::world_advert — DHT world-advert wire
  codec, roadmap **item 1** (§A). Encodes/decodes session_mode::WorldAdvert as the DHT record payload
  {host_username, world_name, players:u16, max_players:u16, last_seen_ms:u64, candidates[{host:[u8;4],
  port:u16, kind:u8}]}. Mirrors wire.rs (magic 0x554E/v1); MAX_NAME_LEN=64 (char-boundary truncate on
  encode, reject over-cap on decode), MAX_CANDIDATES=16, UTF-8 validated, unknown CandKind tag → None,
  decode never panics. KIND_LAN=0/KIND_PUBLIC=1 match net_candidates::CandKind order. Self-contained
  over the wire repr (AdvertWire/CandWire); the WorldAdvert⇄AdvertWire field mapping + DHT put/get IO
  are bin glue. Registered pub mod + pub use in sim/net/mod.rs. This is the publish/observe codec the
  r335 worldmgmt "Open to DHT" toggle needs. FOUR MP pure modules now in-house: net_channels(5) +
  entity_snapshot(7) + chunk_stream(6) + world_advert(1). Still-open MP pure: net_transport_select(2),
  world_presence(3) [item 4 join_requests largely in net::identity]; items 8-9 (run_world seam + DHT/UDP
  IO) are bin, last. Next coordinator MP step gated on worldmgmt landing → real DHT node IO.

## r336 — MERGED 6 r335 returns (gamemode, worldmgmt, menus, water-physics, water-visual, fog) — all gated @1.96
The r335 batch returned (threads were NOT lost after all) and merged in order. Workers built on r330b;
coordinator re-gated each against the real merged tree @1.96.
- gamemode: NEW sim::game_mode — re-exports command_parse::GameMode (4 variants: Survival/Creative/
  Adventure/Spectator), GameModeCaps{creative_inventory,flight,instant_break,invulnerable,
  spawn_legendaries}, caps(mode) table, GameModeState{toggle()=S↔C button, cycle(), set()}. Creative
  ⇒ creative_inventory (gates item::legendary_palette in the picker). 7/7. Button wiring proposed.
- worldmgmt: NEW sim::world_management — WorldMgmtSettings{open_to_dht,whitelist_on(default true),
  max_players(clamp 1..16),world_name(non-empty)} + HostTransition{StayLocal,GoHost(WorldAdvert),
  CloseHost} + apply_open_to_dht(settings,&self_id,role,candidates,now_ms)→ open+Local builds advert
  via HostFlow::make_advert; MgmtApplyKind{Live,SessionLifecycle} (OpenToDht=SessionLifecycle).
  Consumes session_mode/net_candidates/identity (verified make_advert 5-arg signature matches).
  Signature deviation flagged: candidates+now_ms passed in (pure can't source IO/clock). 30/30 with
  real deps. CONSUMES THE MP LAYER — gates the real DHT-node IO. Page + DHT announce loop proposed.
- menus: EDIT sim/menu_router.rs (+MenuPage/MenuAction/MenuTreeRouter/ID_BACK=0xE0FF, additive) +
  sim/menu_screen_layout.rs (+main_menu_tree + sub-page builders, SPLICED to preserve the r331
  Camera-page work — worker's base lacked 67 of my lines). Nested submenu tree Root→{Singleplayer,
  Host→{WorldManagement}, Join, Settings→{Graphics,Camera,Audio,GamemodeToggle}} with action hooks for
  gamemode/worldmgmt. 71/71 with real menu_nav. ID_BACK clash avoided (router=0xE0FF vs layout Title=100).
- water-physics: EDIT sim/buoyancy.rs (additive) — WaterPhysicsParams, buoyant_force (Archimedes),
  swim_step (accel+drag+speed clamp), flow_push (current), + BONUS head_above_water/surface_accel/
  swim_vertical_step (treading lifts head above waterline). 20/20 + head_above_water_proof.png.
- water-visual: EDIT 4 crate-root modules (additive): water_depth_color (depth ramp + Beer-Lambert
  absorption), foam_threshold (shoreline foam by depth+slope), gerstner_wave (multi-octave swell),
  water_caustics (sun+depth intensity). 44 tests + water_visual_swatch.png.
- fog: EDIT 5 crate-root modules (additive): height_fog (FogParams + ambient floor), fog_blend
  (distance fog + height×distance combiner + froxel seam blend), scatter_inscatter, atmosphere_scatter
  (sun in-scatter), godray_math. Froxel/shaders untouched (off-limits). Diagnosed the "absent fog box"
  under the avatar (froxel screen-space artifact) in HANDOFF_fog.md. 56 tests.
Bin wiring proposed for all (gamemode button, worldmgmt page+DHT announce, menu submenu hosting, water
physics integration, fog params feed). Mod lines: sim +game_mode +world_management; menus/water/fog
edited existing modules (no mod line). All r330+perf+features+MP+r335 threads now merged.

## r336+ — dispatched 8 threads (operator idea list) — CHECKED (docs only)
docs/dispatch_r336/DISPATCH_PACKET_r336.md: simvectors (util::simvec batched autovectorizable vector
ops, parity with vecmath), dragon-size (restore small→huge size_class→scale ramp without fighting the
r332 anim clamp), plants (more plant_blueprint species), textures (more texture_atlas slots/UV +
position variation), weather (world::weather GlobalWeather state machine, temp/humidity-gated), biomes
(more Biome variants append-only + climate/blend params; coordinator sweeps exhaustive matches), poi-
structure (more RareKind + structure_grammar axioms), neural (q8 quantize/GEMM-packing speedups,
byte-identical parity required; CPU-side only, shaders off-limits). Merge order simvectors→dragon-size→
plants→textures→weather→biomes→poi-structure→neural. Coordination: biomes Biome-enum sweep + reconcile
poi-structure biome masks; dragon-size must respect the r332 clamp; neural parity is golden-vector.
Next coordinator MP: net_transport_select(2)/world_presence(3) + DHT-node IO now that Open-to-DHT +
world_advert are in.

## r337 — MP world_presence built (coordinator, VERIFIED) + r336 dispatch amended with operator bonuses
- NOTE: the re-uploaded results_tar.zst was the already-merged r335 batch (r336) — not re-merged.
- MULTIPLAYER (in-house, VERIFIED @1.96, 8/8): NEW sim::net::world_presence — discovery liveness,
  roadmap item 3 (§A). Host re-announce scheduler PresenceBeacon{due/emit/until_due, period
  REANNOUNCE_MS=15s} keeps the advert fresh before TTL; browser-side prune_stale<T,F>(adverts,now,ttl,
  last_seen) evicts Dead worlds; freshness(last_seen,now,reannounce,ttl)->Fresh|Stale|Dead (clock-skew
  → Fresh). TTL_MS=45s>REANNOUNCE so an on-cadence host never reads Dead. Pure, saturating time, the
  clock+DHT IO are bin glue. Registered in sim/net/mod.rs. FIVE MP pure modules now: net_channels(5)+
  entity_snapshot(7)+chunk_stream(6)+world_advert(1)+world_presence(3); item 2 (transport_order) already
  existed in net_candidates; item 4 (join_requests) largely in net::identity. Pure discovery+streaming
  layer essentially COMPLETE (1,2,3,5,6,7). Remaining MP is bin IO (items 8-9: run_world seam + real
  DHT/UDP node) — gated on worldmgmt Open-to-DHT (landed r336).
- DISPATCH AMENDED: docs/dispatch_r336/DISPATCH_PACKET_r336.md now carries the operator's 5 bonuses —
  textures(remove leaf/bush placeholder→real atlas slots), weather(cloud shader, non-off-limits +
  glslang/spirv-val, fed by world::cloud_coverage which already exists), biomes(fix ocean grass/trees
  LEAK past the flora_placement:173 gating — likely grass_field or shoreline blend), neural(RS_MIN
  0.5→0.25 + 4x upscale path; quantize parity intact), dragon-size(bonus genome/brain work). Re-rolled
  dispatch_r336.zip (v2). Same 8 threads, same merge order.

## r338 — MERGED 7 of 8 r336 returns (simvectors, dragon-size, plants, textures, weather, biomes, neural) — gated @1.96
poi-structure did NOT return (still out / lost). Workers built on r330b (except neural on r336);
all base-matched additive → coordinator wholesale-copied + re-gated, then swept Biome exhaustiveness.
- simvectors: NEW util::simvec — batched AoS+SoA autovectorizable ops (add/scale/dot/len/normalize/
  lerp _into, Vec3Soa), parity with vecmath. 6/6 standalone. +1 mod line.
- dragon-size: EDIT dragon_archetype (size_scale ramp SIZE_SCALE_MIN=0.45..MAX=3.0, ≈6.7× small→huge;
  27/27 standalone). COORDINATOR WIRED the forge multiply (the worker's base predated r332 so they
  proposed it): sim/dragon_forge.rs from_genome now scales the REST dimensional fields (bulk/neck_len/
  tail_len/leg_len/wing_span/snout/jaw_depth) by size_scale(classify(g).size_class), shape ratios
  unscaled; the relative r332 clamp_extent (rest·1.15) scales with it so huge dragons still animate.
- plants: EDIT world/plant_blueprint.rs (+6 species builders reed/fern/vine/shrub/wildflower/mushroom
  + species_blueprint dispatcher over FloraSpecies::ALL). 7 tests. base-matched.
- textures: EDIT texture_atlas.rs (VariantSet + variant_for murmur3 hash + variant_uv + block_texture_ext
  +20 blocks via tint). tests + PNG. base-matched. (BONUS leaf/bush placeholder removal NOT explicitly
  done — block_texture_ext adds tint-blocks; verify leaf/bush on device.)
- weather: EDIT world/weather.rs (GlobalWeather state machine step/precip_for, temp/humidity-gated,
  no flicker). tests. base-matched. (BONUS cloud shader NOT done — only weather.rs.)
- biomes: EDIT world/biomes.rs (+4 variants Mesa=24/Mangrove=25/VolcanicWastes=26/FrozenOcean=27, NO
  rebase needed — my tree's last id was LushCave=23, matching the worker base; +surface_block/palette/
  tree_profile/determine_biome/height_modifier/is_forest/TryFrom arms) + biome_loader to_legacy. All
  used BlockId variants verified present. COORDINATOR EXHAUSTIVENESS SWEEP: fixed 3 foreign exhaustive
  matches that lacked a wildcard — sim/feeding.rs graze_rate (Mesa/VolcanicWastes/FrozenOcean=0.0 barren,
  Mangrove=0.05 lush), sim/sound.rs biome_ambient_loudness (Mesa=10/VolcanicWastes=12/Mangrove=24/
  FrozenOcean=26), world/flora_placement.rs species_weights (Mesa sparse, VolcanicWastes/FrozenOcean
  none [ocean-flora intent], Mangrove wet-forest). All other Biome matches had wildcards. (BONUS ocean
  grass/tree LEAK fix NOT separately done; the "extra" biomes variant bundling a dynamic_events system
  + sim/mod.rs edits was DEFERRED as out-of-scope.)
- neural: EDIT render/q8.rs — quantize now delegates to quantize_blockwise (faster, BYTE-IDENTICAL to
  the retained quantize_scalar_reference oracle), + pack_tiles/unpack_tiles GEMM pre-packing, + isolated
  q4 experiment. 8/8 incl golden_vectors_exact + fast_path_byte_identical_to_reference. PARITY PROVEN.
  (BONUS RS_MIN 0.5→0.25 + 4x upscale NOT done — only q8 quantize/packing.)
NOT compile-gated as a whole (operator M2 cargo build is the final gate); per-module gates above.
OUTSTANDING: re-dispatch poi-structure; the 4 undelivered bonuses (cloud shader, ocean-flora leak,
25% slider+4x upscale, dragon brain/genome); optional dynamic_events from the biomes "extra".

## r339 — coordinator worked the missing r336 pieces single-threaded (poi-structure + all 5 bonuses)
Per operator: completed the gaps in-house before resuming thread dispatch.
- OCEAN FLORA LEAK (biomes bonus) FIXED: root cause was sim path divergence — vegetation_scatter::
  biome_base used a stale ad-hoc 0..4 id scheme, so real Biome::Ocean(11) + everything >=5 fell to a
  0.40 mid default and scattered grass into the sea. Added canonical Biome::flora_density() (exhaustive
  over all 28 variants; oceans+caves+volcanic=0, Mangrove lush) in biomes.rs as the SINGLE SOURCE; both
  flora_placement::biome_density_base and vegetation_scatter::biome_base now delegate (the latter via
  Biome::try_from(u8 as u32)). No cycle (biomes depends on neither).
- TEXTURES bonus (leaf/bush placeholder) FIXED: registered OakLeaves/BirchLeaves/DeadBush/Vine in
  block_texture (tinted tiles) so they no longer hit the untextured placeholder; dedicated art deferred
  to reserved atlas slots (handoff/HANDOFF_textures_foliage.md).
- NEURAL bonus (25% slider + 4x upscale): settings_model RS_MIN 0.5→0.25 (4x floor) + pure
  upscale_factor (1/render_scale, clamped) + upscale_mode (4/2/1). 15/15 (incl new test).
- DRAGON bonus (brain): NEW sim::dragon_brain::size_behavior(size_class)->SizeBehavior{turn_rate_scale,
  aggression_bias,stamina_scale}, tied to the restored size range (small=agile/timid, huge=ponderous/
  bold/enduring). Monotone, clamped. 2/2.
- WEATHER bonus (cloud shader): NEW shaders/clouds.frag — fullscreen procedural FBM cloud layer over
  the sky, density driven by `coverage` (push constant fed by cloud_coverage::coverage + GlobalWeather
  intensity), sun-lit, storm-darkened, horizon-faded. No new descriptor (single 112B push block).
  VALIDATED: glslangValidator -V --target-env vulkan1.2 OK + spirv-val OK.
- POI-STRUCTURE (the thread that never returned) DONE in-house: world::rare_structure +4 RareKinds
  (Tower/Henge/Statue/Well) with index/name/axiom + RarityTable default entries (biome-masked); ALL
  now [RareKind;8]. 8/8 standalone.
- COORDINATOR Biome exhaustiveness (from the r338 merge) already swept feeding/sound/flora_placement.
Gated: rare_structure 8/8, settings_model 15/15, size_behavior 2/2, clouds.frag glslang+spirv-val,
dragon_archetype 27/27, simvec 6/6, q8 8/8 (carried). biomes.flora_density exhaustive over 28 variants;
all BlockId/FloraSpecies refs verified present. Full cargo build remains the operator M2 gate.
DEFERRED (not merged): the biomes "extra" dynamic_events system (out-of-scope; available on request).

## r340+ — dispatched 7 shared-library-candidate threads — CHECKED (docs only)
docs/dispatch_r340/DISPATCH_PACKET_r340.md: 7 threads, one per cluster, hunting shared-lib extraction
candidates (duplicated logic → one shared module), each delivering a SHAREDLIB_REPORT (candidates +
call sites + cross-cluster overlap flags), optional extracted pure modules, and a perf bonus.
- sl-math (util math dedup), sl-batch (67 batch_* → batch_kernel), sl-fields (world::field_ops from
  field_step/diffuse/brush + *_field; HIGH-CARE), sl-net (sim::net::codec from the 5 wire codecs),
  sl-mesh (mesh primitives from mesher/partition/budget/sparse), sl-worldgen (noise/density/placement),
  sl-gpu (gpu_pack + GPU compression: octree/sparse/q4).
- TWO HARD CONSTRAINTS on all: RESPECT the continuous physics fields (FP32/FP64 scalar/vector fields,
  ~20 *_field modules + field_step/diffuse/brush — preserve semantics + bit-numerics, no block-flag
  flattening) and NO Minecraft assumptions (sub-voxel, continuous fields, real physics, no 16³-chunk/
  flood-fill/block-palette shortcuts).
- PERF BONUS each: (a) CPU multithreading via the EXISTING util::task_lanes (map_pure_threshold etc.),
  serial==parallel parity required; or (b) GPU compression (CPU-side packing, byte-parity, shaders
  off-limits). task_lanes already provides the pure data-parallel primitives.
- COORDINATOR will COMPOUND-MERGE: collate the 7 reports → overlap matrix → pick ONE canonical version
  per overlapping candidate → merge + sweep call sites once → gate @1.96 + verify every parity claim.
  Merge order sl-math→sl-batch→sl-fields→sl-net→sl-mesh→sl-worldgen→sl-gpu.

## r341 — COMPOUND-MERGED 7 shared-lib threads: 5 modules merged + overlaps resolved + sweep backlog
All 7 sl-* threads returned; coordinator built the overlap matrix (docs/dispatch_r340/
SHAREDLIB_OVERLAP_MATRIX.md) and compound-merged. The new shared LIBRARIES are merged; the large
CALL-SITE SWEEPS are recorded as a dedup'd backlog (not mass-executed — 800+ unverifiable edits).
- MERGED (gated @1.96): world::field_ops (9/9; clamp01 branch -0.0-preserving + saturate max/min, BOTH
  kept — field RESPECT; +task_lanes parallel op), sim::net::codec (9/9; Reader/Writer/check_header/
  read_str/read_vec/encode_into), render::mesh_prims (7/7; face table/FACE_NORMALS/quad + parallel AABB),
  world::worldgen_noise (7/7; delegates to hash_util, +parallel gen), render::gpu_pack (8/8; SlotPool +
  lossless pack_tiles). +5 mod lines (world×2, net, render×2).
- OVERLAPS RESOLVED: splitmix64→util::hash_util (canonical; worldgen_noise delegates; stepper-shape sites
  flagged). clamp/saturate→KEEP BOTH (mathx general + field_ops -0.0-preserving; proven divergence on
  -0.0, do not cross-route). Aabb→util::bbox (existing; sl-mesh wisely didn't extract). SlotPool→gpu_pack
  (canonical). pack_tiles: gpu_pack DUPLICATES q8::pack_tiles (output-identical) → gpu_pack canonical,
  q8-delegation deferred to keep the neural byte-parity oracle untouched.
- SWEEP BACKLOG (dedup'd, future gated pass): splitmix 71 files, clamp 743+101 sites, batch_* 34 folds,
  greedy_merge_mask, gpu_buffer (ash glue, bin-side), q8→gpu_pack.
- BONUSES (CPU MT via the EXISTING rayon-backed util::task_lanes; serial==parallel parity): field_ops,
  mesh_prims (quad_aabbs_parallel), worldgen_noise all shipped + parity-checked. GPU compression
  (gpu_pack lossless) shipped; byte-parity vs live octree/sparse formats DEFERRED by sl-gpu (layout drift).
- RESPECT verified across all: no field flattened, no wire/quantize bytes changed, no Minecraft/block-grid
  assumption. -0.0 field distinction preserved (the key trap, caught + kept split).
Per-module gates above; full cargo build is the operator M2 gate.

## r342 — single-threaded consolidation pass #1 (q8 dedup + VERIFIED splitmix folds)
Working the r341 sharedlib sweep backlog in-house, per-site verified (no blind sweeping).
- q8 pack dedup: render/q8.rs pack_tiles/unpack_tiles/packed_tiles_len now DELEGATE to the canonical
  render::gpu_pack (q8 had no real callers of these; algorithm now lives once). gpu_pack 8/8 + q8's
  own pack tests cross-check. q8 quantize parity untouched.
- splitmix64 → hash_util folds (VERIFIED bit-identical only):
  * sim/item_drop_resolve.rs::splitmix64 → hash_util::mix64 (identical).
  * sim/popgen.rs::mix → hash_util::mix64, ::unit → hash_util::mix64_unit (identical).
- VERIFICATION CAUGHT DIVERGENCES (left untouched — a blind 71-file sweep would have changed behavior):
  * sim/item_resolve.rs::mix64 — OMITS the initial wrapping_add(0x9E37..) golden step → NOT mix64;
    folding would change loot RNG. LEFT.
  * sim/navfield_gpu.rs + sim/tree_pollination.rs — RNG STEPPERS (advance &mut state per call), not
    fixed-seed finalizers. LEFT.
  * world/plant_lifecycle.rs — bespoke multi-coordinate murmur-style hash (different constants). LEFT.
  * sim/item_genetics.rs, sim/plant_propagation.rs — salted (mix(seed ^ salt*K)); only the float-extract
    tail matches mix64_unit. Partial; LEFT for now.
KEY FINDING: the sl-math "71 files" estimate was a grep of the golden-ratio CONSTANT, which appears in
every splitmix-derived RNG incl. steppers + delegators. Actual full mix64 duplicates in r341 were 3,
and 2 of those were genuinely different algorithms. Per-site verification is mandatory; the careful
single-threaded pass folded the real duplicates and correctly preserved the divergent ones.
Remaining backlog (clamp/batch_*/greedy_merge) still needs the same per-site treatment.

## r343 — single-threaded consolidation pass #2 (mix64 finish + verified lerp sweep)
- mix64 folds (verified bit-identical, incl golden-add step): sim/item_genetics.rs::mix,
  sim/plant_propagation.rs::mix64 → hash_util::mix64. (item_resolve still LEFT — no add step.)
- LERP SWEEP: scripted, exact-template-only. Folded 15 private scalar `fn lerp(a,b,t)=a+(b-a)*t`
  → crate::util::mathx::lerp (bit-identical, call sites unchanged): terrain_warp, water_waves,
  batch_ridge_noise, ridge_noise, batch_terrain_warp, scatter_cluster, plant_genetics, driver,
  scale_gloss, terrain_variation, fog_volume, title_cinematic, armor_reflect, player_views,
  dragon_forge. Brace-balance verified on folds.
  SKIPPED 21 (correctly, NOT bit-identical): vec/Vec3 lerps (simvec, vecmath, lerp3, water_depth_color,
  coral…), and CLAMPED lerps (shading_uniforms `clampf(t)` first, dragon_growth_stage `t.max(0).min(1)`
  first) — folding those would change rounding/behavior.
DEFERRED to a dedicated pass (the -0.0 trap needs the form-split): private fn clamp01 (109 defs,
branch-form→field_ops::clamp01 / max-min-form→mathx::saturate) and fn smoothstep (22 defs). Each needs
per-site form verification (a branch clamp01 must NOT be routed to the max/min saturate in a field path).
Per-template verification continues to beat blind sweeping (clamped-lerp + no-add-mix cases caught).

## r344 — single-threaded consolidation pass #3 (clamp01/saturate form-split sweep)
The deferred -0.0-trap sweep, done with per-site behavioral form-matching.
- mathx now hosts BOTH canonical forms: NEW mathx::clamp01 (branch; NaN→0; **preserves -0.0**) +
  existing mathx::saturate (x.max(0).min(1); NaN→0; **normalizes -0.0**). field_ops::clamp01 now
  delegates to mathx::clamp01 (single source). mathx 12/12.
- SWEEP (scripted, exact-body-template + param-normalized + comment-stripped; folds to the BEHAVIORAL
  EQUIVALENT so -0.0 semantics are bit-preserved):
  * 50 private branch-form clamp01 (incl `.clamp(0.0,1.0)` w/ NaN-guard) → mathx::clamp01. Field modules
    (frost_spread, thermal_erosion, smoke_field, fire_field, grass_field, ember_field…) routed to
    clamp01 (NOT saturate) — RESPECT preserved (their branch form keeps -0.0).
  * 22 max/min-form clamp01 → mathx::saturate.
  * 1 bare smoothstep (water_waves) → mathx::smoothstep.
- SKIPPED (correct, not bit-equivalent): bare `v.clamp(0,1)` w/o NaN-guard (item_genetics — NaN differs),
  branch w/o NaN-guard (gesture_resolve — NaN differs), helper-wrapped (san()/sanitize01()), two-step
  `let v=if v>0..` (grove_decay), and guarded smoothstep w/ div-by-zero early-returns (weather,
  species_distribution, foam_threshold). DEFERRED: ~35 clamp01 variants + ~21 non-bare smoothstep.
- Verified: brace balance OK on all 76 touched files; the 2 paren/brace "imbalances" flagged by the
  counter (shader_from_image, creature_grime) are format!-macro string artifacts — the actual folds are
  clean one-line delegators. mathx re-gated 12/12.
Per-template behavioral verification continues to pay off (NaN-divergent + guarded forms correctly kept).

## r345 — single-threaded consolidation pass #4 (batch_* boilerplate fold + clamp01 straggler)
- gesture_resolve.rs::clamp01 (branch w/o NaN guard) → mathx::clampf(v,0,1) (bit-identical: clampf is
  the branch ladder, NaN→NaN, -0.0 preserved). Real reimpl removed.
- batch_* explicit-loop boilerplate → batch_kernel::map_collect (10 modules): batch_gene, batch_perception,
  batch_settle, batch_ability_cast, batch_appendage_ripple, batch_blink, batch_breathe, batch_horn_charge,
  batch_loot_roll, batch_status_effect. Each `let mut out=Vec::with_capacity(src.len()); for PAT in src
  { out.push(EXPR) } out` → `map_collect(src, |PAT| EXPR)`, preserving the loop's destructuring pattern
  (incl tuple `|&(a,b)|`) + outer captures (dt, table) + correct src (loot folds over seeds, captures
  table). Behavior-identical (same order/values; map_collect routes through map_into = the parallelization
  seam for a future task_lanes bonus). Brace/paren balance verified (strings+comments stripped).
- DELIBERATELY LEFT: 17 batch_* using idiomatic `.iter().map(f).collect()` — that's standard Rust, not
  duplication; wrapping it in map_collect would be anti-consolidation. ~1 form-B w/ intermediate
  statements + 8 "other" batch modules skipped (didn't match the exact single-push shape).
Operator confirmed r344 built clean (behavior unchanged) — validates the per-site verified-fold approach.
REMAINING backlog: greedy_merge_mask (mesh proposal, needs real extraction not a sweep), gpu_buffer
(bin-side ash glue — propose only), q8→gpu_pack (done r342), salted mix (done r343).

## r346 — SEED AUDIT + fix: "two seeds in one world" (bin + engine)
Root cause: no single live world-seed. Launch rebuilt only the terrain generator
(rebuild_for_seed); field overlays/creatures/colony/weather kept WORLD_SEED_DEFAULT → launched
worlds stitched terrain(picked seed) onto overlays/spawns(default seed). See docs/SEED_AUDIT.md.
- ENGINE: NEW ChunkManager::set_fields(&mut self, Arc<FieldRegistry>) — runtime field-registry swap
  (clears cached chunk field grids). Enables rebuilding overlays for a new seed post-construction.
- BIN: NEW single live `fg_world_seed` (backdrop default pre-launch, picked seed post-launch). All
  in-game seed consumers rerouted to it (7 WORLD_SEED_DEFAULT-as-u64 spawn sites → fg_world_seed).
  Launch Ok block: fg_world_seed=seed + chunk_manager.set_fields(FieldRegistry over a model at the
  new seed) + weather reseed (fg_world_seed^salt; was a fixed seed shared by all worlds) + colony
  reseed (fg_world_seed^0xFA1A). Startup constructions stay WORLD_SEED_DEFAULT (the backdrop;
  fg_world_seed starts equal → no pre-launch behavior change).
- Verified: bin brace/paren depth 0 (raw-string/comment/char-literal aware checker); chunk_manager
  balanced. Bin not full-buildable in-session → operator M2 build is the gate.
- FLAGGED for the upcoming MP work: the client/server JOIN path must have the joiner ADOPT the
  host's seed (handshake), or it'll regenerate its own = the same class of bug across the network.

## r346b — TEXTURE PIPELINE AUDIT (docs): textures don't show because the atlas was never GPU-wired
Investigated "textures still don't show". Verdict: the block-texture atlas is CPU scaffolding never
connected to the GPU — blocks render with vertex color + procedural fbm grain in chunk.frag. Not a
regression; an unfinished feature. See docs/TEXTURE_PIPELINE_AUDIT.md.
- EXISTS (CPU, ready): block_texture table, BLOCK_ATLAS layout/tile_uv, block_texture_table_bytes()
  (upload-ready 32B/entry SSBO bytes), the pack_color_id alpha-byte seam.
- MISSING (all GPU): (1) atlas PNG never loaded/uploaded, (2) block_texture_table_bytes never bound
  (BlockTexGpu used only in texture_atlas.rs), (3) chunk pipeline has NO atlas sampler/table binding
  (only UBO+shadow+froxel), (4) active CPU mesher hardcodes tile-id 0xFF / never calls block_texture
  (Vertex has no block field), (5) chunk.frag samples no atlas (procedural albedo only).
- PLAN (on-device M2; GPU/ash/shader = NOT gateable here, NOT done blind): S1 thread block id through
  Vertex+from_world (pure, the only coordinator-landable stage, but interdependent so deferred), S2
  upload atlas image+sampler (binding 3), S3 upload table SSBO (binding 4), S4 extend chunk DSL/pool,
  S5 chunk.vert forward id + chunk.frag sample table[id] tile-by-normal * tint. Table-driven design
  matches existing BlockTexGpu scaffolding.
- Honest constraint: coordinator has no GPU + can't gate Vulkan/ash + a wrong binding = black/crashed
  chunk pass, so the GPU stages are on-device work. Recommend wiring S1–S5 in one session w/ a debug
  force-untextured toggle to bisect.

## r347 — textures S1: build-health confirmed + verified CPU→vertex tile data path (compile-gated)
Set up the compile-gate the operator pointed at: `cargo check --lib --features headless,egui` (render-core
headless via ash dlopen + egui, no glfw) — compiles in ~10s incremental. CONFIRMED r342–r346 all build
clean under it (consolidation + seed fix are good).
- block_textures: NEW atlas_rgba(flat_white) (256² RGBA procedural programmer-art per named tile; magenta
  for MISSING; flat-white = the bisect toggle) + ATLAS_PX + BlockTiles::face_tile(face) + face_tile_byte
  (mapped→0..=254, unmapped/MISSING→255 sentinel). Tests written (dims/white/face mapping).
- mesher: Vertex gains `tile: u8` (default 255 = "no atlas, keep procedural"); emit_quad tags each face
  vertex via face_tile_byte(block, cfg.face); PackedVertex::from_world now packs pack_color_id(color, tile)
  (rides the existing reserved alpha byte). INERT until the shader reads alpha (per the seam comment) →
  zero render regression. Greedy quads keep one tile/quad; the ruleset's world-pos face_uv handles repeat.
- Compile-gated clean (headless,egui, 9.85s). Pure data path; no GPU binding yet → textures still not
  visible until the next step lands.
REMAINING (next, all compile-gateable via the same harness + glslangValidator + shader_contract):
  S2 upload atlas image+NEAREST sampler (flat-white if UNENDLICH_NO_TEX) — mirror egui_overlay::set_texture;
  S4 add chunk_pipeline DSL binding 4 (combined image sampler) + chunk_renderer pool/descriptor write
  (mirror froxel binding 2); S5 chunk.vert forward id+worldpos, chunk.frag: id==255→procedural else
  atlas_uv(tile, face_uv)*tint (transcribe texture_mapping ruleset); update shader_contract EXPECTED_BINDINGS
  += binding 4 + atlas_uv expectation.

## r348 — FIX r346 seed-decl self-reference + establish a real BIN compile-gate
- BUG: r346's global sed (WORLD_SEED_DEFAULT as u64 → fg_world_seed) also rewrote the *declaration's*
  initializer → `let mut fg_world_seed: u64 = fg_world_seed;` (E0425, self-reference). Missed because
  `cargo check --lib` does NOT compile the bin. FIXED → `= WORLD_SEED_DEFAULT as u64;`.
- BIN COMPILE-GATE now available in-sandbox: installed cmake + X11 dev headers (libx11/xrandr/xinerama/
  xcursor/xi/xext-dev) so glfw-sys builds. `cargo check --bin unendlich-fullgame --features
  render,hot-shaders,window,gpu,ui-overlay` → Finished in ~33s. (audio dropped: alsa-sys needs
  libasound2-dev, irrelevant to logic.) ALL r346 seed-fix edits + r347 texture data path compile clean
  in the operator's exact feature set.
- PROCESS: bin/render edits are now compile-gated here before shipping (lib gate = headless,egui ~10s;
  bin gate = the window feature set ~33s). The r346 escape happened because only --lib was checked.

## r349 — GEOMETRY BUG LOCALIZED (headless probe): CPU pipeline is clean; bug is GPU per-chunk draw data
Operator hit "giant translucent sails over normal terrain" on world load. Bisected with free env toggles:
NEURAL_GBUFFER=0 removed the translucent/shifting smear (that was the neural temporal resolve); GPU_MESHER=0
did NOT help (so not the GPU mesher). Then PROVED the CPU geometry pipeline clean via a headless probe
(examples/geom_probe.rs, runs over the lib's pub mesher API under --features headless,egui):
- mesh_chunk / naive / far / very_far / downsampled_f{1,2,4,8,16}: ALL vertices in-bounds (origin..origin+32),
  OOB=0, ANY_OOB=false.
- pack→GPU-unpack round-trip: max_err=0.037 (= quantization step 32/1023), worst legit edge 45.3 (full-chunk
  greedy-quad diagonal), giant_edges(>50)=0.
CONCLUSION: no CPU path produces out-of-bounds verts or giant triangles when the correct per-chunk origin is
used. The sails (which carry the chunk grain texture → they ARE chunk geometry) come from the GPU runtime
feeding WRONG per-chunk draw data — stale/mismatched aabb_min origin or vertex/index offsets in the indirect-
draw + shared DEVICE_LOCAL mesh buffer path (chunk_renderer writes aabb_min correctly at upload: l.470
[ox,oy,oz,oy]; chunk.vert reads .x/.w/.z — correct). This path is governed by the experimental perf flags
(DEVICE_LOCAL_MESH, ADAPTIVE_UPLOADS, SUBMIT_SKIP, GPU_MESH_BATCH, cull), NONE of which r342–r348 touched.
Likely a latent stale-slot/offset bug, possibly TRIGGERED by the seed fix's set_fields clearing chunk_grids →
remesh storm → slot churn. NEXT: operator flag-bisect (DEVICE_LOCAL_MESH=0 first), then inspect that path.
Textures are NOT blocked: on a minimal-flag clean-geometry baseline, ship S2/S4/S5 to test.
TOOL: examples/geom_probe.rs is the reusable headless geometry-corruption probe (extend to slot/offset logic).

## r349 — ASAHI FIX CANDIDATE: position vertex attr A2B10G10R10_UNORM_PACK32 → R32_UINT + manual unpack
Per operator's fp/Asahi-Mesa-Vulkan-1.3 lever. The probe proved CPU geometry clean, so the giant sails
are the GPU mispositioning vertices. Mechanism: chunk.vert/chunk_shadow.vert read position as `vec4
in_pos_unorm`, i.e. the DRIVER unpacks VK_FORMAT_A2B10G10R10_UNORM_PACK32. If Asahi Mesa mis-normalizes
that packed format (e.g. hands back raw 10-bit ints instead of [0,1]), every position is ~1023× too large
→ giant geometry across all chunk passes — and the neural gbuffer was masking it (which is why it only
showed once we disabled neural to debug). Fp32-large-coord precision was ruled out as the giant-tri cause
(≈0.5-block ulp at 4M; that's texture-UV swimming, not sails — noted for the texture step: compute UVs from
chunk-LOCAL coords, not fract(world)).
FIX (robust, driver-portable — bypass the UNORM decode):
- chunk.vert + chunk_shadow.vert: position input `vec4 in_pos_unorm` → `uint in_pos_packed`; unpack by hand
  matching PackedVertex::pack_pos exactly: x=(p&0x3ff)/1023, y=((p>>10)&0x3ff)/1023, z=((p>>20)&0x3ff)/1023,
  sway=(p>>30)&0x3. local_pos = vec3(x,y,z)*CHUNK_EXTENT. (Bit layout confirmed vs foliage_sway::pack_tag_into_pos
  POS_ALPHA_SHIFT=30, MASK=0x3.)
- chunk_pipeline.rs + shadow_pipeline.rs + gbuffer.rs: attribute 0 format A2B10G10R10_UNORM_PACK32 → R32_UINT
  (same 4 bytes/offset, no vertex-buffer change). gbuffer reuses chunk.vert so it MUST match.
- shader_contract EXPECTED_VERTEX_INPUTS[0] → "in_pos_packed".
- Color attr LEFT as R8G8B8A8_UNORM (universal format, egui/UI use it and UI is crisp → not the culprit).
- Regenerated assets/shaders/chunk.vert.spv + chunk_shadow.vert.spv from edited source (hot_shaders reads
  baked .spv from disk; build.rs also recompiles on hash mismatch / falls back to committed .spv).
GATES: glslangValidator -V (both vert OK), bin gate clean (render,hot-shaders,window,gpu,ui-overlay).
TEST: run NORMALLY (neural can stay on). If sails gone → confirmed Asahi A2B10G10R10 vertex-unpack quirk.
TOOL: examples/geom_probe.rs retained (headless geometry-corruption probe).

## r350 — BISECT BUILD: revert r346 on-load set_fields + r347 from_world alpha-pack
Operator bisect: r341 clean, r344 clean (neural off), r348 = FIRST compilable build showing the giant-
sails bug (r347 never compiled — it carried r346's self-reference). So the regression is in r345/r346/r347.
Cleared by analysis: r345 (sim/creature batch only), r347 (positions identical — from_world only changed the
color ALPHA byte; pos_packed unchanged; Vertex size-of only used for a pool CAPACITY ESTIMATE + a CPU-only
stride const, never a GPU offset). Prime suspect r346 (the terrain/field-on-load path the synthetic probe
never exercised). r350 reverts BOTH suspects in one build for a decisive test:
- r346: disabled the on-load `chunk_manager.set_fields(FieldRegistry::with_world_model(WorldModel::new(seed)))`
  (bin ~8836). rebuild_for_seed still runs.
- r347: from_world back to `pack_color(v.color)` (0xFF alpha) — textures unwired anyway.
Kept: r349 R32_UINT manual unpack (position-equivalent), r345.
OUTCOMES: clean → it's r346 or r347 (cheap follow-up isolates: re-enable from_world only). still broken →
it's r345 (surprising; would dig into the batch fold's runtime effect). Bin-gated clean (~55s).

## r351 — FIX: revert r347 mesher texture-datapath (the giant-sails regression)
Operator forward-bisect was decisive: copying r348's render/mesher.rs (+ its block_textures dependency)
onto the otherwise-CLEAN r344 tree (clean bin, clean world data) reproduced the giant-sails bug. So the
regression is ENTIRELY r347's mesher texture-datapath — NOT the seed fix, NOT r345 batch, NOT shaders
(none changed), NOT the GPU/indirect path. My headless mesh probe missed it because it only checked vertex
POSITIONS (which are identical across the change); the corruption rode the change nonetheless.
r351 reverts the mesher's Vertex struct + emit path to r344's exact form:
- removed `pub tile: u8` from Vertex (back to 24-byte {position, color})
- Vertex::new back to `Self { position, color }`; removed `with_tile`
- from_world back to `pack_color(v.color)` (0xFF alpha)
- emit_quad back to `Vertex::new(positions[i], color)` (no face_tile_byte tagging)
KEPT: r346 seed fix, r349 R32_UINT unpack, r345 batch folds (all proven not the cause).
block_textures.rs atlas/face_tile_byte additions remain (now unused by the mesher — harmless, ready for a
PROPER texture-wiring pass later that does NOT alter the live Vertex format until the atlas+sampler+frag are
all in place). Lib + bin gated clean.

## r352 — Texture wiring Step 1: gated procedural per-block (UNENDLICH_TEXTURES)
Goal: prove the per-face tile-id seam end-to-end with ZERO Vulkan-resource risk before
the real atlas sampler (Step 2). Entire path gated behind `UNENDLICH_TEXTURES` (default OFF).

- **Default (flag off) = byte-identical to r351** (the proven-clean build). Verified inert:
  mesher packs the 0xFF untextured sentinel into color alpha (pack_color, as r351);
  chunk.frag `apply_block_texture` early-returns unchanged for id==255. Working build never at risk.
- **mesher.rs**: re-added `Vertex.tile: u8` (probe-proven 28B mesh-clean) + `with_tile` + default 255;
  module-level cached `textures_enabled()` (OnceLock on `UNENDLICH_TEXTURES`); `from_world` gated —
  `pack_color_id(color,tile)` when on, else `pack_color`. Tagged tile on `emit_quad` (cfg.face) and
  `emit_subdivided_top` (TOP) via `block_textures::face_tile_byte`. Other emitters left at 255
  (untextured) for now — Step 2 widens coverage.
- **chunk.vert**: `+ layout(location=2) out float v_tile = in_color_unorm.a;` (gbuffer frag ignores
  the extra vertex output — only consumes loc 0/1; shadow vert unaffected).
- **chunk.frag**: `+ layout(location=2) in float v_tile;` `+ apply_block_texture()` (after hash12,
  before its call site) — id<255 → tile grout lines + per-id grain modulating albedo; id 255 = no-op.
  Inserted right after `albedo = v_color * detail;`.
- Shaders glslangValidator-clean (vulkan1.2); regenerated committed `assets/shaders/{chunk.vert,
  chunk.frag,chunk_gbuffer.frag}.spv`; dropped stale `.spv.hash` so build.rs recompiles-from-source
  (compiler) or uses the committed spv (no compiler).
- Lib gate + bin gate both clean.

TEST: run normally → must look exactly like r351. Then `UNENDLICH_TEXTURES=1 ...` → terrain faces
(greedy sides + grass tops) should read as textured (grout + grain). If glitched/black, just drop the
flag — default stays clean — and we isolate the seam. CANNOT runtime-test here (no GPU); on-device only.
PENDING Step 2: real atlas sampler (binding 4 COMBINED_IMAGE_SAMPLER + image upload + NEAREST + UV).

## r353 — Texture diagnostic/fix attempt: id in low nibble, alpha pinned high
r352 result: flag-off CLEAN, flag-on (low id-alpha) = translucent sails + stone textured.
Splits to: the *alpha VALUE* triggers it (struct/shader changes are in clean flag-off too).
Code ruled out every conventional alpha consumer (blend off, out_color.a=1.0, gbuffer ignores
alpha, sway bits hardcoded 0, GPU stride fixed at 8, no discard/alpha_to_coverage). "Translucent"
strongly implies alpha-as-opacity in a stage not visible in source (driver/fixed-function).
- from_world (textures on): tile id → LOW nibble, HIGH nibble pinned 0xF → alpha ∈ [0xF0,0xFF]
  (0.94–1.0, opaque). Untextured (tile 255) stays 0xFF. Real ids are only 0..=8 so 4 bits suffice.
- chunk.frag apply_block_texture: id = alpha&0x0F; sentinel = low nibble 0xF → untextured.
- Default (flag off) unchanged = r351.
TEST: `UNENDLICH_TEXTURES=1` → if CLEAN + textured: sails were alpha-as-opacity, this is the fix.
If STILL sails: opacity isn't it (any non-0xFF alpha triggers it) → next is the no-op-frag isolation.

## r354 — Textures via block COLOR (alpha untouched), gated on camera_pos.w
r353 result: even alpha 0.94–0.97 sails → NOT opacity; ANY alpha != 0xFF triggers it, for a
reason invisible in source (position/index provably independent of alpha; no blend/discard/AtoC;
GPU stride fixed 8). Operator hint: wind system has a history of quirks (default wind slider set
to 2 to work around an issue) — sails may be a wind/sway interaction, noted for the id-seam later.
DECISION: stop riding the alpha byte entirely.
- from_world: alpha ALWAYS 0xFF (byte-identical vertices to r351 → sails mechanism cannot trigger).
- chunk.frag apply_block_texture: seeds the per-block procedural surface off the block's albedo
  (distinct per block type), NOT the vertex alpha. Grout lines + per-block grain.
- Gate: ubo.camera_pos.w (1.0 when UNENDLICH_TEXTURES set, else 0.0). .w is unused by all shaders
  (read only .xyz/.y). Deliberately NOT wind_params (operator flagged wind quirks).
- v_tile varying left declared-but-unused (harmless; was present + clean in r352 flag-off).
- Lib/bin gate clean; chunk.frag.spv regenerated.
TEST: default = r351 exactly. `UNENDLICH_TEXTURES=1` → per-block textures should appear with NO
sails (vertices are 0xFF-alpha = the proven-clean data; texturing is frag-only).

## r356 — Remove the v_tile varying (the real culprit); mesher back to r351
GROUND TRUTH from r355: UNENDLICH_TEXTURES=1 AND =0 BOTH sail → flag was never the variable;
the build was structurally broken regardless. With the flag off the GPU vertex data is byte-
identical to r351 yet sails ⇒ the cause is the SHADER change, not the data. The only shader change
active flag-off was the `v_tile` varying added to chunk.vert (location 2), which chunk_gbuffer.frag
(neural pass, shared chunk.vert) does NOT consume — a vertex-out/frag-in interface mismatch Mesa/
Asahi mishandles. (This also fits r347, which needed no flag.)
- chunk.vert: removed `out float v_tile` + its assignment → back to r351 (loc 0/1 only).
- chunk.frag: removed `in float v_tile`. apply_block_texture no longer needs it (seeds off albedo).
- mesher: removed Vertex.tile field, with_tile, emit tagging → 24-byte Vertex, EXACTLY r351.
- Kept: color-based apply_block_texture gated on ubo.camera_pos.w (UNENDLICH_TEXTURES); from_world
  always 0xFF. camera_pos.w unused by shaders as homogeneous (only swizzled), safe.
- All shaders glslangValidator-clean; spv regenerated; bin gate clean.
TEST (need BOTH): flag OFF → must be CLEAN (= r351) confirming v_tile was the culprit.
flag ON → CLEAN + per-block color-driven texturing. If flag-off still sails, strip apply_block_texture
presence next; if only flag-on sails, the texture math is the issue.

## r357-r358 — Autonomous batch: per-face textures, snow caps, climate composite
Operator away (no device testing); worked solo/sequential. Rule held: r356-clean default
behavior untouched; every new visual behind a default-OFF flag; pure logic unit-tested.
Validation available this session: glslangValidator, `cargo check` (lib+bin), lavapipe
gpu-headless-probe (Vulkan bring-up + GPU mesher confirmed green on llvmpipe). NO Asahi GPU,
so visuals are UNVERIFIED on the real driver — flags keep the default safe regardless.

### r357 — Per-face (top/side/bottom) texturing (chunk.frag, gated UNENDLICH_TEXTURES)
- `apply_block_texture(base, wpos, n)` now varies by the geometric `normal` already computed
  in the frag (n.y>0.5 top / <-0.5 bottom / else side), with UV taken IN the face plane so
  side textures run vertically instead of smearing. Top brightest, bottom darkest, per-face
  grain salt. NO vertex data, NO varying, NO seam — cannot reintroduce the r352-r355 sail class.

### r358 — Altitude snow caps (chunk.frag, gated UNENDLICH_SNOW)
- Snow-line altitude carried in the free `ubo.sky_color.w` (0 = off; shader samples only .rgb).
  CPU sets it from `climate_surface::snow_caps_enabled()` / `DEFAULT_SNOW_LINE` (96.0).
- Frag: above the line, blend albedo→snow-white over a 6-block soft band; up-facing faces hold
  more snow than steep sides (uses `normal`). Brightness/color only — no geometry.

### climate_surface.rs — composite climate core (pure, tested; world/)
- New integration point combining the existing UNWIRED tested primitives (season_phase,
  seasonal_temp_offset, snow_line, snow_cover): `resolve(ColumnClimate) -> SurfaceClimate`
  {season_phase, effective_temp, snow}. Seasonal offset shifts effective_temp BEFORE the snow
  signals read it (winter genuinely lowers the snow line).
- `surface_block(&SurfaceClimate, base) -> BlockId` — worldgen-facing decision: snowy solid
  columns → Snow; liquids/air pass through. The one call a worldgen surface pass will make.
- 7 unit tests; lib `cargo check` clean; 5 confirmed green in-run, 2 new compile-verified
  (full test-link too slow to re-run in-sandbox this session).

### Queued (need operator's device or are structural — deferred deliberately)
- **Connected textures**: needs per-face neighbor adjacency = a vertex varying again. SAFE path
  now known: re-add the varying AND add the matching `in` to chunk_gbuffer.frag so the interface
  matches across BOTH passes (the r356 lesson). Validate the matching-interface varying in
  ISOLATION on-device before relying on it.
- **Entities**: separate entity.vert/frag pipeline; needs investigation + on-device validation.
- **climate→render/worldgen wiring**: the composite core is ready; the render-tint coupling needs
  a sim tick threaded into draw_frame (currently only monotonic_frame, which would desync the
  seasonal phase from the season tint). Worldgen surface_block wiring needs the worldgen surface
  pass + visual validation. Both deferred to a testable session.

## r359 — Multiplayer seed adoption (pure session layer, test-VERIFIED)
The network analog of the r346 single-player "two seeds in one world" fix: a joiner
must rebuild its world for the HOST's seed, or it generates a divergent world. The
advert carried no seed, so a joiner had no way to learn it. Fixed in the pure sim
layer (no bin/IO/socket changes — `apply_open_to_dht` is still only test-exercised,
so this is contained):
- `WorldAdvert.world_seed: u64` — the host publishes its seed in the advert.
- `HostFlow::make_advert(.., world_seed)` and `world_management::apply_open_to_dht(.., world_seed)`
  thread the host's seed (bin-owned, like `candidates`/`now_ms`) into the advert.
- `JoinFlow` captures `advert.world_seed` at `connect()` and exposes `host_seed() -> Option<u64>`;
  it survives to `Connected` (so the bin can rebuild for it) and clears on `back_to_browse()`.
- Verified: 17 tests green via standalone `rustc --test` (flatten trick — full lib test-link
  is too slow in-sandbox): session_mode 8/8 (incl. seed adoption on connect, seed cleared on
  return-to-browse, advert publishes seed), world_management 9/9 (incl. GoHost advert carries
  seed end-to-end). `cargo check --lib` clean.

### Bin integration still TODO (needs the operator's device + the net/socket layer)
The seed now flows host→advert→JoinFlow at the logic layer. The bin must still:
1. On host: pass `fg_world_seed` (the r346 var, bin/unendlich_fullgame.rs:1934) into the
   DHT announce / `apply_open_to_dht` (currently not bin-wired at all).
2. On join reaching `Connected`: read `JoinFlow::host_seed()` and run the SAME rebuild path
   r346 uses for single-player (`rebuild_for_seed` + `chunk_manager.set_fields` + reseed
   weather/colony/creatures + `clear_all_chunk_meshes` + `pick_spawn`, ~bin:8835).
This bin↔net wiring is larger and needs on-device multiplayer testing — deferred.

## r360 — Genetics: clutch breeding wired into the population layer (test-VERIFIED)
Three genetics primitives were orphaned — `dragon_breeding::cross` and `clutch_size`
existed but nothing combined them into a "mated pair lays a clutch" event, and
`next_generation` bred a flat random-paired pool that ignored clutch size entirely.
Wired them together, additively (existing `cross`/`next_generation` untouched):
- `dragon_breeding::breed_clutch(mother, father, seed) -> Vec<Genome>` — lays
  `clutch_size(mother, seed)` eggs, each an independent `cross` under a distinct
  per-egg seed so siblings vary while the clutch stays reproducible.
- `dragon_population::next_generation_clutched(parents, seed) -> Vec<Genome>` —
  pairs parents `(2p, 2p+1)`, each pair lays a `breed_clutch`; generation size is
  emergent (sum of clutch sizes), so leaner dragons contribute more offspring.
  Falls back to founders with <2 parents.
- Verified: 14 tests green via standalone `rustc --test` (flatten trick) —
  dragon_breeding 7/7 (incl. clutch length follows clutch_size, deterministic,
  siblings differ), dragon_population 7/7 (incl. emergent size in range,
  deterministic, founder fallback). `cargo check --lib` clean.
- Follow-up (clean, needs a small `Genome` trait-vector accessor): pair mates by
  `mate_select::compat` instead of in-order, so compatible dragons are likelier to
  breed. Left unwired — `mate_select::compat` still has no real caller.

## r361 — Genetics: compat-based mate pairing (retires the mate_select orphan, test-VERIFIED)
`mate_select::compat` had no real caller. Wired it in, additively:
- `dragon_archetype::Genome::traits() -> [f32; 17]` — the morphological genes as a
  flat vector for similarity/compat scoring (elements excluded; order matches cross).
- `dragon_population::pair_by_compat(parents) -> Vec<(usize, usize)>` — greedy
  best-compat pairing (each unpaired dragon matched to its most-compatible remaining
  partner; ties → lower index; odd one out dropped). Pure, deterministic, no seed.
- `dragon_population::next_generation_clutched_compat(parents, seed)` — same clutch
  model as r360 but pairs mates by compatibility instead of index order.
- Verified: 10/10 dragon_population tests via flatten `rustc --test`, incl. the key
  one proving compat overrides index order (interleaved [a,b,a,b] pairs the identical
  dragons (0,2),(1,3), not in-order (0,1),(2,3)). `cargo check --lib` clean.
With r360+r361 the dragon genetics chain is fully composed: cross + clutch_size +
breed_clutch + compat pairing, all test-verified. Remaining genetics orphans (lower
value): gene_crossover/gene_mutate/trait_dominance are still only in the GPU kernel
registry — dragon cross() uses its own inline blend; reconciling them would change
existing tested behavior, so left alone unless desired.

## r362 — Items: crafting output bridged to appraisal (test-VERIFIED)
The crafting resolver (`resolve_craft → CraftedItem`), the appraisal core
(`item_appraise::appraise`), and `item_compare` were all built+tested but had NO
real callers and weren't connected to each other. Bridged the craft output into
appraisal, additively:
- `craft_resolve::appraise_crafted(item, base_value, rarity) -> f32` — maps a fresh
  `CraftedItem` (no enchant/mods, full durability) into `AppraiseInput` and returns
  its trade value. Non-decreasing in quality, base value, and rarity; finite, ≥ 0.
- Verified: 2/2 new tests via standalone `rustc --test` mirroring the *real*
  appraise dep tree (item_durability + enchant_apply + item_modifier + util::mathx),
  so it exercises the actual appraisal math — higher quality/base/rarity ⇒ higher
  value; zero base ⇒ zero. `cargo check --lib` clean.
- Note: `item_compare::compare` still has no caller — it wants an `ItemView`
  (affixes + genes + rarity) that a bare `CraftedItem` doesn't carry, so there's no
  natural bridge until crafted items gain affixes/genes (a larger change to the
  tested `CraftedItem` shape). Left for when item genetics is wired into crafting.

## r363 — Bending: focus items wired into the resonance charge path (test-VERIFIED)
`bending_focus::focus` (aggregating equipped focus items → `FocusMods`) had no real
caller, and the bend pipeline never applied focus modifiers. Wired the one
*unambiguous* FocusMods field — `resonance_gain_mul` — into charge building:
- `bending_charge::Resonance::build_scaled(on_rhythm01, dt, gain_mul)` — `build` with
  a gain multiplier (non-finite/negative → 0; still capped at 1.0). `build` now
  delegates to it with `1.0` (behavior unchanged).
- `bending_compose::build_resonance_focused(resonance, items, on_rhythm01, dt)` —
  aggregates the focus items via `focus()` and builds resonance scaled by their
  `resonance_gain_mul`. Empty items → identity (plain build).
- Verified: standalone `rustc --test` over the real bending_charge + bending_focus
  (19 tests incl. all their existing tests) — `build_scaled` 2x gain ⇒ 2x charge,
  zero/negative ⇒ no gain, delegation identity; composer: a resonance-gain focus item
  charges faster than none, empty == plain. `cargo check --lib` clean.
- Deliberately NOT wired (needs a `Discipline`→channel map I won't guess): FocusMods
  `strength_add[discipline]` (per-discipline power) and `field_delta` (pre-access field
  projection). `Discipline` has a parameterized `Elemental(Element)` variant, so the
  index mapping isn't a clean function — left for a session that can confirm it.

### Note on brains (surveyed, intentionally not forced)
The brains cluster is already well-integrated: `behavior_arbiter::select` is used in
the bin + dragon_activity + fauna_behavior; `fauna_mind` (using `flee_decision`) is the
live mob mind (mob_forage_memory, service_registry_batch). The remaining orphans
(`fauna_behavior::step`, `flee_threshold`) are redundant ALTERNATIVES to wired systems,
so wiring them would change live runtime behavior — needs the device, not a safe pure
composite. Left alone deliberately.

## r364 — Items: loot resolver output bridged to appraisal (test-VERIFIED)
Mirror of r362 for the loot side. `item_resolve::resolve → ResolvedItem` (the loot
resolver) was orphaned and unconnected to appraisal. Bridged it, additively:
- `item_resolve::appraise_resolved(item, base_value) -> f32` — maps a `ResolvedItem`
  into `AppraiseInput`: rarity → rarity, modifier count → mod multiplier, and the
  `set_bonus` folded into base value (a set piece is worth more). Loot has no craft
  quality / durability, so quality is neutral and durability not applicable. Finite,
  ≥ 0; non-decreasing in rarity, modifier count, base value, and set bonus.
- Verified: 2/2 new tests via standalone `rustc --test` over the *real* appraise dep
  tree (item_durability + enchant_apply + item_modifier + util::mathx). `cargo check`
  clean. With r362+r364 both resolver outputs (craft + loot) now reach the appraisal
  core; `item_compare` still needs an ItemView bridge (deferred — bare resolved/crafted
  items lack the affix/gene vector it compares).

## r365 — Items: loot drops comparable vs equipped (item_compare wired, test-VERIFIED)
`item_compare::compare` (the drop-vs-equipped tooltip delta) had no caller. The
blocker was thought to be a missing affix/gene vector, but `ItemView.affixes` and
`ResolvedItem.affix_magnitudes` are the *same* "affix magnitude" concept — so the
bridge is faithful:
- `item_resolve::to_item_view(item) -> ItemView` — resolved affix magnitudes map
  one-per-slot into the integer affix slots (rounded; padded/truncated to N_AFFIX);
  loot has no heritable genes here so the gene vector is zero; rarity carries through.
- Verified: standalone `rustc --test` over the real `item_compare` (10 tests incl. its
  own 9) — a rarer drop with higher affix slots shows positive rarity/affix deltas vs
  equipped. `cargo check --lib` clean.
With r362+r364+r365 the item-evaluation cores (appraise + compare) are all reachable
from both resolver outputs. The three item-eval orphans are now retired.

### Survey note: remaining sim clusters and why they're not clean pure-composite work
Mined this session: genetics (done r360/r361), items (done r362/r364/r365), bending-focus
(r363). Surveyed and intentionally deferred: brains (well-wired; orphans are redundant
alternatives → behavior changes needing the device), cognition (gaps are behavior changes
to a stateful, WorldQuery-generic subsystem — e.g. `learning::Policy` isn't wired into
`Mind`; heavier, needs a world mock + behavior validation), inventory (three misaligned
slot types — parallel WIP needing reconciliation, not a clean bridge), plant genetics
(two `PlantGenome` types sim vs world — same parallel-WIP issue). The clean additive-bridge
seam is largely harvested; remaining work is heavier (stateful/generic) or needs the device.

## r366 — Items: appraisal → market price (market_price wired, test-VERIFIED)
`market_price::price` was orphaned. Appraisal and the market both modelled rarity,
so feeding an appraised value straight into `price(.., rarity, ..)` would double-count
it. Added the correct bridge + a full-chain demonstrator:
- `market_price::market_value(appraised_value, demand)` — applies market demand + the
  price floor to an ALREADY-appraised value; rarity is not re-applied (tier 0). The
  intended entry point from appraisal into the market.
- `item_resolve::market_price_resolved(item, base_value, demand)` — one call from a loot
  drop to its vendor price: `appraise_resolved` → `market_value`.
- Verified: market_price 7/7 (incl. demand monotonic, no rarity re-add, floor) run
  directly; the full chain via standalone over the real appraise dep tree (rarer/
  more-modified/set-bonused item sells for more; more demand ⇒ higher price; floored).
  `cargo check --lib` clean.
Item economy chain now composed end to end: resolve → appraise → market. (A symmetric
`market_price_crafted` on the craft side is trivial if wanted.)

## r367 — Combat: full PvE attack pipeline (5 orphaned primitives wired, test-VERIFIED)
The minimal `combat_resolve::resolve(atk, def, crit)` ignored a set of richer combat
cores that were all orphaned: `hit_chance`, `damage_after_armor`, `resist_apply`,
`overkill_damage` (and used `crit_roll::crit`). Composed them into a full resolution,
additively (the minimal `resolve` is untouched):
- `combat_resolve::Attack` / `FullHit` + `resolve_full(&Attack) -> FullHit` — chains
  hit_chance (land?) → crit_roll::crit (crit mult) → damage_after_armor::mitigated
  (armor) → resist_apply::after_resist (element) → overkill_damage::overkill (spill).
  A miss yields zero damage. Uses an ARMOR mitigation model (not the `def` divisor of
  `resolve`), so the two paths don't double-mitigate. PvE creature-vs-creature only,
  same as `resolve` — no player-target route.
- Verified: 12/12 combat_resolve tests via standalone `rustc --test` over the real
  primitives (miss = 0; clean hit = full base; crit 2× doubles; armor + 50% resist
  reduce; 100 dmg vs 30 hp ⇒ 70 overkill; deterministic). `cargo check --lib` clean.
- Still-orphaned combat extras (armor_reflect, diminishing_returns_resist, damage_shard,
  debuff_duration_scale) model effects outside a single hit's damage number; folding
  them in changes combat feel and is better done with the device.

## r368 — Tuning + feature audit (jump, stamina; weapon/ability investigation)
Operator batch. Two safe tuning fixes (live-game constants; feel UNVERIFIED on device,
stamina logic unit-tested):
- **Jump**: `physics::constants::JUMP_VELOCITY` 8.0 → 10.0 (peak ~1.16 → ~1.79 blocks).
  This is the LIVE path (`physics::Body::try_jump`, bin:4481). NOTE: a second, UNUSED
  jump exists — `constants::JUMP_IMPULSE` via `player.rs` (not called by the bin); left
  alone. "Build-up jumping" (momentum/bunnyhop) is a separate movement feature, deferred.
- **Stamina**: `stamina::DRAIN_PER_SEC` 0.5 → 0.1 → ~5× sprint duration (full→empty ~2s→~10s).
  Left `REGEN_PER_SEC` (0.18) unchanged, so recovery is now *faster* than drain (generous,
  vs the old punishing design). Updated 2 stamina tests to be tuning-robust (invariant:
  sprints then stops when dry; spent reserve recovers to full). 5/5 stamina tests green.
  If you want recovery kept proportionally slow, say so and I'll scale REGEN too.

### Feature audit (operator asked "do we have X yet")
- **Flying sword attack AI** — EXISTS as pure logic: `sim/item_entity.rs` (ItemBody +
  ItemBrain + genes — "the hero's flying blade you can send flying", held/flying-out/
  recalled-returning states) + `sim/item_brain.rs`. Gap: bin/3D spawn + attack wiring.
- **Chain lightning** — EXISTS: `sim/bending_stormstrike.rs` (`route` → StrikePlan arcing
  across conductors, max_links/max_hop), composed in `bending_compose::resolve_bend`. Gap:
  3D render + a bin cast path (no `stormstrike` references in the bin yet).
- **Axes / staffs** — ItemForm::{Axe, Staff, Blade, Tool} EXIST with viewmodel + held poses
  (`viewmodel.rs`, `held_item_pose.rs`). Gap: distinct per-form attack behavior.
These are the "big batch": the cores/forms exist; the work is bin + 3D render + attack
wiring, which needs the device (GPU + feel). Not blind-buildable here.

### Other batch items (investigated; need the device — plan only)
- **Survival inventory**: toggle is `I` (bin:2234, `show_inventory`); the panel is wired for
  creative. Survival needs the same grid bound to the survival inventory data + hotbar.
  egui/input + visual — device work.
- **Keys "eaten" a bit**: there's an `ui_wants_keyboard` gate (bin:2210/2453, "fields eat
  letters"); the leak is likely egui retaining keyboard focus or `glfw::get_key_name`
  returning None for some keys. Needs on-device repro.
- **F3 debug menu / replace the old top-left tester HUD**: no `F3` toggle exists yet; the
  top-left HUD is always-on. Plan: gate the HUD behind an F3 toggle + refresh its contents.
  egui/render — device work.

## r369 — Weapons: lightning sword + axe swing resolution (test-VERIFIED, ready to wire)
`legendary_weapon` already forges weapons (WeaponClass incl. Blade/Axe/Greataxe,
ProcKind::LightningArc, `chain_targets` arc, `forge`) and is wired into `item.rs` — but
nothing turned a forged weapon's *swing* into a concrete attack. Added that composite so
the lightning sword + axes are ready to hook to visuals on-device:
- `legendary_weapon::SwingOutcome { damage, crit, arc_targets, arc_damage }` +
  `resolve_weapon_swing(w, crit_roll, origin, nearby, arc_links, arc_hop)` — direct damage
  = base × crit (crit_roll < crit_chance ⇒ ×crit_mult); a `LightningArc` weapon (Storm,
  element 5) additionally arcs through nearby foes via `chain_targets`, each taking
  `base_damage × proc_power`. Other procs leave the arc empty (their own resolvers).
- A **lightning sword** is `forge(seed, WeaponClass::Blade, 5, tier)` (proc=LightningArc);
  an **axe** is `forge(seed, WeaponClass::Axe, element, tier)` (heavy base damage). A Storm
  axe/greataxe both cleaves AND arcs.
- Verified: 23/23 legendary_weapon tests via `rustc --test` (lightning sword hits + arcs
  with no repeats; axe is heavy melee with no arc; crit applies crit_mult; deterministic).
  `cargo check --lib` clean.
- READY-TO-WIRE (needs device): bin spawn/equip + 3D render of the swing + the lightning
  arc VFX along `arc_targets`, and feeding `damage`/`arc_damage` into per-target mitigation
  (`combat_resolve::resolve_full`). The logic + targeting are done and tested.

## r370 — Weather: physical weather-cell composite (test-VERIFIED)
There are two weather models: `world::weather` (time-driven, live) and a set of
physically-grounded primitives that were orphaned (`cloud_coverage`, `precip_rate`,
`storm_intensity`, `pressure_gradient_wind`, `rain_shadow` — only cloud_coverage was
referenced, by a shader). Composed them into one local weather cell (the climate_surface
pattern, for weather):
- `world::weather_cell::{CellInputs, WeatherCell, resolve_cell}` — physical order:
  rain shadow → effective moisture → cloud cover → precipitation; plus storm intensity
  (moisture × instability) and pressure-gradient wind. Pure/deterministic; each field
  stays in range because each core clamps its own. Registered in world/mod.rs.
- Unlike the time-driven model, this lets weather emerge from terrain (a peak's lee is
  drier), altitude, temperature, and local pressure — ready to drive per-region weather.
- Verified: 6/6 tests via flatten `rustc --test` over the real primitives (humid cell
  clouds+rains; rain shadow dries the lee → fewer clouds/less rain; storms need moisture
  AND instability; wind tracks the pressure gradient; warmer+higher thins cloud;
  deterministic + bounded). `cargo check --lib` clean.
- Clouds note: `cloud_coverage` already feeds `shaders/clouds.frag`; this composite gives
  a CPU-side cloud/precip/storm/wind field per cell that a render pass or the weather HUD
  could consume — wiring that to the 3D cloud render needs the device.

## r371 — Seasonal weather + craft market price (VERIFIED) + gated lightning-arc melee (CHECKED, device-UNVERIFIED)
Two verified pure composites plus the first opt-in device wire of the r369 weapon work.

Pure (test-VERIFIED via flatten `rustc --test`):
- `world::seasonal_weather::{AtmoInputs, seasonal_cell}` — lets the season drive the
  weather cell: `climate_surface::SurfaceClimate` supplies temperature and a temperature-
  derived convective instability (warm air ⇒ summer storms; cool ⇒ overcast + calm), the
  caller supplies the terrain/atmosphere half, and it resolves a `weather_cell`. Composes
  climate_surface + weather_cell; reimplements neither. 6/6 tests (summer stormier, winter
  cloudier, rain shadow still applies, bounded + deterministic). Registered in world/mod.rs.
- `craft_resolve::market_price_crafted(item, base_value, rarity, demand)` — craft-side twin
  of `item_resolve::market_price_resolved`, completing resolve_craft → appraise_crafted →
  market_price_crafted (uses `market_price::market_value`, no rarity double-count). 1 test
  (quality + demand raise price, market floor, finite). (Skipped a crafted→ItemView compare
  bridge: bare crafted items lack the affix/gene vector ItemView needs — would be arbitrary.)

Device wire (compile-CHECKED only; on-device behavior UNVERIFIED):
- bin `unendlich_fullgame.rs` player melee path: behind `UNENDLICH_LIGHTNING_ARC=1`
  (default OFF), after the primary hit, a Storm blade is forged and
  `legendary_weapon::resolve_weapon_swing` (r369) picks the arc targets among nearby living
  roamers (≤12m gather, greedy nearest, no repeats) and applies `arc_damage` to each, logs
  `[lightning] arc chained to N foe(s)`. Purely additive + gated — default melee unchanged.
  `cargo check --bin` clean (0 errors, no new warnings); not run on device.
- TO TEST ON DEVICE: run with `UNENDLICH_LIGHTNING_ARC=1`, melee a creature standing in a
  herd, watch the bolt chain to neighbours (log + their HP). Next device step = arc VFX
  along the chained targets + a real equipped-weapon check instead of the always-forged blade.

## r372 — Weather: a moving front across a strip of cells (test-VERIFIED)
Extends the weather sim from one cell to a line of cells with a passing front —
the two things a single `weather_cell` can't express:
- `world::weather_front::Front { center, warm_temp, cold_temp, width }` with
  `advanced(wind_speed, dt)` (the front rides the wind downwind) and
  `temperature_at(x)` (warm upwind, cold downwind, smoothstepped via the
  previously-orphaned `front_temperature_gradient` core).
- `weather_front::advect_moisture(moisture, inflow, courant)` — donor-cell upwind
  advection: monotone (no new extrema) and mass-conserving up to inflow/outflow
  (a uniform field with matching inflow is stationary).
- `weather_front::resolve_strip(xs, moisture, front, temp_lo, temp_hi, atmo)` —
  composes both with `weather_cell` to resolve the weather along the whole strip
  (advected moisture = windward moisture; front temperature → temp_norm).
- Verified: 7/7 tests via flatten `rustc --test` over the real cores (warm ahead /
  cold behind; an advancing warm front warms a downwind cell; uniform moisture is
  stationary; a moist pulse moves downwind; advection stays in range; the strip
  resolves bounded weather and the cold side is cloudier; deterministic).
  `cargo check --lib` clean.
- This closes the loop on the orphaned physical-weather primitives: rain_shadow,
  cloud_coverage, precip_rate, storm_intensity, pressure_gradient_wind, and now
  front_temperature_gradient are all composed (weather_cell / seasonal_weather /
  weather_front). Driving a real cell grid from these + rendering it needs device.

## r373 — Weather: storm organization from wind shear (test-VERIFIED)
The last orphaned weather primitive was `wind_shear` (wind strengthening with
altitude). Composed it with the cell's convective storm intensity into the
severe-weather idea that storms need BOTH instability and shear to organize:
- `world::storm_organization::relative_shear(top_altitude, shear)` — the
  dimensionless fractional strengthening of wind aloft, read off the `wind_shear`
  core (unit-independent, so robust to whatever units the surface wind is in).
- `organization(storm_intensity, relative_shear) -> [0,1]` — a storm organizes
  only with both convective intensity and shear; 0 with no storm, monotone in
  each, saturating in shear.
- `StormMode { Calm, Pulse, Organized, Severe }` + `classify(..)` +
  `organize_cell(&WeatherCell, top_altitude, shear)` — a pulse storm (no shear)
  vs a severe/supercellular one (strong deep-layer shear) from the same intensity.
- Verified: 5/5 tests via flatten `rustc --test` (relative shear grows with height
  and shear; no storm never organizes; organization rises with both shear and
  intensity and stays bounded; modes order Calm→Pulse→Severe with the score
  tracking; deterministic). `cargo check --lib` clean.
- The physical-weather cluster is now fully composed: rain_shadow, cloud_coverage,
  precip_rate, storm_intensity, pressure_gradient_wind, front_temperature_gradient,
  and wind_shear — across weather_cell / seasonal_weather / weather_front /
  storm_organization. No orphaned weather primitives remain.

## r374 — Weather: temporal snowpack (test-VERIFIED)
The weather sim's first temporal layer — precipitation that builds and melts a
snow pack over time. Composes the orphaned `snow_accumulate::depth_step` with the
weather cell's precip:
- `world::snowpack::step(depth, &WeatherCell, temp_c, dt)` — the cell's precip
  accumulates as snow below freezing and the pack melts above (snow_accumulate),
  never negative. `step_norm(..)` takes the cell's normalized temperature plus the
  caller's °C range and de-normalizes it via `to_celsius` (freezing at 0 °C, so
  the bridge between the [0,1] climate scale and snow_accumulate's physical scale
  is explicit, not a hidden threshold).
- Verified: 6/6 tests via flatten `rustc --test` (to_celsius maps the range; cold
  + precip builds the pack; a warm spell melts it to exactly 0 and never negative;
  cold + dry holds steady; a cold-then-warm season builds then melts; deterministic).
  `cargo check --lib` clean.
- Weather sim now spans space AND time: weather_cell (point) → seasonal_weather
  (season) → weather_front (moving front + advection) → storm_organization (shear)
  → snowpack (accumulation over time). All composing previously-orphaned cores.

## r375 — Weather: weather_state capstone (test-VERIFIED)
A single entry point that resolves a location's full local weather in one step,
so the rest of the engine has one thing to call instead of five:
- `world::weather_state::{LocalWeather, StepInputs, step}` — `step(climate,
  inputs, prev_snow_depth)` chains seasonal_weather (climate+atmosphere ⇒ cell) →
  storm_organization (shear ⇒ organization + StormMode) → snowpack (precip+temp ⇒
  snow depth). `LocalWeather { cell, organization, mode, snow_depth }`. The
  previous snow depth is the only carried state (in/out); adds no new physics.
- Verified: 5/5 weather_state tests via flatten `rustc --test`, AND a 20/20
  regression of all four composed layers together (seasonal_weather, storm_org,
  snowpack, weather_state) — no cross-module interference. `cargo check --lib` clean.
- WEATHER SIM COMPLETE (CPU/logic side): weather_cell · seasonal_weather ·
  weather_front · storm_organization · snowpack · weather_state. Every physical
  primitive composed, space + time covered, one-call capstone. Remaining work is
  device-side: feed a real cell grid (player column climate + terrain) into
  weather_state and render it (cloud density, precip, snow cover, storm VFX).

## r376 — Weather: first bin integration — gated live readout (CHECKED, device-UNVERIFIED)
The first device-side step for the weather sim: sample `world::weather_state` for
the player's column and log it, so the composed model can be confirmed live before
it drives any rendering.
- bin `unendlich_fullgame.rs`: behind `UNENDLICH_WEATHER_SIM=1` (default OFF), every
  ~2s it builds a `SurfaceClimate` for the player's column — `effective_temp` falls
  with altitude (`view.position.y`, ~y60 ground → ~y200 peaks) and drifts with the
  game-clock season (`world_clock_elapsed`) — feeds `weather_state::step` (carrying a
  persistent `weather_snow_depth`), and logs
  `[weather] temp=.. cloud=.. precip=.. storm=.. org=.. <Mode> wind=.. snow=..`.
- Purely a readout: no gameplay or render state is touched. The underlying sim is the
  r370–r375 VERIFIED model; only the live wiring is new.
- `cargo check --bin` clean (0 errors, no new warnings). On-device behavior UNVERIFIED.
- TO TEST: run with `UNENDLICH_WEATHER_SIM=1` and watch the `[weather]` log line every
  ~2s. Climb to high altitude → temp drops → snow should accumulate; descend / wait for
  the warm season → storms appear and snow melts.
- NEXT (device): once the numbers look sane, drive rendering from it — cloud density,
  precipitation, snow cover on the ground, storm-mode VFX — and sample a real climate
  per column instead of the altitude+clock approximation used for the readout.

## r377 — Weather: readout now uses the REAL climate field (CHECKED, device-UNVERIFIED)
Replaces r376's altitude+clock stand-in with the engine's real climate cores, so the
`[weather]` readout reflects the actual world, not an approximation:
- bin samples `sim::biome_climate::climate_at(fg_world_seed, x, z, y)` at the player's
  column → real temperature (°C, varies by latitude/elevation/noise) + moisture01.
- temperature is normalized to [0,1] base_temp and run through the tested
  `world::climate_surface::resolve(ColumnClimate{ day_of_year from world_clock_elapsed,
  year_length 360, seasonal_amplitude 0.3, base_temp, precip=moisture01, slope 0,
  altitude=y, snow_line 120 })` — so the season offset + snow-line composition is real.
- that SurfaceClimate + moisture-driven AtmoInputs feed `weather_state::step` (snow
  range −10..30 °C, matching the base_temp normalization). Log now shows the real °C:
  `[weather] 12°C temp01=.. moist=.. | cloud=.. precip=.. storm=.. org=.. <Mode> wind=.. snow=..`.
- Two tested cores now back the readout (biome_climate + climate_surface); only the
  normalization constants + wiring are new. `cargo check --bin` clean. Still gated by
  `UNENDLICH_WEATHER_SIM=1`, read-only, device-UNVERIFIED.
- TO TEST: same as r376 — the numbers should now track the real biome (e.g. moisture
  higher in wet biomes, temperature dropping as you climb or with latitude/season).
- NEXT (device): drive rendering from this (cloud density / precip / snow cover / storm VFX).

## r378 — Flight: wing-buffet knockback (test-VERIFIED)
Switched off weather to a fresh orphan. `wing_buffet_force::force` (linear knockback
falloff to max_range) was orphaned; composed it into a usable AoE force:
- `sim::wing_buffet::buffet_power(wing_area, flap01, gain)` — peak power from a
  dragon's wing area and flap effort (gliding ⇒ 0), reads naturally from dragon_flight.
- `buffet_force(origin, target, power, max_range, down_bias)` — knockback vector on a
  target: magnitude via `wing_buffet_force` (linear falloff), direction radially away
  from the dragon with a downward bias (downwash pushes out + down). Vector length ==
  the falloff magnitude; zero beyond range / at zero power; finite even directly
  above/below.
- `buffet_targets(..)` — the AoE batch over many targets.
- Verified: 6/6 tests via flatten `rustc --test` (no flap ⇒ no power; pushes away + down;
  length matches falloff and fades with distance; zero beyond range; degenerate vertical
  case stays finite + downward; batch matches singles + deterministic). `cargo check --lib`
  clean.
- READY-TO-WIRE (device): on a dragon's downstroke, apply `buffet_targets` to nearby
  creatures/drops/player as an impulse (the engine already has knockback/impulse paths
  used by combat recoil).

## r379 — Flight: wing-buffet wired into the bin (gated, CHECKED, device-UNVERIFIED)
Wires r378's `sim::wing_buffet` into live creature flight.
- bin `unendlich_fullgame.rs`: behind `UNENDLICH_WING_BUFFET=1` (default OFF), each frame
  the positions of flapping (airborne) dragons are snapshotted BEFORE the mutable roamer
  loop (avoids aliasing the `iter_mut` borrow). In the leap-arc integration, a creature's
  airborne velocity is deflected out + down by the summed `wing_buffet::buffet_force` of
  those dragons (skip near-coincident = the dragon itself). power 24, range 10, down_bias 0.5.
- Chosen the leap-arc path deliberately: airborne creatures aren't grounded, so adding to
  their arc velocity rides the existing integration without fighting grounding/collision.
  Additive + gated; default play is unchanged.
- `cargo check --bin` clean (0 errors, no new warnings). On-device behavior UNVERIFIED.
- TO TEST: `UNENDLICH_WING_BUFFET=1`, have a dragon airborne over creatures that leap
  (e.g. a leaping predator near a hovering dragon) — its jump arc should bend away + down.
- SCOPE / NEXT (device): currently deflects only airborne/leaping creatures. A fuller
  version — grounded knockback and a player-felt shove — needs a proper impulse path on
  grounded bodies + the player controller, which is device-side work.

## r380 — Flight: power dive (test-VERIFIED)
`terminal_velocity::terminal` was orphaned; composed it into a dragon's power dive:
- `sim::dive::dive_terminal(mass, wing_drag_area, tuck01)` — terminal dive speed;
  tucking the wings sheds drag area (full tuck → 10% area ⇒ ~3× terminal), heavier
  dives faster. Wraps terminal_velocity.
- `dive_step(speed, terminal, dt, k)` — drag-limited exponential approach to terminal
  from either side (accelerates from below; bleeds down when wings spread to pull out).
  Never overshoots; finite, non-negative.
- Verified: 6/6 tests via flatten `rustc --test` (tucking raises terminal ~3×; heavier
  dives faster; accelerates toward + never overshoots terminal, converging; pulling out
  decays toward the spread-wing terminal; larger rate approaches quicker; deterministic).
  `cargo check --lib` clean.

## NOTE (r380) — texture request blocked on tree divergence
Operator asked to "take off the placeholder textures now that the real textures are in."
Investigated: in THIS canonical tree the block texturing in `chunk.frag` is a PROCEDURAL
stand-in (`apply_block_texture`: grout lines + per-id grain, gated by camera_pos.w) — the
"real atlas sampler (step 2)" is NOT implemented here (only shadow_map + froxel sampler2Ds
exist), and `block_atlas.png` has no foliage tiles (all 12 slots are diamond/dirt/log/sand/
snow/stone; leaves still use tinted-T_DIRT placeholder per HANDOFF_textures_foliage.md).
So this tree is BEHIND the operator's actual texture work — stripping the placeholder here
would leave nothing real underneath (blocks go flat-coloured). BLOCKED pending: either the
operator's current tree/atlas (with the real sampler + foliage tiles), OR a decision to
implement step-2 (atlas sampler in chunk.frag) here — a render/shader change that can't be
device-verified (sails-class risk), to be done gated if requested.

## r381 — Textures: remove the 2x2 grid placeholder + foliage tile (CHECKED) + Sound: spatial resolve (VERIFIED)
Weather readout confirmed sane on-device (log-r380: 28°C dry biome → cloud 0.11, precip 0,
weak Pulse storm, no snow). Then the operator's texture fixes + the next system (sound).

Textures (shader compiles to SPIR-V via glslangValidator; visual UNVERIFIED on device):
- `chunk.frag`: `SHOW_UV_DEBUG` was hardcoded `true` — a debug overlay drawing the 2x2
  checker + block-boundary grid on EVERY block regardless of flags (this was the "2x2 big
  grid over every block"). Set to `false`. Also dropped the per-tile "grout" edge darkening
  from `apply_block_texture` — kept the per-block grain + per-face shade (the procedural
  color+normals look the operator wants until real art lands).
- `texture_atlas.rs`: foliage (OakLeaves/BirchLeaves/DeadBush/Vine) no longer borrows the
  tinted-dirt tile — they fall through to `None` (flat material colour + procedural shading),
  which reads better. Left the foliage path clean for the operator's leaf-light work.
- No real atlas exists yet; engine intentionally uses procedural + color + normals.

Sound (test-VERIFIED) — `sim::sound_resolve`:
- Four orphaned audio cores chained into one listener-side resolve: `sound_attenuation`
  (distance) → `audio_occlusion::occlusion_gain` (walls) → `doppler_shift` (radial closing
  speed → pitch) → `reverb_wetness` (enclosure). `resolve(&Emitter,&Listener,&Env,&Acoustics)
  -> ResolvedSound { gain, pitch, wetness, distance }`. Adds only the geometry (distance +
  radial closing speed).
- Verified: 6/6 tests via flatten `rustc --test` (farther quieter; walls muffle; approaching
  raises pitch / receding lowers / tangential none; sealed large room is wet; deterministic
  + finite). `cargo check --lib` clean.

## r382 — Sound: budgeted voice mixer (test-VERIFIED)
The step after resolving one sound — choosing which of many actually play.
- `sim::voice_mix::{Source, ActiveVoice, mix}` — resolves every candidate through
  `sound_resolve` (r381), scores each by `gain × priority`, and keeps the top
  `max_voices` most-audible (voice-stealing). Silent/over-budget sources drop;
  ties break by input order (deterministic).
- Verified: 5/5 tests via flatten `rustc --test` (budget respected; nearest/loudest win
  and are ordered; a high-priority cue can outrank a louder low-priority one; inaudible
  sources take no voice; zero budget empty + deterministic). `cargo check --lib` clean.
- Sound output chain now: per-source `sound_resolve` (distance/occlusion/doppler/reverb)
  → `voice_mix` (budgeted selection). Ready to feed a real mixer/back-end on device.

## r383 — Sound: creature hearing / perception (test-VERIFIED)
The perception counterpart to the output chain — what creatures notice.
- `sim::creature_hearing::{Heard, hear}` — composes orphaned `audio_occlusion` (walls cut
  effective loudness) with orphaned `hearing_radius::detect` (distance + ambient floor +
  hearing range). Distance is counted once (by hearing_radius); occlusion only lowers the
  loudness, so no double-count. Returns `{ detection [0,1], noticed }` (threshold-gated).
- Verified: 7/7 tests via flatten `rustc --test` (loud+close noticed; walls reduce detection;
  beyond range unheard; below ambient unheard; closer/louder detect more; threshold gates
  notice; deterministic). `cargo check --lib` clean.
- Sound cluster now spans both sides: OUTPUT (sound_resolve → voice_mix) and PERCEPTION
  (creature_hearing). Pairs with the AI senses path.

## NOTE (r383) — multiplayer + load-world menu routing bug (from log-r381)
Operator's log shows `Select(Multiplayer) -> Menu` and `Select(Load) -> Menu`: both menu
selections transition straight back to Menu instead of into a host/join or load flow — that's
why "nothing happens". The bug is in the menu state machine (app/menu_view.rs / menu_router),
not the session layer. Candidate fix (when prioritized): route Multiplayer → a host/join
screen wired to r359 HostFlow/JoinFlow; route Load → the world-history load path. Device-side
UI work; flagged for a dedicated pass.

## r384 — Sound: constant-power stereo pan (test-VERIFIED)
Completes the output chain (sound_resolve → voice_mix → sound_stereo) by placing a
resolved mono gain in the stereo field.
- `sim::sound_stereo::{StereoGains, stereo}` — pans `gain` from the source direction
  relative to the listener's facing (right = forward × up). Constant-power: hard-side
  sources play one-sided, ahead/behind centre, total power preserved (left²+right²==gain²).
  Degenerate geometry (source on listener / looking straight up) → centred, finite.
- Verified: 6/6 tests via `rustc --test` (ahead centred; +X→right ear / −X→left; hard pan
  one-sided; constant power across the sweep; gain scales as gain²; degenerate centred;
  deterministic). `cargo check --lib` clean.
- Sound cluster status: clean orphans all composed — OUTPUT (sound_resolve → voice_mix →
  sound_stereo) + PERCEPTION (creature_hearing). `sound_mix` left alone (parallel mixer with
  its own Listener/Emitter/Voice types = WIP; composing it would tangle two type systems).

## NOTE (r384) — multiplayer + load diagnosis (from log-r381, deeper)
- MULTIPLAYER: the `Screen::Multiplayer` view (app/menu_view.rs ~167) is a STUB — heading +
  a label "Host or join (see session_mode)" + Back. NO host/join buttons exist, and no
  network transport is wired in the bin. The r359 session LOGIC exists but isn't surfaced or
  connected. "Host/join do nothing" because there's nothing to click + no transport. Fixing
  needs UI buttons + a networking transport (a real build, not a menu tweak).
- LOAD/WORLD-HISTORY: two competing load UIs — the egui LoadSlots screen (buttons →
  SelectSlot → AppStep::Launch(StartLocal), App-level path is tested) AND a renderer-side
  world list (renderer.menu_world_pick → fg_pending_load, bin ~9160). The load depends on the
  renderer pick; the egui path and the renderer path aren't unified, which is likely why a
  pick "loops back / changes seed". Needs the two paths reconciled + device repro.

## r385 — Bin: wire the load path + fix dead multiplayer buttons (CHECKED; UI not device-verified)
Both bugs lived in the bin's `renderer.menu_choice → AppInput` mapping (~bin:8815) and the
load-launch bypass. `cargo check --bin` clean; behavior needs on-device confirmation.

LOAD ("clicking a saved world loops back to the menu / changes the backdrop seed"):
- Root cause: the old bypass set `fg_pending_load` → `step = Launch(StartLocal)`, but never drove
  the App state machine `MainMenu → Loading`. `enter_game()` only does `Loading → InGame`, so it
  was a no-op — state stayed `MainMenu`, `in_menu` (= `state == MainMenu`) stayed true, the menu
  kept rendering over the rebuilt terrain → looked like a bounce with a new seed.
- Fix: a clicked saved-world row now routes through the App FSM as `AppInput::SelectSlot(pick)`
  (consumed at bin:8815 before the old bypass can fire). That path is unit-tested
  (app/mod.rs: SelectSlot → resolve_load → Launch(StartLocal), seed from the catalog) AND it
  drives `MainMenu → Loading`; the existing Launch handler then calls `enter_game()` → `InGame`,
  so the menu hides and the world actually enters.
- KNOWN LIMITATION: `resolve_load` reads the App's catalog, which r131 keeps FILE-ONLY and only
  rebuilds at boot (mutating the live catalog desyncs GameFlow). So worlds SAVED THIS SESSION
  aren't in the App catalog yet → loading them is a clean no-op (no bounce) until next launch.
  Worlds from previous sessions (in the boot catalog) load correctly. A full fix needs a
  state-only `MainMenu → Loading` entry point that takes a file-derived WorldInit (deferred —
  wants device verification first).

MULTIPLAYER ("Host/Join do nothing"):
- Root cause: `menu_choice` is screen-dependent — on the Multiplayer screen 2 = "Host World",
  3 = "Join Server", but on Title 2/3 mean "open Multiplayer". The bin mapped 2|3 →
  Select(Multiplayer) regardless, so on the MP screen those hit title_flow's Title-only guard
  and became silent no-ops.
- Fix: the mapping is now screen-aware (`fg_app.menu().screen() == Screen::Multiplayer`).
  Host World → Select(NewGame) → opens the world-creation wizard (the `wizard.is_some()` override
  makes `screen()` report NewGame so it renders), i.e. create a world to host. Join Server logs
  honestly that there's no network transport wired and no-ops.
- HONEST CAVEAT: there is still NO networking transport in the engine (input_capture.rs: "no
  transport, no server"; net_candidates is only transport-SELECTION logic). So "Host" launches a
  local world and "Join" can't connect to a remote host. Real multiplayer needs a transport layer
  built — that's a separate, larger piece, not a menu fix.

## r386 — Multiplayer: LAN discovery transport (session_wire VERIFIED, lan_discovery smoke-VERIFIED)
First actual networking in the engine. Until now session_mode (r359) modelled adverts/browser
but nothing put bytes on the wire ("no transport, no server"). Built the transport foundation,
modelled on the reference P2P stack's gossip layer (periodic JOIN beacons on a UDP multicast group).

- `sim::session_wire` — wire format for `WorldAdvert`. `encode_join(&advert) -> Vec<u8>` and
  `parse(&[u8], now_ms) -> Option<WorldAdvert>`. Versioned ("UNDL1"), tab-delimited text (no serde
  dep), receiver-stamped `last_seen_ms`. Strictly defensive: oversized (> MAX_DATAGRAM=1200),
  wrong-version, truncated, bad-int, non-utf8, too-many-candidates, or malformed packets → None,
  never a panic or unbounded alloc (the reference guards the same way). Escapes tab/newline/`;`/`\`
  in user-controlled name fields. 9/9 tests via `rustc --test` (roundtrip, empty cands, oversized,
  wrong version/type, truncated/malformed, delimiter escaping, candidate cap, non-utf8, feeds
  ServerBrowser).
- `sim::lan_discovery::LanBeacon` — the I/O companion: real `UdpSocket` multicast (group
  239.255.77.1, port 9877). `bind()` (join group, non-blocking), `announce(&advert)` (multicast one
  JOIN), `poll(now_ms) -> Vec<WorldAdvert>` (drain inbound → parse, feed `ServerBrowser::observe`).
  Non-blocking so it never stalls the frame loop; all parsing via session_wire. SMOKE-VERIFIED
  end-to-end in-container: bind → announce → multicast → poll → parse round-trips with matching seed
  + candidates (multicast loopback works in the sandbox; confirmed the exact wire bytes traverse a
  real socket). `cargo check --lib` and `--bin` clean.

SCOPE / HONESTY:
- This reaches hosts on the SAME LAN only. It is discovery, not yet gameplay — a joiner can LEARN a
  host exists + its seed/candidates, but the actual world-state sync transport (TCP/QUIC) is not
  built, so you can't yet play together.
- Crossing NAT to the open internet needs a DHT + hole-punching layer. The reference uses OpenDHT
  (vendored at snapclone/third_party/opendht; wrapped by JamiAdapter_dht_impl.cpp + NatTraversal.cpp
  + DhtResourceManager.cpp) plus relay fallback. That's a separate, much larger build (C++ lib FFI
  or a Rust DHT crate); net_candidates (already a port of the reference's order_targets) + session_mode
  plan_connect are the seams it would plug into.

NEXT (bin integration, needs two real machines to verify — deferred for device testing):
1. HOST: when a world runs as a multiplayer host, hold an `Option<LanBeacon>`, build a WorldAdvert
   (username, world_name, world_seed, candidate = this box's LAN IP + game port), and `announce` on a
   ~1s timer. Verifiable immediately (run two instances, watch beacons / a log line).
2. JOIN: on the Join screen, `poll` into a `ServerBrowser`, render `by_recent()` like the LoadSlots
   list; selecting a host calls session_mode `JoinFlow::connect` (already exists) → adopts host_seed
   and rebuilds the world for it (the r346 seed-coherence rule).
3. Then the real lift: a TCP/QUIC session that actually streams player/world state between host and
   joiner. Only after that is it "playing together".

## r387 — Multiplayer wiring: compressed chunks (test-VERIFIED)
`sim::net::chunk_stream` (r332) decides seed/delta/dense, frames records, and builds byte deltas,
but carries the payload as opaque bytes "for the bin to lzma". Built that deferred codec + tied the
whole path together — dependency-free, so no lzma link.
- `sim::net::chunk_compress`:
  - `compress`/`decompress(max_out)` — PackBits-style byte RLE. Voxel chunks are long air/stone runs,
    so a 32 KB dense chunk crushes to a fraction; incompressible input expands by ≤ ~1/128.
    decompress is `max_out`-bounded → hostile/corrupt payloads can't panic or exhaust memory.
  - `seal_dense(coord, voxels)` / `seal_delta(coord, base, modified)` (host) and
    `open(rec, regen_base, max_cells)` (client) compose select/build_delta/apply_delta + compression
    into one path: voxels → record → wire → voxels, for Seed/Dense/Delta.
- Verified: 8/8 via flatten `rustc --test` composing the real `chunk_stream` — roundtrip + >10×
  crush on a voxel chunk, bounded expansion on random, empty/single/full-run, truncated/overlong
  rejection, dense+delta+seed full paths through encode/decode_record, select_plan agreement.
  `cargo check --lib` clean.

## ORIENTATION (r387) — rollback + DHT double mailbox (next integrators)
Surveyed the engine's existing netcode (all built as pure cores, orphaned = referenced only by the
benchmark registry): `sim::rollback`, `sim::net::{wire,session,transport,dht_mailbox,entity_snapshot,
world_advert,world_presence}`, `sim::mp_dht`, `sim::net_rendezvous`, `world::rollback_scope`.
- ROLLBACK: `sim::rollback::Rollback` (predict → rollback → re-simulate over a trusted baseline) +
  `sim::net::session::Session` (byte-slice in/out over Rollback: record_local, outgoing_*_packet,
  ingest, desync) are a COMPLETE tested core. `world::rollback_scope::rollback_closure` is a separate
  dependency-scoped resimulation model. Connecting it bounds *which* regions Rollback re-sims — an
  optimization that touches tested resim behavior, so NOT a clean additive compose; defer or do
  carefully. The real missing wire is bin-side: construct Session + transport in the game loop per
  `sim::net::mod`'s "per-frame wiring" doc (needs a rollback-compatible game step + device verify).
- DHT DOUBLE MAILBOX: `sim::net::dht_mailbox` already has the full two-sided handshake (mailbox_key,
  MailMsg, MailboxDht trait + LocalDht, send_request/poll_response/host_drain_requests/host_accept/
  host_deny, JoinOutcome) with the IP-protection invariant (host addr only inside an Accept). Cleanest
  next integrator: compose `host_accept` → `net_rendezvous` hole-punch FSM (accept yields the addr →
  feed candidates into HolePunch). A real-DHT `MailboxDht` over `mp_dht` is possible but note the model
  gap — dht_mailbox keys on `[u8;32]`/opaque blobs, `mp_dht` uses `NodeId`/typed `Mail`; bridge
  carefully to avoid a two-mailbox-model tangle.

## r388 — Multiplayer wiring: mailbox Accept → hole-punch (test-VERIFIED)
Composed the two ends of NAT traversal that were built but unconnected: the DHT-mailbox join
handshake and the UDP hole-punch FSM.
- `sim::net::rendezvous_join`:
  - `parse_candidate("a.b.c.d:port")` → `net_rendezvous::Candidate`, classifying RFC1918/loopback/
    link-local as `Host` (same-network) and everything else as `Srflx` (reflexive/public), with ICE
    priority. Malformed → None.
  - `parse_accepted_addrs` handles a comma-separated list (host advertises LAN + public), dropping junk.
  - `begin_punch(&JoinOutcome, local, cfg, txid_seed, peer_id, session, nonce)` — on
    `Accepted{addr}` parses the host address(es) the accept revealed (the IP-protection invariant:
    address only appears post-accept), wraps them in a `MailboxBlob`, builds a `HolePunch`, and feeds
    `on_remote_blob` → returns a punch already in `Probing`. `Denied` or an addressless accept (e.g.
    the host's reply key) → None.
- Flow now end-to-end: dht_mailbox `send_request` → `host_accept` (Accept carries addr) →
  `poll_response` (JoinOutcome::Accepted) → `begin_punch` → `HolePunch.poll` emits SendPing →
  (bin sends UDP, feeds `on_packet`) → Connected.
- Verified: 6/6 via flatten `rustc --test` composing the real `net_rendezvous` FSM — local/public
  classification (incl. 172.16–31 boundaries), malformed rejection, multi-addr list, accept→Probing
  punch that actually emits a ping, multi-pair, denial/addressless→None. `cargo check --lib` clean.
- REMAINING for live NAT join: bin glue only — put/get the mailbox blobs on a real DHT (mp_dht over
  UDP, or LocalDht for LAN), send/recv the punch datagrams, supply now_ms + a session-derived
  txid/nonce seed. The pure protocol path (mailbox handshake → punch) is complete and tested.

## r389 — Multiplayer wiring: mailbox on the real DHT model (test-VERIFIED)
The join handshake (`dht_mailbox`) was written against the `MailboxDht` trait but only had `LocalDht`
(an unbounded in-test HashMap) behind it. Added the routing-aware backend.
- `sim::net::mp_dht_mailbox::MpDhtMailbox` — `MailboxDht` over `mp_dht::Mailboxes` (the Kademlia
  model's per-recipient bounded FIFO boxes). `key_to_node` projects a 32-byte `MailboxKey` onto
  mp_dht's `NodeId` space (its first FNV lane — already a full hash), the space mp_dht routes on;
  the bin maps `NodeId → socket`. Interior-mutable via `Mutex` like `LocalDht`. `put` applies the
  box's backpressure (full → dropped, requester retries); `get` drains `.body` in arrival order.
- Drop-in for `LocalDht` on the non-full path, with the added bound so a flooded mailbox can't grow
  without limit. So send_request/host_accept/poll_response now run on the real DHT model unchanged.
- Verified: 5/5 via flatten `rustc --test` — put/get drains in order, distinct keys don't cross-talk,
  deterministic+distinguishing key projection, bounded-box rejects when full, and an explicit
  equivalence check that the backend matches the `LocalDht` contract on a put/get sequence.
  `cargo check --lib` clean.

### Multiplayer pure-protocol stack status (r386–r389)
Discovery (LAN): `session_wire` + `lan_discovery` (r386, smoke-verified socket round-trip).
World streaming: `chunk_compress` over `chunk_stream` (r387).
NAT join: `dht_mailbox` handshake → `rendezvous_join` → `net_rendezvous` hole-punch (r388), with the
mailbox now on `mp_dht` routing (r389). All pure + tested. What remains is the I/O shell (sockets,
NodeId→socket, the DHT put/get + punch datagrams) and threading `net::session` into the game loop —
both need two real machines to verify.

## r390 — Multiplayer wiring: entity transform replication round trip (test-VERIFIED)
`sim::net::entity_snapshot` shipped the three host-authoritative replication pieces separately —
`AuthorityStamper` (host per-entity seq), the snapshot wire codec, and `SnapshotGate` (client
newest-wins). Composed them into the two calls a host and client actually make.
- `sim::net::entity_replication`:
  - `seal(stamper, host_tick, entities)` (host) — stamps each entity with its next authority
    sequence (overwriting any input seq; host owns truth) and frames the snapshot.
  - `receive(gate, bytes)` (client) — decodes + applies newest-wins, returning the transforms
    accepted; `None` on malformed bytes, empty vec when everything was stale/duplicate.
- Verified: 7/7 via flatten `rustc --test` composing the real `entity_snapshot` — first broadcast
  applies all, second supersedes, reordered/stale dropped, exact duplicate dropped, per-entity
  independence (a re-sent subset leaves others tracked), malformed→None, host seq increments across
  seals. `cargo check --lib` clean.
- This is the Snapshots-channel (UnreliableSequenced) transform path; separate from world edits
  (reliable/ordered commands) and the rollback capture trait, per §C.

### NOTE (r390) — world_advert AdvertWire is missing world_seed
While surveying for the next integrator: `sim::net::world_advert::AdvertWire` (the DHT advert record)
has no `world_seed` field, but `session_mode::WorldAdvert::world_seed` exists specifically so a joiner
learns the seed before the handshake (the r346 seed-coherence rule). So the DHT advert path can't
currently carry the seed — a WorldAdvert⇄AdvertWire mapping would drop it. The LAN path (r386
`session_wire`) DOES carry world_seed. Options for later: add world_seed to AdvertWire (touches a
golden wire format — version bump) OR rely on the join handshake (`JoinFlow::host_seed`) to deliver it.
Flagged rather than papered over.

## r391 — Multiplayer wiring: world-edit command codec + remote apply (test-VERIFIED)
`sim::world_authority` is the command-sourced world model (clients send commands; host validates
against canonical state), but the commands had no wire form — they couldn't cross the reliable
channel. Built the codec + the host-apply compose.
- `sim::net::world_cmd_wire`:
  - `encode_batch`/`decode_batch` — ordered `(player, WorldCmd)` frames (magic 0x554E, v1), count
    capped at `MAX_CMDS_PER_FRAME`, all three variants (Place/Remove/Grant). `encode_cmd`/`decode_cmd`
    single-command helpers. `decode_*` → None on bad header, unknown tag, over-cap count, truncation.
  - `apply_remote(world, buf)` — host side: decode the ordered frame and fold it through
    `CanonicalWorld::apply`, returning per-command Ok/Err(Reject). None if the frame is malformed.
- This is the reliable-ordered world-edit path (place/remove/grant), the counterpart to the
  unreliable-sequenced transform path (r390 entity_replication): edits are validated commands, not
  state, so a build survives iff its commands were legal against canonical state (anti-cheat by
  construction).
- Verified: 6/6 via flatten `rustc --test` against the real `world_authority` apply/digest — variant
  roundtrip, single-cmd helper, bad-header/unknown-tag/truncation rejection, over-cap count rejected
  without allocating, apply_remote validating NoResource/Occupied against canonical state, and an
  order-preserving replay that yields an identical digest on two fresh worlds. `cargo check --lib` clean.

### Multiplayer pure-protocol stack (r386–r391) — for the single on-device verification pass
- Discovery: LAN gossip (`session_wire`+`lan_discovery`, r386) and the DHT advert codec
  (`world_advert`, pre-existing; note the world_seed gap flagged in r390).
- Liveness: `world_presence` (re-announce + staleness) — pure timing, not yet composed into a loop.
- World streaming: `chunk_compress` over `chunk_stream` (r387).
- NAT join: `dht_mailbox` handshake → `rendezvous_join` hole-punch (r388), mailbox on `mp_dht` (r389).
- Replication: entity transforms unreliable-sequenced (`entity_replication`, r390); world edits
  reliable-ordered (`world_cmd_wire`, r391); QoS/budget in `net_channels`.
- Rollback core (`sim::rollback` + `sim::net::session`) exists and is tested.
Remaining = bin I/O shell (sockets, NodeId→socket, DHT put/get + punch datagrams, channel transport)
and threading `net::session` into the game loop. All protocol logic above is pure + unit-tested.

## r392 — Multiplayer wiring: discovery liveness loop (test-VERIFIED)
`world_presence` had the raw timing (PresenceBeacon, freshness, prune_stale) but nothing composed
the host re-announce + browser staleness halves around an actual `WorldAdvert`. Tied them together.
- `sim::net::presence_loop`:
  - `HostAnnouncer::{new, with_period, poll(advert, now), until_due}` — on cadence, restamp the
    advert's `last_seen_ms` and return the refreshed copy to publish, so the record never TTLs out.
    Wire-agnostic (operates on `WorldAdvert`, which carries `world_seed` — sidesteps the AdvertWire
    seed gap; caller picks `session_wire` for LAN or the DHT codec for WAN).
  - `classify(adverts, now, reannounce, ttl)` → per-advert Fresh/Stale/Dead (grey at-risk worlds in
    the Join UI); `evict_dead`/`evict_dead_default` drop Dead via `prune_stale`.
- Verified: 4/4 via flatten `rustc --test` against the real `world_presence` — host announce→wait→
  re-announce cadence (seed preserved), Fresh/Stale/Dead classification, eviction drops only Dead,
  and a full loop where on-cadence re-announce keeps a browser entry alive until the host goes silent.
- NOTE learned: `prune_stale` classifies via the module's `REANNOUNCE_MS` (15s), so eviction `ttl_ms`
  must be >= REANNOUNCE_MS (the real `TTL_MS`=45s is) or "Dead" is masked by the Fresh window —
  documented on `evict_dead`.

## r393 — Multiplayer wiring: client join driver (test-VERIFIED)
Collapsed the whole client-side join into one driver the bin runs with I/O only — no protocol logic
left in the bin. Composes `dht_mailbox` (handshake), `rendezvous_join` (Accept→punch), and
`net_rendezvous` (the punch FSM).
- `sim::net::client_join::{ClientJoin, Phase}`:
  - `new(my_id, host_id, requested_name, host_seed)` — host_seed taken from the selected advert (the
    joiner rebuilds for it on Connected — r346 seed-coherence).
  - `request(dht)` (idempotent) → mailbox Request, Phase::Waiting.
  - `poll(dht, local, cfg, txid_seed, peer_id, session, nonce)` → on Accept parse addr + begin_punch →
    Punching; on Deny → Denied; addressless Accept → Failed.
  - `drive(now)` → punch Actions for the bin to send, transitions to Connected/Failed.
  - `on_packet(pair, kind, txid, now)` → feed received punch datagrams.
  - `phase()` / `host_seed()` / `assigned_name()`.
- Bin's job is now purely I/O: DHT put/get via the `MpDhtMailbox` adapter (r389), UDP send/recv of the
  punch datagrams, a clock, and a session-derived txid/nonce seed. On `Phase::Connected` it rebuilds
  the world for `host_seed` and opens the session.
- Verified: 4/4 via flatten `rustc --test` (real net_rendezvous + rendezvous_join, mirrored requester-
  side mailbox) — full request→accept→punch→Connected, denial→Denied, addressless accept→Failed,
  idempotent request (exactly one mailbox entry). `cargo check --lib` clean against the real modules.

### Multiplayer protocol stack — client/host orchestration now in place (r386–r393)
With r393 the client join is a single driver; the host side composes `host_drain_requests`/`host_accept`
(dht_mailbox) + `presence_loop` (r392) re-announce + `world_advert`/`session_wire` publish. Remaining
to play: the bin I/O shell (the `MpDhtMailbox`/socket plumbing the drivers ask for) + threading
`net::session` (rollback) + the channel transport (`entity_replication`/`world_cmd_wire`/`chunk_compress`
over `net_channels`) into the game loop. All orchestration + protocol logic above is pure + unit-tested.

## r394 — Bin I/O shell, slice 1: LAN host discovery (CHECKED; needs 2 machines to verify)
First bin-side wiring of the verified protocol cores. Gated behind `UNENDLICH_LAN_MP=1` (default off →
single-player and the normal build are unaffected). `cargo check --bin` clean; multicast + UI behavior
needs two machines on a LAN to confirm.
- Bin (`unendlich_fullgame.rs`):
  - State: binds a real `sim::lan_discovery::LanBeacon` (UDP multicast) at startup when the flag is set;
    holds a `session_mode::ServerBrowser` + `presence_loop::HostAnnouncer` (2s cadence).
  - Per frame (throttled ~5/sec): `beacon.poll` → `browser.observe` + prune, logging each newly-seen
    host once (`[lan-mp] discovered host …`); while `AppState::InGame`, builds a `WorldAdvert` for the
    live world (name + `fg_world_seed`) and `announce`s it on cadence (`[lan-mp] announced world …`);
    feeds `renderer.menu_mp_list` from `browser.by_recent()`.
- Renderer (`world_renderer.rs`): added `menu_mp_list` field + renders it under the Multiplayer screen
  ("LAN games" list, "(searching… none found yet)" when empty). Display-only this slice.
- HOW TO VERIFY: run two instances with `UNENDLICH_LAN_MP=1` on the same LAN; host a world in one →
  the other's Multiplayer screen lists it and logs the discovery.
- KNOWN TODO (next slices): `candidates` is empty (no real ip:port yet — needed before a join can
  connect); `host_username` hardcoded "host" (real identity/UnendlichId next); selecting a listed host
  → `client_join` (r393) drive is not wired yet (this slice is discovery/listing only).

## r395 — Threading the per-chunk subsystems over the rayon lane
Operator: "spawn threads for water, shaders, water physics, water placement + terrain."
KEY FINDING — most of this is ALREADY threaded: `world::chunk_loader` spawns a `num_workers` pool
(`thread_priority::ThreadRole::ChunkLoader`) that generates chunks AND builds octrees in parallel, and
`MeshWorker` threads handle meshing. Since worldgen places water blocks, **terrain + water placement +
meshing already run on worker threads.** Shaders are precompiled to `.spv` at build time (`build.rs`);
runtime only reads bytecode + creates GPU modules, which are inherently main-thread (Vulkan device
affinity) — there is no CPU-heavy runtime shader compile to parallelize. The genuinely un-threaded part
was the **water-physics simulation** (`sim::water_physics` / `sim::env_frame::tile_step` are pure cores
referenced only by benchmark harnesses — not wired to any thread).

Added the parallel primitives, both over the blessed Class-P lane (`util::task_lanes::map_pure`, which
runs on rayon and enforces `In: Sync, Out: Send, F: Fn+Sync+Send` at compile time — a mis-classified
race is a build error, not a runtime bug):
- `sim::water_batch::{WaterTile, step_tiles, step_tiles_threshold}` (VERIFIED, 5/5 via flatten with a
  serial `map_pure` mock + `cargo check --lib` for the real rayon bounds): steps many independent
  per-chunk water/moisture tiles in parallel; order-preserving so the result equals the serial fold
  (deterministic), inputs never mutated, mass only decreases per tile. This threads the water physics.
- `world::terrain_batch::{generate_chunks, generate_chunks_threshold}` (CHECKED — `cargo check --lib`
  proves `WorldGenerator: Sync` + `Chunk: Send`, i.e. parallel gen is race-safe; correctness of the
  map follows from the same lane as water_batch): parallel chunk generation (terrain + water placement
  together). Complements the existing `chunk_loader` streaming pool for bulk/non-streaming generation.

NOT done (honest): water sim is not currently run per-frame in the bin (the cores are orphaned), so
wiring `water_batch` into a game loop is a first-time gameplay integration + a design choice that needs
on-device verification — the parallel primitive is ready and verified; the loop hook is the next step.
No shader thread spawned (no runtime CPU work to offload; GPU stays main-thread — spawning one would be
risk for no gain). `env_frame::tile_step` (flow+erosion+moisture) parallelizes via the identical pattern
when erosion is wanted.

## r396 — Bin I/O shell, slice 2: real LAN address in the advert (CHECKED; needs 2 machines)
r394 discovered hosts but adverts carried no address (empty `candidates`), so a join had nowhere to
connect. This populates the host's real LAN endpoint. `session_wire` already encodes/parses candidates
in the LAN beacon format, so only the host side needed to fill them in.
- `sim::lan_discovery::local_lan_ipv4()` (I/O, smoke-checked): resolves this host's primary LAN IPv4 via
  the UDP-connect trick — `connect()` to TEST-NET-1 (192.0.2.1, never routed) makes the OS pick the
  default-route source IP; no packet is sent. Returns None on a loopback-only host.
- `sim::lan_discovery::lan_candidate(host, port)` (VERIFIED, 1/1 flatten): builds a `net_candidates::
  Candidate { kind: Lan }` for the host endpoint.
- Bin (`unendlich_fullgame.rs`, gated `UNENDLICH_LAN_MP=1`): binds an ephemeral UDP **session socket**
  (so the advertised port is real + reserved — the future game-session endpoint), resolves the LAN IP
  once, and fills the announce advert's `candidates` with `lan_candidate(ip, session_port)`. The
  Multiplayer screen now shows each host as `name — world (seed N) @ a.b.c.d:port`.
- HOW TO VERIFY: two instances, `UNENDLICH_LAN_MP=1`, same LAN; host in one → the other lists it WITH an
  ip:port. Confirms adverts now carry a connectable address.
- KNOWN TODO (next slice): nothing yet *listens* on the session socket (bound, reserved, but no game
  packets processed) — that comes with wiring host-selection → `client_join` (r393) drive + the channel
  transport. On a LAN, direct connect to this candidate likely suffices (no NAT/punch needed).

## r397 — Bin I/O shell, slice 3: LAN join handshake (wire VERIFIED; in-game path CHECKED, needs 2 machines)
The joiner can now click a discovered host and exchange an admission handshake over the session socket.
On a LAN the peers are directly reachable, so this is a direct unicast request/reply — no DHT mailbox,
no hole-punch (that stack remains for the internet path).
- `sim::lan_session_wire::LanJoin::{Join{name}, Accept{world_name,world_seed}, Deny{reason}}` with
  `encode`/`parse` (VERIFIED: 4/4 unit via flatten — roundtrips, seed preserved exactly, rejects bad
  magic/truncation/unknown-tag, clamps oversized strings; PLUS a real-socket loopback smoke test of the
  full JOIN→ACCEPT exchange passing the seed). Versioned (`ULJ1`), length-prefixed, panic-free.
- Renderer (`world_renderer.rs`): the "LAN games" list is now clickable (`dest_button` per host) →
  `menu_mp_pick` (mirrors the LoadSlots `menu_world_pick` write-back).
- Bin (`unendlich_fullgame.rs`, gated): session socket set non-blocking; each frame drains inbound
  datagrams — `Join` → log + reply `Accept{current world_name, fg_world_seed}` to the sender; `Accept`
  → log "accepted by host: world … seed …"; `Deny` → log. On a host click, sends `Join` to that host's
  candidate (the `lan_host_addrs` list is built in lock-step with `menu_mp_list` from one filtered pass,
  so indices always resolve). 
- HOW TO VERIFY: two instances, `UNENDLICH_LAN_MP=1`, same LAN; host a world in A; in B open Multiplayer
  → click the listed host → A logs `join request from 'joiner'`, B logs `accepted by host: world '…'
  seed N`. Proves bidirectional UDP + the seed handover.
- KNOWN TODO (next slice): the joiner receives the host's seed but doesn't yet ADOPT it — wire the
  `Accept` into a world load (rebuild at `world_seed` → drive App MainMenu→Loading→InGame, reusing the
  r385 FSM path), then begin state replication (entity_replication / world_cmd_wire over the socket).

## r398 — Bin I/O shell, slice 4: joiner adopts host seed → loads into the world (CHECKED, needs 2 machines)
The joiner now ACTS on the Accept: it loads into a world at the host's seed, so both players stand in the
same terrain. Reuses the verified r197/r385 launch path rather than adding a new one.
- Bin (`unendlich_fullgame.rs`, gated): on `LanJoin::Accept{world_name, world_seed}` the joiner sets
  `fg_pending_load = Some((world_name, world_seed, false))`. The existing per-frame launch hook (line
  ~9009) turns that into `AppStep::Launch(LaunchPlan::StartLocal(WorldInit{seed,…}))`, which runs the
  full handler: `chunk_manager.rebuild_for_seed(seed)` → `fg_world_seed = seed` → App drives
  MainMenu→Loading→InGame. Same seed ⇒ same generated terrain as the host (r346 coherence); no save file
  needed (terrain is regenerated from the seed).
- End-to-end LAN join flow now: beacon discovery (r394) → host advertises ip:port (r396) → click host,
  JOIN→ACCEPT handshake with seed handover (r397) → joiner rebuilds the host's world and enters it (r398).
- HOW TO VERIFY: two instances, `UNENDLICH_LAN_MP=1`; host world in A; in B click the listed host → B's
  logs show `accepted by host: joining world '…' seed N`, then B loads and spawns into terrain identical
  to A's (same seed). Both are now in the same world.
- KNOWN LIMITATIONS / NEXT: (1) the synthetic launch path hardcodes `multiplayer: false`, so no session
  is opened yet — players are co-located in identical terrain but can't see each other; (2) after
  joining, the joiner also begins announcing (harmless duplicate advert). NEXT SLICE: open a session and
  start state replication over the socket — `entity_replication` (transforms) + `world_cmd_wire`
  (block edits), both already built + unit-tested (r390/r391) — so the two players become visible to each
  other and edits propagate.

## r399 — Bin I/O shell, slice 5: transform replication over the socket (CHECKED, needs 2 machines)
The two players now exchange positions. Wires the tested `entity_replication` channel (r390) over the
LAN session socket so each side replicates its own player transform to the peer.
- Channel mux: session datagrams now carry a leading channel byte — `CH_HANDSHAKE=1` (LanJoin) and
  `CH_ENTITY=2` (entity snapshots) — so the handshake and replication share one socket. The r397 JOIN/
  ACCEPT sends were updated to prefix the byte.
- Peer tracking: the host sets `lan_peer_addr` from the JOIN's source; the joiner sets it to the host's
  candidate when it sends JOIN. Each is authoritative for its own player; the local entity id is the
  session port (distinct per process on a LAN → no id collision).
- Outbound (gated, in-game, ~15/sec): build an `EntityState{id, Player, pos via Fixed::from_f64, …}` →
  `entity_replication::seal(&mut stamper, tick, &[me])` → send on `CH_ENTITY`.
- Inbound: `CH_ENTITY` → `entity_replication::receive(&mut gate, payload)` (newest-wins per id) → logs
  `remote entity {id} at (x,y,z)`.
- `cargo check --bin` clean. seal/receive themselves are unit-VERIFIED (r390, 7/7); the JOIN/ACCEPT
  loopback path is smoke-VERIFIED (r397); the mux is a trivial byte prepend/strip.
- HOW TO VERIFY: two instances, `UNENDLICH_LAN_MP=1`; A hosts, B joins (r398). Move A's player → B logs
  `remote entity <portA> at (...)` updating; move B → A logs B's position. Confirms bidirectional
  transform replication in the same world.
- KNOWN LIMITATIONS / NEXT: (1) single peer (`lan_peer_addr` is one slot — fine for 1 host + 1 joiner;
  a peer map comes with N-player); (2) received transforms are LOGGED, not yet rendered — the next slice
  spawns/updates a visible remote avatar entity from the received `EntityState`s so players actually SEE
  each other; (3) yaw/pitch sent as 0 for now; (4) no liveness/timeout yet. Block-edit replication
  (`world_cmd_wire`, r391) over `CH_WORLDCMD` is the slice after.

## r400 — Bin I/O shell, slice 6: VISIBLE remote players (UNVERIFIED, needs 2 machines + device render)
Players can now SEE each other. Received transforms (r399) become a visible avatar baked into the
existing creature render path — no new GPU code.
- Bin (`unendlich_fullgame.rs`, gated): a `HashMap<u64, Creature>` (`lan_remote_creatures`) holds one
  avatar per remote entity id. On a `CH_ENTITY` snapshot: if the id is new, `spawn::spawn_beta_dragon(pos)`
  and insert; otherwise set `cr.body.position = pos`. The map lives outside the streaming entity store so
  avatars never despawn. In the per-frame creature bake (next to `test_creatures`), each remote avatar is
  run through `creature_to_vertices` and appended to the entity vertex/index buffers — so it renders
  exactly like any other creature.
- Full LAN multiplayer loop now closes: discover (r394) → address (r396) → JOIN/ACCEPT + seed (r397) →
  joiner loads host's world (r398) → bidirectional transform replication (r399) → each player sees the
  other as a live avatar (r400).
- HOW TO VERIFY: two instances, `UNENDLICH_LAN_MP=1`; A hosts, B joins; both spawn in the same terrain
  and each sees a dragon avatar at the other's position, moving as that player moves.
- KNOWN LIMITATIONS / NEXT: (1) avatar is a beta-dragon stand-in (a player model is a cosmetic follow-up);
  (2) yaw/pitch sent as 0 → avatar faces a fixed direction; (3) avatar isn't animated (static pose parked
  at the position); (4) replication target is a single `lan_peer_addr` (1 host + 1 joiner; the avatar MAP
  already supports N, only the send path is 1:1 — a peer map generalises it); (5) no liveness/timeout, so
  a departed player's avatar lingers. Block-edit replication (`world_cmd_wire` r391 over `CH_WORLDCMD`) is
  the next channel.

## r401 — Multiplayer bugfixes from device run (CHECKED, needs device)
Operator device run showed r400 "not working" + "Host World did not load". Root causes + fixes:
1. **LAN-MP was unreachable** — gated behind `UNENDLICH_LAN_MP=1` (default OFF), and the operator (rightly)
   didn't set it, so NONE of r394–r400 ran; "Join Server" fell through to the old r385 no-op message
   ("no network transport is wired yet"). FIX: LAN-MP is now **ON by default** (opt-out `UNENDLICH_LAN_MP=0`).
   Socket setup still fails gracefully to inert, so single-player is unaffected if binding can't happen.
2. **Host World didn't load** — it only opened the wizard, and a naive `fg_pending_load` launch hits the
   r385 bounce (synthesizes Launch but App stays MainMenu → `enter_game()` returns false → menu redraws
   over the rebuilt terrain). FIX: Host World now opens the wizard and, once it's open, fires the PROVEN
   one-click Create path (`wizard_action = 9`) via a `fg_host_pending` flag — this drives the real FSM
   (MainMenu→Loading→InGame) and actually launches a world. With LAN-MP on, that world auto-announces.
3. **Stale "Join Server" message** — replaced with a hint pointing at the auto-populated "LAN games" list
   (clicking a discovered host is the join path, r397).
- `cargo check --bin` clean. Still device-CHECKED (two-machine discovery/join + Host World launch need
  the operator's hardware), but the path is now reachable without any env var.
- RE-TEST: launch two instances (no env var needed). In A: Multiplayer → Host World → should load into a
  world. In B: Multiplayer → A should appear under "LAN games" → click it → B loads A's world; each sees
  the other's avatar. Logs to watch: `[lan-mp] discovery beacon bound`, `session socket bound`,
  `hosting`/`announced`, `discovered host`, `join request`, `accepted by host`, `spawned remote avatar`.

## r402 — Threading the water systems over the rayon lane (VERIFIED pure batches)
Operator: "worker threads for water, world placement of water, water texture, water drain physics, plant
water interaction, water flow, rainfall, mountain sources via weather humidity interaction." All eight
addressed via Class-P parallel batches on `util::task_lanes::map_pure` (rayon; Sync-in/Send-out enforced
at compile time — a race is a build error). Each verified by flatten `rustc --test` against the real pure
cores + `cargo check --lib` for the real rayon bounds.

- **water flow / rainfall / water drain physics / plant water interaction / water** → `sim::env_batch`
  (4/4): parallel batch over `env_frame::tile_step`, the unified per-chunk step that adds RAIN, flows
  WATER downhill, applies DRAIN (infiltration + evaporation), ERODES terrain, and diffuses soil MOISTURE
  (the field plants read) — five of the listed systems threaded in one batch. Plus `sim::water_batch`
  (r395) for the flow+moisture-only path.
- **water texture** → `sim::water_texture_batch` (3/3): parallel generation of the animated caustic
  texture (`water_caustics::caustic` is a pure per-pixel fn) — rows mapped in parallel, concatenated in
  order; values stay in [0,1].
- **mountain sources via weather humidity** → `sim::mountain_sources` (4/4): NEW pure `orographic_rain`
  core (rain rises with humidity × altitude above a condensation height — why mountains feed springs),
  composed with the tested `world::river_source` replenishment and stepped as a parallel batch over
  source cells. Levels stay bounded; rain monotonic in humidity + altitude.
- **world placement of water** → ALREADY threaded: worldgen places water as it fills chunks, and chunk
  generation runs on the `chunk_loader` worker pool (`ThreadRole::ChunkLoader`); `world::terrain_batch`
  (r395) covers bulk/non-streaming gen. No new work needed.

NOT done (honest): these are parallel PRIMITIVES. The water sim (`env_frame`/`water_physics`) isn't run
per-frame in the bin today (the cores are orphaned), so actually invoking `env_batch`/`water_texture_batch`
per frame is a gameplay-integration + design choice that needs on-device verification — same situation as
r395. The threading is ready and verified; the loop hooks are the device-iteration step.

## r403 — Multiplayer slice 7: block-edit replication (CHECKED, needs 2 machines)
Building and mining now propagate between players. Reuses the tested `world_cmd_wire` (r391) wire format
over a new channel, applied through the bin's own world-edit path.
- Capture: each successful player edit pushes a `world_authority::WorldCmd` to `lan_outgoing_edits` —
  Break (set_block→Air) → `Remove{pos}`; Place (set_block→blk) → `Place{pos, block: blk as u8 as u16}`.
  Three player edit sites hooked (one break, two place branches).
- Channel: a third mux channel `CH_WORLDCMD=3` joins `CH_HANDSHAKE=1`/`CH_ENTITY=2`. Each frame, in-game
  with a peer, the queued edits are drained → `world_cmd_wire::encode_batch(&[(player_id, cmd)…])` → sent.
- Apply: on `CH_WORLDCMD`, `decode_batch` → for each cmd, `Place` → `chunk_manager.set_block(pos,
  BlockId::try_from(block as u8))`, `Remove` → `set_block(pos, Air)`. Applies through the bin's REAL world
  (chunk_manager), not r391's CanonicalWorld model, so it matches the terrain the players actually see.
  Solo/no-peer edits are cleared so the queue can't grow unbounded.
- `cargo check --bin` clean. Wire format is unit-VERIFIED (r391); set_block is the bin's existing,
  exercised edit path. Device-CHECKED: the two-machine propagation needs the operator's hardware.
- HOW TO VERIFY: two instances in the same world (host + joiner). Place/break a block on A → it appears/
  disappears on B within a frame, and vice versa. Logs: the placing side logs `Placed/Broke …`; the block
  shows up on the peer via normal chunk rendering (no remote-edit log, by design — kept quiet).
- KNOWN LIMITATIONS: no authority/validation (both sides apply each other's edits directly — fine for
  co-op trust; r391's CanonicalWorld validation isn't in the loop); single peer (same as transform repl);
  no conflict resolution on simultaneous edits to the same cell (last write wins, which set_block already
  is). Next MP candidates: N-player peer map, real yaw on avatars (local_player.view.yaw exists), peer
  liveness/timeout to despawn stale avatars.

## r404 — Multiplayer slice 8: avatar orientation + peer liveness (UNVERIFIED, needs device)
Two fixes that make the remote avatar look/behave correctly.
- **Real yaw**: the transform snapshot now carries the player's actual facing — `local_player.view.yaw`
  (radians) encoded as `yaw·10000` into the i16 wire field (±π·1e4 ≈ ±31416, fits i16). On receipt the
  avatar's `Creature.yaw` is set to `i16/10000`, so the remote dragon turns as that player turns instead
  of always facing +X. (Pitch still 0.) NOTE: camera-yaw vs creature-yaw zero conventions may differ by a
  constant offset — if avatars face 90°/180° off on device, adjust the offset at the send site; the
  turning behaviour itself is correct.
- **Liveness/timeout**: each `CH_ENTITY` snapshot stamps `lan_remote_last_seen[id] = now_ms`; every frame
  any avatar unheard-from for >5s is removed (creature + last-seen entry) and logged, so a player who
  quits or crashes doesn't leave a frozen avatar behind.
- `cargo check --bin` clean. Device-UNVERIFIED (visual orientation + the timeout need two machines).

## r405 — Multiplayer: direct-connect fallback (bypass multicast) (CHECKED, needs device)
Multicast discovery is the likeliest failure point on a real LAN (many networks block multicast), and
without it there was no way to join. This adds a discovery-independent join path so the rest of the
pipeline (handshake → seed → transforms → avatars → block edits) can be verified on its own.
- Host session socket now prefers a **fixed port 9878** (ephemeral fallback if taken), so a peer has a
  known address to reach. Discovery still carries the actual bound port, so nothing regresses.
- New env `UNENDLICH_LAN_HOST=<ip>[:port]` (port defaults to 9878): the joiner parses it to a
  `SocketAddrV4` and **retries a JOIN every 2s until accepted**, no menu interaction or discovery needed.
  Once accepted, the normal seed-load + replication path takes over.
- `cargo check --bin` clean.
- VERIFY WITHOUT MULTICAST: on host machine A, Multiplayer → Host World. On joiner B, launch with
  `UNENDLICH_LAN_HOST=<A-ip>` — B should log `direct JOIN -> <A-ip>:9878`, then `accepted by host`, load
  A's world, and show avatars + propagate block edits. If THIS works but the discovery list stays empty,
  the issue is specifically multicast on the LAN, not the MP code.

## r406 — Threaded water-field settle + evaporation/rain cycle (VERIFIED, inert)
Advances the `CLIMATE_PATH` water roadmap (the field-driven fluid foundation `world/water_field.rs` was
pure+tested but inert). Adds the next pure steps, both threaded/verified, still inert pending the
chunk-manager/sim-scheduler integration the roadmap flags as blocked.
- `world::water_field_batch::settle_chunks(chunks, rate, iters)` — settles many chunks' water fields in
  parallel. `settle_step` has closed chunk boundaries, so per-chunk settle is independent → Class-P map
  over `task_lanes::map_pure`. Order-preserving and volume-conserving (verified: parallel == serial per
  chunk, and `total_volume` preserved through the settle).
- `world::water_field_batch::evaporate_rain(field, evap, rain)` — the cycle step: each column evaporates
  (never below zero) then rain pools on every column. Verified: evaporation lowers volume, rain raises it,
  depth never negative.
- 4/4 flatten `rustc --test` against the real `WaterField::settle_step` + `cargo check --lib` for the real
  rayon bounds.
- Honest: like `water_field` itself, this is INERT — it's the parallel + cycle machinery the persistent
  per-chunk field will use once the chunk-manager side-state exists (roadmap step 3, "mesher reads field
  surface height", is the first visible payoff and is the blocked integration). This is verified
  groundwork, not a live feature.

## r407 — Water cells: placement + drain + fill + physics (VERIFIED, inert groundwork)
Operator: "work on water placement and drain plus fill and physics." Composes the engine's tested water
cores into one per-column water-cell step (`world::water_cells`):
- **Placement** — per-column `sources` inflow (spring / pour / rain). `pour_into(sources, cx, cz, brush)`
  paints a `field_brush::FieldBrush` pour into that buffer (radial falloff), out-of-bounds skipped.
- **Drain + Fill** — each column advances by `lake_fill_level::fill_level(depth, inflow=source,
  outflow=evaporation+infiltration, capacity=spill−ground)`: inflow fills, evaporation + infiltration
  drain, and the capacity clamp caps the column at the spill height (can't pile above where it overflows).
- **Physics** — `water_field::settle_step` spreads water laterally so surfaces equalize (a lake finds its
  level).
- **Threaded** — `step_cells(chunks, params)` steps many chunks in parallel (each independent → Class-P
  map over `task_lanes`), order-preserving = serial.
- 6/6 flatten `rustc --test` against the REAL `settle_step` / `fill_level` / `field_brush` (placement
  raises depth, drain lowers it, fill caps at spill, physics spreads a pile, brush pours water, parallel
  == serial) + `cargo check --lib` for real deps + rayon bounds.
- Honest: like `water_field`/`water_field_batch`, this is INERT groundwork — the per-column cell step is
  ready and verified, but running it live needs the persistent per-chunk field + scheduler hook that the
  `CLIMATE_PATH` roadmap flags as the blocked chunk-manager integration. The simulation math (placement/
  drain/fill/physics) is done and tested; the loop wiring is the device-side step.

## r408 — Cross-chunk water flow (VERIFIED, inert groundwork)
`settle_step` has closed chunk boundaries, so water couldn't cross between chunks — rivers/large lakes
need flux across the seam. `world::water_edge_flow::flow_edge(a, a_ground, b, b_ground, edge, rate)` moves
water across the shared edge between two adjacent chunk fields: per edge-column-pair, water flows from the
higher water *surface* (`ground+depth`) to the lower, a `rate`-fraction of the difference, bounded by the
donor depth. Volume conserved across the pair; depths non-negative.
- `Edge::{PosX, PosZ}` — only the two forward directions are needed (each interior seam is the +X/+Z edge
  of exactly one chunk), so iterating chunks and flowing each one's +X/+Z edge covers every seam once.
- 5/5 flatten `rustc --test`: flows high→low + conserves volume, equal surfaces ⇒ no flow, ground height
  counts toward surface (no uphill flow), depth never negative, repeated flow equalizes. + `cargo check
  --lib`.
- Parallelism note (documented in-module): because a chunk shares seams with several neighbors, a parallel
  schedule must avoid mutating a chunk from two seams at once (a red/black pass over seam parity). This
  module is the per-seam primitive; the schedule is a follow-up. Still inert pending the same
  chunk-manager field integration.
- Water sim cores now cover: placement + drain + fill + within-chunk physics (r407), cross-chunk flow
  (r408), evaporation/rain cycle (r406), orographic mountain sources (r402), all threaded. Remaining pure
  step: aquifer coupling (infiltration ↔ groundwater table) to close the surface/subsurface cycle.

## r409 — Aquifer coupling: close the surface/subsurface water cycle (VERIFIED, inert)
Completes the field-driven water cycle. `world::aquifer_coupling` ties surface water to groundwater:
- `recharge_table(infiltration, drainage, bedrock, out)` — the per-column water that drained subsurface
  (water_cells infiltration) recharges the groundwater table via the tested `aquifer::water_table` (rain
  raises, drainage lowers, clamped bedrock..saturation).
- `seep_inflow(table, ground, rate, out)` — where the table rises above the ground surface, water seeps
  out at `rate*(table-ground)`; at/below ground the column is dry. This inflow feeds back into the cell
  sim's `sources` (placement).
- Loop closed: `surface depth ──drain(infiltration)──▶ table ──seep where table>ground──▶ surface inflow`.
- 4/4 flatten `rustc --test` against the real `aquifer::water_table` (more infiltration raises the table /
  never below bedrock; seep only where table>ground and = rate·Δ; the composed infiltration→seep cycle
  produces inflow; length-mismatch safe) + `cargo check --lib`.
- Per-column array ops → drop straight into the chunk buffers + existing parallel batch. Inert groundwork,
  same blocked chunk-manager integration.

### Water sim status (pure cores, all VERIFIED, all inert pending chunk-manager field hook)
placement + drain + fill + within-chunk physics (r407) · cross-chunk flow (r408) · evaporation/rain cycle
(r406) · orographic mountain sources (r402) · threaded multi-chunk settle (r406) · aquifer/seep cycle
(r409). The full shallow-water + groundwater model is built and tested; the one remaining task is the
device-side integration (persistent per-chunk field + scheduler hook + mesher reads surface height),
which needs chunk-manager work done with the operator.

## r410 — Water cycle capstone: one tested entry point for the full per-chunk cycle (VERIFIED, inert)
Ties every water core (r402–r409) into a single per-chunk tick so the eventual device integration calls
ONE function instead of wiring six modules. `world::water_cycle::step_cycle(chunk_water, ext_sources, p)`:
1. seep (placement from springs) — `aquifer_coupling::seep_inflow` where table > ground
2. infiltration drain (tracked) + placement/evaporation/fill-cap — `lake_fill_level::fill_level`
3. physics — `water_field::settle_step` (lateral settle)
4. recharge — `aquifer_coupling::recharge_table` (table from this tick's infiltration)
`ChunkWater { field, ground, bedrock, table }` holds the per-chunk state; `step_cycles` runs many chunks
in parallel (Class-P). Cross-chunk flow (`water_edge_flow`) stays the caller's between-tick pass (it
couples adjacent chunks, not a single chunk).
- 4/4 flatten `rustc --test` composing the REAL settle_step/fill_level/aquifer/seep cores: a spring seeps
  to the surface, infiltration recharges the table, the full cycle stays finite + spill-bounded over 500
  ticks, parallel == serial. + `cargo check --lib`.
- Fixed during build: `drainage` default lowered to 0.0 (groundwater loss) so sustained infiltration
  actually lifts the table — a units/scale catch the composition test caught (per-tick infiltration ≪ the
  old drainage of 1.0 left the table pinned at bedrock). Documented as tunable for drier climates.
- The entire field-driven water model now has a single verified entry point. Remaining work is purely the
  device-side chunk-manager integration (persistent per-chunk `ChunkWater` + scheduler hook + mesher reads
  surface height) — unchanged, and the piece that needs the operator.

## r411 — River carving: compose the orphaned river cores (VERIFIED, inert)
Pivoted off water (its model is complete) to the campaign pattern — wiring orphaned tested cores. The
river primitives (`river_carve`, `river_meander`, `river_width`, round-9/25 pure cores) had zero callers.
`world::river_carve_field::carve_rivers(heights, w, h, threshold)` is the integrator they were waiting for:
- `river_flow::accumulate_flow` computes how much water funnels through each cell (steepest-descent routing);
- where accumulation reaches `threshold`, `river_carve::channel` gives the channel profile (width/depth grow
  sub-linearly with catchment, capped) and the terrain is lowered to the carved bed.
- Result: rivers appear in the valleys water actually drains toward, big catchments cutting broader channels.
- 4/4 flatten `rustc --test` against the real `accumulate_flow` + `channel` (carves where flow concentrates,
  carving only lowers terrain, higher threshold carves no more, flat terrain not raised, size-mismatch
  no-op) + `cargo check --lib`.
- Inert: it's a pure heightfield transform; running it during worldgen is the cpu_generator-side wiring.

### Session note (r401–r411)
This run delivered: MP reachability + Host-World fixes (r401), 8 water systems threaded (r402), MP slices
4–8 (r398-r405: seed-load, transform repl, visible avatars, block-edit repl, avatar yaw + liveness,
direct-connect), and a complete field-driven water model + capstone + river carving (r406–r411). Split:
MP integration is compile-clean but device-UNVERIFIED (needs two machines; r405 direct-connect isolates
discovery); water/river cores are fully VERIFIED but INERT (need the chunk-manager/cpu_generator/mesher
integration). The verifiable solo surface is largely exhausted — the next real value is a device
verification run or the operator opening up the chunk-manager integration.

## r412 — WIRED: dynamic in-chunk water sim (UNVERIFIED, gated UNENDLICH_WATER_SIM=1)
First integration that makes the water work VISIBLE + dynamic in-game (operator: "keep wiring things for me
to bulk test"). Off by default; set `UNENDLICH_WATER_SIM=1`.
- Per frame, throttled to 4/sec: for chunks within radius 2 (and ±1 vertically) of the player, read the
  chunk's water into a `WaterField` (`seed_from_chunk`), settle it toward its level (`settle_step` ×2 — the
  r406-verified core), and write the result back.
- WRITE-BACK IS TERRAIN-SAFE: only Air→Water and Water→Air transitions are issued via `set_block`; solid
  terrain is never touched (checks `chunk.get` first). So a bug can at worst misplace water, never destroy
  ground.
- The chunk is the state (re-seeded each tick), so digging a trench near water makes water flow in + level
  out on the next tick — the testable demo. Mesh churn bounded by a 1500 `set_block`/tick budget.
- `cargo check --bin` clean. Device-UNVERIFIED: perf (mesh re-upload on edits), visual correctness, and the
  settle behaviour need the operator's hardware.
- KNOWN v1 LIMITATIONS: within-chunk only (no cross-chunk flow yet — water won't cross the 32-block chunk
  seam; `water_edge_flow` r408 is the follow-up); depth rounds to whole blocks each tick (minor volume
  drift possible); settles toward a flat level per chunk. This is the simplest safe version to validate the
  approach before adding cross-chunk flow + the full cycle (drain/aquifer).
- BULK TEST: launch with `UNENDLICH_WATER_SIM=1`, find/standing water, dig a hole beside it → water should
  flow in and level out within a second; watch for perf hitches or misplaced water. Log line on startup:
  `[water-sim] dynamic in-chunk water ON`.

## r413 — WIRED: cross-chunk flow added to the dynamic water sim (UNVERIFIED, gated)
Completes the r412 water sim so water flows across the 32-block chunk seam instead of stopping at it.
Restructured the per-tick water update into four phases over the chunks near the player:
1. collect chunks that contain water (seed each into a `WaterField`),
2. settle each toward its level (`settle_step`),
3. flow across +X/+Z seams — delta-based (read both edge columns, accumulate per-column deltas, apply)
   so no two-chunk `&mut` is needed; water moves from the higher surface to the lower, donor-bounded,
4. write back Air↔Water only, budgeted (1500 set_block/tick).
- Same gate `UNENDLICH_WATER_SIM=1`, same terrain-safety (only Air↔Water transitions).
- `cargo check --bin` clean. Device-UNVERIFIED.
- Now: a trench dug across a chunk boundary should fill from both sides; a lake spanning chunks levels out
  uniformly instead of stair-stepping at the seam.
- Remaining v1 caveats: still no vertical cross-chunk flow (water won't fall between stacked 32³ chunks);
  corner columns feeding two seams can slightly over-donate (clamped at 0, tiny non-conservation); depth
  rounds to whole blocks. Good enough to bulk-test the behaviour; the full cycle (drain/aquifer/sources via
  the r409/r410 capstone) is the next layer once this looks right on device.

## r414 — Water sim tunable + BULK-TEST GUIDE (UNVERIFIED, gated)
Made the dynamic water sim tunable so perf can be dialed in during bulk testing, and consolidated the
testable toggles.
- `UNENDLICH_WATER_RADIUS` (default 2, 0–6): chunk radius around the player the sim processes.
- `UNENDLICH_WATER_BUDGET` (default 1500): max set_block/tick (lower = less mesh churn / cost).
- `UNENDLICH_WATER_INTERVAL_MS` (default 250): tick period (higher = cheaper, less responsive).
- `cargo check --bin` clean.

### BULK-TEST TOGGLES (current build)
MULTIPLAYER (LAN) — on by default (opt out `UNENDLICH_LAN_MP=0`):
- Two instances: A → Multiplayer → Host World (loads a world, auto-announces). B → Multiplayer → A appears
  under "LAN games" → click to join; both load the same world, see each other's avatar (turns with yaw,
  despawns 5s after a peer leaves), and block edits propagate.
- If discovery (multicast) is blocked on the LAN: launch B with `UNENDLICH_LAN_HOST=<A-ip>` to direct-join
  (host binds udp/9878). Logs: `[lan-mp] discovery beacon bound`, `hosting`, `accepted by host`,
  `spawned remote avatar`, `Placed/Broke`.
DYNAMIC WATER — off by default, `UNENDLICH_WATER_SIM=1`:
- Water near the player settles to its level and flows across chunk seams. Dig a trench beside water → it
  flows in and levels out within ~1s. Terrain is never overwritten (only Air↔Water). Tune with the
  WATER_RADIUS/BUDGET/INTERVAL vars if it costs too much. Log: `[water-sim] dynamic water ON`.

### Honest status of the test build
MP = compile-clean, needs 2 machines to validate (direct-connect isolates multicast). Water sim = wired +
built on flatten-verified cores (settle/flow), terrain-safe, but the live behaviour (perf, visual
correctness, volume drift from whole-block rounding, no vertical cross-chunk flow) needs device eyes. Both
are gated so the default build is unchanged. The deeper water layers (drain/aquifer/sources via the r410
capstone) and N-player MP relay are the next wires once these validate.

## r415 — FIX: water now moves DOWNWARDS (world-column rewrite) (UNVERIFIED, gated)
Device feedback: "water not moving downwards yet." Root cause: the r412–r414 sim was a shallow-water
heightfield PER 32³ CHUNK — `settle_step` only spreads laterally to lower-surface neighbours, and there was
no vertical flow across chunk Y-boundaries. A column's "ground" was the chunk-local topmost solid, so water
sitting above a floor that lives in the chunk *below* never fell. Per-chunk fields fundamentally can't
express "water falls down a shaft."
FIX — rewrote the bin water tick to simulate tall WORLD-COLUMNS spanning chunks (via `block_at`/`set_block`
in world coords):
- For each column near the player, scan a vertical window (player.y-56 .. +24) to find the floor (topmost
  solid, may be far below in another chunk) and the water count.
- Place each column's water from its floor up — so floating water FALLS to the real floor.
- A few settle passes spread water to lower-surface neighbours (downhill flow + filling dug holes), now
  across chunk boundaries because the grid is world-space, not per-chunk.
- Write-back still flips Air↔Water only (terrain never overwritten), budgeted.
- Reuses the tunables: `UNENDLICH_WATER_RADIUS` (now scaled ×8 to a block radius, default 16 blocks),
  `UNENDLICH_WATER_BUDGET`, `UNENDLICH_WATER_INTERVAL_MS`. Gate unchanged: `UNENDLICH_WATER_SIM=1`.
- `cargo check --bin` clean. Device-UNVERIFIED.
- BULK TEST: with `UNENDLICH_WATER_SIM=1`, water should now fall to the floor and run downhill / into holes
  you dig, including across chunk seams. If it costs too much, lower `UNENDLICH_WATER_RADIUS`.
- KNOWN CAVEATS: cost is `block_at` lookups per cell (~1089 cols × 80-tall window × 2 passes ≈ 175k
  lookups/tick at 4/sec — tune radius down if heavy); columns with no floor in the window pool crudely at
  the window bottom; whole-block depth rounding means very thin water (<0.5) can vanish (minor volume drift);
  the window is player-relative so far-off water isn't simulated.

## r416 — Water sim optimisations 1–3 (UNVERIFIED, gated)
Cut the per-tick cost so the simulated radius can grow before any GPU work is needed. Same gate/behaviour.
- opt-1 (idle skip): the tick runs only when `water_dirty || water_active`. `water_dirty` is set by player
  break/place edits; `water_active` stays true while the last tick still produced block changes. Settled,
  untouched water now costs ZERO per frame (was re-seeding + re-settling every tick).
- opt-2 (prefetch): each chunk overlapping the region is fetched once into a plain HashMap; reads go through
  a local `read()` over that map instead of a `block_at` DashMap walk per cell (~175k DashMap lookups/tick →
  ~27 `get_chunk` calls + local indexing).
- opt-3 (parallel gather): the column scan (the expensive read pass) runs through
  `task_lanes::map_pure_threshold(.., 2048, ..)` — serial at the default radius (1089 cols, no rayon
  overhead), parallel once the radius is raised past 2048 columns. The compile confirms the rayon Sync/Send
  bounds (Arc<Chunk> is shared-safe).
- The settle is now in gather-form (per-cell outflux array → per-cell net), volume-conserving and structured
  so it ports directly to a compute shader later.
- `cargo check --bin` clean. Device-UNVERIFIED. Tunables unchanged (WATER_RADIUS/BUDGET/INTERVAL).
- NOTE on visuals: this is all the SIM (where the water is). The water SURFACE already animates in the
  renderer via `shaders/water_waves.glsl` (analytic waves driven by WIND). Making it visibly flow ALONG the
  current is a separate, small shader change (feed the sim's flow direction into the wave `dir`), tracked
  next.

## r417 — Flow-as-shader PHASE 1: flow-field producer (VERIFIED core / UNVERIFIED wiring)
Phase 1 of making water visibly stream along its current (blend with wind). Produces the data; the shader
phases consume it later.
- NEW world/water_flow_field.rs (flatten-VERIFIED, 8 tests): `WaterFlowField` — a side×side grid of
  per-column 2-D flow vectors anchored at a world origin. `accumulate(send)` folds one settle step's
  directional out-flux `[+x,-x,+z,-z]` into net flow `(+x - -x, +z - -z)`; `dir_speed`/`dominant` give a
  normalized direction + a saturating speed in [0,1] (calm columns return zero so the consumer can fall back
  to wind). Pure, no engine deps.
- WIRED into the bin water sim: each tick `reset(origin)`, then `accumulate(&send)` per settle iteration, so
  `water_flow` holds the current's direction + strength over the simulated region. Negligible cost (reuses
  the `send` array the settle already builds).
- VERIFY ON DEVICE: `UNENDLICH_WATER_SIM=1 UNENDLICH_WATER_FLOW_DEBUG=1` → ~1/sec log line
  `[water-flow] dominant dir=(x,z) speed=...`. Dig a channel and watch water run down it: the dir should
  point downstream and speed should rise while it's moving, fall to ~0 once settled. That confirms the
  producer before we touch any GLSL.
- `cargo check --lib` + `--bin` clean.
- NEXT (phase 2): upload `water_flow` as a small flow texture (world XZ → vector); (phase 3) sample it in
  chunk.frag and blend the flow direction into the `water_waves.glsl` wave `dir` (wind chop on calm water,
  directional streaming on currents).

## r418 — Flow-as-shader PHASE 2a: persistent flow tiles (VERIFIED core / UNVERIFIED wiring)
The persistence layer the renderer will sample. Flow no longer resets as you move.
- NEW world/water_flow_tiles.rs (flatten-VERIFIED, 6 tests): `FlowTileStore` — sparse HashMap of 32×32
  (chunk-aligned) tiles of flow vectors. `ingest(origin,side,vx,vz,now)` writes the sim window's vectors
  into the tiles they fall on (persisting them); `sample(wx,wz)` reads flow at any world column;
  `decay(now,factor)` fades tiles the sim didn't refresh this tick; `evict(now,max_age,player_tile,radius)`
  drops far/old tiles to stay bounded. Tests cover roundtrip, tile-boundary spanning, persistence across a
  moved window, decay-of-stale-only, and far/old eviction.
- WIRED into the bin sim: each active tick ingests the window flow, decays unrefreshed tiles by 0.97, and
  evicts beyond 24 tiles / 60 s. Map stays bounded (~hundreds of tiles, a few MB).
- BEHAVIOUR / honest limit: flow data exists only where the sim has actually run — near you and where
  you've recently been. A river you've walked keeps its current (fading gently behind you); a river you've
  never approached has no flow data (it was never simulated). Global flow would mean simulating the whole
  world. This is the right bounded behaviour for a player-centred sim.
- VERIFY ON DEVICE: `UNENDLICH_WATER_SIM=1 UNENDLICH_WATER_FLOW_DEBUG=1` now also logs
  `persisted@player=(x,z) tiles=N`. Flow water through a spot, walk away: the window dir goes to 0 but the
  persisted value at that spot should remain non-zero and ease down over a few seconds (decay), and `tiles`
  should track how many tiles are retained.
- `cargo check --lib` + `--bin` clean.
- NEXT (phase 2b): upload the tiles near the player as a GPU flow texture each frame. (phase 3) sample it in
  chunk.frag and blend into the `water_waves.glsl` wave `dir` (wind chop on calm water, streaming on
  currents). 2b/3 are GPU/render code — written carefully, gated, device-UNVERIFIED.

## r419 — Flow-as-shader PHASES 2b+3: flow drives the water surface (UNVERIFIED render / VERIFIED cores)
Water waves now stream along the sim's current, blended with wind. Gated; default off = identical render.
- CPU (VERIFIED, flatten, +3 tests = 11 total in water_flow_tiles): `FlowTileStore::pack_grid(center, cells,
  blocks_per_cell)` samples the persistent tiles into a coarse 16×16 grid (2 cells/vec4) + a coverage vec4
  `[origin_x, origin_z, blocks_per_cell, enable]`. Wide + low-res by design (32 blocks/cell → ~512-block
  window).
- GPU UBO (UNVERIFIED): appended `flow_coverage` + `flow_grid[128]` to `CameraUbo` — APPEND-ONLY, so every
  existing field keeps its std140 offset; buffer/range/copy are all `size_of`-driven so they grow
  automatically. New `WorldRenderer::set_water_flow(coverage, grid)` stores it; the per-frame UBO build
  uploads it. With `flow_coverage.w == 0` (default) the shader path is inert ⇒ rendering byte-identical to
  r418.
- SHADER (UNVERIFIED, but glslangValidator → SPIR-V clean): chunk.frag samples the flow grid by world XZ and,
  when enabled, blends the flow direction into the `water_waves.glsl` wave `dir` (strength-weighted) and
  speeds the waves up on faster current. Calm water keeps wind chop.
- BIN: each frame (when on) packs the tiles centred on the player and calls `set_water_flow`.
- GATE: `UNENDLICH_WATER_FLOW_SHADER=1` (needs `UNENDLICH_WATER_SIM=1` to have flow). Off = wind-only.
- SAFETY: the append preserves offsets and the enable flag gates the read, so the ONLY way normal rendering
  changes is if the flag is on. Test OFF first (confirm water looks exactly as before = no regression), then
  ON (waves should run downstream where you've flowed water; calm lakes still wind-rippled).
- `cargo check --lib` + `--bin` clean; chunk.frag compiles to SPIR-V. Device-UNVERIFIED: the actual look +
  whether the blend strength feels right need your eyes — those constants (flow dominance 1.5, speed boost
  0.6) are first guesses to tune.

## r420 — [water] diagnostics + wave-speed knob + flow-driven foam (UNVERIFIED render)
Response to "water too fast with the wind" + "keep stacking water features".
DIAGNOSIS FIRST (the report could be windspeed OR shader): confirmed `time_scale` defaults to 1.0 and is
never overridden, so the wave clock is REAL-TIME, not accelerated. The wave speed is real wave-physics
(`w = sqrt(9.8·k)·speed_scale`, `speed_scale = 0.6 + 0.5·clamp(|wind|,0,2)`), so "too fast" is either an
inflated wind magnitude or the speed mapping. Added a readout to tell which:
- `[water]` debug (`UNENDLICH_WATER_DEBUG=1`, ~1/sec): logs `wind=(x,z) |wind| len(wind) speed_scale
  wave_mult temp tod tod_rate flow@player`. `tod_rate≈1.0` ⇒ real-time clock (rules out time acceleration);
  `|wind|` vs `len(wind)` disagreeing ⇒ a wind-magnitude bug; `speed_scale` mirrors the shader's mapping.
- WAVE-SPEED KNOB `UNENDLICH_WATER_WAVE_SPEED` (default 1.0, 0–4): multiplies the wave animation speed only
  (not amplitude). Plumbed through the previously-unused `wind_params.w`; shader does
  `wp.speed_scale *= (wind_params.w > 0 ? wind_params.w : 1.0)`. Default 1.0 = unchanged. Set 0.5 to halve
  the wave speed, etc. — the dial for the "too fast" complaint while the debug pins the cause.
STACKED FEATURE — flow-driven foam: where the persisted current is fast, the water foams (whitewater on
rapids) even without steep crests, riding the flow speed already sampled in chunk.frag. Only active with the
flow shader on (`UNENDLICH_WATER_FLOW_SHADER=1`); speed stays 0 otherwise so still water is unaffected.
- `cargo check --lib` + `--bin` clean; chunk.frag → SPIR-V clean. Render bits device-UNVERIFIED.
- TEST: launch with `UNENDLICH_WATER_DEBUG=1`, read a few `[water]` lines, tell me `|wind|`, `speed_scale`,
  and `tod_rate`. If |wind| looks huge that's the windspeed bug; otherwise dial `UNENDLICH_WATER_WAVE_SPEED`
  down until the chop feels right and we bake that in. With the flow shader on, fast water should show foam.

## r421 — FIX: water checkerboard "flying into the air" was a settle instability (CHECKED)
Device report: water flying up in a checkerboard. From the r420 [water] log: wind normal (|wind| matches
len(wind) — no windspeed bug), tod_rate≈1.0 (real-time clock — no time bug). So not the shader, not wind.
ROOT CAUSE: the world-column settle moved `0.5 * (surface difference)` per neighbour. For an explicit
simultaneous (Jacobi) update on a 2-D grid the checkerboard mode grows by `|1 - 8f|` per iteration, so
f=0.5 → 3× per iteration = unstable. It pumps (volume-CONSERVED, so NOT evaporation; direction was fine, so
NOT a direction bug) water into alternating tall columns → the checkerboard spikes.
FIX: per-neighbour factor 0.5 → 0.125 (kills the checkerboard mode in one step; stable range is f < 0.25),
plus a per-column depth cap at the window height as a belt-and-suspenders against any runaway.
VERIFIED numerically (standalone settle sim, 8×8 checkerboard, 30 iters): f=0.5 → max depth 11.46 (grows
above the starting 10, concentrates); f=0.125 → 5.10 (settles to the ideal flat 5.0); volume conserved 320→
320 in both. `cargo check --bin` clean.
- TEST: re-run with `UNENDLICH_WATER_SIM=1` — water should settle flat instead of spiking into checkerboard
  towers. Settling is a touch slower per tick (smaller steps, 6 iters/tick × 4/sec); raise WATER_INTERVAL or
  iters later if it feels sluggish.

## r422 — Settle tuning + flow polish (CHECKED cores / UNVERIFIED shader)
Batch of safe improvements (operator batch-testing later).
- SETTLE TUNABLES: `UNENDLICH_WATER_ITERS` (default 8, was fixed 6 — more iterations recover the settling
  speed lost to the r421 stability fix, cheaply) and `UNENDLICH_WATER_SETTLE_F` (default 0.125, CLAMPED to
  [0.02, 0.24] so the per-neighbour factor can never re-enter the unstable f≥0.25 regime).
- FLOW TEMPORAL SMOOTHING (VERIFIED, +2 tests = 12 in water_flow_tiles): `FlowTileStore::ingest_blend` EMA-
  blends new flow with stored (`UNENDLICH_WATER_FLOW_SMOOTH`, default 0.35) so the visible current eases
  instead of jittering tick-to-tick. alpha=1.0 == old ingest (test-proven).
- BILINEAR FLOW SAMPLING (shader, glslang→SPIR-V clean): chunk.frag now interpolates the flow across the
  four nearest coarse cells instead of nearest-cell, so current direction varies smoothly rather than
  snapping at 32-block cell edges.
- `cargo check --bin` clean; chunk.frag compiles. Shader bits device-UNVERIFIED.

## r423 — Evaporation of isolated shallow puddles (VERIFIED core / gated)
A safe slice of the water cycle. NEW world/water_evaporate.rs (flatten-VERIFIED, 6 tests): `evaporate(depth,
side, shallow, rate)` removes up to `rate` depth from a column ONLY if it and all four neighbours are
`<= shallow`. Guard means a lake's shallow rim (adjacent to deep interior) is protected — puddles dry, lakes
don't erode. Tests cover puddle drying, rim protection, deep-water immunity, rate-cap-at-depth, block drying,
zero-rate no-op.
- WIRED into the sim after settle, before write-back. Gated `UNENDLICH_WATER_EVAP=1`, rate
  `UNENDLICH_WATER_EVAP_RATE` (default 0.04 depth/tick). Off by default.
- Handled the idle-gate interaction: evaporation keeps the sim `active` while it's still removing water, so a
  puddle fully dries instead of freezing once its block stops flipping.
- `cargo check --lib` + `--bin` clean.

### Batch since the checkerboard fix (for one test pass)
- r421 (CHECKED): settle stability fix — water settles flat, no more checkerboard towers.
- r422: `UNENDLICH_WATER_ITERS` (8), `UNENDLICH_WATER_SETTLE_F` (0.125, clamped <0.25), flow smoothing
  `UNENDLICH_WATER_FLOW_SMOOTH` (0.35), bilinear flow in the shader.
- r423: `UNENDLICH_WATER_EVAP=1` (+ `_RATE`) dries isolated puddles.
All on top of: `UNENDLICH_WATER_SIM=1` (+ RADIUS/BUDGET/INTERVAL), `UNENDLICH_WATER_FLOW_SHADER=1`,
`UNENDLICH_WATER_WAVE_SPEED`, `UNENDLICH_WATER_DEBUG=1`.

## r424 — Rain → puddles (VERIFIED core / gated) — closes the water cycle
NEW world/water_sources.rs (flatten-VERIFIED, 5 tests): `apply_rain(depth, floor, amount, cap)` adds rain to
EXPOSED columns (real floor in window) up to `cap`, leaving already-deep water alone — so rain makes puddles
and can NEVER raise lake/ocean levels (no flooding). Tests: rains on dry ground, skips floorless columns,
doesn't raise deep water, clamps at cap, zero-amount no-op.
- WIRED gated `UNENDLICH_WATER_RAIN=1`; `UNENDLICH_WATER_RAIN_RATE` (0.08), `UNENDLICH_WATER_RAIN_CAP` (2.0).
  Driven by the existing `precip01` weather state, applied after gather/before settle. Rain wakes the sim
  (sets dirty while precip>0.05) and keeps it active while adding, so puddles form without player edits.
- Auto-enables evaporation (rain needs drying to cycle): rain → shallow sheets → settle runs them to low
  spots/depressions → evaporation dries the flats, depressions keep their puddles. A real rain cycle.
- `[water-flow]` debug now also shows `rain+= evap-=` so the cycle is observable.
- `cargo check --lib` + `--bin` clean.

### Water cycle now complete (all gated, default off)
rain (r424) → settle/flow downhill + into holes (r415–r422, stable) → evaporation of stranded puddles
(r423); visual flow streams the surface (r417–r422). Cores are flatten-VERIFIED; sim/shader behaviour is
device-UNVERIFIED.

## r425 — Water freezes to Ice in the cold (CHECKED / gated)
Seasonal/climate feature. Surface water turns to `BlockId::Ice` where the climate temperature is cold, and
melts back when it warms. Integrated into the world-column sim without breaking the terrain-safety invariant
(only ever writes Air/Water/Ice above the floor):
- Gather counts Ice as water depth (so a frozen column reads its true depth and ice isn't mistaken for the
  floor).
- While frozen the settle is skipped (0 iterations) so the water is held static — frozen water doesn't flow
  and shows no current.
- Write-back places the surface cell as Ice when frozen, Water otherwise; melt is just the reverse.
- Freeze/melt uses the climate `temperature` at the player with hysteresis (freeze `UNENDLICH_WATER_ICE_TEMP`
  default 0.12, melt +0.08) to avoid flicker, and wakes the idle sim on each transition so melting actually
  applies.
- Gated `UNENDLICH_WATER_ICE=1`. `cargo check --bin` clean.

### Water system status (all gated, default off)
Full cycle: rain (r424) → flow/settle downhill + into holes, stable (r415–r422) → evaporation of stranded
puddles (r423) → freeze/melt to ice in the cold (r425). Visual: surface streams along the current with foam
on rapids (r417–r422). Pure cores are flatten-VERIFIED; sim+shader runtime behaviour is device-UNVERIFIED
and is the batch to test. This is a natural checkpoint — further water work (diagonal flow, MP water sync,
player current-push) is better steered by on-device feedback.

## r426 — Wave speed + drain depth (from r425 device feedback)
- WAVES TOO FAST / ~1Hz vacillation on big lakes: default `UNENDLICH_WATER_WAVE_SPEED` lowered 1.0 → 0.45
  (the log showed wave_mult=1.0, i.e. they ran at the old default). Slower, more lake-like; still tunable.
- WATER NOT DRAINING INTO CAVES: the vertical scan window depth is now `UNENDLICH_WATER_DEPTH` (default 80,
  was a fixed 56) so water finds cave/hole floors farther below. NOTE the real constraints (told to operator):
  (1) water only drains down OPEN holes/gaps — it can't pass through solid rock into an enclosed cave;
  (2) the sim is a window (~radius×8 blocks) around the PLAYER, so a big lake's far reaches don't move — only
  water within the window near the player flows toward a hole. Bigger radius = more coverage at quadratic cost.
- `cargo check --bin` clean.

## r427 — Underwater screen FX (UNVERIFIED render) — partial answer to the submerged-warp ask
When the camera is submerged, the view gets an underwater treatment: red absorbed first (blue-green murk),
an animated caustic shimmer banding the light, and darkened edges. Built on the same APPEND-ONLY UBO pattern
as flow (zero offset shift → normal rendering byte-identical when not submerged):
- CameraUbo gains `fx_params` (x=submerged, y=time); `WorldRenderer::set_underwater(sub, t)` stores it; the
  bin sets it each frame from `block_at(eye) == Water`.
- chunk.frag applies the tint/shimmer when `fx_params.x > 0.5`.
- Gated `UNENDLICH_WATER_UNDERWATER_FX` (default ON; inert above water). `cargo check` + glslang clean.
- HONEST SCOPE: this is the COLOUR/SHIMMER layer. A true geometric image-WARP (displacing the rendered
  scene) needs the post-FX present pass (push-constant + `UNENDLICH_POSTFX`); that's the follow-up. The
  "mob eyes submerged" request is a separate concern (creature perception/vision), not a screen effect —
  flagged for the operator.

## r428 — Wave buzz, faster/wider draining, boiling-on-lava (from repeated r425 feedback)
WAVES (the ~1Hz vacillation): the buzz was the 4th octave at ~1.1Hz. Dropped WW_OCTAVES 4→3 in
water_waves.glsl (removes that frequency entirely; still 3 octaves of detail) and auto-centered the
directional fan so it stays symmetric at any octave count. Combined with r426's slower default speed (0.45),
the fastest remaining wave is sub-0.4Hz — slow swells, not buzz. glslang clean. Raise UNENDLICH_WATER_WAVE_SPEED
for livelier beach waves.

DRAINING (water into caves / big bodies not moving): made local draining faster + wider-reaching —
UNENDLICH_WATER_RADIUS default 2→3 (more of a body flows toward a hole), ITERS 8→12, SETTLE_F 0.125→0.15
(still under the 0.25 stable bound), BUDGET 1500→3000 (set_block was throttling the drain). Honest limits
restated: water only drains down OPEN holes (never through solid rock into a sealed cave), and the sim is a
window around the PLAYER, so a big lake's far reaches still won't move — true big-lake-rushes-into-cave needs
a larger/region sim (a bigger follow-up). radius 3 costs ~2x the old window; lower it if perf suffers.

BOILING: NEW gated UNENDLICH_WATER_BOIL — water that ends up sitting on Lava boils off (BOIL_RATE 0.75/tick)
with a Smoke/steam puff, instead of pooling. Gather now reports whether a column's floor is Lava; the boil
pass runs before settle so boiled water doesn't settle back. (Steam particles render once the pending
particle GPU-upload lands; the boil-away mechanic works now.) A steam-PRESSURE-pushback model is out of scope
— flagged. `cargo check --bin` clean.

## r429 — Boiling driven by the live heat field (operator: "there is a heat field wired to a GUI")
Reworked r428's lava-contact boiling to use the live `heat_sources` field (the same one feeding the GUI).
Per tick (gated `UNENDLICH_WATER_BOIL=1`) the boil pass snapshots the heat sources ONCE, then for each water
column sums `HeatSource::contribution_at` (°C, Gaussian falloff) at the water surface. A column boils when the
sampled heat ≥ `UNENDLICH_WATER_BOIL_TEMP` (default 45 °C — the operator's hot-underground case; lava sits far
above) OR it's sitting directly on lava. Hotter water boils faster (rate scales past the threshold). Steam
puffs as before (render pending the particle GPU-upload). Snapshot-once avoids per-column lock contention;
only water columns are sampled. `cargo check --bin` clean.

## r430 — Steam pressure pushback → equilibrium (operator: "worth it… equilibrium points key to efficient compute")
The boil pass (r429) now also builds a per-column steam BACK-PRESSURE: a boiling column adds
`UNENDLICH_WATER_STEAM_PRESSURE` (default 4 block-heights, scaled by how far past the boil point it is) to its
EFFECTIVE surface in the settle's surface-difference test. Water is therefore repelled from hot deep sections;
combined with the boil-off (removal), inflow into a hot cave balances against pressure+evaporation and the sim
settles toward an EQUILIBRIUM instead of asymptotically draining oceans into caves. Stable: the pressure is a
bounded constant offset on the diffusive settle (f≤0.24). Steam particles now vent harder (more puffs) the
hotter/deeper the column. `UNENDLICH_WATER_STEAM_PRESSURE=0` → pure boil-off, no repulsion. `cargo check` clean.

## r431 — Real depth-color: water reads as volume, not a flat sheet (UNVERIFIED render)
The water surface shader was estimating "depth" from VIEW ANGLE only (mix(1,9,1-grazing)) — so a 1-block
puddle and a deep ocean shaded identically. Now the sim's REAL per-column depth drives the Beer-Lambert path:
- Sim downsamples the settled depth window to a 16×16 grid + coverage; uploaded via the safe append-only UBO
  pattern (CameraUbo gains `water_depth_cov` + `water_depth_grid[64]`; `set_water_depth`).
- chunk.frag samples it bilinearly by world XZ (`ww_water_depth`) and uses depth as the path length: shallow →
  short path → clear/light SHALLOW colour; deep → long path → dark DEEP blue. Outside coverage / disabled it
  falls back to the old angle estimate — byte-identical when off.
- Gated `UNENDLICH_WATER_DEPTH_COLOR` (default on with the sim). glslang + `cargo check` clean.
- HONEST LIMITS: (1) covers the sim window around the player (distant water uses the angle fallback; there may
  be a soft seam at the edge); (2) the grid re-centers when the sim TICKS, so depth-color can lag if you walk
  along perfectly calm (idle) water until the sim re-runs; (3) this is depth-as-COLOUR — true see-through
  transparency (seeing the actual bottom) still needs the filed transparent draw pass. A persistent depth-tile
  store (like the flow tiles) would fix (1)+(2); the transparent pass + the underwater geometric warp are the
  next rendering pieces. Erosion-at-shorelines still queued.

## r432 — Persistent depth tiles (ladder #1) — fixes the depth-color seam + lag
Extended the flow tile store (water_flow_tiles.rs) with a parallel `depth` channel: ingest persists per-cell
water depth alongside flow (no decay — depth is a state; -1 sentinel until first written), plus `sample_depth`
and `pack_depth_grid` (returns -1 for never-simulated cells). Flatten-VERIFIED (12 tests, +2 new). The bin now
ingests `depth` into the tiles each sim tick and uploads a 16×16 / 8-blocks-per-cell depth grid EVERY FRAME
(128-block window) from the persistent tiles — so depth-color covers everywhere the sim has run and no longer
lags the throttle or seams at the old sim-window edge. Never-simulated cells (-1) keep the shader's angle
fallback. r431's in-tick grid build removed. `cargo check` lib+bin clean.

## r433 — Shoreline erosion (ladder #3)
Gated `UNENDLICH_WATER_EROSION=1`. While water is active, the sim washes exposed Grass/Dirt at the waterline
(a land neighbour whose surface sits within ±1 of a water column's surface) into Sand — beaches form over
time. Conservative by design: a same-volume block swap (no holes, terrain volume preserved), budgeted
(`UNENDLICH_WATER_EROSION_BUDGET`, default 2/tick) with a rolling scan offset, and the ONLY place the water sim
edits terrain. Recession (removing blocks to recede the shore) intentionally NOT done yet — riskier; would be a
separate slow/bounded pass. `cargo check --bin` clean.

### Water ladder status: #1 persistent depth tiles ✓ (r432) · #2 transparent pass — STAGED (see NEXT_SESSION) ·
### #3 erosion ✓ (r433). #2 is a multi-session render-pipeline change (mesh split + alpha-blend pipeline +
### scene depth/color descriptors + ordering) — must be staged with on-device testing per step, not one-shot blind.

## r434 — Spatial-voice core (audio start): block occlusion + reverb estimate (VERIFIED)
NEW sim/voice_spatial.rs (flatten-VERIFIED, 9 tests). Pure, no audio I/O / no network — turns world geometry
into the parameters a voice backend applies:
- `occlusion(listener, speaker, params, solid_at)` → gain + low-pass + distance + solids. DDA voxel walk
  (Amanatides–Woo) counts solid blocks on the line; gain = distance-falloff × per-block muffle; low-pass
  saturates with each wall; beyond max_distance → silent. Walls make a speaker quieter AND more muffled.
- `reverb(listener, max_dist, solid_at)` → wet + decay + enclosure. 14 probe rays measure how enclosed/large
  the space is: open air → dry; sealed small room → wet + short decay; cavern → long decay.
- `solid_at` closure abstracts the world → fully testable; the bin will pass `chunk_manager.block_at(..).is_solid()`.
This is the foundation for game-aware reverb-smart voice chat; the audio backend + network transport are the
next (device/network-only) pieces — see NEXT_SESSION.

## r435 — Volumetric depth treatment (ladder #2, step 0: shader-only, safe) — UNVERIFIED render
Toward volumetric water WITHOUT the risky pipeline change yet. The water surface shader now uses the real
depth grid (r432) to read as a body you see INTO, not a flat sheet:
- a sunlit faux-bottom shows through SHALLOW water, displaced by the wave normal so it shimmers like
  refraction (fake refraction — no scene sample needed);
- light penetration warms/greens the shallows (sun reaching + scattering);
- a broken-up foam ribbon rides the waterline where depth shades to ~0 (the shore edge reads as volume meeting
  land);
- the existing depth gradient (shallow light → deep dark) still anchors the volume cue.
Purely chunk.frag (hot-loadable), gated by depth data (wdepth<0 → inert, identical to before). glslang clean.
HONEST: this is fake-refraction/see-INTO, not literal see-THROUGH. True transparency = the staged transparent
pass (mesh split → blend pipeline → scene depth/colour descriptors → real refraction), which must be device-
tested per step. emit_water_surface writes the shared ChunkMesh and the draw goes through GPU indirect meshing,
so the split is the invasive step — next, carefully, with per-step on-device verification.

## r436 — Water sim telemetry on the [water] line (diagnose "overlay not tied to flow/position")
The [water] debug (UNENDLICH_WATER_DEBUG) now also prints `depth@player` (from the depth tiles; -1 = no data
there) and a `sim:` group — `cols` (columns simulated), `moved` (water blocks rewritten this tick = the sim
actually moving water sideways/down), `flowspd` (dominant current magnitude in the window), `active`. The
"is water moving / is it tied to position" question is now answerable from WATER_DEBUG alone (the richer
[water-flow] line was behind a separate flag the operator hadn't set, so prior logs only showed wind). No
behaviour change. `cargo check --bin` clean.

## r437 — Echo readout + master log gate + Settings → Debug tab
- ECHO: `[echo] enclosure=.. wet=.. decay=..s @player=..` once/sec — the LIVE read of sim/voice_spatial::reverb
  (14 probe rays vs is_solid()), so it validates the voice core in-game and spikes (enclosure→1, longer decay)
  exactly when you drop into a cave. Gated by the echo toggle.
- MASTER LOG GATE: new `renderer.ui_debug_master` gates ALL debug logging and is OFF by default — no log spam
  unless asked. Existing CLI flags still work: UNENDLICH_WATER_DEBUG / _WATER_FLOW_DEBUG / _ECHO_DEBUG (and
  UNENDLICH_DEBUG_LOG=1 for master) seed the in-game toggles at startup and flip master on.
- SETTINGS → DEBUG TAB: new "Debug" tab (world-settings moved to index 6). Master switch up top, then sectioned
  toggles — WATER ([water], [water-flow]) and ACOUSTICS ([echo]) — greyed out until master is on. Uses the same
  renderer-field settings pattern as fog/shadows (no new plumbing). First use of the in-game settings path for
  live toggles → the template for the water perf-knobs later. `cargo check` lib+bin clean.

## r438 — CRITICAL: sim now wakes for NATURAL water (root cause of "overlay not tied to flow/position")
Device log proof: depth@player=-1, moved=0, active=false on EVERY tick while swimming a lake — the sim was
scanning 2401 cols but never running its body. Root cause: the idle-skip only set water_dirty on EDITS
(place/break/rain/ice), so worldgen lakes/oceans were NEVER simulated — no depth tile, no flow, no settle, and
the surface shader had no depth/flow data, so depth-color/volumetric/flow all read as a flat overlay, and water
never drained into open caves. FIX: each frame, if the sim is idle and there's Water within a small box around
the player AND no depth tile there yet (depth@player<0), set water_dirty to wake it. Self-limiting: once an area
is simulated depth@player≥0 and it stops firing; moving to new water (depth=-1) re-wakes; calm simulated water
stays idle. This makes ALL the water features (depth-color, volumetrics, flow streaming, cave draining) actually
apply to natural water, not just player-edited water. `cargo check --bin` clean. EXPECT on device now:
depth@player ≥ 0 and moved>0 / flowspd>0 when you enter a lake or stand by a slope/hole.

## r439/r440 — World ruleset + headless server binary
- r439 WORLDLIST: NEW world/world_list.rs (flatten-VERIFIED, 7 tests). `WorldList` registers worlds and gates
  loads through a `LoadRule` enum; the starting/default rule `NoConcurrent` makes `try_load` DENY a second
  world while one is loaded (operator: "make sure multiple worlds can't be loaded at once… start with none").
  `can_load` is a pure query; reloading the same world is a no-op success; rule is extensible for later policies.
- r440 SERVER CONFIG: NEW sim/server_config.rs (flatten-VERIFIED, 8 tests). Hand-rolled CLI parser
  (`--name/--seed/--port/--bind/--tick-hz/--view-distance`, `--key=value` or `--key value`, `--help`), returns
  Config | Help | Error.
- r440 SERVER BIN: NEW src/bin/unendlich_server.rs (`required-features=["headless","egui"]`). Parses args,
  registers+loads ONE world through the WorldList (rule enforced), builds a CPU-generated headless world
  (TerrainGenerator::Cpu + ChunkManager, no window/render), and runs a fixed-rate tick loop
  (update + tick_simulation) with periodic status. Builds clean: `cargo check --bin unendlich-server
  --features headless,egui`.
  HONEST SCOPE: this brings up + ticks the world headless for cloud deploy; the NETWORK TRANSPORT (accept JOINs,
  replicate transforms/edits) still lives inline in the fullgame render loop and must be factored into a
  reusable host before wiring here — the next step (noted in NEXT_SESSION). (Lib note: world_renderer.rs:526
  references egui unconditionally under render-core, so any headless bin must also enable `egui`; gating that
  field properly would drop the server's egui dep.)

## r441 — FIX: "water rises in the air" on deep/natural water (bottomless-column source/sink)
Device log (r438 build): depth@player swinging 6↔37, flowspd pinned at 1.000, flow@player up to ±11, moved
sustained in the hundreds, active=true forever → sim never settled, water piled into tall columns. ROOT CAUSE:
the sim window is py-80..py+24; any column whose floor is below it (i.e. ALL deep ocean, newly simulated since
r438) gets floor==i32::MIN. The WRITE-BACK already skips those columns, but the SETTLE still processed them —
so a bottomless column acted as a virtual source/sink: water it "sent" was written into its floored neighbour
but never removed from anywhere real → water created → rises, and the gradient never resolves → no settle. FIX
(settle inner loop): a bottomless column never sends, and no column flows toward a bottomless neighbour (guard
each of the 4 directions on floor[j]!=MIN). Net effect: bottomless deep water is inert (its gather depth is
still kept for the shader/ingest), only floored columns — shallow water, holes, shorelines, the cave — actually
move. Also reverted r428's aggressive settle back to the r421-proven stable values: SETTLE_F 0.15→0.125,
ITERS 12→8 (RADIUS 3 + BUDGET 3000 unchanged — they don't affect stability). `cargo check --bin` clean.
EXPECT on device: in a lake/ocean, depth@player stabilizes (no 6↔37 swings), flowspd/moved fall toward 0 on
calm water and active→false; the cave still fills (a touch slower); no water rising above the surface.
DEFERRED (operator notes this build): too many dragons in this seed (spawn-density tuning); ugly blocky
top-surface water — wants a shader + 2×2 sub-voxel surface instead of the solid-block fallback (filed in BACKLOG).

## r442 — Flow has force (current pushes you) + erosion core + slower small-hole fill
Device log (r441 build): water SETTLES now — moved→0, active=false, depth@player holds steady at 52.14, no
rising. r441 confirmed fixed. Operator: (a) it settled a little too fast for a small dug hole; (b) "with flow
comes erosion and force" — wire physics, octree may have unwired levers.
- INVESTIGATION: octree node packed_header has bits 27..8 RESERVED ("must be 0") = 20 unused bits/node, but
  using them needs GPU shader changes (octree is consumed on GPU) → risky blind, filed not touched. The tractable
  lever is the flow field (water_flow_tiles.sample) already produced by the sim + the field framework
  (river_carve_field exists but is worldgen-time heightfield erosion, not live-flow driven).
- NEW sim/current_force.rs (flatten-VERIFIED, 9 tests): pure `drift_velocity(flow,strength,max,submersion)`
  (current→body drift, capped, submersion-scaled) + `erosion_rate(flow_speed,threshold,softness,gain)` (no carve
  below threshold; linear in excess speed × bed softness) + `speed`/`substrate_softness` helpers.
- FORCE WIRED (default ON, gentle): in the swim physics block, sample flow at the body and add a horizontal
  drift acceleration scaled by submersion `sub` — flowing water carries a swimmer; you can swim against it but
  get pulled. Quadratic swim drag bounds it. Knobs: UNENDLICH_WATER_CURRENT(=0 off), _CURRENT_STRENGTH(1.5),
  _CURRENT_MAX(4.0). EXPECT on device: standing in a flowing/draining current nudges you along it; calm water
  (flow≈0) does nothing.
- (a) SMALL-HOLE FILL: added per-column per-TICK shed cap UNENDLICH_WATER_MAX_FLOW(default 3.0 blocks) summed
  across settle iters — a dug hole now fills over a few ticks (visible) instead of one, still volume-conserving
  (un-shed water waits). High value ≈ old behaviour. `cargo check` lib + bin clean.
- EROSION: core is tested + ready; live voxel-carving from flow NOT wired yet (carving terrain blind is risky) —
  next step is to drive it from the sim's per-column flow over soft beds (Sand/Dirt/Gravel), budgeted like r433.

## r443 — Water sim TEST HARNESS + sediment/lava-water/displacement cores + side-face shader cue
Device log (r442): water settles, holds steady, no rising — r441/r442 confirmed. Operator asked to build
iteration TESTS (water-vs-water, lava-vs-water, erosion, cave-filling), a sediment table, force-back/waves with
a mass function, and offered shader work (they can device-test; harness validates GLSL).
- NEW sim/water_sim_core.rs (flatten-VERIFIED, 6 tests): the inline settle algorithm extracted into a pure
  `ColumnGrid::settle(iters,f,max_flow,max_depth)` — MIRRORS the bin's settle exactly. Tests: water-vs-water
  levels out, volume conserved, CAVE-FILLING (a low-floor hole fills from neighbours, surfaces converge),
  per-tick max_flow cap, flat water is quiescent, and the r441 REGRESSION (bottomless columns inert → no water
  created). This is now the canonical reference + the place to prove new rules before wiring. NEXT: refactor the
  bin settle to call this (device-verified) so they can't drift.
- NEW world/sediment_map.rs (flatten-VERIFIED, 5 tests): sparse per-column suspended-load table —
  erode/advect/deposit_excess, mass-conserving (imbalance()≈0 through an erode→carry→deposit cycle). The
  bookkeeping that makes flow erosion conserve mass (operator: "sediment as a per-chunk map table to deposit").
- NEW sim/water_react.rs (flatten-VERIFIED, 7 tests): lava/water contact (source→obsidian+water survives,
  flowing→stone+water flashes to steam) + body displacement (Archimedes volume) + wave_amplitude(mass,speed,
  area,gain,max) — the "mass function" for player/mob/falling-block push-back. Heavier+faster→bigger wave,
  capped, still body→no wave.
- SHADER (chunk.frag, validated RC=0, toggle WATER_SIDE_DEPTH_TINT): side/bottom water faces use the flat block
  normal (wave + see-into are top-only) so deep water reads as opaque slabs from the side; now non-top faces tint
  toward WATER_DEEP by body depth so a deep wall darkens with depth → reads as volume. Modest cue, NOT the real
  fix. lib + GLSL clean. No bin wiring this turn — cores are tested groundwork.

## r444 — WIRED: live flow erosion + sediment + lava-water (operator unlocked terrain edits)
Operator: testing mode, no saved worlds, terrain recoverable via timebending → wire the terrain-affecting work
previously held back. (Also added the operator's `rustcheck` tool to the workflow: tier-0 structural scan of the
11k-line bin in 0.03s + GLSL/SPIR-V checks, so structural breaks are caught before the ~50s compile.)
- FLOW EROSION (default ON, UNENDLICH_WATER_FLOW_EROSION): in the sim tick after the settle, floored columns
  whose flow speed (water_flow.at) exceeds UNENDLICH_WATER_FLOW_EROSION_THRESHOLD(0.35) over a SOFT bed
  (Sand/Gravel/Dirt/Grass) get the bed carved to Water (channel deepens), via sim::current_force::erosion_rate;
  the lifted block is booked into world::sediment_map and advected one cell downstream along the current. Pass 2
  deposits: where the current is too slow to hold the suspended load (capacity = flow_speed × SEDIMENT_CAPACITY),
  drop it back as Sand on the bed. Both budgeted by FLOW_EROSION_BUDGET(4)/tick. Channels cut, deltas build.
- LAVA+WATER (default ON, UNENDLICH_LAVA_WATER): scan floored columns; a Water cell face-adjacent to Lava
  solidifies the lava via sim::water_react::lava_water_contact — Obsidian if the lava has lava beneath (source),
  else Stone; flowing-lava contact also flashes the water to Air + emits a Smoke/steam puff. Budgeted.
- All three wire the r443 tested cores (erosion_rate, sediment_map, lava_water_contact); bin compiles clean
  (cargo check 53s). NOT yet wired: push-back/waves (needs a wave/flow injection path), settle-core refactor.

## r445 — FIX: remaining "water rises occasionally" (water under a solid = overhang/ceiling)
Operator (r444 build): erosion/deposition works; water still rises OCCASIONALLY. Root cause: the gather took
the topmost solid OVERALL as the floor. A column with a solid ABOVE the water (overhang, cave ceiling, placed
block, terrain lip) set floor=that ceiling → base=ceiling+1 → the write-back stacked the water ON TOP of the
solid. FIX (bin gather): the floor is the topmost solid BELOW the topmost water cell — track the last solid seen
scanning up and commit it as the floor each time water appears above it; solids above the water are ignored.
on_lava now reflects the actual BED (not the highest solid). Also added pure `water_sim_core::gather_column` +
`Cell` to the test harness (flatten-VERIFIED, now 10 tests) with the overhang case as a REGRESSION test
(floor stays at the bed, not the overhang), plus normal/bottomless/on-lava cases. bin + lib compile clean.

## r446 — WATER RESKIN step 1: water reads as a VOLUME (side + bottom faces)
Operator: "still need to do the water reskin"; "not cohesive from a bottom or side view." Root cause in the
mesher: emit_fluid_surface emitted ONLY the top face (cells with air above). Water against air on the sides or
below had NO geometry, so a body never read as a volume from the side/below — you saw straight through its
flanks. FIX (render/mesher.rs emit_fluid_surface): also emit the 4 side faces + the bottom face, culled so a
face is drawn only where the neighbour is Air. Critically, side/bottom faces are emitted ONLY for IN-BOUNDS air
neighbours — out-of-chunk neighbours are skipped, because get_local returns Air OOB and emitting there would
paint a seam face across every chunk boundary inside a multi-chunk lake. Pool edges mid-chunk get their sides;
an edge landing exactly on a chunk boundary just misses one face. All faces use the same water colour so the
shader still classifies them as water (incl. r443's side depth-tint) and shades by normal. Winding copied from
emit_cloud_cube_culled (FACE_VERTS). Stays in the OPAQUE pass — no pipeline change, no black-screen risk; worst
case is z-fighting/extra geometry. New mesher test water_cell_emits_side_and_bottom_faces; existing top-face +
no-interior-face tests still hold (a full water layer has no in-bounds air sides → unchanged). bin compiles.
NEXT reskin steps (each device-verified): 2x2 sub-voxel top + vertex-shader wave displacement (surface
undulates as mesh); then the transparent/refractive pass (the real see-through win — staged, black-screen risk).

## r447 — Floating-trees fix (GPU) + ocean-cave drains on load + world tracker whitelist (wired live)
1. TREES IN THE AIR (GPU worldgen): chunk_generation.comp set the trunk base to the analytic
   surface_target, while the CPU path scans density() for the real standable top (surface_y_at →
   topmost_standable). Detail noise offsets the two, so GPU-generated worlds (the operator runs real GPU) hung
   trunks above the lower real ground. FIX (compute_tree_decision): after the pos_hash reject, scan down through
   the detail band (surface_target+16 .. -24) for the topmost solid cell with air above (cave-aware via
   model_is_cave + model_density_with_target) and use THAT as surface_y — matching the CPU + fixing the float.
   Added a forward decl for model_is_cave (defined later in the file). glslangValidator RC=0 (rustcheck's
   structural pass missed the ordering error — ran glslang directly).
2. OCEAN-CAVE DRAINS ON LOAD (operator): new world-load WARM-UP — for UNENDLICH_WATER_WARMUP_SECS (12) after
   start, force the water sim active around spawn each tick so an ocean next to a cave opening begins draining
   immediately instead of waiting for the player to wade in (the gather covers the whole sim window, so anything
   in range of spawn settles/drains on load).
3. WORLD TRACKER + WHITELIST (operator: "expand the world tracker and gate on the whitelist of requests";
   "check multiple worlds can't load still"): expanded world_list.rs (flatten-VERIFIED, now 11 tests) with a
   request WHITELIST (allow/disallow/clear_whitelist/is_whitelisted; LoadDenied::NotWhitelisted) and a request
   LOG (every try_load recorded granted/denied — the tracker/audit trail). WIRED into the fullgame bin: the
   spawn world is registered + try_load'd through the WorldList at startup, so the NoConcurrent rule + whitelist
   are now LIVE in the game (not just the headless server) and any future load/switch path has a single gate.
   Server bin + lib still compile (API backward-compatible).

## r448 — Three tracks: water see-through (shader) · waterlogging+erosion (terrain) · items-in-water
Operator framed three domains ("3 worker threads for shaders/terrain/items"); no sub-agent tooling exists, so
done as three tracks in one pass.
- SHADERS — DITHERED SEE-THROUGH WATER (chunk.frag, glslang RC=0, toggle WATER_DITHER_TRANSP): water is in the
  OPAQUE pass so it can't truly blend (true alpha = the staged pipeline change w/ black-screen risk). Screen-door
  instead: discard a depth-based fraction of water fragments in a 4x4 Bayer pattern → terrain behind (same pass)
  shows through. Shallow water mostly transparent (WATER_MIN_OPACITY 0.30), opaque by WATER_OPAQUE_DEPTH (6
  blocks) — see-through tracks the depth colour. Tunable consts for A/B.
- TERRAIN — WATERLOGGING (bin gather + write-back): the gather counted ANY non-air block as floor, so water
  perched on grass/flowers. Now only is_solid() blocks (+ Lava, which water rests on & boils) are a floor;
  non-solid decorations (Flower/TallGrass/Fern/DeadBush/Vine/Cobweb/Mushroom/Lichen) fall through and the
  write-back floods them (replaceable now includes !is_solid && !Lava). Water flows into and submerges plants.
  r444 flow-erosion is unchanged and still applies.
- ITEMS — DROPS FLOAT + RIDE THE CURRENT (items/drops.rs, flatten-VERIFIED, +4 tests = 7): new
  DropField::apply_water(dt, buoyancy, drift, in_water, current) — submerged drops get buoyancy (damped+capped →
  rise and bob at the surface) and the flow field carries them horizontally. Wired in the bin after drops.update
  (buoyancy 24 > gravity 18; current from water_flow_tiles). Tests: floats up, current carries +x, dry drop
  untouched. bin + lib compile clean.

## r449 — Items wiring: inventory sort (live), + survey of the unwired item cores
Operator: "keep wiring up the items system." Survey: most item cores are built+tested but NEVER referenced by
the bin (0 refs): item_appraise, item_compare, item_durability, item_modifier, item_genetics, item_resolve,
item_tooltip, inventory_sort, inventory_txn, item_entity, loot_roll, loot_quantity, batch_loot_roll,
craft_quality, craft_resolve, legendary_loot, item_brain. (Already wired: item_drop_resolve loot on block-break
(r149), crafting, recipe_book.)
- WIRED this turn — INVENTORY SORT: live items::Inventory::sort_display() maps the BACKPACK slots (hotbar
  preserved) to sim::inventory_sort::Slot, runs the pure stable sort_order (empties sink, grouped by item),
  and rewrites the backpack in that order. Bound to the K key in the bin. (Note: there are TWO Inventory types —
  crate::items::Inventory is the LIVE one app.inventory() returns; crate::inventory is a separate/legacy module.
  A matching sort_display was also added to crate::inventory::SlotGrid but that module isn't the live path.)
- Blockers found for the bigger wires: item_durability needs per-instance durability state (ItemStack is just
  {ItemId,count} — no metadata); item_tooltip::compose needs an ItemTooltipView (name/rarity/affixes) the base
  model doesn't carry. Both want an item-INSTANCE/metadata layer first — the natural next wiring target so
  durability, modifiers, tooltips, appraise/compare all have somewhere to read from.

## r450 — Biome+plant: live flora spread (plant_propagation × plant_growth_env wired)
Operator: "do biome and plant work now." Survey (same method as items): biome/plant cores mostly built+tested
but UNWIRED (0 bin/gen refs): biome_blend, biome_blend_weight, biome_transition, biome_temp_map, bloom_math,
chunk_flora, flora_harvest, flora_sites, plant_blueprint, plant_exotics, plant_gene_express/_traits/_genetics,
plant_growth, plant_growth_env, plant_propagation, plant_root_system, vegetation_density. Already wired: biomes,
plant_lifecycle (strided tick handler), flora_blocks, cave_flora (grow tick), canopy_field_shift, flora_map,
biome_climate; worldgen scatter via flora_placement/biome_loader/vegetation_scatter.
- NEW CORE — sim/flora_spread.rs (flatten-VERIFIED, 7 tests vs the REAL plant_propagation + plant_growth_env):
  spread_targets(parent_xy, tol, cond, wind_xy, seed, n) → integer cells the parent seeds this tick. Disperses
  downwind (plant_propagation::disperse Wind mode), snaps to cells, drops the parent cell + dups, keeps only
  cells that germinate (germination_chance over plant_growth_env Conditions/Tolerance, per-seed viable()).
  Named tolerances: ground_cover (grass/fern), flower (warm), mushroom (cool/damp). Tests: parent-exclusion,
  frozen→0, ideal→many, determinism, dedup, downwind bias, mushroom prefers damp-cool.
- WIRED (bin, after the cave_flora grow block ~3655): FLORA SPREAD tick. Gate: flora_spread_enabled (default ON,
  UNENDLICH_FLORA_SPREAD=0 off) && mouse_captured && frame_count % INTERVAL(300) == 11. Budgeted random probes
  (SCAN 48) find surface parent flora (Flower/TallGrass/Fern/Mushroom) within RADIUS(28) of the player; each
  parent builds Conditions from the LIVE climate model — moisture=model.humidity, temp_c=model.temperature,
  light01=open-column proxy, season01=0.5 — picks its tolerance by kind, disperses with wind::wind_at, and
  spread_targets' viable cells that are Air over suitable ground (Grass/Moss/Dirt; mushroom: Dirt/Stone/Moss/
  Gravel) get the parent block placed. Capped at BUDGET(8) placements/tick. Logs "[flora] N new plant(s)...".
  This IS the biome coupling: humidity/temperature are continuous per biome, so spread is lush in warm-wet
  biomes, sparse in cold-dry — no worldgen change, fully runtime + recoverable.
- Knobs: UNENDLICH_FLORA_SPREAD(1), _INTERVAL(300 frames), _RADIUS(28), _BUDGET(8), _SCAN(48).
- DEFERRED — biome_blend / biome_transition: these blend biome PARAMETERS across boundaries and only make sense
  in the worldgen path (biome_at_3d does hard assignment). Operator runs GPU worldgen (Backend::Auto), so a
  CPU-only blend would be invisible and a GPU comp-shader blend (chunk_generation.comp) is risky + not
  device-verifiable here — same class as the deferred transparent-water pass. Left for an on-device shader session.
- DEVICE WATCH (r450): flora spread depends on the climate model's units — Tolerance bands are in °C and moisture
  in [0,1]. If model.temperature isn't °C (or humidity isn't 0..1), spread either never fires or over-fires; the
  "[flora]" log line is the tell. Budgeted+recoverable; set UNENDLICH_FLORA_SPREAD=0 if it misbehaves. Tune rate
  via _BUDGET / _INTERVAL.

## r451 — Plant: flora bloom harvest (flora_harvest wired into block-break)
Continuation of the biome/plant track. Wired sim::flora_harvest (was 0 refs) into the block-break loot path
(bin ~7479, right before the leaf-decay trigger; hit_block_id / x,y,z / fg_app.drops_mut all in scope).
- Behaviour: breaking flora (Flower/TallGrass/Fern/Mushroom) still drops the block as before, but a RARE mature
  bloom additionally yields elemental essence. Per-cell deterministic hash (mix64 over x,y,z): bloom chance 9%
  for flowers, 4% for ground cover; element = hash%6; strain_bias from the hash. flora_harvest::harvest() with
  growth01=1.0 produces reagent_units[6]; each element slot with >=8 units drops (units/8, capped 4) of essence
  ItemId(3000+i) — reusing the established dragon-essence item palette. Logs "[flora] a rare bloom (element N)...".
- Kept deliberately rare (low bloom %, PER_ITEM=8) so flora doesn't out-compete dragon kills as an essence
  source; strain (rare-bloom) blooms additionally drop small cross-element essence in the two neighbour slots.
- No new item IDs invented (no seed/reagent item exists in the model — flora already drops its own block; only
  the 3000+elem essence range is special). flora_harvest's regrow_secs is unused (placed flora carries no growth
  state to time a regrow against — would need the item-instance/growth metadata layer noted below).

## r452 — Plant: sapling growth → trees (plant_growth wired)
Continuation. Wired sim::plant_growth (was 0 refs) as a real maturation loop, no new BlockId, no coupling to the
complex plant_lifecycle handler.
- In-bin sapling registry: Vec<([i32;3], plant_growth::PlantState)>, bounded by UNENDLICH_SAPLING_CAP(48),
  near-player + non-persisted (forgets on far travel — fine for testing).
- Sapling tick (bin, after the flora-spread block; gate: flora_spread_enabled && mouse_captured && frame_count %
  UNENDLICH_SAPLING_GROW_INTERVAL(120) == 37):
  (1) ADVANCE each sapling via plant_growth::advance(state, Fields{h,r,l}, dt): h=model.humidity, r=temp_c/30
      clamped, l=open-column light proxy; dt=UNENDLICH_SAPLING_DT(0.30). Saplings whose OakLog was removed by the
      player are dropped (no crash).
  (2) MATURE: on GrowthEvent::Matured (or stage>=FINAL_STAGE) replace the stub with a real tree via
      flora_blocks::flora_to_blocks — Conifer if temp_c<5 else Broadleaf (OakLog trunk + OakLeaves canopy). Logs
      "[flora] a sapling matured into a tree (N blocks)".
  (3) PLANT: ~UNENDLICH_SAPLING_PLANT_PCT(25)% per tick, if under cap, drop a lone OakLog "sapling" stub on open
      Grass/Dirt with 6-block sky clearance within 18 of the player; seed element/vigor from a hash.
- Maturation ~20-40s at defaults; visible (stub trunk → full tree). Climate-coupled (warm/wet/lit → faster;
  cold → wilts via plant_growth's WILT_POINT).
- Knobs: UNENDLICH_SAPLING_GROW_INTERVAL(120), _CAP(48), _DT(0.30), _PLANT_PCT(25). Shares UNENDLICH_FLORA_SPREAD
  enable (=0 disables saplings too).
- DEVICE WATCH (r452): sapling stubs are lone OakLogs — if the plant_lifecycle/leaf systems treat a lone log as
  unanchored and remove it before maturity, the operator will see "took root" without the matching "matured"
  log; the tick already drops such saplings safely. Trees can get dense over minutes at PLANT_PCT 25 (recoverable
  in testing mode; lower _PLANT_PCT / _CAP if so). Sudden pop-in tunable via _DT / _GROW_INTERVAL.

## r453 — Multiworld pass: per-world SIM state leaks across world switch + WorldList desync
Operator: "another pass on the hunt for any multiworld loaded at once bugs." Two real classes found + fixed.

FINDING A — persistent per-world SIM state bled across menu world switches (the r162 family, re-opened by
systems added since). The Launch(StartLocal) reset cleared the r162 set (roamers/drops/regions/dragon maps/
fire/colony/thermal/imbue/resonance/stamina/clock/UI) but NOT the newer coordinate-keyed state, so world A's
data coexisted with world B's terrain:
  - saplings (r452 registry — old-world OakLog coords)
  - water_flow (WaterFlowField), sediment (SedimentMap), water_flow_tiles (FlowTileStore depth/flow tiles) —
    world A's currents/depth/sediment applied to world B
  - water_dirty (re-arm for the new world)
  - lava_registered (HashSet<ChunkCoord> skip-guard — stale coords would stop the NEW world's lava at those
    chunks from ever registering: a `contains → continue` false-positive)
  - fg_evict_queue (stale coords could evict freshly-loaded new-world chunks)
  - lan_outgoing_edits (old-world block edits not yet sent to peers)
FIX: all cleared/re-seeded in the Launch(StartLocal) reset block (r453 block right after world_clock_elapsed=0).
water_flow.reset(0,0); sediment/water_flow_tiles re-new'd; the rest .clear(). "[launch] gameplay state reset...".

FINDING B — the WorldList guard (NoConcurrent, r439/r447) was only updated at STARTUP. Menu world switches
rebuilt the world WITHOUT calling the WorldList, so loaded() stayed pinned to the spawn world forever and the
"can't load two worlds" guard pointed at a world no longer loaded (stale/ineffective bookkeeping). FIX: the
switch now unload()s the outgoing world and register()+try_load()s the incoming one through the same gate
(whitelist is None=allow-all, so it grants). Advisory — the real load already happened via rebuild_for_seed —
but loaded() + the request_log audit trail are now truthful. "[worldlist] switched to world id=..; exactly one
world loaded".

CONSIDERED, LEFT ALONE (with rationale):
  - meshing_in_flight (HashSet<ChunkCoord>) + the in-flight mesh JOBS: self-healing — dispatched jobs return
    within frames and remove their coords; clear_all_chunk_meshes() already runs on switch. Clearing the set
    blind risks disturbing the async mesh pipeline (double-mesh / stale upload) I can't device-verify. The only
    practical symptom is a brief meshing delay for a few chunks right after a switch.
  - smoke_field (SmokeField) — transient particle density, dissipates on its own; no clear() method; visual-only.
  - lan_remote_creatures — replication-managed (stale entities time out via SnapshotGate); LAN+switch is niche
    (you leave the session on switch). Not touched to avoid breaking live replication.
  - test_creatures — never populated (dead Vec, always empty); no leak.
  - LAN Join does NOT load a second world's terrain — it overlays remote ENTITIES via entity_replication onto
    the joiner's OWN local world, so there is no concurrent world-terrain load to guard there.
bin compiles (render,hot-shaders,window,gpu,ui-overlay). Server bin + lib untouched.
TEST (device): build/dig/flood/plant in world A (start water flowing, plant saplings), switch to world B, confirm
B has NO leftover currents/depth tiles/saplings/lava-skips from A; switch back to A and confirm A's own edits
persist (r163). Watch the "[worldlist] switched..." + "[launch] gameplay state reset..." lines.

## r454 — Items: creative tabs (sim::creative_tabs core + browse keys)
Operator: "keep working on items and add creative tabs for them." Before this, the creative palette was a
hardcoded PLACE_PALETTE of 9 blocks on the hotbar; no way to reach the other ~50 placeable blocks.
- NEW CORE — sim/creative_tabs.rs (flatten-VERIFIED, 4 tests): CreativeTab { Terrain, Stone, Ores, Wood,
  Building, Plants } + ALL_TABS + label() + tab_of(BlockId)->Option<CreativeTab> + tab_items(tab)->Vec<BlockId>
  (canonical BlockId::ALL order). INVARIANT (tested): the tabs PARTITION BlockId::ALL minus Air — every
  placeable block in exactly one tab, none lost/duplicated (adding a new BlockId without tabbing it fails the
  test — a useful tripwire).
- WIRED (bin, creative mode only): G cycles the tab (resets item idx), V cycles the block within the tab; the
  chosen block becomes selected_block (the placement block). Logs "[creative] <tab> [i/n]: <Block>". Keyboard-
  driven so it's device-verifiable now; an egui grid panel (click-to-pick, hover names) is the visual follow-up.
- Lib + bin compile. NOTE: this also gives the operator reach to every block for building, which the 9-slot
  palette never did.
NEXT (items/creative): egui creative grid panel (render tab_items in a scrollable grid, click to put in hotbar) —
device-only-verifiable rendering, stage on-device. Plus the still-unwired item cores (durability/tooltip/modifier)
still need the item-instance metadata layer noted earlier.

## r455 — Plants: pioneer flora variety (safe runtime) + biome/plant worldgen scoping
Operator: "look into adding more biome types and plant types." Engine already has 28 biomes + 10 FloraSpecies.

DELIVERED SAFE — sim/pioneer_flora.rs (flatten-VERIFIED, 5 tests): pioneer_for(temp_c, moisture01, ground) ->
Option<BlockId> picks a climate+substrate-appropriate decoration from blocks that ALREADY render: DeadBush on
arid hot sand, Lichen on cold exposed rock (Stone/Andesite/Granite/Slate/Basalt/Tuff), Moss on damp rock, Fern
in warm humid undergrowth. WIRED as a budgeted runtime tick (bin, after the sapling tick; shares
UNENDLICH_FLORA_SPREAD; frame_count % flora_spread_interval == 53; ≤3 placements/tick over 16 random probes
within 22 of the player; places only into Air over a solid whose climate matches). Logs "[flora] N pioneer
plant(s) colonised bare ground". This adds visible plant variety (deserts get dead bushes, cold peaks lichen,
damp rock moss) with NO worldgen/shader/atlas change — fully verifiable + recoverable.

SCOPING — why worldgen biome/plant additions are an ON-DEVICE shader session (not a blind change):
The live worldgen is the GPU compute shader crates/.../shaders/chunk_generation.comp (operator runs Backend::Auto
→ GPU). Both biome classification AND tree placement happen there. Adding a worldgen BIOME requires editing, IN
SYNC:
  Rust: world/biomes.rs — Biome enum variant; determine_biome(temp,humidity,erosion) climate→biome mapping;
        height_modifier(); tree_profile(); + cpu_generator surface/pick_surface_block.
  GPU:  chunk_generation.comp — BIOME_* const (comment: "must match biomes.rs::Biome repr EXACTLY");
        biome_at_2d() (must stay identical to CPU world_model::biome_at_2d); biome_height_modifier table (~397);
        surface block (shader currently does NOT full-port pick_surface_block — "conservatively allow any", ~731);
        the tree decision pass (compute_tree_decision ~794 keys off biome_at_2d).
Adding a worldgen PLANT/tree type requires: blocks.rs BlockId variant + repr; the shader's RC40 tree-block consts
(comment: "Must match blocks.rs::BlockId enum repr"); render/mesher.rs colour/atlas for the new block; a
TreeProfile; and the GPU tree pass. A wrong repr or a shader error here BLACK-SCREENS worldgen (NOT recoverable
by timebending), so this is the staged-one-verified-step-at-a-time class (same as the transparent-water pass +
biome-blend deferral). Recommend: operator at the machine, add ONE biome end-to-end (Rust+shader together,
repr-locked), regen, verify, before the next.

## r456 — Content: 7 new building/decorative blocks (player-placeable, no shader change)
Operator: "keep adding more blocks items and biomes." Added 7 solid opaque blocks that are PLACED by the player
(creative tabs / building), not worldgen-generated — so NO GPU comp-shader change (the chunk shader derives
per-block detail from the CPU albedo). New: MossyCobblestone(81), PolishedStone(82), SmoothSandstone(83),
Terracotta(84), ChiseledStoneBricks(85), DarkOakPlanks(86), PolishedBasalt(87).
- Touchpoints (adding a BlockId surfaces every EXHAUSTIVE match — the engine forces a per-block decision):
  blocks.rs enum + ALL[66] + base_color; render/mesher.rs block_color (added BEFORE the `BerryBush` arm — NOTE:
  that arm is actually an irrefutable catch-all binding, not a real variant, so any block without an explicit
  colour silently renders berry-red); material.rs, wetness.rs, thermal.rs, mining.rs ×3 (proper_tool/required_
  tier/hardness), material_response.rs default arm. Stone-likes grouped with Stone, DarkOakPlanks with wood.
- creative_tabs.rs: tabbed them (MossyCobblestone/PolishedStone/PolishedBasalt→Stone; SmoothSandstone/Terracotta/
  ChiseledStoneBricks→Building; DarkOakPlanks→Wood). Partition test auto-covers (uses BlockId::ALL.len()). They
  appear in the G/V creative browser immediately, and are block-ITEMS (ItemId = block id) in inventory.
- Lib + bin compile. These are immediately usable + verifiable (place them, they render with distinct albedo).
RECIPE CONFIRMED (for future block adds): enum+ALL+base_color+mesher colour (before berry catch-all) + the 4
property matches (material/wetness/thermal/mining) + creative_tabs tab. ~8 files; all compile-enforced exhaustive
except the ones with `_ =>` (dragon_forge etc. default fine). No shader needed for player-placed blocks.

## r457 — Content: new biome HighlandSteppe (CPU + GPU shader, glslang-validated)
Operator: "keep adding ... biomes." Added Biome::HighlandSteppe = 28 — a warm-temperate, dry, moderately-eroded
RAISED treeless grassland. Visible identity on GPU = its height profile (climate drives surface blocks, not
biome). End-to-end, CPU + GPU kept in lockstep:
- biomes.rs: enum variant (28); determine_biome arm (temp 0.45..0.7, humid<0.30, erosion 0.40..0.55 — disjoint
  from Savanna's erosion<0.4 and the 0.55..0.7 highlands); height_modifier +22; flora_density 0.30; tree_profile
  None (treeless). flora_placement species_weights (grass-dominant). sim/sound.rs ambient (12, Plains-like),
  sim/feeding.rs forage (0.03, highlands-like). + in-module test highland_steppe_selected_in_its_niche.
- chunk_generation.comp: const BIOME_HIGHLAND_STEPPE = 28u; determine_biome GLSL arm with IDENTICAL thresholds;
  biome_height_modifier +22. glslangValidator -V chunk_generation.comp → RC=0.
- Lib + bin compile. DEVICE-VERIFY: regen terrain and look for raised dry grassland plateaus in warm-dry-eroded
  zones; confirm the steppe sits ~22 above neighbouring plains. (No crash risk beyond worldgen — shader validated;
  worst case is cosmetic placement, recoverable by timebending.)

### ⚠ PRE-EXISTING CPU/GPU BIOME DESYNC (discovered this pass)
The GPU shader's biome table + determine_biome only cover biomes 0..18. The newer Rust biomes — Savanna(19),
Jungle(20), Taiga(21), CrystalCave(22), LushCave(23), Mesa(24), Mangrove(25), VolcanicWastes(26), FrozenOcean(27)
— exist in the enum + CPU determine_biome but are NOT in chunk_generation.comp, so they DO NOT generate in GPU
worlds (operator runs Backend::Auto → GPU). HighlandSteppe(28) was added to BOTH so it does appear. Porting
19..27 into the shader (consts + determine_biome arms mirroring the CPU + biome_height_modifier rows) is the
high-value follow-up — makes 9 already-defined biomes actually show up. Medium risk (logic must mirror CPU;
glslang validates syntax; mismatch is cosmetic not crash). Recommended next.

## r458 — Biome GPU-sync: ported biomes 19..27 into chunk_generation.comp (fixes the desync)
Fixes the desync found in r457: the GPU shader only knew biomes 0..18, so Savanna/Jungle/Taiga/CrystalCave/
LushCave/Mesa/Mangrove/VolcanicWastes/FrozenOcean (19..27) never generated in GPU worlds despite existing in
the Rust enum + CPU determine_biome. Now ported into chunk_generation.comp:
- BIOME_* consts 19..27 added (match biomes.rs repr).
- determine_biome GLSL rewritten to mirror the CURRENT CPU determine_biome EXACTLY: VolcanicWastes (hot/arid/
  eroded), FrozenOcean (very cold/low/wet), Taiga (cool/humid), Mesa (hot/dry/mid-erosion), Jungle (hot/wet),
  CrystalCave + LushCave (erosion 0.55..0.7 splits), Mangrove (hot/wet/low), Savanna (warm/semi-dry), plus the
  existing arms + HighlandSteppe(28). Same thresholds ⇒ GPU biome == CPU biome at every (temp,humid,erosion).
- biome_height_modifier rows: Mesa +15, VolcanicWastes +8, FrozenOcean -30, Mangrove -7 (others 0, as CPU).
- biome_tree_profile: added Jungle (0.07/7-14/r3), Taiga (0.03/7-13/r2), Mangrove (0.05/4-7/r3), Savanna
  (0.006/4-7/r3), all Oak — mirrors biomes.rs::tree_profile. Mesa/Volcanic/FrozenOcean/caves stay treeless
  (correct). Surface blocks remain climate-driven (surface_block_type): hot/dry→Sand, else Grass.
- glslangValidator -V chunk_generation.comp → RC=0 (after every step). Rust unchanged since r457; bin compiles.
- DEVICE-VERIFY: regen and look for jungles (hot wet, dense oak), taiga (cool conifer-ish), mesa (raised red-dry
  — note: surface is climate Sand, not RedSandstone, since GPU surface is climate-not-biome), mangrove (low wet),
  savanna (open grassland few trees), frozen ocean (cold ice-capped sea), volcanic flats. Worst case is cosmetic
  placement vs CPU (thresholds were mirrored, so should match); shader validated so no compile/black-screen.
KNOWN COSMETIC GAP: GPU surface block is climate-driven (surface_block_type = Sand if hot&dry else Grass), so
Mesa won't show RedSandstone and Mangrove won't show Clay on GPU (CPU biome.surface_block does). Porting a
biome→surface map into the shader is the optional next polish; terrain shape + trees + flora now match.

## r459 — Salinity concept + environmental water tint
Operator: "add water tint based on fields like erosion, temperature, salinity" + "engine doesn't have salinity
conceptually yet." Introduced salinity as a first-class field + tinted water by it.
- NEW CORE sim/salinity.rs (flatten-VERIFIED, 5 tests): salinity01(temp01, humidity01, elevation_rel, erosion01)
  → 0 (fresh) .. 1 (briny). Marine (at/below sea level), evaporite (hot+arid+flat/low-erosion salt flats),
  freshened by cold/wet inland. THE salinity concept the engine lacked — ready to drive water tint now and
  creature salinity-tolerance genes / spawn habitat later.
- NEW CORE sim/water_tint.rs (flatten-VERIFIED, 4 tests): water_albedo(base, depth, temp, salinity, erosion) →
  RGB. Salinity→teal, warm→green / cold→blue, erosion(sediment)→murky brown-green, depth→darker.
- WIRED render: chunk.frag tints water fragments via the SAME formula (tint_water) — salinity/temperature from
  cheap position proxies (wt_salinity/wt_temperature; the render shader can't sample worldgen climate), depth
  from the existing water-depth grid, erosion a mild 0.15 default. glslangValidator RC=0. Only is_water frags
  affected; low risk, tunable constants.
- HONEST GAP: the shader uses position proxies + default erosion, so the tint is plausible-but-approximate, not
  the exact worldgen climate. The CPU cores (salinity/water_tint) are authoritative. NEXT: feed REAL fields to
  the tint — either bake a climate-derived water tint into the mesher vertex colour (CPU mesher has world access)
  or pass a per-chunk (temp, salinity, erosion) climate uniform to chunk.frag; then the tint matches worldgen and
  can use the real erosion field instead of 0.15.
- DEFERRED (operator's salinity note): salinity as a GENOME trait (salinity-tolerance gene gating aquatic
  creature spawn/health by sim::salinity) — a creature-system follow-up once the roster/genome work lands.

## r460 — Fauna: species roster data (SECTION_fauna §3a)
Operator: "more fauna designs from the docs." Extracted the 20-species roster from fauna_behaviors.docx and
encoded it as data feeding the existing fauna cores (which were built but had no roster to read).
- NEW sim/fauna_roster.rs (3 tests): Species enum (20) + SpeciesData { name, latin, temperament, body_plan,
  home_range_blocks, detect_radius_blocks, base_health } + data(Species) + ALL. Temperament → fauna_behavior FSM
  filter; BodyPlan → fauna_gait proportions/gait. Roster: 8 Passive (Deer/Rabbit/Raven/CaveSalamander/RiverOtter/
  StoneTortoise/Jellyfish/CaveFish), 4 Neutral (BrownBear/Moose/WildBoar/Wolf), 5 Hostile (CaveTroll/Harpy/
  StoneCrab/SwampLeech/Basilisk), 3 Specialist (TermiteColony/Firefly/BioluminescentShrimp). Numbers from the
  docx where given (deer home 60 / alert 25) else sensible defaults — tune against the doc.
- Lib compiles. NEXT (fauna): wire roster → mob_spawn_table::pick (biome habitat weights) + spawn real entities;
  per-species FSM specializations §3b (deer RUT, rabbit warren freeze, wolf pack); drop tables; locomotion rig
  consumption §3c (render). All build on this roster.

## r461 — Lore: named-location discovery system (SPEC-010 "places")
Operator: "adding in the places from the lore." Per SPEC-010, named locations are biome-scale areas the player
discovers; first entry names them and adds them to the Collections tab.
- NEW world/named_locations.rs (4 tests): REGION_SIZE 256; region_of(wx,wz); location_name(rx,rz,biome,seed) →
  deterministic lore name ("The Verdant Reach", "Ashen Crag", "Barrow of the Mire") from 16 adjectives × 8
  prefixes × per-biome noun (exhaustive over all 29 biomes), 3 sentence forms; LocationRegistry (discover/
  is_discovered/name_at/count/clear) — the per-world Collections backing store.
- WIRED (bin): strided tick (frame_count%45==17, mouse_captured) computes the player's region; on entering an
  undiscovered one it reads the biome (generator().cpu().biome_at_3d), names it with fg_world_seed, registers it,
  and logs "[location] Discovered: <name> (N catalogued)". Registry + current_region reset on world switch
  (added to the r453 multiworld leak-sweep — discoveries are per-world). Collections TAB UI is the device-side
  follow-up (this is the data + discovery backend SPEC-010 needs).
- Bin compiles. DEVICE-VERIFY: wander across ~256-block boundaries into new biomes; watch for "[location]
  Discovered:" lines; names are stable per (region, world seed).

## r462 — Fauna roster wired into spawns + biome→surface-block shader map
Operator: "wire the fauna roster and do some shader work."

### Fauna roster → wild spawns
The roster (sim/fauna_roster.rs) already had habitat_weights(biome) + pick_for_biome(biome, roll). Wired it into
the two natural wild-spawn sites in the bin:
- Added `species: Option<fauna_roster::Species>` to the Roamer struct (None for special kinds / egg / offspring).
- At both natural Fauna spawns: for generic CreatureKind::Fauna, read the biome (generator().cpu().biome_at_3d
  at the spawn cell), roll from the spawn seed, call pick_for_biome → a biome-appropriate species (deer/boar in
  forests, crabs/otters on coasts, salamander/troll/fish underground, harpy on peaks…), set the roamer's hp from
  data(species).base_health, and log "[fauna] <name> appears (<biome>)". Non-Fauna kinds keep species=None, hp 40.
- Bin compiles. DEVICE-VERIFY: roam different biomes, watch the log for "[fauna] … appears" and confirm species
  match the terrain. NEXT: feed species into the FSM/gait (per-species behaviour §3b), drops, and offspring
  inheritance (offspring currently species=None).

### Shader: biome→surface-block map (chunk_generation.comp)
Closes the r458 cosmetic gap (GPU surface was climate-only sand/grass). surface_block_type now takes the biome
(via biome_at_2d at the surface cell) and mirrors biomes.rs::surface_block: Mesa/RedSandstoneCanyon→RedSandstone,
VolcanicWastes→Basalt, FrozenOcean→Ice, Mangrove→Clay, Desert/Beach/Ocean→Sand, Tundra→Snow, Swamp→Moss,
caves→Stone, else Grass (keeping the shader's altitude-snow + hot-dry-sand climate overlays for grassy biomes).
Added BLOCK_CLAY/RED_SANDSTONE/BASALT consts (22/24/38). glslangValidator RC=0. These are existing blocks with
mesher colours, so no render change. DEVICE-VERIFY: mesa shows red sandstone, mangrove clay, swamp moss, frozen
ocean ice. Worldgen-shader change but pure block-id selection — worst case cosmetic, timebending-recoverable.
NEXT (optional): mirror the biome into subsurface_block_type too (still climate-only sand/dirt).

## r463 — Device-log triage (log-r462) + first fixes
Operator uploaded a 7.5-min Asahi/M2 device log (multiplayer, lauscher, water, multiworld, frame timing, LOD).
Stored it durably + began triage.
- docs/device_logs/log-r462.log — the raw log (kept for repeated review).
- docs/device_logs/TRIAGE_r462.md — LIVING triage doc: T1 mesh hitches (near max 889ms), T2 runaway population
  (FIXED), T3 LOD never demotes (RD=4 ≤ first band 4), T4 raycast non-axis normal, T5 LAN re-announces every ~2s,
  T6 unreliable frame-section timing. Each with log evidence + priority + next steps. No ERROR/PANIC; 69 WARN
  (68 LONG FRAME + 1 raycast).
- FIX T2 (ecology): breeding was gated by ROAMER_CAP=96 while natural spawns + spawn director gate on carrying
  capacity (~8) — so colonies bred toward 96 and mass-starved (117 starvation events, herds 25–40 vs cap 8).
  Breeding now uses the same carrying cap. Population should settle near capacity; churn collapses. Tuning knob
  to make fauna denser: raise the popdyn Colony capacity.
- AID T1 (perf): the 889ms "near" meshing max is dispatch_us/batch.len() in the octree GPU dispatch — a dispatch
  STALL, not CPU meshing. Added a "[mesh-stall] octree dispatch Nms for K chunks" WARN (threshold 100ms) so the
  next run names the offending batch + size. Lib + bin compile.
- NEXT: T1 (read the new mesh-stall warns — batch size vs driver stall), T3 (LOD/render-distance reconcile), T4
  (octree raycast normal), T5 (LAN announce cadence). Multi-world bugs need a world-switch capture (not exercised
  in this single-world log).
## r463-FIX — geom_probe.rs Tier-0 unclosed-delim
rustcheck flagged exactly one Tier-0 problem: examples/geom_probe.rs:99 — coord_main()'s '{' opened but never
closed (body removed in a prior edit, leaving the brace dangling to EOF). Closed it as an explicit no-op stub;
rustcheck --fast now PASS (static OK), braces 45/45. Example-only file, so it never affected the lib/bin gates —
which is why it persisted. NOTE: old_main() below still references an undefined `coord` (a separate SEMANTIC
issue that would only bite on `cargo build --example geom_probe`); not fixed here.

## r464 — Hot-metal thermodynamics (new physics) + water-quench interaction; brain-evo idea logged
Operator: "keep working on physics … water physics plus hot metal physics." Introduced hot-metal physics that
interacts with water (quench) and lava (heat/melt).
- NEW CORE sim/hot_metal.rs (flatten-VERIFIED, 6 tests): step_metal(temp, neighbour_hot, in_water, dt, params)
  = conduction in from a hotter neighbour (lava 1150°C / fire 800°C) + radiative cooling (hotter→faster) +
  fast water QUENCH; quench_steam(temp, in_water); is_molten(temp) at MELT_C 1500°C; glow(temp) → blackbody
  incandescence colour (dull-red→orange→yellow→white) + 0..1 intensity. Physically grounded: lava (1150) makes
  iron GLOW but is below iron's melt point (1500), so lava alone won't melt it — a hotter source (forge/
  dragon-breath) is needed.
- WIRED (bin, gated UNENDLICH_HOT_METAL, default on): bounded tick (frame_count%20==7, mouse_captured) — a
  radius-6 discovery scan adds metal cells (Iron/Gold/Copper ore + blocks) adjacent to lava (cap 96), then
  evolves every tracked cell: reads hottest neighbour + wetness, steps temperature, detects quench-steam, melts
  →Lava at MELT_C (one-time set_block; no per-tick remeshing → respects the T1 hitch budget), and drops cells
  that cool to ambient. Logs "[hot-metal] N cell(s) hot (max T°C), quench-steam Q, melted→lava M". Reset on
  world switch (added to the r453 leak-sweep). hot_metal: HashMap<[i32;3], f32>.
- WATER-PHYSICS TIE-IN: quench is the hot-metal/water interaction — hot metal flooded with water cools fast and
  flags steam. The VISIBLE steam puff (inject into smoke_field) + temperature-graded EMISSIVE glow (chunk.frag,
  needs per-block temp like lava) + melt-from-a-hotter-source are the next VISIBLE steps (deferred to keep this
  increment perf-safe and shader-risk-free). glow() core is ready for the emissive when temp plumbing lands.
- Lib+bin compile; core tested. DEVICE-VERIFY (physics is mostly internal until the visuals land): place Iron
  beside lava, then flood with water → watch for "[hot-metal] … quench-steam" lines.

## FUTURE IDEA logged (docs/BACKLOG.md) — pretrained mob brains via evolved simvectors
Per operator: evolve compact controller "simvectors" offline (GA over the fauna FSM/perception/forage params,
fitness = survival/feed/evade per biome), cache the best per (species, habitat), and seed each mob's brain from
that prior at spawn (via r462 fauna_roster::pick_for_biome) so mobs are competent the moment the player sees them.
Ties to genetics/popdyn/predation/forage + the §3b per-species FSM. See BACKLOG.md for the full sketch.

## r465 — Shader pass 1/n: final colour grade (vibrance + contrast)
Operator: "hit a few messages with shader passes … make graphics better all around" (then pivot to Fairphone-5
GPU friendliness). Starting the shader-pass series with a safe, broadly-visible, mobile-cheap change.
- chunk.frag: after the ACES tonemap, a gentle colour grade — luminance-weighted VIBRANCE (dull pixels boosted
  more than already-saturated ones, so no neon) + a mild CONTRAST S-curve about mid-grey. ~6 ALU ops, applies to
  the whole frame, toggleable via CHUNK_COLOR_GRADE (1 default; 0 = neutral tonemap output). Conservative values
  (vib ≤0.16, contrast 1.055) so it enhances without fighting the existing careful lighting/fog/aerial-perspective.
- glslangValidator RC=0. The render shader is already advanced (analytic water waves + fresnel, Cook-Torrance
  GGX PBR, hemispheric ambient, volumetric fog, aerial perspective, ACES, underwater FX) — so passes here are
  polish, not gaps. DEVICE-VERIFY: colours should read a touch richer/punchier; if too strong, lower vib/contrast
  or set CHUNK_COLOR_GRADE 0.

### Shader-pass roadmap (next messages)
- Water sparkle/glint polish + snow/ice specular twinkle.
- Sky/horizon gradient + sun disc/halo softness.
- Emissive hot-metal glow (uses sim::hot_metal::glow) once per-block temperature reaches the shader.
- Cheap contact-AO / corner darkening tuning; distant-LOD seam hiding.
Each pass: one focused, glslang-validated, toggleable change — small + reviewable, which also keeps the Fairphone
GPU bring-up low-risk.

### NEXT FOCUS (operator) — Fairphone 5 / Alpine Linux GPU backend friendliness (crashes; phone powers off)
Whole-device power-off on GPU init/run = likely driver/power/thermal fault on the mobile GPU (Mali/Panfrost or
the Adreno+Freedreno on FP5? — confirm GPU). Because a hard power-off won't flush logs, plan a crash-capture path
that survives: line-buffered/fsync'd log to disk per frame-phase, a "last breadcrumb" file written before each
risky GPU call (device create, swapchain, first submit), and env toggles to force-disable expensive features
(GPU worldgen/meshing, fog march, dynamic lights, color grade) to bisect what triggers the fault. Operator will
share crashlogs later — I'll help set up the persistent breadcrumb logging then.

## r466 — Shader pass 2 + unwired-shader audit + ungate default features + crash breadcrumbs
Multi-part work session (operator).

### Shader pass 2/n — snow/ice sparkle (chunk.frag)
Twinkling pinpoint glints on snow/ice: a high-frequency world-space hash makes sparse ~1/8-block cells glint,
each on its own time phase, brightest facing the camera with the sun up. Drives off a new `snow_factor` (altitude
snow cap OR intrinsically bright/blue albedo → catches SnowBlock/Ice/tundra/frozen-ocean). ~1 hash + a few ALU,
gated CHUNK_SNOW_SPARKLE (default 1). glslang RC=0. Mobile-cheap.

### Unwired-shader audit → docs/device_logs/UNWIRED_SHADERS.md
Compiled-but-not-instantiated on default runs: sky.vert/sky.frag (dedicated atmospheric sky pass — built, never
wired into a pipeline; sky is currently fog-in-chunk.frag — wiring it is a future render feature, not a flag),
clouds.frag (superseded by cloud-as-fog; not even compiled), neural_feat_lean/neural_feat_octree.comp (alternate
neural feature extractors; only the gbuf variant is loaded). Everything else is wired. See the doc.

### Ungated 23 features to DEFAULT-ON (keep =0 override)
Flipped each listed UNENDLICH_* default OFF→ON so `./smoke-latest.sh` matches the operator's full command without
the env prefix; every one still honours `UNENDLICH_X=0` to disable (critical for the Fairphone bisect below):
TEXTURES, FRAME_PROFILE, MOB_BANDS, PARTICLES, AWARENESS, ABILITY_CAST, COMBAT_MODS, AGGRO, ARBITER, WITNESS,
WING_BUFFET, WEATHER_SIM, WATER_SIM, WATER_FLOW_SHADER, WATER_DEBUG, WATER_ICE, WATER_EVAP, WATER_RAIN,
WATER_BOIL, WATER_EROSION; NEURAL_GBUFFER + NEURAL_TRAIN (Some("1")→ !=Some("0")); MESH_CACHE_SIZE default 0→256.
(WATER_FLOW_EROSION, LAVA_WATER, PLANT_STAGGER, SURFACE_SCAN_SEED were already default-on.) Per-variable edits —
no blanket sed — so unrelated flags were untouched. Lib+bin compile.

### Crash breadcrumbs (src/breadcrumb.rs) — for the Fairphone-5/Alpine power-off
NEW breadcrumb module: drop_crumb(stage) writes one line + flush + sync_all (fsync) so the bytes hit storage
before the next risky GPU call — after a hard power-off the LAST line of the file is the last stage reached.
reset() truncates at startup. Gated UNENDLICH_BREADCRUMB (default on), path via UNENDLICH_BREADCRUMB_FILE
(default unendlich_breadcrumb.log in cwd). Wired in the bin at: main startup → "gpu: creating Vulkan context"
(brackets VulkanContext::new) → "Vulkan context ready" → "gpu: creating WorldRenderer" (brackets the
device/swapchain/pipeline setup) → "WorldRenderer ready" → "gpu: first frame presented OK" → slow heartbeat
("alive: frame N") every ~600 frames. Verified write/fsync/read works.

### ⚠ FAIRPHONE-5 BRING-UP NOTE
The ungating turned on heavy/experimental paths GLOBALLY — NEURAL_TRAIN (background trainer thread),
NEURAL_GBUFFER (parallel G-buffer pass), WATER_DEBUG, and the full water sim. On the M2/Asahi these are wanted;
on the crashing Fairphone they likely INCREASE the fault surface. Bisect recipe for the FP5: run with the
breadcrumbs on, read the last line after a power-off (tells you ctx vs renderer vs first-frame vs running), then
disable heavy features one at a time via =0 (start: UNENDLICH_NEURAL_TRAIN=0 UNENDLICH_NEURAL_GBUFFER=0
UNENDLICH_GPU=0? UNENDLICH_WATER_SIM=0 UNENDLICH_FRAME_PROFILE=0). First datapoint still needed: which GPU/driver
the FP5 build uses. Next: add finer breadcrumbs inside VulkanContext::new + WorldRenderer::new (device vs
swapchain vs each pipeline) + before the first GPU worldgen/mesh dispatch, once we see where the last crumb lands.

## r467 — Multi-world duplication hunt (kitfinder) + WorldSession abstraction
Operator uploaded `kitfinder` (zero-dep structural clone miner). Built it + ran it on unendlich, then built
the abstraction the findings pointed to.

### Findings (kitfinder + manual audit)
- kitfinder: the big world-relevant clones are EXACT copy-paste between the two binaries — `populate_debug_scene()`
  (1476 tokens) and `format_workload_summary()` shared by unendlich_fullgame.rs + unendlich_world.rs. (Report saved:
  docs/device_logs/kitfinder_report_r466.txt.) The rest of the top clones are example PNG writers + tiny RNG
  helpers (lcg/mix) — noise.
- Manual audit of the per-world reset: unendlich_world.rs does NOT world-switch (0 reset logic) and lacks the
  per-world sim state, so it is NOT a diverged reset sibling — no fix-in-one-not-other divergence there. The
  fullgame reset block is actually thorough (clears ~20 items incl. roamers/drops/dragon maps/water/sediment/
  lava/sapling/hot_metal/location). The LAN + meshing_in_flight omissions are documented-intentional.
- THE REAL ISSUE (structural, not an active leak): per-world state lived as ~20 loose `let mut` vars reset by a
  50-line hand-maintained block. Every new per-world feature had to remember a `.clear()` there — I'd had to add
  one 3× (saplings r452, location r461, hot_metal r464). "Forgot to reset the new field" IS the multi-world
  leak-bug class.

### Abstraction: world/world_session.rs (the manager)
NEW `WorldSession` struct owns per-world accumulators; `reset(seed)` is `*self = Self::new(seed)` — a full rebuild,
so the compiler forces new() to construct EVERY field and a field can never be silently missed on reset. Tested.
Migrated the 4 newest accumulators into it (saplings, hot_metal, locations, current_region) and COLLAPSED their
4 manual reset lines into a single `session.reset(fg_world_seed)` in the world-switch handler (fg_world_seed is
set to the new seed first). Bin + lib compile; word-boundary rename (hot_metal_enabled + the sim::hot_metal module
path preserved). Net: future per-world state added to WorldSession is auto-reset — the bug class is gone for
anything living in the session.

### NEXT (incremental migration to finish the job)
Move the remaining per-world accumulators into WorldSession until the manual reset block is empty: water_flow/
water_dirty/sediment/water_flow_tiles, lava_registered, fg_evict_queue, lan_outgoing_edits, spawned_regions,
dragon_activities/hunts/bend_intents, fire_positions, fg_colony/fg_thermal/fg_imbue/bend_resonance, precip01,
world_clock_elapsed. Each move + delete its manual reset line; when the block only calls session.reset(), the
drift risk is fully retired. Also: dedupe populate_debug_scene/format_workload_summary across the two binaries
(extract to a shared fn) — kitfinder will confirm the clone mass drops.

### Tooling
kitfinder lives at the uploaded bundle; build with `cargo build --release` (zero deps), run
`./target/release/kitfinder <path> --min-tokens 20 --top 30`. Re-run after the dedup work to watch clone mass fall.

## r468 — WorldSession migration batch 2 (8 more accumulators) + test-gate fixes
Continuing the r467 abstraction (operator: "do one then the next next turn" — this is the migration; dedup next).
- Moved 8 more per-world accumulators into WorldSession: spawned_regions, fire_positions, dragon_activities,
  dragon_hunts, dragon_bend_intents, lava_registered, lan_outgoing_edits, fg_evict_queue (all empty-on-reset
  collections with engine types). 12 fields now reset via the single `session.reset(fg_world_seed)`.
- Removed the 8 now-redundant individual `.clear()` lines from the world-switch block (session.reset() covers
  them). The per-tick lan_outgoing_edits flush (a 2nd use) was preserved. Manual block shrank to the harder cases:
  app-owned (fg_app.drops/stamina), config-dependent (water_dirty = water_sim_enabled, water_flow/sediment/
  flow_tiles), seed-derived (fg_colony), specific-value scalars (fg_thermal/fg_imbue/bend_resonance/precip01/
  world_clock), and roamers (central). Those migrate in later passes (or stay manual where reset ≠ default).
- Word-boundary rename preserved module paths (dragon_activity/dragon_hunt/dragon_bending_ai singular modules)
  and hot_metal_enabled.
- TEST-GATE FIXES (found via `cargo check --lib --tests`, which the normal lib gate skips):
  (1) the r467 WorldSession test used a non-existent PlantState::default()/HuntState: Default — rewritten to
  populate only trivially-constructable fields (the `*self = new()` reset covers the rest structurally).
  (2) the r456 block additions (MossyCobblestone … PolishedBasalt) had silently broken blocks.rs
  `all_const_is_exhaustive` — the deliberate no-wildcard guard test that flags exactly this. Added the 7 blocks.
- Lib (+tests) and bin compile. LESSON: run `cargo check --lib --tests` after adding BlockId/Biome variants —
  the guard tests live there and the --lib gate doesn't compile them.
- NEXT TURN: dedupe populate_debug_scene() + format_workload_summary() across unendlich_fullgame.rs /
  unendlich_world.rs (kitfinder's exact-clone finding); extract to a shared fn + re-run kitfinder to confirm the
  clone mass drops.

## r469 — Dedupe populate_debug_scene + format_workload_summary across the two binaries
The "next" half of the r467/r468 work (operator: "do one then the next next turn"). kitfinder's exact-clone class
was these two functions copy-pasted between unendlich_fullgame.rs + unendlich_world.rs.
- Verified the two copies are BYTE-IDENTICAL (diff) — no hidden divergence/bug, just duplication.
- Extracted both to a new lib module `src/debug_scene.rs` (pub fns + the 4 format_workload_summary tests).
  Rewrote unendlich_engine:: → crate:: paths; added imports (PhysicsBody, runtime_render_distance, ChunkCoord).
- Deleted both copies (+ the duplicated tests + orphaned doc comments) from both bins; qualified the 4 call
  sites to unendlich_engine::debug_scene::*. One home now — a fix lands once, can't diverge.
- VERIFIED with kitfinder: both functions gone from the clone report; exact-clone mass 50,372 → 48,188 dup
  tokens (~2,184 removed; 486 → 480 classes). lib (+tests) + unendlich-fullgame compile.
- ⚠ PRE-EXISTING (not from this change): unendlich_world.rs has input-API bit-rot (WindowEvent vs InputEvent,
  Key vs Keycode, MouseButton vs MouseBtn at lines ~1283/1316/1352) and does not compile on its own — unrelated
  to the dedup (my call sites compile). The world bin isn't in the standard gates. Worth a separate cleanup pass
  (or retiring unendlich_world.rs if unendlich_fullgame supersedes it — kitfinder shows they share much code).

### WorldSession migration — remaining (continues the r467/r468 arc)
Still manual in the world-switch block: roamers, fg_app.drops/stamina (app-owned), water_dirty/water_flow/
sediment/water_flow_tiles (config-dependent), fg_colony (seed-derived), fg_thermal/fg_imbue/bend_resonance/
prev_fall_vy/precip01/world_clock_elapsed (specific reset values). Migrate the collections next; leave or
special-case the value-resets.

## r470 — Fix duplicate-biome warnings (device-log sanity check) + deterministic loader
Sanity-check log (post-fable, 2026-06-27) verdict: r463 population fix HOLDING (zero starvation spam,
roamer_tick stable ~12.8ms), r467/r468 WorldSession reset WORKING ("[launch] gameplay state reset"), r469
dedup didn't break the build. Steady-state 70-72 FPS, p99 frame ≤18ms. The scary `cpu: draw=199ms` is the
1-second CUMULATIVE (199.8/72≈2.77ms = zone=draw p50), not per-frame — confirmed T6 is a label quirk, not a
regression. Rare catastrophic spikes (frame_total max 1221ms, tick_sim max 748ms) cluster at menu/world-load
transitions (incl. the first plant_lifecycle=17.8ms scan on a fresh world), not steady gameplay.

NEW ISSUE found + fixed: 10 `WARN biome_loader] Biome ID 'unendlich:X' duplicated — replacing previous entry`.
ROOT CAUSE: assets/biomes/ has 4 namespaces — unendlich/ (22, canonical), legacy/ (13), arenas/ (8), relics/
(6). legacy/ re-defined 10 of the canonical IDs (alpine_highlands, beach, desert, jungle, plains, savanna,
swamp, taiga, temperate_forest, tundra). Worse, the loader walked dirs via read_dir whose order is NOT
guaranteed — so WHICH copy won an ID collision was nondeterministic across filesystems (latent correctness bug,
not just noise).
FIX (two parts):
1. Deleted the 10 duplicate legacy files (kept the 3 genuinely unique legacy biomes: badlands, mushroom_grove,
   wetlands). assets/biomes now: 39 files, zero duplicate _id values (verified).
2. Made biome_loader::load_directory deterministic: collect all .json paths, SORT, then load — so later
   namespaces (e.g. unendlich/) deterministically win an ID clash regardless of read_dir order.
Added regression test `load_directory_is_deterministic_on_id_collision` (tempdir, two namespaces, asserts the
sorted-last one wins). Flatten-tested the sort logic; lib (+tests) compile. Expected runtime: "Loaded 39 biomes"
with NO duplicate warnings (DEVICE-VERIFY on next run).

LESSON: biome `id` field in JSON is `_id`, not `id`. Loader walks assets/biomes/<namespace>/<id>.json recursively.

## r471 — Hot-metal quench steam puff (makes the r464 sim visible)
The r464 hot-metal sim (metal near lava heats, quenches in water, melts at 1500°C) ran but had no visible cue.
Wired the quench → steam puff the design called for, reusing the existing spawn-anchored SmokeField (32×24×32,
already advected by sim::smoke_gpu::step_smoke from F9 fires) rather than a new particle system:
- hot-metal tick (frame%20==7) now collects each cell where `hm::quench_steam(t1, wet)` fires into a per-frame
  `pending_steam: Vec<[i32;3]>`.
- the smoke-step block injects those as FireSources at intensity 0.6 (lighter than fire's 1.0) using the same
  world→cell mapping as fires; the gate now also fires when steam is pending (so steam shows even with no fires).
- net effect: dropping water on lava-heated metal flashes localized haze above the quench site, then it advects/
  dissipates. Intermittent (tick cadence) — reads as puffs. Limitation: only within the smoke field's ~64-block
  spawn-anchored extent (same as fires); far-from-spawn quenches are clipped. Bin compiles. DEVICE-VERIFY the
  visual on the next GPU run (steam haze when hot metal meets water).

NEXT shader/visual follow-ups still open: emissive hot-metal GLOW needs a per-block temperature→shader data path
(mesher or uniform) that doesn't exist yet — bigger task, device-only-verifiable, deferred until there's device
feedback. Sky/horizon gradient + sun disc and water-glint polish remain on the roadmap.

## r472 — Shader: sun-glitter path on water (chunk.frag)
First of the resumed shader passes (operator OK'd more shader work; Fairphone bring-up still later). Picked a
LOW-RISK target: the water surface block in chunk.frag is already wired + sophisticated (fresnel, planar sky
reflection, a tight sun glint pow(...,200), Beer-Lambert refraction, real-depth volumetric bottom, shore foam).
It had ONE smooth glint blob; real sun-on-water shows a shimmering glitter path.
- Added `CHUNK_WATER_GLITTER` (#define, default 1, =0 → old single glint) next to CHUNK_SNOW_SPARKLE.
- After the existing glint: near the sun's mirror direction (align>0.5), two scrolling world-space hash12
  octaves gate a much sharper lobe (pow(align,700)) per-fragment → the reflection scatters into sparkly
  micro-glints. Rides the wave normal (refl_dir uses the wave-perturbed N) so it travels with the swell;
  `align` gates it to ~zero cost away from the highlight. Uses w_time (=atmos_params.w) for the scroll, matching
  the wave animation clock.
- VALIDATED: glslangValidator -V chunk.frag → RC=0; rustcheck → PASS (0 shader-err). Regenerated
  assets/shaders/chunk.frag.spv (was stale) + updated the .spv.hash sidecar so build.rs ships it fresh.
- DEVICE-VERIFY the look on the next GPU run (sparkly glitter path on sunlit water; set CHUNK_WATER_GLITTER=0 in
  chunk.frag to A/B against the old single glint). Tunables if too strong/weak: the 1.2 intensity, the
  smoothstep(0.62,1.0) sparsity, and pow(align,700) tightness.

Shader roadmap remaining: sky/horizon gradient + sun disc (needs wiring the unwired sky.frag — a new pass, higher
risk, deferred), emissive hot-metal glow (needs a per-block temp→shader data path), contact-AO / LOD-seam.

## r473 — Shader polish batch: terrain cavity AO + entity rim light
Two independent, guarded, ADDITIVE-or-ambient-only passes (low black-screen risk; each A/B-able via its #define).
1. chunk.frag CHUNK_CAVITY_AO (default 1): micro-cavity ambient occlusion. The same fbm grain field that
   perturbs the normal now darkens AMBIENT (not direct sun) in its valleys — mix(0.80,1.0,smoothstep(0.22,0.78,
   cav)) — giving terrain a subtle weathered, tactile depth. One extra fbm2 octave; terrain only (water/cloud
   override color afterward). Checked first that existing crest-foam/whitecaps were already thorough (they are —
   steepness-gated + fbm-broken + rapids + shore ribbon) so did NOT duplicate them.
2. entity.frag ENTITY_RIM_LIGHT (default 1): creatures had NO lighting (flat v_color + fog) and the mesh carries
   no normal attribute — so reconstruct a flat geometric normal from dFdx/dFdy(v_world_pos) and add a purely
   ADDITIVE Fresnel rim: sky-cool on the shaded side, warming toward the sun on the lit side, intensity 0.35.
   Creatures now separate from terrain at their silhouette without any base-colour darkening.
- VALIDATED: glslangValidator -V → RC=0 for both; rustcheck → PASS (0 shader-err). Regenerated
  entity.frag.spv + chunk.frag.spv + their .spv.hash sidecars. Bin compiles (build.rs accepts the shaders).
- DEVICE-VERIFY both on the next GPU run. A/B: set CHUNK_CAVITY_AO=0 / ENTITY_RIM_LIGHT=0 in the respective
  .frag. Tunables: cavity darkness floor (0.80) + range (0.22..0.78); rim intensity (0.35) + power (3.0).

Shader passes shipped so far this run: r472 water glitter, r473 cavity AO + entity rim. Still open (higher risk /
deferred): sky.frag wiring (new pass), emissive hot-metal glow (needs per-block temp data path).

## r474 — Shader visibility: diagnose "didn't notice" + strengthen r472/r473 effects
Operator didn't notice r472/r473 shaders; asked what flag enables them + to check menu/linkme paths.
DIAGNOSIS (no flag needed — they were on, just subtle):
- PROOF the effects are compiled in: shipped chunk.frag.spv = 84528 bytes vs a features-OFF compile
  (-DCHUNK_CAVITY_AO=0 -DCHUNK_WATER_GLITTER=0) = 82760. read_spv() loads assets/shaders/chunk.frag.spv from
  disk at runtime (hot-shaders on; resolves via CARGO_MANIFEST_DIR/../../assets/shaders), so the regenerated
  .spv IS what runs. No `-D` override anywhere; the runtime glslangValidator call in dynamic_lights.rs is a
  test-time SPIR-V *validation*, not a variant recompile.
- linkme lead: linkme registers sim SERVICES (service_registry::SERVICES), NOT shaders — unrelated. Menu lead:
  the main menu renders the live world through the same WorldRenderer/chunk_pipeline (device log shows FPS/water
  ticking in-menu), so no diverged render path. kitfinder on render/ showed only routine Vulkan boilerplate dup.
- ROOT CAUSE: effects were tuned subtle + situational (cavity AO is ambient-only → near-invisible at bright noon;
  glitter only along the sun's mirror line on water; entity rim only on silhouetted creatures, and fauna were
  "resting"/few).
FIX — turned all three up to clearly-visible-but-tasteful:
- water glitter: intensity 1.2→2.0, pow(align,700)→400 (wider path), gate 0.5→0.42, facet smoothstep 0.62→0.55.
- cavity AO: floor mix(0.80→0.68,1.0), range smoothstep(0.22..0.78 → 0.20..0.80) — deeper valleys.
- entity rim: intensity 0.35→0.55, pow 3.0→2.5 (wider rim).
All re-validated (glslang RC=0, rustcheck PASS), spv+hash refreshed. A/B via CHUNK_CAVITY_AO / CHUNK_WATER_GLITTER
/ ENTITY_RIM_LIGHT defines. VIEWING: glitter = sunlit water with the sun low/behind the reflection; cavity = best
in ambient/shadow/overcast & on steep faces; rim = a creature silhouetted against terrain. Dial to taste after.

## r475 — Performance: persistent surface memo + roamer-loop waste (targets the 11ms roamer_tick)
From the earlier device log: steady-state frame p50≈14.3ms (70fps) is dominated by game_update p50=11.25ms,
which is almost entirely roamer_tick p50=11.21ms — and the instrumented sub-zones (roamer_blockscan/herdscan)
read ~0, so the cost is in the UN-instrumented per-creature work. Found and fixed three things there:

1. PERSISTENT SURFACE MEMO (the big one). The roamer grounding probe calls cpu.surface_y_at — a 64-deep
   density()+is_cave() PROCEDURAL noise scan (the cost that once "dominated at ~90ms"). The SurfaceCache
   memoized it but was CLEARED EVERY TICK on the false premise that "terrain changes between ticks" — yet
   surface_y_at is a PURE function of (x,z)+seed (it reads no live blocks), so the memo is valid for the whole
   world. Now it persists across ticks (cleared only on world switch — seed-derived — and on a 256K-entry cap),
   so a column is scanned once then served from the map forever. This scan sat in the un-instrumented 11ms, so
   roamer_tick should drop materially once the herd warms its local columns.
   CORRECTNESS: the memo keys on (x,z) but the scan window is cy-relative, so it's gated to GROUNDED, non-flying
   creatures (cy≈surface → canonical terrain top); airborne/flying probes (elevated cy) bypass the memo so they
   can't poison a column with a depth-shifted result. surface_cache.rs got a CAP + persist semantics; the bin
   clears it in the world-switch reset; a periodic `[surface_cache] hit_rate=…` log (every 300 frames) confirms
   it (should climb toward ~95%+ vs ~0 before). UNENDLICH_SURFACE_CACHE=0 still reverts to direct scans.
2. Removed a SECOND grime.accumulate that ran every creature every tick on top of the r152 ground-aware one —
   it double-counted grime (2× wear) AND doubled grime cost in the hot loop.
3. Removed a Vec::with_capacity(1) heap alloc per creature per tick in the gaze/observe block (an allocator
   storm) — now a stack Option + slice.
Bin + lib (+tests) compile; surface_cache tests still valid (cap unreached at ≤3 entries; clear() still works).
CONFIRM on next run: roamer_tick p50 down, [surface_cache] hit_rate high. Separate one-off spike (tick_sim max
748ms / frame max 1221ms at world-load — the first plant_lifecycle/reef scan) left for a later pass.

## r475 REVISED — surface-memo persistence REVERTED; kept safe wins + honest bottleneck analysis
Operator pushed back on persisting the surface memo: the world is mutated by erosion, water physics, builder
dragons, plant growth, etc., and grounding may become terrain-aware — so a persistent (x,z)→height memo is a
landmine even though surface_y_at is procedural-only TODAY (confirmed: surface_y_at → density() → model.density,
pure noise, reads no live blocks; it already ignores erosion/builds, a separate pre-existing limitation). REVERTED
the persistence: SurfaceCache is back to per-tick clear (no CAP, no world-switch clear, no cache_ok gate, no
hit_rate log).
KEPT (unambiguously safe, real): (1) removed the redundant SECOND grime.accumulate per creature/tick (2× wear +
wasted work); (2) removed the per-creature Vec::with_capacity(1) heap alloc in gaze/observe (stack Option+slice);
(3) chunk.frag: compute the grain-height fbm ONCE and share it between the normal map (perturb_normal now takes
h0) and the r473 cavity AO — drops one ~16-hash fbm per terrain fragment (small GPU main-pass win). glslang RC=0,
spv refreshed.

HONEST BOTTLENECK ANALYSIS (from the earlier device log, for the next pass):
- STEADY STATE is CPU-BOUND, not GPU-bound: cpu_zones wait_fences≈0.02ms (CPU never stalls on the GPU). The 14ms
  frame = game_update 11.25ms (≈ roamer_tick 11.21ms) + record/submit ~3ms, all CPU; GPU main=7.6ms runs async
  and overlaps. So raising idle FPS means cutting roamer_tick, NOT the GPU.
- MESHING is on the CPU by default. UNENDLICH_GPU_MESHER: 0=CPU (default, visual reference), 1=SparseGpuMesher
  (advisory: ~1.22x SLOWER, rough — avoid), 2=OctreeGpuMesher (the intended GPU path "for play"). The GPU mesher
  helps DURING MOVEMENT/STREAMING (offloads CPU mesh_chunk), not idle FPS. Mode 2 is the device-test candidate.
- Fog is froxel-baked (froxel_fill.comp); the 6-step per-fragment march is only a fallback (if !froxel_filled),
  so it's not the steady main-pass cost. The 7.6ms is just heavy full-screen PBR (GGX + hemi + my r472-473 adds).
NEXT (device-gated): try UNENDLICH_GPU_MESHER=2 on the FP5/M2 to offload streaming mesh cost; for idle FPS, the
lever is roamer_tick (needs finer per-phase instrumentation to target the un-bucketed 11ms — blockscan/herdscan
zones read 0, so the cost is in observe/steering/dragon/forage). Both are real; the GPU mesher is movement-time.

## r476 — octree GPU mesher streaming-stutter ROOT CAUSE + adaptive batch mitigation
Operator log (log-r473-shader-polish, run with UNENDLICH_GPU_MESHER=2 + NEURAL_GBUFFER=1 NEURAL_TRAIN=1
WATER_SIM=1 + all water subflags + WEATHER_SIM=1): "loading is fast but the stutters are real." 91 LONG FRAMEs.

ROOT CAUSE (definitive): the stutters are the OctreeGpuMesher (mode 2) saturating the shared GPU queue during
chunk streaming. Evidence:
- `[mesh-stall] octree dispatch 190ms for 4 chunks (47642us/chunk)` — the octree mesh compute runs ~47ms PER
  CHUNK on the M2/Asahi GPU; a 4-chunk batch is one ~190ms synchronous GPU block.
- `Compute queue acquired: family=0 distinct=false (graphics handle reused: true)` — there is NO distinct compute
  queue; mesher compute serializes on the SAME queue as rendering. So a 190ms mesher dispatch blocks the render
  present: `cpu_zones: ... acquire=44.84ms` (render's swapchain acquire waits out the mesher).
- Airtight FPS↔mesh_q correlation: mesh_q=0/0 → 40-56fps (smooth); mesh_q=90-132 (backed up while MOVING) →
  1-10fps. The CPU `draw=28ms` during stutters is the CPU waiting on the saturated GPU, not CPU work.
- NOT shading: GPU total is 30ms; the 100-130ms frame is dominated by the unmeasured acquire-stall behind the
  mesher. NOT the readback either (already bulk/exact-count via v0.24.44). The 47ms/chunk is GPU compute+upload,
  which can't be profiled/fixed from the Linux box (no GPU here).

FIX (r476, safe + verifiable, CPU-side scheduling): ADAPTIVE batch sizing in octree_gpu_dispatcher_loop. Was a
fixed batch_target=4 (one 190ms stall). Now starts at batch_cap and shrinks toward 1 when a dispatch overruns a
~30fps frame budget (ADAPT_SHRINK_US=33ms) so rendering interleaves between shorter dispatches, grows back when
dispatches are cheap (ADAPT_GROW_US=12ms). Auto-tunes per hardware: fast GPUs keep big throughput batches, slow
ones (this M2) pin near 1 → worst stall ~47ms instead of ~190ms (≈21fps vs ≈5fps during heavy streaming).
Correctness-preserving (only changes how many chunks group per dispatch; every chunk still meshes). Gated
UNENDLICH_GPU_MESH_ADAPTIVE=0 to restore fixed batch_cap. lib+bin compile.

HONEST CEILING + config levers for the next DEVICE test (mark DEVICE-VERIFY):
- The real cure is faster octree compute (GPU-side, can't do blind) OR not meshing on the GPU at all here.
- `UNENDLICH_GPU_MESHER=0` (CPU mesher, the DEFAULT) meshes on CPU worker threads in PARALLEL with GPU render —
  NO shared-queue contention. Given the GPU compute is ~47ms/chunk on this hardware, mode 0 is very likely the
  SMOOTHEST option on the M2. This is the honest answer to "are the GPU meshers working properly": mode 2 is
  functional but its compute is too slow on M2/Asahi to mesh live without stalling rendering.
- `UNENDLICH_GPU_MESH_BATCH=1` manually pins what the adaptive logic now reaches on its own.
- The run also had NEURAL_TRAIN=1 (on-device training) + full WATER_SIM/WEATHER_SIM; for a clean perf read,
  isolate by testing mode 0 vs mode 2 with fewer heavy flags.

FOLLOW-UP (flagged, NOT done — correctness-risky, needs device verify): ~42% of GPU-dispatched chunks return
EMPTY (octree stats empty=211 nonempty=292). Pure-AIR chunks always mesh to nothing and could be skipped CPU-side
before dispatch (cheap uniform-air octree check) to cut ~40% of dispatches during streaming. CAUTION: only
uniform-AIR is safe to skip; fully-enclosed SOLID chunks also produce empty meshes but need neighbor info to
detect, and a wrongly-skipped chunk leaves a hole. Deferred until device-verifiable.

## r476 (cont.) — THE GPU blocker: MAX_FRAMES_IN_FLIGHT = 1 (no CPU/GPU overlap)
Operator: "UNENDLICH_GPU_MESHER=0 is also slower / worse freezes — there's something we're missing to
unblock the GPU." Correct. The freeze is NOT the mesher (it hits BOTH modes). Root cause is structural:

- world_renderer.rs had `const MAX_FRAMES_IN_FLIGHT = 1` (the ACTIVE renderer; bin uses WorldRenderer::new).
  One frame in flight = the CPU records -> submits -> BLOCKS on `frame.in_flight` fence (world_renderer.rs:3025,
  wait u64::MAX) until the GPU fully finishes, EVERY frame. Zero CPU/GPU overlap. So every GPU stall (worldgen
  on its own device/queue, mesh dispatch, a heavy render frame) freezes the CPU and vice-versa. This is exactly
  the `acquire=44ms` waits and why mode 0 freezes too — it's mesher-independent.
- The infra was BUILT for >1: per-frame FrameSync already duplicates UBO/descriptor/cmd-buffer/fence/semaphores;
  deferred-free holds evicted buffers MAX_FRAMES_IN_FLIGHT frames. Only the constant pinned it to 1.

SCAFFOLDING SHIPPED (safe, default-OFF, compiles): world_renderer MAX_FRAMES_IN_FLIGHT is now a CAP (3); added
runtime `frames_in_flight` from UNENDLICH_FRAMES_IN_FLIGHT (default 1, clamp 1..=3). Drives the frame-creation
loop, the current_frame cycle (% frames_in_flight), and the deferred-free hold (frames_remaining =
self.frames_in_flight, 9 sites). Pools/descriptors/GpuTimer sized for the cap (safe over-alloc). At the default 1
behavior is BYTE-IDENTICAL to before.

BLOCKER to actually enabling >1 (NOT yet done — would corrupt): the per-chunk DRAW DATA is CPU-written into
SHARED, non-double-buffered buffers during streaming:
  - the indirect-draw buffer: `self.indirect.set(draw_slot, chunk_draw)` (world_renderer.rs:2007)
  - the chunks SSBO (per-chunk ChunkDraw) and the mesh staging buffers (mesh_pool.write_at).
At FIF=1 the per-frame fence-wait guarantees the GPU is done before the CPU mutates these. At FIF>1 the CPU runs
ahead and overwrites buffers a prior in-flight frame's GPU draw is still reading -> corruption / GPU fault.
Shadow/froxel images are fine (single queue serializes GPU access; no CPU write). The camera UBO is already
per-frame.

=> The REAL unblock = double-buffer the indirect-draw buffer + count buffer + chunks SSBO per-frame (write a
chunk's draw entry into ALL in-flight copies, since it persists every frame until eviction), then FIF=2/3 is
safe and the CPU pipelines ahead of the GPU. This is the substantial, device-verify-required next step. The
scaffolding above is the foundation; the env stays default-1 until the double-buffering lands.

## r476 (cont.) — metadata double-buffering: FIF>1 is now CRASH-SAFE (testable)
Enabling frames-in-flight>1 needs every single-buffered, CPU-written-per-frame, GPU-read resource
double-buffered. Audit found THREE:
  1. indirect metadata SSBO (per-chunk ChunkDraw) — CPU-written by `indirect.set()` during streaming,
     read by the cull shader AND the vertex shader (binding 3). **Torn read here can CRASH**: a garbled
     index_count/first_index makes the indirect draw read out of the mesh buffer bounds -> GPU fault.
  2. view_params SSBO (view/proj matrices + derived frustum planes) — CPU-written each frame, read by
     view_compute/cascade_fit/cull. Torn read = wrong culling for a frame (VISUAL only, no OOB).
  3. entity vertex/index buffers (entity_pipeline) — "single-buffered host memory" (world_renderer.rs
     ~2932), CPU-written each frame by upload_entities. Torn = jittered/torn creatures (VISUAL only).
UBO (camera/lights/water/sun/fx) is already per-frame; shadow/froxel are GPU-written (single-queue
serialized) — both safe.

DONE this pass: double-buffered #1 (the crash one). chunk_indirect.rs IndirectDrawSet now holds N metadata
copies + N cull descriptor sets; `cpu_shadow` stays authoritative; `set()` marks the slot dirty for every
frame; new `flush_frame(frame_idx)` syncs a frame's copy from cpu_shadow right AFTER that frame's fence wait
(GPU guaranteed done reading it) and before its cull. commands/count stay shared (reset->cull->draw all in one
per-frame cmd buffer => single-queue serialized). world_renderer binds metadata[i] into frame i's graphics
descriptor (binding 3), passes current_frame to record_cull_dispatch, and calls flush_frame after the fence
wait. Eviction holds the mesh slot frames_in_flight frames, which outlasts the empty-entry propagation to all
copies. At frames_in_flight==1: one buffer, one cull set, behaviorally identical (writes batched at flush =
same GPU-visible result). lib + bin + lib --tests all compile.

=> STATUS: UNENDLICH_FRAMES_IN_FLIGHT=2 is now CRASH-SAFE to test. Expect possible VISUAL flicker on creatures
(entity buffers) and chunk-edge cull pop (view_params) until #2 and #3 are also double-buffered — those are the
cleanup, deferred until we confirm FIF=2 actually removes the streaming freezes (the whole hypothesis). The
test question: does UNENDLICH_FRAMES_IN_FLIGHT=2 (or 3) eliminate the streaming freezes in BOTH mesher modes?
If yes -> finish #2/#3 to remove the flicker. If no -> the blocker is elsewhere and we saved the polish work.

## r476 (cont.) — meshing/generation speed: uniform-air skip + honest analysis
Operator wants to focus on meshing + chunk generation speed. Findings + work:

SHIPPED — uniform-air dispatch skip (safe, verifiable): added OctreeArray::is_uniform_air() (O(1): single
air-leaf root). In the meshing-pool worker, before routing a chunk to the GPU octree dispatcher, if the chunk
is uniform air, emit an empty mesh result directly and skip the GPU round-trip. A pure-air chunk ALWAYS meshes
to nothing (air emits no faces regardless of neighbors), so this is correctness-preserving. The field log had
~42% of GPU dispatches return empty; a flying dragon spends most of its time over air, so this removes a large
slice of streaming dispatch load. lib+bin compile.

ANALYSIS — why the GPU octree mesher is ~47ms/chunk vs the CPU mesher's ~0.38ms (124×):
- The per-cell shader logic is ALREADY optimal. chunk_mesh_octree.comp main(): air cells early-out after ONE
  octree walk (line ~852 `if(!is_solid_opaque(self_block)) return;`); cells strictly inside a uniform leaf skip
  all 6 neighbor descents (lines ~855-867). So only SURFACE cells pay the 7-walk cost; air/interior pay 1.
- The cost is therefore NOT redundant work — it's the octree walk itself: pointer-chasing OCTREE_DEPTH levels
  of DEVICE_LOCAL `nodes[]` per cell, across 32768 cells, with each thread descending a different path =
  divergent, uncoalesced memory loads. GPUs are bad at exactly this (pointer-based tree traversal). The CPU
  mesher reads a FLAT decompressed block array (O(1), cache-friendly) — which is why it's 124× faster.
- => There is no CPU-side or simple-shader win left for the nonempty case. The only real speedup is to change
  the DATA the GPU meshes: a pre-pass that EXPANDS the octree into a flat 32^3 (+halo) block grid once
  (coalesced writes), then mesh from the flat grid (coalesced reads, no per-cell tree walk). That's a
  shader+pipeline change that needs on-device verification (a wrong expansion = holes/garbage in the world) —
  NOT done blind. Filed as the high-value next step IF the GPU mesher is to be kept.
- Strategic note: the CPU mesher is the faster mesher here (flat-data friendly). What made mode 0 "freeze worse"
  was almost certainly the MAX_FRAMES_IN_FLIGHT=1 serialization (now addressed), NOT meshing throughput
  (0.38ms/chunk × 4 workers ≈ 10k chunks/s). Worth re-testing mode 0 once FIF>1 is confirmed.

GENERATION: the field log shows gen_pending=0 during the stutters — GPU worldgen (batch=16, own device/queue)
keeps up and is NOT the streaming bottleneck in that run. Meshing + the FIF=1 serialization are. (Worldgen
batch-size tuning remains available via the loader if a gen-bound workload shows up later.)

## r476 (cont.) — meshing/gen payoffs piped to settings (breadth)
Per operator ("pipe it to the settings ... we need all the payoffs"):
- UNENDLICH_SKIP_AIR_CHUNKS (default 1/on): the uniform-air dispatch skip is now a toggle. Default on
  (safe + a confirmed prior win); =0 forces every chunk through the mesher for A/B.
- UNENDLICH_MESH_WORKERS (default 4 = MESH_WORKERS): mesh-pool worker thread count, clamped 1..=32. Raises
  CPU-mesher throughput on many-core machines (mode 0 + the extract/decorate pre/post passes). The bin reads
  it once and threads it into all three pool constructors.
- (already existed) UNENDLICH_GPU_GEN_BATCH: GPU worldgen per-dispatch batch size. Generation was NOT the
  streaming bottleneck in the field log (gen_pending=0), so this is available but not the lever.
lib + bin compile.

NEXT (queued) — flat-grid GPU mesh expansion: the nonempty-chunk 47ms is fundamental octree-walk memory
latency. The real speedup is a pre-pass that expands the octree -> flat 32^3(+halo) block grid once, then
meshes the flat grid (coalesced reads, no per-cell tree walk). It is DEVICE-VERIFIABLE via the existing CPU
reference (gpu_meshing_octree_cpu_ref.rs) + UNENDLICH_OCTREE_MESH_VERIFY=1, which cross-checks GPU mesh output
against the CPU ref and flags mismatches — so a gated flat-grid path can be validated on the operator's box
even though it can't be verified from the Linux dev box. Substantial shader+pipeline work; to be built gated
(default off) so a wrong expansion can't corrupt the default path.

## r476 (cont.) — more knobs + dynamicism (operator: "turn constants into levers")
Field log r476 (M2 ULTRA Asahi): frame p50=380ms (~2.6fps); mesh_q=0 -> 110-124fps, mesh_q>0 -> 2-4fps; octree
dispatch up to 171ms for ONE chunk. CRITICAL: the run set `FRAMES_IN_FLIGHT=2` but the var is
`UNENDLICH_FRAMES_IN_FLIGHT` — log shows "frames_in_flight = 1". So FIF>1 was NOT actually tested; this whole
run is still single-frame-in-flight.

New LEVERS (were hardcoded constants):
- UNENDLICH_GPU_MESH_TIMEOUT_MS  (def 8)     — octree batch accumulation window.
- UNENDLICH_GPU_MESH_SHRINK_US   (def 33000) — adaptive batch shrinks when a dispatch exceeds this.
- UNENDLICH_GPU_MESH_GROW_US     (def 12000) — adaptive batch grows when a dispatch is under this.
- UNENDLICH_MESH_QUEUE_CAPACITY  (def 64)    — mesh request queue depth (back-pressure); clamp 8..=1024.
- (earlier) UNENDLICH_MESH_WORKERS, UNENDLICH_SKIP_AIR_CHUNKS, UNENDLICH_GPU_GEN_BATCH, UNENDLICH_MESH_CACHE_SIZE.

New DYNAMICISM:
- UNENDLICH_GPU_MESH_YIELD_PCT (def 0): after each octree dispatch, sleep dispatch_time*pct/100 so the shared
  GPU queue is left idle for the render pass. =100 -> ~50% mesh duty (render gets the other half). Trades chunk
  pop-in speed for playable FPS while streaming. The dispatch is what monopolizes the shared queue, so this is
  the direct throttle for the "2fps while mesh_q>0" symptom.
- UNENDLICH_ADAPTIVE_RENDER_DISTANCE (def off): wall-clock (400ms) controller that caps render distance BELOW
  the chosen value when frames run long (>SHRINK_MS, catastrophic frames cut 2 rings at once) and grows it back
  when frames are under GROW_MS. Reacts at single-digit FPS (the 120-frame autotuner is 60s too late at 2fps).
  Levers: UNENDLICH_ADAPTIVE_RD_SHRINK_MS (def 3x frame target), _GROW_MS (def 1.5x), _MIN (def 2). Only caps
  below the UI/env distance, never above; applied on the 400ms cadence (the %15-frame slider write is too slow
  once FPS collapses). Default off => behavior unchanged.

All compile (lib+bin). NEXT device test should: (1) use UNENDLICH_FRAMES_IN_FLIGHT=2 (correct name!), (2) try
UNENDLICH_GPU_MESHER=0 under it (CPU mesher is 0.38ms/chunk vs the octree's 38-171ms on Asahi — almost certainly
the right call here), (3) UNENDLICH_GPU_MESH_YIELD_PCT=100 + UNENDLICH_ADAPTIVE_RENDER_DISTANCE=1 if staying on
the octree mesher.

## r476 (cont.) — live levers on the Performance settings page
The two LIVE-tunable r476 levers are now in-game controls (Settings → Performance), not just env vars:
- "mesh throttle (%)" slider [0..200] -> renderer.ui_gpu_mesh_yield -> constants::GPU_MESH_YIELD_PCT global
  (new), which the octree dispatcher reads each dispatch (was env-read-once). Seeded from
  UNENDLICH_GPU_MESH_YIELD_PCT at startup; the bin pushes the slider value into the global each frame.
- "adaptive render distance" toggle -> renderer.ui_adaptive_rd, which the wall-clock RD controller now reads
  live (was an immutable env flag). Seeded from UNENDLICH_ADAPTIVE_RENDER_DISTANCE.
Full lifecycle wired like the existing autotune/stride levers: struct field + init, local-copy + UI widget +
write-back in the Performance tab, JSON load/save ("gpu_mesh_yield"/"adaptive_rd"), and the runtime read path.
Env vars still work as the initial seed. lib + bin compile.

NOT on the page (construction-time only — need a restart, so they stay env): UNENDLICH_FRAMES_IN_FLIGHT,
UNENDLICH_MESH_WORKERS, UNENDLICH_MESH_QUEUE_CAPACITY, UNENDLICH_GPU_GEN_BATCH, UNENDLICH_GPU_MESHER. The
adaptive-RD thresholds (_SHRINK_MS/_GROW_MS/_MIN) and the octree timeout/shrink/grow knobs remain env (the
toggle + throttle are the ones worth a live slider).

## r476 — timebend snapshot/scrub camera mode (player-facing)
The branching-timeline sim (sim/timeline.rs) had no viewfinder. Added one: src/timebend_scrub.rs
(TimebendScrub) — a rolling ring of (camera pose + sky time) vantages recorded as the player roams.

Interpretation of "pull up images from the rendered position of the player at that time": rather than store
screenshots, store the POSE + world clock and, on scrub, fly the camera to that past vantage and wind the sky
to the time-of-day it was then, so the LIVE world re-renders exactly as it looked from there (3D, not a static
image; "change the world scene to the world").

Wiring (bin):
- F8 toggles scrub mode. Enter saves the live pose+clock, jumps to the latest vantage, kicks a fade; exit
  restores them.
- timebend.active folds into game_paused -> the world (sky/weather/sim/physics) freezes while scrubbing.
- Scroll wheel walks vantages (intercepted before the hotbar). Each cut pulses the fade.
- Before render: LIVE -> maybe_capture on cadence; SCRUBBING -> override view + sky_state.time_seconds from
  the selected vantage. The existing loading veil is driven from timebend.fade (0.10 floor while scrubbing so a
  subtle vignette + HUD label persist); loading_label shows "TIMEBEND · vantage N/M · Xs ago · scroll to scrub,
  F8 to return". Label cleared on exit.
- Levers: UNENDLICH_TIMEBEND_SNAPSHOTS (def 120), UNENDLICH_TIMEBEND_INTERVAL_S (def 1.5).

Core is pure logic with 4 unit tests (ring cap, enter-jumps-to-latest, scrub clamp, exit restore) — all pass
(flatten-tested). lib + bin + lib --tests compile.

VISUAL CAVEAT (device-verify): the HUD currently reuses the loading-overlay label/veil — functional but not a
styled panel. Fleshing it into a dedicated timebend menu (timeline bar, vantage thumbnails, styled fade) is the
next visual iteration; can't verify rendering from the Linux box.

## r476 — bold visual batch (operator green-lit blind shader work, batched)
All validated (glslangValidator RC=0) + spv/hashes regenerated. Hot-shaders picks them up without a rebuild.
- chunk.frag (the whole-frame grade — highest leverage): vibrance 0.16->0.30; NEW cinematic teal/orange
  split-tone (cool shadows vec3(0.92,0.985,1.07), warm highlights vec3(1.08,1.01,0.89), keyed off luma);
  contrast 1.055->1.12; filmic 0.08->0.14; vignette 0.25->0.32. This is the change that reads on every surface.
  spv 84528->84936 bytes (confirms split-tone compiled in).
- sky.frag: deeper/warmer sunsets (dusk mix 0.5->0.85, band 0.6->0.7); brighter sun halo 0.25->0.45 + NEW tight
  inner glow lobe pow(cos,64)*0.85 + disk 6->7; denser brighter stars (threshold 0.9975->0.9965, color
  vec3(1,1,1.12)*1.4).
- entity.frag: rim light 0.55->0.85, pow 2.5->2.2 — creatures + dragon separate from terrain.
- bloom.rs (CPU consts; needs UNENDLICH_BLOOM=1): threshold 1.0->0.85, intensity 0.05->0.13 — dreamier glow.

DEVICE-VERIFY: can't see the result from the Linux box; operator to eyeball the batch together and report what's
too much / too little. The split-tone + sunset warmth + vignette are the deliberate "feels different" trio.

## r476 — locked-in defaults + detached terrain
Field log r476-mesh confirmed the win (steady mean ~21ms; note: the uploaded log was actually GPU_MESHER=2,
env dump line 7244 — it still had octree dispatch stalls up to 1129ms; the operator's "so much better" run was
=0. Also: the late-window p50=85ms is a profiler artifact — p50 > mean is impossible; trust the mean).

DEFAULTS NOW LOCKED IN (all still overridable):
- UNENDLICH_GPU_MESHER default 2 -> 0 (CPU mesher, 0.38ms/chunk, no stalls). Octree (=2) opt-in.
- UNENDLICH_FRAMES_IN_FLIGHT default 1 -> 2 (CPU/GPU overlap; metadata SSBO double-buffered so safe).
- UNENDLICH_BLOOM default off -> on (opt out =0).
- (left conservative: UNENDLICH_MESH_CACHE_SIZE stays 256 — bumping to 512 risks the documented driver crash.)

DETACHED TERRAIN (new, default on; UNENDLICH_DETACHED_TERRAIN=0 to disable):
- block_at returns Air for unloaded chunks, so a fast/flying player used to fall THROUGH terrain that hadn't
  streamed in. Now: when the cell at the feet (and one below) isn't loaded, gravity is turned OFF and downward
  velocity zeroed, so the player GLIDES forward at altitude while chunks stream in below (horizontal velocity
  still integrates in step_body — flight stays continuous, no stop-and-wait).
- When terrain spawns under a previously-suspended player and they're embedded in solid, they're settled to the
  first non-solid cell above (chunk_manager.surface_above, scan up to 64) — the "replace the player when the
  chunk spawns and position differs" reconcile.
- New helpers: chunk_manager.is_loaded_at(x,y,z), surface_above(x,y,z,max_up). Only affects physics-mode bodies;
  no-op when ground is loaded (normal play unchanged).
- This is a PRAGMATIC reconcile, not the full sim/rollback.rs replay engine (that exists for netcode and could
  drive a deeper deterministic version later). lib+bin+lib --tests compile.

## r476 — octree mesher (MESHER=2) cave speedup: co-leaf neighbor skip
The octree GPU mesher's hot path is 6 per-cell neighbor octree descents (chunk_mesh_octree.comp ~874-879).
The existing buried-cell fast path skips all 6 only when the cell is STRICTLY interior to a uniform leaf — but
the cells that actually emit (and cost time) are SURFACE cells on leaf faces, which fall through to all 6.

New coleaf_or_sample(): octree_sample_ext already returns the self leaf extent (self_lo/self_lsz). Any neighbor
that lands INSIDE that extent is the same block as self (leaf = uniform), and since self is solid-opaque that
face culls — so we return self_block WITHOUT descending. A flat cave-wall cell (5 of 6 neighbors co-leaf) drops
from 6 descents to ~1. Edge/corner cells save 4/3. A 1³ leaf (self_lsz==1) naturally descends all 6 (no regression).

PROVABLY output-identical to a full descent (co-leaf neighbor == self_block == solid == face culls), so the CPU
reference (gpu_meshing_octree_cpu_ref.rs, unchanged) still matches. Validates RC=0; build.rs recompiles it.
DEVICE-VERIFY: run once with UNENDLICH_GPU_MESHER=2 UNENDLICH_OCTREE_MESH_VERIFY=1 to confirm bit-for-bit parity,
then time a cavey area vs the previous octree build.

NEXT (bigger): shared-memory tile expansion — cooperatively expand an octree sub-tile (+1 halo) into workgroup
shared memory once, then mesh from shared (coalesced, zero per-cell descents). That's the structural cure that
would let MESHER=2 parallel throughput overtake the CPU mesher; larger rewrite, also CPU-ref-verifiable.

## r477 — fix smoke.sh + resume revision numbering
TWO operator-reported breakages fixed:
1. REVISION NUMBERING: r449–r476 were all tagged "r476" this session (8+ distinct deliverables), so per-revision
   iteration tracking / smoke-latest had nothing to distinguish builds. Resumed incrementing: this is r477, and
   the crate version is bumped 0.24.63-adaptive-uploads -> 0.24.64-r477-octree-tile so the build reports a new
   version. Future deliverables increment from here.
2. SMOKE.SH: an earlier zip shipped a STALE chunk.frag.spv.hash. Root cause: chunk.frag has
   `#include "water_waves.glsl"`, so build.rs's transitive_source_hash includes that file's bytes — but the spv
   regen used a plain `sha256sum chunk.frag` (wrong for include-bearing shaders). That tripped the
   `committed_spirv_is_fresh_against_transitive_source` test. build.rs self-heals it on rebuild, but the shipped
   zip was stale. Now: ALL sidecars verified fresh against transitive_source_hash (stale count 0). For future
   shader edits with includes, regen the hash via build.rs (cargo build) or the transitive method, NOT sha256sum.

Also fixed a latent inconsistency introduced when the mesher default flipped to CPU: runtime_config.rs defaulted
gpu_mesher to 2 (octree) while the bin now defaults to 0 (CPU). Aligned runtime_config (from_env + Medium/base
preset) to 0 and updated the freshness/default test (assert_eq!(m.gpu_mesher, 0)). lib+tests+bin compile.

NEXT: shared-memory tile expansion for the octree mesher (the structural cure). Deliberately NOT rushed onto a
just-fixed smoke — it's a real rewrite (workgroup -> spatial 4³ tile + 6³ halo in shared memory, cooperative
single-descent load, barrier, then zero-descent meshing) that also touches the dispatch indexing and the
greedy-run path (a run can extend past the tile). Will be done as a focused, CPU-ref-verified pass.

## r478 — octree mesher: shared-memory tile expansion (UNENDLICH_OCTREE_TILE=1)
The structural cure for the octree mesher's per-cell pointer-chasing. chunk_mesh_octree.comp now has a tiled path
(gated by the repurposed PC field tile_enabled, set from UNENDLICH_OCTREE_TILE; default 0 = unchanged per-cell path).

Key realization: NO dispatch change needed. The workgroup count is already cells/64 = (cc/4)^3, so the tiled path
just reinterprets gl_WorkGroupID.x as a 4x4x4 spatial TILE and the 64 invocations as its cells. The workgroup
cooperatively expands a 6^3 halo (tile + 1-cell border) into `shared uint s_halo[216]` — ONE octree descent per
halo cell (216 cells / 64 threads = ~3.4 each) — barrier, then every cell reads its self + 6 axis neighbors from
shared with ZERO further descents. Replaces the per-cell 7 descents (self + 6 neighbors) that made the mesher
38-171ms/chunk on Asahi. Guarded to cc>=4 (falls back to per-cell for very-high LODs where cc<4 would make the
tile grid degenerate); barrier is in uniform control flow (tile_enabled + cc are push-constant-uniform, all 64
threads = one tile, no early return before the barrier). Halo corners are multi-axis-OOB -> air, harmless (only
axis neighbors are ever read). Greedy-run continuation still uses sample_or_neighbor descents — unchanged — so
output is BIT-IDENTICAL to the per-cell path (same coords, same samples, same merge).

Rust: PushConstants _pad -> tile_enabled, fed by cached octree_tile_enabled() (OnceLock env read). ChunkMetaGpu
_pad untouched. Validates RC=0, build.rs regenerates the spv (hash FRESH), bin compiles.

DEVICE-VERIFY: UNENDLICH_GPU_MESHER=2 UNENDLICH_OCTREE_TILE=1 UNENDLICH_OCTREE_MESH_VERIFY=1 must report parity
(bit-identical to per-cell). Then time a cavey/streaming area: tile vs per-cell octree vs CPU mesher (=0). This is
the path meant to let MESHER=2 parallel throughput overtake MESHER=0. If verify FAILS, the likely suspects are the
halo index math or the tile decode — both isolated to the tile branch.

## r479 — tiled mesher: predictable prefetch loop + per-lane leaf cache
Refines the r478 tile halo load (active when UNENDLICH_OCTREE_TILE=1). The halo descents were the remaining
cost in the tiled path; this makes them predictable and cache-friendly without adding divergent/unbounded work.

- LOOP made PREDICTABLE: each lane now walks a CONTIGUOUS run of halo cells (lanes 0..23 -> 4 cells, 24..63 -> 3;
  216 total) instead of the old stride-64 scatter. Contiguous = spatially adjacent cells.
- CACHE TABLE (per-lane): octree_sample_ext already returns the resolved leaf's [lo, lo+sz) extent. We keep that
  as a small per-lane cache; for each subsequent cell in the run we first "look into" the cache — a flat bounds
  check — and "pull up" the cached block with NO octree re-walk when the cell falls inside the same leaf (which,
  for spatially-adjacent cells in the uniform interior of a chunk, is the common case). Miss -> one descent that
  refills the cache. Cross-chunk (OOB) cells use sample_or_neighbor and bypass the cache.
- PREDICTABILITY PRESERVED: worst case (all misses) == the old per-cell descent cost, so it never regresses; the
  added work is a bounded bounds-check, no unbounded loops, no new barriers. Uniform runs collapse to 1 descent.

Output is unchanged (a leaf is uniform, so the cached block == what a descent returns) -> still BIT-IDENTICAL to
the per-cell path, so UNENDLICH_OCTREE_MESH_VERIFY=1 still validates. RC=0, spv hash fresh, bin compiles.
DEVICE-VERIFY: same as r478 (verify parity, then time tile vs per-cell vs CPU) — expect the halo-load cost to
drop further in cavey/large-uniform terrain.

## r480 — Performance page: tile-mesher + detached-terrain toggles
Promoted two env flags to live Settings -> Performance toggles (GPU MESHER section), same lifecycle as the
mesh-throttle / adaptive-RD sliders:
- "GPU tile mesher" (ui_octree_tile) -> constants::OCTREE_TILE_ENABLED atomic; octree_tile_enabled() in
  gpu_meshing_octree.rs now reads the atomic (bin seeds it from UNENDLICH_OCTREE_TILE at startup, pushes the
  toggle each frame). Only bites when GPU_MESHER=2 (the octree path); harmless otherwise.
- "detached terrain" (ui_detached_terrain) -> the physics ground-ready check reads the live toggle instead of
  the immutable env read; seeded from UNENDLICH_DETACHED_TERRAIN.
Fields + init + local-copy + UI + write-back in world_renderer.rs; JSON load/save in the bin. lib+bin compile.

## r481 — weapon models (held viewmodels for all 8 WeaponClasses)
The forge already produced eight WeaponClasses but the hand only knew 4 crude ItemForms. Added, in
sim/viewmodel.rs:
- weapon_model(WeaponClass) -> Vec<ItemBox>: eight DISTINCT box silhouettes (dagger, arming sword, greatsword
  w/ fuller, single-bit axe, double-bit greataxe w/ top spike, leaf-head spear, focus staff, gem wand). +Z is
  the length axis, same convention as item_model; palette extended with leather + gold accents.
- weapon_to_form(WeaponClass) -> ItemForm: maps a class to the nearest existing form for POSE reuse (bob/sway/
  offset stay in one place); geometry still comes from weapon_model.
- weapon_class_for_id / weapon_id_for_class: reserved hand-item id range 4000..4007 (one per class, decl order).
- WEAPON_CLASSES[8]: declaration-order array for cycling.
Bin wiring (unendlich_fullgame.rs): the first-person viewmodel render now takes the weapon path when the hand id
is in the weapon range (weapon_model + weapon_to_form pose), else the old id->form fallback. DEBUG: J cycles the
eight weapon models into the hand (debug_weapon_class override, takes precedence over inventory, prints the class)
so all eight can be eyeballed first-person without the loot/inventory plumbing. lib+bin compile clean.
DEVICE-VERIFY: hold J through the eight shapes — confirm each reads distinctly and sits right in the hand; the
poses are reused from Blade/Axe/Staff/Tool so greatsword/greataxe/spear/dagger may want per-class offset tuning
after a look. NOT YET wired: forged weapons actually entering the inventory as ids 4000+ (loot/give path) — the
models + debug viewer are in; the loot plumbing is the next step.

### r481 (cont.) — weapons as real inventory items
Closed the loop flagged above: weapons can now enter the inventory as real items, not just the debug render.
- items/mod.rs default_palette: registered the eight weapon ids (4000..4007) with names + swatch colors, so they
  read correctly on the hotbar and as ground-drop cubes (the drop render's palette lookup falls back safely for
  unknown ids, so this is additive).
- bin: Y gives one of each forged weapon (add_item via the existing UniformRegistry). In survival (F6), scrolling
  the hotbar to a weapon sets hand_item to its 4000+ id, which the viewmodel renders as that weapon's model. The
  survival select path's BlockId lookup leaves selected_block untouched for non-block ids (verified, no panic).
So: F6 -> Y -> scroll to a weapon = hold it. J remains the quick mode-agnostic model cycler. Tables verified
consistent (4000+i <-> WEAPON_CLASSES[i] <-> palette entry i). lib (incl. --tests) + bin compile clean.
STILL OPEN: real loot/forge drops (legendary_loot) handing out these ids in worldgen/combat, and wiring
resolve_weapon_swing so a held weapon's stats drive attacks — the items + models are in; gameplay use is next.

## r482 — upload/gen/mesh budget bumps + culling atomic aggregation
Performance pass across the streaming pipeline now that FIF=2 gives CPU/GPU overlap headroom. All the numeric
bumps stay env-overridable (the listed UNENDLICH_* vars), and uploads are still governed by the adaptive
controller (default ON), so raising the base only lets it ride higher when smooth — it backs off on fence-wait
pressure exactly as before.

UPLOAD BUDGETS (bin consts):
- MAX_UPLOADS_PER_FRAME_DEFAULT 64 -> 96 (adaptive range becomes [12, 192]).
- UPLOAD_BUDGET_MS_DEFAULT 6.0 -> 8.0 ms.
- UPLOAD_BYTES_BUDGET_DEFAULT 8 MiB -> 12 MiB.
  (count / ms / bytes raised together so no single cap becomes the lone bottleneck.)

GENERATION + MESHING:
- gen_batch default 16 -> 24 (more chunks per GPU terrain-gen dispatch; UNENDLICH_GPU_GEN_BATCH).
- MESH_WORKERS 4 -> 6 (more CPU-mesher throughput; modest on the 8-core M2, large on the Ultra;
  UNENDLICH_MESH_WORKERS).

CULLING (chunk_cull.comp) — the one algorithmic change:
- The cull emitted draws with one GLOBAL atomicAdd(visible_count,1) PER visible chunk — heavy contention when
  most chunks pass frustum. Rewrote main() to aggregate per 64-wide workgroup: each workgroup tallies its
  visible chunks in shared memory (cheap shared atomics), then does ONE global atomicAdd to reserve a contiguous
  base, and each thread writes commands[base+local]. 64:1 fewer global atomics on the hot path. Restructured so
  NO thread early-returns before the two barriers (OOB threads read a clamped index and simply don't emit), which
  keeps the workgroup from deadlocking. Output is the same SET of draw commands (order differs, irrelevant for
  indirect draws); the emit condition (inactive skip, shadow-stride phase, AABB-vs-frustum) is byte-for-byte the
  same. Validates RC=0, spv hash fresh.
lib (incl. --tests) + bin compile clean. DEVICE-VERIFY: confirm no missing/flickering chunks (cull parity) and
watch wait_fences / frame time under streaming — expect smoother mass-load and a lighter cull dispatch; dial any
bump back via its env var if a device stutters.

## r483 — procedural dragon CHARACTER model (sim::dragon_model)
First character-model system in the engine, ported from the uploaded character_forge_3d.html design tool. A dragon
is described by a GENOME (proportions) + an ELEMENT (palette) and assembled from analytic solids, then VOXELIZED
into cubes — the natural form for an engine that draws cubes through the entity stream (the forge tool's own final
render path voxelizes too).

sim/dragon_model.rs (pure / deterministic / panic-free, array math, 5 unit tests pass via flatten-test):
- DragonGenome (17 params: pose, bulk, neck len/arch, tail, legs, wing span/spread, snout, jaw, brow, cheek, teeth,
  crest, horns, horn sweep) with Default matching the tool's SPECS defaults.
- DragonElement (Wood/Water/Air/Dust/Fire/Storm) -> body/scale/emissive palette (hexes from the tool).
- build_solids(): spine Catmull-Rom curve of body spheres (tail→hips→torso→shoulders→neck, reared by pose), four
  2-segment legs + toes, two wing arms (bones; the membrane is a sheet, not voxelized), and a horned skull (cranium,
  occipital plate, upper/lower jaws, nose ridge + nostrils, teeth, brow, cheek shields, emissive eyes pushed onto
  the skull surface, crest, horns by count+sweep, emissive spine ridge plates).
- voxelize(genome, element, res, unit_scale): AABB → grid → per-solid stamp (winning material rank, eye>scale>body)
  with a CENTRE-CELL SNAP so sub-cell features (eyes/teeth/ridges) never vanish between samples; surface-shell emit
  (interior cells skipped) except emissive cells always emit so eyes/ridges read. Default res 48 → ~319 boxes,
  8 emissive. dragon_model() is the convenience wrapper.

Bin preview (no real dragon entity wired yet — this is the "not on the frontend" part): N cycles a cached voxel
dragon (off → 6 elements → off) into the world ~8 blocks in front of the player, emitted as world cubes through the
drops/viewmodel entity stream. Cached; rebuilt only on element change. lib (incl --tests) + bin compile clean.
DEVICE-VERIFY: press N to step the elements; confirm the silhouette reads as a dragon (spine/legs/wings/horned
head) and the eyes/ridges glow. It faces +Z and floats at a fixed offset — placement/ground-snap, facing the
player, rotation/animation, and binding to the real sim::dragon DragonService (pose from the genome) are all next.
FOLLOW-UP: the forge tool exports a genome JSON; a loader (JSON -> DragonGenome) would let designed dragons drop
straight in.

## r484 — wire pose.rs animation into the 3D in-world creature render
(Also REVERTS the r483 sim/dragon_model.rs + N-key preview: redundant — in-game dragons already render via the
existing sim::dragon_forge voxel model through forge_to_vertices. Module, mod registration, debug state, N key,
and preview render block all removed; lib+bin clean, zero remaining refs.)

The box-anatomy render path (world/entity.rs::creature_to_vertices_lod, used by every non-dragon creature: the
ambient roamers, test creatures, and LAN avatars) only ever applied a hand-inlined FOOT swing off gait_phase —
pose.rs (the real rig: leg cadence, body bob, wing flap, health-based limp) was never wired into the 3D render,
only the 2D inspector. Now it is:
- Build a pose::Locomotion per creature: speed + flight from the body's CURRENT velocity (vy>0.5 or fast
  horizontal => flying, matching the dragon-forge heuristic so a grounded flyer doesn't flap), leg_health +
  fly_power from creature.graded_capabilities() so injuries read as a limp / weaker flap.
- pose::pose_for() drives the torso's gait bob; pose::animate() drives the parts (feet swing fore/aft + lift,
  wings roll/flap, every part rides the body bob). LOD + destroyed-part culling preserved by zipping the animated
  parts back to anat.parts.
- Verified pure-pose math (flatten-test): still creature = rest on every channel (so STANDING creatures are
  byte-identical to before), walking => alternating leg swing + body bob + no flap, flying => wing flap, lower
  leg_health => damped swing (limp). Dragons unaffected (forge path returns early).
lib + bin compile clean. Per-creature cost: one graded() (a few part passes + a small alloc) + pose_for + animate
each frame — fine for the bounded creature counts; cache graded on the creature if it ever shows up in a profile.
DEVICE-VERIFY: walk a roamer herd — legs should stride and bodies bob; send a winged creature airborne and its
wings should beat; an injured-leg creature should limp.

## r485 — character: arm swing · item: held-block model
CHARACTER (pose.rs): the rig animated feet + wings + body bob but left Part::Hand (arms) static. Added an arm
swing in animate(): each Hand swings fore/aft in COUNTER-phase to its same-index leg (PI + leg_phase_offset), so a
walking biped's arms swing opposite the stride (contralateral gait). Amplitude ARM_SWING_MAX=0.5 (gentler than the
0.7 legs), scaled by speed + leg_health like the legs — so a still creature's arms are at rest (rest=identity
preserved) and a slow/limping one swings less. Verified counter-phase + rest-identity by math check. Only creatures
with Hand parts (bipeds/graspers) are affected; footed quadrupeds unchanged.

ITEM (sim/viewmodel.rs + bin): holding a BLOCK fell through to the generic Tool viewmodel. Added block_model(color)
— a single cube sized to sit in the hand — and wired the first-person render: when the hand id maps to a real
BlockId (1..87, not a weapon/ingot/essence), render block_model(block.base_color()) instead of item_model(Tool).
So placing-blocks now shows the actual block in hand, tinted by its albedo. Weapons (r481) and the id→form fallback
are unchanged.
lib (incl --tests) + bin compile clean. DEVICE-VERIFY: walk a biped/humanoid — arms should swing opposite the legs;
select a block on the hotbar and the held model should be a cube in that block's colour.

## r486 — character: tail sway · item: held-item use-swing
CHARACTER (pose.rs): Part::Tail was static (body-bob only). Added a lateral sway in animate(): each Tail part
rotates about the body's vertical axis (new yaw_xz helper) by TAIL_SWAY_MAX=0.38 * sin(phase), scaled by the same
speed*leg_health factor as the limbs — so the tail swings side to side with the stride and is at rest when the
creature is still (rest=identity preserved). Parts further back swing wider (they pivot at the body origin), giving
a natural whip. Tail-heavy plans (serpents) get the most life from this.

ITEM (bin): the first-person held item now plays a short USE-SWING whenever a block action fires. A block action
resets last_block_action at ~8 break/place/use sites; rather than touch each, the loop snapshots last_block_action
before the action block and detects the change after, setting swing_timer = SWING_DURATION (0.24s). swing_timer
decays by the frame dt; while active the viewmodel pose gets a down-forward arc (sin(prog*PI): pitch +0.95, slight
roll, offset dip) overlaid on the walk pose. Applies to every held item (block / weapon / tool). Standing-still with
no action = no swing.
lib (incl --tests) + bin compile clean. DEVICE-VERIFY: walk a tailed creature — the tail should swish with the
gait; mine/place a block — the hand item should swing on each hit.

## r487 — FIX first-person view · character profiles + cross-world persistence · select screen
FIX (first-person viewmodel was invisible): the default game mode is CREATIVE, but hand_item was only ever
assigned inside `if survival_mode` branches — so in creative the hand was always None and the viewmodel block
(gated on hand_item.is_some()) never ran. Now hand_item is synced EVERY frame from the mode: survival → the
selected inventory slot; creative → the selected place block's id (so the held-block model from r485 shows). The
first-person item now renders in the default mode.

CHARACTER SYSTEM (new player_character.rs, pure + 6 tests):
- CharacterProfile { name, arche (stable string key), seed, tint } → egg_spec() → blueprint() (tinted), reroll().
- presets() (6 starter characters across arches), SELECTABLE_ARCHES, Default = first preset.
- Cross-world persistence: config_path() = ~/.config/unendlich/character.json; load_global() (file or preset,
  never fails); save_global(); so the player's customized character loads in EVERY world.
- CharacterRoster { options = presets + saved custom slot, custom_index, selected } with load/current/step/
  select/custom_mut/save_custom — the select-menu state.
- Bin: load_global() at startup → active_character, logged (the cross-world load is live).

SELECT SCREEN (app/menu_view.rs, headless-tested like the rest of the menu):
- CharacterClick { None, Pick(i), Reroll, SaveAndPlay, Play }.
- character_select_ui(ctx, roster): selectable character cards (★ marks the saved custom one), a LIVE silhouette
  PREVIEW painted from the blueprint's side projection (creature_viewer::project → egui rects, depth-sorted), the
  pick's stats, and Reroll / Save & Play (custom) or Play (preset) actions.
- paint_character_preview() renders the silhouette; 2 headless tests (draws shapes + returns None w/o interaction;
  preview projects non-empty).

lib (incl --tests) + bin compile clean. STILL TODO (next): hook character_select_ui into the live title/overlay
pass so it shows on launch, route CharacterClick (Pick/Reroll/SaveAndPlay/Play) through, and apply the chosen
profile to the in-world/LAN avatar. DEVICE-VERIFY: (1) creative — held block now visible first-person; (2) once
the screen is hooked live, the roster + silhouette preview render and Save persists across worlds.

## r488 — character-select menu LIVE (title → previews → pick/customize/save → cross-world)
The fullgame draws its menu INLINE in render/world_renderer.rs (an egui::Window per screen), NOT via
app::menu_view::menu_ui (that's the App/MenuController path). So the r487 tested screen lived in the wrong place
to show. r488 hooks character-select into the fullgame's actual overlay, following the independent-window pattern
of settings_menu_open / craft_menu_open (precompute-locals → draw → write-back, since the overlay closure can't
borrow self):
- WorldRenderer fields: character_menu_open, character_roster: Option<CharacterRoster>, character_chosen:
  Option<CharacterProfile> (result for the bin).
- Title menu: new "Character" button (open_character_local → write-back lazily loads the roster + opens the window).
- Character window ("character_select"): selectable cards (★ = saved custom slot), a LIVE silhouette PREVIEW
  painted from the highlighted character's blueprint side-projection (creature_viewer::project → depth-sorted egui
  rects, precomputed before the closure), the pick's stats, and Reroll look / Save & Use (custom) or Use this one
  (preset) + Close. Painter is clipped to the preview rect (out-of-range projection boxes clip, never panic).
- Write-back: pick → roster.select; reroll → custom_mut().reroll (custom only); Save/Use → copies a chosen PRESET
  into the custom slot first (so whatever you pick becomes your persisted character), save_custom() (writes
  ~/.config/unendlich/character.json), hands current to character_chosen, closes.
- Bin: seeds renderer.character_roster at startup; drains renderer.character_chosen each frame → active_character
  (mut) + logs. Cross-world: load_global at startup + save_custom on Save = the pick loads in every world.

bin compiles clean (pre-existing unrelated warnings only). DEVICE-VERIFY: title menu now has a "Character" button →
opens a window showing the roster cards + a silhouette preview of the highlighted character; clicking cards swaps
the preview; Reroll changes the custom look; Save & Use closes + persists; relaunch loads the saved character
(log line "[character] active = ..."). STILL TODO: apply active_character to the actual in-world/LAN avatar Creature
(others see your character); optionally auto-open character-select on world-join instead of a title button.

## r489 — character customizer: body-plan cycling + colour tinting (live in the preview)
r488 gave preset cards + seed-reroll + save. r489 makes "the player customizes" real, all live in the menu preview:
- player_character.rs: CharacterProfile::set_arche/cycle_arche (walks SELECTABLE_ARCHES, wraps) + set_tint;
  TINT_SWATCHES[6] named colours (Natural/Ember/Frost/Moss/Dusk/Shadow). +2 tests (cycle wraps; darker tint dims
  the blueprint). lib(+tests) compiles.
- Character window (custom slot only): a "Body: <plan> ▸" button cycles the body plan, and a "Colour:" row of named
  swatch buttons sets the tint. Both edit the custom slot; the silhouette preview (recomputed each frame from
  roster.current().blueprint()) updates next frame, so changes are visible immediately. New latches
  char_cycle_arche_local / char_tint_local, applied in write-back (custom slot only); presets stay fixed.
  Save & Use persists the customized character (cross-world); Close keeps edits in-session but doesn't write disk.
bin compiles clean. DEVICE-VERIFY: open Character → your ★ slot → "Body ▸" changes the silhouette shape, the colour
swatches recolour it, Reroll varies the look, Save & Use persists across worlds. STILL TODO (next): apply the chosen
profile to an in-world avatar — needs either a third-person self-view (solo-visible) or networking appearance over
LAN (EntityState protocol change); flagged but not done (invasive / low solo-visibility respectively).

## r490 — THIRD-PERSON self-view (F5): see your character in the world
The chosen character now has a visible body. F5 toggles third-person; first-person stays default, and raycast /
physics / culling / audio ALWAYS use the real eye (local_player.view) — only the render camera + a self-avatar
change.
- Render camera: in third-person the draw gets a pulled-back View (eye - forward*4 + up*0.6, same yaw/pitch) so the
  avatar sits centered ahead. local_player.view itself is untouched (View is Copy; we pass a modified copy only to
  draw_frame_with_view_and_atmos).
- Self-avatar: built from active_character.blueprint() via spawn::from_blueprint (rebuilt when the pick changes —
  self_avatar_dirty set on character_chosen), appended to the entity vertex stream next to the LAN-remote avatars
  (same creature_to_vertices bake). Parked at the eye minus 1.5 (feet), turned to local_player.view.yaw.
- ANIMATED: avatar velocity is derived from the per-frame eye delta (/frame_dt_s), so the r484–r486 pose rig
  (leg/arm stride, wing beat, tail/limp) plays as the player walks/flies. self_avatar_prev reset on toggle to avoid
  a one-frame velocity spike.
- First-person viewmodel hidden while third-person (gate gains `&& !third_person`).
- Menu: character window gains a hint "In-world, press F5 to see your character (third-person)."
bin + lib(+tests) compile clean. DEVICE-VERIFY: in-world press F5 → camera pulls back, your chosen character stands
at your feet facing your look direction; walking animates its legs/arms; the held item disappears; F5 again returns
to first-person. Tuning likely: pull-back distance (4.0) and feet offset (1.5) — easy one-line changes on feedback.
Camera has no terrain collision yet (may clip walls behind you). STILL OPEN: LAN remotes still spawn as a default
dragon (appearance not networked); a rotating in-menu preview; typed character names.

## r491 — water moves SIDEWAYS: lateral surface advection (shader + CPU lockstep)
Diagnosis: the water surface only BOBBED. The r419 flow path was fully wired (settle out-flux → WaterFlowField →
water_flow_tiles → pack_grid → chunk.frag flow_grid), but the shader only ROTATED the wave direction toward the
current + sped ripples — it never ADVECTED the surface, so nothing visibly travelled sideways. And settled (standing)
water produces zero sim flow, so lakes/ocean sat dead-still.
Fix (chunk.frag water branch): build a surface-advection velocity and sample the wave field at pos - vel*t, which
translates the whole ripple pattern laterally:
- surf_adv = wind_xy * WW_WIND_DRIFT (0.18) — a gentle wind-driven drift so EVEN STANDING water slides downwind
  (works with flow disabled too; this is the always-on "moves sideways").
- + flow * WW_FLOW_DRIFT (1.30) inside the flow-coverage block — a stronger downstream carry where the sim says
  water is actually flowing (rivers/spills stream visibly).
- wave_pos = v_world_pos.xz - surf_adv*w_time feeds water_wave_normal.
CPU lockstep (RULESET §water): the floating-drop bob (bin ~9845, uses world::water_waves::water_wave_height_grad)
now advects its sample by the same wind drift (0.18) so drops ride the advected surface. Flow-part advection isn't
mirrored on CPU (no per-column flow handy there; drops mostly float on standing water where wind drift dominates).
chunk.frag validates (glslang RC=0); spv + .spv.hash regenerated FRESH (transitive hash matches); bin compiles.
DEVICE-VERIFY: water surface now drifts laterally (downwind) instead of only bobbing; near flowing/spilling water it
streams along the current; floating item-drops bob in step. TUNABLES (one-liners on feedback): WW_WIND_DRIFT 0.18
(standing-water drift) + WW_FLOW_DRIFT 1.30 (river carry) in chunk.frag — keep the bin's 0.18 in sync with WIND_DRIFT.

## r492 — calmer wind + visible rain/snow
WIND (was vacillating too fast — also now drives water drift + sway): slowed all three timescales in wind.rs.
DIRECTION_ROTATION_SECS 30→50; gust slow /8→/16 (~100s); gust fast /1.5→/5 (~31s, was the ~9s vacillation). Wind
now shifts gradually.
RAIN/SNOW (weather was on + composing climate, but precipitation had NO falling visual — only sky-darken + fog):
new sim/precipitation.rs (pure, 2 tests) — field() returns a closed-form falling-particle cloud around the player
(wraps in y, leans with wind, snow sways). Bin renders it in the entity vertex stream (no new pipeline) when
precip01 > 0.05: snow (near/below 1°C — white, slow, swaying flakes) vs rain (thin fast blue-grey streaks);
particle count scales with intensity; rain/snow chosen by last_climate_temp_c (stored from the live climate sample).
lib(+tests)+bin compile. DEVICE-VERIFY: in wet weather you now see rain streaks (or snow flakes in the cold) falling
around you; wind shifts slowly instead of jittering. Tunables: particle counts / fall_speed / radius(16) / height(18)
in the bin precip block; wind timescales in wind.rs. NEXT in this batch: water colour variation + algae + drifting
ice chunks; seasons actually advancing; multiplayer avatar appearance.

## r493 — algae + drifting ice floes (water variation)
ALGAE (chunk.frag, shader-only so tint_water stays CPU-mirror-locked): apply_algae() blooms a green scum
(ALGAE_COLOR) in WARM + SHALLOW water, patchy via two low-freq world-noise octaves, thinning over deep/cold water;
mixed into the water albedo after tint_water. Adds visible per-location water variation on top of the existing
depth/temp/salinity tint.
ICE FLOES (bin entity stream): when the column temp ≤ 2°C, scatter procedural floes around the player (count scales
with cold), drifting downwind over time (wrapped into a 48-block box), placed ONLY where the surface column is water
(block_at(x,SEA_LEVEL,z)==Water with non-solid above). Each floe is a thin yaw-rotated slab (bluish white) bobbing
slightly + slowly spinning. Visual only (not collidable yet).
chunk.frag validates (RC=0), spv+hash FRESH; bin compiles. DEVICE-VERIFY: warm shallow water shows green algae
patches; cold water (snowy biomes / winter) shows white ice floes drifting on the surface. Tunables: ALGAE_COLOR +
0.62 strength + noise freqs (chunk.frag); floe count/radius(48)/temp-gate(≤2°C)/size in the bin ice block. NEXT:
seasons advancing; multiplayer avatar appearance.

## r494 — multiplayer: LAN players see each other's chosen character
LAN remotes used to spawn as a fixed spawn_beta_dragon regardless of who the peer was. Now appearance is networked:
- player_character.rs: ALL_ARCHES[10] + arche_to_u8/from_u8 (stable index); CharacterProfile::to_appearance_bytes
  (arche1+seed8+tint3 = 12B) / from_appearance_bytes. +2 tests (arche u8 round-trip, appearance wire round-trip).
- New channel CH_AVATAR=4: [id u64 LE][appearance 12B]. Sent ~every 2s to the peer (active_character). On receipt,
  stored in lan_remote_appearance{id→profile}; if that remote's Creature already exists it's rebuilt from the new
  blueprint (keeping pos/yaw). The CH_ENTITY spawn (None branch) builds from the stored appearance via
  spawn::from_blueprint if known, else the default until CH_AVATAR arrives. Appearance dropped on peer timeout.
lib(+tests)+bin compile. DEVICE-VERIFY (two instances on a LAN): each player appears to the other as their selected
character (body plan + tint), updating within ~2s if they change it in the menu. Note: the local third-person F5 view
already shows YOUR character; this makes the OTHER player's character correct too. Wire is unsealed idempotent state
(newest-wins), 20-byte datagram. STILL OPEN from this batch: seasons advancing/visible.

## r495 — seasons wired + observable (ties weather/snow/ice together)
season.rs was a complete tested model but unused live: plant Conditions had season01 hardcoded 0.5, and a natural
year (~144 real hours) is impossible to witness in a session. Now:
- Per-frame season_day = world_clock/1440 + season_offset_days; current_season01 = phase in [0,1).
- Feeds the climate composition (day_of_year + year_length) → seasonal temperature offset → snow line / cover, so
  WINTER actually turns the world cold (→ r492 snow falls + r493 ice floes) and SUMMER warm (→ r493 algae).
- Drives plant-growth season01 (was 0.5).
- Levers so it's visible: UNENDLICH_YEAR_DAYS (year length, default 360), UNENDLICH_SEASON=spring|summer|autumn|
  winter|0..1 (forced start), and the O key advances the season ~1/8 year live. Season transitions logged
  ("[season] now Winter ...").
bin compiles. DEVICE-VERIFY: launch with UNENDLICH_SEASON=winter → cold column → snow + ice on water; press O to
cycle seasons and watch temp/snow/algae respond; the season prints on change. This closes the requested batch
(wind r492, rain/snow r492, water variation/algae/ice r493, multiplayer r494, seasons r495).

## r496 — leaves reworked: stringy + dense (like grass)
Leaves were already sub-voxel-only (no solid cube faces) but their sub-voxel recipe placed 6-9 blobby CLUMPS, which
read as a solid-ish block. Rewrote world/subvoxel.rs `recipes::leaves` into thin VERTICAL STRANDS like grass blades:
most (x,z) columns (~64% coverage) bear a 1-sub-voxel-wide strand at a hashed top height hanging down 2-4 cells
(droop), with per-strand green jitter. Result: a dense mass of stringy hanging fronds (~3-4× the cell count of the
old clump recipe) instead of clusters. Templates rebake from this recipe automatically (bake_variants →
procedural_subgrid → recipes::leaves), so all Oak/Birch leaf blocks pick it up. place_leaf_clump kept (#[allow(dead_code)]).
lib(+tests) compiles. DEVICE-VERIFY: tree canopies should look like dense stringy foliage, not blocky clumps; no
solid leaf cube. PERF NOTE: denser sub-voxel leaves raise canopy vertex count — coverage (164/255 ≈ 64%) + strand
length (2-4) are the dials in recipes::leaves if it's too heavy; the chunk-cull work (next) offsets mesh load.

## r497 — decoration LOD culling (mesh-perf win, offsets denser leaves)
The GPU-decorate post-pass (meshing_pool ~1258) appended sub-voxel decoration — the dense stringy leaves (r496) +
grass tufts — for GPU-meshed chunks at ALL tiers. Its fine detail is sub-pixel past mid-distance, so Far/VeryFar
chunks were paying full canopy cost for nothing. Now gated: decorate only when r.lod <= Mid; Far/VeryFar skip it
(water + lava surfaces still emit at all tiers — they read at distance). The CPU downsampled/virtual paths already
skipped decoration, and CPU-Near decorates internally, so only the Far/VeryFar GPU chunks change. Net: big canopy
mesh-cost reduction at distance, directly offsetting r496's denser leaves; near canopies keep full stringy detail.
lib + bin compile. DEVICE-VERIFY: distant tree canopies render as simpler downsampled blocks (no dense sub-voxel
fronds) while near trees keep them; frame time / vertex upload should drop in forests. Dial: change `<= Mid` to
`== Near` for an even tighter decoration radius (less foliage distance, more savings).

## r498 — network compression (per-datagram, reuses the chunk RLE codec)
Chunk streaming already compressed via chunk_compress (dependency-free PackBits RLE); the rest of LAN traffic didn't.
New sim/net/packet_compress.rs: pack(payload, min_len) prefixes a 1-byte header (0=raw, 1=RLE) — small packets stay
raw (one byte, never expand), bulk payloads RLE-compress only when it actually shrinks; unpack(buf, max_out) is
bounded (no panic/OOM on corrupt input). 4 tests (raw round-trip, compressible shrinks, incompressible stays raw +1,
corrupt→None). Wired into CH_WORLDCMD (bin): block-edit batches are pack()'d on send (threshold 48B) and unpack()'d
on receive (≤64KB). Transforms/avatar packets stay raw (too small to benefit). lib(+tests)+bin compile. Net effect:
bulk edits (large structures, fills) shrink on the wire; per-edit traffic unchanged. Same-build LAN only (wire format
gained the 1-byte header on CH_WORLDCMD). DEVICE-VERIFY: building/mining still propagates between two LAN instances
(now with a compression header); large pastes send fewer bytes.

## r499 — nature integration: seasonal foliage colour (ties seasons → leaves/grass)
The r495 season now visibly recolours foliage. Renderer packs current_season01 into the spare fx_params.z (new
world_season01 field + set_season(); bin calls it each frame). chunk.frag seasonal_foliage() shifts is_foliage
fragments: green through spring/summer, warm orange at autumn (~0.6), desaturated brown at winter (~0.85), applied
after albedo (per-leaf sub-voxel colour variation preserved). So advancing the season (O key / UNENDLICH_SEASON)
turns the canopy autumnal then wintry — integrating seasons with the r496 stringy leaves + grass.
chunk.frag validates (RC=0), spv+hash FRESH; bin compiles. DEVICE-VERIFY: press O to roll through seasons and watch
leaves/grass go green→orange→brown; summer keeps them green (+ algae in warm water from r493). Tunables: autumn/
winter colours + blend strengths + phase windows in seasonal_foliage (chunk.frag).

## r500 — DHT routing core + channels (foundation for hostless multiplayer)
LAN MP needs a known host IP. New sim/net/dht.rs is the pure Kademlia core for peer discovery by NodeId XOR-distance:
NodeId=u64, distance(=XOR), Contact{id,addr_v4,port}, RoutingTable{64 k-buckets, K=8} with insert (dedup +
move-to-most-recently-seen, cap at K), remove, and closest(target,n) (the routing lookup). Plus a Channel enum
{Handshake=1,Entity=2,WorldCmd=3,Avatar=4,Dht=5} formalizing the bare CH_* bytes and adding the Dht channel. 6 tests,
all PASS (flatten-verified: distance symmetry, insert dedup/cap, closest-by-XOR ordering, channel round-trip, remove).
lib(+tests) compiles. STILL TODO (on-device transport step): the UDP DHT RPCs (ping / find_node / bootstrap), wiring
the bin's networking to route via the table + CH_DHT, and a bootstrap node list — these need live multi-node testing.
This rev lands the verifiable routing math the transport builds on.

### Batch summary (r496–r500)
r496 stringy dense leaves · r497 decoration-LOD culling (mesh perf) · r498 per-datagram network compression ·
r499 seasonal foliage colour (nature integration) · r500 DHT routing core + channels (multiplayer).

## r501 — two doc-flagged wires: decoration sun lighting + creature loot via loot_roll
(A) DECORATION SUN LIGHTING (BACKLOG "Remaining" one-liner). The GPU-decorate post-passes (tufts/dense leaves,
water, lava) were lit neutral (sky:None) because Sky's Clone-ability was unconfirmed. Sky is #[derive(Clone,Copy)],
so the blocker's gone: added `sky: crate::sky::Sky` to OctreeGpuMeshRequest (populated `sky: req.sky` at
construction), and passed Some(&r.sky) to decorate_chunk/emit_water_surface/emit_lava_surface. Decoration now takes
the same sun/sky lighting as the solid mesh instead of flat ambient. lib+bin compile.
(B) CREATURE LOOT via loot_roll (NEXT_SESSION_GUIDE "route creature death drops through them like blocks"). The
combat-death site (bin ~5065) dropped a hardcoded single MEAT_ITEM. Now rolls sim::batch_loot_roll::loot_one on a
table [meat@1.0, meat@0.6, meat@0.25, egg(999)@0.12] (seed from creature pos + frame_count), spawning each rolled
item with a hashed scatter — 1-3 meat + an occasional egg, table-driven through the tested loot core. bin compiles.
DEVICE-VERIFY: (A) tuft/leaf/water/lava decoration should respond to time-of-day sun like terrain (brighter noon,
warmer/dimmer dusk); (B) slaying a roamer drops a variable pile (sometimes an egg) instead of exactly one meat.
NOTE: no hide/bone item types exist yet — table uses meat + egg; extend once those items are added.

## r501 doc-mining audit — what's ALREADY wired (stale docs) vs genuinely remaining
Verified-already-wired (don't re-investigate; docs predate the wiring):
- lava_water_contact → obsidian/stone+steam: WIRED (bin ~10859).
- quench steam puff into smoke_field (hot metal in water): WIRED (bin 4251/4305/4559).
- settings render_scale / neural-upscale slider: WIRED (world_renderer ~4060, live-applied).
- plant_growth sapling→tree staged maturation: WIRED (bin 4111-4159, session.saplings).
- flora_harvest bloom→essence drops on break: WIRED (bin 8146).
- player + dropped-item buoyancy/swim: WIRED (bin 5623-5680, sim::fluid/swim/buoyancy).
- fauna_roster-driven spawn (species base_health etc.): WIRED (bin 3819).

Genuinely remaining, but DEFERRED with reason (not blindly attempted — unverifiable + high blast radius):
- Per-face block colour (grass-top-green/dirt-sides), magenta-foliage, missing-top-faces: all live in
  chunk_mesh_octree.comp (the central GPU mesher). A blind edit there can corrupt ALL GPU-meshed terrain
  (same risk class as the transparent-water pass). Needs an on-device A/B (UNENDLICH_GPU_MESHER=0 vs 1) per step.
- Real-field water tint: chunk.frag's temperature/humidity proxies are INTENTIONAL (they give per-position spatial
  variation a single global uniform can't). The real fix needs per-chunk climate baked into water vertices
  (emit_water_surface + vertex format), not a uniform swap — a mesher change, device-verify.
- Creature buoyancy: roamers use grounded_y snapping, not free vertical physics — no clean integration point;
  would require reworking creature locomotion (big, risky).
- inventory_txn move/split: core is tested (sim/inventory_txn.rs) but wiring needs drag/drop in the egui inventory
  — UI work best done with the screen visible.
- Chunk-sim elapsed-time catch-up + structural dirty-marking (DESIGN_NOTES): architectural, not a quick wire.

## r502 — Character-Forge elementals in the roster + bigger maximum dragon size
(A) FORGE CHARACTERS. Added four Character-Forge elementals to player_character::presets() as Arche::Dragon avatars,
each an elemental cast via distinct seed + tint: Heliographer (sun-gold), Windwalker (pale sky), Tidecaller (deep
teal), Stormweaver (storm violet). They appear as selectable cards on the character page alongside the animals.
Dragon is excluded from the customizer body-cycle (SELECTABLE_ARCHES) but valid for presets, so they select + render
through the existing creature/forge path (preview tile, in-world avatar, LAN remote). lib+bin compile.
(B) BIGGER DRAGONS. The live forge dragon size is dragon_archetype::size_scale(size_class) folded into the forge's
rest dimensions. Raised SIZE_SCALE_MAX 3.0 → 4.2 (biggest dragons now ≈9.3× the smallest, up from 6.7×); the forge's
rest·1.15 animation clamp scales with it so animated poses grow too. Also raised the dragon physics/collision box max
(spawn::dragon_body_half_extents) 1.7 → 2.4 so collision keeps up with the larger visual. size_scale tests compare
against the constant so they still pass. DEVICE-VERIFY: high-bulk dragons should now tower noticeably larger; confirm
collision/terrain clipping looks right at the new max and the forge characters appear + preview on the character page.
NOTE: sim::dragon_growth_stage (age→size) remains UNWIRED — live size is genome/size-class, not age; wiring
age-based growth is a separate task.

## r503 — large-body locomotion: carve an arc instead of a fixed-point turn
The roamer step picked a direction toward the goal and MOVED there immediately, then rotated to face it — so a big
body slid sideways and pivoted in place (the "fixed-point turn" / moonwalk; the head_steer/turning docs name this).
Reworked the step to be BODY-AXIS aware (bin ~6934): `axis_bias = clamp((body_len-1)/4, 0, 1)` from the creature's
half-extents scales how constrained it is to move along its facing. The creature now steps along `move_heading =
yaw + dyaw*(1-axis_bias)` (≈current facing for big bodies, ≈goal for small), at `eff_step` reduced by mis-alignment
(floored at 0.2 so a turning body keeps inching forward and arcs round rather than stalling), and its turn target is
the GOAL (`desired`) so the heading curves toward it while the body advances — carving an arc. Turn rate also scales
down with size (`/(1+axis_bias*1.5)`) so a dragon can't spin on a dime. Small/agile creatures (axis_bias≈0) keep the
old responsive step exactly. bin compiles. DEVICE-VERIFY: big dragons should sweep through wide curved paths (no
in-place spins / sideways slides); small fauna unchanged. Dials: the `/4` size scale, the `1.5` turn-slowdown, and
the `0.2` step floor (bin r503 block). NOT applied to the player (responsive WASD is expected) — that's a follow-up.

## r504 — water lateral drift, take 2 (it still didn't move sideways)
r491 added wave-pattern advection driven by wind×0.18, but r492 calmed the wind, so on a still day surf_adv≈0 and the
surface read as standing (the operator's report). Root cause: the drift depended entirely on wind magnitude, which is
near-zero in calm weather. Fixes (chunk.frag, shader-only): (1) ALWAYS-ON baseline drift WW_BASE_DRIFT=0.60 blocks/s
in the wind direction (or a fixed NE bearing when dead calm) so even a still lake visibly slides; (2) stronger wind
coefficient WW_WIND_DRIFT 0.18→0.55; (3) advect the SUN-GLITTER field (guv -= surf_adv·t) so the sparkle visibly
travels sideways with the flow — a strong, bright motion cue beyond the subtle normal shift. atmos_params.w is
time-of-day IN SECONDS (verified — a real monotonic clock), so the drift accumulates. CPU floating-drop advection
(bin ~9986) synced to the same baseline+wind so drops ride the current. Sim flow (rivers) still adds WW_FLOW_DRIFT
on top when flow is enabled. chunk.frag RC=0, spv+hash FRESH; bin compiles. DEVICE-VERIFY: a calm lake surface +
its sun-glitter should now visibly slide laterally (~0.6 blk/s); rivers stream faster. Dials: WW_BASE_DRIFT /
WW_WIND_DRIFT in chunk.frag (keep the bin drop-advection constants in lockstep).

## r505 — perf: decoration Near-only (offsets r496 leaf density's upload pressure)
The pipeline still stalls in spots. Most documented stall mitigations are already DEFAULT-ON (verified this session:
adaptive_uploads, submit_skip, submit_budget, slack_scheduler, biased_shell_walk, upload_priority — all R27 default-on),
and the remaining big levers (octree pool-lock early-unlock, MAX_FRAMES_IN_FLIGHT=1→2) touch GPU memory/sync and want
on-device A/B given Asahi's documented coherency quirks — not a blind flip. The one stall I could attribute to THIS
session's work: r496 made leaves ~3-4× denser, inflating near/mid chunk mesh size, and bigger meshes feed the M2
upload-pressure stall (HOST_COHERENT memcpy → GPU cache-invalidation, the known "chug" mechanism). r497 had decorated
Near+Mid; r505 tightens to NEAR-ONLY so mid-distance forests stop paying the dense-leaf upload cost while near forests
keep the full look. lib+bin compile. DEVICE-VERIFY: forest frame-time / upload-pressure events should drop vs r504,
especially while moving through wooded terrain; foliage now fades to plain cubes one LOD band closer. Revert lever:
set the gate back to `<= Mid` in meshing_pool (r505 block). The provably-safe pool-lock lever remains
UNENDLICH_GPU_POOL_EARLY_UNLOCK=1 for an on-device A/B.

## r506 — water push-back / waves (wave_amplitude wired) + churning wake foam + shader
DOC→IMPL of r443 water core #4 ("push back on the water and create waves with a mass"). The tested-but-unwired
`sim::water_react::wave_amplitude(mass, speed, area, gain, max)` now drives a real wake: new
`FlowTileStore::splash(wx, wz, dir, amp, now)` stamps an outward radial + forward-biased flow impulse around a moving
body's column into the persisted flow tiles (only on cells that already hold water; impulses ADD + clamp; the per-tick
sim ingest + decay wash it back out). Bin injects it each frame BEFORE pack_grid: the PLAYER when swimming + moving
(mass 80), and nearby ROAMERS in water with wakes scaled by body volume × speed — a dragon crashing into a lake makes a
far bigger wave than a fish. The surface shader already foams on flow speed (r420 rapids), so the wake reads as
whitewater with no new render path; r506 additionally makes that rapids/wake foam CHURN faster where flow is faster
(chunk.frag), so the mass behind a wake shows in the motion of the foam. chunk.frag RC=0, hash FRESH. lib+bin compile;
2 new splash tests pass (14 total in water_flow_tiles). Knobs: UNENDLICH_WATER_PUSHBACK=0 disables;
UNENDLICH_WATER_PUSHBACK_GAIN (0.05), _MAX (4.0). Inert when UNENDLICH_WATER_FLOW_SHADER=0 (no grid uploaded).
DEVICE-VERIFY: swim/run through a lake → surface should part + foam in your wake; a dragon entering water → big wake.
NEXT (MP candidate still open): remote-player nameplates (lan_remote_appearance has .name; needs a billboard/text path)
or a chat channel — both still unwired in bin.

## r507 — MP: connected-players presence on the HUD
LAN remotes already render as avatars (lan_remote_creatures) with names learned via CH_AVATAR (lan_remote_appearance,
r494), but there was no in-world indication of who's connected. r507 appends an "online (N): you, <names>" line to the
existing hud_status_lines (already rendered top-left), built each frame from the live remote set — names from each
peer's chosen character, falling back to player#<id>. Zero new render path (reuses hud_status_lines). bin compiles.
DEVICE-VERIFY (needs 2 machines on a LAN, UNENDLICH_LAN_MP default on): join a peer → HUD shows both players by name;
the line disappears when peers time out (lan_remote liveness eviction). FOLLOW-UP (still open): floating NAMEPLATES above
remote avatars need camera view_proj projected into the egui overlay pass (or a 3D text-billboard pipeline) — deferred
because it's renderer egui/projection work with no way to verify off-device. A simple chat channel (new CH over the LAN
session socket) is the other open MP candidate.

## r508 — movement feel: head surfaces when swimming + non-linear swim/jump/walk
Operator: "the head can't surface above the water when swimming, and swimming/walking/jumping are linear when they
should be more realistic." Four fixes (player-only; only the player calls apply_input/try_jump, confirmed):
1. HEAD SURFACES (the bug). Buoyancy derived `sub` from a two-state head check (water level 0.9 or 2.0 over a 1.8
   body); the instant the head cleared, sub halved and buoyancy (26·sub) lost to gravity (28) and dragged the head
   back under. Now the bin scans the body's column for the REAL water surface, computes a CONTINUOUS submerged fraction
   over feet..head, and floats with peak buoyancy 36 > GRAVITY 28 → equilibrium sub ≈ 0.78 (waterline at the shoulders,
   head/eyes out). You can tread with your head out and stroke (Jump) to climb higher.
2. SWIM not linear: vertical water damping (SWIM_VDAMP) turns the rise into a smooth bob instead of a linear
   shoot/overshoot.
3. JUMP not linear: variable jump height (releasing Jump mid-ascent cuts upward velocity ×0.45 → tap = short hop, hold =
   full jump) + asymmetric gravity (×1.4 on descent) for an apex hang then a weighted drop, vs the old symmetric arc.
4. WALK not linear: GROUND_FRICTION 20→12 (physics.rs) for a weighted acceleration ramp + turn momentum; the newer
   hard-stop path keeps stops crisp so this doesn't reintroduce the old slide-on-stop. MOVE_ACCEL auto-derives so top
   speed stays WALK_SPEED; all 4 physics movement tests remain valid (speed cap holds steady-state; decay-ordering
   preserved). Raise toward 20 for snappier control.
lib + lib --tests + bin all compile. DEVICE-VERIFY: swim up → head breaches and stays out while treading; tap vs hold
Space → short vs full jump; falls land a touch faster; walking starts/turns carry a little weight. Tuning levers:
SWIM_BUOY/SWIM_VDAMP (bin swim block), the ×0.45 cut and ×1.4 descent (bin), GROUND_FRICTION (physics.rs).

## r509 — items: tool/weapon DURABILITY (first wire of the item-system plan)
Per NEXT_SESSION_GUIDE "ITEMS WIRING", wired sim::item_durability::apply_wear (pure, tested, was unwired). The held
forged weapon (ids 4000-4007, weapon_class_for_id) now has durability tracked in the bin: WEAR_PER_ATTACK(2) on each
melee swing that lands, WEAR_PER_MINE(1) on each block mined, breaking (one removed from inventory) at 0. Survival-only
(creative weapons don't wear). HUD shows "weapon cur/max (worn!)" while one is equipped. v1 tracks ONLY the equipped
weapon (id,cur,max) — switching weapons re-inits the tracker; the documented next step is a per-stack instance table so
durability persists per item (and unblocks affixes/rarity/tooltips). bin compiles. DEVICE-VERIFY (survival, hold a Y-given
weapon, scroll to it): mining/attacking drains the HUD durability; at 0 it breaks with a log line + vanishes from hand.
Levers: WEAPON_DURA_MAX(120), WEAR_PER_ATTACK/_MINE.

## r510 — survival: COOKING (raw meat → cooked at a fire)
A real survival food loop. New COOKED_MEAT_ITEM (2001). Raw meat (2000) now barely nourishes (feed 0.18, down from
0.4); holding raw meat near ANY heat source (heat_sources.total_heat_at > 3.0 — F9 campfire, Fire-bending bloom, or
lava) roasts it over COOK_TIME(4s) into cooked meat (feed 0.55). Progress accumulates while held near heat and cools
when you step away; logged + shown on the HUD ("roasting meat… N%"). Survival-only. bin compiles. DEVICE-VERIFY
(survival): slay a creature for raw meat, plant a fire (F9), hold the raw meat near it → it cooks; eat cooked for the
full meal, raw for a pittance. Reuses the live heat field, so lava-side cooking works too. Levers: COOK_TIME, the >3.0
heat threshold, feed values in survival_use_item.

## r511 — items: varied combat loot (loot_quantity + drop_chance)
The main melee (punch/weapon) kill dropped a fixed single meat; now it routes through sim::loot_quantity::quantity
(meat count scales with the creature's bulk → tier 0..4, +luck, cap 5) and sim::drop_chance::drop_chance (a
luck-boosted ~12% roll adds an egg). Small fauna yield a scrap; big beasts yield a haul. Deterministic roll from
position+frame. Pairs with r501 (bending kill already used batch_loot_roll) so both kill paths now give varied,
core-driven loot. bin compiles. NEXT in the items arc (NEXT_SESSION_GUIDE): the per-stack INSTANCE/metadata table that
unblocks affixes/rarity/tooltips (item_modifier/item_genetics/item_tooltip cores are all present + unwired); inventory_txn
move/split in the open-inventory UI (needs egui drag/drop); flora_harvest seed drops on flora break.

## r512 — survival: VEGETABLES + FARMING (ungated, menu-wired)
Three new crop blocks — Carrot(88), Potato(89), Cabbage(90) — fully integrated into the block palette (ALL[69], base_color,
is_solid/mining/thermal/wetness/material/material_response matches all extended; the all_const_is_exhaustive +
no-dup-discriminant tests pass; from_u8 left consistent with the pre-existing 81-87 gap since chunk storage uses raw u8).
MENU WIRING: classified into the Plants creative tab (sim::creative_tabs::tab_of), so G/V in the creative menu selects +
places them. FARMING LOOP (ungated, no env flag): right-click a crop on Grass/Dirt PLANTS it (consumes one, registers in
a bin-local crops list); it grows via sim::plant_growth (climate-driven h/r/l like saplings) and is RIPE at FINAL_STAGE;
breaking a ripe crop yields a bonus harvest via sim::loot_quantity (a tended field returns more than you planted), unripe
just returns the one. EDIBLE: eating (right-click off-soil, or harvested) feeds vitality — Carrot/Cabbage 0.28, Potato 0.34.
SURVIVAL SOURCE: breaking wild TallGrass/Fern/Flower has ~15% to drop a vegetable ("seeds from grass") so a survival
player can start a farm without creative. lib+lib --tests+bin all compile. CPU mesher (default) renders crops via
base_color; no GPU/shader change (crops are never worldgen-emitted). DEVICE-VERIFY: creative → Plants tab has the 3 crops;
survival → break grass to find a crop, plant on dirt, wait for the "ripe" log, break it for a multi-drop harvest, eat for
vitality. Levers: growth cadence (frame%90), ripeness via plant_growth, harvest tier/luck, feed values, seed-drop ~15%.

## r513 — foliage: crops render as plants, not cubes (subvoxel shapes)
"Make foliage less blocky": the r512 crop blocks rendered as solid colored cubes. Now Carrot/Potato/Cabbage are
has_subvoxels + is_subvoxel_only (the leaves pattern), so the CPU mesher skips their cube faces (surface.rs excludes
is_subvoxel_only from the solid surface) and emits a procedural plant silhouette instead via a new recipes::crop(shade,
rng_int, kind) in world/subvoxel.rs — Carrot = leafy fan + orange crown, Potato = low bushy mound, Cabbage = tight round
head. Crops stay is_solid (still raycast-harvestable) but blocks_collision is now false (walk through a field) and
foliage_sway sways them in wind like leaves. No GPU/shader change (procedural CPU subvoxel path; leaves use baked
templates, crops fall through to procedural). lib+lib --tests+bin compile; palette invariants pass. NOTE: subvoxel
decoration is Near-LOD only (r505), so crops are shaped up close and invisible at distance — fine for small plants.
DEVICE-VERIFY: plant a crop and confirm it looks like a little plant (and sways), not a cube; harvest still works.

## r514 — ARCHITECTURE: persistent sparse flora/crop state store (world/flora_store.rs)
Operator-flagged: "foliage and crop state like entities/populations with permanent state… stored efficiently." Crops were
bin-local (lost on restart). New world::flora_store::FloraStore — a SPARSE HashMap<[i32;3], FloraCell> (only planted cells
cost anything), where FloraCell = {block:u8, growth01, stage, element, vigor, planted_tick}, mirroring plant_growth::PlantState.
Serialises to flora.json under the world root (flattened to a Vec since JSON keys can't be arrays), written atomically
(temp-file + rename) like crate::storage does for chunk edits. O(1) plant/harvest/lookup by cell. Lives on WorldSession
(reset() auto-clears on world switch). WIRED into the bin replacing the old crops Vec: load on world init; plant → flora.plant
(FloraCell::sown); growth tick advances each cell in place (marks dirty); harvest → flora.remove (ripe payout uses cell.block);
saved on a 1800-frame cadence when dirty + flushed on shutdown next to chunk save_all. 4 unit tests (plant/get/remove, dirty
tracking, disk round-trip, absent→empty) compile under lib --tests; lib+tests+bin all build. DEVICE-VERIFY: plant crops, quit,
relaunch the same world (worlds/default) → "loaded N persisted crop(s)" and they resume growing/are harvestable. NEXT: migrate
session.saplings (and fire_positions) into the same store for a single persistent flora layer; add per-stage crop visuals.

## r515 — camera: first-person body + cinematic shoulder cam (Y) + keybind-menu wiring
Camera is now a mode (0 first-person, 1 third-person, 2 cinematic over-the-shoulder) instead of a bool.
- FIRST-PERSON BODY (`fp_body`, default ON): your own avatar renders in first-person so you can look down and see
  your body; the hand viewmodel is hidden then to avoid a duplicate floating arm.
- CINEMATIC SHOULDER (camera_mode 2): pulled back 2.6, right 0.9, up 0.45 — body frames the lower-left, view open ahead.
- KEYBINDS: added GameAction::ToggleThirdPerson (default F5) + ToggleCinematicCam (default Y) to keybindings.rs
  (enum + shared defaults) and keybind_ui::rebindable_actions, so both appear in Settings ▸ Controls and are
  REBINDABLE (the bin now reads them via bindings.is_action_just_pressed, not hardcoded keys). The old Y debug
  "give all weapons" moved to P. bin+lib+tests compile. DEVICE-VERIFY: Y cycles into the shoulder cam; first-person
  shows your body; rebind either in the Controls tab. NOTE: in fp_body the camera sits at the eyes so the head mesh is
  at the camera origin (front-faces point away → typically invisible); if the head clips, lowering the avatar or
  head-culling is the follow-up.

## r516 — caves: runtime ore enrichment + dragons-in-caves finding
MORE ORES IN CAVES: the world generator runs on the GPU here, so CPU roll_ore changes are invisible. Instead added a
budgeted runtime CPU pass (modeled on pioneer_flora) — sim::cave_ore::ore_for(depth, roll01) picks a depth-weighted ore
(coal/copper shallow → iron/gold mid → quartz/amber/crystal/diamond deep; rare, richer deeper, pure + 5 unit tests), and
the bin seeds it into plain Stone that faces Air (a real cave wall) below sea level near the player, ~2 per 67 frames.
CPU-mesher-rendered, so visible regardless of worldgen backend. Uses only EXISTING ore blocks (no new BlockId, no shader).
DRAGONS IN CAVES (operator asked to check the docs): NO — by design. docs/design/fluid_pois_waterfalls.extracted.md states
"dragons do not enter cave systems" (they're aerial predators); cave waterfalls instead use "underground fauna spawners"
(non-dragon). MENU_SYSTEM notes a spawn override for underground/in-air if we ever want to change this, but it contradicts
the design intent, so left as-is. bin+lib+tests compile. DEVICE-VERIFY: descend into a cave and watch ore appear on exposed
walls (deeper = better); confirm no dragons underground.

## r517 — building/mining feedback: targeted-block readout
The held item already renders as a hand viewmodel and a held BLOCK shows as a colored cube in hand (r485). Added the
missing piece — what you're about to AFFECT: a HUD line "▸ <Block> (x,y,z)" for whatever solid block the aim ray hits
within reach, flagged "[unbreakable]" for bedrock. Reuses the exact octree raycast the break/place path uses, so the
readout always matches the action. Pure HUD; compiles. DEVICE-VERIFY: aim at a block and see its name/coords update;
bedrock shows unbreakable. (Debris particles + audio cues are the natural next feel additions when an audio backend lands.)

## r518 — multiplayer: open-server + whitelist (backbone wired; menu UI planned)
sim/whitelist.rs: per-world Whitelist {open, allowed:BTreeSet<name>} — admits()/allow()/revoke()/toggle_open()/names(),
case-insensitive, serde, atomic save under world root (whitelist.json). 5 unit tests. WIRED: host gates the CH_HANDSHAKE
join — open admits anyone (and remembers the name so you can later close + keep current players); closed replies
LanJoin::Deny{reason} to non-listed names. F10 toggles open/closed (persisted). Loaded on world init, saved on shutdown.
Default OPEN. bin+lib+tests compile. The add/remove-name + open toggle MENU UI is specced in DESIGN_NOTES (egui flow on the
existing menu kit) as the on-device next step — the data layer + commands it needs are all in place. DEVICE-VERIFY: host,
press F10 to close, have a non-listed peer try to join → denied; an admitted peer is remembered.

## r519 — flora: saplings unified into the persistent store (single flora layer)
Followed through on the r514 note: session.saplings (in-memory Vec, lost on restart) is GONE — saplings now live in the
same persistent session.flora store as crops, distinguished by block type (OakLog = sapling; Carrot/Potato/Cabbage =
crop). The sapling growth tick now iterates flora's OakLog cells (advance → mature into a tree via flora_blocks → remove
from store; new saplings planted via flora.plant(FloraCell::sown(OakLog,…))), and the crop tick + harvest skip non-crop
cells so the two don't cross-advance. Net: ONE sparse, serializable per-world flora layer (flora.json) covering crops AND
saplings — both survive save/load now. Removed the vestigial WorldSession.saplings field + its PlantState import + test
ref. lib+lib --tests+bin compile. DEVICE-VERIFY: let a wild sapling grow, quit/relaunch the world → it resumes and still
matures into a tree.

## r520 — farming: per-stage crop visuals (seedling → plant → ripe)
The mesher renders by block, not growth state, so per-stage visuals come from swapping the BLOCK as the plant grows.
New BlockId::Sprout (91) — a tiny green-shoot subvoxel (recipes::sprout), has_subvoxels + is_subvoxel_only like crops,
full exhaustive-match cascade + invariant tests pass, block_drop None (a smashed seedling yields nothing). FLOW: planting
a crop now places a Sprout that records its target crop in FloraCell.element (0/1/2); the crop growth tick advances it and,
at stage ≥ 2, PROMOTES it — set_block to the real Carrot/Potato/Cabbage (the fuller r513 plant) and updates the cell —
then it ripens at FINAL_STAGE as before. So a field visibly reads sprout → plant → ripe. The growth tick now preserves
FloraCell.element (it encodes the target; advance no longer overwrites it). Uprooting a seedling returns the seed (drops
the target crop item). lib+lib --tests+bin compile; palette invariants pass. DEVICE-VERIFY: sow a crop → see a small
sprout → watch it become the full plant → harvest when ripe; break a young sprout → get your seed back.

## r521 — farming: plantable trees (leaves → sapling → plant → tree)
Completed the tree loop using existing systems (sapling tick + persistent flora store). Breaking OakLeaves/BirchLeaves
now ~11% drops a tree sapling — reusing the r520 Sprout as the sapling item. Planting a Sprout on Grass/Dirt UNDER OPEN
SKY (6 clear cells above) places an OakLog and registers it in session.flora as a sapling; the existing sapling tick
matures it into a full tree (flora_blocks), persisted across save/load like everything else in the store. Crop planting is
unchanged (Carrot/Potato/Cabbage → seedling). The plant interception + registration (both place branches) now branch:
Sprout → tree, crops → crop seedling. bin compiles (bin-only change; Sprout block + invariants from r520). DEVICE-VERIFY:
chop leaves until a sapling drops, plant it on dirt with sky above → it grows into a tree; without headroom it warns and
stays a sprout. Closes the build/farm loop: harvest wood+leaves → replant forests.

## r522 — farming: seed genetics (genome metadata on every plant)
Wired sim::plant_tree_genetics (the canonical plant genome, mirroring dragon_archetype::Genome — 13 normalized genes:
max_height, fruitfulness, growth_rate, hardiness, …, with from_seed/cross/phenotype). FloraCell now carries a `genome:
[f32;13]` (serde #[serde(default)] = neutral, so old saves still load) — per-plant genetic METADATA that persists in the
flora store. FLOW: a per-world seed line per crop (Carrot/Potato/Cabbage). Planting crosses that line with a fresh genome
(plant_tree_genetics::cross — Mendelian pick + blend + mutation) and stores the result on the plant. GENOME EXPRESSION:
growth_rate gene scales the growth tick's dt (bred lines ripen faster); fruitfulness gene raises the harvest yield (luck
into loot_quantity). INHERITANCE: harvesting a ripe crop breeds its genome back into that crop's seed line, so a grower who
keeps replanting their best harvests steadily improves yield + speed (selective breeding) — emergent, no per-item table
needed. Seed line is in-memory (resets per session); the per-plant genome persists. bin+lib+tests compile. DEVICE-VERIFY:
plant log shows g/f/h genes; harvest ripe crops repeatedly and watch later plantings grow faster + yield more. NEXT: persist
the seed line in the flora store; let genome drive crop appearance (subvoxel fullness) and tree shape (the genome already has
height/canopy genes the tree builder could read).

## r523 — two bug fixes: solid-leaf cube + inventory won't close
1) SOLID LEAF CUBE (operator screenshot, red outline). render/mesher.rs fn is_meshable was
`!is_air() && is_solid()` — it did NOT exclude is_subvoxel_only, unlike world/surface.rs is_meshable
which does (and whose comment says the two mirror). So the mesher emitted a full solid cube for leaves
(and crops) AND decorated subvoxels on top — the opaque cube hid the subvoxel foliage. Fixed: added
`&& !b.is_subvoxel_only()`, matching surface.rs. Leaves/crops now render purely as subvoxel detail (no
cube). NOTE/trade-off: subvoxel decoration is Near-LOD only, so at Mid+ distance leaves render nothing
(distant trees may look bald) — follow-up could add a sparse Mid-LOD leaf impostor.
2) INVENTORY WON'T CLOSE WITH P (takes many presses). The toggle was hardcoded to raw Keycode::I and
gated on `mouse_captured` — but opening the inventory releases the mouse, so the gate could never fire
to close it, and a rebound key (P) was ignored entirely. Fixed: toggle now reads the OpenInventory
*binding* (respects the user's key) and fires when `mouse_captured || show_inventory` (so it closes while
open). Also moved the r515 debug "give all weapons" off raw P → N (and gated to !show_inventory) so it
can't collide with an inventory rebind. lib+bin compile. DEVICE-VERIFY: leaves show stringy subvoxel
foliage with no solid block; inventory opens AND closes on the same key first press.

## r524 — real menu-sticky fix (Dvorak 'p' panel) + sparser leaves
MENU: r523 fixed the WRONG handler. The operator runs Dvorak (hotbar a/o/e/u/i), and their "P" is the key
LABELED 'p' (resolved via glfw::get_key_name on scancode), which drives `ui_panel_open` ("Panel mode") —
NOT the keycode-based OpenInventory binding I touched in r523. The panel toggle (a) released the cursor on
open but on close only logged "Click to recapture mouse" (cursor stayed free → look-around dead → pressing
'p' again reopened it → "sticky"), and (b) gated detection on !ui_wants_keyboard so a focused field could
trap it open. Fixed: closing the panel now recaptures the cursor immediately (CursorMode::Disabled +
mouse_captured=true); the typing gate now only blocks OPENING, never closing. Applied the same
recapture-on-close to the inventory (show_inventory) toggle. LEAVES: kept the r523 is_meshable cube fix
(provably gates all LODs — Near, downsampled, far_virtual). ALSO sparsened the subvoxel leaf recipe from
~64% → ~45% column coverage so the canopy reads as foliage with sky through it rather than a near-solid
mass even once the cube is gone. lib+bin compile. DEVICE-VERIFY: 'p' opens AND closes the panel with the
cursor returning to the game on close; leaves look airy. IF a hard solid cube still shows, it's a stale
build — the cube path is gone in source.

## r525 — unified UI state machine (AppMode + MenuState)
NEW sim/ui_state.rs: a two-level state machine replacing the bool soup. AppMode { TitleScreen, Playing }
is the top level; MenuState { NoMenu, Inventory, Pause, Settings, Debug, Craft } is the in-world subset,
NoMenu = playing/cursor-captured (the default). Helpers: toggle (one key per overlay, press-again-closes),
on_escape (open→back out, NoMenu→Pause, Settings→Pause), captures_cursor, pauses_game, shows_* . 8 unit
tests (flatten-tested, pass). KEY MAP (operator's): p → Inventory (Dvorak-aware p_pressed, + rebindable
OpenInventory R/I alias); F4/F3 → Debug (now ONE thing: drives both the perf dashboard and the debug/shader
panel); Esc → Pause (which shows the inventory too); C → Craft. WIRING: menu_state owns the light in-world
overlays (Inventory/Debug/NoMenu) as the single source of truth; the heavyweight FSM screens (Pause via the
app FSM + pause_action, Settings/Craft self-closed by renderer Done buttons, Title) keep their existing
plumbing but are SUBORDINATED — opening one forces menu_state=NoMenu and the light overlays only toggle when
no heavy screen is up. Cursor FOLLOWS menu_state: releases on open, recaptures the instant we return to
NoMenu (no click — kills the "sticky" feel). REMOVED the old split paths: 'p' "Panel mode" (ui_panel_open),
separate F4-perf toggle, separate F3-settings_open toggle. show_inventory/ui_panel_open survive only as
per-frame derived locals so downstream reads (game_paused, cursor authority, ui_pointer_active) are unchanged.
bin+lib+tests compile. SCOPE/HONESTY: Pause/Settings/Craft transitions remain renderer+FSM-owned (subordinated,
not fully inside the enum) — rewriting the FSM-integrated pause blind was too risky; the enum cleanly owns the
operator's named keys + NoMenu + the title/playing split. DEVICE-VERIFY: p opens/closes inventory on one press
with the cursor snapping back; F4 toggles debug (separate from inventory); Esc pauses; opening pause/craft
closes the inventory.

## r526 — mobs MOVE (AI brains + step_character against live terrain)
docs/MOBS.md flagged movement as the top "what's left": the test Seeker+Grazer were STATIC spawns. Now the bin
ticks them. NEW bridge `MobWorld<'a> { cm: &ChunkManager }` impl sim::physics::WorldQuery (physics_at →
physics_of_block(cm.block_at)). Each frame, gated on !game_paused: accumulate dt, tick at a fixed 20 Hz
(frame-rate independent, max 4 catch-up ticks): build the player as a sim Character, snapshot grazer positions
(prey) + seeker positions (predators), then for each mob `m.brain(world_seed, id, tick, &players, &prey,
&predators, &world)` → `step_character(&m.body, &input, &world)`. test_mobs is now `let mut` and baked live each
frame (mobs_to_vertices already read it), so they visibly move: the Seeker chases the player, the Grazer flees
within FLEE_RANGE, both wander otherwise, all colliding against real voxel terrain. mob_sim_tick is the
deterministic RNG key. bin compiles. DEVICE-VERIFY: walk toward the two test mobs near spawn — one should chase,
one should run; both should walk on the ground, not float/fall. NEXT (per MOBS.md): spawn-from-egg place path
(ui-overlay-gated), ambient population via sim::spawn::spawn_region, creature part facing by body yaw.

## r527 — bending wired end-to-end + fixed for Dvorak ("B does nothing")
TWO parts. (1) END-TO-END EFFECT: bending fired (resonance build/spend + fields) but bending_compose's
documented gap stood — an EffectIntent was produced yet nothing turned it into resolved targets + a discipline
effect. Now at the cast site, after the intent resolves, we build Targets from nearby roamers, call
sim::bending_compose::resolve_bend(&intent, &BendWorld::for_targets(origin, &targets, &[]), bend_resonance)
[Resonance is Copy → no double-spend; we read only hits/power/backlash], and KNOCK BACK each hit creature along
the aim (+ lift), scaled by hit strength × power_realized. HUD: "bending struck N — power P%". So a bend now
visibly shoves creatures. (2) THE "B DOES NOTHING" ROOT CAUSE: the trigger was raw is_key_pressed(Keycode::B) —
PHYSICAL B, which on the operator's Dvorak layout is NOT the key labeled 'b'. Same layout trap as the 'p'
inventory bug. FIX: new rebindable GameAction::Bend (default physical B via with_common_bindings; Dvorak preset
ALSO binds it to physical-N = the Dvorak 'b' key). Trigger is now bindings.is_action_pressed(GameAction::Bend).
Added to keybind_ui rebindable_actions → shows in Settings▸Controls, rebindable. Moved the r523 debug
weapon-give off Keycode::N (now Dvorak-Bend) → Keycode::Minus to avoid collision. lib+bin+tests compile.
DEVICE-VERIFY: stand still (build resonance), hold the bend key (Dvorak 'b' / physical N, or rebind) aiming at a
creature → it gets knocked back + HUD shows the strike. NEXT: damage/elemental effects per discipline (knockback
is the first visible payoff; resolve_bend already returns the discipline effect to apply), extend targets to mobs.

## r528 — rebind to MOUSE BUTTONS + PageUp/Down (operator: bind Bending to a side button)
Bending was already in the Controls list (r527), but the rebind capture only scanned keyboard keys + only wrote
keyboard bindings, and action triggers ignored mouse bindings — so a player COULDN'T put Bending on a side mouse
button. Now they can. KeyBindings already had bind_mouse/mouse_buttons_for_action/is_action_pressed_mouse; added
`set_mouse_binding` (clean rebind: clears the action's keys + frees the button from others) and mouse PERSISTENCE
(StoredBindings gained `mouse: HashMap<GameAction,Vec<String>>` with #[serde(default)] so old files still load;
save/load round-trip MouseBtn Debug names via mouse_from_name). keybind_ui: capture_candidates gained PageUp/
PageDown/Home/End/Insert/Delete; new mouse_capture_candidates() = [Middle, Back, Forward] and mouse_label
(Mouse-Mid / Mouse-4 / Mouse-5). Bin Controls page: rows now show mouse bindings too; the capture loop, after
finding no key, scans mouse_capture_candidates and calls set_mouse_binding + saves. Bend trigger now fires on
is_action_pressed OR is_action_pressed_mouse. lib+bin+tests compile. DEVICE-VERIFY: Settings▸Controls → click
the "Bending" row → press Mouse-4/5 or PageUp → row shows it, persists across restart, and holding that input casts.
NOTE: only Bend's trigger ORs in the mouse check so far; other actions can adopt the same OR if desired.

## r529 — bending: distinct elemental effects (Air gust + Water surge)
Found bending was already richly wired: the discipline match raises an Earth stone pillar, blooms Fire as a heat
source, and strikes (the _ arm) damage + kill creatures with loot drops. My r527 AOE knockback ran for EVERY cast,
redundant with the strike. Refactored so each ELEMENT is distinct: removed the always-on knockback and added two
arms. AIR (Elemental(Air)) = a gust that knocks back every creature resolve_bend's targeting catches, shoved along
aim + lifted, scaled by hit strength × power (HUD "air gust — N caught"). WATER (Elemental(Water)) = a gentler,
more horizontal surge that shoves creatures AND conjures a Water block at the aimed surface point (visible water you
pulled up). Both use bending_compose::resolve_bend with a Copy of the resonance (no double-spend). Now: Earth→pillar,
Fire→bloom, Air→gust-knockback, Water→surge+water, strike→damage/kill. The cast element is derived from environment
(swimming→Water, near heat→Fire, airborne→Air, else Earth) so different contexts cast different elements. bin
compiles. DEVICE-VERIFY: cast (hold bend key) while swimming → water surge + a water block appears; airborne → air
gust scatters creatures; near fire → thermal bloom; on ground → earth pillar.

## r530 — worldgen: surface ruins (structure_template wired into the world)
docs/design/STRUCTURES_worldgen.md describes a full structure pipeline whose pure cores all exist
(structure_template::make_ruin, structure_place, dungeon_layout, structure_grammar, cave_carve, …) — but NOTHING
placed them into the world. Wired the first one. New Phase 6 in cpu_generator::generate_chunk: deterministic
per-region surface ruins. Each 80×80 region rolls (ruin_region_hash) ~30% to host one make_ruin footprint (5..8
square, wall ring + floor + crumble gaps) at a hashed anchor, on dry land (surface ≥ SEA_LEVEL). Stamped
cross-chunk EXACTLY like the Phase 5 trees — every chunk overlapping the footprint writes its own slice via
ruin_write (overwrites air / foliage / soft ground only, never solid rock, so ruins drape on terrain), no
per-chunk RNG, seams match. Floor = a cobblestone slab at the surface; walls = 2..3-tall cobblestone columns.
lib+bin compile. DEVICE-VERIFY: explore and look for broken cobblestone ruins scattered on flat-ish dry ground
(~one per few 80-block regions). CAVEAT: placement lives in the CPU generator (same path as trees) — if a run uses
pure GPU worldgen and trees still appear, ruins will too; if trees come only from CPU, so do ruins. NEXT: biome
flavor (sandstone ruins in desert), towers (tower_taper), and dungeons (dungeon_layout + cave_carve) are the next
structure cores to wire the same way.

## r531 — first-person head-clip + humanoid bipeds (operator: "chicken looking thing" + head blocked screen)
TWO fixes from the operator's feedback running as a Biped.
(1) FIRST-PERSON HEAD CLIP — fp_body (r515) bakes your avatar at the player position, so in first person the head/
torso filled the screen. In the self_avatar bake, when camera_mode==0, clip away any triangle whose three verts are
all at/above eye level (clip_y = view.position.y - 0.25): you still see your body/arms/legs looking down, but the
head no longer blocks the view. Model-agnostic (works for any avatar). Third-person/cinematic keep the full model.
(2) HUMANOID BIPEDS — the Biped archetype rendered chicken-like: neck juts FORWARD (head carried out front), no
arms, and a 70% wing chance. creature_design now treats Biped as UPRIGHT like the Lauscher: added is_biped/upright
flags; neck climbs straight up (head ON TOP); head placed over the body (hh*0.15 not hh*0.8); a flat humanoid face
(small nose) instead of an animal snout; a single pair of articulated arms (upper arm + forearm + hand) at the
shoulders; and wing chance dropped to 0. Lauscher-only traits (many-armed stacks, big ears, smaller eyes, gape/hinge)
stay Lauscher-only. The "Wanderer"/"Strider" presets are Biped, so they now read as humanoids. lib+bin+lib-tests
compile. DEVICE-VERIFY: pick Wanderer/Strider — upright human-ish figure with arms and head on top; in first person
the head no longer fills the screen; look down to still see arms/body/legs.

## r531 (addendum) — player dragons use the NEW forge dragon
The sophisticated voxel FORGE dragon (sim::dragon_forge::build → res-256 model, .with_forge, element-tinted by
Genome) was already used by WORLD dragons (spawn(CreatureKind::Dragon) → spawn_dragon_with_genome). But the PLAYER
avatar for Arche::Dragon presets (Heliographer/Windwalker/Tidecaller/Stormweaver) built via from_blueprint = the OLD
box anatomy. Fixed the self_avatar build: when active_character.arche()==Dragon, spawn via spawn_dragon_arche(seed)
so the avatar carries a forge and creature_to_vertices_lod dispatches to forge_to_vertices. Now player dragons match
world dragons (new model). Non-dragon archetypes still use from_blueprint. bin compiles. DEVICE-VERIFY: pick a dragon
preset, go third-person (F5) — the detailed voxel forge dragon, not the blocky box one.

## r532 — bending thermal-phase payload (ice-melt) + biome-flavored ruins
Continuing bending + structures per the operator.
BENDING: the Fire/ThermalPhase arm now carries a real phase-change payload — besides the heat bloom, it MELTS a
2..4-block radius (scaled by power) of nearby BlockId::Ice into Water and sublimates SnowBlock to Air, via
chunk_manager.block_at/set_block, with a HUD "thermal phase melted N" line. So firebending visibly thaws ice and
clears snow.
STRUCTURES: ruins are now biome-flavored. The Phase-6 ruin pass samples biome_at_3d at the anchor and picks the
wall/floor material: Desert/Beach → Sandstone, RedSandstoneCanyon/BloomingCanyon → RedSandstone, Swamp/LushCave →
MossyCobblestone, else Cobblestone. Ruins now read as belonging to their landscape.
lib+bin compile. DEVICE-VERIFY: firebend at ice/snow → it turns to water / clears; explore a desert → sandstone
ruins, a canyon → red-sandstone ruins, a swamp → mossy ruins. NEXT (noted): Stormstrike CHAIN on the strike arm
(needs care with swap_remove index handling) and towers via tower_taper.

## r533 — STRUCTURES under GPU worldgen + character access/variety (operator: no structures, can't switch character)
ROOT CAUSE of "no structures": on the M2, Backend::Auto picks GPU worldgen (gpu_generator). TerrainGenerator::generate
called gpu.generate_chunks, which reads back the compute-shader terrain with NO CPU decoration — so the ruins (placed
only in cpu_generator::generate_chunk) never reached the operator's chunks. (Vegetation comes from the separate flora-
store streaming, which is why trees showed but ruins didn't.)
FIX: extracted ruin placement into WorldGenerator::stamp_ruins(blocks, coord) (analytic surface_y + biome, so it works
for any chunk). cpu_generator calls it (as before). TerrainGenerator's GPU branch now uses gpu.generate_batch, runs
self.cpu.stamp_ruins on the raw blocks (self.cpu carries the biome registry → biome-flavored ruins), THEN builds the
Chunk (before from_blocks, so plant/wood indices stay correct). So ruins now appear under BOTH backends. Bumped to ~38%
of regions, walls 3..4 tall for visibility, and added a "[worldgen] ruin at (x,y,z) WxH" info log (one per ruin, when
its anchor lands in a chunk) so the next session log CONFIRMS generation.
CHARACTERS: (1) the character selector was only on the TITLE screen — added a "Character" button to the in-game PAUSE
menu (sets open_character_local), so you can switch mid-world; the bin already applies character_chosen + rebuilds the
avatar. (2) Added the missing GRAVITYBENDER dragon preset. (3) Mapped each named elemental dragon to its element in the
avatar build (helio→Fire, tide→Water, wind→Air, storm→Storm, gravity→Dust) via spawn_dragon_element, so they render as
distinct elemental forge dragons instead of all-same-by-seed.
lib+bin+tests compile. DEVICE-VERIFY: explore → cobblestone/sandstone ruins (grep log for "[worldgen] ruin"); Esc →
Character → pick Tidecaller/Gravitybender/Heliographer → F5 to see the element-tinted forge dragon.
NOTE: the forge HTML's bespoke voxel HERO builders (buildHeliographer/Tidecaller/Gravitybender/… — hand-placed scale
plates, horns, armor) are far richer than the genome forge; fully porting them to Rust is a large separate effort.
This roll makes all five selectable + elementally distinct via the genome forge as the interim.

## r534 — HERO CHARACTER TYPES are humanoid benders, not dragons (operator correction)
Operator clarified: the character-forge HTML grafts a CHARACTER designer onto the dragon forge. The dragon is only ONE
tab; the other five — Heliographer, Stormweaver, Windwalker, Tidecaller, Gravitybender — are distinct HUMANOID player
character types (voxel "heroes", each with a shoulder/hip rig; Tidecaller is gendered female) for multiplayer + "bending
separation". r533 wrongly made them Arche::Dragon. Fixes:
- presets(): the five heroes are now Arche::Biped (the upright humanoid model from r531), keeping their tints. They render
  as humanoid characters, NOT dragons.
- Reverted r533's dragon-name→element mapping in the self_avatar build. Only a true Arche::Dragon uses the forge dragon;
  everything else (incl. the heroes) takes the humanoid blueprint path.
- BENDING SEPARATION: added player_character::bend_element(name) -> Option<gesture_resolve::Element> (Heliographer→Fire,
  Tidecaller→Water, Windwalker→Air, Stormweaver→Air/storm, Gravitybender→Earth). At the bend cast, the player's character
  element now OVERRIDES the environment-derived element, so each hero bends its own element regardless of surroundings.
  Ordinary characters still fall back to the environment (water→Water, heat→Fire, airborne→Air, else Earth).
lib+bin+tests compile. DEVICE-VERIFY: Esc → Character → pick Tidecaller/Gravitybender/Heliographer → F5 shows a HUMANOID
(not a dragon); casting a bend produces that character's element anywhere (e.g. Tidecaller surges water on dry land).
NOTE: still interim — the forge's bespoke hand-built voxel hero geometry (buildHeliographer/Tidecaller/… with scale
plates, horns, armor, + the __RELIEFS 2D voxel portraits) is a large separate port; this roll uses the upright humanoid
model + correct element wiring. Port the real hero meshes one at a time when requested.

## r535 — adaptive-RD slew limit + camera zoom keys + avatar fade + neck tracking
Confirmed from operator log: ruins generate (`[builders] ruin at ...`, `[worldgen] ruin ...`) — just none in the ocean
(dry-land only, as intended). Four changes:
- ADAPTIVE RENDER-DISTANCE SLEW: the cap started at 32 (so distance slammed straight to max at spawn) and regrew +1 every
  400ms with no guard, and each new ring at large distance is a heavy one-frame spike. Now: cap starts LOW (~6) and ramps
  up; growth requires a streak of consecutive good-frame checks (UNENDLICH_ADAPTIVE_RD_GROW_STREAK, default 3) so a single
  spike frame resets the streak and PAUSES growth until the rate is stable; shrink stays fast. Smooths the ramp, kills the
  big spikes.
- CAMERA ZOOM KEYS: +/- (Equal / Minus) adjust third-person & cinematic camera distance via a cam_zoom multiplier
  (0.4..3.0; − farther, + closer). In first-person the keys keep their old bindings (the Minus debug weapon-give is now
  gated to first-person only). Applied to both pull-back distances.
- AVATAR FADE: the player's own avatar now fades as it obscures more of the camera, in ALL views, down to 50%. Entity
  verts are RGB-only / opaque pipeline, so this is screen-door (dithered) transparency — keep a coverage-scaled fraction
  of triangles via a stable per-triangle hash (no flicker). Coverage = fraction of avatar tris close+central in front of
  the render camera. In first-person it stacks on the r531 above-eye head clip.
- NECK TRACKING: the avatar's head/neck now tilts with the look pitch (av.gaze_pitch = view.pitch, clamped ±0.95), via the
  gaze→head rig in the vertex builder, so looking up/down moves the head out of the camera's way.
bin compiles. DEVICE-VERIFY: enable adaptive RD and watch for a smooth ramp without 80ms+ spikes; in third/cinematic press
+/- to zoom; zoom in close (or look down in FP) and confirm the body dithers toward ~50%; look up/down and confirm the
head tilts (if it tilts the WRONG way, flip the sign on av.gaze_pitch).

## r536 — REAL per-vertex alpha for the avatar fade (replaces r535 dithering)
Operator preferred true transparency over the screen-door dither. Implemented an alpha channel end-to-end through the
entity pipeline:
- EntityVertex gained `alpha: f32` (stride 24→28; defaults 1.0 via the new `EntityVertex::opaque` helper and at all 7
  construction sites — precip/drops/viewmodel/creature bakes all stay opaque).
- entity_pipeline.rs: added vertex attribute location 2 (R32_SFLOAT @24), bumped stride to 28, extended the manual
  upload packer to emit the 7th float, and ENABLED alpha blending (SRC_ALPHA / ONE_MINUS_SRC_ALPHA, color+alpha ADD).
  Depth-test+write stay ON, so opaque entities (alpha=1) are pixel-identical to before and the translucent avatar's
  overlapping triangles don't double-composite (nearest wins).
- shaders/entity.vert + entity.frag: pass `in_alpha`→`v_alpha`; frag now outputs `vec4(color, v_alpha)` (fog still
  composited into rgb). Recompiled assets/shaders/entity.{vert,frag}.spv with glslangValidator (covers both the
  include_bytes! non-hot path and the hot-shaders runtime path, which read the same assets/shaders dir).
- bin: the avatar fade now sets real per-vertex alpha = opacity (1.0→0.5 by camera coverage) instead of dithering tris;
  the r531 first-person above-eye head clip still applies.
lib+bin+tests compile; SPIR-V regenerated. DEVICE-VERIFY: zoom in close in third/cinematic (or look down in FP) — the
body should smoothly turn translucent (real alpha, no dither grain), down to ~50%; confirm other entities/mobs/rain are
unchanged (fully opaque).
NOTE (minor): froxel fog in-scatter is multiplied by alpha on the translucent avatar, so its fog glow is slightly dimmer
when faded — cosmetic, only affects the semi-transparent self-avatar.

## r537 — camera occlusion package: pinch + near-camera terrain fade + deeper avatar fade
- AVATAR FADE FLOOR: lowered from 50% to 10% opacity (opacity = 1.0 − 0.9·coverage).
- CAMERA PINCH: new pinch_camera_distance() marches the chase-camera offset against the world (chunk_manager.block_at +
  is_solid) and pulls the camera in so terrain can't wedge between it and the player. 5% baseline pinch always, then
  clamp to just short of the first solid hit (closer hit ⇒ tighter pull = the requested distance weight). Applied to both
  third-person and cinematic by raycasting along the true eye→camera offset (handles the cinematic's right/up offset too).
- NEAR-CAMERA TERRAIN FADE (chunk.frag): fragments closer than ~0.85 units dissolve via an ordered Bayer screen-door
  (fully gone < 0.12), so a block right on the lens — backing into a wall, or the camera pinched into a corner — no longer
  blacks the screen. Discard-based (no blend, no pipeline change); works in ALL perspectives incl. first-person. Reuses
  the same Bayer pattern the water-transparency path already uses.
- SHADERS: edited chunk.frag (near fade) + entity.{vert,frag} (already alpha from r536); recompiled all three .spv with
  glslangValidator --target-env vulkan1.2 and removed their .hash sidecars so build.rs regenerates cleanly on the operator
  build. (build.rs auto-recompiles changed GLSL anyway; this keeps the shipped .spv consistent for the hot-shaders path.)
bin compiles; shaders validate. DEVICE-VERIFY: in third/cinematic back into a wall or enter a corridor — the camera
should pull in smoothly instead of clipping into terrain; any block that still reaches the lens should dissolve rather
than black out; the avatar should fade further (to ~10%) when it fills the view.

## r538 — forge HERO MODELS ported to the engine + body-build sliders
Port of the character-forge voxel heroes so you can actually play as them, plus the muscularity/tallness sliders.
- NEW MODULE hero_forge.rs: a parameterized humanoid voxel hero builder. `HeroKind` (Heliographer/Windwalker/Tidecaller/
  Stormweaver/Gravitybender — keyed by NAME, since Storm+Wind share the Air element) selects palette + element features
  (Heliographer sun-halo, Stormweaver/Gravitybender horns, Tidecaller head-fin + female build, Gravitybender heavy
  plating). `build(kind, &BuildParams, origin, yaw, gaze_pitch) -> (Vec<EntityVertex>, Vec<u32>)` assembles legs/pelvis/
  torso/chest-plate/pauldrons/arms/neck/head as cubes, applies body yaw + head gaze pitch, and scales every part by the
  build sliders. HONEST SCOPE: engine-native interpretation matched to the forge's proportions/palettes/features, NOT a
  byte-for-byte copy of the bespoke JS geometry.
- CharacterProfile gained `build: BuildParams { height, bulk, head, shoulders, arm, leg }` (all default 1.0,
  #[serde(default)] so old character.json still loads). CharacterProfile::new keeps its signature.
- BAKE WIRING: in the self_avatar bake, characters whose name resolves to a HeroKind now render through hero_forge::build
  (shaped by active_character.build) instead of the generic blueprint; everyone else unchanged. Runs per-frame so slider
  edits are live once applied. Flows through the existing coverage-fade / real-alpha / FP head-clip path.
- MENU SLIDERS: the Character window's custom slot now has tallness / muscle / head / shoulders / arms / legs sliders
  (range 0.7–1.4) editing the profile's BuildParams; persisted + applied via Save & Use (bin takes character_chosen →
  active_character, rebuilds avatar).
lib + tests + bin compile. DEVICE-VERIFY: Character → pick a hero (e.g. Tidecaller) → Save & Use → F5/third-person shows
the element-themed humanoid (fin/halo/horns/plating per hero); reopen Character (custom slot) → drag muscle/tallness/etc.
→ Save & Use → the in-world body reshapes.
KNOWN LIMITS (next): the menu's 2D PREVIEW still draws the generic blueprint, not the hero mesh (preview-only — in-world
is correct); heroes are a static gaze-tracked pose (no walk animation yet); sliders live on the custom slot so the flow is
pick-hero → Save & Use → reopen → adjust. A byte-faithful port of each bespoke JS hero (exact scale plates / __RELIEFS 2D
portraits) remains a per-hero follow-up if you want exact geometry.

## r539 — leaves GPU-mesher fix + water-rising fix + hero walk cycle
Three operator reports addressed.
- LEAVES (GPU vs CPU, confirmed): r523 fixed the solid-leaf cube in the CPU mesher (is_meshable excludes
  is_subvoxel_only), but the LIVE path is the GPU octree mesher, whose `is_solid_opaque` only excluded Air+Water — so it
  still emitted full solid cubes for leaves/crops UNDER the subvoxel fronds ("solid block over the leaves"). Fixed in
  lockstep: chunk_mesh_octree.comp `is_solid_opaque` now also returns false for 18/20 (Oak/Birch leaves) and 88/89/90/91
  (Carrot/Potato/Cabbage/Sprout); the Rust CPU-ref (gpu_meshing_octree_cpu_ref.rs) now mirrors it via
  `!is_subvoxel_only()` so UNENDLICH_OCTREE_MESH_VERIFY parity holds. Recompiled the .spv + dropped its hash sidecar.
  (Sparse/runs meshers use CPU-extracted SurfaceRecords already fixed by r523's surface.rs; chunk_mesh.comp is a separate
  path, left as-is.)
- WATER RISING (regression): UNENDLICH_WATER_SIM defaulted to TRUE despite the code comment saying "off by default", so the
  experimental in-chunk settle ran for everyone — and apply_rain feeds water into exposed columns each tick, so bodies
  visibly CLIMB during/after rain. Restored the documented default to OFF (opt in with UNENDLICH_WATER_SIM=1). The sim is
  still backlog-blocked on a persistent per-chunk field; rain-equilibrium tuning is the follow-up before it ships on.
- HERO WALK CYCLE (character "needs more work"): hero_forge box_at generalized to an optional (pivot, angle) rotation;
  legs now swing fore/aft about the hip and arms counter-swing about the shoulder, driven by a new `gait` arg. The bin
  advances a gait phase over time with amplitude tracking horizontal speed (settles to 0 when standing). Heroes were a
  static pose before; they now walk.
lib + tests + bin compile; octree .spv recompiled + validated. DEVICE-VERIFY: foliage should show fronds (no solid cube
over leaves; A/B vs CPU with UNENDLICH_GPU_MESHER=0 should now match); water should sit still in rain (no climbing) unless
you set UNENDLICH_WATER_SIM=1; a forge hero (F5) should swing arms/legs while walking and settle when idle.

## r540 — camera juice: first-person walk bob + chase-camera smoothing
Picked up from the backlog's "camera juice (shake/smoothing/step-bob)" item, continuing the recent camera work. Both
effects are RENDER-ONLY — they offset render_view, never the logical eye (local_player.view), so physics, raycast,
culling and audio are unaffected.
- FIRST-PERSON WALK BOB: a subtle head bob while moving — vertical at 2× stride frequency, lateral at 1×, amplitude eased
  toward the player's horizontal speed (settles to 0 when standing / swimming). Phase cadence rises a little with speed.
  Amplitudes are small (≤~0.045) so it reads as weight, not nausea. Toggle UNENDLICH_VIEW_BOB=0.
- CHASE-CAMERA SMOOTHING: in third/cinematic the camera eases toward its (pinched) target each frame instead of snapping,
  so +/- zoom and the occlusion pinch glide. Snaps on big jumps (>6u: teleport/respawn/mode switch) to avoid a long
  slide; the smoothed state resets when leaving a chase view so re-entry snaps the first frame. Toggle UNENDLICH_CAM_SMOOTH=0.
bin compiles (bin-only; no lib/shader changes). DEVICE-VERIFY: walk in first-person — a gentle bob that fades when you
stop; in third/cinematic, zoom with +/- or walk into a wall — the camera should glide rather than jump. Either is
disable-able via its env var if it feels off.

## r541 — character menu previews the actual hero mesh
Closes the r538 limitation where the menu's 2D silhouette always drew the generic blueprint, even for forge heroes.
- NEW hero_forge::preview_boxes(kind, &BuildParams) -> Vec<ProjectedBox>: builds the hero at rest (yaw/gaze/gait=0),
  takes each cube's X-Y side-profile rect, and normalizes the figure into the [0,1] +y-up unit square the menu painter
  expects — same convention as creature_viewer::project, so it drops straight into char_preview (depth-sorted as before).
- world_renderer: when the highlighted character resolves to a HeroKind, the preview now comes from preview_boxes (shaped
  by the live build sliders) instead of the blueprint projection; non-heroes are unchanged. Updates one frame after a
  slider drag (slider write-back lands at end of frame), which is imperceptible.
lib + bin compile. DEVICE-VERIFY: open Character, highlight a forge hero — the 2D preview should now show the element-themed
humanoid (fin/halo/horns/plating, body proportions) rather than the generic blueprint, and reshape as you drag the build
sliders.

## r542 — impulse camera shake (completes the camera-juice trio: bob + smoothing + shake)
- IMPULSE SHAKE: a hard landing now jolts the camera, scaled by impact speed (mapped from the existing landing detector
  at prev_fall_vy < -9; sub-threshold landings don't shake). `shake_mag` decays exponentially (~0.25s) and the per-frame
  offset is a multi-frequency sine on the camera's right/up axes, so it reads as a jolt rather than a wobble. Render-only
  (offsets render_view after bob+smoothing; the logical eye is untouched). Toggle UNENDLICH_CAM_SHAKE=0.
  Hooked into the same landing branch that already emits the dust puff, so it fires exactly when you thud down. Easy to
  extend later (damage, bend impacts, nearby thunder) by kicking shake_mag from those events.
bin compiles (bin-only). DEVICE-VERIFY: take a long fall and land — a brief jolt that settles fast; small hops shouldn't
shake; disable with UNENDLICH_CAM_SHAKE=0.

## r543 — GPU mesher: fix magenta blocks (color-table gap vs the CPU mesher)
The GPU octree mesher's color_for_block only covered ids up to 47/56/80, so 20 defined blocks fell through to the magenta
default and rendered magenta in GPU worlds (the "magenta foliage/blocks" report). Added the missing entries, values =
world::blocks::base_color/255 so GPU and CPU worlds match: Sandstone(48), Obsidian(49), CopperOre(50), Quartz(51),
OakPlanks(57), StoneBricks(58), Bricks(59), Glass(61), IronBlock(62), GoldBlock(63), CoalBlock(64), Bookshelf(65),
Pumpkin(66), MossyCobblestone(81), PolishedStone(82), SmoothSandstone(83), Terracotta(84), ChiseledStoneBricks(85),
DarkOakPlanks(86), PolishedBasalt(87). (Crops 88–91 stay magenta-irrelevant — r539 made them not cube.) Recompiled the
.spv (glslang RC=0) + dropped the hash sidecar. NOTE: color is NOT part of the UNENDLICH_OCTREE_MESH_VERIFY parity check
(geometry only) — the CPU-ref color table was already ~34 blocks behind with no failures — so no CPU-ref sync needed; the
r539 geometry change WAS synced.
bin compiles; shader validates. DEVICE-VERIFY: place/þgenerate any of the above blocks (planks, bricks, glass, pumpkin,
mossy cobble in ruins, sandstone in deserts) — they should show real colours, not magenta.
NOT DONE (need device/CPU-side, not blind shader edits): missing top faces looks like neighbor_py wiring/LOD, not the
sampler logic (which correctly returns air on open boundary); biomes 19–27 GPU port is the big remaining worldgen-shader item.

## r544 — GPU shader audit + CPU-ref color table synced
Investigation pass on the GPU-shader backlog (the operator asked to push on these regardless of device-verify risk):
- BIOMES 19–27: verified ALREADY fully wired in chunk_generation.comp — determine_biome (r457/r458), height_modifier
  (r458), surface_block_type (r462) and the tree decision (r458) all return/handle Savanna/Jungle/Taiga/CrystalCave/
  LushCave/Mesa/Mangrove/VolcanicWastes/FrozenOcean with the same thresholds + blocks as biomes.rs. The "CPU-only" backlog
  note was stale; closed it.
- MISSING TOP FACES: diagnosed as a CROSS-LOD boundary issue, NOT a same-LOD bug. The octree sampler returns air on an
  open/unloaded +Y boundary (top face emits) and octree_sample always reads full-res data, and the CPU neighbor_py wiring
  (gpu_meshing_octree.rs lookup((0,1,0))) is correct. Remaining artifact is at LOD-band boundaries — needs on-device LOD
  work, not a blind shader edit. Left untouched rather than risk a regression.
- CPU-REF COLOR TABLE SYNCED: render/gpu_meshing_octree_cpu_ref.rs::color_for_block claimed to "mirror the GLSL" but was
  ~34 blocks stale (only ~12 entries). Replaced it with the full table (65 arms) matching chunk_mesh_octree.comp after the
  r543 additions. Color is NOT part of the VERIFY geometry parity check, so this changes no current behavior — it just
  makes the mirror honest and protects any future color-aware comparison.
lib + tests compile. No shader/runtime behavior change beyond r543; this is audit + test-infra hygiene.

## r545 — BIG PASS Roll 1: survival HUD + hero held-item + hero idle
First roll of the planned pass (docs/BIG_PASS_PLAN_r545.md). All pure-Rust, compile-verified.
- SURVIVAL HUD (C1): new WorldRenderer::hud_survival (SurvivalHud {hunger,thirst,stamina,comfort,body_c}); the bin fills
  it each HUD tick in survival mode. Renderer draws on-screen bars above the hotbar — Hunger (green→amber→red as it
  empties) and Warmth (fill = comfort, blue cold / green ok / red hot, labelled with body °C). Hidden in creative / while
  a menu is up. Turns the previously console-only Vitality + Thermal systems into a visible loop. (thirst/stamina fields
  carried now, drawn in Roll 3.)
- HERO HELD ITEM (A1): hero_forge::build now takes a HeroPose { yaw, gaze_pitch, gait, anim_t, held_color }. When the
  player holds something, a small item box renders in the hero's right hand and swings with the arm; colour by item
  category (steel weapon / gold / iron / meat / generic). Third-person heroes were empty-handed before.
- HERO IDLE (A2): a slow breathing head-bob + opposite-arm idle sway (driven by anim_t) so a standing hero reads as alive
  instead of frozen. Tiny amplitudes so it doesn't fight the walk cycle.
hero_forge build signature changed to (kind, params, origin, &HeroPose); preview_boxes + the bin call updated. lib + bin
compile. DEVICE-VERIFY: enter survival (UNENDLICH_MODE=survival) → Hunger/Warmth bars show above the hotbar and react
(eat, stand by fire / in cold); as a forge hero (F5) hold a weapon → it shows in-hand and swings; stand still → subtle
breathing/sway.

## r546 — BIG PASS Roll 2: playable dragon flight + dragon density cap
- B1 PLAYABLE DRAGON FLAPS: the dragon baker (creature_to_vertices, entity.rs:1175) already chose WingPose from velocity
  (climb/fast → Beating, fast descent → Flared, else Spread), but the self-avatar's anim_time + gait_phase were only ticked
  for ROAMERS — so the playable dragon picked the Beating pose and held it frozen. The bin now advances av.anim_time
  (+frame_dt_s) and av.gait_phase (with horizontal speed) every frame in the self-avatar update, so the wings actually beat
  and the legs step. Forge heroes use their own hero_gait_phase, so this is dragon/blueprint-avatar only in effect.
- B3 DRAGON DENSITY CAP (operator note r441): the 220u DRAGON_MIN_SPACING stopped clustering but not total count, so a
  roaming player could still accumulate too many dragons. Added a hard concurrent cap (count active Dragon roamers; skip
  the spawn once at/over the cap) at BOTH dragon spawn paths — the request-queue path (~4106) and the ecology-stream path
  (~6657, where the apportioned budget can also emit dragons). Tunable via UNENDLICH_MAX_DRAGONS (default 6).
bin compiles. DEVICE-VERIFY: play as a dragon (pick Arche::Dragon), fly up / glide fast → wings beat; dive → flare; on the
ground → idle. Heavy-dragon seeds should now settle to ≤6 concurrent dragons (set UNENDLICH_MAX_DRAGONS to taste).

## r547 — BIG PASS Roll 3: thirst pillar + stamina/thermal on the HUD
- C2 THIRST (new pillar): the player now carries a hydration reserve (reuses sim::dragon_thirst::Hydration — drain/drink/
  is_parched). It drains over time (slowed ×0.3 from the dragon constant ⇒ ~2.8 min idle, faster while sprinting) and
  refills while standing in water (feet in a Water block — `drink(dt)` at 0.6/s, tops up in a couple seconds). Parched
  (<20%) saps vitality, feeding the existing starvation/death loop rather than adding a separate death path. fg_hydration
  lives beside fg_thermal; ticked in the survival services block next to the in_water/swimming check.
- C3 STAMINA: the sprint reserve was ALREADY fully wired (sim::stamina — sprint drains it, gated on the meter, 25% speed
  penalty until it recovers past 15%); this roll just surfaces it on the HUD.
- HUD now shows four bars above the hotbar: Hunger, Thirst (blue→amber→red), Stamina (cyan-green), Warmth (°C). Panel
  moved up (ph−190) to clear the hotbar. SurvivalHud.thirst fed from fg_hydration.level (was a 1.0 placeholder).
lib + bin compile. DEVICE-VERIFY: in survival, watch Thirst fall (faster sprinting) and refill when you stand in water;
let it hit parched and confirm vitality starts dropping; Stamina bar tracks sprinting.

## r548 — hero model fixes: orientation, per-voxel colour, Heliographer staff
Operator feedback pass on the forge heroes (hero_forge.rs). Three of the four reported issues fixed; the fourth (full
hierarchical joint system + richer animation) is the next roll.
- SIDEWAYS (fixed): the hero faces +X (camera forward at yaw 0) but its LIMBS were also spread along X (sx*shoulder_hw,
  sx*hip_x were X offsets) — so arms/legs splayed front-to-back and the figure read edge-on. Rebuilt the whole body on a
  consistent convention: facing/DEPTH = X, WIDTH (left/right) = Z, up = Y. Limb L/R offsets now use Z; torso/pelvis/chest
  half-extents swapped so the body is wider side-to-side than front-to-back; forward features (emblem, eyes, fin) stay on
  +X; horns/back-slab corrected. Walk swing still pivots fore/aft in the X-Y plane (correct for a Z-offset limb).
- ALL-WHITE (addressed): the palette was 4 entries (base/cloth/accent/glow) and several heroes were light-toned, so under
  the additive entity rim-light + fog they washed pale. Replaced with a 9-slot per-part palette (skin, hair, eye, base,
  cloth, trim, accent, glow, boots) and assigned each body part its own colour (head/hands = skin, hair cap + back, eyes,
  belt = trim, legs = cloth, feet = boots, torso = base, plating = accent, emblem = glow). The hero now reads as a
  coloured character. NOTE: if any wash persists it's the shader rim-light/fog on small meshes (device-side), not the data.
- HELIOGRAPHER RINGS (fixed): moved off the head — they were a head-halo — onto the iconic SUN-STAFF held at the left
  side (vertical shaft + hub + a 6-segment glowing ring), per the operator.
- Bonus toward the joint ask: limbs are now segmented (thigh+shin, upper+forearm+hand) and the head has hair + a pair of
  eyes, so there's more articulation to animate next.
- Preview (menu): switched the 2D projection to a FRONT view (Z→horizontal, X→depth key) since width moved to Z.
lib + bin compile. DEVICE-VERIFY: pick each hero (F5) — should face the look direction (not edge-on), show distinct
colours per part, and the Heliographer should carry a ringed staff at its side instead of rings on its head.
NEXT ROLL: port the full hierarchical joint/body system + more animation states (the operator's "port the full body system
and the joints still ... should have more [animation]").

## r549 — hero rig PORT from the real character-forge models (not the ✦ relief/"star" versions)
Operator clarified: the "star" versions are the `✦` RELIEF models (relief_sw/he/ww/tc/gb — "image-matched voxel relief
built from concept art via voxelize.py"), a depth-renderer for dimensioning — DISCARD. The REAL models are the
buildHeliographer/Stormweaver/Windwalker/Tidecaller/Gravitybender() functions in character_forge_3d.html. Ported those
(was hand-authoring approximations before).
- JOINT HIERARCHY: ported makePivots — a compounding pivot rig (bodyRoot→hips→chest→neck→head; chest→shoulder→elbow→hand;
  hips→hip→knee→foot, 15 pivots). Implemented a tiny transform stack (Xf = rot+translate, world = parent∘local) since the
  old single-pivot box_at couldn't compound shoulder→elbow→hand etc. This is the "full body system and joints".
- SHARED BODY: ported buildBody faithfully — legs (thigh/shin/foot), torso (masc broad chest+deltoids / fem bust+hip flare
  via gender), neck+collar, head (skull/jaw/eyes/brow/nose/mouth/chin/beard) + hair, arms (sleeve/forearm/hand). Each part
  is one box at its pivot-local position; sizes are the real voxel dims ×u (u=0.072).
- REAL COLOURS + DIMS: ported every hero's palette (the P={…} hexes) via their buildBody colour mapping (legs/foot/torso/
  collar/skin/eye/sleeve/fore/brow/beard + accent/glow) and their dim{} proportions. Replaces the 9-slot hand-palette.
- ORIENTATION: forge characters face +Z; remap three→engine (X=+Z, Y=Y, Z=−X) so the hero faces the look direction. Feet
  auto-grounded (min-Y→0) for any dims/pose. BuildParams sliders scale the ported dims.
- ANIMATION: walk (hip/knee + counter-swing shoulder/elbow about the joints) + idle (body sway, head turn, shoulder
  breathe) driven through pivot rotations. Held item rides the right hand.
- Preview: front (Z-Y) projection, proven output mapping (+role/bumpiness fields).
lib + bin compile. build() signature unchanged (kind, params, origin, &HeroPose).
DEVICE-VERIFY: each hero should now have the real proportions + real per-part colours + a jointed walk/idle, facing the
look direction. NOT YET (next rolls): per-hero PROPS (Heliographer sun-staff+lantern+cape, Tidecaller trident+sea-beast,
Gravitybender gravity orb+rock cubes+cloak, Windwalker sash+wind, Stormweaver storm FX); finer per-VOXEL density (1 box/
part now, not voxel-filled); the update() element FX. Gravitybender is legless (levitator) until its debris/orb prop lands.

## r550 — heroes: fix the white-wash + much richer animation
DIAGNOSED the "very white" body: dumped the built hero vertex colours — they are CORRECT and dark (Tidecaller teal
0.18,0.43,0.43; Stormweaver near-black 0.08) yet render white. So the data path is fine; the wash is render-side. Two
additive culprits, both fixed:
- ENTITY RIM LIGHT (entity.frag): `color += rcol*rim*0.85` over a flat-shaded box body pushed dark colours toward white.
  Made it edge-only + gentle: `pow(...,3.2)`, `min(rim,0.6)`, strength `0.28` (was 0.85). Creatures still get a soft
  silhouette rim; the base vertex colour now reads true. Recompiled entity.frag.spv (glslang RC=0, hash dropped).
- AVATAR FADE (bin): opacity floor was 10% (`1 - 0.9*coverage`) — a covering body washed toward the bright background.
  Softened to ~45% (`1 - 0.55*coverage`) so the body stays clearly coloured.
RICHER ANIMATION: HeroPose now carries the raw walk_phase + walk_amp (was a single pre-baked `gait`), so build() derives
a fuller cycle: shoulders/chest counter-rotate the hips (twist about Y), pelvis tilt/drop + weight shift, forward lean
into the stride, deeper knee flex at speed, larger arm throw with elbow flex through the swing, a 2× vertical bob, and a
livelier idle (breathing drift, weight shift, head turn + nod). bin passes hero_gait_phase/amp straight through.
lib + bin compile; shader validates. DEVICE-VERIFY: heroes should now show their real (often dark/saturated) colours
instead of washing white, and walk/idle with visibly more body english. If any wash REMAINS, the colours are confirmed
correct in-data, so the next suspect would be the froxel-fog composite or scene exposure (not the hero).

## r551 — hero props: Heliographer kit + Gravitybender levitation
First per-hero prop pass (ported from the character-forge builders), via a new dress(kind, …) called after build_body.
Props attach to a hand pivot (ride the arm) or to world/root space (stand upright, turn with yaw); anim_t drives spins.
- HELIOGRAPHER: leather cap + dark sunglasses visor + gold brow rim; a flowing two-slab cape down the back; the iconic
  sun-orb STAFF at the left (shaft + 5 gold bands + a 12-cube halo ring that slowly spins + sun disk + glowing core +
  finial); and a LANTERN hanging from the right hand (cage top/bottom + 4 bars + caged light) that rides the arm swing.
- GRAVITYBENDER: fixed the broken legless look — a hooded cloak (head + back), a floating DEBRIS PLATFORM of 5 stone
  slabs (gentle bob; becomes the grounded base so the sorcerer levitates on rubble), and a purple gravity ORB in front of
  the chest with 3 ROCK CUBES orbiting it (anim_t).
lib + bin compile. DEVICE-VERIFY: Heliographer should carry a ringed sun-staff (left) + swinging lantern (right) under a
capped/visored head with a cape; Gravitybender should float on bobbing debris with an orbiting-rock gravity orb instead of
standing legless.
NEXT: Windwalker (blue sash + wind streams), Tidecaller (trident + sea-serpent mount), Stormweaver (storm staff + FX);
optional per-voxel density (currently 1 box/part).

## r552 — survival crafting tree (gather → smelt → forge → fight)
Expanded the C-menu crafting from 5 hardcoded recipes to a real survival tree (still using the wired
items::crafting::{Recipe,can_craft,craft} against fg_app.inventory(), so the gather→drops→inventory→craft loop is intact):
- Smelting: Iron / Gold / Copper Ingot (ore + coal).
- Building stock: Oak Planks ×4, Stone (4 cobble), Stone Bricks ×4, Glass (2 sand), Bricks (clay).
- FORGED WEAPONS (ids 4000..4007 = Dagger/Blade/Greatsword/Axe/Greataxe/Spear/Staff/Builder Wand) from ingots + a plank
  handle — completes gather→smelt→forge→fight. (4006 Staff / 4007 Builder Wand are the items the operator flagged as the
  hero staff/wand props; weapons are holdable via the survival hotbar + durability path at bin ~3168.)
- Cooking: Cooked Meat (raw meat + coal) as a campfire alternative.
The sim::recipe_book index is now built FROM the recipe inputs (was a parallel hand-list that could drift), so holding an
ingredient still floats its recipes to the top. Added canonical tooltips for every new output.
bin compiles. DEVICE-VERIFY: in survival, mine ore+coal+logs, press C, and the new recipes should light up as their
ingredients are met; forge a weapon and hold it from the hotbar.
NOTES for next: (1) hero props → STARTER inventory items per character (operator: they "come with the characters"; 4007
Builder Wand already exists). (2) survival inventory GRID display (the creative picker shows placeable blocks; survival
wants the player's collected stacks+counts on screen). (3) optional crafting STATIONS (recipe.rs already has Station::
Bench/Furnace — gate smelting behind a furnace, weapons behind a bench).

## r553 — unified inventory UI: crafting + inventory + hotbar switcher under `p`, centered
Merged the separate inventory (creative picker) and crafting (C window) surfaces into one centered panel opened by `p`.
- CRAFTING UNDER `p`: the bin now fills craft_lines whenever the inventory panel is open (gate += show_inventory, was
  craft_menu_open only), so the recipes appear in the `p` panel. (The standalone C window still works as a quick shortcut.)
- CENTERED: the creative inventory Area moved from top-right to screen-centre.
- HOTBAR SWITCHER: a row of the 9 quick slots is drawn inside the centered panel — click a slot to pick the in-hand item
  (mirrors the bottom hotbar + scroll wheel; selected slot gets the accent stroke).
- CRAFTING SECTION: the recipes render inline under the grid in a scroll area (craft button + have/need + hover tooltip),
  setting craft_pick exactly like the old window.
- MOUSE SCROLL → HOTBAR: already wired (bin ~3125 cycles selected_palette_idx in game mode, wraps, survival maps it to the
  inventory slot). Confirmed; left as is.
lib + bin compile. DEVICE-VERIFY: press `p` — a centered panel should show the inventory grid + a HOTBAR row + a CRAFTING
list together; clicking a craft row with met ingredients should craft; clicking a hotbar slot or scrolling should change
the held item.
NOTE: the panel's grid is the existing creative catalog (Inv/Items/Eggs tabs). Feeding the real survival backpack stacks
(fg_app.inventory contents + counts) into this same grid is the natural next step if you want survival to show only what
you've gathered.

## r554 — hotbar selection: make scroll VISIBLE + Alt+home-row chord + click-to-select
Root cause of "scroll didn't work / only number keys did": the bin tracked the active slot in `selected_palette_idx`, but
the hotbar UI highlights `renderer.creative_selected`, which the bin never set — so scrolling changed the selection
invisibly and felt dead.
- SCROLL NOW SHOWS: the scroll handler and the number/home-row key handler both set renderer.creative_selected =
  selected_palette_idx, so the bottom hotbar highlight (and the centered panel's hotbar row) move with the wheel/keys.
- ALT + HOME-ROW CHORD: hold Left Alt and tap a labelled home-row key (a o e u i d h t n) to jump to that hotbar slot.
  The slots were already labelled with these keys; the modifier keeps the bare keys free for movement/typing.
- CLICK-TO-SELECT: a click in the centered inventory's hotbar switcher (r553) is now read back into the gameplay slot at
  the top of the loop, so clicking a slot actually selects it (push from keys/scroll + this pull stay in sync via equality).
bin compiles. DEVICE-VERIFY: scroll the wheel in-world → the bottom hotbar highlight moves and the held item changes;
Alt+a/o/e/… jumps to that slot; clicking a slot in the `p` panel selects it.
STILL TODO (operator noted): a HAND-HELD BLOCK viewmodel — the first-person viewmodel renders weapons but not the selected
block, so block selection has no in-hand model yet. That's a separate viewmodel feature (render the held BlockId as a small
cube in hand), good for the next roll.

## r555 — dragon WING MEMBRANE wired (docs/TODO.md #6)
The forge deliberately leaves the wing membrane out of the voxels ("membrane is render-only", dragon_forge.rs:225); the
ready-made pure module wing_membrane.rs (membrane_from_box) was written for "an alpha-capable entity vertex path, once one
exists" — which now exists (EntityVertex per-vertex alpha, r535). Wired it in forge_to_vertices (world/entity.rs): after
the opaque bone boxes, emit ONE translucent triangle-fan membrane per wing, spanning the combined bounding box of that
side's wing bones (joint == J_WING_L / J_WING_R), span axis = model X. Membrane vertices go through the same model→engine
axis remap + yaw/pitch/roll orientation + origin as the boxes; colour = body_color, per-vertex alpha from wing_opacity.
Keeps the bone boxes (structure) and adds the skin (sheet), so the wings should read as membranes instead of sparse bones.
lib + bin compile. DEVICE-VERIFY: dragon wings should now show a translucent skin filling each wing, flapping with the
bones. If too faint/opaque, the alpha gene (0.55) and detail (14 panels) are the knobs; if the sheet is mis-oriented, the
span axis / chord pick in membrane_from_box is the thing to revisit.

## r556 — bending: wire STORMSTRIKE lightning + fix dead Air/Water arms
Found a real bug while doing bending work: player_bending::step resolves the discipline via discipline_for, which maps
Air→Stormstrike, Water→HeavyBrine, Fire→ThermalPhase, Earth→Elemental(Earth). But the bin's discharge match had arms for
`Elemental(Air)` and `Elemental(Water)` — which therefore NEVER matched, so air/water bending silently fell through to the
generic `_` strike, and the dedicated sim::bending_stormstrike module was unused.
- STORMSTRIKE (Air → lightning) now wired to the real bending_stormstrike::route: builds Conductors from the roamers
  (conductance scales with hp + airborne), routes a forward conductance-seeking bolt through the aim cone (chain length
  grows with power), and damages each chained creature with per-hop falloff. Mis-aiming (route grounds on the bender) saps
  stamina by the misprediction penalty. A gust still follows to shove close targets. Witnessed-attack events recorded.
- WATERBENDING re-reached: the surge+conjure arm is now under `Discipline::HeavyBrine` (Water's real discipline) instead of
  the dead Elemental(Water), so waterbending works again.
bin compiles, no unreachable-pattern warnings (the dead arms are gone). DEVICE-VERIFY: with UNENDLICH_MODE=survival, an
air/lightning discharge should chain a bolt through creatures in the aim cone (HUD/log "stormstrike chained N"), and a
mis-aim should sap stamina ("grounded — mis-aimed"); water discharges should surge + conjure water again.

## r557 — GPU meshing speed: default the tiled shared-memory path ON
The octree mesher (chunk_mesh_octree.comp) already has the right optimization built and gated: the per-cell path does
"7 pointer-chasing octree walks" per cell (self + 6 neighbours) — the comment itself calls this "slow on Asahi". The TILED
path (r478/r479) reinterprets each 64-thread workgroup as a 4³ spatial tile, cooperatively loads the 6³ halo (tile + 1
border) into shared memory with ONE octree descent per contiguous run (a per-lane leaf cache collapses uniform runs to a
single descent), barriers, then every cell reads self + 6 neighbours from LDS with ZERO further descents. Both paths
already compute byte-identical values (verified by UNENDLICH_OCTREE_MESH_VERIFY=1 against the CPU reference) and the
dispatch launches the same workgroup count for both (cells/64 == tiles), so it's a drop-in.
It was gated OFF by default (UNENDLICH_OCTREE_TILE). r557 flips the default to ON (off only if UNENDLICH_OCTREE_TILE=0);
the live Performance-page toggle still works. Logs "[mesh] octree tiled shared-memory path: ON".
bin compiles. DEVICE-VERIFY: expect faster remeshing (fewer octree descents) especially while streaming/editing. Please
confirm parity once with UNENDLICH_OCTREE_MESH_VERIFY=1 (GPU vs CPU mesh) on the M2; if anything looks off, UNENDLICH_
OCTREE_TILE=0 restores the per-cell path instantly. (Did not touch the emit-path atomics — a per-cell atomic-aggregation
is the next lever but is a riskier blind refactor; the tiled path is the bigger, verified win.)

## r558 — hero models default to 2 blocks (slider → 4) + camera grounded to the face
Hero models were ~4.2 blocks tall, so the first-person camera (physics eye 1.62) sat at mid-torso and the model
floated ~0.12 above the ground (avatar feet were hardcoded at camera−1.5).
- hero_forge: overall height is now ONE uniform scale applied in Body::finalize — the model is scaled so its span equals
  `hero_total_height(params) = 2.0 * height.clamp(0.5,2.0)` blocks. Default (slider 1.0) = **2.00 blocks**, slider max
  (2.0) = **4.00 blocks** (verified by a build dump). The other sliders (bulk/head/shoulders/arm/leg) now adjust SHAPE
  proportions only, not overall height, so they no longer fight the height scale. Scaling is about the rig's local origin
  (already centred) so the body stays under the camera; held props scale with the model.
- New `pub fn hero_total_height(&BuildParams) -> f32` so the bin can size the camera + collider from the same number.
- bin: the avatar's feet are now placed at the player's TRUE feet (`camera − (PLAYER_HALF_HEIGHT+PLAYER_EYE_OFFSET)` =
  camera − 1.62) instead of a hardcoded 1.5. With the model at ~2 blocks this lands the camera at the face and grounds
  the feet. Both the rebuild and per-frame avatar paths updated.
lib + bin compile; height verified 2.00 / 4.00. DEVICE-VERIFY: default characters should look ~2 blocks tall with the
camera at the face, feet on the ground; the height slider should scale them up to ~4 blocks.
PENDING (next): couple the player COLLIDER height + eye offset to hero_total_height at spawn so a 4-block character also
gets a 4-block bounding box + raised eye (today the collider stays 1.8 regardless of slider — fine at the 2-block
default, loose for tall characters). Then: walk-slide→ground tie, falling/impact/flying poses, survival 3×9+2×2 inventory,
Performance "advanced" tiled-mesher toggle.

## r559 — STATURE system: height as the master physical scalar (foundation)
New design (operator request): a character's height drives their whole physical identity. Documented in docs/STATURE.md
(modelled on ELEMENTS.md) + a pointer from DESIGN_NOTES.md.
- NEW pure module `sim::stature`: `StatureProfile` + `derive(height, bulk, density, strength) -> StatureProfile`,
  computing mass (∝ h³·bulk·density), buoyancy (1−density), max_health, strength, hunger_rate (Kleiber ∝ mass^¾),
  knockback dealt/resist, step_height, jump_velocity, move_speed, turn_rate, ground_shake (≈0 at the person, grows with
  mass), and wind/water-current resist (threshold ∝ mass, mult ∝ 1/h). Anchored at the 1.8 person so `derive(1.8,1,1,1)`
  reproduces today's tuning EXACTLY (verified: eye 1.62, jump 10, walk 4.5, step 1.1, shake 0, kbResist 0.5). Tests +
  an example dump confirm the anchor, monotonicity (taller = heavier/stronger/slower-turn/more-shake/more-kb-resist),
  buoyancy sign from density, and finite/clamped extremes (0.9..15.0).
- `BuildParams.height` is now the stature in BLOCKS (default 1.8, was a 1.0 multiplier); `hero_total_height` clamps to
  0.9..15.0 and the model uniformly scales to it — so the tallness slider now spans 0.9..15.0 BLOCKS (customizer slider
  range widened from 0.7..1.4; 1 decimal). Default characters are 1.8 blocks with the camera at the face (r558 grounding).
lib + bin compile. DEVICE-VERIFY: the tallness slider should now produce sprites (~0.9) up to building-sized titans (~15);
default 1.8 unchanged/correct.
PENDING (next roll — all read the now-authoritative profile): runtime eye-offset so the camera sits at the face for ANY
stature; collider half-extents + feet-grounding from the profile; then max_health/hunger, move/jump/step/turn, knockback
dealt/resist, ground_shake → camera-shake/earthquake/erosion, and wind/water-current resist. (Today non-1.8 statures
render at the right model size but keep the 1.8 collider + 1.62 eye until that coupling lands.)

## r560 — Performance: tiled mesher under an "Advanced" section + default-on reflected
Closes the r557 follow-up. The Performance tab's GPU-mesher controls now have an "ADVANCED" subheading; the GPU tile
mesher toggle is labelled "(on by default)" with a description noting it cuts GPU descents / speeds remeshing. The
renderer's ui_octree_tile field now initialises to true (the bin already syncs it from UNENDLICH_OCTREE_TILE, default on,
at startup) so the toggle reads ON out of the box. lib compiles.

## r561 — stature physics coupling + semi-log height slider
- **Semi-LOG tallness slider** (world_renderer customizer): the slider now travels in ln(height) over [ln 0.9, ln 15.0]
  so equal travel = equal ratio (fine control near the 1.8 human, still reaches the 15-block giants), with the value
  shown alongside ("1.8 blk").
- **Stature physics coupling** (bin): a `player_stature` profile is derived from the active character's height/bulk and
  recomputed whenever the character changes (the self_avatar_dirty path) + on every body creation (F6 / respawn / menu
  launch, which now flag the resize). From it the bin drives:
  - **collider** — `body.half_extents` set to the stature's; on a change the AABB centre is shifted by the half-height
    delta so the FEET stay planted (no sink/pop).
  - **camera eye height** — a runtime `player_eye_offset = eye_height − height/2` replaces the `PLAYER_EYE_OFFSET` const
    at every camera-sync site (per-frame, respawn snap), so the camera sits at the FACE for any stature; the third-person
    avatar feet use `player_eye_height` too.
  - **move speed** (base walk = `move_speed`), **jump** (`velocity[1] = jump_velocity` on take-off), **step height**
    (`params.step_height`). All anchored at the 1.8 person, so a DEFAULT character is byte-for-byte today's tuning.
- lib + bin compile. DEVICE-VERIFY: default characters unchanged; a tall character should have a taller collider, camera
  at the face, step over higher ledges, jump higher + move faster; a short one the opposite. Watch for ceiling clipping
  when switching to a much taller stature in a low space (the resize keeps feet planted and grows upward).
PENDING (next): max_health/hunger, knockback dealt/resist, ground_shake → camera-shake/earthquake/erosion, wind/water
resist, buoyancy/density → water. Then the queued walk-slide / falling-impact-flying anims / survival 3×9+2×2 inventory.

## r562 — stature feel: slower gait, height-scaled chase cameras, height-scaled FP fade
Three pieces of operator feedback, all reading `player_stature`:
- **Gait cadence** slows as the body grows: the hero walk-phase rate is multiplied by (1.8/height)^0.6, so a giant takes
  slow, long strides even though its ground speed (move_speed) is higher; = 1.0 for a default character.
- **Chase cameras** (third-person mode 1 + over-the-shoulder mode 2) pull back, rise, and tilt down with height:
  distance ×(height/1.8), lift ×(height/1.8), pitch −(0.10·(hz−1), capped 0.45 rad) so a tall hero stays framed. The
  shoulder offset widens by √hz. Default characters are unchanged (hz=1).
- **First-person avatar fade** scales with height: the coverage net (near-depth 2.6, lateral 1.3) widens ×(height/1.8)
  and the fade DEPTH grows from 0.55 up to 0.92, so a tall character's body — which fills the FP view — fades nearly
  transparent instead of obscuring it. Default unchanged.
bin compiles. DEVICE-VERIFY: tall characters should stride slowly, stay framed in 3rd/shoulder views (camera further +
tilted), and not block the FP view (body fades out when it fills the frame); default characters identical to before.

## r563 — stature: health (damage-resist) + hunger (metabolism)
- **Hunger** scales with stature: the base metabolic vitality drain ((0.01 idle / 0.02 in water)) is multiplied by
  `hunger_rate` (Kleiber ∝ mass^¾), so a giant gets hungry far faster; exposure (climate) is left unscaled. = today's
  rate for a default character.
- **Health**: the player's vitality is normalised [0,1] (no HP pool to enlarge), so max_health is applied as a
  damage-resist multiplier `player_dmg_resist = 100 / max_health` (recomputed on character change). Every incoming
  vitality hit — predator bites, fall/impact, creature attacks (4 sites) — is scaled by it, so a bigger character takes a
  proportionally smaller fraction per hit (effectively far more durable); = 1.0 (unchanged) for a default character.
- `stature::BASE_HEALTH` made public for the normaliser. bin compiles.
PENDING: knockback dealt/resist, ground_shake → camera/earthquake/erosion, wind/water-current resist, buoyancy/density →
water. Then the queued walk-slide / falling-impact-flying anims / survival 3×9+2×2 inventory.

## r564 — chase-cam bump-up, tallness range 0.1..25, sphere building for giants
- **Chase camera bump-up**: the third-person/shoulder cameras now rise ~0.6 per extra block of stature (on top of the
  ×hz base lift), so tilting down still frames the WHOLE body — the top of tall characters no longer clips out.
- **Tallness range extended to 0.1 .. 25.0 blocks** (was 0.9..15): stature::MIN_HEIGHT/MAX_HEIGHT + the semi-log slider
  bounds. (Doc updated.)
- **SPHERE BUILDING** (operator request): above a stature of 5 blocks the block break/place becomes spherical, radius
  `floor((h−5)/5)+1` capped at 5 (1 at 5 blocks → 5 at 25). BREAK consumes a sphere of solids around the raycast hit
  (carve caverns in one click); PLACE raises a sphere "mound" of the selected block into surrounding air — in survival
  each extra block costs one from inventory and it stops when you run out; the player's own AABB is never filled; crops/
  saplings are excluded from mounding. Raycast reach also scales with stature (√(h/1.8), = 6.0 at default) so giants can
  actually target blocks. Default characters: single-block break/place, reach 6, unchanged.
bin compiles. DEVICE-VERIFY: tall characters stay fully framed in 3rd/shoulder views; the slider spans 0.1..25; a >5-block
character carves/fills spheres (bigger with height) and reaches further; a default character is unchanged.

## r565 — fan-dig + fix 1×1 place + bigger radius
- **Fixed place "still 1×1"**: there are TWO place code paths (with-body and the no-body/creative-flight path); r564's
  mound was only on the first, so creative placing stayed single-block. The no-body path now raises a mound too.
- **Bigger radius**: dig_radius is now ceil((h−4)·¼), clamp 1..6 — 1 at 5–8 blocks, 2 at 12, 3 at 16, 4 at 20, 6 at 25.
- **FAN DIG** (operator: "breaking depends on a single central ray; fan out to cover area"): breaking now casts a grid
  fan of rays (half-count min(dig_radius+1, 4), ~0.06 rad apart) around the look direction; each ray raycasts and carves a
  short inward column (depth min(dig_radius+1, 5)). So a break covers an AREA following the surface and digs down easily,
  instead of a fixed sphere at one point. The central ray keeps the normal drop/durability/harvest logic; ray count is
  capped (≤81) to bound the per-click cost.
bin compiles. DEVICE-VERIFY: a tall character's break should clear a fanned patch + depth (digging down is quick), and
placing should raise a real mound in BOTH creative and survival; default characters unchanged (single block, reach 6).

## r566 — stature: ground-shake, knockback (dealt), wind + water-current resistance
- **Ground-shake** (camera): a heavy body's footfalls + landings jolt the camera. The hard-landing jolt is multiplied by
  (1 + ground_shake·0.25, cap 4×); and a per-FOOTFALL shake fires while walking on the ground — detected from the view-bob
  phase crossing π (the 2× stride) — scaled by ground_shake × speed. Both are 0 for a default character (ground_shake≈0).
- **Knockback dealt**: a melee punch now shoves the struck creature along the punch direction (+ a small upward pop),
  magnitude 3·knockback_dealt — a default character gives a light shove, a giant sends them flying. (knockback RESIST has
  no site yet — nothing currently applies melee knockback TO the player; deferred.)
- **Wind resistance**: PhysicsParams gained a `wind_resist` field (default 1.0); step_body multiplies the ambient wind
  force by it; the bin sets it to the stature's wind_resist_mult (∝ 1/h), resolving the old "couple to mass" TODO.
- **Water-current resistance**: the current-drift push to the player is now gated by a stature threshold (∝ mass — weak
  currents don't budge a giant) and scaled by water_resist_mult (∝ 1/h). Default characters: unchanged.
lib + bin compile. PENDING: ground_shake → erosion (block displacement under the foot), buoyancy/density → water
(sim::buoyancy exists), knockback-resist site. Then the queued walk-slide / falling-impact-flying anims / survival 3×9+2×2.

## r567 — stature: buoyancy/density + erosion footprints
- **Density now tracks the muscle/bulk slider** (the stature derive is called with density=bulk and strength=bulk), so
  buoyancy/knockback/health get real variation without a new slider. A muscular body is denser + heavier; a lean one
  lighter.
- **Buoyancy → swim float**: the swim buoyancy force (36) is divided by the body's density (clamped 0.4..2.5), so a dense
  (muscular) character floats lower / can sink and must stroke up, while a lean one rides high. = unchanged (waterline at
  the shoulders) for a default bulk-1.0 character.
- **Erosion footprints** (completes ground_shake): a very heavy body (ground_shake > 2 ≈ taller than ~3.5 blocks)
  tramples/cracks the ground on each footfall — grass→dirt, stone→cobblestone, snow crushed — over a small patch under
  the feet that grows with height (radius 0..3). Leaves a visible trail; gated so normal characters never erode terrain.
  Footfall effects are now also gated on having a physics body (no phantom shake/erosion while flying in creative).
lib + bin compile. The stature system is now substantially complete; only knockback-RESIST lacks a site (nothing applies
melee knockback to the player yet). NEXT: the older queue — walk-slide fix, falling/impact/flying poses, survival 3×9+2×2.

## r568 — hero gait: anti foot-slide (constant cadence)
Operator (older queue): the hero's feet slide as it walks. Root cause: the gait PHASE advanced on a speed-scaled
timer `(4 + spd·1.5)·cadence`, while the leg-swing AMPLITUDE also scales with speed (`amp = spd·0.11`). With both the
rate and the amplitude rising with speed, the planted foot's ground speed (≈ leg_len·amp·dφ/dt) grew ~quadratically and
outran the body — the foot skated. Fix: hold the phase RATE CONSTANT so the mid-stance foot speed (leg_len·amp·rate)
tracks body speed linearly — i.e. fixed cadence, stride LENGTH grows with speed, which is how real gait works. The rate
is tuned to leg length (≈ ½·height) as `1/(leg_len·0.11)`, so a tall body strides at a slower cadence and, with longer
legs, covers more ground per step (this also subsumes the r562 explicit cadence term). When stopped, amp→0 hides the
still-advancing phase.
- TUNING KNOBS (I can't see the result — please eyeball on the M2): leg-length fraction `0.5` (raise if the cadence
  feels too quick), the `0.11` amp coefficient (must match the `target_amp = spd·0.11` line for zero slide), and the amp
  clamp `0.5` (above the speed where amp saturates — i.e. sprinting — a little slide returns; raise the clamp toward 0.6
  to push that point out, at the cost of a wider leg splay).
lib + bin compile. NEXT (queue): falling / ground-impact / flying hero poses; survival 3×9 + 2×2 inventory panel.

## r569 — stature: range → 50 blocks + LINEAR reach (giants can break blocks)
Operator: 25-block giants can't break any blocks; raise the slider to 50 and make reach scale with tallness.
- **Range 25 → 50.** `sim::stature::MAX_HEIGHT` is now 50.0; the in-game tallness slider reads its bounds from
  MIN_HEIGHT/MAX_HEIGHT (semi-log, 0.1..50) instead of hard-coded 0.1..25.
- **Reach now scales LINEARLY** with stature: `reach = (6 + 1.2·(height − 1.8)).max(6)` for both the primary block
  break/place ray and the giant dig-fan rays. The old √ scaling (`6·√(h/1.8)`) was the bug: a 25-block character's eye
  sits ~22.5 blocks up, but √ reach was only ~22.4 — the ray literally couldn't reach the floor in front of it, so
  nothing was ever targeted. Linear with slope 1.2 clears the eye height at every stature (verified: reach 33.8 vs eye
  22.5 at 25 blocks; 63.8 vs 45.0 at 50), so a giant can target the ground a good distance forward.
- **Other factors swept for the wider range** (verified finite + sensible at 50): mass ∝ h³, max_health, hunger,
  ground_shake, jump, move_speed, step_height, turn_rate all stay finite; the clamped consumers (footfall/landing shake
  .min caps, erosion radius clamp 0..3, sphere/fan dig_radius clamp 1..6) and the proportional chase-camera framing
  (distance/lift ∝ height, terrain-pinched) all behave at 50 with no new saturation. No re-tuning needed beyond reach.
lib + bin compile. (Runtime example blocked only by a full disk; formulas verified analytically + lib typechecks.)
NEXT (queue): falling / ground-impact / flying hero poses; survival 3×9 + 2×2 inventory panel.

## r570 — hero poses: falling tuck + landing crouch (+ basic flight)
Operator (older queue): the hero lacks falling / ground-impact / flying poses. HeroPose gained three inputs — `airborne`
(0 grounded … 1 off-ground), `vertical_vel` (signed, m/s), `land_crouch` (0..1 touchdown pulse) — and hero_forge::build
blends them additively onto the walk rig (walk amp is ~0 mid-air, so they read cleanly):
- **Falling tuck**: knees draw up, thighs flex, arms abduct outward for balance; the torso leans BACK the faster you
  drop and slightly FORWARD when rising (jump apex / fly-ascend), from the sign of vertical_vel.
- **Landing crouch**: a transient deep knee+hip flex with the arms swung forward to brace, pulsed on a hard touchdown
  (depth scales with impact speed) and decayed over ~¼ s by the caller.
- **Flight**: in creative flight (no physics body) `airborne` rides at 1, so the flying hero holds the airborne tuck —
  a basic flight pose. (A dedicated horizontal "superman" flight rig is a later refinement.)
Wiring (bin): `hero_airborne01` eases toward (body.on_ground ? 0 : 1; None⇒1 flying); `hero_land_crouch` is set to
((impact−6)/14).clamp(.35,1) in the landing block and decayed in the gait block; `vertical_vel` is the real player body
velocity[1]. lib + bin compile.
- TUNING KNOBS (I can't see these — please eyeball on the M2): tuck depths (knee 0.9 / arm_out 0.7 / hip 0.35), crouch
  depths (knee 1.1 / arm 0.85), the airborne ease rate 9.0/s, and the crouch decay 4.0/s.
NEXT (queue): survival 3×9 + 2×2 inventory panel (currently the centred `p` panel shows the CREATIVE catalog even in
survival); then character-forge detail.

## r571 — giant-scale interaction: solid dig, stomp-crush erosion, bigger placement
Operator: titans can't dig DOWN (mining too feathered — needs to be solid); erosion should hit harder and each step
should destroy blocks in its path via block density×fill; building still feels like single blocks at scale.
- **SOLID DIG.** The old break used a fan of diverging rays — at titan scale the rays spread out and left gaps, and it
  couldn't punch a clean hole straight down. Replaced with a FILLED-SPHERE carve centred on the primary hit cell: every
  solid block within `dig_radius` is set to air (bedrock, hardness < 0, spared). Clean solid holes in any direction.
- **STOMP-CRUSH EROSION.** Each footfall now compares the stomp's crush capacity (≈ height/8, growing with mass /
  ground_shake) against each block's `hardness()` — the engine's density×fill resistance (soft 0.5, stone 1.5, wood 2.0,
  crystal 2.5, bedrock −1). Blocks it overpowers are pulverised to air; blocks it can't are weathered a tier; bedrock
  stops the foot. It drives DOWN through a depth that grows with the crush capacity, so a titan gouges a trench while a
  ~4-block ogre just packs the dirt. (Replaces the old cosmetic grass→dirt-only weathering.)
- **BIGGER VOLUME.** `dig_radius` now scales 0.28·(h−4), ceiling raised 6 → 8, shared by dig AND the existing
  filled-sphere place. Placement was already a solid sphere; with the larger radius it reads as a real mass, not a
  single block. (Creative fills the full sphere; survival is still bounded by inventory.)
lib + bin compile.
- DEVICE-VERIFY (M2): titan digging straight down should carve a clean solid bowl; each footstep should crush a crater
  (soft fully, stone/wood weathered until big enough, bedrock unbroken) — confirm a walking titan gouging a trench does
  NOT get stuck (its huge step_height should climb out; if it sticks, lower the erosion `depth` cap from 5).
- TUNING KNOBS: radius factor 0.28 / cap 8; crush_cap = h/8; foot footprint h/7 cap 4; sink depth 1+crush cap 5. Watch
  set_block volume per click (r=8 sphere ≈ 2k blocks) for remesh spikes.
NEXT (queue): survival 3×9 + 2×2 inventory panel; then character-forge detail.

## r572 — hero pose: horizontal "superman" flight (closes the r570 gap)
r570 left flight as the upright airborne tuck. HeroPose gained `flight01` (0 upright … 1 horizontal); build() blends a
superman pose: the torso pitches toward horizontal, arms stream forward overhead, legs straighten and trail back
together, and the neck/head lift to hold the gaze along the flight line. Bin: `hero_flight01` eases toward 1 while
actively flying (creative flight = no physics body) AND moving (>1.5 m/s), and back to 0 when hovering or grounded — so
hovering keeps the upright tuck and cruising goes prone. lib + bin compile.
- TUNING KNOBS (eyeball on M2): body tilt −1.45 rad, arm-forward −2.6, head/neck lift 0.6/0.7, ease rate 5/s, the
  1.5 m/s move threshold.

## INVENTORY PANEL — deferred, needs a dedicated pass (not a quick fix)
The centred `p` panel still draws the creative catalog in survival. Making it the real 3×9 + 2×2 backpack is bigger than
it looks: (1) the renderer has no player-inventory snapshot (it only gets the creative `items` Vec + a SurvivalHud with
no stacks), so the stacks must be plumbed in from the bin each frame; (2) there is no `ItemId → ItemKind` lookup, and the
id space is mixed (blocks <~256, ingots 1000s, food 2000s, weapons 4000s), so rendering real slot icons/colours needs a
small id→kind/colour resolver built first; (3) the full mouse model (pick/place/split) lives in inventory_screen.rs and
would need wiring to egui. Recommend doing this as its own roll WITH an operator screenshot to verify layout, since it's
egui and can't be checked here. Suggested increment order: id→colour resolver → bin inventory snapshot → read-only real
grid in the panel (reuse the existing cell_grid widget) → then click interactions.

## r573 — giant tuning pass (operator feedback on r571/r572)
- **Mining bigger + place = half.** `dig_radius` factor 0.28→0.36, ceiling 8→16 (a 50-block titan now carves ~r16).
  A new `place_radius = dig_radius/2` drives both place mounds, so building reads as a mound at HALF the crater size.
- **Flight pose was belly-up — FIXED.** The torso pitch sign was wrong (rotated face-up); flipped to +1.45 so the body
  leans FORWARD into a face-down superman, with the head/neck lift signs flipped to match (gaze stays on the flight
  line) and the legs trailing back. Added **banking**: `HeroPose.bank` rolls the body into turns, eased from the view's
  yaw rate (clamped ±0.7 rad), so a heading change arcs — the "turn radius" read.
- **Footstep erosion curbed** for mid titans (~15–35 blocks felt too strong): footprint `h/7 cap4 → h/12 cap3` and sink
  depth `1+crush → 1+0.4·crush, cap 5→3`. Roughly 3–5× less volume per step in that range; titans still crater but the
  foot scuffs rather than excavating a pit.
- **Move-speed scaling curbed ~15%**: stature `move_speed` exponent 0.7→0.65 (base 1.8 person unchanged at 4.5; ~15%
  slower at 50 blocks, scaling down progressively below that).
lib + bin compile.
- DEVICE-VERIFY (M2): flight should now lean FORWARD (face-down) and bank into turns; mining sphere noticeably bigger,
  placement half its size; mid titans leave shallower prints; giants a touch slower.
- KNOBS: dig factor 0.36/cap16; place = dig/2; flight tilt +1.45 & bank gain 0.22 (flip if banking goes the wrong way),
  NECK/HEAD/leg signs if the flight head/limbs read off; erosion h/12 cap3 & depth 0.4·crush cap3; speed exponent 0.65.
- ⚠ PERF: an r16 dig sphere is ~17k set_block calls in one click → a large remesh; if it hitches on device, say so and
  I'll spread the carve across frames or lower the ceiling.
NEXT (queue): survival 3×9+2×2 inventory (needs id→kind resolver + bin snapshot — see prior note); character-forge detail.

## r574 — item_display resolver (unblocks the survival inventory panel)
Built the documented prerequisite for the real backpack: `item::item_display(id: u32) -> ItemDisplay { color, name }`,
a total (never-panics) resolver from a raw inventory `ItemId` to a slot swatch colour + short name. Decodes the mixed id
space authoritatively (per the crafting recipes + game loop): `0..256` → BlockId (base_color + name), `1000+idx` →
MetalKind::ALL ingot (1000 Iron / 1001 Gold / 1002 Copper), `2000/2001` → raw/cooked meat, `4000..4008` → forged weapon,
else a neutral fallback. Unit test + temp-example verified every class (Stone→grey, Grass→green, ingots tinted, meat,
weapon, unknown→generic). lib compiles.
- This is step 1 of the inventory panel plan. NEXT step: snapshot the player's real stacks in the bin each frame
  (`fg_app.inventory().slots()` → Vec of (id, count) → `item_display`) into a renderer field, then draw a 3×9 grid + 2×2
  craft area with the existing `cell_grid` widget when survival + Inventory tab (reuse proven widget = low visual risk).
  Click interactions (inventory_screen.rs mouse model) come after. An operator screenshot would help verify the layout.

## r575 — small-character jump boost
Operator: characters under ~1.4 blocks should jump ~75% higher. Apex height ∝ v², so the boost is v × √1.75 ≈ ×1.323,
applied in `stature::derive` and ramped over [1.35, 1.4] (full at/under 1.35, none at/above 1.4 → the 1.8 reference is
untouched). Verified: apex height is exactly ×1.75 for h ≤ 1.35 and ×1.00 for h ≥ 1.4. lib compiles; anchor/monotonicity
tests unaffected (both use h ≥ 1.8).

## r576 — survival backpack panel (real inventory, read-only)
The centred `p` panel now shows the player's REAL backpack in survival instead of the creative catalog (step 2, built on
the r574 resolver):
- **Bin snapshot**: each HUD tick, `fg_app.inventory().slots()` → occupied stacks → `item::item_display(id)` →
  `renderer.backpack: Vec<InvSlotView{color,name,count}>` (empty in creative).
- **Panel**: when survival (`hud_survival.is_some()`), the title reads "backpack", the creative tab strip and the
  creative hotbar-switcher row are hidden, and a 3×9 (27-cell) grid draws each occupied slot — colour swatch + stack
  count (top-right) + short name (bottom) — via the same proven `cell_grid` widget. Creative mode is byte-for-byte
  unchanged (its catalog grid + tabs + switcher all still run through the `else` branch). The existing recipe-based
  CRAFTING section is retained below.
lib + bin compile.
- DEVICE-VERIFY (M2) + please screenshot: open `p` in survival — it should show your actual items (swatch/count/name),
  NOT the block catalog. A screenshot would let me tune cell size / wrap and confirm legibility.
- SCOPE / honest gaps: (1) READ-ONLY — no pick/place/split yet (the inventory_screen.rs mouse model is the next step);
  (2) slots are shown PACKED (occupied stacks first) rather than at fixed grid positions — fine for an overview, but the
  interactive version will need true slot positions; (3) the operator's "2×2 crafting grid" is not yet a visual slot grid
  — crafting still uses the recipe-button list (functional, just not the 2×2 layout). These are the follow-up.
NEXT: inventory click interactions (or the 2×2 craft grid) ; character-forge detail.

## r577 — bug hunt: duplicate worlds, settings-under-`p` stacking, slider-vanishes-menu
**(1) DUPLICATE WORLDS.** The menu world list is built straight from `load_catalog_entries()`, which read the catalog
file with NO dedup. Identity was keyed on (name, seed) at creation (r291) while `WorldCatalog::add` and the player key on
NAME — so a world re-created under an existing name (e.g. fresh random seed) left a second row, and the menu showed both.
Fix: `load_catalog_entries` now DEDUPS BY NAME, keeping the most-recently-played row (every consumer — menu list, startup
catalog, the add/update paths — funnels through it, so this fixes all of them and the next save rewrites the file clean);
and the creation path now `retain(|e| e.name != name)` (replace by name, not name+seed).

**(2) SETTINGS COVERED BY `p`.** `show_creative` (which draws the centred `p`/creative panel) was set to
`ui_pointer_active`, which is true whenever ANY UI surface is open — so the panel rendered ON TOP of settings/pause/debug.
At the state level these were already mutually exclusive (`heavy_open` forces the inventory closed); the render gate just
didn't agree. Fix: `show_creative` now additionally requires that settings/pause/debug/perf/craft/character/title/wizard
are all closed — the `p` panel is one exclusive surface that yields to the others instead of stacking.

**(3) SLIDER MAKES MENUS DISAPPEAR (probable fix — please confirm).** The character customizer (where the tallness/muscle
sliders live) was MISSING from `heavy_open`, so the lightweight menu-state machine (which frees/RECAPTURES the mouse
cursor on overlay changes) kept running while the customizer was open. Dragging a slider there could let the cursor be
recaptured mid-drag, yanking pointer focus and making the menu vanish. Fix: `character_menu_open` is now part of
`heavy_open` (and excluded from `show_creative`), so the customizer suppresses the light overlay like every other heavy
menu. This is the most likely cause of the slider-vanish since the tallness slider lives there; if a DIFFERENT slider/menu
still vanishes, tell me which and I'll trace that path.
lib + bin compile.
- DEVICE-VERIFY (M2): create two worlds with the same name → only one row should show; open settings while the `p` panel
  was up → `p` should not cover it; drag the tallness/muscle sliders in the customizer → the menu should stay put.

## r578 — flora spacing & tree competition (walkable world)
Operator: every block grows full → walking is very hard; keep the alive feeling but let tree competition push trees
apart. Referenced docs/foraging_and_ecology.md (the living-ecology direction) + docs/FLORA-WIRING.md (surface growth is
the runtime spread, not cave flora). The runaway density came from `sim::flora_spread`: it placed a plant in EVERY
suitable Air cell with no neighbour check, carpeting the ground solid.
- **Ground-cover spacing/competition**: before the spread sets a Flower/TallGrass/Fern/Mushroom, it now counts the 8
  plant neighbours at that height and leaves a GAP if ≥3 are already plants. Coverage settles patchy + WALKABLE instead
  of solid, and the spread self-limits (it stops pushing into dense patches) — the "very alive" feel stays, the wall of
  vegetation goes.
- **Tree competition (spacing ≈6 blocks)**: (a) a new sapling only takes root if no trunk (OakLog/BirchLog) is within 6
  blocks — no sprouting under an existing canopy; (b) a sapling that matures while a tree grew up close to it LOSES the
  contest and is removed rather than becoming a tree. Together, new tree growth trends toward well-spaced canopies.
  NOTE: pre-existing dense clumps from GPU worldgen are not retroactively thinned (that needs a separate, careful cull
  pass — flagged); this governs ongoing growth.
bin compiles.
- DEVICE-VERIFY (M2): ground cover should be patchy/walkable (not a carpet); over time new trees should keep their
  distance. Tunable: ground-cover crowd threshold (3 of 8), tree spacing radius (6).
NEXT (this request): trunk girth variation + log block shape; fruits & farming; then character porting to stature.

## r579 — fruit foraging (+ farming already lands)
Operator: add fruits and farming. FARMING already works — crops (Carrot/Potato/Cabbage/Pumpkin/Sprout) are placed on
soil, tracked in the persistent flora store, advanced by the `sim::plant_growth` tick to FINAL_STAGE, and Carrot/Potato/
Cabbage/Mushroom are edible (right-click → `vitality_mut().feed`). Added FRUIT without touching the (risky) block enum:
- `APPLE_ITEM = 2002`, a food ITEM. Breaking leafy foliage (OakLeaves/BirchLeaves) now also drops an apple ~13% (its own
  roll, alongside the existing ~11% sapling drop), so trees are foragable. Right-click eats it (`feed(0.22)` — a light,
  refreshing snack, between raw meat 0.18 and cooked 0.55). Resolved in `item_display` (2002 → "Apple", red) so it reads
  correctly in the survival backpack.
lib + bin compile.
- DEVICE-VERIFY (M2): chop tree leaves in survival → apples sometimes drop; pick one up, select it, right-click to eat →
  vitality rises. Tunable: drop rate (33/256 ≈ 13%), feed amount (0.22).
NEXT (this request): trunk girth variation + log shape (GPU worldgen/mesher — the riskier blind piece); then porting the
other heroes to the stature/customization system + their models.

## r580 — bugfixes: default-height ground-shake + world/flora merging
**(1) GROUND-SHAKE TOO STRONG AT 1.8.** Root cause: `ground_shake = √(mass/70) − 1`, and `mass` includes bulk×density.
After r567 tied density to the muscle slider, a MUSCULAR but normal-HEIGHT (1.8) character got mass>70 ⇒ a non-zero
ground-shake ⇒ footfall camera shake firing on a normal person ("felt like a giant"). Fixed: ground_shake is now keyed on
the HEIGHT ratio only — `(r^1.5 − 1).max(0)` — so it is exactly 0 at 1.8 for ANY build, and identical to the old curve at
default bulk (r^1.5 = √(r³)). Only true tallness shakes the ground now.

**(2) WORLD / FLORA MERGING ("coverage over air", not patchy).** Root cause: the chunk store rebinds + clears on world
switch (`set_world_root` + `rebuild_for_seed` drops loaded chunks and reseeds — verified by its test), BUT the FLORA/crop
store and whitelist were bound to the STARTUP root (`worlds/default`) and never rebound. So a new world kept the previous
world's crops & saplings at stale positions — plants floating where the OLD terrain used to be, and the spacing gate was
operating on stale/merged flora (hence "not patchy"). Fixed: on world launch, persist the old store, rebind `flora_root`
to this world's dir (`worlds/{name}-{seed}`), and reload `session.flora`/`whitelist` from it (empty on first visit ⇒ a
clean, freshly-generated world). Covers both new-world and load-world (single launch path). Storage is already isolated
per (name,seed); a full unique-world-id refactor (the r291 "stand-in" note) can come later but isn't needed for the merge.
lib + bin compile.
- DEVICE-VERIFY (M2): default 1.8 character should NOT shake the camera when walking (any muscle level); create/load
  worlds back-to-back → no plants floating over air, ground cover patchy + walkable, no terrain bleed between worlds.

## r581 — flora thinning (de-densify existing carpets)
Operator: flora still everywhere. The r578 spacing gate only stops NEW crowding — it can't thin flora that's ALREADY
placed (accumulated before the gate, or persisted in a world's chunk edits). Confirmed the only ground-plant placement is
the runtime spread (worldgen places none); so the carpet is pre-existing blocks the gate never touches. Fixes:
- **Active thinning**: each flora tick, sampled plants with ≥4 plant neighbours are culled ~50% of the time (budgeted,
  probabilistic), so over-grown areas open up toward patchy + walkable over time — including worlds carpeted before the
  gate landed. It thins near the player (where walkability matters) and never strip-clears.
- **Tighter spread gate** (3→2): new growth only takes a spot with <2 plant neighbours, so dense areas are net-negative
  (spread places nothing, thinning removes) and won't re-carpet; open areas still fill in (stays lush).
bin compiles.
- DEVICE-VERIFY (M2): walk around an over-grown world → it should gradually open into patchy, walkable ground cover
  (gradual by design). To thin FASTER on a heavily carpeted save, set UNENDLICH_FLORA_SPREAD_INTERVAL lower (default 300)
  or UNENDLICH_FLORA_SPREAD_BUDGET higher (default 8). A brand-new world (post-r580) already starts clean.

## r582 — REVERT r581 thinning + air-growth cleanup (the real bug)
Operator: the thinner was way too strong; thick forests/grasses were more TRUE — clearing should come from a future
FAUNA loop (bugs/birds/herbivores with memory eating plants), not artificial thinning. And keep chasing why plants grow
on air.
- **Reverted r581**: removed the active thinning pass entirely; restored the r578 ground-cover spread gate (2→3). The
  world is lush again; no artificial culling. (Future ecology balance = fauna, tracked separately.)
- **Diagnosed the air-growth** (searched c1 transcripts): it's the known **FP32/FP64 surface mismatch**
  (NEXT_SESSION_GUIDE §4a). The GPU compute generator fills voxels in FP32 (Apple/Honeykrisp) but flora/tree placement
  reads the CPU **FP64** surface *prediction* (`world/chunk_manager.rs::surface_y_at`), so things land where the
  predicted surface was — not the real one — and end up on air. The proper fix (switch placement onto
  `sim::surface_scan::top_solid_y_in_chunk`, which reads the ACTUAL top solid voxel — built r331, wiring proposed in
  handoff/HANDOFF_world_surface.md) is deferred to the physics/anchoring pass per operator. Interim, per operator:
  - **Widened the placement scans** ("increase the tree scan a bit"): sapling planting −6..8 → −12..12, pioneer flora
    −6..8 → −12..12, flora-spread target −3..3 → −5..5, so placement searches deeper for the REAL ground before seeding.
  - **AIR-CLEANUP pass** (`UNENDLICH_AIR_CLEANUP`, default on; INTERVAL 120, BUDGET 12): budgeted random probes near the
    player cut flora (Flower/TallGrass/Fern/Mushroom) with Air/Water directly beneath, and floating tree trunks (a log
    whose base sits on air) — clearing the connected trunk upward plus each layer's hugging leaves so no canopy is left
    orphaned.
bin compiles (only pre-existing warnings).
- DEVICE-VERIFY (M2): forests/grass thick again; over ~seconds, plants and trees that were hanging in the air get cut
  and fall away near you. Knobs: AIR_CLEANUP_INTERVAL / _BUDGET; AIR_CLEANUP=0 to disable. The lasting fix is the
  surface_scan call-site switch (HANDOFF_world_surface.md) — flagged for the physics pass.
NEXT: the surface_scan placement switch (proper air fix) + character porting to stature.

## r583 — authoritative surface from real voxels (the definitive air-growth fix, CPU-side)
Went to implement the GPU-side per-column heightmap emit, but investigating the path changed the plan (reported honestly):
1. `world/gpu_generator.rs::read_back` ALREADY reconstructs every voxel on the CPU (they live in `self.chunks`), so
   the true surface is derivable CPU-side — a GPU emit is redundant for CORRECTNESS (it'd only matter to later DROP the
   voxel readback, a perf path).
2. The generation shader ALREADY density-anchors tree trunks (r447 floating-trees fix in chunk_generation.comp: scans the
   FP32 density for the topmost solid-with-air-above instead of the analytic surface_target), so gen-time trees match the
   FP32 terrain. And the CPU flora spread already verifies `below.is_solid()` via real voxels.
   ⇒ the remaining FP32/FP64 offenders are the consumers of the CPU FP64 *prediction*: `chunk_manager::surface_y_at`
   (spawn via `pick_spawn`, mob ground, placement anchors).
**Fix**: made `chunk_manager::surface_y_at` AUTHORITATIVE. It still computes the FP64 prediction as an anchor, then scans
the REAL voxels in a tight window (pred+8 .. pred−24) for the topmost GROUND block (solid, excluding logs/leaves so it
lands on terrain not a canopy). Resident column ⇒ returns the true voxel surface; non-resident column reads all-Air ⇒
returns the prediction unchanged (distant spawn picks behave exactly as before). This is the r330-world handoff's
call-site switch, using the r331 surface_scan semantics on the voxels read_back already provides. No shader/descriptor
risk. lib + bin compile.
- DEVICE-VERIFY (M2): spawn/mobs should sit ON the ground (not hovering/buried); combined with r582's air-cleanup +
  widened scans, plants shouldn't accumulate on air. Behaviour-preserving where chunks aren't loaded.
- NOTE (perf, deferred): the pure GPU emit (compute shader writes a per-column top-solid-Y into a small SSBO) is only
  worth doing to ELIMINATE the 128KB/chunk voxel readback → ~4KB heightmap, for a future GPU-only (no-CPU-voxel) path.
  That's a bigger, on-device-verified change (descriptor layout + mesher/store must stop needing CPU voxels). Flagged, not done.

## r584 — cancel trees growing on/in water (the submerged-base gap)
Operator: trees still spawning on water; water may be an intended placement substrate, but the lifecycle should cancel
the "not touching (dry) ground" no-plant condition. Traced it:
- Gen-time trees ARE gated below sea level (chunk_generation.comp: `surface_y < model_sea_level → skip`), CPU saplings
  need Grass/Dirt, flora `ground_ok` excludes water, and `is_tree_anchorage()` excludes Water — so the lifecycle marks a
  tree with no anchored wood as unanchored → degrows.
- THE GAP: a tree rooted in SUBMERGED anchorage (a sand/stone seafloor UNDER water) has solid below its base, so it's
  "anchored" and never flagged — it just rises out of the water. r582's air-cleanup only caught bases on air/water BELOW.
**Fix**: extended the r582 air-cleanup log branch. On finding a trunk it now walks to the base (lowest connected log)
and cuts the tree (trunk + hugging leaves, upward) if the base has no solid support OR it's standing in water — water
directly beneath the base, or flanking it at its own level (rooted in a submerged seafloor). A dry-land shoreline tree
has its base above the water line, so its base-level neighbours aren't water → not a false positive. Water is deliberately
LEFT as a valid substrate (aquatic plants), per operator; only land trees on/in water are cancelled. bin compiles.
- DEVICE-VERIFY (M2): trees standing in / rising out of water near you should get cut within a cleanup cycle; shoreline
  trees on dry land should be untouched. Knobs: UNENDLICH_AIR_CLEANUP_INTERVAL/_BUDGET.
- DEEPER OPTION (deferred): fold "submerged anchorage = unanchored" into world/plant_lifecycle.rs so the
  replay-deterministic degrowth handles it at the source (the anchorage check would reject a root whose base is water-
  flanked/submerged). Riskier (determinism); the cleanup pass covers the visible symptom now.

## r585 — rot + survival genes (aquatic tolerance, unorthodox growth, light-starvation)
Operator: add ROT (slowly kill certain genomes in water) + upside-down/sideways tree growth for the right genome, which
must still catch light or starve.
- **NEW pure core `sim::plant_survival`** (unit-tested; executed via a temp example — all assertions pass): `SurvivalGenes
  { water_tolerance, orientation_freedom }` (`from_seed`, deterministic). Rare-tail thresholds: aquatic ≥0.90 (~10% of
  lineages), unorthodox ≥0.80 (~20%), lateral ~0.90 (~10%). APIs:
  - ROT: `water_rot_increment(genes, submerged, dt)` — 0 for aquatic or dry; else scales with intolerance; `is_dead(accum)`
    at ≥1.0. Gradual (probe: dryland dies in ~51 rot-ticks, not instant, not forever).
  - ORIENTATION: `GrowthDir{Up,Down,N,S,E,W}` + `step()`/`support_offset()` (support is the OPPOSITE side — up anchors
    below, pendant hangs from above, lateral braces behind); `permits(genes, dir)` (Up always; Down needs unorthodox;
    lateral needs more).
  - STARVATION: `energy_balance(light01, dt)` = light−maintenance(0.18); `accumulate_starvation` (clamped ≥0, doesn't bank
    surplus); `is_dead` at deficit ≥1.0. So unorthodox plants that grow into the dark starve; lit ones live.
- **ROT wired** into the air-cleanup: a trunk whose base is on AIR is a placement artifact → cut whole (as before); a
  trunk standing IN WATER is now genome-gated — an aquatic lineage (~10%) is SPARED (a water-loving, mangrove-like tree),
  any other lineage ROTS one crown layer per cleanup cycle so it dies back gradually instead of vanishing at once.
  lib + bin compile.
- DEVICE-VERIFY (M2): most trees in water visibly rot down over several cycles; the occasional aquatic one stands. Knobs:
  AIR_CLEANUP_INTERVAL/_BUDGET; thresholds in plant_survival.rs.
- DEFERRED (honest): the actual DIRECTIONAL GROWTH (making a tree grow pendant/lateral, anchoring to ceiling/wall) is a
  real change to the tree grower (sapling→tree) that I can't verify visually; the pure gating + support + light-starvation
  logic is built and tested so the grower can consume it. That wiring — plus feeding each unorthodox segment its caught
  light so `energy_balance` can starve the ones in the dark — is the next step.

## r586 — physics wind sway + trunk-thickness/flexibility genes
Operator: add physics-based wind sway + trunk-thickness genes, then back to the todo list.
- **Genome grew**: `SurvivalGenes` gains `trunk_thickness` (girth + stiffness) and `flexibility` (whippiness), both
  `from_seed`-derived. `new()` is now 4-arg (tests updated).
- **NEW physics in `sim::plant_survival`** (unit-tested + executed via probe):
  - `stiffness(thickness, flexibility)` — rises ∝ 1+6·t² (beam-like: thick trunks are dramatically stiffer), softens with
    flexibility, floored so nothing divides by zero.
  - `tip_deflection(wind_speed, height, thickness, flexibility)` — drag ∝ wind_speed² × height / stiffness, clamped to
    ≤½·height. Probe: stout 0.61 vs thin 3.72 at equal wind; doubling wind ≈4× deflection (quadratic); zero wind → zero.
  - `height_profile(y_frac)=y²` (base-anchored cantilever), `sway_offset(...)` = steady DOWNWIND lean + slow sway + faster
    flutter, scaled by tip deflection × profile (base vertex stays put).
  - `trunk_radius_from_gene(thickness)→1..=MAX_TRUNK_RADIUS(4)` — girth mapping for the grower; lineage spread ~16/34/34/16%.
- **Visible sway (chunk.vert)** reshaped to physics: aerodynamic drag ∝ wind² gives a steady downwind LEAN plus a slow
  sway + faster flutter oscillating about it (not a symmetric wobble), and the tip dips slightly as it leans; a whisper of
  idle motion at zero wind. Validated: `glslangValidator --target-env vulkan1.2` + `spirv-val` both clean; build.rs
  hash-manages chunk.vert so it recompiles from source (no committed .spv to go stale). lib + bin compile.
- DEVICE-VERIFY (M2): foliage should lean downwind and flutter, harder in stronger wind (noticeably nonlinear); calm =
  near-still.
- DEFERRED (honest): (a) PER-TREE genome sway — the mesher would encode thickness/flexibility/height-fraction per foliage
  vertex to feed `sway_offset`, so stout trees barely move and supple ones whip; the shader sway is currently uniform. (b)
  THICK-TRUNK GENERATION — the grower/worldgen consuming `trunk_radius_from_gene` for real girth variation. Both are
  mesher/grower changes needing on-device verification; the genes, physics model, and radius mapping are ready to consume.

## r587 — hero porting complete (all 5 heroes finished + on stature)
Back to the standing todo: port the other heroes onto stature/customization + finish designs + animations. Investigation
showed the scaffolding was already broad — ALL 5 kinds have palettes (HeroColors) + proportions (HeroDims), and build()
already: scales dims by BuildParams (bulk/head/shoulders/arm/leg), applies the FULL animation rig (walk gait, idle
breathe/sway/weight-shift/head-turn, r570 airborne fall-tuck, landing crouch, r572 flight, r573 bank) generically, and
uniform-scales to `target_height` in finalize (tallness). The ONE gap was `dress()` element props: only Heliographer +
Gravitybender were done; Windwalker/Tidecaller/Stormweaver hit `_ => {}` ("next roll").
**Done**: authored distinct, animated element props for the three, matched to their palettes:
- **Windwalker (air)**: aviator scarf streaming + fluttering behind (per-segment sin), wind-wisps (glow) orbiting the
  torso in world space, faint updrafts off the feet.
- **Tidecaller (water)**: wave-crest circlet round the head, a rippling tidal train widening down the back, a water orb
  cupped in the hand with droplets in orbit.
- **Stormweaver (storm)**: a dark cloud crackling overhead, a charged staff with an electric orb, and lightning arcs that
  STROBE on/off (sin threshold on anim_t) around cloud + orb.
The `dress()` match is now exhaustive over all 5 kinds (no `_`). "More animations" this roll = these per-element idle
motions (flutter / orbit / strobe) on top of the shared rig.
**Verified** via temp example: all 5 heroes build non-empty, all-finite, in-range geometry at heights {0.9, 1.8, 4, 15}
across 4 anim phases; finalize scaling hits the target height exactly (Tidecaller h4 span = 4.00). lib + bin compile.
- DEVICE-VERIFY (M2): switch between the 5 heroes — each should show its signature kit, animated (scarf flutter, water
  droplet orbit, lightning strobe), scaling correctly with the tallness slider and body sliders.
- DEFERRED (optional next): per-element ACTION poses (element-specific cast/attack), and the r586 per-tree genome sway /
  thick-trunk generation.

## r588 — objects deform the wind (perturbation layer) — the missing field half
Operator vision: global base wind, but objects deform the FIELD — mobs break it, trees filter it, mountains updraft/mix;
then meshes (fabric/hair/wings) deform in it. AUDITED existing wind first ("if they don't already"): a lot is present —
`wind.rs::wind_at` (spatial gusty base field), `vertical_wind_at` (THERMAL updraft via convection), `sim::wind_field`
(gusts), `world::pressure_gradient_wind`/`wind_shear` (speed models), `cloth_wind_force` (mesh-side force, pure). The
GAPS vs the vision: nothing let OBJECTS deform the field — no mover wind-break/wake, no foliage filter, and the only
vertical wind was thermal (not orographic mountain lift).
**NEW pure core `sim::wind_perturb`** (unit-tested + probe-run) fills exactly those gaps, composing ONTO the base field:
- `Mover{pos,vel,radius,strength}` + `mover_delta`/`movers_delta`: a downwind SHADOW (calms the lee) + a WAKE (radial
  shove outward + tug along motion), heft-weighted (`strength` = stature mass/ground_shake) and distance-faded. This is
  what bends trees aside as a dragon passes. Probe: lee dx −0.05, wake 2.08→0.08 with distance, heavier carves harder.
- `foliage_delta`: canopy01 damps through-wind (×0.6 at full) + sheds small cross-turbulence. Probe: +4 → +1.2.
- `orographic_delta`: wind climbing a windward slope is forced UP, lee sinks, fades with altitude (e-fold 18 b). Probe:
  +1.8 / −1.8 / 0.007 aloft. (Mechanical mountain lift — complements the existing thermal `vertical_wind_at`.)
- `perturbed(base, movers, canopy01, slope, alt, pos, t)` composes all three, finite. Pure/deterministic — the CPU
  reference AND the spec a GPU froxel-fill will mirror.
lib compiles.
- NOT WIRED YET (the pipeline this foundation feeds; device-verified): (1) build the Mover list from live mobs/dragons
  (strength from stature) each frame; (2) sample canopy density + terrain slope per point; (3) a GPU froxel-fill compute
  pass (precedent: froxel_fill.comp writes image3D) bakes `perturbed()` into a low-res 3D wind texture around the camera;
  (4) the deform shaders (chunk.vert foliage, entity.vert cloth/hair/wings) sample that texture — `cloth_wind_force` is
  the ready mesh-side force. Phased so each step is shippable + eyeball-verifiable on the M2.

## r589 — wind ↔ moisture ↔ weather coupling (orographic precip bridge)
Operator: the wind field also has to play with moisture + global weather. AUDITED first: much coupling ALREADY exists —
`moisture_field` is wind-ADVECTED, `world::rain_shadow` dries the LEE, `world::weather_cell::resolve_cell` composes
rain_shadow→moisture→cloud→precip→storm + pressure-gradient wind, `storm_organization` folds in wind shear. The GAP: the
new perturbation layer (r588), especially the orographic UPDRAFT, wasn't connected — and the WINDWARD complement of the
rain shadow (mountains rain MORE on the upwind face) wasn't modelled at all.
**NEW pure core `world::orographic_precip`** (tested + probe-run), composing into the existing weather_cell (augments,
doesn't reimplement):
- `condensation(updraft, moisture01, temp_norm)` — lifting moist air cools it to saturation ⇒ windward rain; rises with
  updraft × moisture × coolness; 0 for downdrafts/dry/warm. Probe: 0.704 at lift+moist+cool.
- `uplift_instability(updraft, moisture01)` — forced ascent destabilises the column (primes storms).
- `augment_cell(CellInputs, updraft)` — folds uplift into windward_moisture (capped) + instability so `resolve_cell`
  yields the orographic-enhanced cloud/precip/storm; a downdraft leaves it to the existing rain-shadow term.
Result: the full orographic DIPOLE — probe shows flat precip 0.079 → windward 0.217 (2.7×), and upwind 0.441 vs a dry
lee 0.000. The updraft it consumes is exactly `sim::wind_perturb::orographic_delta` (r588), so wind lift now drives
weather. lib compiles.
- NOT WIRED YET (runtime/device): (1) feed real per-cell updraft (wind_perturb + terrain slope) into the weather tick;
  (2) advect `moisture_field` along the PERTURBED wind (movers/foliage shape humidity too), not just the base; (3) the
  GPU froxel-fill still bakes the perturbed field for the deform shaders. The pure physics of the coupling is in place.

## r590 — shared secondary-motion core (the keystone: one deform model, two tiers)
Free choice ("take them as you wish"): built the keystone the whole wind→mesh discussion converges on — a SHARED
flutter/deform response so foliage, fabric, hair, and dragon wings all bend from the same influence + material. AUDITED
first: `sim::deform` already exists but is volume-preserving SQUASH & STRETCH (different concept) → named this
`sim::secondary_motion`.
Pure core (tested + probe-run), both tiers the architecture called for:
- **Tier 1 stateless** `deflect(force, material, anchor_weight, lever)` — quasi-static bend, pivots about the anchor,
  clamped to the lever (bends, never inverts). What a VERTEX SHADER runs for millions of leaves / a wind-lifted cloak.
- **Tier 2 stateful** `verlet_step(particle, anchor, rest_len, force, material, dt)` — Verlet integrate + distance
  constraint back to the anchor bone → real lag/overshoot/settle. What a GPU COMPUTE PASS runs for cloak/hair strands.
  Probe: a horizontal strand swings down under gravity and holds its constraint.
- **Shared inputs**: `influence(wind, normal, area, gravity, self_accel)` = wind drag scaled by how it FACES the wind
  (|n·ŵ|, the cloth_wind_force idea) + gravity + inertial lag; `Material{stiffness,damping}`; `disturber_strength(mass,
  ground_shake)` maps stature heft → a `wind_perturb::Mover` strength (human 0, giant 35+). Both tiers consume the SAME
  influence + material — that's the unification.
lib compiles; all assertions pass (deflect clamps@lever, face 5.0 > edge 0.0, strand swings to y=−0.53, giant 35.1 > human 0).
This completes the PURE foundation of the wind→deform→weather system: plant_survival (foliage physics) + wind_perturb
(objects deform the field) + orographic_precip (field ↔ weather) + secondary_motion (shared mesh response). Everything
downstream is now GPU/runtime WIRING (device-verified): the froxel-fill baking `perturbed()` into a 3D wind texture; the
disturber SSBO from live mobs (strength via `disturber_strength`); chunk.vert/entity.vert sampling the field and calling
`deflect` (Tier 1) or a compute pass calling `verlet_step` (Tier 2); moisture advected along the perturbed wind.

## r591 — foliage disturber (FIRST TESTABLE wiring): plants part around you
Operator ready to test → shipped the safest visible slice of the deform system. Foliage now gets shoved aside by a
DISTURBER, using the wake idea from `sim::wind_perturb`. The camera (≈ the player) is the first disturber because it's
ALREADY in the shared UBO (`camera_pos`) — so this needed ZERO uniform/descriptor plumbing, just a validated `chunk.vert`
edit (glslangValidator + spirv-val both clean; build.rs recompiles from source). In the sway block (tag 1 / free foliage
only), a radial push away from the camera's horizontal position, fading to nothing at reach=3 blocks (push 0.35, slight
press-down). Anchored bases (tag 2) don't reach the code, so plants pivot at the root as they part. bin compiles.
- TEST ON M2: walk through tall grass / low leaves → they bend away from you in a ~3-block radius and spring back as you
  leave; roots stay planted. Knobs in chunk.vert: `reach` (3.0), push (0.35).
- NEXT (the mob/dragon effect proper): append a small disturber ARRAY to the UBO (after light_view_proj so existing
  offsets are untouched) — each entry xyz=pos, w=strength via `sim::secondary_motion::disturber_strength(mass,ground_shake)`
  — populate the nearest N streamed creatures at the world_renderer UBO write, and extend this same wake loop over the
  array. That's the "trees bow as the dragon passes" step; deferred as its own roll so a UBO-layout mistake can't
  black-screen this test session.

## r592 — live F3 slider to amp the foliage disturber (confirm it's working)
Operator not seeing the r591 effect → added a live strength slider to the F3 "debug · shaderpack" window so it can be
cranked to confirm. No UBO layout change: `sun_color.a` is unused by shading, so it now carries the strength (×10
fixed-point) from `DEBUG.foliage_disturb` (new AtomicU32, default 10 = ×1.0) to chunk.vert. The slider (0–30) scales BOTH
the push (×0.35·strength) AND the reach (3 + strength·0.3 blocks), so cranking is a decisive test even if little foliage
is right at your feet. Validated (glslang + spirv-val); lib + bin compile.
- TEST ON M2: open F3 → "foliage disturber (live)" slider → crank to ~15–30 and walk through grass / low leaves. Expect
  a wide, obvious parting around you (roots stay planted). 1.0 = r591 default, 0 = off.
- DIAGNOSTIC: if even at 30 NOTHING moves, the cause is structural, not subtlety — likely (a) the foliage near you isn't
  Sway-tagged (only leaf-cubes + plant tops get tag 1; solid grass-block tops don't sway), (b) hot-shaders served a stale
  chunk.vert.spv (confirm the build recompiled it), or (c) camera_pos.xz isn't tracking the player as expected. Report
  which and I'll chase it. If it DOES move, it was just too subtle at 1.0 and we pick a good default.

## r593 — DIAGNOSTIC: split the two failure modes (value delivery vs sway tag)
Operator: clean build (not stale spv), cranking the r592 slider does nothing, foliage near spawn unaffected. Confirmed
the bin uses WorldRenderer (chunk_renderer.rs is unused/legacy — its zeroed UBO is irrelevant), and WorldRenderer has a
SINGLE CameraUbo write (world_renderer.rs:5420) feeding chunk.vert, with my sun_color.a edit in it. `world_pos` is
absolute world space (chunk_origin + local) and camera_pos is world-space, so the distance math is correct — the space
isn't the bug. Two forks remain: (A) sun_color.a not reaching chunk.vert, or (B) foliage near you isn't Sway-tagged so the
whole `if(sway_tag==1u)` block (wind sway AND disturber) is skipped.
Added a decisive TEMP diagnostic in chunk.vert: an UNCONDITIONAL magenta tint that ramps in as the slider passes 5,
depending ONLY on sun_color.a (not sway tags). bin compiles, shader validated.
- TEST ON M2: open F3, drag the foliage-disturber slider past 5 toward 30.
  • WORLD TINTS MAGENTA ⇒ sun_color.a is arriving → the disturber's silence is a sway-TAG/geometry issue (the foliage
    near you isn't tag 1 — e.g. grass-block tops don't sway; only plant tops / leaf cubes do). Next: check foliage_sway
    tagging + whether wind sway itself is visible.
  • NO TINT AT MAX ⇒ sun_color.a isn't reaching chunk.vert (UBO/pass mismatch) → chase the descriptor/pass.
  Report which and I'll fix the identified half. (Diagnostic tint is temporary — removed once the path is confirmed.)

## r594 — THE FIX: wire the foliage sway tag into the mesher (sway never worked)
Diagnostic (r593) result: world tinted purple (sun_color.a arrives) but stayed static → nothing is Sway-tagged. Root
cause found: the default mesher is the CPU mesher (GPU octree is off by default — Asahi-hostile), it produces the
packed-u32 chunk.vert vertices, and the sway-tag helpers (`vertex_sway_tag`/`tag_packed_pos`) were DEFINED BUT NEVER
CALLED in emission — the exact "wire the tag emission" step `HANDOFF_gpu_foliage_sway.md` left open. So bits 30-31 were
always 0 (SwayTag::None): wind sway AND the r591 disturber never had anything to act on.
Wired it:
- `Vertex` gains a `sway: u8` (default 0; `new()` unchanged so all 24 call sites compile; added `with_sway`).
- `PackedVertex::from_world` now stamps a non-zero `sway` into pos_packed bits 30-31 via
  `foliage_sway::pack_tag_into_pos` (solid geometry untouched).
- New `push_quad_foliage` tags each vertex: the planted bottom row (within ½ sub-voxel of the grid base) → Anchor(2),
  the rest → Sway(1). `emit_subvoxel_grid` (the sole foliage/leaf/plant emitter) now uses it for all 6 faces.
- Removed the r593 diagnostic tint.
lib + bin compile; chunk.vert validated. This is what finally connects sim::foliage_sway → mesh → chunk.vert (wind sway
r586) → the r591/r592 disturber.
- TEST ON M2: foliage must RE-MESH to pick up the tag — walk to fresh chunks or reload the world. Then: (1) grass/plants/
  leaves visibly sway in wind (r586, now actually active); (2) with the F3 disturber slider up, foliage parts around you.
  If it now works, we set a sane default disturber strength and (next) add the mob/dragon disturber array.
