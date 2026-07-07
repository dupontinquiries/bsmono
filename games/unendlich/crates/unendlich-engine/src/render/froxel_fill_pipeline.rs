//! Compute pipeline that fills the volumetric froxel scattering
//! volume each frame.
//!
//! The compute shader (`src/shaders/froxel_fill.comp`) runs once per
//! frame, BEFORE the main color render pass and AFTER the shadow
//! pre-pass — so the per-froxel shadow query reads from the current
//! frame's shadow map. Output is written into the
//! `FroxelVolume`'s 3D image at binding 2 (declared as a writeonly
//! image3D, format rgba16f).
//!
//! Descriptor set layout — distinct from the chunk pipeline's, because
//! compute reads/writes different bindings:
//!   binding 0: UBO (CameraUbo) — same buffer the graphics pipeline
//!              uses, just bound to a separate descriptor set.
//!   binding 1: combined image sampler (shadow map) — same as the
//!              chunk pipeline's binding 1.
//!   binding 2: storage image (the froxel volume's image, in GENERAL
//!              layout) — write target.
//!
//! Push constants:
//!   - inverse view-projection matrix (so the shader can unproject
//!     froxel indices into world positions).
//!   - camera near/far.
//!   - froxel extent (160, 90, 64).
//!   - froxel near/far depth slice bounds in meters (0.5, 200).
//! All packed into ~92 bytes; well under the 128-byte minimum
//! Vulkan guarantees.

use ash::{vk, Device};
use std::ffi::CStr;

#[cfg(not(feature = "hot-shaders"))]
const COMP_SPV: &[u8] = include_bytes!("../../../../assets/shaders/froxel_fill.comp.spv");

#[cfg(feature = "hot-shaders")]
fn load_comp_spv() -> Vec<u8> {
    crate::render::hot_shaders::read_spv("froxel_fill.comp.spv")
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct FroxelFillPushConstants {
    pub inv_view_proj:        [[f32; 4]; 4], // 64 B
    pub cam_near_far:         [f32; 4],      // 16 B (x=near, y=far)
    pub froxel_extent_packed: [f32; 4],      // 16 B (xyz=extent)
    pub froxel_near_far:      [f32; 4],      // 16 B (x=near, y=far)
}

pub struct FroxelFillPipeline {
    pub pipeline: vk::Pipeline,
    pub layout: vk::PipelineLayout,
    pub descriptor_set_layout: vk::DescriptorSetLayout,
}

impl FroxelFillPipeline {
    pub fn new(device: &Device) -> Result<Self, vk::Result> {
        #[cfg(not(feature = "hot-shaders"))]
        let comp_module = create_shader_module(device, COMP_SPV)?;
        #[cfg(feature = "hot-shaders")]
        let comp_module = {
            let v = load_comp_spv();
            create_shader_module(device, &v)?
        };
        let entry = unsafe { CStr::from_bytes_with_nul_unchecked(b"main\0") };

        // ─── Descriptor set layout ──────────────────────────────────
        let dsl_bindings = [
            vk::DescriptorSetLayoutBinding::builder()
                .binding(0)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE)
                .build(),
            vk::DescriptorSetLayoutBinding::builder()
                .binding(1)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE)
                .build(),
            vk::DescriptorSetLayoutBinding::builder()
                .binding(2)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::COMPUTE)
                .build(),
        ];
        let dsl_info = vk::DescriptorSetLayoutCreateInfo::builder()
            .bindings(&dsl_bindings);
        let descriptor_set_layout = unsafe { device.create_descriptor_set_layout(&dsl_info, None)? };

        // ─── Pipeline layout (with push constants) ──────────────────
        let push_range = vk::PushConstantRange::builder()
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .offset(0)
            .size(std::mem::size_of::<FroxelFillPushConstants>() as u32)
            .build();
        let set_layouts = [descriptor_set_layout];
        let push_ranges = [push_range];
        let layout_info = vk::PipelineLayoutCreateInfo::builder()
            .set_layouts(&set_layouts)
            .push_constant_ranges(&push_ranges);
        let layout = unsafe { device.create_pipeline_layout(&layout_info, None)? };

        // ─── Compute pipeline ───────────────────────────────────────
        let stage = vk::PipelineShaderStageCreateInfo::builder()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(comp_module)
            .name(entry)
            .build();
        let info = vk::ComputePipelineCreateInfo::builder()
            .stage(stage)
            .layout(layout);

        let pipelines = unsafe {
            device.create_compute_pipelines(
                vk::PipelineCache::null(),
                std::slice::from_ref(&info),
                None,
            )
        }.map_err(|(_, e)| e)?;

        unsafe {
            device.destroy_shader_module(comp_module, None);
        }

        Ok(Self {
            pipeline: pipelines[0],
            layout,
            descriptor_set_layout,
        })
    }

    pub fn destroy(&mut self, device: &Device) {
        unsafe {
            device.destroy_pipeline(self.pipeline, None);
            device.destroy_pipeline_layout(self.layout, None);
            device.destroy_descriptor_set_layout(self.descriptor_set_layout, None);
        }
    }
}

fn create_shader_module(device: &Device, bytes: &[u8]) -> Result<vk::ShaderModule, vk::Result> {
    assert_eq!(bytes.len() % 4, 0);
    let words: Vec<u32> = bytes.chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let info = vk::ShaderModuleCreateInfo::builder().code(&words);
    unsafe { device.create_shader_module(&info, None) }
}
