//! `world::worldgen_noise` — r340-sl-worldgen: consolidated worldgen primitives.
//!
//! The worldgen cluster re-implements the same deterministic-hash / unit-roll /
//! density-curve math in several places. This module is the parity-preserving
//! shared home for the pieces that are provably identical, plus the canonical
//! placement-roll for new code. Built on the existing `util::hash_util`
//! splitmix64 (the ONE bit-mixer) — it does NOT introduce a new hash family.
//!
//! RESPECT (worldgen determinism): every helper here is pure and seeded; the
//! ones marked "drop-in" reproduce an existing call site **bit-for-bit** (proven
//! in tests), so routing those sites here cannot move a single spawn. The
//! distinct mixer families (`fmix64`, the bespoke flora mixer, the 32-bit plant
//! hash) are reported, not silently merged — swapping a site onto a *different*
//! mixer would relocate placements, which is forbidden. Terrain/climate stay
//! continuous FP fields sampled elsewhere; this only consolidates the hash/roll/
//! density arithmetic those samplers feed.
//!
//! Cross-cluster note: `bits_to_unit` and `fmix64` are generic hash primitives
//! that ideally live in `util::hash_util` (owned by sl-math). They are placed
//! here provisionally; see `SHAREDLIB_REPORT-r340-sl-worldgen.md`.

use crate::util::hash_util;

#[inline]
fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

/// Hashed `u64` bits → `[0,1)` using the top 24 bits. This is the
/// `((h >> 40) as f32) / ((1u64 << 24) as f32)` idiom duplicated verbatim in
/// `flora_placement::hash01` and `vegetation_scatter::unit` — **drop-in**
/// (bit-identical). (Generic enough to belong in `util::hash_util`.)
#[inline]
pub fn bits_to_unit(h: u64) -> f32 {
    ((h >> 40) as f32) / ((1u64 << 24) as f32)
}

/// MurmurHash3 64-bit finalizer (`fmix64`) — the OTHER mixer family used by
/// `cpu_generator::tree_origin_hash`. Provided as a **named** primitive so that
/// site can route here *without changing its bytes* (bit-identical finalizer),
/// instead of being forced onto `hash_util::mix64` (which would alter terrain).
/// (Also a `util::hash_util` candidate.)
#[inline]
pub fn fmix64(mut h: u64) -> u64 {
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
    h ^= h >> 33;
    h
}

/// Canonical 2D position hash for a `seed` and integer cell `(x, z)`, built on
/// `hash_util::mix64_2` (splitmix64). The proposed target for NEW worldgen
/// rolls and for sites already bit-identical to `hash_util::mix64`
/// (`structure_grammar::hash`, `vegetation_scatter::mix`). NOTE: this is *not*
/// a byte-for-byte replacement for the bespoke coordinate-combiners in
/// `flora_placement::hash01` / `vegetation_scatter::hash2` — swapping those
/// changes placements, so the report only *proposes* migrating them.
#[inline]
pub fn hash_pos2(seed: u64, x: i32, z: i32) -> u64 {
    hash_util::mix64_2(hash_util::mix64_2(seed, x as u32 as u64), z as u32 as u64)
}

/// Canonical placement roll in `[0,1)` for cell `(x, z)` under `seed`.
#[inline]
pub fn roll01(seed: u64, x: i32, z: i32) -> f32 {
    bits_to_unit(hash_pos2(seed, x, z))
}

/// The shared lushness × moisture × slope density curve. Reproduces BOTH
/// existing density functions exactly with the right parameters:
///   * `vegetation_scatter::density` = `density_curve(base, m, s, 0.0, 1.0)`
///     → `base * m * (1 - s)`
///   * `flora_placement::flora_density` = `density_curve(base, m, s, 0.4, 0.8)`
///     → `base * (0.4 + 0.6 m) * (1 - 0.8 s)`
/// `moisture_floor` is the dryness baseline (moisture maps `[0,1] → [floor,1]`);
/// `slope_atten` is how strongly slope suppresses density. Result clamped `[0,1]`.
#[inline]
pub fn density_curve(base: f32, moisture01: f32, slope01: f32, moisture_floor: f32, slope_atten: f32) -> f32 {
    let m = clamp01(moisture01);
    let s = clamp01(slope01);
    let mlo = clamp01(moisture_floor);
    let satten = clamp01(slope_atten);
    clamp01(base * (mlo + (1.0 - mlo) * m) * (1.0 - satten * s))
}

/// Canonical pure per-cell placement decision: place iff the roll is under the
/// (clamped) density. `density <= 0` never places, `>= 1` always does — matching
/// `vegetation_scatter::place`. This is the data-parallel kernel (see
/// [`scatter_region`]); pure ⇒ `Send + Sync` ⇒ `task_lanes`-safe.
#[inline]
pub fn place_at(seed: u64, x: i32, z: i32, density: f32) -> bool {
    roll01(seed, x, z) < clamp01(density)
}

