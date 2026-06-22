//! `sim::scent_decay` — how much of a scent trail is left after it has aged
//! (follow-on to `scent_track`, round 20).
//!
//! [`remaining`] applies exponential decay: a freshly laid trail (`age` 0) is at
//! full `strength`, drops to half after one `half_life`, a quarter after two, and
//! approaches 0 as it ages. The result never increases with age and stays within
//! `[0, strength]`. A `scent_track` reader can call this per cell to fade the
//! field a GPU kernel will mirror.
//!
//! Pure and deterministic: one `exp2` — no clock, no randomness, no
//! cross-imports.

fn nonneg(x: f32) -> f32 {
    if x > 0.0 {
        x
    } else {
        0.0
    }
}

/// Remaining scent strength after aging `age` with the given `half_life`.
///
/// `strength · 2^(-age/half_life)`. A non-positive `half_life` decays instantly
/// (full only at `age <= 0`). Negative `age` is treated as fresh.
pub fn remaining(strength: f32, age: f32, half_life: f32) -> f32 {
    let a = nonneg(age);
    if !(half_life > 0.0) {
        return if a <= 0.0 { strength } else { 0.0 };
    }
    strength * (-(a / half_life)).exp2()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn age_zero_is_full() {
        assert_eq!(remaining(10.0, 0.0, 5.0), 10.0);
        // negative age also treated as fresh
        assert_eq!(remaining(10.0, -3.0, 5.0), 10.0);
    }

    #[test]
    fn half_life_halves() {
        assert!((remaining(10.0, 5.0, 5.0) - 5.0).abs() < 1e-4);
        // two half-lives → a quarter
        assert!((remaining(10.0, 10.0, 5.0) - 2.5).abs() < 1e-4);
    }

    #[test]
    fn approaches_zero() {
        assert!(remaining(10.0, 1000.0, 5.0) < 1e-3);
    }

    #[test]
    fn bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..5000 {
            let strength = (lcg(&mut seed) % 100_001) as f32 / 100.0; // >= 0
            let age = (lcg(&mut seed) % 200_001) as f32 / 100.0 - 1000.0; // incl negative
            let hl = (lcg(&mut seed) % 100_001) as f32 / 100.0 - 100.0; // incl <= 0
            let r = remaining(strength, age, hl);
            assert!(r >= 0.0 && r <= strength + 1e-3, "{} not in [0,{}]", r, strength);
        }
    }

    #[test]
    fn monotone_non_increasing_in_age() {
        let mut seed = 0x3303_u32;
        for _ in 0..3000 {
            let strength = (lcg(&mut seed) % 100_001) as f32 / 100.0;
            let hl = (lcg(&mut seed) % 100_000 + 1) as f32 / 100.0;
            let a1 = (lcg(&mut seed) % 100_001) as f32 / 100.0;
            let a2 = (lcg(&mut seed) % 100_001) as f32 / 100.0;
            let lo = a1.min(a2);
            let hi = a1.max(a2);
            assert!(remaining(strength, hi, hl) <= remaining(strength, lo, hl) + 1e-4);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let strength = (lcg(&mut seed) % 100_001) as f32 / 100.0;
            let age = (lcg(&mut seed) % 100_001) as f32 / 100.0;
            let hl = (lcg(&mut seed) % 100_000 + 1) as f32 / 100.0;
            assert_eq!(remaining(strength, age, hl), remaining(strength, age, hl));
        }
    }
}
