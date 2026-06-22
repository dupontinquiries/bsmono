//! Headless geometry-bounds probe (fast path via the lib's public API).
use unendlich_engine::world::chunk::{Chunk, ChunkCoord};
use unendlich_engine::world::blocks::BlockId;
use unendlich_engine::render::mesher::{self, ChunkMesh, PackedVertex, mesh_chunk_greedy};
use unendlich_engine::world::world_model::WorldModel;
use unendlich_engine::sky::Sky;
use std::sync::Arc;

/// Replicate the GPU unpack (chunk.vert): local = (bits/1023)*32, world = origin+local.
fn gpu_unpack(pos_packed: u32, origin: [f32; 3]) -> [f32; 3] {
    let r = (pos_packed & 0x3ff) as f32 / 1023.0;
    let g = ((pos_packed >> 10) & 0x3ff) as f32 / 1023.0;
    let b = ((pos_packed >> 20) & 0x3ff) as f32 / 1023.0;
    [origin[0] + r * 32.0, origin[1] + g * 32.0, origin[2] + b * 32.0]
}

/// Pack every vertex (from_world) then GPU-unpack; report round-trip error and
/// any giant triangle edge in the reconstructed positions.
fn roundtrip(name: &str, mesh: &ChunkMesh, origin: (i32, i32, i32)) {
    let of = [origin.0 as f32, origin.1 as f32, origin.2 as f32];
    let mut max_err = 0.0f32;
    let recon: Vec<[f32; 3]> = mesh.vertices.iter().map(|v| {
        let pv = PackedVertex::from_world(v, of);
        let u = gpu_unpack(pv.pos_packed, of);
        for k in 0..3 { max_err = max_err.max((u[k] - v.position[k]).abs()); }
        u
    }).collect();
    // giant-edge scan over triangles (index triples)
    let mut giant = 0usize; let mut worst = 0.0f32;
    for t in mesh.indices.chunks_exact(3) {
        let (a, b, c) = (recon[t[0] as usize], recon[t[1] as usize], recon[t[2] as usize]);
        for (p, q) in [(a, b), (b, c), (c, a)] {
            let d = ((p[0]-q[0]).powi(2)+(p[1]-q[1]).powi(2)+(p[2]-q[2]).powi(2)).sqrt();
            worst = worst.max(d);
            if d > 50.0 { giant += 1; }
        }
    }
    println!("[{name}] roundtrip max_err={max_err:.4} worst_edge={worst:.1} giant_edges(>50)={giant}");
}

fn varied_chunk(coord: ChunkCoord) -> Chunk {
    let mut c = Chunk::empty(coord);
    for x in 0..32i32 {
        for z in 0..32i32 {
            let h = 4 + ((x / 3 + z / 5) % 12);
            for y in 0..h {
                let b = if y < h - 3 { BlockId::Stone }
                        else if y < h - 1 { BlockId::Dirt }
                        else { BlockId::Grass };
                c.set(x, y, z, b);
            }
        }
    }
    c
}

fn report(name: &str, mesh: &ChunkMesh, origin: (i32, i32, i32)) -> bool {
    let (ox, oy, oz) = origin;
    let lo = [ox as f32 - 4.0, oy as f32 - 4.0, oz as f32 - 4.0];
    let hi = [ox as f32 + 36.0, oy as f32 + 36.0, oz as f32 + 36.0];
    let (mut mnx, mut mny, mut mnz) = (f32::MAX, f32::MAX, f32::MAX);
    let (mut mxx, mut mxy, mut mxz) = (f32::MIN, f32::MIN, f32::MIN);
    let mut bad = 0usize; let mut sample = None;
    for v in &mesh.vertices {
        let p = v.position;
        mnx = mnx.min(p[0]); mny = mny.min(p[1]); mnz = mnz.min(p[2]);
        mxx = mxx.max(p[0]); mxy = mxy.max(p[1]); mxz = mxz.max(p[2]);
        let oob = p.iter().any(|c| !c.is_finite())
            || p[0]<lo[0]||p[0]>hi[0]||p[1]<lo[1]||p[1]>hi[1]||p[2]<lo[2]||p[2]>hi[2];
        if oob { bad+=1; if sample.is_none(){sample=Some(p);} }
    }
    println!("[{name}] verts={} bbox=([{mnx:.1},{mny:.1},{mnz:.1}]..[{mxx:.1},{mxy:.1},{mxz:.1}]) OOB={bad} sample={sample:?}", mesh.vertices.len());
    bad>0
}

fn main() {
    // Small origin (near world center) vs large origins (player far out) — the
    // fp32 catastrophic-cancellation / precision regime Asahi Mesa would expose.
    // The LIVE renderer path: mesh_chunk_greedy with Some(sky), Some(world_model),
    // Some(tick) — exercises emit_subdivided_top/emit_jittered_top (the per-block
    // height-jitter top faces my None-probe skipped).
    let sky = Sky::new();
    let wm = Arc::new(WorldModel::new(1234));
    for &(cx, cy, cz, label) in &[
        (2i32, 0i32, 3i32, "LIVE_near"),
        (32768, 0, 32768, "LIVE_far_1.05M"),
    ] {
        println!("=== live-path origin {label} (chunk {cx},{cy},{cz}) ===");
        let coord = ChunkCoord::new(cx, cy, cz);
        let origin = coord.block_origin();
        let c = varied_chunk(coord);
        let m = mesh_chunk_greedy(&c, Some(&sky), Some(&wm), Some(100));
        roundtrip("live_greedy", &m, origin);
        report("live_greedy", &m, origin);
    }
}

#[allow(dead_code)]
fn coord_main() {

#[allow(dead_code)]
fn old_main() {
    let origin = coord.block_origin();
    let c = varied_chunk(coord);
    let mut bad = false;
    let mc = mesher::mesh_chunk(&c);
    roundtrip("mesh_chunk", &mc, origin);
    roundtrip("naive", &mesher::mesh_chunk_naive(&c), origin);
    roundtrip("downsampled_f8", &mesher::mesh_chunk_downsampled(&c, 8), origin);
    bad |= report("mesh_chunk", &mc, origin);
    bad |= report("naive", &mesher::mesh_chunk_naive(&c), origin);
    bad |= report("far", &mesher::mesh_chunk_far(&c), origin);
    bad |= report("very_far", &mesher::mesh_chunk_very_far(&c), origin);
    for f in [1,2,4,8,16] {
        bad |= report(&format!("downsampled_f{f}"), &mesher::mesh_chunk_downsampled(&c, f), origin);
    }
    println!("ANY_OOB={bad}");
}
