# WORKER PIPELINE — refilling the threads

How the parallel threads stay fed without me guessing. Pairs with
`THREADED_CHAT_SETUP.md` (the playbook) and `BACKLOG_INBOX.md` (the raw stream).

## 2026-06-04 — PIVOT: engine-task rounds (the pure-core queue is closed)
The base now has **285 sim cores**; the READY QUEUE below is exhausted and the
remaining work is integration-tier (real engine code), not pure cores. Per the
"≥6 genuinely pure cores or say so" rule, we **stop cutting SIM-core rounds**.
T01 correctly refused an empty R26 round and flagged this — that was the right call.

From here the threads run **engine-task rounds (E-rounds)**: one real, doc-grounded
task per thread, validated with the tools the threads now have. The threads CAN
compile and run; they have **llvm-pipe / lavapipe** (so `vk-harness` does real
shader↔CPU parity without hardware), the **harnesses**, and the **docs**. Briefs
are named **`WORKER_BRIEF.md`** at the package root and stamped `WORKER_ID`/`ROUND`.
Deliverable is an **overlay** of changed files + a short report — *not* the
multi-core RETURN_KIT flow.

### Current assignments — ROUND R26 (resume; base = current coordinator tree: single-thread R26–R44 + froxel + biome-IP cleanup)
Threads keep their stream identity; each gets one engine task that fits its stream.
- **T01 (sim) → heat-source spatial index** — `crates/unendlich-engine/src/heat_source.rs`,
  O(n)→O(nearby) (the heat field feeds convection/plant/ice sims); pure-test harness.
  (`docs/FOLLOWUPS.md` "Fix B")
- **T02 (atmo) → froxel-fog lavapipe parity test** — new
  `crates/vk-harness/tests/froxel_parity.rs`, GPU `cell_density` vs CPU oracle on
  llvm-pipe. (`docs/RENDER-PARITY.md`, `handoff/GPU_HARNESS.md`)
- **T03 (world) → wire the §5c debug toggles** — `bin/unendlich_world.rs` +
  `render/meshing_pool.rs` + post-process/shadow setup. (`SESSION_HANDOFF.md §5c`)
- **T04 (temporal) → temporal-accumulation CPU model** — `handoff/froxel_temporal_model.c`
  (reproject + EMA; bound ghosting/warmup before any shader change).
  (`SHADER_ROADMAP.md` "Next-increment analysis")
- **T05 (mesh) → §5b light-leak seam blend** — `render/mesher.rs` (edge skirts /
  shared-edge vertex snap). (`SESSION_HANDOFF.md §5b`)
- **MODE: LOW EFFORT** for all threads — tasks are fully scoped; execute, validate, return.
- Off-limits to all five (coordinator-owned): `froxel_fill.comp`, `chunk.frag`.
- **Return:** workers hand back the **changed file(s)** (repo paths) — a `RETURN_KIT`
  patch (`CHANGES-T0X-R26.patch`) is optional, not required, and workers do **not** block on
  HARNESS GREEN (coordinator validates on merge). Merge order: **T02 → T04 → T01 → T05 → T03**
  → roll `unendlich-r26-merged.zip`. Loadable coordinator: `COORDINATOR_BOOTSTRAP.md`.

### 2026-06-04 — base hygiene: copyrighted biome names removed (flagged by T01)
`assets/biomes/destiny2/` → `relics/`, `assets/biomes/smite/` → `arenas/`. All file
names, `_id`s, nested namespace refs, and the `biome_loader.rs` / `blocks.rs`
references were renamed to original names (0 IP refs remain; `cargo build` green).

## The cycle (per round)
1. **Patches return** → provenance-check, apply onto the current base, re-verify every
   module (purity + `pure_test` + full `run.sh` GREEN), merge `DECISIONS-T0?.md` fragments,
   roll the next base zip.
