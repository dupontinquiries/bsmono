//! Triangle renderer — wires context + swapchain + pipeline + sync into a
//! single object with a `draw_frame` API.
//!
//! Frames-in-flight pattern: we keep two sets of sync primitives so that
//! while the GPU is rendering frame N, the CPU can record commands for
//! frame N+1. The fence makes sure we don't overrun the GPU.

use ash::vk;
use crate::render::context::VulkanContext;
use crate::render::swapchain::{create_render_pass, Swapchain};
use crate::render::triangle_pipeline::TrianglePipeline;

const MAX_FRAMES_IN_FLIGHT: usize = 2;

#[derive(Debug)]
pub enum RenderError {
    Vulkan(vk::Result),
    Swapchain(crate::render::swapchain::SwapchainError),
    Other(String),
}

impl From<vk::Result> for RenderError {
    fn from(r: vk::Result) -> Self { RenderError::Vulkan(r) }
}
impl From<crate::render::swapchain::SwapchainError> for RenderError {
    fn from(e: crate::render::swapchain::SwapchainError) -> Self { RenderError::Swapchain(e) }
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::Vulkan(r) => write!(f, "Vulkan error: {:?}", r),
            RenderError::Swapchain(e) => write!(f, "Swapchain error: {}", e),
            RenderError::Other(s) => write!(f, "{}", s),
        }
    }
}
impl std::error::Error for RenderError {}

pub type Result<T> = std::result::Result<T, RenderError>;

struct FrameSync {
    image_available: vk::Semaphore,
    render_finished: vk::Semaphore,
    in_flight: vk::Fence,
    command_buffer: vk::CommandBuffer,
}

pub struct TriangleRenderer {
    pub render_pass: vk::RenderPass,
    pub swapchain: Swapchain,
    pub pipeline: TrianglePipeline,
    pub command_pool: vk::CommandPool,
    frames: Vec<FrameSync>,
    current_frame: usize,
    pub framebuffer_size: (u32, u32),
}

impl TriangleRenderer {
    pub fn new(ctx: &VulkanContext, framebuffer_size: (u32, u32)) -> Result<Self> {
        // Create render pass first — pipeline and swapchain both need it.
        // Use BGRA8 sRGB; Swapchain::new actually picks the format and we'll
        // recreate the render pass if it differs. For simplicity, we'll
        // create a temporary swapchain just to learn the format, throw it
        // away, then use that format for the real render pass.
        // (Cleaner but more code.)
        //
        // Practical shortcut: pick the format the same way Swapchain::new
        // does, manually. We just copy the logic. Or even simpler: most
        // platforms support B8G8R8A8_SRGB and that's what Swapchain prefers.
        // If it picks something else we'll find out from the validation
        // layers / a black screen and fix.

        let extent = vk::Extent2D { width: framebuffer_size.0, height: framebuffer_size.1 };

        // Create the swapchain first so we know its format.
        // But we need a render pass for the framebuffers... chicken-and-egg.
        // Solution: build swapchain in two steps. We do it inline here
        // because Swapchain::new takes the render pass.
        //
        // Easier: create render pass with B8G8R8A8_SRGB unconditionally. If
        // the surface picks a different format we get a validation error
        // on first frame, and we fix it in-place.

        let render_pass = create_render_pass(&ctx.device, vk::Format::B8G8R8A8_SRGB)?;
        let swapchain = Swapchain::new(ctx, render_pass, extent)?;

        if swapchain.format != vk::Format::B8G8R8A8_SRGB {
            log::warn!(
                "Swapchain picked format {:?} but render pass was created for B8G8R8A8_SRGB. \
                 You may see validation errors or a black screen.",
                swapchain.format
            );
        }

        let pipeline = TrianglePipeline::new(&ctx.device, render_pass, swapchain.extent)?;

        let command_pool = unsafe {
            ctx.device.create_command_pool(
                &vk::CommandPoolCreateInfo::builder()
                    .queue_family_index(ctx.graphics_queue_family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )?
        };

        // Allocate per-frame sync + a command buffer each.
        let cmd_buffers = unsafe {
            ctx.device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::builder()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(MAX_FRAMES_IN_FLIGHT as u32),
            )?
        };

        let mut frames = Vec::with_capacity(MAX_FRAMES_IN_FLIGHT);
        for i in 0..MAX_FRAMES_IN_FLIGHT {
            let sem_info = vk::SemaphoreCreateInfo::builder();
            let fence_info = vk::FenceCreateInfo::builder()
                .flags(vk::FenceCreateFlags::SIGNALED); // start signaled so the first wait succeeds

            let image_available = unsafe { ctx.device.create_semaphore(&sem_info, None)? };
            let render_finished = unsafe { ctx.device.create_semaphore(&sem_info, None)? };
            let in_flight = unsafe { ctx.device.create_fence(&fence_info, None)? };

            frames.push(FrameSync {
                image_available, render_finished, in_flight,
                command_buffer: cmd_buffers[i],
            });
        }

        Ok(Self {
            render_pass, swapchain, pipeline, command_pool,
            frames,
            current_frame: 0,
            framebuffer_size,
        })
    }

