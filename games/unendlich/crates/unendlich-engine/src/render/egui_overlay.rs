//! `egui_overlay` — a minimal, hand-rolled egui renderer for the engine's raw
//! Vulkan (ash 0.37, egui 0.34). Gated behind the `ui-overlay` feature. See
//! docs/EGUI-OVERLAY.md for the full contract; this is implementation order
//! step 1 — *the module compiles and can paint egui meshes*. It is not yet
//! called from the render loop (step 2) and there is no blur yet (step 3).
//!
//! UNVERIFIED: written blind (no Rust/Vulkan toolchain here). Build with
//! `--features ui-overlay`; the likely nudges are exact ash 0.37 builder methods
//! and a couple of egui 0.34 names (`FontImage::srgba_pixels`, `ImageData`
//! variants, `Context::run`/`tessellate`). The Vulkan idioms mirror
//! chunk_pipeline.rs / depth.rs / froxel.rs / chunk_renderer.rs.

use crate::render::context::VulkanContext;
use ash::vk;
use std::collections::HashMap;
use std::ffi::CStr;

// Generous fixed capacities for the first cut (a hello-world dialog is tiny).
// Grow-on-demand is a later refinement; for now we clamp.
const MAX_VERTICES: usize = 64 * 1024;
const MAX_INDICES: usize = 128 * 1024;
const VERTEX_SIZE: usize = std::mem::size_of::<egui::epaint::Vertex>(); // repr(C) = 20

#[cfg(not(feature = "hot-shaders"))]
const EGUI_VERT_SPV: &[u8] = include_bytes!("../../../../assets/shaders/egui.vert.spv");
#[cfg(not(feature = "hot-shaders"))]
const EGUI_FRAG_SPV: &[u8] = include_bytes!("../../../../assets/shaders/egui.frag.spv");

struct EguiTexture {
    image: vk::Image,
    memory: vk::DeviceMemory,
    view: vk::ImageView,
    set: vk::DescriptorSet,
    /// Full atlas dimensions, so partial (delta.pos) sub-rect updates can be
    /// validated to fit and copied into the existing image at the offset.
    width: u32,
    height: u32,
    /// r230: when true this entry only owns its descriptor `set` — the
    /// image/memory/view are borrowed from an engine-owned render target (the
    /// blurred-scene texture for frosted glass). Cleanup frees only the set.
    external: bool,
}

/// One tessellated egui primitive resolved to a single indexed draw with its own
/// scissor + texture. Built in `prepare`, consumed in `record`.
struct DrawCmd {
    clip: egui::Rect,
    tex: egui::TextureId,
    first_index: u32,
    index_count: u32,
    vertex_offset: i32,
}

/// Host-visible vertex+index buffers for ONE frame-in-flight slot. We keep a ring
/// of these (one per FIF slot) so frame N's `prepare` writes a different slot than
/// the one frame N-1's GPU draw is still reading — no write-while-in-use hazard.
struct FrameBuffers {
    vbuf: vk::Buffer,
    vmem: vk::DeviceMemory,
    vptr: *mut u8,
    ibuf: vk::Buffer,
    imem: vk::DeviceMemory,
    iptr: *mut u8,
}

pub struct EguiOverlay {
    pub ctx: egui::Context,
    pipeline: vk::Pipeline,
    pipeline_layout: vk::PipelineLayout,
    set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    sampler: vk::Sampler,
    textures: HashMap<egui::TextureId, EguiTexture>,
    /// Per-frame-in-flight vertex/index buffers (ring). Indexed by `slot`.
    frames: Vec<FrameBuffers>,
    /// Frame slot written by the last `prepare`, replayed by `record`.
    slot: usize,
    /// Draw list prepared this frame (filled by `prepare`, drained by `record`).
    pending: Vec<DrawCmd>,
    /// pixels_per_point captured at `prepare` time, reused by `record`.
    prepared_ppp: f32,
}

fn create_shader_module(device: &ash::Device, bytes: &[u8]) -> Result<vk::ShaderModule, vk::Result> {
    let words: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let info = vk::ShaderModuleCreateInfo::builder().code(&words);
    unsafe { device.create_shader_module(&info, None) }
}

