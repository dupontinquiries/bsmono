//! renderer.rs — skydribble's object pass, transplanted into egui. The shape of the port:
//!
//!   skydribble (wgpu 22, owns the surface)      CADscope (wgpu 29, guest in egui's pass)
//!   ─────────────────────────────────────       ────────────────────────────────────────
//!   winit surface + own render pass          →  eframe(wgpu) pass, `depth_buffer: 32`
//!   Depth32Float, MSAA 4                     →  Depth32Float (egui-owned), MSAA 1 (PARITY)
//!   one dynamic-offset object UBO, 256 B     →  identical — `ObjectRaw` layout kept verbatim
//!   params.w = "rt self id + 1"              →  params.w = pick_id + 1 (same slot, same +1)
//!
//! Persistent GPU state lives in `ViewRenderer`, stored inside egui-wgpu's `CallbackResources`
//! type-map (the custom3d_wgpu pattern); per-frame data travels in `ViewportCallback`, which the
//! app hands to `egui_wgpu::Callback::new_paint_callback` each frame. `prepare` uploads, `paint`
//! draws into egui's own render pass — no second surface, no texture round-trip.

use std::collections::HashMap;
use std::sync::Arc;

use wgpu::util::DeviceExt;

use crate::meshgen::{self, MeshData, Vertex};

/// One object uniform slot. Byte-identical to skydribble's `ObjectRaw`: 64 floats = 256 bytes,
/// which is both a universally supported dynamic-offset alignment and the stride the WGSL `Obj`
/// struct pads to. Do not reorder; the shader and the stride math both assume this.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ObjectRaw {
    pub model: [f32; 16],
    pub nrm: [f32; 16],
    pub color: [f32; 4],
    /// x mode (0 solid, 1 ground grid), y emissive, z spec, w pick_id + 1.
    pub params: [f32; 4],
    pub _pad: [f32; 24],
}

impl Default for ObjectRaw {
    fn default() -> Self {
        ObjectRaw { model: [0.0; 16], nrm: [0.0; 16], color: [1.0; 4], params: [0.0; 4], _pad: [0.0; 24] }
    }
}

pub const OBJ_STRIDE: u64 = std::mem::size_of::<ObjectRaw>() as u64;

/// Frame-global uniforms; mirrors the WGSL `Globals` struct field-for-field.
#[repr(C)]
#[derive(Clone, Copy, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Globals {
    pub view_proj: [f32; 16],
    pub inv_view_proj: [f32; 16],
    /// xyz eye, w time in seconds (drives the selection breathe).
    pub cam: [f32; 4],
    /// xyz TO-sun direction (normalize on the CPU), w intensity.
    pub sun: [f32; 4],
    /// rgb sky/fog colour, w density.
    pub fog: [f32; 4],
    /// x selected pick_id + 1 (0 = nothing selected).
    pub misc: [f32; 4],
}

/// Which cached mesh a draw uses. Parametric primitives that can't be expressed as a pure scale of
/// a unit mesh (capsule cap/torus tube proportions) carry a quantized ratio in the key so each
/// distinct proportion gets its own cached mesh instead of a per-frame rebuild.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum MeshKind {
    Cuboid,
    Sphere,
    Cylinder,
    /// tube/major ratio in thousandths.
    Torus { ratio_milli: u32 },
    /// cylindrical half-height / radius in thousandths.
    Capsule { hh_milli: u32 },
    GroundSlab,
    /// A mesh built by the kernel's dual contourer rather than a parametric generator — the CSG
    /// path. The key identifies it (part id today); the data rides along on the first
    /// `FrameDraw` that names it and is cached until the epoch bumps.
    Generated(u64),
}

fn build_mesh(kind: MeshKind) -> MeshData {
    match kind {
        MeshKind::Cuboid | MeshKind::GroundSlab => meshgen::boxx(),
        MeshKind::Sphere => meshgen::sphere(28, 44),
        MeshKind::Cylinder => meshgen::cylinder(48),
        MeshKind::Torus { ratio_milli } => meshgen::torus(ratio_milli as f32 / 1000.0, 64, 28),
        MeshKind::Capsule { hh_milli } => meshgen::capsule(hh_milli as f32 / 1000.0, 12, 32),
        // Never reached: generated meshes arrive as data on the draw, not built from a key.
        MeshKind::Generated(_) => MeshData::new(),
    }
}

