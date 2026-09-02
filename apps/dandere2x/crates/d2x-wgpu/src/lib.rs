//! `d2x_core::net::Network` on wgpu compute.
//!
//! Vulkan on Asahi (Mesa/Honeykrisp), Metal on macOS, DX12/GL elsewhere —
//! wgpu picks. The network's op list is executed as-is: one bind group per
//! op built once at init, per-tile parameter writes, one command buffer per
//! tile, one readback of packed RGBA8. Every activation buffer is bound
//! read-write, because a dense block reads `[0, n)` and writes `[n, n+32)`
//! of the *same* buffer and wgpu rejects mixing read-only and read-write
//! bindings of one buffer in a bind group.
//!
//! Honeykrisp rules from `maintenance/ASAHI_GPU.md` / `ASAHI_VULKAN_LESSONS.md`
//! honoured: the adapter's own limits, no subgroup ops, no workgroup memory,
//! no dynamic shifts in WGSL, dispatch dimensions checked against the 65535
//! limit, a software adapter named out loud.
//!
//! [`selftest`] checks the runner byte-for-byte (±1 for f32 reordering)
//! against the CPU reference. A green selftest on llvmpipe validates
//! layouts, bounds, tile edges and readback — which is most of what breaks —
//! not this driver's numerics on M-series.

use anyhow::{anyhow, Context};
use d2x_core::frame::Frame;
use d2x_core::net::{tiles, Act, Network, Op, Tensor, TileIo};
use d2x_core::upscale::Upscaler;
use std::sync::Arc;
use wgpu::util::DeviceExt;

pub const CONV_WGSL: &str = include_str!("../shaders/conv3x3.wgsl");
pub const DECONV_WGSL: &str = include_str!("../shaders/deconv4x4s2.wgsl");
pub const ADD_WGSL: &str = include_str!("../shaders/add_scaled.wgsl");
pub const PACK_WGSL: &str = include_str!("../shaders/pack.wgsl");

#[derive(Debug, Clone, Copy)]
pub struct GpuOptions {
    /// Requested tile side in input pixels. Clamped down if the adapter's
    /// buffer limits can't hold the network's largest buffer for it.
    pub tile: u32,
    /// Refuse a software adapter (llvmpipe, SwiftShader, …) instead of warning.
    pub require_hw: bool,
    pub low_power: bool,
}

impl Default for GpuOptions {
    fn default() -> Self {
        GpuOptions { tile: 0, require_hw: false, low_power: false }
    }
}

/// Names that mean "there is no GPU here".
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

/// Default tile per network: big for the deep same-pad nets (seams shrink
/// with tile size and the body is the cost either way), exact for valid nets.
pub fn default_tile(net: &Network) -> u32 {
    if net.exact {
        240
    } else if net.unshuffle >= 2 {
        400
    } else {
        256
    }
}

/// Bytes of the largest buffer the network needs for a tile of side `t`.
pub fn max_buffer_bytes(net: &Network, t: u64) -> u64 {
    let region = t + 2 * net.pre_pad as u64 + net.unshuffle as u64;
    let feat = region.div_ceil(net.unshuffle as u64);
    net.bufs.iter().map(|b| (b.channels.max(4) as u64) * (feat * b.level as u64).pow(2) * 4).max().unwrap_or(0)
}

