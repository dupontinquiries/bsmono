//! Gated post-FX pipeline state (UNENDLICH_POSTFX), kept in one place so the
//! `world_renderer` integration is just a field + a few call sites.
//!
//! Bundles: the offscreen color target, the offscreen + present render passes,
//! the present pipeline, and the framebuffers, plus the per-frame recording of
//! the present pass. `MAX_FRAMES_IN_FLIGHT == 1`, so a single offscreen target
//! and a single present descriptor set are race-free (the CPU waits on the
//! frame fence before recording the next frame).
//!
//! Increment 1a: the offscreen target is swapchain-format and `present.frag` is
//! a passthrough copy, so `POSTFX=1` must look IDENTICAL to `POSTFX=0`. The HDR
//! step (1b) switches the format to R16G16B16A16_SFLOAT and moves the tonemap
//! into the present pass. See toolkit/harness/RULESET.md §ssr.

use ash::vk;
use crate::render::context::VulkanContext;
use crate::render::offscreen::OffscreenTarget;
use crate::render::present_pipeline::PresentPipeline;
use crate::render::bloom::{Bloom, BLOOM_INTENSITY};
use crate::render::swapchain::{Swapchain, create_offscreen_render_pass, create_present_render_pass};

pub struct PostFx {
    offscreen: OffscreenTarget,
    offscreen_render_pass: vk::RenderPass,
    present_render_pass: vk::RenderPass,
    present_pipeline: PresentPipeline,
    offscreen_framebuffer: vk::Framebuffer,
    present_framebuffers: Vec<vk::Framebuffer>,
    extent: vk::Extent2D,
    /// Bloom (UNENDLICH_BLOOM, requires post-FX). `None` when bloom is off; then
    /// present binding 1 is pointed at the scene image and intensity pushed as 0.
    bloom: Option<Bloom>,
}

impl PostFx {
    /// Scene color format for the offscreen target. `R16G16B16A16_SFLOAT` gives
    /// linear HDR headroom above 1.0 for moving the tonemap into the present
    /// pass (1c) and for bloom/SSR, and is a mandatory color-attachment +
    /// linear-sampled format (no capability query needed). The present pass
    /// stays swapchain-format. As of 1b the scene pipelines (chunk/entity) are
    /// built against the offscreen pass because this differs from the swapchain
    /// format; `present.frag` still copies, so the result stays identical (the
    /// chunk shader still tonemaps into [0,1]; HDR headroom is unused until 1c).
    const SCENE_HDR_FORMAT: vk::Format = vk::Format::R16G16B16A16_SFLOAT;

    /// `Some(PostFx)` when `UNENDLICH_POSTFX` is set, else `None` (the renderer
    /// then uses its existing direct-to-swapchain path, byte-identical to today).
    pub fn new(
        ctx: &VulkanContext,
        swapchain: &Swapchain,
        depth_view: vk::ImageView,
        depth_format: vk::Format,
    ) -> Result<Option<Self>, vk::Result> {
        let enabled = std::env::var("UNENDLICH_POSTFX")
            .map(|v| v != "0" && !v.is_empty())
            .unwrap_or(false);
        if !enabled {
            return Ok(None);
        }
        log::info!(
            "UNENDLICH_POSTFX=1: scene -> offscreen HDR ({:?}) -> present -> swapchain ({:?}) [increment 1b: HDR storage; present still copies]",
            Self::SCENE_HDR_FORMAT, swapchain.format
        );

        let offscreen_render_pass =
            create_offscreen_render_pass(&ctx.device, Self::SCENE_HDR_FORMAT, depth_format)
                .map_err(|_| vk::Result::ERROR_INITIALIZATION_FAILED)?;
        let present_render_pass = create_present_render_pass(&ctx.device, swapchain.format)
            .map_err(|_| vk::Result::ERROR_INITIALIZATION_FAILED)?;

        // vk handles are Copy, so passing `present_render_pass` to the pipeline
        // and also storing it in the struct is fine.
        let mut pf = PostFx {
            offscreen: OffscreenTarget::new(
                &ctx.instance,
                &ctx.device,
                ctx.physical_device,
                swapchain.extent,
                Self::SCENE_HDR_FORMAT,
            )?,
            offscreen_render_pass,
            present_render_pass,
            present_pipeline: PresentPipeline::new(&ctx.device, present_render_pass)?,
            offscreen_framebuffer: vk::Framebuffer::null(),
            present_framebuffers: Vec::new(),
            extent: swapchain.extent,
            bloom: None,
        };
        pf.build_framebuffers(ctx, swapchain, depth_view)?;
        pf.present_pipeline
            .write_offscreen(&ctx.device, pf.offscreen.view, pf.offscreen.sampler);

        // Bloom (UNENDLICH_BLOOM): a bright-pass that samples the linear-HDR
        // scene offscreen. Present binding 1 is the bloom result when active, or
        // the scene image itself when off (always a valid descriptor); the
        // intensity push constant is 0 when off, making the composite a no-op.
        pf.bloom = Bloom::new(ctx, swapchain.extent, pf.offscreen.view, pf.offscreen.sampler)?;
        match &pf.bloom {
            Some(b) => pf
                .present_pipeline
                .write_bloom(&ctx.device, b.view(), b.sampler()),
            None => pf
                .present_pipeline
                .write_bloom(&ctx.device, pf.offscreen.view, pf.offscreen.sampler),
        }
        Ok(Some(pf))
    }

