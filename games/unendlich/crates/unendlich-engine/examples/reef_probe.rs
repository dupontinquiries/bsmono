use std::sync::Arc;
use unendlich_engine::world::chunk::ChunkCoord;
use unendlich_engine::world::cpu_generator::WorldGenerator;
use unendlich_engine::world::ticks::{TickId, TickRegistry};
fn main() {
    let gen = WorldGenerator::new(1337);
    let bx: i32 = std::env::var("REEF_X").ok().and_then(|v| v.parse().ok()).unwrap_or(-4480);
    let bz: i32 = std::env::var("REEF_Z").ok().and_then(|v| v.parse().ok()).unwrap_or(-6400);
    let cs = unendlich_engine::constants::CHUNK_SIZE;
    let sy = gen.surface_y_at(bx, bz, -64, 200).unwrap_or(64);
    println!("floor y={} sea={:.1}", sy, gen.sea_level_at(bx, bz));
    for y in [51, 55, 60, 63] {
        println!("density({},{},{}) = {:.4} (cave {})", bx, y, bz,
            gen.density(bx, y, bz),
            gen.is_cave(bx, y, bz) as u8);
    }
    let coord = ChunkCoord::new(bx.div_euclid(cs), sy.div_euclid(cs), bz.div_euclid(cs));
    let chunk = gen.generate_chunk(coord);
    // column census at local (0,0)
    use unendlich_engine::world::blocks::BlockId;
    use unendlich_engine::world::chunk::Chunk;
    for ly in (0..cs).rev() {
        let b = chunk.blocks[Chunk::block_index(0, ly, 0)];
        if ly > cs - 6 || b != BlockId::Air {
            println!("ly={} {:?}", ly, b);
        }
        if b != BlockId::Air && b != BlockId::Water { break; }
    }
    let chunks = dashmap::DashMap::new();
    chunks.insert(coord, Arc::new(chunk));
    let mut reg = TickRegistry::new();
    reg.register("reef_seeder", unendlich_engine::world::reef_seeder::reef_handler());
    let mut total = 0;
    for t in 0..1024u64 {
        let e = reg.run_tick(&chunks, None, None, Some(coord), TickId(t), 0.5, 0.5, None, None);
        total += e.block_edits.len();
    }
    println!("reef edits over one full period: {total}");
}
