//! `sim::ability_cooldown` — how long until an ability is ready again.
//!
//! [`cd_remaining`] returns the time left on an ability's cooldown given when it
//! was `last_cast`, the current time `now`, and the `cooldown` length. Just after
//! a cast it returns the full cooldown; once at least `cooldown` has elapsed it
//! returns 0 (ready); in between it returns the partial remainder. A cooldown of
//! 0 is always ready, and the result is clamped to `[0, cooldown]` so clock skew
//! can't produce a negative or over-long timer.
//!
//! Pure and deterministic: a subtract and a clamp — no clock (the caller passes
//! `now`), no randomness, no cross-imports.

/// Cooldown time remaining (0 means ready).
///
/// `clamp(cooldown - (now - last_cast), 0, max(0, cooldown))`.
pub fn cd_remaining(last_cast: f32, now: f32, cooldown: f32) -> f32 {
    let cap = if cooldown > 0.0 { cooldown } else { 0.0 };
    let elapsed = now - last_cast;
    let remaining = cap - elapsed;
    if remaining < 0.0 {
        0.0
    } else if remaining > cap {
        cap
    } else {
        remaining
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
    fn just_cast_is_full_cooldown() {
        assert_eq!(cd_remaining(5.0, 5.0, 3.0), 3.0);
    }

    #[test]
    fn elapsed_at_or_past_cooldown_is_zero() {
        assert_eq!(cd_remaining(0.0, 10.0, 3.0), 0.0);
        assert_eq!(cd_remaining(0.0, 3.0, 3.0), 0.0); // exactly elapsed
    }

    #[test]
    fn mid_is_partial() {
        assert_eq!(cd_remaining(0.0, 1.0, 3.0), 2.0);
        assert!((cd_remaining(2.0, 3.5, 3.0) - 1.5).abs() < 1e-6);
    }

    #[test]
    fn cooldown_zero_always_ready() {
        let mut seed = 0x0001_u32;
        for _ in 0..1000 {
            let last = (lcg(&mut seed) % 1000) as f32 / 10.0;
            let now = (lcg(&mut seed) % 1000) as f32 / 10.0;
            assert_eq!(cd_remaining(last, now, 0.0), 0.0);
        }
    }

    #[test]
    fn clamped_both_ends() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..5000 {
            let last = (lcg(&mut seed) % 20_001) as f32 / 100.0 - 100.0;
            let now = (lcg(&mut seed) % 20_001) as f32 / 100.0 - 100.0;
            let cd = (lcg(&mut seed) % 20_001) as f32 / 100.0 - 50.0; // incl <= 0
            let r = cd_remaining(last, now, cd);
            let cap = if cd > 0.0 { cd } else { 0.0 };
            assert!(r >= 0.0 && r <= cap, "{} not in [0,{}]", r, cap);
        }
        // now before last_cast (clock skew) → clamps up to full cooldown, not over
        assert_eq!(cd_remaining(10.0, 5.0, 3.0), 3.0);
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let last = (lcg(&mut seed) % 10_001) as f32 / 100.0;
            let now = (lcg(&mut seed) % 10_001) as f32 / 100.0;
            let cd = (lcg(&mut seed) % 10_001) as f32 / 100.0;
            assert_eq!(cd_remaining(last, now, cd), cd_remaining(last, now, cd));
        }
    }
}
