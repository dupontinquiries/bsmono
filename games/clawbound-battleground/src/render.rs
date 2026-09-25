// wgpu renderer: instanced microvoxels with shadow mapping, terrain,
// sky, HDR + ACES post. Shared cube geometry; per-model instance buffers.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};
use wgpu::util::DeviceExt;

use crate::voxel::{CUBE_IDX, CUBE_NORMS, CUBE_VERTS, VoxInst, VoxelMesh};

pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const SHADOW_SIZE: u32 = 2048;
/// Three lanes x two teams x inner/outer. terrain.wgsl hard-codes it.
pub const MAX_TOWER_RINGS: usize = 12;
/// Draw slots per frame. 512 held a 5v5 (10 cats of ~15 parts, ~120
/// world chunks, towers, minions); a 15v15 needs about three times the cats.
/// 256 B each (DYN_ALIGN), so this is 1 MiB of uniform buffer.
pub const MAX_DRAWS: usize = 4096;
pub const MAX_PARTICLES: usize = 48000;
pub const DYN_ALIGN: u64 = 256;

// --- GPU structs --------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct FrameUniform {
    pub view_proj: [[f32; 4]; 4],
    pub inv_view_proj: [[f32; 4]; 4],
    pub light_view_proj: [[f32; 4]; 4],
    pub sun_dir: [f32; 4],
    pub cam_pos: [f32; 4],
    pub fog_color: [f32; 4],
    /// x time, y fog density, z exposure, w night
    pub params: [f32; 4],
    /// rgb sun colour × intensity, w cloud cover
    pub sun_col: [f32; 4],
    /// x rain, y snow, z wetness, w lightning
    pub weather: [f32; 4],
    /// xyz wind (world u/s), w gust
    pub wind: [f32; 4],
    /// x flood rise (u), y quake shake, z fire glow near camera, w unused
    pub event: [f32; 4],
    /// What is shouldering through the flora: xyz where, w how hard (signed:
    /// a wake swings leaves back through rest). APPENDED, and only
    /// voxel.wgsl declares it -- the other shaders bind the same buffer with
    /// the shorter struct, which is legal because no layout sets a
    /// min_binding_size.
    pub brush: [[f32; 4]; crate::puppet::MAX_BRUSHES],
    /// Blast fronts: a = xyz centre + ring radius, b = strength, width.
    /// voxel.wgsl (trees shudder as the ring passes).
    pub blast_a: [[f32; 4]; crate::groundfx::MAX_BLASTS],
    pub blast_b: [[f32; 4]; crate::groundfx::MAX_BLASTS],
    /// Broken ground: x, z, radius, how open. terrain.wgsl.
    pub cracks: [[f32; 4]; crate::groundfx::MAX_CRACKS],
    /// Tower range rings as the player should see them: x, z, range, state.
    /// terrain.wgsl. See `Game::tower_rings`.
    pub towers: [[f32; 4]; MAX_TOWER_RINGS],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct DrawData {
    pub model: [[f32; 4]; 4],
    pub tint: [f32; 4],
    pub params: [f32; 4], // emissive, voxel_size, unused, unused
}

#[derive(Clone, Copy, Debug)]
pub struct Camera {
    pub eye: Vec3,
    pub target: Vec3,
    pub fov_y: f32,
}

impl Camera {
    pub fn view_proj(&self, aspect: f32) -> Mat4 {
        let view = Mat4::look_at_rh(self.eye, self.target, Vec3::Y);
        let proj = Mat4::perspective_rh(self.fov_y, aspect, 0.25, 500.0);
        proj * view
    }
}

// --- mesh cache ----------------------------------------------------------------

/// A smooth mesh on the GPU (`smoothvox`): vertices and triangle indices.
struct SmoothGpu {
    vbuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
    count: u32,
}

/// Bytes in the smooth pipeline's `Lights` uniform (smooth.wgsl).
const LIGHTS_BYTES: u64 = 16 + 16 * 8 + 16 * 8 + 16 * 32;

pub struct GpuMesh {
    buf: wgpu::Buffer,
    count: u32,
    /// Bounding sphere of the instances in model space, for shadow culling.
    bound: (Vec3, f32),
}

// --- renderer -------------------------------------------------------------------

/// What the last frame drew: for profiling culling and budgets.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct FrameStats {
    pub draws: u32,
    /// Voxel instances in the main pass (opaque + transparent).
    pub instances: u64,
    /// Voxel instances in the shadow pass.
    pub shadow_instances: u64,
}

/// The per-map meshes the renderer owns directly: terrain, water, the flood
/// plane. Built for the active map (`map::active`), rebuilt on a switch.
struct WorldGeometry {
    terrain_vbuf: wgpu::Buffer,
    terrain_ibuf: wgpu::Buffer,
    terrain_count: u32,
    water_vbuf: wgpu::Buffer,
    water_ibuf: wgpu::Buffer,
    water_count: u32,
    flood_vbuf: wgpu::Buffer,
    flood_ibuf: wgpu::Buffer,
    flood_count: u32,
}

impl WorldGeometry {
    fn build(device: &wgpu::Device) -> WorldGeometry {
        let (tv, ti) = build_terrain();
        let terrain_vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrain vbuf"),
            contents: bytemuck::cast_slice(&tv),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let terrain_ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("terrain ibuf"),
            contents: bytemuck::cast_slice(&ti),
            usage: wgpu::BufferUsages::INDEX,
        });
        let terrain_count = ti.len() as u32;
        let (wv, wi) = build_water();
        let water_vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("water vbuf"),
            contents: bytemuck::cast_slice(&wv),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let water_ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("water ibuf"),
            contents: bytemuck::cast_slice(&wi),
            usage: wgpu::BufferUsages::INDEX,
        });
        let water_count = wi.len() as u32;
        let (fv, fi) = build_flood_plane();
        let flood_vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("flood vbuf"),
            contents: bytemuck::cast_slice(&fv),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let flood_ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("flood ibuf"),
            contents: bytemuck::cast_slice(&fi),
            usage: wgpu::BufferUsages::INDEX,
        });
        let flood_count = fi.len() as u32;
        WorldGeometry { terrain_vbuf, terrain_ibuf, terrain_count, water_vbuf, water_ibuf, water_count, flood_vbuf, flood_ibuf, flood_count }
    }
}

pub struct Renderer {
    /// The map the terrain, water and flood meshes were built for.
    pub world_map: crate::map::MapId,
    /// Counters for the last `render_frame`.
    pub stats: FrameStats,
    /// Draws refused this frame for want of a slot; reported, not silent.
    pub dropped_draws: u32,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub width: u32,
    pub height: u32,
    pub sample_count: u32,

    cube_vbuf: wgpu::Buffer,
    cube_ibuf: wgpu::Buffer,
    meshes: HashMap<u64, GpuMesh>,
    next_mesh_id: u64,

    main_bgl: wgpu::BindGroupLayout,
    shadow_bgl: wgpu::BindGroupLayout,
    post_bgl: wgpu::BindGroupLayout,

    voxel_pipe: wgpu::RenderPipeline,
    voxel_alpha_pipe: wgpu::RenderPipeline,
    shadow_pipe: wgpu::RenderPipeline,
    terrain_pipe: wgpu::RenderPipeline,
    water_pipe: wgpu::RenderPipeline,
    sky_pipe: wgpu::RenderPipeline,
    post_pipe: wgpu::RenderPipeline,

    frame_buf: wgpu::Buffer,
    draw_buf: wgpu::Buffer,
    particle_buf: wgpu::Buffer,
    main_bind: wgpu::BindGroup,
    shadow_bind: wgpu::BindGroup,
    shadow_tex: wgpu::Texture,
    shadow_view: wgpu::TextureView,
    shadow_sampler: wgpu::Sampler,

    terrain_vbuf: wgpu::Buffer,
    terrain_ibuf: wgpu::Buffer,
    terrain_count: u32,
    water_vbuf: wgpu::Buffer,
    water_ibuf: wgpu::Buffer,
    water_count: u32,
    flood_vbuf: wgpu::Buffer,
    flood_ibuf: wgpu::Buffer,
    flood_count: u32,

    post_sampler: wgpu::Sampler,
    post_params: wgpu::Buffer,
    hdr_msaa: Option<(wgpu::Texture, wgpu::TextureView)>,
    hdr_resolve: Option<(wgpu::Texture, wgpu::TextureView)>,
    depth: Option<(wgpu::Texture, wgpu::TextureView)>,
    post_bind: Option<wgpu::BindGroup>,

