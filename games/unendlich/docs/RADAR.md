# WIRING RADAR — generated scratchpad (file:line anchors)
# regen: python3 tools/wiring_radar.py   |   verify: ... check FILE   |   ... anchor FILE STR

## A zero-callsite pub fns  (624)
crates/unendlich-engine/src/render_parity.rs:65 pub fn from_rgb8_coverage
crates/unendlich-engine/src/render_parity.rs:103 pub fn cpu_reference
crates/unendlich-engine/src/render_parity.rs:216 pub fn assert_parity
crates/unendlich-engine/src/moisture_field.rs:115 pub fn recenter
crates/unendlich-engine/src/moisture_field.rs:149 pub fn sample_world
crates/unendlich-engine/src/moisture_field.rs:158 pub fn begin_tick
crates/unendlich-engine/src/moisture_field.rs:166 pub fn sample_world_interp
crates/unendlich-engine/src/moisture_field.rs:308 pub fn wind_sampler
crates/unendlich-engine/src/vertex_pack.rs:114 pub fn unpack_vertex
crates/unendlich-engine/src/water_field.rs:136 pub fn tick_chunk_water
crates/unendlich-engine/src/inventory.rs:29 pub fn single
crates/unendlich-engine/src/inventory.rs:146 pub fn move_into
crates/unendlich-engine/src/inventory.rs:199 pub fn is_full_for
crates/unendlich-engine/src/inventory.rs:230 pub fn selected_stack
crates/unendlich-engine/src/inventory.rs:237 pub fn consume_selected
crates/unendlich-engine/src/physics.rs:147 pub fn fly_mode
crates/unendlich-engine/src/physics.rs:161 pub fn noclip
crates/unendlich-engine/src/physics.rs:258 pub fn new_player_with_id
crates/unendlich-engine/src/texture_atlas.rs:55 pub fn tile_uv_inset
crates/unendlich-engine/src/texture_atlas.rs:108 pub fn for_normal
crates/unendlich-engine/src/pose.rs:48 pub fn from_graded
crates/unendlich-engine/src/pose.rs:58 pub fn walking
crates/unendlich-engine/src/pose.rs:169 pub fn animate
crates/unendlich-engine/src/input_capture.rs:176 pub fn latest
crates/unendlich-engine/src/whitelist.rs:87 pub fn serialize
crates/unendlich-engine/src/thread_probe.rs:164 pub fn set_with_context
crates/unendlich-engine/src/thread_probe.rs:174 pub fn set_context
crates/unendlich-engine/src/wing_membrane.rs:40 pub fn triangle_count
crates/unendlich-engine/src/wetness.rs:114 pub fn pack_region
crates/unendlich-engine/src/wetness.rs:260 pub fn apply_wetness
crates/unendlich-engine/src/wetness.rs:274 pub fn wet_albedo_factor
crates/unendlich-engine/src/creature_viewer.rs:51 pub fn role_color
crates/unendlich-engine/src/spawn.rs:350 pub fn resolve_use_seeded
crates/unendlich-engine/src/sumtree.rs:61 pub fn from_field
crates/unendlich-engine/src/sumtree.rs:91 pub fn region_sum
crates/unendlich-engine/src/creature_design.rs:169 pub fn jaw_hinge
crates/unendlich-engine/src/creature_design.rs:218 pub fn gaze_forward
crates/unendlich-engine/src/creature_design.rs:244 pub fn resting_gaze
crates/unendlich-engine/src/creature_design.rs:996 pub fn random
crates/unendlich-engine/src/creature_design.rs:1006 pub fn with_choice
crates/unendlich-engine/src/creature_design.rs:1012 pub fn with_element
crates/unendlich-engine/src/mob.rs:30 pub fn can_fly
crates/unendlich-engine/src/mob.rs:36 pub fn strike
crates/unendlich-engine/src/blur_kernel.rs:18 pub fn variance
crates/unendlich-engine/src/hotbar.rs:145 pub fn cycle_tab
crates/unendlich-engine/src/hotbar.rs:155 pub fn on_key
crates/unendlich-engine/src/action_queue.rs:79 pub fn press
crates/unendlich-engine/src/action_queue.rs:86 pub fn release
crates/unendlich-engine/src/action_queue.rs:107 pub fn dropped
crates/unendlich-engine/src/moisture_texture.rs:134 pub fn initialize_zero
crates/unendlich-engine/src/moisture_texture.rs:261 pub fn descriptor_image_info
crates/unendlich-engine/src/sky.rs:111 pub fn is_night
crates/unendlich-engine/src/ui_feel.rs:73 pub fn shimmer_phase
crates/unendlich-engine/src/ui_feel.rs:85 pub fn parallax_offset
crates/unendlich-engine/src/ui_feel.rs:110 pub fn set_target
crates/unendlich-engine/src/keybindings.rs:179 pub fn qwerty_wasd_preset
crates/unendlich-engine/src/keybindings.rs:245 pub fn get_action
crates/unendlich-engine/src/keybindings.rs:252 pub fn actions_for_key
crates/unendlich-engine/src/keybindings.rs:261 pub fn set_binding
crates/unendlich-engine/src/keybindings.rs:292 pub fn bind_mouse
crates/unendlich-engine/src/keybindings.rs:300 pub fn is_action_pressed_mouse
crates/unendlich-engine/src/keybindings.rs:309 pub fn is_action_just_pressed_mouse
crates/unendlich-engine/src/keybindings.rs:313 pub fn save_to_file
crates/unendlich-engine/src/keybindings.rs:326 pub fn load_from_file
crates/unendlich-engine/src/keybindings.rs:348 pub fn load_or_default
crates/unendlich-engine/src/settings_schema.rs:106 pub fn to_kv
crates/unendlich-engine/src/settings_schema.rs:125 pub fn from_kv
crates/unendlich-engine/src/view_cull.rs:51 pub fn cull_mask
crates/unendlich-engine/src/view_cull.rs:57 pub fn visible_indices
crates/unendlich-engine/src/emblem.rs:53 pub fn for_skill
crates/unendlich-engine/src/recipe.rs:130 pub fn recipes_for
crates/unendlich-engine/src/recipe.rs:135 pub fn recipes_at
crates/unendlich-engine/src/player.rs:53 pub fn new_at_position
crates/unendlich-engine/src/player.rs:115 pub fn update_movement_modifiers
crates/unendlich-engine/src/server_connect.rs:63 pub fn to_string_addr
crates/unendlich-engine/src/server_connect.rs:72 pub fn is_allowed
crates/unendlich-engine/src/permissions.rs:85 pub fn survival_player
crates/unendlich-engine/src/permissions.rs:98 pub fn spectator
crates/unendlich-engine/src/permissions.rs:112 pub fn can_use_command
crates/unendlich-engine/src/collision.rs:37 pub fn from_center_half
crates/unendlich-engine/src/collision.rs:51 pub fn half_extents
crates/unendlich-engine/src/collision.rs:74 pub fn expanded
crates/unendlich-engine/src/collision.rs:98 pub fn surface_area
crates/unendlich-engine/src/collision.rs:141 pub fn intersects_segment
crates/unendlich-engine/src/collision.rs:183 pub fn intersects_aabb
crates/unendlich-engine/src/fabric.rs:118 pub fn tip
crates/unendlich-engine/src/fabric.rs:124 pub fn total_length
crates/unendlich-engine/src/debug_overlay.rs:113 pub fn primitive_count
crates/unendlich-engine/src/storm_field.rs:78 pub fn airborne_total
crates/unendlich-engine/src/item.rs:65 pub fn emblem
crates/unendlich-engine/src/item.rs:121 pub fn from_ore
crates/unendlich-engine/src/item.rs:179 pub fn is_spawn_egg
crates/unendlich-engine/src/item.rs:241 pub fn creative_palette
crates/unendlich-engine/src/sampling.rs:152 pub fn occlusion
crates/unendlich-engine/src/sampling.rs:165 pub fn immersion
crates/unendlich-engine/src/sampling.rs:177 pub fn settle_normal
crates/unendlich-engine/src/snow.rs:56 pub fn depth_blocks
crates/unendlich-engine/src/snow.rs:150 pub fn snowfall_amount
crates/unendlich-engine/src/creature.rs:256 pub fn vision_coverage
crates/unendlich-engine/src/creature.rs:478 pub fn with_yaw
crates/unendlich-engine/src/creature.rs:494 pub fn settle_posture
crates/unendlich-engine/src/creature.rs:516 pub fn quadruped_feet
crates/unendlich-engine/src/creature.rs:561 pub fn max_step_height
crates/unendlich-engine/src/creature.rs:568 pub fn footfall
crates/unendlich-engine/src/creature.rs:583 pub fn with_part
crates/unendlich-engine/src/creature.rs:586 pub fn add_part
crates/unendlich-engine/src/creature.rs:592 pub fn capabilities
crates/unendlich-engine/src/creature.rs:595 pub fn add_limb
crates/unendlich-engine/src/creature.rs:601 pub fn aabb_count
crates/unendlich-engine/src/creature.rs:652 pub fn combined_aabb
crates/unendlich-engine/src/creature.rs:673 pub fn collides_with
crates/unendlich-engine/src/inventory_screen.rs:70 pub fn wants_mouse_captured
crates/unendlich-engine/src/inventory_screen.rs:87 pub fn toggle
crates/unendlich-engine/src/inventory_screen.rs:95 pub fn set_tab
crates/unendlich-engine/src/inventory_screen.rs:106 pub fn left_click
crates/unendlich-engine/src/inventory_screen.rs:135 pub fn right_click
crates/unendlich-engine/src/occupancy.rs:32 pub fn from_voxels
crates/unendlich-engine/src/occupancy.rs:99 pub fn first_solid_ray
crates/unendlich-engine/src/occupancy.rs:154 pub fn coarse_occlusion
crates/unendlich-engine/src/light.rs:173 pub fn propagate_block_light
crates/unendlich-engine/src/light.rs:290 pub fn add_light
crates/unendlich-engine/src/light.rs:320 pub fn remove_light
crates/unendlich-engine/src/light.rs:372 pub fn skylight_columns
crates/unendlich-engine/src/creature_viewer_ui.rs:191 pub fn creature_viewer_window
crates/unendlich-engine/src/input_codes.rs:113 pub fn from_name
crates/unendlich-engine/src/input_codes.rs:119 pub fn to_i32
crates/unendlich-engine/src/input_codes.rs:124 pub fn from_i32
crates/unendlich-engine/src/input_codes.rs:234 pub fn input_event_from_glfw
crates/unendlich-engine/src/input_codes.rs:262 pub fn key_action
crates/unendlich-engine/src/input_codes.rs:285 pub fn keycode_from_winit
crates/unendlich-engine/src/thermal.rs:249 pub fn thermal_shock
crates/unendlich-engine/src/thermal.rs:342 pub fn resolve_thermal_failures
crates/unendlich-engine/src/telemetry/achievements.rs:68 pub fn register_all
crates/unendlich-engine/src/telemetry/mod.rs:80 pub fn live
crates/unendlich-engine/src/telemetry/mod.rs:83 pub fn register_specs
crates/unendlich-engine/src/telemetry/mod.rs:104 pub fn bump
crates/unendlich-engine/src/telemetry/mod.rs:108 pub fn set_counter
crates/unendlich-engine/src/telemetry/mod.rs:112 pub fn achievements
crates/unendlich-engine/src/telemetry/mod.rs:118 pub fn build_report
crates/unendlich-engine/src/telemetry/report.rs:124 pub fn append_to
crates/unendlich-engine/src/app/menu_view.rs:67 pub fn menu_ui
crates/unendlich-engine/src/app/menu_view.rs:190 pub fn title_flythrough_ui
crates/unendlich-engine/src/app/menu_view.rs:276 pub fn map_click
crates/unendlich-engine/src/app/mod.rs:173 pub fn menu_mut
crates/unendlich-engine/src/app/mod.rs:196 pub fn cinematic_elapsed
crates/unendlich-engine/src/app/mod.rs:297 pub fn gameplay_tick
crates/unendlich-engine/src/world/water_field.rs:75 pub fn seed_from_chunk
crates/unendlich-engine/src/world/water_field.rs:202 pub fn absorb_at
crates/unendlich-engine/src/world/ticks.rs:350 pub fn strided
crates/unendlich-engine/src/world/field_brush.rs:166 pub fn apply_brush_3d
crates/unendlich-engine/src/world/field_brush.rs:179 pub fn apply_brush_2d
crates/unendlich-engine/src/world/plant_exotics.rs:117 pub fn effect_at
crates/unendlich-engine/src/world/span_alloc.rs:52 pub fn total_free
crates/unendlich-engine/src/world/span_alloc.rs:62 pub fn can_alloc
crates/unendlich-engine/src/world/stem.rs:118 pub fn tip_count
crates/unendlich-engine/src/world/stem.rs:125 pub fn grow_step
crates/unendlich-engine/src/world/lumen.rs:64 pub fn charged
crates/unendlich-engine/src/world/lumen.rs:255 pub fn bench_field
crates/unendlich-engine/src/world/octree_scheduler.rs:148 pub fn drain_ready
crates/unendlich-engine/src/world/octree_scheduler.rs:199 pub fn mark_processed
crates/unendlich-engine/src/world/season.rs:102 pub fn season_for_phase
crates/unendlich-engine/src/world/octree_region.rs:127 pub fn from_octree_chunk_local
crates/unendlich-engine/src/world/octree_region.rs:176 pub fn is_stable
crates/unendlich-engine/src/world/chunk_manager.rs:491 pub fn octree_sampler
crates/unendlich-engine/src/world/chunk_manager.rs:509 pub fn world_epoch
crates/unendlich-engine/src/world/chunk_manager.rs:684 pub fn set_tick_period
crates/unendlich-engine/src/world/chunk_manager.rs:1770 pub fn generator_seed
crates/unendlich-engine/src/world/chunk_manager.rs:1953 pub fn loaded_coords
crates/unendlich-engine/src/world/water_waves.rs:116 pub fn water_wave_height
crates/unendlich-engine/src/world/water_waves.rs:122 pub fn water_wave_normal
crates/unendlich-engine/src/world/plant_blueprint.rs:72 pub fn vertical
crates/unendlich-engine/src/world/spawn_streaming.rs:126 pub fn active_count
crates/unendlich-engine/src/world/pressure_sampler.rs:138 pub fn pressure_samples
crates/unendlich-engine/src/world/block_material.rs:32 pub fn roughness_f
crates/unendlich-engine/src/world/block_material.rs:36 pub fn reflectivity_f
crates/unendlich-engine/src/world/block_material.rs:39 pub fn height_f
crates/unendlich-engine/src/world/block_material.rs:42 pub fn emissive_f
crates/unendlich-engine/src/world/block_material.rs:52 pub fn with_weathering
crates/unendlich-engine/src/world/lod.rs:255 pub fn lod_bands_for_rd
crates/unendlich-engine/src/world/lod.rs:376 pub fn lod_cell_stride
crates/unendlich-engine/src/world/lod.rs:396 pub fn cull_boundary_face
crates/unendlich-engine/src/world/lod.rs:457 pub fn unpack_flags
crates/unendlich-engine/src/world/lod.rs:912 pub fn downsample_for_lod
crates/unendlich-engine/src/world/batch_density_noise.rs:204 pub fn fbm_3d_row_x
crates/unendlich-engine/src/world/cave_field.rs:182 pub fn slice_void_counts
crates/unendlich-engine/src/world/surface.rs:81 pub fn face_mask
crates/unendlich-engine/src/world/surface.rs:262 pub fn face
crates/unendlich-engine/src/world/effects.rs:80 pub fn verb_effects
crates/unendlich-engine/src/world/biome_loader.rs:267 pub fn get_by_id
crates/unendlich-engine/src/world/biome_loader.rs:314 pub fn load_directory
crates/unendlich-engine/src/world/biome_loader.rs:345 pub fn load_file
crates/unendlich-engine/src/world/attributes.rs:119 pub fn set_base
crates/unendlich-engine/src/world/attributes.rs:147 pub fn set_role_level
crates/unendlich-engine/src/world/brick_occupancy.rs:59 pub fn needs_mesh
crates/unendlich-engine/src/world/brick_occupancy.rs:70 pub fn exposed_faces
crates/unendlich-engine/src/world/flow_field.rs:132 pub fn step_target
crates/unendlich-engine/src/world/shader_residency.rs:47 pub fn is_resident
crates/unendlich-engine/src/world/shader_residency.rs:51 pub fn resident_count
crates/unendlich-engine/src/world/mob_persistence.rs:74 pub fn mark_removed
crates/unendlich-engine/src/world/mob_persistence.rs:81 pub fn save
crates/unendlich-engine/src/world/mob_persistence.rs:98 pub fn is_removed
crates/unendlich-engine/src/world/mob_persistence.rs:111 pub fn removed_count
crates/unendlich-engine/src/world/mob_persistence.rs:115 pub fn moved_count
crates/unendlich-engine/src/world/mob_persistence.rs:124 pub fn to_bytes
crates/unendlich-engine/src/world/mob_persistence.rs:152 pub fn from_bytes
crates/unendlich-engine/src/world/mob_persistence.rs:245 pub fn resolve_region
crates/unendlich-engine/src/world/world_model.rs:97 pub fn at_block_nearest
crates/unendlich-engine/src/world/world_model.rs:108 pub fn clear_cache
crates/unendlich-engine/src/world/texture_mapping.rs:71 pub fn tile_for_face
crates/unendlich-engine/src/world/texture_mapping.rs:139 pub fn face_uv
crates/unendlich-engine/src/world/ecosystem_census.rs:128 pub fn report
crates/unendlich-engine/src/world/ecosystem_census.rs:176 pub fn census
crates/unendlich-engine/src/world/gpu_mesher_vk.rs:294 pub fn pool_buffer
crates/unendlich-engine/src/world/gpu_mesher_vk.rs:302 pub fn mesh_batch
crates/unendlich-engine/src/world/entity.rs:182 pub fn contains_point
crates/unendlich-engine/src/world/entity.rs:490 pub fn despawn_entity
crates/unendlich-engine/src/world/entity.rs:600 pub fn resolve_intersection
crates/unendlich-engine/src/world/entity.rs:689 pub fn entity_resolve_intersection
crates/unendlich-engine/src/world/entity.rs:738 pub fn at_least
crates/unendlich-engine/src/world/entity.rs:741 pub fn at_most
crates/unendlich-engine/src/world/entity.rs:834 pub fn evaluate_spawn_rules
crates/unendlich-engine/src/world/entity.rs:936 pub fn default_color_for_kind
crates/unendlich-engine/src/world/entity.rs:1282 pub fn blueprint_to_vertices
crates/unendlich-engine/src/world/area_mining.rs:81 pub fn area_break_cells
crates/unendlich-engine/src/world/area_mining.rs:107 pub fn cube_area_break
crates/unendlich-engine/src/world/chunk.rs:514 pub fn recompute_cached_all_air
crates/unendlich-engine/src/world/chunk.rs:521 pub fn is_explicit
crates/unendlich-engine/src/world/chunk.rs:693 pub fn explicit_count
crates/unendlich-engine/src/world/chunk.rs:741 pub fn age_block
crates/unendlich-engine/src/world/chunk.rs:774 pub fn is_transitioning
crates/unendlich-engine/src/world/chunk.rs:780 pub fn is_all_air
crates/unendlich-engine/src/world/adaptive_stream.rs:67 pub fn for_hz
crates/unendlich-engine/src/world/adaptive_stream.rs:75 pub fn streaming_us
crates/unendlich-engine/src/world/adaptive_stream.rs:160 pub fn drain_to_budget
crates/unendlich-engine/src/world/blocks.rs:210 pub fn is_replaceable
crates/unendlich-engine/src/world/blocks.rs:242 pub fn bounce
crates/unendlich-engine/src/world/blocks.rs:266 pub fn is_visible
crates/unendlich-engine/src/world/blocks.rs:348 pub fn is_foliage_colored
crates/unendlich-engine/src/world/blocks.rs:458 pub fn hardness
crates/unendlich-engine/src/world/stage_tint.rs:52 pub fn transition_tint
crates/unendlich-engine/src/world/stage_tint.rs:70 pub fn stage_tint
crates/unendlich-engine/src/world/pathing.rs:137 pub fn dims
crates/unendlich-engine/src/world/subvoxel.rs:172 pub fn grass_tuft
crates/unendlich-engine/src/world/cave_flora.rs:49 pub fn from_stem
crates/unendlich-engine/src/world/collapse.rs:39 pub fn support_distance
crates/unendlich-engine/src/world/collapse.rs:63 pub fn floating
crates/unendlich-engine/src/world/collapse.rs:72 pub fn overhang
crates/unendlich-engine/src/world/surface_run.rs:167 pub fn compression_ratio
crates/unendlich-engine/src/world/layered_rle.rs:39 pub fn from_dense
crates/unendlich-engine/src/world/layered_rle.rs:99 pub fn run_count
crates/unendlich-engine/src/world/layered_rle.rs:115 pub fn composite_get
crates/unendlich-engine/src/world/chunk_flora.rs:37 pub fn populate
crates/unendlich-engine/src/world/chunk_flora.rs:79 pub fn pearl_count
crates/unendlich-engine/src/world/mesh_brick.rs:86 pub fn mark_brick
crates/unendlich-engine/src/world/mesh_brick.rs:91 pub fn mark_all
crates/unendlich-engine/src/world/mesh_brick.rs:100 pub fn clear_all
crates/unendlich-engine/src/world/upload_repr.rs:217 pub fn all_octree_baseline_bytes
crates/unendlich-engine/src/world/upload_repr.rs:227 pub fn project_residency
crates/unendlich-engine/src/world/ecosystem.rs:152 pub fn spawns_around
crates/unendlich-engine/src/world/biomes.rs:54 pub fn erosion_factor
crates/unendlich-engine/src/world/biomes.rs:78 pub fn cave_threshold
crates/unendlich-engine/src/world/biomes.rs:281 pub fn from_climate
crates/unendlich-engine/src/world/fields.rs:56 pub fn filled
crates/unendlich-engine/src/world/fields.rs:255 pub fn invalidate
crates/unendlich-engine/src/world/octree_compact.rs:84 pub fn compact
crates/unendlich-engine/src/world/elemental.rs:66 pub fn raise_wall
crates/unendlich-engine/src/world/elemental.rs:96 pub fn raise_pillar
crates/unendlich-engine/src/world/elemental.rs:116 pub fn smash_crater
crates/unendlich-engine/src/world/elemental.rs:137 pub fn fill_sphere
crates/unendlich-engine/src/world/elemental.rs:210 pub fn run_verb
crates/unendlich-engine/src/world/elemental.rs:247 pub fn apply_edits
crates/unendlich-engine/src/world/rebuild_coalescer.rs:36 pub fn started_at
crates/unendlich-engine/src/world/rebuild_coalescer.rs:47 pub fn note_edit
crates/unendlich-engine/src/world/rebuild_coalescer.rs:61 pub fn after_rebuild
crates/unendlich-engine/src/world/device_tier.rs:121 pub fn budgets_for_caps
crates/unendlich-engine/src/world/stem_gpu.rs:84 pub fn grow_step_parallel
crates/unendlich-engine/src/world/stem_gpu.rs:186 pub fn bench_growth
crates/unendlich-engine/src/world/octree_budget.rs:21 pub fn target_octree_bytes
crates/unendlich-engine/src/world/octree_raycast.rs:192 pub fn raycast_world
crates/unendlich-engine/src/world/shader_contract.rs:103 pub fn check_world_shaders
crates/unendlich-engine/src/world/mesh_epoch.rs:38 pub fn bumped
crates/unendlich-engine/src/world/brick_mesher.rs:50 pub fn mesh_brick
crates/unendlich-engine/src/world/flora_sites.rs:68 pub fn find_sites
crates/unendlich-engine/src/world/plant_genetics.rs:194 pub fn growth_response
crates/unendlich-engine/src/world/material_response.rs:181 pub fn bump_amplitude
crates/unendlich-engine/src/world/parkour.rs:129 pub fn count_walls
crates/unendlich-engine/src/world/parkour.rs:161 pub fn try_leap
crates/unendlich-engine/src/world/gpu_chunk_pool_vk.rs:131 pub fn slot_stride_bytes
crates/unendlich-engine/src/world/gpu_chunk_pool_vk.rs:144 pub fn slot_offset_bytes
crates/unendlich-engine/src/world/cpu_generator.rs:127 pub fn set_biome_registry
crates/unendlich-engine/src/world/cpu_generator.rs:1007 pub fn surface_run_preview
crates/unendlich-engine/src/world/scheduler.rs:165 pub fn tick_dt_for_band
crates/unendlich-engine/src/sim/command_parse.rs:224 pub fn opens_gui
crates/unendlich-engine/src/sim/command_parse.rs:232 pub fn to_command_string
crates/unendlich-engine/src/sim/grove_decay.rs:61 pub fn alive
crates/unendlich-engine/src/sim/skills.rs:79 pub fn xp
crates/unendlich-engine/src/sim/skills.rs:87 pub fn total_levels
crates/unendlich-engine/src/sim/skills.rs:101 pub fn can_invest
crates/unendlich-engine/src/sim/skills.rs:107 pub fn invest
crates/unendlich-engine/src/sim/skills.rs:132 pub fn scaled
crates/unendlich-engine/src/sim/skills.rs:138 pub fn respec
crates/unendlich-engine/src/sim/bending_thermal_phase.rs:60 pub fn heat_to_phase
crates/unendlich-engine/src/sim/bending_thermal_phase.rs:99 pub fn technique_cost01
crates/unendlich-engine/src/sim/env_frame.rs:62 pub fn frame_env
crates/unendlich-engine/src/sim/sync_check.rs:74 pub fn confirmed
crates/unendlich-engine/src/sim/sync_check.rs:108 pub fn reconcile
crates/unendlich-engine/src/sim/consensus.rs:53 pub fn two_of_any
crates/unendlich-engine/src/sim/consensus.rs:58 pub fn majority_of
crates/unendlich-engine/src/sim/consensus.rs:76 pub fn confirmed_digest
crates/unendlich-engine/src/sim/consensus.rs:137 pub fn bench_tally
crates/unendlich-engine/src/sim/shader_from_image.rs:399 pub fn synth_pattern_glsl
crates/unendlich-engine/src/sim/popgen.rs:103 pub fn analyze
crates/unendlich-engine/src/sim/ai_budget.rs:66 pub fn dt_scale
crates/unendlich-engine/src/sim/physics.rs:466 pub fn with_friction
crates/unendlich-engine/src/sim/driver.rs:119 pub fn with_max_catchup
crates/unendlich-engine/src/sim/driver.rs:171 pub fn render_pose
crates/unendlich-engine/src/sim/driver.rs:179 pub fn latest_pose
crates/unendlich-engine/src/sim/driver.rs:209 pub fn mob_count
crates/unendlich-engine/src/sim/driver.rs:216 pub fn mob_pose
crates/unendlich-engine/src/sim/driver.rs:227 pub fn local
crates/unendlich-engine/src/sim/fluid.rs:182 pub fn fluid_drag
crates/unendlich-engine/src/sim/fluid.rs:192 pub fn water_color
crates/unendlich-engine/src/sim/fluid.rs:260 pub fn flow_field
crates/unendlich-engine/src/sim/fluid.rs:283 pub fn surface_normal
crates/unendlich-engine/src/sim/perception_field.rs:86 pub fn alerted
crates/unendlich-engine/src/sim/mob_forage_memory.rs:89 pub fn predator
crates/unendlich-engine/src/sim/mob_forage_memory.rs:109 pub fn forget_secs
crates/unendlich-engine/src/sim/recipe_book.rs:50 pub fn recipe_count
crates/unendlich-engine/src/sim/recipe_book.rs:55 pub fn ingredient_count
crates/unendlich-engine/src/sim/recipe_book.rs:91 pub fn find_by_any
crates/unendlich-engine/src/sim/diffusion.rs:170 pub fn run_many
crates/unendlich-engine/src/sim/surface_cache.rs:36 pub fn get_or
crates/unendlich-engine/src/sim/combustion_spread.rs:62 pub fn burn_duration
crates/unendlich-engine/src/sim/combustion_spread.rs:121 pub fn tick_burn
crates/unendlich-engine/src/sim/egg_genome.rs:22 pub fn to_egg
crates/unendlich-engine/src/sim/batch_body_arc.rs:56 pub fn arc_batch_into
crates/unendlich-engine/src/sim/inventory_grid.rs:120 pub fn count_of
crates/unendlich-engine/src/sim/inventory_grid.rs:183 pub fn move_to
crates/unendlich-engine/src/sim/inventory_grid.rs:213 pub fn stack_onto
crates/unendlich-engine/src/sim/salvage_yield.rs:80 pub fn salvage
crates/unendlich-engine/src/sim/plant_growth_env.rs:114 pub fn should_wilt
crates/unendlich-engine/src/sim/plant_growth_env.rs:125 pub fn effective_rate
crates/unendlich-engine/src/sim/rest_cycle.rs:72 pub fn rhythm_for_mob
crates/unendlich-engine/src/sim/survival_exposure.rs:132 pub fn to_env_sample
crates/unendlich-engine/src/sim/desync_monitor.rs:64 pub fn interval
crates/unendlich-engine/src/sim/desync_monitor.rs:74 pub fn wants_deep_audit
crates/unendlich-engine/src/sim/desync_monitor.rs:115 pub fn event_line
crates/unendlich-engine/src/sim/desync_monitor.rs:138 pub fn total_ms
crates/unendlich-engine/src/sim/heat_distort.rs:147 pub fn bench_distort
crates/unendlich-engine/src/sim/plant_propagation.rs:102 pub fn germination_chance
crates/unendlich-engine/src/sim/locomotion_sim.rs:210 pub fn min_separation
crates/unendlich-engine/src/sim/locomotion_sim.rs:223 pub fn trace_json
crates/unendlich-engine/src/sim/aabb_resolve.rs:68 pub fn mtv
crates/unendlich-engine/src/sim/affliction.rs:146 pub fn has
crates/unendlich-engine/src/sim/affliction.rs:150 pub fn is_clear
crates/unendlich-engine/src/sim/creature_grime.rs:64 pub fn grime_factor01
crates/unendlich-engine/src/sim/ecology_population.rs:218 pub fn spawn_pressure
crates/unendlich-engine/src/sim/sound.rs:130 pub fn footstep_loudness_on_stanced
crates/unendlich-engine/src/sim/sound.rs:296 pub fn can_hear
crates/unendlich-engine/src/sim/predation.rs:29 pub fn from_capabilities
crates/unendlich-engine/src/sim/predation.rs:71 pub fn hunt
crates/unendlich-engine/src/sim/inventory_sort.rs:59 pub fn sort_order
crates/unendlich-engine/src/sim/camera_tremble.rs:52 pub fn pos_magnitude
crates/unendlich-engine/src/sim/dragon.rs:245 pub fn make_recurrent
crates/unendlich-engine/src/sim/dragon.rs:262 pub fn head_scan
crates/unendlich-engine/src/sim/dragon.rs:356 pub fn breath_scale
crates/unendlich-engine/src/sim/dragon.rs:376 pub fn take_damage
crates/unendlich-engine/src/sim/dragon.rs:384 pub fn is_dead
crates/unendlich-engine/src/sim/liveliness.rs:32 pub fn limpness
crates/unendlich-engine/src/sim/liveliness.rs:80 pub fn breath_for
crates/unendlich-engine/src/sim/navfield_gpu.rs:160 pub fn bench_navfield
crates/unendlich-engine/src/sim/navfield_gpu.rs:260 pub fn bench_flow
crates/unendlich-engine/src/sim/smoke_gpu.rs:190 pub fn bench_smoke_step
crates/unendlich-engine/src/sim/world_authority.rs:134 pub fn of
crates/unendlich-engine/src/sim/spawn.rs:96 pub fn spawn_around
crates/unendlich-engine/src/sim/projectile.rs:42 pub fn dust_clod
crates/unendlich-engine/src/sim/projectile.rs:55 pub fn impact
crates/unendlich-engine/src/sim/essence_refine.rs:117 pub fn refine
crates/unendlich-engine/src/sim/dragon_elemental.rs:33 pub fn choose_element
crates/unendlich-engine/src/sim/fauna_gait.rs:48 pub fn proportions
crates/unendlich-engine/src/sim/fauna_gait.rs:119 pub fn gait_for
crates/unendlich-engine/src/sim/attach_frame.rs:53 pub fn from_yaw_pitch
crates/unendlich-engine/src/sim/ambient.rs:70 pub fn heard_loudness_masked
crates/unendlich-engine/src/sim/impact_response.rs:49 pub fn respond
crates/unendlich-engine/src/sim/mob.rs:331 pub fn brain
crates/unendlich-engine/src/sim/net_rendezvous.rs:65 pub fn candidate_priority
crates/unendlich-engine/src/sim/net_rendezvous.rs:279 pub fn on_remote_blob
crates/unendlich-engine/src/sim/net_rendezvous.rs:314 pub fn on_packet
crates/unendlich-engine/src/sim/net_rendezvous.rs:333 pub fn poll
crates/unendlich-engine/src/sim/emission.rs:139 pub fn total_heat
crates/unendlich-engine/src/sim/emission.rs:143 pub fn touches
crates/unendlich-engine/src/sim/emission.rs:257 pub fn breath_cone
crates/unendlich-engine/src/sim/emission.rs:330 pub fn sky_call
crates/unendlich-engine/src/sim/emission.rs:342 pub fn trail
crates/unendlich-engine/src/sim/forage_sim.rs:60 pub fn grass_count
crates/unendlich-engine/src/sim/grounding.rs:53 pub fn slip
crates/unendlich-engine/src/sim/quad_reposition.rs:44 pub fn settle_stance
crates/unendlich-engine/src/sim/dragon_locomotion.rs:124 pub fn sprint_step
crates/unendlich-engine/src/sim/fauna_mind.rs:176 pub fn recalled_threat
crates/unendlich-engine/src/sim/hotbar.rs:35 pub fn verb
crates/unendlich-engine/src/sim/hotbar.rs:81 pub fn with_items
crates/unendlich-engine/src/sim/hotbar.rs:95 pub fn active_index
crates/unendlich-engine/src/sim/hotbar.rs:118 pub fn active_ability
crates/unendlich-engine/src/sim/hotbar.rs:134 pub fn assign_from
crates/unendlich-engine/src/sim/hotbar.rs:146 pub fn consume_active_one
crates/unendlich-engine/src/sim/ingame_menu.rs:42 pub fn overlay
crates/unendlich-engine/src/sim/ingame_menu.rs:49 pub fn is_paused
crates/unendlich-engine/src/sim/ingame_menu.rs:77 pub fn event
crates/unendlich-engine/src/sim/dragon_growth_stage.rs:136 pub fn dragon_growth_stage
crates/unendlich-engine/src/sim/session_mode.rs:164 pub fn on_failed
crates/unendlich-engine/src/sim/session_mode.rs:170 pub fn back_to_browse
crates/unendlich-engine/src/sim/session_mode.rs:208 pub fn go_live
crates/unendlich-engine/src/sim/session_mode.rs:217 pub fn make_advert
crates/unendlich-engine/src/sim/inventory_txn.rs:51 pub fn moved_any
crates/unendlich-engine/src/sim/inventory_txn.rs:80 pub fn quick_move
crates/unendlich-engine/src/sim/inventory_txn.rs:138 pub fn undo_quick_move
crates/unendlich-engine/src/sim/inventory_txn.rs:181 pub fn split_even
crates/unendlich-engine/src/sim/inventory_txn.rs:250 pub fn consolidate
crates/unendlich-engine/src/sim/inventory_txn.rs:277 pub fn equip_swap
crates/unendlich-engine/src/sim/world_launch.rs:97 pub fn needs_generation
crates/unendlich-engine/src/sim/legendary_weapon.rs:155 pub fn forge
crates/unendlich-engine/src/sim/dragon_bending.rs:117 pub fn channel_step
crates/unendlich-engine/src/sim/item_entity.rs:136 pub fn new_blade
crates/unendlich-engine/src/sim/item_entity.rs:150 pub fn new_blade_with_genes
crates/unendlich-engine/src/sim/item_entity.rs:162 pub fn launch
crates/unendlich-engine/src/sim/item_entity.rs:175 pub fn attend
crates/unendlich-engine/src/sim/landscape_memory.rs:48 pub fn observe_line
crates/unendlich-engine/src/sim/landscape_memory.rs:74 pub fn heading_familiarity
crates/unendlich-engine/src/sim/sound_masking.rs:37 pub fn can_hear
crates/unendlich-engine/src/sim/sound_masking.rs:45 pub fn effective_range
crates/unendlich-engine/src/sim/path_memory.rs:73 pub fn clearance
crates/unendlich-engine/src/sim/path_memory.rs:92 pub fn penalize
crates/unendlich-engine/src/sim/indirect_batch.rs:51 pub fn batch
crates/unendlich-engine/src/sim/main_menu.rs:86 pub fn input
crates/unendlich-engine/src/sim/bending_targeting.rs:243 pub fn resolve_targets
crates/unendlich-engine/src/sim/sound_genome.rs:86 pub fn mimic
crates/unendlich-engine/src/sim/mob_limb_girth.rs:55 pub fn exceeds_girth
crates/unendlich-engine/src/sim/ore_resource_veins.rs:164 pub fn vein_density
crates/unendlich-engine/src/sim/exec_tier.rs:58 pub fn runs_on_cpu
crates/unendlich-engine/src/sim/exec_tier.rs:61 pub fn runs_on_gpu
crates/unendlich-engine/src/sim/exec_tier.rs:88 pub fn exec_tier
crates/unendlich-engine/src/sim/dragon_thirst.rs:61 pub fn drink
crates/unendlich-engine/src/sim/ao_gpu.rs:169 pub fn bench_ao
crates/unendlich-engine/src/sim/carry.rs:76 pub fn danger
crates/unendlich-engine/src/sim/carry.rs:91 pub fn regrowth_multiplier
crates/unendlich-engine/src/sim/dragon_brain.rs:124 pub fn think
crates/unendlich-engine/src/sim/foveation.rs:162 pub fn bench_integrate_foveated
crates/unendlich-engine/src/sim/craft_resolve.rs:111 pub fn resolve_craft
crates/unendlich-engine/src/sim/craft_resolve.rs:142 pub fn repair_estimate
crates/unendlich-engine/src/sim/menu_screen_layout.rs:72 pub fn layout_for_title
crates/unendlich-engine/src/sim/rollback.rs:99 pub fn set_local_input
crates/unendlich-engine/src/sim/input_prediction.rs:112 pub fn get_or_predict
crates/unendlich-engine/src/sim/dht_join.rs:310 pub fn type_address
crates/unendlich-engine/src/sim/dht_join.rs:320 pub fn begin
crates/unendlich-engine/src/sim/dht_join.rs:340 pub fn on_phase
crates/unendlich-engine/src/sim/dht_join.rs:391 pub fn peers
crates/unendlich-engine/src/sim/mp_dht.rs:108 pub fn put
crates/unendlich-engine/src/sim/mp_dht.rs:118 pub fn fetch
crates/unendlich-engine/src/sim/mp_dht.rs:153 pub fn should_link
crates/unendlich-engine/src/sim/batch_settle.rs:44 pub fn settle_one
crates/unendlich-engine/src/sim/personality.rs:56 pub fn confronts
crates/unendlich-engine/src/sim/breath_attack.rs:73 pub fn try_breathe
crates/unendlich-engine/src/sim/fall.rs:47 pub fn flee_step_avoiding_cliffs
crates/unendlich-engine/src/sim/batch_perception.rs:58 pub fn perceive_one
crates/unendlich-engine/src/sim/batch_perception.rs:63 pub fn perceive_batch
crates/unendlich-engine/src/sim/title_cinematic.rs:247 pub fn dragon_genome
crates/unendlich-engine/src/sim/title_cinematic.rs:278 pub fn pose
crates/unendlich-engine/src/sim/title_cinematic.rs:285 pub fn as_path
crates/unendlich-engine/src/sim/title_cinematic.rs:336 pub fn vignette_and_framing
crates/unendlich-engine/src/sim/head_steer.rs:97 pub fn world_yaw
crates/unendlich-engine/src/sim/head_steer.rs:105 pub fn pull_body
crates/unendlich-engine/src/sim/pylon.rs:140 pub fn effects_from
crates/unendlich-engine/src/sim/crafting.rs:228 pub fn matching_index
crates/unendlich-engine/src/sim/menu_controller.rs:90 pub fn settings
crates/unendlich-engine/src/sim/menu_controller.rs:93 pub fn settings_mut
crates/unendlich-engine/src/sim/menu_controller.rs:96 pub fn browser
crates/unendlich-engine/src/sim/menu_controller.rs:99 pub fn browser_mut
crates/unendlich-engine/src/sim/menu_controller.rs:182 pub fn host
crates/unendlich-engine/src/sim/fixed.rs:82 pub fn to_f64
crates/unendlich-engine/src/sim/column_memo.rs:50 pub fn get_or
crates/unendlich-engine/src/sim/touch_rays.rs:74 pub fn feel_into
crates/unendlich-engine/src/sim/volumetric_gpu.rs:114 pub fn bench_integrate
crates/unendlich-engine/src/sim/bending_feedback.rs:116 pub fn combine
crates/unendlich-engine/src/sim/elemental.rs:181 pub fn record_deed
crates/unendlich-engine/src/sim/elemental.rs:231 pub fn evolve
crates/unendlich-engine/src/sim/perception_fog.rs:188 pub fn bench_perceive
crates/unendlich-engine/src/sim/articulation.rs:60 pub fn children
crates/unendlich-engine/src/sim/articulation.rs:86 pub fn total_curl
crates/unendlich-engine/src/sim/elements.rs:60 pub fn has_fire
crates/unendlich-engine/src/sim/elements.rs:63 pub fn has_ice
crates/unendlich-engine/src/sim/elements.rs:66 pub fn has_wind
crates/unendlich-engine/src/sim/item_brain.rs:139 pub fn socket
crates/unendlich-engine/src/sim/item_brain.rs:255 pub fn roll_material
crates/unendlich-engine/src/sim/player_views.rs:75 pub fn with_anim_secs
crates/unendlich-engine/src/sim/player_views.rs:80 pub fn player_count
crates/unendlich-engine/src/sim/player_views.rs:84 pub fn views
crates/unendlich-engine/src/sim/player_views.rs:87 pub fn ids
crates/unendlich-engine/src/sim/player_views.rs:93 pub fn viewport_of
crates/unendlich-engine/src/sim/player_views.rs:100 pub fn add_player
crates/unendlich-engine/src/sim/player_views.rs:120 pub fn remove_player
crates/unendlich-engine/src/sim/player_views.rs:133 pub fn set_aspect
crates/unendlich-engine/src/sim/player_views.rs:208 pub fn compute_overlap
crates/unendlich-engine/src/sim/settings_model.rs:116 pub fn draft
crates/unendlich-engine/src/sim/settings_model.rs:121 pub fn committed
crates/unendlich-engine/src/sim/settings_model.rs:136 pub fn revert
crates/unendlich-engine/src/sim/settings_model.rs:141 pub fn dirty
crates/unendlich-engine/src/sim/concealment.rs:43 pub fn detectability
crates/unendlich-engine/src/sim/survival.rs:40 pub fn comfortable
crates/unendlich-engine/src/sim/turtle.rs:34 pub fn next_state
crates/unendlich-engine/src/sim/turtle.rs:54 pub fn speed_mult
crates/unendlich-engine/src/sim/dragon_blueprint_from_genome.rs:95 pub fn dragon_blueprint_from_genome
crates/unendlich-engine/src/sim/lauscher.rs:102 pub fn consider_lair
crates/unendlich-engine/src/sim/lauscher.rs:115 pub fn hear
crates/unendlich-engine/src/sim/bending_information.rs:118 pub fn disrupt
crates/unendlich-engine/src/sim/bending_information.rs:130 pub fn mask
crates/unendlich-engine/src/sim/dragon_signature.rs:93 pub fn signature_from
crates/unendlich-engine/src/sim/menu_nav.rs:34 pub fn action
crates/unendlich-engine/src/sim/menu_nav.rs:37 pub fn submenu
crates/unendlich-engine/src/sim/menu_nav.rs:113 pub fn move_focus
crates/unendlich-engine/src/sim/menu_nav.rs:140 pub fn activate
crates/unendlich-engine/src/sim/collision_probe.rs:87 pub fn lean_sized
crates/unendlich-engine/src/sim/collision_probe.rs:107 pub fn walkable_ahead
crates/unendlich-engine/src/sim/collision_probe.rs:117 pub fn leap_target
crates/unendlich-engine/src/sim/collision_probe.rs:136 pub fn coarsened
crates/unendlich-engine/src/sim/armor.rs:34 pub fn solar
crates/unendlich-engine/src/sim/armor.rs:62 pub fn repair_in_sun
crates/unendlich-engine/src/sim/gpu_bench.rs:94 pub fn metric_line
crates/unendlich-engine/src/sim/gpu_bench.rs:150 pub fn visibility_cpu
crates/unendlich-engine/src/sim/gpu_bench.rs:167 pub fn bench_visibility
crates/unendlich-engine/src/sim/item_compare.rs:62 pub fn compare
crates/unendlich-engine/src/sim/dragon_mode.rs:28 pub fn air
crates/unendlich-engine/src/sim/lauscher_lair.rs:127 pub fn aged
crates/unendlich-engine/src/sim/lauscher_lair.rs:132 pub fn home
crates/unendlich-engine/src/sim/lauscher_lair.rs:136 pub fn climbs
crates/unendlich-engine/src/sim/lauscher_lair.rs:139 pub fn decorations
crates/unendlich-engine/src/sim/lauscher_lair.rs:142 pub fn stored_echoes
crates/unendlich-engine/src/sim/lauscher_lair.rs:148 pub fn record_echo
crates/unendlich-engine/src/sim/lauscher_lair.rs:240 pub fn block_edits
crates/unendlich-engine/src/sim/lauscher_lair.rs:264 pub fn unease
crates/unendlich-engine/src/sim/water_sight.rs:55 pub fn water_path
crates/unendlich-engine/src/sim/water_sight.rs:147 pub fn scatter_detection
crates/unendlich-engine/src/sim/water_sight.rs:186 pub fn perceive_through_water
crates/unendlich-engine/src/sim/forage_memory.rs:29 pub fn known_count
crates/unendlich-engine/src/sim/forage_memory.rs:58 pub fn forget_stale
crates/unendlich-engine/src/sim/dragon_forge.rs:1009 pub fn skin_with_flex
crates/unendlich-engine/src/sim/dragon_forge.rs:1379 pub fn place_surface_glow
crates/unendlich-engine/src/sim/menu_router.rs:113 pub fn stack
crates/unendlich-engine/src/sim/menu_router.rs:149 pub fn pop_forced
crates/unendlich-engine/src/sim/menu_router.rs:189 pub fn replace_forced
crates/unendlich-engine/src/sim/menu_router.rs:210 pub fn reset_to_root
crates/unendlich-engine/src/sim/stamina.rs:39 pub fn can_sprint
crates/unendlich-engine/src/sim/neighbor_grid.rs:55 pub fn for_candidates
crates/unendlich-engine/src/sim/neighbor_grid.rs:71 pub fn candidates
crates/unendlich-engine/src/sim/ragdoll.rs:33 pub fn with_velocity
crates/unendlich-engine/src/sim/lightning.rs:58 pub fn arc_path
crates/unendlich-engine/src/sim/lightning.rs:121 pub fn arc_tree
crates/unendlich-engine/src/sim/bending_runtime.rs:117 pub fn update_from_input
crates/unendlich-engine/src/sim/body_scale.rs:29 pub fn dragon
crates/unendlich-engine/src/sim/body_scale.rs:36 pub fn sight_range
crates/unendlich-engine/src/sim/body_scale.rs:41 pub fn probe_span
crates/unendlich-engine/src/sim/body_scale.rs:47 pub fn path_tile
crates/unendlich-engine/src/sim/net/session.rs:69 pub fn record_local
crates/unendlich-engine/src/sim/net/session.rs:76 pub fn outgoing_input_packet
crates/unendlich-engine/src/sim/net/session.rs:89 pub fn outgoing_sync_packet
crates/unendlich-engine/src/sim/net/session.rs:95 pub fn note_confirmed
crates/unendlich-engine/src/sim/net/session.rs:101 pub fn ingest
crates/unendlich-engine/src/sim/net/session.rs:139 pub fn remote_ack
crates/unendlich-engine/src/sim/net/session.rs:144 pub fn desync
crates/unendlich-engine/src/sim/net/transport.rs:77 pub fn loopback_pair
crates/unendlich-engine/src/sim/cognition/forager.rs:48 pub fn goal
crates/unendlich-engine/src/sim/cognition/forager.rs:57 pub fn eat
crates/unendlich-engine/src/sim/cognition/behavior.rs:96 pub fn gape_for_drive
crates/unendlich-engine/src/sim/cognition/navigate.rs:39 pub fn posture_for_clearance
crates/unendlich-engine/src/sim/cognition/navigate.rs:236 pub fn heading_toward_3d
crates/unendlich-engine/src/sim/cognition/sentinel_agent.rs:50 pub fn sense_threats
crates/unendlich-engine/src/sim/cognition/perception_batch.rs:108 pub fn lod
crates/unendlich-engine/src/sim/cognition/perceive_bridge.rs:61 pub fn mind_tick_from_perception
crates/unendlich-engine/src/sim/cognition/imitation.rs:43 pub fn credibility
crates/unendlich-engine/src/sim/cognition/imitation.rs:58 pub fn adopt_posture
crates/unendlich-engine/src/sim/cognition/mob_sim.rs:495 pub fn scare_latency
crates/unendlich-engine/src/sim/cognition/service.rs:67 pub fn with_cadence
crates/unendlich-engine/src/sim/cognition/mind.rs:56 pub fn with_memory
crates/unendlich-engine/src/render/cascade_fit_pipeline.rs:301 pub fn cascade_fit_cpu_reference
crates/unendlich-engine/src/render/mesher.rs:693 pub fn unpack_color
crates/unendlich-engine/src/render/mesher.rs:1076 pub fn mesh_chunk_far
crates/unendlich-engine/src/render/mesher.rs:1086 pub fn mesh_chunk_very_far
crates/unendlich-engine/src/render/postfx.rs:143 pub fn scene_target
crates/unendlich-engine/src/render/postfx.rs:151 pub fn scene_render_pass
crates/unendlich-engine/src/render/postfx.rs:159 pub fn present_render_pass
crates/unendlich-engine/src/render/postfx.rs:168 pub fn begin_present_copy
crates/unendlich-engine/src/render/postfx.rs:229 pub fn end_present
crates/unendlich-engine/src/render/meshing_pool.rs:448 pub fn queue_capacity
crates/unendlich-engine/src/render/meshing_pool.rs:451 pub fn pending_requests
crates/unendlich-engine/src/render/gpu_meshing_octree_cpu_ref.rs:196 pub fn mesh_chunk_octree_cpu_reference
crates/unendlich-engine/src/render/mesh_buffer_alloc.rs:129 pub fn allocation_count
crates/unendlich-engine/src/render/mesh_buffer_alloc.rs:130 pub fn free_span_count
crates/unendlich-engine/src/render/mesh_buffer_alloc.rs:221 pub fn check_invariants
crates/unendlich-engine/src/render/gpu_timer.rs:538 pub fn skipped_frames
crates/unendlich-engine/src/render/vulkan_state_model.rs:116 pub fn run_render_pass
crates/unendlich-engine/src/render/mesh_buffer_pool.rs:113 pub fn vert_free_bytes
crates/unendlich-engine/src/render/mesh_buffer_pool.rs:114 pub fn idx_free_bytes
crates/unendlich-engine/src/render/mesh_buffer_pool.rs:776 pub fn is_device_local
crates/unendlich-engine/src/render/view_params.rs:275 pub fn write_planes
crates/unendlich-engine/src/render/world_renderer.rs:2006 pub fn mesh_cache_hits
crates/unendlich-engine/src/render/world_renderer.rs:2012 pub fn mesh_cache_misses
crates/unendlich-engine/src/render/world_renderer.rs:2017 pub fn mesh_cache_size
crates/unendlich-engine/src/render/world_renderer.rs:2338 pub fn mesh_or_empty_version_for
crates/unendlich-engine/src/render/world_renderer.rs:2357 pub fn mesh_version_for_lod
crates/unendlich-engine/src/render/world_renderer.rs:2729 pub fn draw_frame_with_view
crates/unendlich-engine/src/render/world_renderer.rs:4838 pub fn mesh_vert_largest_free
crates/unendlich-engine/src/render/world_renderer.rs:4839 pub fn mesh_idx_largest_free
crates/unendlich-engine/src/render/adaptive_upload.rs:99 pub fn with_thresholds
crates/unendlich-engine/src/render/adaptive_upload.rs:107 pub fn with_cap_range
crates/unendlich-engine/src/render/adaptive_upload.rs:118 pub fn with_smoothing
crates/unendlich-engine/src/items/inventory.rs:185 pub fn move_into
crates/unendlich-engine/src/items/debug_ui.rs:47 pub fn debug_inventory_ui
crates/unendlich-engine/src/items/debug_ui.rs:125 pub fn debug_inventory_window
crates/unendlich-engine/src/items/debug_hud.rs:41 pub fn build_hotbar_markers
crates/unendlich-engine/src/items/debug_hud.rs:96 pub fn hotbar_summary
crates/unendlich-engine/src/items/crafting.rs:155 pub fn craft_shaped
crates/unendlich-engine/src/items/crafting.rs:201 pub fn shaped
crates/unendlich-engine/src/items/crafting.rs:205 pub fn first_craftable
crates/unendlich-engine/src/items/crafting.rs:209 pub fn craftable
crates/unendlich-engine/src/items/crafting.rs:215 pub fn match_shaped

