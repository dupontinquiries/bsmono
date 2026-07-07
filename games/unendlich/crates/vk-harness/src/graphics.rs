//! Headless **graphics** pipeline harness — the render-side analog of
//! `ComputeHarness`. It renders into an offscreen RGBA8 image (no window, no
//! swapchain) and reads the pixels back, so a `.vert`/`.frag` pair and a vertex
//! layout can be verified by *asserting on the output pixels* on lavapipe. This
//! is what turns "the render pipeline needs the M2" into "shader + vertex-format
//! correctness is provable in-sandbox" (aesthetics and real-GPU perf still need
//! the M2, but logic does not).

use ash::{vk, Device, Entry, Instance};

/// One vertex attribute for the packed/interleaved vertex buffer.
#[derive(Clone, Copy)]
pub struct VertexAttr {
    pub location: u32,
    pub format: vk::Format,
    pub offset: u32,
}

pub struct GraphicsHarness {
    _entry: Entry,
    instance: Instance,
    device: Device,
    phys: vk::PhysicalDevice,
    queue: vk::Queue,
    queue_family: u32,
}

impl GraphicsHarness {
    pub fn new() -> Result<Self, String> {
        unsafe {
            let entry = Entry::load().map_err(|e| format!("no Vulkan loader: {e:?}"))?;
            let app = vk::ApplicationInfo::builder().api_version(vk::make_api_version(0, 1, 1, 0));
            let ci = vk::InstanceCreateInfo::builder().application_info(&app);
            let instance = entry.create_instance(&ci, None).map_err(|e| format!("instance: {e:?}"))?;

            let phys_devices = instance.enumerate_physical_devices().map_err(|e| format!("enum: {e:?}"))?;
            let mut chosen = None;
            for pd in phys_devices {
                let props = instance.get_physical_device_queue_family_properties(pd);
                if let Some(qf) = props.iter().position(|q| {
                    q.queue_flags.contains(vk::QueueFlags::GRAPHICS) && q.queue_count > 0
                }) {
                    chosen = Some((pd, qf as u32));
                    break;
                }
            }
            let (phys, queue_family) = chosen.ok_or("no graphics queue")?;
            let prio = [1.0f32];
            let qci = vk::DeviceQueueCreateInfo::builder()
                .queue_family_index(queue_family)
                .queue_priorities(&prio);
            let qcis = [qci.build()];
            let dci = vk::DeviceCreateInfo::builder().queue_create_infos(&qcis);
            let device = instance.create_device(phys, &dci, None).map_err(|e| format!("device: {e:?}"))?;
            let queue = device.get_device_queue(queue_family, 0);
            Ok(Self { _entry: entry, instance, device, phys, queue, queue_family })
        }
    }

    pub fn device_name(&self) -> String {
        unsafe {
            let p = self.instance.get_physical_device_properties(self.phys);
            std::ffi::CStr::from_ptr(p.device_name.as_ptr()).to_string_lossy().into_owned()
        }
    }

    unsafe fn mem_type(&self, bits: u32, want: vk::MemoryPropertyFlags) -> Result<u32, String> {
        let mem = self.instance.get_physical_device_memory_properties(self.phys);
        for i in 0..mem.memory_type_count {
            if bits & (1 << i) != 0 && mem.memory_types[i as usize].property_flags.contains(want) {
                return Ok(i);
            }
        }
        Err(format!("no memory type for {want:?}"))
    }

    /// Render `vertex_count` vertices into a `dim`×`dim` RGBA8 image and return
    /// the pixels (row-major, 4 bytes/pixel). `vertex_data` is
    /// `(bytes, stride, attrs)`; pass `None` to draw from `gl_VertexIndex` only.
    pub fn render_offscreen(
        &self,
        dim: u32,
        clear: [f32; 4],
        vert_spv: &[u32],
        frag_spv: &[u32],
        vertex_data: Option<(&[u8], u32, &[VertexAttr])>,
        vertex_count: u32,
        push: &[u8],
        indices: Option<(&[u8], u32)>,
        ubo: Option<&[u8]>,
    ) -> Result<Vec<u8>, String> {
        self.render_impl(dim, clear, vert_spv, frag_spv, vertex_data, vertex_count, push, indices, ubo, None)
    }

