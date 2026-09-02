//! waifu2x `upconv_7` on wgpu compute.
//!
//! Vulkan on Asahi (Mesa/Honeykrisp), Metal on macOS, DX12/GL elsewhere —
//! wgpu picks. Two kernels (`conv3x3.wgsl`, `deconv4x4s2.wgsl`) run per
//! tile on a pair of ping-pong activation buffers; the deconv emits packed
//! RGBA8 straight into the readback buffer.
//!
//! Every choice that touched Honeykrisp in this tree is honoured here
//! (`maintenance/ASAHI_GPU.md`, `ASAHI_VULKAN_LESSONS.md`): the adapter's
//! own limits are requested, no subgroup ops, no workgroup memory, no
//! dynamic shifts in WGSL, dispatch dimensions checked against the 65535
//! limit, and a software adapter is named out loud rather than run
//! silently.
//!
//! [`selftest`] checks the kernels byte-for-byte (±1 for f32 reordering)
//! against `d2x_core::waifu2x`'s CPU forward pass. A green selftest on
//! llvmpipe validates the plumbing — layouts, bounds, tile edges, readback —
//! which is most of what breaks; the M-series run is still the evidence
//! for the numerics on that driver.

use anyhow::{anyhow, Context};
use d2x_core::frame::{Frame, Pad};
use d2x_core::upscale::Upscaler;
use d2x_core::waifu2x::{extract_tile, tiles, write_tile_rgba8, Tensor, Upconv7, PRE_PAD};
use wgpu::util::DeviceExt;

pub const CONV_WGSL: &str = include_str!("../shaders/conv3x3.wgsl");
pub const DECONV_WGSL: &str = include_str!("../shaders/deconv4x4s2.wgsl");

#[derive(Debug, Clone, Copy)]
pub struct GpuOptions {
    /// Requested tile side in input pixels. Clamped down if the adapter's
    /// buffer limits can't hold the largest activation for it.
    pub tile: u32,
    /// Refuse a software adapter (llvmpipe, SwiftShader, …) instead of
    /// warning. What a fleet deployment wants; CI wants the warning.
    pub require_hw: bool,
    pub low_power: bool,
}

impl Default for GpuOptions {
    fn default() -> Self {
        GpuOptions { tile: 240, require_hw: false, low_power: false }
    }
}

/// Names that mean "there is no GPU here". Pure so it can be tested against
/// the strings drivers actually report.
pub fn adapter_is_software(name: &str, device_type: wgpu::DeviceType) -> bool {
    if device_type == wgpu::DeviceType::Cpu {
        return true;
    }
    let n = name.to_ascii_lowercase();
    ["llvmpipe", "swiftshader", "softpipe", "software", "microsoft basic render"].iter().any(|s| n.contains(s))
}

#[derive(Debug, Clone)]
pub struct AdapterReport {
    pub name: String,
    pub backend: String,
    pub device_type: String,
    pub driver: String,
    pub software: bool,
    pub max_storage_binding: u64,
    pub max_buffer: u64,
    pub max_workgroups_per_dim: u32,
}

impl std::fmt::Display for AdapterReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} [{} / {}] driver: {}{} — storage binding {} MiB, buffer {} MiB",
            self.name,
            self.backend,
            self.device_type,
            self.driver,
            if self.software { " (SOFTWARE)" } else { "" },
            self.max_storage_binding >> 20,
            self.max_buffer >> 20
        )
    }
}

fn report(adapter: &wgpu::Adapter) -> AdapterReport {
    let i = adapter.get_info();
    let l = adapter.limits();
    AdapterReport {
        name: i.name.clone(),
        backend: format!("{:?}", i.backend),
        device_type: format!("{:?}", i.device_type),
        driver: format!("{} {}", i.driver, i.driver_info).trim().to_string(),
        software: adapter_is_software(&i.name, i.device_type),
        max_storage_binding: l.max_storage_buffer_binding_size as u64,
        max_buffer: l.max_buffer_size,
        max_workgroups_per_dim: l.max_compute_workgroups_per_dimension,
    }
}

/// List every adapter wgpu can see. The `probe` subcommand.
pub fn probe() -> Vec<AdapterReport> {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
    pollster::block_on(instance.enumerate_adapters(wgpu::Backends::all())).iter().map(report).collect()
}

/// Bytes of the largest activation buffer a tile of side `t` needs: the
/// conv6 output, 256 channels of (t + 2)². (Input is t + 14, each conv
/// takes 2.)
fn activation_bytes(t: u64) -> u64 {
    256 * (t + 2) * (t + 2) * 4
}

