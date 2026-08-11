# WIRING RADAR — generated scratchpad (file:line anchors)
# regen: python3 tools/wiring_radar.py   |   verify: ... check FILE   |   ... anchor FILE STR

## A zero-callsite pub fns  (1092)
crates/unendlich-engine/src/render_parity.rs:65 pub fn from_rgb8_coverage
crates/unendlich-engine/src/render_parity.rs:103 pub fn cpu_reference
crates/unendlich-engine/src/render_parity.rs:216 pub fn assert_parity
crates/unendlich-engine/src/moisture_field.rs:115 pub fn recenter
crates/unendlich-engine/src/moisture_field.rs:149 pub fn sample_world
crates/unendlich-engine/src/moisture_field.rs:158 pub fn begin_tick
crates/unendlich-engine/src/moisture_field.rs:166 pub fn sample_world_interp
crates/unendlich-engine/src/moisture_field.rs:308 pub fn wind_sampler
crates/unendlich-engine/src/water_depth_color.rs:24 pub fn default_tint
crates/unendlich-engine/src/water_depth_color.rs:38 pub fn depth_ramp
crates/unendlich-engine/src/water_depth_color.rs:53 pub fn default_absorption
crates/unendlich-engine/src/water_depth_color.rs:60 pub fn apply_absorption
crates/unendlich-engine/src/vertex_pack.rs:114 pub fn unpack_vertex
crates/unendlich-engine/src/water_field.rs:136 pub fn tick_chunk_water
crates/unendlich-engine/src/inventory.rs:170 pub fn move_into
crates/unendlich-engine/src/inventory.rs:223 pub fn is_full_for
crates/unendlich-engine/src/inventory.rs:254 pub fn selected_stack
crates/unendlich-engine/src/inventory.rs:261 pub fn consume_selected
crates/unendlich-engine/src/physics.rs:156 pub fn fly_mode
crates/unendlich-engine/src/physics.rs:269 pub fn new_player_with_id
crates/unendlich-engine/src/texture_atlas.rs:108 pub fn for_normal
crates/unendlich-engine/src/texture_atlas.rs:315 pub fn resolve_variant_uv
crates/unendlich-engine/src/texture_atlas.rs:358 pub fn block_texture_full
crates/unendlich-engine/src/water_caustics.rs:62 pub fn caustic_intensity
crates/unendlich-engine/src/pose.rs:54 pub fn from_graded
crates/unendlich-engine/src/pose.rs:64 pub fn walking
crates/unendlich-engine/src/input_capture.rs:176 pub fn latest
crates/unendlich-engine/src/whitelist.rs:87 pub fn serialize
crates/unendlich-engine/src/thread_probe.rs:164 pub fn set_with_context
crates/unendlich-engine/src/thread_probe.rs:174 pub fn set_context
crates/unendlich-engine/src/gerstner_wave.rs:47 pub fn multi_octave_height
crates/unendlich-engine/src/gerstner_wave.rs:54 pub fn multi_octave_displacement
crates/unendlich-engine/src/gerstner_wave.rs:67 pub fn amplitude_bound
crates/unendlich-engine/src/profiler.rs:143 pub fn histogram
crates/unendlich-engine/src/wing_membrane.rs:40 pub fn triangle_count
crates/unendlich-engine/src/wetness.rs:115 pub fn pack_region
crates/unendlich-engine/src/wetness.rs:261 pub fn apply_wetness
crates/unendlich-engine/src/wetness.rs:275 pub fn wet_albedo_factor
crates/unendlich-engine/src/creature_viewer.rs:51 pub fn role_color
crates/unendlich-engine/src/spawn.rs:442 pub fn resolve_use_seeded
crates/unendlich-engine/src/sumtree.rs:61 pub fn from_field
crates/unendlich-engine/src/sumtree.rs:91 pub fn region_sum
crates/unendlich-engine/src/creature_design.rs:161 pub fn jaw_hinge
crates/unendlich-engine/src/creature_design.rs:210 pub fn gaze_forward
crates/unendlich-engine/src/creature_design.rs:232 pub fn resting_gaze
crates/unendlich-engine/src/creature_design.rs:1008 pub fn random
crates/unendlich-engine/src/creature_design.rs:1018 pub fn with_choice
crates/unendlich-engine/src/creature_design.rs:1024 pub fn with_element
crates/unendlich-engine/src/region_budget.rs:61 pub fn charge_mesh
crates/unendlich-engine/src/region_budget.rs:66 pub fn charge_gen
crates/unendlich-engine/src/region_budget.rs:71 pub fn charge_resident
crates/unendlich-engine/src/region_budget.rs:83 pub fn region_cost
crates/unendlich-engine/src/region_budget.rs:117 pub fn region_count
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
crates/unendlich-engine/src/keybindings.rs:227 pub fn qwerty_wasd_preset
crates/unendlich-engine/src/keybindings.rs:297 pub fn get_action
crates/unendlich-engine/src/keybindings.rs:304 pub fn actions_for_key
crates/unendlich-engine/src/keybindings.rs:386 pub fn is_action_just_pressed_mouse
crates/unendlich-engine/src/keybindings.rs:439 pub fn load_or_default
crates/unendlich-engine/src/foam_threshold.rs:49 pub fn shore_foam
crates/unendlich-engine/src/settings_schema.rs:106 pub fn to_kv
crates/unendlich-engine/src/settings_schema.rs:125 pub fn from_kv
crates/unendlich-engine/src/pngutil.rs:13 pub fn write_png
crates/unendlich-engine/src/view_cull.rs:51 pub fn cull_mask
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
crates/unendlich-engine/src/mob_visibility.rs:115 pub fn cull_batch
crates/unendlich-engine/src/collision.rs:37 pub fn from_center_half
crates/unendlich-engine/src/collision.rs:51 pub fn half_extents
crates/unendlich-engine/src/collision.rs:74 pub fn expanded
crates/unendlich-engine/src/collision.rs:98 pub fn surface_area
crates/unendlich-engine/src/collision.rs:141 pub fn intersects_segment
crates/unendlich-engine/src/collision.rs:183 pub fn intersects_aabb
crates/unendlich-engine/src/runtime_config.rs:185 pub fn matching_preset
crates/unendlich-engine/src/fabric.rs:118 pub fn tip
crates/unendlich-engine/src/fabric.rs:124 pub fn total_length
crates/unendlich-engine/src/debug_overlay.rs:113 pub fn primitive_count
crates/unendlich-engine/src/storm_field.rs:78 pub fn airborne_total
crates/unendlich-engine/src/item.rs:66 pub fn emblem
crates/unendlich-engine/src/item.rs:122 pub fn from_ore
crates/unendlich-engine/src/item.rs:189 pub fn weapon
crates/unendlich-engine/src/item.rs:259 pub fn is_spawn_egg
crates/unendlich-engine/src/item.rs:325 pub fn item_catalog
crates/unendlich-engine/src/timebend_scrub.rs:143 pub fn timeline_fraction
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
crates/unendlich-engine/src/inventory_screen.rs:121 pub fn wants_mouse_captured
crates/unendlich-engine/src/inventory_screen.rs:146 pub fn set_tab
crates/unendlich-engine/src/inventory_screen.rs:157 pub fn left_click
crates/unendlich-engine/src/inventory_screen.rs:186 pub fn right_click
crates/unendlich-engine/src/occupancy.rs:32 pub fn from_voxels
crates/unendlich-engine/src/occupancy.rs:99 pub fn first_solid_ray
crates/unendlich-engine/src/occupancy.rs:154 pub fn coarse_occlusion
crates/unendlich-engine/src/loot.rs:111 pub fn drops_with_legendary
crates/unendlich-engine/src/light.rs:173 pub fn propagate_block_light
crates/unendlich-engine/src/light.rs:290 pub fn add_light
crates/unendlich-engine/src/light.rs:320 pub fn remove_light
crates/unendlich-engine/src/light.rs:372 pub fn skylight_columns
crates/unendlich-engine/src/creature_viewer_ui.rs:191 pub fn creature_viewer_window
crates/unendlich-engine/src/input_codes.rs:119 pub fn to_i32
crates/unendlich-engine/src/input_codes.rs:124 pub fn from_i32
crates/unendlich-engine/src/input_codes.rs:262 pub fn key_action
crates/unendlich-engine/src/input_codes.rs:285 pub fn keycode_from_winit
crates/unendlich-engine/src/thermal.rs:250 pub fn thermal_shock
crates/unendlich-engine/src/thermal.rs:343 pub fn resolve_thermal_failures
crates/unendlich-engine/src/util/mathx.rs:81 pub fn remap
crates/unendlich-engine/src/util/simvec.rs:33 pub fn add_into
crates/unendlich-engine/src/util/simvec.rs:41 pub fn scale_into
crates/unendlich-engine/src/util/simvec.rs:49 pub fn dot_into
crates/unendlich-engine/src/util/simvec.rs:57 pub fn len_into
crates/unendlich-engine/src/util/simvec.rs:69 pub fn normalize_into
crates/unendlich-engine/src/util/simvec.rs:82 pub fn lerp_into
crates/unendlich-engine/src/util/simvec.rs:117 pub fn from_aos
crates/unendlich-engine/src/util/simvec.rs:132 pub fn to_aos
crates/unendlich-engine/src/util/simvec.rs:144 pub fn len_into_soa
crates/unendlich-engine/src/util/simvec.rs:153 pub fn dot_into_soa
crates/unendlich-engine/src/util/simvec.rs:170 pub fn normalize_into_soa
crates/unendlich-engine/src/util/mesh_dirty.rs:121 pub fn invalidate_all
crates/unendlich-engine/src/util/mesh_dirty.rs:124 pub fn tracked_count
crates/unendlich-engine/src/util/task_lanes.rs:53 pub fn compute_patches
crates/unendlich-engine/src/util/task_lanes.rs:64 pub fn apply_patches
crates/unendlich-engine/src/util/fastmath.rs:75 pub fn cos_fast
crates/unendlich-engine/src/util/fastmath.rs:83 pub fn exp_fast
crates/unendlich-engine/src/util/vecmath.rs:29 pub fn scale_clamp01
crates/unendlich-engine/src/util/batch_kernel.rs:35 pub fn map_into
crates/unendlich-engine/src/telemetry/achievements.rs:68 pub fn register_all
crates/unendlich-engine/src/telemetry/mod.rs:83 pub fn register_specs
crates/unendlich-engine/src/telemetry/mod.rs:104 pub fn bump
crates/unendlich-engine/src/telemetry/mod.rs:108 pub fn set_counter
crates/unendlich-engine/src/telemetry/mod.rs:112 pub fn achievements
crates/unendlich-engine/src/telemetry/mod.rs:118 pub fn build_report
crates/unendlich-engine/src/telemetry/report.rs:124 pub fn append_to
crates/unendlich-engine/src/app/menu_view.rs:88 pub fn menu_ui
crates/unendlich-engine/src/app/menu_view.rs:227 pub fn title_flythrough_ui
crates/unendlich-engine/src/app/menu_view.rs:323 pub fn map_click
crates/unendlich-engine/src/app/menu_view.rs:393 pub fn character_select_ui
crates/unendlich-engine/src/app/mod.rs:184 pub fn menu_mut
crates/unendlich-engine/src/app/mod.rs:207 pub fn cinematic_elapsed
crates/unendlich-engine/src/app/mod.rs:310 pub fn gameplay_tick
crates/unendlich-engine/src/app/multiplayer_svc.rs:55 pub fn toggle_host
crates/unendlich-engine/src/world/water_field.rs:75 pub fn seed_from_chunk
crates/unendlich-engine/src/world/water_field.rs:202 pub fn absorb_at
crates/unendlich-engine/src/world/worldgen_noise.rs:44 pub fn fmix64
crates/unendlich-engine/src/world/worldgen_noise.rs:80 pub fn density_curve
crates/unendlich-engine/src/world/worldgen_noise.rs:101 pub fn scatter_region
crates/unendlich-engine/src/world/ticks.rs:350 pub fn strided
crates/unendlich-engine/src/world/field_brush.rs:166 pub fn apply_brush_3d
crates/unendlich-engine/src/world/field_brush.rs:179 pub fn apply_brush_2d
crates/unendlich-engine/src/world/plant_exotics.rs:117 pub fn effect_at
crates/unendlich-engine/src/world/span_alloc.rs:62 pub fn can_alloc
crates/unendlich-engine/src/world/occlusion_flood.rs:80 pub fn culled_count
crates/unendlich-engine/src/world/stem.rs:118 pub fn tip_count
crates/unendlich-engine/src/world/stem.rs:125 pub fn grow_step
crates/unendlich-engine/src/world/orographic_precip.rs:48 pub fn augment_cell
crates/unendlich-engine/src/world/water_field_batch.rs:27 pub fn settle_chunks
crates/unendlich-engine/src/world/water_field_batch.rs:40 pub fn evaporate_rain
crates/unendlich-engine/src/world/lumen.rs:64 pub fn charged
crates/unendlich-engine/src/world/lumen.rs:255 pub fn bench_field
crates/unendlich-engine/src/world/octree_scheduler.rs:148 pub fn drain_ready
crates/unendlich-engine/src/world/octree_scheduler.rs:199 pub fn mark_processed
crates/unendlich-engine/src/world/water_cells.rs:52 pub fn pour_into
crates/unendlich-engine/src/world/water_cells.rs:87 pub fn step_cells
crates/unendlich-engine/src/world/season.rs:102 pub fn season_for_phase
crates/unendlich-engine/src/world/octree_region.rs:127 pub fn from_octree_chunk_local
crates/unendlich-engine/src/world/octree_region.rs:176 pub fn is_stable
crates/unendlich-engine/src/world/chunk_manager.rs:498 pub fn octree_sampler
crates/unendlich-engine/src/world/chunk_manager.rs:679 pub fn set_fields
crates/unendlich-engine/src/world/chunk_manager.rs:706 pub fn set_tick_period
crates/unendlich-engine/src/world/chunk_manager.rs:1878 pub fn generator_seed
crates/unendlich-engine/src/world/chunk_manager.rs:2108 pub fn loaded_coords
crates/unendlich-engine/src/world/water_waves.rs:115 pub fn water_wave_height
crates/unendlich-engine/src/world/water_waves.rs:121 pub fn water_wave_normal
crates/unendlich-engine/src/world/plant_blueprint.rs:669 pub fn reed_blueprint
crates/unendlich-engine/src/world/plant_blueprint.rs:684 pub fn fern_blueprint
crates/unendlich-engine/src/world/plant_blueprint.rs:703 pub fn vine_blueprint
crates/unendlich-engine/src/world/plant_blueprint.rs:726 pub fn shrub_blueprint
crates/unendlich-engine/src/world/plant_blueprint.rs:743 pub fn wildflower_blueprint
crates/unendlich-engine/src/world/plant_blueprint.rs:764 pub fn mushroom_blueprint
crates/unendlich-engine/src/world/plant_blueprint.rs:779 pub fn species_colors
crates/unendlich-engine/src/world/plant_blueprint.rs:799 pub fn species_blueprint
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
crates/unendlich-engine/src/world/brick_plan.rs:93 pub fn brick_count
crates/unendlich-engine/src/world/brick_plan.rs:102 pub fn touched_chunks
crates/unendlich-engine/src/world/brick_plan.rs:133 pub fn dirty_brick_total
crates/unendlich-engine/src/world/batch_density_noise.rs:204 pub fn fbm_3d_row_x
crates/unendlich-engine/src/world/cave_field.rs:182 pub fn slice_void_counts
crates/unendlich-engine/src/world/chunk_loader.rs:210 pub fn cancel
crates/unendlich-engine/src/world/chunk_loader.rs:221 pub fn retain_wanted
crates/unendlich-engine/src/world/update_class.rs:115 pub fn effective_policy
crates/unendlich-engine/src/world/update_class.rs:124 pub fn needs_remesh
crates/unendlich-engine/src/world/update_class.rs:130 pub fn needs_relight
crates/unendlich-engine/src/world/surface.rs:81 pub fn face_mask
crates/unendlich-engine/src/world/surface.rs:262 pub fn face
crates/unendlich-engine/src/world/effects.rs:80 pub fn verb_effects
crates/unendlich-engine/src/world/biome_loader.rs:272 pub fn get_by_id
crates/unendlich-engine/src/world/biome_loader.rs:319 pub fn load_directory
crates/unendlich-engine/src/world/biome_loader.rs:362 pub fn load_file
crates/unendlich-engine/src/world/attributes.rs:119 pub fn set_base
crates/unendlich-engine/src/world/attributes.rs:147 pub fn set_role_level
crates/unendlich-engine/src/world/brick_occupancy.rs:59 pub fn needs_mesh
crates/unendlich-engine/src/world/brick_occupancy.rs:70 pub fn exposed_faces
crates/unendlich-engine/src/world/flow_field.rs:132 pub fn step_target
crates/unendlich-engine/src/world/shader_residency.rs:47 pub fn is_resident
crates/unendlich-engine/src/world/shader_residency.rs:51 pub fn resident_count
crates/unendlich-engine/src/world/mob_persistence.rs:74 pub fn mark_removed
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
crates/unendlich-engine/src/world/ecosystem_census.rs:176 pub fn census
crates/unendlich-engine/src/world/gpu_mesher_vk.rs:294 pub fn pool_buffer
crates/unendlich-engine/src/world/gpu_mesher_vk.rs:302 pub fn mesh_batch
crates/unendlich-engine/src/world/block_textures.rs:116 pub fn face_tile
crates/unendlich-engine/src/world/block_textures.rs:128 pub fn face_tile_byte
crates/unendlich-engine/src/world/block_textures.rs:141 pub fn atlas_rgba
crates/unendlich-engine/src/world/entity.rs:182 pub fn contains_point
crates/unendlich-engine/src/world/entity.rs:490 pub fn despawn_entity
crates/unendlich-engine/src/world/entity.rs:600 pub fn resolve_intersection
crates/unendlich-engine/src/world/entity.rs:689 pub fn entity_resolve_intersection
crates/unendlich-engine/src/world/entity.rs:738 pub fn at_least
crates/unendlich-engine/src/world/entity.rs:741 pub fn at_most
crates/unendlich-engine/src/world/entity.rs:834 pub fn evaluate_spawn_rules
crates/unendlich-engine/src/world/entity.rs:934 pub fn opaque
crates/unendlich-engine/src/world/entity.rs:951 pub fn default_color_for_kind
crates/unendlich-engine/src/world/entity.rs:1376 pub fn blueprint_to_vertices
crates/unendlich-engine/src/world/area_mining.rs:81 pub fn area_break_cells
crates/unendlich-engine/src/world/area_mining.rs:107 pub fn cube_area_break
crates/unendlich-engine/src/world/chunk.rs:528 pub fn recompute_cached_all_air
crates/unendlich-engine/src/world/chunk.rs:547 pub fn is_explicit
crates/unendlich-engine/src/world/chunk.rs:727 pub fn explicit_count
crates/unendlich-engine/src/world/chunk.rs:775 pub fn age_block
crates/unendlich-engine/src/world/chunk.rs:808 pub fn is_transitioning
crates/unendlich-engine/src/world/chunk.rs:814 pub fn is_all_air
crates/unendlich-engine/src/world/weather_front.rs:37 pub fn advanced
crates/unendlich-engine/src/world/weather_front.rs:58 pub fn advect_moisture
crates/unendlich-engine/src/world/weather_front.rs:88 pub fn resolve_strip
crates/unendlich-engine/src/world/adaptive_stream.rs:67 pub fn for_hz
crates/unendlich-engine/src/world/adaptive_stream.rs:75 pub fn streaming_us
crates/unendlich-engine/src/world/adaptive_stream.rs:160 pub fn drain_to_budget
crates/unendlich-engine/src/world/sim_prewarm.rs:32 pub fn plan_prewarm
crates/unendlich-engine/src/world/water_edge_flow.rs:40 pub fn flow_edge
crates/unendlich-engine/src/world/blocks.rs:268 pub fn is_replaceable
crates/unendlich-engine/src/world/blocks.rs:300 pub fn bounce
crates/unendlich-engine/src/world/blocks.rs:324 pub fn is_visible
crates/unendlich-engine/src/world/stage_tint.rs:52 pub fn transition_tint
crates/unendlich-engine/src/world/stage_tint.rs:70 pub fn stage_tint
crates/unendlich-engine/src/world/water_flow_field.rs:74 pub fn dir_speed
crates/unendlich-engine/src/world/subvoxel.rs:178 pub fn grass_tuft
crates/unendlich-engine/src/world/cave_flora.rs:49 pub fn from_stem
crates/unendlich-engine/src/world/weather.rs:548 pub fn precip_for
crates/unendlich-engine/src/world/collapse.rs:39 pub fn support_distance
crates/unendlich-engine/src/world/collapse.rs:63 pub fn floating
crates/unendlich-engine/src/world/collapse.rs:72 pub fn overhang
crates/unendlich-engine/src/world/terrain_batch.rs:26 pub fn generate_chunks_threshold
crates/unendlich-engine/src/world/surface_run.rs:167 pub fn compression_ratio
crates/unendlich-engine/src/world/sediment_map.rs:90 pub fn imbalance
crates/unendlich-engine/src/world/layered_rle.rs:39 pub fn from_dense
crates/unendlich-engine/src/world/layered_rle.rs:99 pub fn run_count
crates/unendlich-engine/src/world/layered_rle.rs:115 pub fn composite_get
crates/unendlich-engine/src/world/chunk_flora.rs:37 pub fn populate
crates/unendlich-engine/src/world/chunk_flora.rs:79 pub fn pearl_count
crates/unendlich-engine/src/world/mesh_brick.rs:86 pub fn mark_brick
crates/unendlich-engine/src/world/mesh_brick.rs:91 pub fn mark_all
crates/unendlich-engine/src/world/mesh_brick.rs:100 pub fn clear_all
crates/unendlich-engine/src/world/flora_map.rs:26 pub fn ctx_at
crates/unendlich-engine/src/world/upload_repr.rs:217 pub fn all_octree_baseline_bytes
crates/unendlich-engine/src/world/upload_repr.rs:227 pub fn project_residency
crates/unendlich-engine/src/world/worldpack.rs:181 pub fn resolve_brushes
crates/unendlich-engine/src/world/world_list.rs:75 pub fn with_rule
crates/unendlich-engine/src/world/world_list.rs:84 pub fn set_rule
crates/unendlich-engine/src/world/world_list.rs:96 pub fn disallow
crates/unendlich-engine/src/world/world_list.rs:103 pub fn clear_whitelist
crates/unendlich-engine/src/world/world_list.rs:115 pub fn request_log
crates/unendlich-engine/src/world/world_list.rs:127 pub fn worlds
crates/unendlich-engine/src/world/gen_falloff.rs:38 pub fn ring_budget
crates/unendlich-engine/src/world/gen_falloff.rs:60 pub fn effective_budget
crates/unendlich-engine/src/world/terrain_llm.rs:53 pub fn take_pending
crates/unendlich-engine/src/world/ecosystem.rs:152 pub fn spawns_around
crates/unendlich-engine/src/world/biomes.rs:66 pub fn erosion_factor
crates/unendlich-engine/src/world/biomes.rs:91 pub fn cave_threshold
crates/unendlich-engine/src/world/biomes.rs:388 pub fn from_climate
crates/unendlich-engine/src/world/fields.rs:56 pub fn filled
crates/unendlich-engine/src/world/fields.rs:255 pub fn invalidate
crates/unendlich-engine/src/world/octree_compact.rs:84 pub fn compact
crates/unendlich-engine/src/world/elemental.rs:66 pub fn raise_wall
crates/unendlich-engine/src/world/elemental.rs:96 pub fn raise_pillar
crates/unendlich-engine/src/world/elemental.rs:116 pub fn smash_crater
crates/unendlich-engine/src/world/elemental.rs:137 pub fn fill_sphere
crates/unendlich-engine/src/world/elemental.rs:210 pub fn run_verb
crates/unendlich-engine/src/world/elemental.rs:247 pub fn apply_edits
crates/unendlich-engine/src/world/rare_structure.rs:221 pub fn select_for_region
crates/unendlich-engine/src/world/rebuild_coalescer.rs:36 pub fn started_at
crates/unendlich-engine/src/world/rebuild_coalescer.rs:47 pub fn note_edit
crates/unendlich-engine/src/world/rebuild_coalescer.rs:61 pub fn after_rebuild
crates/unendlich-engine/src/world/device_tier.rs:121 pub fn budgets_for_caps
crates/unendlich-engine/src/world/stem_gpu.rs:84 pub fn grow_step_parallel
crates/unendlich-engine/src/world/stem_gpu.rs:186 pub fn bench_growth
crates/unendlich-engine/src/world/octree_budget.rs:21 pub fn target_octree_bytes
crates/unendlich-engine/src/world/simd_cull.rs:45 pub fn cull_spheres
crates/unendlich-engine/src/world/simd_cull.rs:131 pub fn visible_count
crates/unendlich-engine/src/world/octree_raycast.rs:192 pub fn raycast_world
crates/unendlich-engine/src/world/water_cycle.rs:106 pub fn step_cycles
crates/unendlich-engine/src/world/shader_contract.rs:103 pub fn check_world_shaders
crates/unendlich-engine/src/world/mesh_epoch.rs:38 pub fn bumped
crates/unendlich-engine/src/world/column_cache.rs:50 pub fn get_or_compute
crates/unendlich-engine/src/world/column_cache.rs:95 pub fn hits
crates/unendlich-engine/src/world/brick_mesher.rs:50 pub fn mesh_brick
crates/unendlich-engine/src/world/flora_sites.rs:68 pub fn find_sites
crates/unendlich-engine/src/world/plant_genetics.rs:194 pub fn growth_response
crates/unendlich-engine/src/world/material_response.rs:181 pub fn bump_amplitude
crates/unendlich-engine/src/world/parkour.rs:129 pub fn count_walls
crates/unendlich-engine/src/world/parkour.rs:161 pub fn try_leap
crates/unendlich-engine/src/world/gpu_chunk_pool_vk.rs:131 pub fn slot_stride_bytes
crates/unendlich-engine/src/world/gpu_chunk_pool_vk.rs:144 pub fn slot_offset_bytes
crates/unendlich-engine/src/world/named_locations.rs:94 pub fn name_at
crates/unendlich-engine/src/world/cpu_generator.rs:127 pub fn set_biome_registry
crates/unendlich-engine/src/world/cpu_generator.rs:1279 pub fn surface_run_preview
crates/unendlich-engine/src/world/scheduler.rs:207 pub fn tick_dt_for_band
crates/unendlich-engine/src/world/field_ops.rs:74 pub fn diffuse_flux
crates/unendlich-engine/src/world/field_ops.rs:133 pub fn diffuse_gather
crates/unendlich-engine/src/world/field_ops.rs:151 pub fn diffuse_gather_parallel
crates/unendlich-engine/src/sim/command_parse.rs:250 pub fn opens_gui
crates/unendlich-engine/src/sim/command_parse.rs:258 pub fn to_command_string
crates/unendlich-engine/src/sim/technique_catalogue.rs:159 pub fn castable
crates/unendlich-engine/src/sim/technique_catalogue.rs:165 pub fn effective_cost
crates/unendlich-engine/src/sim/shader_variant.rs:76 pub fn defines
crates/unendlich-engine/src/sim/refresh_snap.rs:22 pub fn snap_at_or_below
crates/unendlich-engine/src/sim/refresh_snap.rs:36 pub fn nearest_snap
crates/unendlich-engine/src/sim/grove_decay.rs:61 pub fn alive
crates/unendlich-engine/src/sim/mesh_partition.rs:77 pub fn opposite
crates/unendlich-engine/src/sim/mesh_partition.rs:89 pub fn axis_sign
crates/unendlich-engine/src/sim/mesh_partition.rs:104 pub fn from_axis_delta
crates/unendlich-engine/src/sim/mesh_partition.rs:141 pub fn from_face_index
crates/unendlich-engine/src/sim/mesh_partition.rs:160 pub fn assign_quad
crates/unendlich-engine/src/sim/mesh_partition.rs:168 pub fn dirtied_by_neighbor
crates/unendlich-engine/src/sim/mesh_partition.rs:208 pub fn unmark
crates/unendlich-engine/src/sim/stroke_learning.rs:104 pub fn infer_aliases
crates/unendlich-engine/src/sim/stroke_learning.rs:157 pub fn add_session_alias
crates/unendlich-engine/src/sim/stroke_learning.rs:183 pub fn should_promote
crates/unendlich-engine/src/sim/stroke_learning.rs:190 pub fn promote
crates/unendlich-engine/src/sim/stroke_learning.rs:195 pub fn is_permanent
crates/unendlich-engine/src/sim/skills.rs:79 pub fn xp
crates/unendlich-engine/src/sim/skills.rs:87 pub fn total_levels
crates/unendlich-engine/src/sim/skills.rs:101 pub fn can_invest
crates/unendlich-engine/src/sim/skills.rs:107 pub fn invest
crates/unendlich-engine/src/sim/skills.rs:132 pub fn scaled
crates/unendlich-engine/src/sim/skills.rs:138 pub fn respec
crates/unendlich-engine/src/sim/env_frame.rs:62 pub fn frame_env
crates/unendlich-engine/src/sim/sync_check.rs:74 pub fn confirmed
crates/unendlich-engine/src/sim/consensus.rs:53 pub fn two_of_any
crates/unendlich-engine/src/sim/consensus.rs:58 pub fn majority_of
crates/unendlich-engine/src/sim/consensus.rs:76 pub fn confirmed_digest
crates/unendlich-engine/src/sim/consensus.rs:137 pub fn bench_tally
crates/unendlich-engine/src/sim/shader_from_image.rs:395 pub fn synth_pattern_glsl
crates/unendlich-engine/src/sim/secondary_motion.rs:53 pub fn influence
crates/unendlich-engine/src/sim/secondary_motion.rs:67 pub fn deflect
crates/unendlich-engine/src/sim/secondary_motion.rs:106 pub fn disturber_strength
crates/unendlich-engine/src/sim/popgen.rs:101 pub fn analyze
crates/unendlich-engine/src/sim/provocation.rs:129 pub fn update_escalating
crates/unendlich-engine/src/sim/ai_budget.rs:66 pub fn dt_scale
crates/unendlich-engine/src/sim/mesh_budget.rs:61 pub fn estimate_cost
crates/unendlich-engine/src/sim/mesh_budget.rs:98 pub fn default_budget
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
crates/unendlich-engine/src/sim/audio_occlusion.rs:75 pub fn occluded_mix_gains
crates/unendlich-engine/src/sim/audio_occlusion.rs:110 pub fn muffle_cutoff_hz
crates/unendlich-engine/src/sim/recipe_book.rs:50 pub fn recipe_count
crates/unendlich-engine/src/sim/recipe_book.rs:55 pub fn ingredient_count
crates/unendlich-engine/src/sim/recipe_book.rs:91 pub fn find_by_any
crates/unendlich-engine/src/sim/diffusion.rs:170 pub fn run_many
crates/unendlich-engine/src/sim/redirect.rs:68 pub fn attempt_redirect
crates/unendlich-engine/src/sim/surface_cache.rs:37 pub fn get_or
crates/unendlich-engine/src/sim/combustion_spread.rs:62 pub fn burn_duration
crates/unendlich-engine/src/sim/combustion_spread.rs:121 pub fn tick_burn
crates/unendlich-engine/src/sim/egg_genome.rs:22 pub fn to_egg
crates/unendlich-engine/src/sim/wind_perturb.rs:141 pub fn perturbed
crates/unendlich-engine/src/sim/batch_body_arc.rs:56 pub fn arc_batch_into
crates/unendlich-engine/src/sim/inventory_grid.rs:120 pub fn count_of
crates/unendlich-engine/src/sim/inventory_grid.rs:183 pub fn move_to
crates/unendlich-engine/src/sim/inventory_grid.rs:213 pub fn stack_onto
crates/unendlich-engine/src/sim/salvage_yield.rs:80 pub fn salvage
crates/unendlich-engine/src/sim/plant_growth_env.rs:110 pub fn should_wilt
crates/unendlich-engine/src/sim/plant_growth_env.rs:121 pub fn effective_rate
crates/unendlich-engine/src/sim/rest_cycle.rs:72 pub fn rhythm_for_mob
crates/unendlich-engine/src/sim/survival_exposure.rs:132 pub fn to_env_sample
crates/unendlich-engine/src/sim/surface_scan.rs:60 pub fn top_solid_y
crates/unendlich-engine/src/sim/surface_scan.rs:69 pub fn top_solid_y_by
crates/unendlich-engine/src/sim/surface_scan.rs:104 pub fn top_solid_y_in_chunk
crates/unendlich-engine/src/sim/record_zones.rs:133 pub fn zone_samples
crates/unendlich-engine/src/sim/desync_monitor.rs:64 pub fn interval
crates/unendlich-engine/src/sim/desync_monitor.rs:74 pub fn wants_deep_audit
crates/unendlich-engine/src/sim/desync_monitor.rs:115 pub fn event_line
crates/unendlich-engine/src/sim/whitelist.rs:61 pub fn revoke
crates/unendlich-engine/src/sim/whitelist.rs:82 pub fn set_open
crates/unendlich-engine/src/sim/shape_scale.rs:60 pub fn is_hollow
crates/unendlich-engine/src/sim/shape_scale.rs:80 pub fn resolved_dimensions
crates/unendlich-engine/src/sim/shape_scale.rs:104 pub fn integrity
crates/unendlich-engine/src/sim/shape_scale.rs:124 pub fn material_for_s
crates/unendlich-engine/src/sim/shape_scale.rs:139 pub fn upgrade_one_tier
crates/unendlich-engine/src/sim/shape_scale.rs:152 pub fn forced_deepslate_holds
crates/unendlich-engine/src/sim/heat_distort.rs:147 pub fn bench_distort
crates/unendlich-engine/src/sim/item_resolve.rs:181 pub fn to_item_view
crates/unendlich-engine/src/sim/item_resolve.rs:213 pub fn market_price_resolved
crates/unendlich-engine/src/sim/moving_platform.rs:124 pub fn carry
crates/unendlich-engine/src/sim/moving_platform.rs:131 pub fn on_top
crates/unendlich-engine/src/sim/locomotion_sim.rs:210 pub fn min_separation
crates/unendlich-engine/src/sim/locomotion_sim.rs:223 pub fn trace_json
crates/unendlich-engine/src/sim/legendary_tier.rs:113 pub fn region_has_rare
crates/unendlich-engine/src/sim/legendary_tier.rs:121 pub fn biome_anchor_region
crates/unendlich-engine/src/sim/legendary_tier.rs:131 pub fn world_has_mythic
crates/unendlich-engine/src/sim/legendary_tier.rs:138 pub fn tier_from_roll
crates/unendlich-engine/src/sim/mob_viewer.rs:60 pub fn yaw
crates/unendlich-engine/src/sim/mob_viewer.rs:64 pub fn pitch
crates/unendlich-engine/src/sim/mob_viewer.rs:70 pub fn focus_region
crates/unendlich-engine/src/sim/mob_viewer.rs:76 pub fn rotate
crates/unendlich-engine/src/sim/mob_viewer.rs:82 pub fn zoom
crates/unendlich-engine/src/sim/mob_viewer.rs:95 pub fn clear_focus
crates/unendlich-engine/src/sim/mob_viewer.rs:115 pub fn is_inspecting
crates/unendlich-engine/src/sim/mob_viewer.rs:128 pub fn eye_offset
crates/unendlich-engine/src/sim/aabb_resolve.rs:68 pub fn mtv
crates/unendlich-engine/src/sim/item_category.rs:124 pub fn filter_indices
crates/unendlich-engine/src/sim/dragon_roost.rs:76 pub fn detection_range
crates/unendlich-engine/src/sim/dragon_roost.rs:84 pub fn is_named
crates/unendlich-engine/src/sim/dragon_roost.rs:90 pub fn has_marks
crates/unendlich-engine/src/sim/dragon_roost.rs:147 pub fn roost_modifiers_at
crates/unendlich-engine/src/sim/affliction.rs:150 pub fn is_clear
crates/unendlich-engine/src/sim/locomotion_mode.rs:94 pub fn applies_buoyancy
crates/unendlich-engine/src/sim/locomotion_mode.rs:97 pub fn applies_gait
crates/unendlich-engine/src/sim/locomotion_mode.rs:100 pub fn applies_flight
crates/unendlich-engine/src/sim/locomotion_mode.rs:103 pub fn applies_gravity
crates/unendlich-engine/src/sim/convergence.rs:62 pub fn skippable_past_horizon
crates/unendlich-engine/src/sim/convergence.rs:102 pub fn classify_structural
crates/unendlich-engine/src/sim/creature_grime.rs:60 pub fn grime_factor01
crates/unendlich-engine/src/sim/ecology_population.rs:218 pub fn spawn_pressure
crates/unendlich-engine/src/sim/hot_metal.rs:49 pub fn glow
crates/unendlich-engine/src/sim/forage_harvest.rs:228 pub fn apply_harvest
crates/unendlich-engine/src/sim/forage_harvest.rs:244 pub fn total_units
crates/unendlich-engine/src/sim/sound.rs:130 pub fn footstep_loudness_on_stanced
crates/unendlich-engine/src/sim/sound.rs:300 pub fn can_hear
crates/unendlich-engine/src/sim/deeptime_convergence.rs:123 pub fn min_stage_count
crates/unendlich-engine/src/sim/deeptime_convergence.rs:138 pub fn plan_stage_cost
crates/unendlich-engine/src/sim/deeptime_convergence.rs:149 pub fn classify_system
crates/unendlich-engine/src/sim/predation.rs:29 pub fn from_capabilities
crates/unendlich-engine/src/sim/predation.rs:71 pub fn hunt
crates/unendlich-engine/src/sim/fire_field.rs:51 pub fn fuel
crates/unendlich-engine/src/sim/fire_field.rs:103 pub fn total_fuel
crates/unendlich-engine/src/sim/fire_field.rs:108 pub fn total_intensity
crates/unendlich-engine/src/sim/fire_field.rs:113 pub fn burning_count
crates/unendlich-engine/src/sim/player_traits.rs:124 pub fn traits_for
crates/unendlich-engine/src/sim/player_traits.rs:148 pub fn is_targetable
crates/unendlich-engine/src/sim/player_traits.rs:154 pub fn takes_fall_damage
crates/unendlich-engine/src/sim/player_traits.rs:174 pub fn has_hunger
crates/unendlich-engine/src/sim/camera_tremble.rs:52 pub fn pos_magnitude
crates/unendlich-engine/src/sim/dragon.rs:245 pub fn make_recurrent
crates/unendlich-engine/src/sim/dragon.rs:262 pub fn head_scan
crates/unendlich-engine/src/sim/dragon.rs:356 pub fn breath_scale
crates/unendlich-engine/src/sim/dragon.rs:376 pub fn take_damage
crates/unendlich-engine/src/sim/particles.rs:140 pub fn particles
crates/unendlich-engine/src/sim/particles.rs:247 pub fn render_buffer
crates/unendlich-engine/src/sim/mountain_sources.rs:63 pub fn step_sources
crates/unendlich-engine/src/sim/mountain_sources.rs:73 pub fn step_sources_threshold
crates/unendlich-engine/src/sim/liveliness.rs:32 pub fn limpness
crates/unendlich-engine/src/sim/liveliness.rs:80 pub fn breath_for
crates/unendlich-engine/src/sim/navfield_gpu.rs:160 pub fn bench_navfield
crates/unendlich-engine/src/sim/navfield_gpu.rs:260 pub fn bench_flow
crates/unendlich-engine/src/sim/smoke_gpu.rs:190 pub fn bench_smoke_step
crates/unendlich-engine/src/sim/mob_bands.rs:39 pub fn band_label
crates/unendlich-engine/src/sim/trophic_web.rs:77 pub fn is_apex
crates/unendlich-engine/src/sim/trophic_web.rs:177 pub fn migration_pull
crates/unendlich-engine/src/sim/spawn.rs:118 pub fn spawn_around
crates/unendlich-engine/src/sim/projectile.rs:42 pub fn dust_clod
crates/unendlich-engine/src/sim/projectile.rs:55 pub fn impact
crates/unendlich-engine/src/sim/world_management.rs:59 pub fn open_to_dht
crates/unendlich-engine/src/sim/world_management.rs:63 pub fn whitelist_on
crates/unendlich-engine/src/sim/world_management.rs:66 pub fn max_players
crates/unendlich-engine/src/sim/world_management.rs:69 pub fn world_name
crates/unendlich-engine/src/sim/world_management.rs:75 pub fn set_open_to_dht
crates/unendlich-engine/src/sim/world_management.rs:80 pub fn set_whitelist_on
crates/unendlich-engine/src/sim/world_management.rs:86 pub fn set_max_players
crates/unendlich-engine/src/sim/world_management.rs:104 pub fn name_valid
crates/unendlich-engine/src/sim/world_management.rs:136 pub fn apply_open_to_dht
crates/unendlich-engine/src/sim/essence_refine.rs:117 pub fn refine
crates/unendlich-engine/src/sim/hybrid_step.rs:47 pub fn plan_schedule
crates/unendlich-engine/src/sim/hybrid_step.rs:67 pub fn substep_seed
crates/unendlich-engine/src/sim/hybrid_step.rs:114 pub fn total_exact_evals
crates/unendlich-engine/src/sim/dragon_elemental.rs:33 pub fn choose_element
crates/unendlich-engine/src/sim/fauna_gait.rs:48 pub fn proportions
crates/unendlich-engine/src/sim/fauna_gait.rs:119 pub fn gait_for
crates/unendlich-engine/src/sim/attach_frame.rs:53 pub fn from_yaw_pitch
crates/unendlich-engine/src/sim/builder_genome.rs:69 pub fn prevalence
crates/unendlich-engine/src/sim/builder_genome.rs:110 pub fn has_material_sensing
crates/unendlich-engine/src/sim/builder_genome.rs:116 pub fn builds_coherently
crates/unendlich-engine/src/sim/builder_genome.rs:152 pub fn architecture_style
crates/unendlich-engine/src/sim/builder_genome.rs:164 pub fn multi_material
crates/unendlich-engine/src/sim/infusion_catalog.rs:122 pub fn try_infuse
crates/unendlich-engine/src/sim/object_permanence.rs:57 pub fn ghost
crates/unendlich-engine/src/sim/object_permanence.rs:87 pub fn confidence
crates/unendlich-engine/src/sim/object_permanence.rs:128 pub fn observe_partial
crates/unendlich-engine/src/sim/object_permanence.rs:145 pub fn search_point
crates/unendlich-engine/src/sim/ambient.rs:70 pub fn heard_loudness_masked
crates/unendlich-engine/src/sim/dragon_energy.rs:81 pub fn bending_output
crates/unendlich-engine/src/sim/dragon_energy.rs:92 pub fn flight_range_scale
crates/unendlich-engine/src/sim/dragon_energy.rs:108 pub fn seeks_food
crates/unendlich-engine/src/sim/dragon_energy.rs:113 pub fn will_fight_prolonged
crates/unendlich-engine/src/sim/dragon_energy.rs:119 pub fn is_desperate
crates/unendlich-engine/src/sim/dragon_energy.rs:125 pub fn expend
crates/unendlich-engine/src/sim/dragon_energy.rs:132 pub fn can_afford
crates/unendlich-engine/src/sim/item_genome.rs:56 pub fn village
crates/unendlich-engine/src/sim/impact_response.rs:49 pub fn respond
crates/unendlich-engine/src/sim/block_placement.rs:122 pub fn can_place
crates/unendlich-engine/src/sim/stance_culture.rs:59 pub fn adopt
crates/unendlich-engine/src/sim/stance_culture.rs:64 pub fn forget_by_coiner
crates/unendlich-engine/src/sim/stance_culture.rs:93 pub fn is_shared_culture
crates/unendlich-engine/src/sim/emission.rs:139 pub fn total_heat
crates/unendlich-engine/src/sim/emission.rs:257 pub fn breath_cone
crates/unendlich-engine/src/sim/emission.rs:330 pub fn sky_call
crates/unendlich-engine/src/sim/emission.rs:342 pub fn trail
crates/unendlich-engine/src/sim/forage_sim.rs:60 pub fn grass_count
crates/unendlich-engine/src/sim/dive.rs:19 pub fn dive_terminal
crates/unendlich-engine/src/sim/dive.rs:29 pub fn dive_step
crates/unendlich-engine/src/sim/grounding.rs:53 pub fn slip
crates/unendlich-engine/src/sim/quad_reposition.rs:44 pub fn settle_stance
crates/unendlich-engine/src/sim/dragon_locomotion.rs:124 pub fn sprint_step
crates/unendlich-engine/src/sim/sim_scheme.rs:66 pub fn choose_scheme
crates/unendlich-engine/src/sim/fauna_mind.rs:172 pub fn recalled_threat
crates/unendlich-engine/src/sim/draw_batch.rs:54 pub fn instance_count
crates/unendlich-engine/src/sim/draw_batch.rs:64 pub fn group
crates/unendlich-engine/src/sim/draw_batch.rs:110 pub fn savings
crates/unendlich-engine/src/sim/hotbar.rs:35 pub fn verb
crates/unendlich-engine/src/sim/hotbar.rs:81 pub fn with_items
crates/unendlich-engine/src/sim/hotbar.rs:95 pub fn active_index
crates/unendlich-engine/src/sim/hotbar.rs:118 pub fn active_ability
crates/unendlich-engine/src/sim/hotbar.rs:134 pub fn assign_from
crates/unendlich-engine/src/sim/hotbar.rs:146 pub fn consume_active_one
crates/unendlich-engine/src/sim/ingame_menu.rs:42 pub fn overlay
crates/unendlich-engine/src/sim/ingame_menu.rs:49 pub fn is_paused
crates/unendlich-engine/src/sim/ingame_menu.rs:77 pub fn event
crates/unendlich-engine/src/sim/session_mode.rs:181 pub fn on_failed
crates/unendlich-engine/src/sim/session_mode.rs:187 pub fn back_to_browse
crates/unendlich-engine/src/sim/session_mode.rs:226 pub fn go_live
crates/unendlich-engine/src/sim/inventory_txn.rs:51 pub fn moved_any
crates/unendlich-engine/src/sim/inventory_txn.rs:80 pub fn quick_move
crates/unendlich-engine/src/sim/inventory_txn.rs:138 pub fn undo_quick_move
crates/unendlich-engine/src/sim/inventory_txn.rs:181 pub fn split_even
crates/unendlich-engine/src/sim/inventory_txn.rs:277 pub fn equip_swap
crates/unendlich-engine/src/sim/world_launch.rs:97 pub fn needs_generation
crates/unendlich-engine/src/sim/river_type.rs:146 pub fn flow_speed
crates/unendlich-engine/src/sim/river_type.rs:150 pub fn dragon_thirst_refill
crates/unendlich-engine/src/sim/dragon_bending.rs:117 pub fn channel_step
crates/unendlich-engine/src/sim/item_entity.rs:136 pub fn new_blade
crates/unendlich-engine/src/sim/item_entity.rs:150 pub fn new_blade_with_genes
crates/unendlich-engine/src/sim/item_entity.rs:162 pub fn launch
crates/unendlich-engine/src/sim/item_entity.rs:175 pub fn attend
crates/unendlich-engine/src/sim/landscape_memory.rs:48 pub fn observe_line
crates/unendlich-engine/src/sim/landscape_memory.rs:74 pub fn heading_familiarity
crates/unendlich-engine/src/sim/sound_masking.rs:37 pub fn can_hear
crates/unendlich-engine/src/sim/sound_masking.rs:45 pub fn effective_range
crates/unendlich-engine/src/sim/bending_fx.rs:76 pub fn blooms
crates/unendlich-engine/src/sim/bending_fx.rs:86 pub fn core_fx
crates/unendlich-engine/src/sim/bending_fx.rs:107 pub fn brine_mist_samples
crates/unendlich-engine/src/sim/bending_fx.rs:117 pub fn bloom_scale
crates/unendlich-engine/src/sim/bending_fx.rs:127 pub fn conduction_overlay
crates/unendlich-engine/src/sim/timeline.rs:77 pub fn raised
crates/unendlich-engine/src/sim/timeline.rs:84 pub fn tower
crates/unendlich-engine/src/sim/timeline.rs:191 pub fn branch_count
crates/unendlich-engine/src/sim/timeline.rs:216 pub fn timebend
crates/unendlich-engine/src/sim/timeline.rs:286 pub fn project_live_structure
crates/unendlich-engine/src/sim/timeline.rs:301 pub fn lineage
crates/unendlich-engine/src/sim/path_memory.rs:73 pub fn clearance
crates/unendlich-engine/src/sim/path_memory.rs:92 pub fn penalize
crates/unendlich-engine/src/sim/game_speed.rs:45 pub fn multiplier_to_pos
crates/unendlich-engine/src/sim/indirect_batch.rs:51 pub fn batch
crates/unendlich-engine/src/sim/main_menu.rs:86 pub fn input
crates/unendlich-engine/src/sim/sound_genome.rs:86 pub fn mimic
crates/unendlich-engine/src/sim/water_sim_core.rs:188 pub fn gather_column
crates/unendlich-engine/src/sim/sim_chain.rs:67 pub fn plan_chain
crates/unendlich-engine/src/sim/sim_chain.rs:120 pub fn merge_compression
crates/unendlich-engine/src/sim/mob_limb_girth.rs:55 pub fn exceeds_girth
crates/unendlich-engine/src/sim/gamemode_traits.rs:220 pub fn set_overlay
crates/unendlich-engine/src/sim/ore_resource_veins.rs:164 pub fn vein_density
crates/unendlich-engine/src/sim/exec_tier.rs:58 pub fn runs_on_cpu
crates/unendlich-engine/src/sim/exec_tier.rs:61 pub fn runs_on_gpu
crates/unendlich-engine/src/sim/exec_tier.rs:88 pub fn exec_tier
crates/unendlich-engine/src/sim/dragon_voice.rs:140 pub fn same_lineage_as
crates/unendlich-engine/src/sim/infusion.rs:62 pub fn infusion_capacity
crates/unendlich-engine/src/sim/infusion.rs:103 pub fn effective_affinity
crates/unendlich-engine/src/sim/infusion.rs:112 pub fn add_sway_mark
crates/unendlich-engine/src/sim/infusion.rs:123 pub fn sway_mark_count
crates/unendlich-engine/src/sim/infusion.rs:129 pub fn consume_slot
crates/unendlich-engine/src/sim/infusion.rs:140 pub fn apply_infusion
crates/unendlich-engine/src/sim/infusion.rs:153 pub fn relation_same_or_clean
crates/unendlich-engine/src/sim/unendlich_id.rs:71 pub fn to_line
crates/unendlich-engine/src/sim/unendlich_id.rs:77 pub fn from_line
crates/unendlich-engine/src/sim/time_interval_set.rs:61 pub fn from_interval
crates/unendlich-engine/src/sim/time_interval_set.rs:80 pub fn intervals
crates/unendlich-engine/src/sim/time_interval_set.rs:86 pub fn total_span
crates/unendlich-engine/src/sim/time_interval_set.rs:120 pub fn intersection
crates/unendlich-engine/src/sim/time_interval_set.rs:136 pub fn difference
crates/unendlich-engine/src/sim/water_tint.rs:14 pub fn water_albedo
crates/unendlich-engine/src/sim/ao_gpu.rs:169 pub fn bench_ao
crates/unendlich-engine/src/sim/carry.rs:76 pub fn danger
crates/unendlich-engine/src/sim/carry.rs:91 pub fn regrowth_multiplier
crates/unendlich-engine/src/sim/dragon_brain.rs:158 pub fn think
crates/unendlich-engine/src/sim/dragon_brain.rs:370 pub fn size_behavior
crates/unendlich-engine/src/sim/tutorial_ladder.rs:60 pub fn voice_shape
crates/unendlich-engine/src/sim/tutorial_ladder.rs:73 pub fn first_technique
crates/unendlich-engine/src/sim/tutorial_ladder.rs:86 pub fn mentor
crates/unendlich-engine/src/sim/tutorial_ladder.rs:250 pub fn choir_unlocked
crates/unendlich-engine/src/sim/tutorial_ladder.rs:276 pub fn run_element
crates/unendlich-engine/src/sim/phrase_learning.rs:71 pub fn degradation_loop
crates/unendlich-engine/src/sim/phrase_learning.rs:82 pub fn generations_until_lost
crates/unendlich-engine/src/sim/foveation.rs:162 pub fn bench_integrate_foveated
crates/unendlich-engine/src/sim/water_react.rs:38 pub fn displaced_volume
crates/unendlich-engine/src/sim/water_texture_batch.rs:15 pub fn caustic_texture
crates/unendlich-engine/src/sim/wing_loading.rs:34 pub fn wing_loading
crates/unendlich-engine/src/sim/wing_loading.rs:40 pub fn aspect_ratio
crates/unendlich-engine/src/sim/wing_loading.rs:60 pub fn stall_speed_sea_level
crates/unendlich-engine/src/sim/wing_loading.rs:69 pub fn max_range_speed
crates/unendlich-engine/src/sim/wing_loading.rs:74 pub fn min_power_speed
crates/unendlich-engine/src/sim/wing_loading.rs:97 pub fn needs_updraft
crates/unendlich-engine/src/sim/game_mode.rs:114 pub fn cycle
crates/unendlich-engine/src/sim/forge_types.rs:84 pub fn resonance_bonus
crates/unendlich-engine/src/sim/forge_types.rs:106 pub fn supports_combination
crates/unendlich-engine/src/sim/forge_types.rs:114 pub fn compound_infusions_solo
crates/unendlich-engine/src/sim/forge_types.rs:119 pub fn unlocks_ancient_recipes
crates/unendlich-engine/src/sim/craft_resolve.rs:113 pub fn resolve_craft
crates/unendlich-engine/src/sim/craft_resolve.rs:144 pub fn repair_estimate
crates/unendlich-engine/src/sim/craft_resolve.rs:174 pub fn market_price_crafted
crates/unendlich-engine/src/sim/menu_screen_layout.rs:45 pub fn with_value
crates/unendlich-engine/src/sim/menu_screen_layout.rs:52 pub fn display_text
crates/unendlich-engine/src/sim/menu_screen_layout.rs:92 pub fn layout_for_title
crates/unendlich-engine/src/sim/menu_screen_layout.rs:190 pub fn camera_gameplay_layout
crates/unendlich-engine/src/sim/menu_screen_layout.rs:463 pub fn main_menu_tree
crates/unendlich-engine/src/sim/craft_sway.rs:108 pub fn is_dominant
crates/unendlich-engine/src/sim/waterfall_poi.rs:119 pub fn rt60_typical
crates/unendlich-engine/src/sim/grass_field.rs:97 pub fn mean_growth
crates/unendlich-engine/src/sim/flora_genetics.rs:41 pub fn homozygous
crates/unendlich-engine/src/sim/flora_genetics.rs:115 pub fn fire_resistance
crates/unendlich-engine/src/sim/flora_genetics.rs:120 pub fn size_class
crates/unendlich-engine/src/sim/flora_genetics.rs:149 pub fn leaf_temp_rise_c
crates/unendlich-engine/src/sim/flora_genetics.rs:168 pub fn linked_trait_bonus
crates/unendlich-engine/src/sim/flora_genetics.rs:180 pub fn crypsis
crates/unendlich-engine/src/sim/flora_genetics.rs:190 pub fn biodiversity_bonus
crates/unendlich-engine/src/sim/rollback.rs:99 pub fn set_local_input
crates/unendlich-engine/src/sim/creative_tabs.rs:102 pub fn tab_items_matching
crates/unendlich-engine/src/sim/creative_tabs.rs:108 pub fn search_all
crates/unendlich-engine/src/sim/input_prediction.rs:112 pub fn get_or_predict
crates/unendlich-engine/src/sim/dht_join.rs:310 pub fn type_address
crates/unendlich-engine/src/sim/dht_join.rs:320 pub fn begin
crates/unendlich-engine/src/sim/dht_join.rs:340 pub fn on_phase
crates/unendlich-engine/src/sim/dht_join.rs:391 pub fn peers
crates/unendlich-engine/src/sim/salinity.rs:13 pub fn salinity01
crates/unendlich-engine/src/sim/water_shape.rs:43 pub fn from_state_mod
crates/unendlich-engine/src/sim/water_shape.rs:113 pub fn life_fraction
crates/unendlich-engine/src/sim/mp_dht.rs:153 pub fn should_link
crates/unendlich-engine/src/sim/net_channels.rs:146 pub fn config_for
crates/unendlich-engine/src/sim/net_channels.rs:200 pub fn budget_tick
crates/unendlich-engine/src/sim/net_channels.rs:227 pub fn total_granted
crates/unendlich-engine/src/sim/net_channels.rs:237 pub fn keepalive_due
crates/unendlich-engine/src/sim/batch_settle.rs:44 pub fn settle_one
crates/unendlich-engine/src/sim/combat_resolve.rs:106 pub fn resolve_full
crates/unendlich-engine/src/sim/personality.rs:56 pub fn confronts
crates/unendlich-engine/src/sim/breath_attack.rs:73 pub fn try_breathe
crates/unendlich-engine/src/sim/fall.rs:47 pub fn flee_step_avoiding_cliffs
crates/unendlich-engine/src/sim/batch_perception.rs:58 pub fn perceive_one
crates/unendlich-engine/src/sim/batch_perception.rs:63 pub fn perceive_batch
crates/unendlich-engine/src/sim/title_cinematic.rs:247 pub fn dragon_genome
crates/unendlich-engine/src/sim/title_cinematic.rs:278 pub fn pose
crates/unendlich-engine/src/sim/title_cinematic.rs:285 pub fn as_path
crates/unendlich-engine/src/sim/title_cinematic.rs:336 pub fn vignette_and_framing
crates/unendlich-engine/src/sim/sound_stereo.rs:36 pub fn stereo
crates/unendlich-engine/src/sim/head_steer.rs:97 pub fn world_yaw
crates/unendlich-engine/src/sim/head_steer.rs:105 pub fn pull_body
crates/unendlich-engine/src/sim/region_digest.rs:85 pub fn digest_fixed
crates/unendlich-engine/src/sim/region_digest.rs:93 pub fn converged
crates/unendlich-engine/src/sim/pylon.rs:140 pub fn effects_from
crates/unendlich-engine/src/sim/crafting.rs:228 pub fn matching_index
crates/unendlich-engine/src/sim/player_rank.rs:69 pub fn progress_to_next
crates/unendlich-engine/src/sim/menu_controller.rs:90 pub fn settings
crates/unendlich-engine/src/sim/menu_controller.rs:93 pub fn settings_mut
crates/unendlich-engine/src/sim/menu_controller.rs:96 pub fn browser
crates/unendlich-engine/src/sim/menu_controller.rs:99 pub fn browser_mut
crates/unendlich-engine/src/sim/fixed.rs:82 pub fn to_f64
crates/unendlich-engine/src/sim/flight_modes.rs:46 pub fn power_factor
crates/unendlich-engine/src/sim/flight_modes.rs:60 pub fn is_soaring
crates/unendlich-engine/src/sim/flight_modes.rs:100 pub fn generic
crates/unendlich-engine/src/sim/flight_modes.rs:104 pub fn small_bird
crates/unendlich-engine/src/sim/flight_modes.rs:108 pub fn soarer
crates/unendlich-engine/src/sim/flight_modes.rs:139 pub fn select_mode
crates/unendlich-engine/src/sim/column_memo.rs:50 pub fn get_or
crates/unendlich-engine/src/sim/touch_rays.rs:74 pub fn feel_into
crates/unendlich-engine/src/sim/volumetric_gpu.rs:114 pub fn bench_integrate
crates/unendlich-engine/src/sim/ui_state.rs:67 pub fn pauses_game
crates/unendlich-engine/src/sim/ui_state.rs:74 pub fn shows_inventory
crates/unendlich-engine/src/sim/ui_state.rs:79 pub fn shows_pause_menu
crates/unendlich-engine/src/sim/ui_state.rs:84 pub fn shows_settings
crates/unendlich-engine/src/sim/ui_state.rs:89 pub fn shows_debug
crates/unendlich-engine/src/sim/ui_state.rs:94 pub fn shows_craft
crates/unendlich-engine/src/sim/ui_state.rs:114 pub fn on_escape
crates/unendlich-engine/src/sim/cave_dragon.rs:104 pub fn rt60_s
crates/unendlich-engine/src/sim/cave_dragon.rs:130 pub fn is_hostile
crates/unendlich-engine/src/sim/cave_dragon.rs:138 pub fn enemy_detect_range_blocks
crates/unendlich-engine/src/sim/cave_dragon.rs:170 pub fn habitability
crates/unendlich-engine/src/sim/dragon_territory.rs:99 pub fn defends
crates/unendlich-engine/src/sim/dragon_territory.rs:129 pub fn territories_conflict
crates/unendlich-engine/src/sim/sub_discipline.rs:18 pub fn sub_discipline_unlocked
crates/unendlich-engine/src/sim/sub_discipline.rs:96 pub fn renown_tier
crates/unendlich-engine/src/sim/sub_discipline.rs:114 pub fn display_title
crates/unendlich-engine/src/sim/elemental.rs:181 pub fn record_deed
crates/unendlich-engine/src/sim/elemental.rs:231 pub fn evolve
crates/unendlich-engine/src/sim/skill_constellation.rs:59 pub fn add_node
crates/unendlich-engine/src/sim/skill_constellation.rs:88 pub fn is_available
crates/unendlich-engine/src/sim/skill_constellation.rs:122 pub fn total_spent
crates/unendlich-engine/src/sim/skill_constellation.rs:125 pub fn unlocked_count
crates/unendlich-engine/src/sim/skill_constellation.rs:139 pub fn can_eventually_reach
crates/unendlich-engine/src/sim/fauna_roster.rs:106 pub fn habitat_weights
crates/unendlich-engine/src/sim/dragon_population.rs:50 pub fn next_generation_clutched
crates/unendlich-engine/src/sim/dragon_population.rs:101 pub fn next_generation_clutched_compat
crates/unendlich-engine/src/sim/resonance.rs:40 pub fn item_resonance
crates/unendlich-engine/src/sim/resonance.rs:49 pub fn resonance_rating
crates/unendlich-engine/src/sim/resonance.rs:62 pub fn mitigate
crates/unendlich-engine/src/sim/legendary_loot.rs:68 pub fn ungated
crates/unendlich-engine/src/sim/fog_model.rs:51 pub fn transmittance_from_tau
crates/unendlich-engine/src/sim/fog_model.rs:80 pub fn height_density
crates/unendlich-engine/src/sim/fog_model.rs:90 pub fn sea_level_fade
crates/unendlich-engine/src/sim/fog_model.rs:107 pub fn airmass_exponential
crates/unendlich-engine/src/sim/fog_model.rs:134 pub fn rayleigh_phase
crates/unendlich-engine/src/sim/fog_model.rs:150 pub fn cornette_shanks_phase
crates/unendlich-engine/src/sim/fog_model.rs:161 pub fn double_lobe_hg
crates/unendlich-engine/src/sim/fog_model.rs:170 pub fn multi_scatter_hg
crates/unendlich-engine/src/sim/fog_model.rs:207 pub fn apply_water_multiscatter
crates/unendlich-engine/src/sim/perception_fog.rs:188 pub fn bench_perceive
crates/unendlich-engine/src/sim/foliage_sway.rs:84 pub fn is_eligible
crates/unendlich-engine/src/sim/foliage_sway.rs:166 pub fn unpack_tag
crates/unendlich-engine/src/sim/foliage_sway.rs:175 pub fn tag_unorm
crates/unendlich-engine/src/sim/foliage_sway.rs:182 pub fn tag_from_unorm
crates/unendlich-engine/src/sim/foliage_sway.rs:216 pub fn sway_displacement
crates/unendlich-engine/src/sim/panel_tabs.rs:94 pub fn next_enabled
crates/unendlich-engine/src/sim/panel_tabs.rs:99 pub fn prev_enabled
crates/unendlich-engine/src/sim/sim_vector.rs:137 pub fn end_tick
crates/unendlich-engine/src/sim/sim_vector.rs:144 pub fn stage_seed
crates/unendlich-engine/src/sim/sim_vector.rs:149 pub fn push_correction
crates/unendlich-engine/src/sim/sim_vector.rs:154 pub fn push_event
crates/unendlich-engine/src/sim/articulation.rs:60 pub fn children
crates/unendlich-engine/src/sim/articulation.rs:86 pub fn total_curl
crates/unendlich-engine/src/sim/wing_buffet.rs:16 pub fn buffet_power
crates/unendlich-engine/src/sim/wing_buffet.rs:51 pub fn buffet_targets
crates/unendlich-engine/src/sim/elements.rs:60 pub fn has_fire
crates/unendlich-engine/src/sim/elements.rs:63 pub fn has_ice
crates/unendlich-engine/src/sim/elements.rs:66 pub fn has_wind
crates/unendlich-engine/src/sim/armor_set.rs:71 pub fn equip
crates/unendlich-engine/src/sim/armor_set.rs:74 pub fn unequip
crates/unendlich-engine/src/sim/armor_set.rs:112 pub fn aggregate
crates/unendlich-engine/src/sim/item_brain.rs:139 pub fn socket
crates/unendlich-engine/src/sim/item_brain.rs:255 pub fn roll_material
crates/unendlich-engine/src/sim/item_breeding.rs:37 pub fn all_round
crates/unendlich-engine/src/sim/item_breeding.rs:41 pub fn damage_only
crates/unendlich-engine/src/sim/frame_correlation.rs:212 pub fn spike_count
crates/unendlich-engine/src/sim/frame_correlation.rs:216 pub fn frame_count
crates/unendlich-engine/src/sim/player_views.rs:75 pub fn with_anim_secs
crates/unendlich-engine/src/sim/player_views.rs:80 pub fn player_count
crates/unendlich-engine/src/sim/player_views.rs:84 pub fn views
crates/unendlich-engine/src/sim/player_views.rs:87 pub fn ids
crates/unendlich-engine/src/sim/player_views.rs:93 pub fn viewport_of
crates/unendlich-engine/src/sim/player_views.rs:100 pub fn add_player
crates/unendlich-engine/src/sim/player_views.rs:120 pub fn remove_player
crates/unendlich-engine/src/sim/player_views.rs:133 pub fn set_aspect
crates/unendlich-engine/src/sim/player_views.rs:208 pub fn compute_overlap
crates/unendlich-engine/src/sim/bending_compose.rs:233 pub fn build_resonance_focused
crates/unendlich-engine/src/sim/creative_items.rs:105 pub fn tab_counts
crates/unendlich-engine/src/sim/poi.rs:112 pub fn dominant_attractor
crates/unendlich-engine/src/sim/settings_model.rs:55 pub fn as_u32
crates/unendlich-engine/src/sim/settings_model.rs:127 pub fn upscale_factor
crates/unendlich-engine/src/sim/settings_model.rs:133 pub fn upscale_mode
crates/unendlich-engine/src/sim/settings_model.rs:242 pub fn draft
crates/unendlich-engine/src/sim/settings_model.rs:247 pub fn committed
crates/unendlich-engine/src/sim/settings_model.rs:262 pub fn revert
crates/unendlich-engine/src/sim/settings_model.rs:267 pub fn dirty
crates/unendlich-engine/src/sim/concealment.rs:43 pub fn detectability
crates/unendlich-engine/src/sim/turning.rs:89 pub fn turn_radius
crates/unendlich-engine/src/sim/turning.rs:98 pub fn ease_yaw_g_limited
crates/unendlich-engine/src/sim/survival.rs:40 pub fn comfortable
crates/unendlich-engine/src/sim/companion_roster.rs:94 pub fn deepen
crates/unendlich-engine/src/sim/companion_roster.rs:151 pub fn total_abilities
crates/unendlich-engine/src/sim/turtle.rs:34 pub fn next_state
crates/unendlich-engine/src/sim/turtle.rs:54 pub fn speed_mult
crates/unendlich-engine/src/sim/world_prober.rs:154 pub fn surface_hit_rate
crates/unendlich-engine/src/sim/world_prober.rs:160 pub fn biome_hit_rate
crates/unendlich-engine/src/sim/world_prober.rs:166 pub fn columns_cached
crates/unendlich-engine/src/sim/buoyancy.rs:197 pub fn buoyant_force
crates/unendlich-engine/src/sim/buoyancy.rs:210 pub fn swim_step
crates/unendlich-engine/src/sim/buoyancy.rs:253 pub fn flow_push
crates/unendlich-engine/src/sim/buoyancy.rs:262 pub fn head_above_water
crates/unendlich-engine/src/sim/buoyancy.rs:305 pub fn swim_vertical_step
crates/unendlich-engine/src/sim/dragon_blueprint_from_genome.rs:173 pub fn dragon_blueprint_grown
crates/unendlich-engine/src/sim/ability_bar.rs:92 pub fn bound_count
crates/unendlich-engine/src/sim/ability_bar.rs:97 pub fn bound_techniques
crates/unendlich-engine/src/sim/ability_bar.rs:105 pub fn usable
crates/unendlich-engine/src/sim/lauscher.rs:102 pub fn consider_lair
crates/unendlich-engine/src/sim/bending_information.rs:118 pub fn disrupt
crates/unendlich-engine/src/sim/bending_information.rs:130 pub fn mask
crates/unendlich-engine/src/sim/dragon_signature.rs:93 pub fn signature_from
crates/unendlich-engine/src/sim/menu_nav.rs:34 pub fn action
crates/unendlich-engine/src/sim/menu_nav.rs:37 pub fn submenu
crates/unendlich-engine/src/sim/menu_nav.rs:113 pub fn move_focus
crates/unendlich-engine/src/sim/menu_nav.rs:140 pub fn activate
crates/unendlich-engine/src/sim/inventory_transfer.rs:40 pub fn transfer_all
crates/unendlich-engine/src/sim/inventory_transfer.rs:50 pub fn transfer_matching
crates/unendlich-engine/src/sim/portal_graph.rs:57 pub fn add_portal
crates/unendlich-engine/src/sim/portal_graph.rs:78 pub fn link
crates/unendlich-engine/src/sim/portal_graph.rs:113 pub fn remove_portal
crates/unendlich-engine/src/sim/portal_graph.rs:121 pub fn traverse
crates/unendlich-engine/src/sim/portal_graph.rs:136 pub fn nearest_in_dim
crates/unendlich-engine/src/sim/portal_graph.rs:150 pub fn dimensions
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
crates/unendlich-engine/src/sim/plant_survival.rs:95 pub fn may_grow_unorthodox
crates/unendlich-engine/src/sim/plant_survival.rs:127 pub fn support_offset
crates/unendlich-engine/src/sim/plant_survival.rs:131 pub fn is_lateral
crates/unendlich-engine/src/sim/plant_survival.rs:140 pub fn permits
crates/unendlich-engine/src/sim/plant_survival.rs:152 pub fn water_rot_increment
crates/unendlich-engine/src/sim/plant_survival.rs:168 pub fn energy_balance
crates/unendlich-engine/src/sim/plant_survival.rs:174 pub fn accumulate_starvation
crates/unendlich-engine/src/sim/plant_survival.rs:228 pub fn sway_offset
crates/unendlich-engine/src/sim/plant_survival.rs:251 pub fn trunk_radius_from_gene
crates/unendlich-engine/src/sim/water_sight.rs:55 pub fn water_path
crates/unendlich-engine/src/sim/water_sight.rs:147 pub fn scatter_detection
crates/unendlich-engine/src/sim/water_sight.rs:186 pub fn perceive_through_water
crates/unendlich-engine/src/sim/forage_memory.rs:29 pub fn known_count
crates/unendlich-engine/src/sim/forage_memory.rs:58 pub fn forget_stale
crates/unendlich-engine/src/sim/dragon_forge.rs:1107 pub fn skin_with_flex
crates/unendlich-engine/src/sim/dragon_forge.rs:1730 pub fn place_surface_glow
crates/unendlich-engine/src/sim/dragon_forge.rs:1775 pub fn scale_model
crates/unendlich-engine/src/sim/dragon_forge.rs:1807 pub fn build_grown
crates/unendlich-engine/src/sim/menu_router.rs:113 pub fn stack
crates/unendlich-engine/src/sim/menu_router.rs:149 pub fn pop_forced
crates/unendlich-engine/src/sim/menu_router.rs:189 pub fn replace_forced
crates/unendlich-engine/src/sim/menu_router.rs:210 pub fn reset_to_root
crates/unendlich-engine/src/sim/menu_router.rs:511 pub fn from_id
crates/unendlich-engine/src/sim/menu_router.rs:570 pub fn from_id
crates/unendlich-engine/src/sim/menu_router.rs:597 pub fn current_page
crates/unendlich-engine/src/sim/menu_router.rs:613 pub fn move_focus
crates/unendlich-engine/src/sim/menu_router.rs:621 pub fn activate
crates/unendlich-engine/src/sim/menu_router.rs:656 pub fn pages
crates/unendlich-engine/src/sim/stamina.rs:40 pub fn can_sprint
crates/unendlich-engine/src/sim/acoustics.rs:28 pub fn l2_normalize
crates/unendlich-engine/src/sim/acoustics.rs:142 pub fn effective_hearing
crates/unendlich-engine/src/sim/acoustics.rs:162 pub fn spatial_impression
crates/unendlich-engine/src/sim/roamer_cap.rs:65 pub fn overflow_count
crates/unendlich-engine/src/sim/neighbor_grid.rs:71 pub fn candidates
crates/unendlich-engine/src/sim/ragdoll.rs:225 pub fn from_creature
crates/unendlich-engine/src/sim/lightning.rs:58 pub fn arc_path
crates/unendlich-engine/src/sim/lightning.rs:121 pub fn arc_tree
crates/unendlich-engine/src/sim/bending_runtime.rs:117 pub fn update_from_input
crates/unendlich-engine/src/sim/body_scale.rs:29 pub fn dragon
crates/unendlich-engine/src/sim/body_scale.rs:36 pub fn sight_range
crates/unendlich-engine/src/sim/body_scale.rs:41 pub fn probe_span
crates/unendlich-engine/src/sim/body_scale.rs:47 pub fn path_tile
crates/unendlich-engine/src/sim/net/chunk_stream.rs:57 pub fn select_plan
crates/unendlich-engine/src/sim/net/chunk_stream.rs:91 pub fn coord
crates/unendlich-engine/src/sim/net/chunk_stream.rs:147 pub fn encode_record
crates/unendlich-engine/src/sim/net/chunk_stream.rs:178 pub fn decode_record
crates/unendlich-engine/src/sim/net/dht_mailbox.rs:181 pub fn send_request
crates/unendlich-engine/src/sim/net/dht_mailbox.rs:196 pub fn poll_response
crates/unendlich-engine/src/sim/net/dht_mailbox.rs:216 pub fn host_drain_requests
crates/unendlich-engine/src/sim/net/dht_mailbox.rs:245 pub fn host_accept
crates/unendlich-engine/src/sim/net/dht_mailbox.rs:266 pub fn host_deny
crates/unendlich-engine/src/sim/net/client_reconcile.rs:72 pub fn predicted
crates/unendlich-engine/src/sim/net/identity.rs:62 pub fn grouped
crates/unendlich-engine/src/sim/net/identity.rs:167 pub fn name_for
crates/unendlich-engine/src/sim/net/identity.rs:233 pub fn address_if_accepted
crates/unendlich-engine/src/sim/net/lag_compensation.rs:180 pub fn rewind
crates/unendlich-engine/src/sim/net/lag_compensation.rs:187 pub fn rewind_all
crates/unendlich-engine/src/sim/net/lag_compensation.rs:200 pub fn view_time
crates/unendlich-engine/src/sim/net/lag_compensation.rs:246 pub fn resolve_shot
crates/unendlich-engine/src/sim/net/rendezvous_join.rs:29 pub fn parse_candidate
crates/unendlich-engine/src/sim/net/world_advert.rs:85 pub fn encode_advert
crates/unendlich-engine/src/sim/net/world_advert.rs:106 pub fn decode_advert
crates/unendlich-engine/src/sim/net/presence_loop.rs:83 pub fn evict_dead_default
crates/unendlich-engine/src/sim/net/codec.rs:54 pub fn is_at_end
crates/unendlich-engine/src/sim/net/codec.rs:108 pub fn check_header
crates/unendlich-engine/src/sim/net/codec.rs:120 pub fn read_str
crates/unendlich-engine/src/sim/net/codec.rs:131 pub fn read_vec
crates/unendlich-engine/src/sim/net/codec.rs:236 pub fn write_vec
crates/unendlich-engine/src/sim/net/codec.rs:263 pub fn encode_into
crates/unendlich-engine/src/sim/net/chunk_pool.rs:92 pub fn set_worker
crates/unendlich-engine/src/sim/net/chunk_pool.rs:100 pub fn remove_worker
crates/unendlich-engine/src/sim/net/chunk_pool.rs:113 pub fn worker_count
crates/unendlich-engine/src/sim/net/chunk_pool.rs:117 pub fn worker
crates/unendlich-engine/src/sim/net/chunk_pool.rs:139 pub fn reprioritize
crates/unendlich-engine/src/sim/net/chunk_pool.rs:228 pub fn reap_timeouts
crates/unendlich-engine/src/sim/net/chunk_pool.rs:249 pub fn in_flight_count
crates/unendlich-engine/src/sim/net/chunk_pool.rs:252 pub fn done_count
crates/unendlich-engine/src/sim/net/chunk_pool.rs:255 pub fn state_of
crates/unendlich-engine/src/sim/net/chunk_compress.rs:88 pub fn seal_dense
crates/unendlich-engine/src/sim/net/chunk_compress.rs:94 pub fn seal_delta
crates/unendlich-engine/src/sim/net/client_join.rs:80 pub fn assigned_name
crates/unendlich-engine/src/sim/net/player_roster.rs:102 pub fn leave
crates/unendlich-engine/src/sim/net/player_roster.rs:113 pub fn disconnect
crates/unendlich-engine/src/sim/net/player_roster.rs:118 pub fn reconnect
crates/unendlich-engine/src/sim/net/player_roster.rs:140 pub fn set_role
crates/unendlich-engine/src/sim/net/player_roster.rs:153 pub fn by_slot
crates/unendlich-engine/src/sim/net/player_roster.rs:179 pub fn privileged
crates/unendlich-engine/src/sim/net/world_presence.rs:88 pub fn prune_stale
crates/unendlich-engine/src/sim/net/entity_snapshot.rs:227 pub fn tracked
crates/unendlich-engine/src/sim/net/dht.rs:49 pub fn self_id
crates/unendlich-engine/src/sim/net/session.rs:69 pub fn record_local
crates/unendlich-engine/src/sim/net/session.rs:76 pub fn outgoing_input_packet
crates/unendlich-engine/src/sim/net/session.rs:89 pub fn outgoing_sync_packet
crates/unendlich-engine/src/sim/net/session.rs:95 pub fn note_confirmed
crates/unendlich-engine/src/sim/net/session.rs:139 pub fn remote_ack
crates/unendlich-engine/src/sim/net/session.rs:144 pub fn desync
crates/unendlich-engine/src/sim/net/world_cmd_wire.rs:69 pub fn encode_cmd
crates/unendlich-engine/src/sim/net/world_cmd_wire.rs:146 pub fn decode_cmd
crates/unendlich-engine/src/sim/net/world_cmd_wire.rs:157 pub fn apply_remote
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
crates/unendlich-engine/src/phys/rope.rs:123 pub fn stretch_error
crates/unendlich-engine/src/phys/fluid_forces.rs:15 pub fn submersion_fraction
crates/unendlich-engine/src/phys/fluid_forces.rs:59 pub fn fluid_velocity_delta
crates/unendlich-engine/src/phys/fluid_forces.rs:83 pub fn swim_control_accel
crates/unendlich-engine/src/phys/hydro.rs:23 pub fn hydrostatic_pressure
crates/unendlich-engine/src/phys/hydro.rs:39 pub fn orifice_flow_rate
crates/unendlich-engine/src/phys/hydro.rs:50 pub fn head_driven_flow
crates/unendlich-engine/src/phys/hydro.rs:59 pub fn equalized_surface
crates/unendlich-engine/src/phys/acoustics.rs:21 pub fn propagation_delay
crates/unendlich-engine/src/phys/acoustics.rs:68 pub fn box_room_rt60
crates/unendlich-engine/src/render/cascade_fit_pipeline.rs:301 pub fn cascade_fit_cpu_reference
crates/unendlich-engine/src/render/q8.rs:36 pub fn quantize_scalar_reference
crates/unendlich-engine/src/render/q8.rs:119 pub fn dequant
crates/unendlich-engine/src/render/q8.rs:177 pub fn quantize_q4_block_experimental
crates/unendlich-engine/src/render/q8.rs:192 pub fn dequant_q4_experimental
crates/unendlich-engine/src/render/mesher.rs:854 pub fn unpack_color
crates/unendlich-engine/src/render/mesher.rs:881 pub fn sway_flag
crates/unendlich-engine/src/render/mesher.rs:891 pub fn vertex_sway_tag
crates/unendlich-engine/src/render/mesher.rs:901 pub fn tag_packed_pos
crates/unendlich-engine/src/render/mesher.rs:1304 pub fn mesh_chunk_far
crates/unendlich-engine/src/render/mesher.rs:1314 pub fn mesh_chunk_very_far
crates/unendlich-engine/src/render/egui_overlay.rs:766 pub fn register_external_texture
crates/unendlich-engine/src/render/neural_train.rs:393 pub fn try_take_weights
crates/unendlich-engine/src/render/postfx.rs:143 pub fn scene_target
crates/unendlich-engine/src/render/postfx.rs:151 pub fn scene_render_pass
crates/unendlich-engine/src/render/postfx.rs:159 pub fn present_render_pass
crates/unendlich-engine/src/render/postfx.rs:168 pub fn begin_present_copy
crates/unendlich-engine/src/render/postfx.rs:229 pub fn end_present
crates/unendlich-engine/src/render/mesh_prims.rs:79 pub fn normal_f32
crates/unendlich-engine/src/render/mesh_prims.rs:92 pub fn offset_i8
crates/unendlich-engine/src/render/mesh_prims.rs:103 pub fn face_from_offset
crates/unendlich-engine/src/render/mesh_prims.rs:180 pub fn quad_aabbs_parallel
crates/unendlich-engine/src/render/gpu_pack.rs:70 pub fn free_slot
crates/unendlich-engine/src/render/gpu_pack.rs:184 pub fn pack_bits
crates/unendlich-engine/src/render/gpu_pack.rs:206 pub fn unpack_bits
crates/unendlich-engine/src/render/meshing_pool.rs:461 pub fn queue_capacity
crates/unendlich-engine/src/render/meshing_pool.rs:464 pub fn pending_requests
crates/unendlich-engine/src/render/segmented_alloc.rs:113 pub fn segment_count
crates/unendlich-engine/src/render/segmented_alloc.rs:115 pub fn max_segments
crates/unendlich-engine/src/render/segmented_alloc.rs:121 pub fn segment_capacities
crates/unendlich-engine/src/render/gpu_meshing_octree_cpu_ref.rs:254 pub fn mesh_chunk_octree_cpu_reference
crates/unendlich-engine/src/render/mesh_buffer_alloc.rs:129 pub fn allocation_count
crates/unendlich-engine/src/render/mesh_buffer_alloc.rs:130 pub fn free_span_count
crates/unendlich-engine/src/render/mesh_buffer_alloc.rs:221 pub fn check_invariants
crates/unendlich-engine/src/render/gpu_timer.rs:538 pub fn skipped_frames
crates/unendlich-engine/src/render/copy_coalesce.rs:60 pub fn coalesce_in_place
crates/unendlich-engine/src/render/vulkan_state_model.rs:116 pub fn run_render_pass
crates/unendlich-engine/src/render/mesh_buffer_pool.rs:149 pub fn vert_free_bytes
crates/unendlich-engine/src/render/mesh_buffer_pool.rs:150 pub fn idx_free_bytes
crates/unendlich-engine/src/render/mesh_buffer_pool.rs:872 pub fn is_device_local
crates/unendlich-engine/src/render/view_params.rs:275 pub fn write_planes
crates/unendlich-engine/src/render/world_renderer.rs:2364 pub fn mesh_cache_hits
crates/unendlich-engine/src/render/world_renderer.rs:2370 pub fn mesh_cache_misses
crates/unendlich-engine/src/render/world_renderer.rs:2375 pub fn mesh_cache_size
crates/unendlich-engine/src/render/world_renderer.rs:2736 pub fn mesh_or_empty_version_for
crates/unendlich-engine/src/render/world_renderer.rs:2755 pub fn mesh_version_for_lod
crates/unendlich-engine/src/render/world_renderer.rs:3163 pub fn draw_frame_with_view
crates/unendlich-engine/src/render/world_renderer.rs:6856 pub fn mesh_vert_largest_free
crates/unendlich-engine/src/render/world_renderer.rs:6857 pub fn mesh_idx_largest_free
crates/unendlich-engine/src/render/adaptive_upload.rs:99 pub fn with_thresholds
crates/unendlich-engine/src/render/adaptive_upload.rs:107 pub fn with_cap_range
crates/unendlich-engine/src/render/adaptive_upload.rs:118 pub fn with_smoothing
crates/unendlich-engine/src/items/inventory.rs:213 pub fn move_into
crates/unendlich-engine/src/items/debug_ui.rs:47 pub fn debug_inventory_ui
crates/unendlich-engine/src/items/debug_ui.rs:125 pub fn debug_inventory_window
crates/unendlich-engine/src/items/inventory_filter.rs:120 pub fn filter_indices
crates/unendlich-engine/src/items/inventory_filter.rs:135 pub fn category_counts
crates/unendlich-engine/src/items/recipe_build.rs:42 pub fn to_recipe_book
crates/unendlich-engine/src/items/recipe_build.rs:52 pub fn standard_book
crates/unendlich-engine/src/items/recipe_build.rs:108 pub fn elemental_stave
crates/unendlich-engine/src/items/recipe_build.rs:113 pub fn living_weapon
crates/unendlich-engine/src/items/recipe_build.rs:118 pub fn parkour_grippers
crates/unendlich-engine/src/items/debug_hud.rs:41 pub fn build_hotbar_markers
crates/unendlich-engine/src/items/debug_hud.rs:96 pub fn hotbar_summary
crates/unendlich-engine/src/items/crafting.rs:155 pub fn craft_shaped
crates/unendlich-engine/src/items/crafting.rs:205 pub fn first_craftable
crates/unendlich-engine/src/items/crafting.rs:209 pub fn craftable
crates/unendlich-engine/src/items/crafting.rs:215 pub fn match_shaped
crates/unendlich-engine/src/items/recipe_gen.rs:84 pub fn planks
crates/unendlich-engine/src/items/recipe_gen.rs:89 pub fn sticks
crates/unendlich-engine/src/items/recipe_gen.rs:94 pub fn slab
crates/unendlich-engine/src/items/recipe_gen.rs:99 pub fn stairs
crates/unendlich-engine/src/items/drops.rs:102 pub fn apply_water

