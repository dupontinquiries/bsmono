//! neural_gemm_pipeline.rs — the lean MLP as the tiled `linear_q8_0` GEMM (3
//! layers) + `silu` (2), the PERF ship-path alternative to the fused
//! `neural_shade.comp` (which stays the scalar correctness reference). This is the
//! path `neural_shading_integration.md` §4 recommends shipping: on the M2 the GEMM
//! collapses toward its FLOP ceiling while the fused per-pixel scalar dequant does
//! not.
//!
//! **Harness form**, mirroring the validated `mlp.py::GpuMLP` byte-for-byte: the
//! bias is folded by augmenting each layer's input with a constant `1.0` at index
//! `d` and zero-padding to a multiple of 32 (`q8::quantize_layer`'s convention), and
//! the augment is done on the CPU between dispatches with host-visible IO and a
//! fence wait per dispatch. That makes it directly parity-checkable against the
//! Python net and the fused shader. The production in-frame form keeps the
//! activations device-local, fuses the augment into a small kernel, and records the
//! dispatch chain into the frame command buffer (like `view_compute_pipeline`) — see
//! the integration doc. Same kernels, same weights; only the IO/scheduling differs.
//!
//! Weights per layer are `(qw, qs, k, n)` from `q8::quantize_layer` (k already
//! padded; n = output width). The chain for the lean net is 14→64→48→3:
//!   feat[m,14] ─aug→ linear(L0) → [m,64] ─silu→ ─aug→ linear(L1) → [m,48] ─silu→
//!   ─aug→ linear(L2) → [m,3].

use ash::{Device, Instance, vk};
use std::path::Path;
use std::sync::Mutex;

const LINEAR_WG: u32 = 8; // linear_q8_0.comp: local_size 8x8
const SILU_WG: u32 = 64; // silu.comp: local_size 64
const ENTRY_NAME: &std::ffi::CStr =
    unsafe { std::ffi::CStr::from_bytes_with_nul_unchecked(b"main\0") };

/// Max pixels per `forward` call in the harness (a tile). Production sizes the
/// device-local activation buffers to the framebuffer instead.
pub const MAX_GEMM_PIXELS: u32 = 65_536;

#[repr(C)]
#[derive(Copy, Clone)]
struct LinearPush {
    m: u32,
    n: u32,
    k: u32,
    kb: u32,
}
const _: [(); 16] = [(); std::mem::size_of::<LinearPush>()];

#[repr(C)]
#[derive(Copy, Clone)]
struct SiluPush {
    n: u32,
}

#[derive(Debug)]
pub enum GemmError {
    Vulkan(vk::Result),
    ShaderMissing(String),
    Io(std::io::Error),
    Backend(String),
}
impl From<vk::Result> for GemmError {
    fn from(r: vk::Result) -> Self {
        Self::Vulkan(r)
    }
}
impl From<std::io::Error> for GemmError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

struct Buf {
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    ptr: *mut u8,
}

/// One quantized layer: weights (uploaded once), output width `n`, padded input
/// width `k`.
struct Layer {
    qw: Buf,
    qs: Buf,
    n: usize,
    k: usize,
}

pub struct NeuralGemmPipeline {
    lin_dsl: vk::DescriptorSetLayout,
    lin_layout: vk::PipelineLayout,
    lin_pipeline: vk::Pipeline,
    lin_pool: vk::DescriptorPool,
    lin_set: vk::DescriptorSet,

    sil_dsl: vk::DescriptorSetLayout,
    sil_layout: vk::PipelineLayout,
    sil_pipeline: vk::Pipeline,
    sil_pool: vk::DescriptorPool,
    sil_set: vk::DescriptorSet,

    buf_in: Buf,  // augmented activation / silu input (host-visible)
    buf_out: Buf, // dispatch output (host-visible)

    layers: Vec<Layer>,
    fin0: usize, // real feature count into layer 0

    command_pool: vk::CommandPool,
    command_buffer: vk::CommandBuffer,
    fence: vk::Fence,
    queue: vk::Queue,
    queue_submit_lock: Mutex<()>,
    device: Device,
}
unsafe impl Send for NeuralGemmPipeline {}

