//! A small, self-contained wgpu renderer for egui's tessellated output — the same
//! approach unendlich takes (hand-rolled overlay) since egui-wgpu's versions don't
//! line up with our wgpu 22 / winit 0.29. Uploads egui's font/image textures and
//! draws its clipped meshes with per-primitive scissor over the final frame.

use std::collections::HashMap;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct U { screen: [f32; 2], _p: [f32; 2] }

pub struct EguiRender {
    pipeline: wgpu::RenderPipeline,
    ubuf: wgpu::Buffer,
    ubg: wgpu::BindGroup,
    tex_bgl: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    textures: HashMap<egui::TextureId, (wgpu::Texture, wgpu::BindGroup)>,
    vbuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
    vcap: u64,
    icap: u64,
}

const WGSL: &str = r#"
struct U { screen: vec2<f32>, pad: vec2<f32> };
@group(0) @binding(0) var<uniform> u: U;
@group(1) @binding(0) var tex: texture_2d<f32>;
@group(1) @binding(1) var samp: sampler;
struct VO { @builtin(position) pos: vec4<f32>, @location(0) uv: vec2<f32>, @location(1) col: vec4<f32> };
@vertex fn vs(@location(0) p: vec2<f32>, @location(1) uv: vec2<f32>, @location(2) c: vec4<f32>) -> VO {
  var o: VO;
  o.pos = vec4<f32>(2.0 * p.x / u.screen.x - 1.0, 1.0 - 2.0 * p.y / u.screen.y, 0.0, 1.0);
  o.uv = uv; o.col = c;  // egui colors are premultiplied sRGB; passthrough for an Unorm target
  return o;
}
@fragment fn fs(i: VO) -> @location(0) vec4<f32> {
  return i.col * textureSample(tex, samp, i.uv);
}
"#;

