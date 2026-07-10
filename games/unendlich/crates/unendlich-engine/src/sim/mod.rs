//! Deterministic simulation layer — the foundation for rollback netcode.
//!
//! # The split that everything else hangs off
//!
//! The engine is divided into two worlds:
//!
//! - **Simulation** (this module): deterministic, fixed-tick (60 Hz),
//!   snapshottable, integer-only math ([`fixed::Fixed`]). It holds the
//!   entities that are networked and rolled back. A tick is a pure function
//!   [`state::step`]; the same state + inputs always yield the same next
//!   state, bit-for-bit, on every peer.
//! - **Presentation** (the renderer, mesher, camera, and the future
//!   animation system): runs at display rate, reads sim state, and
//!   interpolates between ticks for smooth motion. It **never** feeds back
//!   into the simulation. Animation is presentation: it is not rolled back,
//!   it simply re-reads whatever the corrected sim says.
//!
//! Get this split wrong and rollback can't be added later; get it right and
//! multiplayer, replays, and deterministic tests all fall out of the same
//! machinery.
//!
//! # Scope of the rolled-back state
//!
//! [`state::SimState`] holds **entities only** (characters today; mobs and
//! projectiles later) — never the voxel world. The terrain is far too large
//! to snapshot every tick; block edits will enter as deterministic events in
//! the input stream and the world is synced separately.
//!
//! # Build status
//!
//! - Brick 1 — [`fixed`] + [`state`]: deterministic types and the pure step. ✅
//! - Brick 2 — [`rollback`]: prediction + rollback engine, proven in
//!   single-player tests via injected input delay. ✅
//! - Brick 3 — [`physics`]: character collision against the voxel world
//!   through a read-only [`physics::WorldQuery`], with a graded material
//!   model ([`physics::BlockPhysics`]: collision class + friction) so
//!   foliage, fluid, ice, and stone differ without special cases. ✅
//! - Brick 4 — [`net`]: transport + session. UDP input exchange with
//!   redundant resend for loss tolerance, ack-based pruning, and
//!   confirmed-tick checksum exchange for desync detection. Protocol and
//!   session logic are pure and tested; the socket is a thin std-only shell. ✅
//! - Brick 5 — animation / wire-up: presentation-layer interpolation driven
//!   by the sim. [`driver::SimDriver`] is the first half — it steps the sim at
//!   a fixed 60 Hz from wall-clock time and exposes an interpolated
//!   [`driver::RenderPose`]. Wiring it into the bin's game loop (input → driver
//!   → camera) is the remaining integration step. 🚧