/// One instance to draw this frame. `mesh` is `Some` only for a `Generated` kind the renderer may
/// not have cached yet; cloning an `Arc` per frame is the whole cost of carrying it.
#[derive(Clone)]
pub struct FrameDraw {
    pub kind: MeshKind,
    pub object: ObjectRaw,
    pub mesh: Option<Arc<MeshData>>,
}

struct GpuMesh {
    vbuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
    index_count: u32,
}

/// Persistent GPU state: pipeline, uniform buffers, mesh cache. Lives in egui-wgpu's
/// `CallbackResources`; created once at app startup from the eframe `RenderState`.
pub struct ViewRenderer {
    pipeline: wgpu::RenderPipeline,
    globals_buf: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    obj_layout: wgpu::BindGroupLayout,
    obj_buf: wgpu::Buffer,
    obj_bg: wgpu::BindGroup,
    obj_capacity: u32,
    /// The material atlas: one array texture, one layer per loaded material. Assigning a material
    /// to a part is then an index in its object uniform rather than a bind group swap, so a scene
    /// with a dozen materials still draws in one pass.
    mat_bg: wgpu::BindGroup,
    mat_layers: u32,
    mat_layout: wgpu::BindGroupLayout,
    meshes: HashMap<MeshKind, GpuMesh>,
    /// Bumped by the app whenever the model is re-evaluated; generated meshes are dropped on
    /// change so an edited document cannot leak GPU buffers for geometry that no longer exists.
    epoch: u64,
}

