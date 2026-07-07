//! `sim::loot_roll` — roll a drop table into the items that actually fall.
//!
//! [`roll`] takes a table of `(item_id, count, chance)` rows and a `seed`, and
//! returns the `(item_id, count)` drops that landed. Each row is rolled
//! independently against its `chance` (`0..1`): chance `1` always drops, chance
//! `0` never does. The same `seed` always yields the same loot (so a chest opens
//! identically on every peer and survives rollback), while different seeds give
//! different loot.
//!
//! Each row hashes `(seed, row_index)` for its roll, so rows decorrelate and a
//! repeated item at different table positions rolls independently. Pure: no
//! clock, no RNG state, no cross-imports.

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

/// Deterministic `[0,1)` roll for a given seed and row index.
fn row_roll(seed: u64, index: usize) -> f32 {
    let h = mix64(seed ^ (index as u64).wrapping_mul(GOLDEN));
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Roll a drop table. Returns `(item_id, count)` for each row that dropped, in
/// table order.
pub fn roll(table: &[(u16, u32, f32)], seed: u64) -> Vec<(u16, u32)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < table.len() {
        let (id, count, chance) = table[i];
        // row_roll ∈ [0,1); chance clamped to [0,1].
        // chance 1 → always (roll < 1); chance 0 → never (roll < 0 impossible).
        if row_roll(seed, i) < clamp01(chance) {
            out.push((id, count));
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chance_one_always_drops_with_counts() {
        let table = [(1u16, 5u32, 1.0f32), (2, 3, 1.0), (3, 99, 2.0)];
        let mut seed = 0u64;
        while seed < 500 {
            let got = roll(&table, seed);
            assert_eq!(got, vec![(1, 5), (2, 3), (3, 99)], "seed {}", seed);
            seed += 1;
        }
    }

    #[test]
    fn chance_zero_never_drops() {
        let table = [(1u16, 1u32, 0.0f32), (2, 1, -0.5)];
        let mut seed = 0u64;
        while seed < 500 {
            assert!(roll(&table, seed).is_empty(), "seed {}", seed);
            seed += 1;
        }
    }

    #[test]
    fn counts_are_respected() {
        let table = [(7u16, 42u32, 1.0f32)];
        let got = roll(&table, 0xABC);
        assert_eq!(got, vec![(7, 42)]);
    }

    #[test]
    fn empty_table_empty_drops() {
        assert!(roll(&[], 12345).is_empty());
    }

    #[test]
    fn deterministic_per_seed() {
        let table = [(1u16, 1u32, 0.5f32), (2, 2, 0.3), (3, 1, 0.8)];
        let mut seed = 0u64;
        while seed < 1000 {
            assert_eq!(roll(&table, seed), roll(&table, seed));
            seed += 1;
        }
    }

    #[test]
    fn seed_sensitive() {
        // a mid-chance table should produce different loot for some seed pair
        let table = [(1u16, 1u32, 0.5f32), (2, 1, 0.5), (3, 1, 0.5), (4, 1, 0.5)];
        let base = roll(&table, 0);
        let mut differ = false;
        let mut seed = 1u64;
        while seed < 500 {
            if roll(&table, seed) != base {
                differ = true;
                break;
            }
            seed += 1;
        }
        assert!(differ, "loot never changed with seed");
    }

    #[test]
    fn roughly_half_drop_at_half_chance() {
        // sanity: ~50% drop rate over many seeds for a single 0.5 row
        let table = [(1u16, 1u32, 0.5f32)];
        let mut drops = 0u32;
        let mut seed = 0u64;
        while seed < 4000 {
            if !roll(&table, seed).is_empty() {
                drops += 1;
            }
            seed += 1;
        }
        let frac = drops as f32 / 4000.0;
        assert!(frac > 0.4 && frac < 0.6, "drop frac {}", frac);
    }
}
