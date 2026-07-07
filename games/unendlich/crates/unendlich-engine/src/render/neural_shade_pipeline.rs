//! neural_shade_pipeline.rs — compute pipeline for the fused neural shading
//! pass (`neural_shade.comp`): a feature buffer + the 3 Q8 layers -> shaded RGB.
//!
//! Modeled byte-for-byte on `octree_probe_pipeline.rs` (same submit/fence/queue-
//! lock pattern), so this is the **verification harness** form: standalone submit
//! + host-visible IO, run it on the M2 to confirm the GPU pass == the Python net.
//! For the render loop, the only change is recording the dispatch into the frame
//! command buffer (like `view_compute_pipeline`) with device-local feat/out
//! buffers wired between the feature pass and present — see RUST_INTEGRATION.md.
//!
//! Weights (qw/qs per layer) come from `q8::quantize_layer`, byte-identical to
//! the validated Python export. Bindings match neural_shade.comp:
//!   0 feat  1 qw1 2 sc1  3 qw2 4 sc2  5 qw3 6 sc3  7 out

use ash::{Device, Instance, vk};
use std::path::Path;
use std::sync::Mutex;

const WORKGROUP_SIZE: u32 = 64;
const ENTRY_NAME: &std::ffi::CStr =
    unsafe { std::ffi::CStr::from_bytes_with_nul_unchecked(b"main\0") };

/// Max pixels per shade call in the harness (a tile; production sizes feat/out
/// to the framebuffer and keeps them device-local).
pub const MAX_SHADE_PIXELS: u32 = 65_536;
const FEAT_DIM: u32 = 104;

#[repr(C)]
#[derive(Copy, Clone)]
struct ShadePush {
    m: u32, fin0: u32, n1: u32, k1: u32, n2: u32, k2: u32, n3: u32, k3: u32,
}
const _: [(); 32] = [(); std::mem::size_of::<ShadePush>()];

#[derive(Debug)]
pub enum ShadeError { Vulkan(vk::Result), ShaderMissing(String), Io(std::io::Error), Backend(String) }
impl From<vk::Result> for ShadeError { fn from(r: vk::Result) -> Self { Self::Vulkan(r) } }
impl From<std::io::Error> for ShadeError { fn from(e: std::io::Error) -> Self { Self::Io(e) } }

struct Buf { buffer: vk::Buffer, memory: vk::DeviceMemory, ptr: *mut u8 }

pub struct NeuralShadePipeline {
    pub descriptor_set_layout: vk::DescriptorSetLayout,
    pub pipeline_layout: vk::PipelineLayout,
    pub pipeline: vk::Pipeline,
    descriptor_pool: vk::DescriptorPool,
    descriptor_set: vk::DescriptorSet,

    feat: Buf,
    out: Buf,
    weights: [Buf; 6],          // qw1,sc1,qw2,sc2,qw3,sc3 (uploaded once)
    dims: ShadePush,

    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    fence: vk::Fence,
    queue: vk::Queue,
    queue_submit_lock: Mutex<()>,
    device: Device,
}
unsafe impl Send for NeuralShadePipeline {}

