//! Renderer. Setup and descriptor shapes follow the reference repo's
//! clawbound app.rs (wgpu 22 + winit 0.29): srgb surface, Depth32Float, one
//! render pass drawing sky → scene → HUD. Per-object data goes through a
//! single dynamic-offset uniform buffer (256-byte strides).

use std::sync::Arc;

use wgpu::util::DeviceExt;
use winit::window::Window;

use crate::meshgen::{MeshData, Vertex};

pub const MAX_OBJECTS: usize = 8192;
pub const OBJ_STRIDE: u64 = 256;
pub const MAX_UI_VERTS: usize = 90_000; // pixel-font text is many small quads
pub const SAMPLES: u32 = 4;   // MSAA

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Globals {
    pub view_proj: [f32; 16],
    pub inv_view_proj: [f32; 16],
    pub cam: [f32; 4],
    pub sun: [f32; 4],
    pub fog: [f32; 4],
    pub misc: [f32; 4],
    pub rt: [f32; 4],   // x rays on, y quality, z spare, w ground wetness
    /// x ray distance multiplier, y AO probe count, z spare, w spare
    pub ray2: [f32; 4],
    pub hmap: [f32; 4], // x half extent, y texels
    pub gcol: [f32; 4], // rgb terrain base color, w rt shape count
    pub cine: [f32; 4], // x exposure, y vignette, z grain, w filmic
    pub post: [f32; 4], // x bloom strength, y bloom threshold, zw spare
    /// Up to 4 portals for the ray marcher: per portal, [pos.xyz, radius]
    /// then the face quaternion. `pcount.x` is how many are live.
    pub pcount: [f32; 4],
    pub portals: [[f32; 4]; 16],
}

/// Ray-marchable shape record for the shader (mirrors world::Shape plus the
/// per-frame dynamic proxies: ball, cars, live pad orbs).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RtShapeRaw {
    pub a: [f32; 4], // xyz pos, w bounding radius
    pub b: [f32; 4], // kind data
    pub c: [f32; 4], // inverse rotation quaternion
    pub d: [f32; 4], // rgb albedo, w emissive
    pub e: [f32; 4], // x spec, y kind, zw spare
}

pub const MAX_RT_SHAPES: usize = 512;
/// Baked candidate grid: RC_DIM x RC_DIM cells over the world XZ span, each
/// cell = [count, RC_K shape ids, pad] u32s — the "bake and project" that
/// keeps per-fragment gathers from scanning the whole shape list.
pub const RC_DIM: usize = 40;
/// car reflection SDF atlas: per-model 3D distance slabs stacked on Z,
/// distances byte-encoded as (d + 0.6) / 2.4
pub const CSDF_W: u32 = 36;
pub const CSDF_H: u32 = 24;
/// Z-resolution of ONE model's baked SDF slab. Was 44; cut to 16 to
/// make room for full roster coverage under the hard 3D-texture depth
/// limit (see `CSDF_MAXM`'s own doc, right below, for the limit
/// itself) — the actual trade-off this session's "ungate more
/// vehicles" request runs into: `SLAB * MAXM` is a fixed budget, and
/// raising model COUNT enough to reach the bot roster (48 families,
/// on top of 33 base cars + 28 electric doubles + 14 hovercars = 123
/// total) means each model gets less depth resolution. 16 is a real
/// cut (2.75x less than 44), not a free lunch — flagged here rather
/// than buried in a number, and easy to walk back if it reads as a
/// visible quality loss once someone can actually look at it.
pub const CSDF_SLAB: u32 = 16;
/// How many models can hold a baked collision/reflection SDF at once.
/// Was 12 — a base garage of 33 named cars plus 14 hovercars already
/// exceeds that before a single electric variant or bot is counted,
/// and the bot roster alone (`assets/bots/`) is 48 families. Raised to
/// comfortably cover the current roster with real headroom: the cost
/// is trivial — `CSDF_W * CSDF_H * CSDF_SLAB` bytes per model slot,
/// about 37KB each, so even a generous cap costs single-digit
/// megabytes of GPU memory. `shader.wgsl`'s `CSDF_D` constant MUST
/// equal `CSDF_SLAB as f32 * CSDF_MAXM as f32` exactly — it did not,
/// even at the old cap (384 vs. the true 528), a second, independent
/// bug from this one: the SDF depth-slice normalization has likely
/// been sampling into the wrong model's slab for a while. Both are
/// fixed together here since changing one without the other would
/// make the mismatch worse, not better.
/// How many models can hold a baked collision/reflection SDF at once.
/// Was 12 — a base garage of 33 named cars plus 14 hovercars already
/// exceeds that before a single electric variant or bot is counted,
/// and the bot roster alone (`assets/bots/`) is 48 families: 123
/// models total need a slot to fully "ungate" the `[`/`]` switcher.
///
/// The SDF texture below is a genuine `TextureDimension::D3` (a real
/// volume texture, not a 2D array), and `render.rs` requests
/// `wgpu::Limits::default()` rather than the adapter's native limits
/// — so the effective `max_texture_dimension_3d` is WebGPU's baseline
/// guarantee, 2048, regardless of what the actual GPU could do.
/// `CSDF_SLAB * CSDF_MAXM` is that texture's depth and cannot safely
/// exceed it. Reaching 123 models within that budget meant cutting
/// `CSDF_SLAB` from 44 to 16 (see its own doc) rather than raising
/// this count alone — raising MAXM without shrinking SLAB first would
/// have hit the same wall this comment already found once, at 44
/// models, with cars and hovercars alone still short of the full
/// roster. 126 gives 123 real headroom without touching the 2048
/// ceiling (16*126=2016).
///
/// `shader.wgsl`'s `CSDF_D` constant MUST equal
/// `CSDF_SLAB as f32 * CSDF_MAXM as f32` exactly — it did not, even
/// at the ORIGINAL cap (384 vs. the true 44*12=528), an independent
/// bug fixed in the same pass that first touched this constant: the
/// SDF depth-slice normalization had likely been sampling into the
/// wrong model's slab for a while before either of these changes.
pub const CSDF_MAXM: u32 = 1344;

