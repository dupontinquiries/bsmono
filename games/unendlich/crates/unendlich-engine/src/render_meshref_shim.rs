//! Minimal pure-CPU surface of the `render` module, exposed under the
//! `mesh-ref` feature. It re-uses the real `mesher` and the octree CPU
//! reference verbatim (via `#[path]`) WITHOUT pulling glfw/ash/vk-sys, so the
//! GPU-vs-CPU mesh parity harness can build and run headless on lavapipe.
//!
//! This module body is selected only when `mesh-ref` is on and `render` is off
//! (see lib.rs); when the full `render` feature is enabled, the complete render
//! module supersedes it. Both expose the identical paths
//! `crate::render::mesher` and `crate::render::gpu_meshing_octree_cpu_ref`, so
//! the CPU reference's internal `use crate::render::mesher::...` resolves the
//! same way under either feature.

#[path = "render/mesher.rs"]
pub mod mesher;

#[path = "render/gpu_meshing_octree_cpu_ref.rs"]
pub mod gpu_meshing_octree_cpu_ref;

// Pure-logic upload-cap controller (no glfw/ash). Exposed headless so its unit
// tests run and its AIMD logic is in the verifiable surface.
#[path = "render/adaptive_upload.rs"]
pub mod adaptive_upload;