/// Largest tile whose largest buffer fits the adapter's limits.
pub fn fit_tile(net: &Network, requested: u32, max_storage_binding: u64, max_buffer: u64) -> u32 {
    let cap = max_storage_binding.min(max_buffer);
    let mut t = requested.max(16);
    while max_buffer_bytes(net, t as u64) > cap && t > 16 {
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
    in_scale: u32,
    pad: u32,
    dst_off: u32,
    act: u32,
    has_res: u32,
    _p0: u32,
    slope: f32,
    res_scale: f32,
    _p1: f32,
    _p2: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DeconvParams {
    in_w: u32,
    in_h: u32,
    in_ch: u32,
    out_w: u32,
    out_h: u32,
    _p0: u32,
    _p1: u32,
    _p2: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct AddParams {
    n: u32,
    _p0: u32,
    _p1: u32,
    _p2: u32,
    scale: f32,
    _p3: f32,
    _p4: f32,
    _p5: f32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct PackParams {
    in_w: u32,
    in_h: u32,
    r: u32,
    s: u32,
    u: u32,
    sk_w: u32,
    sk_h: u32,
    add_input: u32,
}

enum Step {
    Conv { op: usize, params: wgpu::Buffer, bind: wgpu::BindGroup },
    Deconv { op: usize, params: wgpu::Buffer, bind: wgpu::BindGroup },
    Copy { src: usize, dst: usize, dst_off: usize, ch: usize },
    Add { dst: usize, ch: usize, scale: f32, params: wgpu::Buffer, bind: wgpu::BindGroup },
}

pub struct WgpuUpscaler {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pub adapter: AdapterReport,
    pub net: Arc<Network>,
    pub tile: u32,
    conv_pipeline: wgpu::ComputePipeline,
    deconv_pipeline: wgpu::ComputePipeline,
    add_pipeline: wgpu::ComputePipeline,
    pack_pipeline: wgpu::ComputePipeline,
    bufs: Vec<wgpu::Buffer>,
    steps: Vec<Step>,
    pack_params: wgpu::Buffer,
    pack_bind: wgpu::BindGroup,
    out: wgpu::Buffer,
    staging: wgpu::Buffer,
    _weights: Vec<wgpu::Buffer>,
}

fn storage(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only }, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }
}

fn uniform(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
        count: None,
    }
}

impl WgpuUpscaler {
    pub fn new(net: Arc<Network>, opts: GpuOptions) -> anyhow::Result<Self> {
        // wgpu 29: `InstanceDescriptor::default()` is gone (display-handle
        // split); this constructor honours WGPU_BACKEND etc.
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
            let msg = format!("adapter is a software rasterizer: {rep}\n  ~100x slower than a GPU. On Asahi this usually means Honeykrisp is missing or shadowed (`vulkaninfo --summary`).");
            if opts.require_hw {
                return Err(anyhow!("{msg}\n  Refusing because --require-hw was given."));
            }
            eprintln!("warning: {msg}");
        }
        // The adapter's own limits are always valid; wgpu's defaults may not
        // be (beatspace died on max_color_attachments this way).
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("dandere2x"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            experimental_features: wgpu::ExperimentalFeatures::default(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        }))
        .context("request_device")?;

        let requested = if opts.tile == 0 { default_tile(&net) } else { opts.tile };
        let tile = fit_tile(&net, requested, rep.max_storage_binding, rep.max_buffer);
        if tile != requested {
            eprintln!("note: tile {requested} does not fit this adapter's buffer limits; using {tile}");
        }
        let region = tile as u64 + 2 * net.pre_pad as u64 + net.unshuffle as u64;
        let feat = region.div_ceil(net.unshuffle as u64);
        let out_side = region * net.scale as u64;
        let max_dim = out_side.div_ceil(16) as u32;
        anyhow::ensure!(max_dim <= rep.max_workgroups_per_dim, "tile {tile} needs {max_dim} workgroups per dimension, limit {}", rep.max_workgroups_per_dim);

        let module = |label: &str, src: &str| device.create_shader_module(wgpu::ShaderModuleDescriptor { label: Some(label), source: wgpu::ShaderSource::Wgsl(src.into()) });
        let conv_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("conv"),
            entries: &[uniform(0), storage(1, false), storage(2, true), storage(3, true), storage(4, false), storage(5, true), storage(6, false)],
        });
        let deconv_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("deconv"),
            entries: &[uniform(0), storage(1, false), storage(2, true), uniform(3), storage(4, false)],
        });
        let add_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("add"), entries: &[uniform(0), storage(1, false), storage(2, false)] });
        let pack_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: Some("pack"), entries: &[uniform(0), storage(1, false), storage(2, false), storage(3, false)] });
        let pipeline = |label: &str, src: &str, layout: &wgpu::BindGroupLayout| {
            // wgpu 29: bind_group_layouts are Option<&_>; push constants became `immediate_size`.
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: Some(label), bind_group_layouts: &[Some(layout)], immediate_size: 0 });
            let m = module(label, src);
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: Some(label), layout: Some(&pl), module: &m, entry_point: Some("main"), compilation_options: Default::default(), cache: None })
        };
        let conv_pipeline = pipeline("conv3x3", CONV_WGSL, &conv_layout);
        let deconv_pipeline = pipeline("deconv4x4s2", DECONV_WGSL, &deconv_layout);
        let add_pipeline = pipeline("add_scaled", ADD_WGSL, &add_layout);
        let pack_pipeline = pipeline("pack", PACK_WGSL, &pack_layout);

        // Activation buffers, one per BufId, sized for the largest tile.
        let bufs: Vec<wgpu::Buffer> = net
            .bufs
            .iter()
            .enumerate()
            .map(|(i, b)| {
                let side = feat * b.level as u64;
                let bytes = (b.channels.max(4) as u64) * side * side * 4;
                device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some(&format!("buf{i}")),
                    size: bytes.max(16),
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                })
            })
            .collect();
        let dummy = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("dummy"), contents: &[0u8; 64], usage: wgpu::BufferUsages::STORAGE });
        let out_bytes = out_side * out_side * 4;
        let out = device.create_buffer(&wgpu::BufferDescriptor { label: Some("out"), size: out_bytes, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
        let staging = device.create_buffer(&wgpu::BufferDescriptor { label: Some("staging"), size: out_bytes, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });

        let mut weights = Vec::new();
        let mut steps = Vec::with_capacity(net.ops.len());
        let uni = |label: &str, size: usize| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: size as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
        for (i, op) in net.ops.iter().enumerate() {
            match op {
                Op::Conv(c) => {
                    anyhow::ensure!(c.dst_off % 4 == 0, "{}: dst_off {} not a multiple of 4", c.name, c.dst_off);
                    let oc4 = c.out_ch.div_ceil(4);
                    let ocp = oc4 * 4; // padded out channels
                    let mut w: Vec<f32> = Vec::with_capacity(oc4 * c.src_ch * 9 * 4);
                    for g in 0..oc4 {
                        for ic in 0..c.src_ch {
                            for k in 0..9 {
                                for j in 0..4 {
                                    let oc = g * 4 + j;
                                    w.push(if oc < c.out_ch { c.weight[(oc * c.src_ch + ic) * 9 + k] } else { 0.0 });
                                }
                            }
                        }
                    }
                    let mut b = vec![0.0f32; ocp];
                    b[..c.out_ch].copy_from_slice(&c.bias);
                    let mut pr = vec![0.0f32; ocp];
                    if c.act == Act::PReLU {
                        pr[..c.out_ch].copy_from_slice(&c.prelu);
                    }
                    let wbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(&c.name), contents: bytemuck::cast_slice(&w), usage: wgpu::BufferUsages::STORAGE });
                    let bbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("bias"), contents: bytemuck::cast_slice(&b), usage: wgpu::BufferUsages::STORAGE });
                    let pbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("prelu"), contents: bytemuck::cast_slice(&pr), usage: wgpu::BufferUsages::STORAGE });
                    let params = uni("conv params", std::mem::size_of::<ConvParams>());
                    let res: &wgpu::Buffer = match c.residual {
                        Some((rb, _)) => &bufs[rb],
                        None => &dummy,
                    };
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some(&c.name),
                        layout: &conv_layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 1, resource: bufs[c.src].as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 2, resource: wbuf.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 3, resource: bbuf.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 4, resource: bufs[c.dst].as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 5, resource: pbuf.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 6, resource: res.as_entire_binding() },
                        ],
                    });
                    weights.push(wbuf);
                    weights.push(bbuf);
                    weights.push(pbuf);
                    steps.push(Step::Conv { op: i, params, bind });
                }
                Op::Deconv(d) => {
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
                    let wbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("dw"), contents: bytemuck::cast_slice(&w), usage: wgpu::BufferUsages::STORAGE });
                    let bbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("db"), contents: bytemuck::cast_slice(&[d.bias[0], d.bias[1], d.bias[2], 0.0f32]), usage: wgpu::BufferUsages::UNIFORM });
                    let params = uni("deconv params", std::mem::size_of::<DeconvParams>());
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("deconv"),
                        layout: &deconv_layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 1, resource: bufs[d.src].as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 2, resource: wbuf.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 3, resource: bbuf.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 4, resource: bufs[d.dst].as_entire_binding() },
                        ],
                    });
                    weights.push(wbuf);
                    weights.push(bbuf);
                    steps.push(Step::Deconv { op: i, params, bind });
                }
                Op::Copy { src, dst, dst_off, ch } => steps.push(Step::Copy { src: *src, dst: *dst, dst_off: *dst_off, ch: *ch }),
                Op::AddScaled { dst, src, ch, scale } => {
                    let params = uni("add params", std::mem::size_of::<AddParams>());
                    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                        label: Some("add"),
                        layout: &add_layout,
                        entries: &[
                            wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 1, resource: bufs[*dst].as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 2, resource: bufs[*src].as_entire_binding() },
                        ],
                    });
                    steps.push(Step::Add { dst: *dst, ch: *ch, scale: *scale, params, bind });
                }
            }
        }
        let pack_params = uni("pack params", std::mem::size_of::<PackParams>());
        let pack_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("pack"),
            layout: &pack_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: pack_params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: bufs[net.output].as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: bufs[net.input].as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: out.as_entire_binding() },
            ],
        });

        Ok(WgpuUpscaler { device, queue, adapter: rep, net, tile, conv_pipeline, deconv_pipeline, add_pipeline, pack_pipeline, bufs, steps, pack_params, pack_bind, out, staging, _weights: weights })
    }

    /// Run the network on one tile's input tensor; returns packed RGBA8 for
    /// the full output region (`scale × region`).
    pub fn run_tile(&mut self, input: &Tensor) -> anyhow::Result<Vec<u8>> {
        let net = self.net.clone();
        anyhow::ensure!(input.c == net.bufs[net.input].channels, "input channels");
        self.queue.write_buffer(&self.bufs[net.input], 0, bytemuck::cast_slice(&input.data));

        // Walk the op list computing each buffer's current spatial size, exactly
        // as the CPU executor does, and encode.
        let mut dims: Vec<Option<(u32, u32)>> = vec![None; net.bufs.len()];
        dims[net.input] = Some((input.h as u32, input.w as u32));
        let mut enc = self.device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("tile") });
        for step in &self.steps {
            match step {
                Step::Conv { op, params, bind } => {
                    let Op::Conv(c) = &net.ops[*op] else { unreachable!() };
                    let (ih, iw) = dims[c.src].ok_or_else(|| anyhow!("{}: unsized source", c.name))?;
                    let (vh, vw) = (ih * c.in_scale, iw * c.in_scale);
                    let (oh, ow) = (vh + 2 * c.pad - 2, vw + 2 * c.pad - 2);
                    dims[c.dst] = Some((oh, ow));
                    let (act, slope) = match c.act {
                        Act::None => (0, 0.0),
                        Act::Leaky(s) => (1, s),
                        Act::PReLU => (2, 0.0),
                    };
                    let (has_res, res_scale) = c.residual.map_or((0, 0.0), |(_, s)| (1, s));
                    self.queue.write_buffer(params, 0, bytemuck::bytes_of(&ConvParams { in_w: iw, in_h: ih, in_ch: c.src_ch as u32, out_ch4: c.out_ch.div_ceil(4) as u32, out_w: ow, out_h: oh, in_scale: c.in_scale, pad: c.pad, dst_off: c.dst_off as u32, act, has_res, _p0: 0, slope, res_scale, _p1: 0.0, _p2: 0.0 }));
                    let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some(&c.name), timestamp_writes: None });
                    pass.set_pipeline(&self.conv_pipeline);
                    pass.set_bind_group(0, bind, &[]);
                    pass.dispatch_workgroups(ow.div_ceil(32), oh.div_ceil(32), c.out_ch.div_ceil(4) as u32);
                }
                Step::Deconv { op, params, bind } => {
                    let Op::Deconv(d) = &net.ops[*op] else { unreachable!() };
                    let (ih, iw) = dims[d.src].ok_or_else(|| anyhow!("deconv: unsized source"))?;
                    let (oh, ow) = (2 * ih - 4, 2 * iw - 4);
                    dims[d.dst] = Some((oh, ow));
                    self.queue.write_buffer(params, 0, bytemuck::bytes_of(&DeconvParams { in_w: iw, in_h: ih, in_ch: d.in_ch as u32, out_w: ow, out_h: oh, _p0: 0, _p1: 0, _p2: 0 }));
                    let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("deconv"), timestamp_writes: None });
                    pass.set_pipeline(&self.deconv_pipeline);
                    pass.set_bind_group(0, bind, &[]);
                    pass.dispatch_workgroups(ow.div_ceil(16), oh.div_ceil(16), 1);
                }
                Step::Copy { src, dst, dst_off, ch } => {
                    let (h, w) = dims[*src].ok_or_else(|| anyhow!("copy: unsized source"))?;
                    if dims[*dst].is_none() {
                        dims[*dst] = Some((h, w));
                    }
                    let plane = (h as u64) * (w as u64) * 4;
                    enc.copy_buffer_to_buffer(&self.bufs[*src], 0, &self.bufs[*dst], (*dst_off as u64) * plane, (*ch as u64) * plane);
                }
                Step::Add { dst, ch, scale, params, bind } => {
                    let (h, w) = dims[*dst].ok_or_else(|| anyhow!("add: unsized dst"))?;
                    let n = (*ch as u32) * h * w;
                    self.queue.write_buffer(params, 0, bytemuck::bytes_of(&AddParams { n, _p0: 0, _p1: 0, _p2: 0, scale: *scale, _p3: 0.0, _p4: 0.0, _p5: 0.0 }));
                    let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("add"), timestamp_writes: None });
                    pass.set_pipeline(&self.add_pipeline);
                    pass.set_bind_group(0, bind, &[]);
                    pass.dispatch_workgroups(n.div_ceil(1024), 1, 1);
                }
            }
        }
        let (oh, ow) = dims[net.output].ok_or_else(|| anyhow!("output unsized"))?;
        let (fh, fw) = (oh * net.shuffle, ow * net.shuffle);
        self.queue.write_buffer(&self.pack_params, 0, bytemuck::bytes_of(&PackParams { in_w: ow, in_h: oh, r: net.shuffle, s: net.scale, u: net.unshuffle, sk_w: input.w as u32, sk_h: input.h as u32, add_input: net.add_input as u32 }));
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("pack"), timestamp_writes: None });
            pass.set_pipeline(&self.pack_pipeline);
            pass.set_bind_group(0, &self.pack_bind, &[]);
            pass.dispatch_workgroups(fw.div_ceil(16), fh.div_ceil(16), 1);
        }
        let out_bytes = (fw as u64) * (fh as u64) * 4;
        enc.copy_buffer_to_buffer(&self.out, 0, &self.staging, 0, out_bytes);
        self.queue.submit(Some(enc.finish()));

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
        let net = self.net.clone();
        let padded = net.pad_frame(frame);
        let mut out = Frame::new(frame.width * net.scale, frame.height * net.scale);
        for rect in tiles(frame.width, frame.height, self.tile) {
            let io: TileIo = net.tile_io(rect);
            let input = net.extract_input(&padded, io);
            let rgba = self.run_tile(&input)?;
            net.write_output_rgba8(&mut out, io, &rgba);
        }
        Ok(out)
    }
}