/// Largest tile whose activation buffer fits the adapter's limits.
pub fn fit_tile(requested: u32, max_storage_binding: u64, max_buffer: u64) -> u32 {
    let cap = max_storage_binding.min(max_buffer);
    let mut t = requested.max(16);
    while activation_bytes(t as u64) > cap && t > 16 {
        t -= 8;
    }
    t
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ConvParams {
    in_w: u32,
    in_h: u32,
    in_ch: u32,
    out_ch4: u32,
    out_w: u32,
    out_h: u32,
    _pad0: u32,
    _pad1: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DeconvParams {
    in_w: u32,
    in_h: u32,
    in_ch: u32,
    out_w: u32,
    out_h: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

struct ConvLayer {
    in_ch: u32,
    out_ch: u32,
    params: wgpu::Buffer,
    bind_group: wgpu::BindGroup,
}

pub struct WgpuUpscaler {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pub adapter: AdapterReport,
    model_name: String,
    pub tile: u32,
    conv_pipeline: wgpu::ComputePipeline,
    deconv_pipeline: wgpu::ComputePipeline,
    convs: Vec<ConvLayer>,
    deconv_params: wgpu::Buffer,
    deconv_bind: wgpu::BindGroup,
    act_in: wgpu::Buffer,
    out: wgpu::Buffer,
    staging: wgpu::Buffer,
    // Kept alive for the bind groups.
    _act_a: wgpu::Buffer,
    _act_b: wgpu::Buffer,
    _weights: Vec<wgpu::Buffer>,
}

impl WgpuUpscaler {
    pub fn new(model: &Upconv7, opts: GpuOptions) -> anyhow::Result<Self> {
        // wgpu 29: `InstanceDescriptor::default()` is gone (the display-handle
        // split); `new_without_display_handle_from_env` honours WGPU_BACKEND etc.
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle_from_env());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: if opts.low_power { wgpu::PowerPreference::LowPower } else { wgpu::PowerPreference::HighPerformance },
            force_fallback_adapter: false,
            compatible_surface: None,
        }))
        .map_err(|e| {
            anyhow!(
                "no GPU adapter: {e}\n  On Asahi: install mesa-vulkan-drivers (asahi COPR) + vulkan-loader and check `vulkaninfo --summary`.\n  Elsewhere: `dandere2x probe` lists what wgpu can see; `--backend cpu` needs no GPU."
            )
        })?;
        let rep = report(&adapter);
        if rep.software {
            let msg = format!(
                "adapter is a software rasterizer: {rep}\n  This will be ~100x slower than a GPU. On Asahi this usually means Honeykrisp is missing or shadowed (`vulkaninfo --summary`)."
            );
            if opts.require_hw {
                return Err(anyhow!("{msg}\n  Refusing because --require-hw was given."));
            }
            eprintln!("warning: {msg}");
        }

        // The adapter's own limits are always valid; wgpu's defaults may
        // not be (beatspace died on max_color_attachments this way).
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("dandere2x"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .context("request_device")?;

        let tile = fit_tile(opts.tile, rep.max_storage_binding, rep.max_buffer);
        if tile != opts.tile {
            eprintln!("note: tile {} does not fit this adapter's buffer limits; using {}", opts.tile, tile);
        }
        let t = tile as u64;
        let in_side = t + 2 * PRE_PAD as u64;
        // Per-dimension dispatch check (the 65535 lesson): conv is 2x2-blocked
        // over up to (t+12)² outputs, deconv over (2t)².
        let max_dim = (2 * t).div_ceil(16) as u32;
        anyhow::ensure!(max_dim <= rep.max_workgroups_per_dim, "tile {tile} needs {max_dim} workgroups per dimension, limit {}", rep.max_workgroups_per_dim);

        let conv_module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("conv3x3"), source: wgpu::ShaderSource::Wgsl(CONV_WGSL.into()) });
        let deconv_module = device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some("deconv4x4s2"), source: wgpu::ShaderSource::Wgsl(DECONV_WGSL.into()) });

        let storage = |binding: u32, read_only: bool| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let uniform = |binding: u32| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let conv_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("conv"),
            entries: &[uniform(0), storage(1, true), storage(2, true), storage(3, true), storage(4, false)],
        });
        let deconv_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("deconv"),
            entries: &[uniform(0), storage(1, true), storage(2, true), uniform(3), storage(4, false)],
        });
        let make_pipeline = |label: &str, module: &wgpu::ShaderModule, layout: &wgpu::BindGroupLayout| {
            // wgpu 29: bind_group_layouts are Option<&_>, push constants became `immediate_size`.
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(label), bind_group_layouts: &[Some(layout)], immediate_size: 0 });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        let conv_pipeline = make_pipeline("conv3x3", &conv_module, &conv_layout);
        let deconv_pipeline = make_pipeline("deconv4x4s2", &deconv_module, &deconv_layout);

        // Activation buffers.
        let act_bytes = activation_bytes(t);
        let act_in = device.create_buffer(&wgpu::BufferDescriptor { label: Some("act_in"), size: 3 * in_side * in_side * 4, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let act_a = device.create_buffer(&wgpu::BufferDescriptor { label: Some("act_a"), size: act_bytes, usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false });
        let act_b = device.create_buffer(&wgpu::BufferDescriptor { label: Some("act_b"), size: act_bytes, usage: wgpu::BufferUsages::STORAGE, mapped_at_creation: false });
        let out_bytes = (2 * t) * (2 * t) * 4;
        let out = device.create_buffer(&wgpu::BufferDescriptor { label: Some("out"), size: out_bytes, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
        let staging = device.create_buffer(&wgpu::BufferDescriptor { label: Some("staging"), size: out_bytes, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });

        // Weights, in the kernels' layouts.
        let mut weights = Vec::new();
        let mut convs = Vec::new();
        anyhow::ensure!(model.convs.len() == 6, "upconv_7 has 6 convs");
        for (i, l) in model.convs.iter().enumerate() {
            anyhow::ensure!(l.out_ch % 4 == 0, "conv {i}: out_ch {} not a multiple of 4", l.out_ch);
            let oc4 = l.out_ch / 4;
            let mut w: Vec<f32> = Vec::with_capacity(oc4 * l.in_ch * 9 * 4);
            for g in 0..oc4 {
                for ic in 0..l.in_ch {
                    for k in 0..9 {
                        for j in 0..4 {
                            w.push(l.weight[((g * 4 + j) * l.in_ch + ic) * 9 + k]);
                        }
                    }
                }
            }
            let mut b: Vec<f32> = Vec::with_capacity(oc4 * 4);
            for g in 0..oc4 {
                for j in 0..4 {
                    b.push(l.bias[g * 4 + j]);
                }
            }
            let wbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(&format!("w{i}")), contents: bytemuck::cast_slice(&w), usage: wgpu::BufferUsages::STORAGE });
            let bbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(&format!("b{i}")), contents: bytemuck::cast_slice(&b), usage: wgpu::BufferUsages::STORAGE });
            let params = device.create_buffer(&wgpu::BufferDescriptor { label: Some(&format!("p{i}")), size: std::mem::size_of::<ConvParams>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            // Ping-pong: L0 in→A, L1 A→B, L2 B→A, L3 A→B, L4 B→A, L5 A→B.
            let (src, dst): (&wgpu::Buffer, &wgpu::Buffer) = match i {
                0 => (&act_in, &act_a),
                1 | 3 | 5 => (&act_a, &act_b),
                _ => (&act_b, &act_a),
            };
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(&format!("conv{i}")),
                layout: &conv_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: src.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: wbuf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 3, resource: bbuf.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 4, resource: dst.as_entire_binding() },
                ],
            });
            weights.push(wbuf);
            weights.push(bbuf);
            convs.push(ConvLayer { in_ch: l.in_ch as u32, out_ch: l.out_ch as u32, params, bind_group });
        }
        let d = &model.deconv;
        anyhow::ensure!(d.out_ch == 3, "deconv out_ch {}", d.out_ch);
        let mut w: Vec<f32> = Vec::with_capacity(d.in_ch * 16 * 4);
        for ic in 0..d.in_ch {
            for k in 0..16 {
                for oc in 0..3 {
                    w.push(d.weight[(ic * 3 + oc) * 16 + k]);
                }
                w.push(0.0);
            }
        }
        let dw = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("dw"), contents: bytemuck::cast_slice(&w), usage: wgpu::BufferUsages::STORAGE });
        let db = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("db"), contents: bytemuck::cast_slice(&[d.bias[0], d.bias[1], d.bias[2], 0.0f32]), usage: wgpu::BufferUsages::UNIFORM });
        let deconv_params = device.create_buffer(&wgpu::BufferDescriptor { label: Some("dp"), size: std::mem::size_of::<DeconvParams>() as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        let deconv_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("deconv"),
            layout: &deconv_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: deconv_params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: act_b.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: dw.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: db.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: out.as_entire_binding() },
            ],
        });
        weights.push(dw);
        weights.push(db);

        Ok(WgpuUpscaler {
            device,
            queue,
            adapter: rep,
            model_name: model.name.clone(),
            tile,
            conv_pipeline,
            deconv_pipeline,
            convs,
            deconv_params,
            deconv_bind,
            act_in,
            out,
            staging,
            _act_a: act_a,
            _act_b: act_b,
            _weights: weights,
        })
    }

    /// Run the network on one pre-padded tile (`3×h×w`, `h,w ≤ tile+14`),
    /// returning `(2h−28)×(2w−28)` packed RGBA8.
    pub fn run_tile(&mut self, input: &Tensor) -> anyhow::Result<Vec<u8>> {
        anyhow::ensure!(input.c == 3, "input must be RGB");
        let side = (self.tile + 2 * PRE_PAD) as usize;
        anyhow::ensure!(input.w <= side && input.h <= side, "tile {}x{} exceeds {}", input.w, input.h, side);
        anyhow::ensure!(input.w > 14 && input.h > 14, "tile too small");
        self.queue.write_buffer(&self.act_in, 0, bytemuck::cast_slice(&input.data));

        let (mut w, mut h) = (input.w as u32, input.h as u32);
        let mut encoder = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("tile") });
        for l in &self.convs {
            let (ow, oh) = (w - 2, h - 2);
            self.queue.write_buffer(&l.params, 0, bytemuck::bytes_of(&ConvParams { in_w: w, in_h: h, in_ch: l.in_ch, out_ch4: l.out_ch / 4, out_w: ow, out_h: oh, _pad0: 0, _pad1: 0 }));
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("conv"), timestamp_writes: None });
            pass.set_pipeline(&self.conv_pipeline);
            pass.set_bind_group(0, &l.bind_group, &[]);
            // 2x2 outputs per thread, 16x16 threads per workgroup.
            pass.dispatch_workgroups(ow.div_ceil(32), oh.div_ceil(32), l.out_ch / 4);
            drop(pass);
            (w, h) = (ow, oh);
        }
        let (ow, oh) = (2 * w - 4, 2 * h - 4);
        self.queue.write_buffer(&self.deconv_params, 0, bytemuck::bytes_of(&DeconvParams { in_w: w, in_h: h, in_ch: self.convs[5].out_ch, out_w: ow, out_h: oh, _pad0: 0, _pad1: 0, _pad2: 0 }));
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("deconv"), timestamp_writes: None });
            pass.set_pipeline(&self.deconv_pipeline);
            pass.set_bind_group(0, &self.deconv_bind, &[]);
            pass.dispatch_workgroups(ow.div_ceil(16), oh.div_ceil(16), 1);
        }
        let out_bytes = (ow as u64) * (oh as u64) * 4;
        encoder.copy_buffer_to_buffer(&self.out, 0, &self.staging, 0, out_bytes);
        self.queue.submit(Some(encoder.finish()));

        let slice = self.staging.slice(..out_bytes);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device.poll(wgpu::PollType::wait_indefinitely()).context("device poll")?;
        rx.recv().context("map_async dropped")?.context("map_async")?;
        let bytes = slice.get_mapped_range().to_vec();
        self.staging.unmap();
        Ok(bytes)
    }

    pub fn upscale_frame(&mut self, frame: &Frame) -> anyhow::Result<Frame> {
        let padded = frame.padded(PRE_PAD, Pad::Replicate);
        let mut out = Frame::new(frame.width * 2, frame.height * 2);
        for rect in tiles(frame.width, frame.height, self.tile) {
            let input = extract_tile(&padded, rect);
            let rgba = self.run_tile(&input)?;
            write_tile_rgba8(&mut out, rect, &rgba);
        }
        Ok(out)
    }
}

