//! gpu_egui — a minimal egui→wgpu renderer, so the GPU path can draw the same egui HUD the software
//! path does **without** upgrading winit (0.29→0.30) or pulling version-locked egui-winit/egui-wgpu.
//!
//! Soldier's HUD is drawn with `egui::Painter` (immediate-mode shapes + text — no interactive widgets,
//! so no input plumbing is needed). This runs a headless `egui::Context` each frame, lets the shared
//! `crate::hud` painters draw into it, tessellates, and paints the resulting meshes over the finished
//! 3-D frame with a small 2-D pipeline. Uses egui 0.34 + wgpu 22 (both already deps).
//!
//! Scope note: colours/gamma follow the standard egui convention (sRGB vertex colours, converted in
//! the shader); exact pixel/gamma match is a on-screen tuning detail, but the HUD *content* is the
//! identical shared code, so it can't drift from the software path.

use egui::epaint::Primitive;
use egui::{ClippedPrimitive, Context, TexturesDelta};
use std::collections::HashMap;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    screen_size_points: [f32; 2],
    _pad: [f32; 2],
}

/// One uploaded egui texture (the font atlas, plus any user textures) + its bind group.
struct Tex {
    texture: wgpu::Texture,
    bind: wgpu::BindGroup,
}

pub struct EguiOverlay {
    ctx: Context,
    pipeline: wgpu::RenderPipeline,
    uniform_buf: wgpu::Buffer,
    uniform_bind: wgpu::BindGroup,
    tex_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: HashMap<egui::TextureId, Tex>,
    vbuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
    vcap: u64,
    icap: u64,
    /// Prepared draw list for the current frame: (index range, base vertex, clip rect in physical px).
    calls: Vec<(std::ops::Range<u32>, i32, [u32; 4], egui::TextureId)>,
    pixels_per_point: f32,
}