fn pick_memory_type(
    vk: &VulkanContext,
    bits: u32,
    props: vk::MemoryPropertyFlags,
) -> Option<u32> {
    let mp = unsafe {
        vk.instance
            .get_physical_device_memory_properties(vk.physical_device)
    };
    (0..mp.memory_type_count).find(|&i| {
        bits & (1 << i) != 0 && mp.memory_types[i as usize].property_flags.contains(props)
    })
}

fn host_buffer(
    vk: &VulkanContext,
    size: u64,
    usage: vk::BufferUsageFlags,
) -> Result<(vk::Buffer, vk::DeviceMemory, *mut u8), vk::Result> {
    let info = vk::BufferCreateInfo::builder()
        .size(size)
        .usage(usage)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = unsafe { vk.device.create_buffer(&info, None)? };
    let req = unsafe { vk.device.get_buffer_memory_requirements(buffer) };
    let mt = pick_memory_type(
        vk,
        req.memory_type_bits,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    )
    .ok_or(vk::Result::ERROR_FEATURE_NOT_PRESENT)?;
    let alloc = vk::MemoryAllocateInfo::builder()
        .allocation_size(req.size)
        .memory_type_index(mt);
    let memory = unsafe { vk.device.allocate_memory(&alloc, None)? };
    unsafe { vk.device.bind_buffer_memory(buffer, memory, 0)? };
    let ptr = unsafe {
        vk.device
            .map_memory(memory, 0, req.size, vk::MemoryMapFlags::empty())? as *mut u8
    };
    Ok((buffer, memory, ptr))
}

impl EguiOverlay {
    /// Create the pipeline + resources. `render_pass` is the (color-only, LOAD)
    /// UI pass the overlay draws into — see docs/EGUI-OVERLAY.md frame flow.
    pub fn new(
        vk: &VulkanContext,
        render_pass: vk::RenderPass,
        frames_in_flight: usize,
    ) -> Result<Self, vk::Result> {
        let device = &vk.device;

        // descriptor set layout: one combined image sampler (the texture).
        let bindings = [vk::DescriptorSetLayoutBinding::builder()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)
            .build()];
        let set_layout = unsafe {
            device.create_descriptor_set_layout(
                &vk::DescriptorSetLayoutCreateInfo::builder().bindings(&bindings),
                None,
            )?
        };

