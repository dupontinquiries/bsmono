//! `octree-bench` — measure the v0.24.23 OctreeArray's compression
//! ratio against dense Chunk storage on real terrain.
//!
//! Generates a cubic shell of chunks around sea-level (so the mix
//! includes underground stone, surface terrain, and sky-air), encodes
//! each chunk to its sparse octree, and prints per-chunk + aggregate
//! statistics:
//!
//! - Dense byte size:   `CHUNK_VOLUME` × `size_of::<BlockId>()` = 32 KiB
//! - Octree node count: how many `OctreeNode`s the sparse encoding needs
//! - Octree byte size:  `nodes × 36 B`
//! - Compression ratio: dense ÷ octree (>1 = win, <1 = loss)
//! - Round-trip:        verify `to_chunk(from_chunk(c)) == c` for every chunk
//!
//! The output tells us roughly what to expect when the v0.24.24 GPU
//! path uploads octrees instead of dense chunks. A surface chunk that
//! compresses to 30% of dense saves a lot on the wire; an
//! all-different-cells chunk that "compresses" to 110% of dense tells
//! us where the sparse encoding loses.
//!
//! Run with:
//!     cargo run --release -p unendlich-engine --bin octree-bench
//!
//! No GPU access needed; this is a pure CPU bench.

use unendlich_engine::constants::{CHUNK_SIZE, CHUNK_VOLUME, SEA_LEVEL};
use unendlich_engine::world::cpu_generator::WorldGenerator;
use unendlich_engine::world::octree::OctreeArray;
use unendlich_engine::world::ChunkCoord;
use std::time::Instant;

const SEED: u32 = 0xCAFE_F00D;
const DENSE_BYTES: usize = CHUNK_VOLUME; // BlockId is repr(u8), one byte per cell

/// Chebyshev-radius cube of chunks centered on sea level.
fn shell(radius: i32) -> Vec<ChunkCoord> {
    let sea_chunk_y = SEA_LEVEL / CHUNK_SIZE;
    let side = (2 * radius + 1) as usize;
    let mut out = Vec::with_capacity(side * side * side);
    for dz in -radius..=radius {
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                out.push(ChunkCoord::new(dx, sea_chunk_y + dy, dz));
            }
        }
    }
    out
}

fn classify(coord: ChunkCoord) -> &'static str {
    let sea_chunk_y = SEA_LEVEL / CHUNK_SIZE;
    if coord.y < sea_chunk_y - 1 {
        "underground"
    } else if coord.y > sea_chunk_y + 1 {
        "sky       "
    } else {
        "surface   "
    }
}

