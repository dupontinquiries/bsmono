//! `sim::affinity_access` — which bending disciplines the local field unlocks.
//!
//! Resonance Bending (spec §2.2 + Appendix A) has no named-ability registry;
//! instead a bender may use a discipline only where the ambient **field** is
//! strong enough, and how *powerfully* they wield it rises with that field above
//! the access threshold. This module is that gate: ambient [`Fields`] in,
//! [`Access`] out (a bitset of unlocked disciplines + a per-discipline strength).
//!
//! ## Disciplines ↔ fields (one discipline per field channel)
//! The seven disciplines are indexed by the field channel that governs them,
//! matching the canonical channel order `h,r,w,p,l,s,b = 0..6` used by
//! [`super::artifact_imbue`] and [`super::dragon_attunement`]:
//!
//! ```text
//!  i  field  governs discipline      threshold (Appendix A)
//!  0   h     Tideshaping (humidity)        0.45
//!  1   r     Embercraft  (heat)            0.55
//!  2   w     Galecalling (wind)            0.40
//!  3   p     Stonepress  (pressure)        0.60
//!  4   l     Stormcalling(lightning)       0.70
//!  5   s     Sandweaving (storm/particulate)0.50
//!  6   b     Verdancy    (biome richness)  0.35
//! ```
//!
//! A discipline `i` is **unlocked** iff `field[i] >= THRESHOLDS[i]`, and its
//! strength is `(field[i] − threshold) / (1 − threshold)` clamped to `0..1`
//! (0 right at the threshold, 1 when the field is maxed; 0 while locked). So
//! strength is monotonic in the governing field, and an all-zero field unlocks
//! nothing (every threshold is positive — no unconditional baseline).
//!
//! ## Artifact composition (the bonus)
//! Because access reads only `Fields`, adding an imbued artifact's
//! [`super::artifact_imbue::FieldDelta`] (same `h,r,w,p,l,s,b` channels) onto
//! the ambient fields *before* calling [`access`] literally unlocks or
//! strengthens a discipline — a worn ember-stone raises `r` and opens Embercraft
//! where the ambient field alone would not. No special-casing here: the delta
//! and the field live in the same units, so a plain per-channel add (then this
//! gate) is the whole mechanism. (Demonstrated in the tests.)
//!
//! Pure, deterministic, clamps out-of-range/NaN, never panics. No
//! `crate::render`, no `ash`/`vk`, no IO, no clock, no RNG, no
//! `.unwrap()`/`.expect()` in non-test code.

/// Ambient field values, each in `0..1` (clamped internally).
/// `h`=humidity, `r`=heat/radiation, `w`=wind, `p`=pressure, `l`=lightning,
/// `s`=storm/particulate, `b`=biome richness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fields {
    pub h: f32,
    pub r: f32,
    pub w: f32,
    pub p: f32,
    pub l: f32,
    pub s: f32,
    pub b: f32,
}

/// Unlocked disciplines: bit `i` of `disciplines` is set iff discipline `i` is
/// accessible, and `strength[i]` is its `0..1` power (0 when locked).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Access {
    pub disciplines: u8,
    pub strength: [f32; 7],
}

/// Number of disciplines / field channels.
pub const DISCIPLINES: usize = 7;

/// Appendix-A access thresholds, indexed by field channel `h,r,w,p,l,s,b`.
pub const THRESHOLDS: [f32; DISCIPLINES] = [0.45, 0.55, 0.40, 0.60, 0.70, 0.50, 0.35];

/// Clamp to `[0, 1]`; maps NaN to 0 (`f32::max` returns the non-NaN operand).
#[inline]
fn clamp01(x: f32) -> f32 {
    crate::util::mathx::saturate(x) // r343 dedup
}

/// The seven field values in canonical channel order.
#[inline]
fn channels(f: &Fields) -> [f32; DISCIPLINES] {
    [f.h, f.r, f.w, f.p, f.l, f.s, f.b]
}

