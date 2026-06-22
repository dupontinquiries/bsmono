//! Depth-only graphics pipeline for the shadow pre-pass.
//!
//! Differences from `ChunkPipeline`:
//!   - No fragment shader (depth-only). The vertex shader writes
//!     gl_Position; the rasterizer fills depth; nothing else runs.
//!   - No color attachment / no color blend state.
//!   - Front-face culling (vs back in the main pipeline). Drawing only
//!     the back faces of casters into the shadow map gives "back of
//!     caster" depth values, which biases nicely against
//!     "is this front-face self-shadowed?" comparisons.
//!   - Depth bias enabled (slope-scale + constant). Fights shadow
//!     acne caused by the same surface having both shadow-write
//!     and shadow-test happen at slightly different depth values
//!     because of the rasterizer's float quantization.
//!   - Same vertex input layout as ChunkPipeline so the same vertex
//!     buffers can be drawn into both pipelines without reshuffling.
//!   - Same descriptor set 0 binding 0 (CameraUbo) so both pipelines
//!     can share descriptor sets per-frame.

use ash::{vk, Device};
use std::ffi::CStr;
use crate::render::mesher::PackedVertex;

#[cfg(not(feature = "hot-shaders"))]
const VERT_SPV: &[u8] = include_bytes!("../../../../assets/shaders/chunk_shadow.vert.spv");

#[cfg(feature = "hot-shaders")]
fn load_vert_spv() -> Vec<u8> {
    crate::render::hot_shaders::read_spv("chunk_shadow.vert.spv")
}

pub struct ShadowPipeline {
    pub pipeline: vk::Pipeline,
    pub layout: vk::PipelineLayout,
    pub descriptor_set_layout: vk::DescriptorSetLayout,
}

impl ShadowPipeline {
    pub fn new(
        device: &Device,
        render_pass: vk::RenderPass,
    ) -> Result<Self, vk::Result> {
        #[cfg(not(feature = "hot-shaders"))]
        let vert_module = create_shader_module(device, VERT_SPV)?;
        #[cfg(feature = "hot-shaders")]
        let vert_module = {
            let v = load_vert_spv();
            create_shader_module(device, &v)?
        };
        let entry = unsafe { CStr::from_bytes_with_nul_unchecked(b"main\0") };

        // Only a vertex stage. No fragment shader — Vulkan permits
        // pipelines with only a depth attachment and no fragment
        // stage; the rasterizer fills depth without any per-fragment
        // execution.
        let stages = [
            vk::PipelineShaderStageCreateInfo::builder()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert_module)
                .name(entry)
                .build(),
        ];

