//! `sim::artifact_imbue` — ordinary items slowly becoming field-amplifying
//! artifacts (Resonance Bending spec §9, §12).
//!
//! An item held by a sufficiently attuned bearer in a matching environment
//! accrues an *imbue counter* (0..=1). Once imbued, the artifact projects a
//! **Field Amplification** — a local nudge to the environment's field channels
//! for the element it has attuned to (§10.4 affinity poll): a worn ember-stone
//! literally makes the air around the bearer read hotter.
//!
//! Element palette matches [`super::elemental_multiplier`]:
//! `0=Wood 1=Water 2=Air 3=Dust 4=Fire 5=Storm`. Each element governs only
//! certain field channels; the rest stay zero:
//!
//! ```text
//! Wood (0) -> b, h      Dust  (3) -> s, p
//! Water(1) -> h         Fire  (4) -> r
//! Air  (2) -> w         Storm (5) -> w, r
//! ```
//!
//! The `l` channel is governed by no element and is always zero here.
//!
//! Pure and deterministic: data in, data out. No GPU, IO, clock, or RNG; no
//! `.unwrap()`/`.expect()` in non-test code; all arithmetic saturating.

/// Persisted imbue progress for one item.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImbueState {
    /// Imbue progress, saturating in `0..=1`.
    pub counter: f32,
    /// Attuned element, `0..5` (see palette above). Read modulo 6.
    pub element: u8,
    /// Artifact tier — scales the projected amplification.
    pub tier: u8,
}

/// A delta applied to the local environment field poll. One field per channel;
/// only the attuned element's channels are non-zero.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldDelta {
    pub h: f32,
    pub r: f32,
    pub w: f32,
    pub p: f32,
    pub l: f32,
    pub s: f32,
    pub b: f32,
}

impl FieldDelta {
    /// All channels zero.
    pub const fn zero() -> Self {
        FieldDelta { h: 0.0, r: 0.0, w: 0.0, p: 0.0, l: 0.0, s: 0.0, b: 0.0 }
    }
}

// ── Tunables (tune here, never in the formula) ───────────────────────────────

/// Minimum bearer tier for imbuing to make progress (an "attuned" bearer).
pub const MIN_BEARER_TIER: u8 = 2;
/// Minimum environment match for imbuing to make progress.
pub const MATCH_THRESHOLD: f32 = 0.5;
/// Counter gained per unit (match × time) while progressing — slow on purpose.
pub const IMBUE_RATE: f32 = 0.05;
/// Base amplification magnitude at counter 1, tier 0.
const AMP_BASE: f32 = 1.0;
/// Extra amplification magnitude per artifact tier.
const AMP_PER_TIER: f32 = 0.5;

#[inline]
fn clamp01(x: f32) -> f32 {
    crate::util::mathx::saturate(x) // r343 dedup
}

/// Advance the imbue counter for one tick.
///
/// The counter rises **only** when the bearer is attuned enough
/// (`bearer_tier >= MIN_BEARER_TIER`) *and* the live environment matches the
/// artifact's element well enough (`match01 >= MATCH_THRESHOLD`). Progress
/// speed scales with how good the match is. The counter saturates at `1.0` and
/// never decreases here. `element` and `tier` pass through unchanged.
///
/// Total for all inputs: `match01` is clamped to `0..=1`, and a non-finite or
/// non-positive `dt` contributes no progress (no time-travel, no NaN/overflow).
pub fn tick_imbue(state: ImbueState, match01: f32, bearer_tier: u8, dt: f32) -> ImbueState {
    let m = clamp01(match01);
    // Reject degenerate dt (NaN/inf/negative) — those make no progress.
    let dt = if dt.is_finite() && dt > 0.0 { dt } else { 0.0 };

    let gated_open = bearer_tier >= MIN_BEARER_TIER && m >= MATCH_THRESHOLD;
    let rise = if gated_open { IMBUE_RATE * m * dt } else { 0.0 };

    // Start from a sanitised current counter so a corrupt input can't poison
    // the result; saturate in 0..=1 (huge `rise` -> 1.0, never overflows).
    let base = clamp01(state.counter);
    let counter = clamp01(base + rise);

    ImbueState { counter, element: state.element, tier: state.tier }
}

