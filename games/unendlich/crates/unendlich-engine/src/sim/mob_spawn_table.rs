//! `sim::mob_spawn_table` — pick which species spawns, weighted, in context.
//!
//! [`pick`] takes the spawn context (`biome`, `light`), a list of
//! `(species_id, weight)` candidates, and a `roll` in `[0,1)`, and returns the
//! chosen species (or `None` if nothing can spawn). Selection is proportional to
//! weight: a species with twice the weight is twice as likely across a roll
//! sweep. Zero-weight entries never spawn.
//!
//! Context steers *which* species a given roll yields without disturbing those
//! proportions: the roll is rotated by a hash of `(biome, light)` before
//! indexing the cumulative weights. Rotating a uniform roll keeps it uniform, so
//! a full `[0,1)` sweep still lands on every positive-weight species in the
//! correct ratio — but the same roll spawns different things in different biomes
//! / light levels. Pure and deterministic; weights are normalized by their sum.

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

/// Context rotation offset in `[0,1)`, from biome and a quantized light level.
fn context_offset(biome: u8, light: f32) -> f32 {
    let light_bucket = (clamp01(light) * 8.0) as u64; // 0..=8
    let key = (biome as u64) ^ light_bucket.wrapping_mul(GOLDEN);
    let h = mix64(key);
    (h >> 40) as f32 / (1u64 << 24) as f32
}

/// Pick a species id by weight, in context. `None` if there are no candidates
/// or every weight is non-positive.
pub fn pick(biome: u8, light: f32, weights: &[(u16, f32)], roll: f32) -> Option<u16> {
    // total positive weight
    let mut total = 0.0f32;
    let mut i = 0;
    while i < weights.len() {
        let w = weights[i].1;
        if w > 0.0 {
            total += w;
        }
        i += 1;
    }
    if !(total > 0.0) {
        return None;
    }

    // rotate the (clamped) roll by the context offset, staying in [0,1)
    let base = clamp01(roll);
    let rotated = base + context_offset(biome, light);
    let er = rotated - rotated.floor(); // fract → [0,1)
    let target = er * total;

    // walk the cumulative positive weights
    let mut acc = 0.0f32;
    let mut last_positive: Option<u16> = None;
    let mut j = 0;
    while j < weights.len() {
        let (id, w) = weights[j];
        if w > 0.0 {
            last_positive = Some(id);
            acc += w;
            if target < acc {
                return Some(id);
            }
        }
        j += 1;
    }
    // float edge (target == total): fall back to the last positive entry
    last_positive
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_is_none() {
        assert_eq!(pick(0, 0.5, &[], 0.3), None);
        // all-zero weights also None
        assert_eq!(pick(0, 0.5, &[(1, 0.0), (2, 0.0)], 0.3), None);
    }

    #[test]
    fn zero_weight_never_picked() {
        let table = [(10u16, 0.0f32), (20, 1.0), (30, 0.0)];
        let mut i = 0;
        while i < 1000 {
            let roll = i as f32 / 1000.0;
            let got = pick(3, 0.4, &table, roll);
            assert!(got != Some(10) && got != Some(30), "picked a zero-weight species");
            assert_eq!(got, Some(20));
            i += 1;
        }
    }

    #[test]
    fn higher_weight_more_likely() {
        let table = [(1u16, 1.0f32), (2, 3.0)];
        let (mut c1, mut c2) = (0u32, 0u32);
        let mut i = 0;
        while i < 4000 {
            let roll = i as f32 / 4000.0;
            match pick(5, 0.5, &table, roll) {
                Some(1) => c1 += 1,
                Some(2) => c2 += 1,
                _ => {}
            }
            i += 1;
        }
        // species 2 (weight 3) should win roughly 3:1 over species 1
        assert!(c2 > c1, "weights not respected: {} vs {}", c1, c2);
        let ratio = c2 as f32 / c1 as f32;
        assert!(ratio > 2.0 && ratio < 4.0, "ratio off: {}", ratio);
    }

    #[test]
    fn roll_sweep_covers_all_positive() {
        let table = [(1u16, 1.0f32), (2, 1.0), (3, 2.0)];
        let mut seen = [false; 4];
        let mut i = 0;
        while i < 2000 {
            let roll = i as f32 / 2000.0;
            if let Some(id) = pick(2, 0.6, &table, roll) {
                seen[id as usize] = true;
            }
            i += 1;
        }
        assert!(seen[1] && seen[2] && seen[3], "sweep missed a species");
    }

    #[test]
    fn proportions_independent_of_total_scale() {
        // doubling all weights shouldn't change proportions
        fn frac_of_2(table: &[(u16, f32)]) -> f32 {
            let mut c2 = 0u32;
            let mut tot = 0u32;
            let mut i = 0;
            while i < 4000 {
                let roll = i as f32 / 4000.0;
                if let Some(id) = pick(0, 0.5, table, roll) {
                    if id == 2 {
                        c2 += 1;
                    }
                    tot += 1;
                }
                i += 1;
            }
            c2 as f32 / tot as f32
        }
        let a = frac_of_2(&[(1, 1.0), (2, 3.0)]);
        let b = frac_of_2(&[(1, 100.0), (2, 300.0)]);
        assert!((a - b).abs() < 0.02, "proportions changed with scale: {} vs {}", a, b);
    }

    #[test]
    fn deterministic() {
        let table = [(1u16, 1.0f32), (2, 2.0), (3, 0.5)];
        let mut i = 0;
        while i < 2000 {
            let roll = i as f32 / 2000.0;
            assert_eq!(pick(7, 0.3, &table, roll), pick(7, 0.3, &table, roll));
            i += 1;
        }
    }
}