pub mod cognition;
pub mod driver;
pub mod fixed;
pub mod goals;
pub mod gpu_bench;
pub mod mob;
pub mod spawn;
pub mod salinity;
pub mod sound;
pub mod fluid;
pub mod medium;
pub mod navfield_gpu;
pub mod ao_gpu;
pub mod volumetric_gpu;
pub mod smoke_gpu;
pub mod heat_distort;
pub mod foveation;
pub mod perception_fog;
pub mod perception_field;
pub mod sync_check;
pub mod consensus;
pub mod desync_monitor;
pub mod world_authority;
pub mod net;
pub mod personality;
pub mod physics;
pub mod rollback;
pub mod netview; // webhost-kit snapshot (spectate the rollback sim in a browser)
/// EXPERIMENTAL (r238): batch-as-field grass LOD — front high-res per-blade,
/// back fast-forwarded as a density/growth/sway field. Pure model always
/// built + tested; live tick-path hookup is behind `grass-field-batch`.
pub mod grass_field;
/// EXPERIMENTAL (r239): batch-as-field fire LOD — front high-res per-cell burn,
/// back fast-forwarded as a fuel/intensity/heat field (reaction-diffusion
/// front). Pure model always built + tested; hookup behind `fire-field-batch`.
pub mod fire_field;
/// EXPERIMENTAL (r239): vision-driven mesh scheduling — mesh what player/mob
/// view cones need now/soon, defer obscured/behind chunks (the dense-cave
/// win). Pure decision model always built + tested; hookup behind
/// `mesh-vision-sched`. Cone math mirrors sim::perception.
pub mod mesh_vision;
/// EXPERIMENTAL (r244): per-chunk shader variant selection — pick the cheapest
/// chunk.frag variant (fog steps / water / dynamic lights / shadows compiled
/// out) a chunk actually needs. Pure decision model always built + tested;
/// pipeline hookup behind `shader-variants`.
pub mod shader_variant;
/// Authoritative game-speed (time-dilation) model: log slider + closest-to-1x
/// reconciliation across peers. Server-side only.
pub mod game_speed;
pub mod voice_spatial; // r434: pure spatial-voice params (block occlusion + reverb estimate)
pub mod server_config; // r440: headless server CLI config (pure, tested)
pub mod current_force; // r442: flow→drift force + erosion potential (pure, tested)
pub mod water_sim_core; // r443: pure settle core extracted for testing (water-vs-water, cave-fill, r441 regression)
pub mod water_react;
pub mod water_tint; // r459: water surface albedo from temperature/salinity/erosion/depth (pure, tested)
/// Branching timelines for timebending — fork-on-past-edit over the rollback
/// model, with structure decay/erosion forward and stale-branch propagation.
pub mod timeline;
pub mod state;
pub mod vision;
pub mod eyes;
pub mod localize;
pub mod awareness;
pub mod popgen;
pub mod senses;
pub mod landscape_memory;
pub mod feeding;
pub mod hot_metal;
pub mod popdyn;
pub mod forage_memory;
pub mod forage_sim;
pub mod posture;
pub mod collision_probe;
pub mod body_collision;
pub mod body_scale;
pub mod stamina;
pub mod service_registry;
pub mod service_registry_gen;
pub mod service_registry_gen2;
pub mod kernel_registry_gen;
pub mod kernel_registry_gen2;
pub mod kernel_registry;
pub mod service_registry_batch;
pub mod fauna_mind;
pub mod creature_from_image;
pub mod creature_grime;
pub mod mob_forage_memory;
pub mod spatial_audio;
pub mod voxel_text;
pub mod turning;
pub mod deform;
pub mod ragdoll;
pub mod liveliness;
pub mod rest_cycle;
pub mod locomotion_sim;
pub mod head_steer;
pub mod grounding;
pub mod merge_contract;
pub mod stumble;
pub mod cadence;
pub mod surface_cache;
pub mod leap;
pub mod water_sight;
pub mod concealment;
pub mod lauscher;
pub mod sentinel;
pub mod elemental;
pub mod stature;
pub mod lightning;
pub mod armor;
pub mod item_entity;
pub mod item_brain;
pub mod emission;
pub mod column_memo;
pub mod ai_budget;
pub mod buddy_pool;
pub mod neighbor_grid;
pub mod carry;
pub mod shield;
pub mod gravity;
pub mod fall;
pub mod pylon;
pub mod summon;
pub mod projectile;
pub mod affliction;
pub mod combat;
pub mod joint;
pub mod skills;
pub mod articulation;
pub mod dragon;
pub mod ambient;
pub mod genetics;
pub mod diffusion;
pub mod elements;
pub mod exec_tier;
pub mod perception;
pub mod step_height;
pub mod nav_query;
pub mod dragon_elemental;
pub mod touch_rays;
pub mod path_memory;
pub mod large_body_grounding;
pub mod squeeze;
pub mod fish_school;
pub mod swim;
pub mod attach_frame;
pub mod turtle;
pub mod item_genetics;
pub mod item_breeding; // r594: generational breeding-loop policy atop item_genetics
pub mod dragon_signature;
pub mod mob_memory;
pub mod moving_platform; // r594: deterministic platform paths + rider carry
pub mod portal_graph; // r594: dimension/portal data model + linking/traversal
pub mod block_placement; // r594: general block-placement validation (building)
pub mod item_category; // r594: item->filter category classifier (feeds inventory_sort)
pub mod inventory_transfer; // r594: cross-grid stack transfer (quick-move/deposit-all) atop inventory_grid
pub mod infusion; // r594: infusion slot economy + sway-mark dominance (crafting genome)
pub mod armor_set; // r594: armour-set aggregator (piece modifiers + proportional set bonuses)
pub mod object_permanence; // r595: target ghost record + uncertainty growth + give-up (mob brain)
pub mod refresh_snap; // r604: autotune 500Hz target + fps snap ladder
pub mod dragon_energy; // r605: dragon energy pool (6 states + capability gates + feed/expend)
pub mod wing_loading;
pub mod flight_modes; // r675: 7-mode flight FSM (flap/bound/undulate/soar/stoop) from flying_animal_behaviour §2.1
pub mod provocation; // r607: context-sensitive fauna threat response (passive/neutral/hostile + sticky provocation)
pub mod dragon_territory; // r609: age-scaled core/influence dragon territory + deference/conflict
pub mod resonance; // r613: character-panel Resonance Rating (POWER) + Resilience mitigation
pub mod skill_constellation; // r614: bending-discipline skill tree (nodes/prereqs/env-gate/unlock)
pub mod ability_bar; // r615: five-slot technique binding row (composes with skill_constellation)
pub mod player_rank; // r616: Resonance Rank progression from lifetime chi (header stat + XP bar)
pub mod panel_tabs; // r617: character-screen top-level tab bar (CHARACTER/CLAN/COLLECTIONS/JOURNEY/INVENTORY)
pub mod sub_discipline; // r618: sub-discipline unlock (resonance_depth>=0.70), depth tier, renown title
pub mod mob_viewer; // r619: 3D orbit/close-up inspection camera (COLLECTIONS mob viewer)
pub mod creative_items; // r620: creative-menu tabs incl Tools/Weapons + search (over item facets)
pub mod hybrid_disciplines; // r621: access gates for hybrid/sub/novel bending disciplines (spec 6-7)
pub mod technique_catalogue; // r622: named technique catalogue (signature->move, cost, tier gate)
pub mod redirect; // r623: REDIRECT counter - route an incoming technique, gain resonance
pub mod locomotion_mode; // r624: walk/swim/fly/fall medium state machine (hysteretic)
pub mod flight_phase; // r625: flight FSM (takeoff/climb/cruise/soar/glide/approach/STOOP) for dragons+fauna; fixes fly-too-low/no-swoop
pub mod player_traits; // r627: gamemode trait layer (10 traits, mode-trait matrix, precedence)
pub mod river_type; // r628: seven river types + gameplay profiles (flow/wave-surf/swim-camera/thirst)
pub mod fog_model; // r641: clean-room physics fog CPU core (Beer-Lambert, height density, phase fns, analytic airmass, fog visibility distance, underwater absorption)
pub mod secondary_stats; // r644: computed Mobility/Resilience/Recovery stats (move-speed mult, damage reduction, chi regen) from gear/discipline/companions/level
pub mod tutorial_ladder; // r645: practice-world 7-beat training ladder + Choir harmonisation — doubles as a runnable bending debug suite
pub mod tutorial_arena; // r661: bounded practice-arena geometry for the Bending Tutorial world
pub mod stroke_learning; // r646: per-player bending voice alias inference (learns pronunciation from near-miss + self-correction)
pub mod shape_word; // r646: parse a spoken bending phrase -> ShapeCmd{form,qualifiers,state} (Earth+Water vocab, hollow->Deep)
pub mod shape_scale; // r647: resolve shape dimensions + Earth material from qualifiers + structural factor S (composes with shape_word)
pub mod water_shape; // r648: water-shape lifecycle (hold/decay/flow/break/frozen -> revert to fluid); composes with shape_word::StateMod
pub mod bending_field;
pub mod waterfall_poi; // r677: waterfall POI environmental-property profiles (fluid_pois_waterfalls §1.1)
pub mod flora_genetics; // r652: diploid tree genetics (FR/WR/SR/SP alleles, dominance, seed inheritance+mutation) + colour extension (pigment->absorption/leaf-temp, hue, colour-trait linkage, fauna crypsis)
pub mod trophic_web; // r653: predator-prey graph (land+sea) + self-rebalancing population dynamics (predation pressure/food availability/migration pull)
pub mod time_interval_set; // r629: bounded tick-interval set algebra (union/intersection/difference) for time-bending queries
pub mod env_deform;
pub mod flex_bend;
pub mod foot_ik;
pub mod strafe_face;

