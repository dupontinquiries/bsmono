//! Compare GPU sparse mesher output to CPU naive mesher output.
//!
//! Runs the same set of representative chunks through both paths and
//! reports topology equivalence (same quad count per chunk) plus
//! per-chunk and aggregate timing.
//!
//! Usage:
//!   cargo run --release --features="render,gpu" --bin gpu-mesh-bench
//!
//! Opens a hidden GLFW window only to satisfy VulkanContext's surface
//! requirement; no rendering happens. Headless context support could
//! eliminate the window entirely, filed for later.

#![cfg(feature = "gpu")]

use glfw::{ClientApiHint, WindowHint};
use std::sync::Arc;
use std::thread;
use std::time::Instant;
use unendlich_engine::constants::WORLD_SEED_DEFAULT;
use unendlich_engine::render::context::VulkanContext;
use unendlich_engine::render::gpu_meshing_sparse::SparseGpuMesher;
use unendlich_engine::render::mesher::mesh_chunk_naive;
use unendlich_engine::render::meshing_pool::{MeshRequest, MeshingPool};
use unendlich_engine::sky::Sky;
use unendlich_engine::world::chunk::{Chunk, ChunkCoord};
use unendlich_engine::world::cpu_generator::WorldGenerator;
use unendlich_engine::world::lod::{LodLevel, LodSettings};
use unendlich_engine::world::surface::{extract_surface, extract_surface_runs};

/// Production uses 4 mesh workers (see `unendlich_world::MESH_WORKERS`).
const POOL_WORKERS: usize = 4;