    fn build_framebuffers(
        &mut self,
        ctx: &VulkanContext,
        swapchain: &Swapchain,
        depth_view: vk::ImageView,
    ) -> Result<(), vk::Result> {
        // Offscreen framebuffer: [offscreen color, shared depth].
        let attachments = [self.offscreen.view, depth_view];
        let info = vk::FramebufferCreateInfo::builder()
            .render_pass(self.offscreen_render_pass)
            .attachments(&attachments)
            .width(swapchain.extent.width)
            .height(swapchain.extent.height)
            .layers(1);
        self.offscreen_framebuffer = unsafe { ctx.device.create_framebuffer(&info, None)? };

        // Present framebuffers: one per swapchain image, color only.
        for &view in &swapchain.image_views {
            let att = [view];
            let info = vk::FramebufferCreateInfo::builder()
                .render_pass(self.present_render_pass)
                .attachments(&att)
                .width(swapchain.extent.width)
                .height(swapchain.extent.height)
                .layers(1);
            self.present_framebuffers
                .push(unsafe { ctx.device.create_framebuffer(&info, None)? });
        }
        self.extent = swapchain.extent;
        Ok(())
    }

    /// Render pass + framebuffer the main (scene) pass should target when
    /// post-FX is active.
    pub fn scene_target(&self) -> (vk::RenderPass, vk::Framebuffer) {
        (self.offscreen_render_pass, self.offscreen_framebuffer)
    }

    /// The offscreen (scene) render pass. The chunk/entity pipelines are built
    /// against this when post-FX is active, because its color format (HDR float)
    /// differs from the swapchain and pipelines must be render-pass-compatible
    /// with the pass they draw into.
    pub fn scene_render_pass(&self) -> vk::RenderPass {
        self.offscreen_render_pass
    }

    /// The present (final, swapchain) render pass. The egui overlay is built
    /// against this when post-FX is active, so the UI composites here — on the
    /// sRGB swapchain, on top of the copied scene — instead of inside the scene
    /// pass. Same color format as the old main pass, so egui looks identical.
    pub fn present_render_pass(&self) -> vk::RenderPass {
        self.present_render_pass
    }

