//! hash_util — splitmix64 family (guide §4.1). The ONE bit-mixer every
//! per-seed deterministic value derives from. std-only, pure, const.
/// Splitmix64: bijective u64 -> u64 bit-mixer. Deterministic,
/// avalanche-correct. Foundation for per-seed value generation.
#[inline]
pub const fn mix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Map a u64 through mix64 to [0, 1). Top 24 bits give uniform floats.
#[inline]
pub fn mix64_unit(x: u64) -> f32 {
    (mix64(x) >> 40) as f32 / (1u64 << 24) as f32
}

/// Combine two coordinates into one seed before mixing. Order-sensitive.
#[inline]
pub const fn mix64_2(a: u64, b: u64) -> u64 {
    mix64(a ^ b.rotate_left(32) ^ 0xA076_1D64_78BD_642F)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mix64_is_deterministic_and_spreads() {
        assert_eq!(mix64(1), mix64(1));
        assert_ne!(mix64(1), mix64(2));
        let u = mix64_unit(12345);
        assert!((0.0..1.0).contains(&u));
    }
    #[test]
    fn mix64_2_order_sensitive() {
        assert_ne!(mix64_2(1, 2), mix64_2(2, 1));
    }
}
