//! Vulkan rendering pipeline — graphics path.
//!
//! Available only with `--features render` (= gpu + window + vk-sys).

pub mod adaptive_upload;
pub mod camera;
pub mod dynamic_lights;
pub mod chunk_pipeline;
pub mod entity_pipeline;
pub mod chunk_renderer;
pub mod context;
pub mod depth;
pub mod gbuffer;
pub mod froxel;
pub mod froxel_fill_pipeline;
// T06-R27: offscreen color target — the headless render destination (and the
// windowed post-FX scene target). ash-only, no glfw.
pub mod offscreen;
#[cfg(feature = "hot-shaders")]
pub mod hot_shaders;
pub mod light;
pub mod mesher;
pub mod mesh_buffer_alloc;
pub mod mesh_buffer_pool;
pub mod chunk_indirect;
pub mod gpu_timer;
pub mod cpu_timer;
pub mod shadow_bucket_timer;
pub mod mesher_timer;
pub mod meshing_pool;
#[cfg(feature = "gpu")]
pub mod gpu_meshing_sparse;
pub mod renderer;
pub mod shadow_map;
pub mod shadow_pipeline;
pub mod swapchain;
pub mod triangle_pipeline;
pub mod view_compute_pipeline;
pub mod cascade_fit_pipeline;
pub mod field_diffuse_gpu;
pub mod view_params;
pub mod vulkan_state_model;
pub mod world_renderer;

// v0.24.26-octree-on-gpu: GPU-resident octree substrate.
//
// `octree_buffer` holds the per-chunk OctreeArray bytes in a single
// HOST_VISIBLE storage buffer, sub-allocated by SpanAllocator.
// `octree_probe_pipeline` is the verification consumer — runs
// `shaders/octree_probe.comp` to round-trip samples from GPU to CPU
// so the wire format is falsifiable end-to-end (gate via
// `UNENDLICH_OCTREE_VERIFY=1`). No production consumer migrates this
// ship; this is foundation for the mesher/raycast migrations.
pub mod octree_buffer;
pub mod octree_probe_pipeline;
// r301: neural deferred shading (forward-ported from the r67 integration; compile-only,
// not yet wired into the render loop — the live-context wiring is M2-side per RUST_INTEGRATION.md).
pub mod q8;
pub mod neural_shade_pipeline;
pub mod neural_gemm_pipeline;
pub mod neural_feat_pipeline;
pub mod neural_train;

// v0.24.27-octree-mesher: the first real consumer of the octree
// substrate. GPU mesher that reads chunks' octrees directly
// (instead of CPU-extracted `SurfaceRecord[]`). Opt-in via
// `UNENDLICH_GPU_MESHER=2`. Strict Camp 1 quads, open-boundary;
// the bumpy 4×4 mesher is a follow-up. See module docs.
pub mod gpu_meshing_octree;

// v0.24.28-bumpy-foundation: CPU reference port of the GPU octree
// mesher. Unit-tested on synthetic octrees so algorithm-level bugs
// (coordinate space, indexing, winding) surface in `cargo test`,
// not after a build-ship-playtest cycle. The v0.24.27 chunk-local-
// vs-world-coords debug arc cost three sub-ship iterations; this
// module is the prevention.
pub mod gpu_meshing_octree_cpu_ref;

#[cfg(feature = "ui-overlay")]
pub mod egui_overlay;
// r155 AUDIT NOTE: present_pipeline.rs, bloom.rs, postfx.rs exist on disk
// but are deliberately NOT declared — they are bit-rotted against the
// current swapchain API (they import create_offscreen_render_pass /
// create_present_render_pass / create_bloom_render_pass, none of which
// exist in render::swapchain anymore). Declaring them breaks the build.
// The PostFx session must FIRST restore those three render-pass ctors
// in swapchain.rs, then declare bloom + postfx, then integrate.
// present_pipeline imports only ash and IS safe — declared below.
pub mod present_pipeline;

pub use camera::{Camera, FlyCamera};
pub use chunk_renderer::ChunkRenderer;
pub use context::VulkanContext;
pub use mesher::{mesh_chunk, ChunkMesh, Vertex};
pub use meshing_pool::{MeshRequest, MeshResult, MeshingPool};
pub use renderer::TriangleRenderer;
pub use world_renderer::WorldRenderer;
pub mod mesh_prims; // r340-sl-mesh: shared face/quad primitives
pub mod gpu_pack; // r340-sl-gpu: shared GPU packing/slot recycler