// Round 6 — Worker 01 (game systems). Self-contained, pure gameplay-state
// modules: chat-command parsing, the inventory grid, crafting recipe matching,
// and recipe indexing. Each defines its own item id/stack locally for now, so
// they are reached by path (e.g. `sim::inventory_grid::ItemId`) rather than
// re-exported here; unifying on a shared item type is a follow-up.
pub mod command_parse;
pub mod game_mode; // r335-gamemode: gamemode switch model + caps
pub mod world_management; // r335-worldmgmt: world-mgmt settings + Open-to-DHT
pub mod crafting;
pub mod creative_tabs;
pub mod inventory_grid;
pub mod recipe_book;

// Round 7 — Worker 01 (sim, reactive tier). Cheap, pure, every-tick cores:
// emissive glow from element+arousal, gentle survival drives, and fall-impact
// consequences. Each is self-contained (defines its own small input types) and
// is reached by path rather than re-exported here.
pub mod emissive;
pub mod fall_impact;
pub mod survival;

// Round 8 — Worker 01 (sim, reactive tier). Pure every-tick decision cores:
// predator/prey flee-or-hunt choice, the egg genome (round-trip + seeded
// breeding), and diet-aware block foraging selection. Each is self-contained
// (defines its own small input types) and is reached by path, not re-exported.
pub mod egg_genome;
pub mod feeding_select;
pub mod predator_prey;
pub mod prey_scan; // r594: perception-bounded prey/threat selection (neighbor_grid index + predator_prey::decide)

