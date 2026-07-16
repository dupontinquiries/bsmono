//! gpu.rs — an optional GPU renderer (wgpu + winit), an alternative to the software painter in render.rs.
//! Adapted from the clawbound wgpu renderer, extended so the material atlas is sampled on-GPU (clawbound is
//! vertex-colour only). Per frame it pulls the game's 3D `frame_meshes`, flattens them to GPU vertices
//! (position, normal, colour, atlas-UV, material slot), transforms with a view-projection matrix in the
//! vertex shader, and samples `atlas × vertex-colour` in the fragment shader with per-pixel tiling so floors,
//! walls, brick, stone, etc. read the same as the software path.
//!
//! Build/run:  cargo run --release --features gpu --bin gpu
//!
//! VERIFICATION NOTE: this file is written to compile; it needs an actual GPU + display to run, which the
//! authoring environment does not have. Treat first-run behaviour (and the look) as unverified until tried.
#![cfg(feature = "gpu")]

use crate::camera::FpsCam;
use crate::game::Game;
use crate::gpu_menu::{self, MenuAction, MenuScreen};
use crate::loadout::Loadout;
use crate::math::Vec3;
use crate::mesh::{atlas_slot, build_material_atlas, Mesh3, ATLAS_COLS};
use crate::player::Input;
use std::sync::Arc;
use std::time::Instant;
use wgpu::util::DeviceExt;
use winit::{
    event::{DeviceEvent, ElementState, Event, MouseButton, WindowEvent},
    event_loop::EventLoop,
    keyboard::{KeyCode, PhysicalKey},
    window::{Window, WindowBuilder},
};

// ── GPU vertex ────────────────────────────────────────────────────────────────────────────────────────
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vert {
    pos: [f32; 3],
    nrm: [f32; 3],
    col: [f32; 4], // rgb + opacity
    uv: [f32; 2],
    slot: f32, // atlas slot (0 = white tile → untextured)
    emis: f32, // emissive strength (glow: lava, crystals, tracers, markers)
    reflect: f32, // environment reflectivity (water, ice, metal, car paint)
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct U {
    viewproj: [[f32; 4]; 4],
    light: [f32; 4],  // xyz dir, w unused
    key: [f32; 4],    // key light colour
    amb: [f32; 4],    // ambient colour
    fog: [f32; 4],    // rgb fog colour, w unused
    params: [f32; 4], // x = atlas cols, y = fog near, z = fog far, w = point-light count
    cam: [f32; 4],    // xyz camera position (for view-dependent reflection/fresnel)
    sky: [f32; 4],    // sky environment colour
    ground: [f32; 4], // ground environment colour
    cr: [f32; 4],     // camera right (xyz), w = tan(fovy/2)
    cu: [f32; 4],     // camera up (xyz)
    cf: [f32; 4],     // camera forward (xyz)
    hud: [f32; 4],    // x = health frac, y = ammo frac, z = low-health flag
    // Dynamic point lights, packed as pairs: [pos.xyz, radius], [col.rgb, intensity]. Up to MAX_PLIGHTS.
    plights: [[f32; 4]; MAX_PLIGHTS * 2],
}

const MAX_PLIGHTS: usize = 16;

/// Flatten meshes into three vertex lists for correct draw order:
///   `solid`   — single-sided opaque, backface-culled, writes depth (walls, most geometry)
///   `opaque`  — double-sided opaque (foliage, particles), no cull, writes depth
///   `trans`   — translucent (water, glass), no cull, NO depth write, sorted back-to-front so they blend
///               in the right order over everything already drawn.
fn flatten(meshes: &[Mesh3], cam: Vec3) -> (Vec<Vert>, Vec<Vert>, Vec<Vert>) {
    let mut solid = Vec::new();
    let mut opaque = Vec::new();
    // translucent meshes are gathered with a camera distance, then sorted far→near before flattening
    let mut trans_meshes: Vec<(f32, &Mesh3)> = Vec::new();
    let push_mesh = |m: &Mesh3, dst: &mut Vec<Vert>| {
        let slot = m.material.map(atlas_slot).unwrap_or(0) as f32;
        let emis = m.emissive;
        let reflect = m.reflect;
        let base = m.color;
        for tri in &m.tris {
            for &idx in tri {
                let i = idx as usize;
                let p = m.verts[i];
                let n = m.norms.get(i).copied().unwrap_or(Vec3::new(0.0, 1.0, 0.0));
                let c = m.vcol.as_ref().and_then(|vc| vc.get(i)).copied().unwrap_or(base);
                let uv = m.uv.get(i).copied().unwrap_or(egui::Pos2::new(0.5, 0.5));
                let op = c.a() as f32 / 255.0;
                dst.push(Vert {
                    pos: [p.x, p.y, p.z],
                    nrm: [n.x, n.y, n.z],
                    col: [c.r() as f32 / 255.0, c.g() as f32 / 255.0, c.b() as f32 / 255.0, op],
                    uv: [uv.x, uv.y],
                    slot,
                    emis,
                    reflect,
                });
            }
        }
    };
    for m in meshes {
        let translucent = m.color.a() < 250 && !m.single_sided;
        if translucent {
            // centroid distance from the camera for back-to-front sorting
            let mut c = Vec3::new(0.0, 0.0, 0.0);
            if !m.verts.is_empty() {
                for v in &m.verts {
                    c = c + *v;
                }
                c = c * (1.0 / m.verts.len() as f32);
            }
            let d = (c.x - cam.x).powi(2) + (c.y - cam.y).powi(2) + (c.z - cam.z).powi(2);
            trans_meshes.push((d, m));
        } else if m.single_sided {
            push_mesh(m, &mut solid);
        } else {
            push_mesh(m, &mut opaque);
        }
    }
    trans_meshes.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)); // far → near
    let mut trans = Vec::new();
    for (_, m) in trans_meshes {
        push_mesh(m, &mut trans);
    }
    (solid, opaque, trans)
}

