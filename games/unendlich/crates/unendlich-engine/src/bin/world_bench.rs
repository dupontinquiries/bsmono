//! Benchmark: generate 32^3 chunks via the CPU path and the unified
//! TerrainGenerator (with auto fallback). The GPU path is currently
//! disabled (being rewritten for 3D-noise generation).
//!
//! Run with:
//!     cargo run --release --bin world-bench

use unendlich_engine::constants::{CHUNK_VOLUME, SEA_LEVEL};
use unendlich_engine::world::{Backend, ChunkCoord, TerrainGenerator};
use unendlich_engine::world::cpu_generator::WorldGenerator;
use std::time::Instant;

const SEED: u64 = 0xCAFE_F00D;

/// Generate a cubic shell of chunks centered at the surface band.
/// `radius` is the Chebyshev radius. The y range is centered at sea-level
/// chunks (so we get a mix of underground, surface, and air chunks).
fn radius_chunks(radius: i32) -> Vec<ChunkCoord> {
    // Center y on the chunk that contains sea level.
    let sea_chunk_y = SEA_LEVEL / unendlich_engine::constants::CHUNK_SIZE;
    let mut out = Vec::with_capacity(((2 * radius + 1).pow(3)) as usize);
    for dz in -radius..=radius {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                out.push(ChunkCoord::new(dx, sea_chunk_y + dy, dz));
            }
        }
    }
    out
}

fn fmt_ms(seconds: f64) -> String {
    format!("{:>8.2} ms", seconds * 1000.0)
}

fn fmt_chunks_per_sec(n: usize, seconds: f64) -> String {
    if seconds <= 0.0 { return "    inf cps".into(); }
    format!("{:>7.0} cps", n as f64 / seconds)
}

fn count_solid(blocks: &[unendlich_engine::world::BlockId]) -> usize {
    blocks.iter().filter(|b| b.is_solid()).count()
}

fn bench_cpu(coords: &[ChunkCoord]) -> (f64, usize) {
    let gen = WorldGenerator::new(SEED as u32);
    let start = Instant::now();
    let mut total_solid = 0usize;
    for &c in coords {
        let chunk = gen.generate_chunk(c);
        total_solid += count_solid(&chunk.blocks);
    }
    (start.elapsed().as_secs_f64(), total_solid)
}

fn bench_terrain(label: &str, backend: Backend, coords: &[ChunkCoord]) -> Option<(f64, usize, bool)> {
    let gen = match TerrainGenerator::new(SEED, backend, 16) {
        Ok(g) => g,
        Err(e) => {
            println!("  [skip] {}: {}", label, e);
            return None;
        }
    };
    let used_gpu = gen.has_gpu();

    // Warm-up.
    let _ = gen.generate(&coords[0..coords.len().min(4)]);

    let start = Instant::now();
    let chunks = gen.generate(coords);
    let elapsed = start.elapsed().as_secs_f64();

    let solid: usize = chunks.iter().map(|c| count_solid(&c.blocks)).sum();
    Some((elapsed, solid, used_gpu))
}

fn header(s: &str) {
    println!();
    println!("─── {} ───", s);
}

fn run_for_radius(radius: i32) {
    let coords = radius_chunks(radius);
    let n = coords.len();
    println!(
        "\n=== radius {} ({} chunks, {} blocks) ===",
        radius, n, n * CHUNK_VOLUME,
    );

    header("CPU (raw, single-threaded)");
    let (cpu_t, cpu_solid) = bench_cpu(&coords);
    println!("  total: {}  ({}, {} solid blocks)",
             fmt_ms(cpu_t), fmt_chunks_per_sec(n, cpu_t), cpu_solid);

    header("TerrainGenerator: Cpu backend");
    if let Some((t, s, _)) = bench_terrain("cpu-backed", Backend::Cpu, &coords) {
        println!("  total: {}  ({}, {} solid)",
                 fmt_ms(t), fmt_chunks_per_sec(n, t), s);
    }

    header("TerrainGenerator: Auto backend");
    if let Some((t, s, gpu)) = bench_terrain("auto", Backend::Auto, &coords) {
        let tag = if gpu { "GPU active" } else { "GPU unavailable, CPU fallback" };
        println!("  total: {}  ({}, {} solid)  [{}]",
                 fmt_ms(t), fmt_chunks_per_sec(n, t), s, tag);
    }

    header("TerrainGenerator: Gpu backend (force)");
    if let Some((t, s, _)) = bench_terrain("gpu-forced", Backend::Gpu, &coords) {
        println!("  total: {}  ({}, {} solid)",
                 fmt_ms(t), fmt_chunks_per_sec(n, t), s);
        if cpu_t > 0.0 {
            println!("  speedup vs CPU: {:.2}x", cpu_t / t.max(1e-9));
        }
    }
}

fn determinism_check() {
    println!("\n=== determinism: CPU vs CPU (same seed, 3 chunks) ===");
    let g1 = WorldGenerator::new(SEED as u32);
    let g2 = WorldGenerator::new(SEED as u32);
    let coords = [
        ChunkCoord::new(0, 0, 0),
        ChunkCoord::new(2, 1, -3),
        ChunkCoord::new(-5, -2, 7),
    ];
    let mut all_match = true;
    for c in coords {
        let a = g1.generate_chunk(c);
        let b = g2.generate_chunk(c);
        let m = a.blocks == b.blocks;
        if !m { all_match = false; }
        println!("  chunk {:?}: blocks identical: {}", c, m);
    }
    println!("  -> all match: {}", all_match);
}