// Round 9 — Worker 01 (sim, reactive tier). Pure every-tick cores: 3D local
// swim/fly step direction, the element-crossing (hybrid) rule table, and the
// bond/taming state machine. Each is self-contained (own input types) and is
// reached by path, not re-exported here.
pub mod bond_tame;
pub mod hybrid_rules;
pub mod nav3d;

// Round 10 — Worker 01 (sim, reactive tier). Pure every-tick cores: the voice
// genome + Mimic interpolation, a capacity-bounded decaying landmark memory,
// and keystone-grove vitality. Each is self-contained (own input types) and is
// reached by path, not re-exported here.
pub mod grove_decay;
pub mod landmark_memory;
pub mod sound_genome;

// Round 11 — Worker 01 (sim, reactive tier). Pure cores: three-rule boid
// flocking, the dragon symbol-language encode/decode/similarity grammar, and
// the rollback-netcode input ring buffer + predictor. Each is self-contained
// (own input types) and is reached by path, not re-exported here.
pub mod boids;
pub mod dragon_language;
pub mod input_prediction;

// Round 12 — Worker 01 (sim, reactive tier). Pure cores: the Mimic learning a
// voice over exposure, carrier pollination routing (nearest unvisited bloom),
// and PvE combat hit resolution. Each is self-contained (own input types) and
// is reached by path, not re-exported here.
pub mod combat_resolve;
pub mod mimic_learn;
pub mod pollination_route;

// Round 13 — Worker 01 (sim, microopt). Batched cores, each shipping a scalar
// reference plus a batched impl that is bit-for-bit identical to it: perception
// over many agents, foot/ground settle, and the flock step (mirroring r11
// boids). Self-contained; reached by path, not re-exported here.
pub mod batch_boids;
pub mod batch_perception;
pub mod batch_settle;

// Round 14 — Worker 01 (sim). Dragon animation + fast spawning: torso breathing
// oscillation, sound→jaw/neck vocal pose, and a microopt batched genome→
// phenotype expression (scalar reference + bit-identical batch). Each is
// self-contained (own input types) and is reached by path, not re-exported.
pub mod batch_gene;
pub mod breathe;
pub mod vocal_pose;

// Round 15 — Worker 01 (sim). Toe/claw curl from grip + gait, and a microopt
// batched breathe (scalar reference + bit-identical batch). Each is
// self-contained (own input types) and is reached by path, not re-exported.
// (This round also adds docs/design/STRUCTURES_creature.md — a design doc, no code.)
pub mod batch_breathe;
pub mod claw_curl;

// Round 16 — Worker 01 (sim). Mob + item + animation: eyelid blink,
// velocity-driven appendage ripple, weighted mob spawn table, weighted loot
// roll, item durability wear/repair, and a microopt batched blink (scalar
// reference + bit-identical batch). Each is self-contained (own input types)
// and is reached by path, not re-exported here.
pub mod appendage_ripple;
pub mod batch_blink;
pub mod blink;
pub mod item_durability;
pub mod loot_roll;
pub mod mob_spawn_table;

