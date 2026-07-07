//! Shadow map: an offscreen depth-only render target used by the
//! shadow pre-pass.
//!
//! v0.24.2a: the image is now a 4-layer 2D array (allocated for future
//! cascade use). Only layer 0 is rendered into and sampled — the other
//! 3 layers are reserved for v0.24.2c (multi-cascade rendering) and
//! v0.24.2d (per-fragment cascade selection). No visual change.
//!
//! Memory: 4 × 1024² × 4B = 16MB total — identical to the previous
//! single-2048² configuration. The per-cascade resolution will drop
//! to 1024² in v0.24.2c when all four cascades are rendered; for
//! now layer 0 stays at SHADOW_MAP_SIZE (currently 2048).
//!
//! ## Why D32_SFLOAT
//!
//! Floating-point depth gives us a usable precision profile across
//! the [0, 1] depth range without the bit-packed quirks of D24/D16.
//! Vulkan guarantees `D32_SFLOAT` as a valid depth format, and our
//! existing `DepthBuffer::pick_depth_format` already prefers it.
//!
//! ## Why a separate sampler
//!
//! The shadow map is sampled in the fragment shader with comparison
//! filtering (`LESS_OR_EQUAL`) so PCF taps return 0/1 already-compared
//! shadow values instead of raw depths. This needs `compare_enable`
//! on the sampler — different from the regular sampler we'd use for
//! a color texture.
//!
//! ## Why CLAMP_TO_BORDER with white border
//!
//! Texels outside the shadow map's coverage area should read as "fully
//! lit" (the world is sunlit there), not "fully shadowed". Border
//! color = 1.0 (max depth) means the comparison returns 1 (not in
//! shadow). With `CLAMP_TO_EDGE` instead, the edge texel's value
//! would smear into the surroundings — wrong.

use ash::{vk, Device, Instance};

/// Default square edge of ONE shadow cascade in texels. Held at 2048
/// through v0.24.2c (4 cascades × 2048² × 4B = 64MB total — temporary
/// peak because C0 still covers the full ~200m in v0.24.2c).
/// v0.24.2d (not yet shipped) plans to shrink to 1024² simultaneous
/// with adopting the design's logarithmic splits.
///
/// v0.24.4 chug investigation: at rd=8, shadow's p50 GPU time is
/// 77ms (rendering ~32 chunks into the 2048² depth target).
/// `runtime_shadow_map_size()` lets users override the size at
/// startup via `UNENDLICH_SHADOW_MAP_SIZE` to test how much of the
/// shadow cost is fragment-bound (scales with map size) vs
/// vertex/draw-bound (independent of map size).
pub const SHADOW_MAP_SIZE_DEFAULT: u32 = 2048;

/// Resolve the shadow map size from the env. Accepts 512, 1024, or
/// 2048. Anything else is clamped to the default with a warning.
///
/// Read once at engine startup; baked into the `ShadowMap`'s `extent`
/// field. Mid-run changes aren't supported because the framebuffer +
/// image are sized at construction.
pub fn runtime_shadow_map_size() -> u32 {
    match std::env::var("UNENDLICH_SHADOW_MAP_SIZE") {
        Ok(v) => match v.parse::<u32>() {
            Ok(n @ (512 | 1024 | 2048)) => n,
            Ok(n) => {
                log::warn!(
                    "UNENDLICH_SHADOW_MAP_SIZE={} not in {{512,1024,2048}}; using default {}",
                    n, SHADOW_MAP_SIZE_DEFAULT,
                );
                SHADOW_MAP_SIZE_DEFAULT
            }
            Err(_) => {
                log::warn!(
                    "UNENDLICH_SHADOW_MAP_SIZE={} not a valid u32; using default {}",
                    v, SHADOW_MAP_SIZE_DEFAULT,
                );
                SHADOW_MAP_SIZE_DEFAULT
            }
        },
        Err(_) => SHADOW_MAP_SIZE_DEFAULT,
    }
}

/// Number of shadow cascades. v0.24.2a allocated the array; v0.24.2c
/// renders into all 4 layers.
pub const NUM_CASCADES: u32 = 4;

