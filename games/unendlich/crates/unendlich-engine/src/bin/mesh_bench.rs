//! Measure naive vs greedy mesh sizes across a representative chunk grid.
//!
//! Not a benchmark of timing — just vertex/index/quad counts. This tells us
//! the actual upload-bandwidth and draw-call savings the renderer will see.
//!
//! Run with: `cargo run --release --bin mesh-bench --features render`

use unendlich_engine::constants::WORLD_SEED_DEFAULT;
use unendlich_engine::render::mesher::{mesh_chunk_greedy, mesh_chunk_naive};
use unendlich_engine::world::chunk::ChunkCoord;
use unendlich_engine::world::cpu_generator::WorldGenerator;
use unendlich_engine::world::lod::downsample_pow2;
use std::time::Instant;

fn main() {
    let gen = WorldGenerator::new(WORLD_SEED_DEFAULT as u32);

    println!("Meinkraft mesh-bench: greedy vs naive on representative chunks\n");
    println!("  {:>6}  {:>10}  {:>10}  {:>6}  {:>6}", "chunk_y", "naive_v", "greedy_v", "ratio", "%saved");
    println!("  {:->6}  {:->10}  {:->10}  {:->6}  {:->6}", "", "", "", "", "");

    let mut total_naive = 0u64;
    let mut total_greedy = 0u64;
    let mut total_naive_time_us = 0u128;
    let mut total_greedy_time_us = 0u128;

    // r62-06: also mesh distance-LOD decimated copies (Far=2³→1, VeryFar=4³→1)
    // through the SAME greedy mesher to measure the per-tier quad reduction.
    let mut total_dec2 = 0u64;
    let mut total_dec4 = 0u64;

    // Sample chunks at different vertical levels — surface, underground, sky.
    for chunk_y in [-2i32, -1, 0, 1, 2, 3, 4] {
        let mut row_naive = 0u64;
        let mut row_greedy = 0u64;
        let mut row_dec2 = 0u64;
        let mut row_dec4 = 0u64;
        // 4×4 horizontal sample per layer.
        for cz in -2..2 {
            for cx in -2..2 {
                let chunk = gen.generate_chunk(ChunkCoord::new(cx, chunk_y, cz));

                let t = Instant::now();
                let naive = mesh_chunk_naive(&chunk);
                total_naive_time_us += t.elapsed().as_micros();

                let t = Instant::now();
                let greedy = mesh_chunk_greedy(&chunk, None, None, None);
                total_greedy_time_us += t.elapsed().as_micros();

                // decimated tiers (silhouette-preserving block downsample)
                let dec2 = mesh_chunk_greedy(&downsample_pow2(&chunk, 2), None, None, None);
                let dec4 = mesh_chunk_greedy(&downsample_pow2(&chunk, 4), None, None, None);

                row_naive  += naive.vertex_count() as u64;
                row_greedy += greedy.vertex_count() as u64;
                row_dec2   += dec2.vertex_count() as u64;
                row_dec4   += dec4.vertex_count() as u64;
            }
        }
        let ratio = if row_greedy > 0 { row_naive as f64 / row_greedy as f64 } else { 0.0 };
        // Convert to f64 BEFORE subtraction so we get a signed result;
        // u64 - u64 panics in debug / wraps in release when greedy > naive.
        // Negative pct means greedy is producing MORE vertices than naive
        // (regression — happens for chunks dense in soft blocks where
        // greedy bypasses merging on per-block side faces).
        let pct = if row_naive > 0 {
            100.0 * (row_naive as f64 - row_greedy as f64) / row_naive as f64
        } else { 0.0 };
        println!("  {:>6}  {:>10}  {:>10}  {:>5.1}x  {:>+6.1}%",
            chunk_y, row_naive, row_greedy, ratio, pct);
        total_naive += row_naive;
        total_greedy += row_greedy;
        total_dec2 += row_dec2;
        total_dec4 += row_dec4;
    }

    println!("  {:->6}  {:->10}  {:->10}  {:->6}  {:->6}", "", "", "", "", "");
    let total_ratio = if total_greedy > 0 { total_naive as f64 / total_greedy as f64 } else { 0.0 };
    let total_pct = if total_naive > 0 {
        100.0 * (total_naive as f64 - total_greedy as f64) / total_naive as f64
    } else { 0.0 };
    println!("  {:>6}  {:>10}  {:>10}  {:>5.1}x  {:>+6.1}%",
        "TOTAL", total_naive, total_greedy, total_ratio, total_pct);

    // r62-06: distance-LOD decimation report (greedy near vs decimated greedy).
    let dec2_pct = if total_greedy > 0 {
        100.0 * (total_greedy as f64 - total_dec2 as f64) / total_greedy as f64
    } else { 0.0 };
    let dec4_pct = if total_greedy > 0 {
        100.0 * (total_greedy as f64 - total_dec4 as f64) / total_greedy as f64
    } else { 0.0 };
    println!("\n  r62-06 distance-LOD decimation (greedy mesher, vertex counts):");
    println!("    tier,vertices,vs_near_pct");
    println!("    near(1),{},0.0", total_greedy);
    println!("    far(2),{},{:.1}", total_dec2, dec2_pct);
    println!("    veryfar(4),{},{:.1}", total_dec4, dec4_pct);

    println!("\n  Total mesh time: naive {:>6} µs, greedy {:>6} µs  ({:.2}x)",
        total_naive_time_us, total_greedy_time_us,
        total_naive_time_us as f64 / total_greedy_time_us.max(1) as f64);

    // Memory: each Vertex is position[3]+color[3] = 6 f32 = 24 bytes.
    let naive_mb  = (total_naive  as f64 * 24.0) / (1024.0 * 1024.0);
    let greedy_mb = (total_greedy as f64 * 24.0) / (1024.0 * 1024.0);
    let delta_mb = naive_mb - greedy_mb;
    let label = if delta_mb >= 0.0 { "saved" } else { "regression" };
    println!("  Vertex buffer size: naive {:.2} MB, greedy {:.2} MB ({:+.2} MB {})",
        naive_mb, greedy_mb, delta_mb, label);
}
