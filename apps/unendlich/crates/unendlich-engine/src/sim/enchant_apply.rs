//! `sim::enchant_apply` — apply an enchant level's bonus to a base stat.
//!
//! [`apply`] adds `level · per_level` to a `base` stat and clamps the total to
//! `[0, cap]`. Level 0 (or a `per_level` of 0) leaves the base untouched; higher
//! levels scale the bonus up until it hits the cap. The cap is the stat ceiling,
//! so a heavily enchanted item can't push a stat arbitrarily high.
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

/// Apply an enchant of `level` (bonus `per_level` each) to `base`, capped.
///
/// `clamp(base + level·per_level, 0, max(0, cap))`.
pub fn apply(base: f32, level: u8, per_level: f32, cap: f32) -> f32 {
    let c = if cap > 0.0 { cap } else { 0.0 };
    let total = base + (level as f32) * per_level;
    clampf(total, 0.0, c)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn level_zero_is_base() {
        assert_eq!(apply(10.0, 0, 2.0, 100.0), 10.0);
        assert_eq!(apply(0.0, 0, 5.0, 100.0), 0.0);
    }

    #[test]
    fn scales_with_level() {
        let l2 = apply(10.0, 2, 2.0, 100.0);
        let l5 = apply(10.0, 5, 2.0, 100.0);
        assert_eq!(l2, 14.0);
        assert_eq!(l5, 20.0);
        assert!(l5 > l2);
    }

    #[test]
    fn capped() {
        // level 100 * 2 = 200 over base 10 → clamped to cap 50
        assert_eq!(apply(10.0, 100, 2.0, 50.0), 50.0);
    }

    #[test]
    fn per_level_zero_is_base() {
        assert_eq!(apply(10.0, 5, 0.0, 100.0), 10.0);
        assert_eq!(apply(33.0, 255, 0.0, 100.0), 33.0);
    }

    #[test]
    fn bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..5000 {
            let base = (lcg(&mut seed) % 20_001) as f32 / 100.0 - 100.0; // incl negatives
            let level = (lcg(&mut seed) & 0xFF) as u8;
            let per = (lcg(&mut seed) % 20_001) as f32 / 100.0 - 100.0;
            let cap = (lcg(&mut seed) % 20_001) as f32 / 100.0 - 50.0; // incl <= 0
            let v = apply(base, level, per, cap);
            let hi = if cap > 0.0 { cap } else { 0.0 };
            assert!(v >= 0.0 && v <= hi, "{} not in [0,{}]", v, hi);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let base = (lcg(&mut seed) % 10_001) as f32 / 100.0;
            let level = (lcg(&mut seed) & 0xFF) as u8;
            let per = (lcg(&mut seed) % 10_001) as f32 / 100.0;
            let cap = (lcg(&mut seed) % 10_001) as f32 / 100.0;
            assert_eq!(apply(base, level, per, cap), apply(base, level, per, cap));
        }
    }
}