// ── column-major 4x4 matrix helpers (wgpu NDC: z in 0..1) ───────────────────────────────────────────────
type M4 = [[f32; 4]; 4];
fn mul(a: &M4, b: &M4) -> M4 {
    let mut o = [[0.0f32; 4]; 4];
    for c in 0..4 {
        for r in 0..4 {
            o[c][r] = a[0][r] * b[c][0] + a[1][r] * b[c][1] + a[2][r] * b[c][2] + a[3][r] * b[c][3];
        }
    }
    o
}
fn perspective(fovy: f32, aspect: f32, near: f32, far: f32) -> M4 {
    let f = 1.0 / (fovy * 0.5).tan();
    let mut m = [[0.0f32; 4]; 4];
    m[0][0] = f / aspect;
    m[1][1] = f;
    m[2][2] = far / (near - far);
    m[2][3] = -1.0;
    m[3][2] = near * far / (near - far);
    m
}
fn look_at(eye: Vec3, fwd: Vec3) -> M4 {
    let f = fwd.normalized();
    let up = Vec3::new(0.0, 1.0, 0.0);
    let s = f.cross(up).normalized();
    let u = s.cross(f);
    [
        [s.x, u.x, -f.x, 0.0],
        [s.y, u.y, -f.y, 0.0],
        [s.z, u.z, -f.z, 0.0],
        [-s.dot(eye), -u.dot(eye), f.dot(eye), 1.0],
    ]
}
fn cam_forward(cam: &FpsCam) -> Vec3 {
    // early-fps: yaw = 0 looks toward −Z.
    Vec3::new(cam.yaw.sin() * cam.pitch.cos(), cam.pitch.sin(), -cam.yaw.cos() * cam.pitch.cos())
}

// ── wgpu state ──────────────────────────────────────────────────────────────────────────────────────────
struct State {
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    pipe_solid: wgpu::RenderPipeline,
    pipe_double: wgpu::RenderPipeline,
    pipe_trans: wgpu::RenderPipeline,
    pipe_cross: wgpu::RenderPipeline,
    pipe_sky: wgpu::RenderPipeline,
    ubuf: wgpu::Buffer,
    bind: wgpu::BindGroup,
    vbuf_solid: wgpu::Buffer,
    vcap_solid: u64,
    vcount_solid: u32,
    vbuf_double: wgpu::Buffer,
    vcap_double: u64,
    vcount_double: u32,
    vbuf_trans: wgpu::Buffer,
    vcap_trans: u64,
    vcount_trans: u32,
    depth: wgpu::TextureView,
    msaa: wgpu::TextureView,
    scene_view: wgpu::TextureView,
    post_bgl: wgpu::BindGroupLayout,
    post_samp: wgpu::Sampler,
    post_bind: wgpu::BindGroup,
    post_ubuf: wgpu::Buffer,
    pipe_post: wgpu::RenderPipeline,
    format: wgpu::TextureFormat,
    egui: crate::gpu_egui::EguiOverlay,
    scale: f32,
}

const SAMPLES: u32 = 4; // MSAA sample count

