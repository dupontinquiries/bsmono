//! View-derivation compute pipeline (v0.24.0b).
//!
//! Owns the Vulkan resources for `view_compute.comp`: descriptor set
//! layout (1 binding, the ViewParams SSBO), pipeline layout (push
//! constant `view_index`), compute pipeline, and a single descriptor
//! set that points at the ViewParams buffer.
//!
//! Per frame, the renderer dispatches this pipeline once per active
//! view slot, with the slot index in the push constant. The shader
//! reads view+projection matrices CPU-written to that slot and writes
//! back the derived view_proj, inverse_view_proj, and frustum planes.
//!
//! Why a separate pipeline (not part of the cull pipeline)
//!
//! The cull pipeline reads ViewParams; this pipeline writes ViewParams.
//! Mixing read-only and read-write descriptor types in one pipeline
//! works but is messier than having two pipelines with disjoint
//! responsibilities. A barrier between this pass and the cull pass
//! makes the data-flow explicit.
//!
//! Cost: a single one-invocation compute dispatch per active view.
//! Negligible: well under 0.1ms total for the foreseeable view count.

#![cfg(feature = "render-core")]

use ash::{vk, Device};
use std::ffi::CStr;
use std::path::Path;

const ENTRY_NAME: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"main\0") };

/// Push constants for `view_compute.comp`. Matches the GLSL `pc` block.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct ViewComputePushConstants {
    pub view_index: u32,
}

const _: [(); 4] = [(); std::mem::size_of::<ViewComputePushConstants>()];

#[derive(Debug)]
pub enum ViewComputeError {
    Vulkan(vk::Result),
    ShaderMissing(String),
    Io(std::io::Error),
}
impl From<vk::Result> for ViewComputeError {
    fn from(r: vk::Result) -> Self { Self::Vulkan(r) }
}
impl From<std::io::Error> for ViewComputeError {
    fn from(e: std::io::Error) -> Self { Self::Io(e) }
}
impl std::fmt::Display for ViewComputeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Vulkan(r) => write!(f, "Vulkan error: {:?}", r),
            Self::ShaderMissing(p) => write!(f, "Shader missing: {}", p),
            Self::Io(e) => write!(f, "I/O error: {}", e),
        }
    }
}
impl std::error::Error for ViewComputeError {}

/// View-derivation compute pipeline + descriptor wiring.
pub struct ViewComputePipeline {
    pub descriptor_set_layout: vk::DescriptorSetLayout,
    pub pipeline_layout: vk::PipelineLayout,
    pub pipeline: vk::Pipeline,
    pub descriptor_pool: vk::DescriptorPool,
    pub descriptor_set: vk::DescriptorSet,

    device: Device,
}

// SAFETY: Vulkan handles are u64 IDs, safe to send across threads
// provided the device they belong to is not destroyed.
unsafe impl Send for ViewComputePipeline {}

impl ViewComputePipeline {
    /// Build the pipeline and bind it to the given ViewParams buffer.
    /// Caller owns the buffer's lifetime; this struct just records its
    /// handle in a descriptor set.
    pub unsafe fn new(
        device: &Device,
        view_params_buffer: vk::Buffer,
    ) -> Result<Self, ViewComputeError> {
        let descriptor_set_layout = create_descriptor_set_layout(device)?;
        let pipeline_layout = create_pipeline_layout(device, descriptor_set_layout)?;
        let pipeline = create_compute_pipeline(device, pipeline_layout)?;
        let (descriptor_pool, descriptor_set) =
            allocate_descriptor_set(device, descriptor_set_layout)?;
        update_descriptor_set(device, descriptor_set, view_params_buffer);

        Ok(Self {
            descriptor_set_layout,
            pipeline_layout,
            pipeline,
            descriptor_pool,
            descriptor_set,
            device: device.clone(),
        })
    }

    /// Record one view-derivation dispatch for `view_index`.
    ///
    /// Caller is responsible for:
    /// - Having written `view` and `projection` matrices for the slot
    ///   before this is recorded (CPU host-coherent write is visible
    ///   to the GPU by submit-order).
    /// - Issuing a shader-write → shader-read barrier on the
    ///   ViewParams buffer between this dispatch and the subsequent
    ///   cull dispatch that reads the derived planes.
    pub unsafe fn record_dispatch(
        &self,
        cmd: vk::CommandBuffer,
        view_index: u32,
    ) {
        self.device.cmd_bind_pipeline(
            cmd, vk::PipelineBindPoint::COMPUTE, self.pipeline,
        );
        self.device.cmd_bind_descriptor_sets(
            cmd, vk::PipelineBindPoint::COMPUTE, self.pipeline_layout,
            0, std::slice::from_ref(&self.descriptor_set), &[],
        );
        let push = ViewComputePushConstants { view_index };
        let bytes = std::slice::from_raw_parts(
            &push as *const _ as *const u8,
            std::mem::size_of::<ViewComputePushConstants>(),
        );
        self.device.cmd_push_constants(
            cmd, self.pipeline_layout,
            vk::ShaderStageFlags::COMPUTE, 0, bytes,
        );
        // Single-invocation dispatch — the shader has local_size_x = 1
        // and we issue one workgroup, so exactly one invocation runs.
        self.device.cmd_dispatch(cmd, 1, 1, 1);
    }

