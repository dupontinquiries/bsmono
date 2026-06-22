//! Bloom: bright-pass + half-res downsample, composited additively into the
//! present pass in LINEAR space (before the tonemap). Gated by `UNENDLICH_BLOOM`,
//! which requires `UNENDLICH_POSTFX` (bloom samples the linear-HDR offscreen
//! scene that post-FX produces).
//!
//! Fragment-based on purpose: it reuses the proven offscreen-target +
//! sampling-render-pass + fullscreen-triangle pattern from `PostFx`/
//! `PresentPipeline` (the same one shipped in post-FX 1a/1b), so there are no
//! hand-rolled compute storage-image barriers — the bloom render pass'
//! final layout (SHADER_READ_ONLY) hands the result straight to the present
//! pass's sampler.
//!
//! This increment adds the separable Gaussian blur (ping-pong half-res) on top
//! of the bright-pass, turning it into a soft glow. `BLOOM_BLUR_PASSES = 0`
//! falls back to the raw bright-pass (the v0.24.104 look). See
//! toolkit/harness/RULESET.md §bloom.
//!
//! NOTE: `bloom_prefilter.frag` is compiled by the same shader path that builds
//! the post-FX shaders (`present.vert`/`present.frag`), which — like those —
//! are not in build.rs's explicit list; the dev shader-compile step covers the
//! whole `shaders/` directory.

use ash::{vk, Device};
use std::ffi::CStr;

use crate::render::context::VulkanContext;
use crate::render::offscreen::OffscreenTarget;
use crate::render::swapchain::create_bloom_render_pass;

// Bright-pass tunables (pushed to bloom_prefilter.frag). The scene is linear
// HDR, so a threshold near 1.0 catches highlights (sun disc, bright sky, water
// glints) while leaving the [0,1] midtones untouched. Dial to taste.
const BLOOM_THRESHOLD: f32 = 1.0;
const BLOOM_KNEE: f32 = 0.5;

/// Additive intensity pushed to present.frag when bloom is active (0 when off).
pub const BLOOM_INTENSITY: f32 = 0.05;

/// Mandatory color-attachment + storage-free, linear-sampled HDR format (same
/// as the scene offscreen). Holds bright energy > 1.0 so the glow is HDR-correct.
const BLOOM_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;

#[cfg(not(feature = "hot-shaders"))]
const VERT_SPV: &[u8] = include_bytes!("../../../../assets/shaders/present.vert.spv");
#[cfg(not(feature = "hot-shaders"))]
const PREFILTER_SPV: &[u8] = include_bytes!("../../../../assets/shaders/bloom_prefilter.frag.spv");

#[cfg(feature = "hot-shaders")]
fn load_vert_spv() -> Vec<u8> {
    crate::render::hot_shaders::read_spv("present.vert.spv")
}
#[cfg(feature = "hot-shaders")]
fn load_prefilter_spv() -> Vec<u8> {
    crate::render::hot_shaders::read_spv("bloom_prefilter.frag.spv")
}

#[cfg(not(feature = "hot-shaders"))]
const BLUR_SPV: &[u8] = include_bytes!("../../../../assets/shaders/bloom_blur.frag.spv");
#[cfg(feature = "hot-shaders")]
fn load_blur_spv() -> Vec<u8> {
    crate::render::hot_shaders::read_spv("bloom_blur.frag.spv")
}

/// Separable-blur iterations (each = one horizontal + one vertical pass).
/// 0 = bright-pass only (the v0.24.104 look — a handy A/B fallback); 1 = a soft
/// half-res glow; 2 widens it further. Tunable.
const BLOOM_BLUR_PASSES: u32 = 1;

pub struct Bloom {
    target: OffscreenTarget, // half-res RGBA16F bright-pass result
    render_pass: vk::RenderPass,
    framebuffer: vk::Framebuffer,
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    descriptor_set_layout: vk::DescriptorSetLayout,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    // Separable blur (ping-pong). `target` is "A" — the final result the present
    // pass samples; `scratch` is "B". The blur pipeline runs H (A->B) then V
    // (B->A), BLOOM_BLUR_PASSES times.
    scratch: OffscreenTarget,
    fb_b: vk::Framebuffer,
    blur_pipeline: vk::Pipeline,
    blur_layout: vk::PipelineLayout,
    blur_dsl: vk::DescriptorSetLayout,
    blur_pool: vk::DescriptorPool,
    blur_set_a: vk::DescriptorSet, // samples A -> writes B  (horizontal)
    blur_set_b: vk::DescriptorSet, // samples B -> writes A  (vertical)
    extent: vk::Extent2D, // half-res
}

