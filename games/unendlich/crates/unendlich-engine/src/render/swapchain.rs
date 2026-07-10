//! Swapchain creation, image views, framebuffers.
//!
//! The swapchain is the chain of images we render into and present to the
//! surface. It needs to be recreated on window resize, so this module exposes
//! a `Swapchain::recreate` that throws away the old set and builds a new one.

use ash::extensions::khr;
use ash::{vk, Device, Instance};
use crate::render::context::VulkanContext;

#[derive(Debug)]
pub enum SwapchainError {
    Vulkan(vk::Result),
    NoSurfaceFormats,
    NoPresentModes,
}

impl From<vk::Result> for SwapchainError {
    fn from(r: vk::Result) -> Self { SwapchainError::Vulkan(r) }
}

impl std::fmt::Display for SwapchainError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SwapchainError::Vulkan(r) => write!(f, "Vulkan error: {:?}", r),
            SwapchainError::NoSurfaceFormats => write!(f, "Surface has no formats"),
            SwapchainError::NoPresentModes => write!(f, "Surface has no present modes"),
        }
    }
}
impl std::error::Error for SwapchainError {}

pub type Result<T> = std::result::Result<T, SwapchainError>;

/// Map a Vulkan present mode to gpu-kit's version-neutral name (None for modes gpu-kit's policy core
/// doesn't model — e.g. the shared-demand variants — which we simply don't offer to the policy).
fn vk_present_to_gpukit(m: vk::PresentModeKHR) -> Option<gpu_kit::PresentMode> {
    match m {
        vk::PresentModeKHR::FIFO => Some(gpu_kit::PresentMode::Fifo),
        vk::PresentModeKHR::FIFO_RELAXED => Some(gpu_kit::PresentMode::FifoRelaxed),
        vk::PresentModeKHR::MAILBOX => Some(gpu_kit::PresentMode::Mailbox),
        vk::PresentModeKHR::IMMEDIATE => Some(gpu_kit::PresentMode::Immediate),
        _ => None,
    }
}

/// Map gpu-kit's chosen present mode back to Vulkan. Total: every gpu-kit variant has a vk peer.
fn gpukit_present_to_vk(m: gpu_kit::PresentMode) -> vk::PresentModeKHR {
    match m {
        gpu_kit::PresentMode::Fifo => vk::PresentModeKHR::FIFO,
        gpu_kit::PresentMode::FifoRelaxed => vk::PresentModeKHR::FIFO_RELAXED,
        gpu_kit::PresentMode::Mailbox => vk::PresentModeKHR::MAILBOX,
        gpu_kit::PresentMode::Immediate => vk::PresentModeKHR::IMMEDIATE,
    }
}

#[cfg(test)]
mod present_mode_tests {
    use super::*;

    #[test]
    fn vk_gpukit_present_roundtrips() {
        for m in [
            vk::PresentModeKHR::FIFO,
            vk::PresentModeKHR::FIFO_RELAXED,
            vk::PresentModeKHR::MAILBOX,
            vk::PresentModeKHR::IMMEDIATE,
        ] {
            assert_eq!(gpukit_present_to_vk(vk_present_to_gpukit(m).unwrap()), m);
        }
    }

    #[test]
    fn default_intent_picks_fifo_even_when_mailbox_is_offered() {
        // The stutter fix: with MAILBOX available, the default (Vsync) still chooses FIFO.
        let modes = [vk::PresentModeKHR::IMMEDIATE, vk::PresentModeKHR::MAILBOX, vk::PresentModeKHR::FIFO];
        let supported: Vec<_> = modes.iter().copied().filter_map(vk_present_to_gpukit).collect();
        let picked = gpu_kit::Policy { present: gpu_kit::Present::Vsync, ..Default::default() }
            .pick_present_mode(&supported);
        assert_eq!(gpukit_present_to_vk(picked), vk::PresentModeKHR::FIFO);

        // And low-latency intent still prefers MAILBOX when it's there.
        let picked_ll = gpu_kit::Policy { present: gpu_kit::Present::LowLatency, ..Default::default() }
            .pick_present_mode(&supported);
        assert_eq!(gpukit_present_to_vk(picked_ll), vk::PresentModeKHR::MAILBOX);
    }
}

pub struct Swapchain {
    pub handle: vk::SwapchainKHR,
    pub format: vk::Format,
    pub extent: vk::Extent2D,
    pub images: Vec<vk::Image>,
    pub image_views: Vec<vk::ImageView>,
    pub framebuffers: Vec<vk::Framebuffer>,
}