// Round 17 — Worker 01 (sim). Entity-body motion + mob AI: spine turn arc,
// per-section terrain-follow amplitude, top-threat scoring, fight/flee decision,
// herd cohesion+separation steering, and a microopt batched appendage ripple
// (scalar reference + bit-identical batch). Each is self-contained (own input
// types) and is reached by path, not re-exported here.
pub mod batch_appendage_ripple;
pub mod body_arc;
pub mod flee_decision;
pub mod herd_cohesion;
pub mod segment_lerp;
pub mod threat_assess;

// Round 18 — Worker 01 (sim). Items + progression + mob state: rarity-weighted
// item modifier roll, stack merge with overflow, superlinear xp curve, stacking
// status effect tick, time+distance aggro decay, and a microopt batched loot
// roll (scalar reference + bit-identical batch). Each is self-contained (own
// input types) and is reached by path, not re-exported here.
pub mod aggro_decay;
pub mod batch_loot_roll;
pub mod item_modifier;
pub mod stack_merge;
pub mod status_effect;
pub mod xp_curve;

// Round 19 — Worker 01 (sim). Mob behavior + life cycle: pack flanking ring,
// territorial intrusion, day/night sleep cycle, hunger drive, mate-compatibility
// scoring, and a microopt batched threat scoring (scalar reference + bit-
// identical batch). Each is self-contained (own input types) and is reached by
// path, not re-exported here.
pub mod batch_threat_assess;
pub mod hunger_drive;
pub mod mate_select;
pub mod pack_hunt;
pub mod sleep_cycle;
pub mod territory;

// Round 20 — Worker 01 (sim). Horn system + obsession + mob: horn archetype
// roll, horn charge accrual/discharge, obsession→drive weights, scent-gradient
// tracking, and a microopt batched status-effect tick (scalar reference + bit-
// identical batch). Each is self-contained (own input types) and is reached by
// path, not re-exported here.
pub mod batch_status_effect;
pub mod horn_charge;
pub mod horn_type;
pub mod obsession;
pub mod scent_track;

// Round 21 — Worker 01 (sim). Abilities + items + mob: horn-discharge ability
// cast, socketed-gem stat application, item-set bonus thresholds, seasonal
// migration direction, and a microopt batched aggro decay (scalar reference +
// bit-identical batch). Each is self-contained (own input types) and is reached
// by path, not re-exported here.
pub mod ability_cast;
pub mod batch_aggro_decay;
pub mod gem_socket;
pub mod migrate;
pub mod set_bonus;

// Round 22 — Worker 01 (sim). GPU-parallelizable sim references (CPU references
// a GPU field-kernel mirrors): logistic population step, advect+diffuse pollen
// spread, batched plant genome→phenotype expression, ability-aura AoE falloff,
// and a microopt batched horn-charge (scalar reference + bit-identical batch).
// Each is self-contained (own input types) and is reached by path, not
// re-exported here.
pub mod ability_aura;
pub mod batch_horn_charge;
pub mod plant_gene_express;
pub mod pollen_spread;
pub mod popdyn_step;

// Round 23 — Worker 01 (sim). Combat/status/spawn + perf: crit roll multiplier,
// status-stack add/cap, proximity threat sharing, spawn-budget rate limiting,
// and a microopt batched ability cast (scalar reference + bit-identical batch).
// Each is self-contained (own input types) and is reached by path, not
// re-exported here.
pub mod batch_ability_cast;
pub mod crit_roll;
pub mod spawn_budget;
pub mod status_stack;
pub mod threat_share;

// Round 24 — Worker 01 (sim). Abilities/mobs + perf: enchant-level stat apply,
// fish schooling steer, exponential scent-trail decay, ability cooldown timer,
// and a microopt batched crit roll (scalar reference + bit-identical batch).
// Each is self-contained (own input types) and is reached by path, not
// re-exported here.
pub mod ability_cooldown;
pub mod batch_crit_roll;
pub mod enchant_apply;
pub mod scent_decay;
pub mod school_fish;

// Round 25 — Worker 01 (sim). Taming/mounts/abilities + perf: feed-based taming
// progress, mount stamina drain/regen, combo damage multiplier, scent-gradient
// follow direction, and a microopt batched status-stack add (scalar reference +
// bit-identical batch). Each is self-contained (own input types) and is reached
// by path, not re-exported here.
pub mod ability_combo;
pub mod batch_status_stack;
pub mod mount_stamina;
pub mod scent_gradient;
pub mod taming;

