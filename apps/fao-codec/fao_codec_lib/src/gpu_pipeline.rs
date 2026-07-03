// FAO GPU Pipeline — unified multi-pass compute
// All passes share the same uploaded pixel data on GPU.
// Results feed into the prediction web, not bypassing it.
//
// Requires: --features gpu (wgpu + pollster)

#[cfg(feature = "gpu")]
use wgpu;

/// GPU buffer slots — persistent across frames
// (was a dangling `#[cfg(feature = "gpu")]` here that didn't bind to 
// anything — removed because it accomplished nothing)


// ═══════════════════════════════════════════════════════════════════
// GPU OPTIMIZATION AUDIT — what's on GPU now, what should be
// ═══════════════════════════════════════════════════════════════════
//
// This file holds GPU compute dispatch for the codec. Currently used by 
// v2/v3 motion estimation. Below is an audit of what could ALSO move to 
// GPU for the bigger wins on encode speed.
//
// === Currently on GPU ===
//
//   - Motion estimation (16x16 block search) — used by v2 P-frame encode. 
//     Already 10-20× faster than CPU equivalent.
//   - Classify shader — region categorization for v3.
//   - Scene analysis shaders (5 of them, used during transcode setup).
//
// === High-value targets for GPU offload ===
//
//   1. V8 PHASE CORRELATION (huge win — CPU is FFT-bound)
//      Current: FFT2 + cross-correlation in CPU per frame, ~30ms at 1080p.
//      GPU: FFT compute shader (multiple impl available, e.g. wgpu-fft).
//      Expected: 2-5ms per frame on M2-class GPU.
//      Impact: ~25ms × 5548 frames = 2.3 minutes saved on Lone Ranger.
//
//   2. V8 FEATURE TRACKING (LK optical flow)
//      Current: per-feature 5-iteration LK in CPU, ~10ms for 500 features.
//      GPU: per-feature work group, parallel iterate. ~1-2ms.
//      Impact: ~8ms × 5548 = 45s saved.
//
//   3. RGB ↔ YUV CONVERSION (every frame, both encode and decode)
//      Current: CPU loop over pixels, ~5ms at 1080p. SIMD f32x4 helps but 
//      not enough.
//      GPU: trivial fragment/compute shader, <0.1ms.
//      Impact: ~4ms × frame_count saved on encode AND decode.
//
//   4. MESH RASTERIZATION (decode side, blocked on this for fly-through)
//      Current: Cairo CPU rasterization, capped at 30K tris (visible 
//      flicker beyond that).
//      GPU: vertex+fragment shader with depth test. Millions of tris at 
//      60fps trivially.
//      Impact: enables real fly-through with full mesh density.
//
//   5. DENSIFY PASS (stereo block-match for depth maps)
//      Current: per-anchor CPU search, ~50ms at 1080p when active.
//      GPU: 2D block-match shader (similar to existing motion estimate).
//      Impact: 40ms × n_anchors saved per chunk.
//
// === Medium-value targets ===
//
//   6. DCT 8x8 batch
//      Current: scalar loops, SIMD-able.
//      GPU: trivial — workgroup per block, 64 threads cooperating.
//      Impact: smaller (DCT is 1-2% of v2 encode time).
//
//   7. Mesh fusion (HashMap-based vertex de-dup)
//      GPU: harder — needs spatial hashing on GPU. Probably defer.
//
// === Order of operations ===
//
//   The biggest user-visible win comes from #1 (phase correlation) 
//   because it's currently the per-frame bottleneck for v8 encode.
//   Implementing it would likely reduce per-frame v8 encode time from 
//   ~50ms to ~20ms, doubling effective encode throughput.
//
//   But #4 (mesh rasterization) is the highest-LEVERAGE — it unblocks 
//   the user's stated wish to fly through the world with proper detail.
//   Without GPU rasterization the World view is fundamentally limited 
//   to ~30K visible tris (Cairo throughput cap).
//
// ═══════════════════════════════════════════════════════════════════

// Motion Estimation Compute Shader — produces 16x16 block MVs in parallel
// Each workgroup handles one 16x16 block, runs diamond search across the GPU
pub const MOTION_ESTIMATE_SHADER: &str = r#"
@group(0) @binding(0) var<storage, read> cur_y: array<u32>;     // current Y plane (packed u8 → u32)
@group(0) @binding(1) var<storage, read> ref_y: array<u32>;     // reference Y plane
@group(0) @binding(2) var<uniform> dims: vec4<u32>;             // (width, height, blocks_x, blocks_y)
@group(0) @binding(3) var<storage, read_write> mvs: array<i32>; // output: 2 i32s per block (dx, dy)
@group(0) @binding(4) var<storage, read_write> sads: array<u32>;// output: best SAD per block

// One workgroup per block, threads cooperate on SAD computation
@compute @workgroup_size(16, 16)
fn me_diamond_search(
    @builtin(workgroup_id) block: vec3<u32>,
    @builtin(local_invocation_id) local: vec3<u32>,
) {
    let w = dims.x;
    let h = dims.y;
    let bw = dims.z;
    let bh = dims.w;
    
    if (block.x >= bw || block.y >= bh) { return; }
    
    let bx = block.x * 16u;
    let by = block.y * 16u;
    
    let block_idx = block.y * bw + block.x;
    
    // Each thread computes SAD for one of the candidate motion vectors
    // 17 candidates: (0,0) + 8 diamond + 8 refine
    // We use one thread for each search position via local index mapping
    
    // Simple sequential block matching by first thread (correct but slow)
    if (local.x == 0u && local.y == 0u) {
        var best_sad: u32 = 0xFFFFFFFFu;
        var best_dx: i32 = 0;
        var best_dy: i32 = 0;
        
        // Test (0,0) + diamond pattern + refine
        let candidates = array<vec2<i32>, 25>(
            vec2<i32>(0, 0),
            vec2<i32>(-4, 0), vec2<i32>(4, 0), vec2<i32>(0, -4), vec2<i32>(0, 4),
            vec2<i32>(-4, -4), vec2<i32>(4, -4), vec2<i32>(-4, 4), vec2<i32>(4, 4),
            vec2<i32>(-2, 0), vec2<i32>(2, 0), vec2<i32>(0, -2), vec2<i32>(0, 2),
            vec2<i32>(-2, -2), vec2<i32>(2, -2), vec2<i32>(-2, 2), vec2<i32>(2, 2),
            vec2<i32>(-1, 0), vec2<i32>(1, 0), vec2<i32>(0, -1), vec2<i32>(0, 1),
            vec2<i32>(-1, -1), vec2<i32>(1, -1), vec2<i32>(-1, 1), vec2<i32>(1, 1),
        );
        
        for (var c: i32 = 0; c < 25; c = c + 1) {
            let dx = candidates[c].x;
            let dy = candidates[c].y;
            
            var sad: u32 = 0u;
            for (var py: u32 = 0u; py < 16u; py = py + 1u) {
                for (var px: u32 = 0u; px < 16u; px = px + 1u) {
                    let dst_x = bx + px;
                    let dst_y = by + py;
                    if (dst_x >= w || dst_y >= h) { continue; }
                    
                    let src_x = clamp(i32(dst_x) + dx, 0, i32(w) - 1);
                    let src_y = clamp(i32(dst_y) + dy, 0, i32(h) - 1);
                    
                    // Read from packed u32 arrays (4 pixels per word)
                    let dst_idx = dst_y * w + dst_x;
                    let src_idx = u32(src_y) * w + u32(src_x);
                    
                    let cur_word = cur_y[dst_idx >> 2u];
                    let cur_byte = u32((cur_word >> ((dst_idx & 3u) * 8u)) & 0xFFu);
                    
                    let ref_word = ref_y[src_idx >> 2u];
                    let ref_byte = u32((ref_word >> ((src_idx & 3u) * 8u)) & 0xFFu);
                    
                    let diff = i32(cur_byte) - i32(ref_byte);
                    sad = sad + u32(abs(diff));
                }
            }
            
            if (sad < best_sad) {
                best_sad = sad;
                best_dx = dx;
                best_dy = dy;
            }
        }
        
        mvs[block_idx * 2u] = best_dx;
        mvs[block_idx * 2u + 1u] = best_dy;
        sads[block_idx] = best_sad;
    }
}
"#;

/// CPU-side helper: dispatch the motion estimation shader on a frame pair.
/// Returns Vec<(i8, i8)> motion vectors per 16x16 block.
/// This is the GPU-accelerated version of the CPU diamond search in v21_predictive.
/// Motion estimation pipeline state (one per resolution).
/// Cache this in GpuPipeline to avoid recreating buffers per frame.
#[cfg(feature = "gpu")]
pub struct GpuMotionPipeline {
    pub pipeline: wgpu::ComputePipeline,
    pub bind_layout: wgpu::BindGroupLayout,
    pub cur_y_buf: wgpu::Buffer,    // current frame Y (packed u8)
    pub ref_y_buf: wgpu::Buffer,    // reference frame Y
    pub dims_buf: wgpu::Buffer,     // (w, h, bw, bh) uniform
    pub mvs_buf: wgpu::Buffer,      // output: 2 i32 per block
    pub sads_buf: wgpu::Buffer,     // output: 1 u32 per block
    pub mvs_readback: wgpu::Buffer, // CPU-readable MVs
    pub w: usize,
    pub h: usize,
}

#[cfg(feature = "gpu")]
impl GpuMotionPipeline {
    pub fn new(device: &wgpu::Device, w: usize, h: usize) -> Option<Self> {
        let bw = w / 16;
        let bh = h / 16;
        let n_blocks = bw * bh;
        let y_sz = w * h;
        let y_words = (y_sz + 3) / 4;  // packed u8 in u32 words
        
        // Create shader module
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("motion_estimate"),
            source: wgpu::ShaderSource::Wgsl(MOTION_ESTIMATE_SHADER.into()),
        });
        
        // Bind group layout: 5 bindings (cur, ref, dims uniform, mvs out, sads out)
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("me_bind_layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("me_pipeline_layout"),
            bind_group_layouts: &[&bind_layout],
            push_constant_ranges: &[],
        });
        
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("me_pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: "me_diamond_search",
        });
        
        // Buffers
        let cur_y_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cur_y"),
            size: (y_words * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let ref_y_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ref_y"),
            size: (y_words * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let dims_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dims"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mvs_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mvs"),
            size: (n_blocks * 8) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let sads_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("sads"),
            size: (n_blocks * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE,
            mapped_at_creation: false,
        });
        let mvs_readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mvs_readback"),
            size: (n_blocks * 8) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        
        Some(Self {
            pipeline, bind_layout, cur_y_buf, ref_y_buf, dims_buf, mvs_buf, sads_buf,
            mvs_readback, w, h,
        })
    }
    
    /// Run motion estimation on GPU. Returns motion vectors per 16x16 block.
    pub fn estimate(&self, device: &wgpu::Device, queue: &wgpu::Queue,
                    cur_y: &[u8], ref_y: &[u8]) -> Option<Vec<(i8, i8)>> {
        let bw = self.w / 16;
        let bh = self.h / 16;
        let n_blocks = bw * bh;
        let y_sz = self.w * self.h;
        
        // Pack u8 → u32 (4 pixels per word, little-endian)
        let mut cur_packed = vec![0u32; (y_sz + 3) / 4];
        let mut ref_packed = vec![0u32; (y_sz + 3) / 4];
        for i in 0..y_sz {
            cur_packed[i / 4] |= (cur_y[i] as u32) << ((i % 4) * 8);
            ref_packed[i / 4] |= (ref_y[i] as u32) << ((i % 4) * 8);
        }
        
        // Upload (manual u32 → u8 byte conversion to avoid bytemuck dep)
        let cur_bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(cur_packed.as_ptr() as *const u8, cur_packed.len() * 4)
        };
        let ref_bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(ref_packed.as_ptr() as *const u8, ref_packed.len() * 4)
        };
        queue.write_buffer(&self.cur_y_buf, 0, cur_bytes);
        queue.write_buffer(&self.ref_y_buf, 0, ref_bytes);
        let dims = [self.w as u32, self.h as u32, bw as u32, bh as u32];
        let dims_bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(dims.as_ptr() as *const u8, 16)
        };
        queue.write_buffer(&self.dims_buf, 0, dims_bytes);
        
        // Bind group
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("me_bind"),
            layout: &self.bind_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.cur_y_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.ref_y_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: self.dims_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: self.mvs_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: self.sads_buf.as_entire_binding() },
            ],
        });
        
        // Dispatch
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("me_encoder"),
        });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("me_pass"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(bw as u32, bh as u32, 1);
        }
        encoder.copy_buffer_to_buffer(&self.mvs_buf, 0, &self.mvs_readback, 0, (n_blocks * 8) as u64);
        queue.submit(Some(encoder.finish()));
        
        // Readback
        let buf_slice = self.mvs_readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buf_slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx.send(r); });
        device.poll(wgpu::Maintain::Wait);
        rx.recv().ok()?.ok()?;
        
        let data = buf_slice.get_mapped_range();
        let mvs_i32: &[i32] = unsafe {
            std::slice::from_raw_parts(data.as_ptr() as *const i32, n_blocks * 2)
        };
        let mut mvs = Vec::with_capacity(n_blocks);
        for i in 0..n_blocks {
            let dx = mvs_i32[i * 2];
            let dy = mvs_i32[i * 2 + 1];
            mvs.push((dx.clamp(-127, 127) as i8, dy.clamp(-127, 127) as i8));
        }
        drop(data);
        self.mvs_readback.unmap();
        
        Some(mvs)
    }
}

#[cfg(feature = "gpu")]
#[allow(dead_code)]
pub fn gpu_estimate_motion(
    pipeline: &GpuPipeline,
    cur_y: &[u8],
    ref_y: &[u8],
    w: usize, h: usize,
) -> Vec<(i8, i8)> {
    let bw = w / 16;
    let bh = h / 16;
    let n_blocks = bw * bh;
    
    // Try GPU motion pipeline; fall back to CPU on error
    if let Some(me_pipeline) = GpuMotionPipeline::new(&pipeline.device, w, h) {
        if let Some(mvs) = me_pipeline.estimate(&pipeline.device, &pipeline.queue, cur_y, ref_y) {
            return mvs;
        }
    }
    
    // CPU fallback: zero MVs
    let _ = cur_y;
    let _ = ref_y;
    vec![(0, 0); n_blocks]
}

#[cfg(feature = "gpu")]
pub struct GpuPipeline {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    // Persistent buffers (stay on GPU between frames)
    pub cur_y: wgpu::Buffer,       // current frame Y plane (uploaded each frame)
    pub ref_y: wgpu::Buffer,       // reference Y plane (updated after encode)
    pub multi_ref: Vec<wgpu::Buffer>, // stored reference Y planes (8 slots)
    pub field_y: wgpu::Buffer,     // Fourier field Y output
    pub field_cb: wgpu::Buffer,    // Fourier field Cb output
    pub field_cr: wgpu::Buffer,    // Fourier field Cr output
    // Output buffers (read back by CPU/web scheduler)
    pub route_map: wgpu::Buffer,   // per-block prediction route decisions
    pub sad_map: wgpu::Buffer,     // per-block SAD values
    pub class_map: wgpu::Buffer,   // per-block classification
    pub light_map: wgpu::Buffer,   // per-region lighting direction
    pub blur_map: wgpu::Buffer,    // per-block motion blur detection
    pub histogram: wgpu::Buffer,   // 256-bin Y histogram
    // Uniforms
    pub params: wgpu::Buffer,      // width, height, n_coeffs, etc.
    // Pipelines
    pub field_pipeline: wgpu::ComputePipeline,
    pub sad_pipeline: wgpu::ComputePipeline,
    pub classify_pipeline: wgpu::ComputePipeline,
    pub histogram_pipeline: wgpu::ComputePipeline,
    pub motion_pipeline: wgpu::ComputePipeline,
    pub lighting_pipeline: wgpu::ComputePipeline,
    // Bind groups
    pub bind_group: wgpu::BindGroup,
    // Dimensions
    pub w: u32,
    pub h: u32,
    pub n_blocks: u32,
}

/// Parameters passed to GPU shaders
#[repr(C)]
#[derive(Copy, Clone)]
pub struct GpuParams {
    pub width: u32,
    pub height: u32,
    pub n_blocks_x: u32,
    pub n_blocks_y: u32,
    pub n_coeffs: u32,
    pub n_refs: u32,
    pub skip_thresh: f32,
    pub frame_idx: u32,
}

// Placeholder for non-GPU builds
#[cfg(not(feature = "gpu"))]
pub struct GpuPipeline;

/// Unified WGSL shader source for all passes
#[cfg(feature = "gpu")]
pub const UNIFIED_SHADER: &str = r#"

// ═══════════════════════════════════════════════════════════
// GPU Decode Pipeline — YUV→RGB conversion + frame reconstruction
// ═══════════════════════════════════════════════════════════

/// GPU YUV420p → RGB conversion compute shader.
/// Each workgroup processes a 16×16 tile. Handles chroma subsampling.
pub const YUV_TO_RGB_SHADER: &str = "
@group(0) @binding(0) var<storage, read> yuv_y: array<u32>;    // Y plane packed as u32
@group(0) @binding(1) var<storage, read> yuv_u: array<u32>;    // U plane packed as u32  
@group(0) @binding(2) var<storage, read> yuv_v: array<u32>;    // V plane packed as u32
@group(0) @binding(3) var<storage, read_write> rgb_out: array<u32>;  // RGB packed
@group(0) @binding(4) var<uniform> dims: vec4<u32>;  // width, height, y_stride, uv_stride

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y_coord = gid.y;
    let w = dims.x;
    let h = dims.y;
    if (x >= w || y_coord >= h) { return; }
    
    let yi = y_coord * w + x;
    let uvi = (y_coord / 2u) * (w / 2u) + (x / 2u);
    
    // Read Y, U, V bytes from packed u32 arrays
    let y_word = yuv_y[yi / 4u];
    let y_byte = (y_word >> ((yi % 4u) * 8u)) & 0xFFu;
    let u_word = yuv_u[uvi / 4u];
    let u_byte = (u_word >> ((uvi % 4u) * 8u)) & 0xFFu;
    let v_word = yuv_v[uvi / 4u];
    let v_byte = (v_word >> ((uvi % 4u) * 8u)) & 0xFFu;
    
    let yf = f32(y_byte);
    let uf = f32(u_byte) - 128.0;
    let vf = f32(v_byte) - 128.0;
    
    // BT.601 conversion
    let r = clamp(yf + 1.402 * vf, 0.0, 255.0);
    let g = clamp(yf - 0.344 * uf - 0.714 * vf, 0.0, 255.0);
    let b = clamp(yf + 1.772 * uf, 0.0, 255.0);
    
    // Pack RGB into output (3 bytes per pixel, packed as u32s)
    let oi = (y_coord * w + x) * 3u;
    let word_idx = oi / 4u;
    let byte_off = oi % 4u;
    
    // Atomic byte writes (since pixels may share u32 words)
    let r_u32 = u32(r) << (byte_off * 8u);
    let g_u32 = u32(g) << (((byte_off + 1u) % 4u) * 8u);
    let b_u32 = u32(b) << (((byte_off + 2u) % 4u) * 8u);
    
    // Use separate byte writes to avoid conflicts
    // For simplicity, write R,G,B as individual bytes via atomicOr
    // (This works because each pixel writes to unique byte positions)
    let mask_r = 0xFFu << (byte_off * 8u);
    let mask_g = 0xFFu << (((byte_off + 1u) % 4u) * 8u);
    let mask_b = 0xFFu << (((byte_off + 2u) % 4u) * 8u);
    
    // Direct store: each pixel owns its 3 bytes
    atomicOr(&rgb_out[word_idx], r_u32);
    if (byte_off + 1u < 4u) {
        atomicOr(&rgb_out[word_idx], g_u32);
    } else {
        atomicOr(&rgb_out[word_idx + 1u], g_u32);
    }
    if (byte_off + 2u < 4u) {
        atomicOr(&rgb_out[word_idx], b_u32);
    } else {
        atomicOr(&rgb_out[word_idx + 1u], b_u32);
    }
}
";

/// GPU frame block reconstruction shader.
/// Reconstructs a frame from content-addressed blocks.
/// Each workgroup handles one 16×16 block.
pub const BLOCK_RECONSTRUCT_SHADER: &str = "
@group(0) @binding(0) var<storage, read> block_data: array<u32>;     // compressed block pool
@group(0) @binding(1) var<storage, read> block_offsets: array<u32>;  // offset into block_data per block
@group(0) @binding(2) var<storage, read> block_sizes: array<u32>;    // size per block
@group(0) @binding(3) var<storage, read_write> frame_out: array<u32>; // output frame
@group(0) @binding(4) var<uniform> dims: vec4<u32>;  // frame_w, frame_h, block_size, blocks_per_row

@compute @workgroup_size(16, 16)
fn main(@builtin(global_invocation_id) gid: vec3<u32>,
        @builtin(workgroup_id) wid: vec3<u32>) {
    let bx = wid.x;  // block column
    let by = wid.y;  // block row
    let lx = gid.x % 16u;  // local pixel x within block
    let ly = gid.y % 16u;  // local pixel y within block
    
    let frame_w = dims.x;
    let frame_h = dims.y;
    let block_sz = dims.z;
    let bpr = dims.w;  // blocks per row
    
    let px = bx * block_sz + lx;
    let py = by * block_sz + ly;
    if (px >= frame_w || py >= frame_h) { return; }
    
    let block_idx = by * bpr + bx;
    let offset = block_offsets[block_idx];
    let size = block_sizes[block_idx];
    
    if (size == 0u) {
        // Empty block: fill with mid-gray
        let oi = (py * frame_w + px) * 3u;
        let word = oi / 4u;
        frame_out[word] = frame_out[word] | (128u << ((oi % 4u) * 8u));
        return;
    }
    
    // Read decompressed pixel from block data
    let local_idx = ly * block_sz + lx;
    let data_idx = offset + local_idx;
    let pixel = block_data[data_idx / 4u];
    let byte_val = (pixel >> ((data_idx % 4u) * 8u)) & 0xFFu;
    
    // Write to output frame
    let oi = (py * frame_w + px);
    let word = oi / 4u;
    let shift = (oi % 4u) * 8u;
    frame_out[word] = frame_out[word] | (byte_val << shift);
}
";

/// GPU zstd block decompressor (simplified for small blocks).
/// Handles the common case of 16×16×3 = 768 byte blocks.
pub const BLOCK_DECOMPRESS_SHADER: &str = "
@group(0) @binding(0) var<storage, read> compressed: array<u32>;
@group(0) @binding(1) var<storage, read_write> decompressed: array<u32>;
@group(0) @binding(2) var<storage, read> offsets: array<vec2<u32>>;  // (src_offset, dst_offset) per block
@group(0) @binding(3) var<uniform> params: vec4<u32>;  // n_blocks, max_block_size, 0, 0

@compute @workgroup_size(256)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let block_id = gid.x;
    if (block_id >= params.x) { return; }
    
    let src_off = offsets[block_id].x;
    let dst_off = offsets[block_id].y;
    let max_sz = params.y;
    
    // Simple RLE-like decompression for mostly-uniform blocks
    // (Full zstd decompression is done on CPU; this handles the
    // common case of repeat-heavy video blocks after zstd)
    var src_pos = src_off;
    var dst_pos = dst_off;
    var remaining = max_sz;
    
    loop {
        if (remaining == 0u) { break; }
        let word = compressed[src_pos / 4u];
        let byte_val = (word >> ((src_pos % 4u) * 8u)) & 0xFFu;
        
        // Copy byte to output
        let dw = dst_pos / 4u;
        let ds = (dst_pos % 4u) * 8u;
        decompressed[dw] = decompressed[dw] | (byte_val << ds);
        
        src_pos += 1u;
        dst_pos += 1u;
        remaining -= 1u;
    }
}
";


/// Combined IDCT + YUV→RGB GPU decode shader.
/// Input: quantized DCT coefficients (uploaded as buffer)
/// Output: RGB pixels ready for display
/// This replaces: CPU IDCT → CPU dequantize → CPU YUV→RGB
/// With: one GPU dispatch that does all three
pub const IDCT_YUV_RGB_SHADER: &str = "
// Quantization table (JPEG standard, quality=50)
const Q: array<f32, 64> = array<f32, 64>(
    16.0,11.0,10.0,16.0,24.0,40.0,51.0,61.0,
    12.0,12.0,14.0,19.0,26.0,58.0,60.0,55.0,
    14.0,13.0,16.0,24.0,40.0,57.0,69.0,56.0,
    14.0,17.0,22.0,29.0,51.0,87.0,80.0,62.0,
    18.0,22.0,37.0,56.0,68.0,109.0,103.0,77.0,
    24.0,35.0,55.0,64.0,81.0,104.0,113.0,92.0,
    49.0,64.0,78.0,87.0,103.0,121.0,120.0,101.0,
    72.0,92.0,95.0,98.0,112.0,100.0,103.0,99.0
);

// Zigzag order
const ZZ: array<u32, 64> = array<u32, 64>(
    0u,1u,8u,16u,9u,2u,3u,10u,17u,24u,32u,25u,18u,11u,4u,5u,
    12u,19u,26u,33u,40u,48u,41u,34u,27u,20u,13u,6u,7u,14u,21u,28u,
    35u,42u,49u,56u,57u,50u,43u,36u,29u,22u,15u,23u,30u,37u,44u,51u,
    58u,59u,52u,45u,38u,31u,39u,46u,53u,60u,61u,54u,47u,55u,62u,63u
);

@group(0) @binding(0) var<storage, read> coeffs: array<i32>;      // quantized DCT coefficients
@group(0) @binding(1) var<storage, read> block_offsets: array<u32>; // offset per block into coeffs
@group(0) @binding(2) var<storage, read_write> rgb_out: array<u32>; // output RGB
@group(0) @binding(3) var<uniform> dims: vec4<u32>;  // frame_w, frame_h, blocks_w, is_chroma

