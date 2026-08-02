//! Offscreen render of an asset loaded by skydribble's own loader, through a
//! minimal wgpu pipeline (the game's swapchain path needs a window; this
//! harness needs only an adapter). Writes a PNG.
use std::path::PathBuf;
use wgpu::util::DeviceExt;

fn main() {
    let dir = PathBuf::from(std::env::args().nth(1).expect("usage: botshot <asset dir> <out.png>"));
    let out = PathBuf::from(std::env::args().nth(2).unwrap_or("botshot.png".into()));
    let (raws, half) = skydribble::model::load_raw(&dir, 4.2);

    let instance = wgpu::Instance::default();
    let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        force_fallback_adapter: false,
        compatible_surface: None,
    })) {
        Some(a) => a,
        None => {
            eprintln!("botshot: no wgpu adapter available in this environment; \
                       the asset loaded fine ({} prims) — run on hardware for pixels", raws.len());
            std::process::exit(2);
        }
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default(), None))
        .expect("device");

    let (w, h) = (960u32, 540u32);
    let color = device.create_texture(&wgpu::TextureDescriptor {
        label: None, size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let depth = device.create_texture(&wgpu::TextureDescriptor {
        label: None, size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Depth32Float,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT, view_formats: &[],
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None,
        source: wgpu::ShaderSource::Wgsl(r#"
struct U { mvp: mat4x4<f32>, color: vec4<f32> };
@group(0) @binding(0) var<uniform> u: U;
struct VOut { @builtin(position) pos: vec4<f32>, @location(0) nrm: vec3<f32> };
@vertex fn vs(@location(0) p: vec3<f32>, @location(1) n: vec3<f32>, @location(2) _uv: vec2<f32>) -> VOut {
  var o: VOut; o.pos = u.mvp * vec4<f32>(p, 1.0); o.nrm = n; return o;
}
@fragment fn fs(v: VOut) -> @location(0) vec4<f32> {
  let l = normalize(vec3<f32>(0.5, 0.8, 0.35));
  let lit = 0.35 + 0.55 * max(dot(normalize(v.nrm), l), 0.0);
  return vec4<f32>(u.color.rgb * lit, 1.0);
}"#.into()),
    });

    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0, visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false,
                min_binding_size: None,
            }, count: None,
        }],
    });
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None, bind_group_layouts: &[&bgl], push_constant_ranges: &[],
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None, layout: Some(&pl),
        vertex: wgpu::VertexState {
            module: &shader, entry_point: "vs",
            compilation_options: Default::default(),
            buffers: &[wgpu::VertexBufferLayout {
                array_stride: 32,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
            }],
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader, entry_point: "fs",
            compilation_options: Default::default(),
            targets: &[Some(wgpu::TextureFormat::Rgba8UnormSrgb.into())],
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: Some(wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: Default::default(), bias: Default::default(),
        }),
        multisample: Default::default(), multiview: None, cache: None,
    });

    // camera: 3/4 view framing the half extents
    let eye = glam::Vec3::new(half.x * 1.6 + 2.0, half.y * 1.1 + 1.2, half.z * 1.6 + 2.0);
    let view = glam::Mat4::look_at_rh(eye, glam::Vec3::new(0.0, half.y * 0.15, 0.0), glam::Vec3::Y);
    let proj = glam::Mat4::perspective_rh(0.9, w as f32 / h as f32, 0.05, 100.0);
    let mvp = proj * view;

    let mut encoder = device.create_command_encoder(&Default::default());
    let cview = color.create_view(&Default::default());
    let dview = depth.create_view(&Default::default());
    let mut ubufs = Vec::new();
    let mut vbufs = Vec::new();
    for r in &raws {
        let vraw: Vec<u8> = r.verts.iter().flat_map(|v| {
            let mut b = Vec::with_capacity(32);
            for f in v.pos { b.extend_from_slice(&f.to_le_bytes()); }
            for f in v.nrm { b.extend_from_slice(&f.to_le_bytes()); }
            for f in v.uv { b.extend_from_slice(&f.to_le_bytes()); }
            b
        }).collect();
        let vb = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None, contents: &vraw, usage: wgpu::BufferUsages::VERTEX,
        });
        let ib = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None, contents: bytemuck::cast_slice(&r.idx), usage: wgpu::BufferUsages::INDEX,
        });
        let mut ur = [0u8; 80];
        ur[..64].copy_from_slice(bytemuck::cast_slice(&mvp.to_cols_array()));
        ur[64..].copy_from_slice(bytemuck::cast_slice(&r.color));
        let ub = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None, contents: &ur, usage: wgpu::BufferUsages::UNIFORM,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None, layout: &bgl,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: ub.as_entire_binding() }],
        });
        ubufs.push((bg, r.idx.len() as u32));
        vbufs.push((vb, ib));
    }
    {
        let mut rp = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &cview, resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.05, g: 0.06, b: 0.08, a: 1.0 }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &dview,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: None,
            }),
            timestamp_writes: None, occlusion_query_set: None,
        });
        rp.set_pipeline(&pipeline);
        for (i, (bg, n)) in ubufs.iter().enumerate() {
            rp.set_bind_group(0, bg, &[]);
            rp.set_vertex_buffer(0, vbufs[i].0.slice(..));
            rp.set_index_buffer(vbufs[i].1.slice(..), wgpu::IndexFormat::Uint32);
            rp.draw_indexed(0..*n, 0, 0..1);
        }
    }
    let bpr = (w * 4 + 255) / 256 * 256;
    let read = device.create_buffer(&wgpu::BufferDescriptor {
        label: None, size: (bpr * h) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    encoder.copy_texture_to_buffer(
        color.as_image_copy(),
        wgpu::ImageCopyBuffer {
            buffer: &read,
            layout: wgpu::ImageDataLayout {
                offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    queue.submit([encoder.finish()]);
    let slice = read.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::Maintain::Wait);
    let data = slice.get_mapped_range();
    let mut img = image::RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let o = (y * bpr + x * 4) as usize;
            img.put_pixel(x, y, image::Rgba([data[o], data[o + 1], data[o + 2], 255]));
        }
    }
    img.save(&out).expect("save png");
    println!("botshot: rendered {} prims via {:?} -> {}", raws.len(), adapter.get_info().backend, out.display());
}
