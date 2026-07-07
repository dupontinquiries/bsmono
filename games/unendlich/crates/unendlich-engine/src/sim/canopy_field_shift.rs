//! `sim::canopy_field_shift` — a growing plant as a local elemental field source.
//!
//! Plantgrowth is the one discipline that *edits* the field (spec §6.1). R34
//! `germination` emits a `strain_bias` for rare bloom strains; this module turns
//! a growing plant into a field source: its canopy raises biotic density (`B`)
//! and traps moisture (`H`), and casts shade that lowers light (`L`). The edit
//! magnitude scales with how grown the plant is and how strong its strain is.
//! Pure, deterministic, finite (R1/R6/R11); standalone `rustc --test`-able.
//!
//! Downstream (the coordinator's wiring) the returned [`FieldDelta`] is added
//! into `affinity_access` as a field bonus — a mature high-strain bloom raises
//! local B/H enough to open Plantgrowth/Coral access nearby, the flora analog of
//! an imbued artifact. This core only produces the delta.

/// A growing plant's state relevant to its field edit.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct PlantState {
    /// Bloom-strain lean `0..1` (from `germination`); amplifies the edit.
    pub strain_bias: f32,
    /// Growth progress `0..1` (0 = just sprouted, 1 = mature).
    pub growth01: f32,
    /// Elemental lineage tag (identity for downstream access routing; not used
    /// to shape the H/L/B pattern here).
    pub element: u8,
}

/// The local field edit a plant applies: additive deltas to moisture, light, and
/// biotic fields. `h`/`b` are `>= 0` (raised); `l` is `<= 0` (shaded).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct FieldDelta {
    pub h: f32,
    pub l: f32,
    pub b: f32,
}

/// Peak biotic raise at full magnitude.
const B_MAX: f32 = 0.30;
/// Peak moisture raise at full magnitude.
const H_MAX: f32 = 0.25;
/// Peak light reduction at full magnitude (shade).
const L_MAX: f32 = 0.40;
/// Base edit fraction from growth alone (a plain plant still shifts a little).
const BASE: f32 = 0.4;
/// How much strain_bias amplifies the edit on top of the base.
const STRAIN_GAIN: f32 = 0.6;

fn clamp01(v: f32) -> f32 {
    crate::util::mathx::saturate(v) // r343 dedup
}

/// Field edit for a growing plant. Magnitude is `growth01 · (BASE + STRAIN_GAIN ·
/// strain_bias)`, so it's zero at zero growth and rises with both growth and
/// strain. Raises `b`/`h`, lowers `l`.
pub fn canopy_shift(p: &PlantState) -> FieldDelta {
    let g = clamp01(p.growth01);
    let s = clamp01(p.strain_bias);
    let mag = g * (BASE + STRAIN_GAIN * s); // 0 at g=0; rises with g and s

    FieldDelta {
        h: H_MAX * mag,
        l: -(L_MAX * mag), // shade: light goes down
        b: B_MAX * mag,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plant(strain_bias: f32, growth01: f32) -> PlantState {
        PlantState { strain_bias, growth01, element: 0 }
    }

    #[test]
    fn zero_growth_zero_delta() {
        let d = canopy_shift(&plant(1.0, 0.0));
        assert_eq!(d, FieldDelta { h: 0.0, l: 0.0, b: 0.0 });
    }

    #[test]
    fn monotone_in_growth() {
        let lo = canopy_shift(&plant(0.5, 0.2));
        let hi = canopy_shift(&plant(0.5, 0.8));
        assert!(hi.b > lo.b && hi.h > lo.h, "B/H should rise with growth");
        assert!(hi.l < lo.l, "L should drop (more negative) with growth");
    }

    #[test]
    fn monotone_in_strain_bias() {
        let lo = canopy_shift(&plant(0.1, 1.0));
        let hi = canopy_shift(&plant(0.9, 1.0));
        assert!(hi.b > lo.b && hi.h > lo.h, "B/H should rise with strain_bias");
        assert!(hi.l < lo.l, "L should drop further with strain_bias");
    }

    #[test]
    fn b_h_nonneg_l_lowered_under_canopy() {
        let d = canopy_shift(&plant(0.8, 1.0));
        assert!(d.b >= 0.0 && d.h >= 0.0, "B/H must be non-negative");
        assert!(d.l < 0.0, "a full canopy must lower L");
    }

    #[test]
    fn finite_and_clamps_out_of_range() {
        let mut s = 0x35FA_u64; // documented seed (R20)
        let mut rng = || {
            s = s.wrapping_mul(6364136223846793005).wrapping_add(1);
            (s >> 40) as f32 / ((1u64 << 24) as f32)
        };
        for _ in 0..3000 {
            // deliberately out-of-range inputs are clamped, not propagated
            let d = canopy_shift(&plant(rng() * 4.0 - 2.0, rng() * 4.0 - 2.0));
            assert!(d.h.is_finite() && d.l.is_finite() && d.b.is_finite(), "non-finite delta");
            assert!((0.0..=B_MAX + 1e-6).contains(&d.b), "b oob: {}", d.b);
            assert!((0.0..=H_MAX + 1e-6).contains(&d.h), "h oob: {}", d.h);
            assert!((-(L_MAX) - 1e-6..=0.0).contains(&d.l), "l oob: {}", d.l);
        }
        // NaN inputs sanitise to zero growth ⇒ zero delta
        let nan = canopy_shift(&plant(f32::NAN, f32::NAN));
        assert_eq!(nan, FieldDelta { h: 0.0, l: 0.0, b: 0.0 });
        // saturating inputs cap at the maxima
        let full = canopy_shift(&plant(1.0, 1.0));
        assert!((full.b - B_MAX).abs() < 1e-6 && (full.h - H_MAX).abs() < 1e-6);
        assert!((full.l + L_MAX).abs() < 1e-6);
    }

    #[test]
    fn deterministic() {
        let p = plant(0.4, 0.7);
        assert_eq!(canopy_shift(&p), canopy_shift(&p));
    }
}
