//! World subsystem: blocks, biomes, generation, chunks, loading.

pub mod area_mining;
pub mod attributes;
pub mod biomes;
pub mod ecosystem;
pub mod ecosystem_census;
pub mod mob_persistence;
pub mod spawn_streaming;
pub mod biome_loader;
pub mod blocks;
pub mod block_transform;
pub mod chunk;
pub mod chunk_loader;
pub mod chunk_manager;
pub mod block_textures;
pub mod block_material;
pub mod texture_mapping;
pub mod shader_contract;
pub mod cluster;
pub mod cpu_generator;
pub mod terrain_batch; // r395: parallel chunk generation (terrain + water placement) over the rayon lane
pub mod surface_run;
pub mod entity;
pub mod fields;
pub mod flight;
pub mod leaf_decay;
pub mod lod;
pub mod material_response;
pub mod noise;
pub mod octree;
pub mod octree_raycast;
pub mod octree_region;
pub mod octree_scheduler;
pub mod parkour;
pub mod pathing;
pub mod plant_blueprint;
pub mod plant_lifecycle;
pub mod reef_seeder; // r192: fluid POIs (corals + waterfall scoring)
pub mod plant_genetics;
pub mod remesh_policy;
pub mod upload_repr;
pub mod plant_exotics;
pub mod pollination;
pub mod stage_tint;
pub mod pressure_sampler;
pub mod scheduler;
pub mod season;
pub mod subvoxel;
// (weather declared after weathering below to avoid name confusion)
#[cfg(any(feature = "render-core", feature = "mesh-ref"))]
pub mod subvoxel_templates;
pub mod surface;
pub mod terrain_generator;
pub mod ticks;
pub mod weathering;
pub mod rock;
pub mod sparse_features;
pub mod adaptive_stream;
pub mod mesh_brick;
pub mod brick_mesher;
pub mod brick_occupancy;
pub mod draw_batch;
pub mod octree_budget;
pub mod layered_rle;
pub mod record_partition;
pub mod shader_residency;
pub mod region_graph;
pub mod rollback_scope;
pub mod weather;
pub mod water_field;
pub mod water_flow_field; // r417: per-column flow vectors from the settle (phase 1 of flow-as-shader)
pub mod water_flow_tiles; // r418: persistent sparse flow-vector tiles (phase 2 of flow-as-shader)
pub mod water_evaporate; // r423: dries isolated shallow puddles (guarded; never lakes)
pub mod water_sources; // r424: rain fills exposed columns up to a shallow cap (puddles, no flood)
pub mod named_locations;
pub mod world_list; // r439: world registry + load ruleset (no concurrent worlds to start)
pub mod sediment_map; // r443: suspended-sediment transport table for mass-conserving erosion
pub mod water_field_batch; // r406: threaded multi-chunk settle + evap/rain cycle
pub mod water_cells; // r407: placement+drain+fill+physics water-cell step (threaded)
pub mod water_edge_flow; // r408: cross-chunk water flow across shared edges
pub mod aquifer_coupling; // r409: infiltration↔groundwater↔seep cycle
pub mod water_cycle; // r410: full per-chunk water cycle (capstone)
pub mod world_model;
pub mod chunk_flora;
pub mod flora_sites;
pub mod cave_flora;
pub mod stem_gpu;
pub mod stem;
pub mod lumen;
pub mod collapse;
pub mod erosion;
pub mod effects;
pub mod field_brush;
pub mod elemental;

// Worker-03 WORLD/PERF pure cores (logic only; coordinator wires into octree/
// chunk_manager / growth tick).
pub mod competition;
pub mod lod_upload;
pub mod octree_compact;
pub mod recompaction;
// Round 2.
pub mod device_tier;
pub mod orogeny;
pub mod structure_place;
// Round 3.
pub mod aquatic_materials;
pub mod bathymetry;
pub mod sea_level;
// Round 4.
pub mod coral;
pub mod span_alloc;
pub mod structure_template;
// Round 5.
pub mod flow_field;
pub mod river_source;
pub mod water_cell;
// Round 6.
pub mod giant_cave;
pub mod locate;
pub mod voxel_raycast;
// Round 7.
pub mod block_predicates;
pub mod octree_remerge;
pub mod rebuild_coalescer;
// Round 9.
pub mod cave_network;
pub mod ore_distribution;
pub mod river_carve;
pub mod river_carve_field; // r411: compose accumulate_flow + river_carve into a heightfield river-carving pass
// Round 11.
pub mod cave_carve;
pub mod cave_field; // WG1 r102
pub mod chunk_priority;
pub mod mesh_decimate;
// Round 14.
pub mod dust_motes;
pub mod ember_field;
pub mod smoke_field;
// Round 15.
pub mod batch_chunk_priority;
pub mod mesh_tier;
// Round 16.
pub mod batch_mesh_tier;
pub mod ridge_noise;
pub mod strata;
pub mod terrace;
pub mod terrain_warp;
pub mod thermal_erosion;
// Round 17.
pub mod batch_ridge_noise;
pub mod cave_room;
pub mod moisture_spread;
pub mod ore_vein_richness;
pub mod river_mouth;
pub mod scatter_cluster;
// Round 19.
pub mod batch_terrain_warp;
pub mod flow_route;
pub mod lake_fill;
pub mod sediment_deposit;
pub mod snow_line;
// Round 21.
pub mod batch_biome_temp_map;
pub mod cave_pool;
pub mod frost_spread;
pub mod gravel_slide;
pub mod waterfall;
// Round 23.
pub mod aquifer;
pub mod batch_gen_estimate;
pub mod cave_breeze;
pub mod erosion_advect;

