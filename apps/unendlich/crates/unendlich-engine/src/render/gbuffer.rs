//! gbuffer.rs — the neural deferred G-buffer: a gated, parallel render target that
//! re-draws the chunk geometry through `chunk_gbuffer.frag` into three attachments
//! (normal / albedo / worldpos) the neural feature pass will consume.
//!
//! Gated by `UNENDLICH_NEURAL_GBUFFER` (constructed only when set), and a SEPARATE
//! pass from the main render: `WorldRenderer` draws chunks straight to the
//! swapchain, which can't carry extra attachments, so the G-buffer is its own pass
//! over its own images + depth. Default-off ⇒ byte-identical to r300.
//!
//! The pipeline REUSES the chunk pipeline's `layout` + per-frame descriptor set (so
//! the same set `WorldRenderer` already binds works here) and `chunk.vert` + its
//! vertex input; only the fragment shader, render pass, and color-blend attachment
//! count differ. It is an INERT producer beyond filling the images — nothing samples
//! them yet (the feature/shade passes are the next wiring). The render pass leaves
//! the color attachments in SHADER_READ_ONLY_OPTIMAL so that step is ready. See
//! `neural_shading_integration.md` §2/§2a. Indirect path only (the `use_indirect`
//! default); the non-indirect fallback skips it.

use crate::render::depth::DepthBuffer;
use crate::render::mesher::PackedVertex;
use crate::render::offscreen::OffscreenTarget;
use ash::{vk, Device, Instance};
use std::ffi::CStr;

// G-buffer attachment formats (the §2 contract): worldpos needs full fp32 (B1 — it
// is the hint-ray origin), normal/albedo tolerate fp16.
pub const FMT_NORMAL: vk::Format = vk::Format::R16G16B16A16_SFLOAT;
pub const FMT_ALBEDO: vk::Format = vk::Format::R16G16B16A16_SFLOAT;
pub const FMT_WORLDPOS: vk::Format = vk::Format::R32G32B32A32_SFLOAT;

#[cfg(not(feature = "hot-shaders"))]
const VERT_SPV: &[u8] = include_bytes!("../../../../assets/shaders/chunk.vert.spv");
#[cfg(not(feature = "hot-shaders"))]
const GBUF_FRAG_SPV: &[u8] = include_bytes!("../../../../assets/shaders/chunk_gbuffer.frag.spv");

pub struct NeuralGBuffer {
    // Attachment order matches the render pass / framebuffer: 0 normal, 1 albedo,
    // 2 worldpos, 3 depth.
    pub normal: OffscreenTarget,
    pub albedo: OffscreenTarget,
    pub worldpos: OffscreenTarget,
    depth: DepthBuffer,
    pub render_pass: vk::RenderPass,
    pub framebuffer: vk::Framebuffer,
    pub pipeline: vk::Pipeline,
    pub extent: vk::Extent2D,
}

impl NeuralGBuffer {
    /// `chunk_layout`: the chunk pipeline's `PipelineLayout`, reused so the same
    /// per-frame descriptor set binds against this pipeline unchanged.
    pub fn new(
        instance: &Instance,
        device: &Device,
        physical_device: vk::PhysicalDevice,
        extent: vk::Extent2D,
        chunk_layout: vk::PipelineLayout,
    ) -> Result<Self, vk::Result> {
        let normal = OffscreenTarget::new(instance, device, physical_device, extent, FMT_NORMAL)?;
        let albedo = OffscreenTarget::new(instance, device, physical_device, extent, FMT_ALBEDO)?;
        let worldpos = OffscreenTarget::new(instance, device, physical_device, extent, FMT_WORLDPOS)?;
        let depth = DepthBuffer::new(instance, device, physical_device, extent)?;

        let render_pass = create_render_pass(device, depth.format)?;
        let views = [normal.view, albedo.view, worldpos.view, depth.view];
        let fb_info = vk::FramebufferCreateInfo::builder()
            .render_pass(render_pass)
            .attachments(&views)
            .width(extent.width)
            .height(extent.height)
            .layers(1);
        let framebuffer = unsafe { device.create_framebuffer(&fb_info, None)? };

        let pipeline = create_pipeline(device, render_pass, chunk_layout)?;

        Ok(Self { normal, albedo, worldpos, depth, render_pass, framebuffer, pipeline, extent })
    }

    pub fn destroy(&mut self, device: &Device) {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_framebuffer(self.framebuffer, None);
            device.destroy_render_pass(self.render_pass, None);
        }
        self.depth.destroy(device);
        self.normal.destroy(device);
        self.albedo.destroy(device);
        self.worldpos.destroy(device);
    }
}

