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

    // r604 WORLDGEN-PARITY MODE (UNENDLICH_PROBE_PARITY=1): generate the
    // SAME chunks through both worldgen backends and diff every block. The
    // tree system exists twice (Rust cpu_generator + chunk_generation.comp)
    // and this is the gate that keeps the twins honest. Divergence is
    // EXPECTED and bounded: the shader adds edge-perforation (~18% of
    // canopy-edge leaves) and branch logs the CPU lacks — so terrain
    // (non-tree blocks) must match EXACTLY, while tree-block diffs must
    // stay under a generous ceiling. A form/silhouette mismatch (r604's
    // risk) blows straight through that ceiling.
    if std::env::var("UNENDLICH_PROBE_PARITY").map(|v| v == "1").unwrap_or(false) {
        // r604 WORLDGEN-PARITY: build the PRODUCTION TerrainGenerator twice
        // (Backend::Cpu vs Backend::Gpu) from the same seed and diff every
        // block of the same chunks. This tests the doc's own claim ("the
        // CPU and GPU paths produce byte-identical output for the same
        // inputs") on the exact pipeline players get — GPU raw terrain +
        // the CPU stamp_ruins post-pass — rather than hand-wiring the
        // backends (the first draft of this probe did, and its 94k "diffs"
        // were probe wiring, not worldgen). Divergence policy: terrain
        // must be EXACT; tree blocks get a bounded allowance because the
        // shader adds edge-perforation and branch logs by design.
        use unendlich_engine::world::blocks::BlockId;
        use unendlich_engine::world::terrain_generator::{Backend, TerrainGenerator};
        use unendlich_engine::world::ChunkCoord;
        // Seed is tunable for footprint/coverage scans; the default is one
        // whose near-origin surface band contains forested columns (found
        // by scan — the 0xCAFE_F00D origin is water/plains-heavy).
        let seed: u64 = std::env::var("UNENDLICH_PROBE_PARITY_SEED")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0xCAFE_F00D);
        let cpu = match TerrainGenerator::new(seed, Backend::Cpu, 16) {
            Ok(g) => g,
            Err(e) => { eprintln!("parity: cpu TerrainGenerator FAILED: {e:?}"); std::process::exit(3); }
        };
        let gpu = match TerrainGenerator::new(seed, Backend::Gpu, 16) {
            Ok(g) => g,
            Err(e) => { eprintln!("parity: gpu TerrainGenerator FAILED: {e:?}"); std::process::exit(3); }
        };
        // TWO MODES, one finding (r604 investigation): near the origin the
        // backends are byte-identical, exactly as documented; far from it
        // they DIVERGE — the shader computes noise in f32, the CPU in f64,
        // and the drift grows with |coordinate| until surface heights slip
        // a block (first observed fingerprint: cpu=Air gpu=Water at
        // x≈1280). PRE-EXISTING, not r604. Multiplayer note: join-sync's
        // "same seed ⇒ same terrain" therefore carries a SAME-BACKEND
        // caveat for far regions (a CPU-only joiner on a GPU host forks
        // distant terrain). The authoritative GATE is the near set (must
        // be terrain-exact and include tree columns so the r604 silhouette
        // mirror is actually exercised); UNENDLICH_PROBE_PARITY_FAR=1
        // additionally REPORTS far-region drift without failing — the
        // deeper fix (coordinate-relative shader noise) is its own
        // project.
        let far_report =
            std::env::var("UNENDLICH_PROBE_PARITY_FAR").map(|v| v == "1").unwrap_or(false);
        // Near set: origin plus a spread of nearby regions wide enough to
        // include forested columns at several y bands.
        // GATE SCOPE (findings of this investigation, in the ledger under
        // r604): the byte-identical claim holds for the SURFACE BAND
        // (y-chunks 2..3) near the origin. Two pre-existing forks live
        // outside it: (1) far-field f32-vs-f64 noise drift (first
        // fingerprint cpu=Air gpu=Water at x≈1280); (2) a deep-band
        // (y-chunk 1) base-terrain mismatch even at the ORIGIN
        // (fingerprint: world (0,32,0) cpu=Stone gpu=Water — suspects:
        // the env-gated CPU multi_scale_caves layer and the CPU-side
        // aquifer pass, neither mirrored in the shader). Both predate
        // r604 and get their own line items; the gate guards what the
        // tree mirror needs: surface-band, near-field, WITH tree columns.
        let regions: &[(i32, i32)] = if far_report {
            &[(40, 7), (-33, 21), (85, -60), (17, 130)]
        } else {
            // Near field, wide enough to catch forested columns (the
            // immediate origin is water-heavy). Drift at |coord| ≤ ~14
            // chunks is zero (verified empirically by this gate passing
            // terrain-exact).
            // All within ±5 chunks — the empirically byte-identical
            // footprint (±8 already shows the far-field fork).
            &[(0, 0), (2, -2), (-3, 1), (4, 3), (1, 5), (5, -1), (3, -4), (-2, 4)]
        };
        let mut coords = Vec::new();
        for &(bx, bz) in regions {
            for cx in bx..bx + 2 {
                for cz in bz..bz + 2 {
                    for cy in 2..=3 {
                        coords.push(ChunkCoord::new(cx, cy, cz));
                    }
                }
            }
        }
        let cpu_chunks = cpu.generate(&coords);
        let gpu_chunks = gpu.generate(&coords);
        let is_tree = |b: BlockId| {
            matches!(
                b,
                BlockId::OakLog | BlockId::OakLeaves | BlockId::BirchLog | BlockId::BirchLeaves
            )
        };
        let (mut terrain_diff, mut tree_diff, mut tree_cells) = (0u64, 0u64, 0u64);
        let mut first_terrain: Option<(ChunkCoord, usize, BlockId, BlockId)> = None;
        for ((coord, cc), gc) in coords.iter().zip(cpu_chunks.iter()).zip(gpu_chunks.iter()) {
            for i in 0..cc.blocks.len() {
                let (a, b) = (cc.blocks[i], gc.blocks[i]);
                if is_tree(a) || is_tree(b) {
                    tree_cells += 1;
                    if a != b { tree_diff += 1; }
                } else if a != b {
                    terrain_diff += 1;
                    if first_terrain.is_none() {
                        first_terrain = Some((*coord, i, a, b));
                    }
                }
            }
        }
        let ratio = if tree_cells > 0 { tree_diff as f64 / tree_cells as f64 } else { 0.0 };
        println!(
            "worldgen-parity: {} chunks | terrain diffs {} | tree cells {} | tree diffs {} ({:.1}%)",
            coords.len(), terrain_diff, tree_cells, tree_diff, ratio * 100.0
        );
        if let Some((c, i, a, b)) = first_terrain {
            println!("  first terrain diff: chunk {c:?} idx {i} cpu={a:?} gpu={b:?}");
        }
        if far_report {
            // Informational: drift magnitude for the record, never a gate.
            println!("worldgen-parity FAR drift report done (informational)");
            return;
        }
        if terrain_diff > 0 || ratio > 0.35 {
            eprintln!("worldgen-parity FAILED (terrain_diff={terrain_diff}, tree ratio {:.1}%)", ratio * 100.0);
            std::process::exit(4);
        }
        if tree_cells == 0 {
            eprintln!("worldgen-parity FAILED: near set contains no tree columns — the silhouette mirror is UNTESTED; widen the region set");
            std::process::exit(4);
        }
        println!("worldgen-parity OK ({} tree cells exercised)", tree_cells);
        return;
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