    // queued frame state
    opaque: Vec<(u64, u32)>,       // (mesh id, draw index)
    transparent: Vec<(u64, u32)>,
    draws: Vec<DrawData>,
    particles: Vec<VoxInst>,
    // --- ultra preview: smooth, furred, PBR characters ------------------
    smooth_pipe: wgpu::RenderPipeline,
    lights_buf: wgpu::Buffer,
    lights_bind: wgpu::BindGroup,
    smooth_meshes: HashMap<u64, SmoothGpu>,
    smooth_queue: Vec<(u64, u32)>,
    /// Voxel draws that only cast shadows (an ultra cat's shadow is its
    /// voxel parts': the same shape, and no second shadow pipeline).
    shadow_only: Vec<(u64, u32)>,
    /// Coloured point lights this frame: (xyz, radius), (rgb * intensity).
    pub lights: Vec<([f32; 4], [f32; 4])>,
    /// Spheres the coloured lights' shadows are traced against (the cats).
    pub occluders: Vec<[f32; 4]>,
    /// Fur shells per smooth draw, and strands per unit of model space.
    pub fur_shells: u32,
    pub fur_density: f32,
    camera: Camera,
    time: f32,
    pub exposure: f32,
    /// Sky state for this frame — set by the caller each frame (see weather.rs).
    pub atmosphere: crate::weather::Atmosphere,
    /// Set by the scene each frame from `puppet::Troupe::brushes`.
    pub brushes: Vec<[f32; 4]>,
    /// Set by the scene each frame from `groundfx::GroundFx`.
    pub blast_a: [[f32; 4]; crate::groundfx::MAX_BLASTS],
    pub blast_b: [[f32; 4]; crate::groundfx::MAX_BLASTS],
    pub cracks: [[f32; 4]; crate::groundfx::MAX_CRACKS],
    /// Set by the scene each frame from `Game::tower_rings`.
    pub towers: [[f32; 4]; MAX_TOWER_RINGS],
    /// Shadow-map samples marched per fragment for light shafts through the
    /// canopy. 0 turns them off: on a handset GPU this is the first thing to
    /// give up, and the screen-space shafts toward the sun remain.
    pub shaft_steps: u32,
    /// (flood rise, quake shake, fire glow) — set by the caller (disasters.rs).
    pub event: (f32, f32, f32),
}

impl Renderer {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue, width: u32, height: u32, target_format: wgpu::TextureFormat) -> Renderer {
        let cube_inter: Vec<f32> = CUBE_VERTS
            .iter()
            .zip(CUBE_NORMS.iter())
            .flat_map(|(p, n)| vec![p[0], p[1], p[2], n[0], n[1], n[2]])
            .collect();
        let cube_vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cube vbuf"),
            contents: bytemuck::cast_slice(&cube_inter),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let cube_ibuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("cube ibuf"),
            contents: bytemuck::cast_slice(&CUBE_IDX),
            usage: wgpu::BufferUsages::INDEX,
        });

        // bind group layouts
        let main_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("main bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: true,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison),
                    count: None,
                },
            ],
        });

        let shadow_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shadow bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: true,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });

        let post_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: true,
                    },
                    count: None,
                },
            ],
        });

        let voxel_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("voxel.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/voxel.wgsl").into()),
        });
        let shadow_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shadow.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/shadow.wgsl").into()),
        });
        let terrain_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("terrain.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/terrain.wgsl").into()),
        });
        let water_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("water"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/water.wgsl").into()),
        });
        let sky_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("sky.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/sky.wgsl").into()),
        });
        let post_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("post.wgsl"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/post.wgsl").into()),
        });

        let voxel_vbl = wgpu::VertexBufferLayout {
            array_stride: 24,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 },
            ],
        };
        let inst_vbl = wgpu::VertexBufferLayout {
            array_stride: 48,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &[
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 0, shader_location: 2 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 16, shader_location: 3 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 32, shader_location: 4 },
            ],
        };
        let terrain_vbl = wgpu::VertexBufferLayout {
            array_stride: 40,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 24, shader_location: 2 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 36, shader_location: 3 },
            ],
        };
        // water surface: position + bed height (for depth) + flow direction
        let water_vbl = wgpu::VertexBufferLayout {
            array_stride: 32,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 12, shader_location: 1 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 16, shader_location: 2 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 24, shader_location: 3 },
            ],
        };

        let mk_layout = |ls: [&wgpu::BindGroupLayout; 1]| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &ls,
                push_constant_ranges: &[],
            })
        };
        let main_pl = mk_layout([&main_bgl]);
        let shadow_pl = mk_layout([&shadow_bgl]);
        let post_pl = mk_layout([&post_bgl]);

        let depth_state_write = depth_state(DepthUse::Opaque);
        let depth_state_ro = depth_state(DepthUse::Blended);

        let msaa = wgpu::MultisampleState { count: 4, ..Default::default() };
        let msaa1 = wgpu::MultisampleState::default();

        let voxel_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voxel"),
            layout: Some(&main_pl),
            vertex: wgpu::VertexState { module: &voxel_module, entry_point: "vs_main", buffers: &[voxel_vbl.clone(), inst_vbl.clone()] },
            fragment: Some(wgpu::FragmentState {
                module: &voxel_module,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Back), ..Default::default() },
            depth_stencil: Some(depth_state_write.clone()),
            multisample: msaa,
            multiview: None,
        });

        let voxel_alpha_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("voxel-alpha"),
            layout: Some(&main_pl),
            vertex: wgpu::VertexState { module: &voxel_module, entry_point: "vs_main", buffers: &[voxel_vbl.clone(), inst_vbl.clone()] },
            fragment: Some(wgpu::FragmentState {
                module: &voxel_module,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(depth_state_ro.clone()),
            multisample: msaa,
            multiview: None,
        });

        let shadow_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow"),
            layout: Some(&shadow_pl),
            vertex: wgpu::VertexState { module: &shadow_module, entry_point: "vs_main", buffers: &[voxel_vbl.clone(), inst_vbl.clone()] },
            fragment: None,
            primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Back), ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: msaa1,
            multiview: None,
        });

        // the terrain receives the coloured point lights too (ultra), so it
        // shares the lights layout with the smooth characters
        let lights_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lights bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let lit_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("lit layout"),
            bind_group_layouts: &[&main_bgl, &lights_bgl],
            push_constant_ranges: &[],
        });
        let terrain_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain"),
            layout: Some(&lit_pl),
            vertex: wgpu::VertexState { module: &terrain_module, entry_point: "vs_main", buffers: &[terrain_vbl] },
            fragment: Some(wgpu::FragmentState {
                module: &terrain_module,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Back), ..Default::default() },
            depth_stencil: Some(depth_state_write.clone()),
            multisample: msaa,
            multiview: None,
        });

        let water_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("water"),
            layout: Some(&main_pl),
            vertex: wgpu::VertexState { module: &water_module, entry_point: "vs_main", buffers: &[water_vbl] },
            fragment: Some(wgpu::FragmentState {
                module: &water_module,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(depth_state_ro.clone()),
            multisample: msaa,
            multiview: None,
        });

        let sky_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky"),
            layout: Some(&main_pl),
            vertex: wgpu::VertexState { module: &sky_module, entry_point: "vs_main", buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &sky_module,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FORMAT,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(depth_state_ro),
            multisample: msaa,
            multiview: None,
        });

        // the ultra preview's smooth characters (smooth.wgsl): the world's
        // bind group, plus the coloured lights and their occluders
        let smooth_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("smooth"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/smooth.wgsl").into()),
        });
        let lights_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lights"),
            size: LIGHTS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let lights_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lights bind"),
            layout: &lights_bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: lights_buf.as_entire_binding() }],
        });
        let smooth_pl = &lit_pl;
        let smooth_vbl = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<crate::smoothvox::SmoothVert>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 12, shader_location: 1 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x4, offset: 24, shader_location: 2 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32, offset: 40, shader_location: 3 },
            ],
        };
        let smooth_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("smooth"),
            layout: Some(smooth_pl),
            vertex: wgpu::VertexState { module: &smooth_module, entry_point: "vs_main", buffers: &[smooth_vbl] },
            fragment: Some(wgpu::FragmentState {
                module: &smooth_module,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState { format: HDR_FORMAT, blend: Some(wgpu::BlendState::REPLACE), write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Back), ..Default::default() },
            depth_stencil: Some(depth_state_write.clone()),
            multisample: msaa,
            multiview: None,
        });

        let post_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("post"),
            layout: Some(&post_pl),
            vertex: wgpu::VertexState { module: &post_module, entry_point: "vs_main", buffers: &[] },
            fragment: Some(wgpu::FragmentState {
                module: &post_module,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::REPLACE),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: None,
            multisample: msaa1,
            multiview: None,
        });

        // frame uniform + storage buffers
        let frame_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("frame uniform"),
            size: std::mem::size_of::<FrameUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let draw_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("draw storage"),
            size: (MAX_DRAWS as u64) * DYN_ALIGN,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let particle_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("particles"),
            size: (MAX_PARTICLES as u64) * 48,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let shadow_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("shadow map"),
            size: wgpu::Extent3d { width: SHADOW_SIZE, height: SHADOW_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let shadow_view = shadow_tex.create_view(&Default::default());
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow sampler"),
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });

        let main_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("main bind"),
            layout: &main_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: frame_buf.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &draw_buf,
                        offset: 0,
                        size: std::num::NonZeroU64::new(std::mem::size_of::<DrawData>() as u64),
                    }),
                },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&shadow_view) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&shadow_sampler) },
            ],
        });

        let shadow_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shadow bind"),
            layout: &shadow_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: frame_buf.as_entire_binding() },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &draw_buf,
                        offset: 0,
                        size: std::num::NonZeroU64::new(std::mem::size_of::<DrawData>() as u64),
                    }),
                },
            ],
        });

        // terrain
        let WorldGeometry { terrain_vbuf, terrain_ibuf, terrain_count, water_vbuf, water_ibuf, water_count, flood_vbuf, flood_ibuf, flood_count } = WorldGeometry::build(&device);

        let post_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let post_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post params"),
            // 3 vec4, view-proj, its inverse, the camera, and per blast a
            // world centre+radius and a strength+thickness
            size: 48 + 64 * 2 + 16 + 32 * crate::groundfx::MAX_RINGS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let mut r = Renderer {
            device,
            queue,
            width,
            height,
            sample_count: 4,
            cube_vbuf,
            cube_ibuf,
            meshes: HashMap::new(),
            next_mesh_id: 1,
            world_map: crate::map::active(),
            main_bgl,
            shadow_bgl,
            post_bgl,
            voxel_pipe,
            voxel_alpha_pipe,
            shadow_pipe,
            terrain_pipe,
            water_pipe,
            sky_pipe,
            post_pipe,
            frame_buf,
            draw_buf,
            particle_buf,
            main_bind,
            shadow_bind,
            shadow_tex,
            shadow_view,
            shadow_sampler,
            terrain_vbuf,
            terrain_ibuf,
            terrain_count,
            water_vbuf,
            water_ibuf,
            water_count,
            flood_vbuf,
            flood_ibuf,
            flood_count,
            post_sampler,
            post_params,
            hdr_msaa: None,
            hdr_resolve: None,
            depth: None,
            post_bind: None,
            opaque: Vec::new(),
            transparent: Vec::new(),
            draws: Vec::new(),
            dropped_draws: 0,
            stats: FrameStats::default(),
            particles: Vec::new(),
            smooth_pipe,
            lights_buf,
            lights_bind,
            smooth_meshes: HashMap::new(),
            smooth_queue: Vec::new(),
            shadow_only: Vec::new(),
            lights: Vec::new(),
            occluders: Vec::new(),
            // stylized locks: few shells, chunky cells (smooth.wgsl)
            fur_shells: 7,
            fur_density: 24.0,
            camera: Camera { eye: Vec3::ZERO, target: Vec3::Y, fov_y: 1.05 },
            time: 0.0,
            exposure: 0.92,
            atmosphere: crate::weather::Weather::new().atmosphere(),
            brushes: Vec::new(),
            blast_a: Default::default(),
            blast_b: Default::default(),
            cracks: Default::default(),
            towers: Default::default(),
            // OFF. Both tunings tried so far turned the far field milky rather
            // than drawing shafts: under this jungle's canopy almost every
            // view ray is half-occluded, so the term fires everywhere, and 8
            // jittered samples average to glow. Measured on one frame: top
            // fifth of the image +19% and then +25% luma against the same
            // frame without. Kept behind the knob; see TODO.md.
            shaft_steps: 0,
            event: (0.0, 0.0, 0.0),
        };
        r.resize(width, height);
        r
    }

    /// Rebuild the terrain, water and flood plane for the active map, if
    /// they were built for another. The map switch: before this the map was
    /// fixed at launch, because `new` was the only place they were built.
    pub fn rebuild_world_geometry(&mut self) {
        let m = crate::map::active();
        if m == self.world_map {
            return;
        }
        let g = WorldGeometry::build(&self.device);
        self.terrain_vbuf = g.terrain_vbuf;
        self.terrain_ibuf = g.terrain_ibuf;
        self.terrain_count = g.terrain_count;
        self.water_vbuf = g.water_vbuf;
        self.water_ibuf = g.water_ibuf;
        self.water_count = g.water_count;
        self.flood_vbuf = g.flood_vbuf;
        self.flood_ibuf = g.flood_ibuf;
        self.flood_count = g.flood_count;
        self.world_map = m;
    }

    /// The id the next uploaded mesh will get: a mark to free back to.
    pub fn mesh_mark(&self) -> u64 {
        self.next_mesh_id
    }

    /// Free every mesh uploaded since `mark` (ids are handed out in
    /// order): how a scene gives its GPU memory back on a map switch.
    pub fn free_meshes_from(&mut self, mark: u64) {
        self.meshes.retain(|&id, _| id < mark);
        self.smooth_meshes.retain(|&id, _| id < mark);
    }

    pub fn mesh_count(&self) -> usize {
        self.meshes.len() + self.smooth_meshes.len()
    }

    pub fn upload_mesh(&mut self, mesh: &VoxelMesh) -> u64 {
        let id = self.next_mesh_id;
        self.next_mesh_id += 1;
        let buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("voxel instances"),
            contents: bytemuck::cast_slice(&mesh.inst),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        let bound = {
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for i in &mesh.inst {
                let p = Vec3::new(i.off_ao[0], i.off_ao[1], i.off_ao[2]);
                lo = lo.min(p);
                hi = hi.max(p);
            }
            if mesh.inst.is_empty() {
                (Vec3::ZERO, 0.0)
            } else {
                let c = (lo + hi) * 0.5;
                // + a generous voxel: instance scale is per voxel
                (c, (hi - c).length() + 2.0)
            }
        };
        self.meshes.insert(id, GpuMesh { buf, count: mesh.inst.len() as u32, bound });
        id
    }

    pub fn mesh_voxels(&self, id: u64) -> u32 {
        self.meshes.get(&id).map_or(0, |m| m.count)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width.max(1);
        self.height = height.max(1);
        // HDR scene targets (MSAA)
        let msaa_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hdr msaa"),
            size: wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 4,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let msaa_view = msaa_tex.create_view(&Default::default());
        let resolve_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("hdr resolve"),
            size: wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let resolve_view = resolve_tex.create_view(&Default::default());
        let depth_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("scene depth"),
            size: wgpu::Extent3d { width: self.width, height: self.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 4,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            // sampled by the post pass: blast fronts are occluded by what
            // stands in front of them (post.wgsl)
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let depth_view = depth_tex.create_view(&Default::default());
        self.hdr_msaa = Some((msaa_tex, msaa_view));
        self.hdr_resolve = Some((resolve_tex, resolve_view));
        self.depth = Some((depth_tex, depth_view));
        if let (Some((_, rview)), Some((_, dview))) = (&self.hdr_resolve, &self.depth) {
            self.post_bind = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("post bind"),
                layout: &self.post_bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(rview) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.post_sampler) },
                    wgpu::BindGroupEntry { binding: 2, resource: self.post_params.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(dview) },
                ],
            }));
        }
    }
}

