//! `sim::region_digest` — a stable, deterministic content digest of a region's
//! COARSE state. One digest serves three deep-time jobs at once:
//!   * **seed conditioning** (`simulation_guided_seeding.md`): the statistical
//!     jump's PRNG is keyed on `(world_seed, region, tick, digest)`, so a drought
//!     region and a wet one with the same (region, tick) draw different outcomes;
//!   * **branch identity** (`timebending_set_algebra.md`): two regions with the
//!     same digest are content-equal → share by pointer (intersection is cheap);
//!   * **convergence merge** (`vector_chaining_and_timewarp.md` /
//!     `deeptime_convergence_classes.md` §4.5): consecutive checkpoints with the
//!     same digest collapse — "nothing distinct happened" — so a deep chain's
//!     cost is proportional to DISTINCT digests, not raw span.
//!
//! Two requirements the docs insist on, both met here:
//!   1. **Coarse**: quantize before hashing (buckets, not raw values) so trivial
//!      jitter doesn't make every checkpoint look distinct — convergent channels
//!      that settle to the same bucket produce the same digest and merge.
//!   2. **Stable + versioned**: a fixed field order and an explicit splitmix64
//!      (the project's standard `mix64`, NOT `DefaultHasher` whose output isn't
//!      stable across builds/platforms), plus a `DIGEST_VERSION` so a change to
//!      the scheme is detectable rather than silently reshuffling identities.
//!
//! Inputs are [`Fixed`] (the deterministic 48.16 integer type) so the digest is
//! bit-identical on every machine. Pure & deterministic: no clock/RNG/IO/panic.

use crate::sim::fixed::Fixed;

/// Bump when the digest scheme changes (field set, quantization, or mixing) so a
/// stale digest is detectable instead of silently colliding/reshuffling.
pub const DIGEST_VERSION: u64 = 1;

/// The project's standard stable hash mixer (splitmix64 finaliser). Identical to
/// the `mix64` used across the sim crate — reproduced here so the digest has no
/// external dependency and is guaranteed stable.
#[inline]
fn mix64(mut x: u64) -> u64 {
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^ (x >> 31)
}

/// Fold one value into a running hash (order-sensitive — a fixed field order is
/// part of the contract).
#[inline]
fn fold(acc: u64, v: u64) -> u64 {
    mix64(acc ^ mix64(v.wrapping_add(0x9E37_79B9_7F4A_7C15)))
}

/// Quantize a `Fixed` to a coarse bucket index given `buckets_per_unit` (e.g. 16
/// → sixteenth-of-a-unit buckets). Coarser buckets ⇒ more checkpoints collapse to
/// the same digest ⇒ more merging. Negative values quantize symmetrically.
#[inline]
pub fn quantize(v: Fixed, buckets_per_unit: i64) -> i64 {
    // v.0 is value * 2^16. bucket = round(value * buckets_per_unit)
    //                              = round(v.0 * buckets_per_unit / 2^16)
    let b = buckets_per_unit.max(1);
    let num = (v.0 as i128) * (b as i128);
    let half = 1i128 << 15; // 2^15 for rounding 2^16 denominator
    let rounded = if num >= 0 {
        (num + half) >> 16
    } else {
        -(((-num) + half) >> 16)
    };
    rounded as i64
}

/// Build a region's coarse content digest from a field of quantized values.
///
/// `region` and `tick` anchor the digest to where/when it is (so identical state
/// in different regions/ticks is still distinguishable for seed conditioning),
/// and `quantized_fields` are the region's coarse state values already bucketed
/// via [`quantize`] in a FIXED order the caller keeps stable. The version is
/// folded in first so a scheme change shifts every digest detectably.
pub fn region_digest(region: u32, tick: u64, quantized_fields: &[i64]) -> u64 {
    let mut acc = DIGEST_VERSION;
    acc = fold(acc, region as u64);
    acc = fold(acc, tick);
    for &q in quantized_fields {
        acc = fold(acc, q as u64);
    }
    mix64(acc)
}

/// Convenience: quantize a slice of `Fixed` fields at one bucket resolution and
/// digest them in one call (the common case — a region's coarse field vector).
pub fn digest_fixed(region: u32, tick: u64, fields: &[Fixed], buckets_per_unit: i64) -> u64 {
    let q: Vec<i64> = fields.iter().map(|&f| quantize(f, buckets_per_unit)).collect();
    region_digest(region, tick, &q)
}

/// Whether two checkpoints are content-equal at the digest level — the merge /
/// branch-sharing test. (Trivial equality, named for intent.)
#[inline]
pub fn converged(a: u64, b: u64) -> bool {
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quantize_rounds_to_buckets() {
        // 0.5 at 16 buckets/unit = bucket 8; 0.53 also bucket ~8.5→8/9 boundary.
        assert_eq!(quantize(Fixed::from_frac(1, 2), 16), 8);
        assert_eq!(quantize(Fixed::from_int(1), 16), 16);
        assert_eq!(quantize(Fixed::ZERO, 16), 0);
    }

    #[test]
    fn quantize_symmetric_for_negatives() {
        assert_eq!(quantize(Fixed::from_frac(-1, 2), 16), -8);
        assert_eq!(quantize(Fixed::from_int(-1), 16), -16);
    }

    #[test]
    fn coarse_jitter_collapses_to_same_digest() {
        // two nearly-equal states that fall in the same coarse bucket → identical
        // digest → they MERGE (the whole point of coarse quantization).
        let a = [Fixed::from_f64(0.50), Fixed::from_f64(0.30)];
        let b = [Fixed::from_f64(0.51), Fixed::from_f64(0.305)]; // tiny jitter
        let da = digest_fixed(7, 100, &a, 8); // 8 buckets/unit = coarse
        let db = digest_fixed(7, 100, &b, 8);
        assert_eq!(da, db, "sub-bucket jitter must not change the digest");
        assert!(converged(da, db));
    }

    #[test]
    fn distinct_states_give_distinct_digests() {
        let a = [Fixed::from_f64(0.2), Fixed::from_f64(0.2)];
        let b = [Fixed::from_f64(0.9), Fixed::from_f64(0.1)];
        assert_ne!(digest_fixed(7, 100, &a, 16), digest_fixed(7, 100, &b, 16));
    }

    #[test]
    fn region_and_tick_distinguish_identical_state() {
        // identical coarse state, different region or tick → different digest, so
        // seed conditioning doesn't collide across space/time.
        let s = [Fixed::from_f64(0.4)];
        let base = digest_fixed(1, 10, &s, 16);
        assert_ne!(base, digest_fixed(2, 10, &s, 16)); // different region
        assert_ne!(base, digest_fixed(1, 11, &s, 16)); // different tick
    }

    #[test]
    fn field_order_matters() {
        // a fixed field order is part of the contract: swapping fields changes
        // the digest (so callers must keep the order stable).
        let ab = [Fixed::from_f64(0.2), Fixed::from_f64(0.8)];
        let ba = [Fixed::from_f64(0.8), Fixed::from_f64(0.2)];
        assert_ne!(digest_fixed(1, 1, &ab, 16), digest_fixed(1, 1, &ba, 16));
    }

    #[test]
    fn deterministic_across_calls() {
        let s = [Fixed::from_f64(0.33), Fixed::from_f64(0.67)];
        assert_eq!(digest_fixed(5, 42, &s, 16), digest_fixed(5, 42, &s, 16));
    }

    #[test]
    fn empty_fields_is_stable() {
        // a region with no coarse fields still digests deterministically.
        assert_eq!(region_digest(3, 9, &[]), region_digest(3, 9, &[]));
    }
}
