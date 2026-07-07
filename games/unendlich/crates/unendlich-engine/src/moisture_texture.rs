//! moisture_texture.rs — GPU side of the moisture sim (fog approach 4).
//!
//! ⚠️  DRAFT — NOT registered in lib.rs yet. It is intentionally left out of the
//! module tree so it can't break the build you use to see approach 3. Register
//! it (`pub mod moisture_texture;`) and wire it per `harness/GPU_BRIDGE.md` when
//! we light up approach 4 — that's done as one unit (binding 4 + descriptor
//! write + bin tick) so it can be debugged together. This is blind Vulkan
//! mirrored from `froxel.rs`; expect to shake out a line or two on first build.
//!
//! A small 2D single-channel (R32_SFLOAT) texture holding the 2.5D moisture
//! field's per-cell visible fog density (`MoistureField::fog_density`). World-
//! anchored: the shader maps world XZ -> [0,1] UV via the field's world rect
//! (passed in spare UBO slots, see GPU_BRIDGE.md), so it never swims. Uploaded
//! each sim tick from a persistent host-visible staging buffer.

use ash::{vk, Device, Instance};

pub struct MoistureTexture {
    pub format: vk::Format,
    pub image: vk::Image,
    pub memory: vk::DeviceMemory,
    pub view: vk::ImageView,
    pub sampler: vk::Sampler,
    pub width: u32,
    pub height: u32,
    staging: vk::Buffer,
    staging_mem: vk::DeviceMemory,
    staging_ptr: *mut f32,
    initialized: bool, // image transitioned to SHADER_READ at least once
}

impl MoistureTexture {
    pub fn new(
        instance: &Instance,
        device: &Device,
        physical_device: vk::PhysicalDevice,
        width: u32,
        height: u32,
    ) -> Result<Self, vk::Result> {
        let format = vk::Format::R32_SFLOAT;
        let mem_props =
            unsafe { instance.get_physical_device_memory_properties(physical_device) };

        // ── sampled 2D image (device-local) ──────────────────────────────
        let image_info = vk::ImageCreateInfo::builder()
            .image_type(vk::ImageType::TYPE_2D)
            .extent(vk::Extent3D { width, height, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .format(format)
            .tiling(vk::ImageTiling::OPTIMAL)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
            .samples(vk::SampleCountFlags::TYPE_1)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let image = unsafe { device.create_image(&image_info, None)? };
        let req = unsafe { device.get_image_memory_requirements(image) };
        let mt = find_memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL, &mem_props)
            .ok_or(vk::Result::ERROR_FORMAT_NOT_SUPPORTED)?;
        let memory = unsafe {
            device.allocate_memory(
                &vk::MemoryAllocateInfo::builder().allocation_size(req.size).memory_type_index(mt),
                None,
            )?
        };
        unsafe { device.bind_image_memory(image, memory, 0)?; }

        let view = unsafe {
            device.create_image_view(
                &vk::ImageViewCreateInfo::builder()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(format)
                    .subresource_range(color_range()),
                None,
            )?
        };

        // LINEAR sampler with edge clamp — bilinear lookup, no wraparound at the
        // field window edge (out-of-window samples read the boundary cell).
        let sampler = unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo::builder()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                    .anisotropy_enable(false)
                    .unnormalized_coordinates(false),
                None,
            )?
        };

        // ── persistent host-visible staging buffer (w*h f32) ─────────────
        let bytes = (width as vk::DeviceSize) * (height as vk::DeviceSize) * 4;
        let staging = unsafe {
            device.create_buffer(
                &vk::BufferCreateInfo::builder()
                    .size(bytes)
                    .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                    .sharing_mode(vk::SharingMode::EXCLUSIVE),
                None,
            )?
        };
        let sreq = unsafe { device.get_buffer_memory_requirements(staging) };
        let smt = find_memory_type(
            sreq.memory_type_bits,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
            &mem_props,
        ).ok_or(vk::Result::ERROR_FORMAT_NOT_SUPPORTED)?;
        let staging_mem = unsafe {
            device.allocate_memory(
                &vk::MemoryAllocateInfo::builder().allocation_size(sreq.size).memory_type_index(smt),
                None,
            )?
        };
        unsafe { device.bind_buffer_memory(staging, staging_mem, 0)?; }
        let staging_ptr = unsafe {
            device.map_memory(staging_mem, 0, bytes, vk::MemoryMapFlags::empty())? as *mut f32
        };

