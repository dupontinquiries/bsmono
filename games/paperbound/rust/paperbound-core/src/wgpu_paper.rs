//! wgpu_paper.rs — slice 12: the portable GPU renderer.
//!
//! `gpu.rs` already flattens a papercraft `Mesh` into interleaved pos/nrm/col
//! buffers; what it lacked was anything that could actually draw them. The
//! previous on-ramp targeted the monorepo's Vulkan engine, which meant the GPU
//! path only existed on a machine with a current toolchain and a Vulkan loader.
//! wgpu replaces that: ONE source covers Metal (the M2 Ultra) and Vulkan (the
//! Asahi M2), and its shader front-end (naga) lets the WGSL be validated in a
//! unit test with no GPU, no window, and no CI runner tax.
//!
//! The module is split so the untestable part is as small as possible:
//!
//!   * `wgpu-shader` (CPU only) — the WGSL source, the `Camera` uniform and its
//!     byte packing, and the vertex layout description. All of it verifiable
//!     headlessly: naga parses and validates the shader, and the uniform's
//!     layout is asserted against the std140-ish rules WGSL expects.
//!   * `wgpu-render` (needs a device) — adapter/device acquisition, buffer
//!     upload, pipeline construction, and an offscreen render to a texture that
//!     can be read back as an image.
//!
//! Style contract: see `shaders/paper.wgsl`. Hard facets, candlelit key, cool
//! fill, ink edges, world-anchored paper fibre. The uniform defaults below are
//! the same desk lighting the software renderer uses, so the two paths agree.

/// The WGSL source, compiled into the binary so a shipped build cannot lose it.
pub const PAPER_WGSL: &str = include_str!("shaders/paper.wgsl");

/// Camera + lighting uniform. Field order and padding match the WGSL `Camera`
/// struct exactly; every member is a 16-byte vec4 so the layout is trivially
/// std140-compatible and the same bytes work on Metal and Vulkan.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "wgpu-shader", derive(bytemuck::Pod, bytemuck::Zeroable))]
pub struct CameraUniform {
    pub view_proj: [[f32; 4]; 4],
    /// xyz = eye position, w unused
    pub eye: [f32; 4],
    /// xyz = key direction (normalized), w = key intensity
    pub key_dir: [f32; 4],
    /// rgb = key colour, a = ambient amount
    pub key_col: [f32; 4],
    /// rgb = fill colour, a = rim amount
    pub fill_col: [f32; 4],
    /// rgb = ink colour, a = paper-grain amount
    pub ink_col: [f32; 4],
}

impl Default for CameraUniform {
    /// The candlelit desk: a warm key low from the left, a dim cool room fill
    /// from the right, ink for the creases, and a whisper of grain. These are
    /// the software renderer's numbers so both paths light a model the same.
    fn default() -> Self {
        CameraUniform {
            view_proj: IDENTITY,
            eye: [0.0, 3.0, 6.0, 0.0],
            key_dir: [-0.55, 0.70, 0.45, 1.15],
            key_col: [1.00, 0.86, 0.62, 0.34],
            fill_col: [0.42, 0.52, 0.72, 0.30],
            ink_col: [0.10, 0.08, 0.09, 0.10],
        }
    }
}

pub const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

impl CameraUniform {
    /// Size in bytes, as the GPU sees it.
    pub const SIZE: usize = std::mem::size_of::<CameraUniform>();