impl Swapchain {
    pub fn new(
        ctx: &VulkanContext,
        render_pass: vk::RenderPass,
        desired_extent: vk::Extent2D,
    ) -> Result<Self> {
        let caps = unsafe {
            ctx.surface_loader.get_physical_device_surface_capabilities(ctx.physical_device, ctx.surface)?
        };
        let formats = unsafe {
            ctx.surface_loader.get_physical_device_surface_formats(ctx.physical_device, ctx.surface)?
        };
        let modes = unsafe {
            ctx.surface_loader.get_physical_device_surface_present_modes(ctx.physical_device, ctx.surface)?
        };

        if formats.is_empty() { return Err(SwapchainError::NoSurfaceFormats); }
        if modes.is_empty()   { return Err(SwapchainError::NoPresentModes); }

        // Pick BGRA8 sRGB if present, else first format.
        let format = formats.iter().find(|f| {
            f.format == vk::Format::B8G8R8A8_SRGB
                && f.color_space == vk::ColorSpaceKHR::SRGB_NONLINEAR
        }).copied().unwrap_or(formats[0]);

        // Present mode via the shared gpu-kit policy core (version-agnostic: it selects over the
        // surface's own capability list and always falls back to FIFO). UNENDLICH_VSYNC picks the
        // *intent*; gpu-kit picks a supported mode for it.
        //
        //   UNENDLICH_VSYNC=1|on|fifo    → Vsync      (FIFO — no tearing, smoothest; the default)
        //   UNENDLICH_VSYNC=mailbox      → LowLatency (MAILBOX→IMMEDIATE→…→FIFO)
        //   UNENDLICH_VSYNC=immediate|tearing|0 → Fast (IMMEDIATE→MAILBOX→…→FIFO)
        //   default (unset)              → Vsync (FIFO)
        //
        // NOTE: the default is now FIFO, not MAILBOX. MAILBOX on Wayland/Asahi is the stutter source
        // (the compositor re-paces newest-wins presentation), so FIFO is the correct smooth baseline;
        // the low-latency modes remain one env var away for displays/compositors that handle them well.
        let present = match std::env::var("UNENDLICH_VSYNC").ok().as_deref() {
            Some("1") | Some("on") | Some("fifo") => gpu_kit::Present::Vsync,
            Some("mailbox") => gpu_kit::Present::LowLatency,
            Some("immediate") | Some("tearing") | Some("0") => gpu_kit::Present::Fast,
            _ => gpu_kit::Present::Vsync,
        };
        let supported: Vec<gpu_kit::PresentMode> = modes.iter().copied().filter_map(vk_present_to_gpukit).collect();
        let picked = gpu_kit::Policy { present, ..gpu_kit::Policy::default() }.pick_present_mode(&supported);
        let present_mode = gpukit_present_to_vk(picked);
        log::info!("Swapchain present mode: {:?} (intent {:?}), format: {:?}", present_mode, present, format.format);

        // Resolve extent — driver may impose CURRENT_EXTENT, otherwise we use
        // what we asked for, clamped.
        let extent = if caps.current_extent.width != u32::MAX {
            caps.current_extent
        } else {
            vk::Extent2D {
                width:  desired_extent.width.clamp(caps.min_image_extent.width, caps.max_image_extent.width),
                height: desired_extent.height.clamp(caps.min_image_extent.height, caps.max_image_extent.height),
            }
        };

        let mut image_count = caps.min_image_count + 1;
        if caps.max_image_count > 0 && image_count > caps.max_image_count {
            image_count = caps.max_image_count;
        }

        let qfs_unique = if ctx.graphics_queue_family != ctx.present_queue_family {
            vec![ctx.graphics_queue_family, ctx.present_queue_family]
        } else {
            vec![ctx.graphics_queue_family]
        };

        let mut info = vk::SwapchainCreateInfoKHR::builder()
            .surface(ctx.surface)
            .min_image_count(image_count)
            .image_format(format.format)
            .image_color_space(format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .pre_transform(caps.current_transform)
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(present_mode)
            .clipped(true);

        if qfs_unique.len() > 1 {
            info = info
                .image_sharing_mode(vk::SharingMode::CONCURRENT)
                .queue_family_indices(&qfs_unique);
        } else {
            info = info.image_sharing_mode(vk::SharingMode::EXCLUSIVE);
        }

        let handle = unsafe { ctx.swapchain_loader.create_swapchain(&info, None)? };
        let images = unsafe { ctx.swapchain_loader.get_swapchain_images(handle)? };

        let image_views = images.iter().map(|&image| {
            let view_info = vk::ImageViewCreateInfo::builder()
                .image(image)
                .view_type(vk::ImageViewType::TYPE_2D)
                .format(format.format)
                .components(vk::ComponentMapping::default())
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            unsafe { ctx.device.create_image_view(&view_info, None) }
        }).collect::<std::result::Result<Vec<_>, _>>()?;

        let framebuffers = image_views.iter().map(|&view| {
            let attachments = [view];
            let info = vk::FramebufferCreateInfo::builder()
                .render_pass(render_pass)
                .attachments(&attachments)
                .width(extent.width)
                .height(extent.height)
                .layers(1);
            unsafe { ctx.device.create_framebuffer(&info, None) }
        }).collect::<std::result::Result<Vec<_>, _>>()?;

        Ok(Self {
            handle, format: format.format, extent,
            images, image_views, framebuffers,
        })
    }

    pub fn destroy(&mut self, ctx: &VulkanContext) {
        unsafe {
            for &fb in &self.framebuffers {
                ctx.device.destroy_framebuffer(fb, None);
            }
            for &v in &self.image_views {
                ctx.device.destroy_image_view(v, None);
            }
            ctx.swapchain_loader.destroy_swapchain(self.handle, None);
        }
        self.framebuffers.clear();
        self.image_views.clear();
        self.images.clear();
    }

    /// Build framebuffers for a render pass that has both a color and a depth
    /// attachment. Replaces the framebuffers built in `new()`.
    ///
    /// This exists because we can't easily build the depth attachment until
    /// after we know the swapchain extent. So Swapchain::new builds
    /// color-only framebuffers; the chunk renderer then calls this to swap
    /// them out for color+depth ones.
    pub fn rebuild_framebuffers_with_depth(
        &mut self,
        ctx: &VulkanContext,
        render_pass: vk::RenderPass,
        depth_view: vk::ImageView,
    ) -> Result<()> {
        // Tear down old framebuffers.
        unsafe {
            for &fb in &self.framebuffers {
                ctx.device.destroy_framebuffer(fb, None);
            }
        }
        self.framebuffers.clear();

        for &view in &self.image_views {
            let attachments = [view, depth_view];
            let info = vk::FramebufferCreateInfo::builder()
                .render_pass(render_pass)
                .attachments(&attachments)
                .width(self.extent.width)
                .height(self.extent.height)
                .layers(1);
            let fb = unsafe { ctx.device.create_framebuffer(&info, None)? };
            self.framebuffers.push(fb);
        }

        Ok(())
    }
}

/// Create the simple single-color-attachment render pass that the triangle uses.
pub fn create_render_pass(device: &Device, format: vk::Format) -> Result<vk::RenderPass> {
    let color_attachment = vk::AttachmentDescription::builder()
        .format(format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);

    let color_ref = vk::AttachmentReference {
        attachment: 0,
        layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
    };
    let color_refs = [color_ref];

    let subpass = vk::SubpassDescription::builder()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&color_refs);