impl Bloom {
    /// `Some(Bloom)` when `UNENDLICH_BLOOM` is set, else `Ok(None)`. The caller
    /// only constructs this when post-FX is active (it passes the offscreen
    /// scene view/sampler as the bright-pass source).
    pub fn new(
        ctx: &VulkanContext,
        full_extent: vk::Extent2D,
        scene_view: vk::ImageView,
        scene_sampler: vk::Sampler,
    ) -> Result<Option<Self>, vk::Result> {
        let enabled = std::env::var("UNENDLICH_BLOOM")
            .map(|v| v != "0" && !v.is_empty())
            .unwrap_or(false);
        if !enabled {
            return Ok(None);
        }
        let extent = half_extent(full_extent);
        log::info!(
            "UNENDLICH_BLOOM=1: bright-pass + half-res ({}x{}) bloom, additive composite (intensity {})",
            extent.width, extent.height, BLOOM_INTENSITY
        );

        let render_pass = create_bloom_render_pass(&ctx.device, BLOOM_FORMAT)
            .map_err(|_| vk::Result::ERROR_INITIALIZATION_FAILED)?;

        let target = OffscreenTarget::new(
            &ctx.instance,
            &ctx.device,
            ctx.physical_device,
            extent,
            BLOOM_FORMAT,
        )?;

        let (pipeline, layout, descriptor_set_layout, descriptor_pool, descriptor_set) =
            build_prefilter_pipeline(&ctx.device, render_pass)?;

        let scratch = OffscreenTarget::new(
            &ctx.instance,
            &ctx.device,
            ctx.physical_device,
            extent,
            BLOOM_FORMAT,
        )?;
        let (blur_pipeline, blur_layout, blur_dsl, blur_pool, blur_set_a, blur_set_b) =
            build_blur_pipeline(&ctx.device, render_pass)?;

        let mut b = Bloom {
            target,
            render_pass,
            framebuffer: vk::Framebuffer::null(),
            pipeline,
            layout,
            descriptor_set_layout,
            descriptor_pool,
            descriptor_set,
            scratch,
            fb_b: vk::Framebuffer::null(),
            blur_pipeline,
            blur_layout,
            blur_dsl,
            blur_pool,
            blur_set_a,
            blur_set_b,
            extent,
        };
        b.build_framebuffers(&ctx.device)?;
        b.write_scene(&ctx.device, scene_view, scene_sampler);
        b.write_blur_sets(&ctx.device);
        Ok(Some(b))
    }

    fn build_framebuffers(&mut self, device: &Device) -> Result<(), vk::Result> {
        self.framebuffer = make_fb(device, self.render_pass, self.target.view, self.extent)?;
        self.fb_b = make_fb(device, self.render_pass, self.scratch.view, self.extent)?;
        Ok(())
    }

    /// Point the two blur descriptor sets at the ping-pong targets: set_a reads
    /// A (target), set_b reads B (scratch). Re-call when the targets are recreated.
    fn write_blur_sets(&self, device: &Device) {
        write_sampler(device, self.blur_set_a, self.target.view, self.target.sampler);
        write_sampler(device, self.blur_set_b, self.scratch.view, self.scratch.sampler);
    }

