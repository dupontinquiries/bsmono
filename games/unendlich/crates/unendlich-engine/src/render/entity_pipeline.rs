//! Graphics pipeline for entities (mobs, dropped items). Renders the per-frame
//! vertex bake from `world::entity::snapshot_to_vertices` through the
//! `entity.vert` / `entity.frag` shaders, into the main render pass alongside
//! chunks, reusing the chunk pipeline's camera descriptor-set layout (the entity
//! shaders read the same `CameraUbo` at set 0, binding 0).
//!
//! The vertex bake is regenerated each frame on the CPU and uploaded into a
//! host-visible buffer that grows on demand. Entity counts are small (dozens),
//! so a per-frame re-bake is cheaper than maintaining persistent per-entity
//! buffers. The decode + shaders are proven in `vk-harness/tests/entity_render.rs`.
//!
//! Caller contract: `upload` must run before `record`.
//!
//! r-entity-uaf: `upload`'s buffer-growth path used to `destroy_buffer` the old
//! `vbuf`/`vmem` immediately and replace them, on the stated assumption that
//! the caller had already waited on the frame's in-flight fence, making the
//! (single, not per-frame-in-flight) buffer safe to free. That was true when
//! `frames_in_flight` was hardcoded to 1 (each `draw_frame` fully drained the
//! GPU before returning, so the wait from the *previous* iteration covered
//! everything outstanding). It stopped being true once `frames_in_flight`
//! defaulted to 2 (r476, for CPU/GPU overlap): with two slots, the *other*
//! slot's command buffer -- which also binds this same shared, non-per-slot
//! buffer -- can still be executing on the GPU when growth frees it out from
//! under it. There is also no public API to wait for a specific frame's fence
//! independently of calling `draw_frame`, so the old contract was not
//! satisfiable by any correct caller once FIF > 1, only accidentally safe at
//! FIF == 1. Fixed by retiring (not destroying) the old buffer for
//! `frames_in_flight` frames, mirroring `WorldRenderer`'s `PendingDestroy`
//! discipline for chunk mesh/indirect slots -- see `process_retired`.

use ash::{vk, Device};
use std::ffi::CStr;

use crate::world::entity::EntityVertex;

const VERT_SPV: &[u8] = include_bytes!("../../../../assets/shaders/entity.vert.spv");
const FRAG_SPV: &[u8] = include_bytes!("../../../../assets/shaders/entity.frag.spv");

/// A retired `(buffer, memory)` pair, kept alive until the GPU is certain to
/// have moved past any command buffer that could have referenced it. See the
/// r-entity-uaf module doc for why this exists and how the countdown is safe.
struct RetiredBuffer {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    frames_remaining: usize,
}

pub struct EntityPipeline {
    pub pipeline: vk::Pipeline,
    pub layout: vk::PipelineLayout,
    mem_props: vk::PhysicalDeviceMemoryProperties,
    vbuf: vk::Buffer,
    vmem: vk::DeviceMemory,
    vcap: u64,
    ibuf: vk::Buffer,
    imem: vk::DeviceMemory,
    icap: u64,
    index_count: u32,
    /// How many frames a retired buffer must survive before it's actually
    /// destroyed. Set from `WorldRenderer`'s own `frames_in_flight` at
    /// construction (same source of truth chunk `PendingDestroy` uses), not
    /// hardcoded, so `UNENDLICH_FRAMES_IN_FLIGHT=1` gets the old (correct,
    /// slightly cheaper) immediate-free behavior back automatically.
    frames_in_flight: usize,
    /// Buffers replaced by a growth event, not yet safe to actually free.
    /// Drained by `process_retired`, called once per frame from the same
    /// spot `WorldRenderer::process_pending_destroys` runs.
    retired: Vec<RetiredBuffer>,
}

impl EntityPipeline {
    pub fn new(
        device: &Device,
        render_pass: vk::RenderPass,
        camera_set_layout: vk::DescriptorSetLayout,
        mem_props: vk::PhysicalDeviceMemoryProperties,
        frames_in_flight: usize,
    ) -> Result<Self, vk::Result> {
        Self::new_with(device, render_pass, camera_set_layout, mem_props, frames_in_flight, true)
    }

