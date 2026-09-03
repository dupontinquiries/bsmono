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
pub const CONV_RT4_WGSL: &str = include_str!("../shaders/conv3x3_rt4.wgsl");
pub const CONV_RT1_WGSL: &str = include_str!("../shaders/conv3x3_rt1.wgsl");
pub const CONV_WINO_WGSL: &str = include_str!("../shaders/conv3x3_wino.wgsl");
pub const DECONV_WGSL: &str = include_str!("../shaders/deconv4x4s2.wgsl");
pub const ADD_WGSL: &str = include_str!("../shaders/add_scaled.wgsl");
pub const PACK_WGSL: &str = include_str!("../shaders/pack.wgsl");

#[derive(Debug, Clone, Copy)]
pub struct GpuOptions {
    /// Requested tile side in input pixels (0 = per-model default). Clamped
    /// down to what the adapter's limits and `vram_budget` allow.
    pub tile: u32,
    /// Refuse a software adapter (llvmpipe, SwiftShader, …) instead of warning.
    pub require_hw: bool,
    pub low_power: bool,
    /// Activation sets rotating in flight: tile k+1 uploads and tile k−1 reads
    /// back while tile k computes. 1 = fully synchronous.
    pub inflight: usize,
    /// GPU-memory budget for this upscaler's activation sets (0 = adapter
    /// limits only). On unified memory this is host RAM.
    pub vram_budget: u64,
    /// Conv kernel variant; both are bit-exact against the CPU reference,
    /// they differ only in speed per device. `dandere2x bench` compares.
    pub kernel: ConvKernel,
}

/// Which conv kernel to run. All three are bit-exact against the CPU
/// reference; they trade arithmetic intensity against register pressure,
/// and which wins is a property of the device, not of the network.
///
/// Measured on an M2 Ultra (Honeykrisp, RealESRGAN x2, tile 512):
/// `Rt2` 878 GMAC/s, `Rt4` 671. The 4x4 tile does 2.9x the FMAs per load
/// but needs ~130 live registers, and AGX occupancy falls off a cliff past
/// ~64 — so the extra arithmetic buys nothing and the lost parallelism
/// costs 24%. On llvmpipe (no occupancy limit) `Rt4` wins by 1.46x. Hence
/// the default is the middle one and `bench` picks per machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ConvKernel {
    /// 1 pixel × 4 channels per thread: lowest registers, lowest intensity.
    Rt1,
    /// 2x2 pixels × 4 channels per thread, 16x16 workgroups. Fastest on AGX.
    #[default]
    Rt2,
    /// 4x4 pixels × 4 channels per thread, 8x8 workgroups: ~2.9x the FMAs
    /// per load, ~130 live registers. Fastest where registers are plentiful.
    Rt4,
    /// Winograd F(2x2, 3x3): 16 multiplies per 2x2 output tile instead of
    /// 36. Weights are transformed once at upload (entries in {1, ½}, so
    /// exact in f32); the input and output transforms are adds.
    Wino,
}

impl ConvKernel {
    fn source(self) -> &'static str {
        match self {
            ConvKernel::Rt1 => CONV_RT1_WGSL,
            ConvKernel::Rt2 => CONV_WGSL,
            ConvKernel::Rt4 => CONV_RT4_WGSL,
            ConvKernel::Wino => CONV_WINO_WGSL,
        }
    }

    /// Transformed weight taps per (out-channel group, in-channel).
    fn taps(self) -> usize {
        if self == ConvKernel::Wino { 16 } else { 9 }
    }
    /// Output pixels covered per workgroup along each axis.
    fn wg_span(self) -> u32 {
        match self {
            ConvKernel::Rt1 => 16,
            ConvKernel::Rt2 | ConvKernel::Rt4 => 32,
            ConvKernel::Wino => 32,
        }
    }
    pub const ALL: [ConvKernel; 4] = [ConvKernel::Rt1, ConvKernel::Rt2, ConvKernel::Rt4, ConvKernel::Wino];
}