## A2 zero-callsite clusters (by family)  (9)
sim: 637 fns
world: 197 fns
(root): 140 fns
render: 52 fns
items: 21 fns
util: 20 fns
phys: 10 fns
app: 8 fns
telemetry: 7 fns

## B discarded values (bin)  (55)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:670 let _render_probe = unendlich_engine::thread_probe::register("render");
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:750 let _ = (sim_distance, prewarm_budget); // consumed by the prewarm loop wiring
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:761 let _ = column_cache_cap; // consumed by the generator wiring
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:774 let _ = gen_falloff_floor; // consumed by the loader budget wiring
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:1371 let _ = std::env::var("UNENDLICH_GPU_MESHER"); // keep the env read side-effect-free / avo
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2921 let _ = s.set_nonblocking(true); // per-frame recv must not block the loop
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:3784 let _ = whitelist.save(&flora_root);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:3923 let _ = std::fs::write(
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:3978 let _ = fg_app.tick(
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:3993 let _ = fg_app.tick(unendlich_engine::app::AppInput::Resume, 0);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:4012 let _ = fg_app.tick(unendlich_engine::app::AppInput::QuitToTitle, 0);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:4020 let _ = fg_app.tick(unendlich_engine::app::AppInput::Pause, 0);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:4024 let _ = fg_app.tick(unendlich_engine::app::AppInput::Resume, 0);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:4521 let _b = chunk_manager.generator().cpu().biome_at_3d(sx as i32, sy as i32, sz as i32).biom
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:4522 let _roll = ((wseed >> 16) & 0xffff) as f32 / 65535.0;
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:4523 let _s = unendlich_engine::sim::fauna_roster::pick_for_biome(_b, _roll);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:4831 let _ =
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:4857 let _ =
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:5483 let _b = chunk_manager.generator().cpu().biome_at_3d(sx as i32, sy as i32, sz as i32).biom
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:5484 let _roll = ((wseed >> 16) & 0xffff) as f32 / 65535.0;
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:5485 let _s = unendlich_engine::sim::fauna_roster::pick_for_biome(_b, _roll);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:5565 let _ = std::hint::black_box(
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:5572 let _ = std::hint::black_box(
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:5579 let _ = std::hint::black_box(
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:5589 let _ = std::hint::black_box(
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:5601 let _ = std::hint::black_box(
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:5612 let _ = std::hint::black_box(
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:5680 let _stam_mult = fg_app.stamina_mut().tick(sprint_urge, dt);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:6356 let _ = chunk_manager.set_block(bx, sy0 + 1, bz, BlockId::Water);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:7591 let _t_roamer = Instant::now();
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:8209 let _t_bs = Instant::now();
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:8366 let _t_hs = Instant::now();
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:9585 let _ = fg_app.inventory_mut().remove_item(wid, 1);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:9843 let _ = fg_app.inventory_mut().remove_item(wid, 1);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:10150 let _ = fg_app.inventory_mut().remove_item(unendlich_engine::items::ItemId(selected_block 
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:10204 let _ = fg_app.inventory_mut().remove_item(
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:10291 let _ = fg_app.inventory_mut().remove_item(unendlich_engine::items::ItemId(selected_block 
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:10340 let _ = fg_app.inventory_mut().remove_item(
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:10641 let _ = player_chunk_changed;
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:11134 let _ = renderer.upload_chunk_octree_versioned(
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:11155 let _ = submitted; // available for future debug logging
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:11464 let _ = uploaded_this_frame;
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:12000 let _t_cmesh = Instant::now();
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:13479 let _ = eroded;
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:13595 let _ = chunk_manager.set_block(wx, wy, wz, BlockId::Air);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:13805 let _ = fg_app.tick(AppInput::WizardSetName("New World".into()), 0);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:13812 let _ = fg_app.tick(AppInput::WizardSetSeed(SeedChoice::Fixed(pv)), 0);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:13927 let _ = session.flora.save(&flora_root);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:13928 let _ = whitelist.save(&flora_root);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:14143 let _ = sock.send_to(&dg, src);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:14160 let _ = sock.send_to(&dg, src);
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:14347 let _ = sock.send_to(&dg, std::net::SocketAddr::V4(peer));
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:14357 let _ = sock.send_to(&dg, std::net::SocketAddr::V4(peer));
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:14375 let _ = sock.send_to(&dg, std::net::SocketAddr::V4(peer));
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:14839 let _ = whitelist.save(&flora_root);

## C TODO/stub markers  (61)
crates/unendlich-engine/src/water_field.rs:47 ...on). Skeleton //!   only with TODO comments. Coming in a follow-up session. //! ...
crates/unendlich-engine/src/water_field.rs:199 ...tal);     }      /// The tick stub is a no-op today: total volume must be unchan...
crates/unendlich-engine/src/water_field.rs:202 .../// For now it verifies the stub doesn't accidentally lose mass.     #[test]    ...
crates/unendlich-engine/src/water_field.rs:212 ...s() < 1e-6,             "tick stub leaked volume: before={}, after={}", before, ...
crates/unendlich-engine/src/creature_design.rs:813 ...tails vary a lot in length: a stub of 4 up to a long 10-segment         // whip ...
crates/unendlich-engine/src/creature.rs:888 ...- 1e-4);     }      /// Tiny stub world for collision tests: a single solid bloc...
crates/unendlich-engine/src/util/batch_kernel.rs:19 ...stay bit-identical. //!   // TODO fold into `util::mathx` once r330-refactor-mat...
crates/unendlich-engine/src/util/batch_kernel.rs:55 ...cal copies on every input. // TODO fold into `util::mathx::nonneg` on r330-refac...
crates/unendlich-engine/src/app/menu_view.rs:309 ...lands on a deterministic /// placeholder `Fixed(0)` the seed-entry step then let...
crates/unendlich-engine/src/world/plant_lifecycle.rs:215 ...AL_TICKS` ticks (the previous placeholder). /// /// The dirty flag is set by `Ch...
crates/unendlich-engine/src/world/plant_lifecycle.rs:228 ...tructural-dirty: replaces the placeholder     // POI_REFRESH_INTERVAL_TICKS cade...
crates/unendlich-engine/src/world/plant_lifecycle.rs:767 ...// ticks (the previous placeholder cadence).             //             // struc...
crates/unendlich-engine/src/world/octree_scheduler.rs:60 ...land — for v0.24.23 we have a placeholder /// catalog covering the obvious cases...
crates/unendlich-engine/src/world/chunk_manager.rs:50 ...ne of the conversion helpers (TODO: add when /// the first caller migrates). #[d...
crates/unendlich-engine/src/world/gpu_chunk_pool.rs:32 ...n-backed pool or a CPU-shadow stub. The CPU stub exists to //! keep the unit tes...
crates/unendlich-engine/src/world/gpu_chunk_pool.rs:32 ...or a CPU-shadow stub. The CPU stub exists to //! keep the unit tests independent...
crates/unendlich-engine/src/world/weathering.rs:15 ...vironment-driven equilibrium (TODO, //!     phase 6+ when caves and water flow a...
crates/unendlich-engine/src/world/world_model.rs:372 ...lightly     /// higher seas — placeholder for a real water-cycle later).     pub...
crates/unendlich-engine/src/world/entity.rs:867 ...and /// condition values are placeholders pending playtest tuning — /// they're ...
crates/unendlich-engine/src/world/entity.rs:1218 ...ather than sparse bones (docs/TODO.md #6). One triangle fan per side across     ...
crates/unendlich-engine/src/world/chunk.rs:147 .../// The 10-tick cadence is a placeholder until structural dirty     /// marking ...
crates/unendlich-engine/src/world/chunk.rs:191 .../// **The end-state of the placeholder cadence in     /// `POI_REFRESH_INTERVAL_...
crates/unendlich-engine/src/world/blocks.rs:155 ...llision()`, `blocks_light()` (TODO),     /// `produces_face_against(neighbor)` (...
crates/unendlich-engine/src/world/blocks.rs:156 ...uces_face_against(neighbor)` (TODO), `is_meshable_volume()`     /// (TODO), etc....
crates/unendlich-engine/src/world/blocks.rs:157 ...s_meshable_volume()`     /// (TODO), etc. Old call sites still using `is_solid` ...
crates/unendlich-engine/src/world/blocks.rs:158 .../// debt — flag with `// TODO migrate to narrower predicate` when     /// touchi...
crates/unendlich-engine/src/world/biomes.rs:133 ...// TODO: add MapleLog/MapleLeaves blocks.             }),...
crates/unendlich-engine/src/world/fields.rs:290 ...//!     //! Today this is a stub returning a constant scalar (will become a 3D  ...
crates/unendlich-engine/src/world/leaf_decay.rs:79 ...oesn't yet — wind is a static stub — but will once wind is /// directional and t...
crates/unendlich-engine/src/world/leaf_decay.rs:222 ...// scalar 0.3-everywhere stub; a future directional wind                 // fiel...
crates/unendlich-engine/src/world/cpu_generator.rs:669 ...d         // hook in the bin (TODO when implemented): each Lava block         //...
crates/unendlich-engine/src/sim/ai_budget.rs:20 ...he viewer. Thresholds are /// placeholders (tune against the mob cap and render ...
crates/unendlich-engine/src/sim/physics.rs:457 ...ove. Reproduces brick-1's /// placeholder ground plane (rest height `y = 0`) and...
crates/unendlich-engine/src/sim/inventory_grid.rs:9 ...in isolation (R1): it owns a placeholder [`max_stack`] //! table rather than rea...
crates/unendlich-engine/src/sim/forage_harvest.rs:18 ...ks::BlockId`) as a documented placeholder until dedicated forage //! items exist...
crates/unendlich-engine/src/sim/forage_harvest.rs:50 ...lockId` discriminant     /// (placeholder mapping to the nearest existing palett...
crates/unendlich-engine/src/sim/forage_harvest.rs:54 ...ower,    // 25 — edible-plant placeholder             Forageable::Mushroom => Bl...
crates/unendlich-engine/src/sim/forage_harvest.rs:60 ...BlockId::OakLeaves,   // 18 — placeholder         };         b as u16     }     ...
crates/unendlich-engine/src/sim/emission.rs:159 ...three channels. These /// are placeholder ratios (the design doc treats numbers ...
crates/unendlich-engine/src/sim/landscape_memory.rs:8 ...value`] //! instead of a flat placeholder. //! //! Pure + deterministic. Bounded...
crates/unendlich-engine/src/sim/lore_attribution.rs:7 ...(the lore //! prose itself is placeholder). `sim::item_tooltip` renders the attr...
crates/unendlich-engine/src/sim/disposition.rs:61 ...tity_seed` (see module docs — placeholders until those /// genes exist). The POI...
crates/unendlich-engine/src/sim/disposition.rs:70 ...// behavioural loci (derived placeholders, distinct salts so they're independent...
crates/unendlich-engine/src/sim/menu_screen_layout.rs:425 ...layers / Name). /// These are placeholder action slots; the coordinator wires T-...
crates/unendlich-engine/src/sim/menu_screen_layout.rs:447 ...// r330 settings sub-pages — placeholder leaves (Back only) the coordinator     ...
crates/unendlich-engine/src/sim/menu_router.rs:542 ...or T-worldmgmt's page fields (placeholders until wired).     OpenToDht,     Whit...
crates/unendlich-engine/src/render/chunk_renderer.rs:198 ...Both samplers         // are placeholders for this bin (no shadow pre-pass, no f...
crates/unendlich-engine/src/render/chunk_renderer.rs:286 ...1 (shadow             // map placeholder), binding 2 (froxel volume placeholder)...
crates/unendlich-engine/src/render/chunk_renderer.rs:286 ...er), binding 2 (froxel volume placeholder).             // All stable for the re...
crates/unendlich-engine/src/render/gpu_timer.rs:549 .../// session this returns a placeholder.     pub fn flush_lifetime_summary(&self)...
crates/unendlich-engine/src/render/gpu_timer.rs:596 .../// `flush_log` returns a placeholder and other methods are no-ops.     pub fn s...
crates/unendlich-engine/src/render/gpu_meshing_octree.rs:524 .../ to guarantee freshness. The TODO is filed.                 match pool.upload_i...
crates/unendlich-engine/src/render/gpu_meshing_octree.rs:833 ...e normal case) start with the placeholder         // `Err(NOT_READY)` which mean...
crates/unendlich-engine/src/render/world_renderer.rs:2609 ...mes, M skipped)"     /// Or a placeholder if the device doesn't support timestam...
crates/unendlich-engine/src/render/world_renderer.rs:3914 ...// placeholder art until the shader/neural pass....
crates/unendlich-engine/src/items/loot.rs:12 ...the //! `has_tool` flag is a placeholder until a real tool/tier system exists.  ...
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:1762 ...he renderer (left as a marked TODO — not validated without a GPU).     // UNENDL...
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:1813 ...// controller's local regen stub — build while still, spend per cast.     let mu...
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:4582 ...// gesture: HOLD F7 (placeholder until the actions/ritual menu).             let...
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:6530 ...tooltips: format FINAL, lore placeholder for world voice).                 let t...
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:11773 ...ow;                 // thirst placeholder until Roll 3). Hidden in creative mode...

## D doc promises  (60)
crates/unendlich-engine/src/render_parity.rs:26 //! once the chunk stream has settled (e.g. N frames after the last upload).
crates/unendlich-engine/src/vertex_pack.rs:11 //!   normals (bumpy tops, future normal-mapping), not just the 6 face axes.
crates/unendlich-engine/src/water_field.rs:21 //! time (any BlockId::Water cell gets field value 1.0). Eventually:
crates/unendlich-engine/src/water_field.rs:29 //! blocks but unused by the mesher. Future sessions wire it in.
crates/unendlich-engine/src/water_field.rs:112 /// Future logic (in priority order):
crates/unendlich-engine/src/water_field.rs:123 ///    faster than dry stone. Eventually stone → gravel → sand.
crates/unendlich-engine/src/physics.rs:26 //!   field machinery to vary it spatially is future work).
crates/unendlich-engine/src/physics.rs:185 /// `ChunkManager` (today) and any future world-state source (server snapshot,
crates/unendlich-engine/src/texture_atlas.rs:231 /// stays the shipped art; this layout documents where variant runs live once the
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
crates/unendlich-engine/src/debug_settings.rs:231 /// Fragment-shader debug visualisation modes — reserved for a future
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
crates/unendlich-engine/src/world/biome_loader.rs:233 /// allowing JSON edits to take effect at runtime — is a future addition;
crates/unendlich-engine/src/world/world_model.rs:195 /// sit underneath the noise. Future evolution: each becomes a field of its
crates/unendlich-engine/src/world/texture_mapping.rs:49 /// checked and any future port (e.g. into the mesher) starts from a proven spec.
crates/unendlich-engine/src/world/gpu_mesher_vk.rs:14 //!    reads vertex+index data directly (this initial form) or — eventually —
crates/unendlich-engine/src/world/gpu_mesher_vk.rs:25 //! risky part of v0.18.1; once the plumbing is right, an algorithmic upgrade

## E commented registrations  (2)
crates/unendlich-engine/src/bin/unendlich_world.rs:387 // tick_reg.register("leaf_decay", ...) plus the "[flora] N leaves
    // orphaned" bl
crates/unendlich-engine/src/bin/unendlich_world.rs:396 //   tick_reg.register("leaf_decay",
    //       unendlich_engine::world::leaf_decay:

## F undeclared files  (3)
crates/unendlich-engine/src/render_meshref_shim.rs  (no `mod render_meshref_shim` in lib.rs)
crates/unendlich-engine/src/render/bloom.rs  (no `mod bloom` in mod.rs)
crates/unendlich-engine/src/render/postfx.rs  (no `mod postfx` in mod.rs)

## G env-var asymmetry  (124)
script sets UNENDLICH_FOO; no env::var read (dead/renamed knob)
script sets UNENDLICH_PLANAR_OVERFLOW; no env::var read (dead/renamed knob)
code reads UNENDLICH_ABILITY_CAST; script doesn't set it (undocumented knob)
code reads UNENDLICH_ADAPTIVE_RD; script doesn't set it (undocumented knob)
code reads UNENDLICH_ADAPTIVE_RD_GROW_MS; script doesn't set it (undocumented knob)
code reads UNENDLICH_ADAPTIVE_RD_GROW_STREAK; script doesn't set it (undocumented knob)
code reads UNENDLICH_ADAPTIVE_RD_MIN; script doesn't set it (undocumented knob)
code reads UNENDLICH_ADAPTIVE_RD_SHRINK_MS; script doesn't set it (undocumented knob)
code reads UNENDLICH_ADAPTIVE_RENDER_DISTANCE; script doesn't set it (undocumented knob)
code reads UNENDLICH_AGGRO; script doesn't set it (undocumented knob)
code reads UNENDLICH_AIR_CLEANUP; script doesn't set it (undocumented knob)
code reads UNENDLICH_AIR_CLEANUP_BUDGET; script doesn't set it (undocumented knob)
code reads UNENDLICH_AIR_CLEANUP_INTERVAL; script doesn't set it (undocumented knob)
code reads UNENDLICH_ARBITER; script doesn't set it (undocumented knob)
code reads UNENDLICH_AWARENESS; script doesn't set it (undocumented knob)
code reads UNENDLICH_BREADCRUMB; script doesn't set it (undocumented knob)
code reads UNENDLICH_BREADCRUMB_FILE; script doesn't set it (undocumented knob)
code reads UNENDLICH_CAM_SHAKE; script doesn't set it (undocumented knob)
code reads UNENDLICH_CAM_SMOOTH; script doesn't set it (undocumented knob)
code reads UNENDLICH_CAVE_FLORA; script doesn't set it (undocumented knob)
code reads UNENDLICH_COLUMN_CACHE; script doesn't set it (undocumented knob)
code reads UNENDLICH_COMBAT_MODS; script doesn't set it (undocumented knob)
code reads UNENDLICH_COPY_COALESCE; script doesn't set it (undocumented knob)
code reads UNENDLICH_DEBUG_LOG; script doesn't set it (undocumented knob)
code reads UNENDLICH_DETACHED_TERRAIN; script doesn't set it (undocumented knob)
code reads UNENDLICH_ECHO_DEBUG; script doesn't set it (undocumented knob)
code reads UNENDLICH_FLORA_SPREAD; script doesn't set it (undocumented knob)
code reads UNENDLICH_FLORA_SPREAD_BUDGET; script doesn't set it (undocumented knob)
code reads UNENDLICH_FLORA_SPREAD_INTERVAL; script doesn't set it (undocumented knob)
code reads UNENDLICH_FLORA_SPREAD_RADIUS; script doesn't set it (undocumented knob)
code reads UNENDLICH_FLORA_SPREAD_SCAN; script doesn't set it (undocumented knob)
code reads UNENDLICH_FRAMES_IN_FLIGHT; script doesn't set it (undocumented knob)
code reads UNENDLICH_FRAME_PROFILE; script doesn't set it (undocumented knob)
code reads UNENDLICH_GEN_BACKEND; script doesn't set it (undocumented knob)
code reads UNENDLICH_GEN_FALLOFF_FLOOR; script doesn't set it (undocumented knob)
code reads UNENDLICH_GPU_MESH_ADAPTIVE; script doesn't set it (undocumented knob)
code reads UNENDLICH_GPU_MESH_YIELD_PCT; script doesn't set it (undocumented knob)
code reads UNENDLICH_GRAZE_GATE; script doesn't set it (undocumented knob)
code reads UNENDLICH_HOT_METAL; script doesn't set it (undocumented knob)
code reads UNENDLICH_LAN_HOST; script doesn't set it (undocumented knob)
code reads UNENDLICH_LAN_MP; script doesn't set it (undocumented knob)
code reads UNENDLICH_LAVA_WATER; script doesn't set it (undocumented knob)
code reads UNENDLICH_LEAF_TICK_STRIDE; script doesn't set it (undocumented knob)
code reads UNENDLICH_LIGHTNING_ARC; script doesn't set it (undocumented knob)
code reads UNENDLICH_MAX_DRAGONS; script doesn't set it (undocumented knob)
code reads UNENDLICH_MESH_INDEX_POOL_MB; script doesn't set it (undocumented knob)
code reads UNENDLICH_MESH_QUEUE_CAPACITY; script doesn't set it (undocumented knob)
code reads UNENDLICH_MESH_VERT_POOL_MB; script doesn't set it (undocumented knob)
code reads UNENDLICH_MESH_WORKERS; script doesn't set it (undocumented knob)
code reads UNENDLICH_MOB_BANDS; script doesn't set it (undocumented knob)
code reads UNENDLICH_NEURAL_GBUFFER; script doesn't set it (undocumented knob)
code reads UNENDLICH_NEURAL_TRAIN; script doesn't set it (undocumented knob)
code reads UNENDLICH_NEURAL_TRAIN_SELFTEST; script doesn't set it (undocumented knob)
code reads UNENDLICH_NO_BURIED_CULL; script doesn't set it (undocumented knob)
code reads UNENDLICH_NO_REBAR; script doesn't set it (undocumented knob)
code reads UNENDLICH_OCCLUSION_CULL; script doesn't set it (undocumented knob)
code reads UNENDLICH_OCTREE_TILE; script doesn't set it (undocumented knob)
code reads UNENDLICH_PARTICLES; script doesn't set it (undocumented knob)
code reads UNENDLICH_PLANT_BUILD_SPREAD; script doesn't set it (undocumented knob)
code reads UNENDLICH_PLANT_STAGGER; script doesn't set it (undocumented knob)
code reads UNENDLICH_PLANT_TICK_STRIDE; script doesn't set it (undocumented knob)
code reads UNENDLICH_POI; script doesn't set it (undocumented knob)
code reads UNENDLICH_PREWARM_BUDGET; script doesn't set it (undocumented knob)
code reads UNENDLICH_SAPLING_CAP; script doesn't set it (undocumented knob)
code reads UNENDLICH_SAPLING_DT; script doesn't set it (undocumented knob)
code reads UNENDLICH_SAPLING_GROW_INTERVAL; script doesn't set it (undocumented knob)
code reads UNENDLICH_SAPLING_PLANT_PCT; script doesn't set it (undocumented knob)
code reads UNENDLICH_SAVES; script doesn't set it (undocumented knob)
code reads UNENDLICH_SEASON; script doesn't set it (undocumented knob)
code reads UNENDLICH_SIMD_PRECULL; script doesn't set it (undocumented knob)
code reads UNENDLICH_SIM_DISTANCE; script doesn't set it (undocumented knob)
code reads UNENDLICH_SIM_SWEEP; script doesn't set it (undocumented knob)
code reads UNENDLICH_SKIP_AIR_CHUNKS; script doesn't set it (undocumented knob)
code reads UNENDLICH_SNOW; script doesn't set it (undocumented knob)
code reads UNENDLICH_SUBMIT_BUDGET_MS; script doesn't set it (undocumented knob)
code reads UNENDLICH_SURFACE_CACHE; script doesn't set it (undocumented knob)
code reads UNENDLICH_SURFACE_SCAN_SEED; script doesn't set it (undocumented knob)
code reads UNENDLICH_TEXTURES; script doesn't set it (undocumented knob)
code reads UNENDLICH_TIMEBEND_INTERVAL_S; script doesn't set it (undocumented knob)
code reads UNENDLICH_TIMEBEND_SNAPSHOTS; script doesn't set it (undocumented knob)
code reads UNENDLICH_VIEW_BOB; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_BOIL; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_BOIL_RATE; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_BOIL_TEMP; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_BUDGET; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_CURRENT; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_CURRENT_MAX; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_CURRENT_STRENGTH; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_DEBUG; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_DEPTH; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_DEPTH_COLOR; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_EROSION; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_EROSION_BUDGET; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_EVAP; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_EVAP_RATE; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_FLOW_DEBUG; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_FLOW_EROSION; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_FLOW_EROSION_BUDGET; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_FLOW_EROSION_THRESHOLD; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_FLOW_SHADER; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_FLOW_SMOOTH; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_ICE; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_ICE_TEMP; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_INTERVAL_MS; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_ITERS; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_MAX_FLOW; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_PUSHBACK; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_PUSHBACK_GAIN; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_PUSHBACK_MAX; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_RADIUS; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_RAIN; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_RAIN_CAP; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_RAIN_RATE; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_SEDIMENT_CAPACITY; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_SETTLE_F; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_SIM; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_STEAM_PRESSURE; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_UNDERWATER_FX; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_WARMUP_SECS; script doesn't set it (undocumented knob)
code reads UNENDLICH_WATER_WAVE_SPEED; script doesn't set it (undocumented knob)
code reads UNENDLICH_WEATHER_SIM; script doesn't set it (undocumented knob)
code reads UNENDLICH_WING_BUFFET; script doesn't set it (undocumented knob)
code reads UNENDLICH_WITNESS; script doesn't set it (undocumented knob)
code reads UNENDLICH_YEAR_DAYS; script doesn't set it (undocumented knob)

## H dead debug toggles  (7)
crates/unendlich-engine/src/debug_settings.rs:31 shader_mode  (zero consumers)
crates/unendlich-engine/src/debug_settings.rs:48 disable_ssr  (zero consumers)
crates/unendlich-engine/src/debug_settings.rs:49 disable_bloom  (zero consumers)
crates/unendlich-engine/src/debug_settings.rs:53 freeze_lod  (zero consumers)
crates/unendlich-engine/src/debug_settings.rs:55 pause_sim  (zero consumers)
crates/unendlich-engine/src/debug_settings.rs:58 show_chunk_borders  (zero consumers)
crates/unendlich-engine/src/debug_settings.rs:65 fog_approach  (zero consumers)

## I unproduced AppInputs  (0)
(clean)

## J doc->module map  (14)
bending_character_menu_ui_v2.extracted.md: ambient, camera, collision, creature, depth, diffusion, dragon, elemental, elements, emission, emissive, fabric, fixed, gravity
bending_discipline_panels_all.extracted.md: ambient, audio, blocks, buoyancy, camera, cloud, cluster, collision, combat, context, creature, depth, dragon, effects
character_panel_reference.extracted.md: ambient, blocks, cloud, collections, combat, context, crafting, depth, dragon, elemental, elements, fabric, identity, inventory
crafting_inventory_layout.extracted.md: ambient, blocks, chunk, cluster, coral, crafting, depth, dragon, effects, elemental, elements, emission, emissive, fabric
fluid_pois_waterfalls.extracted.md: acoustics, ambient, audio, biomes, blocks, bloom, cloud, cluster, collapse, context, convergence, crafting, creature, deposition
gamemode_trait_system.extracted.md: ambient, attributes, behavior, biomes, blocks, carry, collision, combat, concealment, context, crafting, depth, disposition, dragon
headless_coverage_testing.extracted.md: audio, awareness, behavior, biomes, blocks, chunk, collections, combat, context, crafting, depth, dragon, driver, drops
item_armor_lore_tooltips.extracted.md: ambient, blocks, carry, collapse, combat, crafting, creature, depth, dragon, drops, effects, elemental, elements, emissive
llm_helper_library_guide.extracted.md: audio, blocks, carry, chunk, cluster, constants, context, crafting, creature, creature_design, creature_viewer, creature_viewer_ui, dragon, drops
mob_dragon_viewer_ui_v2.extracted.md: ambient, blocks, bloom, camera, cloud, combat, creature, depth, dragon, effects, elemental, elements, emission, emissive
phone_controller_lan-v1r2.extracted.md: audio, camera, cloud, codec, context, creature, dragon, driver, drops, entity, fixed, identity, input, inventory
textures_overlay_system.extracted.md: carry, climate, context, depth, erosion, gravity, identity, light, material, resonance, surface, weathering
touch_input_reference.extracted.md: ambient, audio, camera, collapse, context, dragon, elements, entity, fixed, input, inventory, material, navigate, noise
tutorial_system.extracted.md: ambient, audio, blocks, camera, carry, context, crafting, creature, depth, dragon, elemental, elements, emissive, genetics

## K sibling-scope risks (bin)  (40)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:36 `chunk_manager` used above its only let (959)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:1089 `camera` used above its only let (1092)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:918 `roamers` used above its only let (1225)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:456 `local_player` used above its only let (1280)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:284 `selected_block` used above its only let (1666)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:285 `hand_item` used above its only let (1672)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:725 `adaptive_rd` used above its only let (2486)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:607 `warmup_ms` used above its only let (2552)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2685 `surface_cache` used above its only let (2687)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:138 `species` used above its only let (5074)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:5089 `remove` used above its only let (5157)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:3709 `hatched` used above its only let (5460)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:843 `fields` used above its only let (5980)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:5379 `melted` used above its only let (6223)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:6898 `params` used above its only let (7235)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:1580 `spawns` used above its only let (7410)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:129 `persistent` used above its only let (7482)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:5661 `sample` used above its only let (7866)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:177 `disposition` used above its only let (7938)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:913 `wariness` used above its only let (7987)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:1879 `approach` used above its only let (8074)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:1914 `drives` used above its only let (8089)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:8112 `fatigue` used above its only let (8414)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:8440 `heading` used above its only let (8537)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:7896 `nearest` used above its only let (8703)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:3076 `cleared` used above its only let (10102)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:1248 `removed` used above its only let (10615)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:980 `loaded` used above its only let (10684)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:6701 `already` used above its only let (11001)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:840 `world_model` used above its only let (11025)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:1859 `player` used above its only let (11743)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2019 `moving` used above its only let (11751)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:8623 `loudness` used above its only let (11753)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:1979 `radius` used above its only let (12326)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:1156 `palette` used above its only let (12389)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:12177 `walk_phase` used above its only let (12522)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:1849 `weapon` used above its only let (12526)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:6293 `grounded` used above its only let (12863)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:4312 `resident` used above its only let (13000)
crates/unendlich-engine/src/bin/unendlich_fullgame.rs:2473 `visible` used above its only let (13005)