impl EguiRender {
    pub fn new(device: &wgpu::Device, target_format: wgpu::TextureFormat) -> Self {
        let sh = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("egui"), source: wgpu::ShaderSource::Wgsl(WGSL.into()) });
        let ubgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None, entries: &[wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::VERTEX, ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None }],
        });
        let tex_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None, entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT, ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
            ],
        });
        let ubuf = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: std::mem::size_of::<U>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let ubg = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &ubgl, entries: &[wgpu::BindGroupEntry { binding: 0, resource: ubuf.as_entire_binding() }] });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None, bind_group_layouts: &[&ubgl, &tex_bgl], push_constant_ranges: &[] });
        let blend = wgpu::BlendState { color: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::One, dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha, operation: wgpu::BlendOperation::Add }, alpha: wgpu::BlendComponent { src_factor: wgpu::BlendFactor::OneMinusDstAlpha, dst_factor: wgpu::BlendFactor::One, operation: wgpu::BlendOperation::Add } };
        let vlayout = wgpu::VertexBufferLayout {
            array_stride: 20, step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &[
                wgpu::VertexAttribute { offset: 0, shader_location: 0, format: wgpu::VertexFormat::Float32x2 },
                wgpu::VertexAttribute { offset: 8, shader_location: 1, format: wgpu::VertexFormat::Float32x2 },
                wgpu::VertexAttribute { offset: 16, shader_location: 2, format: wgpu::VertexFormat::Unorm8x4 },
            ],
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("egui"), layout: Some(&pl),
            vertex: wgpu::VertexState { module: &sh, entry_point: "vs", buffers: &[vlayout], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState { module: &sh, entry_point: "fs", targets: &[Some(wgpu::ColorTargetState { format: target_format, blend: Some(blend), write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
            primitive: wgpu::PrimitiveState::default(), depth_stencil: None, multisample: wgpu::MultisampleState::default(), multiview: None, cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor { label: None, mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });
        let vbuf = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 4096 * 20, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let ibuf = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: 4096 * 4, usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        EguiRender { pipeline, ubuf, ubg, tex_bgl, sampler, textures: HashMap::new(), vbuf, ibuf, vcap: 4096 * 20, icap: 4096 * 4 }
    }

    fn upload_texture(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, id: egui::TextureId, delta: &egui::epaint::ImageDelta) {
        let [w, h] = delta.image.size();
        let pixels: Vec<u8> = match &delta.image {
            egui::ImageData::Color(img) => img.pixels.iter().flat_map(|c| c.to_array()).collect(),
            egui::ImageData::Font(img) => img.srgba_pixels(None).flat_map(|c| c.to_array()).collect(),
        };
        // egui sends two kinds of updates for a texture id:
        //   * pos == None  -> a FULL image: the atlas was just created or GREW (height
        //                     doubled). (Re)create the GPU texture at this size and
        //                     upload the whole thing.
        //   * pos == Some  -> a PARTIAL update: only the sub-rect at `pos` changed
        //                     (newly rasterized glyphs). Write it into the EXISTING
        //                     texture; never recreate, or we'd throw away every glyph
        //                     already in the atlas and leave stale UVs pointing at a
        //                     tiny sub-image (garbled text after opening a submenu).
        match delta.pos {
            None => {
                let tex = device.create_texture(&wgpu::TextureDescriptor {
                    label: None, size: wgpu::Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 },
                    mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8Unorm, usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC, view_formats: &[],
                });
                let view = tex.create_view(&Default::default());
                let bg = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &self.tex_bgl, entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ] });
                queue.write_texture(wgpu::ImageCopyTexture { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                    &pixels, wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(4 * w as u32), rows_per_image: Some(h as u32) },
                    wgpu::Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 });
                self.textures.insert(id, (tex, bg));
            }
            Some([px, py]) => {
                if let Some((tex, _)) = self.textures.get(&id) {
                    queue.write_texture(wgpu::ImageCopyTexture { texture: tex, mip_level: 0, origin: wgpu::Origin3d { x: px as u32, y: py as u32, z: 0 }, aspect: wgpu::TextureAspect::All },
                        &pixels, wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(4 * w as u32), rows_per_image: Some(h as u32) },
                        wgpu::Extent3d { width: w as u32, height: h as u32, depth_or_array_layers: 1 });
                }
                // If the id isn't known yet, egui's protocol was violated; safe to ignore.
            }
        }
    }

    /// Draw egui output over `view`. Call inside the frame, after the game render.
    pub fn render(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, enc: &mut wgpu::CommandEncoder, view: &wgpu::TextureView,
                  size: [u32; 2], ppp: f32, prims: &[egui::ClippedPrimitive], deltas: &egui::TexturesDelta) {
        for (id, delta) in &deltas.set { self.upload_texture(device, queue, *id, delta); }
        queue.write_buffer(&self.ubuf, 0, bytemuck::bytes_of(&U { screen: [size[0] as f32 / ppp, size[1] as f32 / ppp], _p: [0.0; 2] }));

        // pack all meshes into the shared vertex/index buffers
        let mut verts: Vec<u8> = Vec::new();
        let mut idx: Vec<u32> = Vec::new();
        let mut batches: Vec<(egui::TextureId, std::ops::Range<u32>, [u32; 4])> = Vec::new();
        for p in prims {
            if let egui::epaint::Primitive::Mesh(mesh) = &p.primitive {
                let base = (verts.len() / 20) as u32;
                for v in &mesh.vertices {
                    verts.extend_from_slice(&v.pos.x.to_le_bytes());
                    verts.extend_from_slice(&v.pos.y.to_le_bytes());
                    verts.extend_from_slice(&v.uv.x.to_le_bytes());
                    verts.extend_from_slice(&v.uv.y.to_le_bytes());
                    verts.extend_from_slice(&v.color.to_array());
                }
                let start = idx.len() as u32;
                idx.extend(mesh.indices.iter().map(|i| i + base));
                let c = p.clip_rect;
                let sx = (c.min.x * ppp).max(0.0) as u32;
                let sy = (c.min.y * ppp).max(0.0) as u32;
                let sw = ((c.width() * ppp) as u32).min(size[0].saturating_sub(sx));
                let sh = ((c.height() * ppp) as u32).min(size[1].saturating_sub(sy));
                if sw > 0 && sh > 0 { batches.push((mesh.texture_id, start..idx.len() as u32, [sx, sy, sw, sh])); }
            }
        }
        if idx.is_empty() { return; }
        if (verts.len() as u64) > self.vcap { self.vbuf = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: verts.len() as u64, usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }); self.vcap = verts.len() as u64; }
        let ibytes = (idx.len() * 4) as u64;
        if ibytes > self.icap { self.ibuf = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: ibytes, usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }); self.icap = ibytes; }
        queue.write_buffer(&self.vbuf, 0, &verts);
        queue.write_buffer(&self.ibuf, 0, bytemuck::cast_slice(&idx));

        let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("egui"), color_attachments: &[Some(wgpu::RenderPassColorAttachment { view, resolve_target: None, ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None,
        });
        rp.set_pipeline(&self.pipeline);
        rp.set_bind_group(0, &self.ubg, &[]);
        rp.set_vertex_buffer(0, self.vbuf.slice(..));
        rp.set_index_buffer(self.ibuf.slice(..), wgpu::IndexFormat::Uint32);
        for (tid, range, [sx, sy, sw, sh]) in batches {
            if let Some((_, bg)) = self.textures.get(&tid) {
                rp.set_scissor_rect(sx, sy, sw, sh);
                rp.set_bind_group(1, bg, &[]);
                rp.draw_indexed(range, 0, 0..1);
            }
        }
        drop(rp);
        for id in &deltas.free { self.textures.remove(id); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn headless() -> (wgpu::Device, wgpu::Queue) {
        pollster::block_on(async {
            let inst = wgpu::Instance::new(wgpu::InstanceDescriptor { backends: wgpu::Backends::all(), ..Default::default() });
            let ad = inst.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::None, compatible_surface: None, force_fallback_adapter: false,
            }).await.expect("no adapter");
            ad.request_device(&wgpu::DeviceDescriptor {
                label: None, required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults(), memory_hints: wgpu::MemoryHints::default(),
            }, None).await.expect("no device")
        })
    }

    fn color_delta(w: usize, h: usize, c: egui::Color32, pos: Option<[usize; 2]>) -> egui::epaint::ImageDelta {
        egui::epaint::ImageDelta {
            image: egui::ImageData::Color(Arc::new(egui::ColorImage::new([w, h], c))),
            options: egui::TextureOptions::default(),
            pos,
        }
    }

    // Reproduces the on-device symptom: a full font-atlas upload, then a partial
    // glyph patch (what egui emits when a submenu rasterizes new glyphs). The patch
    // must land in place without discarding the rest of the atlas. The pre-fix code
    // replaced the whole atlas with the tiny patch texture -> garbled text.
    #[test]
    fn partial_atlas_update_preserves_atlas() {
        let (device, queue) = headless();
        let mut er = EguiRender::new(&device, wgpu::TextureFormat::Rgba8Unorm);
        let id = egui::TextureId::Managed(0);
        let a = egui::Color32::from_rgb(200, 100, 50); // original atlas fill
        let b = egui::Color32::from_rgb(10, 20, 30);   // the new glyph patch

        er.upload_texture(&device, &queue, id, &color_delta(64, 32, a, None));
        {
            let (tex, _) = er.textures.get(&id).expect("atlas missing after full upload");
            assert_eq!((tex.width(), tex.height()), (64, 32));
        }

        // Partial 4x4 patch at (10,5): must NOT shrink or replace the 64x32 atlas.
        er.upload_texture(&device, &queue, id, &color_delta(4, 4, b, Some([10, 5])));
        let (tex, _) = er.textures.get(&id).expect("atlas missing after partial update");
        assert_eq!((tex.width(), tex.height()), (64, 32),
            "partial update replaced the whole atlas (this is the garbled-text bug)");

        // Read the atlas back and confirm the patch landed while the rest survived.
        let bpr: usize = 64 * 4; // 256, already row-aligned
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: None, size: (bpr * 32) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false,
        });
        let mut enc = device.create_command_encoder(&Default::default());
        enc.copy_texture_to_buffer(
            wgpu::ImageCopyTexture { texture: &tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::ImageCopyBuffer { buffer: &buf, layout: wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(bpr as u32), rows_per_image: Some(32) } },
            wgpu::Extent3d { width: 64, height: 32, depth_or_array_layers: 1 });
        queue.submit([enc.finish()]);
        buf.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        device.poll(wgpu::Maintain::Wait);
        let data = buf.slice(..).get_mapped_range();
        let px = |x: usize, y: usize| { let o = y * bpr + x * 4; [data[o], data[o + 1], data[o + 2], data[o + 3]] };

        assert_eq!(px(0, 0), a.to_array(), "top-left of atlas should keep its original glyphs");
        assert_eq!(px(60, 30), a.to_array(), "far corner of atlas should keep its original glyphs");
        assert_eq!(px(9, 7), a.to_array(), "pixel just outside the patch should be untouched");
        assert_eq!(px(12, 7), b.to_array(), "patch interior should hold the new glyph");
    }
}
