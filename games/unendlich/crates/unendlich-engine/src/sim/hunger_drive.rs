//! `sim::hunger_drive` — a creature's hunger creeps up over time and drops when
//! it eats.
//!
//! [`tick`] advances hunger (`0` sated .. `1` starving) by `HUNGER_RATE · dt`
//! and subtracts however much it `ate` this step, clamped to `[0, 1]`. With no
//! time passing and no food the value is unchanged; more elapsed time always
//! means at least as hungry. A simpler sibling of the round-7 `survival` core,
//! isolated to the single hunger axis.
//!
//! Pure and deterministic: one linear step and a clamp — no clock, no
//! randomness, no cross-imports.

/// Hunger gained per second with no food.
pub const HUNGER_RATE: f32 = 0.1;

fn clamp01(x: f32) -> f32 {
    let x = if x > 0.0 { x } else { 0.0 };
    if x < 1.0 {
        x
    } else {
        1.0
    }
}

fn nonneg(x: f32) -> f32 {
    if x > 0.0 {
        x
    } else {
        0.0
    }
}

/// Advance hunger one tick.
///
/// `new = clamp01(hunger + HUNGER_RATE·dt − ate)`. Negative `dt`/`ate` are
/// floored at 0, so time only ever adds hunger and food only ever removes it.
pub fn tick(hunger: f32, dt: f32, ate: f32) -> f32 {
    clamp01(clamp01(hunger) + HUNGER_RATE * nonneg(dt) - nonneg(ate))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn rises_with_time() {
        let h = tick(0.3, 2.0, 0.0);
        assert!(h > 0.3, "hunger should rise, got {}", h);
    }

    #[test]
    fn eating_lowers_hunger() {
        let h = tick(0.8, 0.0, 0.5);
        assert!(h < 0.8 && (h - 0.3).abs() < 1e-6);
    }

    #[test]
    fn clamped_to_unit_range() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..4000 {
            let hunger = (lcg(&mut seed) % 300_001) as f32 / 100_000.0 - 1.0; // incl out-of-range
            let dt = (lcg(&mut seed) % 200_001) as f32 / 1000.0 - 50.0;
            let ate = (lcg(&mut seed) % 200_001) as f32 / 100_000.0 - 0.5;
            let h = tick(hunger, dt, ate);
            assert!(h >= 0.0 && h <= 1.0, "hunger {}", h);
        }
    }

    #[test]
    fn dt_zero_and_ate_zero_unchanged() {
        let mut seed = 0x0001_u32;
        for _ in 0..1000 {
            let hunger = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            assert_eq!(tick(hunger, 0.0, 0.0), hunger);
        }
    }

    #[test]
    fn monotone_hungrier_with_more_time() {
        let mut seed = 0x3303_u32;
        for _ in 0..3000 {
            let hunger = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let d1 = (lcg(&mut seed) % 100_001) as f32 / 1000.0;
            let d2 = (lcg(&mut seed) % 100_001) as f32 / 1000.0;
            let lo = d1.min(d2);
            let hi = d1.max(d2);
            assert!(tick(hunger, hi, 0.0) >= tick(hunger, lo, 0.0) - 1e-6);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let hunger = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let dt = (lcg(&mut seed) % 100_001) as f32 / 1000.0;
            let ate = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            assert_eq!(tick(hunger, dt, ate), tick(hunger, dt, ate));
        }
    }
}
