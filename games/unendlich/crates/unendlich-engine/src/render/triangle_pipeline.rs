//! Graphics pipeline for the hardcoded triangle.
//!
//! No vertex inputs (vertices are baked into the shader via gl_VertexIndex).
//! No descriptor sets, no push constants. Just the bare minimum to render
//! something to the screen.

use ash::{vk, Device};
use std::ffi::CStr;

const VERT_SPV: &[u8] = include_bytes!("../../../../assets/shaders/triangle.vert.spv");
const FRAG_SPV: &[u8] = include_bytes!("../../../../assets/shaders/triangle.frag.spv");

pub struct TrianglePipeline {
    pub pipeline: vk::Pipeline,
    pub layout: vk::PipelineLayout,
}

impl TrianglePipeline {
    pub fn new(
        device: &Device,
        render_pass: vk::RenderPass,
        extent: vk::Extent2D,
    ) -> std::result::Result<Self, vk::Result> {
        let vert_module = create_shader_module(device, VERT_SPV)?;
        let frag_module = create_shader_module(device, FRAG_SPV)?;

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

        // No vertex inputs — the shader uses gl_VertexIndex.
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::builder();

        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::builder()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
            .primitive_restart_enable(false);

        // We use dynamic viewport + scissor so we don't need to recreate the
        // pipeline on resize. The pipeline still requires the count to be set.
        let viewport_state = vk::PipelineViewportStateCreateInfo::builder()
            .viewport_count(1)
            .scissor_count(1);

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

        let layout_info = vk::PipelineLayoutCreateInfo::builder();
        let layout = unsafe { device.create_pipeline_layout(&layout_info, None)? };

        let info = vk::GraphicsPipelineCreateInfo::builder()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport_state)
            .rasterization_state(&rasterizer)
            .multisample_state(&multisample)
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

        // We don't pass `extent` through anymore (dynamic state covers it),
        // but keep the param so the call site doesn't have to special-case.
        let _ = extent;

        Ok(Self { pipeline: pipelines[0], layout })
    }

    pub fn destroy(&mut self, device: &Device) {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
        }
    }
}

fn create_shader_module(device: &Device, bytes: &[u8]) -> std::result::Result<vk::ShaderModule, vk::Result> {
    // SPIR-V is little-endian u32 stream; ash's API wants &[u32].
    assert_eq!(bytes.len() % 4, 0, "SPIR-V bytecode size must be a multiple of 4");
    let words: Vec<u32> = bytes.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();

    let info = vk::ShaderModuleCreateInfo::builder().code(&words);
    unsafe { device.create_shader_module(&info, None) }
}