        // pipeline layout: push constant = vec2 screen_size (points), vertex stage.
        let push_ranges = [vk::PushConstantRange::builder()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(8)
            .build()];
        let set_layouts = [set_layout];
        let pipeline_layout = unsafe {
            device.create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::builder()
                    .set_layouts(&set_layouts)
                    .push_constant_ranges(&push_ranges),
                None,
            )?
        };

        // shader modules (mirror chunk_pipeline: embedded .spv normally, disk read
        // when hot-shaders is on)
        #[cfg(not(feature = "hot-shaders"))]
        let (vert, frag) = (
            create_shader_module(device, EGUI_VERT_SPV)?,
            create_shader_module(device, EGUI_FRAG_SPV)?,
        );
        #[cfg(feature = "hot-shaders")]
        let (vert, frag) = {
            let v = crate::render::hot_shaders::read_spv("egui.vert.spv");
            let f = crate::render::hot_shaders::read_spv("egui.frag.spv");
            (create_shader_module(device, &v)?, create_shader_module(device, &f)?)
        };
        let entry = CStr::from_bytes_with_nul(b"main\0").unwrap();
        let stages = [
            vk::PipelineShaderStageCreateInfo::builder()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert)
                .name(entry)
                .build(),
            vk::PipelineShaderStageCreateInfo::builder()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(frag)
                .name(entry)
                .build(),
        ];

        // vertex input: epaint::Vertex { pos:vec2, uv:vec2, color:rgba8 } = 20 bytes
        let bind_desc = [vk::VertexInputBindingDescription::builder()
            .binding(0)
            .stride(VERTEX_SIZE as u32)
            .input_rate(vk::VertexInputRate::VERTEX)
            .build()];
        let attr_desc = [
            vk::VertexInputAttributeDescription::builder()
                .location(0)
                .binding(0)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(0)
                .build(),
            vk::VertexInputAttributeDescription::builder()
                .location(1)
                .binding(0)
                .format(vk::Format::R32G32_SFLOAT)
                .offset(8)
                .build(),
            vk::VertexInputAttributeDescription::builder()
                .location(2)
                .binding(0)
                .format(vk::Format::R8G8B8A8_UNORM)
                .offset(16)
                .build(),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::builder()
            .vertex_binding_descriptions(&bind_desc)
            .vertex_attribute_descriptions(&attr_desc);

        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::builder()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST);

        // viewport/scissor are dynamic (set per-prim at draw); counts are fixed.
        let viewport_state = vk::PipelineViewportStateCreateInfo::builder()
            .viewport_count(1)
            .scissor_count(1);
        let dynamic = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state =
            vk::PipelineDynamicStateCreateInfo::builder().dynamic_states(&dynamic);

        let raster = vk::PipelineRasterizationStateCreateInfo::builder()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::builder()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);

        // straight-alpha blend (sRGB/premultiply tuning is a screenshot item).
        let blend_attachment = [vk::PipelineColorBlendAttachmentState::builder()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::SRC_ALPHA)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .build()];
        let color_blend =
            vk::PipelineColorBlendStateCreateInfo::builder().attachments(&blend_attachment);

        // The world's main render pass (subpass 0) has a depth attachment, so any
        // pipeline used there MUST supply a depth-stencil state — but UI sits on
        // top of everything, so test + write are off (compare ALWAYS).
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::builder()
            .depth_test_enable(false)
            .depth_write_enable(false)
            .depth_compare_op(vk::CompareOp::ALWAYS)
            .depth_bounds_test_enable(false)
            .stencil_test_enable(false);

        let info = vk::GraphicsPipelineCreateInfo::builder()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .depth_stencil_state(&depth_stencil)
            .color_blend_state(&color_blend)
            .dynamic_state(&dynamic_state)
            .layout(pipeline_layout)
            .render_pass(render_pass)
            .subpass(0)
            .build();
        let pipeline = unsafe {
            device
                .create_graphics_pipelines(vk::PipelineCache::null(), &[info], None)
                .map_err(|(_, e)| e)?[0]
        };
        unsafe {
            device.destroy_shader_module(vert, None);
            device.destroy_shader_module(frag, None);
        }

        let sampler = unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo::builder()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )?
        };

        let pool_sizes = [vk::DescriptorPoolSize::builder()
            .ty(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(16)
            .build()];
        let descriptor_pool = unsafe {
            device.create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::builder()
                    .flags(vk::DescriptorPoolCreateFlags::FREE_DESCRIPTOR_SET)
                    .max_sets(16)
                    .pool_sizes(&pool_sizes),
                None,
            )?
        };

        // One vertex/index buffer pair per frame-in-flight slot (ring).
        let mut frames = Vec::with_capacity(frames_in_flight.max(1));
        for _ in 0..frames_in_flight.max(1) {
            let (vbuf, vmem, vptr) = host_buffer(
                vk,
                (MAX_VERTICES * VERTEX_SIZE) as u64,
                vk::BufferUsageFlags::VERTEX_BUFFER,
            )?;
            let (ibuf, imem, iptr) = host_buffer(
                vk,
                (MAX_INDICES * 4) as u64,
                vk::BufferUsageFlags::INDEX_BUFFER,
            )?;
            frames.push(FrameBuffers { vbuf, vmem, vptr, ibuf, imem, iptr });
        }

        // r222→r223: install the custom menu fonts (UnendlichBlocks game
        // font + UnendlichGlyphs UI marks) into the egui font stack once, at
        // context creation, so titles/nav/headings can opt into the blocky
        // face via unendlich_ui::menu::blocks_font(). Body text keeps the
        // default proportional font. Fonts are embedded (no runtime path).
        let ctx = egui::Context::default();
        unendlich_ui::menu::install_fonts(
            &ctx,
            include_bytes!("../../../../docs/design/UnendlichBlocks-Regular.ttf"),
            include_bytes!("../../../../docs/design/UnendlichGlyphs-Regular.ttf"),
        );

        Ok(Self {
            ctx,
            pipeline,
            pipeline_layout,
            set_layout,
            descriptor_pool,
            sampler,
            textures: HashMap::new(),
            frames,
            slot: 0,
            pending: Vec::new(),
            prepared_ppp: 1.0,
        })
    }

    /// PRE-PASS phase: run one egui frame, upload any textures it changed (font
    /// atlas etc — this submits to the queue, so it MUST run before the render
    /// pass begins), tessellate, and fill the vertex/index buffers. The resulting
    /// draw list is stashed in `self.pending` for `record` to replay inside the
    /// pass. Safe to call mid primary-command-buffer recording (the primary isn't
    /// submitted yet), but never between cmd_begin_render_pass / cmd_end_render_pass.
    pub fn prepare(
        &mut self,
        vk: &VulkanContext,
        slot: usize,
        pixels_per_point: f32,
        raw_input: egui::RawInput,
        run_ui: impl FnMut(&egui::Context),
    ) -> Result<(), vk::Result> {
        self.slot = slot % self.frames.len();
        // Lay text out at the SAME pixels_per_point we tessellate + render with.
        // Without this, egui lays out galleys at its default ppp while we
        // tessellate at `pixels_per_point`, so the cached text shapes mismatch
        // and epaint warns "pixels_per_point have changed between text layout
        // and tessellation". set_pixels_per_point applies at the pass begun by
        // run(), so layout uses it.
        self.ctx.set_pixels_per_point(pixels_per_point);
        let output = self.ctx.run(raw_input, run_ui);
        // Tessellate + record at exactly the ppp egui used this pass, so the
        // two never disagree even on the frame the scale changes.
        let ppp = self.ctx.pixels_per_point();
        self.prepared_ppp = ppp;

        // upload/patch textures egui changed this frame
        for (id, delta) in &output.textures_delta.set {
            self.set_texture(vk, *id, delta)?;
        }

        let prims = self.ctx.tessellate(output.shapes, ppp);

        // Concatenate all meshes into the shared vertex/index buffers; one
        // DrawCmd per primitive carries its own scissor + texture.
        let mut draws: Vec<DrawCmd> = Vec::new();
        let mut verts: Vec<egui::epaint::Vertex> = Vec::new();
        let mut indices: Vec<u32> = Vec::new();
        for p in &prims {
            if let egui::epaint::Primitive::Mesh(mesh) = &p.primitive {
                if verts.len() + mesh.vertices.len() > MAX_VERTICES
                    || indices.len() + mesh.indices.len() > MAX_INDICES
                {
                    break; // clamp (first cut)
                }
                draws.push(DrawCmd {
                    clip: p.clip_rect,
                    tex: mesh.texture_id,
                    first_index: indices.len() as u32,
                    index_count: mesh.indices.len() as u32,
                    vertex_offset: verts.len() as i32,
                });
                verts.extend_from_slice(&mesh.vertices);
                indices.extend_from_slice(&mesh.indices);
            }
        }

        if !draws.is_empty() {
            let fb = &self.frames[self.slot];
            unsafe {
                std::ptr::copy_nonoverlapping(
                    verts.as_ptr() as *const u8,
                    fb.vptr,
                    verts.len() * VERTEX_SIZE,
                );
                std::ptr::copy_nonoverlapping(
                    indices.as_ptr() as *const u8,
                    fb.iptr,
                    indices.len() * 4,
                );
            }
        }

        for id in &output.textures_delta.free {
            self.free_texture(vk, *id);
        }
        self.pending = draws;
        Ok(())
    }

    /// IN-PASS phase: replay the draw list prepared by `prepare` into `cmd`, which
    /// must be inside the UI render pass. `fb_px` = framebuffer size in pixels.
    /// Pure recording — no queue submits — so it is safe between
    /// cmd_begin_render_pass and cmd_end_render_pass.
    pub fn record(&self, vk: &VulkanContext, cmd: vk::CommandBuffer, fb_px: [u32; 2]) {
        if self.pending.is_empty() {
            return;
        }
        let pixels_per_point = self.prepared_ppp;
        let fb = &self.frames[self.slot];
        let device = &vk.device;
        unsafe {
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_bind_vertex_buffers(cmd, 0, &[fb.vbuf], &[0]);
            device.cmd_bind_index_buffer(cmd, fb.ibuf, 0, vk::IndexType::UINT32);
            device.cmd_set_viewport(
                cmd,
                0,
                &[vk::Viewport {
                    x: 0.0,
                    y: 0.0,
                    width: fb_px[0] as f32,
                    height: fb_px[1] as f32,
                    min_depth: 0.0,
                    max_depth: 1.0,
                }],
            );
            let screen_pts = [
                fb_px[0] as f32 / pixels_per_point,
                fb_px[1] as f32 / pixels_per_point,
            ];
            device.cmd_push_constants(
                cmd,
                self.pipeline_layout,
                vk::ShaderStageFlags::VERTEX,
                0,
                bytemuck_cast(&screen_pts),
            );
            for d in &self.pending {
                let set = match self.textures.get(&d.tex) {
                    Some(t) => t.set,
                    None => continue,
                };
                // clip rect (points) → physical pixels, clamped to framebuffer
                let min_x = (d.clip.min.x * pixels_per_point).max(0.0) as i32;
                let min_y = (d.clip.min.y * pixels_per_point).max(0.0) as i32;
                let max_x = (d.clip.max.x * pixels_per_point).min(fb_px[0] as f32) as i32;
                let max_y = (d.clip.max.y * pixels_per_point).min(fb_px[1] as f32) as i32;
                if max_x <= min_x || max_y <= min_y {
                    continue;
                }
                device.cmd_set_scissor(
                    cmd,
                    0,
                    &[vk::Rect2D {
                        offset: vk::Offset2D { x: min_x, y: min_y },
                        extent: vk::Extent2D {
                            width: (max_x - min_x) as u32,
                            height: (max_y - min_y) as u32,
                        },
                    }],
                );
                device.cmd_bind_descriptor_sets(
                    cmd,
                    vk::PipelineBindPoint::GRAPHICS,
                    self.pipeline_layout,
                    0,
                    &[set],
                    &[],
                );
                device.cmd_draw_indexed(cmd, d.index_count, 1, d.first_index, d.vertex_offset, 0);
            }
        }
    }

    /// (Re)create + upload an egui texture (font atlas or user image). First cut:
    /// full-image upload (ignores partial `pos` sub-rect updates — fine for a
    /// static label, see docs/EGUI-OVERLAY.md).
    fn set_texture(
        &mut self,
        vk: &VulkanContext,
        id: egui::TextureId,
        delta: &egui::epaint::ImageDelta,
    ) -> Result<(), vk::Result> {
        let (w, h) = (delta.image.width() as u32, delta.image.height() as u32);
        // egui 0.34: ImageData is Color-only (font atlas is pre-converted to a
        // white-RGB + coverage-alpha ColorImage upstream).
        let pixels: Vec<u8> = match &delta.image {
            egui::epaint::ImageData::Color(img) => {
                img.pixels.iter().flat_map(|c| c.to_array()).collect()
            }
        };

        // staging buffer
        let (staging, smem, sptr) = host_buffer(
            vk,
            pixels.len() as u64,
            vk::BufferUsageFlags::TRANSFER_SRC,
        )?;
        unsafe {
            std::ptr::copy_nonoverlapping(pixels.as_ptr(), sptr, pixels.len());
        }

        // PARTIAL UPDATE: egui sends delta.pos = Some([x,y]) when it adds glyphs
        // to an EXISTING atlas without regrowing it. We must copy the sub-rect
        // into the live image at that offset, NOT recreate the whole texture —
        // recreating it at the sub-rect's size was corrupting the atlas, which
        // is why all text rendered as blobs. Full updates (pos = None, e.g. the
        // initial atlas or a regrow) fall through to the (re)create path below.
        if let Some([ox, oy]) = delta.pos {
            if let Some(existing) = self.textures.get(&id) {
                if ox as u32 + w <= existing.width && oy as u32 + h <= existing.height {
                    let image = existing.image;
                    self.one_time(vk, |device, cmd| unsafe {
                        let sub = vk::ImageSubresourceRange {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            base_mip_level: 0, level_count: 1,
                            base_array_layer: 0, layer_count: 1,
                        };
                        let to_dst = vk::ImageMemoryBarrier::builder()
                            .old_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                            .src_access_mask(vk::AccessFlags::SHADER_READ)
                            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                            .image(image).subresource_range(sub).build();
                        device.cmd_pipeline_barrier(
                            cmd, vk::PipelineStageFlags::FRAGMENT_SHADER,
                            vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(),
                            &[], &[], &[to_dst]);
                        let region = vk::BufferImageCopy::builder()
                            .image_subresource(vk::ImageSubresourceLayers {
                                aspect_mask: vk::ImageAspectFlags::COLOR,
                                mip_level: 0, base_array_layer: 0, layer_count: 1,
                            })
                            .image_offset(vk::Offset3D { x: ox as i32, y: oy as i32, z: 0 })
                            .image_extent(vk::Extent3D { width: w, height: h, depth: 1 })
                            .build();
                        device.cmd_copy_buffer_to_image(
                            cmd, staging, image,
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[region]);
                        let to_read = vk::ImageMemoryBarrier::builder()
                            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                            .dst_access_mask(vk::AccessFlags::SHADER_READ)
                            .image(image).subresource_range(sub).build();
                        device.cmd_pipeline_barrier(
                            cmd, vk::PipelineStageFlags::TRANSFER,
                            vk::PipelineStageFlags::FRAGMENT_SHADER, vk::DependencyFlags::empty(),
                            &[], &[], &[to_read]);
                    })?;
                    unsafe {
                        vk.device.destroy_buffer(staging, None);
                        vk.device.free_memory(smem, None);
                    }
                    return Ok(());
                }
                // Sub-rect doesn't fit the recorded dims (shouldn't happen) —
                // fall through and do a full recreate as a safe fallback.
            }
        }

        // image
        let device = &vk.device;
        let image = unsafe {
            device.create_image(
                &vk::ImageCreateInfo::builder()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .extent(vk::Extent3D { width: w, height: h, depth: 1 })
                    .mip_levels(1)
                    .array_layers(1)
                    .samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::SAMPLED)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )?
        };
        let req = unsafe { device.get_image_memory_requirements(image) };
        let mt = pick_memory_type(vk, req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)
            .ok_or(vk::Result::ERROR_FEATURE_NOT_PRESENT)?;
        let memory = unsafe {
            device.allocate_memory(
                &vk::MemoryAllocateInfo::builder()
                    .allocation_size(req.size)
                    .memory_type_index(mt),
                None,
            )?
        };
        unsafe { device.bind_image_memory(image, memory, 0)? };

        // one-time upload: transition → copy → transition (mirrors froxel.rs)
        self.one_time(vk, |device, cmd| unsafe {
            let sub = vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            };
            let to_dst = vk::ImageMemoryBarrier::builder()
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .image(image)
                .subresource_range(sub)
                .build();
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_dst],
            );
            let region = vk::BufferImageCopy::builder()
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_extent(vk::Extent3D { width: w, height: h, depth: 1 })
                .build();
            device.cmd_copy_buffer_to_image(
                cmd,
                staging,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
            );
            let to_read = vk::ImageMemoryBarrier::builder()
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ)
                .image(image)
                .subresource_range(sub)
                .build();
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_read],
            );
        })?;
        unsafe {
            device.destroy_buffer(staging, None);
            device.free_memory(smem, None);
        }

        let view = unsafe {
            device.create_image_view(
                &vk::ImageViewCreateInfo::builder()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(vk::Format::R8G8B8A8_UNORM)
                    .subresource_range(vk::ImageSubresourceRange {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        base_mip_level: 0,
                        level_count: 1,
                        base_array_layer: 0,
                        layer_count: 1,
                    }),
                None,
            )?
        };
        let set_layouts = [self.set_layout];
        let set = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::builder()
                    .descriptor_pool(self.descriptor_pool)
                    .set_layouts(&set_layouts),
            )?[0]
        };
        let image_info = [vk::DescriptorImageInfo::builder()
            .image_view(view)
            .sampler(self.sampler)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .build()];
        let write = [vk::WriteDescriptorSet::builder()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&image_info)
            .build()];
        unsafe { device.update_descriptor_sets(&write, &[]) };

        // replace any previous texture for this id
        if let Some(old) = self.textures.insert(id, EguiTexture { image, memory, view, set, width: w, height: h, external: false }) {
            self.destroy_texture(vk, old);
        }
        Ok(())
    }

    fn free_texture(&mut self, vk: &VulkanContext, id: egui::TextureId) {
        if let Some(t) = self.textures.remove(&id) {
            self.destroy_texture(vk, t);
        }
    }

    /// r230: expose an ENGINE-OWNED image (view + sampler) to egui as a
    /// `TextureId::User`, so menu UI can sample it (the blurred-scene texture
    /// for frosted glass). The image/memory/view are NOT owned here — egui
    /// only allocates a descriptor set pointing at them. Re-call with the same
    /// `slot` after a swapchain resize to repoint at the new view; the old
    /// descriptor set is freed. The view MUST be in SHADER_READ_ONLY_OPTIMAL
    /// when the egui pass runs. Returns the stable `TextureId` to hand to the
    /// UI (e.g. via `Surface::frosted_blur`).
    pub fn register_external_texture(
        &mut self,
        vk: &VulkanContext,
        slot: u64,
        view: vk::ImageView,
        sampler: vk::Sampler,
    ) -> Result<egui::TextureId, vk::Result> {
        let device = &vk.device;
        let id = egui::TextureId::User(slot);
        let set_layouts = [self.set_layout];
        let set = unsafe {
            device.allocate_descriptor_sets(
                &vk::DescriptorSetAllocateInfo::builder()
                    .descriptor_pool(self.descriptor_pool)
                    .set_layouts(&set_layouts),
            )?[0]
        };
        let image_info = [vk::DescriptorImageInfo::builder()
            .image_view(view)
            .sampler(sampler)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .build()];
        let write = [vk::WriteDescriptorSet::builder()
            .dst_set(set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&image_info)
            .build()];
        unsafe { device.update_descriptor_sets(&write, &[]) };

        // external entry: only the descriptor set is owned here.
        let entry = EguiTexture {
            image: vk::Image::null(),
            memory: vk::DeviceMemory::null(),
            view,
            set,
            width: 0,
            height: 0,
            external: true,
        };
        if let Some(old) = self.textures.insert(id, entry) {
            self.destroy_texture(vk, old);
        }
        Ok(id)
    }

    fn destroy_texture(&self, vk: &VulkanContext, t: EguiTexture) {
        unsafe {
            let _ = vk.device.free_descriptor_sets(self.descriptor_pool, &[t.set]);
            // r230: external textures borrow an engine-owned render target —
            // free only the descriptor set, never the image/memory/view.
            if !t.external {
                vk.device.destroy_image_view(t.view, None);
                vk.device.destroy_image(t.image, None);
                vk.device.free_memory(t.memory, None);
            }
        }
    }

    /// Run a one-shot command buffer on the graphics queue and wait (mirrors the
    /// engine's upload/transition helpers in froxel.rs / chunk_renderer.rs).
    fn one_time(
        &self,
        vk: &VulkanContext,
        record: impl FnOnce(&ash::Device, vk::CommandBuffer),
    ) -> Result<(), vk::Result> {
        let device = &vk.device;
        let pool = unsafe {
            device.create_command_pool(
                &vk::CommandPoolCreateInfo::builder().queue_family_index(vk.graphics_queue_family),
                None,
            )?
        };
        let cmd = unsafe {
            device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::builder()
                    .command_pool(pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )?[0]
        };
        unsafe {
            device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::builder()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            record(device, cmd);
            device.end_command_buffer(cmd)?;
            let cmds = [cmd];
            let submit = vk::SubmitInfo::builder().command_buffers(&cmds).build();
            let _g = vk.queue_submit_lock.lock().expect("queue submit lock");
            device.queue_submit(vk.graphics_queue, &[submit], vk::Fence::null())?;
            device.queue_wait_idle(vk.graphics_queue)?;
            device.destroy_command_pool(pool, None);
        }
        Ok(())
    }

    pub fn destroy(&mut self, vk: &VulkanContext) {
        let device = &vk.device;
        let texs: Vec<EguiTexture> = self.textures.drain().map(|(_, t)| t).collect();
        for t in texs {
            self.destroy_texture(vk, t);
        }
        unsafe {
            for fb in &self.frames {
                device.destroy_buffer(fb.vbuf, None);
                device.free_memory(fb.vmem, None);
                device.destroy_buffer(fb.ibuf, None);
                device.free_memory(fb.imem, None);
            }
            device.destroy_sampler(self.sampler, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.set_layout, None);
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
        }
    }
}

/// Reinterpret a `&[f32; 2]` as bytes for the push constant (no extra deps).
fn bytemuck_cast(v: &[f32; 2]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(v.as_ptr() as *const u8, 8) }
}
