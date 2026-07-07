//! `world::flora_map` — the seam between worldgen and `flora_placement`. For each
//! surface cell it builds a [`FloraCtx`] from the LIVE generator's biome/climate/
//! surface fields and asks `flora_placement::place_at` what (if anything) grows
//! there. This is what chunk-populate calls to scatter surface flora; the result
//! is deterministic from `(cell, world_seed)`.

use crate::world::cpu_generator::WorldGenerator;
use crate::world::flora_placement::{place_at, FloraCtx, FloraSpecies};

const PROBE_MIN: i32 = -16;
const PROBE_MAX: i32 = 248;

/// One placed plant: world position (one block above the surface) + species.
#[derive(Clone, Copy, Debug)]
pub struct FloraInstance {
    pub pos: [i32; 3],
    pub species: FloraSpecies,
}

// r297: the former `surface()` helper is gone — all column surface lookups now go
// through WorldProber::surface_y (see ctx_at_cached), with PROBE_MIN..PROBE_MAX as
// the scan range, so the deep scans are memoized within a flora batch.


/// Build the flora context at a surface cell from the generator's real fields.
pub fn ctx_at(gen: &WorldGenerator, x: i32, z: i32) -> Option<FloraCtx> {
    // r297: original signature preserved (throwaway prober → still one scan/column).
    let mut prober = crate::sim::world_prober::WorldProber::new();
    ctx_at_cached(gen, &mut prober, x, z)
}

/// Prober-aware column context. Every `surface(...)` here is the deep column scan
/// (`density` + `is_cave` over PROBE_MIN..PROBE_MAX); this routes the centre probe,
/// the biome, and the four slope probes through `prober`, so within a region the
/// repeated/adjacent column scans collapse to memo hits.
pub fn ctx_at_cached(
    gen: &WorldGenerator,
    prober: &mut crate::sim::world_prober::WorldProber,
    x: i32,
    z: i32,
) -> Option<FloraCtx> {
    let sy = prober.surface_y(gen, x, z, PROBE_MIN, PROBE_MAX)?;
    let s = prober.biome_sample(gen, x, sy, z);
    let sea = gen.sea_level_at(x, z).floor() as i32;
    let mut h = |dx, dz| {
        prober
            .surface_y(gen, x + dx, z + dz, PROBE_MIN, PROBE_MAX)
            .unwrap_or(sy)
    };
    let slope = (((h(1, 0) - h(-1, 0)).abs().max((h(0, 1) - h(0, -1)).abs())) as f32 / 6.0)
        .clamp(0.0, 1.0);
    Some(FloraCtx {
        biome: s.biome,
        moisture: s.humidity as f32,
        temperature01: s.temperature as f32,
        slope,
        surface_y: sy,
        sea_level: sea,
        // Bare rock = a steep face with no soil to root in. (Erosion drives biome
        // selection, not bare-rock — using it here wrongly suppressed all mountain
        // flora.) Biome species weights handle the rest.
        is_rock: slope > 0.7,
    })
}

/// Surface-flora instances in a horizontal region — the chunk-populate call.
pub fn region_flora(gen: &WorldGenerator, x0: i32, z0: i32, w: i32, d: i32) -> Vec<FloraInstance> {
    // r297: original signature preserved for existing callers/tests — runs with a
    // throwaway prober (each column still scanned once, just no cross-call reuse).
    let mut prober = crate::sim::world_prober::WorldProber::new();
    region_flora_cached(gen, &mut prober, x0, z0, w, d)
}

/// Prober-aware flora scan. Routes every per-column surface/biome lookup through
/// `prober` so the deep column scans (`ctx_at` does FIVE per column — surface +
/// biome + four slope probes) are memoized: a column scanned once is free, and the
/// four slope probes of one column are the centre probes of adjacent columns, so
/// across a region the slope scans become almost entirely cache hits. This is the
/// flora half of the ecosystem-stream region-crossing freeze (r296 cached the
/// populate_region half; this caches the larger flora half). The caller clears the
/// prober per batch. NOTE: flora uses PROBE_MIN..PROBE_MAX (a different, narrower
/// range than the spawn/biome pass), so flora must use its OWN prober — a column's
/// cached surface is only coherent for one probe range.
pub fn region_flora_cached(
    gen: &WorldGenerator,
    prober: &mut crate::sim::world_prober::WorldProber,
    x0: i32,
    z0: i32,
    w: i32,
    d: i32,
) -> Vec<FloraInstance> {
    let seed = gen.seed() as u64;
    let mut out = Vec::new();
    for cz in z0..z0 + d {
        for cx in x0..x0 + w {
            if let Some(ctx) = ctx_at_cached(gen, prober, cx, cz) {
                if let Some(species) = place_at(cx, cz, &ctx, seed) {
                    out.push(FloraInstance { pos: [cx, ctx.surface_y + 1, cz], species });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn region_scatters_flora_above_the_surface_never_in_the_sea() {
        let g = WorldGenerator::new(7);
        let flora = region_flora(&g, -60, -60, 120, 120);
        assert!(!flora.is_empty(), "a 120x120 region should grow some flora");
        for f in &flora {
            let sea = g.sea_level_at(f.pos[0], f.pos[2]).floor() as i32;
            assert!(f.pos[1] > sea, "flora must sit above sea level");
        }
    }

    #[test]
    fn deterministic_per_seed() {
        let a = region_flora(&WorldGenerator::new(11), 0, 0, 40, 40);
        let b = region_flora(&WorldGenerator::new(11), 0, 0, 40, 40);
        assert_eq!(a.len(), b.len());
    }
}