// ---------------------------------------------------------------------------
// Frame recording + passes
// ---------------------------------------------------------------------------

/// Portrait / default daylight sun. In-game the sun comes from `Atmosphere`.
pub const SUN_DIR: Vec3 = Vec3::new(-0.42, 0.82, 0.32);

impl Renderer {
    fn brush_uniform(&self) -> [[f32; 4]; crate::puppet::MAX_BRUSHES] {
        let mut out = [[0.0f32; 4]; crate::puppet::MAX_BRUSHES];
        for (slot, b) in out.iter_mut().zip(&self.brushes) {
            *slot = *b;
        }
        out
    }

    /// Upload a smooth mesh (ultra preview); ids share the voxel meshes'
    /// counter, so `free_meshes_from` frees these too.
    pub fn upload_smooth(&mut self, m: &crate::smoothvox::SmoothMesh) -> u64 {
        let id = self.next_mesh_id;
        self.next_mesh_id += 1;
        if m.indices.is_empty() {
            return id;
        }
        let vbuf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("smooth vbuf"), contents: bytemuck::cast_slice(&m.verts), usage: wgpu::BufferUsages::VERTEX });
        let ibuf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("smooth ibuf"), contents: bytemuck::cast_slice(&m.indices), usage: wgpu::BufferUsages::INDEX });
        self.smooth_meshes.insert(id, SmoothGpu { vbuf, ibuf, count: m.indices.len() as u32 });
        id
    }

    /// Queue a smooth, furred draw (ultra preview).
    pub fn queue_smooth(&mut self, mesh: u64, model: Mat4, tint: [f32; 4]) {
        if self.draws.len() >= MAX_DRAWS - 1 {
            self.dropped_draws += 1;
            return;
        }
        let idx = self.draws.len() as u32;
        self.draws.push(DrawData { model: model.to_cols_array_2d(), tint, params: [0.0, 0.0, 0.0, 0.0] });
        self.smooth_queue.push((mesh, idx));
    }

    /// Queue a voxel draw that only casts a shadow.
    pub fn queue_shadow_only(&mut self, mesh: u64, model: Mat4, voxel: f32) {
        if self.draws.len() >= MAX_DRAWS - 1 {
            self.dropped_draws += 1;
            return;
        }
        let idx = self.draws.len() as u32;
        self.draws.push(DrawData { model: model.to_cols_array_2d(), tint: [1.0; 4], params: [0.0, voxel, 0.0, 0.0] });
        self.shadow_only.push((mesh, idx));
    }

    pub fn clear_queues(&mut self) {
        self.opaque.clear();
        self.transparent.clear();
        self.smooth_queue.clear();
        self.shadow_only.clear();
        self.draws.clear();
        if self.dropped_draws > 0 {
            eprintln!("[clawbound] {} draws dropped last frame (MAX_DRAWS {})", self.dropped_draws, MAX_DRAWS);
            self.dropped_draws = 0;
        }
    }

    pub fn queue_draw(&mut self, mesh: u64, model: Mat4, tint: [f32; 4], emissive: f32, voxel: f32, alpha: bool) {
        // The LAST slot is the particle draw's (see `render_frame`'s
        // staging). `>= MAX_DRAWS` let a full frame queue MAX_DRAWS draws and
        // the particle slot then wrote one past the buffer -- a wgpu
        // validation panic the first time a frame filled up (a 15v15).
        if self.draws.len() >= MAX_DRAWS - 1 {
            self.dropped_draws += 1;
            return;
        }
        let idx = self.draws.len() as u32;
        self.draws.push(DrawData {
            model: model.to_cols_array_2d(),
            tint,
            params: [emissive, voxel, 0.0, 0.0],
        });
        if alpha {
            self.transparent.push((mesh, idx));
        } else {
            self.opaque.push((mesh, idx));
        }
    }

    pub fn set_particles(&mut self, p: Vec<VoxInst>) {
        self.particles = p;
    }

    pub fn render_frame(&mut self, target: &wgpu::TextureView, cam: Camera, time: f32) {
        self.camera = cam;
        self.time = time;
        // Depth-testing the blended pipelines fixes blended-vs-opaque
        // ordering; two blended surfaces still composite in queue order
        // unless sorted. Painter's order, far to near.
        let eye = self.camera.eye;
        let mut order: Vec<(f32, (u64, u32))> = self
            .transparent
            .iter()
            .map(|t| {
                let c = self.draws[t.1 as usize].model[3];
                (Vec3::new(c[0], c[1], c[2]).distance_squared(eye), *t)
            })
            .collect();
        sort_far_to_near(&mut order);
        self.transparent = order.into_iter().map(|(_, t)| t).collect();
        let aspect = self.width as f32 / self.height as f32;
        let vp = cam.view_proj(aspect);
        let inv = vp.inverse();

        // shadow ortho covering the arena + inner jungle ring; the sun moves
        // with the weather (night sinks it), so the light matrix follows.
        let atm = self.atmosphere;
        let sun = atm.sun_dir.normalize();
        // The one shadow map used to cover a fixed 300 u box: 0.146 u per
        // texel, a third of a voxel, so every shadow was a blob. It is now
        // aimed at what the camera is looking at and sized to the view.
        let half = crate::groundfx::shadow_half_extent(cam.eye.distance(cam.target));
        let lvp = crate::groundfx::fit_shadow(cam.target, sun, half, SHADOW_SIZE);

        let fog_color = [atm.fog_color[0], atm.fog_color[1], atm.fog_color[2], 1.0];
        let frame = FrameUniform {
            view_proj: vp.to_cols_array_2d(),
            inv_view_proj: inv.to_cols_array_2d(),
            light_view_proj: lvp.to_cols_array_2d(),
            sun_dir: [sun.x, sun.y, sun.z, 0.0],
            cam_pos: [cam.eye.x, cam.eye.y, cam.eye.z, 1.0],
            fog_color,
            params: [time, atm.fog_density, self.exposure, atm.night],
            sun_col: [atm.sun_col[0], atm.sun_col[1], atm.sun_col[2], atm.cloud_cover],
            weather: [atm.rain, atm.snow, atm.wetness, atm.lightning],
            wind: [atm.wind.x, atm.wind.y, atm.wind.z, atm.gust],
            event: [self.event.0, self.event.1, self.event.2, self.shaft_steps as f32],
            brush: self.brush_uniform(),
            blast_a: self.blast_a,
            blast_b: self.blast_b,
            cracks: self.cracks,
            towers: self.towers,
        };
        self.queue.write_buffer(&self.frame_buf, 0, bytemuck::bytes_of(&frame));

        // draw storage (256-byte dynamic slots)
        // reserve last slot for the particle draw
        let mut staging = vec![0u8; (self.draws.len() + 1) * DYN_ALIGN as usize];
        for (i, d) in self.draws.iter().enumerate() {
            let off = i * DYN_ALIGN as usize;
            staging[off..off + std::mem::size_of::<DrawData>()]
                .copy_from_slice(bytemuck::bytes_of(d));
        }
        let particle_draw_idx = self.draws.len();
        let particle_dd = DrawData {
            model: Mat4::IDENTITY.to_cols_array_2d(),
            tint: [1.0, 1.0, 1.0, 1.0],
            params: [1.0, 1.0, 0.0, 0.0], // emissive, voxel size 1 (per-instance scale)
        };
        let off = particle_draw_idx * DYN_ALIGN as usize;
        staging[off..off + std::mem::size_of::<DrawData>()]
            .copy_from_slice(bytemuck::bytes_of(&particle_dd));
        self.queue.write_buffer(&self.draw_buf, 0, &staging);
        if !self.particles.is_empty() {
            self.queue.write_buffer(&self.particle_buf, 0, bytemuck::cast_slice(&self.particles));
        }
        let mut post_data: Vec<f32> = Vec::with_capacity(12 + 4 * crate::groundfx::MAX_RINGS);
        self.queue.write_buffer(
            &self.post_params,
            0,
            // exposure, vignette, time, lightning | bloom, night, rain, 0 |
            // sun uv x, sun uv y, ray strength, contrast
            {
                // project the sun (a far point along its direction) to screen uv
                let sp = vp * (cam.eye + sun * 400.0).extend(1.0);
                let (sun_uv, ray) = if sp.w > 0.0 {
                    let n = sp.truncate() / sp.w;
                    let uv = [n.x * 0.5 + 0.5, 0.5 - n.y * 0.5];
                    let in_frame = (1.0 - ((n.x.abs() - 1.0).max(0.0) + (n.y.abs() - 1.0).max(0.0)) * 1.5).clamp(0.0, 1.0);
                    // shafts want a low sun and a clear-ish sky
                    let low = (1.0 - sun.y * 1.1).clamp(0.0, 1.0);
                    ([uv[0], uv[1]], in_frame * (0.25 + 0.6 * low) * (1.0 - atm.cloud_cover * 0.8) * (1.0 - atm.night * 0.7))
                } else {
                    ([0.5, 0.5], 0.0)
                };
                let (shell_a, shell_b) = crate::groundfx::shell_blasts(&self.blast_a, &self.blast_b);
                let _ = aspect;
                post_data.clear();
                post_data.extend_from_slice(&[
                    self.exposure * (1.0 + atm.night * 0.35),
                    0.55 + atm.night * 0.2,
                    time,
                    atm.lightning,
                    0.55 + atm.night * 0.3,
                    atm.night,
                    atm.rain,
                    // heat shimmer: fire near the camera (the wildfire's glow)
                    self.event.2.clamp(0.0, 1.0),
                    sun_uv[0],
                    sun_uv[1],
                    ray,
                    0.22,
                ]);
                post_data.extend_from_slice(&vp.to_cols_array());
                post_data.extend_from_slice(&vp.inverse().to_cols_array());
                post_data.extend_from_slice(&[cam.eye.x, cam.eye.y, cam.eye.z, 0.0]);
                for r in shell_a {
                    post_data.extend_from_slice(&r);
                }
                for r in shell_b {
                    post_data.extend_from_slice(&r);
                }
                bytemuck::cast_slice(&post_data)
            },
        );

        // the coloured lights and occluders (smooth.wgsl / terrain.wgsl
        // `Lights`): written every frame -- the terrain reads them too, and
        // outside ultra they are empty
        {
            let mut l = vec![0.0f32; (LIGHTS_BYTES / 4) as usize];
            let nl = self.lights.len().min(8);
            let no = self.occluders.len().min(32);
            l[0..4].copy_from_slice(&[nl as f32, no as f32, self.fur_shells as f32, self.fur_density]);
            for (k, (p, c)) in self.lights.iter().take(8).enumerate() {
                l[4 + k * 4..8 + k * 4].copy_from_slice(p);
                l[36 + k * 4..40 + k * 4].copy_from_slice(c);
            }
            for (k, o) in self.occluders.iter().take(32).enumerate() {
                l[68 + k * 4..72 + k * 4].copy_from_slice(o);
            }
            self.queue.write_buffer(&self.lights_buf, 0, bytemuck::cast_slice(&l));
        }

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame"),
        });

        {
            let count = |q: &[(u64, u32)]| q.iter().filter_map(|(m, _)| self.meshes.get(m)).map(|m| m.count as u64).sum::<u64>();
            let (o, t) = (count(&self.opaque), count(&self.transparent));
            self.stats = FrameStats { draws: self.draws.len() as u32, instances: o + t, shadow_instances: 0 };
        }
        let mut shadow_n = 0u64;
        // --- shadow pass ---------------------------------------------------------
        if !self.opaque.is_empty() {
            let mut sp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            sp.set_pipeline(&self.shadow_pipe);
            sp.set_bind_group(0, &self.shadow_bind, &[0]);
            sp.set_vertex_buffer(0, self.cube_vbuf.slice(..));
            sp.set_index_buffer(self.cube_ibuf.slice(..), wgpu::IndexFormat::Uint32);
            // Only what can land in the shadow map. It covers a box round
            // what the camera looks at, and drawing every chunk the CAMERA
            // sees into it doubled the voxel work of every frame
            // (CLAWBOUND_SHOTS=perf: "shadow" == "instances" everywhere).
            for (mesh_id, di) in self.opaque.iter().chain(self.shadow_only.iter()) {
                if let Some(m) = self.meshes.get(mesh_id) {
                    let model = Mat4::from_cols_array_2d(&self.draws[*di as usize].model);
                    let c = model.transform_point3(m.bound.0);
                    let sc = model.x_axis.truncate().length().max(model.y_axis.truncate().length()).max(model.z_axis.truncate().length());
                    if !sphere_in_frustum(&lvp, c, m.bound.1 * sc) {
                        continue;
                    }
                    shadow_n += m.count as u64;
                    sp.set_bind_group(0, &self.shadow_bind, &[(*di as u32) * DYN_ALIGN as u32]);
                    sp.set_vertex_buffer(1, m.buf.slice(..));
                    sp.draw_indexed(0..36, 0, 0..m.count);
                }
            }
        }

        self.stats.shadow_instances = shadow_n;
        // --- main scene pass (HDR MSAA) ------------------------------------------
        if let (Some((_, msaa_view)), Some((_, depth_view)), Some((_, resolve_view))) =
            (&self.hdr_msaa, &self.depth, &self.hdr_resolve)
        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: msaa_view,
                    resolve_target: Some(resolve_view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.04, g: 0.05, b: 0.09, a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            // sky
            rp.set_pipeline(&self.sky_pipe);
            rp.set_bind_group(0, &self.main_bind, &[0]);
            rp.draw(0..3, 0..1);

            // terrain
            rp.set_pipeline(&self.terrain_pipe);
            rp.set_bind_group(0, &self.main_bind, &[0]);
            rp.set_bind_group(1, &self.lights_bind, &[]);
            rp.set_vertex_buffer(0, self.terrain_vbuf.slice(..));
            rp.set_index_buffer(self.terrain_ibuf.slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed(0..self.terrain_count, 0, 0..1);

            // opaque voxels
            rp.set_pipeline(&self.voxel_pipe);
            rp.set_vertex_buffer(0, self.cube_vbuf.slice(..));
            rp.set_index_buffer(self.cube_ibuf.slice(..), wgpu::IndexFormat::Uint32);
            for (mesh_id, di) in &self.opaque {
                if let Some(m) = self.meshes.get(mesh_id) {
                    rp.set_bind_group(0, &self.main_bind, &[(*di as u32) * DYN_ALIGN as u32]);
                    rp.set_vertex_buffer(1, m.buf.slice(..));
                    rp.draw_indexed(0..36, 0, 0..m.count);
                }
            }

            // the ultra preview's smooth, furred characters: the skin and
            // `fur_shells` shells in one instanced draw per part
            if !self.smooth_queue.is_empty() {
                rp.set_pipeline(&self.smooth_pipe);
                rp.set_bind_group(1, &self.lights_bind, &[]);
                for (mesh_id, di) in &self.smooth_queue {
                    if let Some(m) = self.smooth_meshes.get(mesh_id) {
                        rp.set_bind_group(0, &self.main_bind, &[(*di as u32) * DYN_ALIGN as u32]);
                        rp.set_vertex_buffer(0, m.vbuf.slice(..));
                        rp.set_index_buffer(m.ibuf.slice(..), wgpu::IndexFormat::Uint32);
                        rp.draw_indexed(0..m.count, 0, 0..(1 + self.fur_shells));
                    }
                }
            }

            // water surface (after the opaque world so the bed shows through)
            if self.water_count > 0 {
                rp.set_pipeline(&self.water_pipe);
                rp.set_bind_group(0, &self.main_bind, &[0]);
                rp.set_vertex_buffer(0, self.water_vbuf.slice(..));
                rp.set_index_buffer(self.water_ibuf.slice(..), wgpu::IndexFormat::Uint32);
                rp.draw_indexed(0..self.water_count, 0, 0..1);
                // the flood: a second sheet over the whole floor, only while it rises
                if self.event.0 > 0.01 && self.flood_count > 0 {
                    rp.set_vertex_buffer(0, self.flood_vbuf.slice(..));
                    rp.set_index_buffer(self.flood_ibuf.slice(..), wgpu::IndexFormat::Uint32);
                    rp.draw_indexed(0..self.flood_count, 0, 0..1);
                }
            }

            // transparent voxels + particles
            rp.set_pipeline(&self.voxel_alpha_pipe);
            rp.set_vertex_buffer(0, self.cube_vbuf.slice(..));
            rp.set_index_buffer(self.cube_ibuf.slice(..), wgpu::IndexFormat::Uint32);
            for (mesh_id, di) in &self.transparent {
                if let Some(m) = self.meshes.get(mesh_id) {
                    rp.set_bind_group(0, &self.main_bind, &[(*di as u32) * DYN_ALIGN as u32]);
                    rp.set_vertex_buffer(1, m.buf.slice(..));
                    rp.draw_indexed(0..36, 0, 0..m.count);
                }
            }
            if !self.particles.is_empty() {
                rp.set_bind_group(0, &self.main_bind, &[particle_draw_idx as u32 * DYN_ALIGN as u32]);
                rp.set_vertex_buffer(1, self.particle_buf.slice(..(self.particles.len() as u64) * 48));
                rp.draw_indexed(0..36, 0, 0..self.particles.len() as u32);
            }
        }

        // --- post pass → target ----------------------------------------------------
        if let Some(post_bind) = &self.post_bind {
            let mut pp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("post"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pp.set_pipeline(&self.post_pipe);
            pp.set_bind_group(0, post_bind, &[]);
            pp.draw(0..3, 0..1);
        }

        self.queue.submit([encoder.finish()]);
    }

    /// Render a single god (body + weapon) to an RGBA8 buffer for UI portraits.
    fn f16_to_f32(h: u16) -> f32 {
        let sign = ((h >> 15) & 1) as f32;
        let exp = ((h >> 10) & 0x1F) as i32;
        let man = (h & 0x3FF) as f32;
        let v = if exp == 0 {
            man / 1024.0 * 2f32.powi(-14)
        } else if exp == 31 {
            f32::INFINITY
        } else {
            (1.0 + man / 1024.0) * 2f32.powi(exp - 15)
        };
        if sign > 0.5 { -v } else { v }
    }

    pub fn render_portrait(&mut self, body: u64, weapon: u64) -> Vec<u8> {
        const S: u32 = 128;
        let msaa_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("portrait msaa"),
            size: wgpu::Extent3d { width: S, height: S, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 4,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let resolve_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("portrait resolve"),
            size: wgpu::Extent3d { width: S, height: S, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let depth_tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("portrait depth"),
            size: wgpu::Extent3d { width: S, height: S, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 4,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });

        let cam = Camera {
            eye: Vec3::new(1.9, 1.85, 3.1),
            target: Vec3::new(0.0, 1.05, 0.0),
            fov_y: 0.66,
        };
        let vp = cam.view_proj(1.0);
        let sun = SUN_DIR.normalize();
        let light_view = Mat4::look_at_rh(sun * 6.0, Vec3::ZERO, Vec3::Y);
        let light_proj = Mat4::orthographic_rh(-3.0, 3.0, -3.0, 3.0, 1.0, 20.0);
        let frame = FrameUniform {
            view_proj: vp.to_cols_array_2d(),
            inv_view_proj: vp.inverse().to_cols_array_2d(),
            light_view_proj: (light_proj * light_view).to_cols_array_2d(),
            sun_dir: [sun.x, sun.y, sun.z, 0.0],
            cam_pos: [cam.eye.x, cam.eye.y, cam.eye.z, 1.0],
            fog_color: [0.5, 0.35, 0.28, 1.0],
            params: [0.0, 0.0, 1.35, 0.0],
            sun_col: [1.14, 1.0, 0.84, 0.1],
            weather: [0.0; 4],
            wind: [0.0; 4],
            event: [0.0; 4],
            brush: self.brush_uniform(),
            blast_a: self.blast_a,
            blast_b: self.blast_b,
            cracks: self.cracks,
            towers: self.towers,
        };
        self.queue.write_buffer(&self.frame_buf, 0, bytemuck::bytes_of(&frame));

        // draws: body slot 0, weapon slot 1
        let yaw = -0.5f32;
        let base = Mat4::from_axis_angle(Vec3::Y, yaw);
        let dds = [
            DrawData { model: base.to_cols_array_2d(), tint: [1.0; 4], params: [0.0, VOX, 0.0, 0.0] },
            DrawData {
                model: (base
                    * Mat4::from_translation(Vec3::new(0.42, 0.95, 0.05))
                    * Mat4::from_axis_angle(Vec3::Z, -0.9)
                    * Mat4::from_axis_angle(Vec3::X, -0.4)
                    * Mat4::from_translation(Vec3::new(0.0, -0.3, 0.0)))
                .to_cols_array_2d(),
                tint: [1.0; 4],
                params: [0.0, VOX, 0.0, 0.0],
            },
        ];
        let mut staging = vec![0u8; 2 * DYN_ALIGN as usize];
        for (i, d) in dds.iter().enumerate() {
            let off = i * DYN_ALIGN as usize;
            staging[off..off + std::mem::size_of::<DrawData>()].copy_from_slice(bytemuck::bytes_of(d));
        }
        self.queue.write_buffer(&self.draw_buf, 0, &staging);

        let read_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("portrait read"),
            size: (S * S * 8) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = self.device.create_command_encoder(&Default::default());
        let msaa_view = msaa_tex.create_view(&Default::default());
        let resolve_view = resolve_tex.create_view(&Default::default());
        let depth_view = depth_tex.create_view(&Default::default());
        {
            let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("portrait"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &msaa_view,
                    resolve_target: Some(&resolve_view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.05, g: 0.06, b: 0.10, a: 1.0 }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &depth_view,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rp.set_pipeline(&self.voxel_pipe);
            rp.set_bind_group(0, &self.main_bind, &[0]);
            rp.set_vertex_buffer(0, self.cube_vbuf.slice(..));
            rp.set_index_buffer(self.cube_ibuf.slice(..), wgpu::IndexFormat::Uint32);
            if let Some(m) = self.meshes.get(&body) {
                rp.set_vertex_buffer(1, m.buf.slice(..));
                rp.draw_indexed(0..36, 0, 0..m.count);
            }
            rp.set_bind_group(0, &self.main_bind, &[DYN_ALIGN as u32]);
            if let Some(m) = self.meshes.get(&weapon) {
                rp.set_vertex_buffer(1, m.buf.slice(..));
                rp.draw_indexed(0..36, 0, 0..m.count);
            }
        }
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &resolve_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &read_buf,
                layout: wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(S * 8), rows_per_image: None },
            },
            wgpu::Extent3d { width: S, height: S, depth_or_array_layers: 1 },
        );
        self.queue.submit([encoder.finish()]);

        let (tx, rx) = std::sync::mpsc::channel();
        let map = read_buf.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::Maintain::Wait);
        let _ = pollster::block_on(async {
            loop {
                if rx.try_recv().is_ok() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            let _ = map;
        });
        let data = read_buf.slice(..).get_mapped_range();

        // f16 RGBA → ACES → sRGB-ish u8
        let mut out = vec![0u8; (S * S * 4) as usize];
        for px in 0..(S * S) as usize {
            let off = px * 8;
            let r = Self::f16_to_f32(u16::from_le_bytes([data[off], data[off + 1]])) * 1.35;
            let g = Self::f16_to_f32(u16::from_le_bytes([data[off + 2], data[off + 3]])) * 1.35;
            let b = Self::f16_to_f32(u16::from_le_bytes([data[off + 4], data[off + 5]])) * 1.35;
            let aces = |x: f32| ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0);
            out[px * 4] = (aces(r).powf(1.0 / 2.2) * 255.0) as u8;
            out[px * 4 + 1] = (aces(g).powf(1.0 / 2.2) * 255.0) as u8;
            out[px * 4 + 2] = (aces(b).powf(1.0 / 2.2) * 255.0) as u8;
            out[px * 4 + 3] = 255;
        }
        out
    }
}

