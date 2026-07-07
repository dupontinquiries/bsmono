//! neural_feat_pipeline.rs — the neural feature pass, render-loop (in-frame) form.
//!
//! Samples the image G-buffer (`gbuffer.rs`) into a device-local feature buffer
//! `[width*height, 14]` via `neural_feat_gbuf.comp`, recording the dispatch into the
//! frame command buffer. Gated alongside the G-buffer by `UNENDLICH_NEURAL_GBUFFER`.
//!
//! r310: the feature pass now marches the octree for the 6 hint rays. Two extra
//! bindings — the global octree node buffer (the pool's `buffer`, binding 4) and a
//! host-visible `base_grid` (binding 5) the renderer rebuilds each frame mapping
//! chunk coord → root index (EMPTY = 0xFFFFFFFF) over a `GRID_DIMS` neighborhood.
//!
//! Validate the feature buffer via RenderDoc (`TRANSFER_SRC`): base-8 features +
//! hint distances in [0,1] (1.0 = open, smaller = nearer occluder), varying with
//! geometry. Encoding must match the trainer (B2).

use ash::{vk, Device, Instance};
use std::ffi::CStr;

const WG: u32 = 8; // neural_feat_gbuf.comp: local_size 8x8
pub const FEAT_STRIDE: u32 = 14;

/// Base-grid extent in chunks (the hint-ray march neighborhood). Buffer is the
/// product * 4 bytes (~144 KB at [48,16,48]); centered on the player each frame.
pub const GRID_DIMS: [u32; 3] = [48, 16, 48];

#[cfg(not(feature = "hot-shaders"))]
const FEAT_SPV: &[u8] = include_bytes!("../../../../assets/shaders/neural_feat_gbuf.comp.spv");

#[repr(C)]
#[derive(Copy, Clone)]
struct FeatPush {
    sun_dir: [f32; 4],
    width: u32,
    height: u32,
    y_scale: f32,
    _pad0: u32,
    grid_origin: [i32; 4], // xyz = min chunk coord covered by base_grid
    grid_dims: [u32; 4],   // xyz = extent in chunks
}
const _: [(); 64] = [(); std::mem::size_of::<FeatPush>()];

struct Buf {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
}

pub struct NeuralFeatPipeline {
    descriptor_set_layout: vk::DescriptorSetLayout,
    pipeline_layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,
    feat: Buf,
    // host-visible base grid (CPU writes each frame, GPU reads in the march)
    grid_buffer: vk::Buffer,
    grid_memory: vk::DeviceMemory,
    grid_ptr: *mut u8,
    grid_len: usize, // u32 count
    extent: vk::Extent2D,
}
// SAFETY: grid_ptr is a raw pointer into device memory we own; writes go through
// &self methods on the render thread.
unsafe impl Send for NeuralFeatPipeline {}

impl NeuralFeatPipeline {
    pub fn new(
        instance: &Instance,
        device: &Device,
        physical_device: vk::PhysicalDevice,
        extent: vk::Extent2D,
    ) -> Result<Self, vk::Result> {
        let descriptor_set_layout = create_descriptor_set_layout(device)?;
        let pipeline_layout = create_pipeline_layout(device, descriptor_set_layout)?;
        let pipeline = create_pipeline(device, pipeline_layout)?;
        let (descriptor_pool, descriptor_set) = allocate_descriptor_set(device, descriptor_set_layout)?;

        let feat_bytes = extent.width as u64 * extent.height as u64 * FEAT_STRIDE as u64 * 4;
        let feat = device_buf(instance, device, physical_device, feat_bytes)?;

        let grid_len = (GRID_DIMS[0] * GRID_DIMS[1] * GRID_DIMS[2]) as usize;
        let (grid_buffer, grid_memory, grid_ptr) =
            host_buf(instance, device, physical_device, grid_len as u64 * 4)?;

        Ok(Self {
            descriptor_set_layout,
            pipeline_layout,
            pipeline,
            descriptor_pool,
            descriptor_set,
            feat,
            grid_buffer,
            grid_memory,
            grid_ptr,
            grid_len,
            extent,
        })
    }

    pub fn grid_len(&self) -> usize {
        self.grid_len
    }

    /// Copy the freshly-built base grid (len == `grid_len()`, EMPTY = u32::MAX for
    /// absent chunks) into the host-visible buffer the march reads.
    pub fn upload_base_grid(&self, grid: &[u32]) {
        let n = grid.len().min(self.grid_len);
        unsafe {
            std::ptr::copy_nonoverlapping(grid.as_ptr() as *const u8, self.grid_ptr, n * 4);
        }
    }