fn main() {
    let radius: i32 = std::env::args()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(2);

    println!("octree-bench v0.24.23-octree-foundation");
    println!("  seed:        0x{:08X}", SEED);
    println!("  chunk size:  {}³ = {} cells", CHUNK_SIZE, CHUNK_VOLUME);
    println!("  dense bytes: {} (BlockId is repr(u8))", DENSE_BYTES);
    println!("  node bytes:  40 (OctreeNode: packed_header + last_modified_tick + 8 child indices)");
    println!("  shell radius: {} (= {}³ = {} chunks)",
        radius, 2 * radius + 1, (2 * radius + 1).pow(3));
    println!();

    let coords = shell(radius);
    let gen = WorldGenerator::new(SEED);

    println!("Generating {} chunks...", coords.len());
    let t0 = Instant::now();
    let chunks: Vec<_> = coords.iter().map(|&c| gen.generate_chunk(c)).collect();
    let gen_secs = t0.elapsed().as_secs_f64();
    println!("  generated in {:.2}s ({:.0} chunks/sec)",
        gen_secs, coords.len() as f64 / gen_secs);
    println!();

    println!("Encoding to octree + round-trip verification...");
    println!("  {:>8} {:>10}  {:>8} {:>10} {:>8}  {:>4}  class",
        "coord", "nodes", "bytes", "ratio", "build_us", "rt");
    println!("  {}", "-".repeat(72));

    let mut total_nodes: usize = 0;
    let mut total_octree_bytes: usize = 0;
    let mut total_build_us: u128 = 0;
    let mut worst_chunk: Option<(ChunkCoord, usize)> = None;
    let mut best_chunk: Option<(ChunkCoord, usize)> = None;
    let mut roundtrip_failures: usize = 0;

    // Print every chunk for small radii, every Nth for larger.
    let print_stride = (coords.len() / 64).max(1);

    for (i, chunk) in chunks.iter().enumerate() {
        let t1 = Instant::now();
        let oct = OctreeArray::from_chunk(chunk);
        let build_us = t1.elapsed().as_micros();

        let restored = oct.to_chunk();
        let rt_ok = restored.blocks == chunk.blocks;
        if !rt_ok {
            roundtrip_failures += 1;
        }

        total_nodes += oct.node_count();
        total_octree_bytes += oct.byte_size();
        total_build_us += build_us;

        let nodes = oct.node_count();
        match best_chunk {
            None => best_chunk = Some((chunk.coord, nodes)),
            Some((_, b)) if nodes < b => best_chunk = Some((chunk.coord, nodes)),
            _ => {}
        }
        match worst_chunk {
            None => worst_chunk = Some((chunk.coord, nodes)),
            Some((_, w)) if nodes > w => worst_chunk = Some((chunk.coord, nodes)),
            _ => {}
        }

        if i % print_stride == 0 {
            let ratio = DENSE_BYTES as f64 / oct.byte_size() as f64;
            println!("  ({:>2},{:>2},{:>2}) {:>10}  {:>8} {:>9.2}x {:>7}us  {:>4}  {}",
                chunk.coord.x, chunk.coord.y, chunk.coord.z,
                nodes,
                oct.byte_size(),
                ratio,
                build_us,
                if rt_ok { "OK" } else { "FAIL" },
                classify(chunk.coord),
            );
        }
    }

    println!();
    println!("Aggregate:");
    let dense_total = DENSE_BYTES * coords.len();
    let ratio_total = dense_total as f64 / total_octree_bytes as f64;
    println!("  total chunks:       {}", coords.len());
    println!("  total dense bytes:  {} ({:.1} MiB)",
        dense_total, dense_total as f64 / 1_048_576.0);
    println!("  total octree bytes: {} ({:.1} MiB)",
        total_octree_bytes, total_octree_bytes as f64 / 1_048_576.0);
    println!("  total nodes:        {}", total_nodes);
    println!("  mean nodes/chunk:   {:.1}", total_nodes as f64 / coords.len() as f64);
    println!("  compression ratio:  {:.2}x (dense / octree)", ratio_total);
    println!("  total build time:   {:.2}s ({:.1} us/chunk avg)",
        total_build_us as f64 / 1e6,
        total_build_us as f64 / coords.len() as f64);
    if let Some((coord, n)) = best_chunk {
        println!("  best (fewest):      ({},{},{}) → {} nodes ({} B)",
            coord.x, coord.y, coord.z, n, n * 40);
    }
    if let Some((coord, n)) = worst_chunk {
        println!("  worst (most):       ({},{},{}) → {} nodes ({} B)",
            coord.x, coord.y, coord.z, n, n * 40);
    }
    println!("  roundtrip failures: {}", roundtrip_failures);

    if roundtrip_failures > 0 {
        eprintln!();
        eprintln!("FAIL: {} chunks did not round-trip. The octree encoding",
            roundtrip_failures);
        eprintln!("is lossy or corrupted; do not ship this version.");
        std::process::exit(1);
    }

    println!();
    println!("All {} chunks round-tripped successfully.", coords.len());
    if ratio_total >= 1.0 {
        println!("Octree representation is {:.1}% the size of dense.",
            100.0 / ratio_total);
    } else {
        println!("WARNING: octree is {:.1}% LARGER than dense.",
            (1.0 / ratio_total - 1.0) * 100.0);
        println!("This is expected for high-entropy chunks (lots of");
        println!("unique block IDs per cell). Compression wins land");
        println!("on chunks with homogeneous regions: deep stone,");
        println!("clear sky, large water bodies.");
    }

    // v0.24.23 follow-up: incremental edit cost benchmark.
    // Compares the cost of `set_block` (incremental, with leaf
    // expansion) vs `from_chunk` (full rebuild).
    //
    // Edits are scattered across the chunk to force tree expansion
    // in many subtrees; this is the worst case for incremental cost
    // (and the best case for full rebuild relative to incremental,
    // since the chunk ends up well-decomposed after the edits).
    println!();
    println!("=== Edit-cost bench (incremental set_block vs full from_chunk) ===");
    let bench_chunks = std::cmp::min(8, coords.len());
    let edit_count: i32 = 100;
    let mut incremental_us_total: u128 = 0;
    let mut rebuild_us_total: u128 = 0;
    for i in 0..bench_chunks {
        let original_chunk = chunks[i].clone();
        let starting_octree = OctreeArray::from_chunk(&original_chunk);
        // Scattered edit positions across the 32³ chunk.
        let edits: Vec<(i32, i32, i32, unendlich_engine::world::blocks::BlockId)> = (0..edit_count)
            .map(|i| {
                let x = (i * 7 + 3).rem_euclid(CHUNK_SIZE) as i32;
                let y = (i * 11 + 5).rem_euclid(CHUNK_SIZE) as i32;
                let z = (i * 13 + 17).rem_euclid(CHUNK_SIZE) as i32;
                let block = if i % 2 == 0 {
                    unendlich_engine::world::blocks::BlockId::Stone
                } else {
                    unendlich_engine::world::blocks::BlockId::Dirt
                };
                (x, y, z, block)
            })
            .collect();

        // Incremental: clone the starting octree, apply edits via set_block.
        let mut incr_octree = starting_octree.clone();
        let t0 = Instant::now();
        for (i, &(x, y, z, block)) in edits.iter().enumerate() {
            incr_octree.set_block(x, y, z, block, (i + 1) as u32);
        }
        incremental_us_total += t0.elapsed().as_micros();

        // Full rebuild: apply edits to dense chunk, then from_chunk
        // for every edit (this is the "before propagation" cost).
        let mut rebuild_chunk = original_chunk.clone();
        let t0 = Instant::now();
        for &(x, y, z, block) in &edits {
            rebuild_chunk.set(x, y, z, block);
            let _ = OctreeArray::from_chunk(&rebuild_chunk);
        }
        rebuild_us_total += t0.elapsed().as_micros();
    }
    let incremental_per_edit = incremental_us_total as f64 / (bench_chunks as i32 * edit_count) as f64;
    let rebuild_per_edit = rebuild_us_total as f64 / (bench_chunks as i32 * edit_count) as f64;
    println!("  chunks tested:        {} × {} edits each", bench_chunks, edit_count);
    println!("  incremental per edit: {:.2} µs", incremental_per_edit);
    println!("  rebuild per edit:     {:.2} µs", rebuild_per_edit);
    if incremental_per_edit > 0.0 {
        println!("  speedup:              {:.1}× ({}× cheaper)",
            rebuild_per_edit / incremental_per_edit,
            (rebuild_per_edit / incremental_per_edit) as u32);
    }
}
