//! `sim::batch_settle` — batched foot/ground settling, with a scalar reference.
//!
//! A contact (a foot, a hoof, a landing paw) eases its height toward the ground
//! beneath it, but no faster than `max_step` per call so it doesn't snap.
//! [`settle_one`] is the reference; [`settle_batch`] settles a whole skeleton's
//! worth of contacts in one pass. Both call the same `#[inline]` `kernel`, so
//! the batch result is **bit-for-bit identical** to the scalar path — the only
//! difference is iteration shape.
//!
//! Perf shape: contacts are independent, read from a contiguous slice and
//! written to a contiguous `Vec`, so the loop streams and vectorizes; nothing
//! here forces a per-contact indirect call.
//!
//! Pure and deterministic: no clock, no randomness, no cross-imports.

/// One foot/ground contact to settle.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Contact {
    /// Current height of the contact point.
    pub height: f32,
    /// Ground height beneath it (the rest target).
    pub ground: f32,
    /// Maximum settle step per call (negative treated as zero).
    pub max_step: f32,
}

/// The shared arithmetic: move `height` toward `ground`, capped at `max_step`,
/// never overshooting the ground.
#[inline]
fn kernel(c: &Contact) -> f32 {
    let step = if c.max_step > 0.0 { c.max_step } else { 0.0 };
    let delta = c.ground - c.height;
    let clamped = if delta > step {
        step
    } else if delta < -step {
        -step
    } else {
        delta
    };
    c.height + clamped
}

/// Scalar reference: settle one contact, returning its new height.
pub fn settle_one(c: &Contact) -> f32 {
    kernel(c)
}

/// Batched: settle every contact, returning new heights in order.
pub fn settle_batch(contacts: &[Contact]) -> Vec<f32> {
    crate::util::batch_kernel::map_collect(contacts, |c| kernel(c))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn rf(seed: &mut u32) -> f32 {
        (lcg(seed) % 200_001) as f32 / 1000.0 - 100.0
    }
    fn rand_contact(seed: &mut u32) -> Contact {
        Contact {
            height: rf(seed),
            ground: rf(seed),
            max_step: (lcg(seed) % 100_001) as f32 / 1000.0, // [0, 100]
        }
    }

    #[test]
    fn batch_matches_scalar_bit_for_bit() {
        let mut seed = 0x5E77_u32;
        for _ in 0..3000 {
            let n = (lcg(&mut seed) % 16) as usize;
            let mut cs = Vec::new();
            for _ in 0..n {
                cs.push(rand_contact(&mut seed));
            }
            let batch = settle_batch(&cs);
            assert_eq!(batch.len(), cs.len());
            for i in 0..cs.len() {
                assert_eq!(batch[i].to_bits(), settle_one(&cs[i]).to_bits());
            }
        }
    }

    #[test]
    fn single_equals_batch_of_one() {
        let mut seed = 0x0001_u32;
        for _ in 0..2000 {
            let c = rand_contact(&mut seed);
            let b = settle_batch(&[c]);
            assert_eq!(b.len(), 1);
            assert_eq!(b[0].to_bits(), settle_one(&c).to_bits());
        }
    }

    #[test]
    fn idempotent_at_rest() {
        let c = Contact {
            height: 3.0,
            ground: 3.0,
            max_step: 0.5,
        };
        // already at the ground → unchanged, and stays put on re-settle
        let once = settle_one(&c);
        assert_eq!(once, 3.0);
        let twice = settle_one(&Contact {
            height: once,
            ground: 3.0,
            max_step: 0.5,
        });
        assert_eq!(twice, 3.0);
    }

    #[test]
    fn bounded_between_height_and_ground() {
        let mut seed = 0xB0BD_u32;
        for _ in 0..3000 {
            let c = rand_contact(&mut seed);
            let r = settle_one(&c);
            let lo = c.height.min(c.ground);
            let hi = c.height.max(c.ground);
            assert!(r >= lo - 1e-4 && r <= hi + 1e-4, "{} not in [{},{}]", r, lo, hi);
        }
    }

    #[test]
    fn empty_yields_empty() {
        assert!(settle_batch(&[]).is_empty());
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD3DE_u32;
        for _ in 0..2000 {
            let n = (lcg(&mut seed) % 8) as usize;
            let mut cs = Vec::new();
            for _ in 0..n {
                cs.push(rand_contact(&mut seed));
            }
            assert_eq!(settle_batch(&cs), settle_batch(&cs));
        }
    }
}