pub struct ShadowMap {
    pub format: vk::Format,
    pub image: vk::Image,
    pub memory: vk::DeviceMemory,
    /// 2D image view of layer 0. The main fragment shader's sampler
    /// descriptor binds this view to sample shadow depth. (Until CSM
    /// lands, only one cascade is rendered into; this view is the
    /// only one read by anything.)
    pub view: vk::ImageView,
    pub sampler: vk::Sampler,
    /// Framebuffer wrapping `view`. The shadow pre-pass renders into
    /// this framebuffer once per frame. v0.24.2c.6 reverted to using
    /// this single framebuffer after a c.5 bisect found that
    /// per-layer framebuffers caused a 2.8× shadow-cost regression
    /// on Honeykrisp (root cause not yet understood — filed for
    /// driver-investigation. The path forward for CSM is probably
    /// multiview rather than per-layer framebuffers).
    pub framebuffer: vk::Framebuffer,
    pub render_pass: vk::RenderPass,
    /// v0.24.4 (shadow-stride): a parallel render-pass + framebuffer
    /// pair whose loadOp is `LOAD` (not `CLEAR`) and initial_layout
    /// is `DEPTH_STENCIL_READ_ONLY_OPTIMAL`. Used when
    /// UNENDLICH_SHADOW_CHUNK_STRIDE > 1 to preserve previous frames'
    /// depth values for un-rerendered chunks. The framebuffer must
    /// be a separate object because it's tied to the render pass.
    /// We could share `view` between both framebuffers if we made the
    /// VkFramebufferCreateInfo's render_pass field "compatible" with
    /// both — but creating two FBs is cleaner.
    pub framebuffer_load: vk::Framebuffer,
    pub render_pass_load: vk::RenderPass,
    pub extent: vk::Extent2D,
}

