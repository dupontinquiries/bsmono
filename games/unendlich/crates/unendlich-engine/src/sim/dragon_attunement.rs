//! `sim::dragon_attunement` — element + field affinity -> breath power scale.
//!
//! A high-`attunement01` dragon's breath power tracks its element's governing
//! field sharply; a low-attunement dragon is field-independent (flat scale).
//! An optional `sub_discipline` (rare sub-element id) expresses only when the
//! dragon is highly attuned AND its rare field thresholds are all met.
//!
//! ## Element → governing field(s) (spec §14.3)
//!
//! | Element | Primary field      | Rare sub-discipline threshold            |
//! |---------|--------------------|------------------------------------------|
//! | Fire    | R (heat/radiation) | Lava (id=0): S high & R high             |
//! | Water   | H (humidity)       | Ice  (id=1): H high & R low              |
//! | Air     | W (wind)           | Gale (id=2): W high & P high             |
//! | Dust    | S (storm/particulate)| Ash (id=3): S high & L high             |
//! | Storm   | W & R              | Thunder (id=4): L high & W high          |
//! | Wood    | B (biome) & H      | Blight (id=5): B high & S high           |
//!
//! ## Scale formula
//! `field_affinity` = the element's governing field value (or average of two),
//! clamped to [0,1]. Then:
//!
//! ```text
//! scale = BASE + attunement01 * field_affinity * PEAK
//! ```
//! where `BASE = 0.5` (low-attune floor) and `PEAK = 1.0` (max bonus).
//! At attunement=0 the scale is constant `BASE` regardless of fields.
//! At attunement=1 and field_affinity=1 the scale is `BASE + PEAK = 1.5`.
//! Scale is always in `(0, 1.5]` and finite.
//!
//! ## Sub-discipline thresholds
//! A `sub_discipline` expresses only when `attunement01 >= SUB_ATTUNE_MIN`
//! AND all of its threshold fields are `>= SUB_FIELD_MIN`. Otherwise `None`.

/// Field values — each in [0, 1]; clamped internally.
/// h=humidity, r=heat/radiation, w=wind, p=pressure,
/// l=lightning, s=storm/particulate, b=biome richness.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fields {
    pub h: f32, // humidity
    pub r: f32, // heat / radiation
    pub w: f32, // wind
    pub p: f32, // pressure
    pub l: f32, // lightning
    pub s: f32, // storm / particulate
    pub b: f32, // biome richness
}

impl Fields {
    /// All fields at zero (no affinity).
    pub fn zero() -> Self {
        Self { h: 0.0, r: 0.0, w: 0.0, p: 0.0, l: 0.0, s: 0.0, b: 0.0 }
    }
    /// All fields at maximum.
    pub fn full() -> Self {
        Self { h: 1.0, r: 1.0, w: 1.0, p: 1.0, l: 1.0, s: 1.0, b: 1.0 }
    }
}

/// The six dragon breath elements (matches `elemental_multiplier` palette:
/// Wood=0, Water=1, Air=2, Dust=3, Fire=4, Storm=5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Element { Wood, Water, Air, Dust, Fire, Storm }

/// Result of attunement evaluation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Attuned {
    /// Breath power multiplier. Always finite and > 0.
    pub scale: f32,
    /// Rare sub-discipline id, or `None`. Expressed only when attunement is
    /// high enough and the element's rare field thresholds are all met.
    /// Known ids: Lava=0, Ice=1, Gale=2, Ash=3, Thunder=4, Blight=5.
    pub sub_discipline: Option<u8>,
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Scale at attunement=0 (or field_affinity=0): the floor.
const BASE: f32 = 0.5;
/// Maximum bonus on top of BASE (reached at attunement=1, field_affinity=1).
const PEAK: f32 = 1.0;
/// Minimum attunement for a sub-discipline to express.
const SUB_ATTUNE_MIN: f32 = 0.75;
/// Minimum field value for each of a sub-discipline's threshold fields.
const SUB_FIELD_MIN: f32  = 0.65;
/// Threshold below which a field counts as "low" for inverse-field subs (e.g. Ice: R low).
const FIELD_LOW_MAX: f32  = 0.35;

// ---------------------------------------------------------------------------
// Core function
// ---------------------------------------------------------------------------

/// Clamp a field value to [0, 1].
#[inline(always)]
fn cf(v: f32) -> f32 { v.clamp(0.0, 1.0) }

