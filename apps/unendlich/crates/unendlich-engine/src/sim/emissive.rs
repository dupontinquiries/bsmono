//! `sim::emissive` — element + arousal → the **glow** painted onto a creature:
//! the lit eyes of a horror mob, the kindling neck of a dragon, the seams that
//! flare when something is about to act.
//!
//! Reactive tier: this runs every tick for every visible creature, so it is a
//! cheap pure function — no allocation, no clock, no randomness. Given an
//! [`Element`], an `arousal` (how worked-up the creature is, `0..1`) and an
//! `elem_strength` (how strongly it expresses that element, `0..1`), it returns
//! an [`Emissive`]: a fixed per-element hue, a bounded intensity, and the set of
//! body [`regions`](Emissive::regions) that light up.
//!
//! The colour is presentation only; nothing here reads or writes the renderer.
//! It is kept here, in the sim, so the *decision* to glow is deterministic and
//! identical on every peer — the renderer is a read-only consumer of the result.
//!
//! `Element` here is a small **self-contained** set (the brief's palette, with
//! `Storm` rather than the engine's `Lightning`); reconciling it with
//! `sim::elemental::Element` is a documented follow-up, kept out of this file so
//! it compiles and tests standalone.

/// The emissive palette for this core. Deliberately self-contained so the file
/// builds under `rustc --test` with no crate imports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Element {
    Wood,
    Water,
    Air,
    Dust,
    Fire,
    Storm,
}

/// Body regions that can light up, as a bitmask in a single `u8`.
///
/// Used as `RegionMask` bits inside [`Emissive::regions`]; combine with `|`.
pub mod region {
    /// The eyes — the classic horror-mob tell.
    pub const EYES: u8 = 1 << 0;
    /// The neck / throat — a charging dragon.
    pub const NECK: u8 = 1 << 1;
    /// The mouth / maw.
    pub const MOUTH: u8 = 1 << 2;
    /// The spine ridge.
    pub const SPINE: u8 = 1 << 3;
    /// The trailing edge of wings.
    pub const WING_EDGE: u8 = 1 << 4;
}

/// The computed glow for one creature this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Emissive {
    /// Linear RGB hue, each channel `0..1`. Fixed per [`Element`].
    pub color: [f32; 3],
    /// Glow strength, bounded to `[0, 1]`.
    pub intensity: f32,
    /// Bitmask of [`region`] bits that are lit. Always non-zero.
    pub regions: u8,
}

/// Clamp an `f32` into `[lo, hi]` without any panic path. NaN folds toward `lo`.
fn clampf(x: f32, lo: f32, hi: f32) -> f32 {
    // `max` then `min`; if x is NaN, `max(lo)` yields lo (NaN-vs-number max
    // returns the number), keeping the result in range and deterministic.
    let x = if x > lo { x } else { lo };
    if x < hi {
        x
    } else {
        hi
    }
}

/// The fixed hue for an element. Each element is a visibly distinct colour.
fn hue(el: Element) -> [f32; 3] {
    match el {
        Element::Wood => [0.30, 0.80, 0.25],  // green
        Element::Water => [0.20, 0.55, 0.95], // blue
        Element::Air => [0.80, 0.95, 1.00],   // pale cyan
        Element::Dust => [0.85, 0.70, 0.45],  // tan
        Element::Fire => [1.00, 0.45, 0.10],  // orange-red
        Element::Storm => [0.65, 0.35, 0.95], // violet
    }
}

/// The regions an element lights. Always non-zero.
fn regions_for(el: Element) -> u8 {
    use region::*;
    match el {
        Element::Wood => SPINE | NECK,
        Element::Water => NECK | MOUTH,
        Element::Air => WING_EDGE | EYES,
        Element::Dust => SPINE | WING_EDGE,
        Element::Fire => EYES | MOUTH,
        Element::Storm => EYES | NECK | WING_EDGE,
    }
}

