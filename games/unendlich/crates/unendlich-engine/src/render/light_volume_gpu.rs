//! GPU side of `world::light_volume`: the toroidal 3D light texture the chunk
//! shader samples (descriptor binding 4), and the per-frame copies that keep it
//! current.
//!
//! One `R8G8B8A8_UNORM` 3D image, `[sky, r, g, b]` per cell, sampled with
//! LINEAR filtering and REPEAT addressing: the shader divides a world position
//! by the image size and the wrap does the toroidal mapping. The image stays in
//! GENERAL layout for its whole life (copies and sampling are both valid
//! there), so the only synchronisation is a barrier pair around each frame's
//! copies.
//!
//! Uploads go through a persistently mapped staging buffer with one slice per
//! frame in flight, so a slice is only rewritten after that frame's fence has
//! been waited on (the renderer waits before recording).
use std::collections::VecDeque;

use ash::{vk, Device, Instance};

use crate::world::light_volume::Upload;

const BOX: u32 = 32;
const BOX_BYTES: usize = (BOX * BOX * BOX * 4) as usize;

pub struct LightVolumeGpu {
    pub image: vk::Image,
    pub memory: vk::DeviceMemory,
    pub view: vk::ImageView,
    pub sampler: vk::Sampler,
    pub extent: vk::Extent3D,
    staging: Option<(vk::Buffer, vk::DeviceMemory, *mut u8)>,
    boxes_per_frame: usize,
    pending: VecDeque<Upload>,
    /// Boxes copied so far (for the trace line).
    pub uploaded: u64,
    /// Which chunk's data was last copied into each box, and its sky value
    /// at the box's middle cell (for the trace line: does the GPU hold what
    /// the CPU thinks it holds).
    pub owners: std::collections::HashMap<[u32; 3], ([i32; 3], u64)>,
}

// The raw staging pointer is only touched from the render thread that owns
// the renderer.
unsafe impl Send for LightVolumeGpu {}

impl LightVolumeGpu {
    /// Create the image (cleared to open sky: `[1, 0, 0, 0]`) and, when
    /// `boxes_per_frame > 0`, the staging buffer for `frames` frames in flight.
    /// `extent = [1, 1, 1]` with no staging is the placeholder for renderers
    /// that bind the chunk pipeline without a light loop.
    pub fn new(
        instance: &Instance,
        device: &Device,
        physical_device: vk::PhysicalDevice,
        command_pool: vk::CommandPool,
        queue: vk::Queue,
        extent: [u32; 3],
        frames: usize,
        boxes_per_frame: usize,
    ) -> Result<Self, vk::Result> {
        let format = vk::Format::R8G8B8A8_UNORM;
        let extent = vk::Extent3D { width: extent[0], height: extent[1], depth: extent[2] };
        let image_info = vk::ImageCreateInfo::builder()
            .image_type(vk::ImageType::TYPE_3D)
            .extent(extent)
            .mip_levels(1)
            .array_layers(1)
            .format(format)
            .tiling(vk::ImageTiling::OPTIMAL)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .usage(vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST)
            .samples(vk::SampleCountFlags::TYPE_1)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let image = unsafe { device.create_image(&image_info, None)? };
        let mem_props = unsafe { instance.get_physical_device_memory_properties(physical_device) };
        let req = unsafe { device.get_image_memory_requirements(image) };
        let mti = find_memory_type(req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL, &mem_props)
            .ok_or(vk::Result::ERROR_OUT_OF_DEVICE_MEMORY)?;
        let memory = unsafe {
            device.allocate_memory(
                &vk::MemoryAllocateInfo::builder().allocation_size(req.size).memory_type_index(mti),
                None,
            )?
        };
        unsafe { device.bind_image_memory(image, memory, 0)? };
        let range = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        let view = unsafe {
            device.create_image_view(
                &vk::ImageViewCreateInfo::builder()
                    .image(image)
                    .view_type(vk::ImageViewType::TYPE_3D)
                    .format(format)
                    .subresource_range(range),
                None,
            )?
        };
        let sampler = unsafe {
            device.create_sampler(
                &vk::SamplerCreateInfo::builder()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::REPEAT)
                    .address_mode_v(vk::SamplerAddressMode::REPEAT)
                    .address_mode_w(vk::SamplerAddressMode::REPEAT)
                    .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                    .anisotropy_enable(false)
                    .unnormalized_coordinates(false),
                None,
            )?
        };

