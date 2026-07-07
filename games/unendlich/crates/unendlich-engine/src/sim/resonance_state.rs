//! `sim::resonance_state` — a bender's Resonance, the core bending resource
//! (spec §4, §10.3).
//!
//! Coherence is a `0..=3` value that **rises** with stillness and a matching
//! environment and **falls** with technique cost (`spent`) and incoming
//! `damage`. Its integer *tier* gates technique power: it feeds the charge term
//! in `gesture_resolve` and the tier gate in `hybrid_form`. Stand still in a
//! matched field to charge up; take a hit and your hybrid form drops.
//!
//! Pure and deterministic: data in, data out. No GPU, IO, clock, or RNG; no
//! `.unwrap()`/`.expect()` in non-test code; every input is sanitised so the
//! result is always finite and in `0..=3`.

/// A bender's resonance. `coherence` is held in `0..=3`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Resonance {
    pub coherence: f32,
}

/// One update tick's drivers.
pub struct Inputs {
    /// How still the bender is, `0..=1` (1 = perfectly still).
    pub stillness01: f32,
    /// How well the live environment matches the bender's element, `0..=1`.
    pub match01: f32,
    /// Coherence spent this tick on techniques (≥ 0).
    pub spent: f32,
    /// Incoming damage this tick (≥ 0).
    pub damage: f32,
    /// Timestep (seconds, ≥ 0). Scales the *gain* term only.
    pub dt: f32,
}

// ── Tunables (tune here, never in the formula) ───────────────────────────────

/// Coherence gained per second at full stillness.
pub const GAIN_STILL: f32 = 0.6;
/// Coherence gained per second at a full environment match.
pub const GAIN_MATCH: f32 = 0.4;
/// Maximum coherence (and the start of the top tier).
pub const MAX_COHERENCE: f32 = 3.0;

#[inline]
fn fin(x: f32, fallback: f32) -> f32 {
    if x.is_finite() { x } else { fallback }
}
#[inline]
fn clamp(x: f32, lo: f32, hi: f32) -> f32 {
    x.max(lo).min(hi)
}
#[inline]
fn clamp01(x: f32) -> f32 {
    clamp(x, 0.0, 1.0)
}

/// Advance resonance one tick:
///
/// ```text
/// coherence += dt * (GAIN_STILL*stillness01 + GAIN_MATCH*match01)
///            - (spent + damage)
/// ```
///
/// then clamp to `0..=MAX_COHERENCE`. The gain is `dt`-scaled; the cost
/// (`spent + damage`) is an instantaneous subtraction. All inputs are
/// sanitised: `stillness01`/`match01` clamp to `0..=1`, `spent`/`damage`/`dt`
/// floor at `0`, and any non-finite input (incl. the stored coherence) is
/// treated as `0`, so the output is always finite and in range.
pub fn update(r: Resonance, i: &Inputs) -> Resonance {
    let still = clamp01(fin(i.stillness01, 0.0));
    let matched = clamp01(fin(i.match01, 0.0));
    let spent = fin(i.spent, 0.0).max(0.0);
    let damage = fin(i.damage, 0.0).max(0.0);
    let dt = fin(i.dt, 0.0).max(0.0);

    let base = clamp(fin(r.coherence, 0.0), 0.0, MAX_COHERENCE);
    let gain = dt * (GAIN_STILL * still + GAIN_MATCH * matched);
    let coherence = clamp(base + gain - (spent + damage), 0.0, MAX_COHERENCE);

    Resonance { coherence }
}