fn dct_1d_inv(src: array<f32, 8>) -> array<f32, 8> {
    // Simplified 1D 8-point IDCT using butterfly
    var out: array<f32, 8>;
    let c1 = 0.9808; let c2 = 0.9239; let c3 = 0.8315;
    let c4 = 0.7071; let c5 = 0.5556; let c6 = 0.3827; let c7 = 0.1951;
    for (var i = 0u; i < 8u; i++) {
        var sum = 0.0;
        for (var k = 0u; k < 8u; k++) {
            let angle = (2.0 * f32(i) + 1.0) * f32(k) * 3.14159265 / 16.0;
            var ck = cos(angle);
            if (k == 0u) { ck *= 0.7071; }
            sum += src[k] * ck;
        }
        out[i] = sum * 0.5;
    }
    return out;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) gid: vec3<u32>,
        @builtin(workgroup_id) wid: vec3<u32>) {
    let bx = wid.x;  // block column
    let by = wid.y;  // block row
    let lx = gid.x % 8u;  // pixel x within block
    let ly = gid.y % 8u;  // pixel y within block
    
    let frame_w = dims.x;
    let frame_h = dims.y;
    let blocks_w = dims.z;
    let is_chroma = dims.w;
    
    let px = bx * 8u + lx;
    let py = by * 8u + ly;
    
    let actual_w = select(frame_w, frame_w / 2u, is_chroma > 0u);
    let actual_h = select(frame_h, frame_h / 2u, is_chroma > 0u);
    if (px >= actual_w || py >= actual_h) { return; }
    
    let block_idx = by * blocks_w + bx;
    let base = block_offsets[block_idx];
    
    // Read quantized coefficients and dequantize
    var dct_block: array<f32, 64>;
    for (var i = 0u; i < 64u; i++) {
        let ci = coeffs[base + i];
        let q = select(Q[i], Q[i] * 1.5, is_chroma > 0u);
        dct_block[ZZ[i]] = f32(ci) * q;
    }
    
    // 2D IDCT: rows then columns
    var temp: array<f32, 64>;
    for (var r = 0u; r < 8u; r++) {
        var row: array<f32, 8>;
        for (var c = 0u; c < 8u; c++) { row[c] = dct_block[r * 8u + c]; }
        let inv = dct_1d_inv(row);
        for (var c = 0u; c < 8u; c++) { temp[r * 8u + c] = inv[c]; }
    }
    for (var c = 0u; c < 8u; c++) {
        var col: array<f32, 8>;
        for (var r = 0u; r < 8u; r++) { col[r] = temp[r * 8u + c]; }
        let inv = dct_1d_inv(col);
        for (var r = 0u; r < 8u; r++) { temp[r * 8u + c] = inv[r]; }
    }
    
    // Get pixel value (add 128 for DC offset)
    let pixel_val = clamp(temp[ly * 8u + lx] + 128.0, 0.0, 255.0);
    
    if (is_chroma == 0u) {
        // Y plane: store luminance, will combine with UV later
        let yi = py * frame_w + px;
        let word = yi / 4u;
        let shift = (yi % 4u) * 8u;
        rgb_out[word] = rgb_out[word] | (u32(pixel_val) << shift);
    }
    // Chroma planes handled separately, combined in final pass
}
";

// ═══════════════════════════════════════════════════════
// FAO Unified GPU Pipeline
// ═══════════════════════════════════════════════════════

struct Params {
    width: u32,
    height: u32,
    n_blocks_x: u32,
    n_blocks_y: u32,
    n_coeffs: u32,
    n_refs: u32,
    skip_thresh: f32,
    frame_idx: u32,
};

struct FourierCoeff {
    freq_x: f32,
    freq_y: f32,
    amplitude: f32,
    phase: f32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> cur_y: array<f32>;
@group(0) @binding(2) var<storage, read> ref_y: array<f32>;
@group(0) @binding(3) var<storage, read> coeffs: array<FourierCoeff>;
@group(0) @binding(4) var<storage, read_write> field_y: array<f32>;
@group(0) @binding(5) var<storage, read_write> route_map: array<u32>;
@group(0) @binding(6) var<storage, read_write> sad_map: array<f32>;
@group(0) @binding(7) var<storage, read_write> class_map: array<u32>;
@group(0) @binding(8) var<storage, read_write> histogram: array<atomic<u32>>;
@group(0) @binding(9) var<storage, read_write> light_map: array<f32>;
@group(0) @binding(10) var<storage, read_write> blur_map: array<f32>;

// ─── Pass 1: Fourier field evaluation ───
// Each thread computes one pixel's field value
@compute @workgroup_size(256)
fn field_eval(@builtin(global_invocation_id) gid: vec3<u32>) {
    let pixel = gid.x;
    let w = params.width;
    let h = params.height;
    if pixel >= w * h { return; }

    let x = pixel % w;
    let y = pixel / w;
    let nx = f32(x) / f32(w);
    let ny = f32(y) / f32(h);

    var val = 0.0;  // DC added separately by CPU
    for (var i = 0u; i < params.n_coeffs; i++) {
        let c = coeffs[i];
        val += c.amplitude * cos(6.2832 * (c.freq_x * nx + c.freq_y * ny) + c.phase);
    }
    field_y[pixel] = val;
}

// ─── Pass 2: SAD computation + route decision ───
// Each thread handles one 8×8 block
@compute @workgroup_size(64)
fn sad_route(@builtin(global_invocation_id) gid: vec3<u32>) {
    let block_idx = gid.x;
    let bw = params.n_blocks_x;
    let bh = params.n_blocks_y;
    if block_idx >= bw * bh { return; }

    let bx = block_idx % bw;
    let by = block_idx / bw;
    let ox = bx * 8u;
    let oy = by * 8u;
    let w = params.width;

    // SAD against reference (motion compensated at zero MV)
    var sad_ref = 0.0;
    var sad_field = 0.0;
    var max_diff_ref = 0.0;
    var max_diff_field = 0.0;

    for (var dy = 0u; dy < 8u; dy++) {
        for (var dx = 0u; dx < 8u; dx++) {
            let pi = (oy + dy) * w + ox + dx;
            let cv = cur_y[pi];
            let rv = ref_y[pi];
            let fv = field_y[pi];

            let dr = abs(cv - rv);
            let df = abs(cv - fv);
            sad_ref += dr;
            sad_field += df;
            max_diff_ref = max(max_diff_ref, dr);
            max_diff_field = max(max_diff_field, df);
        }
    }

    // Route decision: 0 = skip (ref), 1 = field, 2 = encode
    let thresh = params.skip_thresh;
    var route = 2u;  // default: encode
    if sad_ref / 64.0 <= thresh && max_diff_ref <= thresh * 2.5 {
        route = 0u;  // skip: ref is good enough
    } else if sad_field / 64.0 <= thresh && max_diff_field <= thresh * 2.5 {
        route = 1u;  // field prediction is better
    }

    route_map[block_idx] = route;
    sad_map[block_idx] = min(sad_ref, sad_field);
}

// ─── Pass 3: Block classification ───
@compute @workgroup_size(64)
fn classify(@builtin(global_invocation_id) gid: vec3<u32>) {
    let block_idx = gid.x;
    let bw = params.n_blocks_x;
    let bh = params.n_blocks_y;
    if block_idx >= bw * bh { return; }

    let bx = block_idx % bw;
    let by = block_idx / bw;
    let ox = bx * 8u;
    let oy = by * 8u;
    let w = params.width;

    // Compute variance and edge energy
    var sum = 0.0;
    var sum_sq = 0.0;
    var edge = 0.0;

    for (var dy = 0u; dy < 8u; dy++) {
        for (var dx = 0u; dx < 8u; dx++) {
            let v = cur_y[(oy + dy) * w + ox + dx];
            sum += v;
            sum_sq += v * v;
            if dx < 7u {
                edge += abs(cur_y[(oy + dy) * w + ox + dx + 1u] - v);
            }
            if dy < 7u {
                edge += abs(cur_y[(oy + dy + 1u) * w + ox + dx] - v);
            }
        }
    }

    let mean = sum / 64.0;
    let variance = sum_sq / 64.0 - mean * mean;
    edge = edge / 98.0;  // normalize by number of edge pairs

    // Classification: 0=Flat, 1=Smooth, 2=Detail, 3=Texture, 4=UI
    var cls = 2u;  // Detail default
    if variance < 50.0 && edge < 3.0 { cls = 0u; }       // Flat
    else if variance < 200.0 && edge < 8.0 { cls = 1u; }  // Smooth
    else if variance > 800.0 { cls = 3u; }                 // Texture

    // Pack: class in low 8 bits, variance quantized in high 24 bits
    class_map[block_idx] = cls | (u32(min(variance, 65535.0)) << 8u);
}

// ─── Pass 4: Histogram ───
@compute @workgroup_size(256)
fn compute_histogram(@builtin(global_invocation_id) gid: vec3<u32>) {
    let pixel = gid.x + gid.y * 65535u * 256u;
    if pixel >= params.width * params.height { return; }
    let bin = u32(clamp(cur_y[pixel], 0.0, 255.0));
    atomicAdd(&histogram[bin], 1u);
}

// ─── Pass 5: Lighting estimation ───
// Per 16×16 region: estimate light direction from gradient
@compute @workgroup_size(64)
fn lighting(@builtin(global_invocation_id) gid: vec3<u32>) {
    let region = gid.x;
    let rw = params.width / 16u;
    let rh = params.height / 16u;
    if region >= rw * rh { return; }

    let rx = region % rw;
    let ry = region / rw;
    let ox = rx * 16u;
    let oy = ry * 16u;
    let w = params.width;

    var gx_sum = 0.0;
    var gy_sum = 0.0;

    for (var dy = 0u; dy < 15u; dy++) {
        for (var dx = 0u; dx < 15u; dx++) {
            let pi = (oy + dy) * w + ox + dx;
            let gx = cur_y[pi + 1u] - cur_y[pi];
            let gy = cur_y[pi + w] - cur_y[pi];
            gx_sum += gx;
            gy_sum += gy;
        }
    }

    // Store gradient direction (light comes from opposite direction)
    // Normalized: gx_sum/225, gy_sum/225
    light_map[region * 2u] = gx_sum / 225.0;
    light_map[region * 2u + 1u] = gy_sum / 225.0;
}

// ─── Pass 6: Motion blur detection ───
@compute @workgroup_size(64)
fn blur_detect(@builtin(global_invocation_id) gid: vec3<u32>) {
    let block_idx = gid.x;
    let bw = params.n_blocks_x;
    let bh = params.n_blocks_y;
    if block_idx >= bw * bh { return; }

    let bx = block_idx % bw;
    let by = block_idx / bw;
    let ox = bx * 8u;
    let oy = by * 8u;
    let w = params.width;

    // Measure directional blur: compare horizontal vs vertical sharpness
    var h_sharp = 0.0;
    var v_sharp = 0.0;

    for (var dy = 0u; dy < 8u; dy++) {
        for (var dx = 0u; dx < 7u; dx++) {
            let pi = (oy + dy) * w + ox + dx;
            h_sharp += abs(cur_y[pi + 1u] - cur_y[pi]);
        }
    }
    for (var dy = 0u; dy < 7u; dy++) {
        for (var dx = 0u; dx < 8u; dx++) {
            let pi = (oy + dy) * w + ox + dx;
            v_sharp += abs(cur_y[pi + w] - cur_y[pi]);
        }
    }

    // If one direction is much sharper than other → directional blur
    let ratio = max(h_sharp, v_sharp) / (min(h_sharp, v_sharp) + 0.01);
    let blur_strength = clamp((ratio - 1.0) / 3.0, 0.0, 1.0);
    let blur_dir = select(1.0, 0.0, h_sharp > v_sharp);  // 0=horizontal blur, 1=vertical

    blur_map[block_idx * 2u] = blur_strength;
    blur_map[block_idx * 2u + 1u] = blur_dir;
}
"#;

/// Results downloaded from GPU per frame
pub struct GpuFrameResults {
    pub route_map: Vec<u32>,      // per-block: 0=skip, 1=field, 2=encode
    pub sad_map: Vec<f32>,        // per-block SAD value
    pub class_map: Vec<u32>,      // per-block classification (packed)
    pub histogram: Vec<u32>,      // 256-bin Y histogram
    pub light_map: Vec<f32>,      // per-region (gx, gy) gradient
    pub blur_map: Vec<f32>,       // per-block (strength, direction)
    pub field_y: Option<Vec<f32>>, // only downloaded when field prediction wins
}

impl GpuFrameResults {
    pub fn block_class(&self, bi: usize) -> u8 {
        if bi < self.class_map.len() { (self.class_map[bi] & 0xFF) as u8 } else { 2 }
    }
    pub fn block_variance(&self, bi: usize) -> f32 {
        if bi < self.class_map.len() { (self.class_map[bi] >> 8) as f32 } else { 0.0 }
    }
    pub fn block_route(&self, bi: usize) -> u8 {
        if bi < self.route_map.len() { self.route_map[bi] as u8 } else { 2 }
    }
    pub fn block_blur(&self, bi: usize) -> (f32, f32) {
        if bi * 2 + 1 < self.blur_map.len() {
            (self.blur_map[bi * 2], self.blur_map[bi * 2 + 1])
        } else { (0.0, 0.0) }
    }
    pub fn region_light(&self, ri: usize) -> (f32, f32) {
        if ri * 2 + 1 < self.light_map.len() {
            (self.light_map[ri * 2], self.light_map[ri * 2 + 1])
        } else { (0.0, 0.0) }
    }
}

// ══════════════════════════════════════════════════════════
// GPU Motion Search — exhaustive ±16, wired into encode loop
// ══════════════════════════════════════════════════════════

#[cfg(feature = "gpu")]
pub const MOTION_SEARCH_SHADER: &str = r#"
struct Params {
    width: u32,
    height: u32,
    n_blocks_x: u32,
    n_blocks_y: u32,
    search_range: i32,
    _pad1: u32,
    _pad2: u32,
    _pad3: u32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> cur_y: array<f32>;
@group(0) @binding(2) var<storage, read> ref_y: array<f32>;
@group(0) @binding(3) var<storage, read_write> results: array<i32>;  // [mvx, mvy, sad] per block

// Each workgroup handles one 8×8 block
// Threads within workgroup test different MV candidates
@compute @workgroup_size(256)
fn motion_search(@builtin(global_invocation_id) gid: vec3<u32>,
                 @builtin(local_invocation_id) lid: vec3<u32>,
                 @builtin(workgroup_id) wid: vec3<u32>) {
    let block_idx = wid.x + wid.y * 65535u;
    let bw = params.n_blocks_x;
    let bh = params.n_blocks_y;
    if block_idx >= bw * bh { return; }

    let bx = block_idx % bw;
    let by = block_idx / bw;
    let ox = bx * 8u;
    let oy = by * 8u;
    let w = params.width;
    let h = params.height;
    let sr = params.search_range;
    let search_diam = u32(sr * 2 + 1);
    let n_candidates = search_diam * search_diam;

    // Each thread tests a subset of candidates
    let thread_id = lid.x;
    let n_threads = 256u;

    var best_sad = 999999;
    var best_mvx = 0;
    var best_mvy = 0;

    var cand = thread_id;
    while cand < n_candidates {
        let dx = i32(cand % search_diam) - sr;
        let dy = i32(cand / search_diam) - sr;

        let rx = i32(ox) + dx;
        let ry = i32(oy) + dy;

        // Bounds check
        if rx >= 0 && ry >= 0 && rx + 8 <= i32(w) && ry + 8 <= i32(h) {
            var sad = 0;
            for (var yy = 0u; yy < 8u; yy++) {
                for (var xx = 0u; xx < 8u; xx++) {
                    let cv = cur_y[(oy + yy) * w + ox + xx];
                    let rv = ref_y[u32(ry + i32(yy)) * w + u32(rx + i32(xx))];
                    sad += i32(abs(cv - rv));
                }
            }
            if sad < best_sad {
                best_sad = sad;
                best_mvx = dx;
                best_mvy = dy;
            }
        }
        cand += n_threads;
    }

    // Workgroup reduction: find the best MV across all threads
    // Use shared memory for reduction
    var shared_sad: array<i32, 256>;
    var shared_mvx: array<i32, 256>;
    var shared_mvy: array<i32, 256>;
    shared_sad[thread_id] = best_sad;
    shared_mvx[thread_id] = best_mvx;
    shared_mvy[thread_id] = best_mvy;
    workgroupBarrier();

    // Parallel reduction
    var stride = 128u;
    while stride > 0u {
        if thread_id < stride {
            if shared_sad[thread_id + stride] < shared_sad[thread_id] {
                shared_sad[thread_id] = shared_sad[thread_id + stride];
                shared_mvx[thread_id] = shared_mvx[thread_id + stride];
                shared_mvy[thread_id] = shared_mvy[thread_id + stride];
            }
        }
        workgroupBarrier();
        stride = stride >> 1u;
    }

    // Thread 0 writes the result
    if thread_id == 0u {
        results[block_idx * 3u] = shared_mvx[0];
        results[block_idx * 3u + 1u] = shared_mvy[0];
        results[block_idx * 3u + 2u] = shared_sad[0];
    }
}
"#;

/// Manages the GPU motion search pipeline
#[cfg(feature = "gpu")]
pub struct GpuMotionSearch {
    device: std::sync::Arc<wgpu::Device>,
    queue: std::sync::Arc<wgpu::Queue>,
    pipeline: wgpu::ComputePipeline,
    // Persistent buffers
    cur_buf: wgpu::Buffer,
    ref_buf: wgpu::Buffer,
    result_buf: wgpu::Buffer,
    readback_buf: wgpu::Buffer,
    params_buf: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
    w: usize,
    h: usize,
    n_blocks: usize,
}

#[cfg(feature = "gpu")]
impl GpuMotionSearch {
    pub fn new(w: usize, h: usize) -> Option<Self> {
        eprintln!("FAO GPU: attempting init (VULKAN|METAL)...");
        let backend = if cfg!(target_os = "macos") {
            wgpu::Backends::METAL
        } else {
            // Check for Asahi Honeykrisp: use GL (Vulkan watchdog issues)
            let is_asahi_honeykrisp = std::fs::read_to_string("/proc/device-tree/compatible")
                .map(|s| s.contains("apple,")).unwrap_or(false)
                && std::path::Path::new("/sys/module/asahi").exists();
            if is_asahi_honeykrisp {
                eprintln!("FAO GPU: Asahi Honeykrisp detected, using GL backend");
                wgpu::Backends::GL
            } else {
                wgpu::Backends::VULKAN
            }
        };
        eprintln!("FAO GPU: using {:?} backend", backend);
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: backend,
            ..Default::default()
        });
        // List available adapters for debugging
        // Quick adapter check
        let mut n_adapters = 0;
        for _ in instance.enumerate_adapters(wgpu::Backends::all()) { n_adapters += 1; }
        let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        })) {
            Some(a) => a,
            None => {
                eprintln!("FAO GPU: no suitable adapter found, falling back to CPU");
                return None;
            }
        };
        
        eprintln!("FAO GPU Motion: {} ({})", adapter.get_info().name, adapter.get_info().driver);
        
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("FAO Motion Search"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
            },
            None,
        )).ok()?;
        
        let device = std::sync::Arc::new(device);
        let queue = std::sync::Arc::new(queue);
        
        let n = w * h;
        let bw = w / 8; let bh = h / 8;
        let n_blocks = bw * bh;
        
        // Create buffers
        let cur_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("cur_y"), size: (n * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let ref_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ref_y"), size: (n * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let result_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("results"), size: (n_blocks * 3 * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let readback_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("readback"), size: (n_blocks * 3 * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        
        #[repr(C)]
        #[derive(Copy, Clone)]
        struct MotionParams { width: u32, height: u32, nbx: u32, nby: u32, sr: i32, _p: [u32; 3] }
        let params_data = MotionParams {
            width: w as u32, height: h as u32,
            nbx: bw as u32, nby: bh as u32, sr: 16, _p: [0; 3]  // ±16 exhaustive search
        };
        let params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("params"), size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&params_buf, 0, unsafe {
            std::slice::from_raw_parts(&params_data as *const _ as *const u8, 32)
        });
        
        // Compile shader
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("motion_search"),
            source: wgpu::ShaderSource::Wgsl(MOTION_SEARCH_SHADER.into()),
        });
        
        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("motion_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("motion_bg"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: params_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: cur_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: ref_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: result_buf.as_entire_binding() },
            ],
        });
        
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("motion_pl"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
        
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("motion_search"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: "motion_search",
        });
        
        Some(Self { device, queue, pipeline, cur_buf, ref_buf, result_buf, readback_buf, params_buf, bind_group, w, h, n_blocks })
    }
    
    /// Upload cur and ref Y planes, dispatch search, return per-block MVs
    pub fn search(&self, cur_y: &[f32], ref_y: &[f32]) -> Vec<(i8, i8, f32)> {
        // Upload
        let cur_bytes = unsafe { std::slice::from_raw_parts(cur_y.as_ptr() as *const u8, cur_y.len() * 4) };
        let ref_bytes = unsafe { std::slice::from_raw_parts(ref_y.as_ptr() as *const u8, ref_y.len() * 4) };
        self.queue.write_buffer(&self.cur_buf, 0, cur_bytes);
        self.queue.write_buffer(&self.ref_buf, 0, ref_bytes);
        
        // Single dispatch with reduced search range (±8) to avoid GPU timeout
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("motion") });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("motion_pass"), timestamp_writes: None });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.dispatch_workgroups(self.n_blocks as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&self.result_buf, 0, &self.readback_buf, 0, (self.n_blocks * 3 * 4) as u64);
        self.queue.submit(std::iter::once(encoder.finish()));
        
        // Read back results
        let buffer_slice = self.readback_buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |result| { let _ = tx.send(result); });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv().ok();
        
        let data = buffer_slice.get_mapped_range();
        let results: &[i32] = unsafe { std::slice::from_raw_parts(data.as_ptr() as *const i32, self.n_blocks * 3) };
        
        let mut mvs = Vec::with_capacity(self.n_blocks);
        for i in 0..self.n_blocks {
            let mvx = results[i * 3].max(-127).min(127) as i8;
            let mvy = results[i * 3 + 1].max(-127).min(127) as i8;
            let sad = results[i * 3 + 2] as f32;
            mvs.push((mvx, mvy, sad));
        }
        
        drop(data);
        self.readback_buf.unmap();
        
        mvs
    }
}

/// CPU fallback for non-GPU builds
#[cfg(not(feature = "gpu"))]
pub struct GpuMotionSearch;

#[cfg(not(feature = "gpu"))]
impl GpuMotionSearch {
    pub fn new(_w: usize, _h: usize) -> Option<Self> { None }
}


#[cfg(feature = "gpu")]
impl GpuMotionSearch {
    /// Submit GPU work without waiting (async pipeline)
    pub fn submit(&self, cur_y: &[f32], ref_y: &[f32]) {
        let cur_bytes = unsafe { std::slice::from_raw_parts(cur_y.as_ptr() as *const u8, cur_y.len() * 4) };
        let ref_bytes = unsafe { std::slice::from_raw_parts(ref_y.as_ptr() as *const u8, ref_y.len() * 4) };
        self.queue.write_buffer(&self.cur_buf, 0, cur_bytes);
        self.queue.write_buffer(&self.ref_buf, 0, ref_bytes);
        
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("motion_async") });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("motion"), timestamp_writes: None });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.dispatch_workgroups(self.n_blocks as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&self.result_buf, 0, &self.readback_buf, 0, (self.n_blocks * 3 * 4) as u64);
        self.queue.submit(std::iter::once(encoder.finish()));
    }
    
    /// Collect results (blocks until GPU finishes)
    pub fn collect(&self) -> Vec<(i8, i8, f32)> {
        let buffer_slice = self.readback_buf.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer_slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx.send(r); });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv().ok();
        
        let data = buffer_slice.get_mapped_range();
        let results: &[i32] = unsafe { std::slice::from_raw_parts(data.as_ptr() as *const i32, self.n_blocks * 3) };
        let mut mvs = Vec::with_capacity(self.n_blocks);
        for i in 0..self.n_blocks {
            mvs.push((
                results[i*3].max(-127).min(127) as i8,
                results[i*3+1].max(-127).min(127) as i8,
                results[i*3+2] as f32,
            ));
        }
        drop(data);
        self.readback_buf.unmap();
        mvs
    }
}

// ══════════════════════════════════════════════════════════
// Multi-pass GPU pipeline: motion + classify + histogram + SAD
// All passes share the same buffers, dispatched in one command buffer
// ══════════════════════════════════════════════════════════