    let dep = vk::SubpassDependency::builder()
        .src_subpass(vk::SUBPASS_EXTERNAL)
        .dst_subpass(0)
        .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .src_access_mask(vk::AccessFlags::empty())
        .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
        .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE);

    let attachments = [*color_attachment];
    let subpasses = [*subpass];
    let deps = [*dep];

    let info = vk::RenderPassCreateInfo::builder()
        .attachments(&attachments)
        .subpasses(&subpasses)
        .dependencies(&deps);

    Ok(unsafe { device.create_render_pass(&info, None)? })
}

/// Render pass with both color and depth attachments. Used by the chunk
/// renderer.
pub fn create_render_pass_with_depth(
    device: &Device,
    color_format: vk::Format,
    depth_format: vk::Format,
) -> Result<vk::RenderPass> {
    let color_attachment = vk::AttachmentDescription::builder()
        .format(color_format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::STORE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::PRESENT_SRC_KHR);

    let depth_attachment = vk::AttachmentDescription::builder()
        .format(depth_format)
        .samples(vk::SampleCountFlags::TYPE_1)
        .load_op(vk::AttachmentLoadOp::CLEAR)
        .store_op(vk::AttachmentStoreOp::DONT_CARE)
        .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
        .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
        .initial_layout(vk::ImageLayout::UNDEFINED)
        .final_layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL);

    let color_ref = vk::AttachmentReference {
        attachment: 0,
        layout: vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
    };
    let depth_ref = vk::AttachmentReference {
        attachment: 1,
        layout: vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL,
    };
    let color_refs = [color_ref];

    let subpass = vk::SubpassDescription::builder()
        .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
        .color_attachments(&color_refs)
        .depth_stencil_attachment(&depth_ref);

    let dep = vk::SubpassDependency::builder()
        .src_subpass(vk::SUBPASS_EXTERNAL)
        .dst_subpass(0)
        .src_stage_mask(
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
        )
        .src_access_mask(vk::AccessFlags::empty())
        .dst_stage_mask(
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT
                | vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS,
        )
        .dst_access_mask(
            vk::AccessFlags::COLOR_ATTACHMENT_WRITE
                | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
        );

    let attachments = [*color_attachment, *depth_attachment];
    let subpasses = [*subpass];
    let deps = [*dep];

    let info = vk::RenderPassCreateInfo::builder()
        .attachments(&attachments)
        .subpasses(&subpasses)
        .dependencies(&deps);

    Ok(unsafe { device.create_render_pass(&info, None)? })
}

// silence unused import warning when we add Instance later
#[allow(dead_code)]
fn _unused(_i: &Instance, _l: &khr::Swapchain) {}
