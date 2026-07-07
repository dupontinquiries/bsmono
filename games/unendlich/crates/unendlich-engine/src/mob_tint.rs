//! mob_tint.rs — pure per-individual colour variation.
//!
//! Deterministic per-individual RGB multiplier near `[1,1,1]`, derived from a
//! seed: a value (brightness) shift scales all channels and a hue shift skews
//! red↑/blue↓ (or the reverse), each bounded by its range. Seed `0` is the
//! canonical baseline `[1,1,1]`. Pure, deterministic, panic-free (R1/R11/R21).

/// splitmix64-style hash of `(seed, salt)` -> `[0,1)`.
fn hash01(seed: u64, salt: u64) -> f32 {
    let mut z = seed.wrapping_add(salt).wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^= z >> 31;
    (z >> 40) as f32 / ((1u64 << 24) as f32)
}

/// RGB multiplier for the individual identified by `seed`. `hue_range` and
/// `val_range` (clamped `>= 0`) bound the hue and value deviations. Seed `0`
/// returns the baseline `[1,1,1]`; both ranges `0` also returns `[1,1,1]`.
/// Channels are clamped `>= 0`.
pub fn tint(seed: u64, hue_range: f32, val_range: f32) -> [f32; 3] {
    if seed == 0 {
        return [1.0, 1.0, 1.0];
    }
    let hr = hue_range.max(0.0);
    let vr = val_range.max(0.0);
    let hue_shift = (hash01(seed, 0xA1) * 2.0 - 1.0) * hr; // [-hr, hr]
    let val_shift = (hash01(seed, 0xB2) * 2.0 - 1.0) * vr; // [-vr, vr]
    let base = 1.0 + val_shift;
    [
        (base + hue_shift).max(0.0),
        base.max(0.0),
        (base - hue_shift).max(0.0),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seed_zero_is_baseline() {
        assert_eq!(tint(0, 0.3, 0.3), [1.0, 1.0, 1.0]);
    }

    #[test]
    fn within_plus_minus_range() {
        let (hr, vr) = (0.15f32, 0.2f32);
        let bound = hr + vr;
        for seed in 1..2000u64 {
            let t = tint(seed, hr, vr);
            for ch in t {
                assert!((ch - 1.0).abs() <= bound + 1e-5, "channel {} outside ±{}", ch, bound);
            }
        }
    }

    #[test]
    fn channels_non_negative() {
        // even with huge ranges, channels never go negative
        for seed in 1..3000u64 {
            for ch in tint(seed, 5.0, 5.0) {
                assert!(ch >= 0.0, "negative channel {}", ch);
            }
        }
    }

    #[test]
    fn deterministic_per_seed() {
        assert_eq!(tint(12345, 0.2, 0.2), tint(12345, 0.2, 0.2));
    }

    #[test]
    fn seed_sensitive() {
        let mut differ = false;
        let a = tint(1, 0.2, 0.2);
        for seed in 2..64u64 {
            if tint(seed, 0.2, 0.2) != a {
                differ = true;
                break;
            }
        }
        assert!(differ, "tint not sensitive to seed");
    }

    #[test]
    fn zero_range_is_identity() {
        for seed in 0..100u64 {
            assert_eq!(tint(seed, 0.0, 0.0), [1.0, 1.0, 1.0], "range 0 should be identity");
        }
    }
}