/// Compute the glow for a creature.
///
/// `arousal` and `elem_strength` are treated as `0..1` (values outside are
/// clamped). Intensity is their product, so it is `0` when either input is `0`,
/// non-decreasing in each input, and never leaves `[0, 1]`.
pub fn emissive_for(el: Element, arousal: f32, elem_strength: f32) -> Emissive {
    let a = clampf(arousal, 0.0, 1.0);
    let s = clampf(elem_strength, 0.0, 1.0);
    Emissive {
        color: hue(el),
        intensity: clampf(a * s, 0.0, 1.0),
        regions: regions_for(el),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Element; 6] = [
        Element::Wood,
        Element::Water,
        Element::Air,
        Element::Dust,
        Element::Fire,
        Element::Storm,
    ];

    /// Small deterministic LCG → `f32` in `[0, 1]`, for property tests.
    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn unit(seed: &mut u32) -> f32 {
        (lcg(seed) % 100_001) as f32 / 100_000.0
    }

    #[test]
    fn intensity_zero_when_both_inputs_zero() {
        for el in ALL {
            assert_eq!(emissive_for(el, 0.0, 0.0).intensity, 0.0);
        }
    }

    #[test]
    fn intensity_zero_when_either_input_zero() {
        for el in ALL {
            assert_eq!(emissive_for(el, 0.0, 1.0).intensity, 0.0);
            assert_eq!(emissive_for(el, 1.0, 0.0).intensity, 0.0);
        }
    }

    #[test]
    fn each_element_has_a_distinct_hue() {
        for i in 0..ALL.len() {
            for j in (i + 1)..ALL.len() {
                assert_ne!(
                    hue(ALL[i]),
                    hue(ALL[j]),
                    "elements {:?} and {:?} share a hue",
                    ALL[i],
                    ALL[j]
                );
            }
        }
    }

    #[test]
    fn regions_nonzero_for_all_elements() {
        for el in ALL {
            assert_ne!(emissive_for(el, 0.5, 0.5).regions, 0);
        }
    }

    #[test]
    fn deterministic() {
        let mut seed = 0x51A2_u32;
        for _ in 0..500 {
            let el = ALL[(lcg(&mut seed) as usize) % ALL.len()];
            let a = unit(&mut seed);
            let s = unit(&mut seed);
            assert_eq!(emissive_for(el, a, s), emissive_for(el, a, s));
        }
    }

    #[test]
    fn prop_intensity_in_unit_range_even_for_wild_inputs() {
        let mut seed = 0xBEEF_u32;
        for _ in 0..2000 {
            let el = ALL[(lcg(&mut seed) as usize) % ALL.len()];
            // include out-of-domain and negative inputs
            let a = unit(&mut seed) * 4.0 - 1.5;
            let s = unit(&mut seed) * 4.0 - 1.5;
            let e = emissive_for(el, a, s);
            assert!(e.intensity >= 0.0 && e.intensity <= 1.0, "got {}", e.intensity);
            for c in e.color {
                assert!(c >= 0.0 && c <= 1.0);
            }
        }
    }

    #[test]
    fn prop_monotonic_in_arousal() {
        let mut seed = 0x1234_u32;
        for _ in 0..2000 {
            let el = ALL[(lcg(&mut seed) as usize) % ALL.len()];
            let s = unit(&mut seed);
            let a1 = unit(&mut seed);
            let a2 = unit(&mut seed);
            let lo = a1.min(a2);
            let hi = a1.max(a2);
            let i_lo = emissive_for(el, lo, s).intensity;
            let i_hi = emissive_for(el, hi, s).intensity;
            assert!(i_hi >= i_lo, "not monotonic: {} then {}", i_lo, i_hi);
        }
    }

    #[test]
    fn prop_monotonic_in_elem_strength() {
        let mut seed = 0x77AA_u32;
        for _ in 0..2000 {
            let el = ALL[(lcg(&mut seed) as usize) % ALL.len()];
            let a = unit(&mut seed);
            let s1 = unit(&mut seed);
            let s2 = unit(&mut seed);
            let lo = s1.min(s2);
            let hi = s1.max(s2);
            let i_lo = emissive_for(el, a, lo).intensity;
            let i_hi = emissive_for(el, a, hi).intensity;
            assert!(i_hi >= i_lo);
        }
    }
}
