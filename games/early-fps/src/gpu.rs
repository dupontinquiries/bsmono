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
    // Dynamic point lights, packed as pairs: [pos.xyz, radius], [col.rgb, intensity]. Up to MAX_PLIGHTS.
    plights: [[f32; 4]; MAX_PLIGHTS * 2],
}

const MAX_PLIGHTS: usize = 16;

/// Flatten meshes into two vertex lists: `solid` (single-sided, safe to backface-cull — walls, most geometry)
/// and `double` (double-sided — water, foliage, particles, billboards). Culling the solid set halves its
/// fill and removes the far side of every wall.
fn flatten(meshes: &[Mesh3]) -> (Vec<Vert>, Vec<Vert>) {
    let mut solid = Vec::new();
    let mut double = Vec::new();
    for m in meshes {
        let slot = m.material.map(atlas_slot).unwrap_or(0) as f32;
        let emis = m.emissive;
        let base = m.color;
        let dst = if m.single_sided { &mut solid } else { &mut double };
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
                });
            }
        }
    }
    (solid, double)
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
    ubuf: wgpu::Buffer,
    bind: wgpu::BindGroup,
    vbuf_solid: wgpu::Buffer,
    vcap_solid: u64,
    vcount_solid: u32,
    vbuf_double: wgpu::Buffer,
    vcap_double: u64,
    vcount_double: u32,
    depth: wgpu::TextureView,
}

fn make_depth(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d { width: w.max(1), height: h.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
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
            present_mode: caps.present_modes[0],
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        // ── material atlas → GPU texture ──
        let tile = 512usize;
        let (atlas_bytes, asz, _) = build_material_atlas(tile, ATLAS_COLS);
        let atlas_tex = device.create_texture_with_data(
            &queue,
            &wgpu::TextureDescriptor {
                label: Some("atlas"),
                size: wgpu::Extent3d { width: asz as u32, height: asz as u32, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8UnormSrgb,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            },
            wgpu::util::TextureDataOrder::LayerMajor,
            &atlas_bytes,
        );
        let atlas_view = atlas_tex.create_view(&Default::default());
        let samp = device.create_sampler(&wgpu::SamplerDescriptor { mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });

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
        ];
        // NOTE: front_face defaults to Ccw here. If single-sided surfaces (walls) render see-through or
        // inside-out on a real GPU, the winding is the other way — flip this to `Cw` for the solid pipeline.
        let mk_pipe = |cull: Option<wgpu::Face>| {
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
                depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: true, depth_compare: wgpu::CompareFunction::Less, stencil: Default::default(), bias: Default::default() }),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        let pipe_solid = mk_pipe(Some(wgpu::Face::Back));
        let pipe_double = mk_pipe(None);

        let vcap = 64 * 1024 * std::mem::size_of::<Vert>() as u64;
        let mk_vbuf = || device.create_buffer(&wgpu::BufferDescriptor { label: Some("v"), size: vcap, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let vbuf_solid = mk_vbuf();
        let vbuf_double = mk_vbuf();
        let depth = make_depth(&device, config.width, config.height);

        State {
            surface,
            device,
            queue,
            config,
            pipe_solid,
            pipe_double,
            ubuf,
            bind,
            vbuf_solid,
            vcap_solid: vcap,
            vcount_solid: 0,
            vbuf_double,
            vcap_double: vcap,
            vcount_double: 0,
            depth,
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
    }

    fn upload(&mut self, solid: &[Vert], double: &[Vert]) {
        let sb = bytemuck::cast_slice(solid);
        if sb.len() as u64 > self.vcap_solid {
            self.vcap_solid = (sb.len() as u64).next_power_of_two();
            self.vbuf_solid = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("vs"), size: self.vcap_solid, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        }
        if !sb.is_empty() {
            self.queue.write_buffer(&self.vbuf_solid, 0, sb);
        }
        self.vcount_solid = solid.len() as u32;

        let db = bytemuck::cast_slice(double);
        if db.len() as u64 > self.vcap_double {
            self.vcap_double = (db.len() as u64).next_power_of_two();
            self.vbuf_double = self.device.create_buffer(&wgpu::BufferDescriptor { label: Some("vd"), size: self.vcap_double, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        }
        if !db.is_empty() {
            self.queue.write_buffer(&self.vbuf_double, 0, db);
        }
        self.vcount_double = double.len() as u32;
    }

    fn render(&mut self, u: &U) {
        self.queue.write_buffer(&self.ubuf, 0, bytemuck::bytes_of(u));
        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(_) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
        };
        let view = frame.texture.create_view(&Default::default());
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let fog = u.fog;
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color { r: fog[0] as f64, g: fog[1] as f64, b: fog[2] as f64, a: 1.0 }), store: wgpu::StoreOp::Store } })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment { view: &self.depth, depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }), stencil_ops: None }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rp.set_bind_group(0, &self.bind, &[]);
            // solid geometry: backface-culled
            if self.vcount_solid > 0 {
                rp.set_pipeline(&self.pipe_solid);
                rp.set_vertex_buffer(0, self.vbuf_solid.slice(..));
                rp.draw(0..self.vcount_solid, 0..1);
            }
            // double-sided geometry (water, foliage, particles): both faces
            if self.vcount_double > 0 {
                rp.set_pipeline(&self.pipe_double);
                rp.set_vertex_buffer(0, self.vbuf_double.slice(..));
                rp.draw(0..self.vcount_double, 0..1);
            }
        }
        self.queue.submit([enc.finish()]);
        frame.present();
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