/// Models per row and column of the atlas grid.
///
/// The atlas was a 3D texture used as a 1D stack: models advanced only depth,
/// so 2048/SLAB = 126 was the whole roster, while width (36) and height (24)
/// sat nearly empty against the SAME 2048 limit on every axis.
///
/// A model now occupies one tile of a TILES x TILES grid, and only the leftover
/// index advances depth. 16 models per depth slab.
///
/// 1,344 holds the ENTIRE shipped library: 221 chassis at their real pose
/// counts is 1,289 slabs, and 1,344 is the next multiple of TILES^2 above it.
///
/// 144 x 96 x 1344 = 18.6 MB of GPU texture. Sized to the library rather than
/// to a round number, because the alternative is a cap that quietly refuses
/// assets the machine could hold — which is exactly what 126 was doing: it
/// loaded 20 of 221 chassis and the other 201 were invisible without a flag.
///
/// The tiling is what makes 18.6 MB reachable at all. Stacked on depth alone
/// this same roster needs 1,289 x 16 = 20,624 depth, ten times the 2048 limit.
///
/// MUST equal `CSDF_TILES` in shader.wgsl. The shader's own comment records
/// that three numbers had to agree here once and two were wrong; this is a
/// fourth, so it is asserted in a test rather than trusted.
pub const CSDF_TILES: u32 = 4;

/// Atlas dimensions, derived rather than written down twice.
pub const fn csdf_extent() -> (u32, u32, u32) {
    (CSDF_W * CSDF_TILES,
     CSDF_H * CSDF_TILES,
     CSDF_SLAB * CSDF_MAXM / (CSDF_TILES * CSDF_TILES))
}

/// How many models the SDF volume can hold on THIS device.
///
/// `CSDF_MAXM` is the compile-time floor: what WebGPU's baseline 2048 depth
/// guarantees, and what every shader constant is sized against. This is the
/// runtime ceiling, read back from the device that was actually created — an
/// adapter reporting 4096 or 8192 doubles or quadruples the roster.
///
/// Read back rather than assumed: a device may grant less than requested, and
/// sizing a texture from a limit that was not honoured is a validation error at
/// creation rather than a smaller roster.
///
/// ## Why this is not simply "raise the constant"
///
/// The depth is `CSDF_SLAB * models`, and SLAB was already cut from 44 to 16 to
/// reach 126 within 2048. Both halves are real: SLAB is the depth resolution of
/// each model's field, and cutting it further trades silhouette accuracy for
/// count. So the count moves with the DEVICE, and the slab stays put.
/// NOT YET USED TO SIZE THE TEXTURE, and that is deliberate.
///
/// `shader.wgsl` hardcodes `CSDF_D = 2016.0` and divides by it to find a
/// model's slab. Sizing the texture from this function without changing that
/// constant would make every sample read the wrong depth — a silhouette
/// assembled from other models' fields, which renders and is wrong.
///
/// The shader comment already records that "three numbers had to agree ... and
/// two of the three were already wrong" once before. Raising capacity means
/// pushing CSDF_D in as a uniform so there is ONE number, not a fourth place to
/// disagree. Until then this reports what the device could hold and the roster
/// stays at CSDF_MAXM.
pub fn sdf_capacity(device_max_3d: u32) -> u32 {
    // TILING. This function predates the atlas becoming a TILES x TILES grid
    // and was never updated: it computed depth-only stacking, as if a model
    // still advanced depth alone. Since tiling, `TILES^2` models share one
    // depth slab, so it under-reported by a factor of sixteen.
    //
    // It went unnoticed because the `.max(CSDF_MAXM)` floor below swallowed
    // the answer on every device — 4096 reported 256, the floor returned
    // 1344, and the number was never wrong in a way anything could see. The
    // doc note that this is "NOT YET USED TO SIZE THE TEXTURE" is why it was
    // a stale number rather than a rendering bug, and also why nothing caught
    // it: an unused answer is never checked against reality.
    let slabs = device_max_3d / CSDF_SLAB;
    let by_device = slabs.saturating_mul(CSDF_TILES * CSDF_TILES);
    // Never below the compile-time floor: shader constants and the staging
    // buffer are sized against CSDF_MAXM, and a device reporting less than
    // baseline would otherwise produce a roster the shader cannot index.
    by_device.max(CSDF_MAXM)
}
pub const RC_K: usize = 28;
pub const RC_STRIDE: usize = 32;
pub const HMAP_SIZE: u32 = 1024;
/// Scene renders into HDR; bloom + tonemap live in the composite pass.
const HDR_FMT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
pub const HMAP_HALF: f32 = 660.0;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ObjectRaw {
    pub model: [f32; 16],
    pub nrm: [f32; 16],
    pub color: [f32; 4],
    pub params: [f32; 4], // x mode, y emissive, z spec, w rt self id + 1
    /// x skin id, y phase, z intensity, w speed (0..1) — the skin's inputs
    pub skin: [f32; 4],
    /// xyz: direction to the ball in the BODY's own frame; w: boost 0..1.
    /// Skins that read these can do things a texture cannot — face the play,
    /// open under power, know how high they are.
    pub skin2: [f32; 4],
    /// xyz: the last contact point in the BODY's frame; w: seconds since.
    /// A finish can then spread rings from where you actually hit something.
    pub skin3: [f32; 4],
    pub _pad: [f32; 12],
}