impl Upscaler for WgpuUpscaler {
    fn scale(&self) -> u32 {
        self.net.scale
    }
    fn upscale(&mut self, frame: &Frame) -> anyhow::Result<Frame> {
        self.upscale_frame(frame)
    }
    fn name(&self) -> String {
        format!("wgpu [{}] x{} on {} tile {}", self.net.name, self.net.scale, self.adapter.name, self.tile)
    }
}

#[derive(Debug, Clone)]
pub struct SelftestReport {
    pub adapter: AdapterReport,
    pub network: String,
    pub frame_px: (u32, u32),
    pub compared: usize,
    pub exact: usize,
    pub worst_abs_diff: u8,
    pub pass: bool,
}

impl std::fmt::Display for SelftestReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        writeln!(f, "adapter: {}", self.adapter)?;
        writeln!(f, "network: {}", self.network)?;
        writeln!(f, "frame {}x{} -> {} output bytes compared", self.frame_px.0, self.frame_px.1, self.compared)?;
        writeln!(f, "exact: {} ({:.1}%), worst |gpu-cpu| = {}", self.exact, 100.0 * self.exact as f64 / self.compared.max(1) as f64, self.worst_abs_diff)?;
        write!(f, "{}", if self.pass { "PASS — within ±1 (f32 reordering)" } else { "FAIL — kernels disagree with the CPU reference" })
    }
}

