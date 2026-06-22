//! Chunk renderer — renders a single chunk's mesh.
//!
//! Wires together:
//! * `Swapchain` (with depth attachment)
//! * `DepthBuffer`
//! * `ChunkPipeline` (vertex inputs + depth test + UBO descriptor)
//! * `ChunkMesh` from `mesher::mesh_chunk`
//! * Per-frame uniform buffer for the camera matrix
//!
//! Single chunk only — that's what proves chunk rendering works without
//! committing to multi-chunk management. Adding more chunks later means
//! adding a `Vec<UploadedChunk>` and looping in `record_commands`.

use ash::vk;
use crate::render::camera::Camera;
use crate::render::chunk_pipeline::ChunkPipeline;
use crate::render::context::VulkanContext;
use crate::render::depth::DepthBuffer;
use crate::render::froxel::FroxelVolume;
use crate::render::mesher::{mesh_chunk, mesh_chunk_with_sky_and_model, ChunkMesh, PackedVertex};
use crate::render::shadow_map::ShadowMap;
use crate::render::swapchain::{create_render_pass_with_depth, Swapchain};
use crate::world::chunk::Chunk;
use std::time::Instant;

/// v0.24.2c.10: matched to world_renderer's MAX_FRAMES_IN_FLIGHT.
/// See that module's docstring for the no-latency rationale.
const MAX_FRAMES_IN_FLIGHT: usize = 1;

#[derive(Debug)]
pub enum ChunkRenderError {
    Vulkan(vk::Result),
    Swapchain(crate::render::swapchain::SwapchainError),
    Other(String),
}

impl From<vk::Result> for ChunkRenderError {
    fn from(r: vk::Result) -> Self { ChunkRenderError::Vulkan(r) }
}
impl From<crate::render::swapchain::SwapchainError> for ChunkRenderError {
    fn from(e: crate::render::swapchain::SwapchainError) -> Self { ChunkRenderError::Swapchain(e) }
}

impl std::fmt::Display for ChunkRenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChunkRenderError::Vulkan(r) => write!(f, "Vulkan error: {:?}", r),
            ChunkRenderError::Swapchain(e) => write!(f, "Swapchain: {}", e),
            ChunkRenderError::Other(s) => write!(f, "{}", s),
        }
    }
}
impl std::error::Error for ChunkRenderError {}

pub type Result<T> = std::result::Result<T, ChunkRenderError>;

// CameraUbo layout MUST match what chunk.vert and chunk.frag declare,
// because both shaders are shared between this minimal-bin renderer
// and `WorldRenderer`. We don't actually use most of these fields here
// — the simple test bins (`unendlich-chunk`, `unendlich-palette`)
// don't have atmosphere, sun, or shadow data. Filling the unused
// fields with zeros is fine; the shaders read them but the values
// don't affect the rendered output for these bins (no fog, no
// directional sun multiplier, no shadow sampling).
//
// Adding a field here without a matching update in chunk.vert/frag
// (or vice versa) will desync the layouts and break the descriptor
// set's UBO size at validation time. Keep them in lockstep.
#[repr(C)]
#[derive(Copy, Clone)]
struct CameraUbo {
    view_proj:       [[f32; 4]; 4],
    camera_pos:      [f32; 4],
    sun_dir:         [f32; 4],
    sun_color:       [f32; 4],
    sky_color:       [f32; 4],
    ambient_color:   [f32; 4],
    atmos_params:    [f32; 4],
    wind_params:     [f32; 4],
    viewport_rect:   [f32; 4],
    light_view_proj: [[f32; 4]; 4],
}

struct UploadedMesh {
    vertex_buffer: vk::Buffer,
    vertex_memory: vk::DeviceMemory,
    index_buffer: vk::Buffer,
    index_memory: vk::DeviceMemory,
    index_count: u32,
}

struct FrameSync {
    image_available: vk::Semaphore,
    render_finished: vk::Semaphore,
    in_flight: vk::Fence,
    command_buffer: vk::CommandBuffer,
    /// Per-frame UBO so we can update the camera while the previous frame
    /// is still rendering with its own UBO contents.
    ubo_buffer: vk::Buffer,
    ubo_memory: vk::DeviceMemory,
    ubo_ptr: *mut u8,
    descriptor_set: vk::DescriptorSet,
}