fn color_attachment(format: vk::Format) -> vk::AttachmentDescription {
    vk::AttachmentDescription::builder()
        .format(format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
        .build()
}

fn create_render_pass(device: &Device, depth_format: vk::Format) -> Result<vk::RenderPass, vk::Result> {
    let attachments = [
        color_attachment(FMT_NORMAL),
        color_attachment(FMT_ALBEDO),
        color_attachment(FMT_WORLDPOS),
        vk::AttachmentDescription::builder()
            .format(depth_format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::DONT_CARE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
            .build(),
    ];
    let color_refs = [
        vk::AttachmentReference { attachment: 0, layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL },
        vk::AttachmentReference { attachment: 1, layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL },
        vk::AttachmentReference { attachment: 2, layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL },
    ];
    let depth_ref = vk::AttachmentReference {
        attachment: 3,
        layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
    };
    let subpass = vk::SubpassDescription::builder()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&color_refs)
        .depth_stencil_attachment(&depth_ref)
        .build();
    let subpasses = [subpass];
    // External→pass acquire, and pass→external release into SHADER_READ so the
    // (future) feature/shade pass can sample the color attachments.
    let deps = [
        vk::SubpassDependency {
            src_subpass: vk::SUBPASS_EXTERNAL,
            dst_subpass: 0,
            src_stage_mask: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            dst_stage_mask: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
            src_access_mask: vk::AccessFlags::empty(),
            dst_access_mask: vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
            dependency_flags: vk::DependencyFlags::empty(),
        },
        vk::SubpassDependency {
            src_subpass: 0,
            dst_subpass: vk::SUBPASS_EXTERNAL,
            src_stage_mask: vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            dst_stage_mask: vk::PipelineStageFlags::FRAGMENT_SHADER
                | vk::PipelineStageFlags::COMPUTE_SHADER,
            src_access_mask: vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            dst_access_mask: vk::AccessFlags::SHADER_READ,
            dependency_flags: vk::DependencyFlags::empty(),
        },
    ];
    let info = vk::RenderPassCreateInfo::builder()
        .attachments(&attachments)
        .subpasses(&subpasses)
        .dependencies(&deps);
    unsafe { device.create_render_pass(&info, None) }
}

fn create_pipeline(
    device: &Device,
    render_pass: vk::RenderPass,
    layout: vk::PipelineLayout,
) -> Result<vk::Pipeline, vk::Result> {
    #[cfg(not(feature = "hot-shaders"))]
    let (vert_module, frag_module) = (
        create_shader_module(device, VERT_SPV)?,
        create_shader_module(device, GBUF_FRAG_SPV)?,
    );
    #[cfg(feature = "hot-shaders")]
    let (vert_module, frag_module) = {
        let v = crate::render::hot_shaders::read_spv("chunk.vert.spv");
        let f = crate::render::hot_shaders::read_spv("chunk_gbuffer.frag.spv");
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

    // Same packed 8-byte vertex format as the chunk pipeline (chunk.vert).
    let binding = vk::VertexInputBindingDescription {
        binding: 0,
        stride: PackedVertex::STRIDE,
        input_rate: vk::VertexInputRate::VERTEX,
    };
    let attrs = [
        vk::VertexInputAttributeDescription {
            binding: 0,
            location: 0,
            format: vk::Format::R32_UINT,
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

    // Default winding/cull matches the chunk pipeline's variants[0] (CW + BACK);
    // honor UNENDLICH_NO_CULL for parity with the forward pass.
    let no_cull = std::env::var("UNENDLICH_NO_CULL").ok().as_deref() == Some("1");
    let cull = if no_cull { vk::CullModeFlags::NONE } else { vk::CullModeFlags::BACK };
    let rasterizer = vk::PipelineRasterizationStateCreateInfo::builder()
        .depth_clamp_enable(false)
        .rasterizer_discard_enable(false)
        .polygon_mode(vk::PolygonMode::FILL)
        .line_width(1.0)
        .cull_mode(cull)
        .front_face(vk::FrontFace::CLOCKWISE)
        .depth_bias_enable(false);

    let multisample = vk::PipelineMultisampleStateCreateInfo::builder()
        .sample_shading_enable(false)
        .rasterization_samples(vk::SampleCountFlags::TYPE_1);

    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::builder()
        .depth_test_enable(true)
        .depth_write_enable(true)
        .depth_compare_op(vk::CompareOp::LESS)
        .depth_bounds_test_enable(false)
        .stencil_test_enable(false);

    // One blend-attachment per G-buffer color attachment (3), blend disabled.
    let one = vk::PipelineColorBlendAttachmentState::builder()
        .color_write_mask(vk::ColorComponentFlags::RGBA)
        .blend_enable(false)
        .build();
    let color_blend_attachments = [one, one, one];
    let color_blend = vk::PipelineColorBlendStateCreateInfo::builder()
        .logic_op_enable(false)
        .attachments(&color_blend_attachments);

    let dyn_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
    let dynamic_state = vk::PipelineDynamicStateCreateInfo::builder().dynamic_states(&dyn_states);

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
        .subpass(0)
        .build();

    let pipelines = unsafe {
        device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None)
    }
    .map_err(|(_, e)| e)?;

    unsafe {
        device.destroy_shader_module(vert_module, None);
        device.destroy_shader_module(frag_module, None);
    }
    Ok(pipelines[0])
}

fn create_shader_module(device: &Device, bytes: &[u8]) -> Result<vk::ShaderModule, vk::Result> {
    assert_eq!(bytes.len() % 4, 0);
    let words: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let info = vk::ShaderModuleCreateInfo::builder().code(&words);
    unsafe { device.create_shader_module(&info, None) }
}