impl Default for ObjectRaw {
    fn default() -> Self {
        ObjectRaw {
            model: [0.0; 16],
            nrm: [0.0; 16],
            color: [1.0; 4],
            params: [0.0; 4],
            skin: [0.0; 4], skin2: [0.0; 4], skin3: [0.0; 4], _pad: [0.0; 12],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct UiVert {
    pub pos: [f32; 2],
    pub color: [f32; 4],
}

pub struct ObjDraw {
    pub mesh: usize,
    pub tex: Option<usize>,
    pub raw: ObjectRaw,
}

/// Order the frame for the GPU rather than for the code that built it.
///
/// Two things happen here. OPAQUE geometry is grouped by (mesh, texture) so
/// consecutive draws share state and the buffer/bind-group churn collapses —
/// with a draw call per object, that churn was most of the cost. TRANSLUCENT
/// geometry is then drawn afterwards, sorted BACK TO FRONT, which is also
/// the only order in which alpha blending is actually correct: the glass
/// tubes used to composite in whatever order the world happened to store
/// them in.
pub fn order_for_draw(objs: &mut [ObjDraw], eye: [f32; 3]) -> usize {
    let depth = |o: &ObjDraw| -> f32 {
        let m = &o.raw.model;
        let (dx, dy, dz) = (m[12] - eye[0], m[13] - eye[1], m[14] - eye[2]);
        dx * dx + dy * dy + dz * dz
    };
    let solid = |o: &ObjDraw| o.raw.color[3] >= 0.985;
    objs.sort_by(|a, b| {
        match (solid(a), solid(b)) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            (true, true) => a
                .mesh
                .cmp(&b.mesh)
                .then(a.tex.cmp(&b.tex)),
            // translucent: furthest first
            (false, false) => depth(b).total_cmp(&depth(a)),
        }
    });
    objs.iter().filter(|o| solid(o)).count()
}

struct GpuMesh {
    vbuf: wgpu::Buffer,
    ibuf: wgpu::Buffer,
    icount: u32,
}

pub struct Gpu {
    /// Present accounting. The frame counter used to tick once per loop
    /// iteration whether or not anything reached the screen, which reads as
    /// hundreds of FPS while the display updates a few times a second.
    pub presented: u64,
    pub reconfigures: u64,
    pub dropped: u64,
    surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    depth: wgpu::TextureView,
    msaa: wgpu::TextureView,

    sky_pipe: wgpu::RenderPipeline,
    scene_pipe: wgpu::RenderPipeline,
    ui_pipe: wgpu::RenderPipeline,

    globals_buf: wgpu::Buffer,
    globals_bg: wgpu::BindGroup,
    rt_buf: wgpu::Buffer,
    rc_buf: wgpu::Buffer,
    /// scene render resolution factor (1.0 native, 0.5 = quarter the pixels)
    pub render_scale: f32,
    csdf_tex: wgpu::Texture,
    hmap_tex: wgpu::Texture,
    obj_buf: wgpu::Buffer,
    obj_bg: wgpu::BindGroup,
    tex_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    tex_bgs: Vec<wgpu::BindGroup>,
    white_tex: usize,

    ui_buf: wgpu::Buffer,

    meshes: Vec<GpuMesh>,

    // ---- bloom / composite post chain ----
    post_layout: wgpu::BindGroupLayout,
    post_samp: wgpu::Sampler,
    bright_pipe: wgpu::RenderPipeline,
    blur_pipe: wgpu::RenderPipeline,
    comp_pipe: wgpu::RenderPipeline,
    pp_bright: wgpu::Buffer,
    pp_blurh: wgpu::Buffer,
    pp_blurv: wgpu::Buffer,
    pp_comp: wgpu::Buffer,
    hdr: Option<wgpu::TextureView>,
    bloom_a: Option<wgpu::TextureView>,
    bloom_b: Option<wgpu::TextureView>,
    bg_bright: Option<wgpu::BindGroup>,
    bg_blurh: Option<wgpu::BindGroup>,
    bg_blurv: Option<wgpu::BindGroup>,
    bg_comp: Option<wgpu::BindGroup>,
}

impl Gpu {
    pub fn new(window: Arc<Window>) -> Gpu {
        pollster::block_on(Self::new_async(window))
    }

    async fn new_async(window: Arc<Window>) -> Gpu {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::default());
        let surface = instance.create_surface(window.clone()).unwrap();
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .expect("no GPU adapter");
        let (device, queue) = adapter
            .request_device(
                &wgpu::DeviceDescriptor {
                    label: Some("blackstar"),
                    required_features: wgpu::Features::empty(),
                    // ASK FOR THE ADAPTER'S 3D TEXTURE LIMIT, not just its
                    // resolution.
                    //
                    // `using_resolution` raises the 2D dimensions and leaves
                    // max_texture_dimension_3d at WebGPU's baseline 2048 — which
                    // is what caps the SDF volume at CSDF_SLAB * CSDF_MAXM and
                    // gates the model roster at 126. Most desktop and many
                    // mobile adapters report 2048 here too, so this is not a
                    // free win; it is a request for what the device actually
                    // has, and `sdf_capacity` below reads back what was granted
                    // rather than assuming it was honoured.
                    required_limits: {
                        let mut l = wgpu::Limits::default().using_resolution(adapter.limits());
                        l.max_texture_dimension_3d = adapter.limits().max_texture_dimension_3d;
                        l
                    },
                    memory_hints: wgpu::MemoryHints::default(),
                },
                None,
            )
            .await
            .expect("no device");

        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or(caps.formats[0]);
        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: size.width.max(1),
            height: size.height.max(1),
            present_mode: wgpu::PresentMode::Fifo,
            alpha_mode: caps.alpha_modes[0],
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);
        let depth = make_depth(&device, (config.width, config.height));
        let msaa = make_msaa(&device, (config.width, config.height));

        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shader"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        // ---- bind group layouts ----
        let globals_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("globals"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // the ray-marchable shape list
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                // baked terrain heightmap (manual bilinear in the shader, so
                // no float32-filterable feature is required)
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: true }, has_dynamic_offset: false, min_binding_size: None },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true }, view_dimension: wgpu::TextureViewDimension::D3, multisampled: false },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let obj_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object"),
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
        let tex_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("texture"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
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
            ],
        });

        // ---- buffers ----
        let globals_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let rc_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rt-cells"),
            size: (RC_DIM * RC_DIM * RC_STRIDE * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let rt_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("rt-shapes"),
            size: (std::mem::size_of::<RtShapeRaw>() * MAX_RT_SHAPES) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let csdf_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("car-sdf"),
            size: {
                let (w, h, d) = csdf_extent();
                wgpu::Extent3d { width: w, height: h, depth_or_array_layers: d }
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::R8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let csdf_view = csdf_tex.create_view(&wgpu::TextureViewDescriptor { dimension: Some(wgpu::TextureViewDimension::D3), ..Default::default() });
        let csdf_samp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("car-sdf-samp"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let hmap_tex = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("heightmap"),
            size: wgpu::Extent3d { width: HMAP_SIZE, height: HMAP_SIZE, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::R32Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let hmap_view = hmap_tex.create_view(&wgpu::TextureViewDescriptor::default());
        let globals_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("globals"),
            layout: &globals_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: rt_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&hmap_view) },
                wgpu::BindGroupEntry { binding: 3, resource: rc_buf.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&csdf_view) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(&csdf_samp) },
            ],
        });
        let obj_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("objects"),
            size: OBJ_STRIDE * MAX_OBJECTS as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let obj_bg = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("objects"),
            layout: &obj_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &obj_buf,
                    offset: 0,
                    size: wgpu::BufferSize::new(OBJ_STRIDE),
                }),
            }],
        });
        let ui_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ui"),
            size: (std::mem::size_of::<UiVert>() * MAX_UI_VERTS) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("linear"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        // ---- pipelines ----
        let vlayout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x2],
        };
        let ui_vlayout = wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<UiVert>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x2, 1 => Float32x4],
        };

        let depth_write = wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: true,
            depth_compare: wgpu::CompareFunction::Less,
            stencil: Default::default(),
            bias: Default::default(),
        };
        let depth_skip = wgpu::DepthStencilState {
            format: wgpu::TextureFormat::Depth32Float,
            depth_write_enabled: false,
            depth_compare: wgpu::CompareFunction::Always,
            stencil: Default::default(),
            bias: Default::default(),
        };

        let sky_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("sky"),
            bind_group_layouts: &[&globals_layout],
            push_constant_ranges: &[],
        });
        let sky_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("sky"),
            layout: Some(&sky_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_sky",
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_sky",
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FMT,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(depth_skip.clone()),
            multisample: wgpu::MultisampleState { count: SAMPLES, ..Default::default() },
            multiview: None,
            cache: None,
        });

        let scene_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("scene"),
            bind_group_layouts: &[&globals_layout, &obj_layout, &tex_layout],
            push_constant_ranges: &[],
        });
        let scene_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("scene"),
            layout: Some(&scene_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_scene",
                buffers: &[vlayout],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_scene",
                targets: &[Some(wgpu::ColorTargetState {
                    format: HDR_FMT,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                // DOUBLE-SIDED, and it is not a stylistic choice. Every
                // imported car mesh has thousands of open edges — the
                // triangles are present but wound inconsistently, so back
                // face culling HIDES them and the model looks like it has
                // holes punched in it. Measured: 10,694 open edges on
                // VULCAN, 26,253 on ASSAEL. Culling is a saving that only
                // pays out on closed, consistently wound geometry, and this
                // is not that.
                cull_mode: None,
                front_face: wgpu::FrontFace::Ccw,
                ..Default::default()
            },
            depth_stencil: Some(depth_write),
            multisample: wgpu::MultisampleState { count: SAMPLES, ..Default::default() },
            multiview: None,
            cache: None,
        });

        let ui_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ui"),
            bind_group_layouts: &[],
            push_constant_ranges: &[],
        });
        let ui_pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("ui"),
            layout: Some(&ui_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: "vs_ui",
                buffers: &[ui_vlayout],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: "fs_ui",
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview: None,
            cache: None,
        });

        // ---- bloom / composite post chain -------------------------------
        let post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("post"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 5,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 6,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 7,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(16),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let post_pipe_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post"),
            bind_group_layouts: &[&globals_layout, &post_layout],
            push_constant_ranges: &[],
        });
        let mk_post = |entry: &str, fmt: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&post_pipe_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: "vs_full",
                    buffers: &[],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: entry,
                    targets: &[Some(wgpu::ColorTargetState {
                        format: fmt,
                        blend: None,
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview: None,
                cache: None,
            })
        };
        let bright_pipe = mk_post("fs_bright", HDR_FMT);
        let blur_pipe = mk_post("fs_blur", HDR_FMT);
        let comp_pipe = mk_post("fs_composite", format);
        let post_samp = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        let mk_pbuf = || {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("post-params"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let pp_bright = mk_pbuf();
        let pp_blurh = mk_pbuf();
        let pp_blurv = mk_pbuf();
        let pp_comp = mk_pbuf();

        let mut gpu = Gpu {
            presented: 0,
            reconfigures: 0,
            dropped: 0,
            surface,
            device,
            queue,
            config,
            depth,
            msaa,
            sky_pipe,
            scene_pipe,
            ui_pipe,
            globals_buf,
            globals_bg,
            rt_buf,
            rc_buf,
            render_scale: 1.0,
            csdf_tex,
            hmap_tex,
            obj_buf,
            obj_bg,
            tex_layout,
            sampler,
            tex_bgs: Vec::new(),
            white_tex: 0,
            ui_buf,
            meshes: Vec::new(),
            post_layout,
            post_samp,
            bright_pipe,
            blur_pipe,
            comp_pipe,
            pp_bright,
            pp_blurh,
            pp_blurv,
            pp_comp,
            hdr: None,
            bloom_a: None,
            bloom_b: None,
            bg_bright: None,
            bg_blurh: None,
            bg_blurv: None,
            bg_comp: None,
        };
        gpu.white_tex = gpu.add_texture(1, 1, &[255, 255, 255, 255]);
        gpu.rebuild_post();
        gpu
    }

    /// Live vsync switch: Fifo (vsync) vs AutoNoVsync (tear-or-mailbox).
    /// Frames that actually reached the screen, surfaces reconfigured, and
    /// frames abandoned. The first is the only honest thing to call FPS.
    pub fn present_stats(&self) -> (u64, u64, u64) {
        (self.presented, self.reconfigures, self.dropped)
    }

    pub fn set_vsync(&mut self, on: bool) {
        self.config.present_mode = if on {
            wgpu::PresentMode::Fifo
        } else {
            wgpu::PresentMode::AutoNoVsync
        };
        self.surface.configure(&self.device, &self.config);
    }

    pub fn resize(&mut self, w: u32, h: u32) {
        if w == 0 || h == 0 {
            return;
        }
        self.config.width = w;
        self.config.height = h;
        self.surface.configure(&self.device, &self.config);
        let rd = self.render_dims();
        self.depth = make_depth(&self.device, rd);
        self.msaa = make_msaa(&self.device, rd);
        self.rebuild_post();
    }

    /// (Re)create the HDR + bloom targets and their bind groups for the
    /// current size, and refresh the per-pass texel/dir params.
    /// Scene-pass dimensions: the surface size scaled by `render_scale`.
    /// The composite pass samples this with a linear filter, so a half-scale
    /// scene upscales to the native window for free.
    pub fn render_dims(&self) -> (u32, u32) {
        let s = self.render_scale.clamp(0.25, 1.0);
        (
            (((self.config.width.max(1)) as f32 * s) as u32).max(64),
            (((self.config.height.max(1)) as f32 * s) as u32).max(64),
        )
    }

    /// Change the scene render resolution and rebuild every sized target.
    pub fn set_render_scale(&mut self, s: f32) {
        let s = s.clamp(0.25, 1.0);
        if (s - self.render_scale).abs() < 0.01 {
            return;
        }
        self.render_scale = s;
        let rd = self.render_dims();
        self.depth = make_depth(&self.device, rd);
        self.msaa = make_msaa(&self.device, rd);
        self.rebuild_post();
    }

    fn rebuild_post(&mut self) {
        let (w, h) = self.render_dims();
        let (hw, hh) = ((w / 2).max(1), (h / 2).max(1));
        let mk = |dev: &wgpu::Device, w: u32, h: u32| {
            dev.create_texture(&wgpu::TextureDescriptor {
                label: Some("post-target"),
                size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: HDR_FMT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default())
        };
        let hdr = mk(&self.device, w, h);
        let ba = mk(&self.device, hw, hh);
        let bb = mk(&self.device, hw, hh);

        let write = |q: &wgpu::Queue, buf: &wgpu::Buffer, tx: f32, ty: f32, dx: f32, dy: f32| {
            q.write_buffer(buf, 0, bytemuck::cast_slice(&[tx, ty, dx, dy]));
        };
        write(&self.queue, &self.pp_bright, 1.0 / w as f32, 1.0 / h as f32, 0.0, 0.0);
        write(&self.queue, &self.pp_blurh, 1.0 / hw as f32, 1.0 / hh as f32, 1.0, 0.0);
        write(&self.queue, &self.pp_blurv, 1.0 / hw as f32, 1.0 / hh as f32, 0.0, 1.0);
        write(&self.queue, &self.pp_comp, 1.0 / w as f32, 1.0 / h as f32, 0.0, 0.0);

        let bg = |dev: &wgpu::Device, t: &wgpu::TextureView, t2: &wgpu::TextureView, pb: &wgpu::Buffer, samp: &wgpu::Sampler, layout: &wgpu::BindGroupLayout| {
            dev.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("post"),
                layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::TextureView(t) },
                    wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(samp) },
                    wgpu::BindGroupEntry { binding: 7, resource: pb.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::TextureView(t2) },
                ],
            })
        };
        // NOTE: the second texture slot (t_post2) is only read by the
        // composite, but wgpu marks every *bound* view as in-use regardless
        // of what the shader samples — so the dummy fill must never be the
        // pass's own color target (COLOR_TARGET is exclusive). `hdr` is safe
        // in every post pass: it's sampled, never attached.
        self.bg_bright = Some(bg(&self.device, &hdr, &hdr, &self.pp_bright, &self.post_samp, &self.post_layout));
        self.bg_blurh = Some(bg(&self.device, &ba, &hdr, &self.pp_blurh, &self.post_samp, &self.post_layout));
        self.bg_blurv = Some(bg(&self.device, &bb, &hdr, &self.pp_blurv, &self.post_samp, &self.post_layout));
        self.bg_comp = Some(bg(&self.device, &hdr, &ba, &self.pp_comp, &self.post_samp, &self.post_layout));
        self.hdr = Some(hdr);
        self.bloom_a = Some(ba);
        self.bloom_b = Some(bb);
    }

    pub fn aspect(&self) -> f32 {
        self.config.width as f32 / self.config.height.max(1) as f32
    }

    pub fn add_mesh(&mut self, data: &MeshData) -> usize {
        let vbuf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-v"),
            contents: bytemuck::cast_slice(&data.verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ibuf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-i"),
            contents: bytemuck::cast_slice(&data.idx),
            usage: wgpu::BufferUsages::INDEX,
        });
        self.meshes.push(GpuMesh { vbuf, ibuf, icount: data.idx.len() as u32 });
        self.meshes.len() - 1
    }

    /// Upload the baked per-cell static candidate lists.
    /// Upload the car reflection SDF atlas (R8, encoded distances).
    pub fn set_car_sdfs(&self, data: &[u8]) {
        // The atlas is TILED, so the row stride is the full atlas width — not
        // one model's. Uploading with the untiled stride would shear every
        // model across its neighbours' tiles.
        let (aw, ah, ad) = csdf_extent();
        assert_eq!(data.len(), (aw * ah * ad) as usize,
                   "sdf upload is not the atlas size — bake and extent disagree");
        self.queue.write_texture(
            wgpu::ImageCopyTexture { texture: &self.csdf_tex, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            data,
            wgpu::ImageDataLayout { offset: 0, bytes_per_row: Some(aw), rows_per_image: Some(ah) },
            wgpu::Extent3d { width: aw, height: ah, depth_or_array_layers: ad },
        );
    }

    pub fn set_rcells(&self, data: &[u32]) {
        assert_eq!(data.len(), RC_DIM * RC_DIM * RC_STRIDE);
        self.queue.write_buffer(&self.rc_buf, 0, bytemuck::cast_slice(data));
    }

    /// Upload this frame's ray-marchable shape list (truncated at capacity).
    pub fn set_rt_shapes(&self, shapes: &[RtShapeRaw]) {
        let n = shapes.len().min(MAX_RT_SHAPES);
        if n > 0 {
            self.queue.write_buffer(&self.rt_buf, 0, bytemuck::cast_slice(&shapes[..n]));
        }
    }

    /// Upload the baked terrain heightmap (HMAP_SIZE x HMAP_SIZE f32 meters).
    pub fn set_heightmap(&self, data: &[f32]) {
        assert_eq!(data.len(), (HMAP_SIZE * HMAP_SIZE) as usize);
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &self.hmap_tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(data),
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * HMAP_SIZE),
                rows_per_image: Some(HMAP_SIZE),
            },
            wgpu::Extent3d { width: HMAP_SIZE, height: HMAP_SIZE, depth_or_array_layers: 1 },
        );
    }

    /// Swap the geometry in an existing mesh slot (map changes rebuild the
    /// terrain in place so world::M_* indices stay stable).
    pub fn replace_mesh(&mut self, idx: usize, data: &MeshData) {
        let vbuf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-v"),
            contents: bytemuck::cast_slice(&data.verts),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let ibuf = self.device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("mesh-i"),
            contents: bytemuck::cast_slice(&data.idx),
            usage: wgpu::BufferUsages::INDEX,
        });
        self.meshes[idx] = GpuMesh { vbuf, ibuf, icount: data.idx.len() as u32 };
    }

    pub fn add_texture(&mut self, w: u32, h: u32, rgba: &[u8]) -> usize {
        let tex = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("tex"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        self.queue.write_texture(
            wgpu::ImageCopyTexture {
                texture: &tex,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::ImageDataLayout {
                offset: 0,
                bytes_per_row: Some(4 * w),
                rows_per_image: Some(h),
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        );
        let view = tex.create_view(&Default::default());
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tex-bg"),
            layout: &self.tex_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        self.tex_bgs.push(bg);
        self.tex_bgs.len() - 1
    }

    pub fn render(&mut self, globals: &Globals, objs: &[ObjDraw], ui: &[UiVert]) {
        let frame = match self.surface.get_current_texture() {
            Ok(f) => f,
            Err(wgpu::SurfaceError::Lost) | Err(wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.config);
                return;
            }
            Err(_) => return,
        };
        let view = frame.texture.create_view(&Default::default());

        self.queue.write_buffer(&self.globals_buf, 0, bytemuck::bytes_of(globals));

        let n = objs.len().min(MAX_OBJECTS);
        let mut raws: Vec<u8> = vec![0; n * OBJ_STRIDE as usize];
        for (i, o) in objs.iter().take(n).enumerate() {
            let start = i * OBJ_STRIDE as usize;
            raws[start..start + std::mem::size_of::<ObjectRaw>()]
                .copy_from_slice(bytemuck::bytes_of(&o.raw));
        }
        if n > 0 {
            self.queue.write_buffer(&self.obj_buf, 0, &raws);
        }
        let ui_n = ui.len().min(MAX_UI_VERTS);
        if ui_n > 0 {
            self.queue.write_buffer(&self.ui_buf, 0, bytemuck::cast_slice(&ui[..ui_n]));
        }

        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("frame") });
        let hdr = self.hdr.as_ref().expect("post targets");
        {
            // 1) sky + scene into the HDR target (MSAA resolved into it)
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("scene-hdr"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &self.msaa,
                    resolve_target: Some(hdr),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color { r: 0.9, g: 0.8, b: 0.85, a: 1.0 }),
                        store: wgpu::StoreOp::Discard, // resolved into hdr
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });

            rp.set_pipeline(&self.sky_pipe);
            rp.set_bind_group(0, &self.globals_bg, &[]);
            rp.draw(0..3, 0..1);

            rp.set_pipeline(&self.scene_pipe);
            rp.set_bind_group(0, &self.globals_bg, &[]);
            // With the draws ordered by (mesh, texture), the state only
            // changes when it genuinely has to — the vertex and index
            // buffers and the texture bind group are re-bound on a run
            // boundary instead of on every single object.
            let mut cur_mesh = usize::MAX;
            let mut cur_tex = usize::MAX;
            for (i, o) in objs.iter().take(n).enumerate() {
                let mesh = &self.meshes[o.mesh];
                let tex = o.tex.unwrap_or(self.white_tex);
                rp.set_bind_group(1, &self.obj_bg, &[(i as u64 * OBJ_STRIDE) as u32]);
                if tex != cur_tex {
                    rp.set_bind_group(2, &self.tex_bgs[tex], &[]);
                    cur_tex = tex;
                }
                if o.mesh != cur_mesh {
                    rp.set_vertex_buffer(0, mesh.vbuf.slice(..));
                    rp.set_index_buffer(mesh.ibuf.slice(..), wgpu::IndexFormat::Uint32);
                    cur_mesh = o.mesh;
                }
                rp.draw_indexed(0..mesh.icount, 0, 0..1);
            }
        }
        // 2) bloom: bright pass into half-res, then a separable blur
        let post = |enc: &mut wgpu::CommandEncoder, target: &wgpu::TextureView, pipe: &wgpu::RenderPipeline, bgrp: &wgpu::BindGroup, globals: &wgpu::BindGroup| {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("post"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: target,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rp.set_pipeline(pipe);
            rp.set_bind_group(0, globals, &[]);
            rp.set_bind_group(1, bgrp, &[]);
            rp.draw(0..3, 0..1);
        };
        post(&mut enc, self.bloom_a.as_ref().unwrap(), &self.bright_pipe, self.bg_bright.as_ref().unwrap(), &self.globals_bg);
        post(&mut enc, self.bloom_b.as_ref().unwrap(), &self.blur_pipe, self.bg_blurh.as_ref().unwrap(), &self.globals_bg);
        post(&mut enc, self.bloom_a.as_ref().unwrap(), &self.blur_pipe, self.bg_blurv.as_ref().unwrap(), &self.globals_bg);
        // 3) composite (bloom add + exposure + tonemap + vignette + grain), then UI
        {
            let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("composite"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            rp.set_pipeline(&self.comp_pipe);
            rp.set_bind_group(0, &self.globals_bg, &[]);
            rp.set_bind_group(1, self.bg_comp.as_ref().unwrap(), &[]);
            rp.draw(0..3, 0..1);

            if ui_n > 0 {
                rp.set_pipeline(&self.ui_pipe);
                rp.set_vertex_buffer(0, self.ui_buf.slice(..));
                rp.draw(0..ui_n as u32, 0..1);
            }
        }
        self.queue.submit(Some(enc.finish()));
        frame.present();
        // THE COUNTER HAS TO COUNT. This increment was added in the same
        // change that made FPS mean "frames that reached the screen" — and
        // it did not make it into the file, so `presented` stayed at zero
        // and the readout dutifully reported 0 FPS forever. A counter is
        // worth exactly as much as the line that advances it.
        self.presented = self.presented.wrapping_add(1);
    }
}

