//! `sim::crit_roll` — does an attack land a critical hit, and by how much?
//!
//! [`crit`] takes a `roll` in `[0,1)`, a crit `chance`, and a crit `mult`, and
//! returns the damage scale to apply: the (clamped) multiplier on a crit, or `1`
//! on a normal hit. A `chance` of 0 never crits, a `chance` of 1 always does, and
//! the multiplier is floored at 1 so a crit can never reduce damage.
//!
//! Pure and deterministic: one comparison — no clock, no randomness (the caller
//! supplies the roll), no cross-imports.

/// Damage scale for an attack.
///
/// Returns `max(1, mult)` when `roll < chance` (a crit), else `1.0`. `chance` is
/// treated as clamped: `<= 0` never crits, `>= 1` always crits.
pub fn crit(roll: f32, chance: f32, mult: f32) -> f32 {
    let m = if mult > 1.0 { mult } else { 1.0 };
    if chance >= 1.0 {
        m
    } else if chance <= 0.0 {
        1.0
    } else if roll < chance {
        m
    } else {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn roll_below_chance_crits() {
        assert_eq!(crit(0.1, 0.5, 2.0), 2.0);
        assert_eq!(crit(0.49, 0.5, 3.0), 3.0);
    }

    #[test]
    fn roll_at_or_above_chance_is_normal() {
        assert_eq!(crit(0.5, 0.5, 2.0), 1.0);
        assert_eq!(crit(0.9, 0.5, 2.0), 1.0);
    }

    #[test]
    fn chance_zero_never_crits() {
        let mut seed = 0x5111_u32;
        for _ in 0..2000 {
            let roll = (lcg(&mut seed) % 100_000) as f32 / 100_000.0;
            assert_eq!(crit(roll, 0.0, 2.0), 1.0);
        }
    }

    #[test]
    fn chance_one_always_crits() {
        let mut seed = 0x7777_u32;
        for _ in 0..2000 {
            let roll = (lcg(&mut seed) % 100_000) as f32 / 100_000.0;
            assert_eq!(crit(roll, 1.0, 2.5), 2.5);
        }
    }

    #[test]
    fn bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..5000 {
            let roll = (lcg(&mut seed) % 100_000) as f32 / 100_000.0;
            let chance = (lcg(&mut seed) % 200_001) as f32 / 100_000.0 - 0.5; // incl <0,>1
            let mult = (lcg(&mut seed) % 100_001) as f32 / 1000.0 - 50.0; // incl < 1
            let m = if mult > 1.0 { mult } else { 1.0 };
            let v = crit(roll, chance, mult);
            assert!(v >= 1.0 && v <= m, "{} not in [1,{}]", v, m);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let roll = (lcg(&mut seed) % 100_000) as f32 / 100_000.0;
            let chance = (lcg(&mut seed) % 100_001) as f32 / 100_000.0;
            let mult = (lcg(&mut seed) % 10_001) as f32 / 1000.0;
            assert_eq!(crit(roll, chance, mult), crit(roll, chance, mult));
        }
    }
}
