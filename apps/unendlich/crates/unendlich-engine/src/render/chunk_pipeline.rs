//! Graphics pipeline for rendering chunk meshes.
//!
//! Differences from the triangle pipeline:
//! * Vertex inputs (pos: vec3, color: vec3) — `Vertex` struct from `mesher`
//! * Depth test (LESS, write enabled)
//! * Back-face culling (CCW front face)
//! * Descriptor set 0 binding 0: uniform buffer with view-projection matrix

use ash::{vk, Device};
use std::ffi::CStr;
use crate::render::mesher::PackedVertex;

// Production path: embed shader bytecode at compile time. Self-contained
// binary, no runtime file dependency.
#[cfg(not(feature = "hot-shaders"))]
const VERT_SPV: &[u8] = include_bytes!("../../../../assets/shaders/chunk.vert.spv");
#[cfg(not(feature = "hot-shaders"))]
const FRAG_SPV: &[u8] = include_bytes!("../../../../assets/shaders/chunk.frag.spv");

// Hot-iteration path: load shader bytecode at pipeline-creation time.
// Lets you edit GLSL → cargo build (regenerates .spv via build.rs) →
// restart the binary, with no Rust recompile of any other module.
#[cfg(feature = "hot-shaders")]
fn load_vert_spv() -> Vec<u8> {
    crate::render::hot_shaders::read_spv("chunk.vert.spv")
}
#[cfg(feature = "hot-shaders")]
fn load_frag_spv() -> Vec<u8> {
    crate::render::hot_shaders::read_spv("chunk.frag.spv")
}

pub struct ChunkPipeline {
    pub pipeline: vk::Pipeline,
    /// All six front-face \u00d7 cull winding variants, built once at startup
    /// and selected per-frame by `current()` for the live winding sweep in the
    /// debug panel. `pipeline` aliases `variants[0]` (the default, CW+BACK).
    pub variants: [vk::Pipeline; 6],
    pub layout: vk::PipelineLayout,
    pub descriptor_set_layout: vk::DescriptorSetLayout,
}

impl ChunkPipeline {
    pub fn new(
        device: &Device,
        render_pass: vk::RenderPass,
    ) -> Result<Self, vk::Result> {
        #[cfg(not(feature = "hot-shaders"))]
        let (vert_module, frag_module) = (
            create_shader_module(device, VERT_SPV)?,
            create_shader_module(device, FRAG_SPV)?,
        );
        #[cfg(feature = "hot-shaders")]
        let (vert_module, frag_module) = {
            let v = load_vert_spv();
            let f = load_frag_spv();
            (
                create_shader_module(device, &v)?,
                create_shader_module(device, &f)?,
            )
        };
        let entry = unsafe { CStr::from_bytes_with_nul_unchecked(b"main\0") };

        let stages = [
            vk::PipelineShaderStageCreateInfo::builder()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert_module)
                .name(entry)
                .build(),
            vk::PipelineShaderStageCreateInfo::builder()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(frag_module)
                .name(entry)
                .build(),
        ];