impl ViewRenderer {
    /// `depth_format` must match what eframe gave egui's pass — `depth_buffer: 32` in
    /// `NativeOptions` maps to `Depth32Float` (egui_wgpu::depth_format_from_bits). `samples`
    /// likewise must match the pass (eframe default multisampling ⇒ 1).
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        target_format: wgpu::TextureFormat,
        depth_format: Option<wgpu::TextureFormat>,
        samples: u32,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("cadscope-view"),
            source: wgpu::ShaderSource::Wgsl(crate::SHADER.into()),
        });

        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cadscope-globals"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Globals>() as u64),
                },
                count: None,
            }],
        });
        let obj_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cadscope-object"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(OBJ_STRIDE),
                },
                count: None,
            }],
        });
        let mat_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cadscope-materials"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
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
                // norm_ and spec_ maps, same layer indexing as the albedo.
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2Array,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cadscope-lit"),
            bind_group_layouts: &[Some(&globals_layout), Some(&obj_layout), Some(&mat_layout)],
            immediate_size: 0,
        });

        let attrs = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2];
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("cadscope-lit"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_main"),
                compilation_options: Default::default(),
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Vertex>() as u64,
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &attrs,
                }],
            },
            primitive: wgpu::PrimitiveState {
                cull_mode: Some(wgpu::Face::Back),
                ..Default::default()
            },
            depth_stencil: depth_format.map(|format| wgpu::DepthStencilState {
                format,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: samples.max(1), ..Default::default() },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: target_format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });

        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cadscope-globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cadscope-globals"),
            layout: &globals_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: globals_buf.as_entire_binding() }],
        });

        // A one-layer placeholder so the pipeline is always bindable, even before any material is
        // loaded. Mid-grey rather than white: a missing texture should look plainly wrong, not
        // like a correctly lit unpainted surface.
        // Flat placeholders: mid-grey albedo, a normal map meaning "no perturbation" (0,0,1
        // encoded), and full specular. A material that fails to load then looks unpainted rather
        // than looking lit by a broken normal.
        let (mat_bg, mat_layers) = Self::make_atlas(
            device,
            queue,
            &mat_layout,
            &[vec![128u8; 4]],
            &[vec![128, 128, 255, 255]],
            &[vec![255u8; 4]],
            1,
        );

        let obj_capacity = 256u32;
        let (obj_buf, obj_bg) = Self::make_obj_buffer(device, &obj_layout, obj_capacity);

        ViewRenderer {
            pipeline,
            globals_buf,
            globals_bg,
            obj_layout,
            obj_buf,
            obj_bg,
            obj_capacity,
            mat_bg,
            mat_layers,
            mat_layout,
            meshes: HashMap::new(),
            epoch: 0,
        }
    }

    fn make_gpu_mesh(device: &wgpu::Device, m: &MeshData) -> GpuMesh {
        GpuMesh {
            vbuf: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("cadscope-mesh-v"),
                contents: bytemuck::cast_slice(&m.verts),
                usage: wgpu::BufferUsages::VERTEX,
            }),
            ibuf: device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("cadscope-mesh-i"),
                contents: bytemuck::cast_slice(&m.idx),
                usage: wgpu::BufferUsages::INDEX,
            }),
            index_count: m.idx.len() as u32,
        }
    }

    /// Build the atlas from RGBA layers, each `size` square. Replacing it wholesale is simplest and
    /// costs nothing that matters: materials are loaded when a document opens, not per frame.
    fn array_texture(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layers: &[Vec<u8>],
        size: u32,
        srgb: bool,
        label: &str,
    ) -> wgpu::TextureView {
        let count = layers.len().max(1) as u32;
        let tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: count },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // Albedo is colour and wants sRGB decode; a normal map is a vector and a specular map
            // is a mask, and decoding either as colour would bend the numbers.
            format: if srgb { wgpu::TextureFormat::Rgba8UnormSrgb } else { wgpu::TextureFormat::Rgba8Unorm },
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (i, data) in layers.iter().enumerate() {
            let expect = (size * size * 4) as usize;
            if data.len() != expect {
                log::error!("material layer {i} is {} bytes, expected {expect}", data.len());
                continue;
            }
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &tex,
                    mip_level: 0,
                    origin: wgpu::Origin3d { x: 0, y: 0, z: i as u32 },
                    aspect: wgpu::TextureAspect::All,
                },
                data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size * 4),
                    rows_per_image: Some(size),
                },
                wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 1 },
            );
        }
        tex.create_view(&wgpu::TextureViewDescriptor {
            dimension: Some(wgpu::TextureViewDimension::D2Array),
            ..Default::default()
        })
    }

    /// Build the three-map atlas. `albedo`, `normal` and `spec` are parallel: layer *i* of each
    /// belongs to the same material, which is what lets one index select all three.
    fn make_atlas(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        layout: &wgpu::BindGroupLayout,
        albedo: &[Vec<u8>],
        normal: &[Vec<u8>],
        spec: &[Vec<u8>],
        size: u32,
    ) -> (wgpu::BindGroup, u32) {
        let count = albedo.len().max(1) as u32;
        let view = Self::array_texture(device, queue, albedo, size, true, "cadscope-albedo");
        let nview = Self::array_texture(device, queue, normal, size, false, "cadscope-normal");
        let sview = Self::array_texture(device, queue, spec, size, false, "cadscope-spec");
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("cadscope-material"),
            // Repeat, because triplanar coordinates run well past [0,1] and the textures tile.
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cadscope-materials"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&nview) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&sview) },
            ],
        });
        (bg, count)
    }

    /// Replace the atlas with a new set of materials. Layer order is what `params.y` indexes.
    pub fn set_materials(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        albedo: &[Vec<u8>],
        normal: &[Vec<u8>],
        spec: &[Vec<u8>],
        size: u32,
    ) {
        let (bg, count) = Self::make_atlas(device, queue, &self.mat_layout, albedo, normal, spec, size);
        self.mat_bg = bg;
        self.mat_layers = count;
    }

    pub fn material_count(&self) -> u32 {
        self.mat_layers
    }

    fn make_obj_buffer(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        capacity: u32,
    ) -> (wgpu::Buffer, wgpu::BindGroup) {
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cadscope-objects"),
            size: OBJ_STRIDE * capacity as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cadscope-objects"),
            layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(OBJ_STRIDE),
                }),
            }],
        });
        (buf, bg)
    }

    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, cb: &ViewportCallback) {
        // Generated meshes live exactly as long as something draws them. That covers a document
        // edit (old part keys vanish) and, more sharply, Grow mode's live preview, which hands over
        // a fresh key every time it re-meshes.
        if cb.epoch != self.epoch || cb.draws.iter().any(|d| matches!(d.kind, MeshKind::Generated(_))) {
            let live: std::collections::HashSet<MeshKind> = cb.draws.iter().map(|d| d.kind).collect();
            self.meshes.retain(|k, _| !matches!(k, MeshKind::Generated(_)) || live.contains(k));
            self.epoch = cb.epoch;
        }
        if cb.draws.len() as u32 > self.obj_capacity {
            self.obj_capacity = (cb.draws.len() as u32).next_power_of_two();
            let (buf, bg) = Self::make_obj_buffer(device, &self.obj_layout, self.obj_capacity);
            self.obj_buf = buf;
            self.obj_bg = bg;
        }
        for d in &cb.draws {
            if self.meshes.contains_key(&d.kind) {
                continue;
            }
            let gpu = match (&d.kind, &d.mesh) {
                (MeshKind::Generated(_), Some(m)) => Self::make_gpu_mesh(device, m),
                // A generated kind with no data yet: skip, and pick it up on a later frame.
                (MeshKind::Generated(_), None) => continue,
                (kind, _) => Self::make_gpu_mesh(device, &build_mesh(**&kind)),
            };
            self.meshes.insert(d.kind, gpu);
        }
        queue.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(&cb.globals));
        if !cb.draws.is_empty() {
            // ObjectRaw is exactly OBJ_STRIDE bytes, so a contiguous slice IS the strided layout.
            let objs: Vec<ObjectRaw> = cb.draws.iter().map(|d| d.object).collect();
            queue.write_buffer(&self.obj_buf, 0, bytemuck::cast_slice(&objs));
        }
    }

    fn draw(&self, render_pass: &mut wgpu::RenderPass<'_>, draws: &[FrameDraw]) {
        render_pass.set_pipeline(&self.pipeline);
        render_pass.set_bind_group(0, &self.globals_bg, &[]);
        render_pass.set_bind_group(2, &self.mat_bg, &[]);
        for (i, d) in draws.iter().enumerate() {
            let Some(mesh) = self.meshes.get(&d.kind) else { continue };
            render_pass.set_bind_group(1, &self.obj_bg, &[i as u32 * OBJ_STRIDE as u32]);
            render_pass.set_vertex_buffer(0, mesh.vbuf.slice(..));
            render_pass.set_index_buffer(mesh.ibuf.slice(..), wgpu::IndexFormat::Uint32);
            render_pass.draw_indexed(0..mesh.index_count, 0, 0..1);
        }
    }
}