fn make_depth(device: &wgpu::Device, dims: (u32, u32)) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d { width: dims.0.max(1), height: dims.1.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: SAMPLES,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

fn make_msaa(device: &wgpu::Device, dims: (u32, u32)) -> wgpu::TextureView {
    // (renders at HDR precision; resolved into the hdr target for post)
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("msaa"),
            size: wgpu::Extent3d { width: dims.0.max(1), height: dims.1.max(1), depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: SAMPLES,
            dimension: wgpu::TextureDimension::D2,
            format: HDR_FMT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

#[cfg(test)]
mod atlas_layout_tests {
    use super::*;

    /// The shader's constants must equal the Rust ones.
    ///
    /// shader.wgsl's own comment records that "three numbers had to agree ...
    /// and two of the three were already wrong" — and tiling adds a fourth.
    /// Read out of the .wgsl rather than restated, so the test fails when
    /// either side moves instead of when someone remembers to update it.
    #[test]
    fn the_shader_agrees_with_render_rs() {
        let src = include_str!("shader.wgsl");
        let konst = |name: &str| -> f32 {
            let i = src.find(&format!("const {name}: f32 =")).expect(name);
            let rest = &src[i..];
            let eq = rest.find('=').unwrap();
            let end = rest[eq..].find(';').unwrap() + eq;
            rest[eq + 1..end].trim().parse().expect(name)
        };
        assert_eq!(konst("CSDF_SLAB") as u32, CSDF_SLAB, "slab depth");
        assert_eq!(konst("CSDF_TILES") as u32, CSDF_TILES, "tile grid");
        // CSDF_D is the DEPTH in models, not voxels: SLAB x depth-slabs.
        assert_eq!(konst("CSDF_D") as u32, CSDF_MAXM, "atlas model capacity");
    }

    #[test]
    fn the_atlas_fits_every_axis_limit() {
        // 2048 applies to width and height too, which is the whole reason
        // tiling has room — but it is a limit, not free space.
        let (w, h, d) = csdf_extent();
        assert!(w <= 2048 && h <= 2048 && d <= 2048, "{w}x{h}x{d}");
    }

    #[test]
    fn every_model_lands_in_a_distinct_tile() {
        // The bug tiling could introduce: two models mapping to one tile
        // renders one silhouette in place of another, and looks like a
        // mismatched asset rather than an index collision.
        let mut seen = std::collections::HashSet::new();
        for m in 0..CSDF_MAXM {
            let t = (m % CSDF_TILES,
                     (m / CSDF_TILES) % CSDF_TILES,
                     m / (CSDF_TILES * CSDF_TILES));
            assert!(seen.insert(t), "model {m} collides at {t:?}");
        }
        assert_eq!(seen.len(), CSDF_MAXM as usize);
    }

    #[test]
    fn the_last_model_stays_inside_the_texture() {
        let (w, h, d) = csdf_extent();
        let m = CSDF_MAXM - 1;
        let (tx, ty, tz) = (m % CSDF_TILES,
                            (m / CSDF_TILES) % CSDF_TILES,
                            m / (CSDF_TILES * CSDF_TILES));
        assert!(tx * CSDF_W + CSDF_W <= w);
        assert!(ty * CSDF_H + CSDF_H <= h);
        assert!(tz * CSDF_SLAB + CSDF_SLAB <= d, "depth overrun at model {m}");
    }
}

#[cfg(test)]
mod sdf_capacity_tests {
    use super::*;

    /// How much depth a roster of `models` actually needs, given tiling.
    /// The inverse of `sdf_capacity`, written out so the tests below check a
    /// relationship rather than restating constants.
    fn depth_for(models: u32) -> u32 {
        models * CSDF_SLAB / (CSDF_TILES * CSDF_TILES)
    }

    #[test]
    fn the_baseline_device_holds_the_whole_shipped_library() {
        // 2048 is WebGPU's guaranteed max_texture_dimension_3d. With TILES^2
        // models per slab that is 2048 models, comfortably above CSDF_MAXM —
        // which is the point of tiling: the library stopped being clipped.
        assert_eq!(sdf_capacity(2048), 2048 / CSDF_SLAB * CSDF_TILES * CSDF_TILES);
        assert!(sdf_capacity(2048) >= CSDF_MAXM, "baseline cannot hold the roster");
        assert!(depth_for(CSDF_MAXM) <= 2048, "the shipped library does not fit the guarantee");
    }

    #[test]
    fn a_bigger_device_gets_a_bigger_roster() {
        // The point of asking the adapter: 4096 is common on desktop. These
        // were 256 and 512 — the pre-tiling numbers, sixteen times too small.
        assert_eq!(sdf_capacity(4096), 4096);
        assert_eq!(sdf_capacity(8192), 8192);
        // and it is genuinely monotonic in the device limit
        let mut prev = 0;
        for d in [2048u32, 4096, 8192, 16384] {
            let c = sdf_capacity(d);
            assert!(c > prev, "capacity did not grow at {d}");
            prev = c;
        }

        // AND THE SAME PROPERTY WITHOUT LITERALS, kept from the other side of
        // the c8/c9 split. The concrete 4096/8192 assertions above are the
        // clearer statement, but they are the second set of literals this test
        // has had: the first pair (256, 512) was written when CSDF_MAXM was
        // 126, and when the floor moved to 1344 both of those clamped to it,
        // so the test still passed while no longer demonstrating anything.
        // Sizing off CSDF_MAXM itself cannot go stale that way again.
        let small = (CSDF_MAXM + 200) * CSDF_SLAB;
        let big = (CSDF_MAXM + 400) * CSDF_SLAB;
        let cap_small = sdf_capacity(small);
        let cap_big = sdf_capacity(big);
        assert!(cap_small > CSDF_MAXM, "should exceed the floor once past it: {cap_small}");
        assert!(cap_big > cap_small, "a bigger device should get a bigger roster: {cap_big} vs {cap_small}");
    }

    #[test]
    fn tiling_is_what_makes_the_library_fit() {
        // the claim in CSDF_TILES' own doc comment: stacked on depth alone
        // this roster needs ten times the 2048 limit. worth asserting, since
        // it is the justification for the whole tiled layout.
        let depth_untiled = CSDF_MAXM * CSDF_SLAB;
        assert!(depth_untiled > 2048 * 8, "untiled depth is {depth_untiled}");
        assert!(depth_for(CSDF_MAXM) <= 2048);
    }

    #[test]
    fn a_smaller_device_never_goes_below_the_floor() {
        // Shader constants and the staging buffer are sized against CSDF_MAXM.
        // A device reporting less than baseline would otherwise produce a
        // roster the shader cannot index — a worse failure than a full one.
        assert_eq!(sdf_capacity(1024), CSDF_MAXM);
        assert_eq!(sdf_capacity(0), CSDF_MAXM);
    }

    #[test]
    fn a_reported_capacity_actually_fits_the_device() {
        // A partial slab is a model whose field is truncated mid-depth, which
        // reads as a silhouette with a slice missing rather than as an error.
        // The old form of this test compared against a bare 2016 — the
        // pre-tiling depth — and so could not have caught the tiling bug.
        for d in [2048u32, 3000, 4096, 5000, 8192] {
            let cap = sdf_capacity(d);
            let needed = depth_for(cap);
            // either it fits the device, or it is the floor, which is allowed
            // to exceed a small device on purpose (see sdf_capacity)
            assert!(
                needed <= d || cap == CSDF_MAXM,
                "device {d} was told it can hold {cap} models, which needs {needed} depth"
            );
            assert_eq!(cap % (CSDF_TILES * CSDF_TILES), 0, "{d} gave a partial tile row");
        }
    }
}

