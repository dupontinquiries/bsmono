//! The engine's `sim::` namespace, reproduced.
//!
//! The extracted files say `use crate::sim::hearing_radius;` and so on.
//! Keeping that path valid is what lets them stay byte-identical to the
//! copies still in `unendlich-engine`, so the two can be diffed
//! mechanically until the engine depends on this crate instead.

pub mod audio_occlusion;
pub mod creature_hearing;
pub mod fog_model;
pub mod hearing_radius;
pub mod light_physics;
pub mod neighbor_grid;
pub mod path_cost;
pub mod path_memory;
pub mod perception;
pub mod predator_prey;
pub mod prey_scan;
pub mod river_flow;
pub mod scent_decay;
pub mod scent_gradient;
pub mod scent_strength;
pub mod scent_track;
pub mod sight_detect;
pub mod spatial_audio;
/// The one module here that is NOT byte-identical to the engine's copy --
/// its `linkme` service registration was cut. See the file header.
pub mod stamina;
pub mod step_height;
pub mod terrain_variation;
pub mod wind_field;