/// Per-frame paint payload: globals + the draw list. Add via
/// `ui.painter().add(egui_wgpu::Callback::new_paint_callback(rect, callback))`.
pub struct ViewportCallback {
    pub globals: Globals,
    pub draws: Vec<FrameDraw>,
    /// Increment when the model is re-evaluated; see `ViewRenderer::epoch`.
    pub epoch: u64,
}

impl egui_wgpu::CallbackTrait for ViewportCallback {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &egui_wgpu::ScreenDescriptor,
        _egui_encoder: &mut wgpu::CommandEncoder,
        callback_resources: &mut egui_wgpu::CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        if let Some(r) = callback_resources.get_mut::<ViewRenderer>() {
            r.upload(device, queue, self);
        } else {
            log::error!("ViewRenderer missing from CallbackResources — was it inserted at startup?");
        }
        Vec::new()
    }

    fn paint(
        &self,
        _info: egui::PaintCallbackInfo,
        render_pass: &mut wgpu::RenderPass<'static>,
        callback_resources: &egui_wgpu::CallbackResources,
    ) {
        if let Some(r) = callback_resources.get::<ViewRenderer>() {
            r.draw(render_pass, &self.draws);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn object_raw_is_exactly_one_dynamic_offset_slot() {
        assert_eq!(std::mem::size_of::<ObjectRaw>(), 256);
        assert_eq!(OBJ_STRIDE, 256);
    }

    #[test]
    fn globals_matches_the_wgsl_struct_size() {
        // 2 mat4 + 4 vec4 = (32 + 16) floats = 192 bytes.
        assert_eq!(std::mem::size_of::<Globals>(), 192);
    }

    #[test]
    fn parametric_mesh_keys_are_distinct() {
        use std::collections::HashSet;
        let keys: HashSet<MeshKind> = [
            MeshKind::Torus { ratio_milli: 350 },
            MeshKind::Torus { ratio_milli: 200 },
            MeshKind::Capsule { hh_milli: 800 },
            MeshKind::Generated(7),
            MeshKind::Generated(8),
        ]
        .into_iter()
        .collect();
        assert_eq!(keys.len(), 5);
    }
}
