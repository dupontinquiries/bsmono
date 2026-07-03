//! v8 GPU mesh rasterizer (offscreen render via wgpu)
//! 
//! Mirrors the CPU rasterizer in v8_rasterizer.rs but uses the GPU's 
//! hardware triangle rasterizer + depth test for ~30-100x speedup at 
//! 2K+ resolutions.
//! 
//! Architecture:
//!   - WGSL vertex/fragment shader pair that:
//!     * Applies optional world transform
//!     * Projects through camera pose (rotate by inverse quat, pinhole)
//!     * Maps to NDC, runs hardware raster + depth test
//!     * Outputs vertex-interpolated RGB
//!   - Persistent device/queue/pipeline/buffers; one instance per 
//!     V8Decoder, reused across frames
//!   - Buffers grow on demand when meshes get larger
//!   - Output is read back to CPU as RGB bytes (matching CPU API)
//! 
//! Performance: at 2560×1440 with 10k triangles, expect ~0.5-2ms vs 
//! the CPU's 30-50ms. The readback (~10MB GPU→CPU copy + map) adds 
//! ~1-3ms. Total ~3-5ms per frame vs ~30-50ms CPU.
//! 
//! Feature-gated behind `gpu`. When disabled, callers fall back to the 
//! CPU rasterizer transparently.

#![cfg(feature = "gpu")]

use crate::v8_reconstruction_codec::{SceneMesh, CameraPose};

/// WGSL shader source. Mirrors the math in v8_rasterizer::project / 
/// rotate_by_quat.
const RASTERIZER_SHADER: &str = r#"
struct Uniforms {
    cam_pos: vec3<f32>,
    _pad0: f32,
    inv_rot: vec4<f32>,
    focal: vec2<f32>,
    principal: vec2<f32>,
    world_transform: mat4x4<f32>,
    has_world_transform: u32,
    width: u32,
    height: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<uniform> uniforms: Uniforms;

struct VertexIn {
    @location(0) position: vec3<f32>,
    @location(1) color: vec3<f32>,
}

struct VertexOut {
    @builtin(position) clip_pos: vec4<f32>,
    @location(0) color: vec3<f32>,
}

fn rotate_by_quat(q: vec4<f32>, v: vec3<f32>) -> vec3<f32> {
    let qv = q.xyz;
    let qw = q.w;
    let t = 2.0 * cross(qv, v);
    return v + qw * t + cross(qv, t);
}

@vertex
fn vs_main(in: VertexIn) -> VertexOut {
    var world: vec3<f32>;
    if (uniforms.has_world_transform != 0u) {
        let w4 = uniforms.world_transform * vec4<f32>(in.position, 1.0);
        world = w4.xyz;
    } else {
        world = in.position;
    }
    
    let rel = world - uniforms.cam_pos;
    let cam = rotate_by_quat(uniforms.inv_rot, rel);
    
    var out: VertexOut;
    out.color = in.color;
    
    if (cam.z < 0.000001) {
        // Cull behind camera: write a clip position outside the view 
        // volume. Hardware will discard.
        out.clip_pos = vec4<f32>(2.0, 2.0, 2.0, 1.0);
        return out;
    }
    
    let u = uniforms.focal.x * cam.x / cam.z + uniforms.principal.x;
    let v = uniforms.focal.y * cam.y / cam.z + uniforms.principal.y;
    
    // Pixel coords → NDC. Note Y flip: pixel (0,0) is top-left, 
    // NDC (-1,-1) is bottom-left.
    let ndc_x = (u / f32(uniforms.width)) * 2.0 - 1.0;
    let ndc_y = 1.0 - (v / f32(uniforms.height)) * 2.0;
    
    // Map camera Z to [0, 1] for depth buffer using a near-only 
    // hyperbolic mapping:
    //     depth = 1.0 - near / cam.z
    // Properties:
    //   - cam.z = near   → depth = 0   (closest, passes Less compare)
    //   - cam.z → ∞      → depth → 1   (farthest, fails Less compare)
    //   - Monotonic in cam.z for all cam.z > 0
    //   - No clamping / no far plane
    //   - Precision concentrated near the camera (where it matters)
    // 
    // Previously we used a linear clamp `cam.z / 1000.0` which mapped 
    // any cam.z > 1000 to a constant 0.9999. For v8 content where 
    // typical scene depths reach 30000+ units, every distant vertex 
    // got the same depth value — depth-test ordering became random and 
    // distant objects "jittered" forward as if closer than the actual 
    // foreground geometry. User-reported symptom: "background seems 
    // closer to the screen based off the jitter."
    // Reverse-Z-like hyperbolic mapping, no upper clamp:
    //   depth = 1.0 - 1.0 / cam.z  for cam.z > 1
    //   depth approaches 1 as cam.z → ∞ but never reaches it
    //   floating-point gives us enough distinct values near 1 to 
    //   discriminate even very distant geometry
    // 
    // For cam.z in (0, 1] we'd get negative depth — that's "closer 
    // than near plane" which we cull by the earlier `cam.z < 0.000001` 
    // check (technically allows tiny positive values through; clamp 
    // the lower bound to keep them in-range).
    let depth_norm = max(1.0 - 1.0 / cam.z, 0.0);
    
    out.clip_pos = vec4<f32>(ndc_x, ndc_y, depth_norm, 1.0);
    return out;
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    return vec4<f32>(in.color, 1.0);
}
"#;

/// Uniform block matching the WGSL Uniforms struct exactly. Must be 
/// 16-byte aligned at top level; WGSL std140-like rules apply.
#[repr(C, align(16))]
#[derive(Copy, Clone)]
struct UniformsPacked {
    cam_pos: [f32; 3],
    _pad0: f32,
    inv_rot: [f32; 4],
    focal: [f32; 2],
    principal: [f32; 2],
    // mat4x4 is 64 bytes
    world_transform: [f32; 16],
    has_world_transform: u32,
    width: u32,
    height: u32,
    _pad1: u32,
}

/// GPU mesh rasterizer. One instance per V8Decoder; buffers reused 
/// across frames and grown on demand when meshes get larger.
pub struct GpuMeshRasterizer {
    device: std::sync::Arc<wgpu::Device>,
    queue: std::sync::Arc<wgpu::Queue>,
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    render_calls: u64,  // diagnostic counter
    