/// GPU vs CPU on a small frame spanning two tiles with odd dimensions, so
/// edge patches, partial workgroups and the tile seam are all exercised.
pub fn selftest(net: Arc<Network>, opts: GpuOptions) -> anyhow::Result<SelftestReport> {
    let tile = if opts.tile == 0 { 40 } else { opts.tile.min(64) };
    let mut gpu = WgpuUpscaler::new(net.clone(), GpuOptions { tile, ..opts })?;
    let (w, h) = (gpu.tile + 13, 37u32);
    let mut frame = Frame::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let v = |k: u32| ((x * 7 + y * 13 + k * 31) % 251) as u8;
            frame.set_px(x, y, [v(0), v(1), v(2)]);
        }
    }
    let got = gpu.upscale_frame(&frame)?;
    let want = net.upscale_cpu(&frame, gpu.tile);
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
    Ok(SelftestReport { adapter: gpu.adapter.clone(), network: net.name.clone(), frame_px: (w, h), compared: got.data.len(), exact, worst_abs_diff: worst, pass: worst <= 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shader fitness gate: every kernel parses and validates under naga
    /// with no GPU at all.
    #[test]
    fn shaders_validate() {
        for (name, src) in [("conv3x3", CONV_WGSL), ("deconv4x4s2", DECONV_WGSL), ("add_scaled", ADD_WGSL), ("pack", PACK_WGSL)] {
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
        assert!(!adapter_is_software("Intel(R) Arc(TM) A770 Graphics", wgpu::DeviceType::DiscreteGpu));
    }

    fn models_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../models")
    }

    /// upconv_7 with synthetic weights exercises valid convs + deconv; SRVGG
    /// (vendored) exercises same-pad, PReLU, pixel-shuffle and the input
    /// skip. Both skip when no adapter exists (lavapipe counts).
    #[test]
    fn gpu_matches_cpu_if_adapter_present() {
        if probe().is_empty() {
            eprintln!("skipping: no wgpu adapter");
            return;
        }
        let m = d2x_core::waifu2x::Upconv7::synthetic(7);
        let mut json_like = Vec::new();
        json_like.push(m); // keep the model alive; build the network directly
        let net = net_from_upconv7(&json_like[0]);
        let r = selftest(Arc::new(net), GpuOptions { tile: 32, ..Default::default() }).unwrap();
        eprintln!("{r}");
        assert!(r.pass, "{r}");

        let p = models_dir().join("realesr-animevideov3.safetensors");
        if p.exists() {
            let net = Network::load(&p).unwrap();
            let r = selftest(Arc::new(net), GpuOptions { tile: 24, ..Default::default() }).unwrap();
            eprintln!("{r}");
            assert!(r.pass, "{r}");
        }
        // RRDB x2 (67 MB, not vendored): copies, add-scaled, fused residuals,
        // folded x2 upsampling, pixel-unshuffle. Heavy on a software adapter.
        if let Ok(w) = std::env::var("D2X_RRDB_X2") {
            let net = Network::load(std::path::Path::new(&w)).unwrap();
            let r = selftest(Arc::new(net), GpuOptions { tile: 32, ..Default::default() }).unwrap();
            eprintln!("{r}");
            assert!(r.pass, "{r}");
        }
    }

    fn net_from_upconv7(m: &d2x_core::waifu2x::Upconv7) -> Network {
        use d2x_core::net::{BufSpec, ConvOp, DeconvOp};
        let bufs = vec![BufSpec { channels: 3, level: 1 }, BufSpec { channels: 256, level: 1 }, BufSpec { channels: 256, level: 1 }, BufSpec { channels: 3, level: 2 }];
        let mut ops = Vec::new();
        let mut cur = 0;
        for (i, c) in m.convs.iter().enumerate() {
            let dst = if cur == 1 { 2 } else { 1 };
            ops.push(Op::Conv(ConvOp { name: format!("conv{i}"), src: cur, src_ch: c.in_ch, in_scale: 1, dst, dst_off: 0, out_ch: c.out_ch, pad: 0, act: Act::Leaky(0.1), prelu: Vec::new(), residual: None, weight: c.weight.clone(), bias: c.bias.clone() }));
            cur = dst;
        }
        ops.push(Op::Deconv(DeconvOp { src: cur, dst: 3, in_ch: 256, out_ch: 3, weight: m.deconv.weight.clone(), bias: m.deconv.bias.clone() }));
        Network { name: "synthetic upconv7".into(), ops, bufs, input: 0, output: 3, scale: 2, unshuffle: 1, shuffle: 1, add_input: false, pre_pad: 7, exact: true }
    }
}