impl NeuralShadePipeline {
    /// `layers`: (qw, qs, k, n) per layer from `q8::quantize_layer` (k already
    /// padded; n = output width). fin0 is the real feature count into layer 1 (104).
    pub unsafe fn new(
        device: &Device, instance: &Instance, physical_device: vk::PhysicalDevice,
        queue: vk::Queue, queue_family: u32,
        l1: (&[u32], &[f32], usize, usize),
        l2: (&[u32], &[f32], usize, usize),
        l3: (&[u32], &[f32], usize, usize),
    ) -> Result<Self, ShadeError> {
        let descriptor_set_layout = create_descriptor_set_layout(device)?;
        let pipeline_layout = create_pipeline_layout(device, descriptor_set_layout)?;
        let pipeline = create_compute_pipeline(device, pipeline_layout)?;
        let (descriptor_pool, descriptor_set) = allocate_descriptor_set(device, descriptor_set_layout)?;

        let feat = host_buf(device, instance, physical_device,
                            (MAX_SHADE_PIXELS * FEAT_DIM) as u64 * 4)?;
        let out = host_buf(device, instance, physical_device,
                           (MAX_SHADE_PIXELS * 3) as u64 * 4)?;
        let mk = |bytes: &[u8]| -> Result<Buf, ShadeError> {
            let b = host_buf(device, instance, physical_device, bytes.len() as u64)?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), b.ptr, bytes.len());
            Ok(b)
        };
        let as_u8 = |s: &[u32]| std::slice::from_raw_parts(s.as_ptr() as *const u8, s.len() * 4);
        let f_u8 = |s: &[f32]| std::slice::from_raw_parts(s.as_ptr() as *const u8, s.len() * 4);
        let weights = [mk(as_u8(l1.0))?, mk(f_u8(l1.1))?, mk(as_u8(l2.0))?,
                       mk(f_u8(l2.1))?, mk(as_u8(l3.0))?, mk(f_u8(l3.1))?];
        let dims = ShadePush { m: 0, fin0: FEAT_DIM,
                               n1: l1.3 as u32, k1: l1.2 as u32,
                               n2: l2.3 as u32, k2: l2.2 as u32,
                               n3: l3.3 as u32, k3: l3.2 as u32 };

        let bufs: [vk::Buffer; 8] = [feat.buffer, weights[0].buffer, weights[1].buffer,
            weights[2].buffer, weights[3].buffer, weights[4].buffer, weights[5].buffer, out.buffer];
        update_descriptor_set(device, descriptor_set, &bufs);

        let pool_info = vk::CommandPoolCreateInfo::builder()
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
            .queue_family_index(queue_family);
        let command_pool = device.create_command_pool(&pool_info, None)?;
        let command_buffer = device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::builder().command_pool(command_pool)
                .level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1))?[0];
        let fence = device.create_fence(&vk::FenceCreateInfo::builder(), None)?;

        Ok(Self { descriptor_set_layout, pipeline_layout, pipeline, descriptor_pool,
                  descriptor_set, feat, out, weights, dims, command_pool, command_buffer,
                  fence, queue, queue_submit_lock: Mutex::new(()), device: device.clone() })
    }

    /// Shade `m` pixels: `features` is row-major [m, 104]. Returns [m, 3] RGB.
    pub unsafe fn shade(&mut self, features: &[f32], m: u32) -> Result<Vec<f32>, ShadeError> {
        assert!(m <= MAX_SHADE_PIXELS && features.len() == (m * FEAT_DIM) as usize);
        std::ptr::copy_nonoverlapping(features.as_ptr() as *const u8, self.feat.ptr,
                                      features.len() * 4);
        self.device.reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())?;
        self.device.begin_command_buffer(self.command_buffer,
            &vk::CommandBufferBeginInfo::builder().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT))?;
        self.device.cmd_bind_pipeline(self.command_buffer, vk::PipelineBindPoint::COMPUTE, self.pipeline);
        self.device.cmd_bind_descriptor_sets(self.command_buffer, vk::PipelineBindPoint::COMPUTE,
            self.pipeline_layout, 0, std::slice::from_ref(&self.descriptor_set), &[]);
        let mut push = self.dims; push.m = m;
        self.device.cmd_push_constants(self.command_buffer, self.pipeline_layout,
            vk::ShaderStageFlags::COMPUTE, 0,
            std::slice::from_raw_parts(&push as *const _ as *const u8, std::mem::size_of::<ShadePush>()));
        self.device.cmd_dispatch(self.command_buffer, (m + WORKGROUP_SIZE - 1) / WORKGROUP_SIZE, 1, 1);
        self.device.end_command_buffer(self.command_buffer)?;
        self.device.reset_fences(&[self.fence])?;
        let submit = vk::SubmitInfo::builder()
            .command_buffers(std::slice::from_ref(&self.command_buffer)).build();
        { let _g = self.queue_submit_lock.lock().unwrap();
          self.device.queue_submit(self.queue, &[submit], self.fence)?; }
        self.device.wait_for_fences(&[self.fence], true, u64::MAX)?;
        let mut rgb = vec![0f32; (m * 3) as usize];
        std::ptr::copy_nonoverlapping(self.out.ptr as *const f32, rgb.as_mut_ptr(), rgb.len());
        Ok(rgb)
    }

    pub unsafe fn destroy(&mut self) {
        let d = &self.device;
        d.destroy_fence(self.fence, None);
        d.free_command_buffers(self.command_pool, &[self.command_buffer]);
        d.destroy_command_pool(self.command_pool, None);
        for b in std::iter::once(&self.feat).chain(std::iter::once(&self.out)).chain(self.weights.iter()) {
            d.unmap_memory(b.memory); d.destroy_buffer(b.buffer, None); d.free_memory(b.memory, None);
        }
        d.destroy_pipeline(self.pipeline, None);
        d.destroy_pipeline_layout(self.pipeline_layout, None);
        d.destroy_descriptor_pool(self.descriptor_pool, None);
        d.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
    }
}