pub struct ChunkRenderer {
    pub render_pass: vk::RenderPass,
    pub swapchain: Swapchain,
    pub depth: DepthBuffer,
    pub pipeline: ChunkPipeline,
    pub command_pool: vk::CommandPool,
    descriptor_pool: vk::DescriptorPool,
    frames: Vec<FrameSync>,
    current_frame: usize,
    pub framebuffer_size: (u32, u32),

    pub camera: Camera,
    pub start_time: Instant,

    /// Placeholder shadow map. The legacy single-chunk renderer doesn't
    /// run a real shadow pre-pass, but the shared chunk pipeline now
    /// has a binding 1 for the shadow sampler — so we provide a
    /// resource to satisfy the descriptor. Cleared once at creation
    /// to depth=1.0 so any sample reads as "fully lit". Image is
    /// otherwise unused by this bin.
    shadow: ShadowMap,

    /// Placeholder froxel volume. Same reason as shadow: the shared
    /// chunk pipeline binds a 3D sampler at binding 2. We allocate
    /// the minimum viable resource and bind it. The volume sits at
    /// its sentinel (alpha=0) value, so the fragment shader's
    /// fallback path runs (per-fragment march) — same visual behavior
    /// as in the world bin.
    froxel: FroxelVolume,

    /// v0.24.20-vertex-pack: minimal chunks-metadata SSBO with exactly
    /// one entry. The vertex shader reads `chunks[gl_InstanceIndex]`
    /// (gl_InstanceIndex = 0 for the direct-draw path's single
    /// instance) to recover the demo chunk's world origin so it can
    /// reconstruct world position from the packed chunk-local vertex.
    /// Populated in `upload_mesh` once we know the chunk's coord.
    chunks_buffer: vk::Buffer,
    chunks_memory: vk::DeviceMemory,
    chunks_ptr: *mut u8,

    mesh: Option<UploadedMesh>,
}

// SAFETY (v0.24.20-vertex-pack): the raw `*mut u8` chunks_ptr is a
// persistent map into HOST_VISIBLE + HOST_COHERENT memory. It's
// written only from the render thread (through `&mut self`) during
// `upload_mesh`. The legacy single-chunk renderer does not share
// this struct across threads; this impl exists to satisfy the
// auto-trait inference now that the struct holds a raw pointer.
unsafe impl Send for ChunkRenderer {}

impl ChunkRenderer {
    pub fn new(ctx: &VulkanContext, framebuffer_size: (u32, u32), camera: Camera) -> Result<Self> {
        let extent = vk::Extent2D { width: framebuffer_size.0, height: framebuffer_size.1 };

        // Build a color-only swapchain first (we need its format), then build
        // the depth buffer at the matching extent, then build the
        // color+depth render pass, then rebuild swapchain framebuffers with
        // depth.
        let initial_render_pass = crate::render::swapchain::create_render_pass(
            &ctx.device, vk::Format::B8G8R8A8_SRGB,
        )?;
        let mut swapchain = Swapchain::new(ctx, initial_render_pass, extent)?;
        // Done with the initial render pass — we'll recreate with depth below.
        unsafe { ctx.device.destroy_render_pass(initial_render_pass, None); }

        let depth = DepthBuffer::new(&ctx.instance, &ctx.device, ctx.physical_device, swapchain.extent)?;
        let render_pass = create_render_pass_with_depth(&ctx.device, swapchain.format, depth.format)?;
        swapchain.rebuild_framebuffers_with_depth(ctx, render_pass, depth.view)?;

        let pipeline = ChunkPipeline::new(&ctx.device, render_pass)?;

        let command_pool = unsafe {
            ctx.device.create_command_pool(
                &vk::CommandPoolCreateInfo::builder()
                    .queue_family_index(ctx.graphics_queue_family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?
        };

        let cmd_buffers = unsafe {
            ctx.device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::builder()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(MAX_FRAMES_IN_FLIGHT as u32),
            )?
        };

        // Descriptor pool sized for MAX_FRAMES_IN_FLIGHT * (1 UBO + 2
        // combined image samplers) — matches the chunk pipeline's
        // three-binding layout (UBO + shadow + froxel). Both samplers
        // are placeholders for this bin (no shadow pre-pass, no froxel
        // fill), but we still satisfy the bindings.
        let descriptor_pool = unsafe {
            let sizes = [
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::UNIFORM_BUFFER,
                    descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
                },
                vk::DescriptorPoolSize {
                    ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
                    descriptor_count: (MAX_FRAMES_IN_FLIGHT * 2) as u32,
                },
                vk::DescriptorPoolSize {
                    // v0.24.20-vertex-pack: binding 3 = chunks SSBO.
                    ty: vk::DescriptorType::STORAGE_BUFFER,
                    descriptor_count: MAX_FRAMES_IN_FLIGHT as u32,
                },
            ];
            ctx.device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::builder()
                    .pool_sizes(&sizes)
                    .max_sets(MAX_FRAMES_IN_FLIGHT as u32),
                None,
            )?
        };