impl std::str::FromStr for ConvKernel {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "rt1" | "1x1" => Ok(ConvKernel::Rt1),
            "rt2" | "2x2" => Ok(ConvKernel::Rt2),
            "rt4" | "4x4" => Ok(ConvKernel::Rt4),
            "wino" | "winograd" => Ok(ConvKernel::Wino),
            _ => Err(anyhow!("kernel must be rt1|rt2|rt4|wino")),
        }
    }
}

impl Default for GpuOptions {
    fn default() -> Self {
        GpuOptions { tile: 0, require_hw: false, low_power: false, inflight: 2, vram_budget: 0, kernel: ConvKernel::default() }
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

/// Default tile per network when there is no memory budget to size it by:
/// big for the deep same-pad nets (seams shrink with tile size and the body
/// is the cost either way), exact for valid nets.
pub fn default_tile(net: &Network) -> u32 {
    if net.exact {
        240
    } else if net.unshuffle >= 2 {
        400
    } else {
        256
    }
}

/// With a budget, take the largest tile it allows up to `max` (bigger tiles
/// mean fewer, fuller dispatches and less padding overhead) — a body conv on
/// a 400-px tile is ~1.3 waves on a 76-core GPU, most of a wave wasted.
pub const MAX_AUTO_TILE: u32 = 1024;

/// Bytes of the largest buffer the network needs for a tile of side `t`.
pub fn max_buffer_bytes(net: &Network, t: u64) -> u64 {
    let region = t + 2 * net.pre_pad as u64 + net.unshuffle as u64;
    let feat = region.div_ceil(net.unshuffle as u64);
    net.bufs.iter().map(|b| (b.channels.max(4) as u64) * (feat * b.level as u64).pow(2) * 4).max().unwrap_or(0)
}

/// Total bytes of one activation set (all buffers, output, staging) for a tile of side `t`.
pub fn set_bytes(net: &Network, t: u64) -> u64 {
    let region = t + 2 * net.pre_pad as u64 + net.unshuffle as u64;
    let feat = region.div_ceil(net.unshuffle as u64);
    let act: u64 = net.bufs.iter().map(|b| (b.channels.max(4) as u64) * (feat * b.level as u64).pow(2) * 4).sum();
    let out = (region * net.scale as u64).pow(2) * 4;
    act + 2 * out
}

/// Largest tile whose largest buffer fits the adapter's limits and whose
/// whole set fits `set_budget` (0 = no budget).
pub fn fit_tile(net: &Network, requested: u32, max_storage_binding: u64, max_buffer: u64) -> u32 {
    fit_tile_budget(net, requested, max_storage_binding, max_buffer, 0)
}

pub fn fit_tile_budget(net: &Network, requested: u32, max_storage_binding: u64, max_buffer: u64, set_budget: u64) -> u32 {
    let cap = max_storage_binding.min(max_buffer);
    let mut t = requested.max(16);
    while t > 16 && (max_buffer_bytes(net, t as u64) > cap || (set_budget > 0 && set_bytes(net, t as u64) > set_budget)) {
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

/// Weight buffers of one op, shared by every activation set.
enum OpWeights {
    Conv { w: wgpu::Buffer, b: wgpu::Buffer, prelu: wgpu::Buffer },
    Deconv { w: wgpu::Buffer, b: wgpu::Buffer },
    None,
}

/// One set of activation buffers + bind groups + readback. Sets rotate so
/// several tiles are in flight.
struct ActSet {
    bufs: Vec<wgpu::Buffer>,
    steps: Vec<Step>,
    pack_params: wgpu::Buffer,
    pack_bind: wgpu::BindGroup,
    out: wgpu::Buffer,
    staging: wgpu::Buffer,
}

/// Device + queue shared by every upscaler tier of a job.
pub struct GpuContext {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub adapter: AdapterReport,
}

impl GpuContext {
    pub fn new(opts: &GpuOptions) -> anyhow::Result<Arc<GpuContext>> {
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
        Ok(Arc::new(GpuContext { device, queue, adapter: rep }))
    }
}

pub struct WgpuUpscaler {
    ctx: Arc<GpuContext>,
    pub net: Arc<Network>,
    pub tile: u32,
    pub inflight: usize,
    pub kernel: ConvKernel,
    conv_pipeline: wgpu::ComputePipeline,
    deconv_pipeline: wgpu::ComputePipeline,
    add_pipeline: wgpu::ComputePipeline,
    pack_pipeline: wgpu::ComputePipeline,
    sets: Vec<ActSet>,
    _weights: Vec<OpWeights>,
    /// Bytes of GPU memory held by the activation sets.
    pub set_bytes_total: u64,
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
        let ctx = GpuContext::new(&opts)?;
        Self::with_context(ctx, net, opts)
    }

    pub fn adapter(&self) -> &AdapterReport {
        &self.ctx.adapter
    }

    pub fn with_context(ctx: Arc<GpuContext>, net: Arc<Network>, opts: GpuOptions) -> anyhow::Result<Self> {
        let device = &ctx.device;
        let rep = &ctx.adapter;
        let inflight = opts.inflight.clamp(1, 8);
        let per_set = if opts.vram_budget > 0 { opts.vram_budget / inflight as u64 } else { 0 };
        let requested = if opts.tile != 0 {
            opts.tile
        } else if per_set > 0 && !net.exact {
            MAX_AUTO_TILE
        } else {
            default_tile(&net)
        };
        let tile = fit_tile_budget(&net, requested, rep.max_storage_binding, rep.max_buffer, per_set);
        if tile != requested {
            eprintln!("note: tile {requested} does not fit this adapter's limits / the VRAM budget with {inflight} sets in flight; using {tile}");
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
        let conv_pipeline = pipeline("conv3x3", opts.kernel.source(), &conv_layout);
        let deconv_pipeline = pipeline("deconv4x4s2", DECONV_WGSL, &deconv_layout);
        let add_pipeline = pipeline("add_scaled", ADD_WGSL, &add_layout);
        let pack_pipeline = pipeline("pack", PACK_WGSL, &pack_layout);

        // Weights once, in the kernels' layouts.
        let mut weights: Vec<OpWeights> = Vec::with_capacity(net.ops.len());
        for (i, op) in net.ops.iter().enumerate() {
            weights.push(match op {
                Op::Conv(c) => {
                    anyhow::ensure!(c.dst_off % 4 == 0, "{}: dst_off {} not a multiple of 4", c.name, c.dst_off);
                    let oc4 = c.out_ch.div_ceil(4);
                    let ocp = oc4 * 4;
                    let taps = opts.kernel.taps();
                    let mut w: Vec<f32> = Vec::with_capacity(oc4 * c.src_ch * taps * 4);
                    for g in 0..oc4 {
                        for ic in 0..c.src_ch {
                            // Per output channel lane: the 9 raw taps, or the
                            // 16 Winograd taps G g Gᵀ (G entries 1 and ½, so
                            // the transform is exact in f32).
                            let mut lane = [[0.0f32; 16]; 4];
                            for (j, l) in lane.iter_mut().enumerate() {
                                let oc = g * 4 + j;
                                if oc >= c.out_ch {
                                    continue;
                                }
                                let gk = &c.weight[(oc * c.src_ch + ic) * 9..(oc * c.src_ch + ic) * 9 + 9];
                                if taps == 9 {
                                    l[..9].copy_from_slice(gk);
                                } else {
                                    const GM: [[f32; 3]; 4] = [[1.0, 0.0, 0.0], [0.5, 0.5, 0.5], [0.5, -0.5, 0.5], [0.0, 0.0, 1.0]];
                                    // tmp = G g  (4x3), then out = tmp Gᵀ (4x4)
                                    let mut tmp = [[0.0f32; 3]; 4];
                                    for (r, t) in tmp.iter_mut().enumerate() {
                                        for cc in 0..3 {
                                            t[cc] = (0..3).map(|k| GM[r][k] * gk[k * 3 + cc]).sum();
                                        }
                                    }
                                    for r in 0..4 {
                                        for cc in 0..4 {
                                            l[r * 4 + cc] = (0..3).map(|k| tmp[r][k] * GM[cc][k]).sum();
                                        }
                                    }
                                }
                            }
                            for k in 0..taps {
                                for l in lane.iter() {
                                    w.push(l[k]);
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
                    OpWeights::Conv {
                        w: device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(&c.name), contents: bytemuck::cast_slice(&w), usage: wgpu::BufferUsages::STORAGE }),
                        b: device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("bias"), contents: bytemuck::cast_slice(&b), usage: wgpu::BufferUsages::STORAGE }),
                        prelu: device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("prelu"), contents: bytemuck::cast_slice(&pr), usage: wgpu::BufferUsages::STORAGE }),
                    }
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
                    OpWeights::Deconv {
                        w: device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("dw"), contents: bytemuck::cast_slice(&w), usage: wgpu::BufferUsages::STORAGE }),
                        b: device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("db"), contents: bytemuck::cast_slice(&[d.bias[0], d.bias[1], d.bias[2], 0.0f32]), usage: wgpu::BufferUsages::UNIFORM }),
                    }
                }
                _ => {
                    let _ = i;
                    OpWeights::None
                }
            });
        }
        let dummy = device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some("dummy"), contents: &[0u8; 64], usage: wgpu::BufferUsages::STORAGE });
        let uni = |label: &str, size: usize| device.create_buffer(&wgpu::BufferDescriptor { label: Some(label), size: size as u64, usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });

        // Activation sets.
        let mut sets = Vec::with_capacity(inflight);
        for si in 0..inflight {
            let bufs: Vec<wgpu::Buffer> = net
                .bufs
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    let side = feat * b.level as u64;
                    let bytes = (b.channels.max(4) as u64) * side * side * 4;
                    device.create_buffer(&wgpu::BufferDescriptor { label: Some(&format!("s{si}b{i}")), size: bytes.max(16), usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false })
                })
                .collect();
            let out_bytes = out_side * out_side * 4;
            let out = device.create_buffer(&wgpu::BufferDescriptor { label: Some("out"), size: out_bytes, usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
            let staging = device.create_buffer(&wgpu::BufferDescriptor { label: Some("staging"), size: out_bytes, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false });
            let mut steps = Vec::with_capacity(net.ops.len());
            for (i, op) in net.ops.iter().enumerate() {
                match (op, &weights[i]) {
                    (Op::Conv(c), OpWeights::Conv { w, b, prelu }) => {
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
                                wgpu::BindGroupEntry { binding: 2, resource: w.as_entire_binding() },
                                wgpu::BindGroupEntry { binding: 3, resource: b.as_entire_binding() },
                                wgpu::BindGroupEntry { binding: 4, resource: bufs[c.dst].as_entire_binding() },
                                wgpu::BindGroupEntry { binding: 5, resource: prelu.as_entire_binding() },
                                wgpu::BindGroupEntry { binding: 6, resource: res.as_entire_binding() },
                            ],
                        });
                        steps.push(Step::Conv { op: i, params, bind });
                    }
                    (Op::Deconv(d), OpWeights::Deconv { w, b }) => {
                        let params = uni("deconv params", std::mem::size_of::<DeconvParams>());
                        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: Some("deconv"),
                            layout: &deconv_layout,
                            entries: &[
                                wgpu::BindGroupEntry { binding: 0, resource: params.as_entire_binding() },
                                wgpu::BindGroupEntry { binding: 1, resource: bufs[d.src].as_entire_binding() },
                                wgpu::BindGroupEntry { binding: 2, resource: w.as_entire_binding() },
                                wgpu::BindGroupEntry { binding: 3, resource: b.as_entire_binding() },
                                wgpu::BindGroupEntry { binding: 4, resource: bufs[d.dst].as_entire_binding() },
                            ],
                        });
                        steps.push(Step::Deconv { op: i, params, bind });
                    }
                    (Op::Copy { src, dst, dst_off, ch }, _) => steps.push(Step::Copy { src: *src, dst: *dst, dst_off: *dst_off, ch: *ch }),
                    (Op::AddScaled { dst, src, ch, scale }, _) => {
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
                    _ => anyhow::bail!("op {i}: weights/op mismatch"),
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
            sets.push(ActSet { bufs, steps, pack_params, pack_bind, out, staging });
        }
        let set_bytes_total = set_bytes(&net, tile as u64) * inflight as u64;
        Ok(WgpuUpscaler { ctx, net, tile, inflight, kernel: opts.kernel, conv_pipeline, deconv_pipeline, add_pipeline, pack_pipeline, sets, _weights: weights, set_bytes_total })
    }

    /// Encode and submit one tile on activation set `si`; returns the
    /// submission to wait for and the packed output byte count.
    fn submit_tile(&self, si: usize, input: &Tensor) -> anyhow::Result<(wgpu::SubmissionIndex, u64)> {
        let net = &self.net;
        let set = &self.sets[si];
        let (device, queue) = (&self.ctx.device, &self.ctx.queue);
        anyhow::ensure!(input.c == net.bufs[net.input].channels, "input channels");
        queue.write_buffer(&set.bufs[net.input], 0, bytemuck::cast_slice(&input.data));

        let mut dims: Vec<Option<(u32, u32)>> = vec![None; net.bufs.len()];
        dims[net.input] = Some((input.h as u32, input.w as u32));
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("tile") });
        for step in &set.steps {
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
                    queue.write_buffer(params, 0, bytemuck::bytes_of(&ConvParams { in_w: iw, in_h: ih, in_ch: c.src_ch as u32, out_ch4: c.out_ch.div_ceil(4) as u32, out_w: ow, out_h: oh, in_scale: c.in_scale, pad: c.pad, dst_off: c.dst_off as u32, act, has_res, _p0: 0, slope, res_scale, _p1: 0.0, _p2: 0.0 }));
                    let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some(&c.name), timestamp_writes: None });
                    pass.set_pipeline(&self.conv_pipeline);
                    pass.set_bind_group(0, bind, &[]);
                    let span = self.kernel.wg_span();
                    pass.dispatch_workgroups(ow.div_ceil(span), oh.div_ceil(span), c.out_ch.div_ceil(4) as u32);
                }
                Step::Deconv { op, params, bind } => {
                    let Op::Deconv(d) = &net.ops[*op] else { unreachable!() };
                    let (ih, iw) = dims[d.src].ok_or_else(|| anyhow!("deconv: unsized source"))?;
                    let (oh, ow) = (2 * ih - 4, 2 * iw - 4);
                    dims[d.dst] = Some((oh, ow));
                    queue.write_buffer(params, 0, bytemuck::bytes_of(&DeconvParams { in_w: iw, in_h: ih, in_ch: d.in_ch as u32, out_w: ow, out_h: oh, _p0: 0, _p1: 0, _p2: 0 }));
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
                    enc.copy_buffer_to_buffer(&set.bufs[*src], 0, &set.bufs[*dst], (*dst_off as u64) * plane, (*ch as u64) * plane);
                }
                Step::Add { dst, ch, scale, params, bind } => {
                    let (h, w) = dims[*dst].ok_or_else(|| anyhow!("add: unsized dst"))?;
                    let n = (*ch as u32) * h * w;
                    queue.write_buffer(params, 0, bytemuck::bytes_of(&AddParams { n, _p0: 0, _p1: 0, _p2: 0, scale: *scale, _p3: 0.0, _p4: 0.0, _p5: 0.0 }));
                    let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("add"), timestamp_writes: None });
                    pass.set_pipeline(&self.add_pipeline);
                    pass.set_bind_group(0, bind, &[]);
                    pass.dispatch_workgroups(n.div_ceil(1024), 1, 1);
                }
            }
        }
        let (oh, ow) = dims[net.output].ok_or_else(|| anyhow!("output unsized"))?;
        let (fh, fw) = (oh * net.shuffle, ow * net.shuffle);
        queue.write_buffer(&set.pack_params, 0, bytemuck::bytes_of(&PackParams { in_w: ow, in_h: oh, r: net.shuffle, s: net.scale, u: net.unshuffle, sk_w: input.w as u32, sk_h: input.h as u32, add_input: net.add_input as u32 }));
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("pack"), timestamp_writes: None });
            pass.set_pipeline(&self.pack_pipeline);
            pass.set_bind_group(0, &set.pack_bind, &[]);
            pass.dispatch_workgroups(fw.div_ceil(16), fh.div_ceil(16), 1);
        }
        let out_bytes = (fw as u64) * (fh as u64) * 4;
        enc.copy_buffer_to_buffer(&set.out, 0, &set.staging, 0, out_bytes);
        let idx = queue.submit(Some(enc.finish()));
        Ok((idx, out_bytes))
    }

    /// Wait for a submitted tile and read its packed output.
    fn finish_tile(&self, si: usize, idx: wgpu::SubmissionIndex, out_bytes: u64) -> anyhow::Result<Vec<u8>> {
        let set = &self.sets[si];
        let slice = set.staging.slice(..out_bytes);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        // Wait for *this* submission only, so later tiles keep computing.
        self.ctx.device.poll(wgpu::PollType::Wait { submission_index: Some(idx), timeout: None }).context("device poll")?;
        let mapped = match rx.try_recv() {
            Ok(r) => r,
            Err(_) => {
                // The map callback fires on a poll that observes the copy; make sure of it.
                self.ctx.device.poll(wgpu::PollType::wait_indefinitely()).context("device poll")?;
                rx.recv().context("map_async dropped")?
            }
        };
        mapped.context("map_async")?;
        let bytes = slice.get_mapped_range().to_vec();
        set.staging.unmap();
        Ok(bytes)
    }

    /// Run the network on one tile's input tensor synchronously; returns
    /// packed RGBA8 for the full output region (`scale × region`).
    pub fn run_tile(&mut self, input: &Tensor) -> anyhow::Result<Vec<u8>> {
        let (idx, n) = self.submit_tile(0, input)?;
        self.finish_tile(0, idx, n)
    }

    /// Tiles rotate through the activation sets: with `inflight` sets, the
    /// upload of tile k+1 and the readback of tile k−1 overlap the compute
    /// of tile k.
    pub fn upscale_frame(&mut self, frame: &Frame) -> anyhow::Result<Frame> {
        let net = self.net.clone();
        let padded = net.pad_frame(frame);
        let mut out = Frame::new(frame.width * net.scale, frame.height * net.scale);
        let mut pending: std::collections::VecDeque<(usize, wgpu::SubmissionIndex, TileIo, u64)> = std::collections::VecDeque::new();
        for (k, rect) in tiles(frame.width, frame.height, self.tile).into_iter().enumerate() {
            let si = k % self.inflight;
            if pending.len() == self.inflight {
                let (s, idx, io, n) = pending.pop_front().unwrap();
                let rgba = self.finish_tile(s, idx, n)?;
                net.write_output_rgba8(&mut out, io, &rgba);
            }
            let io: TileIo = net.tile_io(rect);
            let input = net.extract_input(&padded, io);
            let (idx, n) = self.submit_tile(si, &input)?;
            pending.push_back((si, idx, io, n));
        }
        while let Some((s, idx, io, n)) = pending.pop_front() {
            let rgba = self.finish_tile(s, idx, n)?;
            net.write_output_rgba8(&mut out, io, &rgba);
        }
        Ok(out)
    }
}

