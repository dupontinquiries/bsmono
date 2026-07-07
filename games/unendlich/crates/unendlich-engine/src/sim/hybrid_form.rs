//! Dual-element **Hybrid Form** resolution (spec §8) — R36, PLY (player mechanics).
//!
//! A hybrid form (Tide-Stone, Storm-Body, …) fuses two of the bender's seven
//! disciplines. Per §8 it becomes available only when **both** constituent
//! disciplines are strongly accessible (each strength ≥ [`THRESHOLD`]) **and**
//! the bender's resonance has reached **tier 3**. Among every qualifying pair
//! the strongest is chosen — "strongest" = the highest *minimum* of the two
//! disciplines' strengths, i.e. the pair whose weaker half is still the
//! strongest weak-half available (a fusion is only as solid as its weaker leg).
//!
//! This is the capstone of the bending player loop:
//! `affinity_access` (what's accessible) + `resonance_state` (how charged) →
//! `hybrid_form` (which §8 fusion ignites). Self-contained and pure: discipline
//! strengths + resonance tier in, a form decision out. No RNG, I/O, GPU, or
//! wall-clock; deterministic; all outputs finite; never panics.

/// Number of bending disciplines.
pub const NUM_DISCIPLINES: usize = 7;
/// Number of distinct unordered discipline pairs = C(7,2) = 21. Valid
/// `form_id`s are `0..NUM_FORMS`.
pub const NUM_FORMS: u8 = 21;
/// Per-discipline strength at/above which a discipline counts as "strongly
/// accessible" and can take part in a fusion.
pub const THRESHOLD: f32 = 0.6;
/// Resonance tier required for any hybrid form to ignite.
pub const REQUIRED_TIER: u8 = 3;
/// Base sustain cost of holding a hybrid form (always paid when active).
pub const COST_BASE: f32 = 1.0;
/// Additional sustain cost scaled by the fusion's binding strength (its weaker
/// leg). A stronger, more potent fusion costs more to hold.
pub const COST_GAIN: f32 = 2.0;

/// Per-discipline accessible strength (e.g. produced by `affinity_access`).
/// Index = discipline id in `0..NUM_DISCIPLINES`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Affinities {
    pub strength: [f32; NUM_DISCIPLINES],
}

/// The resolved hybrid form. When `active` is false the form is dormant and
/// `form_id`/`cost` are inert (0 / 0.0).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HybridForm {
    pub active: bool,
    /// Encodes the fused discipline pair; decode with [`pair_of`]. Only
    /// meaningful when `active`.
    pub form_id: u8,
    /// Sustain cost while held; strictly positive when `active`.
    pub cost: f32,
}