fn make_depth(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: SAMPLES,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

/// The multisampled colour target the scene renders into; it's resolved down to the (single-sample) surface.
fn make_msaa(device: &wgpu::Device, format: wgpu::TextureFormat, w: u32, h: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("msaa"),
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: SAMPLES,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

/// Box-downsample an RGBA image into a concatenated mip chain (level 0 = full res, each level half-size).
/// Used to give the atlas mipmaps so distant textures don't shimmer.
fn build_mips(base: &[u8], size: usize, levels: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(base.len() * 2);
    out.extend_from_slice(base);
    let mut prev = base.to_vec();
    let mut psize = size;
    for _ in 1..levels {
        let nsize = (psize / 2).max(1);
        let mut next = vec![0u8; nsize * nsize * 4];
        for y in 0..nsize {
            for x in 0..nsize {
                let x0 = x * 2;
                let y0 = y * 2;
                let x1 = (x0 + 1).min(psize - 1);
                let y1 = (y0 + 1).min(psize - 1);
                for c in 0..4 {
                    let s = prev[(y0 * psize + x0) * 4 + c] as u32
                        + prev[(y0 * psize + x1) * 4 + c] as u32
                        + prev[(y1 * psize + x0) * 4 + c] as u32
                        + prev[(y1 * psize + x1) * 4 + c] as u32;
                    next[(y * nsize + x) * 4 + c] = (s / 4) as u8;
                }
            }
        }
        out.extend_from_slice(&next);
        prev = next;
        psize = nsize;
    }
    out
}

/// The single-sample offscreen texture the MSAA scene resolves into; the post pass samples it to the surface.
fn make_scene(device: &wgpu::Device, format: wgpu::TextureFormat, w: u32, h: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("scene"),
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

/// Choose the swapchain present mode via the shared gpu-kit policy instead of blindly taking
/// `caps.present_modes[0]` (whose order isn't guaranteed — it can hand back Mailbox/Immediate and
/// stutter on Wayland). Default intent is Vsync (Fifo, smoothest); `SOLDIER_VSYNC` overrides it,
/// matching unendlich's knob so the fleet's games behave the same:
///   1|on|fifo → Vsync · mailbox → LowLatency · immediate|tearing|0 → Fast
fn choose_present_mode(supported: &[wgpu::PresentMode]) -> wgpu::PresentMode {
    fn to_gk(m: wgpu::PresentMode) -> Option<gpu_kit::PresentMode> {
        match m {
            wgpu::PresentMode::Fifo => Some(gpu_kit::PresentMode::Fifo),
            wgpu::PresentMode::FifoRelaxed => Some(gpu_kit::PresentMode::FifoRelaxed),
            wgpu::PresentMode::Mailbox => Some(gpu_kit::PresentMode::Mailbox),
            wgpu::PresentMode::Immediate => Some(gpu_kit::PresentMode::Immediate),
            _ => None, // AutoVsync/AutoNoVsync are request hints, not surface caps
        }
    }
    fn from_gk(m: gpu_kit::PresentMode) -> wgpu::PresentMode {
        match m {
            gpu_kit::PresentMode::Fifo => wgpu::PresentMode::Fifo,
            gpu_kit::PresentMode::FifoRelaxed => wgpu::PresentMode::FifoRelaxed,
            gpu_kit::PresentMode::Mailbox => wgpu::PresentMode::Mailbox,
            gpu_kit::PresentMode::Immediate => wgpu::PresentMode::Immediate,
        }
    }
    let present = match std::env::var("SOLDIER_VSYNC").ok().as_deref() {
        Some("1") | Some("on") | Some("fifo") => gpu_kit::Present::Vsync,
        Some("mailbox") => gpu_kit::Present::LowLatency,
        Some("immediate") | Some("tearing") | Some("0") => gpu_kit::Present::Fast,
        _ => gpu_kit::Present::Vsync,
    };
    let gk: Vec<gpu_kit::PresentMode> = supported.iter().copied().filter_map(to_gk).collect();
    from_gk(gpu_kit::Policy { present, ..gpu_kit::Policy::default() }.pick_present_mode(&gk))
}

impl State {
    async fn new(window: Arc<Window>) -> State {
        let size = window.inner_size();
        let instance = wgpu::Instance::default();
        let surface = instance.create_surface(window.clone()).unwrap();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("no GPU adapter");
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor { label: None, required_features: wgpu::Features::empty(), required_limits: adapter.limits(), memory_hints: Default::default() }, None)
            .await
            .expect("no device");

        let caps = surface.get_capabilities(&adapter);
        let format = caps.formats.iter().copied().find(|f| f.is_srgb()).unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: choose_present_mode(&caps.present_modes),
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // ── material atlas → GPU texture (with mipmaps for distance stability) ──
        let tile = 512usize;
        let (atlas_bytes, asz, _) = build_material_atlas(tile, ATLAS_COLS);
        // Cap mip levels so the coarsest still has enough pixels per cell to limit cross-cell bleeding.
        let mut levels = 1u32;
        while (asz >> levels) >= 32 && levels < 6 {
            levels += 1;
        }
        let mip_data = build_mips(&atlas_bytes, asz, levels as usize);
        let atlas_tex = device.create_texture_with_data(
            &queue,
            &wgpu::TextureDescriptor {
                label: Some("atlas"),
                size: wgpu::Extent3d { width: asz as u32, height: asz as u32, depth_or_array_layers: 1 },
                mip_level_count: levels,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::MipMajor,
            &mip_data,
        );
        let atlas_view = atlas_tex.create_view(&Default::default());
        // trilinear + anisotropic filtering (needs the mip chain above)
        let samp = device.create_sampler(&wgpu::SamplerDescriptor {
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            anisotropy_clamp: 8,
            ..Default::default()
        });

        let ubuf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("u"), size: std::mem::size_of::<U>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&atlas_view) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&samp) },
            ],
        });

        let sh = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: None, source: wgpu::ShaderSource::Wgsl(SCENE_WGSL.into()) });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[&bgl], push_constant_ranges: &[] });
        let attrs = [
            wgpu::VertexAttribute { offset: 0, shader_location: 0, format: wgpu::VertexFormat::Float32x3 },
            wgpu::VertexAttribute { offset: 12, shader_location: 1, format: wgpu::VertexFormat::Float32x3 },
            wgpu::VertexAttribute { offset: 24, shader_location: 2, format: wgpu::VertexFormat::Float32x4 },
            wgpu::VertexAttribute { offset: 40, shader_location: 3, format: wgpu::VertexFormat::Float32x2 },
            wgpu::VertexAttribute { offset: 48, shader_location: 4, format: wgpu::VertexFormat::Float32 },
            wgpu::VertexAttribute { offset: 52, shader_location: 5, format: wgpu::VertexFormat::Float32 },
            wgpu::VertexAttribute { offset: 56, shader_location: 6, format: wgpu::VertexFormat::Float32 },
        ];
        // NOTE: front_face defaults to Ccw here. If single-sided surfaces (walls) render see-through or
        // inside-out on a real GPU, the winding is the other way — flip this to `Cw` for the solid pipeline.
        let mk_pipe = |cull: Option<wgpu::Face>, depth_write: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: None,
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &sh,
                    entry_point: "vs",
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Vert>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &attrs }],
                },
                fragment: Some(wgpu::FragmentState { module: &sh, entry_point: "fs", compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })] }),
                primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, front_face: wgpu::FrontFace::Ccw, cull_mode: cull, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: depth_write, depth_compare: wgpu::CompareFunction::Less, stencil: Default::default(), bias: Default::default() }),
                multisample: wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
                multiview: None,
                cache: None,
            })
        };
        let pipe_solid = mk_pipe(Some(wgpu::Face::Back), true);
        let pipe_double = mk_pipe(None, true);
        let pipe_trans = mk_pipe(None, false); // translucent: test depth but don't write it

        // Sky gradient: a fullscreen triangle drawn first inside the scene pass. It sits at the far plane
        // (depth test Always, no depth write) so scene geometry always draws over it.
        let pipe_sky = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky"),
            layout: Some(&layout),
            vertex: wgpu::VertexState { module: &sh, entry_point: "vs_sky", compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &sh, entry_point: "fs_sky", compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: false, depth_compare: wgpu::CompareFunction::Always, stencil: Default::default(), bias: Default::default() }),
            multisample: wgpu::MultisampleState { count: SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
            multiview: None,
            cache: None,
        });

        // Crosshair overlay: a tiny screen-space pipeline drawn after the scene resolves, straight onto the
        // single-sample surface (so no MSAA, no depth, alpha-blended). Generates its own verts from the index.
        let pipe_cross = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("crosshair"),
            layout: Some(&layout),
            vertex: wgpu::VertexState { module: &sh, entry_point: "vs_overlay", compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &sh, entry_point: "fs_overlay", compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: Some(wgpu::BlendState::ALPHA_BLENDING), write_mask: wgpu::ColorWrites::ALL })] }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        let vcap = 64 * 1024 * std::mem::size_of::<Vert>() as u64;
        let mk_vbuf = || device.create_buffer(&wgpu::BufferDescriptor { label: Some("v"), size: vcap, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let vbuf_solid = mk_vbuf();
        let vbuf_double = mk_vbuf();
        let vbuf_trans = mk_vbuf();
        let depth = make_depth(&device, config.width, config.height);
        let msaa = make_msaa(&device, format, config.width, config.height);
        let scene_view = make_scene(&device, format, config.width, config.height);

        // Post-processing pass: samples the resolved scene and applies tonemap + vignette + a cheap bloom.
        let post_samp = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, address_mode_u: wgpu::AddressMode::ClampToEdge, address_mode_v: wgpu::AddressMode::ClampToEdge, ..Default::default() });
        let post_ubuf = device.create_buffer(&wgpu::BufferDescriptor { label: Some("postu"), size: 16, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let post_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let post_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post"),
            layout: &post_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&scene_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&post_samp) },
                wgpu::BindGroupEntry { binding: 2, resource: post_ubuf.as_entire_binding() },
            ],
        });
        let post_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some("post"), bind_group_layouts: &[&post_bgl], push_constant_ranges: &[] });
        let post_sh = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("post"), source: wgpu::ShaderSource::Wgsl(POST_WGSL.into()) });
        let pipe_post = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("post"),
            layout: Some(&post_layout),
            vertex: wgpu::VertexState { module: &post_sh, entry_point: "vs_post", compilation_options: Default::default(), buffers: &[] },
            fragment: Some(wgpu::FragmentState { module: &post_sh, entry_point: "fs_post", compilation_options: Default::default(), targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })] }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
            depth_stencil: None,
            multisample: Default::default(),
            multiview: None,
            cache: None,
        });

        let egui = crate::gpu_egui::EguiOverlay::new(&device, format);
        let scale = window.scale_factor() as f32;
        State {
            surface,
            device,
            queue,
            config,
            pipe_solid,
            pipe_double,
            pipe_trans,
            pipe_cross,
            pipe_sky,
            ubuf,
            bind,
            vbuf_solid,
            vcap_solid: vcap,
            vcount_solid: 0,
            vbuf_double,
            vcap_double: vcap,
            vcount_double: 0,
            vbuf_trans,
            vcap_trans: vcap,
            vcount_trans: 0,
            depth,
            msaa,
            scene_view,
            post_bgl,
            post_samp,
            post_bind,
            post_ubuf,
            pipe_post,
            format,
            egui,
            scale,
        }
    }

    fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        self.surface.configure(&self.device, &self.config);
        self.depth = make_depth(&self.device, w, h);
        self.msaa = make_msaa(&self.device, self.format, w, h);
        self.scene_view = make_scene(&self.device, self.format, w, h);
        self.post_bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post"),
            layout: &self.post_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&self.scene_view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.post_samp) },
                wgpu::BindGroupEntry { binding: 2, resource: self.post_ubuf.as_entire_binding() },
            ],
        });
    }

    fn upload(&mut self, solid: &[Vert], opaque: &[Vert], trans: &[Vert]) {
        fn up(device: &wgpu::Device, queue: &wgpu::Queue, buf: &mut wgpu::Buffer, cap: &mut u64, cnt: &mut u32, verts: &[Vert], label: &'static str) {
            let bytes: &[u8] = bytemuck::cast_slice(verts);
            if bytes.len() as u64 > *cap {
                *cap = (bytes.len() as u64).next_power_of_two();
                *buf = device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: *cap, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            }
            if !bytes.is_empty() {
                queue.write_buffer(buf, 0, bytes);
            }
            *cnt = verts.len() as u32;
        }
        up(&self.device, &self.queue, &mut self.vbuf_solid, &mut self.vcap_solid, &mut self.vcount_solid, solid, "vs");
        up(&self.device, &self.queue, &mut self.vbuf_double, &mut self.vcap_double, &mut self.vcount_double, opaque, "vd");
        up(&self.device, &self.queue, &mut self.vbuf_trans, &mut self.vcap_trans, &mut self.vcount_trans, trans, "vt");
    }

    fn render(&mut self, u: &U, game: &crate::game::Game, menu: &mut Option<MenuScreen>, loadout: &mut Loadout, menu_events: Vec<egui::Event>) -> MenuAction {
        let mut action = MenuAction::None;
        self.queue.write_buffer(&self.ubuf, 0, bytemuck::bytes_of(u));
        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(_) => {
                self.surface.configure(&self.device, &self.config);
                return action;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let fog = u.fog;
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &self.msaa, resolve_target: Some(&self.scene_view), ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: fog[0] as f64, g: fog[1] as f64, b: fog[2] as f64, a: 1.0 }), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &self.depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rp.set_bind_group(0, &self.bind, &[]);
            // 0) sky gradient (fills the background at the far plane)
            rp.set_pipeline(&self.pipe_sky);
            rp.draw(0..3, 0..1);
            // 1) solid geometry, backface-culled, writes depth
            if self.vcount_solid > 0 {
                rp.set_pipeline(&self.pipe_solid);
                rp.set_vertex_buffer(0, self.vbuf_solid.slice(..));
                rp.draw(0..self.vcount_solid, 0..1);
            }
            // 2) opaque double-sided (foliage/particles), writes depth
            if self.vcount_double > 0 {
                rp.set_pipeline(&self.pipe_double);
                rp.set_vertex_buffer(0, self.vbuf_double.slice(..));
                rp.draw(0..self.vcount_double, 0..1);
            }
            // 3) translucent (water/glass), sorted back-to-front, tests depth but doesn't write it
            if self.vcount_trans > 0 {
                rp.set_pipeline(&self.pipe_trans);
                rp.set_vertex_buffer(0, self.vbuf_trans.slice(..));
                rp.draw(0..self.vcount_trans, 0..1);
            }
        }
        // post-processing pass: tonemap + vignette + cheap bloom, offscreen scene → surface
        {
            // feed the damage-vignette params: [low-health flag, time, health frac, 0]
            self.queue.write_buffer(&self.post_ubuf, 0, bytemuck::cast_slice(&[u.hud[2], u.fog[3], u.hud[0], 0.0f32]));
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("post"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rp.set_pipeline(&self.pipe_post);
            rp.set_bind_group(0, &self.post_bind, &[]);
            rp.draw(0..3, 0..1);
        }
        // crosshair + HUD overlay pass: draw onto the post-processed surface (load, keep it)
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("overlay"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rp.set_pipeline(&self.pipe_cross);
            rp.set_bind_group(0, &self.bind, &[]);
            rp.draw(0..36, 0..1); // crosshair (2) + health bg/fill + ammo bg/fill
        }
        // egui HUD overlay: run the shared `crate::hud` painters through a headless egui Context and
        // paint the tessellated meshes on top of the finished frame. No winit 0.30 rewrite and no
        // egui-winit/egui-wgpu — just egui 0.34 (already a dep) + a small 2-D wgpu pass (gpu_egui).
        {
            let w = self.config.width;
            let h = self.config.height;
            let scale = self.scale;
            self.egui.prepare(&self.device, &self.queue, w, h, scale, menu_events, |ctx| {
                if let Some(screen) = menu {
                    action = gpu_menu::ui(ctx, screen, loadout);
                    return;
                }
                let painter = ctx.layer_painter(egui::LayerId::new(
                    egui::Order::Foreground,
                    egui::Id::new("gpu_hud"),
                ));
                let rect = ctx.content_rect();
                // the game-only subset of the shared HUD (GPU path carries no app Settings/build state)
                crate::hud::draw_damage_vignette(&painter, rect, game);
                crate::hud::draw_hit_flash(&painter, rect, game);
                crate::hud::draw_underwater_tint(&painter, rect, game);
                crate::hud::draw_reticle(&painter, rect, game, 0.0);
                crate::hud::draw_hitmarker(&painter, rect, game);
                crate::hud::draw_damage_arrows(&painter, rect, game);
                crate::hud::draw_hud(&painter, rect, game);
                crate::hud::draw_scorestreaks(&painter, rect, game);
                crate::hud::draw_compass(&painter, rect, game);
                crate::hud::draw_killfeed(&painter, rect, game);
                if !game.player.alive {
                    crate::hud::draw_center_banner(
                        &painter,
                        rect,
                        "YOU WERE KILLED",
                        egui::Color32::from_rgb(220, 50, 50),
                    );
                }
            });
            self.egui.paint(&mut enc, &view, w, h);
        }
        self.queue.submit([enc.finish()]);
        frame.present();
        action
    }
}

