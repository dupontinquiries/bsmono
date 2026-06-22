# unendlich — TODO (deferred work, sequenced after the wiring fix)

## 1. Generative AAA content from art targets (docs/art-targets/) — AFTER wiring
Realize these targets as mobs / shaders / abilities, in tandem with the spec docs:
- **Mobs**: draco-rex; bioluminescent terror (emissive translucent jaw + glowing
  eyes, misty-forest hunt); metal-rex + two-headed blood-rex (dusk predators);
  the Ryujin firedragon trio (attack poses).
- **Bending / abilities**: airbending (swirling streamer VFX), firebending
  (speedster, coastal-city), stonebending (prehistoric dino predators).
- **Shaders**: volumetric fog + god rays (see "shader fog and rays examples"),
  bioluminescence/emissive, translucent membranes — overlaps dispatch-r82 threads
  (mob-eyes-glow, wing-membrane, physics-debris, bending VFX).
- **Bosses / scenes**: player vs Red Vector; player vs Ironheart (Ryujin
  Observatory Pit); Ironheart vs the Ryujin trio.
Approach: each maps to (a) a forge/genome archetype, (b) a shader pass, (c) an
ability/bending discipline — built and verified via the headless render collage
before any GPU build. Do NOT start until the menu→world wiring is verified.

## 2. -fullgame should start with no streamed world — ideally a small BAKED world
Today the bin builds `TerrainGenerator::new(WORLD_SEED_DEFAULT, …)` at startup
(~line 315) and streams the big procedural world behind the menu. Desired: start
with NO big world — a small baked/fixed world (or empty) under the title menu, and
only generate the real world on "Create". NOTE (blocker): the terrain generator +
field WorldModel are built once at startup before the menu; starting baked / making
Create regenerate from the chosen seed needs the generator lifecycle to be
(re)creatable post-menu (the real unit behind "regenerate worldgen from spec.seed").

## 3. [FIXED r88] Forge rigging bug — glowy back-spikes mis-rigged (found via collage)
The dragon_walk_collage (r85) shows the emissive back-spike (glow) boxes animating
as if they were feet — i.e. `assign_joint` in dragon_forge::build is binding rear
glow boxes to a LEG joint instead of the spine/back, so they swing on the gait
phase. Fix: exclude glow/back boxes from leg-joint assignment (or bind dorsal
boxes to the spine joints). FIXED: `assign_joint` no longer blanket-pins glow→J_HEAD; only head-region glow
(eyes) pins to the head, dorsal/accent glow rides the nearest body joint, and glow is
never a leg. Confirmed via the collage (back-spikes hold still, legs swing) + the
joint diagnostic (HEAD 20→8, dorsal glow → SPINE/TAIL); 17 forge tests pass.

## 4. [FIXED] Forge "dark slab" — REMOVED the dark-throat boxes (found via collage)
The big dark box on the head is TWO mouth-flagged boxes at center≈[0,3.19,~2.82],
half=[2.63,0.54,0.27] — i.e. ~5.3 units WIDE (spans the whole creature), thin, flat,
rigged to J_HEAD so it never flexes. A maw cavity shouldn't be body-wide; this reads
as the misplaced "wingskin" slab. Fix in `build_solids`/`voxelize`: the mouth/maw solid
half-width should track the head/jaw width, not the body. (If a wing MEMBRANE is wanted
here instead, model it as J_WING-rigged boxes so the r82-03 membrane shader + wing flex
apply.) Re-check via `cargo run --example sim_collage` (the dark slab should shrink to a
head-width maw, or move to the wings).

## 5. Forge: more FACE joints for animation
Skeleton is J_HEAD + J_JAW only. Add finer face joints (brow/eyes/cheeks/jaw-tips) so
expressions + finer head animation are possible. Touches `skeleton`, `assign_joint`
(route face boxes by region), `skin` (animate the new joints), FORGE_JOINTS, and the
joint tests. Render the joint diagnostic to verify the new regions bind sensibly.