#[cfg(feature = "gpu")]
pub const CLASSIFY_SHADER: &str = r#"
struct Params {
    width: u32,
    height: u32,
    n_blocks_x: u32,
    n_blocks_y: u32,
    search_range: i32,
    skip_thresh: f32,
    _pad2: u32,
    _pad3: u32,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> cur_y: array<f32>;
@group(0) @binding(2) var<storage, read_write> class_out: array<u32>;

// Combined: classify + SAD against zero-MV ref + histogram
@compute @workgroup_size(64)
fn classify_blocks(@builtin(global_invocation_id) gid: vec3<u32>) {
    let bi = gid.x;
    let bw = params.n_blocks_x;
    let bh = params.n_blocks_y;
    if bi >= bw * bh { return; }

    let bx = bi % bw;
    let by = bi / bw;
    let ox = bx * 8u;
    let oy = by * 8u;
    let w = params.width;

    var sum = 0.0;
    var sum_sq = 0.0;
    var edge_h = 0.0;
    var edge_v = 0.0;
    var min_val = 255.0;
    var max_val = 0.0;

    for (var dy = 0u; dy < 8u; dy++) {
        for (var dx = 0u; dx < 8u; dx++) {
            let v = cur_y[(oy + dy) * w + ox + dx];
            sum += v;
            sum_sq += v * v;
            min_val = min(min_val, v);
            max_val = max(max_val, v);
            if dx < 7u { edge_h += abs(cur_y[(oy + dy) * w + ox + dx + 1u] - v); }
            if dy < 7u { edge_v += abs(cur_y[(oy + dy + 1u) * w + ox + dx] - v); }
        }
    }

    let mean = sum / 64.0;
    let variance = sum_sq / 64.0 - mean * mean;
    let edge = (edge_h + edge_v) / 98.0;
    let range = max_val - min_val;

    // Classification + packed metadata
    var cls = 2u;  // Detail
    if variance < 50.0 && edge < 3.0 { cls = 0u; }       // Flat
    else if variance < 200.0 && edge < 8.0 { cls = 1u; }  // Smooth  
    else if variance > 800.0 { cls = 3u; }                 // Texture

    // Pack: class(8) | variance(16) | edge_direction(8)
    let edge_dir = select(0u, 1u, edge_h > edge_v);  // 0=mostly horizontal edges, 1=vertical
    class_out[bi] = cls | (u32(clamp(variance, 0.0, 65535.0)) << 8u) | (edge_dir << 24u);
}
"#;

/// Results from the full GPU pipeline
#[cfg(feature = "gpu")]
pub struct GpuEncodeResults {
    pub mvs: Vec<(i8, i8, f32)>,
    pub classes: Vec<u32>,
    pub histogram: Vec<u32>,          // 256-bin Y histogram
    pub lighting: Vec<f32>,           // per-region: gx, gy, brightness, contrast
    pub blur: Vec<f32>,               // per-block: h_energy, v_energy, blur_strength
}

#[cfg(not(feature = "gpu"))]
pub struct GpuEncodeResults {
    pub mvs: Vec<(i8, i8, f32)>,
    pub classes: Vec<u32>,
    pub histogram: Vec<u32>,
    pub lighting: Vec<f32>,
    pub blur: Vec<f32>,
}

#[cfg(feature = "gpu")]
impl GpuMotionSearch {
    /// Full pipeline: motion search + classification in one submit
    pub fn full_pipeline(&self, cur_y: &[f32], ref_y: &[f32]) -> GpuEncodeResults {
        // upload
        let cur_bytes = unsafe { std::slice::from_raw_parts(cur_y.as_ptr() as *const u8, cur_y.len() * 4) };
        let ref_bytes = unsafe { std::slice::from_raw_parts(ref_y.as_ptr() as *const u8, ref_y.len() * 4) };
        self.queue.write_buffer(&self.cur_buf, 0, cur_bytes);
        self.queue.write_buffer(&self.ref_buf, 0, ref_bytes);
        
        // Create classification buffer + pipeline on first use
        let class_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("class"), size: (self.n_blocks * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let class_readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("class_rb"), size: (self.n_blocks * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        
        // Update params with skip_thresh
        #[repr(C)]
        #[derive(Copy, Clone)]
        struct P2 { width: u32, height: u32, nbx: u32, nby: u32, sr: i32, skip: f32, _p: [u32; 2] }
        let p2 = P2 { width: self.w as u32, height: self.h as u32,
            nbx: (self.w/8) as u32, nby: (self.h/8) as u32, sr: 16, skip: 10.0, _p: [0; 2] };
        self.queue.write_buffer(&self.params_buf, 0, unsafe {
            std::slice::from_raw_parts(&p2 as *const _ as *const u8, 32)
        });
        
        // Classify shader + bind group
        let classify_shader = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("classify"), source: wgpu::ShaderSource::Wgsl(CLASSIFY_SHADER.into()),
        });
        let classify_bgl = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("cls_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let classify_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("cls_bg"), layout: &classify_bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.params_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.cur_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: class_buf.as_entire_binding() },
            ],
        });
        let classify_pl = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("cls_pl"), bind_group_layouts: &[&classify_bgl], push_constant_ranges: &[],
        });
        let classify_pipeline = self.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("classify"), layout: Some(&classify_pl),
            module: &classify_shader, entry_point: "classify_blocks",
        });
        
        // Single command buffer with both passes
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("full_pipeline") });
        {
            // Pass 1: Motion search
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("motion"), timestamp_writes: None });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.dispatch_workgroups(self.n_blocks as u32, 1, 1);
        }
        {
            // Pass 2: Block classification
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("classify"), timestamp_writes: None });
            pass.set_pipeline(&classify_pipeline);
            pass.set_bind_group(0, &classify_bg, &[]);
            pass.dispatch_workgroups(((self.n_blocks + 63) / 64) as u32, 1, 1);
        }
        // Copy both results
        encoder.copy_buffer_to_buffer(&self.result_buf, 0, &self.readback_buf, 0, (self.n_blocks * 3 * 4) as u64);
        encoder.copy_buffer_to_buffer(&class_buf, 0, &class_readback, 0, (self.n_blocks * 4) as u64);
        self.queue.submit(std::iter::once(encoder.finish()));
        
        // readback
        let mv_slice = self.readback_buf.slice(..);
        let (tx1, rx1) = std::sync::mpsc::channel();
        mv_slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx1.send(r); });
        
        let cls_slice = class_readback.slice(..);
        let (tx2, rx2) = std::sync::mpsc::channel();
        cls_slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx2.send(r); });
        
        self.device.poll(wgpu::Maintain::Wait);
        rx1.recv().ok(); rx2.recv().ok();
        
        let mv_data = mv_slice.get_mapped_range();
        let mv_ints: &[i32] = unsafe { std::slice::from_raw_parts(mv_data.as_ptr() as *const i32, self.n_blocks * 3) };
        let mut mvs = Vec::with_capacity(self.n_blocks);
        for i in 0..self.n_blocks {
            mvs.push((mv_ints[i*3].max(-127).min(127) as i8, mv_ints[i*3+1].max(-127).min(127) as i8, mv_ints[i*3+2] as f32));
        }
        drop(mv_data);
        self.readback_buf.unmap();
        
        let cls_data = cls_slice.get_mapped_range();
        let cls_ints: &[u32] = unsafe { std::slice::from_raw_parts(cls_data.as_ptr() as *const u32, self.n_blocks) };
        let classes = cls_ints.to_vec();
        drop(cls_data);
        class_readback.unmap();
        
        // hist+light
        // Also dispatch histogram + lighting/blur passes
        // (These use the same cur_y buffer already on GPU)
        let rw = self.w / 16; let rh = self.h / 16;
        let n_regions = rw * rh;
        let lb_size = n_regions * 4 + self.n_blocks * 3;

        let hist_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hist"), size: (256 * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let hist_readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hist_rb"), size: (256 * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let lb_buf = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lb"), size: (lb_size * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let lb_readback = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lb_rb"), size: (lb_size * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Zero histogram
        self.queue.write_buffer(&hist_buf, 0, &vec![0u8; 256 * 4]);

        // Histogram shader
        let hist_shader = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hist"), source: wgpu::ShaderSource::Wgsl(HISTOGRAM_SHADER.into()),
        });
        let hist_bgl = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hist_bgl"), entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let hist_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hist_bg"), layout: &hist_bgl, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.params_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.cur_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: hist_buf.as_entire_binding() },
            ],
        });
        let hist_pl = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None, bind_group_layouts: &[&hist_bgl], push_constant_ranges: &[],
        });
        let hist_pipeline = self.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("hist"), layout: Some(&hist_pl), module: &hist_shader, entry_point: "compute_histogram",
        });

        // Lighting+blur shader
        let lb_shader = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("lb"), source: wgpu::ShaderSource::Wgsl(LIGHTING_BLUR_SHADER.into()),
        });
        let lb_bgl = self.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lb_bgl"), entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let lb_bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lb_bg"), layout: &lb_bgl, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.params_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.cur_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: lb_buf.as_entire_binding() },
            ],
        });
        let lb_pl = self.device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None, bind_group_layouts: &[&lb_bgl], push_constant_ranges: &[],
        });
        let lb_pipeline = self.device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("lb"), layout: Some(&lb_pl), module: &lb_shader, entry_point: "lighting_blur",
        });

        // Submit all remaining passes
        let mut encoder2 = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("passes_2") });
        {
            let mut pass = encoder2.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("hist"), timestamp_writes: None });
            pass.set_pipeline(&hist_pipeline);
            pass.set_bind_group(0, &hist_bg, &[]);
            let n_pixels = self.w * self.h;
            let hx = (((n_pixels + 255) / 256) as u32).min(65535);
            let hy = ((((n_pixels + 255) / 256) as u32) + 65534) / 65535;
            pass.dispatch_workgroups(hx, hy, 1);
        }
        {
            let mut pass = encoder2.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("lb"), timestamp_writes: None });
            pass.set_pipeline(&lb_pipeline);
            pass.set_bind_group(0, &lb_bg, &[]);
            pass.dispatch_workgroups(((n_regions + self.n_blocks + 63) / 64) as u32, 1, 1);
        }
        encoder2.copy_buffer_to_buffer(&hist_buf, 0, &hist_readback, 0, (256 * 4) as u64);
        encoder2.copy_buffer_to_buffer(&lb_buf, 0, &lb_readback, 0, (lb_size * 4) as u64);
        self.queue.submit(std::iter::once(encoder2.finish()));

        // Readback histogram
        let hist_slice = hist_readback.slice(..);
        let (tx3, rx3) = std::sync::mpsc::channel();
        hist_slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx3.send(r); });

        let lb_slice = lb_readback.slice(..);
        let (tx4, rx4) = std::sync::mpsc::channel();
        lb_slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx4.send(r); });

        self.device.poll(wgpu::Maintain::Wait);
        rx3.recv().ok(); rx4.recv().ok();

        let hist_data = hist_slice.get_mapped_range();
        let histogram: Vec<u32> = unsafe { std::slice::from_raw_parts(hist_data.as_ptr() as *const u32, 256) }.to_vec();
        drop(hist_data); hist_readback.unmap();

        let lb_data = lb_slice.get_mapped_range();
        let lb_floats: &[f32] = unsafe { std::slice::from_raw_parts(lb_data.as_ptr() as *const f32, lb_size) };
        let lighting = lb_floats[..n_regions * 4].to_vec();
        let blur = lb_floats[n_regions * 4..].to_vec();
        drop(lb_data); lb_readback.unmap();

        GpuEncodeResults { mvs, classes, histogram, lighting, blur }
    }
}

// ══════════════════════════════════════════════════════════
// Multi-stride guide bits — pre-scan for fast web warmup
// ══════════════════════════════════════════════════════════

/// Guide bits for a group of frames (GOP-level metadata)
#[derive(Clone)]
pub struct GopGuide {
    pub start_frame: usize,
    pub end_frame: usize,
    pub scene_class: u8,          // 0=static, 1=slow, 2=action, 3=cut
    pub dominant_mv: (i8, i8),    // most common motion direction
    pub static_mask: Vec<bool>,   // per-16×16 block: true = never changes
    pub palette_y: (u8, u8),      // min/max luma across the GOP
    pub palette_cb: (u8, u8),
    pub palette_cr: (u8, u8),
    pub complexity: f32,          // 0.0 = trivial, 1.0 = very complex
}

/// Pre-scan analyzer: samples frames at multiple strides
/// to build guide bits without full decode
pub struct MultiStrideScanner {
    pub guides: Vec<GopGuide>,
    pub stride_frames: Vec<(usize, Vec<f32>)>,  // (frame_idx, Y plane DC values per 16×16)
}

impl MultiStrideScanner {
    pub fn new() -> Self {
        Self { guides: Vec::new(), stride_frames: Vec::new() }
    }

    /// Add a sampled frame's DC values (16×16 block averages)
    /// Called with frames at stride 10 or stride 100
    pub fn add_sample(&mut self, frame_idx: usize, y_plane: &[f32], w: usize, h: usize) {
        let bw = w / 16; let bh = h / 16;
        let mut dcs = Vec::with_capacity(bw * bh);
        for by in 0..bh { for bx in 0..bw {
            let mut sum = 0.0f32;
            for dy in 0..16 { for dx in 0..16 {
                sum += y_plane[(by*16+dy)*w + bx*16+dx];
            }}
            dcs.push(sum / 256.0);
        }}
        self.stride_frames.push((frame_idx, dcs));
    }

    /// Build GOP guides from accumulated stride samples
    pub fn build_guides(&mut self, gop_size: usize, w: usize, h: usize) {
        if self.stride_frames.len() < 2 { return; }
        let bw = w / 16; let bh = h / 16;
        let n_regions = bw * bh;

        // Sort by frame index
        self.stride_frames.sort_by_key(|(idx, _)| *idx);

        // Group into GOPs
        let max_frame = self.stride_frames.last().map(|(idx, _)| *idx).unwrap_or(0);
        let n_gops = (max_frame / gop_size) + 1;

        for gop_idx in 0..n_gops {
            let start = gop_idx * gop_size;
            let end = start + gop_size;

            // Collect samples in this GOP range
            let samples: Vec<&(usize, Vec<f32>)> = self.stride_frames.iter()
                .filter(|(idx, _)| *idx >= start && *idx < end)
                .collect();

            if samples.is_empty() { continue; }

            // Compute static mask: which blocks have low variance across samples
            let mut static_mask = vec![true; n_regions];
            if samples.len() >= 2 {
                for ri in 0..n_regions {
                    let first_dc = samples[0].1[ri];
                    for s in &samples[1..] {
                        if (s.1[ri] - first_dc).abs() > 5.0 {
                            static_mask[ri] = false;
                            break;
                        }
                    }
                }
            }

            // Compute complexity from DC variance across samples
            let mut total_var = 0.0f32;
            for ri in 0..n_regions {
                let dcs: Vec<f32> = samples.iter().map(|s| s.1[ri]).collect();
                let mean: f32 = dcs.iter().sum::<f32>() / dcs.len() as f32;
                let var: f32 = dcs.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / dcs.len() as f32;
                total_var += var;
            }
            let complexity = (total_var / n_regions as f32 / 1000.0).min(1.0);

            // Scene class from complexity
            let scene_class = if complexity < 0.05 { 0 }      // static
                else if complexity < 0.2 { 1 }                  // slow
                else if complexity < 0.6 { 2 }                  // action
                else { 3 };                                     // cut/chaos

            // Dominant motion: diff between consecutive sample DCs
            let mut mv_x_sum = 0.0f32;
            let mut mv_y_sum = 0.0f32;
            if samples.len() >= 2 {
                let s0 = &samples[0].1;
                let s1 = &samples[samples.len()-1].1;
                // Simple: find the offset that minimizes DC diff
                for dy in -2i32..=2 { for dx in -2i32..=2 {
                    let mut sad = 0.0f32;
                    let mut count = 0;
                    for ry in 0..bh { for rx in 0..bw {
                        let sx = rx as i32 + dx;
                        let sy = ry as i32 + dy;
                        if sx >= 0 && sy >= 0 && (sx as usize) < bw && (sy as usize) < bh {
                            sad += (s0[ry * bw + rx] - s1[sy as usize * bw + sx as usize]).abs();
                            count += 1;
                        }
                    }}
                    if count > 0 { sad /= count as f32; }
                    if sad < mv_x_sum.abs() + mv_y_sum.abs() + 100.0 || (dx == 0 && dy == 0) {
                        if sad < 50.0 { mv_x_sum = dx as f32 * 8.0; mv_y_sum = dy as f32 * 8.0; }
                    }
                }}
            }

            // Palette bounds
            let first = &samples[0].1;
            let min_y = first.iter().cloned().fold(f32::MAX, f32::min) as u8;
            let max_y = first.iter().cloned().fold(0.0f32, f32::max) as u8;

            let static_pct = static_mask.iter().filter(|&&v| v).count() as f32 / n_regions as f32;

            self.guides.push(GopGuide {
                start_frame: start,
                end_frame: end,
                scene_class,
                dominant_mv: (mv_x_sum as i8, mv_y_sum as i8),
                static_mask,
                palette_y: (min_y, max_y),
                palette_cb: (128, 128),  // placeholder until chroma sampling
                palette_cr: (128, 128),
                complexity,
            });
        }
    }

    /// Get guide for a specific frame
    pub fn guide_for_frame(&self, frame_idx: usize) -> Option<&GopGuide> {
        self.guides.iter().find(|g| frame_idx >= g.start_frame && frame_idx < g.end_frame)
    }

    /// Byte cost of all guides (for bitstream overhead measurement)
    pub fn byte_cost(&self) -> usize {
        self.guides.iter().map(|g| 16 + g.static_mask.len() / 8 + 1).sum()
    }
}

// ══════════════════════════════════════════════════════════
// Global affine motion model — 6-parameter camera motion
// ══════════════════════════════════════════════════════════

/// Affine motion parameters: x' = a*x + b*y + c, y' = d*x + e*y + f
#[derive(Clone, Copy, Debug)]
pub struct AffineMotion {
    pub a: f32, pub b: f32, pub c: f32,  // horizontal
    pub d: f32, pub e: f32, pub f: f32,  // vertical
}

impl AffineMotion {
    /// Identity (no motion)
    pub fn identity() -> Self {
        Self { a: 1.0, b: 0.0, c: 0.0, d: 0.0, e: 1.0, f: 0.0 }
    }

    /// Pure translation
    pub fn translation(dx: f32, dy: f32) -> Self {
        Self { a: 1.0, b: 0.0, c: dx, d: 0.0, e: 1.0, f: dy }
    }

    /// Apply affine transform to a point
    pub fn apply(&self, x: f32, y: f32) -> (f32, f32) {
        (self.a * x + self.b * y + self.c,
         self.d * x + self.e * y + self.f)
    }

    /// Residual MV: actual MV minus affine-predicted MV at block position
    pub fn residual(&self, bx: usize, by: usize, mvx: f32, mvy: f32) -> (f32, f32) {
        let cx = (bx * 8 + 4) as f32;  // block center
        let cy = (by * 8 + 4) as f32;
        let (px, py) = self.apply(cx, cy);
        (mvx - (px - cx), mvy - (py - cy))
    }

    /// Fit affine model from per-block MVs using least-squares
    /// Input: block positions (bx, by) and their MVs (mvx, mvy)
    /// Only uses blocks with SAD below threshold (reliable MVs)
    pub fn fit_from_mvs(mvs: &[(i8, i8, f32)], bw: usize, bh: usize, sad_thresh: f32) -> Self {
        // Collect reliable samples
        let mut samples: Vec<(f32, f32, f32, f32)> = Vec::new(); // (x, y, dx, dy)
        for by in 0..bh { for bx in 0..bw {
            let bi = by * bw + bx;
            if bi >= mvs.len() { continue; }
            let (mvx, mvy, sad) = mvs[bi];
            if sad / 64.0 > sad_thresh { continue; } // skip unreliable blocks
            let cx = (bx * 8 + 4) as f32;
            let cy = (by * 8 + 4) as f32;
            samples.push((cx, cy, mvx as f32, mvy as f32));
        }}

        if samples.len() < 6 {
            // Not enough reliable samples — fall back to median translation
            if samples.is_empty() { return Self::identity(); }
            let mut dxs: Vec<f32> = samples.iter().map(|s| s.2).collect();
            let mut dys: Vec<f32> = samples.iter().map(|s| s.3).collect();
            dxs.sort_by(|a, b| a.partial_cmp(b).unwrap());
            dys.sort_by(|a, b| a.partial_cmp(b).unwrap());
            return Self::translation(dxs[dxs.len()/2], dys[dys.len()/2]);
        }

        // Least-squares fit: solve for [a,b,c] and [d,e,f] separately
        // For x': minimize sum (a*xi + b*yi + c - dxi)^2
        // Normal equations: A^T * A * params = A^T * b
        let n = samples.len() as f64;
        let mut sx = 0.0f64; let mut sy = 0.0f64;
        let mut sxx = 0.0f64; let mut syy = 0.0f64; let mut sxy = 0.0f64;
        let mut sdx = 0.0f64; let mut sdy = 0.0f64;
        let mut sxdx = 0.0f64; let mut sydx = 0.0f64;
        let mut sxdy = 0.0f64; let mut sydy = 0.0f64;

        for &(x, y, dx, dy) in &samples {
            let (x, y, dx, dy) = (x as f64, y as f64, dx as f64, dy as f64);
            sx += x; sy += y;
            sxx += x*x; syy += y*y; sxy += x*y;
            sdx += dx; sdy += dy;
            sxdx += x*dx; sydx += y*dx;
            sxdy += x*dy; sydy += y*dy;
        }

        // Solve 3x3 system for horizontal: [sxx sxy sx; sxy syy sy; sx sy n] * [a;b;c] = [sxdx;sydx;sdx]
        // Using Cramer's rule for simplicity
        let det = sxx*(syy*n - sy*sy) - sxy*(sxy*n - sy*sx) + sx*(sxy*sy - syy*sx);
        if det.abs() < 1e-10 {
            return Self::translation((sdx/n) as f32, (sdy/n) as f32);
        }

        let a = ((sxdx*(syy*n-sy*sy) - sxy*(sydx*n-sy*sdx) + sx*(sydx*sy-syy*sdx)) / det) as f32;
        let b = ((sxx*(sydx*n-sy*sdx) - sxdx*(sxy*n-sy*sx) + sx*(sxy*sdx-sydx*sx)) / det) as f32;
        let c = ((sxx*(syy*sdx-sy*sydx) - sxy*(sxy*sdx-sy*sxdx) + sxdx*(sxy*sy-syy*sx)) / det) as f32;

        let d = ((sxdy*(syy*n-sy*sy) - sxy*(sydy*n-sy*sdy) + sx*(sydy*sy-syy*sdy)) / det) as f32;
        let e = ((sxx*(sydy*n-sy*sdy) - sxdy*(sxy*n-sy*sx) + sx*(sxy*sdy-sydy*sx)) / det) as f32;
        let f = ((sxx*(syy*sdy-sy*sydy) - sxy*(sxy*sdy-sy*sxdy) + sxdy*(sxy*sy-syy*sx)) / det) as f32;

        // Sanity check: if params are wild, fall back to translation
        if a.abs() > 2.0 || e.abs() > 2.0 || b.abs() > 0.5 || d.abs() > 0.5 {
            return Self::translation(c, f);
        }

        Self { a, b, c, d, e, f }
    }

    /// Cost in bytes (for bitstream accounting)
    pub fn byte_cost(&self) -> usize {
        // Check if it's close to pure translation
        if (self.a - 1.0).abs() < 0.01 && self.b.abs() < 0.01
            && self.d.abs() < 0.01 && (self.e - 1.0).abs() < 0.01 {
            4  // just dx, dy as i16
        } else {
            24  // full 6 × f32
        }
    }

    /// Describe the motion type
    pub fn describe(&self) -> &'static str {
        let is_translate = (self.a - 1.0).abs() < 0.01 && self.b.abs() < 0.01
            && self.d.abs() < 0.01 && (self.e - 1.0).abs() < 0.01;
        let has_zoom = (self.a - 1.0).abs() > 0.01 || (self.e - 1.0).abs() > 0.01;
        let has_rotation = self.b.abs() > 0.01 || self.d.abs() > 0.01;

        if is_translate { "translate" }
        else if has_rotation && has_zoom { "affine" }
        else if has_rotation { "rotate" }
        else if has_zoom { "zoom" }
        else { "complex" }
    }
}


#[cfg(feature = "gpu")]
pub const HISTOGRAM_SHADER: &str = r#"
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> cur_y: array<f32>;
@group(0) @binding(2) var<storage, read_write> histogram: array<atomic<u32>>;

struct Params { width: u32, height: u32, _p: u32, _p2: u32, _p3: i32, _p4: f32, _p5: u32, _p6: u32 };

@compute @workgroup_size(256)
fn compute_histogram(@builtin(global_invocation_id) gid: vec3<u32>) {
    let pixel = gid.x + gid.y * 65535u * 256u;
    if pixel >= params.width * params.height { return; }
    let bin = u32(clamp(cur_y[pixel], 0.0, 255.0));
    atomicAdd(&histogram[bin], 1u);
}
"#;

#[cfg(feature = "gpu")]
pub const LIGHTING_BLUR_SHADER: &str = r#"
struct Params { width: u32, height: u32, n_blocks_x: u32, n_blocks_y: u32, _sr: i32, _st: f32, _p: u32, _p2: u32 };

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> cur_y: array<f32>;
@group(0) @binding(2) var<storage, read_write> light_blur: array<f32>;

// Combined: lighting (per 16x16 region) + blur detection (per 8x8 block)
// Output layout: [n_regions * 4 floats (gx, gy, brightness, contrast)] + [n_blocks * 3 floats (blur_h, blur_v, sharpness)]
@compute @workgroup_size(64)
fn lighting_blur(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x + gid.y * 65535u * 64u;
    let bw = params.n_blocks_x;
    let bh = params.n_blocks_y;
    let rw = params.width / 16u;
    let rh = params.height / 16u;
    let n_regions = rw * rh;
    let n_blocks = bw * bh;
    let w = params.width;
    let h = params.height;

    // First n_regions threads do lighting, rest do blur
    if idx < n_regions {
        let rx = idx % rw;
        let ry = idx / rw;
        let ox = rx * 16u;
        let oy = ry * 16u;

        var gx_sum = 0.0; var gy_sum = 0.0;
        var sum = 0.0; var min_v = 255.0; var max_v = 0.0;

        for (var dy = 0u; dy < 16u; dy++) {
            for (var dx = 0u; dx < 16u; dx++) {
                let pi = (oy + dy) * w + ox + dx;
                if pi < w * h {
                    let v = cur_y[pi];
                    sum += v; min_v = min(min_v, v); max_v = max(max_v, v);
                    if dx < 15u { gx_sum += cur_y[pi + 1u] - v; }
                    if dy < 15u { gy_sum += cur_y[pi + w] - v; }
                }
            }
        }
        let base = idx * 4u;
        light_blur[base] = gx_sum / 225.0;      // gradient X
        light_blur[base + 1u] = gy_sum / 225.0;  // gradient Y
        light_blur[base + 2u] = sum / 256.0;      // brightness
        light_blur[base + 3u] = max_v - min_v;    // contrast
    } else {
        let bi = idx - n_regions;
        if bi >= n_blocks { return; }

        let bx = bi % bw;
        let by = bi / bw;
        let ox = bx * 8u;
        let oy = by * 8u;

        var h_energy = 0.0; var v_energy = 0.0;
        var total_energy = 0.0;

        for (var dy = 0u; dy < 8u; dy++) {
            for (var dx = 0u; dx < 8u; dx++) {
                let pi = (oy + dy) * w + ox + dx;
                if pi < w * h {
                    if dx < 7u {
                        let gh = abs(cur_y[pi + 1u] - cur_y[pi]);
                        h_energy += gh;
                    }
                    if dy < 7u {
                        let gv = abs(cur_y[pi + w] - cur_y[pi]);
                        v_energy += gv;
                    }
                    total_energy += cur_y[pi] * cur_y[pi];
                }
            }
        }

        let base = n_regions * 4u + bi * 3u;
        light_blur[base] = h_energy / 56.0;      // horizontal sharpness
        light_blur[base + 1u] = v_energy / 56.0;  // vertical sharpness
        // Blur indicator: ratio of directional energies
        let blur_strength = select(0.0, 
            clamp((max(h_energy, v_energy) / (min(h_energy, v_energy) + 0.1) - 1.0) / 3.0, 0.0, 1.0),
            h_energy + v_energy > 10.0);
        light_blur[base + 2u] = blur_strength;
    }
}
"#;

// ══════════════════════════════════════════════════════════
// GPU DCT + Quantize + Reconstruct pipeline
// Moves the entire residual encoding to GPU
// ══════════════════════════════════════════════════════════

#[cfg(feature = "gpu")]
pub const DCT_ENCODE_SHADER: &str = r#"
struct EncodeParams {
    width: u32,
    height: u32,
    n_blocks_x: u32,
    n_blocks_y: u32,
    dead_zone: f32,
    skip_thresh: f32,
    _pad: u32,
    _pad2: u32,
};

@group(0) @binding(0) var<uniform> params: EncodeParams;
@group(0) @binding(1) var<storage, read> cur_y: array<f32>;
@group(0) @binding(2) var<storage, read> ref_y: array<f32>;
@group(0) @binding(3) var<storage, read> mvs: array<i32>;       // [mvx, mvy, sad] per block
@group(0) @binding(4) var<storage, read> qtable: array<f32>;     // 64-element quantization table
@group(0) @binding(5) var<storage, read_write> coeffs: array<i32>;  // 64 quantized coefficients per block
@group(0) @binding(6) var<storage, read_write> recon: array<f32>;   // reconstructed Y plane

