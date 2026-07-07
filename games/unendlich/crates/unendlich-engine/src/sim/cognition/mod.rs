//! Cognition — the server-side, **non-rolled-back** cognitive layer that sits
//! above the deterministic sim core.
//!
//! Modules here read the world and the sim freely, may hold large per-creature
//! state, and ultimately emit compact decisions that re-enter the rolled-back
//! core as recorded input (the "cognition output is input" bridge described in
//! `unendlich-creature-cognition.md`). Nothing in this module is part of
//! [`crate::sim::state::SimState`].
//!
//! - [`memory`] — landmark memory cortex (spatial, decaying). ✅
//! - perception batching, appraisal (world-delta → signals), and the
//!   intent assembler land here as later bricks.

pub mod appraisal;
pub mod cognitive_sim;
pub mod memory;
pub mod mind;
pub mod navigate;
pub mod wander;
pub mod steering;
pub mod perception_batch;
pub mod sentinel_agent;
pub mod flock;
pub mod forager;
pub mod behavior;
pub mod perceive_bridge;
pub mod mob_sim;
pub mod imitation;
pub mod learning;
pub mod service;

pub use appraisal::{appraise, SelfState, Sighting};
pub use cognitive_sim::CognitiveSim;
pub use memory::{Landmark, LandmarkTag, Memory};
pub use mind::{Intent, Mind};
pub use navigate::{heading_toward, Locomotion, NavRegion};
pub use service::CognitionService;