## A2 zero-callsite clusters (by family)  (7)
sim: 290 fns
world: 146 fns
(root): 132 fns
render: 32 fns
items: 10 fns
telemetry: 8 fns
app: 6 fns

## B discarded values (bin)  (22)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:399 let _render_probe = unendlich_engine::thread_probe::register("render");
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2174 let _ = std::fs::write(
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2229 let _ = fg_app.tick(
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2244 let _ = fg_app.tick(unendlich_engine::app::AppInput::Resume, 0);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2256 let _ = fg_app.tick(unendlich_engine::app::AppInput::QuitToTitle, 0);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2264 let _ = fg_app.tick(unendlich_engine::app::AppInput::Pause, 0);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2268 let _ = fg_app.tick(unendlich_engine::app::AppInput::Resume, 0);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2845 let _ = std::hint::black_box(
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2852 let _ = std::hint::black_box(
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2859 let _ = std::hint::black_box(
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2869 let _ = std::hint::black_box(
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2881 let _ = std::hint::black_box(
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2892 let _ = std::hint::black_box(
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2946 let _stam_mult = fg_app.stamina_mut().tick(sprint_urge, dt);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:6088 let _ = player_chunk_changed;
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:6480 let _ = renderer.upload_chunk_octree_versioned(
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:6501 let _ = submitted; // available for future debug logging
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:6800 let _ = uploaded_this_frame;
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:7521 let _ = fg_app.tick(AppInput::WizardSetName("New World".into()), 0);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:7528 let _ = fg_app.tick(AppInput::WizardSetSeed(SeedChoice::Fixed(pv)), 0);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:8527 let _ = TickId::ZERO; // silence unused if branches drop the use
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:8553 let _ = sim_t; // sim_t was used by old non-heat-aware path

## C TODO/stub markers  (50)
crates/unendlich-engine/src/water_field.rs:47 ...on). Skeleton //!   only with TODO comments. Coming in a follow-up session. //! ...
crates/unendlich-engine/src/water_field.rs:199 ...tal);     }      /// The tick stub is a no-op today: total volume must be unchan...
crates/unendlich-engine/src/water_field.rs:202 .../// For now it verifies the stub doesn't accidentally lose mass.     #[test]    ...
crates/unendlich-engine/src/water_field.rs:212 ...s() < 1e-6,             "tick stub leaked volume: before={}, after={}", before, ...
crates/unendlich-engine/src/creature_design.rs:801 ...tails vary a lot in length: a stub of 4 up to a long 10-segment         // whip ...
crates/unendlich-engine/src/creature.rs:888 ...- 1e-4);     }      /// Tiny stub world for collision tests: a single solid bloc...
crates/unendlich-engine/src/app/menu_view.rs:262 ...lands on a deterministic /// placeholder `Fixed(0)` the seed-entry step then let...
crates/unendlich-engine/src/world/plant_lifecycle.rs:215 ...AL_TICKS` ticks (the previous placeholder). /// /// The dirty flag is set by `Ch...
crates/unendlich-engine/src/world/plant_lifecycle.rs:228 ...tructural-dirty: replaces the placeholder     // POI_REFRESH_INTERVAL_TICKS cade...
crates/unendlich-engine/src/world/plant_lifecycle.rs:752 ...// ticks (the previous placeholder cadence).             //             // struc...
crates/unendlich-engine/src/world/octree_scheduler.rs:60 ...land — for v0.24.23 we have a placeholder /// catalog covering the obvious cases...
crates/unendlich-engine/src/world/chunk_manager.rs:50 ...ne of the conversion helpers (TODO: add when /// the first caller migrates). #[d...
crates/unendlich-engine/src/world/gpu_chunk_pool.rs:32 ...n-backed pool or a CPU-shadow stub. The CPU stub exists to //! keep the unit tes...
crates/unendlich-engine/src/world/gpu_chunk_pool.rs:32 ...or a CPU-shadow stub. The CPU stub exists to //! keep the unit tests independent...
crates/unendlich-engine/src/world/weathering.rs:15 ...vironment-driven equilibrium (TODO, //!     phase 6+ when caves and water flow a...
crates/unendlich-engine/src/world/world_model.rs:372 ...lightly     /// higher seas — placeholder for a real water-cycle later).     pub...
crates/unendlich-engine/src/world/entity.rs:867 ...and /// condition values are placeholders pending playtest tuning — /// they're ...
crates/unendlich-engine/src/world/chunk.rs:147 .../// The 10-tick cadence is a placeholder until structural dirty     /// marking ...
crates/unendlich-engine/src/world/chunk.rs:191 .../// **The end-state of the placeholder cadence in     /// `POI_REFRESH_INTERVAL_...
crates/unendlich-engine/src/world/blocks.rs:120 ...llision()`, `blocks_light()` (TODO),     /// `produces_face_against(neighbor)` (...
crates/unendlich-engine/src/world/blocks.rs:121 ...uces_face_against(neighbor)` (TODO), `is_meshable_volume()`     /// (TODO), etc....
crates/unendlich-engine/src/world/blocks.rs:122 ...s_meshable_volume()`     /// (TODO), etc. Old call sites still using `is_solid` ...
crates/unendlich-engine/src/world/blocks.rs:123 .../// debt — flag with `// TODO migrate to narrower predicate` when     /// touchi...
crates/unendlich-engine/src/world/biomes.rs:120 ...// TODO: add MapleLog/MapleLeaves blocks.             }),...
crates/unendlich-engine/src/world/fields.rs:290 ...//!     //! Today this is a stub returning a constant scalar (will become a 3D  ...
crates/unendlich-engine/src/world/leaf_decay.rs:78 ...oesn't yet — wind is a static stub — but will once wind is /// directional and t...
crates/unendlich-engine/src/world/leaf_decay.rs:172 ...// scalar 0.3-everywhere stub; a future directional wind                 // fiel...
crates/unendlich-engine/src/world/cpu_generator.rs:588 ...d         // hook in the bin (TODO when implemented): each Lava block         //...
crates/unendlich-engine/src/sim/ai_budget.rs:20 ...he viewer. Thresholds are /// placeholders (tune against the mob cap and render ...
crates/unendlich-engine/src/sim/physics.rs:457 ...ove. Reproduces brick-1's /// placeholder ground plane (rest height `y = 0`) and...
crates/unendlich-engine/src/sim/inventory_grid.rs:9 ...in isolation (R1): it owns a placeholder [`max_stack`] //! table rather than rea...
crates/unendlich-engine/src/sim/forage_harvest.rs:18 ...ks::BlockId`) as a documented placeholder until dedicated forage //! items exist...
crates/unendlich-engine/src/sim/forage_harvest.rs:50 ...lockId` discriminant     /// (placeholder mapping to the nearest existing palett...
crates/unendlich-engine/src/sim/forage_harvest.rs:54 ...ower,    // 25 — edible-plant placeholder             Forageable::Mushroom => Bl...
crates/unendlich-engine/src/sim/forage_harvest.rs:60 ...BlockId::OakLeaves,   // 18 — placeholder         };         b as u16     }     ...
crates/unendlich-engine/src/sim/emission.rs:159 ...three channels. These /// are placeholder ratios (the design doc treats numbers ...
crates/unendlich-engine/src/sim/landscape_memory.rs:8 ...value`] //! instead of a flat placeholder. //! //! Pure + deterministic. Bounded...
crates/unendlich-engine/src/render/chunk_renderer.rs:198 ...Both samplers         // are placeholders for this bin (no shadow pre-pass, no f...
crates/unendlich-engine/src/render/chunk_renderer.rs:286 ...1 (shadow             // map placeholder), binding 2 (froxel volume placeholder)...
crates/unendlich-engine/src/render/chunk_renderer.rs:286 ...er), binding 2 (froxel volume placeholder).             // All stable for the re...
crates/unendlich-engine/src/render/gpu_timer.rs:549 .../// session this returns a placeholder.     pub fn flush_lifetime_summary(&self)...
crates/unendlich-engine/src/render/gpu_timer.rs:596 .../// `flush_log` returns a placeholder and other methods are no-ops.     pub fn s...
crates/unendlich-engine/src/render/gpu_meshing_octree.rs:516 .../ to guarantee freshness. The TODO is filed.                 match pool.upload_i...
crates/unendlich-engine/src/render/gpu_meshing_octree.rs:825 ...e normal case) start with the placeholder         // `Err(NOT_READY)` which mean...
crates/unendlich-engine/src/render/world_renderer.rs:2228 ...mes, M skipped)"     /// Or a placeholder if the device doesn't support timestam...
crates/unendlich-engine/src/render/world_renderer.rs:3097 ...// placeholder art until the shader/neural pass....
crates/unendlich-engine/src/items/loot.rs:12 ...the //! `has_tool` flag is a placeholder until a real tool/tier system exists.  ...
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:1096 ...// controller's local regen stub — build while still, spend per cast.     let mu...
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2568 ...// gesture: HOLD F7 (placeholder until the actions/ritual menu).             let...
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:3462 ...tooltips: format FINAL, lore placeholder for world voice).                 let t...

## D doc promises  (60)
crates/unendlich-engine/src/render_parity.rs:26 //! once the chunk stream has settled (e.g. N frames after the last upload).
crates/unendlich-engine/src/vertex_pack.rs:11 //!   normals (bumpy tops, future normal-mapping), not just the 6 face axes.
crates/unendlich-engine/src/water_field.rs:21 //! time (any BlockId::Water cell gets field value 1.0). Eventually:
crates/unendlich-engine/src/water_field.rs:29 //! blocks but unused by the mesher. Future sessions wire it in.
crates/unendlich-engine/src/water_field.rs:112 /// Future logic (in priority order):
crates/unendlich-engine/src/water_field.rs:123 ///    faster than dry stone. Eventually stone → gravel → sand.
crates/unendlich-engine/src/physics.rs:26 //!   field machinery to vary it spatially is future work).
crates/unendlich-engine/src/physics.rs:174 /// `ChunkManager` (today) and any future world-state source (server snapshot,
crates/unendlich-engine/src/temperature.rs:43 //! NOT yet: heat sources (fires, lava), absorption (sun warming
crates/unendlich-engine/src/temperature.rs:47 //! ## Future
crates/unendlich-engine/src/temperature.rs:209 /// future project (probably tied to a sky-exposure cache in
crates/unendlich-engine/src/input_capture.rs:102 /// Layout is intentionally compact: this struct will be sent over the
crates/unendlich-engine/src/raycast.rs:17 //! but tagging lets future systems budget rays per kind, prioritize between
crates/unendlich-engine/src/sky.rs:6 //!   - The renderer (eventually) uses ambient color for the clear color and
crates/unendlich-engine/src/sky.rs:53 /// for future regression tests and as documentation of the convention.
crates/unendlich-engine/src/thermal_mass.rs:29 //! eventually (in the water_field module's tick handler) but
crates/unendlich-engine/src/snapshot.rs:55 //! ## Future shape
crates/unendlich-engine/src/depth_fade.rs:12 /// ramping to `1` once the gap reaches `fade`. `fade` guarded `> 0`.
crates/unendlich-engine/src/debug_settings.rs:213 /// Fragment-shader debug visualisation modes — reserved for a future
crates/unendlich-engine/src/cloud.rs:13 //! Future tie-ins (the gameplay loop):
crates/unendlich-engine/src/cloud.rs:34 //! Future steps will couple this with the wind-advected moisture
crates/unendlich-engine/src/cloud.rs:63 /// Future: add `time: f32` parameter; offset XZ sampling by
crates/unendlich-engine/src/creature.rs:24 //! ## Limitations / future
crates/unendlich-engine/src/creature.rs:28 //!   need an articulation/skeleton system (future).
crates/unendlich-engine/src/light.rs:98 /// obvious home for a future sky-light channel — and packs as 0. Channels above
crates/unendlich-engine/src/wind.rs:24 //! Future:
crates/unendlich-engine/src/wind.rs:136 /// individual ticks. Future: add diurnal modulation (warmer days =
crates/unendlich-engine/src/world/ticks.rs:19 //! sequentially in registration order. Future work is to parallelize
crates/unendlich-engine/src/world/ticks.rs:343 /// will run on background threads in the future).
crates/unendlich-engine/src/world/stem.rs:13 //! - **The future rope graph.** Every node already carries a cell + parent link,
crates/unendlich-engine/src/world/plant_lifecycle.rs:21 /// Bits in `flags` reserved for future use (decaying, withered-from-fire,
crates/unendlich-engine/src/world/plant_lifecycle.rs:486 /// wet plants won't dry, dry plants will dry. Future improvement could
crates/unendlich-engine/src/world/plant_lifecycle.rs:631 /// **Tick culling** (skip far-away chunks) is a future addition — would
crates/unendlich-engine/src/world/chunk_priority.rs:6 //! (edited, awaiting re-mesh/upload) are boosted so visible edits land quickly.
crates/unendlich-engine/src/world/plant_blueprint.rs:280 /// blueprint walker. Future: terminators (leaves at the very tip)
crates/unendlich-engine/src/world/plant_blueprint.rs:377 /// blocks), see future work in cpu_generator.
crates/unendlich-engine/src/world/weathering.rs:44 //! ## Future
crates/unendlich-engine/src/world/weathering.rs:66 /// Future: read humidity / temperature / sky-exposure fields to
crates/unendlich-engine/src/world/weathering.rs:97 /// This is a worldgen approximation. Future: a real cave-weathering
crates/unendlich-engine/src/world/weathering.rs:115 /// time progresses (future tick handler).
crates/unendlich-engine/src/world/pressure_sampler.rs:37 /// other dimensions are reserved for future pressure-driven physics
crates/unendlich-engine/src/world/lod.rs:4 //! that future increments (F2-F6) build on top of. It does NOT
crates/unendlich-engine/src/world/lod.rs:194 /// trivially tunable. Future increment: drive these from
crates/unendlich-engine/src/world/lod.rs:410 /// behaviour matches v0.21.x exactly. Future increments will add
crates/unendlich-engine/src/world/lod.rs:443 //   bits 16..31 : reserved for future use
crates/unendlich-engine/src/world/lod.rs:484 // Far=4, VeryFar=8). Users will tune via a future settings UI; for now
crates/unendlich-engine/src/world/lod.rs:599 // FUTURE NOTE — tick rate as second consumer of distance falloff
crates/unendlich-engine/src/world/chunk_loader.rs:58 /// future GPU mesher, physics, raycast, item region reads) all see a
crates/unendlich-engine/src/world/recompaction.rs:42 /// policy is ready to fire; it disarms after firing and re-arms only once the
crates/unendlich-engine/src/world/surface.rs:16 //! survive the future octree migration. Octree leaves carry world
crates/unendlich-engine/src/world/surface.rs:49 ///   bits 14..32: reserved (0 in v1; future: per-face material flags,
crates/unendlich-engine/src/world/surface.rs:95 /// Future octree implementation will replace this with a tree walk;
crates/unendlich-engine/src/world/biome_loader.rs:6 //! at startup. Future-proofs us toward AI/storytelling biome generation
crates/unendlich-engine/src/world/biome_loader.rs:34 /// future subsystems.
crates/unendlich-engine/src/world/biome_loader.rs:228 /// allowing JSON edits to take effect at runtime — is a future addition;
crates/unendlich-engine/src/world/world_model.rs:195 /// sit underneath the noise. Future evolution: each becomes a field of its
crates/unendlich-engine/src/world/texture_mapping.rs:49 /// checked and any future port (e.g. into the mesher) starts from a proven spec.
crates/unendlich-engine/src/world/gpu_mesher_vk.rs:14 //!    reads vertex+index data directly (this initial form) or — eventually —
crates/unendlich-engine/src/world/gpu_mesher_vk.rs:25 //! risky part of v0.18.1; once the plumbing is right, an algorithmic upgrade
crates/unendlich-engine/src/world/gpu_mesher_vk.rs:28 //! # NOT yet wired into ChunkManager

## E commented registrations  (1)
crates/unendlich-engine/src/bin/unendlich_world.rs:387 //   tick_reg.register("leaf_decay",
    //       unendlich_engine::world::leaf_decay:

## F undeclared files  (3)
crates/unendlich-engine/src/render_meshref_shim.rs  (no `mod render_meshref_shim` in lib.rs)
crates/unendlich-engine/src/render/bloom.rs  (no `mod bloom` in mod.rs)
crates/unendlich-engine/src/render/postfx.rs  (no `mod postfx` in mod.rs)

## G env-var asymmetry  (4)
script sets UNENDLICH_FOO; no env::var read (dead/renamed knob)
script sets UNENDLICH_PLANAR_OVERFLOW; no env::var read (dead/renamed knob)
code reads UNENDLICH_LEAF_TICK_STRIDE; script doesn't set it (undocumented knob)
code reads UNENDLICH_PLANT_TICK_STRIDE; script doesn't set it (undocumented knob)

## H dead debug toggles  (13)
crates/unendlich-engine/src/debug_settings.rs:31 shader_mode  (zero consumers)
crates/unendlich-engine/src/debug_settings.rs:37 no_cull  (zero consumers)
crates/unendlich-engine/src/debug_settings.rs:38 disable_ssr  (zero consumers)
crates/unendlich-engine/src/debug_settings.rs:39 disable_bloom  (zero consumers)
crates/unendlich-engine/src/debug_settings.rs:43 freeze_lod  (zero consumers)
crates/unendlich-engine/src/debug_settings.rs:44 freeze_time  (zero consumers)
crates/unendlich-engine/src/debug_settings.rs:45 pause_sim  (zero consumers)
crates/unendlich-engine/src/debug_settings.rs:46 mesher_mid_gpu  (zero consumers)
crates/unendlich-engine/src/debug_settings.rs:47 neutral_tint  (zero consumers)
crates/unendlich-engine/src/debug_settings.rs:48 show_chunk_borders  (zero consumers)
crates/unendlich-engine/src/debug_settings.rs:55 fog_approach  (zero consumers)
crates/unendlich-engine/src/debug_settings.rs:59 advect_clamp  (zero consumers)
crates/unendlich-engine/src/debug_settings.rs:62 fog_steps  (zero consumers)

## I unproduced AppInputs  (1)
AppInput::SelectSlot — handled by App, never produced by the bin

## J doc->module map  (13)
bending_character_menu_ui_v2.extracted.md: ambient, camera, collision, creature, depth, diffusion, dragon, elemental, elements, emission, emissive, fabric, fixed, gravity
bending_discipline_panels_all.extracted.md: ambient, audio, blocks, buoyancy, camera, cloud, cluster, collision, combat, context, creature, depth, dragon, effects
character_panel_reference.extracted.md: ambient, blocks, cloud, combat, context, crafting, depth, dragon, elemental, elements, fabric, inventory, iridescence, light
crafting_inventory_layout.extracted.md: ambient, blocks, chunk, cluster, coral, crafting, depth, dragon, effects, elemental, elements, emission, emissive, fabric
fluid_pois_waterfalls.extracted.md: ambient, audio, biomes, blocks, bloom, cloud, cluster, collapse, context, crafting, creature, deposition, depth, dragon
gamemode_trait_system.extracted.md: ambient, attributes, behavior, biomes, blocks, carry, collision, combat, concealment, context, crafting, depth, dragon, drops
headless_coverage_testing.extracted.md: audio, awareness, behavior, biomes, blocks, chunk, combat, context, crafting, depth, dragon, driver, drops, ecosystem
item_armor_lore_tooltips.extracted.md: ambient, blocks, carry, collapse, combat, crafting, creature, depth, dragon, drops, effects, elemental, elements, emissive
mob_dragon_viewer_ui_v2.extracted.md: ambient, blocks, bloom, camera, cloud, combat, creature, depth, dragon, effects, elemental, elements, emission, emissive
phone_controller_lan-v1r2.extracted.md: audio, camera, cloud, context, creature, dragon, driver, drops, entity, fixed, input, inventory, learning, noise
textures_overlay_system.extracted.md: carry, climate, context, depth, erosion, gravity, light, material, surface, weathering
touch_input_reference.extracted.md: ambient, audio, camera, collapse, context, dragon, elements, entity, fixed, input, inventory, material, navigate, noise
tutorial_system.extracted.md: ambient, audio, blocks, camera, carry, context, crafting, creature, depth, dragon, elemental, elements, emissive, genetics

## K sibling-scope risks (bin)  (34)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:33 `chunk_manager` used above its only let (520)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:594 `camera` used above its only let (597)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:239 `local_player` used above its only let (751)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:154 `selected_block` used above its only let (1076)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:155 `hand_item` used above its only let (1078)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:336 `warmup_ms` used above its only let (1392)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:1872 `occupied` used above its only let (2649)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:1707 `placed` used above its only let (2687)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2016 `hatched` used above its only let (2758)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:479 `fields` used above its only let (3171)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:1103 `element` used above its only let (3238)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:3694 `params` used above its only let (3905)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:999 `spawns` used above its only let (4003)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:126 `persistent` used above its only let (4044)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2927 `sample` used above its only let (4305)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:4630 `heading` used above its only let (4704)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:4335 `nearest` used above its only let (4870)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:730 `removed` used above its only let (6062)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:1644 `loaded` used above its only let (6128)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:3603 `already` used above its only let (6350)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:476 `world_model` used above its only let (6374)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:1327 `player` used above its only let (7031)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:4790 `loudness` used above its only let (7041)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:645 `palette` used above its only let (7165)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:7811 `breakdown` used above its only let (7848)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:1804 `elapsed` used above its only let (7910)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:387 `cmdline` used above its only let (8028)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:8041 `octree_chunks` used above its only let (8067)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:6616 `result` used above its only let (8279)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2846 `surface` used above its only let (8465)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2498 `player_pos` used above its only let (8528)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2918 `intensity` used above its only let (8653)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:1327 `chunks` used above its only let (8742)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:8663 `radius` used above its only let (8781)