// ── input accumulation between frames ───────────────────────────────────────────────────────────────────
#[derive(Default)]
struct Keys {
    w: bool,
    a: bool,
    s: bool,
    d: bool,
    space: bool,
    shift: bool,
    ctrl: bool,
    fire: bool,
    ads: bool,
    dyaw: f32,
    dpitch: f32,
}
impl Keys {
    fn to_input(&mut self) -> Input {
        let mut inp = Input::default();
        inp.fwd = (self.w as i32 - self.s as i32) as f32;
        inp.strafe = (self.d as i32 - self.a as i32) as f32;
        inp.jump = self.space;
        inp.sprint = self.shift;
        inp.crouch = self.ctrl;
        inp.fire = self.fire;
        inp.ads = self.ads;
        inp.dyaw = self.dyaw;
        inp.dpitch = self.dpitch;
        self.dyaw = 0.0;
        self.dpitch = 0.0;
        inp
    }
}

/// Grab + hide the cursor for gameplay (FPS look), or release + show it for the menu — called
/// whenever the menu opens/closes so the player can actually point at buttons instead of fighting
/// a locked, invisible cursor.
fn set_cursor_state(window: &Window, menu_open: bool) {
    if menu_open {
        let _ = window.set_cursor_grab(winit::window::CursorGrabMode::None);
        window.set_cursor_visible(true);
    } else {
        let _ = window.set_cursor_grab(winit::window::CursorGrabMode::Locked).or_else(|_| window.set_cursor_grab(winit::window::CursorGrabMode::Confined));
        window.set_cursor_visible(false);
    }
}