impl EguiOverlay {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("egui-shader"),
            source: wgpu::ShaderSource::Wgsl(SHADER.into()),
        });

        let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("egui-uniform-bgl"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("egui-tex-bgl"),
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
            ],
        });

        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("egui-pl"),
            bind_group_layouts: &[&uniform_bgl, &tex_bgl],
            push_constant_ranges: &[],
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("egui-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 5 * 4, // pos:2f + uv:2f + color:4xu8
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 0, shader_location: 0 },
                        wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x2, offset: 8, shader_location: 1 },
                        wgpu::VertexAttribute { format: wgpu::VertexFormat::Unorm8x4, offset: 16, shader_location: 2 },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState {
                        color: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::One, // egui vertex colours are premultiplied in-shader
                            dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                            operation: wgpu::BlendOperation::Add,
                        },
                        alpha: wgpu::BlendComponent {
                            src_factor: wgpu::BlendFactor::OneMinusDstAlpha,
                            dst_factor: wgpu::BlendFactor::One,
                            operation: wgpu::BlendOperation::Add,
                        },
                    }),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::TriangleList, ..Default::default() },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("egui-uniform"),
            size: std::mem::size_of::<Uniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("egui-uniform-bg"),
            layout: &uniform_bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniform_buf.as_entire_binding() }],
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("egui-sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("egui-vbuf"),
            size: 4096 * 20,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let ibuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("egui-ibuf"),
            size: 8192 * 4,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let ctx = Context::default();
        crate::hud::theme::apply_visuals(&ctx);
        EguiOverlay {
            ctx,
            pipeline,
            uniform_buf,
            uniform_bind,
            tex_bgl,
            sampler,
            textures: HashMap::new(),
            vbuf,
            ibuf,
            vcap: 4096 * 20,
            icap: 8192 * 4,
            calls: Vec::new(),
            pixels_per_point: 1.0,
        }
    }

    /// Run one egui frame (the `ui` closure draws the HUD via `egui::Context`), upload its textures +
    /// meshes, and record the draw list. Call once per frame before `paint`. `width`/`height` are
    /// physical pixels; `scale` is the DPI scale (pixels per point).
    pub fn prepare(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        width: u32,
        height: u32,
        scale: f32,
        ui: impl FnMut(&Context),
    ) {
        self.pixels_per_point = scale.max(0.1);
        let points = egui::vec2(width as f32 / self.pixels_per_point, height as f32 / self.pixels_per_point);
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), points)),
            ..Default::default()
        };
        #[allow(deprecated)] // run() (Context closure) is the ergonomic API for our painter HUD;
        // run_ui() hands a &mut Ui instead. Pinned to egui 0.34 where run() exists.
        let full = self.ctx.run(raw, ui);
        self.set_textures(device, queue, &full.textures_delta);

        let prims = self.ctx.tessellate(full.shapes, self.pixels_per_point);

        // pack all meshes into the shared vertex/index buffers
        let mut verts: Vec<u8> = Vec::new();
        let mut idx: Vec<u8> = Vec::new();
        self.calls.clear();
        for ClippedPrimitive { clip_rect, primitive } in &prims {
            let mesh = match primitive {
                Primitive::Mesh(m) => m,
                Primitive::Callback(_) => continue,
            };
            if mesh.indices.is_empty() {
                continue;
            }
            let base_vertex = (verts.len() / 20) as i32;
            let index_start = (idx.len() / 4) as u32;
            for v in &mesh.vertices {
                verts.extend_from_slice(&v.pos.x.to_le_bytes());
                verts.extend_from_slice(&v.pos.y.to_le_bytes());
                verts.extend_from_slice(&v.uv.x.to_le_bytes());
                verts.extend_from_slice(&v.uv.y.to_le_bytes());
                verts.extend_from_slice(&[v.color.r(), v.color.g(), v.color.b(), v.color.a()]);
            }
            for &i in &mesh.indices {
                idx.extend_from_slice(&i.to_le_bytes());
            }
            let index_end = (idx.len() / 4) as u32;
            // clip rect → physical pixels, clamped to the frame
            let ppp = self.pixels_per_point;
            let cx0 = (clip_rect.min.x * ppp).round().clamp(0.0, width as f32) as u32;
            let cy0 = (clip_rect.min.y * ppp).round().clamp(0.0, height as f32) as u32;
            let cx1 = (clip_rect.max.x * ppp).round().clamp(0.0, width as f32) as u32;
            let cy1 = (clip_rect.max.y * ppp).round().clamp(0.0, height as f32) as u32;
            if cx1 <= cx0 || cy1 <= cy0 {
                continue;
            }
            self.calls.push((index_start..index_end, base_vertex, [cx0, cy0, cx1 - cx0, cy1 - cy0], mesh.texture_id));
        }

        self.grow_and_write(device, queue, &verts, &idx);
        queue.write_buffer(
            &self.uniform_buf,
            0,
            bytemuck::bytes_of(&Uniform { screen_size_points: [points.x, points.y], _pad: [0.0; 2] }),
        );

        for id in &full.textures_delta.free {
            self.textures.remove(id);
        }
    }

    /// Paint the prepared HUD onto `view` (the finished frame). Must be called after `prepare`.
    pub fn paint(&self, encoder: &mut wgpu::CommandEncoder, view: &wgpu::TextureView, width: u32, height: u32) {
        if self.calls.is_empty() {
            return;
        }
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("egui-hud"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        rp.set_pipeline(&self.pipeline);
        rp.set_bind_group(0, &self.uniform_bind, &[]);
        rp.set_vertex_buffer(0, self.vbuf.slice(..));
        rp.set_index_buffer(self.ibuf.slice(..), wgpu::IndexFormat::Uint32);
        for (range, base_vertex, [x, y, w, h], tex_id) in &self.calls {
            let tex = match self.textures.get(tex_id) {
                Some(t) => t,
                None => continue,
            };
            // clamp scissor to the frame (wgpu validates bounds)
            let sx = (*x).min(width.saturating_sub(1));
            let sy = (*y).min(height.saturating_sub(1));
            let sw = (*w).min(width - sx);
            let sh = (*h).min(height - sy);
            if sw == 0 || sh == 0 {
                continue;
            }
            rp.set_scissor_rect(sx, sy, sw, sh);
            rp.set_bind_group(1, &tex.bind, &[]);
            rp.draw_indexed(range.clone(), *base_vertex, 0..1);
        }
    }

    fn grow_and_write(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, verts: &[u8], idx: &[u8]) {
        if verts.len() as u64 > self.vcap {
            self.vcap = (verts.len() as u64).next_power_of_two();
            self.vbuf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("egui-vbuf"),
                size: self.vcap,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if idx.len() as u64 > self.icap {
            self.icap = (idx.len() as u64).next_power_of_two();
            self.ibuf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("egui-ibuf"),
                size: self.icap,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        if !verts.is_empty() {
            queue.write_buffer(&self.vbuf, 0, verts);
        }
        if !idx.is_empty() {
            queue.write_buffer(&self.ibuf, 0, idx);
        }
    }

    fn set_textures(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, delta: &TexturesDelta) {
        for (id, image_delta) in &delta.set {
            // egui 0.34 delivers every image (incl. the font atlas) as RGBA Color.
            let img = match &image_delta.image {
                egui::ImageData::Color(img) => img,
            };
            let (w, h) = (img.size[0] as u32, img.size[1] as u32);
            let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();

            match image_delta.pos {
                // partial update (e.g. a newly-rasterised glyph) → write the sub-region into place
                Some([x, y]) => {
                    if let Some(tex) = self.textures.get(id) {
                        queue.write_texture(
                            wgpu::ImageCopyTexture {
                                texture: &tex.texture,
                                mip_level: 0,
                                origin: wgpu::Origin3d { x: x as u32, y: y as u32, z: 0 },
                                aspect: wgpu::TextureAspect::All,
                            },
                            &rgba,
                            wgpu::ImageDataLayout {
                                offset: 0,
                                bytes_per_row: Some(4 * w),
                                rows_per_image: Some(h),
                            },
                            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                        );
                    }
                }
                // full (re)create
                None => {
                    let size = wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 };
                    let texture = device.create_texture_with_data(
                        queue,
                        &wgpu::TextureDescriptor {
                            label: Some("egui-tex"),
                            size,
                            mip_level_count: 1,
                            sample_count: 1,
                            dimension: wgpu::TextureDimension::D2,
                            format: wgpu::TextureFormat::Rgba8UnormSrgb,
                            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                            view_formats: &[],
                        },
                        wgpu::util::TextureDataOrder::LayerMajor,
                        &rgba,
                    );
                    let view = texture.create_view(&Default::default());
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("egui-tex-bg"),
                        layout: &self.tex_bgl,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                        ],
                    });
                    self.textures.insert(*id, Tex { texture, bind });
                }
            }
        }
    }
}

