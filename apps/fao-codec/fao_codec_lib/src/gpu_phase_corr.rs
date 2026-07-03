//! GPU phase correlation for global camera motion estimation.
//!
//! Mirrors the CPU implementation in `phase_correlation::PhaseCorrelationPass`
//! but runs the entire 2D FFT pipeline on the GPU:
//!
//!   1. Window + DC-remove the centered N×N Y-plane patch from cur and prev
//!      (CPU computes the per-window mean in one fast pass and ships it as
//!      a uniform; GPU does the per-pixel windowing).
//!   2. 2D forward FFT (rows then columns) on each — Stockham radix-2.
//!   3. Cross-power spectrum: A · conj(B) / |A · conj(B)|.
//!   4. 2D inverse FFT — produces the correlation surface.
//!   5. Find peak via parallel reduction.
//!   6. Read back peak coords + magnitude for sub-pixel refinement on CPU.
//!
//! Each frame is one round-trip: upload 2× N²/4 × 4 bytes of Y plus a
//! 16-byte uniform; dispatch 7 shader passes; read back 16 bytes.
//!
//! # FFT
//!
//! Stockham auto-sort radix-2. log2(N) stages, each stage:
//!   - reads from one ping-pong buffer
//!   - writes butterflied output to the other
//!   - swaps roles for the next stage
//!
//! For N=256, 8 stages run inside one workgroup with shared memory holding
//! both buffers (256 × 8 bytes × 2 = 4 KB — well within the 16 KB limit
//! on every modern GPU).

#![cfg(feature = "gpu")]

use wgpu;
use crate::phase_correlation::CameraMotion;

pub const PHASE_CORR_WGSL: &str = include_str!("phase_corr.wgsl");

/// One-shot GPU pipeline for a fixed window size N (typically 256).
/// Reused across frames — buffers are persistent.
pub struct GpuPhaseCorr {
    pub window_size: u32,
    pub device: std::sync::Arc<wgpu::Device>,
    pub queue: std::sync::Arc<wgpu::Queue>,
    
    pub windowing_pipeline: wgpu::ComputePipeline,
    pub fft_rows_pipeline: wgpu::ComputePipeline,
    pub fft_cols_pipeline: wgpu::ComputePipeline,
    pub cross_spectrum_pipeline: wgpu::ComputePipeline,
    pub ifft_rows_pipeline: wgpu::ComputePipeline,
    pub ifft_cols_pipeline: wgpu::ComputePipeline,
    pub peak_find_pipeline: wgpu::ComputePipeline,
    
    /// Packed cur Y window (256×256 → 16384 u32 words of u8s).
    pub cur_y_buf: wgpu::Buffer,
    pub prev_y_buf: wgpu::Buffer,
    /// Precomputed Hann window of length N.
    pub hann_buf: wgpu::Buffer,
    /// Complex spectrum A (cur). 256×256 × 2 f32 = 512 KB.
    pub spec_a_buf: wgpu::Buffer,
    /// Complex spectrum B (prev). Same size. Used during windowing+FFT,
    /// then collapsed into spec_a after cross_spectrum.
    pub spec_b_buf: wgpu::Buffer,
    /// Peak result: (peak_x, peak_y, peak_val_bits, second_val_bits) — 16B.
    pub peak_buf: wgpu::Buffer,
    pub peak_readback: wgpu::Buffer,
    /// Uniform: (ws: u32, _pad: u32, mean_cur: f32, mean_prev: f32).
    pub dims_buf: wgpu::Buffer,
    
    pub bind_layout: wgpu::BindGroupLayout,
    pub bind_group: wgpu::BindGroup,
}