#[cfg(feature = "gpu")]
pub mod gpu_generator;

pub mod gpu_chunk_pool;

#[cfg(feature = "gpu")]
pub mod gpu_chunk_pool_vk;

#[cfg(feature = "gpu")]
pub mod gpu_mesher_vk;

pub use biomes::{Biome, BiomeSample};
pub use blocks::BlockId;
pub use chunk::{Chunk, ChunkCoord};
pub use chunk_loader::{block_to_chunk, chunks_in_radius, ChunkLoader};
pub use chunk_manager::ChunkManager;
pub use terrain_generator::{Backend, TerrainGenerator};
pub mod biome_blend;
pub mod flow_accumulation;
pub mod structure_grammar;
pub mod rare_structure;
pub mod field_ops; // r340-sl-fields: shared field operators
pub mod worldgen_noise; // r340-sl-worldgen: shared noise/density // r332-structures: rare-structure placement selector
pub mod hydraulic_erosion;
pub mod dungeon_layout;
pub mod mineral_hardness;
pub mod vegetation_scatter;
pub mod biome_transition;
pub mod mineral_vein_trace;
pub mod block_edit;
pub mod edit_coalesce;
pub mod octree_invariants;
pub mod biome_temp_map;
pub mod cliff_detect;
pub mod beach_band;
pub mod cave_decoration;
pub mod batch_strata;
pub mod foam_edge;
pub mod ripple_ring;
pub mod cave_moss;
pub mod erosion_settle;
pub mod batch_cliff_detect;
pub mod prefetch_predict;
pub mod chunk_schedule;
pub mod field_step;
pub mod gen_estimate;
pub mod batch_flow_route;
pub mod lava_flow;
pub mod snow_accumulate;
pub mod sediment_settle;
pub mod batch_erosion_advect;
pub mod river_meander;
pub mod glacier_carve;
pub mod dune_migrate;
pub mod batch_frost_spread;
pub mod canyon_carve;
pub mod delta_fan;
pub mod karst_sink;
pub mod batch_lava_flow;
pub mod mesa_form;
pub mod hoodoo_carve;
pub mod fjord_cut;
pub mod batch_river_meander;
pub mod talus_slope;
pub mod oxbow_lake;
pub mod alluvial_fan;
pub mod batch_canyon_carve;
pub mod altitude_temp;
pub mod rain_shadow;
pub mod tree_line;
pub mod batch_altitude_temp;
pub mod ore_grade;
pub mod forest_density;
pub mod spawn_density;
pub mod batch_ore_grade;
pub mod wave_height;
pub mod tide_level;
pub mod salinity;
pub mod batch_tide_level;
pub mod cloud_coverage;
pub mod wind_shear;
pub mod precip_rate;
pub mod batch_cloud_coverage;
pub mod sound_attenuation;
pub mod doppler_shift;
pub mod reverb_wetness;
pub mod batch_sound_attenuation;
pub mod season_phase;
pub mod daylight_hours;
pub mod seasonal_temp_offset;
pub mod batch_seasonal_temp_offset;
pub mod nest_suitability;
pub mod territory_overlap;
pub mod resource_regen;
pub mod batch_nest_suitability;
pub mod grass_sway;
pub mod vegetation_density;
pub mod snow_cover;
pub mod climate_surface;
pub mod weather_cell;
pub mod seasonal_weather;
pub mod weather_front;
pub mod storm_organization;
pub mod snowpack;
pub mod weather_state;
pub mod batch_vegetation_density;
pub mod flow_velocity;
pub mod erosion_capacity;
pub mod deposition;
pub mod batch_flow_velocity;
pub mod arch_height;
pub mod tower_taper;
pub mod wall_crenellation;
pub mod batch_arch_height;
pub mod room_fits;
pub mod corridor_length;
pub mod stair_steps;
pub mod batch_corridor_length;
pub mod cave_tunnel_radius;
pub mod stalactite_length;
pub mod biome_blend_weight;
pub mod batch_cave_tunnel_radius;
pub mod lake_fill_level;
pub mod river_width;
pub mod moisture_from_water;
pub mod batch_river_width;
pub mod building_spacing;
pub mod plaza_radius;
pub mod wall_perimeter;
pub mod batch_plaza_radius;
pub mod road_grade_penalty;
pub mod switchback_count;
pub mod bridge_needed;
pub mod batch_road_grade_penalty;
pub mod front_temperature_gradient;
pub mod pressure_gradient_wind;
pub mod storm_intensity;
pub mod batch_storm_intensity;
pub mod plant_gene_traits;
pub mod batch_density_noise;
pub mod octree_encode_choice;
// B1 (r102) bughunt: world-epoch stamp to drop stale in-flight mesh results
// after a world reset (reset_with_generator/rebuild_for_seed). Pure predicate.
pub mod mesh_epoch;
pub mod flora_placement;
pub mod title_structure;
pub mod flora_map;
pub mod flora_blocks;
pub mod water_waves; // r151b: file existed but was never declared