pub fn run() {
    let event_loop = EventLoop::new().unwrap();
    let window = Arc::new(WindowBuilder::new().with_title("early-fps (GPU)").build(&event_loop).unwrap());
    let _ = window.set_cursor_grab(winit::window::CursorGrabMode::Locked).or_else(|_| window.set_cursor_grab(winit::window::CursorGrabMode::Confined));
    window.set_cursor_visible(false);
    let mut state = pollster::block_on(State::new(window.clone()));

    let mut game = Game::new(1234);
    let mut keys = Keys::default();
    let mut last = Instant::now();
    // menu state: None = playing (the previous, only, behaviour); Some(screen) = paused, showing
    // that screen. Escape used to just quit the whole app (elwt.exit()) — now it opens/navigates/
    // closes this instead.
    let mut menu: Option<MenuScreen> = None;
    let mut loadout = Loadout::default();
    let mut cursor_pos = egui::pos2(0.0, 0.0);
    // real pointer/keyboard events for the menu's egui widgets to actually be clickable — the HUD
    // never needed this (paint-only), so EguiOverlay::prepare previously had no events path at all.
    let mut menu_events: Vec<egui::Event> = Vec::new();

    event_loop
        .run(move |event, elwt| match event {
            Event::DeviceEvent { event: DeviceEvent::MouseMotion { delta }, .. } => {
                if menu.is_none() {
                    keys.dyaw += delta.0 as f32 * 0.0025;
                    keys.dpitch += -delta.1 as f32 * 0.0025;
                }
            }
            Event::WindowEvent { window_id, event } if window_id == window.id() => match event {
                WindowEvent::CloseRequested => elwt.exit(),
                WindowEvent::Resized(sz) => state.resize(sz.width, sz.height),
                WindowEvent::CursorMoved { position, .. } => {
                    // only matters while the menu is open — cursor_pos feeds egui's PointerMoved;
                    // during gameplay the cursor is locked and look comes from DeviceEvent::MouseMotion.
                    if menu.is_some() {
                        let scale = window.scale_factor() as f32;
                        cursor_pos = egui::pos2(position.x as f32 / scale.max(0.01), position.y as f32 / scale.max(0.01));
                        menu_events.push(egui::Event::PointerMoved(cursor_pos));
                    }
                }
                WindowEvent::MouseInput { state: st, button, .. } => {
                    let down = st == ElementState::Pressed;
                    if menu.is_some() {
                        let btn = match button {
                            MouseButton::Left => Some(egui::PointerButton::Primary),
                            MouseButton::Right => Some(egui::PointerButton::Secondary),
                            _ => None,
                        };
                        if let Some(b) = btn {
                            menu_events.push(egui::Event::PointerButton { pos: cursor_pos, button: b, pressed: down, modifiers: egui::Modifiers::default() });
                        }
                    } else {
                        match button {
                            MouseButton::Left => keys.fire = down,
                            MouseButton::Right => keys.ads = down,
                            _ => {}
                        }
                    }
                }
                WindowEvent::KeyboardInput { event: ke, .. } => {
                    let down = ke.state == ElementState::Pressed;
                    if let PhysicalKey::Code(code) = ke.physical_key {
                        if code == KeyCode::Escape {
                            if down {
                                menu = match menu {
                                    None => {
                                        keys = Keys::default(); // don't carry held movement into the pause
                                        Some(MenuScreen::Main)
                                    }
                                    Some(MenuScreen::Main) => {
                                        last = Instant::now(); // don't let menu time count as a huge dt
                                        None
                                    }
                                    Some(_) => Some(MenuScreen::Main), // a submenu backs out to Main first
                                };
                                set_cursor_state(&window, menu.is_some());
                            }
                        } else if menu.is_none() {
                            match code {
                                KeyCode::KeyW => keys.w = down,
                                KeyCode::KeyA => keys.a = down,
                                KeyCode::KeyS => keys.s = down,
                                KeyCode::KeyD => keys.d = down,
                                KeyCode::Space => keys.space = down,
                                KeyCode::ShiftLeft => keys.shift = down,
                                KeyCode::ControlLeft => keys.ctrl = down,
                                _ => {}
                            }
                        }
                    }
                }
                WindowEvent::RedrawRequested => {
                    let now = Instant::now();
                    let dt = (now - last).as_secs_f32().min(0.05);
                    last = now;

                    if menu.is_none() {
                        let inp = keys.to_input();
                        game.step(&inp, dt);
                    } // paused: the world holds at its last stepped state, same idea as the HUD's
                      // existing "YOU WERE KILLED" freeze-frame — nothing new here, just gated wider.

                    let cam = game.player.view_cam();
                    let fwd = cam_forward(&cam);
                    let aspect = state.config.width as f32 / state.config.height.max(1) as f32;
                    let fovy = 75f32.to_radians();
                    let vp = mul(&perspective(fovy, aspect, 0.05, 400.0), &look_at(cam.pos, fwd));
                    let wup = Vec3::new(0.0, 1.0, 0.0);
                    let right = fwd.cross(wup).normalized();
                    let camup = right.cross(fwd);
                    let tanh = (fovy * 0.5).tan();

                    let meshes = game.frame_meshes(&cam, false);
                    let (solid, opaque, trans) = flatten(&meshes, cam.pos);
                    state.upload(&solid, &opaque, &trans);

                    let sky = game.light.sky;
                    let grd = game.light.ground;
                    let fog = game.fog.color;
                    // HUD readouts
                    let hf = (game.player.health / game.player.max_health.max(1.0)).clamp(0.0, 1.0);
                    let w = game.player.weapon();
                    let af = (w.mag as f32 / w.kind.def().mag_size.max(1) as f32).clamp(0.0, 1.0);
                    let low = if hf < 0.3 { 1.0 } else { 0.0 };
                    // pack the game's dynamic point lights (muzzle, jetpack, rockets, explosions, lava, markers)
                    let dl = game.dynamic_lights();
                    let mut plights = [[0.0f32; 4]; MAX_PLIGHTS * 2];
                    let n = dl.len().min(MAX_PLIGHTS);
                    for (k, l) in dl.iter().take(MAX_PLIGHTS).enumerate() {
                        plights[k * 2] = [l.pos.x, l.pos.y, l.pos.z, l.radius.max(0.1)];
                        plights[k * 2 + 1] = [l.color.r() as f32 / 255.0, l.color.g() as f32 / 255.0, l.color.b() as f32 / 255.0, l.intensity];
                    }
                    let u = U {
                        viewproj: vp,
                        light: { let d = game.light.dir; [d.x, d.y, d.z, 0.0] },
                        key: [1.05, 0.99, 0.9, 0.0],
                        amb: [sky.r() as f32 / 255.0 * 0.6 + 0.25, sky.g() as f32 / 255.0 * 0.6 + 0.25, sky.b() as f32 / 255.0 * 0.6 + 0.25, 0.0],
                        fog: [fog.r() as f32 / 255.0, fog.g() as f32 / 255.0, fog.b() as f32 / 255.0, game.time],
                        params: [ATLAS_COLS as f32, game.fog.start, game.fog.end, n as f32],
                        cam: [cam.pos.x, cam.pos.y, cam.pos.z, aspect],
                        sky: [sky.r() as f32 / 255.0, sky.g() as f32 / 255.0, sky.b() as f32 / 255.0, 0.0],
                        ground: [grd.r() as f32 / 255.0, grd.g() as f32 / 255.0, grd.b() as f32 / 255.0, 0.0],
                        cr: [right.x, right.y, right.z, tanh],
                        cu: [camup.x, camup.y, camup.z, 0.0],
                        cf: [fwd.x, fwd.y, fwd.z, 0.0],
                        hud: [hf, af, low, 0.0],
                        plights,
                    };
                    let events = std::mem::take(&mut menu_events);
                    match state.render(&u, &game, &mut menu, &mut loadout, events) {
                        MenuAction::None => {}
                        MenuAction::Resume => {
                            menu = None;
                            last = Instant::now(); // don't let time spent in the menu count as a dt jump
                            set_cursor_state(&window, false);
                        }
                        MenuAction::StartGame(theme) => {
                            let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
                            let seed = (nanos as u64) ^ 0x9E3779B97F4A7C15;
                            game = Game::new_with(seed, Some(theme), loadout);
                            menu = None;
                            last = Instant::now();
                            set_cursor_state(&window, false);
                        }
                        MenuAction::Quit => elwt.exit(),
                    }
                    window.request_redraw();
                }
                _ => {}
            },
            _ => {}
        })
        .unwrap();
}