impl GpuPhaseCorr {
    pub fn new(
        device: std::sync::Arc<wgpu::Device>,
        queue: std::sync::Arc<wgpu::Queue>,
        window_size: usize,
    ) -> Option<Self> {
        let ws = window_size;
        if !ws.is_power_of_two() || ws < 32 || ws > 1024 { return None; }
        // The shader is hard-coded to N=256 for shared-memory sizing
        // (workgroup-local arrays must have compile-time constant size).
        // For other sizes we'd need a different shader variant.
        if ws != 256 { return None; }
        let n = (ws * ws) as u64;
        
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("phase_corr"),
            source: wgpu::ShaderSource::Wgsl(PHASE_CORR_WGSL.into()),
        });
        
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("phase_corr_bg_layout"),
            entries: &[
                // 0: cur_y (read-only storage)
                bg_entry(0, wgpu::BufferBindingType::Storage { read_only: true }),
                // 1: prev_y (read-only storage)
                bg_entry(1, wgpu::BufferBindingType::Storage { read_only: true }),
                // 2: hann (read-only storage)
                bg_entry(2, wgpu::BufferBindingType::Storage { read_only: true }),
                // 3: spec_a (read-write storage)
                bg_entry(3, wgpu::BufferBindingType::Storage { read_only: false }),
                // 4: spec_b (read-write storage)
                bg_entry(4, wgpu::BufferBindingType::Storage { read_only: false }),
                // 5: peak (read-write storage with atomics)
                bg_entry(5, wgpu::BufferBindingType::Storage { read_only: false }),
                // 6: dims (uniform)
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("phase_corr_layout"),
            bind_group_layouts: &[&bind_layout],
            push_constant_ranges: &[],
        });
        
        let make_pipeline = |entry: &str| -> wgpu::ComputePipeline {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: entry,
            })
        };
        
        let windowing_pipeline = make_pipeline("windowing");
        let fft_rows_pipeline = make_pipeline("fft_rows");
        let fft_cols_pipeline = make_pipeline("fft_cols");
        let cross_spectrum_pipeline = make_pipeline("cross_spectrum");
        let ifft_rows_pipeline = make_pipeline("ifft_rows");
        let ifft_cols_pipeline = make_pipeline("ifft_cols");
        let peak_find_pipeline = make_pipeline("peak_find");
        
        let y_words = ((ws * ws + 3) / 4) as u64;
        let mk_buf = |label: &str, sz: u64, usage: wgpu::BufferUsages| -> wgpu::Buffer {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: sz,
                usage,
                mapped_at_creation: false,
            })
        };
        let storage = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC;
        let cur_y_buf = mk_buf("cur_y", y_words * 4, storage);
        let prev_y_buf = mk_buf("prev_y", y_words * 4, storage);
        let hann_buf = mk_buf("hann", ws as u64 * 4, storage);
        let spec_a_buf = mk_buf("spec_a", n * 8, storage);
        let spec_b_buf = mk_buf("spec_b", n * 8, storage);
        let peak_buf = mk_buf("peak", 4 * 4, storage);
        let peak_readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("peak_readback"),
            size: 4 * 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let dims_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dims"),
            size: 4 * 4,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        
        // Upload Hann window once.
        let hann: Vec<f32> = (0..ws).map(|i| {
            let t = i as f32 / (ws - 1) as f32;
            0.5 * (1.0 - (2.0 * std::f32::consts::PI * t).cos())
        }).collect();
        queue.write_buffer(&hann_buf, 0, bytemuck_cast(&hann));
        
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("phase_corr_bg"),
            layout: &bind_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: cur_y_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: prev_y_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: hann_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: spec_a_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: spec_b_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: peak_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 6, resource: dims_buf.as_entire_binding() },
            ],
        });
        
        Some(Self {
            window_size: ws as u32,
            device, queue,
            windowing_pipeline, fft_rows_pipeline, fft_cols_pipeline,
            cross_spectrum_pipeline, ifft_rows_pipeline, ifft_cols_pipeline,
            peak_find_pipeline,
            cur_y_buf, prev_y_buf, hann_buf,
            spec_a_buf, spec_b_buf, peak_buf, peak_readback, dims_buf,
            bind_layout, bind_group,
        })
    }
    
    /// Run the full GPU pipeline for one frame pair. Returns CameraMotion 
    /// matching the CPU implementation's output format.
    /// 
    /// `cur_yuv` / `prev_yuv` are full-frame Y planes (size = full_w * full_h).
    /// The N×N analysis window is taken from the CENTER of each frame.
    pub fn estimate(
        &self,
        cur_yuv: &[u8],
        prev_yuv: &[u8],
        full_w: usize,
        full_h: usize,
    ) -> CameraMotion {
        let ws = self.window_size as usize;
        if full_w < ws || full_h < ws { return CameraMotion::none(); }
        if cur_yuv.len() < full_w * full_h || prev_yuv.len() < full_w * full_h {
            return CameraMotion::none();
        }
        
        // Extract centered N×N windows (CPU-side — copying 64K bytes is 
        // trivial vs the GPU pipeline cost).
        let cx = (full_w - ws) / 2;
        let cy = (full_h - ws) / 2;
        let mut cur_win = vec![0u8; ws * ws];
        let mut prev_win = vec![0u8; ws * ws];
        for y in 0..ws {
            let src = (cy + y) * full_w + cx;
            let dst = y * ws;
            cur_win[dst..dst+ws].copy_from_slice(&cur_yuv[src..src+ws]);
            prev_win[dst..dst+ws].copy_from_slice(&prev_yuv[src..src+ws]);
        }
        
        // Compute means on CPU (single pass, ~64K adds — negligible).
        let mean_cur: f32 = (cur_win.iter().map(|&b| b as u32).sum::<u32>() as f64 / (ws*ws) as f64) as f32;
        let mean_prev: f32 = (prev_win.iter().map(|&b| b as u32).sum::<u32>() as f64 / (ws*ws) as f64) as f32;
        
        // Pack u8 → u32 for storage buffer access (4 pixels per word, little-endian).
        let pack = |bytes: &[u8]| -> Vec<u32> {
            bytes.chunks(4).map(|c| {
                let mut w = 0u32;
                for (i, &b) in c.iter().enumerate() { w |= (b as u32) << (i * 8); }
                w
            }).collect()
        };
        let cur_packed = pack(&cur_win);
        let prev_packed = pack(&prev_win);
        
        // Upload buffers.
        self.queue.write_buffer(&self.cur_y_buf, 0, bytemuck_cast_u32(&cur_packed));
        self.queue.write_buffer(&self.prev_y_buf, 0, bytemuck_cast_u32(&prev_packed));
        // Uniform: ws (u32), pad, mean_cur, mean_prev. We bit-cast f32→u32 
        // so the uniform is just 4 u32s in WGSL.
        let dims = [ws as u32, 0u32, mean_cur.to_bits(), mean_prev.to_bits()];
        self.queue.write_buffer(&self.dims_buf, 0, bytemuck_cast_u32(&dims));
        // Clear peak buffer. The val_bits slots start at 0.0 (bit pattern 
        // 0u32) — atomicMax in the shader only updates with POSITIVE floats 
        // (the shader skips v <= 0.0), so 0u32 as bit-pattern is the correct 
        // initial sentinel. Negative floats and the IEEE 754 sign bit would 
        // give bogus orderings under atomicMax — using 0.0 sidesteps that.
        let peak_init = [0u32, 0u32, 0u32, 0u32];
        self.queue.write_buffer(&self.peak_buf, 0, bytemuck_cast_u32(&peak_init));
        
        // Encode the dispatch chain.
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("phase_corr_encode"),
        });
        let n2 = ws * ws;
        let dispatch_grid = |pass: &mut wgpu::ComputePass, threads: u32, group_size: u32| {
            let groups = (threads + group_size - 1) / group_size;
            pass.dispatch_workgroups(groups, 1, 1);
        };
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("phase_corr_pass"),
                timestamp_writes: None,
            });
            pass.set_bind_group(0, &self.bind_group, &[]);
            
            // 1. Windowing: per-pixel, 64×64×64 dispatches at 64 threads each.
            //    Uses workgroup_size(64). For N=256 → n2 = 65536 → 1024 wgs.
            pass.set_pipeline(&self.windowing_pipeline);
            dispatch_grid(&mut pass, n2 as u32, 64);
            
            // 2. FFT rows: dispatch 256 workgroups (one per row), each runs
            //    the in-workgroup Stockham FFT on 256 points.
            pass.set_pipeline(&self.fft_rows_pipeline);
            pass.dispatch_workgroups(ws as u32, 1, 1);
            
            // 3. FFT cols: same pattern, 256 workgroups (one per column).
            pass.set_pipeline(&self.fft_cols_pipeline);
            pass.dispatch_workgroups(ws as u32, 1, 1);
            
            // 4. Cross-power spectrum: per-pixel.
            pass.set_pipeline(&self.cross_spectrum_pipeline);
            dispatch_grid(&mut pass, n2 as u32, 64);
            
            // 5. IFFT rows.
            pass.set_pipeline(&self.ifft_rows_pipeline);
            pass.dispatch_workgroups(ws as u32, 1, 1);
            
            // 6. IFFT cols.
            pass.set_pipeline(&self.ifft_cols_pipeline);
            pass.dispatch_workgroups(ws as u32, 1, 1);
            
            // 7. Peak find: parallel reduction with atomic peak update.
            pass.set_pipeline(&self.peak_find_pipeline);
            dispatch_grid(&mut pass, n2 as u32, 64);
        }
        // Copy peak_buf → peak_readback for CPU read.
        encoder.copy_buffer_to_buffer(&self.peak_buf, 0, &self.peak_readback, 0, 16);
        self.queue.submit(Some(encoder.finish()));
        
        // Map+read peak_readback. wgpu's poll-based map is synchronous-ish
        // in this thread context (we block on receiver).
        let buf_slice = self.peak_readback.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        buf_slice.map_async(wgpu::MapMode::Read, move |r| { let _ = tx.send(r); });
        self.device.poll(wgpu::Maintain::Wait);
        match rx.recv() {
            Ok(Ok(())) => {}
            _ => return CameraMotion::none(),
        }
        let data = buf_slice.get_mapped_range();
        let peak_x = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        let peak_y = u32::from_le_bytes([data[4], data[5], data[6], data[7]]);
        let peak_val = f32::from_bits(u32::from_le_bytes([data[8], data[9], data[10], data[11]]));
        let second_val = f32::from_bits(u32::from_le_bytes([data[12], data[13], data[14], data[15]]));
        drop(data);
        self.peak_readback.unmap();
        
        // Convert peak coordinates to signed displacement (FFT wraps past N/2).
        let ws_i = ws as i32;
        let dx_raw = if peak_x as i32 > ws_i / 2 { peak_x as i32 - ws_i } else { peak_x as i32 };
        let dy_raw = if peak_y as i32 > ws_i / 2 { peak_y as i32 - ws_i } else { peak_y as i32 };
        
        // No GPU-side sub-pixel refinement yet — would require fetching the 
        // peak's 4 neighbors on readback. CPU CameraMotion accepts integer 
        // pixel displacement; the parabolic interpolation in the reference
        // path adds <0.5 pixel of accuracy that we lose here. Acceptable
        // for a first GPU pass.
        let confidence = if second_val > 0.0 && peak_val > 0.0 {
            ((peak_val - second_val) / peak_val).clamp(0.0, 1.0)
        } else if peak_val > 0.0 {
            1.0
        } else {
            0.0
        };
        
        CameraMotion {
            dx: dx_raw as f32,
            dy: dy_raw as f32,
            confidence,
            // GPU path doesn't compute zoom — would require log-polar
            // remapping. Match the CPU default (1.0 = no zoom).
            zoom: 1.0,
        }
    }
}

