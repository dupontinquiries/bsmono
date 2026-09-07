//! A headless compute runner for the convert pass.
//!
//! This exists so the colour maths in `shaders/convert.wgsl` can be *executed*
//! and checked against hand-computed values, rather than shipped on the
//! strength of it looking right. It runs anywhere a Vulkan device exists,
//! including Mesa's lavapipe (`VK_ICD_FILENAMES=.../lvp_icd.json`), which is
//! how it is verified in CI without a GPU.
//!
//! # What this does and does not verify
//!
//! Verified here: the WGSL compiles and passes naga's validator, and the
//! matrix, range expansion, plane layout and transfer-function branches produce
//! the right numbers.
//!
//! **Not** verified here: the DMA-BUF import path. Lavapipe does not expose
//! `VK_EXT_image_drm_format_modifier`, so planes are uploaded through a staging
//! buffer instead of imported. That is the one part of `import.rs` that still
//! needs real hardware, and the layout/modifier/queue-ownership rules in its
//! module docs remain unverified.

use ash::{vk, Device, Entry, Instance};
use std::ffi::CString;

pub const CONVERT_WGSL: &str = include_str!("../shaders/convert.wgsl");

#[derive(Debug)]
pub enum HarnessError {
    /// No Vulkan device. Callers should skip rather than fail.
    NoDevice(String),
    Shader(String),
    Vulkan(String),
}

impl std::fmt::Display for HarnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HarnessError::NoDevice(s) => write!(f, "no vulkan device: {s}"),
            HarnessError::Shader(s) => write!(f, "shader: {s}"),
            HarnessError::Vulkan(s) => write!(f, "vulkan: {s}"),
        }
    }
}

type R<T> = std::result::Result<T, HarnessError>;

fn vkerr(what: &str) -> impl Fn(vk::Result) -> HarnessError + '_ {
    move |e| HarnessError::Vulkan(format!("{what}: {e:?}"))
}

/// Compile the WGSL to SPIR-V through naga, which also validates it.
pub fn compile_convert_shader() -> R<Vec<u32>> {
    let module = naga::front::wgsl::parse_str(CONVERT_WGSL)
        .map_err(|e| HarnessError::Shader(format!("wgsl parse: {e:?}")))?;

    let info = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|e| HarnessError::Shader(format!("validation: {e:?}")))?;

    let mut opts = naga::back::spv::Options::default();
    // The compute entry point writes to a storage image; without this naga
    // emits a capability the driver may reject.
    opts.lang_version = (1, 3);
    naga::back::spv::write_vec(
        &module,
        &info,
        &opts,
        Some(&naga::back::spv::PipelineOptions {
            shader_stage: naga::ShaderStage::Compute,
            entry_point: "main".into(),
        }),
    )
    .map_err(|e| HarnessError::Shader(format!("spv: {e:?}")))
}

/// Uniform block, matching `Params` in the shader. `repr(C)` and the trailing
/// pad are load-bearing: a mismatch here shifts every field and the failure
/// looks like wrong colour maths.
#[repr(C)]
#[derive(Copy, Clone, Debug)]
pub struct Params {
    pub matrix_id: u32,
    pub transfer_id: u32,
    pub range_id: u32,
    pub plane_count: u32,
    pub width: u32,
    pub height: u32,
    pub target_nits: f32,
    pub _pad: u32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            matrix_id: 1, // BT.709
            transfer_id: 0,
            range_id: 0, // limited
            plane_count: 2,
            width: 0,
            height: 0,
            target_nits: 203.0,
            _pad: 0,
        }
    }
}

/// One input plane: raw bytes plus its dimensions and Vulkan format.
pub struct PlaneData {
    pub width: u32,
    pub height: u32,
    pub format: vk::Format,
    pub bytes: Vec<u8>,
}

impl PlaneData {
    pub fn r8(width: u32, height: u32, bytes: Vec<u8>) -> Self {
        Self {
            width,
            height,
            format: vk::Format::R8_UNORM,
            bytes,
        }
    }
    pub fn rg8(width: u32, height: u32, bytes: Vec<u8>) -> Self {
        Self {
            width,
            height,
            format: vk::Format::R8G8_UNORM,
            bytes,
        }
    }
}

