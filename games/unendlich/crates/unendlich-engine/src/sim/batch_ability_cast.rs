//! `sim::batch_ability_cast` — batched ability casting, mirroring
//! `sim::ability_cast` (round 21), with a scalar reference for parity.
//!
//! [`cast_one`] resolves one `(element, charge)` discharge into an [`Effect`]
//! exactly as the r21 core does; [`cast_batch`] resolves many at once (a volley,
//! or every creature firing on the same tick). Both route through the same
//! `#[inline]` `kernel`, so the batch is **bit-for-bit identical** to casting each
//! individually.
//!
//! Perf shape: casts are read from a contiguous `(element, charge)` slice and
//! written to a contiguous `Vec<Effect>` in one streaming pass, no cross-cast
//! dependency. Pure and deterministic — no clock, no randomness, no cross-imports.

/// Number of known elements (identical to the r21 core).
pub const ELEMENT_COUNT: u8 = 6;
/// Kind id meaning "no effect".
pub const KIND_NONE: u8 = 0;
/// Fallback kind for an unknown element.
pub const KIND_FALLBACK: u8 = 1;

const POWER_SCALE: f32 = 2.0;
const MAX_POWER: f32 = 100.0;
const RADIUS_SCALE: f32 = 0.5;
const MAX_RADIUS: f32 = 20.0;

/// A cast ability effect (mirrors r21 `ability_cast::Effect`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Effect {
    pub kind: u8,
    pub power: f32,
    pub radius: f32,
}

fn clampf(x: f32, lo: f32, hi: f32) -> f32 {
    let x = if x > lo { x } else { lo };
    if x < hi {
        x
    } else {
        hi
    }
}

/// The shared cast kernel (identical to r21 `ability_cast::cast`).
#[inline]
fn kernel(element: u8, charge: f32) -> Effect {
    if !(charge > 0.0) {
        return Effect { kind: KIND_NONE, power: 0.0, radius: 0.0 };
    }
    let kind = if element < ELEMENT_COUNT {
        element + 1
    } else {
        KIND_FALLBACK
    };
    Effect {
        kind,
        power: clampf(charge * POWER_SCALE, 0.0, MAX_POWER),
        radius: clampf(charge * RADIUS_SCALE, 0.0, MAX_RADIUS),
    }
}

/// Scalar reference: resolve one cast.
pub fn cast_one(element: u8, charge: f32) -> Effect {
    kernel(element, charge)
}

/// Batched: resolve every `(element, charge)` cast, in order.
pub fn cast_batch(states: &[(u8, f32)]) -> Vec<Effect> {
    crate::util::batch_kernel::map_collect(states, |&(element, charge)| kernel(element, charge))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn rand_state(seed: &mut u32) -> (u8, f32) {
        let el = (lcg(seed) & 0xFF) as u8;
        let charge = (lcg(seed) % 200_001) as f32 / 100.0 - 100.0;
        (el, charge)
    }
    fn same(a: Effect, b: Effect) -> bool {
        a.kind == b.kind
            && a.power.to_bits() == b.power.to_bits()
            && a.radius.to_bits() == b.radius.to_bits()
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
            let batch = cast_batch(&states);
            assert_eq!(batch.len(), states.len());
            for i in 0..states.len() {
                let (e, c) = states[i];
                assert!(same(batch[i], cast_one(e, c)));
            }
        }
    }

    #[test]
    fn empty_yields_empty() {
        assert!(cast_batch(&[]).is_empty());
    }

    #[test]
    fn single_equals_batch_of_one() {
        let mut seed = 0x0001_u32;
        for _ in 0..2000 {
            let (e, c) = rand_state(&mut seed);
            let b = cast_batch(&[(e, c)]);
            assert_eq!(b.len(), 1);
            assert!(same(b[0], cast_one(e, c)));
        }
    }

    #[test]
    fn bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..3000 {
            let (e, c) = rand_state(&mut seed);
            let eff = cast_one(e, c);
            assert!(eff.power >= 0.0 && eff.power <= MAX_POWER);
            assert!(eff.radius >= 0.0 && eff.radius <= MAX_RADIUS);
            assert!(eff.kind <= ELEMENT_COUNT);
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
            assert_eq!(cast_batch(&states), cast_batch(&states));
        }
    }

    #[test]
    fn zero_charge_entry_is_inert() {
        let b = cast_batch(&[(2, 0.0), (3, 5.0)]);
        assert_eq!(b[0], Effect { kind: KIND_NONE, power: 0.0, radius: 0.0 });
        assert_eq!(b[1].kind, 4);
    }
}