pub use cognition::{Landmark, LandmarkTag, Memory};
pub use driver::{RawInput, RenderPose, SimDriver};
pub use fixed::{Fixed, Vec3};
pub use goals::{choose as choose_goal, Goal, Signals};
pub use mob::{AiMode, AiState, Mob, MobKind};
pub use net::{Ingested, Session, Transport, UdpTransport};
pub use personality::Disposition;
pub use physics::{BlockPhysics, CollisionClass, FlatWorld, SceneWorld, WorldQuery};
pub use rollback::{PlayerId, Rollback};
pub use state::{checksum, step, Character, Input, SimState, Tick};
pub use vision::{can_see, line_of_sight};
pub mod guard_side;
pub mod vital_cover;
pub mod engage_distance;
pub mod flank_turn_rate;
pub mod batch_spawn_budget;
pub mod limb_detail_flag;
pub mod damage_shard;
pub mod dismember_check;
pub mod stance_bias;
pub mod batch_scent_decay;
pub mod pack_flank;
pub mod cover_seek;
pub mod focus_fire;
pub mod disengage;
pub mod batch_guard_side;
pub mod lift_force;
pub mod drag_force;
pub mod glide_ratio;
pub mod bank_radius;
pub mod batch_flap_thrust;
pub mod gene_crossover;
pub mod gene_mutate;
pub mod trait_dominance;
pub mod hatch_time;
pub mod batch_gene_crossover;
pub mod elemental_multiplier;
pub mod burn_tick;
pub mod freeze_slow;
pub mod resist_apply;
pub mod batch_burn_tick;
pub mod craft_quality;
pub mod market_price;
pub mod skill_gain;
pub mod repair_cost;
pub mod batch_market_price;
pub mod heuristic_octile;
pub mod path_cost;
pub mod step_toward;
pub mod turn_penalty;
pub mod batch_path_cost;
pub mod breath_cone;
pub mod breath_damage;
pub mod ability_charge_time;
pub mod mana_cost;
pub mod batch_breath_damage;
pub mod sight_detect;
pub mod hearing_radius;
pub mod scent_strength;
pub mod awareness_blend;
pub mod batch_sight_detect;
pub mod stat_total;
pub mod carry_weight;
pub mod poison_tick;
pub mod stun_duration;
pub mod batch_stat_total;
pub mod projectile_drop;
pub mod ballistic_range;
pub mod knockback_impulse;
pub mod terminal_velocity;
pub mod dive;
pub mod batch_knockback_impulse;
pub mod memory_decay;
pub mod threat_memory;
pub mod habituation;
pub mod reward_estimate;
pub mod batch_memory_decay;
pub mod affix_count;
pub mod affix_magnitude;
pub mod rarity_tier;
pub mod set_tier;
pub mod batch_affix_magnitude;
pub mod breath_falloff;
pub mod breath_recharge;
pub mod roar_fear;
pub mod wing_buffet_force;
pub mod wing_buffet;
pub mod batch_breath_falloff;
pub mod drop_chance;
pub mod loot_quantity;
pub mod magic_find_weight;
pub mod status_combo_multiplier;
pub mod batch_drop_chance;
pub mod wet_lightning_bonus;
pub mod freeze_shatter_bonus;
pub mod diminishing_returns_resist;
pub mod debuff_duration_scale;
pub mod batch_wet_lightning_bonus;
pub mod hit_chance;
pub mod damage_after_armor;
pub mod block_reduction;
pub mod overkill_damage;
pub mod batch_damage_after_armor;
pub mod aggression_level;
pub mod flee_threshold;
pub mod target_priority;
pub mod patrol_wait_time;
pub mod batch_target_priority;

// Round 28 — DRAGONS / material genes (pure cores). Gene value in → material
// parameter out, monotonic and range-clamped; the shader consumption of these
// values is the coordinator's. Each is self-contained (own input/output types)
// and is reached by path, not re-exported here.
pub mod armor_reflect;
pub mod wing_opacity;
pub mod scale_gloss;

