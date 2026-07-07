//! Volumetric froxel scattering volume — groundwork (v0.16 Phase 1).
//!
//! ## What a froxel volume is
//!
//! A "froxel" is a frustum-aligned voxel — a 3D cell in a grid that
//! tiles the camera's view frustum. The volume is sized
//! (FROXEL_X × FROXEL_Y × FROXEL_Z), where:
//!   - X and Y are screen-space tile coordinates (much lower than
//!     framebuffer resolution; we don't need per-pixel scattering
//!     because scattering is low-frequency).
//!   - Z is depth slice along the camera's forward axis, distributed
//!     LOGARITHMICALLY (more slices near the camera, fewer far away)
//!     so closer scattering gets more precision.
//!
//! Each froxel stores: `vec4(scattered_rgb, transmittance)`.
//!   - .rgb  = light scattered TOWARD the camera by particles in the
//!             froxel's volume, accumulated front-to-back along the
//!             camera ray. The fragment shader samples and adds this
//!             to the final color.
//!   - .a    = transmittance through the volume in front of this
//!             froxel. The fragment shader multiplies opaque-surface
//!             color by .a to get the "fog dimming" effect.
//!
//! In a complete froxel system, a compute shader runs each frame:
//!   1. Compute per-froxel density (humidity, height, canopy field,
//!      noise — the same inputs the fragment shader's march uses).
//!   2. Compute per-froxel in-scatter (sample shadow map, modulate
//!      by phase function and sun color).
//!   3. Integrate front-to-back along Z to produce the final
//!      `(scattered, transmittance)` per froxel.
//!   4. Optionally accumulate against the previous frame's volume
//!      (temporal reprojection) for smoother / cheaper output.
//!
//! Then chunk.frag does ONE trilinear sample of this volume per
//! fragment, and the entire fog march loop disappears.
//!
//! ## What v0.16 does (groundwork only)
//!
//! - Allocates the 3D image + view + sampler.
//! - Creates one descriptor binding (set 0, binding 2) the chunk
//!   pipeline can sample from.
//! - Initializes the volume to a SENTINEL value (`vec4(0, 0, 0, 0)`
//!   for all texels — transmittance=0 is the "not filled, use the
//!   fallback march" signal).
//! - Provides `bind_descriptor_image_info()` so WorldRenderer and
//!   ChunkRenderer can wire it.
//!
//! v0.16 does NOT yet:
//! - Run any per-frame fill (compute or graphics-into-3D).
//! - Have the fragment shader actually consume the volume; the
//!   fragment shader will check `transmittance == 0` and fall back
//!   to the existing march. This way visual behavior is identical
//!   to v0.15 — the v0.16 ship is purely an infrastructure layer
//!   that the v0.17 fill-path will plug into without any further
//!   wiring.
//!
//! ## Why this size
//!
//! 160×90×64 = 921,600 froxels × 8 bytes (RGBA16F) = ~7.4 MB.
//! 160×90 maps cleanly to a 16×12-pixel screen tile at 1080p (160=
//! 1920/12, 90=1080/12). 64 Z slices is the standard "enough for
//! smooth volumetric depth" count used in Frostbite/UE5/Decima.
//!
//! ## Format choice
//!
//! `R16G16B16A16_SFLOAT` — half-float RGBA. Range matches HDR scene
//! values; precision is plenty for scattering data. Gives ~7.4MB
//! at the chosen extent.
//!
//! ## Why `IMAGE_USAGE_STORAGE`
//!
//! The future fill path (compute shader writes per-froxel) needs
//! storage usage. We declare it now so the same image can be reused
//! when fill lands. SAMPLED is also declared for the read path
//! (chunk.frag will sample via 3D sampler).

use ash::{vk, Device, Instance};

/// Width of the froxel grid (X), in screen-space tiles.
pub const FROXEL_X: u32 = 160;
/// Height of the froxel grid (Y), in screen-space tiles.
pub const FROXEL_Y: u32 = 90;
/// Depth slices (Z), distributed logarithmically along the camera's
/// forward axis. 64 is the modern standard.
pub const FROXEL_Z: u32 = 64;

/// Near plane distance for the froxel volume (meters along view).
/// Below this, the fragment shader falls through to the per-fragment
/// march for the immediate-camera region — froxels are too coarse
/// near the camera to look right.
pub const FROXEL_NEAR_M: f32 = 0.5;

/// Far plane distance for the froxel volume (meters along view).
/// Beyond this, the fragment shader uses the per-fragment march for
/// distance haze. 200m matches the existing fog max.
pub const FROXEL_FAR_M: f32 = 200.0;

