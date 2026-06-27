//! rng.rs — a small, fast, deterministic PRNG ([`SplitMix64`]).
//!
//! splitmix64 is the standard seeding/utility generator (the same constants
//! kitfinder's `splitmix_seeds`, audio-kit's mixers, and snapcore's sim already
//! use, just copied each time). It is NOT cryptographic — for keys/tokens use
//! `crypto-kit` / `net-kit::auth::generate_key`. This is for simulations, jitter,
//! shuffles, procedural generation, and hash mixing. std only.

/// A SplitMix64 generator. Cheap to create and copy; deterministic for a given
/// seed (same seed → same sequence on every platform).
#[derive(Clone, Copy, Debug)]
pub struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    /// Seed the generator. Any `u64` is a valid seed (including 0).
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    /// Next pseudo-random `u64`.
    pub fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Next `f64` in `[0, 1)` (53 bits of precision).
    pub fn next_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Uniform in `[0, n)` (returns 0 when `n == 0`). Uses modulo — fine for the
    /// non-cryptographic uses here; the small low-end bias is irrelevant for
    /// simulation/jitter.
    pub fn next_bounded(&mut self, n: u64) -> u64 {
        if n == 0 {
            0
        } else {
            self.next_u64() % n
        }
    }

    /// Fisher–Yates shuffle of `slice` in place.
    pub fn shuffle<T>(&mut self, slice: &mut [T]) {
        for i in (1..slice.len()).rev() {
            let j = self.next_bounded(i as u64 + 1) as usize;
            slice.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deterministic_for_a_given_seed() {
        let mut a = SplitMix64::new(42);
        let mut b = SplitMix64::new(42);
        for _ in 0..1000 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
    }

    #[test]
    fn distinct_seeds_diverge() {
        let mut a = SplitMix64::new(1);
        let mut b = SplitMix64::new(2);
        assert!(a.next_u64() != b.next_u64());
    }

    #[test]
    fn f64_is_in_unit_interval() {
        let mut r = SplitMix64::new(7);
        for _ in 0..10_000 {
            let x = r.next_f64();
            assert!((0.0..1.0).contains(&x), "out of range: {x}");
        }
    }

    #[test]
    fn bounded_stays_in_range() {
        let mut r = SplitMix64::new(99);
        for _ in 0..10_000 {
            assert!(r.next_bounded(6) < 6);
        }
        assert_eq!(r.next_bounded(0), 0);
        assert_eq!(r.next_bounded(1), 0);
    }

    #[test]
    fn shuffle_is_a_permutation() {
        let mut r = SplitMix64::new(123);
        let mut v: Vec<u32> = (0..50).collect();
        let orig = v.clone();
        r.shuffle(&mut v);
        let mut sorted = v.clone();
        sorted.sort_unstable();
        assert_eq!(sorted, orig, "shuffle preserves the multiset");
        assert_ne!(v, orig, "and (with overwhelming probability) reorders it");
    }
}