const VOX: f32 = crate::models::GOD_DRAW_VOXEL;

// ---------------------------------------------------------------------------
// Terrain geometry
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct TerrainVert {
    pos: [f32; 3],
    normal: [f32; 3],
    color: [f32; 3],
    /// material id for the detail shader: 0 grass, 1 dirt lane, 2 riverbed,
    /// 3 sand bank, 4 hill outcrop, 5 mountain rock, 6 moss
    mat: f32,
}

fn hash2(x: i32, z: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x27D4EB2D) ^ (z as u32).wrapping_mul(0x165667B1);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B3C6D);
    h ^= h >> 12;
    (h & 0xFFFF) as f32 / 65535.0
}

fn vnoise(x: f32, z: f32) -> f32 {
    let xi = x.floor() as i32;
    let zi = z.floor() as i32;
    let xf = x - xi as f32;
    let zf = z - zi as f32;
    let s = |t: f32| t * t * (3.0 - 2.0 * t);
    let a = hash2(xi, zi);
    let b = hash2(xi + 1, zi);
    let c = hash2(xi, zi + 1);
    let d = hash2(xi + 1, zi + 1);
    a + (b - a) * s(xf) + (c - a) * s(zf) + (a - b - c + d) * s(xf) * s(zf)
}

fn lane_distance(x: f32, z: f32) -> f32 {
    use crate::consts::Lane;
    let mut best = f32::MAX;
    for lane in [Lane::Top, Lane::Mid, Lane::Bot] {
        let pts = lane.polyline();
        for w in pts.windows(2) {
            let (a, b) = (w[0], w[1]);
            let ab = b - a;
            let t = ((Vec3::new(x, 0.0, z) - a).dot(ab) / ab.length_squared().max(0.001)).clamp(0.0, 1.0);
            let d = (a + ab * t - Vec3::new(x, 0.0, z)).length();
            best = best.min(d);
        }
    }
    best
}

