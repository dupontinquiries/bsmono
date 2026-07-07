//! `cave_moss` — moss/flora coverage on a cave surface.
//!
//! Worker-03 / WORLD pure core, round 20. Moss favors damp, shaded rock: more
//! where moisture is high and light is low, none on dry rock, less in bright
//! spots (near a cave mouth or a light shaft). A per-seed jitter gives patchy
//! coverage. Pairs with `cave_decoration` (R18) / `moisture_spread` (R17).
//!
//! Pure & deterministic; no clock/rng/unwrap. Self-contained for `rustc --test`.
#![allow(dead_code)]

/// Moss coverage in [0,1] from `moisture` and `light` (both ≈[0,1]) at a cell,
/// jittered by `seed`. Rises with moisture and shade; zero when dry.
pub fn moss(moisture: f32, light: f32, seed: u64) -> f32 {
    let m = clamp01(moisture);
    let l = clamp01(light);
    let base = m * (1.0 - l); // wet + shaded ⇒ high
    let j = 0.7 + unit(hash(seed)) * 0.3; // [0.7,1) patchiness
    clamp01(base * j)
}

// r193 (guide 4.1): shared splitmix.
fn hash(x: u64) -> u64 { crate::util::hash_util::mix64(x) }
fn unit(h: u64) -> f32 {
    ((h >> 40) as f32) / ((1u64 << 24) as f32)
}
fn clamp01(v: f32) -> f32 {
    crate::util::mathx::clamp01(v) // r343 dedup
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wet_and_dim_grows_more() {
        assert!(moss(0.9, 0.1, 5) > moss(0.3, 0.1, 5));
    }

    #[test]
    fn dry_is_bare() {
        for s in 0..20 {
            assert_eq!(moss(0.0, 0.2, s), 0.0);
        }
    }

    #[test]
    fn brighter_grows_less() {
        assert!(moss(0.9, 0.1, 5) > moss(0.9, 0.9, 5));
    }

    #[test]
    fn bounded() {
        for mi in 0..=10 {
            for li in 0..=10 {
                for s in 0..5 {
                    let v = moss(mi as f32 / 10.0, li as f32 / 10.0, s);
                    assert!(v >= 0.0 && v <= 1.0);
                }
            }
        }
    }

    #[test]
    fn deterministic_per_seed() {
        assert_eq!(moss(0.6, 0.3, 42), moss(0.6, 0.3, 42));
    }

    #[test]
    fn seed_sensitive() {
        // With nonzero base, the jitter differs between seeds.
        assert_ne!(moss(0.8, 0.2, 1), moss(0.8, 0.2, 2));
    }
}