    // Per-frame state — recreated only on size change.
    width: u32,
    height: u32,
    color_target: wgpu::Texture,
    color_view: wgpu::TextureView,
    depth_target: wgpu::Texture,
    depth_view: wgpu::TextureView,
    readback_buf: wgpu::Buffer,
    
    // Persistent buffers — grown on demand.
    vbuf: wgpu::Buffer,
    vbuf_cap: u64,  // bytes
    ibuf: wgpu::Buffer,
    ibuf_cap: u64,  // bytes
    uniform_buf: wgpu::Buffer,
}

impl GpuMeshRasterizer {
    /// Construct a new rasterizer with the given output dimensions. 
    /// Tries to acquire a wgpu adapter; returns None on systems without 
    /// a supported backend (no Metal/Vulkan/DX12/GL).
    /// 
    /// The width/height are the initial render-target size. Resizes 
    /// happen automatically when render() is called with different 
    /// dimensions.
    pub fn new(width: u32, height: u32) -> Option<Self> {
        // Block on async wgpu init via pollster.
        pollster::block_on(Self::new_async(width, height))
    }
    
    async fn new_async(width: u32, height: u32) -> Option<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::PRIMARY,
            ..Default::default()
        });
        let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: false,
        }).await?;
        
        // Log adapter info for diagnostics. If users see surprises 
        // (e.g., LLVMpipe software rasterizer picked over real GPU), 
        // they'll see why and can troubleshoot drivers.
        let info = adapter.get_info();
        eprintln!("[v8 GPU raster] adapter: {} ({:?}) — backend={:?}", 
            info.name, info.device_type, info.backend);
        
        let (device, queue) = adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("fao-gpu-rasterizer"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
            },
            None,
        ).await.ok()?;
        
        let device = std::sync::Arc::new(device);
        let queue = std::sync::Arc::new(queue);
        
        // Shader module
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fao-rasterizer-shader"),
            source: wgpu::ShaderSource::Wgsl(RASTERIZER_SHADER.into()),
        });
        
        // Bind group layout: single uniform buffer
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("fao-rasterizer-bgl"),
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
        
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("fao-rasterizer-pl"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("fao-rasterizer-pipeline"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_main",
                buffers: &[wgpu::VertexBufferLayout {
                    array_stride: 24,  // 3 floats position + 3 floats color = 24 bytes
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: &[
                        wgpu::VertexAttribute {
                            offset: 0,
                            shader_location: 0,
                            format: wgpu::VertexFormat::Float32x3,
                        },
                        wgpu::VertexAttribute {
                            offset: 12,
                            shader_location: 1,
                            format: wgpu::VertexFormat::Float32x3,
                        },
                    ],
                }],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_main",
                targets: &[Some(wgpu::ColorTargetState {
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleList,
                strip_index_format: None,
                front_face: wgpu::FrontFace::Ccw,
                cull_mode: None,  // matches CPU rasterizer (no back-face cull)
                polygon_mode: wgpu::PolygonMode::Fill,
                unclipped_depth: false,
                conservative: false,
            },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: wgpu::TextureFormat::Depth32Float,
                depth_write_enabled: true,
                depth_compare: wgpu::CompareFunction::Less,
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
        });
        
        let (color_target, color_view, depth_target, depth_view, readback_buf) = 
            Self::make_targets(&device, width, height);
        
        // Initial buffers (small, will grow on demand).
        let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fao-vbuf"),
            size: 1024,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let ibuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fao-ibuf"),
            size: 1024,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fao-uniform"),
            size: std::mem::size_of::<UniformsPacked>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        
        Some(Self {
            device, queue, pipeline, bind_group_layout,
            render_calls: 0,
            width, height,
            color_target, color_view, depth_target, depth_view, readback_buf,
            vbuf, vbuf_cap: 1024,
            ibuf, ibuf_cap: 1024,
            uniform_buf,
        })
    }
    
    fn make_targets(
        device: &wgpu::Device,
        width: u32,
        height: u32,
    ) -> (wgpu::Texture, wgpu::TextureView, wgpu::Texture, wgpu::TextureView, wgpu::Buffer) {
        let color = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("fao-color-target"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let color_view = color.create_view(&wgpu::TextureViewDescriptor::default());
        
        let depth = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("fao-depth-target"),
            size: wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor::default());
        
        // Readback buffer: RGBA8 = 4 bytes/pixel. wgpu requires row 
        // pitch to be a multiple of 256 bytes.
        let bytes_per_row = ((width * 4 + 255) / 256) * 256;
        let readback_size = (bytes_per_row * height) as u64;
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fao-readback"),
            size: readback_size,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        
        (color, color_view, depth, depth_view, readback)
    }
    
    /// Render a mesh + pose to RGB bytes. Returns None on failure 
    /// (empty mesh, GPU error, etc) so the caller can fall back to the 
    /// CPU rasterizer.
    pub fn render(
        &mut self,
        mesh: &SceneMesh,
        world_transform: Option<&[f32; 16]>,
        pose: &CameraPose,
        width: usize,
        height: usize,
    ) -> Option<Vec<u8>> {
        if mesh.vertices.is_empty() || mesh.indices.is_empty() { return None; }
        if width == 0 || height == 0 { return None; }
        
        // Diagnostic on the first call: dump mesh stats + pose so we 
        // can verify GPU is getting the same inputs as CPU.
        if self.render_calls == 0 {
            let nv = mesh.vertices.len();
            let nt = mesh.indices.len() / 3;
            let has_colors = !mesh.vertex_colors.is_empty();
            let first_v = mesh.vertices.first().map(|v| (v.x, v.y, v.z));
            let first_c = mesh.vertex_colors.first().copied();
            eprintln!("[v8 GPU raster] first render: {} verts, {} tris, has_colors={}, first_v={:?}, first_c={:?}",
                nv, nt, has_colors, first_v, first_c);
            eprintln!("[v8 GPU raster]   pose: pos=({:.2},{:.2},{:.2}) rot=[{:.3},{:.3},{:.3},{:.3}] focal=[{:.1},{:.1}] princ=[{:.1},{:.1}]",
                pose.position.x, pose.position.y, pose.position.z,
                pose.rotation[0], pose.rotation[1], pose.rotation[2], pose.rotation[3],
                pose.focal[0], pose.focal[1],
                pose.principal[0], pose.principal[1]);
            eprintln!("[v8 GPU raster]   world_transform: {}", 
                if world_transform.is_some() { "present" } else { "none" });
        }
        self.render_calls += 1;
        
        let w = width as u32;
        let h = height as u32;
        
        // Resize targets if needed.
        if w != self.width || h != self.height {
            let (color, color_view, depth, depth_view, readback) =
                Self::make_targets(&self.device, w, h);
            self.color_target = color;
            self.color_view = color_view;
            self.depth_target = depth;
            self.depth_view = depth_view;
            self.readback_buf = readback;
            self.width = w;
            self.height = h;
        }
        
        // Pack vertices + colors into interleaved buffer.
        let n_verts = mesh.vertices.len();
        let mut packed: Vec<f32> = Vec::with_capacity(n_verts * 6);
        for i in 0..n_verts {
            let v = &mesh.vertices[i];
            packed.push(v.x);
            packed.push(v.y);
            packed.push(v.z);
            let c = mesh.vertex_colors.get(i).copied().unwrap_or([0.5, 0.5, 0.5]);
            packed.push(c[0]);
            packed.push(c[1]);
            packed.push(c[2]);
        }
        let packed_bytes: &[u8] = bytemuck_cast(&packed);
        let packed_len = packed_bytes.len() as u64;
        
        if packed_len > self.vbuf_cap {
            // Grow with 2× headroom.
            let new_cap = (packed_len * 2).max(self.vbuf_cap * 2);
            self.vbuf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fao-vbuf"),
                size: new_cap,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.vbuf_cap = new_cap;
        }
        self.queue.write_buffer(&self.vbuf, 0, packed_bytes);
        
        // Index buffer.
        // 
        // Degenerate-triangle filter: skip triangles whose edge-length 
        // ratio is extreme. These appear when a vertex jumps to a bad 
        // position (corrupted depth, lost feature track), producing 
        // long thin streaks across the screen — the "long lines" 
        // visible artifact the user reported.
        // 
        // The filter is on by default; FAO_NO_CULL=1 disables it for 
        // diagnostic purposes. Cost: 3 edge-length computations per 
        // triangle (~30K sqrt's for a 10K-triangle mesh, negligible).
        // 
        // Threshold rationale:
        //   - Ratio > 50: max edge is 50× the min edge — needle-shape, 
        //     almost always a bad vertex. Real triangulated features 
        //     produce roughly equilateral triangles (ratio < 5).
        //   - Absolute max > 10000 scene-units: vertices at extreme 
        //     positions, likely numerical instability or NaN-propagated 
        //     from a bad camera pose during triangulation.
        let cull_disabled = std::env::var("FAO_NO_CULL")
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false);
        let filtered_indices: Vec<u32> = if cull_disabled {
            mesh.indices.clone()
        } else {
            let mut out = Vec::with_capacity(mesh.indices.len());
            let mut culled = 0usize;
            for tri in mesh.indices.chunks_exact(3) {
                let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
                if i0 >= n_verts || i1 >= n_verts || i2 >= n_verts { 
                    culled += 1; 
                    continue; 
                }
                let v0 = &mesh.vertices[i0];
                let v1 = &mesh.vertices[i1];
                let v2 = &mesh.vertices[i2];
                // Edge lengths (squared — cheaper to compare).
                let d01 = (v1.x - v0.x).powi(2) + (v1.y - v0.y).powi(2) + (v1.z - v0.z).powi(2);
                let d12 = (v2.x - v1.x).powi(2) + (v2.y - v1.y).powi(2) + (v2.z - v1.z).powi(2);
                let d20 = (v0.x - v2.x).powi(2) + (v0.y - v2.y).powi(2) + (v0.z - v2.z).powi(2);
                let max_d2 = d01.max(d12).max(d20);
                let min_d2 = d01.min(d12).min(d20);
                // Reject if any vertex is NaN/inf (max becomes NaN/inf).
                if !max_d2.is_finite() {
                    culled += 1;
                    continue;
                }
                // Absolute size limit: max edge length > 10000 → likely bad
                const MAX_EDGE_SQ: f32 = 10000.0 * 10000.0;
                if max_d2 > MAX_EDGE_SQ {
                    culled += 1;
                    continue;
                }
                // Aspect-ratio limit: max/min > 50 → needle-shaped, 
                // likely a streak-bug triangle. We compare squared 
                // distances so the threshold is 50² = 2500.
                const ASPECT_SQ: f32 = 50.0 * 50.0;
                if min_d2 > 1e-6 && max_d2 / min_d2 > ASPECT_SQ {
                    culled += 1;
                    continue;
                }
                out.push(tri[0]);
                out.push(tri[1]);
                out.push(tri[2]);
            }
            // Only log on first frame to avoid flooding.
            if self.render_calls <= 1 && culled > 0 {
                eprintln!("[v8 GPU raster] degenerate-tri cull: {} of {} triangles dropped",
                    culled, mesh.indices.len() / 3);
            }
            out
        };
        let indices_bytes: &[u8] = bytemuck_cast(&filtered_indices);
        let indices_len = indices_bytes.len() as u64;
        if indices_len > self.ibuf_cap {
            let new_cap = (indices_len * 2).max(self.ibuf_cap * 2);
            self.ibuf = self.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("fao-ibuf"),
                size: new_cap,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.ibuf_cap = new_cap;
        }
        self.queue.write_buffer(&self.ibuf, 0, indices_bytes);
        
        // Uniforms.
        // 
        // pose.rotation is stored as [w, x, y, z]. The shader treats 
        // inv_rot as a vec4 where .xyz is the rotation axis and .w is 
        // the scalar. So we need to:
        //   1. Build the conjugate in [w,x,y,z]: [w, -x, -y, -z]
        //   2. Reorder to shader's [x, y, z, w] layout: [-x, -y, -z, w]
        // Result: array indices [1,2,3,0] of the original rotation, 
        // negating the xyz components.
        let inv_rot = [
            -pose.rotation[1],  // → shader.x (was conjugate xyz)
            -pose.rotation[2],  // → shader.y
            -pose.rotation[3],  // → shader.z
             pose.rotation[0],  // → shader.w (was conjugate scalar)
        ];
        let uniforms = UniformsPacked {
            cam_pos: [pose.position.x, pose.position.y, pose.position.z],
            _pad0: 0.0,
            inv_rot,
            // Defensive fallback for poses with zero focal length (e.g., 
            // unwritten shard entries). Matches the CPU rasterizer's 
            // fallback to 1000.0.
            focal: [
                if pose.focal[0] > 0.0 { pose.focal[0] } else { 1000.0 },
                if pose.focal[1] > 0.0 { pose.focal[1] } else { 1000.0 },
            ],
            principal: pose.principal,
            world_transform: world_transform.copied().unwrap_or_else(|| {
                // Identity matrix
                let mut m = [0.0f32; 16];
                m[0] = 1.0; m[5] = 1.0; m[10] = 1.0; m[15] = 1.0;
                m
            }),
            has_world_transform: if world_transform.is_some() { 1 } else { 0 },
            width: w,
            height: h,
            _pad1: 0,
        };
        let uniforms_bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                &uniforms as *const _ as *const u8,
                std::mem::size_of::<UniformsPacked>(),
            )
        };
        self.queue.write_buffer(&self.uniform_buf, 0, uniforms_bytes);
        
        // Bind group.
        let bind_group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("fao-rasterizer-bg"),
            layout: &self.bind_group_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: self.uniform_buf.as_entire_binding(),
            }],
        });
        
        // Encode + submit.
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("fao-rasterizer-encoder"),
        });
        {
            let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("fao-rasterizer-rpass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: 0.0, g: 0.0, b: 0.0, a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rpass.set_pipeline(&self.pipeline);
            rpass.set_bind_group(0, &bind_group, &[]);
            rpass.set_vertex_buffer(0, self.vbuf.slice(..));
            rpass.set_index_buffer(self.ibuf.slice(..), wgpu::IndexFormat::Uint32);
            // Use filtered_indices length, not mesh.indices, so dropped 
            // triangles don't get drawn (they would draw from stale 
            // buffer contents otherwise).
            let n_indices = filtered_indices.len() as u32;
            rpass.draw_indexed(0..n_indices, 0, 0..1);
        }
        
        // Copy color target → readback buffer.
        let bytes_per_row = ((w * 4 + 255) / 256) * 256;
        encoder.copy_texture_to_buffer(
            wgpu::ImageCopyTexture {
                texture: &self.color_target,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::ImageCopyBuffer {
                buffer: &self.readback_buf,
                layout: wgpu::ImageDataLayout {
                    offset: 0,
                    bytes_per_row: Some(bytes_per_row),
                    rows_per_image: Some(h),
                },
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        
        self.queue.submit(Some(encoder.finish()));
        
        // Map readback, copy RGBA→RGB, drop alpha.
        let slice = self.readback_buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx.send(r); });
        self.device.poll(wgpu::Maintain::Wait);
        if rx.recv().ok()?.is_err() { return None; }
        
        let data = slice.get_mapped_range();
        let row_bytes_actual = (w * 4) as usize;
        let row_bytes_padded = bytes_per_row as usize;
        let mut rgb = Vec::with_capacity((w * h * 3) as usize);
        for y in 0..h as usize {
            let row_start = y * row_bytes_padded;
            for x in 0..w as usize {
                let pi = row_start + x * 4;
                rgb.push(data[pi]);
                rgb.push(data[pi + 1]);
                rgb.push(data[pi + 2]);
                // skip alpha
            }
        }
        drop(data);
        self.readback_buf.unmap();
        
        // Detect all-black output: signals that rasterization produced 
        // no visible geometry (everything culled, off-screen, or behind 
        // camera). The CPU rasterizer returns None in this case so the 
        // caller can fall back to source video. We mirror that: if no 
        // pixel is non-zero, return None.
        // 
        // The check is cheap — sample stride keeps total work to ~10K 
        // byte loads regardless of frame size. False negatives (we say 
        // "all black" when there are pixels we missed) only happen if 
        // every sampled pixel is exactly 0, which means the rendered 
        // content is in fact mostly black.
        let stride = (rgb.len() / 4096).max(1);
        let any_nonzero = rgb.iter().step_by(stride).any(|&b| b != 0);
        
        if self.render_calls <= 3 {
            let n_nonzero = rgb.iter().take(1000).filter(|&&b| b != 0).count();
            let max_byte = rgb.iter().take(10000).max().copied().unwrap_or(0);
            eprintln!("[v8 GPU raster] render #{}: {} nonzero / 1000 bytes, max_byte={}, rgb_len={}, sample-any_nonzero={}",
                self.render_calls, n_nonzero, max_byte, rgb.len(), any_nonzero);
        }
        
        if !any_nonzero {
            return None;
        }
        
        Some(rgb)
    }
}

/// Cast a slice of POD floats/u32s to bytes. Equivalent to a 
/// std::slice::from_raw_parts call with mem::size_of<T>() multiplier; 
/// used instead of pulling in bytemuck as a dep.
fn bytemuck_cast<T: Copy>(s: &[T]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(
            s.as_ptr() as *const u8,
            std::mem::size_of_val(s),
        )
    }
}