impl NeuralGemmPipeline {
    /// `fin0`: real feature count into layer 0 (e.g. 14 for the lean net).
    /// `layers`: `(qw, qs, k, n)` per layer from `q8::quantize_layer`, in order.
    pub unsafe fn new(
        device: &Device,
        instance: &Instance,
        physical_device: vk::PhysicalDevice,
        queue: vk::Queue,
        queue_family: u32,
        fin0: usize,
        layers: &[(&[u32], &[f32], usize, usize)],
    ) -> Result<Self, GemmError> {
        assert!(!layers.is_empty(), "need at least one layer");

        let lin_dsl = create_descriptor_set_layout(device, 4)?;
        let lin_layout =
            create_pipeline_layout(device, lin_dsl, std::mem::size_of::<LinearPush>() as u32)?;
        let lin_pipeline = create_compute_pipeline(device, lin_layout, "linear_q8_0")?;
        let (lin_pool, lin_set) = allocate_descriptor_set(device, lin_dsl, 4)?;

        let sil_dsl = create_descriptor_set_layout(device, 2)?;
        let sil_layout =
            create_pipeline_layout(device, sil_dsl, std::mem::size_of::<SiluPush>() as u32)?;
        let sil_pipeline = create_compute_pipeline(device, sil_layout, "silu")?;
        let (sil_pool, sil_set) = allocate_descriptor_set(device, sil_dsl, 2)?;

        // Upload each layer's weights once; sanity-check the padding convention.
        let as_u8 = |s: &[u32]| std::slice::from_raw_parts(s.as_ptr() as *const u8, s.len() * 4);
        let f_u8 = |s: &[f32]| std::slice::from_raw_parts(s.as_ptr() as *const u8, s.len() * 4);
        let mk = |bytes: &[u8]| -> Result<Buf, GemmError> {
            let b = host_buf(device, instance, physical_device, bytes.len() as u64)?;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), b.ptr, bytes.len());
            Ok(b)
        };
        let mut built = Vec::with_capacity(layers.len());
        let mut max_width = fin0;
        for (i, &(qw, qs, k, n)) in layers.iter().enumerate() {
            let d = if i == 0 { fin0 } else { layers[i - 1].3 };
            // q8::quantize_layer pads k = ceil((d+1)/32)*32; assert the caller's
            // weights line up with the activation width we'll augment to.
            debug_assert_eq!(k, ((d + 1 + 31) / 32) * 32, "layer {i} k vs augmented d");
            debug_assert_eq!(qw.len(), n * k / 4, "layer {i} qw size");
            debug_assert_eq!(qs.len(), n * (k / 32), "layer {i} qs size");
            built.push(Layer { qw: mk(as_u8(qw))?, qs: mk(f_u8(qs))?, n, k });
            max_width = max_width.max(k).max(n);
        }

        let bytes = MAX_GEMM_PIXELS as u64 * max_width as u64 * 4;
        let buf_in = host_buf(device, instance, physical_device, bytes)?;
        let buf_out = host_buf(device, instance, physical_device, bytes)?;

        let pool_info = vk::CommandPoolCreateInfo::builder()
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
            .queue_family_index(queue_family);
        let command_pool = device.create_command_pool(&pool_info, None)?;
        let command_buffer = device.allocate_command_buffers(
            &vk::CommandBufferAllocateInfo::builder()
                .command_pool(command_pool)
                .level(vk::CommandBufferLevel::PRIMARY)
                .command_buffer_count(1),
        )?[0];
        let fence = device.create_fence(&vk::FenceCreateInfo::builder(), None)?;

        Ok(Self {
            lin_dsl,
            lin_layout,
            lin_pipeline,
            lin_pool,
            lin_set,
            sil_dsl,
            sil_layout,
            sil_pipeline,
            sil_pool,
            sil_set,
            buf_in,
            buf_out,
            layers: built,
            fin0,
            command_pool,
            command_buffer,
            fence,
            queue,
            queue_submit_lock: Mutex::new(()),
            device: device.clone(),
        })
    }

    /// Run the MLP over `m` pixels. `feat` is row-major `[m, fin0]`. Returns the
    /// final layer output `[m, layers.last().n]` (e.g. `[m, 3]` shaded RGB).
    pub unsafe fn forward(&mut self, feat: &[f32], m: u32) -> Result<Vec<f32>, GemmError> {
        assert!(m <= MAX_GEMM_PIXELS);
        assert_eq!(feat.len(), (m as usize) * self.fin0, "feat size != m*fin0");
        let n_layers = self.layers.len();
        let mut a: Vec<f32> = feat.to_vec();
        let mut d = self.fin0;
        for li in 0..n_layers {
            a = self.linear(&a, m, d, li)?;
            d = self.layers[li].n;
            if li + 1 < n_layers {
                a = self.silu(&a)?;
            }
        }
        Ok(a)
    }

    /// One linear layer: CPU-augment `a` (`[m, d]`) → `Xa` (`[m, k]`, with
    /// `Xa[:,d]=1` carrying the bias, zeros to the 32-pad), dispatch `linear_q8_0`,
    /// read back `[m, n]`.
    unsafe fn linear(&mut self, a: &[f32], m: u32, d: usize, li: usize) -> Result<Vec<f32>, GemmError> {
        let (n, k) = (self.layers[li].n, self.layers[li].k);
        let m_us = m as usize;
        // Build the augmented, zero-padded activation [m, k].
        let mut xa = vec![0f32; m_us * k];
        for r in 0..m_us {
            xa[r * k..r * k + d].copy_from_slice(&a[r * d..r * d + d]);
            xa[r * k + d] = 1.0; // bias multiplier
            // [d+1 .. k] left zero
        }
        std::ptr::copy_nonoverlapping(xa.as_ptr() as *const u8, self.buf_in.ptr, xa.len() * 4);

        let bufs = [
            self.buf_in.buffer,
            self.layers[li].qw.buffer,
            self.layers[li].qs.buffer,
            self.buf_out.buffer,
        ];
        update_descriptor_set(&self.device, self.lin_set, &bufs);

        let push = LinearPush { m, n: n as u32, k: k as u32, kb: (k / 32) as u32 };
        let groups_x = (n as u32 + LINEAR_WG - 1) / LINEAR_WG;
        let groups_y = (m + LINEAR_WG - 1) / LINEAR_WG;
        self.dispatch(
            self.lin_pipeline,
            self.lin_layout,
            self.lin_set,
            &push as *const _ as *const u8,
            std::mem::size_of::<LinearPush>(),
            groups_x,
            groups_y,
        )?;

        let mut out = vec![0f32; m_us * n];
        std::ptr::copy_nonoverlapping(self.buf_out.ptr as *const f32, out.as_mut_ptr(), out.len());
        Ok(out)
    }

    /// Element-wise SiLU over `a`.
    unsafe fn silu(&mut self, a: &[f32]) -> Result<Vec<f32>, GemmError> {
        let len = a.len();
        std::ptr::copy_nonoverlapping(a.as_ptr() as *const u8, self.buf_in.ptr, len * 4);
        let bufs = [self.buf_in.buffer, self.buf_out.buffer];
        update_descriptor_set(&self.device, self.sil_set, &bufs);
        let push = SiluPush { n: len as u32 };
        let groups = (len as u32 + SILU_WG - 1) / SILU_WG;
        self.dispatch(
            self.sil_pipeline,
            self.sil_layout,
            self.sil_set,
            &push as *const _ as *const u8,
            std::mem::size_of::<SiluPush>(),
            groups,
            1,
        )?;
        let mut out = vec![0f32; len];
        std::ptr::copy_nonoverlapping(self.buf_out.ptr as *const f32, out.as_mut_ptr(), out.len());
        Ok(out)
    }

    /// Record one compute dispatch, submit, and wait (harness pattern — the
    /// descriptor set must already be updated and the previous submit complete).
    unsafe fn dispatch(
        &self,
        pipeline: vk::Pipeline,
        layout: vk::PipelineLayout,
        set: vk::DescriptorSet,
        push: *const u8,
        push_size: usize,
        groups_x: u32,
        groups_y: u32,
    ) -> Result<(), GemmError> {
        self.device
            .reset_command_buffer(self.command_buffer, vk::CommandBufferResetFlags::empty())?;
        self.device.begin_command_buffer(
            self.command_buffer,
            &vk::CommandBufferBeginInfo::builder().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
        )?;
        self.device
            .cmd_bind_pipeline(self.command_buffer, vk::PipelineBindPoint::COMPUTE, pipeline);
        self.device.cmd_bind_descriptor_sets(
            self.command_buffer,
            vk::PipelineBindPoint::COMPUTE,
            layout,
            0,
            std::slice::from_ref(&set),
            &[],
        );
        self.device.cmd_push_constants(
            self.command_buffer,
            layout,
            vk::ShaderStageFlags::COMPUTE,
            0,
            std::slice::from_raw_parts(push, push_size),
        );
        self.device
            .cmd_dispatch(self.command_buffer, groups_x, groups_y, 1);
        self.device.end_command_buffer(self.command_buffer)?;
        self.device.reset_fences(&[self.fence])?;
        let submit = vk::SubmitInfo::builder()
            .command_buffers(std::slice::from_ref(&self.command_buffer))
            .build();
        {
            let _g = self.queue_submit_lock.lock().unwrap();
            self.device.queue_submit(self.queue, &[submit], self.fence)?;
        }
        self.device.wait_for_fences(&[self.fence], true, u64::MAX)?;
        Ok(())
    }

    pub unsafe fn destroy(&mut self) {
        let d = &self.device;
        d.destroy_fence(self.fence, None);
        d.free_command_buffers(self.command_pool, &[self.command_buffer]);
        d.destroy_command_pool(self.command_pool, None);
        let mut bufs: Vec<&Buf> = vec![&self.buf_in, &self.buf_out];
        for l in &self.layers {
            bufs.push(&l.qw);
            bufs.push(&l.qs);
        }
        for b in bufs {
            d.unmap_memory(b.memory);
            d.destroy_buffer(b.buffer, None);
            d.free_memory(b.memory, None);
        }
        d.destroy_pipeline(self.lin_pipeline, None);
        d.destroy_pipeline(self.sil_pipeline, None);
        d.destroy_pipeline_layout(self.lin_layout, None);
        d.destroy_pipeline_layout(self.sil_layout, None);
        d.destroy_descriptor_pool(self.lin_pool, None);
        d.destroy_descriptor_pool(self.sil_pool, None);
        d.destroy_descriptor_set_layout(self.lin_dsl, None);
        d.destroy_descriptor_set_layout(self.sil_dsl, None);
    }
}

