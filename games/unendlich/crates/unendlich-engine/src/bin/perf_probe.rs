//! `perf-probe` — surgical, headless, quick-turnaround timing of the CPU-side
//! render-prep pipeline, swept over a render-distance matrix. No window, no GPU
//! surface: builds fine with zero features and runs anywhere (CI, llvmpipe box,
//! the M2), so it is the fast feedback loop for worldgen/octree perf candidates.
//!
//! Stages timed per chunk, per shell radius:
//!   1. worldgen   — `WorldGenerator::generate_chunk` (the dominant CPU cost)
//!   2. octree     — `OctreeArray::from_chunk` (sparse encode)
//!   3. roundtrip  — `OctreeArray::to_chunk` (decode; also verifies correctness)
//!
//! Output is CSV on stdout (machine-diffable across candidate branches) plus a
//! short human summary on stderr. Deterministic seed.
//!
//! Usage:  perf-probe [radii] [--repeats N]
//!   radii    comma list of shell radii to sweep   (default "1,2,3")
//!   --repeats  measured passes per radius, min kept (default 1)
//! e.g.  perf-probe 1,2,3,4 --repeats 3

use std::time::Instant;
use unendlich_engine::constants::{CHUNK_SIZE, SEA_LEVEL};
use unendlich_engine::world::cpu_generator::WorldGenerator;
use unendlich_engine::world::octree::OctreeArray;
use unendlich_engine::world::ChunkCoord;

const SEED: u32 = 0xCAFE_F00D;

fn shell(radius: i32) -> Vec<ChunkCoord> {
    let sea_chunk_y = (SEA_LEVEL as i32) / (CHUNK_SIZE as i32);
    let mut out = Vec::new();
    for dx in -radius..=radius {
        for dy in -radius..=radius {
            for dz in -radius..=radius {
                out.push(ChunkCoord::new(dx, sea_chunk_y + dy, dz));
            }
        }
    }
    out
}

struct StageTimes {
    worldgen_us: f64,
    octree_us: f64,
    roundtrip_us: f64,
    nodes: usize,
    octree_bytes: usize,
    rt_failures: usize,
}

fn measure(coords: &[ChunkCoord]) -> StageTimes {
    let gen = WorldGenerator::new(SEED);

    // 1. worldgen
    let t = Instant::now();
    let chunks: Vec<_> = coords.iter().map(|&c| gen.generate_chunk(c)).collect();
    let worldgen_us = t.elapsed().as_micros() as f64;

    // 2. octree encode + 3. roundtrip (+ correctness)
    let mut octree_us = 0.0;
    let mut roundtrip_us = 0.0;
    let mut nodes = 0usize;
    let mut octree_bytes = 0usize;
    let mut rt_failures = 0usize;
    for chunk in &chunks {
        let t1 = Instant::now();
        let oct = OctreeArray::from_chunk(chunk);
        octree_us += t1.elapsed().as_micros() as f64;

        let t2 = Instant::now();
        let restored = oct.to_chunk();
        roundtrip_us += t2.elapsed().as_micros() as f64;
        if restored.blocks != chunk.blocks {
            rt_failures += 1;
        }
        nodes += oct.node_count();
        octree_bytes += oct.byte_size();
    }

    StageTimes { worldgen_us, octree_us, roundtrip_us, nodes, octree_bytes, rt_failures }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut radii: Vec<i32> = vec![1, 2, 3];
    let mut repeats = 1usize;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--repeats" => {
                if let Some(v) = args.get(i + 1).and_then(|s| s.parse().ok()) {
                    repeats = v;
                }
                i += 2;
            }
            other => {
                let parsed: Vec<i32> = other.split(',').filter_map(|s| s.trim().parse().ok()).collect();
                if !parsed.is_empty() {
                    radii = parsed;
                }
                i += 1;
            }
        }
    }
    let repeats = repeats.max(1);

    eprintln!("perf-probe — CPU render-prep pipeline (seed {SEED:#x}, {CHUNK_SIZE}^3 chunks, repeats {repeats})");
    // CSV header
    println!("radius,chunks,stage,total_ms,per_chunk_us,chunks_per_sec,extra");

    for &r in &radii {
        let coords = shell(r);
        let n = coords.len();
        // warmup (caches, allocator) — discarded
        let _ = measure(&coords);

        // keep the fastest of `repeats` passes (least noisy)
        let mut best: Option<StageTimes> = None;
        for _ in 0..repeats {
            let m = measure(&coords);
            best = Some(match best {
                Some(b) if b.worldgen_us <= m.worldgen_us => b,
                _ => m,
            });
        }
        let m = best.expect("at least one pass");

        let per = |total_us: f64| total_us / n as f64;
        let cps = |total_us: f64| if total_us > 0.0 { n as f64 / (total_us / 1e6) } else { 0.0 };

        println!("{r},{n},worldgen,{:.3},{:.1},{:.0},", m.worldgen_us / 1000.0, per(m.worldgen_us), cps(m.worldgen_us));
        println!("{r},{n},octree,{:.3},{:.1},{:.0},nodes={}", m.octree_us / 1000.0, per(m.octree_us), cps(m.octree_us), m.nodes);
        println!("{r},{n},roundtrip,{:.3},{:.1},{:.0},rt_fail={}", m.roundtrip_us / 1000.0, per(m.roundtrip_us), cps(m.roundtrip_us), m.rt_failures);
        let total_us = m.worldgen_us + m.octree_us + m.roundtrip_us;
        let dense = n as f64 * (CHUNK_SIZE as f64).powi(3);
        let ratio = if dense > 0.0 { m.octree_bytes as f64 / dense } else { 0.0 };
        println!("{r},{n},TOTAL,{:.3},{:.1},{:.0},octree_ratio={:.3}", total_us / 1000.0, per(total_us), cps(total_us), ratio);

        eprintln!(
            "  r={r:<2} {n:>4} chunks | worldgen {:>6.1}us/ch ({:>5.0} ch/s) | octree {:>5.1}us/ch | rt {:>5.1}us/ch | rt_fail {}",
            per(m.worldgen_us), cps(m.worldgen_us), per(m.octree_us), per(m.roundtrip_us), m.rt_failures
        );
    }
}