## 6. Forge: wings are under-modeled (found via top-down collage)
8 wing boxes total, 4 per side (WING_L #15/17/18/19, WING_R #48/50/51/53),
correctly rigged + symmetric but sparse: splayed out/back to x≈±6 with gaps and
NO membrane geometry. In the side view they collapse to a back-smear; the top-down
(dragon_joints_top.png) shows the real sparse fan. Fix options: (a) add a proper
wing-bone fan + membrane boxes rigged J_WING so the wing reads as a sheet; or (b)
emit membrane quads between wing bones for the r82-03 membrane shader to stretch.
Also note: back-leg boxes (#49/52, #14/16) sit at wing-distance (x≈±6) rather than
under the body — eyeball in-game; assign_joint's leg branch (lateral>0.18 && ty<0.42)
may be catching far-lateral haunch boxes. Verify via the top-down collage.

## 7. Forge locomotion — drag/claw zones, gait modes, wing tuck, foot planting
[FIXED r94: the "pulled-in" wingtips/wing-base/tail were a lerp-the-rotation chord
artifact — skin now rotates each box by (w × angle) along the ARC, so limbs hold
their true radius. 17 forge tests pass; verified in dragon_walk_top.png.]
REMAINING (the operator's drag/claw-zone design):
- CLAW ZONES (foot planting): [PARTLY FIXED r95] `leg_swing` now sweeps the stance
  foot at CONSTANT speed (linear stance, eased swing) instead of a sine — the plantable
  profile (test: leg_swing_plants_then_swings). REMAINING for ZERO world-slide: the bin
  must match gait_phase advance to body speed (gait_phase is 'advanced by distance'), and
  ideally feed a body-advance offset into ForgeAnim so skin counter-translates the planted
  foot. Foot LIFT during swing needs a knee DOF (currently single-pivot legs can't lift).
- GAIT↔WING COUPLING: [VERIFIED r96] WingPose::Folded tucks the wings (no flap) —
  confirmed in dragon_walk_top.png. REMAINING: the BIN must set wing_pose from
  locomotion state (grounded walk → Folded; flight → Spread/beat). Compile-on-end.
- FOOT LIFT: [PROFILE DONE r96] leg_lift raises the swing foot (test
  leg_lift_clears_during_swing). Visible amplitude is currently muted because this
  model's feet sit high (the leg-placement issue below) — fixing leg placement makes
  the lift read; the lift logic itself is correct + applied.
- GAIT MODES: bipedal (wings tucked, forelegs up, walk on hind legs) vs quadrupedal
  (all 4 down, wings tucked). A `Gait`/stance enum feeding leg + wing poses.
- ATTACK/FLIGHT POSES: certain bending moves (air/lightning/water) + flight deploy the
  wings. Pose set on top of the gait.

## 8. Menu seed surface-run preview [DONE r98]
`cpu_generator::surface_run_preview(seed,w,h) -> RGBA` draws the seed's TOP-DOWN
elevation map (terrain ramp: water/sand/grass/rock/snow) (test surface_preview_is_terrain_shaped_and_seed_varying;
example seed_preview -> seed_previews.png). REMAINING (compile-on-end): the wizard
uploads the RGBA as an egui texture next to the seed field and refreshes it when the
seed changes.

## 9. seed -> worldgen LAUNCH WIRING [WIRED r98, compile-on-end]
Bin AppStep::Launch(StartLocal) now calls chunk_manager.rebuild_for_seed(init.seed,
Backend::Auto, gen_batch) + pick_spawn + moves the player to the new surface. Lib
gates green; verify on-device by launching two Fixed(N) seeds (different spawn + the
logged seed matches). Startup TerrainGenerator stays as the menu-backdrop world.

## 10. Forge belly/leg slide [FIXED r99]
assign_joint now classifies a leg only when the box is nearer a leg joint than the
trunk centerline (xz) — low ventral belly boxes near the centreline stay on the spine
instead of swinging with the gait. Test belly_boxes_stay_on_the_trunk_not_legs.
NOTE: legs still need the bin to feed body-advance for zero WORLD slide (compile-on-end).

## 11. Menu polish [PARTIAL r101]
- VOXEL TITLE [DONE r101]: sim::voxel_text::voxel_text("UNENDLICH") → extruded 5×7
  block letters (test unendlich_spells_out; example voxel_title → unendlich_title.png).
  REMAINING (compile-on-end): the menu places these as floating world blocks above
  the title scene; camera trailers away from them into the sections.
- SPATIAL AUDIO MIXER [LOGIC DONE r101]: sim::spatial_audio (mix_gains + menu layout,
  position-driven crossfade; tested). REMAINING (compile-on-end): rodio playback shell
  behind an `audio` feature + transcode the opus stems to ogg — see
  handoff/AUDIO_SPATIAL_MIXER.md.
- STILL TODO (compile-on-end, egui/render): mouse parallax on the menu, frosted-glass
  panels/buttons (extend the inventory-panel frosted start).

## 12. r102 — dragon fixes + 2nd-dispatch merge + audio feature
- BLACK BOX REMOVED: dragon_forge::build no longer emits the throat occluder (it
  rendered as a giant black slab on the head). Bleed test relaxed to tol=8 (open maw
  showing a few px beats a black box). Confirmed via dragon_walk_collage.png.
- IDLE CLOCK FIXED: creature.anim_time was NEVER advanced (frozen at 0 for every live
  creature) — the forge's idle anims (breathing, wing beat, jaw, head idle-scan) were
  dead; only gaze + verlet wing physics moved. Bin now ticks anim_time += dt per
  creature (compile-on-end). gait_phase already advances on movement (entity.rs feeds
  both into skin()).  STILL: "legs slide" when a creature is airborne/leaping
  (gait_phase forced 0 while sliding via v*dt) — that path is correct for flight but
  may read as sliding; revisit if ground walkers still slide on-device.
- 2nd DISPATCH MERGED: sim::creature_from_image (C3 surface + dragon_forge::place_surface_glow),
  sim::shader_from_image (H1 material + H2 synth_pattern_glsl, glslang/spirv-val clean),
  sim::fauna_mind (W1, registered). All green: 22 forge / 7 creature / 12 shader / 7
  fauna_mind / registry / 3 bleed.  PENDING consolidation: C1 shape_from_image +
  C2 elements_from_image (not yet folded into creature_from_image); H3 swatch/score.
- AUDIO FEATURE: `audio = ["dep:rodio"]` + optional rodio + gated audio::SpatialMusic
  (drives sim::spatial_audio by listener position). Default build green (rodio resolved
  into lock, not built). `--features ...,audio` now compiles on M2. Stems need opus→ogg.
- STILL OWED: forge wing membrane (WINGS=8 sparse, no skin for the bulk); menu voxel
  title placement + parallax + frosted panels (compile-on-end); C1/C2/H3 consolidation;
  the in-flight stale-mesh world-epoch stamp (handed to B1 bughunting).

## 13. r103 — r102 dispatch returns merged (5/6 threads)
All CHECKED (render gate green) + PROVEN (proofs regenerated here). NOT yet WIRED
(on-device bin/pass/flag) — handoffs noted:
- B1 world-epoch: world::mesh_epoch + ChunkManager.world_epoch (bumped on reset).
  4+1 tests. WIRING OWED: mesh pipeline records world_epoch() at job-issue, drops
  results where !mesh_result_is_current(job_epoch, current) before upload (fixes the
  in-flight stale-mesh flicker). BUGS_FOUND.md catalogs audited-clean bug families.
- S1 shaders: sim::creature_grime (grime factor + belly-weighted tint + grime_frag_glsl)
  + render::dynamic_lights (PointLight, gather_nearest, attenuation, apply_lights CPU ref
  + dynamic_lights_frag_glsl(N)). 6+5 tests, GLSL spirv-val clean. WIRING OWED: descriptor/
  light pass + feed grime into the forge material (docs/S1-shaders-handoff.md).
- I1 viewmodel: sim::viewmodel (held-item pose: walk bob + look sway, per-form box model,
  view/world placement). 7 tests. WIRING OWED: viewmodel draw pass + CameraBasis fill from
  the glm camera; reuse forge anim phase for walk_phase (HANDOFF_I1_viewmodel.md).
- M1 mobs: sim::mob_forage_memory (vitality + decaying food-memory + forage→flee→rest SM,
  registered service). 15 tests + registry smoke. Pure; ties to fauna_mind.
- WG1 caves: world::cave_field — DEEP expansion per cave_biomes.docx. Floor y=4 → −1024
  (~1000-block column), lower taper (no guillotine), depth-stratified morphology (shallow
  worm tunnels → mid chasm fissures → deep geothermal caverns). 11 tests; deepest carved
  y=−1023; surface integrity over 30k samples. Complementary to existing cave_carve/
  cave_network (explicit geometry) — this is the implicit multi-scale field layer.
  WIRING OWED: generator solidity decision under UNENDLICH_MULTI_SCALE_CAVES=1 calls
  cave_field::solid_after_caves(...). Off by default.
- WG2 plants: DID NOT RETURN this round — re-dispatch.
- H1 re-returned (material_from_image) — already merged in r102; no-op.

## Process note (from a return-side commentary)
Worker flagged that the dispatch side has rigor (pure-core rule, in-sandbox gates,
named 6897/3 baseline) but the RETURN side lacks per-thread verdict tiers:
checked / proven / wired / ran / kept. Every deliverable here is checked+proven but
none are wired (last-mile bin/pass/flag is concentrated at the coordinator). Consider
generating the next packet's "State of the world" from a real status ledger so each
thread sees its prior deliverable's true landing state. Tracked above per-thread.

## 14. r104 — WG2 plants merged + pivot to wired/proven snapshots
- WG2 world::flora_placement merged (7 tests): biome species mix + density + clustering,
  excludes ocean/rock/cave/underwater. Recreated from pasted diff (no patch file).
- gameplay_pov_collage example: headless first-person POV compositing the real cores —
  forge dragon grimed (creature_grime) + lit (dynamic_lights), viewmodel item in hand
  (viewmodel), flora ground (flora_placement), per-situation sky/light. Proves the cores
  integrate into a player view. Dragon is blocky (CPU painter's, not the Vulkan path).
- docs/WIRING_STATUS.md: verdict-tier ledger. Everything CHECKED+PROVEN, ~nothing WIRED.
  Next reachable rung = render/generator-layer wiring (verifiable headless), not bin.
- MUSIC still blocked: rodio can't decode opus; koYopper.opus needs opus→ogg stems first.
- NEXT VERIFIABLE WIRING: cave_field → generator solidity (compiles headless → live slice).

## 15. r105 — caves WIRED into generator + opus-native decode-once audio
- CAVES WIRED: cpu_generator::is_cave now OR's world::cave_field::cave_at behind
  UNENDLICH_MULTI_SCALE_CAVES=1 (cached). Every `density>T && !is_cave` site gets deep
  caves. examples/world_cave_slice.rs renders the ACTUAL generated world off vs on:
  +35548 carved voxels in a slice, surface crust intact (assert), the deep geothermal
  cavern visible. First truly-wired feature, verified headless.
- AUDIO (did it myself, opus-native): ffmpeg present; confirmed koYopper.opus = opus
  48k stereo 147.6s. audiopus+ogg build in-sandbox (apt: autotools + libasound2-dev).
  audio::decode_opus_file (ogg demux + audiopus) decodes ONCE -> Arc<[i16]>; engine fn
  verified on the real file (14,167,680 samples, peak 32767, matches ffmpeg). PcmLoop =
  custom rodio Source looping by index-wrap (NO re-decode — the rodio inefficiency
  avoided), 3 tests. SpatialMusic (start_single/start_spatial) + per-frame update.
  Full `audio` feature COMPILES in-sandbox (rodio/cpal/audiopus/ogg). Default build green.
  BIN WIRED (cfg audio, compile-on-end): start_single before loop + update per frame.
  Remaining: audio device to actually hear it (no hardware now); separate stems for the
  spatial mix (single track plays today). Asset at assets/music/koYopper.opus or $UNENDLICH_MUSIC.

## 16. r106 — menu voxel title WIRED (player-order: menu first)
- app::menu_view::draw_voxel_title(): the Title screen now paints sim::voxel_text
  "UNENDLICH" as solid 3D block letters (cyan→violet gradient + depth pop) in place of
  the plain text heading. Wired into the REAL menu_ui; verified via menu_snapshot
  (menu_title.png) — the same egui code the game runs. Removed stray sim/mod.rs.orig.
- Menu polish remaining (player-order, render/egui-layer, verifiable via menu_snapshot):
  frosted-glass buttons/panel (unendlich-ui has frosted Frame; menu uses raw egui buttons),
  mouse parallax backdrop. THEN in-world order: blocks/entities/textures (forge as live
  creature mesh + grime into material; flora into chunk-populate; block/entity textures),
  THEN physics (capstone — hardest to snapshot).

## 17. r107 — title CORRECTED to in-world voxel blocks (not UI text)
- Reverted the egui big-text title from menu_view (operator: didn't want it in the UI).
- world::title_structure::unendlich_blocks(origin) -> world block positions, block =
  BlockId::Crystal (pale-violet, emissive). 2 tests (upright, offset-to-origin, bounds).
- examples/title_blocks_scene.rs: renders the title as 3D voxel cubes (sky/ground,
  face-shaded, gradient) — proves the in-game-blocks look. 286 blocks.
- BIN WIRED (compile-on-end): Launch handler places the UNENDLICH monument floating
  near spawn (origin [sx-26, sy+12, sz-24]) via chunk_manager.set_block. The block
  renderer draws it — the player is greeted by the title built from real blocks.
- PHYSICS verification note (operator): structures-that-fall + cave-ins are deterministic
  state-over-time TESTS (tower -> step gravity -> assert fall; pull support -> assert
  cave-in). Easy to verify headless — do it that way when physics comes up (capstone).
- Menu polish still open (verifiable via menu_snapshot): frosted-glass buttons, parallax.

## 18. r108 — menu frosted-glass buttons
- app::menu_view::apply_frosted_style(ui): scoped egui visuals give the menu buttons
  translucent rounded glass fills, a faint light edge, and brighter hover/press. Wired
  into the real menu_ui; verified via menu_snapshot (menu_title.png).
- MENU POLISH GATING NOTE: the dramatic frosted-BLUR + mouse parallax both need a 3D
  world backdrop rendered BEHIND the menu (currently the menu is egui on flat dark; the
  fly-in is egui pieces, not a voxel scene). Building that backdrop = render the world via
  WorldRenderer under the menu + composite egui on top + a blur pass. Larger render task,
  harder to verify headless (menu_snapshot is egui-only). Deferred in favor of higher-ROI
  in-world wiring next: blocks/entities/textures (forge as live creature mesh + grime into
  material; flora into chunk-populate) — all render-snapshot verifiable.

## 19. r109 — creature grime WIRED into the forge dragon material (blocks/entities)
- Creature gained a `grime: GrimeState` field (clean() default; Debug+Clone match).
- world::entity::forge_to_vertices now applies creature_grime::apply_grime_color to the
  forge BODY boxes, belly-weighted (low boxes read as belly -> full grime; back stays
  mostly clean). Eyes (glow) + mouth keep their colours. The forge dragon is already the
  live mesh for arche=dragon, so this is the visible half of the legacy-dino fix:
  the live dragon now weathers.
- examples/forge_grime_collage.rs: same genome/pose rendered clean vs mud-wallowed
  (grime01 0.00 -> 1.00) via a CPU z-buffer raster of forge_to_vertices — belly + legs
  brown, back/neck/eyes clean. Verified.
- BIN FEED (compile-on-end): per-creature tick accumulates a modest earthy grime each
  frame (GrimeSample), so dragons gradually weather in play. Earthy default for now;
  biome-specific soil colour + water-washing can refine when testable.
- creature_grime tests still 6/6.

## 20. r110 — surface flora wired to the generator (decision layer)
- world::flora_map: region_flora(gen, x0,z0,w,d) builds a FloraCtx from the LIVE
  generator's fields (biome_at_3d, humidity->moisture, temperature, surface_y_at,
  neighbour-slope, sea_level) and calls flora_placement::place_at per cell. The
  chunk-populate seam (chunk calls it with its footprint). 2 tests.
- FIX: is_rock was `erosion>0.7` which fired on ALL mountain terrain and suppressed
  every plant; corrected to `slope>0.7` (steep bare faces). Biome species_weights
  handle which species per biome. (Probe band narrowed to -16..248 for speed.)
- examples/flora_scatter_map.rs: top-down over the real world (seed 7), terrain base
  + per-species scatter. 2952 plants/140^2 cells: grass/shrub/conifer on the misty-
  mountain ground, clustered, NONE on water or beach. Proves the decision is wired.
- REMAINING (instantiation): turn each FloraInstance into actual in-world geometry —
  grow a stem/CaveFlora-style plant or place plant blocks during chunk generation.
  Heavier render integration; the decision seam is ready for it.

## 21. r111 — flora INSTANTIATED as world blocks (blocks/entities)
- world::flora_blocks::flora_to_blocks(species, base) -> Vec<([i32;3],BlockId)>: ground
  cover = single block (TallGrass/Flower/Fern/Mushroom), Shrub = leaf clump, Cactus =
  3-stack, Conifer/Broadleaf/Palm = OakLog/BirchLog trunk + OakLeaves/BirchLeaves canopy
  (cone/round/fronds). 3 tests.
- region_flora_blocks(gen,x0,z0,w,d): the chunk-populate seam — flora_map decides,
  flora_to_blocks instantiates. Returns all (pos,block) for a region.
- examples/flora_patch_scene.rs: a 44x44 land patch of seed 7 rendered as real voxel
  cubes (terrain + flora), 3/4 top-down z-buffer raster — a populated forest with tree
  trunks/canopies + ground cover. 3618 flora blocks. Proves flora is real geometry.
  (Note: patch must be on land — origin at seed 7 is the water/beach corner.)
- BIN WIRED (compile-on-end): Launch handler populates a 40x40 flora patch around spawn
  via region_flora_blocks + set_block (streaming mesher picks up the edits). Modest
  radius for launch snappiness. gen via chunk_manager.generator().cpu() (&Arc deref).
- Long-term: move into per-chunk generate_chunk (deterministic, no set_block storm) with
  cross-chunk canopy handling.

## 22. r112 — HARDWARE-RUN FIXES (lock corruption, audio path, visible wiring)
- Operator ran on M2 Max / Honeykrisp Mesa. Three issues fixed:
- (1) Cargo.lock was TRUNCATED mid-checksum (line ~5258) — my cp/rm lock-dance captured a
  half-written file. Regenerated clean (5336 lines, parses, includes audiopus/ogg/rodio).
  STOP doing the lock-dance; run cargo directly (1.96 handles v4 lock fine).
- (2) No audio: the music default path was assets/music/koYopper.opus but the shipped
  asset is assets/music/title_koyopper.opus. Fixed fallback list (UNENDLICH_MUSIC ->
  title_koyopper -> koYopper -> assets/title_koyopper) + logs which track loaded or WHY
  it didn't ("no device / decode failed" / "no music started"). Verified the shipped
  title_koyopper.opus decodes through the engine (14,167,680 samples).
- (3) No visible changes: monument + flora were only in the menu→Launch arm, but the
  operator spawns straight into the world at startup. Added a ONE-SHOT in the main loop
  (frame_count>90, after spawn chunks load) that places the UNENDLICH monument + a 40x40
  flora patch at the player's position via set_block, LOGGING the success counts
  ("landmarks: placed monument N blocks + flora N/M blocks..."). Re-arms on Launch
  (landmarks_placed=false) so new worlds get them too. Removed the Launch-arm duplicates.
- All compile-on-end (bin needs glfw); used only verified APIs (set_block, region_flora_blocks,
  title_structure, SpatialMusic). Lib gates green; lock verified intact post-build.