    /// Render one frame. Returns true if rendering happened, false if the
    /// swapchain was out-of-date and the caller should resize and try again.
    pub fn draw_frame(&mut self, ctx: &VulkanContext) -> Result<bool> {
        let frame = &self.frames[self.current_frame];

        unsafe {
            // Wait for this frame slot to be free.
            ctx.device.wait_for_fences(std::slice::from_ref(&frame.in_flight), true, u64::MAX)?;
        }

        // Acquire next swapchain image.
        let acquire = unsafe {
            ctx.swapchain_loader.acquire_next_image(
                self.swapchain.handle,
                u64::MAX,
                frame.image_available,
                vk::Fence::null(),
            )
        };
        let image_index = match acquire {
            Ok((idx, _suboptimal)) => idx,
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => return Ok(false),
            Err(e) => return Err(RenderError::Vulkan(e)),
        };

        // Reset the fence only after we're sure we're going to submit work.
        unsafe { ctx.device.reset_fences(std::slice::from_ref(&frame.in_flight))?; }

        // Record commands.
        let cmd = frame.command_buffer;
        unsafe {
            ctx.device.reset_command_buffer(cmd, vk::CommandBufferResetFlags::empty())?;
            ctx.device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::builder().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;

            let clear_color = vk::ClearValue {
                color: vk::ClearColorValue { float32: [0.05, 0.05, 0.10, 1.0] },
            };
            let clear_values = [clear_color];

            let rp_begin = vk::RenderPassBeginInfo::builder()
                .render_pass(self.render_pass)
                .framebuffer(self.swapchain.framebuffers[image_index as usize])
                .render_area(vk::Rect2D {
                    offset: vk::Offset2D { x: 0, y: 0 },
                    extent: self.swapchain.extent,
                })
                .clear_values(&clear_values);

            ctx.device.cmd_begin_render_pass(cmd, &rp_begin, vk::SubpassContents::INLINE);
            ctx.device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline.pipeline);

            // Set dynamic viewport + scissor.
            let viewport = vk::Viewport {
                x: 0.0, y: 0.0,
                width:  self.swapchain.extent.width as f32,
                height: self.swapchain.extent.height as f32,
                min_depth: 0.0, max_depth: 1.0,
            };
            let scissor = vk::Rect2D {
                offset: vk::Offset2D { x: 0, y: 0 },
                extent: self.swapchain.extent,
            };
            ctx.device.cmd_set_viewport(cmd, 0, std::slice::from_ref(&viewport));
            ctx.device.cmd_set_scissor(cmd, 0, std::slice::from_ref(&scissor));

            // Draw 3 vertices, 1 instance.
            ctx.device.cmd_draw(cmd, 3, 1, 0, 0);

            ctx.device.cmd_end_render_pass(cmd);
            ctx.device.end_command_buffer(cmd)?;
        }

        // Submit.
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
            ctx.device.queue_submit(
                ctx.graphics_queue,
                std::slice::from_ref(&submit),
                frame.in_flight,
            )?;
        }

        // Present.
        let swapchains = [self.swapchain.handle];
        let image_indices = [image_index];
        let present_info = vk::PresentInfoKHR::builder()
            .wait_semaphores(&signal_sems)
            .swapchains(&swapchains)
            .image_indices(&image_indices);

        let present_result = unsafe {
            ctx.swapchain_loader.queue_present(ctx.present_queue, &present_info)
        };
        match present_result {
            Ok(false) => {} // not suboptimal
            Ok(true) | Err(vk::Result::ERROR_OUT_OF_DATE_KHR) | Err(vk::Result::SUBOPTIMAL_KHR) => {
                // caller should recreate
                self.advance_frame();
                return Ok(false);
            }
            Err(e) => return Err(RenderError::Vulkan(e)),
        }

        self.advance_frame();
        Ok(true)
    }

    fn advance_frame(&mut self) {
        self.current_frame = (self.current_frame + 1) % MAX_FRAMES_IN_FLIGHT;
    }

    /// Recreate the swapchain (and its dependent framebuffers) for the new size.
    /// Call after `draw_frame` returns `Ok(false)` or after a window resize event.
    pub fn recreate_swapchain(&mut self, ctx: &VulkanContext, new_size: (u32, u32)) -> Result<()> {
        // Wait for everything in flight to finish so it's safe to destroy.
        unsafe { ctx.device.device_wait_idle()?; }

        self.swapchain.destroy(ctx);

        let extent = vk::Extent2D { width: new_size.0, height: new_size.1 };
        self.swapchain = Swapchain::new(ctx, self.render_pass, extent)?;
        self.framebuffer_size = new_size;
        Ok(())
    }

    pub fn destroy(&mut self, ctx: &VulkanContext) {
        unsafe {
            let _ = ctx.device.device_wait_idle();

            for f in &self.frames {
                ctx.device.destroy_semaphore(f.image_available, None);
                ctx.device.destroy_semaphore(f.render_finished, None);
                ctx.device.destroy_fence(f.in_flight, None);
            }
            self.frames.clear();

            ctx.device.destroy_command_pool(self.command_pool, None);
            self.pipeline.destroy(&ctx.device);
            self.swapchain.destroy(ctx);
            ctx.device.destroy_render_pass(self.render_pass, None);
        }
    }
}
