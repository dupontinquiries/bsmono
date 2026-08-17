//! Interactive path. Space plays/reverses at the chassis' xform_dur,
//! [ ] cycle chassis, 1-3 cycle variants, drag orbits. Geometry is re-posed on
//! the CPU each frame via the same rig::points_at the tests verify — one code
//! path, one truth.

use crate::rig::{self, Bot};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use wgpu::util::DeviceExt;
use winit::{
    event::{ElementState, Event, MouseButton, WindowEvent},
    event_loop::EventLoop,
    keyboard::{KeyCode, PhysicalKey},
    window::WindowBuilder,
};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vert {
    pos: [f32; 3],
    nrm: [f32; 3],
    tint: [f32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    eye: [f32; 4],
}

fn mat_mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut m = [[0.0; 4]; 4];
    for r in 0..4 { for c in 0..4 { for k in 0..4 { m[r][c] += a[r][k] * b[k][c]; } } }
    m
}

fn look_proj(eye: [f32; 3], at: [f32; 3], aspect: f32) -> [[f32; 4]; 4] {
    let f = {
        let d = [at[0] - eye[0], at[1] - eye[1], at[2] - eye[2]];
        let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        [d[0] / l, d[1] / l, d[2] / l]
    };
    let s = {
        let c = [f[1] * 0.0 - f[2] * 1.0, f[2] * 0.0 - f[0] * 0.0, f[0] * 1.0 - f[1] * 0.0];
        let l = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt().max(1e-6);
        [c[0] / l, c[1] / l, c[2] / l]
    };
    let u = [s[1] * f[2] - s[2] * f[1], s[2] * f[0] - s[0] * f[2], s[0] * f[1] - s[1] * f[0]];
    let view = [
        [s[0], u[0], -f[0], 0.0],
        [s[1], u[1], -f[1], 0.0],
        [s[2], u[2], -f[2], 0.0],
        [-(s[0] * eye[0] + s[1] * eye[1] + s[2] * eye[2]),
         -(u[0] * eye[0] + u[1] * eye[1] + u[2] * eye[2]),
         f[0] * eye[0] + f[1] * eye[1] + f[2] * eye[2], 1.0],
    ];
    let (fovy, zn, zf) = (0.9f32, 0.05f32, 400.0f32);
    let t = 1.0 / (fovy / 2.0).tan();
    let proj = [
        [t / aspect, 0.0, 0.0, 0.0],
        [0.0, t, 0.0, 0.0],
        [0.0, 0.0, zf / (zn - zf), -1.0],
        [0.0, 0.0, zn * zf / (zn - zf), 0.0],
    ];
    mat_mul(view, proj)
}

fn tessellate(bot: &Bot, t: f64) -> Vec<Vert> {
    let mut out = Vec::new();
    for i in 0..bot.parts.len() {
        let pts = bot.points_at(i, t);
        let p = &bot.parts[i];
        for tri in p.idx.chunks_exact(3) {
            let [a, b, c] = [pts[tri[0] as usize], pts[tri[1] as usize], pts[tri[2] as usize]];
            let n = {
                let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
                let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
                let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
                let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(1e-9);
                [(n[0] / l) as f32, (n[1] / l) as f32, (n[2] / l) as f32]
            };
            for q in [a, b, c] {
                out.push(Vert { pos: [q[0] as f32, q[1] as f32, q[2] as f32], nrm: n, tint: p.tint });
            }
        }
    }
    out
}

