//! Deterministic gene mutation: a splitmix64 hash of `seed` gives a signed jitter
//! scaled by `rate`, added to the gene and clamped to [0,1]. Same inputs ⇒ same
//! output (no RNG state) so worldgen/breeding stays reproducible (R11).

pub fn mutate(gene: f32, rate: f32, seed: u64) -> f32 {
    let g = gene.clamp(0.0, 1.0);
    let r = rate.clamp(0.0, 1.0);
    let mut x = seed.wrapping_add(0x9E37_79B9_7F4A_7C15);
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    let u = (x >> 11) as f32 / (1u64 << 53) as f32; // [0,1)
    let jitter = (u - 0.5) * 2.0 * r;               // [-rate, rate]
    (g + jitter).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn rate0_unchanged() { assert_eq!(mutate(0.5, 0.0, 12345), 0.5); }
    #[test] fn deterministic() { assert_eq!(mutate(0.5, 0.1, 12345).to_bits(), mutate(0.5, 0.1, 12345).to_bits()); }
    #[test] fn seeds_produce_variety() {
        let mut set = std::collections::BTreeSet::new();
        for s in 0..12u64 { set.insert(mutate(0.5, 0.5, s).to_bits()); }
        assert!(set.len() > 1);
    }
    #[test] fn bounded() { for s in 0..50u64 { let v = mutate(0.5, 0.9, s); assert!((0.0..=1.0).contains(&v)); } }
    #[test] fn jitter_within_rate() { for s in 0..50u64 { assert!((mutate(0.5, 0.1, s) - 0.5).abs() <= 0.1 + 1e-6); } }
    #[test] fn deterministic2() { assert_eq!(mutate(0.3, 0.2, 7), mutate(0.3, 0.2, 7)); }
}
