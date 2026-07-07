//! noise_bench (r196) — times the SoA worldgen noise hot path. Compare
//! baseline vs RUSTFLAGS="-C target-cpu=native" to see autovectorization
//! headroom (the real perf lever fastmath analysis pointed to).
use std::time::Instant;
use unendlich_engine::world::cpu_generator::WorldGenerator;
fn main() {
    let gen = WorldGenerator::new(1337);
    let cs = unendlich_engine::constants::CHUNK_SIZE;
    use unendlich_engine::world::chunk::ChunkCoord;
    // warm
    let _ = gen.generate_chunk(ChunkCoord::new(0, 2, 0));
    let t = Instant::now();
    let mut acc = 0u64;
    for i in 0..64 {
        let c = gen.generate_chunk(ChunkCoord::new(i % 8, 2, i / 8));
        acc += c.blocks.len() as u64;
    }
    let dt = t.elapsed();
    std::hint::black_box(acc);
    println!("64 chunks gen: {:.1}ms ({:.2}ms/chunk), cs={}",
        dt.as_secs_f64()*1e3, dt.as_secs_f64()*1e3/64.0, cs);
}