impl Upscaler for WgpuUpscaler {
    fn as_dyn(&mut self) -> &mut dyn Upscaler {
        self
    }
    fn scale(&self) -> u32 {
        self.net.scale
    }
    /// Pack small inputs up to one tile: one tile is one dispatch's worth of
    /// work, and a residual of a few cells on its own leaves most of the
    /// device idle.
    fn canvas(&self) -> u32 {
        self.tile
    }
    fn upscale(&mut self, frame: &Frame) -> anyhow::Result<Frame> {
        self.upscale_frame(frame)
    }
    fn name(&self) -> String {
        format!("wgpu [{}] x{} on {} tile {} x{} in flight ({} MiB) kernel {:?}", self.net.name, self.net.scale, self.ctx.adapter.name, self.tile, self.inflight, self.set_bytes_total >> 20, self.kernel)
    }
}

#[derive(Debug, Clone)]
pub struct BenchReport {
    pub adapter: String,
    pub network: String,
    pub kernel: ConvKernel,
    pub tile: u32,
    pub region_px: u64,
    pub ms_per_tile: f64,
    pub gmac_per_s: f64,
    /// Full-frame time at 1080p if every pixel went through the network.
    pub s_per_1080p_frame: f64,
}

impl std::fmt::Display for BenchReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} | {} | kernel {:?} | tile {} ({} px region) | {:.1} ms/tile | {:.1} GMAC/s ({:.2} TFLOPS) | ≈ {:.2} s per full 1080p frame",
            self.adapter, self.network, self.kernel, self.tile, self.region_px, self.ms_per_tile, self.gmac_per_s, self.gmac_per_s * 2.0 / 1000.0, self.s_per_1080p_frame
        )
    }
}