        // Clear to open sky and move to GENERAL, once, synchronously.
        unsafe {
            let cmd = device.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::builder()
                    .command_pool(command_pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )?[0];
            device.begin_command_buffer(
                cmd,
                &vk::CommandBufferBeginInfo::builder().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )?;
            let to_general = vk::ImageMemoryBarrier::builder()
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .image(image)
                .subresource_range(range)
                .build();
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TOP_OF_PIPE,
                vk::PipelineStageFlags::TRANSFER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                std::slice::from_ref(&to_general),
            );
            device.cmd_clear_color_image(
                cmd,
                image,
                vk::ImageLayout::GENERAL,
                &vk::ClearColorValue { float32: [1.0, 0.0, 0.0, 0.0] },
                std::slice::from_ref(&range),
            );
            let to_read = vk::ImageMemoryBarrier::builder()
                .old_layout(vk::ImageLayout::GENERAL)
                .new_layout(vk::ImageLayout::GENERAL)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::SHADER_READ)
                .image(image)
                .subresource_range(range)
                .build();
            device.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                std::slice::from_ref(&to_read),
            );
            device.end_command_buffer(cmd)?;
            let bufs = [cmd];
            device.queue_submit(queue, &[*vk::SubmitInfo::builder().command_buffers(&bufs)], vk::Fence::null())?;
            device.queue_wait_idle(queue)?;
            device.free_command_buffers(command_pool, &bufs);
        }

        let staging = if boxes_per_frame > 0 {
            let size = (BOX_BYTES * boxes_per_frame * frames.max(1)) as u64;
            let buf = unsafe {
                device.create_buffer(
                    &vk::BufferCreateInfo::builder()
                        .size(size)
                        .usage(vk::BufferUsageFlags::TRANSFER_SRC)
                        .sharing_mode(vk::SharingMode::EXCLUSIVE),
                    None,
                )?
            };
            let req = unsafe { device.get_buffer_memory_requirements(buf) };
            let mti = find_memory_type(
                req.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                &mem_props,
            )
            .ok_or(vk::Result::ERROR_OUT_OF_HOST_MEMORY)?;
            let mem = unsafe {
                device.allocate_memory(
                    &vk::MemoryAllocateInfo::builder().allocation_size(req.size).memory_type_index(mti),
                    None,
                )?
            };
            unsafe { device.bind_buffer_memory(buf, mem, 0)? };
            let ptr = unsafe { device.map_memory(mem, 0, size, vk::MemoryMapFlags::empty())? } as *mut u8;
            Some((buf, mem, ptr))
        } else {
            None
        };

        Ok(Self {
            image,
            memory,
            view,
            sampler,
            extent,
            staging,
            boxes_per_frame,
            pending: VecDeque::new(),
            uploaded: 0,
            owners: std::collections::HashMap::new(),
        })
    }

    /// Queue boxes for upload. A newer box for the same texel replaces a queued
    /// older one, so a burst of relights of one chunk costs one copy.
    pub fn queue(&mut self, ups: Vec<Upload>) {
        for u in ups {
            if let Some(old) = self.pending.iter_mut().find(|p| p.texel == u.texel) {
                *old = u;
            } else {
                self.pending.push_back(u);
            }
        }
    }

    pub fn pending(&self) -> usize {
        self.pending.len()
    }

    /// Record this frame's copies into `cmd` (outside any render pass).
    /// `frame_slot` picks the staging slice; it must be the frame-in-flight
    /// index whose fence the caller has already waited on.
    ///
    /// # Safety
    /// `cmd` must be in the recording state and not inside a render pass.
    pub unsafe fn record(&mut self, device: &Device, cmd: vk::CommandBuffer, frame_slot: usize) -> usize {
        let Some((buf, _, ptr)) = self.staging else { return 0 };
        if self.pending.is_empty() {
            return 0;
        }
        let base = frame_slot * self.boxes_per_frame * BOX_BYTES;
        let mut regions = Vec::new();
        for k in 0..self.boxes_per_frame {
            let Some(u) = self.pending.pop_front() else { break };
            let off = base + k * BOX_BYTES;
            debug_assert_eq!(u.data.len() * 4, BOX_BYTES);
            std::ptr::copy_nonoverlapping(u.data.as_ptr() as *const u8, ptr.add(off), BOX_BYTES);
            self.owners.insert(u.texel, (u.chunk, crate::world::light_volume::box_hash(&u.data)));
            regions.push(
                vk::BufferImageCopy::builder()
                    .buffer_offset(off as u64)
                    .buffer_row_length(BOX)
                    .buffer_image_height(BOX)
                    .image_subresource(vk::ImageSubresourceLayers {
                        aspect_mask: vk::ImageAspectFlags::COLOR,
                        mip_level: 0,
                        base_array_layer: 0,
                        layer_count: 1,
                    })
                    .image_offset(vk::Offset3D { x: u.texel[0] as i32, y: u.texel[1] as i32, z: u.texel[2] as i32 })
                    .image_extent(vk::Extent3D { width: BOX, height: BOX, depth: BOX })
                    .build(),
            );
        }
        let range = vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: 0,
            layer_count: 1,
        };
        let before = vk::ImageMemoryBarrier::builder()
            .old_layout(vk::ImageLayout::GENERAL)
            .new_layout(vk::ImageLayout::GENERAL)
            .src_access_mask(vk::AccessFlags::SHADER_READ)
            .dst_access_mask(vk::AccessFlags::TRANSFER_WRITE)
            .image(self.image)
            .subresource_range(range)
            .build();
        device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::FRAGMENT_SHADER,
            vk::PipelineStageFlags::TRANSFER,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            std::slice::from_ref(&before),
        );
        device.cmd_copy_buffer_to_image(cmd, buf, self.image, vk::ImageLayout::GENERAL, &regions);
        let after = vk::ImageMemoryBarrier::builder()
            .old_layout(vk::ImageLayout::GENERAL)
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
            std::slice::from_ref(&after),
        );
        self.uploaded += regions.len() as u64;
        regions.len()
    }

    pub fn destroy(&mut self, device: &Device) {
        unsafe {
            if let Some((buf, mem, _)) = self.staging.take() {
                device.unmap_memory(mem);
                device.destroy_buffer(buf, None);
                device.free_memory(mem, None);
            }
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
    (0..props.memory_type_count)
        .find(|&i| type_filter & (1 << i) != 0 && props.memory_types[i as usize].property_flags.contains(flags))
}
