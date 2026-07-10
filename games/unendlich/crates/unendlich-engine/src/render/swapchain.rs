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

        // v0.24.2c.10: present mode selection — three-way knob now.
        //
        //   UNENDLICH_VSYNC=1            → FIFO (capped to refresh, no tearing,
        //                                  CPU/GPU block on present queue —
        //                                  CHOOSE for quiet fans, accept latency).
        //   UNENDLICH_VSYNC=immediate    → IMMEDIATE (zero latency, may tear,
        //   UNENDLICH_VSYNC=tearing             CHOOSE for input-critical workflows
        //   UNENDLICH_VSYNC=0                   like first-person where tearing is
        //                                       less perceptible than latency).
        //   default (unset or =mailbox)  → MAILBOX (newest-wins, no tearing,
        //                                  CPU never waits on present —
        //                                  the no-latency baseline).
        //
        // MAILBOX is the c.10 default because the no-latency ship cares
        // about presented-frame freshness above all else. If MAILBOX
        // isn't supported (some Asahi/Wayland configurations), we fall
        // back to FIFO and log a warning so the user knows latency
        // mitigation is degraded.
        let vsync_request = std::env::var("UNENDLICH_VSYNC").ok();
        let present_mode = match vsync_request.as_deref() {
            Some("1") | Some("on") | Some("fifo") => vk::PresentModeKHR::FIFO,
            Some("immediate") | Some("tearing") => {
                if modes.contains(&vk::PresentModeKHR::IMMEDIATE) {
                    vk::PresentModeKHR::IMMEDIATE
                } else {
                    log::warn!(
                        "UNENDLICH_VSYNC=immediate requested but IMMEDIATE present mode \
                         isn't supported by this surface — falling back to MAILBOX"
                    );
                    if modes.contains(&vk::PresentModeKHR::MAILBOX) {
                        vk::PresentModeKHR::MAILBOX
                    } else {
                        log::warn!("MAILBOX not supported either — falling back to FIFO (latency degraded)");
                        vk::PresentModeKHR::FIFO
                    }
                }
            }
            // Default + explicit mailbox/0 — try MAILBOX, fall back to FIFO.
            _ => {
                if modes.contains(&vk::PresentModeKHR::MAILBOX) {
                    vk::PresentModeKHR::MAILBOX
                } else {
                    log::warn!(
                        "MAILBOX present mode not supported by this surface \
                         (driver/compositor limitation) — falling back to FIFO. \
                         Latency mitigation degraded. Try UNENDLICH_VSYNC=immediate \
                         if your display can tolerate tearing."
                    );
                    vk::PresentModeKHR::FIFO
                }
            }
        };
        log::info!("Swapchain present mode: {:?}, format: {:?}", present_mode, format.format);

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