unsafe fn create_descriptor_set_layout(
    device: &Device,
    n: u32,
) -> Result<vk::DescriptorSetLayout, GemmError> {
    let bindings: Vec<_> = (0..n)
        .map(|i| {
            vk::DescriptorSetLayoutBinding::builder()
                .binding(i)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE)
                .build()
        })
        .collect();
    let info = vk::DescriptorSetLayoutCreateInfo::builder().bindings(&bindings);
    Ok(device.create_descriptor_set_layout(&info, None)?)
}

unsafe fn create_pipeline_layout(
    device: &Device,
    set: vk::DescriptorSetLayout,
    push_size: u32,
) -> Result<vk::PipelineLayout, GemmError> {
    let pcr = vk::PushConstantRange::builder()
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
        .offset(0)
        .size(push_size);
    let info = vk::PipelineLayoutCreateInfo::builder()
        .set_layouts(std::slice::from_ref(&set))
        .push_constant_ranges(std::slice::from_ref(&pcr));
    Ok(device.create_pipeline_layout(&info, None)?)
}

unsafe fn create_compute_pipeline(
    device: &Device,
    layout: vk::PipelineLayout,
    stem: &str,
) -> Result<vk::Pipeline, GemmError> {
    let candidates = [
        format!(
            "{}/../../assets/shaders/{stem}.comp.spv",
            env!("CARGO_MANIFEST_DIR")
        ),
        format!("assets/shaders/{stem}.comp.spv"),
        format!("src/shaders/{stem}.comp.spv"),
    ];
    let bytes = candidates
        .iter()
        .find(|p| Path::new(p).exists())
        .map(std::fs::read)
        .transpose()?
        .ok_or_else(|| GemmError::ShaderMissing(candidates[0].clone()))?;
    let words: Vec<u32> = bytes
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let module =
        device.create_shader_module(&vk::ShaderModuleCreateInfo::builder().code(&words), None)?;
    let stage = vk::PipelineShaderStageCreateInfo::builder()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(module)
        .name(ENTRY_NAME);
    let info = vk::ComputePipelineCreateInfo::builder()
        .stage(stage.build())
        .layout(layout);
    let pipelines = device
        .create_compute_pipelines(vk::PipelineCache::null(), &[info.build()], None)
        .map_err(|(_, r)| GemmError::Vulkan(r))?;
    device.destroy_shader_module(module, None);
    Ok(pipelines[0])
}