const SCENE_WGSL: &str = r#"
struct U {
  viewproj: mat4x4<f32>,
  light: vec4<f32>,
  key: vec4<f32>,
  amb: vec4<f32>,
  fog: vec4<f32>,
  params: vec4<f32>,   // x=cols, y=fog_near, z=fog_far, w=point-light count
  cam: vec4<f32>,      // xyz camera position, w = aspect
  sky: vec4<f32>,      // sky env colour
  ground: vec4<f32>,   // ground env colour
  cr: vec4<f32>,       // camera right (xyz), w = tan(fovy/2)
  cu: vec4<f32>,       // camera up
  cf: vec4<f32>,       // camera forward
  hud: vec4<f32>,      // x=health frac, y=ammo frac, z=low-health flag
  plights: array<vec4<f32>, 32>,   // pairs: [pos.xyz, radius], [col.rgb, intensity] × 16
};
@group(0) @binding(0) var<uniform> u: U;
@group(0) @binding(1) var atlas: texture_2d<f32>;
@group(0) @binding(2) var samp: sampler;

struct VO {
  @builtin(position) pos: vec4<f32>,
  @location(0) nrm: vec3<f32>,
  @location(1) col: vec4<f32>,
  @location(2) uv: vec2<f32>,
  @location(3) slot: f32,
  @location(4) wz: f32,
  @location(5) emis: f32,
  @location(6) wpos: vec3<f32>,
  @location(7) reflect: f32,
};

@vertex fn vs(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>, @location(2) c: vec4<f32>, @location(3) uv: vec2<f32>, @location(4) slot: f32, @location(5) emis: f32, @location(6) reflect: f32) -> VO {
  var o: VO;
  o.pos = u.viewproj * vec4<f32>(p, 1.0);
  o.nrm = n;
  o.col = c;
  o.uv = uv;
  o.slot = slot;
  o.wz = o.pos.w;   // view-space depth proxy for fog
  o.emis = emis;
  o.wpos = p;
  o.reflect = reflect;
  return o;
}

