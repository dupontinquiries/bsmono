// wgpu renderer: instanced microvoxels with shadow mapping, terrain,
// sky, HDR + ACES post. Shared cube geometry; per-model instance buffers.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3, Vec4};
use wgpu::util::DeviceExt;

use crate::voxel::{CUBE_IDX, CUBE_NORMS, CUBE_VERTS, VoxInst, VoxelMesh};

pub const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const SHADOW_SIZE: u32 = 2048;
pub const MAX_DRAWS: usize = 512;
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
    /// xyz wind (world u/s), w unused
    pub wind: [f32; 4],
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

pub struct GpuMesh {
    buf: wgpu::Buffer,
    count: u32,
}

// --- renderer -------------------------------------------------------------------

pub struct Renderer {
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
    camera: Camera,
    time: f32,
    pub exposure: f32,
    /// Sky state for this frame — set by the caller each frame (see weather.rs).
    pub atmosphere: crate::weather::Atmosphere,
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

        let depth_state_write = wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: Default::default(),
            bias: Default::default(),
        };
        let depth_state_ro = wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Always,
            stencil: Default::default(),
            bias: Default::default(),
        };

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

        let terrain_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("terrain"),
            layout: Some(&main_pl),
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

        let post_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let post_params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post params"),
            size: 48,
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
            post_sampler,
            post_params,
            hdr_msaa: None,
            hdr_resolve: None,
            depth: None,
            post_bind: None,
            opaque: Vec::new(),
            transparent: Vec::new(),
            draws: Vec::new(),
            particles: Vec::new(),
            camera: Camera { eye: Vec3::ZERO, target: Vec3::Y, fov_y: 1.05 },
            time: 0.0,
            exposure: 0.92,
            atmosphere: crate::weather::Weather::new().atmosphere(),
        };
        r.resize(width, height);
        r
    }

    pub fn upload_mesh(&mut self, mesh: &VoxelMesh) -> u64 {
        let id = self.next_mesh_id;
        self.next_mesh_id += 1;
        let buf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("voxel instances"),
            contents: bytemuck::cast_slice(&mesh.inst),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        self.meshes.insert(id, GpuMesh { buf, count: mesh.inst.len() as u32 });
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
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth_tex.create_view(&Default::default());
        self.hdr_msaa = Some((msaa_tex, msaa_view));
        self.hdr_resolve = Some((resolve_tex, resolve_view));
        self.depth = Some((depth_tex, depth_view));
        if let Some((_, rview)) = &self.hdr_resolve {
            self.post_bind = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("post bind"),
                layout: &self.post_bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(rview) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.post_sampler) },
                    wgpu::BindGroupEntry { binding: 2, resource: self.post_params.as_entire_binding() },
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
    pub fn clear_queues(&mut self) {
        self.opaque.clear();
        self.transparent.clear();
        self.draws.clear();
    }

    pub fn queue_draw(&mut self, mesh: u64, model: Mat4, tint: [f32; 4], emissive: f32, voxel: f32, alpha: bool) {
        if self.draws.len() >= MAX_DRAWS {
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
        let aspect = self.width as f32 / self.height as f32;
        let vp = cam.view_proj(aspect);
        let inv = vp.inverse();

        // shadow ortho covering the arena + inner jungle ring; the sun moves
        // with the weather (night sinks it), so the light matrix follows.
        let atm = self.atmosphere;
        let sun = atm.sun_dir.normalize();
        let light_view = Mat4::look_at_rh(sun * 130.0 + Vec3::new(0.0, 0.0, 0.0), Vec3::ZERO, Vec3::Y);
        let light_proj = Mat4::orthographic_rh(-150.0, 150.0, -150.0, 150.0, 1.0, 320.0);
        let lvp = light_proj * light_view;

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
                bytemuck::cast_slice(&[
                    self.exposure * (1.0 + atm.night * 0.35),
                    0.55 + atm.night * 0.2,
                    time,
                    atm.lightning,
                    0.55 + atm.night * 0.3,
                    atm.night,
                    atm.rain,
                    0.0f32,
                    sun_uv[0],
                    sun_uv[1],
                    ray,
                    0.22,
                ])
            },
        );

        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("frame"),
        });

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
            for (mesh_id, di) in &self.opaque {
                if let Some(m) = self.meshes.get(mesh_id) {
                    sp.set_bind_group(0, &self.shadow_bind, &[(*di as u32) * DYN_ALIGN as u32]);
                    sp.set_vertex_buffer(1, m.buf.slice(..));
                    sp.draw_indexed(0..36, 0, 0..m.count);
                }
            }
        }

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

            // water surface (after the opaque world so the bed shows through)
            if self.water_count > 0 {
                rp.set_pipeline(&self.water_pipe);
                rp.set_bind_group(0, &self.main_bind, &[0]);
                rp.set_vertex_buffer(0, self.water_vbuf.slice(..));
                rp.set_index_buffer(self.water_ibuf.slice(..), wgpu::IndexFormat::Uint32);
                rp.draw_indexed(0..self.water_count, 0, 0..1);
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

/// Shared terrain height field: flat playable floor, rolling jungle hills
/// beyond the arena bounds, a mountain rim at the horizon, and river valleys
/// that dip under the lanes (roads stay solid — bridges span the crossings).
pub fn terrain_height(x: f32, z: f32) -> f32 {
    use crate::consts::{river_dist, GROUND_HALF, RIVER_HALF_WIDTH, RIVER_DEPTH, ARENA_RADIUS};
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
    if r > 172.0 {
        let t = ((r - 172.0) / (GROUND_HALF - 172.0)).clamp(0.0, 1.0);
        h += t * t * 30.0 + vnoise(x * 0.05 + 13.0, z * 0.05) * t * 10.0;
    }
    // river valley (masked under lanes so the roads stay solid)
    let rd = river_dist(x, z);
    let river_reach = RIVER_HALF_WIDTH + 7.0;
    if rd < river_reach {
        let ld = lane_distance(x, z);
        let mask = smoothstep(4.0, 9.5, ld);
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
    let planes = [r3 + r0, r3 - r0, r3 + r1, r3 - r1, r3 + r2, r3 - r2];
    planes
        .iter()
        .all(|p| p.x * center.x + p.y * center.y + p.z * center.z + p.w > -radius)
}

fn build_terrain() -> (Vec<TerrainVert>, Vec<u32>) {
    use crate::consts::{river_dist, GROUND_HALF, RIVER_HALF_WIDTH, ARENA_RADIUS};
    const HALF: f32 = GROUND_HALF;
    // 1.6 u grid (was 3.2): ~80k verts, fine enough for the river channels
    // and lane edges to read; the fragment shader adds the sub-metre detail.
    const STEP: f32 = 1.6;
    const N: usize = ((HALF * 2.0) / STEP) as usize + 1;

    let mut verts = Vec::with_capacity(N * N);
    for iz in 0..N {
        for ix in 0..N {
            let x = -HALF + ix as f32 * STEP;
            let z = -HALF + iz as f32 * STEP;
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
            if r > 175.0 && h > 14.0 {
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
                if ld < 5.4 {
                    mat = 1.0;
                    let t = (1.0 - ld / 5.4).clamp(0.0, 1.0);
                    let dirt = [0.42, 0.345, 0.26];
                    g = [g[0] + (dirt[0] - g[0]) * t, g[1] + (dirt[1] - g[1]) * t, g[2] + (dirt[2] - g[2]) * t];
                    if ((x / 3.2).floor() + (z / 3.2).floor()) as i32 % 2 == 0 {
                        g = [g[0] * 0.93, g[1] * 0.93, g[2] * 0.93];
                    }
                } else if ld < 7.0 {
                    let t = (1.0 - (ld - 5.4) / 1.6) * 0.25;
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

    let mut idx = Vec::with_capacity((N - 1) * (N - 1) * 6);
    for iz in 0..N - 1 {
        for ix in 0..N - 1 {
            let a = (ix + N * iz) as u32;
            let b = (ix + 1 + N * iz) as u32;
            let c = (ix + N * (iz + 1)) as u32;
            let d = (ix + 1 + N * (iz + 1)) as u32;
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

fn build_water() -> (Vec<WaterVert>, Vec<u32>) {
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
