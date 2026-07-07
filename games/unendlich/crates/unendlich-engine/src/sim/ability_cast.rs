//! `sim::ability_cast` — turn a horn discharge into a typed ability effect
//! (follows `horn_charge`, round 20).
//!
//! [`cast`] takes the discharging horn's `element` and the `charge` released, and
//! produces an [`Effect`]: a kind (chosen by element), a power, and a radius
//! (both scaling with charge). More charge means a bigger, stronger effect;
//! discharging with no charge produces the inert "none" effect; an unknown
//! element falls back to the basic kind rather than failing. Power and radius are
//! capped so a hugely overcharged horn can't produce an unbounded blast.
//!
//! Pure and deterministic: element→kind lookup and clamped linear scaling — no
//! clock, no randomness, no cross-imports.

/// Number of known elements (Wood, Water, Air, Dust, Fire, Storm — cf. round 7).
pub const ELEMENT_COUNT: u8 = 6;
/// Kind id meaning "no effect".
pub const KIND_NONE: u8 = 0;
/// Fallback kind for an unknown element.
pub const KIND_FALLBACK: u8 = 1;

const POWER_SCALE: f32 = 2.0;
const MAX_POWER: f32 = 100.0;
const RADIUS_SCALE: f32 = 0.5;
const MAX_RADIUS: f32 = 20.0;

/// A cast ability effect.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Effect {
    /// Effect kind: `KIND_NONE` when inert, else `element + 1` (or the fallback).
    pub kind: u8,
    /// Effect power (`0..=MAX_POWER`).
    pub power: f32,
    /// Effect radius (`0..=MAX_RADIUS`).
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

/// Cast an ability from a discharging horn.
///
/// `charge <= 0` returns the inert `KIND_NONE` effect. Otherwise the kind is
/// `element + 1` for known elements (or `KIND_FALLBACK` for unknown ones), and
/// power/radius scale with charge up to their caps.
pub fn cast(element: u8, charge: f32) -> Effect {
    if !(charge > 0.0) {
        return Effect {
            kind: KIND_NONE,
            power: 0.0,
            radius: 0.0,
        };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }

    #[test]
    fn more_charge_more_power_and_radius() {
        let small = cast(2, 1.0);
        let big = cast(2, 10.0);
        assert!(big.power > small.power);
        assert!(big.radius > small.radius);
    }

    #[test]
    fn element_selects_kind() {
        assert_eq!(cast(0, 5.0).kind, 1);
        assert_eq!(cast(3, 5.0).kind, 4);
        assert_ne!(cast(0, 5.0).kind, cast(3, 5.0).kind);
    }

    #[test]
    fn zero_charge_no_effect() {
        let e = cast(2, 0.0);
        assert_eq!(e, Effect { kind: KIND_NONE, power: 0.0, radius: 0.0 });
        // negative charge too
        assert_eq!(cast(2, -5.0).kind, KIND_NONE);
    }

    #[test]
    fn bounded() {
        let mut seed = 0xB0AD_u32;
        for _ in 0..5000 {
            let el = (lcg(&mut seed) & 0xFF) as u8;
            let charge = (lcg(&mut seed) % 200_001) as f32 / 100.0 - 100.0; // incl negatives
            let e = cast(el, charge);
            assert!(e.power >= 0.0 && e.power <= MAX_POWER, "power {}", e.power);
            assert!(e.radius >= 0.0 && e.radius <= MAX_RADIUS, "radius {}", e.radius);
            assert!(e.kind <= ELEMENT_COUNT, "kind {}", e.kind);
        }
    }

    #[test]
    fn element_fallback_for_unknown() {
        let e = cast(200, 5.0);
        assert_eq!(e.kind, KIND_FALLBACK);
        assert!(e.power > 0.0); // still a real cast
    }

    #[test]
    fn deterministic() {
        let mut seed = 0xD37E_u32;
        for _ in 0..3000 {
            let el = (lcg(&mut seed) & 0xFF) as u8;
            let charge = (lcg(&mut seed) % 100_001) as f32 / 100.0;
            assert_eq!(cast(el, charge), cast(el, charge));
        }
    }
}
