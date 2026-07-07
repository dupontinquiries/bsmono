//! `unendlich-shared` — types and machinery shared across the
//! unendlich crates: spec/result format, run identifiers, structured
//! event log, run-summary block, zone-stats accumulator.
//!
//! Has no engine deps (no Vulkan, no glfw, no nalgebra). Anything that
//! both the engine and the launcher need to agree on lives here.

pub mod zone_stats;
pub mod run_summary;
pub mod run_id;
pub mod events;
pub mod test_spec;