/// What a pipeline does with the scene depth buffer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DepthUse {
    /// Terrain, opaque voxels: test and write.
    Opaque,
    /// Water, alpha voxels, particles, and the sky triangle: test, never write.
    Blended,
}

/// The depth state for a pipeline. A free function on purpose: it needs no
/// device, so `tests::blended_draws_are_depth_tested` can pin it.
///
/// History, because it has now happened twice: `Blended` shipped as
/// `CompareFunction::Always`, which made the river sheet ignore the depth
/// buffer and paint over canopies, banks and bridge piers. `3f70d2d7d` fixed
/// it; the v8 merge `1d46c86c5` replaced this file wholesale and the fix went
/// with it, because nothing but a screenshot could tell.
///
/// `LessEqual`, not `Less`: the sky is a full-screen triangle at NDC depth
/// exactly 1.0, which is the clear value, and it shares this state. A strict
/// `Less` would reject every sky fragment.
pub fn depth_state(usage: DepthUse) -> wgpu::DepthStencilState {
    let (write, compare) = match usage {
        DepthUse::Opaque => (true, wgpu::CompareFunction::Less),
        DepthUse::Blended => (false, wgpu::CompareFunction::LessEqual),
    };
    wgpu::DepthStencilState {
        format: wgpu::TextureFormat::Depth32Float,
        depth_write_enabled: write,
        depth_compare: compare,
        stencil: Default::default(),
        bias: Default::default(),
    }
}

