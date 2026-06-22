//! Bending **focus / relic** modifier aggregation (R39, ITM).
//!
//! The bending runtime (`sim/bending_runtime.rs`) runs one tick as
//! `fields → affinity_access → resonance_state → hybrid_form` (+
//! `gesture_resolve`). Equipped bending **items** modify that tick: a focus or
//! relic boosts a discipline, speeds resonance gain, or — as an imbued artifact
//! (`artifact_imbue`) — projects an elemental field. This core is the (formerly
//! absent) aggregator that folds every equipped item into one [`FocusMods`] the
//! runtime applies.
//!
//! Intended use in `bending_runtime`:
//! - add [`FocusMods::field_delta`] to the ambient `fields` **before**
//!   `affinity_access` (its source is typically `artifact_imbue`),
//! - add [`FocusMods::strength_add`] to the per-discipline `access.strength`,
//! - scale the resonance gain by [`FocusMods::resonance_gain_mul`].
//!
//! Pure & deterministic: same items → same mods, every call. Saturating and
//! clamped; non-finite inputs are neutralised; out-of-range disciplines are
//! ignored. No RNG, I/O, GPU, wall-clock; never panics.

/// Number of bending disciplines / field channels.
pub const NUM_DISCIPLINES: usize = 7;

/// Resonance-gain multiplier is clamped to `[RES_MUL_MIN, RES_MUL_MAX]`.
pub const RES_MUL_MIN: f32 = 0.0;
pub const RES_MUL_MAX: f32 = 4.0;

/// Saturating guard on the aggregated additive channels, purely to keep outputs
/// finite under pathological inputs (huge magnitudes / item counts). Generous
/// enough never to clip realistic gear.
pub const ADD_LIMIT: f32 = 1.0e6;

/// One equipped bending item's contribution.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FocusItem {
    /// Discipline this item favours (`0..NUM_DISCIPLINES`). Out-of-range is
    /// ignored for the strength bonus (the rest of the item still applies).
    pub discipline: u8,
    /// Additive bonus to that discipline's affinity strength.
    pub strength_bonus: f32,
    /// Multiplier on resonance gain (`1.0` = neutral).
    pub resonance_gain_mul: f32,
    /// Additive field projection (e.g. from `artifact_imbue`), per channel.
    pub field_delta: [f32; NUM_DISCIPLINES],
}

/// The aggregated modifier set the runtime applies for this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FocusMods {
    /// Per-discipline strength bonus, summed across items.
    pub strength_add: [f32; NUM_DISCIPLINES],
    /// Product of item multipliers, clamped to `[RES_MUL_MIN, RES_MUL_MAX]`.
    pub resonance_gain_mul: f32,
    /// Per-channel summed field projection, applied to fields before access.
    pub field_delta: [f32; NUM_DISCIPLINES],
}

impl FocusMods {
    /// The neutral modifier set: no adds, neutral multiplier, no field delta.
    pub fn identity() -> FocusMods {
        FocusMods {
            strength_add: [0.0; NUM_DISCIPLINES],
            resonance_gain_mul: 1.0,
            field_delta: [0.0; NUM_DISCIPLINES],
        }
    }
}

/// Non-finite → 0.0. Defined unconditionally (used on the normal aggregation
/// path, including release builds).
#[inline]
fn finite_or_zero(v: f32) -> f32 {
    if v.is_finite() {
        v
    } else {
        0.0
    }
}

