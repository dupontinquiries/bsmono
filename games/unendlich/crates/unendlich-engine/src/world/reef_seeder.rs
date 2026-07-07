//! reef_seeder — r192: fluid-POI feature slice (fluid_pois_waterfalls doc).
//! Wires the orphaned coral GENOME system and the waterfall() intensity
//! scorer into the live world via one idempotent, throttled tick handler:
//!  - PROTO-REEFS: ocean-floor sites (Water column over Sand/Gravel/Stone,
//!    depth >= 3) grow corals from CoralGenome::from_seed(site) — branch
//!    count/radius/hue are the PHENOTYPE; built from the existing palette
//!    (Bone trunks, Flower/Moss tips by hue) until dedicated coral blocks
//!    land with the texture session.
//!  - WATERFALL POIs: in-chunk ledges where a Water cell sits beside a
//!    >=4-block air drop are scored with waterfall(flow, drop); strong
//!    sites are counted + logged (carving the actual falling column needs
//!    neighbor-chunk writes — next slice).
//! Idempotent (same blocks every visit) + throttled (1/1024 ticks per
//! chunk, coord-hashed phase) so re-runs are harmless and cheap.
use crate::world::blocks::BlockId;
use crate::world::chunk::Chunk;
use crate::world::coral::CoralGenome;
use crate::world::ticks::TickContext;
use crate::world::waterfall::waterfall;

const PERIOD: u64 = 1024;
const MAX_WRITES_PER_CHUNK: usize = 60;

// r193 (guide 4.1): adopt the shared mixer. Coral layouts reshuffle once
// (pre-release cosmetic); determinism preserved per (seed, coords).
#[inline]
fn h2(a: i64, b: i64) -> u64 {
    crate::util::hash_util::mix64_2(a as u64, b as u64)
}

pub fn reef_handler() -> std::sync::Arc<dyn Fn(&mut TickContext) + Send + Sync> {
    std::sync::Arc::new(move |ctx: &mut TickContext| {
        let cs = crate::constants::CHUNK_SIZE;
        for coord in ctx.sorted_chunk_coords() {
            let phase = h2(coord.x as i64, (coord.y as i64) << 20 ^ coord.z as i64) % PERIOD;
            if ctx.tick_id.raw() % PERIOD != phase {
                continue;
            }
            let Some(entry) = ctx.chunks.get(&coord) else { continue };
            let chunk = entry.value().clone();
            drop(entry);
            let (ox, oy, oz) = (coord.x * cs, coord.y * cs, coord.z * cs);
            let mut writes = 0usize;
            let mut falls = 0u32;
            for lz in 0..cs {
                for lx in 0..cs {
                    // top-down scan of this column inside the chunk
                    let mut water_run = 0i32;
                    for ly in (0..cs).rev() {
                        let b = chunk.blocks[Chunk::block_index(lx, ly, lz)];
                        if b == BlockId::Water {
                            water_run += 1;
                            // waterfall POI: water beside an in-chunk drop
                            if lx + 1 < cs {
                                let side = chunk.blocks[Chunk::block_index(lx + 1, ly, lz)];
                                if side == BlockId::Air {
                                    let mut drop = 0i32;
                                    let mut dy = ly - 1;
                                    while dy >= 0
                                        && chunk.blocks[Chunk::block_index(lx + 1, dy, lz)]
                                            == BlockId::Air
                                    {
                                        drop += 1;
                                        dy -= 1;
                                    }
                                    if drop >= 4
                                        && waterfall(water_run as f32, drop as f32) > 0.55
                                    {
                                        falls += 1;
                                    }
                                }
                            }
                            continue;
                        }
                        // first non-water under a water run = seafloor
                        if water_run >= 3
                            // Grass/Dirt included: the surface paint still
                            // consults the un-veto'd model biome on old
                            // chunks (r192 ledger: paint-path seam).
                            && matches!(
                                b,
                                BlockId::Sand | BlockId::Gravel | BlockId::Stone
                                    | BlockId::Grass | BlockId::Dirt
                            )
                        {
                            let (wx, wy, wz) = (ox + lx, oy + ly, oz + lz);
                            let site = h2(wx as i64, (wz as i64) << 1 ^ wy as i64);
                            if site % 83 == 0 && writes < MAX_WRITES_PER_CHUNK {
                                let ph = CoralGenome::from_seed(site).express();
                                let trunk_h = (ph.branches / 3).clamp(1, 4) as i32;
                                // rgb tint -> palette pick: warm = Flower, cool = Moss
                                let tip = if ph.rgb[0] >= ph.rgb[2] { BlockId::Flower } else { BlockId::Moss };
                                for t in 1..=trunk_h {
                                    if ly + t < cs {
                                        ctx.enqueue_edit(wx, wy + t, wz, BlockId::Bone);
                                        writes += 1;
                                    }
                                }
                                // tips: small arms by radius, hue-colored
                                let r = (ph.radius / 3).clamp(1, 2) as i32;
                                for (dx, dz) in [(r, 0), (-r, 0), (0, r), (0, -r)] {
                                    let (ax, az) = (lx + dx, lz + dz);
                                    if ax >= 0 && ax < cs && az >= 0 && az < cs
                                        && ly + trunk_h < cs
                                        && writes < MAX_WRITES_PER_CHUNK
                                        && chunk.blocks
                                            [Chunk::block_index(ax, ly + trunk_h, az)]
                                            == BlockId::Water
                                    {
                                        ctx.enqueue_edit(
                                            wx + dx,
                                            wy + trunk_h,
                                            wz + dz,
                                            tip,
                                        );
                                        writes += 1;
                                    }
                                }
                            }
                        }
                        break; // column resolved at first non-water
                    }
                }
            }
            if falls > 0 || writes > 0 {
                log::info!(
                    "[reef] chunk ({},{},{}): {} coral block(s), {} waterfall POI(s)",
                    coord.x, coord.y, coord.z, writes, falls
                );
            }
        }
    })
}