pub fn run() {
    let event_loop = EventLoop::new().unwrap();
    let window = Arc::new(WindowBuilder::new().with_title("early-fps (GPU)").build(&event_loop).unwrap());
    let _ = window.set_cursor_grab(winit::window::CursorGrabMode::Locked).or_else(|_| window.set_cursor_grab(winit::window::CursorGrabMode::Confined));
    window.set_cursor_visible(false);
    let mut state = pollster::block_on(State::new(window.clone()));

    let mut game = Game::new(1234);
    let mut keys = Keys::default();
    let mut last = Instant::now();

    event_loop
        .run(move |event, elwt| match event {
            Event::DeviceEvent { event: DeviceEvent::MouseMotion { delta }, .. } => {
                keys.dyaw += delta.0 as f32 * 0.0025;
                keys.dpitch += -delta.1 as f32 * 0.0025;
            }
            Event::WindowEvent { window_id, event } if window_id == window.id() => match event {
                WindowEvent::CloseRequested => elwt.exit(),
                WindowEvent::Resized(sz) => state.resize(sz.width, sz.height),
                WindowEvent::MouseInput { state: st, button, .. } => {
                    let down = st == ElementState::Pressed;
                    match button {
                        MouseButton::Left => keys.fire = down,
                        MouseButton::Right => keys.ads = down,
                        _ => {}
                    }
                }
                WindowEvent::KeyboardInput { event: ke, .. } => {
                    let down = ke.state == ElementState::Pressed;
                    if let PhysicalKey::Code(code) = ke.physical_key {
                        match code {
                            KeyCode::KeyW => keys.w = down,
                            KeyCode::KeyA => keys.a = down,
                            KeyCode::KeyS => keys.s = down,
                            KeyCode::KeyD => keys.d = down,
                            KeyCode::Space => keys.space = down,
                            KeyCode::ShiftLeft => keys.shift = down,
                            KeyCode::ControlLeft => keys.ctrl = down,
                            KeyCode::Escape => elwt.exit(),
                            _ => {}
                        }
                    }
                }
                WindowEvent::RedrawRequested => {
                    let now = Instant::now();
                    let dt = (now - last).as_secs_f32().min(0.05);
                    last = now;

                    let inp = keys.to_input();
                    game.step(&inp, dt);

                    let cam = game.player.view_cam();
                    let fwd = cam_forward(&cam);
                    let aspect = state.config.width as f32 / state.config.height.max(1) as f32;
                    let vp = mul(&perspective(75f32.to_radians(), aspect, 0.05, 400.0), &look_at(cam.pos, fwd));

                    let meshes = game.frame_meshes(&cam, false);
                    let (solid, double) = flatten(&meshes);
                    state.upload(&solid, &double);

                    let sky = game.light.sky;
                    let fog = game.fog.color;
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
                        fog: [fog.r() as f32 / 255.0, fog.g() as f32 / 255.0, fog.b() as f32 / 255.0, 0.0],
                        params: [ATLAS_COLS as f32, game.fog.start, game.fog.end, n as f32],
                        plights,
                    };
                    state.render(&u);
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
};

@vertex fn vs(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>, @location(2) c: vec4<f32>, @location(3) uv: vec2<f32>, @location(4) slot: f32, @location(5) emis: f32) -> VO {
  var o: VO;
  o.pos = u.viewproj * vec4<f32>(p, 1.0);
  o.nrm = n;
  o.col = c;
  o.uv = uv;
  o.slot = slot;
  o.wz = o.pos.w;   // view-space depth proxy for fog
  o.emis = emis;
  o.wpos = p;
  return o;
}

@fragment fn fs(i: VO) -> @location(0) vec4<f32> {
  // atlas cell from the material slot, with per-pixel tiling inside the cell
  let cols = u.params.x;
  let s = floor(i.slot + 0.5);
  let tx = s - cols * floor(s / cols);
  let ty = floor(s / cols);
  let au = (tx + fract(i.uv.x)) / cols;
  let av = (ty + fract(i.uv.y)) / cols;
  let texel = textureSample(atlas, samp, vec2<f32>(au, av));
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

  // distance fog toward the sky/fog colour
  let fnear = u.params.y;
  let ffar = max(u.params.z, fnear + 1.0);
  let f = clamp((i.wz - fnear) / (ffar - fnear), 0.0, 1.0);
  lit = mix(lit, u.fog.xyz, f);

  // emissive punches through fog and lighting (lava/crystals/tracers/markers glow)
  lit = lit + albedo * clamp(i.emis, 0.0, 3.0);

  return vec4<f32>(lit, i.col.a);
}
"#;
