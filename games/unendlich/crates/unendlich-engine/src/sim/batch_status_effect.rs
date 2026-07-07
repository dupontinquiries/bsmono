//! `sim::batch_status_effect` — batched status-effect ticking, mirroring
//! `sim::status_effect` (round 18), with a scalar reference for parity.
//!
//! [`status_one`] ticks one stacking effect exactly as the r18 core does (apply
//! `magnitude · stacks`, then decay the stacks by `floor(DECAY_PER_SEC · dt)`);
//! [`status_batch`] ticks a whole entity's worth of active effects at a shared
//! `dt`, each with its own `(stacks, magnitude)`. Both route through the same
//! `#[inline]` `kernel`, so the batch is **bit-for-bit identical** to ticking
//! each effect on its own.
//!
//! Perf shape: effects are read from a contiguous `(stacks, magnitude)` slice and
//! written to a contiguous `Vec` at one shared `dt`, no cross-effect dependency —
//! one streaming pass per frame instead of a call per active buff/DoT. Pure and
//! deterministic.

/// Stacks lost per second (identical to the r18 core).
pub const DECAY_PER_SEC: f32 = 1.0;

/// The shared arithmetic (identical to r18 `status_effect::tick`).
#[inline]
fn kernel(stacks: u8, magnitude: f32, dt: f32) -> (u8, f32) {
    let raw = magnitude * stacks as f32;
    let applied = if raw > 0.0 { raw } else { 0.0 };

    let dt = if dt > 0.0 { dt } else { 0.0 };
    let lost_f = DECAY_PER_SEC * dt;
    let lost = if lost_f >= u8::MAX as f32 {
        u8::MAX
    } else {
        lost_f as u8
    };
    (stacks.saturating_sub(lost), applied)
}

/// Scalar reference: tick one stacking effect.
pub fn status_one(stacks: u8, magnitude: f32, dt: f32) -> (u8, f32) {
    kernel(stacks, magnitude, dt)
}

/// Batched: tick every effect by the shared `dt`. Each input is
/// `(stacks, magnitude)`; output is `(new_stacks, applied)` in order.
pub fn status_batch(states: &[(u8, f32)], dt: f32) -> Vec<(u8, f32)> {
    crate::util::batch_kernel::map_collect(states, |&(stacks, magnitude)| kernel(stacks, magnitude, dt))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn rand_state(seed: &mut u32) -> (u8, f32) {
        let stacks = (lcg(seed) & 0xFF) as u8;
        let mag = (lcg(seed) % 200_001) as f32 / 1000.0 - 100.0;
        (stacks, mag)
    }
    fn same(a: (u8, f32), b: (u8, f32)) -> bool {
        a.0 == b.0 && a.1.to_bits() == b.1.to_bits()
    }

    #[test]
    fn batch_matches_scalar_bit_for_bit() {
        let mut seed = 0xB17C_u32;
        for _ in 0..2000 {
            let n = (lcg(&mut seed) % 16) as usize;
            let mut states = Vec::new();
            for _ in 0..n {
                states.push(rand_state(&mut seed));
            }
            let dt = (lcg(&mut seed) % 100_000) as f32 / 1000.0;
            let batch = status_batch(&states, dt);
            assert_eq!(batch.len(), states.len());
            for i in 0..states.len() {
                let (s, m) = states[i];
                assert!(same(batch[i], status_one(s, m, dt)));
            }
        }
    }

    #[test]
    fn empty_yields_empty() {
        assert!(status_batch(&[], 1.0).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let mut seed = 0x0001_u32;
        for _ in 0..2000 {
            let (s, m) = rand_state(&mut seed);
            let dt = (lcg(&mut seed) % 100_000) as f32 / 1000.0;
            let b = status_batch(&[(s, m)], dt);
            assert_eq!(b.len(), 1);
            assert!(same(b[0], status_one(s, m, dt)));
        }
    }

    #[test]
    fn bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..3000 {
            let (s, m) = rand_state(&mut seed);
            let dt = (lcg(&mut seed) % 100_000) as f32 / 1000.0;
            let (new, applied) = status_one(s, m, dt);
            assert!(new <= s, "stacks grew");
            assert!(applied >= 0.0, "negative applied");
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..2000 {
            let n = (lcg(&mut seed) % 10) as usize;
            let mut states = Vec::new();
            for _ in 0..n {
                states.push(rand_state(&mut seed));
            }
            let dt = (lcg(&mut seed) % 100_000) as f32 / 1000.0;
            assert_eq!(status_batch(&states, dt), status_batch(&states, dt));
        }
    }

    #[test]
    fn zero_stacks_entry_applies_nothing() {
        let b = status_batch(&[(0, 5.0), (3, 2.0)], 0.0);
        assert_eq!(b[0], (0, 0.0));
        assert_eq!(b[1], (3, 6.0));
    }
}