/// All disciplines whose Appendix-A field threshold is met, with per-discipline
/// strength rising from 0 at the threshold to 1 at a maxed field.
pub fn access(f: &Fields) -> Access {
    let raw = channels(f);
    let mut disciplines: u8 = 0;
    let mut strength = [0.0f32; DISCIPLINES];
    let mut i = 0;
    while i < DISCIPLINES {
        let fi = clamp01(raw[i]);
        let ti = THRESHOLDS[i];
        if fi >= ti {
            disciplines |= 1 << i;
            let denom = (1.0 - ti).max(1e-6); // every ti < 1, guard anyway
            strength[i] = clamp01((fi - ti) / denom);
        }
        // locked ⇒ bit clear, strength stays 0
        i += 1;
    }
    Access { disciplines, strength }
}

/// Is `discipline` (index `0..6`) accessible under field `f`? `false` for any
/// out-of-range index.
pub fn can_access(f: &Fields, discipline: u8) -> bool {
    let d = discipline as usize;
    if d >= DISCIPLINES {
        return false;
    }
    clamp01(channels(f)[d]) >= THRESHOLDS[d]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg(seed: &mut u32) -> u32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        *seed >> 8
    }
    fn randf(seed: &mut u32, lo: f32, hi: f32) -> f32 {
        lo + (lcg(seed) % 1_000_001) as f32 / 1_000_000.0 * (hi - lo)
    }
    fn zero() -> Fields {
        Fields { h: 0.0, r: 0.0, w: 0.0, p: 0.0, l: 0.0, s: 0.0, b: 0.0 }
    }
    /// Fields with one channel set, rest zero.
    fn only(idx: usize, v: f32) -> Fields {
        let mut f = zero();
        match idx {
            0 => f.h = v,
            1 => f.r = v,
            2 => f.w = v,
            3 => f.p = v,
            4 => f.l = v,
            5 => f.s = v,
            _ => f.b = v,
        }
        f
    }

    #[test]
    fn each_appendix_a_row_gates_at_its_threshold() {
        // One assertion-group per Appendix-A row (7 rows).
        for i in 0..DISCIPLINES {
            let ti = THRESHOLDS[i];
            // exactly at threshold -> unlocked (bit set), strength 0
            let at = access(&only(i, ti));
            assert!(at.disciplines & (1 << i) != 0, "row {} should unlock at threshold", i);
            assert!(at.strength[i].abs() < 1e-6, "row {} strength 0 at threshold, got {}", i, at.strength[i]);
            assert!(can_access(&only(i, ti), i as u8));

            // just below threshold -> bit clear & strength 0
            let below = access(&only(i, ti - 0.01));
            assert!(below.disciplines & (1 << i) == 0, "row {} must be locked just below", i);
            assert_eq!(below.strength[i], 0.0, "row {} strength 0 when locked", i);
            assert!(!can_access(&only(i, ti - 0.01), i as u8));

            // maxed field -> strength ~1
            let full = access(&only(i, 1.0));
            assert!((full.strength[i] - 1.0).abs() < 1e-5, "row {} strength ~1 at max, got {}", i, full.strength[i]);

            // a single field unlocks ONLY its own discipline
            assert_eq!(at.disciplines, 1 << i, "row {} leaked into other disciplines", i);
        }
    }

    #[test]
    fn strength_monotone_in_governing_field() {
        for i in 0..DISCIPLINES {
            let ti = THRESHOLDS[i];
            let mut prev = -1.0f32;
            let mut v = 0.0f32;
            while v <= 1.0001 {
                let s = access(&only(i, v)).strength[i];
                assert!(s >= prev - 1e-7, "row {} strength dropped at field {}: {} < {}", i, v, s, prev);
                if v > ti + 1e-3 && v < 0.999 {
                    // strictly increasing above the threshold
                    let s2 = access(&only(i, v + 0.05)).strength[i];
                    assert!(s2 > s, "row {} not strictly increasing above threshold", i);
                }
                prev = s;
                v += 0.02;
            }
        }
    }

    #[test]
    fn all_zero_unlocks_nothing() {
        let a = access(&zero());
        assert_eq!(a.disciplines, 0);
        assert_eq!(a.strength, [0.0; DISCIPLINES]);
        for i in 0..DISCIPLINES {
            assert!(!can_access(&zero(), i as u8));
        }
    }

    #[test]
    fn all_full_unlocks_everything_at_full_strength() {
        let f = Fields { h: 1.0, r: 1.0, w: 1.0, p: 1.0, l: 1.0, s: 1.0, b: 1.0 };
        let a = access(&f);
        assert_eq!(a.disciplines, 0b0111_1111, "all 7 disciplines set");
        for i in 0..DISCIPLINES {
            assert!((a.strength[i] - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn can_access_matches_access_bits_and_rejects_out_of_range() {
        let mut s = 0xACCE_0000u32;
        for _ in 0..20000 {
            let f = Fields {
                h: randf(&mut s, -0.2, 1.2),
                r: randf(&mut s, -0.2, 1.2),
                w: randf(&mut s, -0.2, 1.2),
                p: randf(&mut s, -0.2, 1.2),
                l: randf(&mut s, -0.2, 1.2),
                s: randf(&mut s, -0.2, 1.2),
                b: randf(&mut s, -0.2, 1.2),
            };
            let a = access(&f);
            for i in 0..DISCIPLINES {
                assert_eq!(can_access(&f, i as u8), a.disciplines & (1 << i) != 0);
            }
            // out of range disciplines are never accessible
            assert!(!can_access(&f, 7));
            assert!(!can_access(&f, 200));
        }
    }

    #[test]
    fn nan_and_out_of_range_are_clamped() {
        let nan = f32::NAN;
        // NaN clamps to 0 -> locked everywhere
        let f_nan = Fields { h: nan, r: nan, w: nan, p: nan, l: nan, s: nan, b: nan };
        let a = access(&f_nan);
        assert_eq!(a.disciplines, 0);
        assert_eq!(a.strength, [0.0; DISCIPLINES]);

        // huge / negative clamp to 1 / 0
        let f_big = Fields { h: 1e9, r: -1e9, w: 50.0, p: -3.0, l: f32::INFINITY, s: 2.0, b: -0.0 };
        let a2 = access(&f_big);
        // h,w,l,s clamp to 1 (unlocked, strength 1); r,p,b clamp to 0 (locked)
        assert!(a2.disciplines & (1 << 0) != 0); // h
        assert!(a2.disciplines & (1 << 2) != 0); // w
        assert!(a2.disciplines & (1 << 4) != 0); // l
        assert!(a2.disciplines & (1 << 5) != 0); // s
        assert!(a2.disciplines & (1 << 1) == 0); // r locked
        assert!(a2.disciplines & (1 << 3) == 0); // p locked
        assert!(a2.disciplines & (1 << 6) == 0); // b locked
        for v in a2.strength.iter() {
            assert!(v.is_finite() && *v >= 0.0 && *v <= 1.0);
        }
    }

    #[test]
    fn deterministic() {
        let mut s = 0xACCE_0001u32;
        for _ in 0..5000 {
            let f = Fields {
                h: randf(&mut s, 0.0, 1.0),
                r: randf(&mut s, 0.0, 1.0),
                w: randf(&mut s, 0.0, 1.0),
                p: randf(&mut s, 0.0, 1.0),
                l: randf(&mut s, 0.0, 1.0),
                s: randf(&mut s, 0.0, 1.0),
                b: randf(&mut s, 0.0, 1.0),
            };
            assert_eq!(access(&f), access(&f));
        }
    }

    #[test]
    fn artifact_delta_unlocks_a_discipline() {
        // Ambient heat just below Embercraft's threshold: locked.
        let ambient = Fields { r: THRESHOLDS[1] - 0.1, ..zero() };
        assert!(!can_access(&ambient, 1), "Embercraft should be locked on ambient alone");

        // A worn ember-stone (artifact_imbue Fire delta targets the `r` channel)
        // adds +0.2 to r. Composing it before the access call unlocks Embercraft.
        let ember_delta_r = 0.2f32;
        let with_artifact = Fields { r: ambient.r + ember_delta_r, ..ambient };
        assert!(can_access(&with_artifact, 1), "ember-stone should open Embercraft");
        assert!(access(&with_artifact).strength[1] > 0.0);
        // and it strengthens with a stronger artifact
        let stronger = Fields { r: ambient.r + 0.5, ..ambient };
        assert!(access(&stronger).strength[1] > access(&with_artifact).strength[1]);
    }
}