impl ShadowMap {
    pub fn new(
        instance: &Instance,
        device: &Device,
        physical_device: vk::PhysicalDevice,
    ) -> Result<Self, vk::Result> {
        // v0.24.4: resolve the runtime shadow size (env-overridable).
        // Baked into `extent` here; the field on the returned struct
        // is what downstream code reads — no other consumer should
        // re-call runtime_shadow_map_size() and risk a disagreement.
        let size = runtime_shadow_map_size();
        log::info!("Shadow map size: {0}×{0}", size);
        let extent = vk::Extent2D {
            width: size,
            height: size,
        };
        let format = pick_depth_format(instance, physical_device)?;

        // The depth image. Usage: depth attachment + sampled (so the
        // main pass's fragment shader can read it).
        //
        // v0.24.2a: 4 array layers (1 used today, 3 reserved for CSM).
        // The image-view created below references only layer 0, so
        // current rendering and sampling are unchanged.
        let image_info = vk::ImageCreateInfo::builder()
            .image_type(vk::ImageType::TYPE_2D)
            .extent(vk::Extent3D { width: extent.width, height: extent.height, depth: 1 })
            .mip_levels(1)
            .array_layers(NUM_CASCADES)
            .format(format)
            .tiling(vk::ImageTiling::OPTIMAL)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .usage(
                vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT
                    | vk::ImageUsageFlags::SAMPLED,
            )
            .samples(vk::SampleCountFlags::TYPE_1)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let image = unsafe { device.create_image(&image_info, None)? };

        let req = unsafe { device.get_image_memory_requirements(image) };
        let mem_props = unsafe { instance.get_physical_device_memory_properties(physical_device) };
        let memory_type_index = find_memory_type(
            req.memory_type_bits,
            vk::MemoryPropertyFlags::DEVICE_LOCAL,
            &mem_props,
        ).ok_or(vk::Result::ERROR_FORMAT_NOT_SUPPORTED)?;

        let alloc_info = vk::MemoryAllocateInfo::builder()
            .allocation_size(req.size)
            .memory_type_index(memory_type_index);
        let memory = unsafe { device.allocate_memory(&alloc_info, None)? };
        unsafe { device.bind_image_memory(image, memory, 0)?; }

        let view_info = vk::ImageViewCreateInfo::builder()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .subresource_range(cascade_subresource_range(0));
        let view = unsafe { device.create_image_view(&view_info, None)? };

        // Sampler with comparison enabled and white border. PCF in the
        // fragment shader will use `texture(shadow_sampler, uv).r` —
        // because of compare_enable + LESS_OR_EQUAL, the returned r is
        // already 1.0 (not in shadow) or 0.0 (in shadow). LINEAR
        // filtering gives free 2×2 PCF.
        let sampler_info = vk::SamplerCreateInfo::builder()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_BORDER)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_BORDER)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_BORDER)
            .border_color(vk::BorderColor::FLOAT_OPAQUE_WHITE)
            .compare_enable(true)
            .compare_op(vk::CompareOp::LESS_OR_EQUAL)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .anisotropy_enable(false)
            .unnormalized_coordinates(false);
        let sampler = unsafe { device.create_sampler(&sampler_info, None)? };

        // Render pass: single depth attachment. No color attachment;
        // the shadow vertex shader has no fragment output, only depth.
        let depth_attachment = vk::AttachmentDescription::builder()
            .format(format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::CLEAR)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            // After the pass, the image is sampled by the main pass's
            // fragment shader, so transition to SHADER_READ_ONLY.
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL)
            .build();
        let depth_ref = vk::AttachmentReference::builder()
            .attachment(0)
            .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
            .build();
        let subpass = vk::SubpassDescription::builder()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .depth_stencil_attachment(&depth_ref)
            .build();
        // Subpass dependency: ensure the sampled-by-main-pass read
        // happens-after the shadow pass writes the depth.
        let dependencies = [
            // External → shadow subpass (clear → depth-write).
            vk::SubpassDependency::builder()
                .src_subpass(vk::SUBPASS_EXTERNAL)
                .dst_subpass(0)
                .src_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
                .src_access_mask(vk::AccessFlags::SHADER_READ)
                .dst_stage_mask(vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS)
                .dst_access_mask(vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE)
                .dependency_flags(vk::DependencyFlags::BY_REGION)
                .build(),
            // Shadow subpass → external (depth-write → main-pass sample).
            vk::SubpassDependency::builder()
                .src_subpass(0)
                .dst_subpass(vk::SUBPASS_EXTERNAL)
                .src_stage_mask(vk::PipelineStageFlags::LATE_FRAGMENT_TESTS)
                .src_access_mask(vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
                .dst_access_mask(vk::AccessFlags::SHADER_READ)
                .dependency_flags(vk::DependencyFlags::BY_REGION)
                .build(),
        ];
        let attachments = [depth_attachment];
        let subpasses = [subpass];
        let rp_info = vk::RenderPassCreateInfo::builder()
            .attachments(&attachments)
            .subpasses(&subpasses)
            .dependencies(&dependencies);
        let render_pass = unsafe { device.create_render_pass(&rp_info, None)? };

        // Framebuffer wrapping the depth view.
        let attachments = [view];
        let fb_info = vk::FramebufferCreateInfo::builder()
            .render_pass(render_pass)
            .attachments(&attachments)
            .width(extent.width)
            .height(extent.height)
            .layers(1);
        let framebuffer = unsafe { device.create_framebuffer(&fb_info, None)? };

        // v0.24.4 (shadow-stride): parallel LOAD-variant render pass.
        // Same single-depth-attachment shape, but loadOp = LOAD and
        // initial_layout = DEPTH_STENCIL_READ_ONLY_OPTIMAL (matching
        // the standard pass's final_layout) so we preserve previous
        // frames' depth values for un-rerendered chunks.
        //
        // The "External → subpass" dependency now src_stage is
        // FRAGMENT_SHADER (the previous frame's main-pass sample of
        // the shadow map). dst is the same: EARLY_FRAGMENT_TESTS for
        // this frame's depth-writes.
        let depth_attachment_load = vk::AttachmentDescription::builder()
            .format(format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::STORE)
            .stencil_load_op(vk::AttachmentLoadOp::DONT_CARE)
            .stencil_store_op(vk::AttachmentStoreOp::DONT_CARE)
            // Match the standard pass's final_layout. The shadow map
            // entered this layout when the previous frame's main pass
            // finished sampling it.
            .initial_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL)
            .final_layout(vk::ImageLayout::DEPTH_STENCIL_READ_ONLY_OPTIMAL)
            .build();
        let depth_ref_load = vk::AttachmentReference::builder()
            .attachment(0)
            .layout(vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
            .build();
        let subpass_load = vk::SubpassDescription::builder()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .depth_stencil_attachment(&depth_ref_load)
            .build();
        let dependencies_load = [
            vk::SubpassDependency::builder()
                .src_subpass(vk::SUBPASS_EXTERNAL)
                .dst_subpass(0)
                .src_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
                .src_access_mask(vk::AccessFlags::SHADER_READ)
                .dst_stage_mask(
                    vk::PipelineStageFlags::EARLY_FRAGMENT_TESTS
                    | vk::PipelineStageFlags::LATE_FRAGMENT_TESTS,
                )
                .dst_access_mask(
                    vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_READ
                    | vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE,
                )
                .dependency_flags(vk::DependencyFlags::BY_REGION)
                .build(),
            vk::SubpassDependency::builder()
                .src_subpass(0)
                .dst_subpass(vk::SUBPASS_EXTERNAL)
                .src_stage_mask(vk::PipelineStageFlags::LATE_FRAGMENT_TESTS)
                .src_access_mask(vk::AccessFlags::DEPTH_STENCIL_ATTACHMENT_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags::FRAGMENT_SHADER)
                .dst_access_mask(vk::AccessFlags::SHADER_READ)
                .dependency_flags(vk::DependencyFlags::BY_REGION)
                .build(),
        ];
        let attachments_load = [depth_attachment_load];
        let subpasses_load = [subpass_load];
        let rp_load_info = vk::RenderPassCreateInfo::builder()
            .attachments(&attachments_load)
            .subpasses(&subpasses_load)
            .dependencies(&dependencies_load);
        let render_pass_load = unsafe { device.create_render_pass(&rp_load_info, None)? };

        let attachments = [view];
        let fb_load_info = vk::FramebufferCreateInfo::builder()
            .render_pass(render_pass_load)
            .attachments(&attachments)
            .width(extent.width)
            .height(extent.height)
            .layers(1);
        let framebuffer_load = unsafe { device.create_framebuffer(&fb_load_info, None)? };

        Ok(Self {
            format,
            image,
            memory,
            view,
            sampler,
            framebuffer,
            render_pass,
            framebuffer_load,
            render_pass_load,
            extent,
        })
    }

    pub fn destroy(&mut self, device: &Device) {
        unsafe {
            device.destroy_framebuffer(self.framebuffer, None);
            device.destroy_framebuffer(self.framebuffer_load, None);
            device.destroy_render_pass(self.render_pass, None);
            device.destroy_render_pass(self.render_pass_load, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
            device.free_memory(self.memory, None);
        }
    }
}