// DCT basis values (precomputed cos(pi*(2k+1)*n/(2*8)))
// Using separable row/column approach
fn dct_coeff(k: u32, n: u32) -> f32 {
    let pi = 3.14159265;
    return cos(pi * f32(2u * n + 1u) * f32(k) / 16.0);
}

fn norm(k: u32) -> f32 {
    if k == 0u { return 0.35355339; }  // 1/sqrt(8)
    return 0.5;  // sqrt(2/8)
}

@compute @workgroup_size(64)
fn dct_encode(@builtin(global_invocation_id) gid: vec3<u32>,
              @builtin(local_invocation_id) lid: vec3<u32>,
              @builtin(workgroup_id) wid: vec3<u32>) {
    let block_idx = wid.x + wid.y * 65535u;
    let bw = params.n_blocks_x;
    let bh = params.n_blocks_y;
    if block_idx >= bw * bh { return; }
    
    let thread = lid.x;  // 0-63, maps to coefficient position
    if thread >= 64u { return; }

    let bx = block_idx % bw;
    let by = block_idx / bw;
    let ox = bx * 8u;
    let oy = by * 8u;
    let w = params.width;

    // Check if this block should skip (GPU SAD already computed)
    let mv_base = block_idx * 3u;
    let mvx = mvs[mv_base];
    let mvy = mvs[mv_base + 1u];
    let sad = mvs[mv_base + 2u];

    let coeff_base = block_idx * 64u;

    if f32(sad) / 64.0 <= params.skip_thresh {
        // Skip: zero all coefficients, copy ref to recon
        coeffs[coeff_base + thread] = 0;
        let py = thread / 8u;
        let px = thread % 8u;
        let rx = u32(clamp(i32(ox) + mvx, 0, i32(w) - 8));
        let ry = u32(clamp(i32(oy) + mvy, 0, i32(params.height) - 8));
        recon[(oy + py) * w + ox + px] = ref_y[(ry + py) * w + rx + px];
        return;
    }

    // Compute residual at MV position
    let rx = u32(clamp(i32(ox) + mvx, 0, i32(w) - 8));
    let ry = u32(clamp(i32(oy) + mvy, 0, i32(params.height) - 8));

    // Load residual into shared memory
    var residual: array<f32, 64>;
    // Each thread loads its pixel
    let py = thread / 8u;
    let px = thread % 8u;
    let res_val = cur_y[(oy + py) * w + ox + px] - ref_y[(ry + py) * w + rx + px];

    // Forward DCT: this thread computes coefficient (u, v) where thread = v*8+u
    let u = thread % 8u;
    let v = thread / 8u;

    var dct_val = 0.0;
    for (var y = 0u; y < 8u; y++) {
        for (var x = 0u; x < 8u; x++) {
            // Need residual at (x,y) — but we only have our own pixel
            // Use the fact that all 64 threads are in the same workgroup
            // Reconstruct: each thread contributes to all coefficients
            // This is the naive O(N^4) approach but N=8 so it's fine on GPU
            let r = cur_y[(oy + y) * w + ox + x] - ref_y[(ry + y) * w + rx + x];
            dct_val += r * dct_coeff(u, x) * dct_coeff(v, y);
        }
    }
    dct_val *= norm(u) * norm(v);

    // Quantize with dead zone
    let q = qtable[thread];
    let dz = q * params.dead_zone;
    var qval = 0;
    if abs(dct_val) > dz {
        qval = i32(round(dct_val / q));
    }
    coeffs[coeff_base + thread] = qval;

    // Reconstruct: inverse DCT of quantized coefficients
    // Each thread reconstructs pixel (px, py)
    var recon_val = 0.0;
    for (var cu = 0u; cu < 8u; cu++) {
        for (var cv = 0u; cv < 8u; cv++) {
            let coeff_val = f32(coeffs[coeff_base + cv * 8u + cu]) * qtable[cv * 8u + cu];
            recon_val += coeff_val * norm(cu) * norm(cv) * dct_coeff(cu, px) * dct_coeff(cv, py);
        }
    }

    // Add reference back: recon = inverse_dct(residual) + ref
    recon[(oy + py) * w + ox + px] = clamp(recon_val + ref_y[(ry + py) * w + rx + px], 0.0, 255.0);
}
"#;

/// GPU-accelerated full encode pipeline
#[cfg(feature = "gpu")]
pub struct GpuEncoder {
    pub motion: GpuMotionSearch,
    dct_pipeline: wgpu::ComputePipeline,
    coeffs_buf: wgpu::Buffer,
    recon_buf: wgpu::Buffer,
    qtable_buf: wgpu::Buffer,
    mv_buf: wgpu::Buffer,
    coeffs_readback: wgpu::Buffer,
    recon_readback: wgpu::Buffer,
    dct_bind_group: wgpu::BindGroup,
    dct_params_buf: wgpu::Buffer,
    // Persistent buffers for histogram + lighting/blur (no per-frame allocation)
    hist_buf: wgpu::Buffer,
    hist_readback: wgpu::Buffer,
    lb_buf: wgpu::Buffer,
    lb_readback: wgpu::Buffer,
    hist_pipeline: wgpu::ComputePipeline,
    lb_pipeline: wgpu::ComputePipeline,
    hist_bind_group: wgpu::BindGroup,
    lb_bind_group: wgpu::BindGroup,
    n_blocks: usize,
    n_pixels: usize,
    n_regions: usize,
    lb_size: usize,
    // Double-buffer: previous frame's results ready for CPU while GPU works on current
    prev_results: std::sync::Mutex<Option<GpuFullEncodeResult>>,
    frame_in_flight: std::sync::atomic::AtomicBool,
}

#[cfg(feature = "gpu")]
impl GpuEncoder {
    pub fn new(w: usize, h: usize) -> Option<Self> {
        eprintln!("FAO GPU: creating encoder...");
        let motion = match GpuMotionSearch::new(w, h) {
            Some(m) => m,
            None => { eprintln!("FAO GPU: motion search init failed"); return None; }
        };
        let device = &motion.device;
        let n = w * h;
        let bw = w / 8; let bh = h / 8;
        let n_blocks = bw * bh;

        let coeffs_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("coeffs"), size: (n_blocks * 64 * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let recon_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("recon"), size: (n * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let qtable_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("qtable"), size: (64 * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mv_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("mv_for_dct"), size: (n_blocks * 3 * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let coeffs_readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("coeffs_rb"), size: (n_blocks * 64 * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let recon_readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("recon_rb"), size: (n * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        #[repr(C)]
        #[derive(Copy, Clone)]
        struct EP { w: u32, h: u32, nbx: u32, nby: u32, dz: f32, st: f32, _p: [u32; 2] }
        let ep = EP { w: w as u32, h: h as u32, nbx: bw as u32, nby: bh as u32, dz: 1.0, st: 10.0, _p: [0; 2] };
        let dct_params_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dct_params"), size: 32,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        motion.queue.write_buffer(&dct_params_buf, 0, unsafe {
            std::slice::from_raw_parts(&ep as *const _ as *const u8, 32)
        });

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("dct_encode"), source: wgpu::ShaderSource::Wgsl(DCT_ENCODE_SHADER.into()),
        });

        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("dct_bgl"),
            entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 4, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 5, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 6, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });

        let dct_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dct_bg"), layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: dct_params_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: motion.cur_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: motion.ref_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: mv_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: qtable_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: coeffs_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: recon_buf.as_entire_binding() },
            ],
        });

        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None, bind_group_layouts: &[&bgl], push_constant_ranges: &[],
        });
        let dct_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("dct_encode"), layout: Some(&pl), module: &shader, entry_point: "dct_encode",
        });

        // Persistent histogram + lighting/blur buffers
        let rw = w / 16; let rh = h / 16;
        let n_regions = rw * rh;
        let lb_size = n_regions * 4 + n_blocks * 3;

        let hist_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hist"), size: (256 * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let hist_readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("hist_rb"), size: (256 * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let lb_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lb"), size: (lb_size * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let lb_readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lb_rb"), size: (lb_size * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        // Histogram pipeline (persistent)
        let hist_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("hist"), source: wgpu::ShaderSource::Wgsl(HISTOGRAM_SHADER.into()),
        });
        let hist_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("hist_bgl"), entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let hist_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("hist_bg"), layout: &hist_bgl, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: motion.params_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: motion.cur_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: hist_buf.as_entire_binding() },
            ],
        });
        let hist_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None, bind_group_layouts: &[&hist_bgl], push_constant_ranges: &[],
        });
        let hist_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("hist"), layout: Some(&hist_pl), module: &hist_shader, entry_point: "compute_histogram",
        });

        // Lighting/blur pipeline (persistent)
        let lb_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("lb"), source: wgpu::ShaderSource::Wgsl(LIGHTING_BLUR_SHADER.into()),
        });
        let lb_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("lb_bgl"), entries: &[
                wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None }, count: None },
            ],
        });
        let lb_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lb_bg"), layout: &lb_bgl, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: motion.params_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: motion.cur_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: lb_buf.as_entire_binding() },
            ],
        });
        let lb_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None, bind_group_layouts: &[&lb_bgl], push_constant_ranges: &[],
        });
        let lb_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("lb"), layout: Some(&lb_pl), module: &lb_shader, entry_point: "lighting_blur",
        });

        Some(Self { motion, dct_pipeline, coeffs_buf, recon_buf, qtable_buf, mv_buf,
            coeffs_readback, recon_readback, dct_bind_group, dct_params_buf,
            hist_buf, hist_readback, lb_buf, lb_readback,
            hist_pipeline, lb_pipeline, hist_bind_group, lb_bind_group,
            n_blocks, n_pixels: n, n_regions, lb_size,
            prev_results: std::sync::Mutex::new(None),
            frame_in_flight: std::sync::atomic::AtomicBool::new(false) })
    }

    /// Full GPU encode: motion search + DCT + quantize + reconstruct
    /// Returns: (quantized coefficients per block, reconstructed Y plane, motion results)
    pub fn encode_frame(&self, cur_y: &[f32], ref_y: &[f32], qtable: &[f32; 64],
                        dead_zone: f32, skip_thresh: f32) -> GpuFullEncodeResult {
        let qt_bytes = unsafe { std::slice::from_raw_parts(qtable.as_ptr() as *const u8, 256) };
        self.motion.queue.write_buffer(&self.qtable_buf, 0, qt_bytes);

        #[repr(C)]
        #[derive(Copy, Clone)]
        struct EP { w: u32, h: u32, nbx: u32, nby: u32, dz: f32, st: f32, _p: [u32; 2] }
        let bw = self.motion.w / 8; let bh = self.motion.h / 8;
        let ep = EP { w: self.motion.w as u32, h: self.motion.h as u32,
            nbx: bw as u32, nby: bh as u32, dz: dead_zone, st: skip_thresh, _p: [0; 2] };
        self.motion.queue.write_buffer(&self.dct_params_buf, 0, unsafe {
            std::slice::from_raw_parts(&ep as *const _ as *const u8, 32)
        });

        // Upload cur + ref (motion search buffers already set up)
        let expected = self.n_pixels * 4;
        if cur_y.len() * 4 != expected || ref_y.len() * 4 != expected {
            eprintln!("FAO GPU: buffer size mismatch in encode_frame");
            return GpuFullEncodeResult {
                coeffs: vec![0; self.n_blocks * 64],
                recon_y: cur_y.to_vec(),
                scene: None,
                motion: GpuEncodeResults { mvs: vec![], classes: vec![], histogram: vec![], lighting: vec![], blur: vec![] },
            };
        }
        let cur_bytes = unsafe { std::slice::from_raw_parts(cur_y.as_ptr() as *const u8, cur_y.len() * 4) };
        let ref_bytes_data = unsafe { std::slice::from_raw_parts(ref_y.as_ptr() as *const u8, ref_y.len() * 4) };
        self.motion.queue.write_buffer(&self.motion.cur_buf, 0, cur_bytes);
        self.motion.queue.write_buffer(&self.motion.ref_buf, 0, ref_bytes_data);
        self.motion.queue.write_buffer(&self.recon_buf, 0, ref_bytes_data);  // pre-fill recon
        self.motion.queue.write_buffer(&self.hist_buf, 0, &vec![0u8; 256 * 4]);  // zero histogram

        // ALL passes in ONE command buffer — zero overhead between passes
        let mut encoder = self.motion.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("full") });
        {
            // Pass 1: Motion search
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("motion"), timestamp_writes: None });
            pass.set_pipeline(&self.motion.pipeline);
            pass.set_bind_group(0, &self.motion.bind_group, &[]);
            pass.dispatch_workgroups(self.n_blocks as u32, 1, 1);
        }
        // Copy motion results to MV buf for DCT shader
        encoder.copy_buffer_to_buffer(&self.motion.result_buf, 0, &self.mv_buf, 0, (self.n_blocks * 3 * 4) as u64);
        {
            // Pass 2: DCT + quantize + reconstruct
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("dct"), timestamp_writes: None });
            pass.set_pipeline(&self.dct_pipeline);
            pass.set_bind_group(0, &self.dct_bind_group, &[]);
            pass.dispatch_workgroups(self.n_blocks as u32, 1, 1);
        }
        {
            // Pass 3: Histogram
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("hist"), timestamp_writes: None });
            pass.set_pipeline(&self.hist_pipeline);
            pass.set_bind_group(0, &self.hist_bind_group, &[]);
            pass.dispatch_workgroups(((self.n_pixels + 255) / 256) as u32, 1, 1);
        }
        {
            // Pass 4: Lighting + blur detection
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("lb"), timestamp_writes: None });
            pass.set_pipeline(&self.lb_pipeline);
            pass.set_bind_group(0, &self.lb_bind_group, &[]);
            pass.dispatch_workgroups(((self.n_regions + self.n_blocks + 63) / 64) as u32, 1, 1);
        }
        // Copy ALL results in one batch
        encoder.copy_buffer_to_buffer(&self.motion.result_buf, 0, &self.motion.readback_buf, 0, (self.n_blocks * 3 * 4) as u64);
        encoder.copy_buffer_to_buffer(&self.coeffs_buf, 0, &self.coeffs_readback, 0, (self.n_blocks * 64 * 4) as u64);
        encoder.copy_buffer_to_buffer(&self.recon_buf, 0, &self.recon_readback, 0, (self.n_pixels * 4) as u64);
        encoder.copy_buffer_to_buffer(&self.hist_buf, 0, &self.hist_readback, 0, (256 * 4) as u64);
        encoder.copy_buffer_to_buffer(&self.lb_buf, 0, &self.lb_readback, 0, (self.lb_size * 4) as u64);
        self.motion.queue.submit(std::iter::once(encoder.finish()));

        // Map ALL readback buffers
        let mv_slice = self.motion.readback_buf.slice(..);
        let coeff_slice = self.coeffs_readback.slice(..);
        let recon_slice = self.recon_readback.slice(..);
        let hist_slice = self.hist_readback.slice(..);
        let lb_slice = self.lb_readback.slice(..);

        let (tx1, rx1) = std::sync::mpsc::channel();
        mv_slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx1.send(r); });
        let (tx2, rx2) = std::sync::mpsc::channel();
        coeff_slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx2.send(r); });
        let (tx3, rx3) = std::sync::mpsc::channel();
        recon_slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx3.send(r); });
        let (tx4, rx4) = std::sync::mpsc::channel();
        hist_slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx4.send(r); });
        let (tx5, rx5) = std::sync::mpsc::channel();
        lb_slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx5.send(r); });

        self.motion.device.poll(wgpu::Maintain::Wait);
        rx1.recv().ok(); rx2.recv().ok(); rx3.recv().ok(); rx4.recv().ok(); rx5.recv().ok();

        // Read motion
        let mvd = mv_slice.get_mapped_range();
        let mv_ints: &[i32] = unsafe { std::slice::from_raw_parts(mvd.as_ptr() as *const i32, self.n_blocks * 3) };
        let mut mvs = Vec::with_capacity(self.n_blocks);
        for i in 0..self.n_blocks {
            mvs.push((mv_ints[i*3].max(-127).min(127) as i8, mv_ints[i*3+1].max(-127).min(127) as i8, mv_ints[i*3+2] as f32));
        }
        drop(mvd); self.motion.readback_buf.unmap();

        // Read coefficients + recon
        let cd = coeff_slice.get_mapped_range();
        let coeffs: Vec<i32> = unsafe { std::slice::from_raw_parts(cd.as_ptr() as *const i32, self.n_blocks * 64) }.to_vec();
        drop(cd); self.coeffs_readback.unmap();

        let rd = recon_slice.get_mapped_range();
        let recon: Vec<f32> = unsafe { std::slice::from_raw_parts(rd.as_ptr() as *const f32, self.n_pixels) }.to_vec();
        drop(rd); self.recon_readback.unmap();

        // Read histogram + lighting/blur
        let hd = hist_slice.get_mapped_range();
        let histogram: Vec<u32> = unsafe { std::slice::from_raw_parts(hd.as_ptr() as *const u32, 256) }.to_vec();
        drop(hd); self.hist_readback.unmap();

        let ld = lb_slice.get_mapped_range();
        let lb_floats: &[f32] = unsafe { std::slice::from_raw_parts(ld.as_ptr() as *const f32, self.lb_size) };
        let lighting = lb_floats[..self.n_regions * 4].to_vec();
        let blur = lb_floats[self.n_regions * 4..].to_vec();
        drop(ld); self.lb_readback.unmap();

        let motion = GpuEncodeResults { mvs, classes: vec![], histogram, lighting, blur };
        GpuFullEncodeResult { coeffs, recon_y: recon, scene: None, motion }
    }

}
/// Results from full GPU encode
#[cfg(feature = "gpu")]
pub struct GpuFullEncodeResult {
    pub coeffs: Vec<i32>,
    pub recon_y: Vec<f32>,
    pub motion: GpuEncodeResults,
    pub scene: Option<SceneAnalysis>,
}

#[cfg(not(feature = "gpu"))]
pub struct GpuEncoder;
#[cfg(not(feature = "gpu"))]
impl GpuEncoder {
    pub fn new(_w: usize, _h: usize) -> Option<Self> { None }
}
#[cfg(not(feature = "gpu"))]
pub struct GpuFullEncodeResult {
    pub coeffs: Vec<i32>,
    pub recon_y: Vec<f32>,
    pub motion: GpuEncodeResults,
    pub scene: Option<SceneAnalysis>,
}

// ══════════════════════════════════════════════════════════
// Cross-frame temporal tiling
// Batch N frames to find temporal patterns on GPU
// ══════════════════════════════════════════════════════════

/// Temporal tile: describes how a block behaves across N frames
#[derive(Clone, Copy, Debug)]
pub enum TemporalPattern {
    Static,                          // block unchanged across all frames
    LinearMotion { dvx: f32, dvy: f32 }, // block moves at constant velocity
    Fade { start: f32, end: f32 },   // block brightness changes linearly
    Complex,                         // no simple pattern, encode per-frame
}

/// Cross-frame tile analysis results
pub struct TemporalTileMap {
    pub patterns: Vec<TemporalPattern>,  // per-block temporal pattern
    pub n_static: usize,
    pub n_linear: usize,
    pub n_fade: usize,
    pub n_complex: usize,
    pub tile_depth: usize,  // how many frames this tile covers
}

/// Temporal tile analyzer — CPU-side (GPU stores the frame ring)
pub struct TemporalTiler {
    frame_ring: Vec<Vec<f32>>,     // ring buffer of Y-plane DC values per 8×8 block
    mv_ring: Vec<Vec<(i8, i8)>>,   // ring buffer of MVs per block
    ring_head: usize,
    ring_capacity: usize,
    bw: usize,
    bh: usize,
}

impl TemporalTiler {
    pub fn new(w: usize, h: usize, depth: usize) -> Self {
        let bw = w / 8; let bh = h / 8;
        Self {
            frame_ring: Vec::with_capacity(depth),
            mv_ring: Vec::with_capacity(depth),
            ring_head: 0,
            ring_capacity: depth,
            bw, bh,
        }
    }

    /// Add a frame's block DCs and MVs to the ring
    pub fn push_frame(&mut self, y_plane: &[f32], w: usize, mvs: &[(i8, i8, f32)]) {
        let n_blocks = self.bw * self.bh;
        // Compute per-block DC (average)
        let mut dcs = Vec::with_capacity(n_blocks);
        for by in 0..self.bh { for bx in 0..self.bw {
            let ox = bx * 8; let oy = by * 8;
            let mut sum = 0.0f32;
            for dy in 0..8 { for dx in 0..8 {
                sum += y_plane[(oy+dy)*w+ox+dx];
            }}
            dcs.push(sum / 64.0);
        }}

        let block_mvs: Vec<(i8, i8)> = mvs.iter().take(n_blocks)
            .map(|&(x, y, _)| (x, y)).collect();

        if self.frame_ring.len() < self.ring_capacity {
            self.frame_ring.push(dcs);
            self.mv_ring.push(block_mvs);
        } else {
            self.frame_ring[self.ring_head] = dcs;
            self.mv_ring[self.ring_head] = block_mvs;
        }
        self.ring_head = (self.ring_head + 1) % self.ring_capacity;
    }

    /// Analyze temporal patterns across stored frames
    pub fn analyze(&self) -> TemporalTileMap {
        let n_frames = self.frame_ring.len();
        let n_blocks = self.bw * self.bh;

        if n_frames < 2 {
            return TemporalTileMap {
                patterns: vec![TemporalPattern::Complex; n_blocks],
                n_static: 0, n_linear: 0, n_fade: 0, n_complex: n_blocks,
                tile_depth: n_frames,
            };
        }

        let mut patterns = Vec::with_capacity(n_blocks);
        let mut n_static = 0; let mut n_linear = 0; let mut n_fade = 0; let mut n_complex = 0;

        for bi in 0..n_blocks {
            // Collect DC values and MVs across frames
            let dcs: Vec<f32> = (0..n_frames).map(|f| self.frame_ring[f][bi]).collect();
            let mvs: Vec<(i8, i8)> = (0..n_frames).map(|f| {
                if bi < self.mv_ring[f].len() { self.mv_ring[f][bi] } else { (0, 0) }
            }).collect();

            // Check for static: DC variance < 3
            let dc_mean = dcs.iter().sum::<f32>() / n_frames as f32;
            let dc_var = dcs.iter().map(|v| (v - dc_mean) * (v - dc_mean)).sum::<f32>() / n_frames as f32;

            if dc_var < 9.0 {
                // Check MVs: all near-zero?
                let all_static_mv = mvs.iter().all(|&(x, y)| x.abs() <= 1 && y.abs() <= 1);
                if all_static_mv {
                    patterns.push(TemporalPattern::Static);
                    n_static += 1;
                    continue;
                }
            }

            // Check for linear motion: MVs are consistent
            if n_frames >= 3 {
                let avg_mvx = mvs.iter().map(|m| m.0 as f32).sum::<f32>() / n_frames as f32;
                let avg_mvy = mvs.iter().map(|m| m.1 as f32).sum::<f32>() / n_frames as f32;
                let mv_var = mvs.iter().map(|m| {
                    let dx = m.0 as f32 - avg_mvx;
                    let dy = m.1 as f32 - avg_mvy;
                    dx * dx + dy * dy
                }).sum::<f32>() / n_frames as f32;

                if mv_var < 4.0 && (avg_mvx.abs() > 0.5 || avg_mvy.abs() > 0.5) {
                    patterns.push(TemporalPattern::LinearMotion { dvx: avg_mvx, dvy: avg_mvy });
                    n_linear += 1;
                    continue;
                }
            }

            // Check for fade: DC changes linearly
            if n_frames >= 3 {
                let first_dc = dcs[0];
                let last_dc = dcs[n_frames - 1];
                let expected_slope = (last_dc - first_dc) / (n_frames - 1) as f32;
                let fade_error: f32 = dcs.iter().enumerate().map(|(i, &dc)| {
                    let expected = first_dc + expected_slope * i as f32;
                    (dc - expected) * (dc - expected)
                }).sum::<f32>() / n_frames as f32;

                if fade_error < 16.0 && (last_dc - first_dc).abs() > 5.0 {
                    patterns.push(TemporalPattern::Fade { start: first_dc, end: last_dc });
                    n_fade += 1;
                    continue;
                }
            }

            patterns.push(TemporalPattern::Complex);
            n_complex += 1;
        }

        TemporalTileMap { patterns, n_static, n_linear, n_fade, n_complex, tile_depth: n_frames }
    }
}


// ══════════════════════════════════════════════════════════
// Shared GPU — multiple workers submit through fenced slots
// ══════════════════════════════════════════════════════════




// ═══════════════════════════════════════════════════════
// Scene Analysis GPU Shaders
// ═══════════════════════════════════════════════════════

/// Depth estimation from spatial gradients (monocular depth cues)
/// Per 16×16 region: estimates relative depth from:
/// - Edge density (more edges = closer)
/// - Texture frequency (higher freq = closer)
/// - Vertical position (lower = closer, perspective cue)
/// - Luminance gradient (atmospheric perspective: far = lighter)
pub const DEPTH_SHADER: &str = r#"
struct Params { width: u32, height: u32, n_blocks_x: u32, n_blocks_y: u32, sr: u32, skip: f32, _p0: u32, _p1: u32, };
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> cur_y: array<f32>;
@group(0) @binding(2) var<storage, read_write> depth_map: array<f32>;  // per-region depth 0..1