/// Order blended draws far-to-near by squared distance. NaN sorts as
/// "equal" rather than panicking: a degenerate model matrix costs one
/// mis-ordered blend, not the frame.
pub fn sort_far_to_near<T>(items: &mut [(f32, T)]) {
    items.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
}

/// Shared terrain height field: flat playable floor, rolling jungle hills
/// beyond the arena bounds, a mountain rim at the horizon, and river valleys
/// that dip under the lanes (roads stay solid — bridges span the crossings).
pub fn terrain_height(x: f32, z: f32) -> f32 {
    if crate::map::active() == crate::map::MapId::Pass {
        return crate::pass::terrain_height(x, z);
    }
    use crate::consts::{river_dist, GROUND_HALF, RIVER_HALF_WIDTH, RIVER_DEPTH, ARENA_RADIUS, RIM_RADIUS};
    let r = (x * x + z * z).sqrt();
    let mut h = 0.0f32;
    // rolling jungle hills outside the playable floor
    if r > ARENA_RADIUS {
        let t = ((r - ARENA_RADIUS) / 42.0).min(1.0);
        let s = t * t * (3.0 - 2.0 * t);
        let n = vnoise(x * 0.045, z * 0.045);
        let n2 = vnoise(x * 0.11 + 7.3, z * 0.11 - 3.1);
        h = s * (2.2 + n * 6.0 + n2 * 2.2);
    }
    // mountain rim beyond the hills
    if r > RIM_RADIUS {
        let t = ((r - RIM_RADIUS) / (GROUND_HALF - RIM_RADIUS)).clamp(0.0, 1.0);
        h += t * t * 30.0 + vnoise(x * 0.05 + 13.0, z * 0.05) * t * 10.0;
    }
    // river valley (masked under lanes so the roads stay solid)
    let rd = river_dist(x, z);
    let river_reach = RIVER_HALF_WIDTH + 7.0;
    if rd < river_reach {
        let ld = lane_distance(x, z);
        let mut mask = smoothstep(4.0, 9.5, ld);
        // The lanes used to fill the channel in wherever they came near, so
        // the "bridges" stood over a causeway with no water under it. At a
        // crossing the river now runs THROUGH, and a log is the way over.
        // A SLUICE, not a pond. The lane does not cross the river here, it
        // grazes it, nearly parallel for ~25 u; opening a disc round the
        // crossing flooded that whole stretch and left the 13 u log as a raft
        // in the middle (one screenshot was enough). So the causeway stays,
        // cut by a channel across it just narrower than the log is long: the
        // log's ends rest on the road, and water runs through the gap the way
        // the log will roll.
        for c in crate::logs::crossings() {
            let (rx, rz) = (x - c.at.x, z - c.at.z);
            let along = (rx * c.along.x + rz * c.along.z).abs();
            let across = (rx * -c.along.z + rz * c.along.x).abs();
            let gap = (1.0 - smoothstep(crate::logs::SLUICE_HALF, crate::logs::SLUICE_HALF + 2.0, along)) * (1.0 - smoothstep(16.0, 22.0, across));
            mask = mask.max(gap);
        }
        // banks slope down past the water line, then the channel bowls out
        let t = (rd / river_reach).clamp(0.0, 1.0);
        let depth = (1.0 - t).powf(1.25) * (RIVER_DEPTH + 0.6) - 0.3;
        h -= depth.max(0.0) * mask;
    }
    h
}

pub fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Gribb–Hartmann frustum test (wgpu clip space: z ∈ [0, 1]).
pub fn sphere_in_frustum(vp: &Mat4, center: Vec3, radius: f32) -> bool {
    let m = vp.to_cols_array_2d();
    let row = |j: usize| Vec4::new(m[0][j], m[1][j], m[2][j], m[3][j]);
    let (r0, r1, r2, r3) = (row(0), row(1), row(2), row(3));
    // NEAR IS `r2`, NOT `r3 + r2`. wgpu clip space is 0 <= z <= w (the D3D
    // convention); `r3 + r2` is the OpenGL near plane for -w <= z <= w. The
    // function's own comment already said wgpu.
    //
    // MEASURED RATHER THAN ASSERTED, because I first wrote that this made the
    // near plane "far too permissive -- geometry behind the camera passes",
    // and that is wrong. Sweeping a point along the view axis, the two forms
    // differ over about 0.1 units just in front of the camera: the old one
    // additionally passes objects between the eye and the near plane, which
    // are clipped by the rasteriser anyway. Real, tiny, and not what I claimed
    // before checking.
    //
    // Corrected because a cull test should agree with the clip space it names,
    // not because it was costing frames.
    let planes = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r2, r3 - r2];
    planes.iter().all(|p| {
        // NORMALISE. The test compares a plane equation against `-radius`,
        // which is only a DISTANCE if the plane normal is unit length. The
        // rows of a view-projection are not, so without this the effective
        // cull radius is scaled by each plane's normal magnitude -- different
        // per plane, and wrong by more the further the projection is from
        // square. Conservative enough to hide on a 30-unit chunk; not on a
        // 2.2-unit minion.
        let n = (p.x * p.x + p.y * p.y + p.z * p.z).sqrt();
        if n <= f32::EPSILON {
            // A degenerate plane cannot reject anything; treating it as a pass
            // over-draws, where treating it as a fail would make objects
            // disappear on a bad matrix.
            return true;
        }
        (p.x * center.x + p.y * center.y + p.z * center.z + p.w) / n > -radius
    })
}