fn pick_depth_format(instance: &Instance, pdev: vk::PhysicalDevice) -> Result<vk::Format, vk::Result> {
    let candidates = [
        vk::Format::D32_SFLOAT,
        vk::Format::D32_SFLOAT_S8_UINT,
        vk::Format::D24_UNORM_S8_UINT,
        vk::Format::D16_UNORM,
    ];
    for f in candidates {
        let props = unsafe { instance.get_physical_device_format_properties(pdev, f) };
        if props.optimal_tiling_features.contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT) {
            return Ok(f);
        }
    }
    Err(vk::Result::ERROR_FORMAT_NOT_SUPPORTED)
}

fn find_memory_type(
    type_filter: u32,
    flags: vk::MemoryPropertyFlags,
    props: &vk::PhysicalDeviceMemoryProperties,
) -> Option<u32> {
    for i in 0..props.memory_type_count {
        if type_filter & (1 << i) != 0
            && props.memory_types[i as usize].property_flags.contains(flags)
        {
            return Some(i);
        }
    }
    None
}

/// Compute the `VkImageSubresourceRange` that one cascade's image view
/// should reference. Pure function so unit tests can verify per-layer
/// wiring without a real Vulkan device.
///
/// Contract:
/// - `aspect_mask = DEPTH` (the shadow image has no stencil)
/// - `base_mip_level = 0, level_count = 1` (no mipmaps)
/// - `base_array_layer = layer, layer_count = 1` (one layer per cascade)
///
/// Failure mode this guards against: a future edit accidentally
/// passing a wrong `layer` value, or setting `layer_count` to anything
/// other than 1. Either would cause the framebuffer to write to or the
/// sampler to read from the wrong subresource.
pub fn cascade_subresource_range(layer: u32) -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::DEPTH,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: layer,
        layer_count: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// v0.24.2a pins NUM_CASCADES at 4. Subsequent ships rely on this
    /// value (cascade-fit dispatches one workgroup per cascade,
    /// render passes loop 0..NUM_CASCADES, etc.). Bumping it requires
    /// touching all consumers; failing this test is the canary.
    #[test]
    fn num_cascades_is_four() {
        assert_eq!(NUM_CASCADES, 4);
    }

    /// SHADOW_MAP_SIZE_DEFAULT is constant per cascade. At 1 cascade
    /// × 2048² (current v0.24.2a) or 4 cascades × 1024² (planned
    /// v0.24.2c), total memory is the same 16MB at D32_SFLOAT. This
    /// test pins the current default; users can override at runtime
    /// via `UNENDLICH_SHADOW_MAP_SIZE` (v0.24.4+).
    #[test]
    fn shadow_map_size_is_pinned() {
        assert_eq!(SHADOW_MAP_SIZE_DEFAULT, 2048);
    }

    // ===== Cascade subresource range wiring (v0.24.2c.2) =====
    //
    // The per-layer image views must each reference one and only one
    // array layer, with the right `base_array_layer` index. If any of
    // these fields drift, the framebuffer writes to a wrong layer or
    // a sampler reads garbage.

    /// Every cascade's subresource range references exactly that
    /// cascade's layer index.
    #[test]
    fn cascade_subresource_range_targets_one_layer() {
        for layer in 0..NUM_CASCADES {
            let r = cascade_subresource_range(layer);
            assert_eq!(r.base_array_layer, layer,
                "cascade {} has wrong base_array_layer", layer);
            assert_eq!(r.layer_count, 1,
                "cascade {} has wrong layer_count (must be 1, got {})",
                layer, r.layer_count);
        }
    }

    /// All cascades reference the depth aspect (the shadow image has
    /// no stencil component) at mip level 0 (the image has no mipmaps).
    #[test]
    fn cascade_subresource_range_aspect_and_mips_are_pinned() {
        for layer in 0..NUM_CASCADES {
            let r = cascade_subresource_range(layer);
            assert_eq!(r.aspect_mask, vk::ImageAspectFlags::DEPTH);
            assert_eq!(r.base_mip_level, 0);
            assert_eq!(r.level_count, 1);
        }
    }

    /// Different layers produce ranges that target different layers
    /// — sanity check that the function isn't a constant.
    #[test]
    fn cascade_subresource_range_distinguishes_layers() {
        let ranges: Vec<_> = (0..NUM_CASCADES)
            .map(cascade_subresource_range)
            .collect();
        for i in 0..ranges.len() {
            for j in 0..ranges.len() {
                if i == j { continue; }
                assert_ne!(
                    ranges[i].base_array_layer, ranges[j].base_array_layer,
                    "two different cascades produced the same layer index",
                );
            }
        }
    }

    /// Pin the legacy `view` field's expected layer. It must point at
    /// layer 0 — the main fragment shader's sampler descriptor binds
    /// this view and assumes layer-0 contents. In v0.24.2c only C0 is
    /// rendered into layer 0, so this is the layer the main pass
    /// reads. v0.24.2d will replace this view with a 2D_ARRAY view.
    #[test]
    fn legacy_shadow_view_points_at_layer_zero() {
        let r = cascade_subresource_range(0);
        assert_eq!(r.base_array_layer, 0);
        assert_eq!(r.layer_count, 1);
    }
}