fn aces(x: f32) -> f32 {
  return clamp((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14), 0.0, 1.0);
}
// Matches the software renderer's grade(): ACES tonemap + saturation lift + split-tone (warm highlights,
// cool shadows) keyed to the sun term, so the GPU path reads the same graded, filmic look.
fn grade(c: vec3<f32>, ndl: f32) -> vec3<f32> {
  let e = 1.05;
  var r = aces(c.r * e);
  var g = aces(c.g * e);
  var b = aces(c.b * e);
  let lum = 0.299 * r + 0.587 * g + 0.114 * b;
  let sat = 1.14;
  r = clamp(lum + (r - lum) * sat, 0.0, 1.0);
  g = clamp(lum + (g - lum) * sat, 0.0, 1.0);
  b = clamp(lum + (b - lum) * sat, 0.0, 1.0);
  let shadow = clamp(1.0 - ndl, 0.0, 1.0);
  let hi = clamp(ndl, 0.0, 1.0);
  r = clamp(r + hi * 0.035 - shadow * 0.012, 0.0, 1.0);
  g = clamp(g + hi * 0.01, 0.0, 1.0);
  b = clamp(b - hi * 0.03 + shadow * 0.045, 0.0, 1.0);
  return vec3<f32>(r, g, b);
}

@fragment fn fs(i: VO) -> @location(0) vec4<f32> {
  // atlas cell from the material slot, with per-pixel tiling inside the cell
  let cols = u.params.x;
  let s = floor(i.slot + 0.5);
  let tx = s - cols * floor(s / cols);
  let ty = floor(s / cols);
  let au = (tx + fract(i.uv.x)) / cols;
  let av = (ty + fract(i.uv.y)) / cols;
  // gradients from the CONTINUOUS uv (not the fract'd one) so mip selection is correct across tile seams
  let ddx = dpdx(i.uv) / cols;
  let ddy = dpdy(i.uv) / cols;
  let texel = textureSampleGrad(atlas, samp, vec2<f32>(au, av), ddx, ddy);
  var albedo = i.col.rgb * texel.rgb;

  // alpha cutout (foliage/particles); water/glass blend via the pipeline's alpha blending
  if (i.col.a < 0.35) { discard; }

  let nrm = normalize(i.nrm);
  let L = normalize(u.light.xyz);
  let ndl = max(dot(nrm, L), 0.0);
  var shade = u.amb.xyz + u.key.xyz * ndl;

  // dynamic point lights (muzzle, jetpack, rockets, explosions, lava, markers): distance-attenuated diffuse
  let np = i32(u.params.w);
  for (var k = 0; k < np; k = k + 1) {
    let lp = u.plights[k * 2];
    let lc = u.plights[k * 2 + 1];
    let d = lp.xyz - i.wpos;
    let dist = length(d);
    let atten = clamp(1.0 - dist / max(lp.w, 0.1), 0.0, 1.0);
    let pndl = max(dot(nrm, d / max(dist, 0.001)), 0.0);
    shade = shade + lc.rgb * (lc.w * atten * atten * pndl);
  }
  var lit = albedo * shade;

  // view-dependent fresnel sheen (all surfaces) + environment reflection (water/ice/metal), matching the
  // software renderer: a Schlick-fresnel blend toward a sky↔ground gradient, plus a sharp sun glint.
  let V = normalize(u.cam.xyz - i.wpos);
  let nv = max(dot(nrm, V), 0.0);
  let rim = pow(1.0 - nv, 3.0) * 0.12 * (0.4 + 0.6 * (0.5 + 0.5 * nrm.y));
  lit = lit + u.sky.xyz * rim;
  if (i.reflect > 0.001) {
    let fr = 1.0 - nv;
    let fres = fr * fr * fr * fr * fr;
    let amt = min(i.reflect * (0.18 + 0.82 * fres), 0.78);
    let horizon = mix(u.ground.xyz, u.sky.xyz, 0.72);
    var envc = horizon;
    if (nrm.y >= 0.0) { envc = mix(horizon, u.sky.xyz, nrm.y); }
    else { envc = mix(horizon, u.ground.xyz, -nrm.y); }
    let env = mix(envc, albedo, 0.16);
    lit = mix(lit, env, amt);
    let H = normalize(L + V);
    let glint = pow(max(dot(nrm, H), 0.0), 80.0) * i.reflect * 1.5 * (0.3 + 0.7 * ndl);
    lit = lit + u.key.xyz * glint;
  }

  // filmic grade + split-tone (matches the software renderer), keyed to the sun term
  lit = grade(lit, ndl);

  // distance fog toward the sky/fog colour
  let fnear = u.params.y;
  let ffar = max(u.params.z, fnear + 1.0);
  let f = clamp((i.wz - fnear) / (ffar - fnear), 0.0, 1.0);
  lit = mix(lit, u.fog.xyz, f);

  // emissive punches through fog and lighting (lava/crystals/tracers/markers glow)
  lit = lit + albedo * clamp(i.emis, 0.0, 3.0);

  return vec4<f32>(lit, i.col.a);
}

// ── overlay: crosshair + health/ammo bars. Six quads (36 verts), each coloured in the vertex shader. ──
struct OV { @builtin(position) pos: vec4<f32>, @location(0) col: vec4<f32> };

fn quad_corner(local: u32, x0: f32, y0: f32, x1: f32, y1: f32) -> vec2<f32> {
  var uu = 0.0;
  var vv = 0.0;
  if (local == 1u) { uu = 1.0; vv = 0.0; }
  else if (local == 2u) { uu = 1.0; vv = 1.0; }
  else if (local == 3u) { uu = 0.0; vv = 0.0; }
  else if (local == 4u) { uu = 1.0; vv = 1.0; }
  else if (local == 5u) { uu = 0.0; vv = 1.0; }
  return vec2<f32>(mix(x0, x1, uu), mix(y0, y1, vv));
}

@vertex fn vs_overlay(@builtin(vertex_index) vi: u32) -> OV {
  var o: OV;
  let q = vi / 6u;
  let l = vi % 6u;
  let aspect = max(u.cam.w, 0.001);
  var p = vec2<f32>(0.0, 0.0);
  var col = vec4<f32>(1.0, 1.0, 1.0, 1.0);
  if (q == 0u) {
    let tx = 0.020 / aspect; let th = 0.0022;
    p = quad_corner(l, -tx, -th, tx, th);
    col = vec4<f32>(0.95, 0.97, 1.0, 0.85);
  } else if (q == 1u) {
    let ty = 0.020; let thx = 0.0022 / aspect;
    p = quad_corner(l, -thx, -ty, thx, ty);
    col = vec4<f32>(0.95, 0.97, 1.0, 0.85);
  } else if (q == 2u) {
    p = quad_corner(l, -0.92, -0.94, -0.52, -0.90);
    col = vec4<f32>(0.05, 0.06, 0.08, 0.72);
  } else if (q == 3u) {
    let hf = u.hud.x;
    p = quad_corner(l, -0.912, -0.933, -0.912 + 0.384 * hf, -0.907);
    let g = vec3<f32>(0.3, 0.85, 0.35);
    let r = vec3<f32>(0.9, 0.25, 0.2);
    col = vec4<f32>(mix(r, g, hf), 0.95);
  } else if (q == 4u) {
    p = quad_corner(l, 0.52, -0.94, 0.92, -0.90);
    col = vec4<f32>(0.05, 0.06, 0.08, 0.72);
  } else {
    let af = u.hud.y;
    p = quad_corner(l, 0.528, -0.933, 0.528 + 0.384 * af, -0.907);
    col = vec4<f32>(0.95, 0.75, 0.25, 0.95);
  }
  o.pos = vec4<f32>(p, 0.0, 1.0);
  o.col = col;
  return o;
}
@fragment fn fs_overlay(i: OV) -> @location(0) vec4<f32> {
  return i.col;
}