    /// Record the feature dispatch into `cmd`, sampling `gbuffer`'s images and
    /// marching `octree_nodes` via the base grid (origin = min chunk coord). The
    /// descriptor set is rewritten each frame (single frame in flight).
    pub unsafe fn record(
        &self,
        device: &Device,
        cmd: vk::CommandBuffer,
        gbuffer: &crate::render::gbuffer::NeuralGBuffer,
        octree_nodes: vk::Buffer,
        sun_dir: [f32; 3],
        y_scale: f32,
        grid_origin: [i32; 3],
    ) {
        let img = |view: vk::ImageView, sampler: vk::Sampler| {
            vk::DescriptorImageInfo::builder()
                .image_view(view)
                .sampler(sampler)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)
                .build()
        };
        let infos = [
            img(gbuffer.normal.view, gbuffer.normal.sampler),
            img(gbuffer.albedo.view, gbuffer.albedo.sampler),
            img(gbuffer.worldpos.view, gbuffer.worldpos.sampler),
        ];
        let buf = |b: vk::Buffer| {
            vk::DescriptorBufferInfo::builder().buffer(b).range(vk::WHOLE_SIZE).build()
        };
        let feat_info = buf(self.feat.buffer);
        let oct_info = buf(octree_nodes);
        let grid_info = buf(self.grid_buffer);
        let sampler_write = |bind: u32, info: &vk::DescriptorImageInfo| {
            vk::WriteDescriptorSet::builder()
                .dst_set(self.descriptor_set)
                .dst_binding(bind)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .image_info(std::slice::from_ref(info))
                .build()
        };
        let buffer_write = |bind: u32, info: &vk::DescriptorBufferInfo| {
            vk::WriteDescriptorSet::builder()
                .dst_set(self.descriptor_set)
                .dst_binding(bind)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(info))
                .build()
        };
        let writes = [
            sampler_write(0, &infos[0]),
            sampler_write(1, &infos[1]),
            sampler_write(2, &infos[2]),
            buffer_write(3, &feat_info),
            buffer_write(4, &oct_info),
            buffer_write(5, &grid_info),
        ];
        device.update_descriptor_sets(&writes, &[]);

        device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, self.pipeline);
        device.cmd_bind_descriptor_sets(
            cmd,
            vk::PipelineBindPoint::COMPUTE,
            self.pipeline_layout,
            0,
            std::slice::from_ref(&self.descriptor_set),
            &[],
        );
        let push = FeatPush {
            sun_dir: [sun_dir[0], sun_dir[1], sun_dir[2], 0.0],
            width: self.extent.width,
            height: self.extent.height,
            y_scale,
            _pad0: 0,
            grid_origin: [grid_origin[0], grid_origin[1], grid_origin[2], 0],
            grid_dims: [GRID_DIMS[0], GRID_DIMS[1], GRID_DIMS[2], 0],
        };
        device.cmd_push_constants(
            cmd,
            self.pipeline_layout,
            vk::ShaderStageFlags::COMPUTE,
            0,
            std::slice::from_raw_parts(&push as *const _ as *const u8, std::mem::size_of::<FeatPush>()),
        );
        let gx = (self.extent.width + WG - 1) / WG;
        let gy = (self.extent.height + WG - 1) / WG;
        device.cmd_dispatch(cmd, gx, gy, 1);
    }

    pub fn destroy(&mut self, device: &Device) {
        unsafe {
            device.unmap_memory(self.grid_memory);
            device.destroy_buffer(self.grid_buffer, None);
            device.free_memory(self.grid_memory, None);
            device.destroy_buffer(self.feat.buffer, None);
            device.free_memory(self.feat.memory, None);
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.pipeline_layout, None);
            device.destroy_descriptor_pool(self.descriptor_pool, None);
            device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
        }
    }
}

fn create_descriptor_set_layout(device: &Device) -> Result<vk::DescriptorSetLayout, vk::Result> {
    let bindings = [
        sampler_binding(0),
        sampler_binding(1),
        sampler_binding(2),
        storage_binding(3),
        storage_binding(4), // octree nodes
        storage_binding(5), // base grid
    ];
    let info = vk::DescriptorSetLayoutCreateInfo::builder().bindings(&bindings);
    unsafe { device.create_descriptor_set_layout(&info, None) }
}

fn sampler_binding(b: u32) -> vk::DescriptorSetLayoutBinding {
    vk::DescriptorSetLayoutBinding::builder()
        .binding(b)
        .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
        .build()
}

fn storage_binding(b: u32) -> vk::DescriptorSetLayoutBinding {
    vk::DescriptorSetLayoutBinding::builder()
        .binding(b)
        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
        .build()
}