// Round 28 — SHADERS thread (SHD). Pure CPU reference for temporal froxel
// accumulation (neighbourhood-clamp + exponential blend); the coordinator
// mirrors it into the froxel shader and proves GPU↔CPU parity.
pub mod temporal_accumulate;
pub mod item_resolve;
pub mod dragon_blueprint_from_genome;
pub mod craft_resolve;
pub mod dragon_growth_stage;
pub mod item_appraise;
pub mod seed_dispersal;
pub mod field_diffuse_ref;
pub mod foot_reach;
pub mod combat_stance;
pub mod forage_target;
pub mod batch_body_arc;
pub mod batch_engage_distance;
pub mod descriptor_layout_diff;
pub mod quad_stance;
pub mod quad_reposition;
pub mod behavior_arbiter;
pub mod salvage_yield;
pub mod stack_deposit;
pub mod visible_delta;
pub mod chunk_frustum_cull;
pub mod artifact_imbue;
pub mod dragon_attunement;
pub mod germination;
pub mod gesture_resolve;
pub mod draw_metadata_pack;
pub mod affinity_access;
pub mod essence_refine;
pub mod draw_order_sort;
pub mod canopy_field_shift;
pub mod combat_reclaim;
pub mod frustum_plane_extract;
pub mod resonance_state;
pub mod incubation_imprint;
pub mod dragon_material_yield;
pub mod hybrid_form;
pub mod frame_pacer;
pub mod cull_dispatch_split;
pub mod upload_priority_sort;
pub mod dragon_archetype;
pub mod shadow_refresh_plan;
pub mod bending_runtime;
pub mod flora_harvest;
pub mod item_compare;
pub mod lod_budget;
pub mod inventory_sort;
pub mod save_slots;
pub mod render_plan;
pub mod gesture_detect;
pub mod quadruped_gait;
pub mod body_posture;
pub mod head_scan;
pub mod roar_sample;
pub mod impact_response;
pub mod predation;
pub mod dragon_forge;
pub mod bending_focus;
pub mod indirect_batch;
pub mod submit_schedule;
pub mod pioneer_flora;
pub mod cave_ore; // r516: depth-weighted ore for the runtime cave-enrichment pass
pub mod whitelist; // r518: open-server access control (allow-list + open/closed policy)
pub mod ui_state; // r525: unified two-level UI state machine (AppMode + MenuState)
pub mod plant_growth;
pub mod perf_primitives;
pub mod main_menu;
pub mod sound_masking;
pub mod fog_volume;
pub mod light_physics;
pub mod shading_uniforms;
pub mod erosion;
pub mod water_physics;
pub mod water_batch; // r395: parallel water simulation across the rayon worker lane
pub mod flora_spread;
pub mod plant_growth_env;
pub mod plant_survival;
pub mod ingame_menu;
pub mod plant_tree_genetics;
pub mod bee_pollination;
pub mod wind_field;
pub mod wind_perturb;
pub mod secondary_motion;
pub mod movement_tune;
pub mod mp_dht;
pub mod env_frame;
pub mod env_batch; // r402: parallel per-chunk water/env step (rain/flow/drain/moisture/erosion)
pub mod water_texture_batch; // r402: parallel water caustic texture generation
pub mod mountain_sources; // r402: orographic precip (weather humidity → mountain springs), threaded
pub mod forge_lod;
pub mod dragon_flight;
pub mod breath_attack;
pub mod dragon_breeding;
pub mod dragon_locomotion;
pub mod dragon_observe;
pub mod dragon_hearing;
pub mod dragon_activity;
pub mod dragon_bending;
pub mod dragon_bending_ai;
pub mod dragon_population;
pub mod particulate_field;
pub mod dragon_air;
pub mod dragon_mode;
pub mod dragon_hunt;
pub mod mob_pixel_peeper;
pub mod camera_tremble;
pub mod plant_root_system;
pub mod plant_propagation;
pub mod verlet_strand;
pub mod aabb_resolve;
pub mod pcf_shadow;
pub mod dither_ordered;
pub mod buoyancy;
pub mod dragon_wing_flex;
pub mod menu_nav;
pub mod legendary_weapon;
pub mod net_rendezvous;
pub mod title_flow;
pub mod camera_collision;
pub mod viewport_split;
pub mod sound_mix;
pub mod dragon_thirst;
pub mod fauna_behavior;
pub mod fauna_roster;
pub mod fauna_gait;
pub mod tree_pollination;
pub mod combustion_spread;
pub mod river_flow;
pub mod bending_stormstrike;
pub mod net_candidates;
pub mod net_channels; // r330: multiplayer QoS channel table + per-tick byte budgeter (§6)
// r330 returns (coordinator-reconciled mod lines for the merged worker modules):
pub mod item_tooltip; // r330-items: pure item/affix/lore tooltip composition
pub mod roamer_cap; // r330-mobs: spawn-admission cap (no despawn)
pub mod surface_scan; // r330-world: top-solid-Y from generated voxels (FP32/FP64 fix core)
pub mod foliage_sway; // r330-gpu: foliage wind-sway vertex tag (§4b)
pub mod legendary_loot; // r331: legendary weapon drop-selection core (swords)
pub mod frame_correlation; // r332-perf: per-frame spike correlation probe (coordinator)
pub mod mesh_budget; // hh-r332-t1: per-frame near-mesh build budget
pub mod record_zones; // hh-r332-t2: record-pass sub-zone accounting
pub mod draw_batch; // hh-r332-t2: instanced-draw grouping
pub mod mesh_partition; // hh-r332-t3: directional sub-meshes + neighbour dirty-scope
pub mod audio_occlusion; // r332-sound: occlusion-aware spatial audio
pub mod sound_resolve; // r381-sound: distance+occlusion+doppler+reverb composite
pub mod voice_mix; // r382-sound: budgeted voice selection over sound_resolve
pub mod creature_hearing; // r383-sound: occlusion + hearing_radius perception
pub mod sound_stereo; // r384-sound: constant-power stereo pan over sound_resolve
pub mod session_wire; // r386-mp: LAN gossip wire format for WorldAdvert discovery
pub mod lan_discovery; // r386-mp: UDP multicast transport wrapping session_wire
pub mod lan_session_wire; // r397-mp: unicast LAN join handshake (JOIN/ACCEPT/DENY)
pub mod bending_compose; // r332-bending: end-to-end bending composition
pub mod mob_limb_girth;
pub mod dragon_brain;
pub mod game_flow;
pub mod world_catalog;
pub mod new_world_config;
pub mod settings_model;
pub mod pause_overlay;
pub mod menu_screen_layout;
pub mod session_mode;
pub mod unendlich_id; // r197: persistent player id + rollback-default netmode
pub mod player_bending;
pub mod bending_charge;
pub mod bending_heavy_brine;
pub mod bending_thermal_phase;
pub mod bending_information;
pub mod bending_targeting;
pub mod bending_feedback;
pub mod music_director;
pub mod new_game_wizard;
pub mod player_views;
pub mod menu_controller;
pub mod item_drop_resolve;
pub mod mob_spawn_director;
pub mod menu_router;
pub mod inventory_txn;
pub mod vertex_ao;
pub mod world_launch;
pub mod biome_climate;
pub mod ore_resource_veins;
pub mod terrain_variation;
pub mod ecology_population;
pub mod species_distribution;
pub mod survival_exposure;
pub mod forage_harvest;
pub mod hotbar;
pub mod title_cinematic;
pub mod lauscher_lair;
pub mod dht_join;

