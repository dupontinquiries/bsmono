//! util — narrow, honest helper modules (llm_helper_library_guide).
//! No catch-all: each module owns one concept. std-only, pure, tested.
pub mod bbox;
pub mod batch_kernel; // r331: shared batch-map helper (refactor-batch)
pub mod fastmath; // r196: approximate transcendentals (SIM/VISUAL only)
pub mod mathx; // r331: exact scalar-math helpers (refactor-math)
pub mod hash_util;
pub mod task_lanes; // r196: operation-gated parallelism (P/C/S classes)
pub mod vecmath;
pub mod simvec; // r336-simvectors: batched autovectorizable vector ops