/// Decode an IEEE 754 binary16, which is what an `rgba16float` storage image
/// reads back as.
fn half_to_f32(h: u16) -> f32 {
    let sign = ((h >> 15) & 1) as u32;
    let exp = ((h >> 10) & 0x1F) as u32;
    let frac = (h & 0x3FF) as u32;
    let bits = if exp == 0 {
        if frac == 0 {
            sign << 31
        } else {
            // Subnormal: renormalise.
            let mut e = -1i32;
            let mut f = frac;
            while f & 0x400 == 0 {
                f <<= 1;
                e -= 1;
            }
            let f = f & 0x3FF;
            (sign << 31) | (((127 - 15 + e + 1) as u32) << 23) | (f << 13)
        }
    } else if exp == 31 {
        (sign << 31) | (0xFF << 23) | (frac << 13)
    } else {
        (sign << 31) | ((exp + 127 - 15) << 23) | (frac << 13)
    };
    f32::from_bits(bits)
}

pub struct Harness {
    _entry: Entry,
    instance: Instance,
    device: Device,
    phys: vk::PhysicalDevice,
    queue: vk::Queue,
    queue_family: u32,
    command_pool: vk::CommandPool,
}

impl Harness {
    /// Returns `NoDevice` when there is no Vulkan implementation, so tests can
    /// skip rather than fail on a machine without one.
    pub fn new() -> R<Self> {
        // SAFETY: loading the system Vulkan loader; failure is reported, not
        // undefined.
        let entry = unsafe { Entry::load() }
            .map_err(|e| HarnessError::NoDevice(format!("loader: {e}")))?;

        let app_name = CString::new("vidkit-convert-harness").unwrap();
        let app_info = vk::ApplicationInfo::builder()
            .application_name(&app_name)
            .api_version(vk::make_api_version(0, 1, 1, 0));
        let create = vk::InstanceCreateInfo::builder().application_info(&app_info);
        // SAFETY: create info is valid for the duration of the call.
        let instance = unsafe { entry.create_instance(&create, None) }
            .map_err(|e| HarnessError::NoDevice(format!("create_instance: {e:?}")))?;

        // SAFETY: instance is live.
        let physicals = unsafe { instance.enumerate_physical_devices() }
            .map_err(vkerr("enumerate_physical_devices"))?;
        let phys = *physicals
            .first()
            .ok_or_else(|| HarnessError::NoDevice("no physical devices".into()))?;

        // SAFETY: phys came from this instance.
        let families = unsafe { instance.get_physical_device_queue_family_properties(phys) };
        let queue_family = families
            .iter()
            .position(|f| f.queue_flags.contains(vk::QueueFlags::COMPUTE))
            .ok_or_else(|| HarnessError::NoDevice("no compute queue".into()))?
            as u32;

        let priorities = [1.0f32];
        let qinfo = [vk::DeviceQueueCreateInfo::builder()
            .queue_family_index(queue_family)
            .queue_priorities(&priorities)
            .build()];
        let dinfo = vk::DeviceCreateInfo::builder().queue_create_infos(&qinfo);
        // SAFETY: qinfo outlives the call.
        let device = unsafe { instance.create_device(phys, &dinfo, None) }
            .map_err(vkerr("create_device"))?;
        // SAFETY: family index came from the same device.
        let queue = unsafe { device.get_device_queue(queue_family, 0) };

        let pinfo = vk::CommandPoolCreateInfo::builder()
            .queue_family_index(queue_family)
            .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER);
        // SAFETY: device is live.
        let command_pool = unsafe { device.create_command_pool(&pinfo, None) }
            .map_err(vkerr("create_command_pool"))?;