unsafe fn create_descriptor_set_layout(device: &Device) -> Result<vk::DescriptorSetLayout, ShadeError> {
    let bindings: Vec<_> = (0..8u32).map(|i| vk::DescriptorSetLayoutBinding::builder()
        .binding(i).descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
        .descriptor_count(1).stage_flags(vk::ShaderStageFlags::COMPUTE).build()).collect();
    let info = vk::DescriptorSetLayoutCreateInfo::builder().bindings(&bindings);
    Ok(device.create_descriptor_set_layout(&info, None)?)
}

unsafe fn create_pipeline_layout(device: &Device, set: vk::DescriptorSetLayout)
    -> Result<vk::PipelineLayout, ShadeError> {
    let pcr = vk::PushConstantRange::builder().stage_flags(vk::ShaderStageFlags::COMPUTE)
        .offset(0).size(std::mem::size_of::<ShadePush>() as u32);
    let info = vk::PipelineLayoutCreateInfo::builder()
        .set_layouts(std::slice::from_ref(&set)).push_constant_ranges(std::slice::from_ref(&pcr));
    Ok(device.create_pipeline_layout(&info, None)?)
}

unsafe fn create_compute_pipeline(device: &Device, layout: vk::PipelineLayout)
    -> Result<vk::Pipeline, ShadeError> {
    let candidates = [
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/shaders/neural_shade.comp.spv"),
        "assets/shaders/neural_shade.comp.spv",
        "src/shaders/neural_shade.comp.spv",
    ];
    let bytes = candidates.iter().find(|p| Path::new(p).exists())
        .map(|p| std::fs::read(p)).transpose()?
        .ok_or_else(|| ShadeError::ShaderMissing(candidates[0].to_string()))?;
    let words: Vec<u32> = bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect();
    let module = device.create_shader_module(&vk::ShaderModuleCreateInfo::builder().code(&words), None)?;
    let stage = vk::PipelineShaderStageCreateInfo::builder()
        .stage(vk::ShaderStageFlags::COMPUTE).module(module).name(ENTRY_NAME);
    let info = vk::ComputePipelineCreateInfo::builder().stage(stage.build()).layout(layout);
    let pipelines = device.create_compute_pipelines(vk::PipelineCache::null(), &[info.build()], None)
        .map_err(|(_, r)| ShadeError::Vulkan(r))?;
    device.destroy_shader_module(module, None);
    Ok(pipelines[0])
}

unsafe fn allocate_descriptor_set(device: &Device, layout: vk::DescriptorSetLayout)
    -> Result<(vk::DescriptorPool, vk::DescriptorSet), ShadeError> {
    let sizes = [vk::DescriptorPoolSize { ty: vk::DescriptorType::STORAGE_BUFFER, descriptor_count: 8 }];
    let pool = device.create_descriptor_pool(
        &vk::DescriptorPoolCreateInfo::builder().pool_sizes(&sizes).max_sets(1), None)?;
    let sets = device.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::builder()
        .descriptor_pool(pool).set_layouts(std::slice::from_ref(&layout)))?;
    Ok((pool, sets[0]))
}

unsafe fn update_descriptor_set(device: &Device, set: vk::DescriptorSet, bufs: &[vk::Buffer; 8]) {
    let infos: Vec<_> = bufs.iter().map(|&b| vk::DescriptorBufferInfo::builder()
        .buffer(b).range(vk::WHOLE_SIZE).build()).collect();
    let writes: Vec<_> = (0..8usize).map(|i| vk::WriteDescriptorSet::builder()
        .dst_set(set).dst_binding(i as u32)
        .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
        .buffer_info(std::slice::from_ref(&infos[i])).build()).collect();
    device.update_descriptor_sets(&writes, &[]);
}

unsafe fn host_buf(device: &Device, instance: &Instance, physical_device: vk::PhysicalDevice, size: u64)
    -> Result<Buf, ShadeError> {
    let info = vk::BufferCreateInfo::builder().size(size.max(4))
        .usage(vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = device.create_buffer(&info, None)?;
    let req = device.get_buffer_memory_requirements(buffer);
    let props = instance.get_physical_device_memory_properties(physical_device);
    let mut chosen = None;
    for i in 0..props.memory_type_count {
        if req.memory_type_bits & (1 << i) != 0 && props.memory_types[i as usize].property_flags
            .contains(vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT) {
            chosen = Some(i); break;
        }
    }
    let mt = chosen.ok_or_else(|| ShadeError::Backend("no HOST_VISIBLE|COHERENT memory".into()))?;
    let memory = device.allocate_memory(&vk::MemoryAllocateInfo::builder()
        .allocation_size(req.size).memory_type_index(mt), None)?;
    device.bind_buffer_memory(buffer, memory, 0)?;
    let ptr = device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut u8;
    Ok(Buf { buffer, memory, ptr })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn push_is_32_bytes() { assert_eq!(std::mem::size_of::<ShadePush>(), 32); }
}
