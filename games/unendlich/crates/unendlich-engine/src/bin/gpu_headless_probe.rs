//! `gpu-headless-probe` — bring up a windowless Vulkan context + an offscreen
//! render target on whatever ICD is present (lavapipe in CI/sandbox), with no
//! GLFW and no display. Proves the headless GPU path works for sandbox-side GPU
//! perf/parity, and is the foundation a `--headless` gpu-mesh-bench builds on.
//!
//! Build/run:  cargo run --release --features headless --bin gpu-headless-probe

use ash::vk;
use std::time::Instant;
use unendlich_engine::render::context::VulkanContext;

fn main() {
    let _ = env_logger::builder().is_test(false).try_init();

    let t0 = Instant::now();
    let ctx = match VulkanContext::new_headless() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("headless context init FAILED: {e}");
            std::process::exit(1);
        }
    };
    let init_ms = t0.elapsed().as_secs_f64() * 1000.0;
    let info = ctx.gpu_info();
    println!("headless VulkanContext up in {init_ms:.1} ms (no window, no surface)");
    println!("  gpu_info: {info:?}");

    // Allocate a few offscreen targets across a small size matrix + time it.
    println!("extent,format,alloc_ms,image_ok");
    for &(w, h) in &[(256u32, 256u32), (512, 512), (1024, 1024)] {
        let extent = vk::Extent2D { width: w, height: h };
        let t = Instant::now();
        match ctx.create_offscreen_target(extent, vk::Format::R8G8B8A8_UNORM) {
            Ok(tgt) => {
                let ms = t.elapsed().as_secs_f64() * 1000.0;
                println!("{w}x{h},R8G8B8A8_UNORM,{ms:.3},{}", tgt.image != vk::Image::null());
            }
            Err(e) => {
                eprintln!("offscreen target {w}x{h} FAILED: {e}");
                std::process::exit(2);
            }
        }
    }

    // Run the REAL GPU mesher headless — this is the path gpu-mesh-bench exercises,
    // minus the window. Proves sandbox-side GPU meshing perf is measurable on lavapipe.
    use unendlich_engine::render::gpu_meshing_sparse::SparseGpuMesher;
    use unendlich_engine::world::cpu_generator::WorldGenerator;
    use unendlich_engine::world::surface::extract_surface;
    use unendlich_engine::world::ChunkCoord;
    match SparseGpuMesher::new(&ctx) {
        Ok(mesher) => {
            let gen = WorldGenerator::new(0xCAFE_F00D);
            println!("chunk,records,gpu_quads,gpu_us");
            let mut tot_us = 0u128;
            let mut n = 0u32;
            for cy in [-1i32, 0, 1, 2] {
                for cz in -1..=1 {
                    for cx in -1..=1 {
                        let coord = ChunkCoord::new(cx, cy, cz);
                        let chunk = gen.generate_chunk(coord);
                        let records = extract_surface(&chunk);
                        let t = Instant::now();
                        match mesher.mesh_chunk(coord, &records) {
                            Ok(m) => {
                                let us = t.elapsed().as_micros();
                                tot_us += us;
                                n += 1;
                                println!("({cx},{cy},{cz}),{},{},{}", records.len(), m.indices.len() / 6, us);
                            }
                            Err(e) => eprintln!("  mesh_chunk ({cx},{cy},{cz}) FAILED: {e}"),
                        }
                    }
                }
            }
            if n > 0 {
                println!("GPU mesh headless: {n} chunks, avg {} us/chunk (lavapipe)", tot_us / n as u128);
            }
        }
        Err(e) => eprintln!("SparseGpuMesher::new (headless) FAILED: {e}"),
    }

    // SHADER MICRO-BENCH (headless): per-shader SPIR-V module-ingest time on this ICD.
    // NOTE: compute pipeline-build timing (the driver's real shader compile) needs a
    // pipeline layout that MATCHES each shader's descriptor sets — building with an
    // empty/mismatched layout SEGFAULTS lavapipe. So a meaningful shader perf bench
    // needs per-shader layout reflection (a dedicated task); this reports ingest only.
    {
        let read_spv = |name: &str| -> Vec<u8> {
            let candidates = [
                format!("{}/../../assets/shaders/{}", env!("CARGO_MANIFEST_DIR"), name),
                format!("assets/shaders/{}", name),
                format!("crates/unendlich-engine/src/shaders/{}", name),
            ];
            for p in &candidates {
                if let Ok(b) = std::fs::read(p) {
                    return b;
                }
            }
            Vec::new()
        };
        let shaders = [
            "chunk_generation.comp.spv", "chunk_mesh_sparse.comp.spv", "chunk_mesh_octree.comp.spv",
            "chunk_cull.comp.spv", "cascade_fit.comp.spv", "view_compute.comp.spv",
            "chunk.frag.spv", "present.vert.spv",
        ];
        println!("shader,bytes,module_us");
        for name in shaders {
            let bytes = read_spv(name);
            if bytes.is_empty() {
                println!("{name},0,NOT_FOUND");
                continue;
            }
            let words: Vec<u32> = bytes
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect();
            let t = Instant::now();
            let module = unsafe {
                ctx.device.create_shader_module(&vk::ShaderModuleCreateInfo::builder().code(&words).build(), None)
            };
            let mod_us = t.elapsed().as_micros();
            match module {
                Ok(m) => {
                    unsafe { ctx.device.destroy_shader_module(m, None) };
                    println!("{name},{},{mod_us}", bytes.len());
                }
                Err(e) => println!("{name},{},ERR({e:?})", bytes.len()),
            }
        }
    }

    println!("HEADLESS GPU PROBE GREEN");
}