/// Scatter a placement decision over a region of cells. `density_at(&cell)`
/// gives each cell's density (pure). Routed through `task_lanes` so a large
/// region parallelizes; the per-cell kernel shares no mutable state, so the
/// result is **identical to the serial path** (proven in tests). PERF BONUS (a).
pub fn scatter_region<F>(seed: u64, cells: &[(i32, i32)], min_parallel: usize, density_at: F) -> Vec<bool>
where
    F: Fn(&(i32, i32)) -> f32 + Sync + Send,
{
    crate::util::task_lanes::map_pure_threshold(cells, min_parallel, |&(x, z)| {
        place_at(seed, x, z, density_at(&(x, z)))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Replicas of the existing call sites, copied VERBATIM, to prove parity.
    fn site_unit(h: u64) -> f32 {
        ((h >> 40) as f32) / ((1u64 << 24) as f32) // flora_placement:241 / vegetation:55
    }
    fn site_mix64(x: u64) -> u64 {
        // structure_grammar::hash / vegetation_scatter::mix — verbatim.
        let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn site_tree_finalizer(mut h: u64) -> u64 {
        // cpu_generator::tree_origin_hash finalizer — verbatim.
        h ^= h >> 33;
        h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
        h ^= h >> 33;
        h = h.wrapping_mul(0xc4ce_b9fe_1a85_ec53);
        h ^= h >> 33;
        h
    }
    fn site_veg_density(base: f32, m: f32, s: f32) -> f32 {
        clamp01(base * clamp01(m) * (1.0 - clamp01(s)))
    }
    fn site_flora_density(base: f32, m: f32, s: f32) -> f32 {
        clamp01(base * (0.4 + 0.6 * clamp01(m)) * (1.0 - 0.8 * clamp01(s)))
    }

    fn lcg(state: &mut u64) -> u64 {
        *state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *state
    }

    #[test]
    fn bits_to_unit_matches_sites_and_is_unit() {
        let mut s = 1;
        for _ in 0..5000 {
            let h = lcg(&mut s);
            assert_eq!(bits_to_unit(h).to_bits(), site_unit(h).to_bits());
            assert!((0.0..1.0).contains(&bits_to_unit(h)));
        }
    }

    #[test]
    fn hash_util_mix64_is_bit_identical_to_the_mix_sites() {
        // structure_grammar::hash and vegetation_scatter::mix ARE hash_util::mix64.
        let mut s = 0xDEAD;
        for _ in 0..5000 {
            let x = lcg(&mut s);
            assert_eq!(hash_util::mix64(x), site_mix64(x), "mix64 must match the mix sites for {x}");
        }
    }

    #[test]
    fn fmix64_matches_tree_origin_finalizer() {
        let mut s = 0xBEEF;
        for _ in 0..5000 {
            let h = lcg(&mut s);
            assert_eq!(fmix64(h), site_tree_finalizer(h), "fmix64 must match tree_origin finalizer");
        }
    }

    #[test]
    fn density_curve_reproduces_both_existing_curves() {
        let mut s = 7;
        for _ in 0..3000 {
            let base = bits_to_unit(lcg(&mut s));
            let m = bits_to_unit(lcg(&mut s)) * 1.3 - 0.15; // include out-of-range
            let sl = bits_to_unit(lcg(&mut s)) * 1.3 - 0.15;
            assert_eq!(density_curve(base, m, sl, 0.0, 1.0).to_bits(), site_veg_density(base, m, sl).to_bits());
            assert_eq!(density_curve(base, m, sl, 0.4, 0.8).to_bits(), site_flora_density(base, m, sl).to_bits());
        }
    }

    #[test]
    fn place_at_is_deterministic_and_density_monotone() {
        assert_eq!(place_at(42, 3, 7, 0.5), place_at(42, 3, 7, 0.5));
        assert!(!place_at(42, 3, 7, 0.0), "density 0 never places");
        assert!(place_at(42, 3, 7, 1.0), "density 1 always places (roll < 1)");
        // Over a region, higher density ⇒ at least as many placements.
        let cells: Vec<(i32, i32)> = (0..64).flat_map(|x| (0..64).map(move |z| (x, z))).collect();
        let count = |d: f32| cells.iter().filter(|&&(x, z)| place_at(9, x, z, d)).count();
        let (lo, hi) = (count(0.25), count(0.75));
        assert!(hi >= lo, "more density must not reduce placements ({lo} -> {hi})");
        assert!(hi > lo, "0.75 should place clearly more than 0.25 ({lo} -> {hi})");
    }

    #[test]
    fn roll01_unit_and_deterministic() {
        let mut s = 3;
        for _ in 0..2000 {
            let (x, z) = (lcg(&mut s) as i32, lcg(&mut s) as i32);
            let r = roll01(123, x, z);
            assert!((0.0..1.0).contains(&r));
            assert_eq!(r.to_bits(), roll01(123, x, z).to_bits());
        }
    }

    // PERF BONUS (a): the parallel scatter must equal the serial scatter exactly.
    #[test]
    fn parallel_scatter_equals_serial() {
        let cells: Vec<(i32, i32)> = (0..200).flat_map(|x| (0..200).map(move |z| (x, z))).collect();
        let density_at = |&(x, z): &(i32, i32)| {
            // a cheap pure density field — varies per cell, no shared state.
            density_curve(0.6, ((x ^ z) & 31) as f32 / 31.0, (x & 7) as f32 / 7.0, 0.4, 0.8)
        };
        let serial: Vec<bool> = cells.iter().map(|&(x, z)| place_at(77, x, z, density_at(&(x, z)))).collect();
        let parallel = scatter_region(77, &cells, 256, density_at);
        assert_eq!(serial, parallel, "task_lanes scatter must be bit-identical to serial");
    }
}