unsafe fn allocate_descriptor_set(
    device: &Device,
    layout: vk::DescriptorSetLayout,
    n: u32,
) -> Result<(vk::DescriptorPool, vk::DescriptorSet), GemmError> {
    let sizes = [vk::DescriptorPoolSize {
        ty: vk::DescriptorType::STORAGE_BUFFER,
        descriptor_count: n,
    }];
    let pool = device.create_descriptor_pool(
        &vk::DescriptorPoolCreateInfo::builder()
            .pool_sizes(&sizes)
            .max_sets(1),
        None,
    )?;
    let sets = device.allocate_descriptor_sets(
        &vk::DescriptorSetAllocateInfo::builder()
            .descriptor_pool(pool)
            .set_layouts(std::slice::from_ref(&layout)),
    )?;
    Ok((pool, sets[0]))
}

unsafe fn update_descriptor_set(device: &Device, set: vk::DescriptorSet, bufs: &[vk::Buffer]) {
    let infos: Vec<_> = bufs
        .iter()
        .map(|&b| {
            vk::DescriptorBufferInfo::builder()
                .buffer(b)
                .range(vk::WHOLE_SIZE)
                .build()
        })
        .collect();
    let writes: Vec<_> = (0..bufs.len())
        .map(|i| {
            vk::WriteDescriptorSet::builder()
                .dst_set(set)
                .dst_binding(i as u32)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(std::slice::from_ref(&infos[i]))
                .build()
        })
        .collect();
    device.update_descriptor_sets(&writes, &[]);
}

