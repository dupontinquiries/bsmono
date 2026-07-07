#![cfg(feature = "render-core")]
//! field_diffuse_gpu — GPU offload of one explicit 2D field-diffusion step.
//!
//! Runs `field_diffuse.comp` (one thread per cell, gather stencil) and is the
//! GPU counterpart of [`crate::sim::field_diffuse_ref::diffuse_step`]. Per R22,
//! the kernel's math is *defined* by that CPU reference and proven bit-for-bit
//! against it on lavapipe by the parity test at the bottom of this file.
//!
//! This is the one-shot, self-contained form (allocate → upload → dispatch →
//! read back → tear down) used to validate the kernel. The hot-path engine
//! integration (persistent pipeline + ping-ponged device-local buffers, the
//! `GPU_COMPUTE_PLAN.md` §2 offload of the weather/moisture field off
//! `tick_sim`) reuses this same `.comp`; only the resource lifetime differs.

use ash::{vk, Device, Instance};
use std::ffi::CStr;

use crate::render::context::VulkanContext;

const ENTRY: &CStr = unsafe { CStr::from_bytes_with_nul_unchecked(b"main\0") };

#[repr(C)]
#[derive(Clone, Copy)]
struct Push {
    w: u32,
    h: u32,
    coeff: f32,
    dt: f32,
}
const _: [(); 16] = [(); std::mem::size_of::<Push>()];

unsafe fn create_host_buffer(
    instance: &Instance,
    device: &Device,
    physical_device: vk::PhysicalDevice,
    size: u64,
) -> Result<(vk::Buffer, vk::DeviceMemory, *mut u8), vk::Result> {
    let info = vk::BufferCreateInfo::builder()
        .size(size)
        .usage(vk::BufferUsageFlags::STORAGE_BUFFER)
        .sharing_mode(vk::SharingMode::EXCLUSIVE);
    let buffer = device.create_buffer(&info, None)?;
    let req = device.get_buffer_memory_requirements(buffer);
    let props = instance.get_physical_device_memory_properties(physical_device);
    let want = vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT;
    let mut chosen = None;
    for i in 0..props.memory_type_count {
        if req.memory_type_bits & (1 << i) != 0
            && props.memory_types[i as usize].property_flags.contains(want)
        {
            chosen = Some(i);
            break;
        }
    }
    let mt = chosen.ok_or(vk::Result::ERROR_FEATURE_NOT_PRESENT)?;
    let alloc = vk::MemoryAllocateInfo::builder()
        .allocation_size(req.size)
        .memory_type_index(mt);
    let memory = device.allocate_memory(&alloc, None)?;
    device.bind_buffer_memory(buffer, memory, 0)?;
    let ptr = device.map_memory(memory, 0, size, vk::MemoryMapFlags::empty())? as *mut u8;
    Ok((buffer, memory, ptr))
}

