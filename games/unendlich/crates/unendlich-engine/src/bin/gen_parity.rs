//! gen_parity — is the GPU terrain generator's output the same before and after a shader
//! change, and how far is it from the CPU generator?
//!
//!   gen_parity [n_side]        (default 4 → a (2·n_side)² × 4-high block of chunks, 256 chunks)
//!
//! Prints, for the GPU-forced backend: a hash of every block id it produced (run before and
//! after a shader change: equal hash = bit-exact), the wall time per chunk, and the number of
//! voxels that differ from the CPU generator on the same coordinates. The GPU path is
//! designed for CPU parity (the r607/r611/r613 notes in the shader), so the mismatch count is
//! the parity number a change must not raise. Runs on any Vulkan device, lavapipe included —
//! the value is the comparison, not the speed.
use std::time::Instant;
use unendlich_engine::world::cpu_generator::WorldGenerator;
use unendlich_engine::world::terrain_generator::{Backend, TerrainGenerator};
use unendlich_engine::world::ChunkCoord;

const SEED: u64 = 1337;

fn fnv(h: &mut u64, b: u8) { *h ^= b as u64; *h = h.wrapping_mul(0x100000001b3); }

fn main() {
    let n: i32 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(4);
    let mut coords = Vec::new();
    for cz in -n..n { for cx in -n..n { for cy in 0..4 { coords.push(ChunkCoord { x: cx, y: cy, z: cz }); } } }

    let gpu = match TerrainGenerator::new(SEED, Backend::Gpu, 16) {
        Ok(g) => g,
        Err(e) => { eprintln!("gen_parity: GPU backend unavailable: {e}"); std::process::exit(2); }
    };
    let _ = gpu.generate(&coords[..4]); // pipeline warm-up
    let t0 = Instant::now();
    let gpu_chunks = gpu.generate(&coords);
    let gpu_s = t0.elapsed().as_secs_f64();

    let cpu = WorldGenerator::new(SEED as u32);
    let t1 = Instant::now();
    let cpu_chunks: Vec<_> = coords.iter().map(|&c| cpu.generate_chunk(c)).collect();
    let cpu_s = t1.elapsed().as_secs_f64();

    let mut h: u64 = 0xcbf29ce484222325;
    let mut mismatch = 0usize; let mut total = 0usize; let mut solid = 0usize;
    for (g, c) in gpu_chunks.iter().zip(&cpu_chunks) {
        assert_eq!(g.coord, c.coord);
        for (a, b) in g.blocks.iter().zip(&c.blocks) {
            let ga = *a as u16; fnv(&mut h, (ga & 0xff) as u8); fnv(&mut h, (ga >> 8) as u8);
            total += 1; if a != b { mismatch += 1; } if *a != unendlich_engine::world::blocks::BlockId::Air { solid += 1; }
        }
    }
    println!("gen_parity: {} chunks | GPU blocks hash {h:016x} | solid {solid} | GPU {:.2} ms/chunk | CPU {:.2} ms/chunk (1 thread) | GPU≠CPU voxels {mismatch} of {total} ({:.3}%)",
        coords.len(), gpu_s * 1000.0 / coords.len() as f64, cpu_s * 1000.0 / coords.len() as f64,
        mismatch as f64 * 100.0 / total.max(1) as f64);
}