    /// `depth_write = false` makes an EFFECTS pipeline: same shaders, vertex
    /// format and blending, depth-tested against the world but not writing
    /// depth, so overlapping translucent effects (dragon smoke wisps) blend
    /// through each other instead of the nearest one hiding the rest.
    pub fn new_with(
        device: &Device,
        render_pass: vk::RenderPass,
        camera_set_layout: vk::DescriptorSetLayout,
        mem_props: vk::PhysicalDeviceMemoryProperties,
        frames_in_flight: usize,
        depth_write: bool,
    ) -> Result<Self, vk::Result> {
        let vert_module = create_shader_module(device, VERT_SPV)?;
        let frag_module = create_shader_module(device, FRAG_SPV)?;
        let entry = unsafe { CStr::from_bytes_with_nul_unchecked(b"main\0") };

        let stages = [
            vk::PipelineShaderStageCreateInfo::builder()
                .stage(vk::ShaderStageFlags::VERTEX).module(vert_module).name(entry).build(),
            vk::PipelineShaderStageCreateInfo::builder()

                .stage(vk::ShaderStageFlags::FRAGMENT).module(frag_module).name(entry).build(),
        ];

        // EntityVertex: position[3] f32 @0, color[3] f32 @12, alpha f32 @24, stride 28.
        let binding = vk::VertexInputBindingDescription {
            binding: 0,
            stride: 28,
            input_rate: vk::VertexInputRate::VERTEX,
        };
        let attrs = [
            vk::VertexInputAttributeDescription { binding: 0, location: 0, format: vk::Format::R32G32B32_SFLOAT, offset: 0 },
            vk::VertexInputAttributeDescription { binding: 0, location: 1, format: vk::Format::R32G32B32_SFLOAT, offset: 12 },
            // r535: per-vertex alpha for real transparency (avatar fade).
            vk::VertexInputAttributeDescription { binding: 0, location: 2, format: vk::Format::R32_SFLOAT, offset: 24 },
        ];
        let bindings = [binding];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::builder()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attrs);

        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::builder()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
            .primitive_restart_enable(false);

        let viewport_state = vk::PipelineViewportStateCreateInfo::builder()
            .viewport_count(1).scissor_count(1);