/// Run one diffusion step on the GPU and return the next grid.
///
/// Mirrors the reference's no-op guard: an empty grid, or `src` not exactly
/// `width·height` long, returns a copy of `src` unchanged. `spv` is the
/// compiled `field_diffuse.comp`. Never panics.
///
/// # Safety
/// `ctx` must be a live context whose device/compute queue are valid.
pub unsafe fn diffuse_step_gpu(
    ctx: &VulkanContext,
    spv: &[u8],
    src: &[f32],
    width: usize,
    height: usize,
    coeff: f32,
    dt: f32,
) -> Result<Vec<f32>, vk::Result> {
    let n = width * height;
    if n == 0 || src.len() != n {
        return Ok(src.to_vec());
    }
    let device = &ctx.device;
    let size = (n * std::mem::size_of::<f32>()) as u64;

    // Buffers: src (uploaded), dst (read back). Both host-visible/coherent.
    let (src_buf, src_mem, src_ptr) =
        create_host_buffer(&ctx.instance, device, ctx.physical_device, size)?;
    let (dst_buf, dst_mem, dst_ptr) =
        create_host_buffer(&ctx.instance, device, ctx.physical_device, size)?;
    std::ptr::copy_nonoverlapping(src.as_ptr() as *const u8, src_ptr, size as usize);

    // Descriptor set layout: binding 0 = src SSBO, binding 1 = dst SSBO.
    let dsl_bindings = [
        vk::DescriptorSetLayoutBinding::builder()
            .binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .build(),
        vk::DescriptorSetLayoutBinding::builder()
            .binding(1)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::COMPUTE)
            .build(),
    ];
    let set_layout = device.create_descriptor_set_layout(
        &vk::DescriptorSetLayoutCreateInfo::builder().bindings(&dsl_bindings),
        None,
    )?;

    let pcr = vk::PushConstantRange::builder()
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
        .offset(0)
        .size(std::mem::size_of::<Push>() as u32);
    let pipeline_layout = device.create_pipeline_layout(
        &vk::PipelineLayoutCreateInfo::builder()
            .set_layouts(std::slice::from_ref(&set_layout))
            .push_constant_ranges(std::slice::from_ref(&pcr)),
        None,
    )?;

    let words: Vec<u32> = spv
        .chunks_exact(4)
        .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
        .collect();
    let module = device
        .create_shader_module(&vk::ShaderModuleCreateInfo::builder().code(&words), None)?;
    let stage = vk::PipelineShaderStageCreateInfo::builder()
        .stage(vk::ShaderStageFlags::COMPUTE)
        .module(module)
        .name(ENTRY);
    let pipeline = device
        .create_compute_pipelines(
            vk::PipelineCache::null(),
            &[vk::ComputePipelineCreateInfo::builder()
                .stage(stage.build())
                .layout(pipeline_layout)
                .build()],
            None,
        )
        .map_err(|(_, e)| e)?[0];

    // Descriptor pool + set, pointed at the two buffers.
    let pool_sizes = [vk::DescriptorPoolSize::builder()
        .ty(vk::DescriptorType::STORAGE_BUFFER)
        .descriptor_count(2)
        .build()];
    let desc_pool = device.create_descriptor_pool(
        &vk::DescriptorPoolCreateInfo::builder().pool_sizes(&pool_sizes).max_sets(1),
        None,
    )?;
    let desc_set = device.allocate_descriptor_sets(
        &vk::DescriptorSetAllocateInfo::builder()
            .descriptor_pool(desc_pool)
            .set_layouts(std::slice::from_ref(&set_layout)),
    )?[0];
    let src_info = [vk::DescriptorBufferInfo::builder().buffer(src_buf).offset(0).range(size).build()];
    let dst_info = [vk::DescriptorBufferInfo::builder().buffer(dst_buf).offset(0).range(size).build()];
    let writes = [
        vk::WriteDescriptorSet::builder()
            .dst_set(desc_set).dst_binding(0)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&src_info).build(),
        vk::WriteDescriptorSet::builder()
            .dst_set(desc_set).dst_binding(1)
            .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
            .buffer_info(&dst_info).build(),
    ];
    device.update_descriptor_sets(&writes, &[]);

    // Record one dispatch covering the grid (8×8 local size, rounded up).
    let cmd_pool = device.create_command_pool(
        &vk::CommandPoolCreateInfo::builder()
            .queue_family_index(ctx.compute_queue_family)
            .flags(vk::CommandPoolCreateFlags::TRANSIENT),
        None,
    )?;
    let cmd = device.allocate_command_buffers(
        &vk::CommandBufferAllocateInfo::builder()
            .command_pool(cmd_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1),
    )?[0];
    device.begin_command_buffer(
        cmd,
        &vk::CommandBufferBeginInfo::builder()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
    )?;
    device.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::COMPUTE, pipeline);
    device.cmd_bind_descriptor_sets(
        cmd, vk::PipelineBindPoint::COMPUTE, pipeline_layout,
        0, std::slice::from_ref(&desc_set), &[],
    );
    let push = Push { w: width as u32, h: height as u32, coeff, dt };
    let push_bytes =
        std::slice::from_raw_parts(&push as *const _ as *const u8, std::mem::size_of::<Push>());
    device.cmd_push_constants(cmd, pipeline_layout, vk::ShaderStageFlags::COMPUTE, 0, push_bytes);
    let gx = ((width as u32) + 7) / 8;
    let gy = ((height as u32) + 7) / 8;
    device.cmd_dispatch(cmd, gx, gy, 1);
    // Make the shader's writes visible to the host read-back.
    let barrier = vk::MemoryBarrier::builder()
        .src_access_mask(vk::AccessFlags::SHADER_WRITE)
        .dst_access_mask(vk::AccessFlags::HOST_READ)
        .build();
    device.cmd_pipeline_barrier(
        cmd,
        vk::PipelineStageFlags::COMPUTE_SHADER,
        vk::PipelineStageFlags::HOST,
        vk::DependencyFlags::empty(),
        &[barrier], &[], &[],
    );
    device.end_command_buffer(cmd)?;

    let fence = device.create_fence(&vk::FenceCreateInfo::builder(), None)?;
    {
        let _lock = ctx.queue_submit_lock.lock().unwrap_or_else(|e| e.into_inner());
        let submit = vk::SubmitInfo::builder().command_buffers(std::slice::from_ref(&cmd)).build();
        device.queue_submit(ctx.compute_queue, &[submit], fence)?;
    }
    device.wait_for_fences(&[fence], true, u64::MAX)?;

    // Read back.
    let mut out = vec![0.0f32; n];
    std::ptr::copy_nonoverlapping(dst_ptr as *const u8, out.as_mut_ptr() as *mut u8, size as usize);

    // Teardown (reverse order).
    device.destroy_fence(fence, None);
    device.destroy_command_pool(cmd_pool, None);
    device.destroy_descriptor_pool(desc_pool, None);
    device.destroy_pipeline(pipeline, None);
    device.destroy_shader_module(module, None);
    device.destroy_pipeline_layout(pipeline_layout, None);
    device.destroy_descriptor_set_layout(set_layout, None);
    device.unmap_memory(src_mem);
    device.destroy_buffer(src_buf, None);
    device.free_memory(src_mem, None);
    device.unmap_memory(dst_mem);
    device.destroy_buffer(dst_buf, None);
    device.free_memory(dst_mem, None);

    Ok(out)
}