        // Same vertex layout as the main pipeline (v0.24.20-vertex-pack
        // 8-byte packed format). The shadow shader doesn't use color but
        // we still declare it for layout compatibility with chunk.vert
        // — same vertex buffer, same descriptor set, same stride.
        let binding = vk::VertexInputBindingDescription {
            binding: 0,
            stride: PackedVertex::STRIDE, // 8
            input_rate: vk::VertexInputRate::VERTEX,
        };
        let attrs = [
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 0,
                format: vk::Format::R32_UINT, // r349: manual unpack in shader (Asahi UNORM quirk)
                offset: 0,
            },
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 1,
                format: vk::Format::R8G8B8A8_UNORM,
                offset: 4,
            },
        ];
        let bindings = [binding];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::builder()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attrs);

        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::builder()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
            .primitive_restart_enable(false);

        let viewport_state = vk::PipelineViewportStateCreateInfo::builder()
            .viewport_count(1)
            .scissor_count(1);

        // Front-face culling: draw only back faces into the shadow
        // map. Combined with the slope-scale bias, this is the
        // standard recipe for clean self-shadowing without acne or
        // peter-panning.
        //
        // Note: front_face is the OPPOSITE of the main pipeline.
        // Main uses CLOCKWISE (because the projection's Y-flip
        // reverses winding). Shadow rendering uses no Y-flip in its
        // projection (the projection is built with `ortho_rh_zo` in
        // light.rs without a [1][1] negation), so winding is back
        // to its natural CCW orientation. With FRONT culling, we
        // draw only the back-facing-from-light triangles.
        let rasterizer = vk::PipelineRasterizationStateCreateInfo::builder()
            .depth_clamp_enable(false)
            .rasterizer_discard_enable(false)
            .polygon_mode(vk::PolygonMode::FILL)
            .line_width(1.0)
            .cull_mode(vk::CullModeFlags::FRONT)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            // Slope-scale + constant depth bias to fight shadow acne.
            // Tuning notes:
            //   constant_factor: applied in normalized depth units
            //     after rasterization. 1.25 is standard.
            //   slope_factor: scaled by the maximum depth slope of
            //     the polygon. 1.75 lets steeply-angled surfaces get
            //     more bias (they need it more — the depth gradient
            //     across a single shadow texel is larger).
            // Adjust if you see peter-panning (shadow detached from
            // caster, bias too high) or acne (jagged self-shadow,
            // bias too low).
            .depth_bias_enable(true)
            .depth_bias_constant_factor(1.25)
            .depth_bias_slope_factor(1.75);

        let multisample = vk::PipelineMultisampleStateCreateInfo::builder()
            .sample_shading_enable(false)
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);

        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::builder()
            .depth_test_enable(true)
            .depth_write_enable(true)
            .depth_compare_op(vk::CompareOp::LESS)
            .depth_bounds_test_enable(false)
            .stencil_test_enable(false);

        // No color attachment → no color blend state needed for the
        // pipeline, but Vulkan still wants a (zero-sized) color blend
        // info struct.
        let color_blend = vk::PipelineColorBlendStateCreateInfo::builder()
            .logic_op_enable(false);

        let dyn_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state = vk::PipelineDynamicStateCreateInfo::builder()
            .dynamic_states(&dyn_states);

        // Descriptor set layout: must be compatible with the main chunk
        // pipeline's so the same `frame.descriptor_set` can be bound to
        // both. Bindings 0 (UBO), 1+2 (samplers, fragment-only — unused
        // here but required for set compatibility), and 3 (chunks SSBO,
        // v0.24.20-vertex-pack — used to read chunk_origin for vertex
        // unpacking).
        //
        // Per Vulkan's descriptor-set-compatibility rules, layouts are
        // compatible up to the bindings the pipeline references. The
        // shadow shader uses bindings 0 and 3; including the unused
        // 1+2 here keeps the layout structurally identical to the main
        // pipeline's so no per-pipeline descriptor allocation is needed.
        let dsl_bindings = [
            vk::DescriptorSetLayoutBinding::builder()
                .binding(0)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .build(),
            vk::DescriptorSetLayoutBinding::builder()
                .binding(1)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                .build(),
            vk::DescriptorSetLayoutBinding::builder()
                .binding(2)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                .build(),
            vk::DescriptorSetLayoutBinding::builder()
                .binding(3)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::VERTEX)
                .build(),
        ];
        let dsl_info = vk::DescriptorSetLayoutCreateInfo::builder()
            .bindings(&dsl_bindings);
        let descriptor_set_layout = unsafe { device.create_descriptor_set_layout(&dsl_info, None)? };

        let set_layouts = [descriptor_set_layout];
        // v0.24.2c: 64-byte push constant (one mat4) carrying the
        // current cascade's view-projection. The vertex shader reads
        // `cascade_view_proj` from here instead of from the shared
        // CameraUbo's `light_view_proj` field (which would race against
        // CPU writes between cascade render passes).
        let pcr = vk::PushConstantRange::builder()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(64)
            .build();
        let pcrs = [pcr];
        let layout_info = vk::PipelineLayoutCreateInfo::builder()
            .set_layouts(&set_layouts)
            .push_constant_ranges(&pcrs);
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
            device.create_graphics_pipelines(
                vk::PipelineCache::null(),
                std::slice::from_ref(&info),
                None,
            )
        }.map_err(|(_, e)| e)?;

        unsafe {
            device.destroy_shader_module(vert_module, None);
        }

        Ok(Self { pipeline: pipelines[0], layout, descriptor_set_layout })
    }

    pub fn destroy(&mut self, device: &Device) {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
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