    /// Like [`render_offscreen`](Self::render_offscreen) but also binds a sampled
    /// RGBA8 texture at set 0, binding 1 (a `sampler2D atlas` for the fragment
    /// shader). `texture` is `(rgba_bytes, width, height)`, NEAREST/clamp.
    #[allow(clippy::too_many_arguments)]
    pub fn render_offscreen_tex(
        &self,
        dim: u32,
        clear: [f32; 4],
        vert_spv: &[u32],
        frag_spv: &[u32],
        vertex_data: Option<(&[u8], u32, &[VertexAttr])>,
        vertex_count: u32,
        push: &[u8],
        indices: Option<(&[u8], u32)>,
        ubo: Option<&[u8]>,
        texture: (&[u8], u32, u32),
    ) -> Result<Vec<u8>, String> {
        self.render_impl(dim, clear, vert_spv, frag_spv, vertex_data, vertex_count, push, indices, ubo, Some(texture))
    }

    #[allow(clippy::too_many_arguments)]
    fn render_impl(
        &self,
        dim: u32,
        clear: [f32; 4],
        vert_spv: &[u32],
        frag_spv: &[u32],
        vertex_data: Option<(&[u8], u32, &[VertexAttr])>,
        vertex_count: u32,
        push: &[u8],
        indices: Option<(&[u8], u32)>,
        ubo: Option<&[u8]>,
        texture: Option<(&[u8], u32, u32)>,
    ) -> Result<Vec<u8>, String> {
        let dev = &self.device;
        unsafe {
            // ---- Offscreen color image (device-local) ----
            let fmt = vk::Format::R8G8B8A8_UNORM;
            let img_ci = vk::ImageCreateInfo::builder()
                .image_type(vk::ImageType::TYPE_2D)
                .format(fmt)
                .extent(vk::Extent3D { width: dim, height: dim, depth: 1 })
                .mip_levels(1)
                .array_layers(1)
                .samples(vk::SampleCountFlags::TYPE_1)
                .tiling(vk::ImageTiling::OPTIMAL)
                .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC)
                .initial_layout(vk::ImageLayout::UNDEFINED);
            let image = dev.create_image(&img_ci, None).map_err(|e| format!("image: {e:?}"))?;
            let ireq = dev.get_image_memory_requirements(image);
            let imem_type = self.mem_type(ireq.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)
                .or_else(|_| self.mem_type(ireq.memory_type_bits, vk::MemoryPropertyFlags::empty()))?;
            let imem = dev
                .allocate_memory(&vk::MemoryAllocateInfo::builder().allocation_size(ireq.size).memory_type_index(imem_type), None)
                .map_err(|e| format!("img mem: {e:?}"))?;
            dev.bind_image_memory(image, imem, 0).map_err(|e| format!("bind img: {e:?}"))?;

            let view_ci = vk::ImageViewCreateInfo::builder()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(fmt)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1,
                });
            let view = dev.create_image_view(&view_ci, None).map_err(|e| format!("view: {e:?}"))?;

            // ---- Render pass ----
            let color_attach = vk::AttachmentDescription::builder()
                .format(fmt)
                .samples(vk::SampleCountFlags::TYPE_1)
                .load_op(vk::AttachmentLoadOp::CLEAR)
                .store_op(vk::AttachmentStoreOp::STORE)
                .initial_layout(vk::ImageLayout::UNDEFINED)
                .final_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL)
                .build();
            let color_ref = vk::AttachmentReference { attachment: 0, layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL };
            let color_refs = [color_ref];
            let subpass = vk::SubpassDescription::builder()
                .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
                .color_attachments(&color_refs)
                .build();
            let attaches = [color_attach];
            let subpasses = [subpass];
            let rp_ci = vk::RenderPassCreateInfo::builder().attachments(&attaches).subpasses(&subpasses);
            let render_pass = dev.create_render_pass(&rp_ci, None).map_err(|e| format!("rp: {e:?}"))?;

            let views = [view];
            let fb_ci = vk::FramebufferCreateInfo::builder()
                .render_pass(render_pass)
                .attachments(&views)
                .width(dim).height(dim).layers(1);
            let framebuffer = dev.create_framebuffer(&fb_ci, None).map_err(|e| format!("fb: {e:?}"))?;

            // ---- Shader modules ----
            let mk_shader = |spv: &[u32]| -> Result<vk::ShaderModule, String> {
                let ci = vk::ShaderModuleCreateInfo::builder().code(spv);
                dev.create_shader_module(&ci, None).map_err(|e| format!("shader: {e:?}"))
            };
            let vs = mk_shader(vert_spv)?;
            let fs = mk_shader(frag_spv)?;
            let entry_name = std::ffi::CString::new("main").unwrap();
            let stages = [
                vk::PipelineShaderStageCreateInfo::builder()
                    .stage(vk::ShaderStageFlags::VERTEX).module(vs).name(&entry_name).build(),
                vk::PipelineShaderStageCreateInfo::builder()
                    .stage(vk::ShaderStageFlags::FRAGMENT).module(fs).name(&entry_name).build(),
            ];

            // ---- Vertex input ----
            let (bindings, attrs): (Vec<_>, Vec<_>) = match vertex_data {
                Some((_, stride, attrs)) => (
                    vec![vk::VertexInputBindingDescription {
                        binding: 0, stride, input_rate: vk::VertexInputRate::VERTEX,
                    }],
                    attrs.iter().map(|a| vk::VertexInputAttributeDescription {
                        location: a.location, binding: 0, format: a.format, offset: a.offset,
                    }).collect(),
                ),
                None => (vec![], vec![]),
            };
            let vi = vk::PipelineVertexInputStateCreateInfo::builder()
                .vertex_binding_descriptions(&bindings)
                .vertex_attribute_descriptions(&attrs);
            let ia = vk::PipelineInputAssemblyStateCreateInfo::builder()
                .topology(vk::PrimitiveTopology::TRIANGLE_LIST);
            let viewport = vk::Viewport { x: 0.0, y: 0.0, width: dim as f32, height: dim as f32, min_depth: 0.0, max_depth: 1.0 };
            let scissor = vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: vk::Extent2D { width: dim, height: dim } };
            let viewports = [viewport];
            let scissors = [scissor];
            let vp = vk::PipelineViewportStateCreateInfo::builder().viewports(&viewports).scissors(&scissors);
            let rs = vk::PipelineRasterizationStateCreateInfo::builder()
                .polygon_mode(vk::PolygonMode::FILL)
                .cull_mode(vk::CullModeFlags::NONE)
                .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
                .line_width(1.0);
            let ms = vk::PipelineMultisampleStateCreateInfo::builder()
                .rasterization_samples(vk::SampleCountFlags::TYPE_1);
            let blend_attach = vk::PipelineColorBlendAttachmentState::builder()
                .color_write_mask(vk::ColorComponentFlags::RGBA)
                .blend_enable(false)
                .build();
            let blend_attaches = [blend_attach];
            let cb = vk::PipelineColorBlendStateCreateInfo::builder().attachments(&blend_attaches);

            // Optional descriptor set 0: UBO at binding 0, sampled atlas at binding 1.
            let mut dsl_bindings: Vec<vk::DescriptorSetLayoutBinding> = Vec::new();
            if ubo.is_some() {
                dsl_bindings.push(
                    vk::DescriptorSetLayoutBinding::builder()
                        .binding(0)
                        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                        .descriptor_count(1)
                        .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                        .build(),
                );
            }
            if texture.is_some() {
                dsl_bindings.push(
                    vk::DescriptorSetLayoutBinding::builder()
                        .binding(1)
                        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                        .descriptor_count(1)
                        .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                        .build(),
                );
            }
            let set_layouts: Vec<vk::DescriptorSetLayout> = if dsl_bindings.is_empty() {
                Vec::new()
            } else {
                let ci = vk::DescriptorSetLayoutCreateInfo::builder().bindings(&dsl_bindings);
                vec![dev.create_descriptor_set_layout(&ci, None).map_err(|e| format!("dsl: {e:?}"))?]
            };

            let pc_ranges = if push.is_empty() {
                Vec::new()
            } else {
                vec![vk::PushConstantRange {
                    stage_flags: vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    offset: 0,
                    size: (((push.len() + 3) / 4) * 4) as u32,
                }]
            };
            let pl_ci = vk::PipelineLayoutCreateInfo::builder()
                .set_layouts(&set_layouts)
                .push_constant_ranges(&pc_ranges);
            let pipeline_layout = dev.create_pipeline_layout(&pl_ci, None).map_err(|e| format!("pl layout: {e:?}"))?;

            let gp_ci = vk::GraphicsPipelineCreateInfo::builder()
                .stages(&stages)
                .vertex_input_state(&vi)
                .input_assembly_state(&ia)
                .viewport_state(&vp)
                .rasterization_state(&rs)
                .multisample_state(&ms)
                .color_blend_state(&cb)
                .layout(pipeline_layout)
                .render_pass(render_pass)
                .subpass(0)
                .build();
            let pipeline = dev
                .create_graphics_pipelines(vk::PipelineCache::null(), &[gp_ci], None)
                .map_err(|(_, e)| format!("pipeline: {e:?}"))?[0];

            // ---- Vertex buffer (host-visible) ----
            let vbuf = match vertex_data {
                Some((bytes, _, _)) if !bytes.is_empty() => {
                    let (buf, mem) = self.host_buffer(bytes.len() as u64, vk::BufferUsageFlags::VERTEX_BUFFER)?;
                    let ptr = dev.map_memory(mem, 0, bytes.len() as u64, vk::MemoryMapFlags::empty())
                        .map_err(|e| format!("map vbuf: {e:?}"))?;
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
                    dev.unmap_memory(mem);
                    Some((buf, mem))
                }
                _ => None,
            };

            let ibuf = match indices {
                Some((bytes, _)) if !bytes.is_empty() => {
                    let (buf, mem) = self.host_buffer(bytes.len() as u64, vk::BufferUsageFlags::INDEX_BUFFER)?;
                    let ptr = dev.map_memory(mem, 0, bytes.len() as u64, vk::MemoryMapFlags::empty())
                        .map_err(|e| format!("map ibuf: {e:?}"))?;
                    std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
                    dev.unmap_memory(mem);
                    Some((buf, mem))
                }
                _ => None,
            };

            // ---- Readback buffer ----
            let rb_size = (dim * dim * 4) as u64;
            let (rb_buf, rb_mem) = self.host_buffer(rb_size, vk::BufferUsageFlags::TRANSFER_DST)?;

            // Optional sampled texture (atlas) image + view + sampler + staging.
            let tex_res = if let Some((pixels, tw, th)) = texture {
                let tfmt = vk::Format::R8G8B8A8_UNORM;
                let tci = vk::ImageCreateInfo::builder()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(tfmt)
                    .extent(vk::Extent3D { width: tw, height: th, depth: 1 })
                    .mip_levels(1).array_layers(1).samples(vk::SampleCountFlags::TYPE_1)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
                    .initial_layout(vk::ImageLayout::UNDEFINED);
                let timg = dev.create_image(&tci, None).map_err(|e| format!("tex img: {e:?}"))?;
                let treq = dev.get_image_memory_requirements(timg);
                let tmt = self.mem_type(treq.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)
                    .or_else(|_| self.mem_type(treq.memory_type_bits, vk::MemoryPropertyFlags::empty()))?;
                let tmem = dev.allocate_memory(&vk::MemoryAllocateInfo::builder().allocation_size(treq.size).memory_type_index(tmt), None)
                    .map_err(|e| format!("tex mem: {e:?}"))?;
                dev.bind_image_memory(timg, tmem, 0).map_err(|e| format!("bind tex: {e:?}"))?;
                let tview_ci = vk::ImageViewCreateInfo::builder()
                    .image(timg).view_type(vk::ImageViewType::TYPE_2D).format(tfmt)
                    .subresource_range(vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 });
                let tview = dev.create_image_view(&tview_ci, None).map_err(|e| format!("tex view: {e:?}"))?;
                let samp_ci = vk::SamplerCreateInfo::builder()
                    .mag_filter(vk::Filter::NEAREST).min_filter(vk::Filter::NEAREST)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE);
                let sampler = dev.create_sampler(&samp_ci, None).map_err(|e| format!("sampler: {e:?}"))?;
                let (sbuf, smem) = self.host_buffer(pixels.len() as u64, vk::BufferUsageFlags::TRANSFER_SRC)?;
                let sptr = dev.map_memory(smem, 0, pixels.len() as u64, vk::MemoryMapFlags::empty()).map_err(|e| format!("map stage: {e:?}"))?;
                std::ptr::copy_nonoverlapping(pixels.as_ptr(), sptr as *mut u8, pixels.len());
                dev.unmap_memory(smem);
                Some((timg, tmem, tview, sampler, sbuf, smem, tw, th))
            } else {
                None
            };

            // One descriptor set 0 holding whichever of {ubo@0, sampler@1} are present.
            let (ubo_buf_mem, desc): (Option<(vk::Buffer, vk::DeviceMemory)>, Option<(vk::DescriptorPool, vk::DescriptorSet)>) =
                if !set_layouts.is_empty() {
                    let mut sizes: Vec<vk::DescriptorPoolSize> = Vec::new();
                    if ubo.is_some() { sizes.push(vk::DescriptorPoolSize { ty: vk::DescriptorType::UNIFORM_BUFFER, descriptor_count: 1 }); }
                    if texture.is_some() { sizes.push(vk::DescriptorPoolSize { ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER, descriptor_count: 1 }); }
                    let pool_ci = vk::DescriptorPoolCreateInfo::builder().max_sets(1).pool_sizes(&sizes);
                    let dpool = dev.create_descriptor_pool(&pool_ci, None).map_err(|e| format!("dpool: {e:?}"))?;
                    let alloc = vk::DescriptorSetAllocateInfo::builder().descriptor_pool(dpool).set_layouts(&set_layouts);
                    let dset = dev.allocate_descriptor_sets(&alloc).map_err(|e| format!("dset: {e:?}"))?[0];

                    let ubo_bm = if let Some(bytes) = ubo {
                        let (buf, mem) = self.host_buffer(bytes.len() as u64, vk::BufferUsageFlags::UNIFORM_BUFFER)?;
                        let ptr = dev.map_memory(mem, 0, bytes.len() as u64, vk::MemoryMapFlags::empty()).map_err(|e| format!("map ubo: {e:?}"))?;
                        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr as *mut u8, bytes.len());
                        dev.unmap_memory(mem);
                        Some((buf, mem, bytes.len() as u64))
                    } else {
                        None
                    };

                    let mut writes: Vec<vk::WriteDescriptorSet> = Vec::new();
                    let binfo;
                    if let Some((buf, _, len)) = ubo_bm {
                        binfo = [vk::DescriptorBufferInfo { buffer: buf, offset: 0, range: len }];
                        writes.push(vk::WriteDescriptorSet::builder().dst_set(dset).dst_binding(0)
                            .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER).buffer_info(&binfo).build());
                    }
                    let iinfo;
                    if let Some((_, _, tview, sampler, _, _, _, _)) = tex_res {
                        iinfo = [vk::DescriptorImageInfo { sampler, image_view: tview, image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL }];
                        writes.push(vk::WriteDescriptorSet::builder().dst_set(dset).dst_binding(1)
                            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER).image_info(&iinfo).build());
                    }
                    dev.update_descriptor_sets(&writes, &[]);
                    (ubo_bm.map(|(b, m, _)| (b, m)), Some((dpool, dset)))
                } else {
                    (None, None)
                };

            // ---- Command buffer ----
            let pool = dev
                .create_command_pool(&vk::CommandPoolCreateInfo::builder().queue_family_index(self.queue_family), None)
                .map_err(|e| format!("pool: {e:?}"))?;
            let cbs = dev
                .allocate_command_buffers(&vk::CommandBufferAllocateInfo::builder().command_pool(pool).command_buffer_count(1).level(vk::CommandBufferLevel::PRIMARY))
                .map_err(|e| format!("cb alloc: {e:?}"))?;
            let cmd = cbs[0];
            dev.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::builder()).map_err(|e| format!("begin: {e:?}"))?;

            // Upload the sampled texture (staging buffer -> image) before the pass.
            if let Some((timg, _, _, _, sbuf, _, tw, th)) = tex_res {
                let sub = vk::ImageSubresourceRange { aspect_mask: vk::ImageAspectFlags::COLOR, base_mip_level: 0, level_count: 1, base_array_layer: 0, layer_count: 1 };
                let to_dst = vk::ImageMemoryBarrier::builder()
                    .old_layout(vk::ImageLayout::UNDEFINED).new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED).dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(timg).subresource_range(sub)
                    .src_access_mask(vk::AccessFlags::empty()).dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                    .build();
                dev.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::TRANSFER, vk::DependencyFlags::empty(), &[], &[], &[to_dst]);
                let region = vk::BufferImageCopy {
                    buffer_offset: 0, buffer_row_length: 0, buffer_image_height: 0,
                    image_subresource: vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 },
                    image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
                    image_extent: vk::Extent3D { width: tw, height: th, depth: 1 },
                };
                dev.cmd_copy_buffer_to_image(cmd, sbuf, timg, vk::ImageLayout::TRANSFER_DST_OPTIMAL, &[region]);
                let to_ro = vk::ImageMemoryBarrier::builder()
                    .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL).new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                    .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED).dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                    .image(timg).subresource_range(sub)
                    .src_access_mask(vk::AccessFlags::TRANSFER_WRITE).dst_access_mask(vk::AccessFlags::SHADER_READ)
                    .build();
                dev.cmd_pipeline_barrier(cmd, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::FRAGMENT_SHADER, vk::DependencyFlags::empty(), &[], &[], &[to_ro]);
            }

            let clear_vals = [vk::ClearValue { color: vk::ClearColorValue { float32: clear } }];
            let rp_begin = vk::RenderPassBeginInfo::builder()
                .render_pass(render_pass)
                .framebuffer(framebuffer)
                .render_area(scissor)
                .clear_values(&clear_vals);
            dev.cmd_begin_render_pass(cmd, &rp_begin, vk::SubpassContents::INLINE);
            dev.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline);
            if let Some((_, dset)) = desc {
                dev.cmd_bind_descriptor_sets(cmd, vk::PipelineBindPoint::GRAPHICS, pipeline_layout, 0, &[dset], &[]);
            }
            if !push.is_empty() {
                dev.cmd_push_constants(
                    cmd,
                    pipeline_layout,
                    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    0,
                    push,
                );
            }
            if let Some((buf, _)) = vbuf {
                dev.cmd_bind_vertex_buffers(cmd, 0, &[buf], &[0]);
            }
            match (ibuf, indices) {
                (Some((ibuf_h, _)), Some((_, index_count))) => {
                    dev.cmd_bind_index_buffer(cmd, ibuf_h, 0, vk::IndexType::UINT32);
                    dev.cmd_draw_indexed(cmd, index_count, 1, 0, 0, 0);
                }
                _ => dev.cmd_draw(cmd, vertex_count, 1, 0, 0),
            }
            dev.cmd_end_render_pass(cmd);

            // image (now TRANSFER_SRC_OPTIMAL) → readback buffer
            let copy = vk::BufferImageCopy {
                buffer_offset: 0, buffer_row_length: 0, buffer_image_height: 0,
                image_subresource: vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1,
                },
                image_offset: vk::Offset3D { x: 0, y: 0, z: 0 },
                image_extent: vk::Extent3D { width: dim, height: dim, depth: 1 },
            };
            dev.cmd_copy_image_to_buffer(cmd, image, vk::ImageLayout::TRANSFER_SRC_OPTIMAL, rb_buf, &[copy]);
            dev.end_command_buffer(cmd).map_err(|e| format!("end: {e:?}"))?;

            let fence = dev.create_fence(&vk::FenceCreateInfo::builder(), None).map_err(|e| format!("fence: {e:?}"))?;
            let cmd_bufs = [cmd];
            let submit = vk::SubmitInfo::builder().command_buffers(&cmd_bufs).build();
            dev.queue_submit(self.queue, &[submit], fence).map_err(|e| format!("submit: {e:?}"))?;
            dev.wait_for_fences(&[fence], true, 5_000_000_000).map_err(|e| format!("wait: {e:?}"))?;

            // Read back.
            let ptr = dev.map_memory(rb_mem, 0, rb_size, vk::MemoryMapFlags::empty()).map_err(|e| format!("map rb: {e:?}"))?;
            let mut out = vec![0u8; rb_size as usize];
            std::ptr::copy_nonoverlapping(ptr as *const u8, out.as_mut_ptr(), rb_size as usize);
            dev.unmap_memory(rb_mem);

            // ---- Teardown ----
            dev.destroy_fence(fence, None);
            dev.destroy_command_pool(pool, None);
            dev.destroy_buffer(rb_buf, None);
            dev.free_memory(rb_mem, None);
            if let Some((buf, mem)) = vbuf {
                dev.destroy_buffer(buf, None);
                dev.free_memory(mem, None);
            }
            if let Some((buf, mem)) = ibuf {
                dev.destroy_buffer(buf, None);
                dev.free_memory(mem, None);
            }
            dev.destroy_pipeline(pipeline, None);
            dev.destroy_pipeline_layout(pipeline_layout, None);
            if let Some((buf, mem)) = ubo_buf_mem {
                dev.destroy_buffer(buf, None);
                dev.free_memory(mem, None);
            }
            if let Some((dpool, _)) = desc {
                dev.destroy_descriptor_pool(dpool, None);
            }
            if let Some((timg, tmem, tview, sampler, sbuf, smem, _, _)) = tex_res {
                dev.destroy_sampler(sampler, None);
                dev.destroy_image_view(tview, None);
                dev.destroy_image(timg, None);
                dev.free_memory(tmem, None);
                dev.destroy_buffer(sbuf, None);
                dev.free_memory(smem, None);
            }
            for sl in set_layouts {
                dev.destroy_descriptor_set_layout(sl, None);
            }
            dev.destroy_shader_module(vs, None);
            dev.destroy_shader_module(fs, None);
            dev.destroy_framebuffer(framebuffer, None);
            dev.destroy_render_pass(render_pass, None);
            dev.destroy_image_view(view, None);
            dev.destroy_image(image, None);
            dev.free_memory(imem, None);
            Ok(out)
        }
    }

    unsafe fn host_buffer(&self, size: u64, usage: vk::BufferUsageFlags) -> Result<(vk::Buffer, vk::DeviceMemory), String> {
        let dev = &self.device;
        let buf = dev
            .create_buffer(&vk::BufferCreateInfo::builder().size(size).usage(usage).sharing_mode(vk::SharingMode::EXCLUSIVE), None)
            .map_err(|e| format!("buffer: {e:?}"))?;
        let req = dev.get_buffer_memory_requirements(buf);
        let mt = self.mem_type(req.memory_type_bits, vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT)?;
        let mem = dev
            .allocate_memory(&vk::MemoryAllocateInfo::builder().allocation_size(req.size).memory_type_index(mt), None)
            .map_err(|e| format!("alloc: {e:?}"))?;
        dev.bind_buffer_memory(buf, mem, 0).map_err(|e| format!("bind: {e:?}"))?;
        Ok((buf, mem))
    }
}

impl Drop for GraphicsHarness {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}