    /// Point the bright-pass descriptor at the (linear HDR) scene offscreen.
    /// Call on creation and on every swapchain resize (the scene target is
    /// recreated; the set/pool persist).
    pub fn write_scene(&self, device: &Device, view: vk::ImageView, sampler: vk::Sampler) {
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

    /// Record the bright-pass into the half-res bloom target. Call AFTER the
    /// scene pass has ended (the scene offscreen is then SHADER_READ_ONLY) and
    /// BEFORE the present pass begins (this opens + closes its own render pass).
    pub fn record(&self, device: &Device, cmd: vk::CommandBuffer) {
        let begin = vk::RenderPassBeginInfo::builder()
            .render_pass(self.render_pass)
            .framebuffer(self.framebuffer)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.extent,
            });
        let vp = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: self.extent.width as f32,
            height: self.extent.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let sc = vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: self.extent,
        };
        let push = [BLOOM_THRESHOLD, BLOOM_KNEE];
        let push_bytes =
            unsafe { std::slice::from_raw_parts(push.as_ptr() as *const u8, std::mem::size_of_val(&push)) };
        unsafe {
            // load_op = DONT_CARE: the fullscreen triangle overwrites every texel.
            device.cmd_begin_render_pass(cmd, &begin, vk::SubpassContents::INLINE);
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            device.cmd_set_viewport(cmd, 0, std::slice::from_ref(&vp));
            device.cmd_set_scissor(cmd, 0, std::slice::from_ref(&sc));
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.layout,
                0,
                std::slice::from_ref(&self.descriptor_set),
                &[],
            );
            device.cmd_push_constants(
                cmd,
                self.layout,
                vk::ShaderStageFlags::FRAGMENT,
                0,
                push_bytes,
            );
            device.cmd_draw(cmd, 3, 1, 0, 0);
            device.cmd_end_render_pass(cmd);
        }

        // Separable blur, ping-ponging A<->B: H (A->B) then V (B->A), repeated.
        // With BLOOM_BLUR_PASSES = 0 this is skipped and `target` (A) holds the
        // raw bright-pass (the v0.24.104 look).
        let inv_w = 1.0 / self.extent.width as f32;
        let inv_h = 1.0 / self.extent.height as f32;
        for _ in 0..BLOOM_BLUR_PASSES {
            self.blur_pass(device, cmd, self.fb_b, self.blur_set_a, [inv_w, 0.0]); // H: A->B
            self.blur_pass(device, cmd, self.framebuffer, self.blur_set_b, [0.0, inv_h]); // V: B->A
        }
    }

    fn blur_pass(
        &self,
        device: &Device,
        cmd: vk::CommandBuffer,
        fb: vk::Framebuffer,
        set: vk::DescriptorSet,
        dir: [f32; 2],
    ) {
        let begin = vk::RenderPassBeginInfo::builder()
            .render_pass(self.render_pass)
            .framebuffer(fb)
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.extent,
            });
        let vp = vk::Viewport {
            x: 0.0,
            y: 0.0,
            width: self.extent.width as f32,
            height: self.extent.height as f32,
            min_depth: 0.0,
            max_depth: 1.0,
        };
        let sc = vk::Rect2D {
            offset: vk::Offset2D { x: 0, y: 0 },
            extent: self.extent,
        };
        let push_bytes =
            unsafe { std::slice::from_raw_parts(dir.as_ptr() as *const u8, std::mem::size_of_val(&dir)) };
        unsafe {
            device.cmd_begin_render_pass(cmd, &begin, vk::SubpassContents::INLINE);
            device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.blur_pipeline);
            device.cmd_set_viewport(cmd, 0, std::slice::from_ref(&vp));
            device.cmd_set_scissor(cmd, 0, std::slice::from_ref(&sc));
            device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.blur_layout,
                0,
                std::slice::from_ref(&set),
                &[],
            );
            device.cmd_push_constants(
                cmd,
                self.blur_layout,
                vk::ShaderStageFlags::FRAGMENT,
                0,
                push_bytes,
            );
            device.cmd_draw(cmd, 3, 1, 0, 0);
            device.cmd_end_render_pass(cmd);
        }
    }
    pub fn view(&self) -> vk::ImageView {
        self.target.view
    }
    pub fn sampler(&self) -> vk::Sampler {
        self.target.sampler
    }

    /// Recreate the extent-dependent target + framebuffer after a swapchain
    /// resize (render pass + pipeline are extent-independent and kept). Caller
    /// must have device-idled and must re-point `write_scene` at the new scene
    /// offscreen afterwards.
    pub fn recreate(
        &mut self,
        ctx: &VulkanContext,
        full_extent: vk::Extent2D,
        scene_view: vk::ImageView,
        scene_sampler: vk::Sampler,
    ) -> Result<(), vk::Result> {
        unsafe {
            ctx.device.destroy_framebuffer(self.framebuffer, None);
            ctx.device.destroy_framebuffer(self.fb_b, None);
        }
        self.target.destroy(&ctx.device);
        self.scratch.destroy(&ctx.device);
        self.extent = half_extent(full_extent);
        self.target = OffscreenTarget::new(
            &ctx.instance,
            &ctx.device,
            ctx.physical_device,
            self.extent,
            BLOOM_FORMAT,
        )?;
        self.scratch = OffscreenTarget::new(
            &ctx.instance,
            &ctx.device,
            ctx.physical_device,
            self.extent,
            BLOOM_FORMAT,
        )?;
        self.build_framebuffers(&ctx.device)?;
        self.write_scene(&ctx.device, scene_view, scene_sampler);
        self.write_blur_sets(&ctx.device);
        Ok(())
    }

    pub fn destroy(&mut self, ctx: &VulkanContext) {
        unsafe {
            ctx.device.destroy_framebuffer(self.framebuffer, None);
            ctx.device.destroy_framebuffer(self.fb_b, None);
            ctx.device.destroy_pipeline(self.pipeline, None);
            ctx.device.destroy_pipeline_layout(self.layout, None);
            ctx.device.destroy_descriptor_pool(self.descriptor_pool, None);
            ctx.device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
            ctx.device.destroy_pipeline(self.blur_pipeline, None);
            ctx.device.destroy_pipeline_layout(self.blur_layout, None);
            ctx.device.destroy_descriptor_pool(self.blur_pool, None);
            ctx.device.destroy_descriptor_set_layout(self.blur_dsl, None);
            ctx.device.destroy_render_pass(self.render_pass, None);
        }
        self.target.destroy(&ctx.device);
        self.scratch.destroy(&ctx.device);
    }
}