pub mod shader_from_image;
pub mod viewmodel;
pub mod precipitation;
pub mod particles;   // r263 merge: pooled particle system (PARTICLES worker, 8 tests)
pub mod mob_bands;   // r263 merge: band-aware mob tick decision (T-MOBS worker, 6 tests)
pub mod convergence; // r273: convergence classes + free structural pre-filter (deep-time)
pub mod sim_scheme;  // r274: adaptive scheme selector (sim-it / vector / drive+prefill)
pub mod region_digest; // r274: stable coarse-state content digest (seed/branch/merge)
pub mod sim_vector;  // r275: committed checkpoint-director (span/seed/waypoints/deltas/events)
pub mod sim_chain;   // r276: multi-vector chain assembler + convergence-merge collapse
pub mod acoustics;   // r285: fingerprint-based sound classification + masking + echo (creature hearing)
pub mod poi;         // r286: POI scoring (food/shelter/friend/foe) → arbiter Drives + nav target
pub mod disposition; // r289: express(genome) → Disposition (poi weights/boldness/acuity/sight) [genome loci step 1]
pub mod spatial_grid; // r292: coarse grid turning O(n²) all-pairs into O(n) neighbour-pairs (mate-finding, prey)
pub mod world_prober; // r296: unified within-batch memo for column-keyed world queries (surface + biome), shared across systems
