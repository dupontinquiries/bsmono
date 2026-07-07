//! `sim::ability_combo` — the damage multiplier for chaining abilities together
//! within a combo window.
//!
//! [`combo_mult`] returns the multiplier for a combo of `chain` hits: it starts
//! at `base` and grows by `per` for each link in the chain, up to `cap`. A chain
//! of 0 (or a `per` of 0) is just the base multiplier; longer chains scale up but
//! never exceed the cap. The result never drops below `base`, so a combo only
//! ever rewards.
//!
//! Pure and deterministic: a multiply-add and a clamp — no clock, no randomness,
//! no cross-imports.

fn clampf(x: f32, lo: f32, hi: f32) -> f32 {
    let x = if x > lo { x } else { lo };
    if x < hi {
        x
    } else {
        hi
    }
}

/// Combo damage multiplier.
///
/// `clamp(base + chain·per, base, max(base, cap))` — starts at `base`, grows by
/// `per` per link, capped (the cap can't pull below `base`).
pub fn combo_mult(chain: u8, base: f32, per: f32, cap: f32) -> f32 {
    let hi = if cap > base { cap } else { base };
    clampf(base + (chain as f32) * per, base, hi)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn chain_zero_is_base() {
        assert_eq!(combo_mult(0, 1.0, 0.5, 10.0), 1.0);
        assert_eq!(combo_mult(0, 2.5, 0.5, 10.0), 2.5);
    }

    #[test]
    fn grows_per_hit() {
        let c1 = combo_mult(1, 1.0, 0.5, 10.0);
        let c4 = combo_mult(4, 1.0, 0.5, 10.0);
        assert_eq!(c1, 1.5);
        assert_eq!(c4, 3.0);
        assert!(c4 > c1);
    }

    #[test]
    fn capped() {
        assert_eq!(combo_mult(100, 1.0, 0.5, 10.0), 10.0);
    }

    #[test]
    fn per_zero_is_base() {
        assert_eq!(combo_mult(5, 1.0, 0.0, 10.0), 1.0);
        assert_eq!(combo_mult(255, 1.5, 0.0, 10.0), 1.5);
    }

    #[test]
    fn bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..5000 {
            let chain = (lcg(&mut seed) & 0xFF) as u8;
            let base = (lcg(&mut seed) % 10_001) as f32 / 1000.0;
            let per = (lcg(&mut seed) % 20_001) as f32 / 1000.0 - 10.0; // incl negative
            let cap = (lcg(&mut seed) % 30_001) as f32 / 1000.0;
            let m = combo_mult(chain, base, per, cap);
            let hi = if cap > base { cap } else { base };
            assert!(m >= base && m <= hi, "{} not in [{},{}]", m, base, hi);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let chain = (lcg(&mut seed) & 0xFF) as u8;
            let base = (lcg(&mut seed) % 10_001) as f32 / 1000.0;
            let per = (lcg(&mut seed) % 10_001) as f32 / 1000.0;
            let cap = (lcg(&mut seed) % 20_001) as f32 / 1000.0;
            assert_eq!(combo_mult(chain, base, per, cap), combo_mult(chain, base, per, cap));
        }
    }
}