fn bg_entry(binding: u32, ty: wgpu::BufferBindingType) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn bytemuck_cast(s: &[f32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(s.as_ptr() as *const u8, std::mem::size_of_val(s)) }
}

fn bytemuck_cast_u32(s: &[u32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(s.as_ptr() as *const u8, std::mem::size_of_val(s)) }
}

// ── Tests ────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phase_correlation::PhaseCorrelationPass;
    
    /// Async wgpu setup. Returns None if no adapter available (CI 
    /// environments without GPU).
    fn try_make_device() -> Option<(std::sync::Arc<wgpu::Device>, std::sync::Arc<wgpu::Queue>)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::all(),
            ..Default::default()
        });
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: None,
        }))?;
        let (device, queue) = pollster::block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("phase_corr_test"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
            },
            None,
        )).ok()?;
        Some((std::sync::Arc::new(device), std::sync::Arc::new(queue)))
    }
    
    /// Generate a Y-plane with a recognizable feature shifted by (dx, dy).
    /// The shift wraps around the frame edges (toroidal) so the FFT-based 
    /// phase correlation can find it without edge artifacts.
    fn synthetic_pair(w: usize, h: usize, dx: i32, dy: i32) -> (Vec<u8>, Vec<u8>) {
        let mut prev = vec![0u8; w * h];
        // Sparse random-ish "features" — multi-scale dots so the FFT has 
        // content across the spectrum.
        for y in 0..h {
            for x in 0..w {
                let v = (((x * 7 + y * 13) ^ (x * y / 17)) & 0xff) as u8;
                prev[y * w + x] = v.saturating_add(64);
            }
        }
        let mut cur = vec![0u8; w * h];
        for y in 0..h {
            for x in 0..w {
                let sx = ((x as i32 - dx).rem_euclid(w as i32)) as usize;
                let sy = ((y as i32 - dy).rem_euclid(h as i32)) as usize;
                cur[y * w + x] = prev[sy * w + sx];
            }
        }
        (cur, prev)
    }
    
    #[test]
    fn gpu_matches_cpu_on_known_shift() {
        let (device, queue) = match try_make_device() {
            Some(d) => d,
            None => {
                eprintln!("[gpu_phase_corr test] no GPU adapter — skipping");
                return;
            }
        };
        let gpu = match GpuPhaseCorr::new(device, queue, 256) {
            Some(g) => g,
            None => {
                eprintln!("[gpu_phase_corr test] could not init pipeline — skipping");
                return;
            }
        };
        let cpu = PhaseCorrelationPass::new(256);
        
        let w = 384;
        let h = 384;
        for &(dx, dy) in &[(0i32, 0i32), (5, -3), (-7, 12), (15, 15)] {
            let (cur, prev) = synthetic_pair(w, h, dx, dy);
            let cpu_motion = cpu.estimate(&cur, &prev, w, h);
            let gpu_motion = gpu.estimate(&cur, &prev, w, h);
            
            // Tolerate ±1 pixel difference (CPU has sub-pixel; GPU is integer-only).
            let diff_x = (cpu_motion.dx - gpu_motion.dx).abs();
            let diff_y = (cpu_motion.dy - gpu_motion.dy).abs();
            assert!(diff_x <= 1.5 && diff_y <= 1.5,
                "shift ({},{}) — CPU=({:.2},{:.2}), GPU=({:.2},{:.2}), diff=({:.2},{:.2})",
                dx, dy, cpu_motion.dx, cpu_motion.dy, gpu_motion.dx, gpu_motion.dy, diff_x, diff_y);
        }
    }
}