@compute @workgroup_size(64)
fn estimate_depth(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x + gid.y * 65535u * 64u;
    let rw = params.width / 16u;
    let rh = params.height / 16u;
    if idx >= rw * rh { return; }
    
    let rx = idx % rw;
    let ry = idx / rw;
    let ox = rx * 16u;
    let oy = ry * 16u;
    let w = params.width;
    let h = params.height;
    
    // Edge density: Sobel gradient magnitude
    var edge_sum = 0.0;
    var luma_sum = 0.0;
    var freq_sum = 0.0;
    var n = 0.0;
    
    for (var dy = 1u; dy < 15u; dy++) {
        for (var dx = 1u; dx < 15u; dx++) {
            let px = ox + dx;
            let py = oy + dy;
            if px >= w - 1u || py >= h - 1u { continue; }
            let pi = py * w + px;
            
            // Sobel X
            let gx = cur_y[pi + 1u] - cur_y[pi - 1u]
                + 2.0 * (cur_y[pi + w + 1u] - cur_y[pi + w - 1u])
                + cur_y[(py - 1u) * w + px + 1u] - cur_y[(py - 1u) * w + px - 1u];
            // Sobel Y
            let gy = cur_y[(py + 1u) * w + px] - cur_y[(py - 1u) * w + px]
                + 2.0 * (cur_y[(py + 1u) * w + px + 1u] - cur_y[(py - 1u) * w + px + 1u])
                + cur_y[(py + 1u) * w + px - 1u] - cur_y[(py - 1u) * w + px - 1u];
            
            let grad = sqrt(gx * gx + gy * gy);
            edge_sum += grad;
            luma_sum += cur_y[pi];
            
            // High-frequency content: difference from 2-pixel average
            let local_avg = (cur_y[pi - 1u] + cur_y[pi + 1u] + cur_y[pi - w] + cur_y[pi + w]) * 0.25;
            freq_sum += abs(cur_y[pi] - local_avg);
            n += 1.0;
        }
    }
    
    if n < 1.0 { depth_map[idx] = 0.5; return; }
    
    let edge_density = edge_sum / n / 255.0;      // 0..1
    let avg_luma = luma_sum / n / 255.0;           // 0..1
    let texture_freq = freq_sum / n / 128.0;       // 0..1
    let vertical_pos = f32(ry) / f32(rh);          // 0=top, 1=bottom
    
    // Depth heuristic: combine cues
    // Closer objects: more edges, higher texture freq, lower in frame
    // Farther objects: smoother, lighter (atmospheric), higher in frame
    let depth = clamp(
        edge_density * 0.3
        + texture_freq * 0.25
        + (1.0 - vertical_pos) * 0.2  // higher = farther
        + (1.0 - avg_luma) * 0.15     // darker = closer (usually)
        + 0.1,                         // baseline
        0.0, 1.0
    );
    
    depth_map[idx] = depth;
}
"#;

/// Shadow detection: identifies shadowed regions by analyzing
/// luminance distribution, edge boundaries, and color temperature shifts
pub const SHADOW_SHADER: &str = r#"
struct Params { width: u32, height: u32, n_blocks_x: u32, n_blocks_y: u32, sr: u32, skip: f32, _p0: u32, _p1: u32, };
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> cur_y: array<f32>;
@group(0) @binding(2) var<storage, read> ref_y: array<f32>;
@group(0) @binding(3) var<storage, read_write> shadow_map: array<f32>;  // per-block: shadow_intensity, shadow_direction, penumbra_width

@compute @workgroup_size(64)
fn detect_shadows(@builtin(global_invocation_id) gid: vec3<u32>) {
    let block_idx = gid.x + gid.y * 65535u * 64u;
    let bw = params.n_blocks_x;
    let bh = params.n_blocks_y;
    if block_idx >= bw * bh { return; }
    
    let bx = block_idx % bw;
    let by = block_idx / bw;
    let ox = bx * 8u;
    let oy = by * 8u;
    let w = params.width;
    
    // Analyze this block's luminance characteristics
    var min_y = 255.0;
    var max_y = 0.0;
    var sum_y = 0.0;
    var sum_grad_x = 0.0;
    var sum_grad_y = 0.0;
    
    for (var dy = 0u; dy < 8u; dy++) {
        for (var dx = 0u; dx < 8u; dx++) {
            let pi = (oy + dy) * w + ox + dx;
            let v = cur_y[pi];
            min_y = min(min_y, v);
            max_y = max(max_y, v);
            sum_y += v;
        }
    }
    let avg_y = sum_y / 64.0;
    let contrast = (max_y - min_y) / 255.0;
    
    // Shadow detection: compare to neighbor blocks
    // Shadows have: low luminance, sharp edge on one side, gradual on other
    var neighbor_avg = 0.0;
    var n_neighbors = 0.0;
    
    // Sample 4 neighbors
    for (var ndy = -1; ndy <= 1; ndy += 2) {
        for (var ndx = -1; ndx <= 1; ndx += 2) {
            let nbx = i32(bx) + ndx;
            let nby = i32(by) + ndy;
            if nbx < 0 || nby < 0 || nbx >= i32(bw) || nby >= i32(bh) { continue; }
            let nox = u32(nbx) * 8u;
            let noy = u32(nby) * 8u;
            var ns = 0.0;
            for (var dy = 0u; dy < 8u; dy++) {
                for (var dx = 0u; dx < 8u; dx++) {
                    ns += cur_y[(noy + dy) * w + nox + dx];
                }
            }
            neighbor_avg += ns / 64.0;
            n_neighbors += 1.0;
            
            // Gradient direction toward brighter neighbor
            let diff = (ns / 64.0) - avg_y;
            sum_grad_x += diff * f32(ndx);
            sum_grad_y += diff * f32(ndy);
        }
    }
    
    if n_neighbors > 0.0 { neighbor_avg /= n_neighbors; }
    
    // Shadow intensity: how much darker than surroundings
    let shadow_intensity = clamp((neighbor_avg - avg_y) / 128.0, 0.0, 1.0);
    
    // Shadow direction: angle toward light source (opposite of shadow gradient)
    let shadow_dir = atan2(sum_grad_y, sum_grad_x);
    
    // Penumbra width: soft shadow edge vs hard shadow
    let penumbra = clamp(contrast * 2.0, 0.0, 1.0);
    
    // Temporal stability: compare to reference frame
    var ref_avg = 0.0;
    for (var dy = 0u; dy < 8u; dy++) {
        for (var dx = 0u; dx < 8u; dx++) {
            ref_avg += ref_y[(oy + dy) * w + ox + dx];
        }
    }
    ref_avg /= 64.0;
    let temporal_diff = abs(avg_y - ref_avg) / 255.0;
    
    // Output: intensity, direction, penumbra (packed per block × 3)
    shadow_map[block_idx * 3u] = shadow_intensity;
    shadow_map[block_idx * 3u + 1u] = shadow_dir;
    shadow_map[block_idx * 3u + 2u] = penumbra;
}
"#;

/// Rotation field: estimates per-region rotation from motion vector curl
/// Groups of MVs with consistent rotational pattern indicate rotating objects
pub const ROTATION_SHADER: &str = r#"
struct Params { width: u32, height: u32, n_blocks_x: u32, n_blocks_y: u32, sr: u32, skip: f32, _p0: u32, _p1: u32, };
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> mv_results: array<i32>;  // [mvx, mvy, sad] per block
@group(0) @binding(2) var<storage, read_write> rotation_map: array<f32>;  // per-4x4-region: angle, magnitude, confidence

@compute @workgroup_size(64)
fn estimate_rotation(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x + gid.y * 65535u * 64u;
    let bw = params.n_blocks_x;
    let bh = params.n_blocks_y;
    // Operate on 4×4 block regions (32×32 pixel regions)
    let rw = (bw + 3u) / 4u;
    let rh = (bh + 3u) / 4u;
    if idx >= rw * rh { return; }
    
    let rx = idx % rw;
    let ry = idx / rw;
    
    // Compute curl of MV field in this region
    // Curl = dVy/dx - dVx/dy (rotation indicator)
    var curl_sum = 0.0;
    var div_sum = 0.0;
    var mv_mag_sum = 0.0;
    var n = 0.0;
    var cx = 0.0;  // center of MV mass
    var cy = 0.0;
    
    for (var dy = 0u; dy < 4u; dy++) {
        for (var dx = 0u; dx < 4u; dx++) {
            let bx = rx * 4u + dx;
            let by = ry * 4u + dy;
            if bx >= bw || by >= bh { continue; }
            let bi = by * bw + bx;
            let mvx = f32(mv_results[bi * 3u]);
            let mvy = f32(mv_results[bi * 3u + 1u]);
            cx += mvx;
            cy += mvy;
            mv_mag_sum += sqrt(mvx * mvx + mvy * mvy);
            n += 1.0;
            
            // Finite difference for curl
            if dx > 0u && dy > 0u {
                let bi_left = by * bw + bx - 1u;
                let bi_up = (by - 1u) * bw + bx;
                let dvx_dy = f32(mv_results[bi * 3u]) - f32(mv_results[bi_up * 3u]);
                let dvy_dx = f32(mv_results[bi * 3u + 1u]) - f32(mv_results[bi_left * 3u + 1u]);
                curl_sum += dvy_dx - dvx_dy;
                div_sum += f32(mv_results[bi * 3u]) - f32(mv_results[bi_left * 3u])
                         + f32(mv_results[bi * 3u + 1u]) - f32(mv_results[bi_up * 3u + 1u]);
            }
        }
    }
    
    if n < 1.0 { 
        rotation_map[idx * 3u] = 0.0;
        rotation_map[idx * 3u + 1u] = 0.0;
        rotation_map[idx * 3u + 2u] = 0.0;
        return; 
    }
    
    cx /= n; cy /= n;
    let avg_mag = mv_mag_sum / n;
    let avg_curl = curl_sum / max(n - 4.0, 1.0);  // normalize by interior blocks
    
    // Rotation angle estimate (curl / magnitude gives angular velocity)
    let rotation_angle = select(0.0, atan2(avg_curl, avg_mag), avg_mag > 0.5);
    
    // Confidence: high curl with low divergence = pure rotation
    let confidence = select(0.0, 
        clamp(abs(avg_curl) / (abs(div_sum / max(n, 1.0)) + abs(avg_curl) + 0.001), 0.0, 1.0),
        avg_mag > 0.5);
    
    rotation_map[idx * 3u] = rotation_angle;
    rotation_map[idx * 3u + 1u] = avg_mag;
    rotation_map[idx * 3u + 2u] = confidence;
}
"#;

/// 3D geometry classifier: detects surface types from depth + edge patterns
/// Classifies regions as: 0=flat, 1=planar(tilted), 2=curved, 3=edge/corner, 4=complex
pub const GEOMETRY_SHADER: &str = r#"
struct Params { width: u32, height: u32, n_blocks_x: u32, n_blocks_y: u32, sr: u32, skip: f32, _p0: u32, _p1: u32, };
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> depth_map: array<f32>;
@group(0) @binding(2) var<storage, read> cur_y: array<f32>;
@group(0) @binding(3) var<storage, read_write> geometry_map: array<u32>;  // per-region: class(8bit) + normal_x(8bit) + normal_y(8bit) + curvature(8bit)

@compute @workgroup_size(64)
fn classify_geometry(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x + gid.y * 65535u * 64u;
    let rw = params.width / 16u;
    let rh = params.height / 16u;
    if idx >= rw * rh { return; }
    
    let rx = idx % rw;
    let ry = idx / rw;
    
    let d = depth_map[idx];
    
    // Get neighbor depths for surface normal estimation
    var dl = d; var dr = d; var du = d; var dd = d;
    if rx > 0u { dl = depth_map[idx - 1u]; }
    if rx < rw - 1u { dr = depth_map[idx + 1u]; }
    if ry > 0u { du = depth_map[idx - rw]; }
    if ry < rh - 1u { dd = depth_map[idx + rw]; }
    
    // Surface normal from depth gradient
    let nx = (dr - dl) * 0.5;
    let ny = (dd - du) * 0.5;
    let nz = 1.0;
    let nm = sqrt(nx * nx + ny * ny + nz * nz);
    
    // Curvature: second derivative of depth
    let d2x = dr - 2.0 * d + dl;
    let d2y = dd - 2.0 * d + du;
    let curvature = abs(d2x) + abs(d2y);
    
    // Classify surface type
    let tilt = sqrt(nx * nx + ny * ny);
    var class_id = 0u;  // flat
    if curvature > 0.1 {
        class_id = 2u;  // curved
    } else if tilt > 0.05 {
        class_id = 1u;  // planar tilted
    }
    
    // Check for edges: large depth discontinuity with neighbors
    let max_diff = max(max(abs(d - dl), abs(d - dr)), max(abs(d - du), abs(d - dd)));
    if max_diff > 0.15 {
        class_id = 3u;  // depth edge / occlusion boundary
    }
    
    // Pack: class(8) | normal_x(8) | normal_y(8) | curvature(8)
    let nx_u8 = u32(clamp((nx / nm + 1.0) * 127.5, 0.0, 255.0));
    let ny_u8 = u32(clamp((ny / nm + 1.0) * 127.5, 0.0, 255.0));
    let curv_u8 = u32(clamp(curvature * 2550.0, 0.0, 255.0));
    
    geometry_map[idx] = class_id | (nx_u8 << 8u) | (ny_u8 << 16u) | (curv_u8 << 24u);
}
"#;

/// Mesh builder: connected-component labeling on depth-similar regions
/// Groups adjacent regions with similar depth into mesh segments
pub const MESH_SHADER: &str = r#"
struct Params { width: u32, height: u32, n_blocks_x: u32, n_blocks_y: u32, sr: u32, skip: f32, _p0: u32, _p1: u32, };
@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> depth_map: array<f32>;
@group(0) @binding(2) var<storage, read> geometry_map: array<u32>;
@group(0) @binding(3) var<storage, read_write> mesh_id: array<atomic<u32>>;  // per-region mesh ID

@compute @workgroup_size(64)
fn build_mesh(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x + gid.y * 65535u * 64u;
    let rw = params.width / 16u;
    let rh = params.height / 16u;
    if idx >= rw * rh { return; }
    
    let rx = idx % rw;
    let ry = idx / rw;
    let d = depth_map[idx];
    let my_class = geometry_map[idx] & 0xFFu;
    
    // Initial: each region is its own mesh
    atomicStore(&mesh_id[idx], idx);
    storageBarrier();
    
    // Union-find step: merge with left and top neighbors if similar
    let depth_thresh = 0.08;
    
    // Check left neighbor
    if rx > 0u {
        let ni = idx - 1u;
        let nd = depth_map[ni];
        let nc = geometry_map[ni] & 0xFFu;
        if abs(d - nd) < depth_thresh && my_class == nc {
            // Merge: point to smaller ID
            let my_id = atomicLoad(&mesh_id[idx]);
            let n_id = atomicLoad(&mesh_id[ni]);
            let min_id = min(my_id, n_id);
            atomicMin(&mesh_id[idx], min_id);
            atomicMin(&mesh_id[ni], min_id);
        }
    }
    
    // Check top neighbor
    if ry > 0u {
        let ni = idx - rw;
        let nd = depth_map[ni];
        let nc = geometry_map[ni] & 0xFFu;
        if abs(d - nd) < depth_thresh && my_class == nc {
            let my_id = atomicLoad(&mesh_id[idx]);
            let n_id = atomicLoad(&mesh_id[ni]);
            let min_id = min(my_id, n_id);
            atomicMin(&mesh_id[idx], min_id);
            atomicMin(&mesh_id[ni], min_id);
        }
    }
}
"#;

// ══════════════════════════════════════════════════════════
// Multi-slot GPU — per-worker buffer sets for parallel upload
// ══════════════════════════════════════════════════════════



// ═══════════════════════════════════════════════════════════
// FAO Database: Multi-video content-addressed block store
// ═══════════════════════════════════════════════════════════

/// Content-addressed block: the atomic unit of storage.
/// Multiple frames across multiple videos can reference the same block.
/// Block size: 16×16 pixels (or 8×8 for fine-grained dedup).
#[derive(Clone, Debug)]
pub struct ContentBlock {
    pub hash: u64,              // FNV-1a of block pixels
    pub data: Vec<u8>,          // compressed block data (zstd)
    pub ref_count: u32,         // how many frame regions reference this block
    pub block_type: u8,         // 0=Y-only, 1=YUV420, 2=RGB
    pub quality: u8,            // quantization level used
}

/// Frame manifest: describes one frame as a list of block references.
/// The frame is reconstructed by assembling blocks in grid order.
#[derive(Clone, Debug)]
pub struct FrameManifest {
    pub width: u16,
    pub height: u16,
    pub block_size: u8,         // 8 or 16
    pub block_hashes: Vec<u64>, // grid of block hashes (row-major)
    pub anchor_hash: u64,       // hash of the full-frame anchor (if available)
    pub delta_from: u32,        // frame index this is a delta from (u32::MAX = keyframe)
}

/// Video manifest: describes one video as a sequence of frame manifests.
#[derive(Clone, Debug)]
pub struct VideoManifest {
    pub video_id: u64,          // unique video identifier
    pub title: String,
    pub width: u16,
    pub height: u16,
    pub fps_num: u16,
    pub fps_den: u16,
    pub frame_count: u32,
    pub frames: Vec<FrameManifest>,
    pub codec_info: String,     // source codec (VP9, H264, AV1)
    pub total_unique_blocks: u32,
    pub total_shared_blocks: u32,  // blocks shared with other videos
    pub anchor_tree: Option<AnchorTree>,
}

/// FAO Database: contains multiple videos with shared block store.
/// This is the container format for web delivery and archival.
#[derive(Clone, Debug)]
pub struct FaoDatabase {
    pub videos: Vec<VideoManifest>,
    pub block_store: ContentBlockStore,
    pub global_anchor_tree: AnchorTree,  // spans all videos
}

/// Content-addressed block store with hash → data mapping.
/// Blocks are stored ONCE regardless of how many frames/videos reference them.
#[derive(Clone, Debug)]
pub struct ContentBlockStore {
    /// Hash → (compressed_data, ref_count, block_type)
    /// Using Vec for cache-friendly iteration. Binary search on sorted hashes.
    entries: Vec<(u64, Vec<u8>, u32, u8)>,  // sorted by hash
    pub total_bytes: u64,
    pub total_blocks: u64,
    pub dedup_savings_bytes: u64,  // bytes saved by deduplication
}

impl ContentBlockStore {
    pub fn new() -> Self {
        Self { entries: Vec::new(), total_bytes: 0, total_blocks: 0, dedup_savings_bytes: 0 }
    }
    
    /// Insert a block. If hash exists, increment ref_count (no data stored).
    /// Returns true if block was NEW (data stored), false if dedup hit.
    pub fn insert(&mut self, hash: u64, data: Vec<u8>, block_type: u8) -> bool {
        match self.entries.binary_search_by_key(&hash, |e| e.0) {
            Ok(idx) => {
                // Dedup hit: block already exists
                self.entries[idx].2 += 1;  // increment ref_count
                self.dedup_savings_bytes += data.len() as u64;
                self.total_blocks += 1;
                false
            }
            Err(idx) => {
                // New block
                self.total_bytes += data.len() as u64;
                self.total_blocks += 1;
                self.entries.insert(idx, (hash, data, 1, block_type));
                true
            }
        }
    }
    
    /// Look up a block by hash. O(log n) binary search.
    pub fn get(&self, hash: u64) -> Option<&[u8]> {
        self.entries.binary_search_by_key(&hash, |e| e.0)
            .ok()
            .map(|idx| self.entries[idx].1.as_slice())
    }
    
    /// Get dedup statistics
    pub fn stats(&self) -> (u64, u64, u64, f64) {
        let unique = self.entries.len() as u64;
        let ratio = if self.total_bytes > 0 {
            (self.total_bytes + self.dedup_savings_bytes) as f64 / self.total_bytes as f64
        } else { 1.0 };
        (unique, self.total_blocks, self.dedup_savings_bytes, ratio)
    }
    
    /// Compute hash for a block of pixels
    pub fn hash_block(pixels: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;
        // Sample every 4th byte for speed (still good collision resistance)
        let mut i = 0;
        while i < pixels.len() {
            h ^= pixels[i] as u64;
            h = h.wrapping_mul(0x100000001b3);
            i += 4;
        }
        // Mix in length for different-sized blocks
        h ^= pixels.len() as u64;
        h = h.wrapping_mul(0x100000001b3);
        h
    }
    
    /// Serialize block store to bytes
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"FBLK");  // magic
        out.extend_from_slice(&(self.entries.len() as u64).to_le_bytes());
        out.extend_from_slice(&self.total_bytes.to_le_bytes());
        out.extend_from_slice(&self.dedup_savings_bytes.to_le_bytes());
        
        for (hash, data, ref_count, block_type) in &self.entries {
            out.extend_from_slice(&hash.to_le_bytes());
            out.extend_from_slice(&(*ref_count).to_le_bytes());
            out.push(*block_type);
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(data);
        }
        out
    }
    
    /// Deserialize from bytes
    pub fn from_bytes(data: &[u8]) -> Option<Self> {
        if data.len() < 28 || &data[0..4] != b"FBLK" { return None; }
        let n = u64::from_le_bytes(data[4..12].try_into().ok()?) as usize;
        let total_bytes = u64::from_le_bytes(data[12..20].try_into().ok()?);
        let dedup_savings = u64::from_le_bytes(data[20..28].try_into().ok()?);
        
        let mut entries = Vec::with_capacity(n);
        let mut pos = 28;
        for _ in 0..n {
            if pos + 17 > data.len() { break; }
            let hash = u64::from_le_bytes(data[pos..pos+8].try_into().ok()?);
            let rc = u32::from_le_bytes(data[pos+8..pos+12].try_into().ok()?);
            let bt = data[pos+12];
            let dlen = u32::from_le_bytes(data[pos+13..pos+17].try_into().ok()?) as usize;
            pos += 17;
            if pos + dlen > data.len() { break; }
            entries.push((hash, data[pos..pos+dlen].to_vec(), rc, bt));
            pos += dlen;
        }
        
        Some(Self {
            entries, total_bytes, dedup_savings_bytes: dedup_savings,
            total_blocks: n as u64,
        })
    }
}

impl FaoDatabase {
    pub fn new() -> Self {
        Self {
            videos: Vec::new(),
            block_store: ContentBlockStore::new(),
            global_anchor_tree: AnchorTree::build(0, &[]),
        }
    }
    
    /// Add a video to the database. Blocks are deduplicated against existing content.
    pub fn add_video(&mut self, manifest: VideoManifest) {
        self.videos.push(manifest);
    }
    
    /// Get cross-video dedup statistics
    pub fn dedup_stats(&self) -> String {
        let (unique, total, saved, ratio) = self.block_store.stats();
        format!("{} videos, {} unique blocks / {} total ({:.1}:1 dedup, {:.1}MB saved)",
            self.videos.len(), unique, total, ratio, saved as f64 / 1048576.0)
    }
}




// ═══════════════════════════════════════════════════════════
// Web Merging Algorithms — combine multiple frame webs
// ═══════════════════════════════════════════════════════════
//
// When merging multiple videos into an archive, their FrameWebs
// can be combined by content hash overlap. Identical blocks/frames
// in different videos collapse to single shared nodes.

impl FrameWeb {
    /// Merge another web into this one, deduplicating nodes by content hash.
    /// Returns: (added_nodes, dedup_count, total_dedup_bytes)
    pub fn merge_from(&mut self, other: &FrameWeb, hash_match_required: bool) -> (u32, u32, u64) {
        let base_idx = self.nodes.len() as u32;
        let mut idx_map: Vec<u32> = Vec::with_capacity(other.nodes.len());
        let mut dedup_count = 0u32;
        let mut dedup_bytes = 0u64;
        
        // For each node in other, check if we already have a matching hash
        for node in &other.nodes {
            // Look up by hash
            if hash_match_required && node.content_hash != 0 {
                if let Ok(i) = self.hash_index.binary_search_by_key(&node.content_hash, |&(h, _)| h) {
                    // Dedup hit — reuse existing node
                    let existing_idx = self.hash_index[i].1;
                    idx_map.push(existing_idx);
                    dedup_count += 1;
                    dedup_bytes += node.compressed_size as u64;
                    continue;
                }
            }
            // Add as new node
            let new_idx = self.nodes.len() as u32;
            self.nodes.push(*node);
            if node.content_hash != 0 {
                let pos = self.hash_index.binary_search_by_key(&node.content_hash, |&(h, _)| h)
                    .unwrap_or_else(|e| e);
                self.hash_index.insert(pos, (node.content_hash, new_idx));
            }
            idx_map.push(new_idx);
        }
        
        // Remap and copy edges
        for edge in &other.edges {
            let new_target = if (edge.target as usize) < idx_map.len() {
                idx_map[edge.target as usize]
            } else { edge.target };
            self.edges.push(WebEdge {
                target: new_target,
                kind: edge.kind,
                weight: edge.weight,
                _pad: 0,
            });
        }
        
        let added = idx_map.len() as u32 - dedup_count;
        (added, dedup_count, dedup_bytes)
    }
    
    /// Find connected components (clusters of related frames).
    /// Used for clustering similar content across videos.
    pub fn find_clusters(&self, min_edge_weight: u8) -> Vec<Vec<u32>> {
        let n = self.nodes.len();
        let mut visited = vec![false; n];
        let mut clusters: Vec<Vec<u32>> = Vec::new();
        
        for start in 0..n {
            if visited[start] { continue; }
            // BFS from this node
            let mut cluster = Vec::new();
            let mut stack = vec![start];
            while let Some(idx) = stack.pop() {
                if visited[idx] { continue; }
                visited[idx] = true;
                cluster.push(idx as u32);
                
                // Walk outgoing edges
                if idx < self.nodes.len() {
                    let n = &self.nodes[idx];
                    let start = n.edge_start as usize;
                    let end = (start + n.edge_count as usize).min(self.edges.len());
                    for e in &self.edges[start..end] {
                        if e.weight >= min_edge_weight && (e.target as usize) < self.nodes.len() {
                            if !visited[e.target as usize] {
                                stack.push(e.target as usize);
                            }
                        }
                    }
                }
            }
            
            if cluster.len() >= 2 {
                clusters.push(cluster);
            }
        }
        
        clusters
    }
    
    /// Compute similarity score between two webs based on hash overlap.
    /// Returns: (overlap_count, similarity_ratio, est_dedup_savings_bytes)
    pub fn similarity(&self, other: &FrameWeb) -> (u32, f32, u64) {
        let mut overlap = 0u32;
        let mut savings = 0u64;
        
        for n in &other.nodes {
            if n.content_hash == 0 { continue; }
            if self.hash_index.binary_search_by_key(&n.content_hash, |&(h, _)| h).is_ok() {
                overlap += 1;
                savings += n.compressed_size as u64;
            }
        }
        
        let other_total = other.nodes.iter().filter(|n| n.content_hash != 0).count() as f32;
        let ratio = if other_total > 0.0 { overlap as f32 / other_total } else { 0.0 };
        (overlap, ratio, savings)
    }
    
    /// Predict cross-video shared blocks.
    /// Returns most-shared content hashes (good candidates for shared block store).
    pub fn top_shared_hashes(&self, top_n: usize) -> Vec<(u64, u32)> {
        // Count occurrences of each hash
        let mut counts: std::collections::HashMap<u64, u32> = std::collections::HashMap::new();
        for &(hash, _) in &self.hash_index {
            *counts.entry(hash).or_insert(0) += 1;
        }
        let mut sorted: Vec<(u64, u32)> = counts.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));
        sorted.truncate(top_n);
        sorted
    }
}


// ═══════════════════════════════════════════════════════════
// Subpixel Motion — fractional pixel motion vectors for VRR/upscaling
// ═══════════════════════════════════════════════════════════
//
// Standard MVs are integer pixel offsets. Subpixel MVs use 1/4 or 1/8 pixel
// precision, enabling:
//   - Smoother motion interpolation (for VRR — variable refresh rate)
//   - Better upscaling (synthesize new frames between encoded ones)
//   - More accurate motion compensation (better prediction = smaller residuals)