    /// Begin the present pass and record the fullscreen copy of the offscreen
    /// color into the swapchain image, but DO NOT end the pass. The caller can
    /// then record the egui overlay (so the UI composites on top of the copied
    /// scene, on the sRGB swapchain, never tonemapped) and finally call
    /// [`end_present`]. Call after the scene pass has ended.
    pub fn begin_present_copy(&self, ctx: &VulkanContext, cmd: vk::CommandBuffer, image_index: u32) {
        // Bloom bright-pass first — its own render pass, recorded BEFORE the
        // present pass begins. The scene offscreen is SHADER_READ_ONLY after the
        // scene pass, so the bright-pass can sample it, and the bloom pass' final
        // layout makes its result available to the present sampler below.
        if let Some(b) = &self.bloom {
            b.record(&ctx.device, cmd);
        }
        let begin = vk::RenderPassBeginInfo::builder()
            .render_pass(self.present_render_pass)
            .framebuffer(self.present_framebuffers[image_index as usize])
            .render_area(vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.extent,
            });
        // No clear_values: present color load_op is DONT_CARE (quad overwrites
        // every pixel before egui blends on top).
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
        unsafe {
            ctx.device.cmd_begin_render_pass(cmd, &begin, vk::SubpassContents::INLINE);
            ctx.device.cmd_bind_pipeline(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.present_pipeline.pipeline,
            );
            ctx.device.cmd_set_viewport(cmd, 0, std::slice::from_ref(&vp));
            ctx.device.cmd_set_scissor(cmd, 0, std::slice::from_ref(&sc));
            ctx.device.cmd_bind_descriptor_sets(
                cmd,
                vk::PipelineBindPoint::GRAPHICS,
                self.present_pipeline.layout,
                0,
                std::slice::from_ref(&self.present_pipeline.descriptor_set),
                &[],
            );
            // bloom_intensity push constant: the configured intensity when bloom
            // is active, 0.0 when off (exact no-op composite -> identical to 1c).
            let intensity: f32 = if self.bloom.is_some() { BLOOM_INTENSITY } else { 0.0 };
            ctx.device.cmd_push_constants(
                cmd,
                self.present_pipeline.layout,
                vk::ShaderStageFlags::FRAGMENT,
                0,
                &intensity.to_ne_bytes(),
            );
            ctx.device.cmd_draw(cmd, 3, 1, 0, 0);
        }
    }

    /// End the present pass opened by [`begin_present_copy`].
    pub fn end_present(&self, ctx: &VulkanContext, cmd: vk::CommandBuffer) {
        unsafe {
            ctx.device.cmd_end_render_pass(cmd);
        }
    }

    /// Recreate extent-dependent resources after a swapchain resize. Render
    /// passes + pipeline are extent-independent and kept. Caller must have
    /// device-idled (the renderer's recreate_swapchain does).
    pub fn recreate(
        &mut self,
        ctx: &VulkanContext,
        swapchain: &Swapchain,
        depth_view: vk::ImageView,
    ) -> Result<(), vk::Result> {
        unsafe {
            ctx.device.destroy_framebuffer(self.offscreen_framebuffer, None);
            for &fb in &self.present_framebuffers {
                ctx.device.destroy_framebuffer(fb, None);
            }
        }
        self.present_framebuffers.clear();
        self.offscreen.destroy(&ctx.device);

        self.offscreen = OffscreenTarget::new(
            &ctx.instance,
            &ctx.device,
            ctx.physical_device,
            swapchain.extent,
            Self::SCENE_HDR_FORMAT,
        )?;
        self.build_framebuffers(ctx, swapchain, depth_view)?;
        self.present_pipeline
            .write_offscreen(&ctx.device, self.offscreen.view, self.offscreen.sampler);
        // Bloom target tracks the (half) swapchain extent; re-point both its
        // bright-pass source and present binding 1 at the recreated images.
        if self.bloom.is_some() {
            let (sv, ss) = (self.offscreen.view, self.offscreen.sampler);
            let b = self.bloom.as_mut().unwrap();
            b.recreate(ctx, swapchain.extent, sv, ss)?;
            let (bv, bs) = (b.view(), b.sampler());
            self.present_pipeline.write_bloom(&ctx.device, bv, bs);
        } else {
            self.present_pipeline
                .write_bloom(&ctx.device, self.offscreen.view, self.offscreen.sampler);
        }
        Ok(())
    }

    pub fn destroy(&mut self, ctx: &VulkanContext) {
        unsafe {
            ctx.device.destroy_framebuffer(self.offscreen_framebuffer, None);
            for &fb in &self.present_framebuffers {
                ctx.device.destroy_framebuffer(fb, None);
            }
            ctx.device.destroy_render_pass(self.offscreen_render_pass, None);
            ctx.device.destroy_render_pass(self.present_render_pass, None);
        }
        self.present_framebuffers.clear();
        self.present_pipeline.destroy(&ctx.device);
        self.offscreen.destroy(&ctx.device);
        if let Some(b) = &mut self.bloom {
            b.destroy(ctx);
        }
    }
}
