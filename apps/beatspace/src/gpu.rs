//! Optional GPU backend (`--features gpu`).
//!
//! Runs the **generator** forward pass on the GPU via `wgpu`, batched: many
//! latent vectors in, many mel "beats" out, in one set of dispatches. wgpu
//! picks a real device through Vulkan (Mesa, incl. Apple GPU on Asahi), Metal,
//! DX12, or GL — so this works where candle's CUDA/Metal backends don't.
//!
//! This is an *inference* path only (training stays on candle/CPU). At eval
//! time BatchNorm is a fixed per-channel affine, so the whole generator reduces
//! to: one `fc` GEMM, then four transposed-conv passes (each folding BN into a
//! scale/shift and applying ReLU, or Tanh on the last). It loads the exact same
//! weights as the candle generator and matches it numerically.
//!
//! GPU + batching pays off for *offline* work — pre-rendering a latent-space
//! grid, a long A→B walk, or a pile of variations — not single-beat realtime,
//! where per-dispatch and transfer overhead dominate at batch size 1.

use anyhow::{anyhow, Result};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::model::GenParams;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct FcParams {
    batch: u32,
    latent: u32,
    f0: u32,
    _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct DeconvParams {
    batch: u32,
    cin: u32,
    cout: u32,
    ih: u32,
    iw: u32,
    oh: u32,
    ow: u32,
    k: u32,
    stride: u32,
    pad: u32,
    act: u32,
    _pad: u32,
}

const FC_WGSL: &str = r#"
struct P { batch:u32, latent:u32, f0:u32, pad:u32 };
@group(0) @binding(0) var<storage, read>        z:    array<f32>;
@group(0) @binding(1) var<storage, read>        w:    array<f32>;
@group(0) @binding(2) var<storage, read>        bias: array<f32>;
@group(0) @binding(3) var<storage, read_write>  outp: array<f32>;
@group(0) @binding(4) var<uniform>              p:    P;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    let total = p.batch * p.f0;
    if (idx >= total) { return; }
    let b = idx / p.f0;
    let o = idx % p.f0;
    var acc = bias[o];
    let zoff = b * p.latent;
    let woff = o * p.latent;          // w is (f0, latent) row-major
    for (var k: u32 = 0u; k < p.latent; k = k + 1u) {
        acc = acc + z[zoff + k] * w[woff + k];
    }
    outp[idx] = acc;
}
"#;

const DECONV_WGSL: &str = r#"
struct P { batch:u32, cin:u32, cout:u32, ih:u32, iw:u32, oh:u32, ow:u32,
           k:u32, stride:u32, pad:u32, act:u32, pad2:u32 };
@group(0) @binding(0) var<storage, read>        inp:   array<f32>;
@group(0) @binding(1) var<storage, read>        w:     array<f32>;   // (cin,cout,k,k)
@group(0) @binding(2) var<storage, read>        scale: array<f32>;   // (cout,)
@group(0) @binding(3) var<storage, read>        shift: array<f32>;   // (cout,)
@group(0) @binding(4) var<storage, read_write>  outp:  array<f32>;
@group(0) @binding(5) var<uniform>              p:     P;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    let total = p.batch * p.cout * p.oh * p.ow;
    if (idx >= total) { return; }

    let plane = p.oh * p.ow;
    let b  = idx / (p.cout * plane);
    var r  = idx % (p.cout * plane);
    let oc = r / plane;
    r      = r % plane;
    let oy = r / p.ow;
    let ox = r % p.ow;

    let k = p.k; let s = p.stride; let pd = p.pad;
    var acc = 0.0;
    for (var ic: u32 = 0u; ic < p.cin; ic = ic + 1u) {
        let inbase = (b * p.cin + ic) * p.ih * p.iw;
        for (var ky: u32 = 0u; ky < k; ky = ky + 1u) {
            let ty = i32(oy) + i32(pd) - i32(ky);
            if (ty < 0) { continue; }
            if (u32(ty) % s != 0u) { continue; }
            let iy = u32(ty) / s;
            if (iy >= p.ih) { continue; }
            for (var kx: u32 = 0u; kx < k; kx = kx + 1u) {
                let tx = i32(ox) + i32(pd) - i32(kx);
                if (tx < 0) { continue; }
                if (u32(tx) % s != 0u) { continue; }
                let ix = u32(tx) / s;
                if (ix >= p.iw) { continue; }
                let wv = w[((ic * p.cout + oc) * k + ky) * k + kx];
                acc = acc + inp[inbase + iy * p.iw + ix] * wv;
            }
        }
    }
    acc = acc * scale[oc] + shift[oc];
    if (p.act == 1u) { acc = max(acc, 0.0); }
    else if (p.act == 2u) { acc = tanh(acc); }
    outp[idx] = acc;
}
"#;