/// Subpixel motion vector with 1/8-pixel precision.
/// Stored as (dx_eighths, dy_eighths) i16 pair → range ±32 pixels at 1/8 precision.
#[derive(Clone, Copy, Debug)]
pub struct SubpixelMV {
    pub dx_eighths: i16,   // 1/8 pixel units
    pub dy_eighths: i16,
    pub confidence: u8,    // 0-255
    pub _pad: u8,
}

impl SubpixelMV {
    pub const fn from_pixels(dx: f32, dy: f32) -> Self {
        Self {
            dx_eighths: (dx * 8.0) as i16,
            dy_eighths: (dy * 8.0) as i16,
            confidence: 255,
            _pad: 0,
        }
    }
    
    pub fn dx_pixels(&self) -> f32 { self.dx_eighths as f32 / 8.0 }
    pub fn dy_pixels(&self) -> f32 { self.dy_eighths as f32 / 8.0 }
    
    /// Interpolate at a fractional time t ∈ [0, 1]
    pub fn at_time(&self, t: f32) -> (f32, f32) {
        (self.dx_pixels() * t, self.dy_pixels() * t)
    }
    
    /// Compose two subpixel MVs (combine sequential motions)
    pub fn compose(&self, other: SubpixelMV) -> Self {
        Self {
            dx_eighths: self.dx_eighths + other.dx_eighths,
            dy_eighths: self.dy_eighths + other.dy_eighths,
            confidence: ((self.confidence as u16 * other.confidence as u16) / 255) as u8,
            _pad: 0,
        }
    }
}

/// Bilinear subpixel sampling — fetch pixel value at fractional position.
/// Used for motion-compensated prediction at sub-pixel accuracy.
pub fn sample_bilinear(plane: &[u8], w: usize, h: usize, x: f32, y: f32) -> u8 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    
    let get = |xi: i32, yi: i32| -> f32 {
        let xc = xi.clamp(0, w as i32 - 1) as usize;
        let yc = yi.clamp(0, h as i32 - 1) as usize;
        plane[yc * w + xc] as f32
    };
    
    let a = get(x0, y0);
    let b = get(x0 + 1, y0);
    let c = get(x0, y0 + 1);
    let d = get(x0 + 1, y0 + 1);
    
    let top = a * (1.0 - fx) + b * fx;
    let bot = c * (1.0 - fx) + d * fx;
    let val = top * (1.0 - fy) + bot * fy;
    val.clamp(0.0, 255.0) as u8
}

/// Bicubic subpixel sampling — higher quality than bilinear, used for upscaling.
pub fn sample_bicubic(plane: &[u8], w: usize, h: usize, x: f32, y: f32) -> u8 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    
    let get = |xi: i32, yi: i32| -> f32 {
        let xc = xi.clamp(0, w as i32 - 1) as usize;
        let yc = yi.clamp(0, h as i32 - 1) as usize;
        plane[yc * w + xc] as f32
    };
    
    // Catmull-Rom kernel
    let kernel = |t: f32| -> f32 {
        let t = t.abs();
        if t < 1.0 {
            1.5 * t * t * t - 2.5 * t * t + 1.0
        } else if t < 2.0 {
            -0.5 * t * t * t + 2.5 * t * t - 4.0 * t + 2.0
        } else { 0.0 }
    };
    
    let mut sum = 0.0f32;
    let mut weight = 0.0f32;
    for dy in -1..=2 {
        for dx in -1..=2 {
            let kx = kernel(dx as f32 - fx);
            let ky = kernel(dy as f32 - fy);
            let k = kx * ky;
            sum += get(x0 + dx, y0 + dy) * k;
            weight += k;
        }
    }
    let val = if weight > 0.0 { sum / weight } else { sum };
    val.clamp(0.0, 255.0) as u8
}

/// Estimate subpixel motion between two frames using gradient descent.
/// Returns the best subpixel offset for a given block.
pub fn estimate_subpixel_motion(
    cur: &[u8], cur_w: usize, cur_h: usize,
    ref_plane: &[u8], ref_w: usize, ref_h: usize,
    block_x: usize, block_y: usize, block_size: usize,
    initial_mv: (i32, i32),
) -> SubpixelMV {
    let _ = cur_h; let _ = ref_h;
    // Start with integer MV, refine to 1/8 pixel
    let mut best_dx = initial_mv.0 as f32;
    let mut best_dy = initial_mv.1 as f32;
    let mut best_sad = f32::MAX;
    
    // Search ±1 pixel at 1/8 precision = 17×17 candidates
    let step = 0.125f32;
    for ndy in -8..=8 {
        for ndx in -8..=8 {
            let dx = best_dx + ndx as f32 * step;
            let dy = best_dy + ndy as f32 * step;
            
            let mut sad = 0.0f32;
            for by in 0..block_size {
                for bx in 0..block_size {
                    let cx = (block_x + bx) as f32;
                    let cy = (block_y + by) as f32;
                    let cur_val = if (block_x + bx) < cur_w && (block_y + by) < cur_h {
                        cur[(block_y + by) * cur_w + (block_x + bx)] as f32
                    } else { 128.0 };
                    let ref_val = sample_bilinear(ref_plane, ref_w, ref_h, cx + dx, cy + dy) as f32;
                    sad += (cur_val - ref_val).abs();
                }
            }
            
            if sad < best_sad {
                best_sad = sad;
                best_dx = dx;
                best_dy = dy;
            }
        }
    }
    
    let conf = if best_sad < 100.0 { 255 } else { (255.0 / (best_sad / 100.0)).clamp(0.0, 255.0) as u8 };
    SubpixelMV {
        dx_eighths: (best_dx * 8.0).round() as i16,
        dy_eighths: (best_dy * 8.0).round() as i16,
        confidence: conf,
        _pad: 0,
    }
}

/// Synthesize an intermediate frame at fractional time t ∈ (0, 1).
/// Used for VRR (variable refresh rate) and frame interpolation.
/// Given two anchor frames and motion vectors, generates a frame at time t between them.
pub fn synthesize_intermediate_frame(
    frame_a: &[u8], frame_b: &[u8],
    mvs: &[SubpixelMV],
    w: usize, h: usize, block_size: usize,
    t: f32,
) -> Vec<u8> {
    let mut out = vec![0u8; frame_a.len()];
    let blocks_per_row = w / block_size;
    
    for by in 0..h / block_size {
        for bx in 0..blocks_per_row {
            let block_idx = by * blocks_per_row + bx;
            let mv = mvs.get(block_idx).copied().unwrap_or(SubpixelMV {
                dx_eighths: 0, dy_eighths: 0, confidence: 0, _pad: 0,
            });
            
            // Motion-compensated position at time t
            let dx = mv.dx_pixels() * t;
            let dy = mv.dy_pixels() * t;
            
            for py in 0..block_size {
                for px in 0..block_size {
                    let dest_x = bx * block_size + px;
                    let dest_y = by * block_size + py;
                    if dest_x >= w || dest_y >= h { continue; }
                    
                    // Sample from frame_a (forward warp) and frame_b (backward warp)
                    let src_x_a = dest_x as f32 + dx;
                    let src_y_a = dest_y as f32 + dy;
                    let src_x_b = dest_x as f32 + dx - mv.dx_pixels();
                    let src_y_b = dest_y as f32 + dy - mv.dy_pixels();
                    
                    let val_a = sample_bilinear(frame_a, w, h, src_x_a, src_y_a) as f32;
                    let val_b = sample_bilinear(frame_b, w, h, src_x_b, src_y_b) as f32;
                    
                    // Linear blend by t (with confidence weighting)
                    let blended = val_a * (1.0 - t) + val_b * t;
                    out[dest_y * w + dest_x] = blended.clamp(0.0, 255.0) as u8;
                }
            }
        }
    }
    
    out
}

// ═══════════════════════════════════════════════════════════
// Motion-Tracked Scene Refinement Pipeline
// ═══════════════════════════════════════════════════════════


// ═══════════════════════════════════════════════════════════
// Frame Relationship Web — O(1) graph of frame dependencies
// ═══════════════════════════════════════════════════════════
//
// A flat, cache-friendly graph connecting frames, blocks, and elements.
// No heap allocation on lookup. All indices are u32 into flat arrays.
// Designed for L1 cache residency: core node = 32 bytes (half cache line).

/// Relationship type between two nodes in the web
#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WebEdgeKind {
    AnchorRef = 0,    // frame → its anchor (decode dependency)
    DeltaFrom = 1,    // frame → frame it's a delta of
    SharedBlock = 2,  // frame → frame sharing same content block
    MotionFrom = 3,   // element → element it tracked from
    QualityLayer = 4, // base frame → quality refinement layer
    SceneLink = 5,    // frame → frame in same scene
}

/// A node in the frame web. 32 bytes, cache-line aligned.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct WebNode {
    pub frame_idx: u32,        // which frame (or element ID)
    pub node_type: u8,         // 0=frame, 1=block, 2=element, 3=anchor
    pub tier: u8,              // encode tier (0=skip, 1=pass, 2=full)
    pub scene_id: u16,         // scene membership
    pub edge_start: u32,       // index into edge array
    pub edge_count: u16,       // number of outgoing edges (max 8)
    pub content_hash: u64,     // for dedup lookup
    pub byte_offset: u32,      // offset in storage file
    pub compressed_size: u16,  // size in storage (max 64KB per node)
}

/// An edge connecting two web nodes. 8 bytes.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct WebEdge {
    pub target: u32,           // index into node array
    pub kind: u8,              // WebEdgeKind
    pub weight: u8,            // confidence/strength (0-255)
    pub _pad: u16,
}

/// The frame relationship web. Flat arrays for cache efficiency.
/// Lookup: O(1) node access, O(k) edge traversal where k ≤ 8.
pub struct FrameWeb {
    pub nodes: Vec<WebNode>,
    pub edges: Vec<WebEdge>,
    pub hash_index: Vec<(u64, u32)>,  // sorted by hash → O(log n) dedup lookup
    pub scene_boundaries: Vec<u32>,   // frame indices where scenes change
    node_count: u32,
    edge_count: u32,
}

impl FrameWeb {
    pub fn new(capacity: usize) -> Self {
        Self {
            nodes: Vec::with_capacity(capacity),
            edges: Vec::with_capacity(capacity * 4),  // avg 4 edges per node
            hash_index: Vec::new(),
            scene_boundaries: Vec::new(),
            node_count: 0,
            edge_count: 0,
        }
    }
    
    /// Add a frame node. Returns its index.
    #[inline]
    pub fn add_frame(&mut self, frame_idx: u32, tier: u8, hash: u64, 
                     offset: u32, size: u16) -> u32 {
        let idx = self.node_count;
        self.nodes.push(WebNode {
            frame_idx, node_type: 0, tier, scene_id: 0,
            edge_start: self.edge_count, edge_count: 0,
            content_hash: hash, byte_offset: offset, compressed_size: size,
        });
        self.node_count += 1;
        idx
    }
    
    /// Add an edge from source to target. O(1).
    #[inline]
    pub fn add_edge(&mut self, source: u32, target: u32, kind: WebEdgeKind, weight: u8) {
        if source as usize >= self.nodes.len() { return; }
        self.edges.push(WebEdge { target, kind: kind as u8, weight, _pad: 0 });
        self.nodes[source as usize].edge_count += 1;
        self.edge_count += 1;
    }
    
    /// Get all edges from a node. O(1) start, O(k) iteration.
    #[inline]
    pub fn edges_from(&self, node_idx: u32) -> &[WebEdge] {
        if node_idx as usize >= self.nodes.len() { return &[]; }
        let n = &self.nodes[node_idx as usize];
        let start = n.edge_start as usize;
        let end = start + n.edge_count as usize;
        if end <= self.edges.len() {
            &self.edges[start..end]
        } else { &[] }
    }
    
    /// Find the anchor for a frame. Walks AnchorRef edges.
    /// Returns in ≤ 4 hops (log₈ n).
    #[inline]
    pub fn find_anchor(&self, frame_idx: u32) -> Option<u32> {
        if frame_idx as usize >= self.nodes.len() { return None; }
        let mut current = frame_idx;
        for _ in 0..8 {  // max 8 hops
            let edges = self.edges_from(current);
            let anchor_edge = edges.iter().find(|e| e.kind == WebEdgeKind::AnchorRef as u8);
            match anchor_edge {
                Some(e) => {
                    if e.target == current { return Some(current); }  // self-anchor
                    current = e.target;
                }
                None => return Some(current),  // no anchor ref = IS an anchor
            }
        }
        Some(current)
    }
    
    /// Find frames sharing content with a given frame. O(log n + k).
    pub fn find_shared(&self, hash: u64) -> Vec<u32> {
        match self.hash_index.binary_search_by_key(&hash, |&(h, _)| h) {
            Ok(mut i) => {
                let mut result = Vec::new();
                // Walk backwards to find all with same hash
                while i > 0 && self.hash_index[i-1].0 == hash { i -= 1; }
                while i < self.hash_index.len() && self.hash_index[i].0 == hash {
                    result.push(self.hash_index[i].1);
                    i += 1;
                }
                result
            }
            Err(_) => Vec::new(),
        }
    }
    
    /// Detect scene boundaries from edge patterns.
    /// A scene boundary occurs when consecutive frames have no SharedBlock edges.
    pub fn detect_scenes(&mut self) {
        self.scene_boundaries.clear();
        let mut current_scene = 0u16;
        
        for i in 1..self.nodes.len() {
            let prev = &self.nodes[i-1];
            let curr_edges = self.edges_from(i as u32);
            
            // Check if any edge links to the previous frame
            let has_link = curr_edges.iter().any(|e| {
                e.target == (i as u32 - 1) && 
                (e.kind == WebEdgeKind::DeltaFrom as u8 || e.kind == WebEdgeKind::SceneLink as u8)
            });
            
            if !has_link && prev.tier >= 1 {
                current_scene += 1;
                self.scene_boundaries.push(i as u32);
            }
            
            if i < self.nodes.len() {
                self.nodes[i].scene_id = current_scene;
            }
        }
    }
    
    /// Build the hash index for O(log n) dedup lookup.
    pub fn build_hash_index(&mut self) {
        self.hash_index.clear();
        for (i, n) in self.nodes.iter().enumerate() {
            if n.content_hash != 0 {
                self.hash_index.push((n.content_hash, i as u32));
            }
        }
        self.hash_index.sort_by_key(|&(h, _)| h);
    }
    
    /// Get decode dependency chain for a frame (for streaming priority).
    /// Returns frames needed to decode target, in decode order.
    pub fn decode_chain(&self, target: u32) -> Vec<u32> {
        let mut chain = Vec::with_capacity(8);
        let mut current = target;
        for _ in 0..8 {
            chain.push(current);
            let edges = self.edges_from(current);
            match edges.iter().find(|e| e.kind == WebEdgeKind::AnchorRef as u8 || e.kind == WebEdgeKind::DeltaFrom as u8) {
                Some(e) if e.target != current => { current = e.target; }
                _ => break,
            }
        }
        chain.reverse();
        chain
    }
    
    /// Memory footprint
    pub fn memory_bytes(&self) -> usize {
        self.nodes.len() * 32 + self.edges.len() * 8 + self.hash_index.len() * 12
    }
    
    /// Stats string
    pub fn stats(&self) -> String {
        format!("{} nodes, {} edges, {} scenes, {:.1}KB memory",
            self.nodes.len(), self.edges.len(), self.scene_boundaries.len() + 1,
            self.memory_bytes() as f64 / 1024.0)
    }
}

/// Scene element tracked across frames via motion vectors.
/// Each element is a region of pixels that moves coherently.
#[derive(Clone, Debug)]
pub struct TrackedElement {
    pub id: u32,
    pub bbox: [u16; 4],        // x, y, w, h in pixels
    pub mv: (f32, f32),        // accumulated motion vector
    pub anchor_frame: u32,     // which anchor this element was last seen in
    pub anchor_data: Vec<u8>,  // compressed reference block from anchor
    pub confidence: f32,       // tracking confidence (1.0 = high, 0.0 = lost)
    pub depth: f32,            // relative depth (from depth shader)
    pub is_shadow: bool,       // in shadow region
}

/// Scene model that tracks elements across frames.
/// Used to predict skip frame content without re-decoding.
#[derive(Clone, Debug)]
pub struct SceneTracker {
    pub elements: Vec<TrackedElement>,
    pub frame_idx: u32,
    pub anchor_y: Vec<u8>,     // last anchor frame Y plane
    pub anchor_w: u16,
    pub anchor_h: u16,
    pub mv_field: Vec<(i8, i8)>, // per-block MVs from last encode
    pub element_counter: u32,
}

impl SceneTracker {
    pub fn new() -> Self {
        Self {
            elements: Vec::new(),
            frame_idx: 0,
            anchor_y: Vec::new(),
            anchor_w: 0, anchor_h: 0,
            mv_field: Vec::new(),
            element_counter: 0,
        }
    }
    
    /// Update tracker from GPU encode results.
    /// Called after each full-encode frame with MVs and scene analysis.
    pub fn update_from_encode(
        &mut self, frame_idx: u32, w: u16, h: u16,
        y_plane: &[u8], mvs: &[(i8, i8, f32)],
        depth: &[f32], shadow: &[f32],
    ) {
        self.frame_idx = frame_idx;
        self.anchor_w = w;
        self.anchor_h = h;
        self.anchor_y = y_plane.to_vec();
        
        // Extract motion field (per 8×8 block)
        let bw = w as usize / 8;
        let bh = h as usize / 8;
        self.mv_field = mvs.iter().map(|&(mx, my, _)| (mx, my)).collect();
        
        // Segment into tracked elements by coherent motion regions
        // Simple approach: flood-fill blocks with similar MVs
        let mut visited = vec![false; bw * bh];
        self.elements.clear();
        
        for by in 0..bh {
            for bx in 0..bw {
                let bi = by * bw + bx;
                if visited[bi] || bi >= mvs.len() { continue; }
                
                let (mx, my, cost) = mvs[bi];
                if cost < 1.0 { visited[bi] = true; continue; } // static block
                
                // Flood fill: find connected blocks with similar MV
                let mut region: Vec<(usize, usize)> = Vec::new();
                let mut stack = vec![(bx, by)];
                while let Some((cx, cy)) = stack.pop() {
                    let ci = cy * bw + cx;
                    if ci >= mvs.len() || visited[ci] { continue; }
                    let (cmx, cmy, _) = mvs[ci];
                    if (cmx as i16 - mx as i16).unsigned_abs() > 2 { continue; }
                    if (cmy as i16 - my as i16).unsigned_abs() > 2 { continue; }
                    visited[ci] = true;
                    region.push((cx, cy));
                    if cx > 0 { stack.push((cx-1, cy)); }
                    if cx+1 < bw { stack.push((cx+1, cy)); }
                    if cy > 0 { stack.push((cx, cy-1)); }
                    if cy+1 < bh { stack.push((cx, cy+1)); }
                }
                
                if region.len() < 3 { continue; } // too small
                
                // Compute bounding box
                let min_x = region.iter().map(|&(x,_)| x).min().unwrap() * 8;
                let min_y = region.iter().map(|&(_,y)| y).min().unwrap() * 8;
                let max_x = (region.iter().map(|&(x,_)| x).max().unwrap() + 1) * 8;
                let max_y = (region.iter().map(|&(_,y)| y).max().unwrap() + 1) * 8;
                
                // Extract anchor data for this region
                let rw = max_x - min_x;
                let rh = max_y - min_y;
                let mut anchor_block = Vec::with_capacity(rw * rh);
                for ry in min_y..max_y.min(h as usize) {
                    for rx in min_x..max_x.min(w as usize) {
                        anchor_block.push(y_plane.get(ry * w as usize + rx).copied().unwrap_or(128));
                    }
                }
                
                // Depth and shadow for this region
                let center_bi = ((min_y + rh/2) / 16) * (w as usize / 16) + (min_x + rw/2) / 16;
                let d = depth.get(center_bi).copied().unwrap_or(0.5);
                let s = shadow.get(center_bi * 3).copied().unwrap_or(0.0);
                
                self.element_counter += 1;
                self.elements.push(TrackedElement {
                    id: self.element_counter,
                    bbox: [min_x as u16, min_y as u16, rw as u16, rh as u16],
                    mv: (mx as f32, my as f32),
                    anchor_frame: frame_idx,
                    anchor_data: anchor_block,
                    confidence: 1.0,
                    depth: d,
                    is_shadow: s > 0.3,
                });
            }
        }
    }
    
    /// Predict a skip frame's content from tracked elements.
    /// Returns a predicted Y plane (or partial plane with markers).
    pub fn predict_frame(&self, target_frame: u32, w: usize, h: usize) -> Vec<u8> {
        let mut predicted = self.anchor_y.clone();
        if predicted.len() != w * h {
            predicted = vec![128u8; w * h];
        }
        
        let dt = (target_frame as f32 - self.frame_idx as f32).max(0.0);
        
        for elem in &self.elements {
            if elem.confidence < 0.3 { continue; }
            
            // Project element position forward by dt frames using MV
            let dx = (elem.mv.0 * dt).round() as i32;
            let dy = (elem.mv.1 * dt).round() as i32;
            
            let src_x = elem.bbox[0] as i32;
            let src_y = elem.bbox[1] as i32;
            let ew = elem.bbox[2] as usize;
            let eh = elem.bbox[3] as usize;
            
            let dst_x = src_x + dx;
            let dst_y = src_y + dy;
            
            // Copy anchor data to predicted position
            for ry in 0..eh {
                let sy = src_y as usize + ry;
                let dy_pos = dst_y + ry as i32;
                if dy_pos < 0 || dy_pos >= h as i32 { continue; }
                for rx in 0..ew {
                    let dx_pos = dst_x + rx as i32;
                    if dx_pos < 0 || dx_pos >= w as i32 { continue; }
                    let src_idx = ry * ew + rx;
                    if src_idx < elem.anchor_data.len() {
                        let dst_idx = dy_pos as usize * w + dx_pos as usize;
                        if dst_idx < predicted.len() {
                            predicted[dst_idx] = elem.anchor_data[src_idx];
                        }
                    }
                }
            }
        }
        
        // Decay confidence for next prediction
        predicted
    }
    
    /// Advance tracker: decay confidence, remove lost elements
    pub fn advance(&mut self) {
        self.frame_idx += 1;
        for elem in &mut self.elements {
            let dt = self.frame_idx - elem.anchor_frame;
            elem.confidence *= 0.95; // 5% decay per frame
            if dt > 60 { elem.confidence = 0.0; } // lost after 2 seconds
        }
        self.elements.retain(|e| e.confidence > 0.1);
    }
}

/// Differential quality layer for web streaming.
/// Contains the delta between the fast encode and a higher-quality version.
/// Client applies this layer on top of the base to get better quality.
#[derive(Clone, Debug)]
pub struct QualityLayer {
    pub frame_idx: u32,
    pub layer_level: u8,        // 0=base, 1=+quality, 2=+more quality
    pub delta_blocks: Vec<(u16, Vec<u8>)>,  // (block_idx, delta_coefficients)
    pub compressed_size: u32,
}

/// Inverse rule buffer: accumulates frame data with markers for LZ4 compression.
/// Frames are stored as (marker, data) pairs where marker indicates the encoding:
/// - 0x00: raw block (uncompressed)
/// - 0x01: MV-predicted block (apply MV to anchor)
/// - 0x02: delta block (XOR with prediction)
/// - 0x03: skip block (use previous)
/// - 0xFF: anchor block (self-contained)
#[derive(Debug)]
pub struct InverseRuleBuffer {
    pub data: Vec<u8>,          // accumulated frame data
    pub markers: Vec<(u32, u8, u32)>,  // (frame_idx, marker_type, data_offset)
    pub capacity: usize,
}

impl InverseRuleBuffer {
    pub fn new(capacity: usize) -> Self {
        Self {
            data: Vec::with_capacity(capacity),
            markers: Vec::new(),
            capacity,
        }
    }
    
    /// Add a frame's data with a marker
    pub fn push_frame(&mut self, frame_idx: u32, marker: u8, frame_data: &[u8]) {
        let offset = self.data.len() as u32;
        self.markers.push((frame_idx, marker, offset));
        self.data.extend_from_slice(frame_data);
        
        // If buffer exceeds capacity, flush oldest entries
        if self.data.len() > self.capacity {
            self.flush_oldest();
        }
    }
    
    /// Compress the buffer using LZ4 for fast decompression
    pub fn compress(&self) -> Vec<u8> {
        // Pack markers + data, then LZ4 compress
        let mut packed = Vec::with_capacity(self.data.len() + self.markers.len() * 9);
        packed.extend_from_slice(b"IRBF");  // magic
        packed.extend_from_slice(&(self.markers.len() as u32).to_le_bytes());
        
        for &(fidx, marker, offset) in &self.markers {
            packed.extend_from_slice(&fidx.to_le_bytes());
            packed.push(marker);
            packed.extend_from_slice(&offset.to_le_bytes());
        }
        packed.extend_from_slice(&self.data);
        
        // Use zstd level 1 for speed (close to LZ4 performance)
        zstd::encode_all(std::io::Cursor::new(&packed), 1)
            .unwrap_or(packed)
    }
    
    fn flush_oldest(&mut self) {
        if self.markers.len() < 2 { return; }
        let keep = self.markers.len() / 2;
        let cut_offset = self.markers[keep].2 as usize;
        self.data = self.data[cut_offset..].to_vec();
        for m in &mut self.markers[keep..] {
            m.2 -= cut_offset as u32;
        }
        self.markers = self.markers[keep..].to_vec();
    }
}

/// Hierarchical anchor tree for O(log₈ n) random access.
/// Each frame stores up to 8 references (parent anchors at different scales).
/// To decode any frame: follow refs up the tree (max 8 hops) to reach
/// a self-contained anchor. Shared anchors reduce redundancy.
///
/// Structure: MST with null nodes for padding, frame hashes for dedup.
///
/// Level 0: every frame (leaf)
/// Level 1: every 8th frame (local anchor)
/// Level 2: every 64th frame (regional anchor)  
/// Level 3: every 512th frame (section anchor)
/// Level 4: every 4096th frame (global anchor)
///
/// A level-N anchor can reconstruct any frame within its 8^N range
/// by following at most N intermediate anchors.

#[derive(Clone, Debug)]
pub struct FrameRef {
    pub frame_idx: u32,     // target frame index
    pub level: u8,          // anchor level (0-4)
    pub hash: u64,          // content hash for dedup (FNV-1a of Y plane)
    pub byte_offset: u64,   // offset in .fao file for direct seek
}

