//! `sim::item_modifier` — roll the affixes (modifiers) on a dropped item.
//!
//! [`roll_mods`] picks a set of modifier ids from a weighted `pool`, where the
//! number of affixes scales with `rarity` (`0..1`): a common item rolls few or
//! none, a legendary rolls the maximum. Each affix slot is a weighted draw from
//! the pool keyed by `(seed, slot)`, so the result is fixed for a given seed
//! (the same drop is identical on every peer) but different seeds roll different
//! affixes. Zero-weight entries are never chosen.
//!
//! Pure: a hash per slot and a cumulative-weight walk — no clock, no RNG state,
//! no cross-imports. The affix count is bounded by [`MAX_MODS`].

/// Maximum affixes an item can roll (at full rarity).
pub const MAX_MODS: usize = 5;
const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;

fn mix64(mut x: u64) -> u64 {
    x ^= x >> 30;
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94d0_49bb_1331_11eb);
    x ^= x >> 31;
    x
}

fn hash01(seed: u64, slot: u64) -> f32 {
    let h = mix64(seed ^ slot.wrapping_mul(GOLDEN));
    (h >> 40) as f32 / (1u64 << 24) as f32
}

fn clamp01(x: f32) -> f32 {
    let x = if x > 0.0 { x } else { 0.0 };
    if x < 1.0 {
        x
    } else {
        1.0
    }
}

/// Weighted draw of one modifier id from the pool for a given roll in `[0,1)`.
/// Skips non-positive weights. Returns `None` if no positive weight exists.
fn draw(pool: &[(u16, f32)], total: f32, r: f32) -> Option<u16> {
    let target = r * total;
    let mut acc = 0.0f32;
    let mut last: Option<u16> = None;
    let mut i = 0;
    while i < pool.len() {
        let (id, w) = pool[i];
        if w > 0.0 {
            last = Some(id);
            acc += w;
            if target < acc {
                return Some(id);
            }
        }
        i += 1;
    }
    last
}

/// Roll the affixes for an item of the given `rarity` from `pool`.
///
/// Count is `floor(clamp01(rarity) * MAX_MODS)`; each slot draws one weighted id.
/// Returns an empty vec for an empty / all-zero-weight pool.
pub fn roll_mods(rarity: f32, pool: &[(u16, f32)], seed: u64) -> Vec<u16> {
    // total positive weight
    let mut total = 0.0f32;
    let mut i = 0;
    while i < pool.len() {
        if pool[i].1 > 0.0 {
            total += pool[i].1;
        }
        i += 1;
    }
    if !(total > 0.0) {
        return Vec::new();
    }

    let count = (clamp01(rarity) * MAX_MODS as f32) as usize; // floor, 0..=MAX_MODS
    let mut out = Vec::with_capacity(count);
    let mut slot = 0u64;
    while (slot as usize) < count {
        if let Some(id) = draw(pool, total, hash01(seed, slot)) {
            out.push(id);
        }
        slot += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn higher_rarity_rolls_more() {
        let pool = [(1u16, 1.0f32), (2, 1.0), (3, 1.0)];
        let low = roll_mods(0.1, &pool, 0xABC);
        let high = roll_mods(0.95, &pool, 0xABC);
        assert!(high.len() > low.len(), "{} !> {}", high.len(), low.len());
    }

    #[test]
    fn empty_pool_is_empty() {
        assert!(roll_mods(1.0, &[], 0x1).is_empty());
        assert!(roll_mods(1.0, &[(1, 0.0), (2, 0.0)], 0x1).is_empty());
    }

    #[test]
    fn deterministic_per_seed() {
        let pool = [(1u16, 2.0f32), (2, 1.0), (3, 3.0)];
        let mut seed = 0u64;
        while seed < 1000 {
            assert_eq!(roll_mods(0.8, &pool, seed), roll_mods(0.8, &pool, seed));
            seed += 1;
        }
    }

    #[test]
    fn seed_sensitive() {
        let pool = [(1u16, 1.0f32), (2, 1.0), (3, 1.0), (4, 1.0)];
        let base = roll_mods(1.0, &pool, 0);
        let mut differ = false;
        let mut seed = 1u64;
        while seed < 500 {
            if roll_mods(1.0, &pool, seed) != base {
                differ = true;
                break;
            }
            seed += 1;
        }
        assert!(differ, "affixes never changed with seed");
    }

    #[test]
    fn count_is_bounded() {
        let pool = [(1u16, 1.0f32), (2, 1.0)];
        let mut seed = 0xF00D_u32;
        for _ in 0..2000 {
            let rarity = (lcg(&mut seed) % 200_001) as f32 / 100_000.0 - 0.5; // incl <0,>1
            let r = roll_mods(rarity, &pool, lcg(&mut seed) as u64);
            assert!(r.len() <= MAX_MODS);
        }
    }

    #[test]
    fn zero_weight_never_rolled() {
        // id 99 has weight 0 → must never appear
        let pool = [(1u16, 1.0f32), (99, 0.0), (2, 1.0)];
        let mut seed = 0u64;
        while seed < 2000 {
            for id in roll_mods(1.0, &pool, seed) {
                assert_ne!(id, 99, "rolled a zero-weight affix");
            }
            seed += 1;
        }
    }
}
