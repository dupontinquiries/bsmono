//! Meinkraft library — voxel game engine modules.
//!
//! Public API is organized as:
//! * `world::*`    - terrain generation, chunks, biomes, blocks
//! * `keybindings` - configurable controls
//! * `input`       - keyboard/mouse state cache
//! * `player`      - player physics & input handling
//! * `constants`   - tunable game-wide values

pub mod pngutil;
pub mod constants;
pub mod runtime_flags;
pub mod thread_probe;
pub mod warn_snipe;
pub mod render_parity;
pub mod thread_priority;
pub mod cloud;
pub mod creature;
pub mod heat_source;
pub mod debug_overlay;
pub mod dev_mode; // r206: UNENDLICH_DEV runtime conveniences
pub mod debug_settings;
pub mod lightning_path;
pub mod mesh_smooth;
pub mod storm_field;
pub mod tornado_vortex;
pub mod blur_kernel;
pub mod batch_motion_blur_vec;
pub mod exposure_adapt;
pub mod fxaa_edge;
pub mod refraction_offset;
pub mod batch_light_falloff;
pub mod depth_fade;
pub mod motion_blur_vec;
pub mod tonemap_curve;
pub mod batch_lod_crossfade;
pub mod edge_highlight;
pub mod heat_haze;
pub mod spark_burst;
pub mod sss_approx;
pub mod batch_spec_highlight;
pub mod contact_shadow;
pub mod lod_select;
pub mod rim_light;
pub mod uv_scroll;
pub mod batch_icon_atlas;
pub mod health_bar_uv;
pub mod nameplate_fade;
pub mod outline_width;
pub mod spec_highlight;
pub mod batch_pipeline_key;
pub mod damage_flash;
pub mod held_item_pose;
pub mod icon_atlas;
pub mod mob_tint;
pub mod batch_draw_key;
pub mod impostor_uv;
pub mod anim_cadence;
pub mod armor_reflect;
pub mod wing_opacity;
pub mod wing_membrane;
pub mod cloud_noise;
pub mod shadow_cascade;
pub mod ssr_params;
pub mod froxel_index;
pub mod ribbon_mesh;
pub mod silhouette_lod;
pub mod bloom_math;
pub mod lod_skirt;
pub mod spirv_reflect_check;
pub mod server_connect;
pub mod settings_schema;
pub mod whitelist;
pub mod world_params;
pub mod climate;
pub mod footprint_decal;
pub mod menu_layout;
pub mod mob_lod;
pub mod mob_instances;
pub mod mob_visibility; // r594: per-view frustum cull + projected-size LOD selection (feeds mob_instances::pack)
pub mod tonemap_lut;
pub mod pbr_terms;
pub mod viewport_layout;
pub mod view_cull;
pub mod moisture_field;
pub mod moisture_texture;
#[cfg(feature = "render-core")]
pub mod local_player;
pub mod timebend_scrub;
pub mod permissions;
pub mod phys; // r200: general physics-sim helpers (rope, fluid forces)
pub mod physics;
pub mod profiler; // r204: zone-histogram profiler (percentiles, structured dump)
pub mod region_budget; // r206: per-region work-budget tracker
pub mod raycast;
pub mod sky;
pub mod weather;
pub mod visual_fade;
pub mod light;
pub mod occupancy;
pub mod frame_schedule;
pub mod plant_noise;
pub mod material;
pub mod wetness;
pub mod vertex_pack;
pub mod thermal;
pub mod texture_atlas;
pub mod creature_design;
pub mod genetics;
pub mod creature_viewer;
pub mod player_character;
pub mod hero_forge;
#[cfg(feature = "egui_debug")]
pub mod creature_viewer_ui;
pub mod air;
pub mod fabric;
pub mod pose;
pub mod collision;
pub mod inventory;
pub mod action_queue;
pub mod inventory_screen;
pub mod ui_feel;
pub mod emblem;
pub mod item;
pub mod spawn;
pub mod mob;
pub mod loot;
pub mod recipe;
pub mod mining;
pub mod hotbar;
#[cfg(test)]
mod shader_artifacts;
pub mod crafting;
pub mod sumtree;
pub mod season;
pub mod sampling;
pub mod snow;
pub mod snapshot;
pub mod storage;
pub mod temperature;
pub mod thermal_mass;
pub mod wind;
pub mod water_field;
pub mod util; // r193: helper libraries (llm_helper_library_guide)
pub mod world;
pub mod sim;        // [applied onto 88b5542] simulation subsystem
#[cfg(feature = "audio")]
pub mod audio; // spatial music playback (rodio); pure mixer lives in sim::spatial_audio
pub mod breadcrumb;
pub mod debug_scene;
pub mod app;        // app core: the testable composition layer the thin bin calls
pub mod items;      // [applied] inventory/placement/crafting/drops/loot + debug ui/hud
pub mod telemetry;  // [applied] run telemetry: achievements/report/window

