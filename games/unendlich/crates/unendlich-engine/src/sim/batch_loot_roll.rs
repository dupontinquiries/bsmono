//! `sim::batch_loot_roll` — batched loot rolling, mirroring `sim::loot_roll`
//! (round 16), with a scalar reference for parity.
//!
//! [`loot_one`] rolls one drop table against one seed exactly as the r16 core
//! does; [`loot_batch`] rolls the same table against many seeds at once — the
//! shape you get killing a pack of the same mob, each kill its own seed. Both
//! route through the same `#[inline]` `roll_table` kernel, so each batch result
//! is **bit-for-bit identical** to the scalar roll for that seed.
//!
//! Perf shape: one table is borrowed once and rolled against a contiguous seed
//! slice, each producing an independent drop list — a single pass over the kills
//! instead of a separate call (and table re-read) per mob. Pure and
//! deterministic: a per-row hash, no clock, no RNG state, no cross-imports.

const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

fn mix64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    x
}

fn clamp01(x: f32) -> f32 {
    let x = if x > 0.0 { x } else { 0.0 };
    if x < 1.0 {
        x
    } else {
        1.0
    }
}

fn row_roll(seed: u64, index: usize) -> f32 {
    let h = mix64(seed ^ (index as u64).wrapping_mul(GOLDEN));
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// The shared kernel (identical to the r16 `loot_roll::roll`). Both paths call
/// exactly this, so scalar and batch agree bit-for-bit.
#[inline]
fn roll_table(table: &[(u16, u32, f32)], seed: u64) -> Vec<(u16, u32)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < table.len() {
        let (id, count, chance) = table[i];
        if row_roll(seed, i) < clamp01(chance) {
            out.push((id, count));
        }
        i += 1;
    }
    out
}

/// Scalar reference: roll the table once for `seed`.
pub fn loot_one(table: &[(u16, u32, f32)], seed: u64) -> Vec<(u16, u32)> {
    roll_table(table, seed)
}

/// Batched: roll the table once per seed, returning one drop list per seed.
pub fn loot_batch(table: &[(u16, u32, f32)], seeds: &[u64]) -> Vec<Vec<(u16, u32)>> {
    crate::util::batch_kernel::map_collect(seeds, |&seed| roll_table(table, seed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn rand_table(seed: &mut u32) -> Vec<(u16, u32, f32)> {
        let n = (lcg(seed) % 8) as usize;
        let mut t = Vec::new();
        for _ in 0..n {
            let id = (lcg(seed) & 0xFFFF) as u16;
            let count = lcg(seed) % 64 + 1;
            let chance = (lcg(seed) % 100_001) as f32 / 100_000.0;
            t.push((id, count, chance));
        }
        t
    }

    #[test]
    fn batch_matches_scalar_bit_for_bit() {
        let mut seed = 0xB17C_u32;
        for _ in 0..1500 {
            let table = rand_table(&mut seed);
            let m = (lcg(&mut seed) % 16) as usize;
            let mut seeds = Vec::new();
            for _ in 0..m {
                seeds.push(((lcg(&mut seed) as u64) << 20) ^ lcg(&mut seed) as u64);
            }
            let batch = loot_batch(&table, &seeds);
            assert_eq!(batch.len(), seeds.len());
            for i in 0..seeds.len() {
                assert_eq!(batch[i], loot_one(&table, seeds[i]));
            }
        }
    }

    #[test]
    fn empty_yields_empty() {
        // no seeds → no drop lists
        assert!(loot_batch(&[(1, 1, 1.0)], &[]).is_empty());
        // empty table → each seed yields an empty drop list
        let b = loot_batch(&[], &[1, 2, 3]);
        assert_eq!(b.len(), 3);
        for d in b {
            assert!(d.is_empty());
        }
    }

    #[test]
    fn single_equals_batch_of_one() {
        let mut seed = 0x0001_u32;
        for _ in 0..1500 {
            let table = rand_table(&mut seed);
            let s = ((lcg(&mut seed) as u64) << 20) ^ lcg(&mut seed) as u64;
            let b = loot_batch(&table, &[s]);
            assert_eq!(b.len(), 1);
            assert_eq!(b[0], loot_one(&table, s));
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..1500 {
            let table = rand_table(&mut seed);
            let m = (lcg(&mut seed) % 10) as usize;
            let mut seeds = Vec::new();
            for _ in 0..m {
                seeds.push(lcg(&mut seed) as u64);
            }
            assert_eq!(loot_batch(&table, &seeds), loot_batch(&table, &seeds));
        }
    }

    #[test]
    fn seed_sensitive() {
        // a mid-chance table should give different loot across the seed batch
        let table = [(1u16, 1u32, 0.5f32), (2, 1, 0.5), (3, 1, 0.5)];
        let seeds: Vec<u64> = (0u64..64).collect();
        let b = loot_batch(&table, &seeds);
        let first = &b[0];
        let mut differ = false;
        for d in &b {
            if d != first {
                differ = true;
                break;
            }
        }
        assert!(differ, "loot identical across all seeds");
    }

    #[test]
    fn chance_one_and_zero_behave() {
        let table = [(7u16, 3u32, 1.0f32), (8, 2, 0.0)];
        let b = loot_batch(&table, &[1, 2, 3]);
        for d in b {
            assert_eq!(d, vec![(7, 3)]); // always 7, never 8
        }
    }
}