fn build_terrain() -> (Vec<TerrainVert>, Vec<u32>) {
    use crate::consts::{river_dist, GROUND_HALF, RIVER_HALF_WIDTH, ARENA_RADIUS};
    let half_ground = crate::consts::ground_half();
    let _ = GROUND_HALF;
    // 1.6 u grid (was 3.2): ~80k verts, fine enough for the river channels
    // and lane edges to read; the fragment shader adds the sub-metre detail.
    const STEP: f32 = 1.6;
    let n_side: usize = ((half_ground * 2.0) / STEP) as usize + 1;

    let mut verts = Vec::with_capacity(n_side * n_side);
    for iz in 0..n_side {
        for ix in 0..n_side {
            let x = -half_ground + ix as f32 * STEP;
            let z = -half_ground + iz as f32 * STEP;
            let h = terrain_height(x, z);
            // normal from height diffs
            let hl = terrain_height(x - STEP, z);
            let hr = terrain_height(x + STEP, z);
            let hd = terrain_height(x, z - STEP);
            let hu = terrain_height(x, z + STEP);
            let n = Vec3::new(hl - hr, 2.0 * STEP, hd - hu).normalize();
            let slope = 1.0 - n.y;

            let r = (x * x + z * z).sqrt();
            let n1 = vnoise(x * 0.11, z * 0.11);
            let n2 = vnoise(x * 0.31, z * 0.31);
            let nm = vnoise(x * 0.055 + 31.7, z * 0.055 - 12.4); // moss patches
            let rd = river_dist(x, z);
            let ld = lane_distance(x, z);

            let mut color;
            let mut mat = 0.0f32;
            if crate::map::active() == crate::map::MapId::Pass {
                (color, mat) = pass_ground_color(x, z, h, slope, n1, n2, nm);
            } else if r > crate::consts::RIM_RADIUS + 3.0 && h > 14.0 {
                // mountain rim: dark jungle rock with forest ledges
                let rock = [0.24 + n1 * 0.05, 0.24 + n2 * 0.04, 0.22 + n1 * 0.03];
                let forest = [0.085 + n1 * 0.03, 0.17 + n2 * 0.05, 0.075];
                let is_rock = slope > 0.10 || n2 > 0.62;
                mat = if is_rock { 5.0 } else { 0.0 };
                color = if is_rock { rock } else { forest };
                color = [color[0] * (0.85 + n1 * 0.3), color[1] * (0.85 + n1 * 0.3), color[2] * (0.85 + n1 * 0.3)];
            } else if r > ARENA_RADIUS {
                // overgrown hills ring
                let g = [0.075 + n1 * 0.035, 0.175 + n2 * 0.06, 0.07 + n2 * 0.02];
                let outcrop = slope > 0.16 || (n2 > 0.7 && h > 4.0);
                mat = if outcrop { 4.0 } else { 0.0 };
                color = if outcrop {
                    [0.30 + n1 * 0.05, 0.28 + n2 * 0.04, 0.25] // outcrop
                } else {
                    g
                };
            } else {
                // playable jungle floor — rich, mottled greens
                let mut g = [
                    0.125 + n1 * 0.07 + nm * 0.025,
                    0.25 + n1 * 0.09 + nm * 0.045,
                    0.10 + n2 * 0.045,
                ];
                if nm > 0.62 {
                    // dense moss patches
                    let t = smoothstep(0.62, 0.85, nm) * 0.55;
                    g = [g[0] * (1.0 - t * 0.45), g[1] * (1.0 - t * 0.1), g[2] * (1.0 - t * 0.35)];
                    mat = 6.0;
                }
                // dirt lane with worn edges + subtle checker
                if ld < 7.0 {
                    // A soft shoulder, and no checker. The lane used to be a
                    // hard `ld < 5.4` with a 3.2 u checkerboard darkening, both
                    // evaluated per VERTEX on a 1.6 u grid: that is the
                    // sawtooth along every road. The edge is now a smooth
                    // ramp the grid can carry, broken up per-pixel by noise in
                    // terrain.wgsl; the shader owns all fine pattern.
                    let t = 1.0 - smoothstep(2.6, 7.0, ld + (n1 - 0.5) * 1.8);
                    if t > 0.5 {
                        mat = 1.0;
                    }
                    let dirt = [0.42, 0.345, 0.26];
                    g = [g[0] + (dirt[0] - g[0]) * t, g[1] + (dirt[1] - g[1]) * t, g[2] + (dirt[2] - g[2]) * t];
                } else if ld < 8.5 {
                    let t = (1.0 - (ld - 7.0) / 1.5) * 0.25;
                    g = [g[0] * (1.0 - t * 0.4), g[1] * (1.0 - t * 0.25), g[2] * (1.0 - t * 0.5)];
                }
                // rivers: a pebbled bed under the water (the surface is its own
                // mesh, see build_water), sandy banks above the water line
                let wet_bed = h < crate::consts::WATER_LEVEL + 0.05 && rd < RIVER_HALF_WIDTH + 7.0;
                if wet_bed {
                    mat = 2.0;
                    let t = (1.0 - rd / (RIVER_HALF_WIDTH + 2.0)).clamp(0.0, 1.0);
                    g = [
                        0.30 + n2 * 0.06 - t * 0.08,
                        0.30 + n1 * 0.05 - t * 0.06,
                        0.24 + n2 * 0.04 - t * 0.02,
                    ];
                } else if rd < RIVER_HALF_WIDTH + 5.0 && ld > 5.4 {
                    mat = 3.0;
                    let t = (1.0 - (rd - RIVER_HALF_WIDTH) / 5.0).clamp(0.0, 1.0);
                    g = [g[0] + (0.46 - g[0]) * t * 0.85, g[1] + (0.40 - g[1]) * t * 0.85, g[2] + (0.27 - g[2]) * t * 0.85];
                }
                // The rim of the floor fades into the hill ring's darker
                // green. It was a hard circle, as visible from above as a
                // painted line; the arena edge is now a treeline, not a stripe.
                let edge = smoothstep(ARENA_RADIUS - 14.0, ARENA_RADIUS, r);
                if edge > 0.0 {
                    let hill = [0.075 + n1 * 0.035, 0.175 + n2 * 0.06, 0.07 + n2 * 0.02];
                    g = [g[0] + (hill[0] - g[0]) * edge, g[1] + (hill[1] - g[1]) * edge, g[2] + (hill[2] - g[2]) * edge];
                }
                // team base tints
                for (bz, c) in [
                    (crate::consts::Team::Blue.base_pos().z, [0.20, 0.34, 0.60]),
                    (crate::consts::Team::Red.base_pos().z, [0.60, 0.27, 0.23]),
                ] {
                    let d = ((x).powi(2) + (z - bz).powi(2)).sqrt();
                    if d < 18.0 {
                        let t = (1.0 - d / 18.0) * 0.45;
                        g = [g[0] + (c[0] - g[0]) * t, g[1] + (c[1] - g[1]) * t, g[2] + (c[2] - g[2]) * t];
                    }
                }
                color = g;
            }
            verts.push(TerrainVert {
                pos: [x, h, z],
                normal: n.to_array(),
                color,
                mat,
            });
        }
    }

    let mut idx = Vec::with_capacity((n_side - 1) * (n_side - 1) * 6);
    for iz in 0..n_side - 1 {
        for ix in 0..n_side - 1 {
            let a = (ix + n_side * iz) as u32;
            let b = (ix + 1 + n_side * iz) as u32;
            let c = (ix + n_side * (iz + 1)) as u32;
            let d = (ix + 1 + n_side * (iz + 1)) as u32;
            idx.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    (verts, idx)
}

// ---------------------------------------------------------------------------
// Water surface — two strips (one per river) at WATER_LEVEL, 1.0 u cells.
// Each vertex carries the bed height under it so the shader can shade by
// depth, and the local flow direction (river tangent, pointing south).
// Lanes carve the bed flat above the water line, so the surface simply ends
// up under the road there and the depth test hides it.
// ---------------------------------------------------------------------------

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct WaterVert {
    pos: [f32; 3],
    bed: f32,
    flow: [f32; 2],
    uv: [f32; 2],
}

/// Twinfang Pass ground: smooth grey stone underfoot, a little tough grass
/// in the clearings, dark wet rock on the cliff faces, paler weathered rock
/// on the plateau, snow on the heights and in the high hollows. Cold and
/// desaturated on purpose; the only saturated colour on this map is a cat.
fn pass_ground_color(x: f32, z: f32, h: f32, slope: f32, n1: f32, n2: f32, nm: f32) -> ([f32; 3], f32) {
    let d = crate::pass::floor().sdf(x, z);
    let lerp = |a: [f32; 3], b: [f32; 3], t: f32| [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t];
    if d <= 0.3 {
        // the floor
        let stone = [0.40 + n1 * 0.05, 0.41 + n1 * 0.05, 0.43 + n2 * 0.04];
        let rd = crate::pass::river_dist(x, z);
        if rd < crate::pass::STREAM_HALF_WIDTH + 1.2 {
            // stream bed: dark, water-polished pebble
            return ([0.24 + n2 * 0.05, 0.26 + n2 * 0.05, 0.28 + n1 * 0.04], 2.0);
        }
        // grass only away from the lanes, in the clearings' hearts, patchy
        let away = (crate::consts::lane_clearance(x, z) - 6.0).clamp(0.0, 6.0) / 6.0;
        let grass_t = (smoothstep(0.55, 0.78, nm) * away * smoothstep(-1.0, -3.5, d)).clamp(0.0, 1.0);
        let grass = [0.27 + n1 * 0.04, 0.33 + n2 * 0.05, 0.22];
        // a darker margin where the cliff meets the floor: scree and damp
        let foot = smoothstep(-2.5, 0.3, d) * 0.25;
        let c = lerp(stone, grass, grass_t * 0.85);
        return ([c[0] * (1.0 - foot), c[1] * (1.0 - foot), c[2] * (1.0 - foot * 0.8)], if grass_t > 0.5 { 0.0 } else { 5.0 });
    }
    let face = [0.25 + n1 * 0.04, 0.26 + n1 * 0.04, 0.29 + n2 * 0.04];
    let top = [0.38 + n1 * 0.06, 0.38 + n1 * 0.05, 0.39 + n2 * 0.05];
    let snow = [0.84 + n2 * 0.06, 0.86 + n2 * 0.06, 0.90 + n2 * 0.05];
    // faces are the steep parts; tops the flat ones
    let c = lerp(top, face, smoothstep(0.08, 0.35, slope));
    // snow: certain above the snowline, in patches on flat high ground below it
    let snowline = 44.0 + n1 * 10.0;
    let patchy = smoothstep(20.0, 30.0, h) * smoothstep(0.72, 0.88, nm) * (1.0 - smoothstep(0.1, 0.3, slope));
    let snow_t = smoothstep(snowline - 6.0, snowline + 4.0, h).max(patchy) * (1.0 - smoothstep(0.35, 0.6, slope) * 0.6);
    (lerp(c, snow, snow_t), if snow_t > 0.5 { 6.0 } else { 4.0 })
}

/// The pass's streams: a strip of water over each, 0.5 u cells, cut to the
/// wet part like the jungle's rivers. The lip-to-pool stretch is the
/// waterfall and is a prop, not a strip.
fn build_pass_water() -> (Vec<WaterVert>, Vec<u32>) {
    use crate::consts::WATER_LEVEL;
    const STEP: f32 = 0.5;
    let half_w = crate::pass::STREAM_HALF_WIDTH + 1.4;
    let mut verts = Vec::new();
    let mut idx = Vec::new();
    for s in crate::pass::streams() {
        for w in s[1..].windows(2) {
            let (a, b) = (w[0], w[1]);
            let along = b - a;
            let len = along.length();
            if len < 1e-3 {
                continue;
            }
            let dir = along / len;
            let side = Vec3::new(-dir.z, 0.0, dir.x);
            // overlap the ends a little so the joints have no gaps
            let (s0, s1) = (-half_w * 0.6, len + half_w * 0.6);
            let nl = ((s1 - s0) / STEP) as usize + 1;
            let nw = (half_w * 2.0 / STEP) as usize + 1;
            let base = verts.len() as u32;
            for il in 0..nl {
                for iw in 0..nw {
                    let p = a + dir * (s0 + il as f32 * STEP) + side * (-half_w + iw as f32 * STEP);
                    verts.push(WaterVert {
                        pos: [p.x, WATER_LEVEL, p.z],
                        bed: terrain_height(p.x, p.z),
                        flow: [dir.x, dir.z],
                        uv: [(iw as f32 * STEP - half_w) / half_w, (s0 + il as f32 * STEP) * 0.05],
                    });
                }
            }
            for il in 0..nl - 1 {
                for iw in 0..nw - 1 {
                    let i = |l: usize, w: usize| base + (w + nw * l) as u32;
                    let (qa, qb, qc, qd) = (i(il, iw), i(il, iw + 1), i(il + 1, iw), i(il + 1, iw + 1));
                    let dry = [qa, qb, qc, qd].iter().all(|&k| verts[k as usize].bed > WATER_LEVEL + 0.05);
                    if !dry {
                        idx.extend_from_slice(&[qa, qc, qb, qb, qc, qd]);
                    }
                }
            }
        }
    }
    (verts, idx)
}

fn build_water() -> (Vec<WaterVert>, Vec<u32>) {
    if crate::map::active() == crate::map::MapId::Pass {
        return build_pass_water();
    }
    use crate::consts::{river_x, GROUND_HALF, RIVER_HALF_WIDTH, WATER_LEVEL};
    const STEP: f32 = 1.0;
    let half_w = RIVER_HALF_WIDTH + 6.0;
    let nx = (half_w * 2.0 / STEP) as usize + 1;
    let nz = (GROUND_HALF * 2.0 / STEP) as usize + 1;
    let mut verts = Vec::with_capacity(nx * nz * 2);
    let mut idx = Vec::new();
    for side in [-1.0f32, 1.0] {
        let base = verts.len() as u32;
        for iz in 0..nz {
            let z = -GROUND_HALF + iz as f32 * STEP;
            let cx = river_x(side, z);
            // tangent of the centreline, pointing -z (downstream = south)
            let dz = 0.5;
            let tx = river_x(side, z - dz) - river_x(side, z + dz);
            let flow = glam::Vec2::new(tx, -2.0 * dz).normalize();
            for ix in 0..nx {
                let x = cx - half_w + ix as f32 * STEP;
                let bed = terrain_height(x, z);
                verts.push(WaterVert {
                    pos: [x, WATER_LEVEL, z],
                    bed,
                    flow: flow.to_array(),
                    uv: [(x - cx) / half_w, z * 0.05],
                });
            }
        }
        for iz in 0..nz - 1 {
            for ix in 0..nx - 1 {
                let a = base + (ix + nx * iz) as u32;
                let b = base + (ix + 1 + nx * iz) as u32;
                let c = base + (ix + nx * (iz + 1)) as u32;
                let d = base + (ix + 1 + nx * (iz + 1)) as u32;
                // skip cells that are entirely dry (bed above water) to save fill
                let dry = [a, b, c, d].iter().all(|&i| verts[i as usize].bed > WATER_LEVEL + 0.15);
                if !dry {
                    idx.extend_from_slice(&[a, c, b, b, c, d]);
                }
            }
        }
    }
    (verts, idx)
}

/// Flood sheet: the playable floor at WATER_LEVEL (the shader lifts it by
/// the flood rise), 2 u cells, skipping the river strips the normal water
/// mesh already covers.
fn build_flood_plane() -> (Vec<WaterVert>, Vec<u32>) {
    use crate::consts::{river_dist, ARENA_RADIUS, RIVER_HALF_WIDTH, WATER_LEVEL};
    const STEP: f32 = 2.0;
    let half = crate::consts::arena_radius() + 10.0;
    let _ = ARENA_RADIUS;
    let n = (half * 2.0 / STEP) as usize + 1;
    let mut verts = Vec::with_capacity(n * n);
    let mut idx = Vec::new();
    for iz in 0..n {
        let z = -half + iz as f32 * STEP;
        for ix in 0..n {
            let x = -half + ix as f32 * STEP;
            verts.push(WaterVert { pos: [x, WATER_LEVEL, z], bed: terrain_height(x, z), flow: [0.0, -1.0], uv: [x * 0.05, z * 0.05] });
        }
    }
    for iz in 0..n - 1 {
        for ix in 0..n - 1 {
            let a = (ix + n * iz) as u32;
            let b = (ix + 1 + n * iz) as u32;
            let c = (ix + n * (iz + 1)) as u32;
            let d = (ix + 1 + n * (iz + 1)) as u32;
            let v = &verts[a as usize];
            let r = (v.pos[0] * v.pos[0] + v.pos[2] * v.pos[2]).sqrt();
            if r > half - 2.0 || river_dist(v.pos[0], v.pos[2]) < RIVER_HALF_WIDTH + 5.0 {
                continue;
            }
            idx.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    (verts, idx)
}

#[cfg(test)]
mod depth_tests {
    use super::*;

    /// The river painted over canopies twice (see `depth_state`). This is the
    /// check that was missing both times: no GPU needed, so it runs anywhere.
    #[test]
    fn blended_draws_are_depth_tested() {
        let d = depth_state(DepthUse::Blended);
        assert_ne!(d.depth_compare, wgpu::CompareFunction::Always, "water would ignore the depth buffer");
        assert!(!d.depth_write_enabled, "a blended surface must not occlude what is drawn after it");
        // The sky shares this state and sits at exactly the clear depth.
        assert_eq!(d.depth_compare, wgpu::CompareFunction::LessEqual, "strict Less rejects the sky at depth 1.0");
    }

    #[test]
    fn opaque_draws_write_depth() {
        let d = depth_state(DepthUse::Opaque);
        assert!(d.depth_write_enabled);
        assert_eq!(d.depth_compare, wgpu::CompareFunction::Less);
    }

    /// Both directions are failures here: near-to-far blends the far surface
    /// over the near one.
    #[test]
    fn blended_draws_sort_far_to_near() {
        let mut v = vec![(1.0, 'n'), (9.0, 'f'), (4.0, 'm'), (f32::NAN, 'x')];
        sort_far_to_near(&mut v);
        let order: Vec<char> = v.iter().filter(|e| e.1 != 'x').map(|e| e.1).collect();
        assert_eq!(order, vec!['f', 'm', 'n']);
    }
}
