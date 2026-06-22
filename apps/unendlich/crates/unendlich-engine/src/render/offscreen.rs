//! Offscreen color target for the gated post-FX pipeline (UNENDLICH_POSTFX).
//!
//! When post-FX is active the scene is rendered here instead of straight to the
//! swapchain; a fullscreen present pass then samples this image (tonemapping /
//! bloom / SSR happen against it) and writes the swapchain. Modeled on
//! `depth.rs` — same image+memory+view pattern and `find_memory_type` helper,
//! plus a combined-image sampler so the present pass can `texture()` it.
//!
//! `format` is a parameter: increment 1a uses the swapchain's own color format
//! (so the existing chunk/entity/egui pipelines stay render-pass-compatible and
//! need no recreation), and the later HDR step switches it to
//! `R16G16B16A16_SFLOAT` for bloom/SSR. Usage is COLOR_ATTACHMENT | SAMPLED.

use ash::{vk, Device, Instance};

pub struct OffscreenTarget {
    pub format: vk::Format,
    pub image: vk::Image,
    pub memory: vk::DeviceMemory,
    pub view: vk::ImageView,
    pub sampler: vk::Sampler,
    pub extent: vk::Extent2D,
}

impl OffscreenTarget {
    pub fn new(
        instance: &Instance,
        device: &Device,
        physical_device: vk::PhysicalDevice,
        extent: vk::Extent2D,
        format: vk::Format,
    ) -> Result<Self, vk::Result> {
        let image_info = vk::ImageCreateInfo::builder()
            .image_type(vk::ImageType::TYPE_2D)
            .extent(vk::Extent3D { width: extent.width, height: extent.height, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .format(format)
            .tiling(vk::ImageTiling::OPTIMAL)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .usage(vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::SAMPLED)
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
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let view = unsafe { device.create_image_view(&view_info, None)? };

        // Combined sampler used by the present pass. Linear filter, clamp to
        // edge (no wrap artifacts at screen borders), no mips.
        let sampler_info = vk::SamplerCreateInfo::builder()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .anisotropy_enable(false)
            .border_color(vk::BorderColor::FLOAT_OPAQUE_BLACK)
            .unnormalized_coordinates(false)
            .compare_enable(false)
            .min_lod(0.0)
            .max_lod(0.0);
        let sampler = unsafe { device.create_sampler(&sampler_info, None)? };

        Ok(Self { format, image, memory, view, sampler, extent })
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