        Ok(Self {
            format, image, memory, view, sampler, width, height,
            staging, staging_mem, staging_ptr, initialized: false,
        })
    }

    /// Clear the image to 0 and leave it in SHADER_READ_ONLY_OPTIMAL so the
    /// chunk pass can sample it from frame 0 even before the first
    /// `record_upload`. One-time, own command buffer (mirrors
    /// `FroxelVolume::clear_to_sentinel`). Call once at renderer init.
    pub fn initialize_zero(
        &mut self,
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

            let to_dst = vk::ImageMemoryBarrier::builder()
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .image(self.image)
                .subresource_range(color_range())
                .build();
            device.cmd_pipeline_barrier(
                cmd, vk::PipelineStageFlags::TOP_OF_PIPE, vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(), &[], &[], std::slice::from_ref(&to_dst),
            );

            let clear = vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 0.0] };
            device.cmd_clear_color_image(
                cmd, self.image, vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &clear, std::slice::from_ref(&color_range()),
            );

            let to_read = vk::ImageMemoryBarrier::builder()
                .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
                .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ)
                .image(self.image)
                .subresource_range(color_range())
                .build();
            device.cmd_pipeline_barrier(
                cmd, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(), &[], &[], std::slice::from_ref(&to_read),
            );

            device.end_command_buffer(cmd)?;
            let cmd_bufs = [cmd];
            let submit = vk::SubmitInfo::builder().command_buffers(&cmd_bufs);
            device.queue_submit(graphics_queue, std::slice::from_ref(&submit), vk::Fence::null())?;
            device.queue_wait_idle(graphics_queue)?;
            device.free_command_buffers(command_pool, &cmd_bufs);
        }
        self.initialized = true;
        Ok(())
    }

    /// Record an upload of `data` (len must == width*height, row-major) into the
    /// command buffer: memcpy into the persistent staging buffer, then barrier +
    /// copy + barrier so the image is SHADER_READ_ONLY by the time the chunk
    /// pass samples it. Call once per sim tick, before the render pass.
    ///
    /// # Safety
    /// `cmd` must be in the recording state on a graphics-capable queue.
    pub unsafe fn record_upload(&mut self, device: &Device, cmd: vk::CommandBuffer, data: &[f32]) {
        debug_assert_eq!(data.len(), (self.width * self.height) as usize);
        std::ptr::copy_nonoverlapping(data.as_ptr(), self.staging_ptr, data.len());

        let (old, src_access, src_stage) = if self.initialized {
            (vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL, vk::AccessFlags::SHADER_READ, vk::PipelineStageFlags::FRAGMENT_SHADER)
        } else {
            (vk::ImageLayout::UNDEFINED, vk::AccessFlags::empty(), vk::PipelineStageFlags::TOP_OF_PIPE)
        };
        let to_dst = vk::ImageMemoryBarrier::builder()
            .old_layout(old)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_access_mask(src_access)
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .image(self.image)
            .subresource_range(color_range())
            .build();
        device.cmd_pipeline_barrier(
            cmd, src_stage, vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(), &[], &[], std::slice::from_ref(&to_dst),
        );

        let region = vk::BufferImageCopy::builder()
            .buffer_offset(0)
            .buffer_row_length(0)   // tightly packed
            .buffer_image_height(0)
            .image_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .image_offset(vk::Offset3D { x: 0, y: 0, z: 0 })
            .image_extent(vk::Extent3D { width: self.width, height: self.height, depth: 1 })
            .build();
        device.cmd_copy_buffer_to_image(
            cmd, self.staging, self.image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL, std::slice::from_ref(&region),
        );

        let to_read = vk::ImageMemoryBarrier::builder()
            .old_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .new_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .dst_access_mask(vk::AccessFlags::SHADER_READ)
            .image(self.image)
            .subresource_range(color_range())
            .build();
        device.cmd_pipeline_barrier(
            cmd, vk::PipelineStageFlags::TRANSFER, vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::DependencyFlags::empty(), &[], &[], std::slice::from_ref(&to_read),
        );
        self.initialized = true;
    }

    /// Descriptor image info for writing binding 4 (combined image sampler).
    pub fn descriptor_image_info(&self) -> vk::DescriptorImageInfo {
        vk::DescriptorImageInfo::builder()
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
            .image_view(self.view)
            .sampler(self.sampler)
            .build()
    }

    pub fn destroy(&mut self, device: &Device) {
        unsafe {
            device.unmap_memory(self.staging_mem);
            device.destroy_buffer(self.staging, None);
            device.free_memory(self.staging_mem, None);
            device.destroy_sampler(self.sampler, None);
            device.destroy_image_view(self.view, None);
            device.destroy_image(self.image, None);
            device.free_memory(self.memory, None);
        }
    }
}

#[inline]
fn color_range() -> vk::ImageSubresourceRange {
    vk::ImageSubresourceRange {
        aspect_mask: vk::ImageAspectFlags::COLOR,
        base_mip_level: 0,
        level_count: 1,
        base_array_layer: 0,
        layer_count: 1,
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
