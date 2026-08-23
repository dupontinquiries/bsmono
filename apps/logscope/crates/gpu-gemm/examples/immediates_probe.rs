//! immediates_probe — dims through immediate data instead of a uniform buffer.
//!
//! ## Why this exists
//!
//! `lib.rs`'s llama.cpp comparison note lists five things that passing dims as
//! push constants would DELETE rather than shrink: the dims content cache, the
//! ring and its wrap fence, `DIMS_RETAIN_CAP` and the retained-keep list, the
//! `dims_alloc_failed` path and the M4 "Buffer 'dims' invalid" crash it exists
//! to survive, and the `SUBMIT_BY_DIMS_WRAP` attribution category.
//!
//! The note recorded the capability as unverified because wgpu 29 has no
//! `PUSH_CONSTANTS`. It renamed the feature to IMMEDIATES. Three greps then say
//! the pieces exist — `var<immediate>` in naga's WGSL front end,
//! `AddressSpace::Immediate` in its IR, `ComputePass::set_immediates` in wgpu.
//!
//! Three greps are not a working path, which is what this is. One kernel, one
//! dispatch, and a readback that would be wrong if the immediate data never
//! arrived. Measured on this sandbox's software adapter:
//!
//! ```text
//! WGSL: parses with var<immediate>
//! adapter: llvmpipe (LLVM 20.1.2, 256 bits) (Gl)
//!   Features::IMMEDIATES supported: true
//!   adapter max_immediate_size: 256 bytes (dims payload is 16)
//! DISPATCH OK — 256 values correct, dims delivered with NO uniform buffer
//! ```
//!
//! ## What it does and does not settle
//!
//! Settled: the WGSL syntax, the feature request, the pipeline-layout field
//! (`immediate_size`), the pass call, and that a kernel really can read shape
//! this way. That is the whole shape of the migration, in one file, runnable.
//!
//! NOT settled: whether Honeykrisp grants `Features::IMMEDIATES`, and at what
//! `max_immediate_size`. wgpu's own docs say Vulkan is 128–256 bytes against
//! our 16, but this ran on a GL software adapter where immediates are emulated
//! with uniforms — the least demanding possible witness. Run it on the M2 and
//! the first four lines answer that before any migration starts.
//!
//! And not settled: that the object count drops. It must, since the category
//! disappears, but "must" is the word that preceded three wrong things in this
//! session's own notes.
//!
//! Run: `cargo run --example immediates_probe`

const SRC: &str = r#"
struct D { n: u32, add: u32, _p0: u32, _p1: u32 };
var<immediate> d: D;
@group(0) @binding(0) var<storage, read_write> o: array<u32>;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) g: vec3<u32>) {
  let i = g.x;
  if (i >= d.n) { return; }
  // `+ d.add` rather than a bare index: a run that silently received zeroed
  // immediates would still write plausible ascending values, and the readback
  // below would pass. The offset makes absence detectable.
  o[i] = i + d.add;
}
"#;

fn main() {
    match naga::front::wgsl::parse_str(SRC) {
        Ok(_) => println!("WGSL: parses with var<immediate>"),
        Err(e) => { println!("WGSL FAILED:\n{}", e.emit_to_string(SRC)); return }
    }

    let inst = wgpu::Instance::default();
    let adapter = pollster::block_on(inst.request_adapter(&Default::default())).expect("adapter");
    let info = adapter.get_info();
    let have = adapter.features().contains(wgpu::Features::IMMEDIATES);
    let max = adapter.limits().max_immediate_size;
    println!("adapter: {} ({:?})", info.name, info.backend);
    println!("  Features::IMMEDIATES supported: {have}");
    println!("  adapter max_immediate_size: {max} bytes (dims payload is 16)");
    if !have || max < 16 {
        println!("NOT AVAILABLE on this adapter — the WGSL half still validates. This is the \
answer the migration needs from the M2 before it starts.");
        return;
    }

    let mut lim = wgpu::Limits::default();
    lim.max_immediate_size = 16;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_features: wgpu::Features::IMMEDIATES,
        required_limits: lim, ..Default::default() })).expect("device");

    let sh = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: None, source: wgpu::ShaderSource::Wgsl(SRC.into()) });
    let n = 256u32;
    let out = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (n * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false });
    let read = device.create_buffer(&wgpu::BufferDescriptor { label: None, size: (n * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ, mapped_at_creation: false });

    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor { label: None,
        entries: &[wgpu::BindGroupLayoutEntry { binding: 0, visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Storage { read_only: false },
                has_dynamic_offset: false, min_binding_size: None }, count: None }]});
    let bg = device.create_bind_group(&wgpu::BindGroupDescriptor { label: None, layout: &bgl,
        entries: &[wgpu::BindGroupEntry { binding: 0, resource: out.as_entire_binding() }]});
    // `immediate_size` on the LAYOUT is the piece that is easy to miss: the
    // pass call fails validation without it, and the error names the pipeline
    // rather than the layout.
    let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor { label: None,
        bind_group_layouts: &[Some(&bgl)], immediate_size: 16 });
    let pipe = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor { label: None,
        layout: Some(&pl), module: &sh, entry_point: Some("main"),
        compilation_options: Default::default(), cache: None });

    let add = 1000u32;
    let dims: [u32; 4] = [n, add, 0, 0];
    let mut enc = device.create_command_encoder(&Default::default());
    {
        let mut p = enc.begin_compute_pass(&Default::default());
        p.set_pipeline(&pipe);
        p.set_bind_group(0, &bg, &[]);
        p.set_immediates(0, bytemuck::cast_slice(&dims));   // the whole point
        p.dispatch_workgroups(n.div_ceil(64), 1, 1);
    }
    enc.copy_buffer_to_buffer(&out, 0, &read, 0, (n * 4) as u64);
    queue.submit(Some(enc.finish()));
    let sl = read.slice(..);
    sl.map_async(wgpu::MapMode::Read, |_| {});
    device.poll(wgpu::PollType::wait_indefinitely()).ok();
    let data: Vec<u32> = bytemuck::cast_slice(&sl.get_mapped_range()).to_vec();

    let want: Vec<u32> = (0..n).map(|i| i + add).collect();
    if data == want {
        println!("DISPATCH OK — {n} values correct, dims delivered with NO uniform buffer");
    } else {
        println!("MISMATCH: first 4 got {:?} want {:?}", &data[..4], &want[..4]);
        std::process::exit(1);
    }
}