        let dsl = pipeline.descriptor_set_layout;
        let layouts: Vec<vk::DescriptorSetLayout> = vec![dsl; MAX_FRAMES_IN_FLIGHT];
        let descriptor_sets = unsafe {
            ctx.device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::builder()
                    .descriptor_pool(descriptor_pool)
                    .set_layouts(&layouts),
            )?
        };

        // Placeholder shadow map. Created here just to satisfy the
        // chunk pipeline's binding 1 (the shadow sampler). We do a
        // one-shot render pass to clear it to depth=1.0 so when this
        // bin's chunk.frag samples shadow_map, every tap returns
        // "fully lit". The image stays in DEPTH_STENCIL_READ_ONLY_OPTIMAL
        // afterwards because the shadow render pass's `final_layout`
        // is set up that way.
        let shadow = ShadowMap::new(&ctx.instance, &ctx.device, ctx.physical_device)?;
        clear_shadow_map_once(ctx, &shadow, command_pool)?;

        // Placeholder froxel volume for binding 2. Same reason as
        // shadow: shared chunk pipeline binds a 3D sampler at binding
        // 2; we satisfy it with a sentinel-cleared volume so the
        // fragment shader's fallback (per-fragment march) runs.
        let froxel = FroxelVolume::new(&ctx.instance, &ctx.device, ctx.physical_device)?;
        froxel.clear_to_sentinel(&ctx.device, command_pool, ctx.graphics_queue)?;

        // v0.24.20-vertex-pack: tiny chunks-metadata SSBO with exactly
        // one entry (sizeof(ChunkDraw) = 64 bytes). The single-chunk
        // demo's direct-draw uses gl_InstanceIndex = 0, so the shader
        // reads chunks[0] for the chunk's world origin. Initialized to
        // zero here; populated in `upload_mesh` once we know the
        // chunk's coord.
        let chunks_size =
            std::mem::size_of::<crate::render::chunk_indirect::ChunkDraw>() as u64;
        let (chunks_buffer, chunks_memory) = create_host_visible_buffer(
            ctx, chunks_size, vk::BufferUsageFlags::STORAGE_BUFFER,
        )?;
        let chunks_ptr = unsafe {
            ctx.device.map_memory(chunks_memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut u8
        };
        // Initialize to all-zero so the very first frame (before
        // `upload_chunk` writes the real origin) reads a zeroed
        // entry. The mesh isn't bound until upload_chunk completes,
        // so drawing prior to that is a no-op anyway.
        unsafe { std::ptr::write_bytes(chunks_ptr, 0, chunks_size as usize); }

        // Build per-frame sync + UBO + descriptor set wiring.
        let mut frames = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            let image_available = unsafe { ctx.device.create_semaphore(&vk::SemaphoreCreateInfo::builder(), None)? };
            let render_finished = unsafe { ctx.device.create_semaphore(&vk::SemaphoreCreateInfo::builder(), None)? };
            let in_flight = unsafe {
                ctx.device.create_fence(
                    &vk::FenceCreateInfo::builder().flags(vk::FenceCreateFlags::SIGNALED),
                    None,
                )?
            };

            let (ubo_buffer, ubo_memory, ubo_ptr) = create_ubo(ctx)?;

            // Wire descriptor set: binding 0 (UBO), binding 1 (shadow
            // map placeholder), binding 2 (froxel volume placeholder).
            // All stable for the renderer's lifetime so written once.
            let buffer_info = vk::DescriptorBufferInfo::builder()
                .buffer(ubo_buffer)
                .offset(0)
                .range(std::mem::size_of::<CameraUbo>() as u64);
            let buffer_infos = [*buffer_info];

            let shadow_image_info = vk::DescriptorImageInfo::builder()
                .image_view(shadow.view)
                .sampler(shadow.sampler)
                .image_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL);
            let shadow_image_infos = [*shadow_image_info];

            let froxel_image_info = vk::DescriptorImageInfo::builder()
                .image_view(froxel.view)
                .sampler(froxel.sampler)
                .image_layout(vk::ImageLayout::GENERAL);
            let froxel_image_infos = [*froxel_image_info];

            // v0.24.20-vertex-pack: binding 3 (chunks SSBO). The buffer
            // was allocated above; here we just describe it for the
            // descriptor write below.
            let chunks_buffer_info = vk::DescriptorBufferInfo::builder()
                .buffer(chunks_buffer)
                .offset(0)
                .range(std::mem::size_of::<crate::render::chunk_indirect::ChunkDraw>() as u64);
            let chunks_buffer_infos = [*chunks_buffer_info];

            let writes = [
                vk::WriteDescriptorSet::builder()
                    .dst_set(descriptor_sets[i])
                    .dst_binding(0)
                    .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                    .buffer_info(&buffer_infos)
                    .build(),
                vk::WriteDescriptorSet::builder()
                    .dst_set(descriptor_sets[i])
                    .dst_binding(1)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&shadow_image_infos)
                    .build(),
                vk::WriteDescriptorSet::builder()
                    .dst_set(descriptor_sets[i])
                    .dst_binding(2)
                    .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                    .image_info(&froxel_image_infos)
                    .build(),
                // v0.24.20-vertex-pack: binding 3 = chunks-metadata
                // SSBO. The vertex shader reads `chunks[0]` (the
                // single-chunk demo only has one entry) to reconstruct
                // world position from the packed chunk-local vertex.
                vk::WriteDescriptorSet::builder()
                    .dst_set(descriptor_sets[i])
                    .dst_binding(3)
                    .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                    .buffer_info(&chunks_buffer_infos)
                    .build(),
            ];
            unsafe { ctx.device.update_descriptor_sets(&writes, &[]); }

            frames.push(FrameSync {
                image_available, render_finished, in_flight,
                command_buffer: cmd_buffers[i],
                ubo_buffer, ubo_memory, ubo_ptr,
                descriptor_set: descriptor_sets[i],
            });
        }

        Ok(Self {
            render_pass, swapchain, depth, pipeline, command_pool, descriptor_pool,
            frames, current_frame: 0,
            framebuffer_size,
            camera,
            start_time: Instant::now(),
            shadow,
            froxel,
            chunks_buffer, chunks_memory, chunks_ptr,
            mesh: None,
        })
    }

    /// Mesh the chunk and upload its vertex/index buffers. Replaces any
    /// previously-uploaded mesh.
    pub fn upload_chunk(&mut self, ctx: &VulkanContext, chunk: &Chunk) -> Result<()> {
        let mesh = mesh_chunk(chunk);
        log::info!(
            "Meshed chunk ({}, {}): {} vertices, {} indices",
            chunk.coord.x, chunk.coord.z, mesh.vertex_count(), mesh.index_count(),
        );
        self.upload_mesh(ctx, chunk.coord, &mesh)
    }

    /// Same as `upload_chunk` but uses the climate-aware mesher path.
    /// The sky drives per-face lighting; the world model drives the
    /// material-response matrix's climate-driven tinting.
    /// Useful for the palette test bin and other diagnostics that want
    /// to exercise the full visual pipeline on a hand-authored chunk.
    pub fn upload_chunk_with_sky_and_model(
        &mut self,
        ctx: &VulkanContext,
        chunk: &Chunk,
        sky: &crate::sky::Sky,
        world_model: &std::sync::Arc<crate::world::world_model::WorldModel>,
    ) -> Result<()> {
        let mesh = mesh_chunk_with_sky_and_model(chunk, sky, world_model);
        log::info!(
            "Meshed chunk ({}, {}) with model: {} vertices, {} indices",
            chunk.coord.x, chunk.coord.z, mesh.vertex_count(), mesh.index_count(),
        );
        self.upload_mesh(ctx, chunk.coord, &mesh)
    }

    fn upload_mesh(
        &mut self,
        ctx: &VulkanContext,
        coord: crate::world::chunk::ChunkCoord,
        mesh: &ChunkMesh,
    ) -> Result<()> {
        // Wait for any in-flight frame using the previous mesh to finish before
        // we destroy/replace it.
        unsafe { ctx.device.device_wait_idle()?; }
        if let Some(old) = self.mesh.take() {
            unsafe {
                ctx.device.destroy_buffer(old.vertex_buffer, None);
                ctx.device.free_memory(old.vertex_memory, None);
                ctx.device.destroy_buffer(old.index_buffer, None);
                ctx.device.free_memory(old.index_memory, None);
            }
        }

        if mesh.is_empty() {
            log::warn!("Tried to upload an empty mesh — nothing to render.");
            return Ok(());
        }

        // v0.24.20-vertex-pack: convert world-space `Vertex`es to packed
        // chunk-local form for upload.
        let (ox, oy, oz) = coord.block_origin();
        let chunk_origin_f = [ox as f32, oy as f32, oz as f32];
        let packed_verts: Vec<PackedVertex> = mesh.vertices.iter()
            .map(|v| PackedVertex::from_world(v, chunk_origin_f))
            .collect();

        let vert_bytes = packed_verts.len() * std::mem::size_of::<PackedVertex>();
        let idx_bytes = (mesh.index_count() as usize) * std::mem::size_of::<u32>();

        let (vertex_buffer, vertex_memory) = create_host_visible_buffer(
            ctx, vert_bytes as u64, vk::BufferUsageFlags::VERTEX_BUFFER,
        )?;
        let (index_buffer, index_memory) = create_host_visible_buffer(
            ctx, idx_bytes as u64, vk::BufferUsageFlags::INDEX_BUFFER,
        )?;

        unsafe {
            // Vertices (packed 8-byte form).
            let dst = ctx.device.map_memory(vertex_memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())?;
            std::ptr::copy_nonoverlapping(
                packed_verts.as_ptr() as *const u8,
                dst as *mut u8,
                vert_bytes,
            );
            ctx.device.unmap_memory(vertex_memory);

            // Indices.
            let dst = ctx.device.map_memory(index_memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())?;
            std::ptr::copy_nonoverlapping(
                mesh.indices.as_ptr() as *const u8,
                dst as *mut u8,
                idx_bytes,
            );
            ctx.device.unmap_memory(index_memory);

            // v0.24.20-vertex-pack: populate the single ChunkDraw entry
            // the vertex shader reads via `chunks[gl_InstanceIndex=0]`.
            // We only care about `aabb_min.xyz` (which holds chunk
            // origin XZ + tight min_y) and `aabb_min.w` (chunk origin
            // Y). The other fields don't affect rendering when this
            // pipeline is invoked via direct draw rather than indirect.
            let chunk_draw = crate::render::chunk_indirect::ChunkDraw {
                index_count: mesh.index_count(),
                first_index: 0,
                vertex_offset: 0,
                flags: 0,
                aabb_min: [ox as f32, oy as f32, oz as f32, oy as f32],
                aabb_max: [
                    ox as f32 + crate::constants::CHUNK_SIZE as f32,
                    oy as f32 + crate::constants::CHUNK_SIZE as f32,
                    oz as f32 + crate::constants::CHUNK_SIZE as f32,
                    0.0,
                ],
            };
            std::ptr::write(
                self.chunks_ptr as *mut crate::render::chunk_indirect::ChunkDraw,
                chunk_draw,
            );
        }

        self.mesh = Some(UploadedMesh {
            vertex_buffer, vertex_memory,
            index_buffer, index_memory,
            index_count: mesh.index_count(),
        });

        Ok(())
    }

    /// Draw one frame. Returns `Ok(false)` if the swapchain is out of date —
    /// caller should call `recreate_swapchain` and try again.
    pub fn draw_frame(&mut self, ctx: &VulkanContext) -> Result<bool> {
        // Update camera & UBO before recording. We update *this* frame's UBO,
        // which is independent of the previous frame's, so no sync needed
        // beyond the per-frame fence we wait on next.
        let aspect = self.swapchain.extent.width as f32 / self.swapchain.extent.height as f32;
        let vp = self.camera.view_projection(aspect);

        let frame_idx = self.current_frame;
        let frame = &self.frames[frame_idx];

        unsafe {
            ctx.device.wait_for_fences(std::slice::from_ref(&frame.in_flight), true, u64::MAX)?;
        }

        // Now safe to write to this frame's UBO.
        //
        // Most fields are unused by this minimal bin (no atmosphere,
        // no sun, no shadow). Filling with zeros / identity gives the
        // shaders a defined value to read; the output is the same as
        // the pre-atmosphere chunk renderer because nothing in this
        // bin's shader path actually consumes atmos/wind/shadow
        // values for visible effect (the conditional fog/shadow
        // contributions multiply by zero or never sample).
        let identity = {
            let mut m = [[0.0f32; 4]; 4];
            m[0][0] = 1.0; m[1][1] = 1.0; m[2][2] = 1.0; m[3][3] = 1.0;
            m
        };
        let ubo = CameraUbo {
            view_proj: mat4_to_array(&vp),
            camera_pos: [0.0; 4],
            sun_dir: [0.0; 4],
            sun_color: [0.0; 4],
            sky_color: [0.0; 4],
            ambient_color: [0.0; 4],
            atmos_params: [0.0; 4],
            wind_params: [0.0; 4],
            viewport_rect: [
                self.swapchain.extent.width as f32,
                self.swapchain.extent.height as f32,
                0.0, 0.0,
            ],
            light_view_proj: identity,
        };
        unsafe {
            std::ptr::copy_nonoverlapping(
                &ubo as *const _ as *const u8,
                frame.ubo_ptr,
                std::mem::size_of::<CameraUbo>(),
            );
        }

        let acquire = unsafe {
            ctx.swapchain_loader.acquire_next_image(
                self.swapchain.handle, u64::MAX,
                frame.image_available, vk::Fence::null(),
            )
        };
        let image_index = match acquire {
            Ok((idx, _)) => idx,
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => return Ok(false),
            Err(e) => return Err(e.into()),
        };

        unsafe { ctx.device.reset_fences(std::slice::from_ref(&frame.in_flight))?; }

        // Record commands.
        let cmd = frame.command_buffer;
        unsafe {
            ctx.device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;
            ctx.device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::builder().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;

            let clears = [
                vk::ClearValue { color: vk::ClearColorValue { float32: [0.55, 0.75, 0.95, 1.0] } }, // sky
                vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 } },
            ];

            let rp_begin = vk::RenderPassBeginInfo::builder()
                .render_pass(self.render_pass)
                .framebuffer(self.swapchain.framebuffers[image_index as usize])
                .render_area(vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: self.swapchain.extent,
                })
                .clear_values(&clears);

            ctx.device.cmd_begin_render_pass(cmd, &rp_begin, vk::SubpassContents::INLINE);
            ctx.device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline.pipeline);

            let viewport = vk::Viewport {
                x: 0.0, y: 0.0,
                width: self.swapchain.extent.width as f32,
                height: self.swapchain.extent.height as f32,
                min_depth: 0.0, max_depth: 1.0,
            };
            let scissor = vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.swapchain.extent,
            };
            ctx.device.cmd_set_viewport(cmd, 0, std::slice::from_ref(&viewport));
            ctx.device.cmd_set_scissor(cmd, 0, std::slice::from_ref(&scissor));

            ctx.device.cmd_bind_descriptor_sets(
                cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline.layout,
                0, std::slice::from_ref(&frame.descriptor_set), &[],
            );

            if let Some(m) = &self.mesh {
                ctx.device.cmd_bind_vertex_buffers(cmd, 0, &[m.vertex_buffer], &[0]);
                ctx.device.cmd_bind_index_buffer(cmd, m.index_buffer, 0, vk::IndexType::UINT32);
                ctx.device.cmd_draw_indexed(cmd, m.index_count, 1, 0, 0, 0);
            }

            ctx.device.cmd_end_render_pass(cmd);
            ctx.device.end_command_buffer(cmd)?;
        }

        let wait_sems = [frame.image_available];
        let wait_stages = [vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT];
        let signal_sems = [frame.render_finished];
        let cmd_bufs = [cmd];

        let submit = vk::SubmitInfo::builder()
            .wait_semaphores(&wait_sems)
            .wait_dst_stage_mask(&wait_stages)
            .command_buffers(&cmd_bufs)
            .signal_semaphores(&signal_sems);
        unsafe {
            ctx.device.queue_submit(ctx.graphics_queue, std::slice::from_ref(&submit), frame.in_flight)?;
        }

        let swapchains = [self.swapchain.handle];
        let image_indices = [image_index];
        let present_info = vk::PresentInfoKHR::builder()
            .wait_semaphores(&signal_sems)
            .swapchains(&swapchains)
            .image_indices(&image_indices);
        let present = unsafe { ctx.swapchain_loader.queue_present(ctx.present_queue, &present_info) };

        match present {
            Ok(false) => {}
            Ok(true) | Err(vk::Result::ERROR_OUT_OF_DATE_KHR) | Err(vk::Result::SUBOPTIMAL_KHR) => {
                self.advance_frame();
                return Ok(false);
            }
            Err(e) => return Err(e.into()),
        }

        self.advance_frame();
        Ok(true)
    }

    fn advance_frame(&mut self) {
        self.current_frame = (self.current_frame + 1) % MAX_FRAMES_IN_FLIGHT;
    }

    pub fn recreate_swapchain(&mut self, ctx: &VulkanContext, new_size: (u32, u32)) -> Result<()> {
        unsafe { ctx.device.device_wait_idle()?; }

        self.swapchain.destroy(ctx);
        self.depth.destroy(&ctx.device);

        let extent = vk::Extent2D { width: new_size.0, height: new_size.1 };

        // Build new color-only swapchain, then new depth buffer, then rebuild
        // framebuffers with the existing render pass + new depth view.
        let temp_rp = crate::render::swapchain::create_render_pass(
            &ctx.device, vk::Format::B8G8R8A8_SRGB,
        )?;
        self.swapchain = Swapchain::new(ctx, temp_rp, extent)?;
        unsafe { ctx.device.destroy_render_pass(temp_rp, None); }

        self.depth = DepthBuffer::new(&ctx.instance, &ctx.device, ctx.physical_device, self.swapchain.extent)?;
        self.swapchain.rebuild_framebuffers_with_depth(ctx, self.render_pass, self.depth.view)?;
        self.framebuffer_size = new_size;

        Ok(())
    }

    pub fn destroy(&mut self, ctx: &VulkanContext) {
        unsafe {
            let _ = ctx.device.device_wait_idle();

            if let Some(m) = self.mesh.take() {
                ctx.device.destroy_buffer(m.vertex_buffer, None);
                ctx.device.free_memory(m.vertex_memory, None);
                ctx.device.destroy_buffer(m.index_buffer, None);
                ctx.device.free_memory(m.index_memory, None);
            }

            for f in &self.frames {
                // Unmap before destroying.
                ctx.device.unmap_memory(f.ubo_memory);
                ctx.device.destroy_buffer(f.ubo_buffer, None);
                ctx.device.free_memory(f.ubo_memory, None);
                ctx.device.destroy_semaphore(f.image_available, None);
                ctx.device.destroy_semaphore(f.render_finished, None);
                ctx.device.destroy_fence(f.in_flight, None);
            }
            self.frames.clear();

            ctx.device.destroy_descriptor_pool(self.descriptor_pool, None);
            ctx.device.destroy_command_pool(self.command_pool, None);
            // v0.24.20-vertex-pack: release the chunks SSBO.
            ctx.device.unmap_memory(self.chunks_memory);
            ctx.device.destroy_buffer(self.chunks_buffer, None);
            ctx.device.free_memory(self.chunks_memory, None);
            self.pipeline.destroy(&ctx.device);
            self.shadow.destroy(&ctx.device);
            self.froxel.destroy(&ctx.device);
            self.depth.destroy(&ctx.device);
            self.swapchain.destroy(ctx);
            ctx.device.destroy_render_pass(self.render_pass, None);
        }
    }
}