    /// A right-handed look-at + perspective, baked to `view_proj`. Written by
    /// hand rather than pulling a math crate: six vec4s and one matrix is not
    /// worth a dependency, and this keeps the port std-only.
    pub fn set_camera(&mut self, eye: [f32; 3], target: [f32; 3], up: [f32; 3], fov_y: f32, aspect: f32, near: f32, far: f32) {
        let f = norm(sub(target, eye));
        let s = norm(cross(f, up));
        let u = cross(s, f);
        let view = [
            [s[0], u[0], -f[0], 0.0],
            [s[1], u[1], -f[1], 0.0],
            [s[2], u[2], -f[2], 0.0],
            [-dot(s, eye), -dot(u, eye), dot(f, eye), 1.0],
        ];
        let t = 1.0 / (fov_y * 0.5).tan();
        // wgpu clip space is 0..1 in Z (like Metal/D3D), NOT -1..1 like GL —
        // getting this wrong is the classic "everything z-fights or vanishes".
        let proj = [
            [t / aspect, 0.0, 0.0, 0.0],
            [0.0, t, 0.0, 0.0],
            [0.0, 0.0, far / (near - far), -1.0],
            [0.0, 0.0, near * far / (near - far), 0.0],
        ];
        self.view_proj = mat_mul(proj, view);
        self.eye = [eye[0], eye[1], eye[2], 0.0];
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] { [a[0] - b[0], a[1] - b[1], a[2] - b[2]] }
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 { a[0] * b[0] + a[1] * b[1] + a[2] * b[2] }
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}
fn norm(a: [f32; 3]) -> [f32; 3] {
    let l = dot(a, a).sqrt().max(1e-6);
    [a[0] / l, a[1] / l, a[2] / l]
}
fn mat_mul(a: [[f32; 4]; 4], b: [[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut o = [[0.0f32; 4]; 4];
    for (i, orow) in o.iter_mut().enumerate() {
        for (j, ocell) in orow.iter_mut().enumerate() {
            let mut s = 0.0;
            for k in 0..4 {
                s += a[k][j] * b[i][k];
            }
            *ocell = s;
        }
    }
    o
}

/// The vertex layout `SceneBuffers` produces: pos(3) + nrm(3) + col(3) f32,
/// tightly interleaved. Returned as plain data so it can be asserted in a test
/// and consumed by the device path without duplicating the offsets.
pub struct VertexLayout {
    pub stride: u64,
    /// (shader_location, byte_offset, component_count)
    pub attrs: [(u32, u64, u32); 3],
}

pub const VERTEX_LAYOUT: VertexLayout = VertexLayout {
    stride: (9 * 4) as u64,
    attrs: [(0, 0, 3), (1, 12, 3), (2, 24, 3)],
};

/// Validate the WGSL with naga — the same front-end wgpu uses, so a pass here
/// means the shader will compile on the device too. Returns the entry-point
/// names on success so a test can assert the pipeline's expectations.
#[cfg(feature = "wgpu-shader")]
pub fn validate_shader() -> Result<Vec<String>, String> {
    let module = naga::front::wgsl::parse_str(PAPER_WGSL).map_err(|e| format!("WGSL parse: {e}"))?;
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::empty(),
    );
    validator.validate(&module).map_err(|e| format!("WGSL validate: {e}"))?;
    Ok(module.entry_points.iter().map(|e| e.name.clone()).collect())
}

// ── the device path ─────────────────────────────────────────────────────────
#[cfg(feature = "wgpu-render")]
pub mod device {
    //! Everything that needs a real adapter. Kept behind its own feature so the
    //! default build — and every CI run — stays CPU-only and fast.

    use super::{CameraUniform, PAPER_WGSL, VERTEX_LAYOUT};
    use crate::gpu::SceneBuffers;

    pub struct PaperRenderer {
        pub device: wgpu::Device,
        pub queue: wgpu::Queue,
        pipeline: wgpu::RenderPipeline,
        bind_group: wgpu::BindGroup,
        camera_buf: wgpu::Buffer,
        format: wgpu::TextureFormat,
    }

    impl PaperRenderer {
        /// Acquire an adapter and build the pipeline. `format` is the target
        /// texture format (Rgba8UnormSrgb for offscreen capture, or the
        /// surface's preferred format when drawing to a window).
        pub async fn new(format: wgpu::TextureFormat) -> Result<Self, String> {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions::default())
                .await
                .ok_or_else(|| "no wgpu adapter (no GPU and no software fallback)".to_string())?;
            let (device, queue) = adapter
                .request_device(&wgpu::DeviceDescriptor::default(), None)
                .await
                .map_err(|e| format!("request_device: {e}"))?;

            let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("paper"),
                source: wgpu::ShaderSource::Wgsl(PAPER_WGSL.into()),
            });
            let camera_buf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("camera"),
                size: std::mem::size_of::<CameraUniform>() as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("paper-bgl"),
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                }],
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("paper-bg"),
                layout: &bgl,
                entries: &[wgpu::BindGroupEntry { binding: 0, resource: camera_buf.as_entire_binding() }],
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("paper-pl"),
                bind_group_layouts: &[&bgl],
                push_constant_ranges: &[],
            });
            let attrs = [
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: VERTEX_LAYOUT.attrs[0].1, shader_location: 0 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: VERTEX_LAYOUT.attrs[1].1, shader_location: 1 },
                wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: VERTEX_LAYOUT.attrs[2].1, shader_location: 2 },
            ];
            let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("paper-pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: "vs_main",
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: VERTEX_LAYOUT.stride,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attrs,
                    }],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: "fs_main",
                    targets: &[Some(format.into())],
                    compilation_options: Default::default(),
                }),
                // Back-face culling OFF: a folded sheet is a SHEET. Both sides of
                // a facet are legitimately visible on a paper model, and culling
                // them punches holes in wings and fins.
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
                cache: None,
            });
            Ok(PaperRenderer { device, queue, pipeline, bind_group, camera_buf, format })
        }

        pub fn format(&self) -> wgpu::TextureFormat { self.format }

        /// Render one scene offscreen and read it back as RGBA8 rows.
        pub fn render_offscreen(&self, scene: &SceneBuffers, cam: &CameraUniform, w: u32, h: u32) -> Vec<u8> {
            use wgpu::util::DeviceExt;
            self.queue.write_buffer(&self.camera_buf, 0, bytemuck::bytes_of(cam));
            let vbuf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("verts"),
                contents: bytemuck::cast_slice(&scene.vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            let ibuf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("idx"),
                contents: bytemuck::cast_slice(&scene.indices),
                usage: wgpu::BufferUsages::INDEX,
            });
            let target = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("target"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            let depth = self.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("depth"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1, sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Depth32Float,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let tv = target.create_view(&Default::default());
            let dv = depth.create_view(&Default::default());
            let bytes_per_row = (w * 4).div_ceil(256) * 256;
            let readback = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("readback"),
                size: (bytes_per_row * h) as u64,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut enc = self.device.create_command_encoder(&Default::default());
            {
                let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("paper-pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &tv,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            // the desk itself — warm dark, not black
                            load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.06, g: 0.05, b: 0.04, a: 1.0 }),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &dv,
                        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
                rp.set_pipeline(&self.pipeline);
                rp.set_bind_group(0, &self.bind_group, &[]);
                rp.set_vertex_buffer(0, vbuf.slice(..));
                rp.set_index_buffer(ibuf.slice(..), wgpu::IndexFormat::Uint32);
                rp.draw_indexed(0..scene.indices.len() as u32, 0, 0..1);
            }
            enc.copy_texture_to_buffer(
                wgpu::ImageCopyTexture { texture: &target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                wgpu::ImageCopyBuffer {
                    buffer: &readback,
                    layout: wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(bytes_per_row), rows_per_image: Some(h) },
                },
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            );
            self.queue.submit(Some(enc.finish()));
            let slice = readback.slice(..);
            slice.map_async(wgpu::MapMode::Read, |_| {});
            self.device.poll(wgpu::Maintain::Wait);
            let data = slice.get_mapped_range();
            let mut out = Vec::with_capacity((w * h * 4) as usize);
            for row in 0..h {
                let start = (row * bytes_per_row) as usize;
                out.extend_from_slice(&data[start..start + (w * 4) as usize]);
            }
            drop(data);
            readback.unmap();
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uniform_is_std140_clean_and_matches_the_wgsl() {
        // Every member is a vec4 (or a mat4 of them), so the struct needs no
        // padding and the same bytes are valid on Metal and Vulkan. A stray
        // vec3 here is the classic silent GPU-side corruption.
        assert_eq!(CameraUniform::SIZE % 16, 0, "uniform must be 16-byte aligned");
        assert_eq!(CameraUniform::SIZE, 64 + 5 * 16, "unexpected uniform size — did a field change?");
        // and the WGSL declares exactly the same members, in the same order
        let decl = PAPER_WGSL.split("struct Camera").nth(1).unwrap().split('}').next().unwrap();
        for field in ["view_proj", "eye", "key_dir", "key_col", "fill_col", "ink_col"] {
            assert!(decl.contains(field), "WGSL Camera is missing {field}");
        }
    }

    #[test]
    fn vertex_layout_matches_the_scene_buffers_it_consumes() {
        assert_eq!(VERTEX_LAYOUT.stride as usize, crate::gpu::VERTEX_STRIDE,
                   "the pipeline stride must match what SceneBuffers packs");
        assert_eq!(VERTEX_LAYOUT.attrs[0], (0, 0, 3), "position first");
        assert_eq!(VERTEX_LAYOUT.attrs[1], (1, 12, 3), "then the FACE normal");
        assert_eq!(VERTEX_LAYOUT.attrs[2], (2, 24, 3), "then colour");
    }

    #[test]
    fn camera_projects_into_wgpu_clip_space() {
        // wgpu's clip Z is 0..1. A point at the near plane must land near 0 and
        // one at the far plane near w — the check that catches a GL-style matrix
        // pasted in from a tutorial, which renders as z-fighting or nothing.
        let mut c = CameraUniform::default();
        c.set_camera([0.0, 0.0, 5.0], [0.0, 0.0, 0.0], [0.0, 1.0, 0.0], 1.0, 1.0, 0.1, 100.0);
        let project = |p: [f32; 3]| {
            let m = c.view_proj;
            let w = m[0][3] * p[0] + m[1][3] * p[1] + m[2][3] * p[2] + m[3][3];
            let z = m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2] + m[3][2];
            (z, w)
        };
        let (zn, wn) = project([0.0, 0.0, 4.9]);   // 0.1 in front of the eye
        let (zf, wf) = project([0.0, 0.0, -95.0]); // 100 away
        assert!((zn / wn).abs() < 0.02, "near plane should map to ~0 (got {})", zn / wn);
        assert!((zf / wf - 1.0).abs() < 0.02, "far plane should map to ~1 (got {})", zf / wf);
    }

    #[cfg(feature = "wgpu-shader")]
    #[test]
    fn wgsl_compiles_under_the_same_front_end_wgpu_uses() {
        let entries = validate_shader().expect("paper.wgsl must parse and validate");
        assert!(entries.iter().any(|e| e == "vs_main"), "vertex entry point");
        assert!(entries.iter().any(|e| e == "fs_main"), "fragment entry point");
    }

    #[test]
    fn shader_keeps_the_paper_style_promises() {
        // Guardrails, not aesthetics-by-assertion: these are the three things a
        // well-meaning "improvement" would quietly break.
        assert!(!PAPER_WGSL.contains("normalize(in.nrm)"),
                "facet normals must NOT be re-normalized per fragment — that rounds off creases");
        assert!(PAPER_WGSL.contains("fn band("),
                "lighting must stay quantized into paint bands");
        assert!(PAPER_WGSL.contains("fibre(in.world)"),
                "paper grain must be world-anchored, or it swims across the sheet");
    }
}