// egui vertex colours are sRGB, non-premultiplied; convert to linear + premultiply so the One /
// OneMinusSrcAlpha blend above is correct. Standard egui-wgpu math.
const SHADER: &str = r#"
struct Uniform { screen_size: vec2<f32>, _pad: vec2<f32> };
@group(0) @binding(0) var<uniform> u: Uniform;
@group(1) @binding(0) var t_tex: texture_2d<f32>;
@group(1) @binding(1) var s_tex: sampler;

struct VOut {
  @builtin(position) pos: vec4<f32>,
  @location(0) uv: vec2<f32>,
  @location(1) color: vec4<f32>,
};

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
  let cutoff = c <= vec3<f32>(0.04045);
  let lower = c / 12.92;
  let higher = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
  return select(higher, lower, cutoff);
}

@vertex
fn vs_main(@location(0) a_pos: vec2<f32>, @location(1) a_uv: vec2<f32>, @location(2) a_col: vec4<f32>) -> VOut {
  var o: VOut;
  o.pos = vec4<f32>(2.0 * a_pos.x / u.screen_size.x - 1.0, 1.0 - 2.0 * a_pos.y / u.screen_size.y, 0.0, 1.0);
  o.uv = a_uv;
  // premultiplied linear
  let lin = srgb_to_linear(a_col.rgb);
  o.color = vec4<f32>(lin * a_col.a, a_col.a);
  return o;
}

@fragment
fn fs_main(in: VOut) -> @location(0) vec4<f32> {
  let tex = textureSample(t_tex, s_tex, in.uv);
  return in.color * tex;
}
"#;