/// Clear the shadow map's depth image to 1.0 (= "fully lit") once at
/// startup. The legacy bin (`unendlich-chunk` / `unendlich-palette`)
/// doesn't run a real shadow pre-pass, so without this clear the
/// shadow image's contents would be UNDEFINED — the chunk fragment
/// shader's `texture(shadow_map, ...)` calls would read garbage and
/// the bin's render output could appear arbitrarily darkened.
///
/// We do a one-shot command-buffer submission that begins the shadow
/// render pass (which has `load_op = CLEAR` on its depth attachment)
/// and immediately ends it. The render-pass `final_layout` then
/// transitions the image to `DEPTH_STENCIL_READ_ONLY_OPTIMAL`, ready
/// for sampling.
///
/// Cost: one synchronous queue submit at startup. Negligible — the
/// legacy bins are throwaway test paths.
fn clear_shadow_map_once(
    ctx: &VulkanContext,
    shadow: &ShadowMap,
    command_pool: vk::CommandPool,
) -> Result<()> {
    let cmd = unsafe {
        ctx.device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::builder()
                .command_pool(command_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1),
        )?[0]
    };
    unsafe {
        ctx.device.begin_command_buffer(
            cmd,
            &vk::CommandBufferBeginInfo::builder()
                .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
        )?;
        let clears = [
            vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: 1.0, stencil: 0 } },
        ];
        let rp_begin = vk::RenderPassBeginInfo::builder()
            .render_pass(shadow.render_pass)
            .framebuffer(shadow.framebuffer)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: shadow.extent,
            })
            .clear_values(&clears);
        ctx.device.cmd_begin_render_pass(cmd, &rp_begin, vk::SubpassContents::INLINE);
        // No draws — just the clear from load_op + the layout
        // transition from final_layout.
        ctx.device.cmd_end_render_pass(cmd);
        ctx.device.end_command_buffer(cmd)?;

        let cmd_bufs = [cmd];
        let submit = vk::SubmitInfo::builder().command_buffers(&cmd_bufs);
        ctx.device.queue_submit(
            ctx.graphics_queue,
            std::slice::from_ref(&submit),
            vk::Fence::null(),
        )?;
        ctx.device.queue_wait_idle(ctx.graphics_queue)?;
        ctx.device.free_command_buffers(command_pool, &cmd_bufs);
    }
    Ok(())
}