fn half_extent(e: vk::Extent2D) -> vk::Extent2D {
    vk::Extent2D {
        width: (e.width / 2).max(1),
        height: (e.height / 2).max(1),
    }
}

/// Fullscreen-triangle bright-pass pipeline (present.vert + bloom_prefilter.frag).
/// Modeled on `PresentPipeline`: no vertex input, no cull, no depth, no blend,
/// dynamic viewport/scissor. One sampler binding (the scene) + an 8-byte
/// fragment push constant (threshold, knee).
type PipelineBits = (
    vk::Pipeline,
    vk::PipelineLayout,
    vk::DescriptorSetLayout,
    vk::DescriptorPool,
    vk::DescriptorSet,
);
fn build_prefilter_pipeline(
    device: &Device,
    render_pass: vk::RenderPass,
) -> Result<PipelineBits, vk::Result> {
    #[cfg(not(feature = "hot-shaders"))]
    let (vert_module, frag_module) = (
        create_shader_module(device, VERT_SPV)?,
        create_shader_module(device, PREFILTER_SPV)?,
    );
    #[cfg(feature = "hot-shaders")]
    let (vert_module, frag_module) = {
        let v = load_vert_spv();
        let f = load_prefilter_spv();
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

    let vertex_input = vk::PipelineVertexInputStateCreateInfo::builder();
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::builder()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
        .primitive_restart_enable(false);
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
    let dynamic_state = vk::PipelineDynamicStateCreateInfo::builder().dynamic_states(&dyn_states);

    // set 0, binding 0 = scene color sampler.
    let dsl_bindings = [vk::DescriptorSetLayoutBinding::builder()
        .binding(0)
        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::FRAGMENT)
        .build()];
    let dsl_info = vk::DescriptorSetLayoutCreateInfo::builder().bindings(&dsl_bindings);
    let descriptor_set_layout = unsafe { device.create_descriptor_set_layout(&dsl_info, None)? };

    let set_layouts = [descriptor_set_layout];
    // Push constant: threshold + knee (2 x f32).
    let push_ranges = [vk::PushConstantRange {
        stage_flags: vk::ShaderStageFlags::FRAGMENT,
        offset: 0,
        size: 8,
    }];
    let layout_info = vk::PipelineLayoutCreateInfo::builder()
        .set_layouts(&set_layouts)
        .push_constant_ranges(&push_ranges);
    let layout = unsafe { device.create_pipeline_layout(&layout_info, None)? };

    let pool_sizes = [vk::DescriptorPoolSize {
        ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
        descriptor_count: 1,
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
        device.create_graphics_pipelines(vk::PipelineCache::null(), std::slice::from_ref(&info), None)
    }
    .map_err(|(_, e)| e)?;

    unsafe {
        device.destroy_shader_module(vert_module, None);
        device.destroy_shader_module(frag_module, None);
    }

    Ok((pipelines[0], layout, descriptor_set_layout, descriptor_pool, descriptor_set))
}

fn make_fb(
    device: &Device,
    render_pass: vk::RenderPass,
    view: vk::ImageView,
    extent: vk::Extent2D,
) -> Result<vk::Framebuffer, vk::Result> {
    let attachments = [view];
    let info = vk::FramebufferCreateInfo::builder()
        .render_pass(render_pass)
        .attachments(&attachments)
        .width(extent.width)
        .height(extent.height)
        .layers(1);
    unsafe { device.create_framebuffer(&info, None) }
}

fn write_sampler(device: &Device, set: vk::DescriptorSet, view: vk::ImageView, sampler: vk::Sampler) {
    let image_info = [vk::DescriptorImageInfo {
        sampler,
        image_view: view,
        image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
    }];
    let write = vk::WriteDescriptorSet::builder()
        .dst_set(set)
        .dst_binding(0)
        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .image_info(&image_info)
        .build();
    unsafe { device.update_descriptor_sets(std::slice::from_ref(&write), &[]); }
}

/// Separable-blur pipeline (present.vert + bloom_blur.frag). Same fixed-function
/// state as the bright-pass; one sampler binding + an 8-byte fragment push
/// constant (vec2 `dir_texel`). Allocates TWO descriptor sets (one per ping-pong
/// source) so H and V each bind their own source without a per-frame rewrite.
type BlurBits = (
    vk::Pipeline,
    vk::PipelineLayout,
    vk::DescriptorSetLayout,
    vk::DescriptorPool,
    vk::DescriptorSet,
    vk::DescriptorSet,
);
fn build_blur_pipeline(device: &Device, render_pass: vk::RenderPass) -> Result<BlurBits, vk::Result> {
    #[cfg(not(feature = "hot-shaders"))]
    let (vert_module, frag_module) = (
        create_shader_module(device, VERT_SPV)?,
        create_shader_module(device, BLUR_SPV)?,
    );
    #[cfg(feature = "hot-shaders")]
    let (vert_module, frag_module) = {
        let v = load_vert_spv();
        let f = load_blur_spv();
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

    let vertex_input = vk::PipelineVertexInputStateCreateInfo::builder();
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::builder()
        .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
        .primitive_restart_enable(false);
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
    let dynamic_state = vk::PipelineDynamicStateCreateInfo::builder().dynamic_states(&dyn_states);

    let dsl_bindings = [vk::DescriptorSetLayoutBinding::builder()
        .binding(0)
        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::FRAGMENT)
        .build()];
    let dsl_info = vk::DescriptorSetLayoutCreateInfo::builder().bindings(&dsl_bindings);
    let descriptor_set_layout = unsafe { device.create_descriptor_set_layout(&dsl_info, None)? };

    let set_layouts = [descriptor_set_layout];
    let push_ranges = [vk::PushConstantRange {
        stage_flags: vk::ShaderStageFlags::FRAGMENT,
        offset: 0,
        size: 8, // vec2 dir_texel
    }];
    let layout_info = vk::PipelineLayoutCreateInfo::builder()
        .set_layouts(&set_layouts)
        .push_constant_ranges(&push_ranges);
    let layout = unsafe { device.create_pipeline_layout(&layout_info, None)? };

    let pool_sizes = [vk::DescriptorPoolSize {
        ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER,
        descriptor_count: 2,
    }];
    let pool_info = vk::DescriptorPoolCreateInfo::builder()
        .pool_sizes(&pool_sizes)
        .max_sets(2);
    let descriptor_pool = unsafe { device.create_descriptor_pool(&pool_info, None)? };
    let alloc_layouts = [descriptor_set_layout, descriptor_set_layout];
    let alloc_info = vk::DescriptorSetAllocateInfo::builder()
        .descriptor_pool(descriptor_pool)
        .set_layouts(&alloc_layouts);
    let sets = unsafe { device.allocate_descriptor_sets(&alloc_info)? };

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
    }
    .map_err(|(_, e)| e)?;

    unsafe {
        device.destroy_shader_module(vert_module, None);
        device.destroy_shader_module(frag_module, None);
    }

    Ok((pipelines[0], layout, descriptor_set_layout, descriptor_pool, sets[0], sets[1]))
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