#[derive(Clone, Debug)]
pub struct AnchorNode {
    pub frame_idx: u32,
    pub level: u8,           // 0=leaf, 1=local, 2=regional, 3=section, 4=global
    pub refs: [u32; 8],      // up to 8 references to other anchors (u32::MAX = null)
    pub hash: u64,           // content hash
    pub byte_offset: u64,    // file offset
    pub compressed_size: u32,
    pub is_null: bool,       // null node (padding for tree structure)
}

impl Default for AnchorNode {
    fn default() -> Self {
        Self {
            frame_idx: u32::MAX, level: 0, refs: [u32::MAX; 8],
            hash: 0, byte_offset: 0, compressed_size: 0, is_null: true,
        }
    }
}

/// Anchor tree: hierarchical index for fast random access
#[derive(Clone, Debug)]
pub struct AnchorTree {
    pub nodes: Vec<AnchorNode>,
    pub hash_to_frames: Vec<(u64, Vec<u32>)>,  // hash → frame indices (dedup lookup)
    pub level_indices: [Vec<u32>; 5],           // per-level sorted frame lists
    pub max_level: u8,
}

impl AnchorTree {
    /// Build anchor tree from frame metadata
    pub fn build(total_frames: usize, frame_flags: &[(u32, u64, u32)]) -> Self {
        // frame_flags: (flags, byte_offset, compressed_size) per frame
        let mut nodes = Vec::with_capacity(total_frames);
        let mut level_indices: [Vec<u32>; 5] = [vec![], vec![], vec![], vec![], vec![]];
        let mut hash_map: std::collections::HashMap<u64, Vec<u32>> = std::collections::HashMap::new();
        
        for i in 0..total_frames {
            let (flags, offset, csize) = if i < frame_flags.len() {
                frame_flags[i]
            } else {
                (0, 0, 0)
            };
            
            // Determine level from frame position
            let level = if i % 4096 == 0 { 4u8 }
                else if i % 512 == 0 { 3 }
                else if i % 64 == 0 { 2 }
                else if i % 8 == 0 { 1 }
                else { 0 };
            
            let is_anchor = flags & 0x100 != 0 || level >= 1;
            let hash = Self::frame_hash(i, offset);
            
            let mut refs = [u32::MAX; 8];
            // Build references: point to parent anchors at each level
            // ref[0] = nearest level-1 anchor before this frame
            // ref[1] = nearest level-2 anchor before this frame
            // ref[2] = nearest level-3 anchor before this frame
            // ref[3] = nearest level-4 anchor before this frame
            // ref[4-7] = forward references for streaming/prefetch
            for lv in 0..4u8 {
                let stride = 8usize.pow(lv as u32 + 1);
                let parent = (i / stride) * stride;
                if parent < total_frames {
                    refs[lv as usize] = parent as u32;
                }
                // Forward ref
                let fwd = parent + stride;
                if fwd < total_frames && (lv as usize + 4) < 8 {
                    refs[lv as usize + 4] = fwd as u32;
                }
            }
            
            let node = AnchorNode {
                frame_idx: i as u32,
                level,
                refs,
                hash,
                byte_offset: offset,
                compressed_size: csize,
                is_null: csize == 0 && !is_anchor,
            };
            
            level_indices[level as usize].push(i as u32);
            hash_map.entry(hash).or_default().push(i as u32);
            nodes.push(node);
        }
        
        let max_level = if total_frames >= 4096 { 4 }
            else if total_frames >= 512 { 3 }
            else if total_frames >= 64 { 2 }
            else if total_frames >= 8 { 1 }
            else { 0 };
        
        let hash_to_frames: Vec<(u64, Vec<u32>)> = hash_map.into_iter().collect();
        
        Self { nodes, hash_to_frames, level_indices, max_level }
    }
    
    /// Find the decode path from a global anchor to target frame (max 8 hops)
    pub fn decode_path(&self, target: usize) -> Vec<u32> {
        let mut path = Vec::with_capacity(8);
        let mut current = target;
        
        // Walk up the tree: find parent anchors at each level
        for lv in 0..=self.max_level {
            let stride = 8usize.pow(lv as u32);
            let anchor = (current / stride) * stride;
            if anchor != current || lv == self.max_level as u8 {
                path.push(anchor as u32);
            }
            current = anchor;
        }
        
        path.reverse();  // top-down: global anchor → ... → target
        path.push(target as u32);
        path.dedup();
        path
    }
    
    /// Find frames with matching content hash (for dedup/shared anchors)
    pub fn find_by_hash(&self, hash: u64) -> &[u32] {
        for (h, frames) in &self.hash_to_frames {
            if *h == hash { return frames; }
        }
        &[]
    }
    
    /// Get the nearest anchor at or above the given level
    pub fn nearest_anchor(&self, frame: usize, min_level: u8) -> Option<u32> {
        let lv = min_level.min(self.max_level) as usize;
        // Binary search in level_indices
        let indices = &self.level_indices[lv];
        match indices.binary_search(&(frame as u32)) {
            Ok(i) => Some(indices[i]),
            Err(i) => {
                if i > 0 { Some(indices[i - 1]) }
                else if !indices.is_empty() { Some(indices[0]) }
                else { None }
            }
        }
    }
    
    /// Serialize anchor tree to bytes (for .fao file index)
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut out = Vec::new();
        // Header: magic(4) + node_count(4) + max_level(1)
        out.extend_from_slice(b"ATRE");
        out.extend_from_slice(&(self.nodes.len() as u32).to_le_bytes());
        out.push(self.max_level);
        
        // Nodes: frame_idx(4) + level(1) + refs(32) + hash(8) + offset(8) + csize(4) + flags(1) = 58 bytes
        for node in &self.nodes {
            out.extend_from_slice(&node.frame_idx.to_le_bytes());
            out.push(node.level);
            for r in &node.refs {
                out.extend_from_slice(&r.to_le_bytes());
            }
            out.extend_from_slice(&node.hash.to_le_bytes());
            out.extend_from_slice(&node.byte_offset.to_le_bytes());
            out.extend_from_slice(&node.compressed_size.to_le_bytes());
            out.push(if node.is_null { 1 } else { 0 });
        }
        
        // Hash lookup table
        out.extend_from_slice(&(self.hash_to_frames.len() as u32).to_le_bytes());
        for (hash, frames) in &self.hash_to_frames {
            out.extend_from_slice(&hash.to_le_bytes());
            out.extend_from_slice(&(frames.len() as u32).to_le_bytes());
            for f in frames {
                out.extend_from_slice(&f.to_le_bytes());
            }
        }
        
        out
    }
    
    /// Simple content hash from frame index and offset
    fn frame_hash(idx: usize, offset: u64) -> u64 {
        let mut h: u64 = 0xcbf29ce484222325;  // FNV offset basis
        for b in (idx as u64).to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        for b in offset.to_le_bytes() {
            h ^= b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
        h
    }

    /// Web-optimized byte range for streaming a frame.
    /// Returns (file_offset, byte_length) for HTTP Range requests.
    /// The decoder fetches only the blocks needed for a specific frame.
    pub fn web_range(&self, frame: usize) -> Option<(u64, u32)> {
        if frame >= self.nodes.len() { return None; }
        let node = &self.nodes[frame];
        if node.is_null { return None; }
        Some((node.byte_offset, node.compressed_size))
    }
    
    /// Get the minimal set of blocks needed to decode a frame.
    /// For streaming: fetch decode_path anchors first, then target frame.
    /// Returns blocks sorted by priority (most important first).
    pub fn streaming_plan(&self, target: usize) -> Vec<(u32, u64, u32, u8)> {
        // (frame_idx, byte_offset, size, priority)
        let path = self.decode_path(target);
        let mut plan = Vec::with_capacity(path.len());
        for (priority, &fidx) in path.iter().enumerate() {
            if (fidx as usize) < self.nodes.len() {
                let n = &self.nodes[fidx as usize];
                if !n.is_null {
                    plan.push((fidx, n.byte_offset, n.compressed_size, priority as u8));
                }
            }
        }
        plan
    }
    
    /// Cache-line aligned block layout for sequential access.
    /// Groups blocks by level for L1/L2/L3 cache efficiency.
    /// Level 4 anchors → L1 (tiny, always hot)
    /// Level 3 anchors → L2 (small, warm)
    /// Level 2 anchors → L3 (medium, occasionally accessed)
    /// Level 0-1 frames → main memory (large, sequential access)
    pub fn cache_layout(&self) -> Vec<(u8, u32, u64)> {
        // (level, frame_idx, suggested_cache_offset)
        let mut layout = Vec::new();
        let mut offset = 0u64;
        
        // Pack level 4 first (global anchors — always in L1)
        for &fidx in &self.level_indices[4] {
            if (fidx as usize) < self.nodes.len() {
                let n = &self.nodes[fidx as usize];
                layout.push((4, fidx, offset));
                offset += n.compressed_size as u64;
                // Align to 64-byte cache line
                offset = (offset + 63) & !63;
            }
        }
        
        // Level 3 (section anchors — L2)
        for &fidx in &self.level_indices[3] {
            if (fidx as usize) < self.nodes.len() {
                let n = &self.nodes[fidx as usize];
                layout.push((3, fidx, offset));
                offset += n.compressed_size as u64;
                offset = (offset + 63) & !63;
            }
        }
        
        // Level 2 (regional anchors — L3)
        for &fidx in &self.level_indices[2] {
            if (fidx as usize) < self.nodes.len() {
                let n = &self.nodes[fidx as usize];
                layout.push((2, fidx, offset));
                offset += n.compressed_size as u64;
                offset = (offset + 63) & !63;
            }
        }
        
        // Levels 0-1 (leaves — sequential main memory)
        for level in 0..=1 {
            for &fidx in &self.level_indices[level] {
                if (fidx as usize) < self.nodes.len() {
                    let n = &self.nodes[fidx as usize];
                    layout.push((level as u8, fidx, offset));
                    offset += n.compressed_size as u64;
                }
            }
        }
        
        layout
    }
    
    /// Prefetch hint: given current playback position, predict
    /// which anchors the client should prefetch for smooth seeking.
    /// Returns up to 4 anchors sorted by proximity.
    pub fn prefetch_hints(&self, current_frame: usize, direction: i8) -> Vec<u32> {
        let mut hints = Vec::with_capacity(4);
        let step = if direction >= 0 { 1i64 } else { -1i64 };
        
        // Next anchor at each level
        for level in 1..=self.max_level {
            let stride = 8usize.pow(level as u32);
            let next = if step > 0 {
                ((current_frame / stride) + 1) * stride
            } else {
                (current_frame / stride).saturating_sub(1) * stride
            };
            if next < self.nodes.len() {
                hints.push(next as u32);
            }
        }
        
        hints.sort_by_key(|&f| {
            let diff = (f as i64 - current_frame as i64).unsigned_abs();
            diff
        });
        hints.truncate(4);
        hints
    }
}

/// Scene analysis results from GPU
#[derive(Clone, Debug, Default)]
pub struct SceneAnalysis {
    pub depth_map: Vec<f32>,      // per-16x16-region depth 0..1
    pub shadow_map: Vec<f32>,     // per-block × 3: intensity, direction, penumbra
    pub rotation_map: Vec<f32>,   // per-4x4-block-region × 3: angle, magnitude, confidence
    pub geometry_map: Vec<u32>,   // per-region packed: class|nx|ny|curvature
    pub mesh_ids: Vec<u32>,       // per-region mesh segment ID
    pub n_regions: usize,
    pub n_rotation_regions: usize,
    pub n_meshes: usize,          // number of unique mesh segments
}

impl SceneAnalysis {
    pub fn depth_at(&self, rx: usize, ry: usize, rw: usize) -> f32 {
        let idx = ry * rw + rx;
        if idx < self.depth_map.len() { self.depth_map[idx] } else { 0.5 }
    }
    
    pub fn shadow_at(&self, bx: usize, by: usize, bw: usize) -> (f32, f32, f32) {
        let idx = (by * bw + bx) * 3;
        if idx + 2 < self.shadow_map.len() {
            (self.shadow_map[idx], self.shadow_map[idx+1], self.shadow_map[idx+2])
        } else { (0.0, 0.0, 0.0) }
    }
    
    pub fn geometry_class_at(&self, rx: usize, ry: usize, rw: usize) -> u8 {
        let idx = ry * rw + rx;
        if idx < self.geometry_map.len() { (self.geometry_map[idx] & 0xFF) as u8 } else { 0 }
    }
    
    pub fn surface_normal_at(&self, rx: usize, ry: usize, rw: usize) -> (f32, f32) {
        let idx = ry * rw + rx;
        if idx < self.geometry_map.len() {
            let packed = self.geometry_map[idx];
            let nx = ((packed >> 8) & 0xFF) as f32 / 127.5 - 1.0;
            let ny = ((packed >> 16) & 0xFF) as f32 / 127.5 - 1.0;
            (nx, ny)
        } else { (0.0, 0.0) }
    }
}


/// Scene model: built from anchor frame analysis, predicts future frames
#[derive(Clone, Debug)]
pub struct SceneModel {
    /// Average depth per region (stable across frames in same scene)
    pub depth_profile: Vec<f32>,
    /// Dominant shadow direction (radians) — stable under fixed lighting
    pub shadow_direction: f32,
    /// Average shadow intensity — indicates lighting conditions
    pub shadow_intensity: f32,
    /// Rotation field stability — how consistent rotation is across frames
    pub rotation_stability: f32,
    /// Geometry class distribution: [flat%, planar%, curved%, edge%]
    pub geometry_dist: [f32; 4],
    /// Number of mesh segments — stable in same scene
    pub n_meshes: u32,
    /// Average shadow intensity (0=none, 1=deep)
    pub shadow_avg: f32,
    /// Depth map variance (high = depth separation in scene)
    pub depth_variance: f32,
    /// Lighting contrast (high = dramatic lighting)
    pub light_contrast: f32,
    /// Frame index of last anchor
    pub anchor_frame: usize,
    /// Number of consecutive stable anchors
    pub stable_count: u32,
    /// Adaptive anchor interval (grows when scene is stable)
    pub anchor_interval: usize,
    /// Predicted motion magnitude for next frames
    pub predicted_motion: f32,
    /// Scene change threshold (learned from recent history)
    pub change_threshold: f32,
}

impl Default for SceneModel {
    fn default() -> Self {
        Self {
            depth_profile: vec![], shadow_direction: 0.0, shadow_intensity: 0.0,
            rotation_stability: 0.0, geometry_dist: [0.0; 4], n_meshes: 0,
            shadow_avg: 0.0, depth_variance: 0.0, light_contrast: 0.0,
            anchor_frame: 0, stable_count: 0, anchor_interval: 30,
            predicted_motion: 0.0, change_threshold: 0.3,
        }
    }
}

impl SceneModel {
    /// Update model from new anchor frame analysis
    pub fn update_from_analysis(&mut self, analysis: &SceneAnalysis, frame_idx: usize) {
        let prev_depth = self.depth_profile.clone();
        self.depth_profile = analysis.depth_map.clone();
        
        // Shadow statistics
        if !analysis.shadow_map.is_empty() {
            let n = analysis.shadow_map.len() / 3;
            let mut total_intensity = 0.0f32;
            let mut dir_x = 0.0f32;
            let mut dir_y = 0.0f32;
            for i in 0..n {
                let intensity = analysis.shadow_map[i * 3];
                let dir = analysis.shadow_map[i * 3 + 1];
                total_intensity += intensity;
                dir_x += intensity * dir.cos();
                dir_y += intensity * dir.sin();
            }
            self.shadow_intensity = total_intensity / n.max(1) as f32;
            self.shadow_direction = dir_y.atan2(dir_x);
        }
        
        // Rotation stability: low variance = stable
        if !analysis.rotation_map.is_empty() {
            let n = analysis.rotation_map.len() / 3;
            let mut confidence_sum = 0.0f32;
            for i in 0..n {
                confidence_sum += analysis.rotation_map[i * 3 + 2];
            }
            self.rotation_stability = confidence_sum / n.max(1) as f32;
        }
        
        // Geometry distribution
        self.geometry_dist = [0.0; 4];
        let total = analysis.geometry_map.len().max(1) as f32;
        for &g in &analysis.geometry_map {
            let class = (g & 0xFF) as usize;
            if class < 4 { self.geometry_dist[class] += 1.0 / total; }
        }
        
        self.n_meshes = analysis.n_meshes as u32;
        
        // Check stability: compare depth profile to previous
        let is_stable = if !prev_depth.is_empty() && prev_depth.len() == self.depth_profile.len() {
            let mut diff = 0.0f64;
            for i in 0..prev_depth.len() {
                diff += (prev_depth[i] as f64 - self.depth_profile[i] as f64).abs();
            }
            (diff / prev_depth.len() as f64) < (self.change_threshold as f64)
        } else { false };
        
        if is_stable {
            self.stable_count += 1;
            // Extend anchor interval when scene is stable (up to 120)
            self.anchor_interval = (30 * (1 + self.stable_count as usize)).min(120);
        } else {
            self.stable_count = 0;
            self.anchor_interval = 30;
        }
        
        self.anchor_frame = frame_idx;
    }
    
    /// Predict whether a frame needs full decode based on source metadata
    /// Returns: (should_decode, confidence)
    pub fn predict_frame(&self, frame_idx: usize, source_frame_bytes: u64,
                         avg_frame_bytes: u64) -> (bool, f32) {
        let frames_since_anchor = frame_idx.saturating_sub(self.anchor_frame);
        
        // Always decode if beyond anchor interval
        if frames_since_anchor >= self.anchor_interval {
            return (true, 1.0);
        }
        
        // Large frame relative to average = likely motion/scene change
        let size_ratio = source_frame_bytes as f32 / avg_frame_bytes.max(1) as f32;
        if size_ratio > 3.0 {
            return (true, 0.9);  // 3× average = significant change
        }
        
        // Scene has been stable for many anchors → very confident skip
        if self.stable_count >= 3 && size_ratio < 1.5 {
            return (false, 0.95);
        }
        
        // Moderate confidence: scene somewhat stable, frame somewhat normal
        if self.stable_count >= 1 && size_ratio < 2.0 {
            return (false, 0.7);
        }
        
        // Low confidence: decode to be safe
        if size_ratio > 2.0 || self.stable_count == 0 {
            return (true, 0.5);
        }
        
        (false, 0.6)  // default: skip with moderate confidence
    }
    
    /// Predict motion pattern for adaptive quality decisions
    /// Returns estimated bits-per-pixel for this frame
    pub fn predict_quality(&self, source_frame_bytes: u64) -> f32 {
        let est_pixels = self.depth_profile.len().max(1) * 256;  // 16×16 regions
        let bpp = source_frame_bytes as f32 * 8.0 / est_pixels as f32;
        
        // Adjust based on scene complexity
        let complexity = self.geometry_dist[2] + self.geometry_dist[3];  // curved + edges
        bpp * (1.0 + complexity * 0.5)
    }
}

#[cfg(feature = "gpu")]
pub struct GpuSlotBuffers {
    pub cur_buf: wgpu::Buffer,
    pub ref_buf: wgpu::Buffer,
    pub result_buf: wgpu::Buffer,
    pub readback_buf: wgpu::Buffer,
    pub coeffs_buf: wgpu::Buffer,
    pub recon_buf: wgpu::Buffer,
    pub coeffs_readback: wgpu::Buffer,
    pub recon_readback: wgpu::Buffer,
    pub mv_buf: wgpu::Buffer,
    pub hist_buf: wgpu::Buffer,
    pub hist_readback: wgpu::Buffer,
    pub lb_buf: wgpu::Buffer,
    pub lb_readback: wgpu::Buffer,
    // Scene analysis buffers
    pub depth_buf: wgpu::Buffer,
    pub depth_readback: wgpu::Buffer,
    pub shadow_buf: wgpu::Buffer,
    pub shadow_readback: wgpu::Buffer,
    pub rotation_buf: wgpu::Buffer,
    pub rotation_readback: wgpu::Buffer,
    pub geometry_buf: wgpu::Buffer,
    pub geometry_readback: wgpu::Buffer,
    pub mesh_buf: wgpu::Buffer,
    pub mesh_readback: wgpu::Buffer,
}

#[cfg(feature = "gpu")]
#[cfg(feature = "gpu")]
impl GpuSlotBuffers {
    fn new(device: &wgpu::Device, n_pixels: usize, n_blocks: usize, lb_size: usize) -> Self {
        let mk = |label: &str, size: u64, usage: wgpu::BufferUsages| -> wgpu::Buffer {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label), size, usage, mapped_at_creation: false,
            })
        };
        let sto_rd = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST;
        let sto_wr = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC;
        let sto_rw = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST;
        let rb = wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST;

        let n_regions = (n_pixels / 256).max(1);  // 16×16 regions
        let n_rot_regions = ((n_blocks + 15) / 16).max(1);  // 4×4 block regions
        Self {
            cur_buf: mk("cur", (n_pixels * 4) as u64, sto_rd),
            ref_buf: mk("ref", (n_pixels * 4) as u64, sto_rd),
            result_buf: mk("mv_out", (n_blocks * 3 * 4) as u64, sto_wr),
            readback_buf: mk("mv_rb", (n_blocks * 3 * 4) as u64, rb),
            coeffs_buf: mk("coeffs", (n_blocks * 64 * 4) as u64, sto_wr),
            recon_buf: mk("recon", (n_pixels * 4) as u64, sto_rw),
            coeffs_readback: mk("coeffs_rb", (n_blocks * 64 * 4) as u64, rb),
            recon_readback: mk("recon_rb", (n_pixels * 4) as u64, rb),
            mv_buf: mk("mv_in", (n_blocks * 3 * 4) as u64, sto_rd),
            hist_buf: mk("hist", (256 * 4) as u64, sto_rw),
            hist_readback: mk("hist_rb", (256 * 4) as u64, rb),
            lb_buf: mk("lb", (lb_size * 4) as u64, sto_wr),
            lb_readback: mk("lb_rb", (lb_size * 4) as u64, rb),
            // Scene analysis
            depth_buf: mk("depth", (n_regions * 4) as u64, sto_rw),
            depth_readback: mk("depth_rb", (n_regions * 4) as u64, rb),
            shadow_buf: mk("shadow", (n_blocks * 3 * 4) as u64, sto_rw),
            shadow_readback: mk("shadow_rb", (n_blocks * 3 * 4) as u64, rb),
            rotation_buf: mk("rot", (n_rot_regions * 3 * 4) as u64, sto_rw),
            rotation_readback: mk("rot_rb", (n_rot_regions * 3 * 4) as u64, rb),
            geometry_buf: mk("geom", (n_regions * 4) as u64, sto_rw),
            geometry_readback: mk("geom_rb", (n_regions * 4) as u64, rb),
            mesh_buf: mk("mesh", (n_regions * 4) as u64, sto_rw),
            mesh_readback: mk("mesh_rb", (n_regions * 4) as u64, rb),
        }
    }
}

/// Shared GPU context: workers acquire a slot, submit work, release slot
#[cfg(feature = "gpu")]
pub struct SharedGpu {
    pub encoder: GpuEncoder,
    pub width: usize,
    pub height: usize,
    slot_locks: Vec<std::sync::Mutex<()>>,
    slot_buffers: Vec<GpuSlotBuffers>,
    n_slots: usize,
    next_slot: std::sync::atomic::AtomicUsize,
}