// ── sky gradient: a fullscreen triangle, sky colour up top fading to the fog/horizon colour lower down ──
struct SO { @builtin(position) pos: vec4<f32>, @location(0) v: f32, @location(1) ndc: vec2<f32> };
@vertex fn vs_sky(@builtin(vertex_index) vi: u32) -> SO {
  // oversized triangle covering the screen: clip positions (-1,-1),(3,-1),(-1,3)
  var xy = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
  var o: SO;
  let p = xy[vi];
  o.pos = vec4<f32>(p, 1.0, 1.0); // z=1 → far plane
  o.v = 1.0 - (p.y * 0.5 + 0.5);  // 0 at top of screen, 1 at bottom
  o.ndc = p;
  return o;
}
@fragment fn fs_sky(i: SO) -> @location(0) vec4<f32> {
  // top: sky; toward the horizon: fog colour. A soft curve keeps the zenith rich.
  let t = clamp(i.v, 0.0, 1.0);
  let horizon = mix(u.sky.xyz, u.fog.xyz, 0.85);
  var c = mix(u.sky.xyz, horizon, pow(t, 0.8));
  // reconstruct the world-space view ray from the camera basis and add a sun disk + glow toward the light
  let ray = normalize(u.cf.xyz + u.cr.xyz * (i.ndc.x * u.cr.w * u.cam.w) + u.cu.xyz * (i.ndc.y * u.cr.w));
  let sun = normalize(u.light.xyz);
  let sd = max(dot(ray, sun), 0.0);
  let disk = smoothstep(0.9986, 0.9995, sd);
  let glow = pow(sd, 260.0) * 0.55;
  c = c + vec3<f32>(1.0, 0.96, 0.86) * (disk + glow);
  return vec4<f32>(c, 1.0);
}
"#;

const POST_WGSL: &str = r#"
@group(0) @binding(0) var scene_tex: texture_2d<f32>;
@group(0) @binding(1) var scene_samp: sampler;
struct PU { v: vec4<f32> }; // x=low-health flag, y=time, z=health frac
@group(0) @binding(2) var<uniform> pu: PU;

struct PO { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32> };

@vertex fn vs_post(@builtin(vertex_index) vi: u32) -> PO {
  var xy = array<vec2<f32>, 3>(vec2<f32>(-1.0, -1.0), vec2<f32>(3.0, -1.0), vec2<f32>(-1.0, 3.0));
  var o: PO;
  let p = xy[vi];
  o.pos = vec4<f32>(p, 0.0, 1.0);
  o.uv = vec2<f32>(p.x * 0.5 + 0.5, 1.0 - (p.y * 0.5 + 0.5)); // flip Y for texture space
  return o;
}

fn bright(c: vec3<f32>) -> vec3<f32> {
  let l = dot(c, vec3<f32>(0.299, 0.587, 0.114));
  return c * (max(l - 0.75, 0.0) / max(l, 0.001));
}

@fragment fn fs_post(i: PO) -> @location(0) vec4<f32> {
  let base = textureSample(scene_tex, scene_samp, i.uv).rgb;
  // cheap bloom: gather bright neighbours around the pixel
  let o = 0.0032;
  var b = bright(base);
  b = b + bright(textureSample(scene_tex, scene_samp, i.uv + vec2<f32>(o, 0.0)).rgb);
  b = b + bright(textureSample(scene_tex, scene_samp, i.uv + vec2<f32>(-o, 0.0)).rgb);
  b = b + bright(textureSample(scene_tex, scene_samp, i.uv + vec2<f32>(0.0, o)).rgb);
  b = b + bright(textureSample(scene_tex, scene_samp, i.uv + vec2<f32>(0.0, -o)).rgb);
  b = b + bright(textureSample(scene_tex, scene_samp, i.uv + vec2<f32>(o, o)).rgb);
  b = b + bright(textureSample(scene_tex, scene_samp, i.uv + vec2<f32>(-o, -o)).rgb);
  b = b * (1.0 / 7.0) * 0.6;
  var col = base + b;
  // (tonemap + grade now happen in the scene shader) — post keeps bloom, vignette, damage flash
  // subtle vignette
  let d = i.uv - vec2<f32>(0.5, 0.5);
  let r2 = dot(d, d);
  col = col * clamp(1.0 - r2 * 0.6, 0.0, 1.0);
  // low-health damage vignette: pulsing red bloom in from the edges
  let pulse = 0.55 + 0.45 * sin(pu.v.y * 6.0);
  let dmg = clamp(pu.v.x * pulse * smoothstep(0.15, 0.5, r2), 0.0, 1.0);
  col = mix(col, vec3<f32>(0.75, 0.05, 0.05), dmg * 0.6);
  return vec4<f32>(col, 1.0);
}
"#;

#[cfg(test)]
mod present_tests {
    use super::choose_present_mode;
    use wgpu::PresentMode;

    #[test]
    fn defaults_to_fifo_even_when_mailbox_offered() {
        // The stutter fix: surfaces often list Mailbox/Immediate first; the default must be Fifo.
        let supported = [PresentMode::Mailbox, PresentMode::Immediate, PresentMode::Fifo];
        std::env::remove_var("SOLDIER_VSYNC");
        assert_eq!(choose_present_mode(&supported), PresentMode::Fifo);
    }

    #[test]
    fn env_override_reaches_low_latency_and_falls_back() {
        std::env::set_var("SOLDIER_VSYNC", "mailbox");
        assert_eq!(choose_present_mode(&[PresentMode::Mailbox, PresentMode::Fifo]), PresentMode::Mailbox);
        // only Fifo offered → low-latency intent still yields Fifo, never panics on an empty pick
        assert_eq!(choose_present_mode(&[PresentMode::Fifo]), PresentMode::Fifo);
        std::env::remove_var("SOLDIER_VSYNC");
    }
}