unsafe fn host_buf(
    device: &Device,
    instance: &Instance,
    physical_device: vk::PhysicalDevice,
    size: u64,
) -> Result<Buf, GemmError> {
    let info = vk::BufferCreateInfo::builder()
        .size(size.max(4))
        .usage(vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_DST)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = device.create_buffer(&info, None)?;
    let req = device.get_buffer_memory_requirements(buffer);
    let props = instance.get_physical_device_memory_properties(physical_device);
    let mut chosen = None;
    for i in 0..props.memory_type_count {
        if req.memory_type_bits & (1 << i) != 0
            && props.memory_types[i as usize]
                .property_flags
                .contains(vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT)
        {
            chosen = Some(i);
            break;
        }
    }
    let mt = chosen.ok_or_else(|| GemmError::Backend("no HOST_VISIBLE|COHERENT memory".into()))?;
    let memory = device.allocate_memory(
        &vk::MemoryAllocateInfo::builder()
            .allocation_size(req.size)
            .memory_type_index(mt),
        None,
    )?;
    device.bind_buffer_memory(buffer, memory, 0)?;
    let ptr = device.map_memory(memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())? as *mut u8;
    Ok(Buf { buffer, memory, ptr })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn linear_push_is_16_bytes() {
        assert_eq!(std::mem::size_of::<LinearPush>(), 16);
    }
}