/// Non-finite → 0.0, then clamp into [0,1]. Guarantees finite, in-range inputs
/// so the resolver can't be skewed or panicked by NaN/∞ strengths.
#[inline]
fn sanitize(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

/// Canonical id of the unordered pair `(i, j)` with `i < j`, enumerated in
/// row-major order over `0 ≤ i < j < NUM_DISCIPLINES`. Inverse of [`pair_of`].
#[inline]
fn form_id_of(i: u8, j: u8) -> u8 {
    let (a, b) = if i <= j { (i, j) } else { (j, i) };
    let mut id: u8 = 0;
    for x in 0..NUM_DISCIPLINES as u8 {
        for y in (x + 1)..NUM_DISCIPLINES as u8 {
            if x == a && y == b {
                return id;
            }
            id += 1;
        }
    }
    0
}

/// The two discipline indices a form fuses. Inverse of the internal id
/// encoding. Out-of-range `form_id` (`>= NUM_FORMS`) is clamped to the last
/// valid form so this never panics and always returns a valid pair.
pub fn pair_of(form_id: u8) -> (u8, u8) {
    let target = if form_id < NUM_FORMS { form_id } else { NUM_FORMS - 1 };
    let mut id: u8 = 0;
    for i in 0..NUM_DISCIPLINES as u8 {
        for j in (i + 1)..NUM_DISCIPLINES as u8 {
            if id == target {
                return (i, j);
            }
            id += 1;
        }
    }
    (0, 1) // unreachable for NUM_DISCIPLINES >= 2; safe default, no panic
}

/// Resolve which §8 hybrid form (if any) ignites for the given discipline
/// strengths and resonance `tier`.
///
/// Returns the strongest qualifying fusion (highest minimum strength across its
/// two disciplines), with a strictly-positive sustain cost; if no pair has both
/// disciplines ≥ [`THRESHOLD`], or `tier < REQUIRED_TIER`, returns an inactive
/// form. Ties are broken deterministically by lowest `form_id`.
pub fn resolve(aff: &Affinities, tier: u8) -> HybridForm {
    let inactive = HybridForm { active: false, form_id: 0, cost: 0.0 };
    if tier < REQUIRED_TIER {
        return inactive;
    }

    let s: [f32; NUM_DISCIPLINES] = {
        let mut out = [0.0f32; NUM_DISCIPLINES];
        for k in 0..NUM_DISCIPLINES {
            out[k] = sanitize(aff.strength[k]);
        }
        out
    };

    let mut best_id: Option<u8> = None;
    let mut best_min = f32::NEG_INFINITY; // sentinel; the >= THRESHOLD gate decides qualification
    let mut id: u8 = 0;
    for i in 0..NUM_DISCIPLINES {
        for j in (i + 1)..NUM_DISCIPLINES {
            let pair_min = s[i].min(s[j]);
            // Both disciplines strongly accessible ⟺ their minimum ≥ THRESHOLD.
            if pair_min >= THRESHOLD && pair_min > best_min {
                best_min = pair_min;
                best_id = Some(id);
            }
            id += 1;
        }
    }

    match best_id {
        Some(form_id) => HybridForm {
            active: true,
            form_id,
            cost: COST_BASE + COST_GAIN * best_min,
        },
        None => inactive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn aff(strength: [f32; NUM_DISCIPLINES]) -> Affinities {
        Affinities { strength }
    }

    #[test]
    fn one_discipline_high_is_inactive() {
        let a = aff([0.9, 0.1, 0.2, 0.0, 0.3, 0.1, 0.0]);
        assert!(!resolve(&a, 3).active, "a single strong discipline cannot fuse");
    }

    #[test]
    fn both_high_but_low_tier_is_inactive() {
        let a = aff([0.9, 0.85, 0.0, 0.0, 0.0, 0.0, 0.0]);
        for tier in 0..REQUIRED_TIER {
            assert!(!resolve(&a, tier).active, "tier {tier} < 3 must stay dormant");
        }
    }

    #[test]
    fn both_high_and_tier3_activates_with_correct_pair_and_positive_cost() {
        let a = aff([0.9, 0.85, 0.1, 0.0, 0.0, 0.0, 0.0]);
        let r = resolve(&a, 3);
        assert!(r.active);
        assert_eq!(pair_of(r.form_id), (0, 1), "the only strong pair is (0,1)");
        assert!(r.cost > 0.0 && r.cost.is_finite(), "cost {} must be positive+finite", r.cost);
    }

    #[test]
    fn strongest_pair_chosen_when_several_qualify() {
        // Pairs over threshold: (0,1) min .7, (0,5) min .65, (1,5) min .65,
        // and (3,6) min .95 — the strongest weak-half wins.
        let a = aff([0.7, 0.72, 0.0, 0.95, 0.0, 0.66, 0.98]);
        let r = resolve(&a, 3);
        assert!(r.active);
        assert_eq!(pair_of(r.form_id), (3, 6), "highest min-strength pair");
        // cost reflects that binding strength (min = 0.95).
        assert!((r.cost - (COST_BASE + COST_GAIN * 0.95)).abs() < 1e-6);
    }

    #[test]
    fn form_id_pair_round_trips_and_covers_all_pairs() {
        let mut seen = std::collections::BTreeSet::new();
        for id in 0..NUM_FORMS {
            let (i, j) = pair_of(id);
            assert!(i < j && (j as usize) < NUM_DISCIPLINES, "pair ({i},{j}) malformed");
            assert_eq!(form_id_of(i, j), id, "round-trip failed for id {id}");
            assert!(seen.insert((i, j)), "duplicate pair for id {id}");
        }
        assert_eq!(seen.len(), NUM_FORMS as usize, "must enumerate all C(7,2) pairs");
    }

    #[test]
    fn pair_of_is_total_no_panic_out_of_range() {
        for id in 0..=u8::MAX {
            let (i, j) = pair_of(id);
            assert!(i < j && (j as usize) < NUM_DISCIPLINES);
        }
    }

    #[test]
    fn deterministic() {
        let a = aff([0.8, 0.9, 0.7, 0.61, 0.0, 0.75, 0.62]);
        let first = resolve(&a, 3);
        for _ in 0..100 {
            assert_eq!(resolve(&a, 3), first, "resolve must be deterministic");
        }
    }

    #[test]
    fn non_finite_and_out_of_range_strengths_are_clamped() {
        // NaN / ±∞ / negatives sanitize to 0 (drop out); the only strong, finite
        // disciplines are 3 (0.7) and 4 (0.8), so they are the pair that fuses.
        let a = aff([f32::NAN, f32::INFINITY, -5.0, 0.7, 0.8, 0.0, f32::NEG_INFINITY]);
        let r = resolve(&a, 3);
        assert!(r.active);
        assert_eq!(pair_of(r.form_id), (3, 4));
        assert!(r.cost.is_finite() && r.cost > 0.0);
    }

    #[test]
    fn above_one_strength_clamps_to_one() {
        // An out-of-range 2.0 is clamped to a valid strong 1.0, so it can fuse.
        let a = aff([2.0, 0.65, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let r = resolve(&a, 3);
        assert!(r.active);
        assert_eq!(pair_of(r.form_id), (0, 1));
        assert!((r.cost - (COST_BASE + COST_GAIN * 0.65)).abs() < 1e-6);
    }

    #[test]
    fn exactly_at_threshold_qualifies() {
        let a = aff([THRESHOLD, THRESHOLD, 0.0, 0.0, 0.0, 0.0, 0.0]);
        let r = resolve(&a, 3);
        assert!(r.active, "strength == THRESHOLD counts as strongly accessible");
        assert_eq!(pair_of(r.form_id), (0, 1));
    }

    #[test]
    fn nothing_strong_is_inactive() {
        let a = aff([0.5, 0.59, 0.1, 0.0, 0.2, 0.4, 0.3]);
        assert!(!resolve(&a, 5).active, "no pair clears the threshold");
    }
}