2. **Read before refilling** (the discipline the user asked for): scan the in-tree docs +
   existing modules before choosing tasks, so the queue draws on real gaps and never
   duplicates what's already there. The high-yield docs:
   - `docs/PERF-LEVERS.md` — what to dial, in cost order (memory→shadow→main).
   - `docs/FOLLOWUPS.md` — diagnosed bugs with file:line hypotheses.
   - `docs/TODO-BACKLOG.md` — the 10 in-tree TODOs, triaged by risk.
   - `docs/SHADER_ROADMAP.md` / `DESIGN_NOTES.md` — the renderer/atmosphere plan.
   - `docs/MOBS.md`, `MOVEMENT.md`, `WATER.md`, `ELEMENTS.md`, `perception_model.md`,
     `sound_genome.md`, `foraging_and_ecology.md` — system designs to pull cores from.
   - `ls crates/unendlich-engine/src/sim,world` + top-level — what exists (e.g. `eyes.rs`,
     `awareness.rs`, `perception_fog.rs`, `navfield_gpu.rs`, `world/pathing.rs`,
     `water_field.rs`, `weathering.rs` already exist — feed/extend, don't re-add).
3. **Pull the next batch** from the READY QUEUE below (3 cores/thread, by module-file so
   patches stay disjoint), optionally + 1 **bonus micro-opt** each.
4. **Cut + ship** the round zips. Stamp `WORKER_ID`/`ROUND` so patch names can't be mislabeled.

A round is only worth cutting when the queue has ≥ ~6 genuinely pure, self-contained,
property-testable cores. If it doesn't, the remaining work is integration-tier (below) and
belongs on the user's machine, not a worker — say so instead of inventing busywork.

## READY QUEUE (pure cores, prioritized — draws on backlog + the docs)
Round 6 is in flight (commands, inventory, crafting, settings, world_params, whitelist,
voxel_raycast, giant_cave). Next up:

- **`block_predicates`** (WORLD) — the *pure* half of TODO #4, centered on **`is_opaque`**
  (the user's point: `is_opaque`/`blocks_light` — not `is_solid` — is the right call for light-
  shaft culling and leaf transparency). Full set: `is_opaque`, `blocks_collision`,
  `produces_face_against`, `is_meshable_volume`, as standalone predicates over `BlockId` with
  no call-site changes. The clean testable core behind both the leaf-boxing fix and light-
  shaft optimization; wiring the mesher/light call sites onto them is integration (build-loop,
  not blind).
- **`emissive`** (SIM/RENDER-adjacent) — pure `genetics/element → emissive {color, intensity,
  region-mask}` for glowing eyes (horror mobs) + glowing necks (dragon elementals). Feeds an
  emissive channel on the entity mesh; the existing bloom turns it into glow. Pure + testable
  (intensity bounded, deterministic, element→hue mapping total). The shader emissive sample +
  bloom tap is integration (entity pass, kit §E).
- **`lod_skirt`** (RENDER/world) — pure skirt-vertex generation for a chunk edge given the
  neighbour's floor level (closes the LOD-transition crack). Pairs with `mesh_smooth` (r5).
- **`weathering`** (WORLD) — TODO #3: pure block→equilibrium step (env-driven), to drive the
  in-tree `weathering.rs` scaffold + footprint weathering.
- **`survival`** (SIM) — the gentle food/water/temp model from stream 4 (slow decay, PvP-safe
  floor) — proposed but never built; pure + deterministic.
- **`sound_genome`** (SIM/audio) — pure genome→voice params per `docs/sound_genome.md`
  (ties to the voice-chat/audio stream).
- **`dragon_language` / `landmark_memory`** (SIM) — design-stage; needs a spec pass first.

### Shader-reasoning lane (sandbox-doable shader work — pure analysis + test patterns)
Shader *code* needs Vulkan (integration), but the reasoning around it is testable here:
- **`spirv_reflect_check`** — a test pattern that reflects a `.spv`'s descriptor layout and
  asserts it matches the Rust-side binding table (the R16 idea, extended to full descriptor
  sets). Catches "changed a GLSL binding, forgot the Rust side."
- **emissive/bloom math** — a pure core for the emissive→tonemap→bloom interaction (so glow
  intensity is predictable, not blown out). Pairs with `pbr_terms` (r5) + `tonemap_lut` (r2).
- **mob→shader cost note** — document the entity-instancing path's per-frame cost model
  (instances × parts via `mob_lod`) so "is it efficient" has an answer before wiring.

## Bonus micro-opt lane (small, batch/dedup — only viable *because* there are many threads)
The user's point: with one thread these never ship; with several, append one to a round.
Each is a pure functional batch transform over a slice (SoA), property-tested against the
per-item version it replaces.
- **`batch_eyes`** — batch `eyes::sees` over a slice of (eye, target) pairs in one pass
  (shared trig, early-out by distance) instead of per-mob. Asserts parity with `sees`.
- **`batch_perception`** — batch `awareness::best_facing` / the being-looked-at check across
  mobs (one frustum build, many tests).
- **`batch_settle`** — batch `posture`/grounding updates over a mob slice.
- **dedup**: hoist repeated noise/`temp_for`/climate samples shared across nearby mobs.
Rule for these: must prove **bit-parity** with the scalar version (a `prop_` test feeding
random inputs to both), so the optimization can't change behaviour.

## Integration-tier (NOT worker cores — user's machine)
Tracked in `handoff/`: the LOD/shadow/entity wiring (`LOD_SHADOW_ENTITY_KIT.md`), SurfaceRun
upload branching (`SURFACERUN_KIT.md`), fog port, macOS/MoltenVK, and the bin edits (remove
sentinels; the is_solid → predicates *call-site* migration). These need Vulkan / the full
build / the bin, which the sandbox can't run.

---

## ROUND STATUS — R7 cut 2026-06-04 (9 cores, in flight)
- **T01 sim:** `emissive` (element→glowing eyes/neck), `survival` (gentle drives, no damage),
  `fall_impact` (landing consequence).
- **T02 render helpers:** `lod_skirt` (seal the LOD crack), `bloom_math` (predictable glow),
  `spirv_reflect_check` ★stretch (descriptor↔binding diff).
- **T03 world:** `block_predicates` ★ (`is_opaque`-centered split), `octree_remerge` (collapse
  uniform sibling leaves), `rebuild_coalescer` (one rebuild per cadence).
Zips: `worker-T01-r7.zip`, `worker-T02-r7.zip`, `worker-T03-r7.zip` (base = partial-R6).
Still queued after R7: `weathering`(if not already present), `sound_genome`(needs spec),
`survival`-expansions, the micro-opt lane (`batch_eyes`/`batch_perception` — must prove bit-parity).
**R6 close-out:** patch T01 R6 (command_parse/inventory_grid/crafting) into the base when it lands.

## ROUND STATUS — R7 MERGED, R8 cut 2026-06-04
- **R6 complete:** T01 (command_parse, crafting, inventory_grid, recipe_book) merged → full R6 in base.
- **R7 merged:** all 9 cores (emissive/survival/fall_impact · lod_skirt/bloom_math/spirv_reflect_check ·
  block_predicates/octree_remerge/rebuild_coalescer) — 97 tests across the 13, HARNESS GREEN.
- **R8 in flight:** predator_prey/egg_genome/feeding_select · lod_band/entity_cost_model/descriptor_registry
  · biome_blend/flow_accumulation/structure_grammar. Zips: worker-T0{1,2,3}-r8.zip (base = merged7).

## ROUND STATUS — R9 prepped 2026-06-04 (staged ahead of R8 return)
- **R8 still in flight** (no patches back yet): predator_prey/egg_genome/feeding_select ·
  lod_band/entity_cost_model/descriptor_registry · biome_blend/flow_accumulation/structure_grammar.
- **R9 staged** (zips on merged7, ready to hand out the moment R8 returns):
  - T01 sim: `nav3d` (3D swim/fly path), `hybrid_rules` (element crossings), `bond_tame` (first-tame FSM).
  - T02 render: `silhouette_lod` (far-entity legibility), `ribbon_mesh` (breath-ribbon geo), `froxel_index`.
  - T03 world: `river_carve` (channel from flow_accum), `cave_network` (tunnel tree), `ore_distribution`.
- Pipeline is now two rounds deep so the threads never wait on the coordinator.

## ROUND STATUS — R8 MERGED 2026-06-04
- **R8 merged** (9 cores, 59 tests, HARNESS GREEN): predator_prey/egg_genome/feeding_select ·
  lod_band/entity_cost_model/descriptor_registry · biome_blend/flow_accumulation/structure_grammar.
  (T02 lib.rs + T03 world/mod.rs hunks rejected on apply — base had diverged past R6 — and were
  hand-reconciled; T01 sim/mod.rs applied clean.)
- **R9 staged on merged7** (ready to hand out now). **R10 being cut on merged8** to stay two-deep.
- Lesson logged: workers built R8 on the R6-partial base, so mod-file hunks need hand-merge at
  integration — expected and cheap given pure additivity. Future round zips ship on the latest merge.

## ROUND STATUS — R9 + R10 MERGED 2026-06-04
- **R9 merged**: nav3d/hybrid_rules/bond_tame · silhouette_lod/ribbon_mesh/froxel_index · river_carve/cave_network/ore_distribution.
- **R10 merged**: sound_genome/landmark_memory/grove_decay · impostor_card/sky_gradient/godray_math · hydraulic_erosion/dungeon_layout/mineral_hardness.
- 18 modules, 118 tests, HARNESS GREEN. R9 applied clean; R10 T02/T03 mod hunks hand-reconciled.
- **R11 + R12 cut on merged10** (two-deep again). Total cores in base now ≈ 48 across 10 rounds + reconcile.

## ROUND STATUS — R13 cut 2026-06-04 (block-edit + microopt + abstraction, per user direction)
- **T03 world (block-editing):** `block_edit` (place/break apply → changed + dirty AABB — the
  long-flagged block place/break, logic half), `edit_coalesce` (N edits → minimal remesh bounds),
  `octree_invariants` (test-buildout: pure well-formed/minimal checker).
- **T02 render (abstraction + microopt):** `pipeline_key` (LOD×pass×material→checked u64 key + decode
  — the flagship typed-pipeline-builder), `batch_emissive` (microopt, BIT-PARITY vs r7 emissive),
  `draw_key` (front-to-back sort key).
- **T01 sim (microopt):** `batch_perception` / `batch_settle` / `batch_boids` — all batched, each
  proving BIT-PARITY against a scalar reference.
- **READY QUEUE (R14+):** `body_arc`, `segment_lerp` (the dragon body-segmentation cores above),
  then more microopts + the audit/cleanup coordinator lane.
- **Coordinator lane:** audits / cleanups / shared-file abstractions run on the merged base between
  rounds (can't be worker patches without collisions).

## ROUND STATUS — R11 MERGED 2026-06-04
- R11 merged (boids/dragon_language/input_prediction · shadow_cascade/cloud_noise/ssr_params ·
  cave_carve/chunk_priority/mesh_decimate); 58 tests; GREEN. Applied 100% CLEAN — cut on merged10,
  so no mod-hunk rejects. (Lesson confirmed: cutting on the latest merge removes reconcile friction.)
- R12 + R13 staged (two-deep). **READY QUEUE R14:** `mesh_tier` (the missing GPU near tier — top),
  then `body_arc`, `segment_lerp`, more microopts + coordinator audit lane.

## ROUND STATUS — R12 MERGED, R14 cut 2026-06-04
- R12 merged (mimic_learn/pollination_route/combat_resolve · atmosphere_scatter/vignette_grain/water_caustics
  · vegetation_scatter/biome_transition/mineral_vein_trace); 58 tests; GREEN.
- **R13 staged**; **R14 cut** (dragon animation + genome + effects):
  - T01 sim: `breathe`, `vocal_pose`, `batch_gene` (BIT-PARITY microopt — fast spawning).
  - T02 render: `armor_reflect`, `wing_opacity`, `anim_cadence` (lerp downsampling — GPU-load lever).
  - T03 world/fx: `smoke_field`, `ember_field`, `dust_motes`.
- READY QUEUE R15: `claw_curl`, `appendage_ripple`, `blink`, then `mesh_tier`, `body_arc`, `segment_lerp`.

## ROUND STATUS — R13 MERGED, R15 cut 2026-06-04
- R13 merged (batch_perception/batch_settle/batch_boids · pipeline_key/batch_emissive/draw_key ·
  block_edit/edit_coalesce/octree_invariants); 56 tests; GREEN. ~75 cores in base over 13 rounds.
- ✓ Design-doc audit: all design docs (handoff/ DRAGON_GENOME, ENTITY_ANIMATION, ENTITY_BODY_SEGMENTATION,
  MESH_TIER_PLAN, LOD_SHADOW_ENTITY_KIT, SURFACERUN_KIT; docs/*; root) confirmed present in the base.
- R14 staged. **R15 cut** — each thread = a STRUCTURE design doc (docs/design/STRUCTURES_*.md) + a
  categorical perf microopt (bit-parity bonus) + one mined core:
  - T01: STRUCTURES_creature.md + batch_breathe + claw_curl.
  - T02: STRUCTURES_render.md + batch_draw_key + impostor_uv.
  - T03: STRUCTURES_worldgen.md + batch_chunk_priority + **mesh_tier** (the missing GPU near tier).
- Standing thread template now: mine vision/TODOs for a core + a categorical perf bonus + (rotating) a design doc.
- READY QUEUE R16: body_arc, segment_lerp, blink, appendage_ripple, more batch_* perf bonuses.

## ROUND STATUS — R14 MERGED, R15 re-cut on merged14 — entering single-threaded mode 2026-06-04
- R14 merged (breathe/vocal_pose/batch_gene · anim_cadence/armor_reflect/wing_opacity ·
  smoke_field/ember_field/dust_motes); 54 tests; GREEN; applied 100% clean. ~84 cores over 14 rounds.
- R15 re-cut on merged14 (supersedes merged13-based R15 zips): structure design docs + bit-parity
  perf bonuses + mined cores (T03 carries **mesh_tier**). Parked & ready for whenever threads resume.
- PIPELINE PARKED: user working single-threaded for a stretch; no thread returns expected. R16 ready
  queue stands (body_arc, segment_lerp, blink, appendage_ripple, more batch_* bonuses) for resume.

## ROUND STATUS — R15 MERGED, R16 cut (big round) 2026-06-04
- R15 merged (batch_breathe/claw_curl + STRUCTURES_creature · batch_draw_key/impostor_uv + STRUCTURES_render
  · mesh_tier/batch_chunk_priority + STRUCTURES_worldgen); 36 tests; GREEN; 100% clean (cut on merged14).
  ~90 cores over 15 rounds. mesh_tier now in base — ready to wire (handoff/MESH_TIER_PLAN.md).
- **R16 cut (big, ~17 cores)** — mob/item/terrain + perf bonuses:
  - T01 sim: blink, appendage_ripple, mob_spawn_table, loot_roll, item_durability, batch_blink.
  - T02 render: icon_atlas, held_item_pose, mob_tint, damage_flash, batch_pipeline_key.
  - T03 world: terrain_warp, ridge_noise, thermal_erosion, strata, terrace, batch_mesh_tier.
- New: BASE_REV stamped into each kit (MERGE_PLAYBOOK improvement #2). See handoff/MERGE_PLAYBOOK.md
  for the full "why merges are easy + how to make them frictionless" writeup.
- READY QUEUE R17: body_arc, segment_lerp, more mob (threat_assess, flee, herd), item (item_modifier,
  stack_merge), terrain (cave_room, river_mouth), + batch_* bonuses.

## ROUND STATUS — R16 MERGED, R17 + R18 cut (two big rounds) 2026-06-04
- R16 merged (blink/appendage_ripple/mob_spawn_table/loot_roll/item_durability/batch_blink ·
  icon_atlas/held_item_pose/mob_tint/damage_flash/batch_pipeline_key ·
  terrain_warp/ridge_noise/thermal_erosion/strata/terrace/batch_mesh_tier); 17/17; GREEN; 100% clean.
  ~107 cores over 16 rounds.
- **R17 cut** — entity-body motion + mob AI + terrain/cave:
  T01 body_arc, segment_lerp, threat_assess, flee_decision, herd_cohesion, batch_appendage_ripple.
  T02 outline_width, health_bar_uv, nameplate_fade, spec_highlight, batch_icon_atlas.
  T03 cave_room, river_mouth, moisture_spread, scatter_cluster, ore_vein_richness, batch_ridge_noise.
- **R18 cut** — items + progression + biome/terrain detail:
  T01 item_modifier, stack_merge, xp_curve, status_effect, aggro_decay, batch_loot_roll.
  T02 lod_crossfade, billboard_face, ao_corner, fog_blend, batch_held_item_pose.
  T03 biome_temp_map, cliff_detect, beach_band, cave_decoration, batch_strata.
- Queue now TWO-deep (R17, R18), ~33 cores staged. Each kit carries BASE_REV=R16. body_arc/segment_lerp
  (R17) complete the ENTITY_BODY_SEGMENTATION cores. READY QUEUE R19: held-item/inventory polish,
  more mob (pack_hunt, territory), water (flow_route), + batch_* bonuses.

## ROUND STATUS — R17 MERGED, R19 cut 2026-06-04
- R17 merged (body_arc/segment_lerp/threat_assess/flee_decision/herd_cohesion/batch_appendage_ripple ·
  outline_width/health_bar_uv/nameplate_fade/spec_highlight/batch_icon_atlas ·
  cave_room/river_mouth/moisture_spread/scatter_cluster/ore_vein_richness/batch_ridge_noise);
  17/17; GREEN; 100% clean. ~124 cores over 17 rounds. body_arc+segment_lerp finish ENTITY_BODY_SEGMENTATION.
- R18 still staged (in flight). **R19 cut** to hold two-deep:
  T01 pack_hunt, territory, sleep_cycle, hunger_drive, mate_select, batch_threat_assess.
  T02 lod_select, contact_shadow, rim_light, uv_scroll, batch_spec_highlight.
  T03 flow_route, lake_fill, snow_line, sediment_deposit, batch_terrain_warp.
- Sandbox perf work shipped while threads run: heat Fix A patch + Fix B (tested drop-in heat_source.rs),
  remesh_gate correction, cull workgroup-aggregation (SPIR-V-validated chunk_cull.comp), dragon-viewer
  genome export. Big remaining: main/shadow raster wall → SurfaceRun far-LOD (M2 architecture work).
- READY QUEUE R20: more items (socket/gem, set_bonus), mob (migrate, scent_track), water (foam, ripple_ring), batch_* bonuses.

## ROUND STATUS — R18 MERGED, R20 cut 2026-06-04
- R18 merged (item_modifier/stack_merge/xp_curve/status_effect/aggro_decay/batch_loot_roll ·
  lod_crossfade/billboard_face/ao_corner/fog_blend/batch_held_item_pose ·
  biome_temp_map/cliff_detect/beach_band/cave_decoration/batch_strata); 16/16; GREEN. Mod hunks
  reconciled (R18 cut on merged16, pre-R17). ~140 cores over 18 rounds.
- R19 staged (in flight). **R20 cut** — HORN SYSTEM (handoff/HORN_SYSTEM.md) + obsession + fx:
  T01 horn_type, horn_charge, obsession, scent_track, batch_status_effect.
  T02 horn_fractal, ribbon_trail, particle_fade, decal_uv, batch_fog_blend.
  T03 foam_edge, ripple_ring, cave_moss, erosion_settle, batch_cliff_detect.
- New design: HORN_SYSTEM.md (exotic 8-24 fractal horns that charge abilities + encode the obsession
  genome) from the user's idea-stream. Viewer follow-up offered (extend horns dial to type+count+fractal).
- READY QUEUE R21: ability_cast (discharge→effect), gem_socket/set_bonus (items), migrate (mob), batch_* bonuses.

## ROUND STATUS — R19 MERGED, R21 cut 2026-06-04
- R19 merged (pack_hunt/territory/sleep_cycle/hunger_drive/mate_select/batch_threat_assess ·
  lod_select/contact_shadow/rim_light/uv_scroll/batch_spec_highlight ·
  flow_route/lake_fill/snow_line/sediment_deposit/batch_terrain_warp); 16/16; GREEN; 100% clean.
  ~156 cores over 19 rounds.
- R20 staged (in flight). **R21 cut** to hold two-deep:
  T01 ability_cast (discharge→effect, pairs horn_charge), gem_socket, set_bonus, migrate, batch_aggro_decay.
  T02 heat_haze, sss_approx, spark_burst, edge_highlight, batch_lod_crossfade.
  T03 waterfall, cave_pool, frost_spread, gravel_slide, batch_biome_temp_map.
- PERF NOTE (R17 log): SurfaceRun is 33× more compact per chunk (srun 5KB vs octree 173KB),
  storage ratio 6.15 → validates routing far/mid chunks to SurfaceRun as the main/shadow-wall fix
  (docs/FOLLOWUPS 2026-06-04). cull p50 ~0 at low visible count (confirms the cull diagnosis).
- READY QUEUE R22: aura_field (ability AoE), enchant_table (items), school_fish (mob), batch_* bonuses.

## ROUND STATUS — R20 MERGED, R22 cut 2026-06-04
- R20 merged (horn_type/horn_charge/obsession/scent_track/batch_status_effect ·
  horn_fractal/ribbon_trail/particle_fade/decal_uv/batch_fog_blend ·
  foam_edge/ripple_ring/cave_moss/erosion_settle/batch_cliff_detect); 15/15; GREEN. ~171 cores/20 rounds.
  Horn system fully in base.
- R21 staged (in flight). **R22 cut** (GPU_COMPUTE_PLAN.md cores):
  T01 popdyn_step, pollen_spread, plant_gene_express, ability_aura, batch_horn_charge.
  T02 region_cull, lod_morph, light_falloff, batch_rim_light.
  T03 prefetch_predict, chunk_schedule, field_step, gen_estimate, batch_flow_route.
- 901s LOG SIGNALS: octree pool 96% full (→ SurfaceRun far-LOD MEMORY-urgent), CPU-bound at extent,
  fires=1M (apply heat Fix A/B). Plan: handoff/GPU_COMPUTE_PLAN.md (GPU field-kernel abstraction +
  multi-level cull + scheduling/prediction). Integration is M2; pure references are R22.
- READY QUEUE R23: ability follow-ons, more field/popdyn refs, scheduler refinements, batch_* bonuses.

## ROUND STATUS — R21 MERGED, GPU HARNESS BUILT, R23 cut 2026-06-04
- R21 merged (ability_cast/gem_socket/set_bonus/migrate/batch_aggro_decay · heat_haze/sss_approx/
  spark_burst/edge_highlight/batch_lod_crossfade · waterfall/cave_pool/frost_spread/gravel_slide/
  batch_biome_temp_map); 15/15; GREEN. ~186 cores over 21 rounds.
- **GPU SANDBOX HARNESS built** (user request). Found existing infra: vk-harness runs real shaders on
  lavapipe vs CPU references (integration-tier). Added the sandbox-tier gate `harness/gpu_check.sh`
  (glslangValidator compile + spirv-val, no device — 25/25 shaders pass), wired into run.sh, documented
  in handoff/GPU_HARNESS.md, ruleset in RULESET.md (R22 CPU-ref · R23 shader gate · R24 lavapipe parity ·
  R25 layout parity · R26 .spv). This lets shader/kernel work be iterated in the sandbox.
- R22 staged (in flight). **R23 cut** to hold two-deep:
  T01 crit_roll, status_stack, threat_share, spawn_budget, batch_ability_cast.
  T02 depth_fade, motion_blur_vec, tonemap_curve, batch_light_falloff.
  T03 erosion_advect, cave_breeze, aquifer, batch_gen_estimate.
- NEXT (sandbox, high-value): pilot a GPU kernel end-to-end — CPU ref (e.g. field_step r22) + a .comp +
  gpu_check + a vk-harness parity-test scaffold — to validate the GPU workflow. SurfaceRun far-LOD +
  heat Fix A/B remain the M2 integration priorities.

## COORDINATOR HABIT (standing) — relevancy lookup before codebase work
Before any sandbox codebase work (diagnosing a perf log, building a tool, answering "does X exist"),
run `bash harness/relevant.sh <topic keywords>` first (RULESET R0). It returns curated glossary hits +
live file/doc matches so existing infra is found, not reinvented. Add a glossary.tsv row whenever a new
subsystem lands. The relevancy tool (harness/relevant.sh + harness/glossary.tsv) ships in the base zip.

## ROUND STATUS — R22 MERGED, RELEVANCY TOOL ADOPTED, R24 cut 2026-06-04
- R22 merged (popdyn_step/pollen_spread/plant_gene_express/ability_aura/batch_horn_charge ·
  region_cull/lod_morph/light_falloff/batch_rim_light · prefetch_predict/chunk_schedule/field_step/
  gen_estimate/batch_flow_route); 14/14; GREEN (incl GPU gate 25/25). ~200 cores over 22 rounds.
- **Relevancy tool adopted into base** (harness/relevant.sh + harness/glossary.tsv, salvaged + verified).
  RULESET R0 + coordinator habit: run `relevant.sh <topic>` before any codebase work. Demonstrated:
  "gpu shader test"→vk-harness/gpu_check.sh/GPU_HARNESS.md first; "surfacerun memory oom"→SURFACERUN_KIT
  + upload_repr. Stale off-lineage dirs (merged23/24) moved aside; base lineage = merged20/21/22.
- R23 staged (in flight). **R24 cut** to hold two-deep:
  T01 enchant_roll, buff_stack, pack_role, nav_avoid, batch_crit_roll.
  T02 chromatic_aberration, exposure_adapt, dither_pattern, batch_lod_morph.
  T03 humidity_advect, soil_moisture, spring_flow, batch_aquifer + docs/design/FIELD_KERNELS.md.
- NEXT (sandbox, high-value): pilot a GPU kernel end-to-end (field_step CPU ref + .comp + gpu_check +
  vk-harness parity scaffold). M2 integration priorities still: SurfaceRun far-LOD + heat Fix A/B.

## ROUND STATUS — R23+R24+R25 MERGED (one batch), R26+R27 cut 2026-06-04
- Re-uploaded batch R23/R24/R25 (all cut on merged21) merged onto merged22 → merged25.
  39/39 new modules, 39/39 tests, HARNESS GREEN (GPU gate 25/25). Used T02-R24-r2 (superseded the
  original T02-R24). ~239 cores over 25 rounds.
    R23: crit_roll/status_stack/threat_share/spawn_budget/batch_ability_cast ·
         depth_fade/motion_blur_vec/tonemap_curve/batch_light_falloff ·
         erosion_advect/cave_breeze/aquifer/batch_gen_estimate.
    R24: enchant_apply/school_fish/scent_decay/ability_cooldown/batch_crit_roll ·
         dof_blur/chromatic_aberration/lens_dirt/batch_depth_fade ·
         lava_flow/snow_accumulate/sediment_settle/batch_erosion_advect.
    R25: taming/mount_stamina/ability_combo/scent_gradient/batch_status_stack ·
         exposure_adapt/fxaa_edge/refraction_offset/batch_motion_blur_vec ·
         river_meander/glacier_carve/dune_migrate/batch_frost_spread.
- TWO design ideas materialized (user) → docs in base + glossary rows:
    handoff/BODY_PART_MESHING.md — part-aware entity meshing: greedy-merge healthy parts, keep
      damaged parts voxel-fine, watertight seams (no gaps), destructible shards. Uses body-part system.
    handoff/COMBAT_STANCE.md — mobs guard the weak side / present healthy limbs, distance by vital
      exposure, turn rates; + my open extensions (pack flanking, terrain guard, attacker memory, wing tuck).
- Nothing was staged after the batch merge → cut TWO rounds to restore two-deep, folding in the new cores:
    R26: T01 stance (guard_side/vital_cover/engage_distance/flank_turn_rate/batch_spawn_budget) ·
         T02 part-mesh (part_mesh_lod/seam_skirt/voxel_greedy_run/batch_chromatic_aberration) ·
         T03 landforms (canyon_carve/delta_fan/karst_sink/batch_lava_flow).
    R27: T01 carnage+bias (limb_detail_flag/damage_shard/dismember_check/stance_bias/batch_scent_decay) ·
         T02 mesh-polish (normal_smooth/specular_aa/crack_fill/batch_dof_blur) ·
         T03 landforms (mesa_form/hoodoo_carve/fjord_cut/batch_river_meander).
- NEXT (sandbox): the GPU-kernel pilot (field_step ref + .comp + gpu_check + vk-harness parity scaffold);
  the dragon-viewer gap-avoidance + part-merge preview ties to BODY_PART_MESHING. M2: SurfaceRun far-LOD,
  heat Fix A/B.

## MODE CHANGE — SINGLE-THREAD (coordinator builds directly) 2026-06-04
User is on the phone app and can't ferry zips to worker chats right now ("single thread until I say
otherwise"). So the coordinator now AUTHORS cores directly in the sandbox (rustc + harness), merges, and
rolls bases — no worker zips needed. Worker zips for R26/R27 were cut earlier but are moot in this mode.
- **R26 built single-threaded → merged26.** 13 cores, 13/13 rustc tests, HARNESS GREEN (GPU gate 25/25):
  T01 guard_side/vital_cover/engage_distance/flank_turn_rate/batch_spawn_budget (COMBAT_STANCE) ·
  T02 part_mesh_lod/seam_skirt/voxel_greedy_run/batch_chromatic_aberration (BODY_PART_MESHING) ·
  T03 canyon_carve/delta_fan/karst_sink/batch_lava_flow.
- NEXT (single-thread): build R27 the same way (limb_detail_flag/damage_shard/dismember_check/stance_bias ·
  normal_smooth/specular_aa/crack_fill · mesa_form/hoodoo_carve/fjord_cut + batch bonuses), then keep going
  through the backlog until the user resumes threading. Also still open: the GPU-kernel pilot.

## SINGLE-THREAD — R27 built → merged27 (2026-06-04)
- **R27 built single-threaded → merged27.** 13 cores, 13/13 rustc tests, HARNESS GREEN (GPU gate 25/25):
  T01 limb_detail_flag/damage_shard/dismember_check/stance_bias/batch_scent_decay (CARNAGE + stance) ·
  T02 normal_smooth/specular_aa/crack_fill/batch_dof_blur (MESH POLISH) ·
  T03 mesa_form/hoodoo_carve/fjord_cut/batch_river_meander.
- Body-part system now complete end-to-end across R26+R27: meshing decisions (part_mesh_lod/seam_skirt/
  voxel_greedy_run) + carnage state (limb_detail_flag/damage_shard/dismember_check) + the polish
  (normal_smooth/specular_aa/crack_fill). Combat stance complete (guard_side/vital_cover/engage_distance/
  flank_turn_rate/stance_bias). ~265 cores over 27 rounds.
- NEXT (single-thread): keep mining backlog — more abilities/items, mob-AI (pack flanking from
  COMBAT_STANCE open list), field/popdyn cores, worldgen; plus the still-open GPU-kernel pilot.

## SINGLE-THREAD — R28 built → merged28 (2026-06-04)
- **R28 → merged28.** 13 cores, 13/13 rustc tests, HARNESS GREEN (GPU gate 25/25):
  T01 pack_flank/cover_seek/focus_fire/disengage/batch_guard_side (PACK COMBAT AI) ·
  T02 emissive_pulse/parallax_offset/contact_ao/batch_specular_aa ·
  T03 talus_slope/oxbow_lake/alluvial_fan/batch_canyon_carve.
- Combat AI now layered: per-mob stance (R26/27) + pack-level flanking/focus/cover/disengage (R28).
  ~278 cores over 28 rounds.
- NEXT (single-thread): more items/abilities + mob variety + worldgen + field cores; GPU-kernel pilot still open.

## SINGLE-THREAD — R29 built → merged29 (2026-06-04)
- **R29 → merged29.** 13 cores, 13/13 rustc tests, HARNESS GREEN (GPU gate 25/25):
  T01 lift_force/drag_force/glide_ratio/bank_radius/batch_flap_thrust (DRAGON FLIGHT) ·
  T02 rayleigh_phase/mie_phase/sun_disk/batch_mie_phase (SKY/ATMOSPHERE) ·
  T03 altitude_temp/rain_shadow/tree_line/batch_altitude_temp (CLIMATE).
- ~291 cores over 29 rounds. Dragon flight physics + atmospheric scattering + climate now in.
- NEXT (single-thread): items/crafting depth, dragon genome/breeding follow-ons, more ecology; GPU pilot open.

## SINGLE-THREAD — R30 built → merged30 (2026-06-04)
- **R30 → merged30.** 13 cores, 13/13 rustc tests, HARNESS GREEN (GPU gate 25/25):
  T01 gene_crossover/gene_mutate/trait_dominance/hatch_time/batch_gene_crossover (GENETICS/BREEDING) ·
  T02 fresnel_schlick/ggx_distribution/iridescence/batch_fresnel (PBR SCALES) ·
  T03 ore_grade/forest_density/spawn_density/batch_ore_grade (ECOLOGY).
- ~304 cores over 30 rounds. The viewer's genome export now has breeding math to feed; dragon scales have
  a PBR/iridescence basis; biomes have resource + ecology drivers.
- NEXT (single-thread): crafting/economy, status/elemental combat, water/ocean, more atmosphere; GPU pilot open.

## SINGLE-THREAD — R31 built → merged31 (2026-06-04)
- **R31 → merged31.** 13 cores, 13/13 rustc tests, HARNESS GREEN (GPU gate 25/25):
  T01 elemental_multiplier/burn_tick/freeze_slow/resist_apply/batch_burn_tick (ELEMENTAL/STATUS) ·
  T02 gerstner_wave/foam_threshold/water_depth_color/batch_gerstner (WATER SURFACE) ·
  T03 wave_height/tide_level/salinity/batch_tide_level (OCEAN/HYDROLOGY).
- ~317 cores over 31 rounds. Combat now has an elemental type system; oceans have wave/tide/salinity drivers.
- NEXT (single-thread): crafting/economy, UI/HUD math, particle systems, more dragon abilities; GPU pilot open.

## SINGLE-THREAD — R32 built → merged32 (2026-06-04)
- **R32 → merged32.** 13 cores, 13/13 rustc tests, HARNESS GREEN (GPU gate 25/25):
  T01 craft_quality/market_price/skill_gain/repair_cost/batch_market_price (CRAFTING/ECONOMY) ·
  T02 healthbar_fill/radial_sweep/minimap_project/batch_healthbar_fill (UI/HUD) ·
  T03 cloud_coverage/wind_shear/precip_rate/batch_cloud_coverage (ATMOSPHERE/WEATHER).
- ~330 cores over 32 rounds. Single-thread run R26→R32 covers: body-part meshing+carnage, combat stance,
  pack AI, flight, sky/atmosphere, climate, genetics, PBR scales, ecology, elemental combat, ocean, crafting,
  HUD, weather.
- NEXT (single-thread): particles/VFX params, sound/ambience math, pathfinding helpers, more dragon abilities;
  GPU-kernel pilot still open.

## SINGLE-THREAD — R33 built → merged33 (2026-06-04)
- **R33 → merged33.** 13 cores, 13/13 rustc tests, HARNESS GREEN (GPU gate 25/25):
  T01 heuristic_octile/path_cost/step_toward/turn_penalty/batch_path_cost (PATHFINDING) ·
  T02 particle_size_curve/particle_alpha_curve/particle_color_ramp/batch_particle_alpha (VFX) ·
  T03 sound_attenuation/doppler_shift/reverb_wetness/batch_sound_attenuation (SPATIAL AUDIO).
- ~343 cores over 33 rounds. NOTE: sandbox disk hit 100% mid-round (accumulated merged trees + /tmp);
  pruned to recent trees only (freed ~9GB). Going forward: clean /tmp test bins + prune old merged dirs each roll.
- NEXT (single-thread): more dragon abilities, inventory/equipment, day-night/seasons, AI senses; GPU pilot open.

## SINGLE-THREAD — R34 built → merged34 (2026-06-04)
- **R34 → merged34.** 13 cores, 13/13 rustc tests, HARNESS GREEN (GPU gate 25/25):
  T01 breath_cone/breath_damage/ability_charge_time/mana_cost/batch_breath_damage (DRAGON BREATH) ·
  T02 sun_elevation_color/ambient_intensity/shadow_length/batch_ambient_intensity (DAY/NIGHT) ·
  T03 season_phase/daylight_hours/seasonal_temp_offset/batch_seasonal_temp_offset (SEASONS).
- ~356 cores over 34 rounds. (Disk: pruning to recent merged trees + cleaning /tmp each roll — stable.)
- NEXT (single-thread): inventory/equipment, AI senses (sight/smell/hearing), territory/nesting, more abilities.

## SINGLE-THREAD — R35 built → merged35 (2026-06-04)
- **R35 → merged35.** 13 cores, 13/13 rustc tests, HARNESS GREEN (GPU gate 25/25):
  T01 sight_detect/hearing_radius/scent_strength/awareness_blend/batch_sight_detect (AI SENSES) ·
  T02 decal_fade/projector_uv/detail_normal_blend/batch_decal_fade (DECALS) ·
  T03 nest_suitability/territory_overlap/resource_regen/batch_nest_suitability (TERRITORY/NESTING).
- ~369 cores over 35 rounds. AI now has senses feeding awareness; world has nesting + territory + resource nodes.
- NEXT (single-thread): inventory/equipment stats, status combat (poison/stun), camera/controls math, more worldgen.

## SINGLE-THREAD — R36 built → merged36 (2026-06-04)
- **R36 → merged36.** 13 cores, 13/13 rustc tests, HARNESS GREEN (GPU gate 25/25):
  T01 stat_total/carry_weight/poison_tick/stun_duration/batch_stat_total (INVENTORY/STATUS) ·
  T02 camera_fov_zoom/camera_shake/depth_to_view_z/batch_depth_to_view_z (CAMERA) ·
  T03 grass_sway/vegetation_density/snow_cover/batch_vegetation_density (VEGETATION).
- ~382 cores over 36 rounds. (Caught a test-design bug in grass_sway — code was fine.)
- NEXT (single-thread): equipment affixes/sets, projectiles/physics, fluid sim, more dragon abilities.

## SINGLE-THREAD — R37 built → merged37 (2026-06-04)
- **R37 → merged37.** 13 cores, 13/13 rustc tests, HARNESS GREEN (GPU gate 25/25):
  T01 projectile_drop/ballistic_range/knockback_impulse/terminal_velocity/batch_knockback_impulse (PHYSICS) ·
  T02 beer_lambert/height_fog/scatter_inscatter/batch_beer_lambert (VOLUMETRIC FOG) ·
  T03 flow_velocity/erosion_capacity/deposition/batch_flow_velocity (HYDRAULIC EROSION).
- ~395 cores over 37 rounds. The hydraulic-erosion trio (flow→capacity→deposition) composes with the
  existing erosion/sediment cores into a full droplet-erosion model.
- NEXT (single-thread): equipment affixes/sets, AI memory/learning, cloth/soft-body, more dragon abilities.

## SINGLE-THREAD — R38 built → merged38 (2026-06-04)
- **R38 → merged38.** 13 cores, 13/13 rustc tests, HARNESS GREEN (GPU gate 25/25):
  T01 memory_decay/threat_memory/habituation/reward_estimate/batch_memory_decay (AI MEMORY/LEARNING) ·
  T02 verlet_step/constraint_correction/cloth_wind_force/batch_verlet_step (CLOTH/SOFT-BODY) ·
  T03 arch_height/tower_taper/wall_crenellation/batch_arch_height (STRUCTURES).
- ~408 cores over 38 rounds. AI-memory cores compose with R35 senses + R28 pack AI; cloth cores give a
  Verlet+constraint solver pair for capes/banners/wings.
- NEXT (single-thread): equipment affixes/sets, rope/spring follow-ons, dungeon/structure layout, more dragon abilities.

## SINGLE-THREAD — R39 built → merged39 (2026-06-04)
- **R39 → merged39.** 13 cores, 13/13 rustc tests, HARNESS GREEN (GPU gate 25/25):
  T01 affix_count/affix_magnitude/rarity_tier/set_tier/batch_affix_magnitude (EQUIPMENT AFFIXES/SETS) ·
  T02 spring_force/damping_force/catenary_sag/batch_spring_force (ROPE/SPRING) ·
  T03 room_fits/corridor_length/stair_steps/batch_corridor_length (DUNGEON LAYOUT).
- ~421 cores over 39 rounds. Affix cores pair with R21 gem_socket/set_bonus + R32 craft_quality into a loot
  pipeline; spring/damping extend R38's Verlet cloth into a full mass-spring solver.
- NEXT (single-thread): dragon abilities/breath follow-ons, loot tables/drop rates, more worldgen (caves/biomes).

## SINGLE-THREAD — R40 built → merged40 (2026-06-04) — 40-ROUND MILESTONE
- **R40 → merged40.** 13 cores, 13/13 rustc tests, HARNESS GREEN (GPU gate 25/25):
  T01 breath_falloff/breath_recharge/roar_fear/wing_buffet_force/batch_breath_falloff (DRAGON BREATH) ·
  T02 trail_alpha_fade/tracer_width/muzzle_flash_intensity/batch_trail_alpha_fade (PROJECTILE VFX) ·
  T03 cave_tunnel_radius/stalactite_length/biome_blend_weight/batch_cave_tunnel_radius (CAVES/BIOMES).
- **~434 cores over 40 rounds, every round HARNESS GREEN.** Breath cores complete the R34 breath kit;
  VFX cores pair with R37 projectiles; cave/biome cores extend the R22/R23 cave+aquifer set.
- INTEGRATION STILL PENDING: cores are a validated pure-logic library; no engine loop yet, and real GPU
  numeric parity (lavapipe/vk-harness) runs only on M2. Natural checkpoint to consider GPU pilot / viewer / threading.

## SINGLE-THREAD — R41 built → merged41 (2026-06-04)
- **R41 → merged41.** 13 cores, 13/13 rustc tests, HARNESS GREEN (GPU gate 25/25):
  T01 drop_chance/loot_quantity/magic_find_weight/status_combo_multiplier/batch_drop_chance (LOOT/STATUS) ·
  T02 burn_glow_intensity/freeze_tint/shock_arc_jitter/batch_burn_glow_intensity (ELEMENTAL VFX) ·
  T03 lake_fill_level/river_width/moisture_from_water/batch_river_width (RIVERS/LAKES).
- ~447 cores over 41 rounds. Loot cores complete the R39 affix pipeline; elemental VFX pair with R31
  elemental/burn/freeze; rivers/lakes extend R24/R25 river_meander + R37 hydraulic erosion.
- NEXT (single-thread): status-effect interactions, more dragon flight, settlement/structure layout — OR pivot to GPU pilot / viewer / threading.

## SINGLE-THREAD — R42 built → merged42 (2026-06-04)
- **R42 → merged42.** 13 cores, 13/13 rustc tests, HARNESS GREEN (GPU gate 25/25):
  T01 wet_lightning_bonus/freeze_shatter_bonus/diminishing_returns_resist/debuff_duration_scale/batch_wet_lightning_bonus (STATUS INTERACTIONS) ·
  T02 stall_factor/climb_rate/turn_g_force/batch_stall_factor (FLIGHT DYNAMICS) ·
  T03 building_spacing/plaza_radius/wall_perimeter/batch_plaza_radius (SETTLEMENT LAYOUT).
- ~460 cores over 42 rounds. Status-interaction cores build on R31 elemental + R41 status_combo; flight
  dynamics extend the R29 lift/drag/glide kit; settlement layout joins R38/R39 structures+dungeon cores.
- NEXT (single-thread): combat resolution, dragon AI behaviors, road/path networks — OR GPU pilot / viewer / threading.

## SINGLE-THREAD — R43 built → merged43 (2026-06-04)
- **R43 → merged43.** 13 cores, 13/13 rustc tests, HARNESS GREEN (GPU gate 25/25):
  T01 hit_chance/damage_after_armor/block_reduction/overkill_damage/batch_damage_after_armor (COMBAT RESOLUTION) ·
  T02 hit_flash_intensity/damage_number_scale/low_health_vignette/batch_damage_number_scale (COMBAT FEEDBACK VFX) ·
  T03 road_grade_penalty/switchback_count/bridge_needed/batch_road_grade_penalty (ROAD NETWORKS).
- ~473 cores over 43 rounds. Combat-resolution cores complete the R23 crit/threat damage chain; road cores
  extend the R33 A* pathfinding kit (heuristic_octile/path_cost/step_toward/turn_penalty).
- NEXT (single-thread): dragon AI behavior trees, economy/trade, weather fronts — OR GPU pilot / viewer / threading.

## SINGLE-THREAD — R44 built → merged44 (2026-06-04)
- **R44 → merged44.** 13 cores, 13/13 rustc tests, HARNESS GREEN (GPU gate 25/25):
  T01 aggression_level/flee_threshold/target_priority/patrol_wait_time/batch_target_priority (DRAGON AI) ·
  T02 cloud_shadow_factor/rain_streak_alpha/lightning_flash/batch_rain_streak_alpha (WEATHER RENDER) ·
  T03 front_temperature_gradient/pressure_gradient_wind/storm_intensity/batch_storm_intensity (WEATHER FRONTS).
- ~486 cores over 44 rounds. AI cores join R28 pack-flank/focus-fire + R35 senses + R38 threat-memory into a
  decision layer; weather cores (render + fronts) extend R32 cloud_coverage/wind_shear/precip_rate.
- NEXT (single-thread): economy/trade, dragon growth/aging, dungeon population — OR GPU pilot / viewer / threading.