pub struct FroxelVolume {
    pub format: vk::Format,
    pub image: vk::Image,
    pub memory: vk::DeviceMemory,
    pub view: vk::ImageView,
    pub sampler: vk::Sampler,
    pub extent: vk::Extent3D,
}

impl FroxelVolume {
    pub fn new(
        instance: &Instance,
        device: &Device,
        physical_device: vk::PhysicalDevice,
    ) -> Result<Self, vk::Result> {
        let format = vk::Format::R16G16B16A16_SFLOAT;
        let extent = vk::Extent3D {
            width: FROXEL_X,
            height: FROXEL_Y,
            depth: FROXEL_Z,
        };

        // 3D image. Usage: STORAGE (for the future compute fill path)
        // + SAMPLED (for chunk.frag to read from). TRANSFER_DST so we
        // can clear it via cmd_clear_color_image at startup.
        let image_info = vk::ImageCreateInfo::builder()
            .image_type(vk::ImageType::TYPE_3D)
            .extent(extent)
            .mip_levels(1)
            .array_layers(1)
            .format(format)
            .tiling(vk::ImageTiling::OPTIMAL)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .usage(
                vk::ImageUsageFlags::STORAGE
                    | vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_DST,
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

        // 3D image view spanning the entire volume.
        let view_info = vk::ImageViewCreateInfo::builder()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_3D)
            .format(format)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let view = unsafe { device.create_image_view(&view_info, None)? };

        // Trilinear sampler with edge-clamp. The fragment shader
        // computes a froxel-space UVW from world position; if the
        // UVW falls outside [0,1] (sample is behind camera or beyond
        // far plane) we want a sentinel value, not wraparound noise.
        // Using CLAMP_TO_EDGE means out-of-bounds samples read as
        // the boundary froxel — combined with the boundary being
        // initialized to "unfilled sentinel", the fragment shader's
        // fallback codepath handles the edge cases.
        let sampler_info = vk::SamplerCreateInfo::builder()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .anisotropy_enable(false)
            .unnormalized_coordinates(false);
        let sampler = unsafe { device.create_sampler(&sampler_info, None)? };

        Ok(Self { format, image, memory, view, sampler, extent })
    }

    /// Clear the volume to the "unfilled sentinel" value. Called once
    /// at startup. The sentinel is `vec4(0, 0, 0, 0)`:
    ///   - .a = 0 (transmittance = 0) is the fragment shader's signal
    ///     that this froxel is unfilled and the per-fragment march
    ///     should be used as fallback. Once the fill pass lands, it
    ///     will write a non-zero alpha to every froxel, and the
    ///     shader will start consuming the trilinear samples.
    ///   - .rgb = 0 means even if accidentally consumed, the
    ///     contribution is dark, not flashy garbage.
    ///
    /// Also transitions the image from UNDEFINED to GENERAL layout
    /// (the fill path expects GENERAL; the sample path is happy to
    /// read from GENERAL too).
    pub fn clear_to_sentinel(
        &self,
        device: &Device,
        command_pool: vk::CommandPool,
        graphics_queue: vk::Queue,
    ) -> Result<(), vk::Result> {
        let cmd = unsafe {
            device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::builder()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )?[0]
        };
        unsafe {
            device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::builder()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;

            // UNDEFINED → TRANSFER_DST_OPTIMAL for the clear.
            let to_transfer = vk::ImageMemoryBarrier::builder()
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .image(self.image)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .build();
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                std::slice::from_ref(&to_transfer),
            );

            let clear_color = vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 0.0] };
            let range = vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            };
            device.cmd_clear_color_image(
                cmd,
                self.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &clear_color,
                std::slice::from_ref(&range),
            );

            // TRANSFER_DST_OPTIMAL → GENERAL for shader sampling +
            // future compute storage writes.
            let to_general = vk::ImageMemoryBarrier::builder()
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ)
                .image(self.image)
                .subresource_range(range)
                .build();
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                std::slice::from_ref(&to_general),
            );

            device.end_command_buffer(cmd)?;
            let cmd_bufs = [cmd];
            let submit = vk::SubmitInfo::builder().command_buffers(&cmd_bufs);
            device.queue_submit(
                graphics_queue,
                std::slice::from_ref(&submit),
                vk::Fence::null(),
            )?;
            device.queue_wait_idle(graphics_queue)?;
            device.free_command_buffers(command_pool, &cmd_bufs);
        }
        Ok(())
    }

    pub fn destroy(&mut self, device: &Device) {
        unsafe {
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
            device.free_memory(self.memory, None);
        }
    }
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