#[cfg(all(test, feature = "headless"))]
mod parity {
    use super::*;
    use crate::sim::field_diffuse_ref::diffuse_step;

    fn load_spv() -> Vec<u8> {
        // Prefer a build.rs-produced asset; fall back to compiling the source.
        for p in [
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/shaders/field_diffuse.comp.spv"),
            "assets/shaders/field_diffuse.comp.spv",
            "src/shaders/field_diffuse.comp.spv",
        ] {
            if std::path::Path::new(p).exists() {
                return std::fs::read(p).unwrap();
            }
        }
        let src = ["src/shaders/field_diffuse.comp",
                   "crates/unendlich-engine/src/shaders/field_diffuse.comp"]
            .into_iter()
            .find(|p| std::path::Path::new(p).exists())
            .expect("field_diffuse.comp source on disk");
        let out = std::env::temp_dir().join("field_diffuse_parity.spv");
        let ok = std::process::Command::new("glslangValidator")
            .args(["-V", src, "-o"])
            .arg(&out)
            .status()
            .expect("glslangValidator on PATH")
            .success();
        assert!(ok, "field_diffuse.comp must compile");
        std::fs::read(out).unwrap()
    }

    fn grid(seed0: u32, n: usize) -> Vec<f32> {
        let mut s = seed0;
        (0..n)
            .map(|_| {
                s = s.wrapping_mul(1664525).wrapping_add(1013904223);
                (s >> 8) as f32 / (1u32 << 24) as f32 * 20.0 - 10.0
            })
            .collect()
    }

    #[test]
    fn gpu_matches_cpu_reference_bit_for_bit() {
        let ctx = VulkanContext::new_headless().expect("headless context (lavapipe/llvmpipe)");
        let spv = load_spv();
        // Sizes that exercise interior, all four clamped borders, the rounded-up
        // dispatch padding (non-multiples of 8), and degenerate 1-wide grids.
        for &(w, h) in &[(8usize, 8usize), (7, 5), (13, 9), (1, 1), (16, 3), (3, 16)] {
            let src = grid(0x1234_5678 ^ ((w * 131 + h) as u32), w * h);
            let coeff = 0.2f32;
            let dt = 1.0f32;
            let mut expected = vec![0.0f32; w * h];
            diffuse_step(&src, &mut expected, w, h, coeff, dt);
            let got = unsafe { diffuse_step_gpu(&ctx, &spv, &src, w, h, coeff, dt) }
                .expect("gpu dispatch");
            assert_eq!(
                got, expected,
                "GPU field_diffuse must match the CPU reference bit-for-bit at {w}x{h}"
            );
        }
    }
}
