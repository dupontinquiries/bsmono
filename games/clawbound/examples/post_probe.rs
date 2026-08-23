//! post_probe — does the AAA path still lay down a lattice?
//!
//! No display needed: render the post shader to an offscreen target, read the
//! pixels back, and measure. The artifact in the report is a HIGH-FREQUENCY
//! pattern — adjacent output pixels differing when the underlying scene is
//! smooth — so it is directly countable.
//!
//! The scene texture is synthetic and deliberately smooth: a gentle colour
//! gradient with a smooth depth ramp in alpha, standing in for the water and
//! sky where the report shows the artifact worst. A correct post pass over a
//! smooth input produces a smooth output plus the intended 26-level Bayer
//! quantisation; a broken one produces the lattice.
//!
//! Run with the OLD tap (source texel) and the NEW tap (output pixel) and
//! compare. Usage: `cargo run --example post_probe -- [old|new] [scale]`

use wgpu::util::DeviceExt;

const SHADER: &str = include_str!("post_probe.wgsl");

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "new".into());
    let scale: f32 = std::env::args().nth(2).and_then(|s| s.parse().ok()).unwrap_or(1.85);
    let out_w = 320u32;
    let out_h = 180u32;
    let scene_w = ((out_w as f32 * scale) as u32).max(1);
    let scene_h = ((out_h as f32 * scale) as u32).max(1);

    pollster::block_on(run(&mode, out_w, out_h, scene_w, scene_h));
}