/// Time the network on full-size tiles: one warm-up, then `iters` tiles
/// with the sets in flight, waiting on the last. The number to hunt with.
pub fn bench(net: Arc<Network>, opts: GpuOptions, iters: u32) -> anyhow::Result<BenchReport> {
    let mut gpu = WgpuUpscaler::new(net.clone(), opts)?;
    let side = gpu.tile;
    let mut frame = Frame::new(side, side);
    for (i, b) in frame.data.iter_mut().enumerate() {
        *b = ((i * 31) % 251) as u8;
    }
    let padded = net.pad_frame(&frame);
    let io = net.tile_io(d2x_core::net::TileRect { x: 0, y: 0, w: side, h: side });
    let input = net.extract_input(&padded, io);
    let _ = gpu.run_tile(&input)?; // warm-up: pipeline compile, first allocation
    let t = std::time::Instant::now();
    let mut pending = std::collections::VecDeque::new();
    for k in 0..iters {
        let si = (k as usize) % gpu.inflight;
        if pending.len() == gpu.inflight {
            let (s, idx, n) = pending.pop_front().unwrap();
            let _ = gpu.finish_tile(s, idx, n)?;
        }
        let (idx, n) = gpu.submit_tile(si, &input)?;
        pending.push_back((si, idx, n));
    }
    while let Some((s, idx, n)) = pending.pop_front() {
        let _ = gpu.finish_tile(s, idx, n)?;
    }
    let per = t.elapsed().as_secs_f64() / iters as f64;
    let region_px = (io.region_w as u64) * (io.region_h as u64);
    let macs = net.macs_per_input_pixel() * region_px as f64;
    let gmac_per_s = macs / per / 1e9;
    let s_per_1080p_frame = net.macs_per_input_pixel() * 1920.0 * 1080.0 / (gmac_per_s * 1e9);
    Ok(BenchReport { adapter: gpu.adapter().name.clone(), network: net.name.clone(), kernel: gpu.kernel, tile: side, region_px, ms_per_tile: per * 1e3, gmac_per_s, s_per_1080p_frame })
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
    // Three sets in flight and a frame of five tiles, so set rotation and
    // per-submission waits are exercised, not just the kernels.
    let mut gpu = WgpuUpscaler::new(net.clone(), GpuOptions { tile, inflight: 3, ..opts })?;
    let (w, h) = (gpu.tile * 2 + 13, gpu.tile + 5);
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
    Ok(SelftestReport { adapter: gpu.adapter().clone(), network: net.name.clone(), frame_px: (w, h), compared: got.data.len(), exact, worst_abs_diff: worst, pass: worst <= 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The shader fitness gate: every kernel parses and validates under naga
    /// with no GPU at all.
    #[test]
    fn shaders_validate() {
        for (name, src) in [("conv3x3", CONV_WGSL), ("conv3x3_rt1", CONV_RT1_WGSL), ("conv3x3_rt4", CONV_RT4_WGSL), ("conv3x3_wino", CONV_WINO_WGSL), ("deconv4x4s2", DECONV_WGSL), ("add_scaled", ADD_WGSL), ("pack", PACK_WGSL)] {
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
        let net = Arc::new(net_from_upconv7(&m));
        let p = models_dir().join("realesr-animevideov3.safetensors");
        for kernel in ConvKernel::ALL {
            let r = selftest(net.clone(), GpuOptions { tile: 32, kernel, ..Default::default() }).unwrap();
            eprintln!("{r}");
            assert!(r.pass, "{kernel:?}: {r}");
            if p.exists() {
                let net = Network::load(&p).unwrap();
                let r = selftest(Arc::new(net), GpuOptions { tile: 24, kernel, ..Default::default() }).unwrap();
                eprintln!("{r}");
                assert!(r.pass, "{kernel:?}: {r}");
            }
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