/// A deconv layer's weights resident on the GPU.
struct GpuLayer {
    w: wgpu::Buffer,
    scale: wgpu::Buffer,
    shift: wgpu::Buffer,
    cin: u32,
    cout: u32,
    k: u32,
    stride: u32,
    pad: u32,
    act: u32,
}

pub struct GpuGenerator {
    device: wgpu::Device,
    queue: wgpu::Queue,
    fc_pipeline: wgpu::ComputePipeline,
    deconv_pipeline: wgpu::ComputePipeline,
    fc_w: wgpu::Buffer,
    fc_b: wgpu::Buffer,
    layers: Vec<GpuLayer>,
    latent: usize,
    f0: usize,
    h0: usize,
    w0: usize,
    n_mels: usize,
    time_frames: usize,
    adapter_name: String,
}

impl GpuGenerator {
    pub fn adapter_name(&self) -> &str {
        &self.adapter_name
    }

    pub fn new(p: &GenParams) -> Result<Self> {
        pollster::block_on(Self::new_async(p))
    }

    async fn new_async(p: &GenParams) -> Result<Self> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                force_fallback_adapter: false,
                compatible_surface: None,
            })
            .await
            .map_err(|e| anyhow!("no GPU adapter (wgpu): {e}"))?;
        let info = adapter.get_info();
        let adapter_name = format!("{} ({:?}, {:?})", info.name, info.device_type, info.backend);

        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("beatspace-gpu"),
                required_features: wgpu::Features::empty(),
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .map_err(|e| anyhow!("request_device: {e}"))?;

        let fc_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("fc"),
            source: wgpu::ShaderSource::Wgsl(FC_WGSL.into()),
        });
        let deconv_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("deconv"),
            source: wgpu::ShaderSource::Wgsl(DECONV_WGSL.into()),
        });
        let fc_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("fc"),
            layout: None,
            module: &fc_module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let deconv_pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("deconv"),
            layout: None,
            module: &deconv_module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });

        let storage = wgpu::BufferUsages::STORAGE;
        let mkbuf = |label: &str, data: &[f32]| {
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytemuck::cast_slice(data),
                usage: storage,
            })
        };

        let fc_w = mkbuf("fc_w", &p.fc_w);
        let fc_b = mkbuf("fc_b", &p.fc_b);
        let layers = p
            .deconvs
            .iter()
            .map(|d| GpuLayer {
                w: mkbuf("deconv_w", &d.w),
                scale: mkbuf("deconv_scale", &d.scale),
                shift: mkbuf("deconv_shift", &d.shift),
                cin: d.cin as u32,
                cout: d.cout as u32,
                k: d.k as u32,
                stride: d.stride as u32,
                pad: d.padding as u32,
                act: d.act,
            })
            .collect();

        Ok(Self {
            device,
            queue,
            fc_pipeline,
            deconv_pipeline,
            fc_w,
            fc_b,
            layers,
            latent: p.latent,
            f0: p.f0,
            h0: p.h0,
            w0: p.w0,
            n_mels: p.n_mels,
            time_frames: p.time_frames,
            adapter_name,
        })
    }

    /// `zs` is `batch * latent` row-major. Returns `batch * n_mels * time_frames`
    /// row-major (each beat is n_mels x time_frames).
    pub fn generate_batch(&self, zs: &[f32], batch: usize) -> Result<Vec<f32>> {
        assert_eq!(zs.len(), batch * self.latent, "z length mismatch");
        let dev = &self.device;

        let storage_io = wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC;
        let uniform = wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST;

        // upload latents
        let z_buf = dev.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("z"),
            contents: bytemuck::cast_slice(zs),
            usage: wgpu::BufferUsages::STORAGE,
        });

        // fc output buffer (batch * f0)
        let fc_out = dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some("fc_out"),
            size: (batch * self.f0 * 4) as u64,
            usage: storage_io,
            mapped_at_creation: false,
        });
        let fc_params = dev.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("fc_params"),
            contents: bytemuck::bytes_of(&FcParams {
                batch: batch as u32,
                latent: self.latent as u32,
                f0: self.f0 as u32,
                _pad: 0,
            }),
            usage: uniform,
        });

        let mut encoder = dev.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gen"),
        });

        // ---- fc pass ----
        {
            let bgl = self.fc_pipeline.get_bind_group_layout(0);
            let bg = dev.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("fc_bg"),
                layout: &bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: z_buf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: self.fc_w.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: self.fc_b.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: fc_out.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 4, resource: fc_params.as_entire_binding() },
                ],
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("fc"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.fc_pipeline);
            pass.set_bind_group(0, &bg, &[]);
            let total = (batch * self.f0) as u32;
            pass.dispatch_workgroups(total.div_ceil(64), 1, 1);
        }

        // ---- deconv passes (ping-pong) ----
        let mut cur = fc_out;
        let mut ih = self.h0;
        let mut iw = self.w0;
        for layer in &self.layers {
            let oh = (ih - 1) * layer.stride as usize - 2 * layer.pad as usize + layer.k as usize;
            let ow = (iw - 1) * layer.stride as usize - 2 * layer.pad as usize + layer.k as usize;
            let out_len = batch * layer.cout as usize * oh * ow;
            let next = dev.create_buffer(&wgpu::BufferDescriptor {
                label: Some("deconv_out"),
                size: (out_len * 4) as u64,
                usage: storage_io,
                mapped_at_creation: false,
            });
            let params = dev.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("deconv_params"),
                contents: bytemuck::bytes_of(&DeconvParams {
                    batch: batch as u32,
                    cin: layer.cin,
                    cout: layer.cout,
                    ih: ih as u32,
                    iw: iw as u32,
                    oh: oh as u32,
                    ow: ow as u32,
                    k: layer.k,
                    stride: layer.stride,
                    pad: layer.pad,
                    act: layer.act,
                    _pad: 0,
                }),
                usage: uniform,
            });
            let bgl = self.deconv_pipeline.get_bind_group_layout(0);
            let bg = dev.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("deconv_bg"),
                layout: &bgl,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: cur.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: layer.w.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: layer.scale.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: layer.shift.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 4, resource: next.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 5, resource: params.as_entire_binding() },
                ],
            });
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("deconv"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.deconv_pipeline);
            pass.set_bind_group(0, &bg, &[]);
            let total = out_len as u32;
            pass.dispatch_workgroups(total.div_ceil(64), 1, 1);
            drop(pass);

            cur = next;
            ih = oh;
            iw = ow;
        }

        // ---- read back final buffer (batch * n_mels * time_frames) ----
        let out_count = batch * self.n_mels * self.time_frames;
        let staging = dev.create_buffer(&wgpu::BufferDescriptor {
            label: Some("staging"),
            size: (out_count * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(&cur, 0, &staging, 0, (out_count * 4) as u64);
        self.queue.submit(Some(encoder.finish()));

        let (tx, rx) = std::sync::mpsc::channel();
        staging.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        dev.poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| anyhow!("poll: {e}"))?;
        rx.recv()
            .map_err(|_| anyhow!("map channel closed"))?
            .map_err(|e| anyhow!("buffer map: {e}"))?;

        let data = staging.slice(..).get_mapped_range();
        let out: Vec<f32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        Ok(out)
    }
}