async fn run(mode: &str, out_w: u32, out_h: u32, scene_w: u32, scene_h: u32) {
    let inst = wgpu::Instance::default();
    let adapter = inst.request_adapter(&wgpu::RequestAdapterOptions::default()).await
        .expect("no adapter");
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor::default(), None).await.expect("device");
    println!("adapter: {:?}", adapter.get_info().name);

    // ── the synthetic scene: smooth colour, smooth depth in alpha ──
    let mut px = vec![0u8; (scene_w * scene_h * 4) as usize];
    for y in 0..scene_h {
        for x in 0..scene_w {
            let u = x as f32 / scene_w as f32;
            let v = y as f32 / scene_h as f32;
            let i = ((y * scene_w + x) * 4) as usize;
            // water-ish blue-green, varying gently
            px[i] = (40.0 + 60.0 * u) as u8;
            px[i + 1] = (120.0 + 60.0 * v) as u8;
            px[i + 2] = (150.0 + 80.0 * (1.0 - v)) as u8;
            // DEPTH is what the outline filter reads, and a smooth ramp is the
            // WRONG test — it has no detail at source-pixel scale, so a tap of
            // 0.54px and a tap of 1.0px read the same thing and both variants
            // score identically. Measured: they did, 0 jumps for all four
            // combinations, which showed the scene was wrong rather than the
            // shader right.
            //
            // Real geometry is not smooth. A low-poly landscape puts depth
            // DISCONTINUITIES wherever two triangles meet, and after 1.85x
            // supersampling those land on source-pixel boundaries. That is the
            // content the two taps disagree about.
            //
            // So: a base ramp plus sharp steps at irregular intervals, the
            // shape of a receding polygon field. Irregular on purpose — a
            // regular grid would beat against the 4x4 Bayer pattern and
            // measure that instead.
            let bands = (u * 37.0).floor() + (v * 23.0).floor() * 1.7;
            let step = (bands * 13.0) % 1.0;
            px[i + 3] = (30.0 + 150.0 * v + 60.0 * step) as u8;
        }
    }
    let scene = device.create_texture_with_data(
        &queue,
        &wgpu::TextureDescriptor {
            label: Some("scene"), size: wgpu::Extent3d { width: scene_w, height: scene_h, depth_or_array_layers: 1 },
            mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING, view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor, &px);
    let scene_view = scene.create_view(&Default::default());

    let samp = device.create_sampler(&wgpu::SamplerDescriptor {
        mag_filter: wgpu::FilterMode::Linear, min_filter: wgpu::FilterMode::Linear, ..Default::default() });

    // fx = (nightmare, night, time, danger), (texelX, texelY, outW, outH)
    let fx: [f32; 8] = [0.0, 1.0, 0.0, 0.0,
                        1.0 / scene_w as f32, 1.0 / scene_h as f32,
                        out_w as f32, out_h as f32];
    let fxbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("fx"), contents: bytemuck::cast_slice(&fx),
        usage: wgpu::BufferUsages::UNIFORM });

    // The shader, with the tap selected by textual substitution so BOTH
    // variants come from the same source and cannot drift apart.
    let tap = if mode == "old" { "fx.screen.xy" } else { "vec2<f32>(1.0, 1.0) / fx.screen.zw" };
    let src = SHADER.replace("__TAP__", tap);
    let sh = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None, source: wgpu::ShaderSource::Wgsl(src.into()) });

    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None, entries: &[
        wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering), count: None },
        wgpu::BindGroupLayoutEntry { binding: 1, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2, multisampled: false }, count: None },
        wgpu::BindGroupLayoutEntry { binding: 2, visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false, min_binding_size: None }, count: None },
    ]});
    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &bgl, entries: &[
        wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::Sampler(&samp) },
        wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&scene_view) },
        wgpu::BindGroupEntry { binding: 2, resource: fxbuf.as_entire_binding() },
    ]});
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None, bind_group_layouts: &[&bgl], push_constant_ranges: &[] });
    let pipe = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: None, layout: Some(&pl),
        vertex: wgpu::VertexState { module: &sh, entry_point: "vs", buffers: &[], compilation_options: Default::default() },
        fragment: Some(wgpu::FragmentState { module: &sh, entry_point: "fs",
            targets: &[Some(wgpu::TextureFormat::Rgba8Unorm.into())], compilation_options: Default::default() }),
        primitive: Default::default(), depth_stencil: None, multisample: Default::default(), multiview: None,
        cache: None });

    let target = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("out"), size: wgpu::Extent3d { width: out_w, height: out_h, depth_or_array_layers: 1 },
        mip_level_count: 1, sample_count: 1, dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC, view_formats: &[] });
    let tview = target.create_view(&Default::default());

    let bpr = (out_w * 4).div_ceil(256) * 256;
    let readback = device.create_buffer(&wgpu::BufferDescriptor { label: None,
        size: (bpr * out_h) as u64, usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false });

    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut rp = enc.begin_render_pass(&wgpu::RenderPassDescriptor { label: None,
            color_attachments: &[Some(wgpu::RenderPassColorAttachment { view: &tview, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store } })],
            depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None });
        rp.set_pipeline(&pipe); rp.set_bind_group(0, &bg, &[]); rp.draw(0..3, 0..1);
    }
    enc.copy_texture_to_buffer(
        wgpu::ImageCopyTexture { texture: &target, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
        wgpu::ImageCopyBuffer { buffer: &readback, layout: wgpu::ImageDataLayout {
            offset: 0, bytes_per_row: Some(bpr), rows_per_image: Some(out_h) } },
        wgpu::Extent3d { width: out_w, height: out_h, depth_or_array_layers: 1 });
    queue.submit(Some(enc.finish()));

    let slice = readback.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::Maintain::Wait);
    let data = slice.get_mapped_range().to_vec();

    // ── the measurement ──
    //
    // The artifact is INK laid down by the outline filter where there is no
    // edge. The scene's depth is a smooth vertical ramp, so a correct pass
    // draws no outline at all and each row is near-uniform horizontally.
    // Count horizontally-adjacent pixels whose green channel jumps by more
    // than one quantisation step (255/26 ~= 9.8): the Bayer pattern alone
    // moves a pixel by at most one step.
    let at = |x: u32, y: u32| -> i32 { data[(y * bpr + x * 4 + 1) as usize] as i32 };
    let mut jumps = 0u32;
    let mut worst = 0i32;
    let mut total = 0u32;
    for y in 1..out_h {
        for x in 1..out_w {
            // Both axes: the artifact is a lattice, not a horizontal streak,
            // and the outline filter taps in both directions.
            for d in [(at(x, y) - at(x - 1, y)).abs(), (at(x, y) - at(x, y - 1)).abs()] {
                if d > 12 { jumps += 1; }
                worst = worst.max(d);
                total += 1;
            }
        }
    }
    println!("mode={mode} scale={:.2} scene={scene_w}x{scene_h} out={out_w}x{out_h}",
             scene_w as f32 / out_w as f32);
    println!("  horizontal jumps > 12/255: {jumps} of {total}  ({:.2}%)   worst={worst}",
             100.0 * jumps as f32 / total as f32);
}
