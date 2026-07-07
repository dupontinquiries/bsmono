//! `sim::status_effect` — tick a stacking status effect (a DoT, a buff) that
//! fades over time.
//!
//! [`tick`] applies the effect for one step and decays its stacks. The amount
//! applied this tick is `magnitude · stacks` (floored at `0`, so a non-positive
//! magnitude does nothing), and the stacks then drop by `floor(DECAY_PER_SEC ·
//! dt)`, so the effect weakens as time passes and eventually expires. A `dt` of
//! `0` applies the effect without decaying it; zero stacks apply nothing.
//!
//! Pure and deterministic: a multiply, a clamp, and a saturating stack drop —
//! no clock, no randomness, no cross-imports.

/// Stacks lost per second of elapsed time.
pub const DECAY_PER_SEC: f32 = 1.0;

/// Tick the effect once.
///
/// Returns `(new_stacks, applied)` where `applied = max(0, magnitude · stacks)`
/// for the current stacks, and `new_stacks = stacks - floor(DECAY_PER_SEC · dt)`
/// (saturating at `0`). Negative `dt` decays nothing.
pub fn tick(stacks: u8, magnitude: f32, dt: f32) -> (u8, f32) {
    // effect applied this tick, using the current stacks; never negative
    let raw = magnitude * stacks as f32;
    let applied = if raw > 0.0 { raw } else { 0.0 };

    // decay stacks by whole steps of elapsed time
    let dt = if dt > 0.0 { dt } else { 0.0 };
    let lost_f = DECAY_PER_SEC * dt;
    let lost = if lost_f >= u8::MAX as f32 {
        u8::MAX
    } else {
        lost_f as u8
    };
    let new_stacks = stacks.saturating_sub(lost);

    (new_stacks, applied)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn applies_magnitude_times_stacks() {
        let (_, applied) = tick(3, 2.0, 0.0);
        assert_eq!(applied, 6.0);
    }

    #[test]
    fn decays_over_time() {
        // 2 seconds at 1 stack/sec → lose 2 stacks
        let (new, _) = tick(5, 1.0, 2.0);
        assert_eq!(new, 3);
        // enough time wipes it out entirely
        let (gone, _) = tick(5, 1.0, 100.0);
        assert_eq!(gone, 0);
    }

    #[test]
    fn zero_stacks_apply_nothing() {
        let (new, applied) = tick(0, 5.0, 1.0);
        assert_eq!(new, 0);
        assert_eq!(applied, 0.0);
    }

    #[test]
    fn applied_clamped_nonnegative() {
        // a negative magnitude must not apply negative effect
        let (_, applied) = tick(4, -3.0, 0.0);
        assert_eq!(applied, 0.0);
    }

    #[test]
    fn dt_zero_does_not_decay() {
        let (new, applied) = tick(7, 2.0, 0.0);
        assert_eq!(new, 7);
        assert_eq!(applied, 14.0);
        // negative dt also doesn't decay
        assert_eq!(tick(7, 2.0, -5.0).0, 7);
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let stacks = (lcg(&mut seed) & 0xFF) as u8;
            let mag = (lcg(&mut seed) % 200_001) as f32 / 1000.0 - 100.0;
            let dt = (lcg(&mut seed) % 100_001) as f32 / 1000.0;
            assert_eq!(tick(stacks, mag, dt), tick(stacks, mag, dt));
        }
    }
}
