//! Fullscreen present/post-process pipeline for the gated post-FX path
//! (UNENDLICH_POSTFX). Draws `present.vert`/`present.frag`: no vertex input
//! (fullscreen triangle from gl_VertexIndex), no cull, no depth, one combined
//! image sampler (set 0, binding 0) bound to the offscreen color target.
//! Modeled on `chunk_pipeline.rs` — same shader-loading cfg, builder patterns,
//! and `create_shader_module` helper.

use ash::{vk, Device};
use std::ffi::CStr;

#[cfg(not(feature = "hot-shaders"))]
const VERT_SPV: &[u8] = include_bytes!("../../../../assets/shaders/present.vert.spv");
#[cfg(not(feature = "hot-shaders"))]
const FRAG_SPV: &[u8] = include_bytes!("../../../../assets/shaders/present.frag.spv");

#[cfg(feature = "hot-shaders")]
fn load_vert_spv() -> Vec<u8> {
    crate::render::hot_shaders::read_spv("present.vert.spv")
}
#[cfg(feature = "hot-shaders")]
fn load_frag_spv() -> Vec<u8> {
    crate::render::hot_shaders::read_spv("present.frag.spv")
}

pub struct PresentPipeline {
    pub pipeline: vk::Pipeline,
    pub layout: vk::PipelineLayout,
    pub descriptor_set_layout: vk::DescriptorSetLayout,
    pub descriptor_pool: vk::DescriptorPool,
    pub descriptor_set: vk::DescriptorSet,
}

impl PresentPipeline {
    pub fn new(device: &Device, render_pass: vk::RenderPass) -> Result<Self, vk::Result> {
        #[cfg(not(feature = "hot-shaders"))]
        let (vert_module, frag_module) = (
            create_shader_module(device, VERT_SPV)?,
            create_shader_module(device, FRAG_SPV)?,
        );
        #[cfg(feature = "hot-shaders")]
        let (vert_module, frag_module) = {
            let v = load_vert_spv();
            let f = load_frag_spv();
            (create_shader_module(device, &v)?, create_shader_module(device, &f)?)
        };
        let entry = unsafe { CStr::from_bytes_with_nul_unchecked(b"main\0") };

        let stages = [
            vk::PipelineShaderStageCreateInfo::builder()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert_module)
                .name(entry)
                .build(),
            vk::PipelineShaderStageCreateInfo::builder()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(frag_module)
                .name(entry)
                .build(),
        ];

        // No vertex input: the fullscreen triangle is generated from
        // gl_VertexIndex in present.vert.
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::builder();

        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::builder()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
            .primitive_restart_enable(false);

        let viewport_state = vk::PipelineViewportStateCreateInfo::builder()
            .viewport_count(1)
            .scissor_count(1);

        // Fullscreen triangle: no culling, winding irrelevant.
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

        // Present pass has no depth attachment; disable all depth/stencil.
        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::builder()
            .depth_test_enable(false)
            .depth_write_enable(false)
            .depth_bounds_test_enable(false)
            .stencil_test_enable(false);

        let color_blend_attachment = vk::PipelineColorBlendAttachmentState::builder()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(false)
            .build();
        let color_blend_attachments = [color_blend_attachment];
        let color_blend = vk::PipelineColorBlendStateCreateInfo::builder()
            .logic_op_enable(false)
            .attachments(&color_blend_attachments);

        let dyn_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state = vk::PipelineDynamicStateCreateInfo::builder()
            .dynamic_states(&dyn_states);

        // Descriptor set 0: binding 0 = scene color (offscreen HDR),
        // binding 1 = bloom color (half-res bloom target, or the scene image
        // itself when bloom is off so the descriptor is always valid).
        let dsl_bindings = [
            vk::DescriptorSetLayoutBinding::builder()
                .binding(0)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                .build(),
            vk::DescriptorSetLayoutBinding::builder()
                .binding(1)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                .build(),
        ];
        let dsl_info = vk::DescriptorSetLayoutCreateInfo::builder().bindings(&dsl_bindings);
        let descriptor_set_layout = unsafe { device.create_descriptor_set_layout(&dsl_info, None)? };

        let set_layouts = [descriptor_set_layout];
        // Push constant: bloom_intensity (f32). 0.0 = bloom off (exact no-op).
        let push_ranges = [vk::PushConstantRange {
            stage_flags: vk::ShaderStageFlags::FRAGMENT,
            offset: 0,
            size: 4,
        }];
        let layout_info = vk::PipelineLayoutCreateInfo::builder()
            .set_layouts(&set_layouts)
            .push_constant_ranges(&push_ranges);
        let layout = unsafe { device.create_pipeline_layout(&layout_info, None)? };

        // One descriptor set holding two combined image samplers (scene + bloom).
        let pool_sizes = [vk::DescriptorPoolSize {
            ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
            descriptor_count: 2,
        }];
        let pool_info = vk::DescriptorPoolCreateInfo::builder()
            .pool_sizes(&pool_sizes)
            .max_sets(1);
        let descriptor_pool = unsafe { device.create_descriptor_pool(&pool_info, None)? };

        let alloc_info = vk::DescriptorSetAllocateInfo::builder()
            .descriptor_pool(descriptor_pool)
            .set_layouts(&set_layouts);
        let descriptor_set = unsafe { device.allocate_descriptor_sets(&alloc_info)? }[0];

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
            device.create_graphics_pipelines(
                vk::PipelineCache::null(),
                std::slice::from_ref(&info),
                None,
            )
        }.map_err(|(_, e)| e)?;

        unsafe {
            device.destroy_shader_module(vert_module, None);
            device.destroy_shader_module(frag_module, None);
        }

        Ok(Self {
            pipeline: pipelines[0],
            layout,
            descriptor_set_layout,
            descriptor_pool,
            descriptor_set,
        })
    }

    /// Point the descriptor set at the offscreen color target. Call after the
    /// target is created and on every swapchain resize (the target is recreated
    /// but the set/pool persist). Caller must device_wait_idle before resize.
    pub fn write_offscreen(&self, device: &Device, view: vk::ImageView, sampler: vk::Sampler) {
        let image_info = [vk::DescriptorImageInfo {
            sampler,
            image_view: view,
            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        }];
        let write = vk::WriteDescriptorSet::builder()
            .dst_set(self.descriptor_set)
            .dst_binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&image_info)
            .build();
        unsafe { device.update_descriptor_sets(std::slice::from_ref(&write), &[]); }
    }

    /// Point binding 1 (the composite/bloom sampler) at an image. When bloom is
    /// active this is the half-res bloom target; when bloom is off, bind the
    /// scene image itself (any valid image) and push `bloom_intensity = 0` so
    /// the composite is an exact no-op. Re-call on swapchain resize.
    pub fn write_bloom(&self, device: &Device, view: vk::ImageView, sampler: vk::Sampler) {
        let image_info = [vk::DescriptorImageInfo {
            sampler,
            image_view: view,
            image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
        }];
        let write = vk::WriteDescriptorSet::builder()
            .dst_set(self.descriptor_set)
            .dst_binding(1)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .image_info(&image_info)
            .build();
        unsafe { device.update_descriptor_sets(std::slice::from_ref(&write), &[]); }
    }

    pub fn destroy(&mut self, device: &Device) {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
        }
    }
}

fn create_shader_module(device: &Device, bytes: &[u8]) -> Result<vk::ShaderModule, vk::Result> {
    assert_eq!(bytes.len() % 4, 0);
    let words: Vec<u32> = bytes.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let info = vk::ShaderModuleCreateInfo::builder().code(&words);
    unsafe { device.create_shader_module(&info, None) }
}