fn main() {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    // Hidden GLFW window — VulkanContext currently requires a surface
    // for queue-family selection. The window is never rendered to.
    let mut glfw = glfw::init(glfw::FAIL_ON_ERRORS).expect("glfw init");
    glfw.window_hint(WindowHint::ClientApi(ClientApiHint::NoApi));
    glfw.window_hint(WindowHint::Visible(false));
    let (window, _events) = glfw
        .create_window(
            64,
            64,
            "gpu-mesh-bench (hidden)",
            glfw::WindowMode::Windowed,
        )
        .expect("create hidden window");

    println!("Initializing VulkanContext...");
    let ctx = match VulkanContext::new(&glfw, &window) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("VulkanContext::new failed: {:?}", e);
            std::process::exit(1);
        }
    };
    let device_name = unsafe {
        let props = ctx
            .instance
            .get_physical_device_properties(ctx.physical_device);
        std::ffi::CStr::from_ptr(props.device_name.as_ptr())
            .to_string_lossy()
            .into_owned()
    };
    println!("VulkanContext ready (device: {})", device_name);

    let mesher = match SparseGpuMesher::new(&ctx) {
        Ok(m) => m,
        Err(e) => {
            eprintln!("SparseGpuMesher::new failed: {}", e);
            std::process::exit(1);
        }
    };
    println!("SparseGpuMesher ready");
    println!(
        "  MAX_BATCH_CHUNKS = {} (effective at runtime = {})",
        unendlich_engine::render::gpu_meshing_sparse::MAX_BATCH_CHUNKS,
        unendlich_engine::render::gpu_meshing_sparse::effective_batch_size()
    );
    println!();

    let gen = WorldGenerator::new(WORLD_SEED_DEFAULT as u32);

    println!("Topology comparison (CPU naive vs GPU sparse)\n");
    println!(
        "  {:>9}  {:>8}  {:>8}  {:>10}  {:>10}  {:>8}  {:>10}",
        "chunk", "records", "cpu_v", "cpu_us", "gpu_us", "speedup", "match"
    );
    println!("  {}", "-".repeat(80));

    let mut total_cpu_us = 0u128;
    let mut total_gpu_us = 0u128;
    let mut total_records = 0usize;
    let mut total_chunks = 0u32;
    let mut total_matches = 0u32;

    for chunk_y in [-1i32, 0, 1, 2] {
        for cz in -1..=1 {
            for cx in -1..=1 {
                let coord = ChunkCoord::new(cx, chunk_y, cz);
                let chunk = gen.generate_chunk(coord);

                let t = Instant::now();
                let cpu_mesh = mesh_chunk_naive(&chunk);
                let cpu_us = t.elapsed().as_micros();

                let t = Instant::now();
                let records = extract_surface(&chunk);
                let n_records = records.len();
                let gpu_mesh = match mesher.mesh_chunk(coord, &records) {
                    Ok(m) => m,
                    Err(e) => {
                        eprintln!(
                            "  ({:>2},{:>2},{:>2}) GPU mesh failed: {}",
                            cx, chunk_y, cz, e
                        );
                        continue;
                    }
                };
                let gpu_us = t.elapsed().as_micros();

                total_cpu_us += cpu_us;
                total_gpu_us += gpu_us;
                total_records += n_records;
                total_chunks += 1;

                let speedup = if gpu_us > 0 {
                    cpu_us as f64 / gpu_us as f64
                } else {
                    0.0
                };

                let cpu_quads = cpu_mesh.indices.len() / 6;
                let gpu_quads = gpu_mesh.indices.len() / 6;
                let topo_match = cpu_quads == gpu_quads;
                if topo_match {
                    total_matches += 1;
                }

                println!(
                    "  ({:>2},{:>2},{:>2})  {:>8}  {:>8}  {:>10}  {:>10}  {:>7.1}x  {}",
                    cx,
                    chunk_y,
                    cz,
                    n_records,
                    cpu_mesh.vertex_count(),
                    cpu_us,
                    gpu_us,
                    speedup,
                    if topo_match {
                        format!("OK ({})", cpu_quads)
                    } else {
                        format!("MISMATCH cpu={} gpu={}", cpu_quads, gpu_quads)
                    }
                );
            }
        }
    }

    println!();
    println!("Total: {} chunks, {} records.", total_chunks, total_records);
    if total_chunks > 0 {
        println!(
            "CPU total: {} us  (avg {} us/chunk)",
            total_cpu_us,
            total_cpu_us / total_chunks as u128
        );
        println!(
            "GPU total: {} us  (avg {} us/chunk)  [v1 path: one chunk per dispatch]",
            total_gpu_us,
            total_gpu_us / total_chunks as u128
        );
        if total_gpu_us > 0 {
            println!(
                "v1 path speedup: {:.2}×",
                total_cpu_us as f64 / total_gpu_us as f64
            );
        }
    }
    println!(
        "Topology matches: {}/{} chunks",
        total_matches, total_chunks
    );

    if total_matches < total_chunks {
        eprintln!(
            "\nWARNING: {} chunks had topology mismatch. Investigate before wiring",
            total_chunks - total_matches
        );
        eprintln!("into production; the renderer would see different quad counts.");
        std::process::exit(2);
    }

    // ─────────────────────────────────────────────────────────────────
    // v2 (batched) path comparison.
    //
    // Same chunks, but submitted as one big batch through
    // `mesh_chunks_batch`. The mesher internally splits into
    // MAX_BATCH_CHUNKS-sized submissions. Expected speedup vs v1:
    // ~10-20× because the per-submit floor is amortized.
    // ─────────────────────────────────────────────────────────────────
    println!();
    println!("─ Batched (v2) path ─\n");

    // Build input array once. Reuse the same coord grid as above.
    let mut all_inputs_owned: Vec<(
        ChunkCoord,
        Vec<unendlich_engine::world::surface::SurfaceRecord>,
    )> = Vec::new();
    for chunk_y in [-1i32, 0, 1, 2] {
        for cz in -1..=1 {
            for cx in -1..=1 {
                let coord = ChunkCoord::new(cx, chunk_y, cz);
                let chunk = gen.generate_chunk(coord);
                let records = extract_surface(&chunk);
                all_inputs_owned.push((coord, records));
            }
        }
    }
    let inputs: Vec<unendlich_engine::render::gpu_meshing_sparse::ChunkBatchInput> =
        all_inputs_owned
            .iter()
            .map(
                |(c, r)| unendlich_engine::render::gpu_meshing_sparse::ChunkBatchInput {
                    coord: *c,
                    records: r.as_slice(),
                },
            )
            .collect();

    // Time the whole batch dispatch.
    let t = Instant::now();
    let outputs = match mesher.mesh_chunks_batch(&inputs) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("mesh_chunks_batch failed: {}", e);
            std::process::exit(1);
        }
    };
    let v2_us = t.elapsed().as_micros();

    let mut v2_matches = 0u32;
    let mut v2_total_quads = 0usize;
    for (idx, out) in outputs.iter().enumerate() {
        let coord = inputs[idx].coord;
        let n_records = inputs[idx].records.len();
        match &out.mesh {
            Ok(mesh) => {
                let quads = mesh.indices.len() / 6;
                v2_total_quads += quads;
                // Topology check: compare against same chunk's CPU mesh.
                let chunk = gen.generate_chunk(coord);
                let cpu_mesh = mesh_chunk_naive(&chunk);
                let cpu_quads = cpu_mesh.indices.len() / 6;
                if cpu_quads == quads {
                    v2_matches += 1;
                } else {
                    eprintln!(
                        "  v2 ({:>2},{:>2},{:>2}) records={} quads cpu={} gpu={} MISMATCH",
                        coord.x, coord.y, coord.z, n_records, cpu_quads, quads
                    );
                }
            }
            Err(e) => {
                eprintln!(
                    "  v2 ({:>2},{:>2},{:>2}) records={}  failed: {}",
                    coord.x, coord.y, coord.z, n_records, e
                );
            }
        }
    }

    println!(
        "Batched dispatch ({} chunks): {} us total ({} us/chunk avg)",
        inputs.len(),
        v2_us,
        v2_us / (inputs.len() as u128).max(1)
    );
    println!("Topology matches: {}/{} chunks", v2_matches, inputs.len());
    println!("Total quads emitted: {}", v2_total_quads);

    if total_gpu_us > 0 {
        println!();
        println!(
            "v2 vs v1 speedup: {:.2}×",
            total_gpu_us as f64 / v2_us as f64
        );
        println!(
            "v2 vs CPU speedup: {:.2}×",
            total_cpu_us as f64 / v2_us as f64
        );
    }

    if v2_matches < inputs.len() as u32 {
        eprintln!("\nWARNING: batched path had topology mismatches.");
        std::process::exit(2);
    }

    // ─────────────────────────────────────────────────────────────────
    // SurfaceRun compression measurement (v0.24.9).
    //
    // Show how much extract_surface_runs compresses the surface
    // representation compared to extract_surface. This is the input-
    // bandwidth and quad-count win the future "GPU runs" kernel will
    // benefit from. CPU-side timing of the extractors gets us a sense
    // of run extraction cost.
    // ─────────────────────────────────────────────────────────────────
    println!();
    println!("─ SurfaceRun compression analysis ─\n");

    // Pre-generate all chunks ONCE so we measure extraction time, not
    // generator time. The generator dominates (~115ms/chunk for full
    // noise+biome+layers) — without this fix the "extract" timing line
    // is meaningless. v0.24.12 fix.
    let pre_chunks: Vec<Chunk> = all_inputs_owned
        .iter()
        .map(|(coord, _)| gen.generate_chunk(*coord))
        .collect();

    let t = Instant::now();
    let mut total_records = 0usize;
    for chunk in &pre_chunks {
        total_records += extract_surface(chunk).len();
    }
    let records_extract_us = t.elapsed().as_micros();

    let t = Instant::now();
    let mut total_runs = 0usize;
    let mut total_run_length = 0usize;
    for chunk in &pre_chunks {
        let runs = extract_surface_runs(chunk);
        total_runs += runs.len();
        for r in &runs {
            total_run_length += r.length() as usize;
        }
    }
    let runs_extract_us = t.elapsed().as_micros();

    // v0.24.13: also time the six-pass reference implementation as a
    // side-by-side comparison. Production uses extract_surface_runs
    // (single-pass); the six-pass version is kept as a test oracle but
    // we time it here too so the readme's "X× faster" claim is verified
    // on the user's hardware.
    let t = Instant::now();
    let mut total_runs_6p = 0usize;
    for chunk in &pre_chunks {
        let runs = unendlich_engine::world::surface::extract_surface_runs_six_pass(chunk);
        total_runs_6p += runs.len();
    }
    let runs_six_pass_us = t.elapsed().as_micros();
    assert_eq!(
        total_runs, total_runs_6p,
        "single-pass and six-pass produced different run counts"
    );

    // v0.24.15: parallel-extract timings. Tests the linear-scaling
    // hypothesis — if extract scales near-linearly with worker count,
    // the GPU path's CPU-side framing cost drops by ~4×, flipping the
    // honest v3 vs CPU-naive-parallel comparison.
    //
    // Uses the same atomic-counter + N-thread pattern as the parallel
    // naive mesher baseline below. POOL_WORKERS matches production's
    // mesh worker count (currently 4).
    let pre_chunks_arc: Arc<Vec<Chunk>> = Arc::new(pre_chunks.clone());
    let records_extract_par_us = {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let next = Arc::new(AtomicUsize::new(0));
        let chunks = Arc::clone(&pre_chunks_arc);
        let n = chunks.len();
        let t = Instant::now();
        let handles: Vec<_> = (0..POOL_WORKERS)
            .map(|_| {
                let next = Arc::clone(&next);
                let chunks = Arc::clone(&chunks);
                thread::spawn(move || {
                    let mut local_total = 0usize;
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= n {
                            break;
                        }
                        local_total += extract_surface(&chunks[i]).len();
                    }
                    local_total
                })
            })
            .collect();
        let mut got = 0usize;
        for h in handles {
            got += h.join().unwrap();
        }
        assert_eq!(
            got, total_records,
            "parallel records extract count mismatch"
        );
        t.elapsed().as_micros()
    };

    let runs_extract_par_us = {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let next = Arc::new(AtomicUsize::new(0));
        let chunks = Arc::clone(&pre_chunks_arc);
        let n = chunks.len();
        let t = Instant::now();
        let handles: Vec<_> = (0..POOL_WORKERS)
            .map(|_| {
                let next = Arc::clone(&next);
                let chunks = Arc::clone(&chunks);
                thread::spawn(move || {
                    let mut local_total = 0usize;
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= n {
                            break;
                        }
                        local_total += extract_surface_runs(&chunks[i]).len();
                    }
                    local_total
                })
            })
            .collect();
        let mut got = 0usize;
        for h in handles {
            got += h.join().unwrap();
        }
        assert_eq!(got, total_runs, "parallel runs extract count mismatch");
        t.elapsed().as_micros()
    };

    // v0.24.17: worker-count sweep. The 4-worker numbers above feed
    // the honest-totals comparison (because production MeshingPool
    // uses 4 workers); the sweep below explores the scaling curve
    // across [1, 2, 4, 8, 16] to find the knee — i.e. the point past
    // which adding workers stops helping. M2 Max has 12 cores (8P+4E);
    // 16 oversubscribes intentionally.
    fn time_extract_par<F>(pre: &Arc<Vec<Chunk>>, n_workers: usize, body: F) -> u128
    where
        F: Fn(&Chunk) -> usize + Send + Sync + 'static + Copy,
    {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let next = Arc::new(AtomicUsize::new(0));
        let chunks = Arc::clone(pre);
        let n = chunks.len();
        let t = Instant::now();
        let handles: Vec<_> = (0..n_workers)
            .map(|_| {
                let next = Arc::clone(&next);
                let chunks = Arc::clone(&chunks);
                thread::spawn(move || {
                    let mut local = 0usize;
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= n {
                            break;
                        }
                        local += body(&chunks[i]);
                    }
                    local
                })
            })
            .collect();
        for h in handles {
            let _ = h.join().unwrap();
        }
        t.elapsed().as_micros()
    }

    // Note: 36 chunks / 16 workers = 2.25 chunks per worker. At
    // worker counts > chunk count, some workers do zero work — that's
    // a hard ceiling on the speedup we can measure here. For a wider
    // workload (say 200 chunks), 16 workers would have ~12 chunks
    // each. This bench measures realistic chunk-load bursts.
    let sweep_workers = [1usize, 2, 4, 8, 16];
    let mut records_sweep = Vec::with_capacity(sweep_workers.len());
    let mut runs_sweep = Vec::with_capacity(sweep_workers.len());
    for &w in &sweep_workers {
        let r = time_extract_par(&pre_chunks_arc, w, |c| extract_surface(c).len());
        let n = time_extract_par(&pre_chunks_arc, w, |c| extract_surface_runs(c).len());
        records_sweep.push(r);
        runs_sweep.push(n);
    }

    println!(
        "  records:               {:>7}  (extract: {} us total, {} us/chunk avg)",
        total_records,
        records_extract_us,
        records_extract_us / (pre_chunks.len() as u128).max(1)
    );
    println!(
        "  records ({}-worker par):       (extract: {} us total)  [v0.24.15]",
        POOL_WORKERS, records_extract_par_us
    );
    println!(
        "  runs (1-pass):         {:>7}  (extract: {} us total, {} us/chunk avg)  [production]",
        total_runs,
        runs_extract_us,
        runs_extract_us / (pre_chunks.len() as u128).max(1)
    );
    println!(
        "  runs (1-pass, {}-worker par):  (extract: {} us total)  [v0.24.15]",
        POOL_WORKERS, runs_extract_par_us
    );
    println!(
        "  runs (6-pass):         {:>7}  (extract: {} us total, {} us/chunk avg)  [reference]",
        total_runs_6p,
        runs_six_pass_us,
        runs_six_pass_us / (pre_chunks.len() as u128).max(1)
    );
    if total_runs > 0 {
        println!(
            "  compression ratio (records/runs): {:.2}×",
            total_records as f64 / total_runs as f64
        );
        println!(
            "  avg run length: {:.2} blocks",
            total_run_length as f64 / total_runs as f64
        );
    }
    if records_extract_us > 0 {
        println!(
            "  runs (1-pass) extract overhead vs records: {:.2}×",
            runs_extract_us as f64 / records_extract_us as f64
        );
    }
    if runs_extract_us > 0 {
        println!(
            "  1-pass speedup over 6-pass: {:.2}×",
            runs_six_pass_us as f64 / runs_extract_us as f64
        );
    }
    if records_extract_par_us > 0 {
        println!(
            "  records parallel speedup ({} workers): {:.2}×",
            POOL_WORKERS,
            records_extract_us as f64 / records_extract_par_us as f64
        );
    }
    if runs_extract_par_us > 0 {
        println!(
            "  runs parallel speedup ({} workers): {:.2}×",
            POOL_WORKERS,
            runs_extract_us as f64 / runs_extract_par_us as f64
        );
    }

    // v0.24.17: worker-count sweep table.
    // 36 chunks total; at worker counts > 36, some workers idle.
    // Note: M2 Max = 8P + 4E = 12 cores; 16 oversubscribes.
    println!();
    println!("  Worker-count sweep (extract time vs worker count):");
    println!(
        "  {:>7}  {:>10}  {:>10}  {:>10}  {:>10}",
        "workers", "records_us", "rec_speedup", "runs_us", "run_speedup"
    );
    for (i, &w) in sweep_workers.iter().enumerate() {
        let r = records_sweep[i];
        let n = runs_sweep[i];
        let r_speedup = records_sweep[0] as f64 / r.max(1) as f64;
        let n_speedup = runs_sweep[0] as f64 / n.max(1) as f64;
        println!(
            "  {:>7}  {:>10}  {:>9.2}×  {:>10}  {:>9.2}×",
            w, r, r_speedup, n, n_speedup
        );
    }
    println!("  (speedup is vs 1-worker baseline in this run; 36 chunks total)");
    println!();
    println!("  Note: the GPU 'runs' kernel (v3, measured below) emits one quad");
    println!("  per run instead of up to 6 quads per record. Quad count drops by");
    println!("  this compression ratio; per-quad GPU work stays the same.");

    // ─────────────────────────────────────────────────────────────────
    // v3 (runs) path: one quad per SurfaceRun.
    //
    // Same batched dispatch model as v2; different kernel; different
    // input format (SurfaceRun[] instead of SurfaceRecord[]).
    // ─────────────────────────────────────────────────────────────────
    println!();
    println!("─ Runs (v3) path ─\n");

    // Build runs inputs for each chunk.
    let mut all_runs_owned: Vec<(
        ChunkCoord,
        Vec<unendlich_engine::world::surface::SurfaceRun>,
    )> = Vec::new();
    for (coord, _) in &all_inputs_owned {
        let chunk = gen.generate_chunk(*coord);
        let runs = extract_surface_runs(&chunk);
        all_runs_owned.push((*coord, runs));
    }
    let runs_inputs: Vec<unendlich_engine::render::gpu_meshing_sparse::ChunkBatchInputRuns> =
        all_runs_owned
            .iter()
            .map(
                |(c, r)| unendlich_engine::render::gpu_meshing_sparse::ChunkBatchInputRuns {
                    coord: *c,
                    runs: r.as_slice(),
                },
            )
            .collect();

    let t = Instant::now();
    let v3_outputs = match mesher.mesh_chunks_batch_runs(&runs_inputs) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("mesh_chunks_batch_runs failed: {}", e);
            std::process::exit(1);
        }
    };
    let v3_us = t.elapsed().as_micros();

    // Topology validation: for the runs path we can't compare quad
    // counts directly (runs emit FEWER quads than naive). Instead
    // verify: sum of run lengths == sum of face bits in records ==
    // CPU naive quad count. All three represent total surface-face
    // count for the chunk.
    let mut v3_matches = 0u32;
    let mut v3_total_quads = 0usize;
    let mut v3_total_face_blocks = 0u32;
    for (idx, out) in v3_outputs.iter().enumerate() {
        let coord = runs_inputs[idx].coord;
        let runs = runs_inputs[idx].runs;
        let face_blocks_in_runs: u32 = runs.iter().map(|r| r.length() as u32).sum();
        v3_total_face_blocks += face_blocks_in_runs;
        match &out.mesh {
            Ok(mesh) => {
                let quads = mesh.indices.len() / 6;
                v3_total_quads += quads;
                // GPU should have produced one quad per run.
                if quads != runs.len() {
                    eprintln!("  v3 ({:>2},{:>2},{:>2}) runs={} quads={} MISMATCH (expected 1 quad per run)",
                        coord.x, coord.y, coord.z, runs.len(), quads);
                    continue;
                }
                // The quad areas should sum to face_blocks_in_runs.
                // We don't decode quad areas from vertex positions here;
                // instead compare against CPU naive's quad count for the
                // chunk: that equals total face count, which equals
                // sum of run lengths.
                let chunk = gen.generate_chunk(coord);
                let cpu_mesh = mesh_chunk_naive(&chunk);
                let cpu_quads = cpu_mesh.indices.len() / 6;
                if face_blocks_in_runs as usize != cpu_quads {
                    eprintln!(
                        "  v3 ({:>2},{:>2},{:>2}) face_count_from_runs={} cpu_quads={} MISMATCH",
                        coord.x, coord.y, coord.z, face_blocks_in_runs, cpu_quads
                    );
                    continue;
                }
                v3_matches += 1;
            }
            Err(e) => {
                eprintln!(
                    "  v3 ({:>2},{:>2},{:>2}) runs={}  failed: {}",
                    coord.x,
                    coord.y,
                    coord.z,
                    runs.len(),
                    e
                );
            }
        }
    }

    println!(
        "Batched dispatch ({} chunks): {} us total ({} us/chunk avg)",
        runs_inputs.len(),
        v3_us,
        v3_us / (runs_inputs.len() as u128).max(1)
    );
    println!(
        "Total quads emitted: {} (sum of run lengths: {})",
        v3_total_quads, v3_total_face_blocks
    );
    println!(
        "Quad-count reduction vs v2: {:.2}× fewer quads",
        v2_total_quads as f64 / v3_total_quads.max(1) as f64
    );
    println!(
        "Topology matches: {}/{} chunks",
        v3_matches,
        runs_inputs.len()
    );

    if total_gpu_us > 0 {
        println!();
        println!(
            "v3 vs v1 speedup: {:.2}×",
            total_gpu_us as f64 / v3_us as f64
        );
        println!("v3 vs v2 speedup: {:.2}×", v2_us as f64 / v3_us as f64);
        println!(
            "v3 vs CPU naive seq speedup: {:.2}×",
            total_cpu_us as f64 / v3_us as f64
        );
    }

    if v3_matches < runs_inputs.len() as u32 {
        eprintln!("\nWARNING: v3 runs path had topology mismatches.");
        // Don't exit — let the parallel CPU baselines still run so the
        // user gets the full picture.
    }

    // ─────────────────────────────────────────────────────────────────
    // Parallel CPU baselines.
    //
    // The earlier "CPU total" number is single-threaded sequential
    // `mesh_chunk_naive`. Production uses a multi-threaded MeshingPool
    // (4 workers). Compare against:
    //   (a) Parallel naive: same kernel, K-thread parallelism.
    //   (b) MeshingPool: production-quality greedy + sky-lit, 4 workers.
    //
    // Both are wall-clock to process the same 36 chunks.
    // ─────────────────────────────────────────────────────────────────
    println!();
    println!("─ Parallel CPU baselines ─\n");

    // Pre-generate chunks once so generation cost isn't part of the
    // mesh timing.
    let chunks: Vec<Chunk> = all_inputs_owned
        .iter()
        .map(|(coord, _)| gen.generate_chunk(*coord))
        .collect();

    // (a) Parallel naive mesher. Spawn POOL_WORKERS threads, each
    // grabs chunks from a shared atomic counter. This is the
    // upper bound on what the naive kernel can achieve with the
    // worker count production uses.
    let par_naive_us = {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let next = Arc::new(AtomicUsize::new(0));
        let chunks_arc = Arc::new(chunks.clone());
        let n_chunks = chunks_arc.len();

        let t = Instant::now();
        let handles: Vec<_> = (0..POOL_WORKERS)
            .map(|_| {
                let next = Arc::clone(&next);
                let chunks = Arc::clone(&chunks_arc);
                thread::spawn(move || loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= n_chunks {
                        break;
                    }
                    let _ = mesh_chunk_naive(&chunks[i]);
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        t.elapsed().as_micros()
    };

    println!(
        "Parallel naive ({} workers, {} chunks): {} us total ({} us/chunk avg)",
        POOL_WORKERS,
        chunks.len(),
        par_naive_us,
        par_naive_us / (chunks.len() as u128).max(1)
    );

    // (b) Production MeshingPool. Same chunks fed through the real
    // production code path: greedy mesher with sky lighting.
    let pool_us = {
        let lod_settings = Arc::new(LodSettings::default());
        let pool = MeshingPool::new(POOL_WORKERS, 256, lod_settings);

        let sky = Sky::new();
        let t = Instant::now();

        // Submit all requests.
        for (i, chunk) in chunks.iter().enumerate() {
            let coord = all_inputs_owned[i].0;
            let chunk_arc = Arc::new(chunk.clone());
            loop {
                let req = MeshRequest {
                    coord,
                    chunk: Arc::clone(&chunk_arc),
                    version_at_submit: 0,
                    sky: sky.clone(),
                    world_model: None,
                    current_tick: None,
                    lod: LodLevel::Near,
                    generator: None,
                    octree: None,
                };
                if pool.submit(req) {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_micros(50));
            }
        }

        // Drain results until we have all of them.
        let mut received = 0usize;
        while received < chunks.len() {
            let results = pool.drain_completed();
            received += results.len();
            if received < chunks.len() {
                std::thread::sleep(std::time::Duration::from_micros(50));
            }
        }
        t.elapsed().as_micros()
    };

    println!(
        "MeshingPool ({} workers, {} chunks): {} us total ({} us/chunk avg)",
        POOL_WORKERS,
        chunks.len(),
        pool_us,
        pool_us / (chunks.len() as u128).max(1)
    );
    println!("  (production-quality: greedy + sky-lit; not directly comparable to naive)");

    // ─────────────────────────────────────────────────────────────────
    // Summary table.
    // ─────────────────────────────────────────────────────────────────
    println!();
    println!("─ Summary ─\n");
    println!("  {:<32}  {:>10}  {:>10}", "Path", "total_us", "us/chunk");
    println!("  {}", "-".repeat(56));
    println!(
        "  {:<32}  {:>10}  {:>10}",
        "CPU naive sequential",
        total_cpu_us,
        total_cpu_us / (chunks.len() as u128).max(1)
    );
    println!(
        "  {:<32}  {:>10}  {:>10}",
        format!("CPU naive parallel ({})", POOL_WORKERS),
        par_naive_us,
        par_naive_us / (chunks.len() as u128).max(1)
    );
    println!(
        "  {:<32}  {:>10}  {:>10}",
        format!("MeshingPool ({} workers, greedy+sky)", POOL_WORKERS),
        pool_us,
        pool_us / (chunks.len() as u128).max(1)
    );
    println!(
        "  {:<32}  {:>10}  {:>10}",
        "GPU sparse v1 (one per submit)",
        total_gpu_us,
        total_gpu_us / (chunks.len() as u128).max(1)
    );
    println!(
        "  {:<32}  {:>10}  {:>10}",
        "GPU sparse v2 (batched)",
        v2_us,
        v2_us / (chunks.len() as u128).max(1)
    );
    println!(
        "  {:<32}  {:>10}  {:>10}",
        "GPU runs v3 (batched, 1D-merged)",
        v3_us,
        v3_us / (chunks.len() as u128).max(1)
    );
    println!();

    println!(
        "Speedup of GPU v2 vs CPU naive sequential:  {:.2}×",
        total_cpu_us as f64 / v2_us as f64
    );
    println!(
        "Speedup of GPU v2 vs CPU naive parallel:    {:.2}×",
        par_naive_us as f64 / v2_us as f64
    );
    println!(
        "Speedup of GPU v2 vs MeshingPool:           {:.2}×",
        pool_us as f64 / v2_us as f64
    );
    println!();
    println!(
        "Speedup of GPU v3 vs CPU naive sequential:  {:.2}×",
        total_cpu_us as f64 / v3_us as f64
    );
    println!(
        "Speedup of GPU v3 vs CPU naive parallel:    {:.2}×",
        par_naive_us as f64 / v3_us as f64
    );
    println!(
        "Speedup of GPU v3 vs MeshingPool:           {:.2}×",
        pool_us as f64 / v3_us as f64
    );

    // ─── Honest totals (extract + dispatch) ─────────────────────────
    // The numbers above compare GPU DISPATCH time against CPU TOTAL
    // time. That's apples-to-oranges: the GPU path also has a CPU-side
    // extract step. Honest comparison is `extract + dispatch` vs the
    // CPU mesher's total (which already includes its own surface scan
    // inside the kernel).
    println!();
    println!("─ Honest totals (extract + dispatch) ─");
    let v2_honest_seq = records_extract_us + v2_us;
    let v2_honest_par = records_extract_par_us + v2_us;
    let v3_honest_seq = runs_extract_us + v3_us;
    let v3_honest_par = runs_extract_par_us + v3_us;
    println!(
        "  GPU v2 honest (seq extract):  {:>7} us = {} (extract) + {} (dispatch)",
        v2_honest_seq, records_extract_us, v2_us
    );
    println!(
        "  GPU v2 honest (par extract):  {:>7} us = {} (extract) + {} (dispatch)",
        v2_honest_par, records_extract_par_us, v2_us
    );
    println!(
        "  GPU v3 honest (seq extract):  {:>7} us = {} (extract) + {} (dispatch)",
        v3_honest_seq, runs_extract_us, v3_us
    );
    println!(
        "  GPU v3 honest (par extract):  {:>7} us = {} (extract) + {} (dispatch)",
        v3_honest_par, runs_extract_par_us, v3_us
    );
    println!();
    println!("  vs CPU naive parallel ({} us):", par_naive_us);
    println!(
        "    v2 (seq extract): {:.2}×  | v2 (par extract): {:.2}×",
        par_naive_us as f64 / v2_honest_seq as f64,
        par_naive_us as f64 / v2_honest_par as f64
    );
    println!(
        "    v3 (seq extract): {:.2}×  | v3 (par extract): {:.2}×",
        par_naive_us as f64 / v3_honest_seq as f64,
        par_naive_us as f64 / v3_honest_par as f64
    );
    println!("  vs MeshingPool ({} us):", pool_us);
    println!(
        "    v3 (par extract): {:.2}×",
        pool_us as f64 / v3_honest_par as f64
    );
    println!();
    println!("Note: GPU v2/v3 produce unlit flat colors; MeshingPool produces");
    println!("greedy meshes with sky-lit per-face shading. Not visually equivalent.");
    println!("v3 emits ~3-5× fewer quads than v2 (1D-merged), partial-greedy.");
    println!("The number that matters for Layer A.3 production-wiring decision");
    println!("is GPU v3 honest (par extract) vs MeshingPool: closest substrate match.");
}
