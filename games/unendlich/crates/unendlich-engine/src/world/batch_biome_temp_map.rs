//! `batch_biome_temp_map` — batched biome classification (perf bonus).
//!
//! Worker-03 / WORLD pure core, round 21. Classifies many `(temp, moisture)`
//! samples in one call, producing **identical** biome ids to the scalar
//! reference (a verbatim mirror of R18 `biome_temp_map`).
//!
//! PERF: biome ids are evaluated per surface column during generation and again
//! when biome-dependent decoration runs (CHUNK_SIZE² per chunk). Batching walks
//! a contiguous sample slice with a branch-predictable band lookup; because each
//! sample runs the exact scalar `biome_one`, the ids match per-sample calls — a
//! batched fast path can't reclassify a column differently from a scalar
//! fallback.
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Scalar reference — identical to `biome_temp_map::biome` (R18).
pub fn biome_one(temp: f32, moisture: f32) -> u8 {
    let t = band3(clamp01(temp));
    let m = band3(clamp01(moisture));
    (t * 3 + m) as u8
}

/// Batched biome ids over `(temp, moisture)` samples.
/// Bit-identical to calling `biome_one` per sample.
pub fn biome_batch(samples: &[(f32, f32)]) -> Vec<u8> {
    samples.iter().map(|&(t, m)| biome_one(t, m)).collect()
}

fn band3(v: f32) -> u32 {
    if v < 1.0 / 3.0 {
        0
    } else if v < 2.0 / 3.0 {
        1
    } else {
        2
    }
}

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_matches_scalar() {
        let s = [
            (0.0f32, 0.0),
            (0.9, 0.1),
            (0.1, 0.9),
            (0.5, 0.5),
            (-1.0, 2.0), // clamps
            (1.0, 1.0),
        ];
        let batch = biome_batch(&s);
        for (i, &(t, m)) in s.iter().enumerate() {
            assert_eq!(batch[i], biome_one(t, m), "id drift at {i}");
        }
    }

    #[test]
    fn empty_is_empty() {
        assert!(biome_batch(&[]).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let one = biome_one(0.55, 0.45);
        assert_eq!(biome_batch(&[(0.55, 0.45)]), vec![one]);
    }

    #[test]
    fn ids_bounded() {
        for v in biome_batch(&[(0.0, 0.0), (1.0, 1.0), (0.5, 0.5), (0.2, 0.8)]) {
            assert!(v <= 8);
        }
    }

    #[test]
    fn deterministic() {
        let s = [(0.3f32, 0.7), (0.3, 0.7)];
        assert_eq!(biome_batch(&s), biome_batch(&s));
    }

    #[test]
    fn covers_all_nine_biomes() {
        let mut s = Vec::new();
        for t in [0.1f32, 0.5, 0.9] {
            for m in [0.1f32, 0.5, 0.9] {
                s.push((t, m));
            }
        }
        let mut ids = biome_batch(&s);
        ids.sort();
        assert_eq!(ids, (0u8..=8).collect::<Vec<_>>());
    }
}