/// Compute attunement result for a dragon breathing `element` with genome
/// `attunement01` in field state `f`.
///
/// Out-of-range values are clamped silently. Pure and deterministic.
pub fn attune(element: Element, attunement01: f32, f: &Fields) -> Attuned {
    let att = cf(attunement01);

    // Clamp all fields.
    let h = cf(f.h); let r = cf(f.r); let w = cf(f.w);
    let p = cf(f.p); let l = cf(f.l); let s = cf(f.s); let b = cf(f.b);

    // Compute field_affinity (primary governing field, 0..1) and check sub.
    let (field_affinity, sub_id, sub_fields_met) = match element {
        Element::Fire  => {
            // Primary: R (heat). Rare: Lava (id=0) when S high & R high.
            let aff = r;
            let sub = s >= SUB_FIELD_MIN && r >= SUB_FIELD_MIN;
            (aff, 0u8, sub)
        }
        Element::Water => {
            // Primary: H. Rare: Ice (id=1) when H high & R low.
            let aff = h;
            let sub = h >= SUB_FIELD_MIN && r <= FIELD_LOW_MAX;
            (aff, 1u8, sub)
        }
        Element::Air   => {
            // Primary: W. Rare: Gale (id=2) when W high & P high.
            let aff = w;
            let sub = w >= SUB_FIELD_MIN && p >= SUB_FIELD_MIN;
            (aff, 2u8, sub)
        }
        Element::Dust  => {
            // Primary: S. Rare: Ash (id=3) when S high & L high.
            let aff = s;
            let sub = s >= SUB_FIELD_MIN && l >= SUB_FIELD_MIN;
            (aff, 3u8, sub)
        }
        Element::Storm => {
            // Primary: avg(W, R). Rare: Thunder (id=4) when L high & W high.
            let aff = (w + r) * 0.5;
            let sub = l >= SUB_FIELD_MIN && w >= SUB_FIELD_MIN;
            (aff, 4u8, sub)
        }
        Element::Wood  => {
            // Primary: avg(B, H). Rare: Blight (id=5) when B high & S high.
            let aff = (b + h) * 0.5;
            let sub = b >= SUB_FIELD_MIN && s >= SUB_FIELD_MIN;
            (aff, 5u8, sub)
        }
    };

    // scale = BASE + attunement * field_affinity * PEAK
    let scale = BASE + att * field_affinity * PEAK;

    // Sub-discipline: only when attunement is high enough AND fields met.
    let sub_discipline = if att >= SUB_ATTUNE_MIN && sub_fields_met {
        Some(sub_id)
    } else {
        None
    };

    Attuned { scale, sub_discipline }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ---- helpers -----------------------------------------------------------

    fn high_att() -> f32 { 1.0 }
    fn low_att()  -> f32 { 0.0 }

    fn assert_finite_positive(a: &Attuned) {
        assert!(a.scale.is_finite(), "scale not finite: {}", a.scale);
        assert!(a.scale > 0.0, "scale not positive: {}", a.scale);
    }

    // ---- brief-specified tests ---------------------------------------------

    #[test]
    fn high_attune_fire_high_r_is_high_scale() {
        let f = Fields { r: 1.0, ..Fields::zero() };
        let a = attune(Element::Fire, high_att(), &f);
        // scale = 0.5 + 1.0 * 1.0 * 1.0 = 1.5
        assert!((a.scale - 1.5).abs() < 1e-5, "expected 1.5, got {}", a.scale);
    }

    #[test]
    fn high_attune_fire_low_r_is_muted() {
        let f = Fields { r: 0.0, ..Fields::zero() };
        let a = attune(Element::Fire, high_att(), &f);
        // scale = 0.5 + 1.0 * 0.0 * 1.0 = 0.5
        assert!((a.scale - 0.5).abs() < 1e-5, "expected 0.5, got {}", a.scale);
    }

    #[test]
    fn low_attune_constant_across_fields() {
        // At attunement=0, scale==BASE regardless of all field values.
        let cases = [Fields::zero(), Fields::full(), Fields { r: 0.9, h: 0.1, ..Fields::zero() }];
        for f in &cases {
            let a = attune(Element::Fire, low_att(), f);
            assert!((a.scale - BASE).abs() < 1e-5,
                "low-attune scale should be BASE={BASE}, got {}", a.scale);
        }
    }

    #[test]
    fn sub_discipline_expresses_when_threshold_met() {
        // Lava (Fire): S high & R high
        let f = Fields { r: 1.0, s: 1.0, ..Fields::zero() };
        let a = attune(Element::Fire, 1.0, &f);
        assert_eq!(a.sub_discipline, Some(0), "Lava should express");
    }

    #[test]
    fn sub_discipline_none_when_threshold_not_met() {
        // Lava needs S high — without it, no sub.
        let f = Fields { r: 1.0, s: 0.0, ..Fields::zero() };
        let a = attune(Element::Fire, 1.0, &f);
        assert_eq!(a.sub_discipline, None, "Lava should NOT express without S");
    }

    #[test]
    fn sub_discipline_none_when_attunement_too_low() {
        // All fields maxed but attunement below threshold.
        let a = attune(Element::Fire, 0.5, &Fields::full());
        assert_eq!(a.sub_discipline, None, "sub should not express at low attunement");
    }

    #[test]
    fn scale_finite_and_positive() {
        let cases = [
            (Element::Fire,  0.0, Fields::zero()),
            (Element::Water, 0.5, Fields::full()),
            (Element::Storm, 1.0, Fields { w: 0.8, r: 0.6, ..Fields::zero() }),
            (Element::Wood,  1.0, Fields { b: 1.0, h: 0.5, ..Fields::zero() }),
        ];
        for (el, att, f) in &cases {
            assert_finite_positive(&attune(*el, *att, f));
        }
    }

    #[test]
    fn out_of_range_fields_clamped() {
        let f_hi = Fields { r: 5.0, h: 9.0, w: 3.0, p: -1.0, l: 2.0, s: -0.5, b: 8.0 };
        let f_lo = Fields { r: -2.0, h: -1.0, w: -3.0, p: -4.0, l: -1.0, s: -1.0, b: -2.0 };
        // Should equal clamped versions (full and zero).
        let a_hi = attune(Element::Fire, 1.0, &f_hi);
        let a_full = attune(Element::Fire, 1.0, &Fields::full());
        assert!((a_hi.scale - a_full.scale).abs() < 1e-5);

        let a_lo = attune(Element::Fire, 1.0, &f_lo);
        let a_zero = attune(Element::Fire, 1.0, &Fields::zero());
        assert!((a_lo.scale - a_zero.scale).abs() < 1e-5);
    }

    #[test]
    fn out_of_range_attunement_clamped() {
        let f = Fields { r: 1.0, ..Fields::zero() };
        let a_hi = attune(Element::Fire, 5.0, &f);
        let a_one = attune(Element::Fire, 1.0, &f);
        assert!((a_hi.scale - a_one.scale).abs() < 1e-5);

        let a_lo = attune(Element::Fire, -2.0, &f);
        let a_zero = attune(Element::Fire, 0.0, &f);
        assert!((a_lo.scale - a_zero.scale).abs() < 1e-5);
    }

    #[test]
    fn deterministic() {
        let f = Fields { r: 0.7, s: 0.8, ..Fields::zero() };
        let a = attune(Element::Fire, 0.9, &f);
        let b = attune(Element::Fire, 0.9, &f);
        assert_eq!(a.scale.to_bits(), b.scale.to_bits());
        assert_eq!(a.sub_discipline, b.sub_discipline);
    }

    // ---- per-element sub-discipline tests ----------------------------------

    #[test]
    fn ice_expresses_when_h_high_r_low() {
        let f = Fields { h: 1.0, r: 0.1, ..Fields::zero() };
        let a = attune(Element::Water, 1.0, &f);
        assert_eq!(a.sub_discipline, Some(1), "Ice should express");
    }

    #[test]
    fn ice_absent_when_r_not_low() {
        let f = Fields { h: 1.0, r: 0.9, ..Fields::zero() };
        let a = attune(Element::Water, 1.0, &f);
        assert_eq!(a.sub_discipline, None, "Ice should NOT express when R is high");
    }

    #[test]
    fn gale_expresses_when_w_and_p_high() {
        let f = Fields { w: 1.0, p: 1.0, ..Fields::zero() };
        assert_eq!(attune(Element::Air, 1.0, &f).sub_discipline, Some(2));
    }

    #[test]
    fn thunder_expresses_when_l_and_w_high() {
        let f = Fields { l: 1.0, w: 1.0, ..Fields::zero() };
        assert_eq!(attune(Element::Storm, 1.0, &f).sub_discipline, Some(4));
    }

    #[test]
    fn blight_expresses_when_b_and_s_high() {
        let f = Fields { b: 1.0, s: 1.0, ..Fields::zero() };
        assert_eq!(attune(Element::Wood, 1.0, &f).sub_discipline, Some(5));
    }

    #[test]
    fn high_attune_monotone_in_primary_field() {
        // For Fire (primary=R), increasing R should increase scale at high attunement.
        let mut prev = -1.0f32;
        let mut r = 0.0f32;
        while r <= 1.0 {
            let f = Fields { r, ..Fields::zero() };
            let s = attune(Element::Fire, 1.0, &f).scale;
            assert!(s >= prev - 1e-5, "scale not monotone at r={r}: {prev} -> {s}");
            prev = s;
            r += 0.05;
        }
    }

    #[test]
    fn property_conservation_and_finite() {
        // LCG sweep (seed 0xA7C3_E19D, documented).
        let mut seed = 0xA7C3_E19Du32;
        let mut lcg = || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 8) as f32 / ((1u32 << 24) as f32)
        };
        let elements = [Element::Wood, Element::Water, Element::Air,
                        Element::Dust, Element::Fire, Element::Storm];
        for _ in 0..2000 {
            let el = elements[(lcg() * 6.0) as usize % 6];
            let att = lcg() * 1.5 - 0.25; // sometimes out of [0,1]
            let f = Fields { h: lcg(), r: lcg(), w: lcg(), p: lcg(),
                             l: lcg(), s: lcg(), b: lcg() };
            let a = attune(el, att, &f);
            assert!(a.scale.is_finite(), "non-finite scale");
            assert!(a.scale > 0.0, "non-positive scale");
            assert!(a.scale <= BASE + PEAK + 1e-5,
                "scale exceeds max: {}", a.scale);
        }
    }
}