impl Upscaler for WgpuUpscaler {
    fn scale(&self) -> u32 {
        2
    }
    fn upscale(&mut self, frame: &Frame) -> anyhow::Result<Frame> {
        self.upscale_frame(frame)
    }
    fn name(&self) -> String {
        format!("waifu2x-wgpu [{}] on {} tile {}", self.model_name, self.adapter.name, self.tile)
    }
}

#[derive(Debug, Clone)]
pub struct SelftestReport {
    pub adapter: AdapterReport,
    pub frame_px: (u32, u32),
    pub compared: usize,
    pub exact: usize,
    pub worst_abs_diff: u8,
    pub pass: bool,
}

impl std::fmt::Display for SelftestReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "adapter: {}", self.adapter)?;
        writeln!(f, "frame {}x{} -> {} output bytes compared", self.frame_px.0, self.frame_px.1, self.compared)?;
        writeln!(f, "exact: {} ({:.1}%), worst |gpu-cpu| = {}", self.exact, 100.0 * self.exact as f64 / self.compared.max(1) as f64, self.worst_abs_diff)?;
        write!(f, "{}", if self.pass { "PASS — within ±1 (f32 reordering)" } else { "FAIL — kernels disagree with the CPU reference" })
    }
}

/// GPU vs CPU on a small frame that spans two tiles and has odd
/// dimensions, so edge patches, the last partial workgroup, and the tile
/// seam are all exercised.
pub fn selftest(model: &Upconv7, opts: GpuOptions) -> anyhow::Result<SelftestReport> {
    let mut gpu = WgpuUpscaler::new(model, GpuOptions { tile: opts.tile.min(48), ..opts })?;
    let (w, h) = (gpu.tile + 13, 37u32);
    let mut frame = Frame::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let v = |k: u32| ((x * 7 + y * 13 + k * 31) % 251) as u8;
            frame.set_px(x, y, [v(0), v(1), v(2)]);
        }
    }
    let got = gpu.upscale_frame(&frame)?;
    let want = d2x_core::waifu2x::upscale_cpu(model, &frame, gpu.tile);
    anyhow::ensure!(got.data.len() == want.data.len(), "size mismatch");
    let mut worst = 0u8;
    let mut exact = 0usize;
    for (&g, &c) in got.data.iter().zip(&want.data) {
        let d = (g as i16 - c as i16).unsigned_abs() as u8;
        worst = worst.max(d);
        if d == 0 {
            exact += 1;
        }
    }
    Ok(SelftestReport { adapter: gpu.adapter.clone(), frame_px: (w, h), compared: got.data.len(), exact, worst_abs_diff: worst, pass: worst <= 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shader fitness gate: both kernels parse and validate under naga
    /// with no GPU at all. Catches the class of error that otherwise
    /// appears only at `create_shader_module` on the device.
    #[test]
    fn shaders_validate() {
        for (name, src) in [("conv3x3", CONV_WGSL), ("deconv4x4s2", DECONV_WGSL)] {
            let module = naga::front::wgsl::parse_str(src).unwrap_or_else(|e| panic!("{name}: {}", e.emit_to_string(src)));
            let mut v = naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::empty());
            v.validate(&module).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        }
    }

    #[test]
    fn software_classifier() {
        assert!(adapter_is_software("llvmpipe (LLVM 20.1.2, 256 bits)", wgpu::DeviceType::Cpu));
        assert!(adapter_is_software("SwiftShader Device (Subzero)", wgpu::DeviceType::Cpu));
        assert!(!adapter_is_software("Apple M2 Ultra (G14D B1)", wgpu::DeviceType::IntegratedGpu));
        assert!(!adapter_is_software("NVIDIA GeForce RTX 3080", wgpu::DeviceType::DiscreteGpu));
        assert!(!adapter_is_software("Intel(R) Arc(TM) A770 Graphics", wgpu::DeviceType::DiscreteGpu));
    }

    #[test]
    fn fit_tile_respects_limits() {
        assert_eq!(fit_tile(240, u64::MAX, u64::MAX), 240);
        // 128 MiB binding limit: 256*(t+2)^2*4 <= 2^27 → t+2 <= 362 → 240 fits
        assert_eq!(fit_tile(240, 1 << 27, u64::MAX), 240);
        // 16 MiB: t+2 <= 128 → steps of 8 down from 240 → 120
        assert_eq!(fit_tile(240, 1 << 24, u64::MAX), 120);
        assert!(fit_tile(240, 1, 1) >= 16);
    }

    /// Runs only where an adapter exists (lavapipe counts). `cargo test`
    /// on a box with no ICD skips it rather than failing.
    #[test]
    fn gpu_matches_cpu_if_adapter_present() {
        if probe().is_empty() {
            eprintln!("skipping: no wgpu adapter");
            return;
        }
        let model = Upconv7::synthetic(7);
        let r = selftest(&model, GpuOptions { tile: 32, ..Default::default() }).unwrap();
        eprintln!("{r}");
        assert!(r.pass, "{r}");
    }
}