        // Vertex binding + attributes for the packed 8-byte vertex format
        // (v0.24.20-vertex-pack). See `mesher::PackedVertex` for the layout.
        //
        // Attribute 0 (position): VK_FORMAT_A2B10G10R10_UNORM_PACK32 — 4 bytes,
        // delivered to the vertex shader as a `vec4` already normalized to
        // [0, 1]. The shader multiplies xyz by 32.0 to recover chunk-local
        // position in blocks, then adds chunk_origin (read from the chunks
        // SSBO at binding 3) to get world position.
        //
        // Attribute 1 (color): VK_FORMAT_R8G8B8A8_UNORM — 4 bytes, delivered
        // as a `vec4` normalized to [0, 1]. Face brightness is already baked
        // into rgb by the mesher; alpha is reserved.
        let binding = vk::VertexInputBindingDescription {
            binding: 0,
            stride: PackedVertex::STRIDE, // 8
            input_rate: vk::VertexInputRate::VERTEX,
        };
        let attrs = [
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 0,
                format: vk::Format::R32_UINT, // r349: manual unpack in shader (Asahi UNORM quirk)
                offset: 0,
            },
            vk::VertexInputAttributeDescription {
                binding: 0,
                location: 1,
                format: vk::Format::R8G8B8A8_UNORM,
                offset: 4, // pos_packed (4 bytes) → color_packed
            },
        ];
        let bindings = [binding];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::builder()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attrs);

        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::builder()
            .topology(vk::PrimitiveTopology::TRIANGLE_LIST)
            .primitive_restart_enable(false);

        let viewport_state = vk::PipelineViewportStateCreateInfo::builder()
            .viewport_count(1)
            .scissor_count(1);

        // Front face is CLOCKWISE because:
        //   1. The mesher emits vertices in CCW order assuming Y-up world space.
        //   2. Vulkan clip space has Y-down, opposite of OpenGL. Our projection
        //      matrix flips Y (camera.rs builds an OpenGL-style projection then
        //      negates the Y row to get Vulkan clip space).
        //   3. That Y-flip reverses apparent winding, so CCW-from-outside in
        //      world space becomes CW-from-outside after projection.
        //
        // If you ever rebuild the projection matrix without the Y-flip, change
        // this back to COUNTER_CLOCKWISE.
        // v0.24.73 diagnostic: UNENDLICH_NO_CULL=1 disables backface culling
        // (cull_mode = NONE) to bisect the "faces visible from the side but
        // gone head-on" report. With culling OFF: if the missing faces
        // REAPPEAR, the bug is winding/culling (the front faces were being
        // culled when viewed head-on); if they STAY missing, the geometry
        // isn't being emitted or is shaded into the fog — not a cull problem.
        // Default OFF (BACK culling) — zero cost when unset.
        let no_cull = std::env::var("UNENDLICH_NO_CULL").ok().as_deref() == Some("1");
        // Six winding/cull variants for the live debug sweep (panel slider).
        // Built once; selected per-frame via `current()`. UNENDLICH_NO_CULL=1
        // forces cull NONE across all of them (legacy diagnostic override).
        let winding_variants: [(vk::FrontFace, vk::CullModeFlags); 6] = [
            (vk::FrontFace::CLOCKWISE,         vk::CullModeFlags::BACK),
            (vk::FrontFace::CLOCKWISE,         vk::CullModeFlags::FRONT),
            (vk::FrontFace::CLOCKWISE,         vk::CullModeFlags::NONE),
            (vk::FrontFace::COUNTER_CLOCKWISE, vk::CullModeFlags::BACK),
            (vk::FrontFace::COUNTER_CLOCKWISE, vk::CullModeFlags::FRONT),
            (vk::FrontFace::COUNTER_CLOCKWISE, vk::CullModeFlags::NONE),
        ];
        let rasterizers: Vec<vk::PipelineRasterizationStateCreateInfo> = winding_variants
            .iter()
            .map(|&(front_face, cull)| {
                let cull = if no_cull { vk::CullModeFlags::NONE } else { cull };
                vk::PipelineRasterizationStateCreateInfo::builder()
                    .depth_clamp_enable(false)
                    .rasterizer_discard_enable(false)
                    .polygon_mode(vk::PolygonMode::FILL)
                    .line_width(1.0)
                    .cull_mode(cull)
                    .front_face(front_face)
                    .depth_bias_enable(false)
                    .build()
            })
            .collect();

        let multisample = vk::PipelineMultisampleStateCreateInfo::builder()
            .sample_shading_enable(false)
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);

        let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::builder()
            .depth_test_enable(true)
            .depth_write_enable(true)
            .depth_compare_op(vk::CompareOp::LESS)
            .depth_bounds_test_enable(false)
            .stencil_test_enable(false);

        let color_blend_attachment = vk::PipelineColorBlendAttachmentState::builder()
            .color_write_mask(vk::ColorComponentFlags::RGBA)
            .blend_enable(false)
            .build();
        let color_blend_attachments = [color_blend_attachment];

        let color_blend = vk::PipelineColorBlendStateCreateInfo::builder()
            .logic_op_enable(false)
            .attachments(&color_blend_attachments);

        let dyn_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state = vk::PipelineDynamicStateCreateInfo::builder()
            .dynamic_states(&dyn_states);

        // Descriptor set: four bindings.
        //   binding 0 — uniform buffer (camera + atmosphere). Visible
        //               to both stages: vertex reads view_proj; fragment
        //               reads everything else.
        //   binding 1 — combined image sampler (shadow map). Fragment-
        //               only. The fragment shader projects v_world_pos
        //               into light-space using `light_view_proj` from
        //               the UBO, then samples this depth image with
        //               comparison filtering to drive the sun-term
        //               shadow attenuation.
        //   binding 2 — combined image sampler (3D froxel volume).
        //               Fragment-only. v0.16 groundwork: the volume
        //               is allocated, cleared to a sentinel, and bound
        //               here, but not yet filled per frame. Fragment
        //               shader checks the sample's transmittance and
        //               falls back to the per-fragment fog march if
        //               the froxel is unfilled (sentinel detected).
        //   binding 3 — storage buffer (chunks metadata SSBO). Vertex-
        //               only. Indexed by `gl_InstanceIndex` (which the
        //               cull shader populates as `chunk_slot_index` via
        //               `first_instance`); reads `aabb_min.xyz` and
        //               `aabb_min.w` to reconstruct the chunk's world
        //               origin from the packed chunk-local vertex
        //               position. v0.24.20-vertex-pack groundwork.
        let dsl_bindings = [
            vk::DescriptorSetLayoutBinding::builder()
                .binding(0)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
                .build(),
            vk::DescriptorSetLayoutBinding::builder()
                .binding(1)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                .build(),
            vk::DescriptorSetLayoutBinding::builder()
                .binding(2)
                .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT)
                .build(),
            vk::DescriptorSetLayoutBinding::builder()
                .binding(3)
                .descriptor_type(vk::DescriptorType::STORAGE_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::VERTEX)
                .build(),
        ];
        let dsl_info = vk::DescriptorSetLayoutCreateInfo::builder()
            .bindings(&dsl_bindings);
        let descriptor_set_layout = unsafe { device.create_descriptor_set_layout(&dsl_info, None)? };

        let set_layouts = [descriptor_set_layout];
        let layout_info = vk::PipelineLayoutCreateInfo::builder()
            .set_layouts(&set_layouts);
        let layout = unsafe { device.create_pipeline_layout(&layout_info, None)? };

        let infos: Vec<vk::GraphicsPipelineCreateInfo> = rasterizers
            .iter()
            .map(|raster| {
                vk::GraphicsPipelineCreateInfo::builder()
                    .stages(&stages)
                    .vertex_input_state(&vertex_input)
                    .input_assembly_state(&input_assembly)
                    .viewport_state(&viewport_state)
                    .rasterization_state(raster)
                    .multisample_state(&multisample)
                    .depth_stencil_state(&depth_stencil)
                    .color_blend_state(&color_blend)
                    .dynamic_state(&dynamic_state)
                    .layout(layout)
                    .render_pass(render_pass)
                    .subpass(0)
                    .build()
            })
            .collect();

        let pipelines = unsafe {
            device.create_graphics_pipelines(
                vk::PipelineCache::null(),
                &infos,
                None,
            )
        }.map_err(|(_, e)| e)?;

        unsafe {
            device.destroy_shader_module(vert_module, None);
            device.destroy_shader_module(frag_module, None);
        }

        let variants = [
            pipelines[0], pipelines[1], pipelines[2],
            pipelines[3], pipelines[4], pipelines[5],
        ];
        Ok(Self { pipeline: variants[0], variants, layout, descriptor_set_layout })
    }

    /// VkPipeline for the currently-selected winding/cull variant
    /// (`DEBUG.winding_variant`). Read live at bind time, so the panel slider
    /// changes the bound pipeline next frame with no re-mesh or recreation.
    pub fn current(&self) -> vk::Pipeline {
        let v = crate::debug_settings::DEBUG
            .winding_variant
            .load(std::sync::atomic::Ordering::Relaxed) as usize;
        self.variants[v.min(self.variants.len() - 1)]
    }

    pub fn destroy(&mut self, device: &Device) {
        unsafe {
            // `pipeline` aliases `variants[0]`; destroy via the array only.
            for &p in &self.variants {
                device.destroy_pipeline(p, None);
            }
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