fn density_distribution() {
    println!("\n=== sanity: density across vertical range ===");
    let g = WorldGenerator::new(SEED as u32);
    let cs = unendlich_engine::constants::CHUNK_SIZE;
    println!("  chunk_y |  solid blocks (out of {})", CHUNK_VOLUME);
    println!("  --------|--------");
    for cy in [-4, -2, 0, 1, 2, 3, 4, 6, 8] {
        let chunk = g.generate_chunk(ChunkCoord::new(0, cy, 0));
        let s = count_solid(&chunk.blocks);
        let pct = 100.0 * s as f64 / CHUNK_VOLUME as f64;
        let world_y_range = (cy * cs, (cy + 1) * cs);
        println!("  {:>7} | {:>6}  ({:>5.1}%)  [world y {}..{}]",
                 cy, s, pct, world_y_range.0, world_y_range.1);
    }
}

fn biome_registry_tour() {
    println!("\n=== biome registry tour (which biome wins at each climate point?) ===");
    let registry = match unendlich_engine::world::biome_loader::BiomeRegistry::load_standard() {
        Ok(r) => r,
        Err(e) => {
            println!("  (registry unavailable: {})", e);
            return;
        }
    };
    println!("  Loaded {} biomes:", registry.len());
    let mut all: Vec<_> = registry.iter().collect();
    all.sort_by(|a, b| {
        a.temperature.partial_cmp(&b.temperature)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.downfall.partial_cmp(&b.downfall).unwrap_or(std::cmp::Ordering::Equal))
    });
    for b in &all {
        println!("    {:<40}  T={:.2}  D={:.2}  -> legacy {:?}",
            b.id, b.temperature, b.downfall, b.to_legacy());
    }

    // Probe a 5x5 climate grid to see which biome wins where.
    println!("\n  Best-fit at climate grid (T in [0,2], D in [0,1]):");
    println!("           D=0.0          D=0.25         D=0.5          D=0.75         D=1.0");
    for ti in 0..=4 {
        let t = ti as f64 * 0.5;
        print!("  T={:.1} ", t);
        for di in 0..=4 {
            let d = di as f64 * 0.25;
            let b = registry.best_fit(t, d).unwrap();
            // Print the namespace:short_id, abbreviated.
            let short = b.id.split(':').nth(1).unwrap_or(&b.id);
            let truncated: String = short.chars().take(14).collect();
            print!(" {:<14}", truncated);
        }
        println!();
    }
}

fn biome_distribution() {
    println!("\n=== biome distribution (1024x1024 block surface samples, every 16) ===");
    // Load the registry so biome lookups use best-fit against the JSON files.
    let registry = match unendlich_engine::world::biome_loader::BiomeRegistry::load_standard() {
        Ok(r) => {
            println!("  (using biome registry: {} biomes loaded)", r.len());
            Some(std::sync::Arc::new(r))
        }
        Err(e) => {
            println!("  (registry unavailable: {} — using legacy lookup)", e);
            None
        }
    };
    let g = match registry {
        Some(r) => WorldGenerator::with_biome_registry(SEED as u32, r),
        None    => WorldGenerator::new(SEED as u32),
    };

    // Track both legacy enum (for back-compat reporting) and registry biome ID.
    let mut legacy_counts: std::collections::HashMap<_, u32> = std::collections::HashMap::new();
    let mut registry_counts: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    let mut total = 0u32;
    // Wider sample range to expose climate variation.
    for z in (-512..512).step_by(16) {
        for x in (-512..512).step_by(16) {
            let sample = g.biome_at_3d(x, SEA_LEVEL, z);
            *legacy_counts.entry(sample.biome).or_insert(0) += 1;
            if let Some(def) = g.biome_def_at(x, SEA_LEVEL, z) {
                *registry_counts.entry(def.id.clone()).or_insert(0) += 1;
            }
            total += 1;
        }
    }

    println!("\n  Legacy enum distribution:");
    let mut sorted: Vec<_> = legacy_counts.into_iter().collect();
    sorted.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
    for (biome, count) in sorted {
        let pct = 100.0 * count as f64 / total as f64;
        println!("    {:>22?}  {:>6}  ({:>5.1}%)", biome, count, pct);
    }

    if !registry_counts.is_empty() {
        println!("\n  Registry biome distribution:");
        let mut sorted: Vec<_> = registry_counts.into_iter().collect();
        sorted.sort_by_key(|(_, c)| std::cmp::Reverse(*c));
        for (id, count) in sorted {
            let pct = 100.0 * count as f64 / total as f64;
            println!("    {:<40}  {:>6}  ({:>5.1}%)", id, count, pct);
        }
    }
}

fn main() {
    env_logger::Builder::from_default_env()
        .filter_level(log::LevelFilter::Info)
        .init();

    println!("Meinkraft world-gen benchmark (3D, 32^3 chunks)");
    println!("Seed: 0x{:016X}", SEED);

    determinism_check();
    density_distribution();
    biome_registry_tour();
    biome_distribution();

    for r in [2, 3, 4] {
        run_for_radius(r);
    }

    println!("\nDone.");
}