        Ok(Self {
            _entry: entry,
            instance,
            device,
            phys,
            queue,
            queue_family,
            command_pool,
        })
    }

    pub fn device_name(&self) -> String {
        // SAFETY: phys belongs to instance.
        let props = unsafe { self.instance.get_physical_device_properties(self.phys) };
        let bytes: Vec<u8> = props
            .device_name
            .iter()
            .take_while(|c| **c != 0)
            .map(|c| *c as u8)
            .collect();
        String::from_utf8_lossy(&bytes).into_owned()
    }

    fn find_memory(&self, bits: u32, want: vk::MemoryPropertyFlags) -> R<u32> {
        // SAFETY: phys belongs to instance.
        let props = unsafe { self.instance.get_physical_device_memory_properties(self.phys) };
        for i in 0..props.memory_type_count {
            if bits & (1 << i) != 0 && props.memory_types[i as usize].property_flags.contains(want) {
                return Ok(i);
            }
        }
        Err(HarnessError::Vulkan(format!(
            "no memory type for bits {bits:#x} with {want:?}"
        )))
    }

    fn create_buffer(
        &self,
        size: u64,
        usage: vk::BufferUsageFlags,
        props: vk::MemoryPropertyFlags,
    ) -> R<(vk::Buffer, vk::DeviceMemory)> {
        let info = vk::BufferCreateInfo::builder()
            .size(size)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        // SAFETY: device is live; info valid for the call.
        let buf = unsafe { self.device.create_buffer(&info, None) }.map_err(vkerr("create_buffer"))?;
        // SAFETY: buf just created on this device.
        let req = unsafe { self.device.get_buffer_memory_requirements(buf) };
        let ix = self.find_memory(req.memory_type_bits, props)?;
        let alloc = vk::MemoryAllocateInfo::builder()
            .allocation_size(req.size)
            .memory_type_index(ix);
        // SAFETY: alloc is valid.
        let mem = unsafe { self.device.allocate_memory(&alloc, None) }
            .map_err(vkerr("allocate_memory(buffer)"))?;
        // SAFETY: buf and mem are compatible by construction.
        unsafe { self.device.bind_buffer_memory(buf, mem, 0) }.map_err(vkerr("bind_buffer_memory"))?;
        Ok((buf, mem))
    }

    fn create_image(
        &self,
        width: u32,
        height: u32,
        format: vk::Format,
        usage: vk::ImageUsageFlags,
    ) -> R<(vk::Image, vk::DeviceMemory, vk::ImageView)> {
        let info = vk::ImageCreateInfo::builder()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D {
                width,
                height,
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        // SAFETY: info valid for the call.
        let image = unsafe { self.device.create_image(&info, None) }.map_err(vkerr("create_image"))?;
        // SAFETY: image just created here.
        let req = unsafe { self.device.get_image_memory_requirements(image) };
        let ix = self.find_memory(req.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)?;
        let alloc = vk::MemoryAllocateInfo::builder()
            .allocation_size(req.size)
            .memory_type_index(ix);
        // SAFETY: alloc valid.
        let mem = unsafe { self.device.allocate_memory(&alloc, None) }
            .map_err(vkerr("allocate_memory(image)"))?;
        // SAFETY: compatible by construction.
        unsafe { self.device.bind_image_memory(image, mem, 0) }.map_err(vkerr("bind_image_memory"))?;

        let vinfo = vk::ImageViewCreateInfo::builder()
            .image(image)
            .view_type(vk::ImageViewType::TYPE_2D)
            .format(format)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            });
        // SAFETY: image is live.
        let view = unsafe { self.device.create_image_view(&vinfo, None) }
            .map_err(vkerr("create_image_view"))?;
        Ok((image, mem, view))
    }

    fn one_shot<F: FnOnce(vk::CommandBuffer)>(&self, f: F) -> R<()> {
        let info = vk::CommandBufferAllocateInfo::builder()
            .command_pool(self.command_pool)
            .level(vk::CommandBufferLevel::PRIMARY)
            .command_buffer_count(1);
        // SAFETY: pool belongs to this device.
        let cbs = unsafe { self.device.allocate_command_buffers(&info) }
            .map_err(vkerr("allocate_command_buffers"))?;
        let cb = cbs[0];
        let begin = vk::CommandBufferBeginInfo::builder()
            .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT);
        // SAFETY: cb is freshly allocated and not in use.
        unsafe { self.device.begin_command_buffer(cb, &begin) }.map_err(vkerr("begin"))?;
        f(cb);
        // SAFETY: recording finished.
        unsafe { self.device.end_command_buffer(cb) }.map_err(vkerr("end"))?;

        let submits = [vk::SubmitInfo::builder().command_buffers(&cbs).build()];
        // SAFETY: cb is recorded and the queue belongs to this device.
        unsafe {
            self.device
                .queue_submit(self.queue, &submits, vk::Fence::null())
                .map_err(vkerr("queue_submit"))?;
            // A fence would be better in production; for a one-shot harness
            // waiting on the queue is simpler and equally correct.
            self.device
                .queue_wait_idle(self.queue)
                .map_err(vkerr("queue_wait_idle"))?;
            self.device.free_command_buffers(self.command_pool, &cbs);
        }
        Ok(())
    }

    fn transition(
        &self,
        cb: vk::CommandBuffer,
        image: vk::Image,
        from: vk::ImageLayout,
        to: vk::ImageLayout,
    ) {
        let barrier = vk::ImageMemoryBarrier::builder()
            .old_layout(from)
            .new_layout(to)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            })
            .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
            .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
            .build();
        // SAFETY: cb is recording; barrier is valid.
        unsafe {
            self.device.cmd_pipeline_barrier(
                cb,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[barrier],
            );
        }
    }

    fn upload_plane(&self, plane: &PlaneData) -> R<(vk::Image, vk::DeviceMemory, vk::ImageView)> {
        let (image, mem, view) = self.create_image(
            plane.width.max(1),
            plane.height.max(1),
            plane.format,
            vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::TRANSFER_DST,
        )?;

        let size = plane.bytes.len().max(1) as u64;
        let (sbuf, smem) = self.create_buffer(
            size,
            vk::BufferUsageFlags::TRANSFER_SRC,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;
        // SAFETY: smem is host visible and coherent; the mapping is used only
        // for this copy and unmapped immediately.
        unsafe {
            let p = self
                .device
                .map_memory(smem, 0, size, vk::MemoryMapFlags::empty())
                .map_err(vkerr("map staging"))? as *mut u8;
            std::ptr::copy_nonoverlapping(plane.bytes.as_ptr(), p, plane.bytes.len());
            self.device.unmap_memory(smem);
        }

        self.one_shot(|cb| {
            self.transition(
                cb,
                image,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            );
            let region = vk::BufferImageCopy::builder()
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_extent(vk::Extent3D {
                    width: plane.width.max(1),
                    height: plane.height.max(1),
                    depth: 1,
                })
                .build();
            // SAFETY: cb recording; buffer and image are compatible.
            unsafe {
                self.device.cmd_copy_buffer_to_image(
                    cb,
                    sbuf,
                    image,
                    vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                    &[region],
                );
            }
            self.transition(
                cb,
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            );
        })?;

        // SAFETY: the copy has completed (queue_wait_idle in one_shot).
        unsafe {
            self.device.destroy_buffer(sbuf, None);
            self.device.free_memory(smem, None);
        }
        Ok((image, mem, view))
    }

    /// Run the convert pass and return the output as linear RGBA rows.
    pub fn run_convert(&self, params: Params, planes: &[PlaneData]) -> R<Vec<[f32; 4]>> {
        let w = params.width.max(1);
        let h = params.height.max(1);

        let spirv = compile_convert_shader()?;

        // Uploaded planes, padded to three bindings; the shader always declares
        // three and Vulkan requires every declared binding to be valid even
        // when the code never samples it.
        let mut uploaded = Vec::new();
        for p in planes {
            uploaded.push(self.upload_plane(p)?);
        }
        while uploaded.len() < 3 {
            uploaded.push(self.upload_plane(&PlaneData::r8(1, 1, vec![0u8]))?);
        }

        let (out_image, out_mem, out_view) = self.create_image(
            w,
            h,
            vk::Format::R16G16B16A16_SFLOAT,
            vk::ImageUsageFlags::STORAGE | vk::ImageUsageFlags::TRANSFER_SRC,
        )?;

        let ubo_size = std::mem::size_of::<Params>() as u64;
        let (ubo, ubo_mem) = self.create_buffer(
            ubo_size,
            vk::BufferUsageFlags::UNIFORM_BUFFER,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;
        // SAFETY: host visible and coherent; written once before use.
        unsafe {
            let p = self
                .device
                .map_memory(ubo_mem, 0, ubo_size, vk::MemoryMapFlags::empty())
                .map_err(vkerr("map ubo"))? as *mut Params;
            *p = params;
            self.device.unmap_memory(ubo_mem);
        }

        let sinfo = vk::SamplerCreateInfo::builder()
            .mag_filter(vk::Filter::LINEAR)
            .min_filter(vk::Filter::LINEAR)
            .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
            .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE);
        // SAFETY: info valid.
        let sampler = unsafe { self.device.create_sampler(&sinfo, None) }
            .map_err(vkerr("create_sampler"))?;

        // Bindings mirror the shader's @group(0) declarations exactly.
        let bindings = [
            binding(0, vk::DescriptorType::UNIFORM_BUFFER),
            binding(1, vk::DescriptorType::SAMPLED_IMAGE),
            binding(2, vk::DescriptorType::SAMPLED_IMAGE),
            binding(3, vk::DescriptorType::SAMPLED_IMAGE),
            binding(4, vk::DescriptorType::SAMPLER),
            binding(5, vk::DescriptorType::STORAGE_IMAGE),
        ];
        let dsl_info = vk::DescriptorSetLayoutCreateInfo::builder().bindings(&bindings);
        // SAFETY: bindings outlive the call.
        let dsl = unsafe { self.device.create_descriptor_set_layout(&dsl_info, None) }
            .map_err(vkerr("create_descriptor_set_layout"))?;

        let pool_sizes = [
            pool_size(vk::DescriptorType::UNIFORM_BUFFER, 1),
            pool_size(vk::DescriptorType::SAMPLED_IMAGE, 3),
            pool_size(vk::DescriptorType::SAMPLER, 1),
            pool_size(vk::DescriptorType::STORAGE_IMAGE, 1),
        ];
        let pool_info = vk::DescriptorPoolCreateInfo::builder()
            .pool_sizes(&pool_sizes)
            .max_sets(1);
        // SAFETY: sizes outlive the call.
        let dpool = unsafe { self.device.create_descriptor_pool(&pool_info, None) }
            .map_err(vkerr("create_descriptor_pool"))?;

        let layouts = [dsl];
        let ainfo = vk::DescriptorSetAllocateInfo::builder()
            .descriptor_pool(dpool)
            .set_layouts(&layouts);
        // SAFETY: pool has room for one set of this layout.
        let sets = unsafe { self.device.allocate_descriptor_sets(&ainfo) }
            .map_err(vkerr("allocate_descriptor_sets"))?;
        let set = sets[0];

        let ubo_info = [vk::DescriptorBufferInfo {
            buffer: ubo,
            offset: 0,
            range: ubo_size,
        }];
        let img_infos: Vec<[vk::DescriptorImageInfo; 1]> = uploaded
            .iter()
            .map(|(_, _, v)| {
                [vk::DescriptorImageInfo {
                    sampler: vk::Sampler::null(),
                    image_view: *v,
                    image_layout: vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                }]
            })
            .collect();
        let samp_info = [vk::DescriptorImageInfo {
            sampler,
            image_view: vk::ImageView::null(),
            image_layout: vk::ImageLayout::UNDEFINED,
        }];
        let out_info = [vk::DescriptorImageInfo {
            sampler: vk::Sampler::null(),
            image_view: out_view,
            image_layout: vk::ImageLayout::GENERAL,
        }];

        let writes = [
            write_buffer(set, 0, &ubo_info),
            write_image(set, 1, vk::DescriptorType::SAMPLED_IMAGE, &img_infos[0]),
            write_image(set, 2, vk::DescriptorType::SAMPLED_IMAGE, &img_infos[1]),
            write_image(set, 3, vk::DescriptorType::SAMPLED_IMAGE, &img_infos[2]),
            write_image(set, 4, vk::DescriptorType::SAMPLER, &samp_info),
            write_image(set, 5, vk::DescriptorType::STORAGE_IMAGE, &out_info),
        ];
        // SAFETY: all referenced resources are live.
        unsafe { self.device.update_descriptor_sets(&writes, &[]) };

        let module_info = vk::ShaderModuleCreateInfo::builder().code(&spirv);
        // SAFETY: spirv is valid SPIR-V produced by naga.
        let module = unsafe { self.device.create_shader_module(&module_info, None) }
            .map_err(vkerr("create_shader_module"))?;

        let pl_info = vk::PipelineLayoutCreateInfo::builder().set_layouts(&layouts);
        // SAFETY: layouts outlive the call.
        let pipeline_layout = unsafe { self.device.create_pipeline_layout(&pl_info, None) }
            .map_err(vkerr("create_pipeline_layout"))?;

        let entry = CString::new("main").unwrap();
        let stage = vk::PipelineShaderStageCreateInfo::builder()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(module)
            .name(&entry)
            .build();
        let cp_info = [vk::ComputePipelineCreateInfo::builder()
            .stage(stage)
            .layout(pipeline_layout)
            .build()];
        // SAFETY: stage and layout are live.
        let pipelines = unsafe {
            self.device
                .create_compute_pipelines(vk::PipelineCache::null(), &cp_info, None)
        }
        .map_err(|(_, e)| HarnessError::Vulkan(format!("create_compute_pipelines: {e:?}")))?;
        let pipeline = pipelines[0];

        let readback_size = (w as u64) * (h as u64) * 8; // rgba16f
        let (rbuf, rmem) = self.create_buffer(
            readback_size,
            vk::BufferUsageFlags::TRANSFER_DST,
            vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
        )?;

        self.one_shot(|cb| {
            self.transition(cb, out_image, vk::ImageLayout::UNDEFINED, vk::ImageLayout::GENERAL);
            // SAFETY: cb recording; pipeline and set are live.
            unsafe {
                self.device
                    .cmd_bind_pipeline(cb, vk::PipelineBindPoint::COMPUTE, pipeline);
                self.device.cmd_bind_descriptor_sets(
                    cb,
                    vk::PipelineBindPoint::COMPUTE,
                    pipeline_layout,
                    0,
                    &[set],
                    &[],
                );
                // Workgroup size is 8x8 in the shader; round up so edge pixels
                // are covered and rely on the shader's bounds check.
                self.device.cmd_dispatch(cb, (w + 7) / 8, (h + 7) / 8, 1);
            }
            self.transition(
                cb,
                out_image,
                vk::ImageLayout::GENERAL,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            );
            let region = vk::BufferImageCopy::builder()
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_extent(vk::Extent3D {
                    width: w,
                    height: h,
                    depth: 1,
                })
                .build();
            // SAFETY: cb recording; image is in TRANSFER_SRC_OPTIMAL.
            unsafe {
                self.device.cmd_copy_image_to_buffer(
                    cb,
                    out_image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    rbuf,
                    &[region],
                );
            }
        })?;

        // SAFETY: the copy completed before one_shot returned.
        let out = unsafe {
            let p = self
                .device
                .map_memory(rmem, 0, readback_size, vk::MemoryMapFlags::empty())
                .map_err(vkerr("map readback"))? as *const u16;
            let count = (w * h) as usize;
            let mut v = Vec::with_capacity(count);
            for i in 0..count {
                v.push([
                    half_to_f32(*p.add(i * 4)),
                    half_to_f32(*p.add(i * 4 + 1)),
                    half_to_f32(*p.add(i * 4 + 2)),
                    half_to_f32(*p.add(i * 4 + 3)),
                ]);
            }
            self.device.unmap_memory(rmem);
            v
        };

        // SAFETY: all work has completed; each handle is destroyed once.
        unsafe {
            self.device.destroy_buffer(rbuf, None);
            self.device.free_memory(rmem, None);
            self.device.destroy_pipeline(pipeline, None);
            self.device.destroy_pipeline_layout(pipeline_layout, None);
            self.device.destroy_shader_module(module, None);
            self.device.destroy_descriptor_pool(dpool, None);
            self.device.destroy_descriptor_set_layout(dsl, None);
            self.device.destroy_sampler(sampler, None);
            self.device.destroy_buffer(ubo, None);
            self.device.free_memory(ubo_mem, None);
            self.device.destroy_image_view(out_view, None);
            self.device.destroy_image(out_image, None);
            self.device.free_memory(out_mem, None);
            for (img, mem, view) in uploaded {
                self.device.destroy_image_view(view, None);
                self.device.destroy_image(img, None);
                self.device.free_memory(mem, None);
            }
        }

        Ok(out)
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        // SAFETY: no work is outstanding; each object is destroyed once.
        unsafe {
            let _ = self.device.device_wait_idle();
            self.device.destroy_command_pool(self.command_pool, None);
            self.device.destroy_device(None);
            self.instance.destroy_instance(None);
        }
        let _ = self.queue_family;
    }
}

fn binding(b: u32, ty: vk::DescriptorType) -> vk::DescriptorSetLayoutBinding {
    vk::DescriptorSetLayoutBinding::builder()
        .binding(b)
        .descriptor_type(ty)
        .descriptor_count(1)
        .stage_flags(vk::ShaderStageFlags::COMPUTE)
        .build()
}

fn pool_size(ty: vk::DescriptorType, n: u32) -> vk::DescriptorPoolSize {
    vk::DescriptorPoolSize {
        ty,
        descriptor_count: n,
    }
}

fn write_buffer(
    set: vk::DescriptorSet,
    binding: u32,
    info: &[vk::DescriptorBufferInfo],
) -> vk::WriteDescriptorSet {
    vk::WriteDescriptorSet::builder()
        .dst_set(set)
        .dst_binding(binding)
        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
        .buffer_info(info)
        .build()
}

fn write_image(
    set: vk::DescriptorSet,
    binding: u32,
    ty: vk::DescriptorType,
    info: &[vk::DescriptorImageInfo],
) -> vk::WriteDescriptorSet {
    vk::WriteDescriptorSet::builder()
        .dst_set(set)
        .dst_binding(binding)
        .descriptor_type(ty)
        .image_info(info)
        .build()
}