fn mat4_to_array(m: &nalgebra_glm::Mat4) -> [[f32; 4]; 4] {
    let s = m.as_slice(); // column-major, length 16
    [
        [s[0], s[1], s[2], s[3]],
        [s[4], s[5], s[6], s[7]],
        [s[8], s[9], s[10], s[11]],
        [s[12], s[13], s[14], s[15]],
    ]
}

fn create_ubo(ctx: &VulkanContext) -> Result<(vk::Buffer, vk::DeviceMemory, *mut u8)> {
    let size = std::mem::size_of::<CameraUbo>() as u64;
    let (buffer, memory) = create_host_visible_buffer(ctx, size, vk::BufferUsageFlags::UNIFORM_BUFFER)?;
    let ptr = unsafe {
        ctx.device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut u8
    };
    Ok((buffer, memory, ptr))
}

fn create_host_visible_buffer(
    ctx: &VulkanContext,
    size: u64,
    usage: vk::BufferUsageFlags,
) -> Result<(vk::Buffer, vk::DeviceMemory)> {
    let info = vk::BufferCreateInfo::builder()
        .size(size)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = unsafe { ctx.device.create_buffer(&info, None)? };

    let req = unsafe { ctx.device.get_buffer_memory_requirements(buffer) };
    let props = unsafe { ctx.instance.get_physical_device_memory_properties(ctx.physical_device) };

    let mut chosen = None;
    for i in 0..props.memory_type_count {
        let mt = props.memory_types[i as usize];
        if req.memory_type_bits & (1 << i) != 0
            && mt.property_flags.contains(vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT)
        {
            chosen = Some(i);
            break;
        }
    }
    let mt = chosen.ok_or_else(|| ChunkRenderError::Other("No host-visible memory type".into()))?;

    let alloc = vk::MemoryAllocateInfo::builder()
        .allocation_size(req.size)
        .memory_type_index(mt);
    let memory = unsafe { ctx.device.allocate_memory(&alloc, None)? };
    unsafe { ctx.device.bind_buffer_memory(buffer, memory, 0)?; }

    Ok((buffer, memory))
}
