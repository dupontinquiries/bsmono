//! phys — general physics-simulation helper modules (r200). Pure,
//! deterministic, std-only, each modeling one classical behavior so the
//! engine can wire convincing motion without a physics-engine dependency.
//! Narrow modules per the helper-library guide: no catch-all.
pub mod acoustics;    // r201: propagation delay, air absorption, Sabine RT60
pub mod fluid_forces; // buoyancy + quadratic drag
pub mod hydro;        // r201: hydrostatic pressure, Torricelli outflow, head flow
pub mod rope;         // Verlet rope / chain / vine