/// Resonance tier `0..=3` from coherence bands (lower-inclusive):
///
/// ```text
/// tier 0 : coherence <  1.0
/// tier 1 : 1.0 <= coherence < 2.0
/// tier 2 : 2.0 <= coherence < 3.0
/// tier 3 : coherence >= 3.0   (fully charged — coherence is capped at 3.0)
/// ```
pub fn tier(r: &Resonance) -> u8 {
    let c = clamp(fin(r.coherence, 0.0), 0.0, MAX_COHERENCE);
    if c >= 3.0 {
        3
    } else if c >= 2.0 {
        2
    } else if c >= 1.0 {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn res(c: f32) -> Resonance { Resonance { coherence: c } }
    fn inputs(stillness01: f32, match01: f32, spent: f32, damage: f32, dt: f32) -> Inputs {
        Inputs { stillness01, match01, spent, damage, dt }
    }

    #[test]
    fn stillness_and_match_raise_coherence() {
        let start = res(0.5);
        let up = update(start, &inputs(1.0, 1.0, 0.0, 0.0, 1.0));
        assert!(up.coherence > start.coherence);
        // each driver alone also helps
        assert!(update(start, &inputs(1.0, 0.0, 0.0, 0.0, 1.0)).coherence > start.coherence);
        assert!(update(start, &inputs(0.0, 1.0, 0.0, 0.0, 1.0)).coherence > start.coherence);
        // no drivers, no cost -> unchanged
        assert_eq!(update(start, &inputs(0.0, 0.0, 0.0, 0.0, 1.0)).coherence, start.coherence);
    }

    #[test]
    fn spent_and_damage_lower_coherence() {
        let start = res(2.0);
        assert!(update(start, &inputs(0.0, 0.0, 0.5, 0.0, 1.0)).coherence < start.coherence);
        assert!(update(start, &inputs(0.0, 0.0, 0.0, 0.5, 1.0)).coherence < start.coherence);
        // a big enough hit overpowers a full charge tick (gain at dt=1 is 1.0)
        let hit = update(res(1.2), &inputs(1.0, 1.0, 0.0, 1.5, 1.0));
        assert!(hit.coherence < 1.2);
    }

    #[test]
    fn clamps_at_zero_and_max() {
        // huge cost -> floors at 0
        assert_eq!(update(res(1.0), &inputs(0.0, 0.0, 100.0, 100.0, 1.0)).coherence, 0.0);
        // huge gain -> caps at MAX
        assert_eq!(update(res(0.0), &inputs(1.0, 1.0, 0.0, 0.0, 1.0e6)).coherence, MAX_COHERENCE);
        // already-overlarge stored value is clamped down on read
        assert_eq!(update(res(9.9), &inputs(0.0, 0.0, 0.0, 0.0, 1.0)).coherence, MAX_COHERENCE);
    }

    #[test]
    fn tier_bands_correct_at_boundaries() {
        assert_eq!(tier(&res(0.0)), 0);
        assert_eq!(tier(&res(0.999)), 0);
        assert_eq!(tier(&res(1.0)), 1);
        assert_eq!(tier(&res(1.999)), 1);
        assert_eq!(tier(&res(2.0)), 2);
        assert_eq!(tier(&res(2.999)), 2);
        assert_eq!(tier(&res(3.0)), 3);
        // out-of-range stored values still map sanely
        assert_eq!(tier(&res(-5.0)), 0);
        assert_eq!(tier(&res(99.0)), 3);
    }

    #[test]
    fn dt_scales_the_gain() {
        let start = res(0.0);
        let a = update(start, &inputs(1.0, 1.0, 0.0, 0.0, 0.1)).coherence;
        let b = update(start, &inputs(1.0, 1.0, 0.0, 0.0, 0.2)).coherence;
        // twice the dt -> twice the gain (no cost), within fp tolerance
        assert!((b - 2.0 * a).abs() < 1e-6, "a={} b={}", a, b);
        // dt = 0 -> no gain (cost still applies)
        assert_eq!(update(res(1.0), &inputs(1.0, 1.0, 0.0, 0.0, 0.0)).coherence, 1.0);
        assert_eq!(update(res(1.0), &inputs(1.0, 1.0, 0.3, 0.0, 0.0)).coherence, 0.7);
    }

    #[test]
    fn nan_and_inf_inputs_clamp_to_finite_range() {
        let bad = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY];
        for &x in &bad {
            let out = update(res(1.5), &inputs(x, x, x, x, x));
            assert!(out.coherence.is_finite());
            assert!((0.0..=MAX_COHERENCE).contains(&out.coherence));
            // non-finite stored coherence is sanitised too
            let out2 = update(res(x), &inputs(0.0, 0.0, 0.0, 0.0, 1.0));
            assert!(out2.coherence.is_finite() && (0.0..=MAX_COHERENCE).contains(&out2.coherence));
        }
    }

    #[test]
    fn negative_inputs_are_floored() {
        // negative spent/damage/dt can't add coherence
        let start = res(1.0);
        let out = update(start, &inputs(0.5, 0.5, -10.0, -10.0, 1.0));
        // cost floored at 0, so this is gain-only -> >= start
        assert!(out.coherence >= start.coherence);
        // negative dt contributes no gain
        assert_eq!(update(start, &inputs(1.0, 1.0, 0.0, 0.0, -5.0)).coherence, start.coherence);
    }

    #[test]
    fn finite_for_bounded_inputs() {
        // deterministic seeded sweep (R20)
        let mut x: u64 = 0x2E50_7A7E_u64; // documented seed
        let mut next = || { x = x.wrapping_mul(6364136223846793005).wrapping_add(1); (x >> 33) as u32 };
        let span = |v: u32, lo: f32, hi: f32| lo + (v % 1000) as f32 / 1000.0 * (hi - lo);
        for _ in 0..5_000 {
            let r = res(span(next(), -1.0, 4.0));
            let i = inputs(
                span(next(), -0.2, 1.2),
                span(next(), -0.2, 1.2),
                span(next(), -1.0, 5.0),
                span(next(), -1.0, 5.0),
                span(next(), -1.0, 4.0),
            );
            let out = update(r, &i);
            assert!(out.coherence.is_finite());
            assert!((0.0..=MAX_COHERENCE).contains(&out.coherence));
            assert!(tier(&out) <= 3);
        }
    }

    #[test]
    fn deterministic_bits() {
        let a = update(res(1.1), &inputs(0.7, 0.3, 0.2, 0.1, 0.5));
        let b = update(res(1.1), &inputs(0.7, 0.3, 0.2, 0.1, 0.5));
        assert_eq!(a.coherence.to_bits(), b.coherence.to_bits());
        assert_eq!(tier(&a), tier(&b));
    }
}