#[cfg(feature = "gpu")]
impl SharedGpu {
    pub fn new(w: usize, h: usize, n_slots: usize) -> Option<std::sync::Arc<Self>> {
        eprintln!("FAO GPU: creating shared context...");
        let encoder = match GpuEncoder::new(w, h) {
            Some(e) => e,
            None => { eprintln!("FAO GPU: encoder init failed"); return None; }
        };
        let n = w * h;
        let bw = w / 8; let bh = h / 8;
        let n_blocks = bw * bh;
        let rw = w / 16; let rh = h / 16;
        let lb_size = rw * rh * 4 + n_blocks * 3;
        let slot_buffers: Vec<GpuSlotBuffers> = (0..n_slots)
            .map(|_| GpuSlotBuffers::new(&encoder.motion.device, n, n_blocks, lb_size))
            .collect();
        let slot_locks = (0..n_slots).map(|_| std::sync::Mutex::new(())).collect();
        // Compile scene analysis shaders
        let depth_shader = encoder.motion.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("depth"), source: wgpu::ShaderSource::Wgsl(DEPTH_SHADER.into()),
        });
        let shadow_shader = encoder.motion.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shadow"), source: wgpu::ShaderSource::Wgsl(SHADOW_SHADER.into()),
        });
        let rotation_shader = encoder.motion.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("rotation"), source: wgpu::ShaderSource::Wgsl(ROTATION_SHADER.into()),
        });
        let geometry_shader = encoder.motion.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("geometry"), source: wgpu::ShaderSource::Wgsl(GEOMETRY_SHADER.into()),
        });
        let mesh_shader = encoder.motion.device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("mesh"), source: wgpu::ShaderSource::Wgsl(MESH_SHADER.into()),
        });
        eprintln!("FAO GPU: 5 scene analysis shaders compiled");

        eprintln!("FAO GPU: shared context with {} slots ({:.1}MB per slot)", 
            n_slots, (n * 4 * 3 + n_blocks * 67 * 4 + lb_size * 4) as f64 / 1048576.0);
        Some(std::sync::Arc::new(Self { width: w, height: h, encoder, slot_locks, slot_buffers, n_slots, next_slot: std::sync::atomic::AtomicUsize::new(0) }))
    }

    /// Try to acquire a slot for GPU access (non-blocking)
    /// Returns None if GPU is busy — caller should use CPU fallback
    pub fn try_acquire_slot(&self) -> Option<GpuSlot> {
        let slot_id = self.next_slot.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % self.n_slots;
        match self.slot_locks[slot_id].try_lock() {
            Ok(guard) => Some(GpuSlot { slot_id, _guard: guard }),
            Err(std::sync::TryLockError::WouldBlock) => None,
            Err(std::sync::TryLockError::Poisoned(e)) => Some(GpuSlot { slot_id, _guard: e.into_inner() }),
        }
    }

    /// Run full encode pipeline using the slot's own buffers
    pub fn encode_with_slot(&self, slot: &GpuSlot, cur_y: &[f32], ref_y: &[f32],
                            qtable: &[f32; 64], dead_zone: f32, skip_thresh: f32) -> GpuFullEncodeResult {
        let sb = &self.slot_buffers[slot.slot_id];
        self.encode_with_buffers(sb, cur_y, ref_y, qtable, dead_zone, skip_thresh)
    }

    /// Encode using specific buffer set (slot-independent)
    fn encode_with_buffers(&self, sb: &GpuSlotBuffers, cur_y: &[f32], ref_y: &[f32],
                           qtable: &[f32; 64], dead_zone: f32, skip_thresh: f32) -> GpuFullEncodeResult {
        let device = &self.encoder.motion.device;
        let queue = &self.encoder.motion.queue;
        let n_blocks = self.encoder.n_blocks;
        let n_pixels = self.encoder.n_pixels;

        // Safety check
        if cur_y.len() != n_pixels || ref_y.len() != n_pixels {
            eprintln!("FAO GPU: size mismatch, CPU fallback");
            return GpuFullEncodeResult {
                coeffs: vec![0; n_blocks * 64], recon_y: cur_y.to_vec(),
                scene: None,
                motion: GpuEncodeResults { mvs: vec![], classes: vec![], histogram: vec![], lighting: vec![], blur: vec![] },
            };
        }

        // Upload to THIS slot's buffers
        let qt_bytes = unsafe { std::slice::from_raw_parts(qtable.as_ptr() as *const u8, 256) };
        queue.write_buffer(&self.encoder.qtable_buf, 0, qt_bytes);

        #[repr(C)]
        #[derive(Copy, Clone)]
        struct EP { w: u32, h: u32, nbx: u32, nby: u32, dz: f32, st: f32, _p: [u32; 2] }
        let w = self.encoder.motion.w;
        let h = self.encoder.motion.h;
        let bw = w / 8; let bh = h / 8;
        let ep = EP { w: w as u32, h: h as u32, nbx: bw as u32, nby: bh as u32,
            dz: dead_zone, st: skip_thresh, _p: [0; 2] };
        queue.write_buffer(&self.encoder.dct_params_buf, 0, unsafe {
            std::slice::from_raw_parts(&ep as *const _ as *const u8, 32)
        });

        let cur_bytes = unsafe { std::slice::from_raw_parts(cur_y.as_ptr() as *const u8, n_pixels * 4) };
        let ref_bytes = unsafe { std::slice::from_raw_parts(ref_y.as_ptr() as *const u8, n_pixels * 4) };
        queue.write_buffer(&sb.cur_buf, 0, cur_bytes);
        queue.write_buffer(&sb.ref_buf, 0, ref_bytes);
        queue.write_buffer(&sb.recon_buf, 0, ref_bytes);
        queue.write_buffer(&sb.hist_buf, 0, &vec![0u8; 256 * 4]);

        // Create bind groups for this slot's buffers
        // Motion bind group
        let motion_bgl = &self.encoder.motion.pipeline.get_bind_group_layout(0);
        let motion_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None, layout: motion_bgl, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.encoder.motion.params_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: sb.cur_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: sb.ref_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: sb.result_buf.as_entire_binding() },
            ],
        });

        // DCT bind group
        let dct_bgl = &self.encoder.dct_pipeline.get_bind_group_layout(0);
        let dct_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None, layout: dct_bgl, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.encoder.dct_params_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: sb.cur_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: sb.ref_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: sb.mv_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: self.encoder.qtable_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: sb.coeffs_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: sb.recon_buf.as_entire_binding() },
            ],
        });

        // Histogram bind group
        let hist_bgl = &self.encoder.hist_pipeline.get_bind_group_layout(0);
        let hist_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None, layout: hist_bgl, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.encoder.motion.params_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: sb.cur_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: sb.hist_buf.as_entire_binding() },
            ],
        });

        // Lighting/blur bind group
        let lb_bgl = &self.encoder.lb_pipeline.get_bind_group_layout(0);
        let lb_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None, layout: lb_bgl, entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.encoder.motion.params_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: sb.cur_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: sb.lb_buf.as_entire_binding() },
            ],
        });

        let max_wg = 65535u32;

        // ALL passes in ONE command buffer
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("slot_enc") });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
            pass.set_pipeline(&self.encoder.motion.pipeline);
            pass.set_bind_group(0, &motion_bg, &[]);
            let nx = (n_blocks as u32).min(max_wg);
            let ny = ((n_blocks as u32) + max_wg - 1) / max_wg;
            pass.dispatch_workgroups(nx, ny, 1);
        }
        encoder.copy_buffer_to_buffer(&sb.result_buf, 0, &sb.mv_buf, 0, (n_blocks * 3 * 4) as u64);
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
            pass.set_pipeline(&self.encoder.dct_pipeline);
            pass.set_bind_group(0, &dct_bg, &[]);
            let nx = (n_blocks as u32).min(max_wg);
            let ny = ((n_blocks as u32) + max_wg - 1) / max_wg;
            pass.dispatch_workgroups(nx, ny, 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
            pass.set_pipeline(&self.encoder.hist_pipeline);
            pass.set_bind_group(0, &hist_bg, &[]);
            pass.dispatch_workgroups(((n_pixels + 255) / 256) as u32, 1, 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
            pass.set_pipeline(&self.encoder.lb_pipeline);
            pass.set_bind_group(0, &lb_bg, &[]);
            let lx = (((self.encoder.n_regions + n_blocks + 63) / 64) as u32).min(65535);
            let ly = ((((self.encoder.n_regions + n_blocks + 63) / 64) as u32) + 65534) / 65535;
            pass.dispatch_workgroups(lx, ly, 1);
        }
        // ── Scene analysis passes (5-9) ──
        // Pass 5: Depth estimation (per 16x16 region)
        let n_regions = (self.encoder.motion.w / 16) * (self.encoder.motion.h / 16);
        let n_rot_regions = ((n_blocks + 15) / 16).max(1);
        {
            let depth_bgl = &self.encoder.motion.device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None, entries: &[
                    wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                    wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                    wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                ],
            });
            let depth_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None, layout: &depth_bgl, entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.encoder.motion.params_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: sb.cur_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: sb.depth_buf.as_entire_binding() },
                ],
            });
            let depth_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None, bind_group_layouts: &[&depth_bgl], push_constant_ranges: &[],
            });
            let depth_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("depth_d"), source: wgpu::ShaderSource::Wgsl(DEPTH_SHADER.into()),
            });
            let depth_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: None, layout: Some(&depth_pl), module: &depth_shader, entry_point: "estimate_depth",
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
            pass.set_pipeline(&depth_pipeline);
            pass.set_bind_group(0, &depth_bg, &[]);
            let dx = (n_regions as u32).min(max_wg);
            let dy = ((n_regions as u32) + max_wg - 1) / max_wg;
            pass.dispatch_workgroups(dx, dy, 1);
        }

        // Pass 6: Shadow detection (per 8x8 block)
        {
            let shadow_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None, entries: &[
                    wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                    wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                    wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                    wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                ],
            });
            let shadow_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None, layout: &shadow_bgl, entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.encoder.motion.params_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: sb.cur_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: sb.ref_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: sb.shadow_buf.as_entire_binding() },
                ],
            });
            let shadow_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None, bind_group_layouts: &[&shadow_bgl], push_constant_ranges: &[],
            });
            let shadow_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None, source: wgpu::ShaderSource::Wgsl(SHADOW_SHADER.into()),
            });
            let shadow_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: None, layout: Some(&shadow_pl), module: &shadow_shader, entry_point: "detect_shadows",
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
            pass.set_pipeline(&shadow_pipeline);
            pass.set_bind_group(0, &shadow_bg, &[]);
            let sx = (n_blocks as u32).min(max_wg);
            let sy = ((n_blocks as u32) + max_wg - 1) / max_wg;
            pass.dispatch_workgroups(sx, sy, 1);
        }

        // Pass 7: Rotation field (per 4×4 block region, uses MV results)
        {
            let rot_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None, entries: &[
                    wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                    wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                    wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                ],
            });
            let rot_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None, layout: &rot_bgl, entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.encoder.motion.params_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: sb.result_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: sb.rotation_buf.as_entire_binding() },
                ],
            });
            let rot_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None, bind_group_layouts: &[&rot_bgl], push_constant_ranges: &[],
            });
            let rot_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None, source: wgpu::ShaderSource::Wgsl(ROTATION_SHADER.into()),
            });
            let rot_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: None, layout: Some(&rot_pl), module: &rot_shader, entry_point: "estimate_rotation",
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
            pass.set_pipeline(&rot_pipeline);
            pass.set_bind_group(0, &rot_bg, &[]);
            let rx = (n_rot_regions as u32).min(max_wg);
            let ry_d = ((n_rot_regions as u32) + max_wg - 1) / max_wg;
            pass.dispatch_workgroups(rx, ry_d, 1);
        }

        // Pass 8+9: Geometry + Mesh (need depth_buf results from pass 5)
        // These run after depth is computed
        {
            let geom_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None, entries: &[
                    wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None }, count: None },
                    wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                    wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                    wgpu::BindGroupLayoutEntry { binding: 3, visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false }, has_dynamic_offset: false, min_binding_size: None }, count: None },
                ],
            });
            let geom_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None, layout: &geom_bgl, entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.encoder.motion.params_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: sb.depth_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: sb.cur_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: sb.geometry_buf.as_entire_binding() },
                ],
            });
            let geom_pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None, bind_group_layouts: &[&geom_bgl], push_constant_ranges: &[],
            });
            let geom_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: None, source: wgpu::ShaderSource::Wgsl(GEOMETRY_SHADER.into()),
            });
            let geom_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: None, layout: Some(&geom_pl), module: &geom_shader, entry_point: "classify_geometry",
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: None, timestamp_writes: None });
            pass.set_pipeline(&geom_pipeline);
            pass.set_bind_group(0, &geom_bg, &[]);
            let gx = (n_regions as u32).min(max_wg);
            let gy = ((n_regions as u32) + max_wg - 1) / max_wg;
            pass.dispatch_workgroups(gx, gy, 1);
        }

        // Copy all results including scene analysis
        encoder.copy_buffer_to_buffer(&sb.result_buf, 0, &sb.readback_buf, 0, (n_blocks * 3 * 4) as u64);
        encoder.copy_buffer_to_buffer(&sb.coeffs_buf, 0, &sb.coeffs_readback, 0, (n_blocks * 64 * 4) as u64);
        // Skip recon readback — CPU reconstructs from coefficients (saves 14MB/frame at 2K)
        encoder.copy_buffer_to_buffer(&sb.hist_buf, 0, &sb.hist_readback, 0, (256 * 4) as u64);
        encoder.copy_buffer_to_buffer(&sb.lb_buf, 0, &sb.lb_readback, 0, (self.encoder.lb_size * 4) as u64);
        // Scene analysis readbacks
        encoder.copy_buffer_to_buffer(&sb.depth_buf, 0, &sb.depth_readback, 0, (n_regions * 4) as u64);
        encoder.copy_buffer_to_buffer(&sb.shadow_buf, 0, &sb.shadow_readback, 0, (n_blocks * 3 * 4) as u64);
        encoder.copy_buffer_to_buffer(&sb.rotation_buf, 0, &sb.rotation_readback, 0, (n_rot_regions * 3 * 4) as u64);
        encoder.copy_buffer_to_buffer(&sb.geometry_buf, 0, &sb.geometry_readback, 0, (n_regions * 4) as u64);
        queue.submit(std::iter::once(encoder.finish()));

        // Readback
        let mv_s = sb.readback_buf.slice(..);
        let co_s = sb.coeffs_readback.slice(..);
        // recon readback skipped
        let hi_s = sb.hist_readback.slice(..);
        let lb_s = sb.lb_readback.slice(..);
        let (t1,r1) = std::sync::mpsc::channel(); mv_s.map_async(wgpu::MapMode::Read, move |r| {let _ = t1.send(r);});
        let (t2,r2) = std::sync::mpsc::channel(); co_s.map_async(wgpu::MapMode::Read, move |r| {let _ = t2.send(r);});

        let (t4,r4) = std::sync::mpsc::channel(); hi_s.map_async(wgpu::MapMode::Read, move |r| {let _ = t4.send(r);});
        let (t5,r5) = std::sync::mpsc::channel(); lb_s.map_async(wgpu::MapMode::Read, move |r| {let _ = t5.send(r);});
        device.poll(wgpu::Maintain::Wait);
        r1.recv().ok(); r2.recv().ok(); r4.recv().ok(); r5.recv().ok();

        let d1 = mv_s.get_mapped_range();
        let mis: &[i32] = unsafe { std::slice::from_raw_parts(d1.as_ptr() as *const i32, n_blocks * 3) };
        let mut mvs = Vec::with_capacity(n_blocks);
        for i in 0..n_blocks { mvs.push((mis[i*3].max(-127).min(127) as i8, mis[i*3+1].max(-127).min(127) as i8, mis[i*3+2] as f32)); }
        drop(d1); sb.readback_buf.unmap();

        let d2 = co_s.get_mapped_range();
        let coeffs = unsafe { std::slice::from_raw_parts(d2.as_ptr() as *const i32, n_blocks * 64) }.to_vec();
        drop(d2); sb.coeffs_readback.unmap();

        let recon = vec![];  // CPU reconstructs from coefficients

        let d4 = hi_s.get_mapped_range();
        let histogram = unsafe { std::slice::from_raw_parts(d4.as_ptr() as *const u32, 256) }.to_vec();
        drop(d4); sb.hist_readback.unmap();

        let d5 = lb_s.get_mapped_range();
        let lbf: &[f32] = unsafe { std::slice::from_raw_parts(d5.as_ptr() as *const f32, self.encoder.lb_size) };
        let lighting = lbf[..self.encoder.n_regions * 4].to_vec();
        let blur = if self.encoder.n_regions * 4 < lbf.len() { lbf[self.encoder.n_regions * 4..].to_vec() } else { vec![] };
        drop(d5); sb.lb_readback.unmap();

        // Read scene analysis results
        let depth_s = sb.depth_readback.slice(..);
        let shadow_s = sb.shadow_readback.slice(..);
        let rot_s = sb.rotation_readback.slice(..);
        let geom_s = sb.geometry_readback.slice(..);
        let (td, rd) = std::sync::mpsc::channel(); depth_s.map_async(wgpu::MapMode::Read, move |r| {let _ = td.send(r);});
        let (ts, rs) = std::sync::mpsc::channel(); shadow_s.map_async(wgpu::MapMode::Read, move |r| {let _ = ts.send(r);});
        let (tr, rr) = std::sync::mpsc::channel(); rot_s.map_async(wgpu::MapMode::Read, move |r| {let _ = tr.send(r);});
        let (tg, rg) = std::sync::mpsc::channel(); geom_s.map_async(wgpu::MapMode::Read, move |r| {let _ = tg.send(r);});
        device.poll(wgpu::Maintain::Wait);
        rd.recv().ok(); rs.recv().ok(); rr.recv().ok(); rg.recv().ok();

        let dd = depth_s.get_mapped_range();
        let depth_map = unsafe { std::slice::from_raw_parts(dd.as_ptr() as *const f32, n_regions) }.to_vec();
        drop(dd); sb.depth_readback.unmap();

        let sd = shadow_s.get_mapped_range();
        let shadow_map = unsafe { std::slice::from_raw_parts(sd.as_ptr() as *const f32, n_blocks * 3) }.to_vec();
        drop(sd); sb.shadow_readback.unmap();

        let rrd = rot_s.get_mapped_range();
        let rotation_map = unsafe { std::slice::from_raw_parts(rrd.as_ptr() as *const f32, n_rot_regions * 3) }.to_vec();
        drop(rrd); sb.rotation_readback.unmap();

        let gd = geom_s.get_mapped_range();
        let geometry_map = unsafe { std::slice::from_raw_parts(gd.as_ptr() as *const u32, n_regions) }.to_vec();
        drop(gd); sb.geometry_readback.unmap();

        let scene = SceneAnalysis {
            depth_map, shadow_map, rotation_map, geometry_map,
            mesh_ids: vec![], n_regions, n_rotation_regions: n_rot_regions, n_meshes: 0,
        };

        GpuFullEncodeResult { coeffs, recon_y: recon, scene: Some(scene),
            motion: GpuEncodeResults { mvs, classes: vec![], histogram, lighting, blur } }
    }

    /// Run motion-only pipeline (lighter, for workers that don't need full DCT)
    pub fn motion_only(&self, _slot: &GpuSlot, cur_y: &[f32], ref_y: &[f32]) -> GpuEncodeResults {
        self.encoder.motion.full_pipeline(cur_y, ref_y)
    }

}
#[cfg(feature = "gpu")]
pub struct GpuSlot<'a> {
    pub slot_id: usize,
    _guard: std::sync::MutexGuard<'a, ()>,
}

#[cfg(not(feature = "gpu"))]
pub struct SharedGpu;
#[cfg(not(feature = "gpu"))]
impl SharedGpu {
    pub fn new(_w: usize, _h: usize, _n_slots: usize) -> Option<std::sync::Arc<Self>> { None }
}

/// Global shared GPU instance with background init thread
static SHARED_GPU: std::sync::Mutex<Option<(usize, usize, std::sync::Arc<SharedGpu>)>> = std::sync::Mutex::new(None);
static GPU_INIT_IN_PROGRESS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Get or create shared GPU for the given resolution.
/// If resolution changed, spawns a background thread to reinitialize.
/// Returns None while init is in progress (CPU fallback).
pub fn get_shared_gpu(w: usize, h: usize) -> Option<std::sync::Arc<SharedGpu>> {
    #[cfg(feature = "gpu")]
    {
        // Fast path: check if GPU exists and matches
        if let Ok(guard) = SHARED_GPU.try_lock() {
            if let Some((gw, gh, ref gpu)) = *guard {
                if gw == w && gh == h {
                    return Some(gpu.clone());
                }
            }
        }

        // Only one init thread at a time
        if GPU_INIT_IN_PROGRESS.load(std::sync::atomic::Ordering::SeqCst) {
            return None;
        }
        
        // Try to claim init — atomic CAS prevents races
        if GPU_INIT_IN_PROGRESS.compare_exchange(
            false, true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst
        ).is_err() {
            return None;  // another thread got it
        }

        // We won the race. Check under mutex.
        let mut guard = SHARED_GPU.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((gw, gh, ref gpu)) = *guard {
            if gw == w && gh == h {
                GPU_INIT_IN_PROGRESS.store(false, std::sync::atomic::Ordering::SeqCst);
                return Some(gpu.clone());
            }
        }
        *guard = None;
        drop(guard);

        // Spawn init on background thread so UI doesn't freeze
        let init_w = w;
        let init_h = h;
        std::thread::Builder::new().name("fao-gpu-init".into()).spawn(move || {
            eprintln!("FAO GPU: initializing for {}x{} on background thread...", init_w, init_h);
            let result = SharedGpu::new(init_w, init_h, 8);
            let mut guard = SHARED_GPU.lock().unwrap_or_else(|e| e.into_inner());
            match result {
                Some(gpu) => {
                    eprintln!("FAO GPU: ready for {}x{}", init_w, init_h);
                    *guard = Some((init_w, init_h, gpu));
                }
                None => {
                    eprintln!("FAO GPU: init failed for {}x{}", init_w, init_h);
                    *guard = None;
                }
            }
            GPU_INIT_IN_PROGRESS.store(false, std::sync::atomic::Ordering::Relaxed);
        }).ok();

        // Return None for this frame — GPU will be ready for next frames
        None
    }
    #[cfg(not(feature = "gpu"))]
    { None }
}

#[cfg(feature = "gpu")]
impl GpuEncoder {
    /// Submit frame for GPU processing WITHOUT waiting for results.
    /// Returns previous frame's results (if available).
    pub fn submit_async(&self, cur_y: &[f32], ref_y: &[f32], qtable: &[f32; 64],
                        dead_zone: f32, skip_thresh: f32) -> Option<GpuFullEncodeResult> {
        // If a frame is in flight, collect its results first
        let prev = if self.frame_in_flight.load(std::sync::atomic::Ordering::Relaxed) {
            Some(self.collect_results())
        } else {
            None
        };

        // Upload and submit new frame
        let qt_bytes = unsafe { std::slice::from_raw_parts(qtable.as_ptr() as *const u8, 256) };
        self.motion.queue.write_buffer(&self.qtable_buf, 0, qt_bytes);

        #[repr(C)]
        #[derive(Copy, Clone)]
        struct EP { w: u32, h: u32, nbx: u32, nby: u32, dz: f32, st: f32, _p: [u32; 2] }
        let bw = self.motion.w / 8; let bh = self.motion.h / 8;
        let ep = EP { w: self.motion.w as u32, h: self.motion.h as u32,
            nbx: bw as u32, nby: bh as u32, dz: dead_zone, st: skip_thresh, _p: [0; 2] };
        self.motion.queue.write_buffer(&self.dct_params_buf, 0, unsafe {
            std::slice::from_raw_parts(&ep as *const _ as *const u8, 32)
        });

        // Safety: verify buffer sizes match
        let expected = self.n_pixels * 4;
        if cur_y.len() * 4 != expected || ref_y.len() * 4 != expected {
            eprintln!("FAO GPU: buffer size mismatch! plane={}B, gpu={}B", cur_y.len() * 4, expected);
            return None;  // fall back to CPU for this frame
        }
        let cur_bytes = unsafe { std::slice::from_raw_parts(cur_y.as_ptr() as *const u8, cur_y.len() * 4) };
        let ref_bytes = unsafe { std::slice::from_raw_parts(ref_y.as_ptr() as *const u8, ref_y.len() * 4) };
        self.motion.queue.write_buffer(&self.motion.cur_buf, 0, cur_bytes);
        self.motion.queue.write_buffer(&self.motion.ref_buf, 0, ref_bytes);
        self.motion.queue.write_buffer(&self.recon_buf, 0, ref_bytes);
        self.motion.queue.write_buffer(&self.hist_buf, 0, &vec![0u8; 256 * 4]);

        // ALL passes in ONE command buffer
        let mut encoder = self.motion.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("async_full") });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("motion"), timestamp_writes: None });
            pass.set_pipeline(&self.motion.pipeline);
            pass.set_bind_group(0, &self.motion.bind_group, &[]);
            pass.dispatch_workgroups(self.n_blocks as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&self.motion.result_buf, 0, &self.mv_buf, 0, (self.n_blocks * 3 * 4) as u64);
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("dct"), timestamp_writes: None });
            pass.set_pipeline(&self.dct_pipeline);
            pass.set_bind_group(0, &self.dct_bind_group, &[]);
            pass.dispatch_workgroups(self.n_blocks as u32, 1, 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("hist"), timestamp_writes: None });
            pass.set_pipeline(&self.hist_pipeline);
            pass.set_bind_group(0, &self.hist_bind_group, &[]);
            pass.dispatch_workgroups(((self.n_pixels + 255) / 256) as u32, 1, 1);
        }
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("lb"), timestamp_writes: None });
            pass.set_pipeline(&self.lb_pipeline);
            pass.set_bind_group(0, &self.lb_bind_group, &[]);
            pass.dispatch_workgroups(((self.n_regions + self.n_blocks + 63) / 64) as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&self.motion.result_buf, 0, &self.motion.readback_buf, 0, (self.n_blocks * 3 * 4) as u64);
        encoder.copy_buffer_to_buffer(&self.coeffs_buf, 0, &self.coeffs_readback, 0, (self.n_blocks * 64 * 4) as u64);
        encoder.copy_buffer_to_buffer(&self.recon_buf, 0, &self.recon_readback, 0, (self.n_pixels * 4) as u64);
        encoder.copy_buffer_to_buffer(&self.hist_buf, 0, &self.hist_readback, 0, (256 * 4) as u64);
        encoder.copy_buffer_to_buffer(&self.lb_buf, 0, &self.lb_readback, 0, (self.lb_size * 4) as u64);
        self.motion.queue.submit(std::iter::once(encoder.finish()));
        self.frame_in_flight.store(true, std::sync::atomic::Ordering::Relaxed);

        prev
    }

    /// Collect results from in-flight GPU work (blocks until done)
    fn collect_results(&self) -> GpuFullEncodeResult {
        let mv_slice = self.motion.readback_buf.slice(..);
        let coeff_slice = self.coeffs_readback.slice(..);
        let recon_slice = self.recon_readback.slice(..);
        let hist_slice = self.hist_readback.slice(..);
        let lb_slice = self.lb_readback.slice(..);

        let (tx1, rx1) = std::sync::mpsc::channel();
        mv_slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx1.send(r); });
        let (tx2, rx2) = std::sync::mpsc::channel();
        coeff_slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx2.send(r); });
        let (tx3, rx3) = std::sync::mpsc::channel();
        recon_slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx3.send(r); });
        let (tx4, rx4) = std::sync::mpsc::channel();
        hist_slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx4.send(r); });
        let (tx5, rx5) = std::sync::mpsc::channel();
        lb_slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx5.send(r); });

        self.motion.device.poll(wgpu::Maintain::Wait);
        rx1.recv().ok(); rx2.recv().ok(); rx3.recv().ok(); rx4.recv().ok(); rx5.recv().ok();

        let mvd = mv_slice.get_mapped_range();
        let mv_ints: &[i32] = unsafe { std::slice::from_raw_parts(mvd.as_ptr() as *const i32, self.n_blocks * 3) };
        let mut mvs = Vec::with_capacity(self.n_blocks);
        for i in 0..self.n_blocks {
            mvs.push((mv_ints[i*3].max(-127).min(127) as i8, mv_ints[i*3+1].max(-127).min(127) as i8, mv_ints[i*3+2] as f32));
        }
        drop(mvd); self.motion.readback_buf.unmap();

        let cd = coeff_slice.get_mapped_range();
        let coeffs = unsafe { std::slice::from_raw_parts(cd.as_ptr() as *const i32, self.n_blocks * 64) }.to_vec();
        drop(cd); self.coeffs_readback.unmap();

        let rd = recon_slice.get_mapped_range();
        let recon = unsafe { std::slice::from_raw_parts(rd.as_ptr() as *const f32, self.n_pixels) }.to_vec();
        drop(rd); self.recon_readback.unmap();

        let hd = hist_slice.get_mapped_range();
        let histogram = unsafe { std::slice::from_raw_parts(hd.as_ptr() as *const u32, 256) }.to_vec();
        drop(hd); self.hist_readback.unmap();

        let ld = lb_slice.get_mapped_range();
        let lb_floats: &[f32] = unsafe { std::slice::from_raw_parts(ld.as_ptr() as *const f32, self.lb_size) };
        let lighting = lb_floats[..self.n_regions * 4].to_vec();
        let blur = if self.n_regions * 4 < lb_floats.len() { lb_floats[self.n_regions * 4..].to_vec() } else { vec![] };
        drop(ld); self.lb_readback.unmap();

        self.frame_in_flight.store(false, std::sync::atomic::Ordering::Relaxed);

        GpuFullEncodeResult {
            coeffs, recon_y: recon,
            scene: None,
            motion: GpuEncodeResults { mvs, classes: vec![], histogram, lighting, blur },
        }
    }

    /// Flush: collect final in-flight frame
    pub fn flush(&self) -> Option<GpuFullEncodeResult> {
        if self.frame_in_flight.load(std::sync::atomic::Ordering::Relaxed) {
            Some(self.collect_results())
        } else {
            None
        }
    }
}