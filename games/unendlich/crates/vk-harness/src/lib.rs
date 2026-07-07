//! Headless CPU-Vulkan compute harness.
//!
//! Spins up a Vulkan **compute-only** context — no window, no surface, no
//! swapchain — so it runs on a software device (Mesa's lavapipe / llvmpipe) in
//! CI or a sandbox with no GPU and no display. The point is to exercise real
//! GPU code paths (SPIR-V upload, descriptors, dispatch, buffer round-trip) in
//! `cargo test`, complementing the engine's CPU *reference* ports (which verify
//! the algorithm) by verifying the actual GPU *plumbing*.
//!
//! Run with the lavapipe ICD selected:
//! ```text
//! VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json \
//!   cargo test -p vk-harness
//! ```
//! `ComputeHarness::available()` returns false (rather than panicking) when no
//! Vulkan device can be reached, so tests can skip gracefully off-CI.

use ash::{vk, Entry, Instance, Device};

pub mod graphics;
use std::ffi::CString;

mod double_spv;
pub use double_spv::DOUBLE_SPV;

/// One std430 storage buffer bound to a compute dispatch (binding index = its
/// position in the `run_compute` slice).
pub enum Binding<'a> {
    /// Uploaded to the GPU, read-only in the shader, not read back.
    Input(&'a [u8]),
    /// Zero-initialized, read back after the dispatch.
    Output(u64),
    /// Uploaded AND read back (e.g. atomic counters seeded then updated).
    InOut(&'a [u8]),
}

/// GPU-side kernel timing from timestamp queries (lavapipe reports nanoseconds).
/// **RELATIVE proxy only**: this is a software (CPU) device, so absolute values
/// have no relationship to real-GPU performance. Use deltas of the same kernel
/// before vs after a change, on the same machine.
#[derive(Debug, Clone)]
pub struct TimingStats {
    pub iters: usize,
    pub min_ns: u64,
    pub p50_ns: u64,
    pub p90_ns: u64,
    pub p99_ns: u64,
    pub max_ns: u64,
    pub mean_ns: u64,
}

impl TimingStats {
    pub fn from_samples(mut s: Vec<u64>) -> Self {
        s.sort_unstable();
        if s.is_empty() {
            return TimingStats { iters: 0, min_ns: 0, p50_ns: 0, p90_ns: 0, p99_ns: 0, max_ns: 0, mean_ns: 0 };
        }
        let n = s.len();
        let pct = |p: f64| s[((p * (n as f64 - 1.0)).round() as usize).min(n - 1)];
        let sum: u128 = s.iter().map(|&x| x as u128).sum();
        TimingStats {
            iters: n,
            min_ns: s[0],
            p50_ns: pct(0.50),
            p90_ns: pct(0.90),
            p99_ns: pct(0.99),
            max_ns: s[n - 1],
            mean_ns: (sum / n as u128) as u64,
        }
    }

    /// Telemetry-style one-liner in microseconds (matches the engine's GPU timer
    /// summary shape so future correlation with real-hardware logs is possible).
    pub fn summary_us(&self, label: &str) -> String {
        let us = |ns: u64| ns as f64 / 1000.0;
        format!(
            "{label:<26} n={:<4} min={:.3} p50={:.3} p90={:.3} p99={:.3} max={:.3}  (us, lavapipe-relative)",
            self.iters, us(self.min_ns), us(self.p50_ns), us(self.p90_ns), us(self.p99_ns), us(self.max_ns)
        )
    }
}

/// A minimal compute-only Vulkan device.
pub struct ComputeHarness {
    _entry: Entry,
    instance: Instance,
    device: Device,
    phys: vk::PhysicalDevice,
    queue: vk::Queue,
    queue_family: u32,
}

impl ComputeHarness {
    /// True if a Vulkan compute device can be created in this environment.
    pub fn available() -> bool {
        Self::new().is_ok()
    }

    /// Create the harness, or return a human-readable error string.
    pub fn new() -> Result<Self, String> {
        unsafe {
            let entry = Entry::load().map_err(|e| format!("no Vulkan loader: {e:?}"))?;
            let app = CString::new("vk-harness").unwrap();
            let app_info = vk::ApplicationInfo::builder()
                .application_name(&app)
                .api_version(vk::make_api_version(0, 1, 1, 0));
            let ci = vk::InstanceCreateInfo::builder().application_info(&app_info);
            let instance = entry
                .create_instance(&ci, None)
                .map_err(|e| format!("create_instance: {e:?}"))?;

            let phys_devices = instance
                .enumerate_physical_devices()
                .map_err(|e| format!("enumerate_physical_devices: {e:?}"))?;
            if phys_devices.is_empty() {
                instance.destroy_instance(None);
                return Err("no Vulkan physical devices".into());
            }

            // Pick the first device exposing a compute-capable queue family.
            let mut chosen: Option<(vk::PhysicalDevice, u32)> = None;
            for pd in phys_devices {
                let props = instance.get_physical_device_queue_family_properties(pd);
                if let Some((qf, _)) = props.iter().enumerate().find(|(_, q)| {
                    q.queue_flags.contains(vk::QueueFlags::COMPUTE) && q.queue_count > 0
                }) {
                    chosen = Some((pd, qf as u32));
                    break;
                }
            }
            let (phys, queue_family) = match chosen {
                Some(c) => c,
                None => {
                    instance.destroy_instance(None);
                    return Err("no compute queue family".into());
                }
            };

            let priorities = [1.0f32];
            let qci = vk::DeviceQueueCreateInfo::builder()
                .queue_family_index(queue_family)
                .queue_priorities(&priorities);
            let qcis = [qci.build()];
            let dci = vk::DeviceCreateInfo::builder().queue_create_infos(&qcis);
            let device = instance
                .create_device(phys, &dci, None)
                .map_err(|e| format!("create_device: {e:?}"))?;
            let queue = device.get_device_queue(queue_family, 0);

            Ok(Self { _entry: entry, instance, device, phys, queue, queue_family })
        }
    }

    /// Human-readable name of the chosen device (e.g. "llvmpipe (...)").
    pub fn device_name(&self) -> String {
        unsafe {
            let p = self.instance.get_physical_device_properties(self.phys);
            let bytes: Vec<u8> = p.device_name.iter().take_while(|&&c| c != 0).map(|&c| c as u8).collect();
            String::from_utf8_lossy(&bytes).into_owned()
        }
    }

    /// True if the chosen device is a CPU (software) implementation.
    pub fn is_cpu_device(&self) -> bool {
        unsafe {
            self.instance.get_physical_device_properties(self.phys).device_type
                == vk::PhysicalDeviceType::CPU
        }
    }

    /// Find a host-visible+coherent memory type satisfying `req`.
    unsafe fn host_memory_type(&self, req: &vk::MemoryRequirements) -> Result<u32, String> {
        let mem = self.instance.get_physical_device_memory_properties(self.phys);
        let want = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
        for i in 0..mem.memory_type_count {
            let supported = req.memory_type_bits & (1 << i) != 0;
            let props = mem.memory_types[i as usize].property_flags;
            if supported && props.contains(want) {
                return Ok(i);
            }
        }
        Err("no host-visible coherent memory type".into())
    }

    /// Run a compute shader with N std430 storage buffers bound at set 0,
    /// bindings 0..N in slice order, optional push constants (bytes; length a
    /// multiple of 4), dispatched `groups_x` x `groups_y` workgroups. Returns
    /// per-binding readback: `Some(bytes)` for `Output`/`InOut`, `None` for
    /// `Input`.
    pub fn run_compute(
        &self,
        spirv: &[u32],
        bindings: &[Binding<'_>],
        push: &[u8],
        groups_x: u32,
        groups_y: u32,
    ) -> Result<Vec<Option<Vec<u8>>>, String> {
        unsafe { self.run_compute_inner(spirv, bindings, push, groups_x, groups_y) }
    }

    unsafe fn run_compute_inner(
        &self,
        spirv: &[u32],
        bindings: &[Binding<'_>],
        push: &[u8],
        groups_x: u32,
        groups_y: u32,
    ) -> Result<Vec<Option<Vec<u8>>>, String> {
        let dev = &self.device;
        let n = bindings.len();
        if n == 0 {
            return Err("no bindings".into());
        }

        // --- create + fill buffers ---
        let mut buffers = Vec::with_capacity(n);
        let mut memories = Vec::with_capacity(n);
        let mut sizes = Vec::with_capacity(n);
        let mut readback = Vec::with_capacity(n);
        for b in bindings {
            let (init, size, rb): (Option<&[u8]>, u64, bool) = match b {
                Binding::Input(d) => (Some(*d), d.len() as u64, false),
                Binding::InOut(d) => (Some(*d), d.len() as u64, true),
                Binding::Output(s) => (None, *s, true),
            };
            let size = size.max(4); // never zero-size
            let buf_ci = vk::BufferCreateInfo::builder()
                .size(size)
                .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
                .sharing_mode(vk::SharingMode::EXCLUSIVE);
            let buffer = dev.create_buffer(&buf_ci, None).map_err(|e| format!("buffer: {e:?}"))?;
            let req = dev.get_buffer_memory_requirements(buffer);
            let mem_type = self.host_memory_type(&req)?;
            let alloc = vk::MemoryAllocateInfo::builder()
                .allocation_size(req.size)
                .memory_type_index(mem_type);
            let memory = dev.allocate_memory(&alloc, None).map_err(|e| format!("alloc: {e:?}"))?;
            dev.bind_buffer_memory(buffer, memory, 0).map_err(|e| format!("bind: {e:?}"))?;

            let ptr = dev.map_memory(memory, 0, size, vk::MemoryMapFlags::empty())
                .map_err(|e| format!("map: {e:?}"))? as *mut u8;
            std::ptr::write_bytes(ptr, 0, size as usize);
            if let Some(d) = init {
                std::ptr::copy_nonoverlapping(d.as_ptr(), ptr, d.len());
            }
            dev.unmap_memory(memory);

            buffers.push(buffer);
            memories.push(memory);
            sizes.push(size);
            readback.push(rb);
        }

        // --- descriptor layout / pool / set ---
        let dsl_bindings: Vec<vk::DescriptorSetLayoutBinding> = (0..n).map(|i| {
            vk::DescriptorSetLayoutBinding::builder()
                .binding(i as u32)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE)
                .build()
        }).collect();
        let dsl_ci = vk::DescriptorSetLayoutCreateInfo::builder().bindings(&dsl_bindings);
        let dsl = dev.create_descriptor_set_layout(&dsl_ci, None).map_err(|e| format!("dsl: {e:?}"))?;

        let pool_size = vk::DescriptorPoolSize::builder()
            .ty(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(n as u32)
            .build();
        let pool_sizes = [pool_size];
        let pool_ci = vk::DescriptorPoolCreateInfo::builder().max_sets(1).pool_sizes(&pool_sizes);
        let pool = dev.create_descriptor_pool(&pool_ci, None).map_err(|e| format!("pool: {e:?}"))?;
        let set_layouts = [dsl];
        let ds_ai = vk::DescriptorSetAllocateInfo::builder()
            .descriptor_pool(pool).set_layouts(&set_layouts);
        let desc_set = dev.allocate_descriptor_sets(&ds_ai).map_err(|e| format!("ds: {e:?}"))?[0];

        let buf_infos: Vec<[vk::DescriptorBufferInfo; 1]> = (0..n).map(|i| {
            [vk::DescriptorBufferInfo::builder()
                .buffer(buffers[i]).offset(0).range(sizes[i]).build()]
        }).collect();
        let writes: Vec<vk::WriteDescriptorSet> = (0..n).map(|i| {
            vk::WriteDescriptorSet::builder()
                .dst_set(desc_set).dst_binding(i as u32)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .buffer_info(&buf_infos[i]).build()
        }).collect();
        dev.update_descriptor_sets(&writes, &[]);

        // --- pipeline ---
        let mut pl_builder = vk::PipelineLayoutCreateInfo::builder().set_layouts(&set_layouts);
        let push_ranges = [vk::PushConstantRange::builder()
            .stage_flags(vk::ShaderStageFlags::COMPUTE).offset(0)
            .size(push.len() as u32).build()];
        if !push.is_empty() {
            pl_builder = pl_builder.push_constant_ranges(&push_ranges);
        }
        let pipeline_layout = dev.create_pipeline_layout(&pl_builder, None).map_err(|e| format!("pl: {e:?}"))?;

        let sm_ci = vk::ShaderModuleCreateInfo::builder().code(spirv);
        let shader = dev.create_shader_module(&sm_ci, None).map_err(|e| format!("shader: {e:?}"))?;
        let entry = CString::new("main").unwrap();
        let stage = vk::PipelineShaderStageCreateInfo::builder()
            .stage(vk::ShaderStageFlags::COMPUTE).module(shader).name(&entry).build();
        let cp_ci = vk::ComputePipelineCreateInfo::builder().stage(stage).layout(pipeline_layout).build();
        let pipeline = dev
            .create_compute_pipelines(vk::PipelineCache::null(), &[cp_ci], None)
            .map_err(|(_, e)| format!("pipeline: {e:?}"))?[0];

        // --- command buffer ---
        let cp_ci = vk::CommandPoolCreateInfo::builder().queue_family_index(self.queue_family);
        let cmd_pool = dev.create_command_pool(&cp_ci, None).map_err(|e| format!("cmdpool: {e:?}"))?;
        let cb_ai = vk::CommandBufferAllocateInfo::builder()
            .command_pool(cmd_pool).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1);
        let cmd = dev.allocate_command_buffers(&cb_ai).map_err(|e| format!("cmd: {e:?}"))?[0];

        let begin = vk::CommandBufferBeginInfo::builder()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        dev.begin_command_buffer(cmd, &begin).map_err(|e| format!("begin: {e:?}"))?;
        dev.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pipeline);
        dev.cmd_bind_descriptor_sets(cmd, vk::PipelineBindPoint::COMPUTE, pipeline_layout, 0, &[desc_set], &[]);
        if !push.is_empty() {
            dev.cmd_push_constants(cmd, pipeline_layout, vk::ShaderStageFlags::COMPUTE, 0, push);
        }
        dev.cmd_dispatch(cmd, groups_x.max(1), groups_y.max(1), 1);
        dev.end_command_buffer(cmd).map_err(|e| format!("end: {e:?}"))?;

        let cmds = [cmd];
        let submit = vk::SubmitInfo::builder().command_buffers(&cmds).build();
        dev.queue_submit(self.queue, &[submit], vk::Fence::null()).map_err(|e| format!("submit: {e:?}"))?;
        dev.queue_wait_idle(self.queue).map_err(|e| format!("wait: {e:?}"))?;

        // --- readback ---
        let mut results: Vec<Option<Vec<u8>>> = Vec::with_capacity(n);
        for i in 0..n {
            if readback[i] {
                let ptr = dev.map_memory(memories[i], 0, sizes[i], vk::MemoryMapFlags::empty())
                    .map_err(|e| format!("map2: {e:?}"))? as *const u8;
                let mut out = vec![0u8; sizes[i] as usize];
                std::ptr::copy_nonoverlapping(ptr, out.as_mut_ptr(), sizes[i] as usize);
                dev.unmap_memory(memories[i]);
                results.push(Some(out));
            } else {
                results.push(None);
            }
        }

        // --- teardown ---
        dev.destroy_command_pool(cmd_pool, None);
        dev.destroy_pipeline(pipeline, None);
        dev.destroy_shader_module(shader, None);
        dev.destroy_pipeline_layout(pipeline_layout, None);
        dev.destroy_descriptor_pool(pool, None);
        dev.destroy_descriptor_set_layout(dsl, None);
        for i in 0..n {
            dev.destroy_buffer(buffers[i], None);
            dev.free_memory(memories[i], None);
        }
        Ok(results)
    }

    /// Benchmark a compute dispatch: set resources up once, then re-submit the
    /// same command buffer `iters` times, measuring GPU-side kernel time with
    /// timestamp queries (lavapipe reports ns; timestampPeriod=1). Returns
    /// timing stats. NOTE: this is a SOFTWARE (lavapipe/CPU) device — absolute
    /// numbers mean nothing for real hardware; only RELATIVE deltas (same kernel
    /// before vs after a change, same machine) are a meaningful proxy.
    pub fn bench_compute(
        &self,
        spirv: &[u32],
        bindings: &[Binding<'_>],
        push: &[u8],
        groups_x: u32,
        groups_y: u32,
        iters: u32,
    ) -> Result<TimingStats, String> {
        unsafe { self.bench_inner(spirv, bindings, push, groups_x, groups_y, iters) }
    }

    unsafe fn bench_inner(
        &self,
        spirv: &[u32],
        bindings: &[Binding<'_>],
        push: &[u8],
        groups_x: u32,
        groups_y: u32,
        iters: u32,
    ) -> Result<TimingStats, String> {
        let dev = &self.device;
        let n = bindings.len();
        if n == 0 { return Err("no bindings".into()); }

        // Buffers (same as run_compute, but readback is irrelevant here).
        let mut buffers = Vec::with_capacity(n);
        let mut memories = Vec::with_capacity(n);
        let mut sizes = Vec::with_capacity(n);
        for b in bindings {
            let (init, size): (Option<&[u8]>, u64) = match b {
                Binding::Input(d) | Binding::InOut(d) => (Some(*d), d.len() as u64),
                Binding::Output(s) => (None, *s),
            };
            let size = size.max(4);
            let buf_ci = vk::BufferCreateInfo::builder().size(size)
                .usage(vk::BufferUsageFlags::STORAGE_BUFFER).sharing_mode(vk::SharingMode::EXCLUSIVE);
            let buffer = dev.create_buffer(&buf_ci, None).map_err(|e| format!("buffer: {e:?}"))?;
            let req = dev.get_buffer_memory_requirements(buffer);
            let mem_type = self.host_memory_type(&req)?;
            let alloc = vk::MemoryAllocateInfo::builder().allocation_size(req.size).memory_type_index(mem_type);
            let memory = dev.allocate_memory(&alloc, None).map_err(|e| format!("alloc: {e:?}"))?;
            dev.bind_buffer_memory(buffer, memory, 0).map_err(|e| format!("bind: {e:?}"))?;
            let ptr = dev.map_memory(memory, 0, size, vk::MemoryMapFlags::empty())
                .map_err(|e| format!("map: {e:?}"))? as *mut u8;
            std::ptr::write_bytes(ptr, 0, size as usize);
            if let Some(d) = init { std::ptr::copy_nonoverlapping(d.as_ptr(), ptr, d.len()); }
            dev.unmap_memory(memory);
            buffers.push(buffer); memories.push(memory); sizes.push(size);
        }

        let dsl_bindings: Vec<vk::DescriptorSetLayoutBinding> = (0..n).map(|i| {
            vk::DescriptorSetLayoutBinding::builder().binding(i as u32)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER).descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE).build()
        }).collect();
        let dsl_ci = vk::DescriptorSetLayoutCreateInfo::builder().bindings(&dsl_bindings);
        let dsl = dev.create_descriptor_set_layout(&dsl_ci, None).map_err(|e| format!("dsl: {e:?}"))?;
        let pool_size = vk::DescriptorPoolSize::builder().ty(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(n as u32).build();
        let pool_sizes = [pool_size];
        let pool_ci = vk::DescriptorPoolCreateInfo::builder().max_sets(1).pool_sizes(&pool_sizes);
        let pool = dev.create_descriptor_pool(&pool_ci, None).map_err(|e| format!("pool: {e:?}"))?;
        let set_layouts = [dsl];
        let ds_ai = vk::DescriptorSetAllocateInfo::builder().descriptor_pool(pool).set_layouts(&set_layouts);
        let desc_set = dev.allocate_descriptor_sets(&ds_ai).map_err(|e| format!("ds: {e:?}"))?[0];
        let buf_infos: Vec<[vk::DescriptorBufferInfo; 1]> = (0..n).map(|i| {
            [vk::DescriptorBufferInfo::builder().buffer(buffers[i]).offset(0).range(sizes[i]).build()]
        }).collect();
        let writes: Vec<vk::WriteDescriptorSet> = (0..n).map(|i| {
            vk::WriteDescriptorSet::builder().dst_set(desc_set).dst_binding(i as u32)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER).buffer_info(&buf_infos[i]).build()
        }).collect();
        dev.update_descriptor_sets(&writes, &[]);

        let mut pl_builder = vk::PipelineLayoutCreateInfo::builder().set_layouts(&set_layouts);
        let push_ranges = [vk::PushConstantRange::builder()
            .stage_flags(vk::ShaderStageFlags::COMPUTE).offset(0).size(push.len() as u32).build()];
        if !push.is_empty() { pl_builder = pl_builder.push_constant_ranges(&push_ranges); }
        let pipeline_layout = dev.create_pipeline_layout(&pl_builder, None).map_err(|e| format!("pl: {e:?}"))?;
        let sm_ci = vk::ShaderModuleCreateInfo::builder().code(spirv);
        let shader = dev.create_shader_module(&sm_ci, None).map_err(|e| format!("shader: {e:?}"))?;
        let entry = CString::new("main").unwrap();
        let stage = vk::PipelineShaderStageCreateInfo::builder()
            .stage(vk::ShaderStageFlags::COMPUTE).module(shader).name(&entry).build();
        let cp_ci = vk::ComputePipelineCreateInfo::builder().stage(stage).layout(pipeline_layout).build();
        let pipeline = dev.create_compute_pipelines(vk::PipelineCache::null(), &[cp_ci], None)
            .map_err(|(_, e)| format!("pipeline: {e:?}"))?[0];

        // Timestamp query pool (2 stamps: before/after dispatch).
        let qp_ci = vk::QueryPoolCreateInfo::builder()
            .query_type(vk::QueryType::TIMESTAMP).query_count(2);
        let qpool = dev.create_query_pool(&qp_ci, None).map_err(|e| format!("qpool: {e:?}"))?;

        let cp_ci2 = vk::CommandPoolCreateInfo::builder().queue_family_index(self.queue_family);
        let cmd_pool = dev.create_command_pool(&cp_ci2, None).map_err(|e| format!("cmdpool: {e:?}"))?;
        let cb_ai = vk::CommandBufferAllocateInfo::builder()
            .command_pool(cmd_pool).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1);
        let cmd = dev.allocate_command_buffers(&cb_ai).map_err(|e| format!("cmd: {e:?}"))?[0];

        // Record once (resubmittable: no ONE_TIME_SUBMIT). Resets the query pool
        // and brackets the dispatch with timestamps each submission.
        let begin = vk::CommandBufferBeginInfo::builder();
        dev.begin_command_buffer(cmd, &begin).map_err(|e| format!("begin: {e:?}"))?;
        dev.cmd_reset_query_pool(cmd, qpool, 0, 2);
        dev.cmd_write_timestamp(cmd, vk::PipelineStageFlags::TOP_OF_PIPE, qpool, 0);
        dev.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pipeline);
        dev.cmd_bind_descriptor_sets(cmd, vk::PipelineBindPoint::COMPUTE, pipeline_layout, 0, &[desc_set], &[]);
        if !push.is_empty() {
            dev.cmd_push_constants(cmd, pipeline_layout, vk::ShaderStageFlags::COMPUTE, 0, push);
        }
        dev.cmd_dispatch(cmd, groups_x.max(1), groups_y.max(1), 1);
        dev.cmd_write_timestamp(cmd, vk::PipelineStageFlags::BOTTOM_OF_PIPE, qpool, 1);
        dev.end_command_buffer(cmd).map_err(|e| format!("end: {e:?}"))?;

        let period = self.instance.get_physical_device_properties(self.phys).limits.timestamp_period;
        let cmds = [cmd];
        let submit = vk::SubmitInfo::builder().command_buffers(&cmds).build();
        let mut samples_ns: Vec<u64> = Vec::with_capacity(iters as usize);
        // One warmup submit (first dispatch pays one-time costs).
        dev.queue_submit(self.queue, &[submit], vk::Fence::null()).map_err(|e| format!("submit: {e:?}"))?;
        dev.queue_wait_idle(self.queue).map_err(|e| format!("wait: {e:?}"))?;
        for _ in 0..iters {
            dev.queue_submit(self.queue, &[submit], vk::Fence::null()).map_err(|e| format!("submit: {e:?}"))?;
            dev.queue_wait_idle(self.queue).map_err(|e| format!("wait: {e:?}"))?;
            let mut ts = [0u64; 2];
            dev.get_query_pool_results(qpool, 0, 2, &mut ts,
                vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WAIT)
                .map_err(|e| format!("query: {e:?}"))?;
            let delta = ts[1].saturating_sub(ts[0]);
            samples_ns.push((delta as f64 * period as f64) as u64);
        }

        dev.destroy_query_pool(qpool, None);
        dev.destroy_command_pool(cmd_pool, None);
        dev.destroy_pipeline(pipeline, None);
        dev.destroy_shader_module(shader, None);
        dev.destroy_pipeline_layout(pipeline_layout, None);
        dev.destroy_descriptor_pool(pool, None);
        dev.destroy_descriptor_set_layout(dsl, None);
        for i in 0..n {
            dev.destroy_buffer(buffers[i], None);
            dev.free_memory(memories[i], None);
        }
        Ok(TimingStats::from_samples(samples_ns))
    }

    /// Convenience: a single-buffer u32 kernel (InOut at binding 0, push
    /// constant `uint n`). Returns the buffer contents after the dispatch.
    pub fn run_u32_kernel(&self, spirv: &[u32], data: &[u32]) -> Result<Vec<u32>, String> {
        if data.is_empty() {
            return Ok(Vec::new());
        }
        let n = data.len() as u32;
        let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_ne_bytes()).collect();
        let groups = (n + 63) / 64;
        let out = self.run_compute(spirv, &[Binding::InOut(&bytes)], &n.to_ne_bytes(), groups, 1)?;
        let raw = out[0].as_ref().ok_or("no readback")?;
        Ok(raw.chunks_exact(4).take(data.len())
            .map(|c| u32::from_ne_bytes([c[0], c[1], c[2], c[3]]))
            .collect())
    }
}

impl Drop for ComputeHarness {
    fn drop(&mut self) {
        unsafe {
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
    }
}