pub mod input; // r328: backend-neutral (no glfw) — always available
pub mod input_capture; // r328: no glfw — always available
pub mod keybind_ui; // r199 hotkey rebinding; r328 glfw-free
pub mod keybindings;
// Backend-neutral input vocabulary (T07-R27 input layer; migration deferred).
pub mod input_codes;
pub mod player; // r328: no glfw — always available

#[cfg(feature = "render-core")]
pub mod render;

// Headless mesh-parity: expose the pure mesher + octree CPU reference under a
// minimal `render` shim when the real render module is off. Mutually exclusive
// with the real render module above.
#[cfg(all(feature = "mesh-ref", not(feature = "render-core")))]
#[path = "render_meshref_shim.rs"]
pub mod render;
pub mod lod_band;
pub mod entity_cost_model;
pub mod descriptor_registry;
pub mod impostor_card;
pub mod sky_gradient;
pub mod godray_math;
pub mod atmosphere_scatter;
pub mod vignette_grain;
pub mod water_caustics;
pub mod pipeline_key;
pub mod batch_emissive;
pub mod draw_key;
pub mod ao_corner;
pub mod batch_held_item_pose;
pub mod billboard_face;
pub mod fog_blend;
pub mod lod_crossfade;
pub mod batch_fog_blend;
pub mod decal_uv;
pub mod horn_fractal;
pub mod particle_fade;
pub mod ribbon_trail;
pub mod region_cull;
pub mod lod_morph;
pub mod light_falloff;
pub mod batch_rim_light;
pub mod batch_depth_fade;
pub mod chromatic_aberration;
pub mod dof_blur;
pub mod lens_dirt;
pub mod part_mesh_lod;
pub mod seam_skirt;
pub mod voxel_greedy_run;
pub mod batch_chromatic_aberration;
pub mod normal_smooth;
pub mod specular_aa;
pub mod crack_fill;
pub mod batch_dof_blur;
pub mod emissive_pulse;
pub mod parallax_offset;
pub mod contact_ao;
pub mod batch_specular_aa;
pub mod rayleigh_phase;
pub mod mie_phase;
pub mod sun_disk;
pub mod batch_mie_phase;
pub mod fresnel_schlick;
pub mod ggx_distribution;
pub mod iridescence;
pub mod batch_fresnel;
pub mod gerstner_wave;
pub mod foam_threshold;
pub mod water_depth_color;
pub mod batch_gerstner;
pub mod healthbar_fill;
pub mod radial_sweep;
pub mod minimap_project;
pub mod batch_healthbar_fill;
pub mod particle_size_curve;
pub mod particle_alpha_curve;
pub mod particle_color_ramp;
pub mod batch_particle_alpha;
pub mod sun_elevation_color;
pub mod ambient_intensity;
pub mod shadow_length;
pub mod batch_ambient_intensity;
pub mod decal_fade;
pub mod projector_uv;
pub mod detail_normal_blend;
pub mod batch_decal_fade;
pub mod camera_fov_zoom;
pub mod camera_shake;
pub mod depth_to_view_z;
pub mod batch_depth_to_view_z;
pub mod beer_lambert;
pub mod height_fog;
pub mod scatter_inscatter;
pub mod batch_beer_lambert;
pub mod verlet_step;
pub mod constraint_correction;
pub mod cloth_wind_force;
pub mod batch_verlet_step;
pub mod spring_force;
pub mod damping_force;
pub mod catenary_sag;
pub mod batch_spring_force;
pub mod trail_alpha_fade;
pub mod tracer_width;
pub mod muzzle_flash_intensity;
pub mod batch_trail_alpha_fade;
pub mod burn_glow_intensity;
pub mod freeze_tint;
pub mod shock_arc_jitter;
pub mod batch_burn_glow_intensity;
pub mod stall_factor;
pub mod climb_rate;
pub mod turn_g_force;
pub mod batch_stall_factor;
pub mod hit_flash_intensity;
pub mod damage_number_scale;
pub mod low_health_vignette;
pub mod batch_damage_number_scale;
pub mod cloud_shadow_factor;
pub mod rain_streak_alpha;
pub mod lightning_flash;
pub mod batch_rain_streak_alpha;
pub mod safe_tune; // r322: crash-safe runtime tuning (per-driver blacklist)
pub mod runtime_config; // r323: central perf/graphics config + presets (unified settings foundation)
pub mod device_budget; // r594: hardware capability -> starting preset + octree pool MB + worker threads