fn create_pipeline_layout(
    device: &Device,
    set: vk::DescriptorSetLayout,
) -> Result<vk::PipelineLayout, vk::Result> {
    let pcr = vk::PushConstantRange::builder()
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
        .offset(0)
        .size(std::mem::size_of::<FeatPush>() as u32);
    let info = vk::PipelineLayoutCreateInfo::builder()
        .set_layouts(std::slice::from_ref(&set))
        .push_constant_ranges(std::slice::from_ref(&pcr));
    unsafe { device.create_pipeline_layout(&info, None) }
}

fn create_pipeline(device: &Device, layout: vk::PipelineLayout) -> Result<vk::Pipeline, vk::Result> {
    #[cfg(not(feature = "hot-shaders"))]
    let bytes: Vec<u8> = FEAT_SPV.to_vec();
    #[cfg(feature = "hot-shaders")]
    let bytes: Vec<u8> = crate::render::hot_shaders::read_spv("neural_feat_gbuf.comp.spv");
    assert_eq!(bytes.len() % 4, 0);
    let words: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let module =
        unsafe { device.create_shader_module(&vk::ShaderModuleCreateInfo::builder().code(&words), None)? };
    let entry = unsafe { CStr::from_bytes_with_nul_unchecked(b"main\0") };
    let stage = vk::PipelineShaderStageCreateInfo::builder()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(module)
        .name(entry);
    let info = vk::ComputePipelineCreateInfo::builder().stage(stage.build()).layout(layout);
    let pipelines = unsafe {
        device.create_compute_pipelines(vk::PipelineCache::null(), &[info.build()], None)
    }
    .map_err(|(_, e)| e)?;
    unsafe { device.destroy_shader_module(module, None) };
    Ok(pipelines[0])
}

fn allocate_descriptor_set(
    device: &Device,
    layout: vk::DescriptorSetLayout,
) -> Result<(vk::DescriptorPool, vk::DescriptorSet), vk::Result> {
    let sizes = [
        vk::DescriptorPoolSize { ty: vk::DescriptorType::COMBINED_IMAGE_SAMPLER, descriptor_count: 3 },
        vk::DescriptorPoolSize { ty: vk::DescriptorType::STORAGE_BUFFER, descriptor_count: 3 },
    ];
    let pool = unsafe {
        device.create_descriptor_pool(
            &vk::DescriptorPoolCreateInfo::builder().pool_sizes(&sizes).max_sets(1),
            None,
        )?
    };
    let sets = unsafe {
        device.allocate_descriptor_sets(
            &vk::DescriptorSetAllocateInfo::builder()
                .descriptor_pool(pool)
                .set_layouts(std::slice::from_ref(&layout)),
        )?
    };
    Ok((pool, sets[0]))
}

fn device_buf(
    instance: &Instance,
    device: &Device,
    physical_device: vk::PhysicalDevice,
    size: u64,
) -> Result<Buf, vk::Result> {
    let info = vk::BufferCreateInfo::builder()
        .size(size.max(4))
        .usage(vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = unsafe { device.create_buffer(&info, None)? };
    let req = unsafe { device.get_buffer_memory_requirements(buffer) };
    let memory = alloc_memory(instance, device, physical_device, &req, vk::MemoryPropertyFlags::DEVICE_LOCAL)?;
    unsafe { device.bind_buffer_memory(buffer, memory, 0)? };
    Ok(Buf { buffer, memory })
}

fn host_buf(
    instance: &Instance,
    device: &Device,
    physical_device: vk::PhysicalDevice,
    size: u64,
) -> Result<(vk::Buffer, vk::DeviceMemory, *mut u8), vk::Result> {
    let info = vk::BufferCreateInfo::builder()
        .size(size.max(4))
        .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = unsafe { device.create_buffer(&info, None)? };
    let req = unsafe { device.get_buffer_memory_requirements(buffer) };
    let memory = alloc_memory(
        instance, device, physical_device, &req,
        vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
    )?;
    unsafe { device.bind_buffer_memory(buffer, memory, 0)? };
    let ptr = unsafe { device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? } as *mut u8;
    Ok((buffer, memory, ptr))
}

fn alloc_memory(
    instance: &Instance,
    device: &Device,
    physical_device: vk::PhysicalDevice,
    req: &vk::MemoryRequirements,
    flags: vk::MemoryPropertyFlags,
) -> Result<vk::DeviceMemory, vk::Result> {
    let props = unsafe { instance.get_physical_device_memory_properties(physical_device) };
    let mut chosen = None;
    for i in 0..props.memory_type_count {
        if req.memory_type_bits & (1 << i) != 0
            && props.memory_types[i as usize].property_flags.contains(flags)
        {
            chosen = Some(i);
            break;
        }
    }
    let mt = chosen.unwrap_or(0);
    unsafe {
        device.allocate_memory(
            &vk::MemoryAllocateInfo::builder().allocation_size(req.size).memory_type_index(mt),
            None,
        )
    }
}
