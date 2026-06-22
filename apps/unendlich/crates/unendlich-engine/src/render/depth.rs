//! Depth buffer for the chunk renderer.

use ash::{vk, Device, Instance};

pub struct DepthBuffer {
    pub format: vk::Format,
    pub image: vk::Image,
    pub memory: vk::DeviceMemory,
    pub view: vk::ImageView,
    pub extent: vk::Extent2D,
}

impl DepthBuffer {
    pub fn new(
        instance: &Instance,
        device: &Device,
        physical_device: vk::PhysicalDevice,
        extent: vk::Extent2D,
    ) -> Result<Self, vk::Result> {
        let format = pick_depth_format(instance, physical_device)?;

        let image_info = vk::ImageCreateInfo::builder()
            .image_type(vk::ImageType::TYPE_2D)
            .extent(vk::Extent3D { width: extent.width, height: extent.height, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .format(format)
            .tiling(vk::ImageTiling::OPTIMAL)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .usage(vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT)
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
                aspect_mask: vk::ImageAspectFlags::DEPTH,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        let view = unsafe { device.create_image_view(&view_info, None)? };

        Ok(Self { format, image, memory, view, extent })
    }

    pub fn destroy(&mut self, device: &Device) {
        unsafe {
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
            device.free_memory(self.memory, None);
        }
    }
}

fn pick_depth_format(instance: &Instance, pdev: vk::PhysicalDevice) -> Result<vk::Format, vk::Result> {
    // D32_SFLOAT is universally supported on desktop GPUs and Asahi.
    // Falls back to D24/D16 if not (rare).
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