/// Aggregate all equipped bending items into one [`FocusMods`]. Empty slice →
/// [`FocusMods::identity`].
pub fn focus(items: &[FocusItem]) -> FocusMods {
    let mut out = FocusMods::identity();

    for item in items {
        // Strength bonus → its discipline (out-of-range ignored, no OOB).
        let d = item.discipline as usize;
        if d < NUM_DISCIPLINES {
            out.strength_add[d] += finite_or_zero(item.strength_bonus);
        }

        // Resonance multiplier: a non-finite item mul is treated as neutral
        // (1.0) so a bad item can't wipe the product; negatives clamp to 0.
        let m = if item.resonance_gain_mul.is_finite() {
            item.resonance_gain_mul.max(0.0)
        } else {
            1.0
        };
        out.resonance_gain_mul *= m;

        // Field projection: per-channel sum.
        for c in 0..NUM_DISCIPLINES {
            out.field_delta[c] += finite_or_zero(item.field_delta[c]);
        }
    }

    // Saturate to keep everything finite and the multiplier in its sane range.
    for c in 0..NUM_DISCIPLINES {
        out.strength_add[c] = out.strength_add[c].clamp(-ADD_LIMIT, ADD_LIMIT);
        out.field_delta[c] = out.field_delta[c].clamp(-ADD_LIMIT, ADD_LIMIT);
    }
    out.resonance_gain_mul = out.resonance_gain_mul.clamp(RES_MUL_MIN, RES_MUL_MAX);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(discipline: u8, strength_bonus: f32, mul: f32, field: [f32; NUM_DISCIPLINES]) -> FocusItem {
        FocusItem { discipline, strength_bonus, resonance_gain_mul: mul, field_delta: field }
    }

    const Z: [f32; NUM_DISCIPLINES] = [0.0; NUM_DISCIPLINES];

    #[test]
    fn empty_is_identity() {
        let m = focus(&[]);
        assert_eq!(m, FocusMods::identity());
        assert_eq!(m.strength_add, Z);
        assert_eq!(m.resonance_gain_mul, 1.0);
        assert_eq!(m.field_delta, Z);
    }

    #[test]
    fn strength_bonus_lands_on_right_index_and_sums() {
        let items = [
            item(2, 0.3, 1.0, Z),
            item(2, 0.2, 1.0, Z),
            item(5, 0.5, 1.0, Z),
        ];
        let m = focus(&items);
        assert!((m.strength_add[2] - 0.5).abs() < 1e-6);
        assert!((m.strength_add[5] - 0.5).abs() < 1e-6);
        for d in [0, 1, 3, 4, 6] {
            assert_eq!(m.strength_add[d], 0.0);
        }
    }

    #[test]
    fn resonance_gain_mul_multiplies_and_clamps() {
        // 2 * 2 = 4 (at the cap)
        let m = focus(&[item(0, 0.0, 2.0, Z), item(0, 0.0, 2.0, Z)]);
        assert!((m.resonance_gain_mul - 4.0).abs() < 1e-6);
        // 2 * 2 * 2 = 8 → clamped to RES_MUL_MAX
        let m2 = focus(&[item(0, 0.0, 2.0, Z), item(0, 0.0, 2.0, Z), item(0, 0.0, 2.0, Z)]);
        assert!((m2.resonance_gain_mul - RES_MUL_MAX).abs() < 1e-6);
        // a 0.0 multiplier nullifies resonance gain
        let m3 = focus(&[item(0, 0.0, 3.0, Z), item(0, 0.0, 0.0, Z)]);
        assert_eq!(m3.resonance_gain_mul, 0.0);
        // a single neutral item leaves it at 1.0
        assert_eq!(focus(&[item(1, 0.1, 1.0, Z)]).resonance_gain_mul, 1.0);
    }

    #[test]
    fn field_delta_sums_per_channel() {
        let a = [0.1, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0];
        let b = [0.2, 0.5, 0.0, 0.0, 0.0, 0.0, -0.3];
        let m = focus(&[item(0, 0.0, 1.0, a), item(0, 0.0, 1.0, b)]);
        assert!((m.field_delta[0] - 0.3).abs() < 1e-6);
        assert!((m.field_delta[1] - 0.5).abs() < 1e-6);
        assert!((m.field_delta[6] - (-0.3)).abs() < 1e-6);
        for c in [2, 3, 4, 5] {
            assert_eq!(m.field_delta[c], 0.0);
        }
    }

    #[test]
    fn out_of_range_discipline_ignored_no_oob() {
        // discipline 7 and 200 are out of range: their strength_bonus is
        // dropped, but field_delta + mul still apply, and nothing panics.
        let f = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0];
        let m = focus(&[item(7, 9.9, 2.0, f), item(200, 5.0, 1.0, Z)]);
        assert_eq!(m.strength_add, Z, "OOB discipline must not contribute strength");
        assert!((m.field_delta[6] - 1.0).abs() < 1e-6, "OOB item's field still applies");
        assert!((m.resonance_gain_mul - 2.0).abs() < 1e-6, "OOB item's mul still applies");
    }

    #[test]
    fn non_finite_inputs_are_neutralised_and_outputs_finite() {
        let bad_field = [f32::NAN, f32::INFINITY, 0.0, 0.0, 0.0, 0.0, 0.0];
        let m = focus(&[
            item(0, f32::NAN, f32::INFINITY, bad_field),
            item(1, 0.4, 2.0, Z),
        ]);
        // NaN strength → 0; the good item's 0.4 on discipline 1 survives.
        assert_eq!(m.strength_add[0], 0.0);
        assert!((m.strength_add[1] - 0.4).abs() < 1e-6);
        // ∞ mul on item 0 treated as neutral (1.0); item 1 contributes 2.0.
        assert!((m.resonance_gain_mul - 2.0).abs() < 1e-6);
        // every output channel finite.
        assert!(m.strength_add.iter().all(|v| v.is_finite()));
        assert!(m.field_delta.iter().all(|v| v.is_finite()));
        assert!(m.resonance_gain_mul.is_finite());
    }

    #[test]
    fn deterministic() {
        let items = [
            item(3, 0.6, 1.5, [0.0, 0.1, 0.0, 0.2, 0.0, 0.0, 0.0]),
            item(3, 0.1, 0.8, Z),
            item(9, 2.0, 2.0, [0.0; NUM_DISCIPLINES]),
        ];
        let first = focus(&items);
        for _ in 0..100 {
            assert_eq!(focus(&items), first);
        }
    }

    #[test]
    fn saturating_guard_keeps_adds_finite() {
        // A pathological pile of huge bonuses must clamp, not overflow to ∞.
        let big = [f32::MAX; NUM_DISCIPLINES];
        let items = [item(0, f32::MAX, 1.0, big); 64];
        let m = focus(&items);
        assert!(m.strength_add.iter().all(|v| v.is_finite()));
        assert!(m.field_delta.iter().all(|v| v.is_finite()));
        assert_eq!(m.strength_add[0], ADD_LIMIT);
        assert_eq!(m.field_delta[0], ADD_LIMIT);
    }
}