/// The Field Amplification the artifact currently projects.
///
/// Magnitude `mag = counter * (AMP_BASE + AMP_PER_TIER * tier)` is routed to
/// only the channels the attuned element governs (see module docs). At
/// `counter == 0` every channel is zero. `mag` is monotonically non-decreasing
/// in both `counter` and `tier`, so each governed channel is too.
pub fn field_amplification(state: &ImbueState) -> FieldDelta {
    let counter = clamp01(state.counter);
    let mag = counter * (AMP_BASE + AMP_PER_TIER * state.tier as f32);

    let mut d = FieldDelta::zero();
    match state.element % 6 {
        0 => { d.b = mag; d.h = mag; }          // Wood
        1 => { d.h = mag; }                     // Water
        2 => { d.w = mag; }                     // Air
        3 => { d.s = mag; d.p = mag; }          // Dust
        4 => { d.r = mag; }                     // Fire
        _ => { d.w = mag; d.r = mag; }          // Storm (5)
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(counter: f32, element: u8, tier: u8) -> ImbueState {
        ImbueState { counter, element, tier }
    }

    fn channels(d: &FieldDelta) -> [f32; 7] {
        [d.h, d.r, d.w, d.p, d.l, d.s, d.b]
    }

    /// Which channel indices each element governs (h,r,w,p,l,s,b = 0..6).
    fn governed(element: u8) -> &'static [usize] {
        match element % 6 {
            0 => &[6, 0], // Wood: b, h
            1 => &[0],    // Water: h
            2 => &[2],    // Air: w
            3 => &[5, 3], // Dust: s, p
            4 => &[1],    // Fire: r
            _ => &[2, 1], // Storm: w, r
        }
    }

    #[test]
    fn counter_rises_only_when_match_and_tier_sufficient() {
        let s0 = st(0.0, 4, 3);
        // both sufficient -> rises
        let hi = tick_imbue(s0, 1.0, MIN_BEARER_TIER, 1.0);
        assert!(hi.counter > 0.0, "should progress when attuned + matching");
        // low match -> no progress
        let lo_m = tick_imbue(s0, MATCH_THRESHOLD - 0.01, MIN_BEARER_TIER, 1.0);
        assert_eq!(lo_m.counter, 0.0);
        // low tier -> no progress
        let lo_t = tick_imbue(s0, 1.0, MIN_BEARER_TIER - 1, 1.0);
        assert_eq!(lo_t.counter, 0.0);
        // passthrough of element/tier
        assert_eq!(hi.element, 4);
        assert_eq!(hi.tier, 3);
    }

    #[test]
    fn better_match_progresses_faster() {
        let s0 = st(0.0, 2, 2);
        let a = tick_imbue(s0, 0.6, 5, 1.0).counter;
        let b = tick_imbue(s0, 1.0, 5, 1.0).counter;
        assert!(b > a, "stronger match should gain more counter");
    }

    #[test]
    fn saturates_at_one() {
        let mut s = st(0.0, 1, 4);
        for _ in 0..10_000 {
            s = tick_imbue(s, 1.0, 5, 1.0);
        }
        assert_eq!(s.counter, 1.0);
        assert!(s.counter <= 1.0);
    }

    #[test]
    fn huge_dt_saturates_without_overflow() {
        let s = tick_imbue(st(0.0, 4, 1), 1.0, 5, 1.0e30);
        assert!(s.counter.is_finite());
        assert_eq!(s.counter, 1.0);
        // degenerate dt makes no progress and stays finite
        for bad in [f32::NAN, f32::INFINITY, -1.0e9] {
            let z = tick_imbue(st(0.3, 4, 1), 1.0, 5, bad);
            assert!(z.counter.is_finite());
            assert_eq!(z.counter, 0.3);
        }
    }

    #[test]
    fn zero_counter_zero_amplification() {
        for e in 0..6u8 {
            for t in 0..5u8 {
                let d = field_amplification(&st(0.0, e, t));
                assert_eq!(d, FieldDelta::zero(), "element {} tier {}", e, t);
            }
        }
    }

    #[test]
    fn only_governed_fields_nonzero() {
        for e in 0..6u8 {
            let d = field_amplification(&st(1.0, e, 3));
            let ch = channels(&d);
            let gov = governed(e);
            for (i, v) in ch.iter().enumerate() {
                if gov.contains(&i) {
                    assert!(*v > 0.0, "governed channel {} should be >0 (elem {})", i, e);
                } else {
                    assert_eq!(*v, 0.0, "ungoverned channel {} must be 0 (elem {})", i, e);
                }
            }
            // l (index 4) is never governed
            assert_eq!(d.l, 0.0);
        }
    }

    #[test]
    fn amplification_monotone_in_counter() {
        for e in 0..6u8 {
            let gov = governed(e);
            let mut prev = field_amplification(&st(0.0, e, 3));
            let mut c = 0.0f32;
            while c <= 1.0 {
                let cur = field_amplification(&st(c, e, 3));
                let pc = channels(&prev);
                let cc = channels(&cur);
                for &i in gov {
                    assert!(cc[i] >= pc[i] - 1e-7, "non-monotone in counter, elem {} ch {}", e, i);
                }
                prev = cur;
                c += 0.1;
            }
        }
    }

    #[test]
    fn amplification_monotone_in_tier() {
        for e in 0..6u8 {
            let gov = governed(e);
            let mut prev = field_amplification(&st(0.8, e, 0));
            for t in 1..=10u8 {
                let cur = field_amplification(&st(0.8, e, t));
                let pc = channels(&prev);
                let cc = channels(&cur);
                for &i in gov {
                    assert!(cc[i] >= pc[i] - 1e-7, "non-monotone in tier, elem {} ch {}", e, i);
                }
                prev = cur;
            }
        }
    }

    #[test]
    fn element_index_wraps() {
        // element read modulo 6: 11 == 5 (Storm)
        assert_eq!(field_amplification(&st(1.0, 11, 2)),
                   field_amplification(&st(1.0, 5, 2)));
    }

    #[test]
    fn finite_for_bounded_inputs() {
        // deterministic seeded sweep (R20)
        let mut x: u64 = 0xA17_1FAC7_u64; // documented seed
        let mut next = || { x = x.wrapping_mul(6364136223846793005).wrapping_add(1); (x >> 33) as u32 };
        for _ in 0..5_000 {
            let counter = (next() % 1000) as f32 / 500.0 - 1.0; // [-1, 1)
            let elem = (next() % 9) as u8;
            let tier = (next() % 12) as u8;
            let m = (next() % 1000) as f32 / 800.0 - 0.1;       // around [-0.1, 1.15)
            let dt = (next() % 1000) as f32 / 100.0;            // [0, 10)
            let ticked = tick_imbue(st(counter, elem, tier), m, (next() % 8) as u8, dt);
            assert!(ticked.counter.is_finite() && (0.0..=1.0).contains(&ticked.counter));
            for v in channels(&field_amplification(&ticked)).iter() {
                assert!(v.is_finite());
            }
        }
    }

    #[test]
    fn deterministic_bits() {
        let a = tick_imbue(st(0.2, 4, 3), 0.8, 4, 0.5);
        let b = tick_imbue(st(0.2, 4, 3), 0.8, 4, 0.5);
        assert_eq!(a.counter.to_bits(), b.counter.to_bits());
        let fa = field_amplification(&a);
        let fb = field_amplification(&b);
        assert_eq!(fa, fb);
    }
}