        // Cull NONE: mobs are few and should be visible from any angle even if a
        // face winding is off; revisit once the bake's winding is confirmed.
        let rasterizer = vk::PipelineRasterizationStateCreateInfo::builder()
            .depth_clamp_enable(false)
            .rasterizer_discard_enable(false)
            .polygon_mode(vk::PolygonMode::FILL)
            .line_width(1.0)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::CLOCKWISE)
            .depth_bias_enable(false);

        let multisample = vk::PipelineMultisampleStateCreateInfo::builder()
            .sample_shading_enable(false)
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);

        // Depth like chunks so entities occlude / are occluded by the world.
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::builder()
            .depth_test_enable(true)
            .depth_write_enable(depth_write)
            .depth_compare_op(vk::CompareOp::LESS)
            .depth_bounds_test_enable(false)
            .stencil_test_enable(false);

        // r535: alpha blending ON so per-vertex alpha gives real transparency.
        // Opaque entities (alpha = 1.0) are unaffected: src*1 + dst*0 == src. The
        // avatar (alpha < 1) blends over the already-drawn opaque world. Depth
        // write stays ON, so overlapping triangles of the same translucent body
        // don't double-composite (nearest wins), keeping the fade visually clean.
        let color_blend_attachment = vk::PipelineColorBlendAttachmentState::builder()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ZERO)
            .alpha_blend_op(vk::BlendOp::ADD)
            .build();
        let color_blend_attachments = [color_blend_attachment];
        let color_blend = vk::PipelineColorBlendStateCreateInfo::builder()
            .logic_op_enable(false)
            .attachments(&color_blend_attachments);

        let dyn_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state = vk::PipelineDynamicStateCreateInfo::builder().dynamic_states(&dyn_states);

        // Reuse the chunk pipeline's camera set layout (identical definition), so
        // the same camera descriptor set binds here. entity.vert reads binding 0.
        let set_layouts = [camera_set_layout];
        let layout_info = vk::PipelineLayoutCreateInfo::builder().set_layouts(&set_layouts);
        let layout = unsafe { device.create_pipeline_layout(&layout_info, None)? };

        let info = vk::GraphicsPipelineCreateInfo::builder()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterizer)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth_stencil)
            .color_blend_state(&color_blend)
            .dynamic_state(&dynamic_state)
            .layout(layout)
            .render_pass(render_pass)
            .subpass(0);

        let pipelines = unsafe {
            device.create_graphics_pipelines(vk::PipelineCache::null(), std::slice::from_ref(&info), None)
        }.map_err(|(_, e)| e)?;

        unsafe {
            device.destroy_shader_module(vert_module, None);
            device.destroy_shader_module(frag_module, None);
        }

        // Small initial buffers; upload grows them as needed.
        let (vbuf, vmem) = make_host_buffer(device, &mem_props, 4096, vk::BufferUsageFlags::VERTEX_BUFFER)?;
        let (ibuf, imem) = make_host_buffer(device, &mem_props, 4096, vk::BufferUsageFlags::INDEX_BUFFER)?;

        Ok(Self {
            pipeline: pipelines[0], layout, mem_props,
            vbuf, vmem, vcap: 4096, ibuf, imem, icap: 4096, index_count: 0,
            frames_in_flight: frames_in_flight.max(1),
            retired: Vec::new(),
        })
    }

    /// Re-bake upload for this frame. Grows the buffers if the bake outgrew
    /// them. Safe to call any time before `record` for the frame currently
    /// being built -- growth no longer frees the old buffer immediately (see
    /// the r-entity-uaf module doc); it's retired instead. Call
    /// `process_retired` once per real frame (from the same spot
    /// `WorldRenderer::process_pending_destroys` runs) to actually reclaim
    /// retired buffers once it's safe.
    pub fn upload(&mut self, device: &Device, verts: &[EntityVertex], indices: &[u32]) -> Result<(), vk::Result> {
        // Pack vertices field-by-field so the GPU layout is exact regardless of
        // EntityVertex's Rust repr: [px,py,pz, cr,cg,cb, a] f32, 28 bytes each.
        let mut vbytes = Vec::with_capacity(verts.len() * 28);
        for v in verts {
            for f in v.position.iter().chain(v.color.iter()) {
                vbytes.extend_from_slice(&f.to_le_bytes());
            }
            vbytes.extend_from_slice(&v.alpha.to_le_bytes());
        }
        let ibytes: Vec<u8> = indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        self.index_count = indices.len() as u32;

        if !vbytes.is_empty() {
            if vbytes.len() as u64 > self.vcap {
                // r-entity-uaf: retire, don't destroy -- a previous frame's
                // command buffer (a different FIF slot than whichever one is
                // "current" right now) may still be executing on the GPU and
                // may still bind this exact buffer, since vbuf/vmem are
                // shared across all slots rather than duplicated per-slot.
                self.retired.push(RetiredBuffer {
                    buffer: self.vbuf, memory: self.vmem,
                    frames_remaining: self.frames_in_flight,
                });
                let cap = (vbytes.len() as u64).next_power_of_two();
                let (b, m) = make_host_buffer(device, &self.mem_props, cap, vk::BufferUsageFlags::VERTEX_BUFFER)?;
                self.vbuf = b; self.vmem = m; self.vcap = cap;
            }
            write_host(device, self.vmem, &vbytes)?;
        }
        if !ibytes.is_empty() {
            if ibytes.len() as u64 > self.icap {
                // r-entity-uaf: see the vbuf retirement above -- same reasoning.
                self.retired.push(RetiredBuffer {
                    buffer: self.ibuf, memory: self.imem,
                    frames_remaining: self.frames_in_flight,
                });
                let cap = (ibytes.len() as u64).next_power_of_two();
                let (b, m) = make_host_buffer(device, &self.mem_props, cap, vk::BufferUsageFlags::INDEX_BUFFER)?;
                self.ibuf = b; self.imem = m; self.icap = cap;
            }

            write_host(device, self.imem, &ibytes)?;
        }
        Ok(())
    }

    /// Record the entity draw into an in-progress render pass. `camera_set` is the
    /// same descriptor set the chunk pass binds.
    pub fn record(&self, device: &Device, cmd: vk::CommandBuffer, camera_set: vk::DescriptorSet) {
        if self.index_count == 0 {
            return;
        }
        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_descriptor_sets(cmd, vk::PipelineBindPoint::GRAPHICS, self.layout, 0, &[camera_set], &[]);
            device.cmd_bind_vertex_buffers(cmd, 0, &[self.vbuf], &[0]);
            device.cmd_bind_index_buffer(cmd, self.ibuf, 0, vk::IndexType::UINT32);
            device.cmd_draw_indexed(cmd, self.index_count, 1, 0, 0, 0);
        }
    }

    /// Tick retired buffers' countdown and actually free any that reach 0.
    /// Call once per real frame -- the same spot
    /// `WorldRenderer::process_pending_destroys` runs from is exactly right,
    /// since that's already established as safe timing for this class of
    /// deferred free (see the r-entity-uaf module doc).
    pub fn process_retired(&mut self, device: &Device) {
        let mut i = 0;
        while i < self.retired.len() {
            if self.retired[i].frames_remaining == 0 {
                let r = self.retired.swap_remove(i);
                unsafe { destroy_buffer(device, r.buffer, r.memory); }
            } else {
                self.retired[i].frames_remaining -= 1;
                i += 1;
            }
        }
    }

    pub fn destroy(&mut self, device: &Device) {
        unsafe {
            destroy_buffer(device, self.vbuf, self.vmem);
            destroy_buffer(device, self.ibuf, self.imem);
            // r-entity-uaf: anything still in `retired` at shutdown -- by
            // this point the caller (WorldRenderer::destroy) has already
            // done a device_wait_idle, so it's unconditionally safe to free
            // these now regardless of remaining countdown.
            for r in self.retired.drain(..) {
                destroy_buffer(device, r.buffer, r.memory);
            }
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
        }
    }
}

