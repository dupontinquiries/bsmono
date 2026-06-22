//! forest_codec_bench — REAL generator + REAL octree, isolating forested
//! surface chunks, to test the synthetic conclusion (lzma already crushes the
//! octree → neural compression isn't the win).
//!
//! Pass 1: scan a grid of chunks around sea level, classify each by foliage
//! (wood+leaf) and solidity. Pass 2: regenerate the most-forested chunk + a
//! bare surface chunk + sky + underground, build the real OctreeArray, print
//! node_count/byte_size, and dump dense block bytes to /tmp for the Python
//! lzma/gzip/bz2 race. Deterministic, so pass-2 regen == pass-1 content.
//!
//! Run: cargo run --release --no-default-features -p unendlich-engine \
//!        --example forest_codec_bench

use std::io::Write;
use unendlich_engine::constants::{CHUNK_SIZE, CHUNK_VOLUME, SEA_LEVEL};
use unendlich_engine::world::blocks::BlockId;
use unendlich_engine::world::cpu_generator::WorldGenerator;
use unendlich_engine::world::octree::OctreeArray;
use unendlich_engine::world::ChunkCoord;

const SEED: u32 = 0xCAFE_F00D;
const DENSE_BYTES: usize = CHUNK_VOLUME; // BlockId is repr(u8): 1 byte/cell = 32 KiB

fn is_wood(b: BlockId) -> bool {
    matches!(b, BlockId::OakLog | BlockId::BirchLog)
}
fn is_leaf(b: BlockId) -> bool {
    matches!(b, BlockId::OakLeaves | BlockId::BirchLeaves)
}

#[derive(Clone, Copy)]
struct Stat {
    coord: ChunkCoord,
    foliage: i32,
    solid: i32,
}

fn measure_dump(gen: &WorldGenerator, coord: ChunkCoord, label: &str, csv: &mut String) {
    let chunk = gen.generate_chunk(coord);
    let oct = OctreeArray::from_chunk(&chunk);
    let nodes = oct.node_count();
    let obytes = oct.byte_size();
    let (mut wood, mut leaf, mut solid) = (0i32, 0i32, 0i32);
    for &b in &chunk.blocks {
        if b != BlockId::Air {
            solid += 1;
        }
        if is_wood(b) {
            wood += 1;
        }
        if is_leaf(b) {
            leaf += 1;
        }
    }
    // dump dense bytes (BlockId repr(u8))
    let bytes: Vec<u8> = chunk.blocks.iter().map(|&b| b as u8).collect();
    let path = format!("/tmp/ue_{}.bin", label);
    std::fs::File::create(&path)
        .unwrap()
        .write_all(&bytes)
        .unwrap();

    let ratio = DENSE_BYTES as f64 / obytes.max(1) as f64;
    println!(
        "  {:<14} coord=({:>3},{:>2},{:>3})  wood={:<5} leaf={:<5} solid={:<6}  \
         octree: {:>6} nodes / {:>6.1} KB  (dense {:.1} KB, {:.2}x)",
        label,
        coord.x,
        coord.y,
        coord.z,
        wood,
        leaf,
        solid,
        nodes,
        obytes as f64 / 1024.0,
        DENSE_BYTES as f64 / 1024.0,
        ratio
    );
    csv.push_str(&format!(
        "{},{},{},{},{},{},{}\n",
        label, nodes, obytes, DENSE_BYTES, wood, leaf, solid
    ));
}

fn main() {
    let gen = WorldGenerator::new(SEED);
    let sea_cy = SEA_LEVEL / CHUNK_SIZE;

    // ---- pass 1: scan grid, find the most-forested + a bare surface chunk ----
    let r = 9;
    let mut best_forest = Stat {
        coord: ChunkCoord::new(0, sea_cy, 0),
        foliage: -1,
        solid: 0,
    };
    let mut best_bare = Stat {
        coord: ChunkCoord::new(0, sea_cy, 0),
        foliage: i32::MAX,
        solid: -1,
    };
    let mut scanned = 0;
    let mut with_foliage = 0;
    for cz in -r..r {
        for cx in -r..r {
            for cy in (sea_cy - 1)..=(sea_cy + 1) {
                let coord = ChunkCoord::new(cx, cy, cz);
                let chunk = gen.generate_chunk(coord);
                let (mut foliage, mut solid) = (0i32, 0i32);
                for &b in &chunk.blocks {
                    if b != BlockId::Air {
                        solid += 1;
                    }
                    if is_wood(b) || is_leaf(b) {
                        foliage += 1;
                    }
                }
                scanned += 1;
                if foliage > 0 {
                    with_foliage += 1;
                }
                if foliage > best_forest.foliage {
                    best_forest = Stat { coord, foliage, solid };
                }
                // bare = surface-ish (some solid, near-zero foliage), most solid
                if foliage <= 2 && solid > best_bare.solid && solid < CHUNK_VOLUME as i32 {
                    best_bare = Stat { coord, foliage, solid };
                }
            }
        }
    }
    println!(
        "scanned {} chunks ({} had foliage). most-forested foliage={}",
        scanned, with_foliage, best_forest.foliage
    );

    // ---- pass 2: measure + dump the interesting chunks ----
    let mut csv = String::from("label,nodes,octree_bytes,dense_bytes,wood,leaf,solid\n");
    println!("\nreal octree on real generated chunks:");
    measure_dump(&gen, ChunkCoord::new(0, sea_cy + 6, 0), "sky", &mut csv); // high = air
    measure_dump(&gen, ChunkCoord::new(0, sea_cy - 6, 0), "underground", &mut csv); // deep = stone
    measure_dump(&gen, best_bare.coord, "bare_surface", &mut csv);
    measure_dump(&gen, best_forest.coord, "forest", &mut csv);

    std::fs::File::create("/tmp/ue_summary.csv")
        .unwrap()
        .write_all(csv.as_bytes())
        .unwrap();
    println!("\nwrote /tmp/ue_summary.csv + /tmp/ue_*.bin");
}