pub fn run(data: &Path, content: &Path, variant: &str, chassis: &str) -> Result<()> {
    env_logger::init();
    let (data, content) = (data.to_path_buf(), content.to_path_buf());
    let chassis_names: Vec<String> = rig::load_chassis(&content)?.keys().cloned().collect();
    let variants = ["watch", "corsair", "prime"];
    let mut ci = chassis_names.iter().position(|c| c == chassis).unwrap_or(0);
    let mut vi = variants.iter().position(|v| *v == variant).unwrap_or(0);
    fn build_bot(data: &PathBuf, content: &PathBuf, variants: &[&str], names: &[String], vi: usize, ci: usize) -> Result<Bot> {
        rig::build(data, content, variants[vi], &names[ci], rig::profile_for(&names[ci]))
    }
    let mut bot = build_bot(&data, &content, &variants, &chassis_names, vi, ci)?;

    let event_loop = EventLoop::new()?;
    let window = Arc::new(WindowBuilder::new().with_title("foldview").build(&event_loop)?);
    let instance = wgpu::Instance::default();
    let surface = instance.create_surface(window.clone())?;
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        compatible_surface: Some(&surface),
        ..Default::default()
    }))
    .context("no adapter")?;
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default(), None))?;
    let mut size = window.inner_size();
    let mut config = surface
        .get_default_config(&adapter, size.width.max(1), size.height.max(1))
        .context("surface config")?;
    surface.configure(&device, &config);

    let shader = device.create_shader_module(wgpu::include_wgsl!("shader.wgsl"));
    let ubuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: std::mem::size_of::<Uniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        }],
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &bgl,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }],
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[&bgl],
        push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None,
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: "vs",
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Vert>() as u64,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x3],
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: "fs",
            targets: &[Some(config.format.into())],
        }),
        primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: Default::default(),
            bias: Default::default(),
        }),
        multisample: Default::default(),
        multiview: None,
    });
    let mut depth = make_depth(&device, &config);

    let (mut t, mut dir, mut playing) = (0.0f64, 1.0f64, false);
    let (mut yaw, mut pitch, mut dragging) = (0.75f64, 0.3f64, false);
    let mut last_cursor: Option<(f64, f64)> = None;
    let mut last = std::time::Instant::now();

    event_loop.run(move |event, elwt| match event {
        Event::WindowEvent { event, .. } => match event {
            WindowEvent::CloseRequested => elwt.exit(),
            WindowEvent::Resized(s) => {
                size = s;
                config.width = s.width.max(1);
                config.height = s.height.max(1);
                surface.configure(&device, &config);
                depth = make_depth(&device, &config);
            }
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                dragging = state == ElementState::Pressed;
                last_cursor = None;
            }
            WindowEvent::CursorMoved { position, .. } => {
                if dragging {
                    if let Some((lx, ly)) = last_cursor {
                        yaw += (position.x - lx) * 0.008;
                        pitch = (pitch + (position.y - ly) * 0.006).clamp(-1.3, 1.3);
                    }
                    last_cursor = Some((position.x, position.y));
                }
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let mut rebuild = false;
                match event.physical_key {
                    PhysicalKey::Code(KeyCode::Space) => { playing = true; dir = -dir; }
                    PhysicalKey::Code(KeyCode::BracketRight) => { ci = (ci + 1) % chassis_names.len(); rebuild = true; }
                    PhysicalKey::Code(KeyCode::BracketLeft) => { ci = (ci + chassis_names.len() - 1) % chassis_names.len(); rebuild = true; }
                    PhysicalKey::Code(KeyCode::Digit1) => { vi = 0; rebuild = true; }
                    PhysicalKey::Code(KeyCode::Digit2) => { vi = 1; rebuild = true; }
                    PhysicalKey::Code(KeyCode::Digit3) => { vi = 2; rebuild = true; }
                    PhysicalKey::Code(KeyCode::Escape) => elwt.exit(),
                    _ => {}
                }
                if rebuild {
                    if let Ok(b) = build_bot(&data, &content, &variants, &chassis_names, vi, ci) {
                        window.set_title(&format!("foldview — {}", b.label));
                        bot = b;
                    }
                }
            }
            WindowEvent::RedrawRequested => {
                let dt = last.elapsed().as_secs_f64();
                last = std::time::Instant::now();
                if playing {
                    t = (t + dir * dt / bot.duration).clamp(0.0, 1.0);
                    if t == 0.0 || t == 1.0 { playing = false; }
                }
                let verts = tessellate(&bot, t);
                let vbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(&verts),
                    usage: wgpu::BufferUsages::VERTEX,
                });
                let dist = 22.0f32;
                let eye = [
                    dist * (pitch.cos() * yaw.sin()) as f32,
                    6.0 + dist * pitch.sin() as f32,
                    dist * (pitch.cos() * yaw.cos()) as f32,
                ];
                let u = Uniforms {
                    view_proj: look_proj(eye, [0.0, 5.0, 0.0], size.width as f32 / size.height.max(1) as f32),
                    eye: [eye[0], eye[1], eye[2], 1.0],
                };
                queue.write_buffer(&ubuf, 0, bytemuck::bytes_of(&u));
                if let Ok(frame) = surface.get_current_texture() {
                    let view = frame.texture.create_view(&Default::default());
                    let mut enc = device.create_command_encoder(&Default::default());
                    {
                        let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                            label: None,
                            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                                view: &view,
                                resolve_target: None,
                                ops: wgpu::Operations {
                                    load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.05, g: 0.06, b: 0.08, a: 1.0 }),
                                    store: wgpu::StoreOp::Store,
                                },
                            })],
                            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                                view: &depth,
                                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                                stencil_ops: None,
                            }),
                            timestamp_writes: None,
                            occlusion_query_set: None,
                        });
                        rp.set_pipeline(&pipeline);
                        rp.set_bind_group(0, &bind, &[]);
                        rp.set_vertex_buffer(0, vbuf.slice(..));
                        rp.draw(0..verts.len() as u32, 0..1);
                    }
                    queue.submit([enc.finish()]);
                    frame.present();
                }
                window.request_redraw();
            }
            _ => {}
        },
        Event::AboutToWait => window.request_redraw(),
        _ => {}
    })?;
    Ok(())
}

fn make_depth(device: &wgpu::Device, config: &wgpu::SurfaceConfiguration) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: config.width, height: config.height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}