fn create_shader_module(device: &Device, bytes: &[u8]) -> Result<vk::ShaderModule, vk::Result> {
    assert_eq!(bytes.len() % 4, 0, "SPIR-V size must be a multiple of 4");
    let words: Vec<u32> = bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let info = vk::ShaderModuleCreateInfo::builder().code(&words);
    unsafe { device.create_shader_module(&info, None) }
}

fn find_memory_type(filter: u32, flags: vk::MemoryPropertyFlags, props: &vk::PhysicalDeviceMemoryProperties) -> Option<u32> {
    for i in 0..props.memory_type_count {
        if filter & (1 << i) != 0 && props.memory_types[i as usize].property_flags.contains(flags) {
            return Some(i);
        }
    }
    None
}

fn make_host_buffer(
    device: &Device,
    props: &vk::PhysicalDeviceMemoryProperties,
    size: u64,
    usage: vk::BufferUsageFlags,
) -> Result<(vk::Buffer, vk::DeviceMemory), vk::Result> {
    unsafe {
        let buf = device.create_buffer(
            &vk::BufferCreateInfo::builder().size(size).usage(usage).sharing_mode(vk::SharingMode::EXCLUSIVE),
            None,
        )?;
        let req = device.get_buffer_memory_requirements(buf);
        let mt = find_memory_type(
            req.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            props,
        ).ok_or(vk::Result::ERROR_FEATURE_NOT_PRESENT)?;
        let mem = device.allocate_memory(
            &vk::MemoryAllocateInfo::builder().allocation_size(req.size).memory_type_index(mt),
            None,
        )?;
        device.bind_buffer_memory(buf, mem, 0)?;
        Ok((buf, mem))
    }
}

fn write_host(device: &Device, mem: vk::DeviceMemory, bytes: &[u8]) -> Result<(), vk::Result> {
    unsafe {
        let ptr = device.map_memory(mem, 0, bytes.len() as u64, vk::MemoryMapFlags::empty())?;
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
        device.unmap_memory(mem);
    }
    Ok(())
}

unsafe fn destroy_buffer(device: &Device, buf: vk::Buffer, mem: vk::DeviceMemory) {
    device.destroy_buffer(buf, None);
    device.free_memory(mem, None);
}