    pub unsafe fn destroy(&self) {
        let d = &self.device;
        d.destroy_pipeline(self.pipeline, None);
        d.destroy_pipeline_layout(self.pipeline_layout, None);
        d.destroy_descriptor_pool(self.descriptor_pool, None);
        d.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
    }
}

unsafe fn create_descriptor_set_layout(device: &Device) -> Result<vk::DescriptorSetLayout, ViewComputeError> {
    let bindings = [
        vk::DescriptorSetLayoutBinding::builder()
            .binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .build(),
    ];
    let info = vk::DescriptorSetLayoutCreateInfo::builder().bindings(&bindings);
    Ok(device.create_descriptor_set_layout(&info, None)?)
}

unsafe fn create_pipeline_layout(
    device: &Device, set_layout: vk::DescriptorSetLayout,
) -> Result<vk::PipelineLayout, ViewComputeError> {
    let pcr = vk::PushConstantRange::builder()
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
        .offset(0)
        .size(std::mem::size_of::<ViewComputePushConstants>() as u32);
    let info = vk::PipelineLayoutCreateInfo::builder()
        .set_layouts(std::slice::from_ref(&set_layout))
        .push_constant_ranges(std::slice::from_ref(&pcr));
    Ok(device.create_pipeline_layout(&info, None)?)
}

unsafe fn create_compute_pipeline(
    device: &Device, layout: vk::PipelineLayout,
) -> Result<vk::Pipeline, ViewComputeError> {
    let candidates = [
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/shaders/view_compute.comp.spv"),
        "assets/shaders/view_compute.comp.spv",
        "src/shaders/view_compute.comp.spv",
    ];
    let mut bytes: Option<Vec<u8>> = None;
    for p in &candidates {
        if Path::new(p).exists() {
            bytes = Some(std::fs::read(p)?);
            break;
        }
    }
    let bytes = bytes.ok_or_else(|| ViewComputeError::ShaderMissing(candidates[0].to_string()))?;
    let words: Vec<u32> = bytes.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();

    let module_info = vk::ShaderModuleCreateInfo::builder().code(&words);
    let module = device.create_shader_module(&module_info, None)?;

    let stage = vk::PipelineShaderStageCreateInfo::builder()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(module)
        .name(ENTRY_NAME);
    let info = vk::ComputePipelineCreateInfo::builder()
        .stage(stage.build())
        .layout(layout);
    let pipelines = device.create_compute_pipelines(
        vk::PipelineCache::null(), &[info.build()], None,
    ).map_err(|(_, r)| ViewComputeError::Vulkan(r))?;

    // Shader module no longer needed once the pipeline is built.
    device.destroy_shader_module(module, None);

    Ok(pipelines[0])
}

unsafe fn allocate_descriptor_set(
    device: &Device, layout: vk::DescriptorSetLayout,
) -> Result<(vk::DescriptorPool, vk::DescriptorSet), ViewComputeError> {
    let sizes = [vk::DescriptorPoolSize {
        ty: vk::DescriptorType::STORAGE_BUFFER, descriptor_count: 1,
    }];
    let info = vk::DescriptorPoolCreateInfo::builder()
        .pool_sizes(&sizes).max_sets(1);
    let pool = device.create_descriptor_pool(&info, None)?;
    let alloc = vk::DescriptorSetAllocateInfo::builder()
        .descriptor_pool(pool)
        .set_layouts(std::slice::from_ref(&layout));
    let sets = device.allocate_descriptor_sets(&alloc)?;
    Ok((pool, sets[0]))
}

unsafe fn update_descriptor_set(
    device: &Device, set: vk::DescriptorSet, view_params: vk::Buffer,
) {
    let v_info = vk::DescriptorBufferInfo::builder()
        .buffer(view_params).range(vk::WHOLE_SIZE).build();
    let writes = [
        vk::WriteDescriptorSet::builder()
            .dst_set(set).dst_binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(std::slice::from_ref(&v_info)).build(),
    ];
    device.update_descriptor_sets(&writes, &[]);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_constants_are_exactly_4_bytes() {
        // A single u32. Pinned so the shader's push-constant layout
        // stays in sync if anyone considers extending the struct.
        assert_eq!(std::mem::size_of::<ViewComputePushConstants>(), 4);
    }
}
